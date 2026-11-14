//! The wire between the host and one `k2-scenerender` process: argv, stdin
//! and stdout only (no port, no token, no file).
//!
//! **argv** (the host's, never the input's): `--sandbox` and the limits, see
//! [`crate::sandbox::Args`]. The worker puts them on itself before it reads a
//! byte of stdin.
//!
//! **Request (stdin):** one line of JSON, the [`Request`], then exactly
//! `input_len` bytes of K2SCENE1 ([`crate::scene`]), then EOF. One request
//! carries a list of views: the scene is read and decoded once and drawn N
//! times (a turntable is one process).
//!
//! ```text
//! {"proto":1,"input_len":1048,"views":[{"view":"auto","width":320},{"view":{"turntable":3},"width":320}],"format":"webp"}
//! <1048 bytes>
//! ```
//!
//! `view` is `"auto"` (the 3/4 view), `{"turntable": k}` (k = 0..7) or
//! `{"camera": i}`; `objects` (optional) draws and frames only those object
//! indices; the height is `round(width / aspect)`, the aspect from the view,
//! else the camera's, else 4:3. `background` (sRGB) sets the opaque images'
//! background; `"transparent": true` draws RGBA with coverage alpha instead
//! (straight colour, see [`crate::raster`]), and refuses a `background`.
//! Absent, both give the opaque default, byte for byte the same as before.
//! An auto view may ask `"framing": "contents"`: fitted to the contents, not
//! to the room around them ([`crate::camera::Framing`], [`crate::contents`]);
//! absent, `"bounds"`, the auto view as it always was. Like the background,
//! the framing is the request's, not the scene's: a host that asks for both
//! keys its cache with it.
//! `"clay": {}` shades every object in one neutral clay tone by its class
//! instead of its base colour: shell darker, contents lighter ([`Clay`]; the
//! tones may be given, `{"shell": [r, g, b], "contents": [r, g, b]}`, sRGB).
//! Absent, objects keep their base colours. It is the request's too.
//! `"ao": {}` darkens creases, corners and where objects meet the floor with
//! screen-space ambient occlusion ([`Ao`]; `{"strength": s, "radius": r}`,
//! both optional, see [`crate::raster::occlusion`]), with or without clay,
//! opaque or transparent (the alpha stays the coverage). Absent, no occlusion:
//! the bytes are as before. It is the request's too.
//! `"ids": true` also draws each view's id image: which object is under each
//! pixel ([`IdsMeta`], [`crate::raster::render_with_ids`]). The stills are the
//! same bytes with it or without; absent, no id image and no table.
//!
//! **Response (stdout):** one line of JSON, the [`Response`], then, when `ok`,
//! each view's image in order (`views[i].image_len` bytes each), each followed,
//! when the request asked for ids, by that view's id image
//! (`views[i].ids.image_len` bytes), then EOF.
//! Each view's [`ViewMeta`] carries the camera it was drawn through
//! ([`CameraMeta`]: view and projection matrices, the pixel mapping), left
//! out only when one of its values isn't finite. A view that asked for content
//! framing also carries [`FramingMeta`]: whether the contents were framed and
//! which objects were left out of the frame. A view drawn with ids carries
//! [`IdsMeta`]: its id image's length and the ids of the objects in it.
//! The worker exits [`EXIT_OK`] after the images, [`EXIT_FAILED`] after a
//! typed failure. Anything else (no line, a malformed one, fewer image bytes
//! than announced, bytes after them) is a breach the host reports as
//! [`FailureCode::RendererError`].

use serde::{Deserialize, Serialize};

use crate::camera::{Framing, ViewSpec};

/// The protocol version. The host refuses a response with another.
pub const PROTO: u64 = 1;

/// The worker's version line (its own, like k2-deckrender's).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The worker binary's file name (beside the daemon).
pub const BINARY_NAME: &str = if cfg!(windows) { "k2-scenerender.exe" } else { "k2-scenerender" };

/// Largest request line (the views' object lists are in it).
pub const MAX_REQUEST_LINE: usize = 4 << 20;

/// Largest response line (the objects drawn without modifiers are in it).
pub const MAX_RESPONSE_LINE: usize = 8 << 20;

/// The id tables of a response's views together list objects up to this many
/// bytes of their JSON ([`IdsMeta::objects`]); each view gets an equal share.
/// Every object in an id image is still counted. Half the response line: the
/// rest of it (cameras, left-out lists, the objects without modifiers) stays
/// well inside the other half.
pub const MAX_IDS_LISTED_BYTES: usize = 4 << 20;

