//! The shared mapping, the producer's lock and the cross-process clock.

use crate::layout::*;
use crate::{Layout, RingError};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A read-write `MAP_SHARED` mapping of the whole ring file.
pub(crate) struct Map {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is plain shared memory; every shared word is read and
// written through atomics, and frame bytes only under a slot's seqlock.
unsafe impl Send for Map {}

impl Map {
    /// Map `len` bytes of `file` (opened read-write).
    pub(crate) fn new(file: &File, len: usize) -> Result<Map, RingError> {
        // SAFETY: a valid fd and length; the kernel picks the address;
        // MAP_FAILED is checked.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(RingError::Io(std::io::Error::last_os_error()));
        }
        Ok(Map {
            ptr: ptr as *mut u8,
            len,
        })
    }

    /// The u32 word at `off` (4-aligned, in the mapping).
    pub(crate) fn u32(&self, off: usize) -> &AtomicU32 {
        assert!(off.is_multiple_of(4) && off + 4 <= self.len, "u32 at {off}");
        // SAFETY: in bounds and aligned (the mapping is page-aligned), and
        // only ever accessed atomically while the mapping lives (`&self`).
        unsafe { &*(self.ptr.add(off) as *const AtomicU32) }
    }

    /// The u64 word at `off` (8-aligned, in the mapping).
    pub(crate) fn u64(&self, off: usize) -> &AtomicU64 {
        assert!(off.is_multiple_of(8) && off + 8 <= self.len, "u64 at {off}");
        // SAFETY: as in `u32`.
        unsafe { &*(self.ptr.add(off) as *const AtomicU64) }
    }

    /// The `len` bytes at `off`, as a raw pointer (checked in bounds).
    pub(crate) fn bytes(&self, off: usize, len: usize) -> *mut u8 {
        assert!(
            off.checked_add(len).is_some_and(|end| end <= self.len),
            "{len} bytes at {off}"
        );
        // SAFETY: in bounds, checked above.
        unsafe { self.ptr.add(off) }
    }

    /// The layout in the header, checked against itself and the mapping's
    /// length.
    pub(crate) fn read_layout(&self) -> Result<Layout, RingError> {
        let bad = |why: String| Err(RingError::Mismatch(why));
        if self.len < HEADER {
            return bad(format!("{} bytes, under the header", self.len));
        }
        if self.u64(H_MAGIC).load(Ordering::Acquire) != MAGIC {
            return bad("no magic".into());
        }
        let version = self.u32(H_VERSION).load(Ordering::Relaxed);
        if version != crate::VERSION {
            return bad(format!("version {version}, not {}", crate::VERSION));
        }
        let (hs, sh) = (
            self.u32(H_HEADER_SIZE).load(Ordering::Relaxed) as usize,
            self.u32(H_SLOT_HEADER).load(Ordering::Relaxed) as usize,
        );
        if (hs, sh) != (HEADER, SLOT_HEADER) {
            return bad(format!("headers of {hs} and {sh} bytes"));
        }
        let layout = Layout {
            max_width: self.u32(H_MAX_WIDTH).load(Ordering::Relaxed),
            max_height: self.u32(H_MAX_HEIGHT).load(Ordering::Relaxed),
            slots: self.u32(H_SLOTS).load(Ordering::Relaxed),
            max_fps: self.u32(H_MAX_FPS).load(Ordering::Relaxed),
        };
        layout
            .validate()
            .map_err(|e| RingError::Mismatch(format!("its layout: {e}")))?;
        let slot_bytes = self.u64(H_SLOT_BYTES).load(Ordering::Relaxed);
        if slot_bytes != layout.slot_bytes() as u64 {
            return bad(format!(
                "{slot_bytes}-byte slots, not {} for its box",
                layout.slot_bytes()
            ));
        }
        if self.len != layout.file_len() {
            return bad(format!(
                "{} bytes, not the {} its layout needs",
                self.len,
                layout.file_len()
            ));
        }
        Ok(layout)
    }

    /// Write a new header (the file is not shared yet).
    pub(crate) fn write_header(&self, layout: &Layout) {
        self.u32(H_VERSION).store(crate::VERSION, Ordering::Relaxed);
        self.u32(H_HEADER_SIZE)
            .store(HEADER as u32, Ordering::Relaxed);
        self.u32(H_SLOT_HEADER)
            .store(SLOT_HEADER as u32, Ordering::Relaxed);
        self.u32(H_SLOTS).store(layout.slots, Ordering::Relaxed);
        self.u32(H_MAX_WIDTH)
            .store(layout.max_width, Ordering::Relaxed);
        self.u32(H_MAX_HEIGHT)
            .store(layout.max_height, Ordering::Relaxed);
        self.u64(H_SLOT_BYTES)
            .store(layout.slot_bytes() as u64, Ordering::Relaxed);
        self.u32(H_MAX_FPS).store(layout.max_fps, Ordering::Relaxed);
        self.u32(H_FLAGS).store(F_WANTS, Ordering::Relaxed);
        self.u64(H_BEAT).store(monotonic_ns(), Ordering::Relaxed);
        self.u32(H_STATE).store(S_WAITING, Ordering::Relaxed);
        self.u64(H_MAGIC).store(MAGIC, Ordering::Release);
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: mapped in `new` with this length, unmapped once; no
        // reference into it outlives `self`.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

/// Take the producer's lock on `file` without waiting: `Ok(false)` when
/// another open of the file holds it. Released when the file is closed,
/// including when the process dies.
pub(crate) fn try_lock(file: &File) -> Result<bool, RingError> {
    // SAFETY: a valid fd; flock has no memory arguments.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(RingError::Io(e))
}

/// Release the lock taken by [`try_lock`] on `file`.
pub(crate) fn unlock(file: &File) {
    // SAFETY: a valid fd; flock has no memory arguments.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

/// Nanoseconds on the machine's monotonic clock, the same in every process:
/// `CLOCK_UPTIME_RAW` on Apple (mach absolute time, the host clock that a
/// `CMSampleBuffer`'s presentation time uses), `CLOCK_MONOTONIC` elsewhere.
pub fn monotonic_ns() -> u64 {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let clock = libc::CLOCK_MONOTONIC;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid clock id and out pointer.
    let rc = unsafe { libc::clock_gettime(clock, &mut ts) };
    assert_eq!(rc, 0, "clock_gettime");
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}
