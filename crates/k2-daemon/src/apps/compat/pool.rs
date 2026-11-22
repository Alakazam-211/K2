//! The data doors' Postgres connections (prd-app-supabase-compat-v1 §3,
//! §7.1; vs-live §3 item 2).
//!
//! **One pool per compat database**, logged in as `<db>_authenticator`: a
//! login with no rights of its own (NOINHERIT) that is a member of `anon`,
//! `authenticated` and `service_role`. This is PostgREST's own model.
//!
//! **Per request, one transaction** ([`run`]):
//! ```text
//! BEGIN
//! SELECT set_config('role', <anon|authenticated|service_role>, true),
//!        set_config('request.jwt.claims', <claims JSON>, true),
//!        set_config('request.jwt.claim.sub', <sub>, true),
//!        set_config('request.method' / 'request.path' / 'request.headers', …, true),
//!        set_config('search_path', <exposed schema>, true),
//!        set_config('statement_timeout', <the role's limit>, true)
//! <the statements the door built itself>
//! COMMIT            (ROLLBACK when a statement fails or the door's check says so)
//! ```
//! Every setting is transaction-local (`is_local = true`), so it ends with
//! the transaction: the next request on the same connection starts as the
//! bare authenticator. The role is one of three constants ([`DbRole`]),
//! never a string from the request. The claims come from the daemon (lane
//! A's §5.2 contract), never from the caller. No door runs SQL text a
//! caller sent.
//!
//! **Recycling.** A connection goes back to the pool through deadpool's
//! `Clean` recycle (`CLOSE ALL; SET SESSION AUTHORIZATION DEFAULT; RESET
//! ALL; UNLISTEN *; pg_advisory_unlock_all(); DISCARD TEMP; DISCARD
//! SEQUENCES`), so even a function that set something for the whole
//! session (`set_config(…, false)`, a temp table) leaves nothing for the
//! next caller.
//!
//! **Where it connects.** 127.0.0.1 at the port of the cluster K2's records
//! name for the database (`sql::cluster::for_db`), with the login's
//! password read from the db secret store by the ref lane A recorded. A
//! changed password (a rotation) gets a new pool on the next request.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_postgres::{NoTls, Row};

use super::door_config::DoorConfig;

/// How long a request waits for a free connection.
const WAIT: Duration = Duration::from_secs(5);
/// How long dialing and logging in may take.
const CONNECT: Duration = Duration::from_secs(5);
/// Connections per compat database (App logins have CONNECTION LIMIT 20;
/// the pool stays well under it).
pub const POOL_SIZE: u32 = 10;

/// The three roles a request may run as. Never anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbRole {
    Anon,
    Authenticated,
    Service,
}

impl DbRole {
    /// The Postgres role name (Supabase's standard names, PRD R0).
    pub fn name(self) -> &'static str {
        match self {
            DbRole::Anon => "anon",
            DbRole::Authenticated => "authenticated",
            DbRole::Service => "service_role",
        }
    }

    /// The statement timeout for this role from the App's limits (ms; 0 = none).
    pub fn timeout_ms(self, cfg: &DoorConfig) -> u32 {
        match self {
            DbRole::Anon => cfg.limits.anon_timeout_ms,
            DbRole::Authenticated => cfg.limits.authenticated_timeout_ms,
            DbRole::Service => cfg.limits.service_timeout_ms.unwrap_or(0),
        }
    }
}

/// Who a request runs as: the role and the claims the daemon built.
#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub role: DbRole,
    /// The `request.jwt.claims` object (PRD §5.2).
    pub claims: Value,
}

impl Caller {
    /// `sub` from the claims, or empty (anon, service).
    pub fn sub(&self) -> String {
        self.claims.get("sub").and_then(Value::as_str).unwrap_or("").to_string()
    }
}

/// What the request's own settings carry (`request.method`, `request.path`,
/// `request.headers`), as PostgREST sets them for functions to read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestMeta {
    pub method: String,
    pub path: String,
    /// Lower-case header names to values, without `cookie`,
    /// `authorization` and `apikey` (credentials never reach SQL).
    pub headers: serde_json::Map<String, Value>,
    /// The schema the request is about (`Accept-Profile` /
    /// `Content-Profile`), first on the search path.
    pub schema: String,
}

/// Where one pool connects.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub db: String,
    pub user: String,
    pub password: String,
    pub size: u32,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("db", &self.db)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("size", &self.size)
            .finish()
    }
}

/// A Postgres error, in the parts PostgREST's error JSON has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgFail {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
    pub hint: Option<String>,
}

/// Why [`run`] didn't commit.
#[derive(Debug)]
pub enum RunError<E> {
    /// Postgres refused a statement (the transaction rolled back).
    Pg(PgFail),
    /// No connection: the pool is exhausted, the cluster is down, or the
    /// login failed. The door answers 503.
    Unavailable(String),
    /// The door's own check of the rows refused (rolled back).
    Check(E),
}

fn pg_fail(e: &tokio_postgres::Error) -> Option<PgFail> {
    let db = e.as_db_error()?;
    Some(PgFail {
        code: db.code().code().to_string(),
        message: db.message().to_string(),
        details: db.detail().map(str::to_string),
        hint: db.hint().map(str::to_string),
    })
}

