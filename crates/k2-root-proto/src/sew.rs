//! Sew on K2 (Sew S29, K11): the pin table, fixed paths and the input
//! validators the root helper's `engine sew` namespace (protocol 7) and
//! the daemon's Sew supervisor share.
//!
//! **Trust anchor.** The helper installs and runs only Sew bytes whose
//! sha256 this table vouches for: the release tarball AND the single `sew`
//! member it extracts (S29 §6.1, gap G3). A K2 release pins Sew; Sew's own
//! pipeline only publishes the assets. Before any root exec of
//! `/usr/local/bin/sew` the helper checks that the file on disk is a
//! release this table knows ([`known_binary`]).
//!
//! **SW1 is not published yet.** Until Sew tags a release, [`SEW_RELEASES`]
//! holds placeholders and [`pinned_artifact`] refuses ("this build has no
//! Sew pin"), so enable refuses at preflight. Builds with the `test-signer`
//! feature (tests and the test-keys example only; release signing refuses
//! those builds) use [`TEST_RELEASES`] instead: K2's own fixture tarballs
//! under `testdata/sew/`, which hold a shell stub, never Sew.
//!
//! **Argv.** Nothing here is ever a helper argv token except the enum
//! names, an install id ([`install_id_ok`]) and a bundle id
//! ([`bundle_id_ok`]). URLs, hosts, paths and keys reach the helper as JSON
//! (or key text) on stdin and are validated there ([`InstallRequest`],
//! [`ConfigRequest`], [`MediaRequest`]).

use serde::{Deserialize, Serialize};

// ── Fixed paths (Sew's own layout, `sew-deploy/src/paths.rs`) ───────────

pub const SEW_BIN: &str = "/usr/local/bin/sew";
/// Where the helper stages a new `/usr/local/bin/sew` before the rename.
pub const SEW_BIN_STAGING: &str = "/usr/local/bin/.sew.k2-new";
pub const SEW_CONFIG_DIR: &str = "/etc/sew";
/// `root:sew 0640`. The helper edits only `[mgmt] allow_uids` (SW7).
pub const SEW_CONFIG: &str = "/etc/sew/sew.toml";
/// `root:sew-mgmt 0640`, `sewsvc_<43 base64url>`. The daemon gets it from
/// the helper (`engine sew token`), never through a supplementary group.
pub const SEW_MGMT_TOKEN: &str = "/etc/sew/mgmt.token";
pub const SEW_MGMT_SOCKET: &str = "/run/sew/mgmt.sock";
pub const SEW_DATA_DIR: &str = "/var/lib/sew";
pub const SEW_UPGRADE_STATE: &str = "/usr/local/lib/sew/upgrade-state.json";
/// Written by `sew media install --sysctl apply` (Sew's canned bytes).
pub const SEW_MEDIA_SYSCTL: &str = "/etc/sysctl.d/60-sew-media.conf";
/// The helper's own Sew staging dir (root 0700): the verified tarballs and
/// the transient install / media JSON Sew reads with `--config-json`.
pub const SEW_STAGE_DIR: &str = "/var/lib/k2-root-helper/sew";
pub const SEW_STAGE_TARBALL: &str = "/var/lib/k2-root-helper/sew/sew.tar.gz";
pub const SEW_STAGE_MEDIA_TARBALL: &str = "/var/lib/k2-root-helper/sew/sew-media.tar.gz";
pub const SEW_STAGE_INSTALL_JSON: &str = "/var/lib/k2-root-helper/sew/install.json";
pub const SEW_STAGE_MEDIA_JSON: &str = "/var/lib/k2-root-helper/sew/media.json";

/// Sew's units (written by `sew install`, never by K2).
pub const UNIT_DB: &str = "sew-db.service";
pub const UNIT_MIGRATE: &str = "sew-migrate.service";
pub const UNIT_SEW: &str = "sew.service";
pub const UNIT_BACKUP_TIMER: &str = "sew-backup.timer";
pub const UNIT_DRILL_TIMER: &str = "sew-drill.timer";
pub const UNIT_MEDIA: &str = "sew-media.service";

