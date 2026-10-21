//! App users' personal wallets, in process: real skin principals, App
//! grants and session passes; the daemon's test vault (fake key store);
//! the handlers the dispatcher calls; the spawn door; the sweep. No vendor
//! CLI, no network, no keychain.

use std::collections::HashMap;

use k2_core::llm_accounts::{store, Tool};
use k2_core::person_wallet::{self as pw, Sealer};
use k2_core::skin::{self, SkinPass};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;
use crate::person_wallet_runtime::{self as prt, VaultSealer};
use crate::sidecar_routes::{AgentCaller, Caller};

const MARKER: &str = "K2TEST_WALLET_SECRET";

fn cred(tag: &str) -> Vec<u8> {
    format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":4102444800000,"subscriptionType":"pro"}}}}"#).into_bytes()
}

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    app: String,
    own: String,
    ws_alice: String,
    ws_bob: String,
    alice: SkinPass,
    bob: SkinPass,
}

impl Fx {
    fn alice_id(&self) -> String {
        self.alice.principal_id.clone().unwrap()
    }
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    let (own, ws_alice, ws_bob) = (format!("own-{tag}"), format!("alice-ws-{tag}"), format!("bob-ws-{tag}"));
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&own, &ws_alice, &ws_bob] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/pw-{r}")]).unwrap();
        }
        k2_core::published_services::insert(&conn, &own, "hr", "node s.js", "/tmp", 4300, "local", "running", "cmd", "").unwrap().id
    };
    let mut passes = Vec::new();
    for (user, ws) in [("alice", &ws_alice), ("bob", &ws_bob)] {
        skin::add_principal_for_app(user, None, &app, None, "test").unwrap();
        skin::set_app_grant_rooms(user, &app, std::slice::from_ref(ws), true, "test").unwrap();
        let s = skin::create_app_session_token(user, &app).unwrap();
        passes.push(crate::apps::gateway::resolve_pass(&s.raw).expect("session pass resolves"));
    }
    let bob = passes.pop().unwrap();
    let alice = passes.pop().unwrap();
    assert!(alice.session && alice.principal_id.is_some());
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        pw::set_app_settings(&conn, &app, Some(true), None, "owner-token").unwrap();
    }
    Fx { _db: db, _home: home, app, own, ws_alice, ws_bob, alice, bob }
}

fn st(r: &CliResponse) -> u16 {
    r.status.split_whitespace().next().unwrap().parse().unwrap()
}

fn body(r: &CliResponse) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

fn code(v: &J) -> String {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}")).to_string()
}

fn skin_call(pass: Option<&SkinPass>, path: &str, post: bool, b: J, q: &[(&str, &str)]) -> (u16, J, String) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle_skin(path, pass.cloned(), post, b.to_string().as_bytes(), &params);
    (st(&r), body(&r), r.body.clone())
}

fn owner_call(caller: Caller, path: &str, post: bool, b: J, q: &[(&str, &str)]) -> (u16, J, String) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = handle_owner(path, caller, post, b.to_string().as_bytes(), &params, "test");
    (st(&r), body(&r), r.body.clone())
}

fn connect_login(name: &str, role: k2_core::connect_users::Role) -> Caller {
    k2_core::connect_users::add_user(name, "password123").expect("add_user");
    if role != k2_core::connect_users::Role::Member {
        k2_core::connect_users::set_role(name, role).expect("set_role");
    }
    Caller::Login { username: name.into() }
}

fn agent(project: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: project.into(), map_key: None, args: vec![], sandboxed: false })
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn)
}

/// A Claude subscription signed in for `pass`'s person, sealed in the real
/// (test) vault, the way a finished sign-in leaves it.
fn signed_in_claude(pass: &SkinPass, label: &str, tag: &str) -> pw::Item {
    let pid = pass.principal_id.clone().unwrap();
    let item = with_conn(|c| pw::create(c, &pid, pw::item_type::LLM_SUBSCRIPTION, "claude", label, Some(&pass.app))).unwrap();
    store::write_private(&store::slot_cred_path(Tool::Claude, &item.id), &cred(tag)).unwrap();
    let done = with_conn(|c| pw::finalize_signin(c, &VaultSealer, &item.id)).unwrap();
    assert!(done.sealed, "sealed in the vault's person scope");
    done
}

