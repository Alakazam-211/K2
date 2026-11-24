//! `/rest/v1`: the PostgREST-compatible data door (prd-app-supabase-compat-v1
//! §7). The grammar and the SQL builder are `k2_rest`; this file keeps the
//! schema cache, runs what the grammar builds as the caller (one
//! transaction through the App's pool, `pool::run_stmts`), and counts the
//! grammar's refusals for `k2 app rest unsupported`.
//!
//! **The schema cache** (PRD §7.1) is per database and per exposed-schema
//! list. It loads on first use, again after [`CACHE_TTL`], at once on
//! [`reload`] (`k2 app rest reload`, after `k2 db migrate`), and once when a
//! request names a table, function or relationship the cache doesn't have
//! (at most every [`MISS_RELOAD`]): a migration that just ran is seen on the
//! next request without anyone asking.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::door::{refuse, Ctx, Resolved};
use super::http::{percent_decode, DoorAnswer, DoorRequest};
use super::pool::{self, DbRole, RequestMeta, RunError};

/// A loaded cache is trusted this long.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
/// A miss reloads the cache at most this often.
pub const MISS_RELOAD: Duration = Duration::from_secs(2);

/// The headers a function may read as `request.headers` (never a credential).
pub fn meta_of(req: &DoorRequest, schema: &str) -> RequestMeta {
    let mut headers = serde_json::Map::new();
    for (k, v) in &req.headers {
        if matches!(k.as_str(), "cookie" | "authorization" | "apikey") {
            continue;
        }
        headers.entry(k.clone()).or_insert_with(|| serde_json::Value::String(v.clone()));
    }
    RequestMeta { method: req.method.clone(), path: req.path.clone(), headers, schema: schema.to_string() }
}

/// The App's pool, or the 503 that says why there is none.
pub async fn pool_of(ctx: &Ctx) -> Result<deadpool_postgres::Pool, DoorAnswer> {
    let cfg = ctx.cfg.clone();
    let t = super::door::blocking(move || pool::target_for(&cfg)).await;
    match t {
        Some(Ok(t)) => pool::pool_for(&t).map_err(|e| refuse(503, "db_unavailable", e, None)),
        Some(Err(e)) => Err(refuse(503, "db_unavailable", e, Some(concat!(k2_core::cli_name!(), " app compat status <app>")))),
        None => Err(refuse(500, "failed", "worker join", None)),
    }
}

// ── the schema cache ─────────────────────────────────────────────────

struct Cached {
    at: Instant,
    cache: Arc<k2_rest::SchemaCache>,
}

type CacheKey = (String, Vec<String>);

fn caches() -> &'static Mutex<HashMap<CacheKey, Cached>> {
    static C: OnceLock<Mutex<HashMap<CacheKey, Cached>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Drop database `db`'s cache: the next request reads the catalogs again.
pub fn reload(db: &str) {
    caches().lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| k.0 != db);
}

async fn cache_for(pool: &deadpool_postgres::Pool, db: &str, schemas: &[String], fresher_than: Option<Duration>) -> Result<Arc<k2_rest::SchemaCache>, String> {
    let key = (db.to_string(), schemas.to_vec());
    {
        let g = caches().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.get(&key) {
            let max = fresher_than.unwrap_or(CACHE_TTL);
            if c.at.elapsed() < max {
                return Ok(c.cache.clone());
            }
        }
    }
    let cache = Arc::new(pool::introspect(pool, schemas).await?);
    caches().lock().unwrap_or_else(|e| e.into_inner()).insert(key, Cached { at: Instant::now(), cache: cache.clone() });
    Ok(cache)
}

// ── refusals the App's status counts ─────────────────────────────────

fn unsupported_counts() -> &'static Mutex<HashMap<String, HashMap<String, u64>>> {
    static U: OnceLock<Mutex<HashMap<String, HashMap<String, u64>>>> = OnceLock::new();
    U.get_or_init(Default::default)
}

