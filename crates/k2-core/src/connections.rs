//! Workspace connections — thin dispatch over `WorkspaceRelation` +
//! `log_activity`.
//!
//! Powers `k2 connections list/add/remove`. A connection is a row
//! in `workspace_relations` linking two projects (`source_project_id`
//! → `target_project_id`) so cross-workspace `k2so msg` can verify
//! the sender is actually allowed to post.
//!
//! Moved to core so the daemon can serve `/cli/connections`
//! headlessly. Same three verbs src-tauri had: `list` / `add` /
//! `remove`.
//!
//! 0.39.0 directive (workspace==agent model): the stored data is
//! intentionally directional (`source → target`) for audit/lineage
//! value, but **user-facing surfaces present connections as
//! bidirectional**. Whoever initiated the relationship, BOTH
//! workspaces see each other in their roster as just "connected" —
//! no direction labels. That's what [`list_peers`] implements; the
//! `list` action of [`connections`] now returns the deduped peer
//! shape so the CLI / SKILL.md / roster never have to think about
//! direction.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::db::schema::{log_activity, WorkspaceRelation, WorkspaceRemoteConnection};
use crate::workspace::agent_identity::resolve_project_id;

/// A connected peer workspace, deduped across the directional
/// `workspace_relations` rows. If the same peer appears as both an
/// outgoing target (this workspace → them) and an incoming source
/// (them → this workspace), they show up once here with
/// `relation_types` carrying both labels (sorted, deduped).
///
/// Returned by [`list_peers`] and the user-facing `connections list`
/// dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub project_id: String,
    pub project_name: String,
    /// 0.39.45 (#31): the peer's filesystem path — `""` when the
    /// project row vanished (orphaned relation).
    #[serde(default)]
    pub path: String,
    /// 0.39.45 (#31): whether this peer is actually addressable — its
    /// project row exists AND its path exists on disk. `list` used to
    /// render orphaned/moved peers indistinguishably from live ones,
    /// sending operators down "the wiring is fine" rabbit holes when
    /// `k2so msg` then bounced.
    #[serde(default)]
    pub reachable: bool,
    /// Every distinct relation_type value seen across the
    /// outgoing+incoming rows that point at this peer. A peer might
    /// be "collaborator" in one direction and "oversees" in the
    /// other; both labels are listed (sorted alphabetically, deduped).
    pub relation_types: Vec<String>,
    /// GAP #3: `false` for LOCAL (same-daemon) peers — every `Peer`
    /// built by [`list_peers`]. The `list` dispatch sets this default
    /// (`#[serde(default)]` → false) so local and remote entries share
    /// one uniform shape in the `connections` array; remote entries
    /// (`{"remote": true, ...}`) are appended separately by `list`.
    #[serde(default)]
    pub remote: bool,
}

/// Best-effort "did you mean" over registered project names (0.39.45,
/// #33). Ranks case-insensitive containment first, then small edit
/// distance (≤ 3). Returns `None` when nothing is plausibly close —
/// a bad suggestion is worse than no suggestion.
pub fn suggest_project_name(conn: &rusqlite::Connection, token: &str) -> Option<String> {
    let token_lc = token.to_lowercase();
    if token_lc.is_empty() {
        return None;
    }
    let mut stmt = conn.prepare("SELECT name FROM projects").ok()?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    let mut best: Option<(usize, String)> = None;
    for name in names {
        let name_lc = name.to_lowercase();
        // Rank by edit distance first (a 1-typo near-miss is the best
        // suggestion); containment either way ranks like distance 2 —
        // useful for partial names ("trading" → "AI Trading Platform")
        // but it must not beat an almost-exact match.
        let d = edit_distance(&token_lc, &name_lc);
        let contained = name_lc.contains(&token_lc) || token_lc.contains(&name_lc);
        let rank = if d <= 3 {
            d.min(if contained { 2 } else { d })
        } else if contained {
            2
        } else {
            continue;
        };
        match &best {
            Some((b, _)) if *b <= rank => {}
            _ => best = Some((rank, name)),
        }
    }
    best.map(|(_, name)| name)
}

/// Classic Levenshtein distance, O(a×b) two-row DP. Inputs are short
/// workspace names; no perf concern.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Resolve a connections target token to a project id: exact name or
/// path first, then case-insensitive name (0.39.45, #33 — operators
/// and LLM agents infer plausible casing and used to bounce on it).
fn resolve_target_project_id(
    conn: &rusqlite::Connection,
    token: &str,
) -> Result<String, String> {
    conn.query_row(
        "SELECT id FROM projects WHERE name = ?1 OR path = ?1 ORDER BY rowid LIMIT 1",
        rusqlite::params![token],
        |row| row.get::<_, String>(0),
    )
    .or_else(|_| {
        conn.query_row(
            "SELECT id FROM projects WHERE name = ?1 COLLATE NOCASE ORDER BY rowid LIMIT 1",
            rusqlite::params![token],
            |row| row.get::<_, String>(0),
        )
    })
    .map_err(|_| match suggest_project_name(conn, token) {
        Some(s) => format!(
            "Workspace '{}' not found — did you mean '{}'? Run `k2 connections list` for registered names, or pass a full path. For a cross-server agent use agent::host.k2.dev (or legacy agent@host.k2.dev).",
            token, s
        ),
        None => format!(
            "Workspace '{}' not found. Run `k2 connections list` for registered names, or pass a full path. For a cross-server agent use agent::host.k2.dev (or legacy agent@host.k2.dev).",
            token
        ),
    })
}

/// Resolve `(name, path)` for a project id; `("Unknown", "")` when the
/// row is gone (orphaned relation).
fn peer_identity(conn: &rusqlite::Connection, pid: &str) -> (String, String) {
    conn.query_row(
        "SELECT name, path FROM projects WHERE id = ?1",
        rusqlite::params![pid],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .unwrap_or_else(|_| ("Unknown".to_string(), String::new()))
}

/// Bidirectional, deduped peer list. Returns each connected
/// workspace exactly once regardless of which side initiated the
/// relationship. Used by user-facing surfaces (roster, manager
/// SKILL.md team section, CLI list) that should not surface
/// direction as a user-visible distinction.
///
/// Output is sorted alphabetically by `project_name` for stable
/// rendering.
pub fn list_peers(project_path: &str) -> Result<Vec<Peer>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    let outgoing = WorkspaceRelation::list_for_source(&conn, &project_id)
        .map_err(|e| e.to_string())?;
    let incoming = WorkspaceRelation::list_for_target(&conn, &project_id)
        .map_err(|e| e.to_string())?;

    let mut peers: HashMap<String, Peer> = HashMap::new();

    for rel in &outgoing {
        let pid = &rel.target_project_id;
        let (name, path) = peer_identity(&conn, pid);
        let reachable = !path.is_empty() && std::path::Path::new(&path).exists();
        peers
            .entry(pid.clone())
            .or_insert_with(|| Peer {
                project_id: pid.clone(),
                project_name: name,
                path,
                reachable,
                relation_types: Vec::new(),
                remote: false,
            })
            .relation_types
            .push(rel.relation_type.clone());
    }

    for rel in &incoming {
        let pid = &rel.source_project_id;
        let (name, path) = peer_identity(&conn, pid);
        let reachable = !path.is_empty() && std::path::Path::new(&path).exists();
        peers
            .entry(pid.clone())
            .or_insert_with(|| Peer {
                project_id: pid.clone(),
                project_name: name,
                path,
                reachable,
                relation_types: Vec::new(),
                remote: false,
            })
            .relation_types
            .push(rel.relation_type.clone());
    }

    // Dedupe + sort relation_types per peer so callers don't see
    // duplicate labels when both directions carry the same type.
    let mut out: Vec<Peer> = peers
        .into_values()
        .map(|mut p| {
            p.relation_types.sort();
            p.relation_types.dedup();
            p
        })
        .collect();
    out.sort_by(|a, b| a.project_name.cmp(&b.project_name));
    Ok(out)
}

/// One connection edge as the 0.40.24 `k2 agent conf` surface renders
/// it. LOCAL edges are ALWAYS mutual: the substrate stores ONE
/// `workspace_relations` row per pair (migration 0051) and a single
/// row IS bidirectional awareness — there is no direction to report
/// (the `--one-way`/direction surface was removed post-build, PRD
/// "Post-build decisions"). Cross-daemon
/// (`workspace_remote_connections`) edges are the exception: the row
/// lives only on THIS daemon, so they carry `bidirectional: false` +
/// `remote: true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfPeer {
    /// Peer workspace display name (`projects.name`), or the remote
    /// `<agent>::<host>` address for cross-daemon edges.
    pub peer: String,
    /// `true` for every local edge (one row per pair = mutual
    /// awareness); `false` only for cross-daemon edges.
    pub bidirectional: bool,
    /// `true` for cross-daemon (`workspace_remote_connections`) edges.
    #[serde(default)]
    pub remote: bool,
}

/// Conf-shaped peer list for `project_path`: LOCAL edges from
/// `workspace_relations` (deduped per peer, always bidirectional) plus
/// REMOTE (`<agent>::<host>`) edges from `workspace_remote_connections`.
/// Sorted by peer name for stable rendering. Errors when the workspace
/// isn't registered.
pub fn list_conf_peers(project_path: &str) -> Result<Vec<ConfPeer>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    let outgoing = WorkspaceRelation::list_for_source(&conn, &project_id)
        .map_err(|e| e.to_string())?;
    let incoming = WorkspaceRelation::list_for_target(&conn, &project_id)
        .map_err(|e| e.to_string())?;

    // Dedupe peer ids across the (historically directional) rows —
    // whichever side holds the row, the pair is mutually connected.
    let mut peer_ids: HashSet<String> = HashSet::new();
    for rel in &outgoing {
        peer_ids.insert(rel.target_project_id.clone());
    }
    for rel in &incoming {
        peer_ids.insert(rel.source_project_id.clone());
    }

    let mut out: Vec<ConfPeer> = peer_ids
        .into_iter()
        .map(|pid| {
            let (name, _path) = peer_identity(&conn, &pid);
            ConfPeer {
                peer: name,
                bidirectional: true,
                remote: false,
            }
        })
        .collect();

    // Cross-daemon edges (GAP #3 rows). Stored only on this daemon —
    // the one genuinely non-mutual shape conf can surface. Display as
    // canonical `agent::host` (normalize-on-read for old `@` rows).
    if let Ok(remotes) = WorkspaceRemoteConnection::list_for_source(&conn, &project_id) {
        for r in remotes {
            out.push(ConfPeer {
                peer: display_remote_addr(&r.agent, &r.host),
                bidirectional: false,
                remote: true,
            });
        }
    }

    out.sort_by(|a, b| a.peer.cmp(&b.peer));
    Ok(out)
}

