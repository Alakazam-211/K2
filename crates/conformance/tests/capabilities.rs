//! The capability rules on every engine that has the capability, one test per rule and
//! engine. Each engine's capabilities here must equal the coverage matrix's
//! (`caps_match_the_matrix`); the text rules are `tests/text.rs`. Last, the planted
//! negatives of the purge rules' byte scans (`planted_negatives`) and of the span purge's
//! extent (`span_mutants`).

use k2_mpc_conformance::kit_history::KitHistory;
use k2_mpc_conformance::kit_undo::KitUndo;
use k2_mpc_conformance::matrix::{self, Cap};
use k2_mpc_conformance::model::{ModelHistory, ModelText};
use k2_mpc_conformance::yrs::YrsText;
use k2_mpc_conformance::{anchors, blame, branches, erasure, events, purge, span, undo, versions};

/// One test per rule of a capability (run by `$module::$run`), and a check that they are
/// all of its rules (`$module::$all`).
macro_rules! rules {
    ($module:ident::$run:ident, $all:ident, $ty:ty; $($name:ident = $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                $module::$run::<$ty>($id);
            }
        )*

        #[test]
        fn every_rule_has_a_test() {
            let mut tested = vec![$($id),*];
            let mut all = $module::$all.to_vec();
            tested.sort_unstable();
            all.sort_unstable();
            assert_eq!(
                tested,
                all,
                "{}::{} and its tests differ",
                stringify!($module),
                stringify!($all)
            );
        }
    };
}

/// An engine's capability suites, and the check that they are the matrix's capabilities.
macro_rules! engine_suite {
    ($info:expr; $($cap:ident: $ty:ty),* $(,)?) => {
        $( engine_suite!(@suite $cap $ty); )*

        #[test]
        fn caps_match_the_matrix() {
            // Text runs on every engine (tests/text.rs).
            let mut ran = vec![Cap::Text $(, engine_suite!(@cap $cap))*];
            let mut declared = $info.caps.to_vec();
            ran.sort();
            declared.sort();
            assert_eq!(ran, declared, "{}: the capabilities tested here and the matrix's differ", $info.name);
        }
    };
    (@cap events) => { Cap::Events };
    (@cap blame) => { Cap::Blame };
    (@cap anchors) => { Cap::Anchors };
    (@cap undo) => { Cap::Undo };
    (@cap versions) => { Cap::Versions };
    (@cap history_purge) => { Cap::HistoryPurge };
    (@cap erasure) => { Cap::Erasure };
    (@cap branches) => { Cap::Branches };
    (@cap branch_at) => { Cap::BranchAt };
    (@cap purge) => { Cap::Purge };
    (@cap blame_at) => { Cap::BlameAt };
    (@cap span_ids) => { Cap::SpanIds };
    (@suite events $ty:ty) => {
        mod events_rules {
            use super::*;
            rules!(events::run, RULES, $ty;
                core_idempotent = "core.idempotent",
                core_causal = "core.causal",
                core_no_equivocation = "core.no_equivocation",
                text_edit_atomic_split = "text.edit_atomic_split",
            );
        }
    };
    (@suite blame $ty:ty) => {
        mod blame_rules {
            use super::*;
            rules!(blame::run, RULES, $ty; text_blame_char = "text.blame_char",);
        }
    };
    (@suite anchors $ty:ty) => {
        mod anchors_rules {
            use super::*;
            rules!(anchors::run, RULES, $ty; text_cursor_stable = "text.cursor_stable",);
        }
    };
    (@suite undo $ty:ty) => {
        mod undo_rules {
            use super::*;
            rules!(undo::run, RULES, $ty;
                hist_undo_atomic = "hist.undo_atomic",
                hist_undo_partial = "hist.undo_partial",
                hist_redo = "hist.redo",
                hist_undo_own_only = "hist.undo_own_only",
            );
        }
    };
    (@suite versions $ty:ty) => {
        mod versions_rules {
            use super::*;
            rules!(versions::run, RULES, $ty;
                hist_version_stable = "hist.version_stable",
                hist_restore_forward = "hist.restore_forward",
                hist_restore_minimal = "hist.restore_minimal",
            );
        }
    };
    (@suite history_purge $ty:ty) => {
        mod history_purge_rules {
            use super::*;
            rules!(erasure::run_history, HISTORY_RULES, $ty;
                erase_versions = "erase.versions",
            );
        }
    };
    (@suite erasure $ty:ty) => {
        mod erasure_rules {
            use super::*;
            rules!(erasure::run_participant, PARTICIPANT_RULES, $ty;
                erase_participant = "erase.participant",
            );
        }
    };
    (@suite branches $ty:ty) => {
        mod branches_rules {
            use super::*;
            rules!(branches::run, RULES, $ty;
                hist_branch_merge = "hist.branch_merge",
            );
        }
    };
    (@suite branch_at $ty:ty) => {
        mod branch_at_rules {
            use super::*;
            rules!(branches::run_at, AT_RULES, $ty;
                hist_merge_any_base = "hist.merge_any_base",
            );
        }
    };
    (@suite purge $ty:ty) => {
        mod purge_rules {
            use super::*;
            rules!(purge::run, RULES, $ty;
                erase_purge = "erase.purge",
                erase_no_readmit = "erase.no_readmit",
            );
        }
    };
    (@suite span_ids $ty:ty) => {
        mod span_ids_rules {
            use super::*;
            rules!(span::run, RULES, $ty;
                erase_span_includes_tombstones = "erase.span_includes_tombstones",
            );
        }
    };
    (@suite blame_at $ty:ty) => {
        mod blame_at_rules {
            use super::*;
            rules!(blame::run_at, AT_RULES, $ty;
                hist_blame_grain = "hist.blame_grain",
            );
        }
    };
}

