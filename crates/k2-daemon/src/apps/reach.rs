//! An App's reach after a widening (0.46.11 field report: a walled App's
//! owner got its database ready, and inside the wall `127.0.0.1:5432`
//! stayed refused until a re-host nobody mentioned).
//!
//! Two causes, both handled here:
//!
//! - **A stale rule (the process exception, on the per-App table).** Where
//!   the root helper loads the per-App `k2app_v6` table (protocol 15), each
//!   App reaches only the Postgres ports its own nft rule names
//!   (`pg_choice::door_ports`: its workspace's and its granted databases'
//!   clusters, and its `DATABASE_URL`'s). The daemon sent that rule only at
//!   start, at boot and when a box service changed, so a database grant or
//!   a new database landed in K2's records while the running App's rule
//!   kept the old list: refused (the table resets every other loopback
//!   port) until the next start. Now every database change re-sends the
//!   rule of each running App whose list moved ([`refresh_running`]): the
//!   door opens at once, without a re-host.
//! - **The door (containers, guarded or open; 0.46.12, S21).** A container
//!   has its own loopback; K2 forwards the gateway link, the egress proxy,
//!   box services and its database door into it (`db_door`: the App's
//!   `127.0.0.1:5432` reaches the daemon on its own proxy port, which
//!   checks the role and database per connection). So a grant or revoke
//!   counts at the next connection with no rule to re-send (a revoke cuts
//!   open sessions too), and a container's nft rule names no cluster port
//!   at all. What the door still can't serve keeps K2's up-front words
//!   (`app_db_door_missing`): an App engine that predates the door
//!   ([`engine_no_door`]), and `--db-raw on` (the raw connection to a
//!   private App database, which K2 doesn't make yet, [`db_raw_refusal`]).
//!   A Postgres box service is never one: the door is the way in.
//!
//! What only a start delivers: a secret (a `DATABASE_URL` among them)
//! reaches an App when it starts. A grant to a running App answers "Takes
//! effect after a re-host: k2 app rehost <app>" and tells the App's agent
//! the same; K2 re-hosts a `DATABASE_URL` App itself when it is idle.
//!
//! `k2 app doctor <app>` ([`doctor_json`]) names whatever is approved but
//! not in the running App.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use k2_core::deploy::store::{self, AppRow};
use serde_json::{json, Value as J};

pub mod codes {
    /// A container App asked for something its database door can't give
    /// (an engine without the door, or a raw private-database connection).
    pub const DB_DOOR_MISSING: &str = "app_db_door_missing";
}

/// The re-host line every "only a start delivers it" answer carries.
pub fn rehost_line(app: &str) -> String {
    format!("Takes effect after a re-host: k2 app rehost {app}")
}

/// How a container App reaches box Postgres (S21).
pub fn door_line(app: &str) -> String {
    format!(
        "{app} reaches its workspace's databases (and ones granted to it) through its database door: 127.0.0.1:5432 inside its \
         wall, checked per connection (k2 app doctor {app})"
    )
}

/// The door can't serve this container App: this box's App engine
/// predates it.
pub fn engine_no_door(app: &str) -> String {
    format!(
        "{app} runs in a container, and this box's App engine predates the database door, so nothing answers on 127.0.0.1:5432 \
         in its wall: the owner updates it (k2 app engine install), then k2 app rehost {app}"
    )
}

/// `--db-raw on` for a container App: refused up front, approving it would
/// change nothing.
pub fn db_raw_refusal(app: &str) -> String {
    format!(
        "--db-raw is the raw connection to an App's private database, which K2 doesn't make yet, so approving it would change \
         nothing. {}; no widening needed",
        door_line(app)
    )
}

/// The door is there for a container App: its running deployment has
/// one, or this box's engine renders it at the next start.
pub fn door_serves(app: &str) -> bool {
    super::db_door::live_door(app).is_some() || super::db_door::engine_ready()
}