// ── GAP #3: cross-daemon (remote) connections ───────────────────────
//
// A REMOTE connection addresses an agent on a DIFFERENT daemon:
//   • Canonical user form:  `<agent>::<host>`   (e.g. `ai::rpm.k2.dev`)
//   • Legacy user form:     `<agent>@<host>`    (compat; still accepted)
//   • Wire form (NOT here): `<fp>::<ws_uuid>::<agent>` — federation send body
//
// Remote edges live in `workspace_remote_connections` (not
// `workspace_relations` — the peer has no local `projects` row).
// `is_remote_connection` is the gate `federation::handle_send` consults
// before dialing a peer. Storage is canonical lowercase `agent::host`;
// gates match on agent+host so old `@` rows keep working.

/// Whether the right-hand side of a 2-part `::` address looks like a network
/// host (user form), not a workspace uuid or other opaque token.
///
/// PRD B1: contains `.` and/or ends with `.k2.dev`. Dotted hosts cover the
/// canonical zone (`rpm.k2.dev`), DNS names, and loopback test hosts
/// (`127.0.0.1:1`).
pub fn looks_like_host(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() {
        return false;
    }
    let lower = h.to_ascii_lowercase();
    h.contains('.') || lower.ends_with(".k2.dev")
}

/// Whether `target` is a REMOTE (cross-daemon) **user** address.
///
/// True for:
/// - legacy `<agent>@<host>`
/// - 2-part `<agent>::<host>` where host looks like a host
///
/// False for bare local names, paths, and the 3-part wire form
/// `<fp>::<ws>::<agent>` (that is `k2 fed send` only).
pub fn is_remote_target(target: &str) -> bool {
    let t = target.trim();
    // Legacy user form — always remote (split on last `@` elsewhere).
    if t.contains('@') {
        return true;
    }
    // Canonical user form `agent::host` OR accidental URL-encoded form
    // if a caller forgot to decode (`agent%3A%3Ahost` still contains no
    // raw `::`). Prefer decoded callers; also accept percent-encoded `::`.
    let decoded = if t.contains("%3A%3A") || t.contains("%3a%3a") {
        crate::agent_hooks::urldecode(t)
    } else {
        t.to_string()
    };
    let d = decoded.as_str();
    if d.contains("::") {
        let parts: Vec<&str> = d.split("::").collect();
        // 2-part user form only; 3-part is federation wire (`k2 fed send`).
        return parts.len() == 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && looks_like_host(parts[1]);
    }
    false
}

/// K2 Connect zone suffix (canonical remote hosts are `<subdomain>.k2.dev`).
const K2_ZONE: &str = ".k2.dev";

/// Normalize a remote host for storage/matching.
///
/// - trim + lowercase
/// - collapse accidental double zone (`rpm.k2.dev.k2.dev` → `rpm.k2.dev`)
///
/// Does **not** invent a zone for off-zone hosts (`peer.example.com` stays).
pub fn normalize_remote_host(host: &str) -> String {
    let mut h = host.trim().to_lowercase();
    // Collapse repeated `.k2.dev` suffixes down to a single trailing zone.
    while h.ends_with(".k2.dev.k2.dev") {
        h.truncate(h.len() - K2_ZONE.len());
    }
    h
}

/// Bare routing label for host/subdomain compare: strip one trailing
/// `.k2.dev` (after [`normalize_remote_host`]) so `rpm` matches `rpm.k2.dev`
/// and a doubled zone still matches after collapse. Off-zone hosts keep
/// their full form (`peer.example.com` → itself).
fn bare_routing_label(host_or_sub: &str) -> String {
    let n = normalize_remote_host(host_or_sub);
    n.strip_suffix(K2_ZONE).unwrap_or(&n).to_string()
}

/// Whether `host` (right-hand side of a remote user address) matches a peer's
/// `subdomain` routing hint. Accepts bare subdomain, full `.k2.dev` host,
/// peer pinned with full host, and collapsed double-zone hosts.
fn host_matches_peer_subdomain(host: &str, subdomain: &str) -> bool {
    let host_label = bare_routing_label(host);
    let sub_label = bare_routing_label(subdomain);
    !host_label.is_empty() && !sub_label.is_empty() && host_label == sub_label
}

/// 0.40.21: whether a **Trusted** federation peer is pinned that ROUTES to
/// `host` (the right-hand side of a remote connection address).
///
/// A remote connection row with NO matching trusted peer is a half-state: the
/// connection exists but every cross-daemon send 404s at dial time because no
/// peer resolves/routes for the host (the exact `ai::rpm.k2.dev` failure this
/// surfaces). A peer's routing hint is its `subdomain` → `<subdomain>.k2.dev`,
/// so a host matches a peer when `host == <subdomain>.k2.dev` (the canonical
/// case) or `host == subdomain` (a peer pinned with the full host). Comparison
/// is case-insensitive, mirroring the remote-addr gate. Fails CLOSED — an
/// unreadable peer store yields `false` so the operator is WARNED rather than
/// told (wrongly) that the connection is wired.
pub fn host_has_trusted_peer(host: &str) -> bool {
    trusted_peer_fingerprint_for_host(host).is_some()
}

/// Fingerprint of the **Trusted** peer whose subdomain routes to `host`, if
/// any. Used to bind `workspace_remote_connections.peer_fingerprint` on
/// create / reconcile (GH #13). Fails CLOSED on unreadable peer store.
pub fn trusted_peer_fingerprint_for_host(host: &str) -> Option<String> {
    use crate::federation::{PeerStore, PeerTrust};
    if normalize_remote_host(host).is_empty() {
        return None;
    }
    let store = PeerStore::load().ok()?;
    store.list().iter().find_map(|p| {
        if p.trust != PeerTrust::Trusted {
            return None;
        }
        if host_matches_peer_subdomain(host, &p.subdomain) {
            Some(p.fingerprint.clone())
        } else {
            None
        }
    })
}

/// Parse a remote **user** address into `(agent, host)`.
///
/// Accepts:
/// - `<agent>::<host>` when host looks like a host (canonical)
/// - `<agent>@<host>` (legacy; split on the LAST `@` so an agent token may
///   itself contain `@`)
///
/// Rejects the 3-part wire form and non-addresses. Host is returned as typed
/// (caller should run [`normalize_remote_host`] for storage).
pub fn parse_remote_addr(addr: &str) -> Result<(String, String), String> {
    let addr = addr.trim();
    if addr.contains("::") {
        let parts: Vec<&str> = addr.split("::").collect();
        if parts.len() == 3 {
            return Err(format!(
                "'{addr}' is federation wire form (<fp>::<workspace>::<agent>) — \
                 use <agent>::<host> for connections / k2 msg"
            ));
        }
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            if !looks_like_host(parts[1]) {
                return Err(format!(
                    "remote address '{addr}' right-hand side does not look like a host \
                     (expected <agent>::<host> with a dotted host, e.g. agent::box.k2.dev)"
                ));
            }
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
        return Err(format!(
            "remote address '{addr}' must be <agent>::<host> (or legacy <agent>@<host>)"
        ));
    }
    let at = addr.rfind('@').ok_or_else(|| {
        format!("'{addr}' is not a remote address (expected <agent>::<host> or <agent>@<host>)")
    })?;
    let agent = &addr[..at];
    let host = &addr[at + 1..];
    if agent.is_empty() || host.is_empty() {
        return Err(format!(
            "remote address '{addr}' must be <agent>::<host> (or legacy <agent>@<host>; both sides non-empty)"
        ));
    }
    Ok((agent.to_string(), host.to_string()))
}

/// Canonical storage / display key: lowercase `agent::host`.
///
/// Accepts `@` or `::` on input. Host is collapsed via
/// [`normalize_remote_host`]. Gates match on agent+host columns so
/// pre-existing `@` rows keep working without migration.
pub fn normalize_remote_addr(addr: &str) -> Result<String, String> {
    let (agent, host) = parse_remote_addr(addr)?;
    let agent = agent.to_ascii_lowercase();
    let host = normalize_remote_host(&host);
    Ok(format!("{agent}::{host}"))
}

/// User-facing display form `agent::host` from stored agent/host columns
/// (normalize-on-read for old `@` rows; always lowercase).
pub fn display_remote_addr(agent: &str, host: &str) -> String {
    format!(
        "{}::{}",
        agent.trim().to_ascii_lowercase(),
        normalize_remote_host(host)
    )
}

