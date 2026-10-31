//! The structured rules (`mlist.*`, `map.*`, `table.*`, `stream.*`) on every engine that has
//! their capabilities, one test per rule and engine; the check that each engine's structured
//! capabilities here are the matrix's; then the planted negatives: every rule fails on its
//! mutant adapter ([`k2_mpc_conformance::mutants`]), and every pinned rule's conforming check
//! fails on the engine that deviates from it.
//!
//! The binary counts heap per thread (`map.steady_bounded`).

use std::panic::{catch_unwind, AssertUnwindSafe};

use k2_mpc_conformance::matrix::{self, Cap};
use k2_mpc_conformance::mutants::{with_mutant, Mutant, Mutated};
use k2_mpc_conformance::yrs::YrsGrid;
use k2_mpc_conformance::{map, mlist, stream, table};

#[global_allocator]
static A: k2_mpc_conformance::heap::Counting = k2_mpc_conformance::heap::Counting;

/// One test per rule (run by `$run`), and a check that they are all of `$all`.
macro_rules! rules {
    ($module:ident :: $run:ident, $all:expr, $ty:ty; $($name:ident = $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                $module::$run::<$ty>($id);
            }
        )*

        #[test]
        fn every_rule_has_a_test() {
            let mut tested = vec![$($id),*];
            let mut all: Vec<&str> = $all;
            tested.sort_unstable();
            all.sort_unstable();
            assert_eq!(tested, all, "{} and its tests differ", stringify!($all));
        }
    };
}

mod kit {
    use super::*;

    #[test]
    fn caps_match_the_matrix() {
        let mut ran = vec![Cap::MList, Cap::Map, Cap::Table, Cap::Merged];
        let mut declared: Vec<Cap> = matrix::KIT
            .caps
            .iter()
            .copied()
            .filter(|c| c.is_structured())
            .collect();
        ran.sort();
        declared.sort();
        assert_eq!(
            ran, declared,
            "the kit's structured capabilities tested here and the matrix's differ"
        );
    }

    mod mlist_rules {
        use super::*;
        rules!(mlist::run, mlist::RULES.to_vec(), YrsGrid;
            mlist_converge = "mlist.converge",
            mlist_move_no_duplicate = "mlist.move_no_duplicate",
            mlist_delete_wins = "mlist.delete_wins",
            mlist_insert_beside_moved = "mlist.insert_beside_moved",
            mlist_move_and_set = "mlist.move_and_set",
            mlist_move_lww = "mlist.move_lww",
            mlist_block_contiguous = "mlist.block_contiguous",
            mlist_chain_one_run = "mlist.chain_one_run",
            mlist_yata_separating_case = "mlist.yata_separating_case",
        );
    }

    mod map_rules {
        use super::*;
        rules!(map::run, map::RULES.to_vec(), YrsGrid;
            map_lww = "map.lww",
            map_delete_vs_set = "map.delete_vs_set",
            map_steady_bounded = "map.steady_bounded",
        );
    }

    mod table_rules {
        use super::*;
        rules!(table::run, table::RULES.iter().copied().filter(|r| !table::MERGED_RULES.contains(r)).collect(), YrsGrid;
            table_cell_kept = "table.cell_kept",
            table_block_contiguous = "table.block_contiguous",
            table_cell_lww = "table.cell_lww",
            table_row_identity = "table.row_identity",
            table_sort_vs_cell = "table.sort_vs_cell",
            table_row_move_keeps_cells = "table.row_move_keeps_cells",
            table_col_move_keeps_cells = "table.col_move_keeps_cells",
            table_col_move_no_duplicate = "table.col_move_no_duplicate",
            table_delete_wins_restorable = "table.delete_wins_restorable",
            table_insert_beside_moved_row = "table.insert_beside_moved_row",
            table_ragged_render = "table.ragged_render",
        );
    }

