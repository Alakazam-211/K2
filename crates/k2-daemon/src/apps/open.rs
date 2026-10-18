//! The open path (`prd-k2-app-runtime-v1` §4.1, §4.3, §8.2; S13, K5;
//! Rosson's Q3 ruling 2026-10-17): the **relaxed container**.
//!
//! An open-path App runs in the same rootless container as a guarded one,
//! through the same `k2-app-engine` (protocol 3, `wall: relaxed`), and
//! keeps the whole floor except four things (§4.3):
//! - a writable root, and `/app` a writable **copy** of the code made at
//!   every start (Q3); what must persist goes in `/data`, which stays;
//! - `/tmp` may run programs;
//! - the App's own port inside (an old service keeps the `--port` it
//!   always used) behind K2's host port, plus extra TCP ports on host
//!   loopback (`k2 app config --port-add`; direct public ports and UDP wait
//!   for 0.46.2, Q3);
//! - egress **broad** (K24): any public host, still only through the App's
//!   own proxy, every first-seen host in the audit; the owner can tighten
//!   it (`k2 app egress <app> --mode allowlist`).
//! Its image is `node24-full` (Debian's toolchain: "any toolchain", K4).
//! The gateway link and pass, box services, the nft rule, the process
//! user, the caps and the door are the same as guarded.
//!
//! **K5 (§8.2):** from 0.46.1 a NEW `k2 publish run --cmd` label on a box
//! where the open path is ready becomes an open-path App: its folder is
//! stored as a Seed as it is (`k2_core::deploy::build::snapshot_folder`, a
//! synthesized manifest in the row), deployed through the shared pipeline
//! (health = the port answers, as `k2 publish run` always checked), and a
//! re-run takes a fresh snapshot. An agent needs "Allow this agent to
//! manage Apps" + "…and publish open Apps", or its run is held for the
//! owner (`publish_routes`). Existing `--cmd` services are untouched in
//! 0.46.1 (Rosson: their environment ends in 0.46.2, with `adopt`). On a
//! box where the open path isn't ready (macOS, no root helper, no engine,
//! the runtime off) `k2 publish run --cmd` keeps 0.46.0's behaviour and
//! says why.

use std::path::PathBuf;
use std::time::Duration;

use k2_core::app_grace::config::{self as appcfg, AppConfig};
use k2_core::deploy::engine::{AppSpec, CgroupStats, EngineCaps, ExecStream, Isolation, Liveness, Prepared, Result, RunHandle, RuntimeEngine, SeedRef};
use k2_core::deploy::store::{self, AppRow};
use k2_core::deploy::{self, codes};
use k2_core::published_services as ps;
use k2_root_proto::engine::Wall;
use serde_json::{json, Value as J};

use super::routes::{actor, db_err, no, Who, R};

/// `engine_ref` prefix of a relaxed instance (`supervisor::engine_of`).
pub const REF_PREFIX: &str = super::guarded::RELAXED_REF_PREFIX;

/// The relaxed container engine. Prepare and start render `wall: relaxed`
/// through the guarded engine's container code; everything after start
/// (liveness, stop, exec, stats) is the same engine call.
pub struct OpenEngine;

pub fn engine() -> &'static OpenEngine {
    static E: OpenEngine = OpenEngine;
    &E
}

pub fn owns(h: &RunHandle) -> bool {
    h.engine_ref.starts_with(REF_PREFIX)
}

impl RuntimeEngine for OpenEngine {
    fn isolation(&self) -> Isolation {
        Isolation::Container
    }

    fn caps(&self) -> EngineCaps {
        EngineCaps {
            separate_user: true,
            memory_limit: true,
            cpu_limit: false,
            // The code is a writable copy (Q3): the Seed itself stays sealed.
            read_only_seed: false,
            // Broad, but only through the App's own proxy (K24).
            egress_gate: true,
            exec: true,
            adopt: true,
        }
    }

