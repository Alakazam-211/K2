//! `/cli/desktop/*` handler tests (headless; a fake deskd). The wire-level
//! checks (dispatcher 405s, route-policy floors, real passports and App
//! passes over TCP) are in `tests/it_misc/desktop_routes_integration.rs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_core::connect_users::{self, Role};
use k2_core::desktop::audit;
use k2_core::desktop::viewers::{self, ViewerHandle, ViewerInfo};
use k2_core::desktop::wire::{Display, Mode};
use k2_core::desktop::{store, ticket, MachineState, CONSOLE};
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use crate::desktop::deskd::{self, Deskd};

struct FakeDeskd {
    calls: Mutex<Vec<&'static str>>,
}

impl Deskd for FakeDeskd {
    fn backend(&self) -> &'static str {
        "macos"
    }
    fn state(&self) -> MachineState {
        MachineState::Ready
    }
    fn enable(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("enable");
        Ok(())
    }
    fn disable(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("disable");
        Ok(())
    }
    fn displays(&self) -> Vec<Display> {
        vec![Display {
            id: "1".into(),
            name: "Built-in".into(),
            width_pt: 1512,
            height_pt: 982,
            width_px: 3024,
            height_px: 1964,
            scale: 2.0,
            main: true,
        }]
    }
}

struct Env {
    _home: k2_core::test_env::TempHome,
    _db: k2_core::db::TestDbGuard,
    _deskd: deskd::TestDeskd,
    fake: Arc<FakeDeskd>,
}

fn env() -> Env {
    let home = k2_core::test_env::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let fake = Arc::new(FakeDeskd { calls: Mutex::new(Vec::new()) });
    let guard = deskd::set_for_test(fake.clone());
    Env { _home: home, _db: db, _deskd: guard, fake }
}

fn login(name: &str, role: Role) -> Who {
    connect_users::add_user(name, "password123").unwrap_or_else(|e| panic!("add_user {name}: {e}"));
    if role != Role::Member {
        connect_users::set_role(name, role).unwrap_or_else(|e| panic!("set_role {name}: {e}"));
    }
    Who::Login { username: name.into(), role }
}

fn shell() -> Who {
    Who::Shell { project_id: "p-shell".into(), session_id: "s-shell".into() }
}

fn agent() -> Who {
    Who::Agent { project_id: "p-agent".into(), session_id: "s-agent".into() }
}

fn call_at(who: &Who, path: &str, body: Value, ingress: &str) -> (u16, Value) {
    let is_post = is_post_route(path);
    let raw = body.to_string();
    let params: HashMap<String, String> = HashMap::new();
    let req = Req {
        path,
        is_post,
        body: raw.as_bytes(),
        params: &params,
        ingress,
        ip: "203.0.113.7",
        token: "login-session-token",
    };
    let r = handle(who, &req);
    let code: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: body is not JSON ({e}): {}", r.body));
    (code, v)
}

/// A remote caller (over Connect).
fn call(who: &Who, path: &str, body: Value) -> (u16, Value) {
    call_at(who, path, body, "tunnel")
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code in {v}"))
}

fn audit_events() -> Vec<String> {
    audit::read(None, 1000).unwrap().iter().map(|l| l["event"].as_str().unwrap().to_string()).collect()
}

#[derive(Default)]
struct FakeViewer(Mutex<Vec<(u16, String)>>);
impl ViewerHandle for FakeViewer {
    fn close(&self, code: u16, reason: &str) {
        self.0.lock().unwrap().push((code, reason.to_string()));
    }
}

fn viewer(user: &str, user_id: &str) -> ViewerInfo {
    ViewerInfo {
        id: String::new(),
        desktop: CONSOLE.into(),
        user: user.into(),
        user_id: user_id.into(),
        role: "owner".into(),
        mode: Mode::View,
        ingress: "tunnel".into(),
        ip: "203.0.113.7".into(),
        device: "web".into(),
        display: "1".into(),
        since: 0,
    }
}

/// Agents, App passes, API keys: `desktop_people_only` on every route;
/// a bad token is the plain 403.
#[test]
fn every_route_refuses_agents_and_app_credentials() {
    let _e = env();
    for path in ROUTES {
        for who in [agent(), Who::Wrong] {
            let (s, v) = call(&who, path, json!({}));
            assert_eq!((s, code(&v)), (403, PEOPLE_ONLY), "{path} {who:?}: {v}");
            assert!(v["error"]["hint"].as_str().unwrap().contains("people only"), "{v}");
        }
        let (s, v) = call(&Who::Invalid, path, json!({}));
        assert_eq!((s, v["error"].as_str()), (403, Some("Invalid or missing auth token")), "{path}: {v}");
    }
    assert!(!store::is_enabled(CONSOLE).unwrap(), "nothing turned it on");
    assert!(audit_events().is_empty(), "a refused caller writes nothing");
}