    mod table_merged_rules {
        use super::*;
        rules!(table::run_merged, table::MERGED_RULES.to_vec(), YrsGrid;
            table_cell_vs_row_delete = "table.cell_vs_row_delete",
            table_cell_vs_col_delete = "table.cell_vs_col_delete",
            table_sort_atomic = "table.sort_atomic",
        );
    }

    mod stream_rules {
        use super::*;
        rules!(stream::run, stream::RULES.to_vec(), YrsGrid;
            stream_merged_replays = "stream.merged_replays",
            stream_merged_matches_local = "stream.merged_matches_local",
            stream_merged_writes = "stream.merged_writes",
        );
    }
}

/// The reference model has no structured capability yet: every structured rule awaits it.
#[test]
fn model_awaits_every_structured_rule() {
    assert!(
        !matrix::MODEL.caps.iter().any(|c| c.is_structured()),
        "the model claims a structured capability: add its suite here"
    );
    for r in matrix::structured_rules() {
        assert!(
            matrix::AWAITING
                .iter()
                .any(|a| a.rule == r.id && a.engine == matrix::MODEL.name),
            "{} on the model: not awaiting",
            r.id
        );
    }
}

#[test]
fn native_entries_cover_the_structured_rules() {
    assert_eq!(matrix::native_problems(), Vec::<String>::new());
}

// ---------- planted negatives ----------

/// `f` must panic: the rule caught what it should.
fn caught(what: &str, f: impl FnOnce()) {
    let r = catch_unwind(AssertUnwindSafe(f));
    assert!(
        r.is_err(),
        "{what}: the rule passed; its check doesn't bite"
    );
}

fn caught_by(m: Mutant, rule: &str, f: impl FnOnce()) {
    caught(&format!("{rule} on mutant {m:?}"), || with_mutant(m, f));
}

/// Every rule's mutant, and the mutant passes the rules it doesn't target (so a failure is
/// the targeted semantic, not a broken adapter).
mod mutants {
    use super::*;

    #[test]
    fn mlist_converge() {
        caught_by(Mutant::MoveAsReinsert, "mlist.converge", || {
            mlist::run::<Mutated>("mlist.converge")
        });
    }

    #[test]
    fn mlist_move_no_duplicate() {
        caught_by(Mutant::MoveAsReinsert, "mlist.move_no_duplicate", || {
            mlist::run::<Mutated>("mlist.move_no_duplicate")
        });
    }

    #[test]
    fn mlist_delete_wins() {
        caught_by(Mutant::FlagLww, "mlist.delete_wins", || {
            mlist::run::<Mutated>("mlist.delete_wins")
        });
    }

    #[test]
    fn mlist_insert_beside_moved() {
        caught_by(
            Mutant::InsertFollowsAnchor,
            "mlist.insert_beside_moved",
            || mlist::run::<Mutated>("mlist.insert_beside_moved"),
        );
    }

    #[test]
    fn mlist_move_and_set() {
        caught_by(Mutant::MoveAsReinsert, "mlist.move_and_set", || {
            mlist::run::<Mutated>("mlist.move_and_set")
        });
    }

    #[test]
    fn mlist_block_contiguous() {
        caught_by(Mutant::Fractional, "mlist.block_contiguous", || {
            mlist::run::<Mutated>("mlist.block_contiguous")
        });
    }

    #[test]
    fn mlist_chain_one_run() {
        caught_by(Mutant::ReverseChains, "mlist.chain_one_run", || {
            mlist::run::<Mutated>("mlist.chain_one_run")
        });
    }

    #[test]
    fn table_cell_kept() {
        caught_by(Mutant::LineDeleteDropsCells, "table.cell_kept", || {
            table::run::<Mutated>("table.cell_kept")
        });
    }

    #[test]
    fn table_cell_vs_row_delete() {
        for m in [Mutant::LineDeleteDropsCells, Mutant::MergedNoMap] {
            caught_by(m, "table.cell_vs_row_delete", || {
                table::run_merged::<Mutated>("table.cell_vs_row_delete")
            });
        }
    }

