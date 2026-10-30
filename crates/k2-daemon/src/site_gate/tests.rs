//! The gate end to end through the real E2E listener (`serve_one` via the
//! accept loop), with a stand-in TLS client and an upstream that counts its
//! accepts (PV-14). Every label here is unique: the test database is shared.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use k2_core::site_visibility as sv;
use k2_core::tunnel::subdomains::{self, SubdomainMap};
use rusqlite::params;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::dev::{DevHooks, DevWake};
use super::*;

const PRIMARY: &str = "rosson";

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let h = k2_core::db::shared();
    let c = h.lock();
    f(&c)
}

/// What the stand-in site saw.
#[derive(Default)]
struct Upstream {
    accepts: AtomicUsize,
    requests: Mutex<Vec<String>>,
}

/// A small HTTP/1.1 site: 200 with its tag, keep-alive; an `Upgrade` request
/// gets a `101`, then echoes raw bytes.
async fn spawn_site() -> (u16, Arc<Upstream>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Upstream::default());
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            s2.accepts.fetch_add(1, Ordering::SeqCst);
            let seen = s2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let Ok(n) = s.read(&mut tmp).await else { return };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    while let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i + 4]).to_string();
                        buf.drain(..i + 4);
                        seen.requests.lock().unwrap().push(head.clone());
                        if head.to_ascii_lowercase().contains("upgrade: websocket") {
                            let _ = s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").await;
                            loop {
                                let Ok(n) = s.read(&mut tmp).await else { return };
                                if n == 0 {
                                    return;
                                }
                                if s.write_all(&tmp[..n]).await.is_err() {
                                    return;
                                }
                            }
                        }
                        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nSITE").await;
                        if head.to_ascii_lowercase().contains("connection: close") {
                            return;
                        }
                    }
                }
            });
        }
    });
    (port, seen)
}

/// The listener, for this test's routing map.
struct Rig {
    port: u16,
    cert: String,
    _home: crate::test_support::TempHome,
    _self_signed: k2_core::test_env::EnvVar,
}

impl Rig {
    /// `routes`: label → port. Holds the crate-wide global-state lock.
    async fn new(routes: &[(&str, u16)]) -> Self {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let home = crate::test_support::TempHome::new();
        let self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let mut targets = HashMap::new();
        for (l, p) in routes {
            targets.insert(l.to_string(), format!("127.0.0.1:{p}"));
        }
        subdomains::store(SubdomainMap { primary: PRIMARY.to_string(), targets });
        let (cert, key) = k2_core::tunnel::tls::load_or_provision_cert(PRIMARY).expect("cert");
        let cfg = k2_core::tunnel::tls::server_config(&cert, &key).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(cfg));
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            crate::tunnel_tls_listener::accept_loop(Arc::new(l), shared, 1).await;
        });
        Rig { port, cert, _home: home, _self_signed: self_signed }
    }

    fn host(&self, label: &str) -> String {
        format!("{label}.{PRIMARY}.k2.dev")
    }

    async fn connect(&self, label: &str) -> tokio_rustls::client::TlsStream<TcpStream> {
        use rustls_pemfile::Item;
        let mut roots = rustls::RootCertStore::empty();
        for item in rustls_pemfile::read_all(&mut self.cert.as_bytes()) {
            if let Ok(Item::X509Certificate(der)) = item {
                roots.add(der).unwrap();
            }
        }
        let cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tcp = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from(self.host(label)).unwrap();
        connector.connect(name, tcp).await.expect("client handshake")
    }

    /// One request on its own connection; the whole answer.
    async fn ask(&self, label: &str, head_lines: &str, method_path: &str) -> String {
        let mut c = self.connect(label).await;
        let req = format!("{method_path} HTTP/1.1\r\nHost: {}\r\n{head_lines}Connection: close\r\n\r\n", self.host(label));
        c.write_all(req.as_bytes()).await.unwrap();
        read_all(&mut c).await
    }

    async fn get(&self, label: &str, cookie: Option<&str>, path: &str) -> String {
        let lines = cookie.map_or(String::new(), |c| format!("Cookie: {c}\r\n"));
        self.ask(label, &lines, &format!("GET {path}")).await
    }

    async fn post_pin(&self, label: &str, origin: Option<&str>, form: &str) -> String {
        let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
        let lines = format!("{origin}Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n", form.len());
        let mut c = self.connect(label).await;
        let req = format!("POST /_k2/site/pin HTTP/1.1\r\nHost: {}\r\n{lines}Connection: close\r\n\r\n{form}", self.host(label));
        c.write_all(req.as_bytes()).await.unwrap();
        read_all(&mut c).await
    }

    /// Open `label` with a one-time link: the cookie pair, ready to send.
    async fn sign_in(&self, label: &str, user: &str) -> String {
        let code = db(|c| sv::mint_open_code(c, label, "/", user)).unwrap();
        let r = self.get(label, None, &format!("/_k2/site/open?code={}", code.code)).await;
        assert!(r.starts_with("HTTP/1.1 303"), "{r}");
        cookie_of(&r, sv::COOKIE_SITE)
    }

    async fn pin_in(&self, label: &str, pin: &str) -> String {
        let r = self.post_pin(label, Some(&format!("https://{}", self.host(label))), &format!("pin={pin}&next=%2F")).await;
        assert!(r.starts_with("HTTP/1.1 303"), "{r}");
        cookie_of(&r, sv::COOKIE_PIN)
    }
}

