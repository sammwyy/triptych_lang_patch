// ASB bytecode parser/rebuilder for Triptych (AZSys engine).
//
// Instruction format (validated against all 347 scripts in script.arc):
//     +0  u32  opcode
//     +4  u32  total record length (header included)
//     +8  ...  args
// The file ends with a terminator record: opcode=0, length=0xFFFFFFFF,
// followed by the remaining bytes (FF FF FF FF + zero padding).
//
// Text-bearing opcodes:
//     0x04 title    args = 16-byte prefix + cp932 string + NUL
//     0x29 speaker  args = 16-byte prefix + cp932 string + NUL (only if len>0x18)
//     0x2a text     args = 16-byte prefix + cp932 string + NUL  (dialogue)
//     0x0d choice   args = 16-byte prefix (+4 = option count) + N strings
// (op 0x30 looks text-like but is actually an RGB color value - not text)
//
// Jump opcodes carrying absolute byte offsets into the same script (must be
// relocated when record sizes change):
//     0x0a, 0x0b, 0x0c   args+0 = u32 target offset (start of an instruction,
//                        may be the terminator record)

use std::collections::HashMap;

use crate::error::PatchError;

pub const TERMINATOR_LEN: u32 = 0xFFFFFFFF;
pub const JUMP_OPS: [u32; 3] = [0x0a, 0x0b, 0x0c];

// opcode -> (type name, prefix length before first string)
fn text_op(op: u32) -> Option<(&'static str, usize)> {
    match op {
        0x04 => Some(("title", 16)),
        0x29 => Some(("speaker", 16)),
        0x2a => Some(("text", 16)),
        0x0d => Some(("choice", 16)),
        _ => None,
    }
}

fn u32_at(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

// Encode a string as CP932 (Shift-JIS superset). Returns Err with the offending
// text if any character is unmappable, mirroring Python's UnicodeEncodeError.
pub fn cp932_encode(s: &str) -> Result<Vec<u8>, PatchError> {
    let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(s);
    if had_errors {
        return Err(PatchError::Encoding(format!("cannot encode {s:?} as cp932")));
    }
    Ok(bytes.into_owned())
}

// Decode CP932 bytes into a String (lossy: invalid bytes become U+FFFD, but the
// shipped scripts are all valid).
pub fn cp932_decode(bytes: &[u8]) -> String {
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
    text.into_owned()
}

pub struct Instr {
    pub start: usize,
    pub op: u32,
    pub raw: Vec<u8>, // full record bytes (header + args)
}

impl Instr {
    pub fn args(&self) -> &[u8] {
        &self.raw[8..]
    }
}

// Parse a decrypted ASB script into a list of Instr (terminator last).
pub fn parse_script(raw: &[u8]) -> Result<Vec<Instr>, PatchError> {
    let mut instrs = Vec::new();
    let mut pos = 0usize;
    while pos < raw.len() {
        let op = u32_at(raw, pos);
        let ln = u32_at(raw, pos + 4);
        if op == 0 && ln == TERMINATOR_LEN {
            instrs.push(Instr {
                start: pos,
                op,
                raw: raw[pos..].to_vec(),
            });
            return Ok(instrs);
        }
        let ln = ln as usize;
        if ln < 8 || pos + ln > raw.len() {
            return Err(PatchError::Format(format!(
                "bad record at {pos:#x}: op={op:#x} len={ln:#x}"
            )));
        }
        instrs.push(Instr {
            start: pos,
            op,
            raw: raw[pos..pos + ln].to_vec(),
        });
        pos += ln;
    }
    Err(PatchError::Format("script has no terminator record".into()))
}

// Return (kind, prefix_len, strings) for a text instruction, or None.
//
// Strings are NUL-terminated cp932, laid out sequentially after the prefix.
pub fn read_strings(instr: &Instr) -> Option<(&'static str, usize, Vec<String>)> {
    let (kind, plen) = text_op(instr.op)?;
    let args = instr.args();
    if instr.op == 0x29 && args.len() <= 0x10 {
        return None; // speaker record without a name
    }
    let count = if instr.op == 0x0d {
        u32_at(args, 4) as usize
    } else {
        1
    };
    let mut strings = Vec::with_capacity(count);
    let mut pos = plen;
    for _ in 0..count {
        let end = pos + args[pos..].iter().position(|&b| b == 0)?;
        strings.push(cp932_decode(&args[pos..end]));
        pos = end + 1;
    }
    Some((kind, plen, strings))
}

// Rebuild a text instruction's raw bytes with replacement strings.
pub fn rebuild_instr(instr: &Instr, new_strings: &[String]) -> Result<Instr, PatchError> {
    let (_, plen, old) = read_strings(instr)
        .ok_or_else(|| PatchError::Format(format!("not a text record at {:#x}", instr.start)))?;
    if new_strings.len() != old.len() {
        return Err(PatchError::Mismatch(format!(
            "string count mismatch at {:#x}: {} != {}",
            instr.start,
            new_strings.len(),
            old.len()
        )));
    }
    let mut body: Vec<u8> = instr.raw[8..8 + plen].to_vec();
    for s in new_strings {
        body.extend_from_slice(&cp932_encode(s)?);
        body.push(0);
    }
    let mut raw: Vec<u8> = Vec::with_capacity(8 + body.len());
    raw.extend_from_slice(&instr.op.to_le_bytes());
    raw.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
    raw.extend_from_slice(&body);
    Ok(Instr {
        start: instr.start,
        op: instr.op,
        raw,
    })
}

// Reassemble instructions into script bytes, relocating jump targets.
//
// `instrs` keep their ORIGINAL .start values; new offsets are computed here.
pub fn rebuild_script(instrs: &[Instr]) -> Result<Vec<u8>, PatchError> {
    // new start offsets
    let mut reloc: HashMap<usize, usize> = HashMap::with_capacity(instrs.len());
    let mut pos = 0usize;
    for ins in instrs {
        reloc.insert(ins.start, pos);
        pos += ins.raw.len();
    }

    let mut out: Vec<u8> = Vec::with_capacity(pos);
    for ins in instrs {
        if JUMP_OPS.contains(&ins.op) {
            let target = u32_at(&ins.raw, 8) as usize;
            let new_target = reloc.get(&target).ok_or_else(|| {
                PatchError::Mismatch(format!(
                    "jump at {:#x} targets {target:#x}, which is not an instruction start",
                    ins.start
                ))
            })?;
            if *new_target != target {
                let mut raw = ins.raw.clone();
                raw[8..12].copy_from_slice(&(*new_target as u32).to_le_bytes());
                out.extend_from_slice(&raw);
                continue;
            }
        }
        out.extend_from_slice(&ins.raw);
    }
    Ok(out)
}
