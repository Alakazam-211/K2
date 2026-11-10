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
//! - **No door (containers, guarded or open).** A container has its own
//!   loopback; K2 forwards only the gateway link, the egress proxy and box
//!   services into it. Its database door (`-T 5432:<cluster port>`,
//!   `StartSpec::db_port`, S21) isn't built: `db_port` is always `None`.
//!   No approval or re-host opens it, so K2 says so up front
//!   ([`container_no_db`]): `--db-raw on` and a Postgres box service are
//!   refused for a container App, and a database grant or a `DATABASE_URL`
//!   for one answers with the same words.
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
    /// A container App asked for something only a database door gives.
    pub const DB_DOOR_MISSING: &str = "app_db_door_missing";
}

/// The re-host line every "only a start delivers it" answer carries.
pub fn rehost_line(app: &str) -> String {
    format!("Takes effect after a re-host: k2 app rehost {app}")
}

/// Why a container App can't reach box Postgres (S21 isn't built).
pub fn container_no_db(app: &str) -> String {
    format!(
        "guarded container Apps can't reach box Postgres yet: {app} runs in a container, where nothing answers on \
         127.0.0.1:5432 (its database door isn't built), and no approval or re-host changes that. Use a process App \
         (the owner turns it on: k2 app exception {app} on) or SQLite"
    )
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
/// host ports, and (per-App table) its database ports.
pub(crate) fn wanted(app: &str, per_app_db: bool) -> (Vec<u16>, Option<Vec<u16>>) {
    (super::box_service_routes::allowed_ports(app), per_app_db.then(|| super::pg_choice::door_ports(app)))
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

/// The running App's gap (`None`: K2 sent it no rule, it isn't running).
/// A rule sent without `--db` on the per-App table admits the whole band,
/// so it lacks no database port.
pub fn gap(app: &str) -> Option<(Applied, Gap)> {
    let a = applied_of(app)?;
    let (b, d) = wanted(app, per_app_db());
    let mut g = Gap { box_ports: minus(&b, &a.box_ports), closed: minus(&a.box_ports, &b), ..Gap::default() };
    if let (Some(w), Some(have)) = (&d, &a.db_ports) {
        g.db_ports = minus(w, have);
        g.closed.extend(minus(have, w));
    }
    Some((a, g))
}

// Red before the fix: today's behaviour at each point the fix lands.

/// A database change says nothing and re-sends nothing today.
pub(crate) fn after_db_change(_v: &mut J) {}

/// A secret granted to an App says nothing about when it arrives today.
pub(crate) fn after_service_grant(_service_id: &str, _env_name: Option<&str>) -> Option<J> {
    None
}

/// Nothing checks the running App today.
pub(crate) fn doctor_json(app: &AppRow) -> J {
    let a = applied_of(&app.app_id);
    json!({
        "ok": true,
        "running": a.is_some(),
        "rule": a.as_ref().map(|r| json!({ "gwPort": r.gw, "boxPorts": r.box_ports, "dbPorts": r.db_ports })),
        "findings": [],
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
