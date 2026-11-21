//! Sew 0.11.3, the pin for K2 0.46.17 (0.11.1, 0.10.6, 0.10.5, 0.10.4,
//! 0.10.3, 0.10.2 and 0.10.0 stay: upgrade sources and rollback targets).
//! K2 never pinned 0.11.2.
//!
//! What 0.11.3 adds: the native self-view sink
//! (`open_native_sink(Track("local:camera"))`); Record/Stop and consent
//! answers now reach the host's provider (the `RoomRecorder` fix);
//! `VoiceControl::set_dsp_sched` (applied from the next call) and the
//! `capture_stage_dsp_*` fields; the noise-probe feature (measurement builds
//! only; iOS 14.2+); and `sew config set
//! recording.consent_prompt_timeout_ms`. Additions only: no new migration,
//! contracts unchanged.
//!
//! Provenance: sew-releases tag v0.11.3, core commit
//! `1c95fa3478c18b27af8980989ad08af75914d1a2` (release.json `git_sha`),
//! SHA256SUMS sha256 `c10cd9c7…68b9`, release.json sha256 `9d954c19…b267`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core and sidecar
//! tarball, core binary, kit, model and DeepFilterNet3 hash in Sew's pin note
//! is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.11.3.SHA256SUMS` and a test holds this entry to it.
//!
//! Migrations: still 0.11.0's two (release.json `schema_max` 1100101, as
//! 0.11.1's; the release binary's `sew version --json` reports schema max
//! 1100101, min 30001, 83 migrations); 0.11.3 adds none. Its feature list
//! is 0.11.1's fifty-one, unchanged (`tool_hook::features`). From 0.11.1 the upgrade and the way back are
//! plain ones. The way back from it to 0.10.6 or any earlier release crosses
//! 0.11.0's two (`k2 hostchat rollback --accept-data-loss`, Sew's
//! `--accept-data-loss-since=<the pre-upgrade bundle>`; a plain rollback is
//! refused with `ROLLBACK_ACROSS_SCHEMA_HINT`). Upgrade sew and sew-media
//! together (K2 always does). The same three sidecars, no new root-helper
//! verb, and the same speech model file as 0.11.1's (base-q8_0). Its
//! DeepFilterNet3 asset `sew-dfn3-0.11.3.tar.gz` holds the same bytes as
//! 0.11.1's ([`super::SEW_DFN3`]). The client crates are 0.11.3's too
//! (sew-call-client and tauri-plugin-sew-voice 0.9.3).

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.11.3 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_11_3: SewRelease = SewRelease {
    version: "0.11.3",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "d570d911324e15f4c19dd5b0df271b16dad58473d1d0c3bbb19a1f36aefc393e",
            binary_sha256: "492012b06766e74838aa3fb5316529c1a0a4d6dd3e535b25bbd8bc3223f3717a",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "0941e56f984c54830d53e73c1fb95ef2c954cddd727be6231d11cdd6fa22cb1a",
            binary_sha256: "5e3b768d61625da775563ca9543b7ee87ef142d12e78fb4013122a0105cc6485",
        },
    ],
    media: Some(SewSidecar {
        version: "0.11.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "8daabeddb34416da9a6d66a1a9d3740d0d7ed46d026bb35b2bbf6bbcc4a7fb74",
                binary_sha256: "19e44a68b5ab73ce2d725f7c7de9a3a6a9435effe999c8c11287c7116ebf92f3",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "7d00e6fd3250efe06180bbd03c32a78a923a9c6867f20c7683878dd36a3e3dcf",
                binary_sha256: "6853b980c0824f8223fd9d877bded696eba2a1a3502c56d3e440b6a2903674c0",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.11.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "ec97e9ac1d529b75bdcc253a9eb7da3ac9652000fee108a7ed6373dd87982cef",
                binary_sha256: "255c93c610cf6bcebcf5f2e8f362b00679c40ce3b5f05b993786d4928d948ca2",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "c43ca434102110be1e74f53add32cf97e921393aa44a444efd250a4cbd5eb46c",
                binary_sha256: "a6aebeab75b6a18e3799d170ffe1fe0ce66ef7361bafe13583358c77a21385d8",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.11.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "2339e97f615dd8dd22526830914d46bd1da639bc2dfc572aab28c9bf81535b79",
                binary_sha256: "3f9faea868cc553544aeb4ebc214aeba2544138004e3b289d44e56335e10dcce",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "487823441276c28c3fb9fe2cd98eb981e2edbb741b41524a3a78650c5a14a133",
                binary_sha256: "7335490ef9abc559b086406c06f5b47b94496c0f08ad2665cacaaefc24357fbe",
            },
        ],
    }),
};
