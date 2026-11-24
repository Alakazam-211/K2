//! Which database an App's storage door uses, and how the door logs in.
//!
//! **The settings** are lane A's [`DoorConfig`]
//! (`door_config::for_app`): the App's compat database, its
//! `<db>_authenticator` login and password ref, the anon lane, the
//! server-key-on-the-public-host switch and the extra origins. Storage
//! answers for an App exactly while compat is on for it (PRD §1 R7).
//!
//! **The login** ([`authenticator_target`]) is that authenticator. It must
//! be: `auth.uid()` and the rest read `request.jwt.claims` only when
//! `session_user` is the database's authenticator (lane A's rule), so a
//! policy run under any other login sees no user.
//!
//! **The schema install** ([`install_schema`]) runs as the database's
//! migrator, which owns the App's tables (PRD §2 "Roles"), so the App's own
//! migrations can write policies on `storage.objects`.

use super::door_config::DoorConfig;
use super::storage_err::{SResult, StorageError};
use super::storage_pg::PgTarget;

/// The App's door settings, if compat is on for it.
pub fn config(app_id: &str) -> Option<DoorConfig> {
    #[cfg(test)]
    if let Some(c) = test_seams::config(app_id) {
        return c;
    }
    super::door_config::for_app(app_id)
}

/// Does the App path read `app_id`'s requests for this door at all?
pub fn watching(app_id: &str) -> bool {
    config(app_id).is_some()
}

/// The door's login: `<db>_authenticator` on the database's own cluster.
pub fn authenticator_target(cfg: &DoorConfig) -> SResult<PgTarget> {
    #[cfg(test)]
    if let Some(t) = test_seams::target(&cfg.database) {
        return Ok(t);
    }
    let cluster = crate::sql::cluster::for_db(&cfg.database)
        .map_err(|e| StorageError::unavailable(format!("the App's database {}: {e}", cfg.database)))?;
    let password = cfg.authenticator_password().map_err(StorageError::unavailable)?;
    Ok(PgTarget { host: "127.0.0.1".into(), port: cluster.port, user: cfg.authenticator.clone(), password, dbname: cfg.database.clone() })
}

/// The migrator's login for `db` (the schema install).
pub fn migrator_target(db: &str) -> SResult<PgTarget> {
    #[cfg(test)]
    if let Some(t) = test_seams::migrator(db) {
        return Ok(t);
    }
    let row = crate::sql::ops::active_row_by_name(db).map_err(|e| StorageError::unavailable(format!("{e:?}")))?;
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let (user, password) = crate::sql::ops::migrator_creds(&secrets, &row).map_err(|e| StorageError::unavailable(format!("{e:?}")))?;
    let cluster = crate::sql::cluster::for_db(db).map_err(StorageError::unavailable)?;
    Ok(PgTarget { host: "127.0.0.1".into(), port: cluster.port, user, password, dbname: db.to_string() })
}

/// Install the `storage` schema into `db` as its migrator. Refused, with
/// nothing run, when the cluster lacks the three standard roles.
pub async fn install_schema(db: &str) -> SResult<()> {
    let t = migrator_target(db)?;
    let mut conn = super::storage_pg::connect(&t).await?;
    let c = conn.client();
    let missing: Vec<String> = c
        .query(super::storage_sql::MISSING_ROLES_SQL, &[])
        .await
        .map_err(super::storage_pg::pgerr)?
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect();
    if !missing.is_empty() {
        return Err(StorageError::unavailable(format!(
            "the cluster has no {} role yet: turn the Supabase-compatible layer on first ({cli} app compat on)",
            missing.join(", "),
            cli = k2_core::brand::CLI_NAME
        )));
    }
    c.batch_execute(super::storage_sql::SCHEMA).await.map_err(super::storage_pg::pgerr)?;
    Ok(())
}

/// Is the `storage` schema in `db`? (Read as the door's login.)
pub async fn schema_present(t: &PgTarget) -> SResult<bool> {
    let mut conn = super::storage_pg::connect(t).await?;
    let row = conn
        .client()
        .query_one("SELECT to_regclass('storage.objects') IS NOT NULL AND to_regclass('storage.buckets') IS NOT NULL", &[])
        .await
        .map_err(super::storage_pg::pgerr)?;
    Ok(row.get(0))
}

#[cfg(test)]
pub mod test_seams {
    //! Door settings and logins by name, without a box: the real-Postgres
    //! tests point a test App at a throwaway server.
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    #[derive(Default)]
    struct Seams {
        configs: HashMap<String, Option<DoorConfig>>,
        targets: HashMap<String, PgTarget>,
        migrators: HashMap<String, PgTarget>,
    }

    fn seams() -> &'static Mutex<Seams> {
        static S: OnceLock<Mutex<Seams>> = OnceLock::new();
        S.get_or_init(Default::default)
    }

    pub fn config(app_id: &str) -> Option<Option<DoorConfig>> {
        seams().lock().unwrap_or_else(|e| e.into_inner()).configs.get(app_id).cloned()
    }
    pub fn target(db: &str) -> Option<PgTarget> {
        seams().lock().unwrap_or_else(|e| e.into_inner()).targets.get(db).cloned()
    }
    pub fn migrator(db: &str) -> Option<PgTarget> {
        seams().lock().unwrap_or_else(|e| e.into_inner()).migrators.get(db).cloned()
    }
    pub fn set_config(app_id: &str, c: Option<DoorConfig>) {
        seams().lock().unwrap_or_else(|e| e.into_inner()).configs.insert(app_id.to_string(), c);
    }
    pub fn set_targets(db: &str, door: PgTarget, migrator: PgTarget) {
        let mut g = seams().lock().unwrap_or_else(|e| e.into_inner());
        g.targets.insert(db.to_string(), door);
        g.migrators.insert(db.to_string(), migrator);
    }

    /// A door config for tests: compat on for `app_id` on `db`.
    pub fn config_for(app_id: &str, db: &str) -> DoorConfig {
        DoorConfig {
            app_id: app_id.to_string(),
            database: db.to_string(),
            database_id: format!("dbid-{db}"),
            authenticator: format!("{db}_authenticator"),
            authenticator_secret_ref: "test".into(),
            anon: false,
            service_key_public: false,
            origins: vec![],
            schemas: vec!["public".into()],
            limits: Default::default(),
            sub_mode: Default::default(),
            unknown_email: Default::default(),
        }
    }
}
