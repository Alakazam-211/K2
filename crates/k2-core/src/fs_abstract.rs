//! Filesystem abstraction — the testability seam inspired by Zed's
//! `crates/fs/src/fs.rs::trait Fs` + `FakeFs`.
//!
//! # Why this exists
//!
//! Most of K2SO's business logic (agent scaffolding, SKILL.md
//! regeneration, work-item triage, heartbeat wake composition) is
//! entangled with `std::fs::*` calls. That coupling means every unit
//! test has to scaffold a real tempdir on disk — slow on macOS, and
//! it forces tests into a "does the happy path work" shape because
//! simulating error cases (permission denied, disk full, a file
//! suddenly disappearing mid-operation) requires root or exotic OS
//! tricks.
//!
//! Zed's answer is a `trait Fs` with two impls:
//! - `RealFs` wraps `std::fs` for production.
//! - `FakeFs` holds an in-memory BTreeMap tree that tests can
//!   construct, mutate, and observe directly.
//!
//! Because business logic takes `&dyn Fs` instead of reaching for
//! `std::fs::*`, tests swap `FakeFs` in and skip disk I/O entirely.
//! Zed's suite runs thousands of filesystem-touching tests in
//! seconds because of this seam.
//!
//! # Scope for K2SO
//!
//! We port the essential methods but skip Zed's heavy extras:
//! - **No file watchers** — K2SO uses `notify` directly for watchers
//!   and the UX wrapper doesn't need the same abstraction.
//! - **No git integration on the trait** — we keep `git2` out; tests
//!   that need git state scaffold a real repo.
//! - **No async executor** — Zed's `BackgroundExecutor` + custom
//!   scheduler are ~1000 LOC of machinery K2SO doesn't need. K2SO's
//!   fs ops are synchronous today; if that changes we can add async
//!   methods later.
//!
//! # Feature gating
//!
//! `RealFs` is always compiled. `FakeFs` is `#[cfg(test)]` — there's
//! no scenario where production code needs it, and keeping it out of
//! release builds shrinks the binary.
//!
//! # Why `#[allow(dead_code)]`
//!
//! The trait, `RealFs`, and `FsMetadata` are public surface the rest
//! of the codebase will consume when Phase E-bis threads `&dyn Fs`
//! through the heartbeat-triage path in `agent_hooks.rs`. Until that
//! migration lands, nothing constructs `RealFs` in production (tests
//! build both `RealFs` + `FakeFs` to assert parity). The allow avoids
//! a misleading "never used" warning for an API that is intentionally
//! pending adoption, not forgotten.

#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Metadata returned by `Fs::metadata`. Keep minimal — only the
/// fields K2SO business logic actually reads.
#[derive(Debug, Clone, PartialEq)]
pub struct FsMetadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// The filesystem abstraction. All methods operate on `&self` so
/// impls can be shared via `Arc<dyn Fs>` across threads.
///
/// Signatures intentionally mirror `std::fs` for ease of substitution
/// at call sites — a caller that was doing `std::fs::read_to_string(p)`
/// becomes `fs.read_to_string(p)` with only the receiver change.
pub trait Fs: Send + Sync {
    /// Read full file contents as UTF-8. Errors propagate like
    /// `std::fs::read_to_string` (NotFound, permission, invalid
    /// UTF-8, etc.).
    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    /// Read full file contents as bytes.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Write bytes to a file, creating or replacing as needed.
    /// Atomicity is NOT guaranteed by this trait — use
    /// `crate::fs_atomic::atomic_write` via a helper if needed.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;

    /// Check whether a path exists. Returns false for any error
    /// (matching `Path::exists`).
    fn exists(&self, path: &Path) -> bool;

    /// Stat the path. Returns NotFound if missing.
    fn metadata(&self, path: &Path) -> io::Result<FsMetadata>;

