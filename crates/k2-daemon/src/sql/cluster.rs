//! One resolver: which Postgres cluster serves a database
//! (prd-pg17-and-db-upgrade-v1 rev 2 §4, V1–V4, V31–V36, V50).
//!
//! Several majors run side by side, one Debian cluster per major
//! (`<major>/main`), each on its own port in K2's band 5432–5449. K2 records
//! them in `sql_clusters` and each database's major in
//! `sql_databases.cluster_major`. Every psql, pg_dump, pg_restore,
//! `exec_as_role`, DSN, grant/revoke, role rotation, seal and doctor call
//! finds its cluster here; nothing else knows a port or a major.
//!
//! - [`for_db`]: the cluster a database lives on (its record; NULL or no
//!   record = the 5432 cluster, today's boxes).
//! - [`for_major`] / [`primary`]: by major, and the box's first cluster
//!   (5432; synthesized from `sql_server.installed_major` before 0201's
//!   backfill or `enable` recorded it).
//! - [`Cluster::helper_psql`]: the helper argv. The 5432 cluster keeps
//!   today's `psql -d <db>` (pg_wrapper's default cluster is the one on
//!   5432), so helpers below protocol 9 keep working; any other cluster
//!   adds `--cluster <major>`, which needs `pgmajor` (protocol 9).
//! - [`Cluster::client_bin`] + `port`: client tools run that major's own
//!   binaries with `-p <port>` (V2, V39: pg_dump must be ≥ the server).
//!
//! The helper's answers for `pg clusters` / `pg ensure-cluster <M>` (S2)
//! are parsed by [`parse_helper_clusters`] / [`parse_helper_cluster`].

use k2_root_proto::pg_major;
use serde::Serialize;

use super::sysops::SystemOps;

/// One cluster K2 knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    /// 0 = unknown (a box whose major was never read).
    pub major: u32,
    pub name: String,
    pub port: u16,
    pub unit: String,
    pub data_dir: String,
    /// `k2` | `adopted` | `foreign`.
    pub role: String,
    /// `baked` | `created` | `adopted`.
    pub origin: String,
    /// `online` | `down` | `unknown`.
    pub state: String,
    /// `auto` | `manual` | `disabled`.
    pub start_conf: String,
    /// Not a row in `sql_clusters` (the legacy 5432 cluster before it was
    /// recorded).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub synthesized: bool,
}

/// The wrapper unit Debian ships (all clusters). Only for a box whose
/// major K2 never read.
const WRAPPER_UNIT: &str = "postgresql";

impl Cluster {
    /// The legacy 5432 cluster of a major (or of an unknown one).
    pub fn legacy_primary(major: u32) -> Self {
        Self {
            major,
            name: pg_major::CLUSTER_NAME.to_string(),
            port: pg_major::PRIMARY_PORT,
            unit: if major > 0 { pg_major::unit(major) } else { WRAPPER_UNIT.to_string() },
            data_dir: if major > 0 { pg_major::data_dir(major) } else { String::new() },
            role: "k2".into(),
            origin: "baked".into(),
            state: "unknown".into(),
            start_conf: "auto".into(),
            synthesized: true,
        }
    }

    /// A K2-layout cluster for `major` on `port` (records, tests).
    pub fn layout(major: u32, port: u16, origin: &str) -> Self {
        Self {
            major,
            name: pg_major::CLUSTER_NAME.to_string(),
            port,
            unit: pg_major::unit(major),
            data_dir: pg_major::data_dir(major),
            role: "k2".into(),
            origin: origin.into(),
            state: "unknown".into(),
            start_conf: "auto".into(),
            synthesized: false,
        }
    }

    /// The cluster on 5432: what pg_wrapper picks without `--cluster`.
    pub fn is_primary(&self) -> bool {
        self.port == pg_major::PRIMARY_PORT
    }

    /// `/usr/lib/postgresql/<M>/bin` (None for an unknown major).
    pub fn bin_dir(&self) -> Option<String> {
        (self.major > 0).then(|| pg_major::bin_dir(self.major))
    }

    /// The client tool for this cluster: that major's own binary when it is
    /// installed (pg_dump must be ≥ the server, V39), else Debian's
    /// wrapper in /usr/bin (old boxes, fakes).
    pub fn client_bin(&self, ops: &dyn SystemOps, tool: &str) -> String {
        if let Some(dir) = self.bin_dir() {
            let p = format!("{dir}/{tool}");
            if ops.path_exists(&p) {
                return p;
            }
        }
        format!("/usr/bin/{tool}")
    }

