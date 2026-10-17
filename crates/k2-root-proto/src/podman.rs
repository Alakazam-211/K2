//! K2's Podman 5 bundle (0.46.1 lane P, Rosson 2026-10-16): on a box
//! whose distro ships Podman < 5 (Ubuntu 24.04: 4.9.3, Debian 12: 4.3.1),
//! `k2 app box prepare --install-podman` installs a pinned, sha256-checked
//! static Podman 5 into a K2-owned prefix through the signed root helper
//! (`apps podman install`, protocol 7). Distros with Podman 5+ (Debian 13,
//! Ubuntu 26.04, Fedora, Arch) keep using the distro's.
//!
//! Source: `mgoltzsche/podman-static` release tarballs. They are static
//! (no glibc or library dependencies, so one tarball serves Ubuntu 24.04
//! and Debian 12) and carry the whole rootless stack walled Apps use:
//! podman + Quadlet, crun, conmon, pasta, catatonit (plus netavark,
//! aardvark-dns, rootlessport). Each release is GPG-signed by its
//! maintainer (key `0CCF102C4F95D89E583FF1D4F8B5AF50344BB503`); K2 pins
//! the tarball's sha256 instead, so this table is the trust root and the
//! helper (itself signed) checks it.
//!
//! Coexistence: the distro's Podman stays where it is, untouched, for
//! everyone else. Only the `k2app` user is pointed at the bundle:
//! - k2-app-engine and the daemon resolve `podman`/`crun` through
//!   [`podman_bin`]/[`crun_bin`] (the bundle when its marker is present);
//! - a per-UID drop-in (`containers.rootless.conf.d/<k2app uid>/`, read by
//!   Podman only for that uid) points Podman at the bundle's conmon,
//!   catatonit, pasta and crun, with the `cgroupfs` manager (the static
//!   crun has no systemd support; Quadlet's `--cgroups=split` keeps each
//!   container inside its unit's delegated cgroup);
//! - a user generator in `/etc/systemd/user-generators` (which shadows the
//!   distro's same-named one) runs the bundle's Quadlet for `k2app` only
//!   and hands every other user to the distro's Quadlet;
//! - where Ubuntu restricts unprivileged user namespaces through AppArmor,
//!   the bundle's podman, crun and pasta get the same unconfined `userns`
//!   profiles Ubuntu ships for its own copies.
//!
//! PATH is never changed for anyone.

use std::path::Path;

/// The pinned Podman release (upstream `containers/podman` v5.8.8, built
/// by podman-static v5.8.8, released 2026-09-30). The 5.x line: the
/// engine was proven on 5.4 (Debian 13); Podman 6 is not adopted yet.
pub const BUNDLE_VERSION: &str = "5.8.8";
/// Where the tarballs come from (the release's own download path).
pub const BUNDLE_URL_BASE: &str =
    "https://github.com/mgoltzsche/podman-static/releases/download";
/// The upstream project (for docs and the doctor).
pub const BUNDLE_SOURCE: &str = "mgoltzsche/podman-static";

