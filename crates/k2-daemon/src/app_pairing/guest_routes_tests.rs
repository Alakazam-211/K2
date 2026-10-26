//! The pairing routes in process (S2): who may start, the fresh sign-in
//! and its password re-check, the host, status for its own person only,
//! check and redeem (one-time, the App's own, browser-shaped refused except
//! the owner's webview origins), push in the same step, test guests, and
//! no code, pass or token in the audit trail.

use std::collections::HashMap;

use k2_core::skin::{self, app_login, app_pairing as store, SkinPass};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

pub(crate) struct Fx {
    pub _db: k2_core::db::TestDbGuard,
    pub _home: crate::test_support::TempHome,
    /// The App's service id.
    pub app: String,
    /// The runtime App id (its label).
    pub app_id: String,
    pub ws: String,
    pub other_app: String,
}

pub(crate) const APNS: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

/// A runtime App `pj<tag>` (tunnelled, on box `box`), its guest `dana`
/// (password `s3cret-horse`), another App with guest `eli`, and pairing on
/// with one iOS app id.
pub(crate) fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let cfg = k2_core::tunnel::config::TunnelConfig { subdomain: "box".into(), ..Default::default() };
    k2_core::tunnel::config::save(&cfg).expect("tunnel config");
    let app_id = format!("pj{tag}");
    let (ws, other) = (format!("ws-{tag}"), format!("ot-{tag}"));
    let (app, other_app) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws, &other] {
            let path = home.path().join(r.as_str());
            std::fs::create_dir_all(&path).expect("project dir");
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, path.to_string_lossy()])
                .expect("project");
        }
        let a = k2_core::published_services::insert(&conn, &ws, &app_id, "node s.js", "/tmp", 4330, "tunnel", "running", "cmd", "").expect("svc");
        let o = k2_core::published_services::insert(&conn, &other, "crm", "node s.js", "/tmp", 4331, "local", "running", "cmd", "").expect("svc");
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: &app_id,
                service_id: &a.id,
                seed_id: None,
                owner_workspace: &ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "test",
            },
        )
        .expect("runtime app");
        (a.id, o.id)
    };
    skin::add_principal_for_app("dana", None, &app, None, "test").expect("dana");
    skin::set_principal_password("dana", Some("s3cret-horse")).expect("password");
    skin::add_principal_for_app("eli", None, &other_app, None, "test").expect("eli");
    turn_on(&app, &[]);
    Fx { _db: db, _home: home, app, app_id, ws, other_app }
}

pub(crate) fn turn_on(app: &str, origins: &[&str]) {
    let input = store::ConfigInput {
        ios: vec!["ABCDE12345.com.you.app".into()],
        native_origins: origins.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    store::set_config(app, &input, "owner").expect("pairing on");
}

pub(crate) fn pass_for(user: &str, app: &str) -> (SkinPass, String) {
    let s = skin::create_app_session_token(user, app).expect("session");
    (skin::resolve_skin_token(&s.raw).expect("live pass"), s.raw)
}

fn status_of(r: &CliResponse) -> u16 {
    r.status.split_whitespace().next().expect("status").parse().expect("number")
}

fn parse(r: &CliResponse) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json ({e}): {}", r.body))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn main_port() -> Ctx {
    Ctx { link_app: None, ip: "203.0.113.9".into() }
}

pub(crate) fn start_call(pass: &SkinPass, body: J) -> (u16, J) {
    let r = handle_pass(START, pass, true, body.to_string().as_bytes(), &HashMap::new(), &main_port());
    (status_of(&r), parse(&r))
}

fn status_call(pass: &SkinPass, id: &str) -> (u16, J) {
    let q: HashMap<String, String> = [("pairingId".to_string(), id.to_string())].into();
    let r = handle_pass(STATUS, pass, false, b"", &q, &main_port());
    (status_of(&r), parse(&r))
}

