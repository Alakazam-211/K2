//! The consumer: makes the ring and reads the newest frame.
//!
//! The producer is another process with the file open read-write, so
//! nothing in the file is trusted here: the slot index, the frame's size,
//! rotation, colour and length are checked against the consumer's own
//! layout before a byte is copied, all maths on them is checked, and the
//! file's length is checked before each use of the mapping (a producer can
//! shrink the file, and touching a mapped page past its end faults).

use crate::layout::*;
use crate::shm::{monotonic_ns, try_lock, unlock, Map};
use crate::{Color, EndReason, FrameInfo, Layout, ProducerState, RingError, RingStats};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, AtomicU64, Ordering};

/// How often a read retries when the producer rewrote the slot under it.
const READ_TRIES: usize = 3;

/// The reading side of a ring, and its owner: it makes the file and
/// removes it when dropped.
pub struct Consumer {
    map: Map,
    file: File,
    path: PathBuf,
    /// The file's device and inode, to remove only our own file.
    id: (u64, u64),
    /// The layout as made, with the rate as last set here. The file's copy
    /// is the producer's to read, never ours to trust.
    layout: Layout,
    /// The frame number last handed out (0: none yet).
    last_frame: u64,
    /// The `H_LATEST` word last refused, to count a bad frame once.
    last_rejected: u64,
    missed: u64,
    torn: u64,
    rejected: u64,
}

impl std::fmt::Debug for Consumer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Consumer")
            .field("path", &self.path)
            .field("layout", &self.layout)
            .field("last_frame", &self.last_frame)
            .finish_non_exhaustive()
    }
}

/// Why a slot wasn't read.
enum Skip {
    /// The producer was writing it, or wrote it again while it was read:
    /// worth another try.
    Torn,
    /// It holds no valid frame (corrupt, or a misbehaving producer).
    Bad,
}

