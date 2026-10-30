//! `k2 publish` visibility, headless (prd-publish-visibility-and-dev-mode-v1
//! PV-14, vs-live PA-13, PA-6): the real daemon binary in a scratch HOME, the
//! real E2E TLS listener, `curl` as the visitor, a stand-in control plane and
//! an upstream that counts its accepts.
//!
//! 1. **The upgrade.** A real database rolled back to 0.46.6 (no 0253, older
//!    `_migrations` rows) with a tunnelled service, and a routing map on disk
//!    naming a label only the control plane knows. A daemon boots on it. Every
//!    site that was public stays public, from the first connection and before
//!    the first fetch; a label the fetched map adds at that point counts as old.
//! 2. **A label made after the upgrade is private**, with zero upstream bytes.
//! 3. **Each mode through the gate:** private with a one-time link, PIN, public.
//! 4. **A restart** keeps sessions and PIN locks (PA-6).
//!
//! The mode changes are written straight to k2.db, through the same
//! `site_visibility` calls the routes make: the routes are another lane's. So
//! this file checks what a new connection meets; cutting live sockets on a
//! change lives in `site_gate::tests`, in one process.
//!
//! Spawn guards: scratch HOME, private XDG_RUNTIME_DIR, `SHELL=/bin/false`, an
//! empty agent shim dir, control plane on a local stand-in, no real `~/.k2`.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_core::site_visibility as sv;
use rusqlite::{params, Connection};

const SUB: &str = "vis";
const TOKEN: &str = "tok-vis";

fn host(label: &str) -> String {
    format!("{label}.{SUB}.k2.dev")
}

/// The site behind every label: 200 "SITE". Counts accepts and keeps the heads.
struct Site {
    port: u16,
    accepts: Arc<AtomicUsize>,
    heads: Arc<Mutex<Vec<String>>>,
}

fn spawn_site() -> Site {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let accepts = Arc::new(AtomicUsize::new(0));
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (a, h) = (accepts.clone(), heads.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            a.fetch_add(1, Ordering::SeqCst);
            let h = h.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                h.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nSITE");
            });
        }
    });
    Site { port, accepts, heads }
}

/// The stand-in control plane: `GET /subdomains` from a list the test changes.
struct ControlPlane {
    port: u16,
    labels: Arc<Mutex<Vec<String>>>,
    fetches: Arc<AtomicUsize>,
    /// While `true`, `/subdomains` answers 503 (the boot's copy on disk is all
    /// the daemon has).
    down: Arc<AtomicBool>,
}

