//! Resumable transfers that survive an app in the background (0.46.9,
//! Tauri App SDK, K2's side). `k2 study app-transfers` is the guide.
//!
//! A phone can't keep a 200 MB upload alive in its own process: iOS hands
//! big transfers to a background `URLSession`, Android to WorkManager, and
//! both speak plain HTTPS with a header they can carry. So a transfer has
//! two halves:
//!
//! - **The session**, made by the person's pass (`files.uploadCreate` on
//!   the App's gateway link or the browser helper). Every rule of today's
//!   chunked upload applies once, up front: the room, `files:write` or
//!   `files:upload`, the folder scope, the drop folders and their caps, the
//!   10 GiB ceiling, free disk and the disk guard. K2 answers an upload id,
//!   an expiry and a short-lived **upload token** (`k2up_<id>.<secret>`)
//!   that opens this one upload and nothing else.
//! - **The door** (`/_k2/transfers/…` on the App's own host or the box's
//!   address, `transfer_door` in k2-daemon). The OS sends the bytes there
//!   with `Authorization: Bearer <token>`: `PUT` ranges with
//!   `Content-Range`, in any order, each idempotent; `HEAD` says what K2
//!   holds; `POST …/complete` checks the size and the sha256 and publishes.
//!   Each request re-reads the person's pass by its id, so a revoked pass,
//!   grant or cap stops the upload at its next range.
//!
//! The bytes land where today's guest chunks land: a dot-named part
//! (`fs_guest_upload::guest_part_name`) in the destination folder, written
//! with `O_NOFOLLOW`, never through a hard link, and published under the
//! first free name without replacing anything. The part is in the same
//! ledger, so the 7-day abandoned-part sweep covers it too; a session also
//! ends at its own expiry ([`sweep_expired`]).
//!
//! Downloads get the same split: a **download grant** (`k2dn_<id>.<secret>`)
//! names one file the person may already save; the door serves it raw with
//! `Range`, `If-Range` and a strong `ETag`, re-checking the person's rights
//! on every request.
//!
//! State is a JSON file per session or grant under `~/.k2/transfers/`
//! (tokens stored only as sha256), so no migration and a daemon restart
//! loses nothing.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fs_commands::{free_disk_space, MAX_TRANSFER_SIZE, MAX_UPLOAD_CHUNK_SIZE};
use crate::fs_guest_upload as gu;

/// Upload tokens: `k2up_<upload id>.<secret>`.
pub const UPLOAD_TOKEN_PREFIX: &str = "k2up_";
/// Download tokens: `k2dn_<grant id>.<secret>`.
pub const DOWNLOAD_TOKEN_PREFIX: &str = "k2dn_";
/// The door's path prefix (with its slash).
pub const DOOR_PREFIX: &str = "/_k2/transfers/";
/// An upload's door path: [`DOOR_PREFIX`]`uploads/<id>`.
pub fn upload_url(id: &str) -> String {
    format!("{DOOR_PREFIX}uploads/{id}")
}
/// A download's door path: [`DOOR_PREFIX`]`downloads/<id>`.
pub fn download_url(id: &str) -> String {
    format!("{DOOR_PREFIX}downloads/{id}")
}

/// An upload session ends this long after it was made (as an abandoned
/// part: a paused upload resumes over a weekend).
pub const SESSION_TTL: Duration = gu::ABANDONED_PART_TTL;
/// An upload token lives this long unless the create asks otherwise…
pub const TOKEN_TTL_DEFAULT_SECS: u64 = 6 * 3600;
/// …and never longer than this, nor shorter than [`TOKEN_TTL_MIN_SECS`].
pub const TOKEN_TTL_MAX_SECS: u64 = 24 * 3600;
pub const TOKEN_TTL_MIN_SECS: u64 = 60;
/// A download grant lives this long by default (same max and min).
pub const DOWNLOAD_TTL_DEFAULT_SECS: u64 = 3600;
/// The most one `PUT` carries: one of today's chunks.
pub const MAX_RANGE_LEN: u64 = MAX_UPLOAD_CHUNK_SIZE;
/// Received ranges held apart at most (a session file stays small).
pub const MAX_FRAGMENTS: usize = 4096;
/// Live upload sessions per person at once.
pub const MAX_LIVE_UPLOADS_PER_HOLDER: usize = 32;
/// Live download grants per person at once.
pub const MAX_LIVE_DOWNLOADS_PER_HOLDER: usize = 256;
/// Upload tokens kept per session (a new one doesn't end the older ones,
/// so transfers the OS already queued keep working until theirs expire).
pub const MAX_TOKENS_PER_UPLOAD: usize = 4;

