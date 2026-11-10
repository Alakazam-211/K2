//! The vault proxy's rules, against the real listener on loopback and a
//! plain-HTTP fake upstream. Temp vault, fake key store, no real home.

use super::*;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::sync::atomic::AtomicUsize;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::inject::{Dst, Inject};
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, AgentActor, Consumer, Delivery, GrantRequest, KeyStore, Kind, Namespace, OpenOptions, PolicyPatch, PutRequest, Scope};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::vault::oauth_refresh::tests as oauth_tests;

pub(super) static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

const VALUE: &str = "k2test_PROXYKEY_9f8e7d6c5b4a39281706";
const WS: &str = "ws-proxy";
const S1: &str = "session-one";
const S2: &str = "session-two";

#[derive(Clone, Debug)]
struct Seen {
    head: String,
    body: Vec<u8>,
}

impl Seen {
    /// Every value of a header, case-insensitively on the name.
    fn header(&self, name: &str) -> Vec<String> {
        self.head
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
            .collect()
    }

    fn request_line(&self) -> &str {
        self.head.lines().next().expect("request line")
    }
}

type Respond = dyn Fn(&Seen, &mut TcpStream) + Send + Sync;

/// A plain-HTTP upstream on 127.0.0.1 that records what it received.
struct Upstream {
    port: u16,
    conns: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Upstream {
    fn start(respond: impl Fn(&Seen, &mut TcpStream) + Send + Sync + 'static) -> Upstream {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind upstream");
        let port = l.local_addr().expect("addr").port();
        let conns = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let respond: Arc<Respond> = Arc::new(respond);
        let (c2, s2) = (conns.clone(), seen.clone());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                c2.fetch_add(1, Ordering::SeqCst);
                let (s3, r3) = (s2.clone(), respond.clone());
                std::thread::spawn(move || {
                    let Some(req) = read_request(&mut s) else { return };
                    s3.lock().push(req.clone());
                    r3(&req, &mut s);
                });
            }
        });
        Upstream { port, conns, seen }
    }

    /// Answers every request with `200` and a small text body.
    fn ok() -> Upstream {
        Upstream::start(|_, s| reply(s, "200 OK", "", "fine"))
    }

    fn site(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn conns(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().clone()
    }
}

fn reply(s: &mut TcpStream, status: &str, extra_headers: &str, body: &str) {
    let resp = format!("HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    s.write_all(resp.as_bytes()).expect("upstream write");
}

fn read_request(s: &mut TcpStream) -> Option<Seen> {
    s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let want: usize = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, v)| v.trim().parse().expect("content-length"));
    while data.len() < head_end + want {
        let n = s.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    Some(Seen { head, body: data[head_end..].to_vec() })
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn code(&self) -> String {
        let v: Value = serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.body));
        v["error"]["code"].as_str().unwrap_or_else(|| panic!("no code: {}", self.body)).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).filter_map(|l| l.split_once(':')).find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string())
    }
}

fn parse_reply(raw: &[u8]) -> Reply {
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("response head") + 4;
    let head = String::from_utf8_lossy(&raw[..at]).to_string();
    let status = head.split_whitespace().nth(1).expect("status").parse().expect("status number");
    let mut body = raw[at..].to_vec();
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    Reply { status, head, body: String::from_utf8_lossy(&body).to_string() }
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").expect("chunk size line");
        let n = usize::from_str_radix(std::str::from_utf8(&b[..eol]).expect("hex").trim(), 16).expect("chunk size");
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[eol + 2..eol + 2 + n]);
        b = &b[eol + 2 + n + 2..];
    }
}

/// One raw HTTP/1.1 exchange (the caller's bytes go out exactly as written).
async fn raw(port: u16, request: String) -> Reply {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connect to the proxy");
    s.write_all(request.as_bytes()).await.expect("write");
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.expect("read");
    parse_reply(&out)
}

fn get(port: u16, path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
}

fn post(port: u16, path: &str, body: &str) -> String {
    format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len())
}

struct Fx {
    _t: TestDir,
    v: Arc<LocalVault>,
    live: Arc<Mutex<HashSet<String>>>,
    deps: Deps,
    running: Running,
    port: u16,
}

