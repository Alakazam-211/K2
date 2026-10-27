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
    assert_eq!((st, v["error"]["action"].clone(), v["error"]["serviceId"].clone()), (403, json!(CONFIRM_APP_ON), json!(f.app)), "{v}");
    let vault = crate::vault::ensure_test_vault();
    let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&f.app), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address, "scope": "any", "confirmToken": tok}));
    assert_eq!((st, v["error"]["action"].clone()), (403, json!(CONFIRM_APP_ANY)), "any asks again: {v}");
    let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&f.app), "owner", "local-auth");
    let any = vault.confirm.mint(CONFIRM_APP_ANY, Some(&f.app), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.app, "sender": s.address, "scope": "any", "confirmToken": tok, "confirmAnyToken": any}));
    assert_eq!((st, v["on"].clone(), v["recipientScope"].clone()), (200, json!(true), json!("any")), "{v}");
    // The App's agent may turn it off, never on.
    let (st, v) = app_post(app_routes::OFF, &agent, json!({"app": f.app}));
    assert_eq!((st, v["on"].clone()), (200, json!(false)), "{v}");
    // The system sender is never granted.
    let sys = add_hosted("system@notify.brand.test");
    senders::set_system(&sys.id, "owner", now()).expect("system");
    let tok = vault.confirm.mint(CONFIRM_APP_ON, Some(&f.other_app), "owner", "local-auth");
    let (st, v) = app_post(app_routes::ON, &Who::Owner, json!({"app": f.other_app, "sender": sys.address, "confirmToken": tok}));
    assert_eq!((st, v["error"]["code"].clone()), (409, json!("system_sender")), "{v}");
    // The confirm sheet names the App from the daemon's record.
    let reason = confirm_reason(CONFIRM_APP_ANY, Some(&f.app)).expect("ours").expect("an App");
    assert!(reason.contains("shop") && reason.contains("ANY"), "{reason}");
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
    let out = send::send(&app_caller(&f.app), &later, now).expect("scheduled");
    let id = out["id"].as_str().expect("id").to_string();
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