/// One pinned tarball: Rust `std::env::consts::ARCH` name → asset name,
/// the tarball's top folder, its sha256 and its size.
#[derive(Debug)]
pub struct PodmanArtifact {
    pub arch: &'static str,
    pub asset: &'static str,
    pub top: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// REAL values for podman-static v5.8.8, computed 2026-10-16 from the
/// downloaded release tarballs (`shasum -a 256`) and cross-checked with
/// the digests GitHub records for the same release assets.
pub const BUNDLE: &[PodmanArtifact] = &[
    PodmanArtifact {
        arch: "x86_64",
        asset: "podman-linux-amd64.tar.gz",
        top: "podman-linux-amd64",
        sha256: "e0e12f9de61bdb054d19a77df0e84ddd51cadfee13688e0fb607d61b6833e8cd",
        size: 34_375_891,
    },
    PodmanArtifact {
        arch: "aarch64",
        asset: "podman-linux-arm64.tar.gz",
        top: "podman-linux-arm64",
        sha256: "816bc5b97dc62396838d825aaccd9d3eb841167a2d31968efc1e6a3e6a0a463b",
        size: 31_352_354,
    },
];

/// A placeholder or malformed pin: install refuses (never an unverifiable
/// install path).
pub fn checksum_is_placeholder(a: &PodmanArtifact) -> bool {
    a.sha256.contains("TODO")
        || a.sha256.len() != 64
        || !a.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The pinned tarball for `arch`.
pub fn artifact_for_arch(arch: &str) -> Result<&'static PodmanArtifact, String> {
    let a = BUNDLE
        .iter()
        .find(|a| a.arch == arch)
        .ok_or_else(|| format!("no K2 Podman bundle for CPU architecture '{arch}' (x86_64, aarch64)"))?;
    if checksum_is_placeholder(a) {
        return Err(format!(
            "refusing to install: the pinned sha256 for {arch} is a placeholder; this build cannot verify the Podman download"
        ));
    }
    Ok(a)
}

/// The download URL for a pinned tarball (`v<version>/<asset>`).
pub fn download_url(a: &PodmanArtifact) -> String {
    format!("{BUNDLE_URL_BASE}/v{BUNDLE_VERSION}/{}", a.asset)
}

/// Largest tarball the helper takes on stdin.
pub const CAP_BUNDLE: usize = 128 * 1024 * 1024;
/// Largest single member it extracts (podman itself is ~46 MiB).
pub const MAX_MEMBER: u64 = 96 * 1024 * 1024;

// ── the boxes the bundle is for ─────────────────────────────────────────

/// `/etc/os-release`, and the file os-release(5) falls back to. On Debian
/// and Ubuntu the first is a symlink to the second; the helper never
/// follows links, so it reads the second when the first isn't a file.
pub const OS_RELEASE: &str = "/etc/os-release";
pub const OS_RELEASE_USR: &str = "/usr/lib/os-release";

/// (`ID`, `VERSION_ID`) pairs the bundle is tested for: the distros whose
/// own Podman is older than 5. Everything else uses the distro's.
pub const SUPPORTED_OS: &[(&str, &str)] = &[("ubuntu", "24.04"), ("debian", "12")];

/// `ID` and `VERSION_ID` from os-release text (quotes stripped).
pub fn os_id(text: &str) -> Option<(String, String)> {
    let get = |k: &str| {
        text.lines().find_map(|l| {
            let v = l.trim().strip_prefix(k)?.strip_prefix('=')?;
            Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        })
    };
    Some((get("ID")?, get("VERSION_ID")?))
}

/// `Some("ubuntu 24.04")` when the bundle is for this box.
pub fn supported_os(os_release: &str) -> Option<String> {
    let (id, ver) = os_id(os_release)?;
    SUPPORTED_OS
        .iter()
        .any(|(i, v)| *i == id && *v == ver)
        .then(|| format!("{id} {ver}"))
}

/// The version gate: Podman missing or older than 5 needs an install.
pub fn needs_install(podman_version: Option<&str>) -> bool {
    match podman_version.and_then(|v| v.split('.').next()?.parse::<u32>().ok()) {
        Some(m) => m < crate::apps::PODMAN_MIN_MAJOR,
        None => true,
    }
}

// ── the K2 prefix ───────────────────────────────────────────────────────

/// `/opt/k2` (made 0755 root if missing) and the bundle under it.
pub const OPT: &str = "/opt";
pub const OPT_K2_NAME: &str = "k2";
pub const OPT_K2: &str = "/opt/k2";
pub const PREFIX_NAME: &str = "podman";
pub const PREFIX: &str = "/opt/k2/podman";
/// Install stages here, then renames over [`PREFIX`]; the previous tree
/// moves here first and is removed after the swap.
pub const STAGE_NAME: &str = ".podman-stage";
pub const OLD_NAME: &str = ".podman-old";

pub const PODMAN: &str = "/opt/k2/podman/bin/podman";
pub const CRUN: &str = "/opt/k2/podman/bin/crun";
pub const PASTA: &str = "/opt/k2/podman/bin/pasta";
pub const CONMON: &str = "/opt/k2/podman/lib/podman/conmon";
pub const CATATONIT: &str = "/opt/k2/podman/lib/podman/catatonit";
pub const QUADLET: &str = "/opt/k2/podman/libexec/podman/quadlet";
/// The install marker, written last inside the staged tree:
/// [`marker`]'s bytes. Its presence is what switches k2app to the bundle.
pub const MARKER_NAME: &str = "k2-bundle.json";
pub const MARKER: &str = "/opt/k2/podman/k2-bundle.json";

/// One tarball member K2 takes: its path inside `<top>/`, where it goes
/// under [`PREFIX`], and whether the bundle is incomplete without it.
/// Everything else in the tarball (runc, fuse-overlayfs, system units,
/// completions, `/etc/containers` defaults) is never written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    pub src: &'static str,
    pub dest: &'static str,
    pub required: bool,
}

