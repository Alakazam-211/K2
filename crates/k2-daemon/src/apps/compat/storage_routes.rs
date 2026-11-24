//! `k2 app storage …` (PRD §11.3; vs-live §3 item 10: `k2 app data` is
//! the App's `/data` folder, so storage is its own group).
//!
//! ```text
//! GET  /cli/app/storage/status         ?app                    the door, buckets, the last import
//! POST /cli/app/storage/enable         {app}                   the storage schema into the compat database
//! GET  /cli/app/storage/buckets        ?app
//! POST /cli/app/storage/bucket         {app, op: create|update|rm, id, public?, fileSizeLimit?, allowedMimeTypes?}
//! GET  /cli/app/storage/ls             ?app&bucket&prefix?
//! POST /cli/app/storage/import         {app, from, keySecret, buckets?, concurrency?}
//! GET  /cli/app/storage/import/status  ?app
//! POST /cli/app/storage/verify         {app, from?, keySecret?}
//! POST /cli/app/storage/key/rotate     {app, yes}
//! ```
//!
//! **Who:** as `k2 app data`: the owner, an Admin, or the App's workspace
//! agent with "Build and run Apps"; a Member reads nothing here and changes
//! nothing. Everything runs as `service_role` on the App's compat database
//! (the CLI is the App's operator, as the Supabase dashboard is).
//!
//! **Warnings, not walls** (R6): a bucket made public answers with a
//! warning (anyone with the URL reads every object in it); a key rotation
//! needs `yes` because every outstanding signed link stops.

use std::collections::HashMap;
use std::future::Future;

use serde_json::{json, Value as J};

use super::storage_bind;
use super::storage_bytes::ByteStore;
use super::storage_err::StorageError;
use super::storage_import;
use super::storage_ops::{self as ops, Ctx};
use super::storage_pg::Caller;
use crate::apps::routes::{app_for, json_body, need_build, no, s, Refusal, Who, R};
use k2_core::brand::CLI_NAME;

pub const ROUTE_STATUS: &str = "/cli/app/storage/status";
pub const ROUTE_ENABLE: &str = "/cli/app/storage/enable";
pub const ROUTE_BUCKETS: &str = "/cli/app/storage/buckets";
pub const ROUTE_BUCKET: &str = "/cli/app/storage/bucket";
pub const ROUTE_LS: &str = "/cli/app/storage/ls";
pub const ROUTE_IMPORT: &str = "/cli/app/storage/import";
pub const ROUTE_IMPORT_STATUS: &str = "/cli/app/storage/import/status";
pub const ROUTE_VERIFY: &str = "/cli/app/storage/verify";
pub const ROUTE_KEY_ROTATE: &str = "/cli/app/storage/key/rotate";

pub const ROUTES: &[&str] =
    &[ROUTE_STATUS, ROUTE_ENABLE, ROUTE_BUCKETS, ROUTE_BUCKET, ROUTE_LS, ROUTE_IMPORT, ROUTE_IMPORT_STATUS, ROUTE_VERIFY, ROUTE_KEY_ROTATE];
pub const POST_ROUTES: &[&str] = &[ROUTE_ENABLE, ROUTE_BUCKET, ROUTE_IMPORT, ROUTE_VERIFY, ROUTE_KEY_ROTATE];

pub const COMPAT_OFF: &str = "compat_off";
pub const STORAGE_FAILED: &str = "storage_failed";

/// A storage error as a CLI refusal (its Supabase status, K2's shape).
fn refuse(e: StorageError) -> Refusal {
    let status = if e.status >= 400 && e.status < 600 { e.status } else { 400 };
    let mut r = no(STORAGE_FAILED, status, format!("{} ({})", e.message, e.code));
    r.extra = Some(json!({ "storage": e.body() }));
    r
}

/// Run `f` to the end on a runtime of its own (the routes are blocking).
fn block<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> T {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime for the storage route");
        rt.block_on(f)
    })
    .join()
    .expect("the storage route's worker")
}

/// The App, its compat settings and a context, for a caller allowed here.
fn ctx_for(who: &Who, app: Option<&str>) -> R<(k2_core::deploy::store::AppRow, Ctx, super::door_config::DoorConfig)> {
    need_build(who)?;
    let row = app_for(who, app)?;
    let cfg = storage_bind::config(&row.app_id).ok_or_else(|| {
        no(COMPAT_OFF, 409, format!("the Supabase-compatible layer is off for {}: {CLI_NAME} app compat on {} --db <db>", row.app_id, row.app_id))
    })?;
    let target = storage_bind::authenticator_target(&cfg).map_err(refuse)?;
    let ctx = Ctx { app_id: row.app_id.clone(), target, bytes: ByteStore::for_app(&row.app_id) };
    Ok((row, ctx, cfg))
}

fn param<'a>(p: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    p.get(k).map(String::as_str).map(str::trim).filter(|v| !v.is_empty())
}

