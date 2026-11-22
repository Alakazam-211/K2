//! The door's Postgres: one transaction per call, the caller's role and
//! claims set inside it (PRD §3, §5.2), PostgREST's own model.
//!
//! ```text
//! BEGIN;
//! SET LOCAL ROLE <anon | authenticated | service_role>;
//! SET LOCAL statement_timeout = …;
//! SELECT set_config('request.jwt.claims', <claims>, true),
//!        set_config('request.jwt.claim.sub', <sub>, true);
//! … SQL the door built itself, values as bind parameters …
//! COMMIT;
//! ```
//!
//! The login is the database's `<db>_authenticator` (lane A: NOINHERIT, a
//! member of the three roles, no rights of its own). It has to be: lane A's
//! `auth.uid()`, `auth.jwt()` … read `request.jwt.claims` only when
//! `session_user` is that login, so the same claims set by any other login
//! read as no user. No caller SQL ever runs here, so a caller can't change
//! the role or the claims.
//!
//! **The pool** is a small copy (PRD §13.1 lane D: "until [lane C's pool]
//! lands it uses its own small copy behind the same trait"): idle clients
//! per (login, database), at most [`MAX_IDLE`] kept, a closed one dropped.
//! A transaction a call leaves open is rolled back when it drops
//! (tokio-postgres sends ROLLBACK), so a pooled client never carries a
//! role or claims into the next call: `SET LOCAL` and `set_config(…, true)`
//! end with the transaction.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tokio_postgres::{Client, NoTls, Transaction};

use super::storage_err::{SResult, StorageError};

/// Idle clients kept per (login, database).
pub const MAX_IDLE: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a door connects. Never printed with its password.
#[derive(Clone, PartialEq, Eq)]
pub struct PgTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub dbname: String,
}

impl std::fmt::Debug for PgTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PgTarget({}@{}:{}/{})", self.user, self.host, self.port, self.dbname)
    }
}

impl PgTarget {
    fn key(&self) -> String {
        format!("{}\u{1}{}\u{1}{}\u{1}{}", self.host, self.port, self.user, self.dbname)
    }
}

/// The three roles a door request runs as (Supabase's names, R0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbRole {
    Anon,
    Authenticated,
    Service,
}

impl DbRole {
    pub fn name(self) -> &'static str {
        match self {
            DbRole::Anon => "anon",
            DbRole::Authenticated => "authenticated",
            DbRole::Service => "service_role",
        }
    }

    /// Supabase's defaults (PRD §7.1): 8 s for `authenticated`, 3 s for
    /// `anon`, none for `service_role`.
    pub fn statement_timeout(self) -> &'static str {
        match self {
            DbRole::Anon => "3s",
            DbRole::Authenticated => "8s",
            DbRole::Service => "0",
        }
    }
}

/// Who a call runs as: the role and the claims object of PRD §5.2.
#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub role: DbRole,
    /// `{"sub", "role", "email", "aud", "session_id", "app_metadata", …}`.
    pub claims: Value,
    /// For the audit line: `guest:<user id>`, `service:<key id>`, `anon`.
    pub label: String,
}

impl Caller {
    pub fn service(label: impl Into<String>) -> Self {
        Caller { role: DbRole::Service, claims: serde_json::json!({ "role": "service_role" }), label: label.into() }
    }

    pub fn anon() -> Self {
        Caller { role: DbRole::Anon, claims: serde_json::json!({ "role": "anon" }), label: "anon".into() }
    }

    /// The user id the claims carry (`auth.uid()`), if any.
    pub fn sub(&self) -> Option<&str> {
        self.claims.get("sub").and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    /// `sub` when it is a uuid: what goes into `storage.objects.owner`
    /// (Supabase writes `owner` only for a uuid `sub`; `owner_id` always).
    pub fn owner_uuid(&self) -> Option<String> {
        self.sub().filter(|s| uuid::Uuid::parse_str(s).is_ok()).map(str::to_string)
    }
}

// ── the pool ─────────────────────────────────────────────────────────

fn idle() -> &'static Mutex<HashMap<String, Vec<Client>>> {
    static P: OnceLock<Mutex<HashMap<String, Vec<Client>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// A pooled client; back to the pool on drop unless it closed.
pub struct Conn {
    client: Option<Client>,
    key: String,
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(c) = self.client.take() {
            if c.is_closed() {
                return;
            }
            let mut g = idle().lock().unwrap_or_else(|e| e.into_inner());
            let v = g.entry(self.key.clone()).or_default();
            if v.len() < MAX_IDLE {
                v.push(c);
            }
        }
    }
}

