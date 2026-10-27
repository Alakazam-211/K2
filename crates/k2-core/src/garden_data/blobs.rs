//! Garden blobs (prd-garden-board-needs-v1 §3, GB1–GB18): content-addressed
//! files for a widget's `data:` space.
//!
//! A value names a blob by a ref, `k2blob:sha256-<64 lower hex>`, anywhere
//! inside its JSON (an exact string; never a ref inside a longer string).
//! The daemon computes every hash from the bytes it received. A ref counts
//! only for a blob the space already holds (`space_blobs`), so dedup across
//! spaces can't hand one widget's readers another's bytes (GB3, V9).
//!
//! This file holds the pure parts (refs, mime sniffing, names, file I/O);
//! the tables and the store calls are `blob_store.rs`.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{DataError, Json};

/// The ref prefix (GB1).
pub const REF_PREFIX: &str = "k2blob:sha256-";
/// One blob (Q1).
pub const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;
/// Held plus pending blobs of one `data:` space (Q2, GB11).
pub const MAX_SPACE_BLOB_BYTES: i64 = 256 * 1024 * 1024;
/// The whole blob folder (Q2): past it GC removes the oldest let-go blobs
/// first, then writes answer `store_full`.
pub const MAX_BLOB_STORE_BYTES: i64 = 2 * 1024 * 1024 * 1024;
/// Distinct refs in one value (GB2).
pub const MAX_REFS_PER_VALUE: usize = 64;
/// An upload no key ever named (a cancelled paste).
pub const BLOB_PENDING_KEEP_SECS: i64 = 24 * 3_600;
/// A blob no key names any more (Q10): longer than the 7-day history and
/// the ~5-week dated snapshots, so a restore never finds its blob gone.
pub const BLOB_UNREF_KEEP_SECS: i64 = 35 * 86_400;
/// One chunk on the wire (raw bytes; `DOWNLOAD_CHUNK_BYTES` in the app).
pub const BLOB_CHUNK_BYTES: usize = 8 * 1024 * 1024;
/// A blob put body: 8 MiB of raw bytes in base64 plus the envelope.
pub const MAX_BLOB_BODY: usize = 12 * 1024 * 1024;
/// An open chunked upload idle this long is dropped.
pub const UPLOAD_IDLE_SECS: i64 = 60;
/// Uploads per placement or agent per minute.
pub const UPLOADS_PER_MIN: u32 = 30;
/// Uploaded bytes per placement or agent per 10 minutes.
pub const UPLOAD_BYTES_PER_10_MIN: usize = 128 * 1024 * 1024;
/// A blob's file name (plain text).
pub const MAX_NAME_CHARS: usize = 255;
/// Missing refs named in one `blob_missing`.
pub const MAX_MISSING_LISTED: usize = 20;
/// A file left by a crash between the write and its row is swept after this.
pub const STRAY_FILE_KEEP_SECS: i64 = 3_600;

/// The `/boot-status` feature key and catalog feature for the blob verbs.
pub const FEATURE: &str = "garden-blobs-v1";

/// What the daemon stores a blob as (GB14). Anything else is
/// `application/octet-stream`.
pub const KNOWN_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/heic",
    "application/pdf",
    "image/svg+xml",
    "application/json",
    "text/plain",
];

pub const OCTET: &str = "application/octet-stream";

/// One blob as a caller sees it (`BlobInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlobInfo {
    #[serde(rename = "ref")]
    pub reference: String,
    pub size: i64,
    pub mime: String,
    pub name: Option<String>,
    /// False when this space already held these bytes.
    pub new: bool,
}

/// One `space_blobs` row joined with its mime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRow {
    pub hash: String,
    pub size: i64,
    pub mime: String,
    pub name: Option<String>,
    /// Live keys in the space that name it.
    pub refs: i64,
    pub added_at: i64,
    /// `Actor::stored()` of the uploader.
    pub added_by: String,
    /// Never named by a key yet.
    pub pending: bool,
}

impl BlobRow {
    pub fn reference(&self) -> String {
        ref_of(&self.hash)
    }

    pub fn info(&self, new: bool) -> BlobInfo {
        BlobInfo { reference: self.reference(), size: self.size, mime: self.mime.clone(), name: self.name.clone(), new }
    }
}

/// What one GC pass removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobGcReport {
    /// `space_blobs` rows dropped (pending past 24 h, let go past 35 d).
    pub rows: usize,
    /// Files removed from disk.
    pub files: usize,
    pub bytes: i64,
}

// ── refs ─────────────────────────────────────────────────────────────

/// A lowercase sha256 hex string.
pub fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The ref for a hash.
pub fn ref_of(hash: &str) -> String {
    format!("{REF_PREFIX}{hash}")
}

