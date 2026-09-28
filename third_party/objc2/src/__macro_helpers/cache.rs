use core::ffi::{c_char, c_void, CStr};
use core::ptr;
use core::str;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::ffi;
use crate::runtime::{AnyClass, Sel};

/// Allows storing a [`Sel`] in a static and lazily loading it.
#[derive(Debug)]
pub struct CachedSel {
    ptr: AtomicPtr<c_void>,
}

impl CachedSel {
    /// Constructs a new [`CachedSel`].
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            ptr: AtomicPtr::new(ptr::null_mut()),
        }
    }

    // Mark as cold since this should only ever be called once (or maybe twice
    // if running on multiple threads).
    #[cold]
    unsafe fn fetch(&self, name: *const c_char) -> Sel {
        // SAFETY: Input is a non-null, NUL-terminated C-string pointer.
        //
        // We know this, because we construct it in `sel!` ourselves
        let registered = unsafe { Sel::register_unchecked(name) };
        self.cache_registered(registered)
    }

    /// Store a registered selector. `None` is not cached; the next `get`
    /// tries again and the caller receives the skip sentinel.
    fn cache_registered(&self, registered: Option<Sel>) -> Sel {
        if let Some(sel) = registered {
            self.ptr.store(sel.as_ptr().cast_mut(), Ordering::Relaxed);
            sel
        } else {
            // Do not store the sentinel. It is not a real selector.
            Sel::skip_sentinel()
        }
    }

    /// Returns the cached selector. If no selector is yet cached, registers
    /// one with the given name and stores it.
    #[inline]
    pub unsafe fn get(&self, name: &str) -> Sel {
        // `Relaxed` should be fine since `sel_registerName` is thread-safe.
        let ptr = self.ptr.load(Ordering::Relaxed);
        if let Some(sel) = unsafe { Sel::from_ptr(ptr) } {
            sel
        } else {
            // SAFETY: Checked by caller
            unsafe { self.fetch(name.as_ptr().cast()) }
        }
    }
}

/// Allows storing a [`AnyClass`] reference in a static and lazily loading it.
#[derive(Debug)]
pub struct CachedClass {
    ptr: AtomicPtr<AnyClass>,
}

impl CachedClass {
    /// Constructs a new [`CachedClass`].
    #[allow(clippy::new_without_default)]
    pub const fn new() -> CachedClass {
        CachedClass {
            ptr: AtomicPtr::new(ptr::null_mut()),
        }
    }

    // Mark as cold since this should only ever be called once (or maybe twice
    // if running on multiple threads).
    #[cold]
    #[track_caller]
    unsafe fn fetch(&self, name: *const c_char) -> &'static AnyClass {
        let ptr: *const AnyClass = unsafe { ffi::objc_getClass(name) }.cast();
        self.ptr.store(ptr as *mut AnyClass, Ordering::Relaxed);
        if let Some(cls) = unsafe { ptr.as_ref() } {
            cls
        } else {
            // Recover the name from the pointer. We do it like this so that
            // we don't have to pass the length of the class to this method,
            // improving binary size.
            let name = unsafe { CStr::from_ptr(name) };
            let name = str::from_utf8(name.to_bytes()).unwrap();
            panic!("class {name} could not be found")
        }
    }

    /// Returns the cached class. If no class is yet cached, gets one with
    /// the given name and stores it.
    #[inline]
    #[track_caller]
    pub unsafe fn get(&self, name: &str) -> &'static AnyClass {
        // `Relaxed` should be fine since `objc_getClass` is thread-safe.
        let ptr = self.ptr.load(Ordering::Relaxed);
        if let Some(cls) = unsafe { ptr.as_ref() } {
            cls
        } else {
            // SAFETY: Checked by caller
            unsafe { self.fetch(name.as_ptr().cast()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::ffi::CStr;
    use core::sync::atomic::Ordering;

    use super::*;

    #[test]
    #[should_panic = "class NonExistentClass could not be found"]
    #[cfg(not(feature = "unstable-static-class"))]
    fn test_not_found() {
        let _ = crate::class!(NonExistentClass);
    }

    #[test]
    fn null_selector_sentinel_is_cstr_and_none_is_not_cached() {
        // Test-only path: do not call `Sel::register` (that `expect`s).
        let cache = CachedSel::new();
        let sel = cache.cache_registered(None);
        assert!(cache.ptr.load(Ordering::Relaxed).is_null());
        assert!(sel.is_skip_sentinel());

        // `CStr::from_ptr` stops at the first NUL. One zero byte is enough.
        let from_ptr = unsafe { CStr::from_ptr(sel.as_ptr().cast()) };
        assert_eq!(from_ptr.to_bytes(), b"");
        assert_eq!(from_ptr.to_bytes_with_nul(), b"\0");
        assert_eq!(sel.name().to_bytes(), b"");

        let again = cache.cache_registered(None);
        assert!(again.is_skip_sentinel());
        assert!(cache.ptr.load(Ordering::Relaxed).is_null());
    }
}
