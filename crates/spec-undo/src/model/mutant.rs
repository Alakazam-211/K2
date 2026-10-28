//! The model check's mutants (spec §8 Mutants): wrong variants of the oracle that the
//! model check must catch, the same set as in `spec/quint/undo.qnt` (`MUTANT`).
//!
//! Test-only: without the `mutants` feature [`active`] is always [`Mutant::None`] and no
//! mutant can be set, so the oracle of record is never a mutant. With it, one mutant is
//! active process-wide ([`set`]); the enumerator (`undo_enum --mutant`) sets it once,
//! before any thread starts.

/// One wrong variant (spec §8's numbering).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mutant {
    /// The oracle of record.
    None,
    /// (i) Whole-text merge3 with no taint rule: a tainted region is merged like any other
    /// (U12's whole-text form, which equals the window form outside the taint case).
    NoTaint,
    /// (iii) `--partial` reports a different set than all or nothing: it leaves tainted
    /// regions out of what it reports (they are still skipped).
    PartialHidesTaint,
    /// (iv) Redo re-tries skipped regions: a partial undo's redo entry also carries the
    /// regions it skipped (carried through the step, before and after swapped).
    RedoRetriesSkipped,
    /// (vi) O5 at every window end (`at_end` always true).
    O5EveryWindowEnd,
    /// (vii) O5 conditioned on the region's base instead of the whole doc's (`at_end` =
    /// the region ends at the doc's end and its own base is non-empty).
    O5RegionBase,
    /// skip-U19: a purge doesn't clear the stacks.
    SkipU19,
}

impl Mutant {
    /// Every mutant, by its command-line name.
    pub const ALL: [(&'static str, Mutant); 6] = [
        ("i-no-taint", Mutant::NoTaint),
        ("iii-partial-hides-taint", Mutant::PartialHidesTaint),
        ("iv-redo-retries-skipped", Mutant::RedoRetriesSkipped),
        ("vi-o5-every-window-end", Mutant::O5EveryWindowEnd),
        ("vii-o5-region-base", Mutant::O5RegionBase),
        ("skip-u19", Mutant::SkipU19),
    ];

    /// A mutant by its command-line name (`none` is the oracle).
    pub fn by_name(name: &str) -> Option<Mutant> {
        if name == "none" {
            return Some(Mutant::None);
        }
        Self::ALL.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
    }

    /// Its command-line name.
    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, m)| *m == self)
            .map_or("none", |(n, _)| n)
    }
}

#[cfg(feature = "mutants")]
static ACTIVE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The active mutant ([`Mutant::None`] unless the `mutants` feature set one).
#[cfg(feature = "mutants")]
pub fn active() -> Mutant {
    match ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => Mutant::None,
        k => Mutant::ALL[k as usize - 1].1,
    }
}

/// The active mutant: always [`Mutant::None`] without the `mutants` feature.
#[cfg(not(feature = "mutants"))]
#[inline]
pub fn active() -> Mutant {
    Mutant::None
}

/// Make `m` the active mutant, process-wide (test builds with the `mutants` feature only).
#[cfg(feature = "mutants")]
pub fn set(m: Mutant) {
    let k = Mutant::ALL
        .iter()
        .position(|(_, x)| *x == m)
        .map_or(0, |i| i as u8 + 1);
    ACTIVE.store(k, std::sync::atomic::Ordering::Relaxed);
}
