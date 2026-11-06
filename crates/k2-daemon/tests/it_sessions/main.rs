//! k2-daemon integration tests: Sessions, the reaper and spawn.
//!
//! One test binary per area. Each module here was its own test binary
//! (`tests/<module>.rs`) and keeps that name, so `<module>::` selects
//! exactly the tests that binary held. Run it with cargo-nextest, which
//! gives every test its own process; see `tests/support/env.rs`.

#[path = "../support"]
mod support {
    pub mod env;
    pub mod app_fixture;
}

#[test]
fn runner_guard() {
    support::env::require_nextest();
}

mod reaper_close_guard_integration;
mod spawn_fail_keeps_pin_integration;
mod unit_writers_killmode;