fn count_unsupported(app_id: &str, message: &str) {
    let mut g = unsupported_counts().lock().unwrap_or_else(|e| e.into_inner());
    *g.entry(app_id.to_string()).or_default().entry(message.to_string()).or_insert(0) += 1;
}

/// The grammar parts this App's requests asked for and the door refused
/// (since the daemon started), most asked first.
pub fn unsupported(app_id: &str) -> Vec<(String, u64)> {
    let g = unsupported_counts().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<(String, u64)> = g.get(app_id).map(|m| m.iter().map(|(k, n)| (k.clone(), *n)).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

// ── one request ──────────────────────────────────────────────────────

fn api_error(e: &k2_rest::ApiError) -> DoorAnswer {
    DoorAnswer::json(e.status, &e.body())
}

fn is_miss(e: &k2_rest::ApiError) -> bool {
    matches!(e.code.as_str(), "PGRST200" | "PGRST202" | "PGRST205" | "42P01")
}

/// The grammar's view of the request.
pub fn grammar_request(req: &DoorRequest) -> k2_rest::Request {
    let rest = req.path.strip_prefix(super::door::REST).unwrap_or("");
    k2_rest::Request {
        method: req.method.clone(),
        path: percent_decode(rest),
        query: req.query_pairs(),
        headers: req.headers.clone(),
        body: req.body.clone(),
    }
}

/// One `/rest/v1` request from `who`.
pub async fn handle(req: &DoorRequest, ctx: &Ctx, who: &Resolved) -> DoorAnswer {
    let pool = match pool_of(ctx).await {
        Ok(p) => p,
        Err(a) => return a,
    };
    let schemas = if ctx.cfg.schemas.is_empty() { vec!["public".to_string()] } else { ctx.cfg.schemas.clone() };
    let opts = k2_rest::Options { schemas: schemas.clone(), max_rows: Some(ctx.cfg.limits.max_rows as u64).filter(|m| *m > 0) };
    let g = grammar_request(req);
    let mut cache = match cache_for(&pool, &ctx.cfg.database, &schemas, None).await {
        Ok(c) => c,
        Err(e) => return refuse(503, "db_unavailable", e, None),
    };
    let mut plan = k2_rest::plan(&g, &cache, &opts);
    if let Err(e) = &plan {
        if is_miss(e) {
            // A table or function that may have just been made: read the
            // catalogs again (at most every MISS_RELOAD) and try once more.
            if let Ok(c) = cache_for(&pool, &ctx.cfg.database, &schemas, Some(MISS_RELOAD)).await {
                cache = c;
                plan = k2_rest::plan(&g, &cache, &opts);
            }
        }
    }
    let plan = match plan {
        Ok(p) => p,
        Err(e) => {
            if e.code == "K2REST001" {
                count_unsupported(&ctx.cfg.app_id, &e.message);
            }
            return api_error(&e);
        }
    };
    let meta = meta_of(req, &plan.schema);
    let timeout = who.caller.role.timeout_ms(&ctx.cfg);
    let run = pool::run_stmts(&pool, &who.caller, &meta, timeout, plan.read_only, &plan.stmts, |outs| k2_rest::respond(&plan, outs)).await;
    match run {
        Ok(resp) => {
            let mut a = DoorAnswer::new(resp.status).body(resp.body);
            for (k, v) in resp.headers {
                a = a.header(&k, v);
            }
            a
        }
        Err(RunError::Check(e)) => api_error(&e),
        Err(RunError::Pg(p)) => {
            let status = k2_rest::pg_status(&p.code, who.caller.role != DbRole::Anon);
            DoorAnswer::json(status, &serde_json::json!({ "code": p.code, "message": p.message, "details": p.details, "hint": p.hint }))
        }
        Err(RunError::Unavailable(why)) => refuse(503, "db_unavailable", why, None),
    }
}

#[cfg(test)]
#[path = "rest_door_real_pg.rs"]
mod real_pg;
