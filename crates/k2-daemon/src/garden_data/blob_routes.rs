//! Garden blob routes (prd-garden-board-needs-v1 §3.4–§3.5, GB13, GB15).
//!
//! Bytes can't ride `/cli/zen/call` (its body is capped at 64 KiB), so the
//! byte verbs are frame rows and the app's renderer moves the bytes over
//! these routes, which run the **same** gates as `/cli/zen/call`:
//!
//! - **Widget door** (`/cli/zen/blob/*`): owner token only, the Origin gate,
//!   placement → widget → live hash, the cap with the signer gate, pause,
//!   the maker's rights, budgets. Never on the cell socket.
//! - **Agent door** (`/cli/zen/data/blob/*`): an agent's grants, as every
//!   other `/cli/zen/data/*` route (`k2 store blob`).
//!
//! Uploads are chunked: the first chunk (offset 0) opens an upload, each
//! next one must start exactly where the last ended, and the `last` chunk
//! commits it. An upload idle 60 s is dropped. Reads are ranged (`offset`,
//! `len` ≤ 8 MiB) and answer base64.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::garden_data::blobs::{
    self, BlobRow, BLOB_CHUNK_BYTES, MAX_BLOB_BYTES, MAX_THUMB_DECODES, MIN_CHUNK_BYTES, THUMB_CACHE_BYTES, THUMB_READ_COST,
    UPLOAD_IDLE_SECS, UPLOAD_MAX_LIFE_SECS,
};
use k2_core::garden_data::{Access, Actor, DataError, Space, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT};
use k2_core::overlay::thumbs;

use super::agent_routes::{access, data_space, int_param, param, parse, ByNames};
use super::audit::{self, Counted};
use super::budget::{self, BudgetKey, Spend};
use super::call::{self, CallBody, Ingress, Placed};
use super::{refused, store};
use crate::zen_data_routes::DataWho;

pub const WIDGET_PUT: &str = "/cli/zen/blob/put";
pub const WIDGET_GET: &str = "/cli/zen/blob/get";
/// 0.46.6 (prd-garden-board-needs-v1 GP3, GP4): files the person picked in
/// K2's own dialog or dropped on the widget, by path, into its data.
pub const WIDGET_INGEST: &str = "/cli/zen/blob/ingest";
pub const AGENT_PUT: &str = "/cli/zen/data/blob/put";
pub const AGENT_GET: &str = "/cli/zen/data/blob/get";
pub const AGENT_LIST: &str = "/cli/zen/data/blob/list";

/// Open uploads, in all and per owner (each holds at most 16 MiB).
pub const MAX_OPEN_UPLOADS: usize = 8;
pub const MAX_OPEN_UPLOADS_PER_OWNER: usize = 2;

pub fn is_widget_route(p: &str) -> bool {
    matches!(p, WIDGET_PUT | WIDGET_GET | WIDGET_INGEST)
}

/// One pick or drop (GP3): at most this many files, each at most a blob
/// (16 MiB), and this many bytes in all.
pub const MAX_INGEST_FILES: usize = 20;
pub const MAX_INGEST_BYTES: u64 = 64 * 1024 * 1024;
/// `accept` entries.
pub const MAX_ACCEPT: usize = 20;

