import os
import yaml
import argparse
import shutil
import struct
import zlib

# Core Constants
TRIPTYCH_KEY = 501685433
ORIGINAL_ARC = "original_script.arc"
TARGET_ARC = "script.arc"
YAML_FILE = "script.yml"

# --- Core ARC Logic ---

class CRC32:
    @staticmethod
    def compute(data: bytes, start: int = 0, length: int = None) -> int:
        if length is None:
            length = len(data) - start
        return zlib.crc32(data[start:start + length]) & 0xFFFFFFFF

class IndexReader:
    def __init__(self, packed: bytes, count: int):
        self.input = packed
        self.output = bytearray(count * 0x40)
        self.control_len = struct.unpack_from('<I', packed, 4)[0]
        self.compr1_len = struct.unpack_from('<I', packed, 8)[0]
        self.compr2_len = struct.unpack_from('<I', packed, 12)[0]
        self.output_len = struct.unpack_from('<I', packed, 0x10)[0]
    
    def unpack(self) -> bytes:
        control = 0x14
        compr1 = control + self.control_len
        compr2 = compr1 + self.compr1_len
        dst = 0
        mask = 0x80
        
        while dst < len(self.output):
            if self.input[control] & mask:
                offset = struct.unpack_from('<H', self.input, compr1)[0]
                compr1 += 2
                copy_count = (offset >> 13) + 3
                offset = (offset & 0x1FFF) + 1
                src = dst - offset
                for _ in range(copy_count):
                    self.output[dst] = self.output[src]
                    dst += 1
                    src += 1
            else:
                copy_count = self.input[compr2] + 1
                compr2 += 1
                self.output[dst:dst + copy_count] = self.input[compr2:compr2 + copy_count]
                compr2 += copy_count
                dst += copy_count
            
            mask >>= 1
            if mask == 0:
                control += 1
                mask = 0x80
        return bytes(self.output)

class ArcEntry:
    def __init__(self, name: str, offset: int = 0, size: int = 0):
        self.name = name
        self.offset = offset
        self.size = size

def decrypt_asb(asb_data: bytes, key: int = TRIPTYCH_KEY) -> bytes:
    if len(asb_data) < 20 or asb_data[:4] != b'ASB\x1a':
        raise ValueError("Invalid ASB signature")
    unpacked = struct.unpack_from('<I', asb_data, 8)[0]
    packed_size = struct.unpack_from('<I', asb_data, 4)[0]
    enc_key = (key ^ unpacked)
    enc_key ^= ((enc_key << 12) | enc_key) << 11
    enc_key &= 0xFFFFFFFF
    encrypted = asb_data[12:12 + packed_size]
    decrypted = bytearray(len(encrypted))
    for i in range(0, len(encrypted), 4):
        if i + 4 <= len(encrypted):
            value = struct.unpack_from('<I', encrypted, i)[0]
            value = (value - enc_key) & 0xFFFFFFFF
            struct.pack_into('<I', decrypted, i, value)
        else:
            for j in range(len(encrypted) - i): decrypted[i + j] = encrypted[i + j]
    return zlib.decompress(decrypted[4:])

def encrypt_asb(decompressed_data: bytes, key: int = TRIPTYCH_KEY) -> bytes:
    compressed = zlib.compress(decompressed_data, 9)
    unpacked_size = len(decompressed_data)
    crc = CRC32.compute(compressed)
    to_encrypt = struct.pack('<I', crc) + compressed
    enc_key = (key ^ unpacked_size)
    enc_key ^= ((enc_key << 12) | enc_key) << 11
    enc_key &= 0xFFFFFFFF
    encrypted = bytearray(len(to_encrypt))
    for i in range(0, len(to_encrypt), 4):
        if i + 4 <= len(to_encrypt):
            value = struct.unpack_from('<I', to_encrypt, i)[0]
            value = (value + enc_key) & 0xFFFFFFFF
            struct.pack_into('<I', encrypted, i, value)
        else:
            for j in range(len(to_encrypt) - i): encrypted[i + j] = to_encrypt[i + j]
    return b'ASB\x1a' + struct.pack('<II', len(encrypted), unpacked_size) + encrypted

