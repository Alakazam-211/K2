//! `k2 vault systemd-cred` and placements (prd-vault-gaps-v1 item 4, plus
//! both security reviews' two rounds of fixes): route tests over a temp
//! vault, the fake key store and a fake root door. Nothing runs sudo, the
//! real helper or systemd.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine as _;
use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, KeyStore, LocalVault, OpenOptions, Role, SecretStore, Selector};
use serde_json::{json, Value};

use super::*;
use crate::root_helper::{Kind, RootHelperState};
use crate::vault::actor::{Channel, VaultCaller};
use crate::vault_routes::{handle_on, CONFIRM_START, DISABLE, DOCTOR, ROLLBACK, ROTATE, SET, SHOW};

const VALUE: &str = "k2test_SYSTEMD_CRED_SENTINEL_7a6b5c4d3e2f1a";
const VALUE2: &str = "k2test_SYSTEMD_CRED_SENTINEL_rotated_99887766";
const EVIL: &str = "k2test_SYSTEMD_CRED_agent_wrote_this_000";

struct Fake {
    linux: bool,
    ready: bool,
    /// argv[1] (`put`, `bind`, …) that fail.
    fail: RefCell<Vec<&'static str>>,
    /// What `creds status` says the allow list holds, and its orphans.
    allowed: RefCell<Vec<String>>,
    orphans: RefCell<Vec<Value>>,
    calls: RefCell<Vec<(Vec<String>, Option<Vec<u8>>)>>,
}

impl CredsDoor for Fake {
    fn linux(&self) -> bool {
        self.linux
    }
    fn helper(&self) -> RootHelperState {
        if !self.ready {
            return RootHelperState::missing();
        }
        RootHelperState {
            kind: Kind::Root,
            version: Some("0.46.4".into()),
            protocol: 10,
            path: "/usr/local/libexec/k2-root-helper".into(),
            package_owned: false,
            version_json: Some(json!({
                "protocol": 10,
                "namespaces": { "creds": true },
                "daemons": [{ "user": "k2", "unit": "k2test-daemon-box.service", "scope": "system" }],
            })),
            error: None,
        }
    }
    fn call(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        self.calls.borrow_mut().push((argv.to_vec(), stdin.map(<[u8]>::to_vec)));
        if argv.get(1).map(String::as_str) == Some("status") {
            return Ok(json!({ "allowedUnits": *self.allowed.borrow(), "orphans": *self.orphans.borrow() }).to_string().into_bytes());
        }
        if self.fail.borrow().iter().any(|f| argv.get(1).map(String::as_str) == Some(*f)) {
            return Err("exit 5: systemd-creds encrypt exit 1: no host key".into());
        }
        Ok(b"{}".to_vec())
    }
}

struct T {
    _dir: TestDir,
    v: Arc<LocalVault>,
    door: Rc<Fake>,
}

impl Drop for T {
    fn drop(&mut self) {
        test_seam::set(None);
    }
}

fn setup(linux: bool, ready: bool) -> T {
    let dir = TestDir::new("systemd-cred");
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(dir.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    let door = Rc::new(Fake {
        linux,
        ready,
        fail: RefCell::new(vec![]),
        allowed: RefCell::new(["k2-watch.service", "k2-control-plane.service", "k2-ops-push.service"].map(String::from).to_vec()),
        orphans: RefCell::new(vec![]),
        calls: RefCell::new(vec![]),
    });
    test_seam::set(Some(door.clone() as Rc<dyn CredsDoor>));
    T { _dir: dir, v, door }
}

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Tty, session_id: None }
}

fn member() -> VaultCaller {
    VaultCaller { actor: Actor::Login { username: "lee".into(), role: Role::Member }, channel: Channel::Login, session_id: None }
}

fn agent(ws: &str) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-1".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("sess-1".into()),
    }
}

fn call(t: &T, path: &str, c: &VaultCaller, body: Value) -> (u16, Value) {
    call_p(t, path, c, body, HashMap::new())
}

