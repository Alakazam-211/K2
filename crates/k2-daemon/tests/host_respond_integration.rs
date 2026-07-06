//! F2 (prd-v1-api-completion §4) — HOST response read-back: the in-session
//! agent's `k2 respond` riding the LOOPBACK TCP `/cli/respond` arm with a
//! SCOPED per-session hook token, drained by
//! `GET /v1/w/<ws>/host-sessions/<id>/messages`. Real dispatcher end-to-end.
//!
//! Contract under test:
//!
//!   1. ROUND-TRIP: spawn a host session via the API, post `/cli/respond`
//!      with a scoped token bound to that session (exactly what the `k2`
//!      CLI's TCP fallback sends), read it back at `latest_seq` via the
//!      messages route; `--final` and the since-cursor behave.
//!   2. OWNER TOKEN REFUSED: the owner token carries no session identity —
//!      `/cli/respond` 403s it (never a silent append).
//!   3. CROSS-SESSION APPEND REFUSED BY CONSTRUCTION: a token bound to
//!      session A can only ever append to A — session B's log stays empty
//!      no matter what the request carries.
//!   4. POST-only guard: GET `/cli/respond` → 405.
//!
//! Note on minting: the daemon injects the scoped token into the child env
//! at spawn only under `K2_HOOK_SCOPED`; the tests mint a token for the
//! spawned session id directly via `session_token::mint_session_token` —
//! the SAME registry + claims the spawn-time mint produces — so the wire
//! contract is exercised without shelling the bash CLI.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;

use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "host-respond-owner-token-f2";

async fn http_req(
    port: u16,
    method: &str,
    path_and_query: &str,
    bearer: Option<&str>,
    content_type: &str,
    body: Option<&str>,
) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let auth = bearer
        .map(|b| format!("Authorization: Bearer {b}\r\n"))
        .unwrap_or_default();
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n\r\n"
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

// ── Env + workspace scaffolding (host_sessions suite pattern) ────────

struct RespondEnv {
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    tmp_home: std::path::PathBuf,
    shim_dir: std::path::PathBuf,
}

