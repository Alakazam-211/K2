//! GH#22 — remote PTY sessions died because the host's age-out
//! reaper killed a session a remote client was attached to.
//!
//! The daemon-side fix is two-fold:
//!
//!   1. `subscriberCount` in `/cli/agents/running` must be REAL for
//!      v2 sessions — sourced from the session's OWN viewer registry
//!      (the `attach_viewer()` registration each grid-WS holds),
//!      not the legacy `session::registry` (always 0 for v2).
//!   2. `/cli/sessions/v2/close` (the reaper's kill endpoint) must
//!      REFUSE to tear down a session that still has attached
//!      subscribers — unless an explicit `force` flag is set.
//!
//! 2026-07-02 (PTY-leak Bug 2): the viewer signal moved off
//! `events_tx.receiver_count()` — internal daemon observers (the
//! child-exit observer, the shared grid emitter) hold receivers on
//! that channel for the session's life, so the count never returned
//! to zero and the un-forced close guard refused EVERY reap. The
//! suite now drives `attach_viewer()` registrations (what the grid-WS
//! holds) and pins that a bare `subscribe_events()` receiver — the
//! internal-observer shape — does NOT count.
//!
//! These tests spawn a REAL `DaemonPtySession` (forking a long-lived
//! `sleep` child) inside the isolated test process. They do NOT
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

/// (1) subscriberCount source: the v2 session's own viewer registry.
/// Zero with nobody attached; reflects the number of live
/// `attach_viewer()` registrations (what each grid-WS holds); drops
/// back to zero when all viewers detach.
#[test]
fn subscriber_count_reflects_attached_grid_ws_viewers() {
    let session = spawn_live_session();

    // No grid-WS attached yet.
    assert_eq!(
        session.subscriber_count(),
        0,
        "fresh session with no grid-WS attached must report 0 subscribers"
    );

    // Each grid-WS attach calls `session.attach_viewer()` and holds
    // the registration for the connection lifetime. Simulate two viewers.
    let reg1 = session.attach_viewer();
    assert_eq!(
        session.subscriber_count(),
        1,
        "one attached viewer must be counted"
    );
    let reg2 = session.attach_viewer();
    assert_eq!(
        session.subscriber_count(),
        2,
        "two attached viewers must be counted"
    );

    // Detach (grid-WS disconnect drops its registration).
    drop(reg1);
    assert_eq!(
        session.subscriber_count(),
        1,
        "count must drop when a viewer detaches"
    );
    drop(reg2);
    assert_eq!(
        session.subscriber_count(),
        0,
        "count must return to 0 when all viewers detach"
    );

    session.kill();
}

/// (1b) 2026-07-02 PTY-leak Bug 2 pin: an internal daemon observer —
/// a bare `subscribe_events()` receiver, exactly what the child-exit
/// observer and the shared grid emitter hold for the session's life —
/// must NEVER count as an attached viewer. Pre-fix, this receiver
/// inflated `subscriber_count()` so it never reached zero, and the
/// un-forced close guard below refused EVERY reap.
#[test]
fn internal_event_receivers_do_not_count_as_viewers() {
    let session = spawn_live_session();

    // Internal-observer shape: hold an events receiver, no viewer reg.
    let _internal_rx = session.subscribe_events();
    let _second_internal_rx = session.subscribe_events();
    assert_eq!(
        session.subscriber_count(),
        0,
        "internal subscribe_events() receivers must not count as viewers"
    );

    // And the un-forced close guard must therefore PROCEED.
    let agent = uniq_agent_name();
    v2_session_map::register(agent.clone(), session.clone());
    let body = format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
    let result = handle_v2_close(&body);
    assert_eq!(result.status, "200 OK");
    assert!(
        result.body.contains(r#""closed":true"#),
        "un-forced close must proceed with only internal observers attached; got: {}",
        result.body
    );

    session.kill();
}

/// (1c) `attach_viewer()` and the `ever_attached` latch share one
/// write point, so the two "attached" signals cannot disagree: the
/// latch flips exactly when the first viewer registers, and STAYS
/// latched after the viewer detaches (it feeds v2_spawn's
/// never-attached bare-shell cap, which must not re-count a
/// once-viewed tab).
#[test]
fn attach_viewer_latches_ever_attached() {
    use std::sync::atomic::Ordering;

    let session = spawn_live_session();
    assert!(
        !session.ever_attached.load(Ordering::Relaxed),
        "fresh session must start never-attached"
    );

    let reg = session.attach_viewer();
    assert!(
        session.ever_attached.load(Ordering::Relaxed),
        "first viewer registration must latch ever_attached"
    );

    drop(reg);
    assert_eq!(session.subscriber_count(), 0);
    assert!(
        session.ever_attached.load(Ordering::Relaxed),
        "ever_attached must STAY latched after the viewer detaches"
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

    // A client attaches over the grid-WS — live viewer registration held.
    let _reg = session.attach_viewer();
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

    let _reg = session.attach_viewer();
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
