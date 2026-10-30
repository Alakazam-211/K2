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
//! **The pin is Sew 0.3.0 (2026-10-19, for K2 0.46.3)**, x86_64 and
//! aarch64, with its media, transcriber and recorder sidecars. The 0.1.0
//! placeholder, 0.2.0 and 0.2.1 entries stay (a pin bump appends; the
//! helper still recognises 0.2.0 and 0.2.1 on a box as upgrade sources). While the last entry is a placeholder
//! [`pinned_artifact`] refuses ("this build has no Sew pin"), so enable
//! refuses at preflight (`sew_not_released`). Builds with the `test-signer`
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
/// `root:sew 0640`. Sew's file: K2 never edits it. The daemon's uid
/// reaches `[mgmt] allow_uids` through `sew install`'s own
/// `mgmt_allow_uids` input (Sew SW5 + SW7).
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
/// The pinned tarball's `sew` member (root 0700, sha256 = the pin's
/// `binary_sha256`), extracted by `engine sew upgrade` for the INSTALLED
/// Sew's `sew upgrade --binary <this> --sha256 <binary_sha256>`: an older
/// Sew (0.1.x) refuses a release tarball with more than one member, and
/// 0.2.0 on ship `LICENSE`, `NOTICE` and `THIRD_PARTY_NOTICES.md` beside
/// `sew`. Removed after the call.
pub const SEW_STAGE_UPGRADE_BIN: &str = "/var/lib/k2-root-helper/sew/sew.new";
/// The voice-engine tarballs the helper KEEPS (root 0700 dir, 0600 files,
/// `sew-media-<version>.tar.gz`), as `/usr/share/doc/sew/previous` keeps the
/// old notices: the pin's, after a successful `media install` / `media
/// upgrade`, and an older release's when the daemon stages it for a
/// rollback. A rollback has the OLD core reinstall the previous pin's
/// engine from here (`media rollback`): sidecar pins are baked into `sew`,
/// so only that core accepts it. Never Sew's `sew-media.prev` (that is
/// Sew's own undo for an engine that won't start). Removed by a purge.
pub const SEW_KEEP_DIR: &str = "/var/lib/k2-root-helper/sew/kept";
/// At most this many kept engine tarballs (oldest dropped first).
pub const SEW_KEEP_MAX: usize = 3;

/// The kept engine tarball of sew-media `version` (a validated x.y.z).
pub fn kept_media_path(version: &str) -> Option<String> {
    crate::version::Semver::parse(version)?;
    Some(format!("{SEW_KEEP_DIR}/sew-media-{version}.tar.gz"))
}
pub const SEW_STAGE_INSTALL_JSON: &str = "/var/lib/k2-root-helper/sew/install.json";
pub const SEW_STAGE_MEDIA_JSON: &str = "/var/lib/k2-root-helper/sew/media.json";
/// The TURN TLS chain and key the daemon hands over (K13), written 0600
/// root for the one `sew media install|turn-cert` call that reads them and
/// removed right after (Sew copies them into its own credential store).
pub const SEW_STAGE_TURN_CERT: &str = "/var/lib/k2-root-helper/sew/turn-fullchain.pem";
pub const SEW_STAGE_TURN_KEY: &str = "/var/lib/k2-root-helper/sew/turn-key.pem";
/// Upper bounds of the TURN chain / key PEM text on stdin.
pub const MAX_TURN_CHAIN: usize = 16 * 1024;
pub const MAX_TURN_KEY: usize = 4 * 1024;

/// Sew's units (written by `sew install`, never by K2).
pub const UNIT_DB: &str = "sew-db.service";
pub const UNIT_MIGRATE: &str = "sew-migrate.service";
pub const UNIT_SEW: &str = "sew.service";
pub const UNIT_BACKUP_TIMER: &str = "sew-backup.timer";
pub const UNIT_DRILL_TIMER: &str = "sew-drill.timer";
pub const UNIT_MEDIA: &str = "sew-media.service";

// ── K2's own teardown of an unrecognised Sew (0.46.3) ───────────────────
//
// `engine sew purge` on a box whose `/usr/local/bin/sew` is missing or not
// a release this build knows: the helper never executes that binary, so it
// does what `sew uninstall --purge` does itself (sew-ops `install.rs`
// `uninstall`, media `uninstall`, `sysctl::remove`), on Sew's fixed layout
// (`sew-deploy/src/paths.rs`), plus the binaries. Users and groups stay
// (Sew's own uninstall keeps them too).

/// Sew's own staging name for an upgrade's atomic swap (`sew upgrade`).
pub const SEW_BIN_SEW_STAGING: &str = "/usr/local/bin/.sew.new";
/// `sew.prev`, `sew.prev.json`, `upgrade-state.json`, and the sidecars'
/// directories (`media/`, `recorder/`, `transcriber/`).
pub const SEW_LIB_DIR: &str = "/usr/local/lib/sew";
/// Shared read-only data (the speech models under `asr/`).
pub const SEW_SHARE_DIR: &str = "/usr/local/share/sew";
/// The sysctl originals `sew media install` recorded before it raised them
/// (`{"format":1,"rmem_max":N,"wmem_max":N,…}`, root 0600). Read BEFORE
/// `/etc/sew` goes.
pub const SEW_MEDIA_SYSCTL_BASELINE: &str = "/etc/sew/media-sysctl.baseline";
/// The live values the drop-in raises.
pub const PROC_RMEM_MAX: &str = "/proc/sys/net/core/rmem_max";
pub const PROC_WMEM_MAX: &str = "/proc/sys/net/core/wmem_max";
/// The recovery text `sew install` writes when it generates the key. K2
/// passes the key on stdin, so it is normally absent; a teardown KEEPS it
/// (it is a key the operator may still want) and reports it.
pub const SEW_RECOVERY_FILE: &str = "/root/sew-master-key.recovery";
/// systemd's credential stores (encrypted ≥ 250, plain 249).
pub const CREDSTORE_DIRS: [&str; 2] = ["/etc/credstore.encrypted", "/etc/credstore"];
/// Sew's credential names: the master key and the retiring one (Sew's
/// purge), and every name the media sidecars own (`sew-media-…`,
/// `sew-recorder-…`, `sew-transcriber-…`, Sew's own cleanup rule). Never
/// a wider glob.
pub const SEW_CRED_NAMES: [&str; 2] = ["sew-master-key", "sew-master-key-prev"];
pub const SEW_CRED_PREFIXES: [&str; 3] = ["sew-media-", "sew-recorder-", "sew-transcriber-"];
/// Sew's system accounts (kept by a teardown; checked for login rights).
pub const SEW_ACCOUNTS: [&str; 5] = ["sew", "sew-db", "sew-media", "sew-transcriber", "sew-recorder"];
/// The largest baseline file read (Sew writes ~100 bytes).
pub const MAX_SYSCTL_BASELINE: usize = 4096;
/// Largest sysctl value accepted from a baseline (`i32::MAX`).
pub const MAX_SYSCTL_VALUE: u64 = 2_147_483_647;

/// Is `name` a unit file (or an enablement link) in Sew's namespace:
/// `sew.service`, `sew-<name>.service`, `sew-<name>.timer`, a template
/// `sew-<name>@.service`, or an instance `sew-<name>@<inst>.service`.
/// `<name>` is `[a-z0-9-]+`, `<inst>` `[A-Za-z0-9_:-]*`. Nothing outside
/// the `sew` / `sew-` prefix ever matches.
pub fn sew_unit_name_ok(name: &str) -> bool {
    if name == UNIT_SEW {
        return true;
    }
    let Some(rest) = name.strip_prefix("sew-") else { return false };
    let Some(mid) = rest.strip_suffix(".service").or_else(|| rest.strip_suffix(".timer")) else { return false };
    let (base, inst) = match mid.split_once('@') {
        Some((b, i)) => (b, Some(i)),
        None => (mid, None),
    };
    if base.is_empty() || !base.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return false;
    }
    match inst {
        None => true,
        // Templates and their instances are services only.
        Some(i) => {
            name.ends_with(".service") && i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':' || b == b'-')
        }
    }
}

/// A unit template (`sew-backup@.service`): never `disable`d by name, its
/// instances are stopped by pattern.
pub fn sew_unit_is_template(name: &str) -> bool {
    name.ends_with("@.service")
}