impl RespondEnv {
    fn set() -> Self {
        let names: [&'static str; 3] = ["K2_API", "K2_SANDBOX_API", "HOME"];
        let prev: Vec<_> = names.iter().map(|n| (*n, std::env::var_os(n))).collect();
        std::env::set_var("K2_API", "1");
        std::env::remove_var("K2_SANDBOX_API");

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_home = std::env::temp_dir()
            .join(format!("k2-host-respond-f2-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).expect("create temp HOME");
        std::env::set_var("HOME", &tmp_home);

        // `claude`-named shim at an ABSOLUTE path, wired per-workspace via
        // the agent_presets/default_agent seam (host_sessions suite pattern
        // — the daemon's login-PATH enrichment can't shadow an abs path).
        let shim_dir = std::env::temp_dir()
            .join(format!("k2-host-respond-shim-{}-{nanos}", std::process::id()));
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

    fn shim(&self) -> String {
        self.shim_dir.join("claude").to_string_lossy().into_owned()
    }
}

/// Point `ws_name`'s default agent at the shim preset (real seam).
fn configure_ws_agent(ws_name: &str, shim_path: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let preset_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
         VALUES (?1, ?2, ?3, '', 1, 999, 0)",
        rusqlite::params![preset_id, format!("hr-shim-{preset_id}"), shim_path],
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

impl Drop for RespondEnv {
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

fn setup_project(name: &str) -> std::path::PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2-host-respond-ws-{name}-{}-{}",
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

/// Spawn a host session in `ws` via the REAL API; return (sessionId, agentName).
async fn spawn_host_session(port: u16, ws: &str) -> (String, String) {
    let (status, resp) = http_req(
        port,
        "POST",
        &format!("/v1/w/{ws}/host-sessions?token={OWNER_TOKEN}"),
        None,
        "application/json",
        Some("{}"),
    )
    .await;
    assert_eq!(status, 200, "host spawn failed: {resp}");
    let v = json(&resp);
    (
        v["sessionId"].as_str().expect("sessionId").to_string(),
        v["agentName"].as_str().expect("agentName").to_string(),
    )
}

async fn close_session(port: u16, agent_name: &str) {
    let (status, body) = http_req(
        port,
        "POST",
        &format!("/cli/sessions/v2/close?token={OWNER_TOKEN}"),
        None,
        "application/json",
        Some(&serde_json::json!({ "agent_name": agent_name, "force": true }).to_string()),
    )
    .await;
    assert_eq!(status, 200, "close {agent_name} failed: {body}");
}

/// Mint a scoped hook token bound to `session_id` — the same registry mint
/// the daemon performs at spawn under K2_HOOK_SCOPED.
fn mint_scoped(session_id: &str, agent: &str) -> String {
    let sid = k2_core::session::SessionId::parse(session_id).expect("uuid");
    k2_daemon::session_token::mint_session_token(
        &sid,
        session_id, // pane_id == session id for daemon spawns
        HookPrincipal {
            workspace_uuid: "test-ws-uuid".to_string(),
            agent_address: agent.to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

// ─────────────────────────────────────────────────────────────────────
// 1 — the round trip
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respond_round_trip_reaches_the_messages_route() {
    let _g = lock();
    let env = RespondEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hr-round");
    configure_ws_agent("hr-round", &env.shim());
    let (session_id, agent) = spawn_host_session(d.port, "hr-round").await;
    let scoped = mint_scoped(&session_id, &agent);

    // The in-session agent emits a progress line (exactly the k2 CLI's TCP
    // fallback wire: POST form body, scoped Bearer).
    let (status, resp) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        Some(&scoped),
        "application/x-www-form-urlencoded",
        Some("message=working%20on%20it&final=0"),
    )
    .await;
    assert_eq!(status, 200, "respond #1 failed: {resp}");
    assert_eq!(json(&resp)["seq"], 1, "body={resp}");

    // …then its final answer.
    let (status, resp) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        Some(&scoped),
        "application/x-www-form-urlencoded",
        Some("message=all%20done&final=1"),
    )
    .await;
    assert_eq!(status, 200, "respond #2 failed: {resp}");
    assert_eq!(json(&resp)["seq"], 2, "body={resp}");

    // The API caller drains the log through the host-sessions family.
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hr-round/host-sessions/{session_id}/messages?since=0&token={OWNER_TOKEN}"),
        None,
        "application/json",
        None,
    )
    .await;
    assert_eq!(status, 200, "messages read failed: {resp}");
    let v = json(&resp);
    assert_eq!(v["latest_seq"], 2, "body={resp}");
    let msgs = v["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 2, "body={resp}");
    assert_eq!(msgs[0]["text"], "working on it");
    assert_eq!(msgs[0]["final"], false);
    assert_eq!(msgs[1]["text"], "all done");
    assert_eq!(msgs[1]["final"], true);

    // The since-cursor holds: a caught-up poll is empty at the same seq.
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hr-round/host-sessions/{session_id}/messages?since=2&token={OWNER_TOKEN}"),
        None,
        "application/json",
        None,
    )
    .await;
    assert_eq!(status, 200);
    let v = json(&resp);
    assert_eq!(v["latest_seq"], 2, "cursor held; body={resp}");
    assert!(v["messages"].as_array().expect("messages").is_empty());

    close_session(d.port, &agent).await;
}

// ─────────────────────────────────────────────────────────────────────
// 2 — owner token refused; bad/missing token refused; POST-only
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respond_refuses_owner_token_garbage_and_get() {
    let _g = lock();
    let _env = RespondEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;

    // Owner token (Bearer) → 403: it names no session, so it can never
    // authorize a session-pinned append.
    let (status, resp) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        Some(OWNER_TOKEN),
        "application/x-www-form-urlencoded",
        Some("message=nope"),
    )
    .await;
    assert_eq!(status, 403, "owner token must be refused; body={resp}");

    // Owner token via ?token= → same refusal.
    let (status, _) = http_req(
        d.port,
        "POST",
        &format!("/cli/respond?token={OWNER_TOKEN}"),
        None,
        "application/x-www-form-urlencoded",
        Some("message=nope"),
    )
    .await;
    assert_eq!(status, 403, "owner query token must be refused");

    // Garbage / absent token → 403.
    let (status, _) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        Some("not-a-real-token.aaaa"),
        "application/x-www-form-urlencoded",
        Some("message=nope"),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        None,
        "application/x-www-form-urlencoded",
        Some("message=nope"),
    )
    .await;
    assert_eq!(status, 403);

    // GET → 405 (POST-only guard).
    let (status, _) = http_req(d.port, "GET", "/cli/respond", None, "text/plain", None).await;
    assert_eq!(status, 405, "GET /cli/respond must 405");
}

// ─────────────────────────────────────────────────────────────────────
// 3 — cross-session append refused by construction
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_token_appends_only_to_its_own_session() {
    let _g = lock();
    let env = RespondEnv::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    setup_project("hr-cross");
    configure_ws_agent("hr-cross", &env.shim());
    let (sid_a, agent_a) = spawn_host_session(d.port, "hr-cross").await;
    let (sid_b, agent_b) = spawn_host_session(d.port, "hr-cross").await;
    assert_ne!(sid_a, sid_b);
    let scoped_a = mint_scoped(&sid_a, &agent_a);

    // A's token appends — the request body has NO way to name a session,
    // so even a hostile body field can't redirect the write to B.
    let hostile_body = format!("message=from-a&session={sid_b}&session_id={sid_b}");
    let (status, resp) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        Some(&scoped_a),
        "application/x-www-form-urlencoded",
        Some(&hostile_body),
    )
    .await;
    assert_eq!(status, 200, "A's own append must succeed: {resp}");

    // B's log is EMPTY; A's log holds the line.
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hr-cross/host-sessions/{sid_b}/messages?since=0&token={OWNER_TOKEN}"),
        None,
        "application/json",
        None,
    )
    .await;
    assert_eq!(status, 200, "B read: {resp}");
    assert!(
        json(&resp)["messages"].as_array().expect("messages").is_empty(),
        "a token bound to A must NEVER write B's log; body={resp}"
    );
    let (status, resp) = http_req(
        d.port,
        "GET",
        &format!("/v1/w/hr-cross/host-sessions/{sid_a}/messages?since=0&token={OWNER_TOKEN}"),
        None,
        "application/json",
        None,
    )
    .await;
    assert_eq!(status, 200, "A read: {resp}");
    let v = json(&resp);
    assert_eq!(v["latest_seq"], 1, "A holds its own line; body={resp}");
    assert_eq!(v["messages"][0]["text"], "from-a");

    // A REVOKED token (cell teardown) stops appending immediately.
    let sid_a_parsed = k2_core::session::SessionId::parse(&sid_a).expect("uuid");
    k2_daemon::session_token::revoke_session(&sid_a_parsed);
    let (status, _) = http_req(
        d.port,
        "POST",
        "/cli/respond",
        Some(&scoped_a),
        "application/x-www-form-urlencoded",
        Some("message=late"),
    )
    .await;
    assert_eq!(status, 403, "revoked scoped token must be refused");

    close_session(d.port, &agent_a).await;
    close_session(d.port, &agent_b).await;
}