/// Does this isolation put the App in a container (no database door)?
pub fn container_isolation(isolation: &str) -> bool {
    isolation == "container" || isolation == "vm"
}

pub fn in_container(app: &AppRow) -> bool {
    !app.is_process_exception() && container_isolation(&app.isolation)
}

// ── the rule each running App holds ──────────────────────────────────

/// What the daemon last sent the root helper for one App
/// (`apps nft app <id> <gw> <box ports…> [--db <ports…>]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub gw: u16,
    pub box_ports: Vec<u16>,
    /// `None`: sent without `--db` (the band table, or an older helper).
    pub db_ports: Option<Vec<u16>>,
}

fn table() -> &'static Mutex<HashMap<String, Applied>> {
    static T: OnceLock<Mutex<HashMap<String, Applied>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

pub(crate) fn record(app: &str, a: Applied) {
    table().lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), a);
}

pub(crate) fn forget(app: &str) {
    table().lock().unwrap_or_else(|e| e.into_inner()).remove(app);
}

pub fn applied_of(app: &str) -> Option<Applied> {
    table().lock().unwrap_or_else(|e| e.into_inner()).get(app).cloned()
}

/// Send one rule to the root helper (a recorder in tests).
pub(crate) fn send(argv: &[String]) -> Result<(), String> {
    #[cfg(test)]
    {
        test_seams::send(argv)
    }
    #[cfg(not(test))]
    {
        crate::root_helper::call_text(argv, None).map(|_| ())
    }
}

/// Is the per-App table in use (its rules carry `--db`)?
fn per_app_db() -> bool {
    crate::sql::sysops::pgnamed_ok(&crate::root_helper::state())
}

/// The ports the App's rule should hold now: its approved box services'
/// host ports, and (per-App table) its database ports. A container App's
/// database door is the daemon's (it dials the cluster), so its rule names
/// no cluster port: pasta never needs one.
pub(crate) fn wanted(app: &str, per_app_db: bool) -> (Vec<u16>, Option<Vec<u16>>) {
    let container = app_row(app).as_ref().is_some_and(in_container);
    let db = per_app_db.then(|| if container { Vec::new() } else { super::pg_choice::door_ports(app) });
    (super::box_service_routes::allowed_ports(app), db)
}

/// What the running App's rule lacks, and what it holds that it no
/// longer should.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Gap {
    pub box_ports: Vec<u16>,
    pub db_ports: Vec<u16>,
    pub closed: Vec<u16>,
}

impl Gap {
    pub fn is_empty(&self) -> bool {
        self.box_ports.is_empty() && self.db_ports.is_empty() && self.closed.is_empty()
    }
    pub fn opened(&self) -> Vec<u16> {
        self.box_ports.iter().chain(&self.db_ports).copied().collect()
    }
}

fn minus(a: &[u16], b: &[u16]) -> Vec<u16> {
    a.iter().filter(|p| !b.contains(p)).copied().collect()
}

/// The running App's gap (`None`: it isn't running, or K2 sent it no
/// rule; a stopped App's next start sends a fresh one). A rule sent
/// without `--db` on the per-App table admits the whole band, so it lacks
/// no database port.
pub fn gap(app: &str) -> Option<(Applied, Gap)> {
    let a = applied_of(app)?;
    if !app_row(app).is_some_and(|r| r.state == store::STATE_RUNNING) {
        return None;
    }
    let (b, d) = wanted(app, per_app_db());
    let mut g = Gap { box_ports: minus(&b, &a.box_ports), closed: minus(&a.box_ports, &b), ..Gap::default() };
    if let (Some(w), Some(have)) = (&d, &a.db_ports) {
        g.db_ports = minus(w, have);
        g.closed.extend(minus(have, w));
    }
    Some((a, g))
}

/// One running App whose rule K2 re-sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub app: String,
    pub opened: Vec<u16>,
    pub closed: Vec<u16>,
    pub result: Result<(), String>,
}

