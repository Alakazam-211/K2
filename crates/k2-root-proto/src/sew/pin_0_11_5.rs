//! Sew 0.11.5, the pin for K2 0.46.18 (0.11.4, 0.11.3, 0.11.1, 0.10.6,
//! 0.10.5, 0.10.4, 0.10.3, 0.10.2 and 0.10.0 stay: upgrade sources and
//! rollback targets).
//!
//! What 0.11.5 changes: security updates in its TLS and mail libraries
//! (rustls's key-exchange downgrade fix and its emSign root distrust, and the
//! lettre and zstd advisories). Two new feature flags, `feed_attachment_info`
//! and `message_get`, both off unless turned on (K2 turns on neither). No new
//! migration, contracts unchanged, and the systemd unit Sew generates is
//! byte-identical to 0.11.4's.
//!
//! Provenance: sew-releases tag v0.11.5, core commit
//! `52f537689e370ef5dde772c2e29b2eddec6029b6` (release.json `git_sha`),
//! SHA256SUMS sha256 `c55d50be…f62c`, release.json sha256 `1749c593…c22c`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core and sidecar
//! tarball, core binary, kit, model and DeepFilterNet3 hash in Sew's pin note
//! (core pins from sew-0.11.5-release.json, PINS-CHECK PASS) is equal. The
//! SUMS file is vendored as `testdata/sew/sew-0.11.5.SHA256SUMS` and a test
//! holds this entry to it.
//!
//! Migrations: still 0.11.0's two (release.json `schema_max` 1100101, as
//! 0.11.4's); 0.11.5 adds none. From 0.11.4, 0.11.3 or 0.11.1 the upgrade
//! and the way back are plain ones. The way back from it to 0.10.6 or any
//! earlier release crosses 0.11.0's two (`k2 hostchat rollback
//! --accept-data-loss`, Sew's `--accept-data-loss-since=<the pre-upgrade
//! bundle>`; a plain rollback is refused with `ROLLBACK_ACROSS_SCHEMA_HINT`).
//! Upgrade sew and sew-media together (K2 always does). The same three
//! sidecars, no new root-helper verb, and the same speech model file as
//! 0.11.4's (base-q8_0). Its DeepFilterNet3 asset `sew-dfn3-0.11.5.tar.gz`
//! holds the same bytes as 0.11.4's ([`super::SEW_DFN3`]). K2's client
//! crates stay on 0.11.4's (sew-call-client and tauri-plugin-sew-voice
//! 0.9.4) until 0.11.5's are published.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.11.5 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_11_5: SewRelease = SewRelease {
    version: "0.11.5",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "772b820c41d830ab1f6c247609ed787c6ae686aaa2f1ab5eaf57b192629cf856",
            binary_sha256: "b50232069dcb551a0d9d738ae5b0c93df8614d515c682d6ac271e1f93265fbe3",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "59cc57ce10527721e581252cc7354aaec58b17f3f7ce1b9d0a78cd5ba276d1dc",
            binary_sha256: "d147b7083939e0b4c710168f180840d0fba01b30792823ee10decd7fbd4ea61d",
        },
    ],
    media: Some(SewSidecar {
        version: "0.11.5",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "55639efc747c76b8c80a601126193349895f26b946bd87da6bb344972abaa255",
                binary_sha256: "d695c838381453cde7fa8a79aec66edd4791ab679df31e07fea487e0791238ad",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "33b392840738ff9bd9fd63a3afcb9da741b45f4c891dc096a26b9a0a64a0a3a5",
                binary_sha256: "dc292aa7a42ba73c4376e905f1df37a95e0f4c2c8d2f37ba7ca2e9e413020e4a",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.11.5",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "4e1c91ae81fcc3cde2b75e2a88c9677773fbb944f911c407650e387f1bec20bb",
                binary_sha256: "72a2f9c7803d073e8e1b6ec04fb7ca5da63ae971e504318a2ecc2d22bc07eb8c",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "0b264789ec206268d308b8480618a8e158792fffb51640b66cb5d4d63777fbbd",
                binary_sha256: "88267f96da0568ebffb7de022a70b15404178b512bfd8f56405095077b5ea8a4",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.11.5",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "b5da93a4d994d17b38316cdc81ac0c272a27d77bd9ccf3774461f604b59a70c3",
                binary_sha256: "f6af8e893adb7c8f81a599172a401b66cd1d8fe7401220d8429c9cdcfb0d73b3",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "48c1d92f83ce05d0f6d81ca4fe4d33d4b0578dd4c3fdd8d6364171d37d090ed3",
                binary_sha256: "2b6b3d631114c4932f2a585660a821de270e8f252e54d586806c068864171f6e",
            },
        ],
    }),
};