/// Sew's client API on loopback (C5).
pub const SEW_CLIENT_PORT: u16 = 8470;
/// Voice: sew-media's loopback signalling port and its public ports
/// (TCP 7881, UDP 7882, TURN-TLS TCP 5349). TURN on :443 only when :443 is
/// free (never on a box where Stalwart holds it, Rosson 2026-10-15).
pub const SEW_MEDIA_LOOPBACK_PORT: u16 = 7880;
pub const SEW_MEDIA_TCP_PORT: u16 = 7881;
pub const SEW_MEDIA_UDP_PORT: u16 = 7882;
pub const SEW_TURN_TLS_PORT: u16 = 5349;

/// Upper bound of a release tarball on stdin (mirrors Sew's own cap).
pub const MAX_TARBALL: usize = 256 * 1024 * 1024;
/// Upper bound of the extracted `sew` / `sew-media` member.
pub const MAX_MEMBER: u64 = 256 * 1024 * 1024;

// ── Pin table ───────────────────────────────────────────────────────────

/// One per-arch artifact: tarball sha256 + the sha256 of the binary member.
#[derive(Debug)]
pub struct SewArtifact {
    pub arch: &'static str,
    pub triple: &'static str,
    pub tarball_sha256: &'static str,
    pub binary_sha256: &'static str,
}

/// The voice sidecar (`sew-media`) of one release: its own version and
/// artifacts (`release.json` `sidecars[]`).
#[derive(Debug)]
pub struct SewSidecar {
    pub version: &'static str,
    pub artifacts: &'static [SewArtifact],
}

/// One pinned Sew release.
#[derive(Debug)]
pub struct SewRelease {
    pub version: &'static str,
    pub artifacts: &'static [SewArtifact],
    pub media: Option<SewSidecar>,
}

/// Every Sew release K2 has ever pinned, oldest first. The LAST entry is
/// the pin. A pin bump appends: old entries stay so the helper still
/// recognises the binary already on a box (upgrade source, rollback).
///
/// PLACEHOLDER until Sew publishes SW1 (a tagged release with
/// `release.json` carrying `binary_sha256`). Paste the snippet Sew's
/// `release.sh` prints here.
pub const SEW_RELEASES: &[SewRelease] = &[SewRelease {
    version: "0.1.0",
    artifacts: &[
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "PLACEHOLDER-sew-SW1-not-published",
            binary_sha256: "PLACEHOLDER-sew-SW1-not-published",
        },
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "PLACEHOLDER-sew-SW1-not-published",
            binary_sha256: "PLACEHOLDER-sew-SW1-not-published",
        },
    ],
    media: None,
}];

/// K2's fixture releases (`testdata/sew/*.tar.gz`, made by a deterministic
/// script: gzip mtime 0, ustar, root-owned members). The `sew` member is a
/// shell stub that is never executed by any test (exec is faked). Used only
/// by `test-signer` builds.
pub const TEST_RELEASES: &[SewRelease] = &[
    SewRelease {
        version: "0.0.1",
        artifacts: &[SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "e70d86ef976c4f8e3d6e3975c1f8c53147184448290d3f7701748aa7ce213c22",
            binary_sha256: "c3d34485f7489c54cfe0ca1698be054ddc46ff9988e03c569a745182b8f59522",
        }],
        media: None,
    },
    SewRelease {
        version: "0.0.2",
        artifacts: &[SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "a5940321b4efb87b6a27098f47140555c10630667739f53d6e5256144f00b7cf",
            binary_sha256: "f6a283a01c9931727e76b819b3bade043c01731523cbdf7d9febb139a7415094",
        }],
        media: Some(SewSidecar {
            version: "0.0.2",
            artifacts: &[SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "2aa81ac2ba8a2c22619a3318f0505cfc7f44a9af5b3047056f4653b24a7cfe5b",
                binary_sha256: "9d98f6f3e080b8a4974c3392a9d8e802357cc6848daaf5a334d65756def5573c",
            }],
        }),
    },
];

/// The table this build uses: the real pins, or the fixtures in a
/// `test-signer` build.
pub fn releases() -> &'static [SewRelease] {
    if cfg!(feature = "test-signer") {
        TEST_RELEASES
    } else {
        SEW_RELEASES
    }
}

/// The pin: the last release.
pub fn pinned() -> &'static SewRelease {
    releases().last().expect("the Sew pin table is never empty")
}