fn open_vault(t: &TestDir) -> Arc<LocalVault> {
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    Arc::new(LocalVault::open(t.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"))
}

fn deps_for(v: &Arc<LocalVault>, live: &Arc<Mutex<HashSet<String>>>, rate: RateLimiter) -> Deps {
    let v2 = v.clone();
    let l2 = live.clone();
    Deps::new(Arc::new(move || Ok(v2.clone())), Arc::new(move |s: &str| l2.lock().contains(s)), Box::leak(Box::new(rate)))
}

impl Fx {
    async fn new() -> Fx {
        Fx::with_rate(RateLimiter::new(1000, 100_000)).await
    }

    async fn with_rate(rate: RateLimiter) -> Fx {
        Fx::with_deps(rate, |d| d).await
    }

    /// A token endpoint stub for refreshing sign-ins.
    async fn with_oauth(http: Arc<dyn HttpClient>) -> Fx {
        Fx::with_deps(RateLimiter::new(1000, 100_000), move |d| d.with_oauth(http, Arc::new(|_: &str, _: &str, _: &k2_vault::SecretMeta| Ok(())))).await
    }

    async fn with_deps(rate: RateLimiter, adjust: impl FnOnce(Deps) -> Deps) -> Fx {
        let t = TestDir::new("vault-proxy");
        let v = open_vault(&t);
        let live = Arc::new(Mutex::new([S1.to_string(), S2.to_string()].into_iter().collect::<HashSet<_>>()));
        let deps = adjust(deps_for(&v, &live, rate));
        let running = serve(&t.path().join("vault"), deps.clone()).await.expect("serve");
        let port = running.listening.port;
        Fx { _t: t, v, live, deps, running, port }
    }

    fn put(&self, scope: Scope, name: &str, hosts: Vec<String>) -> k2_vault::SecretMeta {
        let mut r = PutRequest::new(Namespace::User, scope, name, Kind::ApiKey);
        r.hosts = hosts;
        self.v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta
    }

    /// A workspace secret bound to `up`, and a placeholder for session `sid`.
    fn secret_on(&self, up: &Upstream, name: &str, sid: &str) -> (k2_vault::SecretMeta, String) {
        let m = self.put(Scope::workspace(WS), name, vec![up.site()]);
        let ph = self.v.placeholder_mint(&m.id, WS, sid, GENERIC, None).expect("mint");
        (m, ph)
    }

    fn url(&self, ph: &str, rest: &str) -> String {
        format!("/p/{ph}/{GENERIC}/{rest}")
    }
}

fn caller(session: &str) -> VaultCaller {
    VaultCaller {
        actor: Actor::Agent(AgentActor { workspace_id: WS.into(), session_id: Some(session.into()), shell: false, vault_manage: false }),
        channel: crate::vault::actor::Channel::Plain,
        session_id: Some(session.into()),
    }
}

// ── the wire ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_key_reaches_upstream_and_the_placeholder_never_does() {
    let fx = Fx::new().await;
    let up = Upstream::start(|_, s| reply(s, "200 OK", "X-Ratelimit-Limit: 50\r\nSet-Cookie: upstream=1\r\nContent-Type: text/plain\r\n", "answer"));
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let body = "hello";
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\nx-api-key: {ph}\r\nAuthorization: Bearer {ph}\r\nX-Keep: stays\r\nContent-Length: {}\r\n\r\n{body}",
        fx.url(&ph, "v1/charges?limit=3"),
        body.len()
    );
    let r = raw(fx.port, req).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body, "answer");
    let seen = up.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].request_line(), "POST /v1/charges?limit=3 HTTP/1.1");
    assert_eq!(seen[0].header("authorization"), vec![format!("Bearer {VALUE}")], "the sealed injection, once");
    assert_eq!(seen[0].header("x-keep"), vec!["stays".to_string()]);
    assert_eq!(seen[0].header("host"), vec![format!("127.0.0.1:{}", up.port)], "the caller's Host isn't forwarded");
    assert_eq!(seen[0].body, body.as_bytes());
    assert!(!seen[0].head.contains(&ph) && !seen[0].head.contains(PLACEHOLDER_PREFIX), "{}", seen[0].head);
    // The fetch allowlist for response headers.
    assert_eq!(r.header("x-ratelimit-limit").as_deref(), Some("50"));
    assert_eq!(r.header("content-type").as_deref(), Some("text/plain"));
    assert_eq!(r.header("set-cookie"), None, "not on the response allowlist");
}

/// An agent-typed header with the secret's name is replaced, not sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sealed_header_replaces_one_the_caller_sent() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let req = format!("GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nAuthorization: Bearer typed-by-the-agent\r\n\r\n", fx.url(&ph, "a"));
    assert_eq!(raw(fx.port, req).await.status, 200);
    assert_eq!(up.seen()[0].header("authorization"), vec![format!("Bearer {VALUE}")]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_response_streams_and_an_echoed_key_is_masked() {
    let fx = Fx::new().await;
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    let up = Upstream::start(move |_, s| {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: first\n\n").expect("write");
        s.flush().expect("flush");
        // Nothing more until the caller has the first event: if the proxy
        // buffered the response, the test times out here.
        wait.lock().expect("lock").recv_timeout(Duration::from_secs(20)).expect("the first event never reached the caller");
        // The key split across two writes.
        let (a, b) = VALUE.split_at(9);
        s.write_all(format!("data: key={a}").as_bytes()).expect("write");
        s.flush().expect("flush");
        std::thread::sleep(Duration::from_millis(50));
        s.write_all(format!("{b}\n\ndata: done\n\n").as_bytes()).expect("write");
    });
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", fx.port)).await.expect("connect");
    s.write_all(get(fx.port, &fx.url(&ph, "stream")).as_bytes()).await.expect("write");
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    while !got.windows(11).any(|w| w == b"data: first") {
        let n = tokio::time::timeout(Duration::from_secs(20), s.read(&mut buf)).await.expect("first event in time").expect("read");
        assert!(n > 0, "closed early: {}", String::from_utf8_lossy(&got));
        got.extend_from_slice(&buf[..n]);
    }
    go.send(()).expect("release");
    s.read_to_end(&mut got).await.expect("rest");
    let r = parse_reply(&got);
    assert_eq!(r.status, 200);
    assert!(r.body.contains("data: key=[k2v:K]"), "{}", r.body);
    assert!(r.body.contains("data: done"), "{}", r.body);
    assert!(!r.body.contains(VALUE) && !r.body.contains(&VALUE[..9]), "{}", r.body);
    assert_eq!(r.header("content-type").as_deref(), Some("text/event-stream"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compressed_response_is_refused_rather_than_passed_unredacted() {
    let fx = Fx::new().await;
    let up = Upstream::start(|_, s| reply(s, "200 OK", "Content-Encoding: gzip\r\n", "compressed-bytes"));
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "z"))).await;
    assert_eq!(r.status, 502, "{}", r.body);
    assert_eq!(up.seen()[0].header("accept-encoding"), vec!["identity".to_string()]);
}

/// A secret that goes in the query: its own parameter is the one place in
/// the query where the placeholder belongs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_secret_replaces_its_own_parameter_only() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(WS), "Q", Kind::ApiKey);
    r.hosts = vec![up.site()];
    r.inject = Some(Inject { dst: Dst::Query("key".into()), fmt: "{}".into(), allowed_dst: vec![], allowed_fmt: vec![], sign: None });
    let m = fx.v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta;
    let ph = fx.v.placeholder_mint(&m.id, WS, S1, GENERIC, None).expect("mint");

    let ok = raw(fx.port, get(fx.port, &format!("{}?key={ph}&a=1", fx.url(&ph, "q")))).await;
    assert_eq!(ok.status, 200, "{}", ok.body);
    let line = up.seen()[0].request_line().to_string();
    assert!(line.contains(&format!("key={VALUE}")) && line.contains("a=1"), "{line}");
    assert!(!line.contains(&ph), "{line}");

    let bad = raw(fx.port, get(fx.port, &format!("{}?other={ph}", fx.url(&ph, "q")))).await;
    assert_eq!((bad.status, bad.code().as_str()), (400, "placeholder_misplaced"), "{}", bad.body);
    assert_eq!(up.conns(), 1, "the misplaced one never went out");
}

