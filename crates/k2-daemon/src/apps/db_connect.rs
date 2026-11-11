//! `k2 app db connect|status|check|disconnect` (prd-app-database-integrations-v1
//! §4.1, §4.2, §4.5, slice S2; 0.46.11): one command connects an App to a
//! box Postgres database and proves it from inside the App.
//!
//! **What connect does** (box databases; outside Postgres is S4):
//! 1. reads the App and its box: K2 runs Postgres on Linux only;
//! 2. picks the database: `--new` creates one for the App (on `--major`,
//!    any major in K2's supported list, or in `--cluster`, a named cluster
//!    of its workspace; never installing a Postgres server: a missing
//!    cluster is refused with the command that makes it), or `--db` names
//!    one the App's workspace owns or was granted;
//! 3. plans the switches an **agent** needs and, when any is off (or the
//!    database is another workspace's), raises **one** held request (one
//!    owner card) listing every switch by its Settings name; approving it
//!    turns them on, standing, and connects (exit 2 until then). People
//!    (the owner, an Admin) connect at once; a Member never does;
//! 4. makes the App's own roles (`sql::ops::provision_app_roles`): a
//!    NOLOGIN group `<db>_app_<app>` and a login `<group>__g<n>` (S1's
//!    naming, `db_door::app_group_role`), never the agent's `{db}_agent`;
//! 5. vaults `DATABASE_URL` (the App's login) and, on a database the App's
//!    workspace owns, `DATABASE_MIGRATE_URL` (the migrator's) as App
//!    secrets granted to the App's service as env, never printed (`--as`
//!    renames both: `ANALYTICS_URL`, `ANALYTICS_MIGRATE_URL`);
//! 6. records the connection (`app_db_connections`, 0294): from then the
//!    database door admits the App's login on that database
//!    (`db_door::connected_roles_in`); a process-exception App's nft rule
//!    is re-sent;
//! 7. re-hosts the running App (secrets arrive only at start) and waits
//!    for its health gate;
//! 8. checks it live (`db_probe`): inside a container App's wall through
//!    the engine's exec, else from the daemon; PASS or FAIL with the step
//!    and the App's last output lines.
//!
//! Only ids, role names and secret names are ever printed. Every connect,
//! check and disconnect is in the App's deploy audit and Access Audit.
//!
//! **Open questions answered by the PRD's recommendations** (defaults until
//! Rosson answers, §9): an agent connects without a card only when every
//! switch is already on and the database is its own workspace's (Q3);
//! `DATABASE_MIGRATE_URL` goes in only for databases the App's workspace
//! owns (Q5); approving the card is the confirm for that App and database
//! once: check and re-host don't ask again (Q6).

use std::collections::HashMap;
use std::time::Duration;

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::{self, store::{self, AppRow}};
use k2_vault::{Consumer, Delivery, GrantRequest, PutMode, PutRequest, Requester, SecretBytes, Selector, Use};
use k2_vault::{Kind, Namespace, Scope, SecretStore as _};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_credential, no, Who, R};

pub const CONNECT: &str = "/cli/app/db/connect";
pub const STATUS: &str = "/cli/app/db/status";
pub const CHECK: &str = "/cli/app/db/check";
pub const DISCONNECT: &str = "/cli/app/db/disconnect";
pub const ROUTES: &[&str] = &[CONNECT, STATUS, CHECK, DISCONNECT];
pub const POST_ROUTES: &[&str] = &[CONNECT, CHECK, DISCONNECT];
/// An approved connect card's replay (`approvals::apply_door`); never a
/// route anyone calls.
pub const APPLY_ROUTE: &str = "/cli/app/db/connect/apply";

/// The env name an App's URL arrives as, by default.
pub const DEFAULT_AS: &str = "DATABASE_URL";
/// The origin K2 stamps on an App's database secrets.
pub const ORIGIN: &str = "system:app-db";
/// The in-wall check's exec timeout.
const CHECK_TIMEOUT_SECS: u64 = 30;
/// The App output lines a FAIL shows.
const OUTPUT_LINES: usize = 20;

pub mod codes {
    /// A usage error (exit 1, R7).
    pub const USAGE: &str = "usage";
    /// This box runs no Postgres (macOS, Windows).
    pub const NO_POSTGRES: &str = "app_db_no_postgres";
    /// The cluster the database would go on isn't on this box.
    pub const CLUSTER_MISSING: &str = "app_db_cluster_missing";
    /// A named cluster of another workspace.
    pub const CLUSTER_NOT_YOURS: &str = "app_db_cluster_not_yours";
    /// A major outside K2's supported list.
    pub const MAJOR_UNSUPPORTED: &str = "app_db_major_unsupported";
    /// `--major` and `--cluster` disagree, or `--new` meets a database of
    /// the App's on another major.
    pub const MAJOR_MISMATCH: &str = "app_db_major_mismatch";
    /// No connection under that name.
    pub const NOT_CONNECTED: &str = "app_db_not_connected";
    /// The name is connected to another database: disconnect first.
    pub const ALREADY_CONNECTED: &str = "app_db_already_connected";
    /// The vault is closed: no App secret can be kept.
    pub const VAULT_CLOSED: &str = "app_db_vault_closed";
    /// Postgres or the vault refused a step; the hint says which.
    pub const FAILED: &str = "app_db_failed";
}

// ── the ask ──────────────────────────────────────────────────────────

/// One `k2 app db connect` (and, kept on a held card, its replay).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ask {
    pub app: String,
    #[serde(default)]
    pub new: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub as_name: Option<String>,
    #[serde(default)]
    pub no_migrate_url: bool,
}

fn usage(hint: impl Into<String>) -> super::routes::Refusal {
    no(codes::USAGE, 400, hint)
}

