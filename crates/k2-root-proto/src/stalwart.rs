//! The Stalwart pin table (moved out of `k2-daemon::mail::supervisor`,
//! which re-exports it). The helper's `mail install-bin` accepts exactly
//! the pin or the previous tarball for its own arch; the daemon downloads
//! by the same table. One table, so a helper and a daemon of the same
//! release can never disagree.

/// v0.16.20: the shared-calendar MAIL FROM fix, 0.16.18 JMAP scheduling +
/// recurrences and the 0.16.19 per-user Calendar/set fix. Previous pin:
/// v0.16.10 ([`STALWART_PREVIOUS_VERSION`]) — the only version `k2
/// hostmail upgrade` moves FROM, and the rollback target.
/// Pinning is load-bearing: v0.16 removed the whole REST API relative
/// to earlier lines, upstream has no mgmt-API stability policy, and
/// the config format churns between minors (PRD §4, §16, pre-mortem
/// #8). Upgrades are explicit, K2-shipped, tested operations — the
/// supervisor REFUSES to manage an unrecognized on-disk version, and
/// the daemon updating itself never touches the Stalwart version.
pub const STALWART_PINNED_VERSION: &str = "0.16.20";

/// The pin before [`STALWART_PINNED_VERSION`]: what a box enabled by an
/// older daemon has installed, the ONLY version `k2 hostmail upgrade`
/// moves from, and the binary a failed upgrade reinstalls. Compared with
/// `!=` (CAL56: never semver ordering).
pub const STALWART_PREVIOUS_VERSION: &str = "0.16.10";

/// One pinned release artifact: Rust `std::env::consts::ARCH` name →
/// upstream target triple + the sha256 of the release tarball.
#[derive(Debug)]
pub struct StalwartArtifact {
    pub arch: &'static str,
    pub triple: &'static str,
    pub sha256: &'static str,
}

/// Baked-in checksums for the pinned release (PRD §4.1 — "checksums
/// baked into the daemon at build time").
///
/// REAL values for v0.16.20. The fork release `Alakazam-211/stalwart`
/// v0.16.20 MIRRORS the upstream `stalwartlabs/stalwart` v0.16.20
/// tarballs byte for byte (not a fork build). Verified THREE ways on
/// 2026-10-06: (1) sha256 of the downloaded fork assets, (2) sha256 of
/// the downloaded upstream assets of the same names (identical bytes),
/// (3) the upstream sigstore bundles (`…tar.gz.sigstore.json` →
/// messageSignature.messageDigest; Fulcio identity
/// `stalwartlabs/stalwart/.github/workflows/ci.yml@refs/tags/v0.16.20`,
/// commit 6d5c6589). All matched the fork release body. glibc builds —
/// K2 Linux deployments are Ubuntu/Debian-class boxes (musl/Alpine
/// support would add the musl triples here).
pub const STALWART_SHA256: &[StalwartArtifact] = &[
    StalwartArtifact {
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        sha256: "55184f166f89a0918c6523bb70be300370b63934357731b199dc26a12ca15abf",
    },
    StalwartArtifact {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        sha256: "82183ce973665d2b99822b4a1f9911a8ba14788fd73ee93ce9be92352ccdea64",
    },
];

/// Checksums for [`STALWART_PREVIOUS_VERSION`] (v0.16.10), kept so a
/// failed upgrade can reinstall the old binary (CAL14). Verified two
/// ways at the original bake (2026-07-08): the release's signed sigstore
/// bundles and the downloaded tarballs; re-verified 2026-07-10.
pub const STALWART_PREVIOUS_SHA256: &[StalwartArtifact] = &[
    StalwartArtifact {
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        sha256: "3ec4ab7eff49f61280f2fe2e4f9645ce5308d1840286ef0c0437524f92ac6a33",
    },
    StalwartArtifact {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        sha256: "0c9a80174a8a187477ac0ae4fcca8b7e0f17c84f6cae7b9304d2539459ba44ac",
    },
];

/// True when a checksum entry is a placeholder / malformed — install
/// REFUSES to run in that state (the "never ship an unverifiable
/// install path" guard; a const table edited to `…PLACEHOLDER…` or a
/// truncated hex fails closed).
pub fn checksum_is_placeholder(a: &StalwartArtifact) -> bool {
    a.sha256.contains("PLACEHOLDER")
        || a.sha256.len() != 64
        || !a.sha256.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Resolve the pinned artifact for a `std::env::consts::ARCH` value.
pub fn artifact_for_arch(arch: &str) -> Result<&'static StalwartArtifact, String> {
    artifact_in(STALWART_SHA256, arch)
}

/// The [`STALWART_PREVIOUS_VERSION`] artifact for `arch` (rollback).
pub fn previous_artifact_for_arch(arch: &str) -> Result<&'static StalwartArtifact, String> {
    artifact_in(STALWART_PREVIOUS_SHA256, arch)
}

fn artifact_in(
    table: &'static [StalwartArtifact],
    arch: &str,
) -> Result<&'static StalwartArtifact, String> {
    let art = table.iter().find(|a| a.arch == arch).ok_or_else(|| {
        format!("unsupported CPU architecture '{arch}' — K2 Mail ships x86_64 and aarch64")
    })?;
    if checksum_is_placeholder(art) {
        return Err(format!(
            "refusing to install: the baked-in sha256 for {arch} is a placeholder — \
             this build cannot verify the Stalwart download"
        ));
    }
    Ok(art)
}