/// Unix seconds.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `~/.k2/transfers/uploads`.
pub fn uploads_dir() -> PathBuf {
    crate::paths::k2_home().join("transfers").join("uploads")
}

/// `~/.k2/transfers/downloads`.
pub fn downloads_dir() -> PathBuf {
    crate::paths::k2_home().join("transfers").join("downloads")
}

// ── who holds a transfer ──────────────────────────────────────────────

/// Who made a session or a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    /// The stable identity an upload is bound to (`principal:<id>`, or
    /// `pass:<id>` for a platform pass; `person:<login>` for a person's
    /// download grant).
    pub principal: String,
    /// The pass that made it, re-read on every door request. `None`: a
    /// person (owner or Connect login) on the attachment door; their grant
    /// is checked by its token and expiry alone.
    pub pass_id: Option<String>,
    /// For audit lines.
    pub username: String,
}

// ── received ranges ──────────────────────────────────────────────────

/// Byte ranges held, half-open `[start, end)`, sorted and merged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeSet(pub Vec<(u64, u64)>);

impl RangeSet {
    /// Add `[start, end)`, merging touching and overlapping ranges.
    pub fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let (mut s, mut e) = (start, end);
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut placed = false;
        for &(a, b) in &self.0 {
            if b < s {
                out.push((a, b));
            } else if a > e {
                if !placed {
                    out.push((s, e));
                    placed = true;
                }
                out.push((a, b));
            } else {
                s = s.min(a);
                e = e.max(b);
            }
        }
        if !placed {
            out.push((s, e));
        }
        self.0 = out;
    }

    /// Would `[start, end)` add a separate fragment?
    pub fn would_fragment(&self, start: u64, end: u64) -> bool {
        !self.0.iter().any(|&(a, b)| !(b < start || a > end))
    }

    /// Bytes held.
    pub fn total(&self) -> u64 {
        self.0.iter().map(|(a, b)| b - a).sum()
    }

    /// Every byte of `size` held?
    pub fn complete(&self, size: u64) -> bool {
        size == 0 || (self.0.len() == 1 && self.0[0] == (0, size))
    }

    /// The gaps of `[0, size)`, half-open.
    pub fn missing(&self, size: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut at = 0;
        for &(a, b) in &self.0 {
            if a > at {
                out.push((at, a.min(size)));
            }
            at = at.max(b);
        }
        if at < size {
            out.push((at, size));
        }
        out
    }

    /// As inclusive `[first, last]` pairs (the wire's form, like
    /// `Content-Range`).
    pub fn inclusive(ranges: &[(u64, u64)]) -> Vec<[u64; 2]> {
        ranges.iter().filter(|(a, b)| b > a).map(|&(a, b)| [a, b - 1]).collect()
    }

    /// `first-last,first-last` (inclusive), for the `K2-Upload-Received`
    /// header. Empty when nothing is held.
    pub fn header(ranges: &[(u64, u64)]) -> String {
        Self::inclusive(ranges).iter().map(|[a, b]| format!("{a}-{b}")).collect::<Vec<_>>().join(",")
    }
}

// ── tokens ───────────────────────────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex(n: usize) -> Result<String, String> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|e| format!("random: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// A new id (32 hex characters).
pub fn new_id() -> Result<String, String> {
    random_hex(16)
}

/// True for an id shaped like [`new_id`]'s output.
pub fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `<prefix><id>.<secret>` and its stored hash.
fn mint_token(prefix: &str, id: &str) -> Result<(String, String), String> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|e| format!("random: {e}"))?;
    let token = format!("{prefix}{id}.{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret));
    let hash = sha256_hex(token.as_bytes());
    Ok((token, hash))
}

/// The id a token names, when it has the token's shape (only the shape).
pub fn token_id<'a>(prefix: &str, raw: &'a str) -> Option<&'a str> {
    let rest = raw.trim().strip_prefix(prefix)?;
    let (id, secret) = rest.split_once('.')?;
    let ok = is_id(id) && secret.len() == 43 && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(id)
}

fn hash_matches(stored: &str, raw: &str) -> bool {
    use subtle::ConstantTimeEq;
    let got = sha256_hex(raw.trim().as_bytes());
    stored.len() == got.len() && bool::from(stored.as_bytes().ct_eq(got.as_bytes()))
}

/// Clamp a requested token lifetime.
pub fn clamp_ttl(asked: Option<u64>, default: u64) -> u64 {
    asked.unwrap_or(default).clamp(TOKEN_TTL_MIN_SECS, TOKEN_TTL_MAX_SECS)
}

/// One upload token's stored hash and expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRec {
    pub hash: String,
    pub expires_at: u64,
}

