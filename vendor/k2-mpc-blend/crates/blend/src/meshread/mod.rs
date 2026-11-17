//! The mesh read: a version's meshes, as triangles, in the K2SCENE1 layout a host's renderer
//! takes. Host-only and pure: bytes in, bytes out, with no I/O, clock or environment.
//!
//! - [`k2scene`]: the K2SCENE1 writer. Every count is checked against [`MeshReadLimits`] and the
//!   output's exact length is computed from the counts before one buffer of that length is
//!   filled; [`k2scene::write_header`] writes the header part alone, with the same checks and
//!   text, for a data section already written. With the test-only `testgen` or `fuzzing` feature, `check` reads a K2SCENE1 back
//!   strictly: the rules a renderer applies, written from the layout's description.
//! - [`tri`]: polygons to triangles with the file's corner order kept: a fan for a convex
//!   polygon, ear clipping for a concave one; always `n - 2` triangles for `n` corners.
//! - [`read`]: the read itself, [`mesh_read`] and [`mesh_read_paired`]: the scene drawn, its
//!   objects (each by its address; world matrices composed from their stored location,
//!   rotation and scale; hidden ones kept and marked; modifiers named), its perspective
//!   cameras, each mesh once with its content hash, and a report of what was left out and why.
//!   Its steps over the decode's objects are shared with the live tier's reuse
//!   ([`crate::live::reuse`]), which keeps a carrier's read ([`read::CarrierRead`]) and writes a
//!   live state's header over its data section.
//! - [`hidden`]: the hidden-by rule, one for the mesh read and the live view.
//! - [`camera`]: a camera's view from its world matrix, its data and the render size, with a
//!   fixed arc tangent.
//! - [`mesh`]: a `Mesh` data-block's positions and polygons, read by the file's own schema from
//!   its attribute layers, or from the older vertex, polygon and corner structs; every array's
//!   length checked against its block before anything is allocated for it. Only the vertices a
//!   triangle uses are kept, renumbered in file order.

pub mod camera;
#[cfg(any(test, feature = "testgen", feature = "fuzzing"))]
pub mod check;
pub mod hidden;
pub mod k2scene;
pub mod mesh;
pub mod read;
pub mod tri;

pub use read::{
    mesh_read, mesh_read_paired, Drawn, DrawnCamera, MeshRead, MeshReport, Skipped,
    MESH_READ_FORMAT,
};

use crate::live::LiveLimits;

/// The bounds one mesh read stays under. The defaults are a renderer's own input limits, so the
/// read never writes what the renderer refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MeshReadLimits {
    /// The live tier's bounds, which the decode stays under (the stored file's cap, the
    /// reader's limits).
    pub live: LiveLimits,
    /// Most meshes written.
    pub max_meshes: u64,
    /// Most objects written.
    pub max_objects: u64,
    /// Most cameras written.
    pub max_cameras: u64,
    /// Most vertices over every mesh written (each once).
    pub max_vertices: u64,
    /// Most triangles over every mesh written (each once).
    pub max_triangles: u64,
    /// Most vertices drawn: each mesh counted once per object that uses it.
    pub max_drawn_vertices: u64,
    /// Most triangles drawn: each mesh counted once per object that uses it.
    pub max_drawn_triangles: u64,
    /// The longest header (its JSON), in bytes.
    pub max_header_bytes: u64,
    /// The longest output, in bytes.
    pub max_output_bytes: u64,
    /// The most corners a polygon is ear clipped with; a larger one is fanned (and counted).
    pub ear_clip_max_corners: u32,
    /// The most heap the read may hold at once, in bytes: worked out from the decoded file's
    /// sizes and the meshes' counts before any mesh array or the output is allocated.
    pub max_heap_bytes: u64,
}

impl MeshReadLimits {
    /// The defaults, with `live` as given.
    pub fn with_live(live: LiveLimits) -> Self {
        Self {
            live,
            ..Self::default()
        }
    }
}

impl Default for MeshReadLimits {
    /// The live tier's bounds, 100,000 meshes and objects, 1,000 cameras, 8 million vertices
    /// and triangles stored and drawn, a 32 MiB header, 256 MiB of output, polygons of up to
    /// 4,096 corners ear clipped, 512 MiB of heap.
    fn default() -> Self {
        Self {
            live: LiveLimits::default(),
            max_meshes: 100_000,
            max_objects: 100_000,
            max_cameras: 1_000,
            max_vertices: 8_000_000,
            max_triangles: 8_000_000,
            max_drawn_vertices: 8_000_000,
            max_drawn_triangles: 8_000_000,
            max_header_bytes: 32 << 20,
            max_output_bytes: 256 << 20,
            ear_clip_max_corners: 4_096,
            max_heap_bytes: 512 << 20,
        }
    }
}

/// Why a mesh read refuses. The text is fixed: it never quotes the file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MeshReadError {
    /// A count is over its limit.
    #[error("too large: {what} {got}, at most {max}")]
    TooLarge {
        /// Which count (fixed text).
        what: &'static str,
        /// The count.
        got: u64,
        /// Its limit.
        max: u64,
    },
    /// The file isn't one the live tier reads: the live tier's decline code (`not_blend`,
    /// `live_version`, `live_ambiguous`...).
    #[error("declined: {code}")]
    Declined {
        /// The decline's code.
        code: &'static str,
    },
    /// What was handed to the writer breaks the layout's rules (a value that isn't finite, an
    /// index past its mesh's vertices...): nothing is written.
    #[error("not writable: {what}")]
    Invalid {
        /// What is wrong (fixed text).
        what: &'static str,
    },
    /// The keys given for the version's objects (a carrier's pairing) don't fit its decode:
    /// none are guessed.
    #[error("the pairing doesn't fit: {what}")]
    Pairing {
        /// What doesn't fit (fixed text).
        what: &'static str,
    },
}

impl MeshReadError {
    /// The refusal's code: `too_large` (a limit, the live tier's size caps included), the
    /// live tier's decline code, `invalid` for a writer check, or `pairing_mismatch`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge { .. }
            | Self::Declined {
                code: "live_too_large",
            } => "too_large",
            Self::Declined { code } => code,
            Self::Invalid { .. } => "invalid",
            Self::Pairing { .. } => "pairing_mismatch",
        }
    }
}

/// `got`, unless it is over `max`.
pub(crate) fn at_most(what: &'static str, got: u64, max: u64) -> Result<u64, MeshReadError> {
    if got > max {
        Err(MeshReadError::TooLarge { what, got, max })
    } else {
        Ok(got)
    }
}
