//! Home avatar cache (prd-home-picker-and-remote-avatars-v1 P9, P13–P16, S3).
//!
//! A Home row can point at an agent on another server. The app fetches that
//! agent's image with that server's own login and hands the bytes to THIS
//! computer's daemon, which keeps one copy for every window:
//!
//! ```text
//! ~/.k2/cache/agent-avatars/<server>/<workspace>.<ext>    the image
//! ~/.k2/cache/agent-avatars/<server>/<workspace>.json     its meta
//! ```
//!
//! `<server>` is the row's host key and `<workspace>` its handle (the two
//! halves of `handle::host`). Both are encoded one-to-one: every byte outside
//! `[a-z0-9.-]` becomes `_` plus two lowercase hex digits (P9).
//!
//! The daemon only stores. It never fetches (P10): it holds no remote logins.
//! Change detection is the sha256 of the decoded bytes (P13): a put with the
//! same bytes only moves `fetchedAt` and never rewrites the image.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fs_atomic::atomic_write;

/// Largest decoded image the cache accepts (P15).
pub const MAX_IMAGE_BYTES: usize = 128 * 1024;
/// Most entries the cache keeps; a new entry past this evicts the oldest (P15).
pub const MAX_ENTRIES: usize = 2_000;
/// Longest encoded path component (P9).
pub const MAX_COMPONENT_BYTES: usize = 200;
/// Prune keeps an unlisted entry fetched less than this long ago (P17), so a
/// row another window added a moment ago survives.
pub const PRUNE_GRACE_MS: u64 = 120_000;

const META_VERSION: u32 = 1;
const META_SUFFIX: &str = ".json";

/// One process-wide lock for put and prune (P15).
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// A refusal with a stable wire code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarError {
    /// The address is not `<handle>::<host>`, or a half can't be a file name.
    BadAddress(String),
    /// Not `data:image/<type>;base64,…` (an `https://` icon lands here).
    NotDataUrl,
    /// A data URL whose type the cache doesn't store.
    TypeRefused(String),
    /// The bytes aren't the declared type.
    TypeMismatch(String),
    /// Decoded bytes over [`MAX_IMAGE_BYTES`].
    TooLarge(usize),
    /// An SVG that isn't UTF-8 / has no `<svg` / carries a DOCTYPE or ENTITY.
    SvgRefused(String),
    /// A filesystem error.
    Io(String),
}

impl AvatarError {
    pub fn code(&self) -> &'static str {
        match self {
            AvatarError::BadAddress(_) => "bad_address",
            AvatarError::NotDataUrl => "not_data_url",
            AvatarError::TypeRefused(_) => "type_refused",
            AvatarError::TypeMismatch(_) => "type_mismatch",
            AvatarError::TooLarge(_) => "too_large",
            AvatarError::SvgRefused(_) => "svg_refused",
            AvatarError::Io(_) => "io",
        }
    }
}

impl std::fmt::Display for AvatarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvatarError::BadAddress(m) => write!(f, "bad address: {m}"),
            AvatarError::NotDataUrl => {
                write!(f, "the image must be a base64 data:image/… URL")
            }
            AvatarError::TypeRefused(t) => write!(
                f,
                "image type '{t}' isn't cached (png, jpeg, gif, webp, x-icon, svg+xml)"
            ),
            AvatarError::TypeMismatch(t) => write!(f, "the bytes are not a {t} image"),
            AvatarError::TooLarge(n) => write!(
                f,
                "the image is {n} bytes; the cache takes at most {MAX_IMAGE_BYTES}"
            ),
            AvatarError::SvgRefused(m) => write!(f, "svg refused: {m}"),
            AvatarError::Io(m) => write!(f, "io: {m}"),
        }
    }
}

impl From<std::io::Error> for AvatarError {
    fn from(e: std::io::Error) -> Self {
        AvatarError::Io(e.to_string())
    }
}

// ── Encoding (P9) ───────────────────────────────────────────────────────

/// Encode one path component: bytes outside `[a-z0-9.-]` become `_xx`.
/// Refuses empty, `.`, `..` and anything over [`MAX_COMPONENT_BYTES`]
/// once encoded.
pub fn encode_component(raw: &str) -> Result<String, AvatarError> {
    if raw.is_empty() {
        return Err(AvatarError::BadAddress("empty component".into()));
    }
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-' {
            out.push(b as char);
        } else {
            out.push('_');
            out.push_str(&format!("{b:02x}"));
        }
    }
    if out == "." || out == ".." {
        return Err(AvatarError::BadAddress(format!("'{raw}' can't be a file name")));
    }
    if out.len() > MAX_COMPONENT_BYTES {
        return Err(AvatarError::BadAddress(format!(
            "component is {} bytes encoded; at most {MAX_COMPONENT_BYTES}",
            out.len()
        )));
    }
    Ok(out)
}

