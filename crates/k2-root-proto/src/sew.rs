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
//! **The pin is Sew 0.9.2 (2026-11-11, for K2 0.46.11)**, x86_64 and aarch64,
//! with its media, transcriber and recorder sidecars and its App kit. The
//! 0.1.0 placeholder and the 0.2.0 through 0.9.0 entries stay (a
//! pin bump appends; the helper still recognises each release on a box as
//! an upgrade or rollback source). While the last entry is a placeholder
//! [`pinned_artifact`] refuses ("this build has no Sew pin"), so enable
//! refuses at preflight (`sew_not_released`). Builds with the `test-signer`
//! feature (tests and the test-keys example only; release signing refuses
//! those builds) use [`TEST_RELEASES`] instead: K2's own fixture tarballs
//! under `testdata/sew/`, which hold a shell stub, never Sew. Test-only
//! features swap in Sew's TEST builds: `sew-test-pins` (its own table) and
//! `sew-test-pin-next` (the real table plus Sew's next test build, so a box
//! on the real pin can upgrade to it); release signing refuses both.
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
#[derive(Debug, Clone, Copy)]
pub struct SewArtifact {
    pub arch: &'static str,
    pub triple: &'static str,
    pub tarball_sha256: &'static str,
    pub binary_sha256: &'static str,
}

/// A sidecar of one release (`sew-media`, `sew-transcriber`,
/// `sew-recorder`): its own version and artifacts (`release.json`
/// `sidecars[]`).
#[derive(Debug, Clone, Copy)]
pub struct SewSidecar {
    pub version: &'static str,
    pub artifacts: &'static [SewArtifact],
}

/// One pinned Sew release. `Copy` (all `'static` borrows) so the
/// `sew-test-pin-next` table can copy [`SEW_RELEASES`] in a const.
#[derive(Debug, Clone, Copy)]
pub struct SewRelease {
    pub version: &'static str,
    pub artifacts: &'static [SewArtifact],
    /// Voice (`sew-media`, K13): staged and installed through the helper.
    pub media: Option<SewSidecar>,
    /// Recording (`sew-transcriber`, `sew-recorder`): pinned from 0.2.0 so
    /// the bytes are vouched for; staged and installed through the helper
    /// from protocol 15 (`k2 hostchat recording enable`,
    /// [`crate::sew_recording`]).
    pub transcriber: Option<SewSidecar>,
    pub recorder: Option<SewSidecar>,
}

