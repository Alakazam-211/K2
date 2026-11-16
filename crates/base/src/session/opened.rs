//! **The version a person's app opened** (`note_opened`): the host tells the kit which version
//! of a doc an app is editing, so a save from that app can later merge from the version it was
//! made on (`docs/SESSIONS.md`, "The version an app opened"). One pin per doc, the last call's;
//! kept in the store with the doc's sync state, so a restart keeps it; checked at every use
//! (its version must still be held), and dropped when it fails.

use crate::store::OpenedRec;

/// What a host says the app opened ([`crate::session::Session::note_opened`],
/// [`crate::session::BlobSession::note_opened`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenedAt {
    /// The file as it is on disk now (the usual call, right before the app reads it): the kit
    /// first checks the file (an outside save it hasn't seen is taken in) and writes back what
    /// the file doesn't hold yet, then pins the version the file holds, and holds its next
    /// write-backs to the file for [`crate::session::Config::opened_hold_ms`] (until the first
    /// outside save, at most), so the app reads that version.
    FileNow,
    /// A version the host handed the app itself: a live doc's change seq, a blob doc's version
    /// id.
    Version(u64),
}

/// How a pin was set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenedVia {
    /// [`OpenedAt::FileNow`].
    FileNow,
    /// [`OpenedAt::Version`].
    Version,
    /// An outside save taken in: the app's buffer is its own save now.
    OutsideSave,
}

/// The doc's pin: the version the app opened, when it was noted (host clock, ms), and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Opened {
    pub version: u64,
    pub at_ms: u64,
    pub via: OpenedVia,
}

impl OpenedVia {
    pub(crate) fn code(self) -> u8 {
        match self {
            OpenedVia::FileNow => 0,
            OpenedVia::Version => 1,
            OpenedVia::OutsideSave => 2,
        }
    }

    pub(crate) fn of(code: u8) -> Self {
        match code {
            0 => OpenedVia::FileNow,
            1 => OpenedVia::Version,
            _ => OpenedVia::OutsideSave,
        }
    }
}

impl Opened {
    pub(crate) fn of_rec(r: &OpenedRec) -> Self {
        Opened {
            version: r.version,
            at_ms: r.at_ms,
            via: OpenedVia::of(r.via),
        }
    }
}

/// Whether the pin is planted as kept in memory only (test-only).
pub(crate) fn planted_in_memory() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::OpenedInMemory)
    )
}

/// Whether the pin is planted as never checked at use (test-only).
pub(crate) fn planted_unchecked() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::PinUnchecked)
    )
}

/// Whether the pin is planted as never moved by an outside save (test-only).
pub(crate) fn planted_never_moves() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::PinNeverMoves)
    )
}

/// Whether `FileNow` is planted as pinning without checking the file first (test-only).
pub(crate) fn planted_skips_check() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::FileNowSkipsCheck)
    )
}

/// Whether a save is planted as merging as if there were no pin (test-only).
pub(crate) fn planted_ignored() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::OpenedIgnored)
    )
}

/// Whether a keyed doc's field merge is planted as taking a clash as the save's value
/// (test-only).
pub(crate) fn planted_clash_merged() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::ClashMerged)
    )
}

/// Whether a keyed doc's field merge is planted as unchecked by the pack (test-only).
pub(crate) fn planted_view_check_skipped() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::ViewCheckSkipped)
    )
}

/// Whether the write-back hold is planted as ignored (test-only).
pub(crate) fn planted_hold_ignored() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::HoldIgnored)
    )
}

/// Whether a text doc's outside save is planted as merging from the synced text though a pin
/// is noted (test-only).
pub(crate) fn planted_text_base_synced() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::TextBaseSynced)
    )
}

/// Whether a text doc's outside save merged from the opened version is planted as reporting
/// no kept pairs (test-only).
pub(crate) fn planted_kept_pairs_dropped() -> bool {
    plant_on!(
        crate::differential::opened_mutant()
            == Some(crate::differential::OpenedMutant::KeptPairsDropped)
    )
}
