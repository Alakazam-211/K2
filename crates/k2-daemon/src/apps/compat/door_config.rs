//! One App's Supabase-compatible layer settings (`app_compat`, migration
//! 0326): what `k2 app compat on|set` wrote, read live by the doors
//! (lane C: [`for_app`]) and by the identity code here.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Supabase's defaults (§7.1): 1,000 rows a response, 8 s a statement for
/// `authenticated`, 3 s for `anon`, none for `service_role`, 10 MiB bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
    pub max_rows: u32,
    pub authenticated_timeout_ms: u32,
    pub anon_timeout_ms: u32,
    pub service_timeout_ms: Option<u32>,
    pub max_body_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_rows: 1000, authenticated_timeout_ms: 8000, anon_timeout_ms: 3000, service_timeout_ms: None, max_body_bytes: 10 * 1024 * 1024 }
    }
}

/// What `sub` is on a third-party token's requests (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SubMode {
    /// (a) the issuer's own id (the cut-over setting).
    #[default]
    Token,
    /// (b) the App user's id; the issuer's id moves to `<issuer_key>_uid`.
    User,
}

/// A valid third-party token whose email is no user of the App (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UnknownEmail {
    /// 403 `not_a_user` (the default).
    #[default]
    Refuse,
    /// Supabase's behaviour: `authenticated` with the token's own claims.
    Authenticated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoorConfig {
    pub app_id: String,
    /// The database the doors bind to (never the workspace's oldest).
    pub database: String,
    pub database_id: String,
    /// The door's login, `<db>_authenticator`.
    pub authenticator: String,
    /// Its password's ref in the db secret store.
    pub authenticator_secret_ref: String,
    pub anon: bool,
    pub service_key_public: bool,
    pub origins: Vec<String>,
    pub schemas: Vec<String>,
    pub limits: Limits,
    pub sub_mode: SubMode,
    pub unknown_email: UnknownEmail,
}

impl DoorConfig {
    /// The door login's password, from the db secret store.
    pub fn authenticator_password(&self) -> Result<String, String> {
        use crate::sql::secrets::SecretStore as _;
        crate::sql::secrets::FileSecretStore::default()
            .resolve(&self.authenticator_secret_ref)?
            .ok_or_else(|| format!("the door login's password for {} is missing from the vault ({cli} app compat on {} again)", self.app_id, self.app_id, cli = k2_core::brand::CLI_NAME))
    }
}

/// The row as stored, with what the doors don't need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub config: DoorConfig,
    pub claims_key_ref: String,
    pub users_synced_at: Option<i64>,
    pub created_by: String,
    pub created_at: i64,
}

const COLS: &str = "app_id, database_name, database_id, authenticator_role, authenticator_secret_ref, anon, service_key_public, origins, \
                    schemas, limits, sub_mode, unknown_email, claims_key_ref, users_synced_at, created_by, created_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    let list = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
    Ok(Stored {
        config: DoorConfig {
            app_id: r.get(0)?,
            database: r.get(1)?,
            database_id: r.get(2)?,
            authenticator: r.get(3)?,
            authenticator_secret_ref: r.get(4)?,
            anon: r.get::<_, i64>(5)? != 0,
            service_key_public: r.get::<_, i64>(6)? != 0,
            origins: list(r.get(7)?),
            schemas: list(r.get(8)?),
            limits: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
            sub_mode: if r.get::<_, String>(10)? == "user" { SubMode::User } else { SubMode::Token },
            unknown_email: if r.get::<_, String>(11)? == "authenticated" { UnknownEmail::Authenticated } else { UnknownEmail::Refuse },
        },
        claims_key_ref: r.get(12)?,
        users_synced_at: r.get(13)?,
        created_by: r.get(14)?,
        created_at: r.get(15)?,
    })
}

pub(crate) fn load_in(c: &Connection, app_id: &str) -> Result<Option<Stored>, String> {
    c.query_row(&format!("SELECT {COLS} FROM app_compat WHERE app_id = ?1"), params![app_id], row)
        .optional()
        .map_err(|e| format!("app_compat: {e}"))
}

pub(crate) fn load(app_id: &str) -> Result<Option<Stored>, String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    load_in(&c, app_id)
}

