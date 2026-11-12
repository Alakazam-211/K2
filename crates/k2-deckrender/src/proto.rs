//! The wire between the host and one `k2-deckrender` process. stdin and
//! stdout only: no port, no token, no file.
//!
//! **Request (the worker's stdin):** one line of JSON, the [`Header`], then
//! exactly `input_len` bytes of the `.pptx`, then EOF.
//!
//! ```text
//! {"proto":1,"input_len":48213,"width":1280,"slide":{"index":0},"memory_bytes":1073741824}
//! <48213 bytes>
//! ```
//!
//! `slide` is `{"index": n}` (0-based, among the slides the reader could read,
//! in file order) or `{"sld_id": n}` (the `p:sldId/@id` in the presentation
//! part). `width` is the image's width in pixels; its height follows the
//! slide's aspect ratio.
//!
//! **Response (stdout):** one line of JSON, the [`ResponseHeader`], then,
//! when `ok`, exactly `image_len` bytes of the image (`format`: WebP lossless by
//! default, or PNG; flattened onto white), then EOF.
//!
//! ```text
//! {"proto":1,"ok":true,"format":"webp","image_len":91234,"meta":{"width":1280,"height":720,...}}
//! <91234 bytes>
//! {"proto":1,"ok":false,"code":"too_large","detail":"the slide is 1280 x 5120000 px; at most 8192 a side"}
//! ```
//!
//! The worker exits [`EXIT_OK`] after an image, [`EXIT_FAILED`] after a typed
//! failure. Anything else (no response line, a malformed one, fewer image
//! bytes than announced, bytes after the image) is a breach: the host reports
//! [`FailureCode::RendererError`].
//!
//! **What [`Meta`] carries:** the slide resolved (index, sldId, count), the
//! shapes of the slide itself with their page-space boxes ([`ShapeBox`]), the
//! font families drawn and the ones substituted ([`Fonts`]), and warnings.

use serde::{Deserialize, Serialize};

/// The protocol version. The host refuses a response with another.
pub const PROTO: u64 = 1;

/// The worker's version line (its own, like k2-export's).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The worker binary's file name (beside the daemon).
pub const BINARY_NAME: &str = if cfg!(windows) { "k2-deckrender.exe" } else { "k2-deckrender" };

/// Largest request header line.
pub const MAX_HEADER: usize = 64 << 10;

/// Largest response header line (the shape list is in it).
pub const MAX_RESPONSE_HEADER: usize = 8 << 20;

/// Most shapes listed in [`Meta::shapes`]; the rest are counted in
/// [`Meta::shapes_omitted`].
pub const MAX_SHAPES: usize = 10_000;

/// Longest shape name kept (in characters).
pub const MAX_NAME_CHARS: usize = 256;

/// The worker's exit after an image.
pub const EXIT_OK: i32 = 0;
/// The worker's exit after a typed failure.
pub const EXIT_FAILED: i32 = 1;
/// The worker's exit on a bad command line.
pub const EXIT_USAGE: i32 = 2;

/// Which slide to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlideRef {
    /// 0-based, among the slides the reader could read, in file order.
    Index(u32),
    /// The slide's `p:sldId/@id` in the presentation part.
    SldId(u32),
}

/// Why a render failed. The host maps its own limits onto the same codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// Over a limit: the input's bytes, the image's size, the memory cap, the
    /// output's bytes.
    TooLarge,
    /// A file this reader doesn't render: a password-protected package, a
    /// binary `.ppt`, a package part it doesn't support.
    UnsupportedPart,
    /// The wall-clock or CPU limit.
    Timeout,
    /// The renderer failed: a panic, a crash, a breach of this protocol, or
    /// the worker couldn't start.
    RendererError,
    /// The bytes are not a `.pptx` package (truncated, garbage, malformed).
    InvalidInput,
    /// No slide at that index or sldId (or the deck has none).
    NoSuchSlide,
}

impl FailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureCode::TooLarge => "too_large",
            FailureCode::UnsupportedPart => "unsupported_part",
            FailureCode::Timeout => "timeout",
            FailureCode::RendererError => "renderer_error",
            FailureCode::InvalidInput => "invalid_input",
            FailureCode::NoSuchSlide => "no_such_slide",
        }
    }
}

impl std::fmt::Display for FailureCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The image's encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// WebP lossless (image-webp, pure Rust).
    #[default]
    Webp,
    Png,
}

impl ImageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Webp => "webp",
            ImageFormat::Png => "png",
        }
    }
}

fn default_max_side() -> u32 {
    8192
}

fn default_max_pixels() -> u64 {
    32 << 20
}