// ── errors ───────────────────────────────────────────────────────────

/// Why a transfer step was refused. The daemon maps each to a status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// No such session or grant, a token for another one, a bad or expired
    /// token: one answer, no oracle. 401.
    Unauthorized,
    /// A real upload token past its own expiry (the session lives on):
    /// ask the App's server for a new one (`files.uploadToken`). 401.
    TokenExpired,
    /// The session or grant is past its expiry. 410.
    Expired,
    /// The upload already completed; a range or abort after that. 409.
    AlreadyDone,
    /// A malformed or out-of-bounds range, or a body that isn't the range.
    /// 400.
    BadRange(String),
    /// Over a limit (one range, the 10 GiB ceiling). 413.
    TooLarge(String),
    /// The range's own `K2-Content-Sha256` didn't match its bytes; nothing
    /// was written. 422.
    RangeChecksum { expected: String, actual: String },
    /// `complete` before every byte arrived. 409.
    Incomplete { missing: Vec<(u64, u64)> },
    /// The whole file's sha256 didn't match: every received range is
    /// dropped and the file must be sent again. 422.
    ChecksumMismatch { expected: String, actual: String },
    /// Too many separate ranges held at once. 400.
    TooFragmented,
    /// Not enough disk (the guest message names nobody). 507.
    NoSpace(String),
    /// Too many live sessions or grants for this person. 429.
    TooMany(String),
    /// Bad input, or the part is something other than our regular file. 400.
    Invalid(String),
    /// Disk error. 500.
    Io(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("no such transfer for this token"),
            Self::TokenExpired => f.write_str("this upload token has expired; ask for a new one (files.uploadToken)"),
            Self::Expired => f.write_str("this transfer has expired; start a new one"),
            Self::AlreadyDone => f.write_str("this upload is already complete"),
            Self::RangeChecksum { expected, actual } => {
                write!(f, "this range's sha256 is {actual}, not {expected}; nothing was written, send it again")
            }
            Self::Incomplete { missing } => write!(f, "{} range(s) still missing", missing.len()),
            Self::ChecksumMismatch { expected, actual } => {
                write!(f, "the file's sha256 is {actual}, not {expected}; K2 dropped what it held, send the file again")
            }
            Self::TooFragmented => {
                write!(f, "more than {MAX_FRAGMENTS} separate ranges held; send the gaps between them first")
            }
            Self::BadRange(m) | Self::TooLarge(m) | Self::NoSpace(m) | Self::TooMany(m) | Self::Invalid(m) | Self::Io(m) => {
                f.write_str(m)
            }
        }
    }
}

fn io(e: impl std::fmt::Display) -> TransferError {
    TransferError::Io(e.to_string())
}

/// A sha256 the client sent: 64 hex characters, any case. `None` when
/// absent or blank; `Err` when malformed.
pub fn parse_sha256(raw: Option<&str>) -> Result<Option<String>, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(Some(s.to_ascii_lowercase())),
        Some(_) => Err("sha256 is 64 hex characters".to_string()),
    }
}

// ── the session store ────────────────────────────────────────────────

/// A finished upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// Room-relative path of the published file.
    pub rel_path: String,
    /// Where it landed.
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

/// One upload session, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadSession {
    pub id: String,
    pub holder: Holder,
    /// The room (project id).
    pub project_id: String,
    /// The room's canonical folder (the published path is relative to it).
    pub root: PathBuf,
    /// The canonical destination folder (inside `root`).
    pub dir: PathBuf,
    /// `dir`, room-relative.
    pub rel_dir: String,
    /// The sanitized final name.
    pub filename: String,
    /// The part's file name in `dir` (a guest part name).
    pub part: String,
    /// Declared size.
    pub size: u64,
    /// Declared sha256 (lower-case hex), when given at create.
    pub sha256: Option<String>,
    /// `true`: made with `files:upload` alone (an upload-only drop): the
    /// door re-checks that cap, else `files:write`.
    pub drop: bool,
    pub received: RangeSet,
    pub tokens: Vec<TokenRec>,
    pub created_at: u64,
    pub expires_at: u64,
    pub done: Option<Done>,
}

impl UploadSession {
    pub fn expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    /// The JSON every status answer carries (camelCase; ranges inclusive).
    pub fn status_json(&self) -> serde_json::Value {
        let missing = self.received.missing(self.size);
        let mut v = serde_json::json!({
            "uploadId": self.id,
            "url": upload_url(&self.id),
            "size": self.size,
            "received": RangeSet::inclusive(&self.received.0),
            "receivedBytes": self.received.total(),
            "missing": RangeSet::inclusive(&missing),
            "complete": self.received.complete(self.size),
            "done": self.done.is_some(),
            "expiresAt": self.expires_at.saturating_mul(1000),
            "partMax": MAX_RANGE_LEN,
        });
        if let Some(d) = &self.done {
            v["relPath"] = serde_json::json!(d.rel_path);
            v["sha256"] = serde_json::json!(d.sha256);
        }
        v
    }
}

