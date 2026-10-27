//! Protocol numbers, the build marker, and semver.
//!
//! Protocol history (`prd-k2-root-helper-v1` §7, RH21):
//! - 1 = the original `k2-mail-helper` verbs;
//! - 2 = `version`/`usage`/`snapshot-data`/`restore-data`/`stop` (0.45.0);
//! - 3 = self-update + backup + `harden-store` + the `pg` namespace;
//! - 4 = `keep`;
//! - 5 = `daemon handoff-dropin install|remove` (runtime stage H, 0.46.0).
//! - 6 = the App runtime's `apps` namespace (box prep, runtime slice 2;
//!   the engine install and per-App nft rules, slice 3) + `pg hba-app`
//!   (0.46.1, vs-live 2 AR38). All of it ships together in 0.46.1, behind
//!   the preview; anything added after a 6 build ships needs 7.
//! - 7 = Sew's `engine sew` namespace (Sew S29, K11): stage / install /
//!   upgrade / rollback / uninstall / purge, units, stdin-JSON config,
//!   master-key rotation and the media sysctl drop-in. The daemon gates it
//!   on `protocol >= 7 && namespaces.sew == true` ([`SEW_MIN_PROTOCOL`]).
//!   Shipped in 0.46.0.
//! - 8 = every root-helper change 0.46.1 makes, in one bump
//!   (plan-0461-lanes Q5, renumbered once 0.46.0 shipped 7 for Sew): lane
//!   1's wall verbs, lane P's `apps podman install|uninstall`, and Sew's
//!   licence notices (`engine sew install-bin`, `upgrade` and
//!   `media-install` also put the pinned tarball's `LICENSE`, `NOTICE` and
//!   `THIRD_PARTY_NOTICES.md` under [`crate::sew::SEW_DOC_DIR`] /
//!   [`crate::sew::SEW_MEDIA_DOC_DIR`]), plus Sew's upgrade contract
//!   (2026-10-17): `engine sew upgrade` hands the installed Sew the pinned
//!   tarball's extracted `sew` member (`--binary`, sha256 = the pin's
//!   `binary_sha256`) instead of the tarball, a non-zero Sew exit answers a
//!   `reason`, and `engine sew media upgrade` (new) upgrades the voice
//!   engine through the NEW core; a rollback mirrors it (`engine sew media
//!   rollback`: the OLD core reinstalls its release's engine from the
//!   tarballs the helper keeps under [`crate::sew::SEW_KEEP_DIR`];
//!   `stage-media` keeps an older release's engine; status lists
//!   `staged.keptMedia`), the `front` namespace (front443:
//!   `front install|remove|status`, the box :443 socket held for the
//!   daemon, gated on `namespaces.front`, [`FRONT_MIN_PROTOCOL`]) and
//!   `turn_tls: "passthrough"` in the Sew media requests. Anything after
//!   an 8 build ships needs 9.
//! - 9 = every root-helper change 0.46.3 makes, in one bump, shared by
//!   every lane: `helper cli-refresh` (the daemon's auto-heal refreshes a
//!   stale root-owned `/usr/local/bin/k2` to the CLI compiled into the
//!   helper; gated on this AND `namespaces.cli`,
//!   [`CLI_REFRESH_MIN_PROTOCOL`]) and the `pg` namespace's several-majors
//!   verbs (`namespaces.pgmajor`, [`PGMAJOR_MIN_PROTOCOL`],
//!   `prd-pg17-and-db-upgrade-v1` §15 S2): `clusters`, `add-major`,
//!   `ensure-cluster`, `start-conf`, `install-slice`, `psql --cluster`,
//!   `copy-roles`, `move-dump`, `move-restore`, `move-file-remove`,
//!   `allow-conn`, `drop-db`. Anything after a 9 build ships needs 10.
//! - 10 = every root-helper change 0.46.4 makes, in one bump, shared by
//!   every lane (the ONE place to add to it): the `creds` namespace
//!   (`prd-vault-gaps-v1` item 4, `k2 vault systemd-cred`): `creds status`,
//!   `creds put <cred>` (value on stdin), `creds bind|unbind <unit>
//!   <cred>`, `creds restart <unit>`, `creds remove <cred>`
//!   ([`crate::creds`]). The daemon gates it on this AND
//!   `namespaces.creds == true` ([`CREDS_MIN_PROTOCOL`]). Also the live
//!   dual-run verbs (`prd-app-pg-choice-dualrun-v1` §9,
//!   `namespaces.pgdual`, [`PGDUAL_MIN_PROTOCOL`]): `pg logical-conf`,
//!   `pg move-dump … --schema-only`, `pg move-restore … --keep-force-off`,
//!   `pg force-rls`, `pg allow-conn … quiesce-repl` and `pg dual
//!   setup-source|subscribe|detach|teardown|status` ([`crate::pg_dual`]),
//!   plus the `pg_hba.conf` replication guard line. Another 0.46.4
//!   lane that needs a helper change joins 10 here and adds its own
//!   namespace flag; never 11 before 10 ships. Anything after a 10 build
//!   ships needs 11.
//! - 11 = every root-helper change 0.46.6 makes, in one bump, shared by
//!   every lane (the ONE place to add to it; 0.46.5 shipped 10 unchanged):
//!   `engine sew install reconfigure` (`sew install --reconfigure` after an
//!   upgrade from Sew 0.3.0, Sew 2026-10-27) and `engine sew systemctl
//!   reset-failed` (`k2 hostchat restart`), both under
//!   `namespaces.sewreconf`, [`SEW_RECONFIGURE_MIN_PROTOCOL`]. Another 0.46.6 lane that needs a
//!   helper change (e.g. App custom domains' :80 listener) joins 11 here
//!   with its own namespace flag; never 12 before 11 ships.
//!
//! The first `k2-root-helper` build ships 3 and 4 together and answers 4.
//! Protocols are additive: a newer helper serves an older daemon.

