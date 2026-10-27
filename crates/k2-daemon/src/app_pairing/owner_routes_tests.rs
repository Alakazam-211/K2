//! `/cli/app/pairing*` in process (S3): the owner in person with a fresh
//! confirm sets the phone apps (values checked first), the App's agent
//! reads and turns off, the log follows the App-user switch, the check
//! maps a fake fetcher's answers, an App's removal takes its config.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::app_pairing as store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;
use crate::app_pairing::guest_routes::tests::{fx, pass_for, start_call, Fx};
use crate::app_pairing::links::{Fetched, Fetcher};

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn login(role: Role) -> Who {
    Who::Login { username: "ada".into(), role }
}

fn post(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let st = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle(path, who.clone(), b"", &p);
    let st = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (st, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn switches(ws: &str, build: bool, full: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1", params![ws, build as i64, full as i64])
        .expect("switches");
}

const FPR: &str = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89";

fn set_body(f: &Fx) -> J {
    json!({
        "app": f.app,
        "ios": ["TEAM123456.com.jobs.app"],
        "android": [{"package": "com.jobs.app", "sha256": [FPR]}],
        "linkPaths": ["/jobs/*"],
        "store": {"ios": "https://apps.apple.com/app/id1"},
    })
}

#[test]
fn only_the_owner_in_person_with_a_fresh_confirm_sets_the_phone_apps() {
    let f = fx("oset");
    let vault = crate::vault::ensure_test_vault();
    switches(&f.ws, true, true);
    for who in [agent(&f.ws), login(Role::Admin), Who::Shell { project_id: f.ws.clone(), session_id: "s".into() }] {
        let (st, v) = post(SET, &who, set_body(&f));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{who:?}: {v}");
    }
    let (st, v) = post(SET, &Who::Owner, set_body(&f));
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    assert_eq!(v["error"]["serviceId"], f.app.as_str());
    assert_eq!(v["error"]["action"], super::super::CONFIRM_ACTION);
    // A bad value is refused before the confirm is spent, naming the field.
    let tok = vault.confirm.mint(super::super::CONFIRM_ACTION, Some(&f.app), "owner", "local-auth");
    let mut bad = set_body(&f);
    bad["ios"] = json!(["not-an-app-id"]);
    bad["confirmToken"] = json!(tok);
    let (st, v) = post(SET, &Who::Owner, bad);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    assert!(v["error"]["hint"].as_str().expect("hint").starts_with("ios"), "{v}");
    // A host that isn't the App's.
    let mut bad = set_body(&f);
    bad["hosts"] = json!(["jobs.example.com"]);
    bad["confirmToken"] = json!(tok);
    let (st, v) = post(SET, &Who::Owner, bad);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    assert_eq!(v["error"]["hosts"], json!([format!("{}.box.k2.dev", f.app_id)]));
    // A confirm for another App doesn't open this one.
    let other = vault.confirm.mint(super::super::CONFIRM_ACTION, Some(&f.other_app), "owner", "local-auth");
    let mut b = set_body(&f);
    b["confirmToken"] = json!(other);
    let (st, v) = post(SET, &Who::Owner, b);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "{v}");
    // An outstanding code dies when the phone apps change.
    let (pass, _) = pass_for("dana", &f.app);
    let (_, started) = start_call(&pass, json!({}));
    let outstanding = started["code"].as_str().expect("code").to_string();
    let mut b = set_body(&f);
    b["confirmToken"] = json!(tok);
    let (st, v) = post(SET, &Who::Owner, b.clone());
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["state"], "on");
    assert_eq!(v["ios"], json!(["TEAM123456.com.jobs.app"]));
    assert_eq!(v["hosts"], json!([{"host": format!("{}.box.k2.dev", f.app_id), "pairs": true}]));
    let warnings = v["warnings"].as_array().expect("warnings");
    assert!(warnings.iter().any(|w| w.as_str().is_some_and(|w| w.contains("apple-app-site-association"))), "{v}");
    assert!(warnings.iter().any(|w| w.as_str().is_some_and(|w| w.contains("week"))), "the ids changed: {v}");
    assert_eq!(store::check(&outstanding, &f.app).err(), Some(store::Refused::Unknown));
    let (st, v) = post(SET, &Who::Owner, b);
    assert_eq!((st, code(&v)), (403, "vault_confirm_required"), "a confirm works once: {v}");
    // The link files now name the new apps on the App's host.
    let set = super::super::links::get(&f.app_id).expect("snapshot");
    assert_eq!(set.ios, ["TEAM123456.com.jobs.app"]);
    assert_eq!(set.link_paths, ["/jobs/*"]);
}

#[test]
fn a_service_k2_cant_front_and_host_are_refused() {
    let f = fx("ofront");
    let vault = crate::vault::ensure_test_vault();
    let mut b = set_body(&f);
    b["app"] = json!(f.other_app);
    b["confirmToken"] = json!(vault.confirm.mint(super::super::CONFIRM_ACTION, Some(&f.other_app), "owner", "local-auth"));
    let (st, v) = post(SET, &Who::Owner, b);
    assert_eq!((st, code(&v)), (409, "pairing_needs_k2_front"), "a --cmd service: {v}");
    let (st, v) = get(STATUS, &Who::Owner, &[("app", "host")]);
    assert_eq!((st, code(&v)), (400, "not_for_host"), "{v}");
}

