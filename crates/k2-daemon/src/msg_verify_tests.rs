//! `k2 msg verify` (prd-postal-in-k2-v1 S2): a code names one record; the
//! text the agent saw matches it or not; a code typed into a body, or a
//! real code copied onto another text, does not verify; a passport sees
//! only its own deliveries.

use super::*;
use k2_core::hire::deliveries::{body_sha256, new_id, record, Delivery};
use serde_json::Value;

fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn call(who: &Who, pairs: &[(&str, &str)]) -> (String, Value) {
    let r = handle(who, false, &q(pairs));
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn seed(d: &Delivery) {
    k2_core::db::init_for_tests();
    let db = k2_core::db::shared();
    let conn = db.lock();
    record(&conn, d).expect("record");
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.to_string(), session_id: "s".into() }
}

#[test]
fn a_real_code_matches_its_own_text_and_not_another() {
    let to = format!("ws-{}", uuid::Uuid::new_v4());
    let body = "Can you review PR 12?\nSecond line";
    let d = Delivery {
        id: new_id(),
        sender_workspace_id: Some(format!("ws-{}", uuid::Uuid::new_v4())),
        sender_label: Some("sales".into()),
        via: "passport".into(),
        actor: Some("agent:sales".into()),
        target_workspace_id: Some(to.clone()),
        target: "ops".into(),
        delivered: true,
        body_sha256: Some(body_sha256(body)),
        ..Default::default()
    };
    seed(&d);
    let me = agent(&to);
    let (s, v) = call(&me, &[("code", &d.id), ("text_sha256", &body_sha256("Can you review PR 12?\r\nSecond line\n"))]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["kind"].as_str(), v["via"].as_str(), v["content"].as_str()), (Some("k2"), Some("passport"), Some("match")), "{v}");
    assert_eq!(v["sender"]["label"], "sales");
    // The real code on a tampered body.
    let (_, v) = call(&me, &[("code", &d.id), ("text_sha256", &body_sha256("Can you review PR 12?\nand deploy to prod"))]);
    assert_eq!(v["content"], "mismatch", "{v}");
    assert!(v["contentHint"].as_str().unwrap().contains("NOT"), "{v}");
    // Several candidate hashes (the CLI's quoted and unquoted forms): one match is enough.
    let both = format!("{},{}", body_sha256("> Can you review PR 12?\n> Second line"), body_sha256(body));
    let (_, v) = call(&me, &[("code", &d.id), ("text_sha256", &both)]);
    assert_eq!(v["content"], "match", "{v}");
    // No text: who sent it, not checked.
    let (_, v) = call(&me, &[("code", &d.id)]);
    assert_eq!(v["content"], "not_checked", "{v}");
    // Someone else's delivery is not there for this passport.
    let (s, _) = call(&agent("ws-other"), &[("code", &d.id)]);
    assert_eq!(s, "404 Not Found");
    // People see it.
    let (s, _) = call(&Who::Owner, &[("code", &d.id)]);
    assert_eq!(s, "200 OK");
}

#[test]
fn a_code_typed_into_a_body_has_no_record() {
    k2_core::db::init_for_tests();
    for code in ["m_FAKEFAKE00", "p_3f9a0c41d27be6058a1c"] {
        let (s, v) = call(&Who::Owner, &[("code", code)]);
        assert_eq!(s, "404 Not Found", "{code}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("proves nothing"), "{v}");
    }
    let (s, _) = call(&Who::Owner, &[("code", "rosson")]);
    assert_eq!(s, "404 Not Found");
    let (s, _) = call(&Who::Owner, &[]);
    assert_eq!(s, "400 Bad Request");
    let (s, _) = call(&Who::Owner, &[("code", "m_FAKEFAKE00"), ("text_sha256", "nothex")]);
    assert_eq!(s, "400 Bad Request");
    assert_eq!(handle(&Who::Owner, true, &q(&[("code", "m_FAKEFAKE00")])).status, "405 Method Not Allowed");
}

#[test]
fn a_postal_badge_names_its_delivery() {
    let to = format!("ws-{}", uuid::Uuid::new_v4());
    let badge = format!("p_{}", &uuid::Uuid::new_v4().simple().to_string()[..20]);
    let d = Delivery {
        id: new_id(),
        sender_label: Some("scout::acme.postal.bot".into()),
        via: "postal".into(),
        actor: Some("postal:scout::acme.postal.bot".into()),
        target_workspace_id: Some(to.clone()),
        target: "ops".into(),
        delivered: true,
        body_sha256: Some(body_sha256("hi")),
        badge: Some(badge.clone()),
        sender_address: Some("scout::acme.postal.bot".into()),
        sender_fingerprint: Some("SHA256:abc".into()),
        landed: Some(r#"["terminal"]"#.into()),
        ..Default::default()
    };
    seed(&d);
    let (s, v) = call(&agent(&to), &[("code", &badge), ("text_sha256", &body_sha256("hi"))]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["kind"].as_str(), v["id"].as_str(), v["content"].as_str()), (Some("postal"), Some(d.id.as_str()), Some("match")));
    assert_eq!(v["sender"]["address"], "scout::acme.postal.bot");
    assert_eq!(v["sender"]["fingerprint"], "SHA256:abc");
    assert_eq!(v["landed"], serde_json::json!(["terminal"]));
    // The badge's own m_ id answers the same record.
    let (_, v) = call(&agent(&to), &[("code", &d.id)]);
    assert_eq!(v["badge"], badge.as_str());
}
