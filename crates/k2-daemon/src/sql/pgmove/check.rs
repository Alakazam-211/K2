//! `k2 db upgrade --check` (§6.1): read-only, and step 1 of every move.
//! Each line is ok / warn / refuse with a hint. Any refuse = the move
//! doesn't start (`aborted`, nothing changed).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use k2_root_proto::pg_major::{check_move, k2_layout, NEW_INSTALL};

use super::catalog;
use super::{free_band_port, port_of, roles_to_copy, ClusterOps, ClusterRole, DbRecord, Major, MoveEnv, MoveOpts, SourceState, Users};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    Warn,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckLine {
    pub id: String,
    pub level: Level,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub db: String,
    pub from: Major,
    pub to: Major,
    pub from_port: Option<u16>,
    /// The target cluster's port when it already exists.
    pub to_port: Option<u16>,
    /// Where a new target cluster would go.
    pub new_port: Option<u16>,
    pub size_bytes: u64,
    pub extensions: Vec<String>,
    pub roles: Vec<String>,
    pub pause_secs: (u64, u64),
    pub users: Users,
    pub lines: Vec<CheckLine>,
}

impl Report {
    pub fn refused(&self) -> bool {
        self.lines.iter().any(|l| l.level == Level::Refuse)
    }
}

struct Lines(Vec<CheckLine>);

