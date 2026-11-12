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
    _platform: PlatformGuard,
    fake: Arc<FakeDeskd>,
}

fn env() -> Env {
    let home = k2_core::test_env::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let fake = Arc::new(FakeDeskd { calls: Mutex::new(Vec::new()) });
    let guard = deskd::set_for_test(fake.clone());
    // The console desktop (a Mac, or a Linux box with someone at its
    // screen); the virtual desktop tests below pick Linux with no screen.
    let platform = platform_for_test(false, true);
    Env { _home: home, _db: db, _deskd: guard, _platform: platform, fake }
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
    assert_eq!(POST_ROUTES.len(), 11);
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
    assert_eq!(v["you"], json!({"role": "admin", "mayView": true, "mayControl": true, "mayManage": false, "sameMachine": false, "mayStart": false}));
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
    assert_eq!(v["you"], json!({"role": "member", "mayView": true, "mayControl": false, "mayManage": false, "sameMachine": false, "mayStart": false}));
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
    let _p = platform_for_test(false, true);
    let (s, v) = call(&Who::Owner, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["state"], "unavailable");
    assert_eq!(v["enabled"], false);
    assert!(v["message"].as_str().unwrap().contains("K2 Screen") || cfg!(windows), "{v}");
    assert_eq!(v["displays"], Value::Null);
    let (s, v) = call(&Who::Owner, "/cli/desktop/displays", json!({}));
    assert_eq!((s, v["displays"].clone()), (200, json!([])));
}

// ── virtual desktops (prd-linux-virtual-desktop-v1 L4) ───────────────

use crate::desktop::virtual_desktop::tests::Rig;
use crate::desktop::virtual_desktop::{self as vdm, TestManager};
use k2_core::desktop::virtual_desktop as vd;

/// A Linux server with no screen: a fake manager (fake runner and clock),
/// fake K2 Screens for `virtual` and `virtual:w1`, two workspaces (`w1`
/// with "Allow a virtual desktop" on, `w2` off) and the switch on.
struct VEnv {
    _e: Env,
    rig: Rig,
    _m: TestManager,
    _p: PlatformGuard,
    screens: Vec<(Arc<FakeDeskd>, deskd::TestDesktopDeskd)>,
}

impl VEnv {
    fn screen(&self, i: usize) -> Vec<&'static str> {
        self.screens[i].0.calls.lock().unwrap().clone()
    }
}

fn venv() -> VEnv {
    let e = env();
    let p = platform_for_test(true, false);
    let rig = Rig::new(true, true);
    super::super::virtual_routes::install_hooks(&rig.m);
    let m = vdm::set_for_test(rig.m.clone());
    let mut screens = Vec::new();
    for id in ["virtual", "virtual:w1"] {
        let f = Arc::new(FakeDeskd { calls: Mutex::new(Vec::new()) });
        let g = deskd::set_for_test_desktop(id, f.clone());
        screens.push((f, g));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('w1', 'scientist', '/tmp/w1'), ('w2', 'atlas', '/tmp/w2')", []).unwrap();
        store::set_agents_can_use_desktop(&conn, "w1", true).unwrap();
    }
    store::enable(vd::VIRTUAL, true, "owner").unwrap();
    VEnv { _e: e, rig, _m: m, _p: p, screens }
}

fn get_q(who: &Who, path: &str, q: &[(&str, &str)]) -> (u16, Value) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let req = Req { path, is_post: false, body: b"", params: &params, ingress: "tunnel", ip: "203.0.113.7", token: "t" };
    let r = handle(who, &req);
    let code: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: {e}: {}", r.body)))
}

#[test]
fn on_a_server_with_no_screen_this_server_is_the_virtual_desktop() {
    let _v = venv();
    assert_eq!(current_desktop(), "virtual");
    let owner = login("rosson", Role::Owner);
    let (s, v) = call(&owner, "/cli/desktop/status", json!({}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["desktop"].as_str(), v["kind"].as_str(), v["name"].as_str()), (Some("virtual"), Some("server"), Some("This server")));
    assert_eq!(v["state"], "closed", "{v}");
    assert_eq!(v["you"]["mayStart"], true);
    assert!(v["virtual"]["limits"]["desktops"]["max"].as_u64().is_some(), "{v}");
    assert!(v.get("hideApps").is_none(), "the hide list is the console's");
    let _s = platform_for_test(true, true);
    assert_eq!(current_desktop(), "console", "someone at the screen: the real session");
}