fn session_file(store: &Path, id: &str) -> PathBuf {
    store.join(format!("{id}.json"))
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<(), TransferError> {
    let dir = path.parent().ok_or_else(|| TransferError::Io("no parent folder".into()))?;
    fs::create_dir_all(dir).map_err(io)?;
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(value).map_err(io)?;
    {
        let mut f = fs::File::create(&tmp).map_err(io)?;
        f.write_all(&bytes).map_err(io)?;
        f.sync_all().map_err(io)?;
    }
    fs::rename(&tmp, path).map_err(io)
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, TransferError> {
    match fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| TransferError::Io(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

/// Load a session by id (`None`: no such session).
pub fn load_session(store: &Path, id: &str) -> Result<Option<UploadSession>, TransferError> {
    if !is_id(id) {
        return Ok(None);
    }
    load_json(&session_file(store, id))
}

fn save_session(store: &Path, s: &UploadSession) -> Result<(), TransferError> {
    save_json(&session_file(store, &s.id), s)
}

/// One writer per session: a range's write and its record must not
/// interleave with another range's for the same session.
fn session_lock(id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut g = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 4096 {
        g.retain(|_, l| Arc::strong_count(l) > 1);
    }
    g.entry(id.to_string()).or_default().clone()
}

/// Every live (unexpired) session `principal` holds.
pub fn live_uploads_of(store: &Path, principal: &str, now: u64) -> Result<Vec<UploadSession>, TransferError> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(store) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io(e)),
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json") else { continue };
        if let Ok(Some(s)) = load_session(store, id) {
            if s.holder.principal == principal && !s.expired(now) {
                out.push(s);
            }
        }
    }
    Ok(out)
}

/// What a new upload needs. The daemon has already checked the room, the
/// caps, the folder scope and the drop caps; `dir` is the canonical,
/// jailed destination folder (it exists).
#[derive(Debug, Clone)]
pub struct NewUpload {
    pub holder: Holder,
    pub project_id: String,
    pub root: PathBuf,
    pub dir: PathBuf,
    pub rel_dir: String,
    /// The name as sent (sanitized here).
    pub filename: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub drop: bool,
    pub token_ttl_secs: u64,
}

/// Make a session: its empty part (in the ledger), its file and its first
/// token. Returns the session and the token (shown once).
pub fn create_upload(store: &Path, ledger: &Path, n: NewUpload, now: u64) -> Result<(UploadSession, String), TransferError> {
    if n.size > MAX_TRANSFER_SIZE {
        return Err(TransferError::TooLarge(format!(
            "transfer of {} bytes exceeds the {} GiB ceiling",
            n.size,
            MAX_TRANSFER_SIZE / (1024 * 1024 * 1024)
        )));
    }
    match fs::symlink_metadata(&n.dir) {
        Ok(m) if m.file_type().is_dir() => {}
        _ => return Err(TransferError::Invalid("destination is not a folder".into())),
    }
    if let Some(free) = free_disk_space(&n.dir) {
        if n.size > free {
            return Err(TransferError::NoSpace(crate::disk_guard::GUEST_NO_SPACE.to_string()));
        }
    }
    if crate::disk_guard::reserve_ok(&n.dir, n.size).is_err() {
        return Err(TransferError::NoSpace(crate::disk_guard::GUEST_NO_SPACE.to_string()));
    }
    let live = live_uploads_of(store, &n.holder.principal, now)?;
    if live.iter().filter(|s| s.done.is_none()).count() >= MAX_LIVE_UPLOADS_PER_HOLDER {
        return Err(TransferError::TooMany(format!(
            "at most {MAX_LIVE_UPLOADS_PER_HOLDER} uploads at once per person: complete or abort one first"
        )));
    }
    let id = new_id().map_err(TransferError::Io)?;
    let filename = gu::final_name(&n.filename);
    let part = gu::guest_part_name(&n.holder.principal, &n.project_id, &n.dir, &filename, &id);
    let part_abs = n.dir.join(&part);
    gu::ledger_record(ledger, &part_abs).map_err(TransferError::Io)?;
    {
        let mut o = fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.custom_flags(libc::O_NOFOLLOW);
        }
        o.open(&part_abs).map_err(|e| TransferError::Invalid(format!("upload part could not be made: {e}")))?;
    }
    let (token, hash) = mint_token(UPLOAD_TOKEN_PREFIX, &id).map_err(TransferError::Io)?;
    let s = UploadSession {
        id,
        holder: n.holder,
        project_id: n.project_id,
        root: n.root,
        dir: n.dir,
        rel_dir: n.rel_dir,
        filename,
        part,
        size: n.size,
        sha256: n.sha256,
        drop: n.drop,
        received: RangeSet::default(),
        tokens: vec![TokenRec { hash, expires_at: now + n.token_ttl_secs }],
        created_at: now,
        expires_at: now + SESSION_TTL.as_secs(),
        done: None,
    };
    save_session(store, &s)?;
    Ok((s, token))
}