async fn read_all<S: AsyncRead + Unpin>(s: &mut S) -> String {
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await.expect("the answer ends");
    String::from_utf8_lossy(&out).to_string()
}

fn cookie_of(answer: &str, name: &str) -> String {
    let line = answer.lines().find(|l| l.starts_with(&format!("Set-Cookie: {name}="))).unwrap_or_else(|| panic!("no {name} in {answer}"));
    assert!(line.contains("HttpOnly") && line.contains("Secure") && line.contains("SameSite=Lax") && line.contains("Path=/"), "{line}");
    assert!(!line.contains("Domain"), "host-only: {line}");
    line.trim_start_matches("Set-Cookie: ").split(';').next().unwrap().to_string()
}

fn set_pin(label: &str, pin: &str) {
    let h = sv::hash_pin(pin).unwrap();
    db(|c| sv::set_pin(c, label, &h, None, "test")).unwrap();
}

/// Read until the head of one response ended; its text.
async fn read_head<S: AsyncRead + Unpin>(s: &mut S) -> String {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    while !out.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut b)).await.expect("head in time").unwrap();
        assert!(n > 0, "closed before the head ended: {}", String::from_utf8_lossy(&out));
        out.push(b[0]);
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Wait for the socket to be cut by K2.
async fn assert_cut<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S) {
    let cut = tokio::time::timeout(Duration::from_secs(10), async {
        let mut b = [0u8; 64];
        loop {
            match s.read(&mut b).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await;
    assert!(cut.is_ok(), "K2 must cut this connection");
}

// ── classification ────────────────────────────────────────────────────

#[test]
fn reserved_labels_and_real_apps_are_open_and_a_folder_app_is_covered() {
    db(|c| {
        for r in k2_core::skin::RESERVED_NESTED_LABELS {
            assert_eq!(classify_with(c, r.label, "127.0.0.1:1").unwrap(), Site::Open, "{}", r.label);
        }
        c.execute(
            "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('sg-1', 'p', 'sg-real-app', '(app)', '/t', 41001, 'tunnel', 'running')",
            [],
        )
        .unwrap();
        insert_app(c, "sg-real-app", "sg-1", "seed", "guarded", 41001);
        assert_eq!(classify_with(c, "sg-real-app", "127.0.0.1:41001").unwrap(), Site::Open, "an App's own logins decide");
        assert_eq!(classify_with(c, "sg-taken-label", "127.0.0.1:41001").unwrap(), Site::Open, "a label pointing at an App's front port is that App's");

        c.execute(
            "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('sg-2', 'p', 'sg-folder-app', '(app)', '/t', 41002, 'tunnel', 'running')",
            [],
        )
        .unwrap();
        insert_app(c, "sg-folder-app", "sg-2", "folder", "open", 41002);
        let Site::Gated(g) = classify_with(c, "sg-folder-app", "127.0.0.1:9").unwrap() else {
            panic!("a new `k2 publish run --cmd` folder App has no logins: private by default");
        };
        assert_eq!(g.upstream, "127.0.0.1:41002", "through its front port");
        sv::set_public(c, "sg-folder-app", "launch", "o").unwrap();
        assert_eq!(classify_with(c, "sg-folder-app", "127.0.0.1:9").unwrap(), Site::Public { label: "sg-folder-app".into() });
    });
}

fn insert_app(c: &rusqlite::Connection, id: &str, service: &str, source: &str, path: &str, front: i64) {
    c.execute(
        "INSERT INTO apps (app_id, service_id, owner_workspace, path, source, isolation, front_port, state, created_by, created_at, updated_at)
         VALUES (?1, ?2, 'p', ?3, ?4, 'container', ?5, 'running', 'test', 0, 0)",
        params![id, service, path, source, front],
    )
    .unwrap();
}

#[test]
fn no_row_is_gated_and_public_and_dev_public_differ() {
    db(|c| {
        let Site::Gated(g) = classify_with(c, "sg-no-row", "127.0.0.1:5").unwrap() else {
            panic!("no row = private");
        };
        assert!(!g.dev);
        sv::set_public(c, "sg-pub", "why", "o").unwrap();
        assert!(matches!(classify_with(c, "sg-pub", "127.0.0.1:5").unwrap(), Site::Public { .. }));
        sv::set_dev(c, "sg-pub", true, None, "o").unwrap();
        let Site::Gated(g) = classify_with(c, "sg-pub", "127.0.0.1:5").unwrap() else {
            panic!("public dev is parsed: its responses say dev");
        };
        assert!(g.dev);
    });
}

// ── private ───────────────────────────────────────────────────────────

#[tokio::test]
async fn private_sends_nothing_upstream_without_a_session() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-priv", port)]).await;

    let r = rig.get("sg-priv", None, "/secret").await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(r.contains("This site is private."), "{r}");
    assert!(!r.contains("SITE"), "{r}");

    // A forged cookie, a WebSocket without one, a POST, and garbage: same.
    assert!(rig.get("sg-priv", Some("__Host-k2_site=forged"), "/").await.starts_with("HTTP/1.1 401"));
    let ws = rig.ask("sg-priv", "Upgrade: websocket\r\nSec-WebSocket-Key: x\r\nSec-WebSocket-Version: 13\r\n", "GET /hmr").await;
    assert!(ws.starts_with("HTTP/1.1 401"), "{ws}");
    let post = rig.ask("sg-priv", "Content-Length: 0\r\n", "POST /api").await;
    assert!(post.starts_with("HTTP/1.1 401"), "{post}");
    let mut c = rig.connect("sg-priv").await;
    c.write_all(b"\x00\x01not http\r\n\r\n").await.unwrap();
    assert!(read_all(&mut c).await.starts_with("HTTP/1.1 400"));

    assert_eq!(seen.accepts.load(Ordering::SeqCst), 0, "a visitor who fails the gate never causes a connection to the site");
}

#[tokio::test]
async fn one_time_link_signs_in_strips_the_cookie_and_is_single_use() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-open", port)]).await;

    let code = db(|c| sv::mint_open_code(c, "sg-open", "/docs", sv::USER_OWNER_TOKEN)).unwrap();
    let r = rig.get("sg-open", None, &format!("/_k2/site/open?code={}", code.code)).await;
    assert!(r.starts_with("HTTP/1.1 303"), "{r}");
    assert!(r.contains("Location: /docs\r\n"), "{r}");
    let cookie = cookie_of(&r, sv::COOKIE_SITE);
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 0, "the landing never touches the site");

    let again = rig.get("sg-open", None, &format!("/_k2/site/open?code={}", code.code)).await;
    assert!(again.starts_with("HTTP/1.1 401") && again.contains("expired"), "a reused code fails: {again}");

    let expired = db(|c| sv::mint_open_code(c, "sg-open", "/", sv::USER_OWNER_TOKEN)).unwrap();
    db(|c| c.execute("UPDATE site_open_codes SET expires_at = unixepoch() - 1", []).unwrap());
    let r = rig.get("sg-open", None, &format!("/_k2/site/open?code={}", expired.code)).await;
    assert!(r.contains("expired"), "an expired code fails: {r}");

    // The site is reached, with K2's cookie gone and the site's own kept.
    let lines = format!("Cookie: theme=dark; {cookie}; sid=1\r\n");
    let ok = rig.ask("sg-open", &lines, "GET /docs").await;
    assert!(ok.contains("200 OK") && ok.ends_with("SITE"), "{ok}");
    let seen_head = seen.requests.lock().unwrap().last().cloned().unwrap();
    assert!(seen_head.contains("Cookie: theme=dark; sid=1\r\n"), "{seen_head}");
    assert!(!seen_head.contains("k2_site"), "the site never sees K2's session: {seen_head}");

    // A session is this label's only.
    db(|c| sv::set_private(c, "sg-open-other", "o")).unwrap();
    let (port2, seen2) = spawn_site().await;
    let mut targets = HashMap::new();
    targets.insert("sg-open".to_string(), format!("127.0.0.1:{port}"));
    targets.insert("sg-open-other".to_string(), format!("127.0.0.1:{port2}"));
    subdomains::store(SubdomainMap { primary: PRIMARY.into(), targets });
    assert!(rig.get("sg-open-other", Some(&cookie), "/").await.starts_with("HTTP/1.1 401"));
    assert_eq!(seen2.accepts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_code_for_another_label_does_nothing_here() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-code-a", port)]).await;
    let code = db(|c| sv::mint_open_code(c, "sg-code-b", "/", sv::USER_OWNER_TOKEN)).unwrap();
    let r = rig.get("sg-code-a", None, &format!("/_k2/site/open?code={}", code.code)).await;
    assert!(r.contains("expired"), "{r}");
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 0);
    assert!(db(|c| sv::redeem_open_code(c, "sg-code-b", &code.code)).unwrap().is_some(), "still unspent for its own label");
}