/// A secret name the App gets its URL as: `[A-Z][A-Z0-9_]*_URL`, at most
/// 40 characters, never K2's own (`K2_*`).
pub fn as_name_ok(n: &str) -> bool {
    n.len() <= 40
        && n.ends_with("_URL")
        && n.len() > 4
        && !n.starts_with("K2_")
        && n.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
        && n.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// `DATABASE_URL` → `DATABASE_MIGRATE_URL`.
pub fn migrate_name(as_name: &str) -> String {
    format!("{}_MIGRATE_URL", as_name.trim_end_matches("_URL"))
}

impl Ask {
    pub fn as_name(&self) -> &str {
        self.as_name.as_deref().unwrap_or(DEFAULT_AS)
    }
}

pub(crate) fn parse_ask(v: &J) -> R<Ask> {
    let mut a: Ask = serde_json::from_value(v.clone()).map_err(|e| usage(format!("unreadable connect request: {e}")))?;
    a.app = a.app.trim().to_ascii_lowercase();
    if a.app.is_empty() {
        return Err(usage("name the App: k2 app db connect <app> --new | --db <name>"));
    }
    let trim = |o: &mut Option<String>| *o = o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    trim(&mut a.db);
    trim(&mut a.cluster);
    trim(&mut a.name);
    trim(&mut a.as_name);
    match (a.new, a.db.is_some()) {
        (true, true) => return Err(usage("--new or --db, not both")),
        (false, false) => return Err(usage("say which database: --new (a new one for the App) or --db <name|id> (one its workspace owns or was granted)")),
        _ => {}
    }
    if !a.new && (a.major.is_some() || a.cluster.is_some() || a.name.is_some()) {
        return Err(usage("--major, --cluster and --name go with --new (an existing database keeps its major and cluster)"));
    }
    if let Some(n) = &a.as_name {
        if !as_name_ok(n) {
            return Err(usage(format!("--as {n}: an env name like ANALYTICS_URL (capitals, digits, _; ends with _URL; not K2_*)")));
        }
    }
    Ok(a)
}

// ── the plan ─────────────────────────────────────────────────────────

/// Where the App's database is, or will be.
#[derive(Debug, Clone)]
pub(crate) enum Want {
    Existing(crate::sql::ops::AppDbTarget),
    New { major: u32, cluster: Option<String> },
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub want: Want,
    /// The App's workspace owns the database (always, for `--new`).
    pub owned: bool,
    /// Its name, or what `--new` will make.
    pub label: String,
    pub major: u32,
    pub cluster: String,
}

fn supported_list() -> String {
    k2_root_proto::pg_major::SUPPORTED.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// Does this box run Postgres for Apps (Linux; the tests' fake on any OS)?
fn postgres_here() -> bool {
    crate::sql::sql_supported() || cfg!(test)
}

pub(crate) fn plan(app: &AppRow, ask: &Ask) -> R<Plan> {
    if !postgres_here() {
        return Err(no(
            codes::NO_POSTGRES,
            409,
            format!("this box runs no Postgres for Apps: K2 runs Postgres on Linux boxes only ({}); use SQLite in $K2_APP_DATA", std::env::consts::OS),
        ));
    }
    let ws = app.owner_workspace.as_str();
    if !ask.new {
        let spec = ask.db.as_deref().unwrap_or_default();
        let t = crate::sql::ops::app_db_target(ws, spec).map_err(ops_refusal)?;
        let (_, major, cluster) = crate::sql::ops::cluster_facts(&t.name).map_err(ops_refusal)?;
        return Ok(Plan { owned: t.owned, label: t.name.clone(), major, cluster, want: Want::Existing(t) });
    }
    let (default, _) = crate::sql::cluster::default_major();
    if let Some(c) = &ask.cluster {
        let cl = crate::sql::cluster::for_name(c).ok_or_else(|| {
            no(
                codes::CLUSTER_MISSING,
                409,
                format!("this box has no Postgres cluster named {c}: make it first (k2 db cluster create <major> --name {c}), then connect again"),
            )
        })?;
        if cl.role != "k2" || cl.origin != "created" {
            return Err(no(codes::CLUSTER_MISSING, 409, format!("{} isn't a cluster K2 made, so K2 never puts an App's database in it", cl.label())));
        }
        if cl.owner_project.as_deref().is_some_and(|o| o != ws) {
            return Err(no(
                codes::CLUSTER_NOT_YOURS,
                403,
                format!("{} belongs to another workspace: only that workspace's Apps get databases in it", cl.label()),
            ));
        }
        if let Some(m) = ask.major.filter(|m| *m != cl.major) {
            return Err(no(codes::MAJOR_MISMATCH, 409, format!("{} is Postgres {}, not {m}: leave out --major, or name a cluster on {m}", cl.label(), cl.major)));
        }
        return Ok(Plan {
            owned: true,
            label: format!("a new database in {}", cl.label()),
            major: cl.major,
            cluster: cl.label(),
            want: Want::New { major: cl.major, cluster: Some(cl.name.clone()) },
        });
    }
    let m = ask.major.unwrap_or(default);
    if !k2_root_proto::pg_major::SUPPORTED.contains(&m) {
        return Err(no(
            codes::MAJOR_UNSUPPORTED,
            409,
            format!("Postgres {m} isn't one K2 runs: K2 creates databases on Postgres {} (k2 db status lists them)", supported_list()),
        ));
    }
    let cl = crate::sql::cluster::for_major(m).filter(|c| c.role == "k2" && c.major > 0).ok_or_else(|| {
        let here: Vec<String> = crate::sql::cluster::k2_clusters().iter().filter(|c| c.major > 0).map(|c| c.label()).collect();
        no(
            codes::CLUSTER_MISSING,
            409,
            format!(
                "Postgres {m} isn't running on this box yet{}: connect never installs a Postgres server. Make it first (k2 db create --major {m}; an agent needs \"Let this agent manage Postgres versions\"), or connect with --major on one this box runs",
                if here.is_empty() { String::new() } else { format!(" (it runs {})", here.join(", ")) }
            ),
        )
    })?;
    Ok(Plan { owned: true, label: format!("a new database on Postgres {m}"), major: m, cluster: cl.label(), want: Want::New { major: m, cluster: None } })
}

fn ops_refusal(e: crate::sql::ops::OpsError) -> super::routes::Refusal {
    let status = e.status().get(..3).and_then(|s| s.parse::<u16>().ok()).unwrap_or(500);
    no(e.code(), status, e.hint().to_string())
}

// ── the switches an agent needs (one card) ───────────────────────────

/// One switch connecting needs: its Settings name and where it is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Switch {
    pub id: &'static str,
    pub name: &'static str,
    #[serde(rename = "where")]
    pub place: String,
    pub on: bool,
}

/// What connecting needs for an agent of `ws` (`path` its folder).
pub(crate) fn switches(ws: &str, path: &str, plan: &Plan) -> Vec<Switch> {
    let handle = super::routes::ws_handle(ws);
    let agent_tab = format!("Settings → Workspaces → {handle} → Agent");
    let mut out = vec![
        Switch { id: "apps_build", name: "Build and run Apps", place: format!("{agent_tab} → Apps"), on: super::access::level(ws) >= super::access::Level::Build },
        Switch {
            id: "app_core_db",
            name: "Database (what its Apps may use)",
            place: format!("{agent_tab} → Apps → Database"),
            on: super::cores::ticked(ws).iter().any(|c| c == "db"),
        },
    ];
    if let Want::New { major, cluster } = &plan.want {
        out.push(Switch {
            id: "db_create",
            name: "Let this agent create databases",
            place: agent_tab.clone(),
            on: k2_core::workspace::settings::db_agent_access_for_path(path) == "write",
        });
        let (default, _) = crate::sql::cluster::default_major();
        if cluster.is_some() || *major != default {
            out.push(Switch {
                id: "db_versions",
                name: "Let this agent manage Postgres versions",
                place: agent_tab.clone(),
                on: crate::sql::versions_access::switch_on(ws),
            });
        }
    }
    out
}

/// Turn the listed switches on for `ws`, as the owner who approved.
fn turn_on(ws: &str, path: &str, ids: &[String], by: &str) -> R<Vec<String>> {
    let mut done = Vec::new();
    for id in ids {
        match id.as_str() {
            "apps_build" => {
                if super::access::level(ws) < super::access::Level::Build {
                    super::access::set_switches(ws, super::access::SwitchChange { build: Some(true), full: None }, true, by)?;
                    done.push(id.clone());
                }
            }
            "app_core_db" => {
                if !super::cores::ticked(ws).iter().any(|c| c == "db") {
                    super::cores::set_cores(ws, &[("db".to_string(), true)], true, by)?;
                    done.push(id.clone());
                }
            }
            "db_create" => {
                if k2_core::workspace::settings::db_agent_access_for_path(path) != "write" {
                    crate::sql::ops::persist_db_access(path, Some("write")).map_err(ops_refusal)?;
                    done.push(id.clone());
                }
            }
            "db_versions" => {
                if !crate::sql::versions_access::switch_on(ws) {
                    crate::sql::versions_access::set_switch(ws, true, true, by).map_err(|r| no(r.code, r.status, r.hint))?;
                    done.push(id.clone());
                }
            }
            _ => {}
        }
    }
    Ok(done)
}

// ── records ──────────────────────────────────────────────────────────

/// One `app_db_connections` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Conn {
    pub app_id: String,
    pub as_name: String,
    pub env: String,
    pub database_id: Option<String>,
    pub database: String,
    pub database_project: String,
    pub cluster: String,
    pub major: u32,
    pub group_role: String,
    pub login_role: String,
    pub generation: u32,
    pub inherit_from: String,
    pub migrate_url: bool,
    pub secrets: Vec<String>,
    pub created_by: String,
    pub created_at: i64,
    pub checked_at: Option<i64>,
    pub check: Option<J>,
}

