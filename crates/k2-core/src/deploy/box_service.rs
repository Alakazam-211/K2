//! App box services (0.46.0): the pure and SQLite parts of an
//! owner-approved forward into a walled App for a service on this box's
//! loopback. The forward itself is the guarded engine's (pasta `-T`, like
//! the gateway link) and the door is the root helper's per-App nft rule;
//! the daemon (`k2-daemon` `apps::box_service_routes`) checks the
//! deny-list (`k2_root_proto::apps::box_port_refusal` plus its own
//! listeners) before anything lands here.
//!
//! - **One row per App and name** (`app_box_services`, 0171): `name`
//!   (`sew-api`), `host_port` (the host loopback port), `app_port` (the
//!   port inside the wall; the host port by default), `proto` (`tcp`).
//! - **Widening waits; narrowing doesn't.** A new service is a held
//!   `box_service` request in `app_widenings` until the owner approves it
//!   with a fresh owner confirm (§5.6, always per event). Revoking applies
//!   at once.
//! - **Presets.** Known services (`sew-api`, `sew-signal`) resolve their
//!   port from Sew's own config when it is installed, so a non-default
//!   port still works; the owner still approves the resolved port.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value as J;

use crate::app_grace::requests as reqs;

pub const KIND_BOX_SERVICE: &str = reqs::KIND_BOX_SERVICE;

pub const STATUS_ALLOWED: &str = "allowed";
pub const STATUS_PENDING: &str = "pending";

pub const SOURCE_MANIFEST: &str = "manifest";
pub const SOURCE_REQUEST: &str = "request";

pub const PROTO_TCP: &str = "tcp";

/// Most box services one App holds (allowed + pending); the engine and the
/// root helper hold the same cap (`k2_root_proto::apps::MAX_BOX_SERVICES`).
pub const MAX_PER_APP: usize = 8;

/// The in-wall port that is never a box service's (the gateway link).
pub const IN_WALL_GATEWAY_PORT: u16 = 8730;
/// K2's App range on host loopback (gateway links and egress proxies);
/// never an in-wall port either (`k2_root_proto::apps::RESERVED_PORTS`).
pub const APP_RANGE: (u16, u16) = (38_600, 38_999);

// ── names and presets ────────────────────────────────────────────────

/// `^[a-z][a-z0-9-]{0,31}$`, no `--`, no trailing `-`.
pub fn name_ok(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !n.contains("--")
        && !n.ends_with('-')
}

/// A port inside the wall: ≥ 1024, not the gateway link's 8730, outside
/// K2's App range.
pub fn app_port_ok(p: u16) -> bool {
    p >= 1024 && p != IN_WALL_GATEWAY_PORT && !(APP_RANGE.0..=APP_RANGE.1).contains(&p)
}

/// A known service on this box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Preset {
    pub name: &'static str,
    pub default_port: u16,
    pub what: &'static str,
    /// Where its port lives in Sew's `sew.toml`: (section, key).
    #[serde(skip)]
    pub sew_key: (&'static str, &'static str),
}

/// Sew's config (written by `sew install`; K2 never edits it).
pub const SEW_CONFIG: &str = "/etc/sew/sew.toml";

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "sew-api",
        default_port: 8470,
        what: "Sew's client API (HTTP and the /api/v1/gateway websocket)",
        sew_key: ("server", "listen"),
    },
    Preset {
        name: "sew-signal",
        default_port: 7880,
        what: "Sew's voice signalling (HTTP and the /rtc websocket)",
        sew_key: ("voice.ops", "signal_listen"),
    },
];

pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// A loopback `host:port` (`127.0.0.1:N`, `localhost:N`, `[::1]:N`) →
/// its port. Anything else (a public bind, a socket path) → None.
fn loopback_port(v: &str) -> Option<u16> {
    let v = v.trim();
    let (host, port) = v.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    port.parse::<u16>().ok().filter(|p| *p >= 1024)
}

