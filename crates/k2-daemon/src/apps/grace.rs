//! 0.46.0 "grace" routes for existing Apps (Rosson's cross-over plan,
//! 2026-10-14: 0.46.0 adds the new ways and removes nothing; 0.46.1 is the
//! cut-over). They serve every published `--cmd` service (NSI's Jobs BFF,
//! Hearth) and every runtime App, and are **not** behind the runtime
//! preview flag.
//!
//! ```text
//! GET  /cli/app/warnings   [app]           what breaks at the 0.46.4 cut-over, per service
//! POST /cli/app/clean-mode {app, on, restart?}   opt into the 0.46.4 behaviour early
//! POST /cli/app/key/mint   {app, scopes, rooms?, roles?, label?, deliver?, confirmToken?}
//! GET  /cli/app/keys       [app]           App-scoped keys (never the key)
//! POST /cli/app/key/revoke {id}
//! GET  /cli/app/approvals  [app, all]      held requests (every kind: super::approvals)
//! POST /cli/app/approve    {request, confirmToken}   owner, in person
//! POST /cli/app/deny       {request, reason?}        admin; the requester withdraws
//! ```
//!
//! Who may do what (prd-k2-app-runtime-v1 §5.6, §5.11):
//! - Narrowing never waits: clean mode on, revoking a key. A passport may
//!   narrow its own workspace's Apps.
//! - Widening: the owner (owner token, an Owner login, or a human in a
//!   plain K2 shell tab, NT5) applies it; an agent or a non-owner login
//!   proposes it and the request is held (`202`, exit 2).
//! - Minting an App key is a credential mint: a fresh owner confirm from
//!   every human owner (vault action `app-key-mint`, bound to the App),
//!   owner token included. An agent with "…and manage them end to end"
//!   (switch 2, prd-agent-app-permissions-v1 §2.1) mints its own App's key
//!   into the vault itself (it never sees it), and turns clean mode off
//!   itself. Anyone else's mint is a held request.
//! - Members are read-only (Q7).
//! - Approving is human-presence: the owner token or an Owner login, with
//!   a fresh confirm (`app-approve`, bound to the request id). Never a
//!   passport, never a shell tab (it can't pass the confirm).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use k2_core::app_grace::{self, keys, requests, scan, token_uses};
use k2_core::connect_users::Role;
use k2_core::deploy::{self, codes};
use k2_core::published_services as ps;
use serde_json::{json, Value as J};

use super::routes::{actor, db_err, json_body, need_credential, no, s, ws_handle, Refusal, Who, R};
use crate::cli_response::CliResponse;

pub const MAX_BODY: usize = 64 * 1024;

pub const ROUTES: &[&str] = &[
    "/cli/app/approvals",
    "/cli/app/approve",
    "/cli/app/clean-mode",
    "/cli/app/deny",
    "/cli/app/key/mint",
    "/cli/app/key/revoke",
    "/cli/app/keys",
    "/cli/app/warnings",
];

pub const POST_ROUTES: &[&str] =
    &["/cli/app/approve", "/cli/app/clean-mode", "/cli/app/deny", "/cli/app/key/mint", "/cli/app/key/revoke"];

/// Vault confirm actions this module takes (`vault_routes::CONFIRM_ACTIONS`;
/// approving is `super::approvals::CONFIRM_ACTION`, `app-approve`).
pub const CONFIRM_KEY_MINT: &str = "app-key-mint";
/// The file name an App key arrives under (`$K2_SECRETS_DIR/K2_APP_KEY`).
pub const KEY_FILE: &str = "K2_APP_KEY";

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

pub fn too_large() -> CliResponse {
    super::routes::respond(Err(no(codes::BAD_REQUEST, 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024))))
}

pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    // Q7: a Member login sees Apps and changes nothing.
    if is_post_route(path) {
        if let Err(e) = super::access::member_read_only(&who) {
            return super::routes::respond(Err(e));
        }
    }
    let r = match path {
        "/cli/app/warnings" => warnings(&who, params),
        "/cli/app/clean-mode" => clean_mode(&who, body),
        "/cli/app/key/mint" => key_mint(&who, body),
        "/cli/app/keys" => key_list(&who, params),
        "/cli/app/key/revoke" => key_revoke(&who, body),
        // Held requests of every kind (grace and egress): one set of routes.
        p if super::approvals::ROUTES.contains(&p) => super::approvals::handle(p, &who, body, params),
        _ => Err(no("not_found", 404, "no such App route")),
    };
    super::routes::respond(r)
}

// ── callers ──────────────────────────────────────────────────────────

