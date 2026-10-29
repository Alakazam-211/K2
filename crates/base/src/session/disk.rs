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
    /// Tier 0: another program saved the file while the lease was held, or while the doc
    /// had a version the file didn't have yet. The file's content is now version `version`;
    /// what was current before is version `kept` and was written to the conflict copy `copy`
    /// (beside the file; `None` if that failed). Nothing is lost.
    Conflict {
        version: Seq,
        kept: Seq,
        copy: Option<PathBuf>,
    },
    /// Tier 0.5: such a save was merged by the pack; the merge is version `version` (and is
    /// written back to the file).
    Merged { version: Seq },
}

/// Returned by [`super::Session::flush`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlushOutcome {
    /// Whether the file was written (false when it already held the doc).
    pub written: bool,
    /// HMAC-SHA256 (lowercase hex) of the doc's bytes now, under the doc's own key: what the
    /// file holds when `disk` is [`DiskState::Clean`]. Compare it only with this doc's
    /// hashes; a purge replaces the key. For a tier-0 blob doc, the current blob's id (its
    /// keyed hash).
    pub hash: String,
    /// `None` for a session without a file.
    pub disk: Option<DiskState>,
}

/// The coarsest modification-time granularity the kit allows for: FAT and exFAT keep 2 s,
/// HFS+ 1 s, network shares whatever their server keeps (ext4 and XFS a kernel tick, APFS
/// ns). Two writes inside it can leave the same mtime.
pub(crate) const RACY_NS: u128 = 2_000_000_000;

/// The wall clock in ns since the Unix epoch: what a file's mtime is measured on (never the
/// host's clock).
pub(crate) fn wall_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// A file as last seen: its stat, and whether that stat can vouch for its content (git's
/// "racy clean" rule). A stat taken within [`RACY_NS`] of its own mtime (or with an mtime
/// in the future, or none) is **racy**: a same-size write in place within the same mtime
/// tick would leave it equal. An equal racy stat proves nothing, so the file is read and
/// hashed; the mark clears when it is seen again after mtime + [`RACY_NS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seen {
    pub stat: FileStat,
    pub racy: bool,
}

impl Seen {
    /// `stat`, just taken.
    pub(crate) fn now(stat: FileStat) -> Self {
        Self::at(stat, wall_ns())
    }

    pub(crate) fn at(stat: FileStat, wall_ns: u128) -> Self {
        let racy = stat
            .mtime_ns
            .map_or(true, |m| wall_ns < m.saturating_add(RACY_NS));
        Seen { stat, racy }
    }

    /// Whether `now` (a stat just taken) is surely the same content: equal, and not racy.
    pub(crate) fn vouches_for(&self, now: FileStat) -> bool {
        !self.racy && self.stat == now
    }

    /// What to keep in the store: only a stat that can vouch for the content. A racy one
    /// is not kept (`None`: the next open hashes the file), so a reopen honours the rule
    /// with no change to the record's format.
    pub(crate) fn persisted(&self) -> Option<Vec<u8>> {
        (!self.racy).then(|| self.stat.to_bytes())
    }

    /// A stat read back from the store: the kit keeps only non-racy ones; one kept by an
    /// older kit is racy when its mtime is still within [`RACY_NS`] of now.
    pub(crate) fn loaded(stat: FileStat) -> Self {
        Self::now(stat)
    }
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
    /// A stable encoding, to keep in the store (tier 0 skips re-hashing an unchanged file).
    pub(crate) fn to_bytes(self) -> Vec<u8> {
        let mut b = Vec::with_capacity(41);
        b.extend_from_slice(&self.len.to_le_bytes());
        match self.mtime_ns {
            None => b.push(0),
            Some(t) => {
                b.push(1);
                b.extend_from_slice(&t.to_le_bytes());
            }
        }
        b.extend_from_slice(&self.ino.to_le_bytes());
        b.extend_from_slice(&self.dev.to_le_bytes());
        b
    }

    pub(crate) fn from_bytes(b: &[u8]) -> Option<Self> {
        let u64_at = |i: usize| -> Option<u64> {
            Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
        };
        let len = u64_at(0)?;
        let (mtime_ns, at) = match *b.get(8)? {
            0 => (None, 9),
            1 => (
                Some(u128::from_le_bytes(b.get(9..25)?.try_into().ok()?)),
                25,
            ),
            _ => return None,
        };
        let s = Self {
            len,
            mtime_ns,
            ino: u64_at(at)?,
            dev: u64_at(at + 8)?,
        };
        (b.len() == at + 16).then_some(s)
    }

    /// The modification time in ms since the Unix epoch.
    pub(crate) fn mtime_ms(&self) -> Option<u64> {
        self.mtime_ns.map(|n| (n / 1_000_000) as u64)
    }

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
    atomic_write_with(real, |f| f.write_all(bytes))
}

/// [`atomic_write`], with the content written by `fill` (streamed: a tier-0 blob is copied
/// from the blob store, verified, without being held in memory). An error from `fill`
/// removes the temp file and leaves `real` untouched.
pub(crate) fn atomic_write_with(
    real: &Path,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<(FileStat, Option<io::Error>)> {
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
        fill(&mut f)?;
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

    fn stat_at(mtime_ns: Option<u128>) -> FileStat {
        FileStat {
            len: 7,
            mtime_ns,
            ino: 1,
            dev: 2,
        }
    }

    /// Git's racy rule: a stat taken within RACY_NS of its mtime (or ahead of it, or with
    /// none) can't vouch for the content; one taken after mtime + RACY_NS can.
    #[test]
    fn a_stat_is_racy_until_its_mtime_is_older_than_the_granularity() {
        let m = 1_700_000_000_000_000_000u128;
        let s = stat_at(Some(m));
        assert!(Seen::at(s, m).racy, "taken in its own tick");
        assert!(Seen::at(s, m + RACY_NS - 1).racy);
        assert!(!Seen::at(s, m + RACY_NS).racy, "clears after mtime + G");
        assert!(Seen::at(s, m - 5).racy, "an mtime in the future");
        assert!(Seen::at(stat_at(None), m).racy, "no mtime");
        // an equal stat vouches only when not racy
        assert!(!Seen::at(s, m).vouches_for(s));
        assert!(Seen::at(s, m + RACY_NS).vouches_for(s));
        assert!(!Seen::at(s, m + RACY_NS).vouches_for(stat_at(Some(m + 1))));
        // only a stat that vouches is kept in the store
        assert_eq!(Seen::at(s, m).persisted(), None);
        assert_eq!(Seen::at(s, m + RACY_NS).persisted(), Some(s.to_bytes()));
    }

    #[test]
    fn file_stat_round_trips() {
        for mtime_ns in [None, Some(1_700_000_000_123_456_789u128)] {
            let s = FileStat {
                len: 42,
                mtime_ns,
                ino: 7,
                dev: 9,
            };
            assert_eq!(FileStat::from_bytes(&s.to_bytes()), Some(s));
        }
        assert_eq!(FileStat::from_bytes(&[1, 2, 3]), None);
    }

    #[test]
    fn temp_names_are_recognisable() {
        assert!(is_write_temp(".notes.md.~mp123-4.tmp"));
        assert!(!is_write_temp("notes.md"));
        assert!(!is_write_temp(".notes.md.swp"));
        assert!(!is_write_temp("x.~mp1-1.tmp"), "always a dotfile");
    }
}
