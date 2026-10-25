//! Per-App Postgres major choice (prd-app-pg-choice-dualrun-v1 Part A,
//! 0.46.4).
//!
//! - **The App's database (PA1, Q1)** is the K2 database its
//!   `DATABASE_URL` names, read from the sources a clean start reads (its
//!   live Seed's `.env.k2` references, then its granted env secrets;
//!   `publish_env::database_url`). With none, it is the owner workspace's
//!   default database (the oldest active one, what `k2 db dsn --ref`
//!   vaults). Resolved live: no column. When the App runtime's own
//!   `app_<id>` databases land (S21), the same resolution names them.
//! - **`requires.postgres` (PA4–PA6)**: whole-major ranges. Install,
//!   deploy and re-host refuse an App whose database is on a major outside
//!   its range (`requires_unmet`, 409); a range no supported major meets is
//!   `major_unsupported` (R4: 18 stays off). Rollbacks and restarts don't
//!   check (as `requires.sew`).
//! - **One database, several Apps (PA3)**: [`apps_using`] is every App of
//!   the database's owner workspace and grantee workspaces whose database
//!   resolves to it; `k2 db upgrade` needs every one of their ranges to
//!   hold.
//! - **Install (PA7, Q2)**: [`install_plan`] decides: nothing, create the
//!   database on a major (then its `DATABASE_URL` reference), or refuse.
//!   Install never moves a database.

use k2_core::deploy::manifest::Manifest;
use k2_core::deploy::store::{self, AppRow};
use k2_core::deploy::validate::pg_range_ok;
use k2_root_proto::pg_major::SUPPORTED;
use serde::Serialize;
use serde_json::{json, Value as J};

/// Where [`db_of`] found the App's database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// Its `DATABASE_URL` names it.
    Ref,
    /// No `DATABASE_URL`: the owner workspace's default database.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbRef {
    pub name: String,
    /// The workspace that owns the database.
    pub project_id: String,
    pub major: u32,
    pub source: Source,
}

/// The answer of [`db_of`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbOf {
    /// No database at all (no `DATABASE_URL`, no workspace database, or
    /// no Postgres on this OS).
    None,
    Known(DbRef),
    /// A `DATABASE_URL` K2 can't map to one of its databases (unreadable,
    /// or naming a database K2 doesn't manage).
    Unknown(String),
}

/// What the App's `DATABASE_URL` sources say (the pure input of [`resolve`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlLookup {
    Absent,
    Value(String),
    Unreadable,
}

/// The database name in a Postgres URL (`postgres[ql]://…/<db>[?…]`).
pub fn dbname_from_dsn(dsn: &str) -> Option<String> {
    let rest = dsn.strip_prefix("postgres://").or_else(|| dsn.strip_prefix("postgresql://"))?;
    let after_host = rest.split_once('/')?.1;
    let name = after_host.split(['?', '#']).next()?.trim();
    k2_root_proto::verbs::DbName::parse(name).map(|_| name.to_string())
}

/// Pure: the App's database from its URL lookup, K2's records
/// (`known(name) → (owner project, major)`) and the owner workspace's
/// default database.
pub fn resolve(url: &UrlLookup, known: &dyn Fn(&str) -> Option<(String, u32)>, default: Option<(String, String, u32)>) -> DbOf {
    match url {
        UrlLookup::Value(dsn) => match dbname_from_dsn(dsn) {
            Some(name) => match known(&name) {
                Some((project_id, major)) => DbOf::Known(DbRef { name, project_id, major, source: Source::Ref }),
                None => DbOf::Unknown(format!("its DATABASE_URL names {name}, which isn't a K2 database on this server")),
            },
            None => DbOf::Unknown("its DATABASE_URL isn't a Postgres URL K2 can read".into()),
        },
        UrlLookup::Unreadable => DbOf::Unknown("its DATABASE_URL reference doesn't resolve now (k2 vault doctor)".into()),
        UrlLookup::Absent => match default {
            Some((name, project_id, major)) => DbOf::Known(DbRef { name, project_id, major, source: Source::Default }),
            None => DbOf::None,
        },
    }
}

