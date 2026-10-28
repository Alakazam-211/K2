//! Email Notifications end to end against fakes: the fence, senders, the
//! send pipeline through every door, budgets, idempotency, held sends,
//! relays and the invite swap. No test reaches a mail server, a relay, a
//! resolver or the real vault: every seam is a fake installed for this
//! test's thread ([`engine`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_core::connect_users::Role;
use k2_core::email_notify as store;
use k2_core::skin;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::engine::{self, CredError, HostedMail, MailEngine, RelayCreds, RelayTransport};
use super::*;
use crate::compute_routes::Who;
use crate::mail::dns_verify::{DnsError, DnsResolver, MxHost, SrvAnswer};

// ── fakes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub account: String,
    pub from: String,
    pub rcpt: Vec<String>,
    pub email: J,
    pub params: J,
    pub destroy: bool,
}

#[derive(Default)]
pub(crate) struct FakeEngine {
    pub created: Mutex<Vec<(String, String)>>,
    pub retired: Mutex<Vec<String>>,
    pub taken: Mutex<Vec<String>>,
    pub sent: Mutex<Vec<Sent>>,
    pub swept: Mutex<Vec<(String, i64)>>,
    pub fail_submit: Mutex<Option<String>>,
    /// N6: reports waiting per account; destroyed ids.
    pub reports: Mutex<HashMap<String, Vec<engine::RawReport>>>,
    pub destroyed: Mutex<Vec<String>>,
    /// EN-A27: messages still queued (registry id → envelope id) and the
    /// lookups made.
    pub queued: Mutex<HashMap<String, String>>,
    pub queue_lookups: Mutex<Vec<String>>,
}

impl MailEngine for FakeEngine {
    fn create_sender_account(&self, local: &str, d: &str) -> Result<String, String> {
        let mut c = self.created.lock().unwrap();
        c.push((local.to_string(), d.to_string()));
        Ok(format!("acct{}", c.len()))
    }
    fn retire_account(&self, id: &str) -> Result<(), String> {
        self.retired.lock().unwrap().push(id.to_string());
        Ok(())
    }
    fn address_taken(&self, address: &str, _d: &str) -> Result<bool, String> {
        Ok(self.taken.lock().unwrap().iter().any(|t| t == address))
    }
    fn submit(&self, m: &HostedMail<'_>) -> Result<(), String> {
        if let Some(e) = self.fail_submit.lock().unwrap().clone() {
            return Err(e);
        }
        self.sent.lock().unwrap().push(Sent {
            account: m.account_id.to_string(),
            from: m.from.to_string(),
            rcpt: m.rcpt.to_vec(),
            email: m.email.clone(),
            params: m.params.clone(),
            destroy: m.destroy_copy,
        });
        Ok(())
    }
    fn sweep_account(&self, id: &str, before: i64) -> Result<(u64, u64), String> {
        self.swept.lock().unwrap().push((id.to_string(), before));
        Ok((3, 2))
    }
    fn fetch_reports(&self, account_id: &str, limit: usize) -> Result<Vec<engine::RawReport>, String> {
        let all = self.reports.lock().unwrap().get(account_id).cloned().unwrap_or_default();
        Ok(all.into_iter().take(limit).collect())
    }
    fn destroy_emails(&self, account_id: &str, ids: &[String]) -> Result<(), String> {
        if let Some(list) = self.reports.lock().unwrap().get_mut(account_id) {
            list.retain(|r| !ids.contains(&r.email_id));
        }
        self.destroyed.lock().unwrap().extend(ids.iter().cloned());
        Ok(())
    }
    fn queued_env_id(&self, registry_id: &str) -> Result<Option<String>, String> {
        self.queue_lookups.lock().unwrap().push(registry_id.to_string());
        Ok(self.queued.lock().unwrap().get(registry_id).cloned())
    }
}

#[derive(Default)]
struct FakeDns {
    txt: Mutex<HashMap<String, Vec<String>>>,
}

impl FakeDns {
    fn set(&self, name: &str, v: &str) {
        self.txt.lock().unwrap().insert(name.to_string(), vec![v.to_string()]);
    }
}

impl DnsResolver for FakeDns {
    fn mx(&self, _: &str) -> Result<Vec<MxHost>, DnsError> {
        Err(DnsError::NotFound)
    }
    fn txt(&self, name: &str) -> Result<Vec<Vec<String>>, DnsError> {
        match self.txt.lock().unwrap().get(name) {
            Some(v) => Ok(v.iter().map(|s| vec![s.clone()]).collect()),
            None => Err(DnsError::NotFound),
        }
    }
    fn a(&self, _: &str) -> Result<Vec<std::net::Ipv4Addr>, DnsError> {
        Err(DnsError::NotFound)
    }
    fn ptr(&self, _: std::net::IpAddr) -> Result<Vec<String>, DnsError> {
        Err(DnsError::NotFound)
    }
    fn srv(&self, _: &str) -> Result<Vec<SrvAnswer>, DnsError> {
        Err(DnsError::NotFound)
    }
}

#[derive(Default)]
struct MemCreds {
    closed: bool,
    map: Mutex<HashMap<String, String>>,
}

impl RelayCreds for MemCreds {
    fn put(&self, id: &str, p: &[u8]) -> Result<(), CredError> {
        if self.closed {
            return Err(CredError::VaultClosed);
        }
        self.map.lock().unwrap().insert(id.to_string(), String::from_utf8_lossy(p).to_string());
        Ok(())
    }
    fn get(&self, id: &str) -> Result<Option<String>, CredError> {
        if self.closed {
            return Err(CredError::VaultClosed);
        }
        Ok(self.map.lock().unwrap().get(id).cloned())
    }
    fn destroy(&self, id: &str) -> Result<(), CredError> {
        if self.closed {
            return Err(CredError::VaultClosed);
        }
        self.map.lock().unwrap().remove(id);
        Ok(())
    }
}

#[derive(Default)]
struct FakeRelay {
    sent: Mutex<Vec<(String, Vec<String>, Vec<u8>)>>,
    fail_with_password: bool,
}

impl RelayTransport for FakeRelay {
    fn submit(&self, _relay: &store::Relay, password: &str, from: &str, rcpt: &[String], raw: &[u8]) -> Result<(), String> {
        if self.fail_with_password {
            return Err(format!("535 auth failed for secret {password}"));
        }
        self.sent.lock().unwrap().push((from.to_string(), rcpt.to_vec(), raw.to_vec()));
        Ok(())
    }
}

/// The real lettre wiring over a plaintext loopback (the mock below).
struct PlainRelay;

impl RelayTransport for PlainRelay {
    fn submit(&self, relay: &store::Relay, password: &str, from: &str, rcpt: &[String], raw: &[u8]) -> Result<(), String> {
        crate::mail::external_smtp::submit_plaintext_for_test(&relay.host, relay.port, &relay.username, password, from, rcpt, raw)
    }
}

// ── the fixture ──────────────────────────────────────────────────────

pub(crate) const DOMAIN: &str = "notify.brand.test";

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws: String,
    other_ws: String,
    app: String,
    other_app: String,
    engine: Arc<FakeEngine>,
    dns: Arc<FakeDns>,
    creds: Arc<MemCreds>,
    relay: Arc<FakeRelay>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        engine::set_test_engine(None);
        engine::set_test_dns(None);
        engine::set_test_creds(None);
        engine::set_test_relay(None);
        engine::set_test_hosted_supported(true);
        crate::push_routes::set_test_airgap(false);
    }
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    crate::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let (ws, other_ws) = (format!("ws-{tag}"), format!("ot-{tag}"));
    let (app, other_app) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for r in [&ws, &other_ws] {
            let path = home.path().join(r.as_str());
            std::fs::create_dir_all(&path).expect("dir");
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, path.to_string_lossy()]).expect("project");
        }
        let a = k2_core::published_services::insert(&conn, &ws, "shop", "node s.js", "/tmp", 4350, "local", "running", "cmd", "").expect("svc");
        let o = k2_core::published_services::insert(&conn, &other_ws, "crm", "node s.js", "/tmp", 4351, "local", "running", "cmd", "").expect("svc");
        let now = super::now();
        conn.execute(
            "INSERT INTO mail_domains (id, domain, stalwart_domain_id, send_mode, status, verified_at, created_at) \
             VALUES ('dom1', ?1, 'stw-notify', 'direct', 'verified', ?2, ?2)",
            params![DOMAIN, now - 30 * 86_400],
        )
        .expect("domain");
        (a.id, o.id)
    };
    let engine_fake = Arc::new(FakeEngine::default());
    let dns = Arc::new(FakeDns::default());
    dns.set("_dmarc.brand.test", "v=DMARC1; p=reject; rua=mailto:dmarc@brand.test");
    let creds = Arc::new(MemCreds::default());
    let relay = Arc::new(FakeRelay::default());
    engine::set_test_engine(Some(engine_fake.clone()));
    engine::set_test_dns(Some(dns.clone()));
    engine::set_test_creds(Some(creds.clone()));
    engine::set_test_relay(Some(relay.clone()));
    engine::set_test_hosted_supported(true);
    crate::push_routes::set_test_airgap(false);
    Fx { _db: db, _home: home, ws, other_ws, app, other_app, engine: engine_fake, dns, creds, relay }
}

fn add_hosted(addr: &str) -> store::Sender {
    let v = senders::add(&senders::AddArgs { address: addr.to_string(), name: Some("Brand".into()), ..Default::default() }, "owner", now()).expect("sender add");
    let id = v["sender"]["id"].as_str().expect("id").to_string();
    senders::find_live(&id).expect("live")
}

fn grant(app: &str, s: &store::Sender, scope: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::set_app_on(&conn, app, &s.id, scope, "owner", now()).expect("on");
}

fn proven_user(app: &str, user: &str, email: &str) -> String {
    let p = skin::add_principal_for_app(user, Some(email), app, None, "test").expect("user");
    skin::invites::mark_email_proven(&p.id, email).expect("proven");
    p.id
}

fn app_caller(app: &str) -> send::Caller {
    send::Caller::App { app: app.to_string(), actor: format!("app:{app}") }
}

fn code(r: &R<J>) -> &'static str {
    r.as_ref().expect_err("refused").code
}

fn body_of(r: &crate::cli_response::CliResponse) -> J {
    serde_json::from_str(&r.body).expect("json body")
}

fn status_of(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().expect("status").parse().expect("number")
}

// ── N1: senders and the fence ────────────────────────────────────────

#[test]
fn a_hosted_sender_is_an_account_with_no_mailbox_row() {
    let f = fx("n1add");
    let s = add_hosted("noreply@notify.brand.test");
    assert_eq!(f.engine.created.lock().unwrap().clone(), vec![("noreply".to_string(), "stw-notify".to_string())]);
    assert_eq!(s.state, store::STATE_READY, "DMARC p=reject on the parent passes (E10)");
    assert_eq!(s.path, store::PATH_HOSTED);
    assert!(s.stalwart_account_id.is_some());
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(crate::mail::addresses::load_address(&conn, &s.address).is_none(), "E1: never a mail_addresses row");
    drop(conn);
    // E3: the account object itself has no credential (jmap::notify_tests
    // pins the exact JSON); E1: every hosted-mail lookup is not_found.
    assert!(matches!(crate::mail::hosted::load_active_hosted(&s.address), Err(crate::mail::addresses::AddrError::NotFound(_))));
    let r = crate::mail::hostmail_auth::authorize_mailbox(&s.address).expect_err("not a mailbox");
    assert_eq!(status_of(&r), 404, "{}", r.body);
    assert!(crate::mail::access::can_read(&f.ws, &s.address).is_err(), "no workspace can read it");
}

#[test]
fn one_owner_per_address_and_never_a_mailbox() {
    let f = fx("n1dup");
    let s = add_hosted("noreply@notify.brand.test");
    let again = senders::add(&senders::AddArgs { address: "NoReply@notify.brand.test".into(), ..Default::default() }, "owner", now());
    assert_eq!(code(&again), "exists");
    // An address a mailbox holds (any status).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) \
             VALUES ('ma1', 'inbox@notify.brand.test', 'dom1', 'acctX', ?1, 'active', 1)",
            params![f.ws],
        )
        .expect("mailbox");
    }
    let m = senders::add(&senders::AddArgs { address: "inbox@notify.brand.test".into(), ..Default::default() }, "owner", now());
    assert_eq!(code(&m), "exists");
    // Any Stalwart principal, alias, list or catch-all (E2).
    f.engine.taken.lock().unwrap().push("alias@notify.brand.test".into());
    let a = senders::add(&senders::AddArgs { address: "alias@notify.brand.test".into(), ..Default::default() }, "owner", now());
    assert_eq!(code(&a), "exists");
    // A +tag never.
    assert_eq!(code(&senders::add(&senders::AddArgs { address: "no+x@notify.brand.test".into(), ..Default::default() }, "owner", now())), "usage");
    // A domain that isn't hosted and Verified.
    let d = senders::add(&senders::AddArgs { address: "noreply@elsewhere.test".into(), ..Default::default() }, "owner", now());
    assert_eq!(code(&d), "domain_not_ready");
    // Removing it retires the account and keeps the address reserved (E26).
    senders::remove(&s.id, "owner", now()).expect("rm");
    assert_eq!(f.engine.retired.lock().unwrap().clone(), vec![s.stalwart_account_id.clone().expect("acct")]);
    let r = senders::add(&senders::AddArgs { address: s.address.clone(), ..Default::default() }, "owner", now());
    assert_eq!(code(&r), "reserved");
    assert!(fence::reserved(&s.address), "still reserved for hosted-mail verbs");
}

#[test]
fn the_system_sender_is_one_and_never_an_apps() {
    let f = fx("n1sys");
    let a = add_hosted("noreply@notify.brand.test");
    let b = add_hosted("hello@notify.brand.test");
    senders::set_system(&a.id, "owner", now()).expect("system");
    senders::set_system(&b.id, "owner", now()).expect("the system sender moves");
    assert_eq!(senders::system().expect("read").expect("set").id, b.id);
    assert!(!senders::find_live(&a.id).expect("a").is_system(), "the old one is nobody's");
    // An App's sender can't be the system sender; the system sender can't be an App's (E20).
    grant(&f.app, &a, store::SCOPE_USERS);
    let e = senders::set_system(&a.id, "owner", now()).expect_err("an App's");
    assert_eq!(e.code, "sender_in_use");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(store::set_app_on(&conn, &f.other_app, &b.id, store::SCOPE_USERS, "owner", now()).is_err(), "the system sender is never granted");
    assert!(store::set_app_on(&conn, &f.other_app, &a.id, store::SCOPE_USERS, "owner", now()).is_err(), "another App's sender");
}

#[test]
fn hosted_mail_verbs_refuse_to_claim_a_senders_address() {
    let f = fx("n1fence");
    let s = add_hosted("noreply@notify.brand.test");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) \
             VALUES ('ma2', 'team@notify.brand.test', 'dom1', 'acctT', ?1, 'active', 1)",
            params![f.ws],
        )
        .expect("mailbox");
    }
    // alias add onto a mailbox
    let r = crate::mail::alias::handle_alias_add(json!({"mailbox": "team@notify.brand.test", "alias": s.address}).to_string().as_bytes());
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (409, json!("reserved")), "{}", r.body);
    // list create
    let r = crate::mail::lists::handle_list_create(json!({"address": s.address, "members": []}).to_string().as_bytes());
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (409, json!("reserved")), "{}", r.body);
    // catch-all set
    let r = crate::mail::catchall::handle_catchall_post(json!({"domain": DOMAIN, "address": s.address}).to_string().as_bytes());
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (409, json!("reserved")), "{}", r.body);
    // mint
    let _mail = crate::mail::mail_server_test_lock();
    let fake = crate::mail::addresses::tests::FakeAddrEngine::ok();
    let secrets = crate::mail::secrets::FileSecretStore::at(f._home.path().join("s.json"));
    let e = crate::mail::addresses::mint_address(&fake, &secrets, &f.ws, 0, "noreply", Some(DOMAIN), None).expect_err("reserved");
    assert!(matches!(e, crate::mail::addresses::AddrError::Reserved(_)), "{e:?}");
    assert!(fake.created.lock().unwrap().is_empty(), "nothing reached the mail server");
    // Group create uses the same check.
    assert!(fence::reserved(&s.address) && !fence::reserved("team@notify.brand.test"));
}

