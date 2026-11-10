//! Refreshing sign-ins: everything against a stub token endpoint and a local
//! stub upstream. No real provider is called.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::inject::{ClientAuth, Inject, Sign};
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::vault::SecretRequest;
use k2_vault::{Actor, AgentActor, AuditQuery, Delivery, KeyStore, Kind, Namespace, OpenOptions, PolicyPatch, PutMode, PutRequest, Scope, SecretStore};
use serde_json::json;

use super::*;
use crate::mail::oauth::{EgressHttp, HttpResponse};
use crate::vault::actor::{Channel, VaultCaller};

pub(crate) const OLD_ACCESS: &str = "k2test_oldaccess_0123456789abcdef";
pub(crate) const OLD_REFRESH: &str = "k2test_oldrefresh_0123456789abcdef";
pub(crate) const NEW_ACCESS: &str = "k2test_newaccess_fedcba9876543210";
pub(crate) const NEW_REFRESH: &str = "k2test_newrefresh_fedcba9876543210";
pub(crate) const CLIENT_SECRET: &str = "k2test_clientsecret_55aa55aa55aa";
const ALL_TOKENS: &[&str] = &[OLD_ACCESS, OLD_REFRESH, NEW_ACCESS, NEW_REFRESH, CLIENT_SECRET];
const WS: &str = "ws-oauth";

/// A token endpoint: counts POSTs, remembers the forms, answers from a fn.
pub(crate) struct StubToken {
    calls: AtomicUsize,
    delay_ms: u64,
    forms: Mutex<Vec<Vec<(String, String)>>>,
    basic: Mutex<Vec<(String, String)>>,
    answer: Box<dyn Fn(usize) -> (u16, String) + Send + Sync>,
}

impl StubToken {
    pub(crate) fn new(answer: impl Fn(usize) -> (u16, String) + Send + Sync + 'static) -> StubToken {
        StubToken { calls: AtomicUsize::new(0), delay_ms: 0, forms: Mutex::new(vec![]), basic: Mutex::new(vec![]), answer: Box::new(answer) }
    }

    /// Answers one new access token per POST.
    pub(crate) fn rotating() -> StubToken {
        StubToken::new(|_| (200, json!({ "access_token": NEW_ACCESS, "refresh_token": NEW_REFRESH, "expires_in": 3600 }).to_string()))
    }

    fn slow(mut self) -> StubToken {
        self.delay_ms = 150;
        self
    }

    pub(crate) fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn reply(&self, form: &[(&str, &str)]) -> Result<HttpResponse, OauthError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.forms.lock().push(form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        if self.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
        }
        let (status, body) = (self.answer)(n);
        Ok(HttpResponse { status, body })
    }
}

impl HttpClient for StubToken {
    fn post_form(&self, _url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, OauthError> {
        self.reply(form)
    }

    fn post_form_basic(&self, _url: &str, form: &[(&str, &str)], id: &str, secret: &str) -> Result<HttpResponse, OauthError> {
        self.basic.lock().push((id.to_string(), secret.to_string()));
        self.reply(form)
    }
}

struct Fixture {
    _t: TestDir,
    v: Arc<LocalVault>,
    cards: AtomicUsize,
}

fn fixture(tag: &str) -> Fixture {
    let t = TestDir::new(tag);
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(t.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    Fixture { _t: t, v, cards: AtomicUsize::new(0) }
}

pub(crate) fn bundle(expires_at: i64) -> SecretBytes {
    SecretBytes::from_string(
        json!({ "access_token": OLD_ACCESS, "refresh_token": OLD_REFRESH, "expires_at": expires_at, "client_id": "client-1", "client_secret": CLIENT_SECRET }).to_string(),
    )
}

fn put_with(f: &Fixture, name: &str, token_url: &str, auth: ClientAuth, hosts: Vec<String>, expires_in: i64) -> SecretMeta {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(WS), name, Kind::OauthToken);
    r.hosts = hosts;
    r.inject = Some(Inject { sign: Some(Sign::OauthRefresh { token_url: token_url.into(), client_auth: auth }), ..Inject::bearer() });
    f.v.put(r, bundle(unix_now() + expires_in), &Actor::Owner).expect("put").meta
}

fn put(f: &Fixture, name: &str, expires_in: i64) -> SecretMeta {
    put_with(f, name, "https://login.example.com/token", ClientAuth::Post, vec!["api.example.com".into()], expires_in)
}

/// A reconnect "card": a real pending request row, as the Thread path makes.
fn card_for(f: &Fixture) -> impl Fn(&str, &str, &SecretMeta) -> Result<(), String> + Sync + '_ {
    move |ws: &str, scope: &str, m: &SecretMeta| {
        f.cards.fetch_add(1, Ordering::SeqCst);
        f.v.request_create(SecretRequest {
            card_id: format!("card-{}", uuid::Uuid::new_v4().simple()),
            conversation: "conv".into(),
            session_id: None,
            workspace_id: ws.to_string(),
            requested_by: "k2".into(),
            name: m.name.clone(),
            kind: Kind::OauthToken,
            proposed_scope: scope.to_string(),
            proposed_hosts: m.hosts.clone(),
            proposed_delivery: Delivery::Fetch,
            prompt: String::new(),
            ttl_secs: None,
            status: String::new(),
            answered_by: None,
            secret_id: None,
            created_at: String::new(),
            answered_at: None,
            inject: None,
        })
        .map(|_| ())
        .map_err(|e| e.code.to_string())
    }
}