/// Sew's recorded sysctl originals, strictly: at most
/// [`MAX_SYSCTL_BASELINE`] bytes, a JSON object with `format == 1` and
/// `rmem_max` / `wmem_max` integers in `1..=`[`MAX_SYSCTL_VALUE`]. Root's
/// file, but read as untrusted input: anything else is `None`.
pub fn parse_sysctl_baseline(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() > MAX_SYSCTL_BASELINE {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let o = v.as_object()?;
    if o.get("format")?.as_u64()? != 1 {
        return None;
    }
    let get = |k: &str| o.get(k)?.as_u64().filter(|n| (1..=MAX_SYSCTL_VALUE).contains(n));
    Some((get("rmem_max")?, get("wmem_max")?))
}

/// Sew's client API on loopback (C5).
pub const SEW_CLIENT_PORT: u16 = 8470;
/// Voice: sew-media's loopback signalling port and its public ports
/// (TCP 7881, UDP 7882, TURN-TLS TCP 5349). TURN on :443 goes through
/// K2's front443 SNI router (0.46.1, `passthrough` tier: sew-media on
/// 127.0.0.1:5349, PROXY v2 first); Stalwart may sit behind the same front
/// (opt-in; Rosson 2026-10-17 reversed the 2026-10-15 "never on a box where
/// Stalwart holds :443" ruling for the front).
pub const SEW_MEDIA_LOOPBACK_PORT: u16 = 7880;
pub const SEW_MEDIA_TCP_PORT: u16 = 7881;
pub const SEW_MEDIA_UDP_PORT: u16 = 7882;
pub const SEW_TURN_TLS_PORT: u16 = 5349;

/// Upper bound of a release tarball on stdin (mirrors Sew's own cap).
pub const MAX_TARBALL: usize = 256 * 1024 * 1024;
/// Upper bound of the extracted `sew` / `sew-media` member.
pub const MAX_MEMBER: u64 = 256 * 1024 * 1024;

// ── Licence notices (Sew SW1 B1, from 0.2.0; helper protocol 8) ─────────

/// The licence files a release tarball carries beside its binary (0.2.0
/// on; older tarballs have none). Each binary has its own `LICENSE` and
/// `NOTICE`, so each gets its own directory.
pub const NOTICE_MEMBERS: [&str; 3] = ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md"];
/// Upper bound of one notice member (THIRD_PARTY_NOTICES.md is ~1.3 MB).
pub const MAX_NOTICE: u64 = 8 * 1024 * 1024;
/// The parent of the notice directories (created 0755 root if missing).
pub const DOC_ROOT: &str = "/usr/share/doc";
/// `sew`'s notices (dir root 0755, files root 0644), so the attribution
/// travels with the binary the helper installs: written by `install-bin`
/// (before the binary) and after a successful `upgrade`, from the pinned
/// tarball; removed by a successful `purge`.
pub const SEW_DOC_DIR: &str = "/usr/share/doc/sew";
/// The notices of the binary Sew keeps for rollback
/// (`/usr/local/lib/sew/sew.prev`): a successful `upgrade` moves the old
/// notices here; a successful `rollback` swaps the two sets, exactly as
/// Sew swaps `sew` and `sew.prev`; a purge removes both.
pub const SEW_DOC_PREV_DIR: &str = "/usr/share/doc/sew/previous";
/// `sew-media`'s notices, from the staged, pinned media tarball after a
/// successful `media install`; removed by a successful `media uninstall`.
pub const SEW_MEDIA_DOC_DIR: &str = "/usr/share/doc/sew-media";

// ── Pin table ───────────────────────────────────────────────────────────

/// One per-arch artifact: tarball sha256 + the sha256 of the binary member.
#[derive(Debug)]
pub struct SewArtifact {
    pub arch: &'static str,
    pub triple: &'static str,
    pub tarball_sha256: &'static str,
    pub binary_sha256: &'static str,
}

/// A sidecar of one release (`sew-media`, `sew-transcriber`,
/// `sew-recorder`): its own version and artifacts (`release.json`
/// `sidecars[]`).
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
    /// Voice (`sew-media`, K13): staged and installed through the helper.
    pub media: Option<SewSidecar>,
    /// Recording (`sew-transcriber`, `sew-recorder`): pinned from 0.2.0 so
    /// the bytes are vouched for; no helper verb installs them yet
    /// (`k2 hostchat recording enable`, K-M11, is later work).
    pub transcriber: Option<SewSidecar>,
    pub recorder: Option<SewSidecar>,
}

/// The PostgreSQL major Sew's private `sew-db` runs on, for the pinned
/// releases (0.2.x, 0.3.x). Always passed as `sew install`'s `pg_major`:
/// without it Sew takes the highest major under `/usr/lib/postgresql` at
/// install time, and from 0.46.3 K2 installs PGDG 17 next to 16 for its own
/// databases, so a fresh install could silently land on 17. Moves with the
/// pin (a Sew release that wants another major bumps it here, in the same
/// change). sew-db is socket-only (`/run/sew-db`, no TCP), so it never
/// clashes with K2's clusters; the `postgresql-16` package must stay
/// installed on a box with Sew (the preflight checks these binaries).
pub const SEW_PG_MAJOR: u32 = 16;

/// The server binaries [`SEW_PG_MAJOR`] needs (`/usr/lib/postgresql/<M>/bin/`).
pub const SEW_PG_BINARIES: [&str; 4] = ["postgres", "initdb", "pg_dump", "pg_restore"];

