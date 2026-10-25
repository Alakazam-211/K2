//! Payload rules (CP26–CP27), the 4 KB fit (A7) and the wire shapes (§8.3).

use super::*;
use serde_json::json;

fn stamps() -> Stamps {
    Stamps { app: "svc-1".into(), send_id: new_send_id(), kind: KIND_APP, who: "ab".repeat(16) }
}

fn usage_of(v: J) -> String {
    match parse(&v) {
        Err(Refused::Usage(m)) => m,
        other => panic!("expected usage for {v}, got {other:?}"),
    }
}

#[test]
fn a_full_request_parses_with_defaults() {
    let n = parse(&json!({"to": {"guest": "dana"}, "title": " Hi ", "body": "There"})).expect("minimal");
    assert_eq!((n.title.as_deref(), n.body.as_deref()), (Some("Hi"), Some("There")));
    assert!(n.high_priority && !n.silent && !n.mutable);
    assert_eq!((n.ttl_secs, n.badge, n.sound.clone()), (DEFAULT_TTL_SECS, None, None));
    let n = parse(&json!({"silent": true, "data": {"sync": "1"}})).expect("silent");
    assert!(n.silent && !n.high_priority && n.title.is_none());
    let n = parse(&json!({"title": "t", "body": "b", "sound": "none", "badge": 0, "ttlSecs": 0, "priority": "normal", "mutable": true, "androidChannel": "jobs"}))
        .expect("all");
    assert_eq!((n.sound.clone(), n.badge, n.ttl_secs, n.high_priority, n.mutable), (None, Some(0), 0, false, true));
    let secret = parse(&json!({"title": "SECRET-T", "body": "SECRET-B", "data": {"k": "SECRET-D"}})).expect("valid");
    let dbg = format!("{secret:?}");
    assert!(!dbg.contains("SECRET"), "Debug carries no text: {dbg}");
}

#[test]
fn every_rule_names_its_field_and_never_echoes_text() {
    let secret = "SECRET-TEXT-123";
    let cases = [
        (json!({"body": "b"}), "title and body are required"),
        (json!({"title": "t"}), "title and body are required"),
        (json!({"title": "x".repeat(201), "body": "b"}), "title is at most 200"),
        (json!({"title": "t", "body": "x".repeat(1025)}), "body is at most 1024"),
        (json!({"silent": true, "title": secret}), "carries no title"),
        (json!({"silent": true, "priority": "high"}), "normal priority"),
        (json!({"title": "t", "body": "b", "data": {"n": 1}}), "data values are strings"),
        (json!({"title": "t", "body": "b", "data": {"aps": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"k2Who": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"K2app": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"google.c": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"gcm.n": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"from": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"collapse_key": "x"}}), "reserved"),
        (json!({"title": "t", "body": "b", "data": {"bad key": "x"}}), "data key"),
        (json!({"title": "t", "body": "b", "data": {"k": "x".repeat(1025)}}), "data value is at most"),
        (json!({"title": "t", "body": "b", "topic": "Jobs"}), "topic"),
        (json!({"title": "t", "body": "b", "collapseId": "x".repeat(65)}), "collapseId is at most 64"),
        (json!({"title": "t", "body": "b", "threadId": "x".repeat(257)}), "threadId is at most 256"),
        (json!({"title": "t", "body": "b", "sound": "x".repeat(65)}), "sound"),
        (json!({"title": "t", "body": "b", "badge": 100000}), "badge"),
        (json!({"title": "t", "body": "b", "badge": -1}), "badge"),
        (json!({"title": "t", "body": "b", "ttlSecs": 2419201}), "ttlSecs"),
        (json!({"title": "t", "body": "b", "priority": "urgent"}), "priority"),
        (json!({"title": "t", "body": "b", "androidChannel": "a b"}), "androidChannel"),
        (json!({"title": "t", "body": "b", "app": "other"}), "unknown field app"),
        (json!({"title": "t", "body": "b", "principalId": "x"}), "unknown field principalId"),
    ];
    for (v, want) in cases {
        let m = usage_of(v.clone());
        assert!(m.contains(want), "{m:?} should say {want:?} for {v}");
        assert!(!m.contains(secret), "never echoes: {m}");
    }
    let mut big = serde_json::Map::new();
    for i in 0..33 {
        big.insert(format!("k{i:02}x").replace("k", "d"), json!("v"));
    }
    assert!(usage_of(json!({"title": "t", "body": "b", "data": big})).contains("at most 32 keys"));
    let mut heavy = serde_json::Map::new();
    for i in 0..3 {
        heavy.insert(format!("d{i}"), json!("x".repeat(1000)));
    }
    assert!(usage_of(json!({"title": "t", "body": "b", "data": heavy})).contains("at most 2048 bytes"));
}