const CONN_COLS: &str = "app_id, as_name, env, database_id, database_name, database_project, cluster, major, group_role, login_role, \
                         generation, inherit_from, migrate_url, secrets_json, created_by, created_at, checked_at, check_json";

fn conn_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conn> {
    Ok(Conn {
        app_id: r.get(0)?,
        as_name: r.get(1)?,
        env: r.get(2)?,
        database_id: r.get(3)?,
        database: r.get(4)?,
        database_project: r.get(5)?,
        cluster: r.get(6)?,
        major: r.get::<_, i64>(7)?.max(0) as u32,
        group_role: r.get(8)?,
        login_role: r.get(9)?,
        generation: r.get::<_, i64>(10)?.max(1) as u32,
        inherit_from: r.get(11)?,
        migrate_url: r.get::<_, i64>(12)? != 0,
        secrets: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
        created_by: r.get(14)?,
        created_at: r.get(15)?,
        checked_at: r.get(16)?,
        check: r.get::<_, Option<String>>(17)?.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let c = h.lock();
    f(&c).map_err(super::routes::db_err)
}

pub(crate) fn connections_of(app: &str) -> R<Vec<Conn>> {
    db(|c| {
        let mut st = c.prepare(&format!("SELECT {CONN_COLS} FROM app_db_connections WHERE app_id = ?1 ORDER BY as_name, env"))?;
        let rows = st.query_map(rusqlite::params![app], conn_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

fn connection(app: &str, as_name: &str) -> R<Option<Conn>> {
    Ok(connections_of(app)?.into_iter().find(|c| c.as_name == as_name && c.env == "production"))
}

fn save(c: &Conn) -> R<()> {
    db(|x| {
        x.execute(
            &format!(
                "INSERT INTO app_db_connections ({CONN_COLS}, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 'box') \
                 ON CONFLICT(app_id, as_name, env) DO UPDATE SET database_id = excluded.database_id, database_name = excluded.database_name, \
                 database_project = excluded.database_project, cluster = excluded.cluster, major = excluded.major, group_role = excluded.group_role, \
                 login_role = excluded.login_role, generation = excluded.generation, inherit_from = excluded.inherit_from, \
                 migrate_url = excluded.migrate_url, secrets_json = excluded.secrets_json, checked_at = excluded.checked_at, check_json = excluded.check_json"
            ),
            rusqlite::params![
                c.app_id,
                c.as_name,
                c.env,
                c.database_id,
                c.database,
                c.database_project,
                c.cluster,
                c.major as i64,
                c.group_role,
                c.login_role,
                c.generation as i64,
                c.inherit_from,
                c.migrate_url as i64,
                serde_json::to_string(&c.secrets).unwrap_or_else(|_| "[]".into()),
                c.created_by,
                c.created_at,
                c.checked_at,
                c.check.as_ref().map(J::to_string),
            ],
        )
        .map(|_| ())
    })
}

fn save_check(app: &str, as_name: &str, at: i64, check: &J) -> R<()> {
    db(|c| {
        c.execute(
            "UPDATE app_db_connections SET checked_at = ?3, check_json = ?4 WHERE app_id = ?1 AND as_name = ?2 AND env = 'production'",
            rusqlite::params![app, as_name, at, check.to_string()],
        )
        .map(|_| ())
    })
}

fn forget(app: &str, as_name: &str) -> R<()> {
    db(|c| c.execute("DELETE FROM app_db_connections WHERE app_id = ?1 AND as_name = ?2", rusqlite::params![app, as_name]).map(|_| ()))
}

/// Uninstall disconnects everything (§4.6): each login stops logging in
/// and its sessions end, its secrets go, its rows go. The databases stay.
/// Best effort: the App is going either way; a step that fails is logged.
pub(crate) fn forget_for_uninstall(app: &AppRow) {
    let conns = connections_of(&app.app_id).unwrap_or_default();
    let _ = db(|c| c.execute("DELETE FROM app_db_connections WHERE app_id = ?1", rusqlite::params![app.app_id]).map(|_| ()));
    if conns.is_empty() {
        return;
    }
    super::db_door::recheck_open();
    for c in &conns {
        if let Err(e) = crate::sql::routes::revoke_app_roles(&c.database, &c.group_role, std::slice::from_ref(&c.login_role), &c.inherit_from, true, false) {
            k2_core::log_debug!("[apps] uninstall {}: {}'s access to {} not revoked: {}", app.app_id, c.login_role, c.database, e.hint());
        }
        for n in [c.as_name.clone(), migrate_name(&c.as_name)] {
            if c.secrets.contains(&secret_name(&app.app_id, &n)) {
                if let Err(e) = drop_secret(app, &n) {
                    k2_core::log_debug!("[apps] uninstall {}: secret {n} not removed: {}", app.app_id, e.hint);
                }
            }
        }
        audit(&app.app_id, "db.disconnect", "system:uninstall", json!({ "as": c.as_name, "db": c.database, "roles": roles_of(c) }));
    }
}

fn conn_json(c: &Conn) -> J {
    json!({
        "as": c.as_name,
        "env": c.env,
        "kind": "box",
        "db": c.database,
        "dbId": c.database_id,
        "owned": c.inherit_from == format!("{}_agent", c.database),
        "cluster": c.cluster,
        "major": c.major,
        "roles": roles_of(c),
        "secrets": c.secrets,
        "migrateUrl": c.migrate_url,
        "doorOpen": door_open_for(&c.app_id, &c.database),
        "check": c.check,
        "checkedAt": c.checked_at,
        "connectedBy": c.created_by,
        "connectedAt": c.created_at,
    })
}

fn roles_of(c: &Conn) -> Vec<String> {
    let mut r = vec![c.login_role.clone()];
    if c.migrate_url {
        r.push(format!("{}_migrator", c.database));
    }
    r
}

/// Does the App's door admit its login on `db` now (container: the door's
/// admission; process exception: its nft rule names the cluster port)?
fn door_open_for(app: &str, database: &str) -> bool {
    let Some(row) = super::reach::app_row(app) else { return false };
    if super::reach::in_container(&row) {
        return super::reach::door_serves(app) && super::db_door::admitted(app).iter().any(|a| a.database == database);
    }
    // The process exception and the unwalled engine dial the cluster port.
    true
}

// ── vault ────────────────────────────────────────────────────────────

/// `app-db.<app>.<ENV>`: the vault name of an App's database secret
/// (server scope, the owner's; the App's agent never fetches it).
pub fn secret_name(app: &str, env_name: &str) -> String {
    format!("app-db.{app}.{env_name}")
}

fn vault() -> R<std::sync::Arc<k2_vault::LocalVault>> {
    crate::vault::get().map_err(|e| no(codes::VAULT_CLOSED, 503, format!("the vault is closed ({}): an App's database URL lives there. {}", e.code, e.hint)))
}

fn vault_err(what: &str) -> impl Fn(k2_vault::VaultError) -> super::routes::Refusal + '_ {
    move |e| no(codes::FAILED, 502, format!("{what}: {} ({})", e.hint, e.code))
}

/// Keep `value` as the App's `env_name` and grant it to the App's service
/// as that env var; another secret granted under the same env name is
/// replaced (named in the answer). Returns the replaced secret names.
fn keep_and_grant(app: &AppRow, env_name: &str, value: &str, about: &str) -> R<Vec<String>> {
    let v = vault()?;
    let name = secret_name(&app.app_id, env_name);
    // A disconnect disabled it (a destroyed name never comes back): on
    // again before its new value.
    let _ = v.enable(&Selector::user(Scope::server(), name.clone()), &k2_vault::Actor::Owner);
    let mut put = PutRequest::new(Namespace::User, Scope::server(), name.clone(), Kind::Dsn);
    put.default_delivery = Some(Delivery::Env);
    put.delivery_ceiling = Some(Delivery::Env);
    put.origin = ORIGIN.into();
    put.description = about.to_string();
    put.mode = PutMode::Upsert;
    put.human_given_override = Some(false);
    v.put(put, SecretBytes::from_slice(value.as_bytes()), &k2_vault::Actor::Owner).map_err(vault_err("keeping the App's URL"))?;
    let consumer = Consumer::published_service(app.service_id.clone());
    let grants = v.grants_to(&consumer).map_err(vault_err("reading the App's grants"))?;
    let mut replaced = Vec::new();
    let mut have = false;
    for (g, n) in &grants {
        if g.env_name.as_deref() != Some(env_name) {
            continue;
        }
        if n == &name {
            have = true;
            continue;
        }
        v.revoke(&g.id, &k2_vault::Actor::Owner).map_err(vault_err("replacing the App's old grant"))?;
        replaced.push(n.clone());
    }
    if !have {
        v.grant(
            GrantRequest {
                selector: Selector::user(Scope::server(), name),
                consumer,
                delivery: Delivery::Env,
                env_name: Some(env_name.to_string()),
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &k2_vault::Actor::Owner,
        )
        .map_err(vault_err("granting the App its URL"))?;
    }
    Ok(replaced)
}

/// Take an App secret away: its grants to the App's service go and the
/// secret is disabled (never destroyed: a destroyed name can't come back,
/// and a later connect reuses it). Its login is NOLOGIN by then anyway.
fn drop_secret(app: &AppRow, env_name: &str) -> R<()> {
    let v = vault()?;
    let name = secret_name(&app.app_id, env_name);
    if let Ok(grants) = v.grants_to(&Consumer::published_service(app.service_id.clone())) {
        for (g, _) in grants.iter().filter(|(_, n)| *n == name) {
            v.revoke(&g.id, &k2_vault::Actor::Owner).map_err(vault_err("revoking the App's grant"))?;
        }
    }
    match v.disable(&Selector::user(Scope::server(), name), &k2_vault::Actor::Owner) {
        Ok(()) => Ok(()),
        Err(e) if e.code == "not_found" => Ok(()),
        Err(e) => Err(vault_err("removing the App's URL")(e)),
    }
}

/// The URL the App gets for `env_name`, as the App's service resolves it.
fn url_for_app(app: &AppRow, env_name: &str) -> Option<String> {
    let v = crate::vault::get().ok()?;
    let who = Requester::PublishedService { id: app.service_id.clone(), workspace_id: app.owner_workspace.clone() };
    let r = v.resolve(&Selector::user(Scope::server(), secret_name(&app.app_id, env_name)), Use::Env, &who).ok()?;
    std::str::from_utf8(r.value.expose()).ok().map(str::to_string)
}

// ── routes ───────────────────────────────────────────────────────────

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        CONNECT => connect_route(who, body),
        STATUS => status_route(who, params),
        CHECK => check_route(who, body),
        DISCONNECT => disconnect_route(who, body),
        _ => Err(no("not_found", 404, "no such App database route")),
    }
}

/// POST `/cli/app/db/connect {app, new|db, major?, cluster?, name?, as?, noMigrateUrl?, reason?}`.
fn connect_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let ask = parse_ask(&v)?;
    let app = app_for(who, Some(&ask.app))?;
    super::suspend::refuse_if_suspended(&app.app_id)?;
    super::ownership::refuse_if_parked(&app)?;
    let plan = plan(&app, &ask)?;
    // `--new` again names the database an earlier `--new` made: on another
    // major or cluster that is a mismatch, said here in the App's words.
    if let (Want::New { .. }, Some(own)) = (&plan.want, own_new_db(&app)) {
        let (_, m, label) = crate::sql::ops::cluster_facts(&own).map_err(ops_refusal)?;
        if m != plan.major || label != plan.cluster {
            return Err(no(
                codes::MAJOR_MISMATCH,
                409,
                format!(
                    "{}'s own database {own} (an earlier --new made it) is on {label}, not {}: connect it as it is (k2 app db connect {} --db {own}), \
                     or move it (k2 db upgrade {own} --to {})",
                    app.app_id, plan.cluster, app.app_id, plan.major
                ),
            ));
        }
    }
    if let Some(c) = connection(&app.app_id, ask.as_name())? {
        // The same database again is a reconnect (a fresh password, the
        // same login): `--db <it>`, or `--new` when it is the one `--new`
        // made for this App.
        let same = match &plan.want {
            Want::Existing(t) => t.name == c.database,
            Want::New { .. } => own_new_db(&app).as_deref() == Some(c.database.as_str()),
        };
        if !same {
            return Err(no(
                codes::ALREADY_CONNECTED,
                409,
                format!(
                    "{} already has {} on {}: disconnect it first (k2 app db disconnect {} --as {}), or connect another name with --as",
                    app.app_id, c.as_name, c.database, app.app_id, c.as_name
                ),
            ));
        }
    }
    if let Some(ws) = super::routes::agent_ws(who) {
        if !super::approvals::manage_approved_now() {
            let path = super::routes::ws_by_id(ws).map(|w| w.path).unwrap_or_default();
            let needs = switches(ws, &path, &plan);
            if !plan.owned || needs.iter().any(|s| !s.on) {
                return hold(who, &app, &ask, &plan, &needs, v.get("reason").and_then(J::as_str));
            }
        }
    }
    perform(&actor(who), &app, &ask, &plan)
}

/// The database `--new` made for this App (`k2 db create --id app-<app>`).
fn own_new_db(app: &AppRow) -> Option<String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    c.query_row(
        "SELECT name FROM sql_databases WHERE project_id = ?1 AND client_id = ?2 AND status = 'active' ORDER BY created_at DESC LIMIT 1",
        rusqlite::params![app.owner_workspace, format!("app-{}", app.app_id)],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// File the one card: every switch off, by its Settings name.
fn hold(who: &Who, app: &AppRow, ask: &Ask, plan: &Plan, needs: &[Switch], reason: Option<&str>) -> R<J> {
    let off: Vec<&Switch> = needs.iter().filter(|s| !s.on).collect();
    let mut lines = vec![format!("connect App {} to {} (Postgres {}, {}) as {}", app.app_id, plan.label, plan.major, plan.cluster, ask.as_name())];
    for s in &off {
        lines.push(format!("turn on \"{}\" ({})", s.name, s.place));
    }
    if let Want::Existing(t) = &plan.want {
        if !t.owned {
            lines.push(format!(
                "{} belongs to workspace {} (granted to this one, {}): an App of another workspace's database always waits for you",
                t.name,
                super::routes::ws_handle(&t.project_id),
                t.level
            ));
        }
    }
    lines.push(format!(
        "then K2 makes the App's own login, grants {} to it (never printed), re-hosts it and checks it from inside",
        if plan.owned && !ask.no_migrate_url { format!("{} and {}", ask.as_name(), migrate_name(ask.as_name())) } else { ask.as_name().to_string() }
    ));
    let summary = format!(
        "k2 app db connect {} → {}{}",
        app.app_id,
        plan.label,
        if off.is_empty() { String::new() } else { format!(": turn on {}", off.iter().map(|s| format!("\"{}\"", s.name)).collect::<Vec<_>>().join(", ")) }
    );
    let ids: Vec<&str> = off.iter().map(|s| s.id).collect();
    let r = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: Some(app),
        label: &app.app_id,
        workspace: &app.owner_workspace,
        body: json!({ "route": APPLY_ROUTE, "body": { "ask": ask, "switches": ids } }),
        diff: json!({ "lines": lines, "route": APPLY_ROUTE }),
        summary,
        reasons: vec!["connecting an App to a database: the switches it needs, on one card".into()],
        reason,
    })?;
    let mut out = super::approvals::held_answer(&r);
    out["switches"] = json!(needs);
    out["hint"] = json!(format!(
        "one card for everything connecting needs: the owner approves it once (k2 app approve {} from a terminal outside K2, or \
         Settings → App Approvals); then K2 turns those switches on, connects {} and checks it. Wait for it: \
         k2 app db connect {} … --wait, or k2 app db status {}",
        r.id, app.app_id, app.app_id, app.app_id
    ));
    Ok(out)
}

/// The approved card (`approvals::apply_door`, as the owner who approved).
pub(crate) fn apply(r: &reqs::AppRequest, by: &str) -> R<J> {
    let body = r.body.get("body").cloned().unwrap_or(J::Null);
    let ask: Ask = serde_json::from_value(body["ask"].clone()).map_err(|_| usage("unreadable held connect request"))?;
    let ids: Vec<String> = body["switches"].as_array().map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect()).unwrap_or_default();
    let app = app_for(&Who::Owner, Some(&ask.app))?;
    if app.owner_workspace != r.workspace_id {
        return Err(no(codes::FAILED, 409, format!("{} changed hands since the card was raised: connect it again", app.app_id)));
    }
    let path = super::routes::ws_by_id(&app.owner_workspace).map(|w| w.path).unwrap_or_default();
    let turned = turn_on(&app.owner_workspace, &path, &ids, by)?;
    let plan = plan(&app, &ask)?;
    let mut out = perform(by, &app, &ask, &plan)?;
    out["switchesTurnedOn"] = json!(turned);
    out["request"] = json!(r.id);
    Ok(out)
}

/// Connect for real (the plan's checks passed, the caller may).
fn perform(by: &str, app: &AppRow, ask: &Ask, plan: &Plan) -> R<J> {
    let ws = app.owner_workspace.clone();
    let as_name = ask.as_name().to_string();
    // 1. The database.
    let (target, created) = match &plan.want {
        Want::Existing(t) => (t.clone(), false),
        Want::New { major, cluster } => {
            let name = crate::sql::routes::create_for_app_connect(&ws, &app.app_id, *major, cluster.as_deref(), ask.name.as_deref())
                .map_err(|(st, code, hint)| no(code, st, hint))?;
            (crate::sql::ops::app_db_target(&ws, &name).map_err(ops_refusal)?, true)
        }
    };
    let (_, major, cluster) = crate::sql::ops::cluster_facts(&target.name).map_err(ops_refusal)?;
    // 2. The App's roles: the same login on a reconnect; a new name on a
    // database the App already uses gets the next generation (each
    // connection its own login, one group).
    let prev = connection(&app.app_id, &as_name)?;
    let generation = match prev.as_ref().filter(|c| c.database == target.name) {
        Some(c) => c.generation,
        None => connections_of(&app.app_id)?.iter().filter(|c| c.database == target.name).map(|c| c.generation).max().unwrap_or(0) + 1,
    };
    let group = super::db_door::app_group_role(&target.name, &app.app_id);
    let login = super::db_door::app_login_role(&target.name, &app.app_id, generation);
    let password = crate::sql::secrets::generate_secret().map_err(|e| no(codes::FAILED, 500, e))?;
    crate::sql::routes::provision_app_roles(&target, &group, &login, &password).map_err(ops_refusal)?;
    // 3. The App's secrets (never printed).
    let url = crate::sql::ops::app_login_url(&target.name, &login, &password).map_err(ops_refusal)?;
    let mut secrets = vec![secret_name(&app.app_id, &as_name)];
    let mut replaced = keep_and_grant(app, &as_name, &url, &format!("App {}'s database URL ({}, its own login; k2 app db connect)", app.app_id, target.name))?;
    let migrate = target.owned && !ask.no_migrate_url;
    let mig_name = migrate_name(&as_name);
    if migrate {
        let murl = crate::sql::routes::migrator_url(&ws, &target.name).map_err(ops_refusal)?;
        replaced.extend(keep_and_grant(
            app,
            &mig_name,
            &murl,
            &format!("App {}'s migration URL ({}'s migrator; k2 app db connect)", app.app_id, target.name),
        )?);
        secrets.push(secret_name(&app.app_id, &mig_name));
    } else if prev.as_ref().is_some_and(|c| c.migrate_url) {
        drop_secret(app, &mig_name)?;
    }
    // 4. The record: from now the door admits the App's login there.
    let now = deploy::now();
    let rec = Conn {
        app_id: app.app_id.clone(),
        as_name: as_name.clone(),
        env: "production".into(),
        database_id: Some(target.id.clone()),
        database: target.name.clone(),
        database_project: target.project_id.clone(),
        cluster: cluster.clone(),
        major,
        group_role: group.clone(),
        login_role: login.clone(),
        generation,
        inherit_from: target.inherit_from.clone(),
        migrate_url: migrate,
        secrets: secrets.clone(),
        created_by: prev.as_ref().map(|c| c.created_by.clone()).unwrap_or_else(|| by.to_string()),
        created_at: prev.as_ref().map(|c| c.created_at).unwrap_or(now),
        checked_at: None,
        check: None,
    };
    save(&rec)?;
    // 5. The door: a process-exception App's nft rule names the cluster
    // port (re-sent now); a container's door admits by the record.
    let mut door = json!({});
    crate::apps::reach::after_db_change(&mut door);
    audit(&app.app_id, "db.connect", by, json!({
        "as": as_name, "db": target.name, "cluster": cluster, "major": major, "roles": roles_of(&rec), "secrets": secrets,
        "created": created, "owned": target.owned, "replaced": replaced,
    }));
    // 6. Re-host the running App, then 7. check it.
    let rehost = rehost(app);
    let check = check_now(app, &rec, Some(&rehost));
    let mut out = answer(app, &rec, &check);
    out["created"] = json!(created);
    out["rehost"] = rehost.to_json();
    if !replaced.is_empty() {
        out["replaced"] = json!(replaced);
    }
    // A `.env.k2` reference under the same name would give the App two
    // values at start: say so (names only).
    if let Some(seed) = app.live_seed.as_deref() {
        let cwd = k2_core::deploy::seed::seed_path(seed);
        let refs = crate::vault::publish_env::refs_in(&app.service_id, &cwd);
        let dup: Vec<String> = refs.into_iter().map(|(k, _)| k).filter(|k| *k == as_name || *k == mig_name).collect();
        if !dup.is_empty() {
            out["note"] = json!(format!(
                "{}'s .env.k2 also sets {} (a k2v:// reference): remove that line so the App reads its own login, then deploy it",
                app.app_id,
                dup.join(" and ")
            ));
        }
    }
    Ok(out)
}

/// The connect / check answer (ids only).
fn answer(app: &AppRow, c: &Conn, check: &J) -> J {
    json!({
        "ok": check["ok"].as_bool() != Some(false),
        "app": app.app_id,
        "as": c.as_name,
        "env": c.env,
        "kind": kind_of(app),
        "db": c.database,
        "cluster": c.cluster,
        "major": c.major,
        "roles": roles_of(c),
        "secrets": c.secrets,
        "doorOpen": door_open_for(&app.app_id, &c.database),
        "check": check,
        "checkedAt": check["at"],
        "held": null,
    })
}

fn kind_of(app: &AppRow) -> &'static str {
    if app.is_process_exception() {
        "process-exception"
    } else if super::reach::in_container(app) {
        "container"
    } else {
        "process"
    }
}