    fn prepare(&self, seed: &SeedRef, spec: &AppSpec) -> Result<Prepared> {
        super::guarded::engine().prepare_with(seed, spec, Wall::Relaxed)
    }

    fn start(&self, spec: &AppSpec, p: &Prepared, port: u16) -> Result<RunHandle> {
        super::guarded::engine().start_with(spec, p, port, Wall::Relaxed)
    }

    fn liveness(&self, h: &RunHandle) -> Liveness {
        super::guarded::engine().liveness(h)
    }

    fn stop(&self, h: &RunHandle, grace: Duration) -> Result<()> {
        super::guarded::engine().stop(h, grace)
    }

    fn exec(&self, h: &RunHandle, argv: &[String], tty: bool) -> Result<ExecStream> {
        super::guarded::engine().exec(h, argv, tty)
    }

    fn stats(&self, h: &RunHandle) -> Result<CgroupStats> {
        super::guarded::engine().stats(h)
    }

    /// The guarded engine's adopt returns relaxed instances too (with this
    /// prefix): nothing of its own to adopt.
    fn adopt(&self) -> Result<Vec<RunHandle>> {
        Ok(Vec::new())
    }
}

// ── readiness ────────────────────────────────────────────────────────

/// Is the open path ready on this box? `Err` says why not, in words an
/// agent can act on (or pass to its owner).
pub fn ready() -> std::result::Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("Apps run in a wall on Linux boxes only".into());
    }
    if !deploy::enabled() {
        return Err("the App runtime is off on this box (Settings → K2 Server → Apps, or K2_APPS=1)".into());
    }
    let st = crate::root_helper::state();
    if !st.usable(k2_root_proto::version::APPS_MIN_PROTOCOL) {
        return Err("the box isn't prepared for Apps (no root helper at protocol 6+: k2 app box prepare)".into());
    }
    let v = super::guarded::engine_version().map_err(|e| format!("the App engine isn't answering ({e}): k2 app engine install"))?;
    match v["protocol"].as_u64() {
        Some(p) if p == k2_root_proto::engine::ENGINE_PROTOCOL as u64 => Ok(()),
        p => Err(format!(
            "the App engine speaks protocol {} and this daemon {}: k2 app engine install",
            p.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            k2_root_proto::engine::ENGINE_PROTOCOL
        )),
    }
}

// ── labels (§4.5) ────────────────────────────────────────────────────

/// The computed wall label: never claimed, always derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    /// What makes it open (open labels only).
    pub reason: Option<String>,
    /// The command back to guarded (open labels only).
    pub back: Option<String>,
}

/// §4.5, one pure function. `egress_mode` is the App's stored mode.
pub fn label(app: &AppRow, cfg: &AppConfig, egress_mode: &str) -> Label {
    let plain = |t: &str| Label { text: t.into(), reason: None, back: None };
    if app.is_process_exception() {
        return Label {
            text: "Open (process)".into(),
            reason: Some("the owner's process exception (a device a container can't reach)".into()),
            back: Some(format!("k2 app exception {} off", app.app_id)),
        };
    }
    match (app.path.as_str(), app.isolation.as_str()) {
        ("open", "container") => {
            let mut why = Vec::new();
            if app.source == "folder" {
                why.push("the code is the workspace folder (copied at each start)".to_string());
            } else {
                why.push("a writable copy of the code".to_string());
            }
            if egress_mode == deploy::egress::MODE_BROAD {
                why.push("egress broad (logged)".into());
            }
            let ports = cfg.tcp_ports();
            if !ports.is_empty() {
                why.push(format!("{} extra port{}", ports.len(), if ports.len() == 1 { "" } else { "s" }));
            }
            Label { text: "Open (container)".into(), reason: Some(why.join("; ")), back: Some(format!("k2 app config {} --path guarded", app.app_id)) }
        }
        ("open", _) => plain("Open (no engine on this box)"),
        (_, "container") => plain("Guarded (container)"),
        (_, "vm") => plain("Guarded (VM)"),
        (_, "process") => plain("Preview, unwalled (process)"),
        (_, "none") => plain("Dev (no wall)"),
        _ => plain("Preview"),
    }
}

