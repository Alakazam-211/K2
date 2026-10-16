//! Slice A tests (prd-clone-carries-threads-and-vault-v1 §10 items 10–15
//! and the vault half of 16): per-secret plan, the plan-bound confirm,
//! carry v2 by id, grants, `.env.k2` rewrite, leave-blank cards, expiry,
//! never-carried kinds, no value anywhere, audit lines both sides.
//!
//! Temp HOME, the test vault (temp dir + fake key store) and the shared
//! in-memory DB. Never the real keychain, `~/.k2`, or the prod daemon.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{
    Actor, AgentActor, Consumer, Delivery, GrantRequest, KeyStore, Kind, ListFilter, LocalVault, Namespace, OpenOptions,
    PutRequest, Requester, Role, Scope, ScopeKind, SecretBytes, SecretStore, Selector, Use,
};
use serde_json::{json, Value};

use super::*;
use crate::vault::actor::{Channel, VaultCaller};

const SENTINEL: &str = "sk_test_CACLONE_VALUE_7f6e5d4c3b2a1908";
const SERVER_SENTINEL: &str = "sk_test_CACLONE_SERVER_0a1b2c3d4e5f";

fn owner() -> VaultCaller {
    VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
}

fn login(role: Role) -> VaultCaller {
    VaultCaller { actor: Actor::Login { username: "lee".into(), role }, channel: Channel::Login, session_id: None }
}

fn passport(ws: &str) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: true }),
        channel: Channel::Plain,
        session_id: Some("s1".into()),
    }
}

fn sfx() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_ascii_uppercase()
}

/// A registered source workspace with one secret of every interesting
/// shape in `v`, a `.env.k2` naming two of them, and unrelated secrets
/// that must never be offered.
struct Src {
    _dir: TestDir,
    path: String,
    id: String,
    label: String,
    stripe: String,
    gmail: String,
    tls: String,
    openai: String,
    login: String,
}

