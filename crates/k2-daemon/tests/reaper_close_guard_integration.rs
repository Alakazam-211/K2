//! GH#22 — remote PTY sessions died because the host's age-out
//! reaper killed a session a remote client was attached to.
//!
//! The daemon-side fix is two-fold:
//!
//!   1. `subscriberCount` in `/cli/agents/running` must be REAL for
//!      v2 sessions — sourced from the session's OWN broadcast channel
//!      (the one each grid-WS subscribes to via `subscribe_events()`),
//!      not the legacy `session::registry` (always 0 for v2).
//!   2. `/cli/sessions/v2/close` (the reaper's kill endpoint) must
//!      REFUSE to tear down a session that still has attached
//!      subscribers — unless an explicit `force` flag is set.
//!
//! These tests spawn a REAL `DaemonPtySession` (forking a long-lived
//! `sleep` child) inside the isolated test process and drive the
//! actual `subscribe_events()` receiver the grid-WS uses. They do NOT
//! touch the live daemon: the session lives only in this test
//! process's own `v2_session_map`.

use std::sync::atomic::{AtomicUsize, Ordering};

use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::v2_session_map;
use k2_daemon::v2_spawn::handle_v2_close;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

fn uniq_agent_name() -> String {
    format!(
        "test-reaper-guard-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::SeqCst)
    )
}

/// Spawn a real PTY child that stays alive for the duration of the
/// test (a long `sleep`) so `is_child_alive()` holds and the session
/// is a legitimate reap target.
fn spawn_live_session() -> std::sync::Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        program: Some("/bin/sleep".to_string()),
        args: vec!["60".to_string()],
        ..Default::default()
    };
    DaemonPtySession::spawn(cfg).expect("spawn sleep PTY")
}

/// (1) subscriberCount source: the v2 session's own broadcast channel.
/// Zero with nobody attached; reflects the number of live
/// `subscribe_events()` receivers (what each grid-WS holds); drops
/// back to zero when all subscribers detach.
#[test]
fn subscriber_count_reflects_attached_grid_ws_receivers() {
    let session = spawn_live_session();

    // No grid-WS attached yet.
    assert_eq!(
        session.subscriber_count(),
        0,
        "fresh session with no grid-WS attached must report 0 subscribers"
    );

    // Each grid-WS attach calls `session.subscribe_events()` and holds
    // the receiver for the connection lifetime. Simulate two viewers.
    let rx1 = session.subscribe_events();
    assert_eq!(
        session.subscriber_count(),
        1,
        "one attached subscriber must be counted"
    );
    let rx2 = session.subscribe_events();
    assert_eq!(
        session.subscriber_count(),
        2,
        "two attached subscribers must be counted"
    );

    // Detach (grid-WS disconnect drops its receiver).
    drop(rx1);
    assert_eq!(
        session.subscriber_count(),
        1,
        "count must drop when a subscriber detaches"
    );
    drop(rx2);
    assert_eq!(
        session.subscriber_count(),
        0,
        "count must return to 0 when all subscribers detach"
    );

    session.kill();
}

/// (2a) `/cli/sessions/v2/close` REFUSES to reap a session with an
/// attached subscriber and does NOT unregister it.
#[test]
fn v2_close_refuses_when_subscriber_attached() {
    let agent = uniq_agent_name();
    let session = spawn_live_session();
    v2_session_map::register(agent.clone(), session.clone());

    // A client attaches over the grid-WS — live receiver held.
    let _rx = session.subscribe_events();
    assert_eq!(session.subscriber_count(), 1);

    let body = format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
    let result = handle_v2_close(&body);

    assert_eq!(result.status, "200 OK");
    assert!(
        result.body.contains(r#""closed":false"#),
        "close must report closed=false while a client is attached; got: {}",
        result.body
    );
    assert!(
        result.body.contains("still has attached clients"),
        "refusal must carry the attached-clients reason; got: {}",
        result.body
    );

    // The session must STILL be registered — the reaper was refused.
    assert!(
        v2_session_map::lookup_by_agent_name(&agent).is_some(),
        "refused close must NOT unregister the still-attached session"
    );

    // Cleanup.
    if let Some(s) = v2_session_map::unregister(&agent) {
        s.kill();
    }
}

/// (2b) `/cli/sessions/v2/close` PROCEEDS (unregisters) when no
/// subscribers are attached — the normal reap path.
#[test]
fn v2_close_proceeds_when_no_subscriber() {
    let agent = uniq_agent_name();
    let session = spawn_live_session();
    v2_session_map::register(agent.clone(), session.clone());

    // No grid-WS attached.
    assert_eq!(session.subscriber_count(), 0);

    let body = format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
    let result = handle_v2_close(&body);

    assert_eq!(result.status, "200 OK");
    assert!(
        result.body.contains(r#""closed":true"#),
        "close must proceed (closed=true) when no client is attached; got: {}",
        result.body
    );
    assert!(
        v2_session_map::lookup_by_agent_name(&agent).is_none(),
        "successful close must unregister the session"
    );

    session.kill();
}

/// (2c) `force: true` bypasses the attached-subscriber guard — the
/// deliberate-teardown escape hatch.
#[test]
fn v2_close_force_bypasses_attached_guard() {
    let agent = uniq_agent_name();
    let session = spawn_live_session();
    v2_session_map::register(agent.clone(), session.clone());

    let _rx = session.subscribe_events();
    assert_eq!(session.subscriber_count(), 1);

    let body =
        format!(r#"{{"agent_name":"{}","force":true}}"#, agent).into_bytes();
    let result = handle_v2_close(&body);

    assert_eq!(result.status, "200 OK");
    assert!(
        result.body.contains(r#""closed":true"#),
        "force close must proceed even with a client attached; got: {}",
        result.body
    );
    assert!(
        v2_session_map::lookup_by_agent_name(&agent).is_none(),
        "force close must unregister the session"
    );

    session.kill();
}