fn mark(f: &Fx, user: &str, ws: &str) {
    let (s, v, _) = owner_call(Caller::Owner, O_AGENT, true, json!({"app": f.app, "user": user, "workspace": ws}), &[]);
    assert_eq!(s, 200, "{v}");
}

fn spawn(key: &str, project: &str) -> Result<(HashMap<String, String>, Option<crate::llm_accounts_runtime::SpawnGuard>), k2_core::llm_accounts::WalletError> {
    let mut env = HashMap::new();
    let g = crate::llm_accounts_runtime::apply_spawn_login(Some("claude"), &[], &mut env, key, Some(project), None)?;
    Ok((env, g))
}

// ── Who has a wallet ────────────────────────────────────────────────

#[test]
fn connect_members_the_owner_agents_and_app_passes_have_no_wallet() {
    let f = fx("who");
    // Anything that isn't a k2skn_ pass (a Connect login — even an admin —,
    // the owner token, an agent passport) reaches the handler as `None`.
    for path in [LIST, STATUS, ADD, USE, REMOVE] {
        let post = SKIN_POST.contains(&path);
        let (s, v, _) = skin_call(None, path, post, json!({"provider": "claude"}), &[]);
        assert_eq!((s, code(&v).as_str()), (403, "person_pass_required"), "{path}: {v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("App user account"), "teaches: {v}");
    }
    // The App's own pass (a platform/remote pass) is not a person either.
    let app_pass = skin::link::mint_remote_pass(&f.app, "hr", &["thread:read".to_string()], &[f.own.clone()], 3600).unwrap().raw;
    let p = crate::apps::gateway::resolve_pass(&app_pass).expect("app pass");
    let (s, v, _) = skin_call(Some(&p), LIST, false, json!({}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "person_pass_required"), "{v}");
    // A GET on a POST verb is 405.
    let (s, _, _) = skin_call(Some(&f.alice), ADD, false, json!({}), &[]);
    assert_eq!(s, 405);
}

#[test]
fn the_wallet_is_off_until_the_app_turns_it_on() {
    let f = fx("off");
    with_conn(|c| pw::set_app_settings(c, &f.app, Some(false), None, "owner-token")).unwrap();
    let (s, v, _) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "wallet_off"), "{v}");
    with_conn(|c| pw::set_app_settings(c, &f.app, Some(true), None, "owner-token")).unwrap();
    let (s, v, _) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"], json!([]));
}

// ── Isolation between people ────────────────────────────────────────

#[test]
fn people_are_isolated_from_each_other() {
    let f = fx("iso");
    let key = format!("sk-ant-{MARKER}-alice-0123456789");
    let (s, v, raw) = skin_call(Some(&f.alice), ADD, true, json!({"type": "llm_api_key", "provider": "claude", "key": key}), &[]);
    assert_eq!(s, 200, "{v}");
    assert!(!raw.contains(MARKER), "the key is never echoed");
    assert_eq!(v["item"]["label"], json!("alice Claude"), "default label: <first name> <Tool>");
    assert_eq!(v["login"], J::Null);
    let id = v["item"]["id"].as_str().unwrap().to_string();
    let sub = signed_in_claude(&f.alice, "personal", "alice");

    // Alice sees hers, never the secret.
    let (_, v, raw) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!(v["items"].as_array().unwrap().len(), 2, "{v}");
    assert!(!raw.contains(MARKER));
    // Bob sees nothing of Alice's: list, status, use, remove, sign-in.
    let (_, v, _) = skin_call(Some(&f.bob), LIST, false, json!({}), &[]);
    assert_eq!(v["items"], json!([]), "{v}");
    for (path, post, b, q) in [
        (STATUS, false, json!({}), vec![("id", id.as_str())]),
        (REMOVE, true, json!({"id": id}), vec![]),
        (LOGIN, true, json!({"id": sub.id}), vec![]),
        (USE, true, json!({"provider": "claude", "id": sub.id}), vec![]),
    ] {
        let (s, v, _) = skin_call(Some(&f.bob), path, post, b, &q);
        assert_eq!((s, code(&v).as_str()), (404, "not_found"), "{path}: {v}");
    }
    // Still Alice's, untouched.
    assert!(with_conn(|c| pw::get(c, &id)).unwrap().is_some());
    assert_eq!(VaultSealer.unseal(&f.alice_id(), &sub.id).unwrap().unwrap(), cred("alice"));
}

