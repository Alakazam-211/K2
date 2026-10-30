//! K2's Podman 5 bundle at box prep (0.46.1 lane P, Rosson 2026-10-16):
//! `k2 app box prepare` on a box whose distro ships Podman < 5 (Ubuntu
//! 24.04, Debian 12) offers to install the pinned bundle through the root
//! helper (`apps podman install`, protocol 8) and then re-checks.
//!
//! - Opt-in only: `installPodman: true` (CLI `--install-podman`, `--yes`,
//!   or a yes at the prompt). Without it, prepare stops before any step
//!   with `podman_install_needed` and says what it would install. Nothing
//!   installs on a daemon upgrade or a boot re-apply.
//! - Podman already 5+ (the distro's or the bundle): nothing changes.
//! - `dryRun: true` prints what prepare would change and runs no
//!   mutating root verb (the doctor's read-only `apps status` only).
//! - The daemon downloads the pinned tarball (or reads `podmanFrom`, an
//!   absolute folder holding it, for air-gapped boxes) and checks its
//!   sha256 first (advisory, so a bad download never costs a root call);
//!   the helper checks it again as root against its own pin table.
//! - `POST /cli/app/box/podman/uninstall` (owner) removes it again.
//! - Each install and uninstall writes an auth-audit row (`apps`) besides
//!   the helper's own audit line.

use std::path::Path;
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_root_proto::podman as pb;
use k2_root_proto::version::PODMAN_INSTALL_MIN_PROTOCOL;
use serde_json::{json, Value as J};

use super::routes::{actor, need_credential, no, Refusal, R};
use crate::compute_routes::Who;
use crate::root_helper;

/// What the owner asked about Podman at prepare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PodmanOpts {
    pub install: bool,
    pub dry_run: bool,
    /// Air-gapped: an absolute folder holding the pinned tarball.
    pub from: Option<String>,
}

pub(crate) fn parse(body: &J) -> R<PodmanOpts> {
    let flag = |k: &str| -> R<bool> {
        match body.get(k) {
            None | Some(J::Null) => Ok(false),
            Some(J::Bool(b)) => Ok(*b),
            Some(_) => Err(no("bad_request", 400, format!("{k} is true or false"))),
        }
    };
    let from = match body.get("podmanFrom") {
        None | Some(J::Null) => None,
        Some(J::String(s)) if s.trim().is_empty() => None,
        Some(J::String(s)) if Path::new(s.trim()).is_absolute() => Some(s.trim().to_string()),
        Some(_) => return Err(no("bad_request", 400, "podmanFrom is an absolute folder")),
    };
    Ok(PodmanOpts { install: flag("installPodman")?, dry_run: flag("dryRun")?, from })
}

/// What box prep does about Podman on this box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Podman 5+ already (the distro's or K2's bundle): nothing.
    Keep { version: String, source: &'static str },
    /// The box is one the bundle is for and its Podman is missing or < 5.
    Offer { have: Option<String>, os: String },
    /// Podman missing or < 5, but the bundle isn't for this OS: the
    /// distro's packages are the path (Debian 13, Fedora and Arch ship 5).
    Distro { have: Option<String> },
}

/// The version gate (pure: the doctor's view of Podman, whether the bundle
/// is installed, and `/etc/os-release`).
pub fn decide(podman: Option<&str>, bundle: bool, os_release: Option<&str>) -> Decision {
    if !pb::needs_install(podman) {
        return Decision::Keep {
            version: podman.unwrap_or_default().to_string(),
            source: if bundle { "k2" } else { "distro" },
        };
    }
    let have = podman.map(str::to_string);
    match os_release.and_then(pb::supported_os) {
        Some(os) => Decision::Offer { have, os },
        None => Decision::Distro { have },
    }
}

fn arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

fn userns_restricted() -> bool {
    std::fs::read_to_string(pb::USERNS_RESTRICT).is_ok_and(|s| s.trim() == "1")
}

/// What an install writes, for the dry run and the refusal that offers it.
pub fn bundle_json(arch: Option<&str>, k2app_uid: Option<u32>, userns_restricted: bool) -> J {
    let art = arch.and_then(|a| pb::artifact_for_arch(a).ok());
    let mut writes = vec![
        json!(pb::PREFIX),
        json!(match k2app_uid {
            Some(u) => format!("{}/{}", pb::rootless_conf_dir(u), pb::ROOTLESS_CONF_FILE),
            None => format!("{}/<k2app uid>/{}", pb::ROOTLESS_CONF_PARENT, pb::ROOTLESS_CONF_FILE),
        }),
        json!(pb::USER_GENERATOR),
    ];
    if userns_restricted {
        writes.push(json!(pb::APPARMOR_USERNS_FILE));
    }
    json!({
        "version": pb::BUNDLE_VERSION,
        "source": pb::BUNDLE_SOURCE,
        "arch": arch,
        "asset": art.map(|a| a.asset),
        "url": art.map(pb::download_url),
        // K2 downloads only from its own mirror (Rosson 2026-10-17); the
        // mirror copied the bytes from here (the sha256 pin is the same).
        "mirror": pb::MIRROR_BASE,
        "mirrors": pb::MIRROR_BASES,
        "urls": art.map(pb::download_urls),
        // The host isn't trusted: the sha256 pin and a K2 release-key
        // signature over this comment are.
        "signature": art.map(pb::signature_name),
        "trustedComment": art.map(pb::bundle_trusted_comment),
        "upstream": art.map(pb::upstream_url),
        "sha256": art.map(|a| a.sha256),
        "size": art.map(|a| a.size),
        "prefix": pb::PREFIX,
        "writes": writes,
        "distroPodman": "a distro Podman already on the box is left as it is (only the k2app user runs K2's Podman)",
    })
}