fn no_tokens_in(what: &str, text: &str) {
    for t in ALL_TOKENS {
        assert!(!text.contains(t), "{what} holds a token");
    }
}

fn no_tokens_in_logs() {
    let lines = test_log_lines().lock().join("\n");
    no_tokens_in("the log", &lines);
}

fn no_tokens_in_audit(f: &Fixture, id: &str) {
    f.v.flush_rollups();
    let rows = f.v.audit(&AuditQuery { secret_id: Some(id.to_string()), ..Default::default() }, &Actor::Owner).expect("audit");
    no_tokens_in("the audit", &serde_json::to_string(&rows).expect("json"));
}

fn audit_outcomes(f: &Fixture, id: &str) -> Vec<(String, u64)> {
    f.v.flush_rollups();
    f.v.audit(&AuditQuery { secret_id: Some(id.to_string()), ..Default::default() }, &Actor::Owner)
        .expect("audit")
        .into_iter()
        .filter(|r| r.action == "refresh")
        .map(|r| (r.outcome, r.count as u64))
        .collect()
}

macro_rules! deps {
    ($f:expr, $http:expr, $card:ident) => {
        let $card = card_for($f);
        let $card = Deps { http: &$http, asker_workspace: WS, post_card: &$card };
    };
}

fn run_parallel(n: usize, work: impl Fn() + Sync) {
    let gate = Barrier::new(n);
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                gate.wait();
                work();
            });
        }
    });
}

#[test]
fn a_fresh_token_makes_no_http_call() {
    let f = fixture("o-fresh");
    let m = put(&f, "A", 3600);
    let http = StubToken::rotating();
    deps!(&f, http, d);
    let t = access_token(&f.v, &m.id, &d).expect("token");
    assert_eq!(t.expose(), OLD_ACCESS.as_bytes());
    assert_eq!(http.count(), 0);
}

/// The concurrency rule: 8 parallel uses of a token near expiry are one POST.
#[test]
fn eight_parallel_uses_refresh_once() {
    let f = fixture("o-parallel");
    let m = put(&f, "A", 60);
    let http = StubToken::rotating().slow();
    deps!(&f, http, d);
    let got = Mutex::new(Vec::new());
    run_parallel(8, || {
        let t = access_token(&f.v, &m.id, &d).expect("token");
        got.lock().push(t.expose().to_vec());
    });
    assert_eq!(http.count(), 1, "single-flight: exactly one POST");
    let got = got.into_inner();
    assert_eq!(got.len(), 8);
    assert!(got.iter().all(|t| t == NEW_ACCESS.as_bytes()));
    // And it stays fresh.
    assert_eq!(access_token(&f.v, &m.id, &d).expect("token").expose(), NEW_ACCESS.as_bytes());
    assert_eq!(http.count(), 1);
}