#[test]
fn a_running_sign_in_is_visible_and_typeable_by_its_starter_only() {
    let f = fx("starter");
    let item = with_conn(|c| pw::create(c, &f.alice_id(), pw::item_type::LLM_SUBSCRIPTION, "claude", "work", Some(&f.app))).unwrap();
    let url = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&code_challenge=abc&state=xyz";
    let login = prt::insert_test_login(&item, Some(url), None, true);
    let (s, v, _) = skin_call(Some(&f.alice), LOGIN_STATUS, false, json!({}), &[("loginId", &login)]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["login"]["url"], json!(url), "the whole URL");
    assert_eq!(v["login"]["urlHost"], json!("claude.ai"));
    assert_eq!(v["login"]["needsPaste"], json!(true));
    assert!(v["login"].get("screen").is_none(), "no terminal screen is ever sent: {v}");
    // Bob: the same 404 as a sign-in that doesn't exist; can't type or cancel.
    let (s, v, _) = skin_call(Some(&f.bob), LOGIN_STATUS, false, json!({}), &[("loginId", &login)]);
    assert_eq!((s, code(&v).as_str()), (404, "not_found"), "{v}");
    let (s, _, _) = skin_call(Some(&f.bob), LOGIN_INPUT, true, json!({"loginId": login, "text": "stolen-code"}), &[]);
    assert_eq!(s, 404);
    let (s, _, _) = skin_call(Some(&f.bob), LOGIN_CANCEL, true, json!({"loginId": login}), &[]);
    assert_eq!(s, 404);
    // The owner has no door to it at all (the server's sign-in views are
    // a separate map; the owner token's bypass never applies here).
    assert!(crate::llm_accounts_runtime::view(&login, &"owner-token".to_string()).is_none());
}

// ── The owner's view ───────────────────────────────────────────────

#[test]
fn the_owner_sees_existence_label_and_status_but_never_a_secret() {
    let f = fx("owner");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    let (s, v, raw) = owner_call(Caller::Owner, O_PEOPLE, false, json!({}), &[("app", &f.app)]);
    assert_eq!(s, 200, "{v}");
    assert!(!raw.contains(MARKER), "no secret in the owner's view");
    let rows = v["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    let row = rows[0].as_object().unwrap();
    let mut keys: Vec<&str> = row.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, vec!["addedVia", "dormantSince", "id", "label", "provider", "sealed", "state", "type", "usedBy", "username"]);
    assert_eq!(row["id"], json!(sub.id));
    assert_eq!(row["username"], json!("alice"));
    assert_eq!(row["state"], json!("signed_in"));
    // An Admin login sees the same; a Member doesn't.
    let admin = connect_login("pw_admin_owner", k2_core::connect_users::Role::Admin);
    let (s, _, _) = owner_call(admin, O_PEOPLE, false, json!({}), &[]);
    assert_eq!(s, 200);
    let member = connect_login("pw_member_owner", k2_core::connect_users::Role::Member);
    let (s, v, _) = owner_call(member, O_PEOPLE, false, json!({}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "admin_required"), "{v}");
}

#[test]
fn force_remove_deletes_for_good_and_agents_cannot() {
    let f = fx("force");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    let (s, v, _) = owner_call(agent(&f.own), O_REMOVE, true, json!({"id": sub.id}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "owner_only"), "{v}");
    let (s, v, _) = owner_call(Caller::Owner, O_REMOVE, true, json!({"id": sub.id}), &[]);
    assert_eq!(s, 200, "{v}");
    assert!(with_conn(|c| pw::get(c, &sub.id)).unwrap().is_none(), "row gone");
    assert!(VaultSealer.unseal(&f.alice_id(), &sub.id).unwrap().is_none(), "vault secret destroyed");
    assert!(!pw::slot_dir(Tool::Claude, &sub.id).exists(), "slot folder gone");
    assert!(!store::removed_root().exists() || std::fs::read_dir(store::removed_root()).unwrap().next().is_none(), "not Trash");
}

// ── Agents and daemon.token ────────────────────────────────────────