/// The request header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub proto: u64,
    /// The `.pptx` bytes that follow.
    pub input_len: u64,
    /// The image's width in pixels.
    pub width: u32,
    pub slide: SlideRef,
    /// The worker's memory cap (its allocator's; 0: none). The worker sets
    /// its cap from argv ([`crate::sandbox::SelfLimits`]) before it reads
    /// this; a header that disagrees is a `renderer_error`.
    pub memory_bytes: u64,
    /// Longest image side (the height is checked against it too).
    #[serde(default = "default_max_side")]
    pub max_side: u32,
    /// Most pixels in the image.
    #[serde(default = "default_max_pixels")]
    pub max_pixels: u64,
    /// Render threads (0: the worker's own thread).
    #[serde(default)]
    pub threads: u16,
    /// The image's encoding (default WebP lossless).
    #[serde(default)]
    pub format: ImageFormat,
    /// The text every date field shows (the host's cached field text). Absent: the cached text
    /// of the slide's first date field (its layout's, its master's) as the file saved it. The
    /// worker never reads the clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_text: Option<String>,
    /// TEST ONLY: what a `test-hostile` worker does instead of rendering;
    /// any other build answers `renderer_error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostile: Option<String>,
}

impl Header {
    /// The header line, newline included.
    pub fn line(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec(self).unwrap_or_default();
        v.push(b'\n');
        v
    }

    /// Parse a header line (without its newline).
    pub fn parse(line: &[u8]) -> Result<Header, String> {
        let h: Header = serde_json::from_slice(line).map_err(|e| format!("the request header is not valid: {e}"))?;
        if h.proto != PROTO {
            return Err(format!("the request speaks proto {}, this worker {PROTO}", h.proto));
        }
        Ok(h)
    }
}

/// One shape of the slide itself (not of its layout or master), in drawing
/// order (back to front; a group before its children). Hidden shapes are left
/// out. EMU are 1/12700 pt (914400 an inch), rounded to whole EMU.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeBox {
    /// The shape's id: the file's `p:cNvPr/@id` when it was unique and in
    /// range, else the id the reader gave it.
    pub id: u32,
    pub name: String,
    /// Index in [`Meta::shapes`] of the group this shape is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    /// `shape`, `group`, `picture`, `connector`, `table`, `chart`, `media`,
    /// `ink`, `model3d` or `other`.
    pub kind: String,
    /// The shape's box on the page, `a:xfrm` style: `x`, `y`, `w`, `h` in EMU
    /// (its unrotated box, centred where it is drawn), with `rot` and the
    /// flips, every enclosing group composed in (the shape's own `a:xfrm`, or
    /// its layout placeholder's when it has none).
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// Degrees clockwise.
    pub rot: f64,
    pub flip_h: bool,
    pub flip_v: bool,
    /// The box above is approximate: the shape (or a group it is in) sits in
    /// a group scaled unevenly (child extents to group extents) with a
    /// rotation that isn't a multiple of 90 degrees, so it is drawn sheared,
    /// not as a rotated rectangle.
    pub approx_box: bool,
    /// The drawn shape's page-space axis-aligned bounding box, `[x, y, w, h]`
    /// in EMU (rotation, flips and every group applied).
    pub bbox: [i64; 4],
    /// Shape-local points (`0..w`, `0..h` of its own box, in pt) to page
    /// points as drawn: `[a, b, c, d, e, f]` (x' = a·x + c·y + e, y' = b·x + d·y + f).
    pub affine: [f64; 6],
}

/// A family the slide asks for that this machine doesn't have, and the one
/// drawn instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontSubstitution {
    pub requested: String,
    pub used: String,
}

/// Fonts of the drawn slide.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fonts {
    /// Families the slide's text was drawn in (the layout's and master's text included).
    pub used: Vec<String>,
    /// Families drawn with another: the Latin families the slide's own text
    /// runs ask for (theme fonts resolved) and the families the file embeds,
    /// when the bundled fonts lack them. East Asian, complex-script and
    /// table-cell fonts aren't listed yet.
    pub substituted: Vec<FontSubstitution>,
    /// The families the file embeds. Their font programs are never loaded
    /// (contract Q9: bundled fonts only until the Steward rules on embedded
    /// ones); `used` is what is drawn for each.
    #[serde(default)]
    pub embedded: Vec<EmbeddedFamily>,
}

/// A family the file embeds, reported and not loaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedFamily {
    pub family: String,
    /// Always `true`: the file embeds it.
    pub embedded: bool,
    /// The family drawn for it (a bundled one).
    pub used: String,
}