# --- Patcher Logic ---

class Patcher:
    def __init__(self):
        self.setup_files()
        
    def setup_files(self):
        if os.path.exists(TARGET_ARC) and not os.path.exists(ORIGINAL_ARC):
            print(f"[*] Creating backup: {TARGET_ARC} -> {ORIGINAL_ARC}")
            shutil.copy(TARGET_ARC, ORIGINAL_ARC)
        
        if not os.path.exists(ORIGINAL_ARC):
            print(f"[!] {ORIGINAL_ARC} not found.")
            exit(1)

    def is_real_text(self, text: str) -> bool:
        for char in text:
            cp = ord(char)
            if (0x3040 <= cp <= 0x309F) or (0x30A0 <= cp <= 0x30FF) or \
               (0x4E00 <= cp <= 0x9FFF) or (0x3000 <= cp <= 0x303F):
                return True
        return False

    def get_arc_entries(self, arc_path):
        with open(arc_path, 'rb') as f:
            header = f.read(0x30)
            count = struct.unpack_from('<I', header, 8)[0]
            idx_len = struct.unpack_from('<I', header, 12)[0]
            packed_idx = f.read(idx_len)
            reader = IndexReader(packed_idx, count)
            index_data = reader.unpack()
            
            entries = []
            base_offset = 0x30 + idx_len
            for i in range(count):
                idx_off = i * 0x40
                raw_name = index_data[idx_off+0x10:idx_off+0x40].split(b'\x00', 1)[0]
                name = raw_name.decode('cp932', errors='ignore').strip()
                if name.lower().endswith('.asb'):
                    offset = struct.unpack_from('<I', index_data, idx_off)[0]
                    size = struct.unpack_from('<I', index_data, idx_off + 4)[0]
                    entries.append(ArcEntry(name, base_offset + offset, size))
            return entries

    def dump(self):
        print("[*] Starting Dump...")
        entries = self.get_arc_entries(ORIGINAL_ARC)
        dump_data = {}
        
        with open(ORIGINAL_ARC, 'rb') as f:
            for entry in entries:
                f.seek(entry.offset)
                asb_data = f.read(entry.size)
                try:
                    raw_data = decrypt_asb(asb_data)
                    i = 0
                    while i < len(raw_data):
                        if raw_data[i] < 0x20:
                            i += 1; continue
                        start = i
                        buf = bytearray()
                        while i < len(raw_data) and raw_data[i] >= 0x20:
                            buf.append(raw_data[i]); i += 1
                        if len(buf) >= 2:
                            try:
                                text = buf.decode("cp932").strip()
                                if self.is_real_text(text):
                                    dump_data[f"{entry.name}:{hex(start)}"] = text
                            except: pass
                except Exception as e:
                    print(f"[!] Skip {entry.name}: {e}")

        with open(YAML_FILE, 'w', encoding='utf-8') as y:
            yaml.dump(dump_data, y, allow_unicode=True, sort_keys=False, width=1000)
        print(f"[+] Dump finished. {len(dump_data)} lines -> {YAML_FILE}")

    def patch(self):
        if not os.path.exists(YAML_FILE):
            print(f"[!] {YAML_FILE} missing."); return
            
        with open(YAML_FILE, 'r', encoding='utf-8') as y:
            translations = yaml.safe_load(y)
        if not translations: return

        print("[*] Patching ARC...")
        with open(ORIGINAL_ARC, 'rb') as f:
            full_arc = bytearray(f.read())

        entries = self.get_arc_entries(ORIGINAL_ARC)
        total_patched = 0

        for entry in entries:
            file_prefix = f"{entry.name}:"
            file_translations = {k.split(':')[-1]: v for k, v in translations.items() if k.startswith(file_prefix)}
            if not file_translations: continue

            try:
                asb_data = full_arc[entry.offset : entry.offset + entry.size]
                raw_data = bytearray(decrypt_asb(asb_data))
                modified = False

                for offset_str, new_text in file_translations.items():
                    if new_text is None: continue
                    offset = int(offset_str, 16)
                    end = offset
                    while end < len(raw_data) and raw_data[end] >= 0x20: end += 1
                    
                    orig_len = end - offset
                    encoded_new = str(new_text).encode('cp932' if self.is_real_text(str(new_text)) else 'ascii', errors='ignore')

                    if len(encoded_new) > orig_len:
                        print(f" [!] OVERFLOW in {entry.name}:{offset_str} ({len(encoded_new)} > {orig_len})")
                        continue 

                    raw_data[offset:end] = encoded_new.ljust(orig_len, b'\x20')
                    modified = True
                    total_patched += 1

                if modified:
                    new_asb = encrypt_asb(raw_data)
                    full_arc[entry.offset : entry.offset + entry.size] = new_asb.ljust(entry.size, b'\x00')

            except Exception as e:
                print(f" [!] Error patching {entry.name}: {e}")

        with open(TARGET_ARC, 'wb') as f:
            f.write(full_arc)
        print(f"[+] Process finished. Patched strings: {total_patched}")

