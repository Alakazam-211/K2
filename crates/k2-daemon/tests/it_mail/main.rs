//! k2-daemon integration tests: Mail and notifications.
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

#[cfg(unix)]
mod cell_server_uds_integration;
mod email_notify_live;
mod mail_manage_integration;
