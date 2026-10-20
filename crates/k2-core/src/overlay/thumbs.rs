//! Thread image thumbnails (thread-images-v1, 0.46.3; S3 of
//! `.k2/prds/eval-thread-file-objects-v1.md`, widened by Rosson 2026-10-18
//! to every image type).
//!
//! - **Raster** (PNG, JPEG, GIF's first frame, WebP, BMP, TIFF): decoded
//!   by the `image` crate under hard limits ([`MAX_PIXELS`],
//!   [`MAX_ALLOC`]), scaled to fit `px`, encoded as JPEG (PNG when the
//!   source has transparency).
//! - **HEIC/HEIF**: no Rust decoder ships here; macOS `sips` (always there)
//!   or `heif-convert` (libheif, Linux) converts to JPEG first, under a
//!   timeout. With neither, [`ThumbError::Unsupported`].
//! - **SVG**: never rasterized and never passed through raw. It is
//!   rewritten by an allowlist ([`sanitize_svg`]): no script, no event
//!   handler, no foreignObject, no outside reference, no DOCTYPE. Clients
//!   still show it only as an image (`<img>`), where browsers never run
//!   script; the rewrite is the second wall.
//!
//! Thumbnails are cached under `~/.k2/cache/thread-thumbs`, keyed by the
//! file's identity (workspace, path, size, mtime) and `px`; the cache is
//! pruned oldest-first past [`CACHE_MAX_BYTES`].

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader};
use sha2::{Digest, Sha256};

/// The sizes a client may ask for (the longest side).
pub const PX_SMALL: u32 = 256;
pub const PX_LARGE: u32 = 512;
/// Largest source image decoded (pixels): 40 MP.
pub const MAX_PIXELS: u64 = 40_000_000;
/// Largest decode allocation.
pub const MAX_ALLOC: u64 = 512 * 1024 * 1024;
/// Largest source file read for a thumbnail.
pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
/// Largest SVG sanitized.
pub const MAX_SVG_BYTES: u64 = 2 * 1024 * 1024;
/// HEIC converter timeout.
pub const CONVERT_TIMEOUT: Duration = Duration::from_secs(10);
/// The thumbnail cache's size cap (pruned oldest-first to 3/4 of it).
pub const CACHE_MAX_BYTES: u64 = 200 * 1024 * 1024;

/// One thumbnail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// An SVG rewritten by [`sanitize_svg`].
    pub sanitized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbError {
    /// Not an image, or an image this box can't decode (HEIC with no
    /// converter). The client shows the file chip.
    Unsupported(String),
    /// Over [`MAX_SOURCE_BYTES`], [`MAX_SVG_BYTES`] or [`MAX_PIXELS`].
    TooLarge(String),
    /// The bytes don't decode (a broken or mislabelled file), or the SVG
    /// isn't well-formed XML.
    Bad(String),
    Io(String),
}

impl ThumbError {
    pub fn code(&self) -> &'static str {
        match self {
            ThumbError::Unsupported(_) => "no_thumbnail",
            ThumbError::TooLarge(_) => "preview_too_large",
            ThumbError::Bad(_) => "no_thumbnail",
            ThumbError::Io(_) => "io",
        }
    }

    pub fn hint(&self) -> String {
        match self {
            ThumbError::Unsupported(m) | ThumbError::TooLarge(m) | ThumbError::Bad(m) | ThumbError::Io(m) => m.clone(),
        }
    }
}

/// Is this content type one we thumbnail?
pub fn is_image_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/tiff"
            | "image/heic"
            | "image/heif"
            | "image/svg+xml"
    )
}

/// `px` as asked (`None` = [`PX_SMALL`]); only 256 or 512.
pub fn parse_px(raw: Option<&str>) -> Result<u32, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(PX_SMALL),
        Some("256") => Ok(PX_SMALL),
        Some("512") => Ok(PX_LARGE),
        Some(other) => Err(format!("px is 256 or 512, got {other}")),
    }
}

