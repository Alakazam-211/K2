//! One transcript follower for every reader of a harness session log
//! (prd-daemon-activity-and-thread-working-v1 DA27, A18).
//!
//! The Chat view (`chat_overlay_ws`, [`FollowStart::FromStart`]) and the
//! activity store's per-session follower ([`FollowStart::FromEnd`]) both
//! read through this type. It never locates a file: the caller resolves
//! the path once (and again only on a miss, with backoff) and hands it in.
//!
//! What it guarantees, poll to poll:
//! - a byte offset plus a held partial line, so a line split across two
//!   writes is delivered once, whole;
//! - a rewrite is detected by a new inode, a size below the offset, or a
//!   mismatch in the [`FINGERPRINT_BYTES`] before the offset (a same-size
//!   rewrite in place). A rewrite sets [`FollowPoll::reset`]: `FromStart`
//!   re-reads from byte 0, `FromEnd` and `CatchUp` re-seek to the end (an
//!   activity reader must not replay old turn ends as new evidence);
//! - a record longer than [`MAX_RECORD_BYTES`] is skipped, never buffered;
//! - `FromEnd` attached in the middle of a record skips that record's tail.
//!
//! All I/O is plain blocking `std::fs`; async callers run [`TranscriptFollow::poll`]
//! in `spawn_blocking` or on their own thread.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Records over this size are skipped (DA27).
pub const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;

/// Bytes before the offset compared on every poll (same-size rewrite).
pub const FINGERPRINT_BYTES: u64 = 64;

const READ_CHUNK: usize = 256 * 1024;

/// Where a freshly attached follower starts reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowStart {
    /// Every record in the file (the Chat view).
    FromStart,
    /// Only records appended after attach (activity evidence on a file
    /// that predates the session: a resumed conversation's history).
    FromEnd,
    /// Every record on the first attach, then like `FromEnd` after a
    /// rewrite (activity evidence on a file born after the session
    /// spawned: all of it is this session's).
    CatchUp,
}

/// What one poll found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FollowPoll {
    /// The follower (re)positioned: first attach or a detected rewrite.
    /// A reader drops whatever it built from earlier lines.
    pub reset: bool,
    /// Complete lines, without the trailing newline.
    pub lines: Vec<String>,
    /// Records skipped for being longer than [`MAX_RECORD_BYTES`].
    pub skipped: usize,
    /// The file could not be opened (deleted or moved). The caller
    /// re-resolves; the follower keeps nothing.
    pub missing: bool,
}

/// A follower over one transcript file.
#[derive(Debug)]
pub struct TranscriptFollow {
    start: FollowStart,
    path: PathBuf,
    attached: bool,
    offset: u64,
    file_id: Option<(u64, u64)>,
    /// The bytes `[offset - FINGERPRINT_BYTES, offset)`.
    fingerprint: Vec<u8>,
    pending: Vec<u8>,
    /// Inside an over-long record (or a record cut by a `FromEnd` attach):
    /// drop bytes until the next newline.
    skipping: bool,
}

