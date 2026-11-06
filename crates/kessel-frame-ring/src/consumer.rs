//! The consumer: makes the ring and reads the newest frame.

use crate::layout::*;
use crate::shm::{monotonic_ns, try_lock, unlock, Map};
use crate::{Color, EndReason, FrameInfo, Layout, ProducerState, RingError, RingStats};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, Ordering};

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
    layout: Layout,
    last_frame: u64,
    missed: u64,
    torn: u64,
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

impl Consumer {
    /// Make a new, empty ring at `path` (mode 0600), replacing any file
    /// there: it is built under a temporary name beside `path` and renamed
    /// over it, so a producer never sees a half-made ring. A producer still
    /// attached to a replaced ring writes into the old file, which nobody
    /// reads.
    pub fn create(path: impl AsRef<Path>, layout: Layout) -> Result<Consumer, RingError> {
        layout.validate()?;
        let path = path.as_ref().to_path_buf();
        let mut tmp = path.clone().into_os_string();
        tmp.push(format!(".new-{}", std::process::id()));
        let tmp = PathBuf::from(tmp);
        let (file, map) = match make(&tmp, &path, &layout) {
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
            missed: 0,
            torn: 0,
        })
    }

    /// The ring's file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The ring's layout (its rate as last set).
    pub fn layout(&self) -> Layout {
        let mut l = self.layout;
        l.max_fps = self.map.u32(H_MAX_FPS).load(Ordering::Relaxed);
        l
    }

    /// Mark the consumer as looking. Every read does it; call it on a
    /// timer if reads stop for a while on purpose.
    pub fn touch(&self) {
        self.map
            .u64(H_BEAT)
            .store(monotonic_ns(), Ordering::Release);
    }

    /// Tell the producer whether to write frames (on by default).
    pub fn set_wanted(&self, wanted: bool) {
        let flags = self.map.u32(H_FLAGS);
        if wanted {
            flags.fetch_or(F_WANTS, Ordering::AcqRel);
        } else {
            flags.fetch_and(!F_WANTS, Ordering::AcqRel);
        }
        self.touch();
    }

    /// Ask the producer to stop (it sees [`crate::Producer::stop_requested`]).
    pub fn request_stop(&self) {
        self.map.u32(H_FLAGS).fetch_or(F_STOP, Ordering::AcqRel);
    }

    /// Change the producer's highest rate (0: no limit). Applies from its
    /// next frame.
    pub fn set_max_fps(&self, fps: u32) -> Result<(), RingError> {
        if fps > 240 {
            return Err(RingError::Invalid(format!("{fps} fps")));
        }
        self.map.u32(H_MAX_FPS).store(fps, Ordering::Relaxed);
        Ok(())
    }

    /// What the producer is doing. `Gone` when it started and its lock is
    /// gone without an end (the process was killed).
    pub fn producer(&self) -> Result<ProducerState, RingError> {
        let state = self.map.u32(H_STATE).load(Ordering::Acquire);
        match state {
            S_RUNNING => {
                // Free means no producer holds it: it died without a word.
                if try_lock(&self.file)? {
                    unlock(&self.file);
                    // It may have ended just now.
                    if self.map.u32(H_STATE).load(Ordering::Acquire) == S_RUNNING {
                        return Ok(ProducerState::Gone);
                    }
                    return self.producer();
                }
                Ok(ProducerState::Running)
            }
            S_ENDED => {
                let r = self.map.u32(H_END_REASON).load(Ordering::Relaxed);
                Ok(ProducerState::Ended(
                    EndReason::from_u32(r).unwrap_or(EndReason::SourceClosed),
                ))
            }
            _ => Ok(ProducerState::Waiting),
        }
    }

    /// The newest frame, if it is newer than the last one read: its bytes
    /// (tight NV12) into `out`, which is resized to fit. Frames in between
    /// were skipped ([`RingStats::missed`]). Never blocks.
    pub fn try_read(&mut self, out: &mut Vec<u8>) -> Option<FrameInfo> {
        self.read(out, false)
    }

    /// The newest frame even if it was read already (to encode it again,
    /// for a keyframe on a screen that hasn't changed).
    pub fn read_latest(&mut self, out: &mut Vec<u8>) -> Option<FrameInfo> {
        self.read(out, true)
    }

    fn read(&mut self, out: &mut Vec<u8>, again: bool) -> Option<FrameInfo> {
        self.touch();
        for _ in 0..READ_TRIES {
            let latest = self.map.u64(H_LATEST).load(Ordering::Acquire);
            let frame_no = latest >> 8;
            if latest == 0 || (!again && frame_no <= self.last_frame) {
                return None;
            }
            if let Some(info) = self.copy_slot(latest, out) {
                if frame_no > self.last_frame {
                    self.missed += frame_no - self.last_frame - 1;
                    self.last_frame = frame_no;
                }
                return Some(info);
            }
        }
        self.torn += 1;
        None
    }

    /// Copy the slot `latest` points at, if it still holds that frame and
    /// wasn't rewritten while copying.
    fn copy_slot(&self, latest: u64, out: &mut Vec<u8>) -> Option<FrameInfo> {
        let slot = (latest & 0xff) as usize;
        if slot >= self.layout.slots as usize {
            return None;
        }
        let base = HEADER + slot * self.layout.slot_stride();
        let seq = self.map.u64(base + SL_SEQ);
        let s1 = seq.load(Ordering::Acquire);
        if s1 & 1 == 1 {
            return None;
        }
        let info = FrameInfo {
            frame_no: self.map.u64(base + SL_FRAME_NO).load(Ordering::Relaxed),
            width: self.map.u32(base + SL_WIDTH).load(Ordering::Relaxed),
            height: self.map.u32(base + SL_HEIGHT).load(Ordering::Relaxed),
            rotation: self.map.u32(base + SL_ROTATION).load(Ordering::Relaxed),
            color: Color::from_u32(self.map.u32(base + SL_COLOR).load(Ordering::Relaxed))?,
            timestamp_ns: self.map.u64(base + SL_TIMESTAMP).load(Ordering::Relaxed),
        };
        let len = self.map.u64(base + SL_LEN).load(Ordering::Relaxed) as usize;
        if info.frame_no != latest >> 8
            || info.width == 0
            || info.height == 0
            || len != info.len()
            || len > self.layout.slot_bytes()
        {
            return None;
        }
        out.resize(len, 0);
        let data = self.map.bytes(base + SLOT_HEADER, len);
        // SAFETY: `len` bytes of this slot (bounds checked by `bytes`) into
        // `out`, which holds `len`. The producer may be rewriting them; the
        // sequence check below throws such a copy away.
        unsafe { std::ptr::copy_nonoverlapping(data, out.as_mut_ptr(), len) };
        fence(Ordering::Acquire);
        if seq.load(Ordering::Relaxed) != s1 {
            return None;
        }
        Some(info)
    }

    /// Counters.
    pub fn stats(&self) -> RingStats {
        RingStats {
            written: self.map.u64(H_WRITTEN).load(Ordering::Relaxed),
            dropped: self.map.u64(H_DROPPED).load(Ordering::Relaxed),
            missed: self.missed,
            torn: self.torn,
            attaches: self.map.u64(H_ATTACHES).load(Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(crate) fn map(&self) -> &Map {
        &self.map
    }
}

/// Build the ring at `tmp` and rename it to `path`.
fn make(tmp: &Path, path: &Path, layout: &Layout) -> Result<(File, Map), RingError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)?;
    file.set_len(layout.file_len() as u64)?;
    let map = Map::new(&file, layout.file_len())?;
    map.write_header(layout);
    std::fs::rename(tmp, path)?;
    Ok((file, map))
}

impl Drop for Consumer {
    fn drop(&mut self) {
        let flags = self.map.u32(H_FLAGS);
        flags.fetch_and(!F_WANTS, Ordering::AcqRel);
        flags.fetch_or(F_GONE, Ordering::AcqRel);
        // Remove the file only if it is still ours (not replaced since).
        if let Ok(m) = std::fs::metadata(&self.path) {
            if (m.dev(), m.ino()) == self.id {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
