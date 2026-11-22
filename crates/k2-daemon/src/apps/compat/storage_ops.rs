//! The storage verbs (PRD §10 "Calls in v1"), each run as the caller so
//! the App's storage policies decide. HTTP-free: the door parses, these
//! act, and the real-Postgres tests call them directly too.
//!
//! **The rule check** follows Supabase Storage's own order:
//! - **upload**: a permission test first, as the caller, in a transaction
//!   that is rolled back (the INSERT, or the upsert, that the write will
//!   be). Then the bytes stream to a staged file. Then one short
//!   transaction as `service_role` writes the row: an object already at
//!   that path without upsert is the duplicate answer; with upsert the row
//!   keeps its id and gets a new version. The bytes are placed under
//!   `(id, new version)` before that transaction commits and removed if it
//!   doesn't; the old version's bytes go after it commits.
//! - **download, info, sign**: a SELECT as the caller; no row, no bytes.
//! - **remove**: a DELETE … RETURNING as the caller (the DELETE policy, and
//!   the SELECT policy Postgres applies to RETURNING); bytes go after the
//!   commit.
//! - **move**: an UPDATE as the caller (the UPDATE policy's USING on the
//!   old row and WITH CHECK on the new one). The bytes don't move: they
//!   are under the id.
//! - **copy**: a SELECT of the source and a permission test of the
//!   destination as the caller, then the copy as `service_role`.
//! - **buckets**: as the caller, under `storage.buckets`' own policies.
//!
//! Timestamps are written as Supabase's API writes them
//! (`2026-11-22T10:00:00.000Z`).

use serde_json::{json, Value};

use super::storage_bytes::{ByteStore, Staged};
use super::storage_err::{SResult, StorageError};
use super::storage_pg::{begin_as, connect, pgerr, Caller, PgTarget};

/// Supabase's default per-file limit (50 MiB) and K2's ceiling (5 GiB).
pub const DEFAULT_FILE_LIMIT: u64 = 50 << 20;
pub const MAX_FILE_LIMIT: u64 = 5 << 30;
/// Paths one `remove` or `createSignedUrls` call may name.
pub const MAX_PATHS: usize = 1000;
/// `list`: the most entries one call returns.
pub const MAX_LIST: i64 = 1000;

/// One App's storage: its database login and its bytes.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub app_id: String,
    pub target: PgTarget,
    pub bytes: ByteStore,
}

/// `to_char` for a timestamptz column, as Supabase's JSON writes it.
fn ts(col: &str) -> String {
    format!("to_char({col} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')")
}

/// One `storage.objects` row as JSON text (alias `o`).
fn object_json() -> String {
    format!(
        "json_build_object('name', o.name, 'bucket_id', o.bucket_id, 'owner', o.owner, 'owner_id', o.owner_id, \
         'id', o.id, 'version', o.version, 'updated_at', {}, 'created_at', {}, 'last_accessed_at', {}, \
         'metadata', o.metadata, 'user_metadata', coalesce(o.user_metadata, '{}'::jsonb), \
         'archived_at', NULL, 'is_delete_marker', false, 'is_versioned', false)::text",
        ts("o.updated_at"),
        ts("o.created_at"),
        ts("o.last_accessed_at")
    )
}

/// `GET /object/info/…`: Supabase's ObjectInfo (`metadata` is the user's).
fn info_json() -> String {
    format!(
        "json_build_object('id', o.id, 'name', o.name, 'bucket_id', o.bucket_id, 'version', o.version, \
         'size', (o.metadata->>'size')::bigint, 'content_type', o.metadata->>'mimetype', \
         'cache_control', o.metadata->>'cacheControl', 'content_encoding', o.metadata->>'contentEncoding', \
         'etag', o.metadata->>'eTag', 'last_modified', coalesce(o.metadata->>'lastModified', {u}), \
         'created_at', {c}, 'metadata', coalesce(o.user_metadata, '{{}}'::jsonb), \
         'archived_at', NULL, 'is_delete_marker', false, 'is_versioned', false)::text",
        u = ts("o.updated_at"),
        c = ts("o.created_at"),
    )
}

/// One `storage.buckets` row as JSON text (alias `b`).
fn bucket_json() -> String {
    format!(
        "json_build_object('id', b.id, 'name', b.name, 'owner', coalesce(b.owner::text, ''), 'public', b.public, \
         'type', b.type, 'file_size_limit', b.file_size_limit, 'allowed_mime_types', b.allowed_mime_types, \
         'created_at', {}, 'updated_at', {}, 'versioning_status', 'DISABLED')::text",
        ts("b.created_at"),
        ts("b.updated_at")
    )
}