    /// `-p <port>` as a string, for client argv.
    pub fn port_arg(&self) -> String {
        self.port.to_string()
    }

    /// The helper `psql` argv for `db` on this cluster, then `flags`
    /// (`-v ON_ERROR_STOP=1`, `-tA`, …). The 5432 cluster keeps today's
    /// argv; any other adds `--cluster <major>` (protocol 9, `pgmajor`).
    pub fn helper_psql_argv(&self, db: &str, flags: &[&str]) -> Vec<String> {
        let mut v = vec!["psql".to_string()];
        if !self.is_primary() {
            v.push("--cluster".into());
            v.push(self.major.to_string());
        }
        v.push("-d".into());
        v.push(db.to_string());
        v.extend(flags.iter().map(|s| s.to_string()));
        v
    }

    /// Run SQL as postgres on this cluster through the helper.
    pub fn helper_psql(
        &self,
        ops: &dyn SystemOps,
        db: &str,
        flags: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<String, String> {
        if !self.is_primary() && self.major == 0 {
            return Err(format!(
                "cluster on port {} has no recorded major — run `k2 db clusters` (the doctor re-reads them)",
                self.port
            ));
        }
        let argv = self.helper_psql_argv(db, flags);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        ops.run_helper(&refs, stdin)
    }

    /// `k2` and `adopted` clusters hold K2 databases; `foreign` never does.
    pub fn holds_k2_databases(&self) -> bool {
        self.role != "foreign"
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const COLS: &str = "major, name, port, unit, data_dir, role, origin, state, start_conf";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Cluster> {
    let major: i64 = r.get(0)?;
    let port: i64 = r.get(2)?;
    Ok(Cluster {
        major: major.max(0) as u32,
        name: r.get(1)?,
        port: u16::try_from(port).unwrap_or(0),
        unit: r.get(3)?,
        data_dir: r.get(4)?,
        role: r.get(5)?,
        origin: r.get(6)?,
        state: r.get(7)?,
        start_conf: r.get(8)?,
        synthesized: false,
    })
}

/// `sql_server.installed_major` (the 5432 cluster's major, V6), if read.
pub fn installed_major_in(conn: &rusqlite::Connection) -> Option<u32> {
    conn.query_row("SELECT installed_major FROM sql_server WHERE id = 1", [], |r| r.get::<_, Option<i64>>(0))
        .ok()
        .flatten()
        .filter(|m| (10..=99).contains(m))
        .map(|m| m as u32)
}

/// Every recorded cluster, by port.
pub fn list_in(conn: &rusqlite::Connection) -> Vec<Cluster> {
    let Ok(mut st) = conn.prepare(&format!("SELECT {COLS} FROM sql_clusters ORDER BY port")) else {
        return vec![];
    };
    st.query_map([], from_row).map(|it| it.flatten().collect()).unwrap_or_default()
}

pub fn list() -> Vec<Cluster> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    list_in(&conn)
}

/// The box's first cluster (5432): its record, else the legacy one built
/// from `installed_major`.
pub fn primary_in(conn: &rusqlite::Connection) -> Cluster {
    conn.query_row(
        &format!("SELECT {COLS} FROM sql_clusters WHERE port = ?1"),
        rusqlite::params![pg_major::PRIMARY_PORT as i64],
        from_row,
    )
    .unwrap_or_else(|_| Cluster::legacy_primary(installed_major_in(conn).unwrap_or(0)))
}

pub fn primary() -> Cluster {
    let db = k2_core::db::shared();
    let conn = db.lock();
    primary_in(&conn)
}

/// The cluster recorded for `major` (`<major>/main`). The legacy primary
/// counts when its major matches.
pub fn for_major_in(conn: &rusqlite::Connection, major: u32) -> Option<Cluster> {
    if let Ok(c) = conn.query_row(
        &format!("SELECT {COLS} FROM sql_clusters WHERE major = ?1 AND name = ?2"),
        rusqlite::params![major as i64, pg_major::CLUSTER_NAME],
        from_row,
    ) {
        return Some(c);
    }
    let p = primary_in(conn);
    (p.synthesized && p.major == major && major > 0).then_some(p)
}

pub fn for_major(major: u32) -> Option<Cluster> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for_major_in(&conn, major)
}

/// The recorded major of a database (by Postgres name; dropped rows don't
/// count). `Ok(None)` = no row or NULL: the 5432 cluster.
pub fn db_major_in(conn: &rusqlite::Connection, db_name: &str) -> Option<u32> {
    conn.query_row(
        "SELECT cluster_major FROM sql_databases WHERE name = ?1 AND status != 'dropped' \
         ORDER BY created_at DESC LIMIT 1",
        rusqlite::params![db_name],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .filter(|m| *m > 0)
    .map(|m| m as u32)
}

/// The cluster a database lives on. `postgres` (and any name K2 has no
/// row for) is the 5432 cluster. A recorded major with no cluster record
/// is an error, never a silent fallback to 5432.
pub fn for_db_in(conn: &rusqlite::Connection, db_name: &str) -> Result<Cluster, String> {
    match db_major_in(conn, db_name) {
        None => Ok(primary_in(conn)),
        Some(m) => for_major_in(conn, m).ok_or_else(|| {
            format!(
                "database {db_name} lives on Postgres {m}, but K2 has no record of a {m} cluster — see `k2 db clusters`"
            )
        }),
    }
}

pub fn for_db(db_name: &str) -> Result<Cluster, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for_db_in(&conn, db_name)
}

/// Upsert a cluster record (keyed on major + name). A record never
/// changes `created_at`; `origin` and `role` stay as first recorded unless
/// `force_role` (the owner re-recording an adopted box).
pub fn record_in(conn: &rusqlite::Connection, c: &Cluster, force_role: bool) -> Result<(), String> {
    if !(10..=99).contains(&c.major) {
        return Err(format!("refusing to record a cluster without a major (port {})", c.port));
    }
    let now = now_secs();
    let sql = if force_role {
        "INSERT INTO sql_clusters (major, name, port, data_dir, unit, start_conf, state, role, origin, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10) \
         ON CONFLICT(major, name) DO UPDATE SET port = ?3, data_dir = ?4, unit = ?5, start_conf = ?6, state = ?7, \
           role = ?8, origin = ?9, updated_at = ?10"
    } else {
        "INSERT INTO sql_clusters (major, name, port, data_dir, unit, start_conf, state, role, origin, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10) \
         ON CONFLICT(major, name) DO UPDATE SET port = ?3, data_dir = ?4, unit = ?5, start_conf = ?6, state = ?7, \
           updated_at = ?10"
    };
    conn.execute(
        sql,
        rusqlite::params![
            c.major as i64,
            c.name,
            c.port as i64,
            c.data_dir,
            c.unit,
            c.start_conf,
            c.state,
            c.role,
            c.origin,
            now
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("record cluster {}/{}: {e}", c.major, c.name))
}

/// `enable` read the 5432 cluster's major: record it (once; later reads
/// refresh its state). Before 0.46.3 nothing recorded it.
pub fn record_primary_major(conn: &rusqlite::Connection, major: u32) -> Result<Cluster, String> {
    let existing = primary_in(conn);
    if !existing.synthesized && existing.major != major {
        // Live ≠ recorded (V6): never rewrite silently; the doctor fails.
        return Err(format!(
            "the 5432 cluster answers Postgres {major}, but K2 recorded {} — run `k2 db doctor`",
            existing.major
        ));
    }
    let mut c = if existing.synthesized { Cluster::layout(major, pg_major::PRIMARY_PORT, "baked") } else { existing };
    c.state = "online".into();
    record_in(conn, &c, false)?;
    Ok(c)
}

/// The major `k2 db create` uses without `--major`: the owner's pin, else
/// K2's policy default ([`pg_major::NEW_INSTALL`]). `.1` = the owner
/// pinned it.
pub fn default_major_in(conn: &rusqlite::Connection) -> (u32, bool) {
    let pinned: Option<i64> = conn
        .query_row("SELECT default_major FROM sql_server WHERE id = 1", [], |r| r.get(0))
        .ok()
        .flatten();
    match pinned.filter(|m| (10..=99).contains(m)) {
        Some(m) => (m as u32, true),
        None => (pg_major::NEW_INSTALL, false),
    }
}

pub fn default_major() -> (u32, bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    default_major_in(&conn)
}

/// The owner pins the default major (`k2 db default-major <M>`). Only a
/// major [`pg_major::check_target`] accepts today.
pub fn set_default_major_in(conn: &rusqlite::Connection, major: u32) -> Result<(), pg_major::Refusal> {
    pg_major::check_target(major, false, pg_major::today_days())?;
    let _ = conn.execute(
        "UPDATE sql_server SET default_major = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![major as i64, now_secs()],
    );
    Ok(())
}

/// Record a database's cluster (create, and S5's switch step).
pub fn set_db_major_in(conn: &rusqlite::Connection, database_id: &str, major: u32) -> Result<(), String> {
    conn.execute(
        "UPDATE sql_databases SET cluster_major = ?1 WHERE id = ?2",
        rusqlite::params![major as i64, database_id],
    )
    .map(|_| ())
    .map_err(|e| format!("record database major: {e}"))
}

// ── the helper's answers (S2: `pg clusters`, `pg ensure-cluster <M>`) ──

/// One cluster in the helper's JSON:
/// `{"major":17,"name":"main","port":5433,"dataDir":"/var/lib/postgresql/17/main",
///   "running":true,"startConf":"auto"}` (`unit` optional; derived).
pub fn parse_helper_cluster(v: &serde_json::Value) -> Result<Cluster, String> {
    let major = v.get("major").and_then(serde_json::Value::as_u64).filter(|m| (10..=99).contains(m));
    let Some(major) = major else {
        return Err(format!("helper cluster without a valid major: {v}"));
    };
    let major = major as u32;
    let name = v.get("name").and_then(serde_json::Value::as_str).unwrap_or(pg_major::CLUSTER_NAME).to_string();
    let port = v
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| format!("helper cluster {major}/{name} without a port"))?;
    let data_dir = v.get("dataDir").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    let start_conf = match v.get("startConf").and_then(serde_json::Value::as_str).unwrap_or("auto") {
        s @ ("auto" | "manual" | "disabled") => s.to_string(),
        other => return Err(format!("helper cluster {major}/{name}: unknown start.conf {other:?}")),
    };
    let state = match v.get("running").and_then(serde_json::Value::as_bool) {
        Some(true) => "online",
        Some(false) => "down",
        None => "unknown",
    };
    let k2_layout = name == pg_major::CLUSTER_NAME && data_dir == pg_major::data_dir(major);
    Ok(Cluster {
        major,
        unit: v
            .get("unit")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("postgresql@{major}-{name}.service")),
        name,
        port,
        data_dir,
        // Anything outside K2's layout (another name, a data dir outside
        // /var/lib/postgresql) is never K2's to create into (§10).
        role: if k2_layout { "k2".into() } else { "foreign".into() },
        origin: "adopted".into(),
        state: state.into(),
        start_conf,
        synthesized: false,
    })
}

