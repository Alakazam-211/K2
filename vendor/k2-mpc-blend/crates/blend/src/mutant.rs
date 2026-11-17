//! Planted bugs, for the rows that prove the checks can fail (`tests/mutants.rs`), and one
//! test-only setting, [`gap`]. Without the test-only `mutants` feature, [`active`] is a constant
//! `false`, [`gap`] a constant `None`, and nothing else here exists.

#[cfg(feature = "mutants")]
mod on {
    use std::cell::Cell;

    thread_local! {
        static ACTIVE: Cell<Option<&'static str>> = const { Cell::new(None) };
        static GAP: Cell<Option<u16>> = const { Cell::new(None) };
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

    /// Runs `f` with the writer's newest version set back to `written_to` on this thread
    /// ([`crate::write::written_to`]): a gap between the releases read and written.
    pub fn in_gap<R>(written_to: u16, f: impl FnOnce() -> R) -> R {
        let was = GAP.with(|g| g.replace(Some(written_to)));
        let out = f();
        GAP.with(|g| g.set(was));
        out
    }

    /// The writer's newest version as [`in_gap`] sets it back on this thread, if it does.
    pub fn gap() -> Option<u16> {
        GAP.with(Cell::get)
    }
}

#[cfg(feature = "mutants")]
pub use on::{active, gap, in_gap, with};

/// Whether the mutant `name` is planted: never, without the `mutants` feature.
#[cfg(not(feature = "mutants"))]
#[inline]
pub fn active(_name: &str) -> bool {
    false
}

/// The writer's newest version as a row sets it back: never, without the `mutants` feature.
#[cfg(not(feature = "mutants"))]
#[inline]
pub fn gap() -> Option<u16> {
    None
}