fn call_p(t: &T, path: &str, c: &VaultCaller, body: Value, params: HashMap<String, String>) -> (u16, Value) {
    let b = if body.is_null() { vec![] } else { body.to_string().into_bytes() };
    let r = handle_on(&t.v, path, Ok(c.clone()), &b, &params, "test", None);
    for v in [VALUE, VALUE2, EVIL] {
        assert!(!r.body.contains(v), "a value left in a response: {}", r.body);
    }
    let code: u16 = r.status[..3].parse().expect("status");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json for {path}: {}", r.body)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

/// A ws1 secret: ws1's agent makes it (so it lands in ws1), then the owner
/// sets its value, so the latest version (v2) is person-written.
fn secret(t: &T, name: &str, extra: Value) -> String {
    let mut body = json!({ "name": name, "kind": "api_key", "valueB64": b64("k2test_first_draft") });
    if let (Value::Object(b), Value::Object(e)) = (&mut body, extra) {
        b.extend(e);
    }
    let (s, v) = call(t, SET, &agent("ws1"), body);
    assert_eq!(s, 200, "{v}");
    let id = v["secret"]["id"].as_str().unwrap().to_string();
    let (s, v) = call(t, ROTATE, &owner(), json!({ "id": id, "valueB64": b64(VALUE) }));
    assert_eq!(s, 200, "{v}");
    id
}

fn latest(t: &T, id: &str) -> u32 {
    t.v.meta(&Selector::Id(id.into()), &Actor::Owner).unwrap().latest_version
}

fn mint(t: &T, digest: &str) -> String {
    t.v.confirm.mint(CONFIRM_ACTION, Some(digest), &Actor::Owner.stamp(), "vault-password")
}

fn place_token(t: &T, id: &str, unit: &str, restart: bool, accept: bool) -> String {
    mint(t, &plan_digest(Op::Place, id, latest(t, id), unit, "ingest_key", restart, accept, &[]))
}

fn place_body(t: &T, id: &str, unit: &str, restart: bool) -> Value {
    json!({ "id": id, "unit": unit, "credName": "ingest_key", "restart": restart, "version": latest(t, id),
            "confirmToken": place_token(t, id, unit, restart, false) })
}

fn behind_ids(t: &T, id: &str) -> Vec<String> {
    let l = latest(t, id);
    let mut ids: Vec<String> = t.v.placements(Some(id)).unwrap().rows.into_iter().filter(|p| p.version > 0 && p.version < l).map(|p| p.id).collect();
    ids.sort();
    ids
}

fn replace_body(t: &T, id: &str, accept: bool) -> Value {
    let rows = behind_ids(t, id);
    let tok = mint(t, &plan_digest(Op::Replace, id, latest(t, id), "", "", false, accept, &rows));
    json!({ "id": id, "replace": true, "version": latest(t, id), "rows": rows, "acceptAgentValue": accept, "confirmToken": tok })
}

fn remove_token(t: &T, id: &str, unit: &str) -> String {
    mint(t, &plan_digest(Op::Remove, id, 0, unit, "", false, false, &[]))
}

/// The helper calls that change the box (not the `creds status` read).
fn verbs(t: &T) -> Vec<String> {
    t.door.calls.borrow().iter().map(|(a, _)| a.join(" ")).filter(|v| v != "creds status").collect()
}

fn put_stdin(t: &T) -> Option<Vec<u8>> {
    t.door.calls.borrow().iter().rev().find(|(a, _)| a[1] == "put").and_then(|(_, i)| i.clone())
}

fn clear(t: &T) {
    t.door.calls.borrow_mut().clear();
}

fn assert_no_value_in_argv(t: &T) {
    for (a, _) in t.door.calls.borrow().iter() {
        for x in a {
            assert!(!x.contains("k2test_"), "a value in argv: {a:?}");
        }
    }
}

fn findings(t: &T, c: &VaultCaller, code_: &str) -> Vec<Value> {
    let (_, d) = call(t, DOCTOR, c, Value::Null);
    d["findings"].as_array().unwrap().iter().filter(|f| f["code"] == code_).cloned().collect()
}

/// Place INGEST_KEY on k2-watch.service (restart as asked). Returns its id.
fn placed(t: &T, restart: bool) -> String {
    let id = secret(t, "INGEST_KEY", json!({}));
    let (s, v) = call(t, SYSTEMD_CRED, &owner(), place_body(t, &id, "k2-watch.service", restart));
    assert_eq!(s, 200, "{v}");
    clear(t);
    id
}

#[test]
fn a_passport_never_reaches_systemd_cred() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    for path in [SYSTEMD_CRED, SYSTEMD_CRED_REMOVE] {
        let (s, v) = call(&t, path, &agent("ws1"), place_body(&t, &id, "k2-watch.service", false));
        assert_eq!((s, code(&v)), (403, "owner_only"), "{path}: {v}");
        let (s, v) = call(&t, path, &member(), place_body(&t, &id, "k2-watch.service", false));
        assert_eq!(s, 403, "{path} as a Member: {v}");
    }
    assert!(t.door.calls.borrow().is_empty());
}