impl Consumer {
    /// Make a new, empty ring at `path` (mode 0600), replacing any file
    /// there: it is built under a fresh temporary name beside `path`
    /// (created exclusively, never through a symbolic link) and renamed
    /// over it, so a producer never sees a half-made ring. A producer still
    /// attached to a replaced ring writes into the old file, which nobody
    /// reads.
    pub fn create(path: impl AsRef<Path>, layout: Layout) -> Result<Consumer, RingError> {
        layout.validate()?;
        let path = path.as_ref().to_path_buf();
        let (tmp, file) = create_temp(&path)?;
        let map = match fill(&tmp, &path, &file, &layout) {
            Ok(m) => m,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(match e {
                    // A missing folder is an I/O problem here, not "no ring".
                    RingError::NotFound => RingError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("no folder for {}", path.display()),
                    )),
                    e => e,
                });
            }
        };
        let meta = file.metadata()?;
        Ok(Consumer {
            map,
            file,
            path,
            id: (meta.dev(), meta.ino()),
            layout,
            last_frame: 0,
            last_rejected: 0,
            missed: 0,
            torn: 0,
            rejected: 0,
        })
    }

    /// The ring's file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The ring's layout (its rate as last set by [`Consumer::set_max_fps`]).
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The mapping, if the file still covers all of it. A producer can
    /// shrink the file; touching a page past its end would kill this
    /// process (SIGBUS), so every use of the mapping goes through here.
    /// This narrows that window to the time between the check and the
    /// access; it can't close it (see the README's trust boundary).
    fn map(&self) -> Option<&Map> {
        let whole = self
            .file
            .metadata()
            .is_ok_and(|m| m.len() >= self.layout.file_len() as u64);
        whole.then_some(&self.map)
    }

    /// Mark the consumer as looking. Every read does it; call it on a
    /// timer if reads stop for a while on purpose.
    pub fn touch(&self) {
        if let Some(map) = self.map() {
            map.u64(H_BEAT).store(monotonic_ns(), Ordering::Release);
        }
    }

    /// Tell the producer whether to write frames (on by default).
    pub fn set_wanted(&self, wanted: bool) {
        let Some(map) = self.map() else { return };
        let flags = map.u32(H_FLAGS);
        if wanted {
            flags.fetch_or(F_WANTS, Ordering::AcqRel);
        } else {
            flags.fetch_and(!F_WANTS, Ordering::AcqRel);
        }
        map.u64(H_BEAT).store(monotonic_ns(), Ordering::Release);
    }

    /// Ask the producer to stop (it sees [`crate::Producer::stop_requested`]).
    pub fn request_stop(&self) {
        if let Some(map) = self.map() {
            map.u32(H_FLAGS).fetch_or(F_STOP, Ordering::AcqRel);
        }
    }

    /// Change the producer's highest rate (0: no limit). Applies from its
    /// next frame.
    pub fn set_max_fps(&mut self, fps: u32) -> Result<(), RingError> {
        if fps > MAX_FPS {
            return Err(RingError::Invalid(format!("{fps} fps")));
        }
        self.layout.max_fps = fps;
        if let Some(map) = self.map() {
            map.u32(H_MAX_FPS).store(fps, Ordering::Relaxed);
        }
        Ok(())
    }

    /// What the producer is doing. `Gone` when it started and its lock is
    /// gone without an end (the process was killed). `Mismatch` when the
    /// file has shrunk under the ring: end the share.
    pub fn producer(&self) -> Result<ProducerState, RingError> {
        let Some(map) = self.map() else {
            return Err(shrunk());
        };
        let state = map.u32(H_STATE);
        if state.load(Ordering::Acquire) == S_RUNNING {
            // Free means no producer holds it: it died without a word.
            if !try_lock(&self.file)? {
                return Ok(ProducerState::Running);
            }
            unlock(&self.file);
            // It may have ended just now; read once more, then decide.
            if state.load(Ordering::Acquire) == S_RUNNING {
                return Ok(ProducerState::Gone);
            }
        }
        Ok(match state.load(Ordering::Acquire) {
            // Running again: a new producer started after the check.
            S_RUNNING => ProducerState::Running,
            S_ENDED => {
                let r = map.u32(H_END_REASON).load(Ordering::Relaxed);
                ProducerState::Ended(EndReason::from_u32(r).unwrap_or(EndReason::SourceClosed))
            }
            _ => ProducerState::Waiting,
        })
    }

    /// The newest frame, if it isn't the one read last: its bytes (tight
    /// NV12) into `out`, which is resized to fit. Frames in between were
    /// skipped ([`RingStats::missed`]). Never blocks. None also when the
    /// newest slot holds no valid frame ([`RingStats::rejected`]) or the
    /// file has shrunk ([`Consumer::producer`] says so).
    pub fn try_read(&mut self, out: &mut Vec<u8>) -> Option<FrameInfo> {
        self.read(out, false)
    }

    /// The newest frame even if it was read already (to encode it again,
    /// for a keyframe on a screen that hasn't changed).
    pub fn read_latest(&mut self, out: &mut Vec<u8>) -> Option<FrameInfo> {
        self.read(out, true)
    }

    fn read(&mut self, out: &mut Vec<u8>, again: bool) -> Option<FrameInfo> {
        self.map()?
            .u64(H_BEAT)
            .store(monotonic_ns(), Ordering::Release);
        for _ in 0..READ_TRIES {
            let latest = self.map.u64(H_LATEST).load(Ordering::Acquire);
            let frame_no = latest >> 8;
            // A different number is a newer frame: numbers only count up
            // (and wrap after 2^56 - 1), so a stray huge one can't freeze
            // the ring.
            if frame_no == 0 || (!again && frame_no == self.last_frame) {
                return None;
            }
            match self.copy_slot(latest, out) {
                Ok(info) => {
                    if frame_no != self.last_frame {
                        if frame_no > self.last_frame {
                            let gap = frame_no - self.last_frame - 1;
                            self.missed = self.missed.saturating_add(gap);
                        }
                        self.last_frame = frame_no;
                    }
                    return Some(info);
                }
                Err(Skip::Bad) => {
                    if latest != self.last_rejected {
                        self.last_rejected = latest;
                        self.rejected = self.rejected.saturating_add(1);
                    }
                    return None;
                }
                Err(Skip::Torn) => {}
            }
        }
        self.torn = self.torn.saturating_add(1);
        None
    }

    /// Copy the slot `latest` points at, if it still holds that frame,
    /// describes a frame that fits this ring, and wasn't rewritten while
    /// copying. The caller checked the file's length.
    fn copy_slot(&self, latest: u64, out: &mut Vec<u8>) -> Result<FrameInfo, Skip> {
        let slot = latest & 0xff;
        if slot >= u64::from(self.layout.slots) {
            return Err(Skip::Bad);
        }
        // In the mapping: a slot below the layout's own count.
        let base = HEADER + slot as usize * self.layout.slot_stride();
        let seq = self.map.u64(base + SL_SEQ);
        let s1 = seq.load(Ordering::Acquire);
        if s1 & 1 == 1 {
            return Err(Skip::Torn);
        }
        let frame_no = self.map.u64(base + SL_FRAME_NO).load(Ordering::Relaxed);
        let width = self.map.u32(base + SL_WIDTH).load(Ordering::Relaxed);
        let height = self.map.u32(base + SL_HEIGHT).load(Ordering::Relaxed);
        let rotation = self.map.u32(base + SL_ROTATION).load(Ordering::Relaxed);
        let color = self.map.u32(base + SL_COLOR).load(Ordering::Relaxed);
        let timestamp_ns = self.map.u64(base + SL_TIMESTAMP).load(Ordering::Relaxed);
        let len = self.map.u64(base + SL_LEN).load(Ordering::Relaxed);
        if frame_no != latest >> 8 {
            // Written again since `latest` was read.
            return Err(Skip::Torn);
        }
        // Checked before any size maths, so nothing below can overflow:
        // the box is at most MAX_SIDE a side.
        let valid = (1..=MAX_FRAME_NO).contains(&frame_no)
            && width >= 1
            && height >= 1
            && self.layout.fits(width, height)
            && matches!(rotation, 0 | 90 | 180 | 270);
        let color = Color::from_u32(color).filter(|_| valid);
        let Some(color) = color else {
            return Err(self.bad_or_torn(seq, s1));
        };
        let info = FrameInfo {
            frame_no,
            width,
            height,
            rotation,
            color,
            timestamp_ns,
        };
        let want = info.len();
        if len != want as u64 || want > self.layout.slot_bytes() {
            return Err(self.bad_or_torn(seq, s1));
        }
        out.resize(want, 0);
        let data = self.map.bytes(base + SLOT_HEADER, want);
        // SAFETY: `want` bytes of this slot (bounds checked by `bytes`)
        // into `out`, which holds `want`. This is a seqlock read: the
        // producer may be rewriting these bytes in the other process while
        // they are copied, a data race on paper that only ever yields
        // stale or mixed bytes (plain `u8`s, every bit pattern valid). The
        // sequence check below throws such a copy away before anyone sees
        // it.
        unsafe { std::ptr::copy_nonoverlapping(data, out.as_mut_ptr(), want) };
        fence(Ordering::Acquire);
        if seq.load(Ordering::Relaxed) != s1 {
            return Err(Skip::Torn);
        }
        Ok(info)
    }

    /// A slot that failed a check is torn if it changed since `s1` (the
    /// fields were read mid-write), else bad.
    fn bad_or_torn(&self, seq: &AtomicU64, s1: u64) -> Skip {
        fence(Ordering::Acquire);
        if seq.load(Ordering::Relaxed) == s1 {
            Skip::Bad
        } else {
            Skip::Torn
        }
    }

    /// Counters. The producer's (written, dropped, attaches) are read from
    /// the file, so treat them as hints; they are 0 once the file has
    /// shrunk.
    pub fn stats(&self) -> RingStats {
        let shared = |off: usize| {
            self.map()
                .map_or(0, |m| m.u64(off).load(Ordering::Relaxed))
        };
        RingStats {
            written: shared(H_WRITTEN),
            dropped: shared(H_DROPPED),
            missed: self.missed,
            torn: self.torn,
            rejected: self.rejected,
            attaches: shared(H_ATTACHES),
        }
    }

    #[cfg(test)]
    pub(crate) fn raw_map(&self) -> &Map {
        &self.map
    }

    #[cfg(test)]
    pub(crate) fn file(&self) -> &File {
        &self.file
    }
}