/// Every POST route refuses GET, every GET route refuses POST, before
/// anything else (even for the owner).
#[test]
fn methods_are_checked_first() {
    let _e = env();
    let params = HashMap::new();
    for &path in ROUTES {
        let wrong = !is_post_route(path);
        let req = Req { path, is_post: wrong, body: b"{}", params: &params, ingress: "loopback", ip: "-", token: "" };
        let r = handle(&Who::Owner, &req);
        assert_eq!(r.status, "405 Method Not Allowed", "{path}");
    }
    assert!(!store::is_enabled(CONSOLE).unwrap());
    assert_eq!(POST_ROUTES.len(), 8);
}

/// Owner login over Connect: on, the card, an audit line, a push to the
/// Owner logins' phones, a desktop_state event; off closes every viewer
/// with 4410 `desktop_off`.
#[test]
fn an_owner_turns_it_on_and_off_and_off_closes_every_viewer() {
    let e = env();
    let owner = login("rosson", Role::Owner);
    login("ann", Role::Admin);
    let mark = crate::push_routes::test_push_capture::mark();
    let mut events = crate::session_events::subscribe();

    let (s, v) = call(&owner, "/cli/desktop/enable", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["enabled"].as_bool(), v["changed"].as_bool()), (Some(true), Some(true)), "{v}");
    assert_eq!(v["card"], k2_core::desktop::ENABLE_CARD);
    assert!(store::is_enabled(CONSOLE).unwrap());
    assert_eq!(*e.fake.calls.lock().unwrap(), vec!["enable"]);

    let pushes: Vec<_> = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .filter_map(|p| match p {
            k2_core::push::PushEvent::DesktopNotice { line, kind_id, owners } => Some((line, kind_id, owners)),
            _ => None,
        })
        .collect();
    assert_eq!(pushes.len(), 1, "{pushes:?}");
    assert_eq!(pushes[0].1, "enable");
    assert_eq!(pushes[0].2, vec!["rosson".to_string()], "Owner logins only, never the Admin");
    assert!(pushes[0].0.contains("turned on by rosson"), "{}", pushes[0].0);

    let mut kinds = Vec::new();
    while let Ok(ev) = events.try_recv() {
        kinds.push(ev.kind_name());
    }
    assert!(kinds.contains(&"desktop_state"), "{kinds:?}");

    // Two viewers watching; off closes both with 4410.
    let a = Arc::new(FakeViewer::default());
    let b = Arc::new(FakeViewer::default());
    viewers::register(viewer("rosson", "u-r"), a.clone());
    viewers::register(viewer("ann", "u-a"), b.clone());
    let (s, v) = call(&owner, "/cli/desktop/disable", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["enabled"].as_bool(), v["closed"].as_u64()), (Some(false), Some(2)), "{v}");
    assert_eq!(*a.0.lock().unwrap(), vec![(4410, "desktop_off".to_string())]);
    assert_eq!(*b.0.lock().unwrap(), vec![(4410, "desktop_off".to_string())]);
    assert_eq!(viewers::count(CONSOLE), 0);
    assert!(!store::is_enabled(CONSOLE).unwrap());
    assert_eq!(*e.fake.calls.lock().unwrap(), vec!["enable", "disable"]);

    let lines = audit::read(None, 10).unwrap();
    let ev: Vec<&str> = lines.iter().map(|l| l["event"].as_str().unwrap()).collect();
    assert_eq!(ev, vec!["disable", "enable"], "one line each, newest first");
    assert_eq!(lines[1]["who"], "login:rosson");
    assert_eq!(lines[1]["role"], "owner");
    assert_eq!(lines[1]["ingress"], "tunnel");
    assert_eq!(lines[1]["ip"], "203.0.113.7");
}

