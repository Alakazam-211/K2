//! F1 (prd-v1-api-completion §3) — NON-SANDBOXED HOST SESSIONS on `/v1`,
//! driven end-to-end through the REAL dispatcher (`test_harness::start`).
//!
//! The contract under test:
//!
//!   1. GATE MATRIX: with both gates off the family is surface-absent
//!      (the outer 404); with `K2_API=1` ALONE it is fully served — even on
//!      a build where `can_sandbox()` is false (that's the whole point) —
//!      while the sandbox families stay absent.
//!   2. SPAWN happy-path: cwd == the workspace's registered path, the
//!      minted command is the workspace's configured agent with the
//!      caller's hostile `command`/`args`/`env`/`cwd` body fields DROPPED,
//!      `--dangerously-skip-permissions` STRIPPED (default-off opt-in),
//!      `--session-id <sessionId>` spliced, response `"sandbox":"none"`
//!      with the FROZEN five-key shape, reaper armed, list shows it live.
//!   3. 404-UNIFORMITY: unknown ws / ungranted ws / unknown session id are
//!      indistinguishable.
//!   4. QUOTA: an api-key principal at its cap gets 429 + machine code.
//!   5. CANONICAL GUARD: the pinned canonical session id is refused on
//!      resume, message-live, and messages.
//!   6. MESSAGE-LIVE: the prompt reaches the real PTY (cat echo observed
//!      on the Term), and a spawn-time prompt arrives post-readiness.
//!   7. POST-only/shape guards: a stray POST on `/messages` is a uniform
//!      404, never a 405/500 oracle.
//!
//! ISOLATION: gate env vars + `$HOME` + `PATH` are process-wide — every
//! test serializes on `TEST_LOCK` (api_gate/viewer-claimer pattern).

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "host-sessions-owner-token-f1";

// ── HTTP helpers (async harness style, viewer-claimer suite) ─────────

