//! D1 SessionRegistry + SessionEntry tests.
//!
//! Covers register / lookup / unregister lifecycle, multi-subscriber
//! broadcast fanout, replay-ring trim at cap, and custom replay cap.
//!
//! The registry is a process-wide singleton — each test uses unique
//! `SessionId`s (UUID v4) so parallel runs never collide. Tests do
//! NOT call `len()` / `list_ids()` because those read shared global
//! state and would race with other tests in the same binary.

use std::time::Duration;

use k2_core::session::{
    registry, Frame, SemanticKind, SessionEntry, SessionId, BROADCAST_CAP, REPLAY_CAP,
};
use serde_json::json;
use tokio::sync::broadcast::error::TryRecvError;

fn text_frame(s: &str) -> Frame {
    Frame::Text {
        bytes: s.as_bytes().to_vec(),
        style: None,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Registry lifecycle
// ─────────────────────────────────────────────────────────────────────

#[test]
fn register_then_lookup_returns_same_entry() {
    let id = SessionId::new();
    let a = registry::register(id);
    let b = registry::lookup(&id).expect("lookup should find just-registered entry");
    // Both Arc clones point at the same SessionEntry.
    assert!(std::ptr::eq(
        a.as_ref() as *const _,
        b.as_ref() as *const _
    ));
    registry::unregister(&id);
}

#[test]
fn lookup_of_unknown_id_returns_none() {
    let id = SessionId::new();
    assert!(registry::lookup(&id).is_none());
}

#[test]
fn unregister_removes_from_map() {
    let id = SessionId::new();
    registry::register(id);
    assert!(registry::lookup(&id).is_some());
    registry::unregister(&id);
    assert!(registry::lookup(&id).is_none());
}

#[test]
fn re_register_replaces_old_entry() {
    let id = SessionId::new();
    let a = registry::register(id);
    let b = registry::register(id);
    // Different allocations — the re-register built a fresh entry.
    assert!(!std::ptr::eq(
        a.as_ref() as *const _,
        b.as_ref() as *const _
    ));
    // Only the new one is in the map now.
    let current = registry::lookup(&id).unwrap();
    assert!(std::ptr::eq(
        b.as_ref() as *const _,
        current.as_ref() as *const _
    ));
    registry::unregister(&id);
}

#[test]
fn holder_of_old_arc_still_works_after_unregister() {
    // Unregister removes from the map but existing Arc holders
    // (e.g. a subscriber still reading) keep the entry alive
    // until they drop their Arc. This is the expected tokio
    // broadcast shutdown path.
    let id = SessionId::new();
    let entry = registry::register(id);
    let mut receiver = entry.subscribe();
    registry::unregister(&id);
    // Publishing still delivers to holders of the Arc even after
    // unregister.
    entry.publish(text_frame("still here"));
    let got = receiver.try_recv().expect("frame should have landed");
    match got {
        Frame::Text { bytes, .. } => assert_eq!(bytes, b"still here"),
        _ => panic!("wrong frame"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Replay ring behavior
// ─────────────────────────────────────────────────────────────────────

#[test]
fn replay_ring_stores_frames_in_order() {
    let entry = SessionEntry::new();
    for i in 0..5 {
        entry.publish(text_frame(&format!("f{i}")));
    }
    let snapshot = entry.replay_snapshot();
    assert_eq!(snapshot.len(), 5);
    for (i, frame) in snapshot.iter().enumerate() {
        match frame {
            Frame::Text { bytes, .. } => {
                assert_eq!(bytes, format!("f{i}").as_bytes())
            }
            _ => panic!("expected Text"),
        }
    }
}

#[test]
fn replay_ring_trims_front_at_cap() {
    let entry = SessionEntry::with_replay_cap(3);
    for i in 0..5 {
        entry.publish(text_frame(&format!("f{i}")));
    }
    let snapshot = entry.replay_snapshot();
    assert_eq!(snapshot.len(), 3);
    // The three retained should be f2, f3, f4 — oldest popped.
    let labels: Vec<_> = snapshot
        .iter()
        .map(|f| match f {
            Frame::Text { bytes, .. } => {
                String::from_utf8(bytes.clone()).unwrap()
            }
            _ => String::new(),
        })
        .collect();
    assert_eq!(labels, vec!["f2", "f3", "f4"]);
    assert_eq!(entry.replay_cap(), 3);
}

#[test]
fn replay_ring_cap_covers_claude_resume_bursts() {
    // Bumped from the original 1000 to 50_000 so `claude --resume`
    // (which replays the full prior conversation as a fast burst
    // of tens of thousands of LineMux frames) fits in the ring
    // before the Tauri renderer's late-joining WS subscription
    // attaches. See the comment on REPLAY_CAP for the full story.
    assert_eq!(REPLAY_CAP, 50_000);
}

#[test]
fn broadcast_cap_matches_events_endpoint() {
    // The daemon's /events endpoint uses 256; we mirror for
    // familiarity. If either side changes, update both.
    assert_eq!(BROADCAST_CAP, 256);
}

#[test]
#[should_panic(expected = "replay_cap must be >= 1")]
fn zero_replay_cap_panics() {
    let _ = SessionEntry::with_replay_cap(0);
}

// ─────────────────────────────────────────────────────────────────────
// Broadcast fanout
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn single_subscriber_receives_published_frame() {
    let entry = SessionEntry::new();
    let mut rx = entry.subscribe();
    entry.publish(text_frame("hi"));
    let got = rx.recv().await.expect("should receive");
    match got {
        Frame::Text { bytes, .. } => assert_eq!(bytes, b"hi"),
        _ => panic!("wrong frame"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn multi_subscriber_each_receives_each_frame() {
    let entry = SessionEntry::new();
    let mut a = entry.subscribe();
    let mut b = entry.subscribe();
    let mut c = entry.subscribe();
    entry.publish(text_frame("one"));
    entry.publish(text_frame("two"));
    for rx in [&mut a, &mut b, &mut c] {
        for expected in ["one", "two"] {
            let frame = rx
                .recv()
                .await
                .unwrap_or_else(|e| panic!("recv failed: {e}"));
            match frame {
                Frame::Text { bytes, .. } => {
                    assert_eq!(bytes, expected.as_bytes())
                }
                _ => panic!("wrong frame kind"),
            }
        }
    }
    assert_eq!(entry.subscriber_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn semantic_event_frames_round_trip_through_broadcast() {
    let entry = SessionEntry::new();
    let mut rx = entry.subscribe();
    let published = Frame::SemanticEvent {
        kind: SemanticKind::ToolCall,
        payload: json!({ "name": "bash", "id": "t_1" }),
    };
    entry.publish(published.clone());
    let got = rx.recv().await.unwrap();
    assert_eq!(got, published);
}

#[tokio::test(flavor = "current_thread")]
async fn subscriber_count_reflects_active_receivers() {
    let entry = SessionEntry::new();
    assert_eq!(entry.subscriber_count(), 0);
    let rx1 = entry.subscribe();
    assert_eq!(entry.subscriber_count(), 1);
    let rx2 = entry.subscribe();
    assert_eq!(entry.subscriber_count(), 2);
    drop(rx1);
    // tokio::broadcast doesn't eagerly drop the slot on receiver
    // drop; the count is still 2 until the next publish forces
    // a cleanup pass. Accept this as API behavior — publish once
    // and recheck.
    entry.publish(text_frame("bump"));
    // After publish, recv + drain on rx2 so count stabilizes.
    drop(rx2);
    entry.publish(text_frame("bump2"));
    assert_eq!(entry.subscriber_count(), 0);
}

// ─────────────────────────────────────────────────────────────────────
// Registry integration — produce through registered entry, consume
// through lookup
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn publisher_and_consumer_meet_via_registry_lookup() {
    let id = SessionId::new();
    let producer = registry::register(id);
    let consumer_entry = registry::lookup(&id).expect("registered entry");
    let mut rx = consumer_entry.subscribe();
    producer.publish(text_frame("hello from producer"));
    // Small delay to let the broadcast pass settle.
    tokio::time::timeout(Duration::from_millis(100), async {
        let got = rx.recv().await.unwrap();
        match got {
            Frame::Text { bytes, .. } => {
                assert_eq!(bytes, b"hello from producer")
            }
            _ => panic!("wrong frame"),
        }
    })
    .await
    .expect("recv should not time out");
    registry::unregister(&id);
}

#[tokio::test(flavor = "current_thread")]
async fn late_subscriber_misses_live_but_sees_replay() {
    let id = SessionId::new();
    let entry = registry::register(id);
    // Publish 3 frames BEFORE the subscriber attaches.
    entry.publish(text_frame("pre1"));
    entry.publish(text_frame("pre2"));
    entry.publish(text_frame("pre3"));
    // Late subscribe.
    let snapshot = entry.replay_snapshot();
    let mut rx = entry.subscribe();
    // One more live frame.
    entry.publish(text_frame("live"));
    // Replay ring shows the 3 pre frames in order.
    assert_eq!(snapshot.len(), 3);
    // Live channel shows just the "live" frame (the subscriber
    // attached AFTER the pre frames).
    let got = rx.recv().await.unwrap();
    match got {
        Frame::Text { bytes, .. } => assert_eq!(bytes, b"live"),
        _ => panic!("wrong frame"),
    }
    assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
    registry::unregister(&id);
}