/// Every App with the layer on.
pub(crate) fn all() -> Result<Vec<Stored>, String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    let mut st = c.prepare(&format!("SELECT {COLS} FROM app_compat ORDER BY app_id")).map_err(|e| format!("app_compat: {e}"))?;
    let rows = st.query_map([], row).map_err(|e| format!("app_compat: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("app_compat: {e}"))
}

/// The Apps with the layer on for database `db`.
pub(crate) fn for_database(db: &str) -> Result<Vec<Stored>, String> {
    Ok(all()?.into_iter().filter(|s| s.config.database == db).collect())
}

/// Lane C's read: the App's door settings, live. `None`: the layer is off
/// for the App (or its row can't be read; logged).
pub fn for_app(app_id: &str) -> Option<DoorConfig> {
    match load(app_id) {
        Ok(s) => s.map(|s| s.config),
        Err(e) => {
            k2_core::log_debug!("[compat] {app_id}: {e}");
            None
        }
    }
}

pub(crate) fn upsert(s: &Stored, now: i64) -> Result<(), String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    let cfg = &s.config;
    let json = |v: &Vec<String>| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
    c.execute(
        "INSERT INTO app_compat (app_id, database_id, database_name, authenticator_role, authenticator_secret_ref, claims_key_ref, \
           anon, service_key_public, origins, schemas, limits, sub_mode, unknown_email, users_synced_at, created_by, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16) \
         ON CONFLICT(app_id) DO UPDATE SET database_id = excluded.database_id, database_name = excluded.database_name, \
           authenticator_role = excluded.authenticator_role, authenticator_secret_ref = excluded.authenticator_secret_ref, \
           claims_key_ref = excluded.claims_key_ref, anon = excluded.anon, service_key_public = excluded.service_key_public, \
           origins = excluded.origins, schemas = excluded.schemas, limits = excluded.limits, sub_mode = excluded.sub_mode, \
           unknown_email = excluded.unknown_email, users_synced_at = excluded.users_synced_at, updated_at = ?16",
        params![
            cfg.app_id,
            cfg.database_id,
            cfg.database,
            cfg.authenticator,
            cfg.authenticator_secret_ref,
            s.claims_key_ref,
            cfg.anon as i64,
            cfg.service_key_public as i64,
            json(&cfg.origins),
            json(&cfg.schemas),
            serde_json::to_string(&cfg.limits).unwrap_or_else(|_| "{}".into()),
            match cfg.sub_mode {
                SubMode::Token => "token",
                SubMode::User => "user",
            },
            match cfg.unknown_email {
                UnknownEmail::Refuse => "refuse",
                UnknownEmail::Authenticated => "authenticated",
            },
            s.users_synced_at,
            s.created_by,
            now,
        ],
    )
    .map_err(|e| format!("app_compat: {e}"))?;
    Ok(())
}

pub(crate) fn mark_synced(app_id: &str, now: i64) -> Result<(), String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    c.execute("UPDATE app_compat SET users_synced_at = ?2 WHERE app_id = ?1", params![app_id, now]).map_err(|e| format!("app_compat: {e}"))?;
    Ok(())
}

pub(crate) fn remove(app_id: &str) -> Result<bool, String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    c.execute("DELETE FROM app_identity_issuers WHERE app_id = ?1", params![app_id]).map_err(|e| format!("app_compat: {e}"))?;
    Ok(c.execute("DELETE FROM app_compat WHERE app_id = ?1", params![app_id]).map_err(|e| format!("app_compat: {e}"))? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(app: &str) -> Stored {
        Stored {
            config: DoorConfig {
                app_id: app.into(),
                database: "ws_x_main".into(),
                database_id: "db-1".into(),
                authenticator: "ws_x_main_authenticator".into(),
                authenticator_secret_ref: "ref-pw".into(),
                anon: false,
                service_key_public: false,
                origins: vec![],
                schemas: vec!["public".into()],
                limits: Limits::default(),
                sub_mode: SubMode::Token,
                unknown_email: UnknownEmail::Refuse,
            },
            claims_key_ref: "ref-key".into(),
            users_synced_at: None,
            created_by: "owner".into(),
            created_at: 5,
        }
    }

    #[test]
    fn the_row_round_trips_and_off_is_none() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        assert_eq!(for_app("crm"), None);
        let mut s = stored("crm");
        upsert(&s, 5).unwrap();
        assert_eq!(for_app("crm"), Some(s.config.clone()));
        assert_eq!(for_app("crm").unwrap().limits.max_rows, 1000);
        s.config.anon = true;
        s.config.sub_mode = SubMode::User;
        s.config.unknown_email = UnknownEmail::Authenticated;
        s.config.origins = vec!["https://a.example".into()];
        s.config.limits.max_rows = 50;
        upsert(&s, 6).unwrap();
        let back = load("crm").unwrap().unwrap();
        assert_eq!(back.config, s.config);
        assert_eq!(back.created_at, 5, "created_at stays");
        assert_eq!(for_database("ws_x_main").unwrap().len(), 1);
        assert!(remove("crm").unwrap());
        assert_eq!(for_app("crm"), None);
    }
}
