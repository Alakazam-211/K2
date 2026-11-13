//! Sew 0.9.3: recommended update. The pin for K2 0.46.12 (0.9.2
//! stays: an upgrade source and rollback target). Its SHA256SUMS (sha256
//! `2811b8e1…7264`) is vendored as `testdata/sew/sew-0.9.3.SHA256SUMS` and a
//! test holds this entry to it. Schema max 991001, the same as 0.9.2's:
//! from 0.9.2 the upgrade runs no migration and a rollback to 0.9.2 needs
//! no flag. From 0.8.2 the upgrade is direct (Sew tested it) and crosses
//! 990001, 990002, 990011 (0.9.0's) and 991001, so a rollback to 0.8.x or
//! 0.9.0 needs `k2 hostchat rollback --accept-data-loss` (Sew's
//! `--accept-data-loss-since=<the pre-upgrade bundle>`). K2 records no
//! schema per release: Sew refuses the plain rollback across a migration
//! and K2 names the way back (`ROLLBACK_ACROSS_SCHEMA_HINT` in the
//! daemon). Upgrade sew and sew-media together (K2 always does), then
//! `sew recording upgrade` where the recorder is installed. The same three
//! sidecars, no new root-helper verb, and the same speech model file as
//! 0.9.2's (base-q8_0).

use super::{SewArtifact, SewRelease, SewSidecar};

/// The Sew 0.9.3 entry of [`super::SEW_RELEASES`].
pub(super) const SEW_0_9_3: SewRelease = SewRelease {
    version: "0.9.3",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "57b5adcb7f21f91f12bdb9137dc6720895bff17162a259e32b21d995bfa29acc",
            binary_sha256: "b93b2ae448afd2132bf1d5865936310216fbe49b9ef759f6e0b29a4ee6fccd47",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "2e7e4b11baf7cc03f88f56d25a51fcf0980b9894a661961608859b04230b7ecb",
            binary_sha256: "f64d3f511e4441487f30c713aac571a7ee614d461f7c49ffbd951e7210a0c3cc",
        },
    ],
    media: Some(SewSidecar {
        version: "0.9.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "05df6dfd3f18d6b46c0ed8f62e8a22fabf821e48bc3b5b2d1ffd5e1850ed7ea2",
                binary_sha256: "3bb6ff7703846044bab15d3a66ce6ab37e50ea1ff9444738e70c9a3b98662249",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "c96720f9e88eb404af65fd15b3e8f176028bbd887ba24818cc081bfdedece526",
                binary_sha256: "be8b5142175467e120cb9d558bca2c2fc1003d1e478391ba76be44ff217ff14e",
            },
        ],
    }),
    transcriber: Some(SewSidecar {
        version: "0.9.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "f0f389505dd2f8166c22e2294992efdfaabca3b83ba59d29a9f0bb383469b17f",
                binary_sha256: "8f9e91d3fa245601328c766edaad65f430e730d11198fd4a8e57132581a9262f",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "24fcb26d14e35aa8d39f99c6aa247cdeae314721099cab06182286d94855b805",
                binary_sha256: "b88739598356755fb8dc7f83847dcdca72c8a15bc31c1898d086b323388d25e5",
            },
        ],
    }),
    recorder: Some(SewSidecar {
        version: "0.9.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "8d712d686df9d984fcb42638d98a38b485910c5a6a536c4a8523e40e0388741c",
                binary_sha256: "52c144590529b8a9d981dd8e5fbb24514413a89fcb967014a9da881e5f91fc19",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "5d67bcfe51856ae7a4111373343b308feec457ffbba62ef526985c8c9941d9eb",
                binary_sha256: "ea44cb3fd1481378f9881f8a556345b1fe6c36ef15272deb34e2b62e32971f7c",
            },
        ],
    }),
};
