//! k2-daemon integration tests: Thread, tickets, the inbox, chat passes, Sew and voice.
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
    pub mod sew_ready;
}

#[test]
fn runner_guard() {
    support::env::require_nextest();
}

mod card_dismiss_integration;
mod chat_pass_integration;
mod chat_pass_mint_integration;
mod chat_transcript_route_gone;
mod inbox_stream_integration;
#[cfg(unix)]
mod sew_hub_inject_integration;
mod sidecar_thread_delivery_integration;
mod stamp_strip_integration;
mod thread_activity_integration;
mod thread_adoption_integration;
mod thread_attachments_integration;
mod thread_catchup_integration;
mod thread_delta_attachments_integration;
mod thread_delta_integration;
mod thread_download_integration;
mod thread_file_links_integration;
mod thread_parity_integration;
mod thread_push_integration;
mod thread_read_all_integration;
mod thread_read_integration;
mod thread_stream_integration;
mod ticket_attachments_integration;
mod ticket_brief_integration;
mod tickets_list_all_integration;
#[cfg(unix)]
mod voice_admin_space_integration;
mod voice_chat_role_integration;
#[cfg(unix)]
mod voice_contract_fake_sew;
#[cfg(unix)]
mod voice_occupancy_fake_sew;
mod workspace_msg_integration;
