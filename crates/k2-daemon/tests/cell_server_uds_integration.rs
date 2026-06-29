//! #58 Phase-1 close — end-to-end tests for the per-cell scoped UDS verb
//! server (`cell_server::serve_cell`). Mirrors the awareness_ws_integration
//! harness: bind the real per-cell socket, hand the listener to the real
//! `serve_cell`, then drive raw HTTP/1.1 requests over a tokio `UnixStream`
//! and assert the status + body for the full auth + stamp + dispatch stack.
//!
//! These prove the load-bearing security properties: a scoped token is bound
//! to its OWN cell (can't be replayed on another cell's socket), the
//! capability allowlist denies the owner-escalation surface even with a valid
//! token, the sender identity is stamped server-side (never the body), and a
//! revoked session 403s.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use k2_core::session::SessionId;
use k2_daemon::cell_uds;
use k2_daemon::cell_server;
use k2_daemon::session_token::{self, HookPrincipal};

/// The scoped-token registry + `$HOME` + the per-cell socket dir are
/// process-wide singletons; serialize every test that mutates them.
static ENV_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A SHORT `$HOME` under `/tmp` — a Unix socket path is capped at `SUN_LEN`
/// (~104 bytes on macOS), and `~/.k2/run/cells/<uuid>.sock` under the real
/// `/var/folders/...` tempdir blows past it. Point `$HOME` here so the bound
/// path (and `hook-sessions.json`) stay isolated + short.
fn set_short_home() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = PathBuf::from(format!("/tmp/k2c3{}{}", std::process::id(), nanos % 1_000_000));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);
    // In-memory DB so handle_hook_complete (which locks the shared DB) +
    // inbox handlers are side-effect-free + deterministic.
    let _ = k2_core::db::init_for_tests();
    tmp
}

fn principal() -> HookPrincipal {
    HookPrincipal {
        // Empty workspace_uuid → no own-workspace path resolution, so cases
        // that pass `project=` explicitly are honored verbatim.
        workspace_uuid: String::new(),
        agent_address: "agent-c3".to_string(),
    }
}

/// Mint a real scoped token for a fresh session bound to `pane`, bind that
/// cell's socket, and start serving it. Returns (session_id, token, sock).
fn mint_bind_serve(pane: &str) -> (SessionId, String, PathBuf) {
    let sid = SessionId::new();
    let token = session_token::mint_session_token(&sid, pane, principal());
    let listener = cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = cell_uds::cell_socket_path(&sid);
    // Non-microVM cell (no peer-uid widening) — exercises the same-uid belt.
    cell_server::serve_cell(sid, listener, false);
    (sid, token, sock)
}

// ── Sandbox P1 / Finding-1 helpers (principal resolves to a real workspace) ──

/// A UUID-shaped string (8-4-4-4-12) so `workspace_msg::resolve_workspace`
/// matches it against `projects.id`. Derived from the wall clock so each call
/// is distinct within a test process.
fn fresh_uuid() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{:08x}-0000-4000-8000-{:012x}",
        (n & 0xffff_ffff) as u32,
        (n >> 16) as u64 & 0xffff_ffff_ffff
    )
}

/// Create a real workspace (dir on disk + `projects` row) under `home` and
/// return (uuid, path). A principal whose `workspace_uuid` is this uuid then
/// resolves to this path, so the Finding-1 force pins every operand to it.
fn mk_workspace(home: &Path, name: &str) -> (String, PathBuf) {
    let path = home.join(name);
    std::fs::create_dir_all(&path).expect("create workspace dir");
    let uuid = fresh_uuid();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
        rusqlite::params![uuid, path.to_string_lossy().as_ref(), name],
    )
    .expect("insert workspace project row");
    (uuid, path)
}

/// A principal bound to workspace `uuid` (resolvable via `mk_workspace`).
fn principal_in_ws(uuid: &str) -> HookPrincipal {
    HookPrincipal {
        workspace_uuid: uuid.to_string(),
        agent_address: "agent-c3".to_string(),
    }
}

