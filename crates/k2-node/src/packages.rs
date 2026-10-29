//! System packages for jobs (compute polish v1): root, so only from the
//! machine owner's approved list.
//!
//! - The list lives in [`APPROVED_FILE`] (`/etc/k2-node-packages.toml`,
//!   `root:root 0644`), outside the `k2nodectl`-writable config dir, so
//!   neither a job (uid `k2node`) nor the human's agents (group
//!   `k2nodectl`) can add to it. Root edits it: `sudo k2-node packages
//!   allow <name>…`, or the installer's `--allow-packages`, or (later) the
//!   root helper's `node packages-allow` verb.
//! - Linux: `k2-node` (as `k2node`, under `NoNewPrivileges`, so no sudo)
//!   drops a request `{id, packages}` into `<home>/run/pkg-requests/`.
//!   A root systemd path unit the installer adds
//!   (`k2-node-packages.path` → `k2-node-packages.service`) runs
//!   `k2-node packages serve`: it opens each request without following
//!   links (owner must be the node user, small, regular), deletes it,
//!   re-reads the root-owned list itself, refuses anything not on it, runs
//!   the distro's package manager by absolute path with a clean env and
//!   `--` before the names, and writes the answer into a root-owned
//!   directory (`/run/k2-node-packages/<id>.json`) the node reads. The job
//!   side never gets root and never names anything but package names.
//! - macOS: there is no root path; a missing package fails the job with
//!   the exact `brew install` line for the Mac's owner.

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_node_proto::needs::valid_package;
use serde::{Deserialize, Serialize};

use crate::toolchain::{codes, Fail, Toolchains};

pub const APPROVED_FILE: &str = "/etc/k2-node-packages.toml";
/// The root path unit that serves package requests (Linux).
pub const PATH_UNIT: &str = "/etc/systemd/system/k2-node-packages.path";
/// Requests, relative to the node home (owned by the node user).
pub const REQUEST_DIR: &str = "run/pkg-requests";
/// Answers (root-owned, world-readable).
pub const RESULT_DIR: &str = "/run/k2-node-packages";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkgRequest {
    pub id: String,
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkgResult {
    pub id: String,
    pub ok: bool,
    pub message: String,
}

fn valid_req_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Does this node have the root package installer?
pub fn installer_present() -> bool {
    cfg!(target_os = "linux") && Path::new(PATH_UNIT).is_file() && detect().is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Apt => "apt",
            Manager::Dnf => "dnf",
            Manager::Pacman => "pacman",
            Manager::Zypper => "zypper",
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            Manager::Apt => "/usr/bin/apt-get",
            Manager::Dnf => "/usr/bin/dnf",
            Manager::Pacman => "/usr/bin/pacman",
            Manager::Zypper => "/usr/bin/zypper",
        }
    }

    /// Fixed argv; the names come last, after `--`.
    pub fn install_args(self, pkgs: &[String]) -> Vec<String> {
        let mut a: Vec<String> = match self {
            Manager::Apt => vec!["install", "-y", "-q", "--no-install-recommends"],
            Manager::Dnf => vec!["install", "-y", "-q"],
            Manager::Pacman => vec!["-S", "--needed", "--noconfirm"],
            Manager::Zypper => vec!["--non-interactive", "install"],
        }
        .into_iter()
        .map(str::to_string)
        .collect();
        a.push("--".into());
        a.extend(pkgs.iter().cloned());
        a
    }
}

/// The distro package manager, by absolute path (never PATH).
pub fn detect() -> Option<Manager> {
    [Manager::Apt, Manager::Dnf, Manager::Pacman, Manager::Zypper].into_iter().find(|m| Path::new(m.program()).is_file())
}

fn quiet(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| {
            o.status.success() && {
                let s = String::from_utf8_lossy(&o.stdout);
                // dpkg-query prints a status even for removed packages.
                !program.ends_with("dpkg-query") || s.contains("install ok installed")
            }
        })
        .unwrap_or(false)
}

