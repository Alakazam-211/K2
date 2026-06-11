//! E4 integration tests for `awareness::egress` — the composer.
//!
//! Tests the full four-cell delivery matrix by registering mock
//! InjectProvider + WakeProvider, then calling `deliver` and
//! asserting which channels fired.
//!
//! The test-util DB is used so activity_feed rows actually land.

#![cfg(feature = "test-util")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use k2_core::awareness::{
    self, egress, AgentAddress, AgentSignal, Delivery, InjectProvider,
    SignalKind, WakeProvider, WorkspaceId,
};
use k2_core::db::{init_for_tests, shared};
use k2_core::session::{registry, SessionId};

/// Serialize every egress test — provider slots are global OnceLocks.
static EGRESS_TEST_LOCK: StdMutex<()> =
    StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    EGRESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Mock InjectProvider that records every (agent, bytes) call.
#[derive(Clone, Default)]
struct RecordingInject {
    calls: Arc<StdMutex<Vec<(String, Vec<u8>)>>>,
}

impl InjectProvider for RecordingInject {
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((agent.to_string(), bytes.to_vec()));
        Ok(())
    }
}

/// Mock WakeProvider that records every wake request.
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
        self.calls.lock().unwrap().push(agent.to_string());
        Ok(())
    }
}

fn install_mocks() -> (RecordingInject, RecordingWake) {
    egress::clear_providers_for_tests();
    // G5: reset the per-sender budget counter so tests see a clean
    // slate. Every egress test uses sender="sender" with the default
    // Moderate level (budget=5); accumulating across tests would
    // cause late-ordered tests to hit budget denial unexpectedly.
    awareness::budget::reset_for_tests();
    let inject = RecordingInject::default();
    let wake = RecordingWake::default();
    egress::set_inject_provider(Box::new(inject.clone()));
    egress::set_wake_provider(Box::new(wake.clone()));
    (inject, wake)
}

fn tmp_inbox_root(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-egress-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn workspace() -> WorkspaceId {
    WorkspaceId("test-ws".into())
}

fn ensure_project_row(project_id: &str) {
    let db = shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, path, name) VALUES (?1, ?1, ?1)",
        rusqlite::params![project_id],
    )
    .unwrap();
}

fn signal_to(target: &str, delivery: Delivery) -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "sender".into(),
        },
        AgentAddress::Agent {
            workspace: workspace(),
            name: target.to_string(),
        },
        SignalKind::Msg {
            text: "hello from egress test".into(),
        },
    )
    .with_delivery(delivery)
}

fn register_live_agent(name: &str) -> SessionId {
    let id = SessionId::new();
    let entry = registry::register(id);
    entry.set_agent_name(name);
    id
}

// ─────────────────────────────────────────────────────────────────────
// The four matrix cells
// ─────────────────────────────────────────────────────────────────────

#[test]
fn live_live_injects_and_audits_no_inbox_no_wake() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let (inject, wake) = install_mocks();
    let inbox_root = tmp_inbox_root("live-live");

    // Liveness check uses the legacy registry walk (bare agent
    // names) — `is_agent_live` matches `entry.agent_name() == agent`.
    //
    // **0.37.5 inject path:** workspace-addressed signals route to
    // the bare `<workspace_id>` canonical key (`try_inject` in
    // egress.rs:443). Pre-0.37.5 it was `<workspace>:<agent>`; the
    // suffix was vestigial post-unification and caused the renderer
    // to compute the wrong key (C3PO 5c80bef1).
    //
    // The test verifies BOTH sides: bare-name liveness lookup
    // (legacy registry walk), bare-pid inject delivery (post-0.37.5
    // canonical key). Reverting `egress.rs:try_inject` to the
    // pre-0.37.5 prefix shape MUST flip this assertion to "FAIL"
    // (verifies the test pins the new contract, not a fallback).
    let sid = register_live_agent("bar");
    let signal = signal_to("bar", Delivery::Live);
    let report = egress::deliver(&signal, &inbox_root);

    // PTY-inject fired once. Provider sees the canonical bare-pid key.
    let inject_calls = inject.calls.lock().unwrap().clone();
    assert_eq!(inject_calls.len(), 1);
    assert_eq!(
        inject_calls[0].0, "test-ws",
        "0.37.5: inject delivers to bare workspace_id, NOT prefixed with agent name"
    );
    assert!(
        String::from_utf8_lossy(&inject_calls[0].1)
            .contains("hello from egress test"),
        "injected bytes should carry the msg"
    );
    // Wake did not fire (target was already live).
    assert!(wake.calls.lock().unwrap().is_empty());
    // No inbox file.
    assert!(inbox_root.join("bar").read_dir().is_err()
        || inbox_root
            .join("bar")
            .read_dir()
            .unwrap()
            .next()
            .is_none());

    // Report matches.
    assert!(report.injected_to_pty);
    assert!(!report.woke_offline_target);
    assert!(report.inbox_path.is_none());
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);

    registry::unregister(&sid);
}