impl Src {
    fn new(v: &LocalVault) -> Src {
        let s = sfx();
        let dir = TestDir::new("ca-clone-src");
        let ws = dir.path().join(format!("ca-sales-{}", s.to_ascii_lowercase()));
        fs::create_dir_all(&ws).expect("mkdir");
        fs::write(ws.join("README.md"), "# sales\n").expect("readme");
        let path = fs::canonicalize(&ws).expect("canon").to_string_lossy().to_string();
        let project = k2_core::projects_ops::projects_add_without_git(&path).expect("register source");
        let id = project.id.clone();
        let label = crate::vault::workspace_label(&id);
        let names = |p: &str| format!("{p}_{s}");
        let (stripe, gmail, tls, openai, login) = (names("STRIPE_KEY"), names("GMAIL_TOKEN"), names("TLS_KEY"), names("OPENAI_KEY"), names("LOGIN"));
        let put = |scope: Scope, name: &str, kind: Kind, value: &str| {
            let mut r = PutRequest::new(Namespace::User, scope, name, kind);
            r.hosts = vec!["https://api.example.com".into()];
            v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put").meta
        };
        let m = put(Scope::workspace(id.clone()), &stripe, Kind::ApiKey, SENTINEL);
        v.grant(
            GrantRequest {
                selector: Selector::Id(m.id),
                consumer: Consumer::workspace(id.clone()),
                delivery: Delivery::Env,
                env_name: Some("STRIPE_SECRET_KEY".into()),
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect("grant stripe env");
        put(Scope::workspace(id.clone()), &gmail, Kind::OauthToken, "oauth-value-ca");
        put(Scope::workspace(id.clone()), &tls, Kind::TlsKey, "tls-value-ca");
        put(Scope::workspace(id.clone()), &login, Kind::LoginSlot, "login-value-ca");
        let m = put(Scope::server(), &openai, Kind::ApiKey, SERVER_SENTINEL);
        v.grant(
            GrantRequest {
                selector: Selector::Id(m.id),
                consumer: Consumer::workspace(id.clone()),
                delivery: Delivery::Env,
                env_name: Some("OPENAI_API_KEY".into()),
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect("grant openai env");
        put(Scope::server(), &names("UNUSED"), Kind::ApiKey, "unused-ca");
        put(Scope::workspace(format!("ca-other-{s}")), &names("OTHER"), Kind::ApiKey, "other-ca");
        fs::write(
            Path::new(&path).join(".env.k2"),
            format!("STRIPE_SECRET_KEY=k2v://ws/{label}/{stripe}\nOPENAI_API_KEY=k2v://server/{openai}\nPORT=3000\n"),
        )
        .expect(".env.k2");
        Src { _dir: dir, path, id, label, stripe, gmail, tls, openai, login }
    }

    fn ask(&self, target: Option<VaultTarget>, clone_id: Option<&str>) -> CarryAsk {
        CarryAsk {
            project_path: Some(self.path.clone()),
            vault_target: target,
            clone_id: clone_id.map(str::to_string),
            ..Default::default()
        }
    }
}

impl Drop for Src {
    fn drop(&mut self) {
        let _ = k2_core::projects_ops::projects_delete(&self.id);
    }
}

fn target_of(v: &LocalVault, label: &str) -> VaultTarget {
    VaultTarget { recipient: v.carry_recipient().expect("recipient").encode(), vault_id: v.vault_id().to_string(), label: Some(label.into()) }
}

fn temp_vault(tag: &str) -> (TestDir, Arc<LocalVault>) {
    let dir = TestDir::new(tag);
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(dir.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    (dir, v)
}

fn register(tag: &str) -> (TestDir, String) {
    let dir = TestDir::new(tag);
    let ws = dir.path().join(format!("ca-dst-{}", sfx().to_ascii_lowercase()));
    fs::create_dir_all(&ws).expect("mkdir");
    let path = fs::canonicalize(&ws).expect("canon").to_string_lossy().to_string();
    let p = k2_core::projects_ops::projects_add_without_git(&path).expect("register");
    (dir, p.id)
}

fn value_in(v: &LocalVault, ws: &str, name: &str) -> Option<String> {
    v.resolve(&Selector::user(Scope::workspace(ws), name), Use::System, &Requester::System("test".into()))
        .ok()
        .map(|r| String::from_utf8(r.value.expose().to_vec()).expect("utf8"))
}

fn ws_names(v: &LocalVault, ws: &str) -> Vec<String> {
    let mut n: Vec<String> = v
        .list(&ListFilter::default(), &Actor::System("test".into()))
        .expect("list")
        .into_iter()
        .filter(|m| m.namespace == Namespace::User && m.scope.kind == ScopeKind::Workspace && m.scope.id == ws)
        .map(|m| m.name)
        .collect();
    n.sort();
    n
}

fn no_value(s: &str) {
    assert!(!s.contains(SENTINEL) && !s.contains(SERVER_SENTINEL), "a value leaked: {s}");
}

fn row<'a>(plan: &'a Plan, name: &str) -> &'a PlanRow {
    plan.rows.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}"))
}

fn route(path: &str, c: VaultCaller, body: Value) -> (u16, Value) {
    let r = handle(path, Ok(c), body.to_string().as_bytes(), "test");
    no_value(&r.body);
    let code: u16 = r.status[..3].parse().expect("status");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json: {}", r.body)))
}

fn json_of(r: &CliResponse) -> Value {
    no_value(&r.body);
    serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json: {}", r.body))
}

/// §6.1/§6.2: what is offered and the defaults; Members can't copy (D1).
#[test]
fn the_plan_offers_own_and_used_secrets_with_kind_defaults() {
    crate::test_support::with_temp_home(|| {
        let v = crate::vault::ensure_test_vault();
        let s = Src::new(&v);
        let plan = build_plan(&v, &owner(), &s.id, &s.ask(None, None)).expect("plan");
        let names: Vec<&str> = plan.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names.len(), 4, "own three + one used server secret: {names:?}");
        assert_eq!(row(&plan, &s.stripe).choice, Choice::Copy);
        assert_eq!(row(&plan, &s.stripe).env_names, vec!["STRIPE_SECRET_KEY".to_string()]);
        assert_eq!(row(&plan, &s.gmail).choice, Choice::Skip, "sign-ins skip");
        assert_eq!(row(&plan, &s.tls).choice, Choice::Blank, "box-specific keys blank");
        let o = row(&plan, &s.openai);
        assert_eq!((o.group, o.choice), ("server", Choice::Blank), "shared server secret: blank");
        assert_eq!(o.env_names, vec!["OPENAI_API_KEY".to_string()]);
        assert!(plan.not_carried.iter().any(|n| n.name == s.login && n.reason == "login"), "{:?}", plan.not_carried);
        assert!(plan.rows.iter().all(|r| !r.name.starts_with("UNUSED") && !r.name.starts_with("OTHER")));
        assert!(plan.needs_confirm());
        no_value(&serde_json::to_string(&plan).expect("json"));

        // A Member: nothing copies; asking to copy is refused.
        let m = build_plan(&v, &login(Role::Member), &s.id, &s.ask(None, None)).expect("member plan");
        assert!(m.member && m.rows.iter().all(|r| !r.copy_allowed && r.choice != Choice::Copy));
        assert!(!m.needs_confirm());
        let mut ask = s.ask(None, None);
        ask.secrets = BTreeMap::from([(s.stripe.clone(), Choice::Copy)]);
        assert_eq!(build_plan(&v, &login(Role::Member), &s.id, &ask).expect_err("member copy").code, "copy_needs_admin");
        // Unknown names are refused, not ignored.
        ask.secrets = BTreeMap::from([("NOPE".to_string(), Choice::Skip)]);
        assert_eq!(build_plan(&v, &owner(), &s.id, &ask).expect_err("unknown").code, "unknown_secret");

        // Route: names only; a passport is refused.
        let (code, p) = route(PLAN, owner(), json!({ "project_path": s.path, "vault_target": target_of(&v, "luzz"), "clone_id": "ca-c" }));
        assert_eq!(code, 200, "{p}");
        assert_eq!(p["confirm"]["action"], CONFIRM_ACTION);
        assert!(p["confirm"]["secretId"].as_str().expect("secretId").starts_with("clone:"));
        assert!(p["confirm"]["reason"].as_str().expect("reason").contains("copy 1 secret from"), "{p}");
        let (code, p) = route(PLAN, passport(&s.id), json!({ "project_path": s.path }));
        assert_eq!(code, 403, "{p}");
    });
}

/// §6.4/CL14: a copy needs a single-use confirm bound to the exact plan
/// and the person; Members and passports can't confirm (D1).
#[test]
fn a_copy_needs_a_confirm_bound_to_the_exact_plan_and_person() {
    crate::test_support::with_temp_home(|| {
        let v = crate::vault::ensure_test_vault();
        let s = Src::new(&v);
        let target = target_of(&v, "luzz.k2.dev");
        let ask = s.ask(Some(target.clone()), Some("ca-clone-1"));
        let plan = build_plan(&v, &owner(), &s.id, &ask).expect("plan");
        assert_eq!(take_confirm(&v, &owner(), &plan, &ask).expect_err("no token").code, "vault_confirm_required");
        let (sid, reason, _) = confirm_binding(&v, &owner(), &ask).expect("binding");
        assert!(reason.contains("luzz.k2.dev") && reason.contains(&s.label), "{reason}");

        // Another plan (another clone id): refused.
        let (other_sid, _, _) = confirm_binding(&v, &owner(), &s.ask(Some(target.clone()), Some("ca-clone-2"))).expect("binding");
        assert_ne!(other_sid, sid);
        let mut a = ask.clone();
        a.confirm_token = Some(v.confirm.mint(CONFIRM_ACTION, Some(&other_sid), "owner", "vault-password"));
        assert_eq!(take_confirm(&v, &owner(), &plan, &a).expect_err("other plan").code, "vault_confirm_required");
        // Other choices: another digest.
        let mut changed = ask.clone();
        changed.secrets = BTreeMap::from([(s.tls.clone(), Choice::Skip)]);
        let (changed_sid, _, _) = confirm_binding(&v, &owner(), &changed).expect("binding");
        assert_ne!(changed_sid, sid, "the digest covers every row's choice");
        // Another person's token: refused.
        a.confirm_token = Some(v.confirm.mint(CONFIRM_ACTION, Some(&sid), "connect:lee", "connect-password"));
        assert_eq!(take_confirm(&v, &owner(), &plan, &a).expect_err("other actor").code, "vault_confirm_required");
        // The right one works once.
        a.confirm_token = Some(v.confirm.mint(CONFIRM_ACTION, Some(&sid), "owner", "vault-password"));
        assert_eq!(take_confirm(&v, &owner(), &plan, &a).expect("ok").as_deref(), Some("vault-password"));
        assert_eq!(take_confirm(&v, &owner(), &plan, &a).expect_err("single use").code, "vault_confirm_required");
        // Expired tokens are refused.
        a.confirm_token = Some(v.confirm.mint(CONFIRM_ACTION, Some(&sid), "owner", "vault-password"));
        v.confirm.expire_all();
        assert_eq!(take_confirm(&v, &owner(), &plan, &a).expect_err("expired").code, "vault_confirm_required");

        // Members and passports never confirm.
        let (code, r) = route(CONFIRM, login(Role::Member), json!({ "project_path": s.path, "vault_target": target, "clone_id": "ca-clone-1", "method": "vault-password", "password": "x" }));
        assert_eq!(code, 403, "{r}");
        let (code, r) = route(CONFIRM, passport(&s.id), json!({ "project_path": s.path }));
        assert_eq!(code, 403, "{r}");

        // Through clone/bundle: no token → 403 and no carry, nothing built.
        let body = json!({ "project_path": s.path, "vault_target": target, "clone_id": "ca-clone-1" });
        let r = crate::clone_routes::handle_clone_bundle(body.to_string().as_bytes(), Some(owner()));
        assert!(r.status.starts_with("403"), "{}", r.body);
        assert_eq!(json_of(&r)["error"]["code"], "vault_confirm_required");
        let tmp = k2_core::paths::k2_home().join("clone-tmp");
        assert!(!tmp.exists() || fs::read_dir(&tmp).expect("ls").next().is_none(), "nothing was bundled");
        // A Member landing a blank-only clone needs no confirm.
        let r = crate::clone_routes::handle_clone_bundle(body.to_string().as_bytes(), Some(login(Role::Member)));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let out = json_of(&r);
        assert_eq!(out["vault_carry"]["copied"], json!([]), "{out}");
        assert!(out["vault_carry"]["blank"].as_array().expect("blank").iter().any(|n| n == &json!(s.stripe)), "{out}");
        // A passport is refused at the clone door too.
        let r = crate::clone_routes::handle_clone_bundle(body.to_string().as_bytes(), Some(passport(&s.id)));
        assert!(r.status.starts_with("403"), "{}", r.body);
    });
}

/// The whole push on one daemon: plan → confirm → bundle → unpack. The
/// copied secret lands in the NEW project by id with its env grant, the
/// `.env.k2` reference follows the clone, blank rows become cards once
/// the clone has a Thread, no value is anywhere, audit lines both sides.
#[test]
fn a_clone_copies_chosen_secrets_by_id_and_never_shows_a_value() {
    crate::test_support::with_temp_home(|| {
        let v = crate::vault::ensure_test_vault();
        let s = Src::new(&v);
        let clone_id = format!("ca-clone-{}", sfx());
        let target = target_of(&v, "this server");
        let mut choices = serde_json::Map::new();
        choices.insert(s.tls.clone(), json!("skip"));
        let ask = json!({ "project_path": s.path, "vault_target": target, "clone_id": clone_id, "secrets": choices });
        let (code, plan) = route(PLAN, owner(), ask.clone());
        assert_eq!(code, 200, "{plan}");
        let sid = plan["confirm"]["secretId"].as_str().expect("secretId").to_string();
        let mut body = ask.clone();
        body["confirm_token"] = json!(v.confirm.mint(CONFIRM_ACTION, Some(&sid), "owner", "vault-password"));
        let r = crate::clone_routes::handle_clone_bundle(body.to_string().as_bytes(), Some(owner()));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let b = json_of(&r);
        assert_eq!(b["clone_id"], json!(clone_id));
        assert_eq!(b["vault_carry"]["copied"], json!([s.stripe]), "{b}");
        assert_eq!(b["vault_carry"]["blank"], json!([s.openai]), "{b}");
        let mut skipped: Vec<String> = serde_json::from_value(b["vault_carry"]["skipped"].clone()).expect("skipped");
        skipped.sort();
        let mut want = vec![s.gmail.clone(), s.tls.clone()];
        want.sort();
        assert_eq!(skipped, want);
        let carry = b["vault_carry_b64"].as_str().expect("carry").to_string();
        // The bundle itself: no value, and the manifest names the clone.
        let bundle = PathBuf::from(b["bundle_path"].as_str().expect("bundle"));
        let mut raw = vec![];
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(fs::File::open(&bundle).expect("open")), &mut raw).expect("gunzip");
        no_value(&String::from_utf8_lossy(&raw));
        assert!(String::from_utf8_lossy(&raw).contains(&clone_id), "manifest carries clone_id");

        // Destination (same daemon here): unpack lands it by the new id.
        let dest_dir = TestDir::new("ca-clone-dest");
        let r = crate::clone_routes::handle_clone_unpack(
            json!({ "bundle_path": bundle, "dest_parent": dest_dir.path(), "vault_carry_b64": carry }).to_string().as_bytes(),
            Some(owner()),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let u = json_of(&r);
        let new_id = u["project"]["id"].as_str().expect("id").to_string();
        assert_ne!(new_id, s.id);
        assert_eq!(u["clone_id"], json!(clone_id));
        assert_eq!(u["vault_imported"], json!([s.stripe]), "{u}");
        assert_eq!(u["vault_grants"], json!(1), "{u}");
        assert_eq!(ws_names(&v, &new_id), vec![s.stripe.clone()], "only the copied secret, under the new id");
        assert_eq!(value_in(&v, &new_id, &s.stripe).as_deref(), Some(SENTINEL));
        assert_eq!(value_in(&v, &s.id, &s.stripe).as_deref(), Some(SENTINEL), "the source keeps its own");
        let grants = v.grants_to(&Consumer::workspace(new_id.clone())).expect("grants");
        assert!(grants.iter().any(|(g, n)| n == &s.stripe && g.env_name.as_deref() == Some("STRIPE_SECRET_KEY")), "{grants:?}");
        let meta = v.meta(&Selector::user(Scope::workspace(new_id.clone()), &s.stripe), &Actor::Owner).expect("meta");
        assert!(!meta.human_given, "not an own-server move: person-given marks don't travel (VS20)");

        // .env.k2 follows the clone for the copied name only.
        let dest = u["dest_path"].as_str().expect("dest").to_string();
        let env = fs::read_to_string(Path::new(&dest).join(".env.k2")).expect(".env.k2");
        let new_label = crate::vault::workspace_label(&new_id);
        assert!(env.contains(&format!("STRIPE_SECRET_KEY=k2v://ws/{new_label}/{}", s.stripe)), "{env}");
        assert!(env.contains(&format!("OPENAI_API_KEY=k2v://server/{}", s.openai)), "{env}");
        assert_eq!(u["vault_env_k2_rewritten"], json!(["STRIPE_SECRET_KEY"]));

        // No Thread yet: the blank card waits (names only on disk) …
        assert_eq!(u["vault_cards_deferred"], json!([s.openai]), "{u}");
        let kept = fs::read_to_string(deferred_path(&new_id)).expect("deferred cards");
        no_value(&kept);
        // … and is posted once the clone has its own Thread.
        let conv = format!("ca-conv-{}", sfx());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::WorkspaceSession::upsert(&conn, &uuid::Uuid::new_v4().to_string(), &new_id, None, Some(&conv), "claude", "owner", "idle")
                .expect("pin");
        }
        assert_eq!(post_deferred_cards(&new_id), vec![s.openai.clone()]);
        assert!(!deferred_path(&new_id).exists(), "posted once");
        let docs = k2_core::overlay::read_thread(&conv, 0).expect("thread");
        let cards: Vec<_> = docs.iter().filter_map(|i| i.doc.secret.as_ref()).collect();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].name, s.openai);
        assert_eq!(cards[0].status, "pending");
        no_value(&serde_json::to_string(&docs.iter().map(|i| &i.doc).collect::<Vec<_>>()).expect("docs json"));
        assert_eq!(v.pending_requests_in(&conv).expect("requests"), 1, "the vault request exists for the card");

