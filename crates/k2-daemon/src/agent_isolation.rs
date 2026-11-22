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
//!   cgroup, then hands the request to the TCP dispatcher, its head rebuilt
//!   around the one passport it validated, from a source port it recorded
//!   for that session ([`is_forward_from`]: no secret in any header). The
//!   same route code and guards as TCP, for every route.
//! - **whois** ([`handle_whois`], `GET /cli/agent-whois?pid=N`, owner
//!   only): which agent owns a process.
//! - **The fence switch** (`POST /cli/agent-access/set {toggle: "fence"}`,
//!   owner; an Admin may only turn it on): per agent, off by default.
//!
//! The claim is "labelled by agent; a fenced agent can't change its label"
//! ([`k2_core::agent_cgroup::CLAIM`]); never "cannot be forged" while the
//! cgroups belong to the K2 user (plan M1b: root-owned cgroups).

use std::path::{Path, PathBuf};

use k2_core::agent_cgroup::{self, Decision, PeerAgent, SessionIsolation};
use k2_core::log_debug;
use k2_core::session::SessionId;

/// The owner-only whois route.
pub const WHOIS_ROUTE: &str = "/cli/agent-whois";
/// The CLI's mark on a request it used to send over loopback TCP.
pub const VIA_TCP_HEADER: &str = "x-k2-via-tcp";

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
    if iso.identity && minted && !crate::session_token::bind_agent_cgroup(sid, &iso.ws, iso.root_placed) {
        log_debug!("[agents] session={sid}: no passport record to bind to agent {}", iso.ws);
    }
    Some(iso)
}

/// Review F1: a `k2 job` run of workspace `ws` (selector `run_sid`) gets
/// what the agent's terminals get: its processes start through the exec
/// helper (the agent's cgroup; the fence when on), and when the box has the
/// identity its passport is bound to that cgroup and it gets a session
/// socket of its own (`K2_HOOK_SOCK` into `env`), since a bound passport
/// never works over TCP. `Ok(None)`: as before (no identity, no fence).
/// `Err`: the run doesn't start (never a job outside what was promised).
#[cfg(unix)]
pub fn for_job(run_sid: &str, ws: &str, root: &Path, env: &mut std::collections::HashMap<String, String>) -> Result<Option<SessionIsolation>, String> {
    let ws = ws.trim();
    let (path, fence) = workspace_and_fence(ws);
    let workspace = path.unwrap_or_else(|| root.to_path_buf());
    let Some(iso) = agent_cgroup::intent(ws, true, fence, Some(&workspace)) else { return Ok(None) };
    if iso.identity {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::cell_uds::cells_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("the job's session socket folder: {e}"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("the job's session socket folder: {e}"))?;
        let sock = job_socket(run_sid);
        let _ = std::fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock).map_err(|e| format!("the job's session socket: {e}"))?;
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("the job's session socket: {e}"))?;
        let rt = crate::rt::handle().ok_or("no runtime for the job's session socket")?;
        let _g = rt.enter();
        crate::cell_server::serve_cell_at(run_sid.to_string(), sock.clone(), listener, None);
        env.insert("K2_HOOK_SOCK".into(), sock.to_string_lossy().into_owned());
        if !crate::session_token::bind_agent_cgroup_sid(run_sid, ws, iso.root_placed) {
            return Err("the job's passport is gone".into());
        }
    }
    Ok(Some(iso))
}

/// Review F10: bind a re-minted passport (adoption after the store was
/// lost) when the session's process is in its own agent's cgroup now.
pub fn bind_from_live_cgroup(sid: &SessionId, pid: u32, ws: Option<&str>) {
    let (Some(ws), Ok(a)) = (ws.map(str::trim).filter(|w| !w.is_empty()), agent_cgroup::availability()) else { return };
    match agent_cgroup::cgroup_of_pid(pid).map(|p| agent_cgroup::with_owner(agent_cgroup::classify(&a.agents_rel, &p), pid)) {
        Ok(peer) if peer.ws() == Some(ws) => {
            // M1b: a root-owned cgroup means the root helper placed it.
            let root_owned = matches!(peer, PeerAgent::Agent { root_owned: true, .. });
            if crate::session_token::bind_agent_cgroup(sid, ws, root_owned) {
                log_debug!("[agents] session={sid}: the re-minted passport is bound to agent {ws} (its live cgroup)");
            }
        }
        Ok(peer) => log_debug!("[agents] session={sid}: re-minted passport left unbound: the session is in {}", peer.describe()),
        Err(e) => log_debug!("[agents] session={sid}: re-minted passport left unbound: {e}"),
    }
}