pub const MEMBERS: &[Member] = &[
    Member { src: "usr/local/bin/podman", dest: "bin/podman", required: true },
    Member { src: "usr/local/bin/crun", dest: "bin/crun", required: true },
    Member { src: "usr/local/bin/pasta", dest: "bin/pasta", required: true },
    // amd64 only: pasta re-execs it on CPUs with AVX2.
    Member { src: "usr/local/bin/pasta.avx2", dest: "bin/pasta.avx2", required: false },
    Member { src: "usr/local/lib/podman/conmon", dest: "lib/podman/conmon", required: true },
    Member { src: "usr/local/lib/podman/catatonit", dest: "lib/podman/catatonit", required: true },
    Member { src: "usr/local/lib/podman/netavark", dest: "lib/podman/netavark", required: true },
    Member { src: "usr/local/lib/podman/aardvark-dns", dest: "lib/podman/aardvark-dns", required: true },
    Member { src: "usr/local/lib/podman/rootlessport", dest: "lib/podman/rootlessport", required: true },
    Member { src: "usr/local/libexec/podman/quadlet", dest: "libexec/podman/quadlet", required: true },
];

/// The directories under [`PREFIX`] the members land in, parents first.
pub const DIRS: &[&str] = &["bin", "lib", "lib/podman", "libexec", "libexec/podman"];

/// The member for a tarball path (`<top>/<src>`), if K2 takes it.
pub fn member_for(top: &str, path: &str) -> Option<&'static Member> {
    let rest = path.strip_prefix(top)?.strip_prefix('/')?;
    MEMBERS.iter().find(|m| m.src == rest)
}

/// The marker's canned bytes.
pub fn marker(arch: &str, sha256: &str) -> String {
    format!(
        "{{\"bundle\":\"{BUNDLE_SOURCE}\",\"version\":\"{BUNDLE_VERSION}\",\"arch\":\"{arch}\",\"sha256\":\"{sha256}\"}}\n"
    )
}

// ── what k2app runs ─────────────────────────────────────────────────────

/// True when the bundle is installed (its marker is a regular file).
pub fn installed() -> bool {
    Path::new(MARKER).is_file()
}

/// `podman` for k2app: the bundle's when installed, else the distro's.
pub fn podman_bin() -> &'static str {
    pick(installed(), PODMAN, crate::apps::PODMAN)
}

/// `crun` for k2app (the OCI shim execs it).
pub fn crun_bin() -> &'static str {
    pick(installed(), CRUN, crate::apps::CRUN)
}

/// `pasta` for k2app (the doctor's check).
pub fn pasta_bin() -> &'static str {
    pick(installed(), PASTA, crate::apps::PASTA)
}

/// The resolution rule, testable without a filesystem.
pub fn pick(bundle: bool, ours: &'static str, distro: &'static str) -> &'static str {
    if bundle {
        ours
    } else {
        distro
    }
}

// ── canned config ───────────────────────────────────────────────────────