fn as_it_agent<T>(f: &Fx, run: impl FnOnce() -> T) -> T {
    let path: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT path FROM projects WHERE id = ?1", params![f.other_ws], |r| r.get(0)).expect("path")
    };
    k2_core::workspace::settings::set_mail_manage_enabled(&path, true).expect("mail manage");
    let p = crate::session_token::HookPrincipal { workspace_uuid: f.other_ws.clone(), agent_address: "it".into() };
    crate::caller_workspace::with_request_principal(Some(p), run)
}

#[test]
fn an_it_agent_cant_stop_k2s_own_mail() {
    let f = fx("n1integ");
    let s = add_hosted("noreply@notify.brand.test");
    // E4 (Rosson R1): the owner passes; a mail-manage agent is refused on
    // a domain carrying a sender (send mode, DKIM retire), and stopping the
    // mail server while a hosted sender exists.
    assert!(fence::domain_gate(DOMAIN, "change the send mode").is_ok(), "the owner");
    let r = as_it_agent(&f, || fence::domain_gate(DOMAIN, "change the send mode")).expect_err("fenced");
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (403, json!("owner_only")));
    assert!(as_it_agent(&f, || fence::domain_gate("other.test", "change the send mode")).is_ok(), "other domains are unchanged");
    assert!(as_it_agent(&f, || fence::server_gate("stop")).is_err());
    let r = as_it_agent(&f, || crate::mail::routes_server::handle_server_disable(b"{}"));
    assert_eq!(status_of(&r), 403, "{}", r.body);
    let r = as_it_agent(&f, || crate::mail::dkim::handle_dkim_retire(json!({"domain": DOMAIN, "selector": "s1"}).to_string().as_bytes()));
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (403, json!("owner_only")), "{}", r.body);
    // The queue: a notification is recognised by its return path.
    assert!(fence::is_notification(Some(&format!("<{}>", s.address))));
    assert!(!fence::is_notification(Some("person@notify.brand.test")));
    assert_eq!(fence::count_recipients(&json!({"a@x.test": {}, "b@x.test": {}})), 2);
}

#[test]
fn the_sweep_empties_sender_mailboxes_only() {
    let f = fx("n1sweep");
    let s = add_hosted("noreply@notify.brand.test");
    let now = now();
    let (gone, _) = sweep::hourly(now);
    assert_eq!(gone, 3);
    assert_eq!(f.engine.swept.lock().unwrap().clone(), vec![(s.stalwart_account_id.clone().expect("acct"), now - sweep::MAILBOX_KEEP_SECS)]);
    assert_eq!(senders::find_live(&s.id).expect("s").bounce_hints, 2, "DSNs counted as a bounce hint (E5)");
}

#[test]
fn notifications_routes_are_the_owners_and_admins() {
    let f = fx("n1routes");
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let shell = Who::Shell { project_id: f.ws.clone(), session_id: "s".into() };
    for who in [agent, shell, Who::Login { username: "m".into(), role: Role::Member }] {
        let r = owner_routes::handle(owner_routes::STATUS, who, false, b"", &HashMap::new());
        assert_eq!(status_of(&r), 403, "{}", r.body);
    }
    assert!(!crate::session_token::is_agent_verb(owner_routes::SENDER_ADD), "never an agent verb");
    // GET on a mutation → 405.
    let r = owner_routes::handle(owner_routes::SENDER_ADD, Who::Owner, false, b"", &HashMap::new());
    assert_eq!(status_of(&r), 405);
    // The owner token alone can't add a sender: a fresh confirm, bound to the address.
    let body = json!({"address": "noreply@notify.brand.test"}).to_string();
    let r = owner_routes::handle(owner_routes::SENDER_ADD, Who::Owner, true, body.as_bytes(), &HashMap::new());
    let v = body_of(&r);
    assert_eq!((status_of(&r), v["error"]["code"].clone(), v["error"]["action"].clone()), (403, json!("vault_confirm_required"), json!(CONFIRM_SENDER)), "{v}");
    assert_eq!(v["error"]["secretId"], "noreply@notify.brand.test");
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    let r = owner_routes::handle(owner_routes::SENDER_ADD, admin.clone(), true, body.as_bytes(), &HashMap::new());
    assert_eq!(body_of(&r)["error"]["code"], "owner_only", "an Admin reads and narrows, never adds");
    let vault = crate::vault::ensure_test_vault();
    let wrong = vault.confirm.mint(CONFIRM_SENDER, Some("other@notify.brand.test"), "owner", "local-auth");
    let b = json!({"address": "noreply@notify.brand.test", "confirmToken": wrong}).to_string();
    assert_eq!(status_of(&owner_routes::handle(owner_routes::SENDER_ADD, Who::Owner, true, b.as_bytes(), &HashMap::new())), 403);
    let tok = vault.confirm.mint(CONFIRM_SENDER, Some("noreply@notify.brand.test"), "owner", "local-auth");
    let b = json!({"address": "noreply@notify.brand.test", "confirmToken": tok}).to_string();
    let r = owner_routes::handle(owner_routes::SENDER_ADD, Who::Owner, true, b.as_bytes(), &HashMap::new());
    assert_eq!(status_of(&r), 200, "{}", r.body);
    let r = owner_routes::handle(owner_routes::STATUS, admin, false, b"", &HashMap::new());
    let v = body_of(&r);
    assert_eq!(v["senders"].as_array().expect("senders").len(), 1);
    assert!(v["agpl"].as_str().expect("agpl").contains("Alakazam-211/stalwart"), "E23 source pointer");
    assert!(!r.body.contains("acct1"), "never the Stalwart account id: {}", r.body);
}

// ── N2: the pipeline ─────────────────────────────────────────────────

fn on_app(f: &Fx, scope: &str) -> store::Sender {
    let s = add_hosted("noreply@notify.brand.test");
    grant(&f.app, &s, scope);
    senders::find_live(&s.id).expect("live")
}

#[test]
fn a_send_writes_its_row_then_hands_off_html_text_reply_to_and_headers() {
    let f = fx("n2send");
    let s = on_app(&f, store::SCOPE_USERS);
    proven_user(&f.app, "dana", "dana@x.test");
    let v = json!({"to": "dana@x.test", "subject": "Order k2inv_abc shipped", "text": "plain BODYTOKEN", "html": "<p>BODYTOKEN</p>",
        "replyTo": "help@brand.test", "headers": {"X-App-Order": "42"}, "tags": ["orders"]});
    let out = send::send(&app_caller(&f.app), &v, now()).expect("sent");
    assert_eq!((out["status"].clone(), out["recipients"].clone(), out["accepted"].clone()), (json!("submitted"), json!(1), json!(true)));
    let sent = f.engine.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let m = &sent[0];
    assert_eq!((m.account.as_str(), m.from.as_str()), (s.stalwart_account_id.as_deref().expect("acct"), s.address.as_str()));
    assert_eq!(m.rcpt, vec!["dana@x.test".to_string()]);
    assert_eq!(m.email["from"][0]["email"], s.address, "From is server-stamped");
    assert_eq!(m.email["replyTo"], json!([{"email": "help@brand.test"}]));
    assert_eq!(m.email["header:X-App-Order:asText"], "42");
    assert_eq!(m.email["htmlBody"][0]["type"], "text/html");
    assert_eq!(m.email["textBody"][0]["type"], "text/plain");
    let id = out["id"].as_str().expect("id");
    assert_eq!(m.params, json!({"RET": "HDRS", "ENVID": id}), "E8");
    assert!(m.destroy, "no stored copy (E7)");
    // The log: metadata only, links hidden in the subject.
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_log(&conn, id).expect("read").expect("row")
    };
    assert_eq!((row.status.as_str(), row.recipient_count), ("submitted", 1));
    assert!(row.subject.contains("k2inv_[hidden]") && !row.subject.contains("abc"), "{}", row.subject);
    assert!(row.message_id.is_some());
    let dump = format!("{row:?}");
    assert!(!dump.contains("BODYTOKEN"), "no body in the log: {dump}");
    // No body file anywhere K2 keeps mail.
    let dir = crate::mail::send::DbOutboundStore::default();
    let _ = dir;
    let out_dir = std::env::temp_dir().join(format!("k2-mail-outbound-testdefault-{}", std::process::id()));
    if let Ok(rd) = std::fs::read_dir(&out_dir) {
        for e in rd.flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            assert!(!text.contains("BODYTOKEN"), "a notification body landed in mail-outbound: {:?}", e.path());
        }
    }
    // status and log read it back; another App can't see it.
    assert_eq!(send::status(&f.app, id).expect("status")["status"], "submitted");
    assert_eq!(code(&send::status(&f.other_app, id)), "not_found");
    let log = send::log(&f.app, None, None, None, Some("orders")).expect("log");
    assert_eq!(log["items"].as_array().expect("items").len(), 1);
}

#[test]
fn no_row_no_send_and_an_unreadable_budget_refuses() {
    let f = fx("n2fail");
    on_app(&f, store::SCOPE_ANY);
    let v = json!({"to": "a@x.test", "subject": "s", "text": "t"});
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute_batch("CREATE TRIGGER k2_test_no_log BEFORE INSERT ON email_notify_log BEGIN SELECT RAISE(FAIL, 'nope'); END;").expect("trigger");
    }
    let r = send::send(&app_caller(&f.app), &v, now());
    assert_eq!(r.expect_err("no row").status, 500);
    assert!(f.engine.sent.lock().unwrap().is_empty(), "no row, no send");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute_batch("DROP TRIGGER k2_test_no_log; ALTER TABLE email_notify_log RENAME COLUMN recipient_count TO rc_gone;").expect("break the counter");
    }
    let r = send::send(&app_caller(&f.app), &v, now());
    let e = r.expect_err("fail-closed");
    assert_eq!((e.status, e.code), (503, "failed"), "{e:?}");
    assert!(f.engine.sent.lock().unwrap().is_empty());
}

#[test]
fn refusals_name_their_code_and_are_logged() {
    let f = fx("n2refuse");
    let caller = app_caller(&f.app);
    let v = json!({"to": "a@x.test", "subject": "s", "text": "t"});
    assert_eq!(code(&send::send(&caller, &v, now())), "email_off");
    let s = on_app(&f, store::SCOPE_USERS);
    assert_eq!(code(&send::send(&caller, &v, now())), "recipient_out_of_scope", "a stranger under scope users");
    // An agent-made App user whose address isn't proven (E16).
    skin::add_principal_for_app("eli", Some("eli@x.test"), &f.app, None, "agent:x").expect("eli");
    assert_eq!(code(&send::send(&caller, &json!({"to": "eli@x.test", "subject": "s", "text": "t"}), now())), "recipient_out_of_scope");
    proven_user(&f.app, "dana", "dana@x.test");
    let ok = json!({"to": "dana@x.test", "subject": "s", "text": "t"});
    for (bad, want) in [
        (json!({"to": "dana@x.test", "subject": "s", "text": "t", "headers": {"Bcc": "x@y.test"}}), "header_not_allowed"),
        (json!({"to": "dana@x.test", "subject": "s", "text": "t", "headers": {"X-K2-x": "1"}}), "header_not_allowed"),
        (json!({"to": "dana@x.test", "subject": "s", "text": "t", "headers": {"X-App-A": "1\r\nBcc: e@x.test"}}), "header_not_allowed"),
        (json!({"to": "dana@x.test", "subject": "s", "text": "t", "from": "other@notify.brand.test"}), "sender_not_granted"),
        (json!({"to": "dana@x.test", "subject": "s", "text": "t", "replyTo": "evil@phish.test"}), "recipient_out_of_scope"),
    ] {
        assert_eq!(code(&send::send(&caller, &bad, now())), want, "{bad}");
    }
    let tos: Vec<String> = (0..51).map(|i| format!("u{i}@x.test")).collect();
    assert_eq!(code(&send::send(&caller, &json!({"to": tos, "subject": "s", "text": "t"}), now())), "too_many_recipients");
    // The system sender named as from.
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    assert_eq!(code(&send::send(&caller, &json!({"to": "dana@x.test", "subject": "s", "text": "t", "from": sys.address}), now())), "system_sender");
    // fromName can't be the system sender's (E20).
    assert_eq!(code(&send::send(&caller, &json!({"to": "dana@x.test", "subject": "s", "text": "t", "fromName": "BRAND"}), now())), "usage");
    // Air-gap (E21).
    crate::push_routes::set_test_airgap(true);
    assert_eq!(send::send(&caller, &ok, now()).expect_err("airgap").status, 503);
    crate::push_routes::set_test_airgap(false);
    assert!(f.engine.sent.lock().unwrap().is_empty(), "nothing left");
    send::send(&caller, &ok, now()).expect("the good one goes");
    let refused = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::count_refused(&conn, &f.app, 0).expect("count")
    };
    assert!(refused >= 9, "every refusal is a refused row: {refused}");
    let _ = s;
}

#[test]
fn budgets_count_recipients_and_answer_retry_after() {
    let f = fx("n2budget");
    let s = on_app(&f, store::SCOPE_ANY);
    let now = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let to: Vec<String> = (0..30).map(|i| format!("u{i}@x.test")).collect();
        store::insert_log(
            &conn,
            &store::NewLog {
                sender_id: &s.id,
                origin: store::ORIGIN_APP,
                app_service_id: Some(&f.app),
                purpose: None,
                to: &to,
                subject: "",
                tags: &[],
                status: store::ST_SUBMITTED,
                error_code: None,
                message_id: None,
                send_after: None,
                scope_any: true,
                now: now - 10,
            },
        )
        .expect("prior sends");
    }
    let e = send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "t"}), now).expect_err("any: 30/h");
    assert_eq!((e.status, e.code), (429, "rate_limited"));
    assert_eq!(e.retry_after_ms, Some(3590 * 1000));
    let r = fail_response(&e);
    assert_eq!(body_of(&r)["error"]["retryAfterMs"], 3_590_000);
}

#[test]
fn a_new_domain_warms_up() {
    let f = fx("n2warm");
    let s = on_app(&f, store::SCOPE_USERS);
    proven_user(&f.app, "dana", "dana@x.test");
    let now = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_domains SET verified_at = ?1 WHERE id = 'dom1'", params![now - 86_400]).expect("young domain");
        let to: Vec<String> = (0..20).map(|i| format!("w{i}@x.test")).collect();
        store::insert_log(
            &conn,
            &store::NewLog {
                sender_id: &s.id,
                origin: store::ORIGIN_APP,
                app_service_id: Some(&f.other_app),
                purpose: None,
                to: &to,
                subject: "",
                tags: &[],
                status: store::ST_SUBMITTED,
                error_code: None,
                message_id: None,
                send_after: None,
                scope_any: false,
                now: now - 60,
            },
        )
        .expect("prior sends on the domain");
    }
    let e = send::send(&app_caller(&f.app), &json!({"to": "dana@x.test", "subject": "s", "text": "t"}), now).expect_err("warm-up 20/h");
    assert_eq!(e.code, "rate_limited", "{e:?}");
    assert!(e.hint.contains("warm-up"), "{}", e.hint);
    // System mail is exempt from warm-up.
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now).expect("system");
    let c = send::Composed { subject: "Your code".into(), text: "123456".into(), html: None };
    send::system_send("email_code", "dana@x.test", &c, None).expect("system mail isn't warming up");
}

