//! Live dual-run of one database (prd-app-pg-choice-dualrun-v1 §5, §9;
//! protocol 10, `namespaces.pgdual`): logical replication from a K2
//! database on one major into a schema-only copy on a newer one, then a
//! short cut-over. Every statement the helper runs for it is rendered here
//! from validated tokens only, so both sides (and the tests) see the same
//! canned SQL:
//!
//! - names are fixed shapes around a 12-hex id ([`DualNames`]); databases
//!   are [`crate::verbs::DbName`]s; the replication password is
//!   [`DualSecret`] (URL-safe characters only, never quoted, never in argv);
//! - the replication role is `LOGIN REPLICATION BYPASSRLS NOINHERIT`, one
//!   database's `CONNECT` + `USAGE` + `SELECT` only, statement timeouts off
//!   for it alone (PA23); physical replication by any `k2_repl_*` role is
//!   rejected in `pg_hba.conf` ([`crate::apps::REPL_GUARD_LINE`], PA24);
//! - the publication, the DDL freeze (schema `_k2_dual`, an event
//!   trigger) and the subscription are created by postgres; the
//!   subscription applies as each table's owner (`run_as_owner = false`)
//!   and disables itself on the first error (PA26, PA27);
//! - detach and teardown drop only objects with these exact names, in an
//!   order that works whatever exists (PA31 step 4, PA34).

use serde::Deserialize;

/// `pg dual <op>` operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DualOp {
    /// On the source: hba guard, replication role, grants, publication,
    /// DDL freeze. Stdin: [`DualSecret`].
    SetupSource,
    /// On the target (quiesced, FORCE RLS off): the subscription. Stdin:
    /// [`DualSecret`].
    Subscribe,
    /// The cut-over's detach: subscription (slot name cleared first), then
    /// the slot, freeze, grants and publication on the source. The role
    /// stays until teardown.
    Detach,
    /// Everything this id made, on both sides, the role included. Idempotent.
    Teardown,
    /// Read-only numbers for the lag monitor (open class).
    Status,
}

impl DualOp {
    pub fn parse(t: &str) -> Option<Self> {
        Some(match t {
            "setup-source" => Self::SetupSource,
            "subscribe" => Self::Subscribe,
            "detach" => Self::Detach,
            "teardown" => Self::Teardown,
            "status" => Self::Status,
            _ => return None,
        })
    }
    pub fn token(self) -> &'static str {
        match self {
            Self::SetupSource => "setup-source",
            Self::Subscribe => "subscribe",
            Self::Detach => "detach",
            Self::Teardown => "teardown",
            Self::Status => "status",
        }
    }
    pub const ALL: [DualOp; 5] = [Self::SetupSource, Self::Subscribe, Self::Detach, Self::Teardown, Self::Status];
}

/// A dual-run id: exactly 12 lowercase hex digits (the first 12 of the
/// database's K2 id, PA23), so every derived name stays ≤ 63 bytes.
pub fn dual_id_ok(t: &str) -> bool {
    t.len() == 12 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The objects one run makes, all derived from its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DualNames {
    pub role: String,
    pub publication: String,
    pub subscription: String,
    pub slot: String,
}

/// The DDL freeze's schema and event trigger (one per database: a run is
/// one per database, PA18).
pub const FREEZE_SCHEMA: &str = "_k2_dual";
pub const FREEZE_TRIGGER: &str = "k2_dual_freeze";
/// The SQLSTATE-like message the freeze raises (apps see it).
pub const FREEZE_ERROR: &str = "k2_dual_ddl_frozen";
/// Name prefixes K2 owns (orphan sweeps match on them, PA35).
pub const ROLE_PREFIX: &str = "k2_repl_";
pub const OBJECT_PREFIX: &str = "k2_dual_";

impl DualNames {
    /// `None` for an id that isn't [`dual_id_ok`].
    pub fn of(id: &str) -> Option<Self> {
        dual_id_ok(id).then(|| Self {
            role: format!("{ROLE_PREFIX}{id}"),
            publication: format!("{OBJECT_PREFIX}{id}"),
            subscription: format!("{OBJECT_PREFIX}{id}"),
            slot: format!("{OBJECT_PREFIX}{id}"),
        })
    }
}

/// `setup-source` / `subscribe` stdin: the replication role's password,
/// made by the daemon (32 random bytes, base64url).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DualSecret {
    pub password: String,
}

