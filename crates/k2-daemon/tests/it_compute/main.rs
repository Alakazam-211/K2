//! k2-daemon integration tests: Compute: jobs, nodes, placement and CI.
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

mod ci_integration;
mod ci_preview_gate_integration;
mod compute_admin_integration;
#[cfg(unix)]
mod compute_admin_node_e2e;
mod compute_git_integration;
#[cfg(unix)]
mod compute_integration;
mod compute_lane_secrets_integration;
mod compute_manage_integration;
#[cfg(target_os = "linux")]
mod compute_pkg_node_e2e;
mod compute_place_integration;
mod compute_receipt_v2_integration;
#[cfg(unix)]
mod compute_results_integration;
mod compute_secret_integration;
mod compute_shared_integration;
