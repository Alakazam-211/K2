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

mod active_reaper_integration;
mod activity_events_integration;
mod activity_heartbeat_lock_integration;
mod activity_transcript_integration;
mod agents_routes_integration;
mod attach_size_integration;
mod awareness_ws_integration;
mod boot_status_views_integration;
mod boot_trash_hang_integration;
mod canonical_session_integration;
mod daemon_port_stability_integration;
mod grid_emitter_integration;
mod heartbeat_creation_traps_integration;
mod heartbeat_daemon_tick_integration;
mod heartbeat_fire_v2_integration;
mod heartbeat_lease_watchdog_integration;
mod heartbeat_next_fire_cli_integration;
mod heartbeat_port_claim_integration;
mod heartbeat_single_flight_integration;
mod heartbeat_stale_restart_integration;
mod heartbeat_use_workspace_session_integration;
mod heartbeat_wake_pointer_integration;
mod host_respond_integration;
mod host_sessions_integration;
mod legacy_agent_lock_integration;
mod lifecycle_ws_integration;
mod pin_size_integration;
mod pinned_chat_ensure_integration;
mod reaper_close_guard_integration;
mod remote_session_layer0_integration;
mod remote_session_shell_integration;
mod sidecar_cli_integration;
#[cfg(unix)]
mod sidecar_worktree_integration;
mod spawn_fail_keeps_pin_integration;
mod splice_ingress_integration;
mod test_daemon_runtime_isolation;
mod transport_truncation_integration;
mod triage_integration;
mod unit_writers_killmode;
mod v2_spawn_bare_tab_cap;
mod web_client_layer0_integration;