#[tokio::test]
async fn a_demoted_admin_loses_access_and_a_member_never_had_it() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-roles", port)]).await;
    k2_core::connect_users::add_user("sg-adm", "correct-horse-battery").unwrap();
    k2_core::connect_users::set_role("sg-adm", k2_core::connect_users::Role::Admin).unwrap();
    let cookie = rig.sign_in("sg-roles", "sg-adm").await;
    assert!(rig.get("sg-roles", Some(&cookie), "/").await.ends_with("SITE"));

    // A live keep-alive connection, and a raw socket, for the same session.
    let mut live = rig.connect("sg-roles").await;
    let h = format!("GET / HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\n\r\n", rig.host("sg-roles"));
    live.write_all(h.as_bytes()).await.unwrap();
    assert!(read_head(&mut live).await.contains("200 OK"));
    let mut ws = rig.connect("sg-roles").await;
    let h = format!("GET /hmr HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n", rig.host("sg-roles"));
    ws.write_all(h.as_bytes()).await.unwrap();
    assert!(read_head(&mut ws).await.contains("101"));

    k2_core::connect_users::set_role("sg-adm", k2_core::connect_users::Role::Member).unwrap();
    assert!(rig.get("sg-roles", Some(&cookie), "/").await.starts_with("HTTP/1.1 401"), "a new connection at once");
    assert_cut(&mut ws).await;
    assert_cut(&mut live).await;

    // A code a Member holds opens nothing.
    let code = db(|c| sv::mint_open_code(c, "sg-roles", "/", "sg-adm")).unwrap();
    let r = rig.get("sg-roles", None, &format!("/_k2/site/open?code={}", code.code)).await;
    assert!(r.contains("expired"), "{r}");
}

