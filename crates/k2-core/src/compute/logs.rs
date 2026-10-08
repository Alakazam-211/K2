//! The controller's copy of each job's log (§11.3):
//! `~/.k2/compute/jobs/<job>-g<generation>.log`, a journal of records
//! `u64 seq (BE) · u8 stream · u32 len (BE) · bytes`. Sequence numbers come
//! from the node and are continuous; a replayed chunk (seq ≤ last) is
//! dropped, so a reconnect never duplicates output. Readers page by byte
//! cursor; `since_seq` re-attach scans once.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use super::proto::frames::LogStream;

const HEADER: usize = 8 + 1 + 4;

pub fn stream_byte(s: LogStream) -> u8 {
    match s {
        LogStream::Out => 0,
        LogStream::Err => 1,
        LogStream::Sys => 2,
    }
}

pub fn stream_name(b: u8) -> &'static str {
    match b {
        0 => "out",
        1 => "err",
        _ => "sys",
    }
}

/// One record read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rec {
    pub seq: u64,
    pub stream: u8,
    pub data: Vec<u8>,
}

pub fn dir() -> PathBuf {
    super::compute_dir().join("jobs")
}

pub fn path(job_id: &str, generation: i64) -> PathBuf {
    dir().join(format!("{job_id}-g{generation}.log"))
}

/// (last seq, file size) per open log.
static TAILS: Mutex<Option<HashMap<PathBuf, (u64, u64)>>> = Mutex::new(None);

fn scan_tail(p: &PathBuf) -> (u64, u64) {
    let mut last = 0u64;
    let mut off = 0u64;
    if let Ok(mut f) = std::fs::File::open(p) {
        let mut head = [0u8; HEADER];
        loop {
            if f.read_exact(&mut head).is_err() {
                break;
            }
            let seq = u64::from_be_bytes(head[..8].try_into().unwrap_or([0; 8]));
            let len = u32::from_be_bytes(head[9..13].try_into().unwrap_or([0; 4])) as u64;
            if f.seek(SeekFrom::Current(len as i64)).is_err() {
                break;
            }
            last = seq;
            off += HEADER as u64 + len;
        }
    }
    (last, off)
}

/// Append one chunk. Returns `Ok(false)` when it was a replay (seq ≤ the
/// last stored) or out of order (a gap: the node replays from the hint).
pub fn append(job_id: &str, generation: i64, seq: u64, stream: LogStream, data: &[u8]) -> Result<bool, String> {
    let p = path(job_id, generation);
    let mut g = TAILS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    let (last, size) = *map.entry(p.clone()).or_insert_with(|| scan_tail(&p));
    if seq <= last || seq != last + 1 {
        return Ok(false);
    }
    std::fs::create_dir_all(dir()).map_err(|e| format!("log dir: {e}"))?;
    let mut f = OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("open log: {e}"))?;
    let mut rec = Vec::with_capacity(HEADER + data.len());
    rec.extend_from_slice(&seq.to_be_bytes());
    rec.push(stream_byte(stream));
    rec.extend_from_slice(&(data.len() as u32).to_be_bytes());
    rec.extend_from_slice(data);
    f.write_all(&rec).map_err(|e| format!("write log: {e}"))?;
    map.insert(p, (seq, size + rec.len() as u64));
    Ok(true)
}

/// Last stored seq.
pub fn last_seq(job_id: &str, generation: i64) -> u64 {
    let p = path(job_id, generation);
    let mut g = TAILS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.entry(p.clone()).or_insert_with(|| scan_tail(&p)).0
}

/// Records from byte `cursor`, up to about `max_bytes` of payload.
/// Returns the records and the next cursor.
pub fn read_from(job_id: &str, generation: i64, cursor: u64, max_bytes: usize) -> (Vec<Rec>, u64) {
    let p = path(job_id, generation);
    let mut out = Vec::new();
    let Ok(mut f) = std::fs::File::open(&p) else { return (out, cursor) };
    if f.seek(SeekFrom::Start(cursor)).is_err() {
        return (out, cursor);
    }
    let mut off = cursor;
    let mut total = 0usize;
    let mut head = [0u8; HEADER];
    while total < max_bytes {
        if f.read_exact(&mut head).is_err() {
            break;
        }
        let seq = u64::from_be_bytes(head[..8].try_into().unwrap_or([0; 8]));
        let len = u32::from_be_bytes(head[9..13].try_into().unwrap_or([0; 4])) as usize;
        let mut data = vec![0u8; len];
        if f.read_exact(&mut data).is_err() {
            break; // a record still being written: stop before it
        }
        off += (HEADER + len) as u64;
        total += len;
        out.push(Rec { seq, stream: head[8], data });
    }
    (out, off)
}

/// The byte cursor just after `since_seq` (0 = from the start).
pub fn cursor_after_seq(job_id: &str, generation: i64, since_seq: u64) -> u64 {
    if since_seq == 0 {
        return 0;
    }
    let p = path(job_id, generation);
    let Ok(mut f) = std::fs::File::open(&p) else { return 0 };
    let mut off = 0u64;
    let mut head = [0u8; HEADER];
    loop {
        if f.read_exact(&mut head).is_err() {
            return off;
        }
        let seq = u64::from_be_bytes(head[..8].try_into().unwrap_or([0; 8]));
        let len = u32::from_be_bytes(head[9..13].try_into().unwrap_or([0; 4])) as u64;
        if seq > since_seq {
            return off;
        }
        if f.seek(SeekFrom::Current(len as i64)).is_err() {
            return off;
        }
        off += HEADER as u64 + len;
    }
}

/// Delete logs older than `days`. Best effort.
pub fn sweep(days: u64) {
    let Ok(rd) = std::fs::read_dir(dir()) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    for e in rd.flatten() {
        if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < cutoff) {
            let p = e.path();
            let _ = std::fs::remove_file(&p);
            if let Some(m) = TAILS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                m.remove(&p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_dedupes_replays_refuses_gaps_and_pages() {
        crate::tunnel::test_support::with_temp_home(|| {
            assert!(append("j", 1, 1, LogStream::Out, b"hello ").unwrap());
            assert!(append("j", 1, 2, LogStream::Err, b"oops").unwrap());
            assert!(!append("j", 1, 2, LogStream::Err, b"oops").unwrap(), "replay dropped");
            assert!(!append("j", 1, 5, LogStream::Out, b"gap").unwrap(), "gap refused");
            assert!(append("j", 1, 3, LogStream::Sys, b"[k2-node] done").unwrap());
            assert_eq!(last_seq("j", 1), 3);
            let (recs, cur) = read_from("j", 1, 0, 1 << 20);
            assert_eq!(recs.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
            assert_eq!(recs[1].data, b"oops");
            assert_eq!(stream_name(recs[1].stream), "err");
            let (more, cur2) = read_from("j", 1, cur, 1 << 20);
            assert!(more.is_empty());
            assert_eq!(cur2, cur);
            // Re-attach after seq 1.
            let c = cursor_after_seq("j", 1, 1);
            let (recs, _) = read_from("j", 1, c, 1 << 20);
            assert_eq!(recs.first().map(|r| r.seq), Some(2));
            // Paging by size.
            let (one, _) = read_from("j", 1, 0, 1);
            assert_eq!(one.len(), 1);
            // Another generation is its own log.
            assert_eq!(last_seq("j", 2), 0);
        });
    }
}
