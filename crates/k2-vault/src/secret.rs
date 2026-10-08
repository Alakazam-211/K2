//! Secret values and key material in memory.
//!
//! - [`SecretBytes`] holds a value. It has no `Debug`, `Display`,
//!   `Serialize` or `Clone` (a compile-time check below keeps it that way),
//!   and it is zeroized on drop.
//! - [`Key32`] holds a 32-byte key in its own page-aligned allocation that
//!   is `mlock`ed (best effort) and, on Linux, excluded from core dumps
//!   (`MADV_DONTDUMP`). Zeroized and unlocked on drop.
//!
//! What this does not claim: transient copies made by HTTP stacks, the
//! redactor's automaton and SQLite's page cache are short-lived but not
//! wiped by us (VS18).

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use zeroize::Zeroize;

/// A secret value. The only way to read it is [`SecretBytes::expose`].
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(v: Vec<u8>) -> Self {
        SecretBytes(v)
    }

    pub fn from_slice(v: &[u8]) -> Self {
        SecretBytes(v.to_vec())
    }

    pub fn from_string(s: String) -> Self {
        SecretBytes(s.into_bytes())
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// The value as UTF-8, when it is.
    pub fn expose_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// An explicit copy (there is no `Clone`, so copies are visible in review).
    pub fn duplicate(&self) -> SecretBytes {
        SecretBytes(self.0.clone())
    }

    /// Trim leading/trailing ASCII whitespace (pasted keys often carry a
    /// trailing newline; VS5). Returns true when something was trimmed.
    pub fn trim_ascii_whitespace(&mut self) -> bool {
        let start = self.0.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(self.0.len());
        let end = self.0.iter().rposition(|b| !b.is_ascii_whitespace()).map(|i| i + 1).unwrap_or(start);
        if start == 0 && end == self.0.len() {
            return false;
        }
        let trimmed = self.0[start..end].to_vec();
        self.0.zeroize();
        self.0 = trimmed;
        true
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

// Compile-time guard: SecretBytes must never implement Debug, Display,
// Serialize or Clone. If someone adds one of those impls, the trait method
// below becomes ambiguous and this crate stops compiling.
#[allow(dead_code)]
mod no_leaky_impls {
    use super::SecretBytes;
    trait AmbiguousIfImpl<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
    struct InvalidDebug;
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfImpl<InvalidDebug> for T {}
    struct InvalidDisplay;
    impl<T: ?Sized + std::fmt::Display> AmbiguousIfImpl<InvalidDisplay> for T {}
    struct InvalidSerialize;
    impl<T: ?Sized + serde::Serialize> AmbiguousIfImpl<InvalidSerialize> for T {}
    struct InvalidClone;
    impl<T: ?Sized + Clone> AmbiguousIfImpl<InvalidClone> for T {}
    const _: fn() = || {
        let _ = <SecretBytes as AmbiguousIfImpl<_>>::some_item;
        let _ = <super::Key32 as AmbiguousIfImpl<_>>::some_item;
    };
}

static MLOCK_FAILED: AtomicBool = AtomicBool::new(false);

/// True when at least one key page could not be locked in memory.
pub fn mlock_failed() -> bool {
    MLOCK_FAILED.load(Ordering::Relaxed)
}

fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p <= 0 {
        4096
    } else {
        p as usize
    }
}

/// A 32-byte key in locked, dump-excluded memory.
pub struct Key32 {
    ptr: NonNull<u8>,
    layout: Layout,
    locked: bool,
}

// SAFETY: Key32 owns its allocation exclusively; the bytes are plain data.
unsafe impl Send for Key32 {}
// SAFETY: shared access is read-only.
unsafe impl Sync for Key32 {}

impl Key32 {
    fn alloc() -> Key32 {
        let ps = page_size();
        let layout = Layout::from_size_align(ps, ps).expect("page layout");
        // SAFETY: layout has non-zero size.
        let raw = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        // SAFETY: ptr/len describe our own allocation.
        let locked = unsafe { libc::mlock(ptr.as_ptr() as *const libc::c_void, ps) } == 0;
        if !locked {
            MLOCK_FAILED.store(true, Ordering::Relaxed);
        }
        #[cfg(target_os = "linux")]
        // SAFETY: page-aligned region we own.
        unsafe {
            libc::madvise(ptr.as_ptr() as *mut libc::c_void, ps, libc::MADV_DONTDUMP);
        }
        Key32 { ptr, layout, locked }
    }

    pub fn from_bytes(b: &[u8; 32]) -> Key32 {
        let k = Key32::alloc();
        // SAFETY: allocation is at least one page (>= 32 bytes).
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), k.ptr.as_ptr(), 32) };
        k
    }

    /// Take a 32-byte buffer, wiping the source.
    pub fn from_array(mut b: [u8; 32]) -> Key32 {
        let k = Key32::from_bytes(&b);
        b.zeroize();
        k
    }

    pub fn random() -> Key32 {
        let k = Key32::alloc();
        // SAFETY: we own 32 writable bytes at ptr.
        let buf = unsafe { std::slice::from_raw_parts_mut(k.ptr.as_ptr(), 32) };
        getrandom::getrandom(buf).expect("OS randomness");
        k
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        // SAFETY: the allocation holds at least 32 initialised bytes.
        unsafe { &*(self.ptr.as_ptr() as *const [u8; 32]) }
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn duplicate(&self) -> Key32 {
        Key32::from_bytes(self.as_bytes())
    }
}

impl Drop for Key32 {
    fn drop(&mut self) {
        // SAFETY: we own the allocation; wipe, unlock and free it.
        unsafe {
            let s = std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.layout.size());
            s.zeroize();
            if self.locked {
                libc::munlock(self.ptr.as_ptr() as *const libc::c_void, self.layout.size());
            }
            dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}

impl PartialEq for Key32 {
    fn eq(&self, other: &Self) -> bool {
        // Constant-time compare.
        let mut d = 0u8;
        for (a, b) in self.as_bytes().iter().zip(other.as_bytes().iter()) {
            d |= a ^ b;
        }
        d == 0
    }
}

/// Make the daemon non-dumpable (Q18 yes, VS18): blocks same-uid ptrace
/// and `/proc/<pid>/mem` reads, and turns off core dumps. Linux only; on
/// macOS the hardened runtime without `get-task-allow` already blocks
/// same-user `task_for_pid`. Returns whether the call succeeded.
pub fn set_non_dumpable() -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl with PR_SET_DUMPABLE takes plain integers.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key32_round_trips_and_compares() {
        let a = Key32::from_bytes(&[7u8; 32]);
        let b = a.duplicate();
        assert!(a == b);
        assert_eq!(a.as_bytes(), &[7u8; 32]);
        let r = Key32::random();
        assert!(r != a);
    }

    #[test]
    fn trim_removes_pasted_newlines_only_at_the_edges() {
        let mut s = SecretBytes::from_slice(b"  k2test_abc\n");
        assert!(s.trim_ascii_whitespace());
        assert_eq!(s.expose(), b"k2test_abc");
        let mut t = SecretBytes::from_slice(b"a b");
        assert!(!t.trim_ascii_whitespace());
        assert_eq!(t.expose(), b"a b");
        let mut e = SecretBytes::from_slice(b" \n");
        assert!(e.trim_ascii_whitespace());
        assert!(e.is_empty());
    }
}
