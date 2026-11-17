//! The kit's edit check on the live pack (`BlendLive`'s `Pack::check_edit`): each kind of change
//! the live rules refuse, a person's edit made on the live doc directly, an undo and a restore
//! that would put one in, are refused at the call, typed (the kit's `pack_refused`, `MPC-E823`,
//! with the pack's code), nothing moving and every read Ok; a change the rules take commits and
//! is written back as the pack's hook writes it. The rows are in `edit_hook_support/mod.rs`,
//! shared with `tests/newer.rs` and the planted bugs (`tests/mutants.rs`).

mod edit_hook_support;
mod live_support;

use edit_hook_support as rows;

fn pass(row: fn() -> Result<(), String>) {
    // one line naming the row and its step, so a log shows where it failed
    if let Err(e) = row() {
        panic!("row failed at its step: {}", e.replace('\n', " | "));
    }
}

#[test]
fn each_rule_refuses_at_the_call() {
    pass(rows::row_hook_refuses_each_rule_at_the_call);
}

#[test]
fn an_undo_or_a_restore_is_refused_at_the_call() {
    pass(rows::row_hook_refuses_undo_and_restore);
}

#[test]
fn what_live_writes_commits_and_writes_back() {
    pass(rows::row_hook_takes_what_live_writes);
}
