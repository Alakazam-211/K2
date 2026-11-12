//! The bundled fonts: every file `fonts/manifest.tsv` lists is the pinned upstream file (sha256),
//! with its licence files beside it; the bytes compiled in are those files; and the renderer
//! loads exactly the listed families, no system font.

use std::path::Path;

use k2_deckrender::fonts::{self, FONTS};

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn every_listed_file_matches_its_pin_and_carries_its_licence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!FONTS.is_empty());
    let bundled: Vec<String> = deckcraft_fonts::bundled().iter().map(|b| sha256_hex(b)).collect();
    let mut total = 0usize;
    for f in FONTS {
        let path = root.join(f.file);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", f.file));
        assert_eq!(sha256_hex(&bytes), f.sha256, "{} matches its pin", f.file);
        total += bytes.len();
        let dir = path.parent().expect("folder");
        for lic in f.licence_files {
            assert!(dir.join(lic).is_file(), "{}: licence file {lic} beside it", f.file);
        }
        assert!(!f.licence.is_empty() && !f.version.is_empty() && f.source.starts_with("https://"), "{}: licence, version, source", f.file);
        match f.bytes {
            Some(b) => assert_eq!(sha256_hex(b), f.sha256, "{}: the compiled-in bytes are the pinned file", f.file),
            None => assert!(bundled.contains(&f.sha256.to_string()), "{}: compiled in by deckcraft-fonts", f.file),
        }
    }
    // deckcraft-fonts compiles in nothing the manifest doesn't list.
    for b in &bundled {
        assert!(FONTS.iter().any(|f| f.sha256 == b), "deckcraft-fonts carries an unlisted font ({b})");
    }
    eprintln!("[fonts] {} files, {} bytes, families {:?}", FONTS.len(), total, fonts::manifest_families());
}

#[test]
fn the_renderer_loads_exactly_the_listed_families() {
    let loaded = fonts::init();
    assert!(loaded.ours, "the fonts were set up before anything used deckcraft's font database");
    let db = deckcraft_fonts::FontDb::global();
    let mut have: Vec<String> = db.families();
    have.sort_by_key(|a| a.to_lowercase());
    let mut want: Vec<String> = fonts::manifest_families().into_iter().map(str::to_string).collect();
    want.sort_by_key(|a| a.to_lowercase());
    assert_eq!(have, want, "no system font, nothing unlisted");
    assert!(!db.has_family("DejaVu Sans") && !db.has_family("Helvetica") && !db.has_family("Arial"), "never the system's fonts");
    eprintln!("[fonts] set up in {:?}: {} families, {} faces", loaded.took, loaded.families, loaded.faces);
}