async fn http_req(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("unparsable response: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

// ── Env scaffolding (api_gate GateEnv pattern + PATH shim) ───────────

struct HostEnv {
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    tmp_home: std::path::PathBuf,
    shim_dir: std::path::PathBuf,
}

impl HostEnv {
    /// Redirect `$HOME`, set `K2_API` per `api_on`, clear `K2_SANDBOX_API`,
    /// clamp the spawn-prompt readiness ceiling, and mint an on-disk agent
    /// shim (`claude` → `exec cat`). The shim is wired PER WORKSPACE via
    /// the REAL de-generalization seam (`agent_presets` row + the
    /// workspace's `projects.default_agent`, see [`configure_ws_agent`]) —
    /// an ABSOLUTE path, so the daemon's login-PATH enrichment can never
    /// shadow it with a real `claude` install.
    fn set(api_on: bool) -> Self {
        let names: [&'static str; 4] = [
            "K2_API",
            "K2_SANDBOX_API",
            "HOME",
            "K2_HOST_SESSION_READY_TIMEOUT_SECS",
        ];
        let prev: Vec<_> = names.iter().map(|n| (*n, std::env::var_os(n))).collect();

        match api_on {
            true => std::env::set_var("K2_API", "1"),
            false => std::env::remove_var("K2_API"),
        }
        std::env::remove_var("K2_SANDBOX_API");
        // `cat` never advertises bracketed-paste readiness — clamp the
        // post-spawn prompt injector's ceiling so tests stay fast.
        std::env::set_var("K2_HOST_SESSION_READY_TIMEOUT_SECS", "1");

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_home = std::env::temp_dir()
            .join(format!("k2-host-sess-f1-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).expect("create temp HOME");
        std::env::set_var("HOME", &tmp_home);

        // Agent shim named `claude` (basename drives the provider grammar)
        // at an ABSOLUTE path.
        let shim_dir = std::env::temp_dir()
            .join(format!("k2-host-sess-shim-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&shim_dir).expect("create shim dir");
        let shim = shim_dir.join("claude");
        std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("write shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
                .expect("chmod shim");
        }

        Self { prev, tmp_home, shim_dir }
    }

    /// Absolute path of the `claude`-named shim binary.
    fn shim(&self) -> String {
        self.shim_dir.join("claude").to_string_lossy().into_owned()
    }
}

/// Wire `ws_name`'s DEFAULT AGENT to the shim through the REAL seam: an
/// enabled `agent_presets` row whose command is
/// `"<abs-shim> --dangerously-skip-permissions"` (danger flag included so
/// the resolver's strip is proven END-TO-END on the live argv), pointed at
/// by `projects.default_agent`.
fn configure_ws_agent(ws_name: &str, shim_path: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let preset_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
         VALUES (?1, ?2, ?3, '', 1, 999, 0)",
        rusqlite::params![
            preset_id,
            format!("hs-shim-{preset_id}"),
            format!("{shim_path} --dangerously-skip-permissions"),
        ],
    )
    .expect("insert shim preset");
    let rows = conn
        .execute(
            "UPDATE projects SET default_agent = ?1 WHERE name = ?2",
            rusqlite::params![preset_id, ws_name],
        )
        .expect("set default_agent");
    assert_eq!(rows, 1, "workspace {ws_name} must exist to configure its agent");
}

impl Drop for HostEnv {
    fn drop(&mut self) {
        for (name, val) in &self.prev {
            match val {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        let _ = std::fs::remove_dir_all(&self.tmp_home);
        let _ = std::fs::remove_dir_all(&self.shim_dir);
    }
}

/// Seed a `projects` row (real on-disk dir) and return its path.
fn setup_project(name: &str) -> std::path::PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2-host-sess-ws-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();
    let db = k2_core::db::init_for_tests();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) \
         VALUES (?1, ?2, ?3, 'custom')",
        rusqlite::params![
            uuid::Uuid::new_v4().to_string(),
            project_path.to_string_lossy().as_ref(),
            name,
        ],
    )
    .unwrap();
    project_path
}

/// Mint an API key granted `workspaces` via the REAL create route; returns
/// the raw `k2sk_…` bearer.
async fn mint_api_key(port: u16, label: &str, workspaces: &str) -> String {
    let body = serde_json::json!({ "label": label, "workspaces": [workspaces] }).to_string();
    let (status, resp) = http_req(
        port,
        "POST",
        &format!("/cli/api-keys/create?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "api-key create failed: {resp}");
    json(&resp)["key"].as_str().expect("raw key").to_string()
}

/// All text currently on the session's Term (viewer-claimer suite helper).
fn grid_text(session_id: &str) -> String {
    let sid = k2_core::session::SessionId::parse(session_id).expect("valid session uuid");
    let session = v2_session_map::lookup_by_session_id(&sid)
        .unwrap_or_else(|| panic!("session {session_id} not in v2_session_map"));
    let tm = session.term();
    let t = tm.lock();
    let snap = k2_core::terminal::snapshot_term("f1-probe", &t, 0);
    let mut out = String::new();
    for row in snap.scrollback.iter().chain(snap.grid.iter()) {
        for run in row {
            out.push_str(&run.text);
        }
        out.push('\n');
    }
    out
}

async fn assert_text_appears(session_id: &str, needle: &str, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if grid_text(session_id).contains(needle) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: {needle:?} never appeared on the Term; grid was:\n{}",
            grid_text(session_id)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Kill an api-spawned session by agent name (frees its quota slot via the
/// child-exit observer) so later tests aren't polluted.
async fn close_session(port: u16, agent_name: &str) {
    let (status, body) = http_req(
        port,
        "POST",
        &format!("/cli/sessions/v2/close?token={OWNER_TOKEN}"),
        Some(&serde_json::json!({ "agent_name": agent_name, "force": true }).to_string()),
    )
    .await;
    assert_eq!(status, 200, "close {agent_name} failed: {body}");
    // Give the child-exit observer a beat to run its teardown (quota release).
    tokio::time::sleep(Duration::from_millis(400)).await;
}

const SURFACE_OFF: &str = r#"{"error":"not found"}"#;

// ─────────────────────────────────────────────────────────────────────
// 1 — gate matrix
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_off_host_sessions_are_surface_absent() {
    let _g = lock();
    let _env = HostEnv::set(false);
    let d = test_harness::start(OWNER_TOKEN).await;

    for (method, path, body) in [
        ("POST", format!("/v1/w/x/host-sessions?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/w/x/host-sessions?token={OWNER_TOKEN}"), None),
        ("POST", format!("/v1/w/x/host-sessions/sid?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/w/x/host-sessions/sid/messages?token={OWNER_TOKEN}"), None),
    ] {
        let (status, resp) = http_req(d.port, method, &path, body).await;
        assert_eq!(status, 404, "{method} {path} must be surface-absent; body={resp}");
        assert_eq!(resp, SURFACE_OFF, "{method} {path}: outer surface-off 404");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn k2_api_alone_serves_host_sessions_even_where_sandboxes_cannot() {
    let _g = lock();
    let env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;

    // PREMISE: this (mac / feature-off) build cannot sandbox — host
    // sessions must work anyway (the whole point of the family).
    assert!(
        !k2_daemon::v2_spawn::can_sandbox(),
        "test premise: this build cannot sandbox"
    );

    // The sandbox family is surface-absent under K2_API alone…
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/sandboxes?token={OWNER_TOKEN}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 404, "sandbox family absent under K2_API alone; body={resp}");

    // …while a host-session spawn in a real workspace succeeds (200).
    let _ws = setup_project("hs-gate-on");
    configure_ws_agent("hs-gate-on", &env.shim());
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-gate-on/host-sessions?token={OWNER_TOKEN}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "host-session spawn must work sandbox-less; body={resp}");
    let v = json(&resp);
    assert_eq!(v["sandbox"], "none", "honest label; body={resp}");
    // Unauthenticated probe → 401 from the /v1 auth tier (family exists).
    let (status, _) = http_req(d.port, "GET", "/v1/w/hs-gate-on/host-sessions", None).await;
    assert_eq!(status, 401, "auth tier live for the family");

    close_session(d.port, v["agentName"].as_str().expect("agentName")).await;
}

// ─────────────────────────────────────────────────────────────────────
// 2 — spawn happy-path (the security-property test)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_pins_cwd_mints_command_and_drops_caller_inputs() {
    let _g = lock();
    let env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    let ws_path = setup_project("hs-happy");
    configure_ws_agent("hs-happy", &env.shim());

    // HOSTILE body: caller-supplied command/args/env/cwd MUST all be
    // ignored (the request schema has no such fields — serde drops them).
    let body = serde_json::json!({
        "cols": 100,
        "rows": 30,
        "command": "/bin/evil",
        "args": ["-rf", "/"],
        "env": { "LD_PRELOAD": "/tmp/evil.so" },
        "cwd": "/",
    })
    .to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-happy/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);

    // FROZEN wire shape: exactly sessionId/agentName/workspace/sandbox/stream.
    assert_eq!(
        v.as_object().map(|o| o.len()),
        Some(5),
        "spawn response is FROZEN five-key shape; body={resp}"
    );
    assert_eq!(v["sandbox"], "none");
    assert_eq!(v["workspace"], "hs-happy");
    let session_id = v["sessionId"].as_str().expect("sessionId");
    let agent_name = v["agentName"].as_str().expect("agentName");
    assert!(agent_name.starts_with("api-owner-"), "host-minted name: {agent_name}");
    let grid = v["stream"]["grid"].as_str().expect("stream.grid");
    assert!(
        grid.contains(&format!("session={session_id}")) && grid.contains("token=k2st_"),
        "grid URL carries the per-session stream token: {grid}"
    );

    // The LIVE session proves the policy resolver's work: cwd pinned to the
    // REGISTERED workspace path; program is the workspace agent (shimmed
    // claude), args are EXACTLY the spliced session id — the danger flag
    // stripped, the hostile caller args absent.
    let sid = k2_core::session::SessionId::parse(session_id).expect("uuid");
    let session = v2_session_map::lookup_by_session_id(&sid).expect("session registered");
    assert_eq!(
        session.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
        Some(ws_path.to_string_lossy().into_owned()),
        "cwd MUST be the workspace's registered path"
    );
    assert_eq!(
        session.program.as_deref(),
        Some(env.shim().as_str()),
        "the WORKSPACE-CONFIGURED agent (preset via projects.default_agent)"
    );
    assert_eq!(
        session.args,
        vec!["--session-id".to_string(), session_id.to_string()],
        "exactly the host-spliced premint — the preset's \
         --dangerously-skip-permissions STRIPPED, caller args dropped"
    );

    // Reaper armed for this session.
    assert!(
        k2_daemon::sandbox_reaper::registered(&sid),
        "idle reaper must be armed for an api-spawned host session"
    );

    // The list route surfaces it as live.
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hs-happy/host-sessions?token={OWNER_TOKEN}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "list failed: {resp}");
    let l = json(&resp);
    let entries = l["sessions"].as_array().expect("sessions");
    let mine = entries
        .iter()
        .find(|e| e["sessionId"] == serde_json::json!(session_id))
        .unwrap_or_else(|| panic!("spawned session missing from list: {resp}"));
    assert_eq!(mine["live"], true, "list liveness flag; body={resp}");
    assert_eq!(mine["agentName"], serde_json::json!(agent_name));

    close_session(d.port, agent_name).await;
}

// ─────────────────────────────────────────────────────────────────────
// 3 — 404 uniformity + canonical guard + shape guards
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uniform_404_for_unknown_ungranted_and_unknown_session() {
    let _g = lock();
    let _env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-uniform");
    let key = mint_api_key(d.port, "hs-elsewhere", "hs-some-other-ws").await;

    let uniform = r#"{"error":"no such workspace"}"#;
    for (label, path_q) in [
        // Owner on an unknown ws.
        ("unknown ws", format!("/v1/w/hs-no-such-ws/host-sessions?token={OWNER_TOKEN}")),
        // Granted-elsewhere key on an EXISTING ws (no oracle).
        ("ungranted ws", format!("/v1/w/hs-uniform/host-sessions?token={key}")),
    ] {
        let (status, resp) = http_req(d.port, "POST", &path_q, Some("{}")).await;
        assert_eq!(status, 404, "{label}: body={resp}");
        assert_eq!(resp, uniform, "{label}: byte-identical uniform 404");
    }
    // Unknown session id under a valid ws (message-live) → same shape.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!(
            "/v1/w/hs-uniform/host-sessions/00000000-0000-4000-8000-000000000000?token={OWNER_TOKEN}"
        ),
        Some(r#"{"prompt":"x"}"#),
    )
    .await;
    assert_eq!(status, 404, "unknown session: body={resp}");
    assert_eq!(resp, uniform, "unknown session: byte-identical uniform 404");

    // POST on the messages sub-path (wrong method for the shape) → uniform
    // 404 from the /v1/w/ catch-all, never a 405/oracle.
    let (status, _resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-uniform/host-sessions/sid/messages?token={OWNER_TOKEN}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 404, "POST on the GET-only messages shape is a uniform 404");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_session_is_off_limits_everywhere() {
    let _g = lock();
    let _env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    let ws_path = setup_project("hs-canon");
    let _ = ws_path;

    // Register the workspace's canonical session id.
    let canon_sid = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let pid: String = conn
            .query_row(
                "SELECT id FROM projects WHERE name='hs-canon'",
                [],
                |r| r.get(0),
            )
            .expect("project id");
        conn.execute(
            "INSERT INTO workspace_sessions \
                 (id, project_id, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, 'term-1', ?3, 'claude', 'system', 'active', unixepoch())",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), pid, canon_sid],
        )
        .expect("insert canonical");
    }

    let uniform = r#"{"error":"no such workspace"}"#;
    // Resume the canonical id → refused.
    let body = serde_json::json!({ "session": canon_sid }).to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-canon/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 404, "canonical resume refused; body={resp}");
    assert_eq!(resp, uniform);
    // Message-live the canonical id → refused.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-canon/host-sessions/{canon_sid}?token={OWNER_TOKEN}"),
        Some(r#"{"prompt":"x"}"#),
    )
    .await;
    assert_eq!(status, 404, "canonical message-live refused; body={resp}");
    assert_eq!(resp, uniform);
    // Read the canonical transcript → refused.
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hs-canon/host-sessions/{canon_sid}/messages?token={OWNER_TOKEN}"),
        None,
    )
    .await;
    assert_eq!(status, 404, "canonical read refused; body={resp}");
    assert_eq!(resp, uniform);
}

