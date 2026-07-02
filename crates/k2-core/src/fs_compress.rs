//! Server-side folder → `.zip` compression (0.40.22).
//!
//! The DAEMON owns the filesystem (K2 Connect), so "Compress" on a remote
//! folder must run here — shipping the tree to the client and back would
//! defeat the point. Design constraints:
//!
//! - STREAMING: one entry at a time through a `ZipWriter` over a temp
//!   file — the tree is never held in memory, so a 100 GB folder costs
//!   the same RAM as a 1 MB one.
//! - Deterministic entry ordering (byte-wise sorted names per directory,
//!   depth-first) so the same tree always zips to the same layout.
//! - Finder-style sibling naming: `<name>.zip`, then `<name> 2.zip`,
//!   `<name> 3.zip`, … on collision.
//! - Skips sockets/fifos/device nodes (un-archivable); symlinks are
//!   stored AS links; unix exec bits are preserved per entry.
//! - Durable publish: write to a dot-hidden `.part` sibling, fsync, then
//!   atomic-rename to the final name — a crash never leaves a
//!   complete-looking half-zip.
//! - Cooperative cancel via an `AtomicBool`, checked between entries; a
//!   cancelled job removes its `.part`.
//!
//! Programmatic by design (not context-menu-shaped): the planned
//! "clone workspace to your computer" pull flow compresses a whole
//! workspace dir through this same function.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::fs_commands::validate_path;

/// One zippable entry, discovered by [`plan_entries`].
enum PlannedEntry {
    Dir(PathBuf),
    File(PathBuf),
    Symlink(PathBuf),
}

impl PlannedEntry {
    fn path(&self) -> &Path {
        match self {
            PlannedEntry::Dir(p) | PlannedEntry::File(p) | PlannedEntry::Symlink(p) => p,
        }
    }
}

/// Depth-first, byte-wise-sorted walk of `root`, classifying every entry.
/// Sockets/fifos/devices are silently skipped (they cannot live in a zip);
/// symlinks are NOT followed — they're recorded as links, which also makes
/// a symlink cycle harmless. Returns the flat plan; its length is the
/// progress denominator.
fn plan_entries(root: &Path) -> Result<Vec<PlannedEntry>, String> {
    let mut plan = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| format!("Failed to read {}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        // Reverse-sort so the LIFO stack pops directories in ascending
        // order — the deterministic-layout guarantee.
        children.sort_unstable_by(|a, b| b.cmp(a));
        for child in children {
            let meta = fs::symlink_metadata(&child)
                .map_err(|e| format!("Failed to stat {}: {e}", child.display()))?;
            let ft = meta.file_type();
            if ft.is_symlink() {
                plan.push(PlannedEntry::Symlink(child));
            } else if ft.is_dir() {
                plan.push(PlannedEntry::Dir(child.clone()));
                stack.push(child);
            } else if ft.is_file() {
                plan.push(PlannedEntry::File(child));
            }
            // else: socket/fifo/device — skipped.
        }
    }
    // The stack pops dirs depth-first but plan order interleaves; re-sort
    // the flat plan by path for a single stable, prefix-grouped order.
    plan.sort_unstable_by(|a, b| a.path().cmp(b.path()));
    Ok(plan)
}

