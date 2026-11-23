//! `k2 agent isolation layout`: on stubs only. The root helper is a
//! [`Stub`] (no root call), this box's scope placement is never touched (the
//! stub records the start and the stop), and this computer's owner check is
//! `presence::fake` (no prompt).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use serde_json::{json, Value};

use super::*;
use crate::vault_routes::presence::{self, fake};

const WS_F: &str = "3f1c2d4e-0000-4000-8000-00000000abcd";
const WS_O: &str = "9a8b7c6d-0000-4000-8000-0000000000ef";

/// The root helper and the placement, recorded.
struct Stub {
    helper: Result<(), String>,
    answers: HashMap<&'static str, Result<Value, String>>,
    calls: RefCell<Vec<String>>,
    placing: Cell<bool>,
    started: RefCell<Option<Value>>,
    stopped: Cell<bool>,
}

impl Stub {
    fn new(answers: &[(&'static str, Value)]) -> Self {
        Stub {
            helper: Ok(()),
            answers: answers.iter().map(|(k, v)| (*k, Ok(v.clone()))).collect(),
            calls: RefCell::new(Vec::new()),
            placing: Cell::new(false),
            started: RefCell::new(None),
            stopped: Cell::new(false),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Host for Stub {
    fn helper_ok(&self) -> Result<(), String> {
        self.helper.clone()
    }
    fn agents(&self, verb: &str) -> Result<Value, String> {
        self.calls.borrow_mut().push(verb.to_string());
        self.answers.get(verb).cloned().unwrap_or_else(|| panic!("the stub has no answer for agents {verb}"))
    }
    fn start_placing(&self, layout: &Value) {
        assert!(self.started.borrow().is_none(), "placement started twice");
        *self.started.borrow_mut() = Some(layout.clone());
        self.placing.set(true);
    }
    fn stop_placing(&self) {
        self.stopped.set(true);
        self.placing.set(false);
    }
    fn placing(&self) -> bool {
        self.placing.get()
    }
}

/// This computer's owner check for the test thread, cleared on drop.
struct Presence;

impl Presence {
    fn offers(method: Option<&'static str>, approve: bool) -> Self {
        fake::set(Some(fake::Fake { method, approve }));
        Presence
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        fake::set(None);
    }
}

fn ready_layout(scopes: Value, survivors: Value) -> Value {
    json!({
        "ok": true,
        "ready": true,
        "p1": "refused",
        "pidMax": 4194304,
        "slices": { "k2agents.slice": { "present": true }, "k2agents-open.slice": { "present": true }, "k2agents-fenced.slice": { "present": true } },
        "anchors": { "k2agents-anchor.service": { "active": true, "state": "active" } },
        "scopes": scopes,
        "survivors": survivors,
    })
}

fn post(host: &Stub, caller: Caller, body: Value) -> (u16, Value) {
    let r = handle_with(host, caller, true, body.to_string().as_bytes(), &HashMap::new(), "test");
    parse_answer(&r)
}

fn parse_answer(r: &CliResponse) -> (u16, Value) {
    let code: u16 = r.status.split(' ').next().and_then(|c| c.parse().ok()).unwrap_or_else(|| panic!("status line {}", r.status));
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body));
    (code, v)
}

fn code_of(v: &Value) -> &str {
    v.pointer("/error/code").and_then(Value::as_str).unwrap_or_else(|| panic!("no error code: {v}"))
}

fn audit_outcomes() -> Vec<String> {
    k2_core::auth_audit::read_all()
        .expect("the audit log")
        .into_iter()
        .filter(|l| l["event"] == AUDIT_EVENT)
        .map(|l| l["outcome"].as_str().expect("outcome").to_string())
        .collect()
}

fn login(name: &str, role: k2_core::connect_users::Role) -> Caller {
    k2_core::connect_users::add_user(name, "a password 12345").expect("add the login");
    k2_core::connect_users::set_role(name, role).expect("its role");
    Caller::Login { username: name.to_string() }
}

/// The owner with this computer's check: `agents setup`, then `agents
/// layout` (ready), the setting on, and placement started with that layout;
/// running sessions are listed, not adopted.
#[test]
fn the_owner_turns_scopes_on_with_this_computers_check() {
    let _home = crate::test_support::TempHome::new();
    let _p = Presence::offers(Some(presence::LOCAL_AUTH), true);
    let layout = ready_layout(json!([]), json!([{ "ws": WS_O, "cgroup": "/u/agents/a-x", "pids": 2, "owner": 0, "adoptable": true }]));
    let host = Stub::new(&[("setup", json!({ "ok": true, "changed": true })), ("layout", layout.clone())]);
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::NotSet);
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on" }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(host.calls(), ["setup", "layout"], "setup first, then the report");
    assert_eq!(host.started.borrow().as_ref(), Some(&layout), "placement started with the ready layout");
    assert!(host.placing.get());
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::On, "the owner's setting is kept");
    assert_eq!((v["changed"].as_bool(), v["placing"].as_bool(), v["agentsInOldLayout"].as_u64()), (Some(true), Some(true), Some(1)), "{v}");
    assert!(v["appliesTo"].as_str().expect("appliesTo").contains("Running sessions keep their place"), "{v}");
    assert_eq!(
        fake::last_prompt().as_deref(),
        Some(format!("{}: turn on a root-owned scope for every agent session on this server", k2_core::brand::PRODUCT_NAME).as_str()),
        "this computer's prompt names the step"
    );
    assert!(audit_outcomes().iter().any(|o| o.starts_with("ok on")), "{:?}", audit_outcomes());
    // Again: already on, nothing asked or run.
    fake::set(Some(fake::Fake { method: Some(presence::LOCAL_AUTH), approve: false }));
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on" }));
    assert_eq!((s, v["changed"].as_bool()), (200, Some(false)), "{v}");
    assert_eq!(host.calls(), ["setup", "layout"]);
}

/// The owner without the check: refused with the confirm the CLI asks for,
/// and nothing ran. A declined prompt is refused too. A layout that isn't
/// ready after setup is refused with the reason, and nothing is turned on.
#[test]
fn turning_on_needs_the_check_and_a_ready_layout() {
    let _home = crate::test_support::TempHome::new();
    let layout = ready_layout(json!([]), json!([]));
    {
        // No prompt here (a Linux server): the token is required.
        let _p = Presence::offers(None, false);
        let host = Stub::new(&[("setup", json!({ "ok": true })), ("layout", layout.clone())]);
        let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on" }));
        assert_eq!((s, code_of(&v)), (403, presence::CODE), "{v}");
        assert_eq!((v["confirm"]["action"].as_str(), v["confirm"]["secretId"].as_str()), (Some(presence::ISOLATION_LAYOUT), Some(presence::ISOLATION_LAYOUT_BINDING)), "{v}");
        assert!(v.pointer("/error/hint").and_then(Value::as_str).expect("hint").contains("k2 agent isolation layout on"), "{v}");
        assert!(host.calls().is_empty(), "no setup without the owner check");
        assert!(host.started.borrow().is_none());
        // A token that no check minted: refused (by the vault's confirm
        // store, or because this test process has no vault open).
        let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on", "confirmToken": "not-a-token" }));
        assert!(matches!((s, code_of(&v)), (403, presence::CODE) | (503, "vault_not_open")), "{s} {v}");
        assert!(host.calls().is_empty());
    }
    {
        let _p = Presence::offers(Some(presence::LOCAL_AUTH), false);
        let host = Stub::new(&[("setup", json!({ "ok": true })), ("layout", layout.clone())]);
        let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on" }));
        assert_eq!((s, code_of(&v)), (403, "confirm_failed"), "{v}");
        assert!(host.calls().is_empty());
    }
    {
        let _p = Presence::offers(Some(presence::LOCAL_AUTH), true);
        let mut bad = layout.clone();
        bad["p1"] = json!("ALLOWED");
        let host = Stub::new(&[("setup", json!({ "ok": true })), ("layout", bad)]);
        let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on" }));
        assert_eq!((s, code_of(&v)), (409, "layout_not_ready"), "{v}");
        assert!(v.pointer("/error/hint").and_then(Value::as_str).expect("hint").contains("was not refused"), "{v}");
        assert_eq!(host.calls(), ["setup", "layout"]);
        assert!(host.started.borrow().is_none(), "nothing places scopes");
        assert_eq!(agent_scope::owner_choice(), OwnerChoice::NotSet, "the setting isn't kept");
    }
    {
        // An old helper: refused before any prompt.
        let _p = Presence::offers(Some(presence::LOCAL_AUTH), true);
        let mut host = Stub::new(&[]);
        host.helper = Err("the root helper is protocol 18; scopes need 19".into());
        let (s, v) = post(&host, Caller::Owner, json!({ "mode": "on" }));
        assert_eq!((s, code_of(&v)), (409, "helper_needed"), "{v}");
        assert!(host.calls().is_empty());
    }
    assert!(audit_outcomes().iter().all(|o| o.starts_with("refused on")), "every refusal is audited: {:?}", audit_outcomes());
    assert_eq!(audit_outcomes().len(), 5, "{:?}", audit_outcomes());
}

/// An agent's passport (TCP answers a passport on this route as
/// `WrongCredential`), and a shell tab's: 403 `owner_only`, and the walls
/// map ends an `on` refusal with the `agent.isolation-layout` card's line
/// (`off` and `status` aren't requestable).
#[test]
fn a_passport_is_refused_and_taught_the_card_for_on() {
    let _home = crate::test_support::TempHome::new();
    let _p = Presence::offers(Some(presence::LOCAL_AUTH), true);
    let host = Stub::new(&[("setup", json!({ "ok": true })), ("layout", ready_layout(json!([]), json!([])))]);
    for caller in [Caller::WrongCredential, Caller::Shell { project_id: WS_O.into() }] {
        for mode in ["on", "off", "status"] {
            let (s, v) = post(&host, caller.clone(), json!({ "mode": mode }));
            assert_eq!((s, code_of(&v)), (403, "owner_only"), "{caller:?} {mode}: {v}");
        }
    }
    assert!(host.calls().is_empty() && host.started.borrow().is_none() && !host.stopped.get());
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::NotSet);
    assert!(fake::last_prompt().is_none(), "no prompt for a passport");

