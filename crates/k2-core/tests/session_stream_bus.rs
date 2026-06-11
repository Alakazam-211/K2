//! E1 tests for `awareness::bus` — the in-memory signal fan-out.
//!
//! Covers publish/subscribe wiring, multi-subscriber broadcast,
//! dropped-receiver cleanup on next publish, and the cap constant.
//! I/O-free tests only — E2 covers the filesystem inbox, E4 covers
//! the composed egress path.

use std::sync::Mutex;
use std::time::Duration;

use k2_core::awareness::{
    self, AgentAddress, AgentSignal, SignalKind, WorkspaceId, BUS_CAP,
};
use tokio::sync::broadcast::error::TryRecvError;

/// The bus is a process-wide singleton. Parallel tests all subscribe
/// to the same broadcast channel — a publish from test A can land in
/// test B's receive buffer and cause cross-test false failures. This
/// lock serializes the bus-touching tests. Mirrors the pattern flagged
/// for `companion::settings_bridge::tests` in Phase 1.
static BUS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn test_workspace() -> WorkspaceId {
    WorkspaceId("k2so".into())
}

fn test_signal(text: &str) -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: test_workspace(),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: test_workspace(),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: text.to_string(),
        },
    )
}

#[tokio::test(flavor = "current_thread")]
async fn subscribe_then_publish_delivers_signal() {
    let _g = BUS_TEST_LOCK.lock().unwrap();
    let mut rx = awareness::subscribe();
    let signal = test_signal("hello");
    awareness::publish(signal.clone());
    let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("recv should complete within 1s")
        .expect("recv should return Ok");
    assert_eq!(received.id, signal.id);
    assert!(matches!(received.kind, SignalKind::Msg { text } if text == "hello"));
}

#[tokio::test(flavor = "current_thread")]
async fn publish_without_subscribers_is_silent() {
    let _g = BUS_TEST_LOCK.lock().unwrap();
    // Prior tests may have attached subscribers; take a snapshot.
    let count_before = awareness::subscriber_count();
    // No new subscribe() in this test; publish should succeed (broadcast
    // sender silently drops when the internal buffer advances).
    awareness::publish(test_signal("silent"));
    // Subscriber count unchanged by publish.
    assert_eq!(awareness::subscriber_count(), count_before);
}

#[tokio::test(flavor = "current_thread")]
async fn multi_subscriber_each_receives_each_signal() {
    let _g = BUS_TEST_LOCK.lock().unwrap();
    let mut a = awareness::subscribe();
    let mut b = awareness::subscribe();
    let mut c = awareness::subscribe();
    let s1 = test_signal("one");
    let s2 = test_signal("two");
    awareness::publish(s1.clone());
    awareness::publish(s2.clone());

    for rx in [&mut a, &mut b, &mut c] {
        let g1 = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("first recv timed out")
            .expect("first recv Ok");
        let g2 = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("second recv timed out")
            .expect("second recv Ok");
        assert_eq!(g1.id, s1.id);
        assert_eq!(g2.id, s2.id);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropped_receiver_still_allows_publish() {
    let _g = BUS_TEST_LOCK.lock().unwrap();
    // Subscribe, drop the receiver mid-test, publish again — must
    // not panic or block. Broadcast channel cleans up dropped
    // receivers on the next send.
    let rx = awareness::subscribe();
    drop(rx);
    awareness::publish(test_signal("after-drop-1"));
    awareness::publish(test_signal("after-drop-2"));
    // No assertions on receiver state — the fact that we reached
    // here without panicking is the test.
}

#[test]
fn bus_cap_constant_is_expected_value() {
    // No lock needed — pure constant read. Runs in parallel safely.
    // Lock the cap at 256 so accidental rewrites flag as test
    // failures. If we need to retune, bump here intentionally.
    assert_eq!(BUS_CAP, 256);
}

#[tokio::test(flavor = "current_thread")]
async fn try_recv_on_empty_subscription_is_empty() {
    let _g = BUS_TEST_LOCK.lock().unwrap();
    // A fresh subscriber with no publish should see Empty on
    // try_recv, never something stale from a prior test.
    let mut rx = awareness::subscribe();
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test(flavor = "current_thread")]
async fn signal_round_trips_all_core_fields_through_bus() {
    let _g = BUS_TEST_LOCK.lock().unwrap();
    let mut rx = awareness::subscribe();
    let mut original = test_signal("full-field test");
    original.priority = k2_core::awareness::Priority::High;
    awareness::publish(original.clone());
    let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.id, original.id);
    assert_eq!(received.from, original.from);
    assert_eq!(received.to, original.to);
    assert_eq!(received.priority, original.priority);
}