#[test]
fn an_idempotent_replay_sends_once() {
    let f = fx("n2idem");
    on_app(&f, store::SCOPE_ANY);
    let v = json!({"to": "a@x.test", "subject": "s", "text": "t", "idempotencyKey": "order-1"});
    let a = send::send(&app_caller(&f.app), &v, now()).expect("first");
    let b = send::send(&app_caller(&f.app), &v, now()).expect("replay");
    assert_eq!(a["id"], b["id"]);
    assert_eq!(f.engine.sent.lock().unwrap().len(), 1, "one submit");
    let changed = json!({"to": "a@x.test", "subject": "s", "text": "other", "idempotencyKey": "order-1"});
    assert_eq!(code(&send::send(&app_caller(&f.app), &changed, now())), "idempotency_conflict");
    // Another App's key space is its own.
    let s2 = add_hosted("crm@notify.brand.test");
    grant(&f.other_app, &s2, store::SCOPE_ANY);
    send::send(&app_caller(&f.other_app), &v, now()).expect("the other App's own key");
}

#[test]
fn dmarc_any_policy_passes_missing_refuses_and_a_subdomain_inherits() {
    let f = fx("n2dmarc");
    assert!(dmarc::check(DOMAIN, now()).is_ok(), "brand.test's p=reject covers notify.brand.test");
    f.dns.txt.lock().unwrap().clear();
    dmarc::forget(DOMAIN);
    let e = dmarc::check(DOMAIN, now()).expect_err("missing");
    assert_eq!(e.code, "dmarc_missing");
    f.dns.set("_dmarc.notify.brand.test", "v=DMARC1; p=quarantine");
    dmarc::forget(DOMAIN);
    assert_eq!(dmarc::check(DOMAIN, now()).expect("own").policy, "quarantine");
    // A sender added without DMARC waits as needs_dmarc and never sends.
    f.dns.txt.lock().unwrap().clear();
    dmarc::forget(DOMAIN);
    let s = add_hosted("noreply@notify.brand.test");
    assert_eq!(s.state, store::STATE_NEEDS_DMARC);
    grant(&f.app, &s, store::SCOPE_ANY);
    assert_eq!(code(&send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "t"}), now())), "dmarc_missing");
    f.dns.set("_dmarc.brand.test", "v=DMARC1; p=none");
    dmarc::forget(DOMAIN);
    send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "t"}), now()).expect("ready now");
    assert_eq!(senders::find_live(&s.id).expect("s").state, store::STATE_READY);
}

#[test]
fn owner_resets_use_the_system_sender_and_keep_no_copy() {
    let f = fx("n2reset");
    assert!(crate::app_user_invites::owner_reset_sender(&f.app, None).map_err(|r| r.body).expect("no error").is_none(), "no system sender: today's answer");
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    let s = crate::app_user_invites::owner_reset_sender(&f.app, None).map_err(|r| r.body).expect("ok").expect("a sender");
    assert_eq!((s.address.as_str(), s.notify_sender.as_deref()), (sys.address.as_str(), Some(sys.id.as_str())));
    let c = send::Composed { subject: "Reset your password".into(), text: "link".into(), html: None };
    let id = send::system_send("owner_reset", "dana@x.test", &c, Some(&f.app)).expect("sent");
    let sent = f.engine.sent.lock().unwrap().clone();
    assert!(sent[0].destroy, "the stored copy is destroyed");
    assert_eq!(sent[0].from, sys.address);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = store::get_log(&conn, &id).expect("read").expect("row");
    assert_eq!((row.origin.as_str(), row.purpose.as_deref(), row.subject.as_str()), ("system", Some("owner_reset"), ""), "system subjects aren't kept");
    drop(conn);
    // With the App's own sender on, its resets come from it (Q2).
    let own = add_hosted("noreply@notify.brand.test");
    grant(&f.app, &own, store::SCOPE_USERS);
    let s = crate::app_user_invites::owner_reset_sender(&f.app, None).map_err(|r| r.body).expect("ok").expect("a sender");
    assert_eq!(s.address, own.address);
}

#[test]
fn invites_prefer_the_apps_sender_and_never_ride_the_system_sender() {
    let f = fx("n2invite");
    let by = crate::app_user_invites::By::Agent { ws: f.ws.clone(), actor: "agent:x".into() };
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    let none = crate::app_user_invites::invite_sender(&f.app, Some(&f.ws), &by, None).map_err(|r| r.body).expect("ok");
    assert!(none.is_none(), "E14: no App sender and no workspace address: the system sender is never used for App text");
    let own = add_hosted("noreply@notify.brand.test");
    grant(&f.app, &own, store::SCOPE_USERS);
    let s = crate::app_user_invites::invite_sender(&f.app, Some(&f.ws), &by, None).map_err(|r| r.body).expect("ok").expect("the App's");
    assert_eq!((s.address.as_str(), s.notify_sender.as_deref()), (own.address.as_str(), Some(own.id.as_str())));
    // The agent's gate rides along (off by default refuses).
    assert_eq!(s.gate, crate::mail::send::Gate::Off);
    let c = send::Composed { subject: "Join".into(), text: "link".into(), html: None };
    let e = send::app_link_mail(&own, &f.app, "invite", "dana@x.test", &c, s.gate, &f.ws, "agent:x").expect_err("gated");
    assert_eq!(e.code, "gated");
    let r = send::app_link_mail(&own, &f.app, "invite", "dana@x.test", &c, crate::mail::send::Gate::On, &f.ws, "agent:x").expect("sent");
    assert!(r.is_ok());
    assert_eq!(f.engine.sent.lock().unwrap().last().expect("sent").from, own.address);
}

#[test]
fn a_scheduled_send_waits_then_rechecks() {
    let f = fx("n2sched");
    on_app(&f, store::SCOPE_ANY);
    let now = now();
    let out = send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "later", "sendAt": now + 600}), now).expect("scheduled");
    assert_eq!(out["status"], "scheduled");
    assert!(f.engine.sent.lock().unwrap().is_empty());
    assert_eq!(send::flush_due(now + 599, 10), 0);
    assert_eq!(send::flush_due(now + 601, 10), 1);
    assert_eq!(f.engine.sent.lock().unwrap().len(), 1);
    assert_eq!(send::status(&f.app, out["id"].as_str().expect("id")).expect("s")["status"], "submitted");
    // The App turned off before it was due: it fails, nothing leaves.
    let out = send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "later", "sendAt": now + 600}), now).expect("scheduled");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_app_off(&conn, &f.app, "test", now).expect("off");
    }
    send::flush_due(now + 601, 10);
    assert_eq!(f.engine.sent.lock().unwrap().len(), 1);
    let st = send::status(&f.app, out["id"].as_str().expect("id")).expect("s");
    assert_eq!((st["status"].clone(), st["error"].clone()), (json!("failed"), json!("email_off")));
}

#[test]
fn log_lines_here_never_name_an_address_or_a_subject() {
    // E22: a static check of every daemon log line in this module.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("email_notify");
    let mut seen = 0;
    for e in std::fs::read_dir(&dir).expect("dir").flatten() {
        let src = std::fs::read_to_string(e.path()).expect("read");
        for (n, line) in src.lines().enumerate() {
            if !line.contains("log_debug!(") || e.path().ends_with("tests.rs") {
                continue;
            }
            seen += 1;
            for bad in ["address", "subject", "{to", "email}", ".to)", "text"] {
                assert!(!line.contains(bad), "{}:{}: a log line may name {bad}: {line}", e.path().display(), n + 1);
            }
        }
    }
    assert!(seen >= 5, "found the log lines ({seen})");
}

#[test]
fn notice_md_carries_stalwarts_agpl_notice_and_source() {
    // E23: removing the Stalwart section from NOTICE.md fails here.
    let notice = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../NOTICE.md")).expect("NOTICE.md");
    let pin = crate::mail::supervisor::STALWART_PINNED_VERSION;
    for want in ["Stalwart", "AGPL-3.0-only", "github.com/Alakazam-211/stalwart", &format!("v{pin}"), pin, "never linked, vendored or patched"] {
        assert!(notice.contains(want), "NOTICE.md lacks {want:?} (E23)");
    }
}

// ── N3: relays ───────────────────────────────────────────────────────

fn relay_args() -> relay::AddArgs {
    relay::AddArgs {
        host: "smtp.relay.test".into(),
        port: 587,
        tls: "starttls".into(),
        username: "AKIA".into(),
        password: "S3CRET-relay-pw".into(),
        from_domain: "brand.test".into(),
    }
}

#[test]
fn a_closed_vault_keeps_no_relay_and_writes_no_file() {
    let f = fx("n3closed");
    engine::set_test_creds(Some(Arc::new(MemCreds { closed: true, map: Mutex::new(HashMap::new()) })));
    let e = relay::add(&relay_args(), "owner", now()).expect_err("vault closed");
    assert_eq!((e.status, e.code), (503, "vault_closed"));
    assert!(relay::list().expect("list").is_empty(), "no row without its password");
    assert!(!f._home.path().join(".k2").join("mail-secrets.json").exists(), "never the 0600 file fallback");
    assert!(!format!("{:?}", relay_args()).contains("S3CRET"), "Debug is redacted");
}

#[test]
fn a_mac_sends_through_a_relay_after_a_passing_test() {
    let f = fx("n3mac");
    engine::set_test_hosted_supported(false);
    // Hosted senders are refused on a box without hosted mail.
    let e = senders::add(&senders::AddArgs { address: "noreply@notify.brand.test".into(), ..Default::default() }, "owner", now()).expect_err("no hosted mail");
    assert_eq!(e.code, "no_mail_path");
    let r = relay::add(&relay_args(), "owner", now()).expect("relay");
    let relay_id = r["relay"]["id"].as_str().expect("id").to_string();
    assert_eq!(f.creds.map.lock().unwrap().get(&relay_id).map(String::as_str), Some("S3CRET-relay-pw"));
    assert!(!r.to_string().contains("S3CRET"), "never echoed: {r}");
    let v = senders::add(&senders::AddArgs { address: "noreply@brand.test".into(), relay: Some(relay_id.clone()), ..Default::default() }, "owner", now()).expect("relay sender");
    let sid = v["sender"]["id"].as_str().expect("id").to_string();
    grant(&f.app, &senders::find_live(&sid).expect("s"), store::SCOPE_ANY);
    let msg = json!({"to": "a@x.test", "subject": "s", "text": "t", "html": "<p>h</p>", "replyTo": "help@brand.test"});
    assert_eq!(code(&send::send(&app_caller(&f.app), &msg, now())), "relay_not_ready", "no test yet");
    // The test goes to the owner's own address only.
    assert_eq!(relay::test(&relay_id, &sid, "owner", now()).expect_err("no owner address").code, "no_owner_email");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_owner_email(&conn, Some("owner@me.test"), now()).expect("owner email");
    }
    relay::test(&relay_id, &sid, "owner", now()).expect("test");
    let out = send::send(&app_caller(&f.app), &msg, now()).expect("sent through the relay");
    assert_eq!(out["status"], "submitted");
    let sent = f.relay.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "the test and the send");
    assert_eq!(sent[0].1, vec!["owner@me.test".to_string()], "tests only reach the owner");
    let raw = String::from_utf8_lossy(&sent[1].2).to_string();
    assert!(raw.contains("multipart/alternative") && raw.contains("Reply-To: help@brand.test"), "{raw}");
    assert!(f.engine.sent.lock().unwrap().is_empty(), "no hosted path on a Mac");
}

#[test]
fn a_relay_error_never_carries_the_password() {
    let f = fx("n3pw");
    let r = relay::add(&relay_args(), "owner", now()).expect("relay");
    let relay_id = r["relay"]["id"].as_str().expect("id").to_string();
    engine::set_test_relay(Some(Arc::new(FakeRelay { sent: Mutex::new(Vec::new()), fail_with_password: true })));
    let relay_row = relay::list().expect("list").remove(0);
    let e = relay::submit(&relay_row, "noreply@brand.test", &["a@x.test".to_string()], b"x").expect_err("refused");
    assert!(!e.hint.contains("S3CRET"), "{}", e.hint);
    assert!(e.hint.contains("<redacted>"), "{}", e.hint);
    let _ = (f, relay_id);
}

/// A minimal scripted SMTP server on loopback: greets, EHLO (AUTH PLAIN),
/// AUTH, MAIL, RCPT, DATA, QUIT. Returns the DATA payload.
fn spawn_smtp_mock() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let h = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).expect("timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut w = stream;
        let mut data = Vec::new();
        w.write_all(b"220 mock ESMTP\r\n").expect("w");
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let verb = line.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
            match verb.as_str() {
                "EHLO" | "HELO" => w.write_all(b"250-mock\r\n250 AUTH PLAIN LOGIN\r\n").expect("w"),
                "AUTH" => w.write_all(b"235 ok\r\n").expect("w"),
                "MAIL" | "RCPT" | "RSET" => w.write_all(b"250 ok\r\n").expect("w"),
                "DATA" => {
                    w.write_all(b"354 go\r\n").expect("w");
                    loop {
                        let mut dl = String::new();
                        if reader.read_line(&mut dl).unwrap_or(0) == 0 || dl.trim_end() == "." {
                            break;
                        }
                        data.extend_from_slice(dl.as_bytes());
                    }
                    w.write_all(b"250 queued\r\n").expect("w");
                }
                "QUIT" => {
                    w.write_all(b"221 bye\r\n").expect("w");
                    break;
                }
                other => panic!("mock: unexpected {other}"),
            }
        }
        data
    });
    (port, h)
}

#[test]
fn the_relay_message_reaches_a_loopback_smtp_server_as_alternative() {
    let f = fx("n3loop");
    let (port, h) = spawn_smtp_mock();
    engine::set_test_relay(Some(Arc::new(PlainRelay)));
    let mut a = relay_args();
    a.host = "localhost.test".into();
    let r = relay::add(&a, "owner", now()).expect("relay");
    let id = r["relay"]["id"].as_str().expect("id").to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE email_relays SET host = '127.0.0.1', port = ?2 WHERE id = ?1", params![id, port as i64]).expect("loopback");
    }
    let row = relay::list().expect("list").remove(0);
    let headers = Default::default();
    let raw = compose::rfc822(
        &compose::Message {
            from_name: "Brand",
            from: "noreply@brand.test",
            to: &["a@x.test".to_string()],
            cc: &[],
            subject: "Hello",
            text: Some("plain"),
            html: Some("<p>html</p>"),
            reply_to: &["help@brand.test".to_string()],
            headers: &headers,
            message_id: "mid-9@brand.test",
        },
        std::time::SystemTime::now(),
    )
    .expect("compose");
    relay::submit(&row, "noreply@brand.test", &["a@x.test".to_string()], &raw).expect("submitted over loopback");
    let data = String::from_utf8_lossy(&h.join().expect("mock")).to_string();
    assert!(data.contains("multipart/alternative"), "{data}");
    assert!(data.contains("Reply-To: help@brand.test"), "{data}");
    assert!(data.contains("Message-ID: <mid-9@brand.test>"), "{data}");
    let _ = f;
}

// ── N4: the App door, scope, keys, held sends ────────────────────────