/// Make the thumbnail of the file at `real` (already jailed by the
/// caller) with content type `mime`.
pub fn make(real: &Path, mime: &str, px: u32) -> Result<Thumb, ThumbError> {
    let size = std::fs::metadata(real).map_err(|e| ThumbError::Io(e.to_string()))?.len();
    match mime {
        "image/svg+xml" => {
            if size > MAX_SVG_BYTES {
                return Err(ThumbError::TooLarge(format!("SVG previews stop at {} MB", MAX_SVG_BYTES / (1024 * 1024))));
            }
            let raw = std::fs::read(real).map_err(|e| ThumbError::Io(e.to_string()))?;
            let clean = sanitize_svg(&raw)?;
            let (w, h) = svg_size(&clean);
            Ok(Thumb { mime: "image/svg+xml", bytes: clean.into_bytes(), width: w, height: h, sanitized: true })
        }
        "image/heic" | "image/heif" => {
            if size > MAX_SOURCE_BYTES {
                return Err(too_large());
            }
            let jpeg = heic_to_jpeg(real, px)?;
            raster_from_bytes(&jpeg, Some(ImageFormat::Jpeg), px)
        }
        m if is_image_mime(m) => {
            if size > MAX_SOURCE_BYTES {
                return Err(too_large());
            }
            let bytes = std::fs::read(real).map_err(|e| ThumbError::Io(e.to_string()))?;
            raster_from_bytes(&bytes, None, px)
        }
        other => Err(ThumbError::Unsupported(format!("{other} has no thumbnail; open the file"))),
    }
}

fn too_large() -> ThumbError {
    ThumbError::TooLarge(format!("thumbnails stop at {} MB; open the file", MAX_SOURCE_BYTES / (1024 * 1024)))
}

/// Decode (format sniffed from the bytes, never trusted from the name),
/// scale to fit `px`, encode.
pub fn raster_from_bytes(bytes: &[u8], format: Option<ImageFormat>, px: u32) -> Result<Thumb, ThumbError> {
    let fmt = match format {
        Some(f) => f,
        None => image::guess_format(bytes)
            .map_err(|_| ThumbError::Unsupported("not an image this box can read".into()))?,
    };
    if !matches!(
        fmt,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Bmp | ImageFormat::Tiff
    ) {
        return Err(ThumbError::Unsupported(format!("{fmt:?} has no thumbnail")));
    }
    // The header's size first, so a decompression bomb is refused before
    // any pixel is decoded.
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), fmt)
        .into_dimensions()
        .map_err(|e| ThumbError::Bad(format!("the image doesn't decode: {e}")))?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(ThumbError::TooLarge(format!("{w}x{h} is over the {} MP decode limit", MAX_PIXELS / 1_000_000)));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), fmt);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| ThumbError::Bad(format!("the image doesn't decode: {e}")))?;
    let (sw, sh) = img.dimensions();
    let small = if sw > px || sh > px { img.thumbnail(px, px) } else { img };
    encode(small)
}

fn encode(img: DynamicImage) -> Result<Thumb, ThumbError> {
    let (width, height) = img.dimensions();
    let mut out = Vec::new();
    let alpha = img.color().has_alpha();
    let mime = if alpha {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| ThumbError::Bad(e.to_string()))?;
        "image/png"
    } else {
        let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82);
        rgb.write_with_encoder(enc).map_err(|e| ThumbError::Bad(e.to_string()))?;
        "image/jpeg"
    };
    Ok(Thumb { mime, bytes: out, width, height, sanitized: false })
}

/// The HEIC/HEIF converter this box has, if any.
pub fn heic_converter() -> Option<&'static str> {
    if cfg!(target_os = "macos") && Path::new("/usr/bin/sips").exists() {
        return Some("sips");
    }
    for dir in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
        if Path::new(dir).join("heif-convert").exists() {
            return Some("heif-convert");
        }
    }
    None
}

fn heif_convert_path() -> Option<PathBuf> {
    ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"]
        .iter()
        .map(|d| Path::new(d).join("heif-convert"))
        .find(|p| p.exists())
}

