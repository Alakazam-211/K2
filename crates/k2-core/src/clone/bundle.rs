//! Bundle builder — tar+gz the inventoried files + `manifest.json`.
//!
//! Layout inside the archive:
//! ```text
//! manifest.json
//! workspace/<rel...>      # DestinationClass::Workspace
//! memory/<rel...>         # DestinationClass::Memory
//! sessions/<rel...>       # DestinationClass::Session  (rel keeps <slug>[-branch]/<id>.jsonl)
//! ```
//! The per-class subdir prefix lets the remote unpack route fan files
//! back out to PROJECT vs `~/.claude/projects/<remote-slug>/` without
//! re-deriving the class from path heuristics.

use super::{CloneInventory, CloneManifest, CloneOptions, DestinationClass, WorkspaceSettings};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::path::{Path, PathBuf};

/// Per-class archive subdir prefix.
fn class_prefix(class: DestinationClass) -> &'static str {
    match class {
        DestinationClass::Workspace => "workspace",
        DestinationClass::Memory => "memory",
        DestinationClass::Session => "sessions",
    }
}

/// Tar+gz the inventory's included files (under per-class subdirs) plus a
/// `manifest.json` at the archive root, writing the `.tar.gz` to
/// `out_path`. Returns `out_path` on success.
///
/// `created_at` is the caller-supplied RFC3339 timestamp baked into the
/// manifest (keeps the engine clock-free; see [`CloneInventory::manifest`]).
///
/// `settings` is the source workspace's captured K2 settings (or `None`),
/// recorded in the manifest so the remote unpack route can re-apply them.
pub fn build_bundle(
    inventory: &CloneInventory,
    opts: &CloneOptions,
    created_at: String,
    settings: Option<WorkspaceSettings>,
    out_path: &Path,
) -> Result<PathBuf, String> {
    let manifest = inventory.manifest(opts, created_at, settings);
    let manifest_json = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| format!("serialize manifest: {e}"))?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create bundle parent dir: {e}"))?;
    }

    let file = std::fs::File::create(out_path)
        .map_err(|e| format!("create bundle file {}: {e}", out_path.display()))?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(enc);

    // manifest.json at the root.
    append_bytes(&mut tar, Path::new("manifest.json"), &manifest_json)?;

    // Files, under their class prefix.
    for entry in &inventory.entries {
        let archive_path =
            Path::new(class_prefix(entry.class)).join(normalize_rel(&entry.rel_path));
        append_file(&mut tar, &archive_path, &entry.abs_path)?;
    }

    let enc = tar
        .into_inner()
        .map_err(|e| format!("finalize tar: {e}"))?;
    enc.finish().map_err(|e| format!("finish gzip: {e}"))?;

    Ok(out_path.to_path_buf())
}

/// Re-root a stored rel path on `/` separators (tar paths are `/`-based)
/// and drop any leading separators so it stays relative inside the prefix.
fn normalize_rel(rel: &str) -> PathBuf {
    let unixy = rel.replace('\\', "/");
    PathBuf::from(unixy.trim_start_matches('/'))
}

fn append_file<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    archive_path: &Path,
    abs_path: &Path,
) -> Result<(), String> {
    let mut f = std::fs::File::open(abs_path)
        .map_err(|e| format!("open {} for bundle: {e}", abs_path.display()))?;
    let meta = f
        .metadata()
        .map_err(|e| format!("stat {}: {e}", abs_path.display()))?;
    let mut header = tar::Header::new_gnu();
    header.set_metadata(&meta);
    header.set_size(meta.len());
    header.set_cksum();
    tar.append_data(&mut header, archive_path, &mut f)
        .map_err(|e| format!("append {} to bundle: {e}", archive_path.display()))?;
    Ok(())
}

fn append_bytes<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    archive_path: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, bytes)
        .map_err(|e| format!("append {} to bundle: {e}", archive_path.display()))?;
    Ok(())
}

/// Read a manifest back out of a built bundle. Used by the unpack route
/// (P2) / README fallback (P3) and by tests for round-trip assertions.
pub fn read_manifest_from_bundle(bundle_path: &Path) -> Result<CloneManifest, String> {
    let file = std::fs::File::open(bundle_path)
        .map_err(|e| format!("open bundle {}: {e}", bundle_path.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(dec);
    for entry in ar.entries().map_err(|e| format!("read bundle: {e}"))? {
        let mut entry = entry.map_err(|e| format!("read bundle entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("entry path: {e}"))?
            .to_path_buf();
        if path == Path::new("manifest.json") {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf)
                .map_err(|e| format!("read manifest: {e}"))?;
            return serde_json::from_slice(&buf)
                .map_err(|e| format!("parse manifest: {e}"));
        }
    }
    Err("manifest.json not found in bundle".to_string())
}