fn parse(text: &str) -> SResult<Value> {
    serde_json::from_str(text).map_err(|e| StorageError::internal(format!("row JSON: {e}")))
}

// ── names ────────────────────────────────────────────────────────────

/// Supabase's object key rule (S3-safe characters only).
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 1024
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b"_/!.*'() &$=@;:+,?-".contains(&b))
}

/// Supabase's bucket name rule.
pub fn valid_bucket(name: &str) -> bool {
    !name.is_empty() && name.len() < 101 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_!.*'() &$=@;:+,?-".contains(&b))
}

pub fn must_key(key: &str) -> SResult<()> {
    if valid_key(key) {
        Ok(())
    } else {
        Err(StorageError::invalid_key(key))
    }
}

pub fn must_bucket(name: &str) -> SResult<()> {
    if valid_bucket(name) {
        Ok(())
    } else {
        Err(StorageError::new(400, "InvalidBucketName", "Invalid Input", format!("Bucket name invalid: {name}")))
    }
}

/// `50MB`, `1GB`, `100kb`, `1048576` → bytes (Supabase's file size forms).
pub fn parse_size(v: &Value) -> SResult<Option<u64>> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => n.as_u64().map(Some).ok_or_else(|| StorageError::invalid_request("file_size_limit must be a whole number of bytes")),
        Value::String(s) => {
            let t = s.trim().to_ascii_uppercase();
            let (num, mult) = if let Some(n) = t.strip_suffix("GB") {
                (n, 1u64 << 30)
            } else if let Some(n) = t.strip_suffix("MB") {
                (n, 1u64 << 20)
            } else if let Some(n) = t.strip_suffix("KB") {
                (n, 1u64 << 10)
            } else if let Some(n) = t.strip_suffix('B') {
                (n, 1)
            } else {
                (t.as_str(), 1)
            };
            let n: u64 = num.trim().parse().map_err(|_| StorageError::invalid_request(format!("file size not understood: {s}")))?;
            Ok(Some(n.saturating_mul(mult)))
        }
        _ => Err(StorageError::invalid_request("file_size_limit must be a number or a size like 50MB")),
    }
}

// ── buckets ──────────────────────────────────────────────────────────

/// What the door needs to know about a bucket (read as `service_role`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketInfo {
    pub id: String,
    pub public: bool,
    pub file_size_limit: Option<i64>,
    pub allowed_mime_types: Option<Vec<String>>,
}

impl BucketInfo {
    /// The effective per-file limit.
    pub fn limit(&self) -> u64 {
        match self.file_size_limit {
            Some(n) if n > 0 => (n as u64).min(MAX_FILE_LIMIT),
            _ => DEFAULT_FILE_LIMIT,
        }
    }

    /// Supabase's mime rule: exact, or `image/*` style wildcards.
    pub fn mime_ok(&self, mime: &str) -> bool {
        let Some(list) = self.allowed_mime_types.as_ref().filter(|l| !l.is_empty()) else {
            return true;
        };
        let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        list.iter().any(|a| {
            let a = a.trim().to_ascii_lowercase();
            match a.strip_suffix("/*") {
                Some(top) => m.split('/').next() == Some(top),
                None => a == m,
            }
        })
    }
}

pub async fn bucket_info(ctx: &Ctx, bucket: &str) -> SResult<BucketInfo> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, &Caller::service("k2:bucket")).await?;
    let row = tx
        .query_opt("SELECT id, coalesce(public, false), file_size_limit, allowed_mime_types FROM storage.buckets WHERE id = $1", &[&bucket])
        .await
        .map_err(pgerr)?
        .ok_or_else(StorageError::no_bucket)?;
    Ok(BucketInfo { id: row.get(0), public: row.get(1), file_size_limit: row.get(2), allowed_mime_types: row.get(3) })
}

pub async fn bucket_list(ctx: &Ctx, caller: &Caller) -> SResult<Value> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    // Supabase's list has no ORDER BY unless one is asked for: table order.
    let rows = tx.query(&format!("SELECT {} FROM storage.buckets b", bucket_json()), &[]).await.map_err(pgerr)?;
    let out: SResult<Vec<Value>> = rows.iter().map(|r| parse(&r.get::<_, String>(0))).collect();
    Ok(Value::Array(out?))
}

pub async fn bucket_get(ctx: &Ctx, caller: &Caller, id: &str) -> SResult<Value> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let row = tx
        .query_opt(&format!("SELECT {} FROM storage.buckets b WHERE b.id = $1", bucket_json()), &[&id])
        .await
        .map_err(pgerr)?
        .ok_or_else(StorageError::no_bucket)?;
    parse(&row.get::<_, String>(0))
}

