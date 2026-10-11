//! The Stalwart pin table (moved out of `k2-daemon::mail::supervisor`,
//! which re-exports it). The pin, the previous pin, and every version
//! `k2 hostmail upgrade` moves FROM ([`STALWART_UPGRADE_FROM`]) live here
//! once: the root helper's `mail install-bin` accepts exactly the pin or
//! an upgrade-source tarball for its own arch, its `version` lists them
//! (`stalwartUpgradeFrom`), and the daemon downloads and checks by the
//! same table. One table, so a helper and a daemon of the same release
//! can never disagree.

/// The PINNED Stalwart version the supervisor installs and manages.
///
/// v0.16.25 (K2 0.45.4, field-fixes HF2): the fixes akzm's and lztek's
/// IT agents kept hitting — 0.16.23's `bit_and` (spam-filter v3.0.2's
/// DNSBL rules load) and its one-nameserver-at-a-time DNSSEC resolver
/// (the hickory TCP-retry race behind the false "DNSSEC … Bogus"),
/// 0.16.24's MX lookups outside the DNSSEC resolver while DANE is off and
/// rule updates that overwrite existing objects, 0.16.25's tagged IMAP
/// `NO` on a bad app password, DNSBL multi-code scoring and DKIM rotation
/// fixes. No config or registry migration between 0.16.20 and 0.16.25
/// (upstream: "replace the binary"). Previous pin: v0.16.20
/// ([`STALWART_PREVIOUS_VERSION`]); `k2 hostmail upgrade` also still
/// moves boxes that never left v0.16.10 ([`STALWART_UPGRADE_FROM`]).
/// Pinning is load-bearing: v0.16 removed the whole REST API relative
/// to earlier lines, upstream has no mgmt-API stability policy, and
/// the config format churns between minors (PRD §4, §16, pre-mortem
/// #8). Upgrades are explicit, K2-shipped, tested operations — the
/// supervisor REFUSES to manage an unrecognized on-disk version, and
/// the daemon updating itself never touches the Stalwart version.
pub const STALWART_PINNED_VERSION: &str = "0.16.25";

/// The pin before [`STALWART_PINNED_VERSION`]: what a box enabled or
/// upgraded by K2 0.45.0–0.45.3 has installed. The first of the versions
/// `k2 hostmail upgrade` moves from ([`STALWART_UPGRADE_FROM`]) and the
/// `stalwartPrevious` the mail helper reports. Compared with `!=`
/// (CAL56: never semver ordering).
pub const STALWART_PREVIOUS_VERSION: &str = "0.16.20";

/// The pin before [`STALWART_PREVIOUS_VERSION`] (K2 ≤ 0.44.x enables).
/// Still an upgrade source — a box may skip 0.16.20 and go straight to
/// the pin (0.16.x → 0.16.x is a binary swap upstream) — and therefore a
/// rollback target.
pub const STALWART_OLDER_VERSION: &str = "0.16.10";

/// One pinned release artifact: Rust `std::env::consts::ARCH` name →
/// upstream target triple + the sha256 of the release tarball.
#[derive(Debug)]
pub struct StalwartArtifact {
    pub arch: &'static str,
    pub triple: &'static str,
    pub sha256: &'static str,
}

/// A version + its per-arch tarballs.
#[derive(Debug)]
pub struct StalwartRelease {
    pub version: &'static str,
    pub artifacts: &'static [StalwartArtifact],
}

/// Baked-in checksums for the pinned release (PRD §4.1 — "checksums
/// baked into the daemon at build time").
///
/// REAL values for v0.16.25, computed 2026-10-10 from the downloaded
/// upstream `stalwartlabs/stalwart` v0.16.25 release tarballs and
/// cross-checked against the upstream sigstore bundles
/// (`…tar.gz.sigstore.json` → messageSignature.messageDigest; Fulcio
/// identity `stalwartlabs/stalwart/.github/workflows/ci.yml@refs/tags/v0.16.25`,
/// commit 3f657330). The fork release `Alakazam-211/stalwart` v0.16.25
/// MIRRORS those tarballs byte for byte (not a fork build); the download
/// URL points at the fork. glibc builds — K2 Linux deployments are
/// Ubuntu/Debian-class boxes (musl/Alpine support would add the musl
/// triples here).
pub const STALWART_SHA256: &[StalwartArtifact] = &[
    StalwartArtifact {
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        sha256: "8ba9cc0ea2795121df5c1264af41e82353c5a3d6860161c4ab70817906d912d3",
    },
    StalwartArtifact {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        sha256: "7fc867e8bac70e00c372e8c8e8bc0e73e5f367f797daa5446f8c79225bdfb7e1",
    },
];

/// Checksums for [`STALWART_PREVIOUS_VERSION`] (v0.16.20), the pin of
/// K2 0.45.0–0.45.3. Verified THREE ways on 2026-10-06: sha256 of the
/// fork assets, of the upstream assets of the same names (identical
/// bytes), and the upstream sigstore bundles (commit 6d5c6589).
pub const STALWART_PREVIOUS_SHA256: &[StalwartArtifact] = &[
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

/// Checksums for [`STALWART_OLDER_VERSION`] (v0.16.10). Verified two
/// ways at the original bake (2026-07-08): the release's signed sigstore
/// bundles and the downloaded tarballs; re-verified 2026-07-10.
pub const STALWART_OLDER_SHA256: &[StalwartArtifact] = &[
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

/// Every installed version `k2 hostmail upgrade` moves FROM, newest
/// first. A rollback reinstalls exactly the version the box had, so each
/// entry is also a rollback target, and the mail helper's `install-bin`
/// accepts exactly the pin plus these tarballs.
pub const STALWART_UPGRADE_FROM: &[StalwartRelease] = &[
    StalwartRelease {
        version: STALWART_PREVIOUS_VERSION,
        artifacts: STALWART_PREVIOUS_SHA256,
    },
    StalwartRelease {
        version: STALWART_OLDER_VERSION,
        artifacts: STALWART_OLDER_SHA256,
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

/// The [`STALWART_PREVIOUS_VERSION`] artifact for `arch`.
pub fn previous_artifact_for_arch(arch: &str) -> Result<&'static StalwartArtifact, String> {
    artifact_in(STALWART_PREVIOUS_SHA256, arch)
}

/// The [`STALWART_UPGRADE_FROM`] entry for `version` (`None` = this
/// build can neither upgrade from nor roll back to it).
pub fn upgrade_from_release(version: &str) -> Option<&'static StalwartRelease> {
    STALWART_UPGRADE_FROM.iter().find(|r| r.version == version)
}

/// The tarball of an upgrade-source `version` on `arch` — what a
/// rollback from that version reinstalls.
pub fn upgrade_from_artifact(
    version: &str,
    arch: &str,
) -> Result<&'static StalwartArtifact, String> {
    let rel = upgrade_from_release(version).ok_or_else(|| {
        format!(
            "Stalwart {version} is not a version this daemon upgrades from (it knows {})",
            upgrade_from_versions().join(", ")
        )
    })?;
    artifact_in(rel.artifacts, arch)
}

/// The [`STALWART_UPGRADE_FROM`] versions, newest first.
pub fn upgrade_from_versions() -> Vec<&'static str> {
    STALWART_UPGRADE_FROM.iter().map(|r| r.version).collect()
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