/// The background when the request names none: sRGB 240, 240, 240, the
/// current neutral (K2 Design picks the final one).
pub const DEFAULT_BACKGROUND: [u8; 3] = [240, 240, 240];

// Changing either default clay tone changes images the host has already
// cached under the same renderer build: bump this crate's version in the
// same commit (or change the host's ".clay.ao" cache tag). The same goes
// for the default ambient occlusion strength and radius below.

/// Clay shading's shell tone (floor, walls, ceiling) when the request names
/// none: sRGB 138, 134, 128, a warm mid grey. Lit by the same lights, it
/// stays well clear of the default background (sRGB 240).
pub const CLAY_SHELL: [u8; 3] = [138, 134, 128];

/// Clay shading's contents tone (furniture, props) when the request names
/// none: sRGB 206, 202, 194, a warm light grey. Lit, a face gets between
/// 0.22 of the tone in linear light (ambient alone, about sRGB 103) and
/// about 0.99 of it (turned between both lights, about sRGB 205); a face
/// turned to the key light gets 0.94 (sRGB 200, 196, 188). Lighter than the
/// shell, short of the default background.
pub const CLAY_CONTENTS: [u8; 3] = [206, 202, 194];

// Changing a default occlusion setting needs the same bump (see the clay tones' note).

/// Ambient occlusion's strength when the request names none: the whole
/// effect (a right-angle crease keeps about 0.7 of its light, a narrow gap
/// less); a lower strength is lighter.
pub const AO_STRENGTH: f32 = 1.0;

/// Ambient occlusion's radius when the request names none, as a fraction of
/// the drawn objects' bounding radius (a 6 x 5 m room: about 0.4 m).
pub const AO_RADIUS: f32 = 0.1;

/// The aspect when neither the view nor the camera gives one.
pub const DEFAULT_ASPECT: f64 = 4.0 / 3.0;

/// The worker's exit after its images.
pub const EXIT_OK: i32 = 0;
/// The worker's exit after a typed failure.
pub const EXIT_FAILED: i32 = 1;
/// The worker's exit on a bad command line (it reads nothing).
pub const EXIT_USAGE: i32 = 2;

/// SSAA per axis when the request names none: 4x4 up to 640 px wide, 2x2
/// above (never above the host's cap).
pub fn default_ssaa(width: u32) -> u32 {
    if width <= 640 {
        4
    } else {
        2
    }
}

/// Why a render failed. The host maps its own limits onto the same codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// Over a limit: the input, a count, the image's size, the memory cap,
    /// the output's bytes.
    TooLarge,
    /// The bytes are not a valid K2SCENE1, or the request names a view that
    /// isn't there (a camera, an object).
    InvalidInput,
    /// The wall-clock or CPU limit.
    Timeout,
    /// The renderer failed: a panic, a crash, a breach of this protocol, or
    /// the worker couldn't start or limit itself.
    RendererError,
}

impl FailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureCode::TooLarge => "too_large",
            FailureCode::InvalidInput => "invalid_input",
            FailureCode::Timeout => "timeout",
            FailureCode::RendererError => "renderer_error",
        }
    }

    pub fn from_input(e: &crate::scene::InputError) -> FailureCode {
        if e.code() == "too_large" {
            FailureCode::TooLarge
        } else {
            FailureCode::InvalidInput
        }
    }
}

impl std::fmt::Display for FailureCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The images' encoding.
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

/// Clay shading: each object drawn in one of two neutral tones (sRGB) by its
/// class ([`crate::contents`], the same rule as content framing) instead of
/// its base colour, under the same lights; transparent images keep their
/// coverage alpha. Either tone may be left out for its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clay {
    /// The shell's tone (a darker one): [`CLAY_SHELL`] when absent.
    #[serde(default = "clay_shell")]
    pub shell: [u8; 3],
    /// The contents' tone (a lighter one): [`CLAY_CONTENTS`] when absent.
    #[serde(default = "clay_contents")]
    pub contents: [u8; 3],
}

fn clay_shell() -> [u8; 3] {
    CLAY_SHELL
}

fn clay_contents() -> [u8; 3] {
    CLAY_CONTENTS
}

