//! TC-20…TC-22: link files as an App setting, in process. They answer with
//! pairing off; with pairing on both parts merge into one AASA; a
//! pairing-only App keeps today's bytes exactly; query patterns go into
//! `components`; shipped paths are frozen; raw or non-runtime services and
//! host are refused; the App's agent needs "Build and run Apps".

use std::collections::HashMap;

use k2_core::skin::app_pairing as store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;
use crate::app_pairing::guest_routes::tests::{fx, turn_on};
use crate::app_pairing::links::{answer, AASA, ASSETLINKS};

const FPR: &str = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89";

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn call(path: &str, who: &Who, body: J) -> R<J> {
    handle(path, who, body.to_string().as_bytes(), &HashMap::new())
}

fn status_of(who: &Who, app: &str) -> R<J> {
    let p: HashMap<String, String> = [("app".to_string(), app.to_string())].into_iter().collect();
    handle(STATUS, who, b"", &p)
}

fn refused(r: R<J>) -> (u16, &'static str) {
    let e = r.expect_err("refused");
    (e.status, e.code)
}

fn build_switch(ws: &str, on: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2 WHERE id = ?1", params![ws, on as i64]).expect("switch");
}

/// The body K2 answers for `path` on `host` (`None`: the App answers).
fn served(app_id: &str, path: &str, host: &str) -> Option<String> {
    let bytes = answer(app_id, "GET", path, Some(host))?;
    let text = String::from_utf8(bytes).expect("utf8");
    Some(text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).expect("a body"))
}

fn set_body(app: &str, paths: J, drop: J) -> J {
    json!({
        "app": app,
        "ios": ["TEAM123456.com.jobs.app"],
        "android": [{"package": "com.jobs.app", "sha256": [FPR]}],
        "paths": paths,
        "drop": drop,
    })
}

#[test]
fn links_answer_without_pairing_merge_with_it_and_pairing_alone_keeps_its_bytes() {
    let f = fx("lk1");
    let host = format!("{}.box.k2.dev", f.app_id);
    crate::app_pairing::refresh_links(&f.app);
    let pairing_only = served(&f.app_id, AASA, &host).expect("pairing serves its AASA");
    // Pairing off: nothing.
    store::set_off(&f.app, "owner").unwrap();
    crate::app_pairing::refresh_links(&f.app);
    assert!(served(&f.app_id, AASA, &host).is_none());
    // The setting alone: both files, no pairing path, the query pattern in
    // `components` and the path alone in `paths` (TA-21).
    let v = call(SET, &Who::Owner, set_body(&f.app, json!(["/messages?addr=*", "/tickets/*"]), json!([]))).expect("set");
    assert_eq!(v["state"], "on", "{v}");
    assert_eq!(v["pairing"], false);
    assert_eq!(v["paths"][0]["spec"], "/messages?addr=*");
    assert!(v["paths"][0]["since"].as_i64().is_some(), "{v}");
    let aasa: J = serde_json::from_str(&served(&f.app_id, AASA, &host).expect("AASA with pairing off")).unwrap();
    let d = &aasa["applinks"]["details"][0];
    assert_eq!(d["appID"], "TEAM123456.com.jobs.app");
    assert_eq!(d["paths"], json!(["/messages", "/tickets/*"]));
    assert_eq!(d["components"], json!([{"/": "/messages", "?": {"addr": "*"}}, {"/": "/tickets/*"}]));
    let al: J = serde_json::from_str(&served(&f.app_id, ASSETLINKS, &host).expect("assetlinks")).unwrap();
    assert_eq!(al[0]["target"]["package_name"], "com.jobs.app");
    assert!(served(&f.app_id, store::PAIR_PATH, &host).is_none(), "no pairing page without pairing");
    assert!(served(&f.app_id, AASA, "other.box.k2.dev").is_none(), "only the App's own hosts");
    // Pairing on too: one AASA naming both apps, the pairing path first.
    turn_on(&f.app, &[]);
    crate::app_pairing::refresh_links(&f.app);
    let aasa: J = serde_json::from_str(&served(&f.app_id, AASA, &host).unwrap()).unwrap();
    let ids: Vec<&str> = aasa["applinks"]["details"].as_array().unwrap().iter().map(|d| d["appID"].as_str().unwrap()).collect();
    assert_eq!(ids, ["ABCDE12345.com.you.app", "TEAM123456.com.jobs.app"]);
    assert_eq!(aasa["applinks"]["details"][0]["paths"], json!(["/.well-known/k2-pair", "/messages", "/tickets/*"]));
    // The setting off: pairing's own file, byte for byte (TA-22).
    let v = call(OFF, &Who::Owner, json!({"app": f.app})).expect("off");
    assert_eq!(v["wasOn"], true);
    assert_eq!(served(&f.app_id, AASA, &host).unwrap(), pairing_only, "pairing-only bytes never change");
}

#[test]
fn shipped_paths_are_frozen_and_only_the_apps_agent_with_the_switch_changes_them() {
    let f = fx("lk2");
    let (st, code) = refused(call(SET, &agent(&f.ws), set_body(&f.app, json!(["/a/*"]), json!([]))));
    assert_eq!((st, code), (403, "forbidden"), "the switch is off");
    build_switch(&f.ws, true);
    call(SET, &agent(&f.ws), set_body(&f.app, json!(["/a/*", "/b/*"]), json!([]))).expect("the App's agent sets");
    assert_eq!(refused(status_of(&agent("ot-lk2"), &f.app)).0, 404, "another workspace's agent never sees it");
    let e = call(SET, &agent(&f.ws), set_body(&f.app, json!(["/a/*"]), json!([]))).expect_err("a shipped path dropped");
    assert_eq!((e.status, e.code), (409, LINK_PATH_DROPPED));
    assert_eq!(e.extra.as_ref().unwrap()["dropped"], json!(["/b/*"]));
    assert!(e.hint.contains("--drop '/b/*'"), "{}", e.hint);
    let v = call(SET, &agent(&f.ws), set_body(&f.app, json!(["/a/*"]), json!(["/b/*"]))).expect("dropped on purpose");
    assert_eq!(v["paths"].as_array().unwrap().len(), 1);
    // Every change is in the App's deploy audit.
    let events: Vec<String> = {
        let h = k2_core::db::shared();
        let c = h.lock();
        k2_core::deploy::audit::list_for(&c, k2_core::deploy::audit::SUBJECT_APP, &f.app_id, 50).unwrap().into_iter().map(|r| r.event).collect()
    };
    assert_eq!(events.iter().filter(|e| *e == "links_set").count(), 2, "{events:?}");
    // Not a runtime App (a --cmd service), host, and the internet check.
    let (st, code) = refused(call(SET, &Who::Owner, set_body(&f.other_app, json!(["/a/*"]), json!([]))));
    assert_eq!((st, code), (409, NEEDS_K2_FRONT));
    assert_eq!(refused(status_of(&Who::Owner, "host")), (400, "not_for_host"));
    assert_eq!(refused(call(CHECK, &agent(&f.ws), json!({"app": f.app}))), (403, "owner_only"));
    assert_eq!(refused(call(SET, &Who::Owner, json!({"app": f.app, "paths": ["/x"]}))).1, "usage", "a phone app is needed");
}
