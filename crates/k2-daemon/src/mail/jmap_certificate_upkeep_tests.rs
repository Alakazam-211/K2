use super::*;

#[test]
fn certificate_destroy_reply_gone_is_ok_refusal_is_loud() {
    let ok = serde_json::json!({ "destroyed": ["c1"] });
    parse_certificate_destroyed("c1", &ok).expect("destroyed");
    let gone = serde_json::json!({ "notDestroyed": { "c1": { "type": "notFound" } } });
    parse_certificate_destroyed("c1", &gone).expect("already gone");
    let refused = serde_json::json!({
        "notDestroyed": { "c1": { "type": "forbidden", "description": "in use" } }
    });
    let err = parse_certificate_destroyed("c1", &refused).expect_err("refused");
    assert!(err.contains("forbidden") && err.contains("in use"), "{err}");
    let err = parse_certificate_destroyed("c1", &serde_json::json!({})).expect_err("silent");
    assert!(err.contains("not in the destroyed list"), "{err}");
}

#[test]
fn certificate_list_reads_both_shapes_and_refuses_half_an_answer() {
    let reply = serde_json::json!({ "list": [
        {
            "id": "c1",
            "subjectAlternativeNames": { "mail.example.com": true, "off.example.com": false },
            "notValidBefore": "2026-09-18T00:00:00Z",
            "notValidAfter": "2026-12-17T00:00:00Z",
            "issuer": "Let's Encrypt",
        },
        {
            "id": "c2",
            "subjectAlternativeNames": ["mail.example.org"],
            "notValidBefore": "2026-09-19T00:00:00Z",
            "notValidAfter": "2026-12-18T00:00:00Z",
        },
    ] });
    let list = parse_certificate_list(&reply).expect("parse");
    assert_eq!(list[0].id, "c1");
    assert_eq!(list[0].names, vec!["mail.example.com".to_string()]);
    assert_eq!(
        list[0].not_after,
        chrono::DateTime::parse_from_rfc3339("2026-12-17T00:00:00Z").unwrap().timestamp()
    );
    assert_eq!(list[0].issuer, "Let's Encrypt");
    assert_eq!(list[1].names, vec!["mail.example.org".to_string()]);
    assert_eq!(list[1].issuer, "");

    let no_list = parse_certificate_list(&serde_json::json!({})).expect_err("no list");
    assert!(no_list.contains("no list"), "{no_list}");
    let bad_date = serde_json::json!({ "list": [{
        "id": "c1",
        "subjectAlternativeNames": {},
        "notValidBefore": "2026-09-18T00:00:00Z",
        "notValidAfter": 12,
    }] });
    let err = parse_certificate_list(&bad_date).expect_err("bad date");
    assert!(err.contains("'c1'") && err.contains("notValidAfter"), "{err}");
    let no_names = serde_json::json!({ "list": [{
        "id": "c1",
        "notValidBefore": "2026-09-18T00:00:00Z",
        "notValidAfter": "2026-12-17T00:00:00Z",
    }] });
    let err = parse_certificate_list(&no_names).expect_err("no names");
    assert!(err.contains("subjectAlternativeNames"), "{err}");
}