/// The PostgreSQL major Sew's private `sew-db` runs on, for the pinned
/// releases (0.2.x, 0.3.x). Always passed as a first `sew install`'s
/// `pg_major` (never on `install --reconfigure`: there the install's own
/// `sew.toml` keeps its major, which may differ): without it Sew takes the highest major under `/usr/lib/postgresql` at
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
/// - 0.6.0: App-governed Spaces, with 0.5.1 folded in (never cut on its
///   own). `Alakazam-211/sew-releases` v0.6.0 (a normal release, published
///   2026-10-31 01:30Z; Sew tag v0.6.0 = core
///   `a8c0b652b5513b660f65f544b4ef97d485253116`, release.json `git_sha`),
///   pinned for K2 0.46.7. Printed by `scripts/sew-pins-from-release.sh`
///   from the published assets (SHA256SUMS sha256 `31634a5c…100f`,
///   release.json `66790d38…85b6`), which checked every tarball and binary
///   member and the App kit (`sew-app-kit-0.6.0.tar.gz`, [`SEW_APP_KITS`]);
///   every hash also equals Sew's own pin snippet. The SUMS file is
///   vendored as `testdata/sew/sew-0.6.0.SHA256SUMS` and a test holds this
///   entry to it. Schema max 960301 (0.5.0: 951201), so a rollback to 0.5.0
///   crosses a schema change. Features (20): 0.5.0's twelve plus
///   app_session_member_role, app_voice_channel_create,
///   home_client_owner_approval, k2_managed_guard (0.5.1) and app_controls,
///   app_governed_spaces, app_member_channels, app_mode_gate (0.6.0).
/// - 0.6.1: fixes on 0.6.0, wire changes additive only.
///   `Alakazam-211/sew-releases` v0.6.1 (a normal release, published
///   2026-11-01; Sew tag v0.6.1 = core
///   `8a4a055a6bfb9826b3ed293c92d7ce33bf0215da`, release.json `git_sha`),
///   pinned for K2 0.46.8. Printed by `scripts/sew-pins-from-release.sh`
///   from the published assets (SHA256SUMS sha256 `bbf15330…889d`,
///   release.json `48ad07a9…55c9`), which checked every tarball and binary
///   member line and the App kit (`sew-app-kit-0.6.1.tar.gz`,
///   [`SEW_APP_KITS`]); every hash Sew's pin snippet names (core tarballs
///   and binaries, sidecar tarballs, the kit) is equal. The SUMS file is
///   vendored as `testdata/sew/sew-0.6.1.SHA256SUMS` and a test holds this
///   entry to it. Schema max 961002 (0.6.0: 960301), so a rollback to 0.6.0
///   crosses a schema change. Features: 0.6.0's twenty, unchanged. Wire
///   (additive): the install report gains `warnings` (`pg_major_ignored`:
///   a reconfigure naming another pg_major exits 0; K2 logs Sew's warnings
///   and never fails a run on them), `SET_SLOW_MODE` is bit 48 of
///   `my_perms` (App mode), ATTACH_FILES is masked while uploads are off,
///   `perms_changed` is sent on an uploads change, and forwards can answer
///   `422 quota_exceeded file_bytes_max`. Sew tested 0.6.0 → 0.6.1 and
///   0.3.1 → 0.6.1 on aarch64 with the release tarballs: upgrade,
///   `install --reconfigure`, rollback (refused without
///   `--accept-data-loss-since`; schema 960301 / 930003 → 961002) and
///   re-upgrade, old data read back throughout, doctor 0 fail.
/// - 0.7.1: the server side of native voice, with 0.7.0 (K2 voice rooms, never
///   pinned on its own). `Alakazam-211/sew-releases` v0.7.1 (a normal release,
///   published 2026-11-03; Sew tag v0.7.1 = core
///   `77c5e1f878cf95a6ab95612b95889780249136a5`, release.json `git_sha`), the
///   step to test on before 0.7.2 (K2 0.46.8). Printed by
///   `scripts/sew-pins-from-release.sh` from the published assets (SHA256SUMS
///   sha256 `1a24c3b5…457a`, release.json `9d8c6ec2…b584`), which checked every
///   tarball and binary member and the App kit (`sew-app-kit-0.7.1.tar.gz`,
///   [`SEW_APP_KITS`]); every hash Sew's note names (core tarballs and binaries,
///   sidecar tarballs, the kit) is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.7.1.SHA256SUMS` and a test holds this entry to it.
///   Schema max 971001 (0.6.1: 961002), so a rollback to 0.6.1 crosses a schema
///   change. Features (28): 0.6.1's twenty, 0.7.0's server_governed_space,
///   import_identity_map, mgmt_voice_states, voice_channel_closing,
///   voice_media_path, turn_relay_only, and 0.7.1's call_credential,
///   call_stats_native. No app_origins (that is 0.7.2). `k2_managed_guard`
///   stays advice: `guard::REFUSE_ON_DAEMON_TOML` is `false` at this tag and K2
///   never passes `--managed-by`, so K2 still passes no `--i-am-k2`. Sew tested
///   0.7.0, 0.6.1 and 0.3.1 → 0.7.1 upgrade and rollback on aarch64.
/// - 0.7.2: App multi-host origins and the REAUTH deadline.
///   `Alakazam-211/sew-releases` v0.7.2 (a normal release, published
///   2026-11-04; Sew tag v0.7.2 = core
///   `e49e088d62e94e74b060e12cc78ada8ed4142ff1`, release.json `git_sha`), pinned
///   for K2 0.46.8. Printed by `scripts/sew-pins-from-release.sh` from the
///   published assets (SHA256SUMS sha256 `8717a934…59b7`, release.json
///   `74349c8c…ef25`), which checked every tarball and binary member and the App
///   kit (`sew-app-kit-0.7.2.tar.gz`, [`SEW_APP_KITS`]); every hash Sew's note
///   names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.7.2.SHA256SUMS` and a test holds this entry to it.
///   Schema max 972001 (0.7.1: 971001), so a rollback to 0.7.1 crosses a schema
///   change. Features (29): 0.7.1's twenty-eight plus app_origins. Wire
///   (additive): `origins` in the App settings (at most 32, stored sorted and
///   unique, absent when none), an optional `origin` on joins and on the gateway
///   ticket (`422 invalid_state` `origin_not_listed`). `k2_managed_guard` stays
///   advice (`guard::REFUSE_ON_DAEMON_TOML` is `false` at this tag).
/// - 0.7.3: the rollback puts the voice engine back (MRB1) and the App-scope
///   checks on recording, notify-pref and consent routes (SEC1).
///   `Alakazam-211/sew-releases` v0.7.3 (a normal release, published
///   2026-11-04; Sew tag v0.7.3 = core
///   `48192debedb8f54ffb2253867d7bbbbdb832f726`, release.json `git_sha`), pinned
///   for K2 0.46.8. Printed by `scripts/sew-pins-from-release.sh` from the
///   published assets (SHA256SUMS sha256 `9e03b235…a09e`, release.json
///   `e5a3864f…1987`), which checked every tarball and binary member and the App
///   kit (`sew-app-kit-0.7.3.tar.gz`, MIT OR Apache-2.0, [`SEW_APP_KITS`]); every
///   hash Sew's note names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.7.3.SHA256SUMS` and a test holds this entry to it.
///   Schema max 972001 and 29 features, both as 0.7.2: no new migrations, so a
///   rollback to 0.7.2 crosses no schema change. Wire: nothing new for an
///   in-scope caller. `sew upgrade --rollback` answers `media: {state, …}`
///   (`rolled_back | unchanged | not_installed | left` done; `manual` exit 0 with
///   `warnings: ["media_not_rolled_back"]` and a `command`; `failed` exit 1 with
///   Sew itself rolled back and running), read by the daemon's supervisor.
///   Client crates stay on 0.1.0 (0.7.2's) for 0.46.8.
/// - 0.8.0: many Apps per Space (participant links, sew-58), and 0.7.4's fixes
///   (the tunnel tier's IPv6 bind defaults off, credential renewal past an
///   expired login, the native leave reasons). `Alakazam-211/sew-releases`
///   v0.8.0 (a normal release, published 2026-11-05; Sew tag v0.8.0 = core
///   `0507cb99921de4b0b12414c59e1d284a67f1dc7c`, release.json `git_sha`), pinned
///   for K2 0.46.9. Printed by `scripts/sew-pins-from-release.sh` from the
///   published assets, tarballs included (SHA256SUMS sha256 `a0abc64b…4687`,
///   release.json `d87469a5…2594`), which checked every tarball and binary
///   member and the App kit (`sew-app-kit-0.8.0.tar.gz`, [`SEW_APP_KITS`]); every
///   hash Sew's note names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.8.0.SHA256SUMS` and a test holds this entry to it.
///   Schema max 980001 (0.7.3: 972001), so a rollback to 0.7.3 crosses a schema
///   change and restores the pre-upgrade bundle. Features (30): 0.7.3's
///   twenty-nine plus app_participant_links. Nothing in the mgmt K2 calls
///   changed; the link routes are only for participant Apps, which K2 doesn't
///   make yet. 0.7.4 was never pinned (Sew: go straight to 0.8.0), so a box
///   upgrades from 0.7.3. Client crates 0.4.0 (mirror `d3bbdefa`).
/// - 0.8.1: the phone voice release for native apps (client crates 0.4.1) and
///   the App kit's keepalive fix; the server is unchanged since 0.8.0.
///   `Alakazam-211/sew-releases` v0.8.1 (a normal release, published
///   2026-11-06; Sew tag v0.8.1 = core `21c55199e2b1604669564b91028db0cf8797f2bd`,
///   release.json `git_sha`), pinned for K2 0.46.9. Printed by
///   `scripts/sew-pins-from-release.sh` from the published assets, tarballs
///   included (SHA256SUMS sha256 `07c3d1e8…142c`, release.json
///   `59566c6f…2f9a`), which checked every tarball and binary member and the
///   App kit (`sew-app-kit-0.8.1.tar.gz`, [`SEW_APP_KITS`]); every hash Sew's
///   note names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.8.1.SHA256SUMS` and a test holds this entry to it.
///   Schema max 980001 and 30 features, both as 0.8.0: no new migration, so a
///   rollback to 0.8.0 crosses no schema change (a rollback to 0.7.3 still
///   does). Client crates 0.4.1 (mirror `96e63ae4`).
/// - 0.8.2: native video for desktop and phones (client crates 0.5.0), account
///   erase, and the App kit's native video. `Alakazam-211/sew-releases` v0.8.2
///   (a normal release, published 2026-11-07; Sew tag v0.8.2 = core
///   `2275837ba0e4ec252a95dd4dfe637545b4f97b1b`, release.json `git_sha`), pinned
///   for K2 0.46.9. Printed by `scripts/sew-pins-from-release.sh` from the
///   published assets, tarballs included (SHA256SUMS sha256 `150f62fd…7749`,
///   release.json `3f603f7f…6767`), which checked every tarball and binary
///   member and the App kit (`sew-app-kit-0.8.2.tar.gz`, [`SEW_APP_KITS`]);
///   every hash Sew's note names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.8.2.SHA256SUMS` and a test holds this entry to it.
///   Schema max 980102 (migrations 980101 and 980102; 0.8.1: 980001), so a
///   rollback to 0.8.1 or older crosses a schema change and restores the
///   pre-upgrade bundle. Features (31): 0.8.1's thirty plus `principal_erase`
///   (`POST /mgmt/v1/principals/erase`), which K2 keeps but doesn't call yet;
///   nothing else in the mgmt K2 calls changed (Sew's `k2_mgmt_0.8.2.json`
///   golden adds only the erase route). Sew tested upgrades from 0.8.1, 0.8.0
///   and 0.7.4, each with a rollback. Client crates 0.5.0 (mirror `29beb541`).
/// - 0.8.3: Sew's WebRTC dependency update (dimpl 0.9, upstream str0m fixes)
///   and native video fixes. Never K2's pin (Sew published it the same day as
///   0.9.0), but a box may run it (one was moved to it by hand), so it is a
///   known release: an upgrade source for 0.9.0 and a rollback target, with
///   its sidecars (a rollback to it needs its voice engine).
///   `Alakazam-211/sew-releases` v0.8.3 (a normal release, published
///   2026-11-10; Sew tag v0.8.3 = core `aa1ec3ea8d90b67e191f4b4c19aa9c3b26841742`,
///   release.json `git_sha`). Printed by `scripts/sew-pins-from-release.sh`
///   from the published assets, tarballs included (SHA256SUMS sha256
///   `a484eaea…df3c`, release.json `b2b895b4…52cf`), which checked every tarball
///   and binary member and the App kit (`sew-app-kit-0.8.3.tar.gz`,
///   [`SEW_APP_KITS`]). The SUMS file is vendored as
///   `testdata/sew/sew-0.8.3.SHA256SUMS` and a test holds this entry to it.
///   Schema max 980102 (74 migrations) and 31 features, both as 0.8.2 (its own
///   `sew version --json`): no database change, so 0.8.2 ↔ 0.8.3 crosses no
///   schema change. Sew: upgrade sew and sew-media together (K2 always does).
///   Client crates 0.5.1 (mirror `2924fded`).
/// - 0.9.0: recording in Apps (the `voice:record` cap, App and Space recording
///   settings, rooms that record every call with two or more people, the mgmt
///   recording reads). `Alakazam-211/sew-releases` v0.9.0 (a normal release,
///   published 2026-11-10; Sew tag v0.9.0 = core
///   `0ec838e2c4fbb2c5f02c808dcabfd7664246950e`, release.json `git_sha`), pinned
///   for K2 0.46.11. Printed by `scripts/sew-pins-from-release.sh` from the
///   published assets, tarballs included (SHA256SUMS sha256 `7aed9b64…d8f5`,
///   release.json `59962144…4e7d`), which checked every tarball and binary
///   member and the App kit (`sew-app-kit-0.9.0.tar.gz`, [`SEW_APP_KITS`]);
///   every hash Sew's note names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.9.0.SHA256SUMS` and a test holds this entry to it.
///   Schema max 990011 (77 migrations: 990001, 990002 and 990011 are new), so a
///   rollback to any 0.8.x crosses a schema change and restores the pre-upgrade
///   bundle (`k2 hostchat rollback --accept-data-loss`, which passes Sew's
///   `--accept-data-loss-since=<bundle>`). Features (35): 0.8.3's thirty-one
///   plus app_recording, voice_record_cap, recording_required and
///   mgmt_recording_reads (K2 reads them together as `RecordingFeatures`).
///   The same three sidecars, no new config (everything is off by default),
///   and no new root-helper verb to run it (the recorder install is K2's
///   protocol 15). release.json has no `models[]` yet (the speech model waits
///   on a licence point), so the model pin in [`crate::sew_recording`] stays a
///   placeholder and recording install refuses `recording_model_unpinned`.
///   Sew tested upgrades from 0.8.3, 0.8.2, 0.8.1 and 0.7.4, and a rollback
///   refused without `--accept-data-loss-since` and working with it. Client
///   crates 0.6.0 (mirror `c2fbd6bc`).
/// - 0.9.2: fixes on 0.9.0 and the native plugin's recording rights (0.6.1:
///   `set_recording_rights` and the `recording_controls` event decide Record
///   and Stop). `Alakazam-211/sew-releases` v0.9.2 (a normal release,
///   published 2026-11-11; Sew tag v0.9.2 = core
///   `0489d7a54a7f1b172ca81fe705362ac25df4dff3`, release.json `git_sha`), the
///   pin for K2 0.46.11 (0.9.0 stays: an upgrade source and rollback target).
///   Printed by `scripts/sew-pins-from-release.sh` from the published assets,
///   tarballs included (SHA256SUMS sha256 `8e9a63bb…3f25d`, release.json
///   `1800442a…0ebc`), which checked every tarball and binary member and the
///   App kit (`sew-app-kit-0.9.2.tar.gz`, [`SEW_APP_KITS`]); every hash Sew's
///   note names is equal. The SUMS file is vendored as
///   `testdata/sew/sew-0.9.2.SHA256SUMS` and a test holds this entry to it.
///   Schema max 991001 (one new migration, 991001), so a rollback to 0.9.0 or
///   older crosses a schema change and restores the pre-upgrade bundle (`k2
///   hostchat rollback --accept-data-loss`). Sew: upgrade sew and sew-media
///   together (K2 always does), then the recorder where it is installed (K2's
///   upgrade runs `sew recording upgrade` there; its argv is 0.9.0's). Schema
///   78 migrations; features (37): 0.9.0's thirty-five plus group_dm_names and
///   dm_order_activity (both from the published binary's own `sew version
///   --json`; K2 uses neither). The same three sidecars and no new
///   root-helper verb. The speech model is the same file as 0.9.0's
///   (base-q8_0, now in the release's own SHA256SUMS and `models[]`). Client
///   crates 0.6.1 (mirror `85f5c87f`).
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
    SewRelease {
        version: "0.6.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "38f6ccce8cc570765a9b05a38bbc38b051bbca89d7edffe53897b093bf6692b9",
                binary_sha256: "6868922f6d536d9a4eed3836bdc18762279e34c99409818d0210af5cf6819002",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "5309938053248b17b6c3ef83e780b9e824f95d3144f42f75ce6ecafb0f86c922",
                binary_sha256: "7641d400038cae0c58ac36a4e88e3fbda5b1201f0b8db431bc14eb3f3f483049",
            },
        ],
        media: Some(SewSidecar {
            version: "0.6.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "56d3eecc7075a0116114fb2686ccd69e35a31dd072ef13076b56592e1bc61217",
                    binary_sha256: "78480663488aad6b63f14039e621a30df8da763e366ca7f2234a954560125cbf",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "19046e2ab5e7b7b504fdf3e3aca06b4a72fd2ef6ff970db867d44d607eb68f96",
                    binary_sha256: "49b6df0c151a3be31efd3f4890bdb188f68f8869bd6a473b9c289896b29f693a",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.6.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "9a3c55e57669aef032cc94432a80527aaf985887b8f15e00168d3729b437696b",
                    binary_sha256: "27206cc1d8545067937d5224d94334597da09c42b348097187c2bfe8080f34de",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "fa2ec365a32a2657ba5364426ffb96dc57f8edd69adcea71bda76be30d5678fd",
                    binary_sha256: "166525b6becb0a2c39af431a1c1747615c8f12602d98c20c79e8941d9073eb91",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.6.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "be1b333af12605253a07764f89265a1a942b43a8d2307b6c2f476296020c9f17",
                    binary_sha256: "4b25383cb03229abc4c66c3b3d1fbcea165b15c6da9a056c921b585b7bdf7b6f",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "b88c0e0ba8188eb845fe5f134462e81543fdefa1763cdb96cdc81e2e817f4f5c",
                    binary_sha256: "cf0571544230380d0b095f3bc8648d8be1364df6d0d838bbe8be0541b02a3714",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.6.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "d09e5b81ecd1675809eab1ed28245c200710e05581eec0977cc87cc6a6ca7aba",
                binary_sha256: "a1be61b38e85fc450cb742f375d9c203231094961b3ea77389b32fe208bfbb25",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "4bc2a14aee68fe941d91161872d7a171a0112ab179a133f797e2aa795a8d1f14",
                binary_sha256: "55a401c329ce978a43c6d7667ae4eaba37b684106884cfdd77dee80a77158390",
            },
        ],
        media: Some(SewSidecar {
            version: "0.6.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "a060697d32d60dd9fc076c57eddb6569553488a5cf46de6ae7cb43ec7593d3ec",
                    binary_sha256: "244ad5fa9dc1adbac3ce6b69977b8f73f96df82fd04c6838d5e6684937300bbf",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "0b9d7ed4976b53161b33e64c6a0d3a0fd6c74b0d73f7b472233b54a9593b6d91",
                    binary_sha256: "72820879f04a21a25980581fdb733d5bac1b5a4b1ad3b57a28d2c6f4a63d0128",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.6.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "aabfc3638e99cfd237c39a7fd15e61c4ffe78e138f726dcac6570da3d07ba0c3",
                    binary_sha256: "b751c5086979ff294971dfa5dd69f8baed9bcffc6536afee59dda3dc17499782",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "5b9d3631770e790d3cadad7cc5a62fcbf30a4d71e23a01a5f7567edfbe077f2c",
                    binary_sha256: "499cca6a4475fbd79f4f23c1e84bb08240c2925b0d01bedf4dc55208aa97583a",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.6.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "94ed350014402cc4268c8e8ab77ef4d624fe628a8dac60d4645021306b984081",
                    binary_sha256: "8e9ee0af240e43cc1e2fb964b8f6a1ab6fdf867ee3f65ec106010b7aca851bf4",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "0232fc9663459212c0c8c86fd7b71b06fce8a7f1101964cf6875884850081877",
                    binary_sha256: "41ccd2ed721029216a23f63d7e1a27932ad8a2f45c03e96e6c0f973c2197ea87",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.7.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "48ad5852b233bff582febf77722b2ea7e8d9dce381ab4f3adca78ebf56d32911",
                binary_sha256: "9ae057f513db2f88b896b6859552d1fb96105ab2cc9c38af88f6c6d25d97057a",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "e8aed3887b1e61215e740358f74d555cd694104705a9ed3b557415168a5605de",
                binary_sha256: "01c761832b9e896654b0df3814e69aee2578221727e749049a87a73dafd78138",
            },
        ],
        media: Some(SewSidecar {
            version: "0.7.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "e79c3e4f0422971573f22fba2e1b01ec0a405d2b0e7748c810f0d1cc25d3e40c",
                    binary_sha256: "19a84699a0cc90c3c2bd79aaad1b741496b13339bf9e75979210325610a282ce",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "aaa05004b13e9378d955353d9df7e87e14ad380d0a722f835bb0f477a056819b",
                    binary_sha256: "3e3689b91067f6bf3188e2dc34a0a9a1b80b6270cf37121f8f688a3c5fe7242a",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.7.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "382074b790ef51457956cdfb9d0d434391b0f0b65be74d9381a84aa2e7efd150",
                    binary_sha256: "faafe811e5138e6044e30f86ee8f1c10c6ebe9f02feb8c3cb7b2557d49866f71",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "cc3b7ad7dbb67ad8f1e39e3de29cad88db3c3345724debf6e6b54ed811c1ad92",
                    binary_sha256: "ae96bf72e51d28de5d8da63444a2dc1f158a5c9f4a08dd15db00b2586fa54367",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.7.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "32eac90ff046cdf568bc95763d194189e5fe3a317f035148485b485c4f91bf79",
                    binary_sha256: "fd1280cd0308cd1fddacc7953734528ec8f1f53895dd1105696c682b18e57d7e",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "fe234cb1eca92b999cb6de25c4f6c8ffe1776dcd94de230bac03dac932539339",
                    binary_sha256: "24aee93dd38c3fd00d4162980567f58c5348097e17c602c295442278ee98ec67",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.7.2",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "b8b19ace7c7ddb49d3d370a51cb88f0abdfe6e49d1057b594ed8a3df6438f778",
                binary_sha256: "72ce3d10f88371020e60fd6f8825fc73dc7f93757fdec276170c0c7f9b938810",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "ca12ead6efa58e7a9945db588fcd297fcad51a1f26dbb22dbf3a761c9b011c2a",
                binary_sha256: "da23d074c8698e9dc786d5a005ad47e1d02c77d98e95cf6096ce2ed263219c99",
            },
        ],
        media: Some(SewSidecar {
            version: "0.7.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "a351d6667d49be03b8e354a00d20dca8a9e91b84c17a5afac56bd115a340f148",
                    binary_sha256: "b764a51fdeb0f92aba82eb486556a26db9e05935cf93acb14bc3258fbb346754",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "e61c728dad3c6ce692970e8c733ad7021940096824aeadc9330afd0fb633e15b",
                    binary_sha256: "da6a6fe155be1f4a8812c902f284ce80799a1ff155b03f96775d0685e7cd2182",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.7.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "1981bc36e1cf8317f3bd4f711ee5071f12311a63f1254cf73f73a367014c65d8",
                    binary_sha256: "ae5011dd54fd116fae6cb8b7257ed2eb2b0cc96410c2ddd6603a83b6c1f73046",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "b596e8537e87afd93cd4c086f38e5adc84b95baf0c85a7fb7051c29ae84f0b09",
                    binary_sha256: "84e1405896d83f2fb55bff74a6cedb0ef54f0bf7e4e7c343d743131302134fd7",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.7.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "bc9e5c4873974bff700522073c7093956507d66d3281a1687c5997d22c0da5f9",
                    binary_sha256: "e9d6ee7f9652319a810b88b98a85d42ab0862751fb53a944f8b201127649bc6a",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "022f25593a9e1b4f3580539236104eb3da15d7e1b7c9b7f5c1c896b7717b7d72",
                    binary_sha256: "e0819d41af29cafbffefb690e433d7f2241bb1f117527724bed0d28f68f46c05",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.7.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "926620fd050bad24a3a4f902a2074ade4c4637d989984db241439e304335eca6",
                binary_sha256: "3c4cfea4ec3fffbad8ba9be4edeb431d3bb991719780c32a863d8dd87d53d6b6",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "6c8af48c2ac0e59bb289b190d2f07216076e0740ee3b8bd5dc777f1befc971ce",
                binary_sha256: "2a10b4e50369f5fb287823616036cfb200a09dd5286caab46487519f1f7c6277",
            },
        ],
        media: Some(SewSidecar {
            version: "0.7.3",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "87cbabefab796d846b4062fd3f8107df49a7652fb3156298cec69984a9aba051",
                    binary_sha256: "f7f40060c719c54f0d9fb409e6477f7ffec06a601dc40365731c09112149a300",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "99791518cb15b944fd2e782379c0c57688fc16f34c191a249b8494f02a40b8c6",
                    binary_sha256: "b3456475a7df5811364dd838f3693d350c5201cb143713500f287c94503c5665",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.7.3",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "636ce9c81d07efe95780d560081d11e68eae8422ed221244cec1702d07d434e1",
                    binary_sha256: "cfe1dce3a0451cfc02e772f1c5be1265b65716dce30cf45faeb3be02b5269b9f",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "1beb5a7781009959cbc454a607300c4082b4ac75f5892662c9fa53b4f3f1f40f",
                    binary_sha256: "0e82d8a8c740781e0743f9463f505ad32e4e908dc3fc5bbc468b416cd8792f2c",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.7.3",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "501b586c745b53c5ce8d5555833997b59bceb904e2098c68025e83010cb4731c",
                    binary_sha256: "4ab598362d97901a1ee607f6a3eb237502eccf3224076e6a839be32070e6da1d",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "7f5dc99cb85146c9a23122eebc66281118a16c026ec9960de6d073c6f914d75f",
                    binary_sha256: "dccd501190159dd1834c7a24ebf546157a6c619c815dffa0f85393c117fd1114",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.8.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "e28c8d1d515e23c9c23fa5139c8e3e877e73415ab8321e63eddb6b11ef010c3e",
                binary_sha256: "44a52050b83f41692aa6c3963a3a1af4893878c5ee72e30c6bac80a053b5000b",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "936d540be1fc36eca0b702befaa32ab5052bc31c4b59e397b6bb1e0c0a4501f3",
                binary_sha256: "4bce693a6cc3698a40674482768e8f7afe98c18f9de5c52a776704c4c181b94b",
            },
        ],
        media: Some(SewSidecar {
            version: "0.8.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "dc0f11c6159449b945f3797109ed35bae61bb65e11c2911c7b265e130d1e1f82",
                    binary_sha256: "ec5c3b541595fa726df2a2f6294b36fbb3298a81009185eba033ff86fcaf8c94",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "0daf31499001f35ab0d3e9bb2c0589c82bc204789fdd5047903c285299991cd2",
                    binary_sha256: "3e7ae64b384d70b015556561f6b7a3e388db696bab5686feec5dbed0e9f3ac0f",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.8.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "8352f0501a38fb0e8bd1002aec60c532e8aaed7253f39a8f1bf4a384bd4e45a4",
                    binary_sha256: "05f086c3b338446549eb21c13f9afad46ff01fb6c1450fc8275de46fde7b2ec6",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "756a02f47fc827c9980b3798b970b39a9ececfda465ced71a7b1603e96b81521",
                    binary_sha256: "c8fcf72178bc99c72da15de21f00cc3c1c09c7ddcb68c7bb9565b4deae132dc6",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.8.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "96ee980fec5dc649a95a7d74738bd39663cc5af469a95c64bbaec1f770a2f7a2",
                    binary_sha256: "6a5766405800dd3b421ab6be89f5159d2c7e909ffc1717a139b143b8a098b521",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "41e287b7670c5af5ee49dd0f0bdd04d2eb05bf48b6e4b2efb9de58403ee756a4",
                    binary_sha256: "14896e381cfc881638c068bfc72c7e5f110507adee14fa3bfd6a9154df5b8589",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.8.1",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "d79d2256f8f4a7f0d116129158d29a64ebe3e6275e53adf06d29a7407d369648",
                binary_sha256: "49a9f447140994589442ca96b99c006b23f6bcdf3558611cd6ac3ecb7f26e066",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "ce3d1c23bccfb3b9d896ec576020b3135d28157a2d28cdb1fdec24a68ee46e11",
                binary_sha256: "2ddf783800e7aa9d5cd4b9a616376dc54ed2b42c28e4c9468b5846ddf2717203",
            },
        ],
        media: Some(SewSidecar {
            version: "0.8.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "c8ab36b5d594e5b0a111af697851f7acd1d0cad8af24cba9a9a36bc706d28b91",
                    binary_sha256: "a82f92002a445ca2d75aaee0ae44d778bfd4e8c8d2dbac40f09374899d932415",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "f6c0e7661e9db42add28b3d4f57812a261aeaf54856dedf2a57ea79bb2a52b60",
                    binary_sha256: "f579ea83bb16404ed3d3021bf156471a88f58af70e69c988625280802e90d93c",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.8.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "0f422beda43e6127c3d14c309422bbc1c2e95291131721aabf0d4ac3f94c6371",
                    binary_sha256: "b8056d0b53ea636cb41c7628a2c4498e9034d15e21e3f5e7a6ecccf0d849ed47",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "bd4266d93dc343f18e98e9347bc3ce8876f809518aa41a143b309c9b79cbb830",
                    binary_sha256: "083257cc9be2128618204d189262a1569ecbd2b4291e5565c901c089ba858242",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.8.1",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "99ec3717824fa689ef6b804594d9df9db2151d85dce75bbf4d72f710dcdba65a",
                    binary_sha256: "5bebc42287b469ab92749f5b0544893818ed624cad0604b2e383f394710b3a79",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "b0b9db1e08902213c7cc6dfcfbe3b4fa0cb951d815dbf27b54433e97fd3ee4e2",
                    binary_sha256: "2bc60c68a949ca2c9cc846dae0bb34f860ba25ddfbf510aaa137cc8e81fc5f5f",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.8.2",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "27d98e7a0d5bc1e9b17cb19ebf58262f2441698e606905d83d65d89e7399de45",
                binary_sha256: "66867e3cbc7c8fc5849a251bc8a46656751e2ece974b1ceb9c3ceff920f14a87",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "0a2a63202226a07f0207847c253de17cd54fc4a9a7bff53b3d786a53870a7264",
                binary_sha256: "c38c80feab75f5cbf0762e76ffed115decd148f0a780741828bd7b8b079b9014",
            },
        ],
        media: Some(SewSidecar {
            version: "0.8.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "5b8c220fbd34f2039091bd6e6238242f7ea162ed744be01244d7b51500905293",
                    binary_sha256: "97695e8a9996ef5c6d1094868ec3c5f38ca922f294147758c3ccafea6fc545b3",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "0b6089d356b02fcfb45eaa874efce76b2d02b93c27c53d2d92e46f83102aa4e4",
                    binary_sha256: "79a640ac8655440ddc99b52cb04c43a85dba518ae388b60454ba679ad5e171be",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.8.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "49ae50c15d72156a87d006746db9fa0ac629d3ae71ff9511c8254704b6c68d04",
                    binary_sha256: "2f863060a42368bdb47d29da03199e0dbcfb1de6066c9021af58a2867bfaabed",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "1e888336e79b517a983d41c7de2eaeeadad2987dc322f5caa73a7ff86130ce55",
                    binary_sha256: "14e31a75319d89df205199312278236d02d4d6ef8a76e4a1409def4760f23f54",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.8.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "3a2e34262725612d4681cc660b9376f6a049f5f5c43a15f4ea4a1b19e3be0092",
                    binary_sha256: "67e6555dd45366aa73ad65b4c79c08d27d85d7ec6816118a9c86ceb44ef20bfa",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "ff0d95fe45ab46a5ba29f6417f4a81178705f1e79d632982c149514be519891e",
                    binary_sha256: "13922e6441906c69f13cbd782d96109b2bd3d8c128301af4fd572494cfb36190",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.8.3",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "00e784c7ebc05758524a0bfe6e1354bd43ba3860ab2b19210f97d14416d5581a",
                binary_sha256: "cb1474e4065e860c4d7227b6477b361d002c460c1583dafba4702c525a7e0801",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "7893fd9e151ad5cc6021ea24f88a0795eea6f1c15410ac804cea3d7d41933ce0",
                binary_sha256: "37c7d41d371eff99eb943a76ffe10140c398310fd087d1ac34bf6ac532dfc277",
            },
        ],
        media: Some(SewSidecar {
            version: "0.8.3",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "d6d4254e322ed54f03d11f1663326c76f6091fa9db504804f35cfda74b142c69",
                    binary_sha256: "31990345bbb83cde1baed7987ad75cc72c0aa5e194f1a769f690c626685e61be",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "ee5451e552bfab5ed389fa622871039fa180148f3da6672b3db4d923747fd409",
                    binary_sha256: "8f7f528eff3af16be16646b75970e930a71eb55ecc2314605483525bdba0329f",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.8.3",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "1bf2bed5279fa32747b62f4001b8b18a8fbbc59390e1937d8a07b1ef8695347b",
                    binary_sha256: "0da84798c56642f26cf734886d7aafe5ed70b6a8466e0748de1d22eadda21373",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "cec8ff43b2953f37f7350d7e5c901611589ca56391eeca7329268428e2778f77",
                    binary_sha256: "9241521f5fa33fcc733bff5db48271616700016828acec37ef51ac6fe1058df2",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.8.3",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "b59aa4c3959f7ed62d613fd0782da4273c931a31fc7d5c5697650d2fa9367179",
                    binary_sha256: "38f35bc1d7d6978b7964d67559e59723f363ba50572ee2c95f194e3220c1381e",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "f95f7dfcb378cb8627a3bbe5eb9348361d4aa764004ffa2be231aa5aa80995e6",
                    binary_sha256: "3642b15ba1bdede978c6d51668c1b56a2d4b3d17550293a3871306aba44c5fb0",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.9.0",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "dc31ab060d142b166b455ff392303c49fe41bec33db7dab229fe02d6a61b7a1d",
                binary_sha256: "ba88fdc2607bc9da3d88336c7e4adee1af7152a90a2bd8af1c6c6812caa1ac1d",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "761c432452baf5a488c3a319f96b40e05e88f762eeb8d122e29f6a2675f033d4",
                binary_sha256: "d1888020cf7ea3b82bf25d83d65902cb5debe865d584d8376c5b0d5b064ad302",
            },
        ],
        media: Some(SewSidecar {
            version: "0.9.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "a53812cbf8924e5fca72c5ef2ec95b1742a534fcd15b765e6c62e9409c403aaf",
                    binary_sha256: "c4a58b5513ac989dc9cbfeae5783b454c438e3caef1fc4dda7a40d46e7f82599",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "1c85071b731173682d9da1560d9647c28ae4425adbc8bcd0035cd5eafdeccdf0",
                    binary_sha256: "8f39bb0a469a5bb7e6f3486bb4da6d74cc9bd482f1f716f2190f893ed0eec76d",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.9.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "a5c05a3d670c196843395540100b634a4b7fee0c5bc615845a65ca263d4c2e5f",
                    binary_sha256: "5e577e526f7d3e4615a2d360b62d1fc0af71e1ae170586f1e7281649fc7ef988",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "41e7c22790b35974c3924499d290bce729dbf08bea40780f336cb0e32b53968d",
                    binary_sha256: "cb59f68992330e0923c18e439822b4e76770243b4f21c2689958d19507cd4da5",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.9.0",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "8f51029e79b71a89b2f5da00b0eeee1a6e18d1e33158276a4bc41c0eccfb119f",
                    binary_sha256: "715c91e710597f53905f9b3f6e1abe303692fc03492a1634638901467f423560",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "4238b82d9ce62fc566107e80f3fa9b5715bd4dc04b818dca51c61c9b55b1aa98",
                    binary_sha256: "7e6e02e58afdfa862cdb3efdfcc70c1a39de85c77731d942ef27b1c70d810ebc",
                },
            ],
        }),
    },
    SewRelease {
        version: "0.9.2",
        artifacts: &[
            SewArtifact {
                arch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                tarball_sha256: "fce76f3ef55ad53ea6ad3f32614551f806fd112f5aede8caee4be8aced2c2038",
                binary_sha256: "9af08256897c1a279ab1919c78eb2bb12af1561246f197f58eea3602b59117fd",
            },
            SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "91985f336d747e4490bd26d56e733381492b11a4f8f7f1da81fbb440cb53dd8b",
                binary_sha256: "f13d7c7cd3be3a114dceaf1b36ab0092735fd971471c53f68201a32b7cf0d1c6",
            },
        ],
        media: Some(SewSidecar {
            version: "0.9.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "8fb823bbfe24836a218741d57516b1920bd151b76fb10117b4c7e973b16d9649",
                    binary_sha256: "4f22a87304caf304ad89898c4b47947b817676e0d5599f568406730edc4d0e29",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "ee9da2230c133e563909ee248d7fff30400c1c71ccbcbea25e7ebabe042c50e1",
                    binary_sha256: "588089e75938668964b4eb41fd99b0c4916142bf2f947b8a6ce832fa04091097",
                },
            ],
        }),
        transcriber: Some(SewSidecar {
            version: "0.9.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "53b0708c9ca1d4ec9363cadf1ffa33a04affff324b095957bcc1b5d3fbecea63",
                    binary_sha256: "80f47009fe68e8a1af58e818241e108b69764b146b356d475058a7d51ceaf4b7",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "94dbee0b56afdf4b0f199f1facd2fe356426398f6792d6829f9dc991dbf457d5",
                    binary_sha256: "6d419b90d810b7ad4bc6833ab00f376523b922ef56f352c50c638900ab085aa0",
                },
            ],
        }),
        recorder: Some(SewSidecar {
            version: "0.9.2",
            artifacts: &[
                SewArtifact {
                    arch: "aarch64",
                    triple: "aarch64-unknown-linux-gnu",
                    tarball_sha256: "ecf082ea188df04a6bbef4808eaf24e62afa64abd4d82d2747a7a3aec9d71ee0",
                    binary_sha256: "c9b922529a02eebe1760d0c672666370cd8a0a27c48103a5f5f1a0156ce7c428",
                },
                SewArtifact {
                    arch: "x86_64",
                    triple: "x86_64-unknown-linux-gnu",
                    tarball_sha256: "9e9354e76ecac2e9c3de54d219908ca8da3e8d63f10039eac5d6a8cb70278cf7",
                    binary_sha256: "f42b0dbfaae6eb0d0ba96c693c46676c30b06a821c8dd3e7cd67f62daafdc43b",
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
        // The recording sidecars (0.46.11, K-a), from
        // `testdata/sew/make-recording-fixtures.py`: shell stubs, never run.
        transcriber: Some(SewSidecar {
            version: "0.0.2",
            artifacts: &[SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "f0232517bf84297b80153c1c5d7132f13574f7bba392a09bd37736e5e96c99bf",
                binary_sha256: "36d0765e0a0162d4f6c8a24ced18f7109b041caf9e5eb79ddff1fdcc722fa2b3",
            }],
        }),
        recorder: Some(SewSidecar {
            version: "0.0.2",
            artifacts: &[SewArtifact {
                arch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                tarball_sha256: "85680e195243666f5e8573b2a9d2c0db9b74b086812b0b687de7d48f0b257390",
                binary_sha256: "fa32f8860fcfb41d9cb64739e3efbfba8ecc72b9594a6d2da805a0fc61443ac0",
            }],
        }),
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
/// 0.1.0 stays known, as a real pin bump keeps it). `sew-test-pin-next`
/// (below) is a third table and leaves both out.
#[cfg(all(feature = "sew-test-pins", not(feature = "sew-test-pin-next")))]
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

#[cfg(all(feature = "sew-test-pins-upgrade", feature = "sew-test-pin-next"))]
compile_error!("sew-test-pins-upgrade and sew-test-pin-next are two different Sew test tables: enable one");

/// Sew's NEXT test build (V22 in `prd-k2-voice-rooms-v1-vs-live.md`): a
/// pre-release Sew (0.7.0 for voice rooms) that a box on the REAL pin
/// upgrades to. `sew-test-pins` can't do that, because its table replaces
/// [`SEW_RELEASES`], so the real pin on the box is no known upgrade
/// source there. The `sew-test-pin-next` feature (implies `sew-test-pins`:
/// same flag, same signing refusal) uses [`SEW_RELEASES`] plus this entry
/// as the pin: upgrade from the real pin, and rollback to it, work as in a
/// real pin bump. A default build never selects it
/// (`k2-root-helper/tests/binary.rs` checks the built helper carries none
/// of its hashes).
///
/// **Placeholder until Sew's next test build (0.9.3) exists**:
/// while it is, a `sew-test-pin-next` build pins nothing it can install
/// (`sew_not_released`). To drop the hashes in:
/// 1. `gh release download <tag> -R Alakazam-211/sew-releases -p
///    'sew-*-release.json' -p SHA256SUMS -D <empty dir>` (add the tarballs
///    with `-p 'sew-*.tar.gz'` to have them checked too), or copy Sew's
///    test-build directory;
/// 2. `bash scripts/sew-pins-from-release.sh <dir> --next`, which checks
///    every hash against SHA256SUMS (and the tarballs that are there) and
///    prints `pub const SEW_TEST_NEXT` and `pub const
///    SEW_TEST_NEXT_APP_KIT`;
/// 3. replace both consts below with that output (keep this comment; put
///    its `// provenance:` line in it), and compare every hash with Sew's
///    own snippet;
/// 4. the version must be plain `x.y.z` (`Semver::parse`), above the real
///    pin; both arches (the SYS-LE-1 test boxes are x86_64);
/// 5. build the daemon AND the helper with `--features sew-test-pin-next`
///    (the daemon refuses a helper with another table), run `cargo test
///    -p k2-root-proto --lib --features sew-test-pin-next`, then
///    `install-root-helper.sh --file` and `k2 hostchat upgrade --tarball
///    <test tarball>` on the test box. Never `k2 update` that box. Before
///    the final release, clear `~/.k2/cache/sew/<version>/` there (the
///    final has new hashes).
pub const SEW_TEST_NEXT: SewRelease = SewRelease {
    version: "0.9.3",
    artifacts: &[
        SewArtifact {
            arch: "aarch64",
            triple: "aarch64-unknown-linux-gnu",
            tarball_sha256: "PLACEHOLDER-sew-0.9.3-test-build-not-yet",
            binary_sha256: "PLACEHOLDER-sew-0.9.3-test-build-not-yet",
        },
        SewArtifact {
            arch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            tarball_sha256: "PLACEHOLDER-sew-0.9.3-test-build-not-yet",
            binary_sha256: "PLACEHOLDER-sew-0.9.3-test-build-not-yet",
        },
    ],
    media: None,
    transcriber: None,
    recorder: None,
};
/// [`SEW_TEST_NEXT`]'s App kit (a placeholder is never a kit:
/// [`app_kit`] wants 64 hex).
pub const SEW_TEST_NEXT_APP_KIT: SewAppKit =
    SewAppKit { version: "0.9.3", sha256: "PLACEHOLDER-sew-0.9.3-test-build-not-yet" };

/// [`SEW_RELEASES`] with [`SEW_TEST_NEXT`] appended (the pin).
#[cfg(feature = "sew-test-pin-next")]
const SEW_TEST_PIN_NEXT_TABLE: [SewRelease; SEW_RELEASES.len() + 1] = {
    let mut t = [SEW_TEST_NEXT; SEW_RELEASES.len() + 1];
    let mut i = 0;
    while i < SEW_RELEASES.len() {
        t[i] = SEW_RELEASES[i];
        i += 1;
    }
    t
};

/// [`SEW_APP_KITS`] with [`SEW_TEST_NEXT_APP_KIT`] appended.
#[cfg(feature = "sew-test-pin-next")]
const SEW_TEST_PIN_NEXT_KITS: [SewAppKit; SEW_APP_KITS.len() + 1] = {
    let mut t = [SEW_TEST_NEXT_APP_KIT; SEW_APP_KITS.len() + 1];
    let mut i = 0;
    while i < SEW_APP_KITS.len() {
        t[i] = SEW_APP_KITS[i];
        i += 1;
    }
    t
};

/// The test pin table this build uses (the last entry is the pin).
#[cfg(all(feature = "sew-test-pins", not(feature = "sew-test-pins-upgrade"), not(feature = "sew-test-pin-next")))]
pub const SEW_TEST_PINS: &[SewRelease] = &[SEW_TEST_0_1_0];
#[cfg(feature = "sew-test-pins-upgrade")]
pub const SEW_TEST_PINS: &[SewRelease] = &[SEW_TEST_0_1_0, SEW_TEST_0_1_1];
#[cfg(feature = "sew-test-pin-next")]
pub const SEW_TEST_PINS: &[SewRelease] = &SEW_TEST_PIN_NEXT_TABLE;

/// Present only in a `sew-test-pins` build (`sew-test-pin-next`
/// included); release signing refuses it.
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
/// test builds in a `sew-test-pins` build (with `sew-test-pin-next`: the
/// real pins plus [`SEW_TEST_NEXT`]), else the real pins.
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
#[derive(Debug, Clone, Copy)]
pub struct SewAppKit {
    pub version: &'static str,
    pub sha256: &'static str,
}

/// Every pinned kit, by Sew version. Kits start with Sew 0.5.0: a release
/// before it has none. Each row comes from its release's SHA256SUMS and
/// release.json `app_kit` (`scripts/sew-pins-from-release.sh`), in the
/// same change as the release entry.
pub const SEW_APP_KITS: &[SewAppKit] = &[
    SewAppKit { version: "0.5.0", sha256: "64efdf1d822be2a4c49ccab437be8bddd5068c6e906b968b94beb44d8b1a0418" },
    SewAppKit { version: "0.6.0", sha256: "9f8602281a3029684fd9d12665242b2555d8a86c2e1975621e8282933076bc43" },
    SewAppKit { version: "0.6.1", sha256: "955357393d869b950fa6401462c3d42453cca4f32e84be7b16ca341b4ba20b35" },
    SewAppKit { version: "0.7.1", sha256: "8add54188d362927c14dce6f01710b3ac1ad0956780ca1dd77f8d5fcc9e2a192" },
    SewAppKit { version: "0.7.2", sha256: "5867fe54ff1b6732578b1d61d5890ab474f4337bdd636a31bb1f3b3699d9c7a1" },
    SewAppKit { version: "0.7.3", sha256: "585ab6f099c15d17abb2b8ac6e9664c11e8417d9470ce63d94b116c9adca53c0" },
    SewAppKit { version: "0.8.0", sha256: "0a063ddfd80f5bdc19217f510c018372059ddea55b074e066af4f30cac47f4e9" },
    SewAppKit { version: "0.8.1", sha256: "c1be5c5fd0c1725c3353a4f091b446857ed8c474754a0a33ea2023c0e505ac56" },
    SewAppKit { version: "0.8.2", sha256: "d15e575379256381c1ffc0303a3d3adaf21307ed36d8ea285fc32945baa5e80e" },
    SewAppKit { version: "0.8.3", sha256: "406bdc38d6d10b2efcc22dbca1e6f42850fd889150c08a0820bc9fa04b376d3d" },
    SewAppKit { version: "0.9.0", sha256: "a3bd4d5dba566c5f9f8a30ad27a34bc716128fc29cf7944f992c41c8961b6c2c" },
    SewAppKit { version: "0.9.2", sha256: "59f9ae96dcc9a54597daeba5f3b73fe3fc6f720a7acc7e0cb1e64caa45a34a20" },
];

/// The fixture kit for `test-signer` builds
/// (`testdata/sew/sew-app-kit-0.0.2.tar.gz`, made by
/// `testdata/sew/make-app-kit.py`; never Sew code), for [`TEST_RELEASES`]'
/// pin 0.0.2.
pub const TEST_APP_KITS: &[SewAppKit] =
    &[SewAppKit { version: "0.0.2", sha256: "ebbd6b86f6cad5deef69eaaecc298c76e9a7f448953ba485b9623bebb6734975" }];

/// The kit table this build uses (with `sew-test-pin-next`, the real kits
/// plus [`SEW_TEST_NEXT_APP_KIT`]).
pub fn app_kits() -> &'static [SewAppKit] {
    if cfg!(feature = "test-signer") {
        return TEST_APP_KITS;
    }
    #[cfg(feature = "sew-test-pin-next")]
    {
        &SEW_TEST_PIN_NEXT_KITS
    }
    #[cfg(not(feature = "sew-test-pin-next"))]
    {
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
    "Chat / Voice Hosting isn't available in this K2 release yet. It will be turned on in a later K2 update.";

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
    /// without a supplementary group and K2 never edits `sew.toml`. A first
    /// install always names [`SEW_PG_MAJOR`] (`pg_major`).
    pub fn sew_inputs(&self, daemon_uid: u32) -> serde_json::Value {
        install_inputs(self.provider(), self.jwks_url.as_deref(), daemon_uid, PgMajorInput::Pinned)
    }
}

/// Whether Sew's `InstallInputs` name a Postgres major.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PgMajorInput {
    /// `install`: `pg_major` = [`SEW_PG_MAJOR`], never Sew's "highest
    /// found" (Sew writes the config now).
    Pinned,
    /// `install --reconfigure`: no `pg_major` key at all. On an existing
    /// install Sew's own `sew.toml` (`[db] pg_major`) decides, but a
    /// `pg_major` given must still have its binaries present, so naming 16
    /// refuses (exit 3) a sew-db on another major (sewlab: only Postgres
    /// 17). Without one, the "majors ambiguous" refusal applies only to a
    /// first install with no config (Sew 2026-11-01).
    FromConfig,
}

/// Sew SW5 `InstallInputs` (shared by `install` and `install reconfigure`;
/// `pg` says whether the major is named).
fn install_inputs(provider: &str, jwks_url: Option<&str>, daemon_uid: u32, pg: PgMajorInput) -> serde_json::Value {
    let mut v = serde_json::json!({
        "provider": provider,
        "profile": "shared",
        "mgmt_allow_uids": [daemon_uid],
    });
    match pg {
        PgMajorInput::Pinned => v["pg_major"] = serde_json::json!(SEW_PG_MAJOR),
        PgMajorInput::FromConfig => {}
    }
    if let Some(u) = jwks_url {
        v["k2_jwks_url"] = serde_json::json!(u);
    }
    v
}

/// `engine sew install reconfigure` stdin (protocol 11, 0.46.6): what
/// `install` takes minus the master key (the install's keys stay). The
/// helper adds the daemon's uid, as for `install`, but never `pg_major`:
/// the install's own config keeps its Postgres major.
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

    /// No `pg_major` key: Sew's config on the box keeps its major
    /// ([`PgMajorInput::FromConfig`]).
    pub fn sew_inputs(&self, daemon_uid: u32) -> serde_json::Value {
        install_inputs(self.provider(), self.jwks_url.as_deref(), daemon_uid, PgMajorInput::FromConfig)
    }
}

/// The Sew release whose installs `install reconfigure` brings forward:
/// K2's upgrade flow runs it after an upgrade FROM a version below this
/// (Sew 2026-10-27: after an upgrade from 0.3.0).
pub const SEW_RECONFIGURE_BELOW: &str = "0.3.1";

/// `engine sew config` stdin: the keys `sew config set` accepts, each
/// optional, each validated. Applied in a fixed order. Protocol 15
/// (0.46.11, `namespaces.sewrecording`) adds
/// `recording.consent_prompt_timeout_ms` (a 14 helper refuses it as unknown).
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
    /// Sew 0.9.0 `[recording] consent_prompt_timeout_ms`: how long a person
    /// still `pending` on a recording notice stays in a room where declining
    /// means not joining (Sew then drops them, `recording_consent_timeout`).
    /// Whole milliseconds, [`CONSENT_TIMEOUT_MIN_MS`]..=[`CONSENT_TIMEOUT_MAX_MS`].
    #[serde(rename = "recording.consent_prompt_timeout_ms", default, skip_serializing_if = "Option::is_none")]
    pub consent_prompt_timeout_ms: Option<String>,
}