/// The owner in person: the owner token or an Owner login (can pass a
/// vault confirm).
fn is_owner(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

/// Applies widenings at once (owners widen): the owner, or a human in a
/// plain K2 shell tab (NT5).
fn widens(who: &Who) -> bool {
    is_owner(who) || matches!(who, Who::Shell { .. })
}

fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, role: Role::Owner } => Some(format!("connect:{username}")),
        _ => None,
    }
}

fn own_ws(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id.as_str()),
        _ => None,
    }
}

// ── the App a call names ─────────────────────────────────────────────

/// A published service (a `--cmd` BFF, a `--skin` App, or a runtime App's
/// service row) as these routes see it.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub service_id: String,
    pub project_id: String,
    pub name: String,
    pub kind: String,
    pub cmd: String,
    pub cwd: String,
    /// The runtime App id when the service is a runtime App.
    pub app_id: Option<String>,
}

impl Target {
    fn from_row(r: ps::PublishedService, app_id: Option<String>) -> Self {
        Target { service_id: r.id, project_id: r.project_id, name: r.name, kind: r.kind, cmd: r.cmd, cwd: r.cwd, app_id }
    }

    fn is_cmd(&self) -> bool {
        self.kind != "skin" && self.cmd != ps::CMD_APP_SENTINEL
    }

    fn summary(&self) -> J {
        let se = crate::vault::publish_env::get(&self.service_id);
        json!({
            "serviceId": self.service_id,
            "name": self.name,
            "handle": format!("{}/{}", ws_handle(&self.project_id), self.name),
            "workspace": self.project_id,
            "workspaceHandle": ws_handle(&self.project_id),
            "kind": if self.app_id.is_some() { "app" } else if self.kind == "skin" { "skin" } else { "cmd" },
            "appId": self.app_id,
            "envMode": if self.is_cmd() { Some(se.mode.clone()) } else { None },
            "cleanMode": se.clean_mode,
        })
    }
}

fn not_found(raw: &str) -> Refusal {
    no(codes::APP_NOT_FOUND, 404, format!("no App or published service '{raw}' (k2 publish list, k2 app list)"))
}

/// Resolve `raw`: a runtime App id, a published service id, `<workspace>/<name>`,
/// or a bare publish name (unique on the box, or in the caller's own
/// workspace). A passport sees only its own workspace's (a foreign one is
/// `app_not_found`, never "forbidden").
pub(crate) fn target(who: &Who, raw: Option<&str>) -> R<Target> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the App (its name, <workspace>/<name> or id)"))?;
    let own = own_ws(who).map(str::to_string);
    let found: Vec<Target> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut out: Vec<Target> = Vec::new();
        if let Ok(Some(app)) = deploy::store::get_app(&conn, &raw.to_ascii_lowercase()) {
            if let Ok(Some(row)) = ps::get_by_id(&conn, &app.service_id) {
                out.push(Target::from_row(row, Some(app.app_id)));
            }
        }
        if out.is_empty() {
            if let Ok(Some(row)) = ps::get_by_id(&conn, raw) {
                out.push(Target::from_row(row, None));
            }
        }
        if out.is_empty() {
            if let Some((ws, name)) = raw.split_once('/') {
                drop(conn);
                if let Ok(id) = crate::skin_routes::resolve_app_target(&format!("{ws}/{name}")) {
                    let conn = db.lock();
                    if let Ok(Some(row)) = ps::get_by_id(&conn, &id) {
                        out.push(Target::from_row(row, None));
                    }
                }
            } else {
                let rows = ps::list_all(&conn).map_err(db_err)?;
                for r in rows.into_iter().filter(|r| r.name == raw) {
                    if own.as_deref().is_none_or(|w| w == r.project_id) {
                        out.push(Target::from_row(r, None));
                    }
                }
            }
        }
        out
    };
    let mut found = found;
    if let Some(w) = own.as_deref() {
        found.retain(|t| t.project_id == w);
    }
    match found.len() {
        0 => Err(not_found(raw)),
        1 => {
            let mut t = found.remove(0);
            if t.app_id.is_none() {
                let db = k2_core::db::shared();
                let conn = db.lock();
                t.app_id = deploy::store::get_app_by_service(&conn, &t.service_id).ok().flatten().map(|a| a.app_id);
            }
            Ok(t)
        }
        _ => {
            let names: Vec<String> = found.iter().map(|t| format!("{}/{}", ws_handle(&t.project_id), t.name)).collect();
            Err(Refusal {
                code: "ambiguous",
                status: 409,
                hint: format!("'{raw}' names more than one service; use one of: {}", names.join(", ")),
                extra: Some(json!({ "candidates": names })),
            })
        }
    }
}

