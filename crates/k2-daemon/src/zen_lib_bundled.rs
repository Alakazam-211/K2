//! The bundled widget libraries' bytes, served by the daemon (Gardens on
//! the phone, Rosson Q5, 0.46.6): K2 Companion carries no copy of K2's
//! widget library, so the remote Garden door serves every library, bundled
//! ones included, from these bytes (`zen_remote_routes`, `remote/lib`).
//!
//! The source is K2's own release: the files the desktop app ships in
//! `src/renderer/zen-lib/` (checked in, written by `scripts/vendor-zen-lib.sh`
//! from the pinned npm tarballs, or K2's own kit). Most of them are not an
//! upstream file byte for byte (prebundled to one classic script, rewritten
//! for the sealed frame, or K2's own code), so no public URL serves these
//! exact bytes; they ride in the daemon instead, and every file is checked
//! against its sha256 in `src/shared/zen-lib.json` before it is served.
//!
//! **Generated:** one line per file of every `bundled` row of
//! `src/shared/zen-lib.json`; a test fails when the table and the manifest
//! differ. After a vendor run, regenerate it with
//! `python3 scripts/zen-lib/gen-daemon-libs.py`.

/// `(key, file name, bytes)`, in manifest order.
pub static FILES: &[(&str, &str, &[u8])] = &[
    ("three@0.186.0", "three.js", include_bytes!("../../../src/renderer/zen-lib/three@0.186.0/three.js")),
    ("globe.gl@2.46.2", "globe.gl.min.js", include_bytes!("../../../src/renderer/zen-lib/globe.gl@2.46.2/globe.gl.min.js")),
    ("cannon-es@0.20.0", "cannon-es.js", include_bytes!("../../../src/renderer/zen-lib/cannon-es@0.20.0/cannon-es.js")),
    ("pixi.js@8.21.0", "pixi.min.js", include_bytes!("../../../src/renderer/zen-lib/pixi.js@8.21.0/pixi.min.js")),
    ("pixi.js@8.21.0", "unsafe-eval.min.js", include_bytes!("../../../src/renderer/zen-lib/pixi.js@8.21.0/unsafe-eval.min.js")),
    ("phaser@4.2.1", "phaser.min.js", include_bytes!("../../../src/renderer/zen-lib/phaser@4.2.1/phaser.min.js")),
    ("kaplay@3001.0.19", "kaplay.js", include_bytes!("../../../src/renderer/zen-lib/kaplay@3001.0.19/kaplay.js")),
    ("matter-js@0.20.0", "matter.min.js", include_bytes!("../../../src/renderer/zen-lib/matter-js@0.20.0/matter.min.js")),
    ("d3@7.9.0", "d3.min.js", include_bytes!("../../../src/renderer/zen-lib/d3@7.9.0/d3.min.js")),
    ("d3@7.9.0", "d3-k2.js", include_bytes!("../../../src/renderer/zen-lib/d3@7.9.0/d3-k2.js")),
    ("chart.js@4.5.1", "chart.umd.min.js", include_bytes!("../../../src/renderer/zen-lib/chart.js@4.5.1/chart.umd.min.js")),
    ("echarts@6.1.0", "echarts.min.js", include_bytes!("../../../src/renderer/zen-lib/echarts@6.1.0/echarts.min.js")),
    ("uplot@1.6.32", "uPlot.iife.min.js", include_bytes!("../../../src/renderer/zen-lib/uplot@1.6.32/uPlot.iife.min.js")),
    ("uplot@1.6.32", "uplot-style.js", include_bytes!("../../../src/renderer/zen-lib/uplot@1.6.32/uplot-style.js")),
    ("animejs@4.5.0", "anime.umd.min.js", include_bytes!("../../../src/renderer/zen-lib/animejs@4.5.0/anime.umd.min.js")),
    ("lottie-web@5.13.0", "lottie.min.js", include_bytes!("../../../src/renderer/zen-lib/lottie-web@5.13.0/lottie.min.js")),
    ("perfect-freehand@1.2.3", "perfect-freehand.js", include_bytes!("../../../src/renderer/zen-lib/perfect-freehand@1.2.3/perfect-freehand.js")),
    ("roughjs@4.6.6", "rough.js", include_bytes!("../../../src/renderer/zen-lib/roughjs@4.6.6/rough.js")),
    ("howler@2.2.4", "howler.min.js", include_bytes!("../../../src/renderer/zen-lib/howler@2.2.4/howler.min.js")),
    ("tone@15.1.22", "Tone.js", include_bytes!("../../../src/renderer/zen-lib/tone@15.1.22/Tone.js")),
    ("leaflet@1.9.4", "leaflet.js", include_bytes!("../../../src/renderer/zen-lib/leaflet@1.9.4/leaflet.js")),
    ("leaflet@1.9.4", "leaflet-style.js", include_bytes!("../../../src/renderer/zen-lib/leaflet@1.9.4/leaflet-style.js")),
    ("leaflet@1.9.4", "leaflet-k2.js", include_bytes!("../../../src/renderer/zen-lib/leaflet@1.9.4/leaflet-k2.js")),
    ("marked@18.0.14", "marked.umd.js", include_bytes!("../../../src/renderer/zen-lib/marked@18.0.14/marked.umd.js")),
    ("dompurify@3.4.17", "purify.min.js", include_bytes!("../../../src/renderer/zen-lib/dompurify@3.4.17/purify.min.js")),
    ("highlight.js@11.12.0", "highlight.min.js", include_bytes!("../../../src/renderer/zen-lib/highlight.js@11.12.0/highlight.min.js")),
    ("highlight.js@11.12.0", "highlight-style.js", include_bytes!("../../../src/renderer/zen-lib/highlight.js@11.12.0/highlight-style.js")),
    ("katex@0.18.9", "katex.min.js", include_bytes!("../../../src/renderer/zen-lib/katex@0.18.9/katex.min.js")),
    ("katex@0.18.9", "auto-render.min.js", include_bytes!("../../../src/renderer/zen-lib/katex@0.18.9/auto-render.min.js")),
    ("katex@0.18.9", "katex-style.js", include_bytes!("../../../src/renderer/zen-lib/katex@0.18.9/katex-style.js")),
    ("preact@10.29.8", "preact-htm.js", include_bytes!("../../../src/renderer/zen-lib/preact@10.29.8/preact-htm.js")),
    ("lit@3.3.3", "lit.js", include_bytes!("../../../src/renderer/zen-lib/lit@3.3.3/lit.js")),
    ("font-caveat@5.3.0", "caveat-style.js", include_bytes!("../../../src/renderer/zen-lib/font-caveat@5.3.0/caveat-style.js")),
    ("font-caveat@5.3.0", "caveat-k2.js", include_bytes!("../../../src/renderer/zen-lib/font-caveat@5.3.0/caveat-k2.js")),
    ("k2-help@1.0.0", "k2-help.js", include_bytes!("../../../src/renderer/zen-lib/k2-help@1.0.0/k2-help.js")),
    ("k2-activity@1.0.0", "k2-activity.js", include_bytes!("../../../src/renderer/zen-lib/k2-activity@1.0.0/k2-activity.js")),
    ("k2-thread@1.1.0", "k2-thread.js", include_bytes!("../../../src/renderer/zen-lib/k2-thread@1.1.0/k2-thread.js")),
    ("k2-markdown@1.0.0", "k2-markdown.js", include_bytes!("../../../src/renderer/zen-lib/k2-markdown@1.0.0/k2-markdown.js")),
    ("k2-png@1.0.0", "k2-png.js", include_bytes!("../../../src/renderer/zen-lib/k2-png@1.0.0/k2-png.js")),
];

/// The bytes of `file` in the bundled library `key` (`<id>@<version>`).
pub fn file(key: &str, name: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(k, n, _)| *k == key && *n == name).map(|(_, _, b)| *b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// The table is the manifest's bundled files, byte for byte (sha256 and
    /// size), and nothing else.
    #[test]
    fn the_table_is_every_bundled_file_of_the_manifest() {
        let m = k2_core::zen::stdlib::zen_lib_manifest();
        let mut want = Vec::new();
        for e in m.libs.iter().filter(|e| e.source == k2_core::zen::stdlib::LibSource::Bundled) {
            for f in &e.files {
                want.push((e.key(), f.name.clone()));
                let bytes = file(&e.key(), &f.name).unwrap_or_else(|| panic!("{}/{} is missing from the table; regenerate it", e.key(), f.name));
                assert_eq!(bytes.len() as u64, f.bytes, "{}/{} size", e.key(), f.name);
                let sum: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(sum, f.sha256, "{}/{} sha256", e.key(), f.name);
            }
        }
        let have: Vec<(String, String)> = FILES.iter().map(|(k, n, _)| (k.to_string(), n.to_string())).collect();
        assert_eq!(have, want, "the table and src/shared/zen-lib.json differ; regenerate the table");
    }
}