/// The label for a stored App (reads its config and egress mode).
pub fn label_of(app: &AppRow) -> Label {
    let (cfg, mode) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            appcfg::load(&conn, &app.app_id).unwrap_or_default(),
            deploy::egress::mode(&conn, &app.app_id).unwrap_or_else(|_| deploy::egress::MODE_ALLOWLIST.into()),
        )
    };
    label(app, &cfg, &mode)
}

// ── K5: `k2 publish run --cmd` on the open path ──────────────────────

/// A `k2 publish run --cmd` request the open path takes over.
#[derive(Debug, Clone)]
pub struct Run {
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: PathBuf,
    /// `--port`: the port the service listens on (inside its wall now).
    pub port: u16,
    pub no_tunnel: bool,
}

/// The open-path folder App behind `name` in this workspace, if any.
pub fn folder_app(project_id: &str, name: &str) -> Option<AppRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_app(&conn, &name.to_ascii_lowercase())
        .ok()
        .flatten()
        .filter(|a| a.owner_workspace == project_id && a.source == "folder" && a.path == "open")
}

/// Check a `--cmd` run before anything is written: the label fits an App
/// id, the port fits a relaxed container, the command and folder exist.
pub(crate) fn check_run(r: &Run) -> R<()> {
    let id = r.name.to_ascii_lowercase();
    if let Some((code, msg)) = deploy::app_id_problem(&id) {
        return Err(no(code, 400, format!("from 0.46.1 a new --cmd service is an open-path App: {msg}")));
    }
    if k2_root_proto::apps::AppId::parse(&id).is_none() {
        return Err(no(
            codes::APP_ID_INVALID,
            400,
            format!("from 0.46.1 a new --cmd service is an open-path App, and '{id}' doesn't fit an App id (at most 32 characters, a letter first, no '--', no trailing '-'): pick a shorter label"),
        ));
    }
    if r.cmd.trim().is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, "--cmd is empty"));
    }
    if r.port != 0 && !k2_root_proto::engine::app_port_ok(r.port) {
        return Err(no(
            codes::BAD_REQUEST,
            400,
            format!("port {} can't be an open-path App's port (1024 or more, not 8730, outside K2's 38600-38999)", r.port),
        ));
    }
    if !r.cwd.is_dir() {
        return Err(no(codes::BAD_REQUEST, 400, format!("{} is not a folder", r.cwd.display())));
    }
    Ok(())
}

