//! k2-daemon integration tests: Thread, tickets and the inbox.
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

mod chat_transcript_route_gone;
mod inbox_stream_integration;
mod stamp_strip_integration;
mod thread_attachments_integration;
mod thread_catchup_integration;
mod thread_delta_attachments_integration;
mod thread_delta_integration;
mod thread_download_integration;
mod thread_file_links_integration;
mod thread_parity_integration;
mod thread_read_all_integration;
mod thread_stream_integration;
mod ticket_attachments_integration;
