# tripatch - Triptych Patch Installer & Toolkit

A single small `.exe` that installs (and authors) translation patches for
**Triptych** (ALcot, 2014 Download Edition, AZSys engine).

There are two ways to use it:

- **Players** just double-click `tripatch.exe` and follow two dialogs.
- **Translators** use the `extract` and `build` commands from a terminal.

---

## For players: installing a patch

1. Put `tripatch.exe` and the patch file (a `.json`) anywhere you like.
2. **Double-click `tripatch.exe`.**
3. A window opens and asks you to pick the **patch `.json`** file.
4. Then it asks you to pick the **game folder** - the folder that contains
   `triptych.exe` and `script.arc`.
5. It checks that the folder really is the game, applies the patch, and shows a
   success message. Press Enter to close.

That's it. You can re-run it as many times as you want.

### How it protects your game

The first time you patch, the installer copies the original `script.arc` into a
`backup\` folder inside the game directory:

```
<game>\script.arc            <- the file the game loads (gets patched)
<game>\backup\script.arc     <- pristine original, created automatically
```

On every run it patches **from the backup**, never from the already-patched
file, so re-installing or switching patches never stacks corruption. Your
original is always safe.

### Uninstalling

Copy `backup\script.arc` back over `script.arc`. Done.

---

## For translators: authoring a patch

The tool exposes two commands. Both take `--input` / `--output` paths.

### 1. Extract the text

```
tripatch extract --input path\to\script.arc --output texts.json
```

- Reads the original `script.arc`.
- Makes a pristine backup next to it (`backup\script.arc`) and reads from that.
- Writes every translatable line to `texts.json`.

### 2. Translate

Open `texts.json` in any UTF-8 text editor and fill in the `"en"` fields:

```json
{
  "_meta": { "source": "script.arc", "...": "..." },
  "files": {
    "kar_c1_000_01.asb": [
      { "offset": "0x18f", "type": "text",
        "jp": "太陽が水平線の向こうに沈んでいく。",
        "en": "The sun sinks beyond the horizon." },
      { "offset": "0xc341", "type": "choice",
        "jp": ["選択肢A", "選択肢B"],
        "en": ["Option A", "Option B"] }
    ]
  }
}
```

- An empty `"en"` keeps the original Japanese line.
- `choice` units use a list, one entry per option.
- Translations can be **any length** - records are resized and internal jump
  offsets are relocated automatically.
- `offset` is a stable anchor into the original script. **Never edit it.**

### 3. Build / apply

```
tripatch build --input texts.json --output path\to\game-folder --verify
```

- `--output` is the game folder (or the `script.arc` path directly).
- Creates `backup\script.arc` if missing, patches from the backup, writes the
  result over the game's `script.arc`.
- `--verify` decrypts and re-parses every script in the result as a self check.

The finished `texts.json` is the patch you ship to players - they just
double-click `tripatch.exe` and select it.

---

## How it works (technical)

`script.arc` is an AZSys `ARC\x1a` container holding 347 encrypted `.asb`
scripts. Each script is zlib-compressed and additively encrypted (game key
`0x1DE71CB9`) and contains bytecode records:

```
+0  u32  opcode
+4  u32  record length (including this 8-byte header)
+8  ...  args
```

Text lives inline (CP932, NUL-terminated) in these opcodes:

| opcode | meaning      | extracted as |
|--------|--------------|--------------|
| `0x04` | scene title  | `title`      |
| `0x29` | speaker name | `speaker`    |
| `0x2a` | dialogue     | `text`       |
| `0x0d` | choice menu  | `choice` (list of options) |

Opcodes `0x0a`, `0x0b`, `0x0c` are jumps whose first argument is an absolute
byte offset into the same script; when a record is resized, every jump target
is relocated. The ARC index is LZSS-compressed with a CRC32 and is rebuilt and
re-signed, so entries can grow freely.

### Building from source

Requires a Rust toolchain (`cargo`):

```
cargo build --release
```

The binary is written to `target\release\tripatch.exe`.

---

## Notes & caveats

- Output encoding is CP932 (ASCII is a subset, so plain English is safe). Smart
  quotes, en/em dashes and ellipses are auto-converted to CP932-safe forms; any
  other unmappable character aborts the build with a clear message.
- The engine wraps long Japanese text and may break long English lines
  mid-word; keep lines short if that bothers you.
- Saves made mid-scene with the old `script.arc` store a script position and
  may resume at the wrong spot after patching. Save at choice or title screens,
  or start a fresh game when in doubt.
- UI text (menus, config) is not in `script.arc`; it lives in `system.arc` and
  the executable, and is not handled here.