/// Inverse of [`encode_component`]. `None` for anything it could not have
/// produced (so the mapping stays one-to-one).
pub fn decode_component(enc: &str) -> Option<String> {
    let bytes = enc.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'_' {
            let hex = enc.get(i + 1..i + 3)?;
            if hex.bytes().any(|h| h.is_ascii_uppercase()) {
                return None;
            }
            let v = u8::from_str_radix(hex, 16).ok()?;
            if v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'.' || v == b'-' {
                return None;
            }
            out.push(v);
            i += 3;
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-' {
            out.push(b);
            i += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(out).ok()
}

/// A row address `handle::host`, lowercased and split on its FIRST `::`
/// (matches `parseHomeAddress` in the renderer). A host may hold `::` only
/// inside brackets (`[fe80::1]:38471`).
pub fn parse_address(address: &str) -> Result<(String, String), AvatarError> {
    let t = address.trim().to_lowercase();
    let i = t
        .find("::")
        .ok_or_else(|| AvatarError::BadAddress(format!("'{address}' is not handle::host")))?;
    let (handle, host) = (&t[..i], &t[i + 2..]);
    if handle.is_empty() || host.is_empty() || (host.contains("::") && !host.starts_with('[')) {
        return Err(AvatarError::BadAddress(format!("'{address}' is not handle::host")));
    }
    Ok((handle.to_string(), host.to_string()))
}

/// The normalized address (`handle::host`, lowercase) and its two encoded
/// path components `(server, workspace)`.
pub fn address_key(address: &str) -> Result<(String, String, String), AvatarError> {
    let (handle, host) = parse_address(address)?;
    let server = encode_component(&host)?;
    let ws = encode_component(&handle)?;
    Ok((format!("{handle}::{host}"), server, ws))
}

// ── Data URL validation (P15) ───────────────────────────────────────────

/// A stored image type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Ico,
    Svg,
}

impl ImageKind {
    pub fn ext(self) -> &'static str {
        match self {
            ImageKind::Png => "png",
            ImageKind::Jpeg => "jpg",
            ImageKind::Gif => "gif",
            ImageKind::Webp => "webp",
            ImageKind::Ico => "ico",
            ImageKind::Svg => "svg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageKind::Png => "image/png",
            ImageKind::Jpeg => "image/jpeg",
            ImageKind::Gif => "image/gif",
            ImageKind::Webp => "image/webp",
            ImageKind::Ico => "image/x-icon",
            ImageKind::Svg => "image/svg+xml",
        }
    }

    pub fn from_ext(ext: &str) -> Option<ImageKind> {
        Some(match ext {
            "png" => ImageKind::Png,
            "jpg" => ImageKind::Jpeg,
            "gif" => ImageKind::Gif,
            "webp" => ImageKind::Webp,
            "ico" => ImageKind::Ico,
            "svg" => ImageKind::Svg,
            _ => return None,
        })
    }

    fn from_subtype(sub: &str) -> Option<ImageKind> {
        Some(match sub {
            "png" => ImageKind::Png,
            "jpeg" | "jpg" => ImageKind::Jpeg,
            "gif" => ImageKind::Gif,
            "webp" => ImageKind::Webp,
            "x-icon" | "vnd.microsoft.icon" => ImageKind::Ico,
            "svg+xml" => ImageKind::Svg,
            _ => return None,
        })
    }

    const ALL: [ImageKind; 6] = [
        ImageKind::Png,
        ImageKind::Jpeg,
        ImageKind::Gif,
        ImageKind::Webp,
        ImageKind::Ico,
        ImageKind::Svg,
    ];
}