/// Review F1: a workspace's published `--cmd` service runs in its agent's
/// cgroup and under its fence (no passport: services don't call the daemon).
pub fn for_service(ws: &str, cwd: &Path) -> Option<SessionIsolation> {
    let (path, fence) = workspace_and_fence(ws.trim());
    let workspace = path.unwrap_or_else(|| cwd.to_path_buf());
    agent_cgroup::intent(ws, true, fence, Some(&workspace))
}

/// M1b, at boot: the root helper places agent sessions in root-owned
/// cgroups when it serves `agents place` (protocol 18 and
/// `namespaces.agentcg`). Otherwise the reason, shown in `/boot-status`:
/// the daemon places them itself (traceable only, identity unavailable).
pub fn register_root_placement() {
    if agent_cgroup::switch_off() {
        agent_cgroup::set_no_placer(format!("the emergency off-switch ({}=0)", agent_cgroup::ENV_SWITCH));
        return;
    }
    if let Err(why) = agent_cgroup::availability() {
        agent_cgroup::set_no_placer(why);
        return;
    }
    let st = crate::root_helper::state();
    if !agentcg_ok(&st) {
        let why = crate::root_helper::helper_needed(
            "placing agent sessions in root-owned cgroups (agent identity)",
            k2_root_proto::version::AGENTCG_MIN_PROTOCOL,
            &st,
        );
        log_debug!("[agents] identity unavailable: {why}");
        agent_cgroup::set_no_placer(why);
        return;
    }
    // The helper must take this daemon for the box's registered one (its
    // unit's main process, a system unit, the agents' leaf delegated).
    if let Err(e) = crate::root_helper::call(&["agents".to_string(), "check".to_string()], None) {
        let why = format!("the root helper won't place this daemon's sessions: {e}");
        log_debug!("[agents] identity unavailable: {why}");
        agent_cgroup::set_no_placer(why);
        return;
    }
    agent_cgroup::set_placer(Box::new(|ws: &str, pid: u32, start: u64| {
        let argv = vec!["agents".to_string(), "place".to_string(), ws.to_string(), pid.to_string(), start.to_string()];
        let out = crate::root_helper::call(&argv, None)?;
        let v: serde_json::Value = serde_json::from_slice(&out).map_err(|e| format!("agents place: not JSON: {e}"))?;
        v["cgroup"].as_str().map(str::to_string).ok_or_else(|| format!("agents place: no cgroup in {v}"))
    }));
    log_debug!("[agents] identity on: the root helper places agent sessions in root-owned cgroups");
}

/// Pure: protocol >= 18 and `namespaces.agentcg == true`.
pub fn agentcg_ok(st: &crate::root_helper::RootHelperState) -> bool {
    let ns = st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/agentcg")).and_then(serde_json::Value::as_bool) == Some(true);
    st.usable(k2_root_proto::version::AGENTCG_MIN_PROTOCOL) && ns
}