#[tokio::test]
async fn a_removed_user_and_a_revoked_session_are_out() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-gone", port)]).await;
    k2_core::connect_users::add_user("sg-tmp", "correct-horse-battery").unwrap();
    k2_core::connect_users::set_role("sg-tmp", k2_core::connect_users::Role::Admin).unwrap();
    let cookie = rig.sign_in("sg-gone", "sg-tmp").await;
    assert!(rig.get("sg-gone", Some(&cookie), "/").await.ends_with("SITE"));
    k2_core::connect_users::remove_user("sg-tmp").unwrap();
    assert!(rig.get("sg-gone", Some(&cookie), "/").await.starts_with("HTTP/1.1 401"));

    let cookie = rig.sign_in("sg-gone", sv::USER_OWNER_TOKEN).await;
    assert!(rig.get("sg-gone", Some(&cookie), "/").await.ends_with("SITE"));
    db(|c| sv::revoke_sessions(c, "sg-gone")).unwrap();
    assert!(rig.get("sg-gone", Some(&cookie), "/").await.starts_with("HTTP/1.1 401"), "--revoke-all ends it");
}

#[tokio::test]
async fn every_request_on_a_keep_alive_connection_passes_the_gate() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-ka", port)]).await;
    let cookie = rig.sign_in("sg-ka", sv::USER_OWNER_TOKEN).await;
    let mut c = rig.connect("sg-ka").await;
    let host = rig.host("sg-ka");
    c.write_all(format!("GET /1 HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\n\r\n").as_bytes()).await.unwrap();
    assert!(read_head(&mut c).await.contains("200 OK"));
    let mut b = [0u8; 4];
    c.read_exact(&mut b).await.unwrap();
    assert_eq!(seen.requests.lock().unwrap().len(), 1);

    // Same connection, no cookie: refused, connection over, never forwarded.
    c.write_all(format!("GET /steal HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).await.unwrap();
    let r = read_all(&mut c).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert_eq!(seen.requests.lock().unwrap().len(), 1, "the second request never reached the site");

    // Pipelined behind a good one: the same.
    let mut c = rig.connect("sg-ka").await;
    c.write_all(format!("GET /a HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\n\r\nGET /b HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).await.unwrap();
    let r = read_all(&mut c).await;
    assert!(r.contains("200 OK") && r.contains("401 Unauthorized"), "{r}");
    let heads = seen.requests.lock().unwrap().clone();
    assert!(!heads.iter().any(|h| h.starts_with("GET /b")), "{heads:?}");
}

#[test]
fn the_private_page_is_pinned() {
    let got = String::from_utf8(pages::private_page("docs", false, false)).unwrap();
    let want = format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nX-Robots-Tag: noindex\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'\r\nContent-Length: {len}\r\nContent-Security-Policy: frame-ancestors 'self'\r\nX-Frame-Options: SAMEORIGIN\r\nConnection: close\r\n\r\n{body}",
        len = PRIVATE_BODY.len(),
        body = PRIVATE_BODY
    );
    assert_eq!(got, want);
    assert!(!got.contains("http://") && !got.contains("https://") && !got.contains("src="), "it loads nothing external");
    assert!(!got.to_ascii_lowercase().contains("<form"), "no form on a private site");
    assert!(!got.contains(env!("CARGO_PKG_VERSION")), "no K2 version");
}

const PRIVATE_BODY: &str = concat!(
    "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Private site</title><style>",
    "body{font:16px/1.5 system-ui,sans-serif;margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#111;color:#eee;padding:16px;box-sizing:border-box}main{max-width:26rem}h1{font-size:1.25rem;margin:0 0 .5rem}p{margin:.5rem 0}code{background:#222;padding:.1rem .3rem;border-radius:3px}input{font:inherit;padding:.5rem;width:100%;box-sizing:border-box;margin:.5rem 0;background:#222;color:#eee;border:1px solid #555;border-radius:4px}button,a.b{font:inherit;padding:.5rem 1rem;background:#eee;color:#111;border:0;border-radius:4px;text-decoration:none;display:inline-block}.e{color:#f99}",
    "</style></head><body><main><h1>This site is private.</h1><p>Open it from K2: Published → Open, or <code>k2 publish open docs</code>.</p></main></body></html>\n"
);

// ── PIN ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn pin_page_first_then_the_right_pin_opens_the_site() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-pin", port)]).await;
    set_pin("sg-pin", "hunter22");

    let r = rig.get("sg-pin", None, "/app?x=1").await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(r.contains("needs a PIN") && r.contains("action=\"/_k2/site/pin\"") && r.contains("value=\"/app?x=1\""), "{r}");
    assert!(!r.contains("http://") && !r.contains("src="), "the PIN page loads nothing: {r}");

    let origin = format!("https://{}", rig.host("sg-pin"));
    let wrong = rig.post_pin("sg-pin", Some(&origin), "pin=nothunter&next=%2Fapp").await;
    assert!(wrong.starts_with("HTTP/1.1 401") && wrong.contains("That PIN isn&#39;t right.") && !wrong.contains("nothunter"), "{wrong}");

    let right = rig.post_pin("sg-pin", Some(&origin), "pin=hunter22&next=%2Fapp%3Fx%3D1").await;
    assert!(right.starts_with("HTTP/1.1 303"), "{right}");
    assert!(right.contains("Location: /app?x=1\r\n"), "{right}");
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 0, "nothing reached the site yet");
    let cookie = cookie_of(&right, sv::COOKIE_PIN);
    assert!(right.contains(&format!("Max-Age={}", 7 * 86_400)), "seven days by default: {right}");

    let ok = rig.get("sg-pin", Some(&cookie), "/app").await;
    assert!(ok.ends_with("SITE"), "{ok}");
    let head = seen.requests.lock().unwrap().last().cloned().unwrap();
    assert!(!head.contains("k2_pin"), "{head}");

    // `next` can't send anyone elsewhere.
    let r = rig.post_pin("sg-pin", Some(&origin), "pin=hunter22&next=%2F%2Fevil.example").await;
    assert!(r.contains("Location: /\r\n"), "{r}");

    // The audit has the label and the outcome, never the PIN.
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).unwrap();
    assert!(audit.contains("\"site-pin\"") && audit.contains("sg-pin") && audit.contains("wrong"), "{audit}");
    assert!(!audit.contains("hunter22") && !audit.contains("nothunter"), "{audit}");
}

