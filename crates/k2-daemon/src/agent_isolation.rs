//! Agent Isolation M1, the daemon's half (`plan-agent-isolation-build-v1`
//! §3; owner decisions 2026-11-21). The kernel half (per-agent cgroups,
//! the exec helper, the Landlock fence) is `k2_core::agent_cgroup` and
//! `k2_core::agent_fence`.
//!
//! - **Spawn** ([`for_spawn`]): every terminal session of an agent runs in
//!   `agents/a-<workspace-id>/` when the box has the identity, and its
//!   passport is bound to that cgroup (`session_token`'s record).
//! - **Session socket** ([`check_socket_peer`]): a bound passport is
//!   accepted only from a process in its own agent's cgroup. Anything
//!   else: 403 `passport_wrong_cgroup` and an audit row naming both the
//!   passport's agent and the cgroup's.
//! - **Loopback TCP** ([`tcp_refusal`]): a TCP connection carries no peer
//!   process, so a bound passport is refused there (403
//!   `passport_needs_socket`, audit row), unless the request is the session
//!   socket's own forward ([`forward_to_tcp`]): the `k2` CLI in a session
//!   sends every passport request over the socket, marking the ones it
//!   used to send over TCP (`X-K2-Via-Tcp: 1`); the socket checks the
//!   cgroup, then hands the request to the TCP dispatcher unchanged with a
//!   proof only this daemon process holds. The same route code and the same
//!   guards as TCP, for every route, with no per-route list to fall behind.
//! - **whois** ([`handle_whois`], `GET /cli/agent-whois?pid=N`, owner
//!   only): which agent owns a process.
//! - **The fence switch** (`POST /cli/agent-access/set {toggle: "fence"}`,
//!   owner; an Admin may only turn it on): per agent, off by default.
//!
//! The claim is "traceable, and a copied passport alone no longer works"
//! ([`k2_core::agent_cgroup::CLAIM`]); never "cannot be forged" while the
//! cgroups belong to the K2 user (plan M1b: root-owned cgroups).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use k2_core::agent_cgroup::{self, Decision, PeerAgent, SessionIsolation};
use k2_core::log_debug;
use k2_core::session::SessionId;

/// The owner-only whois route.
pub const WHOIS_ROUTE: &str = "/cli/agent-whois";
/// The CLI's mark on a request it used to send over loopback TCP.
pub const VIA_TCP_HEADER: &str = "x-k2-via-tcp";
/// The socket's forward to TCP: this process's proof, and the session.
pub const PROOF_HEADER: &str = "x-k2-cell-proof";
pub const PROOF_SESSION_HEADER: &str = "x-k2-cell-session";

// ── spawn ─────────────────────────────────────────────────────────────

/// The agent's workspace folder and whether its owner turned the folder
/// fence on (`projects.isolation_mode`, migration 0323).
pub fn workspace_and_fence(ws: &str) -> (Option<PathBuf>, bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row: Option<(String, String)> = conn
        .query_row("SELECT path, isolation_mode FROM projects WHERE id = ?1", [ws], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    match row {
        Some((path, mode)) => (Some(PathBuf::from(path)), mode == "fence"),
        None => (None, false),
    }
}

/// The isolation for a new terminal session of workspace `ws` (`cwd` when
/// the workspace has no row), binding its passport to the agent's cgroup
/// when the session gets the identity and has a passport (`minted`).
pub fn for_spawn(sid: &SessionId, ws: &str, cwd: &str, passthrough: bool, minted: bool) -> Option<SessionIsolation> {
    let ws = ws.trim();
    if ws.is_empty() {
        return None;
    }
    let (path, fence) = workspace_and_fence(ws);
    let workspace = path.unwrap_or_else(|| PathBuf::from(cwd));
    let iso = agent_cgroup::intent(ws, passthrough, fence, Some(Path::new(&workspace)))?;
    if iso.identity && minted && !crate::session_token::bind_agent_cgroup(sid, &iso.ws) {
        log_debug!("[agents] session={sid}: no passport record to bind to agent {}", iso.ws);
    }
    Some(iso)
}

// ── audit ─────────────────────────────────────────────────────────────

/// One audit row for a refused passport: the passport's agent, where it
/// was presented from, and how.
pub fn audit_refusal(event: &str, passport_ws: &str, sid: &str, detail: &str, ingress: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        &format!("agent:{passport_ws}"),
        format!("refused: {detail} (session {sid})"),
        ingress,
        "-",
        "cli",
    ));
    log_debug!("[agents] {event}: {detail} (session {sid}, {ingress})");
}

// ── the session socket ────────────────────────────────────────────────