#[test]
fn macos_answers_linux_only_and_a_missing_helper_gives_the_manual_steps() {
    let t = setup(false, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", false));
    assert_eq!((s, code(&v)), (400, "linux_only"), "{v}");
    drop(t);
    let t = setup(true, false);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", false));
    assert_eq!((s, code(&v)), (503, "root_helper_missing"), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("systemd-creds encrypt --name=ingest_key --with-key=auto"), "{hint}");
    assert!(hint.contains("LoadCredentialEncrypted=ingest_key:/etc/k2/credstore.encrypted/ingest_key.cred"), "{hint}");
    assert!(hint.contains("k2 vault placement add"), "{hint}");
    assert!(t.door.calls.borrow().is_empty());
}

#[test]
fn refusals_confirm_ceiling_deny_list_allow_list_and_cred_names() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let mut b = place_body(&t, &id, "k2-watch.service", false);
    b.as_object_mut().unwrap().remove("confirmToken");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), b);
    assert_eq!((s, code(&v)), (403, "vault_confirm_required"), "no confirm: {v}");
    let low = secret(&t, "LOW_KEY", json!({ "ceiling": "fetch" }));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &low, "k2-watch.service", false));
    assert_eq!((s, code(&v)), (400, "delivery_above_ceiling"), "{v}");
    for u in ["k2-daemon.service", "k2test-daemon-box.service", "k2app-x--d-1.service", "sshd.service"] {
        let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, u, false));
        assert_eq!((s, code(&v)), (400, "unit_not_allowed"), "{u}: {v}");
    }
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "nginx.service", false));
    assert_eq!((s, code(&v)), (400, "unit_not_allowed"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("/etc/k2-root-helper/systemd-cred-units"), "the admin's step: {v}");
    for u in ["k2-watch", "-x.service", "a/b.service", "foo@.service", "foo@bar.service", "k2-.service"] {
        let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, u, false));
        assert_eq!((s, code(&v)), (400, "bad_unit"), "{u}: {v}");
    }
    for cn in ["../x", "tmpfiles.extra", "ssh.authorized_keys.root", "login.motd", "systemd.unit-dropin.x"] {
        let mut b = place_body(&t, &id, "k2-watch.service", false);
        b["credName"] = json!(cn);
        let (s, v) = call(&t, SYSTEMD_CRED, &owner(), b);
        assert_eq!((s, code(&v)), (400, "bad_cred_name"), "{cn}: {v}");
    }
    let mut b = place_body(&t, &id, "k2-watch.service", false);
    b.as_object_mut().unwrap().remove("version");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), b);
    assert_eq!((s, code(&v)), (400, "version_required"), "{v}");
    assert!(verbs(&t).is_empty(), "nothing reached the helper: {:?}", verbs(&t));
}

