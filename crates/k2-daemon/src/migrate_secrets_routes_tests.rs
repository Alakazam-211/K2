//! prd-box-move-carry-v1 S2/S5 route tests: two boxes in temp homes, each
//! with its own vault (never the shared test vault), through the route
//! entry with the callers that matter: a person at a terminal, the K2 app,
//! K2 Web over SSH without a terminal, and an agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, KeyStore, Kind, LocalVault, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::*;
use crate::vault::actor::{Channel, VaultCaller};

fn opts() -> k2_vault::OpenOptions {
    k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor }
}

/// One box: a temp K2 home, a fake key store, and its served vault.
struct TestBox {
    _t: k2_vault::fsutil::TestDir,
    home: PathBuf,
    ks: Arc<FakeKeyStore>,
    cell: Mutex<Result<Arc<LocalVault>, VaultError>>,
}

impl TestBox {
    fn new(tag: &str) -> TestBox {
        let t = k2_vault::fsutil::TestDir::new(tag);
        let home = t.path().join(".k2");
        let ks = Arc::new(FakeKeyStore::new());
        let v = LocalVault::open(crate::vault::vault_dir(&home), ks.clone() as Arc<dyn KeyStore>, opts()).expect("open");
        TestBox { _t: t, home, ks, cell: Mutex::new(Ok(Arc::new(v))) }
    }

    fn v(&self) -> Arc<LocalVault> {
        self.cell.lock().clone().expect("open vault")
    }

    fn call(&self, path: &str, c: VaultCaller, not_person: Option<&'static str>, body: Value) -> (String, Value) {
        let r = handle_in(self, &self.home, path, Ok(c), body.to_string().as_bytes(), &HashMap::new(), "test", not_person);
        let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json: {}", r.body));
        (r.status.to_string(), v)
    }

    /// What `Live` does before moving vault.db aside
    /// ([`crate::vault::release_for_replace`]).
    fn release_served(&self) -> Result<bool, VaultError> {
        let served = self.cell.lock().clone().ok();
        crate::vault::release_for_replace(served.as_deref())
    }

    /// A confirm token for `binding`, as `k2 vault confirm` would mint it.
    fn token(&self, c: &VaultCaller, binding: &str) -> String {
        self.v().confirm.mint(CONFIRM_ACTION, Some(binding), &c.actor.stamp(), "vault-password")
    }
}

impl MoveEnv for TestBox {
    fn vault(&self) -> Result<Arc<LocalVault>, VaultError> {
        self.cell.lock().clone()
    }
    fn apply(&self, home: &Path, identity: &Identity, mf: &MoveFile) -> Result<(Arc<LocalVault>, boxmove::ImportRecord), VaultError> {
        let ks = self.ks.clone() as Arc<dyn KeyStore>;
        // As the daemon's: on Windows the served vault lets go of its file.
        self.release_served()?;
        let i = boxmove::apply_move(&crate::vault::vault_dir(home), ks, identity, mf, opts())?;
        let v = Arc::new(i.vault);
        *self.cell.lock() = Ok(v.clone());
        Ok((v, i.record))
    }
    fn undo(&self, home: &Path, actor: &Actor, proof: Option<&HumanProof>) -> Result<boxmove::ImportRecord, VaultError> {
        let dir = crate::vault::vault_dir(home);
        let ks = self.ks.clone() as Arc<dyn KeyStore>;
        self.release_served()?;
        let rec = boxmove::undo_import(&dir, &ks)?;
        let r = LocalVault::open(&dir, ks, opts()).map(Arc::new);
        if let (Ok(v), Some(p)) = (&r, proof) {
            if v.in_rollback() {
                v.acknowledge_rollback(actor, p)?;
            }
        }
        *self.cell.lock() = r;
        Ok(rec)
    }
}

fn person() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Tty, session_id: None }
}

/// The owner token on TCP: the K2 app (a person once it brings a confirm).
fn app() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

/// K2 Web over SSH with no terminal: the vault socket's peer isn't a person.
const NO_TTY: Option<&str> = Some("no terminal");

