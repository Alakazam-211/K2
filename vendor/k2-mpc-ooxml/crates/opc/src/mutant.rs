//! Planted bugs, for the tests that prove the checks can fail (`tests/mutants.rs`). Without
//! the `mutants` feature, [`active`] is a constant `false` and nothing here exists in a build.
//!
//! One switch serves this crate and the packs built on it: a pack's own `mutant` module
//! re-exports `with` and [`active`], so a mutant planted through the pack reaches the
//! package layer too (a pack's `mutants` feature turns on this crate's).

#[cfg(feature = "mutants")]
thread_local! {
    static ACTIVE: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
}

/// Run `f` with the mutant `name` planted on this thread.
#[cfg(feature = "mutants")]
pub fn with<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    ACTIVE.with(|a| a.set(name));
    let out = f();
    ACTIVE.with(|a| a.set(""));
    out
}

/// Whether the mutant `name` is planted on this thread. Always `false` without the feature.
#[doc(hidden)]
#[inline]
pub fn active(name: &str) -> bool {
    #[cfg(feature = "mutants")]
    {
        ACTIVE.with(|a| a.get() == name)
    }
    #[cfg(not(feature = "mutants"))]
    {
        let _ = name;
        false
    }
}