/// The hash in an exact ref, or `None`.
pub fn hash_of_ref(s: &str) -> Option<&str> {
    s.strip_prefix(REF_PREFIX).filter(|h| valid_hash(h))
}

/// A ref argument: the hash, or `bad_request`.
pub fn parse_ref(s: &str) -> Result<String, DataError> {
    hash_of_ref(s.trim())
        .map(str::to_string)
        .ok_or_else(|| DataError::BadRequest("a blob ref looks like k2blob:sha256-<64 lowercase hex>".into()))
}

/// Every distinct ref in a value (GB2): JSON strings that are exactly a ref,
/// in arrays and object values at any depth (object keys are names, not
/// refs). More than [`MAX_REFS_PER_VALUE`] is `bad_request`.
pub fn scan_refs(v: &Json) -> Result<BTreeSet<String>, DataError> {
    fn walk(v: &Json, out: &mut BTreeSet<String>) {
        match v {
            Json::String(s) => {
                if let Some(h) = hash_of_ref(s) {
                    out.insert(h.to_string());
                }
            }
            Json::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Json::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(v, &mut out);
    if out.len() > MAX_REFS_PER_VALUE {
        return Err(DataError::BadRequest(format!(
            "a value names at most {MAX_REFS_PER_VALUE} blobs; this one names {}",
            out.len()
        )));
    }
    Ok(out)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::zen::bundle::sha256_hex(bytes)
}

// ── mime sniffing (GB14) ─────────────────────────────────────────────

fn ftyp_brand(b: &[u8]) -> Option<&[u8]> {
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        Some(&b[8..12])
    } else {
        None
    }
}

/// An XML document whose root element is `<svg`: an optional BOM,
/// whitespace, `<?xml …?>`, comments and a doctype may come first.
fn looks_like_svg(b: &[u8]) -> bool {
    let head = &b[..b.len().min(4096)];
    let Ok(s) = std::str::from_utf8(head).or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()])) else {
        return false;
    };
    let mut s = s.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("<?") {
            match rest.find("?>") {
                Some(i) => s = rest[i + 2..].trim_start(),
                None => return false,
            }
        } else if let Some(rest) = s.strip_prefix("<!--") {
            match rest.find("-->") {
                Some(i) => s = rest[i + 3..].trim_start(),
                None => return false,
            }
        } else if let Some(rest) = s.strip_prefix("<!") {
            match rest.find('>') {
                Some(i) => s = rest[i + 1..].trim_start(),
                None => return false,
            }
        } else {
            break;
        }
    }
    s.strip_prefix("<svg").is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/'))
}

fn looks_like_text(b: &[u8]) -> bool {
    match std::str::from_utf8(b) {
        Ok(s) => !s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}')),
        Err(_) => false,
    }
}

/// The stored mime of `bytes` (GB14): sniffed from the leading bytes; when
/// the sniff can't tell, the declared mime if it's on [`KNOWN_MIMES`]
/// (never SVG: an SVG must look like one); else `application/octet-stream`.
pub fn sniff(bytes: &[u8], declared: Option<&str>) -> String {
    let b = bytes;
    let sniffed: Option<&str> = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if let Some(brand) = ftyp_brand(b) {
        match brand {
            b"avif" | b"avis" => Some("image/avif"),
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => Some("image/heic"),
            _ => None,
        }
    } else if looks_like_svg(b) {
        Some("image/svg+xml")
    } else if looks_like_text(b) {
        let t = std::str::from_utf8(b).unwrap_or("").trim_start_matches('\u{feff}').trim();
        if (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<Json>(t).is_ok() {
            Some("application/json")
        } else {
            Some("text/plain")
        }
    } else {
        None
    };
    if let Some(m) = sniffed {
        return m.to_string();
    }
    let d = declared.map(|d| d.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).unwrap_or_default();
    if KNOWN_MIMES.contains(&d.as_str()) && d != "image/svg+xml" && d != "text/plain" && d != "application/json" {
        d
    } else {
        OCTET.to_string()
    }
}

/// A file name as kept (plain text): the basename, control characters
/// stripped, at most [`MAX_NAME_CHARS`] characters; `None` when nothing is
/// left.
pub fn clean_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let s: String = base.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    let s = s.trim().to_string();
    if s.is_empty() || s == "." || s == ".." {
        None
    } else {
        Some(s)
    }
}

// ── files (GB5) ──────────────────────────────────────────────────────

/// `<blobs>/sha256/<first 2 hex>/<64 hex>`. The hash is checked first, so
/// no caller string ever becomes a path.
pub fn blob_path(dir: &Path, hash: &str) -> Result<PathBuf, DataError> {
    if !valid_hash(hash) {
        return Err(DataError::BadRequest("not a blob hash".into()));
    }
    Ok(dir.join("sha256").join(&hash[..2]).join(hash))
}

