//! App egress policy (`prd-k2-app-runtime-v1` §4.1, §5.6, §9.2, S15): the
//! pure and SQLite parts. The proxy itself (a per-App loopback listener,
//! HTTP + CONNECT, DNS in the proxy) is `k2-daemon`'s `apps::egress_proxy`.
//!
//! - **Host specs.** `host[:port]` (port 443 when left out): a lowercase
//!   A-label name with at least two labels, an IP literal, or
//!   `*.<suffix>` (any name under the suffix, never the suffix itself).
//!   No URLs, paths, user info or single-label names.
//! - **Modes.** `allowlist` (the guarded default): only hosts with status
//!   `allowed` pass. `broad` (the open path): any public host passes and
//!   each first-seen host is recorded. Either way the daemon's egress gate
//!   still refuses private, loopback, metadata and this box's own
//!   addresses, and the proxy refuses this box's own names.
//! - **Widening waits; narrowing doesn't.** A new host (or `broad`) is a
//!   held request in `app_widenings` until the owner approves it with a
//!   fresh owner confirm (§5.6, always per event, K25). Removing a host or
//!   going back to `allowlist` applies at once.
//! - **Seen hosts.** The proxy records what an App tried (`seen`), capped
//!   per App ([`MAX_SEEN`]) so an App making up names can't grow the table.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value as J;

pub const MODE_ALLOWLIST: &str = "allowlist";
pub const MODE_BROAD: &str = "broad";

pub const STATUS_ALLOWED: &str = "allowed";
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_SEEN: &str = "seen";

pub const SOURCE_MANIFEST: &str = "manifest";
pub const SOURCE_REQUEST: &str = "request";
pub const SOURCE_FIRST_SEEN: &str = "first_seen";

pub const KIND_EGRESS: &str = "egress";
pub const KIND_EGRESS_MODE: &str = "egress_mode";

pub const REQ_PENDING: &str = "pending";
pub const REQ_APPROVED: &str = "approved";
pub const REQ_DENIED: &str = "denied";
pub const REQ_WITHDRAWN: &str = "withdrawn";
pub const REQ_EXPIRED: &str = "expired";

/// Held requests expire after 7 days (§5.6 step 6).
pub const REQUEST_TTL_SECS: i64 = 7 * 24 * 3600;
/// Most `seen` rows kept per App.
pub const MAX_SEEN: i64 = 500;
/// Most hosts in one request or one manifest.
pub const MAX_HOSTS_PER_REQUEST: usize = 64;
/// Most allowed + pending hosts per App.
pub const MAX_LISTED: i64 = 256;

// ── host specs ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct HostSpec {
    /// Lowercase name, IP literal (IPv6 without brackets) or `*.<suffix>`.
    pub host: String,
    pub port: u16,
}

impl HostSpec {
    pub fn is_wildcard(&self) -> bool {
        self.host.starts_with("*.")
    }

    pub fn is_ip(&self) -> bool {
        self.host.parse::<std::net::IpAddr>().is_ok()
    }

    /// Does a request for `host:port` (already normalized with
    /// [`normalize_name`]) fall under this spec?
    pub fn matches(&self, host: &str, port: u16) -> bool {
        if port != self.port {
            return false;
        }
        match self.host.strip_prefix("*.") {
            Some(suffix) => host.len() > suffix.len() + 1 && host.ends_with(suffix) && host.as_bytes()[host.len() - suffix.len() - 1] == b'.',
            None => self.host == host,
        }
    }

    /// `host:port`, IPv6 in brackets.
    pub fn display(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl std::fmt::Display for HostSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display())
    }
}

fn label_ok(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 63
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !l.starts_with('-')
        && !l.ends_with('-')
}

/// A request's host name, as the proxy sees it: lowercase, no trailing
/// dot, A-labels only. `None` when it isn't a valid DNS name or IP.
pub fn normalize_name(raw: &str) -> Option<String> {
    let h = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let h = h.strip_prefix('[').and_then(|x| x.strip_suffix(']')).map(str::to_string).unwrap_or(h);
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }
    if h.is_empty() || h.len() > 253 || !h.is_ascii() {
        return None;
    }
    h.split('.').all(label_ok).then_some(h)
}

/// Names that never leave the box (refused even on `broad`).
pub fn local_name(host: &str) -> bool {
    let h = host.trim_start_matches("*.");
    !h.contains('.')
        || ["localhost", "local", "internal", "localdomain", "home.arpa", "in-addr.arpa", "ip6.arpa"]
            .iter()
            .any(|s| h == *s || h.ends_with(&format!(".{s}")))
}