#[test]
fn the_apps_agent_reads_and_turns_off_but_never_sets() {
    let f = fx("ooff");
    let (st, v) = get(STATUS, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!((st, code(&v)), (403, "forbidden"), "switches off: {v}");
    switches(&f.ws, true, false);
    let (st, v) = get(STATUS, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["state"].as_str(), v["served"].as_bool()), (Some("on"), Some(true)), "{v}");
    // Another workspace's agent never sees it.
    switches("ot-ooff", true, true);
    let (st, v) = get(STATUS, &agent("ot-ooff"), &[("app", &f.app)]);
    assert_eq!(st, 404, "{v}");
    // A Member is read-only.
    let (st, _) = post(OFF, &login(Role::Member), json!({"app": f.app}));
    assert_eq!(st, 403);
    let (pass, _) = pass_for("dana", &f.app);
    let (_, started) = start_call(&pass, json!({}));
    let (st, v) = post(OFF, &agent(&f.ws), json!({"app": f.app}));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["state"].as_str(), v["off"].as_str(), v["wasOn"].as_bool()), (Some("off"), Some("owner_off"), Some(true)), "{v}");
    assert_eq!(store::check(started["code"].as_str().expect("code"), &f.app).err(), Some(store::Refused::Unknown), "off kills codes at once");
    assert!(super::super::links::get(&f.app_id).is_none(), "the link files stop");
    let (st, v) = start_call(&pass, json!({}));
    assert_eq!((st, code(&v)), (403, "pairing_off"));
}

#[test]
fn the_log_names_people_so_the_agent_needs_its_app_user_switch() {
    let f = fx("olog");
    let (pass, _) = pass_for("dana", &f.app);
    let (_, started) = start_call(&pass, json!({}));
    let code_raw = started["code"].as_str().expect("code").to_string();
    let ctx = crate::app_pairing::guest_routes::Ctx::default();
    let r = crate::app_pairing::guest_routes::handle_redeem(
        json!({"code": code_raw, "app": f.app, "device": {"id": "ip", "name": "Dana's iPhone"}}).to_string().as_bytes(),
        "POST /cli/skin/pair HTTP/1.1",
        &ctx,
    );
    assert!(r.response.status.starts_with("200"), "{}", r.response.body);
    let (st, v) = get(LOG, &Who::Owner, &[("app", &f.app)]);
    assert_eq!(st, 200, "{v}");
    let rows = v["pairings"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["username"].as_str(), rows[0]["deviceName"].as_str(), rows[0]["live"].as_bool()), (Some("dana"), Some("Dana's iPhone"), Some(true)));
    assert!(!v.to_string().contains(&code_raw), "never the code");
    let (st, v) = get(LOG, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!((st, code(&v)), (403, "forbidden"), "{v}");
    // The App-user gate (`skin_routes::agent_may_manage_app_users`) is the
    // agent's own "Let this agent manage its Apps' users" switch since 0.46.4
    // (apps.manage-users, migration 0225): Build and run Apps alone is not it.
    switches(&f.ws, true, false);
    let (st, v) = get(LOG, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!((st, code(&v)), (403, "forbidden"), "Build and run Apps alone: {v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE projects SET agents_can_manage_app_users = 1 WHERE id = ?1", params![f.ws]).expect("users switch");
    }
    let (st, v) = get(LOG, &agent(&f.ws), &[("app", &f.app)]);
    assert_eq!(st, 200, "{v}");
    let (st, v) = get(STATUS, &Who::Owner, &[("app", &f.app)]);
    assert_eq!((st, v["pairedPhones"].as_i64(), v["pairingsLastDay"].as_i64()), (200, Some(1), Some(1)), "{v}");
}

struct Fake;

impl Fetcher for Fake {
    fn get(&self, url: &str) -> Fetched {
        if url.contains("app-site-association.cdn-apple.com") {
            return Fetched::Ok { status: 200, body: r#"{"applinks":{"details":[]}}"#.into() };
        }
        if url.ends_with("/apple-app-site-association") {
            let host = url.trim_start_matches("https://").split('/').next().unwrap_or("");
            let app_id = host.split('.').next().unwrap_or("");
            let set = super::super::links::get(app_id).expect("snapshot");
            return Fetched::Ok { status: 200, body: super::super::links::aasa_body(&set) };
        }
        Fetched::Tls
    }
}

#[test]
fn check_maps_what_the_internet_answers() {
    let f = fx("ochk");
    let (st, v) = post(CHECK, &agent(&f.ws), json!({"app": f.app}));
    assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    let vault = crate::vault::ensure_test_vault();
    let mut b = set_body(&f);
    b["confirmToken"] = json!(vault.confirm.mint(super::super::CONFIRM_ACTION, Some(&f.app), "owner", "local-auth"));
    assert_eq!(post(SET, &Who::Owner, b).0, 200);
    set_fetcher_for_tests(Some(Box::new(Fake)));
    let (st, v) = post(CHECK, &Who::Owner, json!({"app": f.app}));
    set_fetcher_for_tests(None);
    assert_eq!(st, 200, "{v}");
    assert_eq!(
        v["hosts"],
        json!([{"host": format!("{}.box.k2.dev", f.app_id), "aasa": "ok", "assetlinks": "tls_error", "appleCdn": "stale"}])
    );
    let (_, s) = get(STATUS, &Who::Owner, &[("app", &f.app)]);
    assert_eq!(s["lastCheck"]["hosts"], v["hosts"]);
}

#[test]
fn removing_the_app_takes_its_pairing_config_and_link_files() {
    let f = fx("orm");
    super::super::refresh_links(&f.app);
    assert!(super::super::links::get(&f.app_id).is_some());
    super::super::forget_app(&f.app, Some(&f.app_id));
    assert!(store::config(&f.app).expect("read").is_none());
    assert!(super::super::links::get(&f.app_id).is_none());
}