// ── re-host ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rehost {
    /// Not running: it gets the URL at its next start.
    NotRunning,
    Done(String),
    Failed(String),
}

impl Rehost {
    fn to_json(&self) -> J {
        match self {
            Rehost::NotRunning => json!({ "result": "not_running" }),
            Rehost::Done(d) => json!({ "result": "done", "deployment": d }),
            Rehost::Failed(why) => json!({ "result": "failed", "error": why }),
        }
    }
}

fn running(app: &AppRow) -> bool {
    app.live_seed.is_some() && app.state == store::STATE_RUNNING
}

/// Re-host a running App so its next start carries the new secrets (and
/// wait for its health gate). Everyone re-signs in if its sessions live in
/// memory: connecting a database is the reason to re-host.
fn rehost(app: &AppRow) -> Rehost {
    if !running(app) {
        return Rehost::NotRunning;
    }
    #[cfg(test)]
    if let Some(r) = test_seams::rehost(&app.app_id) {
        return r;
    }
    match super::rehost::rehost_now_yes(app) {
        super::rehost::Turn::Done(d) => Rehost::Done(d),
        super::rehost::Turn::NotDue => Rehost::Done(app.live_deployment.clone().unwrap_or_default()),
        super::rehost::Turn::Failed(_, why) => Rehost::Failed(why),
        super::rehost::Turn::Deferred(code, detail) => Rehost::Failed(format!("{code}: {detail}")),
    }
}