/// A-M2 / B-L1 and A2-N2 / B2-N1: the confirm is bound to the unit, cred,
/// restart flag, op and the exact version; the daemon writes the text.
#[test]
fn a_confirm_is_bound_to_the_plan_and_the_version() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let v = latest(&t, &id);
    let wrong = [
        mint(&t, &plan_digest(Op::Place, &id, v, "k2-ops-push.service", "ingest_key", true, false, &[])),
        mint(&t, &plan_digest(Op::Place, &id, v, "k2-watch.service", "other_name", true, false, &[])),
        mint(&t, &plan_digest(Op::Place, &id, v, "k2-watch.service", "ingest_key", false, false, &[])),
        mint(&t, &plan_digest(Op::Place, &id, v - 1, "k2-watch.service", "ingest_key", true, false, &[])),
        mint(&t, &plan_digest(Op::Remove, &id, 0, "k2-watch.service", "ingest_key", false, false, &[])),
        t.v.confirm.mint(CONFIRM_ACTION, Some(&id), &Actor::Owner.stamp(), "vault-password"),
    ];
    for tok in wrong {
        let b = json!({ "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "restart": true, "version": v, "confirmToken": tok });
        let (s, v) = call(&t, SYSTEMD_CRED, &owner(), b);
        assert_eq!(s, 403, "a token for another plan: {v}");
    }
    assert!(verbs(&t).is_empty());
    // A version that moved: refused before any confirm is spent.
    let b = json!({ "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "version": v - 1,
                    "confirmToken": mint(&t, &plan_digest(Op::Place, &id, v - 1, "k2-watch.service", "ingest_key", false, false, &[])) });
    let (s, r) = call(&t, SYSTEMD_CRED, &owner(), b);
    assert_eq!((s, code(&r)), (409, "version_changed"), "{r}");
    let plan: CredPlan = serde_json::from_value(json!({ "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "restart": true, "op": "place", "version": v })).unwrap();
    let (digest, text) = confirm_binding(&t.v, &owner(), &plan).unwrap();
    assert_eq!(digest, plan_digest(Op::Place, &id, v, "k2-watch.service", "ingest_key", true, false, &[]));
    assert!(text.contains(&format!("INGEST_KEY v{v}")) && text.contains("written by the owner") && text.contains("restart"), "{text}");
    assert!(!text.contains("WARNING"), "{text}");
    let noversion: CredPlan = serde_json::from_value(json!({ "id": id, "unit": "k2-watch.service", "op": "place" })).unwrap();
    assert_eq!(confirm_binding(&t.v, &owner(), &noversion).unwrap_err().code, "version_required");
    let rm: CredPlan = serde_json::from_value(json!({ "id": id, "unit": "k2-watch.service", "op": "remove" })).unwrap();
    assert!(confirm_binding(&t.v, &owner(), &rm).unwrap().1.starts_with("take INGEST_KEY"));
    assert!(confirm_binding(&t.v, &agent("ws1"), &plan).is_err(), "no confirm plan for an agent");
    // confirm/start shows the same text before the person types.
    let (s, r) = call(&t, CONFIRM_START, &owner(), json!({ "action": "systemd-cred", "systemdCred": { "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "restart": true, "op": "place", "version": v } }));
    if s == 200 {
        assert_eq!(r["confirmText"].as_str(), Some(text.as_str()), "{r}");
    } else {
        assert_eq!(code(&r), "no_confirm_method", "{r}");
    }
}

#[test]
fn systemd_cred_puts_on_stdin_binds_restarts_and_records_a_placement() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), json!({ "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "dryRun": true }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["plan"]["dropinLine"], "LoadCredentialEncrypted=ingest_key:/etc/k2/credstore.encrypted/ingest_key.cred");
    assert_eq!((v["helperReady"].as_bool(), v["allowed"].as_bool()), (Some(true), Some(true)));
    assert_eq!((v["version"].as_u64(), v["author"].as_str(), v["authorIsPerson"].as_bool()), (Some(2), Some("owner"), Some(true)));
    assert!(v["confirmText"].as_str().unwrap().contains("k2-watch.service"));
    assert!(verbs(&t).is_empty());

    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", true));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["restarted"].as_bool(), v["version"].as_u64()), (Some(true), Some(2)));
    assert_eq!(verbs(&t), vec!["creds put ingest_key", "creds bind k2-watch.service ingest_key", "creds restart k2-watch.service"]);
    assert_eq!(put_stdin(&t).as_deref(), Some(VALUE.as_bytes()), "the value goes on stdin");
    assert!(t.door.calls.borrow().iter().filter(|(a, _)| a[1] != "put").all(|(_, i)| i.is_none()), "only put takes stdin");
    assert_no_value_in_argv(&t);
    let (s, _) = call(&t, SYSTEMD_CRED, &owner(), json!({ "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "restart": true, "version": 2 }));
    assert_eq!(s, 403, "a confirm works once");
    let (s, v) = call_p(&t, SHOW, &owner(), Value::Null, HashMap::from([("id".to_string(), id.clone())]));
    assert_eq!(s, 200, "{v}");
    let p = &v["placements"][0];
    assert_eq!((p["target"].as_str(), p["unit"].as_str(), p["state"].as_str()), (Some("systemd_cred"), Some("k2-watch.service"), Some("current")));
    let other = secret(&t, "OTHER_KEY", json!({}));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &other, "k2-ops-push.service", false));
    assert_eq!((s, code(&v)), (409, "cred_name_taken"), "{v}");
}

/// A-C1 / B-H1: "an agent rotate leaves the placement stale, with no
/// helper call". Members and even the owner's plain rotate too.
#[test]
fn an_agent_rotate_leaves_the_placement_stale_with_no_helper_call() {
    let t = setup(true, true);
    let id = placed(&t, true);
    let (s, v) = call(&t, ROTATE, &agent("ws1"), json!({ "id": id, "valueB64": b64(EVIL) }));
    assert_eq!(s, 200, "{v}");
    assert!(verbs(&t).is_empty(), "no creds put, no creds restart: {:?}", verbs(&t));
    assert_eq!(v["replaced"][0]["stale"], true, "{v}");
    assert!(v["replaced"][0]["error"]["hint"].as_str().unwrap().contains("--replace"), "{v}");
    let f = findings(&t, &owner(), "placement_stale");
    assert_eq!(f.len(), 1);
    assert!(f[0]["message"].as_str().unwrap().contains("holds v2 (latest v3)"), "{}", f[0]);
    let (s, _) = call(&t, SET, &agent("ws1"), json!({ "name": "INGEST_KEY", "valueB64": b64(EVIL) }));
    assert_eq!(s, 200);
    let (s, v) = call(&t, ROTATE, &owner(), json!({ "id": id, "valueB64": b64(VALUE2) }));
    assert_eq!(s, 200);
    assert_eq!(v["replaced"][0]["stale"], true);
    assert!(verbs(&t).is_empty(), "{:?}", verbs(&t));
    let (s, v) = call(&t, SYSTEMD_CRED, &agent("ws1"), json!({ "id": id, "replace": true }));
    assert_eq!((s, code(&v)), (403, "owner_only"));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), json!({ "id": id, "replace": true, "dryRun": true }));
    assert_eq!((s, v["behind"].as_u64()), (200, Some(1)), "{v}");
    assert!(v["confirmText"].as_str().unwrap().contains("k2-watch.service (restart)"), "{v}");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), replace_body(&t, &id, false));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["replaced"][0]["ok"], true);
    assert_eq!(verbs(&t), vec!["creds put ingest_key", "creds restart k2-watch.service"]);
    assert_eq!(put_stdin(&t).as_deref(), Some(VALUE2.as_bytes()), "the approved (person-written) version");
    assert!(findings(&t, &owner(), "placement_stale").is_empty());
}

/// A2-N2 / B2-N1: an agent rotates in a value and asks a person to
/// `--replace`: the sheet says an agent wrote it, and it is refused unless
/// the person explicitly accepts an agent-written value.
#[test]
fn an_agent_written_value_needs_an_explicit_accept_and_the_sheet_says_who_wrote_it() {
    let t = setup(true, true);
    let id = placed(&t, true);
    let (s, _) = call(&t, ROTATE, &agent("ws1"), json!({ "id": id, "valueB64": b64(EVIL) }));
    assert_eq!(s, 200);
    clear(&t);
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), json!({ "id": id, "replace": true, "dryRun": true }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["authorIsPerson"], false);
    assert!(v["author"].as_str().unwrap().starts_with("agent:ws1"), "{v}");
    let text = v["confirmText"].as_str().unwrap();
    assert!(text.starts_with("WARNING: no person wrote this value.") && text.contains("written by an agent of workspace"), "{text}");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), replace_body(&t, &id, false));
    assert_eq!((s, code(&v)), (409, "agent_written_value"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("k2 vault reveal INGEST_KEY"), "{v}");
    assert!(verbs(&t).is_empty());
    // A token for "not accepted" doesn't pass with accept set, nor the reverse.
    let mut b = replace_body(&t, &id, false);
    b["acceptAgentValue"] = json!(true);
    let (s, _) = call(&t, SYSTEMD_CRED, &owner(), b);
    assert_eq!(s, 403);
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), replace_body(&t, &id, true));
    assert_eq!(s, 200, "{v}");
    assert_eq!(put_stdin(&t).as_deref(), Some(EVIL.as_bytes()));
    // Rows that changed since the dry run: refused.
    let (s, _) = call(&t, ROTATE, &agent("ws1"), json!({ "id": id, "valueB64": b64(VALUE2) }));
    assert_eq!(s, 200);
    let mut b = replace_body(&t, &id, true);
    b["rows"] = json!(["not-a-row"]);
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), b);
    assert!(s == 409 || s == 403, "{v}");
}

