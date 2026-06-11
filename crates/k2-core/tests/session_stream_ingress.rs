//! E5 ingress tests — APC → bus/egress pipeline.
//!
//! Covers:
//!   - Parsing `"delivery":"inbox"` from APC payloads
//!   - `ingress::from_session` enriches signal.from and signal.session
//!   - End-to-end: APC bytes → line_mux → ingress → egress →
//!     (inbox file OR inject call) + activity_feed row

#![cfg(feature = "test-util")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use k2_core::awareness::{
    self, egress, ingress, AgentAddress, AgentSignal, Delivery,
    InjectProvider, SignalKind, WakeProvider, WorkspaceId,
};
use k2_core::db::init_for_tests;
use k2_core::session::{Frame, SessionId};
use k2_core::term::LineMux;

/// Global provider slots — serialize tests.
static INGRESS_TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    INGRESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Default)]
struct RecordingInject {
    calls: Arc<StdMutex<Vec<(String, Vec<u8>)>>>,
}

impl InjectProvider for RecordingInject {
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((agent.into(), bytes.into()));
        Ok(())
    }
}

#[derive(Clone, Default)]
struct RecordingWake {
    calls: Arc<StdMutex<Vec<String>>>,
}

impl WakeProvider for RecordingWake {
    fn wake(
        &self,
        agent: &str,
        _signal: &AgentSignal,
    ) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(agent.into());
        Ok(())
    }
}

fn install_mocks() -> (RecordingInject, RecordingWake) {
    egress::clear_providers_for_tests();
    let inject = RecordingInject::default();
    let wake = RecordingWake::default();
    egress::set_inject_provider(Box::new(inject.clone()));
    egress::set_wake_provider(Box::new(wake.clone()));
    (inject, wake)
}

fn tmp_inbox(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-ingress-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    ingress::set_inbox_root(path.clone());
    path
}

fn ensure_project(pid: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, path, name) VALUES (?1, ?1, ?1)",
        rusqlite::params![pid],
    )
    .unwrap();
}

fn apc_bytes(verb: &str, json: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x1B); // ESC
    out.push(b'_');
    out.extend_from_slice(format!("k2so:{verb} {json}").as_bytes());
    out.push(0x07); // BEL
    out
}

// ─────────────────────────────────────────────────────────────────────
// Delivery parsing from APC
// ─────────────────────────────────────────────────────────────────────

#[test]
fn apc_without_delivery_defaults_to_live() {
    let _g = lock();
    let mut mux = LineMux::new();
    let frames = mux.feed(&apc_bytes(
        "msg",
        r#"{"to":"bar","text":"hello"}"#,
    ));
    let signal = first_agent_signal(&frames).expect("one signal");
    assert_eq!(signal.delivery, Delivery::Live);
}

#[test]
fn apc_with_delivery_inbox_routes_inbox() {
    let _g = lock();
    let mut mux = LineMux::new();
    let frames = mux.feed(&apc_bytes(
        "msg",
        r#"{"to":"bar","text":"fyi","delivery":"inbox"}"#,
    ));
    let signal = first_agent_signal(&frames).expect("one signal");
    assert_eq!(signal.delivery, Delivery::Inbox);
}

#[test]
fn apc_with_unknown_delivery_falls_back_to_live() {
    let _g = lock();
    let mut mux = LineMux::new();
    let frames = mux.feed(&apc_bytes(
        "msg",
        r#"{"to":"bar","text":"oops","delivery":"galaxy"}"#,
    ));
    let signal = first_agent_signal(&frames).expect("one signal");
    assert_eq!(
        signal.delivery,
        Delivery::Live,
        "unknown delivery string must fall back to Live (don't drop sender intent)"
    );
}

fn first_agent_signal(frames: &[Frame]) -> Option<AgentSignal> {
    frames.iter().find_map(|f| match f {
        Frame::AgentSignal(s) => Some(s.clone()),
        _ => None,
    })
}

// ─────────────────────────────────────────────────────────────────────
// Enrichment + egress end-to-end
// ─────────────────────────────────────────────────────────────────────