#[test]
fn a_rotated_refresh_token_is_kept_and_one_left_out_is_too() {
    let f = fixture("o-rotate");
    let m = put(&f, "A", 10);
    let http = StubToken::rotating();
    deps!(&f, http, d);
    access_token(&f.v, &m.id, &d).expect("token");
    assert_eq!(http.forms.lock()[0].iter().find(|(k, _)| k == "refresh_token").expect("rt").1, OLD_REFRESH);
    assert_eq!(f.v.oauth_current(&m.id).expect("cur").refresh.expect("rt").expose(), NEW_REFRESH.as_bytes());

    // Google's style: no refresh_token in the answer.
    let m2 = put(&f, "B", 10);
    let google = StubToken::new(|_| (200, json!({ "access_token": NEW_ACCESS, "expires_in": 3600 }).to_string()));
    deps!(&f, google, d2);
    access_token(&f.v, &m2.id, &d2).expect("token");
    assert_eq!(f.v.oauth_current(&m2.id).expect("cur").refresh.expect("rt").expose(), OLD_REFRESH.as_bytes());
}

#[test]
fn invalid_grant_expires_it_posts_one_card_and_refuses_uses() {
    let f = fixture("o-grant");
    let m = put(&f, "A", 10);
    let http = StubToken::new(|_| (400, json!({ "error": "invalid_grant" }).to_string()));
    deps!(&f, http, d);
    let e = access_token(&f.v, &m.id, &d).err().expect("expired");
    assert_eq!(e.code, "oauth_expired");
    assert_eq!(e.status, 403, "exit 3");
    assert!(e.hint.contains("sign in again"), "{}", e.hint);
    assert!(f.v.oauth_status(&m.id).expect("status").expect("row").is_expired());
    assert_eq!(f.cards.load(Ordering::SeqCst), 1);

    // The next use: refused, no POST and no second card.
    assert_eq!(access_token(&f.v, &m.id, &d).err().expect("still").code, "oauth_expired");
    assert_eq!(http.count(), 1);
    assert_eq!(f.cards.load(Ordering::SeqCst), 1);
    assert!(audit_outcomes(&f, &m.id).iter().any(|(o, _)| o == "expired"));
}

/// One card per secret, across agents and even a second expiry.
#[test]
fn a_pending_card_means_no_second_card() {
    let f = fixture("o-one-card");
    let m = put(&f, "A", 10);
    let http = StubToken::new(|_| (400, json!({ "error": "invalid_client" }).to_string())).slow();
    deps!(&f, http, d);
    run_parallel(8, || {
        assert_eq!(access_token(&f.v, &m.id, &d).err().expect("expired").code, "oauth_expired");
    });
    assert_eq!(f.cards.load(Ordering::SeqCst), 1);

    // A person rotates it, it dies again, and the first card is still open.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(WS), "A", Kind::OauthToken);
    r.mode = PutMode::Rotate;
    f.v.put(r, bundle(unix_now() + 10), &Actor::Owner).expect("rotate");
    assert_eq!(access_token(&f.v, &m.id, &d).err().expect("expired").code, "oauth_expired");
    assert_eq!(f.cards.load(Ordering::SeqCst), 1, "the card that's already open covers this");
}

/// A token endpoint that echoes tokens must not reach any error, log or
/// audit row: not via `error`, not via `error_description`, not via a bad
/// access token.
#[test]
fn nothing_the_endpoint_says_reaches_errors_logs_or_audit() {
    let hostile: Vec<(&str, (u16, String))> = vec![
        ("a", (400, json!({ "error": "invalid_grant", "error_description": format!("bad {OLD_REFRESH} {CLIENT_SECRET}") }).to_string())),
        ("b", (400, json!({ "error": OLD_REFRESH, "error_description": NEW_ACCESS }).to_string())),
        ("c", (500, format!("<html>{OLD_REFRESH}</html>"))),
        ("d", (200, json!({ "access_token": format!("bad\n{OLD_REFRESH}"), "expires_in": 3600 }).to_string())),
        ("e", (200, json!({ "access_token": NEW_ACCESS, "refresh_token": format!("x y{NEW_REFRESH}"), "expires_in": 3600 }).to_string())),
        ("f", (200, json!({ "refresh_token": NEW_REFRESH, "note": OLD_ACCESS }).to_string())),
    ];
    for (tag, reply) in hostile {
        let f = fixture(&format!("o-hostile-{tag}"));
        let m = put(&f, &format!("HOSTILE_{}", tag.to_uppercase()), -60);
        let http = StubToken::new(move |_| reply.clone());
        deps!(&f, http, d);
        let e = access_token(&f.v, &m.id, &d).err().expect("refused");
        no_tokens_in("the error", &format!("{} {}", e.code, e.hint));
        no_tokens_in("the status", &json!(f.v.oauth_status(&m.id).expect("status")).to_string());
        no_tokens_in_audit(&f, &m.id);
        // A provider's own word for the failure is only kept from a fixed list.
        for (o, _) in audit_outcomes(&f, &m.id) {
            assert!(!o.contains("k2test"), "{o}");
        }
    }
    no_tokens_in_logs();
}

