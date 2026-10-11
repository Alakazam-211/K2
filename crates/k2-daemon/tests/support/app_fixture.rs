//! Shared harness for the App runtime's headless tests
//! (`prd-k2-app-runtime-v1` §12, AR27: one fixture, not another copy per
//! test file). An in-process daemon on a temp `$HOME` with `K2_APPS=1`,
//! loopback HTTP helpers, workspaces, passports, and App folders.
//!
//! Never the real `~/.k2`, never the keychain, never a real agent CLI.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
use k2_core::test_env::{EnvVar, TempHome};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use rusqlite::params;
use serde_json::Value;

pub const OWNER: &str = "owner-token-apps-runtime";

pub struct Env {
    pub home: TempHome,
    _vars: Vec<EnvVar>,
}

impl Env {
    pub fn path(&self) -> &Path {
        self.home.path()
    }
}

/// A temp `$HOME` (short path), the preview flag on, and fast deploy
/// timings for tests. Holds the env lock for the test.
pub fn setup(extra: &[(&str, &str)]) -> Env {
    let home = TempHome::short();
    let mut vars = vec![
        EnvVar::set("K2_APPS", "1"),
        EnvVar::set("K2_APPS_PROBE_EVERY_MS", "100"),
        EnvVar::set("K2_APPS_PROBE_TIMEOUT_MS", "1000"),
        EnvVar::set("K2_APPS_DEPLOY_TIMEOUT_SECS", "20"),
        EnvVar::set("K2_APPS_DRAIN_SECS", "2"),
        EnvVar::set("K2_APPS_STOP_GRACE_SECS", "2"),
    ];
    for (k, v) in extra {
        vars.push(EnvVar::set(k, v));
    }
    Env { home, _vars: vars }
}

// ── HTTP over loopback ───────────────────────────────────────────────

pub struct Resp {
    pub status: u16,
    pub body: String,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", self.body))
    }
    pub fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", self.body)).to_string()
    }
}

pub fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None if method == "POST" => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

pub async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap()
}

// ── workspaces and passports ─────────────────────────────────────────

pub struct Ws {
    pub id: String,
    pub path: PathBuf,
    pub handle: String,
}

pub fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.path().join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let path = std::fs::canonicalize(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .unwrap();
    Ws { id, path, handle: handle.to_string() }
}

pub fn allow_manage(ws: &Ws, on: bool) {
    k2_core::workspace::settings::set_agents_can_manage_skin(&ws.path.to_string_lossy(), on).unwrap();
}

/// A harness-session passport for `ws` (classified as an agent).
pub fn passport(ws: &Ws) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

// ── App folders ──────────────────────────────────────────────────────

/// A python3 HTTP server App: `GET /` and `GET /healthz` answer 200 with
/// `tag`; `GET /slow` holds the connection open for `secs` then answers
/// (for drain tests); `GET /crash` exits the process. Needs python3 (the
/// published-service tests need it too); a missing python3 fails loudly.
pub const SERVER_PY: &str = r#"import http.server, os, sys, time, socketserver
TAG = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "tag.txt")).read().strip()
MODE = os.environ.get("K2_FIXTURE_MODE", "")
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        sys.stderr.write("req %s\n" % self.path); sys.stderr.flush()
    def do_GET(self):
        if self.path == "/crash":
            os._exit(3)
        if self.path.startswith("/slow"):
            time.sleep(float(self.path.split("=")[-1]) if "=" in self.path else 3)
        code = 200
        if self.path == "/env":
            body = ("\n".join(sorted(os.environ.keys())) + "\n").encode()
        else:
            if TAG.startswith("sick") and self.path == "/healthz":
                code = 500
            body = (TAG + "\n").encode()
        self.send_response(code); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
print("fixture %s listening on %s" % (TAG, os.environ["PORT"]), flush=True)
S(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
"#;

pub fn manifest(id: &str, version: &str) -> String {
    format!(
        r#"{{"schema":1,"kind":"app","id":"{id}","name":"{id}","version":"{version}",
"caps":["thread:read"],
"host":{{"mode":"cmd","start":["python3","server.py"],"health":{{"path":"/healthz"}},"drainSecs":2}}}}"#
    )
}

/// Write an App folder with `tag.txt = tag`.
pub fn app_folder(dir: &Path, id: &str, version: &str, tag: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest(id, version)).unwrap();
    std::fs::write(dir.join("server.py"), SERVER_PY).unwrap();
    std::fs::write(dir.join("tag.txt"), tag).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

pub fn require_python3() {
    let ok = std::process::Command::new("python3").arg("-c").arg("print(1)").output().map(|o| o.status.success()).unwrap_or(false);
    assert!(ok, "python3 is required for the App runtime tests");
}

/// Plain HTTP GET to a loopback port; `None` when nothing answers.
pub fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut s = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).ok()?;
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Some((status, body))
}

pub async fn wait_until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}
