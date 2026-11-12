//! The approval request store (migration 0303): states, limits, the
//! `approve.request` permission and who an agent may ask.

use super::*;
use rusqlite::Connection;

fn db() -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

const NOW: i64 = 1_800_000_000;

fn ask(ws: &str, person: &str, nonce: &str) -> NewRequest {
    NewRequest {
        workspace_id: ws.into(),
        session_id: Some("s-1".into()),
        person_id: person.into(),
        person_login: format!("login-{person}"),
        payload: format!("K2-SIGNED-APPROVAL-1\0{{\"nonce\":\"{nonce}\"}}").into_bytes(),
        kind: "k2.release".into(),
        rp: "k2sign@build".into(),
        nonce: nonce.into(),
        answer_by: NOW + 900,
    }
}

fn nonce(i: usize) -> String {
    format!("{i:032x}")
}

#[test]
fn ids_have_the_prefix_and_parse() {
    let id = new_id();
    assert!(id.starts_with("ap_") && id.len() == 19, "{id}");
    assert!(valid_id(&id));
    for bad in ["ap_", "ap_12", "pr_0123456789abcdef", "ap_0123456789ABCDEF", "ap_0123456789abcdeg"] {
        assert!(!valid_id(bad), "{bad}");
    }
}

#[test]
fn states_round_trip() {
    for s in State::ALL {
        assert_eq!(State::parse(s.as_str()), Some(s));
    }
    assert!(!State::Waiting.is_final());
    for s in [State::Approved, State::Declined, State::Expired, State::Cancelled] {
        assert!(s.is_final(), "{s:?}");
    }
}

#[test]
fn insert_keeps_the_bytes_exactly_and_reads_back() {
    let conn = db();
    let mut n = ask("ws-a", "u-1", &nonce(1));
    // Bytes that are not valid UTF-8 come back unchanged: the store never
    // re-writes a payload.
    n.payload.extend_from_slice(&[0xff, 0x00, 0xfe]);
    let r = insert(&conn, &n, NOW).unwrap();
    assert_eq!(r.state, State::Waiting);
    let back = get(&conn, &r.id).unwrap().expect("stored");
    assert_eq!(back.payload, n.payload);
    assert_eq!(back.payload_sha256, sha256_hex(&n.payload));
    assert_eq!(back.person_id, "u-1");
    assert_eq!(back.answer_by, NOW + 900);
    assert_eq!(back.record, None);
}

#[test]
fn a_nonce_is_carried_once_per_relying_party() {
    let conn = db();
    insert(&conn, &ask("ws-a", "u-1", &nonce(7)), NOW).unwrap();
    let e = insert(&conn, &ask("ws-a", "u-1", &nonce(7)), NOW).unwrap_err();
    assert_eq!(e.code, codes::DUPLICATE_NONCE);
}

#[test]
fn waiting_caps_per_person_and_per_agent() {
    let conn = db();
    for i in 0..limits::WAITING_PER_AGENT {
        insert(&conn, &ask("ws-a", "u-1", &nonce(i)), NOW).unwrap();
    }
    let e = insert(&conn, &ask("ws-a", "u-1", &nonce(100)), NOW).unwrap_err();
    assert_eq!(e.code, codes::TOO_MANY_WAITING);
    // Other agents fill the person's cap.
    let mut i = 200;
    let mut ws = 0;
    while waiting_for_person(&conn, "u-1", NOW).unwrap().len() < limits::WAITING_PER_PERSON {
        insert(&conn, &ask(&format!("ws-{ws}"), "u-1", &nonce(i)), NOW).unwrap();
        i += 1;
        ws += 1;
    }
    let e = insert(&conn, &ask("ws-new", "u-1", &nonce(999)), NOW).unwrap_err();
    assert_eq!(e.code, codes::TOO_MANY_WAITING);
    // Another person is not affected.
    insert(&conn, &ask("ws-new", "u-2", &nonce(1000)), NOW).unwrap();
}

#[test]
fn asks_per_hour_are_limited_per_agent() {
    let conn = db();
    for i in 0..limits::ASKS_PER_AGENT_PER_HOUR {
        let r = insert(&conn, &ask("ws-a", "u-1", &nonce(i)), NOW).unwrap();
        cancel(&conn, &r.id, "ws-a", NOW).unwrap();
    }
    let e = insert(&conn, &ask("ws-a", "u-1", &nonce(500)), NOW).unwrap_err();
    assert_eq!(e.code, codes::RATE_LIMITED);
    // An hour later the window has moved.
    insert(&conn, &ask("ws-a", "u-1", &nonce(501)), NOW + 3601).unwrap();
}