fn all_targets(who: &Who) -> R<Vec<Target>> {
    let own = own_ws(who).map(str::to_string);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = ps::list_all(&conn).map_err(db_err)?;
    Ok(rows
        .into_iter()
        .filter(|r| own.as_deref().is_none_or(|w| w == r.project_id))
        .map(|r| {
            let app = deploy::store::get_app_by_service(&conn, &r.id).ok().flatten().map(|a| a.app_id);
            Target::from_row(r, app)
        })
        .filter(Target::is_cmd)
        .collect())
}

// ── held requests ────────────────────────────────────────────────────

/// One line for a held grace request (the approvals list, the owner's
/// confirm prompt and the requester's notice).
fn summary(kind: &str, label: &str, diff: &J) -> String {
    let lines: Vec<String> = match diff {
        J::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let what = match kind {
        requests::KIND_CONFIG => format!("change App {label}'s settings"),
        requests::KIND_PUBLISH_RUN => format!("start the service {label}"),
        requests::KIND_APP_KEY => format!("mint an App key for {label}"),
        requests::KIND_CLEAN_MODE => format!("turn clean mode off for {label}"),
        other => format!("{other} for {label}"),
    };
    if lines.is_empty() {
        what
    } else {
        let shown = if lines.len() > 4 { format!("{} and {} more", lines[..4].join("; "), lines.len() - 4) } else { lines.join("; ") };
        format!("{what}: {shown}")
    }
}

/// Hold a change for the owner (`202`, `status: held`). Returns the
/// answer the route sends.
pub(crate) fn hold(who: &Who, kind: &str, t: Option<&Target>, label: &str, workspace_id: &str, body: J, diff: J, reasons: Vec<String>) -> R<J> {
    let by = actor(who);
    let app_id = t.and_then(|t| t.app_id.clone());
    let req = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        requests::create(
            &conn,
            &requests::NewRequest {
                kind,
                app_id: app_id.as_deref(),
                service_id: t.map(|t| t.service_id.as_str()),
                app_label: label,
                workspace_id,
                summary: summary(kind, label, &diff),
                body,
                diff,
                reasons,
                reason: None,
                deployment_id: None,
                requested_by: &by,
                requester_workspace: super::approvals::own_ws(who),
                requester_session: super::approvals::session_of(who),
            },
            deploy::now(),
        )
        .map_err(db_err)?
    };
    audit("app-request", &by, &format!("held {} {kind} {label}", req.id));
    Ok(super::approvals::held_answer(&req))
}

// ── warnings ─────────────────────────────────────────────────────────

fn warning(code: &str, severity: &str, message: String, fix: String) -> J {
    json!({ "code": code, "severity": severity, "message": message, "fix": fix })
}

/// Names the service gets from K2 itself in clean mode, from its grants
/// and from `.env.k2` (names only; no value is resolved).
fn provided_names(t: &Target) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = crate::vault::publish_env::clean_mode_names().into_iter().map(str::to_string).collect();
    if let Ok(v) = crate::vault::get() {
        if let Ok(gs) = v.grants_to(&k2_vault::Consumer::published_service(t.service_id.clone())) {
            for (g, name) in gs {
                if g.delivery == k2_vault::Delivery::Env {
                    out.insert(g.env_name.clone().unwrap_or(name));
                }
            }
        }
    }
    let se = crate::vault::publish_env::get(&t.service_id);
    let env_file = se.env_file.unwrap_or_else(|| ".env.k2".into());
    if let Ok(p) = crate::vault_use_routes::jailed_path(&PathBuf::from(&t.cwd), &env_file) {
        if let Ok(text) = std::fs::read_to_string(p) {
            for (k, _) in crate::vault::publish_env::parse_env_k2(&text) {
                out.insert(k);
            }
        }
    }
    out
}