/// True when an artifact entry is a placeholder or malformed (install
/// refuses: never ship an unverifiable install path).
pub fn checksum_is_placeholder(a: &SewArtifact) -> bool {
    !hex64(a.tarball_sha256) || !hex64(a.binary_sha256)
}

pub fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn artifact_in(rel: &'static SewRelease, arch: &str) -> Result<&'static SewArtifact, String> {
    let a = rel
        .artifacts
        .iter()
        .find(|a| a.arch == arch)
        .ok_or_else(|| format!("unsupported CPU architecture '{arch}': Sew ships x86_64 and aarch64"))?;
    if checksum_is_placeholder(a) {
        return Err("refusing to install: this build has no Sew pin (placeholder checksum; Sew SW1 not published)".into());
    }
    Ok(a)
}

/// The pinned core artifact for `arch`, or why there is none.
pub fn pinned_artifact(arch: &str) -> Result<&'static SewArtifact, String> {
    artifact_in(pinned(), arch)
}

/// The pinned `sew-media` artifact for `arch`.
pub fn pinned_media_artifact(arch: &str) -> Result<(&'static str, &'static SewArtifact), String> {
    let m = pinned()
        .media
        .as_ref()
        .ok_or_else(|| "this build pins no sew-media (voice) release".to_string())?;
    let a = m
        .artifacts
        .iter()
        .find(|a| a.arch == arch)
        .ok_or_else(|| format!("unsupported CPU architecture '{arch}' for sew-media"))?;
    if checksum_is_placeholder(a) {
        return Err("refusing to install: this build has no sew-media pin (placeholder checksum)".into());
    }
    Ok((m.version, a))
}

/// The release whose `sew` binary has this sha256 on `arch` (any pinned
/// release, old or current). `None` = an unknown binary: never run as root.
pub fn known_binary(sha256_hex: &str, arch: &str) -> Option<&'static str> {
    releases().iter().find_map(|r| {
        r.artifacts
            .iter()
            .find(|a| a.arch == arch && !checksum_is_placeholder(a) && a.binary_sha256.eq_ignore_ascii_case(sha256_hex))
            .map(|_| r.version)
    })
}

/// Semver-ish compare of two `x.y.z` Sew versions (`None` if either isn't).
pub fn version_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let pa = crate::version::Semver::parse(a)?;
    let pb = crate::version::Semver::parse(b)?;
    Some(pa.cmp(&pb))
}

// ── Validators (argv tokens) ────────────────────────────────────────────

/// `^([1-9][0-9]{0,18}|none)$` — Sew's install id, or `none` when enable
/// never reached the link step (Sew's own purge confirm form).
pub fn install_id_ok(s: &str) -> bool {
    s == "none"
        || (!s.is_empty()
            && s.len() <= 19
            && s.bytes().all(|b| b.is_ascii_digit())
            && !s.starts_with('0'))
}

/// A backup bundle id as `sew backup list` prints it: starts with a digit,
/// then `[A-Za-z0-9_-]`, at most 64. Never a dot or a slash, so `sew
/// restore <dir|tar|id>` can only read it as an id.
pub fn bundle_id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_digit()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ── Validators (stdin JSON) ─────────────────────────────────────────────

/// The master key text: 43 base64url characters (32 bytes, no padding),
/// optionally one trailing `\n` (Sew `--master-key-from`).
pub fn master_key_text_ok(s: &str) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.len() == 43 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The mgmt token Sew writes: `sewsvc_<43 base64url>`.
pub fn mgmt_token_ok(s: &str) -> bool {
    s.strip_prefix("sewsvc_")
        .is_some_and(|t| t.len() == 43 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

/// `http://127.0.0.1:<port>/v1/chat/jwks`, port 1..=65535 (S28's JWKS on
/// the daemon's loopback HTTP port). Nothing else.
pub fn jwks_url_ok(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("http://127.0.0.1:") else { return false };
    let Some(port) = rest.strip_suffix("/v1/chat/jwks") else { return false };
    port_text_ok(port)
}

fn port_text_ok(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 5
        && p.bytes().all(|b| b.is_ascii_digit())
        && !p.starts_with('0')
        && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
}

/// A DNS host name: lowercase LDH labels, ≥ 2 labels, ≤ 253 chars, no
/// leading/trailing hyphen in a label, not all-numeric TLD.
pub fn hostname_ok(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    labels.iter().all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }) && !labels.last().is_some_and(|t| t.bytes().all(|b| b.is_ascii_digit()))
}