    #[test]
    fn table_cell_vs_col_delete() {
        for m in [Mutant::LineDeleteDropsCells, Mutant::MergedNoMap] {
            caught_by(m, "table.cell_vs_col_delete", || {
                table::run_merged::<Mutated>("table.cell_vs_col_delete")
            });
        }
    }

    #[test]
    fn table_sort_atomic() {
        caught_by(Mutant::SortAsSteps, "table.sort_atomic", || {
            table::run_merged::<Mutated>("table.sort_atomic")
        });
    }

    #[test]
    fn table_block_contiguous() {
        caught_by(Mutant::Fractional, "table.block_contiguous", || {
            table::run::<Mutated>("table.block_contiguous")
        });
    }

    #[test]
    fn table_row_identity() {
        caught_by(Mutant::CellsByIndex, "table.row_identity", || {
            table::run::<Mutated>("table.row_identity")
        });
    }

    #[test]
    fn table_sort_vs_cell() {
        caught_by(Mutant::CellsByIndex, "table.sort_vs_cell", || {
            table::run::<Mutated>("table.sort_vs_cell")
        });
    }

    #[test]
    fn table_row_move_keeps_cells() {
        caught_by(Mutant::CellsByIndex, "table.row_move_keeps_cells", || {
            table::run::<Mutated>("table.row_move_keeps_cells")
        });
    }

    #[test]
    fn table_col_move_keeps_cells() {
        caught_by(Mutant::CellsByIndex, "table.col_move_keeps_cells", || {
            table::run::<Mutated>("table.col_move_keeps_cells")
        });
    }

    #[test]
    fn table_col_move_no_duplicate() {
        caught_by(
            Mutant::MoveAsReinsert,
            "table.col_move_no_duplicate",
            || table::run::<Mutated>("table.col_move_no_duplicate"),
        );
    }

    #[test]
    fn table_delete_wins_restorable() {
        caught_by(Mutant::FlagLww, "table.delete_wins_restorable", || {
            table::run::<Mutated>("table.delete_wins_restorable")
        });
    }

    #[test]
    fn table_insert_beside_moved_row() {
        caught_by(
            Mutant::InsertFollowsAnchor,
            "table.insert_beside_moved_row",
            || table::run::<Mutated>("table.insert_beside_moved_row"),
        );
    }

    /// A planted renderer (absent cells get no field), on the kit's own replicas.
    #[test]
    fn table_ragged_render() {
        caught(
            "table.ragged_render with a renderer that drops absent cells",
            || {
                table::ragged_render_with::<YrsGrid>(
                    k2_mpc_conformance::mutants::render_absent_as_no_field,
                )
            },
        );
    }

    #[test]
    fn stream_merged_replays() {
        caught_by(Mutant::MergedNoMoves, "stream.merged_replays", || {
            stream::run::<Mutated>("stream.merged_replays")
        });
    }

    #[test]
    fn stream_merged_matches_local() {
        caught_by(Mutant::MergedNoMoves, "stream.merged_matches_local", || {
            stream::run::<Mutated>("stream.merged_matches_local")
        });
    }

    /// The control: each mutant passes a rule it doesn't target, so the adapter itself works.
    #[test]
    fn each_mutant_passes_an_untargeted_rule() {
        let controls: &[(Mutant, &str)] = &[
            (Mutant::MoveAsReinsert, "mlist.insert_beside_moved"),
            (Mutant::FlagLww, "mlist.move_no_duplicate"),
            (Mutant::InsertFollowsAnchor, "mlist.delete_wins"),
            (Mutant::ReverseChains, "mlist.move_no_duplicate"),
            (Mutant::Fractional, "mlist.move_no_duplicate"),
            (Mutant::LineDeleteDropsCells, "mlist.move_and_set"),
            (Mutant::CellsByIndex, "mlist.move_and_set"),
            (Mutant::SortAsSteps, "mlist.delete_wins"),
            (Mutant::MergedNoMoves, "table.cell_kept"),
            (Mutant::MergedNoMap, "mlist.converge"),
            (Mutant::MoveAsReinsert, "table.ragged_render"),
            (Mutant::FlagLww, "table.insert_beside_moved_row"),
            (Mutant::InsertFollowsAnchor, "table.delete_wins_restorable"),
        ];
        for &(m, rule) in controls {
            with_mutant(m, || {
                if rule.starts_with("mlist.") {
                    mlist::run::<Mutated>(rule)
                } else {
                    table::run::<Mutated>(rule)
                }
            });
        }
    }
}