/// Stdin cap for [`DualSecret`].
pub const CAP_DUAL_SECRET: usize = 1024;

impl DualSecret {
    /// 32–128 URL-safe characters: it is written into SQL and a libpq
    /// connection string unquoted-safe, so nothing else is allowed.
    pub fn validate(&self) -> Result<(), &'static str> {
        let p = &self.password;
        if p.len() < 32 || p.len() > 128 || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err("pg dual: the replication password must be 32-128 characters of [A-Za-z0-9_-]");
        }
        Ok(())
    }
}

/// `pg force-rls` stdin: the tables (oids) that had FORCE ROW LEVEL
/// SECURITY before `move-restore --keep-force-off` lifted it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForceOids {
    pub oids: Vec<u32>,
}

pub const CAP_FORCE_OIDS: usize = 64 * 1024;
const MAX_FORCE_OIDS: usize = 5000;

impl ForceOids {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.oids.len() > MAX_FORCE_OIDS {
            return Err("force-rls: at most 5000 tables");
        }
        if self.oids.iter().any(|o| *o < 16384) {
            return Err("force-rls: a system oid (below 16384) is never a K2 table");
        }
        Ok(())
    }
}

// ── the WAL cap and logical decoding (PA21, PA22) ───────────────────────

/// The drop-in `pg logical-conf` writes (`conf.d`, before K2's own
/// `98-k2-cluster.conf`, which says the same `wal_level` on K2-created
/// clusters).
pub const LOGICAL_CONF_FILE: &str = "97-k2-logical.conf";
/// The WAL a slot may hold, in MB (PA22: at most 8 GB; the daemon passes
/// `min(8 GB, 25% of free disk)`).
pub const WAL_KEEP_MB: (u32, u32) = (64, 8192);

pub fn wal_keep_ok(mb: u32) -> bool {
    (WAL_KEEP_MB.0..=WAL_KEEP_MB.1).contains(&mb)
}

/// The drop-in's bytes. `wal_level` takes effect at the cluster's next
/// restart (the owner's `--prepare-live`); `max_slot_wal_keep_size` at
/// the reload the helper sends.
pub fn logical_conf(wal_keep_mb: u32) -> String {
    format!(
        "# K2 (k2-root-helper pg logical-conf; do not edit): live moves.\n\
         wal_level = logical\n\
         max_slot_wal_keep_size = {wal_keep_mb}MB\n"
    )
}

// ── canned SQL ─────────────────────────────────────────────────────────

/// Keep a password out of the server log and pg_stat_statements (as
/// `copy-roles`, review #1 L2).
pub const QUIET_SQL: &str = "SET log_min_error_statement = panic;\n\
     SET log_statement = 'none';\n\
     SET log_min_duration_statement = -1;\n\
     DO $k2$BEGIN IF pg_catalog.current_setting('pg_stat_statements.track_utility', true) IS NOT NULL THEN \
     PERFORM pg_catalog.set_config('pg_stat_statements.track_utility', 'off', false); END IF; END$k2$;\n";

/// `SELECT count(*)` of K2 dual objects of this id already on the source
/// cluster (role) — `setup-source` refuses a second setup.
pub fn role_count_sql(n: &DualNames) -> String {
    format!("SELECT count(*) FROM pg_catalog.pg_roles WHERE rolname = '{}';\n", n.role)
}