fn agent() -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: "ws-ops".into(), session_id: Some("s1".into()), vault_manage: true, shell: false }),
        channel: Channel::Plain,
        session_id: Some("s1".into()),
    }
}

fn put_system(v: &LocalVault, engine: &str, name: &str, value: &str) {
    let mut r = PutRequest::new(Namespace::System, Scope::engine(engine), name, Kind::Password);
    r.default_delivery = Some(k2_vault::Delivery::None);
    v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::System("t".into())).expect("put");
}

fn put_user(v: &LocalVault, name: &str) {
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, Kind::ApiKey);
    r.hosts = vec!["api.github.com".into()];
    v.put(r, SecretBytes::from_slice(b"ghp_value_for_tests_1"), &Actor::Owner).expect("put user");
}

fn code(v: &Value) -> &str {
    v.pointer("/error/code").and_then(Value::as_str).unwrap_or("")
}

#[test]
fn a_box_move_end_to_end_through_the_routes_with_held_steps_and_confirms() {
    let old = TestBox::new("mv-old");
    let new = TestBox::new("mv-new");
    put_system(&old.v(), "frpc", "tunnel.token", "k2tok_from_the_old_box");
    put_user(&old.v(), "GH_TOKEN");

    // New box: K2 Web makes the move key (no person needed; idempotent).
    let (s, b1) = new.call(BEGIN, app(), NO_TTY, json!({}));
    assert_eq!(s, "200 OK", "{b1}");
    let recipient = b1["moveKey"]["recipient"].as_str().expect("recipient").to_string();
    let fp = b1["moveKey"]["fingerprint"].as_str().expect("fp").to_string();
    let (_, b2) = new.call(BEGIN, app(), NO_TTY, json!({}));
    assert_eq!((b2["created"].clone(), b2["moveKey"]["recipient"].clone()), (json!(false), json!(recipient)), "same key until it expires");

    // Old box: K2 Web runs the export → held for the owner, nothing frozen.
    let out = old.home.join("move.k2mv");
    let (s, held) = old.call(EXPORT, app(), NO_TTY, json!({ "to": recipient, "out": out }));
    assert_eq!(s, "202 Accepted", "{held}");
    assert_eq!(held["status"], "held");
    let mv = held["request"]["id"].as_str().expect("mv id").to_string();
    assert!(valid_held_id(&mv), "{mv}");
    assert!(!old.v().is_frozen() && !out.exists());
    // An agent's ask is held the same way; it can't approve.
    let (s, h2) = old.call(EXPORT, agent(), None, json!({ "to": recipient, "out": old.home.join("other.k2mv") }));
    assert_eq!((s.as_str(), h2["status"].as_str()), ("202 Accepted", Some("held")));
    let (s, a) = old.call(APPROVE, agent(), None, json!({ "request": mv }));
    assert_eq!((s.as_str(), code(&a)), ("403 Forbidden", "person_required"), "{a}");
    // Nor can the K2 Web shell without a terminal.
    let (_, a) = old.call(APPROVE, app(), NO_TTY, json!({ "request": mv }));
    assert_eq!(code(&a), "person_required");
    // The owner at a terminal: the confirm is asked, bound to the held id.
    let (s, a) = old.call(APPROVE, person(), None, json!({ "request": mv }));
    assert_eq!((s.as_str(), code(&a)), ("403 Forbidden", "vault_confirm_required"), "{a}");
    assert_eq!(a["confirm"]["secretId"], json!(mv));
    // A confirm for another step doesn't do.
    let wrong = old.token(&person(), &format!("export:{fp}"));
    let (_, a) = old.call(APPROVE, person(), None, json!({ "request": mv, "confirmToken": wrong }));
    assert_ne!(code(&a), "", "a token bound to another step is refused: {a}");
    let t = old.token(&person(), &mv);
    let (s, done) = old.call(APPROVE, person(), None, json!({ "request": mv, "confirmToken": t }));
    assert_eq!(s, "200 OK", "{done}");
    assert_eq!((done["frozen"].clone(), done["moveKey"].clone()), (json!(true), json!(fp)));
    assert!(out.exists() && old.v().is_frozen());
    // Approving twice is refused; status shows it done.
    let t = old.token(&person(), &mv);
    let (_, again) = old.call(APPROVE, person(), None, json!({ "request": mv, "confirmToken": t }));
    assert_eq!(code(&again), "already_decided");
    let (_, st) = old.call(STATUS, app(), NO_TTY, json!({}));
    let row = st["held"].as_array().expect("held").iter().find(|h| h["id"] == json!(mv)).cloned().expect("row");
    assert_eq!(row["status"], "done");
    assert_eq!(st["frozen"]["moveKey"], json!(fp));

    // The frozen old box refuses writes (423) and still reads.
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "LATE", Kind::ApiKey);
    r.hosts = vec!["api.github.com".into()];
    let e = old.v().put(r, SecretBytes::from_slice(b"late_value_12345"), &Actor::Owner).expect_err("frozen");
    assert_eq!((e.code, crate::vault_routes::refuse(&e).status), ("vault_moved_out", "423 Locked"));

    // New box: anyone may dry-run; it changes nothing.
    let new_id_before = new.v().vault_id().to_string();
    let (s, dry) = new.call(IMPORT, agent(), None, json!({ "file": out }));
    assert_eq!(s, "200 OK", "{dry}");
    assert_eq!((dry["dryRun"].clone(), dry["tunnelToken"].clone()), (json!(true), json!("present")));
    assert_eq!(dry["vaultId"], json!(old.v().vault_id()));
    assert_eq!(new.v().vault_id(), new_id_before);
    // The apply as a person at a terminal: this box's vault is empty, so a
    // person's channel is the check (as `k2 vault restore`).
    let (s, imp) = new.call(IMPORT, person(), None, json!({ "file": out, "apply": true }));
    assert_eq!(s, "200 OK", "{imp}");
    assert_eq!(imp["tunnelToken"], "present");
    assert_eq!(imp["rollback"], json!(false));
    assert_eq!(new.v().vault_id(), old.v().vault_id());
    assert!(!new.v().wrap_holders().iter().any(|h| h == boxmove::MOVE_HOLDER));
    assert!(new.v().has_system_secret("frpc", "tunnel.token"));
    // The move key is gone after one import.
    let (_, st) = new.call(STATUS, app(), None, json!({}));
    assert_eq!(st["moveKey"], Value::Null);
    assert!(st["lastImport"]["aside"].as_str().is_some_and(|a| a.starts_with("replaced-")));

    // Undo (before the cut-over): the vault is open now, so the confirm.
    let (_, u) = new.call(UNDO, person(), None, json!({}));
    assert_eq!(code(&u), "vault_confirm_required");
    let t = new.token(&person(), "undo");
    let (s, u) = new.call(UNDO, person(), None, json!({ "confirmToken": t }));
    assert_eq!(s, "200 OK", "{u}");
    assert_eq!(new.v().vault_id(), new_id_before, "this box's own vault is back");
    assert!(!new.v().in_rollback());

    // Old box: the move is called off; unfreeze takes the confirm too.
    let (_, h) = old.call(UNFREEZE, app(), NO_TTY, json!({}));
    assert_eq!(h["status"], "held");
    let t = old.token(&person(), "unfreeze");
    let (s, u) = old.call(UNFREEZE, person(), None, json!({ "confirmToken": t }));
    assert_eq!(s, "200 OK", "{u}");
    assert!(!old.v().is_frozen());
    put_user(&old.v(), "AFTER_UNFREEZE");
    let (_, u) = old.call(UNFREEZE, person(), None, json!({}));
    assert_eq!(code(&u), "not_frozen");
}

