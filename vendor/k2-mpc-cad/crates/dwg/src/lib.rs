//! Reads DWG files for the multiplayer kit, read-only: the 2004 to 2018 family
//! (`AC1018`, `AC1024`, `AC1027`, `AC1032`).
//!
//! - [`bits`]: the bit-coded values the file's sections are written in, read from a bit stream
//!   that refuses, typed, to read past its end;
//! - [`crc`]: the CRC-16, the CRC-32 and the page checksum the container is checked with;
//! - [`lz77`]: the container's compression, decompressed into no more than the size a page
//!   declares, with a bound on that size;
//! - [`container`]: the file header, the page map with its gaps, the section map and the data
//!   pages, every check made ([`read_container`], [`read_section`]). An encrypted or
//!   password-protected drawing is refused;
//! - [`objects`]: the object map and each object's head (size, type, own handle, CRC-16);
//! - [`classes`]: the class list, its CRC-16 and closing sentinel checked;
//! - [`frame`]: one object's data, string and handle streams, its extension data, and the
//!   data every object and every entity begins with;
//! - [`entities`]: each entity kind's name, and the fields of the kinds this reader decodes;
//! - [`polylines`]: the old polylines (2D, 3D, face meshes, meshes) and their vertices;
//! - [`leaders`]: leaders, and multileaders read into the drawing model's leader content;
//! - [`header`]: the header variables the drawing-model read uses, and the measurement system;
//! - [`tables`]: the control objects, layers, linetypes, text styles, registered
//!   applications, dimension styles, block records and layouts;
//! - [`drawing`]: the drawing for the drawing-model read ([`drawing::DwgDrawing`], a
//!   `DrawSource`), so `draw header` and `draw page` read a DWG file as they read a DXF one.
//!
//! The format's facts come from the Open Design Specification for .dwg files (version 5.4.1),
//! read as a source of facts; the code and its tables are our own (`PROVENANCE.md`).
#![forbid(unsafe_code)]

pub mod bits;
pub mod classes;
pub mod container;
pub mod crc;
pub mod drawing;
pub mod entities;
pub mod error;
pub mod frame;
pub mod header;
pub mod leaders;
pub mod lz77;
pub mod objects;
pub mod polylines;
pub mod tables;

pub use container::{read_container, read_section, Container, Gap, Page, Section, SectionPage};
pub use k2_mpc_cad_model::error::CadError;
pub use k2_mpc_cad_model::mutant;

/// The reader's bounds: a file or a page over one is refused, typed ([`error::OVER_BOUND`]),
/// before anything its size names is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The file's size.
    pub max_bytes: u64,
    /// One page's decompressed size.
    pub max_page: usize,
    /// One section's decompressed size.
    pub max_section: u64,
    /// Entries in the object map.
    pub max_objects: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 1 << 30,
            max_page: 1 << 24,
            max_section: 1 << 28,
            max_objects: 1 << 22,
        }
    }
}