/// K2's record of an active database: (owner project, major). A NULL
/// major is the 5432 cluster's.
pub fn known_db_in(conn: &rusqlite::Connection, name: &str) -> Option<(String, u32)> {
    let (project, major): (String, Option<i64>) = conn
        .query_row(
            "SELECT project_id, cluster_major FROM sql_databases WHERE name = ?1 AND status = 'active' ORDER BY created_at DESC LIMIT 1",
            rusqlite::params![name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    let major = match major {
        Some(m) if m > 0 => m as u32,
        _ => crate::sql::cluster::primary_in(conn).major,
    };
    Some((project, major))
}

/// The owner workspace's default database: (name, project, major).
pub fn default_db_in(conn: &rusqlite::Connection, project_id: &str) -> Option<(String, String, u32)> {
    let name = crate::sql::ops::active_default_name_in(conn, project_id)?;
    let (project, major) = known_db_in(conn, &name)?;
    Some((name, project, major))
}

/// What the App's `DATABASE_URL` will be at its next start.
fn url_lookup(app: &AppRow) -> UrlLookup {
    let Some(seed) = app.live_seed.as_deref() else { return UrlLookup::Absent };
    let cwd = k2_core::deploy::seed::seed_path(seed);
    match crate::vault::publish_env::database_url(&app.service_id, &app.owner_workspace, &cwd) {
        crate::vault::publish_env::DbUrl::Absent => UrlLookup::Absent,
        crate::vault::publish_env::DbUrl::Value(v) => UrlLookup::Value(v),
        crate::vault::publish_env::DbUrl::Unreadable => UrlLookup::Unreadable,
    }
}

/// PA1: the App's database, live.
pub fn db_of(app: &AppRow) -> DbOf {
    if !crate::sql::sql_supported() && !cfg!(test) {
        return DbOf::None;
    }
    let url = url_lookup(app);
    let k2db = k2_core::db::shared();
    let conn = k2db.lock();
    resolve(&url, &|n| known_db_in(&conn, n), default_db_in(&conn, &app.owner_workspace))
}

/// The live Seed's `requires.postgres`.
pub fn range_of(app: &AppRow) -> Option<String> {
    let seed = app.live_seed.as_deref()?;
    let row = {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        store::get_seed(&conn, seed).ok().flatten()?
    };
    serde_json::from_value::<Manifest>(row.manifest).ok()?.requires?.postgres
}

fn grantee_projects_in(conn: &rusqlite::Connection, db: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut st) = conn.prepare(
        "SELECT g.project_id FROM sql_grants g JOIN sql_databases d ON d.id = g.database_id \
         WHERE d.name = ?1 AND d.status = 'active' ORDER BY g.project_id",
    ) {
        if let Ok(rows) = st.query_map(rusqlite::params![db], |r| r.get::<_, String>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
}

/// One App that uses a database, with its live Seed's range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub app: String,
    pub requires: Option<String>,
}

/// PA3: every App (any state) of the database's owner workspace and of
/// its grantee workspaces whose database resolves to `db`.
pub fn apps_using(db: &str) -> Vec<User> {
    let candidates: Vec<AppRow> = {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        let Some((owner, _)) = known_db_in(&conn, db) else { return Vec::new() };
        let mut projects = vec![owner];
        projects.extend(grantee_projects_in(&conn, db));
        let mut v = Vec::new();
        for p in projects {
            v.extend(store::list_apps(&conn, Some(&p)).unwrap_or_default());
        }
        v
    };
    let mut out: Vec<User> = candidates
        .iter()
        .filter(|a| matches!(db_of(a), DbOf::Known(r) if r.name == db))
        .map(|a| User { app: a.app_id.clone(), requires: range_of(a) })
        .collect();
    out.sort_by(|a, b| a.app.cmp(&b.app));
    out.dedup();
    out
}

/// Supported majors a range admits, lowest first.
pub fn supported_in(range: &str) -> Vec<u32> {
    SUPPORTED.iter().copied().filter(|m| pg_range_ok(range, *m) == Ok(true)).collect()
}

/// PA6: the highest supported major every range admits.
pub fn best_major<'a>(ranges: impl IntoIterator<Item = &'a str> + Clone) -> Option<u32> {
    SUPPORTED
        .iter()
        .rev()
        .copied()
        .find(|m| ranges.clone().into_iter().all(|r| pg_range_ok(r, *m) == Ok(true)))
}

/// A refusal of this module: `(code, status, hint, extra)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Unmet {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
    pub extra: J,
}

pub const MAJOR_UNSUPPORTED: &str = "major_unsupported";

fn unsupported(range: &str) -> Unmet {
    let sup = SUPPORTED.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    Unmet {
        code: MAJOR_UNSUPPORTED,
        status: 409,
        hint: format!(
            "requires.postgres \"{range}\" is met by no Postgres major K2 runs ({sup}); 18 stays off until K2 supports it. Widen the range"
        ),
        extra: json!({ "requires": range, "supported": SUPPORTED }),
    }
}

