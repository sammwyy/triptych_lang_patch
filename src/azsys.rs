// AZSys engine (ALcot) core library for Triptych (2014 Download Edition).
//
// Handles:
//   - ARC archive parsing/building (AZSys "ARC\x1a" container, LZSS-packed index)
//   - ASB script decryption/encryption (zlib + additive XOR-derived key)
//
// This is a direct port of the reference azsys.py implementation.

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use flate2::Crc;

use crate::error::PatchError;

pub const TRIPTYCH_KEY: u32 = 501685433; // 0x1DE71CB9

// Read a little-endian u32 from `buf` at `pos`.
fn u32_at(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

// Read a little-endian u16 from `buf` at `pos`.
fn u16_at(buf: &[u8], pos: usize) -> u16 {
    u16::from_le_bytes([buf[pos], buf[pos + 1]])
}

// Compute the zlib CRC32 of `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

// ---------------------------------------------------------------------------
// Index (de)compression - LZSS variant with separate control/offset/literal
// streams.
// ---------------------------------------------------------------------------

// Unpack the LZSS-compressed ARC index.
//
// Packed layout:
//   +0x00 u32 CRC32 of packed[0x14:]
//   +0x04 u32 control stream length
//   +0x08 u32 offset (compr1) stream length
//   +0x0C u32 literal (compr2) stream length
//   +0x10 u32 unpacked output length
//   +0x14 streams: control | compr1 | compr2
pub fn unpack_index(packed: &[u8]) -> Vec<u8> {
    let control_len = u32_at(packed, 4) as usize;
    let compr1_len = u32_at(packed, 8) as usize;
    let output_len = u32_at(packed, 0x10) as usize;
    let mut out = vec![0u8; output_len];

    let mut control = 0x14usize;
    let mut compr1 = control + control_len;
    let mut compr2 = compr1 + compr1_len;
    let mut dst = 0usize;
    let mut mask: u8 = 0x80;
    while dst < output_len {
        if packed[control] & mask != 0 {
            let raw = u16_at(packed, compr1);
            compr1 += 2;
            let count = ((raw >> 13) + 3) as usize;
            let offset = ((raw & 0x1FFF) + 1) as usize;
            let mut src = dst - offset;
            for _ in 0..count {
                out[dst] = out[src];
                dst += 1;
                src += 1;
            }
        } else {
            let count = packed[compr2] as usize + 1;
            compr2 += 1;
            out[dst..dst + count].copy_from_slice(&packed[compr2..compr2 + count]);
            compr2 += count;
            dst += count;
        }
        mask >>= 1;
        if mask == 0 {
            control += 1;
            mask = 0x80;
        }
    }
    out
}

// Pack data in the ARC index format.
//
// Emits literal-only runs (control bits all 0). This is always a valid
// encoding of the format; the engine only cares that it unpacks correctly
// and that the CRC matches.
pub fn pack_index(data: &[u8]) -> Vec<u8> {
    let mut control: Vec<u8> = Vec::new();
    let mut compr2: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut bit_count = 0u32;
    let mut cur_byte: u8 = 0;
    while pos < data.len() {
        let chunk = std::cmp::min(256, data.len() - pos);
        compr2.push((chunk - 1) as u8);
        compr2.extend_from_slice(&data[pos..pos + chunk]);
        pos += chunk;
        // control bit 0 = literal run
        cur_byte <<= 1;
        bit_count += 1;
        if bit_count == 8 {
            control.push(cur_byte);
            cur_byte = 0;
            bit_count = 0;
        }
    }
    if bit_count != 0 {
        cur_byte <<= 8 - bit_count;
        control.push(cur_byte);
    }

    let mut packed: Vec<u8> = Vec::with_capacity(0x14 + control.len() + compr2.len());
    packed.extend_from_slice(&0u32.to_le_bytes()); // CRC placeholder
    packed.extend_from_slice(&(control.len() as u32).to_le_bytes());
    packed.extend_from_slice(&0u32.to_le_bytes()); // compr1 (offsets) length - none
    packed.extend_from_slice(&(compr2.len() as u32).to_le_bytes());
    packed.extend_from_slice(&(data.len() as u32).to_le_bytes());
    packed.extend_from_slice(&control);
    packed.extend_from_slice(&compr2);

    let crc = crc32(&packed[0x14..]);
    packed[0..4].copy_from_slice(&crc.to_le_bytes());
    packed
}

// ---------------------------------------------------------------------------
// ARC container
// ---------------------------------------------------------------------------

pub struct ArcEntry {
    pub name: String,
    pub data: Vec<u8>,
    // First 0x10 bytes of the index record (offset, size, ...) get rewritten
    // on build; keep the raw record to preserve unknown fields.
    pub raw_index: Vec<u8>,
}

pub struct Arc {
    pub entries: Vec<ArcEntry>,
    header: Vec<u8>,
}

impl Arc {
    // Parse an ARC container from raw file bytes.
    pub fn load(blob: &[u8]) -> Result<Arc, PatchError> {
        if blob.len() < 0x30 {
            return Err(PatchError::Format("ARC file too small".into()));
        }
        let header = blob[..0x30].to_vec();
        let count = u32_at(blob, 8) as usize;
        let idx_len = u32_at(blob, 12) as usize;
        let packed_idx = &blob[0x30..0x30 + idx_len];
        let index = unpack_index(packed_idx);
        let base = 0x30 + idx_len;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let rec = &index[i * 0x40..(i + 1) * 0x40];
            let offset = u32_at(rec, 0) as usize;
            let size = u32_at(rec, 4) as usize;
            let name_raw = &rec[0x10..0x40];
            let name_end = name_raw.iter().position(|&b| b == 0).unwrap_or(name_raw.len());
            let (name, _, _) = encoding_rs::SHIFT_JIS.decode(&name_raw[..name_end]);
            let name = name.trim().to_string();
            let data = blob[base + offset..base + offset + size].to_vec();
            entries.push(ArcEntry {
                name,
                data,
                raw_index: rec.to_vec(),
            });
        }
        Ok(Arc { entries, header })
    }

    // Serialize the container back to raw file bytes.
    pub fn save(&self) -> Vec<u8> {
        let mut index: Vec<u8> = Vec::new();
        let mut body: Vec<u8> = Vec::new();
        for e in &self.entries {
            let mut rec = e.raw_index.clone();
            rec[0..4].copy_from_slice(&(body.len() as u32).to_le_bytes());
            rec[4..8].copy_from_slice(&(e.data.len() as u32).to_le_bytes());
            index.extend_from_slice(&rec);
            body.extend_from_slice(&e.data);
        }
        let packed_idx = pack_index(&index);
        let mut header = self.header.clone();
        header[8..12].copy_from_slice(&(self.entries.len() as u32).to_le_bytes());
        header[12..16].copy_from_slice(&(packed_idx.len() as u32).to_le_bytes());

        let mut out = Vec::with_capacity(header.len() + packed_idx.len() + body.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&packed_idx);
        out.extend_from_slice(&body);
        out
    }
}