#[tokio::test]
async fn a_wrong_origin_is_refused_before_a_try_is_counted() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-origin", port)]).await;
    set_pin("sg-origin", "hunter22");
    for origin in [Some("https://evil.example"), None, Some("http://sg-origin.rosson.k2.dev")] {
        let r = rig.post_pin("sg-origin", origin, "pin=hunter22").await;
        assert!(r.starts_with("HTTP/1.1 403"), "{origin:?}: {r}");
    }
    let tries: i64 = db(|c| c.query_row("SELECT COUNT(*) FROM site_pin_limits WHERE scope = 'site:sg-origin'", [], |r| r.get(0)).unwrap());
    assert_eq!(tries, 0);
}

#[tokio::test]
async fn a_pin_change_or_a_move_to_private_cuts_the_friend() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-friend", port)]).await;
    set_pin("sg-friend", "hunter22");
    let cookie = rig.pin_in("sg-friend", "hunter22").await;
    assert!(rig.get("sg-friend", Some(&cookie), "/").await.ends_with("SITE"));

    // A live hot-reload socket for the friend.
    let mut ws = rig.connect("sg-friend").await;
    let h = format!("GET /hmr HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n", rig.host("sg-friend"));
    ws.write_all(h.as_bytes()).await.unwrap();
    assert!(read_head(&mut ws).await.contains("101"));
    ws.write_all(b"ping").await.unwrap();
    let mut b = [0u8; 4];
    ws.read_exact(&mut b).await.unwrap();
    assert_eq!(&b, b"ping");

    set_pin("sg-friend", "hunter33");
    assert_cut(&mut ws).await;
    let r = rig.get("sg-friend", Some(&cookie), "/").await;
    assert!(r.contains("needs a PIN"), "the old PIN's session is over: {r}");

    let cookie = rig.pin_in("sg-friend", "hunter33").await;
    db(|c| sv::set_private(c, "sg-friend", "o")).unwrap();
    let r = rig.get("sg-friend", Some(&cookie), "/").await;
    assert!(r.contains("This site is private."), "{r}");
}