/// HEIC → JPEG bytes through the box's converter (a temp file, removed).
fn heic_to_jpeg(real: &Path, px: u32) -> Result<Vec<u8>, ThumbError> {
    let tmp = std::env::temp_dir().join(format!("k2-heic-{}.jpg", uuid::Uuid::new_v4()));
    let mut cmd = match heic_converter() {
        Some("sips") => {
            let mut c = Command::new("/usr/bin/sips");
            // -Z scales the longest side; sips never upsizes past the source.
            c.args(["-s", "format", "jpeg", "-Z", &px.to_string()]).arg(real).arg("--out").arg(&tmp);
            c
        }
        Some(_) => {
            let Some(bin) = heif_convert_path() else {
                return Err(ThumbError::Unsupported("HEIC needs heif-convert (libheif) on this server".into()));
            };
            let mut c = Command::new(bin);
            c.arg(real).arg(&tmp);
            c
        }
        None => {
            return Err(ThumbError::Unsupported(
                "this server can't read HEIC (no sips or heif-convert); open the file".into(),
            ))
        }
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| ThumbError::Io(format!("the HEIC converter didn't start: {e}")))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() > CONVERT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&tmp);
                return Err(ThumbError::Bad("the HEIC converter took too long".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(ThumbError::Io(e.to_string())),
        }
    };
    let out = std::fs::read(&tmp);
    let _ = std::fs::remove_file(&tmp);
    if !status.success() {
        return Err(ThumbError::Bad("the HEIC file doesn't convert".into()));
    }
    out.map_err(|e| ThumbError::Io(e.to_string()))
}

// ── SVG ──────────────────────────────────────────────────────────────

/// SVG elements kept (local names, lowercase). Everything else is dropped
/// with its children; `a` is unwrapped (its children kept).
const SVG_ELEMENTS: &[&str] = &[
    "svg", "g", "defs", "symbol", "use", "title", "desc", "path", "rect", "circle", "ellipse", "line", "polyline",
    "polygon", "text", "tspan", "textpath", "lineargradient", "radialgradient", "stop", "pattern", "clippath", "mask",
    "marker", "filter", "image", "style", "switch", "view", "feblend", "fecolormatrix", "fecomponenttransfer",
    "fecomposite", "feconvolvematrix", "fediffuselighting", "fedisplacementmap", "fedistantlight", "fedropshadow",
    "feflood", "fefunca", "fefuncb", "fefuncg", "fefuncr", "fegaussianblur", "feimage", "femerge", "femergenode",
    "femorphology", "feoffset", "fepointlight", "fespecularlighting", "fespotlight", "fetile", "feturbulence",
];

/// Is an attribute value a reference we keep? `#id` always; a raster
/// `data:` URI only on `<image>`/`<feImage>`.
fn href_ok(value: &str, raster_ok: bool) -> bool {
    let v = value.trim();
    if v.starts_with('#') {
        return true;
    }
    let lower = v.to_ascii_lowercase();
    raster_ok
        && ["data:image/png;", "data:image/jpeg;", "data:image/gif;", "data:image/webp;"]
            .iter()
            .any(|p| lower.starts_with(p))
}

/// Does CSS text (a `style` attribute or element) reach outside or run
/// anything? `url(#id)` is fine; any other `url(`, `@import`,
/// `expression(`, `javascript:` or a `behavior`/`-moz-binding` is not.
fn css_bad(css: &str) -> bool {
    let lower: String = css.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if lower.contains("@import")
        || lower.contains("expression(")
        || lower.contains("javascript:")
        || lower.contains("behavior:")
        || lower.contains("-moz-binding")
        || lower.contains('\\')
    {
        return true;
    }
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("url(") {
        let after = rest[i + 4..].trim_start_matches(['\'', '"']);
        if !after.starts_with('#') {
            return true;
        }
        rest = &rest[i + 4..];
    }
    false
}

fn local_lower(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    let local = s.rsplit(':').next().unwrap_or(&s);
    local.to_ascii_lowercase()
}

