//! Spool files (prd-compute-update-continuity-v1 §5.1): a job's stdout and
//! stderr are regular files the node owns, not pipes to the node (except a
//! secret-env job's, which keeps its pipes and is never adopted). A pipe
//! dies with the k2-node process that reads it (the job's next write gets
//! EPIPE); a file doesn't, so a job keeps writing while one k2-node
//! process hands it to the next.
//!
//! - The node creates each file (`O_CREAT|O_EXCL|O_NOFOLLOW`, 0600) and
//!   keeps a read-write fd; the job gets a second, write-only `O_APPEND`
//!   fd to the same inode (checked). Root never opens a spool path.
//! - The tailer reads what's new every ≤ 250 ms, sends it to the journal,
//!   and records how far it got. A job can truncate its file or jump far
//!   ahead (it holds a write fd): a shrink is noted and reading goes on
//!   from the new end; a jump past [`JUMP_SKIP`] is skipped with a note,
//!   never read byte by byte.
//! - Disk: what was read is given back with a hole punch (Linux
//!   `FALLOC_FL_PUNCH_HOLE`, macOS `F_PUNCHHOLE`), so a flooding job costs
//!   at most what nobody read yet. Where punching isn't supported the file
//!   just grows until the job ends (noted once in `k2-node status`).
//! - A reopen (the next process) uses `O_NOFOLLOW` and must find the same
//!   (device, inode) the run facts recorded.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How often the tailer looks for new bytes.
pub const POLL_MS: u64 = 200;
/// A jump this far past what was read is skipped, not read.
pub const JUMP_SKIP: u64 = 1 << 30;
/// Punch once this much more was read (fewer syscalls).
pub const PUNCH_EVERY: u64 = 1 << 20;
/// Punch on this boundary (APFS and ext4 blocks).
const PUNCH_ALIGN: u64 = 4096;

/// One spool file as the run facts record it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpoolFile {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    /// Bytes already in the journal (read and flushed; a partial line the
    /// tailer held isn't counted, so the next reader reads it again).
    #[serde(default)]
    pub off: u64,
}

/// A freshly made spool file: the node's read-write fd and the job's
/// write-only fd.
pub struct Made {
    pub reader: File,
    pub writer: OwnedFd,
    pub facts: SpoolFile,
}

fn cpath(p: &Path) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).map_err(|_| format!("{} has a NUL", p.display()))
}

