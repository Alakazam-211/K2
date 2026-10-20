//! PRD `prd-boot-status-ghost-v1.md` (0.46.3) — `/boot-status` answers
//! three views from one route:
//!
//! - **full**: loopback, or a valid owner token / agent passport / `k2sk_`
//!   owner or status key / unflagged Connect-login session (Bearer,
//!   `?token=` or the `k2_session` cookie): today's body + `view` +
//!   `protocolMin`;
//! - **compat** (the 0.46.3 default for everyone else): phase, protocol
//!   range, instanceId + `version`, `features`, `serverId`, `publicHost`,
//!   `airgap.enabled` — every field old phones/desktops read without a
//!   token, unchanged;
//! - **public** (`bootStatusPublic = ghost`): phase, protocol range,
//!   instanceId only.
//!
//! Plus the `/ping` ghost banner, the headers on every view, the setting
//! (Owner/Admin), status keys (S2) and — Rosson's condition for 0.46.3 —
//! the Settings → General update handshake across a REAL daemon restart
//! (a spawned `k2-daemon` in a temp HOME is killed and respawned; the
//! running production daemon is never touched).
//!
//! Drives REAL HTTP through the REAL dispatcher (`test_harness::start`
//! binds a loopback listener and a tunnel-ingress listener). ISOLATION:
//! every in-process test serializes on `TEST_LOCK` and points `$HOME` at
//! a fresh tempdir.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::connect_users::{self, Role};
use k2_core::session::SessionId;
use k2_daemon::session_token::{self, HookPrincipal};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-boot-status-views-1";