#[tokio::test]
async fn the_address_limit_locks_only_that_address_and_a_lock_spares_live_sessions() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-limit", port)]).await;
    set_pin("sg-limit", "hunter22");
    let cookie = rig.pin_in("sg-limit", "hunter22").await;

    // The listener's client address is unknown here (no PROXY header), so
    // only the site's limit applies through this door; drive it to its lock.
    let origin = format!("https://{}", rig.host("sg-limit"));
    db(|c| {
        c.execute(
            "INSERT OR REPLACE INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES ('site:sg-limit', 29, unixepoch(), 0)",
            [],
        )
        .unwrap()
    });
    let r = rig.post_pin("sg-limit", Some(&origin), "pin=wrong-one").await;
    assert!(r.starts_with("HTTP/1.1 401"), "the 30th wrong try is still a try: {r}");
    let notices = crate::apps::runtime_notes::captured();
    assert!(
        notices.iter().any(|n| matches!(n, crate::apps::runtime_notes::Captured::Notice { body, .. } if body.contains("sg-limit"))),
        "the owner gets one notice: {notices:?}"
    );

    let r = rig.post_pin("sg-limit", Some(&origin), "pin=hunter22").await;
    assert!(r.starts_with("HTTP/1.1 429") && r.contains("Retry-After: "), "even the right PIN waits now: {r}");
    assert!(rig.get("sg-limit", Some(&cookie), "/").await.ends_with("SITE"), "a lock can't shut out people already in");
}

#[tokio::test]
async fn a_form_too_long_or_chunked_is_refused_unread() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-body", port)]).await;
    set_pin("sg-body", "hunter22");
    let origin = format!("Origin: https://{}\r\n", rig.host("sg-body"));
    let r = rig.ask("sg-body", &format!("{origin}Content-Length: 5000\r\n"), "POST /_k2/site/pin").await;
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    let r = rig.ask("sg-body", &format!("{origin}Transfer-Encoding: chunked\r\n"), "POST /_k2/site/pin").await;
    assert!(r.starts_with("HTTP/1.1 400"), "{r}");
}

// ── public ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_public_label_is_todays_raw_splice_and_a_move_to_private_cuts_it() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-public", port)]).await;
    db(|c| sv::set_public(c, "sg-public", "launch", "o")).unwrap();

    // Raw: a request the parser would refuse still goes through untouched, and
    // `/_k2/site/` is the site's own.
    let r = rig.get("sg-public", None, "/_k2/site/open").await;
    assert!(r.ends_with("SITE"), "{r}");
    let mut c = rig.connect("sg-public").await;
    let raw = format!("GET / HTTP/1.1\r\nHost: {}\r\nX-Forwarded-For: 1.2.3.4\r\nCookie: __Host-k2_site=theirs\r\n\r\n", rig.host("sg-public"));
    c.write_all(raw.as_bytes()).await.unwrap();
    assert!(read_head(&mut c).await.contains("200 OK"));
    let head = seen.requests.lock().unwrap().last().cloned().unwrap();
    assert!(head.contains("X-Forwarded-For: 1.2.3.4") && head.contains("__Host-k2_site=theirs"), "not parsed, not rewritten: {head}");

    // Still open when the site goes private: cut.
    db(|c2| sv::set_private(c2, "sg-public", "o")).unwrap();
    assert_cut(&mut c).await;
    assert!(rig.get("sg-public", None, "/").await.starts_with("HTTP/1.1 401"));
}