fn link_pass(f: &Fx, caps: &[&str]) -> skin::SkinPass {
    let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
    let m = skin::link::mint_link_pass(&f.app, "shop", "link-email", &caps, std::slice::from_ref(&f.ws)).expect("link pass");
    skin::link::resolve_link_pass(&m.raw, "link-email", &f.app).expect("resolves on its link")
}

fn link_call(path: &str, pass: &skin::SkinPass, link_app: Option<&str>, is_post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = link_routes::handle_pass(path, pass, link_app, is_post, body.to_string().as_bytes(), &params);
    (status_of(&r), body_of(&r))
}

#[test]
fn the_link_sends_only_with_its_own_pass_and_email_send() {
    let f = fx("n4link");
    on_app(&f, store::SCOPE_ANY);
    let body = json!({"to": "a@x.test", "subject": "Hi", "text": "There"});
    let with = link_pass(&f, &["email:send", "thread:read"]);
    let (st, v) = link_call(link_routes::SEND, &with, Some(f.app.as_str()), true, body.clone(), &[]);
    assert_eq!((st, v["status"].clone()), (202, json!("submitted")), "{v}");
    assert!(v.get("accepted").is_none(), "the 202 marker isn't in the body: {v}");
    let id = v["id"].as_str().expect("id").to_string();
    let (st, v) = link_call(link_routes::STATUS, &with, Some(f.app.as_str()), false, json!({}), &[("id", &id)]);
    assert_eq!((st, v["status"].clone()), (200, json!("submitted")), "{v}");
    let (st, v) = link_call(link_routes::LOG, &with, Some(f.app.as_str()), false, json!({}), &[]);
    assert_eq!((st, v["items"].as_array().map(Vec::len)), (200, Some(1)), "{v}");
    let without = link_pass(&f, &["thread:read"]);
    let (st, v) = link_call(link_routes::SEND, &without, Some(f.app.as_str()), true, body.clone(), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("cap_not_granted")), "{v}");
    for link in [None, Some(f.other_app.as_str())] {
        let (st, v) = link_call(link_routes::SEND, &with, link, true, body.clone(), &[]);
        assert_eq!((st, v["error"]["code"].clone()), (403, json!("app_pass_required")), "{v}");
    }
    skin::add_principal_for_app("dana", Some("dana@x.test"), &f.app, None, "test").expect("dana");
    let s = skin::create_app_session_token("dana", &f.app).expect("session");
    let guest = skin::resolve_skin_token(&s.raw).expect("pass");
    let (st, v) = link_call(link_routes::SEND, &guest, Some(f.app.as_str()), true, body.clone(), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("app_pass_required")), "a guest never sends: {v}");
    // The App comes from the pass, never the body.
    let mut b = body.clone();
    b["app"] = json!(f.other_app);
    let (st, v) = link_call(link_routes::SEND, &with, Some(f.app.as_str()), true, b, &[]);
    assert_eq!((st, v["error"]["code"].clone()), (400, json!("usage")), "{v}");
    // Another App's id isn't found.
    let (st, _) = link_call(link_routes::STATUS, &with, Some(f.app.as_str()), false, json!({}), &[("id", "enl_nope")]);
    assert_eq!(st, 404);
}

#[test]
fn the_app_key_door_needs_scope_email_and_its_own_app() {
    let f = fx("n4key");
    on_app(&f, store::SCOPE_ANY);
    let mint = |app: &str, scopes: &[&str]| {
        let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        let g = k2_core::app_grace::keys::grant_for_app(app, &scopes, &[], &[]).expect("grant");
        k2_core::api_keys::create_app_key("test", app, &g.to_json()).expect("key").1
    };
    let email_key = mint(&f.app, &["email"]);
    let push_key = mint(&f.app, &["push"]);
    let other_key = mint(&f.other_app, &["email"]);
    let body = json!({"to": "a@x.test", "subject": "Hi", "text": "There"}).to_string();
    let path = format!("/v1/apps/{}/email/send", f.app);
    let r = crate::apps::app_key_routes::handle(&path, true, Some(&email_key), body.as_bytes(), &HashMap::new());
    assert_eq!(r.status, "202 Accepted", "{}", r.body);
    for k in [&push_key, &other_key] {
        let r = crate::apps::app_key_routes::handle(&path, true, Some(k), body.as_bytes(), &HashMap::new());
        assert_eq!(r.status, "404 Not Found", "the uniform answer: {}", r.body);
    }
    assert!(crate::apps::app_key_routes::post_allowed(&path));
    assert_eq!(crate::apps::app_key_routes::body_cap(&path), MAX_BODY, "an email may be 1 MB");
    let id = body_of(&crate::apps::app_key_routes::handle(&path, true, Some(&email_key), body.as_bytes(), &HashMap::new()))["id"].as_str().expect("id").to_string();
    let q: HashMap<String, String> = [("id".to_string(), id)].into_iter().collect();
    let r = crate::apps::app_key_routes::handle(&format!("/v1/apps/{}/email/status", f.app), false, Some(&email_key), b"", &q);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(f.engine.sent.lock().unwrap().len(), 2);
}

#[test]
fn email_send_is_in_no_core_and_is_the_apps_own_cap() {
    assert_eq!(k2_core::workspace::settings::app_core_of_cap(skin::CAP_EMAIL_SEND), None, "the Mail tick never pre-approves it (Q13)");
    assert!(skin::APP_ONLY_CAPS.contains(&skin::CAP_EMAIL_SEND));
    assert!(k2_core::app_grace::keys::SCOPES.contains(&k2_core::app_grace::keys::SCOPE_EMAIL));
}

fn agent_switches(ws: &str, build: bool, full: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1",
        params![ws, build as i64, full as i64],
    )
    .expect("switches");
}

fn app_post(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = app_routes::handle(path, who.clone(), true, body.to_string().as_bytes(), &HashMap::new());
    (status_of(&r), body_of(&r))
}

#[test]
fn on_is_the_owners_with_a_confirm_and_any_needs_a_second() {
    let f = fx("n4on");
    let s = add_hosted("noreply@notify.brand.test");
    agent_switches(&f.ws, true, true);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let (st, v) = app_post(app_routes::ON, &agent, json!({"app": f.app, "sender": s.address}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address}));
    let bind = app_binding(&f.app, &s.address);
    assert_eq!((st, v["error"]["action"].clone(), v["error"]["serviceId"].clone()), (403, json!(CONFIRM_APP_ON), json!(f.app)), "{v}");
    assert_eq!(v["error"]["secretId"], json!(bind), "L11: the confirm binds the App and the sender");
    let vault = crate::vault::ensure_test_vault();
    // L11: a confirm for the App alone, or for another sender, never turns it on.
    let other = add_hosted("other@notify.brand.test");
    for wrong in [f.app.clone(), app_binding(&f.app, &other.address)] {
        let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&wrong), "owner", "local-auth");
        let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address, "confirmToken": tok}));
        assert_eq!((st, v["error"]["code"].clone()), (403, json!("vault_confirm_required")), "{wrong}: {v}");
    }
    let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&bind), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address, "scope": "any", "confirmToken": tok}));
    assert_eq!((st, v["error"]["action"].clone()), (403, json!(CONFIRM_APP_ANY)), "any asks again: {v}");
    let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&bind), "owner", "local-auth");
    let any = vault.confirm.mint(CONFIRM_APP_ANY, Some(&bind), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address, "scope": "any", "confirmToken": tok, "confirmAnyToken": any}));
    assert_eq!((st, v["on"].clone(), v["recipientScope"].clone()), (200, json!(true), json!("any")), "{v}");
    // The App's agent may turn it off, never on.
    let (st, v) = app_post(app_routes::OFF, &agent, json!({"app": f.app}));
    assert_eq!((st, v["on"].clone()), (200, json!(false)), "{v}");
    // The system sender is never granted.
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&app_binding(&f.other_app, &sys.address)), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.other_app, "sender": sys.address, "confirmToken": tok}));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("system_sender")), "{v}");
    // The confirm sheet names the App from the daemon's record.
    let reason = confirm_reason(CONFIRM_APP_ANY, Some(&bind)).expect("ours").expect("an App");
    assert!(reason.contains("shop") && reason.contains("ANY") && reason.contains(&s.address), "{reason}");
    let reason = confirm_reason(CONFIRM_APP_ON, Some(&bind)).expect("ours").expect("an App");
    assert!(reason.contains("shop") && reason.contains(&s.address), "L11: the sheet names the sender: {reason}");
    assert!(matches!(confirm_reason(CONFIRM_APP_ON, Some(&f.app)), Some(Err(_))), "the old App-only id is refused");
    assert!(matches!(confirm_reason(CONFIRM_APP_ON, Some(&app_binding(&f.app, "nobody@notify.brand.test"))), Some(Err(_))), "an unknown sender");
    // L11: the relay confirm binds host, From domain and login, and the sheet prints them.
    let rb = relay_binding("smtp.relay.test", "brand.test", "user@relay");
    assert_eq!(relay_binding_parts(&rb), Some(("smtp.relay.test", "brand.test", "user@relay")));
    let r = confirm_reason(CONFIRM_RELAY, Some(&rb)).expect("ours").expect("text");
    assert!(r.contains("smtp.relay.test") && r.contains("brand.test") && r.contains("user@relay"), "{r}");
    assert!(matches!(confirm_reason(CONFIRM_RELAY, Some("smtp.relay.test")), Some(Err(_))), "host alone is refused");
}

#[test]
fn the_apps_agent_sends_under_its_gate_and_approval_holds_it() {
    let f = fx("n4held");
    let s = on_app(&f, store::SCOPE_ANY);
    agent_switches(&f.ws, true, true);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let send_body = json!({"app": f.app, "to": "a@x.test", "subject": "Held", "text": "t", "html": "<b>h</b>"});
    let (st, v) = app_post(app_routes::SEND, &agent, send_body.clone());
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("gated")), "agentSend is off by default: {v}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET mail_agent_send = 'approval' WHERE id = ?1", params![f.ws]).expect("approval");
    }
    let (st, v) = app_post(app_routes::SEND, &agent, send_body.clone());
    assert_eq!((st, v["status"].clone()), (202, json!("held")), "{v}");
    let ob = v["outboundId"].as_str().expect("outbound").to_string();
    assert!(f.engine.sent.lock().unwrap().is_empty(), "nothing left before approval");
    // An IT agent can't decide it; the owner can.
    let r = as_it_agent(&f, || crate::mail::routes_send::handle_approvals_approve(json!({"id": ob}).to_string().as_bytes()));
    assert_eq!(status_of(&r), 403, "{}", r.body);
    let r = crate::mail::routes_send::handle_approvals_approve(json!({"id": ob}).to_string().as_bytes());
    assert_eq!(status_of(&r), 200, "{}", r.body);
    let sent = f.engine.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from, s.address, "from the App's sender");
    assert_eq!(sent[0].email["htmlBody"][0]["type"], "text/html", "the held html rode along");
    let st = send::status(&f.app, v["id"].as_str().expect("id")).expect("status");
    assert_eq!(st["status"], "submitted");
    // A person sends as themselves, no gate.
    let (st, v) = app_post(app_routes::SEND, &Who::Owner, send_body);
    assert_eq!((st, v["status"].clone()), (202, json!("submitted")), "{v}");
    // The log: the owner, the App's agent; never another workspace's agent.
    let stranger = Who::Agent { project_id: f.other_ws.clone(), session_id: "s".into() };
    let q: HashMap<String, String> = [("app".to_string(), f.app.clone())].into_iter().collect();
    assert_eq!(status_of(&app_routes::handle(app_routes::LOG, agent, false, b"", &q)), 200);
    assert_eq!(status_of(&app_routes::handle(app_routes::LOG, stranger, false, b"", &q)), 404);
}

#[test]
fn a_shell_tab_is_its_workspaces_agent_never_a_person_l1() {
    let f = fx("n11l1");
    on_app(&f, store::SCOPE_ANY);
    let shell = Who::Shell { project_id: f.ws.clone(), session_id: "s".into() };
    let send_body = json!({"app": f.app, "to": "a@x.test", "subject": "Shell", "text": "t"});
    // No switches: a shell tab is no longer let through as a person.
    agent_switches(&f.ws, false, false);
    let (st, v) = app_post(app_routes::SEND, &shell, send_body.clone());
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("forbidden")), "{v}");
    let q: HashMap<String, String> = [("app".to_string(), f.app.clone())].into_iter().collect();
    assert_eq!(status_of(&app_routes::handle(app_routes::LOG, shell.clone(), false, b"", &q)), 403, "no read either");
    // With the switches, it is the App's agent: the agentSend gate applies (off by default).
    agent_switches(&f.ws, true, true);
    let (st, v) = app_post(app_routes::SEND, &shell, send_body.clone());
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("gated")), "{v}");
    assert!(f.engine.sent.lock().unwrap().is_empty(), "nothing left the box");
    assert_eq!(status_of(&app_routes::handle(app_routes::LOG, shell, false, b"", &q)), 200, "reads as the App's agent");
    // A shell tab in another workspace is nobody here.
    let other = Who::Shell { project_id: f.other_ws.clone(), session_id: "s".into() };
    let (st, _) = app_post(app_routes::SEND, &other, send_body.clone());
    assert!(st == 403 || st == 404, "{st}");
    // Turning email on stays the owner's in person.
    let (st, v) = app_post(app_routes::ON, &Who::Shell { project_id: f.ws.clone(), session_id: "s".into() }, json!({"app": f.app, "sender": "x@y.test"}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "{v}");
    // The owner token and an Admin login still send as themselves.
    let (st, v) = app_post(app_routes::SEND, &Who::Owner, send_body.clone());
    assert_eq!((st, v["status"].clone()), (202, json!("submitted")), "{v}");
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    let (st, v) = app_post(app_routes::SEND, &admin, send_body);
    assert_eq!((st, v["status"].clone()), (202, json!("submitted")), "{v}");
}