/// A job run's session socket.
#[cfg(unix)]
pub fn job_socket(run_sid: &str) -> PathBuf {
    crate::cell_uds::cells_dir().join(format!("{run_sid}.sock"))
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

/// A bound passport of another session presented on this session's
/// socket (a copied passport): an audit row naming both. Nothing for an
/// unbound or invalid credential.
pub fn audit_wrong_socket(presented: &str, this_sid: &str) {
    if let Some((sid, ws)) = crate::session_token::bound_agent_of_bearer(presented).filter(|(sid, _)| sid != this_sid) {
        audit_refusal("agent.passport_wrong_socket", &ws, &sid, &format!("the passport of agent {ws} was presented on another session's socket ({this_sid})"), "cell-socket");
    }
}

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
    let a = match agent_cgroup::availability() {
        Ok(a) => a,
        Err(why) => {
            // Identity unavailable now (the off-switch, or the box lost its
            // delegated unit): passports as before. Without the switch that
            // is a box that lost what bound this passport: said loudly, once
            // per boot, with an audit row (review F12).
            if !agent_cgroup::switch_off() {
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| {
                    audit_refusal("agent.identity_lost", bound, &v.session_id, &format!("accepted without the cgroup check: {why}"), "cell-socket");
                    eprintln!("k2-daemon: WARNING: agent cgroup identity is unavailable ({why}) while bound passports exist: they are accepted without the check");
                });
            }
            return None;
        }
    };
    #[cfg(target_os = "linux")]
    let (peer, pid, _pinned) = agent_cgroup::peer_of_socket(fd, &a);
    #[cfg(not(target_os = "linux"))]
    let (peer, pid) = {
        let _ = (fd, &a);
        (PeerAgent::Unknown { why: "no cgroups here".into() }, None::<u32>)
    };
    match agent_cgroup::decide(Some(agent_cgroup::Bound { ws: bound, root_placed: v.agent_root_placed }), &peer) {
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

fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

// ── the socket's forward to TCP (review F3, F5) ───────────────────────
//
// No secret travels in a header. The forwarder records the loopback source
// port of its own connection for the session before it writes a byte; the
// TCP gate accepts a bound passport only on a connection from a recorded
// port for that same session ([`is_forward_from`]), and the record goes when
// the forward ends. A route that logs or proxies the request learns nothing
// reusable. The forwarded head is rebuilt, never passed through: strict
// CRLF, the client's credentials (Authorization, Cookie, `token=` in the
// query or a form body) dropped, and the one passport the socket validated
// put back.

// Keyed by the forwarder's full local address (127.0.0.1 and the port),
// compared exactly: any local process may bind 127.0.0.2 with the same port
// (review R1), none can bind 127.0.0.1:<port> while the forward holds it
// (no SO_REUSEADDR or SO_REUSEPORT on it).
/// The forwarder's own source address: the daemon's loopback listener
/// sees every forward come from it.
const FORWARD_FROM: [u8; 4] = [127, 0, 0, 1];

static FORWARDS: std::sync::Mutex<Option<std::collections::HashMap<std::net::SocketAddr, String>>> = std::sync::Mutex::new(None);

struct ForwardRecord(std::net::SocketAddr);

impl ForwardRecord {
    fn new(local: std::net::SocketAddr, sid: &str) -> Self {
        FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(local, sid.to_string());
        ForwardRecord(local)
    }
}

impl Drop for ForwardRecord {
    fn drop(&mut self) {
        if let Some(m) = FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            m.remove(&self.0);
        }
    }
}

/// `true` when `peer` is one of this daemon's own forwards for `sid`: a
/// loopback connection from the source port the forwarder recorded.
pub fn is_forward_from(peer: Option<std::net::SocketAddr>, sid: &str) -> bool {
    let Some(p) = peer.filter(|p| p.ip() == std::net::IpAddr::from(FORWARD_FROM)) else { return false };
    FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&p)).is_some_and(|s| s == sid)
}