impl Lines {
    fn push(&mut self, id: &str, level: Level, text: impl Into<String>, hint: Option<&str>) {
        self.0.push(CheckLine { id: id.into(), level, text: text.into(), hint: hint.map(str::to_string) });
    }
    fn ok(&mut self, id: &str, text: impl Into<String>) {
        self.push(id, Level::Ok, text, None);
    }
    fn warn(&mut self, id: &str, text: impl Into<String>, hint: &str) {
        self.push(id, Level::Warn, text, Some(hint));
    }
    fn refuse(&mut self, id: &str, text: impl Into<String>, hint: &str) {
        self.push(id, Level::Refuse, text, Some(hint));
    }
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

/// Build the report. `Err` only when a read itself failed (the move then
/// doesn't start either).
pub fn run(ops: &dyn ClusterOps, env: &dyn MoveEnv, rec: &DbRecord, opts: &MoveOpts) -> Result<Report, String> {
    let mut l = Lines(Vec::new());
    // The box's clusters into K2's records first: an unrecorded cluster
    // is someone else's (adopted) until the owner says otherwise (F9).
    env.refresh_clusters();
    let from = rec.major;
    let to = opts.to.unwrap_or(NEW_INSTALL);
    let db = rec.name.as_str();

    // Locks: the helper speaks the majors verbs.
    if opts.helper_ok {
        l.ok("helper", "root helper speaks protocol 9 (pgmajor)");
    } else {
        l.refuse("helper", "the root helper is older than protocol 9", "update the root helper: k2 root-helper update");
    }

    // Majors.
    match check_move(from, to, false, env.now().div_euclid(86_400)).err().map(|r| r.code()) {
        None => l.ok("majors", format!("{from} → {to}")),
        Some(why) => l.refuse("majors", format!("{from} → {to}: {why}"), "moves go up to a supported major (16, 17)"),
    }
    match rec.moved_from {
        Some(mf) if mf >= rec.major => l.refuse(
            "record",
            format!("the record says {db} moved from {mf} but lives on {}", rec.major),
            "inconsistent_record: nothing is deleted or moved until the owner reviews it (inbox, k2 db upgrade --check)",
        ),
        Some(mf) if rec.source_state == SourceState::Kept => l.refuse(
            "kept",
            format!("an earlier move's {mf} copy is still kept for its 14-day rollback"),
            "once the database is healthy on its new major, the owner may delete the old copy: k2 db upgrade <db> --forget-old",
        ),
        _ => {}
    }

    // Source cluster: K2's, the Debian layout, running.
    let rows = ops.clusters()?;
    let src = rows.iter().find(|r| r.major == from && r.name == "main");
    let from_port = src.map(|r| r.port);
    match src {
        None => l.refuse("source", format!("no {from}/main cluster on this box"), "k2 db doctor"),
        Some(r) if !k2_layout(r) => l.refuse("source", "the source cluster lives outside /var/lib/postgresql/<M>/main", "K2 never moves out of a foreign layout"),
        Some(r) if !r.online() => l.refuse("source", format!("{from}/main is not running"), "k2 db doctor"),
        Some(r) => match env.cluster_role(from) {
            ClusterRole::K2 => l.ok("source", format!("{from}/main on port {}", r.port)),
            ClusterRole::Adopted if opts.allow_adopted => l.warn("source", format!("{from}/main is an adopted cluster"), "the owner passed --allow-adopted"),
            ClusterRole::Adopted => l.refuse("source", format!("{from}/main is a cluster set up outside K2"), "owner only: --allow-adopted"),
            ClusterRole::Foreign => l.refuse("source", format!("{from}/main is not K2's cluster"), "K2 never moves a foreign cluster's databases"),
        },
    }

    // Target cluster: exists, or a free port in the band.
    let installed = ops.installed()?;
    let to_port = port_of(&rows, to);
    let mut new_port = None;
    match to_port {
        Some(p) => {
            if env.cluster_role(to) == ClusterRole::Adopted && !opts.allow_adopted {
                l.refuse("target", format!("{to}/main is a cluster set up outside K2"), "owner only: --allow-adopted");
            } else {
                l.ok("target", format!("{to}/main exists on port {p}"));
            }
            if rows.iter().any(|r| r.major == to && r.online()) {
                let have = ops.sql(to, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = {};\n", catalog::quote_lit(db)))?;
                if have.trim() != "0" {
                    l.refuse("target", format!("a database named {db} already exists on {to}"), "an earlier failed move? k2 db doctor");
                }
            }
        }
        None => {
            let listening = ops.band_listening()?;
            match free_band_port(&rows, &listening) {
                Some(p) => {
                    new_port = Some(p);
                    l.ok("target", format!("{to}/main will be created on port {p}"));
                }
                None => l.refuse("target", "no free port in 5432–5449", "free a port in K2's band"),
            }
        }
    }
    if !installed.contains(&to) {
        l.ok("install", format!("postgresql-{to} will be installed (PGDG, pinned key)"));
    }

    // One kept rollback copy per database at most (F7).
    if let Ok(copies) = super::rollback_copies(ops, db) {
        if !copies.is_empty() {
            l.refuse(
                "kept",
                format!("a rolled-back copy is kept: {}", copies.iter().map(|(m, n)| format!("{m}/{n}")).collect::<Vec<_>>().join(", ")),
                "the owner deletes it first: k2 db upgrade <db> --forget-old",
            );
        }
    }

    // Facts from the database itself.
    let facts: Value = if src.is_some_and(|r| r.online()) {
        let t = ops.sql(from, db, &catalog::facts_sql(db))?;
        serde_json::from_str(&t).map_err(|_| "check facts are not JSON".to_string())?
    } else {
        Value::Null
    };
    let size = n(&facts, "size");
    let extensions: Vec<String> = facts["extensions"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    for x in &extensions {
        match ops.extension_available(to, x) {
            Some(true) => l.ok("extensions", format!("{x}: {to} build present")),
            Some(false) => l.refuse("extensions", format!("{x}: no {to} build"), "install the extension's package for the target major"),
            None => l.ok("extensions", format!("{x}: checked once postgresql-{to} is installed")),
        }
    }
    for (k, what) in [
        ("eventTriggers", "event triggers"),
        ("publications", "publications"),
        ("subscriptions", "subscriptions"),
        ("tablespaces", "objects in a non-default tablespace"),
    ] {
        if n(&facts, k) > 0 {
            l.refuse("objects", format!("{} {what}", n(&facts, k)), "a single-database copy can't carry these; remove them or ask the owner");
        }
    }
    if n(&facts, "largeObjects") > 0 {
        l.ok("objects", format!("{} large objects (carried by the dump)", n(&facts, "largeObjects")));
    }
    if n(&facts, "dbSettings") > 0 {
        l.warn("objects", format!("{} ALTER DATABASE/ROLE … SET for this database", n(&facts, "dbSettings")), "recreated on the target");
    }

    // Roles (V32): every one recreated with its verifier; a shared login
    // with another password in the target is a refusal.
    let mut roles = Vec::new();
    if src.is_some_and(|r| r.online()) {
        match roles_to_copy(ops, from, rec) {
            Ok(r) => roles = r,
            Err(e) => l.refuse("roles", e, "rename the role or ask the owner"),
        }
        if !roles.is_empty() {
            if to_port.is_some() && rows.iter().any(|r| r.major == to && r.online()) {
                let dry = ops.copy_roles(from, to, &roles, true)?;
                let list = |k: &str| -> Vec<String> {
                    dry[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
                };
                for r in list("same") {
                    l.ok("roles", format!("{r}: in target: yes, same verifier"));
                }
                for r in list("wouldCreate") {
                    l.ok("roles", format!("{r}: in target: no; created with the same verifier"));
                }
                for r in list("mismatch") {
                    l.refuse("roles", format!("{r}: in target with another password"), "one login can't hold two passwords; rotate it on both clusters first");
                }
                for r in list("refused") {
                    l.refuse("roles", format!("{r}: superuser/replication/BYPASSRLS or an unsafe stored password"), "K2 never copies such a role");
                }
                for r in list("missing") {
                    l.refuse("roles", format!("{r}: not in the source cluster"), "k2 db doctor");
                }
            } else {
                l.ok("roles", format!("{} role(s) created in the new cluster with the same verifiers", roles.len()));
            }
            let md5 = ops.sql(from, "postgres", &catalog::md5_roles_sql(&roles))?;
            let md5: Vec<String> = serde_json::from_str(&md5).unwrap_or_default();
            if !md5.is_empty() {
                l.warn("roles", format!("MD5 passwords: {}", md5.join(", ")), "carried as they are; rotate to SCRAM (k2 db rotate)");
            }
        }
    }

    // Disk (V28): 2 × size + 1 GiB free.
    let need = catalog::disk_needed(size);
    let free = ops.free_bytes()?;
    let kept = ops.moves_bytes();
    if kept > 0 {
        l.ok("disk", format!("{} MiB in kept move dumps (removed by --forget-old or a rollback)", kept >> 20));
    }
    if free >= need {
        l.ok("disk", format!("{} MiB free, {} MiB needed", free >> 20, need >> 20));
    } else {
        l.refuse("disk", format!("{} MiB free, {} MiB needed (2 × the database + 1 GiB)", free >> 20, need >> 20), "free disk space first");
    }

    // Users (V35–V37).
    let users = env.users(db);
    if !users.apps.is_empty() {
        l.ok("users", format!("Apps stopped and restarted on the new port: {}", users.apps.join(", ")));
    }
    for d in &users.literal_dsns {
        l.warn("users", format!("a literal DSN K2 can't rewrite: {d}"), "point it at `k2 db dsn --ref` before the move, or fix it after");
    }
    let shared = env.shared_with(db);
    if !shared.is_empty() {
        l.warn(
            "users",
            format!("shared with {} other workspace(s): their Apps restart and their DATABASE_URL is re-pointed too", shared.len()),
            "a rollback of a shared database is the owner's",
        );
    }
    if users.guest_doors > 0 {
        l.ok("users", format!("{} guest door(s) answer 503 db_maintenance during the pause", users.guest_doors));
    }
    if n(&facts, "sessions") > 0 {
        l.ok("users", format!("{} open session(s) end at the pause", n(&facts, "sessions")));
    }

    let pause = catalog::pause_estimate_secs(size);
    l.ok("pause", format!("about {}–{} s for {} MiB (Apps stopped, doors 503)", pause.0, pause.1, size >> 20));
    l.ok("sew", "Sew's own Postgres is untouched");

    Ok(Report {
        db: db.to_string(),
        from,
        to,
        from_port,
        to_port,
        new_port,
        size_bytes: size,
        extensions,
        roles,
        pause_secs: pause,
        users,
        lines: l.0,
    })
}