fn protect_dir(d: &Path) -> Result<(), DataError> {
    std::fs::create_dir_all(d).map_err(|e| DataError::Unavailable(format!("can't create the blob folder: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| DataError::Unavailable(format!("can't protect the blob folder: {e}")))?;
    }
    Ok(())
}

/// Write `bytes` as the blob `hash` (already computed from them): a
/// dot-named `.part` file in the same folder, checked, fsynced, renamed.
/// An existing blob is never replaced (dedup). Answers true when a new
/// file was written.
pub fn write_file(dir: &Path, hash: &str, bytes: &[u8]) -> Result<bool, DataError> {
    let path = blob_path(dir, hash)?;
    if path.is_file() {
        return Ok(false);
    }
    let folder = path.parent().ok_or_else(|| DataError::Unavailable("blob folder".into()))?;
    protect_dir(dir)?;
    protect_dir(&dir.join("sha256"))?;
    protect_dir(folder)?;
    if sha256_hex(bytes) != hash {
        return Err(DataError::Unavailable("blob bytes don't match their hash".into()));
    }
    let part = folder.join(format!(".{hash}.{}.part", super::tx::random_hex(4)));
    let io = |e: std::io::Error| DataError::Unavailable(format!("can't write a blob: {e}"));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&part).map_err(io)?;
        if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
            let _ = std::fs::remove_file(&part);
            return Err(io(e));
        }
    }
    if path.is_file() {
        // Someone else wrote the same bytes meanwhile: keep theirs.
        std::fs::remove_file(&part).map_err(io)?;
        return Ok(false);
    }
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        io(e)
    })?;
    Ok(true)
}

/// Read `len` bytes at `offset` (clamped to the file). Answers the bytes
/// and whether they reach the end.
pub fn read_range(dir: &Path, hash: &str, offset: u64, len: usize) -> Result<(Vec<u8>, bool), DataError> {
    let path = blob_path(dir, hash)?;
    let mut f = std::fs::File::open(&path)
        .map_err(|e| DataError::Unavailable(format!("a blob file is missing or unreadable: {e}")))?;
    let size = f.metadata().map_err(|e| DataError::Unavailable(format!("blob: {e}")))?.len();
    if offset > size {
        return Err(DataError::BadRequest(format!("offset {offset} is past the end ({size} bytes)")));
    }
    f.seek(SeekFrom::Start(offset)).map_err(|e| DataError::Unavailable(format!("blob: {e}")))?;
    let want = (size - offset).min(len as u64) as usize;
    let mut buf = vec![0u8; want];
    f.read_exact(&mut buf).map_err(|e| DataError::Unavailable(format!("blob: {e}")))?;
    Ok((buf, offset + want as u64 >= size))
}

/// The whole blob (at most [`MAX_BLOB_BYTES`] by construction).
pub fn read_all(dir: &Path, hash: &str) -> Result<Vec<u8>, DataError> {
    read_range(dir, hash, 0, MAX_BLOB_BYTES).map(|(b, _)| b)
}

/// Remove a blob's file (GC). A missing file is fine.
pub fn remove_file(dir: &Path, hash: &str) -> Result<bool, DataError> {
    let path = blob_path(dir, hash)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(DataError::Unavailable(format!("can't remove a blob file: {e}"))),
    }
}

