//! k2-daemon integration tests: Zen and the home avatars.
//!
//! One test binary per area. Each module here was its own test binary
//! (`tests/<module>.rs`) and keeps that name, so `<module>::` selects
//! exactly the tests that binary held. Run it with cargo-nextest, which
//! gives every test its own process; see `tests/support/env.rs`.

#[path = "../support"]
mod support {
    pub mod env;
}

#[test]
fn runner_guard() {
    support::env::require_nextest();
}

mod home_avatars_core;
mod home_avatars_headless;
mod home_two_daemons_integration;
mod zen_columns;
mod zen_core;
mod zen_headless_integration;
mod zen_widget_shot_integration;
mod zen_widgets_headless;