/// One service's report.
pub(crate) fn report(t: &Target) -> J {
    let se = crate::vault::publish_env::get(&t.service_id);
    let cwd = PathBuf::from(&t.cwd);
    let mut warnings: Vec<J> = Vec::new();
    let legacy = se.mode == "legacy" && !se.clean_mode;
    let scan = if t.is_cmd() && cwd.is_dir() { scan::scan_folder(&cwd, &t.cmd) } else { scan::ScanReport::default() };
    let provided = provided_names(t);
    let inherited: BTreeSet<String> = if legacy && se.inherited_names.is_empty() {
        std::env::vars().map(|(k, _)| k).collect()
    } else {
        se.inherited_names.iter().cloned().collect()
    };
    let name = &t.name;
    let cut = app_grace::CUTOVER_RELEASE;
    if legacy {
        warnings.push(warning(
            "legacy_env",
            "breaks",
            format!("{name} inherits the K2 daemon's whole environment ({} variables); that ends in K2 {cut}.", inherited.len()),
            format!("k2 publish env {name} clean (and grant what it needs: k2 publish secret add {name} NAME)"),
        ));
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for r in &scan.env_reads {
        if !seen.insert(r.name.clone()) {
            continue;
        }
        let at = format!("{}:{}", r.file, r.line);
        let n = &r.name;
        if (n.starts_with("K2_") || n.starts_with("K2SO_")) && !provided.contains(n) {
            warnings.push(warning(
                "k2_internal_env",
                "breaks",
                format!("{at} reads {n}: a K2-internal variable no App gets from K2 {cut} (the owner token and the daemon's session variables stay out of reach)."),
                format!("use the App key (k2 app key mint {name} …) and K2_HOST/K2_PORT/K2_GATEWAY_URL"),
            ));
        } else if n == "HOME" {
            warnings.push(warning(
                "home_changes",
                "check",
                format!("{at} reads HOME: in clean mode and from {cut} it is the App's own folder, not the daemon user's home."),
                "keep App files in its folder or K2_SECRETS_DIR; try it with k2 app clean-mode".to_string(),
            ));
        } else if n == "PATH" {
            warnings.push(warning(
                "path_changes",
                "check",
                format!("{at} reads PATH: from {cut} it holds the system folders only."),
                "name programs by absolute path, or install them system-wide".to_string(),
            ));
        } else if !provided.contains(n) && inherited.contains(n) {
            warnings.push(warning(
                "inherited_env_read",
                "breaks",
                format!("{at} reads {n}, which {name} gets only from the daemon's environment; from {cut} it is unset."),
                format!("k2 publish secret add {name} {n} --as env (or a k2v:// reference in .env.k2)"),
            ));
        }
    }
    for p in &scan.k2_paths {
        let at = format!("{}:{}", p.file, p.line);
        let (msg, fix) = match p.what.as_str() {
            "owner_token_file" => (
                format!("{at} reads ~/.k2/daemon.token (the owner token): no App can reach it from {cut}."),
                format!("mint an App key (k2 app key mint {name} --scopes users,rooms …) and call /v1/apps/$K2_APP_ID/…"),
            ),
            "daemon_port_file" => (
                format!("{at} reads ~/.k2/heartbeat.port: out of reach from {cut}."),
                "use K2_HOST and K2_PORT (or K2_GATEWAY_URL), set for every service".to_string(),
            ),
            "k2_database" => (format!("{at} names a K2 database file: out of reach from {cut}."), "use k2 db or the App key routes".to_string()),
            _ => (format!("{at} names a K2 socket: out of reach from {cut}."), "call K2 over K2_GATEWAY_URL with the App key".to_string()),
        };
        warnings.push(warning(&p.what, "breaks", msg, fix));
    }
    let now_path = k2_core::terminal::login_path::augmented_path(&std::env::var("PATH").unwrap_or_default());
    let lookups = if t.is_cmd() { scan::path_lookups(&t.cmd, &cwd, if se.clean_mode { scan::CLEAN_PATH } else { &now_path }, scan::CLEAN_PATH) } else { vec![] };
    for l in &lookups {
        if l.breaks() {
            warnings.push(warning(
                "path_lookup",
                "breaks",
                format!(
                    "{} ({}) is found through the inherited PATH at {}; the clean PATH ({}) doesn't find it.",
                    l.program,
                    l.via,
                    l.now.as_deref().unwrap_or("?"),
                    scan::CLEAN_PATH
                ),
                format!("use the absolute path ({}) or install {} system-wide", l.now.as_deref().unwrap_or("?"), l.program),
            ));
        } else if l.now.is_none() && l.clean.is_none() {
            warnings.push(warning(
                "program_missing",
                "breaks",
                format!("{} ({}) isn't found on any PATH", l.program, l.via),
                format!("install {} or fix the command", l.program),
            ));
        }
    }
    let uses = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        token_uses::list(&conn, &t.service_id).unwrap_or_default()
    };
    for u in &uses {
        warnings.push(warning(
            "owner_token_use",
            "breaks",
            format!(
                "{name} called {} with the owner token {} time(s){}; no App holds the owner token from {cut}.",
                u.route,
                u.count,
                if u.refused > 0 { format!(" ({} refused in clean mode)", u.refused) } else { String::new() }
            ),
            format!("mint an App key (k2 app key mint {name} --scopes users,rooms --rooms …) and call /v1/apps/{}/…", t.service_id),
        ));
    }
    let breaks = warnings.iter().filter(|w| w["severity"] == "breaks").count();
    let mut out = json!({
        "app": t.summary(),
        "cutover": cut,
        "readyForCutover": breaks == 0,
        "breaks": breaks,
        "warnings": warnings,
        "envReads": scan.env_reads,
        "k2Paths": scan.k2_paths,
        "pathLookups": lookups,
        "ownerTokenUses": uses,
        "inheritedNames": if legacy { inherited.into_iter().collect::<Vec<_>>() } else { se.inherited_names.clone() },
        "scan": { "filesScanned": scan.files_scanned, "truncated": scan.truncated },
        "ownerTokenAttribution": super::attribution::availability(),
    });
    if !cfg!(target_os = "linux") {
        out["ownerTokenAttributionNote"] = json!(super::attribution::UNAVAILABLE_NOTE);
    }
    out
}