/// Parse an owner/agent/manifest host spec. Errors teach the shape.
pub fn parse_spec(raw: &str) -> Result<HostSpec, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("empty host".into());
    }
    if t.contains("://") || t.contains('/') || t.contains('@') || t.contains('?') || t.contains('#') {
        return Err(format!("'{t}': give host[:port] (like api.stripe.com:443), not a URL"));
    }
    let (host_part, port) = if let Some(rest) = t.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or_else(|| format!("'{t}': unclosed [ in an IPv6 address"))?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return Err(format!("'{t}': expected [ipv6]:port")),
        };
        (h.to_string(), port)
    } else if t.matches(':').count() > 1 {
        // A bare IPv6 address (no port).
        (t.to_string(), None)
    } else {
        match t.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (t.to_string(), None),
        }
    };
    let port = match port {
        None => 443,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 && !p.starts_with('0') && p.bytes().all(|b| b.is_ascii_digit()) => n,
            _ => return Err(format!("'{t}': the port must be 1-65535")),
        },
    };
    let wild = host_part.starts_with("*.");
    let bare = if wild { &host_part[2..] } else { host_part.as_str() };
    if bare.contains('*') {
        return Err(format!("'{t}': a wildcard is only a leading '*.' (like *.s3.amazonaws.com)"));
    }
    let name = normalize_name(bare).ok_or_else(|| format!("'{t}': not a host name or IP address (use the xn-- form for non-ASCII names)"))?;
    let is_ip = name.parse::<std::net::IpAddr>().is_ok();
    if wild && is_ip {
        return Err(format!("'{t}': an IP address can't be a wildcard"));
    }
    if !is_ip {
        if local_name(&name) {
            return Err(format!("'{t}': local names never leave the box"));
        }
        if wild && name.split('.').count() < 2 {
            return Err(format!("'{t}': a wildcard needs at least two labels after '*.'"));
        }
    }
    Ok(HostSpec { host: if wild { format!("*.{name}") } else { name }, port })
}

/// Parse a list; duplicates folded, order kept.
pub fn parse_specs(raw: &[String]) -> Result<Vec<HostSpec>, String> {
    if raw.len() > MAX_HOSTS_PER_REQUEST {
        return Err(format!("at most {MAX_HOSTS_PER_REQUEST} hosts at a time"));
    }
    let mut out: Vec<HostSpec> = Vec::new();
    for r in raw {
        let s = parse_spec(r)?;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

// ── policy ───────────────────────────────────────────────────────────

/// One App's policy as the proxy holds it in memory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Policy {
    pub broad: bool,
    pub allowed: Vec<HostSpec>,
}

/// What the policy says about one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// On the allowlist (the matching spec).
    Listed(HostSpec),
    /// Not listed, but the App is `broad`.
    Broad,
    /// Not listed on an allowlist App.
    NotListed,
}

impl Policy {
    pub fn verdict(&self, host: &str, port: u16) -> Verdict {
        if let Some(s) = self.allowed.iter().find(|s| s.matches(host, port)) {
            return Verdict::Listed(s.clone());
        }
        if self.broad {
            Verdict::Broad
        } else {
            Verdict::NotListed
        }
    }
}

pub fn mode(conn: &Connection, app: &str) -> rusqlite::Result<String> {
    Ok(conn
        .query_row("SELECT mode FROM app_egress_policy WHERE app_id = ?1", params![app], |r| r.get::<_, String>(0))
        .optional()?
        .unwrap_or_else(|| MODE_ALLOWLIST.to_string()))
}

pub fn set_mode(conn: &Connection, app: &str, mode: &str, by: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_egress_policy (app_id, mode, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(app_id) DO UPDATE SET mode = ?2, updated_by = ?3, updated_at = ?4",
        params![app, mode, by, super::now()],
    )?;
    Ok(())
}

pub fn policy(conn: &Connection, app: &str) -> rusqlite::Result<Policy> {
    let broad = mode(conn, app)? == MODE_BROAD;
    let mut st = conn.prepare("SELECT host, port FROM app_egress_hosts WHERE app_id = ?1 AND status = 'allowed' ORDER BY host, port")?;
    let allowed = st
        .query_map(params![app], |r| Ok(HostSpec { host: r.get(0)?, port: r.get::<_, i64>(1)? as u16 }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Policy { broad, allowed })
}

// ── host rows ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRow {
    pub host: String,
    pub port: u16,
    pub source: String,
    pub status: String,
    pub request_id: Option<String>,
    pub added_by: Option<String>,
    pub added_at: i64,
    pub first_seen_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub hits: i64,
    pub refused: i64,
}

impl HostRow {
    pub fn spec(&self) -> HostSpec {
        HostSpec { host: self.host.clone(), port: self.port }
    }
}

fn host_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HostRow> {
    Ok(HostRow {
        host: r.get(0)?,
        port: r.get::<_, i64>(1)? as u16,
        source: r.get(2)?,
        status: r.get(3)?,
        request_id: r.get(4)?,
        added_by: r.get(5)?,
        added_at: r.get(6)?,
        first_seen_at: r.get(7)?,
        last_seen_at: r.get(8)?,
        hits: r.get(9)?,
        refused: r.get(10)?,
    })
}

