//! The per-attempt log journal (§11.3). Every chunk hits disk before it
//! is sent, so a dropped connection never loses output; on reconnect the
//! controller's `since_seq` picks up where it stopped.
//!
//! Record: `u64 seq BE | u8 stream | u32 len BE | bytes`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use k2_node_proto::crypto::Sha256Stream;
use k2_node_proto::frames::{LogStream, MAX_LOG_CHUNK};

const HEADER: usize = 13;

fn stream_byte(s: LogStream) -> u8 {
    match s {
        LogStream::Out => 1,
        LogStream::Err => 2,
        LogStream::Sys => 3,
    }
}

fn stream_of(b: u8) -> Option<LogStream> {
    match b {
        1 => Some(LogStream::Out),
        2 => Some(LogStream::Err),
        3 => Some(LogStream::Sys),
        _ => None,
    }
}

/// Appends records with the job's log cap applied.
pub struct JournalWriter {
    file: File,
    pub seq: u64,
    /// Payload bytes kept (what the receipt hashes).
    pub bytes: u64,
    pub dropped: u64,
    cap: u64,
    hash: Option<Sha256Stream>,
    /// Secret-env (SE23): every append is redacted before it is written
    /// (the stdout/stderr pumps also stream-redact first, so a value split
    /// across reads is caught too).
    redactor: Option<std::sync::Arc<k2_redact::Redactor>>,
}

impl JournalWriter {
    pub fn create(path: &Path, cap: u64) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("open journal {}: {e}", path.display()))?;
        Ok(Self { file, seq: 0, bytes: 0, dropped: 0, cap, hash: Some(Sha256Stream::default()), redactor: None })
    }

    /// From now on, redact every append with `r` (a secret job, SE23).
    pub fn set_redactor(&mut self, r: std::sync::Arc<k2_redact::Redactor>) {
        self.redactor = Some(r);
    }

    /// The redactor in force, if any (the job's other text uses it too).
    pub fn redactor(&self) -> Option<&std::sync::Arc<k2_redact::Redactor>> {
        self.redactor.as_ref()
    }

    /// Open an existing journal to add to it: its sequence, kept bytes and
    /// hash carry on from what's there (a job whose end is recorded later
    /// by someone other than its runner).
    pub fn reopen(path: &Path, cap: u64) -> Result<Self, String> {
        let mut w = Self::create(path, cap)?;
        let mut r = JournalReader::new(path.to_path_buf());
        loop {
            let recs = r.read_more(256)?;
            if recs.is_empty() {
                break;
            }
            for rec in recs {
                w.seq = rec.seq;
                w.bytes += rec.data.len() as u64;
                if let Some(h) = w.hash.as_mut() {
                    h.update(&rec.data);
                }
            }
        }
        // A torn last record (its writer died mid-write) goes, so what's
        // added next is readable.
        w.file.set_len(r.offset).map_err(|e| format!("trim journal {}: {e}", path.display()))?;
        Ok(w)
    }

    fn record(&mut self, stream: LogStream, data: &[u8]) -> Result<(), String> {
        self.seq += 1;
        let mut buf = Vec::with_capacity(HEADER + data.len());
        buf.extend_from_slice(&self.seq.to_be_bytes());
        buf.push(stream_byte(stream));
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(data);
        self.file.write_all(&buf).map_err(|e| format!("write journal: {e}"))?;
        self.bytes += data.len() as u64;
        if let Some(h) = self.hash.as_mut() {
            h.update(data);
        }
        Ok(())
    }

    /// Append output, split into chunks ≤ MAX_LOG_CHUNK, honouring the cap.
    /// Returns how many records were written.
    pub fn append(&mut self, stream: LogStream, data: &[u8]) -> Result<usize, String> {
        let redacted;
        let data = match self.redactor.as_ref().filter(|r| r.hits(data)) {
            Some(r) => {
                redacted = r.redact(data);
                &redacted[..]
            }
            None => data,
        };
        let room = self.cap.saturating_sub(self.bytes) as usize;
        let keep = data.len().min(room);
        self.dropped += (data.len() - keep) as u64;
        let mut n = 0;
        for chunk in data[..keep].chunks(MAX_LOG_CHUNK) {
            self.record(stream, chunk)?;
            n += 1;
        }
        Ok(n)
    }

    /// Close out: the truncation marker (if anything was dropped), then
    /// `(sha256 hex, bytes, truncated)`.
    pub fn finish(&mut self) -> Result<(String, u64, bool), String> {
        let truncated = self.dropped > 0;
        if truncated {
            let msg = format!("[k2-node] log truncated at {} bytes; {} bytes dropped\n", self.cap, self.dropped);
            self.record(LogStream::Sys, msg.as_bytes())?;
        }
        self.file.sync_all().map_err(|e| format!("sync journal: {e}"))?;
        let sha = self.hash.take().map(|h| h.finish_hex()).unwrap_or_default();
        Ok((sha, self.bytes, truncated))
    }
}