#[test]
fn approve_stores_the_record_as_posted_and_is_final() {
    let conn = db();
    let r = insert(&conn, &ask("ws-a", "u-1", &nonce(1)), NOW).unwrap();
    let record = r#"{"v":1,"payload_b64":"AA==","key_id":"k1","alg":"ES256","sig_b64":"BB=="}"#;
    let a = answer(
        &conn,
        &r.id,
        &Answer::Approve { record: record.into(), key_id: "k1".into(), alg: "ES256".into() },
        &Answered { by: "rosson".into(), via: "phone".into(), device: Some("iPhone".into()) },
        NOW + 10,
    )
    .unwrap();
    assert_eq!(a.state, State::Approved);
    assert_eq!(a.record.as_deref(), Some(record));
    assert_eq!(a.key_id.as_deref(), Some("k1"));
    assert_eq!(a.answered_by.as_deref(), Some("rosson"));
    assert_eq!(a.device.as_deref(), Some("iPhone"));
    // A second answer is refused: the first one stands.
    let e = answer(&conn, &r.id, &Answer::Decline, &Answered { by: "rosson".into(), via: "phone".into(), device: None }, NOW + 11)
        .unwrap_err();
    assert_eq!(e.code, codes::NOT_WAITING);
    assert_eq!(get(&conn, &r.id).unwrap().unwrap().state, State::Approved);
}

#[test]
fn an_answer_after_answer_by_is_refused_and_the_request_expires() {
    let conn = db();
    let r = insert(&conn, &ask("ws-a", "u-1", &nonce(1)), NOW).unwrap();
    let e = answer(&conn, &r.id, &Answer::Decline, &Answered { by: "rosson".into(), via: "phone".into(), device: None }, NOW + 900)
        .unwrap_err();
    assert_eq!(e.code, codes::EXPIRED);
    assert_eq!(get(&conn, &r.id).unwrap().unwrap().state, State::Expired);
}

#[test]
fn expire_due_turns_only_overdue_waiting_requests() {
    let conn = db();
    let a = insert(&conn, &ask("ws-a", "u-1", &nonce(1)), NOW).unwrap();
    let mut late = ask("ws-a", "u-1", &nonce(2));
    late.answer_by = NOW + 3000;
    let b = insert(&conn, &late, NOW).unwrap();
    let c = insert(&conn, &ask("ws-b", "u-1", &nonce(3)), NOW).unwrap();
    cancel(&conn, &c.id, "ws-b", NOW + 1).unwrap();
    let gone = expire_due(&conn, NOW + 900).unwrap();
    assert_eq!(gone.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), vec![a.id.clone()]);
    assert_eq!(get(&conn, &a.id).unwrap().unwrap().state, State::Expired);
    assert_eq!(get(&conn, &b.id).unwrap().unwrap().state, State::Waiting);
    assert_eq!(get(&conn, &c.id).unwrap().unwrap().state, State::Cancelled);
    assert!(expire_due(&conn, NOW + 900).unwrap().is_empty(), "a second sweep is a no-op");
}

#[test]
fn only_the_asking_workspace_cancels() {
    let conn = db();
    let r = insert(&conn, &ask("ws-a", "u-1", &nonce(1)), NOW).unwrap();
    let e = cancel(&conn, &r.id, "ws-b", NOW).unwrap_err();
    assert_eq!(e.code, codes::NOT_FOUND, "another workspace's request is not found, never cancelled");
    assert_eq!(cancel(&conn, &r.id, "ws-a", NOW).unwrap().state, State::Cancelled);
    assert_eq!(cancel(&conn, &r.id, "ws-a", NOW).unwrap_err().code, codes::NOT_WAITING);
}

#[test]
fn the_permission_is_off_until_granted() {
    let conn = db();
    assert!(!agent_may_request(&conn, "ws-a").unwrap());
    set_agent(&conn, "ws-a", true, "owner", NOW).unwrap();
    assert!(agent_may_request(&conn, "ws-a").unwrap());
    assert!(!agent_may_request(&conn, "ws-b").unwrap());
    set_agent(&conn, "ws-a", false, "owner", NOW).unwrap();
    assert!(!agent_may_request(&conn, "ws-a").unwrap());
}

#[test]
fn who_an_agent_may_ask() {
    let conn = db();
    // Default: only the person it works for.
    assert!(may_ask(&conn, "u-owner", "ws-a", true).unwrap());
    assert!(!may_ask(&conn, "u-gina", "ws-a", false).unwrap());
    // Someone else allows it, for themselves.
    set_rule(&conn, "u-gina", "ws-a", Some(Rule::Allow), NOW).unwrap();
    assert!(may_ask(&conn, "u-gina", "ws-a", false).unwrap());
    assert!(!may_ask(&conn, "u-gina", "ws-b", false).unwrap(), "per agent");
    // The owner blocks it: the default no longer applies.
    set_rule(&conn, "u-owner", "ws-a", Some(Rule::Block), NOW).unwrap();
    assert!(!may_ask(&conn, "u-owner", "ws-a", true).unwrap());
    assert_eq!(rule_for(&conn, "u-owner", "ws-a").unwrap(), Some(Rule::Block));
    // Clearing the rule returns to the default.
    set_rule(&conn, "u-owner", "ws-a", None, NOW).unwrap();
    assert!(may_ask(&conn, "u-owner", "ws-a", true).unwrap());
}

#[test]
fn waiting_for_person_skips_overdue_and_answered() {
    let conn = db();
    let a = insert(&conn, &ask("ws-a", "u-1", &nonce(1)), NOW).unwrap();
    let mut short = ask("ws-a", "u-1", &nonce(2));
    short.answer_by = NOW + 5;
    insert(&conn, &short, NOW).unwrap();
    insert(&conn, &ask("ws-a", "u-2", &nonce(3)), NOW).unwrap();
    let ids: Vec<String> = waiting_for_person(&conn, "u-1", NOW + 10).unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![a.id]);
}