#[test]
fn from_session_sets_session_id_and_from_address() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    let _mocks = install_mocks();
    let _inbox = tmp_inbox("from-session");

    let session_id = SessionId::new();
    let ws = WorkspaceId("k2so-ws".into());
    // Build a signal as APC would — from=Broadcast placeholder,
    // to=Agent with empty workspace.
    let bare_signal = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Agent {
            workspace: WorkspaceId(String::new()),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "enriched test".into(),
        },
    );
    let _report = ingress::from_session(
        session_id,
        bare_signal,
        Some("foo"),
        Some(&ws),
    );

    // Enrichment is observable by subscribing to the bus and
    // checking the next signal's fields.
    let mut rx = awareness::subscribe();
    // The signal we published via from_session was consumed by
    // earlier subscribers (none here); we need to send another
    // to observe the enrichment in isolation.
    let bare_signal2 = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Agent {
            workspace: WorkspaceId(String::new()),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "second".into(),
        },
    );
    let _ = ingress::from_session(
        session_id,
        bare_signal2,
        Some("foo"),
        Some(&ws),
    );
    let got = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                rx.recv(),
            )
            .await
        })
        .expect("no timeout")
        .expect("recv");

    assert_eq!(got.session, Some(session_id));
    match &got.from {
        AgentAddress::Agent { workspace, name } => {
            assert_eq!(name, "foo");
            assert_eq!(workspace.0, "k2so-ws");
        }
        other => panic!("from not enriched: {other:?}"),
    }
    match &got.to {
        AgentAddress::Agent { workspace, name } => {
            assert_eq!(name, "bar");
            assert_eq!(workspace.0, "k2so-ws", "to's empty workspace should get filled");
        }
        other => panic!("to not enriched: {other:?}"),
    }
}

#[test]
fn apc_msg_with_live_delivery_triggers_inject_through_ingress() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    let (inject, _wake) = install_mocks();
    let _inbox = tmp_inbox("live-inject");

    // Register "bar" as a live agent so ingress sees Live state.
    use k2_core::session::registry;
    let bar_sid = SessionId::new();
    let bar_entry = registry::register(bar_sid);
    bar_entry.set_agent_name("bar");

    // Build APC from "foo" (the sending session) to "bar".
    let ws = WorkspaceId("k2so-ws".into());
    let from_session_id = SessionId::new();

    // First produce the signal via line_mux.
    let mut mux = LineMux::new();
    let frames = mux.feed(&apc_bytes(
        "msg",
        r#"{"to":"bar","text":"inject me"}"#,
    ));
    let signal = first_agent_signal(&frames).expect("one signal");

    let _report = ingress::from_session(
        from_session_id,
        signal,
        Some("foo"),
        Some(&ws),
    );

    // Inject fired with "inject me" in the bytes. Provider is
    // called with the canonical bare workspace_id key.
    //
    // **0.37.5:** workspace-addressed signals route to bare
    // `<workspace_id>` (post-unification, agent name is metadata
    // not address). Pre-0.37.5 it was `<workspace>:<agent>`; the
    // suffix was vestigial and caused renderer-side mismatches
    // (C3PO 5c80bef1). Reverting `egress.rs:try_inject` to the
    // pre-0.37.5 prefix shape MUST flip this assertion to "FAIL".
    let calls = inject.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "exactly one inject expected");
    assert_eq!(
        calls[0].0, "k2so-ws",
        "0.37.5: inject delivers to bare workspace_id"
    );
    let text = String::from_utf8_lossy(&calls[0].1);
    assert!(text.contains("inject me"), "bytes were: {text}");

    registry::unregister(&bar_sid);
}

#[test]
fn apc_msg_with_delivery_inbox_writes_inbox_no_inject_even_when_live() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    let (inject, _wake) = install_mocks();
    let inbox = tmp_inbox("inbox-over-live");

    use k2_core::session::registry;
    let bar_sid = SessionId::new();
    let bar_entry = registry::register(bar_sid);
    bar_entry.set_agent_name("bar");

    let mut mux = LineMux::new();
    let frames = mux.feed(&apc_bytes(
        "msg",
        r#"{"to":"bar","text":"fyi","delivery":"inbox"}"#,
    ));
    let signal = first_agent_signal(&frames).expect("one signal");

    let ws = WorkspaceId("k2so-ws".into());
    let report = ingress::from_session(
        SessionId::new(),
        signal,
        Some("foo"),
        Some(&ws),
    );

    assert!(
        inject.calls.lock().unwrap().is_empty(),
        "Delivery::Inbox must never inject, even when target is live"
    );
    assert!(report.inbox_path.is_some());
    // And a drain would return it.
    let drained = awareness::inbox::drain(&inbox, "bar");
    assert_eq!(drained.len(), 1);

    registry::unregister(&bar_sid);
}

#[test]
fn apc_with_offline_target_triggers_wake() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    let (_inject, wake) = install_mocks();
    let _inbox = tmp_inbox("live-offline-wake");

    // No session registered for "bar".
    let mut mux = LineMux::new();
    let frames = mux.feed(&apc_bytes(
        "msg",
        r#"{"to":"bar","text":"wake up"}"#,
    ));
    let signal = first_agent_signal(&frames).expect("one signal");

    let ws = WorkspaceId("k2so-ws".into());
    let report = ingress::from_session(
        SessionId::new(),
        signal,
        Some("foo"),
        Some(&ws),
    );

    let wake_calls = wake.calls.lock().unwrap().clone();
    assert_eq!(wake_calls, vec!["bar".to_string()]);
    assert!(report.woke_offline_target);
    assert!(report.inbox_path.is_none()); // Live never writes inbox.
}
