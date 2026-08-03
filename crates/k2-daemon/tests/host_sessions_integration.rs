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
        let names: [&'static str; 5] = [
            "K2_API",
            "K2_SANDBOX_API",
            "HOME",
            "K2_HOST_SESSION_READY_TIMEOUT_SECS",
            "K2_HOST_SESSION_ADOPTION_DELAY_MS",
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
        // Slice W3 — clamp the self-minting adoption probe's 5s window so
        // the shim scenario doesn't sleep real seconds.
        std::env::set_var("K2_HOST_SESSION_ADOPTION_DELAY_MS", "400");

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

    /// Rewrite the shim as a RECORDING agent: `tee -a <capture>` echoes
    /// stdin (grid parity with the plain `cat` shim) AND appends the raw
    /// received bytes to `capture` — so tests can assert on the exact
    /// injected payload without Term line-wrapping mangling long lines.
    /// Returns the capture file path.
    fn make_shim_recording(&self) -> std::path::PathBuf {
        let capture = self.shim_dir.join("received.bytes");
        let shim = self.shim_dir.join("claude");
        std::fs::write(
            &shim,
            format!("#!/bin/sh\nexec tee -a '{}'\n", capture.display()),
        )
        .expect("write recording shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
                .expect("chmod recording shim");
        }
        capture
    }

    /// Mint an additional `exec cat` shim under `name` (the basename drives
    /// the ProviderResume adapter — `codex` gets subcommand grammar + the
    /// self-minting adoption path). Returns its absolute path.
    fn shim_named(&self, name: &str) -> String {
        let shim = self.shim_dir.join(name);
        std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("write named shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
                .expect("chmod named shim");
        }
        shim.to_string_lossy().into_owned()
    }
}

/// Fabricate the on-disk session record a REAL codex writes at boot
/// (`~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl`, line 1 =
/// `session_meta` with the provider-minted id + cwd) so the deferred
/// adoption probe (`ProviderResume::newest_on_disk` → `detect_codex_session`)
/// has something true to discover. The `exec cat` shim can't write this
/// itself — the TEST plays the provider's disk role, pre-seeding before
/// spawn so the single-shot probe can't race the write.
fn write_codex_session_fixture(home: &std::path::Path, provider_sid: &str, cwd: &str) {
    let day_dir = home
        .join(".codex")
        .join("sessions")
        .join("2026")
        .join("07")
        .join("06");
    std::fs::create_dir_all(&day_dir).expect("create codex sessions dir");
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": {
            "id": provider_sid,
            "timestamp": "2026-07-06T00:00:00Z",
            "cwd": cwd,
        }
    });
    std::fs::write(
        day_dir.join(format!("rollout-2026-07-06T00-00-00-{provider_sid}.jsonl")),
        format!("{meta}\n"),
    )
    .expect("write codex rollout fixture");
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
    let _env = HostEnv::set(false); // RAII guard — held to end of test
    let d = test_harness::start(OWNER_TOKEN).await;

    for (method, path, body) in [
        ("POST", format!("/v1/w/x/host-sessions?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/w/x/host-sessions?token={OWNER_TOKEN}"), None),
        ("POST", format!("/v1/w/x/host-sessions/sid?token={OWNER_TOKEN}"), Some("{}")),
        ("GET", format!("/v1/w/x/host-sessions/sid/messages?token={OWNER_TOKEN}"), None),
        ("POST", format!("/v1/w/x/host-sessions/sid/kill?token={OWNER_TOKEN}"), Some("{}")),
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

/// W2 preset-env merge, END-TO-END: a preset row carrying migration-0070
/// `env` metadata spawns a REAL host session whose child process observes
/// the variable in its environment. The shim prints the var then execs
/// `cat`; the value's arrival on the live Term proves the whole chain
/// (preset row → agent_resolve → policy resolver env base → v2 spawn →
/// DaemonPtyConfig.env → child env).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preset_env_reaches_spawned_child_environment() {
    let _g = lock();
    let _env = HostEnv::set(true); // RAII guard — held to end of test (shim unused here)
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-preset-env");

    // Env-dumping shim (basename `claude` so provider grammar applies),
    // in its own dir so the HostEnv shim stays untouched.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dump_dir = std::env::temp_dir()
        .join(format!("k2-host-sess-envshim-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dump_dir).expect("create env shim dir");
    let dump_shim = dump_dir.join("claude");
    std::fs::write(
        &dump_shim,
        "#!/bin/sh\necho \"PRESET_ENV_SEEN=${ZZ_PRESET_ONLY_VAR:-unset}\"\nexec cat\n",
    )
    .expect("write env shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dump_shim, std::fs::Permissions::from_mode(0o755))
            .expect("chmod env shim");
    }
    let dump_shim_path = dump_shim.to_string_lossy().into_owned();

    // Preset row WITH migration-0070 env metadata, wired as the
    // workspace's default agent (the REAL seam, same as configure_ws_agent).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let preset_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agent_presets \
                 (id, label, command, icon, enabled, sort_order, is_built_in, env) \
             VALUES (?1, ?2, ?3, '', 1, 999, 0, '{\"ZZ_PRESET_ONLY_VAR\":\"from-preset-row\"}')",
            rusqlite::params![
                preset_id,
                format!("hs-envshim-{preset_id}"),
                dump_shim_path.clone(),
            ],
        )
        .expect("insert env preset");
        let rows = conn
            .execute(
                "UPDATE projects SET default_agent = ?1 WHERE name = 'hs-preset-env'",
                rusqlite::params![preset_id],
            )
            .expect("set default_agent");
        assert_eq!(rows, 1, "workspace hs-preset-env must exist");
    }

    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-preset-env/host-sessions?token={OWNER_TOKEN}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let session_id = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent_name = v["agentName"].as_str().expect("agentName").to_string();

    assert_text_appears(
        &session_id,
        "PRESET_ENV_SEEN=from-preset-row",
        "preset env var must reach the spawned child's environment",
    )
    .await;

    close_session(d.port, &agent_name).await;
    let _ = std::fs::remove_dir_all(&dump_dir);
}