/// Rewrite an SVG by the allowlist above. A document that isn't
/// well-formed XML, holds a DOCTYPE (entities) or has no `<svg>` root is
/// refused, never passed through.
pub fn sanitize_svg(raw: &[u8]) -> Result<String, ThumbError> {
    use quick_xml::events::{BytesStart, Event};
    use quick_xml::{Reader, Writer};

    let text = std::str::from_utf8(raw).map_err(|_| ThumbError::Bad("the SVG isn't UTF-8".into()))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    // Depth inside a dropped element (its whole subtree goes).
    let mut skip_depth: usize = 0;
    // Open elements we kept or unwrapped (true = written, so its end is written).
    let mut stack: Vec<(String, bool)> = Vec::new();
    let mut saw_root = false;
    let mut in_style = false;

    let filter_start = |e: &BytesStart<'_>, name: &str| -> Result<BytesStart<'static>, ThumbError> {
        let raw_name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        let mut out = BytesStart::new(raw_name);
        let raster_ok = matches!(name, "image" | "feimage");
        for a in e.attributes() {
            let a = a.map_err(|e| ThumbError::Bad(format!("bad SVG attribute: {e}")))?;
            let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            let lkey = key.to_ascii_lowercase();
            let value = a
                .unescape_value()
                .map_err(|e| ThumbError::Bad(format!("bad SVG attribute value: {e}")))?
                .into_owned();
            let lval = value.to_ascii_lowercase();
            let local = lkey.rsplit(':').next().unwrap_or(&lkey).to_string();
            // Event handlers, and any namespace but xlink/xml/xmlns.
            if local.starts_with("on") {
                continue;
            }
            if let Some((ns, _)) = lkey.split_once(':') {
                if !matches!(ns, "xlink" | "xml" | "xmlns") {
                    continue;
                }
            }
            if local == "href" || lkey == "xlink:href" {
                if !href_ok(&value, raster_ok) {
                    continue;
                }
            } else if lkey == "style" {
                if css_bad(&value) {
                    continue;
                }
            } else if lval.contains("javascript:") || lval.contains("data:") || (lval.contains("url(") && css_bad(&value)) {
                continue;
            }
            out.push_attribute((key.as_str(), value.as_str()));
        }
        Ok(out)
    };

    loop {
        let ev = reader.read_event().map_err(|e| ThumbError::Bad(format!("the SVG isn't well-formed: {e}")))?;
        match ev {
            Event::Eof => break,
            Event::DocType(_) => return Err(ThumbError::Bad("an SVG with a DOCTYPE isn't previewed (entities)".into())),
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Start(e) => {
                let name = local_lower(e.name().as_ref());
                if skip_depth > 0 {
                    skip_depth += 1;
                    continue;
                }
                if !saw_root {
                    if name != "svg" {
                        return Err(ThumbError::Bad("not an SVG document".into()));
                    }
                    saw_root = true;
                }
                if name == "a" {
                    stack.push((name, false));
                    continue;
                }
                if !SVG_ELEMENTS.contains(&name.as_str()) {
                    skip_depth = 1;
                    continue;
                }
                let start = filter_start(&e, &name)?;
                writer.write_event(Event::Start(start)).map_err(|e| ThumbError::Io(e.to_string()))?;
                in_style = name == "style";
                stack.push((name, true));
            }
            Event::Empty(e) => {
                let name = local_lower(e.name().as_ref());
                if skip_depth > 0 || name == "a" {
                    continue;
                }
                if !saw_root {
                    if name != "svg" {
                        return Err(ThumbError::Bad("not an SVG document".into()));
                    }
                    saw_root = true;
                }
                if !SVG_ELEMENTS.contains(&name.as_str()) {
                    continue;
                }
                let start = filter_start(&e, &name)?;
                writer.write_event(Event::Empty(start)).map_err(|e| ThumbError::Io(e.to_string()))?;
            }
            Event::End(e) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                in_style = false;
                match stack.pop() {
                    Some((_, true)) => {
                        writer.write_event(Event::End(e)).map_err(|e| ThumbError::Io(e.to_string()))?;
                    }
                    Some((_, false)) => {}
                    None => return Err(ThumbError::Bad("the SVG isn't well-formed".into())),
                }
            }
            Event::Text(t) => {
                if skip_depth > 0 {
                    continue;
                }
                if in_style {
                    let css = t.decode().map_err(|e| ThumbError::Bad(e.to_string()))?;
                    if css_bad(&css) {
                        continue;
                    }
                }
                writer.write_event(Event::Text(t)).map_err(|e| ThumbError::Io(e.to_string()))?;
            }
            Event::CData(c) => {
                if skip_depth > 0 {
                    continue;
                }
                let s = String::from_utf8_lossy(&c).into_owned();
                if in_style && css_bad(&s) {
                    continue;
                }
                writer.write_event(Event::CData(c)).map_err(|e| ThumbError::Io(e.to_string()))?;
            }
            Event::GeneralRef(r) => {
                if skip_depth > 0 {
                    continue;
                }
                // Only the five XML entities and character references.
                let name = String::from_utf8_lossy(&r).into_owned();
                let ok = matches!(name.as_str(), "lt" | "gt" | "amp" | "quot" | "apos") || name.starts_with('#');
                if !ok {
                    return Err(ThumbError::Bad("the SVG uses an entity".into()));
                }
                writer.write_event(Event::GeneralRef(r)).map_err(|e| ThumbError::Io(e.to_string()))?;
            }
        }
    }
    if !saw_root {
        return Err(ThumbError::Bad("not an SVG document".into()));
    }
    String::from_utf8(writer.into_inner().into_inner()).map_err(|e| ThumbError::Bad(e.to_string()))
}