/// The refusal body for a passport presented from the wrong cgroup.
pub fn wrong_cgroup_body(detail: &str) -> String {
    serde_json::json!({
        "error": format!("this passport works only from its own agent's terminals: {detail}"),
        "code": "passport_wrong_cgroup",
        "hint": "run k2 from the agent's own terminal; a passport copied to another shell doesn't work",
    })
    .to_string()
}

/// The cell socket's identity gate for an authorized passport: `Some(body)`
/// = 403. `fd` is the connection, `v` the validated passport.
#[cfg(unix)]
pub fn check_socket_peer(fd: std::os::fd::RawFd, v: &crate::session_token::ValidatedHook) -> Option<String> {
    let bound = v.agent_cgroup.as_deref()?;
    let Ok(a) = agent_cgroup::availability() else {
        // Identity unavailable now (the off-switch, or the box lost its
        // delegated unit): passports as before.
        return None;
    };
    #[cfg(target_os = "linux")]
    let (peer, pid, _pinned) = agent_cgroup::peer_of_socket(fd, &a);
    #[cfg(not(target_os = "linux"))]
    let (peer, pid) = {
        let _ = (fd, &a);
        (PeerAgent::Unknown { why: "no cgroups here".into() }, None::<u32>)
    };
    match agent_cgroup::decide(Some(bound), &peer) {
        Decision::Allow => None,
        Decision::Refuse { detail } => {
            let detail = match pid {
                Some(p) => format!("{detail}, pid {p}"),
                None => detail,
            };
            audit_refusal("agent.passport_wrong_cgroup", bound, &v.session_id, &detail, "cell-socket");
            Some(wrong_cgroup_body(&detail))
        }
    }
}

/// This daemon process's proof for the socket's forward to TCP (random,
/// in memory only: never on disk, never in a child's environment).
fn proof() -> &'static str {
    static P: OnceLock<String> = OnceLock::new();
    P.get_or_init(|| format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()))
}

fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// The request head as the socket forwards it: the client's own
/// `X-K2-Cell-*` / `X-K2-Via-Tcp` / `Connection` lines dropped, then this
/// process's proof for `sid` and `Connection: close`.
pub fn forward_head(head: &str, sid: &str) -> String {
    let mut lines = head.split("\r\n");
    let mut out = String::with_capacity(head.len() + 160);
    out.push_str(lines.next().unwrap_or(""));
    out.push_str("\r\n");
    for l in lines {
        let name = l.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
        if l.is_empty() || name.starts_with("x-k2-cell-") || name == VIA_TCP_HEADER || name == "connection" || name == "proxy-connection" {
            continue;
        }
        out.push_str(l);
        out.push_str("\r\n");
    }
    out.push_str(&format!("X-K2-Cell-Proof: {}\r\nX-K2-Cell-Session: {sid}\r\nConnection: close\r\n\r\n", proof()));
    out
}

/// `true` when the request is the session socket's forward for `sid`.
pub fn is_forward_for(head: &str, sid: &str) -> bool {
    let p = crate::routes::http::extract_header(head, PROOF_HEADER).unwrap_or("");
    let s = crate::routes::http::extract_header(head, PROOF_SESSION_HEADER).unwrap_or("");
    !p.is_empty() && ct_eq(p.trim(), proof()) && s.trim() == sid
}

/// Hand a request the CLI marked `X-K2-Via-Tcp` to the TCP dispatcher on
/// loopback, after the socket's checks, and copy its answer back.
#[cfg(unix)]
pub async fn forward_to_tcp(stream: &mut tokio::net::UnixStream, head: &str, body: &[u8], sid: &str) {
    use tokio::io::AsyncWriteExt;
    let port = k2_core::hook_config::get_port();
    let mut tcp = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(t) => t,
        Err(e) => {
            let b = serde_json::json!({ "error": format!("the session socket couldn't reach the daemon's own port: {e}") }).to_string();
            let resp = format!("HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}", b.len());
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        }
    };
    let fwd = forward_head(head, sid);
    if tcp.write_all(fwd.as_bytes()).await.is_err() || tcp.write_all(body).await.is_err() {
        let _ = stream.shutdown().await;
        return;
    }
    // Answers stream (watch / tail -f): copy until the daemon closes.
    let _ = tokio::io::copy(&mut tcp, stream).await;
    let _ = stream.shutdown().await;
}

// ── loopback TCP ──────────────────────────────────────────────────────

/// The refusal for a bound passport on TCP.
pub fn needs_socket_body() -> String {
    serde_json::json!({
        "error": format!(
            "this agent's passport works only on its own session socket, not over TCP ({} checks which agent's cgroup the caller is in, and a TCP connection doesn't say)",
            k2_core::brand::PRODUCT_NAME
        ),
        "code": "passport_needs_socket",
        "hint": format!("run {} from the agent's own terminal (it uses the session socket); a copied passport doesn't work elsewhere", k2_core::brand::CLI_NAME),
    })
    .to_string()
}

