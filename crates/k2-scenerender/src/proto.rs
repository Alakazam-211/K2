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
//! else the camera's, else 4:3.
//!
//! **Response (stdout):** one line of JSON, the [`Response`], then, when `ok`,
//! each view's image in order (`views[i].image_len` bytes each), then EOF.
//! Each view's [`ViewMeta`] carries the camera it was drawn through
//! ([`CameraMeta`]: view and projection matrices, the pixel mapping), left
//! out only when one of its values isn't finite.
//! The worker exits [`EXIT_OK`] after the images, [`EXIT_FAILED`] after a
//! typed failure. Anything else (no line, a malformed one, fewer image bytes
//! than announced, bytes after them) is a breach the host reports as
//! [`FailureCode::RendererError`].

use serde::{Deserialize, Serialize};

use crate::camera::ViewSpec;

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

/// The background when the request names none: sRGB 240, 240, 240, the
/// current neutral (K2 Design picks the final one).
pub const DEFAULT_BACKGROUND: [u8; 3] = [240, 240, 240];

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
}

impl View {
    pub fn new(view: ViewSpec, width: u32) -> View {
        View { view, objects: None, width, aspect: None }
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
        Request { proto: PROTO, input_len: 0, views, format: ImageFormat::Webp, background: None, ssaa: None, threads: 1, hostile: None }
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
    /// Lens shift in NDC units (always 0 for now).
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
        let mut r = Request::new(vec![View::new(ViewSpec::Auto, 320), View { view: ViewSpec::Turntable(3), objects: Some(vec![1, 2]), width: 640, aspect: Some(1.0) }]);
        r.input_len = 99;
        r.background = Some([10, 20, 30]);
        let line = r.line();
        assert_eq!(Request::parse(&line[..line.len() - 1]).expect("parse"), r);
        let min = br#"{"proto":1,"input_len":3,"views":[{"view":{"camera":2},"width":10}]}"#;
        let m = Request::parse(min).expect("defaults");
        assert_eq!((m.format, m.background, m.ssaa, m.threads, m.hostile), (ImageFormat::Webp, None, None, 0, None));
        assert_eq!(m.views[0].view, ViewSpec::Camera(2));
        assert!(Request::parse(br#"{"proto":2,"input_len":3,"views":[]}"#).is_err());
        assert!(Request::parse(b"not json").is_err());
    }

    #[test]
    fn a_views_camera_crosses_the_wire_bit_for_bit() {
        let s = crate::testgen::camera_scene();
        let mut views = Vec::new();
        for (spec, aspect) in [(ViewSpec::Camera(0), 16.0 / 9.0), (ViewSpec::Auto, 4.0 / 3.0), (ViewSpec::Turntable(5), 1.0)] {
            let f = crate::camera::frame(&s, &[0], spec, aspect).expect("frame");
            let camera = CameraMeta { view: f.view_f32(), proj: f.projection(), fx: f.fx, fy: f.fy, near: f.near, shift_x: 0.0, shift_y: 0.0 };
            assert!(camera.is_finite());
            views.push(ViewMeta { width: 10, height: 10, camera: Some(camera), ..ViewMeta::default() });
        }
        let r = Response { ok: true, code: None, detail: None, format: Some(ImageFormat::Webp), views: Some(views.clone()), ..Response::failed(FailureCode::TooLarge, "") };
        let line = r.line();
        let back = Response::parse(&line[..line.len() - 1]).expect("parse").views.expect("views");
        let bits = |c: &CameraMeta| -> Vec<u32> { c.view.iter().chain(&c.proj).chain(&[c.fx, c.fy, c.near, c.shift_x, c.shift_y]).map(|x| x.to_bits()).collect() };
        for (a, b) in views.iter().zip(&back) {
            assert_eq!(bits(&a.camera.expect("sent")), bits(&b.camera.expect("received")));
        }
        let mut inf = views[0].camera.expect("a camera");
        inf.view[12] = f32::INFINITY;
        assert!(!inf.is_finite());
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