/// The box's own Podman, as `apps packages install` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxPodman {
    None,
    /// The distro's, at `/usr/bin/podman`.
    Distro,
    /// One the owner built, under /usr/local.
    Local(String),
}

/// The packages `apps packages install` asks the package manager for
/// (`pm`: `apt`, `dnf` or `pacman`). Mirrors the helper's
/// `select_packages` (k2-root-helper `apps.rs`, 0.46.3 F3/F4) plus
/// AppArmor's parser package: keep the two in step.
pub fn planned_packages(pm: &str, bundle: bool, have: &BoxPodman, apparmor: bool) -> Result<Vec<&'static str>, String> {
    use k2_root_proto::apps as ap;
    let (base, aa): (&[&'static str], &[&'static str]) = match pm {
        "apt" => (ap::APT_PACKAGES, ap::APT_APPARMOR_PACKAGES),
        "dnf" => (ap::DNF_PACKAGES, &[]),
        "pacman" => (ap::PACMAN_PACKAGES, ap::PACMAN_APPARMOR_PACKAGES),
        other => return Err(format!("no package list for {other}")),
    };
    let mut out: Vec<&'static str> = if bundle {
        base.iter().copied().filter(|p| !ap::BUNDLE_SHIPS.contains(p)).collect()
    } else {
        match have {
            BoxPodman::Local(path) => {
                return Err(format!(
                    "customer_podman: a Podman K2 didn't install is at {path} and K2's Podman bundle isn't for this OS: prepare stops before installing anything"
                ))
            }
            BoxPodman::Distro => base.iter().copied().filter(|p| *p != ap::PODMAN_PACKAGE).collect(),
            BoxPodman::None => base.to_vec(),
        }
    };
    if apparmor {
        out.extend_from_slice(aa);
    }
    Ok(out)
}

/// What the box can tell the dry run (no root): which files exist, and
/// whether a package is installed (`None`: couldn't ask).
pub struct PackageProbe<'a> {
    pub exists: &'a dyn Fn(&str) -> bool,
    pub installed: &'a dyn Fn(&str, &str) -> Option<bool>,
}

/// The real box: the package manager's own read-only query.
pub fn package_installed(pm: &str, pkg: &str) -> Option<bool> {
    let out = match pm {
        "apt" => std::process::Command::new("/usr/bin/dpkg-query").args(["-W", "-f=${db:Status-Abbrev}", pkg]).output(),
        "dnf" => std::process::Command::new("/usr/bin/rpm").args(["-q", pkg]).output(),
        "pacman" => std::process::Command::new(k2_root_proto::apps::PACMAN).args(["-Q", pkg]).output(),
        _ => return None,
    }
    .ok()?;
    Some(match pm {
        // `ii ` = installed; unknown packages exit non-zero.
        "apt" => out.status.success() && String::from_utf8_lossy(&out.stdout).starts_with("ii"),
        _ => out.status.success(),
    })
}