/// B2-N4: a K2 subsystem writing a version (carry, handoff, …) is not on
/// the list, so nothing reaches the box.
#[test]
fn no_system_actor_may_re_place_today() {
    let t = setup(true, true);
    let id = placed(&t, false);
    assert!(REPLACE_SYSTEM_ACTORS.is_empty());
    let (s, _) = call(&t, ROTATE, &owner(), json!({ "id": id, "valueB64": b64(VALUE2) }));
    assert_eq!(s, 200);
    clear(&t);
    let m = t.v.meta(&Selector::Id(id.clone()), &Actor::Owner).unwrap();
    for sys in ["carry", "handoff", "clone", "adopt", "systemd-cred"] {
        let out = after_new_version(&t.v, &m, &Actor::System(sys.into()), Replacer::ConfirmedPerson);
        assert_eq!(out[0]["stale"], true, "{sys}: {out:?}");
    }
    assert!(verbs(&t).is_empty());
}

/// A2-N3 / B2-N2: the rollback confirm is bound to the target version and
/// lists the units; a plain rollback confirm no longer does it.
#[test]
fn rollback_of_a_placed_secret_needs_a_confirm_bound_to_the_target_and_units() {
    let t = setup(true, true);
    let id = placed(&t, true);
    let (s, _) = call(&t, ROTATE, &agent("ws1"), json!({ "id": id, "valueB64": b64(EVIL) }));
    assert_eq!(s, 200);
    clear(&t);
    let plain = t.v.confirm.mint("rollback", Some(&id), &Actor::Owner.stamp(), "vault-password");
    let (s, v) = call(&t, ROLLBACK, &owner(), json!({ "id": id, "version": 2, "confirmToken": plain }));
    assert_eq!(s, 403, "a confirm not bound to the units: {v}");
    let plan = RollbackPlan { sel: Sel { id: Some(id.clone()), ..Default::default() }, version: Some(2), accept_agent_value: false };
    let (digest, text) = rollback_binding(&t.v, &owner(), &plan).unwrap().unwrap();
    assert!(text.contains("v2") && text.contains("written by the owner") && text.contains("k2-watch.service (restart)"), "{text}");
    // v1 was the agent's draft: refused without the accept flag (A3-P3),
    // and the text warns with it.
    let other = RollbackPlan { sel: Sel { id: Some(id.clone()), ..Default::default() }, version: Some(1), accept_agent_value: false };
    assert_eq!(rollback_binding(&t.v, &owner(), &other).unwrap_err().code, "agent_written_value");
    let other = RollbackPlan { sel: Sel { id: Some(id.clone()), ..Default::default() }, version: Some(1), accept_agent_value: true };
    let (d1, t1) = rollback_binding(&t.v, &owner(), &other).unwrap().unwrap();
    assert_ne!(d1, digest, "bound to the target version");
    assert!(t1.starts_with("WARNING: no person wrote that version"), "{t1}");
    let tok = t.v.confirm.mint("rollback", Some(&digest), &Actor::Owner.stamp(), "vault-password");
    let (s, v) = call(&t, ROLLBACK, &owner(), json!({ "id": id, "version": 2, "confirmToken": tok }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["replaced"][0]["ok"], true, "{v}");
    assert_eq!(verbs(&t), vec!["creds put ingest_key", "creds restart k2-watch.service"]);
    assert_eq!(put_stdin(&t).as_deref(), Some(VALUE.as_bytes()), "the restored value");
    // A secret with no systemd credential keeps the plain rollback confirm.
    let plain_id = secret(&t, "PLAIN_KEY", json!({}));
    let p2 = RollbackPlan { sel: Sel { id: Some(plain_id.clone()), ..Default::default() }, version: Some(1), accept_agent_value: false };
    assert!(rollback_binding(&t.v, &owner(), &p2).unwrap().is_none());
    let tok = t.v.confirm.mint("rollback", Some(&plain_id), &Actor::Owner.stamp(), "vault-password");
    let (s, v) = call(&t, ROLLBACK, &owner(), json!({ "id": plain_id, "version": 1, "confirmToken": tok }));
    assert_eq!(s, 200, "{v}");
}

#[test]
fn a_disabled_secret_is_never_re_placed() {
    let t = setup(true, true);
    let id = placed(&t, false);
    let (s, _) = call(&t, ROTATE, &owner(), json!({ "id": id, "valueB64": b64(VALUE2) }));
    assert_eq!(s, 200);
    let (s, _) = call(&t, DISABLE, &owner(), json!({ "id": id }));
    assert_eq!(s, 200);
    clear(&t);
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), replace_body(&t, &id, false));
    assert_eq!((s, code(&v)), (403, "secret_not_active"), "{v}");
    assert!(verbs(&t).is_empty());
}