        // Audit: both sides, names and counts only.
        let audit = fs::read_to_string(k2_core::auth_audit::path()).expect("audit");
        no_value(&audit);
        assert!(audit.contains("ok clone export 1 (blank 1)") && audit.contains(&format!("clone={clone_id}")), "{audit}");
        assert!(audit.contains(&format!("ok clone import 1 from {} into {new_id} clone={clone_id}", v.vault_id())), "{audit}");

        let _ = k2_core::projects_ops::projects_delete(&new_id);
    });
}

/// CL26: a carry opens only for its own clone and only within its hour;
/// a bad carry never fails the unpack.
#[test]
fn a_carry_for_another_clone_or_an_expired_one_is_refused() {
    crate::test_support::with_temp_home(|| {
        let (_sd, src) = temp_vault("ca-exp-src");
        let (_dd, dst) = temp_vault("ca-exp-dst");
        let s = Src::new(&src);
        let (_wd, dst_ws) = register("ca-exp");
        let target = target_of(&dst, "dst");
        let ask = s.ask(Some(target.clone()), Some("ca-clone-a"));
        let plan = build_plan(&src, &owner(), &s.id, &ask).expect("plan");
        let x = export(&src, &owner(), &plan, &target, "ca-clone-a", false, Some("vault-password")).expect("export");
        let carry = x.carry_b64.expect("sealed");
        no_value(&carry);
        no_value(&x.report.to_string());
        assert_eq!(import(&dst, &owner(), &carry, &dst_ws, Some("ca-clone-b")).expect_err("other clone").code, "carry_clone_mismatch");
        assert_eq!(import(&dst, &owner(), &carry, &dst_ws, None).expect_err("no manifest id").code, "carry_clone_mismatch");
        assert!(ws_names(&dst, &dst_ws).is_empty());
        // Copies need the confirm's method (export never seals without it).
        assert_eq!(export(&src, &owner(), &plan, &target, "ca-clone-a", false, None).expect_err("no confirm").code, "vault_confirm_required");

        // Expired.
        let rcp = k2_vault::agefmt::Recipient::parse(&target.recipient).expect("recipient");
        let old = src
            .carry_seal_clone(
                &rcp,
                dst.vault_id(),
                false,
                k2_vault::carry::CloneBinding { clone_id: "ca-clone-a".into(), source_workspace_id: s.id.clone(), source_label: s.label.clone(), ttl_secs: -1 },
                vec![],
                vec![],
            )
            .expect("seal");
        let old = base64::engine::general_purpose::STANDARD.encode(&old);
        assert_eq!(import(&dst, &owner(), &old, &dst_ws, Some("ca-clone-a")).expect_err("expired").code, "carry_expired");
        let out = import_into_project(Ok(dst.clone()), Some(&owner()), &old, &dst_ws, Some("ca-clone-a"));
        assert_eq!(out["vault_error"]["code"], "carry_expired", "{out}");

        // The right clone, in time: lands.
        let out = import(&dst, &owner(), &carry, &dst_ws, Some("ca-clone-a")).expect("import");
        assert_eq!(out["vault_imported"], json!([s.stripe]), "{out}");
        assert_eq!(value_in(&dst, &dst_ws, &s.stripe).as_deref(), Some(SENTINEL));
        // Again: kept, reported as exists.
        let again = import(&dst, &owner(), &carry, &dst_ws, Some("ca-clone-a")).expect("again");
        assert_eq!(again["vault_imported"], json!([]));
        assert!(again["vault_skipped"][0].as_str().expect("skip").contains("exists"), "{again}");
        // Passports never land carries.
        let out = import_into_project(Ok(dst.clone()), Some(&passport(&dst_ws)), &carry, &dst_ws, Some("ca-clone-a"));
        assert_eq!(out["vault_error"]["code"], "people_only");
        let _ = k2_core::projects_ops::projects_delete(&dst_ws);
    });
}

