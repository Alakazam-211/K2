//! Sew 0.10.5: a recording carries its real start time (`started_at`, RFC
//! 3339), so someone who joins mid-call sees the recording's own clock, and a
//! record message says why a transcript couldn't be made
//! (`transcript_failure`: `audio_gone`, `unsupported_language`,
//! `transcriber_error`, `audio_not_captured`). A `dir` keyring mirror is
//! checked (alarms `keys_mirror_unmounted`, `keys_mirror_same_device`,
//! `keys_mirror_unsynced`). The pin for K2 0.46.15 (0.10.4, 0.10.3, 0.10.2
//! and 0.10.0 stay: upgrade sources and rollback targets).
//!
//! Provenance: sew-releases tag v0.10.5, core commit
//! `e2239ea14f230f8adc06d793b61193e61257fae7` (release.json `git_sha`),
//! SHA256SUMS sha256 `2cb14372…4b84f`, release.json sha256 `46e087f7…ec91`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core tarball and
//! binary, sidecar tarball, kit, model and DeepFilterNet3 hash in Sew's pin
//! note is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.10.5.SHA256SUMS` and a test holds this entry to it.
//!
//! One migration (1000501, schema max 1000501), run by Sew at start: the
//! upgrade to 0.10.5 is a plain one, but the way back from it to any earlier
//! release crosses 1000501 (`k2 hostchat rollback --accept-data-loss`, Sew's
//! `--accept-data-loss-since=<the pre-upgrade bundle>`; a plain rollback is
//! refused with `ROLLBACK_ACROSS_SCHEMA_HINT`). Upgrade sew and sew-media
//! together (K2 always does). The same three sidecars, no new root-helper
//! verb, and the same speech model file as 0.10.4's (base-q8_0). Its
//! DeepFilterNet3 asset `sew-dfn3-0.10.5.tar.gz` holds the same bytes as
//! 0.10.4's ([`super::SEW_DFN3`]). Client crates from crates.io:
//! sew-call-client and tauri-plugin-sew-voice 0.8.3 (mirror `56b4a33a`), the
//! rest at 0.10.5.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.10.5 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_10_5: SewRelease = SewRelease {
    version: "0.10.5",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "e2dcd2fbfb533e5886228cd08627618a41f148a03e03345f5cc282973f02afca",
            binary_sha256: "d06f2771c6759456ae9ef701cfa3115922da64438b5c01cd72ef75278733e6b6",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "a24a07003724f567f698051f6389e7328f57908b8ac5faed6039879ace843c6a",
            binary_sha256: "dafecbfc7a5d77be303a024dc983aef4ccf2600164ab3de8a0add36a85ac78d8",
        },
    ],
    media: Some(SewSidecar {
        version: "0.10.5",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "44cdaa8d9cba94abfff0203d67727b43b56b1467d9662beab7cecb5e5730e10b",
                binary_sha256: "d18eeae7d22bf93b0d58e8a29849d2b36c8e38d0aa8f50133a204b0992d1bd6f",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "2ed0105d65c605e2e246781a7a91d5b07108c8e1e94bf6efb53f27b7fe418b81",
                binary_sha256: "801f3d69a2a2f48e0665e7402d23e7ea97193762266e0c8625c8d15561ef44e4",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.10.5",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "0f0424a33968df14aa3958c561d4f6ae5d740b076c2e56d7024c5806c14b253b",
                binary_sha256: "2a0756e9ff124e493ff70778fd8c7ec3964712e6f57f2aaa36424f3fbb7b91b4",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "5096f83f80986cb2784690ddddcf72d08e755cef1ca2aed0e8fd44d0ecfcb8cf",
                binary_sha256: "a5654ee59282f38ac57cc84981f0df54a762336a1513bde37df445d525e61539",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.10.5",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "c3b8eac7407645c4b7703aa58078cb7b4aa550c166da9e1b60cf261a41d6d5b3",
                binary_sha256: "59c272719ad05bf90e4e6b7e4d169c58e63c57689b1086bc1d70193bf574522f",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "d598292e5ee10dcf93de02b789e4cc340e98612d328f91d202e44e8f54f744a7",
                binary_sha256: "f64b660b5a01ed763df151fa609e5da68e9722be845b47a375d51434a709a85b",
            },
        ],
    }),
};