/// The debug and display forms the failure path can reach.
#[test]
fn oauth_errors_and_tokens_print_nothing_secret() {
    let t = Tokens { access_token: NEW_ACCESS.into(), refresh_token: Some(NEW_REFRESH.into()), scope: None, token_type: "Bearer".into(), expires_in: 1, id_token: None };
    no_tokens_in("Tokens debug", &format!("{t:?}"));
    let e = OauthError::Http("login.example.com: timed out".into());
    assert_eq!(outcome_word(&e), "transport");
    let e = OauthError::Provider { error: OLD_REFRESH.into(), description: Some(NEW_ACCESS.into()) };
    assert_eq!(outcome_word(&e), "provider_error", "an unknown error word isn't repeated");
    assert_eq!(outcome_word(&OauthError::Provider { error: "server_error".into(), description: None }), "server_error");
}

#[test]
fn a_failed_refresh_keeps_using_a_token_that_still_works() {
    let f = fixture("o-transient");
    let m = put(&f, "A", 60);
    let http = StubToken::new(|_| (503, "{}".to_string())).slow();
    deps!(&f, http, d);
    let got = Mutex::new(Vec::new());
    run_parallel(8, || {
        got.lock().push(access_token(&f.v, &m.id, &d).expect("old token still works").expose().to_vec());
    });
    assert!(got.into_inner().iter().all(|t| t == OLD_ACCESS.as_bytes()));
    assert_eq!(http.count(), 1, "the outage cost one POST");
    assert!(!f.v.oauth_status(&m.id).expect("status").is_some_and(|s| s.is_expired()), "not expired");
    assert_eq!(f.cards.load(Ordering::SeqCst), 0, "no card for an outage");
    assert!(audit_outcomes(&f, &m.id).iter().any(|(o, _)| o == "error:bad_response"));
}

#[test]
fn a_failed_refresh_with_a_dead_token_refuses_and_remembers() {
    let f = fixture("o-dead");
    let m = put(&f, "A", -60);
    let http = StubToken::new(|_| (503, "upstream died".to_string()));
    deps!(&f, http, d);
    let e = access_token(&f.v, &m.id, &d).err().expect("refused");
    assert_eq!(e.code, "oauth_refresh_failed");
    assert_eq!(access_token(&f.v, &m.id, &d).err().expect("refused").code, "oauth_refresh_failed");
    assert_eq!(http.count(), 1, "remembered for the cooldown");
}

#[test]
fn a_token_url_on_a_private_address_is_refused_by_the_egress_gate() {
    use crate::egress::test_dns;
    let f = fixture("o-egress");
    let _pin = test_dns::pin("login.internal.example", vec![test_dns::plain("10.0.0.5".parse().expect("ip"))]);
    // The creator's check only sees the name; the gate sees the address.
    let m = put_with(&f, "A", "https://login.internal.example/token", ClientAuth::Post, vec!["api.example.com".into()], -60);
    let http = EgressHttp::default();
    deps!(&f, http, d);
    let e = access_token(&f.v, &m.id, &d).err().expect("refused");
    assert_eq!(e.code, "address_not_allowed");
    assert!(!e.hint.contains("/token"), "{}", e.hint);
    assert!(!f.v.oauth_status(&m.id).expect("status").is_some_and(|s| s.is_expired()), "a refusal isn't a dead sign-in");
    assert_eq!(f.cards.load(Ordering::SeqCst), 0);

    // An IP literal straight at the gate.
    let r = EgressHttp::default().post_form("https://10.0.0.5/token", &[("a", "b")]);
    assert!(matches!(r, Err(OauthError::Egress { code: "address_not_allowed", .. })));
}