/// In the `postgres` database of the source cluster: the replication role
/// (PA23). The password is validated ([`DualSecret::validate`]).
pub fn setup_role_sql(n: &DualNames, db: &str, s: &DualSecret) -> String {
    format!(
        "{QUIET_SQL}\
         CREATE ROLE \"{r}\" LOGIN REPLICATION BYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOSUPERUSER CONNECTION LIMIT 4 PASSWORD '{pw}';\n\
         ALTER ROLE \"{r}\" SET statement_timeout = 0;\n\
         ALTER ROLE \"{r}\" SET idle_in_transaction_session_timeout = 0;\n\
         GRANT CONNECT ON DATABASE \"{db}\" TO \"{r}\";\n",
        r = n.role,
        pw = s.password,
    )
}

/// In the source database, one transaction: `USAGE` on every user schema,
/// `SELECT` on every table, the publication, then the DDL freeze last
/// (PA23, PA26, PA27). The freeze catches DDL from any role at
/// `ddl_command_start`.
pub fn setup_db_sql(n: &DualNames, db: &str) -> String {
    format!(
        "BEGIN;\n\
         DO $k2$DECLARE s name; BEGIN\n\
         FOR s IN SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname NOT IN ('pg_catalog', 'information_schema') \
         AND nspname NOT LIKE 'pg\\_%' AND nspname <> '{fs}' LOOP\n\
         EXECUTE pg_catalog.format('GRANT USAGE ON SCHEMA %I TO %I', s, '{r}');\n\
         EXECUTE pg_catalog.format('GRANT SELECT ON ALL TABLES IN SCHEMA %I TO %I', s, '{r}');\n\
         END LOOP; END$k2$;\n\
         CREATE PUBLICATION \"{p}\" FOR ALL TABLES;\n\
         CREATE SCHEMA \"{fs}\" AUTHORIZATION postgres;\n\
         REVOKE ALL ON SCHEMA \"{fs}\" FROM PUBLIC;\n\
         CREATE FUNCTION \"{fs}\".freeze() RETURNS event_trigger LANGUAGE plpgsql SET search_path = pg_catalog AS $k2f$\n\
         BEGIN RAISE EXCEPTION '{fe}: a Postgres move is replicating this database; schema changes wait'\n\
         USING HINT = 'k2 db upgrade {db} --abort or --switch first'; END $k2f$;\n\
         REVOKE ALL ON FUNCTION \"{fs}\".freeze() FROM PUBLIC;\n\
         CREATE EVENT TRIGGER {ft} ON ddl_command_start EXECUTE FUNCTION \"{fs}\".freeze();\n\
         COMMIT;\n",
        r = n.role,
        p = n.publication,
        fs = FREEZE_SCHEMA,
        ft = FREEZE_TRIGGER,
        fe = FREEZE_ERROR,
    )
}

/// The publication exists on the source (`subscribe` checks first).
pub fn publication_count_sql(n: &DualNames) -> String {
    format!("SELECT count(*) FROM pg_catalog.pg_publication WHERE pubname = '{}';\n", n.publication)
}

/// The target database's state for `subscribe`: `<connlimit>,<forced
/// tables>,<subscriptions in it>` (it must be `0,0,0`).
pub fn target_ready_sql(db: &str) -> String {
    format!(
        "SELECT (SELECT datconnlimit FROM pg_catalog.pg_database WHERE datname = '{db}')::text || ',' || \
         (SELECT count(*) FROM pg_catalog.pg_class WHERE relforcerowsecurity)::text || ',' || \
         (SELECT count(*) FROM pg_catalog.pg_subscription s JOIN pg_catalog.pg_database d ON d.oid = s.subdbid \
          WHERE d.datname = '{db}')::text;\n"
    )
}

/// In the target database: the subscription (PA26). `CREATE SUBSCRIPTION`
/// with a slot can't run in a transaction block: no BEGIN.
pub fn subscribe_sql(n: &DualNames, db: &str, src_port: u16, s: &DualSecret) -> String {
    format!(
        "{QUIET_SQL}\
         CREATE SUBSCRIPTION \"{sub}\" CONNECTION 'host=127.0.0.1 port={src_port} dbname={db} user={r} password={pw} \
         options=''-c row_security=off''' PUBLICATION \"{p}\" WITH (copy_data = true, create_slot = true, \
         slot_name = '{slot}', run_as_owner = false, disable_on_error = true, streaming = on, binary = false);\n",
        sub = n.subscription,
        r = n.role,
        pw = s.password,
        p = n.publication,
        slot = n.slot,
    )
}