    /// Is `path` a directory? Convenience wrapper — cheaper than
    /// `metadata` on some backends (FakeFs does a single map lookup).
    fn is_dir(&self, path: &Path) -> bool {
        self.metadata(path).map(|m| m.is_dir).unwrap_or(false)
    }

    /// Is `path` a regular file (not symlink, not dir)?
    fn is_file(&self, path: &Path) -> bool {
        self.metadata(path).map(|m| m.is_file).unwrap_or(false)
    }

    /// List entries in `dir`. Returns entries as absolute paths.
    /// Order is implementation-defined — callers that need stable
    /// order should sort the result themselves.
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>>;

    /// Create `dir` and all missing parents. No-op if `dir` already
    /// exists as a directory.
    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;

    /// Delete a regular file. NotFound errors are propagated —
    /// callers that want "best effort" should match on the error
    /// kind or use `remove_file_if_exists`.
    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// Delete a directory and its entire contents recursively.
    fn remove_dir_all(&self, dir: &Path) -> io::Result<()>;

    /// Rename (atomic on real POSIX filesystems; not guaranteed by
    /// the trait). Preserves content identity.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Create a symlink at `link` pointing to `target`. On non-Unix
    /// hosts some impls may fall back to copy; caller must not
    /// assume symlinks are always first-class.
    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()>;

    /// Read the destination of a symlink at `path`.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    /// Copy a file (follows symlinks for the source). Returns bytes
    /// written.
    fn copy(&self, from: &Path, to: &Path) -> io::Result<u64>;
}

/// Production impl — delegates straight through to `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

impl RealFs {
    pub fn new() -> Self {
        Self
    }
}

impl Fs for RealFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn metadata(&self, path: &Path) -> io::Result<FsMetadata> {
        // symlink_metadata() reports the link itself (not its
        // target) — matches std::fs::symlink_metadata semantics.
        // FakeFs does the same. Callers that want target metadata
        // can canonicalize first.
        let meta = std::fs::symlink_metadata(path)?;
        Ok(FsMetadata {
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            is_symlink: meta.file_type().is_symlink(),
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }

    fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        let entries = std::fs::read_dir(dir)?;
        let mut out = Vec::new();
        for entry in entries {
            out.push(entry?.path());
        }
        Ok(out)
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir_all(&self, dir: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(dir)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(not(unix))]
        {
            std::fs::copy(target, link).map(|_| ())
        }
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::read_link(path)
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<u64> {
        std::fs::copy(from, to)
    }
}

// ── FakeFs ──────────────────────────────────────────────────────────
//
// Test-only in-memory filesystem. Compiled only under `cfg(test)` —
// lands in no production artifact.

// FakeFs is available whenever cfg(test) is active for k2so-core OR the
// `test-util` feature is enabled — the latter lets downstream crates'
// (src-tauri's) test binaries reach FakeFs even though cfg(test) in
// k2so-core is off when compiled as a library dep.
#[cfg(any(test, feature = "test-util"))]
pub use fake::FakeFs;

#[cfg(any(test, feature = "test-util"))]
mod fake {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// Entry in the fake fs tree. Either a file (byte payload +
    /// mtime) or a directory (keys → child entries) or a symlink
    /// (target path, resolved lazily on read).
    #[derive(Debug, Clone)]
    enum Entry {
        File { bytes: Vec<u8>, mtime: SystemTime },
        Dir { entries: BTreeMap<String, Entry> },
        Symlink { target: PathBuf, mtime: SystemTime },
    }

    impl Entry {
        fn new_dir() -> Self {
            Entry::Dir { entries: BTreeMap::new() }
        }
        fn new_file(bytes: Vec<u8>) -> Self {
            Entry::File { bytes, mtime: SystemTime::now() }
        }
        fn new_symlink(target: PathBuf) -> Self {
            Entry::Symlink { target, mtime: SystemTime::now() }
        }
    }

