//! A screenshot a person attaches to their feedback
//! (prd-feedback-screenshot-v1 §4.1, §5). One PNG per feedback.
//!
//! The renderer flattens the person's marks and covers onto the picture and
//! uploads the PNG as a **draft** ([`normalize`] then [`put_draft`]). The
//! person then sees the daemon's own stored copy and confirms it; `send`
//! names that draft by id and by its sha256, and a different hash is
//! refused. So a picture only leaves after the person saw exactly those
//! bytes (the "seen" gate, mirroring the diagnostics preview).
//!
//! What [`normalize`] does, in order: at most [`MAX_BYTES`]; the PNG
//! signature; every chunk well formed with a good CRC; `IHDR` first, each
//! side 1 to [`MAX_SIDE`]; no `acTL` (APNG); no unknown critical chunk; the
//! chunks cut to [`KEEP_CHUNKS`] (text, EXIF, time and every other
//! ancillary chunk go, the pixels don't change); it decodes under
//! [`image::Limits`] ([`MAX_SIDE`], [`MAX_ALLOC`]). The draft's hash is
//! taken after the cut.
//!
//! Drafts live in memory like the diagnostics drafts: owner-bound, an hour,
//! at most [`MAX_DRAFTS_PER_OWNER`] per person and [`MAX_DRAFTS`] in all
//! (the oldest go first). A restart loses them; `send` then answers
//! `410 screenshot_expired`.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The largest PNG kept or sent: 2 MiB.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// The largest side, in pixels.
pub const MAX_SIDE: u32 = 4096;
/// The decoder's allocation cap.
pub const MAX_ALLOC: u64 = 128 * 1024 * 1024;
/// A draft is kept this long.
pub const DRAFT_TTL_MS: i64 = 3_600_000;
/// Live drafts per person, and in all.
pub const MAX_DRAFTS_PER_OWNER: usize = 3;
pub const MAX_DRAFTS: usize = 16;
/// Where the picture came from: K2's window, or an image the person chose.
pub const SOURCES: &[&str] = &["window", "file"];
/// How a window picture was taken (K2, 11-10: stored so the blanks stay
/// honest): `page` = drawn from the page's DOM (terminals, web frames, 3D
/// and video are labelled blanks); `webview` = the main webview's own
/// snapshot (the Q1 fallback). A file has none.
pub const CAPTURES: &[&str] = &["page", "webview"];
/// The chunks a stored PNG keeps (§4.1); every other one is cut.
pub const KEEP_CHUNKS: &[&[u8; 4]] = &[b"IHDR", b"PLTE", b"tRNS", b"IDAT", b"IEND", b"sRGB", b"gAMA", b"pHYs"];

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Why a picture was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShotError {
    /// Over [`MAX_BYTES`] (the decoded size, or what the base64 would decode to).
    TooLarge(usize),
    NotBase64,
    NotPng,
    Damaged(String),
    TooWide { width: u32, height: u32 },
    Animated,
    Unreadable(String),
}

impl ShotError {
    /// The refusal code: `screenshot_too_large` (413) or `bad_screenshot` (400).
    pub fn code(&self) -> &'static str {
        match self {
            ShotError::TooLarge(_) => "screenshot_too_large",
            _ => "bad_screenshot",
        }
    }

    /// The plain line the dialog shows.
    pub fn message(&self) -> String {
        match self {
            ShotError::TooLarge(n) => {
                format!("A screenshot is at most 2 MB; this one is {n} bytes. Attach a smaller image.")
            }
            ShotError::NotBase64 => "The picture isn't base64.".into(),
            ShotError::NotPng => "That isn't a PNG picture.".into(),
            ShotError::Damaged(why) => format!("That PNG is damaged: {why}."),
            ShotError::TooWide { width, height } => {
                format!("A screenshot is at most {MAX_SIDE} pixels a side; this one is {width}×{height}.")
            }
            ShotError::Animated => "Animated PNGs can't be attached.".into(),
            ShotError::Unreadable(_) => "K2 can't read that PNG.".into(),
        }
    }
}

/// A checked PNG, chunks cut, ready to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub png: Vec<u8>,
    /// Lowercase hex sha256 of [`Normalized::png`].
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

/// What the outbox keeps about a picture after its bytes are gone
/// (`product_feedback_outbox.screenshot_meta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub bytes: usize,
    pub width: u32,
    pub height: u32,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
    pub sha256: String,
}

impl Meta {
    pub fn parse(json: &str) -> Option<Meta> {
        serde_json::from_str(json).ok()
    }
}