#[test]
fn each_client_auth_sends_the_secret_its_way() {
    let f = fixture("o-auth");
    let http = StubToken::rotating();
    deps!(&f, http, d);

    let post = put_with(&f, "POST", "https://login.example.com/token", ClientAuth::Post, vec!["api.example.com".into()], 10);
    access_token(&f.v, &post.id, &d).expect("token");
    let form = http.forms.lock()[0].clone();
    assert!(form.contains(&("client_id".to_string(), "client-1".to_string())));
    assert!(form.contains(&("client_secret".to_string(), CLIENT_SECRET.to_string())));
    assert!(form.contains(&("grant_type".to_string(), "refresh_token".to_string())));

    let basic = put_with(&f, "BASIC", "https://login.example.com/token", ClientAuth::Basic, vec!["api.example.com".into()], 10);
    access_token(&f.v, &basic.id, &d).expect("token");
    let form = http.forms.lock()[1].clone();
    assert!(!form.iter().any(|(k, _)| k == "client_id" || k == "client_secret"), "basic carries the client in the header");
    assert_eq!(http.basic.lock()[0], ("client-1".to_string(), CLIENT_SECRET.to_string()));

    let public = put_with(&f, "PUBLIC", "https://login.example.com/token", ClientAuth::None, vec!["api.example.com".into()], 10);
    access_token(&f.v, &public.id, &d).expect("token");
    let form = http.forms.lock()[2].clone();
    assert!(form.iter().any(|(k, _)| k == "client_id"));
    assert!(!form.iter().any(|(k, _)| k == "client_secret"), "none never sends the secret");
}

#[test]
fn a_short_lived_token_doesnt_refresh_on_every_use() {
    let f = fixture("o-short");
    let m = put(&f, "A", 10);
    let http = StubToken::new(|_| (200, json!({ "access_token": NEW_ACCESS, "expires_in": 120 }).to_string()));
    deps!(&f, http, d);
    access_token(&f.v, &m.id, &d).expect("token");
    access_token(&f.v, &m.id, &d).expect("token");
    assert_eq!(http.count(), 1, "120 s tokens are fresh for 60 s, not refreshed on every use");
}

#[test]
fn no_lifetime_from_the_endpoint_is_treated_as_an_hour() {
    let f = fixture("o-nolife");
    let m = put(&f, "A", 10);
    let http = StubToken::new(|_| (200, json!({ "access_token": NEW_ACCESS }).to_string()));
    deps!(&f, http, d);
    access_token(&f.v, &m.id, &d).expect("token");
    access_token(&f.v, &m.id, &d).expect("token");
    assert_eq!(http.count(), 1);
    let exp = f.v.oauth_current(&m.id).expect("cur").expires_at.expect("exp");
    assert!((exp - unix_now() - DEFAULT_LIFETIME_SECS).abs() < 30);
}

#[test]
fn a_person_saving_a_new_sign_in_restarts_it() {
    let f = fixture("o-restart");
    let m = put(&f, "A", 10);
    let dead = StubToken::new(|_| (400, json!({ "error": "invalid_grant" }).to_string()));
    deps!(&f, dead, d);
    assert_eq!(access_token(&f.v, &m.id, &d).err().expect("expired").code, "oauth_expired");

    let mut r = PutRequest::new(Namespace::User, Scope::workspace(WS), "A", Kind::OauthToken);
    r.mode = PutMode::Rotate;
    f.v.put(r, bundle(unix_now() + 3600), &Actor::Owner).expect("rotate");
    let t = access_token(&f.v, &m.id, &d).expect("works again");
    assert_eq!(t.expose(), OLD_ACCESS.as_bytes());
}

// ── through the fetch route, against a local stub upstream ──────────

/// An upstream on loopback: 401 for the old token, 200 echoing the request
/// head for the new one. `always_401` refuses everything.
fn upstream(always_401: bool) -> (u16, Arc<AtomicUsize>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h2 = hits.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            h2.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 8192];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                let head = String::from_utf8_lossy(&got).to_string();
                let refused = always_401 || head.contains(OLD_ACCESS);
                // A hostile echo: the upstream also repeats the refresh token.
                let text = format!("{head} {OLD_REFRESH} {NEW_REFRESH} {CLIENT_SECRET}");
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    if refused { "401 Unauthorized" } else { "200 OK" },
                    text.len(),
                    text
                );
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });
    (port, hits)
}

fn agent_caller() -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: WS.into(), session_id: Some("s1".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("s1".into()),
    }
}

fn fetch_body(name: &str, port: u16) -> String {
    json!({ "secret": name, "url": format!("http://127.0.0.1:{port}/v1/me") }).to_string()
}