const PHONE: &str = "\r\nUser-Agent: JobsApp/1.0 CFNetwork\r\n";

fn check_call(body: J, headers: &str, ctx: &Ctx) -> (u16, J) {
    let r = handle_check(body.to_string().as_bytes(), &format!("POST /cli/skin/pair/check HTTP/1.1{headers}"), ctx).response;
    (status_of(&r), parse(&r))
}

pub(crate) fn redeem_call(body: J, headers: &str, ctx: &Ctx) -> (u16, J, String) {
    let r = handle_redeem(body.to_string().as_bytes(), &format!("POST /cli/skin/pair HTTP/1.1{headers}"), ctx).response;
    (status_of(&r), parse(&r), r.body)
}

fn device(id: &str) -> J {
    json!({ "id": id, "name": "Dana's iPhone" })
}

#[test]
fn a_fresh_web_session_starts_and_the_phone_pairs_once() {
    let f = fx("ok");
    let (pass, _) = pass_for("dana", &f.app);
    let (st, v) = start_call(&pass, json!({}));
    assert_eq!(st, 200, "{v}");
    let host = format!("{}.box.k2.dev", f.app_id);
    assert_eq!(v["host"], host.as_str());
    let code = v["code"].as_str().expect("code").to_string();
    assert!(code.starts_with("K2A-"), "{code}");
    let url = v["pairUrl"].as_str().expect("pairUrl");
    assert_eq!(url, format!("https://{host}/.well-known/k2-pair#{code}"));
    let rows = k2_core::qr::encode(url).expect("qr");
    assert_eq!(v["qr"], json!({ "size": rows.len(), "rows": rows }), "the QR is the link's");
    assert_eq!(v["expiresInSecs"], store::CODE_TTL_SECS);
    let id = v["pairingId"].as_str().expect("pairingId").to_string();
    let (st, s) = status_call(&pass, &id);
    assert_eq!((st, s["state"].as_str()), (200, Some("pending")), "{s}");
    // The phone checks (never consuming), then redeems.
    let (st, c) = check_call(json!({"code": code, "app": f.app}), PHONE, &main_port());
    assert_eq!(st, 200, "{c}");
    assert_eq!((c["username"].as_str(), c["app"].as_str(), c["appName"].as_str()), (Some("dana"), Some(f.app.as_str()), Some(f.app_id.as_str())));
    let (st, r, _) = redeem_call(json!({"code": code, "app": f.app, "device": device("iphone-1")}), PHONE, &main_port());
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["via"], "pairing");
    assert_eq!(r["username"], "dana");
    assert_eq!(r["device"], json!({"id": "iphone-1", "name": "Dana's iPhone"}));
    let token = r["token"].as_str().expect("token");
    assert!(app_login::is_refresh_token(r["refresh"].as_str().expect("refresh")));
    for k in ["rooms", "roomHandles", "caps", "role", "roomAccess", "passExpiresAt", "refreshExpiresAt", "principalId", "app"] {
        assert!(r.get(k).is_some(), "the device-login answer has {k}: {r}");
    }
    let phone = skin::resolve_skin_token(token).expect("the paired pass works");
    assert_eq!(phone.app, f.app);
    // The web page sees which device paired.
    let (_, s) = status_call(&pass, &id);
    assert_eq!(s["state"], "paired", "{s}");
    assert_eq!(s["device"]["name"], "Dana's iPhone");
    // One-time.
    let (st, again, _) = redeem_call(json!({"code": code, "app": f.app, "device": device("iphone-2")}), PHONE, &main_port());
    assert_eq!((st, code_of(&again)), (401, "pair_code_invalid".to_string()));
    let (st, again) = check_call(json!({"code": code, "app": f.app}), PHONE, &main_port());
    assert_eq!((st, code_of(&again)), (401, "pair_code_invalid".to_string()));
    // The person's device list says how it started.
    let list = parse(&crate::app_mobile_routes::handle_devices(&pass));
    assert_eq!(list["devices"][0]["via"], "pairing", "{list}");
    // A phone (a device sign-in's pass) never starts another pairing.
    let (st, v) = start_call(&phone, json!({}));
    assert_eq!((st, code_of(&v)), (403, "pairing_from_device".to_string()), "{v}");
}