// ── the live check ───────────────────────────────────────────────────

/// Check `c` from inside the App (or from the daemon for an App without a
/// wall), store the result, and answer it. `rehost`: what connect's
/// re-host did (a FAIL says so).
fn check_now(app: &AppRow, c: &Conn, rehost: Option<&Rehost>) -> J {
    let now = deploy::now();
    // The row may have changed (a re-host updates the App).
    let app = app_for(&Who::Owner, Some(&app.app_id)).unwrap_or_else(|_| app.clone());
    let (probe, from) = if !running(&app) {
        if matches!(rehost, Some(Rehost::NotRunning)) {
            let v = json!({
                "ok": null, "skipped": "not_running", "at": now, "from": null,
                "note": format!("{} isn't running: it gets {} at its next start; check it then with k2 app db check {}", app.app_id, c.as_name, app.app_id),
            });
            let _ = save_check(&app.app_id, &c.as_name, now, &v);
            return v;
        }
        (super::db_probe::Probe::fail("app_not_running", format!("{} isn't running (k2 app start {})", app.app_id, app.app_id)), "none")
    } else if let Some(Rehost::Failed(why)) = rehost {
        (super::db_probe::Probe::fail("rehost_failed", format!("the re-host that brings it the URL didn't finish: {why}")), "none")
    } else if super::reach::in_container(&app) {
        (in_wall(&app, &c.as_name), "app")
    } else {
        match url_for_app(&app, &c.as_name) {
            Some(u) => (super::db_probe::probe_url(&u, Duration::from_secs(10)), "daemon"),
            None => (super::db_probe::Probe::fail("no_url", format!("{} isn't in the vault for the App (connect it again)", c.as_name)), "daemon"),
        }
    };
    let mut v = probe.to_json();
    v["at"] = json!(now);
    v["from"] = json!(from);
    v["db"] = json!(c.database);
    v["role"] = json!(c.login_role);
    if !probe.ok {
        v["appOutput"] = json!(app_output(&app));
        v["hint"] = json!(fix_for(&probe, &app, c));
    }
    let _ = save_check(&app.app_id, &c.as_name, now, &v);
    v
}