/// GET `/cli/app/warnings?app=` (one) or no `app` (every `--cmd` service
/// the caller can see).
fn warnings(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    match params.get("app").map(String::as_str).filter(|s| !s.trim().is_empty()) {
        Some(raw) => {
            let t = target(who, Some(raw))?;
            if !t.is_cmd() {
                return Ok(json!({ "app": t.summary(), "cutover": app_grace::CUTOVER_RELEASE, "readyForCutover": true, "breaks": 0, "warnings": [],
                    "note": "only server Apps (--cmd services) have an environment; this one has nothing to change" }));
            }
            Ok(report(&t))
        }
        None => {
            let list: Vec<J> = all_targets(who)?.iter().map(report).collect();
            let breaks: u64 = list.iter().map(|r| r["breaks"].as_u64().unwrap_or(0)).sum();
            // The box's tracing, once for the whole list (an empty list on a
            // Mac still says it isn't available there).
            Ok(json!({ "cutover": app_grace::CUTOVER_RELEASE, "services": list, "breaks": breaks,
                "ownerTokenAttribution": super::attribution::availability() }))
        }
    }
}

// ── clean mode ───────────────────────────────────────────────────────

fn bool_field(v: &J, k: &str) -> Option<bool> {
    match v.get(k)? {
        J::Bool(b) => Some(*b),
        J::String(s) => match s.as_str() {
            "on" | "true" | "1" | "yes" => Some(true),
            "off" | "false" | "0" | "no" => Some(false),
            _ => None,
        },
        J::Number(n) => n.as_i64().map(|i| i != 0),
        _ => None,
    }
}

fn restart_service(t: &Target) -> J {
    let running = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::get_by_id(&conn, &t.service_id).ok().flatten().is_some_and(|r| r.desired == "running")
    };
    if !running {
        return json!("not running: applies at its next start");
    }
    if let Err(e) = crate::publish_runtime::stop(&t.project_id, &t.name) {
        return json!(format!("stop failed: {}", e.message));
    }
    match crate::publish_runtime::start(&t.project_id, &t.name) {
        Ok(s) => json!(format!("restarted: {}", s.status)),
        Err(e) => json!(format!("start failed: {}", e.message)),
    }
}

pub(crate) fn set_clean_mode(t: &Target, on: bool, who_label: &str, restart: bool) -> R<J> {
    let se = crate::vault::publish_env::set_clean_mode(&t.service_id, on).map_err(|e| no("store", 500, e))?;
    audit("app-clean-mode", who_label, &format!("{} {}", t.service_id, if on { "on" } else { "off" }));
    let restarted = if restart { restart_service(t) } else { json!("applies at the next start (k2 publish start, or pass restart)") };
    let mut out = json!({ "ok": true, "app": t.summary(), "cleanMode": se.clean_mode, "envMode": se.mode, "restart": restarted,
        "ownerTokenAttribution": super::attribution::availability() });
    if on && !cfg!(target_os = "linux") {
        // Clean mode refuses owner-token calls by tracing them (Linux).
        out["ownerTokenAttributionNote"] = json!(super::attribution::UNAVAILABLE_NOTE);
    }
    Ok(out)
}