/// The shortest and longest `recording.consent_prompt_timeout_ms` K2 sets
/// (Sew's default is 120000).
pub const CONSENT_TIMEOUT_MIN_MS: u64 = 10_000;
pub const CONSENT_TIMEOUT_MAX_MS: u64 = 600_000;

/// A `recording.consent_prompt_timeout_ms` value: decimal, in range.
pub fn consent_timeout_ok(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 7
        && v.bytes().all(|b| b.is_ascii_digit())
        && v.parse::<u64>().is_ok_and(|n| (CONSENT_TIMEOUT_MIN_MS..=CONSENT_TIMEOUT_MAX_MS).contains(&n))
}

impl ConfigRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.jwks_url.is_none()
            && self.public_url.is_none()
            && self.mirror_kind.is_none()
            && self.mirror_path.is_none()
            && self.consent_prompt_timeout_ms.is_none()
        {
            return Err("config refused: no keys");
        }
        if self.consent_prompt_timeout_ms.as_deref().is_some_and(|v| !consent_timeout_ok(v)) {
            return Err("config refused: recording.consent_prompt_timeout_ms");
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
        if let Some(x) = &self.consent_prompt_timeout_ms {
            v.push(("recording.consent_prompt_timeout_ms", x.as_str()));
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
    /// Protocol 12 (0.46.8, §6.6 M9; an 11 helper refuses the key): Sew's
    /// `--relay-only` (`voice.ops.
    /// ice_relay_only`): `true` with the tunnel tier, `false` in direct mode.
    /// Sew refuses relay-only with TURN tier `plain` or `off` (exit 78), so
    /// `true` is accepted only with `turn_tls: passthrough` in the same
    /// request. With it Sew writes `node_ip = 127.0.0.1` itself (K2 sends no
    /// `node_ip`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_only: Option<bool>,
    /// Protocol 12: Sew's `--bind-v4`: [`MEDIA_BIND_LOOPBACK`] in the tunnel
    /// tier (UDP 7882 / TCP 7881 stay off the network), [`MEDIA_BIND_ANY`]
    /// back in direct mode. Never empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_v4: Option<String>,
    /// Protocol 12, Sew 0.7.1 on: Sew's `--turn-trusted-cidr`, the sources
    /// whose PROXY header the TURN listener trusts. Only ever
    /// [`TUNNEL_TRUSTED_CIDRS`] (the helper refuses any other list, so the
    /// daemon can't widen who may claim a client address), and only with
    /// `turn_tls: passthrough` in the same request. The daemon sends it to a
    /// Sew at [`TRUSTED_CIDRS_MIN_SEW`] or later: an older Sew refuses the key
    /// (exit 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_trusted_cidrs: Option<Vec<String>>,
    /// Protocol 14 (0.46.10, `namespaces.sewtunnelclear`; a 13 helper refuses
    /// the key): Sew's `--bind-v6`, sent on a move to the tunnel tier to clear
    /// direct mode's `"::"`. Only ever `""` (IPv6 off), and only with
    /// `relay_only: true` and `turn_tls: passthrough` in the same request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_v6: Option<String>,
    /// Protocol 14: clear the node IP a direct-mode install left in Sew's
    /// media settings. Sew 0.8.2 refuses `"node_ip": ""` in the JSON (and
    /// `null` keeps the old value), so the helper says it on argv instead,
    /// next to the JSON: `--node-ip ''` (an empty list: no node IP). Never in
    /// Sew's JSON ([`MediaRequest::sew_inputs`] drops it). Only `true`, never
    /// with `node_ip`, and only with `relay_only: true` and `turn_tls:
    /// passthrough` (the tunnel tier, where a node IP other than loopback is
    /// a config error: `media.net.node_ip: … is a local address but the
    /// socket is bound to 127.0.0.1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_node_ip: Option<bool>,
}

/// The tunnel tier's PROXY sources: frpc-voice dials sew-media from loopback.
pub const TUNNEL_TRUSTED_CIDRS: [&str; 2] = ["127.0.0.1/32", "::1/128"];
/// The first Sew whose `media install` takes `turn_trusted_cidrs`.
pub const TRUSTED_CIDRS_MIN_SEW: &str = "0.7.1";

/// Does a Sew reporting `version` (`0.7.1`, or with a `+g…` build suffix) take
/// `turn_trusted_cidrs`? Anything K2 can't read as a version is "no": the key
/// is what fails an install, and Sew's own default is loopback.
pub fn sew_takes_trusted_cidrs(version: &str) -> bool {
    let plain = version.split('+').next().unwrap_or(version);
    matches!(version_cmp(plain, TRUSTED_CIDRS_MIN_SEW), Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater))
}