    let params = HashMap::new();
    let teach = |mode: &str| -> Value {
        let body = json!({ "mode": mode });
        let (_, resp) = post(&host, Caller::WrongCredential, body.clone());
        let req = crate::permission::walls::Refused { status: 403, method: "POST", path: ROUTE, params: &params, body: Some(&body), response: &resp, workspace_id: WS_O };
        crate::permission::walls::permission_for(&req).expect("a 403 to a passport always says what to do")
    };
    let on = teach("on");
    assert_eq!(on["action"].as_str(), Some(crate::permission::actions::isolation_layout::ACTION), "{on}");
    let cmd = on["command"].as_str().expect("the Ask line");
    assert!(cmd.ends_with(&format!(" permission request {} --why \"<what you need it for>\"", crate::permission::actions::isolation_layout::ACTION)), "{cmd}");
    for mode in ["off", "status"] {
        let p = teach(mode);
        assert!(p.get("action").is_none() && p["requestable"] == json!(false), "{mode}: {p}");
    }
}

/// An admin login may read the status, never turn scopes on (it can't give
/// the owner check, so no prompt is shown) or off (that loosens).
#[test]
fn an_admin_reads_the_status_but_turns_nothing_on_or_off() {
    let _home = crate::test_support::TempHome::new();
    let _p = Presence::offers(Some(presence::LOCAL_AUTH), true);
    let eve = login("eve", k2_core::connect_users::Role::Admin);
    let layout = ready_layout(json!([]), json!([]));
    let host = Stub::new(&[("setup", json!({ "ok": true })), ("layout", layout.clone()), ("teardown", json!({ "ok": true }))]);
    let (s, v) = post(&host, eve.clone(), json!({ "mode": "on" }));
    assert_eq!((s, code_of(&v)), (403, "owner_only"), "{v}");
    assert!(v.pointer("/error/hint").and_then(Value::as_str).expect("hint").contains("an admin login can't give"), "{v}");
    assert!(fake::last_prompt().is_none(), "no prompt for an admin");
    let (s, v) = post(&host, eve.clone(), json!({ "mode": "off" }));
    assert_eq!((s, code_of(&v)), (403, "owner_only"), "{v}");
    assert!(v.pointer("/error/hint").and_then(Value::as_str).expect("hint").contains("loosens"), "{v}");
    assert!(host.calls().is_empty() && host.started.borrow().is_none() && !host.stopped.get(), "{:?}", host.calls());
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::NotSet);
    let (s, v) = post(&host, eve, json!({ "mode": "status" }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["layout"], layout);
    assert_eq!(audit_outcomes(), ["refused on: admin", "refused off: admin"]);
    // A member login never gets here (the route's Admin floor); the handler
    // refuses one too.
    let mia = login("mia", k2_core::connect_users::Role::Member);
    let (s, v) = post(&host, mia, json!({ "mode": "status" }));
    assert_eq!((s, code_of(&v)), (403, "owner_only"), "{v}");
}