#[test]
fn fetch_sends_the_fresh_token_and_masks_every_echo() {
    crate::test_support::with_temp_home(|| {
        let f = fixture("o-fetch");
        let (port, _) = upstream(false);
        let m = put_with(&f, "MAIL", "https://login.example.com/token", ClientAuth::Post, vec![format!("http://127.0.0.1:{port}")], 10);
        let http = StubToken::rotating();
        deps!(&f, http, d);
        let out = crate::vault_use_routes::fetch_with(&f.v, &agent_caller(), fetch_body("MAIL", port).as_bytes(), &d).expect("fetch");
        assert_eq!(out["status"], 200, "{out}");
        assert_eq!(http.count(), 1);
        let text = out.to_string();
        no_tokens_in("the response", &text);
        assert!(text.contains("[k2v:MAIL]"), "the echoed header is masked");
        no_tokens_in_logs();
        no_tokens_in_audit(&f, &m.id);
    });
}

#[test]
fn a_401_forces_one_refresh_and_one_retry() {
    crate::test_support::with_temp_home(|| {
        let f = fixture("o-401");
        let (port, hits) = upstream(false);
        // Fresh for an hour by its own account, but the upstream refuses it.
        put_with(&f, "MAIL", "https://login.example.com/token", ClientAuth::Post, vec![format!("http://127.0.0.1:{port}")], 3600);
        let http = StubToken::rotating();
        deps!(&f, http, d);
        let out = crate::vault_use_routes::fetch_with(&f.v, &agent_caller(), fetch_body("MAIL", port).as_bytes(), &d).expect("fetch");
        assert_eq!(out["status"], 200, "{out}");
        assert_eq!(http.count(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "the 401 and the retry");
        no_tokens_in("the response", &out.to_string());
    });
}

#[test]
fn parallel_401s_share_one_refresh() {
    crate::test_support::with_temp_home(|| {
        let f = fixture("o-401-par");
        let (port, _) = upstream(false);
        put_with(&f, "MAIL", "https://login.example.com/token", ClientAuth::Post, vec![format!("http://127.0.0.1:{port}")], 3600);
        let http = StubToken::rotating().slow();
        deps!(&f, http, d);
        let body = fetch_body("MAIL", port);
        run_parallel(8, || {
            let out = crate::vault_use_routes::fetch_with(&f.v, &agent_caller(), body.as_bytes(), &d).expect("fetch");
            assert_eq!(out["status"], 200, "{out}");
        });
        assert_eq!(http.count(), 1, "one POST for eight 401s");
    });
}

#[test]
fn a_url_that_always_answers_401_retries_once_and_doesnt_hammer_the_endpoint() {
    crate::test_support::with_temp_home(|| {
        let f = fixture("o-401-loop");
        let (port, hits) = upstream(true);
        put_with(&f, "MAIL", "https://login.example.com/token", ClientAuth::Post, vec![format!("http://127.0.0.1:{port}")], 3600);
        let http = StubToken::rotating();
        deps!(&f, http, d);
        let out = crate::vault_use_routes::fetch_with(&f.v, &agent_caller(), fetch_body("MAIL", port).as_bytes(), &d).expect("fetch");
        assert_eq!(out["status"], 401);
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one retry, no loop");
        assert_eq!(http.count(), 1);
        // Later fetches get the 401 back without another token POST.
        let out = crate::vault_use_routes::fetch_with(&f.v, &agent_caller(), fetch_body("MAIL", port).as_bytes(), &d).expect("fetch");
        assert_eq!(out["status"], 401);
        assert_eq!(http.count(), 1, "at most one forced refresh a minute");
    });
}

#[test]
fn an_expired_sign_in_makes_fetch_exit_3_with_the_right_code() {
    crate::test_support::with_temp_home(|| {
        let f = fixture("o-fetch-exp");
        let (port, hits) = upstream(false);
        put_with(&f, "MAIL", "https://login.example.com/token", ClientAuth::Post, vec![format!("http://127.0.0.1:{port}")], 10);
        let http = StubToken::new(|_| (400, json!({ "error": "invalid_grant" }).to_string()));
        deps!(&f, http, d);
        let e = crate::vault_use_routes::fetch_with(&f.v, &agent_caller(), fetch_body("MAIL", port).as_bytes(), &d).err().expect("refused");
        assert_eq!(e.code, "oauth_expired");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing was sent");
    });
}

/// `execute` fails closed if a caller forgets the refresh path.
#[test]
fn execute_refuses_a_sign_in_that_wasnt_swapped_for_its_token() {
    use crate::vault_fetch::{execute, Allowlist, FetchSpec, Injection};
    use k2_vault::hosts::OriginPattern;
    use k2_vault::inject::Dst;
    use k2_vault::redact::Redactor;
    let pat = OriginPattern::parse("https://api.example.com:443", false).expect("pattern");
    let inj = Injection {
        name: "A".into(),
        value: SecretBytes::from_slice(b"anything"),
        dst: Dst::Header("Authorization".into()),
        fmt: "Bearer {}".into(),
        allowed: vec![pat],
        sign: Some(Sign::OauthRefresh { token_url: "https://login.example.com/token".into(), client_auth: ClientAuth::Post }),
    };
    let allow = Allowlist::from_injections(std::slice::from_ref(&inj));
    let spec = FetchSpec {
        method: "GET".into(),
        url: "https://api.example.com/x".into(),
        headers: vec![],
        body: None,
        max_bytes: 0,
        timeout: Duration::ZERO,
        include_headers: false,
        purpose: crate::egress::Purpose::VaultFetch,
    };
    let e = execute(spec, &allow, vec![inj], &Redactor::empty()).err().expect("refused");
    assert_eq!(e.code, "vault_internal");
}

#[test]
fn a_policy_cant_take_the_signer_off_through_the_vault() {
    let f = fixture("o-policy");
    put(&f, "A", 3600);
    let sel = Selector::user(Scope::workspace(WS), "A");
    let e = f.v.set_policy(&sel, PolicyPatch { default_delivery: Some(Delivery::Env), delivery_ceiling: Some(Delivery::Env), ..Default::default() }, &Actor::Owner).err().expect("refused");
    assert_eq!(e.code, "oauth_fetch_only");
}

#[test]
fn the_mail_refresh_still_posts_what_it_did() {
    use crate::mail::oauth::{refresh, OauthProvider};
    let http = StubToken::rotating();
    refresh(OauthProvider::Gmail, "rt", Some("cid"), Some("sec"), &http).expect("refresh");
    let form = http.forms.lock()[0].clone();
    let keys: Vec<&str> = form.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["client_id", "refresh_token", "grant_type", "client_secret"]);
}