    /// Mutable state behind an Arc<Mutex<_>> so FakeFs is Send+Sync
    /// and matches the RealFs interface (which is trivially
    /// thread-safe via std::fs).
    #[derive(Debug)]
    struct State {
        root: Entry,
        /// Per-path count of `write()` calls. Lets tests assert
        /// "this file was written exactly once" — Zed uses this
        /// pattern for regression protection.
        write_counts: BTreeMap<PathBuf, usize>,
        /// Metadata-call counter — detects tests that do O(n) stats
        /// where O(1) is expected.
        metadata_calls: usize,
        /// read_dir counter for the same reason.
        read_dir_calls: usize,
    }

    impl State {
        fn new() -> Self {
            Self {
                root: Entry::new_dir(),
                write_counts: BTreeMap::new(),
                metadata_calls: 0,
                read_dir_calls: 0,
            }
        }
    }

    /// In-memory filesystem for tests.
    ///
    /// Key properties:
    /// - All state behind a single Mutex — deterministic scheduling
    ///   under parallel access, matches how Zed's FakeFs works.
    /// - Absolute paths only: all operations require `/`-rooted
    ///   paths. Passing a relative path panics (test-only, loud).
    /// - No permission model: tests that need to simulate
    ///   permission-denied can inject errors via `fail_next_write`
    ///   if that becomes important (not implemented today).
    #[derive(Debug, Clone)]
    pub struct FakeFs {
        inner: Arc<Mutex<State>>,
    }

