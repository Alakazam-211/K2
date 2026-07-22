//! Server-side `.zip` → folder extraction (inverse of [`crate::fs_compress`]).
//!
//! The DAEMON owns the filesystem (K2 Connect), so "Extract" on a remote
//! archive must run here — shipping the zip to the client and back would
//! defeat the point. Design constraints:
//!
//! - DEST = sibling folder named after the archive stem (`foo.zip` →
//!   `foo/`), collision-free (`foo (1)/`, `foo (2)/`, …) so a second
//!   extract never overwrites.
//! - ZIP-SLIP: reject absolute member paths and any `..` component
//!   (via `ZipFile::enclosed_name` + an under-dest assert after join).
//! - Caps on entry count and total uncompressed size (align with
//!   transfer ceilings) to blunt zip bombs.
//! - Password-protected / encrypted entries fail loudly (no password UI).
//! - Cooperative cancel via an `AtomicBool`, checked between entries;
//!   a cancelled/failed job removes its partial dest folder.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fs_commands::validate_path;

/// Hard ceiling on member count — enough for real projects, low enough
/// that a malicious central directory cannot pin the worker for hours.
pub const MAX_EXTRACT_ENTRIES: usize = 100_000;

/// Total declared uncompressed size across all members — same order as
/// [`crate::fs_commands::MAX_TRANSFER_SIZE`] (10 GiB).
pub const MAX_EXTRACT_UNCOMPRESSED: u64 = 10 * 1024 * 1024 * 1024;

/// True when `name` ends with `.zip` (ASCII, case-insensitive).
pub fn is_zip_filename(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 4 {
        return false;
    }
    let ext = &bytes[bytes.len() - 4..];
    ext.eq_ignore_ascii_case(b".zip")
}

// ── list (central directory only — no extract) ────────────────────────

/// Hard ceiling on entries returned by [`list_zip_entries`]. Large enough
/// for real project archives; caps wire size + UI work for zip bombs that
/// inflate the central directory with millions of names.
pub const MAX_LIST_ENTRIES: usize = 5_000;

/// One central-directory entry as returned by [`list_zip_entries`].
/// Zip-slip / absolute member names are still listed (raw name) so the
/// UI can show the archive's true contents; extract still rejects them.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ZipListEntry {
    pub name: String,
    /// Declared uncompressed size (0 for directories).
    pub size: u64,
    /// Declared compressed size on the wire.
    pub compressed_size: u64,
    pub is_dir: bool,
}

/// Result of a central-directory list. `truncated` is true when the
/// archive has more members than [`MAX_LIST_ENTRIES`].
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ZipListResult {
    pub entries: Vec<ZipListEntry>,
    pub truncated: bool,
}