#[test]
fn start_returns_the_status_and_status_names_who_is_present_and_when_it_sleeps() {
    let v = venv();
    let owner = login("rosson", Role::Owner);
    let (s, r) = call(&owner, "/cli/desktop/start", json!({"desktop": "virtual", "width": 1601, "height": 901}));
    assert_eq!(s, 200, "{r}");
    assert_eq!((r["desktop"].as_str(), r["state"].as_str()), (Some("virtual"), Some("ready")), "{r}");
    assert_eq!(r["virtual"]["size"], json!({"width": 1600, "height": 900}));
    assert_eq!(audit_events(), vec!["desktop_start"]);
    let now = v.rig.m.now();
    v.rig.m.viewer_joined("virtual", "dv_1", "rosson", true);
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual")]);
    assert_eq!(st["virtual"]["present"], json!([{"viewer": "dv_1", "user": "rosson"}]), "{st}");
    assert_eq!(st["virtual"]["nextSleepAt"], Value::Null, "nobody leaves while someone watches");
    v.rig.m.viewer_hidden("virtual", "dv_1", true);
    v.rig.m.tick();
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual")]);
    assert_eq!(st["virtual"]["present"], json!([]));
    assert_eq!(st["virtual"]["nextSleepAt"], json!(now + vd::SLEEP_AFTER), "{st}");
    // Safe to repeat: a second start returns it as it is.
    let (s, r) = call(&owner, "/cli/desktop/start", json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("ready")), "{r}");
    assert_eq!(v.rig.runs("systemd-run").len(), 2, "started once");
}

#[test]
fn a_hidden_viewers_desktop_sleeps_after_the_timer_and_wakes_when_a_present_viewer_returns() {
    let v = venv();
    let owner = login("rosson", Role::Owner);
    call(&owner, "/cli/desktop/start", json!({}));
    v.rig.m.viewer_joined("virtual", "dv_1", "rosson", true);
    v.rig.m.viewer_hidden("virtual", "dv_1", true);
    v.rig.advance(vd::SLEEP_AFTER);
    v.rig.m.tick();
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[]);
    assert_eq!(st["state"], "asleep", "{st}");
    assert_eq!(st["message"], "The desktop is asleep. Everything on it is kept.");
    assert!(v.screen(0).contains(&"disable"), "K2 Screen's link to it closed: {:?}", v.screen(0));
    v.rig.m.viewer_present("virtual", "dv_1");
    v.rig.m.tick();
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[]);
    assert_eq!(st["state"], "ready", "{st}");
    let events = audit_events();
    assert!(events.contains(&"desktop_sleep".to_string()) && events.contains(&"desktop_wake".to_string()), "{events:?}");
}

#[test]
fn a_member_can_not_start_or_stop_and_an_agent_is_refused() {
    let _v = venv();
    let mel = login("mel", Role::Member);
    for path in ["/cli/desktop/start", "/cli/desktop/stop"] {
        let (s, r) = call(&mel, path, json!({}));
        assert_eq!((s, code(&r)), (403, "owner_or_admin"), "{path}: {r}");
        let (s, r) = call(&agent(), path, json!({}));
        assert_eq!((s, code(&r)), (403, PEOPLE_ONLY), "{path}: {r}");
    }
    let (s, r) = call(&mel, "/cli/desktop/setup", json!({}));
    assert_eq!((s, code(&r)), (403, "owner_only"), "{r}");
    let admin = login("ann", Role::Admin);
    let (s, r) = call(&admin, "/cli/desktop/start", json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("ready")), "an Admin starts it: {r}");
    let (s, r) = call(&admin, "/cli/desktop/stop", json!({}));
    assert_eq!((s, r["closed"].as_bool()), (200, Some(true)), "{r}");
    let (_, st) = get_q(&admin, "/cli/desktop/status", &[]);
    assert_eq!((st["state"].as_str(), st["reason"]["code"].as_str()), (Some("closed"), Some("closed_by")), "{st}");
    assert_eq!(st["message"], "The desktop was closed by ann.");
}