#[test]
fn agents_never_read_a_wallet_and_the_person_scope_is_not_a_user_ref() {
    let f = fx("agents");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    // An agent of the App's own workspace: settings only, and only with switch 2.
    for (path, post) in [(O_PEOPLE, false), (O_AGENTS, false), (O_AGENT, true), (O_REMOVE, true)] {
        let (s, v, _) = owner_call(agent(&f.own), path, post, json!({"id": sub.id, "app": f.app}), &[("app", &f.app)]);
        assert_eq!((s, code(&v).as_str()), (403, "owner_only"), "{path}: {v}");
    }
    let (s, v, _) = owner_call(agent(&f.own), O_SETTINGS, true, json!({"app": f.app, "fallback": true}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "switch2_required"), "{v}");
    with_conn(|c| c.execute("UPDATE projects SET agents_can_manage_skin = 1, agents_can_manage_apps_full = 1 WHERE id = ?1", params![f.own])).unwrap();
    with_conn(|c| {
        c.execute(
            "INSERT INTO apps (app_id, service_id, owner_workspace, state, created_by, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 'running', 'test', 1, 1)",
            params![format!("app-{}", f.app), f.app, f.own],
        )
    })
    .unwrap();
    let (s, v, _) = owner_call(agent(&f.own), O_SETTINGS, true, json!({"app": f.app, "fallback": true}), &[]);
    assert_eq!(s, 200, "switch 2 sets the fallback: {v}");
    assert_eq!(v["settings"]["fallback"], json!(true));
    let (s, v, _) = owner_call(agent(&f.own), O_SETTINGS, true, json!({"app": f.app, "enabled": false}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "owner_only"), "the wallet on/off is the owner's: {v}");
    // Another workspace's agent: never.
    let (s, v, _) = owner_call(agent(&f.ws_bob), O_SETTINGS, false, json!({}), &[("app", &f.app)]);
    assert_eq!((s, code(&v).as_str()), (403, "switch2_required"), "{v}");
    // The person scope is never reachable as a user secret (k2v:// refs and
    // the user namespace refuse it); the value only resolves for the daemon.
    let r = crate::vault::selector_for_ref(&format!("k2v://person/{}/wallet-{}", f.alice_id(), sub.id));
    assert!(r.is_err(), "a person ref must not parse into a selector");
    let v = crate::vault::get().unwrap();
    let mut req = k2_vault::PutRequest::new(k2_vault::Namespace::User, k2_vault::Scope::person(f.alice_id()), "X", k2_vault::Kind::Generic);
    req.origin = "test".into();
    let e = k2_vault::SecretStore::put(&*v, req, k2_vault::SecretBytes::from_slice(b"x"), &k2_vault::Actor::System("test".into())).unwrap_err();
    assert_eq!(e.code, "scope_reserved");
}

// ── The spawn door ─────────────────────────────────────────────────

#[test]
fn the_persons_agent_runs_on_their_subscription_and_nothing_else_does() {
    let f = fx("spawn");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    mark(&f, "alice", &f.ws_alice);
    let slot = pw::slot_dir(Tool::Claude, &sub.id);
    // Her agent: its own key (the pinned chat = the workspace id).
    let (env, g) = spawn(&f.ws_alice, &f.ws_alice).expect("spawn");
    assert_eq!(env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some(slot.to_string_lossy().as_ref()));
    assert_eq!(std::fs::read(store::slot_cred_path(Tool::Claude, &sub.id)).unwrap(), cred("alice"), "unsealed for the session");
    let g = g.expect("guard");
    assert_eq!(g.login.source, k2_core::llm_accounts::pins::Source::PersonPin);
    drop(g);
    // The owner's extra tab in the same workspace: the server default.
    let (env, _) = spawn("tab-1", &f.ws_alice).unwrap();
    assert!(!env.contains_key("CLAUDE_CONFIG_DIR"), "{env:?}");
    // Bob's workspace (not Alice's agent): never her plan.
    let (env, _) = spawn(&f.ws_bob, &f.ws_bob).unwrap();
    assert!(!env.contains_key("CLAUDE_CONFIG_DIR"), "{env:?}");
    // Idle again: the sweep re-seals and wipes the plaintext.
    let sealed = prt::reseal_idle();
    assert!(sealed.contains(&sub.id), "{sealed:?}");
    assert!(!store::slot_cred_path(Tool::Claude, &sub.id).exists());
    assert_eq!(VaultSealer.unseal(&f.alice_id(), &sub.id).unwrap().unwrap(), cred("alice"));
}

