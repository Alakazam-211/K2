//! Storage import (PRD §10 "Import", slice S5b): copy a Supabase project's
//! buckets and objects into an App, resumable, every object checked.
//!
//! `k2 app storage import <app> --from https://<ref>.supabase.co --key-secret <name>`
//!
//! - **The source key** is the project's service-role key, kept in the App
//!   workspace's vault under `<name>` with the project's host allowed for
//!   it (`k2 vault hosts <name> --add <ref>.supabase.co`). The daemon reads
//!   it for that one origin (`Use::Fetch`); it is never printed, never in a
//!   CLI argument, never on disk outside the vault.
//! - **What it copies**: every bucket (id, name, public, file size limit,
//!   mime list), then every object, listed through the source's own
//!   `/storage/v1/object/list` and fetched through
//!   `/storage/v1/object/authenticated/…` as the service role.
//! - **Rows**: an object whose row is already in the App's database (the
//!   dump's rows, `k2 db import supabase`, lane B: those carry the owner)
//!   keeps that row's id and owner and gets its bytes. Otherwise the source's
//!   id, metadata and times are kept. **Policies** on `storage.objects`
//!   come with the dump; this verb moves buckets and bytes.
//! - **Checked**: each object's size against the source's `metadata.size`
//!   and its MD5 against the source's `eTag` (when the eTag is a plain MD5;
//!   a multipart upload's `…-N` eTag is size-checked only). A mismatch is
//!   that object's failure, never a silent copy. The SHA-256 of every byte
//!   placed is recorded; `--verify` re-hashes every placed object and
//!   compares.
//! - **Resumable**: the ledger (`~/.k2/apps/<app>/storage/import/done.jsonl`)
//!   lists every object copied, with the source's eTag and update time. A
//!   second run skips those still the same at the source and with their
//!   bytes on disk, and copies what changed or is new. A large object cut
//!   off mid-transfer resumes from its partial file with `Range`.
//! - **Progress** is `import/state.json` (`k2 app storage import-status`).
//!   Transient source errors (429, 5xx, a dropped connection) are retried
//!   with backoff; an object that still fails is counted and named, and
//!   the run goes on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::storage_bytes::{hash_file, Staged};
use super::storage_err::{SResult, StorageError};
use super::storage_ops::{self as ops, Ctx, Imported};

/// Objects listed per source call (Supabase's own cap is 1000).
const PAGE: i64 = 1000;
/// Tries per source request.
const TRIES: u32 = 5;
/// The most errors kept in the state file.
const MAX_ERRORS: usize = 50;
pub const DEFAULT_CONCURRENCY: usize = 8;

/// The source: a Supabase project's URL and its service-role key.
#[derive(Clone)]
pub struct Source {
    /// `https://<ref>.supabase.co` (no trailing slash).
    pub base: String,
    pub key: String,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Source({})", self.base)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub from: String,
    /// `running`, `done`, `done_with_failures`, `failed`.
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub buckets: usize,
    pub planned: usize,
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub bytes_copied: u64,
    pub errors: Vec<String>,
    /// The disk guard stopped the run (status `stopped_disk_low`); a rerun
    /// once there is room carries on where it stopped.
    #[serde(default)]
    pub disk_low: bool,
}

/// One ledger line: an object copied and checked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Done {
    pub bucket: String,
    pub name: String,
    pub id: String,
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub md5: String,
    pub src_etag: Option<String>,
    pub src_updated_at: Option<String>,
}

/// An object listed at the source.
#[derive(Debug, Clone, PartialEq)]
pub struct SrcObject {
    pub bucket: String,
    pub name: String,
    pub id: Option<String>,
    pub metadata: Value,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl SrcObject {
    fn etag(&self) -> Option<String> {
        self.metadata.get("eTag").and_then(Value::as_str).map(str::to_string)
    }
    fn size(&self) -> Option<u64> {
        self.metadata.get("size").and_then(Value::as_u64)
    }
}

/// The MD5 an eTag states, when it is a plain one (`"<32 hex>"`).
pub fn etag_md5(etag: &str) -> Option<String> {
    let t = etag.trim().trim_matches('"').to_ascii_lowercase();
    (t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit())).then_some(t)
}

pub fn dir(app_id: &str) -> PathBuf {
    super::storage_bytes::ByteStore::for_app(app_id).root.join("import")
}

fn state_path(app_id: &str) -> PathBuf {
    dir(app_id).join("state.json")
}

