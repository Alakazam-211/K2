//! Planted bugs, for the rows that prove the checks can fail (`tests/mutants.rs`). Without the
//! test-only `mutants` feature, [`active`] is a constant `false` and nothing else here exists.

#[cfg(feature = "mutants")]
mod on {
    use std::cell::Cell;

    thread_local! {
        static ACTIVE: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    /// Runs `f` with the mutant `name` planted on this thread.
    pub fn with<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
        let was = ACTIVE.with(|a| a.replace(Some(name)));
        let out = f();
        ACTIVE.with(|a| a.set(was));
        out
    }

    /// Whether the mutant `name` is planted on this thread.
    pub fn active(name: &str) -> bool {
        ACTIVE.with(|a| a.get() == Some(name))
    }
}

#[cfg(feature = "mutants")]
pub use on::{active, with};

/// Whether the mutant `name` is planted: never, without the `mutants` feature.
#[cfg(not(feature = "mutants"))]
#[inline]
pub fn active(_name: &str) -> bool {
    false
}