#[test]
fn fallback_off_refuses_and_on_uses_the_company_plan() {
    let f = fx("fallback");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    mark(&f, "alice", &f.ws_alice);
    with_conn(|c| pw::set_state(c, &sub.id, "needs_login", None)).unwrap();
    let e = spawn(&f.ws_alice, &f.ws_alice).err().expect("refused");
    assert_eq!(e.code(), "person_login_needed", "{e}");
    assert!(e.to_string().contains("signs in again"), "{e}");
    let (s, _, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": f.app, "fallback": true}), &[]);
    assert_eq!(s, 200);
    let (env, _) = spawn(&f.ws_alice, &f.ws_alice).expect("falls back");
    assert!(!env.contains_key("CLAUDE_CONFIG_DIR"), "company plan (server default): {env:?}");
}

// ── Naming the App (smoke r6: settings <AppName> --on, guests wallet_off) ──

#[test]
fn settings_by_app_name_reach_the_guests_service_id() {
    let f = fx("byname");
    with_conn(|c| {
        c.execute(
            "INSERT INTO apps (app_id, service_id, owner_workspace, state, created_by, created_at, updated_at) \
             VALUES ('hrapp', ?1, ?2, 'running', 'test', 1, 1)",
            params![f.app, f.own],
        )
    })
    .unwrap();
    // Off by the App's name → the guest (keyed on the service id) is off.
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": "hrapp", "enabled": false}), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["serviceId"], json!(f.app), "stored under the service id: {v}");
    assert_eq!(v["appId"], json!("hrapp"), "{v}");
    let (s, v, _) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "wallet_off"), "{v}");
    // On by name (any case) → the guest is on.
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": "HRApp", "enabled": true}), &[]);
    assert_eq!(s, 200, "{v}");
    let (s, v, _) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!(s, 200, "the guest sees the wallet the owner turned on by name: {v}");
    // The GET by name reads the same row.
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, false, json!({}), &[("app", "hrapp")]);
    assert_eq!((s, v["settings"]["enabled"].clone(), v["settings"]["serviceId"].clone()), (200, json!(true), json!(f.app)), "{v}");
    // No row was ever written under the name.
    let n: i64 = with_conn(|c| c.query_row("SELECT COUNT(*) FROM app_wallet_settings WHERE service_id = 'hrapp' OR service_id = 'HRApp'", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn settings_by_service_id_and_workspace_name_still_work() {
    let f = fx("bysvc");
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": f.app, "enabled": false}), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["serviceId"], json!(f.app));
    assert_eq!(v["appId"], J::Null, "a published service with no App row: {v}");
    let (s, v, _) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!((s, code(&v).as_str()), (403, "wallet_off"), "{v}");
    let target = format!("{}/hr", f.own);
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": target, "enabled": true}), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["settings"]["serviceId"], json!(f.app));
    let (s, v, _) = skin_call(Some(&f.alice), LIST, false, json!({}), &[]);
    assert_eq!(s, 200, "{v}");
}

#[test]
fn an_unknown_app_is_refused_and_nothing_is_stored() {
    let f = fx("unknown");
    let before: i64 = with_conn(|c| c.query_row("SELECT COUNT(*) FROM app_wallet_settings", [], |r| r.get(0))).unwrap();
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": "nosuchapp-zz", "enabled": true}), &[]);
    assert_eq!((s, code(&v).as_str()), (404, "app_not_found"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("nosuchapp-zz"), "{v}");
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, false, json!({}), &[("app", "nosuchapp-zz")]);
    assert_eq!((s, code(&v).as_str()), (404, "app_not_found"), "{v}");
    let (s, v, _) = owner_call(Caller::Owner, O_PEOPLE, false, json!({}), &[("app", "nosuchapp-zz")]);
    assert_eq!((s, code(&v).as_str()), (404, "app_not_found"), "{v}");
    let (s, v, _) = owner_call(Caller::Owner, O_AGENT, true, json!({"app": "nosuchapp-zz", "user": "alice", "workspace": f.ws_alice}), &[]);
    assert_eq!((s, code(&v).as_str()), (404, "app_not_found"), "{v}");
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": "host", "enabled": true}), &[]);
    assert_eq!((s, code(&v).as_str()), (400, "bad_request"), "{v}");
    // An agent never learns whether a name is an App: unknown reads like
    // "not yours" (403 switch2_required), never a 404.
    let (s, v, _) = owner_call(agent(&f.own), O_SETTINGS, false, json!({}), &[("app", "nosuchapp-zz")]);
    assert_eq!((s, code(&v).as_str()), (403, "switch2_required"), "{v}");
    let after: i64 = with_conn(|c| c.query_row("SELECT COUNT(*) FROM app_wallet_settings", [], |r| r.get(0))).unwrap();
    assert_eq!(before, after, "no row for an unknown App");
    // An App's own service id is known even without a published row.
    with_conn(|c| {
        c.execute(
            "INSERT INTO apps (app_id, service_id, owner_workspace, state, created_by, created_at, updated_at) \
             VALUES ('bare', 'svc-bare-only', ?1, 'running', 'test', 1, 1)",
            params![f.own],
        )
    })
    .unwrap();
    let (s, v, _) = owner_call(Caller::Owner, O_SETTINGS, true, json!({"app": "svc-bare-only", "enabled": true}), &[]);
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["settings"]["serviceId"].clone(), v["appId"].clone()), (json!("svc-bare-only"), json!("bare")));
}