const HOST_COLS: &str = "host, port, source, status, request_id, added_by, added_at, first_seen_at, last_seen_at, hits, refused";

pub fn list_hosts(conn: &Connection, app: &str) -> rusqlite::Result<Vec<HostRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {HOST_COLS} FROM app_egress_hosts WHERE app_id = ?1 \
         ORDER BY CASE status WHEN 'allowed' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END, host, port"
    ))?;
    let rows = st.query_map(params![app], host_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_host(conn: &Connection, app: &str, spec: &HostSpec) -> rusqlite::Result<Option<HostRow>> {
    conn.query_row(
        &format!("SELECT {HOST_COLS} FROM app_egress_hosts WHERE app_id = ?1 AND host = ?2 AND port = ?3"),
        params![app, spec.host, spec.port as i64],
        host_row,
    )
    .optional()
}

fn count_status(conn: &Connection, app: &str, statuses: &[&str]) -> rusqlite::Result<i64> {
    let q = format!(
        "SELECT COUNT(*) FROM app_egress_hosts WHERE app_id = ?1 AND status IN ({})",
        statuses.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",")
    );
    conn.query_row(&q, params![app], |r| r.get(0))
}

/// Which of `specs` are already allowed (exactly, or under an allowed
/// wildcard on the same port)?
pub fn already_allowed(conn: &Connection, app: &str, specs: &[HostSpec]) -> rusqlite::Result<Vec<HostSpec>> {
    let p = policy(conn, app)?;
    Ok(specs
        .iter()
        .filter(|s| p.allowed.contains(s) || (!s.is_wildcard() && matches!(p.verdict(&s.host, s.port), Verdict::Listed(_))))
        .cloned()
        .collect())
}

/// Mark `specs` pending for `request_id` (rows that are `seen` become
/// pending; allowed rows are left alone).
pub fn mark_pending(conn: &Connection, app: &str, specs: &[HostSpec], source: &str, request_id: &str, by: &str) -> rusqlite::Result<()> {
    let now = super::now();
    for s in specs {
        conn.execute(
            "INSERT INTO app_egress_hosts (app_id, host, port, source, status, request_id, added_by, added_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7) \
             ON CONFLICT(app_id, host, port) DO UPDATE SET status = 'pending', source = ?4, request_id = ?5, added_by = ?6, added_at = ?7 \
             WHERE status != 'allowed'",
            params![app, s.host, s.port as i64, source, request_id, by, now],
        )?;
    }
    Ok(())
}

/// Allow `specs` (the owner approved them).
pub fn allow(conn: &Connection, app: &str, specs: &[HostSpec], source: &str, by: &str) -> rusqlite::Result<()> {
    let now = super::now();
    for s in specs {
        conn.execute(
            "INSERT INTO app_egress_hosts (app_id, host, port, source, status, request_id, added_by, added_at) \
             VALUES (?1, ?2, ?3, ?4, 'allowed', NULL, ?5, ?6) \
             ON CONFLICT(app_id, host, port) DO UPDATE SET status = 'allowed', request_id = NULL, added_by = ?5, added_at = ?6",
            params![app, s.host, s.port as i64, source, by, now],
        )?;
    }
    Ok(())
}

/// Narrowing: take `spec` off the list (allowed or pending). A row with
/// traffic stays as `seen` (its counters are history); one without goes.
pub fn remove(conn: &Connection, app: &str, spec: &HostSpec) -> rusqlite::Result<bool> {
    let Some(row) = get_host(conn, app, spec)? else { return Ok(false) };
    if row.status == STATUS_SEEN {
        return Ok(false);
    }
    revert_row(conn, app, &row)?;
    Ok(true)
}

fn revert_row(conn: &Connection, app: &str, row: &HostRow) -> rusqlite::Result<()> {
    if row.hits > 0 || row.refused > 0 {
        conn.execute(
            "UPDATE app_egress_hosts SET status = 'seen', request_id = NULL WHERE app_id = ?1 AND host = ?2 AND port = ?3",
            params![app, row.host, row.port as i64],
        )?;
    } else {
        conn.execute(
            "DELETE FROM app_egress_hosts WHERE app_id = ?1 AND host = ?2 AND port = ?3",
            params![app, row.host, row.port as i64],
        )?;
    }
    Ok(())
}

/// Release the pending rows a closed request held.
fn release_pending(conn: &Connection, app: &str, request_id: &str) -> rusqlite::Result<()> {
    let rows: Vec<HostRow> = list_hosts(conn, app)?
        .into_iter()
        .filter(|r| r.status == STATUS_PENDING && r.request_id.as_deref() == Some(request_id))
        .collect();
    for r in rows {
        revert_row(conn, app, &r)?;
    }
    Ok(())
}