fn status(who: &Who, p: &HashMap<String, String>) -> R<J> {
    need_build(who)?;
    let row = app_for(who, param(p, "app"))?;
    let import = storage_import::read_state(&row.app_id);
    let Some(cfg) = storage_bind::config(&row.app_id) else {
        return Ok(json!({ "app": row.app_id, "on": false, "hint": format!("{CLI_NAME} app compat on {} --db <db>", row.app_id), "import": import }));
    };
    let target = storage_bind::authenticator_target(&cfg).map_err(refuse)?;
    let ctx = Ctx { app_id: row.app_id.clone(), target: target.clone(), bytes: ByteStore::for_app(&row.app_id) };
    let (schema, buckets) = block(async move {
        let schema = storage_bind::schema_present(&target).await?;
        let buckets = if schema { ops::bucket_list(&ctx, &Caller::service("k2:cli")).await? } else { json!([]) };
        Ok::<_, StorageError>((schema, buckets))
    })
    .map_err(refuse)?;
    let mut warnings = Vec::new();
    if !schema {
        warnings.push(format!("the storage schema isn't in {} yet: {CLI_NAME} app storage enable {}", cfg.database, row.app_id));
    }
    for b in buckets.as_array().into_iter().flatten() {
        if b["public"] == json!(true) {
            warnings.push(format!("bucket {} is public: anyone with an object's URL can read it", b["id"].as_str().unwrap_or("?")));
        }
    }
    if cfg.service_key_public {
        warnings.push("the server key is taken on the App's public host (service-key-public on)".into());
    }
    Ok(json!({
        "app": row.app_id,
        "on": true,
        "database": cfg.database,
        "schema": schema,
        "anon": cfg.anon,
        "door": "/storage/v1/ on the App's own host",
        "buckets": buckets,
        "import": import,
        "warnings": warnings,
    }))
}

fn enable(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    need_build(who)?;
    let row = app_for(who, s(&v, "app"))?;
    let cfg = storage_bind::config(&row.app_id)
        .ok_or_else(|| no(COMPAT_OFF, 409, format!("turn the Supabase-compatible layer on first: {CLI_NAME} app compat on {} --db <db>", row.app_id)))?;
    let db = cfg.database.clone();
    block(async move { storage_bind::install_schema(&db).await }).map_err(refuse)?;
    crate::apps::routes::audit_human(&row.app_id, "storage_enable", who, json!({ "database": cfg.database }));
    Ok(json!({ "ok": true, "app": row.app_id, "database": cfg.database, "door": "/storage/v1/" }))
}

fn buckets(who: &Who, p: &HashMap<String, String>) -> R<J> {
    let (_, ctx, _) = ctx_for(who, param(p, "app"))?;
    let b = block(async move { ops::bucket_list(&ctx, &Caller::service("k2:cli")).await }).map_err(refuse)?;
    Ok(json!({ "buckets": b }))
}

fn bucket(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let (row, ctx, _) = ctx_for(who, s(&v, "app"))?;
    let id = s(&v, "id").ok_or_else(|| no("bad_request", 400, "name the bucket (id)"))?.to_string();
    let op = s(&v, "op").unwrap_or("create").to_string();
    let mut spec_body = json!({});
    if let Some(x) = v.get("public") {
        spec_body["public"] = x.clone();
    }
    if let Some(x) = v.get("fileSizeLimit") {
        spec_body["file_size_limit"] = x.clone();
    }
    if let Some(x) = v.get("allowedMimeTypes") {
        spec_body["allowed_mime_types"] = x.clone();
    }
    let spec = ops::BucketSpec::from_body(&spec_body).map_err(refuse)?;
    let public = spec.public == Some(true);
    let svc = Caller::service("k2:cli");
    let (o, i) = (op.clone(), id.clone());
    let out = block(async move {
        match o.as_str() {
            "create" => ops::bucket_create(&ctx, &svc, Some(&i), &i, &spec).await,
            "update" => ops::bucket_update(&ctx, &svc, &i, &spec).await,
            "rm" => ops::bucket_delete(&ctx, &svc, &i).await,
            other => Err(StorageError::invalid_request(format!("op is create, update or rm, not {other}"))),
        }
    })
    .map_err(refuse)?;
    crate::apps::routes::audit_human(&row.app_id, "storage_bucket", who, json!({ "op": op, "bucket": id, "public": public }));
    let mut r = json!({ "ok": true, "op": op, "bucket": id, "result": out });
    if public {
        r["warning"] = json!(format!("bucket {id} is public: anyone with an object's URL (/storage/v1/object/public/{id}/…) can read it, with no sign-in"));
    }
    Ok(r)
}