/// `off` while a fenced session runs in a scope: 409, listing it (agent and
/// scope); nothing changes.
#[test]
fn off_is_refused_while_a_fenced_scope_is_live() {
    let _home = crate::test_support::TempHome::new();
    agent_scope::set_owner_choice(true, "owner").expect("on");
    let scopes = json!([
        { "ws": WS_F, "id": "0123456789abcdef", "kind": "fenced", "pids": 3, "adopted": false },
        { "ws": WS_O, "id": "fedcba9876543210", "kind": "open", "pids": 1, "adopted": false },
    ]);
    let host = Stub::new(&[("layout", ready_layout(scopes, json!([]))), ("teardown", json!({ "ok": true }))]);
    host.placing.set(true);
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "off" }));
    assert_eq!((s, code_of(&v)), (409, "fenced_scopes_live"), "{v}");
    let fenced = v.pointer("/error/fenced").and_then(Value::as_array).expect("the fenced list");
    assert_eq!(fenced.len(), 1, "only the fenced one: {v}");
    assert_eq!(fenced[0]["workspaceId"], WS_F);
    assert_eq!(fenced[0]["scope"], format!("k2-agent-{WS_F}:0123456789abcdef.scope"));
    assert!(!fenced[0]["agent"].as_str().expect("agent").is_empty());
    assert!(v.pointer("/error/hint").and_then(Value::as_str).expect("hint").contains(&format!("k2-agent-{WS_F}:0123456789abcdef.scope")), "{v}");
    assert_eq!(host.calls(), ["layout"], "no teardown");
    assert!(!host.stopped.get() && host.placing.get(), "placement unchanged");
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::On, "the setting unchanged");
    assert_eq!(audit_outcomes(), ["refused off: 1 fenced scope(s) live"]);
}