/// W5 provider-aware principal staging, END-TO-END: an API key minted with
/// `provider: "openai"` + `baseUrl` (through the REAL create route) spawns a
/// host session whose child process observes OPENAI_API_KEY + OPENAI_BASE_URL
/// — and NO ANTHROPIC_API_KEY — in its environment. Proves the whole chain
/// (create route → api_keys row 0071 → resolve_api_key principal →
/// staged_env_pairs → policy resolver → v2 spawn → child env).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_mapped_principal_key_reaches_child_environment() {
    let _g = lock();
    let _env = HostEnv::set(true); // RAII guard — held to end of test
    // The spawned PTY inherits this (daemon) process's env — scrub any
    // ambient credential vars so the ANTH_KEY_SEEN=unset assertion is about
    // K2's staging, not the developer's shell. Restored at the end.
    let scrubbed: Vec<(&str, Option<std::ffi::OsString>)> =
        ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENAI_BASE_URL"]
            .iter()
            .map(|n| {
                let prev = std::env::var_os(n);
                std::env::remove_var(n);
                (*n, prev)
            })
            .collect();
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-provider-env");

    // Env-dumping shim (basename `claude` so provider grammar applies): echo
    // presence + values of the three interesting vars, then exec cat. The
    // credential here is a TEST fixture, not a real secret.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dump_dir = std::env::temp_dir()
        .join(format!("k2-host-sess-provshim-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dump_dir).expect("create provider shim dir");
    let dump_shim = dump_dir.join("claude");
    std::fs::write(
        &dump_shim,
        "#!/bin/sh\n\
         echo \"OAI_KEY_SEEN=${OPENAI_API_KEY:-unset}\"\n\
         echo \"OAI_URL_SEEN=${OPENAI_BASE_URL:-unset}\"\n\
         echo \"ANTH_KEY_SEEN=${ANTHROPIC_API_KEY:-unset}\"\n\
         exec cat\n",
    )
    .expect("write provider shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dump_shim, std::fs::Permissions::from_mode(0o755))
            .expect("chmod provider shim");
    }
    configure_ws_agent("hs-provider-env", &dump_shim.to_string_lossy());

    // Mint the openai-provider key through the REAL route (W5 additive body).
    let body = serde_json::json!({
        "label": "hs-provider-env-key",
        "llmKey": "sk-oai-e2e-fixture",
        "provider": "openai",
        "baseUrl": "https://oai-proxy.example/v1",
        "workspaces": ["hs-provider-env"],
    })
    .to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/cli/api-keys/create?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "provider'd api-key create failed: {resp}");
    let key = json(&resp)["key"].as_str().expect("raw key").to_string();

    // Spawn AS the API-key principal.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-provider-env/host-sessions?token={key}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let session_id = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent_name = v["agentName"].as_str().expect("agentName").to_string();

    assert_text_appears(
        &session_id,
        "OAI_KEY_SEEN=sk-oai-e2e-fixture",
        "OPENAI_API_KEY must reach the spawned child's environment",
    )
    .await;
    assert_text_appears(
        &session_id,
        "OAI_URL_SEEN=https://oai-proxy.example/v1",
        "OPENAI_BASE_URL pass-through must reach the child",
    )
    .await;
    assert_text_appears(
        &session_id,
        "ANTH_KEY_SEEN=unset",
        "an openai-provider key must NOT stage ANTHROPIC_API_KEY",
    )
    .await;

    close_session(d.port, &agent_name).await;
    let _ = std::fs::remove_dir_all(&dump_dir);
    for (name, prev) in scrubbed {
        match prev {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// 3 — 404 uniformity + canonical guard + shape guards
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uniform_404_for_unknown_ungranted_and_unknown_session() {
    let _g = lock();
    let _env = HostEnv::set(true); // RAII guard — held to end of test
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
    let _env = HostEnv::set(true); // RAII guard — held to end of test
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
    // F4: workspace is the URL slug on live paths too (not the abs path).
    assert_eq!(m["workspace"], "hs-inject", "live must return slug; body={resp}");
    // F5: session/status fields are top-level — never nested under capabilities.
    assert!(m.get("sessionId").is_some(), "F5 top-level sessionId; body={resp}");
    assert!(m.get("workspace").is_some(), "F5 top-level workspace; body={resp}");
    if let Some(caps) = m.get("capabilities") {
        assert!(caps.get("sessionId").is_none(), "F5 sessionId not under capabilities; body={resp}");
        assert!(caps.get("workspace").is_none(), "F5 workspace not under capabilities; body={resp}");
        assert!(caps.get("resumed").is_none(), "F5 resumed not under capabilities; body={resp}");
    }
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

// ─────────────────────────────────────────────────────────────────────
// 6 — the `k2 respond` contract preamble (W1, 0.40.30)
// ─────────────────────────────────────────────────────────────────────

/// Poll `capture` until its contents include `needle` (hard 8s deadline,
/// fails loudly with whatever bytes DID arrive).
async fn wait_capture_contains(capture: &std::path::Path, needle: &str, what: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let got = std::fs::read_to_string(capture).unwrap_or_default();
        if got.contains(needle) {
            return got;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: {needle:?} never reached the shim agent; received bytes were:\n{got}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The spawn-time INITIAL prompt must arrive as the Phase 0b stack
/// (FROZEN [`API_SPAWN_PREAMBLE`] + owner guest policy + caller prompt).
/// Follow-up message-live re-asserts guest policy but does NOT re-send
/// the spawn preamble. A body field cannot override the owner policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_prompt_carries_preamble_and_guest_policy_followups_reassert_guest() {
    let _g = lock();
    let env = HostEnv::set(true);
    let capture = env.make_shim_recording();
    let d = test_harness::start(OWNER_TOKEN).await;
    let ws_path = setup_project("hs-preamble");
    configure_ws_agent("hs-preamble", &env.shim());

    // Owner custom policy — must win over any body field.
    let owner_policy = "hs-preamble-OWNER-GUEST-POLICY-marker";
    k2_core::workspace::settings::update_project_setting(
        ws_path.to_string_lossy().as_ref(),
        "api_guest_policy",
        owner_policy,
    )
    .expect("set owner guest policy");

    let caller_prompt = "hs-preamble-caller-prompt-marker";
    // Attacker tries to override guest policy in the body — must be ignored.
    let body = serde_json::json!({
        "prompt": caller_prompt,
        "api_guest_policy": "hs-preamble-ATTACKER-POLICY",
        "apiGuestPolicy": "hs-preamble-ATTACKER-POLICY",
    })
    .to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-preamble/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let session_id = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent = v["agentName"].as_str().expect("agentName").to_string();

    let preamble = k2_daemon::v1_host_sessions::API_SPAWN_PREAMBLE;
    let received = wait_capture_contains(&capture, caller_prompt, "spawn prompt").await;
    let spawn_stack =
        k2_daemon::v1_host_sessions::compose_spawn_inject(owner_policy, caller_prompt);
    assert!(
        received.contains(&spawn_stack),
        "spawn must inject preamble + owner guest policy + caller; received:\n{received}"
    );
    assert!(
        received.contains(preamble),
        "initial injection must carry the FROZEN contract preamble; received:\n{received}"
    );
    assert!(
        received.contains(owner_policy),
        "initial injection must carry owner guest policy; received:\n{received}"
    );
    assert!(
        !received.contains("hs-preamble-ATTACKER-POLICY"),
        "body guest-policy fields must be ignored; received:\n{received}"
    );

    // Follow-up: guest policy re-asserted, preamble NOT repeated.
    let followup = "hs-preamble-followup-marker";
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-preamble/host-sessions/{session_id}?token={OWNER_TOKEN}"),
        Some(
            &serde_json::json!({
                "prompt": followup,
                "api_guest_policy": "hs-preamble-ATTACKER-FOLLOWUP",
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, 200, "message-live failed: {resp}");
    assert_eq!(json(&resp)["delivered"], true, "body={resp}");

    let received = wait_capture_contains(&capture, followup, "follow-up message").await;
    let follow_stack =
        k2_daemon::v1_host_sessions::compose_followup_inject(owner_policy, followup);
    assert!(
        received.contains(&follow_stack),
        "follow-up must re-assert guest policy then caller; received:\n{received}"
    );
    assert_eq!(
        received.matches(preamble).count(),
        1,
        "the contract preamble is delivered EXACTLY ONCE (at spawn); received:\n{received}"
    );
    assert_eq!(
        received.matches(owner_policy).count(),
        2,
        "guest policy must appear on spawn AND follow-up; received:\n{received}"
    );
    assert!(
        !received.contains("hs-preamble-ATTACKER"),
        "body guest-policy overrides must never reach the agent; received:\n{received}"
    );

    close_session(d.port, &agent).await;
}

// ─────────────────────────────────────────────────────────────────────
// 7 — readiness-profile-aware initial-prompt injection (W4, 0.40.30)
// ─────────────────────────────────────────────────────────────────────

/// `configure_ws_agent` variant that also stamps raw migration-0070
/// `readiness` metadata onto the preset row (the shim's basename is
/// `claude`, so WITHOUT the metadata the static table would class it as
/// a poll-trusting provider — declaring `settle:<ms>` proves the preset
/// level of the precedence chain wins on the live spawn path).
fn configure_ws_agent_readiness(ws_name: &str, shim_path: &str, readiness: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let preset_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agent_presets \
             (id, label, command, icon, enabled, sort_order, is_built_in, readiness) \
         VALUES (?1, ?2, ?3, '', 1, 999, 0, ?4)",
        rusqlite::params![
            preset_id,
            format!("hs-shim-{preset_id}"),
            format!("{shim_path} --dangerously-skip-permissions"),
            readiness,
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

/// A SETTLE-profile provider still receives the FULL initial prompt.
///
/// The recording shim (`tee`, basename `claude`) NEVER advertises
/// bracketed paste, and this test raises the readiness ceiling to 5s —
/// so under the pre-W4 poll dialect the injector could only deliver at
/// the 5s best-effort ceiling. The preset declares `settle:250`, which
/// must (a) resolve through the seam as a non-polling 250ms profile and
/// (b) deliver the full preamble-wrapped prompt well before the ceiling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settle_profile_spawn_prompt_reaches_the_pty() {
    let _g = lock();
    let env = HostEnv::set(true);
    // Raise the ceiling ABOVE the declared settle so the two dialects
    // are cleanly distinguishable (HostEnv::Drop restores the prior
    // value). Poll dialect ⇒ delivery at ~5s; settle:250 ⇒ ~250ms.
    std::env::set_var("K2_HOST_SESSION_READY_TIMEOUT_SECS", "5");
    let capture = env.make_shim_recording();
    let d = test_harness::start(OWNER_TOKEN).await;
    let ws_path = setup_project("hs-settle");
    configure_ws_agent_readiness("hs-settle", &env.shim(), "settle:250");

    // (b-seam) The resolution seam itself: preset metadata beats the
    // static table's claude entry — non-polling, exactly 250ms.
    let profile = k2_daemon::v1_host_sessions::policy::resolve_host_injection_profile(
        ws_path.to_string_lossy().as_ref(),
    );
    assert!(
        !profile.ready_via_bracketed_paste,
        "preset-declared settle must override the static claude poll entry"
    );
    assert_eq!(profile.post_spawn_settle, Duration::from_millis(250));

    // (b-live) Spawn WITH an initial prompt and clock the delivery.
    let caller_prompt = "hs-settle-profile-prompt-marker";
    let body = serde_json::json!({ "prompt": caller_prompt }).to_string();
    let started = std::time::Instant::now();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-settle/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let agent = v["agentName"].as_str().expect("agentName").to_string();

    // FULL prompt (preamble + guest policy + caller) on the shim's stdin.
    let received = wait_capture_contains(&capture, caller_prompt, "settle-profile prompt").await;
    let elapsed = started.elapsed();
    let guest = k2_core::workspace::settings::get_api_guest_policy(
        ws_path.to_string_lossy().as_ref(),
    );
    let wrapped =
        k2_daemon::v1_host_sessions::compose_spawn_inject(&guest, caller_prompt);
    assert!(
        received.contains(&wrapped),
        "settle-profile spawn must deliver the FULL wrapped prompt; received:\n{received}"
    );
    // Well inside the 5s ceiling: proves the injector took the 250ms
    // settle wait, not the poll dialect's best-effort-at-ceiling path
    // (generous 4s bound — a full second of margin under the ceiling).
    assert!(
        elapsed < Duration::from_secs(4),
        "prompt took {elapsed:?} — delivery at the ceiling means the settle profile was ignored"
    );

    close_session(d.port, &agent).await;
}

// ─────────────────────────────────────────────────────────────────────
// 8 — Slice W3: self-minting provider adoption → list → resume
// ─────────────────────────────────────────────────────────────────────

/// A `codex`-named shim (self-minting adapter provider: subcommand resume
/// grammar, NO premint) driven END-TO-END through the real dispatcher:
///
///   1. fresh spawn carries NO session identity in argv (nothing to premint);
///   2. the deferred adoption probe discovers the provider-minted id from
///      codex's on-disk store and stamps the `api-…` row → the LIST shows it
///      (live, via the agent-name key — the PTY runs under a different
///      forced daemon SessionId);
///   3. resume-while-LIVE with the ADOPTED id routes into the SAME PTY
///      (no duplicate spawn), echoing the addressed id;
///   4. message-live addressed by the ADOPTED id works (owner recorded at
///      adoption);
///   5. resume-after-death boots a new PTY with codex's SUBCOMMAND grammar
///      (`resume <id>`, preset args dropped) and the response sessionId
///      equals the provider id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_minting_provider_adoption_lists_and_resumes() {
    let _g = lock();
    let env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    let ws_path = setup_project("hs-adopt");
    let codex_shim = env.shim_named("codex");
    configure_ws_agent("hs-adopt", &codex_shim);

    // Pre-seed the provider's on-disk session record (what a real codex
    // writes at boot) BEFORE spawning, so the single-shot 400ms adoption
    // probe deterministically finds it.
    let provider_sid = uuid::Uuid::new_v4().to_string();
    write_codex_session_fixture(&env.tmp_home, &provider_sid, &ws_path.to_string_lossy());

    // (1) Fresh spawn — the FROZEN five-key shape, and a BARE argv: codex
    // has no premint, so the host splices nothing (the danger flag from the
    // preset would also be stripped; here the preset is the bare shim).
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-adopt/host-sessions?token={OWNER_TOKEN}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "codex-shim spawn failed: {resp}");
    let v = json(&resp);
    assert_eq!(v.as_object().map(|o| o.len()), Some(5), "frozen shape; body={resp}");
    assert_eq!(v["sandbox"], "none");
    let spawn_sid = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent = v["agentName"].as_str().expect("agentName").to_string();
    assert_ne!(spawn_sid, provider_sid, "daemon sid is NOT the provider's id");
    {
        let sid = k2_core::session::SessionId::parse(&spawn_sid).expect("uuid");
        let session = v2_session_map::lookup_by_session_id(&sid).expect("registered");
        assert!(
            session.args.is_empty(),
            "self-minting spawn must carry NO session identity in argv: {:?}",
            session.args
        );
    }

    // (2) ADOPTION → the list shows the provider-minted id, LIVE (agent-name
    // keyed liveness — the PTY's daemon SessionId differs). Poll with a hard
    // deadline: the probe fires ~400ms post-spawn.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let listed = loop {
        let (status, resp) = http_req(
            d.port,
            "GET",
            &format!("/v1/w/hs-adopt/host-sessions?token={OWNER_TOKEN}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "list failed: {resp}");
        let l = json(&resp);
        let hit = l["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .find(|e| e["sessionId"] == serde_json::json!(provider_sid))
            .cloned();
        if let Some(entry) = hit {
            break entry;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "adopted session never appeared in the list; last body={resp}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(listed["live"], true, "adopted session must list as LIVE: {listed}");
    assert_eq!(listed["agentName"], serde_json::json!(agent));

    // (3) Resume-while-LIVE with the ADOPTED id → delivered into the SAME
    // PTY (observed on the ORIGINAL session's Term), echoing the adopted id.
    let body = serde_json::json!({ "session": provider_sid, "prompt": "hs-adopt-live-resume" })
        .to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-adopt/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "live resume failed: {resp}");
    let r = json(&resp);
    assert_eq!(r["live"], true, "body={resp}");
    assert_eq!(r["delivered"], true, "body={resp}");
    assert_eq!(r["sessionId"], serde_json::json!(provider_sid), "echo the ADDRESSED id");
    // F4: same slug as cold-spawn / dead-resume — never absolute path.
    assert_eq!(r["workspace"], "hs-adopt", "live resume must return slug; body={resp}");
    // F5: session fields top-level (not nested under capabilities).
    assert_eq!(r["resumed"], true, "body={resp}");
    assert!(r.get("sessionId").is_some());
    if let Some(caps) = r.get("capabilities") {
        assert!(caps.get("sessionId").is_none(), "F5; body={resp}");
        assert!(caps.get("workspace").is_none(), "F5; body={resp}");
    }
    assert_text_appears(&spawn_sid, "hs-adopt-live-resume", "live resume into same PTY").await;

    // (4) Message-live ADDRESSED BY the adopted id (owner recorded at
    // adoption time — the default-deny gate must vouch for it).
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-adopt/host-sessions/{provider_sid}?token={OWNER_TOKEN}"),
        Some(r#"{"prompt":"hs-adopt-msg-by-adopted-id"}"#),
    )
    .await;
    assert_eq!(status, 200, "message-live by adopted id failed: {resp}");
    assert_text_appears(&spawn_sid, "hs-adopt-msg-by-adopted-id", "adopted-id message").await;

    // (5) Kill it, then resume-after-death: codex SUBCOMMAND grammar.
    close_session(d.port, &agent).await;
    let body = serde_json::json!({ "session": provider_sid }).to_string();
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-adopt/host-sessions?token={OWNER_TOKEN}"),
        Some(&body),
    )
    .await;
    assert_eq!(status, 200, "dead resume failed: {resp}");
    let v2 = json(&resp);
    assert_eq!(v2.as_object().map(|o| o.len()), Some(5), "frozen shape; body={resp}");
    assert_eq!(
        v2["sessionId"],
        serde_json::json!(provider_sid),
        "a UUID-shaped provider id rides the forced daemon sid on resume"
    );
    let agent2 = v2["agentName"].as_str().expect("agentName").to_string();
    assert_ne!(agent2, agent, "resume mints a fresh api- agent");
    let resumed = v2_session_map::lookup_by_agent_name(&agent2).expect("resumed PTY registered");
    assert_eq!(
        resumed.args,
        vec!["resume".to_string(), provider_sid.clone()],
        "codex resume grammar: leading subcommand pair, preset args dropped"
    );

    // The resumed spawn's argv stamped the new row (commit-1 grammar scan) —
    // the list still resolves the provider id, and it is LIVE again.
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hs-adopt/host-sessions?token={OWNER_TOKEN}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "post-resume list failed: {resp}");
    let l = json(&resp);
    let live_again = l["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .any(|e| e["sessionId"] == serde_json::json!(provider_sid) && e["live"] == true);
    assert!(live_again, "resumed provider session must list live; body={resp}");

    close_session(d.port, &agent2).await;
}

// ─────────────────────────────────────────────────────────────────────
// 8 — POST …/host-sessions/<id>/kill (integrator spend-cap)
// ─────────────────────────────────────────────────────────────────────

/// Spawn via API key → kill → 200 killed:true; map entry gone; second kill
/// is idempotent killed:false reason:not_live; message-live after kill is 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_session_kill_force_stops_and_is_idempotent() {
    let _g = lock();
    let env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-kill");
    configure_ws_agent("hs-kill", &env.shim());
    let key = mint_api_key(d.port, "hs-kill-key", "hs-kill").await;

    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill/host-sessions?token={key}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let session_id = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent = v["agentName"].as_str().expect("agentName").to_string();

    // Live in the map under the daemon SessionId (premint claude shim).
    let sid = k2_core::session::SessionId::parse(&session_id).expect("valid session uuid");
    assert!(
        v2_session_map::lookup_by_session_id(&sid).is_some(),
        "spawned session must be live in v2_session_map before kill"
    );
    assert!(
        v2_session_map::lookup_by_agent_name(&agent).is_some(),
        "spawned agent_name must be in the map before kill"
    );

    // First kill → 200 killed:true (empty body OK).
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill/host-sessions/{session_id}/kill?token={key}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "kill failed: {resp}");
    let k = json(&resp);
    assert_eq!(k["sessionId"], serde_json::json!(session_id), "body={resp}");
    assert_eq!(k["killed"], true, "body={resp}");
    assert!(k.get("reason").is_none(), "killed:true has no reason; body={resp}");

    // Map entry gone (force unregister).
    assert!(
        v2_session_map::lookup_by_session_id(&sid).is_none(),
        "session must leave v2_session_map after kill"
    );
    assert!(
        v2_session_map::lookup_by_agent_name(&agent).is_none(),
        "agent_name must leave the map after kill"
    );

    // Second kill → idempotent 200 killed:false reason:not_live (ownership
    // still holds; not an existence oracle).
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill/host-sessions/{session_id}/kill?token={key}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "second kill must be idempotent; body={resp}");
    let k2 = json(&resp);
    assert_eq!(k2["sessionId"], serde_json::json!(session_id));
    assert_eq!(k2["killed"], false, "body={resp}");
    assert_eq!(k2["reason"], "not_live", "body={resp}");

    // After kill, message-live is 404 (dead path / not live).
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill/host-sessions/{session_id}?token={key}"),
        Some(r#"{"prompt":"should-404"}"#),
    )
    .await;
    assert_eq!(status, 404, "message-live after kill must 404; body={resp}");
}

/// Cross-principal and ungranted-workspace kill → uniform 404 (no oracle).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_session_kill_refuses_other_principal_and_ungranted_ws() {
    let _g = lock();
    let env = HostEnv::set(true);
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hs-kill-authz");
    configure_ws_agent("hs-kill-authz", &env.shim());
    let owner_key = mint_api_key(d.port, "hs-kill-owner", "hs-kill-authz").await;
    let other_key = mint_api_key(d.port, "hs-kill-other", "hs-kill-authz").await;
    let ungranted = mint_api_key(d.port, "hs-kill-else", "hs-some-other-ws").await;

    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill-authz/host-sessions?token={owner_key}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "spawn failed: {resp}");
    let v = json(&resp);
    let session_id = v["sessionId"].as_str().expect("sessionId").to_string();
    let agent = v["agentName"].as_str().expect("agentName").to_string();

    let uniform = r#"{"error":"no such workspace"}"#;

    // Other principal (granted same workspace) cannot kill owner's session.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill-authz/host-sessions/{session_id}/kill?token={other_key}"),
        None,
    )
    .await;
    assert_eq!(status, 404, "cross-principal kill must 404; body={resp}");
    assert_eq!(resp, uniform, "cross-principal kill: uniform body");

    // Ungranted workspace → same 404 (no oracle that the ws/session exists).
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill-authz/host-sessions/{session_id}/kill?token={ungranted}"),
        None,
    )
    .await;
    assert_eq!(status, 404, "ungranted ws kill must 404; body={resp}");
    assert_eq!(resp, uniform, "ungranted kill: uniform body");

    // Session still live — cross-principal must not have torn it down.
    let sid = k2_core::session::SessionId::parse(&session_id).expect("valid session uuid");
    assert!(
        v2_session_map::lookup_by_session_id(&sid).is_some(),
        "failed kill attempts must leave the session live"
    );

    // Owner can still kill.
    let (status, resp) = http_req(
        d.port,
        "POST",
        &format!("/v1/w/hs-kill-authz/host-sessions/{session_id}/kill?token={owner_key}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "owner kill failed: {resp}");
    assert_eq!(json(&resp)["killed"], true, "body={resp}");

    // Cleanup in case kill left map dirt (should be gone).
    if v2_session_map::lookup_by_agent_name(&agent).is_some() {
        close_session(d.port, &agent).await;
    }
}