/// Every Sew release K2 has ever pinned, oldest first. The LAST entry is
/// the pin. A pin bump appends: old entries stay so the helper still
/// recognises the binary already on a box (upgrade source, rollback).
///
/// - 0.1.0: a placeholder from before Sew published SW1 (never installable).
/// - 0.2.0: SW1, `Alakazam-211/sew-releases` v0.2.0 (Sew main `febfe626`),
///   pinned 2026-10-17. Printed by `scripts/sew-pins-from-release.sh` from
///   that release's `release.json` + SHA256SUMS (SHA256SUMS sha256
///   `ae35e326…07942`, release.json `626dbe5d…2c62c`), which also checked
///   every tarball and binary member. Tarballs carry LICENSE, NOTICE and
///   THIRD_PARTY_NOTICES.md beside the binary.
/// - 0.2.1: `Alakazam-211/sew-releases` v0.2.1 (core tag v0.2.1
///   `13842109cfc9…`), pinned 2026-10-18: 0.2.0 plus TURN passthrough (the
///   front443 `passthrough` tier). Printed by `scripts/sew-pins-from-release.sh`
///   (SHA256SUMS sha256 `21f12773…4b55`, release.json `755ca945…dc2d`), which
///   checked every tarball and binary member. Same schema max as 0.2.0
///   (920002); 0.1.2 → 0.2.1 takes the 0.2.0 upgrade path.
/// - 0.3.0: `Alakazam-211/sew-releases` v0.3.0 (core release-0.3.0 @
///   `8252db23` = main `a9e7734e` + bump), pinned 2026-10-19 for K2 0.46.3.
///   Printed by `scripts/sew-pins-from-release.sh` from the final bytes
///   (SHA256SUMS sha256 `9db09d0a…fa43`, release.json `644404fc…a8f7`),
///   which checked every tarball and binary member; the GitHub release is
///   published after Rosson's yes (bytes final, unchanged). Schema max
///   930003. Sew tested 0.2.1 → 0.3.0 and 0.1.2 → 0.3.0 real-tarball
///   upgrade + rollback on systemd.
/// - 0.3.1: `Alakazam-211/sew-releases` v0.3.1 (tag `f4575953…19f0`),
///   pinned 2026-10-27 for K2 0.46.6 after Rosson's yes and Sew's publish
///   check. Printed by `scripts/sew-pins-from-release.sh` from the
///   published assets (SHA256SUMS sha256 `c7e82dc1…2bba`, release.json
///   `8789dc47…582f`), which checked every tarball and binary member; the
///   SUMS file is vendored as `testdata/sew/sew-0.3.1.SHA256SUMS` and a test
///   holds this entry to it. Schema max 930003 (as 0.3.0). The health
///   contract (`ready`, `reason`, `since`). 0.3.0 → 0.3.1 runs `engine sew
///   install reconfigure` (protocol 11). Sew tested 0.3.0 → 0.3.1 with
///   rollback, re-upgrade and `sew install --reconfigure`, 0.2.1 → 0.3.1
///   via `--tarball`, 0.1.2 → 0.3.1 via `--binary --sha256`.
/// - 0.5.0: "Sew for Apps" + the App kit. `Alakazam-211/sew-releases`
///   v0.5.0 (a normal release, not a pre-release; Sew tag v0.5.0 = core
///   `e8bedc517934b9a19b2f60a1e7e959bfe1510a0c`), pinned 2026-10-28 for K2
///   0.46.6 (Rosson: ships once Sew's cut passes its smoke run). Printed by
///   `scripts/sew-pins-from-release.sh` from the published assets
///   (SHA256SUMS sha256 `1fc29f14…e276`, release.json `b4afa995…3ac9`),
///   which checked every tarball and binary member and the App kit
///   (`sew-app-kit-0.5.0.tar.gz`, [`SEW_APP_KITS`]); every hash also equals
///   Sew's own release entry. The SUMS file is vendored as
///   `testdata/sew/sew-0.5.0.SHA256SUMS` and a test holds this entry to it.
///   Schema max 951201 (0.3.1: 930003), so a rollback to 0.3.1 crosses a
///   schema change. Features: app_channel_create, app_origin, app_quotas,
///   app_scoped_sessions, home_client, k2_managed_avatar,
///   member_external_id, person_notifications, principal_relink,
///   private_spaces, space_unbind, voice_video_cap. Sew tested 0.3.1 →
///   0.5.0 with `install --reconfigure`, rollback (with
///   `--accept-data-loss-since`) and re-upgrade on an aarch64 VM.
pub const SEW_RELEASES: &[SewRelease] = &[
    SewRelease {
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
        transcriber: None,
        recorder: None,
    },
    SewRelease {
        version: "0.2.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "f8b51c780eb1d89775d55bb3756d192b8d488f142394fb59918e00d080abf16b",
                binary_sha256: "8cafa9da19756720f329038806b2e396f5ef24ecd6233f0a82f2db7f37ea4c96",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "908dd7d8a5026022b76595091f8e21af6c8fb37f7486ecd13149e52173f42229",
                binary_sha256: "ef20c6ebb7c253c48fbf64e8372f58c596ce33bbbfe12d6dc9183005b66f9262",
            },
        ],
        media: Some(SewSidecar {
            version: "0.2.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "545a81f91a4fb3816bfa4fa64aa9e475e088155f415fbf9eae4597d0ab39bab1",
                    binary_sha256: "44404a5fabd72e1cdb2488c746289d01fb1f34d46aeb767c8a13ddb8288ce71a",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "6d5c64c2263d7eca0b465d69c114996da42676e38afa9505c6c441dd5797473d",
                    binary_sha256: "195564b6cab0170bc1d823c26c8b06d364ab5c28bf14f695af1595127a4124a6",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.2.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "0d42a591eb792952e77f23cc5ecb00a1ddbca622365d56b4383f608ffce42edf",
                    binary_sha256: "a312abd60383dd3a015279fcc4294d76f1b93b528c198100b2584bbf7dba2d80",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "d438161714ff901449a442afd412d0583ed06aa8d596aa8af8d6afaf1c8b0a86",
                    binary_sha256: "719de7f8ced63a5344eb76b064df7d7a17e9140791c44da1649642fe899ecfaa",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.2.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "8bfbdc2c324c0fafcb5ce9e5ef0f5c702df1f1652d298133e0c89a1d585182ab",
                    binary_sha256: "11ae0573d30a54ba0ac725f5b04f6492e140c63062923d8f195992257c260a2a",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "4b2c956bceec832b66342aa2bd8db6f95c09384db6dceeb5e2b0b7774cb51a4c",
                    binary_sha256: "ea5e9cad87d232edf9599ca3947950e7b2ddf7f468d7b816322e2f55d019a7d2",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.2.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "8116e64ae13d43da196b77fbe1ff02ef81d9860212de3f7585094ed83b606ee2",
                binary_sha256: "f909915d05886752f5a496c895b7ad248f79e1f5d32a95a3ef1c65f578be3cd7",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "6cad696d906b250301fe34e44c080bc127025012257a562622ee85d1488cafcc",
                binary_sha256: "cf456d16f2031b7b2b952ab10c333bdaea8a421aed435dfce6ae033f500a8a6e",
            },
        ],
        media: Some(SewSidecar {
            version: "0.2.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "cfac4231e141ab0678778c40787c2186c13a43c2cefe7c6a1399b2bd0fabe401",
                    binary_sha256: "e806246429f954899630158d0da11e5ed6caaa7157d18cbd24599493f202456c",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "abcecd1e5e6523e2a8cc6549beb299ffab28d3fa8960eb0dbda91287bbcfbed4",
                    binary_sha256: "7fbbbb9a6b46d9e76b4c2911eaa90715d61bcb8c8a8078d6a78201c46d1f7884",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.2.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "d3671fd457d2cfcbedab9f8b9376840f1a3a530addfcb5bbf0b4b6a8300b108f",
                    binary_sha256: "1ca098c50c82975bdb6acf9e635caeb5b17245625ed0aaaf8ef9f3d81ce8acf5",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "f840a861c9e4536e5777698f2c24475dc5bfa688a16f5223fec9c5e5e036675b",
                    binary_sha256: "b20b1684abdf8add1bf63b84bfa747b91dba4650c0dce9c1ef04ce12714434ce",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.2.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "79db2e6348bcbb64c4dd0b6bfb947882110f4e334020bcf61e52783a599dbcd1",
                    binary_sha256: "6c62db7d4fad906c6b22cf6b7175257f9f50fd0af5f5d127841f0b3bd5470dda",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "c45032512d75347603712369980197f9b92fb346a3d59c985b6739fb0615fd0b",
                    binary_sha256: "ad50834969fc941951b5b2fb0b122118d6572b6391bf0b72fb6c51cb92f1ef04",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.3.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "297d556d314db538ce1e88cd7298adfd34be98443dce9cf9ae52c610899ca337",
                binary_sha256: "fbbb513dfa2d701e59c93ad68f5b1657008966d330cb6b0859c099ef35c07ba6",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "2275933841a9194c035e1c558c7c80233bab0be7acea1b6669fda901401d5fb5",
                binary_sha256: "39bbaa5eb0244ac3e6bf1efe3df5d080ef433859b5337803bd246db24f7d91dc",
            },
        ],
        media: Some(SewSidecar {
            version: "0.3.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "0bf34c004069a986c5daacde9ca2bad034ee4a74cac4bfce6ae0da21b3a451af",
                    binary_sha256: "33d29b16e37bd0ffb3c16ae72058542042e8b1a17b4017b1ab0fd97ba64bf06e",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "526e1ccc1aef0e0fb44e2c5cea62b2e62cc406ad0edeb792857c610f8d76fed3",
                    binary_sha256: "8c575d842b85e576e4f869ccd9e3d1fdf3edd042c39ba1641301271e90a0f765",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.3.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "22cadc920961c879d2bbcc26d1df1461796eefbc3a73f6e1891d67b850959389",
                    binary_sha256: "a89a2013bc03d69e7fdf1b4ee77d3015b68336eb9e7ba58c2264717a88576f1b",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "a0dbbae349be127cbe034fc58aee0aeed7f78bc3c9378e5ca0c53cb3ce839ecd",
                    binary_sha256: "a46a7076dcaa9601edf841e2415fa9626e90261b3d4654795b3ec5e9d8c61ce2",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.3.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "3eb07945afcd8f5078813b9d6751b358d167d02dc5463af194d55c27914877c1",
                    binary_sha256: "f9a111a7daf0e0420e4fe0deae768cfac1a0d291b5f54dd667a0b6e2124061ed",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "3f3db7cc984e7df6a33e68d3e905e34ebfea165c3dadf082c6694bceaba4d976",
                    binary_sha256: "a39c6a9950283cab0563daede43a61353f7b832654b4c9a1621acb2b4a933c3e",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.3.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "88e3e5ea39add6f2e53f136de407ca5e3aae3ce672a3cabb181d989656a3584c",
                binary_sha256: "53d66e9d38a582a8721302832d6abf43aee2d6d82e6c172fb162ddd8cda1b2fa",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "db9f8b783449023fc4f4bc98257354afde95c57dae9222d413c1729cfbc39ae1",
                binary_sha256: "60a6b5a233aca211f1cafb382468bbb26c937de6170e72bcbe7c91f7b7dc102b",
            },
        ],
        media: Some(SewSidecar {
            version: "0.3.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "fdf984f8b3d701f3f11b6256dd7c96d4255214d73d892d942cfbc666e472d630",
                    binary_sha256: "bafb622af28888d6fd8defd08e5b51c1b92c3613e8cadcf0008df5cd97d169b4",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "24d97dbe93a190d9e17497cbc4ba5ac396c03792e147cadfddc444c281c47ab9",
                    binary_sha256: "f7a6b77c10013b771c22188b1642f175c0a55f58c3379ca0d427019b1eb62c34",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.3.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "e451f6bb76be98dca87ad64624aea577d871ad06dac6c6913a6a7302f442edf4",
                    binary_sha256: "4f9c55126790351f382f1f57cbfc59131cf3202d396b8294ba685ab801d8e2e4",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "97ffd5e7d107ad548420355ebb7d55f59c3e2db598cbf225380717db6499647f",
                    binary_sha256: "96d6727240fcaf021462e5a755a210c41a7deabb84dbf183f5254b900c21ed4f",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.3.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "74d5b6ae63b58c5a27279fb811c4e959d23f74bdd2fa7c8b551d746cd27ed01e",
                    binary_sha256: "56b8b7597e634c563a6eb8d5a3298a17e0eb1429ab440bb9e6c357175246a270",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "e9af0ca85919881ab1abeb04da901c3bfbea39057f931a12bef73dff008881e0",
                    binary_sha256: "9e07aecd8842e97a50936c74d05fe7ce080d2d867b450623420b156df126ce80",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.5.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "77e0995cd6865e2dce10da95c6e1b767a9f926bbf2c7e625623375ca6291c1ef",
                binary_sha256: "cfeb9e2b504f752f2b37ca925010cd2fc854df9d67321bb7f15adc6a6a834708",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "5dd2c9d2437045315e3b95709bfe34bbaebcee22a6389ac8046431c30a533461",
                binary_sha256: "5cbda1474dff81332b4b1222c058b71c8826516105a7428a5c23e356f8575c1f",
            },
        ],
        media: Some(SewSidecar {
            version: "0.5.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "f83ab83f13e1d2eb95cb036e9111cfdfe3251431a42362eecb989073591a2b0d",
                    binary_sha256: "3aa2a1abc6fe328b3eef506ee68ad8bf0bf902b8116f325a022d9495a298f6f9",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "1fac9dd3ba956b83ebe7037a85944fd035de885a858901a74ab2c9046ea5ee39",
                    binary_sha256: "a2d86929a05831a7c357d32e33ff1e4acad3f83c6c80ffab84c62093c0f69572",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.5.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "f1d1ed60a27cb4eaa99446d0fd9104e60a5cd2683d22aeac0ccab76a50537e59",
                    binary_sha256: "410d873533d967648589bdb44a08cc456fcaa57920fe3fe1b6820a3a7a7acf54",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "d579b787993373a0d70952f7bea7a817aea8ea2152d57219e03ae1a5c8d227bf",
                    binary_sha256: "d2a87ce469eca619fa38fe8dc49c2238886498b2edead8746aa2244ce1bff166",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.5.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "4c6e5e2e2b5a80d4ebe380ffd841923914831b27c1866d7f13f977258945b866",
                    binary_sha256: "2b6e1c7d13dbebb76bb916a70dd4455eb60cbbfcf7f52714038a77cbeeca0079",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "ad31045f0f20701976e07561a9b06d2e320d78f3df4b8d3e9ef33b0a73f150a5",
                    binary_sha256: "1a54b9184079f72c9d8a9ce62de28a7b4a6b91ead5bede791f939993561dd4d4",
                },
            ],
        }),
    },
];

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
        // The previous pin's voice engine: what a rollback to 0.0.1 puts back.
        media: Some(SewSidecar {
            version: "0.0.1",
            artifacts: &[SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "19b51c285aae8c5d2b7f58df9a1d3c8e41290f2c7e361c0e2d764a094f710a3e",
                binary_sha256: "331e90e8cab2d96ee91928ae6a348201b52ff6440d6af56fbbdcef1bb7b62e27",
            }],
        }),
        transcriber: None,
        recorder: None,
    },
    SewRelease {
        version: "0.0.2",
        artifacts: &[SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "d73db8b4ab25f2a02ad63e5b9c6f5e1395f77586eda83794bfd6a898f6d4e1a5",
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
        transcriber: None,
        recorder: None,
    },
];

/// Sew's TEST builds (`/Users/Shared/sew-test-builds/<v>/K2-PIN.txt`, Sew
/// main `901b20c7` = 0.1.0 and `aa355dbe` = 0.1.1, aarch64 only), for the
/// S29 cycle in a throwaway VM before SW1 publishes a release. Compiled
/// ONLY with the `sew-test-pins` feature, which release builds never enable
/// (`sign-root-helper.sh` refuses a helper carrying [`SEW_TEST_PINS_FLAG`];
/// `k2-root-helper/tests/binary.rs` proves a default build carries neither
/// these hashes nor the flag). Generated from each release's
/// `release.json` (Sew's contract; K2-PIN.txt is not authoritative) by
/// `scripts/sew-pins-from-release.sh <dir> --const <NAME>`, which also
/// checks SHA256SUMS and the tarballs; the output is byte-equal to these.
///
/// Two tables, like two K2 releases: `sew-test-pins` pins 0.1.0 (enable),
/// `sew-test-pins-upgrade` appends 0.1.1 as the pin (the upgrade source
/// 0.1.0 stays known, as a real pin bump keeps it).
#[cfg(feature = "sew-test-pins")]
const SEW_TEST_0_1_0: SewRelease = SewRelease {
    version: "0.1.0",
    artifacts: &[SewArtifact {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        tarball_sha256: "922f8f285a87bd22309eab916d000874004c02765a77d8abc7a74349a99b9006",
        binary_sha256: "4dfe51382507e748830a0e008fb96a19cecc7c6d2adb9063c0b2dc533f669837",
    }],
    media: Some(SewSidecar {
        version: "0.1.0",
        artifacts: &[SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "2ebb00641837b17a7253d7984656c62a4c102705a0002cc18a8645f827d596a6",
            binary_sha256: "5edf44e592c28ec3831b495434b98323133ab6438cd5ed74e43e84cea10883d6",
        }],
    }),
    transcriber: Some(SewSidecar {
        version: "0.1.0",
        artifacts: &[SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "62630e62965cf3aa46d923a934a2a1bde2920b4ecff7b4cdb29dcf25922aff29",
            binary_sha256: "e54b37f67dd701f4eed85a6a124c3f7be12ccc77a33a4fe2dd616804973fc919",
        }],
    }),
    recorder: Some(SewSidecar {
        version: "0.1.0",
        artifacts: &[SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "9370335de788e47e4be40aca4c9a8907bfc5112a708962146c594380b7260308",
            binary_sha256: "110ebcaa87bf55283b0197dfdd8b209d73ac32d4eb164de3193409a594a05be1",
        }],
    }),
};

