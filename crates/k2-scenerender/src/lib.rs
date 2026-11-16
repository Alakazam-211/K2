//! K2's 3D stills renderer for live BLEND docs: one `k2-scenerender` process
//! per request draws a scene's views (thumbnails, stills, an 8-frame
//! turntable) on the CPU into WebP lossless or PNG, so a scene that is huge,
//! hostile or broken never takes the daemon with it. All our own code (K2
//! Render's lab spike `scene-still`, ported); no third-party renderer.
//!
//! - [`scene`]: K2SCENE1, the neutral input the BLEND pack's mesh read hands
//!   the host (bounded, typed; the renderer never reads `.blend`);
//! - [`camera`], [`raster`]: the 3/4 auto view (fitted to every object, or
//!   to the contents with a lens shift), the turntable, given cameras;
//!   fixed-point edges, a z-buffer, near clipping, two-light Lambert plus
//!   ambient (the clay look), screen-space ambient occlusion when asked for,
//!   an id image (which object is under each pixel) when asked for, SSAA,
//!   threads by fixed bands;
//! - [`contents`]: which objects are shell (floor, walls) and which contents,
//!   from world boxes alone;
//! - [`srgb`], [`trig`], [`math`]: no libm on the render path, so the bytes are
//!   the same on every CPU;
//! - [`encode`]: WebP lossless (default) and PNG, opaque RGB or transparent
//!   RGBA (coverage alpha, straight colour); id images as 24-bit PNG;
//! - [`proto`], [`render`], [`sandbox`], [`host`]: the wire, the worker's
//!   side, the limits it puts on itself first thing, and the side the
//!   daemon links (posix_spawn, watchdog);
//! - [`digest`]: the containers a scene previews and the raw digest of what
//!   each draws, from the mesh read alone (no object ids);
//! - [`addr`]: their addresses, native addresses in the multiplayer kit's
//!   grammar, and the percent-encoding that puts a name in one;
//! - [`testgen`]: scenes generated in code for the tests (no model files);
//! - [`sha256`]: FIPS 180-4, for the digests (no hash crate).
//!
//! This crate never links k2-core: no tokens, no DB, no OS integration in
//! the process that parses untrusted bytes.

pub mod addr;
pub mod camera;
pub mod contents;
pub mod digest;
pub mod encode;
pub mod host;
pub mod math;
pub mod proto;
pub mod raster;
pub mod render;
pub mod sandbox;
pub mod scene;
pub mod sha256;
pub mod srgb;
pub mod testgen;
pub mod trig;
