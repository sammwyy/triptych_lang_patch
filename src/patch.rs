// High-level patch operations: extract text to JSON, and build/apply a JSON
// patch back into script.arc.
//
// The JSON document mirrors the reference texts.json layout:
//
//   {
//     "_meta": { "game": ..., "source": "script.arc", "note": ... },
//     "files": {
//       "name.asb": [
//         { "offset": "0x18f", "type": "text",   "jp": "...",     "en": "..." },
//         { "offset": "0xc341", "type": "choice", "jp": ["a","b"], "en": ["A","B"] }
//       ]
//     }
//   }
//
// `offset` is the record's byte offset in the ORIGINAL decrypted script and is
// the stable anchor used when applying the patch; it must never be edited.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::asb::{
    cp932_encode, parse_script, read_strings, rebuild_instr, rebuild_script,
};
use crate::azsys::{decrypt_asb, encrypt_asb, Arc, TRIPTYCH_KEY};
use crate::error::PatchError;

// A JSON value that is either a single string or a list of strings. Single text
// records use a string; "choice" records use a list, one entry per option.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StrOrList {
    Many(Vec<String>),
    One(String),
}

impl StrOrList {
    fn to_vec(&self) -> Vec<String> {
        match self {
            StrOrList::One(s) => vec![s.clone()],
            StrOrList::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub offset: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub jp: StrOrList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<StrOrList>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub game: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Document {
    #[serde(rename = "_meta", default)]
    pub meta: Meta,
    // serde_json's preserve_order feature keeps file insertion/parse order.
    pub files: serde_json::Map<String, serde_json::Value>,
}

// CP932 cannot encode some common Unicode punctuation; map to safe forms.
fn sanitize(text: &str) -> String {
    text.replace('\u{2018}', "'") // left single quote
        .replace('\u{2019}', "'") // right single quote
        .replace('\u{201c}', "\"") // left double quote
        .replace('\u{201d}', "\"") // right double quote
        .replace('\u{2013}', "-") // en dash
        .replace('\u{2014}', "\u{2015}") // em dash -> horizontal bar (in cp932)
        .replace('\u{2026}', "...") // ellipsis
        .replace('\u{00a0}', " ") // non-breaking space
}

// The bare file name of the archive a patch targets, e.g. "script.arc".
// Falls back to "script.arc" when the metadata is empty, and strips any
// directory component (older documents stored "backup/script.arc").
pub fn source_name(meta: &Meta) -> String {
    let raw = if meta.source.trim().is_empty() {
        "script.arc"
    } else {
        meta.source.trim()
    };
    let normalized = raw.replace('\\', "/");
    Path::new(&normalized)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "script.arc".to_string())
}

// Read a patch/text document from disk.
pub fn load_document(path: &Path) -> Result<Document, PatchError> {
    let text = fs::read_to_string(path)
        .map_err(|e| PatchError::Io(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| PatchError::Format(format!("invalid patch JSON {}: {e}", path.display())))
}

// ---------------------------------------------------------------------------
// Extract
// ---------------------------------------------------------------------------

pub struct ExtractStats {
    pub total_units: usize,
    pub file_count: usize,
    pub warnings: Vec<String>,
}

// Build the unit list for a single decrypted script.
fn extract_file(name: &str, raw: &[u8], warnings: &mut Vec<String>) -> Result<Vec<Unit>, PatchError> {
    let mut units = Vec::new();
    let instrs = parse_script(raw)?;
    for ins in &instrs {
        let info = read_strings(ins);
        let (kind, _, strings) = match info {
            Some(v) => v,
            None => continue,
        };
        // Sanity: rebuilding with the original strings must reproduce the
        // record byte-for-byte, otherwise our layout assumption is wrong for
        // this record and we must not touch it.
        match rebuild_instr(ins, &strings) {
            Ok(rebuilt) if rebuilt.raw == ins.raw => {}
            _ => {
                warnings.push(format!(
                    "{name}@{:#x} (op {:#x}) does not round-trip; skipped",
                    ins.start, ins.op
                ));
                continue;
            }
        }
        let unit = if kind == "choice" {
            Unit {
                offset: format!("{:#x}", ins.start),
                kind: kind.to_string(),
                jp: StrOrList::Many(strings.clone()),
                en: Some(StrOrList::Many(strings.iter().map(|_| String::new()).collect())),
            }
        } else {
            Unit {
                offset: format!("{:#x}", ins.start),
                kind: kind.to_string(),
                jp: StrOrList::One(strings[0].clone()),
                en: Some(StrOrList::One(String::new())),
            }
        };
        units.push(unit);
    }
    Ok(units)
}

// Extract every translatable text unit from `arc_bytes` and write texts.json.
// `source_label` is recorded in the document metadata (e.g. "script.arc").
pub fn extract(
    arc_bytes: &[u8],
    source_label: &str,
    out_path: &Path,
) -> Result<ExtractStats, PatchError> {
    let arc = Arc::load(arc_bytes)?;
    let mut warnings = Vec::new();
    let mut files = serde_json::Map::new();
    let mut total = 0usize;
    for e in &arc.entries {
        if !e.name.to_lowercase().ends_with(".asb") {
            continue;
        }
        let raw = decrypt_asb(&e.data, TRIPTYCH_KEY)?;
        let units = extract_file(&e.name, &raw, &mut warnings)?;
        if !units.is_empty() {
            total += units.len();
            files.insert(e.name.clone(), serde_json::to_value(&units).unwrap());
        }
    }

    let file_count = files.len();
    let doc = Document {
        meta: Meta {
            game: "Triptych (2014 Download Edition)".to_string(),
            source: source_label.to_string(),
            note: "Fill \"en\" to translate; empty keeps Japanese. \
                   For \"choice\" units, \"en\" is a list matching \"jp\"."
                .to_string(),
        },
        files,
    };

    let json = serde_json::to_string_pretty(&doc)
        .map_err(|e| PatchError::Format(format!("cannot serialize JSON: {e}")))?;
    fs::write(out_path, json)
        .map_err(|e| PatchError::Io(format!("cannot write {}: {e}", out_path.display())))?;

    Ok(ExtractStats {
        total_units: total,
        file_count,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Build / apply
// ---------------------------------------------------------------------------

// Resolve the final CP932 string list for a unit (translation or original).
fn pick(unit: &Unit) -> Result<Vec<String>, PatchError> {
    let jp = unit.jp.to_vec();
    let en = unit.en.as_ref().map(|e| e.to_vec()).unwrap_or_default();
    let mut out = Vec::with_capacity(jp.len());
    for (i, j) in jp.iter().enumerate() {
        let e = en.get(i);
        let s = match e {
            Some(e) if !e.is_empty() => sanitize(e),
            _ => j.clone(),
        };
        // Validate up front so we fail with a clear message, not deep inside.
        cp932_encode(&s).map_err(|_| {
            PatchError::Encoding(format!(
                "offset {}: cannot encode {s:?} as cp932",
                unit.offset
            ))
        })?;
        out.push(s);
    }
    Ok(out)
}

// Parse a hex offset string like "0x18f".
fn parse_offset(s: &str) -> Result<usize, PatchError> {
    let t = s.trim();
    let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    usize::from_str_radix(hex, 16)
        .map_err(|_| PatchError::Format(format!("bad offset {s:?}")))
}

// Patch one decrypted script with its unit list. Returns the new script bytes.
fn patch_script(name: &str, raw: &[u8], units: &[Unit]) -> Result<Vec<u8>, PatchError> {
    let mut instrs = parse_script(raw)?;
    let mut by_start = std::collections::HashMap::new();
    for (i, ins) in instrs.iter().enumerate() {
        by_start.insert(ins.start, i);
    }
    for unit in units {
        let start = parse_offset(&unit.offset)?;
        let idx = *by_start.get(&start).ok_or_else(|| {
            PatchError::Mismatch(format!(
                "{name}: no instruction at {} - patch does not match this script.arc",
                unit.offset
            ))
        })?;
        let strings = pick(unit)?;
        let info = read_strings(&instrs[idx]).ok_or_else(|| {
            PatchError::Mismatch(format!("{name} {}: not a text record", unit.offset))
        })?;
        if strings != info.2 {
            instrs[idx] = rebuild_instr(&instrs[idx], &strings)?;
        }
    }
    rebuild_script(&instrs)
}

pub struct BuildStats {
    pub translated_units: usize,
    pub scripts_modified: usize,
}

// Count units carrying a non-empty translation.
fn count_translated(units: &[Unit]) -> usize {
    units
        .iter()
        .filter(|u| match &u.en {
            Some(StrOrList::One(s)) => !s.is_empty(),
            Some(StrOrList::Many(v)) => v.iter().any(|s| !s.is_empty()),
            None => false,
        })
        .count()
}

// Apply a patch document to source archive bytes, returning the rebuilt
// archive bytes and statistics.
pub fn build(doc: &Document, source_arc: &[u8]) -> Result<(Vec<u8>, BuildStats), PatchError> {
    let mut arc = Arc::load(source_arc)?;
    let mut translated = 0usize;
    let mut patched = 0usize;
    for e in arc.entries.iter_mut() {
        let value = match doc.files.get(&e.name) {
            Some(v) => v,
            None => continue,
        };
        let units: Vec<Unit> = serde_json::from_value(value.clone())
            .map_err(|err| PatchError::Format(format!("bad units for {}: {err}", e.name)))?;
        let raw = decrypt_asb(&e.data, TRIPTYCH_KEY)?;
        let new_raw = patch_script(&e.name, &raw, &units)?;
        translated += count_translated(&units);
        if new_raw != raw {
            e.data = encrypt_asb(&new_raw, TRIPTYCH_KEY)?;
            patched += 1;
        }
    }
    let out = arc.save();
    Ok((
        out,
        BuildStats {
            translated_units: translated,
            scripts_modified: patched,
        },
    ))
}

// Decrypt and re-parse every script in `arc_bytes`, returning the script count.
// Used as a post-build self check.
pub fn verify(arc_bytes: &[u8]) -> Result<usize, PatchError> {
    let arc = Arc::load(arc_bytes)?;
    for e in &arc.entries {
        if !e.name.to_lowercase().ends_with(".asb") {
            continue;
        }
        let raw = decrypt_asb(&e.data, TRIPTYCH_KEY)?;
        let instrs = parse_script(&raw)?;
        for ins in &instrs {
            let _ = read_strings(ins);
        }
    }
    Ok(arc.entries.len())
}

// ---------------------------------------------------------------------------
// Backup-aware install flow
// ---------------------------------------------------------------------------

// Locations involved in applying a patch to a game install.
pub struct Target {
    // The archive the game actually loads (gets overwritten with the patch).
    pub original: PathBuf,
    // The pristine copy kept under <game>/backup/<name>; always used as source.
    pub backup: PathBuf,
}

// Resolve where the target archive and its backup live, given the game
// directory and the source file name from the patch metadata.
pub fn resolve_target(game_dir: &Path, source: &str) -> Target {
    Target {
        original: game_dir.join(source),
        backup: game_dir.join("backup").join(source),
    }
}

// Ensure a pristine backup exists, then return the source bytes to patch
// from. The backup is created from the original on first run and is the
// authoritative source on every subsequent run, so re-patching never stacks
// on an already-patched file.
pub fn ensure_backup(target: &Target) -> Result<Vec<u8>, PatchError> {
    if !target.backup.exists() {
        if !target.original.exists() {
            return Err(PatchError::Io(format!(
                "source file not found: {}",
                target.original.display()
            )));
        }
        if let Some(parent) = target.backup.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                PatchError::Io(format!("cannot create backup folder {}: {e}", parent.display()))
            })?;
        }
        fs::copy(&target.original, &target.backup).map_err(|e| {
            PatchError::Io(format!(
                "cannot back up {} -> {}: {e}",
                target.original.display(),
                target.backup.display()
            ))
        })?;
    }
    fs::read(&target.backup)
        .map_err(|e| PatchError::Io(format!("cannot read backup {}: {e}", target.backup.display())))
}

// Full install: ensure backup, apply the patch document, write over the
// original. Returns build statistics.
pub fn install(doc: &Document, game_dir: &Path) -> Result<(BuildStats, Target), PatchError> {
    let source = source_name(&doc.meta);
    let target = resolve_target(game_dir, &source);
    let source_bytes = ensure_backup(&target)?;
    let (out, stats) = build(doc, &source_bytes)?;
    fs::write(&target.original, &out).map_err(|e| {
        PatchError::Io(format!("cannot write {}: {e}", target.original.display()))
    })?;
    Ok((stats, target))
}

// Helper exposed to the toolkit: given a hint that may be either the game
// directory or the archive file itself, return the game directory.
pub fn game_dir_from_output(output: &Path, source: &str) -> PathBuf {
    if output.is_dir() {
        return output.to_path_buf();
    }
    // If the hint names a file (existing or not) whose name matches the source,
    // the game directory is its parent.
    if output.file_name().map(|n| n == std::ffi::OsStr::new(source)).unwrap_or(false) {
        if let Some(parent) = output.parent() {
            return parent.to_path_buf();
        }
    }
    // Otherwise treat the hint itself as the game directory.
    output.to_path_buf()
}