#[test]
fn a_shared_room_suspends_the_person_pin() {
    let f = fx("shared");
    signed_in_claude(&f.alice, "personal", "alice");
    mark(&f, "alice", &f.ws_alice);
    assert!(spawn(&f.ws_alice, &f.ws_alice).unwrap().0.contains_key("CLAUDE_CONFIG_DIR"));
    // Bob gets Alice's agent's room too: her plan may not answer him.
    skin::set_app_grant_rooms("bob", &f.app, &[f.ws_bob.clone(), f.ws_alice.clone()], true, "test").unwrap();
    let e = spawn(&f.ws_alice, &f.ws_alice).err().expect("refused while shared");
    assert_eq!(e.code(), "person_login_needed");
    assert!(e.to_string().contains("another App user"), "{e}");
}

#[test]
fn personal_agent_marks_need_the_room_through_that_app() {
    let f = fx("marks");
    let (s, v, _) = owner_call(Caller::Owner, O_AGENT, true, json!({"app": f.app, "user": "alice", "workspace": f.ws_bob}), &[]);
    assert_eq!((s, code(&v).as_str()), (409, "room_not_granted"), "{v}");
    mark(&f, "alice", &f.ws_alice);
    let (_, v, _) = owner_call(Caller::Owner, O_AGENTS, false, json!({}), &[]);
    assert_eq!(v["agents"][0]["username"], json!("alice"), "{v}");
    let (s, v, _) = owner_call(Caller::Owner, O_AGENT, true, json!({"workspace": f.ws_alice, "clear": true}), &[]);
    assert_eq!((s, v["cleared"].clone()), (200, json!(true)), "{v}");
}

// ── Lifecycle ──────────────────────────────────────────────────────

#[test]
fn losing_app_access_makes_items_dormant_then_deleted_after_30_days() {
    let f = fx("life");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    mark(&f, "alice", &f.ws_alice);
    let now = 1_900_000_000;
    // Alice's App grant goes: at once, her agent no longer runs on it.
    let grant = skin::list_grants().unwrap().into_iter().find(|g| g.subject_id == f.alice_id() && g.kind == "app" && g.target_id == f.app).unwrap();
    skin::set_grant_enabled(&grant.id, false).unwrap();
    let e = spawn(&f.ws_alice, &f.ws_alice).err().expect("access lost → refused at once");
    assert_eq!(e.code(), "person_login_needed");
    let (dormant, _, deleted, _) = prt::lifecycle(now);
    assert_eq!((dormant, deleted), (1, 0));
    assert!(with_conn(|c| pw::get(c, &sub.id)).unwrap().unwrap().dormant_since.is_some());
    // Re-granted within 30 days: back, no new sign-in.
    skin::set_grant_enabled(&grant.id, true).unwrap();
    let (_, woke, deleted, _) = prt::lifecycle(now + 86_400);
    assert_eq!((woke, deleted), (1, 0));
    assert!(spawn(&f.ws_alice, &f.ws_alice).unwrap().0.contains_key("CLAUDE_CONFIG_DIR"), "works again");
    let _ = prt::reseal_idle();
    // Lost again, and 30 days pass: deleted for good.
    skin::set_grant_enabled(&grant.id, false).unwrap();
    let _ = prt::lifecycle(now + 2 * 86_400);
    let (_, _, deleted, _) = prt::lifecycle(now + 2 * 86_400 + pw::DORMANT_KEEP_SECS);
    assert_eq!(deleted, 1);
    assert!(with_conn(|c| pw::get(c, &sub.id)).unwrap().is_none());
    assert!(VaultSealer.unseal(&f.alice_id(), &sub.id).unwrap().is_none());
}

