//! Sew 0.11.4, the pin for K2 0.46.18 (0.11.3, 0.11.1, 0.10.6, 0.10.5,
//! 0.10.4, 0.10.3, 0.10.2 and 0.10.0 stay: upgrade sources and rollback
//! targets).
//!
//! What 0.11.4 changes: the call engine no longer keeps a core busy while it
//! rejoins after a lost connection, or while a video layer waits behind a
//! lower one (phones save battery and heat on weak networks; no API change);
//! `sew media status --json` lists `inbound` ports with a `required` flag;
//! the App kit 0.11.4 adds read-receipt helpers. No new migration,
//! contracts unchanged.
//!
//! Provenance: sew-releases tag v0.11.4, core commit
//! `6cd5e23f62f2757e33b7b3b12f75ea2919cbbe91` (release.json `git_sha`),
//! SHA256SUMS sha256 `082839cd…6d26`, release.json sha256 `fe91d873…58bd`.
//! Printed by `scripts/sew-pins-from-release.sh` from the downloaded release,
//! tarballs included: it checked every tarball, every binary member and the
//! App kit against release.json and SHA256SUMS. Every core and sidecar
//! tarball, core binary, kit, model and DeepFilterNet3 hash in Sew's pin note
//! (sew-0-11-4-pins-published) is equal. The SUMS file is vendored as
//! `testdata/sew/sew-0.11.4.SHA256SUMS` and a test holds this entry to it.
//!
//! Migrations: still 0.11.0's two (release.json `schema_max` 1100101, as
//! 0.11.3's); 0.11.4 adds none. From 0.11.3 or 0.11.1 the upgrade and the
//! way back are plain ones. The way back from it to 0.10.6 or any earlier
//! release crosses 0.11.0's two (`k2 hostchat rollback --accept-data-loss`,
//! Sew's `--accept-data-loss-since=<the pre-upgrade bundle>`; a plain
//! rollback is refused with `ROLLBACK_ACROSS_SCHEMA_HINT`). Upgrade sew and
//! sew-media together (K2 always does). The same three sidecars, no new
//! root-helper verb, and the same speech model file as 0.11.3's (base-q8_0).
//! Its DeepFilterNet3 asset `sew-dfn3-0.11.4.tar.gz` holds the same bytes as
//! 0.11.3's ([`super::SEW_DFN3`]). The client crates are 0.11.4's too
//! (sew-call-client and tauri-plugin-sew-voice 0.9.4).

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.11.4 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_11_4: SewRelease = SewRelease {
    version: "0.11.4",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "5936272ccdc4b4d2ccaa2f1488470285aac615fefb49f8aa4612f59e88cab4e2",
            binary_sha256: "86ea61d837b0672b567101760857fdc9a84133dbefefbdf81fbcc19a6fd3ef72",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "d2bdd098cd828cd946cd0ed15be3b5508386e66a5197b72c71734896f93ada8e",
            binary_sha256: "fb635e693a7b49bf3822afe9aa87fa494e386b8caa4418eb5613fc125c2c1bc8",
        },
    ],
    media: Some(SewSidecar {
        version: "0.11.4",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "8c74906955cdc07ac9eb2b473aec07790011579419796c86b36f4f340c343e13",
                binary_sha256: "8feefffdf27b852152ce54f5fd99a1929426f976d19778a859c043716a697413",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "58aa919e09c9a6a8f19395bc131861de051e95824f0a71997aba7b739af33eb6",
                binary_sha256: "963410d71687df4178eb6071c80852b07ec018d6e63ad9479f1df25c0bdc285b",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.11.4",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "22575f7dfee852f6d5446fb082a2700cb0a0416b7d8709def89ff9fa96a163ca",
                binary_sha256: "42d9d1a1920647c9b50aa3b9417850a2f348656662e65360f206602a929c056f",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "51f50156b9e3b0ec1ccf4a14c4b64e8feabc5dbc1e04746b912a8a10d0f12d27",
                binary_sha256: "68388131baf8c407b4f6673ab7581dd93d0f651b087d19f21f92f4e5f3799955",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.11.4",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "126963b6944d2f1a68d10e0199504c5bb7bf5d12902716ff04518276c84c63dd",
                binary_sha256: "8193502f94763489f6edf1fbed414d6626387cb199c631137df24a6e5e22c719",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "0677c1decace7d24d4225887afe85cf41a5e42ff214a9b9ee69d1c558e47c98f",
                binary_sha256: "d9df9a4d4d1309c256c7152d693e16889a12e373fe1ee996446f9c2a49ce64f0",
            },
        ],
    }),
};
