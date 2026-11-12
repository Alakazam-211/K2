//! Compiles in the fonts `fonts/manifest.tsv` lists for this crate (`compiled_in` =
//! `k2-deckrender`) and writes the manifest as a table (`fonts.rs` reads it). Reads only local
//! files, and only from under `fonts/` (src/font_path.rs); tests/fonts.rs checks every file
//! against its sha256 pin.
use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "src/font_path.rs"]
mod font_path;

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let manifest = dir.join("fonts/manifest.tsv");
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));
    let mut src = String::from("pub static FONTS: &[BundledFont] = &[\n");
    for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let [family, style, file, sha256, version, source, licence, licence_files, rfn, compiled_in] = f.as_slice() else {
            panic!("fonts/manifest.tsv: a row needs 10 tab-separated fields: {line}");
        };
        assert!(sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()), "bad sha256 in {line}");
        let bytes = match *compiled_in {
            "deckcraft" => "None".to_string(),
            "k2-deckrender" => {
                let path = font_path::compiled_in_font_path(&dir, file).unwrap_or_else(|e| panic!("fonts/manifest.tsv: {e}"));
                println!("cargo::rerun-if-changed={}", path.display());
                format!("Some(include_bytes!({:?}))", path.display().to_string())
            }
            other => panic!("fonts/manifest.tsv: compiled_in is deckcraft or k2-deckrender, not {other}"),
        };
        let lic: Vec<String> = licence_files.split(',').map(|s| format!("{:?}", s.trim())).collect();
        let _ = writeln!(
            src,
            "    BundledFont {{ family: {family:?}, style: {style:?}, file: {file:?}, sha256: {sha256:?}, version: {version:?}, source: {source:?}, licence: {licence:?}, licence_files: &[{}], reserved_font_names: {rfn:?}, bytes: {bytes} }},",
            lic.join(", ")
        );
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("fonts_manifest.rs");
    std::fs::write(&out, src).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}