use std::cmp::Ordering;
use std::fmt;

/// What this build of the helper speaks.
pub const PROTOCOL: u32 = 11;

/// `k2 hostmail upgrade` through the root helper. 4, not 2: the daemon's
/// table moves from 0.16.20 OR 0.16.10 (pin 0.16.25), so `install-bin`
/// must accept every upgrade source and `version` must list them
/// (`stalwartUpgradeFrom`) — the legacy mail helper's protocol 3. Every
/// root helper that does so answers 4 (the first root-helper build ships
/// with this table).
pub const UPGRADE_MIN_PROTOCOL: u32 = 4;
/// `helper self-update` / `self-rollback`.
pub const SELF_UPDATE_MIN_PROTOCOL: u32 = 3;
/// Backup B2 verbs (`backup …`).
pub const BACKUP_MIN_PROTOCOL: u32 = 3;
/// The `pg` namespace. Below it the daemon uses the legacy pg helper.
pub const PG_MIN_PROTOCOL: u32 = 3;
/// The vault keeper (`keep …`).
pub const KEEP_MIN_PROTOCOL: u32 = 4;
/// The daemon unit's hand-off drop-in (`daemon handoff-dropin …`).
pub const HANDOFF_MIN_PROTOCOL: u32 = 5;
/// The App runtime's `apps` namespace (box prep) and `pg hba-app` (0.46.1,
/// AR38). Helpers on the fleet answer ≤ 5, so the daemon treats them as
/// "no App verbs" until they self-update.
pub const APPS_MIN_PROTOCOL: u32 = 6;
/// Sew's `engine sew` namespace (Sew S29, K11). Gated on this AND the
/// `namespaces.sew` flag in `helper version` (protocol numbers on other
/// branches have collided before; the flag keeps them apart).
pub const SEW_MIN_PROTOCOL: u32 = 7;
/// `engine sew install reconfigure` (0.46.6). Gated on this AND
/// `namespaces.sewreconf == true` in `helper version`.
pub const SEW_RECONFIGURE_MIN_PROTOCOL: u32 = 11;
/// `apps podman install|uninstall` (K2's Podman 5 bundle, 0.46.1 lane P).
/// Rides 0.46.1's one helper bump, 8 (7 shipped in 0.46.0 for Sew).
pub const PODMAN_INSTALL_MIN_PROTOCOL: u32 = 8;
/// front443's `front install|remove|status` (0.46.1). Gated on this AND
/// `namespaces.front == true` in `helper version`, the way Sew is.
pub const FRONT_MIN_PROTOCOL: u32 = 8;
/// `helper cli-refresh` (0.46.3). Gated on this AND `namespaces.cli == true`
/// in `helper version`, the way Sew and front are.
pub const CLI_REFRESH_MIN_PROTOCOL: u32 = 9;
/// Postgres majors side by side (prd-pg17-and-db-upgrade-v1 rev 2, 0.46.3):
/// `pg clusters`, `pg add-major`, `pg ensure-cluster`, `pg psql --cluster
/// <M>` and the move verbs. 0.46.3's one helper bump (8 → 9), shared by
/// every lane. Gated on this AND `namespaces.pgmajor == true`.
pub const PGMAJOR_MIN_PROTOCOL: u32 = 9;
/// `k2 vault systemd-cred` (prd-vault-gaps-v1 item 4, 0.46.4): the `creds`
/// namespace. 0.46.4's one helper bump (9 → 10), shared by every lane.
/// Gated on this AND `namespaces.creds == true`.
pub const CREDS_MIN_PROTOCOL: u32 = 10;
/// Live dual-run (prd-app-pg-choice-dualrun-v1 §9, 0.46.4): joins 10 (never
/// 11 before 10 ships). Gated on this AND `namespaces.pgdual == true`; a
/// helper without it answers `--live` with `helper_old` and keeps the copy
/// path.
pub const PGDUAL_MIN_PROTOCOL: u32 = 10;