/// The check inside a container App's wall: the engine runs [`db_probe::PROBE_JS`]
/// as the App (its network, cgroup, user and environment).
///
/// [`db_probe::PROBE_JS`]: super::db_probe::PROBE_JS
fn in_wall(app: &AppRow, as_name: &str) -> super::db_probe::Probe {
    #[cfg(test)]
    if let Some(p) = test_seams::in_wall(&app.app_id, as_name) {
        return p;
    }
    let dep = match super::exec::live_guarded(app) {
        Ok(d) => d,
        Err(e) => return super::db_probe::Probe::fail("app_not_running", e.hint),
    };
    match super::guarded::exec_once(&app.app_id, &dep, &super::db_probe::probe_argv(as_name), CHECK_TIMEOUT_SECS) {
        Ok(r) => {
            let out = r["output"].as_str().unwrap_or_default();
            super::db_probe::parse_marked(out).unwrap_or_else(|| {
                super::db_probe::Probe::fail(
                    "exec_failed",
                    format!("the check inside the App printed no result (exit {}): its image may lack node", r["exitCode"]),
                )
            })
        }
        Err(e) => super::db_probe::Probe::fail("exec_failed", format!("the App engine couldn't run the check: {}", e.message)),
    }
}

/// The App's last output lines (its startup, for a FAIL).
fn app_output(app: &AppRow) -> Vec<String> {
    #[cfg(test)]
    if let Some(l) = test_seams::output(&app.app_id) {
        return l;
    }
    let Some(dep) = app.live_deployment.clone().or_else(|| {
        let h = k2_core::db::shared();
        let c = h.lock();
        store::list_deployments(&c, &app.app_id, 1).ok().and_then(|d| d.into_iter().next()).map(|d| d.id)
    }) else {
        return Vec::new();
    };
    let text = super::routes::read_log(&app.app_id, &dep);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(OUTPUT_LINES)..].iter().map(|l| super::db_probe::scrub(l, 400)).collect()
}