/// The move that would satisfy `range` for a database on `major`.
fn fix_for(db: &str, major: u32, range: &str) -> String {
    match supported_in(range).into_iter().rev().find(|m| *m > major) {
        Some(to) => format!("move the database: k2 db upgrade {db} --to {to} (add --live to keep it serving while it copies)"),
        None => format!(
            "K2 moves databases only to a newer major, and no supported major above {major} meets \"{range}\": widen the App's range, or give it a database on an older major"
        ),
    }
}

/// PA5, pure: the App's database against its range.
pub fn check(app: &str, range: &str, db: &DbOf) -> Result<(), Unmet> {
    if let Err(e) = pg_range_ok(range, 0) {
        return Err(Unmet {
            code: k2_core::deploy::codes::REQUIRES_UNMET,
            status: 409,
            hint: format!("{app}'s requires.postgres is not a range ({e}); fix the manifest (k2 app validate)"),
            extra: json!({ "requires": range }),
        });
    }
    if supported_in(range).is_empty() {
        return Err(unsupported(range));
    }
    match db {
        // A database K2 doesn't manage: K2 can't see its major.
        DbOf::Unknown(_) => Ok(()),
        DbOf::None => Err(Unmet {
            code: k2_core::deploy::codes::REQUIRES_UNMET,
            status: 409,
            hint: format!(
                "{app} needs a Postgres {range} database and its workspace has none. Create one: k2 db create --major {m}, then k2 db dsn --ref (or install with --pg-major {m})",
                m = best_major([range]).unwrap_or(k2_root_proto::pg_major::NEW_INSTALL)
            ),
            extra: json!({ "requires": range, "database": J::Null }),
        }),
        DbOf::Known(r) if pg_range_ok(range, r.major) == Ok(true) => Ok(()),
        DbOf::Known(r) => Err(Unmet {
            code: k2_core::deploy::codes::REQUIRES_UNMET,
            status: 409,
            hint: format!(
                "{app} needs Postgres {range}, and its database {} is on Postgres {}: {}",
                r.name,
                r.major,
                fix_for(&r.name, r.major, range)
            ),
            extra: json!({ "requires": range, "database": { "name": r.name, "major": r.major } }),
        }),
    }
}

/// PA5, live: an App's Seed `requires.postgres` against its database.
pub fn check_live(app: &AppRow, range: Option<&str>) -> Result<(), Unmet> {
    let Some(range) = range else { return Ok(()) };
    if !crate::sql::sql_supported() && !cfg!(test) {
        return Err(Unmet {
            code: k2_core::deploy::codes::REQUIRES_UNMET,
            status: 409,
            hint: format!("{} needs Postgres {range}; K2 runs Postgres on Linux boxes only", app.app_id),
            extra: json!({ "requires": range }),
        });
    }
    check(&app.app_id, range, &db_of(app))
}

// ── install (PA7) ─────────────────────────────────────────────────────

/// What install does about the App's database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallDb {
    Nothing,
    /// The owner workspace has one that fits: `(name, major)`.
    Fits(String, u32),
    /// Create the workspace's database on this major.
    Create(u32),
}