#[test]
fn the_test_goes_only_to_the_owner() {
    let f = fx("n4test");
    on_app(&f, store::SCOPE_USERS);
    agent_switches(&f.ws, true, false);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let (st, v) = app_post(app_routes::TEST, &agent, json!({"app": f.app}));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("no_owner_email")), "{v}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_owner_email(&conn, Some("owner@me.test"), now()).expect("owner email");
    }
    let (st, v) = app_post(app_routes::TEST, &agent, json!({"app": f.app, "to": "someone@else.test"}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(f.engine.sent.lock().unwrap().last().expect("sent").rcpt, vec!["owner@me.test".to_string()], "never another address");
}

#[test]
fn the_box_switch_and_confirm_texts() {
    let _f = fx("n4switch");
    assert!(crate::app_push::switch_off(Some("off")));
    assert!(confirm_reason("not-ours", Some("x")).is_none());
    assert!(matches!(confirm_reason(CONFIRM_SENDER, None), Some(Err(_))));
    let r = confirm_reason(CONFIRM_SENDER, Some("noreply@brand.test")).expect("ours").expect("text");
    assert!(r.contains("noreply@brand.test") && r.contains("no agent can read"), "{r}");
    assert!(CONFIRM_ACTIONS.iter().all(|a| confirm_reason(a, Some("x@y.test")).is_some()));
}

// ── security review (review-email-notifications-core-security-v1) ─────

fn log_row(id: &str) -> store::LogRow {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_log(&conn, id).expect("read").expect("row")
}

fn key_for(app: &str, scopes: &[&str], minted_by_agent: Option<&str>) -> String {
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    let mut g = k2_core::app_grace::keys::grant_for_app(app, &scopes, &[], &[]).expect("grant");
    g.minted_by_agent = minted_by_agent.map(str::to_string);
    k2_core::api_keys::create_app_key("test", app, &g.to_json()).expect("key").1
}

fn key_send(app: &str, key: &str, body: &J) -> (u16, J) {
    let r = crate::apps::app_key_routes::handle(&format!("/v1/apps/{app}/email/send"), true, Some(key), body.to_string().as_bytes(), &HashMap::new());
    (status_of(&r), body_of(&r))
}

fn approval_gate(ws: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("UPDATE projects SET mail_agent_send = 'approval' WHERE id = ?1", params![ws]).expect("approval");
}

#[test]
fn h1_removing_an_app_user_never_gives_budget_back() {
    let f = fx("h1budget");
    on_app(&f, store::SCOPE_ANY);
    skin::add_principal_for_app("sink", Some("sink@attacker.test"), &f.app, None, "test").expect("sink");
    let bcc: Vec<String> = (0..29).map(|i| format!("t{i}@victim.test")).collect();
    let out = send::send(&app_caller(&f.app), &json!({"to": "sink@attacker.test", "bcc": bcc, "subject": "s", "text": "t"}), now()).expect("30 recipients");
    let id = out["id"].as_str().expect("id").to_string();
    assert!(skin::remove_principal("sink").expect("remove"), "removed");
    let row = log_row(&id);
    assert_eq!(row.recipient_count, 30, "the count stays");
    assert!(!row.to.iter().any(|a| a.contains("sink@")), "the address is blanked: {:?}", row.to);
    assert_eq!(row.to[0], store::REMOVED_ADDRESS);
    // The App's any-scope budget (30/h) is still spent.
    skin::add_principal_for_app("sink", Some("sink@attacker.test"), &f.app, None, "test").expect("sink again");
    let e = send::send(&app_caller(&f.app), &json!({"to": "sink@attacker.test", "subject": "s", "text": "t"}), now()).expect_err("budget unchanged");
    assert_eq!(e.code, "rate_limited", "{e:?}");
    assert_eq!(f.engine.sent.lock().unwrap().len(), 1);
}

#[test]
fn h1_l2_a_removed_users_scheduled_send_is_cancelled_and_its_body_goes() {
    let f = fx("h1sched");
    on_app(&f, store::SCOPE_ANY);
    skin::add_principal_for_app("dana", Some("dana@x.test"), &f.app, None, "test").expect("dana");
    let now = now();
    let out = send::send(&app_caller(&f.app), &json!({"to": "dana@x.test", "subject": "s", "text": "later", "sendAt": now + 600}), now).expect("scheduled");
    let id = out["id"].as_str().expect("id").to_string();
    let file = send::scheduled_path(&id).expect("path");
    assert!(file.exists());
    assert!(skin::remove_principal("dana").expect("remove"));
    let row = log_row(&id);
    assert_eq!((row.status.as_str(), row.error_code.as_deref(), row.recipient_count), (store::ST_CANCELLED, Some("recipient_removed"), 1));
    send::tick(now + 601);
    assert!(!file.exists(), "the orphaned body is deleted");
    assert!(f.engine.sent.lock().unwrap().is_empty(), "a cancelled send never leaves");
}

#[test]
fn h2_provider_headers_never_reach_either_path() {
    let f = fx("h2");
    on_app(&f, store::SCOPE_ANY);
    for h in ["X-SMTPAPI", "X-MC-BccAddress", "X-Mailgun-Drop-Message", "X-SES-CONFIGURATION-SET", "X-PM-Message-Stream", "X-Order"] {
        let mut headers = serde_json::Map::new();
        headers.insert(h.to_string(), json!("{\"to\":[\"victim@x.test\"]}"));
        let v = json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": J::Object(headers)});
        assert_eq!(code(&send::send(&app_caller(&f.app), &v, now())), "header_not_allowed", "{h}");
    }
    assert!(f.engine.sent.lock().unwrap().is_empty());
    send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "t", "headers": {"X-App-Order": "1", "X-Entity-Ref-ID": "2"}}), now())
        .expect("the App's own headers pass");
}

#[test]
fn m1_piling_scheduled_sends_onto_one_time_cant_burst() {
    let f = fx("m1pile");
    let s = on_app(&f, store::SCOPE_ANY);
    let now = now();
    let t = now + 3 * 3600;
    let to: Vec<String> = (0..30).map(|i| format!("p{i}@x.test")).collect();
    let first = send::send(&app_caller(&f.app), &json!({"to": to, "subject": "s", "text": "t", "sendAt": t}), now).expect("30 at t");
    for at in [t, t - 1800, t + 1800, t + 3599] {
        let e = send::send(&app_caller(&f.app), &json!({"to": "one@x.test", "subject": "s", "text": "t", "sendAt": at}), now).expect_err("same hour");
        assert_eq!(e.code, "rate_limited", "sendAt {at}: {e:?}");
    }
    // Nor an immediate send within an hour of it.
    let e = send::send(&app_caller(&f.app), &json!({"to": "one@x.test", "subject": "s", "text": "t"}), t - 600).expect_err("now, near t");
    assert_eq!(e.code, "rate_limited");
    // Clear of it is fine; the hour it was made in isn't spent.
    send::send(&app_caller(&f.app), &json!({"to": "one@x.test", "subject": "s", "text": "t"}), now).expect("now is 3 h away");
    send::send(&app_caller(&f.app), &json!({"to": "two@x.test", "subject": "s", "text": "t", "sendAt": t + 3600}), now).expect("the next hour");
    // At flush time: a send that no longer fits (a row landed in its hour
    // meanwhile) waits instead of bursting.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let extra = vec!["late@x.test".to_string()];
        store::insert_log(
            &conn,
            &store::NewLog {
                sender_id: &s.id,
                origin: store::ORIGIN_APP,
                app_service_id: Some(&f.app),
                purpose: None,
                to: &extra,
                subject: "",
                tags: &[],
                status: store::ST_SUBMITTED,
                error_code: None,
                message_id: None,
                send_after: None,
                scope_any: true,
                now: t,
            },
        )
        .expect("a row in that hour");
    }
    assert_eq!(send::flush_due(t + 1, 10), 0, "deferred, not sent");
    let id = first["id"].as_str().expect("id");
    let row = log_row(id);
    assert_eq!((row.status.as_str(), row.send_after), (store::ST_SCHEDULED, Some(t + 1 + send::DEFER_SECS)));
    assert!(f.engine.sent.lock().unwrap().iter().all(|m| m.rcpt.len() == 1), "the 30 didn't leave yet");
    // Two hours on, its own hour is clear: it leaves then, and the next
    // due one (whose hour it now fills) waits in turn.
    assert_eq!(send::flush_due(t + 7201, 10), 1);
    assert_eq!(log_row(id).status, store::ST_SUBMITTED);
    assert!(f.engine.sent.lock().unwrap().iter().any(|m| m.rcpt.len() == 30));
}

#[test]
fn m1_an_app_keeps_at_most_a_days_budget_waiting() {
    let f = fx("m1pending");
    let s = on_app(&f, store::SCOPE_ANY);
    let now = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for d in 0..10i64 {
            let to: Vec<String> = (0..30).map(|i| format!("w{d}x{i}@x.test")).collect();
            store::insert_log(
                &conn,
                &store::NewLog {
                    sender_id: &s.id,
                    origin: store::ORIGIN_APP,
                    app_service_id: Some(&f.app),
                    purpose: None,
                    to: &to,
                    subject: "",
                    tags: &[],
                    status: store::ST_SCHEDULED,
                    error_code: None,
                    message_id: None,
                    send_after: Some(now + 400 * 86_400 + d * 3 * 86_400),
                    scope_any: true,
                    now,
                },
            )
            .expect("waiting");
        }
    }
    let e = send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "t", "sendAt": now + 600}), now).expect_err("300 waiting");
    assert_eq!(e.code, "rate_limited");
    assert!(e.hint.contains("waiting"), "{}", e.hint);
}

#[test]
fn m2_an_agent_cant_mint_itself_an_email_key() {
    let f = fx("m2mint");
    on_app(&f, store::SCOPE_ANY);
    agent_switches(&f.ws, true, true);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let r = crate::apps::grace::handle("/cli/app/key/mint", agent, json!({"app": f.app, "scopes": ["email"]}).to_string().as_bytes(), &HashMap::new());
    let v = body_of(&r);
    assert_eq!(v["status"], "held", "{v}");
    assert!(v["reasons"].to_string().contains("sends email"), "{v}");
    let keys: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT COUNT(*) FROM api_keys WHERE app_service_id = ?1", params![f.app], |r| r.get(0)).expect("count")
    };
    assert_eq!(keys, 0, "no key was minted");
}

#[test]
fn m2_a_runtime_app_sends_only_with_its_cap_approved() {
    let f = fx("m2cap");
    on_app(&f, store::SCOPE_ANY);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: "app_m2",
                service_id: &f.app,
                seed_id: None,
                owner_workspace: &f.ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 4350,
                created_by: "test",
            },
        )
        .expect("runtime row");
    }
    let key = key_for(&f.app, &["email"], None);
    let body = json!({"to": "a@x.test", "subject": "s", "text": "t"});
    let (st, v) = key_send(&f.app, &key, &body);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("cap_not_granted")), "{v}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::apps::grants::record(&conn, "app_m2", &[skin::CAP_EMAIL_SEND.to_string()]).expect("approve");
    }
    let (st, v) = key_send(&f.app, &key, &body);
    assert_eq!(st, 202, "{v}");
}

#[test]
fn m2_a_key_the_agent_minted_follows_its_send_setting() {
    let f = fx("m2gate");
    on_app(&f, store::SCOPE_ANY);
    let key = key_for(&f.app, &["email"], Some(&f.ws));
    let body = json!({"to": "a@x.test", "subject": "s", "text": "t"});
    let (st, v) = key_send(&f.app, &key, &body);
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("gated")), "agentSend off: {v}");
    approval_gate(&f.ws);
    let (st, v) = key_send(&f.app, &key, &body);
    assert_eq!((st, v["status"].clone()), (202, json!("held")), "{v}");
    assert!(f.engine.sent.lock().unwrap().is_empty(), "held for the owner");
}

#[test]
fn m3_a_decided_held_email_keeps_no_body_or_live_link() {
    use crate::mail::send::OutboundStore;
    let f = fx("m3");
    let s = on_app(&f, store::SCOPE_ANY);
    approval_gate(&f.ws);
    agent_switches(&f.ws, true, true);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let (_, v) = app_post(app_routes::SEND, &agent, json!({"app": f.app, "to": "a@x.test", "subject": "Held", "text": "BODY k2inv_live123", "html": "<b>HTML</b>"}));
    let approved = v["outboundId"].as_str().expect("held").to_string();
    let r = crate::mail::routes_send::handle_approvals_approve(json!({"id": approved}).to_string().as_bytes());
    assert_eq!(status_of(&r), 200, "{}", r.body);
    let store_ob = crate::mail::send::DbOutboundStore::default();
    let kept = store_ob.load_message(&approved).expect("kept");
    assert!(kept.text_body.is_empty() && kept.notify.as_ref().expect("notify").html.is_none(), "{kept:?}");
    assert!(!format!("{kept:?}").contains("k2inv_live123"));
    // An invite held, then denied: no live link at rest either.
    let c = send::Composed { subject: "Join".into(), text: "https://x.test/join?t=k2inv_deny456".into(), html: None };
    let held = send::app_link_mail(&s, &f.app, "invite", "new@x.test", &c, crate::mail::send::Gate::Approval, &f.ws, "agent:x")
        .expect("ok")
        .expect_err("held");
    crate::mail::send::deny(&store_ob, &held, "owner", "no", now()).expect("deny");
    let kept = store_ob.load_message(&held).expect("kept");
    assert!(!format!("{kept:?}").contains("k2inv_deny456"), "{kept:?}");
}

#[test]
fn m4_a_line_break_in_an_address_is_refused_everywhere() {
    let f = fx("m4");
    on_app(&f, store::SCOPE_ANY);
    for bad in ["a\r\nX-Injected: y@victim.test", "a b@victim.test", "<a>@victim.test", "\"a\"@victim.test"] {
        assert_eq!(code(&send::send(&app_caller(&f.app), &json!({"to": bad, "subject": "s", "text": "t"}), now())), "usage", "{bad:?}");
        assert_eq!(code(&send::send(&app_caller(&f.app), &json!({"to": "ok@x.test", "bcc": [bad], "subject": "s", "text": "t"}), now())), "usage", "{bad:?}");
    }
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    let c = send::Composed { subject: "Your code".into(), text: "1".into(), html: None };
    assert_eq!(send::system_send("email_code", "a\r\nb@x.test", &c, None).expect_err("refused").code, "usage");
    assert!(f.engine.sent.lock().unwrap().is_empty());
}

#[test]
fn l3_a_retired_senders_mailbox_is_emptied() {
    let f = fx("l3");
    let s = add_hosted("noreply@notify.brand.test");
    senders::remove(&s.id, "owner", now()).expect("rm");
    let now = now();
    sweep::hourly(now);
    let swept = f.engine.swept.lock().unwrap().clone();
    assert!(swept.contains(&(s.stalwart_account_id.clone().expect("acct"), now)), "{swept:?}");
}

#[test]
fn l4_mail_manage_agents_cant_reach_a_senders_domain_or_its_apps_gate() {
    let f = fx("l4");
    let _mail = crate::mail::mail_server_test_lock();
    on_app(&f, store::SCOPE_ANY);
    let owner_only =|r: &crate::cli_response::CliResponse| (status_of(r), body_of(r)["error"]["code"].clone()) == (403, json!("owner_only"));
    let r = as_it_agent(&f, || crate::mail::dkim::handle_dkim_rotate(json!({"domain": DOMAIN}).to_string().as_bytes()));
    assert!(owner_only(&r), "{}", r.body);
    let r = as_it_agent(&f, || crate::mail::dmarc::handle_dmarc_report_to(json!({"domain": DOMAIN, "address": "x@notify.brand.test"}).to_string().as_bytes()));
    assert!(owner_only(&r), "{}", r.body);
    let r = as_it_agent(&f, || crate::mail::ptr::handle_ptr_set(json!({"hostname": "mail.brand.test"}).to_string().as_bytes()));
    assert!(owner_only(&r), "{}", r.body);
    // The App workspace's agentSend: never loosened by an agent; narrowing is fine.
    let r = as_it_agent(&f, || crate::mail::routes_server::handle_config_set(json!({"workspace": f.ws, "agentSend": "on"}).to_string().as_bytes()));
    assert!(owner_only(&r), "{}", r.body);
    let r = as_it_agent(&f, || crate::mail::routes_server::handle_config_set(json!({"workspace": f.ws, "agentSend": "off"}).to_string().as_bytes()));
    assert!(!owner_only(&r), "{}", r.body);
    // A held App email isn't in an IT agent's approvals queue.
    approval_gate(&f.ws);
    agent_switches(&f.ws, true, true);
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let (_, v) = app_post(app_routes::SEND, &agent, json!({"app": f.app, "to": "a@x.test", "subject": "Private", "text": "PREVIEWME"}));
    let ob = v["outboundId"].as_str().expect("held").to_string();
    let r = as_it_agent(&f, || crate::mail::routes_send::handle_approvals_list(&HashMap::new()));
    assert!(!r.body.contains(&ob) && !r.body.contains("PREVIEWME"), "{}", r.body);
    let r = crate::mail::routes_send::handle_approvals_list(&HashMap::new());
    assert!(r.body.contains(&ob), "the owner still sees it: {}", r.body);
}