/// The settable fields of a bucket from a request body.
#[derive(Debug, Clone, Default)]
pub struct BucketSpec {
    pub public: Option<bool>,
    pub file_size_limit: Option<Option<u64>>,
    pub allowed_mime_types: Option<Option<Vec<String>>>,
}

impl BucketSpec {
    pub fn from_body(v: &Value) -> SResult<Self> {
        let mut s = BucketSpec::default();
        if let Some(p) = v.get("public") {
            s.public = Some(p.as_bool().ok_or_else(|| StorageError::invalid_request("public must be true or false"))?);
        }
        if let Some(l) = v.get("file_size_limit") {
            let n = parse_size(l)?;
            if n.is_some_and(|n| n > MAX_FILE_LIMIT) {
                return Err(StorageError::invalid_request(format!("file_size_limit is at most {MAX_FILE_LIMIT} bytes")));
            }
            s.file_size_limit = Some(n);
        }
        if let Some(m) = v.get("allowed_mime_types") {
            s.allowed_mime_types = Some(match m {
                Value::Null => None,
                Value::Array(a) => Some(
                    a.iter()
                        .map(|x| x.as_str().map(str::to_string).ok_or_else(|| StorageError::invalid_request("allowed_mime_types are strings")))
                        .collect::<SResult<Vec<_>>>()?,
                ),
                _ => return Err(StorageError::invalid_request("allowed_mime_types is a list")),
            });
        }
        Ok(s)
    }
}

/// Create a bucket as the caller. `id` defaults to `name`.
pub async fn bucket_create(ctx: &Ctx, caller: &Caller, id: Option<&str>, name: &str, spec: &BucketSpec) -> SResult<Value> {
    must_bucket(name)?;
    let id = id.unwrap_or(name);
    must_bucket(id)?;
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let limit = spec.file_size_limit.flatten().map(|n| n as i64);
    let mimes = spec.allowed_mime_types.clone().flatten();
    let owner = caller.owner_uuid();
    let owner_id = caller.sub().map(str::to_string);
    tx.execute(
        "INSERT INTO storage.buckets (id, name, owner, owner_id, public, file_size_limit, allowed_mime_types) \
         VALUES ($1, $2, $3::text::uuid, $4, $5, $6, $7)",
        &[&id, &name, &owner, &owner_id, &spec.public.unwrap_or(false), &limit, &mimes],
    )
    .await
    .map_err(|e| match StorageError::from_pg(&e) {
        d if d.code == "KeyAlreadyExists" => StorageError::bucket_exists(),
        other => other,
    })?;
    tx.commit().await.map_err(pgerr)?;
    Ok(json!({ "name": id }))
}

pub async fn bucket_update(ctx: &Ctx, caller: &Caller, id: &str, spec: &BucketSpec) -> SResult<Value> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let limit = spec.file_size_limit.map(|o| o.map(|n| n as i64));
    let mimes = spec.allowed_mime_types.clone();
    let n = tx
        .execute(
            "UPDATE storage.buckets SET \
               public = CASE WHEN $2 THEN $3 ELSE public END, \
               file_size_limit = CASE WHEN $4 THEN $5 ELSE file_size_limit END, \
               allowed_mime_types = CASE WHEN $6 THEN $7 ELSE allowed_mime_types END, \
               updated_at = now() \
             WHERE id = $1",
            &[
                &id,
                &spec.public.is_some(),
                &spec.public.unwrap_or(false),
                &limit.is_some(),
                &limit.flatten(),
                &mimes.is_some(),
                &mimes.clone().flatten(),
            ],
        )
        .await
        .map_err(pgerr)?;
    if n == 0 {
        return Err(StorageError::no_bucket());
    }
    tx.commit().await.map_err(pgerr)?;
    Ok(json!({ "message": "Successfully updated" }))
}

pub async fn bucket_delete(ctx: &Ctx, caller: &Caller, id: &str) -> SResult<Value> {
    let mut conn = connect(&ctx.target).await?;
    {
        let tx = begin_as(&mut conn, &Caller::service("k2:bucket")).await?;
        let n: i64 = tx.query_one("SELECT count(*) FROM storage.objects WHERE bucket_id = $1", &[&id]).await.map_err(pgerr)?.get(0);
        if n > 0 {
            return Err(StorageError::new(409, "InvalidRequest", "Bucket not empty", "The bucket you tried to delete is not empty"));
        }
    }
    let tx = begin_as(&mut conn, caller).await?;
    let n = tx.execute("DELETE FROM storage.buckets WHERE id = $1", &[&id]).await.map_err(pgerr)?;
    if n == 0 {
        return Err(StorageError::no_bucket());
    }
    tx.commit().await.map_err(pgerr)?;
    Ok(json!({ "message": "Successfully deleted" }))
}

