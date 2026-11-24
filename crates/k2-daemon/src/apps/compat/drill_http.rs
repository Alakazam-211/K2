//! The acceptance drill's HTTP pass (prd-app-supabase-compat-v1 §12, step
//! 4): for every table and view, the same read through `/rest/v1` on both
//! sides, as every drill identity (the service role, anon, and the users
//! the SQL pass picked), and one line per difference.
//!
//! - **K2's side** ([`k2_side`]) runs the door's own grammar through the
//!   App's pool, exactly as a request would (role, claims, RLS).
//! - **The other side** ([`remote_side`]) is a throwaway Supabase project
//!   restored from the same dump (never the live one), or PostgREST itself:
//!   `GET <base>/<table>?select=*` with `Prefer: count=exact` and an HS256
//!   token minted from the project's JWT secret for the same claims.
//!
//! Each probe keeps the status, the exact count and a digest of the first
//! page's rows (sorted, so row order never counts); `differences` lists
//! every probe whose three don't match. Lane A's SQL pass and the CLI verb
//! (`k2 app compat drill`) put the two passes together.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::door_config::DoorConfig;
use super::pool::{self, Caller, RequestMeta};
use k2_core::brand::PRODUCT_NAME;

/// One drill identity.
#[derive(Debug, Clone)]
pub struct Identity {
    /// `service`, `anon`, `user:<email>`, `phone:<email>/sub=token`…
    pub label: String,
    pub caller: Caller,
}

/// One read's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub table: String,
    pub identity: String,
    pub status: u16,
    /// The exact count (`Content-Range`'s total).
    pub total: Option<u64>,
    /// Rows on the first page, sorted, hashed.
    pub digest: String,
    /// The error code, for a refusal.
    pub code: Option<String>,
}

/// Rows in one answer on both sides (Supabase's default).
pub const PAGE: u64 = 1000;

/// The digest of a JSON array of rows, order ignored.
pub fn digest_rows(body: &Value) -> String {
    let mut rows: Vec<String> = body.as_array().map(|a| a.iter().map(canonical).collect()).unwrap_or_default();
    rows.sort();
    let mut h = Sha256::new();
    for r in rows {
        h.update(r.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// JSON with object keys sorted (a row's key order never counts).
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            format!("{{{}}}", keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[*k]))).collect::<Vec<_>>().join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

fn total_of(range: Option<&str>) -> Option<u64> {
    range?.rsplit('/').next()?.parse().ok()
}

fn probe(table: &str, identity: &str, status: u16, range: Option<&str>, body: &Value) -> Probe {
    let ok = status < 300;
    Probe {
        table: table.to_string(),
        identity: identity.to_string(),
        status,
        total: if ok { total_of(range) } else { None },
        digest: if ok { digest_rows(body) } else { String::new() },
        code: if ok { None } else { body.get("code").and_then(Value::as_str).map(str::to_string) },
    }
}

/// K2's side: every table and view of the App's exposed schemas, read as
/// each identity through the door's grammar.
pub async fn k2_side(cfg: &DoorConfig, identities: &[Identity]) -> Result<Vec<Probe>, String> {
    let c2 = cfg.clone();
    let target = tokio::task::spawn_blocking(move || pool::target_for(&c2)).await.map_err(|e| e.to_string())??;
    let pool = pool::pool_for(&target)?;
    let schemas = if cfg.schemas.is_empty() { vec!["public".to_string()] } else { cfg.schemas.clone() };
    let cache = pool::introspect(&pool, &schemas).await?;
    let mut tables: Vec<(String, String)> = cache.tables.keys().cloned().collect();
    tables.sort();
    let opts = k2_rest::Options { schemas: schemas.clone(), max_rows: Some(PAGE) };
    let mut out = Vec::new();
    for (schema, name) in tables {
        for who in identities {
            let req = k2_rest::Request {
                method: "GET".into(),
                path: format!("/{name}"),
                query: vec![("select".into(), "*".into())],
                headers: vec![("prefer".into(), "count=exact".into()), ("accept-profile".into(), schema.clone())],
                body: Vec::new(),
            };
            let label = if schema == "public" { name.clone() } else { format!("{schema}.{name}") };
            let plan = match k2_rest::plan(&req, &cache, &opts) {
                Ok(p) => p,
                Err(e) => {
                    out.push(probe(&label, &who.label, e.status, None, &e.body()));
                    continue;
                }
            };
            let meta = RequestMeta { method: "GET".into(), path: format!("/rest/v1/{name}"), headers: Default::default(), schema: schema.clone() };
            let timeout = who.caller.role.timeout_ms(cfg);
            match pool::run_stmts(&pool, &who.caller, &meta, timeout, true, &plan.stmts, |outs| k2_rest::respond(&plan, outs)).await {
                Ok(r) => {
                    let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
                    let range = r.headers.iter().find(|(k, _)| k == "Content-Range").map(|(_, v)| v.as_str());
                    out.push(probe(&label, &who.label, r.status, range, &body));
                }
                Err(pool::RunError::Pg(p)) => {
                    let st = k2_rest::pg_status(&p.code, who.caller.role != pool::DbRole::Anon);
                    out.push(probe(&label, &who.label, st, None, &json!({ "code": p.code })));
                }
                Err(pool::RunError::Check(e)) => out.push(probe(&label, &who.label, e.status, None, &e.body())),
                Err(pool::RunError::Unavailable(why)) => return Err(why),
            }
        }
    }
    Ok(out)
}

/// The other side's token for `claims`: HS256 with the project's JWT
/// secret (the throwaway project's, read from the vault by the caller).
pub fn remote_token(claims: &Value, jwt_secret: &str) -> Result<String, String> {
    let mut c = claims.clone();
    c["exp"] = json!(chrono::Utc::now().timestamp() + 3600);
    jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &c, &jsonwebtoken::EncodingKey::from_secret(jwt_secret.as_bytes())).map_err(|e| e.to_string())
}

