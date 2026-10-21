//! Person wallets: isolation between people, sealing, API keys, delete,
//! the person pin, and the person layer of the spawn decision. Temp HOME,
//! an in-memory fake vault, fake credentials with a marker; no CLI, no
//! keychain (off under cfg(test)), no network.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;

use super::decide::{self, Candidate, Facts, Plan, Unusable};
use super::*;
use crate::llm_accounts::pins::{self, ScopeKind, SpawnLogin};
use crate::skin::wallet_access::Person;

const MARKER: &str = "K2TEST_WALLET_SECRET";

struct Home {
    _vars: Vec<crate::test_env::EnvVar>,
    home: crate::test_env::TempHome,
}

impl Home {
    fn new() -> Home {
        let home = crate::test_env::TempHome::new();
        let _vars = ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME", "K2_AIRGAP"]
            .iter()
            .map(|v| crate::test_env::EnvVar::remove(v))
            .collect();
        crate::airgap::set_setting_enabled(false);
        Home { _vars, home }
    }
    fn path(&self) -> PathBuf {
        self.home.path().to_path_buf()
    }
}

fn db() -> Connection {
    let conn = Connection::open(":memory:").unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

fn cred(tag: &str) -> Vec<u8> {
    format!(
        r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":4102444800000,"subscriptionType":"max"}}}}"#
    )
    .into_bytes()
}

/// An in-memory vault keyed by (principal, item).
#[derive(Default)]
struct FakeVault {
    m: Mutex<HashMap<(String, String), (SealKind, Vec<u8>)>>,
    broken: bool,
}

impl Sealer for FakeVault {
    fn seal(&self, p: &str, i: &str, k: SealKind, b: &[u8]) -> Result<(), String> {
        if self.broken {
            return Err("vault: locked".into());
        }
        self.m.lock().unwrap().insert((p.into(), i.into()), (k, b.to_vec()));
        Ok(())
    }
    fn unseal(&self, p: &str, i: &str) -> Result<Option<Vec<u8>>, String> {
        if self.broken {
            return Err("vault: locked".into());
        }
        Ok(self.m.lock().unwrap().get(&(p.to_string(), i.to_string())).map(|(_, b)| b.clone()))
    }
    fn destroy(&self, p: &str, i: &str) -> Result<(), String> {
        self.m.lock().unwrap().remove(&(p.to_string(), i.to_string()));
        Ok(())
    }
}

impl FakeVault {
    fn get(&self, p: &str, i: &str) -> Option<Vec<u8>> {
        self.m.lock().unwrap().get(&(p.to_string(), i.to_string())).map(|(_, b)| b.clone())
    }
}

/// Every text cell of the wallet tables, for "no secret in the DB".
fn all_db_text(conn: &Connection) -> String {
    let mut out = String::new();
    for t in ["person_wallet_items", "person_wallet_use", "person_agents", "app_wallet_settings", "llm_session_logins"] {
        let mut st = conn.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            for c in 0..n {
                if let Ok(Some(s)) = r.get::<_, Option<String>>(c) {
                    out.push_str(&s);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// A subscription item whose CLI sign-in wrote `cred(tag)` into its slot.
fn signed_in_sub(conn: &Connection, v: &FakeVault, principal: &str, label: &str, tag: &str) -> Item {
    let item = create(conn, principal, item_type::LLM_SUBSCRIPTION, "claude", label, Some("hr-app")).unwrap();
    store::write_private(&store::slot_cred_path(Tool::Claude, &item.id), &cred(tag)).unwrap();
    finalize_signin(conn, v, &item.id).unwrap()
}

fn person(id: &str, app: &str, rooms: &[&str]) -> Person {
    let mut apps = BTreeMap::new();
    apps.insert(app.to_string(), rooms.iter().map(|r| r.to_string()).collect());
    Person { principal_id: id.into(), username: format!("user-{id}"), full_name: Some("Patty Lee".into()), is_agent_login: false, apps }
}

// ── Isolation between people ───────────────────────────────────────

#[test]
fn people_never_see_or_touch_each_others_items() {
    let _h = Home::new();
    let conn = db();
    let v = FakeVault::default();
    let a = signed_in_sub(&conn, &v, "p-alice", "personal", "alice");
    let b = signed_in_sub(&conn, &v, "p-bob", "personal", "bob");
    // The same label is fine for two people (unique per person, not server-wide).
    assert_eq!(a.label, b.label);
    assert_ne!(a.id, b.id);
    assert_eq!(list_for(&conn, "p-alice").unwrap().iter().map(|i| i.id.clone()).collect::<Vec<_>>(), vec![a.id.clone()]);
    assert_eq!(list_for(&conn, "p-bob").unwrap().iter().map(|i| i.id.clone()).collect::<Vec<_>>(), vec![b.id.clone()]);
    // Bob asking for Alice's item by id: the same NotFound as a made-up id (no oracle).
    let e = get_own(&conn, "p-bob", &a.id).unwrap_err();
    assert_eq!(e.code(), "not_found");
    assert_eq!(get_own(&conn, "p-bob", "pw_doesnotexist0000").unwrap_err().code(), "not_found");
    // Bob can't pin Alice's item for himself.
    assert_eq!(set_use(&conn, "p-bob", "claude", Some(&a.id)).unwrap_err().code(), "not_found");
    // Each person's sealed secret is their own.
    assert_eq!(v.get("p-alice", &a.id).unwrap(), cred("alice"));
    assert_eq!(v.get("p-bob", &b.id).unwrap(), cred("bob"));
    assert!(v.get("p-bob", &a.id).is_none(), "an item's secret is only under its owner's scope");
    // A duplicate label for the SAME person and provider is refused.
    assert_eq!(create(&conn, "p-alice", item_type::LLM_SUBSCRIPTION, "claude", "PERSONAL", None).unwrap_err().code(), "duplicate_label");
}

// ── Sealing ─────────────────────────────────────────────────────────

#[test]
fn a_finished_sign_in_is_sealed_and_no_plaintext_or_secret_stays_behind() {
    let h = Home::new();
    let conn = db();
    let v = FakeVault::default();
    let item = signed_in_sub(&conn, &v, "p-alice", "work", "a1");
    assert!(item.sealed, "sealed after sign-in");
    assert_eq!(item.state, "signed_in");
    assert_eq!(item.plan.as_deref(), Some("max"));
    assert_eq!(item.expires_at, Some(4_102_444_800));
    let cred_file = store::slot_cred_path(Tool::Claude, &item.id);
    assert!(!cred_file.exists(), "plaintext wiped: {}", cred_file.display());
    assert!(cred_file.starts_with(h.path()), "slot under the temp HOME");
    assert_eq!(v.get("p-alice", &item.id).unwrap(), cred("a1"));
    // The first signed-in item became the person pin.
    assert_eq!(use_for(&conn, "p-alice", "claude").unwrap().as_deref(), Some(item.id.as_str()));
    // No secret anywhere in the database.
    assert!(!all_db_text(&conn).contains(MARKER), "a secret reached the DB");

    // A spawn unseals it into the slot (0600) …
    unseal_into_slot(&v, &item).unwrap();
    assert_eq!(std::fs::read(&cred_file).unwrap(), cred("a1"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&cred_file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // … the CLI refreshes it while running …
    store::write_private(&cred_file, &cred("a2")).unwrap();
    // … and the sweep re-seals the refreshed one and wipes the file.
    let fresh = get(&conn, &item.id).unwrap().unwrap();
    assert!(seal_slot(&conn, &v, &fresh).unwrap());
    assert!(!cred_file.exists());
    assert_eq!(v.get("p-alice", &item.id).unwrap(), cred("a2"), "the refreshed token is what's sealed");
    // Nothing to seal now: a no-op, not an error.
    assert!(!seal_slot(&conn, &v, &fresh).unwrap());
}

#[test]
fn a_closed_vault_keeps_the_sign_in_unsealed_and_says_so() {
    let _h = Home::new();
    let conn = db();
    let v = FakeVault { broken: true, ..Default::default() };
    let item = create(&conn, "p-alice", item_type::LLM_SUBSCRIPTION, "claude", "work", None).unwrap();
    store::write_private(&store::slot_cred_path(Tool::Claude, &item.id), &cred("x")).unwrap();
    let done = finalize_signin(&conn, &v, &item.id).unwrap();
    assert_eq!(done.state, "signed_in");
    assert!(!done.sealed, "must report sealed=false, not pretend");
    assert!(slot_has_plaintext(Tool::Claude, &item.id), "the sign-in is kept, in its private folder");
}

#[test]
fn a_sign_in_that_never_finished_is_not_signed_in() {
    let _h = Home::new();
    let conn = db();
    let v = FakeVault::default();
    let item = create(&conn, "p-alice", item_type::LLM_SUBSCRIPTION, "codex", "work", None).unwrap();
    let e = finalize_signin(&conn, &v, &item.id).unwrap_err();
    assert_eq!(e.code(), "not_signed_in");
    assert_eq!(get(&conn, &item.id).unwrap().unwrap().state, "not_set_up");
    discard_unfinished(&conn, &v, &item.id).unwrap();
    assert!(get(&conn, &item.id).unwrap().is_none());
}

#[test]
fn api_keys_live_only_in_the_vault_and_reach_only_a_spawn_env() {
    let _h = Home::new();
    let conn = db();
    let v = FakeVault::default();
    let key = format!("sk-ant-{MARKER}-0123456789");
    let item = add_api_key(&conn, &v, "p-alice", "gemini", "my key", &key, Some("hr-app")).unwrap();
    assert_eq!(item.state, "signed_in");
    assert!(item.sealed);
    assert!(item.is_api_key());
    assert!(!slot_dir(Tool::Gemini, &item.id).exists(), "no slot, no file for an API key");
    assert!(!all_db_text(&conn).contains(MARKER), "the key reached the DB");
    let env = spawn_env(&v, &item).unwrap();
    assert_eq!(env, vec![("GEMINI_API_KEY".to_string(), key.clone())]);
    // A closed vault: nothing is saved (no half item).
    let broken = FakeVault { broken: true, ..Default::default() };
    assert!(add_api_key(&conn, &broken, "p-alice", "claude", "k2", &key, None).is_err());
    assert_eq!(list_for(&conn, "p-alice").unwrap().len(), 1);
}

#[test]
fn delete_is_for_good_not_trash() {
    let _h = Home::new();
    let conn = db();
    let v = FakeVault::default();
    let item = signed_in_sub(&conn, &v, "p-alice", "work", "a");
    unseal_into_slot(&v, &item).unwrap();
    let dir = slot_dir(Tool::Claude, &item.id);
    assert!(dir.exists());
    delete_item(&conn, &v, &item).unwrap();
    assert!(!dir.exists(), "slot folder deleted");
    assert!(v.get("p-alice", &item.id).is_none(), "vault secret destroyed");
    assert!(get(&conn, &item.id).unwrap().is_none(), "row deleted");
    assert!(use_for(&conn, "p-alice", "claude").unwrap().is_none(), "pin cleared");
    assert!(!store::removed_root().exists() || std::fs::read_dir(store::removed_root()).unwrap().next().is_none(), "nothing went to Trash");
}

#[test]
fn a_persons_codex_home_is_fully_private() {
    let h = Home::new();
    // A shared ~/.codex with sessions that must NOT be linked into a person's home.
    std::fs::create_dir_all(h.path().join(".codex/sessions")).unwrap();
    std::fs::write(h.path().join(".codex/history.jsonl"), b"owner history").unwrap();
    prepare_person_home(Tool::Codex, "pw_testcodex000001", None).unwrap();
    let slot = slot_dir(Tool::Codex, "pw_testcodex000001");
    for ent in std::fs::read_dir(&slot).unwrap().flatten() {
        let m = std::fs::symlink_metadata(ent.path()).unwrap();
        assert!(!m.file_type().is_symlink(), "a person's Codex home links nothing: {}", ent.path().display());
    }
    let cfg = std::fs::read_to_string(slot.join("config.toml")).unwrap();
    assert!(cfg.contains("cli_auth_credentials_store = \"file\""));
}

// ── Settings and marks ─────────────────────────────────────────────

#[test]
fn app_settings_default_off_and_host_is_not_an_app() {
    let conn = db();
    let s = app_settings(&conn, "hr-app").unwrap();
    assert!(!s.enabled && !s.fallback);
    let (before, after) = set_app_settings(&conn, "hr-app", Some(true), None, "owner-token").unwrap();
    assert!(!before.enabled && after.enabled && !after.fallback);
    let (_, after) = set_app_settings(&conn, "hr-app", None, Some(true), "agent:ws").unwrap();
    assert!(after.enabled && after.fallback, "a partial write keeps the other setting");
    assert!(set_app_settings(&conn, "host", Some(true), None, "owner-token").is_err());
}

// ── The spawn decision ─────────────────────────────────────────────

struct World {
    conn: Connection,
    v: FakeVault,
    item: Item,
}

/// Alice's Claude is signed in; workspace `ws-alice` is her personal agent
/// through `hr-app`; the App's wallet is on, fallback as given.
fn world(fallback: bool) -> World {
    let conn = db();
    let v = FakeVault::default();
    let item = signed_in_sub(&conn, &v, "p-alice", "work", "a");
    set_person_agent(&conn, "ws-alice", Some(("p-alice", "hr-app")), Some("owner-token")).unwrap();
    set_app_settings(&conn, "hr-app", Some(true), Some(fallback), "owner-token").unwrap();
    World { conn, v, item }
}

fn alice_facts() -> Facts {
    Facts { person: Some(person("p-alice", "hr-app", &["ws-alice"])), others_in_room: vec![] }
}

fn cand(w: &World, key: &str) -> Option<Candidate> {
    decide::candidate(&w.conn, Tool::Claude, key, Some("ws-alice"), None, false).unwrap()
}

#[test]
fn the_persons_own_agent_uses_their_subscription_and_nothing_else_does() {
    let _h = Home::new();
    let w = world(false);
    // Her agent's own session (pinned chat key = workspace id, or a canonical key).
    for key in ["ws-alice", "canonical:ws-alice"] {
        let c = cand(&w, key).expect("person layer applies");
        match decide::plan(&c, &alice_facts()) {
            Plan::Person(i) => assert_eq!(i.id, w.item.id, "{key}"),
            other => panic!("{key}: {other:?}"),
        }
    }
    // The owner's extra tab in the same workspace: never the person's plan.
    assert!(cand(&w, "tab-123").is_none());
    // Another workspace (not marked): never.
    assert!(decide::candidate(&w.conn, Tool::Claude, "ws-other", Some("ws-other"), None, false).unwrap().is_none());
    // A tool she has no item for: the existing rules.
    assert!(decide::candidate(&w.conn, Tool::Codex, "ws-alice", Some("ws-alice"), None, false).unwrap().is_none());
}

#[test]
fn precedence_chat_pin_beats_the_person_pin_which_beats_the_workspace_pin() {
    let _h = Home::new();
    let w = world(false);
    // A server API key the owner pins on the workspace: the person pin still wins.
    let srv = crate::llm_accounts::pins::add_api_key(&w.conn, Tool::Claude, "company", "sk-company-0123456789", None).unwrap();
    pins::pin(&w.conn, ScopeKind::Workspace, "ws-alice", Some(Tool::Claude), &srv.id, None).unwrap();
    assert!(cand(&w, "ws-alice").is_some(), "person pin > workspace pin");
    // An explicit chat pin on her chat wins over the person pin.
    pins::pin_chat(&w.conn, "ws-alice", None, Some(Tool::Claude), &srv.id, None).unwrap();
    assert!(cand(&w, "ws-alice").is_none(), "chat pin > person pin: the existing rules decide");
    let l = pins::decide_spawn(&w.conn, Tool::Claude, "ws-alice", Some("ws-alice"), None, false).unwrap();
    assert_eq!(l.account_id.as_deref(), Some(srv.id.as_str()));
}

#[test]
fn unusable_matrix() {
    let _h = Home::new();
    let w = world(false);
    let mark = person_agent(&w.conn, "ws-alice").unwrap().unwrap();
    let on = app_settings(&w.conn, "hr-app").unwrap();
    let ok = |i: &Item, f: &Facts, s: &AppSettings| decide::check_usable(Some(i), Some(&mark), f, s);
    assert_eq!(ok(&w.item, &alice_facts(), &on), Ok(()));
    // Gone, agent login, access lost, shared room, wallet off, dormant, needs login, missing, not their agent.
    assert_eq!(ok(&w.item, &Facts::default(), &on), Err(Unusable::PrincipalGone));
    let mut agent = alice_facts();
    agent.person.as_mut().unwrap().is_agent_login = true;
    assert_eq!(ok(&w.item, &agent, &on), Err(Unusable::NotAPerson));
    let lost = Facts { person: Some(person("p-alice", "hr-app", &["ws-elsewhere"])), others_in_room: vec![] };
    assert_eq!(ok(&w.item, &lost, &on), Err(Unusable::AccessLost));
    let other_app = Facts { person: Some(person("p-alice", "other-app", &["ws-alice"])), others_in_room: vec![] };
    assert_eq!(ok(&w.item, &other_app, &on), Err(Unusable::AccessLost), "the room must come through the marked App");
    let shared = Facts { others_in_room: vec!["p-bob".into()], ..alice_facts() };
    assert_eq!(ok(&w.item, &shared, &on), Err(Unusable::SharedRoom));
    let off = AppSettings { enabled: false, ..on.clone() };
    assert_eq!(ok(&w.item, &alice_facts(), &off), Err(Unusable::WalletOff));
    let dormant = Item { dormant_since: Some(1), ..w.item.clone() };
    assert_eq!(ok(&dormant, &alice_facts(), &on), Err(Unusable::Dormant));
    let stale = Item { state: "needs_login".into(), ..w.item.clone() };
    assert_eq!(ok(&stale, &alice_facts(), &on), Err(Unusable::NeedsLogin));
    assert_eq!(decide::check_usable(None, Some(&mark), &alice_facts(), &on), Err(Unusable::Missing));
    let bobs = PersonAgent { principal_id: "p-bob".into(), ..mark.clone() };
    assert_eq!(decide::check_usable(Some(&w.item), Some(&bobs), &alice_facts(), &on), Err(Unusable::NotTheirAgent));
}

#[test]
fn fallback_off_refuses_and_fallback_on_uses_the_company_plan() {
    let _h = Home::new();
    // Off: her sign-in expired → no spawn, with a teaching 409.
    let w = world(false);
    set_state(&w.conn, &w.item.id, "needs_login", None).unwrap();
    let c = cand(&w, "ws-alice").unwrap();
    match decide::plan(&c, &alice_facts()) {
        Plan::Refuse(why) => {
            assert_eq!(why, Unusable::NeedsLogin);
            let e = decide::refusal(&c, &alice_facts(), &why);
            assert_eq!(e.code(), "person_login_needed");
            let msg = e.to_string();
            assert!(msg.contains("Patty's personal agent"), "{msg}");
            assert!(msg.contains("signs in again"), "{msg}");
            assert!(!msg.contains(MARKER));
        }
        other => panic!("{other:?}"),
    }
    // On: the same case falls back (the existing rules: the server default).
    let w = world(true);
    set_state(&w.conn, &w.item.id, "needs_login", None).unwrap();
    let c = cand(&w, "ws-alice").unwrap();
    assert!(matches!(decide::plan(&c, &alice_facts()), Plan::Fallback(Unusable::NeedsLogin)));
    // On, and usable: still her own plan (fallback is only for when hers can't run).
    let w = world(true);
    assert!(matches!(decide::plan(&cand(&w, "ws-alice").unwrap(), &alice_facts()), Plan::Person(_)));
}

#[test]
fn a_conversation_recorded_on_a_person_item_resumes_through_the_person_layer() {
    let _h = Home::new();
    let w = world(false);
    let login = SpawnLogin {
        tool: Tool::Claude,
        account_id: Some(w.item.id.clone()),
        kind: Some("subscription".into()),
        source: pins::Source::PersonPin,
        home: Some(slot_dir(Tool::Claude, &w.item.id)),
        env: vec![],
        carry: None,
    };
    pins::record_spawn(&w.conn, &login, "ws-alice", Some("conv-1")).unwrap();
    let c = decide::candidate(&w.conn, Tool::Claude, "ws-alice", Some("ws-alice"), Some("conv-1"), true).unwrap().expect("resume candidate");
    assert!(c.resume);
    assert_eq!(c.item_id, w.item.id);
    assert!(matches!(decide::plan(&c, &alice_facts()), Plan::Person(_)));
    // Removed since: missing → refuse (fallback off).
    delete_item(&w.conn, &w.v, &w.item).unwrap();
    let c = decide::candidate(&w.conn, Tool::Claude, "ws-alice", Some("ws-alice"), Some("conv-1"), true).unwrap().unwrap();
    assert!(matches!(decide::plan(&c, &alice_facts()), Plan::Refuse(Unusable::Missing)));
    // A resume recorded on a server login never enters the person layer.
    let other = decide::candidate(&w.conn, Tool::Claude, "ws-x", Some("ws-x"), Some("conv-2"), true).unwrap();
    assert!(other.is_none());
}

#[test]
fn person_items_never_reach_the_server_pool() {
    let _h = Home::new();
    let w = world(false);
    // The server wallet lists, pool cycle and pins only ever read llm_accounts.
    assert!(crate::llm_accounts::list(&w.conn).unwrap().iter().all(|e| !is_item_id(&e.id)));
    assert!(crate::llm_accounts::next_signed_in(&w.conn, Tool::Claude).unwrap().is_none());
    let e = pins::pin(&w.conn, ScopeKind::Workspace, "ws-x", Some(Tool::Claude), &w.item.id, None).unwrap_err();
    assert_eq!(e.code(), "not_found", "a person item can't be pinned by anyone");
    let l = pins::decide_spawn(&w.conn, Tool::Claude, "tab-1", Some("ws-alice"), None, false).unwrap();
    assert!(l.account_id.is_none(), "the owner's tab runs on the server default");
}