/// One record read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub stream: LogStream,
    pub data: Vec<u8>,
}

/// Reads records after a position. A torn tail (writer mid-record) stops
/// the read; the next call resumes at the same offset.
pub struct JournalReader {
    path: PathBuf,
    pub offset: u64,
    pub last_seq: u64,
}

impl JournalReader {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0, last_seq: 0 }
    }

    /// Up to `max` complete records with seq > `last_seq`, advancing.
    pub fn read_more(&mut self, max: usize) -> Result<Vec<Record>, String> {
        let mut f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("open journal: {e}")),
        };
        let len = f.metadata().map_err(|e| format!("stat journal: {e}"))?.len();
        f.seek(SeekFrom::Start(self.offset)).map_err(|e| format!("seek journal: {e}"))?;
        let mut out = Vec::new();
        while out.len() < max && self.offset + HEADER as u64 <= len {
            let mut h = [0u8; HEADER];
            f.read_exact(&mut h).map_err(|e| format!("read journal: {e}"))?;
            let seq = u64::from_be_bytes(h[..8].try_into().unwrap_or([0; 8]));
            let stream = stream_of(h[8]).ok_or("journal: bad stream byte")?;
            let n = u32::from_be_bytes(h[9..13].try_into().unwrap_or([0; 4])) as u64;
            if self.offset + HEADER as u64 + n > len {
                break;
            }
            let mut data = vec![0u8; n as usize];
            f.read_exact(&mut data).map_err(|e| format!("read journal: {e}"))?;
            self.offset += HEADER as u64 + n;
            if seq > self.last_seq {
                self.last_seq = seq;
                out.push(Record { seq, stream, data });
            }
        }
        Ok(out)
    }

    /// Skip records up to and including `seq` (resume after a reconnect).
    pub fn skip_to(&mut self, seq: u64) {
        self.last_seq = self.last_seq.max(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_split_and_resume() {
        let d = crate::util::temp_dir("journal");
        let p = d.join("log");
        let mut w = JournalWriter::create(&p, 1 << 30).unwrap();
        w.append(LogStream::Out, b"hello\n").unwrap();
        let big = vec![b'x'; MAX_LOG_CHUNK + 10];
        assert_eq!(w.append(LogStream::Err, &big).unwrap(), 2);
        w.append(LogStream::Out, b"bye\n").unwrap();
        let mut r = JournalReader::new(p.clone());
        let recs = r.read_more(100).unwrap();
        assert_eq!(recs.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(recs[1].data.len(), MAX_LOG_CHUNK);
        assert_eq!(recs[2].stream, LogStream::Err);
        assert!(r.read_more(100).unwrap().is_empty());
        let mut r2 = JournalReader::new(p);
        r2.skip_to(2);
        let rest = r2.read_more(100).unwrap();
        assert_eq!(rest.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![3, 4]);
    }

    #[test]
    fn torn_tail_is_left_for_later() {
        let d = crate::util::temp_dir("journal-torn");
        let p = d.join("log");
        let mut w = JournalWriter::create(&p, 1 << 20).unwrap();
        w.append(LogStream::Out, b"one").unwrap();
        drop(w);
        // Half a record.
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&2u64.to_be_bytes()).unwrap();
        let mut r = JournalReader::new(p.clone());
        assert_eq!(r.read_more(10).unwrap().len(), 1);
        f.write_all(&[1, 0, 0, 0, 2, b'h', b'i']).unwrap();
        let more = r.read_more(10).unwrap();
        assert_eq!(more.len(), 1);
        assert_eq!(more[0].data, b"hi");
    }

    #[test]
    fn cap_truncates_with_one_marker() {
        let d = crate::util::temp_dir("journal-cap");
        let p = d.join("log");
        let mut w = JournalWriter::create(&p, 10).unwrap();
        w.append(LogStream::Out, b"0123456789abcdef").unwrap();
        w.append(LogStream::Out, b"more").unwrap();
        let (sha, bytes, truncated) = w.finish().unwrap();
        assert!(truncated);
        assert!(bytes > 10, "the marker is part of the log");
        assert_eq!(sha.len(), 64);
        let recs = JournalReader::new(p).read_more(10).unwrap();
        assert_eq!(recs[0].data, b"0123456789");
        let last = recs.last().unwrap();
        assert_eq!(last.stream, LogStream::Sys);
        assert_eq!(String::from_utf8_lossy(&last.data), "[k2-node] log truncated at 10 bytes; 10 bytes dropped\n");
        assert_eq!(recs.len(), 2);
    }
}
