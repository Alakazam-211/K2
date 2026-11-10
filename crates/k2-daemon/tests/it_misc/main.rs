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

mod airgap_lan_integration;
mod clone_routes_integration;
mod connect_gap_routes_integration;
mod connect_proxy_integration;
mod context_catalog_create_integration;
mod custom_preset_agent_integration;
mod db_upgrade_switch_integration;
mod federation_peer_rosters_integration;
mod folder_trust_integration;
mod git_tools_integration;
mod grokbots_routes_integration;
#[cfg(unix)]
mod homes_sync_cell_integration;
mod homes_sync_integration;
mod keep_alive_power_status_integration;
#[cfg(windows)]
mod keep_alive_windows_integration;
mod keep_awake_routes_integration;
mod llm_account_for_integration;
#[cfg(unix)]
mod llm_accounts_integration;
mod login_lock_clear;
mod macos_daemon_helper_app_layout;
mod ops_routes_integration;
mod ops_stream_ws_integration;
mod power_helper_route_integration;
mod preset_routes_integration;
mod provision_users_integration;
mod push_log_cli_integration;
mod stash_sync_integration;
mod tunnel_disable_unpair_integration;
mod tunnel_subdomain_switch_integration;
mod v1_agent_hire_integration;
mod vault_run_helper_integration;
mod workspace_layouts_revision_integration;