/// The error for a file that has shrunk under the ring.
fn shrunk() -> RingError {
    RingError::Mismatch("the file has shrunk under the ring".into())
}

/// Create a new file beside `path` under a fresh name: exclusively, so an
/// existing file or symbolic link of that name (planted or left over) is
/// never opened, written or truncated.
fn create_temp(path: &Path) -> Result<(PathBuf, File), RingError> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let mut last = None;
    for _ in 0..8 {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(format!(
            ".new-{}-{:x}-{n}",
            std::process::id(),
            monotonic_ns()
        ));
        let tmp = PathBuf::from(tmp);
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(RingError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no folder for {}", path.display()),
                )))
            }
            Err(e) => return Err(RingError::Io(e)),
        }
    }
    Err(RingError::Io(last.unwrap_or_else(|| {
        std::io::Error::other("no free temporary name")
    })))
}

/// Size and map the new file, write the header, and rename it to `path`.
fn fill(tmp: &Path, path: &Path, file: &File, layout: &Layout) -> Result<Map, RingError> {
    file.set_len(layout.file_len() as u64)?;
    let map = Map::new(file, layout.file_len())?;
    map.write_header(layout);
    std::fs::rename(tmp, path)?;
    Ok(map)
}

impl Drop for Consumer {
    fn drop(&mut self) {
        if let Some(map) = self.map() {
            let flags = map.u32(H_FLAGS);
            flags.fetch_and(!F_WANTS, Ordering::AcqRel);
            flags.fetch_or(F_GONE, Ordering::AcqRel);
        }
        // Remove the file only if it is still ours (not replaced since).
        if let Ok(m) = std::fs::symlink_metadata(&self.path) {
            if (m.dev(), m.ino()) == self.id {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