/// Resolve a source workspace selector (a filesystem PATH, or — as a
/// fallback — an already-resolved project id) to its project id. The
/// gate's caller passes `$PROJECT`/CWD which is a path; the id fallback
/// keeps callers that already hold an id working.
fn resolve_source_project_id(conn: &rusqlite::Connection, path_or_id: &str) -> Option<String> {
    if let Some(id) = resolve_project_id(conn, path_or_id) {
        return Some(id);
    }
    // Fallback: treat the input as a project id directly.
    conn.query_row(
        "SELECT id FROM projects WHERE id = ?1",
        rusqlite::params![path_or_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// THE CROSS-DAEMON SEND GATE. Returns `true` iff the source workspace
/// (a path or project id) has a `workspace_remote_connections` row for
/// `remote_addr` (`agent::host` or legacy `agent@host`). Fails CLOSED —
/// an unresolvable source workspace or any DB error yields `false`, so no
/// connection ⇒ no cross-daemon send. Consulted by `federation::handle_send`.
///
/// Lookup normalizes separator + case; pre-existing `@` rows still match.
pub fn is_remote_connection(source_project_path_or_id: &str, remote_addr: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(source_id) = resolve_source_project_id(&conn, source_project_path_or_id) else {
        return false;
    };
    // Prefer canonical form when parse succeeds so `@`/`::` and case variants
    // all hit the same agent+host match in `exists`.
    let key = normalize_remote_addr(remote_addr)
        .unwrap_or_else(|_| remote_addr.trim().to_string());
    WorkspaceRemoteConnection::exists(&conn, &source_id, &key).unwrap_or(false)
}

/// C2 (0.40.45) — THE LOCAL SAME-DAEMON PEER GATE.
///
/// Returns `true` when `a` and `b` are the same project id, OR when a
/// `workspace_relations` row links them in either direction (the
/// bidirectional awareness model — one row per pair is enough).
/// Fails CLOSED: empty ids, unresolvable rows, or DB errors yield
/// `false`. Used by agent-initiated `msg` / `read` / `inbox compose`
/// into another workspace; owner ambient tokens bypass at the route
/// layer (Phase 2 residual).
pub fn are_local_peers(caller_project_id: &str, target_project_id: &str) -> bool {
    let a = caller_project_id.trim();
    let b = target_project_id.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT 1 FROM workspace_relations \
         WHERE (source_project_id = ?1 AND target_project_id = ?2) \
            OR (source_project_id = ?2 AND target_project_id = ?1) \
         LIMIT 1",
        rusqlite::params![a, b],
        |_| Ok(()),
    )
    .is_ok()
}

/// Dispatch by `action`. Returns a JSON-serialized string.
///
/// **0.39.0 list-shape change**: `action == "list"` now returns the
/// deduped, bidirectional [`Peer`] shape via [`list_peers`] —
/// `{"connections": [{"projectId": "...", "projectName": "...",
/// "relationTypes": [...] }]}`. The pre-0.39 directional shape
/// (`{"id", "direction", "type", "projectId", "projectName"}`) is
/// no longer surfaced because direction is an implementation detail
/// of the relation row, not a user-facing concept in the
/// workspace==agent model. Callers that still need the raw
/// directional rows can use `WorkspaceRelation::list_for_source` /
/// `list_for_target` directly.
///
/// # C1 (0.40.45) — agents-may-create-connections gate
///
/// `add` / `remove` are gated for non-privileged actors:
/// - **Privileged** (owner token, Owner/Admin connect-user): always allowed.
/// - **Agent / non-owner**: allowed only when
///   [`crate::workspace::settings::agents_can_create_connections_for_path`]
///   is true (app master OR per-workspace; default OFF).
///
/// `list` is never gated. Pass `actor_is_privileged = true` for owner-side
/// and in-process setup callers; the daemon computes the flag from the
/// request token / principal.
pub fn connections(
    project_path: &str,
    action: &str,
    target: Option<&str>,
    rel_type: Option<&str>,
) -> Result<String, String> {
    // Default: privileged (owner-like). In-process callers (tests,
    // federation fixtures, Tauri) are owner-side. The daemon agent path
    // must call [`connections_for_actor`] with `actor_is_privileged=false`.
    connections_for_actor(project_path, action, target, rel_type, true)
}

/// Teaching text when an agent is denied connection mutate (add/remove).
/// Canonical owner path — must match CLI exit-3 prose / Settings labels.
pub const CONNECTIONS_MUTATE_DENIED_HINT: &str = "this agent isn't allowed to create or remove \
connections — the owner can enable it in Settings → K2 Connect (Allow agents to create connections) \
or Workspaces → (workspace) → Allow agents to create connections";

/// Same as [`connections`] but with an explicit actor-privilege bit.
///
/// When `actor_is_privileged` is false and the effective
/// `agents_can_create_connections` toggle is off, `add`/`remove` return
/// [`CONNECTIONS_MUTATE_DENIED_HINT`].
pub fn connections_for_actor(
    project_path: &str,
    action: &str,
    target: Option<&str>,
    rel_type: Option<&str>,
    actor_is_privileged: bool,
) -> Result<String, String> {
    if matches!(action, "add" | "remove")
        && !actor_is_privileged
        && !crate::workspace::settings::agents_can_create_connections_for_path(project_path)
    {
        return Err(CONNECTIONS_MUTATE_DENIED_HINT.to_string());
    }
    match action {
        "list" => {
            let peers = list_peers(project_path)?;
            // 0.39.45 (#31): identify WHOSE connections these are. The
            // CLI resolves the source workspace by walking up from the
            // caller's CWD — when that walk lands on a parent/sibling
            // project, the operator used to see an "inherited" list
            // with no clue it wasn't their workspace's. Echoing the
            // resolved identity makes that failure self-evident.
            //
            // GAP #3: also pull REMOTE (cross-daemon) connections for the
            // resolved source workspace and merge them into the same
            // `connections` array (each marked `remote: true`).
            let (workspace, remotes) = {
                let db = crate::db::shared();
                let conn = db.lock();
                let name: Option<String> = conn
                    .query_row(
                        "SELECT name FROM projects WHERE path = ?1",
                        rusqlite::params![project_path],
                        |row| row.get(0),
                    )
                    .ok();
                let ws = serde_json::json!({ "name": name, "path": project_path });
                // Remote connections (best-effort: an unregistered source
                // workspace simply has none).
                let remotes: Vec<WorkspaceRemoteConnection> =
                    match resolve_source_project_id(&conn, project_path) {
                        Some(src_id) => {
                            WorkspaceRemoteConnection::list_for_source(&conn, &src_id)
                                .map_err(|e| e.to_string())?
                        }
                        None => Vec::new(),
                    };
                (ws, remotes)
            };
            // Emit as `{"connections": [...]}` for backwards-compatible
            // envelope shape (CLI parsers still key on "connections").
            // Local entries use the Peer struct's serde camelCase form
            // — `projectId`, `projectName`, `path`, `reachable`,
            // `relationTypes`, `remote: false`. Remote entries carry a
            // uniform-shaped object marked `remote: true`.
            let mut connections: Vec<serde_json::Value> = peers
                .into_iter()
                .map(|p| serde_json::to_value(p).unwrap_or(serde_json::Value::Null))
                .collect();
            for r in remotes {
                // 0.40.21: flag whether a trusted federation peer is actually
                // paired for this remote connection's host. `paired:false`
                // makes the connection-without-peer half-state visible in
                // `connections list` instead of only at send-time as a silent
                // 404. (`reachable` keeps its prior meaning — the row exists —
                // so existing parsers are unchanged; `paired` is the new
                // routing-readiness signal.)
                //
                // PR3 / GH #13: surface `peerFingerprint` + `unbound` when the
                // row has no fingerprint yet. Soft-reconcile: if a Trusted
                // peer now matches the host, bind the fingerprint in-place so
                // pre-existing unbound rows heal without a re-add.
                let mut peer_fp = r
                    .peer_fingerprint
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                if peer_fp.is_none() {
                    if let Some(fp) = trusted_peer_fingerprint_for_host(&r.host) {
                        // Never soft-bind the local fingerprint as a "remote"
                        // peer (self-peer hygiene; same gate as add).
                        let is_self = crate::federation::local_fingerprint()
                            .ok()
                            .is_some_and(|local| local == fp);
                        if !is_self {
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = WorkspaceRemoteConnection::set_peer_fingerprint(
                                &conn, &r.id, Some(&fp),
                            );
                            peer_fp = Some(fp);
                        }
                    }
                }
                let paired = peer_fp.is_some() || host_has_trusted_peer(&r.host);
                let unbound = peer_fp.is_none();
                // PR2: always surface the user form `agent::host` (normalize
                // on read so pre-existing `@` rows list canonically).
                let address = display_remote_addr(&r.agent, &r.host);
                connections.push(serde_json::json!({
                    "remote": true,
                    "address": address,
                    "host": r.host,
                    "agent": r.agent,
                    "projectName": address,
                    "reachable": true,
                    "paired": paired,
                    "peerFingerprint": peer_fp,
                    "unbound": unbound,
                    "relationTypes": [],
                }));
            }
            Ok(serde_json::json!({
                "workspace": workspace,
                "connections": connections
            })
            .to_string())
        }
        "add" => {
            let db = crate::db::shared();
            let conn = db.lock();
            let project_id = resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {}", project_path))?;

            let target_name = target
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Missing 'target' parameter (workspace name or path)".to_string())?;

            // GAP #3: a REMOTE (cross-daemon) user address
            // (`agent::host` or legacy `agent@host`). Persist it in
            // `workspace_remote_connections` instead of `workspace_relations`
            // — the peer has no local `projects` row to point at. Idempotent:
            // re-adding the same source+agent+host is a no-op.
            if is_remote_target(target_name) {
                // PR2: canonical store is lowercase `agent::host`. Accept
                // `@` or `::` on write. Host collapse strips accidental
                // `host.k2.dev.k2.dev` (PR3). Case-insensitive agent+host
                // match (see `WorkspaceRemoteConnection::exists`) keeps
                // folder-case-vs-display-case and old `@` rows working.
                let remote_addr = normalize_remote_addr(target_name.trim())?;
                let (agent, host) = parse_remote_addr(&remote_addr)?;
                // PR3: bind peer_fingerprint from the Trusted peer that
                // routes to this host (GH #13). Fail soft when unpaired —
                // still create the row + warn (auto-pair may land later).
                let peer_fp = trusted_peer_fingerprint_for_host(&host);
                // Reject self-peer: a "remote" whose fingerprint is THIS
                // daemon's identity is not a cross-server peer.
                if let Some(ref fp) = peer_fp {
                    if let Ok(local_fp) = crate::federation::local_fingerprint() {
                        if fp == &local_fp {
                            return Err(format!(
                                "cannot add remote connection '{remote_addr}': peer fingerprint \
                                 matches this server (self-peer) — use a different host"
                            ));
                        }
                    }
                }
                if WorkspaceRemoteConnection::exists(&conn, &project_id, &remote_addr)
                    .map_err(|e| e.to_string())?
                {
                    // Soft-reconcile: fill an empty peer_fingerprint if a
                    // Trusted peer is now available for this host.
                    if let Some(ref fp) = peer_fp {
                        if let Ok(rows) =
                            WorkspaceRemoteConnection::list_for_source(&conn, &project_id)
                        {
                            if let Some(row) = rows.iter().find(|r| {
                                r.agent.eq_ignore_ascii_case(&agent)
                                    && r.host.eq_ignore_ascii_case(&host)
                            }) {
                                let empty = row
                                    .peer_fingerprint
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .is_none();
                                if empty {
                                    let _ = WorkspaceRemoteConnection::set_peer_fingerprint(
                                        &conn, &row.id, Some(fp),
                                    );
                                }
                            }
                        }
                    }
                    return Ok(serde_json::json!({
                        "success": true,
                        "target": remote_addr,
                        "remote": true,
                        "noop": true,
                        "paired": peer_fp.is_some(),
                        "peerFingerprint": peer_fp,
                        "unbound": peer_fp.is_none(),
                        "message": format!("already connected to {}", remote_addr),
                    })
                    .to_string());
                }
                let id = uuid::Uuid::new_v4().to_string();
                WorkspaceRemoteConnection::create(
                    &conn,
                    &id,
                    &project_id,
                    &remote_addr,
                    &host,
                    &agent,
                    peer_fp.as_deref(),
                )
                .map_err(|e| e.to_string())?;
                log_activity(
                    &conn,
                    &project_id,
                    None,
                    "connection.created",
                    None,
                    None,
                    None,
                    Some(&format!("Connected to {} (remote)", remote_addr)),
                );
                // 0.40.21: a remote connection with NO trusted peer paired for
                // its host is a half-state — the row exists but cross-daemon
                // sends will 404 at dial time. Surface it (log + response)
                // instead of letting the operator discover it as a silent
                // send-time failure. Advisory only: the row is still created
                // (auto-pair may add the connection BEFORE the SAS confirm).
                let paired = peer_fp.is_some();
                let mut resp = serde_json::json!({
                    "success": true,
                    "id": id,
                    "target": remote_addr,
                    "remote": true,
                    "paired": paired,
                    "peerFingerprint": peer_fp,
                    "unbound": !paired,
                });
                if !paired {
                    let warning = format!(
                        "connection '{}' added but no trusted federation peer is paired for \
                         host '{}' — pair it (cockpit add-remote / auto-pair) or messages to \
                         it will fail to route (peer_fingerprint unbound)",
                        remote_addr, host
                    );
                    eprintln!("[connections] WARNING: {warning}");
                    resp["warning"] = serde_json::Value::String(warning);
                }
                drop(conn);
                crate::workspace::context_layers::refresh_roster_after_connection_change(&[
                    project_path,
                ]);
                return Ok(resp.to_string());
            }

            // 0.39.45 (#32/#33): case-insensitive name fallback + a
            // did-you-mean error instead of the bare not-found.
            let target_id = resolve_target_project_id(&conn, target_name)?;

            let target_display: String = conn
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    rusqlite::params![target_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| target_name.to_string());

            // Bidirectional-awareness guard (0.39.0): a connection
            // between two workspaces implies BOTH-WAYS awareness
            // regardless of which side initiated it (see [`list_peers`]
            // and migration 0051). If the REVERSE pair already exists
            // (target → this workspace), inserting a new (this →
            // target) row would create the same kind of redundant pair
            // migration 0051 just cleaned up. No-op so the dedup work
            // isn't undone by future redundant adds.
            //
            // Reads the reverse row's (id, relation_type) so the
            // response can tell the caller exactly what existing
            // relation satisfied their request — no surprises if their
            // intent was to confirm a connection, not to add a new one.
            let existing_reverse: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, relation_type FROM workspace_relations \
                     WHERE source_project_id = ?1 AND target_project_id = ?2 \
                     LIMIT 1",
                    rusqlite::params![target_id, project_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .ok();

            if let Some((existing_id, existing_type)) = existing_reverse {
                return Ok(serde_json::json!({
                    "success": true,
                    "id": existing_id,
                    "target": target_display,
                    "noop": true,
                    "message": format!(
                        "already connected (existing relation: {} → this workspace, type: {})",
                        target_display, existing_type
                    ),
                })
                .to_string());
            }

            let id = uuid::Uuid::new_v4().to_string();
            let rel_type = rel_type.unwrap_or("oversees");
            WorkspaceRelation::create(&conn, &id, &project_id, &target_id, rel_type)
                .map_err(|e| e.to_string())?;

            log_activity(
                &conn,
                &project_id,
                None,
                "connection.created",
                None,
                None,
                Some(&target_id),
                Some(&format!("Connected to {}", target_display)),
            );

            let target_path: String = conn
                .query_row(
                    "SELECT path FROM projects WHERE id = ?1",
                    rusqlite::params![target_id],
                    |row| row.get(0),
                )
                .unwrap_or_default();
            drop(conn);
            crate::workspace::context_layers::refresh_roster_after_connection_change(&[
                project_path,
                target_path.as_str(),
            ]);

            Ok(serde_json::json!({
                "success": true,
                "id": id,
                "target": target_display,
            })
            .to_string())
        }
        "remove" => {
            let db = crate::db::shared();
            let conn = db.lock();
            let project_id = resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {}", project_path))?;

            let target_name = target
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Missing 'target' parameter".to_string())?;

            // GAP #3: remote (`agent::host` / legacy `agent@host`) removal —
            // delete the `workspace_remote_connections` row for this source.
            if is_remote_target(target_name) {
                // Same normalize as `add` so remove matches `@`/`::` and case
                // variants (delete matches on agent+host columns).
                let remote_addr = normalize_remote_addr(target_name.trim())?;
                let removed = WorkspaceRemoteConnection::delete(&conn, &project_id, &remote_addr)
                    .map_err(|e| e.to_string())?;
                if removed == 0 {
                    return Err(format!("No connection to '{}' found", remote_addr));
                }
                log_activity(
                    &conn,
                    &project_id,
                    None,
                    "connection.removed",
                    None,
                    None,
                    None,
                    Some(&format!("Disconnected from {} (remote)", remote_addr)),
                );
                drop(conn);
                crate::workspace::context_layers::refresh_roster_after_connection_change(&[
                    project_path,
                ]);
                return Ok(serde_json::json!({"success": true, "remote": true}).to_string());
            }

            // 0.39.45 (#33): same case-insensitive + did-you-mean
            // resolution as `add`.
            let target_id = resolve_target_project_id(&conn, target_name)?;

            let target_path: String = conn
                .query_row(
                    "SELECT path FROM projects WHERE id = ?1",
                    rusqlite::params![target_id],
                    |row| row.get(0),
                )
                .unwrap_or_default();

            let rel_id: Result<String, _> = conn.query_row(
                "SELECT id FROM workspace_relations WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![project_id, target_id],
                |row| row.get(0),
            );
            match rel_id {
                Ok(id) => {
                    WorkspaceRelation::delete(&conn, &id).map_err(|e| e.to_string())?;
                    log_activity(
                        &conn,
                        &project_id,
                        None,
                        "connection.removed",
                        None,
                        None,
                        Some(&target_id),
                        Some(&format!("Disconnected from {}", target_name)),
                    );
                    drop(conn);
                    crate::workspace::context_layers::refresh_roster_after_connection_change(&[
                        project_path,
                        target_path.as_str(),
                    ]);
                    Ok(serde_json::json!({"success": true}).to_string())
                }
                Err(_) => Err(format!("No connection to '{}' found", target_name)),
            }
        }
        other => Err(format!(
            "Unknown action '{}'. Use: list, add, remove",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    //! Tests for `list_peers` — the bidirectional, deduped peer-list
    //! helper added in 0.39.0. These exercise the four shapes
    //! `list_peers` has to handle correctly:
    //!
    //!   - no relations at all → empty Vec
    //!   - outgoing-only relation → one peer
    //!   - incoming-only relation → one peer
    //!   - bidirectional (outgoing + incoming for same peer) →
    //!     one peer with both relation_types listed
    //!
    //! Plus alphabetical sort of multi-peer output.
    //!
    //! The shared in-memory test DB (`init_for_tests`) accumulates
    //! state across tests in the same binary, so each test uses a
    //! UUID-suffixed path to keep its `projects` rows isolated.
    use super::*;
    use uuid::Uuid;

    /// Insert a `projects` row at a unique path so the test doesn't
    /// collide with other tests sharing the same in-memory DB.
    /// Returns `(project_path, project_id)`.
    fn make_project(suffix: &str) -> (String, String) {
        let db = crate::db::shared();
        let conn = db.lock();
        let path = format!("/tmp/list-peers-test-{}-{}", suffix, Uuid::new_v4());
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, suffix, path],
        )
        .expect("insert project");
        (path, id)
    }

    // ── C2 (0.40.45): are_local_peers pure peer-id gate ─────────────

    #[test]
    fn are_local_peers_same_id_ok() {
        let (_p, id) = make_project("same-id");
        assert!(are_local_peers(&id, &id));
    }

    #[test]
    fn are_local_peers_strangers_false() {
        let (_pa, a) = make_project("stranger-a");
        let (_pb, b) = make_project("stranger-b");
        assert!(!are_local_peers(&a, &b));
        assert!(!are_local_peers(&b, &a));
    }

    #[test]
    fn are_local_peers_linked_either_direction() {
        let (_pa, a) = make_project("link-a");
        let (_pb, b) = make_project("link-b");
        let db = crate::db::shared();
        let conn = db.lock();
        WorkspaceRelation::create(&conn, &Uuid::new_v4().to_string(), &a, &b, "collaborator")
            .expect("link");
        drop(conn);
        assert!(are_local_peers(&a, &b), "forward edge");
        assert!(are_local_peers(&b, &a), "reverse awareness");
    }

    #[test]
    fn list_peers_returns_empty_when_no_relations() {
        let (path, _id) = make_project("solo");
        let peers = list_peers(&path).expect("list_peers ok");
        assert!(
            peers.is_empty(),
            "workspace with no relations should have no peers; got {peers:?}"
        );
    }

    #[test]
    fn list_peers_returns_outgoing_only_relation_once() {
        let (src_path, src_id) = make_project("out-src");
        let (_tgt_path, tgt_id) = make_project("out-tgt");
        let db = crate::db::shared();
        let conn = db.lock();
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &src_id,
            &tgt_id,
            "oversees",
        )
        .unwrap();
        drop(conn);

        let peers = list_peers(&src_path).expect("list_peers ok");
        assert_eq!(peers.len(), 1, "outgoing-only relation should yield 1 peer; got {peers:?}");
        let peer = &peers[0];
        assert_eq!(peer.project_id, tgt_id);
        assert_eq!(peer.relation_types, vec!["oversees".to_string()]);
    }

    #[test]
    fn list_peers_returns_incoming_only_relation_once() {
        let (src_path, src_id) = make_project("in-src");
        let (tgt_path, tgt_id) = make_project("in-tgt");
        let db = crate::db::shared();
        let conn = db.lock();
        // Relation goes src → tgt; we call list_peers on tgt, so tgt
        // sees src as an INCOMING peer.
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &src_id,
            &tgt_id,
            "collaborator",
        )
        .unwrap();
        drop(conn);

        let peers = list_peers(&tgt_path).expect("list_peers ok");
        assert_eq!(peers.len(), 1, "incoming-only relation should yield 1 peer; got {peers:?}");
        let peer = &peers[0];
        assert_eq!(peer.project_id, src_id);
        assert_eq!(peer.relation_types, vec!["collaborator".to_string()]);
        // sanity: the source-path side doesn't see itself
        let _ = src_path;
    }

    #[test]
    fn list_peers_dedupes_bidirectional_relation_to_one_entry_with_both_relation_types() {
        let (a_path, a_id) = make_project("bi-a");
        let (_b_path, b_id) = make_project("bi-b");
        let db = crate::db::shared();
        let conn = db.lock();
        // A → B as "oversees"; B → A as "collaborator".
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &a_id,
            &b_id,
            "oversees",
        )
        .unwrap();
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &b_id,
            &a_id,
            "collaborator",
        )
        .unwrap();
        drop(conn);

        let peers = list_peers(&a_path).expect("list_peers ok");
        assert_eq!(
            peers.len(),
            1,
            "bidirectional relation should dedupe to 1 peer; got {peers:?}"
        );
        let peer = &peers[0];
        assert_eq!(peer.project_id, b_id);
        // Both relation_types present (sorted alphabetically:
        // "collaborator" < "oversees").
        assert_eq!(
            peer.relation_types,
            vec!["collaborator".to_string(), "oversees".to_string()],
            "bidirectional relation should expose both labels"
        );
    }

    // ── add-side reverse-pair no-op guard (0.39.0) ───────────────
    //
    // Phase 2.5b workspace==agent insight: a connection between two
    // workspaces implies BIDIRECTIONAL awareness regardless of who
    // initiated it. `connections("add", ...)` therefore must NOT
    // insert a (this→target) row when the reverse (target→this)
    // already exists, or migration 0051's symmetric-dedup work would
    // be silently undone by every future `k2so connections add`.

    #[test]
    fn add_creates_new_relation_when_no_existing() {
        let (src_path, src_id) = make_project("add-new-src");
        let (_tgt_path, tgt_id) = make_project("add-new-tgt");

        // Baseline: no reverse exists; add should insert a fresh row.
        let resp = connections(&src_path, "add", Some("add-new-tgt"), Some("oversees"))
            .expect("add should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        // No "noop" flag because this is a real insert.
        assert!(
            parsed.get("noop").is_none(),
            "fresh add should NOT carry noop flag; got {parsed}"
        );

        let db = crate::db::shared();
        let conn = db.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![src_id, tgt_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "fresh add should create exactly 1 row");
    }

    #[test]
    fn add_no_ops_when_reverse_pair_already_exists() {
        let (src_path, src_id) = make_project("add-noop-src");
        let (_tgt_path, tgt_id) = make_project("add-noop-tgt");

        // Seed the REVERSE pair (target → source) FIRST. This is the
        // "other side initiated the connection" case.
        let db = crate::db::shared();
        {
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &tgt_id,
                &src_id,
                "collaborator",
            )
            .unwrap();
        }

        // Now src side tries to add tgt — must no-op because the
        // bidirectional-awareness contract says they're already
        // connected.
        let resp = connections(&src_path, "add", Some("add-noop-tgt"), Some("oversees"))
            .expect("add should succeed (no-op)");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        assert_eq!(
            parsed["noop"],
            serde_json::json!(true),
            "reverse-pair-exists add must report noop:true; got {parsed}"
        );

        // Storage layer: still exactly 1 row total for this pair (the
        // pre-existing reverse), zero in the forward direction. This
        // is the invariant migration 0051 protects.
        let conn = db.lock();
        let forward: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![src_id, tgt_id],
                |r| r.get(0),
            )
            .unwrap();
        let reverse: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![tgt_id, src_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forward, 0,
            "forward row must NOT be inserted when reverse exists"
        );
        assert_eq!(reverse, 1, "pre-existing reverse row must remain untouched");
    }

    #[test]
    fn add_returns_helpful_message_when_no_op_due_to_reverse_existing() {
        let (src_path, src_id) = make_project("add-msg-src");
        let (_tgt_path, tgt_id) = make_project("add-msg-tgt");

        // Reverse seeded with a distinctive type so the message can
        // surface it.
        let db = crate::db::shared();
        {
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &tgt_id,
                &src_id,
                "peer",
            )
            .unwrap();
        }
        let _ = src_id;

        let resp = connections(&src_path, "add", Some("add-msg-tgt"), Some("oversees"))
            .expect("add should succeed (no-op)");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("already connected"),
            "noop message must announce 'already connected'; got {message:?}"
        );
        assert!(
            message.contains("peer"),
            "noop message must surface the existing relation type so the caller \
             knows what's there; got {message:?}"
        );
    }

    #[test]
    fn list_peers_unaffected_by_add_no_op_guard() {
        // Regression: confirm dedupe still works the same when the
        // no-op guard turns redundant adds into no-ops. Before this
        // test runs, the only row is the reverse (target→source);
        // list_peers from either side should still surface the peer
        // exactly once with the reverse-side relation_type.
        let (src_path, src_id) = make_project("add-list-src");
        let (tgt_path, tgt_id) = make_project("add-list-tgt");

        let db = crate::db::shared();
        {
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &tgt_id,
                &src_id,
                "collaborator",
            )
            .unwrap();
        }

        // src side: attempts add → no-op.
        let _ = connections(&src_path, "add", Some("add-list-tgt"), Some("oversees"));

        // list_peers from src side should see tgt exactly once.
        let src_peers = list_peers(&src_path).expect("list_peers ok");
        assert_eq!(
            src_peers.len(),
            1,
            "src must see exactly 1 peer (incoming reverse); got {src_peers:?}"
        );
        assert_eq!(src_peers[0].project_id, tgt_id);

        // list_peers from tgt side should see src exactly once.
        let tgt_peers = list_peers(&tgt_path).expect("list_peers ok");
        assert_eq!(
            tgt_peers.len(),
            1,
            "tgt must see exactly 1 peer (its own outgoing); got {tgt_peers:?}"
        );
        assert_eq!(tgt_peers[0].project_id, src_id);
    }

    // ── Migration 0051 smoke test ────────────────────────────────
    //
    // Seeds symmetric (A→B + B→A) AND a solo pair (C→D) on a fresh
    // isolated DB, runs migrations, asserts the symmetric pair
    // collapsed to one row with both relation_types merged, and the
    // solo pair remained untouched. Uses an isolated test connection
    // so we can control migration ordering (the shared `init_for_tests`
    // already ran migrations).

    #[test]
    fn migration_0051_dedupes_symmetric_pairs_and_merges_relation_types() {
        use rusqlite::Connection;

        // Fresh in-memory DB; manually run migrations up to (not
        // including) 0051 so we can seed the legacy state, then run
        // 0051 in isolation.
        let conn = Connection::open(":memory:").unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::run_migrations(&conn).expect("run all migrations");

        // Clear out everything seeded by other tests / migrations.
        conn.execute("DELETE FROM workspace_relations", []).unwrap();

        // Seed 4 projects: a, b for the symmetric pair; c, d for solo.
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES \
             ('p-a', '/tmp/0051-a', 'a'), \
             ('p-b', '/tmp/0051-b', 'b'), \
             ('p-c', '/tmp/0051-c', 'c'), \
             ('p-d', '/tmp/0051-d', 'd')",
            [],
        )
        .unwrap();

        // A→B as 'peer' at created_at=100.
        // B→A as 'collaborator' at created_at=200.
        // C→D as 'oversees' at created_at=150 (solo, must stay).
        conn.execute(
            "INSERT INTO workspace_relations \
             (id, source_project_id, target_project_id, relation_type, created_at) VALUES \
             ('rel-ab', 'p-a', 'p-b', 'peer',         100), \
             ('rel-ba', 'p-b', 'p-a', 'collaborator', 200), \
             ('rel-cd', 'p-c', 'p-d', 'oversees',     150)",
            [],
        )
        .unwrap();

        // Sanity: 3 rows before dedup.
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_relations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 3, "test setup should produce 3 relation rows");

        // Backdate the 0051 marker so we can replay it manually
        // against the seeded state. Then re-run migrations — 0051
        // fires on the symmetric pair.
        conn.execute(
            "DELETE FROM _migrations WHERE name = '0051_dedup_symmetric_workspace_relations'",
            [],
        )
        .unwrap();
        crate::db::run_migrations(&conn).expect("re-run with 0051");

        // After dedup: 2 rows (one merged from A↔B + the solo C→D).
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_relations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, 2,
            "expected 2 rows after dedup (1 merged symmetric + 1 solo); got {after}"
        );

        // Solo row untouched.
        let solo_rt: String = conn
            .query_row(
                "SELECT relation_type FROM workspace_relations WHERE id = 'rel-cd'",
                [],
                |r| r.get(0),
            )
            .expect("solo C→D row must survive");
        assert_eq!(
            solo_rt, "oversees",
            "solo C→D row's relation_type must be untouched"
        );

        // Surviving merged row: the EARLIER created_at (A→B at 100)
        // is the keeper; the LATER (B→A at 200) was deleted.
        let keeper_id: String = conn
            .query_row(
                "SELECT id FROM workspace_relations \
                 WHERE source_project_id = 'p-a' AND target_project_id = 'p-b'",
                [],
                |r| r.get(0),
            )
            .expect("keeper A→B row must remain");
        assert_eq!(keeper_id, "rel-ab", "earlier row must be the keeper");

        // Reverse direction must be gone.
        let reverse_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = 'p-b' AND target_project_id = 'p-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reverse_exists, 0, "later B→A row must be deleted");

        // Merged relation_type contains both original labels (order
        // doesn't matter — assert by substring presence on both).
        let merged_rt: String = conn
            .query_row(
                "SELECT relation_type FROM workspace_relations WHERE id = 'rel-ab'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            merged_rt.contains("peer"),
            "merged row must retain 'peer' label; got {merged_rt:?}"
        );
        assert!(
            merged_rt.contains("collaborator"),
            "merged row must retain 'collaborator' label; got {merged_rt:?}"
        );
        assert!(
            merged_rt.contains(';'),
            "merged row must use ';' as separator; got {merged_rt:?}"
        );
    }

    #[test]
    fn migration_0051_is_no_op_on_db_with_no_symmetric_pairs() {
        // Fresh-workspace edge case: a DB whose `workspace_relations`
        // either is empty or only carries unique directional rows
        // should pass through 0051 unchanged. Both states are valid
        // post-migration outcomes.
        use rusqlite::Connection;
        let conn = Connection::open(":memory:").unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::run_migrations(&conn).expect("run all migrations");

        conn.execute("DELETE FROM workspace_relations", []).unwrap();
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES \
             ('p-x', '/tmp/0051-x', 'x'), \
             ('p-y', '/tmp/0051-y', 'y'), \
             ('p-z', '/tmp/0051-z', 'z')",
            [],
        )
        .unwrap();
        // Two solo (non-symmetric) rows. Must both survive.
        conn.execute(
            "INSERT INTO workspace_relations \
             (id, source_project_id, target_project_id, relation_type, created_at) VALUES \
             ('rel-xy', 'p-x', 'p-y', 'oversees', 100), \
             ('rel-yz', 'p-y', 'p-z', 'peer',     200)",
            [],
        )
        .unwrap();

        // Re-run 0051 explicitly.
        conn.execute(
            "DELETE FROM _migrations WHERE name = '0051_dedup_symmetric_workspace_relations'",
            [],
        )
        .unwrap();
        crate::db::run_migrations(&conn).expect("re-run with 0051");

        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_relations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, 2,
            "non-symmetric rows must survive 0051 unchanged; got {after}"
        );
    }

    #[test]
    fn list_peers_sorted_alphabetically_by_name() {
        // Create three peers with names that won't sort the same as
        // insertion order. Names need to be predictable so we can
        // assert order — explicitly set them after make_project's
        // default-name insert by updating via UUIDs.
        let db = crate::db::shared();
        let conn = db.lock();
        let me_path = format!("/tmp/sort-me-{}", Uuid::new_v4());
        let me_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![me_id, "z-me", &me_path],
        )
        .unwrap();
        // Three peers with names that should appear in alphabetical
        // order: "alpha", "bravo", "charlie".
        let mut peer_ids = Vec::new();
        for name in ["charlie", "alpha", "bravo"] {
            let pid = Uuid::new_v4().to_string();
            let path = format!("/tmp/sort-peer-{}-{}", name, Uuid::new_v4());
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![pid, name, &path],
            )
            .unwrap();
            peer_ids.push((name, pid));
        }
        for (_name, pid) in &peer_ids {
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &me_id,
                pid,
                "oversees",
            )
            .unwrap();
        }
        drop(conn);

        let peers = list_peers(&me_path).expect("list_peers ok");
        let names: Vec<&str> = peers.iter().map(|p| p.project_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha", "bravo", "charlie"],
            "peers must be sorted alphabetically by name"
        );
    }

    // ── 0.39.45: case-insensitive resolution + did-you-mean (#32/#33)
    //    and list identity/reachability (#31) ───────────────────────

    #[test]
    fn add_resolves_target_name_case_insensitively() {
        let (src_path, _src_id) = make_project("CiAddSrc");
        let (_tgt_path, tgt_id) = make_project("CiAddTargetXq");

        // Lowercase token against a mixed-case registered name.
        let resp = connections(&src_path, "add", Some("ciaddtargetxq"), Some("oversees"))
            .expect("case-insensitive add must resolve");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        // The canonical (registered) name is echoed back, not the
        // caller's casing.
        assert_eq!(parsed["target"], serde_json::json!("CiAddTargetXq"));

        let db = crate::db::shared();
        let conn = db.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations WHERE target_project_id = ?1",
                rusqlite::params![tgt_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "the relation must actually be created");
    }

    #[test]
    fn add_unknown_target_errors_with_did_you_mean() {
        let (src_path, _src_id) = make_project("DymSrc");
        let (_p, _id) = make_project("Zephyr-Unique-Ws");

        let err = connections(&src_path, "add", Some("zephyr-unique-w"), None)
            .expect_err("unknown target must error loudly");
        assert!(
            err.contains("did you mean 'Zephyr-Unique-Ws'"),
            "error must carry a did-you-mean suggestion, got: {err}"
        );
    }

    #[test]
    fn suggest_prefers_near_miss_over_loose_containment() {
        let _ = make_project("Zq-Host");
        let _ = make_project("zq-host-smoke-ws");
        let db = crate::db::shared();
        let conn = db.lock();
        // 1-typo near-miss of the long name; also CONTAINS the short
        // name. The near-miss must win (live finding from 0.39.45
        // smoke testing: 'K2SO' was suggested for 'k2so-smoke-wz').
        assert_eq!(
            suggest_project_name(&conn, "zq-host-smoke-wz").as_deref(),
            Some("zq-host-smoke-ws"),
        );
    }

    #[test]
    fn suggest_returns_none_when_nothing_is_close() {
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(
            suggest_project_name(&conn, "qwjxzv-completely-unrelated-token-9z8y"),
            None,
            "a wildly-off token must not produce a (mis)suggestion"
        );
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("appa", "appa"), 0);
        assert_eq!(edit_distance("appa", "Appa".to_lowercase().as_str()), 0);
        assert_eq!(edit_distance("appa", "appas"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn list_includes_source_workspace_identity_and_peer_reachability() {
        let (a_path, a_id) = make_project("ListIdentA");
        let (b_path, b_id) = make_project("ListIdentB");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &a_id,
                &b_id,
                "oversees",
            )
            .unwrap();
        }
        // B's path exists on disk → reachable. (make_project paths are
        // fabricated, so create it for the reachable=true case.)
        std::fs::create_dir_all(&b_path).expect("create peer dir");

        let resp = connections(&a_path, "list", None, None).expect("list ok");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(
            parsed["workspace"]["path"],
            serde_json::json!(a_path),
            "list must echo the resolved source workspace path (#31)"
        );
        assert_eq!(parsed["workspace"]["name"], serde_json::json!("ListIdentA"));
        let conns = parsed["connections"].as_array().expect("connections array");
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0]["projectName"], serde_json::json!("ListIdentB"));
        assert_eq!(conns[0]["path"], serde_json::json!(b_path));
        assert_eq!(
            conns[0]["reachable"],
            serde_json::json!(true),
            "peer with an existing path must be reachable"
        );
        std::fs::remove_dir_all(&b_path).expect("cleanup peer dir");
    }

    #[test]
    fn list_flags_peer_with_missing_path_as_unreachable() {
        let (a_path, a_id) = make_project("UnreachA");
        let (_b_path, b_id) = make_project("UnreachB"); // path never created on disk
        {
            let db = crate::db::shared();
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &a_id,
                &b_id,
                "oversees",
            )
            .unwrap();
        }
        let peers = list_peers(&a_path).expect("list ok");
        assert_eq!(peers.len(), 1);
        assert!(
            !peers[0].reachable,
            "peer whose path does not exist on disk must be flagged unreachable (#31)"
        );
    }

    // ── GAP #3: remote (cross-daemon) connections ────────────────────

    #[test]
    fn parse_remote_addr_matrix_colon_at_wire() {
        // Canonical :: user form.
        assert_eq!(
            parse_remote_addr("ai::rpm.k2.dev").unwrap(),
            ("ai".to_string(), "rpm.k2.dev".to_string())
        );
        // Legacy @ form (split on LAST '@').
        assert_eq!(
            parse_remote_addr("ai@rpm.k2.dev").unwrap(),
            ("ai".to_string(), "rpm.k2.dev".to_string())
        );
        assert_eq!(
            parse_remote_addr("a@b@host.example").unwrap(),
            ("a@b".to_string(), "host.example".to_string())
        );
        // Loopback test host (dotted).
        assert_eq!(
            parse_remote_addr("bob::127.0.0.1:1").unwrap(),
            ("bob".to_string(), "127.0.0.1:1".to_string())
        );
        // 3-part wire form rejected for user parse.
        let wire_err = parse_remote_addr("fpdeadbeef::ws-uuid::agent").unwrap_err();
        assert!(
            wire_err.contains("wire form"),
            "3-part must be rejected as wire form; got {wire_err}"
        );
        // Both sides must be non-empty / host-like.
        assert!(parse_remote_addr("@host").is_err());
        assert!(parse_remote_addr("agent@").is_err());
        assert!(parse_remote_addr("no-at-sign").is_err());
        assert!(parse_remote_addr("agent::").is_err());
        assert!(parse_remote_addr("::host.k2.dev").is_err());
        // 2-part :: with non-host RHS rejected.
        assert!(parse_remote_addr("agent::localname").is_err());
    }

    #[test]
    fn normalize_remote_addr_canonical_colon_lowercase() {
        assert_eq!(
            normalize_remote_addr("Ai@RPM.K2.DEV").unwrap(),
            "ai::rpm.k2.dev"
        );
        assert_eq!(
            normalize_remote_addr("Ai::RPM.K2.DEV").unwrap(),
            "ai::rpm.k2.dev"
        );
        // Double zone collapse (PR3 host normalize).
        assert_eq!(
            normalize_remote_addr("ai::rpm.k2.dev.k2.dev").unwrap(),
            "ai::rpm.k2.dev"
        );
    }

    #[test]
    fn is_remote_target_detects_at_and_colon() {
        assert!(is_remote_target("ai@rpm.k2.dev"));
        assert!(is_remote_target("ai::rpm.k2.dev"));
        assert!(is_remote_target("bob::127.0.0.1:9"));
        assert!(!is_remote_target("LocalWorkspace"));
        assert!(!is_remote_target("/tmp/some/path"));
        // 3-part wire is NOT a remote *connection* target.
        assert!(!is_remote_target("fp::ws-uuid::agent"));
        // 2-part :: without host-looking RHS is not a remote target.
        assert!(!is_remote_target("agent::localname"));
    }

    #[test]
    fn looks_like_host_matrix() {
        assert!(looks_like_host("rpm.k2.dev"));
        assert!(looks_like_host("127.0.0.1:1"));
        assert!(looks_like_host("peer.example.com"));
        assert!(!looks_like_host("localname"));
        assert!(!looks_like_host(""));
        assert!(!looks_like_host("   "));
    }

    #[test]
    fn add_remote_persists_row_and_is_idempotent() {
        let (src_path, src_id) = make_project("RemoteAddSrc");
        let addr = "ai@rpm.k2.dev";

        // First add → creates the row, not a no-op.
        let resp = connections(&src_path, "add", Some(addr), None).expect("remote add ok");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        assert_eq!(parsed["remote"], serde_json::json!(true));
        assert!(parsed.get("noop").is_none(), "fresh remote add must not be a no-op");

        // Storage round-trip: exactly one row, correctly decomposed.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
            assert_eq!(rows.len(), 1, "remote add must persist exactly one row");
            // PR2: canonical storage is agent::host (legacy @ accepted on write).
            assert_eq!(rows[0].remote_addr, "ai::rpm.k2.dev");
            assert_eq!(rows[0].host, "rpm.k2.dev");
            assert_eq!(rows[0].agent, "ai");
        }

        // Second add of the SAME addr (and :: spelling) → idempotent no-op.
        let resp2 = connections(&src_path, "add", Some(addr), None).expect("remote re-add ok");
        let parsed2: serde_json::Value = serde_json::from_str(&resp2).expect("valid JSON");
        assert_eq!(parsed2["noop"], serde_json::json!(true), "re-add must report noop");
        let resp3 = connections(&src_path, "add", Some("ai::rpm.k2.dev"), None)
            .expect("colon re-add ok");
        let parsed3: serde_json::Value = serde_json::from_str(&resp3).expect("valid JSON");
        assert_eq!(parsed3["noop"], serde_json::json!(true), "colon re-add must no-op");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
            assert_eq!(rows.len(), 1, "idempotent re-add must not create a duplicate");
        }
    }

    #[test]
    fn add_colon_form_and_gate_matches_both_spellings() {
        let (src_path, _src_id) = make_project("ColonAddSrc");
        let resp = connections(&src_path, "add", Some("ops::peer.example.com"), None)
            .expect("colon add ok");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["target"], serde_json::json!("ops::peer.example.com"));

        assert!(is_remote_connection(&src_path, "ops::peer.example.com"));
        assert!(is_remote_connection(&src_path, "ops@peer.example.com"));
        assert!(is_remote_connection(&src_path, "OPS::PEER.EXAMPLE.COM"));
        assert!(!is_remote_connection(&src_path, "ops::other.example.com"));
    }

    #[test]
    fn remove_remote_deletes_row() {
        let (src_path, src_id) = make_project("RemoteRemoveSrc");
        let addr = "ops@peer.example.com";
        connections(&src_path, "add", Some(addr), None).expect("remote add ok");

        let resp = connections(&src_path, "remove", Some(addr), None).expect("remote remove ok");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));

        let db = crate::db::shared();
        let conn = db.lock();
        let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
        assert!(rows.is_empty(), "remove must delete the remote row");
    }

    #[test]
    fn remove_remote_errors_when_absent() {
        let (src_path, _src_id) = make_project("RemoteRemoveMissSrc");
        let err = connections(&src_path, "remove", Some("ghost@nowhere.dev"), None)
            .expect_err("removing a non-existent remote must error loudly");
        assert!(
            err.contains("No connection"),
            "missing-remote removal must report no connection; got {err}"
        );
    }

    #[test]
    fn list_merges_remote_connections_with_local() {
        let (src_path, src_id) = make_project("MergeSrc");
        let (_tgt_path, tgt_id) = make_project("MergeLocalTgt");
        // One LOCAL connection …
        {
            let db = crate::db::shared();
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &src_id,
                &tgt_id,
                "oversees",
            )
            .unwrap();
        }
        // … and one REMOTE connection.
        connections(&src_path, "add", Some("ai@rpm.k2.dev"), None).expect("remote add ok");

        let resp = connections(&src_path, "list", None, None).expect("list ok");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        let conns = parsed["connections"].as_array().expect("connections array");
        assert_eq!(conns.len(), 2, "list must include both local and remote; got {conns:?}");

        // Local entry carries remote:false (serde default), name = the
        // local project's name.
        let local = conns
            .iter()
            .find(|c| c["projectName"] == serde_json::json!("MergeLocalTgt"))
            .expect("local entry present");
        assert_eq!(local["remote"], serde_json::json!(false));

        // Remote entry carries the GAP #3 shape.
        let remote = conns
            .iter()
            .find(|c| c["remote"] == serde_json::json!(true))
            .expect("remote entry present");
        // PR2: list surfaces canonical agent::host (even when added via @).
        assert_eq!(remote["address"], serde_json::json!("ai::rpm.k2.dev"));
        assert_eq!(remote["host"], serde_json::json!("rpm.k2.dev"));
        assert_eq!(remote["agent"], serde_json::json!("ai"));
        assert_eq!(remote["projectName"], serde_json::json!("ai::rpm.k2.dev"));
        assert_eq!(remote["reachable"], serde_json::json!(true));
    }

    #[test]
    fn exists_matches_case_insensitively_for_preexisting_capital_row() {
        // Repro for the cross-server REPLY 403 ("not a connection"): the
        // reverse row was auto-created from the FOLDER BASENAME
        // (`Cortana@host`, capital C) but the agent's real display name is
        // `cortana` (lowercase), so the reply targets `cortana@host`. The
        // gate must match regardless of case — AND without any migration of
        // the pre-existing capital row.
        let (src_path, src_id) = make_project("CaseGateSrc");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            // Simulate the LIVE pre-existing CAPITAL row (no normalization).
            WorkspaceRemoteConnection::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &src_id,
                "Cortana@z3thon.k2.dev",
                "z3thon.k2.dev",
                "Cortana",
                None,
            )
            .unwrap();
        }

        // Lowercase @ lookup matches the capital stored row.
        assert!(
            is_remote_connection(&src_path, "cortana@z3thon.k2.dev"),
            "lowercase @ lookup must match a pre-existing capital row"
        );
        // Canonical :: lookup also matches old @ storage.
        assert!(
            is_remote_connection(&src_path, "cortana::z3thon.k2.dev"),
            "colon form must match a pre-existing @ row"
        );
        // Upper-case lookup also matches (fully case-insensitive).
        assert!(
            is_remote_connection(&src_path, "CORTANA@Z3THON.K2.DEV"),
            "upper-case lookup must match the stored row case-insensitively"
        );
        // Exact stored casing still matches.
        assert!(
            is_remote_connection(&src_path, "Cortana@z3thon.k2.dev"),
            "exact-case lookup must still match"
        );
        // A genuinely different agent stays closed (no false positives).
        assert!(
            !is_remote_connection(&src_path, "ai@z3thon.k2.dev"),
            "a different agent must NOT match"
        );
        // A different host stays closed.
        assert!(
            !is_remote_connection(&src_path, "cortana@other.k2.dev"),
            "a different host must NOT match"
        );
    }

    #[test]
    fn add_normalizes_remote_addr_to_lowercase_and_looks_up_case_insensitively() {
        let (src_path, src_id) = make_project("CaseAddSrc");

        // Add with mixed case — stored canonical form must be lowercased.
        let resp = connections(&src_path, "add", Some("Cortana@Z3thon.K2.Dev"), None)
            .expect("remote add ok");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        assert_eq!(parsed["remote"], serde_json::json!(true));
        assert_eq!(
            parsed["target"], serde_json::json!("cortana::z3thon.k2.dev"),
            "stored target must be normalized to lowercase agent::host"
        );

        {
            let db = crate::db::shared();
            let conn = db.lock();
            let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
            assert_eq!(rows.len(), 1, "exactly one row persisted");
            assert_eq!(rows[0].remote_addr, "cortana::z3thon.k2.dev");
            assert_eq!(rows[0].agent, "cortana");
            assert_eq!(rows[0].host, "z3thon.k2.dev");
        }

        // Gate opens for any casing / separator of the same addr.
        assert!(is_remote_connection(&src_path, "cortana@z3thon.k2.dev"));
        assert!(is_remote_connection(&src_path, "cortana::z3thon.k2.dev"));
        assert!(is_remote_connection(&src_path, "Cortana@Z3thon.K2.Dev"));
        assert!(is_remote_connection(&src_path, "CORTANA::Z3THON.K2.DEV"));

        // Re-adding a differently-cased form is an idempotent no-op (matches
        // the normalized row), so no duplicate is created.
        let resp2 = connections(&src_path, "add", Some("CORTANA@z3thon.k2.dev"), None)
            .expect("remote re-add ok");
        let parsed2: serde_json::Value = serde_json::from_str(&resp2).expect("valid JSON");
        assert_eq!(parsed2["noop"], serde_json::json!(true), "re-add must no-op");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
            assert_eq!(rows.len(), 1, "case-variant re-add must not duplicate");
        }

        // Remove with yet another casing / :: must delete the row.
        let resp3 = connections(&src_path, "remove", Some("CoRtAnA::Z3THON.k2.dev"), None)
            .expect("remote remove ok");
        let parsed3: serde_json::Value = serde_json::from_str(&resp3).expect("valid JSON");
        assert_eq!(parsed3["success"], serde_json::json!(true));
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
            assert!(rows.is_empty(), "case-variant remove must delete the row");
        }
    }

    #[test]
    fn is_remote_connection_true_only_for_added_remote() {
        let (src_path, _src_id) = make_project("GateSrc");
        let addr = "ai@gate.k2.dev";

        // Before adding → false (fail-closed).
        assert!(
            !is_remote_connection(&src_path, addr),
            "gate must be closed before the connection is added"
        );

        connections(&src_path, "add", Some(addr), None).expect("remote add ok");

        // After adding → true for both spellings.
        assert!(
            is_remote_connection(&src_path, addr),
            "gate must open for the added address (@ form)"
        );
        assert!(
            is_remote_connection(&src_path, "ai::gate.k2.dev"),
            "gate must open for the added address (:: form)"
        );
        // A DIFFERENT addr for the same source stays closed.
        assert!(
            !is_remote_connection(&src_path, "ai@other.k2.dev"),
            "gate must not open for an unconnected remote"
        );
        // An unknown source workspace stays closed (fail-closed).
        assert!(
            !is_remote_connection("/tmp/never-registered-ws-xyz", addr),
            "gate must fail closed for an unresolvable source workspace"
        );
    }

    // ── 0.40.21: connection-without-peer half-state surfaced as a WARNING ──

    #[test]
    fn add_remote_warns_and_flags_unpaired_when_no_trusted_peer() {
        // Isolate ~/.k2 so the peer store is empty (no trusted peer for the
        // host) — the remote add must still succeed but flag the half-state.
        crate::tunnel::test_support::with_temp_home(|| {
            let (src_path, _src_id) = make_project("WarnNoPeerSrc");
            let resp =
                connections(&src_path, "add", Some("ai@rpm.k2.dev"), None).expect("remote add ok");
            let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
            assert_eq!(parsed["success"], serde_json::json!(true));
            assert_eq!(
                parsed["paired"],
                serde_json::json!(false),
                "an unpaired remote host must report paired:false"
            );
            assert!(
                parsed["warning"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("no trusted federation peer"),
                "an unpaired remote add must carry a warning; got {parsed}"
            );
        });
    }

    #[test]
    fn add_remote_no_warning_when_trusted_peer_paired_for_host() {
        crate::tunnel::test_support::with_temp_home(|| {
            use crate::federation::{FederationPeer, PeerStore, PeerTrust};
            let (src_path, _src_id) = make_project("PairedPeerSrc");

            // Pin a TRUSTED peer whose subdomain ("rpm") routes to rpm.k2.dev.
            let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut store = PeerStore::default();
            let fp =
                store.upsert(FederationPeer::pin("rpm-box", "rpm", key.public_key_pem()).unwrap());
            store.set_trust(&fp, PeerTrust::Trusted);
            store.save().unwrap();

            let resp =
                connections(&src_path, "add", Some("ai@rpm.k2.dev"), None).expect("remote add ok");
            let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
            assert_eq!(
                parsed["paired"],
                serde_json::json!(true),
                "a host with a trusted peer must report paired:true"
            );
            assert!(
                parsed.get("warning").is_none(),
                "a paired remote add must NOT carry a warning; got {parsed}"
            );

            // `list` must reflect the paired state too.
            let listed = connections(&src_path, "list", None, None).expect("list ok");
            let lv: serde_json::Value = serde_json::from_str(&listed).expect("valid JSON");
            let remote = lv["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["remote"] == serde_json::json!(true))
                .expect("remote entry present");
            assert_eq!(
                remote["paired"],
                serde_json::json!(true),
                "list must mark the remote connection paired"
            );
        });
    }

    #[test]
    fn host_has_trusted_peer_matches_subdomain_and_full_host_case_insensitively() {
        crate::tunnel::test_support::with_temp_home(|| {
            use crate::federation::{FederationPeer, PeerStore, PeerTrust};
            let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut store = PeerStore::default();
            let fp =
                store.upsert(FederationPeer::pin("box", "rpm", key.public_key_pem()).unwrap());
            // A Pending peer must NOT count as routable.
            store.save().unwrap();
            assert!(
                !host_has_trusted_peer("rpm.k2.dev"),
                "a Pending peer must not satisfy the routability check"
            );

            // Promote → now the host routes (canonical <subdomain>.k2.dev,
            // case-insensitive).
            store.set_trust(&fp, PeerTrust::Trusted);
            store.save().unwrap();
            assert!(host_has_trusted_peer("rpm.k2.dev"));
            assert!(host_has_trusted_peer("RPM.K2.DEV"));
            // Bare subdomain and doubled-zone host also match after normalize.
            assert!(host_has_trusted_peer("rpm"));
            assert!(host_has_trusted_peer("rpm.k2.dev.k2.dev"));
            // A different host stays unrouted.
            assert!(!host_has_trusted_peer("other.k2.dev"));
            assert!(!host_has_trusted_peer(""));
        });
    }

    // ── PR3 / GH #13: peer_fingerprint bind + self-peer + host normalize ──

    #[test]
    fn normalize_remote_host_collapses_double_k2_zone() {
        assert_eq!(normalize_remote_host("RPM.K2.DEV"), "rpm.k2.dev");
        assert_eq!(
            normalize_remote_host("rpm.k2.dev.k2.dev"),
            "rpm.k2.dev",
            "double zone must collapse to a single trailing .k2.dev"
        );
        assert_eq!(
            normalize_remote_host("  rpm.k2.dev.k2.dev.k2.dev  "),
            "rpm.k2.dev"
        );
        assert_eq!(
            normalize_remote_host("peer.example.com"),
            "peer.example.com",
            "off-zone hosts must not grow a .k2.dev suffix"
        );
        assert_eq!(normalize_remote_host(""), "");
    }

    #[test]
    fn add_remote_binds_peer_fingerprint_when_trusted_peer_matches_host() {
        crate::tunnel::test_support::with_temp_home(|| {
            use crate::federation::{FederationPeer, PeerStore, PeerTrust};
            let (src_path, src_id) = make_project("BindFpSrc");

            let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut store = PeerStore::default();
            let fp =
                store.upsert(FederationPeer::pin("rpm-box", "rpm", key.public_key_pem()).unwrap());
            store.set_trust(&fp, PeerTrust::Trusted);
            store.save().unwrap();

            let resp =
                connections(&src_path, "add", Some("ai@rpm.k2.dev"), None).expect("remote add ok");
            let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
            assert_eq!(parsed["success"], serde_json::json!(true));
            assert_eq!(parsed["paired"], serde_json::json!(true));
            assert_eq!(parsed["unbound"], serde_json::json!(false));
            assert_eq!(
                parsed["peerFingerprint"].as_str(),
                Some(fp.as_str()),
                "add response must surface the bound peer fingerprint"
            );

            {
                let db = crate::db::shared();
                let conn = db.lock();
                let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(
                    rows[0].peer_fingerprint.as_deref(),
                    Some(fp.as_str()),
                    "row must persist peer_fingerprint on create"
                );
            }

            // list surfaces fingerprint + unbound:false
            let listed = connections(&src_path, "list", None, None).expect("list ok");
            let lv: serde_json::Value = serde_json::from_str(&listed).expect("valid JSON");
            let remote = lv["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["remote"] == serde_json::json!(true))
                .expect("remote entry");
            assert_eq!(remote["peerFingerprint"].as_str(), Some(fp.as_str()));
            assert_eq!(remote["unbound"], serde_json::json!(false));
            assert_eq!(remote["paired"], serde_json::json!(true));
        });
    }

    #[test]
    fn add_remote_unbound_when_no_trusted_peer_and_list_flags_unbound() {
        crate::tunnel::test_support::with_temp_home(|| {
            let (src_path, src_id) = make_project("UnboundFpSrc");
            let resp =
                connections(&src_path, "add", Some("ai@orphan.k2.dev"), None).expect("add ok");
            let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
            assert_eq!(parsed["paired"], serde_json::json!(false));
            assert_eq!(parsed["unbound"], serde_json::json!(true));
            assert!(parsed["peerFingerprint"].is_null());
            assert!(
                parsed["warning"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("unbound")
                    || parsed["warning"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("no trusted federation peer"),
                "unpaired add must warn; got {parsed}"
            );

            {
                let db = crate::db::shared();
                let conn = db.lock();
                let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
                assert_eq!(rows[0].peer_fingerprint, None);
            }

            let listed = connections(&src_path, "list", None, None).expect("list ok");
            let lv: serde_json::Value = serde_json::from_str(&listed).expect("valid JSON");
            let remote = lv["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["remote"] == serde_json::json!(true))
                .expect("remote entry");
            assert_eq!(remote["unbound"], serde_json::json!(true));
            assert!(remote["peerFingerprint"].is_null());
        });
    }

    #[test]
    fn add_remote_rejects_self_peer_when_trusted_fp_is_local() {
        crate::tunnel::test_support::with_temp_home(|| {
            use crate::federation::{local_fingerprint, FederationPeer, PeerStore, PeerTrust};
            let (src_path, _src_id) = make_project("SelfPeerSrc");

            // Ensure local key material exists, then pin THAT key as a
            // "trusted peer" for host self.k2.dev — connecting to it must
            // fail closed as self-peer.
            let local_fp = local_fingerprint().expect("local fp");
            let local_pem = crate::tunnel::tls::load_or_generate_keypair()
                .expect("local key")
                .public_key_pem();
            let mut store = PeerStore::default();
            let fp = store
                .upsert(FederationPeer::pin("myself", "self", local_pem).unwrap());
            assert_eq!(fp, local_fp, "pinned key must be the local fingerprint");
            store.set_trust(&fp, PeerTrust::Trusted);
            store.save().unwrap();

            let err = connections(&src_path, "add", Some("agent@self.k2.dev"), None)
                .expect_err("self-peer remote connection must be rejected");
            assert!(
                err.contains("self-peer") || err.contains("this server"),
                "error must name self-peer; got {err}"
            );
        });
    }

    #[test]
    fn list_soft_reconciles_empty_peer_fingerprint_from_trusted_peer() {
        crate::tunnel::test_support::with_temp_home(|| {
            use crate::federation::{FederationPeer, PeerStore, PeerTrust};
            let (src_path, src_id) = make_project("ReconcileFpSrc");

            // Pre-existing unbound row (as shipped before PR3).
            {
                let db = crate::db::shared();
                let conn = db.lock();
                WorkspaceRemoteConnection::create(
                    &conn,
                    &Uuid::new_v4().to_string(),
                    &src_id,
                    "ai@rpm.k2.dev",
                    "rpm.k2.dev",
                    "ai",
                    None,
                )
                .unwrap();
            }

            // Later: peer becomes Trusted for rpm.
            let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let mut store = PeerStore::default();
            let fp =
                store.upsert(FederationPeer::pin("rpm-box", "rpm", key.public_key_pem()).unwrap());
            store.set_trust(&fp, PeerTrust::Trusted);
            store.save().unwrap();

            let listed = connections(&src_path, "list", None, None).expect("list ok");
            let lv: serde_json::Value = serde_json::from_str(&listed).expect("valid JSON");
            let remote = lv["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["remote"] == serde_json::json!(true))
                .expect("remote entry");
            assert_eq!(
                remote["peerFingerprint"].as_str(),
                Some(fp.as_str()),
                "list must soft-bind fingerprint from trusted peer"
            );
            assert_eq!(remote["unbound"], serde_json::json!(false));

            // Persisted, not just response-only.
            {
                let db = crate::db::shared();
                let conn = db.lock();
                let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
                assert_eq!(rows[0].peer_fingerprint.as_deref(), Some(fp.as_str()));
            }
        });
    }

    #[test]
    fn add_remote_normalizes_double_zone_host_on_store() {
        crate::tunnel::test_support::with_temp_home(|| {
            let (src_path, src_id) = make_project("DoubleZoneSrc");
            let resp = connections(
                &src_path,
                "add",
                Some("ai@rpm.k2.dev.k2.dev"),
                None,
            )
            .expect("add ok");
            let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
            assert_eq!(
                parsed["target"],
                serde_json::json!("ai::rpm.k2.dev"),
                "stored target must collapse double .k2.dev into canonical agent::host"
            );
            {
                let db = crate::db::shared();
                let conn = db.lock();
                let rows = WorkspaceRemoteConnection::list_for_source(&conn, &src_id).unwrap();
                assert_eq!(rows[0].host, "rpm.k2.dev");
                assert_eq!(rows[0].remote_addr, "ai::rpm.k2.dev");
            }
        });
    }

    // ── C1 (0.40.45): agents_can_create_connections gate ───────────────

    /// Agent (non-privileged) is denied add/remove when the effective
    /// toggle is OFF (default). Owner/privileged always allowed.
    /// Effective OR: app master OR per-workspace unlocks the agent path.
    #[test]
    fn agents_create_connections_gate_owner_always_agent_toggle() {
        crate::tunnel::test_support::with_temp_home(|| {
            let (src_path, _) = make_project("C1GateSrc");
            let (tgt_path, _) = make_project("C1GateTgt");
            let _ = tgt_path;

            // Ensure both flags OFF.
            crate::app_settings::update(serde_json::json!({
                "agentsCanCreateConnections": false
            }))
            .expect("app off");
            let _ = crate::workspace::settings::update_project_setting(
                &src_path,
                "agents_can_create_connections",
                "0",
            );

            // Privileged (owner) always allowed even with toggle off.
            connections_for_actor(&src_path, "add", Some("C1GateTgt"), None, true)
                .expect("owner add must succeed with toggle off");
            connections_for_actor(&src_path, "remove", Some("C1GateTgt"), None, true)
                .expect("owner remove must succeed with toggle off");

            // Agent denied when toggle off.
            let err = connections_for_actor(&src_path, "add", Some("C1GateTgt"), None, false)
                .expect_err("agent add must fail with toggle off");
            assert!(
                err.contains("Allow agents to create connections"),
                "deny must teach Settings path; got {err}"
            );
            assert!(
                err.contains(CONNECTIONS_MUTATE_DENIED_HINT)
                    || err.contains("isn't allowed to create"),
                "deny must use the teaching hint; got {err}"
            );

            // List is never gated for agents.
            connections_for_actor(&src_path, "list", None, None, false)
                .expect("agent list must always work");

            // Per-workspace ON → agent allowed.
            crate::workspace::settings::update_project_setting(
                &src_path,
                "agents_can_create_connections",
                "1",
            )
            .expect("ws on");
            connections_for_actor(&src_path, "add", Some("C1GateTgt"), None, false)
                .expect("agent add with per-workspace on");
            connections_for_actor(&src_path, "remove", Some("C1GateTgt"), None, false)
                .expect("agent remove with per-workspace on");

            // Per-workspace OFF, app master ON → agent allowed (OR).
            crate::workspace::settings::update_project_setting(
                &src_path,
                "agents_can_create_connections",
                "0",
            )
            .expect("ws off");
            crate::app_settings::update(serde_json::json!({
                "agentsCanCreateConnections": true
            }))
            .expect("app on");
            connections_for_actor(&src_path, "add", Some("C1GateTgt"), None, false)
                .expect("agent add with app master on");
        });
    }
}