/// What the worker reports with an image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// The image's size in pixels.
    pub width: u32,
    pub height: u32,
    /// The slide drawn: its index, its sldId when the slide list could be
    /// matched to the package's, and how many slides the reader read.
    pub slide_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sld_id: Option<u32>,
    pub slide_count: u32,
    /// The slide's size in EMU.
    pub slide_size_emu: [i64; 2],
    pub shapes: Vec<ShapeBox>,
    /// Shapes past [`MAX_SHAPES`].
    #[serde(default)]
    pub shapes_omitted: u64,
    pub fonts: Fonts,
    /// The date field text drawn, and where it came from: `request`, `file` (the
    /// slide's own cached field text) or `none` (no date field).
    pub date_text: String,
    pub date_text_from: String,
    /// Where the time went, in milliseconds.
    pub timings: Timings,
    /// Slide parts the package lists but the reader couldn't read (left out).
    #[serde(default)]
    pub lost_slides: u32,
}

/// Where a render's time went, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// Setting up the fonts (the manifest's families, every face loaded): once per process.
    pub fonts_ms: f64,
    /// Reading the package.
    pub parse_ms: f64,
    pub render_ms: f64,
    /// Flattening onto white and encoding.
    pub encode_ms: f64,
    pub families: u32,
    pub faces: u32,
}

/// The response line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseHeader {
    pub proto: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<FailureCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

impl ResponseHeader {
    /// The line for an image of `image_len` bytes.
    pub fn ok(meta: Meta, format: ImageFormat, image_len: usize) -> ResponseHeader {
        ResponseHeader {
            proto: PROTO,
            ok: true,
            code: None,
            detail: None,
            format: Some(format.as_str().into()),
            image_len: Some(image_len as u64),
            meta: Some(meta),
        }
    }

    /// The line for a typed failure.
    pub fn failed(code: FailureCode, detail: impl Into<String>) -> ResponseHeader {
        ResponseHeader { proto: PROTO, ok: false, code: Some(code), detail: Some(detail.into()), format: None, image_len: None, meta: None }
    }

    /// The line, newline included.
    pub fn line(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec(self).unwrap_or_else(|_| {
            // Never: every field serializes. A failure line is still a failure.
            br#"{"proto":1,"ok":false,"code":"renderer_error","detail":"the response did not serialize"}"#.to_vec()
        });
        v.push(b'\n');
        v
    }

    /// Parse a response line (without its newline).
    pub fn parse(line: &[u8]) -> Result<ResponseHeader, String> {
        let r: ResponseHeader = serde_json::from_slice(line).map_err(|e| format!("the response line is not valid: {e}"))?;
        if r.proto != PROTO {
            return Err(format!("the worker speaks proto {}, the host {PROTO}", r.proto));
        }
        match (r.ok, &r.code, &r.image_len, &r.meta) {
            (true, None, Some(_), Some(_)) | (false, Some(_), None, None) => Ok(r),
            _ => Err("the response line is neither an image nor a failure".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_round_trips_and_takes_defaults() {
        let h = Header {
            proto: PROTO,
            input_len: 10,
            width: 640,
            slide: SlideRef::SldId(256),
            memory_bytes: 1 << 30,
            max_side: 4096,
            max_pixels: 1 << 20,
            threads: 0,
            format: ImageFormat::Png,
            date_text: Some("11/11/2026".into()),
            hostile: None,
        };
        let line = h.line();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(Header::parse(&line[..line.len() - 1]).expect("parse"), h);
        let min = br#"{"proto":1,"input_len":3,"width":10,"slide":{"index":2},"memory_bytes":0}"#;
        let m = Header::parse(min).expect("defaults");
        assert_eq!((m.max_side, m.max_pixels, m.threads, m.slide), (8192, 32 << 20, 0, SlideRef::Index(2)));
        assert_eq!((m.format, m.date_text), (ImageFormat::Webp, None));
        assert!(Header::parse(br#"{"proto":2,"input_len":3,"width":10,"slide":{"index":2},"memory_bytes":0}"#).is_err());
        assert!(Header::parse(b"not json").is_err());
    }

    #[test]
    fn failure_lines_name_their_code() {
        let line = ResponseHeader::failed(FailureCode::TooLarge, "x").line();
        let s = String::from_utf8(line.clone()).expect("utf-8");
        assert!(s.contains(r#""code":"too_large""#), "{s}");
        let r = ResponseHeader::parse(&line[..line.len() - 1]).expect("parse");
        assert_eq!(r.code, Some(FailureCode::TooLarge));
        assert!(ResponseHeader::parse(br#"{"proto":1,"ok":true}"#).is_err(), "an image line without its length is a breach");
        for c in [
            FailureCode::TooLarge,
            FailureCode::UnsupportedPart,
            FailureCode::Timeout,
            FailureCode::RendererError,
            FailureCode::InvalidInput,
            FailureCode::NoSuchSlide,
        ] {
            assert_eq!(serde_json::to_value(c).expect("code"), serde_json::Value::String(c.as_str().into()));
        }
    }
}