/// A pool checkout's failure in words (the server's own message when it
/// sent one: a failed login says which).
fn checkout(e: deadpool_postgres::PoolError) -> String {
    match &e {
        deadpool_postgres::PoolError::Backend(b) => match b.as_db_error() {
            Some(d) => format!("no database connection: {} ({})", d.message(), d.code().code()),
            None => format!("no database connection: {b}"),
        },
        _ => format!("no database connection: {e}"),
    }
}

fn classify<E>(e: tokio_postgres::Error) -> RunError<E> {
    match pg_fail(&e) {
        Some(p) => RunError::Pg(p),
        None => RunError::Unavailable(format!("database connection: {e}")),
    }
}

// ── the registry ─────────────────────────────────────────────────────

struct Held {
    /// A digest of the password the pool logs in with.
    pw: [u8; 32],
    size: u32,
    pool: Pool,
}

fn registry() -> &'static Mutex<HashMap<(String, u16, String, String), Held>> {
    static R: OnceLock<Mutex<HashMap<(String, u16, String, String), Held>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn digest(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

fn build(t: &Target) -> Result<Pool, String> {
    let mut pg = tokio_postgres::Config::new();
    pg.host(&t.host)
        .port(t.port)
        .dbname(&t.db)
        .user(&t.user)
        .password(t.password.as_bytes())
        .application_name("k2-compat-door")
        .connect_timeout(CONNECT);
    let mgr = Manager::from_config(pg, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Clean });
    Pool::builder(mgr)
        .max_size(t.size.max(1) as usize)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(WAIT))
        .create_timeout(Some(CONNECT))
        .recycle_timeout(Some(CONNECT))
        .build()
        .map_err(|e| format!("pool: {e}"))
}

/// The pool for `t`: the one held, or a new one when there is none or the
/// password or size changed (the old pool's connections close as their
/// requests end).
pub fn pool_for(t: &Target) -> Result<Pool, String> {
    let key = (t.host.clone(), t.port, t.db.clone(), t.user.clone());
    let pw = digest(&t.password);
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = g.get(&key) {
        if h.pw == pw && h.size == t.size {
            return Ok(h.pool.clone());
        }
    }
    let pool = build(t)?;
    if let Some(old) = g.insert(key, Held { pw, size: t.size, pool: pool.clone() }) {
        old.pool.close();
    }
    Ok(pool)
}

/// Close and forget every pool for database `db` (compat turned off, the
/// database dropped or moved).
pub fn forget_db(db: &str) {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|k, h| {
        let keep = k.2 != db;
        if !keep {
            h.pool.close();
        }
        keep
    });
}

/// Where `cfg`'s pool connects: the database's cluster on loopback and the
/// authenticator's password from the db secret store. Blocking (reads K2's
/// records and the vault): call it off the async threads.
pub fn target_for(cfg: &DoorConfig) -> Result<Target, String> {
    #[cfg(test)]
    {
        if let Some(t) = test_target(&cfg.app_id) {
            return Ok(t);
        }
    }
    let cluster = crate::sql::cluster::for_db(&cfg.database)?;
    let password = cfg.authenticator_password()?;
    Ok(Target { host: "127.0.0.1".into(), port: cluster.port, db: cfg.database.clone(), user: cfg.authenticator.clone(), password, size: POOL_SIZE })
}

#[cfg(test)]
fn test_targets() -> &'static Mutex<HashMap<String, Target>> {
    static T: OnceLock<Mutex<HashMap<String, Target>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

#[cfg(test)]
fn test_target(app_id: &str) -> Option<Target> {
    test_targets().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).cloned()
}

/// Tests: App `app_id`'s pool connects to `t` (a throwaway server).
#[cfg(test)]
pub fn set_test_target(app_id: &str, t: Target) {
    test_targets().lock().unwrap_or_else(|e| e.into_inner()).insert(app_id.to_string(), t);
}

// ── one request ──────────────────────────────────────────────────────

/// The setup statement: role, claims and the request's own settings, all
/// transaction-local. Parameters only; nothing is spliced into the text.
const SETUP: &str = "SELECT set_config('role', $1, true), \
    set_config('request.jwt.claims', $2, true), \
    set_config('request.jwt.claim.sub', $3, true), \
    set_config('request.method', $4, true), \
    set_config('request.path', $5, true), \
    set_config('request.headers', $6, true), \
    set_config('request.cookies', '{}', true), \
    set_config('search_path', $7, true), \
    set_config('statement_timeout', $8, true)";

/// `"schema", public` (identifier-quoted), the request's search path.
pub fn search_path(schema: &str) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    if schema.is_empty() || schema == "public" {
        q("public")
    } else {
        format!("{}, {}", q(schema), q("public"))
    }
}