/// The session an upload token opens, when the token is one of its live
/// tokens and `id` (from the door's path) is the one it names. Anything
/// else is [`TransferError::Unauthorized`]; a session past its expiry is
/// [`TransferError::Expired`].
pub fn authorize_upload(store: &Path, id: &str, raw_token: &str, now: u64) -> Result<UploadSession, TransferError> {
    if token_id(UPLOAD_TOKEN_PREFIX, raw_token) != Some(id) {
        return Err(TransferError::Unauthorized);
    }
    let Some(s) = load_session(store, id)? else { return Err(TransferError::Unauthorized) };
    // Only a real token learns more than "no".
    let Some(t) = s.tokens.iter().filter(|t| hash_matches(&t.hash, raw_token)).max_by_key(|t| t.expires_at) else {
        return Err(TransferError::Unauthorized);
    };
    if s.expired(now) {
        return Err(TransferError::Expired);
    }
    if t.expires_at <= now {
        return Err(TransferError::TokenExpired);
    }
    Ok(s)
}

/// A session `principal` holds (for the pass-side verbs). Another
/// person's id is [`TransferError::Unauthorized`], never "exists".
pub fn session_of(store: &Path, id: &str, principal: &str, now: u64) -> Result<UploadSession, TransferError> {
    let Some(s) = load_session(store, id)? else { return Err(TransferError::Unauthorized) };
    if s.holder.principal != principal {
        return Err(TransferError::Unauthorized);
    }
    if s.expired(now) {
        return Err(TransferError::Expired);
    }
    Ok(s)
}

/// A fresh upload token for a live, unfinished session (the oldest is
/// dropped past [`MAX_TOKENS_PER_UPLOAD`]). Returns the token and its expiry.
pub fn add_token(store: &Path, id: &str, ttl_secs: u64, now: u64) -> Result<(String, u64), TransferError> {
    let lock = session_lock(id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut s) = load_session(store, id)? else { return Err(TransferError::Unauthorized) };
    if s.expired(now) {
        return Err(TransferError::Expired);
    }
    if s.done.is_some() {
        return Err(TransferError::AlreadyDone);
    }
    let (token, hash) = mint_token(UPLOAD_TOKEN_PREFIX, id).map_err(TransferError::Io)?;
    let expires_at = (now + ttl_secs).min(s.expires_at);
    s.tokens.retain(|t| t.expires_at > now);
    s.tokens.push(TokenRec { hash, expires_at });
    while s.tokens.len() > MAX_TOKENS_PER_UPLOAD {
        s.tokens.remove(0);
    }
    save_session(store, &s)?;
    Ok((token, expires_at))
}

fn open_part(path: &Path) -> Result<fs::File, TransferError> {
    let f = gu::open_part_no_follow(path).map_err(|e| TransferError::Invalid(format!("upload part could not be opened: {e}")))?;
    let meta = f.metadata().map_err(io)?;
    // A symlink (O_NOFOLLOW) or anything but our one-link regular file is
    // refused: a hard link at the part's name is someone else's file.
    if !meta.file_type().is_file() || crate::private_fs::link_count(&f).map_or(true, |n| n > 1) {
        return Err(TransferError::Invalid("upload part is not a regular file".into()));
    }
    Ok(f)
}