/// The dry run's `packages`: the exact distro packages prepare would
/// install (only those missing), the ones already there, and what happens
/// to a distro Podman, in words that match the plan (smoke r6 on
/// linux-test: the dry run listed none, and said "the distro's Podman is
/// untouched" while the apply installed the distro's Podman 4.9.3).
/// `bundle`: K2's Podman runs k2app after this prep.
pub fn packages_json(bundle: bool, apparmor: bool, probe: &PackageProbe) -> J {
    use k2_root_proto::apps as ap;
    let pm = if (probe.exists)(ap::APT_GET) {
        "apt"
    } else if (probe.exists)(ap::DNF) {
        "dnf"
    } else if (probe.exists)(ap::PACMAN) {
        "pacman"
    } else {
        return json!({
            "manager": null,
            "install": [],
            "alreadyInstalled": [],
            "note": "no supported package manager (apt-get, dnf, pacman) on this machine: prepare installs nothing here",
        });
    };
    let have = if (probe.exists)(ap::PODMAN) {
        BoxPodman::Distro
    } else if let Some(l) = ap::LOCAL_PODMAN.iter().find(|p| (probe.exists)(p)) {
        BoxPodman::Local(l.to_string())
    } else {
        BoxPodman::None
    };
    let box_podman = match &have {
        BoxPodman::None => J::Null,
        BoxPodman::Distro => json!(ap::PODMAN),
        BoxPodman::Local(p) => json!(p),
    };
    let requested = match planned_packages(pm, bundle, &have, apparmor) {
        Ok(r) => r,
        Err(e) => return json!({ "manager": pm, "install": [], "alreadyInstalled": [], "boxPodman": box_podman, "error": e }),
    };
    let mut install = Vec::new();
    let mut already = Vec::new();
    let mut unknown = false;
    for p in &requested {
        match (probe.installed)(pm, p) {
            Some(true) => already.push(*p),
            Some(false) => install.push(*p),
            None => {
                unknown = true;
                install.push(*p)
            }
        }
    }
    let distro_podman = if install.contains(&ap::PODMAN_PACKAGE) {
        "installs the distro's Podman (the podman package): this box has none yet".to_string()
    } else {
        match &have {
            BoxPodman::Distro if bundle => format!("the distro's Podman at {} stays as it is (only the k2app user runs K2's Podman)", ap::PODMAN),
            BoxPodman::Distro => format!("the distro's Podman at {} stays as it is (K2 doesn't upgrade a Podman it didn't install)", ap::PODMAN),
            BoxPodman::Local(p) => format!("the Podman at {p} stays as it is"),
            BoxPodman::None if bundle => "no distro Podman is installed: K2's Podman in /opt/k2/podman runs the k2app user".to_string(),
            BoxPodman::None => "no Podman is installed".to_string(),
        }
    };
    let mut v = json!({
        "manager": pm,
        "path": if bundle { "bundle" } else { "distro" },
        "install": install,
        "alreadyInstalled": already,
        "requested": requested,
        "boxPodman": box_podman,
        "distroPodman": distro_podman,
    });
    if unknown {
        v["note"] = json!(format!("couldn't ask {pm} which packages are installed: every listed package is shown as to install"));
    }
    v
}

/// The box prep plan with the install in it: `ensure-user` first (the
/// install writes k2app's per-uid Podman config), the install next, then
/// the rest, whose `packages install` now finds Podman 5.
pub fn plan_with_install(base: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let sv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ensure = sv(&["apps", "ensure-user"]);
    let mut out = vec![ensure.clone(), sv(&["apps", "podman", "install"])];
    out.extend(base.into_iter().filter(|a| *a != ensure));
    out
}

pub fn is_install(argv: &[String]) -> bool {
    argv.len() == 3 && argv[0] == "apps" && argv[1] == "podman" && argv[2] == "install"
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// The daemon's advisory check: exactly the pinned tarball.
pub fn check(bytes: &[u8], art: &pb::PodmanArtifact) -> Result<(), String> {
    if bytes.len() > pb::CAP_BUNDLE {
        return Err("the Podman tarball is larger than the install cap".into());
    }
    let got = sha256_hex(bytes);
    if got != art.sha256 {
        return Err(format!(
            "{} sha256 is {got}, the pin is {}: refusing before any root call",
            art.asset, art.sha256
        ));
    }
    Ok(())
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let r = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| format!("http client: {e}"))?
        .get(url)
        .send()
        .map_err(|e| format!("{url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url}: http {}", r.status()));
    }
    r.bytes().map(|b| b.to_vec()).map_err(|e| format!("{url}: {e}"))
}

/// Keys a mirror tarball's `.minisig` may be signed with: the release keys
/// (primary, and the backup, flagged); debug builds also the TEST key.
pub fn keys() -> Vec<k2_root_proto::keys::TrustedKey> {
    k2_root_proto::keys::trusted_keys(cfg!(debug_assertions))
}

/// A tarball that passed both checks, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obtained {
    pub bytes: Vec<u8>,
    pub signer: pb::BundleSigner,
    pub from: String,
}

/// Both checks: the sha256 pin, then the K2 signature over the trusted
/// comment that names this tarball (the mirror host is not trusted).
pub fn verify(bytes: &[u8], sig: &str, art: &pb::PodmanArtifact, keys: &[k2_root_proto::keys::TrustedKey]) -> Result<pb::BundleSigner, String> {
    check(bytes, art)?;
    pb::verify_bundle_sig(bytes, sig, keys, art).map_err(|e| format!("{}.minisig: {e} (refusing before any root call)", art.asset))
}

/// The pinned tarball: from `from` (a folder holding the asset and its
/// `.minisig`) or K2's mirrors in order; checked (sha256 + signature)
/// before it is returned.
pub fn obtain(from: Option<&str>, art: &pb::PodmanArtifact, keys: &[k2_root_proto::keys::TrustedKey]) -> Result<Obtained, String> {
    match from {
        Some(dir) => {
            let d = Path::new(dir);
            if !d.is_absolute() {
                return Err("give podmanFrom as an absolute folder".into());
            }
            let bytes = std::fs::read(d.join(art.asset)).map_err(|e| format!("{}: {e}", d.join(art.asset).display()))?;
            let sig_path = d.join(pb::signature_name(art));
            let sig = std::fs::read_to_string(&sig_path).map_err(|e| format!("{}: {e} (the tarball's K2 signature goes beside it)", sig_path.display()))?;
            let signer = verify(&bytes, &sig, art, keys)?;
            Ok(Obtained { bytes, signer, from: d.display().to_string() })
        }
        None if k2_core::airgap::enabled() => Err(format!(
            "air-gapped: copy {} (sha256 {}) and {} from K2's mirror {} into a folder and prepare with --podman-from <folder>",
            art.asset,
            art.sha256,
            pb::signature_name(art),
            pb::download_url(art)
        )),
        None => {
            let mut why = Vec::new();
            for url in pb::download_urls(art) {
                let got = fetch(&url).and_then(|bytes| {
                    let sig = fetch(&format!("{url}.minisig")).and_then(|b| String::from_utf8(b).map_err(|_| format!("{url}.minisig: not text")))?;
                    let signer = verify(&bytes, &sig, art, keys)?;
                    Ok(Obtained { bytes, signer, from: url.clone() })
                });
                match got {
                    Ok(o) => return Ok(o),
                    Err(e) => why.push(e),
                }
            }
            Err(format!("no K2 mirror gave a good {}: {}", art.asset, why.join("; ")))
        }
    }
}

