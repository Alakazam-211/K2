//! Idempotent sends (0.46.15): the ledger itself. Every test builds its own
//! [`Ledger`] (no shared state), except the `once` ones, which use a fresh
//! random key on the process ledger.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn id(route: &str, sender: &str, key: &str) -> Id {
    Id { route: route.into(), sender: sender.into(), key: key.into() }
}

/// A send that counts its deliveries and answers a fresh delivery id each
/// time it runs.
fn deliver(count: &AtomicUsize) -> CliResponse {
    let n = count.fetch_add(1, Ordering::SeqCst) + 1;
    CliResponse::ok_json(serde_json::json!({ "success": true, "deliveryId": format!("m_{n}") }).to_string())
}

fn body(r: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("answer is not JSON ({e}): {}", r.body))
}

fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn a_repeat_key_answers_the_first_answer_and_delivers_once() {
    let ledger = Ledger::new(TTL, IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let k = key();
    let first = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    let again = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 1, "a repeat must not deliver again");
    assert_eq!(first.status, "200 OK");
    assert_eq!(again.status, "200 OK");
    let (first, again) = (body(&first), body(&again));
    assert_eq!(first["deliveryId"], "m_1", "{first}");
    assert_eq!(again["deliveryId"], "m_1", "the repeat carries the first delivery id: {again}");
    assert!(first.get("replayed").is_none(), "a first answer is no replay: {first}");
    assert_eq!(again["replayed"], true, "{again}");
}

#[test]
fn different_keys_deliver_twice() {
    let ledger = Ledger::new(TTL, IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let a = ledger.run(id("/cli/workspace/msg", "a:ws1", &key()), || deliver(&count));
    let b = ledger.run(id("/cli/workspace/msg", "a:ws1", &key()), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(body(&a)["deliveryId"], "m_1");
    assert_eq!(body(&b)["deliveryId"], "m_2");
    assert!(body(&b).get("replayed").is_none());
}

#[test]
fn another_senders_key_does_not_collide() {
    let ledger = Ledger::new(TTL, IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let k = key();
    let mine = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    let theirs = ledger.run(id("/cli/workspace/msg", "a:ws2", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2, "the same key from another sender is its own send");
    assert_eq!(body(&mine)["deliveryId"], "m_1");
    assert_eq!(body(&theirs)["deliveryId"], "m_2");
    assert!(body(&theirs).get("replayed").is_none(), "{}", theirs.body);
}

#[test]
fn the_same_key_on_another_route_is_its_own_send() {
    let ledger = Ledger::new(TTL, IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let k = key();
    ledger.run(id("/cli/inbox/compose", "a:ws1", &k), || deliver(&count));
    ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn a_refusal_frees_the_key() {
    let ledger = Ledger::new(TTL, IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let k = key();
    let refuse = |count: &AtomicUsize| {
        count.fetch_add(1, Ordering::SeqCst);
        CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":{"code":"not_connected","hint":"x"}}"#.to_string(),
        }
    };
    let first = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || refuse(&count));
    assert_eq!(first.status, "403 Forbidden");
    // Nothing was delivered, so the same key runs again (and may land).
    let again = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(again.status, "200 OK", "{}", again.body);
    assert!(body(&again).get("replayed").is_none(), "{}", again.body);
    // Now it landed: kept.
    let third = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(body(&third)["deliveryId"], body(&again)["deliveryId"]);
}

#[test]
fn a_not_delivered_answer_is_kept() {
    let ledger = Ledger::new(TTL, IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let k = key();
    let waiting = |count: &AtomicUsize| {
        count.fetch_add(1, Ordering::SeqCst);
        CliResponse::ok_json(r#"{"success":false,"reason":"folder_trust_pending","deliveryId":"m_q"}"#.to_string())
    };
    ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || waiting(&count));
    let again = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || waiting(&count));
    assert_eq!(count.load(Ordering::SeqCst), 1, "a queued send is not queued twice");
    assert_eq!(body(&again)["reason"], "folder_trust_pending");
    assert_eq!(body(&again)["replayed"], true);
}

#[test]
fn a_repeat_while_the_first_runs_waits_for_its_answer() {
    let ledger = Arc::new(Ledger::new(TTL, Duration::from_secs(10)));
    let count = Arc::new(AtomicUsize::new(0));
    let k = key();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let first = {
        let (ledger, count, k) = (ledger.clone(), count.clone(), k.clone());
        std::thread::spawn(move || {
            ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || {
                started_tx.send(()).expect("signal start");
                std::thread::sleep(Duration::from_millis(400));
                deliver(&count)
            })
        })
    };
    started_rx.recv_timeout(Duration::from_secs(5)).expect("the first try started");
    let again = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    let first = first.join().expect("first try");
    assert_eq!(count.load(Ordering::SeqCst), 1, "the waiting repeat must not deliver");
    assert_eq!(body(&first)["deliveryId"], "m_1");
    assert_eq!(body(&again)["deliveryId"], "m_1", "{}", again.body);
    assert_eq!(body(&again)["replayed"], true);
}

#[test]
fn a_repeat_past_the_wait_answers_in_progress_and_still_delivers_nothing() {
    let ledger = Arc::new(Ledger::new(TTL, Duration::from_millis(100)));
    let count = Arc::new(AtomicUsize::new(0));
    let k = key();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
    let first = {
        let (ledger, count, k) = (ledger.clone(), count.clone(), k.clone());
        std::thread::spawn(move || {
            ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || {
                started_tx.send(()).expect("signal start");
                finish_rx.recv_timeout(Duration::from_secs(5)).expect("told to finish");
                deliver(&count)
            })
        })
    };
    started_rx.recv_timeout(Duration::from_secs(5)).expect("the first try started");
    let busy = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(busy.status, "409 Conflict", "{}", busy.body);
    assert_eq!(body(&busy)["error"]["code"], "in_progress");
    assert_eq!(body(&busy)["idempotencyKey"], k.as_str());
    finish_tx.send(()).expect("finish");
    first.join().expect("first try");
    let after = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(body(&after)["deliveryId"], "m_1");
}

#[test]
fn a_kept_answer_goes_after_the_ttl() {
    let ledger = Ledger::new(Duration::from_millis(50), IN_FLIGHT_WAIT);
    let count = AtomicUsize::new(0);
    let k = key();
    ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    std::thread::sleep(Duration::from_millis(120));
    let later = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2, "past the TTL the key is forgotten");
    assert_eq!(body(&later)["deliveryId"], "m_2");
}

#[test]
fn a_send_that_panics_frees_its_key() {
    let ledger = Ledger::new(TTL, Duration::from_millis(100));
    let count = AtomicUsize::new(0);
    let k = key();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || panic!("send blew up"));
    }));
    assert!(caught.is_err(), "the send's panic reaches the caller");
    let next = ledger.run(id("/cli/workspace/msg", "a:ws1", &k), || deliver(&count));
    assert_eq!(next.status, "200 OK", "{}", next.body);
    assert_eq!(count.load(Ordering::SeqCst), 1, "the key runs again, no in_progress");
}