/// One proxy decision, recorded: the counters, and a `seen` row the first
/// time a host that isn't listed shows up. Returns true for a first-seen
/// host (the caller audits it). Exact host only (never a wildcard row).
pub fn record(conn: &Connection, app: &str, host: &str, port: u16, let_through: bool, at: i64) -> rusqlite::Result<bool> {
    let (hit, refused) = if let_through { (1i64, 0i64) } else { (0, 1) };
    let n = conn.execute(
        "UPDATE app_egress_hosts SET hits = hits + ?4, refused = refused + ?5, last_seen_at = ?6, \
         first_seen_at = COALESCE(first_seen_at, ?6) WHERE app_id = ?1 AND host = ?2 AND port = ?3",
        params![app, host, port as i64, hit, refused, at],
    )?;
    if n > 0 {
        return Ok(false);
    }
    if count_status(conn, app, &[STATUS_SEEN])? >= MAX_SEEN {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO app_egress_hosts (app_id, host, port, source, status, added_at, first_seen_at, last_seen_at, hits, refused) \
         VALUES (?1, ?2, ?3, 'first_seen', 'seen', ?4, ?4, ?4, ?5, ?6)",
        params![app, host, port as i64, at, hit, refused],
    )?;
    Ok(true)
}

/// Credit a wildcard rule's traffic to the rule row too.
pub fn record_rule_hit(conn: &Connection, app: &str, rule: &HostSpec, at: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE app_egress_hosts SET hits = hits + 1, last_seen_at = ?4, first_seen_at = COALESCE(first_seen_at, ?4) \
         WHERE app_id = ?1 AND host = ?2 AND port = ?3",
        params![app, rule.host, rule.port as i64, at],
    )?;
    Ok(())
}

// ── widening requests ────────────────────────────────────────────────
//
// Egress requests live in the one held-request table (`app_widenings`,
// `app_grace::requests`), next to the 0.46.0 grace kinds: one list, one
// approve (owner, in person), one deny. These helpers keep the host list
// in step with them.

pub use crate::app_grace::requests::AppRequest as Widening;
use crate::app_grace::requests as reqs;

impl reqs::AppRequest {
    /// The hosts an `egress` request adds.
    pub fn hosts(&self) -> Vec<HostSpec> {
        self.diff["egress"]["add"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str()).filter_map(|x| parse_spec(x).ok()).collect())
            .unwrap_or_default()
    }

    /// The mode an `egress_mode` request sets.
    pub fn mode(&self) -> Option<&str> {
        self.diff["egressMode"].as_str()
    }
}

/// An egress request on a runtime App (its service, label and workspace
/// come from the App's row).
pub struct NewWidening<'a> {
    pub app_id: &'a str,
    pub kind: &'a str,
    pub diff: J,
    pub summary: String,
    pub reason: Option<&'a str>,
    pub deployment_id: Option<&'a str>,
    pub requested_by: &'a str,
    pub requester_workspace: Option<&'a str>,
    pub requester_session: Option<&'a str>,
}

pub fn new_request_id() -> String {
    reqs::new_id()
}

pub fn file_request(conn: &Connection, w: &NewWidening<'_>, now: i64) -> rusqlite::Result<Widening> {
    let app = super::store::get_app(conn, w.app_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    reqs::insert(
        conn,
        &reqs::NewRequest {
            kind: w.kind,
            app_id: Some(w.app_id),
            service_id: Some(&app.service_id),
            app_label: w.app_id,
            workspace_id: &app.owner_workspace,
            body: J::Object(Default::default()),
            diff: w.diff.clone(),
            summary: w.summary.clone(),
            reasons: vec!["always per event: a fresh owner confirm, even from the owner token".into()],
            reason: w.reason,
            deployment_id: w.deployment_id,
            requested_by: w.requested_by,
            requester_workspace: w.requester_workspace,
            requester_session: w.requester_session,
        },
        now,
    )
}

pub fn get_request(conn: &Connection, id: &str) -> rusqlite::Result<Option<Widening>> {
    reqs::get(conn, id)
}

/// A request by id or a unique id prefix (at least 6 characters).
pub fn find_request(conn: &Connection, token: &str) -> rusqlite::Result<Option<Widening>> {
    reqs::find(conn, token)
}

/// Requests, newest first. `app` / `status` filter; `workspace` keeps
/// only requests on Apps that workspace owns.
pub fn list_requests(conn: &Connection, app: Option<&str>, status: Option<&str>, workspace: Option<&str>, limit: i64) -> rusqlite::Result<Vec<Widening>> {
    reqs::list(conn, &reqs::Filter { app_id: app, service_id: None, workspace, status }, limit)
}

/// The pending `egress` request on an App, if any (new hosts join it).
pub fn pending_egress_request(conn: &Connection, app: &str) -> rusqlite::Result<Option<Widening>> {
    reqs::pending_of_kind(conn, app, KIND_EGRESS)
}

/// Who asks for hosts, and why.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    /// `manifest` (a deploy) or `request` (`k2 app egress add`).
    pub source: &'a str,
    pub reason: Option<&'a str>,
    pub deployment_id: Option<&'a str>,
    /// The audit actor (`agent:<ws>`, `owner`, `user:<name>`).
    pub by: &'a str,
    pub workspace: Option<&'a str>,
    pub session: Option<&'a str>,
}