/// The refusal that offers the install (prepare without `installPodman`).
fn offer_refusal(have: Option<&str>, os: &str, uid: Option<u32>) -> Refusal {
    let mut r = no(
        "podman_install_needed",
        409,
        format!(
            "{os} ships Podman {}; Apps need 5+. Box prep can install K2's pinned Podman {} ({}) for the k2app user only: run k2 app box prepare --install-podman (or --dry-run to see every change first)",
            have.unwrap_or("none"),
            pb::BUNDLE_VERSION,
            pb::BUNDLE_SOURCE
        ),
    );
    r.extra = Some(json!({ "podman": { "action": "offer", "have": have, "os": os, "bundle": bundle_json(arch(), uid, userns_restricted()) } }));
    r
}

/// Before any step runs: `Ok(None)` = nothing to install (Podman 5+ or a
/// box the bundle isn't for); `Ok(Some(tarball))` = install it (the owner
/// opted in, the helper speaks 7, the download checked out); `Err` = stop
/// (not opted in, an old helper, a bad download).
pub(crate) fn prepare_step(o: &PodmanOpts, d: &Decision, st: &root_helper::RootHelperState, uid: Option<u32>) -> R<Option<Obtained>> {
    let (have, os) = match d {
        Decision::Keep { .. } | Decision::Distro { .. } => return Ok(None),
        Decision::Offer { have, os } => (have.as_deref(), os.as_str()),
    };
    if !o.install {
        return Err(offer_refusal(have, os, uid));
    }
    if !st.usable(PODMAN_INSTALL_MIN_PROTOCOL) {
        return Err(no(
            "helper_needed",
            409,
            root_helper::helper_needed("installing K2's Podman", PODMAN_INSTALL_MIN_PROTOCOL, st),
        ));
    }
    let a = arch().ok_or_else(|| no("unsupported", 409, "no K2 Podman bundle for this CPU (x86_64, aarch64)"))?;
    let art = pb::artifact_for_arch(a).map_err(|e| no("unsupported", 409, e))?;
    let got = obtain(o.from.as_deref(), art, &keys()).map_err(|e| no("podman_download_failed", 409, e))?;
    if got.signer.flagged {
        k2_core::log_debug!(
            "[apps] K2's Podman bundle from {} is signed with the BACKUP key {} (accepted; the primary key should sign the mirror)",
            got.from,
            got.signer.key_id
        );
    }
    Ok(Some(got))
}

/// The dry run: what prepare would do, with no mutating root call.
pub fn dry_run(d: &Decision, o: &PodmanOpts, plan: &[Vec<String>], uid: Option<u32>, doctor: J, packages: J) -> J {
    let podman = match d {
        Decision::Keep { version, source } => json!({ "action": "keep", "have": version, "source": source }),
        Decision::Distro { have } => json!({ "action": "distro", "have": have, "note": "the bundle is for Ubuntu 24.04 and Debian 12; this box uses its distro's Podman" }),
        Decision::Offer { have, os } => json!({
            "action": if o.install { "install" } else { "offer" },
            "have": have,
            "os": os,
            "bundle": bundle_json(arch(), uid, userns_restricted()),
            "note": if o.install { "would install" } else { "prepare stops here unless you pass --install-podman" },
        }),
    };
    let steps: Vec<String> = plan.iter().map(|a| a.join(" ")).collect();
    json!({ "ok": true, "dryRun": true, "changed": false, "podman": podman, "packages": packages, "steps": steps, "box": doctor })
}

/// Dry run is a read: anyone who may read the doctor (the owner, Admin
/// logins, agents through their passport) may ask it.
pub(crate) fn may_dry_run(who: &Who) -> R<()> {
    need_credential(who)
}

/// The auth-audit row for an install or uninstall.
pub(crate) fn audit(event: &str, who: &Who, outcome: &str) {
    super::grace::audit(event, &actor(who), outcome);
}