impl TranscriptFollow {
    /// A follower for `path`. Nothing is read until the first [`poll`](Self::poll).
    pub fn new(path: PathBuf, start: FollowStart) -> Self {
        Self {
            start,
            path,
            attached: false,
            offset: 0,
            file_id: None,
            fingerprint: Vec::new(),
            pending: Vec::new(),
            skipping: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The byte offset read so far.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Read whatever was appended since the last poll.
    pub fn poll(&mut self) -> FollowPoll {
        let mut out = FollowPoll::default();
        let Ok(mut file) = File::open(&self.path) else {
            self.detach();
            out.missing = true;
            return out;
        };
        let Ok(meta) = file.metadata() else {
            self.detach();
            out.missing = true;
            return out;
        };
        let len = meta.len();
        let id = file_id(&meta);
        if !self.attached {
            self.attached = true;
            out.reset = true;
            let from_start = self.start != FollowStart::FromEnd;
            self.position(&mut file, len, id, from_start);
            if !from_start {
                return out;
            }
        } else {
            let replaced = id.is_some() && self.file_id.is_some() && id != self.file_id;
            let shrunk = len < self.offset;
            let rewritten = replaced || shrunk || !self.fingerprint_holds(&mut file);
            if rewritten {
                out.reset = true;
                let from_start = self.start == FollowStart::FromStart;
                self.position(&mut file, len, id, from_start);
                if !from_start {
                    return out;
                }
            }
        }
        if len > self.offset {
            self.read_to(&mut file, len, &mut out);
        }
        out
    }

    /// Forget the position; the next poll attaches afresh.
    fn detach(&mut self) {
        self.attached = false;
        self.offset = 0;
        self.file_id = None;
        self.fingerprint.clear();
        self.pending.clear();
        self.skipping = false;
    }

    /// Start (or restart) at byte 0 or at the end.
    fn position(&mut self, file: &mut File, len: u64, id: Option<(u64, u64)>, from_start: bool) {
        self.file_id = id;
        self.pending.clear();
        self.skipping = false;
        if from_start {
            self.offset = 0;
            self.fingerprint.clear();
        } else {
            self.offset = len;
            self.fingerprint = read_range(file, len.saturating_sub(FINGERPRINT_BYTES), len);
            // Attached mid-record: its tail is not a whole record.
            self.skipping = self.fingerprint.last().is_some_and(|b| *b != b'\n');
        }
    }

    fn fingerprint_holds(&self, file: &mut File) -> bool {
        if self.offset == 0 {
            return true;
        }
        let from = self.offset.saturating_sub(FINGERPRINT_BYTES);
        read_range(file, from, self.offset) == self.fingerprint
    }

    fn read_to(&mut self, file: &mut File, len: u64, out: &mut FollowPoll) {
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut remaining = len - self.offset;
        let mut buf = vec![0u8; READ_CHUNK.min(remaining as usize).max(1)];
        while remaining > 0 {
            let want = (remaining as usize).min(buf.len());
            let n = match file.read(&mut buf[..want]) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            remaining -= n as u64;
            self.offset += n as u64;
            self.note_fingerprint(&buf[..n]);
            self.split(&buf[..n], out);
        }
    }

    fn note_fingerprint(&mut self, bytes: &[u8]) {
        self.fingerprint.extend_from_slice(bytes);
        let keep = FINGERPRINT_BYTES as usize;
        if self.fingerprint.len() > keep {
            let cut = self.fingerprint.len() - keep;
            self.fingerprint.drain(..cut);
        }
    }

    fn split(&mut self, mut bytes: &[u8], out: &mut FollowPoll) {
        while !bytes.is_empty() {
            let nl = bytes.iter().position(|b| *b == b'\n');
            let (part, rest, ends) = match nl {
                Some(i) => (&bytes[..i], &bytes[i + 1..], true),
                None => (bytes, &bytes[bytes.len()..], false),
            };
            if !self.skipping {
                if self.pending.len() + part.len() > MAX_RECORD_BYTES {
                    self.pending.clear();
                    self.skipping = true;
                    out.skipped += 1;
                } else {
                    self.pending.extend_from_slice(part);
                }
            }
            if ends {
                if self.skipping {
                    self.skipping = false;
                } else {
                    let mut line = String::from_utf8_lossy(&self.pending).into_owned();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                    out.lines.push(line);
                }
                self.pending.clear();
            }
            bytes = rest;
        }
    }
}

fn read_range(file: &mut File, from: u64, to: u64) -> Vec<u8> {
    if to <= from || file.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut buf = vec![0u8; (to - from) as usize];
    match file.read_exact(&mut buf) {
        Ok(()) => buf,
        Err(_) => Vec::new(),
    }
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    // No inode: the size and fingerprint checks still catch a replace.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-follow-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        dir.join("t.jsonl")
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("open");
        f.write_all(bytes).expect("append");
    }

    fn lines(poll: &FollowPoll) -> Vec<&str> {
        poll.lines.iter().map(String::as_str).collect()
    }

    /// T-S3a: append, partial line, truncate, atomic replace (new inode),
    /// same-size rewrite (fingerprint), each detected, nothing read twice
    /// and nothing missed.
    #[test]
    fn follower_survives_every_kind_of_write() {
        let path = tmp("kinds");
        std::fs::write(&path, b"{\"n\":1}\n{\"n\":2}\n").expect("seed");
        let mut f = TranscriptFollow::new(path.clone(), FollowStart::FromStart);
        let p = f.poll();
        assert!(p.reset, "first attach resets");
        assert_eq!(lines(&p), vec![r#"{"n":1}"#, r#"{"n":2}"#]);
        assert_eq!(f.poll(), FollowPoll::default(), "no new bytes, nothing read twice");

        // Append.
        append(&path, b"{\"n\":3}\n");
        assert_eq!(lines(&f.poll()), vec![r#"{"n":3}"#]);

        // Partial line: held until its newline arrives, then whole.
        append(&path, b"{\"n\":");
        let p = f.poll();
        assert!(p.lines.is_empty() && !p.reset);
        append(&path, b"4}\r\n");
        assert_eq!(lines(&f.poll()), vec![r#"{"n":4}"#]);

        // Truncate + new content: reset, then the new file from byte 0.
        std::fs::OpenOptions::new().write(true).truncate(true).open(&path).expect("truncate");
        append(&path, b"{\"t\":1}\n");
        let p = f.poll();
        assert!(p.reset, "a shrink is a rewrite");
        assert_eq!(lines(&p), vec![r#"{"t":1}"#]);

        // Atomic replace (write a sibling, rename over): new inode.
        let side = path.with_extension("tmp");
        std::fs::write(&side, b"{\"r\":1}\n{\"r\":2}\n{\"r\":3}\n").expect("side");
        std::fs::rename(&side, &path).expect("rename");
        let p = f.poll();
        assert!(p.reset, "a new inode is a rewrite");
        assert_eq!(lines(&p), vec![r#"{"r":1}"#, r#"{"r":2}"#, r#"{"r":3}"#]);

        // Same-size rewrite in place (same inode, same length).
        {
            let mut w = std::fs::OpenOptions::new().write(true).open(&path).expect("rw");
            w.seek(SeekFrom::Start(0)).expect("seek");
            w.write_all(b"{\"s\":1}\n{\"s\":2}\n{\"s\":3}\n").expect("rewrite");
        }
        let p = f.poll();
        assert!(p.reset, "a fingerprint mismatch is a rewrite");
        assert_eq!(lines(&p), vec![r#"{"s":1}"#, r#"{"s":2}"#, r#"{"s":3}"#]);
        assert_eq!(f.poll(), FollowPoll::default());
    }

    /// T-S3a: a 3 MiB record is skipped, the records around it are not.
    #[test]
    fn oversized_records_are_skipped_not_buffered() {
        let path = tmp("big");
        let mut big = vec![b'x'; 3 * 1024 * 1024];
        big.push(b'\n');
        append(&path, b"{\"before\":1}\n");
        append(&path, &big);
        append(&path, b"{\"after\":1}\n");
        let mut f = TranscriptFollow::new(path, FollowStart::FromStart);
        let p = f.poll();
        assert_eq!(lines(&p), vec![r#"{"before":1}"#, r#"{"after":1}"#]);
        assert_eq!(p.skipped, 1);
    }

    /// T-S3a: random write boundaries still deliver every line exactly once.
    #[test]
    fn split_writes_deliver_each_line_once() {
        let path = tmp("split");
        let want: Vec<String> = (0..200).map(|i| format!("{{\"line\":{i},\"pad\":\"{}\"}}", "z".repeat(i % 37))).collect();
        let all = want.iter().map(|l| format!("{l}\n")).collect::<String>().into_bytes();
        let mut f = TranscriptFollow::new(path.clone(), FollowStart::FromStart);
        let mut got = Vec::new();
        let mut at = 0usize;
        let mut step = 1usize;
        append(&path, b"");
        while at < all.len() {
            let end = (at + step).min(all.len());
            append(&path, &all[at..end]);
            got.extend(f.poll().lines);
            at = end;
            step = step * 7 % 113 + 1;
        }
        got.extend(f.poll().lines);
        assert_eq!(got, want);
    }

    /// `FromEnd` reads only what is appended after attach, and skips the
    /// tail of a record it attached in the middle of.
    #[test]
    fn from_end_skips_history_and_a_cut_record() {
        let path = tmp("end");
        append(&path, b"{\"old\":1}\n{\"old\":2}\n{\"half\":");
        let mut f = TranscriptFollow::new(path.clone(), FollowStart::FromEnd);
        let p = f.poll();
        assert!(p.reset && p.lines.is_empty());
        append(&path, b"1}\n{\"new\":1}\n");
        assert_eq!(lines(&f.poll()), vec![r#"{"new":1}"#]);
        // A rewrite re-seeks to the end: old records are never replayed.
        std::fs::write(&path, b"{\"x\":1}\n").expect("rewrite");
        let p = f.poll();
        assert!(p.reset && p.lines.is_empty());
        append(&path, b"{\"x\":2}\n");
        assert_eq!(lines(&f.poll()), vec![r#"{"x":2}"#]);
    }

    /// `CatchUp` reads the whole file once, then never replays it.
    #[test]
    fn catch_up_reads_once_then_follows_like_from_end() {
        let path = tmp("catch");
        append(&path, b"{\"born\":1}\n{\"born\":2}\n");
        let mut f = TranscriptFollow::new(path.clone(), FollowStart::CatchUp);
        let p = f.poll();
        assert!(p.reset);
        assert_eq!(lines(&p), vec![r#"{"born":1}"#, r#"{"born":2}"#]);
        append(&path, b"{\"born\":3}\n");
        assert_eq!(lines(&f.poll()), vec![r#"{"born":3}"#]);
        std::fs::write(&path, b"{\"re\":1}\n").expect("rewrite");
        let p = f.poll();
        assert!(p.reset && p.lines.is_empty(), "a rewrite never replays");
    }

    /// A deleted file reports `missing`; recreated, it attaches afresh.
    #[test]
    fn a_missing_file_detaches() {
        let path = tmp("gone");
        append(&path, b"{\"a\":1}\n");
        let mut f = TranscriptFollow::new(path.clone(), FollowStart::FromStart);
        assert_eq!(f.poll().lines.len(), 1);
        std::fs::remove_file(&path).expect("rm");
        assert!(f.poll().missing);
        append(&path, b"{\"b\":1}\n");
        let p = f.poll();
        assert!(p.reset);
        assert_eq!(lines(&p), vec![r#"{"b":1}"#]);
    }

    /// T-S3f (follower half): a Chat view follower and an activity
    /// follower on the same file both see every appended record.
    #[test]
    fn two_followers_on_one_file_both_see_every_line() {
        let path = tmp("two");
        append(&path, b"{\"seed\":1}\n");
        let mut chat = TranscriptFollow::new(path.clone(), FollowStart::FromStart);
        let mut act = TranscriptFollow::new(path.clone(), FollowStart::FromEnd);
        assert_eq!(chat.poll().lines.len(), 1);
        assert!(act.poll().lines.is_empty());
        let mut seen_chat = Vec::new();
        let mut seen_act = Vec::new();
        for i in 0..20 {
            append(&path, format!("{{\"i\":{i}}}\n").as_bytes());
            if i % 3 == 0 {
                seen_chat.extend(chat.poll().lines);
            }
            if i % 4 == 0 {
                seen_act.extend(act.poll().lines);
            }
        }
        seen_chat.extend(chat.poll().lines);
        seen_act.extend(act.poll().lines);
        let want: Vec<String> = (0..20).map(|i| format!("{{\"i\":{i}}}")).collect();
        assert_eq!(seen_chat, want);
        assert_eq!(seen_act, want);
    }
}