/// Re-send the rule of every running App whose ports moved (a database
/// grant or revoke, a database made or dropped, a `DATABASE_URL` that
/// names another one). Live: `apps nft app` replaces only that App's line.
pub(crate) fn refresh_running() -> Vec<Refreshed> {
    let mut apps: Vec<String> = table().lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
    apps.sort();
    apps.into_iter().filter_map(|a| refresh_one(&a)).collect()
}

/// [`refresh_running`] for one App; `None` when nothing moved.
pub(crate) fn refresh_one(app: &str) -> Option<Refreshed> {
    let (a, g) = gap(app)?;
    if g.is_empty() {
        return None;
    }
    let result = super::guarded::install_gateway_rule(app, a.gw);
    Some(Refreshed { app: app.to_string(), opened: g.opened(), closed: g.closed, result })
}

fn ports(p: &[u16]) -> String {
    p.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
}

fn app_row(app: &str) -> Option<AppRow> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, app).ok().flatten()
}

fn tell_agent(app: &AppRow, text: &str) {
    if !app.owner_workspace.is_empty() {
        super::alerts::notify(&app.owner_workspace, &format!("[k2 apps] {text}"));
    }
}

/// After a database change (`k2 db grant|revoke|create|drop`): re-send
/// the running Apps' rules and say what it meant for each in the answer
/// (`apps`); a rule that didn't load says to re-host. Then the database
/// door: open sessions whose grant went are cut now (Postgres would keep
/// them), and said. Failures and cuts reach the App's agent too.
pub(crate) fn after_db_change(v: &mut J) {
    let mut notes: Vec<J> = refresh_running().iter().map(note_for).collect();
    notes.extend(door_cut_notes(&super::db_door::recheck_open()));
    if !notes.is_empty() {
        v["apps"] = json!(notes);
    }
}

fn note_for(r: &Refreshed) -> J {
    let a = r.app.as_str();
    let row = app_row(a);
    let text = match (&r.result, r.opened.is_empty()) {
        (Err(e), _) => format!("App {a}: its door to this box's Postgres didn't reload ({e}). {}", rehost_line(a)),
        (Ok(()), false) => format!("App {a}: its door to Postgres port(s) {} opened now (no re-host needed)", ports(&r.opened)),
        (Ok(()), true) => format!("App {a}: Postgres port(s) {} shut now", ports(&r.closed)),
    };
    if let (Some(row), true) = (&row, r.result.is_err()) {
        tell_agent(row, &text);
    }
    json!({ "app": a, "opened": r.opened, "closed": r.closed, "applied": r.result.is_ok(), "note": text })
}

/// One note per App whose open door sessions were cut.
fn door_cut_notes(cuts: &[super::db_door::Cut]) -> Vec<J> {
    let mut by_app: Vec<(String, Vec<String>)> = Vec::new();
    for c in cuts {
        let what = format!("{} as {}", c.database, c.role);
        match by_app.iter_mut().find(|(a, _)| *a == c.app) {
            Some((_, w)) => w.push(what),
            None => by_app.push((c.app.clone(), vec![what])),
        }
    }
    by_app
        .into_iter()
        .map(|(a, what)| {
            let text = format!(
                "App {a}: {} open database session(s) cut now ({}): it may no longer use them, and new connections are refused",
                what.len(),
                what.join(", ")
            );
            if let Some(row) = app_row(&a) {
                tell_agent(&row, &text);
            }
            json!({ "app": a, "doorCut": what.len(), "note": text })
        })
        .collect()
}

// ── a secret granted to an App ───────────────────────────────────────

/// Is the App idle enough to re-host without asking (no deploy running,
/// no open connection)?
fn idle(app: &AppRow) -> bool {
    !super::supervisor::is_busy(&app.app_id) && !super::handover::busy(&app.app_id) && super::supervisor::active_connections(&app.app_id) == 0
}