/// Run `stmts` in one transaction as `caller`, then `check` the rows before
/// committing: `Err` from `check` rolls back (PostgREST rolls back a write
/// whose answer breaks `Accept: …object+json`, for one). Each statement's
/// rows come back in order.
pub async fn run<E>(
    pool: &Pool,
    caller: &Caller,
    meta: &RequestMeta,
    timeout_ms: u32,
    stmts: &[String],
    check: impl FnOnce(&[Vec<Row>]) -> Result<(), E>,
) -> Result<Vec<Vec<Row>>, RunError<E>> {
    let mut client = pool.get().await.map_err(|e| RunError::Unavailable(checkout(e)))?;
    let tx = client.transaction().await.map_err(classify)?;
    let claims = caller.claims.to_string();
    let sub = caller.sub();
    let headers = Value::Object(meta.headers.clone()).to_string();
    let path = search_path(&meta.schema);
    let timeout = timeout_ms.to_string();
    tx.execute(SETUP, &[&caller.role.name(), &claims, &sub, &meta.method, &meta.path, &headers, &path, &timeout])
        .await
        .map_err(classify)?;
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        match tx.query(s.as_str(), &[]).await {
            Ok(rows) => out.push(rows),
            Err(e) => {
                let err = classify(e);
                let _ = tx.rollback().await;
                return Err(err);
            }
        }
    }
    if let Err(e) = check(&out) {
        let _ = tx.rollback().await;
        return Err(RunError::Check(e));
    }
    tx.commit().await.map_err(classify)?;
    Ok(out)
}

/// Any text-like column as a string: text, name, json (its wire form is its
/// text), jsonb (a version byte, then its text). The grammar's statements
/// answer only these.
pub struct AnyText(pub String);

impl<'a> tokio_postgres::types::FromSql<'a> for AnyText {
    fn from_sql(ty: &tokio_postgres::types::Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let raw = if *ty == tokio_postgres::types::Type::JSONB { raw.get(1..).unwrap_or_default() } else { raw };
        Ok(AnyText(String::from_utf8(raw.to_vec())?))
    }
    fn accepts(ty: &tokio_postgres::types::Type) -> bool {
        use tokio_postgres::types::Type;
        matches!(*ty, Type::TEXT | Type::VARCHAR | Type::NAME | Type::JSON | Type::JSONB | Type::UNKNOWN | Type::BPCHAR)
    }
}

/// [`run`] for the grammar's statements (`k2_rest::Stmt`): rows come back
/// as text columns, a write without rows as its affected count, and
/// `check` shapes the answer from them before the commit (its `Err` rolls
/// back). `read_only`: the transaction is READ ONLY (GET and HEAD).
pub async fn run_stmts<T, E>(
    pool: &Pool,
    caller: &Caller,
    meta: &RequestMeta,
    timeout_ms: u32,
    read_only: bool,
    stmts: &[k2_rest::Stmt],
    check: impl FnOnce(&[k2_rest::Out]) -> Result<T, E>,
) -> Result<T, RunError<E>> {
    let mut client = pool.get().await.map_err(|e| RunError::Unavailable(checkout(e)))?;
    let tx = client.build_transaction().read_only(read_only).start().await.map_err(classify)?;
    let claims = caller.claims.to_string();
    let sub = caller.sub();
    let headers = Value::Object(meta.headers.clone()).to_string();
    let path = search_path(&meta.schema);
    let timeout = timeout_ms.to_string();
    tx.execute(SETUP, &[&caller.role.name(), &claims, &sub, &meta.method, &meta.path, &headers, &path, &timeout])
        .await
        .map_err(classify)?;
    let mut outs = Vec::with_capacity(stmts.len());
    for s in stmts {
        let got = match s {
            k2_rest::Stmt::Rows(sql) => tx.query(sql.as_str(), &[]).await.and_then(|rows| {
                rows.iter()
                    .map(|row| (0..row.len()).map(|i| row.try_get::<_, Option<AnyText>>(i).map(|t| t.map(|t| t.0))).collect::<Result<Vec<_>, _>>())
                    .collect::<Result<Vec<_>, _>>()
                    .map(k2_rest::Out::Rows)
            }),
            k2_rest::Stmt::Exec(sql) => tx.execute(sql.as_str(), &[]).await.map(k2_rest::Out::Affected),
        };
        match got {
            Ok(o) => outs.push(o),
            Err(e) => {
                let err = classify(e);
                let _ = tx.rollback().await;
                return Err(err);
            }
        }
    }
    match check(&outs) {
        Ok(t) => {
            tx.commit().await.map_err(classify)?;
            Ok(t)
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(RunError::Check(e))
        }
    }
}

/// The schema cache's one statement, on a bare connection (the catalogs are
/// readable by the authenticator itself).
pub async fn introspect(pool: &Pool, schemas: &[String]) -> Result<k2_rest::SchemaCache, String> {
    let client = pool.get().await.map_err(checkout)?;
    let row = client.query_one(k2_rest::introspect_sql(schemas).as_str(), &[]).await.map_err(|e| format!("schema cache: {e}"))?;
    let text = row.try_get::<_, AnyText>(0).map_err(|e| format!("schema cache: {e}"))?.0;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("schema cache: {e}"))?;
    k2_rest::SchemaCache::from_json(&v)
}

#[cfg(test)]
#[path = "pool_real_pg.rs"]
mod real_pg;