/// PA-13 end to end: a database from before 0.46.7, with public sites both in
/// the service table and known only to the routing map. Nothing may change.
#[tokio::test]
async fn an_upgraded_box_keeps_every_site_public_through_the_gate() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-up-svc", port), ("sg-up-cp", port), ("sg-up-new", port)]).await;
    db(|c| {
        // What the migration and the pass leave: the same calls the boot makes.
        c.execute(
            "INSERT OR REPLACE INTO site_visibility_meta (key, value) VALUES ('grandfather', 'pending')",
            [],
        )
        .unwrap();
        c.execute("DELETE FROM site_visibility_meta WHERE key = 'grandfather_primary'", []).unwrap();
        c.execute(
            "INSERT INTO site_visibility (label, mode, set_by, public_reason) VALUES ('sg-up-svc', 'public', 'upgrade', 'public before 0.46.7')",
            [],
        )
        .unwrap();
        sv::settle_grandfather_at_boot(c, PRIMARY).unwrap();
        // A label made after the upgrade, before the first fetch.
        sv::note_new_label(c, "sg-up-new", "create").unwrap();
    });
    // The real path: the routing map goes through the hook before it routes.
    let map = subdomains::current();
    SETTLED.store(false, Ordering::Relaxed);
    on_map(&map, true);

    for label in ["sg-up-svc", "sg-up-cp"] {
        let r = rig.get(label, None, "/").await;
        assert!(r.ends_with("SITE"), "{label} stays public: {r}");
    }
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 2);
    let r = rig.get("sg-up-new", None, "/").await;
    assert!(r.starts_with("HTTP/1.1 401"), "made after the upgrade: private: {r}");
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 2);
    assert_eq!(db(|c| sv::get(c, "sg-up-cp")).unwrap().unwrap().public_reason.as_deref(), Some(sv::REASON_BEFORE));
    assert!(!db(sv::grandfather_pending).unwrap(), "settled by the fetched map");
}

// ── dev ───────────────────────────────────────────────────────────────

static TOUCHES: AtomicU64 = AtomicU64::new(0);
fn count_touch(label: &str) {
    if label.starts_with("sg-dev") {
        TOUCHES.fetch_add(1, Ordering::SeqCst);
    }
}
static WOKEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn test_wake(label: &str) -> DevWake {
    WOKEN.lock().unwrap().push(label.to_string());
    match label {
        "sg-dev-starting" => DevWake::Starting,
        "sg-dev-failed" => DevWake::Failed("port never opened".into()),
        _ => DevWake::Up,
    }
}

#[tokio::test]
async fn a_dev_site_says_dev_on_every_answer_and_the_socket_passes_the_gate() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-dev-a", port), ("sg-dev-starting", port), ("sg-dev-failed", port)]).await;
    dev::install(DevHooks { touch: count_touch, wake: test_wake });
    db(|c| {
        for l in ["sg-dev-a", "sg-dev-starting", "sg-dev-failed"] {
            sv::set_dev(c, l, true, None, "o").unwrap();
        }
    });

    // Refused: K2's page says dev, and a refused visitor wakes nothing.
    let woken_before = WOKEN.lock().unwrap().len();
    let touched_before = TOUCHES.load(Ordering::SeqCst);
    let r = rig.get("sg-dev-a", None, "/").await;
    assert!(r.starts_with("HTTP/1.1 401") && r.contains("X-K2-Mode: dev\r\n") && r.contains("<title>Dev server: "), "{r}");
    assert_eq!(WOKEN.lock().unwrap().len(), woken_before, "a visitor who fails never wakes it");
    assert_eq!(TOUCHES.load(Ordering::SeqCst), touched_before, "nor counts as use");
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 0);

    // In: every response says dev, a 101 too, and the socket counts as use.
    let cookie = rig.sign_in("sg-dev-a", sv::USER_OWNER_TOKEN).await;
    let r = rig.get("sg-dev-a", Some(&cookie), "/").await;
    assert!(r.contains("X-K2-Mode: dev\r\nX-Robots-Tag: noindex\r\n") && r.ends_with("SITE"), "{r}");
    assert!(TOUCHES.load(Ordering::SeqCst) > touched_before);
    assert!(WOKEN.lock().unwrap().iter().any(|l| l == "sg-dev-a"));

    let mut ws = rig.connect("sg-dev-a").await;
    let h = format!("GET /hmr HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n", rig.host("sg-dev-a"));
    ws.write_all(h.as_bytes()).await.unwrap();
    let head = read_head(&mut ws).await;
    assert!(head.contains("101") && head.contains("X-K2-Mode: dev\r\n"), "{head}");
    let before = TOUCHES.load(Ordering::SeqCst);
    let mut b = [0u8; 2];
    for _ in 0..6 {
        ws.write_all(b"hi").await.unwrap();
        ws.read_exact(&mut b).await.unwrap();
        tokio::time::sleep(DEV_TOUCH).await;
    }
    assert!(TOUCHES.load(Ordering::SeqCst) > before, "a spliced socket carrying data keeps the site in use");

    // Waking: K2's page, no dial.
    let accepts = seen.accepts.load(Ordering::SeqCst);
    let c2 = rig.sign_in("sg-dev-starting", sv::USER_OWNER_TOKEN).await;
    let r = rig.get("sg-dev-starting", Some(&c2), "/").await;
    assert!(r.starts_with("HTTP/1.1 503") && r.contains("Starting the dev server") && r.contains("X-K2-Mode: dev\r\n") && r.contains("Refresh: 2"), "{r}");
    let c3 = rig.sign_in("sg-dev-failed", sv::USER_OWNER_TOKEN).await;
    let r = rig.get("sg-dev-failed", Some(&c3), "/").await;
    assert!(r.starts_with("HTTP/1.1 502") && !r.contains("port never opened"), "why stays in the log: {r}");
    assert_eq!(seen.accepts.load(Ordering::SeqCst), accepts);
}