#[test]
fn live_offline_wakes_and_audits_no_inbox_no_inject() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let (inject, wake) = install_mocks();
    let inbox_root = tmp_inbox_root("live-offline");

    // No session registered for bar — offline.
    let signal = signal_to("bar", Delivery::Live);
    let report = egress::deliver(&signal, &inbox_root);

    assert!(inject.calls.lock().unwrap().is_empty());
    let wake_calls = wake.calls.lock().unwrap().clone();
    assert_eq!(wake_calls.len(), 1);
    assert_eq!(wake_calls[0], "bar");
    // Still no inbox file (Live+Offline uses pending-queue, not inbox).
    let inbox_dir = inbox_root.join("bar");
    assert!(
        !inbox_dir.exists()
            || inbox_dir.read_dir().unwrap().next().is_none(),
        "inbox must be empty for Live+Offline"
    );
    assert!(!report.injected_to_pty);
    assert!(report.woke_offline_target);
    assert!(report.inbox_path.is_none());
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);
}

#[test]
fn inbox_live_writes_file_no_inject_no_wake() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let (inject, wake) = install_mocks();
    let inbox_root = tmp_inbox_root("inbox-live");

    let sid = register_live_agent("bar");
    let signal = signal_to("bar", Delivery::Inbox);
    let report = egress::deliver(&signal, &inbox_root);

    // No inject — sender chose Inbox even though target is live.
    assert!(
        inject.calls.lock().unwrap().is_empty(),
        "Inbox must never inject"
    );
    assert!(
        wake.calls.lock().unwrap().is_empty(),
        "Inbox must never wake"
    );

    // File landed.
    let inbox_path = report.inbox_path.clone().expect("inbox_path set");
    assert!(inbox_path.exists());
    assert!(inbox_path.starts_with(inbox_root.join("bar")));

    // Drain parses correctly.
    let drained =
        awareness::inbox::drain(&inbox_root, "bar");
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].id, signal.id);

    assert!(!report.injected_to_pty);
    assert!(!report.woke_offline_target);
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);

    registry::unregister(&sid);
}

#[test]
fn inbox_offline_writes_file_no_wake() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let (_inject, wake) = install_mocks();
    let inbox_root = tmp_inbox_root("inbox-offline");

    // No session for bar.
    let signal = signal_to("bar", Delivery::Inbox);
    let report = egress::deliver(&signal, &inbox_root);

    assert!(
        wake.calls.lock().unwrap().is_empty(),
        "Inbox delivery must never wake even when target is offline"
    );
    assert!(report.inbox_path.is_some());
    assert!(report.activity_feed_row_id > 0);
}

// ─────────────────────────────────────────────────────────────────────
// Audit invariants
// ─────────────────────────────────────────────────────────────────────

#[test]
fn every_delivery_writes_activity_feed_row() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let _ = install_mocks();
    let inbox_root = tmp_inbox_root("audit");

    for delivery in [Delivery::Live, Delivery::Inbox] {
        let report = egress::deliver(
            &signal_to("bar", delivery),
            &inbox_root,
        );
        assert!(
            report.activity_feed_row_id > 0,
            "no feed row for {delivery:?}"
        );
        assert!(report.published_to_bus);
    }
}

#[test]
fn every_delivery_publishes_to_bus() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let _ = install_mocks();
    let inbox_root = tmp_inbox_root("bus");

    // Subscribe BEFORE delivery so we catch the publish.
    let mut rx = awareness::subscribe();
    let signal = signal_to("anyone", Delivery::Inbox);
    egress::deliver(&signal, &inbox_root);
    let received = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                rx.recv(),
            )
            .await
        });
    let got = received.expect("no timeout").expect("bus recv");
    assert_eq!(got.id, signal.id);
}

// ─────────────────────────────────────────────────────────────────────
// Provider graceful-missing behavior
// ─────────────────────────────────────────────────────────────────────

#[test]
fn no_inject_provider_degrades_to_audit_only_on_live_to_live() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    egress::clear_providers_for_tests();
    let inbox_root = tmp_inbox_root("no-inject-provider");

    let sid = register_live_agent("bar");
    let signal = signal_to("bar", Delivery::Live);
    let report = egress::deliver(&signal, &inbox_root);

    // Inject could not fire (no provider) — report's injected_to_pty
    // stays false.
    assert!(!report.injected_to_pty);
    // But bus + feed still fired.
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);
    // No inbox file (sender didn't pick Inbox).
    assert!(report.inbox_path.is_none());

    registry::unregister(&sid);
}