fn sign_in_on(fx: &Fx, up: &Upstream, name: &str) -> String {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(WS), name, Kind::OauthToken);
    r.hosts = vec![up.site()];
    r.inject = Some(Inject {
        sign: Some(Sign::OauthRefresh { token_url: "https://login.example.com/token".into(), client_auth: k2_vault::inject::ClientAuth::Post }),
        ..Inject::bearer()
    });
    let m = fx.v.put(r, oauth_tests::bundle(chrono::Utc::now().timestamp() + 60), &Actor::Owner).expect("put").meta;
    fx.v.placeholder_mint(&m.id, WS, S1, GENERIC, None).expect("mint")
}

/// §6.2 through the proxy: the upstream gets the current access token, after
/// one refresh; the next call costs no POST.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sign_in_goes_out_as_its_current_access_token_after_one_refresh() {
    let stub = Arc::new(oauth_tests::StubToken::rotating());
    let fx = Fx::with_oauth(stub.clone()).await;
    let up = Upstream::ok();
    let ph = sign_in_on(&fx, &up, "SIGN_IN");

    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "me"))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(up.seen()[0].header("authorization"), [format!("Bearer {}", oauth_tests::NEW_ACCESS)]);
    assert_eq!(stub.count(), 1, "one refresh for a token close to expiry");

    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "me"))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(stub.count(), 1, "the refreshed token is reused");
    assert_eq!(up.seen()[1].header("authorization"), [format!("Bearer {}", oauth_tests::NEW_ACCESS)]);
}

/// A sign-in the provider has refused is 403 `oauth_expired`, and nothing
/// reaches the upstream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_sign_in_stops_at_the_proxy() {
    let stub = Arc::new(oauth_tests::StubToken::new(|_| (400, json!({ "error": "invalid_grant" }).to_string())));
    let fx = Fx::with_oauth(stub).await;
    let up = Upstream::ok();
    let ph = sign_in_on(&fx, &up, "DEAD_SIGN_IN");
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "me"))).await;
    assert_eq!((r.status, r.code().as_str()), (403, "oauth_expired"), "{}", r.body);
    assert_eq!(up.conns(), 0);
}

/// A signer gets the exact body, and its header is K2's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signing_secret_signs_the_exact_body() {
    use k2_vault::inject::{Enc, Sign};
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let sign = Sign::HmacSha256 { header: "X-Hub-Signature-256".into(), encoding: Enc::Hex, prefix: "sha256=".into() };
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(WS), "SIGNED", Kind::ApiKey);
    r.hosts = vec![up.site()];
    r.inject = Some(Inject { dst: Dst::Header("Authorization".into()), fmt: "Bearer {}".into(), allowed_dst: vec![], allowed_fmt: vec![], sign: Some(sign.clone()) });
    let m = fx.v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta;
    let ph = fx.v.placeholder_mint(&m.id, WS, S1, GENERIC, None).expect("mint");
    let body = r#"{"a":"b"}"#;
    let url = format!("http://127.0.0.1:{}/hook", up.port);
    let r = raw(fx.port, post(fx.port, &fx.url(&ph, "hook"), body)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let want = sign_edit(&sign, &SecretBytes::from_slice(VALUE.as_bytes()), "POST", &url, body.as_bytes(), 0).expect("sign");
    let seen = up.seen();
    assert_eq!(seen[0].header("x-hub-signature-256"), vec![String::from_utf8(want[0].1.expose().to_vec()).expect("text")]);
    assert_eq!(seen[0].header("authorization"), Vec::<String>::new(), "a signer doesn't paste the value");
    assert_eq!(seen[0].body, body.as_bytes());
}

