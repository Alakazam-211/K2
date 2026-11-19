//! Sew 0.10.6: DM read receipts (feature `dm_read_receipts`); a person's
//! per-channel notify level over mgmt, `GET`/`POST
//! /mgmt/v1/people/{ext}/channels/{id}/notify[/set]` (feature
//! `person_channel_notify`); a person's notification feed head, `GET
//! /mgmt/v1/people/{ext}/notifications/head` (feature `feed_head`), which
//! K2 0.46.16's chat pushes use; and a recording's `audio_keep_days`. The pin
//! for K2 0.46.16 (0.10.5, 0.10.4, 0.10.3, 0.10.2 and 0.10.0 stay: upgrade
//! sources and rollback targets).
//!
//! Provenance: sew-releases tag v0.10.6, core commit
//! `2caf984560d703b6c167d97086a0e86ad71a3a55` (release.json `git_sha`),
//! SHA256SUMS sha256 `6a0a29dc…c6f4`, release.json sha256 `8a99a104…f819`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core tarball and
//! binary, sidecar tarball, kit, model and DeepFilterNet3 hash in Sew's pin
//! note is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.10.6.SHA256SUMS` and a test holds this entry to it.
//!
//! One migration (1000601, schema max 1000601), run by Sew at start: the
//! upgrade to 0.10.6 is a plain one, but the way back from it to any earlier
//! release crosses 1000601 (`k2 hostchat rollback --accept-data-loss`, Sew's
//! `--accept-data-loss-since=<the pre-upgrade bundle>`; a plain rollback is
//! refused with `ROLLBACK_ACROSS_SCHEMA_HINT`). Upgrade sew and sew-media
//! together (K2 always does). The same three sidecars, no new root-helper
//! verb, and the same speech model file as 0.10.5's (base-q8_0). Its
//! DeepFilterNet3 asset `sew-dfn3-0.10.6.tar.gz` holds the same bytes as
//! 0.10.5's ([`super::SEW_DFN3`]). The client crates stay at 0.10.5's
//! (sew-call-client and tauri-plugin-sew-voice 0.8.3): 0.8.4 comes later.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.10.6 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_10_6: SewRelease = SewRelease {
    version: "0.10.6",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "a43d5399b93cfa04ade4cd33925629b7b19d084001e7fda250c6b903d47f243d",
            binary_sha256: "c9ebd0f278a49ed88736b44fadd60c7844d804399d1fa012d6552841b2a79f14",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "82132cca7b22933e890983375507d16449c19a3f720b16d4011b251fbf8adbc4",
            binary_sha256: "5512be01df664a54ce536ed9ecb87674e0a5de666a763305bc4781eaabb33dc4",
        },
    ],
    media: Some(SewSidecar {
        version: "0.10.6",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "087115c36c37e28b5e1f70c70ee4e7953fb3ab5adf247b67b903a1f8efe8b890",
                binary_sha256: "9609bc0ba704124f0b4d4b8d2a7497e823ebc08a553507932b86aa529839d90d",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "7e3adc3384d5bbb3b35b0489d8a1edbb0d75381e2982e79d3826f2f12821a6fc",
                binary_sha256: "eb619ca9d9adf12c2f99c25a67ad744687f67a4df568cda0c07eea75ceaac9d7",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.10.6",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "c4cd7a05bd3db7504ec6f14d78ff421618b09106a038de893237f4a50280f724",
                binary_sha256: "eba2744f0b3f64b1db7b8975fe9ac753f6ec0c695ff199986300eb8a0d4d71c5",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "235b288948066d8c98409de47206ab336f6b00fa1f137aa3af57df7a03a5d162",
                binary_sha256: "fd729a2148b0293efeb62ef5625fdf8c8369f3c28e63efe518b50ac61e2c8830",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.10.6",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "2318f37430d4c0e85509c20defaf75b1a995a548b3631dcbfc0a1db48b92de24",
                binary_sha256: "8e8ad6446e8d7b9af6d7c3d999bc081295e99499f99ea9c98a187d19672f3cc3",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "4784e4f28120b841394171f9bf296125b196fccd0b8ca79dd771bf123625c784",
                binary_sha256: "2bade4a8268bdc9489e3b83db82e919dabfc84a3105fffa92be81f702a975bd6",
            },
        ],
    }),
};