/// Decode and check a `data:image/<type>;base64,…` URL. Returns the kind
/// and the decoded bytes.
pub fn decode_data_url(url: &str) -> Result<(ImageKind, Vec<u8>), AvatarError> {
    let rest = url
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("data:"))
        .map(|_| &url[5..])
        .ok_or(AvatarError::NotDataUrl)?;
    let (header, payload) = rest.split_once(',').ok_or(AvatarError::NotDataUrl)?;
    let mut parts = header.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let params: Vec<String> = parts.map(|p| p.trim().to_ascii_lowercase()).collect();
    if params.last().map(String::as_str) != Some("base64") {
        return Err(AvatarError::NotDataUrl);
    }
    let sub = mime.strip_prefix("image/").ok_or(AvatarError::TypeRefused(mime.clone()))?;
    let kind = ImageKind::from_subtype(sub).ok_or_else(|| AvatarError::TypeRefused(mime.clone()))?;
    // A base64 payload is 4/3 of the bytes; refuse before decoding.
    let payload = payload.trim();
    if payload.len() / 4 * 3 > MAX_IMAGE_BYTES + 3 {
        return Err(AvatarError::TooLarge(payload.len() / 4 * 3));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| AvatarError::NotDataUrl)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(AvatarError::TooLarge(bytes.len()));
    }
    check_magic(kind, &bytes)?;
    Ok((kind, bytes))
}

fn check_magic(kind: ImageKind, b: &[u8]) -> Result<(), AvatarError> {
    let ok = match kind {
        ImageKind::Png => b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        ImageKind::Jpeg => b.starts_with(&[0xFF, 0xD8, 0xFF]),
        ImageKind::Gif => b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a"),
        ImageKind::Webp => b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP",
        ImageKind::Ico => b.starts_with(&[0x00, 0x00, 0x01, 0x00]),
        ImageKind::Svg => return check_svg(b),
    };
    if ok {
        Ok(())
    } else {
        Err(AvatarError::TypeMismatch(kind.mime().to_string()))
    }
}

fn check_svg(b: &[u8]) -> Result<(), AvatarError> {
    let text = std::str::from_utf8(b)
        .map_err(|_| AvatarError::SvgRefused("not UTF-8".into()))?
        .to_ascii_lowercase();
    if text.contains("<!doctype") || text.contains("<!entity") {
        return Err(AvatarError::SvgRefused("DOCTYPE and ENTITY are not allowed".into()));
    }
    if !text.contains("<svg") {
        return Err(AvatarError::SvgRefused("no <svg> element".into()));
    }
    Ok(())
}

fn sha256_hex(b: &[u8]) -> String {
    let d = Sha256::digest(b);
    d.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── The cache ───────────────────────────────────────────────────────────

/// The meta file beside each image (P9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub v: u32,
    pub ext: Option<String>,
    pub sha256: Option<String>,
    pub bytes: u64,
    pub fetched_at: u64,
    pub missing: bool,
}