#[tokio::test]
async fn the_dev_notice_shows_once_per_pin_session_and_never_for_the_owner() {
    let (port, _seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-dev-notice", port)]).await;
    db(|c| sv::set_dev(c, "sg-dev-notice", true, None, "o")).unwrap();
    set_pin("sg-dev-notice", "hunter22");
    let friend = rig.pin_in("sg-dev-notice", "hunter22").await;
    let page = format!("Cookie: {friend}\r\nSec-Fetch-Dest: document\r\n");

    let r = rig.ask("sg-dev-notice", &page, "GET /app").await;
    assert!(r.starts_with("HTTP/1.1 200") && r.contains("This is a dev server for sg-dev-notice.") && r.contains("/_k2/site/continue?next=/app"), "{r}");
    // A module fetch before Continue isn't a page view.
    let module = format!("Cookie: {friend}\r\nSec-Fetch-Dest: script\r\n");
    assert!(rig.ask("sg-dev-notice", &module, "GET /a.js").await.ends_with("SITE"));

    let r = rig.ask("sg-dev-notice", &page, "GET /_k2/site/continue?next=/app").await;
    assert!(r.starts_with("HTTP/1.1 303") && r.contains("Location: /app\r\n"), "{r}");
    let r = rig.ask("sg-dev-notice", &page, "GET /app").await;
    assert!(r.ends_with("SITE"), "once: {r}");

    let owner = rig.sign_in("sg-dev-notice", sv::USER_OWNER_TOKEN).await;
    let r = rig.ask("sg-dev-notice", &format!("Cookie: {owner}\r\nSec-Fetch-Dest: document\r\n"), "GET /app").await;
    assert!(r.ends_with("SITE"), "the owner in a K2 tab doesn't get it: {r}");
}

#[tokio::test]
async fn public_dev_is_parsed_and_open_and_a_public_sites_k2_paths_are_its_own() {
    let (port, seen) = spawn_site().await;
    let rig = Rig::new(&[("sg-dev-public", port)]).await;
    db(|c| {
        sv::set_public(c, "sg-dev-public", "demo", "o").unwrap();
        sv::set_dev(c, "sg-dev-public", true, None, "o").unwrap();
    });
    let r = rig.get("sg-dev-public", None, "/_k2/site/open").await;
    assert!(r.ends_with("SITE") && r.contains("X-K2-Mode: dev\r\n"), "{r}");
    assert_eq!(seen.accepts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_dead_site_is_k2s_502_only_after_the_gate() {
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let rig = Rig::new(&[("sg-dead", dead)]).await;
    assert!(rig.get("sg-dead", None, "/").await.starts_with("HTTP/1.1 401"), "no session: the gate first");
    let cookie = rig.sign_in("sg-dead", sv::USER_OWNER_TOKEN).await;
    assert!(rig.get("sg-dead", Some(&cookie), "/").await.starts_with("HTTP/1.1 502"));
}

// ── doors ─────────────────────────────────────────────────────────────

#[test]
fn a_custom_name_is_a_door_the_gate_does_not_cover() {
    let _home = crate::test_support::TempHome::new();
    db(|c| {
        c.execute(
            "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('sg-3', 'p', 'sg-door', '(app)', '/t', 41003, 'tunnel', 'running')",
            [],
        )
        .unwrap();
        insert_app(c, "sg-door", "sg-3", "folder", "open", 41003);
        c.execute(
            "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('sg-4', 'p', 'sg-door-real', '(app)', '/t', 41004, 'tunnel', 'running')",
            [],
        )
        .unwrap();
        insert_app(c, "sg-door-real", "sg-4", "seed", "guarded", 41004);
    });
    // A private folder App: no custom name goes past the gate.
    let why = custom_name_refusal("sg-door", "shop.example.com").expect("private: refused");
    assert!(why.starts_with("visibility_door_unsupported:") && why.contains("shop.example.com"), "{why}");
    // On purpose public: allowed. A real App: never this table's.
    db(|c| sv::set_public(c, "sg-door", "launch", "o")).unwrap();
    assert_eq!(custom_name_refusal("sg-door", "shop.example.com"), None);
    assert_eq!(custom_name_refusal("sg-door-real", "shop.example.com"), None);
    assert_eq!(door_refusal("sg-door"), None, "no custom name yet: the gate covers every door");
}

#[test]
fn e2e_off_refuses_private_and_pin() {
    let _home = crate::test_support::TempHome::new();
    let _e2e = k2_core::test_env::EnvVar::set("K2_E2E", "0");
    let why = door_refusal("anything").expect("E2E off");
    assert!(why.starts_with("visibility_door_unsupported:") && why.contains("E2E"), "{why}");
}