/// Ask for `specs` on `app`: the ones not allowed and not already
/// pending go into a held request (a new one, or added to the App's open
/// one when the same requester filed it). Returns the request (None when
/// nothing new was asked for) and the specs that were already allowed.
pub fn request_hosts(conn: &Connection, app: &str, specs: &[HostSpec], ask: &Ask<'_>, now: i64) -> Result<(Option<Widening>, Vec<HostSpec>), String> {
    let Ask { source, reason, deployment_id, by, workspace, session } = *ask;
    let allowed = already_allowed(conn, app, specs).map_err(|e| e.to_string())?;
    let want: Vec<HostSpec> = specs.iter().filter(|s| !allowed.contains(s)).cloned().collect();
    if want.is_empty() {
        return Ok((None, allowed));
    }
    let listed = count_status(conn, app, &[STATUS_ALLOWED, STATUS_PENDING]).map_err(|e| e.to_string())?;
    if listed + want.len() as i64 > MAX_LISTED {
        return Err(format!("an App lists at most {MAX_LISTED} hosts (allowed + pending); remove some first"));
    }
    let open = pending_egress_request(conn, app).map_err(|e| e.to_string())?.filter(|w| w.requested_by == by && w.expires_at > now);
    let w = match open {
        Some(w) => {
            let mut hosts = w.hosts();
            for s in &want {
                if !hosts.contains(s) {
                    hosts.push(s.clone());
                }
            }
            if hosts.len() == w.hosts().len() {
                return Ok((Some(w), allowed));
            }
            let diff = serde_json::json!({ "egress": { "add": hosts.iter().map(HostSpec::display).collect::<Vec<_>>() } });
            reqs::update_diff(conn, &w.id, &diff, &summary_for_hosts(app, &hosts), reason).map_err(|e| e.to_string())?;
            get_request(conn, &w.id).map_err(|e| e.to_string())?.expect("exists")
        }
        None => file_request(
            conn,
            &NewWidening {
                app_id: app,
                kind: KIND_EGRESS,
                diff: serde_json::json!({ "egress": { "add": want.iter().map(HostSpec::display).collect::<Vec<_>>() } }),
                summary: summary_for_hosts(app, &want),
                reason,
                deployment_id,
                requested_by: by,
                requester_workspace: workspace,
                requester_session: session,
            },
            now,
        )
        .map_err(|e| e.to_string())?,
    };
    mark_pending(conn, app, &want, source, &w.id, by).map_err(|e| e.to_string())?;
    Ok((Some(w), allowed))
}

pub fn summary_for_hosts(app: &str, hosts: &[HostSpec]) -> String {
    let list: Vec<String> = hosts.iter().map(HostSpec::display).collect();
    let shown = if list.len() > 6 { format!("{} and {} more", list[..6].join(", "), list.len() - 6) } else { list.join(", ") };
    format!("let App {app} reach {shown}")
}

/// Close a pending request. `Err` codes: `request_not_found`,
/// `not_pending`, `request_expired`.
pub fn decide(conn: &Connection, id: &str, status: &str, by: &str, via: Option<&str>, reason: Option<&str>, now: i64) -> Result<Widening, &'static str> {
    reqs::decide(conn, id, status, by, via, reason, now)
}

/// What closing an egress request does to the host list (called by
/// `app_grace::requests` for every close: approve, deny, withdraw,
/// expire). Approving an `egress` request allows its pending hosts; any
/// other close releases them. Approving `egress_mode` sets the mode.
pub fn on_closed(conn: &Connection, w: &Widening, status: &str, by: &str) -> rusqlite::Result<()> {
    let Some(app) = w.app_id.as_deref() else { return Ok(()) };
    if w.kind == KIND_EGRESS {
        if status == REQ_APPROVED {
            let hosts: Vec<HostSpec> = list_hosts(conn, app)?
                .into_iter()
                .filter(|r| r.status == STATUS_PENDING && r.request_id.as_deref() == Some(w.id.as_str()))
                .map(|r| r.spec())
                .collect();
            for h in hosts {
                let src = get_host(conn, app, &h).ok().flatten().map(|r| r.source).unwrap_or_else(|| SOURCE_REQUEST.into());
                allow(conn, app, std::slice::from_ref(&h), &src, by)?;
            }
        } else {
            release_pending(conn, app, &w.id)?;
        }
    }
    if w.kind == KIND_EGRESS_MODE && status == REQ_APPROVED {
        if let Some(m) = w.mode() {
            set_mode(conn, app, m, by)?;
        }
    }
    Ok(())
}