/// `off` with no scope live: placement stops, the setting is off, and
/// `agents teardown` runs. With an unfenced scope live: no teardown; the
/// slices stay for the next start.
#[test]
fn off_stops_placing_and_tears_down_when_nothing_is_live() {
    let _home = crate::test_support::TempHome::new();
    agent_scope::set_owner_choice(true, "owner").expect("on");
    let host = Stub::new(&[("layout", ready_layout(json!([]), json!([]))), ("teardown", json!({ "ok": true, "stopped": 0, "removed": 6 }))]);
    host.placing.set(true);
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "off" }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(host.calls(), ["layout", "teardown"]);
    assert!(host.stopped.get() && !host.placing.get());
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::Off);
    assert_eq!((v["setting"].as_str(), v["wasPlacing"].as_bool(), v["teardown"]["removed"].as_u64()), (Some("off"), Some(true), Some(6)), "{v}");

    agent_scope::set_owner_choice(true, "owner").expect("on again");
    let open = json!([{ "ws": WS_O, "id": "fedcba9876543210", "kind": "open", "pids": 1, "adopted": false }]);
    let host = Stub::new(&[("layout", ready_layout(open, json!([])))]);
    host.placing.set(true);
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "off" }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(host.calls(), ["layout"], "no teardown while a scope is live");
    assert!(host.stopped.get());
    assert!(v.get("teardown").is_none(), "{v}");
    assert!(v["message"].as_str().expect("message").contains("next daemon start"), "{v}");
    assert_eq!(agent_scope::owner_choice(), OwnerChoice::Off);
    assert_eq!(audit_outcomes(), ["ok off (scopes live: 0, teardown: yes)", "ok off (scopes live: 1, teardown: no)"]);
}

