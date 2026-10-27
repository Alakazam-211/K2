//! Agent look: the color and picture that go with an agent's name.
//!
//! Both already live on the workspace row (a workspace IS an agent):
//! `projects.color` (`#rrggbb`, NOT NULL) and `projects.icon_url` (a data
//! URL, or NULL). The desktop has set them from Settings → Workspaces →
//! Agent → Identity for a long time; this module adds the checks an App
//! guest's write goes through (`agent:identity`, the app identity surface)
//! and the guest view of a stored picture.
//!
//! ## Pictures from a guest
//!
//! A guest sends `data:image/png|jpeg|webp;base64,…`. The daemon checks the
//! declared type against the bytes, decodes the picture with size limits,
//! shrinks it to at most [`MAX_IMAGE_SIDE`] on its longer side and encodes
//! it again (JPEG stays JPEG, PNG and WebP become PNG). What is stored is
//! always the daemon's own encoding: metadata (EXIF, GPS) is dropped and no
//! byte the guest sent is ever served back as-is. SVG and every other type
//! are refused. Nothing in the picture is ever run.
//!
//! ## Pictures to a guest
//!
//! [`guest_image_view`] passes a stored picture to an App only when it is
//! a PNG, JPEG, WebP or GIF data URL whose bytes match its type and are at
//! most [`MAX_WIRE_IMAGE_BYTES`]. An SVG icon the owner picked (or that
//! K2 found in the repo) is never sent to an App: the App sees `null`.

use std::io::Cursor;

use base64::Engine as _;

/// Largest picture a guest may send (decoded bytes).
pub const MAX_INPUT_IMAGE_BYTES: usize = 2 * 1024 * 1024;
/// Largest picture side the decoder accepts (pixels).
pub const MAX_SOURCE_SIDE: u32 = 8192;
/// The stored picture's longer side is at most this (pixels).
pub const MAX_IMAGE_SIDE: u32 = 256;
/// Largest stored picture an App is sent (decoded bytes).
pub const MAX_WIRE_IMAGE_BYTES: usize = 256 * 1024;
/// JPEG quality for a re-encoded JPEG.
const JPEG_QUALITY: u8 = 85;

/// A refusal with a stable wire code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookError {
    /// Not `#rgb` / `#rrggbb`.
    BadColor(String),
    /// Not `data:image/<type>;base64,<bytes>`.
    NotDataUrl,
    /// A type the daemon does not take (SVG, GIF, …).
    TypeRefused(String),
    /// The bytes are not the declared type.
    TypeMismatch(String),
    /// Decoded bytes over [`MAX_INPUT_IMAGE_BYTES`].
    TooLarge(usize),
    /// The picture could not be decoded, or is too big in pixels.
    BadImage(String),
}

impl LookError {
    pub fn code(&self) -> &'static str {
        match self {
            LookError::BadColor(_) => "bad_color",
            LookError::NotDataUrl => "not_data_url",
            LookError::TypeRefused(_) => "type_refused",
            LookError::TypeMismatch(_) => "type_mismatch",
            LookError::TooLarge(_) => "too_large",
            LookError::BadImage(_) => "bad_image",
        }
    }
}

impl std::fmt::Display for LookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookError::BadColor(raw) => {
                write!(f, "color must be #rgb or #rrggbb (got {raw:?})")
            }
            LookError::NotDataUrl => {
                write!(f, "image must be a data:image/png|jpeg|webp;base64 URL")
            }
            LookError::TypeRefused(t) => {
                write!(f, "image type {t} is not accepted (png, jpeg or webp)")
            }
            LookError::TypeMismatch(t) => write!(f, "image bytes are not {t}"),
            LookError::TooLarge(n) => write!(
                f,
                "image is {n} bytes; the limit is {MAX_INPUT_IMAGE_BYTES} bytes"
            ),
            LookError::BadImage(e) => write!(f, "image could not be read: {e}"),
        }
    }
}