/// The value of `key` under `[section]` in a small TOML file (a plain
/// line scan: `key = "value"`; comments and other sections skipped).
fn toml_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut cur = String::new();
    for raw in text.lines() {
        let l = raw.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if let Some(s) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            cur = s.trim().to_string();
            continue;
        }
        if cur != section {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        if k.trim() == key {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Where a preset's port came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub port: u16,
    /// `sew.toml` (read from Sew's config) or `default`.
    pub from: String,
}

/// A preset's host port: from Sew's config text when it names a loopback
/// address, else the preset's default. `Err` when Sew's config binds the
/// service off loopback (a box service is loopback only).
pub fn resolve_preset(p: &Preset, sew_toml: Option<&str>) -> Result<Resolved, String> {
    if let Some(text) = sew_toml {
        if let Some(v) = toml_value(text, p.sew_key.0, p.sew_key.1) {
            return match loopback_port(&v) {
                Some(port) => Ok(Resolved { port, from: SEW_CONFIG.into() }),
                None => Err(format!(
                    "{} listens on '{v}' (Sew's [{}] {}), not on this box's loopback; a box service forwards to 127.0.0.1 only",
                    p.name, p.sew_key.0, p.sew_key.1
                )),
            };
        }
    }
    Ok(Resolved { port: p.default_port, from: "default".into() })
}

// ── rows ─────────────────────────────────────────────────────────────

/// What a caller (a manifest or `k2 app box-service request`) asks for,
/// already resolved and checked by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Want {
    pub name: String,
    pub host_port: u16,
    pub app_port: u16,
    #[serde(default = "tcp")]
    pub proto: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

fn tcp() -> String {
    PROTO_TCP.into()
}

impl Want {
    /// `name → 127.0.0.1:<host>` (`, inside the wall :<app>` when remapped).
    pub fn display(&self) -> String {
        if self.app_port == self.host_port {
            format!("{} (127.0.0.1:{})", self.name, self.host_port)
        } else {
            format!("{} (host 127.0.0.1:{}, in the wall :{})", self.name, self.host_port, self.app_port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxService {
    pub name: String,
    pub host_port: u16,
    pub app_port: u16,
    pub proto: String,
    pub preset: Option<String>,
    pub source: String,
    pub status: String,
    pub request_id: Option<String>,
    pub added_by: String,
    pub added_at: i64,
    pub approved_by: Option<String>,
    pub approved_at: Option<i64>,
}

impl BoxService {
    pub fn want(&self) -> Want {
        Want { name: self.name.clone(), host_port: self.host_port, app_port: self.app_port, proto: self.proto.clone(), preset: self.preset.clone() }
    }
    /// The environment variable the App reads: `K2_BOX_<NAME>`.
    pub fn env_name(&self) -> String {
        env_name(&self.name)
    }
}

/// `sew-api` → `K2_BOX_SEW_API`.
pub fn env_name(name: &str) -> String {
    format!("K2_BOX_{}", name.to_ascii_uppercase().replace('-', "_"))
}

const COLS: &str = "name, host_port, app_port, proto, preset, source, status, request_id, added_by, added_at, approved_by, approved_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BoxService> {
    Ok(BoxService {
        name: r.get(0)?,
        host_port: r.get::<_, i64>(1)? as u16,
        app_port: r.get::<_, i64>(2)? as u16,
        proto: r.get(3)?,
        preset: r.get(4)?,
        source: r.get(5)?,
        status: r.get(6)?,
        request_id: r.get(7)?,
        added_by: r.get(8)?,
        added_at: r.get(9)?,
        approved_by: r.get(10)?,
        approved_at: r.get(11)?,
    })
}

/// Every box service of an App (allowed first, then pending; by name).
pub fn list(conn: &Connection, app: &str) -> rusqlite::Result<Vec<BoxService>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM app_box_services WHERE app_id = ?1 ORDER BY CASE status WHEN 'allowed' THEN 0 ELSE 1 END, name"
    ))?;
    let rows = st.query_map(params![app], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, app: &str, name: &str) -> rusqlite::Result<Option<BoxService>> {
    conn.query_row(&format!("SELECT {COLS} FROM app_box_services WHERE app_id = ?1 AND name = ?2"), params![app, name], row)
        .optional()
}

/// The approved host ports (what the App's nft rule admits).
pub fn allowed_host_ports(conn: &Connection, app: &str) -> rusqlite::Result<Vec<u16>> {
    let mut out: Vec<u16> = Vec::new();
    for s in list(conn, app)? {
        if s.status == STATUS_ALLOWED && !out.contains(&s.host_port) {
            out.push(s.host_port);
        }
    }
    Ok(out)
}

/// Allowed and pending services (what a start forwards into the wall: a
/// pending one's door stays shut until the owner approves, which then
/// takes effect without a restart).
pub fn forwards(conn: &Connection, app: &str) -> rusqlite::Result<Vec<BoxService>> {
    list(conn, app)
}

/// Why `w` can't join `app`'s list (`None` = it can). Names must be new
/// or ask for the same thing again; in-wall ports must be unique; at most
/// [`MAX_PER_APP`].
pub fn conflict(conn: &Connection, app: &str, w: &Want) -> rusqlite::Result<Option<String>> {
    let rows = list(conn, app)?;
    if let Some(r) = rows.iter().find(|r| r.name == w.name) {
        if r.host_port != w.host_port || r.app_port != w.app_port {
            return Ok(Some(format!(
                "{} is already {} on App {app}; revoke it first (k2 app box-service revoke {app} {})",
                w.name,
                r.want().display(),
                w.name
            )));
        }
        return Ok(None);
    }
    if let Some(r) = rows.iter().find(|r| r.app_port == w.app_port) {
        return Ok(Some(format!("port {} inside the wall is already {}'s; pick another with --app-port", w.app_port, r.name)));
    }
    if rows.len() >= MAX_PER_APP {
        return Ok(Some(format!("an App holds at most {MAX_PER_APP} box services (allowed + pending); revoke one first")));
    }
    Ok(None)
}

// ── widening requests ────────────────────────────────────────────────

impl reqs::AppRequest {
    /// The services a `box_service` request adds.
    pub fn box_services(&self) -> Vec<Want> {
        self.diff["boxServices"]["add"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| serde_json::from_value::<Want>(x.clone()).ok()).collect())
            .unwrap_or_default()
    }
}

pub fn summary_for(app: &str, ws: &[Want]) -> String {
    let list: Vec<String> = ws.iter().map(Want::display).collect();
    format!("let App {app} reach {} on this box", list.join(", "))
}

/// Who asks, and why (mirrors `egress::Ask`).
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    pub source: &'a str,
    pub reason: Option<&'a str>,
    pub deployment_id: Option<&'a str>,
    pub by: &'a str,
    pub workspace: Option<&'a str>,
    pub session: Option<&'a str>,
}