/// Remove every object of a bucket the caller may remove.
pub async fn bucket_empty(ctx: &Ctx, caller: &Caller, id: &str) -> SResult<Value> {
    bucket_info(ctx, id).await?;
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let rows = tx
        .query("DELETE FROM storage.objects o WHERE o.bucket_id = $1 RETURNING o.id::text, o.version", &[&id])
        .await
        .map_err(pgerr)?;
    tx.commit().await.map_err(pgerr)?;
    for r in &rows {
        let (oid, v): (String, Option<String>) = (r.get(0), r.get(1));
        if let Some(v) = v {
            ctx.bytes.remove(&oid, &v);
        }
    }
    Ok(json!({ "message": "Successfully emptied" }))
}

// ── objects ──────────────────────────────────────────────────────────

/// A row the caller may read: where its bytes are and how to serve them.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub version: Option<String>,
    pub metadata: Value,
    pub updated_at: String,
}

impl Found {
    pub fn mimetype(&self) -> String {
        self.metadata.get("mimetype").and_then(Value::as_str).unwrap_or("application/octet-stream").to_string()
    }
    pub fn cache_control(&self) -> String {
        self.metadata.get("cacheControl").and_then(Value::as_str).unwrap_or("no-cache").to_string()
    }
    pub fn etag(&self) -> Option<String> {
        self.metadata.get("eTag").and_then(Value::as_str).map(str::to_string)
    }
}

async fn find_in(tx: &tokio_postgres::Transaction<'_>, bucket: &str, name: &str) -> SResult<Option<Found>> {
    let row = tx
        .query_opt(
            &format!(
                "SELECT o.id::text, o.version, coalesce(o.metadata, '{{}}'::jsonb)::text, {} FROM storage.objects o \
                 WHERE o.bucket_id = $1 AND o.name = $2",
                ts("o.updated_at")
            ),
            &[&bucket, &name],
        )
        .await
        .map_err(pgerr)?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(Found {
            id: r.get(0),
            version: r.get(1),
            metadata: parse(&r.get::<_, String>(2))?,
            updated_at: r.get::<_, Option<String>>(3).unwrap_or_default(),
        })),
    }
}

/// The object as the caller sees it (download, info, sign).
pub async fn find(ctx: &Ctx, caller: &Caller, bucket: &str, name: &str) -> SResult<Found> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    find_in(&tx, bucket, name).await?.ok_or_else(StorageError::not_found)
}

/// The object's full row as the caller sees it (`GET /object/info/…`).
pub async fn info(ctx: &Ctx, caller: &Caller, bucket: &str, name: &str) -> SResult<Value> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let row = tx
        .query_opt(&format!("SELECT {} FROM storage.objects o WHERE o.bucket_id = $1 AND o.name = $2", info_json()), &[&bucket, &name])
        .await
        .map_err(pgerr)?
        .ok_or_else(StorageError::not_found)?;
    parse(&row.get::<_, String>(0))
}

/// What an upload carries besides its bytes.
#[derive(Debug, Clone, Default)]
pub struct UploadMeta {
    pub upsert: bool,
    pub mimetype: String,
    pub cache_control: String,
    pub user_metadata: Option<Value>,
    /// The size when the caller declared it (a raw body's Content-Length).
    pub size: Option<u64>,
}

/// Supabase's object metadata for staged bytes.
pub fn object_metadata(staged: &Staged, meta: &UploadMeta, now_iso: &str) -> Value {
    json!({
        "eTag": staged.etag(),
        "size": staged.size,
        "mimetype": meta.mimetype,
        "cacheControl": meta.cache_control,
        "lastModified": now_iso,
        "contentLength": staged.size,
        "httpStatusCode": 200,
    })
}

/// The permission test (rolled back): may `caller` write `bucket/name`?
pub async fn can_upload(ctx: &Ctx, caller: &Caller, bucket: &str, name: &str, meta: &UploadMeta) -> SResult<()> {
    must_key(name)?;
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let md = json!({ "mimetype": meta.mimetype, "size": meta.size, "cacheControl": meta.cache_control }).to_string();
    let um = meta.user_metadata.as_ref().map(Value::to_string);
    let owner = caller.owner_uuid();
    let owner_id = caller.sub().map(str::to_string);
    let sql = if meta.upsert {
        "INSERT INTO storage.objects (bucket_id, name, owner, owner_id, metadata, user_metadata) \
         VALUES ($1, $2, $3::text::uuid, $4, $5::text::jsonb, $6::text::jsonb) \
         ON CONFLICT (bucket_id, name) DO UPDATE SET owner = excluded.owner, owner_id = excluded.owner_id, \
           metadata = excluded.metadata, user_metadata = excluded.user_metadata"
    } else {
        "INSERT INTO storage.objects (bucket_id, name, owner, owner_id, metadata, user_metadata) \
         VALUES ($1, $2, $3::text::uuid, $4, $5::text::jsonb, $6::text::jsonb)"
    };
    let r = tx.execute(sql, &[&bucket, &name, &owner, &owner_id, &md, &um]).await;
    // Never committed: dropping the transaction rolls it back.
    drop(tx);
    r.map(|_| ()).map_err(pgerr)
}