fn fix_for(p: &super::db_probe::Probe, app: &AppRow, c: &Conn) -> String {
    let a = &app.app_id;
    match p.step.as_deref().unwrap_or("") {
        "no_door" if super::reach::in_container(app) && !super::reach::door_serves(a) => super::reach::engine_no_door(a),
        "no_door" => format!("nothing answered for {a}'s database: k2 app doctor {a} shows its door"),
        "door_refused" => format!("the database door refused {}: k2 app doctor {a}", c.login_role),
        "auth_failed" => format!("the login was refused: connect it again (k2 app db connect {a} --db {})", c.database),
        "db_missing" => format!("{} is gone: k2 db list, then connect another (k2 app db disconnect {a} first)", c.database),
        "no_url" => format!("{a} doesn't have {} yet: re-host it (k2 app rehost {a}), then k2 app db check {a}", c.as_name),
        "app_not_running" => format!("start it (k2 app start {a}), then k2 app db check {a}"),
        "rehost_failed" => format!("re-host it (k2 app rehost {a} --yes), then k2 app db check {a}"),
        "exec_failed" => format!("k2 app exec {a} -- node --version shows whether the App can run the check"),
        _ => format!("k2 app logs {a} and k2 app doctor {a}"),
    }
}

/// POST `/cli/app/db/check {app, as?}`: no switch (it changes nothing).
fn check_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, super::routes::s(&v, "app"))?;
    let as_name = super::routes::s(&v, "as").unwrap_or(DEFAULT_AS).to_string();
    let c = connection(&app.app_id, &as_name)?.ok_or_else(|| not_connected(&app.app_id, &as_name))?;
    let check = check_now(&app, &c, None);
    audit(&app.app_id, "db.check", &actor(who), json!({ "as": as_name, "db": c.database, "ok": check["ok"], "step": check["step"] }));
    Ok(answer(&app, &c, &check))
}

fn not_connected(app: &str, as_name: &str) -> super::routes::Refusal {
    no(codes::NOT_CONNECTED, 404, format!("{app} has no database connected as {as_name} (k2 app db status {app}; connect: k2 app db connect {app} --new | --db <name>)"))
}

/// GET `/cli/app/db/status?app=[&request=]`: the App's connections, its
/// kind, and its pending connect cards (and one card's outcome).
fn status_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let conns = connections_of(&app.app_id)?;
    let pending: Vec<J> = db(|c| {
        reqs::list(c, &reqs::Filter { app_id: Some(&app.app_id), status: Some(reqs::STATUS_PENDING), ..Default::default() }, 50)
    })?
    .into_iter()
    .filter(|r| r.body.get("route").and_then(J::as_str) == Some(APPLY_ROUTE))
    .map(|r| json!({ "id": r.id, "summary": r.summary, "requestedBy": r.requested_by, "requestedAt": r.requested_at }))
    .collect();
    let mut out = json!({
        "ok": true,
        "app": app.app_id,
        "kind": kind_of(&app),
        "running": running(&app),
        "postgres": postgres_here(),
        "majors": k2_root_proto::pg_major::SUPPORTED,
        "connections": conns.iter().map(conn_json).collect::<Vec<_>>(),
        "pending": pending,
    });
    if let Some(id) = params.get("request").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let r = db(|c| reqs::find(c, id))?.filter(|r| r.app_label == app.app_id && r.body.get("route").and_then(J::as_str) == Some(APPLY_ROUTE));
        out["request"] = match r {
            Some(r) => json!({ "id": r.id, "status": r.status, "result": r.result, "decidedBy": r.decided_by }),
            None => J::Null,
        };
    }
    Ok(out)
}