/// Write one range (`start`, these `bytes`) of a session whose declared
/// size the client repeats as `total`. Idempotent: the same range again
/// writes the same bytes. `range_sha256` (lower-case hex), when given, must
/// match the bytes before anything is written.
pub fn write_range(
    store: &Path,
    ledger: &Path,
    id: &str,
    start: u64,
    total: u64,
    bytes: &[u8],
    range_sha256: Option<&str>,
    now: u64,
) -> Result<UploadSession, TransferError> {
    let lock = session_lock(id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut s) = load_session(store, id)? else { return Err(TransferError::Unauthorized) };
    if s.expired(now) {
        return Err(TransferError::Expired);
    }
    if s.done.is_some() {
        return Err(TransferError::AlreadyDone);
    }
    let len = bytes.len() as u64;
    if len > MAX_RANGE_LEN {
        return Err(TransferError::TooLarge(format!("one range carries at most {MAX_RANGE_LEN} bytes")));
    }
    if total != s.size {
        return Err(TransferError::BadRange(format!("this upload's size is {}, not {total}", s.size)));
    }
    if len == 0 || start.checked_add(len).is_none_or(|end| end > s.size) {
        return Err(TransferError::BadRange(format!("a range must be inside 0-{}", s.size.saturating_sub(1))));
    }
    if let Some(expected) = range_sha256 {
        let actual = sha256_hex(bytes);
        if actual != expected {
            return Err(TransferError::RangeChecksum { expected: expected.to_string(), actual });
        }
    }
    let end = start + len;
    if s.received.0.len() >= MAX_FRAGMENTS && s.received.would_fragment(start, end) {
        return Err(TransferError::TooFragmented);
    }
    if crate::disk_guard::reserve_ok(&s.dir, len).is_err() {
        return Err(TransferError::NoSpace(crate::disk_guard::GUEST_NO_SPACE.to_string()));
    }
    match fs::symlink_metadata(&s.dir) {
        Ok(m) if m.file_type().is_dir() => {}
        _ => return Err(TransferError::Invalid("destination is not a folder".into())),
    }
    let part = s.dir.join(&s.part);
    // The part went away (the abandoned-part sweep, a person tidying up):
    // what it held is gone, so the session starts again from nothing.
    if matches!(fs::symlink_metadata(&part), Err(ref e) if e.kind() == std::io::ErrorKind::NotFound) {
        s.received = RangeSet::default();
        gu::ledger_record(ledger, &part).map_err(TransferError::Io)?;
    }
    let mut f = open_part(&part)?;
    f.seek(SeekFrom::Start(start)).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    // Durable before it counts as received.
    f.sync_data().map_err(io)?;
    s.received.insert(start, end);
    save_session(store, &s)?;
    Ok(s)
}

/// Hash a file's first `size` bytes.
fn sha256_file(f: &mut fs::File, size: u64) -> Result<String, TransferError> {
    f.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut left = size;
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        let n = f.read(&mut buf[..want]).map_err(io)?;
        if n == 0 {
            return Err(TransferError::Io("upload part is shorter than its ranges".into()));
        }
        h.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Finish an upload: every byte held, the size right, the sha256 right
/// (the one given here and/or at create), then publish under the first
/// free name. Idempotent: a completed session answers its result again.
pub fn complete_upload(store: &Path, ledger: &Path, id: &str, sha256: Option<&str>, now: u64) -> Result<Done, TransferError> {
    let lock = session_lock(id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut s) = load_session(store, id)? else { return Err(TransferError::Unauthorized) };
    if let Some(d) = &s.done {
        return match sha256 {
            Some(x) if x != d.sha256 => Err(TransferError::ChecksumMismatch { expected: x.to_string(), actual: d.sha256.clone() }),
            _ => Ok(d.clone()),
        };
    }
    if s.expired(now) {
        return Err(TransferError::Expired);
    }
    if let (Some(given), Some(declared)) = (sha256, s.sha256.as_deref()) {
        if given != declared {
            return Err(TransferError::Invalid(format!("sha256 {given} is not the one this upload was made with ({declared})")));
        }
    }
    let part = s.dir.join(&s.part);
    if s.size > 0 && !matches!(fs::symlink_metadata(&part), Ok(ref m) if m.file_type().is_file()) {
        s.received = RangeSet::default();
        save_session(store, &s)?;
    }
    if !s.received.complete(s.size) {
        return Err(TransferError::Incomplete { missing: s.received.missing(s.size) });
    }
    let mut f = open_part(&part)?;
    if f.metadata().map_err(io)?.len() != s.size {
        f.set_len(s.size).map_err(io)?;
    }
    let actual = sha256_file(&mut f, s.size)?;
    if let Some(expected) = sha256.or(s.sha256.as_deref()) {
        if actual != expected {
            // Which range went wrong is unknown: drop them all.
            s.received = RangeSet::default();
            f.set_len(0).map_err(io)?;
            save_session(store, &s)?;
            return Err(TransferError::ChecksumMismatch { expected: expected.to_string(), actual });
        }
    }
    f.sync_all().map_err(io)?;
    drop(f);
    let path = gu::publish_no_replace(&part, &s.dir, &s.filename).map_err(|e| match e {
        gu::GuestChunkError::Io(m) => TransferError::Io(m),
        other => TransferError::Invalid(other.to_string()),
    })?;
    if let Err(e) = gu::ledger_clear(ledger, &part) {
        crate::log_debug!("[fs/transfer] {e}");
    }
    let rel_path = path
        .strip_prefix(&s.root)
        .map(|p| crate::paths::slash_rel(p))
        .unwrap_or_else(|_| s.filename.clone());
    let done = Done { rel_path, path, size: s.size, sha256: actual };
    // The tokens stay: a retried complete answers the same result.
    s.done = Some(done.clone());
    save_session(store, &s)?;
    Ok(done)
}

/// Remove a session's part (when it is still our regular part file).
fn remove_part(ledger: &Path, s: &UploadSession) {
    let part = s.dir.join(&s.part);
    if gu::is_guest_part_name(&s.part) {
        if let Ok(m) = fs::symlink_metadata(&part) {
            if m.file_type().is_file() {
                let _ = fs::remove_file(&part);
            }
        }
    }
    let _ = gu::ledger_clear(ledger, &part);
}

/// End a session: its part and its record go. A completed upload's file
/// stays (abort only drops the record).
pub fn abort_upload(store: &Path, ledger: &Path, id: &str) -> Result<(), TransferError> {
    let lock = session_lock(id);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = load_session(store, id)? else { return Err(TransferError::Unauthorized) };
    if s.done.is_none() {
        remove_part(ledger, &s);
    }
    match fs::remove_file(session_file(store, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(e)),
    }
}

// ── download grants ──────────────────────────────────────────────────

/// What the door re-checks before each download request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Recheck {
    /// `files.download`'s rules for this room-relative path.
    #[serde(rename_all = "camelCase")]
    File { workspace: String, rel_path: String },
    /// A Thread attachment's rules: the guest may still read and save this
    /// room-relative path in this room.
    #[serde(rename_all = "camelCase")]
    Attachment { project_id: String, rel_path: String },
    /// A person's grant: its token and expiry alone.
    Person,
}