#[test]
fn l5_sender_set_needs_a_fresh_confirm() {
    let _f = fx("l5");
    let s = add_hosted("noreply@notify.brand.test");
    let post = |b: J| owner_routes::handle(owner_routes::SENDER_SET, Who::Owner, true, b.to_string().as_bytes(), &HashMap::new());
    let r = post(json!({"sender": s.id, "replyTo": "attacker@evil.test"}));
    let v = body_of(&r);
    assert_eq!((status_of(&r), v["error"]["code"].clone(), v["error"]["action"].clone()), (403, json!("vault_confirm_required"), json!(CONFIRM_SENDER)), "{v}");
    assert!(senders::find_live(&s.id).expect("s").reply_to.is_empty(), "nothing changed");
    let vault = crate::vault::ensure_test_vault();
    let tok = vault.confirm.mint(CONFIRM_SENDER, Some(&s.address), "owner", "local-auth");
    let r = post(json!({"sender": s.id, "replyTo": "help@brand.test", "confirmToken": tok}));
    assert_eq!(status_of(&r), 200, "{}", r.body);
    assert_eq!(status_of(&post(json!({"sender": s.id}))), 200, "no change, no confirm");
    assert_eq!(body_of(&post(json!({"sender": s.id, "name": "=?utf-8?B?eA==?="})))["error"]["code"], "usage");
}

#[test]
fn l6_from_name_spoofing_is_refused() {
    let f = fx("l6");
    on_app(&f, store::SCOPE_ANY);
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    for name in [" B r a n d ", "bRaNd", "=?utf-8?B?c2VjdXJpdHlAYmFuay5jb20=?="] {
        let v = json!({"to": "a@x.test", "subject": "s", "text": "t", "fromName": name});
        assert_eq!(code(&send::send(&app_caller(&f.app), &v, now())), "usage", "{name:?}");
    }
}

#[test]
fn l7_upstream_error_text_never_reaches_the_app() {
    let f = fx("l7");
    on_app(&f, store::SCOPE_ANY);
    *f.engine.fail_submit.lock().unwrap() = Some("550 quota for AKIASECRET at internal.host user@x.test".into());
    let e = send::send(&app_caller(&f.app), &json!({"to": "a@x.test", "subject": "s", "text": "t"}), now()).expect_err("refused");
    assert_eq!((e.status, e.code, e.hint.as_str()), (502, "failed", send::UPSTREAM_HINT));
    assert_eq!(send::mask_addresses("550 no user@x.test here"), "550 no [addr] here");
}

#[test]
fn l8_the_apps_log_never_shows_k2s_own_mail_about_its_users() {
    let f = fx("l8");
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    let c = send::Composed { subject: "Reset".into(), text: "link".into(), html: None };
    let id = send::system_send("owner_reset", "newaddr@x.test", &c, Some(&f.app)).expect("sent");
    assert_eq!(code(&send::status(&f.app, &id)), "not_found");
    let log = send::log(&f.app, None, None, None, None).expect("log");
    assert!(!log.to_string().contains("newaddr@x.test"), "{log}");
    let q: HashMap<String, String> = [("app".to_string(), f.app.clone())].into_iter().collect();
    let r = owner_routes::handle(owner_routes::LOG, Who::Owner, false, b"", &q);
    assert!(r.body.contains("owner_reset"), "the owner's log keeps it: {}", r.body);
}

#[test]
fn l9_off_is_exactly_today() {
    let f = fx("l9");
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    on_app(&f, store::SCOPE_ANY);
    set_test_switch(Some(false));
    // The owner's reset keeps today's answer (no notification path).
    assert!(crate::app_user_invites::owner_reset_sender(&f.app, None).map_err(|r| r.body).expect("ok").is_none());
    // The routes aren't served: the dispatcher falls through to its unknown-route answer.
    for p in [owner_routes::STATUS, owner_routes::SENDER_ADD, app_routes::SEND, link_routes::SEND] {
        assert!(!serves(p), "{p}");
    }
    // The key door answers an email verb exactly as it answers an unknown verb.
    let key = key_for(&f.app, &["email"], None);
    let body = json!({"to": "a@x.test", "subject": "s", "text": "t"}).to_string();
    let email = crate::apps::app_key_routes::handle(&format!("/v1/apps/{}/email/send", f.app), true, Some(&key), body.as_bytes(), &HashMap::new());
    let unknown = crate::apps::app_key_routes::handle(&format!("/v1/apps/{}/nope/send", f.app), true, Some(&key), body.as_bytes(), &HashMap::new());
    assert_eq!((email.status, email.content_type, email.body.clone()), (unknown.status, unknown.content_type, unknown.body.clone()));
    assert!(!crate::apps::app_key_routes::post_allowed(&format!("/v1/apps/{}/email/send", f.app)));
    assert_ne!(crate::apps::app_key_routes::body_cap(&format!("/v1/apps/{}/email/send", f.app)), MAX_BODY);
    assert!(f.engine.sent.lock().unwrap().is_empty());
    set_test_switch(None);
    assert!(serves(app_routes::SEND), "back on");
}

#[test]
fn l12_the_scheduled_body_is_0600_and_checked_before_it_is_read() {
    let f = fx("l12");
    on_app(&f, store::SCOPE_ANY);
    let now = now();
    let later = json!({"to": "a@x.test", "subject": "s", "text": "later", "sendAt": now + 600});
    // The widened-mode check is Unix's (Windows files are owner-only DACLs).
    #[cfg(unix)]
    let out = send::send(&app_caller(&f.app), &later, now).expect("scheduled");
    #[cfg(unix)]
    let id = out["id"].as_str().expect("id").to_string();
    #[cfg(unix)]
    let file = send::scheduled_path(&id).expect("path");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).expect("meta").permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(file.parent().expect("dir")).expect("meta").permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("widen");
        send::flush_due(now + 601, 10);
        assert!(f.engine.sent.lock().unwrap().is_empty(), "a widened file isn't trusted");
        assert_eq!(log_row(&id).error_code.as_deref(), Some("lost"));
    }
    // A tampered body (a provider header written in) is refused at flush.
    let out = send::send(&app_caller(&f.app), &later, now).expect("scheduled");
    let id = out["id"].as_str().expect("id").to_string();
    let file = send::scheduled_path(&id).expect("path");
    let raw = std::fs::read_to_string(&file).expect("read");
    assert!(raw.contains("\"headers\":{}"), "{raw}");
    std::fs::write(&file, raw.replace("\"headers\":{}", "\"headers\":{\"X-SMTPAPI\":\"{}\"}")).expect("tamper");
    send::flush_due(now + 601, 10);
    assert!(f.engine.sent.lock().unwrap().is_empty());
    assert_eq!(log_row(&id).error_code.as_deref(), Some("header_not_allowed"));
}

#[test]
fn l13_reply_to_uses_the_public_suffix_list() {
    assert_eq!(send::registrable("shop.github.io").as_deref(), Some("shop.github.io"));
    assert_eq!(send::registrable("a.brand.co.kr").as_deref(), Some("brand.co.kr"));
    assert_eq!(send::registrable("notify.brand.test").as_deref(), Some("brand.test"));
    assert_eq!(send::registrable("github.io"), None, "a public suffix has no registrable part");
}

// ── part 2, slice V: verified addresses ──────────────────────────────

fn person_call(path: &str, person: &str, role: Role, is_post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = verify::person_route(is_post, path, body.to_string().as_bytes(), person, role, &params);
    (status_of(&r), body_of(&r))
}

/// The `k2ev_…` / `k2es_…` token in the last mail sent.
fn token_in_last_mail(f: &Fx, prefix: &str) -> String {
    let sent = f.engine.sent.lock().unwrap().clone();
    let text = sent.last().expect("a mail").email.to_string();
    let at = text.find(prefix).unwrap_or_else(|| panic!("no {prefix} token in {text}"));
    text[at..].chars().take(prefix.len() + 43).collect()
}

fn system_sender_on() -> store::Sender {
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    senders::find_live(&sys.id).expect("live")
}

#[test]
fn v_a_notice_address_is_confirmed_by_post_only_and_any_stop_link_ends_it() {
    let f = fx("vaddr");
    use verify::{ADDRESS_PATH as SHOW, ADDRESS_SET_PATH as SET, STOP_PATH, VERIFY_PATH};
    k2_core::skin::set_front_door("direct", Some("https://box.brand.test"), None, None).expect("door");
    // No system sender: nothing is sent and no row is left.
    let (st, v) = person_call(SET, "owner", Role::Owner, true, json!({"address": "me@x.test"}), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("no_mail_path")), "{v}");
    let sys = system_sender_on();
    // A GET on a POST route is 405.
    let (st, _) = person_call(SET, "owner", Role::Owner, false, json!({}), &[]);
    assert_eq!(st, 405);
    let (st, v) = person_call(SET, "owner", Role::Owner, true, json!({"address": "Me@X.test"}), &[]);
    assert_eq!((st, v["address"]["state"].clone(), v["address"]["address"].clone()), (200, json!("pending"), json!("me@x.test")), "{v}");
    let mail = f.engine.sent.lock().unwrap().last().cloned().expect("sent");
    assert_eq!((mail.from.as_str(), mail.rcpt.clone()), (sys.address.as_str(), vec!["me@x.test".to_string()]));
    let text = mail.email.to_string();
    assert!(text.contains("https://box.brand.test/cli/email/verify?t=k2ev_"), "{text}");
    assert!(text.contains("https://box.brand.test/cli/email/stop?t=k2es_"), "{text}");
    assert!(text.contains("The owner of box.brand.test"), "who asked: {text}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let purpose: String = conn.query_row("SELECT purpose FROM email_notify_log ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).expect("log");
        assert_eq!(purpose, verify::PURPOSE_MAIL);
    }
    let tok = token_in_last_mail(&f, "k2ev_");
    let stop = token_in_last_mail(&f, "k2es_");
    // A GET (a link scanner) renders the page and never confirms.
    let p = verify::page_get(VERIFY_PATH, &format!("t={tok}"));
    assert!(p.status.starts_with("200") && p.html.contains("me@x.test") && p.html.contains("method=\"post\""), "{p:?}");
    assert!(!p.html.contains("<script"), "no script on the page");
    let raw = verify::page_response(&p);
    for h in ["Content-Security-Policy: default-src 'none';", "Cache-Control: no-store", "X-Frame-Options: DENY", "Referrer-Policy: no-referrer"] {
        assert!(raw.contains(h), "{h} in {raw}");
    }
    let (_, v) = person_call(SHOW, "owner", Role::Owner, false, json!({}), &[]);
    assert_eq!((v["address"].clone(), v["pending"][0]["address"].clone()), (J::Null, json!("me@x.test")), "still pending after a GET: {v}");
    // The POST confirms, once.
    let p = verify::page_post(VERIFY_PATH, "", format!("t={tok}").as_bytes());
    assert!(p.status.starts_with("200") && p.html.contains("Confirmed"), "{p:?}");
    let (_, v) = person_call(SHOW, "owner", Role::Owner, false, json!({}), &[]);
    assert_eq!(v["address"]["state"], "verified", "{v}");
    assert!(verify::page_post(VERIFY_PATH, "", format!("t={tok}").as_bytes()).status.starts_with("404"), "a used link");
    assert!(verify::page_get(VERIFY_PATH, "t=k2ev_<script>alert(1)</script>").status.starts_with("404"));
    // Asking again for a confirmed address sends nothing.
    let n = f.engine.sent.lock().unwrap().len();
    let (st, v) = person_call(SET, "owner", Role::Owner, true, json!({"address": "me@x.test"}), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("already_verified")), "{v}");
    assert_eq!(f.engine.sent.lock().unwrap().len(), n);
    // The stop page renders on GET; an RFC 8058 one-click POST stops.
    assert!(verify::page_get(STOP_PATH, &format!("t={stop}")).html.contains("Stop these emails"));
    let (_, v) = person_call(SHOW, "owner", Role::Owner, false, json!({}), &[]);
    assert_eq!(v["address"]["state"], "verified", "a GET never stops");
    let p = verify::page_post(STOP_PATH, &format!("t={stop}"), b"List-Unsubscribe=One-Click");
    assert!(p.status.starts_with("200") && p.html.contains("Stopped"), "{p:?}");
    let (_, v) = person_call(SHOW, "owner", Role::Owner, false, json!({}), &[]);
    assert_eq!(v["address"], J::Null, "{v}");
}

#[test]
fn v_people_read_and_remove_their_own_admins_anyones_and_senders_are_refused() {
    let f = fx("vwho");
    use verify::{ADDRESS_PATH as SHOW, ADDRESS_REMOVE_PATH as REMOVE, ADDRESS_SET_PATH as SET};
    k2_core::skin::set_front_door("direct", Some("https://box.brand.test"), None, None).expect("door");
    let sys = system_sender_on();
    let (st, v) = person_call(SET, "ann", Role::Member, true, json!({"address": sys.address}), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("sender_address")), "{v}");
    let (st, v) = person_call(SET, "ann", Role::Member, true, json!({"address": "ann@x.test"}), &[]);
    assert_eq!(st, 200, "{v}");
    assert!(f.engine.sent.lock().unwrap().last().expect("sent").email.to_string().contains("ann, a person on box.brand.test"));
    // A Member reads and removes only their own.
    let (st, _) = person_call(SHOW, "bob", Role::Member, false, json!({}), &[("person", "ann")]);
    assert_eq!(st, 403);
    let (st, _) = person_call(REMOVE, "bob", Role::Member, true, json!({"person": "ann"}), &[]);
    assert_eq!(st, 403);
    let (st, v) = person_call(SHOW, "ada", Role::Admin, false, json!({}), &[("person", "ann")]);
    assert_eq!((st, v["pending"][0]["address"].clone()), (200, json!("ann@x.test")), "{v}");
    let (st, v) = person_call(REMOVE, "ada", Role::Admin, true, json!({"person": "ann"}), &[]);
    assert_eq!((st, v["removed"].clone()), (200, json!(1)), "{v}");
    let (st, v) = person_call(REMOVE, "ann", Role::Member, true, json!({}), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (404, json!("not_found")), "{v}");
    // A failed send leaves no pending row behind.
    *f.engine.fail_submit.lock().unwrap() = Some("boom".into());
    let (st, _) = person_call(SET, "ann", Role::Member, true, json!({"address": "ann2@x.test"}), &[]);
    assert_ne!(st, 200);
    *f.engine.fail_submit.lock().unwrap() = None;
    let (_, v) = person_call(SHOW, "ann", Role::Member, false, json!({}), &[]);
    assert_eq!(v["pending"], json!([]), "{v}");
    // With no front door, nothing goes out.
    k2_core::skin::set_front_door("direct", None, None, None).expect("no door");
    let (st, v) = person_call(SET, "ann", Role::Member, true, json!({"address": "ann3@x.test"}), &[]);
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("no_front_door")), "{v}");
}

// ── part 2, N6: outcomes, suppressions, auto-pause ───────────────────

fn dsn_report(id: &str, envelope_id: &str, rcpt: &str, action: &str, status: &str) -> engine::RawReport {
    engine::RawReport {
        email_id: id.to_string(),
        kind: engine::ReportKind::Dsn,
        report: format!(
            "Reporting-MTA: dns; mail.brand.test\r\nOriginal-Envelope-Id: {envelope_id}\r\n\r\n\
             Final-Recipient: rfc822; {rcpt}\r\nAction: {action}\r\nStatus: {status}\r\n"
        ),
        original_headers: None,
    }
}

fn queue_reports(f: &Fx, s: &store::Sender, reports: Vec<engine::RawReport>) {
    f.engine.reports.lock().unwrap().insert(s.stalwart_account_id.clone().expect("acct"), reports);
}