#[test]
fn a_failed_re_put_shows_as_placement_stale() {
    let t = setup(true, true);
    let id = placed(&t, false);
    let (s, _) = call(&t, ROTATE, &owner(), json!({ "id": id, "valueB64": b64(VALUE2) }));
    assert_eq!(s, 200);
    t.door.fail.borrow_mut().push("put");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), replace_body(&t, &id, false));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["ok"].as_bool(), v["replaced"][0]["error"]["code"].as_str()), (Some(false), Some("helper_failed")));
    assert_eq!(findings(&t, &owner(), "placement_stale").len(), 1);
    assert_no_value_in_argv(&t);
}

/// B2-N3: files a crash left behind, from `creds status`, to people only.
#[test]
fn the_doctor_lists_orphans_with_their_cleanup_to_people_only() {
    let t = setup(true, true);
    t.door.orphans.borrow_mut().push(json!({ "path": "/etc/k2/credstore.encrypted/lost.cred", "kind": "cred", "cleanup": "rm /etc/k2/credstore.encrypted/lost.cred" }));
    let f = findings(&t, &owner(), "creds_orphan");
    assert_eq!(f.len(), 1);
    assert!(f[0]["message"].as_str().unwrap().contains("As root: rm /etc/k2/credstore.encrypted/lost.cred"), "{}", f[0]);
    clear(&t);
    assert!(findings(&t, &agent("ws1"), "creds_orphan").is_empty());
    assert!(t.door.calls.borrow().is_empty(), "an agent's doctor never asks the helper");
}