/// CL13 on import: whatever a carry holds, never-carried kinds and chat
/// secrets don't land.
#[test]
fn never_carried_kinds_in_a_crafted_carry_do_not_land() {
    crate::test_support::with_temp_home(|| {
        let (_sd, src) = temp_vault("ca-deny-src");
        let (_dd, dst) = temp_vault("ca-deny-dst");
        let (_wd, dst_ws) = register("ca-deny");
        let item = |name: &str, kind: &str, scope_kind: &str| k2_vault::carry::CarryItem {
            scope_kind: scope_kind.into(),
            scope_label: "sales".into(),
            scope_id: Some("ws-src".into()),
            name: name.into(),
            kind: kind.into(),
            hosts: vec![],
            inject: None,
            default_delivery: "fetch".into(),
            delivery_ceiling: "env".into(),
            description: String::new(),
            human_given: false,
            value_b64: k2_vault::carry::CarryItem::encode_value(SENTINEL.as_bytes()),
            source_scope: Some("workspace".into()),
            grants: vec![],
        };
        let mut p = k2_vault::carry::CarryPayload::in_process(src.vault_id(), false, "ca-deny", "ws-src", "sales", vec![], vec![]);
        p.items = vec![item("LOGIN", "login_slot", "workspace"), item("PASS", "app_pass", "workspace"), item("ENGINE", "engine_cred", "workspace"), item("CHAT", "api_key", "session")];
        p.target_vault_id = dst.vault_id().to_string();
        p.source_public_key = src.carry_public_key().expect("pk");
        p.expires_at = Some((chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339());
        let rcp = dst.carry_recipient().expect("recipient");
        let file = src.carry_seal_unchecked(&rcp, &p).expect("crafted");
        let out = import(&dst, &owner(), &base64::engine::general_purpose::STANDARD.encode(&file), &dst_ws, Some("ca-deny")).expect("import");
        assert_eq!(out["vault_imported"], json!([]), "{out}");
        assert_eq!(out["vault_skipped"].as_array().expect("skipped").len(), 4, "{out}");
        assert!(ws_names(&dst, &dst_ws).is_empty());
        no_value(&out.to_string());
        let _ = k2_core::projects_ops::projects_delete(&dst_ws);
    });
}

/// D4: `.env` files as files need the same confirm, Owner/Admin only.
#[test]
fn sending_env_files_needs_the_clone_carry_confirm() {
    crate::test_support::with_temp_home(|| {
        let v = crate::vault::ensure_test_vault();
        let s = Src::new(&v);
        fs::write(Path::new(&s.path).join(".env"), "DATABASE_URL=postgres://x\n").expect(".env");
        let body = json!({ "project_path": s.path, "carry_secrets": true, "live_only": true });
        let r = crate::clone_routes::handle_clone_bundle(body.to_string().as_bytes(), Some(owner()));
        assert!(r.status.starts_with("403"), "{}", r.body);
        assert_eq!(json_of(&r)["error"]["code"], "vault_confirm_required");
        let r = crate::clone_routes::handle_clone_bundle(body.to_string().as_bytes(), Some(login(Role::Member)));
        assert!(r.status.starts_with("403"), "{}", r.body);
        assert_eq!(json_of(&r)["error"]["code"], "copy_needs_admin");
        // The plan lists the files and offers moving them into the vault.
        let (code, p) = route(PLAN, owner(), json!({ "project_path": s.path, "send_env_files": true }));
        assert_eq!(code, 200, "{p}");
        assert!(p["envFiles"].as_array().expect("files").iter().any(|f| f == ".env"), "{p}");
        assert!(p["envFilesHint"].as_str().expect("hint").contains("k2 vault import"), "{p}");
        let sid = p["confirm"]["secretId"].as_str().expect("sid");
        assert!(p["confirm"]["reason"].as_str().expect("reason").contains(".env files"), "{p}");
        let mut b = body.clone();
        b["confirm_token"] = json!(v.confirm.mint(CONFIRM_ACTION, Some(sid), "owner", "vault-password"));
        let r = crate::clone_routes::handle_clone_bundle(b.to_string().as_bytes(), Some(owner()));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(json_of(&r)["manifest_summary"]["carry_secrets"], true);
    });
}

/// §6.5: same box — re-encrypted under the new workspace id, grants and
/// cards as for a carry; same confirm with the target = this vault.
#[test]
fn a_same_box_clone_reencrypts_under_the_new_workspace() {
    crate::test_support::with_temp_home(|| {
        let v = crate::vault::ensure_test_vault();
        let s = Src::new(&v);
        let (_wd, new_id) = register("ca-same");
        let mut ask = s.ask(Some(target_of(&v, "this server")), Some("ca-same-1"));
        ask.secrets = BTreeMap::from([(s.openai.clone(), Choice::Copy)]);
        assert_eq!(copy_same_box(&v, &owner(), &ask, &new_id).expect_err("no confirm").code, "vault_confirm_required");
        let (sid, _, _) = confirm_binding(&v, &owner(), &ask).expect("binding");
        ask.confirm_token = Some(v.confirm.mint(CONFIRM_ACTION, Some(&sid), "owner", "local-auth"));
        let out = copy_same_box(&v, &owner(), &ask, &new_id).expect("copy");
        no_value(&out.to_string());
        let mut got: Vec<String> = serde_json::from_value(out["vault_imported"].clone()).expect("imported");
        got.sort();
        let mut want = vec![s.stripe.clone(), s.openai.clone()];
        want.sort();
        assert_eq!(got, want, "{out}");
        assert_eq!(value_in(&v, &new_id, &s.openai).as_deref(), Some(SERVER_SENTINEL), "a server secret lands as the clone's own");
        assert_eq!(value_in(&v, &new_id, &s.stripe).as_deref(), Some(SENTINEL));
        let grants = v.grants_to(&Consumer::workspace(new_id.clone())).expect("grants");
        assert!(grants.iter().any(|(g, n)| n == &s.openai && g.env_name.as_deref() == Some("OPENAI_API_KEY")), "{grants:?}");
        assert_eq!(value_in(&v, &s.id, &s.stripe).as_deref(), Some(SENTINEL), "source untouched");
        let _ = k2_core::projects_ops::projects_delete(&new_id);
    });
}

static REASON: Mutex<String> = Mutex::new(String::new());

/// `/cli/vault/confirm/finish` with `clone-carry`: the daemon rebuilds the
/// plan from `clonePlan`, binds the token to its digest and writes the
/// Touch ID reason itself (CL14); the client's label is ignored.
#[test]
fn the_generic_confirm_binds_clone_carry_to_the_daemon_built_plan() {
    crate::test_support::with_temp_home(|| {
        let (_d, v) = temp_vault("ca-confirm");
        let s = Src::new(&v);
        let ask = json!({ "project_path": s.path, "vault_target": target_of(&v, "luzz.k2.dev"), "clone_id": "ca-cf" });
        let call = |c: &VaultCaller, body: Value| {
            let r = crate::vault_routes::handle_on(&v, crate::vault_routes::CONFIRM_FINISH, Ok(c.clone()), body.to_string().as_bytes(), &Default::default(), "test", None);
            (r.status[..3].parse::<u16>().expect("status"), serde_json::from_str::<Value>(&r.body).expect("json"))
        };
        let (code, r) = call(&owner(), json!({ "action": CONFIRM_ACTION, "method": "local-auth" }));
        assert_eq!((code, r["error"]["code"].as_str()), (400, Some("clone_plan_required")), "{r}");
        let (code, r) = call(&owner(), json!({ "action": CONFIRM_ACTION, "secretId": "clone:nope", "clonePlan": ask, "method": "local-auth" }));
        assert_eq!((code, r["error"]["code"].as_str()), (400, Some("clone_plan_mismatch")), "{r}");
        let (code, r) = call(&login(Role::Member), json!({ "action": CONFIRM_ACTION, "clonePlan": ask, "method": "connect-password", "password": "x" }));
        assert_eq!(code, 403, "{r}");

        crate::vault_routes::test_hooks::set_local_auth_fake(Some(|reason| {
            *REASON.lock().expect("reason") = reason.to_string();
            Ok(())
        }));
        let (code, r) = call(&owner(), json!({ "action": CONFIRM_ACTION, "clonePlan": ask, "method": "local-auth", "label": "reveal everything" }));
        crate::vault_routes::test_hooks::set_local_auth_fake(None);
        assert_eq!(code, 200, "{r}");
        let reason = REASON.lock().expect("reason").clone();
        assert!(reason.contains("copy 1 secret from") && reason.contains("luzz.k2.dev"), "{reason}");
        assert!(!reason.contains("reveal everything"), "the client's label is ignored: {reason}");
        let ask: CarryAsk = serde_json::from_value(json!({ "project_path": s.path, "vault_target": target_of(&v, "luzz.k2.dev"), "clone_id": "ca-cf", "confirm_token": r["token"] })).expect("ask");
        let plan = build_plan(&v, &owner(), &s.id, &ask).expect("plan");
        assert_eq!(r["secretId"], json!(confirm_binding(&v, &owner(), &ask).expect("binding").0));
        assert_eq!(take_confirm(&v, &owner(), &plan, &ask).expect("token works").as_deref(), Some("local-auth"));
    });
}