/// Write the row for staged bytes, as `service_role` (after
/// [`can_upload`], or for a signed upload URL whose token carries the
/// owner). `owner`: the caller's `sub`.
pub async fn complete_upload(
    ctx: &Ctx,
    staged: &mut Staged,
    bucket: &str,
    name: &str,
    owner: Option<&str>,
    meta: &UploadMeta,
) -> SResult<Value> {
    must_key(name)?;
    let svc = Caller::service("k2:upload");
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, &svc).await?;
    let now_iso: String = tx.query_one(&format!("SELECT {}", ts("now()")), &[]).await.map_err(pgerr)?.get(0);
    let cur = tx
        .query_opt("SELECT id::text, version FROM storage.objects WHERE bucket_id = $1 AND name = $2 FOR UPDATE", &[&bucket, &name])
        .await
        .map_err(pgerr)?;
    if cur.is_some() && !meta.upsert {
        return Err(StorageError::duplicate());
    }
    let (id, old_version): (String, Option<String>) = match &cur {
        Some(r) => (r.get(0), r.get(1)),
        None => (uuid::Uuid::new_v4().to_string(), None),
    };
    let version = uuid::Uuid::new_v4().to_string();
    let md = object_metadata(staged, meta, &now_iso).to_string();
    let um = meta.user_metadata.as_ref().map(Value::to_string);
    let owner_uuid = owner.filter(|s| uuid::Uuid::parse_str(s).is_ok()).map(str::to_string);
    let owner_id = owner.map(str::to_string);
    ctx.bytes.place(staged, &id, &version)?;
    let wrote = if cur.is_some() {
        tx.execute(
            "UPDATE storage.objects SET owner = $2::text::uuid, owner_id = $3, metadata = $4::text::jsonb, \
             user_metadata = coalesce($5::text::jsonb, '{}'::jsonb), version = $6, last_accessed_at = now() WHERE id = $1::text::uuid",
            &[&id, &owner_uuid, &owner_id, &md, &um, &version],
        )
        .await
    } else {
        tx.execute(
            "INSERT INTO storage.objects (id, bucket_id, name, owner, owner_id, metadata, user_metadata, version) \
             VALUES ($1::text::uuid, $2, $3, $4::text::uuid, $5, $6::text::jsonb, coalesce($7::text::jsonb, '{}'::jsonb), $8)",
            &[&id, &bucket, &name, &owner_uuid, &owner_id, &md, &um, &version],
        )
        .await
    };
    let committed = match wrote {
        Ok(_) => tx.commit().await.map_err(pgerr),
        Err(e) => Err(pgerr(e)),
    };
    if let Err(e) = committed {
        ctx.bytes.remove(&id, &version);
        return Err(e);
    }
    if let Some(old) = old_version.filter(|o| *o != version) {
        ctx.bytes.remove(&id, &old);
    }
    Ok(json!({ "Id": id, "Key": format!("{bucket}/{name}"), "version": version }))
}

/// Remove `names` from `bucket` as the caller: the rows it could delete,
/// as Supabase returns them. Names it can't see or delete are skipped.
pub async fn remove(ctx: &Ctx, caller: &Caller, bucket: &str, names: &[String]) -> SResult<Value> {
    if names.len() > MAX_PATHS {
        return Err(StorageError::invalid_request(format!("at most {MAX_PATHS} paths per call")));
    }
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let rows = tx
        .query(
            &format!("DELETE FROM storage.objects o WHERE o.bucket_id = $1 AND o.name = ANY($2) RETURNING {}, o.id::text, o.version", object_json()),
            &[&bucket, &names],
        )
        .await
        .map_err(pgerr)?;
    tx.commit().await.map_err(pgerr)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(parse(&r.get::<_, String>(0))?);
        let (oid, v): (String, Option<String>) = (r.get(1), r.get(2));
        if let Some(v) = v {
            ctx.bytes.remove(&oid, &v);
        }
    }
    Ok(Value::Array(out))
}

/// `DELETE /object/<bucket>/<path>`: one object, or not found.
pub async fn remove_one(ctx: &Ctx, caller: &Caller, bucket: &str, name: &str) -> SResult<Value> {
    let got = remove(ctx, caller, bucket, &[name.to_string()]).await?;
    if got.as_array().is_none_or(|a| a.is_empty()) {
        return Err(StorageError::not_found());
    }
    Ok(json!({ "message": "Successfully deleted" }))
}