/// One download grant, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadGrant {
    pub id: String,
    pub holder: Holder,
    pub token_hash: String,
    /// The canonical file.
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
    pub recheck: Recheck,
    pub created_at: u64,
    pub expires_at: u64,
}

fn grant_file(store: &Path, id: &str) -> PathBuf {
    store.join(format!("{id}.json"))
}

/// Mint a grant for `path` (already checked). Returns it and its token.
#[allow(clippy::too_many_arguments)]
pub fn mint_download(
    store: &Path,
    holder: Holder,
    path: PathBuf,
    name: String,
    mime: String,
    recheck: Recheck,
    ttl_secs: u64,
    now: u64,
) -> Result<(DownloadGrant, String), TransferError> {
    let live = live_downloads_of(store, &holder.principal, now)?;
    if live >= MAX_LIVE_DOWNLOADS_PER_HOLDER {
        return Err(TransferError::TooMany(format!(
            "at most {MAX_LIVE_DOWNLOADS_PER_HOLDER} download tokens live at once per person; they expire within {} h",
            TOKEN_TTL_MAX_SECS / 3600
        )));
    }
    let id = new_id().map_err(TransferError::Io)?;
    let (token, token_hash) = mint_token(DOWNLOAD_TOKEN_PREFIX, &id).map_err(TransferError::Io)?;
    let g = DownloadGrant { id, holder, token_hash, path, name, mime, recheck, created_at: now, expires_at: now + ttl_secs };
    save_json(&grant_file(store, &g.id), &g)?;
    Ok((g, token))
}

fn live_downloads_of(store: &Path, principal: &str, now: u64) -> Result<usize, TransferError> {
    let rd = match fs::read_dir(store) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io(e)),
    };
    let mut n = 0;
    for e in rd.flatten() {
        if let Ok(Some(g)) = load_json::<DownloadGrant>(&e.path()) {
            if g.holder.principal == principal && g.expires_at > now {
                n += 1;
            }
        }
    }
    Ok(n)
}

/// The grant a download token opens (`id` from the door's path).
pub fn authorize_download(store: &Path, id: &str, raw_token: &str, now: u64) -> Result<DownloadGrant, TransferError> {
    if token_id(DOWNLOAD_TOKEN_PREFIX, raw_token) != Some(id) {
        return Err(TransferError::Unauthorized);
    }
    let Some(g) = load_json::<DownloadGrant>(&grant_file(store, id))? else { return Err(TransferError::Unauthorized) };
    if !hash_matches(&g.token_hash, raw_token) {
        return Err(TransferError::Unauthorized);
    }
    if now >= g.expires_at {
        return Err(TransferError::Expired);
    }
    Ok(g)
}

