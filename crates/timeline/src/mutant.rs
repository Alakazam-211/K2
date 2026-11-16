//! Planted bugs, for the rows that prove the checks can fail (`tests/mutants.rs`). Without the
//! test-only `mutants` feature, [`active`] is a constant `false` and nothing else here exists.

#[cfg(feature = "mutants")]
mod on {
    use std::cell::Cell;

    thread_local! {
        static ACTIVE: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    /// Puts back the mutant planted before, when dropped: also when `f` panics.
    struct Restore(Option<&'static str>);

    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|a| a.set(self.0));
        }
    }

    /// Runs `f` with the mutant `name` planted on this thread; the one planted before comes back
    /// after, also when `f` panics.
    pub fn with<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
        let _restore = Restore(ACTIVE.with(|a| a.replace(Some(name))));
        f()
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