# --- Diagnostic Logic ---

def analyze_arc(filepath):
    print(f"\n{'='*40}\nAnalyzing: {filepath}\n{'='*40}")
    if not os.path.exists(filepath):
        print(f"[!] File not found: {filepath}"); return
        
    with open(filepath, 'rb') as f:
        header = f.read(0x30)
        sig, exts, files, idx_len = struct.unpack_from('<IIII', header, 0)
        print(f"Header:\n  Signature: {hex(sig)}\n  Files: {files}\n  Index Len: {idx_len}")
        
        packed_idx = f.read(idx_len)
        crc = struct.unpack_from('<I', packed_idx, 0)[0]
        comp_crc = zlib.crc32(packed_idx[0x14:]) & 0xFFFFFFFF
        print(f"Index CRC: {hex(crc)} | Computed: {hex(comp_crc)} {'✓' if crc == comp_crc else '✗'}")

def compare_arcs(path1, path2):
    analyze_arc(path1)
    analyze_arc(path2)
    print(f"\n{'='*40}\nComparison\n{'='*40}")
    with open(path1, 'rb') as f1, open(path2, 'rb') as f2:
        d1, d2 = f1.read(), f2.read()
        print(f"Sizes: {len(d1)} vs {len(d2)} | Diff: {len(d2)-len(d1)}")
        if d1 == d2:
            print("[+] Files are identical."); return
        
        for i in range(min(len(d1), len(d2))):
            if d1[i] != d2[i]:
                print(f"[*] First difference at {hex(i)}")
                print(f"  Orig: {hex(d1[i])} | New: {hex(d2[i])}")
                break

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Triptych (Download Edition) Language Patcher")
    subparsers = parser.add_subparsers(dest="mode", required=True)
    
    subparsers.add_parser("dump", help="Extract text to YAML")
    subparsers.add_parser("patch", help="Apply YAML translations to ARC")
    
    check_parser = subparsers.add_parser("check", help="Diagnostic/Comparison tool")
    check_parser.add_argument("file1", help="Original ARC")
    check_parser.add_argument("file2", nargs="?", help="Optional: Recompiled ARC to compare")

    args = parser.parse_args()
    p = Patcher()

    if args.mode == "dump":
        p.dump()
    elif args.mode == "patch":
        p.patch()
    elif args.mode == "check":
        if args.file2:
            compare_arcs(args.file1, args.file2)
        else:
            analyze_arc(args.file1)