impl Default for Clay {
    fn default() -> Clay {
        Clay { shell: CLAY_SHELL, contents: CLAY_CONTENTS }
    }
}

/// Screen-space ambient occlusion ([`crate::raster::occlusion`]): each
/// covered pixel's light is darkened by how much the surfaces around it
/// close in on it, from the depth image alone. Either field may be left out
/// for its default.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ao {
    /// How dark full occlusion gets, 0 to 1 ([`AO_STRENGTH`] when absent):
    /// the light is multiplied by `1 - strength * occlusion`.
    #[serde(default = "ao_strength")]
    pub strength: f32,
    /// How far an occluder reaches, as a fraction (above 0, at most 1) of the
    /// drawn objects' bounding radius ([`AO_RADIUS`] when absent): a world
    /// size, so a still looks the same at any width.
    #[serde(default = "ao_radius")]
    pub radius: f32,
}

fn ao_strength() -> f32 {
    AO_STRENGTH
}

fn ao_radius() -> f32 {
    AO_RADIUS
}

impl Default for Ao {
    fn default() -> Ao {
        Ao { strength: AO_STRENGTH, radius: AO_RADIUS }
    }
}

impl Ao {
    /// Both fields in range: the strength in [0, 1], the radius in (0, 1].
    pub fn check(&self) -> Result<(), String> {
        if !(self.strength >= 0.0 && self.strength <= 1.0) {
            return Err(format!("ao strength {}: 0 to 1", self.strength));
        }
        if !(self.radius > 0.0 && self.radius <= 1.0) {
            return Err(format!("ao radius {}: above 0, at most 1", self.radius));
        }
        Ok(())
    }
}

/// One view to draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub view: ViewSpec,
    /// Draw (and frame) only these objects; `None`: all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<Vec<u32>>,
    /// The image's width in pixels.
    pub width: u32,
    /// Width / height; else the camera's, else 4:3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f32>,
    /// How an auto view is fitted (`bounds` when absent). `contents` is
    /// refused on a turntable frame or a camera view.
    #[serde(default, skip_serializing_if = "Framing::is_bounds")]
    pub framing: Framing,
}

impl View {
    pub fn new(view: ViewSpec, width: u32) -> View {
        View { view, objects: None, width, aspect: None, framing: Framing::Bounds }
    }
}

/// The request line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub proto: u64,
    /// The K2SCENE1 bytes that follow.
    pub input_len: u64,
    pub views: Vec<View>,
    #[serde(default)]
    pub format: ImageFormat,
    /// sRGB background; `None`: [`DEFAULT_BACKGROUND`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<[u8; 3]>,
    /// RGBA images, alpha = coverage, colour straight; no `background` with it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transparent: bool,
    /// Clay shading in two tones by class; `None`: base colours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clay: Option<Clay>,
    /// Screen-space ambient occlusion; `None`: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ao: Option<Ao>,
    /// Each view's id image and table too ([`IdsMeta`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ids: bool,
    /// Samples per axis (1-4); `None`: [`default_ssaa`] of each view's width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssaa: Option<u32>,
    /// Render threads (at most the host's cap; 0 or 1: one). The output is
    /// the same for any count.
    #[serde(default)]
    pub threads: u32,
    /// TEST ONLY: what a `test-hostile` worker does instead of rendering;
    /// any other build answers `renderer_error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostile: Option<String>,
}

impl Request {
    pub fn new(views: Vec<View>) -> Request {
        Request { proto: PROTO, input_len: 0, views, format: ImageFormat::Webp, background: None, transparent: false, clay: None, ao: None, ids: false, ssaa: None, threads: 1, hostile: None }
    }

    /// The line, newline included.
    pub fn line(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec(self).unwrap_or_default();
        v.push(b'\n');
        v
    }

    /// Parse a request line (without its newline).
    pub fn parse(line: &[u8]) -> Result<Request, String> {
        let r: Request = serde_json::from_slice(line).map_err(|e| format!("the request is not valid: {e}"))?;
        if r.proto != PROTO {
            return Err(format!("the request speaks proto {}, this worker {PROTO}", r.proto));
        }
        Ok(r)
    }
}