/// `<scheme>://<host>` exactly: no port, path, query, fragment or userinfo.
fn scheme_host_ok(s: &str, scheme: &str) -> bool {
    s.strip_prefix(scheme).and_then(|r| r.strip_prefix("://")).is_some_and(hostname_ok)
}

/// `server.public_url`: `https://<host>`, or `""` to clear it.
pub fn public_url_ok(s: &str) -> bool {
    s.is_empty() || scheme_host_ok(s, "https")
}

/// An absolute, normalised path: no `..`, `.`, `//`, trailing `/`, control
/// characters or shell-meta. (Existence and link checks are the helper's.)
pub fn abs_path_ok(p: &str) -> bool {
    p.starts_with('/')
        && p.len() > 1
        && p.len() <= 1024
        && !p.ends_with('/')
        && !p.contains("//")
        && p.split('/').skip(1).all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && p.bytes().all(|b| b.is_ascii_graphic() && !b"'\"`$;&|<>\\*?[]{}()!#~".contains(&b))
}

/// Roots the keyring mirror must never point at (rev 3 G9: never the
/// vault, never an on-box system path).
pub const MIRROR_FORBIDDEN_ROOTS: &[&str] = &["/var/lib/sew", "/etc", "/run", "/tmp", "/root", "/usr", "/proc", "/sys", "/dev", "/boot", "/var/lib/k2-root-helper"];

/// `keys.mirror.path`: absolute and normalised, outside every forbidden
/// root and outside any `~/.k2` / `.k2so` (the K2 vault's home).
pub fn mirror_path_ok(p: &str) -> bool {
    abs_path_ok(p)
        && !MIRROR_FORBIDDEN_ROOTS.iter().any(|r| p == *r || p.starts_with(&format!("{r}/")))
        && !p.split('/').any(|seg| seg == ".k2" || seg == ".k2so")
}

/// `engine sew install` stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallRequest {
    /// [`jwks_url_ok`].
    pub jwks_url: String,
    /// [`master_key_text_ok`]. Piped to `sew install --master-key-from -`;
    /// never written to disk by K2.
    pub master_key: String,
}

impl InstallRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !jwks_url_ok(&self.jwks_url) {
            return Err("install refused: jwksUrl");
        }
        if !master_key_text_ok(&self.master_key) {
            return Err("install refused: masterKey");
        }
        Ok(())
    }

    /// The `sew install --config-json` body (Sew SW5 `InstallInputs`).
    /// Provider k2, profile shared (the K2 memory split), never a cred mode
    /// (Sew picks encrypted credentials on systemd ≥ 250) and never
    /// `mgmt_users` (no `sew-mgmt` group for the daemon: SW7 allow_uids).
    pub fn sew_inputs(&self) -> serde_json::Value {
        serde_json::json!({"provider": "k2", "k2_jwks_url": self.jwks_url, "profile": "shared"})
    }
}

/// `engine sew config` stdin: the four keys `sew config set` accepts, each
/// optional, each validated. Applied in a fixed order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRequest {
    #[serde(rename = "identity.k2.jwks_url", default, skip_serializing_if = "Option::is_none")]
    pub jwks_url: Option<String>,
    #[serde(rename = "server.public_url", default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(rename = "keys.mirror.kind", default, skip_serializing_if = "Option::is_none")]
    pub mirror_kind: Option<String>,
    #[serde(rename = "keys.mirror.path", default, skip_serializing_if = "Option::is_none")]
    pub mirror_path: Option<String>,
}