fn ledger_path(app_id: &str) -> PathBuf {
    dir(app_id).join("done.jsonl")
}

pub fn read_state(app_id: &str) -> Option<State> {
    serde_json::from_str(&std::fs::read_to_string(state_path(app_id)).ok()?).ok()
}

fn write_state(app_id: &str, s: &State) {
    let p = state_path(app_id);
    let _ = std::fs::create_dir_all(dir(app_id));
    if let Ok(body) = serde_json::to_string_pretty(s) {
        let _ = k2_core::fs_atomic::atomic_write_str(&p, &(body + "\n"));
    }
}

/// The ledger, last line per object wins.
pub fn read_ledger(app_id: &str) -> HashMap<(String, String), Done> {
    let mut m = HashMap::new();
    if let Ok(raw) = std::fs::read_to_string(ledger_path(app_id)) {
        for l in raw.lines() {
            if let Ok(d) = serde_json::from_str::<Done>(l) {
                m.insert((d.bucket.clone(), d.name.clone()), d);
            }
        }
    }
    m
}

/// Is an import running for this App (in this daemon)?
fn running() -> &'static Mutex<std::collections::HashSet<String>> {
    static R: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

pub fn is_running(app_id: &str) -> bool {
    running().lock().unwrap_or_else(|e| e.into_inner()).contains(app_id)
}

// ── the source ───────────────────────────────────────────────────────

fn client() -> SResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| StorageError::internal(format!("http client: {e}")))
}