/// `list`'s options (supabase-js `SearchOptions`).
#[derive(Debug, Clone)]
pub struct ListOpts {
    pub prefix: String,
    pub limit: i64,
    pub offset: i64,
    pub search: String,
    pub sort_column: String,
    pub sort_desc: bool,
}

impl ListOpts {
    pub fn from_body(v: &Value) -> SResult<Self> {
        let mut prefix = v.get("prefix").and_then(Value::as_str).unwrap_or("").to_string();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        let limit = v.get("limit").and_then(Value::as_i64).unwrap_or(100).clamp(1, MAX_LIST);
        let offset = v.get("offset").and_then(Value::as_i64).unwrap_or(0).max(0);
        let search = v.get("search").and_then(Value::as_str).unwrap_or("").to_string();
        let sort = v.get("sortBy");
        let col = sort.and_then(|s| s.get("column")).and_then(Value::as_str).unwrap_or("name");
        let sort_column = match col {
            "name" | "updated_at" | "created_at" | "last_accessed_at" => col.to_string(),
            _ => "name".to_string(),
        };
        let sort_desc = sort.and_then(|s| s.get("order")).and_then(Value::as_str).is_some_and(|o| o.eq_ignore_ascii_case("desc"));
        Ok(ListOpts { prefix, limit, offset, search, sort_column, sort_desc })
    }
}

/// One level under `prefix`, as the caller sees it: folders (`id` null)
/// and files, `name` relative to the prefix.
pub async fn list(ctx: &Ctx, caller: &Caller, bucket: &str, o: &ListOpts) -> SResult<Value> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let dir = if o.sort_desc { "DESC" } else { "ASC" };
    // The sort column is one of four fixed names (ListOpts::from_body).
    let order = if o.sort_column == "name" {
        format!("e.name COLLATE \"C\" {dir}")
    } else {
        format!("(e.id IS NOT NULL), e.{col} {dir} NULLS FIRST, e.name COLLATE \"C\" ASC", col = o.sort_column)
    };
    let sql = format!(
        "WITH o AS ( \
           SELECT substr(name, length($2) + 1) AS rest, id, updated_at, created_at, last_accessed_at, metadata, version \
           FROM storage.objects WHERE bucket_id = $1 AND starts_with(name, $2) \
             AND starts_with(lower(substr(name, length($2) + 1)), lower($3)) \
         ), e AS ( \
           SELECT DISTINCT ON (split_part(rest, '/', 1)) split_part(rest, '/', 1) AS name, NULL::uuid AS id, \
             NULL::timestamptz AS updated_at, NULL::timestamptz AS created_at, NULL::timestamptz AS last_accessed_at, \
             NULL::jsonb AS metadata, NULL::text AS version FROM o WHERE position('/' in rest) > 0 \
           UNION ALL \
           SELECT rest, id, updated_at, created_at, last_accessed_at, metadata, version FROM o WHERE position('/' in rest) = 0 \
         ) \
         SELECT json_build_object('name', e.name, 'id', e.id, 'updated_at', {u}, 'created_at', {c}, \
           'last_accessed_at', {l}, 'metadata', e.metadata, 'version', e.version, 'archived_at', NULL, \
           'is_delete_marker', CASE WHEN e.id IS NULL THEN NULL ELSE false END, \
           'is_versioned', CASE WHEN e.id IS NULL THEN NULL ELSE false END)::text \
         FROM e ORDER BY {order} LIMIT $4 OFFSET $5",
        u = ts("e.updated_at"),
        c = ts("e.created_at"),
        l = ts("e.last_accessed_at"),
    );
    let rows = tx.query(&sql, &[&bucket, &o.prefix, &o.search, &o.limit, &o.offset]).await.map_err(pgerr)?;
    let out: SResult<Vec<Value>> = rows.iter().map(|r| parse(&r.get::<_, String>(0))).collect();
    Ok(Value::Array(out?))
}

/// Move (rename) an object as the caller.
pub async fn move_object(ctx: &Ctx, caller: &Caller, bucket: &str, from: &str, to_bucket: &str, to: &str) -> SResult<Value> {
    must_key(from)?;
    must_key(to)?;
    let mut conn = connect(&ctx.target).await?;
    {
        let tx = begin_as(&mut conn, &Caller::service("k2:move")).await?;
        if find_in(&tx, to_bucket, to).await?.is_some() {
            return Err(StorageError::duplicate());
        }
    }
    let tx = begin_as(&mut conn, caller).await?;
    let row = tx
        .query_opt(
            "UPDATE storage.objects SET bucket_id = $3, name = $4, last_accessed_at = now() \
             WHERE bucket_id = $1 AND name = $2 RETURNING id::text, version",
            &[&bucket, &from, &to_bucket, &to],
        )
        .await
        .map_err(pgerr)?
        .ok_or_else(StorageError::not_found)?;
    tx.commit().await.map_err(pgerr)?;
    let (id, version): (String, Option<String>) = (row.get(0), row.get(1));
    let _ = id;
    Ok(json!({ "message": "Successfully moved", "version": version }))
}