/// `#rgb` / `#rrggbb` (any case, `#` optional) → lowercase `#rrggbb`.
/// Nothing else: no names, no `rgb()`, no alpha.
pub fn normalize_color(raw: &str) -> Result<String, LookError> {
    let t = raw.trim();
    let hex = t.strip_prefix('#').unwrap_or(t);
    let ok = |s: &str| s.chars().all(|c| c.is_ascii_hexdigit());
    match hex.len() {
        6 if ok(hex) => Ok(format!("#{}", hex.to_ascii_lowercase())),
        3 if ok(hex) => {
            let mut out = String::from("#");
            for c in hex.chars() {
                let c = c.to_ascii_lowercase();
                out.push(c);
                out.push(c);
            }
            Ok(out)
        }
        _ => Err(LookError::BadColor(t.chars().take(32).collect())),
    }
}

/// The raster types the magic bytes say these are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sniffed {
    Png,
    Jpeg,
    Webp,
    Gif,
}

fn sniff(bytes: &[u8]) -> Option<Sniffed> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Sniffed::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Sniffed::Jpeg)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Sniffed::Webp)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Sniffed::Gif)
    } else {
        None
    }
}

/// Split `data:<mime>;base64,<payload>` → (lowercase mime, payload).
fn split_data_url(raw: &str) -> Option<(String, &str)> {
    let rest = raw.trim().strip_prefix("data:")?;
    let (head, payload) = rest.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    Some((mime.trim().to_ascii_lowercase(), payload))
}