/// `POST /cli/app/box/podman/uninstall {}` (owner).
pub(crate) fn uninstall_route(who: &Who, _body: &J) -> R<J> {
    need_credential(who)?;
    if !matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. }) {
        return Err(no("owner_only", 403, "removing K2's Podman is the owner's: run it from a terminal outside K2"));
    }
    if !cfg!(target_os = "linux") {
        return Err(no("linux_only", 409, "Apps run on Linux boxes only"));
    }
    let st = root_helper::state();
    if !st.usable(PODMAN_INSTALL_MIN_PROTOCOL) {
        return Err(no(
            "helper_needed",
            409,
            root_helper::helper_needed("removing K2's Podman", PODMAN_INSTALL_MIN_PROTOCOL, &st),
        ));
    }
    // 0.46.3 F9: never inside another root sequence.
    let _held = root_helper::busy::try_hold("a K2 Podman removal").map_err(|e| no("root_busy", 409, e))?;
    let res = root_helper::call_text(&["apps".into(), "podman".into(), "uninstall".into()], None);
    root_helper::invalidate();
    match res {
        Ok(text) => {
            let v: J = serde_json::from_str(&text).unwrap_or(J::String(text));
            audit("app-podman-uninstall", who, &format!("ok removed={}", v["removed"]));
            Ok(json!({ "ok": true, "result": v, "box": super::box_prep::doctor(&super::box_prep::gather()) }))
        }
        Err(e) => {
            audit("app-podman-uninstall", who, &format!("failed {e}"));
            Err(no("podman_uninstall_failed", 409, e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Scratch;
    use crate::root_helper::{Kind, RootHelperState};

    const NOBLE: &str = "ID=ubuntu\nVERSION_ID=\"24.04\"\n";
    const BOOKWORM: &str = "ID=debian\nVERSION_ID=\"12\"\n";
    const TRIXIE: &str = "ID=debian\nVERSION_ID=\"13\"\n";

    fn helper(protocol: u32) -> RootHelperState {
        let mut st = RootHelperState::missing();
        st.kind = Kind::Root;
        st.protocol = protocol;
        st
    }

    #[test]
    fn the_version_gate_offers_only_below_5_on_the_bundles_boxes() {
        // 4.9 (Ubuntu 24.04) and 4.3 (Debian 12): offered.
        assert_eq!(
            decide(Some("4.9.3"), false, Some(NOBLE)),
            Decision::Offer { have: Some("4.9.3".into()), os: "ubuntu 24.04".into() }
        );
        assert_eq!(
            decide(Some("4.3.1"), false, Some(BOOKWORM)),
            Decision::Offer { have: Some("4.3.1".into()), os: "debian 12".into() }
        );
        assert_eq!(decide(None, false, Some(NOBLE)), Decision::Offer { have: None, os: "ubuntu 24.04".into() });
        // 5.x: nothing, whoever installed it.
        assert_eq!(decide(Some("5.4.2"), false, Some(TRIXIE)), Decision::Keep { version: "5.4.2".into(), source: "distro" });
        assert_eq!(decide(Some("5.8.8"), true, Some(NOBLE)), Decision::Keep { version: "5.8.8".into(), source: "k2" });
        assert_eq!(decide(Some("5.0.1"), false, Some(NOBLE)), Decision::Keep { version: "5.0.1".into(), source: "distro" });
        // Old Podman on a box the bundle isn't for: the distro's path.
        assert_eq!(decide(Some("4.9.3"), false, Some("ID=ubuntu\nVERSION_ID=\"22.04\"\n")), Decision::Distro { have: Some("4.9.3".into()) });
        assert_eq!(decide(None, false, Some(TRIXIE)), Decision::Distro { have: None });
        assert_eq!(decide(None, false, None), Decision::Distro { have: None });
    }

    #[test]
    fn not_opted_in_stops_before_any_step_and_says_what_it_would_do() {
        let d = decide(Some("4.9.3"), false, Some(NOBLE));
        let e = prepare_step(&PodmanOpts::default(), &d, &helper(8), Some(996)).unwrap_err();
        assert_eq!((e.code, e.status), ("podman_install_needed", 409));
        assert!(e.hint.contains("--install-podman"), "{}", e.hint);
        let x = e.extra.unwrap();
        assert_eq!(x["podman"]["action"], "offer");
        assert_eq!(x["podman"]["bundle"]["version"], pb::BUNDLE_VERSION);
        assert!(x["podman"]["bundle"]["writes"].as_array().unwrap().iter().any(|w| w == pb::USER_GENERATOR));
        // 5+ and non-bundle boxes never stop and never install.
        for d in [
            decide(Some("5.4.2"), false, Some(TRIXIE)),
            decide(Some("4.9.3"), false, Some("ID=fedora\nVERSION_ID=39\n")),
        ] {
            for install in [false, true] {
                let o = PodmanOpts { install, ..Default::default() };
                assert_eq!(prepare_step(&o, &d, &helper(8), None).ok(), Some(None), "{d:?}");
            }
        }
    }

    #[test]
    fn a_helper_older_than_protocol_8_is_never_asked_to_install() {
        // 6 = 0.45.x apps verbs, 7 = 0.46.0 (Sew); the install verb is 8.
        let d = decide(Some("4.9.3"), false, Some(NOBLE));
        let o = PodmanOpts { install: true, ..Default::default() };
        for p in [6, 7] {
            let e = prepare_step(&o, &d, &helper(p), None).unwrap_err();
            assert_eq!(e.code, "helper_needed", "protocol {p}");
            assert!(e.hint.contains('8'), "protocol {p}: {}", e.hint);
        }
    }

    /// The mirror host isn't trusted: the pinned bytes also need a K2
    /// signature naming them; a missing or foreign signature is refused
    /// before any root call; the backup key is accepted and flagged.
    #[test]
    fn a_pinned_tarball_needs_its_k2_signature() {
        use k2_root_proto::testsign::{public_key_b64, sign, sign_with_test_key};
        // A fake artifact whose pin is these bytes' sha256 (the real
        // tarball isn't in the tree).
        let bytes = b"pretend podman tarball".to_vec();
        let pin: &'static str = Box::leak(sha256_hex(&bytes).into_boxed_str());
        let art: &'static pb::PodmanArtifact = Box::leak(Box::new(pb::PodmanArtifact { arch: "x86_64", asset: "podman-linux-amd64.tar.gz", top: "podman-linux-amd64", sha256: pin, size: bytes.len() as u64 }));
        let k = [k2_root_proto::keys::TEST_KEY];
        let good = sign_with_test_key(&bytes, &pb::bundle_trusted_comment(art));
        assert_eq!(verify(&bytes, &good, art, &k).unwrap().role, "test");
        let e = verify(&bytes, &good, art, k2_root_proto::keys::RELEASE_KEYS).unwrap_err();
        assert!(e.contains("not valid for any trusted key") && e.contains("before any root call"), "{e}");
        let wrong = sign_with_test_key(&bytes, "file:podman-linux-amd64.tar.gz version:5.8.7");
        assert!(verify(&bytes, &wrong, art, &k).unwrap_err().contains("trusted comment"));
        // From a folder: the signature goes beside the tarball.
        let d = Scratch::new();
        std::fs::write(d.path().join(art.asset), &bytes).unwrap();
        let e = obtain(Some(d.path().to_str().unwrap()), art, &k).unwrap_err();
        assert!(e.contains(".minisig") && e.contains("signature goes beside it"), "{e}");
        std::fs::write(d.path().join(pb::signature_name(art)), &good).unwrap();
        let o = obtain(Some(d.path().to_str().unwrap()), art, &k).unwrap();
        assert_eq!((o.bytes, o.signer.flagged), (bytes.clone(), false));
        // The backup key: taken, flagged.
        let seed = [9u8; 32];
        let id = [8, 7, 6, 5, 4, 3, 2, 1];
        let pk: &'static str = Box::leak(public_key_b64(&seed, id).into_boxed_str());
        let backup = k2_root_proto::keys::TrustedKey { role: "backup", key_id: "0102030405060708", public_key_b64: pk };
        let sig = sign(&seed, id, &bytes, &pb::bundle_trusted_comment(art));
        let s = verify(&bytes, &sig, art, &[backup]).unwrap();
        assert!(s.flagged && s.role == "backup");
        // The production key list is the release keys (and TEST in debug).
        assert!(keys().iter().any(|k| k.key_id == "18DB69584200F795"));
        assert!(keys().iter().any(|k| k.key_id == "E9C2D2CAD052297A"));
    }

    #[test]
    fn sha256_mismatch_is_refused_before_any_root_call() {
        let art = pb::artifact_for_arch("x86_64").unwrap();
        let e = check(b"not the tarball", art).unwrap_err();
        assert!(e.contains("refusing before any root call"), "{e}");
        // A folder with a wrong file of the right name: refused too.
        let d = std::env::temp_dir().join(format!("k2-podman-from-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let k = [k2_root_proto::keys::TEST_KEY];
        assert!(obtain(Some(d.to_str().unwrap()), art, &k).unwrap_err().contains(art.asset), "missing file named");
        std::fs::write(d.join(art.asset), b"tampered").unwrap();
        std::fs::write(d.join(pb::signature_name(art)), "x").unwrap();
        assert!(obtain(Some(d.to_str().unwrap()), art, &k).unwrap_err().contains("sha256"));
        assert!(obtain(Some("relative/dir"), art, &k).unwrap_err().contains("absolute"));
        std::fs::remove_dir_all(&d).unwrap();
        // The opted-in step surfaces the same refusal (no download: a
        // folder that holds the wrong bytes).
        let d2 = std::env::temp_dir().join(format!("k2-podman-from2-{}", std::process::id()));
        std::fs::create_dir_all(&d2).unwrap();
        for a in pb::BUNDLE {
            std::fs::write(d2.join(a.asset), b"tampered").unwrap();
        }
        let o = PodmanOpts { install: true, dry_run: false, from: Some(d2.to_string_lossy().into()) };
        let e = prepare_step(&o, &decide(Some("4.9.3"), false, Some(NOBLE)), &helper(8), None).unwrap_err();
        assert_eq!(e.code, "podman_download_failed", "{}", e.hint);
        std::fs::remove_dir_all(&d2).unwrap();
    }

    #[test]
    fn the_install_plan_puts_the_user_then_the_bundle_first() {
        let base: Vec<Vec<String>> = super::super::box_prep::plan(&super::super::box_prep::PrepareOpts {
            runtime: k2_root_proto::apps::OciRuntime::Crun,
            cores_mem_min_mib: None,
            budget: None,
        });
        let p: Vec<String> = plan_with_install(base.clone()).iter().map(|a| a.join(" ")).collect();
        assert_eq!(p[0], "apps ensure-user");
        assert_eq!(p[1], "apps podman install");
        assert_eq!(p[2], "apps packages install");
        assert_eq!(p.iter().filter(|s| *s == "apps ensure-user").count(), 1);
        assert_eq!(p.len(), base.len() + 1);
        for argv in plan_with_install(base) {
            let r: Vec<&str> = argv.iter().map(String::as_str).collect();
            assert!(k2_root_proto::Verb::parse(&r).is_ok(), "{argv:?}");
        }
        assert!(is_install(&["apps".into(), "podman".into(), "install".into()]));
        assert!(!is_install(&["apps".into(), "podman".into(), "uninstall".into()]));
    }

    #[test]
    fn dry_run_lists_changes_and_changes_nothing() {
        let base = vec![vec!["apps".to_string(), "packages".into(), "install".into()], vec!["apps".into(), "ensure-user".into()]];
        let d = decide(Some("4.9.3"), false, Some(NOBLE));
        let o = PodmanOpts { install: true, dry_run: true, from: None };
        let v = dry_run(&d, &o, &plan_with_install(base.clone()), Some(996), json!({"ready": false}), json!({"install": ["uidmap"]}));
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["changed"], false);
        assert_eq!(v["podman"]["action"], "install");
        assert_eq!(v["steps"][1], "apps podman install");
        let writes = v["podman"]["bundle"]["writes"].as_array().unwrap();
        assert!(writes.iter().any(|w| w == "/etc/containers/containers.rootless.conf.d/996/90-k2-podman.conf"), "{writes:?}");
        let keep = dry_run(&decide(Some("5.4.2"), false, Some(TRIXIE)), &o, &base, None, json!({}), json!({}));
        assert_eq!(keep["podman"]["action"], "keep");
        assert!(keep["steps"].as_array().unwrap().iter().all(|s| s != "apps podman install"));
    }

    // Server-only: Podman install runs on Linux boxes, and its "/srv/k2" isn't absolute on Windows.
    #[cfg(unix)]
    #[test]
    fn opts_parse_strictly() {
        assert_eq!(parse(&json!({})).ok(), Some(PodmanOpts::default()));
        let o = parse(&json!({"installPodman": true, "dryRun": true, "podmanFrom": "/srv/k2"})).ok().unwrap();
        assert_eq!(o, PodmanOpts { install: true, dry_run: true, from: Some("/srv/k2".into()) });
        for bad in [json!({"installPodman": "yes"}), json!({"dryRun": 1}), json!({"podmanFrom": "rel/dir"}), json!({"podmanFrom": 3})] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    fn podman_row(f: &super::super::box_prep::Facts) -> J {
        let d = super::super::box_prep::doctor(f);
        d["checks"].as_array().unwrap().iter().find(|c| c["id"] == "podman").cloned().expect("podman row")
    }

    #[test]
    fn the_doctor_points_at_the_install_only_where_it_applies() {
        use super::super::box_prep::Facts;
        let f = Facts { linux: true, podman: Some("4.9.3".into()), os_release: Some(NOBLE.into()), ..Facts::default() };
        let r = podman_row(&f);
        assert_eq!(r["status"], "fail");
        assert!(r["fix"].as_str().unwrap().contains("--install-podman"), "{r}");
        let f = Facts { linux: true, podman: None, os_release: Some(BOOKWORM.into()), ..Facts::default() };
        assert!(podman_row(&f)["fix"].as_str().unwrap().contains("--install-podman"));
        // Not a bundle box: the distro list, no install offered.
        let f = Facts { linux: true, podman: Some("4.9.3".into()), os_release: Some("ID=ubuntu\nVERSION_ID=\"22.04\"\n".into()), ..Facts::default() };
        let r = podman_row(&f);
        assert_eq!(r["status"], "fail");
        assert!(r["fix"].is_null(), "{r}");
        // The bundle installed: ok, and says whose it is.
        let f = Facts { linux: true, podman: Some("5.8.8".into()), podman_bundle: true, os_release: Some(NOBLE.into()), ..Facts::default() };
        let r = podman_row(&f);
        assert_eq!(r["status"], "ok");
        assert!(r["detail"].as_str().unwrap().contains("K2's bundle"), "{r}");
        // Distro 5.x: plain ok.
        let f = Facts { linux: true, podman: Some("5.4.2".into()), os_release: Some(TRIXIE.into()), ..Facts::default() };
        assert_eq!(podman_row(&f)["detail"], "5.4.2");
    }

    #[test]
    fn uninstall_is_the_owners() {
        for w in [
            Who::Login { username: "a".into(), role: Role::Admin },
            Who::Agent { project_id: "p".into(), session_id: "s".into() },
            Who::Shell { project_id: "p".into(), session_id: "s".into() },
        ] {
            assert_eq!(uninstall_route(&w, &json!({})).unwrap_err().status, 403, "{w:?}");
        }
        // Dry run is a read anyone with a credential may ask.
        assert!(may_dry_run(&Who::Agent { project_id: "p".into(), session_id: "s".into() }).is_ok());
        assert!(may_dry_run(&Who::Invalid).is_err());
    }

    fn strs(v: &J) -> Vec<String> {
        v.as_array().unwrap_or_else(|| panic!("not a list: {v}")).iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }

    /// Smoke r6 (linux-test): the dry run listed no packages and said "the
    /// distro's Podman is untouched", then the apply apt-installed the
    /// distro's Podman 4.9.3, crun, conmon, passt, skopeo, uidmap,
    /// catatonit… Now it lists exactly the missing packages and says what
    /// happens to Podman.
    #[test]
    fn the_dry_run_lists_the_exact_missing_packages_and_the_podman_truth() {
        use k2_root_proto::apps as ap;
        // A bare apt box: no Podman at all, only nftables installed.
        let exists = |p: &str| p == ap::APT_GET;
        let installed = |pm: &str, pkg: &str| {
            assert_eq!(pm, "apt");
            Some(pkg == "nftables")
        };
        let v = packages_json(false, false, &PackageProbe { exists: &exists, installed: &installed });
        assert_eq!(v["manager"], "apt", "{v}");
        assert_eq!(strs(&v["install"]), vec!["podman", "crun", "passt", "uidmap", "dbus-user-session", "skopeo", "catatonit"], "{v}");
        assert_eq!(strs(&v["alreadyInstalled"]), vec!["nftables"]);
        assert_eq!(v["boxPodman"], J::Null);
        let words = v["distroPodman"].as_str().unwrap();
        assert!(words.contains("installs the distro's Podman"), "{words}");
        assert!(!v.to_string().contains("untouched"), "{v}");
        // The same box with the AppArmor parser missing: apparmor joins.
        let v = packages_json(false, true, &PackageProbe { exists: &exists, installed: &installed });
        assert!(strs(&v["install"]).ends_with(&["apparmor".to_string()]), "{v}");

        // The distro's Podman is there: never in the list, and kept.
        let exists = |p: &str| p == ap::APT_GET || p == ap::PODMAN;
        let installed = |_: &str, pkg: &str| Some(matches!(pkg, "crun" | "uidmap"));
        let v = packages_json(false, false, &PackageProbe { exists: &exists, installed: &installed });
        assert_eq!(strs(&v["install"]), vec!["passt", "dbus-user-session", "skopeo", "catatonit", "nftables"], "{v}");
        assert_eq!(strs(&v["alreadyInstalled"]), vec!["crun", "uidmap"]);
        assert_eq!(v["boxPodman"], "/usr/bin/podman");
        assert!(v["distroPodman"].as_str().unwrap().contains("stays as it is"), "{v}");

        // K2's Podman runs k2app: the bundle's own parts are left out.
        let exists = |p: &str| p == ap::APT_GET;
        let installed = |_: &str, _: &str| Some(false);
        let v = packages_json(true, false, &PackageProbe { exists: &exists, installed: &installed });
        assert_eq!(strs(&v["install"]), vec!["uidmap", "dbus-user-session", "skopeo", "nftables"], "{v}");
        assert_eq!(v["path"], "bundle");
        assert!(v["distroPodman"].as_str().unwrap().contains("no distro Podman is installed"), "{v}");

        // A source-built Podman with no bundle: prepare stops, says so.
        let exists = |p: &str| p == ap::APT_GET || p == "/usr/local/bin/podman";
        let v = packages_json(false, false, &PackageProbe { exists: &exists, installed: &installed });
        assert!(v["error"].as_str().unwrap().starts_with("customer_podman"), "{v}");
        assert_eq!(strs(&v["install"]), Vec::<String>::new());

        // The package manager can't be asked: everything listed, said so.
        let installed = |_: &str, _: &str| None;
        let v = packages_json(true, false, &PackageProbe { exists: &|p: &str| p == ap::DNF, installed: &installed });
        assert_eq!(v["manager"], "dnf");
        assert_eq!(strs(&v["install"]), vec!["shadow-utils", "skopeo"], "{v}");
        assert!(v["note"].as_str().unwrap().contains("couldn't ask dnf"), "{v}");

        // No package manager (a Mac): nothing to install, said plainly.
        let v = packages_json(false, false, &PackageProbe { exists: &|_: &str| false, installed: &installed });
        assert_eq!((v["manager"].clone(), strs(&v["install"])), (J::Null, vec![]));

        // The dry run carries it, text and JSON alike.
        let d = decide(None, false, Some("ID=ubuntu\nVERSION_ID=\"22.04\"\n"));
        let out = dry_run(&d, &PodmanOpts { dry_run: true, ..Default::default() }, &[], None, json!({}), json!({"install": ["podman"]}));
        assert_eq!(strs(&out["packages"]["install"]), vec!["podman"]);
    }
}