/// POST `/cli/app/clean-mode {app, on, restart?}`. On narrows (anyone who
/// can see the App, agents included); off widens (owner; held otherwise).
fn clean_mode(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let t = target(who, s(&v, "app"))?;
    if !t.is_cmd() {
        return Err(no(codes::BAD_REQUEST, 400, "clean mode is for server Apps (--cmd services); this one has no environment"));
    }
    let on = bool_field(&v, "on").ok_or_else(|| no(codes::BAD_REQUEST, 400, "on: true or false"))?;
    let restart = bool_field(&v, "restart").unwrap_or(false);
    // Off widens: the owner, or an agent with "…and manage them end to
    // end" (prd-agent-app-permissions-v1 §2.1); anyone else proposes.
    if on || widens(who) || super::access::self_widens(who) {
        return set_clean_mode(&t, on, &actor(who), restart);
    }
    let label = t.name.clone();
    hold(
        who,
        requests::KIND_CLEAN_MODE,
        Some(&t),
        &label,
        &t.project_id.clone(),
        json!({ "on": false, "restart": restart }),
        json!({ "cleanMode": "on → off" }),
        vec!["turning clean mode off gives the service back the daemon user's PATH and HOME and owner-token calls".into()],
    )
}

// ── App keys ─────────────────────────────────────────────────────────

fn strings(v: &J, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(J::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(J::String(s)) => vec![s.clone()],
        _ => vec![],
    }
}

/// Mint the key and deliver it. `deliver`: `vault` (a server secret
/// granted to the service as the file `K2_APP_KEY`; nobody sees the value)
/// or `show` (returned once, to the owner in person).
pub(crate) fn mint_and_deliver(t: &Target, grant: &keys::AppKeyGrant, label: &str, deliver: &str, by: &str) -> R<J> {
    let mut grant = grant.clone();
    let secret_name = format!("{KEY_FILE}.{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    if deliver == "vault" {
        crate::vault::get().map_err(|e| no("vault_closed", 503, format!("the vault isn't open ({}): {}; or mint with deliver show", e.code, e.hint)))?;
        grant.vault_secret = Some(secret_name.clone());
    }
    let (id, raw) = k2_core::api_keys::create_app_key(label, &t.service_id, &grant.to_json()).map_err(|e| no("store", 500, e))?;
    let mut out = json!({
        "ok": true,
        "id": id,
        "app": t.summary(),
        "scopes": grant.scopes,
        "rooms": grant.rooms,
        "roleIds": grant.roles,
        "routes": format!("/v1/apps/{}/…", t.service_id),
    });
    if deliver == "vault" {
        if let Err(e) = deliver_to_vault(t, &secret_name, &raw) {
            let _ = k2_core::api_keys::revoke_api_key(&id);
            return Err(no("vault_error", 500, format!("the key was not delivered ({}: {}); it was revoked", e.code, e.hint)));
        }
        out["delivered"] = json!({
            "vaultSecret": secret_name,
            "file": KEY_FILE,
            "note": format!(
                "{} reads it at its next start from $K2_SECRETS_DIR/{KEY_FILE} (restart it: k2 publish stop/start {}); \
                 nobody sees the value",
                t.name, t.name
            ),
        });
    } else {
        out["key"] = json!(raw);
        out["note"] = json!("shown once; K2 keeps only its hash");
    }
    audit("app-key-mint", by, &format!("ok {id} app={} scopes={} via={deliver}", t.service_id, grant.scopes.join(",")));
    Ok(out)
}