/// `pg clusters`: a JSON array or `{"clusters":[…]}`.
pub fn parse_helper_clusters(raw: &str) -> Result<Vec<Cluster>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("helper clusters: not JSON ({e})"))?;
    let arr = match &v {
        serde_json::Value::Array(a) => a.clone(),
        _ => v
            .get("clusters")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .ok_or_else(|| "helper clusters: no `clusters` array".to_string())?,
    };
    arr.iter().map(parse_helper_cluster).collect()
}

/// Fold the helper's view into the records: known clusters get their
/// port/state/start.conf refreshed; a K2-layout cluster K2 didn't know is
/// recorded `adopted` (the 5432 one on a box with no records stays the
/// `k2` primary, `baked`); anything else is recorded `foreign`. Returns
/// the records after.
pub fn fold_helper_view_in(conn: &rusqlite::Connection, seen: &[Cluster]) -> Result<Vec<Cluster>, String> {
    let known = list_in(conn);
    let nothing_recorded = known.is_empty();
    for c in seen {
        let mut rec = c.clone();
        if let Some(k) = known.iter().find(|k| k.major == c.major && k.name == c.name) {
            rec.role = k.role.clone();
            rec.origin = k.origin.clone();
        } else if rec.role == "k2" {
            if nothing_recorded && rec.is_primary() {
                rec.origin = "baked".into();
            } else {
                rec.role = "adopted".into();
                rec.origin = "adopted".into();
            }
        }
        record_in(conn, &rec, false)?;
    }
    for k in known {
        if !seen.iter().any(|c| c.major == k.major && c.name == k.name) {
            let _ = conn.execute(
                "UPDATE sql_clusters SET state = 'unknown', updated_at = ?3 WHERE major = ?1 AND name = ?2",
                rusqlite::params![k.major as i64, k.name, now_secs()],
            );
        }
    }
    Ok(list_in(conn))
}