/// An Admin opens (view and control) but never manages: no switch, no
/// policy, no grants, no viewers, no audit.
#[test]
fn an_admin_watches_and_drives_but_never_manages() {
    let _e = env();
    let admin = login("ann", Role::Admin);
    let (s, v) = call(&admin, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["you"], json!({"role": "admin", "mayView": true, "mayControl": true, "mayManage": false, "sameMachine": false}));
    assert!(v.get("grants").is_none() && v.get("viewers").is_none() && v.get("hideApps").is_none(), "{v}");
    for path in [
        "/cli/desktop/enable",
        "/cli/desktop/disable",
        "/cli/desktop/policy",
        "/cli/desktop/grant",
        "/cli/desktop/revoke",
        "/cli/desktop/grants",
        "/cli/desktop/viewers",
        "/cli/desktop/kick",
        "/cli/desktop/audit",
    ] {
        let (s, v) = call(&admin, path, json!({"user": "x", "hideApps": [], "viewer": "dv_x"}));
        assert_eq!((s, code(&v)), (403, "owner_only"), "{path}: {v}");
    }
    assert!(!store::is_enabled(CONSOLE).unwrap(), "an Admin can't turn it on");
    assert!(audit_events().is_empty(), "refusals change nothing");
    let (s, v) = call(&admin, "/cli/desktop/displays", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["displays"][0]["id"], "1");
}

/// A Member: nothing without a grant; with one, view only, and its status
/// shows only itself. A control grant is refused.
#[test]
fn a_member_needs_a_grant_and_only_watches() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    let mel = login("mel", Role::Member);
    let (s, v) = call(&mel, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["you"]["mayView"], false, "{v}");
    let (s, v) = call(&mel, "/cli/desktop/displays", json!({}));
    assert_eq!((s, code(&v)), (403, "not_allowed"), "{v}");

    let (s, v) = call(&owner, "/cli/desktop/grant", json!({"user": "mel", "mode": "control"}));
    assert_eq!((s, code(&v)), (400, "member_view_only"), "{v}");
    assert!(k2_core::desktop::grants::list(CONSOLE).unwrap().is_empty(), "refused grant stored nothing");

    let mark = crate::push_routes::test_push_capture::mark();
    let (s, v) = call(&owner, "/cli/desktop/grant", json!({"user": "mel"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["grant"]["user"].as_str(), v["grant"]["mode"].as_str()), (Some("mel"), Some("view")), "{v}");
    let pushed = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .any(|p| matches!(p, k2_core::push::PushEvent::DesktopNotice { kind_id, .. } if kind_id == "grant"));
    assert!(pushed, "a new grant pushes to the Owners");

    let (s, v) = call(&mel, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["you"], json!({"role": "member", "mayView": true, "mayControl": false, "mayManage": false, "sameMachine": false}));
    assert!(v.get("grants").is_none() && v.get("viewers").is_none(), "a Member sees only itself: {v}");

    // Grants are for Members: an Admin needs none.
    login("ann", Role::Admin);
    let (s, v) = call(&owner, "/cli/desktop/grant", json!({"user": "ann"}));
    assert_eq!((s, code(&v)), (400, "grant_not_needed"), "{v}");
    let (s, v) = call(&owner, "/cli/desktop/grant", json!({"user": "nobody"}));
    assert_eq!((s, code(&v)), (404, "no_such_user"), "{v}");

    // Revoke closes mel's viewer with 4401 grant_gone.
    let mel_id = connect_users::user_uuid("mel").unwrap().unwrap();
    let mv = Arc::new(FakeViewer::default());
    viewers::register(viewer("mel", &mel_id), mv.clone());
    let (s, v) = call(&owner, "/cli/desktop/revoke", json!({"user": "mel"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["revoked"].as_bool(), v["closed"].as_u64()), (Some(true), Some(1)), "{v}");
    assert_eq!(*mv.0.lock().unwrap(), vec![(4401, "grant_gone".to_string())]);
    let (_, v) = call(&mel, "/cli/desktop/status", json!({}));
    assert_eq!(v["you"]["mayView"], false, "{v}");
}

/// R18: a grant follows the login's id: removed and re-added under the
/// same name, the new login inherits nothing.
#[test]
fn a_re_added_login_inherits_no_grant() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    login("mel", Role::Member);
    assert_eq!(call(&owner, "/cli/desktop/grant", json!({"user": "mel"})).0, 200);
    connect_users::remove_user("mel").unwrap();
    let mel2 = login("mel", Role::Member);
    let (_, v) = call(&mel2, "/cli/desktop/status", json!({}));
    assert_eq!(v["you"]["mayView"], false, "{v}");
    let (_, v) = call(&owner, "/cli/desktop/grants", json!({}));
    assert_eq!(v["grants"], json!([]), "the stale grant is gone: {v}");
}