/// Is `pkg` installed? Read-only, no root.
pub fn installed(os: &str, pkg: &str) -> bool {
    if !valid_package(pkg) {
        return false;
    }
    if os == "macos" {
        return ["/opt/homebrew/opt", "/usr/local/opt"].iter().any(|d| Path::new(d).join(pkg).exists());
    }
    match detect() {
        Some(Manager::Apt) => quiet("/usr/bin/dpkg-query", &["-W", "-f=${Status}", pkg]),
        Some(Manager::Dnf) | Some(Manager::Zypper) => quiet("/usr/bin/rpm", &["-q", "--", pkg]),
        Some(Manager::Pacman) => quiet("/usr/bin/pacman", &["-Q", "--", pkg]),
        None => false,
    }
}

/// Parse `approved = ["a", "b"]`; bad names are refused, not skipped.
pub fn parse_approved(text: &str) -> Result<Vec<String>, String> {
    let t: toml::Table = text.parse().map_err(|e| format!("approved packages file: {e}"))?;
    let mut out = Vec::new();
    for v in t.get("approved").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let s = v.as_str().ok_or("approved entries are strings")?;
        if !valid_package(s) {
            return Err(format!("approved package {s:?} isn't a valid name"));
        }
        if !out.iter().any(|o: &String| o == s) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

pub fn render_approved(pkgs: &[String]) -> String {
    let mut sorted: Vec<String> = pkgs.to_vec();
    sorted.sort();
    sorted.dedup();
    let list = sorted.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(", ");
    format!(
        "# System packages compute jobs may install on this node (root installs them\n\
         # through `sudo k2-node packages install`, never anything else).\n\
         # Edit with: sudo k2-node packages allow <name>... | disallow <name>...\n\
         approved = [{list}]\n"
    )
}

/// Read the list (missing file = empty).
pub fn read_approved(path: &Path) -> Result<Vec<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse_approved(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// As root: the file and its directory must be root-owned and writable
/// by nobody else, or the list isn't the owner's word.
fn trusted_owner_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    for p in [path, path.parent().unwrap_or(Path::new("/"))] {
        let m = match std::fs::symlink_metadata(p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && p == path => return Ok(()),
            Err(e) => return Err(format!("stat {}: {e}", p.display())),
        };
        if m.file_type().is_symlink() {
            return Err(format!("{} is a symlink; refusing", p.display()));
        }
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err(format!("{} must be owned by root and writable only by root", p.display()));
        }
    }
    Ok(())
}

/// What the job side does (as `k2node`): install what's missing, if the
/// owner approved it. `Ok(true)` when something was installed.
pub async fn ensure(tc: &Toolchains, pkgs: &[String], log: &mut (dyn FnMut(&str) + Send)) -> Result<bool, Fail> {
    let missing: Vec<String> = pkgs.iter().filter(|p| !installed(tc.os, p)).cloned().collect();
    if missing.is_empty() {
        return Ok(false);
    }
    if tc.os == "macos" {
        return Err((
            codes::PACKAGE_MISSING,
            format!(
                "missing on this Mac: {m}. Jobs can't install system packages on macOS; the Mac's owner runs once: brew install {m}",
                m = missing.join(" ")
            ),
        ));
    }
    let approved = read_approved(&tc.packages_file).map_err(|e| (codes::PACKAGE_NOT_APPROVED, e))?;
    let refused: Vec<String> = missing.iter().filter(|p| !approved.contains(p)).cloned().collect();
    if !refused.is_empty() {
        return Err((
            codes::PACKAGE_NOT_APPROVED,
            format!(
                "{r} isn't on this node's approved package list ({f}). The machine's owner approves it once with: sudo k2-node packages allow {r}",
                r = refused.join(" "),
                f = tc.packages_file.display()
            ),
        ));
    }
    let Some(m) = detect() else {
        return Err((codes::PACKAGE_INSTALL_FAILED, "no supported package manager (apt, dnf, pacman, zypper) on this node".into()));
    };
    if !installer_present() {
        return Err((
            codes::PACKAGE_INSTALL_FAILED,
            format!(
                "this node has no package installer ({PATH_UNIT}); re-run `sudo k2-node setup` on it, or its owner installs by hand: {} {}",
                m.program(),
                m.install_args(&missing).join(" ")
            ),
        ));
    }
    log(&format!("installing system packages ({}): {}", m.name(), missing.join(" ")));
    let home = tc.root.parent().unwrap_or(Path::new("/")).to_path_buf();
    let r = request_and_wait(&home, Path::new(RESULT_DIR), &missing, Duration::from_secs(15 * 60))
        .await
        .map_err(|e| (codes::PACKAGE_INSTALL_FAILED, e))?;
    if !r.ok {
        return Err((codes::PACKAGE_INSTALL_FAILED, r.message));
    }
    let still: Vec<String> = missing.iter().filter(|p| !installed(tc.os, p)).cloned().collect();
    if !still.is_empty() {
        return Err((codes::PACKAGE_INSTALL_FAILED, format!("{} still not installed after the package manager ran", still.join(" "))));
    }
    Ok(true)
}