/// Status: the report's shape for the owner; a helper that can't report
/// gives `layoutError`; a GET is status, a GET naming on/off is 405, a bad
/// mode 400.
#[test]
fn status_reports_the_layout_and_the_setting() {
    let _home = crate::test_support::TempHome::new();
    let layout = ready_layout(json!([]), json!([]));
    let host = Stub::new(&[("layout", layout.clone())]);
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "status" }));
    assert_eq!(s, 200, "{v}");
    for key in ["placing", "setting", "whyOff", "p1", "pidMax", "oldLayout", "layout", "layoutError"] {
        assert!(v.get(key).is_some(), "{key} missing: {v}");
    }
    assert_eq!((v["placing"].as_bool(), v["setting"].as_str()), (Some(false), Some("not set")), "{v}");
    assert_eq!(v["whyOff"], json!(crate::agent_isolation::why_off_not_turned_on()));
    assert!(v["whyOff"].as_str().expect("whyOff").contains("agent isolation layout on"), "{v}");
    assert_eq!((&v["layout"], &v["layoutError"]), (&layout, &Value::Null));

    agent_scope::set_owner_choice(false, "owner").expect("off");
    let mut host = Stub::new(&[]);
    host.helper = Err("no root helper on this box".into());
    let r = handle_with(&host, Caller::Owner, false, b"", &HashMap::new(), "test");
    let (s, v) = parse_answer(&r);
    assert_eq!(s, 200, "a GET is status: {v}");
    assert_eq!((v["layout"].clone(), v["layoutError"].as_str()), (Value::Null, Some("no root helper on this box")), "{v}");
    assert_eq!(v["whyOff"].as_str(), Some(crate::agent_isolation::WHY_OFF_BY_OWNER));
    assert!(host.calls().is_empty());

    for mode in ["on", "off"] {
        let q: HashMap<String, String> = [("mode".to_string(), mode.to_string())].into_iter().collect();
        let r = handle_with(&host, Caller::Owner, false, b"", &q, "test");
        assert_eq!(r.status, "405 Method Not Allowed", "{mode}: {}", r.body);
    }
    let (s, v) = post(&host, Caller::Owner, json!({ "mode": "maybe" }));
    assert_eq!((s, code_of(&v)), (400, "bad_request"), "{v}");
    let r = handle_with(&host, Caller::Owner, true, b"not json", &HashMap::new(), "test");
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let r = handle_with(&host, Caller::Invalid, true, br#"{"mode":"status"}"#, &HashMap::new(), "test");
    assert_eq!(r.status, "401 Unauthorized", "{}", r.body);
    assert!(audit_outcomes().is_empty(), "status and bad requests change nothing: {:?}", audit_outcomes());
}

/// The pure parts of boot and of `on`: when teardown is due, and which
/// running agents are listed (not adopted) when scopes are turned on live.
#[test]
fn teardown_is_due_only_with_slices_and_no_live_scope() {
    use crate::agent_isolation::{running_in_old_layout, teardown_due};
    assert!(teardown_due(&ready_layout(json!([]), json!([]))));
    assert!(!teardown_due(&ready_layout(json!([{ "ws": WS_O, "id": "fedcba9876543210", "kind": "open" }]), json!([]))));
    let none = json!({ "slices": { "k2agents.slice": { "present": false } }, "anchors": { "k2agents-anchor.service": { "active": false } }, "scopes": [] });
    assert!(!teardown_due(&none), "nothing set up: nothing to remove");
    let anchors_only = json!({ "slices": {}, "anchors": { "k2agents-anchor.service": { "active": true } }, "scopes": [] });
    assert!(teardown_due(&anchors_only));
    let left = running_in_old_layout(&ready_layout(json!([]), json!([{ "ws": WS_O, "pids": 2, "adoptable": false }, { "pids": 1 }])));
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].0, WS_O);
    assert!(left[0].1.contains("next start"), "{left:?}");
}

/// The card: registered, the owner's, a tightening the route checks again
/// at the click; it never runs without this computer's owner check.
#[test]
fn the_card_is_the_owners_and_the_click_is_not_the_owner_check() {
    use crate::permission::actions::isolation_layout as card;
    use k2_core::permission::catalog::{Direction, Floor, Proof};
    let r = crate::permission::actions::find(card::ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.direction, r.spec.proof), (Floor::Owner, Direction::Tighten, Proof::Click));
    assert!(r.spec.words.consequences.contains("agent isolation layout on yourself"), "{}", r.spec.words.consequences);
    let w = &r.spec.walls[0];
    assert_eq!(w.route_prefix, ROUTE);
}