fn decode_b64(payload: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Check, decode, shrink and re-encode a guest's picture. Returns the
/// data URL to store (`data:image/png|jpeg;base64,…`).
pub fn normalize_image_data_url(raw: &str) -> Result<String, LookError> {
    let (mime, payload) = split_data_url(raw).ok_or(LookError::NotDataUrl)?;
    let (want, format) = match mime.as_str() {
        "image/png" => (Sniffed::Png, image::ImageFormat::Png),
        "image/jpeg" | "image/jpg" => (Sniffed::Jpeg, image::ImageFormat::Jpeg),
        "image/webp" => (Sniffed::Webp, image::ImageFormat::WebP),
        other => return Err(LookError::TypeRefused(other.chars().take(64).collect())),
    };
    // A base64 payload is 4/3 of the bytes; refuse before decoding.
    if payload.len() > MAX_INPUT_IMAGE_BYTES / 3 * 4 + 8 {
        return Err(LookError::TooLarge(payload.len() / 4 * 3));
    }
    let bytes = decode_b64(payload).ok_or(LookError::NotDataUrl)?;
    if bytes.len() > MAX_INPUT_IMAGE_BYTES {
        return Err(LookError::TooLarge(bytes.len()));
    }
    if sniff(&bytes) != Some(want) {
        return Err(LookError::TypeMismatch(mime));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| LookError::BadImage(e.to_string()))?;
    if img.width() == 0 || img.height() == 0 {
        return Err(LookError::BadImage("empty picture".into()));
    }
    let img = if img.width() > MAX_IMAGE_SIDE || img.height() > MAX_IMAGE_SIDE {
        img.resize(
            MAX_IMAGE_SIDE,
            MAX_IMAGE_SIDE,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };
    let mut out = Cursor::new(Vec::new());
    let out_mime = if want == Sniffed::Jpeg {
        let rgb = img.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
            .encode_image(&rgb)
            .map_err(|e| LookError::BadImage(e.to_string()))?;
        "image/jpeg"
    } else {
        img.write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| LookError::BadImage(e.to_string()))?;
        "image/png"
    };
    Ok(format!("data:{out_mime};base64,{}", b64(&out.into_inner())))
}

/// Largest side a profile picture's decoder accepts (vs-live U17 a).
pub const PROFILE_SOURCE_SIDE: u32 = 4096;
/// The decoder's allocation cap for a profile picture.
pub const PROFILE_MAX_ALLOC: u64 = 64 * 1024 * 1024;
/// A stored profile picture is at most this many bytes (always JPEG).
pub const PROFILE_MAX_BYTES: usize = 64 * 1024;

/// Check, decode, turn upright, shrink and re-encode an App user's own
/// picture (app-user-profile §3.4, vs-live U17). Like
/// [`normalize_image_data_url`] (declared type must match the bytes;
/// PNG/JPEG/WebP only; ≤ [`MAX_INPUT_IMAGE_BYTES`] in), plus:
/// - a smaller decoder budget ([`PROFILE_SOURCE_SIDE`],
///   [`PROFILE_MAX_ALLOC`]) and at most two decodes at once box-wide
///   ([`crate::skin::guard::decode_slot`]);
/// - the EXIF orientation is applied before the EXIF goes, so a phone
///   selfie comes out upright;
/// - always JPEG out (alpha flattened onto white), quality and size
///   stepped down until it is ≤ [`PROFILE_MAX_BYTES`].
///
/// Returns the JPEG bytes (K2's own encoding; nothing the person sent).
pub fn normalize_profile_image(raw: &str) -> Result<Vec<u8>, LookError> {
    use image::ImageDecoder as _;
    let (mime, payload) = split_data_url(raw).ok_or(LookError::NotDataUrl)?;
    let (want, format) = match mime.as_str() {
        "image/png" => (Sniffed::Png, image::ImageFormat::Png),
        "image/jpeg" | "image/jpg" => (Sniffed::Jpeg, image::ImageFormat::Jpeg),
        "image/webp" => (Sniffed::Webp, image::ImageFormat::WebP),
        other => return Err(LookError::TypeRefused(other.chars().take(64).collect())),
    };
    if payload.len() > MAX_INPUT_IMAGE_BYTES / 3 * 4 + 8 {
        return Err(LookError::TooLarge(payload.len() / 4 * 3));
    }
    let bytes = decode_b64(payload).ok_or(LookError::NotDataUrl)?;
    if bytes.len() > MAX_INPUT_IMAGE_BYTES {
        return Err(LookError::TooLarge(bytes.len()));
    }
    if sniff(&bytes) != Some(want) {
        return Err(LookError::TypeMismatch(mime));
    }
    let _slot = crate::skin::guard::decode_slot();
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(PROFILE_SOURCE_SIDE);
    limits.max_image_height = Some(PROFILE_SOURCE_SIDE);
    limits.max_alloc = Some(PROFILE_MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| LookError::BadImage(e.to_string()))?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| LookError::BadImage(e.to_string()))?;
    if img.width() == 0 || img.height() == 0 {
        return Err(LookError::BadImage("empty picture".into()));
    }
    img.apply_orientation(orientation);
    for side in [MAX_IMAGE_SIDE, 192, 128, 96] {
        let sized = if img.width() > side || img.height() > side {
            img.resize(side, side, image::imageops::FilterType::Lanczos3)
        } else {
            img.clone()
        };
        let rgb = flatten_on_white(&sized);
        for quality in [85u8, 75, 65, 55, 45] {
            let mut out = Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .encode_image(&rgb)
                .map_err(|e| LookError::BadImage(e.to_string()))?;
            let out = out.into_inner();
            if out.len() <= PROFILE_MAX_BYTES {
                return Ok(out);
            }
        }
    }
    Err(LookError::BadImage("the picture would not fit in 64 KiB".into()))
}