/// The SVG's `width`/`height` (or its viewBox), for the client's layout;
/// `(0, 0)` when it says nothing usable.
fn svg_size(svg: &str) -> (u32, u32) {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(svg);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local_lower(e.name().as_ref()) == "svg" => {
                let mut w = None;
                let mut h = None;
                let mut vb = None;
                for a in e.attributes().flatten() {
                    let v = a.unescape_value().map(|c| c.into_owned()).unwrap_or_default();
                    match a.key.as_ref() {
                        b"width" => w = num_prefix(&v),
                        b"height" => h = num_prefix(&v),
                        b"viewBox" => vb = Some(v),
                        _ => {}
                    }
                }
                if let (Some(w), Some(h)) = (w, h) {
                    return (w, h);
                }
                if let Some(vb) = vb {
                    let n: Vec<f64> = vb.split([' ', ',']).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
                    if n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 {
                        return (n[2].round() as u32, n[3].round() as u32);
                    }
                }
                return (0, 0);
            }
            Ok(Event::Eof) | Err(_) => return (0, 0),
            _ => {}
        }
    }
}

fn num_prefix(s: &str) -> Option<u32> {
    let n: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    n.parse::<f64>().ok().filter(|v| *v > 0.0).map(|v| v.round() as u32)
}

// ── cache ────────────────────────────────────────────────────────────

/// The thumbnail cache folder.
pub fn cache_dir() -> PathBuf {
    crate::paths::k2_home().join("cache").join("thread-thumbs")
}

/// A thumbnail's cache key: the file's identity now and the size asked.
pub fn cache_key(project_id: &str, rel: &str, size: u64, modified_ms: i64, px: u32) -> String {
    let mut h = Sha256::new();
    h.update(project_id.as_bytes());
    h.update([0]);
    h.update(rel.as_bytes());
    h.update([0]);
    h.update(size.to_le_bytes());
    h.update(modified_ms.to_le_bytes());
    h.update(px.to_le_bytes());
    let d = h.finalize();
    d.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

fn ext_of(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/svg+xml" => Some("svg"),
        _ => None,
    }
}

fn mime_of_ext(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// A cached thumbnail (`dir` = [`cache_dir`] in production). Dimensions
/// ride the file name: `<key>.<w>x<h>.<ext>`.
pub fn cache_get(dir: &Path, key: &str) -> Option<Thumb> {
    let rd = std::fs::read_dir(dir).ok()?;
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(key).and_then(|r| r.strip_prefix('.')) else { continue };
        let mut parts = rest.split('.');
        let (Some(dims), Some(ext)) = (parts.next(), parts.next()) else { continue };
        let (w, h) = dims.split_once('x')?;
        let mime = mime_of_ext(ext)?;
        let bytes = std::fs::read(ent.path()).ok()?;
        return Some(Thumb {
            mime,
            bytes,
            width: w.parse().ok()?,
            height: h.parse().ok()?,
            sanitized: mime == "image/svg+xml",
        });
    }
    None
}

/// Store a thumbnail, then prune the folder past `max_bytes`
/// (oldest-modified first, down to 3/4 of the cap). Best effort: a cache
/// write that fails only costs a re-make next time.
pub fn cache_put(dir: &Path, key: &str, t: &Thumb, max_bytes: u64) {
    let Some(ext) = ext_of(t.mime) else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".{key}.{}.tmp", uuid::Uuid::new_v4()));
    let path = dir.join(format!("{key}.{}x{}.{ext}", t.width, t.height));
    if std::fs::write(&tmp, &t.bytes).is_err() || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    prune(dir, max_bytes);
}