    impl FakeFs {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: Arc::new(Mutex::new(State::new())),
            })
        }

        /// JSON DSL for seeding the fake tree. Mirrors Zed's
        /// `fs.insert_tree(path, json!({...}))` ergonomics. Each
        /// object becomes a directory; each string becomes a file
        /// containing that string. Arrays are not supported.
        ///
        /// ```ignore
        /// fs.insert_tree("/root", serde_json::json!({
        ///   "a.txt": "hello",
        ///   "sub": { "b.md": "# header" }
        /// }));
        /// ```
        pub fn insert_tree(&self, root: &Path, tree: serde_json::Value) {
            assert!(root.is_absolute(), "FakeFs requires absolute paths: {}", root.display());
            self.insert_tree_inner(root, &tree);
        }

        fn insert_tree_inner(&self, at: &Path, value: &serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    // Create the directory itself, then recurse into children.
                    self.create_dir_all_inner(at).expect("fakefs insert_tree dir");
                    for (key, child) in map {
                        let child_path = at.join(key);
                        self.insert_tree_inner(&child_path, child);
                    }
                }
                serde_json::Value::String(s) => {
                    if let Some(parent) = at.parent() {
                        self.create_dir_all_inner(parent).expect("fakefs insert_tree parent");
                    }
                    self.write_inner(at, s.as_bytes()).expect("fakefs insert_tree write");
                }
                other => panic!(
                    "FakeFs insert_tree: unsupported value at {}: {:?}",
                    at.display(),
                    other
                ),
            }
        }

        /// Insert a single file outside the JSON DSL.
        pub fn insert_file(&self, path: &Path, bytes: &[u8]) {
            assert!(path.is_absolute(), "FakeFs requires absolute paths");
            if let Some(parent) = path.parent() {
                self.create_dir_all_inner(parent).expect("fakefs insert_file parent");
            }
            self.write_inner(path, bytes).expect("fakefs insert_file");
        }

        /// Total write count for a specific path — assertion tool
        /// for "this file was written exactly once" regressions.
        pub fn write_count(&self, path: &Path) -> usize {
            self.inner.lock().write_counts.get(path).copied().unwrap_or(0)
        }

        /// Cumulative metadata() calls. Useful for catching loops
        /// that stat every file in a directory when a single
        /// read_dir would do.
        pub fn metadata_call_count(&self) -> usize {
            self.inner.lock().metadata_calls
        }

        /// Cumulative read_dir() calls.
        pub fn read_dir_call_count(&self) -> usize {
            self.inner.lock().read_dir_calls
        }

        /// List every file (not directories) currently in the tree,
        /// sorted for test determinism.
        pub fn files(&self) -> Vec<PathBuf> {
            let mut out = Vec::new();
            let state = self.inner.lock();
            collect_files(&state.root, Path::new("/"), &mut out);
            out.sort();
            out
        }

        // Internal helpers below operate on the tree. Panicking
        // path-normalization helpers (absolute-only) document the
        // contract loudly for test authors.

        fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
            let mut state = self.inner.lock();
            f(&mut state)
        }

        fn split_path(&self, path: &Path) -> Vec<String> {
            assert!(path.is_absolute(), "FakeFs requires absolute paths: {}", path.display());
            path.components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                    std::path::Component::RootDir => None,
                    other => panic!("unsupported path component: {:?}", other),
                })
                .collect()
        }

        fn get_entry<'a>(&'a self, state: &'a State, path: &Path) -> Option<&'a Entry> {
            let parts = self.split_path(path);
            let mut cur = &state.root;
            for part in parts {
                match cur {
                    Entry::Dir { entries } => {
                        cur = entries.get(&part)?;
                    }
                    _ => return None,
                }
            }
            Some(cur)
        }

        fn get_entry_mut<'a>(&self, state: &'a mut State, path: &Path) -> Option<&'a mut Entry> {
            let parts = self.split_path(path);
            let mut cur = &mut state.root;
            for part in parts {
                match cur {
                    Entry::Dir { entries } => {
                        cur = entries.get_mut(&part)?;
                    }
                    _ => return None,
                }
            }
            Some(cur)
        }

        fn resolve_symlink<'a>(&'a self, state: &'a State, path: &Path) -> Option<&'a Entry> {
            let mut current = path.to_path_buf();
            for _ in 0..16 {
                let entry = self.get_entry(state, &current)?;
                match entry {
                    Entry::Symlink { target, .. } => {
                        current = if target.is_absolute() {
                            target.clone()
                        } else {
                            current.parent().unwrap_or(Path::new("/")).join(target)
                        };
                    }
                    _ => return Some(entry),
                }
            }
            None // symlink cycle
        }

        fn create_dir_all_inner(&self, dir: &Path) -> io::Result<()> {
            self.with_state(|state| {
                let parts = self.split_path(dir);
                let mut cur = &mut state.root;
                for part in parts {
                    let next = match cur {
                        Entry::Dir { entries } => entries
                            .entry(part)
                            .or_insert_with(Entry::new_dir),
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::NotADirectory,
                                format!("intermediate path is not a dir: {}", dir.display()),
                            ))
                        }
                    };
                    cur = next;
                }
                Ok(())
            })
        }

        fn write_inner(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            let parent = path.parent().unwrap_or(Path::new("/"));
            self.create_dir_all_inner(parent)?;
            let file_name = path
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
                .to_string_lossy()
                .into_owned();

            self.with_state(|state| {
                *state.write_counts.entry(path.to_path_buf()).or_insert(0) += 1;
                let parent_entry = self.get_entry_mut(state, parent).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, format!("parent missing: {}", parent.display()))
                })?;
                match parent_entry {
                    Entry::Dir { entries } => {
                        entries.insert(file_name, Entry::new_file(bytes.to_vec()));
                        Ok(())
                    }
                    _ => Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        format!("parent not a dir: {}", parent.display()),
                    )),
                }
            })
        }
    }

    fn collect_files(entry: &Entry, at: &Path, out: &mut Vec<PathBuf>) {
        match entry {
            Entry::Dir { entries } => {
                for (k, v) in entries {
                    collect_files(v, &at.join(k), out);
                }
            }
            Entry::File { .. } => out.push(at.to_path_buf()),
            Entry::Symlink { .. } => {} // don't list symlinks as files
        }
    }

    impl Fs for FakeFs {
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            let bytes = self.read(path)?;
            String::from_utf8(bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }

        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            let state = self.inner.lock();
            let entry = self
                .resolve_symlink(&state, path)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.display().to_string()))?;
            match entry {
                Entry::File { bytes, .. } => Ok(bytes.clone()),
                Entry::Dir { .. } => Err(io::Error::new(
                    io::ErrorKind::IsADirectory,
                    path.display().to_string(),
                )),
                Entry::Symlink { .. } => unreachable!("resolve_symlink eliminates Symlink"),
            }
        }

        fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.write_inner(path, bytes)
        }

        fn exists(&self, path: &Path) -> bool {
            let state = self.inner.lock();
            self.get_entry(&state, path).is_some()
        }

        fn metadata(&self, path: &Path) -> io::Result<FsMetadata> {
            let mut state = self.inner.lock();
            state.metadata_calls += 1;
            let entry = self
                .get_entry(&state, path)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.display().to_string()))?;
            let md = match entry {
                Entry::File { bytes, mtime } => FsMetadata {
                    is_dir: false,
                    is_file: true,
                    is_symlink: false,
                    len: bytes.len() as u64,
                    modified: Some(*mtime),
                },
                Entry::Dir { .. } => FsMetadata {
                    is_dir: true,
                    is_file: false,
                    is_symlink: false,
                    len: 0,
                    modified: None,
                },
                Entry::Symlink { mtime, .. } => FsMetadata {
                    is_dir: false,
                    is_file: false,
                    is_symlink: true,
                    len: 0,
                    modified: Some(*mtime),
                },
            };
            Ok(md)
        }

        fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
            let mut state = self.inner.lock();
            state.read_dir_calls += 1;
            let entry = self
                .get_entry(&state, dir)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, dir.display().to_string()))?;
            match entry {
                Entry::Dir { entries } => Ok(entries.keys().map(|k| dir.join(k)).collect()),
                _ => Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    dir.display().to_string(),
                )),
            }
        }

        fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
            self.create_dir_all_inner(dir)
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
                .to_string_lossy()
                .into_owned();
            self.with_state(|state| {
                let parent_entry = self.get_entry_mut(state, &parent).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, parent.display().to_string())
                })?;
                match parent_entry {
                    Entry::Dir { entries } => {
                        let removed = entries.remove(&name);
                        if removed.is_none() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                path.display().to_string(),
                            ));
                        }
                        Ok(())
                    }
                    _ => Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        parent.display().to_string(),
                    )),
                }
            })
        }

        fn remove_dir_all(&self, dir: &Path) -> io::Result<()> {
            let parent = dir.parent().unwrap_or(Path::new("/")).to_path_buf();
            let name = dir
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no dir name"))?
                .to_string_lossy()
                .into_owned();
            self.with_state(|state| {
                let parent_entry = self.get_entry_mut(state, &parent).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, parent.display().to_string())
                })?;
                match parent_entry {
                    Entry::Dir { entries } => {
                        if entries.remove(&name).is_none() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                dir.display().to_string(),
                            ));
                        }
                        Ok(())
                    }
                    _ => Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        parent.display().to_string(),
                    )),
                }
            })
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            // Remove source entry, insert it at the destination. If
            // dest exists, it's overwritten — matches POSIX rename
            // semantics for same-type replacements.
            let from_parent = from.parent().unwrap_or(Path::new("/")).to_path_buf();
            let from_name = from
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no src name"))?
                .to_string_lossy()
                .into_owned();
            let to_parent = to.parent().unwrap_or(Path::new("/")).to_path_buf();
            let to_name = to
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no dst name"))?
                .to_string_lossy()
                .into_owned();
            // Ensure target parent exists first — same requirement
            // as std::fs::rename: POSIX rename errors if the target
            // dir is missing.
            self.create_dir_all_inner(&to_parent)?;

            self.with_state(|state| {
                // Pop source.
                let entry = {
                    let parent_entry = self.get_entry_mut(state, &from_parent).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, from.display().to_string())
                    })?;
                    match parent_entry {
                        Entry::Dir { entries } => entries.remove(&from_name).ok_or_else(|| {
                            io::Error::new(io::ErrorKind::NotFound, from.display().to_string())
                        })?,
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::NotADirectory,
                                from_parent.display().to_string(),
                            ))
                        }
                    }
                };
                // Put at destination.
                let dest_parent = self.get_entry_mut(state, &to_parent).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, to_parent.display().to_string())
                })?;
                match dest_parent {
                    Entry::Dir { entries } => {
                        entries.insert(to_name, entry);
                        Ok(())
                    }
                    _ => Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        to_parent.display().to_string(),
                    )),
                }
            })
        }

        fn symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
            let parent = link.parent().unwrap_or(Path::new("/")).to_path_buf();
            let name = link
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no link name"))?
                .to_string_lossy()
                .into_owned();
            self.create_dir_all_inner(&parent)?;
            self.with_state(|state| {
                let parent_entry = self.get_entry_mut(state, &parent).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, parent.display().to_string())
                })?;
                match parent_entry {
                    Entry::Dir { entries } => {
                        entries.insert(name, Entry::new_symlink(target.to_path_buf()));
                        Ok(())
                    }
                    _ => Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        parent.display().to_string(),
                    )),
                }
            })
        }

        fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
            let state = self.inner.lock();
            let entry = self.get_entry(&state, path).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, path.display().to_string())
            })?;
            match entry {
                Entry::Symlink { target, .. } => Ok(target.clone()),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("not a symlink: {}", path.display()),
                )),
            }
        }

        fn copy(&self, from: &Path, to: &Path) -> io::Result<u64> {
            let bytes = self.read(from)?;
            let len = bytes.len() as u64;
            self.write(to, &bytes)?;
            Ok(len)
        }
    }
}