fn code_of(v: &J) -> String {
    code(v).to_string()
}

#[test]
fn only_a_guest_session_pass_of_this_app_starts() {
    let f = fx("who");
    let (mut platform, _) = pass_for("dana", &f.app);
    platform.session = false;
    let (st, v) = start_call(&platform, json!({}));
    assert_eq!((st, code(&v)), (403, "guest_required"), "{v}");
    let (mut nobody, _) = pass_for("dana", &f.app);
    nobody.principal_id = None;
    let (st, v) = start_call(&nobody, json!({}));
    assert_eq!((st, code(&v)), (403, "guest_required"), "{v}");
    // Another App's guest: that App doesn't pair.
    let (eli, _) = pass_for("eli", &f.other_app);
    let (st, v) = start_call(&eli, json!({}));
    assert_eq!((st, code(&v)), (403, "pairing_off"), "{v}");
    // The App and the person come from the pass: a body never names them.
    let (dana, _) = pass_for("dana", &f.app);
    let (st, v) = start_call(&dana, json!({"app": f.other_app}));
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    // A guest session on another App's link.
    let ctx = Ctx { link_app: Some(f.other_app.clone()), ip: "-".into() };
    let r = handle_pass(START, &dana, true, b"{}", &HashMap::new(), &ctx);
    assert_eq!((status_of(&r), code(&parse(&r))), (403, "guest_required"));
    // GET start / POST status are 405.
    assert_eq!(status_of(&handle_pass(START, &dana, false, b"", &HashMap::new(), &main_port())), 405);
    assert_eq!(status_of(&handle_pass(STATUS, &dana, true, b"{}", &HashMap::new(), &main_port())), 405);
}

#[test]
fn an_old_sign_in_needs_the_password_and_wrong_ones_lock_the_person() {
    let f = fx("fresh");
    let (pass, _) = pass_for("dana", &f.app);
    store::age_session_for_test(&pass.id, store::FRESH_SIGNIN_SECS + 300);
    let (st, v) = start_call(&pass, json!({}));
    assert_eq!((st, code(&v)), (401, "reauth_required"), "{v}");
    let (st, v) = start_call(&pass, json!({"password": "s3cret-horse"}));
    assert_eq!(st, 200, "the right password starts: {v}");
    for i in 0..3 {
        let (st, v) = start_call(&pass, json!({"password": "wrong-horse"}));
        assert_eq!((st, code(&v)), (401, "bad_login"), "try {i}: {v}");
    }
    let (st, v) = start_call(&pass, json!({"password": "wrong-horse"}));
    assert_eq!((st, code(&v)), (423, "locked"), "the 4th: {v}");
    assert!(v["error"]["retryAfterSecs"].as_i64().is_some_and(|s| s > 0), "{v}");
    // The lockout is the person's App-login lockout: their password login
    // is locked too, and the right password at start is refused while it is.
    let (st, v) = start_call(&pass, json!({"password": "s3cret-horse"}));
    assert_eq!((st, code(&v)), (423, "locked"), "{v}");
    assert!(matches!(skin::check_and_record_login("dana", "s3cret-horse"), skin::SkinLoginOutcome::LockedOut { .. }));
}