/// Percent-encode an object name for a URL path (slashes kept).
pub fn enc_path(name: &str) -> String {
    name.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn retryable(s: reqwest::StatusCode) -> bool {
    s.as_u16() == 429 || s.is_server_error()
}

async fn backoff(try_no: u32) {
    tokio::time::sleep(Duration::from_millis(250u64 << try_no.min(6))).await;
}

/// A JSON call to the source, retried.
async fn src_json(c: &reqwest::Client, src: &Source, method: reqwest::Method, path: &str, body: Option<&Value>) -> SResult<Value> {
    let url = format!("{}/storage/v1{path}", src.base);
    let mut last = String::new();
    for t in 0..TRIES {
        let mut rq = c.request(method.clone(), &url).header("apikey", &src.key).bearer_auth(&src.key);
        if let Some(b) = body {
            rq = rq.header("Content-Type", "application/json").body(b.to_string());
        }
        match rq.send().await {
            Ok(r) if r.status().is_success() => {
                let text = r.text().await.map_err(|e| StorageError::unavailable(format!("{url}: {e}")))?;
                return serde_json::from_str(&text).map_err(|e| StorageError::unavailable(format!("{url}: not JSON: {e}")));
            }
            Ok(r) if retryable(r.status()) => last = format!("{url}: HTTP {}", r.status()),
            Ok(r) => {
                let st = r.status();
                let text = r.text().await.unwrap_or_default();
                return Err(StorageError::unavailable(format!("{url}: HTTP {st}: {}", text.chars().take(300).collect::<String>())));
            }
            Err(e) => last = format!("{url}: {e}"),
        }
        backoff(t).await;
    }
    Err(StorageError::unavailable(format!("the source kept failing: {last}")))
}

/// Every bucket at the source.
pub async fn src_buckets(c: &reqwest::Client, src: &Source) -> SResult<Vec<Value>> {
    match src_json(c, src, reqwest::Method::GET, "/bucket", None).await? {
        Value::Array(a) => Ok(a),
        other => Err(StorageError::unavailable(format!("the source's bucket list is not a list: {other}"))),
    }
}

/// Every object of a bucket at the source (folders walked).
pub async fn src_objects(c: &reqwest::Client, src: &Source, bucket: &str) -> SResult<Vec<SrcObject>> {
    let mut out = Vec::new();
    let mut folders = vec![String::new()];
    while let Some(prefix) = folders.pop() {
        let mut offset = 0i64;
        loop {
            let body = json!({ "prefix": prefix, "limit": PAGE, "offset": offset, "sortBy": { "column": "name", "order": "asc" } });
            let page = src_json(c, src, reqwest::Method::POST, &format!("/object/list/{}", enc_path(bucket)), Some(&body)).await?;
            let items = page.as_array().cloned().unwrap_or_default();
            for it in &items {
                let Some(n) = it.get("name").and_then(Value::as_str) else { continue };
                let full = format!("{prefix}{n}");
                if it.get("id").is_none_or(Value::is_null) {
                    folders.push(format!("{full}/"));
                } else {
                    out.push(SrcObject {
                        bucket: bucket.to_string(),
                        name: full,
                        id: it.get("id").and_then(Value::as_str).map(str::to_string),
                        metadata: it.get("metadata").cloned().unwrap_or(Value::Null),
                        created_at: it.get("created_at").and_then(Value::as_str).map(str::to_string),
                        updated_at: it.get("updated_at").and_then(Value::as_str).map(str::to_string),
                    });
                }
            }
            if (items.len() as i64) < PAGE {
                break;
            }
            offset += PAGE;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Download one object into `part` (resuming a partial file with Range).
async fn fetch(c: &reqwest::Client, src: &Source, o: &SrcObject, part: &PathBuf) -> SResult<()> {
    let url = format!("{}/storage/v1/object/authenticated/{}/{}", src.base, enc_path(&o.bucket), enc_path(&o.name));
    let mut last = String::new();
    for t in 0..TRIES {
        let have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        let mut rq = c.get(&url).header("apikey", &src.key).bearer_auth(&src.key);
        if have > 0 {
            rq = rq.header("Range", format!("bytes={have}-"));
        }
        let mut resp = match rq.send().await {
            Ok(r) => r,
            Err(e) => {
                last = format!("{e}");
                backoff(t).await;
                continue;
            }
        };
        let st = resp.status();
        let append = match st.as_u16() {
            206 if have > 0 => true,
            200 => false,
            // The partial file is already the whole object.
            416 if have > 0 && o.size() == Some(have) => return Ok(()),
            _ if retryable(st) => {
                last = format!("HTTP {st}");
                backoff(t).await;
                continue;
            }
            _ => return Err(StorageError::unavailable(format!("{}/{}: HTTP {st}", o.bucket, o.name))),
        };
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .await
            .map_err(|e| StorageError::internal(format!("partial file: {e}")))?;
        let mut cut = false;
        let mut wrote = 0u64;
        loop {
            match resp.chunk().await {
                Ok(Some(b)) => {
                    f.write_all(&b).await.map_err(|e| StorageError::internal(format!("partial file: {e}")))?;
                    let before = wrote;
                    wrote += b.len() as u64;
                    // An object bigger than its listing said, or a disk
                    // filling from elsewhere: ask the guard again as it grows.
                    if before / SPACE_EVERY != wrote / SPACE_EVERY {
                        if let Err(fd) = k2_core::disk_guard::reserve_ok(part, 0) {
                            f.flush().await.map_err(|e| StorageError::internal(format!("partial file: {e}")))?;
                            return Err(disk_low(&k2_core::disk_guard::upload_refusal_text(&fd)));
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    last = format!("transfer cut: {e}");
                    cut = true;
                    break;
                }
            }
        }
        f.flush().await.map_err(|e| StorageError::internal(format!("partial file: {e}")))?;
        f.sync_all().await.map_err(|e| StorageError::internal(format!("partial file: {e}")))?;
        if !cut {
            return Ok(());
        }
        backoff(t).await;
    }
    Err(StorageError::unavailable(format!("{}/{}: {last}", o.bucket, o.name)))
}

/// How often a download asks the disk guard again.
const SPACE_EVERY: u64 = 16 << 20;

/// The import's disk-guard refusal: it stops the run (the partial file
/// stays, so a rerun resumes).
pub const DISK_LOW: &str = "InsufficientStorage";

fn disk_low(why: &str) -> StorageError {
    StorageError::new(507, DISK_LOW, "insufficient_storage", format!("the import stopped: {why}")).with_http(400)
}

/// The partial file for one object (named from a hash of its path).
fn part_path(app_id: &str, o: &SrcObject) -> PathBuf {
    use sha2::{Digest, Sha256};
    let h = super::storage_bytes::hex(&Sha256::digest(format!("{}\u{0}{}", o.bucket, o.name).as_bytes()));
    dir(app_id).join("partial").join(format!("{}.part", &h[..32]))
}

/// Copy, check and place one object. Returns its ledger line.
async fn copy_one(c: &reqwest::Client, src: &Source, ctx: &Ctx, o: &SrcObject) -> SResult<Done> {
    let part = part_path(&ctx.app_id, o);
    if let Some(d) = part.parent() {
        tokio::fs::create_dir_all(d).await.map_err(|e| StorageError::internal(format!("import folder: {e}")))?;
    }
    // The disk guard's line for bulk writes (as a deploy): the refuse
    // line or the reserve, whichever this object would cross first.
    if let k2_core::disk_guard::Verdict::Refuse(f) = k2_core::disk_guard::check(&[part.clone()], o.size().unwrap_or(0)) {
        return Err(disk_low(&f.sentence("import")));
    }
    fetch(c, src, o, &part).await?;
    let p2 = part.clone();
    let staged = tokio::task::spawn_blocking(move || Staged::adopt(p2))
        .await
        .map_err(|e| StorageError::internal(format!("hash worker: {e}")))?
        .map_err(|e| StorageError::internal(format!("hash: {e}")))?;
    let mut staged = staged;
    if let Some(want) = o.size() {
        if staged.size != want {
            // A short or long copy is not kept: the next run starts clean.
            return Err(StorageError::invalid_request(format!("{}/{}: size {} at the source, {} copied", o.bucket, o.name, want, staged.size)));
        }
    }
    if let Some(md5) = o.etag().as_deref().and_then(etag_md5) {
        if staged.md5_hex != md5 {
            return Err(StorageError::invalid_request(format!("{}/{}: MD5 {} at the source, {} copied", o.bucket, o.name, md5, staged.md5_hex)));
        }
    }
    let mut metadata = o.metadata.clone();
    if !metadata.is_object() {
        metadata = json!({});
    }
    metadata["size"] = json!(staged.size);
    metadata["eTag"] = json!(staged.etag());
    let imp = Imported {
        bucket: o.bucket.clone(),
        name: o.name.clone(),
        src_id: o.id.clone(),
        metadata,
        created_at: o.created_at.clone(),
        updated_at: o.updated_at.clone(),
    };
    let (size, md5, sha) = (staged.size, staged.md5_hex.clone(), staged.sha256_hex.clone());
    let (id, version) = ops::put_imported(ctx, &mut staged, &imp).await?;
    Ok(Done {
        bucket: o.bucket.clone(),
        name: o.name.clone(),
        id,
        version,
        size,
        sha256: sha,
        md5,
        src_etag: o.etag(),
        src_updated_at: o.updated_at.clone(),
    })
}

/// Options for one run.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Only these buckets (empty: all).
    pub buckets: Vec<String>,
    pub concurrency: usize,
}

/// Run the import to the end (the route spawns this). Writes the state
/// file as it goes; returns the final state.
pub async fn run(ctx: Ctx, src: Source, opts: Opts) -> SResult<State> {
    let app = ctx.app_id.clone();
    if !running().lock().unwrap_or_else(|e| e.into_inner()).insert(app.clone()) {
        return Err(StorageError::invalid_request("an import is already running for this App"));
    }
    struct Release(String);
    impl Drop for Release {
        fn drop(&mut self) {
            running().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
        }
    }
    let _release = Release(app.clone());
    let state = Arc::new(Mutex::new(State { from: src.base.clone(), status: "running".into(), started_at: k2_core::deploy::now(), ..Default::default() }));
    write_state(&app, &state.lock().unwrap_or_else(|e| e.into_inner()));
    let r = run_inner(&ctx, &src, &opts, &state).await;
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    s.finished_at = Some(k2_core::deploy::now());
    match r {
        Ok(()) if s.disk_low => s.status = "stopped_disk_low".into(),
        Ok(()) => s.status = if s.failed == 0 { "done".into() } else { "done_with_failures".into() },
        Err(e) => {
            s.status = "failed".into();
            s.errors.push(e.message.clone());
        }
    }
    write_state(&app, &s);
    Ok(s)
}

async fn run_inner(ctx: &Ctx, src: &Source, opts: &Opts, state: &Arc<Mutex<State>>) -> SResult<()> {
    let c = client()?;
    let mut buckets = src_buckets(&c, src).await?;
    if !opts.buckets.is_empty() {
        buckets.retain(|b| b.get("id").and_then(Value::as_str).is_some_and(|id| opts.buckets.iter().any(|w| w == id)));
    }
    let mut objects = Vec::new();
    for b in &buckets {
        let id = b.get("id").and_then(Value::as_str).ok_or_else(|| StorageError::unavailable("a source bucket without an id"))?;
        let name = b.get("name").and_then(Value::as_str).unwrap_or(id);
        let public = b.get("public").and_then(Value::as_bool).unwrap_or(false);
        let limit = b.get("file_size_limit").and_then(Value::as_i64);
        let mimes: Option<Vec<String>> = b.get("allowed_mime_types").and_then(|m| serde_json::from_value(m.clone()).ok());
        ops::put_bucket(ctx, id, name, public, limit, mimes).await?;
        objects.extend(src_objects(&c, src, id).await?);
    }
    let ledger = read_ledger(&ctx.app_id);
    // What the App's database names now: a ledger line counts only while
    // its row still names those bytes (a database restored to an earlier
    // moment may not).
    let rows: std::collections::HashSet<(String, String, String, String)> = ops::all_objects(ctx)
        .await?
        .into_iter()
        .filter_map(|(b, n, id, v, _)| v.map(|v| (b, n, id, v)))
        .collect();
    {
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.buckets = buckets.len();
        s.planned = objects.len();
    }
    write_state(&ctx.app_id, &state.lock().unwrap_or_else(|e| e.into_inner()));
    let ledger_file = Arc::new(tokio::sync::Mutex::new(
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(ledger_path(&ctx.app_id))
            .await
            .map_err(|e| StorageError::internal(format!("import ledger: {e}")))?,
    ));
    let n = opts.concurrency.clamp(1, 32);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Borrowed by every copy's future (each `async move` takes the reference).
    let rows = &rows;
    futures_util::stream::iter(objects)
        .for_each_concurrent(n, |o| {
            let (c, ledger_file, state, stop) = (c.clone(), ledger_file.clone(), state.clone(), stop.clone());
            let prev = ledger.get(&(o.bucket.clone(), o.name.clone())).cloned();
            async move {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let same = prev.as_ref().is_some_and(|d| {
                    d.src_etag == o.etag()
                        && d.src_updated_at == o.updated_at
                        && o.size().is_none_or(|s| s == d.size)
                        && ctx.bytes.has(&d.id, &d.version)
                        && rows.contains(&(d.bucket.clone(), d.name.clone(), d.id.clone(), d.version.clone()))
                });
                if same {
                    state.lock().unwrap_or_else(|e| e.into_inner()).skipped += 1;
                    return;
                }
                match copy_one(&c, src, ctx, &o).await {
                    Ok(d) => {
                        let line = serde_json::to_string(&d).unwrap_or_default() + "\n";
                        let mut f = ledger_file.lock().await;
                        let wrote = f.write_all(line.as_bytes()).await.and(f.flush().await);
                        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                        match wrote {
                            Ok(()) => {
                                s.copied += 1;
                                s.bytes_copied += d.size;
                            }
                            Err(e) => {
                                s.failed += 1;
                                push_err(&mut s, format!("{}/{}: ledger: {e}", o.bucket, o.name));
                            }
                        }
                        let _ = std::fs::remove_file(part_path(&ctx.app_id, &o));
                    }
                    Err(e) if e.code == DISK_LOW => {
                        // The disk guard: stop every copy, say so once.
                        if !stop.swap(true, std::sync::atomic::Ordering::SeqCst) {
                            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                            s.disk_low = true;
                            push_err(&mut s, e.message);
                        }
                    }
                    Err(e) => {
                        // A copy that failed its check starts clean next time.
                        if e.code == "InvalidRequest" {
                            let _ = std::fs::remove_file(part_path(&ctx.app_id, &o));
                        }
                        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                        s.failed += 1;
                        push_err(&mut s, e.message);
                    }
                }
                let s = state.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if (s.copied + s.skipped + s.failed) % 100 == 0 {
                    write_state(&ctx.app_id, &s);
                }
            }
        })
        .await;
    Ok(())
}

fn push_err(s: &mut State, e: String) {
    if s.errors.len() < MAX_ERRORS {
        s.errors.push(e);
    }
}

/// What `--verify` found.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Verify {
    pub checked: usize,
    pub ok: usize,
    /// Rows with no bytes, or bytes that don't hash to what was recorded.
    pub bad: Vec<String>,
    /// Objects at the source not in the App (when a source was given).
    pub missing: Vec<String>,
    /// Bytes on disk no row names (`<id>/<version>`): after the database is
    /// restored to an earlier moment, or a write that never committed.
    /// Named, never removed by verify.
    pub orphans: Vec<String>,
}

/// Re-hash every imported object's bytes against the ledger, check every
/// row has its bytes, and (with a source) that every source object is here.
pub async fn verify(ctx: &Ctx, src: Option<&Source>) -> SResult<Verify> {
    let ledger = read_ledger(&ctx.app_id);
    let rows = ops::all_objects(ctx).await?;
    let mut v = Verify::default();
    let mut here = std::collections::HashSet::new();
    let named: std::collections::HashSet<(String, String)> =
        rows.iter().filter_map(|(_, _, id, version, _)| version.clone().map(|v| (id.clone(), v))).collect();
    let bytes = ctx.bytes.clone();
    let on_disk = tokio::task::spawn_blocking(move || bytes.all_bytes()).await.map_err(|e| StorageError::internal(format!("walk worker: {e}")))?;
    v.orphans = on_disk.into_iter().filter(|k| !named.contains(k)).map(|(i, ver)| format!("{i}/{ver}")).collect();
    for (bucket, name, id, version, _size) in rows {
        here.insert((bucket.clone(), name.clone()));
        v.checked += 1;
        let Some(version) = version else {
            v.bad.push(format!("{bucket}/{name}: no bytes recorded"));
            continue;
        };
        let Some(p) = ctx.bytes.object_path(&id, &version) else {
            v.bad.push(format!("{bucket}/{name}: bad id or version"));
            continue;
        };
        let got = tokio::task::spawn_blocking(move || hash_file(&p)).await.map_err(|e| StorageError::internal(format!("hash worker: {e}")))?;
        match (got, ledger.get(&(bucket.clone(), name.clone()))) {
            (Err(_), _) => v.bad.push(format!("{bucket}/{name}: bytes missing")),
            (Ok((size, _, sha)), Some(d)) if d.id == id && d.version == version && (d.size != size || d.sha256 != sha) => {
                v.bad.push(format!("{bucket}/{name}: bytes don't match the copy's hash"))
            }
            (Ok(_), _) => v.ok += 1,
        }
    }
    if let Some(src) = src {
        let c = client()?;
        for b in src_buckets(&c, src).await? {
            let Some(id) = b.get("id").and_then(Value::as_str) else { continue };
            for o in src_objects(&c, src, id).await? {
                if !here.contains(&(o.bucket.clone(), o.name.clone())) {
                    v.missing.push(format!("{}/{}", o.bucket, o.name));
                }
            }
        }
    }
    Ok(v)
}

/// The source key from the App workspace's vault, for the source's one
/// origin. Never printed.
pub fn source_key(ws_id: &str, name: &str, base: &str) -> SResult<String> {
    use k2_vault::hosts::Origin;
    use k2_vault::{Requester, Scope, SecretStore, Selector, Use};
    let (_, origin) = Origin::parse_url(&format!("{base}/")).map_err(|e| StorageError::invalid_request(format!("--from {base}: {}", e.code)))?;
    let vault = crate::vault::get().map_err(|e| StorageError::unavailable(format!("the vault isn't open ({})", e.code)))?;
    let who = Requester::Session { workspace_id: ws_id.to_string(), session_id: None };
    let host = origin.host_str();
    let r = vault.resolve(&Selector::user(Scope::workspace(ws_id), name), Use::Fetch(&origin), &who).map_err(|e| match e.code {
        "no_allowed_hosts" | "host_not_allowed" => StorageError::access_denied(format!(
            "{name} may not be sent to {host}: a person runs k2 vault hosts {name} --add {host}"
        )),
        _ if e.status == 404 => StorageError::invalid_request(format!("no secret {name} in this workspace's vault: store the project's service-role key there")),
        c => StorageError::access_denied(format!("the vault refused {name} ({c}): {}", e.hint)),
    })?;
    r.value
        .expose_str()
        .map(|s| s.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| StorageError::invalid_request(format!("{name} is empty or not text")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etags_and_paths() {
        assert_eq!(etag_md5("\"5EB63BBBE01EEED093CB22BB8F5ACDC3\"").as_deref(), Some("5eb63bbbe01eeed093cb22bb8f5acdc3"));
        assert_eq!(etag_md5("\"5eb63bbbe01eeed093cb22bb8f5acdc3-4\""), None, "a multipart eTag is not an MD5");
        assert_eq!(etag_md5("W/\"x\""), None);
        assert_eq!(enc_path("org 1/a#b/c%.jpg"), "org%201/a%23b/c%25.jpg");
    }

    #[test]
    fn a_source_never_prints_its_key() {
        let s = Source { base: "https://ref.supabase.co".into(), key: "service-role-secret".into() };
        assert!(!format!("{s:?}").contains("secret"));
    }
}