#[test]
fn manual_rows_go_due_after_a_rotate_until_done_and_agents_keep_only_their_own() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let (s, v) = call(&t, PLACEMENT_ADD, &agent("ws1"), json!({ "id": id, "where": "vercel:prod/k2-web", "note": "Vercel prod" }));
    assert_eq!(s, 200, "{v}");
    let pid = v["placement"]["id"].as_str().unwrap().to_string();
    let (s, v) = call(&t, PLACEMENT_ADD, &agent("ws2"), json!({ "id": id, "where": "box:/etc/x" }));
    assert!(s == 403 || s == 404, "another workspace's secret: {v}");
    let (s, v) = call(&t, PLACEMENT_ADD, &agent("ws1"), json!({ "id": id, "where": "box:/x", "target": "systemd_cred" }));
    assert_eq!((s, code(&v)), (403, "owner_only"), "no agent writes a systemd_cred row: {v}");
    let (s, v) = call(&t, PLACEMENT_ADD, &agent("ws1"), json!({ "id": id, "where": "a\nb" }));
    assert_eq!((s, code(&v)), (400, "bad_where"), "{v}");
    let (s, _) = call(&t, ROTATE, &owner(), json!({ "id": id, "valueB64": b64(VALUE2) }));
    assert_eq!(s, 200);
    let due = findings(&t, &agent("ws1"), "placement_manual_due");
    assert_eq!(due.len(), 1);
    assert!(due[0]["message"].as_str().unwrap().contains(&format!("k2 vault placement done {pid}")), "{}", due[0]);
    let (s, v) = call(&t, PLACEMENT_DONE, &agent("ws1"), json!({ "id": pid }));
    assert_eq!((s, v["placement"]["version"].as_u64()), (200, Some(3)), "{v}");
    assert!(findings(&t, &agent("ws1"), "placement_manual_due").is_empty());
    let (_, l) = call(&t, PLACEMENTS, &agent("ws1"), Value::Null);
    assert_eq!(l["placements"].as_array().unwrap().len(), 1, "{l}");
    let (_, l) = call(&t, PLACEMENTS, &agent("ws2"), Value::Null);
    assert!(l["placements"].as_array().unwrap().is_empty(), "{l}");
    let (s, _) = call(&t, PLACEMENT_REMOVE, &agent("ws2"), json!({ "id": pid }));
    assert!(s == 403 || s == 404);
    let (s, v) = call(&t, PLACEMENT_REMOVE, &agent("ws1"), json!({ "id": pid }));
    assert_eq!(s, 200, "{v}");
    assert!(t.door.calls.borrow().is_empty(), "manual rows never touch the helper");
}