/// Copy an object as the caller (new id, new bytes).
pub async fn copy_object(ctx: &Ctx, caller: &Caller, bucket: &str, from: &str, to_bucket: &str, to: &str, upsert: bool) -> SResult<Value> {
    must_key(from)?;
    must_key(to)?;
    let src = find(ctx, caller, bucket, from).await?;
    let meta = UploadMeta {
        upsert,
        mimetype: src.mimetype(),
        cache_control: src.cache_control(),
        user_metadata: None,
        size: src.metadata.get("size").and_then(Value::as_u64),
    };
    can_upload(ctx, caller, to_bucket, to, &meta).await?;
    let version = src.version.clone().ok_or_else(StorageError::not_found)?;
    let bytes = ctx.bytes.clone();
    let id = src.id.clone();
    let mut staged = tokio::task::spawn_blocking(move || bytes.stage_copy(&id, &version))
        .await
        .map_err(|e| StorageError::internal(format!("copy worker: {e}")))??;
    let done = complete_upload(ctx, &mut staged, to_bucket, to, caller.sub(), &meta).await?;
    Ok(json!({ "Id": done["Id"], "Key": format!("{to_bucket}/{to}"), "name": to, "bucket_id": to_bucket, "version": done["version"] }))
}

/// An object copied in from another Supabase project (`storage_import`).
#[derive(Debug, Clone)]
pub struct Imported {
    pub bucket: String,
    pub name: String,
    /// The source's object id (kept when it is a uuid no other row holds).
    pub src_id: Option<String>,
    /// The source's `metadata` (eTag, size, mimetype, cacheControl …).
    pub metadata: Value,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Write an imported object's row as `service_role`, its bytes placed
/// under `(id, new version)`. A row already at that path (the dump's
/// rows, lane B) keeps its id and owner; otherwise the source's id is
/// kept. Returns `(id, version)`.
pub async fn put_imported(ctx: &Ctx, staged: &mut Staged, o: &Imported) -> SResult<(String, String)> {
    must_key(&o.name)?;
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, &Caller::service("k2:import")).await?;
    let cur = tx
        .query_opt("SELECT id::text, version FROM storage.objects WHERE bucket_id = $1 AND name = $2 FOR UPDATE", &[&o.bucket, &o.name])
        .await
        .map_err(pgerr)?;
    let version = uuid::Uuid::new_v4().to_string();
    let md = o.metadata.to_string();
    let (id, old_version) = match &cur {
        Some(r) => (r.get::<_, String>(0), r.get::<_, Option<String>>(1)),
        None => {
            let want = o.src_id.clone().filter(|s| uuid::Uuid::parse_str(s).is_ok());
            let taken = match &want {
                Some(w) => tx.query_opt("SELECT 1 FROM storage.objects WHERE id = $1::text::uuid", &[w]).await.map_err(pgerr)?.is_some(),
                None => true,
            };
            let id = if taken { uuid::Uuid::new_v4().to_string() } else { want.unwrap_or_default() };
            (id, None)
        }
    };
    ctx.bytes.place(staged, &id, &version)?;
    let wrote = if cur.is_some() {
        tx.execute(
            "UPDATE storage.objects SET metadata = $2::text::jsonb, version = $3 WHERE id = $1::text::uuid",
            &[&id, &md, &version],
        )
        .await
    } else {
        tx.execute(
            "INSERT INTO storage.objects (id, bucket_id, name, metadata, version, created_at, updated_at, last_accessed_at) \
             VALUES ($1::text::uuid, $2, $3, $4::text::jsonb, $5, coalesce($6::text::timestamptz, now()), \
                     coalesce($7::text::timestamptz, now()), coalesce($7::text::timestamptz, now()))",
            &[&id, &o.bucket, &o.name, &md, &version, &o.created_at, &o.updated_at],
        )
        .await
    };
    let committed = match wrote {
        Ok(_) => tx.commit().await.map_err(pgerr),
        Err(e) => Err(pgerr(e)),
    };
    if let Err(e) = committed {
        ctx.bytes.remove(&id, &version);
        return Err(e);
    }
    if let Some(old) = old_version.filter(|v| *v != version) {
        ctx.bytes.remove(&id, &old);
    }
    Ok((id, version))
}