/// Like `mint_bind_serve`, but the principal's workspace resolves to a real
/// path — so the Finding-1 force pins operands to it instead of failing closed.
fn mint_bind_serve_in_ws(pane: &str, ws_uuid: &str) -> (SessionId, String, PathBuf) {
    let sid = SessionId::new();
    let token = session_token::mint_session_token(&sid, pane, principal_in_ws(ws_uuid));
    let listener = cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = cell_uds::cell_socket_path(&sid);
    // Non-microVM cell (no peer-uid widening) — exercises the same-uid belt.
    cell_server::serve_cell(sid, listener, false);
    (sid, token, sock)
}

/// Count `.md` files under `dir` (recursively) — the inbox memo footprint.
fn md_count(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += md_count(&p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("md") {
                n += 1;
            }
        }
    }
    n
}

/// Fire one raw HTTP/1.1 request at the cell socket and return (status, body).
/// One request per connection (the server reads one request then closes), so
/// we read until EOF.
async fn uds(sock: &Path, raw: &str) -> (u16, String) {
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write request");
    stream.flush().await.ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line in response: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn get(path_and_query: &str, bearer: Option<&str>) -> String {
    let auth = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    format!("GET {path_and_query} HTTP/1.1\r\nHost: localhost\r\n{auth}\r\n")
}

fn post_form(path: &str, bearer: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\n\
         Authorization: Bearer {bearer}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

// Give the spawned accept loop a beat to come up before connecting.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case1_correct_pane_hook_complete_is_200() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, token, sock) = mint_bind_serve("pane-1");
    settle().await;
    let (status, _body) = uds(
        &sock,
        &get("/hook/complete?paneId=pane-1&eventType=stop", Some(&token)),
    )
    .await;
    assert_eq!(status, 200, "scoped token + correct pane must complete the hook");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case2_wrong_pane_is_403() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, token, sock) = mint_bind_serve("pane-1");
    settle().await;
    let (status, _) = uds(
        &sock,
        &get("/hook/complete?paneId=pane-WRONG&eventType=stop", Some(&token)),
    )
    .await;
    assert_eq!(status, 403, "a token scoped to pane-1 must NOT complete pane-WRONG");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case3_other_cells_token_on_this_socket_is_403() {
    let _g = lock();
    let _home = set_short_home();
    // Cell A's socket; cell B's token.
    let (_sid_a, _tok_a, sock_a) = mint_bind_serve("pane-A");
    let other_sid = SessionId::new();
    let tok_b = session_token::mint_session_token(&other_sid, "pane-B", principal());
    settle().await;
    let (status, _) = uds(
        &sock_a,
        &get("/hook/complete?paneId=pane-B&eventType=stop", Some(&tok_b)),
    )
    .await;
    assert_eq!(status, 403, "cell B's token must NOT be accepted on cell A's socket");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case4_users_set_role_with_valid_bearer_is_403() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, token, sock) = mint_bind_serve("pane-1");
    settle().await;
    // A VALID scoped token on an owner-escalation route: require_hook denies
    // it pre-handler (capability default-deny), so 403 — never reaches the
    // user-management code.
    let (status, _) = uds(&sock, &post_form("/cli/users/set-role", &token, "username=x&role=admin")).await;
    assert_eq!(status, 403, "scoped token must NOT reach /cli/users/set-role");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case5_inbox_list_is_200_with_parseable_json() {
    let _g = lock();
    let home = set_short_home();
    // The principal owns workspace W; Finding-1 forces the operand to W.
    let (ws_uuid, _ws) = mk_workspace(&home, "ws");
    let (_sid, token, sock) = mint_bind_serve_in_ws("pane-1", &ws_uuid);
    settle().await;
    // `project=` is FORCED to the principal's own workspace; a bogus operand
    // still yields a valid own-workspace listing (never a 400/foreign read).
    let (status, body) =
        uds(&sock, &get("/cli/inbox/list?project=/bogus-other-ws", Some(&token))).await;
    assert_eq!(status, 200, "inbox list over the cell socket must 200; body={body}");
    let _v: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("inbox list body must be JSON: {e}; body={body}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case6_msg_is_200_and_from_is_stamped_not_forged() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, token, sock) = mint_bind_serve("pane-1");
    settle().await;
    // The body FORGES a sender identity; the daemon must stamp `from` from
    // the socket-bound principal ("agent-c3"), so the forged value never
    // takes effect. Workspace won't resolve (no project) → fast return.
    let (status, body) = uds(
        &sock,
        &post_form(
            "/cli/workspace/msg",
            &token,
            "workspace=nonexistent-ws&text=hi&from=FORGED-ATTACKER&wake=false",
        ),
    )
    .await;
    assert_eq!(status, 200, "msg over the cell socket must 200; body={body}");
    let _v: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("msg body must be JSON: {e}; body={body}"));
    assert!(
        !body.contains("FORGED-ATTACKER"),
        "the forged `from` must NOT be reflected (identity is server-stamped); body={body}"
    );
}

/// Durable proof that `from` is stamped from the principal, not the body:
/// compose an inbox memo with a FORGED `from`, then read it back and assert
/// the stored attribution is the principal's agent_address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case6b_inbox_compose_stamps_principal_from_durably() {
    let _g = lock();
    let home = set_short_home();
    // The principal owns workspace W; the operand is forced to it.
    let (ws_uuid, ws) = mk_workspace(&home, "wsc");
    let (_sid, token, sock) = mint_bind_serve_in_ws("pane-1", &ws_uuid);
    let proj = urlencode(ws.to_str().unwrap());
    settle().await;

    let (status, body) = uds(
        &sock,
        &post_form(
            "/cli/inbox/compose",
            &token,
            &format!("project={proj}&title=t1&body=hello&from=FORGED-ATTACKER"),
        ),
    )
    .await;
    assert_eq!(status, 200, "inbox compose must 200; body={body}");

    // Read the composed item back and assert its `from` is the principal.
    let (rstatus, rbody) = uds(
        &sock,
        &get(&format!("/cli/inbox/list?project={proj}"), Some(&token)),
    )
    .await;
    assert_eq!(rstatus, 200, "inbox list must 200; body={rbody}");
    assert!(
        !rbody.contains("FORGED-ATTACKER"),
        "stored memo must NOT carry the forged from; body={rbody}"
    );
    assert!(
        rbody.contains("agent-c3"),
        "stored memo must carry the principal-stamped from (agent-c3); body={rbody}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case7_terminal_write_is_not_200() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, token, sock) = mint_bind_serve("pane-1");
    settle().await;
    // terminal/* is HARD-DENIED (RCE/inject primitive): require_hook denies
    // it → 403 (never 200, never reaches any terminal code).
    let (status, _) = uds(&sock, &get("/cli/terminal/write?paneId=pane-1&data=rm", Some(&token))).await;
    assert_ne!(status, 200, "terminal/write must never succeed over the cell socket");
    assert_eq!(status, 403, "terminal/write is capability-denied pre-handler (403)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case8_no_bearer_no_token_is_403() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, _token, sock) = mint_bind_serve("pane-1");
    settle().await;
    let (status, _) = uds(&sock, &get("/hook/complete?paneId=pane-1&eventType=stop", None)).await;
    assert_eq!(status, 403, "no credential → 403");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case9_revoked_session_is_403() {
    let _g = lock();
    let home = set_short_home();
    // The principal owns workspace W; the operand is forced to it.
    let (ws_uuid, _ws) = mk_workspace(&home, "wsr");
    let (sid, token, sock) = mint_bind_serve_in_ws("pane-1", &ws_uuid);
    let q = "/cli/inbox/list?project=/bogus-other-ws".to_string();
    settle().await;

    // Pre-revoke: authorized → 200.
    let (pre, _) = uds(&sock, &get(&q, Some(&token))).await;
    assert_eq!(pre, 200, "token valid before revoke");

    // Revoke this cell's session (the teardown signal) → the token 403s.
    session_token::revoke_session(&sid);
    let (post, _) = uds(&sock, &get(&q, Some(&token))).await;
    assert_eq!(post, 403, "a revoked session token must 403 within one request");
}

/// #58 red-team (Finding 2): an attacker-declared oversized `Content-Length`
/// must be REFUSED with 413 before the body is read/allocated — the host
/// daemon must not be driveable toward OOM via a sealed cell's request body.
/// We send only the head (huge declared length, no body) and assert 413 comes
/// back without the server blocking to read the phantom body.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case10_oversized_content_length_is_413() {
    let _g = lock();
    let _home = set_short_home();
    let (_sid, token, sock) = mint_bind_serve("pane-1");
    settle().await;
    // 64 MiB declared (> MAX_BODY = 4 MiB), but we send ZERO body bytes — the
    // server must reject on the declared length alone, not hang waiting for it.
    let raw = format!(
        "POST /cli/inbox/compose HTTP/1.1\r\nHost: localhost\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: 67108864\r\n\r\n"
    );
    let (status, _) = uds(&sock, &raw).await;
    assert_eq!(status, 413, "an oversized declared body must be refused with 413");
}

/// Sandbox P1 — Finding 1 (e2e): a scoped request that addresses ANOTHER
/// workspace (`project=<OTHER>`) operates on the PRINCIPAL's OWN workspace W,
/// not OTHER. Compose addressed at OTHER lands under W; the list addressed at
/// OTHER returns W's listing (which now holds the item); and on disk the memo
/// is under W's inbox, never OTHER's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case11_project_is_forced_to_principal_workspace() {
    let _g = lock();
    let home = set_short_home();

    // W = the principal's own workspace; OTHER = a foreign workspace dir the
    // attacker's body tries to address.
    let (ws_uuid, w_path) = mk_workspace(&home, "W");
    let other_path = home.join("OTHER");
    std::fs::create_dir_all(&other_path).expect("create OTHER dir");
    let (_sid, token, sock) = mint_bind_serve_in_ws("pane-1", &ws_uuid);
    settle().await;

    let marker = "FINDING1ForceMarker";
    let other_proj = urlencode(other_path.to_str().unwrap());

    // Compose addressed at OTHER — Finding 1 FORCES the operand to W.
    let (status, body) = uds(
        &sock,
        &post_form(
            "/cli/inbox/compose",
            &token,
            &format!("project={other_proj}&title={marker}&body=x"),
        ),
    )
    .await;
    assert_eq!(status, 200, "compose must 200; body={body}");

    // List addressed at OTHER — also forced to W → returns W's listing, which
    // now contains the composed item.
    let (lstatus, lbody) = uds(
        &sock,
        &get(&format!("/cli/inbox/list?project={other_proj}"), Some(&token)),
    )
    .await;
    assert_eq!(lstatus, 200, "list must 200; body={lbody}");
    assert!(
        lbody.contains(marker),
        "the composed item must appear in W's listing (operand forced to W); body={lbody}"
    );

    // On disk: the memo landed under W, and NOTHING landed under OTHER.
    let w_inbox = k2_core::inbox::inbox_root(&w_path);
    let other_inbox = k2_core::inbox::inbox_root(&other_path);
    assert!(
        md_count(&w_inbox) > 0,
        "the composed memo must exist under W's inbox ({w_inbox:?})"
    );
    assert_eq!(
        md_count(&other_inbox),
        0,
        "NO memo may land under OTHER's inbox ({other_inbox:?}) — the operand was forced to W"
    );
}

/// Minimal query-string percent-encoding for a filesystem path.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