#[test]
fn the_host_must_be_one_the_app_pairs_on() {
    let f = fx("host");
    let (pass, _) = pass_for("dana", &f.app);
    let (st, v) = start_call(&pass, json!({"host": "evil.example"}));
    assert_eq!((st, code(&v)), (400, "host_not_paired"), "{v}");
    assert_eq!(v["error"]["hosts"], json!([format!("{}.box.k2.dev", f.app_id)]));
    let (st, v) = start_call(&pass, json!({"host": format!("{}.BOX.k2.dev.", f.app_id)}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["host"], format!("{}.box.k2.dev", f.app_id));
}

#[test]
fn pairing_off_and_stay_signed_in_off_refuse_the_start() {
    let f = fx("off");
    let (pass, _) = pass_for("dana", &f.app);
    app_login::set_settings(&f.app, None, Some(0), "owner").expect("stay 0");
    let (st, v) = start_call(&pass, json!({}));
    assert_eq!((st, code(&v)), (409, "device_signin_off"), "{v}");
    app_login::set_settings(&f.app, None, Some(30), "owner").expect("stay 30");
    store::set_off(&f.app, "owner").expect("off");
    let (st, v) = start_call(&pass, json!({}));
    assert_eq!((st, code(&v)), (403, "pairing_off"), "{v}");
}

#[test]
fn status_and_cancel_are_the_persons_own() {
    let f = fx("own");
    let (pass, _) = pass_for("dana", &f.app);
    let (_, v) = start_call(&pass, json!({}));
    let id = v["pairingId"].as_str().expect("id").to_string();
    // Another person on another App: exactly like an unknown id.
    let (eli, _) = pass_for("eli", &f.other_app);
    let (st, s) = status_call(&eli, &id);
    assert_eq!((st, code(&s)), (404, "not_found"));
    let (st, s) = status_call(&pass, "apr_0000000000000000000000");
    assert_eq!((st, code(&s)), (404, "not_found"));
    let (st, s) = status_call(&pass, "nope");
    assert_eq!((st, code(&s)), (400, "usage"));
    let r = handle_pass(CANCEL, &eli, true, json!({"pairingId": id}).to_string().as_bytes(), &HashMap::new(), &main_port());
    assert_eq!(parse(&r), json!({"ok": true, "cancelled": false}), "someone else's cancel is a miss");
    let r = handle_pass(CANCEL, &pass, true, json!({"pairingId": id}).to_string().as_bytes(), &HashMap::new(), &main_port());
    assert_eq!(parse(&r), json!({"ok": true, "cancelled": true}));
    let (_, s) = status_call(&pass, &id);
    assert_eq!(s["state"], "cancelled");
}

#[test]
fn a_code_is_its_apps_and_a_browser_page_never_redeems() {
    let f = fx("xapp");
    let (pass, _) = pass_for("dana", &f.app);
    let (_, v) = start_call(&pass, json!({}));
    let code = v["code"].as_str().expect("code").to_string();
    // A browser page (an Origin that isn't a native webview): refused
    // before the code is looked at, so it isn't consumed.
    let evil = "\r\nOrigin: https://evil.example\r\nSec-Fetch-Site: cross-site";
    let (st, r, _) = redeem_call(json!({"code": code, "app": f.app, "device": device("x")}), evil, &main_port());
    assert_eq!((st, code_of(&r)), (403, "pairing_not_from_browser".to_string()), "{r}");
    let (st, r) = check_call(json!({"code": code, "app": f.app}), evil, &main_port());
    assert_eq!((st, code_of(&r)), (403, "pairing_not_from_browser".to_string()), "{r}");
    let (st, r) = check_call(json!({"code": code, "app": f.app}), "\r\nSec-Fetch-Mode: cors", &main_port());
    assert_eq!(st, 403, "fetch metadata without an Origin is a browser: {r}");
    // Off the link the body names the App; on another App's link a body
    // naming this one is the generic refusal.
    let (st, r) = check_call(json!({"code": code}), PHONE, &main_port());
    assert_eq!((st, code_of(&r)), (400, "usage".to_string()), "{r}");
    let other_link = Ctx { link_app: Some(f.other_app.clone()), ip: "-".into() };
    let (st, r) = check_call(json!({"code": code, "app": f.app}), PHONE, &other_link);
    assert_eq!((st, code_of(&r)), (401, "pair_code_invalid".to_string()), "{r}");
    // Another App's redeem consumes it and mints nothing.
    let (st, r, _) = redeem_call(json!({"code": code, "app": f.other_app, "device": device("x")}), PHONE, &main_port());
    assert_eq!((st, code_of(&r)), (401, "pair_code_invalid".to_string()), "{r}");
    let (st, r, _) = redeem_call(json!({"code": code, "app": f.app, "device": device("x")}), PHONE, &main_port());
    assert_eq!((st, code_of(&r)), (401, "pair_code_invalid".to_string()), "consumed by the attempt: {r}");
    assert!(app_login::devices_for(pass.principal_id.as_deref().expect("pid"), &f.app).expect("devices").is_empty());
}

#[test]
fn the_owners_webview_origins_may_redeem() {
    let f = fx("origin");
    turn_on(&f.app, &["tauri://localhost", "capacitor://localhost"]);
    let (pass, _) = pass_for("dana", &f.app);
    for (i, origin) in ["capacitor://localhost", "tauri://localhost", "https://tauri.localhost"].iter().enumerate() {
        store::reset_starts_for_test(&f.app, pass.principal_id.as_deref().expect("pid"));
        let (_, v) = start_call(&pass, json!({}));
        let code = v["code"].as_str().expect("code").to_string();
        let h = format!("\r\nOrigin: {origin}\r\nSec-Fetch-Site: cross-site");
        let (st, r, _) = redeem_call(json!({"code": code, "app": f.app, "device": device(&format!("w{i}"))}), &h, &main_port());
        assert_eq!(st, 200, "{origin}: {r}");
    }
    // Without the owner listing it, capacitor's origin is a browser's.
    turn_on(&f.app, &[]);
    let (_, v) = start_call(&pass, json!({}));
    let code = v["code"].as_str().expect("code").to_string();
    let (st, _, _) = redeem_call(json!({"code": code, "app": f.app, "device": device("z")}), "\r\nOrigin: capacitor://localhost", &main_port());
    assert_eq!(st, 403);
}

#[test]
fn a_bad_device_is_refused_before_the_code_is_used() {
    let f = fx("dev");
    let (pass, _) = pass_for("dana", &f.app);
    let (_, v) = start_call(&pass, json!({}));
    let code = v["code"].as_str().expect("code").to_string();
    for bad in [json!({"code": code, "app": f.app}), json!({"code": code, "app": f.app, "device": {"id": "has space"}}), json!({"code": code, "app": f.app, "device": device("d"), "push": 1})] {
        let (st, r, _) = redeem_call(bad.clone(), PHONE, &main_port());
        assert_eq!((st, code_of(&r)), (400, "usage".to_string()), "{bad}: {r}");
    }
    let (st, r, _) = redeem_call(json!({"code": code, "app": f.app, "device": device("d")}), PHONE, &main_port());
    assert_eq!(st, 200, "still unused: {r}");
}

#[test]
fn push_rides_the_redeem_and_its_refusal_never_fails_the_pairing() {
    let f = fx("push");
    let (pass, _) = pass_for("dana", &f.app);
    let push = json!({"platform": "apns", "pushToken": APNS, "environment": "sandbox"});
    let (_, v) = start_call(&pass, json!({}));
    let (st, r, raw) = redeem_call(json!({"code": v["code"], "app": f.app, "device": device("ph1"), "push": push}), PHONE, &main_port());
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["push"], json!({"registered": false, "code": "push_off"}), "{r}");
    assert!(!raw.contains(APNS), "the push token never comes back");
    skin::app_push::set_switches(&f.app, skin::app_push::Switches { owner_on: Some(true), ..Default::default() }, "owner").expect("push on");
    let (_, v) = start_call(&pass, json!({}));
    let (st, r, raw) = redeem_call(json!({"code": v["code"], "app": f.app, "device": device("ph2"), "push": push}), PHONE, &main_port());
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["push"]["registered"], true, "{r}");
    assert!(r["push"]["id"].as_str().is_some_and(|i| i.starts_with("apd_")), "{r}");
    assert!(!raw.contains(APNS));
    let rows = skin::app_push::devices_for_app(&f.app, None).expect("push rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].device_id, "ph2", "the device sign-in's id is the push device id");
    assert!(rows[0].device_login_id.is_some(), "linked to the paired sign-in");
    // A push deviceId that isn't the device's.
    let (_, v) = start_call(&pass, json!({}));
    let mut p2 = push.clone();
    p2["deviceId"] = json!("other");
    let (st, r, _) = redeem_call(json!({"code": v["code"], "app": f.app, "device": device("ph3"), "push": p2}), PHONE, &main_port());
    assert_eq!(st, 200);
    assert_eq!(r["push"], json!({"registered": false, "code": "usage"}));
}