/// sew-media's media sockets on loopback only (the tunnel tier).
pub const MEDIA_BIND_LOOPBACK: &str = "127.0.0.1";
/// sew-media's media sockets on every interface (Sew's default; direct).
pub const MEDIA_BIND_ANY: &str = "0.0.0.0";
/// Sew's exit code when `sew-media --check-config` refuses the media
/// settings (EX_CONFIG): relay-only with TURN tier `plain` or `off`, a node
/// IP the loopback-bound tunnel tier can't use, a hand-set IPv6 bind… Sew
/// 0.8.2 gives no machine-readable reason: its last stderr line
/// (`sew-media: config: <key>: <why>` or `sew: config: …`) says which.
pub const SEW_EXIT_CONFIG_REFUSED: i64 = 78;
/// The old name of [`SEW_EXIT_CONFIG_REFUSED`] (it is not only relay-only).
pub const SEW_EXIT_RELAY_ONLY_REFUSED: i64 = SEW_EXIT_CONFIG_REFUSED;

/// What `sew media install --config-json` accepts: Sew's own
/// `MediaInstallInputs` as K2 knows it (sew-proto at v0.7.1),
/// `deny_unknown_fields` like Sew's. An unknown key, a wrong type or a bad value
/// fails the install with exit 2. `turn_trusted_cidrs` is new in 0.7.1: an older
/// Sew refuses it ([`sew_takes_trusted_cidrs`]).
/// K2 never parses this in production: the test Sews do, so K2's tests fail
/// on a key real Sew would refuse.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SewMediaInstallInputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tarball: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stun_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_private_node_ip: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_v4: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_v6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_tls: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_advertise_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_proxy_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn_plain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cred_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysctl: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn_trusted_cidrs: Vec<String>,
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
        // Sew refuses relay-only with tier plain/off (exit 78): K2 sends it
        // only together with the passthrough tier it rides.
        if self.relay_only == Some(true) && self.turn_tls.as_deref() != Some("passthrough") {
            return Err("media refused: relay_only needs turn_tls passthrough");
        }
        if self.bind_v4.as_deref().is_some_and(|b| b != MEDIA_BIND_LOOPBACK && b != MEDIA_BIND_ANY) {
            return Err("media refused: bind_v4");
        }
        if let Some(c) = &self.turn_trusted_cidrs {
            if c.iter().map(String::as_str).ne(TUNNEL_TRUSTED_CIDRS) {
                return Err("media refused: turn_trusted_cidrs");
            }
            if self.turn_tls.as_deref() != Some("passthrough") {
                return Err("media refused: turn_trusted_cidrs needs turn_tls passthrough");
            }
        }
        // Protocol 14: direct mode's leftovers are cleared only on a move to
        // the tunnel tier (relay-only passthrough), and only to "none".
        let tunnel = self.relay_only == Some(true) && self.turn_tls.as_deref() == Some("passthrough");
        if let Some(b) = &self.bind_v6 {
            if !b.is_empty() {
                return Err("media refused: bind_v6");
            }
            if !tunnel {
                return Err("media refused: bind_v6 needs relay_only with turn_tls passthrough");
            }
        }
        if let Some(c) = self.clear_node_ip {
            if !c {
                return Err("media refused: clear_node_ip");
            }
            if self.node_ip.is_some() {
                return Err("media refused: clear_node_ip with node_ip");
            }
            if !tunnel {
                return Err("media refused: clear_node_ip needs relay_only with turn_tls passthrough");
            }
        }
        Ok(())
    }

    /// The request carries a protocol-12 key (the voice tunnel's).
    pub fn needs_tunnel_keys(&self) -> bool {
        self.relay_only.is_some() || self.bind_v4.is_some() || self.turn_trusted_cidrs.is_some()
    }

    /// The request carries a protocol-14 key (a tunnel move's clears of
    /// direct mode's leftovers).
    pub fn needs_tunnel_clears(&self) -> bool {
        self.bind_v6.is_some() || self.clear_node_ip.is_some()
    }

    /// `sew media install …` for this request, around the staged
    /// `--config-json` path (the helper appends its own `--json`): install,
    /// or `--reconfigure` (with `--restart --now` when asked), then
    /// `--node-ip ''` when the request clears the node IP (Sew 0.8.2: the
    /// flag beside the JSON, never the key in it).
    pub fn sew_argv(&self, reconfigure: bool, restart_now: bool, config_json: &str) -> Vec<String> {
        let mut a: Vec<String> = vec!["media".into(), "install".into()];
        if reconfigure {
            a.push("--reconfigure".into());
            if restart_now {
                a.extend(["--restart".into(), "--now".into()]);
            }
        }
        a.extend(["--config-json".into(), config_json.into()]);
        if self.clear_node_ip == Some(true) {
            a.extend(["--node-ip".into(), String::new()]);
        }
        a
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
            // Said on argv ([`MediaRequest::sew_argv`]): Sew 0.8.2 refuses
            // the flag together with a `node_ip` key.
            m.remove("clear_node_ip");
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

/// `engine sew mirror-dir` stdin (protocol 12, SW-8): where Sew's keyring
/// mirror folder goes. The helper creates `<ship_path>/sew-keyring-<install_id>`
/// (`sew:sew`, 0700) under an existing ship folder, never through a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MirrorDirRequest {
    /// The backup (ship) folder: [`mirror_path_ok`], and it must exist.
    pub ship_path: String,
    /// Sew's install id ([`install_id_ok`], never `none`).
    pub install_id: String,
}

