//! Sew 0.10.0: recommended update (chat with people on Home, DM call cancel,
//! the upstream WebRTC congestion-feedback fixes). The pin for K2 0.46.13
//! (0.9.3 stays: an upgrade source and rollback target).
//!
//! Provenance: sew-releases tag v0.10.0, core commit
//! `efc04226c6c315197217382335d9d61c876d6b46` (release.json `git_sha`),
//! SHA256SUMS sha256 `4b3bce65…905a`, release.json sha256 `f94f7bdf…03f4`
//! (both as Sew's pin note says). Printed by
//! `scripts/sew-pins-from-release.sh` from the downloaded release, tarballs
//! included: it checked every tarball, every binary member and the App kit
//! against release.json and SHA256SUMS. Every tarball and core binary hash in
//! Sew's pin note is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.10.0.SHA256SUMS` and a test holds this entry to it.
//!
//! Schema max 1000101 (one new migration, 1000101), so from any 0.9.x the
//! upgrade crosses a schema change: a rollback to 0.9.3 (or older) is refused
//! plain and needs `k2 hostchat rollback --accept-data-loss` (Sew's
//! `--accept-data-loss-since=<the pre-upgrade bundle>`; the daemon's
//! `ROLLBACK_ACROSS_SCHEMA_HINT` names it). Sew tested the upgrade from
//! 0.9.3, 0.9.2 and 0.9.0, each with that rollback. Upgrade sew and
//! sew-media together (K2 always does), then `sew recording upgrade` where
//! the recorder is installed. Features (45): 0.9.2's thirty-seven plus
//! recording_controls, channel_member_add_remove, channel_members_can_leave,
//! http_typing, read_state_unread_count, member_channels_level_only,
//! agents_dm_with_box_people and call_cancel (sew-proto 0.10.0's
//! `mgmt::FEATURES`). The same three sidecars, no new root-helper verb, and
//! the same speech model file as 0.9.3's (base-q8_0). Client crates:
//! sew-call-client and tauri-plugin-sew-voice 0.7.0 (mirror `8b5a2ff4`), the
//! rest at 0.10.0 from crates.io.

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.10.0 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_10_0: SewRelease = SewRelease {
    version: "0.10.0",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "7f2bf6949329e682fa3225de0bf12e16408c4749f69d7e892044a91419046901",
            binary_sha256: "61a163a4b247c6f91ee6145e2ef4339b46193b01a02dda15aaf811a334d4e5d7",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "befdaa2b0fea78044e9acc060851328a942cf68e45c070b2e2ad41e71cd3fd06",
            binary_sha256: "c9665962a04e2c77508ffd558cad747a904c4d8aa19d8e96ce40e4449a8803a9",
        },
    ],
    media: Some(SewSidecar {
        version: "0.10.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "5e2c74db8eaac51f93c8d7daafa8491284f48fe1d792dd2e21b7ddeb49d303ad",
                binary_sha256: "553397c5cc2bce3b6dbf6190204e40da4d420b88a6b4ff00ec5c2bad6a814e28",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "b4aadcde5cec8494c8b7360c2c9fa1e940948a809653b0b0a88cd52bd7d36b7a",
                binary_sha256: "e7f5f2fece000c5e8b8750fc4698cf8ca2bba3b3c8ebb4e2f1f5d3711880c5ea",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.10.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "3abb05e03bfdb9c1082907752c62e049b2b533381f3f11efc85e176a17e96bab",
                binary_sha256: "b7f322ba909ee278b329d40abc5775b3c7521b2977c2d714acc82c61d39f6a06",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "dd2eebf1285d26168a39d1694be5713169f0c5b43938827f61acb4a1ed7145bc",
                binary_sha256: "1b560f54e9a96873835553195a427b02edfd25d72e1213b66541f606cac6b09f",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.10.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "9f3938b9efe328013fedbfd087b12f688df7d4ad05ee080ad433e07af044f30e",
                binary_sha256: "b77cc374eee0dcfe1b922421ca77ccd9a4fd77c867570db63bcc0176bae8dafe",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "b6b36b506bed96feb525a32e9c66a61ea0dab27283fb986e1f9c702ebbf9a7ae",
                binary_sha256: "6a2ae241b9fc084a919e2d0368a8174221ea104151b690912ea58d0c02d6ed3e",
            },
        ],
    }),
};
