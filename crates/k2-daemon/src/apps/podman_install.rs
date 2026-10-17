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
        "sha256": art.map(|a| a.sha256),
        "size": art.map(|a| a.size),
        "prefix": pb::PREFIX,
        "writes": writes,
        "distroPodman": "left as it is (only the k2app user runs K2's Podman)",
    })
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

/// The pinned tarball: from `from` (a folder holding the asset) or the
/// pinned URL; checked before it is returned.
pub fn obtain(from: Option<&str>, art: &pb::PodmanArtifact) -> Result<Vec<u8>, String> {
    let bytes = match from {
        Some(dir) => {
            let d = Path::new(dir);
            if !d.is_absolute() {
                return Err("give podmanFrom as an absolute folder".into());
            }
            std::fs::read(d.join(art.asset)).map_err(|e| format!("{}: {e}", d.join(art.asset).display()))?
        }
        None if k2_core::airgap::enabled() => {
            return Err(format!(
                "air-gapped: copy {} (sha256 {}) from {} into a folder and prepare with --podman-from <folder>",
                art.asset,
                art.sha256,
                pb::download_url(art)
            ))
        }
        None => fetch(&pb::download_url(art))?,
    };
    check(&bytes, art)?;
    Ok(bytes)
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
pub(crate) fn prepare_step(o: &PodmanOpts, d: &Decision, st: &root_helper::RootHelperState, uid: Option<u32>) -> R<Option<Vec<u8>>> {
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
    let bytes = obtain(o.from.as_deref(), art).map_err(|e| no("podman_download_failed", 409, e))?;
    Ok(Some(bytes))
}

/// The dry run: what prepare would do, with no mutating root call.
pub fn dry_run(d: &Decision, o: &PodmanOpts, plan: &[Vec<String>], uid: Option<u32>, doctor: J) -> J {
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
    json!({ "ok": true, "dryRun": true, "changed": false, "podman": podman, "steps": steps, "box": doctor })
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

    #[test]
    fn sha256_mismatch_is_refused_before_any_root_call() {
        let art = pb::artifact_for_arch("x86_64").unwrap();
        let e = check(b"not the tarball", art).unwrap_err();
        assert!(e.contains("refusing before any root call"), "{e}");
        // A folder with a wrong file of the right name: refused too.
        let d = std::env::temp_dir().join(format!("k2-podman-from-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(obtain(Some(d.to_str().unwrap()), art).unwrap_err().contains(art.asset), "missing file named");
        std::fs::write(d.join(art.asset), b"tampered").unwrap();
        assert!(obtain(Some(d.to_str().unwrap()), art).unwrap_err().contains("sha256"));
        assert!(obtain(Some("relative/dir"), art).unwrap_err().contains("absolute"));
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
        let v = dry_run(&d, &o, &plan_with_install(base.clone()), Some(996), json!({"ready": false}));
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["changed"], false);
        assert_eq!(v["podman"]["action"], "install");
        assert_eq!(v["steps"][1], "apps podman install");
        let writes = v["podman"]["bundle"]["writes"].as_array().unwrap();
        assert!(writes.iter().any(|w| w == "/etc/containers/containers.rootless.conf.d/996/90-k2-podman.conf"), "{writes:?}");
        let keep = dry_run(&decide(Some("5.4.2"), false, Some(TRIXIE)), &o, &base, None, json!({}));
        assert_eq!(keep["podman"]["action"], "keep");
        assert!(keep["steps"].as_array().unwrap().iter().all(|s| s != "apps podman install"));
    }

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
}