#[cfg(feature = "sew-test-pins-upgrade")]
const SEW_TEST_0_1_1: SewRelease = SewRelease {
    version: "0.1.1",
    artifacts: &[SewArtifact {
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        tarball_sha256: "82fe9e196887b9cfa34528275e535df1b82f7f26309c97025767ee6821e189ba",
        binary_sha256: "35d334be98c5610d45f72c3d19e82eaca02ea3b17ecc37910532f26f9f3f0ade",
    }],
    media: Some(SewSidecar {
        version: "0.1.1",
        artifacts: &[SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "a7d01cae2b5da55b0dfd5541a61833a03a175ccdc408dbe78f3ea6d8f187c328",
            binary_sha256: "35958529b04ccf31a70bfbf147ee64f9803c44fb935c2523c76cc69030423355",
        }],
    }),
    transcriber: Some(SewSidecar {
        version: "0.1.1",
        artifacts: &[SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "95a4260fe6297e4fa95bb8f68b40d52145530151b91e160cb8ffe56a70f15660",
            binary_sha256: "13e1301969d726652c40f81a5aaac9b2cb45b9ef15aee003aebd3a7ca299460b",
        }],
    }),
    recorder: Some(SewSidecar {
        version: "0.1.1",
        artifacts: &[SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "231e744a1f824a918b898d9b5c2107a68a028fea6bd9ad2af63dbecfe7a4f3fa",
            binary_sha256: "dd113b1d5844098b5cc92d04849291b6a5a9c678f064d6fbb270da214f5a284a",
        }],
    }),
};

/// The test pin table this build uses (the last entry is the pin).
#[cfg(all(feature = "sew-test-pins", not(feature = "sew-test-pins-upgrade")))]
pub const SEW_TEST_PINS: &[SewRelease] = &[SEW_TEST_0_1_0];
#[cfg(feature = "sew-test-pins-upgrade")]
pub const SEW_TEST_PINS: &[SewRelease] = &[SEW_TEST_0_1_0, SEW_TEST_0_1_1];

/// Present only in a `sew-test-pins` build; release signing refuses it.
#[cfg(feature = "sew-test-pins")]
#[used]
pub static SEW_TEST_PINS_FLAG: &str = "K2_SEW_TEST_PINS_ENABLED";

/// `Some(flag)` in a `sew-test-pins` build (`helper version` reports it,
/// which also keeps the flag string in the binary for the signing check).
pub fn test_pins_flag() -> Option<&'static str> {
    #[cfg(feature = "sew-test-pins")]
    {
        Some(SEW_TEST_PINS_FLAG)
    }
    #[cfg(not(feature = "sew-test-pins"))]
    {
        None
    }
}

/// The table this build uses: the fixtures in a `test-signer` build, Sew's
/// test builds in a `sew-test-pins` build, else the real pins.
pub fn releases() -> &'static [SewRelease] {
    if cfg!(feature = "test-signer") {
        return TEST_RELEASES;
    }
    #[cfg(feature = "sew-test-pins")]
    {
        SEW_TEST_PINS
    }
    #[cfg(not(feature = "sew-test-pins"))]
    {
        SEW_RELEASES
    }
}

/// A Sew build K2 never pinned, recognised ONLY as an upgrade source
/// (0.46.5, item 12). It is not a release: it is never installed, staged,
/// kept, health-checked as a target or rolled back to, and it never makes
/// `known_binary` true (that stays "a release K2 pinned"). The helper's
/// `engine sew upgrade` accepts it as the installed binary and runs ITS
/// `sew upgrade --binary <pin>` like any older release (Sew confirmed its
/// schema migrates forward), and K2 then refuses a rollback onto it.
#[derive(Debug)]
pub struct SewUpgradeOnly {
    /// What the binary says (`sew version`); for the version compare only.
    pub version: &'static str,
    /// What K2 shows a person.
    pub label: &'static str,
    /// Sew's schema max at that build.
    pub schema: u32,
    pub arch: &'static str,
    /// sha256 of the `/usr/local/bin/sew` file itself.
    pub binary_sha256: &'static str,
}

/// sewlab's Sew 0.1.2 test build (`sew 0.1.2 (d73b0dd, schema 920002,
/// x86_64)`), never published. Sew confirmed 0.3.0 migrates schema 920002
/// forward (2026-10-27); sewlab has a backup.
pub const SEW_UPGRADE_ONLY: &[SewUpgradeOnly] = &[SewUpgradeOnly {
    version: "0.1.2",
    label: "0.1.2 (test build, schema 920002)",
    schema: 920002,
    arch: "x86_64",
    binary_sha256: "7ed8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385",
}];

/// The fixture for `test-signer` builds: [`TEST_UPGRADE_ONLY_STUB`]'s sha.
pub const TEST_UPGRADE_ONLY: &[SewUpgradeOnly] = &[SewUpgradeOnly {
    version: "0.0.0",
    label: "0.0.0 (test build, fixture)",
    schema: 1,
    arch: "x86_64",
    binary_sha256: "4c7831975275c7ac24f04cd92a5115001875c73a39d2938ef5e803a1a822c6a4",
}];

/// The bytes of [`TEST_UPGRADE_ONLY`]'s binary (helper tests write it to
/// `/usr/local/bin/sew`; never executed, exec is faked).
pub const TEST_UPGRADE_ONLY_STUB: &[u8] = b"#!/bin/sh\n# K2 fixture: an unpublished Sew test build (never executed)\n";

/// The upgrade-only table this build uses (fixtures under `test-signer`).
pub fn upgrade_only_builds() -> &'static [SewUpgradeOnly] {
    if cfg!(feature = "test-signer") {
        TEST_UPGRADE_ONLY
    } else {
        SEW_UPGRADE_ONLY
    }
}

/// The upgrade-only build `sha256_hex` is, on `arch`. Never a release
/// (a release's sha answers `None` here).
pub fn upgrade_only_binary(sha256_hex: &str, arch: &str) -> Option<&'static SewUpgradeOnly> {
    if !hex64(sha256_hex) || known_binary(sha256_hex, arch).is_some() {
        return None;
    }
    upgrade_only_builds().iter().find(|b| b.arch == arch && hex64(b.binary_sha256) && b.binary_sha256.eq_ignore_ascii_case(sha256_hex))
}

/// `engine sew upgrade accept-binary <sha>`'s token: exactly 64 lowercase
/// hex (the helper compares it to the file's own sha256, byte for byte).
pub fn accept_sha_ok(s: &str) -> bool {
    hex64(s) && !s.bytes().any(|b| b.is_ascii_uppercase())
}

/// The pin: the last release.
pub fn pinned() -> &'static SewRelease {
    releases().last().expect("the Sew pin table is never empty")
}

// ── App kit (0.46.6, Sew REL-050) ─────────────────────────────────────

/// Sew's App kit for one release (`sew-app-kit-<v>.tar.gz`: the npm-pack
/// tarballs of @sew/client, voice, channel, e2ee and bff, with SHA256SUMS,
/// NOTICES and kit.json), pinned by its sha256 like the release tarballs.
/// Not root's business: the daemon verifies it and keeps a read-only copy
/// for `k2 app sdk add --sew` (`k2_core::app_sdk`).
#[derive(Debug)]
pub struct SewAppKit {
    pub version: &'static str,
    pub sha256: &'static str,
}

/// Every pinned kit, by Sew version. Kits start with Sew 0.5.0: a release
/// before it has none. Each row comes from its release's SHA256SUMS and
/// release.json `app_kit` (`scripts/sew-pins-from-release.sh`), in the
/// same change as the release entry.
pub const SEW_APP_KITS: &[SewAppKit] =
    &[SewAppKit { version: "0.5.0", sha256: "64efdf1d822be2a4c49ccab437be8bddd5068c6e906b968b94beb44d8b1a0418" }];

/// The fixture kit for `test-signer` builds
/// (`testdata/sew/sew-app-kit-0.0.2.tar.gz`, made by
/// `testdata/sew/make-app-kit.py`; never Sew code), for [`TEST_RELEASES`]'
/// pin 0.0.2.
pub const TEST_APP_KITS: &[SewAppKit] =
    &[SewAppKit { version: "0.0.2", sha256: "ebbd6b86f6cad5deef69eaaecc298c76e9a7f448953ba485b9623bebb6734975" }];

/// The kit table this build uses.
pub fn app_kits() -> &'static [SewAppKit] {
    if cfg!(feature = "test-signer") {
        TEST_APP_KITS
    } else {
        SEW_APP_KITS
    }
}

/// The pinned App kit of Sew `version`, if that release has one.
pub fn app_kit(version: &str) -> Option<&'static SewAppKit> {
    app_kits().iter().find(|k| k.version == version && hex64(k.sha256))
}

/// front443: the first Sew release whose `sew media install` knows the
/// `passthrough` TURN tier (Sew TURN-PT, rev 168; 0.2.0 does not).
pub const PASSTHROUGH_MIN_VERSION: crate::version::Semver = crate::version::Semver(0, 2, 1);
/// The refusal code for a `passthrough` request to a Sew that predates it.
pub const SEW_PASSTHROUGH_UNSUPPORTED: &str = "sew_passthrough_unsupported";

/// Does Sew `version` (`x.y.z`) support the `passthrough` TURN tier?
/// Anything unparsable is "no".
pub fn version_supports_passthrough(version: &str) -> bool {
    crate::version::Semver::parse(version).is_some_and(|v| v >= PASSTHROUGH_MIN_VERSION)
}

/// The pinned `sew-media` (what voice installs) supports `passthrough`.
/// Follows the test pins in a `sew-test-pins` build (K-M14: a test helper
/// pinning a newer Sew than the release pin gets the tier).
pub fn pinned_media_supports_passthrough() -> bool {
    pinned().media.as_ref().is_some_and(|m| version_supports_passthrough(m.version))
}