/// List zip central-directory entries without extracting. Rejects
/// non-`.zip` filenames and unreadable archives. Zip-slip member names
/// are included as-is (listing is read-only; extract safety is unchanged).
pub fn list_zip_entries(src: &str) -> Result<ZipListResult, String> {
    let src_path = validate_path(src)?;
    if !src_path.is_file() {
        return Err(format!("Not a regular file: {src}"));
    }
    let file_name = src_path
        .file_name()
        .ok_or_else(|| "Source has no name".to_string())?
        .to_string_lossy()
        .to_string();
    if !is_zip_filename(&file_name) {
        return Err(format!("Not a .zip archive: {file_name}"));
    }

    let file = fs::File::open(&src_path)
        .map_err(|e| format!("Failed to open zip {}: {e}", src_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read zip {}: {e}", src_path.display()))?;

    let total = archive.len();
    let truncated = total > MAX_LIST_ENTRIES;
    let limit = total.min(MAX_LIST_ENTRIES);
    let mut entries = Vec::with_capacity(limit);
    for i in 0..limit {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry {i}: {e}"))?;
        // Raw central-directory name — do NOT filter zip-slip paths here.
        // Listing is observational; extract still enforces member safety.
        entries.push(ZipListEntry {
            name: entry.name().to_string(),
            size: entry.size(),
            compressed_size: entry.compressed_size(),
            is_dir: entry.is_dir(),
        });
    }
    Ok(ZipListResult {
        entries,
        truncated,
    })
}

/// Filename stem with a trailing `.zip` (any case) stripped. Falls back
/// to the full name when the extension is absent (caller should have
/// already required `.zip`).
fn zip_stem(file_name: &str) -> String {
    if is_zip_filename(file_name) {
        file_name[..file_name.len() - 4].to_string()
    } else {
        file_name.to_string()
    }
}

/// Sibling dest folder: `<parent>/<stem>/`, then `<stem> (1)/`, ….
fn sibling_dest_dir(parent: &Path, stem: &str) -> PathBuf {
    let first = parent.join(stem);
    if !first.exists() {
        return first;
    }
    for i in 1..10_000u32 {
        let candidate = parent.join(format!("{stem} ({i})"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological (10k siblings) — caller's create_dir will fail loudly
    // rather than overwrite silently.
    first
}

/// Reject absolute paths and `..` components in a raw zip member name
/// (defense in depth alongside `enclosed_name`).
fn member_name_is_safe(raw: &str) -> bool {
    if raw.is_empty() {
        return false;
    }
    // Absolute (unix `/…` or windows `C:\…` / `\…`).
    let p = Path::new(raw);
    if p.is_absolute() {
        return false;
    }
    // Leading `/` or `\` after zip normalizes separators inconsistently.
    let trimmed = raw.trim_start_matches(['/', '\\']);
    if trimmed.len() != raw.len() && raw.starts_with(['/', '\\']) {
        // Absolute-ish; also catch `//evil`
        return false;
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// After joining `dest / rel`, assert the result is still under `dest`
/// (canonicalized when both exist; otherwise string-prefix on cleaned
/// components). Returns the out path on success.
fn resolve_under_dest(dest: &Path, rel: &Path) -> Result<PathBuf, String> {
    if rel.is_absolute() {
        return Err(format!(
            "Zip-slip rejected: absolute member path {}",
            rel.display()
        ));
    }
    for c in rel.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Zip-slip rejected: unsafe member path {}",
                    rel.display()
                ));
            }
        }
    }
    let out = dest.join(rel);
    // Component walk is the primary guard; also require the joined path
    // starts with dest (works for non-canonical temp trees too).
    if !out.starts_with(dest) {
        return Err(format!(
            "Zip-slip rejected: {} escapes {}",
            out.display(),
            dest.display()
        ));
    }
    Ok(out)
}

/// Symlink targets must stay relative and not climb out of the dest
/// tree via `..` (a link to `/etc/passwd` or `../../secret` is an
/// escape hatch even if the link itself sits under dest).
fn symlink_target_is_safe(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    let p = Path::new(target);
    if p.is_absolute() {
        return false;
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777));
}

/// Extract the zip at `src` into a sibling folder named after the
/// archive stem. `progress(done, total)` fires after every entry;
/// `cancel` is polled between entries. Returns the final dest path.
pub fn extract_from_zip(
    src: &str,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let src_path = validate_path(src)?;
    if !src_path.is_file() {
        return Err(format!("Not a regular file: {src}"));
    }
    let file_name = src_path
        .file_name()
        .ok_or_else(|| "Source has no name".to_string())?
        .to_string_lossy()
        .to_string();
    if !is_zip_filename(&file_name) {
        return Err(format!("Not a .zip archive: {file_name}"));
    }
    let parent = src_path
        .parent()
        .ok_or_else(|| "Cannot extract at filesystem root".to_string())?
        .to_path_buf();
    let stem = zip_stem(&file_name);
    if stem.is_empty() {
        return Err("Archive name has empty stem".to_string());
    }

    let file = fs::File::open(&src_path)
        .map_err(|e| format!("Failed to open zip {}: {e}", src_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read zip {}: {e}", src_path.display()))?;

    let total = archive.len();
    if total > MAX_EXTRACT_ENTRIES {
        return Err(format!(
            "Zip has too many entries ({total} > {MAX_EXTRACT_ENTRIES})"
        ));
    }

    // Pre-scan: zip-slip, encryption, size ceiling — fail BEFORE creating
    // any dest so a bad archive leaves no partial tree.
    let mut declared_uncompressed: u64 = 0;
    for i in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err("Extraction cancelled".to_string());
        }
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry {i}: {e}"))?;
        let raw_name = entry.name().to_string();
        if entry.encrypted() {
            return Err(format!(
                "Password-protected zip entry not supported: {raw_name}"
            ));
        }
        if !member_name_is_safe(&raw_name) || entry.enclosed_name().is_none() {
            return Err(format!("Zip-slip rejected: unsafe member path {raw_name}"));
        }
        declared_uncompressed = declared_uncompressed.saturating_add(entry.size());
        if declared_uncompressed > MAX_EXTRACT_UNCOMPRESSED {
            return Err(format!(
                "Zip uncompressed size exceeds limit ({} > {} bytes)",
                declared_uncompressed, MAX_EXTRACT_UNCOMPRESSED
            ));
        }
    }

    let dest = sibling_dest_dir(&parent, &stem);
    fs::create_dir_all(&dest).map_err(|e| {
        format!(
            "Failed to create extract destination {}: {e}",
            dest.display()
        )
    })?;

    let result = extract_entries(&mut archive, &dest, total as u64, progress, cancel);
    match result {
        Ok(()) => Ok(dest),
        Err(e) => {
            let _ = fs::remove_dir_all(&dest);
            Err(e)
        }
    }
}

fn extract_entries(
    archive: &mut zip::ZipArchive<fs::File>,
    dest: &Path,
    total: u64,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut written: u64 = 0;
    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Extraction cancelled".to_string());
        }
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to open zip entry {i}: {e}"))?;
        let raw_name = entry.name().to_string();
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| format!("Zip-slip rejected: unsafe member path {raw_name}"))?;
        let out = resolve_under_dest(dest, &rel)?;

        if entry.is_dir() {
            fs::create_dir_all(&out)
                .map_err(|e| format!("Failed to create directory {}: {e}", out.display()))?;
        } else if entry.is_symlink() {
            let mut target = String::new();
            entry
                .read_to_string(&mut target)
                .map_err(|e| format!("Failed to read symlink {raw_name}: {e}"))?;
            if !symlink_target_is_safe(&target) {
                return Err(format!(
                    "Zip-slip rejected: unsafe symlink target in {raw_name}: {target}"
                ));
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create parent for {}: {e}", out.display())
                })?;
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target, &out).map_err(|e| {
                    format!("Failed to create symlink {}: {e}", out.display())
                })?;
            }
            #[cfg(not(unix))]
            {
                // Windows symlink creation often needs elevation; fall
                // back to a plain file holding the target path so the
                // extract still completes rather than hard-failing.
                fs::write(&out, target.as_bytes()).map_err(|e| {
                    format!("Failed to write symlink placeholder {}: {e}", out.display())
                })?;
            }
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create parent for {}: {e}", out.display())
                })?;
            }
            let mut outfile = fs::File::create(&out)
                .map_err(|e| format!("Failed to create {}: {e}", out.display()))?;
            #[cfg(unix)]
            let mode = entry.unix_mode();
            // Bound actual bytes written (declared size can lie).
            let budget = MAX_EXTRACT_UNCOMPRESSED.saturating_sub(written);
            let mut limited = (&mut entry).take(budget.saturating_add(1));
            let n = io::copy(&mut limited, &mut outfile)
                .map_err(|e| format!("Failed to extract {raw_name}: {e}"))?;
            written = written.saturating_add(n);
            if written > MAX_EXTRACT_UNCOMPRESSED {
                return Err(format!(
                    "Zip uncompressed size exceeds limit while extracting {raw_name}"
                ));
            }
            #[cfg(unix)]
            if let Some(mode) = mode {
                set_unix_mode(&out, mode);
            }
        }

        progress(i as u64 + 1, total);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir().join(format!("k2-fs-extract-{tag}-{nanos}"));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn no_progress() -> impl Fn(u64, u64) + Sync {
        |_, _| {}
    }

    fn tmp_canon(tmp: &TempDir) -> PathBuf {
        tmp.path().canonicalize().unwrap()
    }

    /// Build a simple zip at `zip_path` with the given (name, bytes) files.
    fn write_zip(zip_path: &Path, files: &[(&str, &[u8])]) {
        let f = fs::File::create(zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            if name.ends_with('/') {
                zip.add_directory(*name, opts).unwrap();
            } else {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extract_simple_zip_to_sibling_folder() {
        let tmp = TempDir::new("simple");
        let zip_path = tmp.path().join("proj.zip");
        write_zip(
            &zip_path,
            &[
                ("proj/", b""),
                ("proj/a.txt", b"alpha"),
                ("proj/sub/", b""),
                ("proj/sub/b.txt", b"beta"),
            ],
        );

        let dest = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect("extract succeeds");
        assert_eq!(dest, tmp_canon(&tmp).join("proj"));
        assert_eq!(fs::read_to_string(dest.join("proj/a.txt")).unwrap(), "alpha");
        assert_eq!(
            fs::read_to_string(dest.join("proj/sub/b.txt")).unwrap(),
            "beta"
        );
    }

    #[test]
    fn extract_collision_uses_numbered_sibling() {
        let tmp = TempDir::new("collide");
        let zip_path = tmp.path().join("data.zip");
        write_zip(&zip_path, &[("x.txt", b"hello")]);
        fs::create_dir_all(tmp.path().join("data")).unwrap();
        fs::write(tmp.path().join("data/marker"), b"keep").unwrap();

        let dest = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect("extract succeeds");
        assert_eq!(dest, tmp_canon(&tmp).join("data (1)"));
        assert_eq!(fs::read_to_string(dest.join("x.txt")).unwrap(), "hello");
        // Original sibling untouched.
        assert_eq!(
            fs::read(tmp.path().join("data/marker")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn extract_rejects_zip_slip_dotdot() {
        let tmp = TempDir::new("slip");
        let zip_path = tmp.path().join("evil.zip");
        // Craft a zip whose central-directory name contains `../`.
        // ZipWriter may normalize; write the local header manually if
        // needed. The zip crate's start_file accepts the name as-is for
        // many paths — try `../escape.txt` first.
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            // enclosed_name rejects ParentDir components.
            zip.start_file("../escape.txt", opts).unwrap();
            zip.write_all(b"pwned").unwrap();
            zip.finish().unwrap();
        }

        let err = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect_err("zip-slip must fail loudly");
        assert!(
            err.to_lowercase().contains("zip-slip") || err.contains("unsafe"),
            "got: {err}"
        );
        // No dest folder created (pre-scan fails first).
        assert!(!tmp.path().join("evil").exists());
        // Nothing escaped next to the zip.
        assert!(!tmp.path().join("escape.txt").exists());
    }

    #[test]
    fn extract_rejects_absolute_member_path() {
        let tmp = TempDir::new("abs");
        let zip_path = tmp.path().join("abs.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("/tmp/k2-extract-abs-pwn", opts).unwrap();
            zip.write_all(b"nope").unwrap();
            zip.finish().unwrap();
        }

        let err = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect_err("absolute member must fail");
        assert!(
            err.to_lowercase().contains("zip-slip") || err.contains("unsafe"),
            "got: {err}"
        );
    }

    #[test]
    fn extract_cancel_removes_partial_dest() {
        let tmp = TempDir::new("cancel");
        let zip_path = tmp.path().join("victim.zip");
        write_zip(
            &zip_path,
            &[("a.txt", b"a"), ("b.txt", b"b"), ("c.txt", b"c")],
        );

        let err = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(true), // pre-cancelled
        )
        .expect_err("cancelled extract must fail loudly");
        assert!(err.contains("cancelled"), "got: {err}");

        // No leftover dest (or only absent).
        assert!(
            !tmp.path().join("victim").exists(),
            "partial dest must be removed"
        );
    }

    #[test]
    fn extract_rejects_non_zip_extension() {
        let tmp = TempDir::new("ext");
        let path = tmp.path().join("notes.txt");
        fs::write(&path, b"not a zip").unwrap();
        let err = extract_from_zip(
            path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect_err("non-zip must fail");
        assert!(err.contains(".zip"), "got: {err}");
    }

    #[test]
    fn extract_case_insensitive_zip_extension() {
        let tmp = TempDir::new("case");
        let zip_path = tmp.path().join("Archive.ZIP");
        write_zip(&zip_path, &[("hi.txt", b"hi")]);
        let dest = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect("Extract.ZIP must work");
        assert_eq!(dest, tmp_canon(&tmp).join("Archive"));
        assert_eq!(fs::read_to_string(dest.join("hi.txt")).unwrap(), "hi");
    }

    #[test]
    fn extract_reports_monotonic_progress() {
        use std::sync::Mutex;
        let tmp = TempDir::new("progress");
        let zip_path = tmp.path().join("many.zip");
        let entries: Vec<(String, Vec<u8>)> = (0..10)
            .map(|i| (format!("f{i}.txt"), b"data".to_vec()))
            .collect();
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        write_zip(&zip_path, &refs);

        let seen: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
        extract_from_zip(
            zip_path.to_str().unwrap(),
            &|d, t| seen.lock().unwrap().push((d, t)),
            &AtomicBool::new(false),
        )
        .expect("extract succeeds");
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 10);
        assert!(seen.iter().all(|(_, t)| *t == 10));
        assert_eq!(seen.last().unwrap().0, 10);
        assert!(
            seen.windows(2).all(|w| w[0].0 < w[1].0),
            "progress not monotonic"
        );
    }

    #[test]
    fn is_zip_filename_detects_case() {
        assert!(is_zip_filename("a.zip"));
        assert!(is_zip_filename("A.ZIP"));
        assert!(is_zip_filename("foo.Zip"));
        assert!(is_zip_filename(".zip")); // extension match; stem emptiness is separate
        assert!(!is_zip_filename("a.txt"));
        assert!(!is_zip_filename("zip"));
        assert!(!is_zip_filename("a.zip.bak"));
    }

    #[test]
    fn list_zip_entries_returns_names_and_sizes() {
        let tmp = TempDir::new("list");
        let zip_path = tmp.path().join("bundle.zip");
        write_zip(
            &zip_path,
            &[
                ("readme.txt", b"hello world"),
                ("sub/", b""),
                ("sub/a.txt", b"aa"),
            ],
        );

        let result = list_zip_entries(zip_path.to_str().unwrap()).expect("list succeeds");
        assert!(!result.truncated);
        assert_eq!(result.entries.len(), 3);

        let readme = result
            .entries
            .iter()
            .find(|e| e.name == "readme.txt")
            .expect("readme.txt");
        assert!(!readme.is_dir);
        assert_eq!(readme.size, b"hello world".len() as u64);
        assert!(readme.compressed_size > 0 || readme.size == 0);

        let dir = result
            .entries
            .iter()
            .find(|e| e.name == "sub/" || e.name == "sub")
            .expect("sub dir");
        assert!(dir.is_dir, "expected directory entry, got {dir:?}");
    }

    #[test]
    fn list_zip_entries_includes_zip_slip_names() {
        // Listing is observational — unsafe names appear so the UI can
        // show what's in the archive. Extract still rejects them.
        let tmp = TempDir::new("list-slip");
        let zip_path = tmp.path().join("evil.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("../escape.txt", opts).unwrap();
            zip.write_all(b"pwned").unwrap();
            zip.start_file("safe.txt", opts).unwrap();
            zip.write_all(b"ok").unwrap();
            zip.finish().unwrap();
        }

        let result = list_zip_entries(zip_path.to_str().unwrap()).expect("list succeeds");
        assert_eq!(result.entries.len(), 2);
        assert!(
            result.entries.iter().any(|e| e.name.contains("..")),
            "zip-slip name must still be listed: {:?}",
            result.entries
        );
        assert!(result.entries.iter().any(|e| e.name == "safe.txt"));

        // Extract safety is unchanged — still rejects the slip path.
        let err = extract_from_zip(
            zip_path.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect_err("extract must still reject zip-slip");
        assert!(
            err.to_lowercase().contains("zip-slip") || err.contains("unsafe"),
            "got: {err}"
        );
    }

    #[test]
    fn list_zip_entries_rejects_non_zip() {
        let tmp = TempDir::new("list-ext");
        let path = tmp.path().join("notes.txt");
        fs::write(&path, b"not a zip").unwrap();
        let err = list_zip_entries(path.to_str().unwrap()).expect_err("non-zip must fail");
        assert!(err.contains(".zip"), "got: {err}");
    }

    #[test]
    fn list_zip_entries_truncated_past_cap() {
        let tmp = TempDir::new("list-cap");
        let zip_path = tmp.path().join("huge.zip");
        // Build just over the list cap (avoid writing 5000 real files if
        // cap is large — use a small override via direct ZipWriter loop).
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            // MAX_LIST_ENTRIES + 3 so truncated is definitely true.
            let n = MAX_LIST_ENTRIES + 3;
            for i in 0..n {
                zip.start_file(format!("f{i}.txt"), opts).unwrap();
                zip.write_all(b"x").unwrap();
            }
            zip.finish().unwrap();
        }

        let result = list_zip_entries(zip_path.to_str().unwrap()).expect("list succeeds");
        assert!(result.truncated);
        assert_eq!(result.entries.len(), MAX_LIST_ENTRIES);
    }
}