/// `/etc/containers/containers.rootless.conf.d/<k2app uid>/` (Podman
/// reads it only for that uid) and the file K2 writes there.
pub const ROOTLESS_CONF_PARENT: &str = "/etc/containers/containers.rootless.conf.d";
pub const ETC_CONTAINERS: &str = "/etc/containers";
pub const ROOTLESS_CONF_D_NAME: &str = "containers.rootless.conf.d";
pub const ROOTLESS_CONF_FILE: &str = "90-k2-podman.conf";

pub fn rootless_conf_dir(uid: u32) -> String {
    format!("{ROOTLESS_CONF_PARENT}/{uid}")
}

/// The k2app-only Podman config.
pub fn rootless_conf() -> String {
    format!(
        "# K2: Podman for the k2app user comes from K2's bundle ({PREFIX}).\n\
         # Written by k2-root-helper `apps podman install`; edits are overwritten.\n\
         [engine]\n\
         helper_binaries_dir = [\"{PREFIX}/lib/podman\", \"{PREFIX}/libexec/podman\", \"{PREFIX}/bin\"]\n\
         conmon_path = [\"{CONMON}\"]\n\
         init_path = \"{CATATONIT}\"\n\
         # The static crun has no systemd support: cgroupfs, inside each\n\
         # Quadlet unit's delegated cgroup (--cgroups=split).\n\
         cgroup_manager = \"cgroupfs\"\n\
         events_logger = \"file\"\n\
         \n\
         [engine.runtimes]\n\
         crun = [\"{CRUN}\"]\n\
         \n\
         [network]\n\
         default_rootless_network_cmd = \"pasta\"\n"
    )
}

/// `/etc/systemd/user-generators/podman-user-generator`: same name as the
/// distro's `/usr/lib/systemd/user-generators/` one, which it shadows.
pub const USER_GENERATORS: &str = "/etc/systemd/user-generators";
pub const USER_GENERATOR_NAME: &str = "podman-user-generator";
pub const USER_GENERATOR: &str = "/etc/systemd/user-generators/podman-user-generator";
/// The distro's Quadlet (Debian and Ubuntu), for every other user.
pub const DISTRO_QUADLET: &str = "/usr/libexec/podman/quadlet";
/// The first line after the shebang: how uninstall knows the file is K2's.
pub const GENERATOR_MARK: &str = "# K2 Podman bundle: Quadlet for k2app (k2-root-helper `apps podman install`).";

/// The user generator: the bundle's Quadlet (writing units that call the
/// bundle's podman) for `k2app` only; everyone else gets the distro's.
pub fn user_generator(k2app_uid: u32) -> String {
    format!(
        "#!/bin/sh\n\
         {GENERATOR_MARK}\n\
         # Edits are overwritten. Every other user gets the distro's Quadlet.\n\
         if [ \"$(/usr/bin/id -u)\" = \"{k2app_uid}\" ]; then\n\
         \x20 PODMAN={PODMAN} exec {QUADLET} -user \"$@\"\n\
         fi\n\
         if [ -x {DISTRO_QUADLET} ]; then\n\
         \x20 exec {DISTRO_QUADLET} -user \"$@\"\n\
         fi\n\
         exit 0\n"
    )
}

/// `1` where Ubuntu (23.10+) makes unconfined programs that create a user
/// namespace drop their capabilities unless an AppArmor profile allows it.
pub const USERNS_RESTRICT: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
pub const APPARMOR_USERNS_FILE_NAME: &str = "k2-podman";
pub const APPARMOR_USERNS_FILE: &str = "/etc/apparmor.d/k2-podman";
/// The profiles in that file (also what `apparmor_parser -R` unloads).
pub const APPARMOR_USERNS_PROFILES: &[&str] = &["k2-podman", "k2-crun", "k2-pasta"];