#[test]
fn n6_a_dsn_records_the_bounce_suppresses_box_wide_and_forged_reports_change_nothing() {
    let f = fx("n6dsn");
    let s = on_app(&f, store::SCOPE_ANY);
    let out = send::send(&app_caller(&f.app), &json!({"to": ["gone@x.test", "ok@y.test"], "subject": "Hi", "text": "t"}), now()).expect("sent");
    let id = out["id"].as_str().expect("id").to_string();
    queue_reports(&f, &s, vec![
        dsn_report("m1", &id, "Gone@X.test", "failed", "5.1.1"),
        dsn_report("m2", &id, "ok@y.test", "delivered", "2.0.0"),
        // Forged: a send this sender never made, and a stranger on a real send.
        dsn_report("m3", "enl_forged", "victim@z.test", "failed", "5.1.1"),
        dsn_report("m4", &id, "victim@z.test", "failed", "5.1.1"),
    ]);
    let (read, recorded) = outcomes::pass(now());
    assert_eq!((read, recorded), (4, 2), "two real outcomes");
    let mut gone = f.engine.destroyed.lock().unwrap().clone();
    gone.sort();
    assert_eq!(gone, vec!["m1", "m2", "m3", "m4"], "every report read is destroyed");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::email_outcomes::suppressed_for(&conn, "gone@x.test", "another-sender", None).as_deref(), Some("hard_bounce"), "box-wide");
        assert_eq!(k2_core::email_outcomes::suppressed_for(&conn, "victim@z.test", &s.id, None), None, "a forged report suppresses nobody");
    }
    // What the App sees.
    let st = send::status(&f.app, &id).expect("status");
    assert_eq!(st["status"], "submitted", "the top-level status is unchanged");
    assert_eq!(st["recipients"], 2, "the count keeps its shape");
    assert_eq!(st["recipientStatus"], json!([{"to": "gone@x.test", "status": "bounced"}, {"to": "ok@y.test", "status": "delivered"}]));
    let log = send::log(&f.app, None, None, None, None).expect("log");
    assert_eq!((log["items"][0]["bounced"].clone(), log["items"][0]["complained"].clone()), (json!(1), json!(0)));
    // The next send drops the suppressed address; all suppressed sends nothing.
    let n = f.engine.sent.lock().unwrap().len();
    let out = send::send(&app_caller(&f.app), &json!({"to": ["gone@x.test", "ok@y.test"], "subject": "Again", "text": "t"}), now()).expect("sent");
    assert_eq!(f.engine.sent.lock().unwrap().last().expect("sent").rcpt, vec!["ok@y.test".to_string()]);
    let st = send::status(&f.app, out["id"].as_str().expect("id")).expect("status");
    assert_eq!(st["recipientStatus"][0], json!({"to": "gone@x.test", "status": "suppressed"}));
    let e = send::send(&app_caller(&f.app), &json!({"to": "gone@x.test", "subject": "x", "text": "t"}), now()).expect_err("all suppressed");
    assert_eq!((e.status, e.code), (409, "all_suppressed"));
    assert_eq!(f.engine.sent.lock().unwrap().len(), n + 1);
    // K2's own fixed mail to it isn't sent either, and answers the same as a sent one.
    let sys = system_sender_on();
    let c = send::Composed { subject: "Code".into(), text: "123".into(), html: None };
    let log_id = send::system_send_from(&sys, "email_code", None, "gone@x.test", &c).expect("same answer");
    assert_eq!(f.engine.sent.lock().unwrap().len(), n + 1, "nothing left the box");
    assert_eq!(log_row(&log_id).error_code.as_deref(), Some("all_suppressed"));
}

#[test]
fn n6_a_complaint_ties_to_its_send_by_message_id_and_suppresses_for_that_sender() {
    let f = fx("n6arf");
    let s = on_app(&f, store::SCOPE_ANY);
    let out = send::send(&app_caller(&f.app), &json!({"to": "angry@x.test", "subject": "Hi", "text": "t"}), now()).expect("sent");
    let row = log_row(out["id"].as_str().expect("id"));
    let mid = row.message_id.clone().expect("message id");
    queue_reports(&f, &s, vec![engine::RawReport {
        email_id: "a1".into(),
        kind: engine::ReportKind::Arf,
        report: "Feedback-Type: abuse\nVersion: 1\nOriginal-Rcpt-To: angry@x.test\n".into(),
        original_headers: Some(format!("From: x\nMessage-ID: <{}>\nSubject: Hi", mid.trim_matches(|c| c == '<' || c == '>'))),
    }]);
    assert_eq!(outcomes::pass(now()), (1, 1));
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::email_outcomes::suppressed_for(&conn, "angry@x.test", &s.id, None).as_deref(), Some("complaint"));
    assert_eq!(k2_core::email_outcomes::suppressed_for(&conn, "angry@x.test", "other", None), None, "only that sender");
}

/// `n` one-recipient submitted rows from `s`, the first `bounced` of them hard-bounced.
fn seed_rate(s: &store::Sender, app: &str, n: usize, bounced: usize) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for i in 0..n {
        let to = vec![format!("r{i}@x.test")];
        let id = store::insert_log(&conn, &store::NewLog {
            sender_id: &s.id, origin: store::ORIGIN_APP, app_service_id: Some(app), purpose: None, to: &to, subject: "", tags: &[],
            status: store::ST_SUBMITTED, error_code: None, message_id: None, send_after: None, scope_any: false, now: now(),
        }).expect("row");
        if i < bounced {
            k2_core::email_outcomes::record(&conn, &id, &s.id, &to[0], k2_core::email_outcomes::K_HARD, Some("5.1.1"), "dsn", now()).expect("outcome");
        }
    }
}

#[test]
fn n6_over_five_percent_hard_bounces_pauses_an_apps_sender_never_the_system_sender() {
    let f = fx("n6pause");
    let s = on_app(&f, store::SCOPE_ANY);
    seed_rate(&s, &f.app, 80, 4);
    assert_eq!(outcomes::evaluate(&s, now()), None, "5% is not over");
    seed_rate(&s, &f.app, 0, 0);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let to = vec!["r-extra@x.test".to_string()];
        let id = store::insert_log(&conn, &store::NewLog {
            sender_id: &s.id, origin: store::ORIGIN_APP, app_service_id: Some(&f.app), purpose: None, to: &to, subject: "", tags: &[],
            status: store::ST_SUBMITTED, error_code: None, message_id: None, send_after: None, scope_any: false, now: now(),
        }).expect("row");
        for k in 0..3 {
            k2_core::email_outcomes::record(&conn, &id, &s.id, &format!("b{k}@x.test"), k2_core::email_outcomes::K_HARD, None, "dsn", now()).expect("o");
        }
    }
    assert_eq!(outcomes::evaluate(&s, now()), Some("hard_bounces"), "7 of 81");
    let e = send::send(&app_caller(&f.app), &json!({"to": "z@x.test", "subject": "x", "text": "t"}), now()).expect_err("paused");
    assert_eq!((e.status, e.code, e.retry_after_ms), (423, "email_paused", None));
    let r = super::fail_response(&e);
    assert!(r.status.starts_with("423"), "{}", r.status);
    // K2's own mail for the App rides the system sender while it's paused (EN-A13).
    let sys = system_sender_on();
    assert_eq!(send::sender_for(Some(&f.app)).expect("ok").expect("a sender").id, sys.id);
    assert!(send::app_sender_if_on(&f.app).is_none());
    // Owner listings show it; resume needs the owner in person and a confirm bound to the address.
    let r = owner_routes::handle(owner_routes::SENDERS, Who::Owner, false, b"", &HashMap::new());
    let listed = body_of(&r)["senders"].as_array().expect("senders").iter().find(|x| x["id"] == s.id).cloned().expect("listed");
    assert_eq!(listed["paused"]["reason"], "hard_bounces", "{listed}");
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    let body = json!({"sender": s.address}).to_string();
    assert_eq!(body_of(&owner_routes::handle(owner_routes::SENDER_RESUME, admin, true, body.as_bytes(), &HashMap::new()))["error"]["code"], "owner_only");
    let r = owner_routes::handle(owner_routes::SENDER_RESUME, Who::Owner, true, body.as_bytes(), &HashMap::new());
    let v = body_of(&r);
    assert_eq!((v["error"]["action"].clone(), v["error"]["secretId"].clone()), (json!(CONFIRM_RESUME), json!(s.address)), "{v}");
    let vault = crate::vault::ensure_test_vault();
    let tok = vault.confirm.mint(CONFIRM_RESUME, Some(&s.address), "owner", "local-auth");
    let b = json!({"sender": s.address, "confirmToken": tok}).to_string();
    let r = owner_routes::handle(owner_routes::SENDER_RESUME, Who::Owner, true, b.as_bytes(), &HashMap::new());
    assert_eq!(status_of(&r), 200, "{}", r.body);
    send::send(&app_caller(&f.app), &json!({"to": "z@x.test", "subject": "x", "text": "t"}), now()).expect("sends again");
    assert_eq!(outcomes::evaluate(&s, now()), None, "a resume starts a new window");
    // The system sender is never paused, however bad its rate.
    seed_rate(&sys, &f.app, 60, 30);
    assert_eq!(outcomes::evaluate(&sys, now()), Some("hard_bounces"), "it trips");
    assert!(!outcomes::is_paused(&sys), "but only a warning");
    send::system_send_from(&sys, "email_code", None, "fine@x.test", &send::Composed { subject: "c".into(), text: "1".into(), html: None }).expect("still sends");
}

#[test]
fn n6_suppression_routes_are_hashed_admins_add_and_the_owner_removes() {
    let _f = fx("n6sup");
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    let post = |who: &Who, path: &str, body: J| {
        let r = owner_routes::handle(path, who.clone(), true, body.to_string().as_bytes(), &HashMap::new());
        (status_of(&r), body_of(&r))
    };
    let (st, v) = post(&admin, owner_routes::SUPPRESSION_ADD, json!({"address": "Bad@X.test"}));
    assert_eq!((st, v["scope"].clone()), (200, json!("box")), "{v}");
    let q: HashMap<String, String> = [("address".to_string(), "bad@x.test".to_string())].into_iter().collect();
    let r = owner_routes::handle(owner_routes::SUPPRESSIONS, admin.clone(), false, b"", &q);
    assert!(!r.body.contains("bad@x.test"), "never the address: {}", r.body);
    assert_eq!(body_of(&r)["suppressions"][0]["reason"], "manual");
    let (st, v) = post(&admin, owner_routes::SUPPRESSION_RM, json!({"address": "bad@x.test"}));
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("owner_only")), "removing widens: the owner's");
    let (st, v) = post(&Who::Owner, owner_routes::SUPPRESSION_RM, json!({"address": "bad@x.test"}));
    assert_eq!((st, v["removed"].clone()), (200, json!(1)), "{v}");
    let r = owner_routes::handle(owner_routes::SUPPRESSION_ADD, Who::Owner, false, b"", &HashMap::new());
    assert_eq!(status_of(&r), 405);
    let agent = Who::Agent { project_id: "ws".into(), session_id: "s".into() };
    assert_eq!(post(&agent, owner_routes::SUPPRESSION_ADD, json!({"address": "x@x.test"})).0, 403);
}

#[test]
fn n6_report_parts_read_only_the_report_and_never_walk_into_a_returned_message() {
    let get = json!({"list": [
        {"id": "e1", "bodyStructure": {"type": "multipart/report", "subParts": [
            {"type": "text/plain", "blobId": "b-text"},
            {"type": "message/delivery-status", "blobId": "b-dsn"},
            {"type": "text/rfc822-headers", "blobId": "b-hdr"}]}},
        {"id": "e2", "bodyStructure": {"type": "multipart/report", "subParts": [
            {"type": "message/feedback-report", "blobId": "b-arf"},
            {"type": "message/rfc822", "blobId": "b-orig", "subParts": [{"type": "message/delivery-status", "blobId": "b-inner"}]}]}},
        {"id": "e3", "bodyStructure": {"type": "text/plain", "blobId": "b-plain"}}
    ]});
    let parts = crate::mail::jmap::report_parts_from_get(&get);
    assert_eq!(parts.len(), 2, "a plain reply to the sender is left for the sweep");
    assert_eq!(parts[0].1, vec![("message/delivery-status".to_string(), "b-dsn".to_string()), ("text/rfc822-headers".to_string(), "b-hdr".to_string())]);
    assert!(!parts[1].1.iter().any(|(_, b)| b == "b-inner"), "never inside the returned message");
    assert_eq!(engine::header_block(b"Message-ID: <a@b>\r\nX: y\r\n\r\nsecret body"), "Message-ID: <a@b>\nX: y");
}

// ── part 2, N7: send-only apex senders ───────────────────────────────

fn add_via(addr: &str, via: &str) -> R<J> {
    senders::add(&senders::AddArgs { address: addr.to_string(), name: Some("Brand".into()), via: Some(via.to_string()), ..Default::default() }, "owner", now())
}

#[test]
fn n7_a_bare_domain_sends_through_its_hosted_bounce_subdomain_and_k2_stamps_the_from() {
    let f = fx("n7send");
    // brand.test's mail is elsewhere (not hosted here); notify.brand.test is.
    let v = add_via("noreply@brand.test", DOMAIN).expect("added");
    let s = senders::find_live(v["sender"]["id"].as_str().expect("id")).expect("live");
    assert_eq!((s.address.as_str(), s.envelope(), s.domain.as_str()), ("noreply@brand.test", "noreply@notify.brand.test", "brand.test"));
    assert_eq!(s.state, store::STATE_READY, "brand.test's p=reject is relaxed: it aligns");
    assert_eq!(v["sender"]["envelope"], "noreply@notify.brand.test");
    assert_eq!(f.engine.created.lock().unwrap().last().cloned(), Some(("noreply".to_string(), "stw-notify".to_string())), "the account is on the bounce subdomain");
    // Both addresses are fenced from mailboxes and other senders.
    assert!(fence::reserved("noreply@brand.test") && fence::reserved("noreply@notify.brand.test"));
    assert_eq!(code(&senders::add(&senders::AddArgs { address: "noreply@notify.brand.test".into(), ..Default::default() }, "owner", now())), "exists");
    // A send: the envelope is the bounce account, the header From the apex.
    grant(&f.app, &s, store::SCOPE_ANY);
    send::send(&app_caller(&f.app), &json!({"to": "c@x.test", "subject": "Hi", "text": "t"}), now()).expect("sent");
    let m = f.engine.sent.lock().unwrap().last().cloned().expect("sent");
    assert_eq!(m.from, "noreply@notify.brand.test", "Identity and MAIL FROM");
    assert_eq!(m.email["from"][0]["email"], "noreply@brand.test", "the From K2 stamps");
    // The domain carries a sender for the L10 parent-zone rule.
    assert!(fence::domain_carries_sender(DOMAIN));
}

#[test]
fn n7_strict_dmarc_on_both_a_hosted_apex_and_a_stray_via_are_refused() {
    let f = fx("n7strict");
    f.dns.set("_dmarc.brand.test", "v=DMARC1; p=reject; adkim=s; aspf=s");
    let e = add_via("noreply@brand.test", DOMAIN).expect_err("strict");
    assert_eq!((e.status, e.code), (409, "dmarc_strict"));
    assert!(f.engine.created.lock().unwrap().is_empty(), "nothing created");
    f.dns.set("_dmarc.brand.test", "v=DMARC1; p=reject; adkim=s");
    let v = add_via("noreply@brand.test", DOMAIN).expect("one relaxed is enough");
    // Turned strict later: the sender stops being ready.
    f.dns.set("_dmarc.brand.test", "v=DMARC1; p=reject; adkim=s; aspf=s");
    let r = senders::recheck(v["sender"]["id"].as_str().expect("id"), now()).expect("recheck");
    assert_eq!(r["sender"]["notReady"]["code"], "dmarc_strict", "{r}");
    // --via must be a subdomain of the address's domain, and a hosted apex doesn't need it.
    assert_eq!(code(&add_via("a@brand.test", "bounce.other.test")), "usage");
    assert_eq!(code(&add_via("b@notify.brand.test", "x.notify.brand.test")), "apex_hosted");
    // The bounce subdomain must be hosted and Verified.
    assert_eq!(code(&add_via("c@brand.test", "bounce.brand.test")), "domain_not_ready");
}