/// Files under the blob folder: `(hash, modified secs)` for blob files and
/// `(path, modified secs)` for leftover `.part` files.
pub fn walk_files(dir: &Path) -> Result<(Vec<(String, i64)>, Vec<(PathBuf, i64)>), DataError> {
    let mut blobs = Vec::new();
    let mut parts = Vec::new();
    let root = dir.join("sha256");
    let Ok(rd) = std::fs::read_dir(&root) else {
        return Ok((blobs, parts));
    };
    let mtime = |p: &Path| -> i64 {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64)
    };
    for sub in rd.flatten() {
        let Ok(inner) = std::fs::read_dir(sub.path()) else { continue };
        for f in inner.flatten() {
            let p = f.path();
            let name = f.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name.ends_with(".part") {
                parts.push((p.clone(), mtime(&p)));
            } else if valid_hash(&name) {
                blobs.push((name, mtime(&p)));
            }
        }
    }
    Ok((blobs, parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const H: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn refs_are_exact_strings_found_at_any_depth() {
        let r = ref_of(H);
        let v = json!({"src": r, "list": [{"deep": r}, "see k2blob:sha256-x"], "k2blob:sha256-key": 1, "n": 2});
        let found = scan_refs(&v).expect("scan");
        assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![H.to_string()]);
        let inside = json!({"t": format!("see {r}"), "u": format!("{r} ")});
        assert!(scan_refs(&inside).expect("scan").is_empty(), "a ref inside a longer string is not a ref");
        assert!(hash_of_ref(&format!("{REF_PREFIX}{}", H.to_uppercase())).is_none(), "upper hex is not a ref");
        assert!(hash_of_ref(&format!("{REF_PREFIX}{}", &H[..63])).is_none());
    }

    #[test]
    fn more_than_64_refs_in_a_value_is_refused() {
        let many: Vec<Json> = (0..65).map(|i| Json::String(ref_of(&sha256_hex(format!("{i}").as_bytes())))).collect();
        let e = scan_refs(&Json::Array(many.clone())).unwrap_err();
        assert_eq!(e.code(), "bad_request");
        assert_eq!(scan_refs(&Json::Array(many[..64].to_vec())).expect("64 is fine").len(), 64);
    }

    #[test]
    fn mime_is_sniffed_never_trusted() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest", Some("text/plain")), "image/png");
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0], None), "image/jpeg");
        assert_eq!(sniff(b"GIF89a....", None), "image/gif");
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ", None), "image/webp");
        assert_eq!(sniff(b"%PDF-1.7\n", None), "application/pdf");
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0", None), "image/avif");
        assert_eq!(sniff(b"\0\0\0\x18ftypheic\0\0", None), "image/heic");
        assert_eq!(sniff(b"<?xml version=\"1.0\"?>\n<!-- x --><svg xmlns='http://www.w3.org/2000/svg'/>", None), "image/svg+xml");
        assert_eq!(sniff(b"<html><svg></svg></html>", Some("image/svg+xml")), "text/plain", "an HTML page is not an SVG");
        assert_eq!(sniff(b"{\"a\": [1, 2]}", None), "application/json");
        assert_eq!(sniff(b"hello\nworld", Some("application/json")), "text/plain");
        assert_eq!(sniff(&[0u8, 1, 2, 3, 0xFE], Some("image/png")), "image/png", "an unknown sniff falls back to a known declared mime");
        assert_eq!(sniff(&[0u8, 1, 2, 3, 0xFE], Some("text/html")), OCTET);
        assert_eq!(sniff(&[0u8, 1, 2, 3, 0xFE], Some("image/svg+xml")), OCTET, "an SVG must look like one");
        assert_eq!(sniff(&[0u8, 1, 2, 3, 0xFE], None), OCTET);
    }

    #[test]
    fn names_are_plain_basenames() {
        assert_eq!(clean_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(clean_name("C:\\Users\\x\\photo.png").as_deref(), Some("photo.png"));
        assert_eq!(clean_name("a\u{0}b\nc.png").as_deref(), Some("abc.png"));
        assert_eq!(clean_name("..").as_deref(), None);
        assert_eq!(clean_name("").as_deref(), None);
        assert_eq!(clean_name(&"x".repeat(400)).map(|s| s.chars().count()), Some(MAX_NAME_CHARS));
    }

    #[test]
    fn files_are_written_once_and_read_in_ranges() {
        let root = crate::test_env::unique_temp_path("gd-blobs");
        let d = root.join("blobs");
        let bytes = b"hello blob".to_vec();
        let h = sha256_hex(&bytes);
        assert!(write_file(d.as_path(), &h, &bytes).expect("write"), "a new file");
        assert!(!write_file(d.as_path(), &h, &bytes).expect("dedup"), "the same bytes again: kept, not replaced");
        assert_eq!(write_file(d.as_path(), &sha256_hex(b"other"), &bytes).unwrap_err().code(), "store_unavailable");
        assert_eq!(read_range(d.as_path(), &h, 6, 100).expect("range"), (b"blob".to_vec(), true));
        assert_eq!(read_range(d.as_path(), &h, 0, 5).expect("range"), (b"hello".to_vec(), false));
        assert_eq!(read_range(d.as_path(), &h, 11, 1).unwrap_err().code(), "bad_request");
        assert_eq!(blob_path(d.as_path(), "../../x").unwrap_err().code(), "bad_request", "no caller string becomes a path");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = blob_path(d.as_path(), &h).expect("path");
            assert_eq!(std::fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(p.parent().expect("dir")).expect("meta").permissions().mode() & 0o777, 0o700);
        }
        let (files, parts) = walk_files(d.as_path()).expect("walk");
        assert_eq!(files.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(), vec![h.as_str()]);
        assert!(parts.is_empty(), "no .part file is left behind");
        assert!(remove_file(d.as_path(), &h).expect("remove"));
        assert!(!remove_file(d.as_path(), &h).expect("gone already"));
        let _ = std::fs::remove_dir_all(root);
    }
}