pub const HELPER_NAME: &str = "k2-root-helper";

/// A plain `x.y.z` release version. Anything else is refused: a version
/// becomes part of a URL, a trusted comment and an anti-rollback compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Semver(pub u32, pub u32, pub u32);

impl Semver {
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split('.');
        let a = part(it.next()?)?;
        let b = part(it.next()?)?;
        let c = part(it.next()?)?;
        if it.next().is_some() {
            return None;
        }
        Some(Self(a, b, c))
    }
}

fn part(p: &str) -> Option<u32> {
    if p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if p.len() > 1 && p.starts_with('0') {
        return None;
    }
    p.parse().ok()
}

impl Ord for Semver {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.0, self.1, self.2).cmp(&(o.0, o.1, o.2))
    }
}
impl PartialOrd for Semver {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl fmt::Display for Semver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The arches the helper ships for.
pub fn arch_ok(arch: &str) -> bool {
    matches!(arch, "x86_64" | "aarch64")
}

/// The marker needle, decoded at runtime so the full prefix appears in a
/// binary only inside the one embedded marker (§9.1). Stored shifted by
/// one byte; `black_box` keeps the optimizer from folding it back into a
/// literal (a plain `concat()` of two pieces was folded — caught by the
/// release-binary marker count).
pub fn marker_prefix() -> String {
    let shifted: &[u8] = std::hint::black_box(b"L3`SPPU`IFMQFS`CVJME!w>");
    shifted.iter().map(|b| (b - 1) as char).collect()
}

/// `K2_ROOT_HELPER_BUILD v=<semver> proto=<n> arch=<arch>;`
pub fn format_marker(version: &str, protocol: u32, arch: &str) -> String {
    format!("{}{version} proto={protocol} arch={arch};", marker_prefix())
}

/// One parsed build marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub version: Semver,
    pub protocol: u32,
    pub arch: String,
}

/// Every marker in `bytes`. `Err` when any occurrence of the prefix does
/// not parse as a whole marker (a truncated or doctored marker is never
/// skipped).
pub fn find_markers(bytes: &[u8]) -> Result<Vec<Marker>, &'static str> {
    let needle = marker_prefix();
    let needle = needle.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let rest = &bytes[i + needle.len()..];
        let end = rest
            .iter()
            .take(64)
            .position(|&b| b == b';')
            .ok_or("marker not terminated")?;
        let body = std::str::from_utf8(&rest[..end]).map_err(|_| "marker not utf-8")?;
        out.push(parse_marker_body(body).ok_or("marker malformed")?);
        i += needle.len() + end + 1;
    }
    Ok(out)
}