// ── part 2, N10a: agent email notices ────────────────────────────────

/// The owner confirms `addr` as their notice address (slice V's store).
fn owner_notice_address(addr: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let i = k2_core::email_verify::issue(&conn, k2_core::email_verify::PURPOSE_PERSON_NOTICE, "owner", addr, "owner", now()).expect("issue");
    k2_core::email_verify::confirm(&conn, &i.token, now()).expect("confirm");
}

fn agent_send(who: &Who, body: J) -> (u16, J) {
    let r = agent_notices::handle(agent_notices::SEND, who.clone(), true, body.to_string().as_bytes());
    (status_of(&r), body_of(&r))
}

#[test]
fn n10_an_agent_emails_a_confirmed_person_from_its_own_sender_once_the_owner_turns_it_on() {
    let f = fx("n10send");
    k2_core::skin::set_front_door("direct", Some("https://box.brand.test"), None, None).expect("door");
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let note = json!({"to": "owner", "subject": "Nightly build failed", "text": "The 02:00 build failed at step 3."});
    // Off by default.
    let (st, v) = agent_send(&agent, note.clone());
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("agent_email_off")), "{v}");
    // The owner grants a sender of the workspace's own, with a confirm bound to both.
    let s = add_hosted("notices@notify.brand.test");
    let body = json!({"workspace": f.ws, "sender": s.address}).to_string();
    let r = owner_routes::handle(owner_routes::AGENT_ON, Who::Owner, true, body.as_bytes(), &HashMap::new());
    let v = body_of(&r);
    let bind = agent_notices::on_binding(&f.ws, &s.address);
    assert_eq!((v["error"]["action"].clone(), v["error"]["secretId"].clone()), (json!(CONFIRM_AGENT_ON), json!(bind)), "{v}");
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    assert_eq!(body_of(&owner_routes::handle(owner_routes::AGENT_ON, admin, true, body.as_bytes(), &HashMap::new()))["error"]["code"], "owner_only");
    let tok = crate::vault::ensure_test_vault().confirm.mint(CONFIRM_AGENT_ON, Some(&bind), "owner", "local-auth");
    let b = json!({"workspace": f.ws, "sender": s.address, "confirmToken": tok}).to_string();
    let r = owner_routes::handle(owner_routes::AGENT_ON, Who::Owner, true, b.as_bytes(), &HashMap::new());
    assert_eq!(status_of(&r), 200, "{}", r.body);
    // The workspace's sender can't also be an App's or the system's.
    let tok = crate::vault::ensure_test_vault().confirm.mint(CONFIRM_APP_ON, Some(&app_binding(&f.app, &s.address)), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address, "confirmToken": tok}));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("sender_in_use")), "{v}");
    assert_eq!(code(&senders::set_system(&s.id, "owner", now())), "sender_in_use");
    // No confirmed notice address yet.
    let (st, v) = agent_send(&agent, note.clone());
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("no_notice_address")), "{v}");
    owner_notice_address("me@x.test");
    let (st, v) = agent_send(&agent, note.clone());
    assert_eq!((st, v["to"].clone()), (202, json!("the owner")), "{v}");
    let m = f.engine.sent.lock().unwrap().last().cloned().expect("sent");
    assert_eq!((m.from.as_str(), m.rcpt.clone()), (s.address.as_str(), vec!["me@x.test".to_string()]));
    let text = m.email.to_string();
    assert!(text.contains("The 02:00 build failed") && text.contains("an AI agent on box.brand.test"), "{text}");
    assert!(text.contains("https://box.brand.test/cli/email/stop?t=k2es_"), "the stop link: {text}");
    assert_eq!(m.email["header:List-Unsubscribe-Post:asText"], "List-Unsubscribe=One-Click");
    assert!(m.email["header:List-Unsubscribe:asText"].as_str().is_some_and(|h| h.starts_with("<https://box.brand.test/cli/email/stop?t=k2es_")));
    assert!(m.email["htmlBody"].is_null(), "text only");
    // The log row carries the workspace.
    let id = v["id"].as_str().expect("id");
    let row = log_row(id);
    assert_eq!((row.origin.as_str(), row.purpose.as_deref()), ("agent", Some(store::PURPOSE_AGENT_NOTICE)));
    // Refusals.
    for (body, code_) in [
        (json!({"to": "owner", "subject": "x", "text": "t", "html": "<b>"}), "usage"),
        (json!({"to": "me@x.test", "subject": "x", "text": "t"}), "person_only"),
        (json!({"to": "nobody", "subject": "x", "text": "t"}), "no_such_person"),
        (json!({"to": "owner", "subject": "", "text": "t"}), "usage"),
        (json!({"to": "owner", "subject": "x", "text": "x".repeat(16 * 1024 + 1)}), "usage"),
    ] {
        assert_eq!(agent_send(&agent, body.clone()).1["error"]["code"], code_, "{body}");
    }
    let (st, v) = agent_send(&Who::Owner, note.clone());
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("agent_only")), "{v}");
    let stranger = Who::Agent { project_id: f.other_ws.clone(), session_id: "s".into() };
    assert_eq!(agent_send(&stranger, note.clone()).1["error"]["code"], "agent_email_off", "another workspace has its own switch");
    // The budget: 10 an hour per workspace.
    for _ in 0..9 {
        assert_eq!(agent_send(&agent, note.clone()).0, 202);
    }
    let (st, v) = agent_send(&agent, note.clone());
    assert_eq!((st, v["error"]["code"].clone()), (429, json!("rate_limited")), "{v}");
}

#[test]
fn n10_a_person_mutes_a_workspace_and_the_agent_can_turn_its_own_notices_off() {
    let f = fx("n10mute");
    k2_core::skin::set_front_door("direct", Some("https://box.brand.test"), None, None).expect("door");
    let s = add_hosted("notices@notify.brand.test");
    agent_notices::agent_on(&f.ws, &s, "owner", now()).expect("on");
    owner_notice_address("me@x.test");
    let agent = Who::Agent { project_id: f.ws.clone(), session_id: "s".into() };
    let note = json!({"to": "owner", "subject": "Hi", "text": "t"});
    assert_eq!(agent_send(&agent, note.clone()).0, 202);
    // The owner mutes this workspace (a person route, never an agent's).
    let (st, v) = person_call(agent_notices::MUTE, "owner", Role::Owner, true, json!({"workspace": f.ws, "muted": true}), &[]);
    assert_eq!((st, v["muted"].clone()), (200, json!(true)), "{v}");
    let (_, v) = person_call(verify::ADDRESS_PATH, "owner", Role::Owner, false, json!({}), &[]);
    assert_eq!(v["mutedWorkspaces"], json!([f.ws]));
    let (st, v) = agent_send(&agent, note.clone());
    assert_eq!((st, v["error"]["code"].clone()), (403, json!("person_muted")), "{v}");
    person_call(agent_notices::MUTE, "owner", Role::Owner, true, json!({"workspace": f.ws, "muted": false}), &[]);
    assert_eq!(agent_send(&agent, note.clone()).0, 202);
    // The agent turns its own off; only the owner turns it back on.
    let r = agent_notices::handle(agent_notices::OFF, agent.clone(), true, b"{}");
    assert_eq!(status_of(&r), 200, "{}", r.body);
    assert_eq!(agent_send(&agent, note.clone()).1["error"]["code"], "agent_email_off");
    let r = agent_notices::handle(agent_notices::OFF, agent.clone(), false, b"");
    assert_eq!(status_of(&r), 405);
    // The passport allowlist: the agent verbs yes, a person's address never.
    assert!(crate::session_token::is_agent_verb(agent_notices::SEND) && crate::session_token::is_agent_verb(agent_notices::OFF));
    for p in [verify::ADDRESS_SET_PATH, verify::ADDRESS_PATH, agent_notices::MUTE, owner_routes::AGENT_ON] {
        assert!(!crate::session_token::is_agent_verb(p), "{p}");
    }
}

// ── part 2, N6: the mail server's delivery webhook (EN-A25, EN-A27) ──

fn hook_body(events: J) -> Vec<u8> {
    json!({ "events": events }).to_string().into_bytes()
}

fn hook_head(sig: Option<&str>) -> String {
    match sig {
        Some(s) => format!("POST /cli/email/events HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Signature: {s}\r\nContent-Type: application/json\r\n\r\n"),
        None => "POST /cli/email/events HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_string(),
    }
}

fn hook_post(key: &str, events: J) -> (u16, J) {
    let body = hook_body(events);
    let r = webhook::handle(true, &hook_head(Some(&webhook::sign(key, &body))), &body);
    (status_of(&r), body_of(&r))
}

struct HookKeyReset;
impl Drop for HookKeyReset {
    fn drop(&mut self) {
        webhook::set_test_key(None);
        set_test_switch(None);
    }
}

#[test]
fn n6_the_webhook_links_a_queued_send_once_and_records_delivered_and_delayed() {
    let f = fx("n6hook");
    let _reset = HookKeyReset;
    webhook::set_test_key(Some(Some("hook-key".into())));
    let s = on_app(&f, store::SCOPE_ANY);
    let out = send::send(&app_caller(&f.app), &json!({"to": ["a@x.test", "b@y.test"], "subject": "Hi", "text": "t"}), now()).expect("sent");
    let id = out["id"].as_str().expect("id").to_string();
    let qid: u64 = 337364143281548288;
    f.engine.queued.lock().unwrap().insert(webhook::registry_id(qid), id.clone());
    let batch = json!([
        {"id": "1", "type": webhook::QUEUED, "data": {"queueId": qid, "from": s.envelope(), "to": ["a@x.test", "b@y.test"]}},
        {"id": "2", "type": "delivery.delivered", "data": {"queueId": qid, "to": "A@x.test", "code": 250}},
        {"id": "3", "type": "delivery.rcpt-to-rejected", "data": {"queueId": qid, "to": "b@y.test", "code": 451}},
        // A recipient the send never had changes nothing.
        {"id": "4", "type": "delivery.delivered", "data": {"queueId": qid, "to": "stranger@z.test", "code": 250}},
    ]);
    let (st, v) = hook_post("hook-key", batch.clone());
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["linked"].clone(), v["recorded"].clone()), (json!(1), json!(2)));
    assert_eq!(*f.engine.queue_lookups.lock().unwrap(), vec!["jluo27qealaa".to_string()], "one lookup, by the registry id");
    let status = send::status(&f.app, &id).expect("status");
    assert_eq!(status["status"], "submitted", "the top-level status keeps its meaning");
    assert_eq!(status["recipientStatus"], json!([{"to": "a@x.test", "status": "delivered"}, {"to": "b@y.test", "status": "delayed"}]));
    // The same batch again (the mail server retries): no new lookup, nothing new.
    let (_, v) = hook_post("hook-key", batch);
    assert_eq!((v["linked"].clone(), v["recorded"].clone()), (json!(0), json!(0)));
    assert_eq!(f.engine.queue_lookups.lock().unwrap().len(), 1);
    // A 5xx rejection is the delivery report's to tell, never a bounce from here.
    let (_, v) = hook_post("hook-key", json!([{"type": "delivery.rcpt-to-rejected", "data": {"queueId": qid, "to": "b@y.test", "code": 550}}]));
    assert_eq!(v["recorded"], 0);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::email_outcomes::suppressed_for(&conn, "b@y.test", &s.id, None), None, "the webhook never suppresses");
    }
    // Mail K2 didn't send (another return path) is never looked up.
    let (_, v) = hook_post("hook-key", json!([{"type": webhook::QUEUED, "data": {"queueId": 5, "from": "person@notify.brand.test", "to": ["c@x.test"]}}]));
    assert_eq!(v["linked"], 0);
    assert_eq!(f.engine.queue_lookups.lock().unwrap().len(), 1, "no lookup for mail that isn't K2's");
    // A K2 send that already left the queue stays submitted: no guess.
    let out = send::send(&app_caller(&f.app), &json!({"to": "d@x.test", "subject": "Fast", "text": "t"}), now()).expect("sent");
    let fast = out["id"].as_str().expect("id").to_string();
    let (_, v) = hook_post(
        "hook-key",
        json!([
            {"type": webhook::QUEUED, "data": {"queueId": 6, "from": s.envelope(), "to": ["d@x.test"]}},
            {"type": "delivery.delivered", "data": {"queueId": 6, "to": "d@x.test", "code": 250}},
        ]),
    );
    assert_eq!((v["linked"].clone(), v["recorded"].clone()), (json!(0), json!(0)));
    assert_eq!(send::status(&f.app, &fast).expect("status")["recipientStatus"], json!([{"to": "d@x.test", "status": "submitted"}]));
    // An envelope id that is another sender's send is never kept for this one.
    let sys = system_sender_on();
    let c = send::Composed { subject: "Code".into(), text: "123".into(), html: None };
    let sys_log = send::system_send_from(&sys, "email_code", None, "e@x.test", &c).expect("sent");
    f.engine.queued.lock().unwrap().insert(webhook::registry_id(7), sys_log);
    let (_, v) = hook_post("hook-key", json!([{"type": webhook::QUEUED, "data": {"queueId": 7, "from": s.envelope(), "to": ["e@x.test"]}}]));
    assert_eq!(v["linked"], 0, "the envelope id must be this sender's own send");
}

#[test]
fn n6_the_webhook_answers_only_on_loopback_with_a_valid_signature_and_the_switch_on() {
    let _f = fx("n6hookauth");
    let _reset = HookKeyReset;
    webhook::set_test_key(Some(Some("hook-key".into())));
    let body = hook_body(json!([]));
    let good = webhook::sign("hook-key", &body);
    let ok = webhook::handle(true, &hook_head(Some(&good)), &body);
    assert_eq!(status_of(&ok), 200);
    let r = webhook::handle(false, &hook_head(Some(&good)), &body);
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (403, json!("forbidden")), "never off this machine");
    let r = webhook::handle(true, &hook_head(None), &body);
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (401, json!("bad_signature")));
    let r = webhook::handle(true, &hook_head(Some(&webhook::sign("another-key", &body))), &body);
    assert_eq!(status_of(&r), 401, "another key");
    let r = webhook::handle(true, &hook_head(Some(&good)), br#"{"events":[{"type":"delivery.delivered"}]}"#);
    assert_eq!(status_of(&r), 401, "the signature covers the body");
    let junk = b"not json".to_vec();
    let r = webhook::handle(true, &hook_head(Some(&webhook::sign("hook-key", &junk))), &junk);
    assert_eq!(status_of(&r), 400);
    webhook::set_test_key(Some(None));
    let r = webhook::handle(true, &hook_head(Some(&good)), &body);
    assert_eq!((status_of(&r), body_of(&r)["error"]["code"].clone()), (503, json!("not_ready")), "no key yet");
    webhook::set_test_key(Some(Some("hook-key".into())));
    set_test_switch(Some(false));
    let r = webhook::handle(true, &hook_head(Some(&good)), &body);
    assert_eq!(status_of(&r), 404, "with the box switch off it is an unknown route");
}