// ── who may use a placeholder ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_placeholder_is_401_and_a_bad_path_is_404() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let unknown = format!("{PLACEHOLDER_PREFIX}{}", "a".repeat(32));
    for bad in [unknown.as_str(), "k2v_ph_short", "not-a-placeholder"] {
        let r = raw(fx.port, get(fx.port, &fx.url(bad, "x"))).await;
        assert_eq!((r.status, r.code().as_str()), (401, "placeholder_unknown"), "{bad}: {}", r.body);
    }
    for path in ["/", "/v1/messages", "/p/", "/p/only-one-part"] {
        assert_eq!(raw(fx.port, get(fx.port, path)).await.status, 404, "{path}");
    }
    // A real placeholder under another provider's name.
    let r = raw(fx.port, get(fx.port, &format!("/p/{ph}/anthropic/v1/messages"))).await;
    assert_eq!((r.status, r.code().as_str()), (400, "provider_mismatch"), "{}", r.body);
    assert_eq!(up.conns(), 0, "nothing reached upstream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_placeholder_is_401() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let m = fx.put(Scope::workspace(WS), "K", vec![up.site()]);
    let ph = fx.v.placeholder_mint(&m.id, WS, S1, GENERIC, Some("2020-01-01T00:00:00Z".into())).expect("mint");
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "x"))).await;
    assert_eq!(r.status, 401, "{}", r.body);
    assert_eq!(up.conns(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_grant_stops_a_live_placeholder_at_the_next_call() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let m = fx.put(Scope::server(), "SHARED", vec![up.site()]);
    let g = fx
        .v
        .grant(
            GrantRequest {
                selector: Selector::Id(m.id.clone()),
                consumer: Consumer::workspace(WS),
                delivery: Delivery::Fetch,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect("grant");
    let ph = fx.v.placeholder_mint(&m.id, WS, S1, GENERIC, None).expect("mint");
    assert_eq!(raw(fx.port, get(fx.port, &fx.url(&ph, "a"))).await.status, 200);
    fx.v.revoke(&g.id, &Actor::Owner).expect("revoke");
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "b"))).await;
    assert_eq!((r.status, r.code().as_str()), (403, "not_granted"), "{}", r.body);
    assert_eq!(up.conns(), 1, "the revoked call never left");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_session_is_401_even_before_its_placeholder_is_dropped() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    assert_eq!(raw(fx.port, get(fx.port, &fx.url(&ph, "a"))).await.status, 200);
    fx.live.lock().remove(S1);
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "b"))).await;
    assert_eq!((r.status, r.code().as_str()), (401, "session_ended"), "{}", r.body);
    fx.v.placeholders_drop_session(S1).expect("drop");
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "c"))).await;
    assert_eq!((r.status, r.code().as_str()), (401, "placeholder_unknown"), "{}", r.body);
    assert_eq!(up.conns(), 1);
}

/// A placeholder acts as its own session and no other: it can't reach
/// another session's secret, can't be minted for one, doesn't travel in a
/// header, and ending one session leaves the other's alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_session_cant_use_anothers_placeholder_or_secret() {
    let _env = k2_core::test_env::lock();
    let fx = Fx::new().await;
    set_listening_for_test(Some(fx.running.listening));
    let up = Upstream::ok();
    let shared = fx.put(Scope::workspace(WS), "SHARED", vec![up.site()]);
    let mut r = PutRequest::new(Namespace::User, Scope::session(S2), "ONLY_S2", Kind::ApiKey);
    r.hosts = vec![up.site()];
    r.expires_at = Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
    let mine = fx.v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta;

    // S1 can't get a placeholder for S2's chat secret through the route.
    let live = |_: &str| true;
    let e = mint_for_caller(&fx.v, &caller(S1), &Selector::Id(mine.id.clone()), None, &live).expect_err("not S1's");
    assert_eq!(e.status, 403, "{e:?}");
    // Nor does a placeholder row naming S2's secret under S1 reach it: the
    // grant is checked for the placeholder's own session on every call.
    let forged = fx.v.placeholder_mint(&mine.id, WS, S1, GENERIC, None).expect("mint");
    let r = raw(fx.port, get(fx.port, &fx.url(&forged, "x"))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(up.conns(), 0, "S2's secret was never sent for S1");

    let ph1 = fx.v.placeholder_mint(&shared.id, WS, S1, GENERIC, None).expect("mint");
    let ph2 = fx.v.placeholder_mint(&shared.id, WS, S2, GENERIC, None).expect("mint");
    assert_ne!(ph1, ph2);

    // Another session's placeholder in a header is dropped, not forwarded.
    let req = format!("GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nx-other: {ph2}\r\n\r\n", fx.url(&ph1, "a"));
    assert_eq!(raw(fx.port, req).await.status, 200);
    let seen = up.seen();
    assert!(!seen[0].head.contains(&ph2) && seen[0].header("x-other").is_empty(), "{}", seen[0].head);

    // Ending S2 ends only S2's.
    fx.live.lock().remove(S2);
    assert_eq!(raw(fx.port, get(fx.port, &fx.url(&ph2, "b"))).await.status, 401);
    assert_eq!(raw(fx.port, get(fx.port, &fx.url(&ph1, "c"))).await.status, 200, "S1's keeps working");
    set_listening_for_test(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rate_limit_and_the_stream_cap_hold() {
    let fx = Fx::with_rate(RateLimiter::new(1, 600)).await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    assert_eq!(raw(fx.port, get(fx.port, &fx.url(&ph, "a"))).await.status, 200);
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "b"))).await;
    assert_eq!((r.status, r.code().as_str()), (429, "rate_limited"), "{}", r.body);
    assert_eq!(up.conns(), 1);

    let streams = Streams::default();
    let mut held: Vec<Slot> = (0..MAX_STREAMS).map(|_| streams.acquire(S1).expect("a slot")).collect();
    assert!(streams.acquire(S1).is_none(), "the 33rd is refused");
    assert!(streams.acquire(S2).is_some(), "per session");
    held.pop();
    assert!(streams.acquire(S1).is_some(), "a finished stream frees its slot");
}

// ── where a secret may go ───────────────────────────────────────────