/// What a read returns for one address (P14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub data_url: Option<String>,
    pub missing: bool,
    pub fetched_at: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PutOutcome {
    pub address: String,
    pub changed: bool,
    pub missing: bool,
    pub sha256: Option<String>,
    pub fetched_at: u64,
    pub evicted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneOutcome {
    pub removed: usize,
    pub kept: usize,
    pub removed_servers: usize,
}

/// The cache folder. [`AvatarCache::local`] is `~/.k2/cache/agent-avatars`;
/// tests use [`AvatarCache::at`] on a temp folder.
#[derive(Debug, Clone)]
pub struct AvatarCache {
    root: PathBuf,
    max_entries: usize,
}

impl AvatarCache {
    pub fn local() -> Self {
        Self::at(crate::paths::k2_home().join("cache").join("agent-avatars"))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        AvatarCache { root: root.into(), max_entries: MAX_ENTRIES }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the image for `address`, `ext` without the dot.
    pub fn image_path(&self, address: &str, ext: &str) -> Result<PathBuf, AvatarError> {
        let (_, server, ws) = address_key(address)?;
        Ok(self.root.join(server).join(format!("{ws}.{ext}")))
    }

    pub fn meta_path(&self, address: &str) -> Result<PathBuf, AvatarError> {
        let (_, server, ws) = address_key(address)?;
        Ok(self.root.join(server).join(format!("{ws}{META_SUFFIX}")))
    }

    fn read_meta(path: &Path) -> Option<Meta> {
        let text = std::fs::read_to_string(path).ok()?;
        let m: Meta = serde_json::from_str(&text).ok()?;
        (m.v == META_VERSION).then_some(m)
    }

    /// Store an image (`Some(data_url)`) or a miss (`None`) for `address`.
    pub fn put(&self, address: &str, data_url: Option<&str>) -> Result<PutOutcome, AvatarError> {
        self.put_at(address, data_url, now_ms())
    }

    /// [`AvatarCache::put`] with an explicit clock (tests).
    pub fn put_at(
        &self,
        address: &str,
        data_url: Option<&str>,
        now: u64,
    ) -> Result<PutOutcome, AvatarError> {
        let (norm, server, ws) = address_key(address)?;
        // Validate before taking the lock or touching disk.
        let decoded = match data_url {
            Some(u) => Some(decode_data_url(u)?),
            None => None,
        };
        let _g = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.root.join(&server);
        let meta_path = dir.join(format!("{ws}{META_SUFFIX}"));
        let prev = Self::read_meta(&meta_path);
        let evicted = if prev.is_none() { self.evict_for_one_more()? } else { 0 };

        let remove_other_images = |keep_ext: Option<&str>| {
            for k in ImageKind::ALL {
                if Some(k.ext()) == keep_ext {
                    continue;
                }
                let p = dir.join(format!("{ws}.{}", k.ext()));
                if p.exists() {
                    let _ = std::fs::remove_file(&p);
                }
            }
        };

        let outcome = match decoded {
            None => {
                let changed = !prev.as_ref().is_some_and(|m| m.missing);
                let meta = Meta {
                    v: META_VERSION,
                    ext: None,
                    sha256: None,
                    bytes: 0,
                    fetched_at: now,
                    missing: true,
                };
                atomic_write(&meta_path, &serde_json::to_vec(&meta).expect("meta serializes"))?;
                remove_other_images(None);
                PutOutcome {
                    address: norm,
                    changed,
                    missing: true,
                    sha256: None,
                    fetched_at: now,
                    evicted,
                }
            }
            Some((kind, bytes)) => {
                let sha = sha256_hex(&bytes);
                let image_path = dir.join(format!("{ws}.{}", kind.ext()));
                let same = prev.as_ref().is_some_and(|m| {
                    !m.missing
                        && m.sha256.as_deref() == Some(sha.as_str())
                        && m.ext.as_deref() == Some(kind.ext())
                }) && image_path.is_file();
                if !same {
                    atomic_write(&image_path, &bytes)?;
                }
                let meta = Meta {
                    v: META_VERSION,
                    ext: Some(kind.ext().to_string()),
                    sha256: Some(sha.clone()),
                    bytes: bytes.len() as u64,
                    fetched_at: now,
                    missing: false,
                };
                atomic_write(&meta_path, &serde_json::to_vec(&meta).expect("meta serializes"))?;
                if !same {
                    remove_other_images(Some(kind.ext()));
                }
                PutOutcome {
                    address: norm,
                    changed: !same,
                    missing: false,
                    sha256: Some(sha),
                    fetched_at: now,
                    evicted,
                }
            }
        };
        Ok(outcome)
    }

    /// Read one address. `None` when there's no entry (or its image file is
    /// gone, so the app refetches).
    pub fn get(&self, address: &str) -> Result<Option<Entry>, AvatarError> {
        let (_, server, ws) = address_key(address)?;
        let dir = self.root.join(server);
        let Some(meta) = Self::read_meta(&dir.join(format!("{ws}{META_SUFFIX}"))) else {
            return Ok(None);
        };
        if meta.missing {
            return Ok(Some(Entry {
                data_url: None,
                missing: true,
                fetched_at: meta.fetched_at,
                sha256: None,
            }));
        }
        let Some(kind) = meta.ext.as_deref().and_then(ImageKind::from_ext) else {
            return Ok(None);
        };
        let Ok(bytes) = std::fs::read(dir.join(format!("{ws}.{}", kind.ext()))) else {
            return Ok(None);
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Some(Entry {
            data_url: Some(format!("data:{};base64,{b64}", kind.mime())),
            missing: false,
            fetched_at: meta.fetched_at,
            sha256: meta.sha256,
        }))
    }

    /// Every entry as `(server_dir, workspace_stem, meta)`.
    fn all_entries(&self) -> Vec<(PathBuf, String, Meta)> {
        let mut out = Vec::new();
        let Ok(servers) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for s in servers.flatten() {
            let dir = s.path();
            if !dir.is_dir() {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&dir) else { continue };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                let Some(stem) = name.strip_suffix(META_SUFFIX) else { continue };
                if decode_component(stem).is_none() {
                    continue;
                }
                if let Some(m) = Self::read_meta(&f.path()) {
                    out.push((dir.clone(), stem.to_string(), m));
                }
            }
        }
        out
    }

    /// Number of entries (meta files) in the cache.
    pub fn entry_count(&self) -> usize {
        self.all_entries().len()
    }

    fn remove_entry(dir: &Path, stem: &str) {
        let _ = std::fs::remove_file(dir.join(format!("{stem}{META_SUFFIX}")));
        for k in ImageKind::ALL {
            let p = dir.join(format!("{stem}.{}", k.ext()));
            if p.exists() {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// Before adding one NEW entry: evict the oldest `fetchedAt` entries so
    /// the cache stays at [`MAX_ENTRIES`]. Returns how many went.
    fn evict_for_one_more(&self) -> Result<usize, AvatarError> {
        let count = self.count_meta_files();
        if count < self.max_entries {
            return Ok(0);
        }
        let mut all = self.all_entries();
        all.sort_by_key(|(_, _, m)| m.fetched_at);
        let drop_n = count + 1 - self.max_entries;
        for (dir, stem, _) in all.iter().take(drop_n) {
            Self::remove_entry(dir, stem);
        }
        Ok(drop_n.min(all.len()))
    }

    /// Cheap count: `*.json` names only, no reads.
    fn count_meta_files(&self) -> usize {
        let Ok(servers) = std::fs::read_dir(&self.root) else { return 0 };
        servers
            .flatten()
            .filter(|s| s.path().is_dir())
            .map(|s| {
                std::fs::read_dir(s.path())
                    .map(|fs| {
                        fs.flatten()
                            .filter(|f| {
                                let n = f.file_name();
                                let n = n.to_string_lossy();
                                n.strip_suffix(META_SUFFIX).is_some_and(|st| decode_component(st).is_some())
                            })
                            .count()
                    })
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Delete every entry whose address is not in `keep` and whose
    /// `fetchedAt` is more than [`PRUNE_GRACE_MS`] old (P17). Stray files
    /// (an orphaned image, a crashed temp file) older than the grace go too,
    /// and server folders left empty are removed.
    pub fn prune(&self, keep: &[String]) -> Result<PruneOutcome, AvatarError> {
        self.prune_at(keep, now_ms())
    }

    pub fn prune_at(&self, keep: &[String], now: u64) -> Result<PruneOutcome, AvatarError> {
        let _g = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let keep: HashSet<(String, String)> = keep
            .iter()
            .filter_map(|a| address_key(a).ok())
            .map(|(_, s, w)| (s, w))
            .collect();
        let mut out = PruneOutcome { removed: 0, kept: 0, removed_servers: 0 };
        let Ok(servers) = std::fs::read_dir(&self.root) else {
            return Ok(out);
        };
        let old = |t: u64| now.saturating_sub(t) > PRUNE_GRACE_MS;
        for s in servers.flatten() {
            let dir = s.path();
            if !dir.is_dir() {
                continue;
            }
            let server = s.file_name().to_string_lossy().to_string();
            // Live image names: the image of every entry that stays.
            let mut live: HashSet<String> = HashSet::new();
            let files: Vec<_> = std::fs::read_dir(&dir).map(|r| r.flatten().collect()).unwrap_or_default();
            for f in &files {
                let name = f.file_name().to_string_lossy().to_string();
                let Some(stem) = name.strip_suffix(META_SUFFIX) else { continue };
                if decode_component(stem).is_none() {
                    continue;
                }
                let meta = Self::read_meta(&f.path());
                let listed = keep.contains(&(server.clone(), stem.to_string()));
                let fetched = meta.as_ref().map(|m| m.fetched_at).unwrap_or(0);
                if listed || !old(fetched) {
                    out.kept += 1;
                    live.insert(name.clone());
                    if let Some(ext) = meta.and_then(|m| m.ext) {
                        live.insert(format!("{stem}.{ext}"));
                    }
                } else {
                    Self::remove_entry(&dir, stem);
                    out.removed += 1;
                }
            }
            // Stray files: anything not live and older than the grace.
            for f in &files {
                let name = f.file_name().to_string_lossy().to_string();
                if live.contains(&name) || !f.path().exists() {
                    continue;
                }
                let mtime = f
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if old(mtime) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
            let empty = std::fs::read_dir(&dir).map(|mut r| r.next().is_none()).unwrap_or(false);
            if empty && std::fs::remove_dir(&dir).is_ok() {
                out.removed_servers += 1;
            }
        }
        Ok(out)
    }
}
