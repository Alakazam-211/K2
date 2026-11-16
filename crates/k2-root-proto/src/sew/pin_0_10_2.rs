//! Sew 0.10.2: noise removal on by default (DeepFilterNet3, with RNNoise as
//! the fallback, in the client crates), a faster first video picture when
//! joining a running call, and WebRTC dependency updates (the str0m fork,
//! sew-str0m 0.25.3). The pin for K2 0.46.13 (0.10.0 stays: an upgrade
//! source and rollback target).
//!
//! Provenance: sew-releases tag v0.10.2, core commit
//! `5c2c29254924cb2e2fcb8ba6fcad44bceb656c87` (release.json `git_sha`),
//! SHA256SUMS sha256 `1b1ae068…ae33`, release.json sha256 `6e22bce8…0b2a`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core tarball and
//! binary, sidecar tarball, kit, model and DeepFilterNet3 hash in Sew's pin
//! note is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.10.2.SHA256SUMS` and a test holds this entry to it.
//!
//! No migration since 0.10.0 (schema max 1000101), so a rollback to 0.10.0
//! is the plain `k2 hostchat rollback`; from 0.9.x the upgrade still crosses
//! 1000101 (`ROLLBACK_ACROSS_SCHEMA_HINT`). Features: 45, the same as 0.10.0.
//! The same three sidecars, no new root-helper verb, and the same speech
//! model file as 0.10.0's (base-q8_0). The release also publishes
//! `sew-dfn3-0.10.2.tar.gz`, the DeepFilterNet3 weights apps bundle
//! ([`super::SEW_DFN3`]; the desktop app's build fetches it with
//! `scripts/fetch-dfn3.sh`; boxes never install it). Client crates:
//! sew-call-client and tauri-plugin-sew-voice 0.8.0, the rest at 0.10.2.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.10.2 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_10_2: SewRelease = SewRelease {
    version: "0.10.2",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "170f9564ca1cea0c7bd92bd8cf1be0bab4d7822bf7a0c2410fe78006c9bfdfd3",
            binary_sha256: "8ee7eb268cadbe11f5f91a214db6ae7be6534764e4d2d1af9b6327d65bae0901",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "846ae420121a8bfb061519d8e2a69e1e1b20b9659f9965f758cbdb7fc8f38aa2",
            binary_sha256: "43213a2387d60fe7bcb7dc609222004ce5bcff816d4c83a7d938606329333fe5",
        },
    ],
    media: Some(SewSidecar {
        version: "0.10.2",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "0a01057e7c6a834d6d54e5cac8ea520a84bdd6b833c0a83e9577a1d343d27018",
                binary_sha256: "46754f051dd6c97fd66e390f34490de10cab6c73caa2306919d93a7f2a49d886",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "aecc84fe3ad805055b594953c5d057328a548e906c1163eb1e641d72d766a4e0",
                binary_sha256: "ff7b9f0c520d7e1994afcdc346ca660e5a7789e085f12bcc1fd54f72576aff1b",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.10.2",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "7ef1508b12539af202a2a0075d3ddc3d0c2a4394bb16b70454cfadb9f8d91b96",
                binary_sha256: "272d95620df2ba6f709ec0b4839b7ec49e201ebfbb63f94a41c8c243e7ddf5e8",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "5b443957de94a9a853773eebb294b4444a441e9d3c613b363ff354cf277fafc1",
                binary_sha256: "9da3a201369be02127cdccef1ccf7a12258d925be314726a74d0a768de2aee0f",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.10.2",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "d2abc955ef395db085ff076b51ddcde305475a7f356ef0c65ffdeae15eeccaf6",
                binary_sha256: "6647de7f4e80288513f2bc42799b4b1940bda574fb5e18c051cbeadbfbcc7963",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "7f4ec4b70cd6a4e8cb265bb433b3d9657b34c4cdf2a715a00c785800cce6c090",
                binary_sha256: "b94f5f03fde91d17640a9038701d62be761b663aa5395fd5d02c51fac37d50dc",
            },
        ],
    }),
};