/// The TCP dispatcher's gate: `Some(body)` = 403 for a request carrying a
/// passport bound to an agent cgroup, unless it is the session socket's own
/// forward for that session. Inert when identity is unavailable (the
/// emergency off-switch included).
pub fn tcp_refusal(query: &str, bearer: Option<&str>, head: &str, ingress: &str) -> Option<String> {
    let presented = crate::routes::http::presented_bearer(query, bearer);
    if presented.is_empty() || !presented.contains('.') {
        return None;
    }
    let (sid, ws) = crate::session_token::bound_agent_of_bearer(presented)?;
    agent_cgroup::availability().ok()?;
    if is_forward_for(head, &sid) {
        return None;
    }
    audit_refusal("agent.passport_needs_socket", &ws, &sid, "a bound passport was presented over TCP", ingress);
    Some(needs_socket_body())
}

// ── whois ─────────────────────────────────────────────────────────────

/// The request's token (`token=`, the dispatcher folds the Bearer into it)
/// is the owner token.
pub fn owner_in(params: &std::collections::HashMap<String, String>) -> bool {
    let owner = k2_core::hook_config::get_token();
    params.get("token").is_some_and(|t| !owner.is_empty() && ct_eq(t.trim(), owner))
}

/// `GET /cli/agent-whois?pid=N` (owner only): which agent owns `pid`.
pub fn handle_whois(params: &std::collections::HashMap<String, String>, is_owner: bool, is_post: bool) -> crate::cli_response::CliResponse {
    use crate::cli_response::CliResponse;
    let answer = |status: &'static str, v: serde_json::Value| CliResponse { status, content_type: "application/json", body: v.to_string() };
    if is_post {
        return answer("405 Method Not Allowed", serde_json::json!({ "error": "GET only (a read)", "code": "method_not_allowed" }));
    }
    if !is_owner {
        return answer("403 Forbidden", serde_json::json!({ "error": "only the server's owner asks which agent owns a process", "code": "owner_only" }));
    }
    let Some(pid) = params.get("pid").and_then(|p| p.trim().parse::<u32>().ok()).filter(|p| *p > 0) else {
        return answer("400 Bad Request", serde_json::json!({ "error": "give pid=<process id>", "code": "bad_request" }));
    };
    let mut v = agent_cgroup::whois(pid);
    if let Some(ws) = v.get("workspaceId").and_then(|w| w.as_str()).map(str::to_string) {
        let (path, fence) = workspace_and_fence(&ws);
        v["workspace"] = serde_json::json!(path);
        v["fence"] = serde_json::json!(fence);
    }
    answer("200 OK", v)
}

/// What `PeerAgent` a process is, for tests and the doctor.
pub fn describe(p: &PeerAgent) -> String {
    p.describe()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forward_drops_client_proofs_and_adds_this_processes() {
        let head = "POST /cli/x HTTP/1.1\r\nHost: localhost\r\nX-K2-Cell-Proof: forged\r\nx-k2-cell-session: other\r\nX-K2-Via-Tcp: 1\r\nConnection: keep-alive\r\nContent-Length: 2";
        let f = forward_head(head, "sid-1");
        assert!(f.starts_with("POST /cli/x HTTP/1.1\r\n"));
        assert!(!f.contains("forged") && !f.contains("other") && !f.to_ascii_lowercase().contains("x-k2-via-tcp") && !f.contains("keep-alive"), "{f}");
        assert!(f.contains("Content-Length: 2\r\n") && f.ends_with("Connection: close\r\n\r\n"), "{f}");
        assert!(is_forward_for(&f, "sid-1"));
        assert!(!is_forward_for(&f, "sid-2"), "the proof names one session");
        assert!(!is_forward_for(head, "sid-1"), "a client's own proof is never this process's");
        let forged = "GET / HTTP/1.1\r\nX-K2-Cell-Proof: 00000000\r\nX-K2-Cell-Session: sid-1\r\n";
        assert!(!is_forward_for(forged, "sid-1"));
    }

    #[test]
    fn whois_is_owner_only_and_get_only() {
        let mut p = std::collections::HashMap::new();
        p.insert("pid".to_string(), "1".to_string());
        assert_eq!(handle_whois(&p, false, false).status, "403 Forbidden");
        assert_eq!(handle_whois(&p, true, true).status, "405 Method Not Allowed");
        p.insert("pid".to_string(), "x".to_string());
        assert_eq!(handle_whois(&p, true, false).status, "400 Bad Request");
        assert!(!crate::session_token::is_agent_verb(WHOIS_ROUTE), "an agent never asks whois");
    }
}