/// The bundle's podman, crun and pasta get exactly what Ubuntu's own
/// `/etc/apparmor.d/{podman,crun}` give the distro copies: unconfined,
/// plus user namespaces. Written only where [`USERNS_RESTRICT`] is `1`
/// (AppArmor 4 ABI). The k2-app profile allows signals from `k2-crun`.
pub fn apparmor_userns_profiles() -> String {
    format!(
        "# K2 Podman bundle: user namespaces for K2's podman, crun and pasta,\n\
         # like Ubuntu's own profiles for the distro copies.\n\
         # Written by k2-root-helper `apps podman install`; edits are overwritten.\n\
         abi <abi/4.0>,\n\
         include <tunables/global>\n\
         \n\
         profile k2-podman {PODMAN} flags=(unconfined) {{\n\
         \x20 userns,\n\
         }}\n\
         \n\
         profile k2-crun {CRUN} flags=(unconfined) {{\n\
         \x20 userns,\n\
         }}\n\
         \n\
         profile k2-pasta {PASTA}{{,.avx2}} flags=(unconfined) {{\n\
         \x20 userns,\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_real_and_cover_both_arches() {
        for arch in ["x86_64", "aarch64"] {
            let a = artifact_for_arch(arch).expect("pinned");
            assert!(!checksum_is_placeholder(a), "{arch}");
            assert!(a.asset.starts_with(a.top), "{arch}");
            assert!(download_url(a).starts_with(
                "https://github.com/mgoltzsche/podman-static/releases/download/v5.8.8/podman-linux-"
            ));
            assert!(a.size > 1_000_000 && (a.size as usize) < CAP_BUNDLE, "{arch}");
        }
        assert!(artifact_for_arch("riscv64").is_err());
        assert!(!BUNDLE_URL_BASE.contains("latest"), "never latest");
        for bad in [
            "TODO",
            "e0e12f9de61bdb054d19a77df0e84ddd51cadfee13688e0fb607d61b6833e8c",
            "E0E12F9DE61BDB054D19A77DF0E84DDD51CADFEE13688E0FB607D61B6833E8CD",
            "zz e12f9de61bdb054d19a77df0e84ddd51cadfee13688e0fb607d61b6833e8c",
        ] {
            let a = PodmanArtifact { arch: "x86_64", asset: "a", top: "a", sha256: bad, size: 1 };
            assert!(checksum_is_placeholder(&a), "{bad}");
        }
    }

    #[test]
    fn the_version_gate() {
        assert!(needs_install(None), "missing");
        assert!(needs_install(Some("4.9.3")), "Ubuntu 24.04");
        assert!(needs_install(Some("4.3.1")), "Debian 12");
        assert!(needs_install(Some("garbage")));
        assert!(!needs_install(Some("5.0.0")));
        assert!(!needs_install(Some("5.4.2")), "Debian 13");
        assert!(!needs_install(Some("5.8.8")), "the bundle itself");
        assert!(!needs_install(Some("6.1.4")));
        assert_eq!(BUNDLE_VERSION.split('.').next(), Some("5"));
    }

    #[test]
    fn os_support_is_exact() {
        let noble = "PRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";
        assert_eq!(supported_os(noble).as_deref(), Some("ubuntu 24.04"));
        let bookworm = "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nVERSION_ID=\"12\"\nID=debian\n";
        assert_eq!(supported_os(bookworm).as_deref(), Some("debian 12"));
        for other in [
            "ID=debian\nVERSION_ID=\"13\"\n",
            "ID=ubuntu\nVERSION_ID=\"26.04\"\n",
            "ID=ubuntu\nVERSION_ID=\"22.04\"\n",
            "ID=fedora\nVERSION_ID=42\n",
            "ID=arch\n",
            "ID_LIKE=ubuntu\nVERSION_ID=\"24.04\"\nID=linuxmint\n",
            "",
        ] {
            assert_eq!(supported_os(other), None, "{other:?}");
        }
        // ID_LIKE / VERSION_CODENAME never stand in for ID / VERSION_ID.
        assert_eq!(os_id("ID_LIKE=debian\nVERSION_ID=12\n"), None);
    }

    #[test]
    fn only_listed_members_are_taken() {
        let top = "podman-linux-amd64";
        assert_eq!(member_for(top, "podman-linux-amd64/usr/local/bin/podman").unwrap().dest, "bin/podman");
        assert_eq!(member_for(top, "podman-linux-amd64/usr/local/libexec/podman/quadlet").unwrap().dest, "libexec/podman/quadlet");
        for not in [
            "podman-linux-amd64/usr/local/bin/runc",
            "podman-linux-amd64/etc/containers/policy.json",
            "podman-linux-amd64/usr/local/lib/systemd/user-generators/podman-user-generator",
            "podman-linux-arm64/usr/local/bin/podman",
            "usr/local/bin/podman",
            "podman-linux-amd64/../usr/local/bin/podman",
            "podman-linux-amd64//usr/local/bin/podman",
        ] {
            assert!(member_for(top, not).is_none(), "{not}");
        }
        for m in MEMBERS {
            assert!(!m.dest.contains(".."), "{m:?}");
            let parent = m.dest.rsplit_once('/').unwrap().0;
            assert!(DIRS.contains(&parent), "{m:?}");
        }
        assert_eq!(MEMBERS.iter().filter(|m| !m.required).count(), 1, "only pasta.avx2 is optional");
    }

    #[test]
    fn paths_and_resolution() {
        for p in [PODMAN, CRUN, PASTA, CONMON, CATATONIT, QUADLET, MARKER] {
            assert!(p.starts_with("/opt/k2/podman/"), "{p}");
            let rel = p.strip_prefix("/opt/k2/podman/").unwrap();
            assert!(rel == MARKER_NAME || MEMBERS.iter().any(|m| m.dest == rel), "{p}");
        }
        assert_eq!(format!("{OPT}/{OPT_K2_NAME}"), OPT_K2);
        assert_eq!(format!("{OPT_K2}/{PREFIX_NAME}"), PREFIX);
        assert_eq!(format!("{USER_GENERATORS}/{USER_GENERATOR_NAME}"), USER_GENERATOR);
        assert_eq!(format!("{ETC_CONTAINERS}/{ROOTLESS_CONF_D_NAME}"), ROOTLESS_CONF_PARENT);
        assert_eq!(pick(true, PODMAN, crate::apps::PODMAN), PODMAN);
        assert_eq!(pick(false, PODMAN, crate::apps::PODMAN), "/usr/bin/podman");
        assert_eq!(rootless_conf_dir(996), "/etc/containers/containers.rootless.conf.d/996");
    }

    #[test]
    fn canned_bytes() {
        let c = rootless_conf();
        assert!(c.contains("cgroup_manager = \"cgroupfs\"\n"));
        assert!(c.contains("crun = [\"/opt/k2/podman/bin/crun\"]\n"));
        assert!(c.contains("conmon_path = [\"/opt/k2/podman/lib/podman/conmon\"]\n"));
        let g = user_generator(996);
        assert!(g.starts_with("#!/bin/sh\n"));
        assert_eq!(g.lines().nth(1), Some(GENERATOR_MARK));
        assert!(g.contains("= \"996\" ]"));
        assert!(g.contains("PODMAN=/opt/k2/podman/bin/podman exec /opt/k2/podman/libexec/podman/quadlet -user \"$@\""));
        assert!(g.contains("exec /usr/libexec/podman/quadlet -user \"$@\""), "others keep the distro's Quadlet");
        let a = apparmor_userns_profiles();
        assert!(a.contains("abi <abi/4.0>,"));
        for p in APPARMOR_USERNS_PROFILES {
            assert!(a.contains(&format!("profile {p} /opt/k2/podman/bin/")), "{p}");
        }
        assert_eq!(a.matches("userns,").count(), 3);
        assert_eq!(a.matches("flags=(unconfined)").count(), 3);
        let m = marker("x86_64", BUNDLE[0].sha256);
        let v: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["version"], BUNDLE_VERSION);
        assert_eq!(v["sha256"], BUNDLE[0].sha256);
    }
}