/// Does the subscription exist (in the target database)?
pub fn subscription_count_sql(n: &DualNames) -> String {
    format!("SELECT count(*) FROM pg_catalog.pg_subscription WHERE subname = '{}';\n", n.subscription)
}

/// In the target database: drop the subscription without reaching the
/// (closed) publisher (L24): disable, forget the slot, drop. Each
/// statement autocommits.
pub fn drop_subscription_sql(n: &DualNames) -> String {
    format!(
        "ALTER SUBSCRIPTION \"{s}\" DISABLE;\n\
         ALTER SUBSCRIPTION \"{s}\" SET (slot_name = NONE);\n\
         DROP SUBSCRIPTION \"{s}\";\n",
        s = n.subscription
    )
}

/// In the source database: the slot (its walsender ended first, waited
/// up to 10 s), the freeze (first, so the DDL after it isn't frozen), the
/// grants and the publication. Idempotent: everything `IF EXISTS`.
pub fn detach_source_sql(n: &DualNames, db: &str) -> String {
    format!(
        "DROP EVENT TRIGGER IF EXISTS {ft};\n\
         DROP SCHEMA IF EXISTS \"{fs}\" CASCADE;\n\
         DO $k2$DECLARE i int := 0; BEGIN\n\
         PERFORM pg_catalog.pg_terminate_backend(active_pid) FROM pg_catalog.pg_replication_slots WHERE slot_name = '{slot}' AND active;\n\
         WHILE EXISTS (SELECT 1 FROM pg_catalog.pg_replication_slots WHERE slot_name = '{slot}' AND active) AND i < 100 LOOP\n\
         PERFORM pg_catalog.pg_sleep(0.1); i := i + 1; END LOOP;\n\
         IF EXISTS (SELECT 1 FROM pg_catalog.pg_replication_slots WHERE slot_name = '{slot}') THEN\n\
         PERFORM pg_catalog.pg_drop_replication_slot('{slot}'); END IF; END$k2$;\n\
         DROP PUBLICATION IF EXISTS \"{p}\";\n\
         DO $k2$DECLARE s name; BEGIN\n\
         IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = '{r}') THEN\n\
         FOR s IN SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname NOT IN ('pg_catalog', 'information_schema') \
         AND nspname NOT LIKE 'pg\\_%' LOOP\n\
         EXECUTE pg_catalog.format('REVOKE ALL ON ALL TABLES IN SCHEMA %I FROM %I', s, '{r}');\n\
         EXECUTE pg_catalog.format('REVOKE ALL ON SCHEMA %I FROM %I', s, '{r}');\n\
         END LOOP; END IF; END$k2$;\n\
         SELECT 'detached {db}';\n",
        ft = FREEZE_TRIGGER,
        fs = FREEZE_SCHEMA,
        slot = n.slot,
        p = n.publication,
        r = n.role,
    )
}

/// In the source cluster's `postgres` database: the role (after
/// [`detach_source_sql`] took its grants inside the database).
pub fn drop_role_sql(n: &DualNames, db: &str) -> String {
    format!(
        "DO $k2$BEGIN IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = '{r}') THEN\n\
         EXECUTE 'REVOKE CONNECT ON DATABASE \"{db}\" FROM \"{r}\"';\n\
         EXECUTE 'DROP ROLE \"{r}\"'; END IF; END$k2$;\n",
        r = n.role
    )
}

/// The `ALTER DATABASE … CONNECTION LIMIT 0` write block that keeps the
/// logical walsender (L6): every other session of the database ends.
pub fn quiesce_repl_sql(db: &str) -> String {
    format!(
        "ALTER DATABASE \"{db}\" CONNECTION LIMIT 0;\n\
         SELECT count(pg_catalog.pg_terminate_backend(pid)) FROM pg_catalog.pg_stat_activity \
         WHERE datname = '{db}' AND pid <> pg_catalog.pg_backend_pid() AND backend_type <> 'walsender';\n"
    )
}