/// Create (or find) the App and deploy a fresh snapshot of the folder.
/// Answers at once with the deployment started (the CLI waits 30 s at
/// most; the first deploy on a box fetches the image).
pub(crate) fn publish_run(who: &Who, r: &Run) -> R<J> {
    check_run(r)?;
    let id = r.name.to_ascii_lowercase();
    let by = actor(who);
    let existing = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_app(&conn, &id).map_err(db_err)?
    };
    let (app, created) = match existing {
        Some(a) if a.owner_workspace == r.project_id && a.source == "folder" && a.path == "open" => (a, false),
        Some(_) => {
            return Err(no(
                codes::APP_EXISTS,
                409,
                format!("'{id}' is already an App on this box (not this workspace's --cmd service): k2 app list"),
            ))
        }
        None => (create(who, r, &id)?, true),
    };
    super::suspend::refuse_if_suspended(&app.app_id)?;
    // A re-run may move the folder or the port, and always takes a fresh
    // snapshot of the code.
    let app = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE apps SET folder_path = ?2, updated_at = ?3 WHERE app_id = ?1",
            rusqlite::params![app.app_id, r.cwd.to_string_lossy(), deploy::now()],
        )
        .map_err(db_err)?;
        let mut cfg = appcfg::load(&conn, &app.app_id).map_err(db_err)?;
        cfg.app_port = (r.port != 0).then_some(r.port);
        conn.execute(
            "UPDATE apps SET config_json = ?2 WHERE app_id = ?1",
            rusqlite::params![app.app_id, serde_json::to_string(&cfg).unwrap_or_default()],
        )
        .map_err(db_err)?;
        store::get_app(&conn, &app.app_id).map_err(db_err)?.ok_or_else(|| no(codes::APP_NOT_FOUND, 404, "the App vanished"))?
    };
    let seed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let m = deploy::build::folder_manifest(&app.app_id, &r.cmd);
        deploy::build::snapshot_folder(&conn, &r.cwd, &super::routes::signer(who), &m).map_err(|f| no(f.code, 422, f.message))?
    };
    let dep = super::supervisor::begin(&app, &seed.seed.hash, store::KIND_DEPLOY, &by, Some("k2 publish run --cmd (open path)"))?;
    let row = super::supervisor::wait_done(&dep, 20);
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::get_by_id(&conn, &app.service_id).map_err(db_err)?
    }
    .map(|s| crate::publish_runtime::to_json(&s));
    let mut out = serde_json::to_value(&svc).unwrap_or(J::Null);
    if !out.is_object() {
        out = json!({ "name": app.app_id });
    }
    let failed = row.as_ref().is_some_and(|d| d.result == store::RESULT_FAILED);
    let l = label_of(&app);
    out["wall"] = json!({
        "path": "open",
        "label": l.text,
        "reason": l.reason,
        "back": l.back,
        "app": app.app_id,
        "created": created,
        "deployment": dep,
        "seed": seed.seed.hash,
        "result": row.as_ref().map(|d| d.result.clone()),
        "note": "from K2 0.46.1 a new --cmd service runs as an open-path App: a relaxed container with a copy of this folder \
                 made at each start (keep what must last in $K2_APP_DATA, /data), egress logged. \
                 Re-run to deploy your changes; k2 app logs|inspect|history <name>.",
    });
    if failed {
        let why = row.and_then(|d| d.error).unwrap_or_else(|| "the deploy failed".into());
        let mut refusal = no(codes::HEALTH_FAILED, 409, format!("{} didn't come up on the open path: {why} (k2 app logs {})", app.app_id, app.app_id));
        refusal.extra = Some(out);
        return Err(refusal);
    }
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: app.owner_workspace.clone() });
    Ok(out)
}