impl MirrorDirRequest {
    /// The request for a mirror folder path `<ship>/sew-keyring-<id>` (what
    /// the daemon's keyring-mirror tie holds). `None` for any other shape.
    pub fn from_mirror_path(path: &str) -> Option<Self> {
        let (ship, name) = path.rsplit_once('/')?;
        let id = name.strip_prefix("sew-keyring-")?;
        Some(Self { ship_path: ship.to_string(), install_id: id.to_string() })
    }

    /// `sew-keyring-<install id>`.
    pub fn dir_name(&self) -> String {
        format!("sew-keyring-{}", self.install_id)
    }

    /// The folder the helper creates (and the daemon then names as
    /// `keys.mirror.path`).
    pub fn mirror_path(&self) -> String {
        format!("{}/{}", self.ship_path, self.dir_name())
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.install_id == "none" || !install_id_ok(&self.install_id) {
            return Err("mirror-dir refused: installId");
        }
        if self.ship_path == "/" || !mirror_path_ok(&self.ship_path) || !mirror_path_ok(&self.mirror_path()) {
            return Err("mirror-dir refused: shipPath");
        }
        Ok(())
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
            assert_eq!(app_kit("0.6.0").map(|k| k.sha256), Some("9f8602281a3029684fd9d12665242b2555d8a86c2e1975621e8282933076bc43"));
            assert_eq!(app_kit("0.6.1").map(|k| k.sha256), Some("955357393d869b950fa6401462c3d42453cca4f32e84be7b16ca341b4ba20b35"));
            assert_eq!(app_kit("0.7.1").map(|k| k.sha256), Some("8add54188d362927c14dce6f01710b3ac1ad0956780ca1dd77f8d5fcc9e2a192"));
            assert_eq!(app_kit("0.7.2").map(|k| k.sha256), Some("5867fe54ff1b6732578b1d61d5890ab474f4337bdd636a31bb1f3b3699d9c7a1"));
            assert_eq!(app_kit("0.7.3").map(|k| k.sha256), Some("585ab6f099c15d17abb2b8ac6e9664c11e8417d9470ce63d94b116c9adca53c0"));
            assert_eq!(app_kit("0.8.0").map(|k| k.sha256), Some("0a063ddfd80f5bdc19217f510c018372059ddea55b074e066af4f30cac47f4e9"));
            assert_eq!(app_kit("0.8.1").map(|k| k.sha256), Some("c1be5c5fd0c1725c3353a4f091b446857ed8c474754a0a33ea2023c0e505ac56"));
            assert_eq!(app_kit("0.8.2").map(|k| k.sha256), Some("d15e575379256381c1ffc0303a3d3adaf21307ed36d8ea285fc32945baa5e80e"));
            assert_eq!(app_kit("0.8.3").map(|k| k.sha256), Some("406bdc38d6d10b2efcc22dbca1e6f42850fd889150c08a0820bc9fa04b376d3d"));
            assert_eq!(app_kit("0.9.0").map(|k| k.sha256), Some("a3bd4d5dba566c5f9f8a30ad27a34bc716128fc29cf7944f992c41c8961b6c2c"));
            assert_eq!(app_kit("0.9.2").map(|k| k.sha256), Some("59f9ae96dcc9a54597daeba5f3b73fe3fc6f720a7acc7e0cb1e64caa45a34a20"));
            if cfg!(feature = "sew-test-pin-next") {
                // The test pin's kit counts once its hash is dropped in.
                assert_eq!(pinned().version, SEW_TEST_NEXT.version);
                assert_eq!(app_kit(pinned().version).is_some(), hex64(SEW_TEST_NEXT_APP_KIT.sha256));
            } else if cfg!(feature = "sew-test-pins") {
                assert!(app_kit(pinned().version).is_none(), "Sew's 0.1.x test builds have no App kit");
            } else {
                assert!(app_kit(pinned().version).is_some(), "the pin has its kit");
            }
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
        assert_eq!(pin_equals_sums("0.5.0", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.5.0").map(|k| k.sha256),
            Some("64efdf1d822be2a4c49ccab437be8bddd5068c6e906b968b94beb44d8b1a0418")
        );
    }

    /// The 0.6.0 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.6.0 release, sha256 31634a5c…100f): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_6_0_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.6.0.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "31634a5c7bdd80e3c1db7a1745dd437e12afae6708944626846ea72bf639100f",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.6.0", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.6.0").map(|k| k.sha256),
            Some("9f8602281a3029684fd9d12665242b2555d8a86c2e1975621e8282933076bc43")
        );
    }

    /// The 0.6.1 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.6.1 release, sha256 bbf15330…889d): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_6_1_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.6.1.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "bbf1533046a7672ae48c6798f947896fe5f3ea224a9b450a43fdc5e7b52f889d",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.6.1", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.6.1").map(|k| k.sha256),
            Some("955357393d869b950fa6401462c3d42453cca4f32e84be7b16ca341b4ba20b35")
        );
    }

    /// The 0.7.1 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.7.1 release, sha256 1a24c3b5…457a): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_7_1_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.7.1.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "1a24c3b55ff43370cd2109d0ab1f2297b20518030142e0a672c1bc2e920e457a",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.7.1", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.7.1").map(|k| k.sha256),
            Some("8add54188d362927c14dce6f01710b3ac1ad0956780ca1dd77f8d5fcc9e2a192")
        );
    }

    /// A rollback has the old core reinstall its own voice engine, so every real
    /// release a box can go back to pins that engine for both arches, under a
    /// path the helper can keep it at (Sew's 0.6.0 → 0.7.2 note, 11-04).
    #[test]
    fn every_rollback_target_pins_its_voice_engine() {
        for r in &SEW_RELEASES[1..] {
            let m = r.media.as_ref().unwrap_or_else(|| panic!("Sew {} pins no sew-media", r.version));
            for arch in ["x86_64", "aarch64"] {
                let a = m.artifacts.iter().find(|a| a.arch == arch).unwrap_or_else(|| panic!("Sew {} sew-media on {arch}", r.version));
                assert!(hex64(a.tarball_sha256), "{}: {a:?}", r.version);
            }
            assert!(kept_media_path(m.version).is_some(), "{}", r.version);
        }
    }

    /// The 0.7.2 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.7.2 release, sha256 8717a934…59b7): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_7_2_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.7.2.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "8717a9340006c3658001e07f1930e4fe1521593210521c57d728e8993c1159b7",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.7.2", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.7.2").map(|k| k.sha256),
            Some("5867fe54ff1b6732578b1d61d5890ab474f4337bdd636a31bb1f3b3699d9c7a1")
        );
    }

    /// The 0.7.3 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.7.3 release, sha256 9e03b235…a09e): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_7_3_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.7.3.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "9e03b2351babeaae025df96d386d93c361f8a541085e1e919aa3a63199d3a09e",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.7.3", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.7.3").map(|k| k.sha256),
            Some("585ab6f099c15d17abb2b8ac6e9664c11e8417d9470ce63d94b116c9adca53c0")
        );
    }

    /// The 0.8.0 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.8.0 release, sha256 a0abc64b…4687): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_8_0_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.8.0.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "a0abc64b24b3287cb9095875b6da267aea848ff4658ecc491296f7a627464687",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.8.0", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.8.0").map(|k| k.sha256),
            Some("0a063ddfd80f5bdc19217f510c018372059ddea55b074e066af4f30cac47f4e9")
        );
    }

    /// The 0.8.1 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.8.1 release, sha256 07c3d1e8…142c): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_8_1_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.8.1.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "07c3d1e8f962559985141b1dd0ed56b749084c8226e84980e30fe2e5138d142c",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.8.1", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.8.1").map(|k| k.sha256),
            Some("c1be5c5fd0c1725c3353a4f091b446857ed8c474754a0a33ea2023c0e505ac56")
        );
    }

    /// The 0.8.2 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.8.2 release, sha256 150f62fd…7749): every core and sidecar
    /// tarball and binary member line, plus the App kit's.
    #[test]
    fn the_0_8_2_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.8.2.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "150f62fdfc2f1d199779d9acb2d8162c1a0b6b9c1235770bc79e1f66a4607749",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.8.2", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.8.2").map(|k| k.sha256),
            Some("d15e575379256381c1ffc0303a3d3adaf21307ed36d8ea285fc32945baa5e80e")
        );
    }

    /// 0.8.3 (never the pin; a known release a box may run) is exactly the
    /// published SHA256SUMS (vendored from the v0.8.3 release, sha256
    /// a484eaea…df3c): every core and sidecar tarball and binary member
    /// line, plus the App kit's.
    #[test]
    fn the_0_8_3_entry_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.8.3.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "a484eaead3a7929640f6150d6a1ee38b3eadd58104fec10c922a4aef5cbcdf3c",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.8.3", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.8.3").map(|k| k.sha256),
            Some("406bdc38d6d10b2efcc22dbca1e6f42850fd889150c08a0820bc9fa04b376d3d")
        );
    }

    /// 0.9.0 (the pin before 0.9.2; an upgrade source and rollback target)
    /// is exactly the published SHA256SUMS (vendored from the v0.9.0
    /// release, sha256 7aed9b64…d8f5): every core and sidecar tarball and
    /// binary member line, plus the App kit's. Each hash in Sew's release
    /// note is checked here too, so a slip in either the table or the
    /// vendored file fails.
    #[test]
    fn the_0_9_0_entry_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.9.0.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "7aed9b64320e982d4807e0ea85dadbfea12405ab6f03ec5a6f1d66359ecdd8f5",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(pin_equals_sums("0.9.0", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        // Sew's note (sew-0.9.0-release-pins-2026-11-10): the tarballs and the kit.
        let pin = SEW_RELEASES.iter().find(|r| r.version == "0.9.0").expect("0.9.0 is known");
        let tar = |side: Option<&SewSidecar>, arch: &str| {
            let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
            arts.iter().find(|a| a.arch == arch).map(|a| a.tarball_sha256).expect("pinned")
        };
        for (side, x86, arm) in [
            (None, "761c432452baf5a488c3a319f96b40e05e88f762eeb8d122e29f6a2675f033d4", "dc31ab060d142b166b455ff392303c49fe41bec33db7dab229fe02d6a61b7a1d"),
            (pin.media.as_ref(), "1c85071b731173682d9da1560d9647c28ae4425adbc8bcd0035cd5eafdeccdf0", "a53812cbf8924e5fca72c5ef2ec95b1742a534fcd15b765e6c62e9409c403aaf"),
            (pin.recorder.as_ref(), "4238b82d9ce62fc566107e80f3fa9b5715bd4dc04b818dca51c61c9b55b1aa98", "8f51029e79b71a89b2f5da00b0eeee1a6e18d1e33158276a4bc41c0eccfb119f"),
            (pin.transcriber.as_ref(), "41e7c22790b35974c3924499d290bce729dbf08bea40780f336cb0e32b53968d", "a5c05a3d670c196843395540100b634a4b7fee0c5bc615845a65ca263d4c2e5f"),
        ] {
            assert_eq!((tar(side, "x86_64"), tar(side, "aarch64")), (x86, arm), "{:?}", side.map(|s| s.version));
        }
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.9.0").map(|k| k.sha256),
            Some("a3bd4d5dba566c5f9f8a30ad27a34bc716128fc29cf7944f992c41c8961b6c2c")
        );
    }

    /// The 0.9.2 pin is exactly the published SHA256SUMS (vendored from the
    /// v0.9.2 release, sha256 8e9a63bb…3f25d): every core and sidecar
    /// tarball and binary member line, plus the App kit's. Each hash in
    /// Sew's pin note (2026-11-11) is checked here too, so a slip in either
    /// the table or the vendored file fails.
    #[test]
    fn the_0_9_2_pin_equals_the_published_sha256sums() {
        let sums = include_str!("../testdata/sew/sew-0.9.2.SHA256SUMS");
        assert_eq!(
            sha256_hex(sums.as_bytes()),
            "8e9a63bb00acb76d14eb91d5fe927bc2b247bd1d4ef40422a3ea7a2654a3f25d",
            "the vendored SUMS file is the published one"
        );
        assert_eq!(SEW_RELEASES.last().unwrap().version, "0.9.2", "0.9.2 is the pin");
        assert_eq!(pin_equals_sums("0.9.2", sums), 17, "4 binaries × 2 arches × (tarball, member) + the App kit");
        let pin = SEW_RELEASES.last().unwrap();
        let bin = |side: Option<&SewSidecar>, arch: &str| {
            let arts = side.map(|s| s.artifacts).unwrap_or(pin.artifacts);
            arts.iter().find(|a| a.arch == arch).map(|a| (a.tarball_sha256, a.binary_sha256)).expect("pinned")
        };
        assert_eq!(
            bin(None, "x86_64"),
            ("91985f336d747e4490bd26d56e733381492b11a4f8f7f1da81fbb440cb53dd8b", "f13d7c7cd3be3a114dceaf1b36ab0092735fd971471c53f68201a32b7cf0d1c6")
        );
        assert_eq!(
            bin(None, "aarch64"),
            ("fce76f3ef55ad53ea6ad3f32614551f806fd112f5aede8caee4be8aced2c2038", "9af08256897c1a279ab1919c78eb2bb12af1561246f197f58eea3602b59117fd")
        );
        for (side, x86, arm) in [
            (pin.media.as_ref(), "ee9da2230c133e563909ee248d7fff30400c1c71ccbcbea25e7ebabe042c50e1", "8fb823bbfe24836a218741d57516b1920bd151b76fb10117b4c7e973b16d9649"),
            (pin.transcriber.as_ref(), "94dbee0b56afdf4b0f199f1facd2fe356426398f6792d6829f9dc991dbf457d5", "53b0708c9ca1d4ec9363cadf1ffa33a04affff324b095957bcc1b5d3fbecea63"),
            (pin.recorder.as_ref(), "9e9354e76ecac2e9c3de54d219908ca8da3e8d63f10039eac5d6a8cb70278cf7", "ecf082ea188df04a6bbef4808eaf24e62afa64abd4d82d2747a7a3aec9d71ee0"),
        ] {
            assert_eq!((bin(side, "x86_64").0, bin(side, "aarch64").0), (x86, arm), "{:?}", side.map(|s| s.version));
        }
        assert_eq!(
            SEW_APP_KITS.iter().find(|k| k.version == "0.9.2").map(|k| k.sha256),
            Some("59f9ae96dcc9a54597daeba5f3b73fe3fc6f720a7acc7e0cb1e64caa45a34a20")
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

    /// A box moved to Sew 0.8.3 by hand (never K2's pin) upgrades to the
    /// 0.9.0 pin as a plain release: its published binary (the sha256 its
    /// own `sew version` printed on a compute node, 37c7d41d… on x86_64) is
    /// a known release, not an accept-binary case; it is older than the pin;
    /// and a rollback onto it has its own voice engine to put back. A binary
    /// that is NOT the published 0.8.3 stays unknown (the owner's
    /// `--accept-binary` path, helper item 12).
    #[cfg(not(any(feature = "test-signer", feature = "sew-test-pins")))]
    #[test]
    fn a_box_on_sew_0_8_3_upgrades_to_the_0_9_0_pin_as_a_known_release() {
        let x86 = "37c7d41d371eff99eb943a76ffe10140c398310fd087d1ac34bf6ac532dfc277";
        assert_eq!(known_binary(x86, "x86_64"), Some("0.8.3"));
        assert_eq!(known_binary(x86, "aarch64"), None, "another arch's binary is not this arch's release");
        assert!(upgrade_only_binary(x86, "x86_64").is_none(), "a release, never an upgrade-only build");
        let r = release("0.8.3").expect("0.8.3 is known");
        for a in r.artifacts {
            assert_eq!(known_binary(a.binary_sha256, a.arch), Some("0.8.3"), "{}", a.arch);
        }
        assert_eq!(pinned().version, "0.9.2");
        assert_eq!(version_cmp("0.8.3", pinned().version), Some(std::cmp::Ordering::Less), "an upgrade, not a downgrade");
        assert_eq!(version_cmp("0.8.2", "0.8.3"), Some(std::cmp::Ordering::Less), "0.8.3 sits between 0.8.2 and the pin");
        for arch in ["x86_64", "aarch64"] {
            let (mv, m) = media_artifact_of("0.8.3", arch).expect("0.8.3's engine");
            assert_eq!(mv, "0.8.3");
            assert_eq!(known_media_tarball(m.tarball_sha256, arch), Some("0.8.3"), "kept for a rollback to 0.8.3");
        }
        // A hand-built 0.8.3 (any other sha) is no release K2 knows.
        assert_eq!(known_binary(&"ab".repeat(32), "x86_64"), None);
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
    /// and 0.2.1 through 0.9.0 stay known; the pin is Sew 0.9.2 for both
    /// linux-gnu triples, with all three sidecars, exactly as
    /// `scripts/sew-pins-from-release.sh` printed it from the verified
    /// release (2026-11-11).
    #[test]
    fn real_table_pins_sew_0_9_2_and_keeps_the_old_entries() {
        assert_eq!(SEW_RELEASES.len(), 17);
        assert_eq!(SEW_RELEASES[15].version, "0.9.0");
        assert!(release_is_pinned(&SEW_RELEASES[15]), "0.9.0 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[14].version, "0.8.3");
        assert!(release_is_pinned(&SEW_RELEASES[14]), "0.8.3 is a real release (an upgrade source and rollback target)");
        assert_eq!(SEW_RELEASES[13].version, "0.8.2");
        assert!(release_is_pinned(&SEW_RELEASES[13]), "0.8.2 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[12].version, "0.8.1");
        assert!(release_is_pinned(&SEW_RELEASES[12]), "0.8.1 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[11].version, "0.8.0");
        assert!(release_is_pinned(&SEW_RELEASES[11]), "0.8.0 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[10].version, "0.7.3");
        assert!(release_is_pinned(&SEW_RELEASES[10]), "0.7.3 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[9].version, "0.7.2");
        assert!(release_is_pinned(&SEW_RELEASES[9]), "0.7.2 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[8].version, "0.7.1");
        assert!(release_is_pinned(&SEW_RELEASES[8]), "0.7.1 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[7].version, "0.6.1");
        assert!(release_is_pinned(&SEW_RELEASES[7]), "0.6.1 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[6].version, "0.6.0");
        assert!(release_is_pinned(&SEW_RELEASES[6]), "0.6.0 stays a real release (the upgrade source)");
        assert_eq!(SEW_RELEASES[5].version, "0.5.0");
        assert!(release_is_pinned(&SEW_RELEASES[5]), "0.5.0 stays a real release (the upgrade source)");
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
        assert_eq!(pin.version, "0.9.2");
        assert!(release_is_pinned(pin));
        assert!(artifact_in(pin, "riscv64").unwrap_err().contains("unsupported"));
        let want = [
            ("x86_64", "91985f336d747e4490bd26d56e733381492b11a4f8f7f1da81fbb440cb53dd8b", "f13d7c7cd3be3a114dceaf1b36ab0092735fd971471c53f68201a32b7cf0d1c6"),
            ("aarch64", "fce76f3ef55ad53ea6ad3f32614551f806fd112f5aede8caee4be8aced2c2038", "9af08256897c1a279ab1919c78eb2bb12af1561246f197f58eea3602b59117fd"),
        ];
        for (arch, tar, bin) in want {
            let a = artifact_in(pin, arch).expect("pinned");
            assert_eq!((a.tarball_sha256, a.binary_sha256), (tar, bin), "{arch}");
            assert_eq!(a.triple, format!("{arch}-unknown-linux-gnu"));
        }
        for (name, s) in [("media", &pin.media), ("transcriber", &pin.transcriber), ("recorder", &pin.recorder)] {
            let s = s.as_ref().unwrap_or_else(|| panic!("{name} pinned"));
            assert_eq!(s.version, "0.9.2", "{name}");
            let mut arches: Vec<&str> = s.artifacts.iter().map(|a| a.arch).collect();
            arches.sort();
            assert_eq!(arches, ["aarch64", "x86_64"], "{name}");
        }
        for a in all_artifacts(pin) {
            assert!(!checksum_is_placeholder(a), "{a:?}");
        }
        // Every hash is distinct (no copy-paste slip between rows), across
        // all sixteen real releases.
        let mut seen = std::collections::HashSet::new();
        for r in &SEW_RELEASES[1..] {
            for a in all_artifacts(r) {
                assert!(seen.insert(a.tarball_sha256) && seen.insert(a.binary_sha256), "{a:?}");
            }
        }
        assert_eq!(seen.len(), 256);
    }

    #[test]
    fn release_pinned_follows_the_last_entry() {
        assert!(release_is_pinned(SEW_RELEASES.last().unwrap()), "0.9.2 is pinned (published 2026-11-11)");
        assert!(release_is_pinned(TEST_RELEASES.last().unwrap()));
        if cfg!(feature = "sew-test-pin-next") && !cfg!(feature = "test-signer") {
            // The test pin until its hashes are dropped in pins nothing.
            assert_eq!(sew_release_pinned(), release_is_pinned(&SEW_TEST_NEXT));
        } else {
            assert!(sew_release_pinned(), "every build pins a release now");
        }
        assert_eq!(SEW_NOT_RELEASED, "sew_not_released");
    }

    #[cfg(all(feature = "sew-test-pins", not(feature = "sew-test-pin-next"), not(feature = "test-signer")))]
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
            assert_eq!(pinned().version, "0.9.2");
            for a in pinned().artifacts {
                assert_eq!(pinned_artifact(a.arch).expect("pinned").binary_sha256, a.binary_sha256);
                assert_eq!(known_binary(a.binary_sha256, a.arch), Some("0.9.2"));
                // Another arch's binary is not this arch's release.
                let other = if a.arch == "x86_64" { "aarch64" } else { "x86_64" };
                assert_eq!(known_binary(a.binary_sha256, other), None);
                assert_eq!(known_binary(a.tarball_sha256, a.arch), None, "a tarball sha is never a binary sha");
            }
            assert_eq!(pinned_media_artifact("x86_64").expect("media pinned").0, "0.9.2");
            // 0.2.0 through 0.9.0 on a box are still recognised (upgrade sources).
            for (r, v) in [
                (&SEW_RELEASES[1], "0.2.0"),
                (&SEW_RELEASES[2], "0.2.1"),
                (&SEW_RELEASES[3], "0.3.0"),
                (&SEW_RELEASES[4], "0.3.1"),
                (&SEW_RELEASES[5], "0.5.0"),
                (&SEW_RELEASES[6], "0.6.0"),
                (&SEW_RELEASES[7], "0.6.1"),
                (&SEW_RELEASES[8], "0.7.1"),
                (&SEW_RELEASES[9], "0.7.2"),
                (&SEW_RELEASES[10], "0.7.3"),
                (&SEW_RELEASES[11], "0.8.0"),
                (&SEW_RELEASES[12], "0.8.1"),
                (&SEW_RELEASES[13], "0.8.2"),
                (&SEW_RELEASES[14], "0.8.3"),
                (&SEW_RELEASES[15], "0.9.0"),
            ] {
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
        assert!(releases().iter().all(|r| r.version != SEW_TEST_NEXT.version), "no test entry in a default build");
        assert!(app_kits().iter().all(|k| k.version != SEW_TEST_NEXT_APP_KIT.version));
        for r in releases() {
            for a in r.artifacts {
                assert_ne!(a.binary_sha256, "4dfe51382507e748830a0e008fb96a19cecc7c6d2adb9063c0b2dc533f669837");
                assert_ne!(a.binary_sha256, "35d334be98c5610d45f72c3d19e82eaca02ea3b17ecc37910532f26f9f3f0ade");
            }
        }
    }

    /// The next test build is no real release: a newer, plain `x.y.z`
    /// version above the real pin, both linux-gnu arches, and not in the
    /// real table (a default build has no test entry: see
    /// `default_build_has_no_sew_test_pins` and `binary.rs`).
    #[test]
    fn the_next_test_build_sits_above_the_real_pin_and_outside_the_real_table() {
        let real_pin = SEW_RELEASES.last().unwrap();
        assert!(crate::version::Semver::parse(SEW_TEST_NEXT.version).is_some(), "plain x.y.z: {}", SEW_TEST_NEXT.version);
        assert_eq!(version_cmp(real_pin.version, SEW_TEST_NEXT.version), Some(std::cmp::Ordering::Less));
        assert!(SEW_RELEASES.iter().all(|r| r.version != SEW_TEST_NEXT.version), "the test build is not a real release");
        assert!(SEW_APP_KITS.iter().all(|k| k.version != SEW_TEST_NEXT.version));
        assert_eq!(SEW_TEST_NEXT_APP_KIT.version, SEW_TEST_NEXT.version);
        let mut arches: Vec<&str> = SEW_TEST_NEXT.artifacts.iter().map(|a| a.arch).collect();
        arches.sort();
        assert_eq!(arches, ["aarch64", "x86_64"], "the SYS-LE-1 test boxes are x86_64");
        for a in SEW_TEST_NEXT.artifacts {
            assert_eq!(a.triple, format!("{}-unknown-linux-gnu", a.arch));
        }
        // Either all placeholders (not yet) or all real: never half a pin.
        let all = all_artifacts(&SEW_TEST_NEXT);
        let real = all.iter().filter(|a| !checksum_is_placeholder(a)).count();
        assert!(real == 0 || real == all.len(), "{all:?}");
        for a in &all {
            for h in [a.tarball_sha256, a.binary_sha256] {
                if hex64(h) {
                    for r in SEW_RELEASES {
                        for b in all_artifacts(r) {
                            assert!(b.tarball_sha256 != h && b.binary_sha256 != h, "{h} is a real release's hash");
                        }
                    }
                }
            }
        }
    }

    /// `sew-test-pin-next` (V22): the table is the real table, entry for
    /// entry, plus [`SEW_TEST_NEXT`] as the pin, so a box on the real pin
    /// upgrades to the test build: the real pin's binary is still a known
    /// release (the upgrade source, and the rollback target), and it is
    /// older than the pin (the helper's and the daemon's upgrade check).
    #[cfg(all(feature = "sew-test-pin-next", not(feature = "test-signer")))]
    #[test]
    fn sew_test_pin_next_is_the_real_table_plus_the_test_build() {
        assert_eq!(test_pins_flag(), Some("K2_SEW_TEST_PINS_ENABLED"), "signing refuses this build");
        let t = releases();
        assert_eq!(t.len(), SEW_RELEASES.len() + 1);
        for (got, want) in t.iter().zip(SEW_RELEASES) {
            assert_eq!(got.version, want.version);
            let (g, w) = (all_artifacts(got), all_artifacts(want));
            assert_eq!(g.len(), w.len(), "{}", want.version);
            for (a, b) in g.iter().zip(&w) {
                assert_eq!((a.arch, a.tarball_sha256, a.binary_sha256), (b.arch, b.tarball_sha256, b.binary_sha256));
            }
        }
        assert_eq!(pinned().version, SEW_TEST_NEXT.version, "the test build is the pin");
        let real_pin = SEW_RELEASES.last().unwrap();
        assert_eq!(release(real_pin.version).map(|r| r.version), Some(real_pin.version));
        // The upgrade: from the real pin on a box to the test pin.
        assert_eq!(version_cmp(real_pin.version, pinned().version), Some(std::cmp::Ordering::Less), "an upgrade, not a downgrade");
        for a in real_pin.artifacts {
            assert_eq!(known_binary(a.binary_sha256, a.arch), Some(real_pin.version), "the real pin is a known upgrade source");
        }
        // The rollback target keeps its voice engine.
        assert_eq!(media_artifact_of(real_pin.version, "x86_64").unwrap().0, real_pin.media.as_ref().unwrap().version);
        // Every real release is still known on a box.
        for r in &SEW_RELEASES[1..] {
            for a in r.artifacts {
                assert_eq!(known_binary(a.binary_sha256, a.arch), Some(r.version));
            }
        }
        // The kits: every real one plus the test build's.
        assert_eq!(app_kits().len(), SEW_APP_KITS.len() + 1);
        assert_eq!(app_kit(real_pin.version).map(|k| k.sha256), SEW_APP_KITS.last().map(|k| k.sha256));
        if release_is_pinned(&SEW_TEST_NEXT) {
            assert!(sew_release_pinned());
            for arch in ["x86_64", "aarch64"] {
                let a = pinned_artifact(arch).expect("the test build is pinned");
                assert_eq!(known_binary(a.binary_sha256, arch), Some(SEW_TEST_NEXT.version));
            }
        } else {
            // Placeholder (Sew's test build not out yet): nothing installs.
            assert!(!sew_release_pinned());
            for arch in ["x86_64", "aarch64"] {
                assert!(pinned_artifact(arch).unwrap_err().contains("no Sew pin"), "{arch}");
            }
            assert!(app_kit(SEW_TEST_NEXT.version).is_none());
        }
    }

    #[cfg(all(feature = "sew-test-pins", not(feature = "sew-test-pin-next"), not(feature = "test-signer")))]
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
        // The real pin (0.9.2) is never selected in a test-pins build.
        assert_ne!(pinned().version, "0.9.2");
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

    /// Sew 2026-11-01: `install --reconfigure` names no Postgres major (the
    /// install's `sew.toml` decides; a named 16 refuses a sew-db on 17),
    /// while a first install still pins [`SEW_PG_MAJOR`]. Both providers.
    #[test]
    fn reconfigure_inputs_name_no_pg_major_but_install_inputs_do() {
        let jwks = "http://127.0.0.1:38471/v1/chat/jwks";
        let rc: ReconfigureRequest = serde_json::from_str(&format!(r#"{{"jwksUrl":"{jwks}"}}"#)).unwrap();
        rc.validate().unwrap();
        let ri = rc.sew_inputs(1001);
        let keys: Vec<&str> = ri.as_object().expect("an object").keys().map(String::as_str).collect();
        assert!(!keys.contains(&"pg_major"), "reconfigure must not name a Postgres major: {ri}");
        assert_eq!(ri["provider"], "k2");
        assert_eq!(ri["profile"], "shared");
        assert_eq!(ri["k2_jwks_url"], jwks);
        assert_eq!(ri["mgmt_allow_uids"], serde_json::json!([1001]));
        let sa: ReconfigureRequest = serde_json::from_str(r#"{"provider":"standalone"}"#).unwrap();
        sa.validate().unwrap();
        let si = sa.sew_inputs(1001);
        assert!(si.get("pg_major").is_none(), "standalone reconfigure must not name a Postgres major: {si}");
        assert!(si.get("k2_jwks_url").is_none());
        // The same request as a first install pins the major.
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let ins: InstallRequest = serde_json::from_str(&format!(r#"{{"jwksUrl":"{jwks}","masterKey":"{key}"}}"#)).unwrap();
        let ii = ins.sew_inputs(1001);
        assert_eq!(ii["pg_major"], serde_json::json!(16), "{ii}");
        // Otherwise the two bodies are the same.
        let mut ii_minus = ii.clone();
        ii_minus.as_object_mut().unwrap().remove("pg_major");
        assert_eq!(ii_minus, ri);
        // A major on reconfigure stdin never parses (deny_unknown_fields).
        for bad in [
            format!(r#"{{"jwksUrl":"{jwks}","pg_major":17}}"#),
            format!(r#"{{"jwksUrl":"{jwks}","pgMajor":16}}"#),
        ] {
            assert!(serde_json::from_str::<ReconfigureRequest>(&bad).is_err(), "{bad}");
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
        // Protocol 15: the K5 rooms' consent timeout, whole ms in range.
        let c: ConfigRequest = serde_json::from_str(r#"{"recording.consent_prompt_timeout_ms":"60000"}"#).unwrap();
        assert!(c.validate().is_ok());
        assert_eq!(c.pairs(), vec![("recording.consent_prompt_timeout_ms", "60000")]);
        for bad in ["", "5000", "600001", "6e4", "-1", "60000 ", "060000x"] {
            let c = ConfigRequest { consent_prompt_timeout_ms: Some(bad.into()), ..Default::default() };
            assert!(c.validate().is_err(), "{bad:?}");
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

    /// Protocol 14 (0.46.10, Sew 0.8.2): a move to the tunnel tier clears
    /// direct mode's leftovers. `bind_v6: ""` and `turn_advertise_port: 0`
    /// reach Sew's JSON; the node IP is cleared on argv (`--node-ip ''`),
    /// never as a JSON key (Sew refuses `"node_ip": ""`, and the flag with
    /// the key).
    #[test]
    fn a_tunnel_move_clears_direct_modes_leftovers_on_argv_and_in_the_json() {
        let r: MediaRequest = serde_json::from_str(
            r#"{"turn_tls":"passthrough","turn_domain":"turn.box.k2.dev","relay_only":true,"bind_v4":"127.0.0.1",
                "turn_advertise_port":0,"bind_v6":"","clear_node_ip":true}"#,
        )
        .unwrap();
        r.validate_reconfigure().unwrap();
        assert!(r.needs_tunnel_keys() && r.needs_tunnel_clears());
        let inputs = r.sew_inputs(None, None);
        assert_eq!(
            inputs,
            serde_json::json!({
                "relay_only": true,
                "turn_tls": "passthrough",
                "turn_domain": "turn.box.k2.dev",
                "bind_v4": "127.0.0.1",
                "bind_v6": "",
                "turn_advertise_port": 0,
                "profile": "shared",
            }),
            "no node_ip and no clear_node_ip in Sew's JSON"
        );
        serde_json::from_value::<SewMediaInstallInputs>(inputs).expect("real Sew's inputs take it");
        assert_eq!(
            r.sew_argv(true, true, "/s/media.json"),
            ["media", "install", "--reconfigure", "--restart", "--now", "--config-json", "/s/media.json", "--node-ip", ""]
        );
        assert_eq!(r.sew_argv(false, false, "/s/media.json"), ["media", "install", "--config-json", "/s/media.json", "--node-ip", ""]);
        // Without the clear: the argv protocols 7–13 always sent.
        let plain = MediaRequest { clear_node_ip: None, ..r.clone() };
        assert_eq!(plain.sew_argv(true, false, "/s/m.json"), ["media", "install", "--reconfigure", "--config-json", "/s/m.json"]);
        // Only "none", only on the tunnel tier, never beside a node IP.
        for bad in [
            r#"{"turn_tls":"passthrough","relay_only":true,"bind_v6":"::"}"#,
            r#"{"turn_tls":"passthrough","relay_only":true,"bind_v6":"::1"}"#,
            r#"{"turn_tls":"passthrough","bind_v6":""}"#,
            r#"{"turn_tls":"direct","relay_only":false,"bind_v6":""}"#,
            r#"{"turn_tls":"passthrough","relay_only":true,"clear_node_ip":false}"#,
            r#"{"turn_tls":"passthrough","relay_only":true,"clear_node_ip":true,"node_ip":"203.0.113.7"}"#,
            r#"{"turn_tls":"passthrough","clear_node_ip":true}"#,
            r#"{"turn_tls":"direct","relay_only":false,"clear_node_ip":true}"#,
            r#"{"clear_node_ip":true}"#,
        ] {
            let m: MediaRequest = serde_json::from_str(bad).unwrap();
            assert!(m.validate_reconfigure().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<MediaRequest>(r#"{"clear_node_ip":"yes"}"#).is_err());
        // The way back to direct carries none of them.
        let back: MediaRequest =
            serde_json::from_str(r#"{"turn_tls":"direct","relay_only":false,"bind_v4":"0.0.0.0","node_ip":"203.0.113.7","turn_advertise_port":5349}"#)
                .unwrap();
        back.validate_reconfigure().unwrap();
        assert!(!back.needs_tunnel_clears());
        assert_eq!(SEW_EXIT_CONFIG_REFUSED, 78);
    }

    /// Protocol 12 (voice rooms §6.6 M5/M7/M9): the tunnel tier's request.
    #[test]
    fn tunnel_tier_keys_are_loopback_only_and_reach_sews_json() {
        let r: MediaRequest = serde_json::from_str(
            r#"{"turn_tls":"passthrough","turn_domain":"turn.box.k2.dev","relay_only":true,"bind_v4":"127.0.0.1"}"#,
        )
        .unwrap();
        r.validate_reconfigure().unwrap();
        assert!(r.needs_tunnel_keys());
        assert!(r.node_ip.is_none(), "the tunnel tier sends no node IP");
        // Exactly what Sew's `MediaInstallInputs` takes for the tunnel tier
        // (Sew, 11-02): an extra key fails the install with exit 2.
        let inputs = r.sew_inputs(None, None);
        assert_eq!(
            inputs,
            serde_json::json!({
                "relay_only": true,
                "turn_tls": "passthrough",
                "turn_domain": "turn.box.k2.dev",
                "bind_v4": "127.0.0.1",
                "profile": "shared",
            })
        );
        serde_json::from_value::<SewMediaInstallInputs>(inputs).expect("real Sew's inputs take it");
        // Sew refuses relay-only with tier plain/off (exit 78): K2 never
        // sends relay-only without the passthrough tier.
        for bad in [
            r#"{"relay_only":true}"#,
            r#"{"relay_only":true,"turn_tls":"plain"}"#,
            r#"{"relay_only":true,"turn_tls":"off"}"#,
            r#"{"relay_only":true,"turn_tls":"direct"}"#,
            r#"{"turn_tls":"passthrough","bind_v4":""}"#,
            r#"{"turn_tls":"passthrough","bind_v4":"10.0.0.5"}"#,
        ] {
            let m: MediaRequest = serde_json::from_str(bad).unwrap();
            assert!(m.validate_reconfigure().is_err(), "{bad}");
        }
        let back: MediaRequest = serde_json::from_str(r#"{"turn_tls":"direct","relay_only":false,"bind_v4":"0.0.0.0"}"#).unwrap();
        back.validate_reconfigure().unwrap();
        let direct: MediaRequest = serde_json::from_str(r#"{"relay_only":false}"#).unwrap();
        direct.validate_reconfigure().unwrap();
        assert!(direct.needs_tunnel_keys());
        let old: MediaRequest = serde_json::from_str(r#"{"turn_tls":"direct"}"#).unwrap();
        assert!(!old.needs_tunnel_keys(), "a pre-0.46.8 request stays protocol-11 clean");
        assert!(old.sew_inputs(None, None).get("relay_only").is_none());
        assert!(serde_json::from_str::<MediaRequest>(r#"{"relay_only":"yes"}"#).is_err());
        // Sew 0.7.1's PROXY sources: exactly K2's loopback pair, with the
        // passthrough tier, through to the key Sew reads.
        let pair = r#"["127.0.0.1/32","::1/128"]"#;
        let with: MediaRequest = serde_json::from_str(&format!(r#"{{"turn_tls":"passthrough","turn_trusted_cidrs":{pair}}}"#)).unwrap();
        with.validate_reconfigure().unwrap();
        assert!(with.needs_tunnel_keys());
        let inputs = with.sew_inputs(None, None);
        assert_eq!(inputs["turn_trusted_cidrs"], serde_json::json!(["127.0.0.1/32", "::1/128"]));
        serde_json::from_value::<SewMediaInstallInputs>(inputs).expect("0.7.1's inputs take it");
        for bad in [
            format!(r#"{{"turn_trusted_cidrs":{pair}}}"#),
            format!(r#"{{"turn_tls":"direct","turn_trusted_cidrs":{pair}}}"#),
            r#"{"turn_tls":"passthrough","turn_trusted_cidrs":[]}"#.to_string(),
            r#"{"turn_tls":"passthrough","turn_trusted_cidrs":["127.0.0.1/32"]}"#.to_string(),
            r#"{"turn_tls":"passthrough","turn_trusted_cidrs":["0.0.0.0/0","::/0"]}"#.to_string(),
            r#"{"turn_tls":"passthrough","turn_trusted_cidrs":["::1/128","127.0.0.1/32"]}"#.to_string(),
        ] {
            let m: MediaRequest = serde_json::from_str(&bad).unwrap();
            assert!(m.validate_reconfigure().is_err(), "{bad}");
        }
        // Which Sew takes it: 0.7.1 and later, with or without a build suffix.
        for (v, yes) in [
            ("0.7.1", true),
            ("0.7.1+g77c5e1f", true),
            ("0.7.2", true),
            ("0.8.0", true),
            ("0.7.0", false),
            ("0.6.1", false),
            ("0.6.1+g1234567", false),
            ("0.0.2", false),
            ("", false),
            ("not-a-version", false),
        ] {
            assert_eq!(sew_takes_trusted_cidrs(v), yes, "{v}");
        }
        // Sew's inputs refuse what real Sew refuses: an unknown key, a wrong type.
        assert!(serde_json::from_str::<SewMediaInstallInputs>(r#"{"tier":"passthrough"}"#).is_err());
        assert!(serde_json::from_str::<SewMediaInstallInputs>(r#"{"relay_only":"true"}"#).is_err());
        assert!(serde_json::from_str::<SewMediaInstallInputs>(r#"{"udp_port":"7882"}"#).is_err());
        // A direct install, staged by the helper, is one Sew takes.
        let d: MediaRequest = serde_json::from_str(
            r#"{"public_url":"wss://voice.box.k2.dev","node_ip":"203.0.113.7","udp_port":7882,"tcp_port":7881,"turn_tls":"direct","turn_domain":"turn-box.k2.dev","turn_advertise_port":5349,"sysctl":"apply","relay_only":false,"bind_v4":"0.0.0.0"}"#,
        )
        .unwrap();
        serde_json::from_value::<SewMediaInstallInputs>(d.sew_inputs(Some("/s/m.tgz"), Some(("/s/c.pem", "/s/k.pem")))).expect("real Sew's inputs take it");
    }

    #[test]
    fn mirror_dir_requests_stay_under_a_safe_ship_folder() {
        let ok = MirrorDirRequest { ship_path: "/mnt/backup/k2".into(), install_id: "48831765147549701".into() };
        ok.validate().unwrap();
        assert_eq!(ok.mirror_path(), "/mnt/backup/k2/sew-keyring-48831765147549701");
        assert_eq!(MirrorDirRequest::from_mirror_path(&ok.mirror_path()).as_ref(), Some(&ok), "the round trip the daemon's tie uses");
        assert_eq!(MirrorDirRequest::from_mirror_path("/mnt/backup/other-48831765147549701"), None);
        assert_eq!(MirrorDirRequest::from_mirror_path("no-slash"), None);
        for (ship, id) in [
            ("/mnt/backup", "none"),
            ("/mnt/backup", "0123"),
            ("/mnt/backup", "../x"),
            ("/", "48831765147549701"),
            ("/var/lib/sew", "48831765147549701"),
            ("/etc/x", "48831765147549701"),
            ("/home/k2/.k2/ship", "48831765147549701"),
            ("relative/path", "48831765147549701"),
            ("/mnt/../etc", "48831765147549701"),
            ("/mnt/backup/", "48831765147549701"),
            ("/mnt/$(id)", "48831765147549701"),
        ] {
            let r = MirrorDirRequest { ship_path: ship.into(), install_id: id.into() };
            assert!(r.validate().is_err(), "{ship} {id}");
        }
        assert!(serde_json::from_str::<MirrorDirRequest>(r#"{"shipPath":"/mnt/b","installId":"1","owner":"root"}"#).is_err());
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
        big.extend(std::iter::repeat_n(b'a', MAX_SYSCTL_BASELINE));
        big.extend(br#""}"#);
        assert_eq!(parse_sysctl_baseline(&big), None, "over the size cap");
    }
}
