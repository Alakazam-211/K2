//! The renderer's fonts: exactly the families `fonts/manifest.tsv` lists (pinned upstream files),
//! never the system's, so a slide renders the same on every host.
//!
//! [`init`] (once per process, before anything draws) makes the process-wide deckcraft font
//! database one with no font folders and no system fallback, adds the manifest's fonts this crate
//! compiles in (the vendored deckcraft-fonts compiles in its last-resort Liberation Sans), and
//! loads every face of every family, so the first render in a process draws exactly as later ones
//! (deckcraft picks a fallback face among the faces already loaded).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use deckcraft_fonts::FontDb;

/// One file of the manifest.
#[derive(Debug, Clone, Copy)]
pub struct BundledFont {
    pub family: &'static str,
    pub style: &'static str,
    /// Relative to crates/k2-deckrender.
    pub file: &'static str,
    pub sha256: &'static str,
    pub version: &'static str,
    pub source: &'static str,
    pub licence: &'static str,
    /// Beside the font file.
    pub licence_files: &'static [&'static str],
    pub reserved_font_names: &'static str,
    /// `None`: compiled in by deckcraft-fonts (`deckcraft_fonts::bundled`).
    pub bytes: Option<&'static [u8]>,
}

include!(concat!(env!("OUT_DIR"), "/fonts_manifest.rs"));

/// What [`init`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loaded {
    /// The database is ours (no font folders). `false`: something used deckcraft's global
    /// database first (in-process tests); system fallback is still turned off on it.
    pub ours: bool,
    pub families: usize,
    pub faces: usize,
    /// Making the database and loading every face.
    pub took: Duration,
}

/// Set up the fonts (once per process; later calls return the first result).
pub fn init() -> Loaded {
    static DONE: OnceLock<Loaded> = OnceLock::new();
    *DONE.get_or_init(|| {
        let started = Instant::now();
        let db = FontDb::with_font_dirs(Vec::new());
        db.set_system_fallback(false);
        add(&db);
        let ours = FontDb::init_global(db);
        let g = FontDb::global();
        if !ours {
            g.set_system_fallback(false);
            add(g);
        }
        let (families, faces) = preload(g);
        Loaded { ours, families, faces, took: started.elapsed() }
    })
}

fn add(db: &FontDb) {
    for f in FONTS {
        if let Some(b) = f.bytes {
            db.add_static_font(b);
        }
    }
}

/// Load every face of every family the database knows, in a fixed order.
pub fn preload(db: &FontDb) -> (usize, usize) {
    let families = db.families();
    let mut faces = 0;
    for fam in &families {
        for style in db.styles(fam) {
            let _ = db.face(fam, &style);
            faces += 1;
        }
    }
    (families.len(), faces)
}

/// The families the manifest lists, sorted, deduplicated.
pub fn manifest_families() -> Vec<&'static str> {
    let mut v: Vec<&str> = FONTS.iter().map(|f| f.family).collect();
    v.sort_unstable();
    v.dedup();
    v
}