#[test]
fn the_payloads_must_fit_four_kilobytes() {
    let mut data = serde_json::Map::new();
    for i in 0..2 {
        data.insert(format!("d{i}"), json!("x".repeat(1000)));
    }
    let v = json!({"title": "x".repeat(200), "body": "y".repeat(1024), "data": data, "threadId": "z".repeat(256)});
    assert!(parse(&v).is_ok(), "under the limit");
    let mut data = serde_json::Map::new();
    for i in 0..2 {
        data.insert(format!("d{i}"), json!("é".repeat(500)));
    }
    let v = json!({"title": "€".repeat(66), "body": "€".repeat(341), "data": data, "threadId": "z".repeat(256), "collapseId": "c".repeat(64)});
    let first = parse(&v);
    assert!(matches!(first, Ok(_) | Err(Refused::TooLarge { limit: MAX_PAYLOAD_BYTES })), "{first:?}");
    // Escaped characters grow on the wire: control characters count six bytes.
    let v = json!({"title": "\u{1}".repeat(200), "body": "\u{1}".repeat(1024), "threadId": "z".repeat(256)});
    assert_eq!(parse(&v).expect_err("too large"), Refused::TooLarge { limit: MAX_PAYLOAD_BYTES });
}

#[test]
fn apns_and_fcm_see_the_text_the_data_and_k2s_stamps_only() {
    let n = parse(&json!({"title": "T", "body": "B", "data": {"jobId": "j_42"}, "topic": "jobs", "collapseId": "job:j_42", "threadId": "job:j_42", "sound": "default", "badge": 3, "mutable": true}))
        .expect("valid");
    let s = stamps();
    let a: J = serde_json::from_slice(&apns_body(&n, &s)).expect("json");
    assert_eq!(
        a,
        json!({"aps": {"alert": {"title": "T", "body": "B"}, "sound": "default", "badge": 3, "thread-id": "job:j_42", "mutable-content": 1},
               "jobId": "j_42", "k2App": "svc-1", "k2Send": s.send_id, "k2Kind": "app", "k2Topic": "jobs", "k2Who": "ab".repeat(16)})
    );
    let h = apns_headers(&n, "com.example.app", 1000);
    assert_eq!(
        h,
        vec![
            ("apns-topic".to_string(), "com.example.app".to_string()),
            ("apns-push-type".to_string(), "alert".to_string()),
            ("apns-priority".to_string(), "10".to_string()),
            ("apns-expiration".to_string(), (1000 + DEFAULT_TTL_SECS as i64).to_string()),
            ("apns-collapse-id".to_string(), "job:j_42".to_string()),
        ]
    );
    let f: J = serde_json::from_slice(&fcm_body(&n, &s, "tok", 1000)).expect("json");
    let m = &f["message"];
    assert_eq!(m["token"], "tok");
    assert_eq!(m["notification"], json!({"title": "T", "body": "B"}));
    assert_eq!(m["data"]["jobId"], "j_42");
    assert_eq!(m["android"], json!({"priority": "HIGH", "ttl": "86400s", "collapse_key": "job:j_42",
        "notification": {"sound": "default", "tag": "job:j_42", "notification_count": 3}}));
    assert!(m["apns"]["headers"].get("apns-topic").is_none(), "FCM sets the topic from the iOS app");
    assert_eq!(m["apns"]["payload"]["aps"]["thread-id"], "job:j_42");

    let silent = parse(&json!({"silent": true, "data": {"sync": "1"}})).expect("silent");
    let a: J = serde_json::from_slice(&apns_body(&silent, &s)).expect("json");
    assert_eq!(a["aps"], json!({"content-available": 1}));
    let h = apns_headers(&silent, "com.example.app", 1000);
    assert!(h.contains(&("apns-push-type".into(), "background".into())) && h.contains(&("apns-priority".into(), "5".into())));
    let f: J = serde_json::from_slice(&fcm_body(&silent, &s, "tok", 1000)).expect("json");
    assert!(f["message"].get("notification").is_none(), "data-only on FCM");
    assert_eq!(f["message"]["android"]["priority"], "NORMAL");
    assert_eq!(new_send_id().len(), SEND_ID_LEN);
}