/// Nothing in the request can move the destination.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_bound_site_ever_receives_the_secret() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let other = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let host = format!("127.0.0.1:{}", other.port);
    let tricks = [
        // A path that looks like another site.
        format!("GET /p/{ph}/{GENERIC}//{host}/x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        format!("GET /p/{ph}/{GENERIC}/http://{host}/x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        format!("GET /p/{ph}/{GENERIC}/@{host}/x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        format!("GET /p/{ph}/{GENERIC}/../../../x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        format!("GET /p/{ph}/{GENERIC}/%2e%2e/%2e%2e/x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        // The proxy-style absolute form.
        format!("GET http://{host}/p/{ph}/{GENERIC}/x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        // Headers a client might use to steer a proxy.
        format!("GET /p/{ph}/{GENERIC}/x HTTP/1.1\r\nHost: {host}\r\nX-Forwarded-Host: {host}\r\nProxy-Authorization: Basic eDp5\r\nConnection: close\r\n\r\n"),
    ];
    let n = tricks.len();
    for t in tricks {
        raw(fx.port, t).await;
    }
    assert_eq!(other.conns(), 0, "another site was contacted");
    assert_eq!(up.conns(), n, "each stayed on the bound site");
    for s in up.seen() {
        assert_eq!(s.header("host"), vec![format!("127.0.0.1:{}", up.port)], "{}", s.head);
        assert!(s.header("proxy-authorization").is_empty(), "{}", s.head);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redirect_off_the_site_is_handed_back_and_a_same_site_one_is_followed() {
    let fx = Fx::new().await;
    let other = Upstream::ok();
    let elsewhere = other.site();
    let up = Upstream::start(move |req, s| {
        if req.request_line().contains("/away") {
            reply(s, "302 Found", &format!("Location: {elsewhere}/stolen\r\n"), "");
        } else if req.request_line().contains("/near") {
            reply(s, "302 Found", "Location: /landed\r\n", "");
        } else {
            reply(s, "200 OK", "", "landed");
        }
    });
    let (_, ph) = fx.secret_on(&up, "K", S1);

    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "away"))).await;
    assert_eq!(r.status, 302, "{}", r.body);
    assert_eq!(other.conns(), 0, "the secret isn't re-sent to another origin");
    assert!(r.header("location").is_some_and(|l| l.ends_with("/stolen")));

    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "near"))).await;
    assert_eq!((r.status, r.body.as_str()), (200, "landed"));
    let seen = up.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[2].header("authorization"), vec![format!("Bearer {VALUE}")], "re-injected on the same origin");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_to_the_secrets_sites_stops_the_placeholder() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let other = Upstream::ok();
    let (m, ph) = fx.secret_on(&up, "K", S1);
    assert_eq!(raw(fx.port, get(fx.port, &fx.url(&ph, "a"))).await.status, 200);
    let sel = Selector::Id(m.id.clone());
    fx.v.set_policy(&sel, PolicyPatch { add_hosts: vec![other.site()], ..Default::default() }, &Actor::Owner).expect("policy");
    let r = raw(fx.port, get(fx.port, &fx.url(&ph, "b"))).await;
    assert_eq!((r.status, r.code().as_str()), (403, "proxy_site_changed"), "{}", r.body);
    assert_eq!((up.conns(), other.conns()), (1, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_url_needs_exactly_one_exact_site() {
    let _env = k2_core::test_env::lock();
    let fx = Fx::new().await;
    set_listening_for_test(Some(fx.running.listening));
    let live = |_: &str| true;
    let (a, b) = (Upstream::ok(), Upstream::ok());
    let two = fx.put(Scope::workspace(WS), "TWO", vec![a.site(), b.site()]);
    let e = mint_for_caller(&fx.v, &caller(S1), &Selector::Id(two.id.clone()), None, &live).expect_err("two sites");
    assert_eq!(e.code, "proxy_needs_one_host");
    assert!(e.hint.contains("k2 vault fetch"), "says what to do: {}", e.hint);
    let wild = fx.put(Scope::workspace(WS), "WILD", vec!["*.example.com".into()]);
    let e = mint_for_caller(&fx.v, &caller(S1), &Selector::Id(wild.id.clone()), None, &live).expect_err("wildcard");
    assert_eq!(e.code, "proxy_needs_one_host");
    let none = fx.put(Scope::workspace(WS), "NO_SITE", vec![]);
    assert_eq!(mint_for_caller(&fx.v, &caller(S1), &Selector::Id(none.id.clone()), None, &live).expect_err("no site").code, "proxy_needs_one_host");
    set_listening_for_test(None);
}

// ── the listener ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listener_answers_on_loopback_and_nowhere_else() {
    let fx = Fx::new().await;
    tokio::net::TcpStream::connect(("127.0.0.1", fx.port)).await.expect("loopback connects");
    let outside: IpAddr = egress::own_addresses()
        .into_iter()
        .find(|ip| matches!(ip, IpAddr::V4(v) if !v.is_loopback()))
        .expect("a test machine has a non-loopback IPv4 address");
    let e = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect((outside, fx.port)))
        .await
        .expect("a refused connection is quick");
    assert!(e.is_err(), "the proxy answered on {outside}");
    let v6 = tokio::net::TcpStream::connect(("::1", fx.port)).await;
    assert!(v6.is_err(), "the proxy answered on ::1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_loopback_peer_is_refused_before_any_lookup() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let req = Request::builder().uri(fx.url(&ph, "x")).body(Empty::<Bytes>::new()).expect("request");
    let r = handle(&fx.deps, "10.1.2.3:5000".parse().expect("addr"), req).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(up.conns(), 0);
    assert!(peer_allowed("127.0.0.1:1".parse().expect("addr")));
    assert!(peer_allowed("[::ffff:127.0.0.1]:1".parse().expect("addr")), "a mapped loopback address is loopback");
    assert!(!peer_allowed("[::ffff:10.0.0.1]:1".parse().expect("addr")));
    assert!(!peer_allowed("192.168.1.2:1".parse().expect("addr")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_port_comes_back_after_a_restart_and_a_taken_one_moves() {
    let t = TestDir::new("vault-proxy-port");
    let v = open_vault(&t);
    let live = Arc::new(Mutex::new(HashSet::new()));
    let dir = t.path().join("vault");
    let deps = deps_for(&v, &live, RateLimiter::for_fetch());

    let first = serve(&dir, deps.clone()).await.expect("serve");
    let port = first.listening.port;
    assert!(!first.listening.moved);
    assert_eq!(std::fs::read_to_string(dir.join(PORT_FILE)).expect("port file").trim(), port.to_string());
    first.stop().await;

    let second = serve(&dir, deps.clone()).await.expect("serve again");
    assert_eq!(second.listening, Listening { port, moved: false }, "a restart keeps the port");
    second.stop().await;

    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).expect("take the port");
    let third = serve(&dir, deps.clone()).await.expect("serve after losing the port");
    assert!(third.listening.moved, "status must say so");
    assert_ne!(third.listening.port, port);
    assert_eq!(std::fs::read_to_string(dir.join(PORT_FILE)).expect("port file").trim(), third.listening.port.to_string());
    drop(squatter);
    third.stop().await;
}

// ── never forwarded, never logged ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_placeholder_in_a_query_or_path_is_refused_before_anything_leaves() {
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let cases = [
        get(fx.port, &format!("{}?token={ph}", fx.url(&ph, "x"))),
        get(fx.port, &fx.url(&ph, &format!("users/{ph}"))),
    ];
    for c in cases {
        let r = raw(fx.port, c).await;
        assert_eq!((r.status, r.code().as_str()), (400, "placeholder_misplaced"), "{}", r.body);
        assert!(!r.body.contains(&ph), "the refusal doesn't echo it: {}", r.body);
    }
    assert_eq!(up.conns(), 0, "not one byte went out");
}

/// Rosson's rule: an agent's conversation can hold its own placeholder, and
/// every later LLM call resends it, so a body holding one must not break the
/// session. It goes through byte for byte (never substituted), and the log
/// says so without the placeholder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_placeholder_in_a_body_goes_through_unchanged() {
    use base64::Engine as _;
    let _env = k2_core::test_env::lock();
    LOG.lock().clear();
    let fx = Fx::new().await;
    let up = Upstream::ok();
    let (_, ph) = fx.secret_on(&up, "K", S1);
    let b64 = base64::engine::general_purpose::STANDARD.encode(format!("user:{ph}"));
    let bodies = [format!(r#"{{"messages":[{{"content":"my key is {ph}"}}]}}"#), format!(r#"{{"auth":"{b64}"}}"#)];
    for b in &bodies {
        let r = raw(fx.port, post(fx.port, &fx.url(&ph, "chat"), b)).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }
    let seen = up.seen();
    assert_eq!(seen.len(), 2);
    for (got, sent) in seen.iter().zip(bodies.iter()) {
        assert_eq!(got.body, sent.as_bytes(), "byte for byte");
        assert!(!String::from_utf8_lossy(&got.body).contains(VALUE), "the placeholder isn't the secret: never substituted");
    }
    assert_eq!(seen[0].header("authorization"), [format!("Bearer {VALUE}")], "the key still goes where it belongs");
    let logs = LOG.lock().clone();
    assert_eq!(logs.iter().filter(|l| l.contains("placeholder_in_body")).count(), 2, "each pass is logged: {logs:?}");
    assert!(logs.iter().all(|l| !l.contains(&ph) && !l.contains(VALUE)), "{logs:?}");
}

/// Everything K2 itself writes or answers about a call: no value, no
/// placeholder, across success and every refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn values_and_placeholders_never_reach_logs_or_refusals() {
    let fx = Fx::new().await;
    LOG.lock().clear();
    let up = Upstream::start(|_, s| reply(s, "500 Boom", "", &format!("upstream says {VALUE}")));
    let (m, ph) = fx.secret_on(&up, "K", S1);
    let mut bodies = Vec::new();
    bodies.push(raw(fx.port, get(fx.port, &fx.url(&ph, "a"))).await.body);
    bodies.push(raw(fx.port, post(fx.port, &fx.url(&ph, "b"), &ph)).await.body);
    bodies.push(raw(fx.port, get(fx.port, &format!("/p/{ph}/anthropic/x"))).await.body);
    // A second site: refused before any lookup, so nothing leaves the test.
    fx.v.set_policy(&Selector::Id(m.id.clone()), PolicyPatch { add_hosts: vec!["https://api.example.com:443".into()], ..Default::default() }, &Actor::Owner).expect("policy");
    bodies.push(raw(fx.port, get(fx.port, &fx.url(&ph, "c"))).await.body);
    fx.live.lock().remove(S1);
    bodies.push(raw(fx.port, get(fx.port, &fx.url(&ph, "d"))).await.body);
    fx.v.placeholders_drop_session(S1).expect("drop");
    bodies.push(raw(fx.port, get(fx.port, &fx.url(&ph, "e"))).await.body);

    let logs = LOG.lock().clone();
    assert!(logs.len() >= 5, "the refusals were logged: {logs:?}");
    for line in logs.iter().chain(bodies.iter()) {
        assert!(!line.contains(VALUE), "{line}");
        assert!(!line.contains(&ph), "{line}");
        assert!(!line.contains(PLACEHOLDER_PREFIX), "{line}");
    }
    assert!(bodies[0].contains("[k2v:K]"), "an upstream error echoing the key is masked: {}", bodies[0]);
}

/// The production code logs through nothing that could carry a request:
/// every log line takes only a code, a port, a name or an error kind.
#[test]
fn the_listener_has_no_log_line_that_could_carry_a_request() {
    let src = include_str!("proxy.rs");
    let prod = src.split("#[cfg(test)]\n#[path").next().expect("source");
    let mut lines = 0;
    for l in prod.lines() {
        let t = l.trim_start();
        if t.starts_with("//") {
            continue;
        }
        if t.contains("println!") || t.contains("tracing::") || t.contains("dbg!") {
            panic!("an unreviewed output path: {l}");
        }
        if t.contains("log_debug!") {
            lines += 1;
            for bad in ["token", "url", "uri", "path", "hint", "value", "parts", "headers", "body"] {
                assert!(!t.contains(bad), "a log line that may carry a request ({bad}): {l}");
            }
        }
    }
    assert_eq!(lines, 6, "a new log line needs a look: update this count after checking it");
}

// ── spawn and the route ─────────────────────────────────────────────

#[test]
fn a_proxy_delivery_reaches_a_new_session_as_exactly_its_pair() {
    use crate::vault::spawn::{materialize, SpawnRefs};
    let _env = k2_core::test_env::lock();
    let v = crate::vault::ensure_test_vault();
    set_listening_for_test(Some(Listening { port: 4242, moved: false }));
    let ws = uuid::Uuid::new_v4().to_string();
    let put = |name: &str, host: &str, d: Delivery| {
        let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), name, Kind::ApiKey);
        r.hosts = vec![host.into()];
        r.default_delivery = Some(d);
        v.put(r, SecretBytes::from_slice(format!("{VALUE}-{name}").as_bytes()), &Actor::Owner).expect("put")
    };
    put("MY_ANTHROPIC", "api.anthropic.com", Delivery::Proxy);
    put("MY_OPENAI", "api.openai.com", Delivery::Proxy);
    put("STRIPE_KEY", "api.stripe.com", Delivery::Proxy);
    put("AS_FETCH", "api.github.com", Delivery::Fetch);

    let sid = uuid::Uuid::new_v4().to_string();
    let mut env = HashMap::new();
    let out = materialize(&mut env, &ws, &sid, false, &SpawnRefs::default());
    assert!(out.skipped.is_empty(), "{out:?}");
    let ph = |k: &str| {
        let t = env.get(k).unwrap_or_else(|| panic!("{k} not set: {env:?}")).clone();
        assert!(k2_vault::refs::looks_like_placeholder(&t), "{k} isn't a placeholder");
        t
    };
    let (a, o, s) = (ph("ANTHROPIC_API_KEY"), ph("OPENAI_API_KEY"), ph("STRIPE_KEY"));
    assert_eq!(env["ANTHROPIC_BASE_URL"], format!("http://127.0.0.1:4242/p/{a}/anthropic"));
    assert_eq!(env["OPENAI_BASE_URL"], format!("http://127.0.0.1:4242/p/{o}/openai/v1"), "OpenAI's SDKs expect /v1");
    assert_eq!(env["STRIPE_KEY_BASE_URL"], format!("http://127.0.0.1:4242/p/{s}/generic"));
    assert!(!env.contains_key("AS_FETCH"), "a fetch secret stays out of the env");
    assert!(!env.values().any(|x| x.contains(VALUE)), "no value in the env: {env:?}");
    assert_eq!(v.placeholder_lookup(&a).expect("lookup").session_id, sid);
    crate::vault::spawn::session_ended(&sid);
    assert_eq!(v.placeholder_lookup(&a).expect_err("gone with the session").code, "placeholder_unknown");

    // A sandboxed cell has its own network: nothing, and it says so.
    let (mut env2, sid2) = (HashMap::new(), uuid::Uuid::new_v4().to_string());
    let out2 = materialize(&mut env2, &ws, &sid2, true, &SpawnRefs::default());
    assert!(!env2.contains_key("ANTHROPIC_API_KEY"), "{env2:?}");
    assert!(out2.skipped.iter().any(|s| s.contains("MY_ANTHROPIC") && s.contains("proxy_unavailable")), "{out2:?}");

    // A name K2 already set stays, and the real key isn't replaced.
    let (mut env3, sid3) = (HashMap::from([("ANTHROPIC_API_KEY".to_string(), "from-the-login".to_string())]), uuid::Uuid::new_v4().to_string());
    let out3 = materialize(&mut env3, &ws, &sid3, false, &SpawnRefs::default());
    assert_eq!(env3["ANTHROPIC_API_KEY"], "from-the-login");
    assert!(!env3.contains_key("ANTHROPIC_BASE_URL"), "a base URL without its placeholder would break the login");
    assert!(out3.skipped.iter().any(|s| s.contains("MY_ANTHROPIC") && s.contains("env_taken")), "{out3:?}");

    // No listener: the delivery is skipped, not silently half-done.
    set_listening_for_test(None);
    let (mut env4, sid4) = (HashMap::new(), uuid::Uuid::new_v4().to_string());
    let out4 = materialize(&mut env4, &ws, &sid4, false, &SpawnRefs::default());
    assert!(!env4.contains_key("OPENAI_API_KEY"));
    assert!(out4.skipped.iter().any(|s| s.contains("MY_OPENAI") && s.contains("proxy_unavailable")), "{out4:?}");
    for s in [&sid2, &sid3, &sid4] {
        crate::vault::spawn::session_ended(s);
    }
}