mod kit {
    use super::*;
    engine_suite!(matrix::KIT;
        events: YrsText,
        blame: YrsText,
        anchors: YrsText,
        undo: KitUndo,
        versions: KitHistory,
        history_purge: KitHistory,
        erasure: KitHistory,
        branches: YrsText,
        purge: YrsText,
    );
}

mod model {
    use super::*;
    engine_suite!(matrix::MODEL;
        events: ModelText,
        blame: ModelText,
        anchors: ModelText,
        versions: ModelHistory,
        history_purge: ModelHistory,
        branches: ModelText,
        branch_at: ModelText,
        purge: ModelText,
        blame_at: ModelText,
        span_ids: ModelText,
    );
}

/// The purge rules' byte scans catch a purge that keeps its content: on the model, with its
/// `PurgeRule::KeepContent` mutant (delete, keep the characters), every purge rule fails.
/// (On the kit, each rule's scan first finds the planted secret, which the rules assert.)
mod planted_negatives {
    use super::*;
    use k2_mpc_conformance::model::with_keep_content_mutant;

    fn caught(id: &str, run: impl FnOnce() + std::panic::UnwindSafe) {
        let r = with_keep_content_mutant(|| std::panic::catch_unwind(run));
        assert!(r.is_err(), "{id}: a purge that keeps its content passed");
    }

    #[test]
    fn erase_purge() {
        caught("erase.purge", || purge::run::<ModelText>("erase.purge"));
    }

    #[test]
    fn erase_no_readmit() {
        caught("erase.no_readmit", || {
            purge::run::<ModelText>("erase.no_readmit")
        });
    }

    #[test]
    fn erase_versions() {
        caught("erase.versions", || {
            erasure::run_history::<ModelHistory>("erase.versions")
        });
    }

    #[test]
    fn erase_span_includes_tombstones() {
        caught("erase.span_includes_tombstones", || {
            span::run::<ModelText>("erase.span_includes_tombstones")
        });
    }

    #[test]
    fn hist_blame_grain() {
        // blame_at hides purged characters by their erased content: the mutant keeps it
        caught("hist.blame_grain", || {
            blame::run_at::<ModelText>("hist.blame_grain")
        });
    }
}

/// `erase.span_includes_tombstones` catches a span purge with the wrong extent: on the
/// model, with its span mutants (visible elements only: the fixed typo keeps its content;
/// the tombstones against either end too: the deleted copies are erased), the rule fails.
mod span_mutants {
    use super::*;
    use k2_mpc_conformance::model::{with_span_mutant, SpanMutant};

    fn caught(m: SpanMutant) {
        let r = with_span_mutant(m, || {
            std::panic::catch_unwind(|| span::run::<ModelText>("erase.span_includes_tombstones"))
        });
        assert!(r.is_err(), "a span purge with {m:?} passed");
    }

    #[test]
    fn visible_only() {
        caught(SpanMutant::VisibleOnly);
    }

    #[test]
    fn with_neighbours() {
        caught(SpanMutant::WithNeighbours);
    }
}