#[test]
fn once_without_a_key_runs_every_time() {
    let count = AtomicUsize::new(0);
    let params = HashMap::new();
    once("/cli/workspace/msg", &params, "owner", || deliver(&count));
    once("/cli/workspace/msg", &params, "owner", || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn once_refuses_a_malformed_key_without_sending() {
    let count = AtomicUsize::new(0);
    let long = "x".repeat(65);
    for bad in ["short", "has space in it", "semi;colon-key", long.as_str()] {
        let params = HashMap::from([(PARAM.to_string(), bad.to_string())]);
        let r = once("/cli/workspace/msg", &params, "owner", || deliver(&count));
        assert_eq!(r.status, "400 Bad Request", "{bad}: {}", r.body);
        assert_eq!(body(&r)["error"]["code"], "usage");
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[test]
fn once_keys_by_the_stamped_passport_not_the_body() {
    let count = AtomicUsize::new(0);
    let k = key();
    let stamped = |ws: &str| {
        HashMap::from([
            (PARAM.to_string(), k.clone()),
            (crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string()),
            ("project_id".to_string(), ws.to_string()),
        ])
    };
    let a1 = once("/cli/workspace/msg", &stamped("ws-a"), "owner", || deliver(&count));
    let a2 = once("/cli/workspace/msg", &stamped("ws-a"), "owner", || deliver(&count));
    let b = once("/cli/workspace/msg", &stamped("ws-b"), "owner", || deliver(&count));
    // The owner token with the same key is another sender too.
    let owner = once("/cli/workspace/msg", &HashMap::from([(PARAM.to_string(), k.clone())]), "owner", || deliver(&count));
    assert_eq!(count.load(Ordering::SeqCst), 3, "ws-a once, ws-b once, the owner once");
    assert_eq!(body(&a1)["deliveryId"], body(&a2)["deliveryId"]);
    assert_ne!(body(&a1)["deliveryId"], body(&b)["deliveryId"]);
    assert_ne!(body(&b)["deliveryId"], body(&owner)["deliveryId"]);
}

#[test]
fn mark_replayed_keeps_the_answer_bytes() {
    assert_eq!(mark_replayed(r#"{"a":1}"#), r#"{"a":1,"replayed":true}"#);
    assert_eq!(mark_replayed("{}"), r#"{"replayed":true}"#);
    assert_eq!(mark_replayed("plain text"), "plain text");
}

#[test]
fn the_federation_uuid_is_stable_per_sender_kind_and_key() {
    let a = federation_msg_uuid("a:ws1", "msg", "k-0000000001");
    assert_eq!(a, federation_msg_uuid("a:ws1", "msg", "k-0000000001"));
    assert_ne!(a, federation_msg_uuid("a:ws2", "msg", "k-0000000001"));
    assert_ne!(a, federation_msg_uuid("a:ws1", "tray", "k-0000000001"));
    assert_ne!(a, federation_msg_uuid("a:ws1", "msg", "k-0000000002"));
    let parsed = uuid::Uuid::parse_str(&a).unwrap_or_else(|e| panic!("{a} is not a UUID: {e}"));
    assert_eq!(parsed.get_version_num(), 5, "{a}");
}