/// The one-line refusal for `version`.
pub fn passthrough_unsupported_message(version: &str) -> String {
    format!(
        "{SEW_PASSTHROUGH_UNSUPPORTED}: Sew {version} has no passthrough TURN tier (needs Sew {PASSTHROUGH_MIN_VERSION} or later); \
         sew-media was left as it is (TURN stays on TCP 5349)"
    )
}

/// The refusal code and words every Sew install path uses while this K2
/// release pins no published Sew release ([`sew_release_pinned`] false):
/// the daemon's `/cli/sew/*` routes, `k2 hostchat` and the root helper.
pub const SEW_NOT_RELEASED: &str = "sew_not_released";
pub const SEW_NOT_RELEASED_MESSAGE: &str =
    "Sew isn't available in this K2 release yet. It will be turned on in a later K2 update.";

/// True when `rel` pins at least one real artifact (not every entry is a
/// placeholder).
pub fn release_is_pinned(rel: &SewRelease) -> bool {
    rel.artifacts.iter().any(|a| !checksum_is_placeholder(a))
}

/// Whether this build pins a real Sew release. False on a release build
/// while [`SEW_RELEASES`]' last entry is a placeholder (Sew SW1 not
/// published); true with `sew-test-pins` (Sew's test builds) and in
/// `test-signer` builds (K2's fixtures). While false, enable, preflight,
/// upgrade and voice install refuse with [`SEW_NOT_RELEASED`] before any
/// effect.
pub fn sew_release_pinned() -> bool {
    release_is_pinned(pinned())
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
        return Err("refusing to install: this build has no Sew pin (placeholder checksum)".into());
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

/// The release `version` of this build's table (old or current).
pub fn release(version: &str) -> Option<&'static SewRelease> {
    releases().iter().find(|r| r.version == version)
}

/// The `sew-media` (version, artifact) that Sew release `core_version`
/// pins for `arch`: what a rollback to that core reinstalls.
pub fn media_artifact_of(core_version: &str, arch: &str) -> Result<(&'static str, &'static SewArtifact), String> {
    let rel = release(core_version).ok_or_else(|| format!("Sew {core_version} is not a release this K2 knows"))?;
    let m = rel.media.as_ref().ok_or_else(|| format!("Sew {core_version} pins no sew-media (voice) release"))?;
    let a = m
        .artifacts
        .iter()
        .find(|a| a.arch == arch && !checksum_is_placeholder(a))
        .ok_or_else(|| format!("Sew {core_version} pins no sew-media for '{arch}'"))?;
    Ok((m.version, a))
}

/// The sew-media version whose release tarball has this sha256 on `arch`
/// (any release of this build's table).
pub fn known_media_tarball(sha256_hex: &str, arch: &str) -> Option<&'static str> {
    releases().iter().find_map(|r| {
        let m = r.media.as_ref()?;
        m.artifacts
            .iter()
            .find(|a| a.arch == arch && !checksum_is_placeholder(a) && a.tarball_sha256.eq_ignore_ascii_case(sha256_hex))
            .map(|_| m.version)
    })
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

/// The identity provider `sew install` sets. Fixed for the life of an
/// install: switching means a fresh install (uninstall --purge, enable).
pub const PROVIDER_K2: &str = "k2";
/// Sew's own password logins (local users with setup links): the 0.46.0
/// demo path, before K2's chat pass (S28) ships.
pub const PROVIDER_STANDALONE: &str = "standalone";

/// `engine sew install` stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallRequest {
    /// `k2` (absent = `k2`) or `standalone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// [`jwks_url_ok`]. Required for `k2`, refused for `standalone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_url: Option<String>,
    /// [`master_key_text_ok`]. Piped to `sew install --master-key-from -`;
    /// never written to disk by K2.
    pub master_key: String,
}

impl InstallRequest {
    /// The provider this request installs (`k2` when absent).
    pub fn provider(&self) -> &str {
        self.provider.as_deref().unwrap_or(PROVIDER_K2)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self.provider() {
            PROVIDER_K2 => {
                if !self.jwks_url.as_deref().is_some_and(jwks_url_ok) {
                    return Err("install refused: jwksUrl");
                }
            }
            PROVIDER_STANDALONE => {
                if self.jwks_url.is_some() {
                    return Err("install refused: jwksUrl with provider standalone");
                }
            }
            _ => return Err("install refused: provider"),
        }
        if !master_key_text_ok(&self.master_key) {
            return Err("install refused: masterKey");
        }
        Ok(())
    }

    /// The `sew install --config-json` body (Sew SW5 `InstallInputs`).
    /// Provider `k2` (with K2's JWKS) or `standalone`, profile shared (the
    /// K2 memory split), never a cred mode (Sew picks encrypted credentials
    /// on systemd ≥ 250) and never `mgmt_users` (no `sew-mgmt` group for
    /// the daemon). `daemon_uid` is the caller check's uid, never an input
    /// field: Sew writes it to `[mgmt] allow_uids` itself (SW5
    /// `mgmt_allow_uids`, SW7), so the daemon reaches the mgmt socket
    /// without a supplementary group and K2 never edits `sew.toml`.
    pub fn sew_inputs(&self, daemon_uid: u32) -> serde_json::Value {
        install_inputs(self.provider(), self.jwks_url.as_deref(), daemon_uid)
    }
}

/// Sew SW5 `InstallInputs` (shared by `install` and `install reconfigure`).
fn install_inputs(provider: &str, jwks_url: Option<&str>, daemon_uid: u32) -> serde_json::Value {
    let mut v = serde_json::json!({
        "provider": provider,
        "profile": "shared",
        "pg_major": SEW_PG_MAJOR,
        "mgmt_allow_uids": [daemon_uid],
    });
    if let Some(u) = jwks_url {
        v["k2_jwks_url"] = serde_json::json!(u);
    }
    v
}

/// `engine sew install reconfigure` stdin (protocol 11, 0.46.6): what
/// `install` takes minus the master key (the install's keys stay). The
/// helper adds `pg_major` and the daemon's uid, exactly as for `install`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReconfigureRequest {
    /// `k2` (absent = `k2`) or `standalone`: the install's own provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// [`jwks_url_ok`]. Required for `k2`, refused for `standalone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwks_url: Option<String>,
}

impl ReconfigureRequest {
    pub fn provider(&self) -> &str {
        self.provider.as_deref().unwrap_or(PROVIDER_K2)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self.provider() {
            PROVIDER_K2 => {
                if !self.jwks_url.as_deref().is_some_and(jwks_url_ok) {
                    return Err("reconfigure refused: jwksUrl");
                }
            }
            PROVIDER_STANDALONE => {
                if self.jwks_url.is_some() {
                    return Err("reconfigure refused: jwksUrl with provider standalone");
                }
            }
            _ => return Err("reconfigure refused: provider"),
        }
        Ok(())
    }

    pub fn sew_inputs(&self, daemon_uid: u32) -> serde_json::Value {
        install_inputs(self.provider(), self.jwks_url.as_deref(), daemon_uid)
    }
}

/// The Sew release whose installs `install reconfigure` brings forward:
/// K2's upgrade flow runs it after an upgrade FROM a version below this
/// (Sew 2026-10-27: after an upgrade from 0.3.0).
pub const SEW_RECONFIGURE_BELOW: &str = "0.3.1";

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
        if self.jwks_url.is_none()
            && self.public_url.is_none()
            && self.mirror_kind.is_none()
            && self.mirror_path.is_none()
        {
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

/// PEM blocks of the allowed labels only (printable ASCII, base64 bodies),
/// at most `max` bytes. `Some(n)` = n complete blocks.
fn pem_blocks(text: &str, labels: &[&str], max: usize) -> Option<usize> {
    if text.is_empty() || text.len() > max || !text.bytes().all(|b| b == b'\n' || b == b'\r' || (0x20..0x7f).contains(&b)) {
        return None;
    }
    let mut n = 0;
    let mut open: Option<&str> = None;
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() {
            continue;
        }
        match open {
            None => {
                let label = line.strip_prefix("-----BEGIN ")?.strip_suffix("-----")?;
                open = Some(labels.iter().copied().find(|l| *l == label)?);
            }
            Some(l) => {
                if let Some(end) = line.strip_prefix("-----END ").and_then(|x| x.strip_suffix("-----")) {
                    if end != l {
                        return None;
                    }
                    open = None;
                    n += 1;
                } else if !line.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=') {
                    return None;
                }
            }
        }
    }
    (open.is_none() && n > 0).then_some(n)
}

/// A TURN TLS certificate chain: 1–4 `CERTIFICATE` blocks, nothing else.
pub fn turn_chain_pem_ok(text: &str) -> bool {
    pem_blocks(text, &["CERTIFICATE"], MAX_TURN_CHAIN).is_some_and(|n| n <= 4)
}

/// A TURN TLS private key: exactly one `PRIVATE KEY` (PKCS#8) or
/// `EC PRIVATE KEY` block.
pub fn turn_key_pem_ok(text: &str) -> bool {
    pem_blocks(text, &["PRIVATE KEY", "EC PRIVATE KEY"], MAX_TURN_KEY) == Some(1)
}

/// `voice.public_url`: `wss://<host>` exactly. On K2 the nested
/// `voice.<box>` label, or an App's own origin that proxies Sew's `/rtc`
/// same-origin (Hearth's BFF for the 0.46.0 demo).
pub fn voice_url_ok(s: &str) -> bool {
    scheme_host_ok(s, "wss")
}

/// `engine sew media install` / `media reconfigure` stdin: the subset of
/// Sew's `MediaInstallInputs` K2 may set. The tarball path is the helper's
/// fixed staging file; the TURN cert and key arrive as PEM TEXT
/// (`turn_cert_pem`, `turn_key_pem`) that the helper writes to its own
/// fixed staging files ([`SEW_STAGE_TURN_CERT`], [`SEW_STAGE_TURN_KEY`]),
/// never as a path; the cred mode is never set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct MediaRequest {
    /// [`voice_url_ok`]. Required to install; on reconfigure, absent keeps
    /// Sew's `[voice] public_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// `stun`, or 1–4 comma-separated literal IPs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_port: Option<u16>,
    /// `off`, `plain`, `external`, `direct` or `passthrough` (Sew TURN-PT,
    /// rev 168: rustls on 127.0.0.1:5349 behind K2's :443 front, PROXY v2
    /// first; cert and key staged as for `direct`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_tls: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_advertise_port: Option<u16>,
    /// `apply` (write Sew's sysctl drop-in) or `skip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysctl: Option<String>,
    /// [`turn_chain_pem_ok`]; with `turn_key_pem`, both or neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_cert_pem: Option<String>,
    /// [`turn_key_pem_ok`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_key_pem: Option<String>,
}

