//! Per-session output ring (PRD §7.4, P20): monotonic `u64` offsets,
//! allocated lazily in 64 KiB steps up to the session's cap.

use std::collections::VecDeque;

pub const DEFAULT_RING: usize = 4 << 20;
pub const MIN_RING: usize = 256 << 10;
pub const MAX_RING: usize = 64 << 20;
pub const TOTAL_CAP: usize = 512 << 20;
const GROW_STEP: usize = 64 << 10;

pub fn clamp_ring(requested: Option<u64>) -> usize {
    match requested {
        None => DEFAULT_RING,
        Some(n) => (n.min(MAX_RING as u64) as usize).max(MIN_RING),
    }
}

pub struct Ring {
    cap: usize,
    buf: VecDeque<u8>,
    /// Offset of `buf[0]`.
    start: u64,
}

impl Ring {
    pub fn new(cap: usize) -> Ring {
        Ring { cap, buf: VecDeque::new(), start: 0 }
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Rebuild a ring at exact offsets (self-upgrade, PRD §7.10): `data`
    /// covers `[start, start + data.len())`. `data` longer than `cap` keeps
    /// its tail (never happens for a ring this build wrote).
    pub fn restore(cap: usize, start: u64, data: Vec<u8>) -> Ring {
        let mut r = Ring { cap, buf: VecDeque::from(data), start };
        let over = r.buf.len().saturating_sub(cap);
        if over > 0 {
            r.buf.drain(..over);
            r.start += over as u64;
        }
        r
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.cap {
            let skip = bytes.len() - self.cap;
            self.start += (self.buf.len() + skip) as u64;
            self.buf.clear();
            self.reserve(self.cap);
            self.buf.extend(&bytes[skip..]);
            return;
        }
        let over = (self.buf.len() + bytes.len()).saturating_sub(self.cap);
        if over > 0 {
            self.buf.drain(..over);
            self.start += over as u64;
        }
        self.reserve(self.buf.len() + bytes.len());
        self.buf.extend(bytes);
    }

    fn reserve(&mut self, want: usize) {
        if self.buf.capacity() < want {
            let target = want.div_ceil(GROW_STEP) * GROW_STEP;
            let target = target.min(self.cap.max(want));
            self.buf.reserve_exact(target - self.buf.len());
        }
    }

    /// Drop the oldest `n` bytes (total-cap trimming).
    pub fn trim_front(&mut self, n: usize) {
        let n = n.min(self.buf.len());
        self.buf.drain(..n);
        self.start += n as u64;
        self.buf.shrink_to(self.buf.len().max(GROW_STEP));
    }

    /// Up to two slices covering `[from, end)`. `from` is clamped to `start`.
    pub fn slices_from(&self, from: u64) -> (&[u8], &[u8]) {
        let from = from.max(self.start).min(self.end());
        let skip = (from - self.start) as usize;
        let (a, b) = self.buf.as_slices();
        if skip >= a.len() {
            (&b[skip - a.len()..], &[])
        } else {
            (&a[skip..], b)
        }
    }

    pub fn copy_from(&self, from: u64) -> Vec<u8> {
        let (a, b) = self.slices_from(from);
        let mut v = Vec::with_capacity(a.len() + b.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_monotonic_across_wrap() {
        let mut r = Ring::new(MIN_RING);
        let chunk: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let mut all = Vec::new();
        for _ in 0..5 {
            r.push(&chunk);
            all.extend_from_slice(&chunk);
        }
        assert_eq!(r.end(), all.len() as u64);
        assert_eq!(r.len(), MIN_RING);
        assert_eq!(r.start(), (all.len() - MIN_RING) as u64);
        assert_eq!(r.copy_from(0), all[all.len() - MIN_RING..].to_vec(), "from < start clamps to start");
        let mid = r.start() + 1234;
        assert_eq!(r.copy_from(mid), all[mid as usize..].to_vec());
        assert!(r.copy_from(r.end()).is_empty());
    }

    #[test]
    fn one_push_larger_than_cap_keeps_the_tail() {
        let mut r = Ring::new(MIN_RING);
        r.push(b"abc");
        let big: Vec<u8> = (0..(MIN_RING + 10)).map(|i| (i % 7) as u8).collect();
        r.push(&big);
        assert_eq!(r.end(), 3 + big.len() as u64);
        assert_eq!(r.copy_from(0), big[10..].to_vec());
    }

    #[test]
    fn grows_lazily_and_trims() {
        let mut r = Ring::new(DEFAULT_RING);
        r.push(&[1u8; 10]);
        assert!(r.buf.capacity() < DEFAULT_RING, "no eager 4 MiB allocation");
        r.trim_front(4);
        assert_eq!((r.start(), r.end()), (4, 10));
    }

    #[test]
    fn restore_keeps_exact_offsets() {
        let mut r = Ring::new(MIN_RING);
        let data: Vec<u8> = (0..(MIN_RING + 5000)).map(|i| (i % 253) as u8).collect();
        r.push(&data);
        let copy = Ring::restore(r.cap(), r.start(), r.copy_from(0));
        assert_eq!((copy.start(), copy.end(), copy.cap()), (r.start(), r.end(), r.cap()));
        assert_eq!(copy.copy_from(r.start() + 17), r.copy_from(r.start() + 17));
    }

    #[test]
    fn clamp() {
        assert_eq!(clamp_ring(None), DEFAULT_RING);
        assert_eq!(clamp_ring(Some(1)), MIN_RING);
        assert_eq!(clamp_ring(Some(u64::MAX)), MAX_RING);
    }
}