/// Job side: drop a request where the root path unit looks, wait for its
/// answer in `result_dir`.
pub async fn request_and_wait(home: &Path, result_dir: &Path, pkgs: &[String], limit: Duration) -> Result<PkgResult, String> {
    let id = format!("pkg-{}", k2_node_proto::crypto::random_id()?);
    let req = PkgRequest { id: id.clone(), packages: pkgs.to_vec() };
    let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    let dir = home.join(REQUEST_DIR);
    // Stage outside the watched dir, then rename in: the path unit must
    // never see a half-written (or temp) file.
    let stage = home.join("run").join(format!(".{id}.json"));
    crate::util::atomic_write(&stage, &body, 0o600)?;
    std::fs::rename(&stage, dir.join(format!("{id}.json"))).map_err(|e| {
        let _ = std::fs::remove_file(&stage);
        format!("queue the package request in {}: {e}", dir.display())
    })?;
    let answer = result_dir.join(format!("{id}.json"));
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if let Ok(s) = std::fs::read_to_string(&answer) {
            if let Ok(r) = serde_json::from_str::<PkgResult>(&s) {
                if r.id == id {
                    return Ok(r);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("no answer from the package installer in {}s (journalctl -u k2-node-packages)", limit.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Read one request file as root: no symlinks, owned by `owner_uid`,
/// regular, small. Deleted once read.
fn take_request(p: &Path, owner_uid: u32) -> Result<PkgRequest, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(p)
        .map_err(|e| format!("open: {e}"));
    let _ = std::fs::remove_file(p);
    let mut f = f?;
    let m = f.metadata().map_err(|e| format!("stat: {e}"))?;
    if !m.is_file() || m.uid() != owner_uid || m.len() > 4096 {
        return Err("not a small regular file owned by the node user".into());
    }
    let mut s = String::new();
    f.read_to_string(&mut s).map_err(|_| "not text".to_string())?;
    let r: PkgRequest = serde_json::from_str(&s).map_err(|_| "not a package request".to_string())?;
    if !valid_req_id(&r.id) {
        return Err("bad request id".into());
    }
    Ok(r)
}

/// `k2-node packages serve --home H` (root; the path unit's service):
/// answer every waiting request, then exit.
pub fn cmd_serve(home: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if !is_root() {
        return Err("packages serve runs as root (k2-node-packages.service)".into());
    }
    let owner = std::fs::metadata(home).map_err(|e| format!("{}: {e}", home.display()))?.uid();
    if owner == 0 {
        return Err(format!("{} is owned by root; refusing", home.display()));
    }
    let out = Path::new(RESULT_DIR);
    std::fs::create_dir_all(out).map_err(|e| format!("create {RESULT_DIR}: {e}"))?;
    let om = std::fs::symlink_metadata(out).map_err(|e| e.to_string())?;
    if om.uid() != 0 || om.file_type().is_symlink() || om.mode() & 0o022 != 0 {
        return Err(format!("{RESULT_DIR} must be a root-owned directory"));
    }
    // Old answers go after an hour.
    if let Ok(rd) = std::fs::read_dir(out) {
        for e in rd.flatten() {
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|a| a.as_secs() > 3600);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let dir = home.join(REQUEST_DIR);
    let entries: Vec<PathBuf> = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let req = match take_request(&p, owner) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("k2-node packages: dropped {name}: {e}");
                continue;
            }
        };
        let r = cmd_install(&req.packages);
        let res = PkgResult {
            id: req.id.clone(),
            ok: r.is_ok(),
            message: match &r {
                Ok(()) => format!("installed {}", req.packages.join(" ")),
                Err(e) => e.clone(),
            },
        };
        eprintln!("k2-node packages: {} → {}", req.packages.join(" "), res.message);
        let body = serde_json::to_vec(&res).map_err(|e| e.to_string())?;
        crate::util::atomic_write(&out.join(format!("{}.json", req.id)), &body, 0o644)?;
    }
    Ok(())
}

// ── root side: `k2-node packages …` ──────────────────────────────────

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// `k2-node packages install <name>…` (root: an admin by hand, or `serve`).
pub fn cmd_install(names: &[String]) -> Result<(), String> {
    if !is_root() {
        return Err("packages install runs as root (k2-node calls it through sudo)".into());
    }
    if names.is_empty() || names.len() > k2_node_proto::needs::MAX_NEEDS {
        return Err("packages install <name>… (1–32 names)".into());
    }
    for n in names {
        if !valid_package(n) {
            return Err(format!("{n:?} isn't a package name"));
        }
    }
    let path = PathBuf::from(APPROVED_FILE);
    trusted_owner_only(&path)?;
    let approved = read_approved(&path)?;
    let refused: Vec<&String> = names.iter().filter(|n| !approved.contains(n)).collect();
    if !refused.is_empty() {
        return Err(format!(
            "not approved on this node: {} (sudo k2-node packages allow <name>)",
            refused.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")
        ));
    }
    let m = detect().ok_or("no supported package manager (apt, dnf, pacman, zypper)")?;
    let run = |args: &[String]| -> Result<bool, String> {
        let st = std::process::Command::new(m.program())
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("DEBIAN_FRONTEND", "noninteractive")
            .env("LANG", "C")
            .current_dir("/")
            .stdin(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("{}: {e}", m.program()))?;
        Ok(st.success())
    };
    if run(&m.install_args(names))? {
        return Ok(());
    }
    // apt: a stale index is the usual reason; refresh once and retry.
    if m == Manager::Apt && run(&["update".into(), "-q".into()])? && run(&m.install_args(names))? {
        return Ok(());
    }
    Err(format!("{} couldn't install {}", m.name(), names.join(" ")))
}

/// `k2-node packages allow|disallow <name>…` (root).
pub fn cmd_edit(names: &[String], allow: bool) -> Result<Vec<String>, String> {
    if !is_root() {
        return Err(format!("run as root: sudo k2-node packages {} …", if allow { "allow" } else { "disallow" }));
    }
    edit_approved(Path::new(APPROVED_FILE), names, allow, true)
}

/// Add or remove names on an approved list, atomically. `root_checks`:
/// the file and its folder must be root's and writable only by root (the
/// real list; off only for a test executor's own file).
pub fn edit_approved(path: &Path, names: &[String], allow: bool, root_checks: bool) -> Result<Vec<String>, String> {
    for n in names {
        if !valid_package(n) {
            return Err(format!("{n:?} isn't a package name"));
        }
    }
    let path = path.to_path_buf();
    if root_checks {
        trusted_owner_only(&path)?;
    }
    let mut list = read_approved(&path)?;
    if allow {
        for n in names {
            if !list.contains(n) {
                list.push(n.clone());
            }
        }
    } else {
        list.retain(|p| !names.contains(p));
    }
    crate::util::atomic_write(&path, render_approved(&list).as_bytes(), 0o644)?;
    let mut sorted = list;
    sorted.sort();
    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_file_round_trips_and_refuses_junk() {
        let l = parse_approved(&render_approved(&["pkg-config".into(), "libssl-dev".into(), "pkg-config".into()])).unwrap();
        assert_eq!(l, vec!["libssl-dev", "pkg-config"]);
        assert!(parse_approved("approved = [\"-y\"]").is_err());
        assert!(parse_approved("approved = [\"a b\"]").is_err());
        assert!(parse_approved("approved = [1]").is_err());
        assert_eq!(parse_approved("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn install_argv_puts_names_after_double_dash() {
        let a = Manager::Apt.install_args(&["libssl-dev".into()]);
        assert_eq!(a, vec!["install", "-y", "-q", "--no-install-recommends", "--", "libssl-dev"]);
        let p = Manager::Pacman.install_args(&["protobuf".into()]);
        assert_eq!(p.last().map(String::as_str), Some("protobuf"));
        assert_eq!(p[p.len() - 2], "--");
    }

    #[test]
    fn root_commands_refuse_non_root_and_unapproved() {
        let ghost = "k2-test-no-such-package-xyz".to_string();
        if is_root() {
            // As root (the build box): off the list → refused, nothing run.
            let e = cmd_install(&[ghost]).unwrap_err();
            assert!(e.contains("not approved"), "{e}");
            assert!(cmd_install(&["-y".into()]).unwrap_err().contains("isn't a package name"));
        } else {
            assert!(cmd_install(&[ghost.clone()]).unwrap_err().contains("root"));
            assert!(cmd_edit(&[ghost], true).unwrap_err().contains("root"));
        }
    }

    #[tokio::test]
    async fn requests_round_trip_through_the_watched_dir() {
        let home = crate::util::temp_dir("pkg-req");
        std::fs::create_dir_all(home.join(REQUEST_DIR)).unwrap();
        let results = home.join("results");
        std::fs::create_dir_all(&results).unwrap();
        // A stand-in for the root service: answer whatever arrives.
        let (h2, r2) = (home.clone(), results.clone());
        let server = tokio::spawn(async move {
            let uid = unsafe { libc::geteuid() };
            for _ in 0..200 {
                for e in std::fs::read_dir(h2.join(REQUEST_DIR)).unwrap().flatten() {
                    let req = take_request(&e.path(), uid).unwrap();
                    let res = PkgResult { id: req.id.clone(), ok: true, message: format!("installed {}", req.packages.join(" ")) };
                    std::fs::write(r2.join(format!("{}.json", req.id)), serde_json::to_vec(&res).unwrap()).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("no request arrived");
        });
        let r = request_and_wait(&home, &results, &["pkg-config".into()], Duration::from_secs(10)).await.unwrap();
        server.await.unwrap();
        assert!(r.ok);
        assert_eq!(r.message, "installed pkg-config");
        assert_eq!(std::fs::read_dir(home.join(REQUEST_DIR)).unwrap().count(), 0, "requests are consumed");
        assert!(!home.join("run").read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with('.')), "no stage left");
    }

    #[test]
    fn root_reads_requests_without_following_links_or_trusting_strangers() {
        let d = crate::util::temp_dir("pkg-take");
        let uid = unsafe { libc::geteuid() };
        let target = d.join("secret");
        std::fs::write(&target, r#"{"id":"pkg-12345678","packages":["x"]}"#).unwrap();
        let link = d.join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(take_request(&link, uid).is_err(), "a symlink is never followed");
        assert!(target.exists(), "the link's target is untouched");
        let good = d.join("good.json");
        std::fs::write(&good, r#"{"id":"pkg-12345678","packages":["x"]}"#).unwrap();
        assert!(take_request(&good, uid.wrapping_add(1)).is_err(), "someone else's file is dropped");
        let big = d.join("big.json");
        std::fs::write(&big, vec![b' '; 5000]).unwrap();
        assert!(take_request(&big, uid).is_err());
        let bad_id = d.join("bad.json");
        std::fs::write(&bad_id, r#"{"id":"../x","packages":["x"]}"#).unwrap();
        assert!(take_request(&bad_id, uid).is_err());
        let ok = d.join("ok.json");
        std::fs::write(&ok, r#"{"id":"pkg-12345678","packages":["pkg-config"]}"#).unwrap();
        assert_eq!(take_request(&ok, uid).unwrap().packages, vec!["pkg-config"]);
        assert!(!ok.exists(), "read once, then gone");
    }

    #[tokio::test]
    async fn a_package_off_the_list_is_refused_before_any_root_call() {
        let home = crate::util::temp_dir("pkg-refuse");
        let mut tc = Toolchains::new(&home, vec![("PATH".into(), "/usr/bin:/bin".into())]);
        tc.packages_file = home.join("approved.toml");
        std::fs::write(&tc.packages_file, render_approved(&["pkg-config".into()])).unwrap();
        let ghost = "k2-test-no-such-package-xyz".to_string();
        let e = ensure(&tc, &[ghost.clone()], &mut |_| {}).await.unwrap_err();
        if tc.os == "macos" {
            assert_eq!(e.0, codes::PACKAGE_MISSING);
            assert!(e.1.contains(&format!("brew install {ghost}")), "{}", e.1);
        } else {
            assert_eq!(e.0, codes::PACKAGE_NOT_APPROVED);
            assert!(e.1.contains(&format!("sudo k2-node packages allow {ghost}")), "{}", e.1);
        }
    }
}