fn open_raw(p: &Path, flags: i32, mode: u32) -> Result<OwnedFd, String> {
    let c = cpath(p)?;
    // SAFETY: valid C path; the fd is owned right below.
    let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_CLOEXEC | libc::O_NOFOLLOW, mode as libc::c_uint) };
    if fd < 0 {
        return Err(format!("open {}: {}", p.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh fd we own.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Make `<dir>/<key>.<ext>` (the dir 0700, made if missing) and both fds.
pub fn create(dir: &Path, key: &str, ext: &str) -> Result<Made, String> {
    crate::util::mkdir_mode(dir, 0o700)?;
    let path = dir.join(format!("{key}.{ext}"));
    // A leftover from an attempt that never started (same key) goes first;
    // it isn't followed if it's a link.
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    let rw = open_raw(&path, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, 0o600)?;
    let w = open_raw(&path, libc::O_WRONLY | libc::O_APPEND, 0)?;
    let reader = File::from(rw);
    let (m1, m2) = (reader.metadata().map_err(|e| e.to_string())?, File::from(w.try_clone().map_err(|e| e.to_string())?).metadata().map_err(|e| e.to_string())?);
    if (m1.dev(), m1.ino()) != (m2.dev(), m2.ino()) || !m1.file_type().is_file() {
        return Err(format!("{} changed while it was opened", path.display()));
    }
    Ok(Made { facts: SpoolFile { path, dev: m1.dev(), ino: m1.ino(), off: 0 }, reader, writer: w })
}

/// Open a spool file again (the next k2-node process): no link, a regular
/// file, the same (device, inode).
pub fn reopen(f: &SpoolFile) -> Result<File, String> {
    let fd = open_raw(&f.path, libc::O_RDWR | libc::O_NONBLOCK, 0)?;
    let file = File::from(fd);
    let m = file.metadata().map_err(|e| format!("stat {}: {e}", f.path.display()))?;
    if !m.file_type().is_file() || m.dev() != f.dev || m.ino() != f.ino {
        return Err(format!("{} isn't the job's output file any more", f.path.display()));
    }
    Ok(file)
}

/// Give `[from, to)` of `f` back to the filesystem (aligned inward). `Ok`
/// with nothing to do when the range is empty after aligning.
pub fn punch(f: &File, from: u64, to: u64) -> std::io::Result<()> {
    let start = from.div_ceil(PUNCH_ALIGN) * PUNCH_ALIGN;
    let end = to / PUNCH_ALIGN * PUNCH_ALIGN;
    if end <= start {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: fallocate on our own fd with a valid range.
        let r = unsafe { libc::fallocate(f.as_raw_fd(), libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE, start as libc::off_t, (end - start) as libc::off_t) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let arg = libc::fpunchhole_t { fp_flags: 0, reserved: 0, fp_offset: start as libc::off_t, fp_length: (end - start) as libc::off_t };
        // SAFETY: F_PUNCHHOLE on our own fd with a filled-in struct.
        let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_PUNCHHOLE, &arg) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = f;
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

/// What one look at a spool file found.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    Data(Vec<u8>),
    /// The file is shorter than what was read (the job truncated it):
    /// reading goes on from `to`.
    Shrunk { from: u64, to: u64 },
    /// The file jumped past [`JUMP_SKIP`] (a sparse extend): skipped to `to`.
    Jumped { from: u64, to: u64 },
    Nothing,
}

/// Reads one spool file from a position.
pub struct Tail {
    file: File,
    /// Bytes read so far (some may still be held as a partial line).
    pub read: u64,
    /// Up to here the file was punched.
    punched: u64,
    /// Punching failed once: never tried again.
    pub punch_failed: Option<String>,
}

impl Tail {
    pub fn new(file: File, from: u64) -> Self {
        Self { file, read: from, punched: from / PUNCH_ALIGN * PUNCH_ALIGN, punch_failed: None }
    }

    /// Up to `max` new bytes.
    pub fn poll(&mut self, max: usize) -> std::io::Result<Seen> {
        let size = self.file.metadata()?.len();
        if size < self.read {
            let from = self.read;
            self.read = size;
            self.punched = self.punched.min(size);
            return Ok(Seen::Shrunk { from, to: size });
        }
        if size - self.read > JUMP_SKIP {
            let from = self.read;
            self.read = size;
            return Ok(Seen::Jumped { from, to: size });
        }
        if size == self.read {
            return Ok(Seen::Nothing);
        }
        let n = ((size - self.read) as usize).min(max);
        let mut buf = vec![0u8; n];
        self.file.seek(SeekFrom::Start(self.read))?;
        let mut got = 0;
        while got < n {
            match self.file.read(&mut buf[got..])? {
                0 => break,
                k => got += k,
            }
        }
        buf.truncate(got);
        self.read += got as u64;
        Ok(if got == 0 { Seen::Nothing } else { Seen::Data(buf) })
    }

    /// Give back what's in the journal (`upto`), once [`PUNCH_EVERY`] more
    /// was read. Returns the error the first time punching fails.
    pub fn give_back(&mut self, upto: u64) -> Option<String> {
        if self.punch_failed.is_some() || upto < self.punched + PUNCH_EVERY {
            return None;
        }
        match punch(&self.file, self.punched, upto) {
            Ok(()) => {
                self.punched = upto / PUNCH_ALIGN * PUNCH_ALIGN;
                None
            }
            Err(e) => {
                let m = e.to_string();
                self.punch_failed = Some(m.clone());
                Some(m)
            }
        }
    }

    /// Allocated bytes of the file (tests: punching keeps disk bounded).
    pub fn allocated(&self) -> u64 {
        self.file.metadata().map(|m| m.blocks() * 512).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn writer_of(m: &Made) -> File {
        File::from(m.writer.try_clone().unwrap())
    }

    #[test]
    fn the_job_writes_through_its_own_fd_and_the_tail_reads_it_in_order() {
        let d = crate::util::temp_dir("spool-basic");
        let m = create(&d.join("spool"), "j-g1", "out").unwrap();
        let mode = std::fs::metadata(&m.facts.path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600, "node-only");
        let mut w = writer_of(&m);
        let mut t = Tail::new(m.reader, 0);
        assert_eq!(t.poll(1024).unwrap(), Seen::Nothing);
        w.write_all(b"one\ntwo").unwrap();
        assert_eq!(t.poll(4).unwrap(), Seen::Data(b"one\n".to_vec()));
        assert_eq!(t.poll(1024).unwrap(), Seen::Data(b"two".to_vec()));
        assert_eq!(t.read, 7);
    }

    #[test]
    fn a_reopen_resumes_at_the_recorded_offset_and_refuses_another_file() {
        let d = crate::util::temp_dir("spool-reopen");
        let m = create(&d.join("spool"), "j-g1", "err").unwrap();
        let mut w = writer_of(&m);
        w.write_all(b"before\nafter\n").unwrap();
        let mut facts = m.facts.clone();
        facts.off = 7;
        drop(m);
        let mut t = Tail::new(reopen(&facts).unwrap(), facts.off);
        assert_eq!(t.poll(1024).unwrap(), Seen::Data(b"after\n".to_vec()));
        // The job (or anyone) swaps the file: the reopen refuses it.
        std::fs::remove_file(&facts.path).unwrap();
        std::fs::write(&facts.path, b"forged\n").unwrap();
        let e = reopen(&facts).map(|_| ()).unwrap_err();
        assert!(e.contains("isn't the job's output file"), "{e}");
        // A link is never followed.
        std::fs::remove_file(&facts.path).unwrap();
        std::os::unix::fs::symlink(d.join("elsewhere"), &facts.path).unwrap();
        assert!(reopen(&facts).is_err());
    }

    #[test]
    fn a_create_never_follows_a_planted_link() {
        let d = crate::util::temp_dir("spool-link");
        let dir = d.join("spool");
        std::fs::create_dir_all(&dir).unwrap();
        let target = d.join("victim");
        std::fs::write(&target, b"keep me").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("j-g1.out")).unwrap();
        let m = create(&dir, "j-g1", "out").unwrap();
        assert!(std::fs::symlink_metadata(&m.facts.path).unwrap().file_type().is_file(), "a fresh file, not the link");
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    }

    /// C8: the job holds a write fd: it can truncate its file or jump far
    /// ahead. A shrink is reported and reading goes on from the new end;
    /// a jump past the limit is skipped without reading it.
    #[test]
    fn truncate_and_sparse_jumps_by_the_job() {
        let d = crate::util::temp_dir("spool-trunc");
        let m = create(&d.join("spool"), "j-g1", "out").unwrap();
        let mut w = writer_of(&m);
        let mut t = Tail::new(m.reader, 0);
        w.write_all(b"0123456789").unwrap();
        assert_eq!(t.poll(1 << 20).unwrap(), Seen::Data(b"0123456789".to_vec()));
        w.set_len(0).unwrap();
        assert_eq!(t.poll(1 << 20).unwrap(), Seen::Shrunk { from: 10, to: 0 });
        w.write_all(b"new\n").unwrap();
        assert_eq!(t.poll(1 << 20).unwrap(), Seen::Data(b"new\n".to_vec()));
        w.set_len(4 + JUMP_SKIP + 1).unwrap();
        assert_eq!(t.poll(1 << 20).unwrap(), Seen::Jumped { from: 4, to: 4 + JUMP_SKIP + 1 });
        w.write_all(b"tail\n").unwrap();
        assert_eq!(t.poll(1 << 20).unwrap(), Seen::Data(b"tail\n".to_vec()));
    }

    /// Q8: reading gives the space back while the job keeps appending
    /// (where the filesystem can punch holes; elsewhere the error is said
    /// once and nothing else changes).
    #[test]
    fn what_was_read_is_given_back_while_the_job_appends() {
        let d = crate::util::temp_dir("spool-punch");
        let m = create(&d.join("spool"), "j-g1", "out").unwrap();
        let mut w = writer_of(&m);
        let mut t = Tail::new(m.reader, 0);
        let chunk = vec![b'x'; 1 << 20];
        for _ in 0..16 {
            w.write_all(&chunk).unwrap();
            while let Seen::Data(_) = t.poll(1 << 20).unwrap() {}
            let _ = t.give_back(t.read);
        }
        assert_eq!(t.read, 16 << 20);
        // The node homes are ext4/xfs (Linux) and APFS (macOS); the test
        // dirs too. Both punch: a failure here is a real failure.
        assert_eq!(t.punch_failed, None, "hole punching failed on this filesystem");
        // What was read and given back reads as zeros now (a hole).
        let mut head = vec![1u8; 4096];
        {
            use std::os::unix::fs::FileExt;
            File::open(&m.facts.path).unwrap().read_exact_at(&mut head, 0).unwrap();
        }
        assert!(head.iter().all(|b| *b == 0), "the given-back range is a hole");
        if cfg!(target_os = "linux") {
            assert!(t.allocated() < 4 << 20, "16 MiB written and read, {} bytes still allocated", t.allocated());
        }
        // APFS: the U0 spike on a mini kept a 64 MB flood at ≤ 7 MB
        // allocated, but freshly written blocks punched at once may stay
        // counted for a while; the live drill checks the disk on a mini.
        assert_eq!(std::fs::metadata(&m.facts.path).unwrap().len(), 16 << 20, "the size stays: the job's O_APPEND writes go on at the end");
    }
}