fn deliver_to_vault(t: &Target, secret: &str, raw: &str) -> Result<(), k2_vault::VaultError> {
    use k2_vault::{Actor, Consumer, Delivery, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore, Selector};
    let vault = crate::vault::get()?;
    let mut put = PutRequest::new(Namespace::User, Scope::server(), secret, Kind::ApiKey);
    put.default_delivery = Some(Delivery::Fetch);
    put.delivery_ceiling = Some(Delivery::File);
    put.description = format!("App key for {} (made by K2; the App reads $K2_SECRETS_DIR/{KEY_FILE})", t.name);
    put.origin = "k2 app key".into();
    vault.put(put, SecretBytes::from_slice(raw.as_bytes()), &Actor::Owner)?;
    // One K2_APP_KEY file per service: the new key replaces the file (the
    // old key keeps working until it's revoked).
    let consumer = Consumer::published_service(t.service_id.clone());
    for (g, _) in vault.grants_to(&consumer)? {
        if g.file_name.as_deref() == Some(KEY_FILE) {
            vault.revoke(&g.id, &Actor::Owner)?;
        }
    }
    vault.grant(
        GrantRequest {
            selector: Selector::user(Scope::server(), secret),
            consumer,
            delivery: Delivery::File,
            env_name: None,
            file_name: Some(KEY_FILE.into()),
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )?;
    Ok(())
}

/// POST `/cli/app/key/mint`. The owner mints with a fresh confirm; anyone
/// else's mint is held for the owner (agents never see a key).
fn key_mint(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let t = target(who, s(&v, "app"))?;
    if !t.is_cmd() && t.app_id.is_none() {
        return Err(no(codes::BAD_REQUEST, 400, "a static (--skin) App has no server to hold a key"));
    }
    let grant = keys::grant_for_app(&t.service_id, &strings(&v, "scopes"), &strings(&v, "rooms"), &strings(&v, "roles"))
        .map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let label = s(&v, "label").map(str::to_string).unwrap_or_else(|| format!("{} App key", t.name));
    let deliver = s(&v, "deliver").unwrap_or("vault");
    if deliver != "vault" && deliver != "show" {
        return Err(no(codes::BAD_REQUEST, 400, "deliver is vault (default) or show"));
    }
    if is_owner(who) {
        let stamp = confirm_stamp(who).unwrap_or_default();
        let Some(token) = s(&v, "confirmToken") else {
            // The confirm binds to the App's service id: say which.
            return Err(Refusal {
                extra: Some(json!({ "serviceId": t.service_id })),
                ..no(
                    "vault_confirm_required",
                    403,
                    "an App key is a credential: minting needs a fresh owner confirm (Touch ID, your login or vault password). \
                     k2 app key mint asks for it from a terminal outside K2.",
                )
            });
        };
        let vault = crate::vault::get().map_err(|e| no("vault_closed", 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))?;
        vault
            .confirm
            .take(token, CONFIRM_KEY_MINT, Some(&t.service_id), &stamp)
            .map_err(|_| no("vault_confirm_required", 403, "confirm again: a confirm works once, for minting this App's key, within 60 seconds"))?;
        return mint_and_deliver(&t, &grant, &label, deliver, &actor(who));
    }
    if deliver == "show" {
        return Err(no(codes::OWNER_ONLY, 403, "only the owner, in person, can see a key; an agent's key goes to the vault"));
    }
    // Switch 2 (prd-agent-app-permissions-v1 §2.1): the agent mints its
    // own App's key itself; it goes to the vault and the agent never sees
    // it. Audited like any mint.
    if super::access::self_widens(who) {
        let mut out = mint_and_deliver(&t, &grant, &label, "vault", &actor(who))?;
        out["via"] = json!(super::access::SELF_VIA);
        return Ok(out);
    }
    let diff = json!({ "scopes": grant.scopes, "rooms": k2_core::skin::handles_for_project_ids(&grant.rooms), "roleIds": grant.roles });
    let label_for = t.name.clone();
    hold(
        who,
        requests::KIND_APP_KEY,
        Some(&t),
        &label_for,
        &t.project_id.clone(),
        json!({ "grant": grant, "label": label }),
        diff,
        vec!["an App key is a credential: the owner mints it (it goes to the vault; nobody sees it)".into()],
    )
}

fn key_json(k: &k2_core::api_keys::AppKeyRow) -> J {
    let g = keys::AppKeyGrant::from_stored(k.grant_json.as_deref());
    let state = if k.revoked_at.is_some() {
        "revoked"
    } else if k.disabled_at.is_some() {
        "disabled"
    } else {
        "active"
    };
    json!({
        "id": k.id,
        "label": k.label,
        "serviceId": k.service_id,
        "state": state,
        "scopes": g.scopes,
        "rooms": k2_core::skin::handles_for_project_ids(&g.rooms),
        "roleIds": g.roles,
        "vaultSecret": g.vault_secret,
        "createdAt": k.created_at,
        "revokedAt": k.revoked_at,
    })
}

/// GET `/cli/app/keys?app=`.
fn key_list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let (rows, app) = match params.get("app").map(String::as_str).filter(|s| !s.trim().is_empty()) {
        Some(raw) => {
            let t = target(who, Some(raw))?;
            (k2_core::api_keys::list_app_keys(Some(&t.service_id)).map_err(|e| no("store", 500, e))?, Some(t.summary()))
        }
        None => {
            let mut rows = k2_core::api_keys::list_app_keys(None).map_err(|e| no("store", 500, e))?;
            if own_ws(who).is_some() {
                let mine: BTreeSet<String> = all_targets(who)?.into_iter().map(|t| t.service_id).collect();
                rows.retain(|k| mine.contains(&k.service_id));
            }
            (rows, None)
        }
    };
    Ok(json!({ "app": app, "keys": rows.iter().map(key_json).collect::<Vec<_>>() }))
}