// ── the sweep ────────────────────────────────────────────────────────

/// What one [`sweep_expired`] pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Upload sessions past their expiry (their parts removed).
    pub uploads: usize,
    /// Download grants past their expiry.
    pub downloads: usize,
}

/// Remove every upload session and download grant past its expiry, and
/// an expired, unfinished session's part.
pub fn sweep_expired(uploads: &Path, downloads: &Path, ledger: &Path, now: u64) -> Result<SweepReport, String> {
    let mut r = SweepReport::default();
    if let Ok(rd) = fs::read_dir(uploads) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".json") else { continue };
            let lock = session_lock(id);
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            match load_session(uploads, id) {
                Ok(Some(s)) if s.expired(now) => {
                    if s.done.is_none() {
                        remove_part(ledger, &s);
                    }
                    fs::remove_file(e.path()).map_err(|e| format!("upload session: {e}"))?;
                    r.uploads += 1;
                }
                Ok(_) => {}
                // An unreadable record is dropped (it opens nothing).
                Err(_) => {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
    if let Ok(rd) = fs::read_dir(downloads) {
        for e in rd.flatten() {
            match load_json::<DownloadGrant>(&e.path()) {
                Ok(Some(g)) if now >= g.expires_at => {
                    fs::remove_file(e.path()).map_err(|e| format!("download grant: {e}"))?;
                    r.downloads += 1;
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
    Ok(r)
}

// ── HTTP pieces the door uses (pure) ─────────────────────────────────

/// `Content-Range: bytes <first>-<last>/<total>` → `(first, last, total)`.
pub fn parse_content_range(v: &str) -> Result<(u64, u64, u64), String> {
    let bad = || "Content-Range is bytes <first>-<last>/<size>".to_string();
    let rest = v.trim().strip_prefix("bytes").ok_or_else(bad)?.trim_start();
    let (span, total) = rest.split_once('/').ok_or_else(bad)?;
    let (a, b) = span.split_once('-').ok_or_else(bad)?;
    let num = |s: &str| s.trim().parse::<u64>().map_err(|_| bad());
    let (a, b, t) = (num(a)?, num(b)?, num(total)?);
    if b < a || b >= t {
        return Err(bad());
    }
    Ok((a, b, t))
}

/// What a download's `Range` header asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeAsk {
    /// The whole file (no header, one we don't serve, or If-Range missed).
    Full,
    /// Inclusive `[first, last]`.
    Part(u64, u64),
    /// Outside the file: 416.
    Unsatisfiable,
}

/// One `bytes=` range of a file of `size` (several ranges: the whole file,
/// as RFC 9110 allows).
pub fn parse_range(v: &str, size: u64) -> RangeAsk {
    let Some(spec) = v.trim().strip_prefix("bytes=") else { return RangeAsk::Full };
    if spec.contains(',') {
        return RangeAsk::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return RangeAsk::Full };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // Suffix: the last `b` bytes.
        let Ok(n) = b.parse::<u64>() else { return RangeAsk::Full };
        if n == 0 || size == 0 {
            return RangeAsk::Unsatisfiable;
        }
        return RangeAsk::Part(size.saturating_sub(n), size - 1);
    }
    let Ok(first) = a.parse::<u64>() else { return RangeAsk::Full };
    if first >= size {
        return RangeAsk::Unsatisfiable;
    }
    let last = if b.is_empty() {
        size - 1
    } else {
        match b.parse::<u64>() {
            Ok(l) if l >= first => l.min(size - 1),
            _ => return RangeAsk::Full,
        }
    };
    RangeAsk::Part(first, last)
}

/// May a `Range` be honoured under this `If-Range`? Only an exact strong
/// ETag match (a date never matches: K2 compares validators, not clocks).
pub fn if_range_allows(if_range: Option<&str>, etag: &str) -> bool {
    match if_range.map(str::trim) {
        None => true,
        Some(v) => v == etag,
    }
}

/// A strong ETag for a file version: its size, its change time and, on
/// Unix, its device and inode. Any edit or replacement changes it.
pub fn file_etag(meta: &fs::Metadata) -> String {
    let mut h = Sha256::new();
    h.update(meta.len().to_le_bytes());
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    h.update(mtime.to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        h.update(meta.dev().to_le_bytes());
        h.update(meta.ino().to_le_bytes());
        h.update(meta.ctime().to_le_bytes());
        h.update(meta.ctime_nsec().to_le_bytes());
    }
    let hex: String = h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

#[cfg(test)]
#[path = "fs_transfer_tests.rs"]
mod tests;