/// R19: nobody watches from this machine: a loopback login, the owner
/// token and a shell tab manage, but never get a ticket.
#[test]
fn same_machine_never_watches() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    assert_eq!(call(&owner, "/cli/desktop/enable", json!({})).0, 200);
    let (s, v) = call_at(&owner, "/cli/desktop/status", json!({}), "loopback");
    assert_eq!(s, 200);
    assert_eq!((v["you"]["sameMachine"].as_bool(), v["you"]["mayView"].as_bool()), (Some(true), Some(false)), "{v}");
    let (s, v) = call_at(&owner, "/cli/desktop/ticket", json!({}), "loopback");
    assert_eq!((s, code(&v)), (403, "same_machine"), "{v}");
    for who in [Who::Owner, shell()] {
        let (s, v) = call_at(&who, "/cli/desktop/status", json!({}), "loopback");
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["you"]["mayManage"], true);
        assert_eq!(v["you"]["mayView"], false);
        let (s, v) = call_at(&who, "/cli/desktop/ticket", json!({}), "loopback");
        assert_eq!((s, code(&v)), (403, "desktop_logins_only"), "{v}");
    }
}

/// R20: a login that may watch gets a 60 s single-use ticket bound to its
/// session; off, there's none.
#[test]
fn a_ticket_is_for_a_remote_login_that_may_watch_and_works_once() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    let (s, v) = call(&owner, "/cli/desktop/ticket", json!({}));
    assert_eq!((s, code(&v)), (409, "desktop_off"), "{v}");
    assert_eq!(call(&owner, "/cli/desktop/enable", json!({})).0, 200);
    let (s, v) = call(&owner, "/cli/desktop/ticket", json!({"display": "1"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["expiresIn"], 60);
    let t = v["ticket"].as_str().unwrap().to_string();
    let claim = ticket::redeem(&t).expect("redeems");
    assert_eq!((claim.username.as_str(), claim.session_token.as_str(), claim.display.as_deref()), ("rosson", "login-session-token", Some("1")));
    assert_eq!(ticket::redeem(&t), None, "single use");
    let mel = login("mel", Role::Member);
    let (s, v) = call(&mel, "/cli/desktop/ticket", json!({}));
    assert_eq!((s, code(&v)), (403, "not_allowed"), "{v}");
}

/// A shell tab is a person: it manages (switch, grants), like the owner.
#[test]
fn a_shell_tab_manages_and_an_agent_doesnt() {
    let _e = env();
    login("mel", Role::Member);
    let (s, v) = call_at(&shell(), "/cli/desktop/enable", json!({}), "cell");
    assert_eq!(s, 200, "{v}");
    let (s, v) = call_at(&shell(), "/cli/desktop/grant", json!({"user": "mel"}), "cell");
    assert_eq!(s, 200, "{v}");
    let (s, v) = call_at(&agent(), "/cli/desktop/disable", json!({}), "cell");
    assert_eq!((s, code(&v)), (403, PEOPLE_ONLY), "{v}");
    assert!(store::is_enabled(CONSOLE).unwrap(), "the agent's disable changed nothing");
    let lines = audit::read(None, 10).unwrap();
    assert_eq!(lines[0]["who"], "shell");
    assert_eq!(lines[0]["event"], "grant");
    assert_eq!(lines[0]["target"], "mel");
}

/// R33: when the access log can't be written, nothing changes.
#[cfg(unix)]
#[test]
fn no_access_log_no_change() {
    let e = env();
    let owner = login("rosson", Role::Owner);
    // Make the log unwritable: replace it with a directory.
    let p = audit::path();
    std::fs::create_dir_all(&p).unwrap();
    let (s, v) = call(&owner, "/cli/desktop/enable", json!({}));
    assert_eq!((s, code(&v)), (503, "audit_unavailable"), "{v}");
    assert!(!store::is_enabled(CONSOLE).unwrap(), "no line, no switch");
    assert!(e.fake.calls.lock().unwrap().is_empty(), "deskd never started");
}

/// The policy route keeps the hide-list and refuses junk; a policy change
/// pushes to the Owners.
#[test]
fn the_hide_list_is_the_owners() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    let (s, v) = call(&owner, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200);
    assert_eq!(v["hideApps"].as_array().unwrap().len(), store::DEFAULT_HIDE_APPS.len(), "{v}");
    let mark = crate::push_routes::test_push_capture::mark();
    let (s, v) = call(&owner, "/cli/desktop/policy", json!({"hideApps": ["com.apple.Passwords", "com.example.Vault"]}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["hideApps"], json!(["com.apple.Passwords", "com.example.Vault"]));
    assert!(crate::push_routes::test_push_capture::since(mark)
        .iter()
        .any(|p| matches!(p, k2_core::push::PushEvent::DesktopNotice { kind_id, .. } if kind_id == "policy")));
    let (s, v) = call(&owner, "/cli/desktop/policy", json!({"hideApps": ["not a bundle"]}));
    assert_eq!((s, code(&v)), (400, "bad_hide_app"), "{v}");
    let (s, v) = call(&owner, "/cli/desktop/policy", json!({}));
    assert_eq!((s, code(&v)), (400, "bad_request"), "{v}");
    assert!(!store::is_enabled(CONSOLE).unwrap(), "a policy change never turns it on");
}

/// Kick closes 4403 `kicked`; open emits `open_desktop` for the caller.
#[test]
fn kick_and_open() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    let ann_id = {
        login("ann", Role::Admin);
        connect_users::user_uuid("ann").unwrap().unwrap()
    };
    let a = Arc::new(FakeViewer::default());
    let id = viewers::register(viewer("ann", &ann_id), a.clone());
    let (s, v) = call(&owner, "/cli/desktop/viewers", json!({}));
    assert_eq!(s, 200);
    assert_eq!(v["viewers"][0]["id"], id.as_str());
    let (s, v) = call(&owner, "/cli/desktop/kick", json!({"viewer": id}));
    assert_eq!((s, v["kicked"].as_u64()), (200, Some(1)), "{v}");
    assert_eq!(*a.0.lock().unwrap(), vec![(4403, "kicked".to_string())]);
    let (s, v) = call(&owner, "/cli/desktop/kick", json!({}));
    assert_eq!((s, code(&v)), (400, "bad_request"), "{v}");

    let mut events = crate::session_events::subscribe();
    let (s, v) = call(&owner, "/cli/desktop/open", json!({"display": "1"}));
    assert_eq!(s, 200, "{v}");
    let mut opened = None;
    while let Ok(ev) = events.try_recv() {
        if let crate::session_events::SessionEvent::OpenDesktop { payload } = ev {
            opened = Some(payload);
        }
    }
    let opened = opened.expect("open_desktop emitted");
    assert_eq!((opened["for"].as_str(), opened["display"].as_str(), opened["desktop"].as_str()), (Some("rosson"), Some("1"), Some("console")));
    assert!(opened.get("owners").is_none(), "a login's open is its own: {opened:?}");
    // From a K2 shell tab: this computer's app and every Owner login.
    let mut events = crate::session_events::subscribe();
    let (s, v) = call_at(&shell(), "/cli/desktop/open", json!({}), "cell");
    assert_eq!((s, v["for"].as_str()), (200, Some("owner")), "{v}");
    let mut opened = None;
    while let Ok(ev) = events.try_recv() {
        if let crate::session_events::SessionEvent::OpenDesktop { payload } = ev {
            opened = Some(payload);
        }
    }
    assert_eq!(opened.expect("open_desktop")["owners"], json!(["rosson"]), "Owner logins only, not the Admin");
    let mel = login("mel", Role::Member);
    let (s, v) = call(&mel, "/cli/desktop/open", json!({}));
    assert_eq!((s, code(&v)), (403, "not_allowed"), "{v}");
}