fn parse_marker_body(body: &str) -> Option<Marker> {
    let mut it = body.split(' ');
    let version = Semver::parse(it.next()?)?;
    let protocol = it.next()?.strip_prefix("proto=")?;
    let arch = it.next()?.strip_prefix("arch=")?;
    if it.next().is_some() || !arch_ok(arch) {
        return None;
    }
    if protocol.is_empty() || protocol.len() > 4 || !protocol.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Marker {
        version,
        protocol: protocol.parse().ok()?,
        arch: arch.to_string(),
    })
}

/// ELF `e_machine` for an arch name.
pub fn elf_machine(arch: &str) -> Option<u16> {
    match arch {
        "x86_64" => Some(62),
        "aarch64" => Some(183),
        _ => None,
    }
}

/// A 64-bit little-endian ELF for `arch`.
pub fn elf_matches(bytes: &[u8], arch: &str) -> bool {
    let Some(want) = elf_machine(arch) else {
        return false;
    };
    bytes.len() >= 64
        && &bytes[0..4] == b"\x7fELF"
        && bytes[4] == 2 // ELFCLASS64
        && bytes[5] == 1 // little-endian
        && u16::from_le_bytes([bytes[18], bytes[19]]) == want
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AR38: 5 is the hand-off drop-in, 6 is the App runtime's namespace,
    /// 7 is Sew's `engine sew` (0.46.0), 8 is every 0.46.1 helper change
    /// (wall verbs, Podman install, Sew's notices). This build answers 8;
    /// the Sew verbs and the apps verbs are still in the catalog
    /// (protocols are additive).
    #[test]
    fn protocol_numbers_follow_ar38() {
        assert_eq!(HANDOFF_MIN_PROTOCOL, 5);
        assert_eq!(APPS_MIN_PROTOCOL, 6);
        assert_eq!(SEW_MIN_PROTOCOL, 7);
        assert_eq!(PROTOCOL, 11, "protocol 11 = 0.46.6's one bump (Sew install reconfigure); 10 = 0.46.4's (creds)");
        assert_eq!(SEW_RECONFIGURE_MIN_PROTOCOL, 11, "Sew reconfigure joins 0.46.6's one bump");
        assert_eq!(CREDS_MIN_PROTOCOL, 10, "creds joins 0.46.4's one bump; never 11 before 10 ships");
        let creds: Vec<_> = crate::verbs::catalog().into_iter().filter(|v| v.namespace() == "creds").collect();
        assert_eq!(creds.len(), 6, "creds status|put|bind|unbind|restart|remove: {creds:?}");
        assert_eq!(CLI_REFRESH_MIN_PROTOCOL, 9);
        assert_eq!(PGMAJOR_MIN_PROTOCOL, 9, "the majors verbs join 0.46.3's one bump; never 10");
        let pgmajor: Vec<_> = crate::verbs::catalog()
            .into_iter()
            .filter(|v| {
                let a = v.argv();
                a[0] == "pg"
                    && matches!(
                        a[1].as_str(),
                        "clusters"
                            | "add-major"
                            | "ensure-cluster"
                            | "start-conf"
                            | "install-slice"
                            | "copy-roles"
                            | "move-dump"
                            | "move-restore"
                            | "move-file-remove"
                            | "allow-conn"
                            | "drop-db"
                    )
            })
            .collect();
        assert!(pgmajor.len() >= 11, "{pgmajor:?}");
        let sew: Vec<_> = crate::verbs::catalog()
            .into_iter()
            .filter(|v| v.namespace() == "sew")
            .collect();
        assert!(!sew.is_empty(), "protocol 8 without any engine sew verb");
        assert_eq!(FRONT_MIN_PROTOCOL, 8, "front joins 0.46.1's protocol 8; never its own bump");
        let front: Vec<_> = crate::verbs::catalog()
            .into_iter()
            .filter(|v| v.namespace() == "front")
            .collect();
        assert_eq!(front.len(), 3, "front install|remove|status");
        let apps: Vec<_> = crate::verbs::catalog()
            .into_iter()
            .filter(|v| v.namespace() == "apps")
            .collect();
        for needed in [
            &["apps", "status"][..],
            &["apps", "ensure-user"],
            &["apps", "subid"],
            &["apps", "linger", "on"],
            &["apps", "delegate", "on"],
            &["apps", "io-cost", "on"],
            &["apps", "reserve-ports", "on"],
            &["apps", "nft", "apply", "38600-38899", "38900-38999", "5432-5449"],
            &["apps", "process-unit", "on"],
            &["apps", "packages", "install"],
            &["apps", "unprep"],
        ] {
            let v = crate::verbs::Verb::parse(needed).expect("apps verb parses");
            assert!(apps.contains(&v), "{needed:?} missing from the catalog");
        }
    }

    /// Lane P's verbs need 8: a 7 helper (0.46.0, Sew) or older refuses
    /// them (unknown argv), so the daemon must not ask one.
    #[test]
    fn podman_verbs_ride_protocol_8() {
        assert_eq!(PODMAN_INSTALL_MIN_PROTOCOL, 8);
        assert!(PODMAN_INSTALL_MIN_PROTOCOL <= PROTOCOL);
        assert!(PODMAN_INSTALL_MIN_PROTOCOL > SEW_MIN_PROTOCOL);
        assert!(PODMAN_INSTALL_MIN_PROTOCOL > APPS_MIN_PROTOCOL);
        let cat = crate::verbs::catalog();
        for argv in [&["apps", "podman", "install"][..], &["apps", "podman", "uninstall"]] {
            let v = crate::verbs::Verb::parse(argv).expect("parses");
            assert!(cat.contains(&v), "{argv:?}");
            assert_eq!(v.class(), crate::verbs::Class::Daemon, "{argv:?}");
        }
    }

    #[test]
    fn semver_is_plain_xyz_and_orders_numerically() {
        assert_eq!(Semver::parse("0.45.10"), Some(Semver(0, 45, 10)));
        assert!(Semver::parse("0.45.10") > Semver::parse("0.45.9"));
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.x",
            "v1.2.3",
            "1.2.3-rc1",
            "1..3",
            "9999999.0.0",
        ] {
            assert!(Semver::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn markers_round_trip_and_doctored_ones_fail() {
        let m = format_marker("0.45.5", 4, "x86_64");
        let mut bin = b"\x7fELF....".to_vec();
        bin.extend_from_slice(m.as_bytes());
        bin.extend_from_slice(b"\0trailing");
        let found = find_markers(&bin).unwrap();
        assert_eq!(
            found,
            vec![Marker {
                version: Semver(0, 45, 5),
                protocol: 4,
                arch: "x86_64".into()
            }]
        );

        let mut two = bin.clone();
        two.extend_from_slice(format_marker("0.45.6", 4, "x86_64").as_bytes());
        assert_eq!(find_markers(&two).unwrap().len(), 2);

        assert!(find_markers(b"nothing here").unwrap().is_empty());
        for bad in [
            format!("{}0.45.5 proto=4 arch=riscv64;", marker_prefix()),
            format!("{}0.45.5 proto=4 arch=x86_64", marker_prefix()),
            format!("{}0.45 proto=4 arch=x86_64;", marker_prefix()),
            format!("{}0.45.5 proto=4 arch=x86_64 extra;", marker_prefix()),
            format!("{}0.45.5 proto= arch=x86_64;", marker_prefix()),
        ] {
            assert!(find_markers(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn elf_machine_check() {
        let mut h = vec![0u8; 64];
        h[0..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[5] = 1;
        h[18..20].copy_from_slice(&62u16.to_le_bytes());
        assert!(elf_matches(&h, "x86_64"));
        assert!(!elf_matches(&h, "aarch64"));
        h[18..20].copy_from_slice(&183u16.to_le_bytes());
        assert!(elf_matches(&h, "aarch64"));
        assert!(!elf_matches(&h[..63], "aarch64"));
        assert!(!elf_matches(b"#!/bin/sh\n", "x86_64"));
    }
}
