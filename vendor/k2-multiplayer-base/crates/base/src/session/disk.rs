//! Disk sync: the file I/O. The kit reads and writes the file itself but never watches it:
//! the host reports events ([`super::Session::file_changed`]) and drives a stat backup
//! ([`super::SessionManager::tick`]). The merge logic is in `live.rs`.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::Seq;

use super::pack::Declined;

/// Where the file stands with the doc. Part of [`super::Status`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiskState {
    /// The file holds the doc.
    Clean,
    /// The doc has changes not yet written; the first came at `since_ms`.
    Dirty { since_ms: u64 },
    /// The file is gone (deleted, replaced by a folder, or a symlink now pointing outside the
    /// root). Nothing is written; the doc and its edits are kept. The host decides the grace
    /// period and what happens after it.
    Missing,
    /// The file stopped being decodable (binary, not UTF-8, too large, mixed line endings).
    /// Nothing is written and edits are refused until it decodes again.
    Declined(String),
}

/// What a disk check found ([`super::Session::file_changed`], [`super::Session::check_disk`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiskCheck {
    /// Nothing changed (or the session has no file).
    Unchanged,
    /// The file holds one of the kit's own recent writes (an echo). Nothing was ingested.
    OwnWrite,
    /// An outside edit was merged in as [`super::EXTERNAL_PID`]; the doc is now at `version`.
    Ingested { version: Seq },
    /// The file is gone.
    Missing,
    /// The file can't be a live doc any more.
    Declined(Declined),
}

/// Returned by [`super::Session::flush`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlushOutcome {
    /// Whether the file was written (false when it already held the doc).
    pub written: bool,
    /// SHA-256 (lowercase hex) of the doc's bytes now: what the file holds when `disk` is
    /// [`DiskState::Clean`].
    pub hash: String,
    /// `None` for a session without a file.
    pub disk: Option<DiskState>,
}

/// What identifies one state of a file cheaply: a rename-save changes the inode, an
/// in-place save the size or mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStat {
    len: u64,
    mtime_ns: Option<u128>,
    ino: u64,
    dev: u64,
}

impl FileStat {
    pub(crate) fn of(m: &Metadata) -> Self {
        #[cfg(unix)]
        let (ino, dev) = {
            use std::os::unix::fs::MetadataExt;
            (m.ino(), m.dev())
        };
        #[cfg(not(unix))]
        let (ino, dev) = (0, 0);
        Self {
            len: m.len(),
            mtime_ns: m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
            ino,
            dev,
        }
    }
}

/// What is at a session's path now.
pub(crate) enum Probe {
    /// A regular file, at `real` (symlinks resolved) inside the root.
    File { real: PathBuf, stat: FileStat },
    /// Nothing there (or a dangling symlink).
    Missing,
    /// Something that isn't a regular file (a folder, a device, a pipe).
    NotFile,
    /// A symlink resolving outside the root.
    Outside(PathBuf),
}

/// Look at `path` without reading it. `root` must be canonical.
pub(crate) fn probe(path: &Path, root: &Path) -> io::Result<Probe> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Probe::Missing),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let real = match fs::canonicalize(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Probe::Missing),
        Err(e) => return Err(e),
        Ok(r) => r,
    };
    if !real.starts_with(root) {
        return Ok(Probe::Outside(real));
    }
    let m = fs::metadata(&real)?;
    if !m.is_file() {
        return Ok(Probe::NotFile);
    }
    Ok(Probe::File {
        stat: FileStat::of(&m),
        real,
    })
}

/// A file's bytes, with the stat taken from the open handle (so it matches what was read
/// unless the file was written in place during the read, which the hash then catches).
pub(crate) enum ReadFile {
    Bytes(Vec<u8>, FileStat),
    TooLarge(u64),
}

pub(crate) fn read(real: &Path, max: u64) -> io::Result<ReadFile> {
    let mut f = File::open(real)?;
    let stat = FileStat::of(&f.metadata()?);
    if stat.len > max {
        return Ok(ReadFile::TooLarge(stat.len));
    }
    let mut buf = Vec::with_capacity(stat.len as usize);
    Read::by_ref(&mut f).take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Ok(ReadFile::TooLarge(buf.len() as u64));
    }
    Ok(ReadFile::Bytes(buf, stat))
}

const TEMP_MARK: &str = ".~mp";

/// Whether `file_name` is one of the kit's write-back temp files
/// (`.<name>.~mp<pid>-<n>.tmp`, beside the file). A host's watcher should ignore them.
pub fn is_write_temp(file_name: &str) -> bool {
    file_name.starts_with('.') && file_name.contains(TEMP_MARK) && file_name.ends_with(".tmp")
}

fn temp_prefix(name: &str) -> String {
    format!(".{name}{TEMP_MARK}")
}

/// Remove write-back temp files a crash left beside `real`. Best effort.
pub(crate) fn clean_temps(real: &Path) {
    let (Some(dir), Some(name)) = (real.parent(), real.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let prefix = temp_prefix(name);
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if let Some(n) = e.file_name().to_str() {
            if n.starts_with(&prefix) && n.ends_with(".tmp") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Write `bytes` to `real` atomically: a temp file in the same folder, the old file's
/// permissions, fsync, rename over, fsync the folder. Readers see the old file or the new
/// one, never a partial file. Returns the new file's stat, and an error from the final
/// folder fsync, if any (the file is in place by then, so that is reported, not failed).
pub(crate) fn atomic_write(real: &Path, bytes: &[u8]) -> io::Result<(FileStat, Option<io::Error>)> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = real
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no folder"))?;
    let name = real
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name is not UTF-8"))?;
    let perms = fs::metadata(real).ok().map(|m| m.permissions());
    let (tmp, mut f) = loop {
        let tmp = dir.join(format!(
            "{}{}-{}.tmp",
            temp_prefix(name),
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let written = (|| {
        f.write_all(bytes)?;
        if let Some(p) = perms {
            f.set_permissions(p)?;
        }
        f.sync_all()?;
        let stat = FileStat::of(&f.metadata()?);
        drop(f);
        fs::rename(&tmp, real)?;
        Ok(stat)
    })();
    match written {
        Ok(stat) => {
            let dir_sync = File::open(dir).and_then(|d| d.sync_all()).err();
            Ok((stat, dir_sync))
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_recognisable() {
        assert!(is_write_temp(".notes.md.~mp123-4.tmp"));
        assert!(!is_write_temp("notes.md"));
        assert!(!is_write_temp(".notes.md.swp"));
        assert!(!is_write_temp("x.~mp1-1.tmp"), "always a dotfile");
    }
}