// ─────────────────────────────────────────────────────────────────────
// 4 — quota 429
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_key_principal_hits_quota_429_at_cap() {
    let _g = lock();
    let env = HostEnv::set(true);
    let prev_cap = std::env::var_os("K2_SANDBOX_PRINCIPAL_CELL_CAP");
    std::env::set_var("K2_SANDBOX_PRINCIPAL_CELL_CAP", "1");
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-quota");
    configure_ws_agent("hs-quota", &env.shim());
    let key = mint_api_key(d.port, "hs-quota-key", "hs-quota").await;

    // First spawn takes the principal's only slot.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-quota/host-sessions?token={key}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "first spawn under the cap; body={resp}");
    let agent = json(&resp)["agentName"].as_str().expect("agentName").to_string();

    // Second spawn → 429 with the machine-readable code.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-quota/host-sessions?token={key}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 429, "at-cap spawn must 429; body={resp}");
    let v = json(&resp);
    assert!(v["code"].is_string(), "429 carries a machine code; body={resp}");

    // Teardown frees the slot: a third spawn succeeds again (proves the
    // child-exit observer released the host-session's quota hold). The
    // observer is async — poll with a hard deadline, fail loudly.
    close_session(d.port, &agent).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let released = loop {
        let (status, resp) = http_req(
            d.port,
            "POST",
            &format!("/v1/w/hs-quota/host-sessions?token={key}"),
            Some("{}"),
        )
        .await;
        if status == 200 {
            break resp;
        }
        assert_eq!(status, 429, "only 429 is acceptable while the slot drains; body={resp}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "quota slot never released after teardown; last body={resp}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    close_session(d.port, json(&released)["agentName"].as_str().expect("agent")).await;

    match prev_cap {
        Some(v) => std::env::set_var("K2_SANDBOX_PRINCIPAL_CELL_CAP", v),
        None => std::env::remove_var("K2_SANDBOX_PRINCIPAL_CELL_CAP"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// 5 — message-live inject + spawn-prompt delivery (real PTY echo)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn message_live_and_spawn_prompt_reach_the_pty() {
    let _g = lock();
    let env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-inject");
    configure_ws_agent("hs-inject", &env.shim());

    // Spawn WITH an initial prompt — the background injector delivers it
    // once the (1s-clamped) readiness ceiling passes; cat echoes it.
    let body = serde_json::json!({ "prompt": "hs-spawn-prompt-marker" }).to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-inject/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let session_id = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent = v["agentName"].as_str().expect("agentName").to_string();

    assert_text_appears(&session_id, "hs-spawn-prompt-marker", "spawn prompt").await;

    // Message-live into the SAME session.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-inject/host-sessions/{session_id}?token={OWNER_TOKEN}"),
        Some(r#"{"prompt":"hs-live-message-marker"}"#),
    )
    .await;
    assert_eq!(status, 200, "message-live failed: {resp}");
    let m = json(&resp);
    assert_eq!(m["delivered"], true, "body={resp}");
    assert_eq!(m["live"], true, "body={resp}");
    assert_eq!(m["sessionId"], serde_json::json!(session_id));
    assert_text_appears(&session_id, "hs-live-message-marker", "message-live").await;

    // Cross-principal message-live is refused (uniform 404): a key granted
    // THIS workspace still can't drive the owner's session.
    let key = mint_api_key(d.port, "hs-inject-other", "hs-inject").await;
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-inject/host-sessions/{session_id}?token={key}"),
        Some(r#"{"prompt":"stolen"}"#),
    )
    .await;
    assert_eq!(status, 404, "cross-principal message-live must 404; body={resp}");

    close_session(d.port, &agent).await;
}