/// Ask for `wants` on `app`: the ones not already listed go into a held
/// request (a new one, or added to the App's open one when the same
/// requester filed it) and are recorded `pending`. Returns the request
/// (None when nothing new was asked for) and the names already allowed.
/// `Err` for a conflict ([`conflict`]).
pub fn request(conn: &Connection, app: &str, wants: &[Want], ask: &Ask<'_>, now: i64) -> Result<(Option<reqs::AppRequest>, Vec<String>), String> {
    let mut new: Vec<Want> = Vec::new();
    let mut allowed: Vec<String> = Vec::new();
    for w in wants {
        if let Some(why) = conflict(conn, app, w).map_err(|e| e.to_string())? {
            return Err(why);
        }
        match get(conn, app, &w.name).map_err(|e| e.to_string())? {
            Some(r) if r.status == STATUS_ALLOWED => allowed.push(r.name),
            Some(_) => {} // already pending
            None if new.iter().any(|n| n.name == w.name || n.app_port == w.app_port) => {
                return Err(format!("{} is asked for twice (or shares port {} inside the wall)", w.name, w.app_port))
            }
            None => new.push(w.clone()),
        }
    }
    if list(conn, app).map_err(|e| e.to_string())?.len() + new.len() > MAX_PER_APP {
        return Err(format!("an App holds at most {MAX_PER_APP} box services (allowed + pending); revoke one first"));
    }
    if new.is_empty() {
        // Already pending (or allowed): point at the open request, if any.
        let open = reqs::pending_of_kind(conn, app, KIND_BOX_SERVICE).map_err(|e| e.to_string())?;
        let pending_named = wants.iter().any(|w| !allowed.contains(&w.name));
        return Ok((open.filter(|_| pending_named), allowed));
    }
    let open = reqs::pending_of_kind(conn, app, KIND_BOX_SERVICE)
        .map_err(|e| e.to_string())?
        .filter(|w| w.requested_by == ask.by && w.expires_at > now);
    let w = match open {
        Some(w) => {
            let mut all = w.box_services();
            all.extend(new.iter().cloned());
            let diff = serde_json::json!({ "boxServices": { "add": all } });
            reqs::update_diff(conn, &w.id, &diff, &summary_for(app, &all), ask.reason).map_err(|e| e.to_string())?;
            reqs::get(conn, &w.id).map_err(|e| e.to_string())?.expect("exists")
        }
        None => {
            let a = super::store::get_app(conn, app).map_err(|e| e.to_string())?.ok_or_else(|| format!("no App {app}"))?;
            reqs::insert(
                conn,
                &reqs::NewRequest {
                    kind: KIND_BOX_SERVICE,
                    app_id: Some(app),
                    service_id: Some(&a.service_id),
                    app_label: app,
                    workspace_id: &a.owner_workspace,
                    body: J::Object(Default::default()),
                    diff: serde_json::json!({ "boxServices": { "add": new } }),
                    summary: summary_for(app, &new),
                    reasons: vec![
                        "always per event: a fresh owner confirm, even from the owner token".into(),
                        "it opens a door from the App's wall to a service on this box".into(),
                    ],
                    reason: ask.reason,
                    deployment_id: ask.deployment_id,
                    requested_by: ask.by,
                    requester_workspace: ask.workspace,
                    requester_session: ask.session,
                },
                now,
            )
            .map_err(|e| e.to_string())?
        }
    };
    for n in &new {
        conn.execute(
            "INSERT INTO app_box_services (app_id, name, host_port, app_port, proto, preset, source, status, request_id, added_by, added_at) \
             VALUES (?1, ?2, ?3, ?4, 'tcp', ?5, ?6, 'pending', ?7, ?8, ?9)",
            params![app, n.name, n.host_port as i64, n.app_port as i64, n.preset, ask.source, w.id, ask.by, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok((Some(w), allowed))
}

/// Revoke one service (allowed or pending) at once. Returns the row it
/// removed. A pending request left asking for nothing is the caller's to
/// withdraw ([`request_is_empty`]).
pub fn revoke(conn: &Connection, app: &str, name: &str) -> rusqlite::Result<Option<BoxService>> {
    let Some(r) = get(conn, app, name)? else { return Ok(None) };
    conn.execute("DELETE FROM app_box_services WHERE app_id = ?1 AND name = ?2", params![app, name])?;
    Ok(Some(r))
}

/// Does a pending request still hold any pending row?
pub fn request_is_empty(conn: &Connection, app: &str, request_id: &str) -> rusqlite::Result<bool> {
    Ok(!list(conn, app)?.iter().any(|r| r.status == STATUS_PENDING && r.request_id.as_deref() == Some(request_id)))
}

/// What closing a `box_service` request does to the list (called by
/// `app_grace::requests` for every close): approving allows its pending
/// rows; any other close removes them.
pub fn on_closed(conn: &Connection, w: &reqs::AppRequest, status: &str, by: &str) -> rusqlite::Result<()> {
    let Some(app) = w.app_id.as_deref() else { return Ok(()) };
    if w.kind != KIND_BOX_SERVICE {
        return Ok(());
    }
    if status == reqs::STATUS_APPROVED {
        conn.execute(
            "UPDATE app_box_services SET status = 'allowed', request_id = NULL, approved_by = ?3, approved_at = ?4 \
             WHERE app_id = ?1 AND request_id = ?2 AND status = 'pending'",
            params![app, w.id, by, super::now()],
        )?;
    } else {
        conn.execute(
            "DELETE FROM app_box_services WHERE app_id = ?1 AND request_id = ?2 AND status = 'pending'",
            params![app, w.id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::store;

    pub(crate) fn conn() -> Connection {
        let c = store::tests::conn();
        for stmt in include_str!("../../drizzle_sql/0168_app_grace.sql").split("--> statement-breakpoint") {
            if stmt.contains("CREATE TABLE IF NOT EXISTS app_widenings") || stmt.contains("ON app_widenings") {
                c.execute_batch(stmt).unwrap();
            }
        }
        // 0171 rebuilds app_widenings with the box_service kind.
        for stmt in include_str!("../../drizzle_sql/0171_app_box_services.sql").split("--> statement-breakpoint") {
            c.execute_batch(stmt).unwrap();
        }
        c.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (app, ws) in [("hearth", "ws-a"), ("other", "ws-b")] {
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

    fn want(name: &str, host: u16, app: u16) -> Want {
        Want { name: name.into(), host_port: host, app_port: app, proto: "tcp".into(), preset: None }
    }

    fn ask<'a>(by: &'a str) -> Ask<'a> {
        Ask { source: SOURCE_REQUEST, reason: Some("Hearth talks to Sew"), deployment_id: None, by, workspace: Some("ws-a"), session: None }
    }

    #[test]
    fn names_ports_and_env() {
        assert!(name_ok("sew-api") && name_ok("x") && name_ok("a1-b2"));
        for bad in ["", "Sew", "-a", "a-", "a--b", "a_b", "9a", "a.b"] {
            assert!(!name_ok(bad), "{bad:?}");
        }
        assert!(!name_ok(&"a".repeat(33)));
        assert!(app_port_ok(8470) && app_port_ok(7880) && app_port_ok(1024));
        for bad in [80, 1023, 8730, 38_600, 38_999] {
            assert!(!app_port_ok(bad), "{bad}");
        }
        assert_eq!(env_name("sew-signal"), "K2_BOX_SEW_SIGNAL");
        assert_eq!(want("sew-api", 8470, 8470).display(), "sew-api (127.0.0.1:8470)");
        assert_eq!(want("sew-api", 8470, 9000).display(), "sew-api (host 127.0.0.1:8470, in the wall :9000)");
    }

    #[test]
    fn presets_read_sews_config_and_refuse_a_public_bind() {
        let api = preset("sew-api").unwrap();
        let sig = preset("sew-signal").unwrap();
        assert!(preset("postgres").is_none());
        assert_eq!(resolve_preset(api, None).unwrap(), Resolved { port: 8470, from: "default".into() });
        assert_eq!(resolve_preset(sig, None).unwrap().port, 7880);
        let toml = "# generated by sew\n[server]\nlisten = \"127.0.0.1:8471\" # moved\n\n[mgmt]\nsocket = \"/run/sew/mgmt.sock\"\n\n[voice.ops]\nsignal_listen = \"127.0.0.1:7890\"\n";
        assert_eq!(resolve_preset(api, Some(toml)).unwrap(), Resolved { port: 8471, from: SEW_CONFIG.into() });
        assert_eq!(resolve_preset(sig, Some(toml)).unwrap().port, 7890);
        // A key in another section isn't it; a config without the key → default.
        let other = "[mgmt]\nlisten = \"127.0.0.1:9999\"\n";
        assert_eq!(resolve_preset(api, Some(other)).unwrap().port, 8470);
        let public = "[server]\nlisten = \"0.0.0.0:8470\"\n";
        assert!(resolve_preset(api, Some(public)).unwrap_err().contains("not on this box's loopback"));
        assert_eq!(resolve_preset(api, Some("[server]\nlisten = \"localhost:8472\"\n")).unwrap().port, 8472);
    }

    #[test]
    fn a_request_holds_until_approved_and_revoke_is_immediate() {
        let c = conn();
        let now = 1_000;
        let (w, allowed) = request(&c, "hearth", &[want("sew-api", 8470, 8470), want("sew-signal", 7880, 7880)], &ask("agent:ws-a"), now).unwrap();
        let w = w.unwrap();
        assert!(allowed.is_empty());
        assert_eq!(w.kind, KIND_BOX_SERVICE);
        assert_eq!(w.status, reqs::STATUS_PENDING);
        assert_eq!(w.box_services().len(), 2);
        assert_eq!(w.summary, "let App hearth reach sew-api (127.0.0.1:8470), sew-signal (127.0.0.1:7880) on this box");
        // Pending: forwarded (the door is shut), not admitted.
        assert_eq!(forwards(&c, "hearth").unwrap().len(), 2);
        assert!(allowed_host_ports(&c, "hearth").unwrap().is_empty());
        // Asking again files nothing new and points at the open request.
        let (again, _) = request(&c, "hearth", &[want("sew-api", 8470, 8470)], &ask("agent:ws-a"), now + 1).unwrap();
        assert_eq!(again.unwrap().id, w.id);
        // Approve: both admitted.
        let d = reqs::decide(&c, &w.id, reqs::STATUS_APPROVED, "owner", Some("touch-id"), None, now + 10).unwrap();
        assert_eq!(d.status, reqs::STATUS_APPROVED);
        assert_eq!(allowed_host_ports(&c, "hearth").unwrap(), vec![8470, 7880]);
        let r = get(&c, "hearth", "sew-api").unwrap().unwrap();
        assert_eq!((r.status.as_str(), r.approved_by.as_deref(), r.request_id.as_deref()), (STATUS_ALLOWED, Some("owner"), None));
        // Asking for an allowed one: nothing held.
        let (none, allowed) = request(&c, "hearth", &[want("sew-api", 8470, 8470)], &ask("agent:ws-a"), now + 11).unwrap();
        assert!(none.is_none());
        assert_eq!(allowed, vec!["sew-api".to_string()]);
        // Revoke: off at once; per App.
        assert!(revoke(&c, "hearth", "sew-signal").unwrap().is_some());
        assert_eq!(allowed_host_ports(&c, "hearth").unwrap(), vec![8470]);
        assert!(revoke(&c, "hearth", "sew-signal").unwrap().is_none());
        assert!(allowed_host_ports(&c, "other").unwrap().is_empty());
    }

    #[test]
    fn deny_withdraw_and_expiry_remove_pending_rows() {
        let c = conn();
        let (w, _) = request(&c, "hearth", &[want("sew-api", 8470, 8470)], &ask("agent:x"), 100).unwrap();
        reqs::decide(&c, &w.unwrap().id, reqs::STATUS_DENIED, "owner", None, Some("not now"), 101).unwrap();
        assert!(list(&c, "hearth").unwrap().is_empty());
        let (w, _) = request(&c, "hearth", &[want("sew-api", 8470, 8470)], &ask("agent:x"), 200).unwrap();
        reqs::decide(&c, &w.unwrap().id, reqs::STATUS_WITHDRAWN, "agent:x", None, None, 201).unwrap();
        assert!(list(&c, "hearth").unwrap().is_empty());
        let (w, _) = request(&c, "hearth", &[want("sew-api", 8470, 8470)], &ask("agent:x"), 300).unwrap();
        let w = w.unwrap();
        assert_eq!(
            reqs::decide(&c, &w.id, reqs::STATUS_APPROVED, "owner", Some("pw"), None, 300 + reqs::TTL_SECS).unwrap_err(),
            "request_expired"
        );
        assert!(list(&c, "hearth").unwrap().is_empty(), "an expired request releases its rows");
        // The uninstall cascade drops the rows.
        let (_w, _) = request(&c, "other", &[want("sew-api", 8470, 8470)], &Ask { workspace: Some("ws-b"), ..ask("agent:y") }, 400).unwrap();
        store::delete_app(&c, "other").unwrap();
        assert_eq!(c.query_row("SELECT COUNT(*) FROM app_box_services WHERE app_id = 'other'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }

    #[test]
    fn conflicts_and_caps() {
        let c = conn();
        request(&c, "hearth", &[want("sew-api", 8470, 8470)], &ask("agent:x"), 1).unwrap();
        let e = request(&c, "hearth", &[want("sew-api", 8471, 8470)], &ask("agent:x"), 2).unwrap_err();
        assert!(e.contains("revoke it first"), "{e}");
        let e = request(&c, "hearth", &[want("other", 9000, 8470)], &ask("agent:x"), 2).unwrap_err();
        assert!(e.contains("already sew-api's"), "{e}");
        let e = request(&c, "hearth", &[want("a", 9000, 9000), want("b", 9001, 9000)], &ask("agent:x"), 2).unwrap_err();
        assert!(e.contains("twice"), "{e}");
        let more: Vec<Want> = (0..(MAX_PER_APP as u16)).map(|i| want(&format!("s{i}"), 9100 + i, 9100 + i)).collect();
        let e = request(&c, "hearth", &more, &ask("agent:x"), 3).unwrap_err();
        assert!(e.contains("at most"), "{e}");
        // A different requester gets its own request.
        let (w1, _) = request(&c, "hearth", &[want("a", 9000, 9000)], &ask("agent:x"), 4).unwrap();
        let (w2, _) = request(&c, "hearth", &[want("b", 9001, 9001)], &ask("agent:y"), 5).unwrap();
        assert_ne!(w1.unwrap().id, w2.as_ref().unwrap().id);
        assert!(!request_is_empty(&c, "hearth", &w2.as_ref().unwrap().id).unwrap());
        revoke(&c, "hearth", "b").unwrap();
        assert!(request_is_empty(&c, "hearth", &w2.unwrap().id).unwrap());
    }
}