impl ConfigRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.jwks_url.is_none() && self.public_url.is_none() && self.mirror_kind.is_none() && self.mirror_path.is_none() {
            return Err("config refused: no keys");
        }
        if self.jwks_url.as_deref().is_some_and(|v| !jwks_url_ok(v)) {
            return Err("config refused: identity.k2.jwks_url");
        }
        if self.public_url.as_deref().is_some_and(|v| !public_url_ok(v)) {
            return Err("config refused: server.public_url");
        }
        // `k2` (an Alakazam target) stays refused until Q1 is ruled.
        if self.mirror_kind.as_deref().is_some_and(|v| v != "none" && v != "dir") {
            return Err("config refused: keys.mirror.kind");
        }
        if self.mirror_path.as_deref().is_some_and(|v| !mirror_path_ok(v)) {
            return Err("config refused: keys.mirror.path");
        }
        Ok(())
    }

    /// `(key, value)` in the order the helper applies them.
    pub fn pairs(&self) -> Vec<(&'static str, &str)> {
        let mut v = Vec::new();
        if let Some(x) = &self.jwks_url {
            v.push(("identity.k2.jwks_url", x.as_str()));
        }
        if let Some(x) = &self.public_url {
            v.push(("server.public_url", x.as_str()));
        }
        // Path before kind, so `kind = dir` never points at a stale path.
        if let Some(x) = &self.mirror_path {
            v.push(("keys.mirror.path", x.as_str()));
        }
        if let Some(x) = &self.mirror_kind {
            v.push(("keys.mirror.kind", x.as_str()));
        }
        v
    }
}

/// `engine sew media install` stdin: the subset of Sew's
/// `MediaInstallInputs` K2 may set. The tarball path is the helper's fixed
/// staging file; cert/key paths are refused here (K13 delivers the TURN
/// cert through its own verb); the cred mode is never set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct MediaRequest {
    /// `wss://<host>` (the nested `voice.<box>` label).
    pub public_url: String,
    /// `stun`, or 1–4 comma-separated literal IPs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_port: Option<u16>,
    /// `off`, `plain`, `external` or `direct`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_tls: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_advertise_port: Option<u16>,
    /// `apply` (write Sew's sysctl drop-in) or `skip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysctl: Option<String>,
}

impl MediaRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !scheme_host_ok(&self.public_url, "wss") {
            return Err("media refused: public_url");
        }
        if let Some(n) = &self.node_ip {
            let ok = n == "stun"
                || (!n.is_empty() && n.split(',').count() <= 4 && n.split(',').all(|ip| ip.parse::<std::net::IpAddr>().is_ok()));
            if !ok {
                return Err("media refused: node_ip");
            }
        }
        for p in [self.udp_port, self.tcp_port].into_iter().flatten() {
            if p < 1024 {
                return Err("media refused: port");
            }
        }
        if self.turn_tls.as_deref().is_some_and(|t| !matches!(t, "off" | "plain" | "external" | "direct")) {
            return Err("media refused: turn_tls");
        }
        if self.turn_domain.as_deref().is_some_and(|d| !hostname_ok(d)) {
            return Err("media refused: turn_domain");
        }
        if self.sysctl.as_deref().is_some_and(|s| s != "apply" && s != "skip") {
            return Err("media refused: sysctl");
        }
        Ok(())
    }

    /// The `sew media install --config-json` body: this request plus the
    /// helper's staged tarball path and the shared profile.
    pub fn sew_inputs(&self, tarball: &str) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("plain struct");
        v["tarball"] = serde_json::json!(tarball);
        v["profile"] = serde_json::json!("shared");
        v
    }
}

/// The 16-hex KEK id Sew reports (`status.keys.kek_id`) for a master key:
/// formula from Sew S4 §5.3 (`blake3::derive_key("sew 2026-09-27 kek id v1",
/// key)[0..8]` big-endian `& 0x7fff_ffff_ffff_ffff`). The proto crate has no
/// blake3, so the daemon computes the derive and this formats the id.
pub fn kek_id_from_derived(derived: &[u8; 32]) -> String {
    let mut b = [0u8; 8];
    b.copy_from_slice(&derived[..8]);
    format!("{:016x}", u64::from_be_bytes(b) & 0x7fff_ffff_ffff_ffff)
}