/// A query or form key as the parsers read it (`%XX`, `+`).
fn decoded_key(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(()) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a=1&token=x&b=2` → `a=1&b=2`: every pair whose key reads `token`
/// (raw or decoded) dropped.
pub fn strip_token_pairs(qs: &str) -> String {
    qs.split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let k = p.split('=').next().unwrap_or("");
            k != "token" && decoded_key(k).trim() != "token"
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The request as the socket forwards it to TCP: `head` (up to, not
/// including, the blank line) and `body`, rebuilt around `passport`.
/// Refuses (400) a bare CR or LF, a folded or nameless header line, or a
/// request line that isn't `METHOD target HTTP/1.x`. The third value is
/// `true` for a WebSocket upgrade (a GET with `Upgrade: websocket`,
/// `Connection: upgrade` and no body): it is forwarded as an upgrade
/// (review R6), the frames then copied both ways.
pub fn rebuild_forward(head: &str, body: &[u8], passport: &str) -> Result<(String, Vec<u8>, bool), &'static str> {
    if head.replace("\r\n", "").contains(['\r', '\n']) {
        return Err("a bare CR or LF in the request head");
    }
    let mut lines = head.split("\r\n");
    let req = lines.next().ok_or("no request line")?;
    let parts: Vec<&str> = req.split(' ').collect();
    let [method, target, version] = parts[..] else { return Err("a malformed request line") };
    if !version.starts_with("HTTP/1.") || method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) || !target.starts_with('/') {
        return Err("a malformed request line");
    }
    let target = match target.split_once('?') {
        Some((p, q)) => {
            let q = strip_token_pairs(q);
            if q.is_empty() {
                p.to_string()
            } else {
                format!("{p}?{q}")
            }
        }
        None => target.to_string(),
    };
    let mut out = format!("{method} {target} {version}\r\n");
    let (mut upgrade_ws, mut connection_upgrade) = (false, false);
    for l in lines {
        if l.is_empty() || l.starts_with([' ', '\t']) {
            return Err("a folded or empty header line");
        }
        let (name, value) = l.split_once(':').ok_or("a header line without a name")?;
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "upgrade" => upgrade_ws = value.trim().eq_ignore_ascii_case("websocket"),
            "connection" => connection_upgrade = value.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")),
            _ => {}
        }
        if name.is_empty() || name.contains(|c: char| c.is_ascii_whitespace()) {
            return Err("a malformed header name");
        }
        if matches!(
            name.as_str(),
            "authorization" | "cookie" | "connection" | "proxy-connection" | "keep-alive" | "content-length" | "transfer-encoding" | "te" | "upgrade"
        ) || name == VIA_TCP_HEADER
            || name.starts_with("x-k2-cell-")
        {
            continue;
        }
        out.push_str(l);
        out.push_str("\r\n");
    }
    let form = !body.is_empty() && {
        let t = String::from_utf8_lossy(body);
        let t = t.trim_start();
        !t.starts_with('{') && !t.starts_with('[')
    };
    let body = if form {
        let s = std::str::from_utf8(body).map_err(|_| "a form body that isn't UTF-8")?;
        strip_token_pairs(s.trim_end_matches(['\r', '\n'])).into_bytes()
    } else {
        body.to_vec()
    };
    if upgrade_ws && connection_upgrade && method == "GET" && body.is_empty() {
        out.push_str(&format!("Authorization: Bearer {passport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"));
        return Ok((out, body, true));
    }
    out.push_str(&format!("Authorization: Bearer {passport}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    Ok((out, body, false))
}

/// Hand a request the CLI marked `X-K2-Via-Tcp` to the TCP dispatcher on
/// loopback, after the socket's checks (`passport` is the one the socket
/// validated for `sid`), and copy its answer back.
#[cfg(unix)]
pub async fn forward_to_tcp(stream: &mut tokio::net::UnixStream, head: &str, body: &[u8], sid: &str, passport: &str) {
    use tokio::io::AsyncWriteExt;
    let answer = |status: &str, msg: String| {
        let b = serde_json::json!({ "error": msg }).to_string();
        format!(
            "HTTP/1.1 {status}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            k2_core::frame_policy::CONNECT_HEADER_LINES,
            b.len()
        )
    };
    let (fwd, body, upgrade) = match rebuild_forward(head, body, passport) {
        Ok(x) => x,
        Err(why) => {
            let _ = stream.write_all(answer("400 Bad Request", format!("the session socket refused the request: {why}")).as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        }
    };
    let port = k2_core::hook_config::get_port();
    let sock = match tokio::net::TcpSocket::new_v4() {
        Ok(s) => s,
        Err(e) => {
            let _ = stream.write_all(answer("502 Bad Gateway", format!("the session socket couldn't open a socket: {e}")).as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        }
    };
    // Bind first, so the source port is known (and recorded) before the
    // daemon can read a byte from this connection.
    let local = sock
        .set_reuseaddr(false)
        .and_then(|_| sock.set_reuseport(false))
        .and_then(|_| sock.bind(std::net::SocketAddr::from((FORWARD_FROM, 0))))
        .and_then(|_| sock.local_addr());
    let record = match local {
        Ok(a) if a.ip() == std::net::IpAddr::from(FORWARD_FROM) => ForwardRecord::new(a, sid),
        Ok(a) => {
            let _ = stream.write_all(answer("502 Bad Gateway", format!("the session socket bound {a}, not {}", std::net::Ipv4Addr::from(FORWARD_FROM))).as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        }
        Err(e) => {
            let _ = stream.write_all(answer("502 Bad Gateway", format!("the session socket couldn't bind: {e}")).as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        }
    };
    let mut tcp = match sock.connect(std::net::SocketAddr::from(([127, 0, 0, 1], port))).await {
        Ok(t) => t,
        Err(e) => {
            drop(record);
            let _ = stream.write_all(answer("502 Bad Gateway", format!("the session socket couldn't reach the daemon's own port: {e}")).as_bytes()).await;
            let _ = stream.shutdown().await;
            return;
        }
    };
    if tcp.write_all(fwd.as_bytes()).await.is_err() || tcp.write_all(&body).await.is_err() {
        // The record goes before the TCP socket closes (review R1).
        drop(record);
        drop(tcp);
        let _ = stream.shutdown().await;
        return;
    }
    if upgrade {
        // A WebSocket (review R6): only after the daemon's 101 do frames
        // flow both ways (until either side closes; the record lives as
        // long). Any other answer is an ordinary one: no second request
        // goes up this connection unrebuilt.
        use tokio::io::AsyncReadExt;
        let mut head = Vec::new();
        let mut b = [0u8; 4096];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
            match tcp.read(&mut b).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&b[..n]),
            }
        }
        // The status line's code is 101 (`HTTP/1.x 101 …`).
        let switched = head.starts_with(b"HTTP/1.") && head.get(8..13) == Some(&b" 101 "[..]);
        if stream.write_all(&head).await.is_err() {
            drop(record);
            drop(tcp);
            return;
        }
        if switched {
            let _ = tokio::io::copy_bidirectional(stream, &mut tcp).await;
        } else {
            let _ = tcp.shutdown().await;
            let _ = tokio::io::copy(&mut tcp, stream).await;
        }
    } else {
        // Answers stream (watch / tail -f): copy until the daemon closes.
        let _ = tokio::io::copy(&mut tcp, stream).await;
    }
    drop(record);
    drop(tcp);
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
/// forward for that session ([`is_forward_from`]). It reads the credential
/// the routes will use: the dispatcher's own fold (Bearer, else `?token=`,
/// else the session cookie; review F4), and also a Bearer or query or
/// cookie passport that the fold would pass over (any bound one refuses).
/// Inert when identity is unavailable (the emergency off-switch included).
pub fn tcp_refusal(query: &str, head: &str, peer: Option<std::net::SocketAddr>, ingress: &str) -> Option<String> {
    agent_cgroup::availability().ok()?;
    let (folded, _) = crate::routes::http::effective_auth_query(query, head);
    let mut seen: Vec<String> = Vec::new();
    seen.extend(crate::routes::http::extract_token(&folded).map(str::to_string));
    seen.extend(crate::routes::http::extract_token(query).map(str::to_string));
    seen.extend(crate::routes::http::extract_bearer(head).map(str::to_string));
    seen.extend(crate::routes::http::extract_session_cookie(head).map(str::to_string));
    seen.extend(crate::routes::http::extract_skin_session_cookie(head).map(str::to_string));
    for t in seen.iter().filter(|t| t.contains('.')) {
        let Some((sid, ws)) = crate::session_token::bound_agent_of_bearer(t) else { continue };
        if is_forward_from(peer, &sid) {
            continue;
        }
        audit_refusal("agent.passport_needs_socket", &ws, &sid, "a bound passport was presented over TCP", ingress);
        return Some(needs_socket_body());
    }
    None
}

/// A form body's params for the routes that still take a body `token`
/// (`/cli/respond`, `/cli/session/complete`), minus a bound passport there
/// (refused with an audit row: a body never carries one past the TCP gate;
/// review F4).
pub fn form_body_without_bound_passport(body: &[u8]) -> std::collections::HashMap<String, String> {
    let mut m = crate::routes::http::parse_form_body(body);
    if m.get("token").is_some_and(|t| audit_body_passport(t, "a TCP form body")) {
        m.remove("token");
    }
    m
}

/// `true` (with an audit row) when `t`, found in a request body, is a
/// passport bound to its agent's cgroup while identity is on.
pub fn audit_body_passport(t: &str, place: &str) -> bool {
    if agent_cgroup::availability().is_err() {
        return false;
    }
    let Some((sid, ws)) = crate::session_token::bound_agent_of_bearer(t) else { return false };
    audit_refusal("agent.passport_needs_socket", &ws, &sid, &format!("a bound passport was presented in {place}"), "tcp");
    true
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

    /// Review F3: the forward carries exactly the passport the socket
    /// validated. The client's own Authorization, Cookie and `token=` (query,
    /// form body, percent-encoded) go; a bare LF (smuggled header) refuses.
    #[test]
    fn the_forward_carries_only_the_validated_passport() {
        let head = "POST /cli/x?token=B.secret&a=1&%74oken=B2.x HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer B.secret\r\nCookie: k2_session=B.secret\r\nX-K2-Cell-Session: other\r\nX-K2-Via-Tcp: 1\r\nConnection: keep-alive\r\nContent-Length: 30\r\nContent-Type: application/x-www-form-urlencoded";
        let (f, body, _) = rebuild_forward(head, b"token=B.secret&message=hi", "A.mine").unwrap();
        assert!(f.starts_with("POST /cli/x?a=1 HTTP/1.1\r\n"), "{f}");
        for gone in ["B.secret", "B2.x", "other", "keep-alive", "Cookie", "X-K2-Via-Tcp"] {
            assert!(!f.contains(gone), "{gone} in {f}");
        }
        assert_eq!(f.matches("Authorization:").count(), 1, "{f}");
        assert!(f.contains("Authorization: Bearer A.mine\r\n") && f.ends_with("Connection: close\r\n\r\n"), "{f}");
        assert_eq!(body, b"message=hi".to_vec());
        assert!(f.contains(&format!("Content-Length: {}\r\n", body.len())), "{f}");
        // The smuggling shape: a bare LF inside a header value.
        let smuggled = "POST /cli/x HTTP/1.1\r\nFoo: x\nX-K2-Cell-Session: sid-B\r\nHost: k2";
        assert!(rebuild_forward(smuggled, b"", "A.mine").is_err());
        assert!(rebuild_forward("GET /x HTTP/1.1\r\nFoo: x\rBar: y", b"", "A.mine").is_err(), "a bare CR");
        assert!(rebuild_forward("GET /x HTTP/1.1\r\n folded", b"", "A.mine").is_err());
        assert!(rebuild_forward("GET x HTTP/1.1", b"", "A.mine").is_err());
        // A JSON body is left as it is (a JSON `token` never reaches a
        // route's params: routes::http::merge_json_object_params, review R3).
        let (f, j, up) = rebuild_forward("POST /x HTTP/1.1", br#"{"token":"x"}"#, "A.mine").unwrap();
        assert_eq!(j, br#"{"token":"x"}"#.to_vec());
        assert!(!up && !f.contains("Upgrade"), "{f}");
        // Review R6: a WebSocket upgrade is forwarded as one, still with
        // only the validated passport.
        let ws = "GET /cli/app/shell?app=x&token=B.secret HTTP/1.1\r\nHost: k2\r\nUpgrade: WebSocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Key: abc==\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer B.secret\r\nX-K2-Via-Tcp: 1";
        let (f, body, up) = rebuild_forward(ws, b"", "A.mine").unwrap();
        assert!(up && body.is_empty(), "{f}");
        assert!(f.starts_with("GET /cli/app/shell?app=x HTTP/1.1\r\n") && !f.contains("B.secret") && !f.contains("keep-alive"), "{f}");
        assert!(f.contains("Sec-WebSocket-Key: abc==\r\n") && f.ends_with("Authorization: Bearer A.mine\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"), "{f}");
        assert!(!f.contains("Content-Length") && !f.contains("Connection: close"), "{f}");
        // Not an upgrade: a POST, a body, or only one of the two headers.
        assert!(!rebuild_forward("POST /x HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade", b"", "A.mine").unwrap().2);
        assert!(!rebuild_forward("GET /x HTTP/1.1\r\nUpgrade: websocket", b"", "A.mine").unwrap().2);
        assert!(!rebuild_forward("GET /x HTTP/1.1\r\nUpgrade: h2c\r\nConnection: Upgrade", b"", "A.mine").unwrap().2);
        assert_eq!(strip_token_pairs("token=a&x=1&%74%6Fken=b&tok%65n=c&token2=d"), "x=1&token2=d");
    }

    /// Review F5: the forward's proof is its own recorded source port, for
    /// one session, only while the forward lives.
    #[test]
    fn a_forward_is_recognised_by_its_full_recorded_address_only() {
        let peer = |p: u16| Some(std::net::SocketAddr::from(([127, 0, 0, 1], p)));
        assert!(!is_forward_from(peer(40001), "sid-1"));
        {
            let _r = ForwardRecord::new(std::net::SocketAddr::from(([127, 0, 0, 1], 40001)), "sid-1");
            // Review R1: the same port on another loopback address.
            assert!(!is_forward_from(Some(std::net::SocketAddr::from(([127, 0, 0, 2], 40001))), "sid-1"), "127.0.0.2, same port");
            assert!(!is_forward_from(Some("[::1]:40001".parse().unwrap()), "sid-1"), "::1, same port");
            assert!(is_forward_from(peer(40001), "sid-1"));
            assert!(!is_forward_from(peer(40001), "sid-2"), "the record names one session");
            assert!(!is_forward_from(peer(40002), "sid-1"), "another connection");
            assert!(!is_forward_from(Some(std::net::SocketAddr::from(([10, 0, 0, 5], 40001))), "sid-1"), "not loopback");
            assert!(!is_forward_from(None, "sid-1"));
        }
        assert!(!is_forward_from(peer(40001), "sid-1"), "the record goes with the forward");
    }

    /// Review F1: every place the daemon starts an agent-supplied command
    /// for a workspace goes through the isolation (the exec helper, the
    /// agent's cgroup, its fence). A new spawn site that runs a workspace's
    /// command must be added here, or this fails.
    #[test]
    fn every_agent_command_spawn_site_goes_through_the_isolation() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let read = |f: &str| std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        // Terminals: both spawn paths decide the isolation with the passport.
        for f in ["v2_spawn.rs", "spawn.rs"] {
            assert!(read(f).contains("agent_isolation::for_spawn("), "{f} spawns terminals without agent_isolation::for_spawn");
        }
        // `k2 job` runs and published `--cmd` services wrap their command.
        let jobs = read("jobs/runner.rs");
        assert!(jobs.contains("agent_isolation::for_job(") && jobs.contains("agent_cgroup::wrap("), "k2 job runs skip the isolation");
        let publish = read("publish_runtime.rs");
        assert!(publish.contains("agent_isolation::for_service(") && publish.contains("agent_cgroup::wrap("), "published services skip the isolation");
        // Terminals a passport can't open or drive at all.
        for p in ["/cli/terminal/create", "/cli/terminal/write", "/cli/terminal/kill", "/cli/sessions/v2/spawn"] {
            assert!(!crate::session_token::is_agent_verb(p), "{p} is an agent verb: a passport could start or drive a terminal outside the isolation");
        }
        // Review R4: every file of this crate that starts a process (outside
        // its `mod tests`) is named here, wrapped or not an agent's command,
        // with the reason. A new spawn site fails until someone decides.
        let named: std::collections::BTreeMap<&str, &str> = SPAWN_SITES.iter().copied().collect();
        let mut found = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if p.is_dir() {
                    if name != "tests" {
                        stack.push(p);
                    }
                    continue;
                }
                if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "tests.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                let prod = text.find("\n#[cfg(test)]\nmod tests {").map_or(text.as_str(), |i| &text[..i]);
                if ["bg_proc::command(", "bg_proc::command_keeping_session(", "bg_proc::tokio_command(", "Command::new("].iter().any(|n| prod.contains(n)) {
                    found.push(p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        found.sort();
        let unnamed: Vec<&String> = found.iter().filter(|f| !named.contains_key(f.as_str())).collect();
        assert!(unnamed.is_empty(), "spawn sites not yet decided (wrapped, or not an agent's command, in SPAWN_SITES): {unnamed:?}");
        let stale: Vec<&&str> = named.keys().filter(|f| !found.iter().any(|x| x == *f)).collect();
        assert!(stale.is_empty(), "SPAWN_SITES names files that no longer start a process: {stale:?}");
        for (f, why) in SPAWN_SITES {
            if why.starts_with("WRAPPED") {
                assert!(read(f).contains("agent_cgroup::wrap("), "{f} is named wrapped but never calls agent_cgroup::wrap");
            }
        }
    }

    /// Every file of this crate that starts a process, and why it is or
    /// isn't the isolation's business (review R4). `WRAPPED`: an agent's
    /// command, through the exec helper into its cgroup and fence.
    const SPAWN_SITES: &[(&str, &str)] = &[
        ("jobs/runner.rs", "WRAPPED: k2 job runs (for_job, own socket, bound passport)"),
        ("publish_runtime.rs", "WRAPPED: published --cmd services (for_service)"),
        ("vault/run.rs", "an agent's argv, run by `k2-daemon --vault-run`, which the CLI starts inside the caller's own session: same cgroup, same fence"),
        ("apps/process_engine.rs", "an App's Seed start argv, not a session's: the process engine is the owner's opt-in exception (SR1, §16 C9); App isolation is the App engine's (App exec passports are unbound, M2)"),
        ("apps/guarded.rs", "skopeo for the guarded container engine (K2's own program)"),
        ("apps/admission.rs", "sysctl reads for App admission (K2's own program)"),
        ("apps/box_prep.rs", "box preparation by the owner: systemctl and K2's own tools"),
        ("apps/db_connect_real_pg.rs", "an #[ignore] real-Postgres harness (node, psql), not daemon code"),
        ("apps/podman_install.rs", "package queries for the owner's Podman install (dpkg-query, rpm, pacman)"),
        ("apps/process_exception.rs", "systemctl for the owner's process-engine exception"),
        ("v1_sandboxes/policy.rs", "cp of sandbox policy files (K2's own program); sandbox commands run in microVM cells (unbound, M2)"),
        ("compute_local.rs", "the owner's 'make this computer a compute node' (k2-node install through the root helper / sudo); compute jobs run in k2-node's own sandbox"),
        ("compute_views.rs", "tmutil reads for the compute views (K2's own program)"),
        ("compute_ws.rs", "hostname (K2's own program)"),
        ("desktop/virtual_desktop.rs", "K2's own desktop programs (Xvfb, the window manager, systemd-run --user) in the k2-desktops slices; a program an agent shows on a desktop starts from its own terminal"),
        ("desktop/launch_agent.rs", "the macOS desktop helper's launch agent (K2's own program)"),
        ("desktop/deskd_link.rs", "K2's own deskd binary"),
        ("desktop/permission.rs", "`open` for a macOS privacy pane, asked by the person"),
        ("llm_accounts_runtime.rs", "harness sign-in runs (claude / codex login) a person starts from Settings, in the login slot"),
        ("subscription_usage.rs", "K2's own usage probes (codex, grok) and keychain reads"),
        ("claude_auth_host.rs", "K2's macOS sign-in host (crontab, launchctl, security)"),
        ("cell_egress.rs", "nft for cell egress rules (K2's own program)"),
        ("connect_device_routes.rs", "scutil for this computer's name (K2's own program)"),
        ("daemon_main.rs", "the daemon itself (current_exe)"),
        ("daemon_stop.rs", "systemctl and K2's own stop helper"),
        ("front443/mod.rs", "systemctl for the box's :443 front (K2's own program)"),
        ("handoff/mod.rs", "the daemon's own handoff (systemctl, current_exe)"),
        ("handoff_setup.rs", "systemctl for the daemon's unit"),
        ("keep_alive.rs", "the daemon's own keep-alive helper (current_exe)"),
        ("publish_dev.rs", "systemd-run --user scopes for a published service's dev caps; never for an isolated service (publish_runtime skips them), and lsof"),
        ("mail/helper.rs", "the mail system user and units, through sudo (K2's own programs)"),
        ("mail/imap_listeners.rs", "the mail engine's listeners, through sudo"),
        ("mail/preflight.rs", "getent and ss reads before mail setup"),
        ("mail/routes_link_oauth.rs", "the browser opener for a person's mail sign-in"),
        ("mail/sieve_lockdown_live.rs", "a live Sieve lockdown check (bash, python3), owner-run"),
        ("mail/sysops.rs", "the mail engine's fixed system commands"),
        ("permission/actions/postal.rs", "`p5 k2 link`, the postal client, after a person approved the card"),
        ("pg_stale.rs", "K2's own Postgres housekeeping commands"),
        ("power/helper.rs", "pmset through K2's power helper"),
        ("power/linux.rs", "systemd-inhibit (keep awake)"),
        ("power/macos.rs", "osascript and pmset (keep awake)"),
        ("root_helper/mod.rs", "sudo for the signed root helper"),
        ("root_helper_routes.rs", "the signed root helper's installer"),
        ("sew/box_backup.rs", "systemctl for Sew's backup unit"),
        ("sew/offbox.rs", "systemctl for Sew's off-box unit"),
        ("sew/ops.rs", "systemctl for Sew's units"),
        ("skin_hydra.rs", "kill for the App sign-in server K2 started"),
        ("sql/pgmove/dual_k2.rs", "psql for a Postgres move (owner-run)"),
        ("sql/pgmove/helper_ops.rs", "psql for a Postgres move (owner-run)"),
        ("sql/pgmove/real_move.rs", "psql for a Postgres move (owner-run)"),
        ("sql/sysops.rs", "Postgres system commands, through sudo"),
        ("update_routes.rs", "the owner's update helper and hostname"),
        ("vault/keeper_client.rs", "sudo for the vault keeper (K2's own helper)"),
        ("vault/server_refs.rs", "ps reads for the vault's process checks"),
        ("vault/ssh_agent.rs", "the vault's ssh-agent is a socket in the daemon; it starts no program outside its test helpers"),
    ];

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