/// The other side: `GET <base>/<table>?select=*` as each identity.
/// `base`: `https://<project>.supabase.co/rest/v1`, or PostgREST's root.
pub async fn remote_side(base: &str, apikey: &str, jwt_secret: &str, tables: &[String], identities: &[Identity]) -> Result<Vec<Probe>, String> {
    // The daemon never honours *_PROXY from its environment.
    let http = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(60)).build().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for t in tables {
        for who in identities {
            let token = remote_token(&who.caller.claims, jwt_secret)?;
            let r = http
                .get(format!("{}/{t}?select=*", base.trim_end_matches('/')))
                .header("apikey", apikey)
                .header("Authorization", format!("Bearer {token}"))
                .header("Prefer", "count=exact")
                .send()
                .await
                .map_err(|e| format!("{t}: {e}"))?;
            let status = r.status().as_u16();
            let range = r.headers().get("content-range").and_then(|v| v.to_str().ok()).map(str::to_string);
            let body: Value = r.json().await.unwrap_or(Value::Null);
            out.push(probe(t, &who.label, status, range.as_deref(), &body));
        }
    }
    Ok(out)
}

/// One line per probe that differs (or is on one side only).
pub fn differences(ours: &[Probe], theirs: &[Probe]) -> Vec<String> {
    let mut out = Vec::new();
    for a in ours {
        match theirs.iter().find(|b| b.table == a.table && b.identity == a.identity) {
            None => out.push(format!("{} as {}: only on {PRODUCT_NAME}", a.table, a.identity)),
            Some(b) if b != a => out.push(format!(
                "{} as {}: {PRODUCT_NAME} {} total={:?} code={:?} rows={} | other {} total={:?} code={:?} rows={}",
                a.table,
                a.identity,
                a.status,
                a.total,
                a.code,
                &a.digest.get(..12).unwrap_or(""),
                b.status,
                b.total,
                b.code,
                &b.digest.get(..12).unwrap_or("")
            )),
            Some(_) => {}
        }
    }
    for b in theirs {
        if !ours.iter().any(|a| a.table == b.table && a.identity == b.identity) {
            out.push(format!("{} as {}: only on the other side", b.table, b.identity));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_ignore_row_and_key_order() {
        let a = json!([{"id": 1, "x": "a"}, {"id": 2, "x": "b"}]);
        let b = json!([{"x": "b", "id": 2}, {"x": "a", "id": 1}]);
        assert_eq!(digest_rows(&a), digest_rows(&b));
        assert_ne!(digest_rows(&a), digest_rows(&json!([{"id": 1, "x": "a"}])));
        assert_eq!(total_of(Some("0-1/2")), Some(2));
        assert_eq!(total_of(Some("*/*")), None);
    }

    #[test]
    fn differences_name_each_probe_that_disagrees() {
        let p = |t: &str, who: &str, st: u16, total: Option<u64>| Probe { table: t.into(), identity: who.into(), status: st, total, digest: "d".into(), code: None };
        let ours = vec![p("notes", "anon", 200, Some(0)), p("notes", "user:a", 200, Some(2)), p("orgs", "anon", 200, Some(3))];
        let theirs = vec![p("notes", "anon", 200, Some(0)), p("notes", "user:a", 200, Some(3)), p("tags", "anon", 200, Some(1))];
        let d = differences(&ours, &theirs);
        assert_eq!(d.len(), 3, "{d:?}");
        assert!(d.iter().any(|l| l.starts_with("notes as user:a: K2 200 total=Some(2)")));
        assert!(d.iter().any(|l| l == "orgs as anon: only on K2"));
        assert!(d.iter().any(|l| l == "tags as anon: only on the other side"));
        let tok = remote_token(&json!({"role": "authenticated", "sub": "x"}), "secret-secret-secret-secret-1234").unwrap();
        assert_eq!(tok.split('.').count(), 3);
    }
}

#[cfg(test)]
#[path = "drill_http_real_pg.rs"]
mod real_pg;
