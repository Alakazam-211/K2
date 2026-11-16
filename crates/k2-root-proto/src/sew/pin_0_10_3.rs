//! Sew 0.10.3: noise removal's DeepFilterNet3 loads on a call's first use
//! and frees its memory after an idle time (desktop runs RNNoise until it is
//! ready), and on phones Off captures unprocessed audio. The pin for K2
//! 0.46.13 (0.10.2 and 0.10.0 stay: upgrade sources and rollback targets).
//!
//! Provenance: sew-releases tag v0.10.3, core commit
//! `6c93eed9f83923508165fe81aab416c1926229dd` (release.json `git_sha`),
//! SHA256SUMS sha256 `c5d18339…7c67`, release.json sha256 `5fc2237c…425a`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core tarball and
//! binary, sidecar tarball, kit, model and DeepFilterNet3 hash in Sew's pin
//! note is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.10.3.SHA256SUMS` and a test holds this entry to it.
//!
//! No migration since 0.10.0 (schema max 1000101), so a rollback to 0.10.2 or
//! 0.10.0 is the plain `k2 hostchat rollback`; from 0.9.x the upgrade still
//! crosses 1000101 (`ROLLBACK_ACROSS_SCHEMA_HINT`). Features: 45, the same as
//! 0.10.2; the mgmt API is 0.10.2's. The same three sidecars, no new
//! root-helper verb, and the same speech model file as 0.10.2's (base-q8_0).
//! Its DeepFilterNet3 asset `sew-dfn3-0.10.3.tar.gz` holds the same bytes as
//! 0.10.2's ([`super::SEW_DFN3`]). Client crates: sew-call-client and
//! tauri-plugin-sew-voice 0.8.1 (mirror `c9c25f5c`), the rest at 0.10.3.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.10.3 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_10_3: SewRelease = SewRelease {
    version: "0.10.3",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "be1ee72bd949918fab9b70df847f2c45f18d5af4281454fd0d94a4336814b68c",
            binary_sha256: "47ab234bb14b3d04e629f3a9722da9974e8d6cbd6f88ffff6adff12d1c90d2e3",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "b79cebeb2410c1bcba3edfd9d972b6cc847572911303f8bb329badf185dd7564",
            binary_sha256: "6997f5ffc26154b0f85a494027983b4cb5608496384e378e07bfbec7f59399d3",
        },
    ],
    media: Some(SewSidecar {
        version: "0.10.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "7dcd0033f2bab3bda78404fe96d012bd377c0ed7424dca153ecc9115c9c07029",
                binary_sha256: "d3d9aacb340bee5dcbf559569c57d88dbb895d88b7426c96501686ee6d776bc7",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "134a2c98a9daf39132a4ead2cdb7cc4ff45cc5d7ae0e78e14b23b240d2bea2c3",
                binary_sha256: "f32fe0f3938fa3e1391d3f1e4a2a9e650a37517c23155df75f317d945f3e6cf3",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.10.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "2bb8c5cc113a74edd07fa69ebd482b953762b7bf00b3a19e266120aa3b768a1f",
                binary_sha256: "7390990d2e82ede103b24169605d89696102acb3f1bd4ebbe6bb1946d4a1ec51",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "2916e2eb469a322eecc9bf05871ade681d1f1f1f91c7fd3b5895f8b5e4b154d9",
                binary_sha256: "f8d34a4cd21175e191e3f195c596ef165f8c4a8c2d4160d6c9ac794e87091a73",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.10.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "77aee904785ec93b2436e12a237dab130caa22b2f0fd135c5c19d8dab7b68124",
                binary_sha256: "5451425bbe4b566b43ff7ef0920cab342768dfd7ac052952f1d15e223f9f3be5",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "7be1fbd82ae2d605b8b567c14cab47d29cfc9d6cd36de36907507541ef68f40c",
                binary_sha256: "cc6354de7597dbb2fdb7f365a986429e9a70dc4f8a8644f228789631eb8e6929",
            },
        ],
    }),
};