/// Read the box's clusters through the helper (`pg clusters`, protocol 9)
/// and fold them into the records.
pub fn reconcile(ops: &dyn SystemOps) -> Result<Vec<Cluster>, String> {
    let raw = ops.run_helper(&["clusters"], None)?;
    let seen = parse_helper_clusters(&raw)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    fold_helper_view_in(&conn, &seen)
}

/// Make sure `major` has a running K2 cluster: the record when it is
/// there, else `pg ensure-cluster <M>` (protocol 9: packages if missing,
/// `pg_createcluster` on the next free band port, the RAM fence,
/// `wal_level = logical`, `password_encryption = scram-sha-256`), then
/// record it `created`. Refuses a major the policy refuses, an answer off
/// K2's layout, and a port outside the band or already recorded.
pub fn ensure_cluster(ops: &dyn SystemOps, major: u32, preview: bool) -> Result<Cluster, EnsureError> {
    pg_major::check_target(major, preview, pg_major::today_days()).map_err(EnsureError::Policy)?;
    if let Some(c) = for_major(major) {
        if c.role == "foreign" {
            return Err(EnsureError::Refused(format!(
                "the {major} cluster on port {} was set up outside K2's layout; K2 never creates databases in it",
                c.port
            )));
        }
        return Ok(c);
    }
    let raw = ops
        .run_helper(&["ensure-cluster", &major.to_string()], None)
        .map_err(EnsureError::Helper)?;
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| EnsureError::Helper(format!("ensure-cluster: not JSON ({e})")))?;
    let mut c = parse_helper_cluster(&v).map_err(EnsureError::Helper)?;
    if c.major != major || c.role != "k2" {
        return Err(EnsureError::Helper(format!(
            "ensure-cluster {major} answered {}/{} in {} — not K2's layout",
            c.major, c.name, c.data_dir
        )));
    }
    if !pg_major::in_band(c.port) {
        return Err(EnsureError::Helper(format!(
            "ensure-cluster {major} answered port {}, outside K2's band {}-{}",
            c.port,
            pg_major::BAND.0,
            pg_major::BAND.1
        )));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(other) = list_in(&conn).into_iter().find(|k| k.port == c.port && k.major != major) {
        return Err(EnsureError::Helper(format!(
            "ensure-cluster {major} answered port {}, which K2 recorded for the {} cluster",
            c.port, other.major
        )));
    }
    c.origin = if c.is_primary() && list_in(&conn).is_empty() { "baked".into() } else { "created".into() };
    record_in(&conn, &c, false).map_err(EnsureError::Helper)?;
    Ok(c)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureError {
    /// The version policy refuses the major (exit 3, its code).
    Policy(pg_major::Refusal),
    /// K2 refuses (adopted/foreign cluster).
    Refused(String),
    /// The helper failed or is too old (`helper_needed: …`).
    Helper(String),
}

impl EnsureError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Policy(r) => r.code(),
            Self::Refused(_) => "cluster_refused",
            Self::Helper(h) if h.starts_with("helper_needed") => "helper_needed",
            Self::Helper(_) => "cluster_unavailable",
        }
    }
    pub fn hint(&self) -> String {
        match self {
            Self::Policy(r) => r.hint(),
            Self::Refused(h) | Self::Helper(h) => h.clone(),
        }
    }
}

/// Clusters that hold K2 databases (role `k2` or `adopted`), plus the
/// legacy primary when nothing is recorded yet.
pub fn k2_clusters_in(conn: &rusqlite::Connection) -> Vec<Cluster> {
    let mut v: Vec<Cluster> = list_in(conn).into_iter().filter(Cluster::holds_k2_databases).collect();
    if v.is_empty() {
        v.push(primary_in(conn));
    }
    v
}

pub fn k2_clusters() -> Vec<Cluster> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_clusters_in(&conn)
}

/// Per-database majors for status (`databases[].major`, the upgrades
/// available today).
pub fn database_majors_in(conn: &rusqlite::Connection) -> Vec<(String, String, u32)> {
    let primary = primary_in(conn).major;
    let Ok(mut st) = conn.prepare(
        "SELECT id, name, cluster_major FROM sql_databases WHERE status != 'dropped' ORDER BY created_at",
    ) else {
        return vec![];
    };
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)))
        .map(|it| {
            it.flatten()
                .map(|(id, name, m)| (id, name, m.filter(|m| *m > 0).map(|m| m as u32).unwrap_or(primary)))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