#[test]
fn no_wake_provider_degrades_to_audit_only_on_live_to_offline() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    egress::clear_providers_for_tests();
    let inbox_root = tmp_inbox_root("no-wake-provider");

    let signal = signal_to("bar", Delivery::Live);
    let report = egress::deliver(&signal, &inbox_root);

    assert!(!report.woke_offline_target);
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);
}

// ─────────────────────────────────────────────────────────────────────
// Broadcast handling
// ─────────────────────────────────────────────────────────────────────

/// F1.5 regression — every egress must store the full signal JSON
/// in activity_feed.metadata so the audit log carries enough to
/// reconstruct the entire message. The 80-char summary is for UI
/// preview; metadata is the primitive source of truth.
#[test]
fn activity_feed_metadata_carries_full_signal_json() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let _ = install_mocks();
    let inbox_root = tmp_inbox_root("metadata-audit");

    let sig = signal_to("bar", Delivery::Inbox);
    let original_id = sig.id;
    let report = egress::deliver(&sig, &inbox_root);
    assert!(report.activity_feed_row_id > 0);

    // Query the row's metadata back out.
    let db = shared();
    let conn = db.lock();
    let meta: Option<String> = conn
        .query_row(
            "SELECT metadata FROM activity_feed WHERE id = ?1",
            rusqlite::params![report.activity_feed_row_id],
            |row| row.get(0),
        )
        .unwrap();
    let meta_str = meta.expect("metadata populated");

    // Parse back into AgentSignal — round-trip must preserve id,
    // delivery, and body.
    let decoded: AgentSignal =
        serde_json::from_str(&meta_str).expect("metadata parses as AgentSignal");
    assert_eq!(decoded.id, original_id);
    assert_eq!(decoded.delivery, Delivery::Inbox);
    match decoded.kind {
        SignalKind::Msg { text } => {
            assert_eq!(text, "hello from egress test");
        }
        _ => panic!("kind changed"),
    }
}

#[test]
fn broadcast_signal_writes_audit_but_no_direct_delivery() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("test-ws");
    let (inject, wake) = install_mocks();
    let inbox_root = tmp_inbox_root("broadcast");

    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "sender".into(),
        },
        AgentAddress::Broadcast,
        SignalKind::Status {
            text: "everyone heads up".into(),
        },
    );
    let report = egress::deliver(&signal, &inbox_root);

    // No per-target inject/wake/inbox — those fan out in the caller,
    // not in deliver(). But audit fires.
    assert!(inject.calls.lock().unwrap().is_empty());
    assert!(wake.calls.lock().unwrap().is_empty());
    assert!(report.inbox_path.is_none());
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);
}

// ─────────────────────────────────────────────────────────────────────
// G0 — audit-always-fires: unknown workspace falls back to `_orphan`
// ─────────────────────────────────────────────────────────────────────
//
// Regression guard for the real-world bug surfaced by Rosson's
// manual three-terminal smoke test on 2026-04-20: the CLI sent a
// filesystem path as the signal's workspace, which didn't match any
// `projects.id` UUID, so the `activity_feed` FK insert failed and
// audit silently dropped (`activity_feed_row_id: 0`).
//
// With the _orphan fallback in place, audit ALWAYS fires — even
// when the sender hands us a workspace id that doesn't resolve.
// The original unresolved id is preserved in `to_project_id` so
// audit consumers don't lose information.