/// The camera a view was drawn through, as the rasterizer used it (the
/// f32 values themselves, not recomputed), so a client can line its own
/// overlay or 3D up with the still.
///
/// World to pixel, for an image `width` x `height`:
///
/// ```text
/// v    = view * [x, y, z, 1]                 (view space: the camera looks down -Z, +Y up)
/// clip = proj * v = [fx * v.x + shift_x * w, fy * v.y + shift_y * w, -v.z - 2 near, w],  w = -v.z
/// ndc  = clip.xy / w                         (in front of the eye: w >= near)
/// px   = (ndc.x * 0.5 + 0.5) * width         (x right)
/// py   = (0.5 - ndc.y * 0.5) * height        (y down)
/// ```
///
/// `(px, py)` is continuous: pixel `(i, j)` covers `[i, i + 1) x [j, j + 1)`
/// and its centre is `(i + 0.5, j + 0.5)`; NDC (-1, 1) is the image's top-left
/// corner and (1, -1) its bottom-right. The rasterizer snaps `(px, py)` to
/// 1/256 of a sample and covers the SSAA sample centres inside each triangle
/// (top-left rule).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraMeta {
    /// World-to-view, column-major 4x4 (the rasterizer's f64 matrix, rounded
    /// to f32; it composes the f64 one with each object's transform).
    pub view: [f32; 16],
    /// Projection, column-major 4x4: OpenGL clip convention, infinite far
    /// plane, the near plane below (see [`crate::camera::Frame::projection`]).
    pub proj: [f32; 16],
    /// x_clip = fx * x_view, y_clip = fy * y_view.
    pub fx: f32,
    pub fy: f32,
    /// The near plane's distance (> 0): nearer is clipped.
    pub near: f32,
    /// Lens shift in NDC units, added to x and y after the divide by w: 0
    /// except for a content-framed view, where it centres the contents.
    pub shift_x: f32,
    pub shift_y: f32,
}

impl CameraMeta {
    /// Every value finite (JSON has no infinity: a degenerate scene's camera
    /// may not be).
    pub fn is_finite(&self) -> bool {
        self.view.iter().chain(&self.proj).chain(&[self.fx, self.fy, self.near, self.shift_x, self.shift_y]).all(|v| v.is_finite())
    }
}

/// What content framing did for one view (only on a view that asked for it).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FramingMeta {
    /// The contents were framed. `false`: no object is contents (none at all,
    /// or every one is shell), so the view fell back to the bounds framing.
    pub contents: bool,
    /// How many drawn objects were left out of the frame (the shell): drawn,
    /// but not fitted.
    pub left_out_count: u64,
    /// Their indices in the scene's objects, ascending, at most
    /// [`crate::contents::MAX_LEFT_OUT_LISTED`] of them.
    pub left_out: Vec<u32>,
    /// The contents were smaller than the least frame (a fraction of the
    /// scene, [`crate::contents::MIN_FRAME_FRACTION`]), so the frame shows
    /// some of the room around them.
    pub grown: bool,
}

/// One object in a view's id image: its index in the scene's objects (the
/// image holds index + 1) and its id as the scene names it, byte for byte
/// (never trimmed, folded or made unique: several objects may share an id,
/// one product split into several objects, and each is listed under its own
/// index).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdEntry {
    pub index: u32,
    pub id: String,
}

/// A view's id image (only when the request asked for ids): PNG, 8-bit RGB,
/// at the still's size, each pixel `r << 16 | g << 8 | b` (see
/// [`crate::encode::ids_png`]): 0 where nothing is drawn, else the index + 1
/// of the object drawn at the pixel's one depth sample, the nearer where
/// objects overlap. Indices, not ids, so renaming an object changes the table,
/// never the image. A pixel is on an edge between objects when a 4-neighbour
/// holds another value (or 0).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdsMeta {
    /// The id image's bytes, right after the view's image.
    pub image_len: u64,
    /// How many objects the id image shows.
    pub count: u64,
    /// Those objects, ascending by index, as many as fit the view's share of
    /// [`MAX_IDS_LISTED_BYTES`]: fewer than `count` when cut short.
    pub objects: Vec<IdEntry>,
}

