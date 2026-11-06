//! k2-daemon integration tests: Apps: the App runtime, App users, App files and the gateway.
//!
//! One test binary per area. Each module here was its own test binary
//! (`tests/<module>.rs`) and keeps that name, so `<module>::` selects
//! exactly the tests that binary held. Run it with cargo-nextest, which
//! gives every test its own process; see `tests/support/env.rs`.

#[path = "../support"]
mod support {
    pub mod env;
    pub mod app_fixture;
    #[cfg(unix)]
    pub mod fake_sew;
}

#[test]
fn runner_guard() {
    support::env::require_nextest();
}

mod agent_app_users_integration;
mod agent_unread_tickets_integration;
mod app_caps_cards_activity_integration;
mod app_caps_no_widening_integration;
mod app_chat_key_integration;
mod app_files_download_integration;
mod app_files_drop_integration;
mod app_front_websocket_integration;
mod app_governed_spaces_integration;
mod app_grace_integration;
mod app_handover_integration;
mod app_mobile_access_integration;
mod app_move_polish_integration;
mod app_pairing_integration;
mod app_project_chat_integration;
mod app_selfmove_integration;
mod app_sew_routes_reachable_integration;
mod app_user_profile_integration;
mod app_users_end_to_end_integration;
mod app_users_switch_integration;
mod apps_custom_domain_external_integration;
mod apps_gateway_body_integration;
mod apps_held_notes_integration;
mod apps_sew_template_integration;
mod apps_static_integration;
mod connections_resolve_integration;
mod fs_untitled_integration;
mod publish_stop_start_integration;
mod sew_for_apps_integration;