#[test]
fn the_app_imports_with_a_confirm_and_a_full_vault_needs_one() {
    let old = TestBox::new("mv-old-2");
    let new = TestBox::new("mv-new-2");
    put_user(&old.v(), "GH_TOKEN");
    put_user(&new.v(), "ALREADY_HERE");
    let (_, b) = new.call(BEGIN, person(), None, json!({}));
    let recipient = b["moveKey"]["recipient"].as_str().expect("r").to_string();
    let fp = b["moveKey"]["fingerprint"].as_str().expect("fp").to_string();
    let out = old.home.join("m.k2mv");
    let t = old.token(&person(), &format!("export:{fp}"));
    let (s, e) = old.call(EXPORT, person(), None, json!({ "to": recipient, "out": out, "confirmToken": t }));
    assert_eq!(s, "200 OK", "{e}");
    // An existing output file is never overwritten.
    let (_, e2) = old.call(EXPORT, person(), None, json!({ "to": recipient, "out": out }));
    assert_eq!(code(&e2), "out_exists");
    // This box's vault holds a secret: the person needs the confirm, and
    // the app (owner token) may import with it.
    let (_, a) = new.call(IMPORT, person(), None, json!({ "file": out, "apply": true }));
    assert_eq!(code(&a), "vault_confirm_required");
    let binding = a["confirm"]["secretId"].as_str().expect("binding").to_string();
    assert_eq!(binding, format!("import:{fp}"));
    let t = new.token(&app(), &binding);
    let (s, a) = new.call(IMPORT, app(), None, json!({ "file": out, "apply": true, "confirmToken": t }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(new.v().user_secret_count(), 1);
}

#[test]
fn import_works_while_this_box_vault_is_closed_and_refuses_bad_input() {
    let old = TestBox::new("mv-old-3");
    let new = TestBox::new("mv-new-3");
    put_system(&old.v(), "frpc", "tunnel.token", "tok");
    // The new box booted on a /home copy without its key.
    *new.cell.lock() = Err(VaultError::new("vault_key_missing", 500, "no key"));
    let (_, b) = new.call(BEGIN, app(), None, json!({}));
    let recipient = b["moveKey"]["recipient"].as_str().expect("r").to_string();
    let fp = b["moveKey"]["fingerprint"].as_str().expect("fp").to_string();
    let out = old.home.join("c.k2mv");
    let t = old.token(&person(), &format!("export:{fp}"));
    let (s, _) = old.call(EXPORT, person(), None, json!({ "to": recipient, "out": out, "confirmToken": t }));
    assert_eq!(s, "200 OK");
    // Relative paths and a missing file are refused by name.
    let (_, r) = new.call(IMPORT, person(), None, json!({ "file": "c.k2mv", "apply": true }));
    assert_eq!(code(&r), "path_not_absolute");
    // An absolute path on every OS (a bare /x has no drive on Windows).
    let missing = new.home.join("nonexistent").join("x.k2mv");
    let (_, r) = new.call(IMPORT, person(), None, json!({ "file": missing }));
    assert_eq!(code(&r), "not_found");
    // Closed vault: no confirm to take; K2 Web is held, a person imports.
    let (s, h) = new.call(IMPORT, app(), NO_TTY, json!({ "file": out, "apply": true }));
    assert_eq!((s.as_str(), h["status"].as_str()), ("202 Accepted", Some("held")));
    let (s, a) = new.call(IMPORT, person(), None, json!({ "file": out, "apply": true }));
    assert_eq!(s, "200 OK", "{a}");
    assert!(new.v().has_system_secret("frpc", "tunnel.token"));
    // Its move key is spent: a second import needs a new begin.
    let (_, r) = new.call(IMPORT, person(), None, json!({ "file": out }));
    assert_eq!(code(&r), "no_move_key");
}

#[test]
fn the_routes_are_post_only_classified_and_never_take_keys_in_a_url() {
    for p in POST_ROUTES {
        assert!(crate::vault_routes::is_post_route(p), "{p}");
        let row = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no route_policy row"));
        assert!(row.post.is_some(), "{p}");
    }
    let row = crate::routes::route_policy::lookup(STATUS).expect("status row");
    assert!(row.get.is_some() && row.post.is_none());
    assert!(!crate::vault_routes::is_post_route(STATUS));
    let b = TestBox::new("mv-url");
    let mut params = HashMap::new();
    params.insert("identity".to_string(), "AGE-SECRET-KEY-1X".to_string());
    let r = handle_in(&b, &b.home, BEGIN, Ok(person()), b"{}", &params, "t", None);
    assert!(r.body.contains("value_on_argv"), "{}", r.body);
}
