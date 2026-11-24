//! The system's random bytes, as Python's `os.urandom` and bash's
//! `/dev/urandom` reads take them: `/dev/urandom` on Unix, and on Windows
//! (where there is no such file) the system's preferred generator
//! (`BCryptGenRandom`, which `os.urandom` calls there too).
//!
//! [`fill`] says whether `buf` was filled. Callers keep their own answer
//! for a `false` (each one's is what its shell code does when the read
//! fails), so nothing here ever hands back zeroes as if they were random.

/// Fills `buf` with the system's random bytes; `false` when it can't.
#[cfg(unix)]
pub fn fill(buf: &mut [u8]) -> bool {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .is_ok()
}

/// Fills `buf` with the system's random bytes; `false` when it can't.
#[cfg(windows)]
pub fn fill(buf: &mut [u8]) -> bool {
    use windows_sys::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    // BCryptGenRandom takes a u32 length: fill in pieces that fit.
    for chunk in buf.chunks_mut(u32::MAX as usize) {
        // SAFETY: `chunk` is a live, writable buffer of `chunk.len()`
        // bytes; no algorithm handle is passed, so the system's
        // preferred generator fills it.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return false;
        }
    }
    true
}

/// Fills `buf` with the system's random bytes; `false` when it can't.
#[cfg(not(any(unix, windows)))]
pub fn fill(_buf: &mut [u8]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::fill;

    /// Two 32-byte draws are filled and differ; neither is all zeroes.
    #[test]
    fn the_system_fills_the_buffer() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        assert!(fill(&mut a), "the system gave no random bytes");
        assert!(fill(&mut b), "the system gave no random bytes");
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn an_empty_buffer_is_filled() {
        assert!(fill(&mut []));
    }
}
