//! The DeepFilterNet3 weights a Sew release publishes for apps (Sew 0.10.2
//! on): `sew-dfn3-<v>.tar.gz`, four members under `dfn3/` (the weights
//! `dfn3_ll.nnef.tar`, their `NOTICE`, `LICENSE-MIT` and `LICENSE-APACHE`).
//! The desktop app bundles the four under `<resources>/dfn3/`, where
//! tauri-plugin-sew-voice's `dfn3` feature loads the weights; boxes never
//! install it (it is no root-helper input).
//!
//! This table is the record; `scripts/fetch-dfn3.sh` (the build step that
//! downloads, checks and stages the bundle) carries the same numbers, and a
//! test holds the two equal.

/// One release's DeepFilterNet3 bundle, as its `release.json` `dfn3` and
/// `SHA256SUMS` list it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SewDfn3 {
    /// The Sew release that published it.
    pub version: &'static str,
    /// The asset's name on the release.
    pub file: &'static str,
    /// The asset's sha256.
    pub sha256: &'static str,
    /// The asset's size.
    pub bytes: u64,
    /// The sha256 of its weights member, `dfn3/dfn3_ll.nnef.tar`.
    pub weights_sha256: &'static str,
}

impl SewDfn3 {
    /// The weights member as `SHA256SUMS` names it
    /// (`sew-dfn3-<v>/dfn3/dfn3_ll.nnef.tar`).
    pub fn weights_member(&self) -> String {
        format!("{}/{DFN3_WEIGHTS_PATH}", self.file.trim_end_matches(".tar.gz"))
    }
}

/// The weights inside the asset, and under the app's resource directory.
pub const DFN3_WEIGHTS_PATH: &str = "dfn3/dfn3_ll.nnef.tar";

/// The four files the app bundles, under its resource directory.
pub const DFN3_BUNDLE_FILES: [&str; 4] = ["dfn3/dfn3_ll.nnef.tar", "dfn3/NOTICE", "dfn3/LICENSE-MIT", "dfn3/LICENSE-APACHE"];

/// Every DeepFilterNet3 bundle K2 has pinned, oldest first; the last is the
/// one the app ships.
/// Sew 0.10.3's, 0.10.4's, 0.10.5's, 0.10.6's, 0.11.1's, 0.11.3's and 0.11.4's
/// assets are the same bytes as 0.10.2's (each with its own name, on its own release).
pub const SEW_DFN3: &[SewDfn3] = &[
    SewDfn3 {
        version: "0.10.2",
        file: "sew-dfn3-0.10.2.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.10.3",
        file: "sew-dfn3-0.10.3.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.10.4",
        file: "sew-dfn3-0.10.4.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.10.5",
        file: "sew-dfn3-0.10.5.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.10.6",
        file: "sew-dfn3-0.10.6.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.11.1",
        file: "sew-dfn3-0.11.1.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.11.3",
        file: "sew-dfn3-0.11.3.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
    SewDfn3 {
        version: "0.11.4",
        file: "sew-dfn3-0.11.4.tar.gz",
        sha256: "153721fd5f9f33663da7d831b3b611adcb9ca9e65233a6128d94941ad48d6697",
        bytes: 36_353_582,
        weights_sha256: "b6e52c0bc66b82e42e3403d22cdd3b847143026d6a41cc4380a97b044f7766af",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The build step fetches exactly the bundle this table pins: the same
    /// release tag, asset, size and both hashes, and stages the same four
    /// files. A pin bump that forgets the script fails here.
    #[test]
    fn the_fetch_script_carries_the_pinned_bundle() {
        let script = include_str!("../../../../scripts/fetch-dfn3.sh");
        let d = SEW_DFN3.last().expect("a DeepFilterNet3 pin");
        let line = |key: &str| -> String {
            let prefix = format!("{key}=\"");
            let l = script
                .lines()
                .find(|l| l.starts_with(&prefix))
                .unwrap_or_else(|| panic!("fetch-dfn3.sh sets {key}"));
            l[prefix.len()..].trim_end_matches('"').to_string()
        };
        assert_eq!(line("DFN3_SEW_VERSION"), d.version);
        assert_eq!(line("DFN3_ASSET"), d.file);
        assert_eq!(line("DFN3_ASSET_SHA256"), d.sha256);
        assert_eq!(line("DFN3_ASSET_BYTES"), d.bytes.to_string());
        assert_eq!(line("DFN3_WEIGHTS_SHA256"), d.weights_sha256);
        for f in DFN3_BUNDLE_FILES {
            assert!(script.contains(f), "fetch-dfn3.sh stages {f}");
        }
    }

    /// The app ships the newest Sew's bundle (a pin bump that publishes a new
    /// one appends it), and its version is a real release K2 knows.
    #[test]
    fn the_shipped_bundle_is_a_known_release() {
        let d = SEW_DFN3.last().expect("a DeepFilterNet3 pin");
        assert!(super::super::SEW_RELEASES.iter().any(|r| r.version == d.version), "{}", d.version);
        assert_eq!(d.weights_member(), format!("sew-dfn3-{}/dfn3/dfn3_ll.nnef.tar", d.version));
        for x in SEW_DFN3 {
            assert!(x.sha256.len() == 64 && x.weights_sha256.len() == 64, "{x:?}");
            assert_ne!(x.sha256, x.weights_sha256);
        }
    }
}