impl MediaRequest {
    /// `media install`: a public URL is required.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.public_url.is_none() {
            return Err("media refused: public_url");
        }
        self.validate_fields()
    }

    /// `media reconfigure`: at least one key.
    pub fn validate_reconfigure(&self) -> Result<(), &'static str> {
        if *self == MediaRequest::default() {
            return Err("media refused: no keys");
        }
        self.validate_fields()
    }

    fn validate_fields(&self) -> Result<(), &'static str> {
        if self.public_url.as_deref().is_some_and(|u| !voice_url_ok(u)) {
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
        if self.turn_tls.as_deref().is_some_and(|t| !matches!(t, "off" | "plain" | "external" | "direct" | "passthrough")) {
            return Err("media refused: turn_tls");
        }
        if self.turn_domain.as_deref().is_some_and(|d| !hostname_ok(d)) {
            return Err("media refused: turn_domain");
        }
        if self.sysctl.as_deref().is_some_and(|s| s != "apply" && s != "skip") {
            return Err("media refused: sysctl");
        }
        match (&self.turn_cert_pem, &self.turn_key_pem) {
            (None, None) => {}
            (Some(c), Some(k)) => {
                if !turn_chain_pem_ok(c) {
                    return Err("media refused: turn_cert_pem");
                }
                if !turn_key_pem_ok(k) {
                    return Err("media refused: turn_key_pem");
                }
            }
            _ => return Err("media refused: turn_cert_pem and turn_key_pem go together"),
        }
        Ok(())
    }

    /// True when this request carries TURN TLS material.
    pub fn has_turn_tls(&self) -> bool {
        self.turn_cert_pem.is_some()
    }

    /// The `sew media install --config-json` body: this request without
    /// the PEM text, plus the helper's staged tarball path (install only),
    /// its staged TURN cert/key paths (when PEM text came) and the shared
    /// profile.
    pub fn sew_inputs(&self, tarball: Option<&str>, turn: Option<(&str, &str)>) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("plain struct");
        if let Some(m) = v.as_object_mut() {
            m.remove("turn_cert_pem");
            m.remove("turn_key_pem");
        }
        if let Some(t) = tarball {
            v["tarball"] = serde_json::json!(t);
        }
        if let Some((c, k)) = turn {
            v["turn_cert"] = serde_json::json!(c);
            v["turn_key"] = serde_json::json!(k);
        }
        v["profile"] = serde_json::json!("shared");
        v
    }
}

/// `engine sew media turn-cert` stdin: a renewed TURN chain and its key
/// (K2 reuses the key on renewal, so Sew restarts nothing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct TurnCertRequest {
    pub cert_pem: String,
    pub key_pem: String,
}

impl TurnCertRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !turn_chain_pem_ok(&self.cert_pem) {
            return Err("turn-cert refused: cert_pem");
        }
        if !turn_key_pem_ok(&self.key_pem) {
            return Err("turn-cert refused: key_pem");
        }
        Ok(())
    }
}