#[test]
fn two_desktops_keep_their_own_viewers_and_k2_screen_links() {
    let v = venv();
    let owner = login("rosson", Role::Owner);
    call(&owner, "/cli/desktop/start", json!({"desktop": "virtual"}));
    call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:w1"}));
    let a = Arc::new(FakeViewer::default());
    let b = Arc::new(FakeViewer::default());
    viewers::register(ViewerInfo { desktop: "virtual".into(), ..viewer("rosson", "u-r") }, a.clone());
    viewers::register(ViewerInfo { desktop: "virtual:w1".into(), ..viewer("rosson", "u-r") }, b.clone());
    let (_, l) = get_q(&owner, "/cli/desktop/viewers", &[("desktop", "virtual:w1")]);
    assert_eq!(l["viewers"].as_array().unwrap().len(), 1, "{l}");
    let (s, r) = call(&owner, "/cli/desktop/stop", json!({"desktop": "virtual:w1"}));
    assert_eq!(s, 200, "{r}");
    assert_eq!(*b.0.lock().unwrap(), vec![(4410, "desktop_closed".to_string())]);
    assert!(a.0.lock().unwrap().is_empty(), "the other desktop's viewer stays");
    assert!(v.screen(1).contains(&"disable"), "{:?}", v.screen(1));
    assert!(!v.screen(0).contains(&"disable"), "the other K2 Screen stays: {:?}", v.screen(0));
    assert_eq!(v.rig.m.state("virtual"), vd::State::Ready);
}

#[test]
fn the_list_shows_a_member_only_what_they_hold_a_grant_for() {
    let _v = venv();
    let owner = login("rosson", Role::Owner);
    let mel = login("mel", Role::Member);
    let (s, l) = get_q(&owner, "/cli/desktop/list", &[("workspace", "w1")]);
    assert_eq!(s, 200, "{l}");
    let ids: Vec<&str> = l["desktops"].as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["virtual", "virtual:w1"], "w2's switch is off: {l}");
    assert_eq!(l["default"], "virtual:w1", "a new tab in w1's workspace opens w1's desktop");
    assert_eq!(l["desktops"][1]["label"], vd::SEPARATE_LABEL);
    assert_eq!(l["desktops"][1]["name"], "scientist");
    let (_, l) = get_q(&owner, "/cli/desktop/list", &[("workspace", "w2")]);
    assert_eq!(l["default"], "virtual", "w2 has no desktop: This server");

    let (_, l) = get_q(&mel, "/cli/desktop/list", &[]);
    assert_eq!(l["desktops"], json!([]), "no grants, nothing to open: {l}");
    call(&owner, "/cli/desktop/grant", json!({"user": "mel", "desktop": "virtual:w1"}));
    let (_, l) = get_q(&mel, "/cli/desktop/list", &[]);
    let ids: Vec<&str> = l["desktops"].as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["virtual:w1"], "{l}");
}

#[test]
fn a_grant_on_one_desktop_opens_only_that_one() {
    let _v = venv();
    let owner = login("rosson", Role::Owner);
    let mel = login("mel", Role::Member);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_agents_can_use_desktop(&conn, "w2", true).unwrap();
    }
    call(&owner, "/cli/desktop/grant", json!({"user": "mel", "desktop": "virtual:w1"}));
    let (_, st) = get_q(&mel, "/cli/desktop/status", &[("desktop", "virtual:w1")]);
    assert_eq!(st["you"]["mayView"], true, "{st}");
    let (_, st) = get_q(&mel, "/cli/desktop/status", &[("desktop", "virtual:w2")]);
    assert_eq!(st["you"]["mayView"], false, "another agent's desktop: {st}");
    let (_, st) = get_q(&mel, "/cli/desktop/status", &[("desktop", "virtual")]);
    assert_eq!(st["you"]["mayView"], false, "nor the server's: {st}");
    let (s, r) = call(&mel, "/cli/desktop/ticket", json!({"desktop": "virtual:w2"}));
    assert_eq!((s, code(&r)), (403, "not_allowed"), "{r}");
    let (s, r) = call(&mel, "/cli/desktop/ticket", json!({"desktop": "virtual:w1"}));
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["desktop"], "virtual:w1");
}