#[test]
fn audit_since_and_limit() {
    let _e = env();
    let owner = login("rosson", Role::Owner);
    call(&owner, "/cli/desktop/enable", json!({}));
    call(&owner, "/cli/desktop/disable", json!({}));
    let params: HashMap<String, String> = [("limit".to_string(), "1".to_string())].into();
    let req = Req { path: "/cli/desktop/audit", is_post: false, body: b"", params: &params, ingress: "tunnel", ip: "-", token: "" };
    let r = handle(&owner, &req);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["lines"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["lines"][0]["event"], "disable");
    assert!(parse_since("1d").is_ok() && parse_since("30m").is_ok() && parse_since("2026-11-10T00:00:00Z").is_ok());
    assert!(parse_since("yesterday").is_err());
    let params: HashMap<String, String> = [("since".to_string(), "soon".to_string())].into();
    let req = Req { path: "/cli/desktop/audit", is_post: false, body: b"", params: &params, ingress: "tunnel", ip: "-", token: "" };
    assert_eq!(handle(&owner, &req).status, "400 Bad Request");
}

/// The NotBuilt backend (every build before D3): the routes work, the
/// state says why there's no picture.
#[test]
fn without_deskd_the_state_says_unavailable() {
    let _home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let _d = deskd::set_for_test(Arc::new(deskd::NotBuilt));
    let (s, v) = call(&Who::Owner, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "unavailable");
    assert_eq!(v["enabled"], false);
    assert!(v["message"].as_str().unwrap().contains("K2 Screen") || cfg!(windows), "{v}");
    assert_eq!(v["displays"], Value::Null);
    let (s, v) = call(&Who::Owner, "/cli/desktop/displays", json!({}));
    assert_eq!((s, v["displays"].clone()), (200, json!([])));
}