/// What the worker reports for one view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewMeta {
    pub width: u32,
    pub height: u32,
    pub ssaa: u32,
    pub image_len: u64,
    /// The camera the view was drawn through; `None` when one of its values
    /// isn't finite (a degenerate scene, say coordinates near f32's limit:
    /// JSON has no infinity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraMeta>,
    /// What content framing did; `None` unless the view asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framing: Option<FramingMeta>,
    /// The id image and its table; `None` unless the request asked for ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdsMeta>,
    /// Objects and triangles drawn (instancing expanded), and the pixels any covered.
    pub objects: u64,
    pub triangles: u64,
    pub covered_px: u64,
    /// Some drawn object's mesh is its base mesh, without its modifiers
    /// (K2 Cores' `modifiers_not_applied`): the view says "shown without modifiers".
    pub without_modifiers: bool,
    pub vertex_ms: f64,
    pub bin_ms: f64,
    pub raster_ms: f64,
    pub encode_ms: f64,
}

/// Where a request's shared time went, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// The request line, the prefix and the header (every bound checked).
    pub header_ms: f64,
    /// The mesh data, read and checked straight into its arrays.
    pub data_ms: f64,
    /// Every view drawn and encoded.
    pub views_ms: f64,
}

/// The response line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub proto: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<FailureCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ImageFormat>,
    /// One per requested view, in order; the images follow in the same order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub views: Option<Vec<ViewMeta>>,
    /// Indices of the scene's objects whose modifiers were not applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects_without_modifiers: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<Timings>,
    /// The worker's up-front memory estimate, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_bytes: Option<u64>,
}

impl Response {
    /// The line for a typed failure.
    pub fn failed(code: FailureCode, detail: impl Into<String>) -> Response {
        Response {
            proto: PROTO,
            ok: false,
            code: Some(code),
            detail: Some(detail.into()),
            format: None,
            views: None,
            objects_without_modifiers: None,
            timings: None,
            estimate_bytes: None,
        }
    }

    /// The line, newline included.
    pub fn line(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec(self).unwrap_or_else(|_| {
            br#"{"proto":1,"ok":false,"code":"renderer_error","detail":"the response did not serialize"}"#.to_vec()
        });
        v.push(b'\n');
        v
    }