#[test]
fn a_console_grant_does_not_open_virtual() {
    let _v = venv();
    let owner = login("rosson", Role::Owner);
    let mel = login("mel", Role::Member);
    call(&owner, "/cli/desktop/grant", json!({"user": "mel", "desktop": "console"}));
    let (_, st) = get_q(&mel, "/cli/desktop/status", &[("desktop", "console")]);
    assert_eq!(st["you"]["mayView"], true, "{st}");
    let (_, st) = get_q(&mel, "/cli/desktop/status", &[]);
    assert_eq!((st["desktop"].as_str(), st["you"]["mayView"].as_bool()), (Some("virtual"), Some(false)), "{st}");
}

#[test]
fn the_virtual_switch_has_its_own_card_and_off_closes_every_virtual_desktop() {
    let v = venv();
    let owner = login("rosson", Role::Owner);
    store::enable(vd::VIRTUAL, false, "owner").unwrap();
    let (s, r) = call(&owner, "/cli/desktop/start", json!({}));
    assert_eq!((s, code(&r)), (409, "desktop_off"), "{r}");
    let (s, r) = call(&owner, "/cli/desktop/enable", json!({}));
    assert_eq!((s, r["desktop"].as_str()), (200, Some("virtual")), "{r}");
    assert_eq!(r["card"], k2_core::desktop::VIRTUAL_ENABLE_CARD);
    assert!(v.screen(0).is_empty(), "turning it on starts nothing: {:?}", v.screen(0));
    call(&owner, "/cli/desktop/start", json!({}));
    call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:w1"}));
    let (s, r) = call(&owner, "/cli/desktop/disable", json!({"desktop": "virtual:w1"}));
    assert_eq!((s, r["desktop"].as_str()), (200, Some("virtual")), "one switch for every virtual desktop: {r}");
    assert_eq!((v.rig.m.state("virtual"), v.rig.m.state("virtual:w1")), (vd::State::Closed, vd::State::Closed));
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual:w1")]);
    assert_eq!(st["reason"]["code"], "switched_off", "{st}");
}

#[test]
fn an_agent_desktop_needs_its_workspace_and_its_switch() {
    let _v = venv();
    let owner = login("rosson", Role::Owner);
    let (s, r) = call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:w2"}));
    assert_eq!((s, code(&r)), (409, "agent_desktop_off"), "{r}");
    assert!(r["error"]["hint"].as_str().unwrap().contains("Allow a virtual desktop"), "{r}");
    let (s, r) = call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:gone1"}));
    assert_eq!((s, code(&r)), (404, "desktop_gone"), "{r}");
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual:gone1")]);
    assert_eq!((st["state"].as_str(), st["message"].as_str()), (Some("gone"), Some("This desktop no longer exists.")), "{st}");
    let (s, r) = get_q(&owner, "/cli/desktop/status", &[("desktop", "Virtual Thing")]);
    assert_eq!((s, code(&r)), (400, "bad_desktop"), "{r}");
    let (s, r) = call(&owner, "/cli/desktop/start", json!({"desktop": "console"}));
    assert_eq!((s, code(&r)), (409, "not_virtual"), "{r}");
}

#[test]
fn past_the_limit_start_says_which_could_close() {
    let v = venv();
    let owner = login("rosson", Role::Owner);
    v.rig.host.set_facts(4, 2 << 30, 8); // two may exist
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_agents_can_use_desktop(&conn, "w2", true).unwrap();
    }
    call(&owner, "/cli/desktop/start", json!({}));
    call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:w1"}));
    let (s, r) = call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:w2"}));
    assert_eq!((s, code(&r)), (409, "too_many_desktops"), "{r}");
    assert_eq!(r["closable"], json!(["virtual", "virtual:w1"]));
}