#[test]
fn set_takes_the_signer_from_the_cli_body() {
    use crate::vault_routes::SignBody;
    let b: SignBody = serde_json::from_value(json!({ "scheme": "oauth-refresh", "tokenUrl": "https://oauth2.googleapis.com/token", "clientAuth": "basic" })).expect("body");
    let s = b.build().expect("signer");
    assert_eq!(s, Sign::OauthRefresh { token_url: "https://oauth2.googleapis.com/token".into(), client_auth: ClientAuth::Basic });
    // Default: post.
    let b: SignBody = serde_json::from_value(json!({ "scheme": "oauth-refresh", "tokenUrl": "https://oauth2.googleapis.com/token" })).expect("body");
    assert!(matches!(b.build().expect("signer"), Sign::OauthRefresh { client_auth: ClientAuth::Post, .. }));
    // Needs a URL, refuses http and flags of other signers.
    let b: SignBody = serde_json::from_value(json!({ "scheme": "oauth-refresh" })).expect("body");
    assert_eq!(b.build().expect_err("url").code, "bad_inject");
    let b: SignBody = serde_json::from_value(json!({ "scheme": "oauth-refresh", "tokenUrl": "http://x.example/t" })).expect("body");
    assert_eq!(b.build().expect_err("http").code, "bad_token_url");
    let b: SignBody = serde_json::from_value(json!({ "scheme": "ovh", "tokenUrl": "https://x.example/t" })).expect("body");
    assert_eq!(b.build().expect_err("not for ovh").code, "bad_inject");
}

#[test]
fn list_and_show_say_expired() {
    use crate::vault_routes::oauth_json;
    let f = fixture("o-status");
    let m = put(&f, "A", 10);
    assert_eq!(oauth_json(f.v.oauth_status(&m.id))["state"], "ok");
    let http = StubToken::new(|_| (400, json!({ "error": "invalid_grant" }).to_string()));
    deps!(&f, http, d);
    let _ = access_token(&f.v, &m.id, &d);
    let out = oauth_json(f.v.oauth_status(&m.id));
    assert_eq!(out["state"], "expired");
    assert_eq!(out["message"], "expired: sign in again");
    no_tokens_in("the status", &out.to_string());
}