#[test]
fn a_deleted_person_loses_their_wallet_and_marks_at_the_next_sweep() {
    let f = fx("gone");
    let bob_id = f.bob.principal_id.clone().unwrap();
    let key = format!("sk-{MARKER}-bob-0123456789");
    let (s, v, _) = skin_call(Some(&f.bob), ADD, true, json!({"type": "llm_api_key", "provider": "codex", "key": key}), &[]);
    assert_eq!(s, 200, "{v}");
    let id = v["item"]["id"].as_str().unwrap().to_string();
    mark(&f, "bob", &f.ws_bob);
    assert!(skin::remove_principal("bob").unwrap());
    let (_, _, deleted, _) = prt::lifecycle(1_900_000_000);
    assert_eq!(deleted, 1);
    assert!(with_conn(|c| pw::get(c, &id)).unwrap().is_none());
    assert!(VaultSealer.unseal(&bob_id, &id).unwrap().is_none());
    assert!(with_conn(|c| pw::person_agent(c, &f.ws_bob)).unwrap().is_none(), "his mark goes in the same pass");
    let audit = std::fs::read_to_string(dirs::home_dir().unwrap().join(".k2/auth-audit.jsonl")).unwrap_or_default();
    assert!(audit.contains("person_gone") && audit.contains(&f.ws_bob), "audited: {audit}");
}

#[test]
fn turning_a_person_into_an_agent_login_clears_their_wallet_and_their_marks() {
    let f = fx("toagent");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    mark(&f, "alice", &f.ws_alice);
    mark(&f, "bob", &f.ws_bob);
    // Alice's login becomes an agent's own login (no longer a person).
    skin::set_principal_agent("alice", Some(&f.own)).unwrap();
    let (_, _, deleted, _) = prt::lifecycle(1_900_000_000);
    assert_eq!(deleted, 1);
    assert!(with_conn(|c| pw::get(c, &sub.id)).unwrap().is_none(), "her items go");
    assert!(VaultSealer.unseal(&f.alice_id(), &sub.id).unwrap().is_none());
    assert!(with_conn(|c| pw::person_agent(c, &f.ws_alice)).unwrap().is_none(), "her personal-agent mark goes in the same pass");
    let bobs = with_conn(|c| pw::person_agent(c, &f.ws_bob)).unwrap().expect("Bob's mark is untouched");
    assert_eq!(bobs.principal_id, f.bob.principal_id.clone().unwrap());
    let audit = std::fs::read_to_string(dirs::home_dir().unwrap().join(".k2/auth-audit.jsonl")).unwrap_or_default();
    assert!(audit.contains("now_an_agent_login") && audit.contains(&f.ws_alice), "the mark's removal is audited: {audit}");
    // The owner's list no longer names her.
    let (_, v, _) = owner_call(Caller::Owner, O_AGENTS, false, json!({}), &[]);
    let rows = v["agents"].as_array().unwrap();
    assert!(rows.iter().all(|r| r["workspace"] != json!(f.ws_alice)), "{v}");
}

#[test]
fn the_person_removes_an_item_for_good() {
    let f = fx("rm");
    let sub = signed_in_claude(&f.alice, "personal", "alice");
    let (s, v, _) = skin_call(Some(&f.alice), REMOVE, true, json!({"id": sub.id}), &[]);
    assert_eq!(s, 200, "{v}");
    assert!(v["note"].as_str().unwrap().contains("vendor"), "{v}");
    assert!(with_conn(|c| pw::get(c, &sub.id)).unwrap().is_none());
    assert!(VaultSealer.unseal(&f.alice_id(), &sub.id).unwrap().is_none());
}