/// Finder-style non-colliding sibling: `<stem>.zip`, `<stem> 2.zip`, ….
fn zip_sibling_path(parent: &Path, stem: &str) -> PathBuf {
    let first = parent.join(format!("{stem}.zip"));
    if !first.exists() {
        return first;
    }
    for i in 2..10_000u32 {
        let candidate = parent.join(format!("{stem} {i}.zip"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological (10k siblings) — effectively unreachable; the caller's
    // rename will fail loudly rather than overwrite silently.
    first
}

/// Unix mode bits for the entry options (preserves exec bits). Non-unix
/// hosts have no mode to preserve.
#[cfg(unix)]
fn unix_mode(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}
#[cfg(not(unix))]
fn unix_mode(_meta: &fs::Metadata) -> Option<u32> {
    None
}

/// Zip64 threshold: entries at/over 4 GiB need `large_file` (zip64) set
/// BEFORE writing; setting it always would waste ~20 bytes/entry.
const ZIP64_THRESHOLD: u64 = 0xFFFF_FFFF;

/// Compress the folder (or single file) at `src` into a sibling
/// `<name>.zip`, streaming entry-by-entry. `progress(done, total)` fires
/// after every entry; `cancel` is polled between entries. Returns the
/// final zip path.
pub fn compress_to_zip(
    src: &str,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let src_path = validate_path(src)?;
    if !src_path.exists() {
        return Err(format!("Not found: {src}"));
    }
    let parent = src_path
        .parent()
        .ok_or_else(|| "Cannot compress the filesystem root".to_string())?
        .to_path_buf();
    let name = src_path
        .file_name()
        .ok_or_else(|| "Source has no name".to_string())?
        .to_string_lossy()
        .to_string();

    // Plan first so progress has a real denominator. Entry names inside
    // the zip are relative to PARENT — unzipping reproduces `<name>/…`
    // beside the archive, exactly like Finder.
    let plan: Vec<PlannedEntry> = if src_path.is_dir() {
        let mut p = vec![PlannedEntry::Dir(src_path.clone())];
        p.extend(plan_entries(&src_path)?);
        p
    } else {
        vec![PlannedEntry::File(src_path.clone())]
    };
    let total = plan.len() as u64;

    // Dot-hidden temp in the SAME dir as the final zip (rename must not
    // cross filesystems). Nanos + pid keep concurrent jobs apart.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let part_path = parent.join(format!(".k2-zip-{}-{nanos}.part", std::process::id()));

    let result = write_zip(&part_path, &parent, &plan, total, progress, cancel);
    match result {
        Ok(()) => {
            // Collision check at PUBLISH time (the walk may have taken
            // minutes; a same-name zip could have appeared).
            let target = zip_sibling_path(&parent, &name);
            fs::rename(&part_path, &target).map_err(|e| {
                let _ = fs::remove_file(&part_path);
                format!("Failed to publish zip: {e}")
            })?;
            Ok(target)
        }
        Err(e) => {
            let _ = fs::remove_file(&part_path);
            Err(e)
        }
    }
}

/// Inner streaming writer — separated so EVERY failure path in the caller
/// can remove the `.part` in one place.
fn write_zip(
    part_path: &Path,
    base: &Path,
    plan: &[PlannedEntry],
    total: u64,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<(), String> {
    use zip::write::SimpleFileOptions;

    let file = fs::File::create(part_path)
        .map_err(|e| format!("Failed to create temp zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));

    let mut done: u64 = 0;
    for entry in plan {
        if cancel.load(Ordering::Relaxed) {
            return Err("Compression cancelled".to_string());
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(base)
            .map_err(|_| format!("Entry escaped the source tree: {}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let meta = fs::symlink_metadata(path)
            .map_err(|e| format!("Failed to stat {}: {e}", path.display()))?;
        let mut options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        if let Some(mode) = unix_mode(&meta) {
            options = options.unix_permissions(mode);
        }

        match entry {
            PlannedEntry::Dir(_) => {
                zip.add_directory(&rel, options)
                    .map_err(|e| format!("Failed to add directory {rel}: {e}"))?;
            }
            PlannedEntry::Symlink(_) => {
                let target = fs::read_link(path)
                    .map_err(|e| format!("Failed to read link {rel}: {e}"))?;
                zip.add_symlink(&rel, target.to_string_lossy(), options)
                    .map_err(|e| format!("Failed to add symlink {rel}: {e}"))?;
            }
            PlannedEntry::File(_) => {
                if meta.len() >= ZIP64_THRESHOLD {
                    options = options.large_file(true);
                }
                zip.start_file(&rel, options)
                    .map_err(|e| format!("Failed to start entry {rel}: {e}"))?;
                let mut f = fs::File::open(path)
                    .map_err(|e| format!("Failed to open {rel}: {e}"))?;
                std::io::copy(&mut f, &mut zip)
                    .map_err(|e| format!("Failed to compress {rel}: {e}"))?;
            }
        }
        done += 1;
        progress(done, total);
    }

    let mut inner = zip
        .finish()
        .map_err(|e| format!("Failed to finalize zip: {e}"))?;
    inner
        .flush()
        .map_err(|e| format!("Failed to flush zip: {e}"))?;
    // fsync BEFORE the caller's atomic rename — never publish lazy bytes.
    inner
        .into_inner()
        .map_err(|e| format!("Failed to flush zip: {e}"))?
        .sync_all()
        .map_err(|e| format!("Failed to fsync zip: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir().join(format!("k2-fs-compress-{tag}-{nanos}"));
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

    fn read_zip_names(path: &Path) -> Vec<String> {
        let f = fs::File::open(path).expect("zip opens");
        let mut z = zip::ZipArchive::new(f).expect("zip parses");
        (0..z.len())
            .map(|i| z.by_index(i).expect("entry").name().to_string())
            .collect()
    }

    #[test]
    fn compress_produces_sibling_zip_with_full_tree() {
        let tmp = TempDir::new("tree");
        let src = tmp.path().join("proj");
        fs::create_dir_all(src.join("sub/inner")).unwrap();
        fs::write(src.join("a.txt"), b"alpha").unwrap();
        fs::write(src.join("sub/b.txt"), b"beta").unwrap();
        fs::write(src.join("sub/inner/c.bin"), vec![7u8; 4096]).unwrap();

        let zip_path = compress_to_zip(
            src.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect("compress succeeds");
        assert_eq!(zip_path, tmp_canon(&tmp).join("proj.zip"));

        let names = read_zip_names(&zip_path);
        // Entries are parent-relative (unzip reproduces `proj/…`) and in
        // deterministic sorted order.
        assert_eq!(
            names,
            vec![
                "proj/",
                "proj/a.txt",
                "proj/sub/",
                "proj/sub/b.txt",
                "proj/sub/inner/",
                "proj/sub/inner/c.bin",
            ],
        );

        // Round-trip one file's bytes.
        let f = fs::File::open(&zip_path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut buf = String::new();
        z.by_name("proj/a.txt").unwrap().read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "alpha");
    }

    /// TempDir path canonicalized — /tmp is a symlink on macOS and
    /// compress_to_zip canonicalizes via validate_path.
    fn tmp_canon(tmp: &TempDir) -> PathBuf {
        tmp.path().canonicalize().unwrap()
    }

    #[test]
    fn compress_collision_appends_finder_style_counter() {
        let tmp = TempDir::new("collide");
        let src = tmp.path().join("data");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x"), b"x").unwrap();
        fs::write(tmp.path().join("data.zip"), b"existing").unwrap();
        fs::write(tmp.path().join("data 2.zip"), b"existing too").unwrap();

        let zip_path = compress_to_zip(
            src.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect("compress succeeds");
        assert_eq!(zip_path, tmp_canon(&tmp).join("data 3.zip"));
        // Pre-existing zips untouched.
        assert_eq!(fs::read(tmp.path().join("data.zip")).unwrap(), b"existing");
    }

    #[test]
    fn compress_reports_monotonic_progress_with_real_total() {
        use std::sync::Mutex;
        let tmp = TempDir::new("progress");
        let src = tmp.path().join("many");
        fs::create_dir_all(&src).unwrap();
        for i in 0..25 {
            fs::write(src.join(format!("f{i:02}.txt")), b"data").unwrap();
        }
        let seen: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
        compress_to_zip(
            src.to_str().unwrap(),
            &|d, t| seen.lock().unwrap().push((d, t)),
            &AtomicBool::new(false),
        )
        .expect("compress succeeds");
        let seen = seen.into_inner().unwrap();
        // 25 files + the root dir entry.
        assert_eq!(seen.len(), 26);
        assert!(seen.iter().all(|(_, t)| *t == 26));
        assert_eq!(seen.last().unwrap().0, 26);
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0), "progress not monotonic");
    }

    #[test]
    fn compress_cancel_removes_part_and_creates_no_zip() {
        let tmp = TempDir::new("cancel");
        let src = tmp.path().join("victim");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f.txt"), b"data").unwrap();

        let err = compress_to_zip(
            src.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(true), // pre-cancelled → first entry check trips
        )
        .expect_err("cancelled compress must fail loudly");
        assert!(err.contains("cancelled"), "got: {err}");

        // No zip, no leftover .part.
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".zip") || n.contains(".part"))
            .collect();
        assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn compress_preserves_exec_bit_and_stores_symlink_as_link() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("mode");
        let src = tmp.path().join("bin");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("run.sh"), b"#!/bin/sh\n").unwrap();
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("run.sh", src.join("alias")).unwrap();

        let zip_path = compress_to_zip(
            src.to_str().unwrap(),
            &no_progress(),
            &AtomicBool::new(false),
        )
        .expect("compress succeeds");

        let f = fs::File::open(&zip_path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let entry = z.by_name("bin/run.sh").expect("script entry");
        assert_eq!(
            entry.unix_mode().map(|m| m & 0o777),
            Some(0o755),
            "exec bits must survive"
        );
        drop(entry);
        // The symlink is present as an entry (stored as a link, its data is
        // the target path — NOT a copy of run.sh's contents).
        let mut link = z.by_name("bin/alias").expect("symlink entry");
        let mut target = String::new();
        link.read_to_string(&mut target).unwrap();
        assert_eq!(target, "run.sh");
    }

    #[test]
    fn compress_skips_fifo_but_keeps_files() {
        #[cfg(unix)]
        {
            let tmp = TempDir::new("fifo");
            let src = tmp.path().join("mix");
            fs::create_dir_all(&src).unwrap();
            fs::write(src.join("keep.txt"), b"kept").unwrap();
            let fifo = src.join("pipe");
            let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo failed");

            let zip_path = compress_to_zip(
                src.to_str().unwrap(),
                &no_progress(),
                &AtomicBool::new(false),
            )
            .expect("compress succeeds despite fifo");
            let names = read_zip_names(&zip_path);
            assert_eq!(names, vec!["mix/", "mix/keep.txt"]);
        }
    }
}