/// PA7, pure: `range` = the Seed's `requires.postgres`, `pick` = the
/// install's `pgMajor`, `existing` = the owner workspace's database.
pub fn install_plan(app: &str, range: Option<&str>, pick: Option<u32>, existing: Option<(&str, u32)>) -> Result<InstallDb, Unmet> {
    if let Some(m) = pick {
        if !SUPPORTED.contains(&m) {
            let sup = SUPPORTED.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
            return Err(Unmet {
                code: MAJOR_UNSUPPORTED,
                status: 400,
                hint: format!("--pg-major {m}: K2 creates databases on Postgres {sup}"),
                extra: json!({ "pgMajor": m, "supported": SUPPORTED }),
            });
        }
    }
    if let Some(r) = range {
        if let Err(e) = pg_range_ok(r, 0) {
            return Err(Unmet {
                code: k2_core::deploy::codes::REQUIRES_UNMET,
                status: 409,
                hint: format!("{app}'s requires.postgres is not a range ({e}); fix the manifest (k2 app validate)"),
                extra: json!({ "requires": r }),
            });
        }
        if supported_in(r).is_empty() {
            return Err(unsupported(r));
        }
        if let Some(m) = pick {
            if pg_range_ok(r, m) != Ok(true) {
                return Err(Unmet {
                    code: k2_core::deploy::codes::REQUIRES_UNMET,
                    status: 409,
                    hint: format!("--pg-major {m} is outside {app}'s requires.postgres \"{r}\": leave it out, or pick {}", supported_in(r).iter().map(u32::to_string).collect::<Vec<_>>().join(" or ")),
                    extra: json!({ "requires": r, "pgMajor": m }),
                });
            }
        }
    }
    if range.is_none() && pick.is_none() {
        return Ok(InstallDb::Nothing);
    }
    match existing {
        Some((name, major)) => {
            let range_ok = range.is_none_or(|r| pg_range_ok(r, major) == Ok(true));
            let pick_ok = pick.is_none_or(|m| m == major);
            if range_ok && pick_ok {
                return Ok(InstallDb::Fits(name.to_string(), major));
            }
            let want = pick.or_else(|| range.and_then(|r| best_major([r]))).unwrap_or(major);
            let fix = if want > major {
                format!("move it first: k2 db upgrade {name} --to {want} (add --live to keep it serving), then install again")
            } else {
                format!("K2 moves databases only to a newer major: install with --pg-major {major}{}", range.map(|r| format!(" if \"{r}\" allows it")).unwrap_or_default())
            };
            Err(Unmet {
                code: k2_core::deploy::codes::REQUIRES_UNMET,
                status: 409,
                hint: format!(
                    "the workspace's database {name} is on Postgres {major}, and {app} needs {}. Install never moves a database: {fix}",
                    match (pick, range) {
                        (Some(m), _) => format!("Postgres {m}"),
                        (None, Some(r)) => format!("Postgres {r}"),
                        (None, None) => unreachable!("handled above"),
                    }
                ),
                extra: json!({ "database": { "name": name, "major": major }, "requires": range, "pgMajor": pick }),
            })
        }
        None => {
            let m = match (pick, range) {
                (Some(m), _) => m,
                (None, Some(r)) => best_major([r]).ok_or_else(|| unsupported(r))?,
                (None, None) => unreachable!("handled above"),
            };
            Ok(InstallDb::Create(m))
        }
    }
}

/// PA7, live: the owner workspace's database from K2's records.
pub fn install_plan_live(app: &str, workspace: &str, range: Option<&str>, pick: Option<u32>) -> Result<InstallDb, Unmet> {
    if range.is_none() && pick.is_none() {
        return Ok(InstallDb::Nothing);
    }
    if !crate::sql::sql_supported() && !cfg!(test) {
        return Err(Unmet {
            code: k2_core::deploy::codes::REQUIRES_UNMET,
            status: 409,
            hint: format!("{app} needs a Postgres database; K2 runs Postgres on Linux boxes only"),
            extra: json!({ "requires": range, "pgMajor": pick }),
        });
    }
    let existing = {
        let k2db = k2_core::db::shared();
        let conn = k2db.lock();
        default_db_in(&conn, workspace).map(|(n, _, m)| (n, m))
    };
    install_plan(app, range, pick, existing.as_ref().map(|(n, m)| (n.as_str(), *m)))
}

// ── walled Apps (PA15) ────────────────────────────────────────────────