struct Resp {
    status: u16,
    body: String,
    headers: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().skip(1).find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): status {} body {}", self.status, self.body))
    }
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>, extra: &[&str]) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("set read timeout");
    let mut extra_lines = String::new();
    for h in extra {
        extra_lines.push_str(h);
        extra_lines.push_str("\r\n");
    }
    let req = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\n{extra_lines}Content-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{extra_lines}\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read response: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (headers, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no HTTP response for {method} {path}: {text:?}"));
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line: {headers:?}"));
    Resp { status, body: body.to_string(), headers: headers.to_string() }
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let _home = k2_core::test_env::TempHome::new();
    let _ = k2_core::db::init_for_tests();
    f();
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn set_mode(mode: &str) {
    k2_core::app_settings::update(serde_json::json!({ "bootStatusPublic": mode })).expect("set bootStatusPublic");
}

fn bearer(tok: &str) -> String {
    format!("Authorization: Bearer {tok}")
}

fn boot(port: u16, extra: &[&str]) -> Resp {
    let r = http(port, "GET", "/boot-status", None, extra);
    assert_eq!(r.status, 200, "/boot-status is always 200; headers={} body={}", r.headers, r.body);
    r
}

/// The exact bytes of the ghost body (§2.2 key order).
fn public_bytes(instance_id: &str) -> String {
    format!(r#"{{"view":"public","phase":"ready","protocol":1,"protocolMin":1,"instanceId":"{instance_id}"}}"#)
}

/// Every view carries `Cache-Control: no-store` + `Vary: Authorization, Cookie`.
fn assert_view_headers(r: &Resp, what: &str) {
    assert_eq!(r.header("Cache-Control").as_deref(), Some("no-store"), "{what}: {}", r.headers);
    assert_eq!(r.header("Vary").as_deref(), Some("Authorization, Cookie"), "{what}: {}", r.headers);
    assert_eq!(r.header("Content-Type").as_deref(), Some("application/json"), "{what}");
}

/// The fields a token-less ≤0.46.2 client reads (desktop R1/R4, Companion
/// C1–C11, k2-watch W1). Compat must keep each exactly as the full body has it.
const OLD_CLIENT_FIELDS: &[&str] = &["phase", "protocol", "instanceId", "version", "features", "serverId", "publicHost"];

/// A Connect login (Member) with a live session token.
fn session_for(username: &str) -> String {
    connect_users::add_user(username, "password123").expect("add_user");
    connect_users::set_role(username, Role::Member).expect("set_role");
    connect_users::create_session(username)
}

fn passport() -> String {
    session_token::mint_session_token(
        &SessionId::new(),
        "bsv-pane",
        HookPrincipal { workspace_uuid: String::new(), agent_address: "bsv-agent".to_string() },
        session_token::CredMode::ApiKey,
        session_token::Provider::Anthropic,
    )
}

/// A real App guest pass (`k2skn_`), via the skin routes.
fn app_pass(port: u16) -> String {
    let owner = |path: &str, body: serde_json::Value| {
        let r = http(port, "POST", &format!("{path}?token={OWNER_TOKEN}"), Some(&body.to_string()), &[]);
        assert_eq!(r.status, 200, "{path}: {}", r.body);
    };
    owner("/cli/skin/users", serde_json::json!({"username": "bsvguest"}));
    owner("/cli/skin/users/password", serde_json::json!({"username": "bsvguest", "password": "s3cret-horse"}));
    let login = http(
        port,
        "POST",
        "/cli/skin/login",
        Some(&serde_json::json!({"username": "bsvguest", "password": "s3cret-horse"}).to_string()),
        &[],
    );
    assert_eq!(login.status, 200, "skin login: {}", login.body);
    let tok = login.json()["token"].as_str().expect("pass token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

fn mint_key(port: u16, body: serde_json::Value) -> (String, String) {
    let r = http(port, "POST", &format!("/cli/api-keys/create?token={OWNER_TOKEN}"), Some(&body.to_string()), &[]);
    assert_eq!(r.status, 200, "api-keys/create {body}: {}", r.body);
    let v = r.json();
    (v["id"].as_str().expect("id").to_string(), v["key"].as_str().expect("key").to_string())
}

// ─────────────────────────────────────────────────────────────────────
// Compat (the 0.46.3 default): old clients see what they always read.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compat_is_the_default_and_keeps_every_field_old_clients_read() {
    let _g = lock();
    with_temp_home(|| {
        k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        })
        .expect("seed tunnel config");
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        let full = boot(d.port, &[]);
        assert_view_headers(&full, "loopback full");
        let full_v = full.json();
        assert_eq!(full_v["view"], "full");
        assert_eq!(full_v["protocolMin"], 1);
        assert_eq!(full_v["publicHost"], "rosson.k2.dev", "seeded tunnel host");
        let features: Vec<&str> =
            full_v["features"].as_array().expect("features").iter().map(|f| f.as_str().unwrap()).collect();
        assert_eq!(features.last(), Some(&"boot-status-views-v1"), "{features:?}");

        // Tunnel, no credential: compat, byte-exact key order.
        let compat = boot(d.tunnel_port, &[]);
        assert_view_headers(&compat, "tunnel compat");
        let c = compat.json();
        let expected = format!(
            r#"{{"view":"compat","phase":"ready","protocol":1,"protocolMin":1,"instanceId":{},"version":{},"features":{},"serverId":{},"publicHost":{},"airgap":{{"enabled":{}}}}}"#,
            full_v["instanceId"], full_v["version"], full_v["features"], full_v["serverId"], full_v["publicHost"],
            full_v["airgap"]["enabled"],
        );
        assert_eq!(compat.body, expected, "compat body bytes");
        for f in OLD_CLIENT_FIELDS {
            assert_eq!(c[*f], full_v[*f], "compat keeps {f} exactly as the full body has it");
        }
        assert_eq!(c["airgap"]["enabled"], full_v["airgap"]["enabled"]);
        for hidden in ["detail", "installKind", "webClient", "scopedHooks", "ops", "api", "listen", "connectLogin"] {
            assert!(c.get(hidden).is_none(), "compat hides {hidden}: {}", compat.body);
        }
        assert!(c["airgap"].get("baked").is_none(), "compat hides airgap.baked");

        // The front-door / LAN shape (Caddy adds X-Forwarded-For) is compat too.
        let lan = boot(d.port, &["X-Forwarded-For: 203.0.113.7"]);
        assert_eq!(lan.body, compat.body, "proxied loopback = Lan = compat");

        // /ping keeps the version banner in compat, everywhere.
        let ping = http(d.tunnel_port, "GET", "/ping", None, &[]);
        assert_eq!((ping.status, ping.body.as_str()), (200, k2_daemon::BANNER));
        assert!(k2_daemon::BANNER.contains(env!("CARGO_PKG_VERSION")));
    });
}

// ─────────────────────────────────────────────────────────────────────
// Ghost behind its switch.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ghost_answers_strangers_phase_protocol_and_instance_id_only() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let iid = boot(d.port, &[]).json()["instanceId"].as_str().expect("instanceId").to_string();
        set_mode("ghost");

        let r = boot(d.tunnel_port, &[]);
        assert_eq!(r.body, public_bytes(&iid), "exact public bytes over the tunnel");
        assert_view_headers(&r, "tunnel public");
        // The vmhost worker greps this substring in compact JSON.
        assert!(r.body.contains(r#""phase":"ready""#));
        let r = boot(d.port, &["X-Forwarded-For: 203.0.113.7"]);
        assert_eq!(r.body, public_bytes(&iid), "front door / LAN");
        let r = boot(d.port, &["X-Real-IP: 203.0.113.7"]);
        assert_eq!(r.body, public_bytes(&iid));

        // Plain loopback stays full whatever the setting.
        let r = boot(d.port, &[]);
        assert_eq!(r.json()["view"], "full");
        assert!(r.json()["version"].is_string());

        // /ping: no version for a stranger in ghost; loopback + authed keep it.
        let p = http(d.tunnel_port, "GET", "/ping", None, &[]);
        assert_eq!((p.status, p.body.as_str()), (200, "k2-daemon"));
        let p = http(d.port, "GET", "/ping", None, &[]);
        assert_eq!(p.body, k2_daemon::BANNER, "loopback /ping");
        let p = http(d.tunnel_port, "GET", "/ping", None, &[&bearer(OWNER_TOKEN)]);
        assert_eq!(p.body, k2_daemon::BANNER, "authed /ping");
        // /health is already generic.
        let h = http(d.tunnel_port, "GET", "/health", None, &[]);
        assert_eq!((h.status, h.body.as_str()), (200, r#"{"status":"ok"}"#));

        // Back to compat restores the old-client fields.
        set_mode("compat");
        let r = boot(d.tunnel_port, &[]);
        assert_eq!(r.json()["view"], "compat");
        assert!(r.json()["version"].is_string());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_credential_earns_full_and_nothing_else_does() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let iid = boot(d.port, &[]).json()["instanceId"].as_str().expect("instanceId").to_string();
        // apiEnabled so /v1 answers 401 (not the surface-off 404) below.
        k2_core::app_settings::update(serde_json::json!({ "apiEnabled": true })).expect("apiEnabled");
        set_mode("ghost");

        let session = session_for("bsvmember");
        let pass = passport();
        let (_owner_key_id, owner_key) = mint_key(d.port, serde_json::json!({"label": "own"}));
        let (status_id, status_key) = mint_key(d.port, serde_json::json!({"label": "k2-watch", "scope": "status"}));
        let app = app_pass(d.port);

        let full = |extra: &[&str], path: &str, what: &str| {
            let r = http(d.tunnel_port, "GET", path, None, extra);
            assert_eq!(r.status, 200, "{what}: {}", r.body);
            let v = r.json();
            assert_eq!(v["view"], "full", "{what} must earn the full view: {}", r.body);
            assert!(v["version"].is_string() && v["installKind"].is_string(), "{what}: {}", r.body);
            assert_view_headers(&r, what);
        };
        let public = |extra: &[&str], path: &str, what: &str| {
            let r = http(d.tunnel_port, "GET", path, None, extra);
            assert_eq!(r.status, 200, "{what} gets 200 public, never 401/403: {}", r.body);
            assert_eq!(r.body, public_bytes(&iid), "{what}");
        };

        // Each credential, each way in (Bearer, ?token=, cookie).
        full(&[&bearer(OWNER_TOKEN)], "/boot-status", "owner Bearer");
        full(&[], &format!("/boot-status?token={OWNER_TOKEN}"), "owner ?token=");
        full(&[&format!("Cookie: k2_session={session}")], "/boot-status", "session cookie");
        full(&[&bearer(&session)], "/boot-status", "session Bearer");
        full(&[], &format!("/boot-status?token={session}"), "session ?token=");
        full(&[&bearer(&pass)], "/boot-status", "agent passport");
        full(&[&bearer(&owner_key)], "/boot-status", "owner API key");
        full(&[&bearer(&status_key)], "/boot-status", "status key");
        // Bearer wins over a junk ?token= (the folding order).
        full(&[&bearer(OWNER_TOKEN)], "/boot-status?token=junk", "Bearer over ?token=");

        // Not credentials here.
        public(&[], "/boot-status", "no token");
        public(&[], "/boot-status?token=", "empty token");
        public(&[&bearer("not-a-token")], "/boot-status", "junk token");
        public(&[&bearer(&"a".repeat(64))], "/boot-status", "junk 64-hex");
        public(&[&bearer("00000000-0000-0000-0000-000000000000.deadbeef")], "/boot-status", "junk passport shape");
        public(&[&bearer("k2sk_notarealkey")], "/boot-status", "junk k2sk_");
        public(&[&bearer(&app)], "/boot-status", "App guest pass (Q3)");
        public(&[&format!("Cookie: k2_skin_session={app}")], "/boot-status", "skin cookie on an operator Host");
        public(&["X-K2-Edge-Sig: v1;kid=x;sig=y"], "/boot-status", "edge signature alone");

        // Must-change-password session: 200 public (Q7), not the 403 every
        // other path gives it — /ping and /health too.
        let flagged = session_for("bsvflagged");
        connect_users::set_must_change_password("bsvflagged", true).expect("flag");
        public(&[&bearer(&flagged)], "/boot-status", "must-change-password session");
        let p = http(d.tunnel_port, "GET", "/ping", None, &[&bearer(&flagged)]);
        assert_eq!((p.status, p.body.as_str()), (200, "k2-daemon"));
        let h = http(d.tunnel_port, "GET", "/health", None, &[&bearer(&flagged)]);
        assert_eq!(h.status, 200);
        let other = http(d.tunnel_port, "GET", &format!("/cli/projects/list?token={flagged}"), None, &[]);
        assert_eq!(other.status, 403, "the gate still holds elsewhere: {}", other.body);

        // Status key: /boot-status only. Never a /v1 principal; owner key is.
        let v1 = http(d.port, "GET", "/v1/ping", None, &[&bearer(&status_key)]);
        assert_eq!(v1.status, 401, "a status key must not open /v1: {}", v1.body);
        let v1 = http(d.port, "GET", "/v1/ping", None, &[&bearer(&owner_key)]);
        assert_eq!(v1.status, 200, "owner keys keep working: {}", v1.body);
        let v1 = http(d.port, "GET", &format!("/cli/projects/list?token={status_key}"), None, &[]);
        assert!(v1.status == 401 || v1.status == 403, "a status key is not a /cli credential: {} {}", v1.status, v1.body);

        // The key lists with its scope; disable then revoke → public (never 401).
        let list = http(d.port, "GET", &format!("/cli/api-keys/list?token={OWNER_TOKEN}"), None, &[]);
        assert_eq!(list.status, 200, "{}", list.body);
        let lv = list.json();
        let rows = lv.as_array().or_else(|| lv["keys"].as_array()).expect("key rows");
        let row = rows.iter().find(|k| k["id"] == status_id.as_str()).expect("status key listed");
        assert_eq!(row["scope"], "status");
        let post = |path: &str| {
            let r = http(
                d.port,
                "POST",
                &format!("{path}?token={OWNER_TOKEN}"),
                Some(&serde_json::json!({"id": status_id}).to_string()),
                &[],
            );
            assert_eq!(r.status, 200, "{path}: {}", r.body);
        };
        post("/cli/api-keys/disable");
        public(&[&bearer(&status_key)], "/boot-status", "disabled status key");
        post("/cli/api-keys/enable");
        full(&[&bearer(&status_key)], "/boot-status", "re-enabled status key");
        post("/cli/api-keys/revoke");
        public(&[&bearer(&status_key)], "/boot-status", "revoked status key");

        // A logged-out session drops to public.
        connect_users::remove_user("bsvmember").expect("remove user");
        public(&[&bearer(&session)], "/boot-status", "session of a removed user");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_key_mint_refuses_unknown_scopes_and_extra_doors() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let create = |body: serde_json::Value| {
            http(d.port, "POST", &format!("/cli/api-keys/create?token={OWNER_TOKEN}"), Some(&body.to_string()), &[])
        };
        let r = create(serde_json::json!({"scope": "root"}));
        assert_eq!(r.status, 400, "{}", r.body);
        assert!(r.body.contains("owner|status"), "{}", r.body);
        let r = create(serde_json::json!({"scope": 7}));
        assert_eq!(r.status, 400, "{}", r.body);
        for extra in [
            serde_json::json!({"scope": "status", "workspaces": "*"}),
            serde_json::json!({"scope": "status", "allow": ["sandboxes"]}),
            serde_json::json!({"scope": "status", "anthropicKey": "sk-x"}),
            serde_json::json!({"scope": "status", "hire": true}),
        ] {
            let r = create(extra.clone());
            assert_eq!(r.status, 400, "{extra}: {}", r.body);
            assert!(r.body.contains("read-only"), "{}", r.body);
        }
        let r = create(serde_json::json!({"scope": "status", "label": "k2-watch"}));
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_eq!(v["scope"], "status");
        assert!(v["key"].as_str().expect("key").starts_with("k2sk_"));
        // Explicit owner scope is today's key.
        let r = create(serde_json::json!({"scope": "owner", "label": "plain"}));
        assert_eq!(r.status, 200, "{}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// The setting: Owner/Admin write, tunnel status, headless.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_round_trips_over_http_and_members_cannot_write_it() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let status = || {
            let r = http(d.port, "GET", &format!("/cli/tunnel/status?token={OWNER_TOKEN}"), None, &[]);
            assert_eq!(r.status, 200, "{}", r.body);
            r.json()
        };
        assert_eq!(status()["bootStatus"], "compat");
        assert_eq!(status()["bootStatusExplicit"], false);

        let member = session_for("bsvsetter");
        let r = http(
            d.tunnel_port,
            "POST",
            &format!("/cli/settings/update?token={member}"),
            Some(r#"{"bootStatusPublic":"ghost"}"#),
            &[],
        );
        assert_eq!(r.status, 403, "a Member must not change what strangers see: {}", r.body);
        assert_eq!(status()["bootStatus"], "compat");

        let r = http(
            d.port,
            "POST",
            &format!("/cli/settings/update?token={OWNER_TOKEN}"),
            Some(r#"{"bootStatusPublic":"ghost"}"#),
            &[],
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(status()["bootStatus"], "ghost");
        assert_eq!(status()["bootStatusExplicit"], true);
        assert_eq!(boot(d.tunnel_port, &[]).json()["view"], "public");

        let r = http(
            d.port,
            "POST",
            &format!("/cli/settings/update?token={OWNER_TOKEN}"),
            Some(r#"{"bootStatusPublic":"hidden"}"#),
            &[],
        );
        assert!(r.status >= 400, "an unknown mode is refused: {} {}", r.status, r.body);
        assert_eq!(status()["bootStatus"], "ghost", "refused write must not persist");

        // GET can't write it (0.44.4 no state change over GET).
        let r = http(d.port, "GET", &format!("/cli/settings/update?token={OWNER_TOKEN}&bootStatusPublic=compat"), None, &[]);
        assert_eq!(r.status, 405, "{}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// Settings → General → Update: the handshake across a REAL restart.
//
// The remote update row (GeneralSection `UpdateHostRow`) reads the host's
// `/boot-status` instanceId before apply, then polls it (with the host's
// own session token since 0.46.3) until the daemon is back `ready` on a new
// instanceId and reads the version it now runs. A spawned `k2-daemon` in a
// temp HOME is killed and respawned on the same HOME (= the restart an
// update does); the caller sits behind a proxy header so it is a remote
// (`Lan`) caller, like the tunnel / front door.
// ─────────────────────────────────────────────────────────────────────

struct Spawned {
    child: std::process::Child,
    port: u16,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_trimmed(p: &Path) -> Option<String> {
    let t = std::fs::read_to_string(p).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn spawn_daemon(root: &Path, rt: &k2_core::test_env::DaemonRuntime, shim: &Path) -> (Spawned, String) {
    let _ = std::fs::remove_file(root.join(".k2/daemon.port"));
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", root)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", shim)
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let deadline = Instant::now() + Duration::from_secs(30);
    let (port, owner) = loop {
        let port = read_trimmed(&root.join(".k2/daemon.port")).and_then(|p| p.parse::<u16>().ok());
        if let (Some(port), Some(owner)) = (port, read_trimmed(&root.join(".k2/daemon.token"))) {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break (port, owner);
            }
        }
        assert!(Instant::now() < deadline, "daemon never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    (Spawned { child, port }, owner)
}

/// A remote client's poll: through a proxy (`Lan`), optionally with its token.
fn remote_poll(port: u16, token: Option<&str>) -> Option<serde_json::Value> {
    let mut extra = vec!["X-Forwarded-For: 198.51.100.4".to_string()];
    if let Some(t) = token {
        extra.push(bearer(t));
    }
    let refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let req = format!(
        "GET /boot-status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n{}\r\n\r\n",
        refs.join("\r\n")
    );
    stream.write_all(req.as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).to_string();
    if !text.starts_with("HTTP/1.1 200") {
        return None;
    }
    serde_json::from_str(text.split_once("\r\n\r\n")?.1).ok()
}

/// Poll until the daemon is back `ready` on an instanceId other than
/// `before` (what `watchUpdateComeback` waits for).
fn wait_comeback(port: u16, token: Option<&str>, before: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(v) = remote_poll(port, token) {
            if v["phase"] == "ready" && v["instanceId"].as_str().is_some_and(|id| id != before) {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "daemon never came back ready on a new instanceId");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn update_flow_handshake_survives_a_real_restart_in_compat_and_ghost() {
    let root = k2_core::test_env::unique_temp_path("boot-views-restart");
    std::fs::create_dir_all(root.join(".k2")).expect("temp HOME");
    let shim = root.join("agent-shim-empty");
    std::fs::create_dir_all(&shim).expect("shim dir");
    let rt = k2_core::test_env::DaemonRuntime::new(&root);
    let post = |port: u16, path: &str, body: &str| http(port, "POST", path, Some(body), &[]);

    let (a, owner) = spawn_daemon(&root, &rt, &shim);
    let deadline = Instant::now() + Duration::from_secs(30);
    while http(a.port, "GET", "/boot-status", None, &[]).json()["phase"] != "ready" {
        assert!(Instant::now() < deadline, "first daemon never ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    // The owner's remote login (an Owner-role Connect session, the token the
    // desktop holds for a remote host).
    let r = post(a.port, &format!("/cli/users/add?token={owner}"), r#"{"username":"updater","password":"password123"}"#);
    assert_eq!(r.status, 200, "users/add: {}", r.body);
    let r = post(a.port, &format!("/cli/users/set-role?token={owner}"), r#"{"username":"updater","role":"owner"}"#);
    assert_eq!(r.status, 200, "users/set-role: {}", r.body);
    let r = post(a.port, "/cli/auth/login", r#"{"username":"updater","password":"password123"}"#);
    assert_eq!(r.status, 200, "login: {}", r.body);
    let session = r.json()["token"].as_str().expect("session token").to_string();

    // ── compat (default): an OLD token-less client and a 0.46.3 client ──
    let before_old = remote_poll(a.port, None).expect("old client poll");
    assert_eq!(before_old["view"], "compat");
    let before_new = remote_poll(a.port, Some(&session)).expect("new client poll");
    assert_eq!(before_new["view"], "full");
    let iid_a = before_new["instanceId"].as_str().expect("instanceId").to_string();
    assert_eq!(before_old["instanceId"], iid_a.as_str());
    // The update job routes the row drives answer the same session.
    let r = http(a.port, "GET", &format!("/cli/daemon/update/status?job_id=nope&token={session}"), None, &["X-Forwarded-For: 198.51.100.4"]);
    assert!(r.status == 404 || r.status == 400, "update/status reachable with the session (unknown job): {} {}", r.status, r.body);
    drop(a); // the restart an update does

    let (b, _) = spawn_daemon(&root, &rt, &shim);
    let back_old = wait_comeback(b.port, None, &iid_a);
    assert_eq!(back_old["view"], "compat", "old clients still read their fields after the restart");
    assert_eq!(back_old["version"], env!("CARGO_PKG_VERSION"), "old client sees the running version");
    assert!(back_old["features"].as_array().is_some_and(|f| !f.is_empty()));
    let back_new = wait_comeback(b.port, Some(&session), &iid_a);
    assert_eq!(back_new["view"], "full", "the session survives the restart: {back_new}");
    assert_eq!(back_new["version"], env!("CARGO_PKG_VERSION"), "the row shows the version the host now runs");
    let iid_b = back_new["instanceId"].as_str().expect("instanceId").to_string();
    assert_ne!(iid_b, iid_a, "a new process");

    // ── ghost: a stranger sees the restart, the client still gets the version ──
    let r = post(b.port, &format!("/cli/settings/update?token={owner}"), r#"{"bootStatusPublic":"ghost"}"#);
    assert_eq!(r.status, 200, "{}", r.body);
    let ghost_before = remote_poll(b.port, None).expect("ghost poll");
    assert_eq!(ghost_before.as_object().map(|o| o.len()), Some(5), "public view: {ghost_before}");
    assert_eq!(ghost_before["instanceId"], iid_b.as_str());
    drop(b);

    let (c, _) = spawn_daemon(&root, &rt, &shim);
    let ghost_back = wait_comeback(c.port, None, &iid_b);
    assert_eq!(ghost_back["view"], "public", "the setting survives the restart");
    assert!(ghost_back.get("version").is_none(), "{ghost_back}");
    let authed_back = wait_comeback(c.port, Some(&session), &iid_b);
    assert_eq!(authed_back["view"], "full");
    assert_eq!(authed_back["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(authed_back["instanceId"], ghost_back["instanceId"]);
    // The local window (loopback) is untouched by the setting.
    assert_eq!(http(c.port, "GET", "/boot-status", None, &[]).json()["view"], "full");
    drop(c);
    let _ = std::fs::remove_dir_all(&root);
}