#[test]
fn a_test_guest_pairs_through_its_apps_link_only() {
    let f = fx("tg");
    skin::add_principal_for_app("probe", None, &f.app, None, "test").expect("guest");
    let pid = skin::list_principals().expect("list").into_iter().find(|p| p.username == "probe").expect("probe").id;
    skin::test_guests::record(&pid, &f.app, &f.ws, "agent:x", 4_102_444_800).expect("test guest");
    let raw = skin::test_guests::with_link_login(Some(&f.app), || skin::create_app_session_token("probe", &f.app)).expect("link session").raw;
    let pass = skin::resolve_skin_token(&raw).expect("live");
    let link = Ctx { link_app: Some(f.app.clone()), ip: "-".into() };
    let (_, v) = start_call(&pass, json!({}));
    let (st, r, _) = redeem_call(json!({"code": v["code"], "app": f.app, "device": device("t1")}), PHONE, &main_port());
    assert_eq!((st, code_of(&r)), (401, "pair_code_invalid".to_string()), "never on the main port: {r}");
    let (_, v) = start_call(&pass, json!({}));
    let (st, r, _) = redeem_call(json!({"code": v["code"], "device": device("t2")}), PHONE, &link);
    assert_eq!(st, 200, "through its App's link: {r}");
}

#[test]
fn no_code_pass_refresh_or_push_token_reaches_the_audit_trail() {
    let f = fx("audit");
    skin::app_push::set_switches(&f.app, skin::app_push::Switches { owner_on: Some(true), ..Default::default() }, "owner").expect("push on");
    let (pass, raw_pass) = pass_for("dana", &f.app);
    let (_, v) = start_call(&pass, json!({}));
    let code = v["code"].as_str().expect("code").to_string();
    let _ = check_call(json!({"code": "K2A-00000-00000-00000-00000", "app": f.app}), PHONE, &main_port());
    let _ = check_call(json!({"code": code, "app": f.app}), PHONE, &main_port());
    let (st, r, _) = redeem_call(
        json!({"code": code, "app": f.app, "device": device("au"), "push": {"platform": "apns", "pushToken": APNS}}),
        PHONE,
        &main_port(),
    );
    assert_eq!(st, 200, "{r}");
    let (token, refresh) = (r["token"].as_str().expect("token"), r["refresh"].as_str().expect("refresh"));
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("auth audit written");
    for e in ["app-pair-start", "app-pair-check", "app-pair\""] {
        assert!(audit.contains(e), "{e} audited: {audit}");
    }
    assert!(audit.contains(v["pairingId"].as_str().expect("id")), "rows name the pairingId");
    let body = store::normalize(&code).expect("normalizes");
    for secret in [code.as_str(), body.as_str(), raw_pass.as_str(), token, refresh, APNS] {
        assert!(!audit.contains(secret), "a secret reached the audit trail: {audit}");
    }
}