/// POST `/cli/app/db/disconnect {app, as?, dropRoles?}`: narrowing, so no
/// switch and no card. The App's login stops logging in and its sessions
/// end now; the door shuts on it; its secrets go; it is re-hosted. The
/// database is never dropped (`--drop-roles` drops the App's roles).
fn disconnect_route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, super::routes::s(&v, "app"))?;
    let as_name = super::routes::s(&v, "as").unwrap_or(DEFAULT_AS).to_string();
    let drop_roles = super::routes::b(&v, "dropRoles");
    let c = connection(&app.app_id, &as_name)?.ok_or_else(|| not_connected(&app.app_id, &as_name))?;
    // The record first: the door refuses the login from the next
    // connection on, then open door sessions are cut.
    forget(&app.app_id, &as_name)?;
    let cut = super::db_door::recheck_open();
    // Its login always stops; the group keeps the workspace's role while
    // another of the App's connections on that database uses it.
    let still_used = connections_of(&app.app_id)?.iter().any(|o| o.database == c.database);
    let pg = crate::sql::routes::revoke_app_roles(&c.database, &c.group_role, std::slice::from_ref(&c.login_role), &c.inherit_from, !still_used, drop_roles);
    let mut secrets_gone = Vec::new();
    for n in [c.as_name.clone(), migrate_name(&c.as_name)] {
        if c.secrets.contains(&secret_name(&app.app_id, &n)) {
            drop_secret(&app, &n)?;
            secrets_gone.push(secret_name(&app.app_id, &n));
        }
    }
    let mut door = json!({});
    crate::apps::reach::after_db_change(&mut door);
    let rehost = rehost(&app);
    audit(&app.app_id, "db.disconnect", &actor(who), json!({
        "as": as_name, "db": c.database, "roles": roles_of(&c), "secrets": secrets_gone, "dropRoles": drop_roles,
        "sessionsCut": cut.iter().filter(|x| x.app == app.app_id).count(), "postgres": pg.as_ref().err().map(|e| e.hint()),
    }));
    let mut out = json!({
        "ok": pg.is_ok(),
        "app": app.app_id,
        "as": as_name,
        "db": c.database,
        "roles": roles_of(&c),
        "rolesDropped": drop_roles && pg.is_ok() && !still_used,
        "secrets": secrets_gone,
        "doorOpen": door_open_for(&app.app_id, &c.database) && still_used,
        "sessionsCut": cut.iter().filter(|x| x.app == app.app_id).count(),
        "rehost": rehost.to_json(),
        "note": format!("{} kept: K2 never drops a database here (k2 db drop --db {} is the owner's)", c.database, c.database),
    });
    if let Err(e) = pg {
        out["error"] = json!({ "code": codes::FAILED, "hint": format!("Postgres didn't take the App's access away: {}", e.hint()) });
    }
    Ok(out)
}

fn audit(app: &str, event: &str, by: &str, payload: J) {
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        if let Err(e) = deploy::audit::append(&c, deploy::audit::SUBJECT_APP, app, event, by, None, &payload) {
            k2_core::log_debug!("[apps] audit {app} {event}: {e}");
        }
    }
    let outcome = format!(
        "app={app} db={} as={} {}",
        payload["db"].as_str().unwrap_or("-"),
        payload["as"].as_str().unwrap_or("-"),
        match payload.get("ok") {
            Some(J::Bool(true)) => "PASS",
            Some(J::Bool(false)) => "FAIL",
            _ => "",
        }
    );
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(&format!("app-{}", event.replace('.', "-")), by, outcome.trim_end().to_string(), "cli", "-", "apps"));
}

// ── a Seed's requires.postgres (§4.8) ────────────────────────────────

/// `k2 app install` with `requires.postgres` (or `--pg-major`), after the
/// App's row exists and before its first start: the same connect as
/// `k2 app db connect <app> --db <name>`, so the App gets its own login
/// and secrets (never the agent's DSN). The install's own gates already
/// ran (and held, for an agent without them). Not running yet: no check.
pub(crate) fn connect_for_install(by: &str, app_id: &str, db_name: &str) -> R<J> {
    let app = app_for(&Who::Owner, Some(app_id))?;
    let ask = Ask {
        app: app.app_id.clone(),
        new: false,
        db: Some(db_name.to_string()),
        major: None,
        cluster: None,
        name: None,
        as_name: None,
        no_migrate_url: false,
    };
    let plan = plan(&app, &ask)?;
    perform(by, &app, &ask, &plan)
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::super::db_probe::Probe;
    use super::Rehost;

    type InWall = Box<dyn Fn(&str, &str) -> Probe>;

    thread_local! {
        static REHOST: RefCell<HashMap<String, Rehost>> = RefCell::new(HashMap::new());
        static REHOSTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static IN_WALL: RefCell<Option<InWall>> = const { RefCell::new(None) };
        static OUTPUT: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
    }

    /// What re-hosting `app` answers (default: done, `d-test`).
    pub fn set_rehost(app: &str, r: Rehost) {
        REHOST.with(|m| m.borrow_mut().insert(app.into(), r));
    }
    pub fn rehosted() -> Vec<String> {
        REHOSTED.with(|v| v.borrow().clone())
    }
    pub(super) fn rehost(app: &str) -> Option<Rehost> {
        REHOSTED.with(|v| v.borrow_mut().push(app.into()));
        Some(REHOST.with(|m| m.borrow().get(app).cloned()).unwrap_or_else(|| Rehost::Done("d-test".into())))
    }
    /// The in-wall check for this thread's Apps: `(app, env name)`.
    pub fn set_in_wall(f: Option<InWall>) {
        IN_WALL.with(|c| *c.borrow_mut() = f);
    }
    pub(super) fn in_wall(app: &str, as_name: &str) -> Option<Probe> {
        IN_WALL.with(|c| c.borrow().as_ref().map(|f| f(app, as_name)))
    }
    pub fn set_output(app: &str, lines: Vec<String>) {
        OUTPUT.with(|m| m.borrow_mut().insert(app.into(), lines));
    }
    pub(super) fn output(app: &str) -> Option<Vec<String>> {
        OUTPUT.with(|m| m.borrow().get(app).cloned())
    }
}

#[cfg(test)]
#[path = "db_connect_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "db_connect_real_pg.rs"]
mod real_pg_connect;