/// Expire every pending request past its 7 days; returns them.
pub fn expire_due(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Widening>> {
    reqs::expire_due(conn, now)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::store;

    fn conn() -> Connection {
        let c = store::tests::conn();
        // 0168's held-request table (its ALTERs need the full schema), then
        // 0169's egress tables.
        for stmt in include_str!("../../drizzle_sql/0168_app_grace.sql").split("--> statement-breakpoint") {
            if stmt.contains("CREATE TABLE IF NOT EXISTS app_widenings") || stmt.contains("ON app_widenings") {
                c.execute_batch(stmt).unwrap();
            }
        }
        c.execute_batch(include_str!("../../drizzle_sql/0169_app_egress.sql").replace("--> statement-breakpoint", "").as_str())
            .unwrap();
        c.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (app, ws) in [("jobs", "ws-a"), ("other", "ws-b")] {
            store::insert_app(
                &c,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: "container",
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        c
    }

    fn spec(s: &str) -> HostSpec {
        parse_spec(s).unwrap()
    }

    #[test]
    fn specs_parse_and_normalize() {
        assert_eq!(spec("API.Stripe.com"), HostSpec { host: "api.stripe.com".into(), port: 443 });
        assert_eq!(spec("api.stripe.com.:8443").port, 8443);
        assert_eq!(spec("api.stripe.com.:8443").host, "api.stripe.com");
        assert_eq!(spec("*.s3.amazonaws.com").host, "*.s3.amazonaws.com");
        assert_eq!(spec("1.2.3.4:80"), HostSpec { host: "1.2.3.4".into(), port: 80 });
        assert_eq!(spec("[2001:db8::1]:443").host, "2001:db8::1");
        assert_eq!(spec("[2001:db8::1]:443").display(), "[2001:db8::1]:443");
        assert_eq!(spec("2001:db8::1").port, 443);
        assert_eq!(spec("xn--bcher-kva.example").host, "xn--bcher-kva.example");
        for bad in [
            "",
            "https://api.stripe.com",
            "api.stripe.com/v1",
            "user@api.stripe.com",
            "api.stripe.com:0",
            "api.stripe.com:65536",
            "api.stripe.com:080",
            "api.stripe.com:x",
            "localhost",
            "localhost:8080",
            "db.local",
            "metadata.google.internal",
            "printer",
            "*.com",
            "*",
            "a.*.example.com",
            "*.1.2.3.4",
            "bücher.example",
            "-bad.example.com",
            "under_score.example.com",
            "[::1",
            "[::1]x",
        ] {
            assert!(parse_spec(bad).is_err(), "{bad:?} parsed");
        }
        let e = parse_spec("https://x.example.com").unwrap_err();
        assert!(e.contains("not a URL"), "{e}");
        assert_eq!(parse_specs(&["a.example.com".into(), "A.example.com:443".into()]).unwrap().len(), 1);
        assert!(parse_specs(&vec!["a.example.com".to_string(); MAX_HOSTS_PER_REQUEST + 1]).is_err());
    }

    #[test]
    fn wildcards_match_names_under_the_suffix_only() {
        let w = spec("*.s3.amazonaws.com");
        assert!(w.matches("bucket.s3.amazonaws.com", 443));
        assert!(w.matches("a.b.s3.amazonaws.com", 443));
        assert!(!w.matches("s3.amazonaws.com", 443), "never the suffix itself");
        assert!(!w.matches("evils3.amazonaws.com", 443), "a label boundary");
        assert!(!w.matches("bucket.s3.amazonaws.com", 80), "same port only");
        assert!(spec("api.example.com").matches("api.example.com", 443));
        assert!(!spec("api.example.com").matches("x.api.example.com", 443));
        assert_eq!(normalize_name("API.Example.COM."), Some("api.example.com".into()));
        assert_eq!(normalize_name("[::1]"), Some("::1".into()));
        assert_eq!(normalize_name("bad_name.com"), None);
        assert!(local_name("x.localhost") && local_name("printer") && !local_name("api.example.com"));
    }

    #[test]
    fn verdicts_follow_mode_and_list() {
        let c = conn();
        let p = policy(&c, "jobs").unwrap();
        assert_eq!(p, Policy::default());
        assert_eq!(p.verdict("api.stripe.com", 443), Verdict::NotListed);
        allow(&c, "jobs", &[spec("api.stripe.com"), spec("*.amazonaws.com")], SOURCE_REQUEST, "owner").unwrap();
        let p = policy(&c, "jobs").unwrap();
        assert_eq!(p.verdict("api.stripe.com", 443), Verdict::Listed(spec("api.stripe.com")));
        assert_eq!(p.verdict("s3.amazonaws.com", 443), Verdict::Listed(spec("*.amazonaws.com")));
        assert_eq!(p.verdict("api.stripe.com", 80), Verdict::NotListed);
        assert_eq!(policy(&c, "other").unwrap().verdict("api.stripe.com", 443), Verdict::NotListed, "per App");
        set_mode(&c, "jobs", MODE_BROAD, "owner").unwrap();
        assert_eq!(policy(&c, "jobs").unwrap().verdict("example.org", 443), Verdict::Broad);
        set_mode(&c, "jobs", MODE_ALLOWLIST, "agent:x").unwrap();
        assert_eq!(policy(&c, "jobs").unwrap().verdict("example.org", 443), Verdict::NotListed);
    }

    #[test]
    fn a_request_holds_until_approved_and_narrowing_is_immediate() {
        let c = conn();
        let now = 1_000;
        let (w, already) = request_hosts(&c, "jobs", &[spec("api.stripe.com"), spec("hooks.slack.com")], &Ask { source: SOURCE_REQUEST, reason: Some("payments"), deployment_id: None, by: "agent:jobs-ws", workspace: Some("ws-a"), session: Some("sess-1") }, now)
        .unwrap();
        let w = w.unwrap();
        assert!(already.is_empty());
        assert!(w.id.starts_with("w_") && w.id.len() == 14);
        assert_eq!(w.status, REQ_PENDING);
        assert_eq!(w.expires_at, now + REQUEST_TTL_SECS);
        assert_eq!(w.hosts(), vec![spec("api.stripe.com"), spec("hooks.slack.com")]);
        assert_eq!(w.summary, "let App jobs reach api.stripe.com:443, hooks.slack.com:443");
        // Pending is not allowed.
        assert_eq!(policy(&c, "jobs").unwrap().verdict("api.stripe.com", 443), Verdict::NotListed);
        assert_eq!(get_host(&c, "jobs", &spec("api.stripe.com")).unwrap().unwrap().status, STATUS_PENDING);
        // The same requester asking again joins the open request.
        let (w2, _) = request_hosts(&c, "jobs", &[spec("api.github.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:jobs-ws", workspace: Some("ws-a"), session: None }, now + 5).unwrap();
        assert_eq!(w2.as_ref().unwrap().id, w.id);
        assert_eq!(w2.unwrap().hosts().len(), 3);
        // Approve: all three allowed.
        let d = decide(&c, &w.id, REQ_APPROVED, "owner", Some("local-auth"), None, now + 10).unwrap();
        assert_eq!(d.status, REQ_APPROVED);
        assert_eq!(d.confirm_via.as_deref(), Some("local-auth"));
        let p = policy(&c, "jobs").unwrap();
        assert_eq!(p.allowed.len(), 3);
        assert_eq!(decide(&c, &w.id, REQ_DENIED, "owner", None, None, now + 11).unwrap_err(), "not_pending");
        // Asking for an allowed host files nothing.
        let (none, already) = request_hosts(&c, "jobs", &[spec("api.stripe.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:jobs-ws", workspace: None, session: None }, now).unwrap();
        assert!(none.is_none());
        assert_eq!(already, vec![spec("api.stripe.com")]);
        // Narrowing: gone at once.
        assert!(remove(&c, "jobs", &spec("api.stripe.com")).unwrap());
        assert_eq!(policy(&c, "jobs").unwrap().verdict("api.stripe.com", 443), Verdict::NotListed);
        assert!(!remove(&c, "jobs", &spec("api.stripe.com")).unwrap(), "already gone");
    }

    #[test]
    fn deny_withdraw_and_expiry_release_pending_rows() {
        let c = conn();
        let (w, _) = request_hosts(&c, "jobs", &[spec("a.example.com")], &Ask { source: SOURCE_MANIFEST, reason: None, deployment_id: Some("d-0123456789ab"), by: "agent:x", workspace: Some("ws-a"), session: None }, 100).unwrap();
        let w = w.unwrap();
        assert_eq!(w.deployment_id.as_deref(), Some("d-0123456789ab"));
        assert_eq!(get_host(&c, "jobs", &spec("a.example.com")).unwrap().unwrap().source, SOURCE_MANIFEST);
        decide(&c, &w.id, REQ_DENIED, "owner", None, Some("not needed"), 101).unwrap();
        assert!(get_host(&c, "jobs", &spec("a.example.com")).unwrap().is_none(), "no traffic: row gone");

        // A host with traffic stays as seen after a withdraw.
        assert!(record(&c, "jobs", "b.example.com", 443, false, 150).unwrap(), "first seen");
        let (w, _) = request_hosts(&c, "jobs", &[spec("b.example.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:x", workspace: Some("ws-a"), session: None }, 200).unwrap();
        let w = w.unwrap();
        assert_eq!(get_host(&c, "jobs", &spec("b.example.com")).unwrap().unwrap().status, STATUS_PENDING);
        decide(&c, &w.id, REQ_WITHDRAWN, "agent:x", None, None, 201).unwrap();
        let r = get_host(&c, "jobs", &spec("b.example.com")).unwrap().unwrap();
        assert_eq!((r.status.as_str(), r.refused), (STATUS_SEEN, 1));

        // Expiry after 7 days; approving an expired request fails.
        let (w, _) = request_hosts(&c, "jobs", &[spec("c.example.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:x", workspace: Some("ws-a"), session: None }, 300).unwrap();
        let w = w.unwrap();
        assert_eq!(decide(&c, &w.id, REQ_APPROVED, "owner", Some("x"), None, 300 + REQUEST_TTL_SECS).unwrap_err(), "request_expired");
        assert_eq!(get_request(&c, &w.id).unwrap().unwrap().status, REQ_EXPIRED);
        assert!(get_host(&c, "jobs", &spec("c.example.com")).unwrap().is_none());
        let (w, _) = request_hosts(&c, "jobs", &[spec("d.example.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:y", workspace: Some("ws-a"), session: None }, 400).unwrap();
        let gone = expire_due(&c, 400 + REQUEST_TTL_SECS).unwrap();
        assert_eq!(gone.iter().map(|g| g.id.clone()).collect::<Vec<_>>(), vec![w.unwrap().id]);
        assert!(expire_due(&c, 400 + REQUEST_TTL_SECS).unwrap().is_empty());
    }

    #[test]
    fn requests_list_by_app_status_and_workspace() {
        let c = conn();
        request_hosts(&c, "jobs", &[spec("a.example.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:a", workspace: Some("ws-a"), session: None }, 1).unwrap();
        request_hosts(&c, "other", &[spec("b.example.com")], &Ask { source: SOURCE_REQUEST, reason: None, deployment_id: None, by: "agent:b", workspace: Some("ws-b"), session: None }, 2).unwrap();
        assert_eq!(list_requests(&c, None, None, None, 50).unwrap().len(), 2);
        assert_eq!(list_requests(&c, Some("jobs"), None, None, 50).unwrap().len(), 1);
        let mine = list_requests(&c, None, Some(REQ_PENDING), Some("ws-b"), 50).unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].app_id.as_deref(), Some("other"));
        assert_eq!((mine[0].workspace_id.as_str(), mine[0].service_id.as_deref()), ("ws-b", Some("svc-other")), "the App row fills its workspace and service");
        let id = &mine[0].id;
        assert_eq!(find_request(&c, &id[..8]).unwrap().unwrap().id, *id);
        assert!(find_request(&c, "w_").unwrap().is_none(), "too short a prefix");
        // A broad-mode request applies the mode on approve.
        let w = file_request(
            &c,
            &NewWidening {
                app_id: "jobs",
                kind: KIND_EGRESS_MODE,
                diff: serde_json::json!({"egressMode": "broad"}),
                summary: "broad".into(),
                reason: None,
                deployment_id: None,
                requested_by: "agent:a",
                requester_workspace: Some("ws-a"),
                requester_session: None,
            },
            10,
        )
        .unwrap();
        decide(&c, &w.id, REQ_APPROVED, "owner", Some("vault-password"), None, 11).unwrap();
        assert_eq!(mode(&c, "jobs").unwrap(), MODE_BROAD);
    }

    #[test]
    fn seen_rows_count_and_are_capped() {
        let c = conn();
        assert!(record(&c, "jobs", "x.example.com", 443, false, 10).unwrap());
        assert!(!record(&c, "jobs", "x.example.com", 443, false, 11).unwrap(), "only the first time");
        let r = get_host(&c, "jobs", &spec("x.example.com")).unwrap().unwrap();
        assert_eq!((r.refused, r.hits, r.first_seen_at, r.last_seen_at), (2, 0, Some(10), Some(11)));
        assert_eq!(r.source, SOURCE_FIRST_SEEN);
        allow(&c, "jobs", &[spec("*.example.org")], SOURCE_REQUEST, "owner").unwrap();
        record_rule_hit(&c, "jobs", &spec("*.example.org"), 12).unwrap();
        assert_eq!(get_host(&c, "jobs", &spec("*.example.org")).unwrap().unwrap().hits, 1);
        for i in 0..(MAX_SEEN + 5) {
            record(&c, "other", &format!("h{i}.example.com"), 443, false, 20).unwrap();
        }
        assert_eq!(count_status(&c, "other", &[STATUS_SEEN]).unwrap(), MAX_SEEN);
        // Uninstall drops everything (FK cascade).
        store::delete_app(&c, "other").unwrap();
        assert_eq!(c.query_row("SELECT COUNT(*) FROM app_egress_hosts WHERE app_id = 'other'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
}