    /// Parse a response line (without its newline).
    pub fn parse(line: &[u8]) -> Result<Response, String> {
        let r: Response = serde_json::from_slice(line).map_err(|e| format!("the response line is not valid: {e}"))?;
        if r.proto != PROTO {
            return Err(format!("the worker speaks proto {}, the host {PROTO}", r.proto));
        }
        match (r.ok, &r.code, &r.views, &r.format) {
            (true, None, Some(_), Some(_)) | (false, Some(_), None, None) => Ok(r),
            _ => Err("the response line is neither images nor a failure".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips_and_takes_defaults() {
        let mut r = Request::new(vec![View::new(ViewSpec::Auto, 320), View { view: ViewSpec::Turntable(3), objects: Some(vec![1, 2]), width: 640, aspect: Some(1.0), framing: Framing::Bounds }]);
        r.input_len = 99;
        r.background = Some([10, 20, 30]);
        let line = r.line();
        assert_eq!(Request::parse(&line[..line.len() - 1]).expect("parse"), r);
        let min = br#"{"proto":1,"input_len":3,"views":[{"view":{"camera":2},"width":10}]}"#;
        let m = Request::parse(min).expect("defaults");
        assert_eq!((m.format, m.background, m.transparent, m.ssaa, m.threads, m.hostile), (ImageFormat::Webp, None, false, None, 0, None));
        // Opaque, the line is what it was; transparent, it says so and round-trips.
        assert!(!String::from_utf8_lossy(&line).contains("transparent"));
        let mut t = r.clone();
        t.background = None;
        t.transparent = true;
        let tl = t.line();
        assert!(String::from_utf8_lossy(&tl).contains(r#""transparent":true"#));
        assert_eq!(Request::parse(&tl[..tl.len() - 1]).expect("parse"), t);
        assert_eq!(m.views[0].view, ViewSpec::Camera(2));
        // Bounds framing is the default and stays off the line; contents round-trips.
        assert_eq!(m.views[0].framing, Framing::Bounds);
        assert!(!String::from_utf8_lossy(&line).contains("framing"));
        let mut c = r.clone();
        c.views[0].framing = Framing::Contents;
        let cl = c.line();
        assert!(String::from_utf8_lossy(&cl).contains(r#""framing":"contents""#));
        assert_eq!(Request::parse(&cl[..cl.len() - 1]).expect("parse"), c);
        // Clay: off when absent (and off the line), defaults for the tones left out.
        assert_eq!(m.clay, None);
        assert!(!String::from_utf8_lossy(&line).contains("clay"));
        let clay = |json: &str| Request::parse(format!(r#"{{"proto":1,"input_len":3,"views":[],"clay":{json}}}"#).as_bytes()).expect("clay").clay;
        assert_eq!(clay("{}"), Some(Clay { shell: CLAY_SHELL, contents: CLAY_CONTENTS }));
        assert_eq!(clay(r#"{"shell":[1,2,3]}"#), Some(Clay { shell: [1, 2, 3], contents: CLAY_CONTENTS }));
        assert_eq!(clay(r#"{"shell":[1,2,3],"contents":[4,5,6]}"#), Some(Clay { shell: [1, 2, 3], contents: [4, 5, 6] }));
        assert!(Request::parse(br#"{"proto":1,"input_len":3,"views":[],"clay":{"shell":[1,2,300]}}"#).is_err());
        let mut k = r.clone();
        k.clay = Some(Clay::default());
        let kl = k.line();
        assert_eq!(Request::parse(&kl[..kl.len() - 1]).expect("parse"), k);
        // Ambient occlusion: off when absent (and off the line), defaults for the fields left out.
        assert_eq!(m.ao, None);
        assert!(!String::from_utf8_lossy(&line).contains(r#""ao""#));
        let ao = |json: &str| Request::parse(format!(r#"{{"proto":1,"input_len":3,"views":[],"ao":{json}}}"#).as_bytes()).expect("ao").ao;
        assert_eq!(ao("{}"), Some(Ao { strength: AO_STRENGTH, radius: AO_RADIUS }));
        assert_eq!(ao(r#"{"strength":0.5}"#), Some(Ao { strength: 0.5, radius: AO_RADIUS }));
        assert_eq!(ao(r#"{"strength":0.25,"radius":0.2}"#), Some(Ao { strength: 0.25, radius: 0.2 }));
        assert!(Request::parse(br#"{"proto":1,"input_len":3,"views":[],"ao":{"radius":"far"}}"#).is_err());
        let mut a = r.clone();
        a.ao = Some(Ao::default());
        let al = a.line();
        assert!(String::from_utf8_lossy(&al).contains(r#""ao":{"#));
        assert_eq!(Request::parse(&al[..al.len() - 1]).expect("parse"), a);
        assert!(Ao::default().check().is_ok() && Ao { strength: 0.0, radius: 1.0 }.check().is_ok());
        for bad in [Ao { strength: -0.1, ..Ao::default() }, Ao { strength: 1.5, ..Ao::default() }, Ao { strength: f32::NAN, ..Ao::default() }, Ao { radius: 0.0, ..Ao::default() }, Ao { radius: 2.0, ..Ao::default() }, Ao { radius: f32::INFINITY, ..Ao::default() }] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        // Ids: off when absent (and off the line), on with `true`.
        assert!(!m.ids);
        assert!(!String::from_utf8_lossy(&line).contains("ids"));
        let mut i = r.clone();
        i.ids = true;
        let il = i.line();
        assert!(String::from_utf8_lossy(&il).contains(r#""ids":true"#));
        assert_eq!(Request::parse(&il[..il.len() - 1]).expect("parse"), i);
        assert!(Request::parse(br#"{"proto":1,"input_len":3,"views":[],"ids":true}"#).expect("ids").ids);
        assert!(Request::parse(br#"{"proto":1,"input_len":3,"views":[],"ids":{}}"#).is_err(), "a flag, not an object");
        assert!(Request::parse(br#"{"proto":2,"input_len":3,"views":[]}"#).is_err());
        assert!(Request::parse(b"not json").is_err());
    }

    #[test]
    fn a_views_camera_crosses_the_wire_bit_for_bit() {
        let s = crate::testgen::camera_scene();
        let mut views = Vec::new();
        for (spec, aspect) in [(ViewSpec::Camera(0), 16.0 / 9.0), (ViewSpec::Auto, 4.0 / 3.0), (ViewSpec::Turntable(5), 1.0)] {
            let f = crate::camera::frame(&s, &[0], spec, aspect).expect("frame");
            let camera = CameraMeta { view: f.view_f32(), proj: f.projection(), fx: f.fx, fy: f.fy, near: f.near, shift_x: f.shift_x, shift_y: f.shift_y };
            assert!(camera.is_finite());
            views.push(ViewMeta { width: 10, height: 10, camera: Some(camera), ..ViewMeta::default() });
        }
        // A content-framed view: a real lens shift, and what it left out.
        let room = crate::testgen::interior_scene();
        let all: Vec<u32> = (0..room.objects.len() as u32).collect();
        let (f, m) = crate::camera::frame_contents(&room, &all, &crate::contents::classify(&room, &all), 4.0 / 3.0).expect("contents");
        assert!(m.contents && f.shift_x != 0.0 && f.shift_y != 0.0);
        let camera = CameraMeta { view: f.view_f32(), proj: f.projection(), fx: f.fx, fy: f.fy, near: f.near, shift_x: f.shift_x, shift_y: f.shift_y };
        let framing = FramingMeta { contents: true, left_out_count: m.left_out.len() as u64, left_out: m.left_out, grown: m.grown };
        views.push(ViewMeta { width: 10, height: 10, camera: Some(camera), framing: Some(framing.clone()), ..ViewMeta::default() });
        let r = Response { ok: true, code: None, detail: None, format: Some(ImageFormat::Webp), views: Some(views.clone()), ..Response::failed(FailureCode::TooLarge, "") };
        let line = r.line();
        let back = Response::parse(&line[..line.len() - 1]).expect("parse").views.expect("views");
        let bits = |c: &CameraMeta| -> Vec<u32> { c.view.iter().chain(&c.proj).chain(&[c.fx, c.fy, c.near, c.shift_x, c.shift_y]).map(|x| x.to_bits()).collect() };
        for (a, b) in views.iter().zip(&back) {
            assert_eq!(bits(&a.camera.expect("sent")), bits(&b.camera.expect("received")));
        }
        assert_eq!(back[3].framing, Some(framing));
        assert!(back[..3].iter().all(|v| v.framing.is_none()));
        assert!(!String::from_utf8_lossy(&Response { views: Some(views[..3].to_vec()), ..r.clone() }.line()).contains("framing"), "absent unless asked");
        let mut inf = views[0].camera.expect("a camera");
        inf.view[12] = f32::INFINITY;
        assert!(!inf.is_finite());
        // A view's id table crosses the wire byte for byte: ids repeated, case kept, spaces kept.
        let ids = IdsMeta {
            image_len: 77,
            count: 4,
            objects: vec![
                IdEntry { index: 0, id: "2O2Fr$t4X7Zf8NOew3FLOH".into() },
                IdEntry { index: 3, id: "2O2Fr$t4X7Zf8NOew3FLOH".into() },
                IdEntry { index: 4, id: "2o2fr$t4x7zf8noew3floh".into() },
                IdEntry { index: 9, id: " Wall \"A\"\u{1}\u{e9} ".into() },
            ],
        };
        let mut with = views[..1].to_vec();
        with[0].ids = Some(ids.clone());
        let wl = Response { views: Some(with), ..r.clone() }.line();
        assert_eq!(Response::parse(&wl[..wl.len() - 1]).expect("parse").views.expect("views")[0].ids, Some(ids));
        assert!(!String::from_utf8_lossy(&line).contains(r#""ids""#), "absent unless asked");
        // A line without it (an older worker) still reads.
        let old = br#"{"proto":1,"ok":true,"format":"webp","views":[{"width":1,"height":1,"ssaa":1,"image_len":0,"objects":0,"triangles":0,"covered_px":0,"without_modifiers":false,"vertex_ms":0,"bin_ms":0,"raster_ms":0,"encode_ms":0}]}"#;
        assert_eq!(Response::parse(old).expect("old").views.expect("views")[0].camera, None);
    }

    #[test]
    fn failure_lines_name_their_code_and_breaches_are_caught() {
        let line = Response::failed(FailureCode::TooLarge, "x").line();
        let s = String::from_utf8(line.clone()).expect("utf-8");
        assert!(s.contains(r#""code":"too_large""#), "{s}");
        assert_eq!(Response::parse(&line[..line.len() - 1]).expect("parse").code, Some(FailureCode::TooLarge));
        assert!(Response::parse(br#"{"proto":1,"ok":true}"#).is_err(), "images without their list is a breach");
        assert!(Response::parse(br#"{"proto":1,"ok":false}"#).is_err(), "a failure without its code is a breach");
        for c in [FailureCode::TooLarge, FailureCode::InvalidInput, FailureCode::Timeout, FailureCode::RendererError] {
            assert_eq!(serde_json::to_value(c).expect("code"), serde_json::Value::String(c.as_str().into()));
        }
    }
}