/// Lowercase hex sha256.
pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Decode the upload's `image`: bare base64, or a `data:image/png;base64,`
/// URL. Refuses over [`MAX_BYTES`] before decoding.
pub fn decode_base64(image: &str) -> Result<Vec<u8>, ShotError> {
    use base64::Engine;
    let t = image.trim();
    let payload = match t.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("data:") => {
            let (head, rest) = t[5..].split_once(',').ok_or(ShotError::NotBase64)?;
            let head = head.to_ascii_lowercase();
            if head != "image/png;base64" {
                return Err(ShotError::NotPng);
            }
            rest
        }
        _ => t,
    };
    let payload: String = payload.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let would_be = payload.len() / 4 * 3;
    if would_be > MAX_BYTES + 3 {
        return Err(ShotError::TooLarge(would_be));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(payload.as_bytes()).map_err(|_| ShotError::NotBase64)?;
    if bytes.len() > MAX_BYTES {
        return Err(ShotError::TooLarge(bytes.len()));
    }
    Ok(bytes)
}

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
};

/// The PNG chunk CRC (ISO 3309) over the chunk type and data.
pub fn crc32(b: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &x in b {
        c = CRC_TABLE[((c ^ u32::from(x)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Check a PNG, cut its chunks to [`KEEP_CHUNKS`], decode it under limits
/// and hash the result.
pub fn normalize(b: &[u8]) -> Result<Normalized, ShotError> {
    if b.len() > MAX_BYTES {
        return Err(ShotError::TooLarge(b.len()));
    }
    if !b.starts_with(&SIGNATURE) {
        return Err(ShotError::NotPng);
    }
    let damaged = |why: &str| ShotError::Damaged(why.to_string());
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&SIGNATURE);
    let mut i = SIGNATURE.len();
    let (mut width, mut height) = (0u32, 0u32);
    let (mut first, mut seen_idat, mut seen_iend) = (true, false, false);
    while i < b.len() {
        if b.len() - i < 12 {
            return Err(damaged("it is cut short"));
        }
        let len = be32(&b[i..]) as usize;
        if len > 0x7FFF_FFFF {
            return Err(damaged("a chunk is too long"));
        }
        let end = i.checked_add(12 + len).ok_or_else(|| damaged("a chunk is too long"))?;
        if end > b.len() {
            return Err(damaged("it is cut short"));
        }
        let ty: [u8; 4] = [b[i + 4], b[i + 5], b[i + 6], b[i + 7]];
        if !ty.iter().all(u8::is_ascii_alphabetic) {
            return Err(damaged("a chunk has a bad name"));
        }
        if crc32(&b[i + 4..i + 8 + len]) != be32(&b[i + 8 + len..]) {
            return Err(damaged("a chunk's checksum is wrong"));
        }
        let data = &b[i + 8..i + 8 + len];
        if first {
            if &ty != b"IHDR" || len != 13 {
                return Err(damaged("the header isn't first"));
            }
            width = be32(data);
            height = be32(&data[4..]);
            if width == 0 || height == 0 {
                return Err(damaged("it has no pixels"));
            }
            if width > MAX_SIDE || height > MAX_SIDE {
                return Err(ShotError::TooWide { width, height });
            }
        } else if &ty == b"IHDR" {
            return Err(damaged("it has two headers"));
        }
        if &ty == b"acTL" {
            return Err(ShotError::Animated);
        }
        let keep = KEEP_CHUNKS.iter().any(|k| **k == ty);
        // A critical chunk (uppercase first letter) a decoder must
        // understand: never dropped silently.
        if !keep && ty[0].is_ascii_uppercase() {
            return Err(damaged("it has a chunk K2 doesn't know"));
        }
        if keep {
            out.extend_from_slice(&b[i..end]);
        }
        seen_idat |= &ty == b"IDAT";
        first = false;
        i = end;
        if &ty == b"IEND" {
            seen_iend = true;
            // Anything after the end is cut too.
            break;
        }
    }
    if !seen_idat {
        return Err(damaged("it has no image data"));
    }
    if !seen_iend {
        return Err(damaged("it has no end"));
    }
    decode_check(&out, width, height)?;
    let sha256 = sha256_hex(&out);
    Ok(Normalized { png: out, sha256, width, height })
}

/// Decode under limits (one of the box-wide decode slots).
fn decode_check(png: &[u8], width: u32, height: u32) -> Result<(), ShotError> {
    let _slot = crate::skin::guard::decode_slot();
    let mut reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| ShotError::Unreadable(e.to_string()))?;
    if img.width() != width || img.height() != height {
        return Err(ShotError::Unreadable("the size changed in decoding".into()));
    }
    Ok(())
}

/// A picture the person is looking at, kept for [`DRAFT_TTL_MS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// `pfs_` + 22 base62.
    pub id: String,
    /// The local sender that uploaded it (`owner`, `login:<user>`).
    pub owner: String,
    pub png: Arc<Vec<u8>>,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub source: String,
    pub capture: Option<String>,
    pub created_at: i64,
}

impl Draft {
    pub fn meta(&self) -> Meta {
        Meta {
            bytes: self.png.len(),
            width: self.width,
            height: self.height,
            source: self.source.clone(),
            capture: self.capture.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

fn drafts() -> &'static Mutex<HashMap<String, Draft>> {
    static D: std::sync::OnceLock<Mutex<HashMap<String, Draft>>> = std::sync::OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_draft_id() -> String {
    format!("pfs_{}", &super::new_client_id()[3..])
}

/// Whether `id` has the draft id shape.
pub fn is_draft_id(id: &str) -> bool {
    id.len() == 26 && id.starts_with("pfs_") && id[4..].bytes().all(|b| b.is_ascii_alphanumeric())
}

fn oldest<'a>(m: &'a HashMap<String, Draft>, owner: Option<&str>) -> Option<&'a String> {
    m.iter()
        .filter(|(_, d)| owner.is_none_or(|o| d.owner == o))
        .min_by(|a, b| (a.1.created_at, a.0).cmp(&(b.1.created_at, b.0)))
        .map(|(k, _)| k)
}

/// Keep a checked picture as `owner`'s draft. Expired drafts go; then the
/// owner's oldest past [`MAX_DRAFTS_PER_OWNER`], then the oldest of all
/// past [`MAX_DRAFTS`].
pub fn put_draft(owner: &str, n: Normalized, source: &str, capture: Option<&str>, now_ms: i64) -> Draft {
    let d = Draft {
        id: new_draft_id(),
        owner: owner.to_string(),
        png: Arc::new(n.png),
        sha256: n.sha256,
        width: n.width,
        height: n.height,
        source: source.to_string(),
        capture: capture.map(str::to_string),
        created_at: now_ms,
    };
    let mut m = drafts().lock().unwrap_or_else(|p| p.into_inner());
    m.retain(|_, v| now_ms - v.created_at < DRAFT_TTL_MS);
    while m.values().filter(|v| v.owner == owner).count() >= MAX_DRAFTS_PER_OWNER {
        let Some(k) = oldest(&m, Some(owner)).cloned() else { break };
        m.remove(&k);
    }
    while m.len() >= MAX_DRAFTS {
        let Some(k) = oldest(&m, None).cloned() else { break };
        m.remove(&k);
    }
    m.insert(d.id.clone(), d.clone());
    d
}

/// The caller's own live draft.
pub fn get_draft(id: &str, owner: &str, now_ms: i64) -> Option<Draft> {
    let m = drafts().lock().unwrap_or_else(|p| p.into_inner());
    m.get(id).filter(|d| d.owner == owner && now_ms - d.created_at < DRAFT_TTL_MS).cloned()
}

/// Forget the caller's own draft (Remove). Whether one was there.
pub fn forget_draft(id: &str, owner: &str) -> bool {
    let mut m = drafts().lock().unwrap_or_else(|p| p.into_inner());
    if m.get(id).is_some_and(|d| d.owner == owner) {
        m.remove(id);
        true
    } else {
        false
    }
}

/// Drop a draft once its row is written.
pub fn drop_draft(id: &str) {
    drafts().lock().unwrap_or_else(|p| p.into_inner()).remove(id);
}

/// Test hook: how many drafts `owner` has (live or not).
#[doc(hidden)]
pub fn draft_count(owner: &str) -> usize {
    drafts().lock().unwrap_or_else(|p| p.into_inner()).values().filter(|d| d.owner == owner).count()
}

/// Test fixtures: PNGs built in memory, and raw chunks.
#[doc(hidden)]
pub mod fixture {
    use super::crc32;

    /// A `w`×`h` RGBA PNG whose pixels vary (so they compress a little).
    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).expect("encode fixture png");
        out.into_inner()
    }

    /// One chunk: length, type, data, CRC.
    pub fn chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = Vec::with_capacity(12 + data.len());
        c.extend_from_slice(&(data.len() as u32).to_be_bytes());
        c.extend_from_slice(ty);
        c.extend_from_slice(data);
        let crc = crc32(&c[4..]);
        c.extend_from_slice(&crc.to_be_bytes());
        c
    }

    /// `png` with `extra` chunks right after IHDR (8 + 25 bytes in).
    pub fn with_chunks_after_ihdr(png: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
        let at = 8 + 25;
        let mut out = png[..at].to_vec();
        for c in extra {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&png[at..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{chunk, png, with_chunks_after_ihdr};
    use super::*;

    fn chunk_names(p: &[u8]) -> Vec<String> {
        let mut i = 8;
        let mut v = Vec::new();
        while i + 12 <= p.len() {
            let len = be32(&p[i..]) as usize;
            v.push(String::from_utf8_lossy(&p[i + 4..i + 8]).to_string());
            i += 12 + len;
        }
        v
    }

    fn pixels(p: &[u8]) -> Vec<u8> {
        image::load_from_memory_with_format(p, image::ImageFormat::Png).unwrap().to_rgba8().into_raw()
    }

    #[test]
    fn crc_matches_the_png_spec() {
        // The CRC of an empty IEND chunk is the well-known AE 42 60 82.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn a_plain_png_is_kept_byte_for_byte() {
        let p = png(40, 30);
        let n = normalize(&p).unwrap();
        assert_eq!((n.width, n.height), (40, 30));
        assert_eq!(n.png, p, "a PNG with only allowed chunks is unchanged");
        assert_eq!(n.sha256, sha256_hex(&p));
        assert_eq!(n.sha256.len(), 64);
    }

    #[test]
    fn text_exif_and_time_are_cut_and_the_pixels_stay() {
        let p = png(64, 48);
        let dirty = with_chunks_after_ihdr(
            &p,
            &[
                chunk(b"tEXt", b"Comment\0secret window title"),
                chunk(b"iTXt", b"Author\0\0\0en\0\0Rosson"),
                chunk(b"zTXt", b"Note\0\0x\x9c\x03\0\0\0\0\x01"),
                chunk(b"eXIf", b"MM\0*\0\0\0\x08\0\0"),
                chunk(b"tIME", &[0x07, 0xEA, 11, 10, 12, 0, 0]),
                chunk(b"sRGB", &[0]),
                chunk(b"pHYs", &[0, 0, 0x0B, 0x13, 0, 0, 0x0B, 0x13, 1]),
            ],
        );
        let n = normalize(&dirty).unwrap();
        let names = chunk_names(&n.png);
        for gone in ["tEXt", "iTXt", "zTXt", "eXIf", "tIME"] {
            assert!(!names.iter().any(|c| c == gone), "{gone} kept: {names:?}");
        }
        assert!(names.contains(&"sRGB".to_string()) && names.contains(&"pHYs".to_string()), "{names:?}");
        assert_eq!(names.first().map(String::as_str), Some("IHDR"));
        assert_eq!(names.last().map(String::as_str), Some("IEND"));
        assert!(!String::from_utf8_lossy(&n.png).contains("secret window title"));
        assert_eq!(pixels(&n.png), pixels(&p), "the cut never changes a pixel");
        assert_eq!(n.sha256, sha256_hex(&n.png), "the hash is taken after the cut");
        assert_ne!(n.sha256, sha256_hex(&dirty));
    }

    #[test]
    fn data_after_iend_is_cut() {
        let mut p = png(8, 8);
        let clean = p.clone();
        p.extend_from_slice(b"trailing bytes after the end");
        assert_eq!(normalize(&p).unwrap().png, clean);
    }

    #[test]
    fn not_a_png_and_damaged_ones_are_refused() {
        assert_eq!(normalize(b"GIF89a....").unwrap_err(), ShotError::NotPng);
        assert_eq!(normalize(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap_err(), ShotError::NotPng);
        let p = png(8, 8);
        // Cut short.
        assert!(matches!(normalize(&p[..p.len() - 20]).unwrap_err(), ShotError::Damaged(_)));
        // A flipped bit in IDAT: the CRC catches it.
        let mut bad = p.clone();
        let idat = bad.windows(4).position(|w| w == b"IDAT").unwrap();
        bad[idat + 6] ^= 0x01;
        assert!(matches!(normalize(&bad).unwrap_err(), ShotError::Damaged(ref w) if w.contains("checksum")));
        // An unknown critical chunk is never dropped silently.
        let odd = with_chunks_after_ihdr(&p, &[chunk(b"QQQQ", b"x")]);
        assert!(matches!(normalize(&odd).unwrap_err(), ShotError::Damaged(_)));
        for e in [ShotError::NotPng, ShotError::Animated, ShotError::Damaged("x".into())] {
            assert_eq!(e.code(), "bad_screenshot");
        }
    }

    #[test]
    fn too_wide_or_tall_is_refused_before_decoding() {
        // An IHDR claiming 4097 a side with almost no data.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4097u32.to_be_bytes());
        ihdr.extend_from_slice(&10u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut p = SIGNATURE.to_vec();
        p.extend(chunk(b"IHDR", &ihdr));
        p.extend(chunk(b"IDAT", &[0x78, 0x9c, 0x03, 0, 0, 0, 0, 1]));
        p.extend(chunk(b"IEND", b""));
        assert_eq!(normalize(&p).unwrap_err(), ShotError::TooWide { width: 4097, height: 10 });
        // 4096 itself is allowed (header-wise).
        assert!(normalize(&png(4096, 2)).is_ok());
    }

    #[test]
    fn a_decompression_bomb_is_refused() {
        // A tiny IDAT under a header claiming 4096×4096 RGBA at 16 bits.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4096u32.to_be_bytes());
        ihdr.extend_from_slice(&4096u32.to_be_bytes());
        ihdr.extend_from_slice(&[16, 6, 0, 0, 0]);
        let mut p = SIGNATURE.to_vec();
        p.extend(chunk(b"IHDR", &ihdr));
        p.extend(chunk(b"IDAT", &[0x78, 0x9c, 0x63, 0, 0, 0, 1, 0, 1]));
        p.extend(chunk(b"IEND", b""));
        assert!(p.len() < 100);
        let started = std::time::Instant::now();
        assert!(matches!(normalize(&p).unwrap_err(), ShotError::Unreadable(_)));
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "refused promptly");
    }

    #[test]
    fn apng_is_refused() {
        let p = png(8, 8);
        let a = with_chunks_after_ihdr(&p, &[chunk(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0])]);
        assert_eq!(normalize(&a).unwrap_err(), ShotError::Animated);
    }

    #[test]
    fn over_two_mib_is_refused_raw_and_as_base64() {
        use base64::Engine;
        let big = vec![0u8; MAX_BYTES + 1];
        assert_eq!(normalize(&big).unwrap_err(), ShotError::TooLarge(MAX_BYTES + 1));
        let b64 = base64::engine::general_purpose::STANDARD.encode(&big);
        let e = decode_base64(&b64).unwrap_err();
        assert!(matches!(e, ShotError::TooLarge(_)), "{e:?}");
        assert_eq!(e.code(), "screenshot_too_large");
    }

    #[test]
    fn base64_bare_or_as_a_png_data_url() {
        use base64::Engine;
        let p = png(4, 4);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&p);
        assert_eq!(decode_base64(&b64).unwrap(), p);
        assert_eq!(decode_base64(&format!("data:image/png;base64,{b64}")).unwrap(), p);
        assert_eq!(decode_base64(&format!("data:image/jpeg;base64,{b64}")).unwrap_err(), ShotError::NotPng);
        assert_eq!(decode_base64("not base64 at all!").unwrap_err(), ShotError::NotBase64);
    }

    #[test]
    fn drafts_are_owned_expire_and_are_capped() {
        let n = || normalize(&png(4, 4)).unwrap();
        let owner = "login:shot-test-owner";
        let d = put_draft(owner, n(), "window", Some("page"), 1_000);
        assert!(is_draft_id(&d.id), "{}", d.id);
        assert_eq!(d.meta().capture.as_deref(), Some("page"));
        assert!(get_draft(&d.id, owner, 2_000).is_some());
        assert!(get_draft(&d.id, "login:someone-else", 2_000).is_none(), "another person's draft is invisible");
        assert!(get_draft(&d.id, owner, 1_000 + DRAFT_TTL_MS).is_none(), "an hour later it is gone");
        assert!(!forget_draft(&d.id, "login:someone-else"));
        assert!(forget_draft(&d.id, owner));
        assert!(!forget_draft(&d.id, owner));
        // Per person: the oldest goes past three.
        let o2 = "login:shot-test-cap";
        let a = put_draft(o2, n(), "file", None, 10);
        let b = put_draft(o2, n(), "file", None, 11);
        let c = put_draft(o2, n(), "file", None, 12);
        let e = put_draft(o2, n(), "file", None, 13);
        assert!(get_draft(&a.id, o2, 14).is_none(), "the oldest went");
        for x in [&b, &c, &e] {
            assert!(get_draft(&x.id, o2, 14).is_some());
        }
        assert_eq!(draft_count(o2), MAX_DRAFTS_PER_OWNER);
    }

    #[test]
    fn meta_round_trips_and_a_file_has_no_capture() {
        let m = Meta { bytes: 10, width: 2, height: 3, source: "file".into(), capture: None, sha256: "ab".into() };
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(j, r#"{"bytes":10,"width":2,"height":3,"source":"file","sha256":"ab"}"#);
        assert_eq!(Meta::parse(&j), Some(m));
    }
}