/// A fake root helper. `gate`: the install waits until it opens (so a
/// test sees `setting_up`); `ok: false` is a helper too old to ask.
struct FakeDoor {
    ok: bool,
    installs: Mutex<u32>,
    gate: (Mutex<bool>, std::sync::Condvar),
    /// The install's answer: the programs appear, or it fails.
    result: Mutex<Result<(), String>>,
    host: Option<Arc<crate::desktop::virtual_desktop::tests::Fake>>,
}

impl FakeDoor {
    fn new(ok: bool, host: Option<Arc<crate::desktop::virtual_desktop::tests::Fake>>) -> Arc<FakeDoor> {
        Arc::new(FakeDoor { ok, installs: Mutex::new(0), gate: (Mutex::new(true), std::sync::Condvar::new()), result: Mutex::new(Ok(())), host })
    }
    fn hold(&self) {
        *self.gate.0.lock().unwrap() = false;
    }
    fn open(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}

impl super::super::virtual_routes::SetupDoor for FakeDoor {
    fn helper_ok(&self) -> bool {
        self.ok
    }
    fn install(&self) -> Result<String, String> {
        *self.installs.lock().unwrap() += 1;
        let mut open = self.gate.0.lock().unwrap();
        while !*open {
            open = self.gate.1.wait(open).unwrap();
        }
        drop(open);
        self.result.lock().unwrap().clone()?;
        if let Some(h) = &self.host {
            h.programs.lock().unwrap().push("Xvfb");
        }
        Ok(r#"{"installed":["xvfb","icewm","fonts-dejavu-core"]}"#.into())
    }
}

/// Status until its state isn't `setting_up` (10 s at most).
fn setup_settles(who: &Who) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (_, st) = get_q(who, "/cli/desktop/status", &[("desktop", "virtual")]);
        if st["state"] != "setting_up" {
            return st;
        }
        assert!(std::time::Instant::now() < deadline, "setup never finished: {st}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The `desktop_state` events seen so far: (desktop, state, message).
fn desktop_states(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if ev.kind_name() == "desktop_state" {
            let j = serde_json::to_value(&ev).unwrap();
            let p = j.get("payload").cloned().unwrap_or_else(|| j.clone());
            let f = |k: &str| p[k].as_str().unwrap_or_else(|| panic!("desktop_state without {k}: {j}")).to_string();
            out.push((f("desktop"), f("state"), f("message")));
        }
    }
    out
}

/// `virtual`'s `desktop_state` events until one says `end` (the install's
/// thread emits after status already reads the outcome): 5 s at most.
fn virtual_states_until(rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>, end: &str) -> Vec<(String, String, String)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out = Vec::new();
    loop {
        out.extend(desktop_states(rx).into_iter().filter(|e| e.0 == "virtual"));
        if out.last().is_some_and(|e| e.1 == end) {
            return out;
        }
        assert!(std::time::Instant::now() < deadline, "no {end} event: {out:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn setup_with_an_old_helper_prints_the_command_and_asks_nothing() {
    let v = venv();
    v.rig.host.programs.lock().unwrap().retain(|p| *p != "Xvfb");
    let owner = login("rosson", Role::Owner);
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[]);
    assert_eq!(st["state"], "not_set_up", "{st}");
    assert!(st["virtual"]["manual"].as_str().unwrap().contains("apt-get install --no-install-recommends xvfb icewm"), "{st}");
    let old = FakeDoor::new(false, None);
    let _g = super::super::virtual_routes::door_for_test(old.clone());
    let (s, r) = call(&owner, "/cli/desktop/setup", json!({}));
    assert_eq!((s, code(&r)), (409, "setup_manual"), "{r}");
    assert!(r["manual"].as_str().unwrap().starts_with("sudo apt-get update"), "{r}");
    assert_eq!(*old.installs.lock().unwrap(), 0, "an old helper is never asked");
}

/// Contract note 11-11: setup answers `setting_up` at once and installs in
/// the background; `desktop_state` follows it to `closed`; a second setup
/// meanwhile is `409 setting_up`.
#[test]
fn setup_answers_at_once_and_installs_in_the_background() {
    let v = venv();
    v.rig.host.programs.lock().unwrap().retain(|p| *p != "Xvfb");
    let owner = login("rosson", Role::Owner);
    let helper = FakeDoor::new(true, Some(v.rig.host.clone()));
    helper.hold();
    let _g = super::super::virtual_routes::door_for_test(helper.clone());
    let mut rx = crate::session_events::subscribe();

    let (s, r) = call(&owner, "/cli/desktop/setup", json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("setting_up")), "answered before the install ends: {r}");
    assert_eq!(r["ok"], true);
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual")]);
    assert_eq!((st["state"].as_str(), st["message"].as_str()), (Some("setting_up"), Some("Installing the desktop packages…")), "{st}");
    assert_eq!(st["virtual"]["setup"]["running"], true, "{st}");
    let (s, r) = call(&owner, "/cli/desktop/setup", json!({}));
    assert_eq!((s, code(&r)), (409, "setting_up"), "one install at a time: {r}");
    let (s, r) = call(&owner, "/cli/desktop/start", json!({}));
    assert_eq!((s, code(&r)), (409, "setting_up"), "no start while the packages install: {r}");