/// A pinned rule's conforming check fails on the engine that deviates (the deviation is
/// real, and the check that a fixed engine must pass bites).
mod pinned {
    use super::*;

    #[test]
    fn mlist_move_lww() {
        caught("mlist.move_lww, strict, on Yrs", || {
            mlist::run_strict::<YrsGrid>("mlist.move_lww")
        });
    }

    #[test]
    fn mlist_yata_separating_case() {
        caught("mlist.yata_separating_case, strict, on Yrs", || {
            mlist::run_strict::<YrsGrid>("mlist.yata_separating_case")
        });
    }

    #[test]
    fn map_lww() {
        caught("map.lww, strict, on Yrs", || {
            map::run_strict::<YrsGrid>("map.lww")
        });
    }

    #[test]
    fn map_delete_vs_set() {
        caught("map.delete_vs_set, strict, on Yrs", || {
            map::run_strict::<YrsGrid>("map.delete_vs_set")
        });
    }

    #[test]
    fn map_steady_bounded() {
        caught("map.steady_bounded, strict, on Yrs", || {
            map::run_strict::<YrsGrid>("map.steady_bounded")
        });
    }

    #[test]
    fn table_cell_lww() {
        caught("table.cell_lww, strict, on Yrs", || {
            table::run_strict::<YrsGrid>("table.cell_lww")
        });
    }

    #[test]
    fn stream_merged_writes() {
        caught("stream.merged_writes, strict, on Yrs", || {
            stream::run_strict::<YrsGrid>("stream.merged_writes")
        });
    }

    /// Every structured deviation of the kit has its strict negative here.
    #[test]
    fn every_structured_deviation_has_a_strict_negative() {
        let tested = [
            "mlist.move_lww",
            "mlist.yata_separating_case",
            "map.lww",
            "map.delete_vs_set",
            "map.steady_bounded",
            "table.cell_lww",
            "stream.merged_writes",
        ];
        let structured: Vec<&str> = matrix::KIT
            .deviations
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| matrix::structured_rules().any(|r| r.id == *id))
            .collect();
        assert_eq!(structured, tested);
    }
}

/// Every structured rule has a planted negative: a mutant above, or its strict check.
#[test]
fn every_rule_has_a_negative() {
    let with_mutant: &[&str] = &[
        "mlist.converge",
        "mlist.move_no_duplicate",
        "mlist.delete_wins",
        "mlist.insert_beside_moved",
        "mlist.move_and_set",
        "mlist.block_contiguous",
        "mlist.chain_one_run",
        "table.cell_kept",
        "table.cell_vs_row_delete",
        "table.cell_vs_col_delete",
        "table.sort_atomic",
        "table.block_contiguous",
        "table.row_identity",
        "table.sort_vs_cell",
        "table.row_move_keeps_cells",
        "table.col_move_keeps_cells",
        "table.col_move_no_duplicate",
        "table.delete_wins_restorable",
        "table.insert_beside_moved_row",
        "table.ragged_render",
        "stream.merged_replays",
        "stream.merged_matches_local",
    ];
    for r in matrix::structured_rules() {
        let pinned = matrix::KIT.deviations.iter().any(|(id, _)| *id == r.id);
        assert!(
            with_mutant.contains(&r.id) || pinned,
            "{}: no planted negative",
            r.id
        );
    }
}