/// Does a picked file match one `accept` entry (`.ext`, `type/*` or
/// `type/subtype`)? The mime is K2's sniff, the name the file's own.
pub(crate) fn accepts(accept: &[String], name: &str, mime: &str) -> bool {
    if accept.is_empty() {
        return true;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    accept.iter().any(|a| {
        let a = a.trim().to_ascii_lowercase();
        if let Some(want) = a.strip_prefix('.') {
            ext.as_deref() == Some(want)
        } else if let Some(top) = a.strip_suffix("/*") {
            mime.split('/').next() == Some(top)
        } else {
            mime == a
        }
    })
}

/// A valid `accept` entry: `.ext` (letters and digits) or a mime pattern.
pub(crate) fn valid_accept(a: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    match a.strip_prefix('.') {
        Some(ext) => !ext.is_empty() && ext.len() <= 16 && ext.chars().all(|c| c.is_ascii_alphanumeric()),
        None => match a.split_once('/') {
            Some((t, s)) => ok(t) && (s == "*" || ok(s)),
            None => false,
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestBody {
    garden: String,
    placement: String,
    hash: String,
    paths: Vec<String>,
    #[serde(default)]
    accept: Vec<String>,
}

/// GP3/GP4: read each path K2's own dialog (or a drop) gave the app, check
/// it, and keep it in the widget's data with its basename as its name.
/// Answers `{files: [{ref, name, size, mime}]}` (never a path). Paths come
/// from K2's renderer only: this is the widget door (owner token, Origin
/// gate), and the owner token can read local files already.
fn ingest(who_label: &str, w: &Placed, space: &Space, b: &IngestBody) -> Result<J, DataError> {
    if b.paths.is_empty() {
        return Ok(json!({ "files": [] }));
    }
    if b.paths.len() > MAX_INGEST_FILES {
        return Err(DataError::BadRequest(format!("at most {MAX_INGEST_FILES} files at a time")));
    }
    if b.accept.len() > MAX_ACCEPT || b.accept.iter().any(|a| !valid_accept(a.trim())) {
        return Err(DataError::BadRequest("accept is up to 20 of .ext, type/* or type/subtype".into()));
    }
    // Check every file before keeping any: all or nothing.
    let mut total = 0u64;
    let mut picked: Vec<(String, Vec<u8>)> = Vec::new();
    for p in &b.paths {
        let path = std::path::Path::new(p);
        if !path.is_absolute() {
            return Err(DataError::BadRequest("a picked file is an absolute path".into()));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let meta = std::fs::metadata(path).map_err(|_| DataError::BadRequest(format!("can't read '{name}'")))?;
        if !meta.is_file() {
            return Err(DataError::BadRequest(format!("'{name}' isn't a file (folders aren't taken)")));
        }
        if meta.len() > MAX_BLOB_BYTES as u64 {
            return Err(DataError::TooLarge(format!("'{name}' is over {} MiB", MAX_BLOB_BYTES / (1024 * 1024))));
        }
        total += meta.len();
        if total > MAX_INGEST_BYTES {
            return Err(DataError::TooLarge(format!("one pick is at most {} MiB in all", MAX_INGEST_BYTES / (1024 * 1024))));
        }
        let bytes = std::fs::read(path).map_err(|_| DataError::BadRequest(format!("can't read '{name}'")))?;
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(DataError::TooLarge(format!("'{name}' is over {} MiB", MAX_BLOB_BYTES / (1024 * 1024))));
        }
        if !accepts(&b.accept, &name, &blobs::sniff(&bytes)) {
            return Err(DataError::BadRequest(format!("'{name}' isn't one of the kinds this widget takes ({})", b.accept.join(" "))));
        }
        picked.push((name, bytes));
    }
    let key = placement_key(w);
    let mut files = Vec::with_capacity(picked.len());
    for (name, bytes) in picked {
        budget::spend(&key, Spend::UploadStart)?;
        budget::spend(&key, Spend::UploadBytes { bytes: bytes.len() })?;
        let info = store().blob_put(space, &bytes, Some(&name), &w.actor())?;
        audit::count(space.as_str(), who_label, None, Counted::Write { ops: 1, bytes: bytes.len() });
        // The name the widget sees is the file's own, cleaned like a stored one.
        let shown = blobs::clean_name(&name).unwrap_or_else(|| "file".to_string());
        files.push(json!({ "ref": info.reference, "name": shown, "size": info.size, "mime": info.mime }));
    }
    Ok(json!({ "files": files }))
}

pub fn is_agent_route(p: &str) -> bool {
    matches!(p, AGENT_PUT | AGENT_GET | AGENT_LIST)
}

// ── shared views ──────────────────────────────────────────────────────

/// `BlobDetail`: one blob as `blobs.info` / `blobs.list` show it.
pub(crate) fn info_json(row: &BlobRow, names: &mut ByNames) -> J {
    json!({
        "ref": row.reference(),
        "size": row.size,
        "mime": row.mime,
        "name": row.name,
        "refs": row.refs,
        "pending": row.pending,
        "addedAt": row.added_at,
        "addedBy": names.wire(&row.added_by),
    })
}

/// A blob the caller may read (GB13 rights): the whole space for `Full`;
/// else a blob a key it can read holds, or one still pending that it
/// uploaded itself.
pub(crate) fn readable_row(space: &Space, acc: &Access, actor: &Actor, hash: &str) -> Result<BlobRow, DataError> {
    let s = store();
    let row = s.blob_row(space, hash)?.ok_or_else(|| DataError::BlobMissing { refs: vec![blobs::ref_of(hash)] })?;
    match acc {
        Access::Full => Ok(row),
        Access::None => Err(DataError::NotGranted { key: None }),
        Access::Grants(_) => {
            if row.pending && row.added_by == actor.stored() {
                return Ok(row);
            }
            let holders = s.blob_holders(space, hash, 10_000)?;
            if holders.iter().any(|k| acc.can_read(k)) {
                Ok(row)
            } else {
                // Security review #6: never name a key outside the grant.
                Err(DataError::NotGranted { key: None })
            }
        }
    }
}

/// One `blobs.list` page (`BlobList`), filtered to what `acc` can read.
pub(crate) fn list_page(space: &Space, acc: &Access, after: Option<&str>, limit: usize, names: &mut ByNames) -> Result<J, DataError> {
    let after = after.map(blobs::parse_ref).transpose()?;
    let filter = acc.read_prefixes();
    let page = store().blob_list(space, after.as_deref(), limit, filter.as_deref())?;
    let items: Vec<J> = page.items.iter().map(|r| info_json(r, names)).collect();
    Ok(json!({
        "items": items,
        "next": page.next.map(|h| blobs::ref_of(&h)),
        "bytes": page.bytes,
        "quota": page.quota,
    }))
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// One cached thumbnail: its mime and bytes.
type Thumb = Arc<(&'static str, Vec<u8>)>;

/// Thumbnails by (hash, px), oldest out first past [`THUMB_CACHE_BYTES`]
/// (security review #3: a blob never changes, so neither does its thumb).
#[derive(Default)]
struct ThumbCache {
    map: HashMap<(String, u32), Thumb>,
    order: VecDeque<(String, u32)>,
    bytes: usize,
}

fn thumb_cache() -> &'static Mutex<ThumbCache> {
    static C: OnceLock<Mutex<ThumbCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(ThumbCache::default()))
}

/// Thumbnails being made now, daemon-wide.
static DECODING: AtomicUsize = AtomicUsize::new(0);

/// Release a decode slot when the guard goes.
struct DecodeSlot;

impl DecodeSlot {
    fn take() -> Result<DecodeSlot, DataError> {
        let prev = DECODING.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_THUMB_DECODES {
            DECODING.fetch_sub(1, Ordering::SeqCst);
            return Err(DataError::Busy);
        }
        Ok(DecodeSlot)
    }
}

impl Drop for DecodeSlot {
    fn drop(&mut self) {
        DECODING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The thumbnail of a blob (cached), made at most [`MAX_THUMB_DECODES`] at
/// a time (more answer `busy`). Review #2: a HEIC goes to `sips` /
/// `heif-convert` only when its own bytes say HEIF.
fn thumbnail(row: &BlobRow, px: u32) -> Result<Thumb, DataError> {
    let key = (row.hash.clone(), px);
    if let Some(t) = thumb_cache().lock().unwrap_or_else(|e| e.into_inner()).map.get(&key) {
        return Ok(t.clone());
    }
    let dir = store().blob_dir();
    let (head, _) = blobs::read_range(&dir, &row.hash, 0, 64)?;
    if blobs::sniff(&head) != row.mime && row.mime != "image/svg+xml" {
        return Err(refused("no_thumbnail", 422, "no thumbnail: the file's bytes don't match its type"));
    }
    if matches!(row.mime.as_str(), "image/heic" | "image/heif") && !blobs::is_heif(&head) {
        return Err(refused("no_thumbnail", 422, "no thumbnail: not a HEIF file"));
    }
    let _slot = DecodeSlot::take()?;
    let path = blobs::blob_path(&dir, &row.hash)?;
    let t = thumbs::make(&path, &row.mime, px).map_err(|e| refused(e.code(), 422, format!("no thumbnail: {}", e.hint())))?;
    let t: Thumb = Arc::new((t.mime, t.bytes));
    let mut c = thumb_cache().lock().unwrap_or_else(|e| e.into_inner());
    if t.1.len() <= THUMB_CACHE_BYTES && !c.map.contains_key(&key) {
        c.bytes += t.1.len();
        c.map.insert(key.clone(), t.clone());
        c.order.push_back(key);
        while c.bytes > THUMB_CACHE_BYTES {
            let Some(old) = c.order.pop_front() else { break };
            if let Some(gone) = c.map.remove(&old) {
                c.bytes -= gone.1.len();
            }
        }
    }
    Ok(t)
}

/// A ranged read, or a `px` thumbnail of an image (GB14: an SVG thumbnail
/// is sanitized; a non-image with `px` is `bad_request`).
fn read_json(space: &Space, row: &BlobRow, offset: u64, len: usize, px: Option<&str>) -> Result<J, DataError> {
    if let Some(px) = px {
        let px = thumbs::parse_px(Some(px)).map_err(DataError::BadRequest)?;
        if !thumbs::is_image_mime(&row.mime) {
            return Err(DataError::BadRequest(format!("px makes image thumbnails; this blob is {}", row.mime)));
        }
        let t = thumbnail(row, px)?;
        return Ok(json!({
            "ref": row.reference(),
            "size": t.1.len(),
            "mime": t.0,
            "name": row.name,
            "offset": 0,
            "len": t.1.len(),
            "eof": true,
            "px": px,
            "base64": b64().encode(&t.1),
        }));
    }
    if len == 0 || len > BLOB_CHUNK_BYTES {
        return Err(DataError::BadRequest(format!("len is 1..={BLOB_CHUNK_BYTES}")));
    }
    let (_, bytes, eof) = store().blob_read(space, &row.hash, offset, len)?;
    Ok(json!({
        "ref": row.reference(),
        "size": row.size,
        "mime": row.mime,
        "name": row.name,
        "offset": offset,
        "len": bytes.len(),
        "eof": eof,
        "base64": b64().encode(&bytes),
    }))
}

/// A thumbnail read costs [`THUMB_READ_COST`] reads (review #3).
fn read_cost(params: &HashMap<String, String>) -> Spend {
    if param(params, "px").is_some() {
        Spend::Reads(THUMB_READ_COST)
    } else {
        Spend::Read
    }
}

fn range_params(params: &HashMap<String, String>) -> Result<(u64, usize), DataError> {
    let offset = int_param(params, "offset")?.unwrap_or(0);
    let len = int_param(params, "len")?.unwrap_or(BLOB_CHUNK_BYTES as i64);
    if offset < 0 || len < 1 {
        return Err(DataError::BadRequest("offset is 0 or more; len is 1 or more".into()));
    }
    Ok((offset as u64, len as usize))
}

// ── chunked uploads ───────────────────────────────────────────────────

struct Upload {
    owner: String,
    space: Space,
    bytes: Vec<u8>,
    name: Option<String>,
    touched: Instant,
    /// Review #8: an upload lives at most UPLOAD_MAX_LIFE_SECS from here.
    started: Instant,
}

fn uploads() -> &'static Mutex<HashMap<String, Upload>> {
    static U: OnceLock<Mutex<HashMap<String, Upload>>> = OnceLock::new();
    U.get_or_init(|| Mutex::new(HashMap::new()))
}

fn upload_id() -> String {
    let mut b = [0u8; 12];
    if let Err(e) = getrandom::getrandom(&mut b) {
        k2_core::log_debug!("[garden-data] upload id RNG: {e}");
    }
    format!("up-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// Drop uploads idle longer than [`UPLOAD_IDLE_SECS`], or open longer than
/// [`UPLOAD_MAX_LIFE_SECS`] however often touched (also run each minute).
pub fn prune_uploads() {
    let mut g = uploads().lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|_, u| {
        u.touched.elapsed().as_secs() < UPLOAD_IDLE_SECS as u64 && u.started.elapsed().as_secs() < UPLOAD_MAX_LIFE_SECS
    });
}

/// One upload chunk, from either door. A declared mime on the wire is
/// accepted and ignored: K2 sniffs the bytes (security review #2).
pub(crate) struct Chunk {
    pub upload: Option<String>,
    pub offset: u64,
    pub base64: String,
    pub last: bool,
    pub name: Option<String>,
}

/// Take one chunk for `owner` (a stable label: the placement, or the
/// agent) into `space`. Answers `{upload, received}` while open, and
/// `BlobInfo` when the last chunk commits.
pub(crate) fn take_chunk(owner: &str, budget_key: Option<&BudgetKey>, space: &Space, actor: &Actor, c: Chunk) -> Result<J, DataError> {
    let bytes = b64()
        .decode(c.base64.as_bytes())
        .map_err(|_| DataError::BadRequest("base64 is standard base64 of the chunk's bytes".into()))?;
    if bytes.len() > BLOB_CHUNK_BYTES {
        return Err(DataError::TooLarge(format!("a chunk is at most {} MiB", BLOB_CHUNK_BYTES / (1024 * 1024))));
    }
    if !c.last && bytes.len() < MIN_CHUNK_BYTES {
        return Err(DataError::BadRequest(format!(
            "every chunk but the last carries at least {} MiB",
            MIN_CHUNK_BYTES / (1024 * 1024)
        )));
    }
    prune_uploads();
    let spend = |what: Spend| -> Result<(), DataError> {
        match budget_key {
            Some(k) => budget::spend(k, what),
            None => Ok(()),
        }
    };
    let commit = |all: Vec<u8>, name: Option<String>| -> Result<J, DataError> {
        let info = store().blob_put(space, &all, name.as_deref(), actor)?;
        audit::count(space.as_str(), owner, None, Counted::Write { ops: 1, bytes: all.len() });
        serde_json::to_value(&info).map_err(|e| DataError::Unavailable(format!("blob info: {e}")))
    };
    match c.upload {
        None => {
            if c.offset != 0 {
                return Err(DataError::BadRequest("a new upload starts at offset 0".into()));
            }
            spend(Spend::UploadStart)?;
            spend(Spend::UploadBytes { bytes: bytes.len() })?;
            if c.last {
                return commit(bytes, c.name);
            }
            let mut g = uploads().lock().unwrap_or_else(|e| e.into_inner());
            let mine = g.values().filter(|u| u.owner == owner).count();
            if mine >= MAX_OPEN_UPLOADS_PER_OWNER || g.len() >= MAX_OPEN_UPLOADS {
                return Err(DataError::Busy);
            }
            let id = upload_id();
            let received = bytes.len();
            g.insert(
                id.clone(),
                Upload {
                    owner: owner.to_string(),
                    space: space.clone(),
                    bytes,
                    name: c.name,
                    touched: Instant::now(),
                    started: Instant::now(),
                },
            );
            Ok(json!({ "upload": id, "received": received }))
        }
        Some(id) => {
            let mut g = uploads().lock().unwrap_or_else(|e| e.into_inner());
            let u = match g.get_mut(&id) {
                Some(u) if u.owner == owner && &u.space == space => u,
                _ => return Err(DataError::BadRequest("no such upload for you (it may have been idle 60 s); start again".into())),
            };
            if c.offset != u.bytes.len() as u64 {
                return Err(DataError::BadRequest(format!(
                    "offset {} doesn't follow the {} bytes received; nothing was added",
                    c.offset,
                    u.bytes.len()
                )));
            }
            if u.bytes.len() + bytes.len() > MAX_BLOB_BYTES {
                g.remove(&id);
                return Err(DataError::TooLarge(format!("a file is at most {} MiB", MAX_BLOB_BYTES / (1024 * 1024))));
            }
            spend(Spend::UploadBytes { bytes: bytes.len() })?;
            u.bytes.extend_from_slice(&bytes);
            u.touched = Instant::now();
            if c.name.is_some() {
                u.name = c.name;
            }
            if c.last {
                let u = g.remove(&id).ok_or_else(|| DataError::Unavailable("an upload vanished".into()))?;
                drop(g);
                return commit(u.bytes, u.name);
            }
            Ok(json!({ "upload": id, "received": u.bytes.len() }))
        }
    }
}

// ── widget door ───────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetPutBody {
    garden: String,
    placement: String,
    hash: String,
    #[serde(default)]
    upload: Option<String>,
    #[serde(default)]
    offset: u64,
    base64: String,
    #[serde(default)]
    last: bool,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Placement → widget → live hash, as `/cli/zen/call` resolves it.
pub(crate) fn placed(garden: &str, placement: &str, hash: &str, verb: &str) -> Result<Placed, DataError> {
    call::resolve(&CallBody {
        garden: garden.to_string(),
        placement: placement.to_string(),
        hash: hash.to_string(),
        verb: verb.to_string(),
        args: Vec::new(),
        confirm_token: None,
    })
}

pub(crate) fn not_paused(w: &Placed) -> Result<(), DataError> {
    if crate::zen_routes::placement_pause(&w.garden, &w.placement).is_some() {
        return Err(refused("paused", 423, "this widget is paused: too many writes. The person can Resume it on the widget"));
    }
    Ok(())
}

pub(crate) fn placement_key(w: &Placed) -> BudgetKey {
    BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() }
}

/// The widget door: the owner token and the Origin gate first, then the
/// placement and the cap. `ingress` is the Origin gate's answer.
pub fn widget(path: &str, who: &DataWho, headers: &str, body: &[u8], params: &HashMap<String, String>) -> Result<J, DataError> {
    if !who.is_owner() {
        return Err(refused(
            "owner_only",
            403,
            "only the K2 app on this computer calls widget data; agents use `k2 store blob`",
        ));
    }
    let ingress = call::origin_gate(headers).inspect_err(|_| {
        audit::line("zen.data.refused", "owner", "code=origin_refused");
    })?;
    if ingress == Ingress::LocalProcess {
        audit::line("zen.data.call", "local-process", path);
    }
    match path {
        WIDGET_PUT => {
            let b: WidgetPutBody = parse(body)?;
            let w = placed(&b.garden, &b.placement, &b.hash, "blobs.put")?;
            call::check_cap(&w, "blobs.put", "store:write")?;
            not_paused(&w)?;
            let (space, acc, _made) = call::data_access(&w, "blobs.put", None)?;
            if !acc.can_write_somewhere() {
                return Err(DataError::NotGranted { key: None });
            }
            let _ = b.mime;
            let chunk = Chunk { upload: b.upload, offset: b.offset, base64: b.base64, last: b.last, name: b.name };
            take_chunk(&w.label(), Some(&placement_key(&w)), &space, &w.actor(), chunk)
        }
        WIDGET_GET => {
            let need = |k: &str| param(params, k).ok_or_else(|| DataError::BadRequest(format!("{k} is required")));
            let w = placed(need("garden")?, need("placement")?, need("hash")?, "blobs.get")?;
            call::check_cap(&w, "blobs.get", "store:read")?;
            budget::spend(&placement_key(&w), read_cost(params))?;
            let hash = blobs::parse_ref(need("ref")?)?;
            let (space, acc, made) = call::data_access(&w, "blobs.get", param(params, "from"))?;
            let row = readable_row(&space, &acc, &w.actor(), &hash)?;
            let (offset, len) = range_params(params)?;
            let out = read_json(&space, &row, offset, len, param(params, "px"))?;
            audit::count(space.as_str(), &w.label(), Some(&made), Counted::Read);
            Ok(out)
        }
        WIDGET_INGEST => {
            let b: IngestBody = parse(body)?;
            let w = placed(&b.garden, &b.placement, &b.hash, "uploads.pick")?;
            call::check_cap(&w, "uploads.pick", "uploads:pick")?;
            // GP1: what's picked is kept as the widget's own data.
            if !w.signed {
                return Err(call::needs_editor(&w));
            }
            not_paused(&w)?;
            let (space, acc, _made) = call::data_access(&w, "uploads.pick", None)?;
            if !acc.can_write_somewhere() {
                return Err(DataError::NotGranted { key: None });
            }
            audit::line("zen.data.ingest", &w.label(), &format!("space={space} files={}", b.paths.len()));
            ingest(&w.label(), &w, &space, &b)
        }
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

// ── agent door ────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPutBody {
    widget: String,
    #[serde(default)]
    upload: Option<String>,
    #[serde(default)]
    offset: u64,
    base64: String,
    #[serde(default)]
    last: bool,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// `/cli/zen/data/blob/*` (GB15). The gates in `zen_data_routes::handle`
/// already ran.
pub fn agent(path: &str, who: &DataWho, body: &[u8], params: &HashMap<String, String>) -> Result<J, DataError> {
    let budget_key = |space: &Space| {
        who.project_id().map(|p| BudgetKey::Agent { project_id: p.to_string(), space: space.as_str().to_string() })
    };
    let actor = who.actor().ok_or(DataError::NotGranted { key: None })?;
    match path {
        AGENT_PUT => {
            let b: AgentPutBody = parse(body)?;
            let space = data_space(&b.widget)?;
            let acc = access(who, &space)?;
            if !acc.can_write_somewhere() {
                audit::line("zen.data.refused", &who.audit_label(), &format!("space={space} code=not_granted"));
                return Err(DataError::NotGranted { key: None });
            }
            let _ = b.mime;
            let chunk = Chunk { upload: b.upload, offset: b.offset, base64: b.base64, last: b.last, name: b.name };
            take_chunk(&who.audit_label(), budget_key(&space).as_ref(), &space, &actor, chunk)
        }
        AGENT_GET => {
            let space = data_space(param(params, "widget").unwrap_or(""))?;
            let hash = blobs::parse_ref(param(params, "ref").ok_or_else(|| DataError::BadRequest("ref is required".into()))?)?;
            let acc = access(who, &space)?;
            if let Some(k) = budget_key(&space) {
                budget::spend(&k, read_cost(params))?;
            }
            let row = readable_row(&space, &acc, &actor, &hash)?;
            let (offset, len) = range_params(params)?;
            let out = read_json(&space, &row, offset, len, param(params, "px"))?;
            audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
            Ok(out)
        }
        AGENT_LIST => {
            let space = data_space(param(params, "widget").unwrap_or(""))?;
            let limit = match int_param(params, "limit")? {
                None => DEFAULT_LIST_LIMIT,
                Some(n) if n >= 1 && n as usize <= MAX_LIST_LIMIT => n as usize,
                Some(_) => return Err(DataError::BadRequest(format!("limit is 1–{MAX_LIST_LIMIT}"))),
            };
            let acc = access(who, &space)?;
            if who.project_id().is_some() && acc == Access::None {
                return Err(DataError::NotGranted { key: None });
            }
            let mut names = ByNames::new();
            let out = list_page(&space, &acc, param(params, "after"), limit, &mut names)?;
            audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
            Ok(out)
        }
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

/// The hourly GC (GB10), after the reaper: one audit line with counts.
pub fn gc_tick() {
    match store().blob_gc(super::now()) {
        Ok(r) if r.rows > 0 || r.files > 0 => {
            audit::line("zen.data.blob_gc", "k2", &format!("rows={} files={} bytes={}", r.rows, r.files, r.bytes));
        }
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[garden-data] blob gc: {e}"),
    }
}

#[cfg(test)]
#[path = "blob_routes_tests.rs"]
mod tests;