/// Start a re-host of `app` in the background; tell its agent if it
/// doesn't finish.
fn kick_rehost(app: &AppRow, what: &str) {
    #[cfg(test)]
    {
        let _ = what;
        test_seams::kicked(&app.app_id);
    }
    #[cfg(not(test))]
    {
        let row = app.clone();
        let granted = what.to_string();
        let spawned = crate::rt::spawn("app-rehost-grant", move || {
            let why = match super::rehost::rehost_now(&row) {
                super::rehost::Turn::Done(_) | super::rehost::Turn::NotDue => return,
                super::rehost::Turn::Failed(_, why) => why,
                super::rehost::Turn::Deferred(code, detail) => format!("{code}: {detail}"),
            };
            tell_agent(&row, &format!("App {}: {granted} was granted to it, and K2's re-host didn't finish ({why}). {}", row.app_id, rehost_line(&row.app_id)));
        });
        if let Err(e) = spawned {
            tell_agent(app, &format!("App {}: {what} was granted to it ({e}). {}", app.app_id, rehost_line(&app.app_id)));
        }
    }
}

/// A secret granted to a published service: when the service is an App,
/// what that means for it (`reach` in the grant's answer, and a note to
/// the App's agent). `env_name`: the variable it arrives as (`None` for a
/// file). `None` for a service that isn't an App.
pub(crate) fn after_service_grant(service_id: &str, env_name: Option<&str>) -> Option<J> {
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::get_app_by_service(&conn, service_id).ok().flatten()?
    };
    let a = app.app_id.clone();
    let what = env_name.unwrap_or("a secret file").to_string();
    let db_url = env_name == Some(crate::vault::publish_env::DATABASE_URL);
    // The rule first: the URL may name another database's cluster.
    let rule = refresh_one(&a);
    if db_url && in_container(&app) && !door_serves(&a) {
        let t = format!("App {a}: {}", engine_no_door(&a));
        tell_agent(&app, &t);
        return Some(json!({ "app": a, "rehost": "no_door", "note": t }));
    }
    if app.live_seed.is_none() || app.state != store::STATE_RUNNING {
        return Some(json!({ "app": a, "rehost": "not_running", "note": format!("App {a} gets {what} when it next starts") }));
    }
    let line = rehost_line(&a);
    if let Some(Refreshed { result: Err(e), .. }) = &rule {
        let t = format!("App {a}: its door to this box's Postgres didn't reload ({e}). {line}");
        tell_agent(&app, &t);
        return Some(json!({ "app": a, "rehost": "needed", "note": t }));
    }
    if db_url && idle(&app) {
        kick_rehost(&app, &what);
        return Some(json!({
            "app": a,
            "rehost": "started",
            "note": format!("{line}. {a} was idle, so K2 is re-hosting it now (k2 app rehost status {a})"),
        }));
    }
    let t = format!("App {a}: {what} was granted to it; the running App keeps what it started with. {line}");
    tell_agent(&app, &t);
    Some(json!({ "app": a, "rehost": "needed", "note": format!("{line} ({a} is running and keeps what it started with)") }))
}

// ── doctor ───────────────────────────────────────────────────────────

/// Does a container App mean to use Postgres (a `DATABASE_URL` naming a
/// K2 database, or `requires.postgres`)?
fn wants_postgres(app: &AppRow) -> bool {
    if super::pg_choice::requires_of(app).is_some() {
        return true;
    }
    matches!(super::pg_choice::db_of(app), super::pg_choice::DbOf::Known(r) if r.source == super::pg_choice::Source::Ref)
}

