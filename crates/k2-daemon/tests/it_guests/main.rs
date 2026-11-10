//! k2-daemon integration tests: Guests, rooms, hubs and live access.
//!
//! One test binary per area. Each module here was its own test binary
//! (`tests/<module>.rs`) and keeps that name, so `<module>::` selects
//! exactly the tests that binary held. Run it with cargo-nextest, which
//! gives every test its own process; see `tests/support/env.rs`.

#[path = "../support"]
mod support {
    pub mod env;
    pub mod app_fixture;
    pub mod fake_sew;
    pub mod live_sockets;
}

#[test]
fn runner_guard() {
    support::env::require_nextest();
}

mod guest_action_cards_integration;
mod guest_no_host_path_integration;
mod guest_read_state_integration;
mod guest_rooms_integration;
mod guest_viewing_integration;
mod hub_viewing_integration;
mod live_access_events_integration;
mod live_access_hub_integration;
mod live_access_integration;
mod must_change_password_integration;
mod presence_integration;
mod presence_kick_integration;
mod room_surfaces_integration;
mod viewer_claimer_integration;