// ── URL capture (fixtures through S1's capture, which the wallet uses) ─

use crate::llm_accounts_runtime::{login_tick_from, LoginMode, LoginTick, LOGIN_COLS, LOGIN_HISTORY};

/// One tick over bytes rendered at the login PTY's width.
fn tick(bytes: &[u8], shim: &str, tool: Tool) -> LoginTick {
    let rows = k2_core::terminal::daemon_pty::scan_rows_from_bytes(LOGIN_COLS as usize, 40, LOGIN_HISTORY, bytes);
    login_tick_from(&rows, shim, tool, LoginMode::OtherDevice, None)
}

const CLAUDE: &str = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload+user%3Aplugins&code_challenge=hhX09VfGxZ4Oe_6zAToJMg0tIdpFMzGge4PW-yYvPLo&code_challenge_method=S256&state=ciSJq3Xk2mV7pL0aB9cD4eF6gH8iJ1kL2mN3oP4qR5s";

#[test]
fn person_sign_ins_capture_the_whole_link_from_real_shaped_output() {
    // Claude (~460 chars, OSC 8 + color, then the paste prompt).
    let bytes = format!(
        "Opening browser to sign in\u{2026}\r\nIf the browser didn't open, visit: \x1b]8;;{CLAUDE}\x07\x1b[94m{CLAUDE}\x1b[39m\x1b]8;;\x07\r\nPaste code here if prompted > "
    );
    let t = tick(bytes.as_bytes(), "", Tool::Claude);
    assert_eq!(t.url.as_deref(), Some(CLAUDE), "whole, never cut");
    assert!(t.paste);
    // The same URL, no OSC 8, hard-wrapped by a CLI at a narrow width: the
    // S1 scan joins it.
    let cols = 120;
    let mut hard = String::from("visit:\r\n");
    for chunk in CLAUDE.as_bytes().chunks(cols) {
        hard.push_str(std::str::from_utf8(chunk).unwrap());
        hard.push_str("\r\n");
    }
    hard.push_str("Paste code here if prompted > ");
    let rows = k2_core::terminal::daemon_pty::scan_rows_from_bytes(cols, 40, LOGIN_HISTORY, hard.as_bytes());
    let scan = crate::llm_accounts_runtime::scan_login(&rows, cols, "");
    let picked = crate::llm_accounts_runtime::pick_url(Tool::Claude, LoginMode::OtherDevice, &scan.candidates);
    assert_eq!(picked.map(|c| c.url).as_deref(), Some(CLAUDE), "hard wrap joined");
    // Codex device auth.
    let t = tick(b"Open https://auth.openai.com/codex/device\r\nand enter this one-time code: ABCD-12345\r\n", "", Tool::Codex);
    assert_eq!(t.url.as_deref(), Some("https://auth.openai.com/codex/device"));
    assert_eq!(t.code.as_deref(), Some("ABCD-12345"));
    assert!(!t.paste);
    // Grok device auth with the code in the link too.
    let t = tick(b"Visit https://accounts.x.ai/device?user_code=WXYZ-9QRT and enter code WXYZ-9QRT\r\n", "", Tool::Grok);
    assert_eq!(t.url.as_deref(), Some("https://accounts.x.ai/device?user_code=WXYZ-9QRT"));
    assert_eq!(t.code.as_deref(), Some("WXYZ-9QRT"));
    // A docs link printed before the sign-in link is not the sign-in link.
    let t = tick(b"See https://docs.example.test/help for help\r\nOpen https://auth.openai.com/codex/device and enter code ABCD-12345\r\n", "", Tool::Codex);
    assert_eq!(t.url.as_deref(), Some("https://auth.openai.com/codex/device"));
    // The BROWSER shim hands a URL over whole: it beats a cut screen read.
    let t = tick(b"visit https://claude.ai/cut", &format!("{CLAUDE}\n"), Tool::Claude);
    assert_eq!(t.url.as_deref(), Some(CLAUDE));
}
