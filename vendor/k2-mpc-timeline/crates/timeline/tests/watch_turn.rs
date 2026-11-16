//! The timeline sample's watched turn, pinned whole as a golden
//! (`tests/watch_turn_golden/trim_held_outside_save.json`): an agent holds a clip and edits it
//! (a trim, a move, a rename, off, a marker), an app's save is taken in, a second agent's trim of
//! the held clip is refused, the edits are written back and the hold is let go; per step, every
//! keyed watch frame and roster event a watching person got, as the kit gives them. The turn and
//! its rows are in `turn_support`, shared with the planted bugs (`tests/mutants.rs`, where the
//! recorder dropping the refusal frame must fail the golden row).
//!
//! Any difference from the golden fails, naming each path; a change made on purpose is
//! re-blessed with `MPC_BLESS_GOLDEN=1` (`kit_view_golden`, at its top).

mod live_support;
mod turn_support;

use turn_support::{
    row_turn_holds_each_frame_kind, row_turn_is_reproducible, row_turn_is_the_golden,
};

#[test]
fn the_watched_turn_is_the_golden() {
    if let Err(e) = row_turn_is_the_golden() {
        panic!("{e}");
    }
}

#[test]
fn the_watched_turn_is_the_same_on_every_run() {
    match row_turn_is_reproducible() {
        Ok(bytes) => println!("watch_turn: two runs, {bytes} bytes each, byte-identical"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn the_watched_turn_holds_each_frame_kind() {
    match row_turn_holds_each_frame_kind() {
        Ok(c) => println!(
            "watch_turn: {} steps, {} frames, {} roster events; {} ops_change ({} the held \
             trim), {} verb_refused, {} outside_save ingested, {} claim_started_elems, {} \
             claim_ended",
            c.steps,
            c.frames,
            c.roster_events,
            c.changes,
            c.held_trims,
            c.refusals,
            c.ingested,
            c.claim_started_elems,
            c.claim_ended
        ),
        Err(e) => panic!("{e}"),
    }
}
