//! Sew 0.10.4: each video stream's lowest layer keeps running while anyone
//! wants it, so a viewer on a weak link gets low-quality video instead of
//! none (a bitrate cap no longer takes it below its floor); on phones
//! DeepFilterNet3's step-down has a 3 s grace window at call start, audio
//! session activation, route changes, camera start and app scene changes.
//! The pin for K2 0.46.15 (0.10.3, 0.10.2 and 0.10.0 stay: upgrade sources
//! and rollback targets).
//!
//! Provenance: sew-releases tag v0.10.4, core commit
//! `5662350605dee0f6c94c896a112ffe8b33a4db5c` (release.json `git_sha`),
//! SHA256SUMS sha256 `8fdcdf24…b1f0`, release.json sha256 `38c0a378…72db`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core tarball and
//! binary, sidecar tarball, kit, model and DeepFilterNet3 hash in Sew's pin
//! note is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.10.4.SHA256SUMS` and a test holds this entry to it.
//!
//! No migration since 0.10.0 (schema max 1000101), so a rollback to 0.10.3,
//! 0.10.2 or 0.10.0 is the plain `k2 hostchat rollback`; from 0.9.x the
//! upgrade still crosses 1000101 (`ROLLBACK_ACROSS_SCHEMA_HINT`). The mgmt
//! API is unchanged (only the golden versions moved). Upgrade sew and
//! sew-media together (K2 always does). The same three sidecars, no new
//! root-helper verb, and the same speech model file as 0.10.3's (base-q8_0).
//! Its DeepFilterNet3 asset `sew-dfn3-0.10.4.tar.gz` holds the same bytes as
//! 0.10.3's ([`super::SEW_DFN3`]). Client crates: sew-call-client and
//! tauri-plugin-sew-voice 0.8.2 (mirror `f4fc904d`), the rest at 0.10.4.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.10.4 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_10_4: SewRelease = SewRelease {
    version: "0.10.4",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "621c6531d75c62906158fdfd7183b25455e6ffa14aba038a78e689b40773629b",
            binary_sha256: "7e78c2d747e317492400c9dcced352b3c2e4223af242a117ec3f238cba942c1e",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "a7ed703ff6045bb9bf06ae2016cc1249b00d8dab4aa7f40d8c9ace3394763c81",
            binary_sha256: "dea46132ea5f930ec64d85126e51982c32ca89513d6ed3175dd3684ebe08cd02",
        },
    ],
    media: Some(SewSidecar {
        version: "0.10.4",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "bf30774074c9df722f30be38378a61caf868687e4a5f3601f0b4747ce0069d7e",
                binary_sha256: "9bf7484b22a863663e7966422436b8e929bcee78918e7f90f31646824cc3c277",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "61aae7d3428ef727a4cdbeb60bc31594342266f84b3d4d72cc332c47224b17ea",
                binary_sha256: "f8107e32b8c80b4736304507e501e480fe49dfafe94293bf588759e9ee040031",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.10.4",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "5f319fe781cb7b913af13933b305d8ce2571e70b2da5dd4b3084ba851d9fee81",
                binary_sha256: "760713f1d388aa0bce36e44af962a08850eda8128bff66d1c0fd283248728ed2",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "ab6235d05508523436426f6bf058c1e9c1c220e50665de059e1c80343e37edd5",
                binary_sha256: "af6c80e383f3ae7a10a11c2689dd8c1ffd6304d74d3a69cbb040aa5b13a447c0",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.10.4",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "2ae6b1fb351179868dff5ab03bded2fdfa88feb671d766a2b61c85b840f1fdb7",
                binary_sha256: "7bfb4b6bccc1044a1f96d84c916df24f708aeae8b23cbb142a78c58c61f653c8",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "a3978790f59001d8577fa116acda54c2675f8d48c850ba3f584d70b5a501d536",
                binary_sha256: "15c1b867fb444ff83688df0651429b5b366ea5315bf68be291a7a0d054b6ec0f",
            },
        ],
    }),
};
