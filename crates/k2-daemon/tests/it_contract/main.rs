//! k2-daemon integration tests: The route contract: dispatcher, refusals and route gates.
//!
//! One test binary per area. Each module here was its own test binary
//! (`tests/<module>.rs`) and keeps that name, so `<module>::` selects
//! exactly the tests that binary held. Run it with cargo-nextest, which
//! gives every test its own process; see `tests/support/env.rs`.

#[path = "../support"]
mod support {
    pub mod env;
    pub mod app_fixture;
    pub mod sew_ready;
}

#[test]
fn runner_guard() {
    support::env::require_nextest();
}

mod api_gate_integration;
mod app_api_integration;
mod contract_routes;
mod ensure_cli_route;
mod refusal_registry;
mod require_manage_refusal_integration;
mod users_admins_only_integration;