/// V1: a plain fetch secret works on demand, with no policy change, and
/// the session is the passport's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_url_mints_for_the_callers_own_session_from_a_fetch_grant() {
    let _env = k2_core::test_env::lock();
    let fx = Fx::new().await;
    set_listening_for_test(Some(fx.running.listening));
    let up = Upstream::ok();
    let m = fx.put(Scope::workspace(WS), "STRIPE_KEY", vec![up.site()]);
    assert_eq!(m.default_delivery, Delivery::Fetch);
    let live = |_: &str| true;
    let out = mint_for_caller(&fx.v, &caller(S1), &Selector::Id(m.id.clone()), Some(7200), &live).expect("mint");
    let ph = out["placeholder"].as_str().expect("placeholder").to_string();
    assert_eq!(out["envName"], "STRIPE_KEY");
    assert_eq!(out["baseUrlEnv"], "STRIPE_KEY_BASE_URL");
    assert_eq!(out["baseUrl"], format!("http://127.0.0.1:{}/p/{ph}/generic", fx.port));
    assert!(out["expiresAt"].is_string(), "{out}");
    assert!(!out.to_string().contains(VALUE), "{out}");
    assert_eq!(fx.v.placeholder_lookup(&ph).expect("lookup").session_id, S1, "the passport's session");
    let r = raw(fx.port, get(fx.port, &format!("/p/{ph}/generic/ok"))).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Not for a person, or a session that isn't running.
    let owner = VaultCaller { actor: Actor::Owner, channel: crate::vault::actor::Channel::Plain, session_id: None };
    assert_eq!(mint_for_caller(&fx.v, &owner, &Selector::Id(m.id.clone()), None, &live).expect_err("a person").status, 403);
    let dead = |_: &str| false;
    assert_eq!(mint_for_caller(&fx.v, &caller(S1), &Selector::Id(m.id.clone()), None, &dead).expect_err("dead session").code, "session_ended");
    set_listening_for_test(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_loop_cant_mint_placeholders_without_end() {
    let _env = k2_core::test_env::lock();
    let fx = Fx::new().await;
    set_listening_for_test(Some(fx.running.listening));
    let up = Upstream::ok();
    let m = fx.put(Scope::workspace(WS), "K", vec![up.site()]);
    let live = |_: &str| true;
    for _ in 0..MAX_PLACEHOLDERS {
        mint_for_caller(&fx.v, &caller(S1), &Selector::Id(m.id.clone()), None, &live).expect("under the cap");
    }
    let e = mint_for_caller(&fx.v, &caller(S1), &Selector::Id(m.id.clone()), None, &live).expect_err("over the cap");
    assert_eq!((e.code, e.status), ("too_many_placeholders", 429));
    mint_for_caller(&fx.v, &caller(S2), &Selector::Id(m.id.clone()), None, &live).expect("per session");
    set_listening_for_test(None);
}

/// The route is POST-only (405 from the shared guards) and has a policy row.
#[test]
fn proxy_url_is_a_post_only_route_with_a_policy_row() {
    use crate::vault_use_routes::PROXY_URL;
    assert!(crate::vault_routes::is_route(PROXY_URL));
    assert!(crate::vault_routes::is_post_route(PROXY_URL), "GET must get the shared guards' 405");
    assert!(crate::routes::route_policy::post_allowed(PROXY_URL), "a POST row");
    let row = crate::routes::route_policy::lookup(PROXY_URL).expect("a policy row");
    assert!(row.post.is_some() && row.get.is_none(), "POST only");
}

#[test]
fn status_says_where_the_listener_is_and_when_it_moved() {
    let _env = k2_core::test_env::lock();
    set_listening_for_test(None);
    assert_eq!(status_json()["port"], Value::Null);
    set_listening_for_test(Some(Listening { port: 7000, moved: false }));
    assert_eq!(status_json(), json!({ "port": 7000, "moved": false }));
    set_listening_for_test(Some(Listening { port: 7001, moved: true }));
    let v = status_json();
    assert_eq!(v["moved"], true);
    assert!(v["hint"].as_str().is_some_and(|h| h.contains("re-run k2 vault proxy-url")), "{v}");
    set_listening_for_test(None);
}

#[test]
fn the_provider_table_is_by_exact_https_site() {
    let site = |s: &str| Origin::parse_url(s).expect("origin").1;
    assert_eq!(provider_id(&site("https://api.anthropic.com")), "anthropic");
    assert_eq!(provider_id(&site("https://api.openai.com")), "openai");
    assert_eq!(provider_id(&site("https://api.x.ai")), "xai");
    assert_eq!(provider_id(&site("https://generativelanguage.googleapis.com")), "gemini");
    assert_eq!(provider_id(&site("https://api.stripe.com")), GENERIC);
    assert_eq!(provider_id(&site("https://api.anthropic.com:8443")), GENERIC, "another port isn't the provider");
    assert_eq!(provider_id(&site("https://api.anthropic.com.evil.example")), GENERIC);
    // A generic pair can't be named after something the env deny list keeps.
    let e = env_pair(&site("https://api.stripe.com"), Some("PATH"), "x").err().expect("denied name");
    assert_eq!(e.code, "env_name_not_allowed");
    let e = env_pair(&site("https://api.stripe.com"), None, "k2-token").err().expect("denied default name");
    assert_eq!(e.code, "env_name_not_allowed");
}