/// Make or update a bucket to match the source (import), as `service_role`.
pub async fn put_bucket(ctx: &Ctx, id: &str, name: &str, public: bool, limit: Option<i64>, mimes: Option<Vec<String>>) -> SResult<()> {
    must_bucket(id)?;
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, &Caller::service("k2:import")).await?;
    tx.execute(
        "INSERT INTO storage.buckets (id, name, public, file_size_limit, allowed_mime_types) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (id) DO UPDATE SET public = excluded.public, file_size_limit = excluded.file_size_limit, \
           allowed_mime_types = excluded.allowed_mime_types, updated_at = now()",
        &[&id, &name, &public, &limit, &mimes],
    )
    .await
    .map_err(pgerr)?;
    tx.commit().await.map_err(pgerr)?;
    Ok(())
}

/// Every object row of the App, as `service_role`: `(bucket, name, id,
/// version, size from metadata)` (the import's verify pass).
pub async fn all_objects(ctx: &Ctx) -> SResult<Vec<(String, String, String, Option<String>, Option<i64>)>> {
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, &Caller::service("k2:verify")).await?;
    let rows = tx
        .query("SELECT bucket_id, name, id::text, version, (metadata->>'size')::bigint FROM storage.objects ORDER BY bucket_id, name", &[])
        .await
        .map_err(pgerr)?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4))).collect())
}

/// Which of `names` the caller may read (for `createSignedUrls`).
pub async fn readable(ctx: &Ctx, caller: &Caller, bucket: &str, names: &[String]) -> SResult<std::collections::HashSet<String>> {
    if names.len() > MAX_PATHS {
        return Err(StorageError::invalid_request(format!("at most {MAX_PATHS} paths per call")));
    }
    let mut conn = connect(&ctx.target).await?;
    let tx = begin_as(&mut conn, caller).await?;
    let rows = tx.query("SELECT name FROM storage.objects WHERE bucket_id = $1 AND name = ANY($2)", &[&bucket, &names]).await.map_err(pgerr)?;
    Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_supabase() {
        assert!(valid_key("6f1c/2c0e/photo-1.jpg"));
        assert!(valid_key("a b/(1)!*'&$=@;:+,?.png"));
        assert!(!valid_key(""));
        assert!(!valid_key("a#b"));
        assert!(!valid_key("a%20b"));
        assert!(!valid_key("caf\u{e9}.jpg"));
        assert!(!valid_key("a\nb"));
        assert!(valid_bucket("photos"));
        assert!(!valid_bucket("a/b"));
        assert!(!valid_bucket(&"x".repeat(101)));
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size(&json!(1024)).unwrap(), Some(1024));
        assert_eq!(parse_size(&json!("50MB")).unwrap(), Some(50 << 20));
        assert_eq!(parse_size(&json!("1gb")).unwrap(), Some(1 << 30));
        assert_eq!(parse_size(&json!("100 KB")).unwrap(), Some(100 << 10));
        assert_eq!(parse_size(&Value::Null).unwrap(), None);
        assert!(parse_size(&json!("lots")).is_err());
        assert!(parse_size(&json!(-1)).is_err());
    }

    #[test]
    fn limits_and_mime_rules() {
        let b = BucketInfo { id: "p".into(), public: false, file_size_limit: None, allowed_mime_types: None };
        assert_eq!(b.limit(), DEFAULT_FILE_LIMIT);
        assert!(b.mime_ok("anything/at-all"));
        let b = BucketInfo { id: "p".into(), public: false, file_size_limit: Some(1 << 40), allowed_mime_types: Some(vec!["image/*".into(), "application/pdf".into()]) };
        assert_eq!(b.limit(), MAX_FILE_LIMIT, "capped at 5 GiB");
        assert!(b.mime_ok("image/jpeg"));
        assert!(b.mime_ok("Image/PNG; charset=binary"));
        assert!(b.mime_ok("application/pdf"));
        assert!(!b.mime_ok("text/html"));
    }

    #[test]
    fn list_options_default_like_supabase_js() {
        let o = ListOpts::from_body(&json!({ "prefix": "org1" })).unwrap();
        assert_eq!((o.prefix.as_str(), o.limit, o.offset, o.sort_column.as_str(), o.sort_desc), ("org1/", 100, 0, "name", false));
        let o = ListOpts::from_body(&json!({ "prefix": "", "limit": 5000, "sortBy": {"column": "created_at; DROP TABLE x", "order": "desc"} })).unwrap();
        assert_eq!((o.prefix.as_str(), o.limit, o.sort_column.as_str(), o.sort_desc), ("", MAX_LIST, "name", true));
    }
}