fn spawn_control_plane(site_port: u16, labels: &[&str]) -> ControlPlane {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let list = Arc::new(Mutex::new(labels.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
    let fetches = Arc::new(AtomicUsize::new(0));
    let down = Arc::new(AtomicBool::new(true));
    let (ll, f, d) = (list.clone(), fetches.clone(), down.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let (ll, f, d) = (ll.clone(), f.clone(), d.clone());
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let (status, body) = if !head.starts_with("GET /subdomains") {
                    ("404 Not Found", "{}".to_string())
                } else if d.load(Ordering::SeqCst) {
                    ("503 Service Unavailable", "{}".to_string())
                } else {
                    f.fetch_add(1, Ordering::SeqCst);
                    let mut rows = vec![serde_json::json!({ "label": SUB, "primary": true, "connected": true, "tier": "pro" })];
                    for label in ll.lock().unwrap().iter() {
                        rows.push(serde_json::json!({ "label": label, "target": format!("127.0.0.1:{site_port}") }));
                    }
                    ("200 OK", serde_json::json!({ "subdomains": rows }).to_string())
                };
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    ControlPlane { port, labels: list, fetches, down }
}

struct Daemon {
    child: Child,
    home: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Daemon {
    fn k2(&self) -> PathBuf {
        self.home.join(".k2")
    }

    fn port(&self) -> u16 {
        std::fs::read_to_string(self.k2().join("daemon.port")).unwrap().trim().parse().unwrap()
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.k2().join("daemon.token")).unwrap().trim().to_string()
    }

    fn e2e_port(&self) -> u16 {
        std::fs::read_to_string(self.k2().join("tunnel-https.port")).unwrap().trim().parse().unwrap()
    }

    fn log_tail(&self) -> String {
        let t = std::fs::read_to_string(self.home.join("daemon.log")).unwrap_or_default();
        let lines: Vec<&str> = t.lines().rev().take(60).collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    }

    /// `curl` as the visitor, on the daemon's real E2E listener. The whole
    /// answer, head first.
    fn visit(&self, label: &str, extra: &[&str], path: &str) -> String {
        let e2e = self.e2e_port();
        let h = host(label);
        let resolve = format!("{h}:{e2e}:127.0.0.1");
        let url = format!("https://{h}:{e2e}{path}");
        let out = Command::new("curl")
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .args(["-sS", "-k", "-i", "--max-time", "30", "--resolve", &resolve])
            .args(extra)
            .arg(&url)
            .output()
            .expect("run curl");
        assert!(out.status.success(), "curl {url}: {}\n{}", String::from_utf8_lossy(&out.stderr), self.log_tail());
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
    }

    fn refresh(&self) {
        let body = post(self.port(), &format!("/cli/tunnel/subdomains/refresh?token={}", self.token()));
        assert!(body.0 == 200, "refresh: {}", body.1);
    }
}

fn post(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(s, "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, text)
}

fn get_status(port: u16, path: &str) -> u16 {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { return 0 };
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    if write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
        return 0;
    }
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    String::from_utf8_lossy(&raw).split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0)
}

fn wait_until(secs: u64, what: &str, log: impl Fn() -> String, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}\n--- daemon log ---\n{}", log());
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn boot(home: &Path, cp_port: u16) -> Daemon {
    let k2 = home.join(".k2");
    for f in ["daemon.port", "daemon.token", "tunnel-https.port"] {
        let _ = std::fs::remove_file(k2.join(f));
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).open(home.join("daemon.log")).unwrap();
    let path_env = k2_core::test_env::path_list_prepend(&home.join("bin"), std::env::var("PATH").unwrap());
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", &path_env)
        .env("SHELL", "/bin/false")
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("TMPDIR", home.join("tmp"))
        .env("K2_E2E_SELF_SIGNED", "1")
        .env("K2_TUNNEL_LEASE", "off")
        .env("K2_CONNECT_BASE", format!("http://127.0.0.1:{cp_port}"))
        .env("K2_CERT_BROKER_URL", "http://127.0.0.1:1")
        .env("K2_TEST_AGENT_SHIM_DIR", home.join("agent-shim-empty"))
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("spawn daemon");
    let d = Daemon { child, home: home.to_path_buf() };
    wait_until(60, "daemon.port + daemon.token", || d.log_tail(), || k2.join("daemon.port").exists() && k2.join("daemon.token").exists());
    let (port, token) = (d.port(), d.token());
    wait_until(60, "boot ready", || d.log_tail(), || get_status(port, &format!("/cli/daemon/handoff/status?token={token}")) == 200);
    d
}

fn stop(mut d: Daemon) {
    let _ = d.child.kill();
    let _ = d.child.wait();
}

fn scratch() -> PathBuf {
    let home = k2_core::test_env::short_tmp(format!("k2vis-{}", std::process::id()));
    for d in [".k2", "bin", "run", "tmp", "agent-shim-empty"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    // The frpc stand-in: the tunnel "connects" and the refresh loop starts.
    let frpc = home.join("bin").join("frpc");
    std::fs::write(&frpc, format!("#!/bin/sh\necho '[I] [proxy_wrapper.go:204] [x] [k2so-{SUB}] start proxy success'\nwhile :; do sleep 1; done\n")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&frpc, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    home
}

fn open_db(home: &Path) -> Connection {
    let c = Connection::open(home.join(".k2").join("k2.db")).expect("open k2.db");
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    c
}

/// Roll a database the daemon made back to what 0.46.6 left: no 0253 tables,
/// no record of it, and earlier migrations that happened a while ago. Then add
/// what a box from before the upgrade has.
fn make_it_a_046_6_database(home: &Path) {
    let c = open_db(home);
    for t in ["site_visibility", "site_sessions", "site_open_codes", "site_pin_limits", "site_visibility_meta"] {
        c.execute_batch(&format!("DROP TABLE {t};")).unwrap();
    }
    assert_eq!(c.execute("DELETE FROM _migrations WHERE name = '0253_site_visibility'", []).unwrap(), 1);
    c.execute("UPDATE _migrations SET applied_at = applied_at - 86400", []).unwrap();
    c.execute("INSERT OR IGNORE INTO projects (id, name, path) VALUES ('p-old', 'old', '/tmp/p-old')", []).unwrap();
    c.execute(
        "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('svc-old', 'p-old', 'old-svc', 'true', '/tmp/p-old', 1, 'tunnel', 'stopped')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO published_services (id, project_id, name, cmd, cwd, port, expose, desired) VALUES ('svc-loc', 'p-old', 'old-local', 'true', '/tmp/p-old', 2, 'local', 'stopped')",
        [],
    )
    .unwrap();
}

fn mode_of(home: &Path, label: &str) -> sv::Mode {
    sv::effective_mode(&open_db(home), label).unwrap()
}

fn cookie(answer: &str, name: &str) -> String {
    let line = answer
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with(&format!("set-cookie: {}=", name.to_ascii_lowercase())))
        .unwrap_or_else(|| panic!("no {name} cookie in:\n{answer}"));
    line.split_once(": ").unwrap().1.split(';').next().unwrap().to_string()
}

#[test]
fn the_upgrade_keeps_public_sites_and_each_mode_holds_through_the_real_gate() {
    let site = spawn_site();
    let home = scratch();
    let cp = spawn_control_plane(site.port, &["old-svc", "old-cp", "old-fetched"]);

    // A fresh boot makes the database (and a fresh database owes no pass).
    let d = boot(&home, cp.port);
    {
        let c = open_db(&home);
        assert!(!sv::grandfather_pending(&c).unwrap(), "a new database owes nothing");
    }
    stop(d);
    make_it_a_046_6_database(&home);

    // What the box had: its tunnel, and a routing map on disk with a label
    // only the control plane knows.
    let k2 = home.join(".k2");
    std::fs::write(
        k2.join("tunnel.json"),
        serde_json::json!({ "server_addr": "127.0.0.1", "server_port": 1, "token": TOKEN, "subdomain": SUB, "auto_start": true, "e2e": true }).to_string(),
    )
    .unwrap();
    let t = format!("127.0.0.1:{}", site.port);
    std::fs::write(k2.join("tunnel-routes.json"), serde_json::json!({ "primary": SUB, "targets": { "old-svc": t, "old-cp": t } }).to_string()).unwrap();

    // ── 1. Boot on it, the control plane down: only the copy on disk. ────
    let d = boot(&home, cp.port);
    wait_until(60, "the E2E listener", || d.log_tail(), || k2.join("tunnel-https.port").exists());
    assert_eq!(cp.fetches.load(Ordering::SeqCst), 0, "no fetch yet: this is the disk copy's turn");
    {
        let c = open_db(&home);
        assert!(sv::grandfather_pending(&c).unwrap(), "the pass is still owed until a fetch");
        let svc = sv::get(&c, "old-svc").unwrap().expect("the migration kept the service");
        assert_eq!((svc.mode, svc.public_reason.as_deref(), svc.set_by.as_str()), (sv::Mode::Public, Some(sv::REASON_BEFORE), sv::SET_BY_UPGRADE));
        assert!(sv::get(&c, "old-local").unwrap().is_none(), "a --no-tunnel service had no public address");
        assert_eq!(sv::get(&c, "old-cp").unwrap().expect("the map on disk kept it").mode, sv::Mode::Public);
    }
    for label in ["old-svc", "old-cp"] {
        let r = d.visit(label, &[], "/");
        assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "{label} stays public from the first connection:\n{r}\n{}", d.log_tail());
    }

    // ── The first fetch: one more old label, and the pass is settled. ────
    cp.down.store(false, Ordering::SeqCst);
    d.refresh();
    let r = d.visit("old-fetched", &[], "/");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "{r}");
    assert_eq!(mode_of(&home, "old-fetched"), sv::Mode::Public);
    assert!(!sv::grandfather_pending(&open_db(&home)).unwrap());
    let kept = sv::grandfathered_public(&open_db(&home)).unwrap();
    let names: Vec<&str> = kept.iter().map(|v| v.label.as_str()).collect();
    assert_eq!(names, vec!["old-cp", "old-fetched", "old-svc"], "the list behind the owner's notice");

    // ── 2. A label made after the upgrade is private. ────────────────────
    cp.labels.lock().unwrap().push("new-site".to_string());
    d.refresh();
    let before = site.accepts.load(Ordering::SeqCst);
    let r = d.visit("new-site", &[], "/secret");
    assert!(r.starts_with("HTTP/1.1 401") && r.contains("This site is private."), "{r}");
    assert!(!r.contains("SITE"), "{r}");
    let ws = d.visit("new-site", &["-H", "Upgrade: websocket", "-H", "Connection: Upgrade", "-H", "Sec-WebSocket-Key: x", "-H", "Sec-WebSocket-Version: 13"], "/hmr");
    assert!(ws.starts_with("HTTP/1.1 401"), "a WebSocket without a session:\n{ws}");
    assert_eq!(site.accepts.load(Ordering::SeqCst), before, "zero upstream connections for a visitor who fails");

    // ── 3a. Private: a one-time link, once. ──────────────────────────────
    let code = sv::mint_open_code(&open_db(&home), "new-site", "/docs", sv::USER_OWNER_TOKEN).unwrap();
    let link = format!("/_k2/site/open?code={}", code.code);
    let r = d.visit("new-site", &[], &link);
    assert!(r.starts_with("HTTP/1.1 303") && r.contains("location: /docs\n") | r.contains("Location: /docs\n"), "{r}");
    assert_eq!(site.accepts.load(Ordering::SeqCst), before, "the landing never touches the site");
    let site_cookie = cookie(&r, sv::COOKIE_SITE);
    let r = d.visit("new-site", &[], &link);
    assert!(r.starts_with("HTTP/1.1 401") && r.contains("expired"), "a reused code fails:\n{r}");
    let cookie_header = format!("Cookie: theme=dark; {site_cookie}");
    let r = d.visit("new-site", &["-H", &cookie_header], "/docs");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "{r}");
    let head = site.heads.lock().unwrap().last().cloned().unwrap();
    assert!(head.contains("Cookie: theme=dark\r\n") && !head.contains("k2_site"), "K2's cookie never reaches the site:\n{head}");

    // ── 3b. PIN: K2's page first; a wrong PIN, a wrong Origin, a right PIN. ──
    let pin = sv::hash_pin("hunter22").unwrap();
    sv::set_pin(&open_db(&home), "new-site", &pin, None, "test").unwrap();
    let before = site.accepts.load(Ordering::SeqCst);
    let r = d.visit("new-site", &[], "/app");
    assert!(r.starts_with("HTTP/1.1 401") && r.contains("needs a PIN") && r.contains("action=\"/_k2/site/pin\""), "{r}");
    let origin = format!("Origin: https://{}", host("new-site"));
    let wrong = d.visit("new-site", &["-H", &origin, "--data", "pin=not-the-pin&next=%2Fapp"], "/_k2/site/pin");
    assert!(wrong.starts_with("HTTP/1.1 401") && wrong.contains("That PIN isn&#39;t right."), "{wrong}");
    let foreign = d.visit("new-site", &["-H", "Origin: https://evil.example", "--data", "pin=hunter22"], "/_k2/site/pin");
    assert!(foreign.starts_with("HTTP/1.1 403"), "{foreign}");
    let right = d.visit("new-site", &["-H", &origin, "--data", "pin=hunter22&next=%2Fapp"], "/_k2/site/pin");
    assert!(right.starts_with("HTTP/1.1 303"), "{right}");
    let pin_cookie = cookie(&right, sv::COOKIE_PIN);
    assert_eq!(site.accepts.load(Ordering::SeqCst), before, "no PIN yet, nothing upstream");
    let pin_header = format!("Cookie: {pin_cookie}");
    let r = d.visit("new-site", &["-H", &pin_header], "/app");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "{r}");
    // The audit names the label and never the PIN.
    let audit = std::fs::read_to_string(k2.join("auth-audit.jsonl")).unwrap();
    assert!(audit.contains("site-pin") && audit.contains("new-site"), "{audit}");
    assert!(!audit.contains("hunter22") && !audit.contains("not-the-pin"), "{audit}");

    // ── 4. A restart keeps sessions and PIN locks (PA-6). ────────────────
    let c = open_db(&home);
    c.execute(
        "INSERT INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES ('site:old-cp', 30, unixepoch(), unixepoch() + 3600)",
        params![],
    )
    .unwrap();
    let pin2 = sv::hash_pin("hunter33").unwrap();
    sv::set_pin(&c, "old-fetched", &pin2, None, "test").unwrap();
    c.execute("INSERT OR REPLACE INTO site_pin_limits (scope, fails, window_start, locked_until) VALUES ('site:old-fetched', 30, unixepoch(), unixepoch() + 3600)", []).unwrap();
    drop(c);
    stop(d);
    let d = boot(&home, cp.port);
    wait_until(60, "the E2E listener", || d.log_tail(), || k2.join("tunnel-https.port").exists());
    let r = d.visit("new-site", &["-H", &pin_header], "/app");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "a PIN session survives a restart:\n{r}\n{}", d.log_tail());
    let locked_origin = format!("Origin: https://{}", host("old-fetched"));
    let r = d.visit("old-fetched", &["-H", &locked_origin, "--data", "pin=hunter33"], "/_k2/site/pin");
    assert!(r.starts_with("HTTP/1.1 429"), "a site lock survives a restart:\n{r}");
    let r = d.visit("old-svc", &[], "/");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "public stays public after a restart:\n{r}");

    // ── 3c. Public: on purpose, and K2's paths are the site's again. ─────
    sv::set_public(&open_db(&home), "new-site", "launch", "test").unwrap();
    let r = d.visit("new-site", &[], "/_k2/site/open");
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("SITE"), "{r}");
    let head = site.heads.lock().unwrap().last().cloned().unwrap();
    assert!(head.starts_with("GET /_k2/site/open"), "{head}");
    let r = d.visit("new-site", &["-H", &pin_header], "/");
    assert!(r.ends_with("SITE"), "{r}");
    assert!(site.heads.lock().unwrap().last().unwrap().contains("k2_pin"), "public is today's raw splice: nothing is parsed or stripped");

    stop(d);
    let _ = std::fs::remove_dir_all(&home);
}