#[cfg(test)]
mod tests {
    //! Parity + correctness tests for the Fs trait. Every test
    //! exercises the same operation on RealFs (with a tempdir) and
    //! FakeFs; any behavioral difference is a bug in the fake.
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmpdir() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "k2so-fsabs-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    // ── FakeFs basics ────────────────────────────────────────────
    #[test]
    fn fake_fs_insert_tree_and_files() {
        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/root"),
            serde_json::json!({
                "a.txt": "hello",
                "sub": { "b.md": "# header" },
            }),
        );
        let files = fs.files();
        assert_eq!(
            files,
            vec![PathBuf::from("/root/a.txt"), PathBuf::from("/root/sub/b.md")]
        );
    }

    #[test]
    fn fake_fs_read_write_roundtrip() {
        let fs = FakeFs::new();
        fs.write(Path::new("/hello.txt"), b"world").unwrap();
        assert_eq!(fs.read_to_string(Path::new("/hello.txt")).unwrap(), "world");
    }

    #[test]
    fn fake_fs_read_missing_returns_not_found() {
        let fs = FakeFs::new();
        let err = fs.read_to_string(Path::new("/nope.txt")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn fake_fs_write_count_tracks_rewrites() {
        let fs = FakeFs::new();
        let p = Path::new("/counted.txt");
        fs.write(p, b"one").unwrap();
        fs.write(p, b"two").unwrap();
        fs.write(p, b"three").unwrap();
        assert_eq!(fs.write_count(p), 3);
    }

    #[test]
    fn fake_fs_metadata_and_is_dir_is_file() {
        let fs = FakeFs::new();
        fs.insert_file(Path::new("/dir/file.md"), b"body");
        assert!(fs.is_file(Path::new("/dir/file.md")));
        assert!(!fs.is_dir(Path::new("/dir/file.md")));
        assert!(fs.is_dir(Path::new("/dir")));
        assert!(!fs.is_file(Path::new("/dir")));
        let md = fs.metadata(Path::new("/dir/file.md")).unwrap();
        assert_eq!(md.len, 4);
        assert!(md.is_file);
        assert!(!md.is_dir);
    }

    #[test]
    fn fake_fs_read_dir_returns_absolute_paths_sorted() {
        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/d"),
            serde_json::json!({ "z.txt": "z", "a.txt": "a", "m.txt": "m" }),
        );
        let mut entries = fs.read_dir(Path::new("/d")).unwrap();
        entries.sort();
        assert_eq!(
            entries,
            vec![
                PathBuf::from("/d/a.txt"),
                PathBuf::from("/d/m.txt"),
                PathBuf::from("/d/z.txt"),
            ]
        );
    }

    #[test]
    fn fake_fs_create_dir_all_is_idempotent() {
        let fs = FakeFs::new();
        fs.create_dir_all(Path::new("/a/b/c")).unwrap();
        fs.create_dir_all(Path::new("/a/b/c")).unwrap(); // no-op
        assert!(fs.is_dir(Path::new("/a/b/c")));
    }

    #[test]
    fn fake_fs_rename_moves_file_atomically() {
        let fs = FakeFs::new();
        fs.write(Path::new("/src.txt"), b"x").unwrap();
        fs.rename(Path::new("/src.txt"), Path::new("/dst.txt")).unwrap();
        assert!(!fs.exists(Path::new("/src.txt")));
        assert!(fs.exists(Path::new("/dst.txt")));
        assert_eq!(fs.read_to_string(Path::new("/dst.txt")).unwrap(), "x");
    }

    #[test]
    fn fake_fs_rename_overwrites_existing_dest() {
        let fs = FakeFs::new();
        fs.write(Path::new("/src.txt"), b"new").unwrap();
        fs.write(Path::new("/dst.txt"), b"old").unwrap();
        fs.rename(Path::new("/src.txt"), Path::new("/dst.txt")).unwrap();
        assert_eq!(fs.read_to_string(Path::new("/dst.txt")).unwrap(), "new");
    }

    #[test]
    fn fake_fs_remove_file_and_remove_dir_all() {
        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/r"),
            serde_json::json!({ "a.txt": "a", "sub": { "b.txt": "b" } }),
        );
        fs.remove_file(Path::new("/r/a.txt")).unwrap();
        assert!(!fs.exists(Path::new("/r/a.txt")));
        fs.remove_dir_all(Path::new("/r/sub")).unwrap();
        assert!(!fs.exists(Path::new("/r/sub")));
        assert!(!fs.exists(Path::new("/r/sub/b.txt")));
    }

    #[test]
    fn fake_fs_symlink_resolves_through_read() {
        let fs = FakeFs::new();
        fs.write(Path::new("/target.md"), b"linked body").unwrap();
        fs.symlink(Path::new("/target.md"), Path::new("/link.md")).unwrap();
        // read() follows the symlink transparently.
        assert_eq!(
            fs.read_to_string(Path::new("/link.md")).unwrap(),
            "linked body"
        );
        // read_link returns the stored target, not the body.
        assert_eq!(
            fs.read_link(Path::new("/link.md")).unwrap(),
            PathBuf::from("/target.md")
        );
    }

    #[test]
    fn fake_fs_metadata_call_counter_instruments_loops() {
        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/d"),
            serde_json::json!({ "a": "1", "b": "2", "c": "3" }),
        );
        let before = fs.metadata_call_count();
        let _ = fs.metadata(Path::new("/d/a"));
        let _ = fs.metadata(Path::new("/d/b"));
        let _ = fs.metadata(Path::new("/d/c"));
        assert_eq!(fs.metadata_call_count() - before, 3);
    }

    #[test]
    fn fake_fs_copy_duplicates_content() {
        let fs = FakeFs::new();
        fs.write(Path::new("/src"), b"abc").unwrap();
        let n = fs.copy(Path::new("/src"), Path::new("/dst")).unwrap();
        assert_eq!(n, 3);
        assert_eq!(fs.read_to_string(Path::new("/dst")).unwrap(), "abc");
        // Source untouched.
        assert_eq!(fs.read_to_string(Path::new("/src")).unwrap(), "abc");
    }

    // ── RealFs ↔ FakeFs parity ───────────────────────────────────
    //
    // Each parity test runs the same script against both impls and
    // asserts the observable outcomes match. If FakeFs ever diverges
    // from POSIX, one of these will fail.

    fn run_parity<F>(script: F)
    where
        F: Fn(&dyn Fs, &Path) -> String,
    {
        let real_root = tmpdir();
        let real = RealFs::new();
        let real_out = script(&real, &real_root);

        let fake = FakeFs::new();
        let fake_root = PathBuf::from("/parity-root");
        let fake_out = script(fake.as_ref(), &fake_root);

        assert_eq!(
            real_out, fake_out,
            "RealFs and FakeFs disagreed:\n real={:?}\n fake={:?}",
            real_out, fake_out
        );

        let _ = std::fs::remove_dir_all(&real_root);
    }

    #[test]
    fn parity_write_then_read() {
        run_parity(|fs, root| {
            let p = root.join("file.txt");
            fs.write(&p, b"contents").unwrap();
            fs.read_to_string(&p).unwrap()
        });
    }

    #[test]
    fn parity_exists_is_false_for_missing() {
        run_parity(|fs, root| {
            let missing = root.join("nope.md");
            fs.exists(&missing).to_string()
        });
    }

    #[test]
    fn parity_rename_moves_and_overwrites() {
        run_parity(|fs, root| {
            let a = root.join("a.txt");
            let b = root.join("b.txt");
            fs.write(&a, b"first").unwrap();
            fs.write(&b, b"second").unwrap();
            fs.rename(&a, &b).unwrap();
            // a gone, b now holds "first"
            format!("a_exists={},b={}", fs.exists(&a), fs.read_to_string(&b).unwrap())
        });
    }

    #[test]
    fn parity_create_dir_all_then_read_dir() {
        run_parity(|fs, root| {
            fs.create_dir_all(&root.join("x/y/z")).unwrap();
            fs.write(&root.join("x/y/z/a.txt"), b"").unwrap();
            fs.write(&root.join("x/y/z/b.txt"), b"").unwrap();
            let mut entries = fs.read_dir(&root.join("x/y/z")).unwrap();
            entries.sort();
            entries
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(",")
        });
    }

    #[test]
    fn parity_remove_file_and_absence() {
        run_parity(|fs, root| {
            let p = root.join("goner.txt");
            fs.write(&p, b"bye").unwrap();
            fs.remove_file(&p).unwrap();
            fs.exists(&p).to_string()
        });
    }

    #[test]
    fn parity_copy_bytes_identical() {
        run_parity(|fs, root| {
            let src = root.join("src.md");
            let dst = root.join("dst.md");
            fs.write(&src, b"copied").unwrap();
            let n = fs.copy(&src, &dst).unwrap();
            format!("n={},dst={}", n, fs.read_to_string(&dst).unwrap())
        });
    }

    #[test]
    fn parity_symlink_and_read_through() {
        run_parity(|fs, root| {
            let target = root.join("actual.md");
            let link = root.join("link.md");
            fs.write(&target, b"linked").unwrap();
            fs.symlink(&target, &link).unwrap();
            fs.read_to_string(&link).unwrap()
        });
    }

    #[test]
    fn parity_metadata_is_file_vs_is_dir() {
        run_parity(|fs, root| {
            let f = root.join("f.txt");
            fs.create_dir_all(root).unwrap();
            fs.write(&f, b"x").unwrap();
            format!(
                "file_isf={},file_isd={},root_isf={},root_isd={}",
                fs.is_file(&f),
                fs.is_dir(&f),
                fs.is_file(root),
                fs.is_dir(root)
            )
        });
    }
}
