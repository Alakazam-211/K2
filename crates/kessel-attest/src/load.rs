//! Reading inputs from disk: `.intoto.jsonl` files, an evidence folder, the
//! files to check.

use std::path::{Path, PathBuf};

use crate::dsse::Envelope;
use crate::trust::TrustDir;
use crate::util::sha256_file;
use crate::verify::FileArg;

/// Largest `.intoto.jsonl` file read.
pub const MAX_JSONL_BYTES: u64 = 64 * 1024 * 1024;

/// Every envelope in a `.intoto.jsonl` file (one per non-empty line),
/// labelled by the file name without `.intoto.jsonl`, plus `#<line>` when
/// the file holds more than one.
pub fn envelopes(path: &Path) -> Result<Vec<(String, Envelope)>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MAX_JSONL_BYTES {
        return Err(format!("{}: over {MAX_JSONL_BYTES} bytes", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = file.strip_suffix(".intoto.jsonl").or_else(|| file.strip_suffix(".jsonl")).or_else(|| file.strip_suffix(".json")).unwrap_or(&file).to_string();
    let lines: Vec<(usize, &str)> = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return Err(format!("{}: no statement in it", path.display()));
    }
    let many = lines.len() > 1;
    lines
        .into_iter()
        .map(|(i, line)| {
            let label = if many { format!("{stem}#{}", i + 1) } else { stem.clone() };
            Envelope::from_json(line).map(|e| (label, e)).map_err(|e| format!("{} line {}: {e}", path.display(), i + 1))
        })
        .collect()
}

/// A file to check, hashed now.
pub fn file_arg(path: &Path) -> Result<FileArg, String> {
    if !path.is_file() {
        return Err(format!("{}: not a file", path.display()));
    }
    Ok(FileArg { path: path.display().to_string(), sha256: sha256_file(path)?, expect: None })
}

/// What an evidence folder (`k2 ci evidence --out DIR` or `k2 ci provenance
/// --out DIR`) holds: `provenance/<label>.intoto.jsonl`, `trust/`, and,
/// when the job files were fetched, `<label>/<subject name>`.
#[derive(Debug)]
pub struct EvidenceDir {
    /// Every statement, by label.
    pub statements: Vec<(String, Envelope)>,
    /// `DIR/trust`.
    pub trust: TrustDir,
    /// The fetched files found beside them, each tied to its label and
    /// subject name.
    pub fetched: Vec<FileArg>,
}

/// Read an evidence folder.
pub fn evidence_dir(dir: &Path) -> Result<EvidenceDir, String> {
    let pdir = dir.join("provenance");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&pdir)
        .map_err(|e| format!("{}: {e} (an evidence folder has provenance/<label>.intoto.jsonl)", pdir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".intoto.jsonl"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{}: no .intoto.jsonl file", pdir.display()));
    }
    let mut statements = Vec::new();
    for p in &paths {
        statements.extend(envelopes(p)?);
    }
    let trust = TrustDir::open(&dir.join("trust"))?;
    let mut fetched = Vec::new();
    for (label, env) in &statements {
        // Which file names to look for: the payload's subjects, read
        // unverified here only to find the files; verify checks it all.
        let Ok(body) = crate::util::unb64(&env.payload) else { continue };
        let Ok(st) = serde_json::from_slice::<crate::statement::Statement>(&body) else { continue };
        for s in &st.subject {
            let Some(name) = s.name.as_deref() else { continue };
            let parts: Vec<&str> = name.split('/').collect();
            if name.starts_with('/') || parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
                continue;
            }
            let mut path = dir.join(label);
            for p in parts {
                path.push(p);
            }
            if path.is_file() {
                let mut f = file_arg(&path)?;
                f.expect = Some((label.clone(), name.to_string()));
                fetched.push(f);
            }
        }
    }
    Ok(EvidenceDir { statements, trust, fetched })
}