#[test]
fn unknown_workspace_falls_back_to_orphan_audit_bucket() {
    let _g = lock();
    init_for_tests();
    // Deliberately do NOT call ensure_project_row — the sender's
    // workspace is an unregistered path, exactly mirroring the CLI
    // bug from the Phase 3.1 smoke test.
    let (_inject, _wake) = install_mocks();
    let inbox_root = tmp_inbox_root("orphan-fallback");

    let unregistered = WorkspaceId("/not/a/registered/project".into());
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: unregistered.clone(),
            name: "sender".into(),
        },
        AgentAddress::Agent {
            workspace: unregistered.clone(),
            name: "any-target".into(),
        },
        SignalKind::Msg {
            text: "audit must still fire".into(),
        },
    )
    .with_delivery(Delivery::Inbox);

    let report = egress::deliver(&signal, &inbox_root);

    // Audit row DID get written — row id is non-zero even though
    // the sender's workspace wasn't registered.
    assert!(
        report.activity_feed_row_id > 0,
        "audit must fire via _orphan fallback (row id = {})",
        report.activity_feed_row_id
    );

    // The row is bucketed under `_orphan`, and the original
    // unresolved workspace is preserved as `to_project_id` so
    // audit consumers can still see what the sender claimed.
    let db = shared();
    let conn = db.lock();
    let (bucket, claimed): (String, Option<String>) = conn
        .query_row(
            "SELECT project_id, to_project_id FROM activity_feed WHERE id = ?1",
            rusqlite::params![report.activity_feed_row_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(bucket, "_orphan");
    assert_eq!(claimed.as_deref(), Some(unregistered.0.as_str()));
}

#[test]
fn orphan_sentinel_row_exists_after_init_for_tests() {
    // init_for_tests seeds both sentinel rows. Sanity-check that
    // a subsequent lookup finds them — if seeding is accidentally
    // skipped in the future, the fallback test above would also
    // break, but this pinpoints the cause more precisely.
    let _g = lock();
    init_for_tests();

    let db = shared();
    let conn = db.lock();
    for sentinel in ["_orphan", "_broadcast"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                rusqlite::params![sentinel],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing sentinel row: {sentinel}");
    }
}

// ─────────────────────────────────────────────────────────────────────
// G5 — per-coordination-level budget denies over-limit senders
// ─────────────────────────────────────────────────────────────────────
//
// End-to-end through `egress::deliver`: an agent sender without an
// AGENT.md defaults to Moderate (budget=5). Five signals land
// normally; the sixth is denied. The DeliveryReport reflects the
// denial AND an activity_feed row with
// `event_type='signal:budget-denied'` is written for audit
// consumers to spot + count.

#[tokio::test(flavor = "current_thread")]
async fn sender_over_budget_is_denied_and_audited() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("budget-ws");
    let (inject, wake) = install_mocks();
    let inbox_root = tmp_inbox_root("budget-denied");

    let sid = register_live_agent("budget-target");

    // Five Msg emits within Moderate budget.
    for i in 0..5 {
        let signal = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("budget-ws".into()),
                name: "chatty-agent".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("budget-ws".into()),
                name: "budget-target".into(),
            },
            SignalKind::Msg {
                text: format!("msg {i}"),
            },
        )
        .with_delivery(Delivery::Live);
        let report = egress::deliver(&signal, &inbox_root);
        assert!(
            !report.budget_denied,
            "emit {i} (within budget=5) must not be denied"
        );
        assert!(report.activity_feed_row_id > 0);
    }

    // Sixth emit → over budget; denied.
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("budget-ws".into()),
            name: "chatty-agent".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("budget-ws".into()),
            name: "budget-target".into(),
        },
        SignalKind::Msg {
            text: "one too many".into(),
        },
    )
    .with_delivery(Delivery::Live);
    let denied = egress::deliver(&signal, &inbox_root);
    assert!(denied.budget_denied, "sixth emit must be denied");
    assert!(!denied.injected_to_pty, "denied signals must not inject");
    assert!(!denied.published_to_bus, "denied signals must not publish to bus");
    assert!(denied.inbox_path.is_none(), "denied signals must not inbox");
    assert!(
        denied.activity_feed_row_id > 0,
        "denied signal must still fire audit"
    );

    // Audit row has the dedicated event_type tagging the denial.
    let db = shared();
    let conn = db.lock();
    let event_type: String = conn
        .query_row(
            "SELECT event_type FROM activity_feed WHERE id = ?1",
            rusqlite::params![denied.activity_feed_row_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_type, "signal:budget-denied");
    drop(conn);

    // Inject fired 5 times (the allowed emits) and NOT on the 6th.
    assert_eq!(inject.calls.lock().unwrap().len(), 5);
    // Wake never fired because the target was live for all emits.
    assert!(wake.calls.lock().unwrap().is_empty());

    registry::unregister(&sid);
}

#[tokio::test(flavor = "current_thread")]
async fn cli_sender_bypasses_budget() {
    let _g = lock();
    init_for_tests();
    ensure_project_row("budget-ws-cli");
    let (inject, _) = install_mocks();
    let inbox_root = tmp_inbox_root("cli-bypass");

    let sid = register_live_agent("cli-target");

    // 20 emits > Chatty budget (10); the `cli` sender-name bypass
    // means none are denied.
    for i in 0..20 {
        let signal = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("budget-ws-cli".into()),
                name: k2_core::awareness::CLI_SENDER_NAME.into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("budget-ws-cli".into()),
                name: "cli-target".into(),
            },
            SignalKind::Msg {
                text: format!("cli-msg {i}"),
            },
        )
        .with_delivery(Delivery::Live);
        let report = egress::deliver(&signal, &inbox_root);
        assert!(!report.budget_denied, "cli emit {i} must never be denied");
    }

    assert_eq!(inject.calls.lock().unwrap().len(), 20);
    registry::unregister(&sid);
}