pub const KEK_ID_CONTEXT: &str = "sew 2026-09-27 kek id v1";

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn real_table_is_placeholder_and_refuses_before_any_effect() {
        let pin = SEW_RELEASES.last().unwrap();
        for a in pin.artifacts {
            assert!(checksum_is_placeholder(a), "{a:?}");
        }
        assert!(artifact_in(pin, "x86_64").unwrap_err().contains("no Sew pin"));
        assert!(artifact_in(pin, "riscv64").unwrap_err().contains("unsupported"));
    }

    #[test]
    fn test_table_pin_is_last_and_known_binary_covers_every_release() {
        let pin = TEST_RELEASES.last().unwrap();
        assert_eq!(pin.version, "0.0.2");
        for r in TEST_RELEASES {
            for a in r.artifacts {
                assert!(!checksum_is_placeholder(a));
            }
        }
        if cfg!(feature = "test-signer") {
            assert_eq!(pinned().version, "0.0.2");
            assert_eq!(known_binary(TEST_RELEASES[0].artifacts[0].binary_sha256, "x86_64"), Some("0.0.1"));
            assert_eq!(known_binary(pin.artifacts[0].binary_sha256, "x86_64"), Some("0.0.2"));
            assert_eq!(known_binary(pin.artifacts[0].binary_sha256, "aarch64"), None);
            assert_eq!(known_binary(&"0".repeat(64), "x86_64"), None);
            // A tarball sha is never a binary sha.
            assert_eq!(known_binary(pin.artifacts[0].tarball_sha256, "x86_64"), None);
        } else {
            assert_eq!(pinned().version, "0.1.0");
            assert!(pinned_artifact("x86_64").is_err());
            assert_eq!(known_binary(&"0".repeat(64), "x86_64"), None);
        }
    }

    #[test]
    fn fixture_tarballs_match_the_test_table() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sew");
        for (file, want) in [
            ("sew-0.0.1-x86_64-unknown-linux-gnu.tar.gz", TEST_RELEASES[0].artifacts[0].tarball_sha256),
            ("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz", TEST_RELEASES[1].artifacts[0].tarball_sha256),
            (
                "sew-media-0.0.2-x86_64-unknown-linux-gnu.tar.gz",
                TEST_RELEASES[1].media.as_ref().unwrap().artifacts[0].tarball_sha256,
            ),
        ] {
            let b = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(sha256_hex(&b), want, "{file}");
        }
    }

    #[test]
    fn install_ids_and_bundle_ids() {
        for ok in ["1", "48831765147549701", "9999999999999999999", "none"] {
            assert!(install_id_ok(ok), "{ok}");
        }
        for bad in ["", "0", "01", "10000000000000000000", "abc", "none ", "-1", "1;id", "NONE"] {
            assert!(!install_id_ok(bad), "{bad}");
        }
        for ok in ["20260927T031512Z", "20260927T031512Z-manual", "1"] {
            assert!(bundle_id_ok(ok), "{ok}");
        }
        for bad in ["", ".", "..", "/var/lib/sew", "a2026", "2026/..", "2026.tar", "20260927T031512Z;id", &"1".repeat(65)] {
            assert!(!bundle_id_ok(bad), "{bad}");
        }
    }

    #[test]
    fn key_token_and_jwks_shapes() {
        let k = "A".repeat(43);
        assert!(master_key_text_ok(&k));
        assert!(master_key_text_ok(&format!("{k}\n")));
        for bad in [&"A".repeat(42), &"A".repeat(44), &format!("{}=", "A".repeat(42)), &format!("{}+", "A".repeat(42)), &format!("{k}\n\n")] {
            assert!(!master_key_text_ok(bad), "{bad}");
        }
        assert!(mgmt_token_ok(&format!("sewsvc_{k}")));
        assert!(!mgmt_token_ok(&k));
        assert!(!mgmt_token_ok(&format!("sewsvc_{k}\n")));
        assert!(jwks_url_ok("http://127.0.0.1:38471/v1/chat/jwks"));
        for bad in [
            "https://127.0.0.1:38471/v1/chat/jwks",
            "http://127.0.0.1:0/v1/chat/jwks",
            "http://127.0.0.1:65536/v1/chat/jwks",
            "http://127.0.0.1:038471/v1/chat/jwks",
            "http://localhost:38471/v1/chat/jwks",
            "http://127.0.0.1:38471/v1/chat/jwks?x=1",
            "http://127.0.0.1:38471/v1/jwks",
            "http://evil@127.0.0.1:38471/v1/chat/jwks",
        ] {
            assert!(!jwks_url_ok(bad), "{bad}");
        }
    }

    #[test]
    fn public_url_and_mirror_path_rules() {
        assert!(public_url_ok(""));
        assert!(public_url_ok("https://chat.acme.dev"));
        for bad in ["http://chat.acme.dev", "https://chat.acme.dev/", "https://chat.acme.dev:443", "https://Chat.acme.dev", "https://localhost", "https://a.b/x", "https://u@a.b"] {
            assert!(!public_url_ok(bad), "{bad}");
        }
        assert!(mirror_path_ok("/mnt/offbox/sew-keyring"));
        for bad in [
            "relative",
            "/",
            "/mnt/offbox/",
            "/mnt//x",
            "/mnt/../etc",
            "/var/lib/sew",
            "/var/lib/sew/keyring",
            "/etc/sew",
            "/tmp/x",
            "/root/x",
            "/home/k2/.k2/vault",
            "/home/k2/.k2so",
            "/mnt/a b",
            "/mnt/$(id)",
        ] {
            assert!(!mirror_path_ok(bad), "{bad}");
        }
    }

    #[test]
    fn stdin_json_is_strict() {
        let ok = r#"{"jwksUrl":"http://127.0.0.1:38471/v1/chat/jwks","masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#;
        let r: InstallRequest = serde_json::from_str(ok).unwrap();
        r.validate().unwrap();
        assert_eq!(r.sew_inputs()["provider"], "k2");
        assert!(r.sew_inputs().get("mgmt_users").is_none(), "no sew-mgmt group (SW7)");
        assert!(r.sew_inputs().get("cred_mode").is_none(), "never a plain cred mode");
        // Unknown keys (a path, a provider, a cred mode) never parse.
        for bad in [
            r#"{"jwksUrl":"http://127.0.0.1:1/v1/chat/jwks","masterKey":"x","provider":"standalone"}"#,
            r#"{"jwksUrl":"http://127.0.0.1:1/v1/chat/jwks","masterKey":"x","cred_mode":"plain"}"#,
            r#"{"jwksUrl":"http://127.0.0.1:1/v1/chat/jwks"}"#,
        ] {
            assert!(serde_json::from_str::<InstallRequest>(bad).is_err(), "{bad}");
        }
        let c: ConfigRequest = serde_json::from_str(r#"{"keys.mirror.kind":"k2"}"#).unwrap();
        assert!(c.validate().is_err(), "k2 mirror kind waits for Q1");
        assert!(serde_json::from_str::<ConfigRequest>(r#"{"mgmt.allow_uids":"0"}"#).is_err());
        assert!(ConfigRequest::default().validate().is_err());
        let c: ConfigRequest = serde_json::from_str(r#"{"keys.mirror.kind":"dir","keys.mirror.path":"/mnt/o"}"#).unwrap();
        c.validate().unwrap();
        assert_eq!(c.pairs(), vec![("keys.mirror.path", "/mnt/o"), ("keys.mirror.kind", "dir")]);
        let m: MediaRequest = serde_json::from_str(r#"{"public_url":"wss://voice.box.k2.dev","sysctl":"apply","node_ip":"stun"}"#).unwrap();
        m.validate().unwrap();
        assert_eq!(m.sew_inputs("/x")["tarball"], "/x");
        for bad in [
            r#"{"public_url":"wss://voice.box.k2.dev","turn_cert":"/etc/shadow"}"#,
            r#"{"public_url":"wss://voice.box.k2.dev","tarball":"/tmp/x"}"#,
            r#"{"public_url":"wss://voice.box.k2.dev","cred_mode":"plain"}"#,
        ] {
            assert!(serde_json::from_str::<MediaRequest>(bad).is_err(), "{bad}");
        }
        for bad in [
            r#"{"public_url":"https://voice.box.k2.dev"}"#,
            r#"{"public_url":"wss://voice.box.k2.dev","sysctl":"system"}"#,
            r#"{"public_url":"wss://voice.box.k2.dev","node_ip":"evil;id"}"#,
            r#"{"public_url":"wss://voice.box.k2.dev","udp_port":53}"#,
        ] {
            let m: MediaRequest = serde_json::from_str(bad).unwrap();
            assert!(m.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn kek_id_format() {
        let mut d = [0u8; 32];
        d[0] = 0xff;
        d[7] = 0x01;
        assert_eq!(kek_id_from_derived(&d), "7f00000000000001");
    }
}
