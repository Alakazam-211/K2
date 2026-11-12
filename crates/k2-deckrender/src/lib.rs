//! K2's `.pptx` slide renderer: one `k2-deckrender` process per render draws
//! a slide to PNG with the vendored deckcraft (`vendor/deckcraft`, v0.18.1:
//! its reader and its CPU renderer), so a file that crashes, loops or
//! allocates without bound never takes the daemon with it.
//!
//! - [`proto`]: the wire (a JSON header line and the `.pptx` bytes in; a JSON
//!   response line and the PNG out) and the failure codes;
//! - [`worker`]: the process side (read, draw, describe), also used
//!   in-process by the tests and the fuzz targets;
//! - [`host`]: the side the daemon links (spawn, limits per OS, watchdog);
//! - [`fonts`]: the only fonts it draws with (`fonts/manifest.tsv`), never the system's.
//!
//! This crate never links k2-core: no tokens, no DB, no OS integration in
//! the process that parses hostile bytes. Nothing in the daemon calls it yet.

pub mod fonts;
pub mod host;
pub mod proto;
pub mod worker;