/// In the target database: FORCE back on exactly these tables, each one a
/// user table or partitioned table of this database (anything else raises
/// before any change: one transaction).
pub fn force_rls_sql(oids: &[u32]) -> String {
    let list = oids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    format!(
        "BEGIN;\n\
         DO $k2$DECLARE t oid; ok boolean; BEGIN\n\
         FOREACH t IN ARRAY ARRAY[{list}]::oid[] LOOP\n\
         SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.oid = t AND c.relkind IN ('r', 'p') AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
         AND n.nspname NOT LIKE 'pg\\_%') INTO ok;\n\
         IF NOT ok THEN RAISE EXCEPTION 'force-rls: oid % is not a table of this database', t; END IF;\n\
         EXECUTE pg_catalog.format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', t::regclass);\n\
         END LOOP; END$k2$;\n\
         COMMIT;\n\
         SELECT {n};\n",
        n = oids.len()
    )
}

/// As the database owner (the daemon logs in with the owner's own
/// password over loopback, as its login check does; never the superuser):
/// refresh every materialized view, in creation order (a later one may
/// read an earlier one).
pub const REFRESH_SQL: &str = "DO $k2$DECLARE m regclass; BEGIN\n\
     FOR m IN SELECT c.oid::regclass FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
     WHERE c.relkind = 'm' AND n.nspname NOT IN ('pg_catalog', 'information_schema') ORDER BY c.oid LOOP\n\
     EXECUTE pg_catalog.format('REFRESH MATERIALIZED VIEW %s', m);\n\
     END LOOP; END$k2$;\n\
     SELECT count(*) FROM pg_catalog.pg_class WHERE relkind = 'm';\n";

/// On the source cluster (in the database): one JSON object for the lag
/// monitor and the cut-over's drain (PA29, PA31 step 3).
pub fn status_source_sql(n: &DualNames) -> String {
    format!(
        "SELECT pg_catalog.json_build_object(\
         'walLevel', pg_catalog.current_setting('wal_level'), \
         'currentLsn', pg_catalog.pg_current_wal_lsn()::text, \
         'slot', (SELECT pg_catalog.json_build_object('active', active, 'confirmedFlushLsn', confirmed_flush_lsn::text, \
           'walStatus', wal_status, 'safeWalSize', safe_wal_size, \
           'lagBytes', pg_catalog.pg_wal_lsn_diff(pg_catalog.pg_current_wal_lsn(), confirmed_flush_lsn)) \
           FROM pg_catalog.pg_replication_slots WHERE slot_name = '{slot}'), \
         'walsenders', (SELECT count(*) FROM pg_catalog.pg_stat_replication WHERE usename = '{r}'), \
         'publication', EXISTS (SELECT 1 FROM pg_catalog.pg_publication WHERE pubname = '{p}'), \
         'freeze', EXISTS (SELECT 1 FROM pg_catalog.pg_event_trigger WHERE evtname = '{ft}'), \
         'role', EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = '{r}'))::text;\n",
        slot = n.slot,
        r = n.role,
        p = n.publication,
        ft = FREEZE_TRIGGER,
    )
}

