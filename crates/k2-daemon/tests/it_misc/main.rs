//! k2-daemon integration tests: Everything else.
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

mod folder_trust_integration;
mod git_tools_integration;
mod grokbots_routes_integration;
mod macos_daemon_helper_app_layout;