impl Conn {
    pub fn client(&mut self) -> &mut Client {
        self.client.as_mut().expect("a live Conn holds its client")
    }

    /// Drop it instead of pooling it (after an error that may have left
    /// the session in a state the next call shouldn't inherit).
    pub fn discard(mut self) {
        self.client.take();
    }
}

/// A client for `t`: an idle one, else a new connection.
pub async fn connect(t: &PgTarget) -> SResult<Conn> {
    let key = t.key();
    loop {
        let c = idle().lock().unwrap_or_else(|e| e.into_inner()).get_mut(&key).and_then(Vec::pop);
        match c {
            Some(c) if !c.is_closed() => return Ok(Conn { client: Some(c), key }),
            Some(_) => continue,
            None => break,
        }
    }
    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&t.host)
        .port(t.port)
        .user(&t.user)
        .password(t.password.as_bytes())
        .dbname(&t.dbname)
        .application_name("k2-storage-door")
        .connect_timeout(CONNECT_TIMEOUT);
    let (client, connection) = cfg
        .connect(NoTls)
        .await
        .map_err(|e| StorageError::unavailable(format!("can't reach the App's database: {}", short(&e))))?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            k2_core::log_debug!("[storage] postgres connection ended: {e}");
        }
    });
    Ok(Conn { client: Some(client), key })
}

/// A connection error without the login's details.
fn short(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(d) => format!("{} ({})", d.message(), d.code().code()),
        None => e.to_string(),
    }
}

/// Begin a transaction as `caller`: the role, its statement timeout and
/// the claims, all local to the transaction.
pub async fn begin_as<'a>(conn: &'a mut Conn, caller: &Caller) -> SResult<Transaction<'a>> {
    let tx = conn.client().transaction().await.map_err(|e| StorageError::from_pg(&e))?;
    // Both names are fixed strings from `DbRole`, never caller text.
    tx.batch_execute(&format!(
        "SET LOCAL ROLE {}; SET LOCAL statement_timeout = '{}'",
        caller.role.name(),
        caller.role.statement_timeout()
    ))
    .await
    .map_err(|e| StorageError::from_pg(&e))?;
    let claims = caller.claims.to_string();
    let sub = caller.sub().unwrap_or("").to_string();
    tx.execute("SELECT set_config('request.jwt.claims', $1, true), set_config('request.jwt.claim.sub', $2, true)", &[&claims, &sub])
    .await
    .map_err(|e| StorageError::from_pg(&e))?;
    Ok(tx)
}

/// Map a driver error.
pub fn pgerr(e: tokio_postgres::Error) -> StorageError {
    StorageError::from_pg(&e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_timeouts() {
        assert_eq!(DbRole::Service.name(), "service_role");
        assert_eq!(DbRole::Authenticated.statement_timeout(), "8s");
        assert_eq!(DbRole::Anon.statement_timeout(), "3s");
        assert_eq!(DbRole::Service.statement_timeout(), "0");
    }

    #[test]
    fn owner_is_a_uuid_sub_only() {
        let u = "6f1c4a52-0b7d-4d0e-9a2c-3b8f1e7d9a10";
        let c = Caller { role: DbRole::Authenticated, claims: serde_json::json!({"sub": u}), label: "x".into() };
        assert_eq!(c.owner_uuid().as_deref(), Some(u));
        let c = Caller { role: DbRole::Authenticated, claims: serde_json::json!({"sub": "firebase-abc"}), label: "x".into() };
        assert_eq!(c.sub(), Some("firebase-abc"));
        assert_eq!(c.owner_uuid(), None, "a non-uuid sub (a third-party id) is owner_id only");
        assert_eq!(Caller::anon().sub(), None);
    }

    #[test]
    fn a_target_never_prints_its_password() {
        let t = PgTarget { host: "127.0.0.1".into(), port: 5432, user: "db_authenticator".into(), password: "hunter2".into(), dbname: "db".into() };
        assert!(!format!("{t:?}").contains("hunter2"));
    }
}