    helper.open();
    let st = setup_settles(&owner);
    assert_eq!(st["state"], "closed", "{st}");
    assert_eq!(st["virtual"]["setup"]["running"], false, "{st}");
    assert_eq!(st["virtual"]["setup"]["failed"], Value::Null, "{st}");
    assert_eq!(*helper.installs.lock().unwrap(), 1);
    // The events: setting_up first, closed last, each with its sentence.
    let events = virtual_states_until(&mut rx, "closed");
    assert_eq!(events.first().map(|e| e.1.as_str()), Some("setting_up"), "{events:?}");
    assert_eq!(events.last().map(|e| e.1.as_str()), Some("closed"), "{events:?}");
    assert!(events.iter().all(|e| !e.2.is_empty()), "every event carries a message: {events:?}");
    assert_eq!(audit_events().iter().filter(|e| *e == "desktop_setup").count(), 2, "started and the outcome");
    let (s, r) = call(&owner, "/cli/desktop/setup", json!({}));
    assert_eq!((s, r["message"].as_str()), (200, Some("The desktop packages are already installed.")), "{r}");
    assert_eq!(*helper.installs.lock().unwrap(), 1, "nothing to install");
}

#[test]
fn a_failed_setup_ends_not_set_up_with_why() {
    let v = venv();
    v.rig.host.programs.lock().unwrap().retain(|p| *p != "Xvfb");
    let owner = login("rosson", Role::Owner);
    let helper = FakeDoor::new(true, None);
    *helper.result.lock().unwrap() = Err("apt-get: E: Unable to locate package xvfb".into());
    let _g = super::super::virtual_routes::door_for_test(helper.clone());
    let mut rx = crate::session_events::subscribe();
    let (s, r) = call(&owner, "/cli/desktop/setup", json!({}));
    assert_eq!((s, r["state"].as_str()), (200, Some("setting_up")), "{r}");
    let st = setup_settles(&owner);
    assert_eq!(st["state"], "not_set_up", "{st}");
    let msg = st["message"].as_str().unwrap();
    assert!(msg.contains("didn't install") && msg.contains("Unable to locate package xvfb"), "{msg}");
    assert!(st["virtual"]["setup"]["failed"].as_str().unwrap().contains("Unable to locate"), "{st}");
    let last = virtual_states_until(&mut rx, "not_set_up").pop().expect("an event");
    assert_eq!(last.1, "not_set_up");
    assert!(last.2.contains("didn't install"), "the event carries why: {last:?}");
    // Trying again clears the old failure while it runs.
    *helper.result.lock().unwrap() = Ok(());
    helper.hold();
    let (s, _) = call(&owner, "/cli/desktop/setup", json!({}));
    assert_eq!(s, 200);
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual")]);
    assert_eq!(st["virtual"]["setup"]["failed"], Value::Null, "{st}");
    helper.open();
    setup_settles(&owner);
}

#[test]
fn on_macos_status_never_reports_a_virtual_state_and_start_or_setup_is_linux_only() {
    let _e = env(); // a Mac: the console only
    let owner = login("rosson", Role::Owner);
    let (s, st) = call(&owner, "/cli/desktop/status", json!({}));
    assert_eq!((s, st["desktop"].as_str(), st["state"].as_str()), (200, Some("console"), Some("ready")), "{st}");
    assert!(st.get("virtual").is_none(), "{st}");
    assert_eq!(st["you"]["mayStart"], false);
    for (path, body) in [("/cli/desktop/start", json!({})), ("/cli/desktop/setup", json!({})), ("/cli/desktop/start", json!({"desktop": "virtual"}))] {
        let (s, r) = call(&owner, path, body);
        assert_eq!((s, code(&r)), (409, "linux_only"), "{path}: {r}");
        assert!(r["error"]["hint"].as_str().unwrap().contains("Linux servers only"), "{r}");
    }
    let (s, r) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual")]);
    assert_eq!((s, code(&r)), (409, "linux_only"), "{r}");
    let (_, l) = get_q(&owner, "/cli/desktop/list", &[]);
    assert_eq!(l["desktops"], json!([{"id": "console", "kind": "server", "name": "This server", "state": "ready"}]), "one entry: no picker");
}

#[test]
fn open_desktop_carries_the_id() {
    let _v = venv();
    let owner = login("rosson", Role::Owner);
    let mut events = crate::session_events::subscribe();
    let (s, r) = call(&owner, "/cli/desktop/open", json!({"desktop": "virtual:w1"}));
    assert_eq!((s, r["desktop"].as_str()), (200, Some("virtual:w1")), "{r}");
    let mut found = false;
    while let Ok(ev) = events.try_recv() {
        if ev.kind_name() == "open_desktop" {
            let j = serde_json::to_value(&ev).unwrap();
            assert!(j.to_string().contains("virtual:w1"), "{j}");
            found = true;
        }
    }
    assert!(found, "open_desktop sent");
}

#[test]
fn status_names_the_agent_holding_a_desktop() {
    let v = venv();
    let owner = login("rosson", Role::Owner);
    call(&owner, "/cli/desktop/start", json!({"desktop": "virtual:w1"}));
    v.rig.m.hold("virtual:w1", "scientist/helper").expect("hold");
    let (_, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", "virtual:w1")]);
    assert_eq!(st["virtual"]["hold"]["by"], "scientist/helper", "{st}");
    assert_eq!(st["virtual"]["hold"]["name"], "scientist (helper)", "{st}");
    assert_eq!(st["workspaceId"], "w1");
    assert_eq!(st["agentsAllowed"], true);
    assert_eq!(super::super::virtual_routes::hold_name("scientist"), "scientist");
}

#[test]
fn an_agent_desktop_id_maps_back_to_its_workspace_even_when_hashed() {
    let _v = venv();
    let owner = login("rosson", Role::Owner);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('Legacy.ID', 'legacy', '/tmp/legacy')", []).unwrap();
        store::set_agents_can_use_desktop(&conn, "Legacy.ID", true).unwrap();
    }
    let odd = vd::agent_desktop("Legacy.ID").unwrap();
    let (_, l) = get_q(&owner, "/cli/desktop/list", &[("workspace", "Legacy.ID")]);
    assert_eq!(l["default"], json!(odd), "the list returns the exact id to open: {l}");
    let (s, st) = get_q(&owner, "/cli/desktop/status", &[("desktop", &odd)]);
    assert_eq!(s, 200, "{st}");
    assert_eq!((st["name"].as_str(), st["workspaceId"].as_str()), (Some("legacy"), Some("Legacy.ID")), "{st}");
}