/// POST `/cli/app/key/revoke {id}`: narrowing, so a passport may revoke
/// its own workspace's App keys. The vault file grant goes with it.
fn key_revoke(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let id = s(&v, "id").ok_or_else(|| no(codes::BAD_REQUEST, 400, "id is required (k2 app keys)"))?;
    let k = k2_core::api_keys::get_app_key(id).ok_or_else(|| no("not_found", 404, format!("no App key '{id}'")))?;
    // Scope check: the caller must see the key's App.
    let t = target(who, Some(&k.service_id)).map_err(|_| no("not_found", 404, format!("no App key '{id}'")))?;
    if matches!(who, Who::Login { role: Role::Member, .. }) {
        return Err(no(codes::OWNER_ONLY, 403, "an Owner or Admin login revokes App keys"));
    }
    let revoked = k2_core::api_keys::revoke_api_key(&k.id).map_err(|e| no("store", 500, e))?;
    let g = keys::AppKeyGrant::from_stored(k.grant_json.as_deref());
    if let (Some(secret), Ok(vault)) = (g.vault_secret.as_deref(), crate::vault::get()) {
        use k2_vault::SecretStore;
        let consumer = k2_vault::Consumer::published_service(t.service_id.clone());
        if let Ok(gs) = vault.grants_to(&consumer) {
            for (gr, name) in gs {
                if name == secret {
                    let _ = vault.revoke(&gr.id, &k2_vault::Actor::Owner);
                }
            }
        }
        let _ = vault.destroy(&k2_vault::Selector::user(k2_vault::Scope::server(), secret), &k2_vault::Actor::Owner);
    }
    audit("app-key-revoke", &actor(who), &format!("{} app={} revoked={revoked}", k.id, t.service_id));
    Ok(json!({ "ok": true, "id": k.id, "revoked": revoked, "app": t.summary() }))
}

// ── approvals ────────────────────────────────────────────────────────
//
// Listing, approving and denying held requests is `super::approvals` (one
// set of routes for the grace kinds and the egress kinds). This module
// carries out an approved grace request.

fn target_by_id(service_id: Option<&str>) -> R<Target> {
    target(&Who::Owner, service_id).map_err(|_| no(codes::APP_NOT_FOUND, 404, "the App was removed"))
}

/// Carry out an approved grace request (`config`, `publish_run`,
/// `app_key`, `clean_mode`). Returns the result JSON.
pub(crate) fn apply_request(r: &requests::AppRequest, by: &str) -> R<J> {
    match r.kind.as_str() {
        requests::KIND_CLEAN_MODE => {
            let t = target_by_id(r.service_id.as_deref())?;
            let on = r.body.get("on").and_then(J::as_bool).unwrap_or(false);
            let restart = r.body.get("restart").and_then(J::as_bool).unwrap_or(false);
            set_clean_mode(&t, on, by, restart)
        }
        requests::KIND_APP_KEY => {
            let t = target_by_id(r.service_id.as_deref())?;
            let grant: keys::AppKeyGrant = serde_json::from_value(r.body.get("grant").cloned().unwrap_or(J::Null))
                .map_err(|_| no(codes::BAD_REQUEST, 400, "the held key request is unreadable"))?;
            let label = r.body.get("label").and_then(J::as_str).unwrap_or("App key").to_string();
            // Re-check the ceiling against today's rooms and roles.
            let g = keys::grant_for_app(&t.service_id, &grant.scopes, &grant.rooms, &grant.roles)
                .map_err(|e| no(codes::BAD_REQUEST, 400, format!("the request no longer fits: {e}")))?;
            mint_and_deliver(&t, &g, &label, "vault", by)
        }
        requests::KIND_CONFIG => {
            let app = r.body.get("app").and_then(J::as_str).ok_or_else(|| no(codes::BAD_REQUEST, 400, "unreadable config request"))?;
            let ops: Vec<app_grace::config::Op> = serde_json::from_value(r.body.get("ops").cloned().unwrap_or(J::Null))
                .map_err(|_| no(codes::BAD_REQUEST, 400, "unreadable config request"))?;
            super::routes::apply_config_ops(app, &ops, by)
        }
        requests::KIND_PUBLISH_RUN => crate::publish_routes::run_approved(&r.body),
        _ => Err(no(codes::BAD_REQUEST, 400, "this request kind is not a grace change")),
    }
}
/// One line in `~/.k2/auth-audit.jsonl` (Settings → Logs → Access Audit).
pub(crate) fn audit(event: &str, user: &str, outcome: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, user, outcome.to_string(), "-", "-", "apps"));
}