/// An outside-probe result token (`engine sew media probe-record`).
pub fn probe_result_ok(s: &str) -> bool {
    s == "ok" || s == "fail"
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

    /// The fixture kit's pin is the committed file; real kits are 64 hex,
    /// one per version, and only from Sew 0.5.0 (the first with a kit).
    #[test]
    fn app_kit_pins_hold() {
        let fixture = include_bytes!("../testdata/sew/sew-app-kit-0.0.2.tar.gz");
        assert_eq!(sha256_hex(fixture), TEST_APP_KITS[0].sha256, "re-run testdata/sew/make-app-kit.py and update TEST_APP_KITS");
        assert_eq!(TEST_APP_KITS[0].version, TEST_RELEASES.last().unwrap().version, "the fixture kit is the fixture pin's");
        let mut seen = std::collections::BTreeSet::new();
        for k in SEW_APP_KITS {
            assert!(hex64(k.sha256), "{}: {}", k.version, k.sha256);
            assert!(seen.insert(k.version), "{} twice", k.version);
            assert!(version_cmp(k.version, "0.5.0") != Some(std::cmp::Ordering::Less), "{}: kits start at 0.5.0", k.version);
            assert!(SEW_RELEASES.iter().any(|r| r.version == k.version), "{}: a kit for a release K2 never pinned", k.version);
        }
        if cfg!(feature = "test-signer") {
            assert!(app_kit("0.0.2").is_some());
        } else {
            assert!(app_kit("0.3.1").is_none(), "0.3.1 has no App kit");
            assert_eq!(app_kit("0.5.0").map(|k| k.sha256), Some("64efdf1d822be2a4c49ccab437be8bddd5068c6e906b968b94beb44d8b1a0418"));
            assert!(app_kit(pinned().version).is_some(), "the pin has its kit");
        }
    }

    /// Every artifact of a release, sidecars included.
    fn all_artifacts(r: &SewRelease) -> Vec<&SewArtifact> {
        let side = [&r.media, &r.transcriber, &r.recorder];
        r.artifacts.iter().chain(side.into_iter().flatten().flat_map(|s| s.artifacts)).collect()
    }

    /// The 0.3.1 pin is exactly the published SHA256SUMS (vendored from
    /// the v0.3.1 release, sha256 c7e82dc1…2bba): every core and sidecar
    /// tarball and binary member line, both ways — nothing pinned that Sew
    /// didn't publish, nothing published that the pin leaves out.
    #[test]
    fn the_0_3_1_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.3.1.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "c7e82dc13616c5f6b84e2c307a67ce441d8b36bdd6d3d4cdf66249455df92bba",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.3.1", sums), 16, "4 binaries × 2 arches × (tarball, member)");
        assert_eq!(SEW_RECONFIGURE_BELOW, "0.3.1", "0.3.0 → 0.3.1 reconfigures");
    }

    /// The 0.5.0 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.5.0 release, sha256 1fc29f14…e276): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_5_0_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.5.0.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "1fc29f14cef129a264678988bdfeb9e0194eb5fe9fdba4e3fe588442484be276",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(SEW_RELEASES.last().unwrap().version, "0.5.0", "0.5.0 is the pin");
        assert_eq!(pin_equals_sums("0.5.0", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.5.0").map(|k| k.sha256),
            Some("64efdf1d822be2a4c49ccab437be8bddd5068c6e906b968b94beb44d8b1a0418")
        );
    }

    /// A pinned release's tarball and binary-member lines (and its App
    /// kit's line, when it has one) are exactly the published SHA256SUMS,
    /// both ways: nothing pinned that Sew didn't publish, nothing published
    /// that the pin leaves out. Returns how many lines matched.
    fn pin_equals_sums(version: &str, sums: &str) -> usize {
        let r = SEW_RELEASES.iter().find(|r| r.version == version).unwrap_or_else(|| panic!("{version} is pinned"));
        let mut want = std::collections::BTreeSet::new();
        let mut put = |bin: &str, version: &str, a: &SewArtifact| {
            let base = format!("{bin}-{version}-{}", a.triple);
            want.insert(format!("{}  {base}.tar.gz", a.tarball_sha256));
            want.insert(format!("{}  {base}/{bin}", a.binary_sha256));
        };
        for a in r.artifacts {
            put("sew", r.version, a);
        }
        for (bin, side) in [("sew-media", &r.media), ("sew-transcriber", &r.transcriber), ("sew-recorder", &r.recorder)] {
            let side = side.as_ref().unwrap_or_else(|| panic!("{version} pins every sidecar"));
            for a in side.artifacts {
                put(bin, side.version, a);
            }
        }
        if let Some(k) = SEW_APP_KITS.iter().find(|k| k.version == version) {
            want.insert(format!("{}  sew-app-kit-{version}.tar.gz", k.sha256));
        }
        let published: std::collections::BTreeSet<String> = sums
            .lines()
            .filter(|l| l.contains(".tar.gz") || l.contains("/sew"))
            .map(str::to_string)
            .collect();
        assert_eq!(want, published, "{version}: pinned lines vs published lines");
        want.len()
    }

    /// item 12: sewlab's 0.1.2 test build is an upgrade source only. It is
    /// in no release table (so never installed, staged, kept or a rollback
    /// target), `known_binary` stays false for it, and it is older than the
    /// pin. The fixture entry is the stub's real sha.
    #[test]
    fn the_sewlab_test_build_is_upgrade_only() {
        let b = &SEW_UPGRADE_ONLY[0];
        assert_eq!(b.binary_sha256, "7ed8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385");
        assert_eq!((b.version, b.label, b.schema, b.arch), ("0.1.2", "0.1.2 (test build, schema 920002)", 920002, "x86_64"));
        assert_eq!(version_cmp(b.version, SEW_RELEASES.last().unwrap().version), Some(std::cmp::Ordering::Less));
        for r in SEW_RELEASES.iter().chain(TEST_RELEASES) {
            assert_ne!(r.version, b.version, "0.1.2 is not a release");
            for a in all_artifacts(r) {
                assert!(!a.binary_sha256.eq_ignore_ascii_case(b.binary_sha256) && !a.tarball_sha256.eq_ignore_ascii_case(b.binary_sha256));
            }
        }
        if !cfg!(feature = "test-signer") && !cfg!(feature = "sew-test-pins") {
            assert_eq!(known_binary(b.binary_sha256, "x86_64"), None, "never 'known' (a release)");
            assert_eq!(upgrade_only_binary(b.binary_sha256, "x86_64").map(|x| x.version), Some("0.1.2"));
            assert_eq!(upgrade_only_binary(&b.binary_sha256.to_uppercase(), "x86_64").map(|x| x.version), Some("0.1.2"));
            assert!(upgrade_only_binary(b.binary_sha256, "aarch64").is_none(), "x86_64 only");
            let pin = pinned_artifact("x86_64").unwrap();
            assert!(upgrade_only_binary(pin.binary_sha256, "x86_64").is_none(), "a release is never upgrade-only");
        }
        let f = &TEST_UPGRADE_ONLY[0];
        assert_eq!(f.binary_sha256, sha256_hex(TEST_UPGRADE_ONLY_STUB));
        assert_eq!(version_cmp(f.version, TEST_RELEASES[0].version), Some(std::cmp::Ordering::Less));
        assert!(upgrade_only_binary("not-hex", "x86_64").is_none());
    }

    #[test]
    fn accept_sha_is_exactly_64_lowercase_hex() {
        assert!(accept_sha_ok("7ed8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385"));
        for bad in [
            "",
            "7ED8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385",
            "7ed8031efba4497131f09609b33a386401e910f992672499bd0d410c6395238",
            "7ed8031efba4497131f09609b33a386401e910f992672499bd0d410c63952385a",
            "g ed8031efba4497131f09609b33a386401e910f992672499bd0d410c6395238",
        ] {
            assert!(!accept_sha_ok(bad), "{bad:?}");
        }
    }

    /// The real table: the old 0.1.0 placeholder stays and refuses, 0.2.0
    /// and 0.2.1 stay known; the pin is Sew 0.3.0 for both linux-gnu
    /// triples, with all three sidecars, exactly as
    /// `scripts/sew-pins-from-release.sh` printed it from the verified
    /// release (2026-10-19).
    #[test]
    fn real_table_pins_sew_0_5_0_and_keeps_the_old_entries() {
        assert_eq!(SEW_RELEASES.len(), 6);
        assert_eq!(SEW_RELEASES[4].version, "0.3.1");
        assert!(release_is_pinned(&SEW_RELEASES[4]), "0.3.1 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[3].version, "0.3.0");
        assert!(release_is_pinned(&SEW_RELEASES[3]), "0.3.0 stays a real release (the reconfigure source)");
        assert_eq!(SEW_RELEASES[1].version, "0.2.0");
        assert!(release_is_pinned(&SEW_RELEASES[1]), "0.2.0 stays a real release");
        assert_eq!(SEW_RELEASES[2].version, "0.2.1");
        assert!(release_is_pinned(&SEW_RELEASES[2]), "0.2.1 stays a real release");
        let old = &SEW_RELEASES[0];
        assert_eq!(old.version, "0.1.0");
        assert!(old.artifacts.iter().all(checksum_is_placeholder));
        assert!(!release_is_pinned(old));
        assert!(artifact_in(old, "x86_64").unwrap_err().contains("no Sew pin"));

        let pin = SEW_RELEASES.last().unwrap();
        assert_eq!(pin.version, "0.5.0");
        assert!(release_is_pinned(pin));
        assert!(artifact_in(pin, "riscv64").unwrap_err().contains("unsupported"));
        let want = [
            ("x86_64", "5dd2c9d2437045315e3b95709bfe34bbaebcee22a6389ac8046431c30a533461", "5cbda1474dff81332b4b1222c058b71c8826516105a7428a5c23e356f8575c1f"),
            ("aarch64", "77e0995cd6865e2dce10da95c6e1b767a9f926bbf2c7e625623375ca6291c1ef", "cfeb9e2b504f752f2b37ca925010cd2fc854df9d67321bb7f15adc6a6a834708"),
        ];
        for (arch, tar, bin) in want {
            let a = artifact_in(pin, arch).expect("pinned");
            assert_eq!((a.tarball_sha256, a.binary_sha256), (tar, bin), "{arch}");
            assert_eq!(a.triple, format!("{arch}-unknown-linux-gnu"));
        }
        for (name, s) in [("media", &pin.media), ("transcriber", &pin.transcriber), ("recorder", &pin.recorder)] {
            let s = s.as_ref().unwrap_or_else(|| panic!("{name} pinned"));
            assert_eq!(s.version, "0.5.0", "{name}");
            let mut arches: Vec<&str> = s.artifacts.iter().map(|a| a.arch).collect();
            arches.sort();
            assert_eq!(arches, ["aarch64", "x86_64"], "{name}");
        }
        for a in all_artifacts(pin) {
            assert!(!checksum_is_placeholder(a), "{a:?}");
        }
        // Every hash is distinct (no copy-paste slip between rows), across
        // all five real releases.
        let mut seen = std::collections::HashSet::new();
        for r in &SEW_RELEASES[1..] {
            for a in all_artifacts(r) {
                assert!(seen.insert(a.tarball_sha256) && seen.insert(a.binary_sha256), "{a:?}");
            }
        }
        assert_eq!(seen.len(), 80);
    }

    #[test]
    fn release_pinned_follows_the_last_entry() {
        assert!(release_is_pinned(SEW_RELEASES.last().unwrap()), "0.5.0 is pinned (published 2026-10-28)");
        assert!(release_is_pinned(TEST_RELEASES.last().unwrap()));
        assert!(sew_release_pinned(), "every build pins a release now");
        assert_eq!(SEW_NOT_RELEASED, "sew_not_released");
    }

    #[cfg(all(feature = "sew-test-pins", not(feature = "test-signer")))]
    #[test]
    fn sew_test_pins_are_a_pinned_release() {
        assert!(release_is_pinned(SEW_TEST_PINS.last().unwrap()));
        assert!(sew_release_pinned());
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
        } else if !cfg!(feature = "sew-test-pins") {
            assert_eq!(pinned().version, "0.5.0");
            for a in pinned().artifacts {
                assert_eq!(pinned_artifact(a.arch).expect("pinned").binary_sha256, a.binary_sha256);
                assert_eq!(known_binary(a.binary_sha256, a.arch), Some("0.5.0"));
                // Another arch's binary is not this arch's release.
                let other = if a.arch == "x86_64" { "aarch64" } else { "x86_64" };
                assert_eq!(known_binary(a.binary_sha256, other), None);
                assert_eq!(known_binary(a.tarball_sha256, a.arch), None, "a tarball sha is never a binary sha");
            }
            assert_eq!(pinned_media_artifact("x86_64").expect("media pinned").0, "0.5.0");
            // 0.2.0, 0.2.1, 0.3.0 and 0.3.1 on a box are still recognised (upgrade sources).
            for (r, v) in [(&SEW_RELEASES[1], "0.2.0"), (&SEW_RELEASES[2], "0.2.1"), (&SEW_RELEASES[3], "0.3.0"), (&SEW_RELEASES[4], "0.3.1")] {
                assert_eq!(r.version, v);
                for a in r.artifacts {
                    assert_eq!(known_binary(a.binary_sha256, a.arch), Some(v));
                }
            }
            assert_eq!(known_binary(&"0".repeat(64), "x86_64"), None);
        }
    }

    /// A default build never selects Sew's test pins (`sew-test-pins` is a
    /// test-only feature; `k2-root-helper/tests/binary.rs` checks the
    /// built binary's bytes).
    #[cfg(not(feature = "sew-test-pins"))]
    #[test]
    fn default_build_has_no_sew_test_pins() {
        assert!(test_pins_flag().is_none());
        // Compare contents: a `const` slice has no stable address.
        let want = if cfg!(feature = "test-signer") { TEST_RELEASES } else { SEW_RELEASES };
        let key = |t: &[SewRelease]| -> Vec<(&str, Vec<&str>)> {
            t.iter().map(|r| (r.version, r.artifacts.iter().map(|a| a.tarball_sha256).collect())).collect()
        };
        assert_eq!(key(releases()), key(want));
        for r in releases() {
            for a in r.artifacts {
                assert_ne!(a.binary_sha256, "4dfe51382507e748830a0e008fb96a19cecc7c6d2adb9063c0b2dc533f669837");
                assert_ne!(a.binary_sha256, "35d334be98c5610d45f72c3d19e82eaca02ea3b17ecc37910532f26f9f3f0ade");
            }
        }
    }

    #[cfg(all(feature = "sew-test-pins", not(feature = "test-signer")))]
    #[test]
    fn sew_test_pins_are_aarch64_only_and_the_upgrade_table_keeps_0_1_0() {
        assert_eq!(test_pins_flag(), Some("K2_SEW_TEST_PINS_ENABLED"));
        let (pin, tar) = if cfg!(feature = "sew-test-pins-upgrade") {
            ("0.1.1", "82fe9e196887b9cfa34528275e535df1b82f7f26309c97025767ee6821e189ba")
        } else {
            ("0.1.0", "922f8f285a87bd22309eab916d000874004c02765a77d8abc7a74349a99b9006")
        };
        assert_eq!(pinned().version, pin);
        assert_eq!(pinned_artifact("aarch64").unwrap().tarball_sha256, tar);
        assert!(pinned_artifact("x86_64").unwrap_err().contains("unsupported"));
        assert_eq!(known_binary("4dfe51382507e748830a0e008fb96a19cecc7c6d2adb9063c0b2dc533f669837", "aarch64"), Some("0.1.0"));
        let v011 = known_binary("35d334be98c5610d45f72c3d19e82eaca02ea3b17ecc37910532f26f9f3f0ade", "aarch64");
        assert_eq!(v011, cfg!(feature = "sew-test-pins-upgrade").then_some("0.1.1"));
        assert_eq!(pinned_media_artifact("aarch64").unwrap().0, pin);
        for r in SEW_TEST_PINS {
            for a in all_artifacts(r) {
                assert!(!checksum_is_placeholder(a), "{a:?}");
            }
        }
        // The real pin (0.5.0) is never selected in a test-pins build.
        assert_ne!(pinned().version, "0.5.0");
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
            (
                "sew-media-0.0.1-x86_64-unknown-linux-gnu.tar.gz",
                TEST_RELEASES[0].media.as_ref().unwrap().artifacts[0].tarball_sha256,
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
        let inputs = r.sew_inputs(1001);
        assert_eq!(inputs["provider"], "k2");
        assert_eq!(inputs["k2_jwks_url"], "http://127.0.0.1:38471/v1/chat/jwks");
        // The PG major is always explicit (never Sew's "highest found").
        assert_eq!(inputs["pg_major"], serde_json::json!(16));
        assert_eq!(SEW_PG_MAJOR, 16, "Sew 0.2.x / 0.3.0 run sew-db on PostgreSQL 16");
        // Standalone (0.46.0 demo): no JWKS, Sew's own password logins.
        let sa: InstallRequest =
            serde_json::from_str(r#"{"provider":"standalone","masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#).unwrap();
        sa.validate().unwrap();
        let si = sa.sew_inputs(1001);
        assert_eq!(si["provider"], "standalone");
        assert!(si.get("k2_jwks_url").is_none());
        assert_eq!(si["pg_major"], serde_json::json!(SEW_PG_MAJOR));
        assert_eq!(si["mgmt_allow_uids"], serde_json::json!([1001]));
        for bad in [
            r#"{"provider":"standalone","jwksUrl":"http://127.0.0.1:1/v1/chat/jwks","masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#,
            r#"{"provider":"k2","masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#,
            r#"{"masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#,
            r#"{"provider":"ldap","masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#,
            r#"{"provider":"standalone","masterKey":"short"}"#,
        ] {
            let r: InstallRequest = serde_json::from_str(bad).unwrap();
            assert!(r.validate().is_err(), "{bad}");
        }
        assert_eq!(inputs["mgmt_allow_uids"], serde_json::json!([1001]), "Sew writes [mgmt] allow_uids (SW5)");
        assert!(inputs.get("mgmt_users").is_none(), "no sew-mgmt group (SW7)");
        assert!(inputs.get("cred_mode").is_none(), "never a plain cred mode");
        // A uid on stdin never parses: only the caller check names it.
        assert!(serde_json::from_str::<InstallRequest>(
            r#"{"jwksUrl":"http://127.0.0.1:1/v1/chat/jwks","masterKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","mgmt_allow_uids":[0]}"#
        )
        .is_err());
        // Unknown keys (a path, a provider, a cred mode) never parse.
        for bad in [
            r#"{"jwksUrl":"http://127.0.0.1:1/v1/chat/jwks","masterKey":"x","mgmt_users":["k2"]}"#,
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
        assert_eq!(m.sew_inputs(Some("/x"), None)["tarball"], "/x");
        assert!(m.sew_inputs(None, None).get("tarball").is_none());
        // Reconfigure: no URL needed, but at least one key.
        assert!(MediaRequest::default().validate_reconfigure().is_err());
        let r: MediaRequest = serde_json::from_str(r#"{"public_url":"wss://hearth.box.k2.dev"}"#).unwrap();
        r.validate_reconfigure().unwrap();
        let r: MediaRequest = serde_json::from_str(r#"{"node_ip":"203.0.113.7"}"#).unwrap();
        r.validate_reconfigure().unwrap();
        assert!(r.validate().is_err(), "install needs public_url");
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
            r#"{"public_url":"wss://voice.box.k2.dev/rtc"}"#,
            r#"{"public_url":"wss://voice.box.k2.dev:443"}"#,
            r#"{"public_url":"ws://voice.box.k2.dev"}"#,
        ] {
            let m: MediaRequest = serde_json::from_str(bad).unwrap();
            assert!(m.validate().is_err(), "{bad}");
        }
        // Only Sew's settable keys parse (Sew `SETTABLE_KEYS`): setup links
        // follow server.public_url (`<public_url>/setup`), and
        // voice.public_url goes through `sew media install`, never
        // `config set`.
        let c: ConfigRequest = serde_json::from_str(r#"{"server.public_url":"https://hearth.box.k2.dev"}"#).unwrap();
        c.validate().unwrap();
        assert_eq!(c.pairs(), vec![("server.public_url", "https://hearth.box.k2.dev")]);
        for bad in [
            r#"{"identity.standalone.setup_url":"https://hearth.box.k2.dev/setup"}"#,
            r#"{"voice.public_url":"wss://hearth.box.k2.dev"}"#,
        ] {
            assert!(serde_json::from_str::<ConfigRequest>(bad).is_err(), "{bad}");
        }
    }

    fn pem(label: &str, body: &str) -> String {
        format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
    }

    /// TURN TLS material rides stdin as PEM text, never as a path: the
    /// helper writes it to its own fixed staging files.
    #[test]
    fn turn_tls_pem_is_text_and_strict() {
        let cert = pem("CERTIFICATE", "MIIBszCCAVmgAwIBAgIU");
        let chain = format!("{cert}{}", pem("CERTIFICATE", "MIIBAAAA+/="));
        let key = pem("PRIVATE KEY", "MIGHAgEAMBMGByqGSM49");
        assert!(turn_chain_pem_ok(&cert));
        assert!(turn_chain_pem_ok(&chain));
        assert!(turn_key_pem_ok(&key));
        assert!(turn_key_pem_ok(&pem("EC PRIVATE KEY", "MHcCAQEEI")));
        for bad in [
            String::new(),
            "/etc/shadow".to_string(),
            key.clone(),
            format!("{cert}{key}"),
            pem("CERTIFICATE", "not base64!"),
            "-----BEGIN CERTIFICATE-----\nMIIB\n".to_string(),
            pem("CERTIFICATE", &"A".repeat(MAX_TURN_CHAIN)),
            format!("{cert}{cert}{cert}{cert}{cert}"),
        ] {
            assert!(!turn_chain_pem_ok(&bad), "{bad:?}");
        }
        for bad in [cert.clone(), format!("{key}{key}"), pem("RSA PUBLIC KEY", "AAAA"), format!("{key}\u{1b}[0m")] {
            assert!(!turn_key_pem_ok(&bad), "{bad:?}");
        }
        let m = MediaRequest {
            public_url: Some("wss://voice.box.k2.dev".into()),
            turn_tls: Some("direct".into()),
            turn_domain: Some("turn-box.k2.dev".into()),
            turn_cert_pem: Some(chain.clone()),
            turn_key_pem: Some(key.clone()),
            ..Default::default()
        };
        m.validate().unwrap();
        assert!(m.has_turn_tls());
        let inputs = m.sew_inputs(Some("/s/m.tar.gz"), Some(("/s/c.pem", "/s/k.pem")));
        assert!(inputs.get("turn_cert_pem").is_none() && inputs.get("turn_key_pem").is_none(), "PEM text never reaches Sew's JSON");
        assert_eq!(inputs["turn_cert"], "/s/c.pem");
        assert_eq!(inputs["turn_key"], "/s/k.pem");
        assert_eq!(inputs["profile"], "shared");
        let pt = MediaRequest { turn_tls: Some("passthrough".into()), ..m.clone() };
        pt.validate().unwrap();
        pt.validate_reconfigure().unwrap();
        assert_eq!(pt.sew_inputs(None, Some(("/s/c.pem", "/s/k.pem")))["turn_tls"], "passthrough");
        for bad in ["PASSTHROUGH", "pass-through", "sni", ""] {
            let r = MediaRequest { turn_tls: Some(bad.into()), ..m.clone() };
            assert!(r.validate().is_err(), "{bad:?}");
        }
        // Only Sew 0.2.1+ knows the tier (0.2.0 does not).
        assert!(!version_supports_passthrough("0.2.0"));
        assert!(!version_supports_passthrough("0.1.1"));
        assert!(version_supports_passthrough("0.2.1"));
        assert!(version_supports_passthrough("0.3.0"));
        assert!(!version_supports_passthrough("0.2.1-rc1"), "unparsable is no");
        assert!(passthrough_unsupported_message("0.2.0").starts_with("sew_passthrough_unsupported: Sew 0.2.0"));
        let half = MediaRequest { turn_key_pem: None, ..m.clone() };
        assert!(half.validate().is_err(), "cert without key");
        let t = TurnCertRequest { cert_pem: chain, key_pem: key };
        t.validate().unwrap();
        assert!(TurnCertRequest { cert_pem: "x".into(), key_pem: t.key_pem.clone() }.validate().is_err());
        assert!(serde_json::from_str::<TurnCertRequest>(r#"{"cert_pem":"a","key_pem":"b","cert":"/etc/x"}"#).is_err());
        assert!(probe_result_ok("ok") && probe_result_ok("fail") && !probe_result_ok("OK") && !probe_result_ok(""));
    }

    #[test]
    fn kek_id_format() {
        let mut d = [0u8; 32];
        d[0] = 0xff;
        d[7] = 0x01;
        assert_eq!(kek_id_from_derived(&d), "7f00000000000001");
    }

    #[test]
    fn teardown_unit_names_stay_in_sews_namespace() {
        for ok in [
            "sew.service",
            "sew-db.service",
            "sew-migrate.service",
            "sew-media.service",
            "sew-recorder.service",
            "sew-transcriber.service",
            "sew-backup.timer",
            "sew-drill.timer",
            "sew-backup@.service",
            "sew-drill@.service",
            "sew-backup@daily.service",
            "sew-drill@latest.service",
        ] {
            assert!(sew_unit_name_ok(ok), "{ok}");
        }
        for bad in [
            "sew",
            "sew.timer",
            "sewing.service",
            "sewer.service",
            "sew-.service",
            "sew-db.socket",
            "sew-db.service.d",
            "sew-../x.service",
            "sew-a/b.service",
            "sew-Db.service",
            "sew-x@.timer",
            "sew-x@a.b.service",
            "k2-sew.service",
            "postgresql.service",
            "sew-db.service~",
        ] {
            assert!(!sew_unit_name_ok(bad), "{bad}");
        }
        assert!(sew_unit_is_template("sew-backup@.service"));
        assert!(!sew_unit_is_template("sew-backup@daily.service"));
    }

    #[test]
    fn sysctl_baseline_parses_strictly() {
        assert_eq!(
            parse_sysctl_baseline(br#"{"format":1,"rmem_max":212992,"wmem_max":212992,"recorded_at":"x","boot_id":"y"}"#),
            Some((212_992, 212_992))
        );
        assert_eq!(parse_sysctl_baseline(br#"{"format":1,"rmem_max":2147483647,"wmem_max":1}"#), Some((2_147_483_647, 1)));
        for bad in [
            &br#"{"format":2,"rmem_max":1,"wmem_max":1}"#[..],
            br#"{"rmem_max":1,"wmem_max":1}"#,
            br#"{"format":1,"rmem_max":0,"wmem_max":1}"#,
            br#"{"format":1,"rmem_max":2147483648,"wmem_max":1}"#,
            br#"{"format":1,"rmem_max":-1,"wmem_max":1}"#,
            br#"{"format":1,"rmem_max":"212992","wmem_max":1}"#,
            br#"{"format":1,"rmem_max":1.5,"wmem_max":1}"#,
            br#"{"format":1,"rmem_max":1}"#,
            br#"[1,2]"#,
            b"rmem_max=1",
            b"",
        ] {
            assert_eq!(parse_sysctl_baseline(bad), None, "{}", String::from_utf8_lossy(bad));
        }
        let mut big = br#"{"format":1,"rmem_max":1,"wmem_max":1,"pad":""#.to_vec();
        big.extend(std::iter::repeat(b'a').take(MAX_SYSCTL_BASELINE));
        big.extend(br#""}"#);
        assert_eq!(parse_sysctl_baseline(&big), None, "over the size cap");
    }
}