/// On the target cluster (in the database): the subscription's state.
pub fn status_target_sql(n: &DualNames) -> String {
    format!(
        "SELECT pg_catalog.json_build_object(\
         'subscription', EXISTS (SELECT 1 FROM pg_catalog.pg_subscription WHERE subname = '{s}'), \
         'enabled', (SELECT subenabled FROM pg_catalog.pg_subscription WHERE subname = '{s}'), \
         'tablesTotal', (SELECT count(*) FROM pg_catalog.pg_subscription_rel r JOIN pg_catalog.pg_subscription s ON s.oid = r.srsubid WHERE s.subname = '{s}'), \
         'tablesReady', (SELECT count(*) FROM pg_catalog.pg_subscription_rel r JOIN pg_catalog.pg_subscription s ON s.oid = r.srsubid WHERE s.subname = '{s}' AND r.srsubstate = 'r'), \
         'receivedLsn', (SELECT max(received_lsn)::text FROM pg_catalog.pg_stat_subscription WHERE subname = '{s}'), \
         'latestEndLsn', (SELECT max(latest_end_lsn)::text FROM pg_catalog.pg_stat_subscription WHERE subname = '{s}'), \
         'lastMsgAt', (SELECT extract(epoch FROM max(last_msg_receipt_time))::bigint FROM pg_catalog.pg_stat_subscription WHERE subname = '{s}'), \
         'applyErrors', (SELECT apply_error_count FROM pg_catalog.pg_stat_subscription_stats WHERE subname = '{s}'), \
         'syncErrors', (SELECT sync_error_count FROM pg_catalog.pg_stat_subscription_stats WHERE subname = '{s}'))::text;\n",
        s = n.subscription,
    )
}