/// The App row for a new label: a published-service row the App runtime
/// drives, the App (`path=open`, `source=folder`, `isolation=container`),
/// egress broad (logged), the front port and the URL.
fn create(who: &Who, r: &Run, id: &str) -> R<AppRow> {
    crate::label_owner::check(id, &r.project_id, false).map_err(|e| no(codes::LABEL_TAKEN, 409, e.message()))?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let holders: Vec<String> = {
            let mut st = conn.prepare("SELECT project_id FROM published_services WHERE name = ?1").map_err(db_err)?;
            let rows = st.query_map(rusqlite::params![id], |row| row.get::<_, String>(0)).map_err(db_err)?;
            rows.collect::<std::result::Result<_, _>>().map_err(db_err)?
        };
        if !holders.is_empty() {
            return Err(no(codes::LABEL_TAKEN, 409, format!("'{id}' is already a published label on this box")));
        }
    }
    let tunnel_cfg = if r.no_tunnel {
        None
    } else {
        Some(crate::publish_runtime::nested_preflight().map_err(|e| no(codes::ENGINE_UNAVAILABLE, 503, e.message))?)
    };
    let dir = deploy::app_dir(id);
    std::fs::create_dir_all(dir.join("logs")).map_err(|e| no("io_error", 500, e.to_string()))?;
    let by = actor(who);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = ps::insert(
            &conn,
            &r.project_id,
            id,
            ps::CMD_APP_SENTINEL,
            &dir.to_string_lossy(),
            0,
            if r.no_tunnel { ps::EXPOSE_LOCAL } else { ps::EXPOSE_TUNNEL },
            ps::DESIRED_STOPPED,
            ps::KIND_CMD,
            "",
        )
        .map_err(db_err)?;
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: id,
                service_id: &svc.id,
                seed_id: Some(id),
                owner_workspace: &r.project_id,
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: &by,
            },
        )
        .map_err(db_err)?;
        conn.execute(
            "UPDATE apps SET path = 'open', source = 'folder', folder_path = ?2 WHERE app_id = ?1",
            rusqlite::params![id, r.cwd.to_string_lossy()],
        )
        .map_err(db_err)?;
        store::set_state(&conn, id, store::STATE_INSTALLING, Some("k2 publish run --cmd (open path)")).map_err(db_err)?;
        // K24: broad, through the App's own proxy, every host logged.
        deploy::egress::set_mode(&conn, id, deploy::egress::MODE_BROAD, "open-path").map_err(db_err)?;
        // A folder App asks for no caps: its approved set is empty (lane 2:
        // a later Seed with caps is held for the owner).
        super::grants::record(&conn, id, &[]).map_err(db_err)?;
    }
    let undo = |why: String| -> super::routes::Refusal {
        super::supervisor::forget(id);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::delete_app(&conn, id);
        let _ = ps::delete(&conn, &r.project_id, id);
        no(codes::ENGINE_UNAVAILABLE, 503, why)
    };
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_app(&conn, id).map_err(db_err)?
    }
    .ok_or_else(|| undo("the App row vanished".into()))?;
    let port = match super::supervisor::ensure_front(&row) {
        Ok(p) => p,
        Err(f) => return Err(undo(f.message)),
    };
    if let Some((cfg, _)) = tunnel_cfg {
        if let Err(e) = crate::publish_runtime::attach_hostname(&r.project_id, id, port, &cfg.token, &cfg.subdomain, false) {
            return Err(undo(format!("the URL step failed: {e}")));
        }
    }
    super::egress_proxy::reload(id);
    let h = k2_core::db::shared();
    let conn = h.lock();
    let _ = deploy::audit::append(
        &conn,
        deploy::audit::SUBJECT_APP,
        id,
        "install",
        &by,
        None,
        &json!({ "path": "open", "source": "folder", "folder": r.cwd, "frontPort": port, "tunnel": !r.no_tunnel, "via": "k2 publish run --cmd" }),
    );
    drop(conn);
    super::routes::mirror(id);
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_app(&conn, id).map_err(db_err)?.ok_or_else(|| no(codes::APP_NOT_FOUND, 404, "the App vanished"))
}

/// `k2 publish stop|start|rm <name>` for an open-path folder App (§8.2:
/// the publish verbs map onto the App).
pub(crate) fn publish_verb(who: &Who, verb: &str, app: &AppRow, keep_hostname: bool) -> R<J> {
    let body = json!({ "app": app.app_id }).to_string();
    let path = match verb {
        "stop" => "/cli/app/stop",
        "start" => "/cli/app/start",
        "rm" => "/cli/app/uninstall",
        _ => return Err(no(codes::BAD_REQUEST, 400, "stop, start or rm")),
    };
    let _ = keep_hostname;
    let resp = super::routes::handle(path, who.clone(), body.as_bytes(), &Default::default());
    let v: J = serde_json::from_str(&resp.body).unwrap_or(J::Null);
    if resp.status.starts_with("2") {
        let mut v = v;
        v["wall"] = json!({ "path": "open", "app": app.app_id, "via": path });
        Ok(v)
    } else {
        let code = v.pointer("/error/code").and_then(J::as_str).unwrap_or("error").to_string();
        let hint = v.pointer("/error/hint").and_then(J::as_str).unwrap_or("the App route refused").to_string();
        let status = resp.status.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(409);
        Err(no(static_code(&code), status, hint))
    }
}

fn static_code(c: &str) -> &'static str {
    match c {
        "apps_manage_off" => codes::APPS_MANAGE_OFF,
        "app_not_found" => codes::APP_NOT_FOUND,
        "owner_only" => codes::OWNER_ONLY,
        "forbidden" => codes::FORBIDDEN,
        _ => "app_refused",
    }
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod tests;
