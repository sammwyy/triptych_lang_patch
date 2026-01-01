# Triptych (Download Edition) Language Patcher

This tool is a text dumper and patcher specifically designed for the **Download Edition (2014)** of the Visual Novel **Triptych**, originally developed by **ALcot** in 2006.

It allows you to extract Japanese scripts from the `.arc` containers, translate them into a `YAML` format, and re-inject them back into the game.

## Information

* **Original Release:** 2006 (ALcot)
* **Target Version:** 2014 Download Edition
* **VNDB:** [https://vndb.org/v537](https://vndb.org/v537)
* **Format:** AZSys `.arc` (ASB script files)

## Features

* **Dump:** Scans and extracts Japanese strings from encrypted ASB files within the script archive.
* **Patch:** Injects translated strings from a YAML file while handling ASB re-encryption and CRC32 recalculation.
* **Check:** Diagnostic tool to analyze ARC headers or compare two archives to ensure file integrity.

## How to use

### 1. Requirements

Ensure you have [Python 3.x](https://www.python.org/) and `PyYAML` installed:

```bash
pip install pyyaml
```

### 2. Setup

Place the game's `script.arc` file inside the tool's root directory.

### 3. Extracting text (Dump)

Run the following command to generate a `script.yml` file:

```bash
python patcher.py dump
```

This will create a backup of your original archive named `original_script.arc`.

### 4. Translating

Open `script.yml` with any text editor (VSCode recommended). Translate the values on the right, keeping the keys (File:Offset) intact.

> **Note:** The patcher uses `CP932` for Japanese and `ASCII` for English to avoid encoding issues.

### 5. Applying the patch

Once translated, run:

```bash
python patcher.py patch
```

This will update `script.arc` with your new text.

### 6. Diagnostics (Optional)

To compare your modified archive with the original or analyze its structure:

```bash
# Analyze a single file
python patcher.py check script.arc

# Compare original vs patched
python patcher.py check original_script.arc script.arc
```

## Credits

* **Author:** [Sammwy](https://github.com/sammwyy)
* **Research & Tools:** - [GARbro](https://github.com/morkt/GARbro) (Format reference)
* [HxD](https://mh-nexus.de/en/hxd/) (Hex inspection)