//! Sew 0.11.1, the pin for K2 0.46.17 (0.10.6, 0.10.5, 0.10.4, 0.10.3,
//! 0.10.2 and 0.10.0 stay: upgrade sources and rollback targets). K2 never
//! pinned 0.11.0, so this pin also brings in 0.11.0's changes.
//!
//! What 0.11.0 brought: the person feed's second version (feature
//! `feed_all_messages`: pushes for a channel at All messages and its
//! keywords, a read clears pushes on every device, `from_external_id` and
//! `thread_root_id` on items), voice-room start notices (`voice_started_feed`)
//! and voice-room invites (`voice_invites`). K2's Home chat pushes act on
//! `feed_all_messages`; K2 gates on neither voice flag. Voice room
//! notifications have their own optional `[voice.notify]` settings, which K2
//! leaves at Sew's defaults. 0.11.1 adds no feature flag and no migration;
//! its change is in the client crates (a phone's noise removal keeps running
//! through a call's busy moments).
//!
//! Provenance: sew-releases tag v0.11.1, core commit
//! `f070d8c1ab623d423b3a46920111e3c5ea2a12b4` (release.json `git_sha`),
//! SHA256SUMS sha256 `6f42ab68…5438`, release.json sha256 `d4544a33…29be`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core and sidecar
//! tarball, kit, model and DeepFilterNet3 hash in Sew's pin note is equal.
//! The SUMS file is vendored as `testdata/sew/sew-0.11.1.SHA256SUMS` and a
//! test holds this entry to it.
//!
//! Migrations: 0.11.0's two (schema max 1100101, as the published x86_64
//! build's `sew version --json` reports); 0.11.1 has none. Sew runs them at
//! start: the upgrade to 0.11.1 is a plain one, but the way back from it to
//! 0.10.6 or any earlier release crosses them (`k2 hostchat rollback
//! --accept-data-loss`, Sew's `--accept-data-loss-since=<the pre-upgrade
//! bundle>`; a plain rollback is refused with `ROLLBACK_ACROSS_SCHEMA_HINT`).
//! Upgrade sew and sew-media together (K2 always does). The same three
//! sidecars, no new root-helper verb, and the same speech model file as
//! 0.10.6's (base-q8_0). Its DeepFilterNet3 asset `sew-dfn3-0.11.1.tar.gz`
//! holds the same bytes as 0.10.6's ([`super::SEW_DFN3`]). The client crates
//! are 0.11.1's too (sew-call-client and tauri-plugin-sew-voice 0.9.1).

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.11.1 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_11_1: SewRelease = SewRelease {
    version: "0.11.1",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "6437e90715fcd72898b5b40ceb371edd774b4017d9565fbe7aad99cc276174ed",
            binary_sha256: "4c26976f00006ab7b3b0a117bdcee399e325dab1ff15b7a91abd861b84bc8803",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "6da0a7bb2cad40b40a0eeb2d0aa039eabca28640f625da99328e4c7111f2ed43",
            binary_sha256: "e55ae62d5396820f9365a8c2f07880e85472315e53d5ebabcb0c0ab3023f6738",
        },
    ],
    media: Some(SewSidecar {
        version: "0.11.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "5079641a0a8a78939fb3920429e62ae227ebe1316df40857e2fbd6e1b6158563",
                binary_sha256: "cb9b772d62d6b83d5e0377bc941cc8004d9cfdb66e8a9ec1227732ae6bf02b03",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "54cc60fb0b81dd7eb05d5ab5fe742c0c3598492c7ba921b6bf48c0c6a94972cb",
                binary_sha256: "0b98465f11231395c9a672349545dc34d1b2e89c5a145b804e55b914d54b1194",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.11.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "f8a0eefdeb114158c7a4f6d7bf6bc8a27efa9458154f4dbb122e20e6b636386f",
                binary_sha256: "7c793bbda5910bbddf5a875130f655863e0f1e4e9dcf67d06bc6cfd9eff233ea",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "399e9651bd8f01113755485f214597c0492cf47d09d5ac8c0ba4159dc040a00d",
                binary_sha256: "ffba5cc553d849bfbae17f2456e20fc9dabe5212f36a0243787a557d04244e64",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.11.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "c01d5e722523103aa6c15221691592c787c04dc4958b6f902416ff8e4ebfd3ef",
                binary_sha256: "369522e88521f51a01c22df73ebe81ad79e04f890c82327a46dc9cfbf9f175bb",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "8707882796b24a30318bd8c7943b686f903a1b727cd64447021869200e8df218",
                binary_sha256: "54241368aa836a0adcb4d87be2c6a197824a591d95ffe706ae8108d4e0516573",
            },
        ],
    }),
};