/// Alpha composited onto white (JPEG has no alpha).
fn flatten_on_white(img: &image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        let mix = |v: u8| ((u32::from(v) * a + 255 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    rgb
}

/// The stored picture as an App may see it, or `None` (no picture, an SVG
/// or other type, bytes that don't match, or over [`MAX_WIRE_IMAGE_BYTES`]).
pub fn guest_image_view(stored: Option<&str>) -> Option<String> {
    let raw = stored?.trim();
    let (mime, payload) = split_data_url(raw)?;
    let want = match mime.as_str() {
        "image/png" => Sniffed::Png,
        "image/jpeg" | "image/jpg" => Sniffed::Jpeg,
        "image/webp" => Sniffed::Webp,
        "image/gif" => Sniffed::Gif,
        _ => return None,
    };
    if payload.len() > MAX_WIRE_IMAGE_BYTES / 3 * 4 + 8 {
        return None;
    }
    let bytes = decode_b64(payload)?;
    if bytes.len() > MAX_WIRE_IMAGE_BYTES || sniff(&bytes) != Some(want) {
        return None;
    }
    Some(raw.to_string())
}

/// One agent's look as stored: `(color, icon_url)` for a workspace id.
pub fn stored_look(project_id: &str) -> Option<(String, Option<String>)> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT color, icon_url FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .ok()
}

/// Write `projects.color` (already normalized). Returns the old color.
pub fn set_color(project_id: &str, color: &str) -> Result<String, String> {
    let color = normalize_color(color).map_err(|e| e.to_string())?;
    let db = crate::db::shared();
    let conn = db.lock();
    let old: String = conn
        .query_row(
            "SELECT color FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )
        .map_err(|_| format!("workspace not found: {project_id}"))?;
    conn.execute(
        "UPDATE projects SET color = ?1 WHERE id = ?2",
        rusqlite::params![color, project_id],
    )
    .map_err(|e| format!("failed to update projects.color: {e}"))?;
    Ok(old)
}

/// Write `projects.icon_url` (`None` clears it). The caller has already
/// normalized a guest picture. Returns the old value.
pub fn set_image(project_id: &str, data_url: Option<&str>) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let old: Option<String> = conn
        .query_row(
            "SELECT icon_url FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get(0),
        )
        .map_err(|_| format!("workspace not found: {project_id}"))?;
    conn.execute(
        "UPDATE projects SET icon_url = ?1 WHERE id = ?2",
        rusqlite::params![data_url, project_id],
    )
    .map_err(|e| format!("failed to update projects.icon_url: {e}"))?;
    Ok(old)
}

/// A short, stable fingerprint of a stored picture for audit lines
/// (`sha256:<12 hex>`), or `none`. Never the picture itself.
pub fn image_fingerprint(data_url: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    match data_url.map(str::trim).filter(|s| !s.is_empty()) {
        None => "none".to_string(),
        Some(s) => {
            let digest = Sha256::digest(s.as_bytes());
            let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
            format!("sha256:{hex}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_data_url(w: u32, h: u32) -> String {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode png");
        format!("data:image/png;base64,{}", b64(&out.into_inner()))
    }

    fn jpeg_data_url(w: u32, h: u32) -> String {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 10, 30]));
        let mut out = Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&img)
            .expect("encode jpeg");
        format!("data:image/jpeg;base64,{}", b64(&out.into_inner()))
    }

    fn webp_data_url(w: u32, h: u32) -> String {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::WebP)
            .expect("encode webp");
        format!("data:image/webp;base64,{}", b64(&out.into_inner()))
    }

    fn decode_stored(url: &str) -> (String, image::DynamicImage) {
        let (mime, payload) = split_data_url(url).expect("data url");
        let bytes = decode_b64(payload).expect("b64");
        (mime, image::load_from_memory(&bytes).expect("decodes"))
    }

    #[test]
    fn colors_normalize_or_refuse() {
        assert_eq!(normalize_color("#AABBCC").unwrap(), "#aabbcc");
        assert_eq!(normalize_color("aabbcc").unwrap(), "#aabbcc");
        assert_eq!(normalize_color(" #F0a ").unwrap(), "#ff00aa");
        for bad in ["", "#", "red", "#abcd", "#gggggg", "rgb(1,2,3)", "#aabbccdd", "#aabbc"] {
            let e = normalize_color(bad).unwrap_err();
            assert_eq!(e.code(), "bad_color", "{bad:?}");
        }
    }

    #[test]
    fn png_is_reencoded_and_shrunk() {
        let out = normalize_image_data_url(&png_data_url(600, 300)).expect("png ok");
        let (mime, img) = decode_stored(&out);
        assert_eq!(mime, "image/png");
        assert_eq!((img.width(), img.height()), (256, 128));
        // Small pictures keep their size.
        let out = normalize_image_data_url(&png_data_url(40, 40)).expect("small ok");
        let (_, img) = decode_stored(&out);
        assert_eq!((img.width(), img.height()), (40, 40));
    }

    #[test]
    fn jpeg_stays_jpeg_and_webp_becomes_png() {
        let out = normalize_image_data_url(&jpeg_data_url(300, 300)).expect("jpeg ok");
        let (mime, img) = decode_stored(&out);
        assert_eq!(mime, "image/jpeg");
        assert_eq!((img.width(), img.height()), (256, 256));
        let out = normalize_image_data_url(&webp_data_url(64, 32)).expect("webp ok");
        let (mime, img) = decode_stored(&out);
        assert_eq!(mime, "image/png");
        assert_eq!((img.width(), img.height()), (64, 32));
    }

    #[test]
    fn refusals_have_stable_codes() {
        let svg = format!(
            "data:image/svg+xml;base64,{}",
            b64(b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>")
        );
        assert_eq!(normalize_image_data_url(&svg).unwrap_err().code(), "type_refused");
        let gif = format!("data:image/gif;base64,{}", b64(b"GIF89a\x01\x00\x01\x00"));
        assert_eq!(normalize_image_data_url(&gif).unwrap_err().code(), "type_refused");
        assert_eq!(
            normalize_image_data_url("https://example.com/a.png").unwrap_err().code(),
            "not_data_url"
        );
        assert_eq!(
            normalize_image_data_url("data:image/png,rawbytes").unwrap_err().code(),
            "not_data_url"
        );
        assert_eq!(
            normalize_image_data_url("data:image/png;base64,@@@").unwrap_err().code(),
            "not_data_url"
        );
        // A JPEG labelled PNG.
        let jpeg = jpeg_data_url(8, 8);
        let mislabelled = jpeg.replacen("image/jpeg", "image/png", 1);
        assert_eq!(normalize_image_data_url(&mislabelled).unwrap_err().code(), "type_mismatch");
        // HTML labelled PNG.
        let html = format!("data:image/png;base64,{}", b64(b"<html><script>x</script></html>"));
        assert_eq!(normalize_image_data_url(&html).unwrap_err().code(), "type_mismatch");
        // Right magic, broken body.
        let broken = format!(
            "data:image/png;base64,{}",
            b64(b"\x89PNG\r\n\x1a\nnot really a png at all")
        );
        assert_eq!(normalize_image_data_url(&broken).unwrap_err().code(), "bad_image");
        // Over the byte cap.
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_INPUT_IMAGE_BYTES + 1, 0);
        let big = format!("data:image/png;base64,{}", b64(&big));
        assert_eq!(normalize_image_data_url(&big).unwrap_err().code(), "too_large");
    }

    #[test]
    fn guest_view_hides_svg_and_oversize() {
        let png = png_data_url(4, 4);
        assert_eq!(guest_image_view(Some(&png)).as_deref(), Some(png.as_str()));
        let svg = format!("data:image/svg+xml;base64,{}", b64(b"<svg/>"));
        assert_eq!(guest_image_view(Some(&svg)), None);
        assert_eq!(guest_image_view(None), None);
        assert_eq!(guest_image_view(Some("")), None);
        let lie = format!("data:image/png;base64,{}", b64(b"<svg/>"));
        assert_eq!(guest_image_view(Some(&lie)), None);
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_WIRE_IMAGE_BYTES + 1, 0);
        let big = format!("data:image/png;base64,{}", b64(&big));
        assert_eq!(guest_image_view(Some(&big)), None);
    }

    #[test]
    fn fingerprint_never_carries_the_picture() {
        assert_eq!(image_fingerprint(None), "none");
        assert_eq!(image_fingerprint(Some("  ")), "none");
        let f = image_fingerprint(Some("data:image/png;base64,AAAA"));
        assert!(f.starts_with("sha256:") && f.len() == "sha256:".len() + 12, "{f}");
    }
}