// ---------------------------------------------------------------------------
// ASB script encryption
// ---------------------------------------------------------------------------

// Derive the per-script additive key from the game key and the unpacked size.
fn derive_key(key: u32, unpacked_size: u32) -> u32 {
    let mut k = (key ^ unpacked_size) as u64;
    k ^= ((k << 12) | k) << 11;
    (k & 0xFFFF_FFFF) as u32
}

// ASB\x1a container -> decompressed script bytes.
pub fn decrypt_asb(asb: &[u8], key: u32) -> Result<Vec<u8>, PatchError> {
    if asb.len() < 12 || &asb[..4] != b"ASB\x1a" {
        return Err(PatchError::Format("bad ASB signature".into()));
    }
    let packed_size = u32_at(asb, 4) as usize;
    let unpacked_size = u32_at(asb, 8);
    let k = derive_key(key, unpacked_size);
    let enc = &asb[12..12 + packed_size];
    let mut dec = vec![0u8; enc.len()];

    let mut i = 0usize;
    while i + 4 <= enc.len() {
        let v = u32_at(enc, i);
        let d = v.wrapping_sub(k);
        dec[i..i + 4].copy_from_slice(&d.to_le_bytes());
        i += 4;
    }
    let rem = enc.len() % 4;
    if rem != 0 {
        let n = enc.len();
        dec[n - rem..].copy_from_slice(&enc[n - rem..]);
    }

    // dec[0:4] = CRC32 of the zlib stream
    let crc = u32_at(&dec, 0);
    let payload = &dec[4..];
    if crc32(payload) != crc {
        return Err(PatchError::Format("ASB CRC mismatch after decrypt".into()));
    }
    let mut out = Vec::new();
    ZlibDecoder::new(payload)
        .read_to_end(&mut out)
        .map_err(|e| PatchError::Format(format!("zlib decompress failed: {e}")))?;
    if out.len() != unpacked_size as usize {
        return Err(PatchError::Format("ASB size mismatch".into()));
    }
    Ok(out)
}

// Decompressed script bytes -> ASB\x1a container.
pub fn encrypt_asb(script: &[u8], key: u32) -> Result<Vec<u8>, PatchError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(9));
    encoder
        .write_all(script)
        .map_err(|e| PatchError::Format(format!("zlib compress failed: {e}")))?;
    let compressed = encoder
        .finish()
        .map_err(|e| PatchError::Format(format!("zlib compress failed: {e}")))?;
    let crc = crc32(&compressed);

    let mut plain: Vec<u8> = Vec::with_capacity(4 + compressed.len());
    plain.extend_from_slice(&crc.to_le_bytes());
    plain.extend_from_slice(&compressed);

    let k = derive_key(key, script.len() as u32);
    let mut enc = vec![0u8; plain.len()];
    let mut i = 0usize;
    while i + 4 <= plain.len() {
        let v = u32_at(&plain, i);
        let e = v.wrapping_add(k);
        enc[i..i + 4].copy_from_slice(&e.to_le_bytes());
        i += 4;
    }
    let rem = plain.len() % 4;
    if rem != 0 {
        let n = plain.len();
        enc[n - rem..].copy_from_slice(&plain[n - rem..]);
    }

    let mut out: Vec<u8> = Vec::with_capacity(12 + enc.len());
    out.extend_from_slice(b"ASB\x1a");
    out.extend_from_slice(&(enc.len() as u32).to_le_bytes());
    out.extend_from_slice(&(script.len() as u32).to_le_bytes());
    out.extend_from_slice(&enc);
    Ok(out)
}