#[test]
fn systemd_cred_rows_are_people_only_and_remove_takes_the_cred_off() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", false));
    assert_eq!(s, 200, "{v}");
    let pid = v["placement"]["id"].as_str().unwrap().to_string();
    let (s, v) = call(&t, PLACEMENT_DONE, &agent("ws1"), json!({ "id": pid }));
    assert_eq!((s, code(&v)), (400, "placement_not_manual"), "{v}");
    for c in [agent("ws1"), owner()] {
        let (s, v) = call(&t, PLACEMENT_REMOVE, &c, json!({ "id": pid }));
        assert_eq!((s, code(&v)), (400, "use_systemd_cred_remove"), "{v}");
    }
    let (s, _) = call(&t, DISABLE, &owner(), json!({ "id": id }));
    assert_eq!(s, 200);
    assert_eq!(findings(&t, &owner(), "placement_of_inactive_secret").len(), 1, "the doctor warns; nothing is removed by itself (Q9)");
    clear(&t);
    let (s, v) = call(&t, SYSTEMD_CRED_REMOVE, &owner(), json!({ "id": id, "unit": "k2-watch.service" }));
    assert_eq!((s, code(&v)), (403, "vault_confirm_required"), "{v}");
    let place_tok = place_token(&t, &id, "k2-watch.service", false, false);
    let (s, _) = call(&t, SYSTEMD_CRED_REMOVE, &owner(), json!({ "id": id, "unit": "k2-watch.service", "confirmToken": place_tok }));
    assert_eq!(s, 403, "a place token doesn't remove");
    let (s, v) = call(&t, SYSTEMD_CRED_REMOVE, &owner(), json!({ "id": id, "unit": "k2-watch.service", "confirmToken": remove_token(&t, &id, "k2-watch.service") }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["credRemoved"], true);
    assert_eq!(verbs(&t), vec!["creds unbind k2-watch.service ingest_key", "creds remove ingest_key"]);
    assert!(t.v.placements(Some(&id)).unwrap().rows.is_empty());
    let (s, v) = call(&t, SYSTEMD_CRED_REMOVE, &owner(), json!({ "id": id, "unit": "k2-watch.service", "confirmToken": remove_token(&t, &id, "k2-watch.service") }));
    assert_eq!(s, 404, "{v}");
}

/// B-M2 / A-L3: no phantom state either way.
#[test]
fn a_failed_bind_takes_a_new_cred_back_out_and_a_failed_rollback_keeps_a_visible_row() {
    let t = setup(true, true);
    let id = secret(&t, "INGEST_KEY", json!({}));
    t.door.fail.borrow_mut().push("bind");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", false));
    assert_eq!((s, code(&v)), (502, "helper_failed"), "{v}");
    assert_eq!(verbs(&t), vec!["creds put ingest_key", "creds bind k2-watch.service ingest_key", "creds remove ingest_key"]);
    assert!(t.v.placements(Some(&id)).unwrap().rows.is_empty(), "rolled back cleanly: no row");
    clear(&t);
    t.door.fail.borrow_mut().push("remove");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", false));
    assert_eq!((s, code(&v)), (502, "helper_failed"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("placement row stays"), "{v}");
    let rows = t.v.placements(Some(&id)).unwrap().rows;
    assert_eq!((rows.len(), rows[0].version), (1, 0), "a pending row");
    let f = findings(&t, &owner(), "placement_stale");
    assert!(f[0]["message"].as_str().unwrap().contains("never fully written"), "{f:?}");
    let other = secret(&t, "OTHER_KEY", json!({}));
    t.door.fail.borrow_mut().push("put");
    let v2 = latest(&t, &other);
    let b = json!({ "id": other, "unit": "k2-ops-push.service", "credName": "other_key", "version": v2,
                    "confirmToken": mint(&t, &plan_digest(Op::Place, &other, v2, "k2-ops-push.service", "other_key", false, false, &[])) });
    let (s, _) = call(&t, SYSTEMD_CRED, &owner(), b);
    assert_eq!(s, 502);
    assert!(t.v.placements(Some(&other)).unwrap().rows.is_empty());
}

/// Review A3-P3: a plain rollback (no placement yet) to an agent's version
/// keeps the agent as author, so a later placement still warns and needs
/// --accept-agent-value.
#[test]
fn a_rollback_to_an_agent_version_still_needs_the_accept_flag_later() {
    let t = setup(true, true);
    // v1 by the agent (secret()'s draft), v2 by the owner.
    let id = secret(&t, "INGEST_KEY", json!({}));
    let tok = t.v.confirm.mint("rollback", Some(&id), &Actor::Owner.stamp(), "vault-password");
    let (s, v) = call(&t, ROLLBACK, &owner(), json!({ "id": id, "version": 1, "confirmToken": tok }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(latest(&t, &id), 3);
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), json!({ "id": id, "unit": "k2-watch.service", "credName": "ingest_key", "dryRun": true }));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["authorIsPerson"], false, "{v}");
    assert!(v["authorText"].as_str().unwrap().contains("a rollback to v1, first written by an agent"), "{v}");
    let (s, v) = call(&t, SYSTEMD_CRED, &owner(), place_body(&t, &id, "k2-watch.service", false));
    assert_eq!((s, code(&v)), (409, "agent_written_value"), "{v}");
    assert!(verbs(&t).is_empty());
}