/// `k2 app doctor <app>`: is everything approved in the running App?
pub(crate) fn doctor_json(app: &AppRow) -> J {
    let a = app.app_id.as_str();
    let line = rehost_line(a);
    let mut findings: Vec<J> = Vec::new();
    let services = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::box_service::list(&conn, a).unwrap_or_default()
    };
    let applied = gap(a);
    if let Some((_, g)) = &applied {
        if !g.box_ports.is_empty() {
            let names: Vec<String> = services.iter().filter(|s| g.box_ports.contains(&s.host_port)).map(|s| s.name.clone()).collect();
            findings.push(json!({
                "id": "box_service_not_applied",
                "detail": format!(
                    "box service approved but not yet applied to the running App: {} (host port {}). {line}",
                    names.join(", "),
                    ports(&g.box_ports)
                ),
            }));
        }
        if !g.db_ports.is_empty() {
            findings.push(json!({
                "id": "database_not_applied",
                "detail": format!("database approved but not yet applied to the running App: Postgres port(s) {}. {line}", ports(&g.db_ports)),
            }));
        }
    }
    let unforwarded = super::box_service_routes::missing_forwards(a);
    if !unforwarded.is_empty() {
        findings.push(json!({
            "id": "box_service_not_forwarded",
            "detail": format!(
                "box service approved but not yet applied to the running App: {} has no forward in its running container. {line}",
                unforwarded.join(", ")
            ),
        }));
    }
    if in_container(app) {
        let ready = super::db_door::engine_ready();
        match super::db_door::live_door(a) {
            // Running without a door, or an engine that won't render one.
            None if wants_postgres(app) && (applied.is_some() || !ready) => {
                let detail = if ready {
                    format!("{a}'s running deployment has no database door (it started before this box's App engine had one). {line}")
                } else {
                    engine_no_door(a)
                };
                findings.push(json!({ "id": "database_door_missing", "detail": detail }));
            }
            // The door is there: does it admit the database the App's
            // DATABASE_URL names?
            Some(_) => {
                if let super::pg_choice::DbOf::Known(r) = super::pg_choice::db_of(app) {
                    if !super::db_door::admitted(a).iter().any(|d| d.database == r.name) {
                        findings.push(json!({
                            "id": "database_not_admitted",
                            "detail": format!(
                                "{a}'s DATABASE_URL names {}, which its database door refuses: its workspace doesn't own it and wasn't \
                                 granted it (k2 db grant <its workspace> --db {} --level read|write)",
                                r.name, r.name
                            ),
                        }));
                    }
                }
            }
            None => {}
        }
    }
    json!({
        "ok": findings.is_empty(),
        "running": applied.is_some(),
        "rule": applied.as_ref().map(|(r, _)| json!({ "gwPort": r.gw, "boxPorts": r.box_ports, "dbPorts": r.db_ports })),
        "dbDoor": super::db_door::live_door(a).map(|p| json!({
            "hostPort": p,
            "inWallPort": k2_root_proto::engine::IN_WALL_DB_PORT,
            "openSessions": super::db_door::open_sessions(a),
            "databases": super::db_door::admitted(a).iter().map(|d| d.database.clone()).collect::<Vec<_>>(),
        })),
        "findings": findings,
    })
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::cell::RefCell;

    thread_local! {
        static SENT: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
        static FAIL: RefCell<Option<String>> = const { RefCell::new(None) };
        static KICKS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// Every rule sent so far (argv), oldest first.
    pub fn sent() -> Vec<Vec<String>> {
        SENT.with(|s| s.borrow().clone())
    }
    /// The next sends fail with this.
    pub fn fail_with(why: Option<&str>) {
        FAIL.with(|f| *f.borrow_mut() = why.map(str::to_string));
    }
    /// Apps K2 started a re-host of.
    pub fn kicks() -> Vec<String> {
        KICKS.with(|k| k.borrow().clone())
    }
    pub(super) fn send(argv: &[String]) -> Result<(), String> {
        if let Some(e) = FAIL.with(|f| f.borrow().clone()) {
            return Err(e);
        }
        SENT.with(|s| s.borrow_mut().push(argv.to_vec()));
        Ok(())
    }
    pub(super) fn kicked(app: &str) {
        KICKS.with(|k| k.borrow_mut().push(app.to_string()));
    }
}

#[cfg(test)]
#[path = "reach_tests.rs"]
mod tests;