fn prune(dir: &Path, max_bytes: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path()))
        })
        .collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max_bytes {
        return;
    }
    files.sort_by_key(|f| f.0);
    let target = max_bytes / 4 * 3;
    let mut left = total;
    for (_, len, p) in files {
        if left <= target {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            left = left.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-thumbs-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = if alpha {
            DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 128])))
        } else {
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([200, 100, 50])))
        };
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    fn encode_as(w: u32, h: u32, f: ImageFormat) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([1, 2, 3])));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), f).unwrap();
        out
    }

    #[test]
    fn every_raster_type_thumbnails_and_fits_px() {
        for (name, f) in [
            ("png", ImageFormat::Png),
            ("jpeg", ImageFormat::Jpeg),
            ("gif", ImageFormat::Gif),
            ("webp", ImageFormat::WebP),
            ("bmp", ImageFormat::Bmp),
            ("tiff", ImageFormat::Tiff),
        ] {
            let bytes = encode_as(1200, 600, f);
            let t = raster_from_bytes(&bytes, None, PX_SMALL).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!((t.width, t.height), (256, 128), "{name}");
            // GIF decodes with an alpha channel (it may be transparent): PNG.
            let want = if name == "gif" { "image/png" } else { "image/jpeg" };
            assert_eq!(t.mime, want, "{name}");
            assert!(!t.sanitized);
            let back = image::load_from_memory(&t.bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back.dimensions(), (256, 128), "{name}");
        }
    }

    #[test]
    fn transparency_stays_png_and_small_images_are_not_upscaled() {
        let t = raster_from_bytes(&png(100, 40, true), None, PX_LARGE).expect("png");
        assert_eq!(t.mime, "image/png");
        assert_eq!((t.width, t.height), (100, 40));
    }

    #[test]
    fn the_format_is_sniffed_not_trusted() {
        // A text file named .png is refused, never decoded as something else.
        let e = raster_from_bytes(b"hello, not an image", None, PX_SMALL).unwrap_err();
        assert_eq!(e.code(), "no_thumbnail", "{e:?}");
        // A truncated PNG fails to decode.
        let mut p = png(64, 64, false);
        p.truncate(40);
        assert_eq!(raster_from_bytes(&p, None, PX_SMALL).unwrap_err().code(), "no_thumbnail");
    }

    /// A BMP header claiming `w`x`h` at 24 bits with no pixel data.
    fn bmp_header(w: i32, h: i32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&[0x42, 0x4d]);
        b.extend_from_slice(&54u32.to_le_bytes()); // file size (the header only)
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
        b.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[0u8; 24]);
        b
    }

    #[test]
    fn a_decompression_bomb_is_refused_before_decode() {
        // A 54-byte BMP claiming 10000x10000 (100 MP).
        let e = raster_from_bytes(&bmp_header(10_000, 10_000), None, PX_SMALL).unwrap_err();
        assert_eq!(e.code(), "preview_too_large", "{e:?}");
        // Under the limit but with no pixels: a decode error, never a panic.
        assert_eq!(raster_from_bytes(&bmp_header(100, 100), None, PX_SMALL).unwrap_err().code(), "no_thumbnail");
    }

    #[test]
    fn px_is_256_or_512() {
        assert_eq!(parse_px(None), Ok(256));
        assert_eq!(parse_px(Some("512")), Ok(512));
        assert!(parse_px(Some("1024")).is_err());
        assert!(parse_px(Some("-1")).is_err());
    }

    #[test]
    fn make_reads_files_by_mime() {
        let d = tmpdir("make");
        let p = d.join("a.webp");
        std::fs::write(&p, encode_as(600, 900, ImageFormat::WebP)).unwrap();
        let t = make(&p, "image/webp", PX_LARGE).expect("webp");
        assert_eq!((t.width, t.height), (341, 512));
        let pdf = d.join("a.pdf");
        std::fs::write(&pdf, b"%PDF-1.7").unwrap();
        assert_eq!(make(&pdf, "application/pdf", PX_SMALL).unwrap_err().code(), "no_thumbnail");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn heic_converts_through_sips_on_macos() {
        let d = tmpdir("heic");
        let src = d.join("in.png");
        std::fs::write(&src, png(800, 400, false)).unwrap();
        let heic = d.join("photo.heic");
        let st = Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&src)
            .arg("--out")
            .arg(&heic)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("sips runs");
        assert!(st.success(), "sips made a HEIC");
        let t = make(&heic, "image/heic", PX_SMALL).expect("heic thumb");
        assert_eq!(t.mime, "image/jpeg");
        assert_eq!((t.width, t.height), (256, 128));
        std::fs::remove_dir_all(&d).unwrap();
    }

    const EVIL_SVG: &str = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="120" height="60" onload="alert(1)">
  <script>alert(2)</script>
  <style>@import url(https://evil.example/x.css); rect { fill: red }</style>
  <style>circle { fill: url(#g) }</style>
  <defs><linearGradient id="g"><stop offset="0" stop-color="#fff"/></linearGradient></defs>
  <a href="javascript:alert(3)"><rect width="10" height="10" onclick="alert(4)" style="fill: url(https://evil.example/p)"/></a>
  <circle cx="5" cy="5" r="4" fill="url(#g)"/>
  <foreignObject><div xmlns="http://www.w3.org/1999/xhtml">hi</div></foreignObject>
  <image href="https://evil.example/track.png" width="1" height="1"/>
  <image href="data:image/png;base64,AAAA" width="1" height="1"/>
  <image href="data:image/svg+xml;base64,AAAA" width="1" height="1"/>
  <use xlink:href="#g"/>
  <use xlink:href="https://evil.example/s.svg#x"/>
  <set attributeName="href" to="javascript:alert(5)"/>
  <animate attributeName="href" values="javascript:alert(6)"/>
  <iframe src="https://evil.example"/>
  <text x="1" y="9">Hello &amp; bye</text>
</svg>"##;

    #[test]
    fn svg_sanitizer_keeps_the_drawing_and_drops_every_active_part() {
        let out = sanitize_svg(EVIL_SVG.as_bytes()).expect("sanitizes");
        let lower = out.to_ascii_lowercase();
        for bad in [
            "<script", "alert(", "onload", "onclick", "@import", "evil.example", "foreignobject", "javascript:",
            "<set", "<animate", "<iframe", "data:image/svg", "<a ", "<?xml",
        ] {
            assert!(!lower.contains(bad), "{bad} survived:\n{out}");
        }
        for good in [
            "<svg", "<rect", "<circle", "fill=\"url(#g)\"", "<lineargradient", "<stop", "xlink:href=\"#g\"",
            "data:image/png;base64,aaaa", "hello &amp; bye", "circle { fill: url(#g) }",
        ] {
            assert!(lower.contains(good), "{good} was lost:\n{out}");
        }
        assert_eq!(svg_size(&out), (120, 60));
        // The rewrite is a fixed point.
        assert_eq!(sanitize_svg(out.as_bytes()).expect("again"), out);
    }

    #[test]
    fn svg_refusals() {
        let doctype = r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY x "boom">]><svg xmlns="http://www.w3.org/2000/svg">&x;</svg>"#;
        assert_eq!(sanitize_svg(doctype.as_bytes()).unwrap_err().code(), "no_thumbnail");
        assert!(sanitize_svg(b"<html><script>1</script></html>").is_err(), "not an svg root");
        assert!(sanitize_svg(b"<svg><g></svg>").is_err(), "not well-formed");
        assert!(sanitize_svg(&[0xff, 0xfe, 0x00]).is_err(), "not utf-8");
        assert!(sanitize_svg(b"").is_err(), "empty");
    }

    #[test]
    fn svg_viewbox_gives_the_size() {
        let s = sanitize_svg(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 150"><rect width="1" height="1"/></svg>"#)
            .unwrap();
        assert_eq!(svg_size(&s), (300, 150));
    }

    #[test]
    fn cache_round_trip_and_prune() {
        let d = tmpdir("cache");
        let t = raster_from_bytes(&png(300, 300, false), None, PX_SMALL).unwrap();
        let k = cache_key("p1", "a/b.png", 10, 99, 256);
        assert_ne!(k, cache_key("p1", "a/b.png", 10, 100, 256), "mtime is in the key");
        assert_ne!(k, cache_key("p1", "a/b.png", 10, 99, 512), "px is in the key");
        assert!(cache_get(&d, &k).is_none());
        cache_put(&d, &k, &t, CACHE_MAX_BYTES);
        assert_eq!(cache_get(&d, &k).expect("cached"), t);
        // Over the cap: older entries go first.
        for i in 0..5 {
            cache_put(&d, &cache_key("p", &format!("{i}"), 1, 1, 256), &t, 1);
        }
        let left = std::fs::read_dir(&d).unwrap().count();
        assert!(left <= 1, "pruned to the cap, {left} left");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