/// PA15, the contract for when the walled App's database door lands (S21;
/// `db_port` is `None` until then, `guarded.rs`): a walled App always dials
/// `127.0.0.1:5432` inside its wall, and the door's host port is its
/// database's cluster port (`cluster::for_db`). A database switch changes
/// the door (the App is re-hosted with the new `db_port`), never the
/// App's DSN. Answers `(in-wall DSN, door host port)`; `None` for a URL
/// that isn't Postgres or a port outside K2's band.
pub fn walled_door(dsn: &str, cluster_port: u16) -> Option<(String, u16)> {
    if !k2_root_proto::engine::db_port_ok(cluster_port) {
        return None;
    }
    let (scheme, rest) = dsn.split_once("://").filter(|(s, _)| *s == "postgres" || *s == "postgresql")?;
    let (authority, tail) = rest.split_once('/')?;
    let (userinfo, _hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let at = userinfo.map(|u| format!("{u}@")).unwrap_or_default();
    Some((format!("{scheme}://{at}127.0.0.1:{}/{tail}", k2_root_proto::engine::IN_WALL_DB_PORT), cluster_port))
}

// ── status (PA17) ─────────────────────────────────────────────────────

/// `database: {name, major, source, sharedWith[], requires, available[]}`
/// for `k2 app list|inspect` and the Settings chip; `null` with no
/// database, `{unknown: why}` when K2 can't tell.
pub fn database_json(app: &AppRow) -> J {
    let range = range_of(app);
    match db_of(app) {
        DbOf::None => J::Null,
        DbOf::Unknown(why) => json!({ "unknown": why, "requires": range }),
        DbOf::Known(r) => {
            let shared: Vec<String> = apps_using(&r.name).into_iter().map(|u| u.app).filter(|a| *a != app.app_id).collect();
            let available: Vec<u32> = SUPPORTED
                .iter()
                .copied()
                .filter(|m| *m > r.major && range.as_deref().is_none_or(|rg| pg_range_ok(rg, *m) == Ok(true)))
                .collect();
            json!({
                "name": r.name,
                "major": r.major,
                "source": r.source,
                "sharedWith": shared,
                "requires": range,
                "available": available,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(n: &str) -> Option<(String, u32)> {
        match n {
            "ws_jobs" => Some(("w1".into(), 16)),
            "ws_other" => Some(("w2".into(), 17)),
            _ => None,
        }
    }

    #[test]
    fn dsn_names() {
        assert_eq!(dbname_from_dsn("postgres://a:b@127.0.0.1:5433/ws_jobs?sslmode=disable").as_deref(), Some("ws_jobs"));
        assert_eq!(dbname_from_dsn("postgresql://a@localhost/ws_jobs").as_deref(), Some("ws_jobs"));
        assert_eq!(dbname_from_dsn("postgres://a@localhost/ws_jobs#x").as_deref(), Some("ws_jobs"));
        assert_eq!(dbname_from_dsn("mysql://a@localhost/ws_jobs"), None);
        assert_eq!(dbname_from_dsn("postgres://a@localhost"), None);
        assert_eq!(dbname_from_dsn("postgres://a@localhost/Bad;Name"), None);
    }

    /// PA1: the reference wins, else the workspace default, else nothing;
    /// a reference K2 can't map is unknown, never the default.
    #[test]
    fn resolution_ref_default_unknown() {
        let default = Some(("ws_jobs".to_string(), "w1".to_string(), 16));
        let r = resolve(&UrlLookup::Value("postgres://x:y@127.0.0.1:5433/ws_other".into()), &known, default.clone());
        assert_eq!(r, DbOf::Known(DbRef { name: "ws_other".into(), project_id: "w2".into(), major: 17, source: Source::Ref }));
        let r = resolve(&UrlLookup::Absent, &known, default.clone());
        assert_eq!(r, DbOf::Known(DbRef { name: "ws_jobs".into(), project_id: "w1".into(), major: 16, source: Source::Default }));
        assert_eq!(resolve(&UrlLookup::Absent, &known, None), DbOf::None);
        match resolve(&UrlLookup::Value("postgres://x@db.example.com/theirs".into()), &known, default.clone()) {
            DbOf::Unknown(why) => assert!(why.contains("theirs"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(resolve(&UrlLookup::Unreadable, &known, default.clone()), DbOf::Unknown(_)));
        assert!(matches!(resolve(&UrlLookup::Value("not a url".into()), &known, default), DbOf::Unknown(_)));
    }

    #[test]
    fn best_major_meets_every_range() {
        assert_eq!(best_major([">=16"]), Some(17));
        assert_eq!(best_major(["16"]), Some(16));
        assert_eq!(best_major([">=16", "<17"]), Some(16));
        assert_eq!(best_major(["16", "17"]), None);
        assert_eq!(best_major(Vec::<&str>::new()), Some(17));
        assert_eq!(best_major(["18"]), None, "18 stays off (R4)");
    }

    fn known_db(major: u32) -> DbOf {
        DbOf::Known(DbRef { name: "ws_jobs".into(), project_id: "w1".into(), major, source: Source::Default })
    }

    /// PA5: met, unmet with the move, unsupported, no database, unknown.
    #[test]
    fn the_range_against_the_database() {
        assert_eq!(check("jobs", ">=16", &known_db(16)), Ok(()));
        let u = check("jobs", ">=17", &known_db(16)).unwrap_err();
        assert_eq!((u.code, u.status), ("requires_unmet", 409));
        assert!(u.hint.contains("ws_jobs") && u.hint.contains("Postgres 16") && u.hint.contains("k2 db upgrade ws_jobs --to 17"), "{}", u.hint);
        assert_eq!(u.extra["database"]["major"], 16);
        // Too new: no move down.
        let u = check("jobs", "16", &known_db(17)).unwrap_err();
        assert!(u.hint.contains("only to a newer major"), "{}", u.hint);
        // R4: only 18 → major_unsupported, whatever the database.
        let u = check("jobs", "18", &known_db(16)).unwrap_err();
        assert_eq!(u.code, MAJOR_UNSUPPORTED);
        let u = check("jobs", ">=17", &DbOf::None).unwrap_err();
        assert!(u.hint.contains("k2 db create --major 17"), "{}", u.hint);
        assert_eq!(check("jobs", ">=17", &DbOf::Unknown("external".into())), Ok(()));
        assert_eq!(check("jobs", "x", &known_db(16)).unwrap_err().code, "requires_unmet");
    }

    /// PA15: a walled App keeps dialling 127.0.0.1:5432 in its wall across
    /// a database switch (16 on 5432 → 17 on 5433); only the door's host
    /// port follows the cluster. Pinned now, with `db_port` still stubbed.
    #[test]
    fn a_walled_app_keeps_the_in_wall_address_across_a_switch() {
        let on16 = "postgres://ws_jobs_agent:pw@127.0.0.1:5432/ws_jobs?sslmode=disable";
        let on17 = "postgres://ws_jobs_agent:pw@127.0.0.1:5433/ws_jobs?sslmode=disable";
        let (a, door_a) = walled_door(on16, 5432).unwrap();
        let (b, door_b) = walled_door(on17, 5433).unwrap();
        assert_eq!(a, "postgres://ws_jobs_agent:pw@127.0.0.1:5432/ws_jobs?sslmode=disable");
        assert_eq!(a, b, "the App's DSN never changes on a switch");
        assert_eq!((door_a, door_b), (5432, 5433), "the door follows the cluster");
        assert_eq!(walled_door("postgres://u:p@db.example.com:6543/x", 5433).unwrap().0, "postgres://u:p@127.0.0.1:5432/x");
        assert!(walled_door(on17, 6000).is_none(), "outside the band");
        assert!(walled_door("mysql://u@h/x", 5433).is_none());
    }

    /// PA7: the install table.
    #[test]
    fn install_table() {
        // Nothing asked: nothing done, whatever exists.
        assert_eq!(install_plan("jobs", None, None, None), Ok(InstallDb::Nothing));
        assert_eq!(install_plan("jobs", None, None, Some(("ws_a", 16))), Ok(InstallDb::Nothing));
        // No database: created on the pick, else the highest the range admits.
        assert_eq!(install_plan("jobs", Some(">=16"), None, None), Ok(InstallDb::Create(17)));
        assert_eq!(install_plan("jobs", Some("16"), None, None), Ok(InstallDb::Create(16)));
        assert_eq!(install_plan("jobs", None, Some(16), None), Ok(InstallDb::Create(16)));
        assert_eq!(install_plan("jobs", Some(">=16"), Some(16), None), Ok(InstallDb::Create(16)));
        // Exists and fits: nothing to create.
        assert_eq!(install_plan("jobs", Some(">=16"), None, Some(("ws_a", 16))), Ok(InstallDb::Fits("ws_a".into(), 16)));
        assert_eq!(install_plan("jobs", None, Some(17), Some(("ws_a", 17))), Ok(InstallDb::Fits("ws_a".into(), 17)));
        // Exists on another major: refused with the move; never moved.
        let u = install_plan("jobs", Some(">=17"), None, Some(("ws_a", 16))).unwrap_err();
        assert_eq!((u.code, u.status), ("requires_unmet", 409));
        assert!(u.hint.contains("k2 db upgrade ws_a --to 17") && u.hint.contains("never moves"), "{}", u.hint);
        let u = install_plan("jobs", None, Some(16), Some(("ws_a", 17))).unwrap_err();
        assert!(u.hint.contains("only to a newer major"), "{}", u.hint);
        // The pick outside the range, an unsupported pick, an 18-only range.
        assert_eq!(install_plan("jobs", Some("17"), Some(16), None).unwrap_err().code, "requires_unmet");
        let u = install_plan("jobs", None, Some(18), None).unwrap_err();
        assert_eq!((u.code, u.status), (MAJOR_UNSUPPORTED, 400));
        assert_eq!(install_plan("jobs", Some("18"), None, None).unwrap_err().code, MAJOR_UNSUPPORTED);
    }
}