fn ls(who: &Who, p: &HashMap<String, String>) -> R<J> {
    let (_, ctx, _) = ctx_for(who, param(p, "app"))?;
    let bucket = param(p, "bucket").ok_or_else(|| no("bad_request", 400, "name the bucket"))?.to_string();
    let body = json!({ "prefix": param(p, "prefix").unwrap_or(""), "limit": param(p, "limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(100) });
    let opts = ops::ListOpts::from_body(&body).map_err(refuse)?;
    let b2 = bucket.clone();
    let entries = block(async move { ops::list(&ctx, &Caller::service("k2:cli"), &b2, &opts).await }).map_err(refuse)?;
    Ok(json!({ "bucket": bucket, "entries": entries }))
}

fn import(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let (row, ctx, _) = ctx_for(who, s(&v, "app"))?;
    let from = s(&v, "from").ok_or_else(|| no("bad_request", 400, "--from https://<ref>.supabase.co"))?.trim_end_matches('/').to_string();
    if !from.starts_with("https://") {
        return Err(no("bad_request", 400, "--from is the project's https:// URL"));
    }
    let secret = s(&v, "keySecret").ok_or_else(|| no("bad_request", 400, "--key-secret <vault secret holding the project's service-role key>"))?;
    if storage_import::is_running(&row.app_id) {
        return Err(no("import_running", 409, format!("an import is already running for {}: {CLI_NAME} app storage import-status {}", row.app_id, row.app_id)));
    }
    let key = storage_import::source_key(&row.owner_workspace, secret, &from).map_err(refuse)?;
    let buckets: Vec<String> =
        v.get("buckets").and_then(J::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let concurrency = v.get("concurrency").and_then(J::as_u64).map(|n| n as usize).unwrap_or(storage_import::DEFAULT_CONCURRENCY);
    let src = storage_import::Source { base: from.clone(), key };
    let opts = storage_import::Opts { buckets, concurrency };
    let app = row.app_id.clone();
    // Its own thread and runtime: the copy outlives this request, and runs
    // with no client attached.
    std::thread::Builder::new()
        .name(format!("storage-import-{app}"))
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build();
            match rt {
                Ok(rt) => {
                    if let Err(e) = rt.block_on(storage_import::run(ctx, src, opts)) {
                        k2_core::log_debug!("[storage] import for {app}: {e}");
                    }
                }
                Err(e) => k2_core::log_debug!("[storage] import runtime for {app}: {e}"),
            }
        })
        .map_err(|e| no(STORAGE_FAILED, 500, format!("start the import: {e}")))?;
    crate::apps::routes::audit_human(&row.app_id, "storage_import", who, json!({ "from": from, "keySecret": secret }));
    Ok(json!({ "started": true, "app": row.app_id, "from": from, "status": format!("{CLI_NAME} app storage import-status {}", row.app_id) }))
}

fn import_status(who: &Who, p: &HashMap<String, String>) -> R<J> {
    need_build(who)?;
    let row = app_for(who, param(p, "app"))?;
    Ok(json!({ "app": row.app_id, "running": storage_import::is_running(&row.app_id), "state": storage_import::read_state(&row.app_id) }))
}

fn verify(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let (row, ctx, _) = ctx_for(who, s(&v, "app"))?;
    let src = match (s(&v, "from"), s(&v, "keySecret")) {
        (Some(f), Some(k)) => {
            let base = f.trim_end_matches('/').to_string();
            Some(storage_import::Source { key: storage_import::source_key(&row.owner_workspace, k, &base).map_err(refuse)?, base })
        }
        (None, None) => None,
        _ => return Err(no("bad_request", 400, "--from and --key-secret go together")),
    };
    let r = block(async move { storage_import::verify(&ctx, src.as_ref()).await }).map_err(refuse)?;
    // Orphan bytes are named but don't make it unclean: no row reaches them.
    let clean = r.bad.is_empty() && r.missing.is_empty();
    Ok(json!({ "app": row.app_id, "clean": clean, "verify": r }))
}

fn key_rotate(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let (row, _, _) = ctx_for(who, s(&v, "app"))?;
    if v.get("yes").and_then(J::as_bool) != Some(true) {
        return Err(no(
            "confirm_needed",
            409,
            format!("rotating {}'s signing key stops every signed URL it has handed out, at once. Run again with --yes", row.app_id),
        ));
    }
    let app = row.app_id.clone();
    super::storage_sign::rotate(&app).map_err(refuse)?;
    crate::apps::routes::audit_human(&row.app_id, "storage_key_rotate", who, json!({}));
    Ok(json!({ "ok": true, "app": row.app_id, "note": "every signed URL made before now has stopped working" }))
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        ROUTE_STATUS => status(who, params),
        ROUTE_ENABLE => enable(who, body),
        ROUTE_BUCKETS => buckets(who, params),
        ROUTE_BUCKET => bucket(who, body),
        ROUTE_LS => ls(who, params),
        ROUTE_IMPORT => import(who, body),
        ROUTE_IMPORT_STATUS => import_status(who, params),
        ROUTE_VERIFY => verify(who, body),
        ROUTE_KEY_ROTATE => key_rotate(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}