/// Every `k2_dual_*` / `k2_repl_*` object on a cluster, from its
/// `postgres` database (the orphan sweep, PA35): slots, roles and the
/// databases holding a K2 publication or freeze are listed by the daemon
/// per database with [`status_source_sql`].
pub const ORPHANS_SQL: &str = "SELECT pg_catalog.json_build_object(\
     'slots', (SELECT coalesce(pg_catalog.json_agg(pg_catalog.json_build_object('name', slot_name, 'db', database, 'active', active)), '[]') \
       FROM pg_catalog.pg_replication_slots WHERE slot_name LIKE 'k2\\_dual\\_%'), \
     'roles', (SELECT coalesce(pg_catalog.json_agg(rolname), '[]') FROM pg_catalog.pg_roles WHERE rolname LIKE 'k2\\_repl\\_%'))::text;\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn n() -> DualNames {
        DualNames::of("0123456789ab").unwrap()
    }

    fn pw() -> DualSecret {
        DualSecret { password: "A".repeat(43) }
    }

    #[test]
    fn ids_names_and_ops() {
        assert!(dual_id_ok("0123456789ab"));
        for bad in ["", "0123456789a", "0123456789abc", "0123456789AB", "0123456789ag", "0123456789a;"] {
            assert!(!dual_id_ok(bad), "{bad}");
            assert!(DualNames::of(bad).is_none());
        }
        let n = n();
        assert_eq!((n.role.as_str(), n.publication.as_str(), n.slot.as_str()), ("k2_repl_0123456789ab", "k2_dual_0123456789ab", "k2_dual_0123456789ab"));
        assert!(n.role.len() <= 63);
        for op in DualOp::ALL {
            assert_eq!(DualOp::parse(op.token()), Some(op));
        }
        assert_eq!(DualOp::parse("drop"), None);
    }

    #[test]
    fn secrets_and_oids_are_validated() {
        assert!(pw().validate().is_ok());
        for bad in ["short", &"a".repeat(129), &format!("{}'", "a".repeat(40)), &format!("{} x", "a".repeat(40)), &format!("{}\\", "a".repeat(40))] {
            assert!(DualSecret { password: bad.to_string() }.validate().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<DualSecret>(r#"{"password":"x","extra":1}"#).is_err());
        assert!(ForceOids { oids: vec![16384, 99999] }.validate().is_ok());
        assert!(ForceOids { oids: vec![1259] }.validate().is_err(), "pg_class's oid");
        assert!(ForceOids { oids: vec![20000; 5001] }.validate().is_err());
        assert!(wal_keep_ok(64) && wal_keep_ok(8192) && !wal_keep_ok(63) && !wal_keep_ok(8193));
        assert_eq!(logical_conf(512), "# K2 (k2-root-helper pg logical-conf; do not edit): live moves.\nwal_level = logical\nmax_slot_wal_keep_size = 512MB\n");
    }

    /// Snapshots of the canned SQL's security-relevant lines.
    #[test]
    fn canned_sql_shapes() {
        let r = setup_role_sql(&n(), "ws_jobs", &pw());
        assert!(r.starts_with(QUIET_SQL), "the password never reaches the log");
        assert!(r.contains(&format!(
            "CREATE ROLE \"k2_repl_0123456789ab\" LOGIN REPLICATION BYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOSUPERUSER CONNECTION LIMIT 4 PASSWORD '{}';",
            "A".repeat(43)
        )));
        assert!(r.contains("ALTER ROLE \"k2_repl_0123456789ab\" SET statement_timeout = 0;"));
        assert!(r.contains("ALTER ROLE \"k2_repl_0123456789ab\" SET idle_in_transaction_session_timeout = 0;"));
        assert!(r.contains("GRANT CONNECT ON DATABASE \"ws_jobs\" TO \"k2_repl_0123456789ab\";"));
        assert!(!r.contains("SUPERUSER;") && !r.contains(" CREATEROLE") && !r.contains("GRANT pg_"));

        let d = setup_db_sql(&n(), "ws_jobs");
        assert!(d.starts_with("BEGIN;\n") && d.ends_with("COMMIT;\n"));
        assert!(d.contains("CREATE PUBLICATION \"k2_dual_0123456789ab\" FOR ALL TABLES;"));
        // The freeze comes last, after every grant and the publication.
        assert!(d.find("CREATE EVENT TRIGGER k2_dual_freeze ON ddl_command_start").unwrap() > d.find("CREATE PUBLICATION").unwrap());
        assert!(d.contains("SET search_path = pg_catalog"), "the freeze function pins its path");
        assert!(d.contains("REVOKE ALL ON FUNCTION \"_k2_dual\".freeze() FROM PUBLIC;"));
        assert!(d.contains("k2 db upgrade ws_jobs --abort"));

        let s = subscribe_sql(&n(), "ws_jobs", 5432, &pw());
        assert!(s.starts_with(QUIET_SQL) && !s.contains("\nBEGIN;"), "CREATE SUBSCRIPTION with a slot runs outside a transaction");
        assert!(s.contains("host=127.0.0.1 port=5432 dbname=ws_jobs user=k2_repl_0123456789ab"));
        assert!(s.contains("options=''-c row_security=off''"));
        for w in ["run_as_owner = false", "disable_on_error = true", "copy_data = true", "create_slot = true", "slot_name = 'k2_dual_0123456789ab'"] {
            assert!(s.contains(w), "{w}");
        }

        let ds = drop_subscription_sql(&n());
        let (dis, none, drop) = (ds.find("DISABLE").unwrap(), ds.find("slot_name = NONE").unwrap(), ds.find("DROP SUBSCRIPTION").unwrap());
        assert!(dis < none && none < drop, "never reaches the closed publisher (L24)");

        let src = detach_source_sql(&n(), "ws_jobs");
        assert!(src.starts_with("DROP EVENT TRIGGER IF EXISTS k2_dual_freeze;"), "the freeze goes first");
        assert!(src.contains("pg_drop_replication_slot('k2_dual_0123456789ab')"));
        assert!(src.contains("DROP PUBLICATION IF EXISTS \"k2_dual_0123456789ab\";"));
        assert!(!src.contains("DROP ROLE"), "the role stays until teardown");
        let role = drop_role_sql(&n(), "ws_jobs");
        assert!(role.contains("REVOKE CONNECT ON DATABASE \"ws_jobs\"") && role.contains("DROP ROLE \"k2_repl_0123456789ab\""));

        let q = quiesce_repl_sql("ws_jobs");
        assert!(q.contains("CONNECTION LIMIT 0") && q.contains("backend_type <> 'walsender'"));

        let f = force_rls_sql(&[16500, 16501]);
        assert!(f.contains("ARRAY[16500, 16501]") && f.contains("relkind IN ('r', 'p')") && f.contains("RAISE EXCEPTION"));
        assert!(f.trim_end().ends_with("SELECT 2;"));
        assert!(REFRESH_SQL.contains("REFRESH MATERIALIZED VIEW"));
        assert!(status_source_sql(&n()).contains("pg_wal_lsn_diff") && status_target_sql(&n()).contains("srsubstate = 'r'"));
    }
}
