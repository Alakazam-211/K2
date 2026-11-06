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
use k2_node_proto::pkgmap::{Mgr, SysName};
use k2_node_proto::pkgpolicy::{codes as pcodes, coverage, Coverage, PolicyFile};
use serde::{Deserialize, Serialize};

use crate::toolchain::{codes, Fail, Toolchains};

pub const APPROVED_FILE: &str = "/etc/k2-node-packages.toml";
/// The owner's package policy per server (root's; protocol 7).
pub const POLICY_FILE: &str = k2_node_proto::pkgpolicy::POLICY_FILE;
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
    /// Whose policy covers it (protocol 7; absent = the owner's, as before).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<RequestServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PkgResult {
    pub id: String,
    pub ok: bool,
    pub message: String,
    /// The refusal code when `ok` is false (protocol 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

fn valid_req_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Does this node have the root package installer?
pub fn installer_present() -> bool {
    cfg!(target_os = "linux") && Path::new(PATH_UNIT).is_file() && detect().is_some()
}

/// The distro package manager, by absolute path (never PATH).
pub fn detect() -> Option<Mgr> {
    crate::pkgsys::detect()
}

/// Which server a job (or a request) is for: its policy line. The node's
/// own server is `owner`; another is matched by its key fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestServer {
    pub owner: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fp: String,
}

impl RequestServer {
    pub fn owner() -> Self {
        RequestServer { owner: true, fp: String::new() }
    }
}

/// Is `pkg` (canonical, or `<manager>:<name>`) installed? Read-only, no
/// root. Virtual names count through their one provider (§5.2).
pub fn installed(os: &str, pkg: &str) -> bool {
    if os == "macos" {
        return valid_package(pkg) && ["/opt/homebrew/opt", "/usr/local/opt"].iter().any(|d| Path::new(d).join(pkg).exists());
    }
    match (crate::pkgsys::host_ctx(), crate::pkgsys::system()) {
        (Some(h), Some(b)) => crate::pkgsys::name_installed(&b, &h, pkg),
        _ => false,
    }
}

/// The owner's package policy file (missing = every server at `list`,
/// today's rule).
pub fn read_policy(path: &Path) -> Result<PolicyFile, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => PolicyFile::parse(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PolicyFile::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Root's approved list and policy file for a config dir: the real ones
/// for `/etc/k2-node`, a test's own copies beside any other.
pub fn files_for(config_dir: &Path) -> (PathBuf, PathBuf) {
    if config_dir == Path::new(crate::paths::DEFAULT_CONFIG_DIR) {
        (PathBuf::from(APPROVED_FILE), PathBuf::from(POLICY_FILE))
    } else {
        (config_dir.join("k2-node-packages.toml"), config_dir.join("k2-node-package-policy.toml"))
    }
}

/// Root: write the policy file atomically (root's, 0644).
pub fn write_policy(path: &Path, f: &PolicyFile, root_checks: bool) -> Result<(), String> {
    if root_checks {
        trusted_owner_only(path)?;
    }
    crate::util::atomic_write(path, f.render().as_bytes(), 0o644)
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
/// owner's list or this server's package policy covers it. `Ok(true)` when
/// something was installed. `pkgs` are canonical (Debian) names or
/// `<manager>:<name>`.
pub async fn ensure(tc: &Toolchains, pkgs: &[String], log: &mut (dyn FnMut(&str) + Send)) -> Result<bool, Fail> {
    if tc.os == "macos" {
        let missing: Vec<String> = pkgs.iter().filter(|p| !installed(tc.os, p)).cloned().collect();
        if missing.is_empty() {
            return Ok(false);
        }
        return Err((
            codes::PACKAGE_MISSING,
            format!(
                "missing on this Mac: {m}. Jobs can't install system packages on macOS; the Mac's owner runs once: brew install {m}",
                m = missing.join(" ")
            ),
        ));
    }
    let (Some(h), Some(b)) = (crate::pkgsys::host_ctx(), crate::pkgsys::system()) else {
        return Err((codes::PACKAGE_INSTALL_FAILED, "no supported package manager (apt, dnf, pacman, zypper) on this node".into()));
    };
    ensure_with(tc, &h, &b, pkgs, log).await
}

/// [`ensure`] on a given host and backend (tests pass a fake one).
pub async fn ensure_with(tc: &Toolchains, h: &crate::pkgsys::HostCtx, b: &dyn crate::pkgsys::Backend, pkgs: &[String], log: &mut (dyn FnMut(&str) + Send)) -> Result<bool, Fail> {
    let unmapped: Vec<&String> = pkgs.iter().filter(|p| h.natives(p).is_none()).collect();
    if !unmapped.is_empty() {
        return Err((
            pcodes::NO_PACKAGE_MAPPING,
            format!(
                "no {m} name for {u} on this {d} node. It can't be requested: use a native name (sys:{m}:<name>), pick another distro (--distro ubuntu), or another node",
                m = h.mgr.name(),
                u = unmapped.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                d = h.distro
            ),
        ));
    }
    let missing: Vec<String> = pkgs.iter().filter(|p| !crate::pkgsys::name_installed(b, h, p)).cloned().collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let approved = read_approved(&tc.packages_file).map_err(|e| (codes::PACKAGE_NOT_APPROVED, e))?;
    let policy = read_policy(&tc.policy_file).map_err(|e| (codes::PACKAGE_NOT_APPROVED, e))?.for_server(tc.server.owner, &tc.server.fp);
    let mut refused = Vec::new();
    for p in &missing {
        match coverage(&policy, &approved, p, &h.natives(p).unwrap_or_default()) {
            Coverage::Denied(d) => {
                return Err((
                    pcodes::PACKAGE_DENIED,
                    format!("{p} is on K2's deny list ({d}): it changes how the machine boots, logs in or starts services. It can't be requested; the machine's owner installs it by hand if they want it"),
                ))
            }
            Coverage::Ask => refused.push(p.clone()),
            _ => {}
        }
    }
    if !refused.is_empty() {
        return Err((
            codes::PACKAGE_NOT_APPROVED,
            format!(
                "{r} isn't on this node's approved package list ({f}) or in its package policy for this server ({l}). Ask the node's owner from K2: k2 compute node packages request <node> {c} --why \"…\"; or the machine's owner approves it once with: sudo k2-node packages allow {r}",
                r = refused.join(" "),
                c = refused.join(","),
                f = tc.packages_file.display(),
                l = policy.summary()
            ),
        ));
    }
    if !tc.pkg_installer {
        return Err((
            codes::PACKAGE_INSTALL_FAILED,
            format!(
                "this node has no package installer ({PATH_UNIT}); re-run `sudo k2-node setup` on it, or its owner installs by hand: sudo k2-node packages install {}",
                missing.join(" ")
            ),
        ));
    }
    log(&format!("installing system packages ({}): {}", h.mgr.name(), missing.join(" ")));
    let home = tc.root.parent().unwrap_or(Path::new("/")).to_path_buf();
    let r = request_and_wait(&home, Path::new(RESULT_DIR), &missing, &tc.server, Duration::from_secs(45 * 60))
        .await
        .map_err(|e| (codes::PACKAGE_INSTALL_FAILED, e))?;
    if !r.ok {
        let code = r.code.as_deref().and_then(known_code).unwrap_or(codes::PACKAGE_INSTALL_FAILED);
        return Err((code, r.message));
    }
    for line in r.message.lines().skip(1) {
        log(line);
    }
    let still: Vec<String> = missing.iter().filter(|p| !crate::pkgsys::name_installed(b, h, p)).cloned().collect();
    if !still.is_empty() {
        return Err((codes::PACKAGE_INSTALL_FAILED, format!("{} still not installed after the package manager ran", still.join(" "))));
    }
    Ok(true)
}

/// A root answer's code, as one of ours (never a string from the file).
fn known_code(c: &str) -> Option<&'static str> {
    [
        pcodes::PACKAGE_NOT_APPROVED,
        pcodes::NO_PACKAGE_MAPPING,
        pcodes::PACKAGE_AMBIGUOUS,
        pcodes::PACKAGE_DENIED,
        pcodes::PACKAGE_REMOVES,
        pcodes::PACKAGE_THIRD_PARTY,
        pcodes::PACKAGE_TOO_BIG,
        pcodes::PKG_SIGNING_OFF,
        pcodes::INDEX_STALE,
        pcodes::PACKAGE_BUSY,
        pcodes::PACKAGE_INSTALL_FAILED,
    ]
    .into_iter()
    .find(|k| *k == c)
}

/// Job side: drop a request where the root path unit looks, wait for its
/// answer in `result_dir`.
pub async fn request_and_wait(home: &Path, result_dir: &Path, pkgs: &[String], server: &RequestServer, limit: Duration) -> Result<PkgResult, String> {
    let id = format!("pkg-{}", k2_node_proto::crypto::random_id()?);
    let req = PkgRequest { id: id.clone(), packages: pkgs.to_vec(), server: Some(server.clone()) };
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
    // A policy-rc.d a crash left behind must not block the owner's services.
    crate::pkgsys::NoServices::clear_stale();
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
        let server = req.server.clone().unwrap_or_else(RequestServer::owner);
        let mut lines: Vec<String> = Vec::new();
        let r = root_install(&req.packages, &server, &mut |l| lines.push(l.to_string()));
        let res = PkgResult {
            id: req.id.clone(),
            ok: r.is_ok(),
            message: match &r {
                Ok(_) => std::iter::once(format!("installed {}", req.packages.join(" "))).chain(lines).collect::<Vec<_>>().join("\n"),
                Err((_, e)) => e.clone(),
            },
            code: r.as_ref().err().map(|(c, _)| c.to_string()),
        };
        eprintln!("k2-node packages: {} → {}", req.packages.join(" "), res.message);
        let body = serde_json::to_vec(&res).map_err(|e| e.to_string())?;
        crate::util::atomic_write(&out.join(format!("{}.json", req.id)), &body, 0o644)?;
    }
    Ok(())
}

// ── root side: `k2-node packages …` ──────────────────────────────────

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// `k2-node packages install <name>…` (root: an admin by hand) — the
/// owner's own policy, with every guard.
pub fn cmd_install(names: &[String]) -> Result<(), String> {
    root_install(names, &RequestServer::owner(), &mut |l| eprintln!("k2-node packages: {l}")).map(|_| ()).map_err(|(c, m)| format!("{c}: {m}"))
}

/// One install, as root: re-read root's list and policy, refuse anything
/// they don't cover for `server`, then [`crate::pkgsys::guarded_install`]
/// with the real manager. Every outcome goes to root's install log.
pub fn root_install(names: &[String], server: &RequestServer, log: &mut dyn FnMut(&str)) -> Result<crate::pkgsys::Report, crate::pkgsys::Refusal> {
    let fail = |e: String| (codes::PACKAGE_INSTALL_FAILED, e);
    if !is_root() {
        return Err(fail("packages install runs as root (k2-node calls it through its root unit)".into()));
    }
    if names.is_empty() || names.len() > k2_node_proto::profiles::MAX_PLAN_PACKAGES {
        return Err(fail(format!("packages install <name>… (1–{} names)", k2_node_proto::profiles::MAX_PLAN_PACKAGES)));
    }
    for n in names {
        if SysName::parse(n).is_none() {
            return Err(fail(format!("{n:?} isn't a package name")));
        }
    }
    let path = PathBuf::from(APPROVED_FILE);
    trusted_owner_only(&path).map_err(fail)?;
    let ppath = PathBuf::from(POLICY_FILE);
    trusted_owner_only(&ppath).map_err(fail)?;
    let approved = read_approved(&path).map_err(fail)?;
    let policy = read_policy(&ppath).map_err(fail)?.for_server(server.owner, &server.fp);
    let h = crate::pkgsys::host_ctx().ok_or_else(|| fail("no supported package manager (apt, dnf, pacman, zypper)".into()))?;
    let b = crate::pkgsys::system().ok_or_else(|| fail("no supported package manager".into()))?;
    let refused: Vec<&String> = names.iter().filter(|n| !coverage(&policy, &approved, n, &h.natives(n).unwrap_or_default()).covered()).collect();
    if !refused.is_empty() {
        let r = refused.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
        let e = (pcodes::PACKAGE_NOT_APPROVED, format!("not approved on this node for this server: {r} (sudo k2-node packages allow {r})"));
        audit_line(server, names, &Err(e.clone()));
        return Err(e);
    }
    let r = crate::pkgsys::guarded_install(&b, &h, names, &crate::pkgsys::Guards { max_install_mb: policy.max_install_mb }, log);
    audit_line(server, names, &r);
    r
}

/// One JSON line per install in root's log (append-only, root's).
pub fn audit_line(server: &RequestServer, names: &[String], r: &Result<crate::pkgsys::Report, crate::pkgsys::Refusal>) {
    audit_line_to(Path::new(k2_node_proto::pkgpolicy::LOG_FILE), &serde_json::json!({
        "server": if server.owner { "own".to_string() } else { server.fp.chars().take(16).collect() },
        "names": names,
    }), r);
}

/// [`audit_line`] with the request id and file (tests, the executor).
pub fn audit_line_to(path: &Path, base: &serde_json::Value, r: &Result<crate::pkgsys::Report, crate::pkgsys::Refusal>) {
    use std::io::Write;
    let mut v = base.clone();
    v["at"] = serde_json::json!(crate::util::now());
    match r {
        Ok(rep) => {
            v["result"] = serde_json::json!(if rep.nothing { "already" } else { "installed" });
            v["natives"] = serde_json::json!(rep.natives);
            v["installs"] = serde_json::json!(rep.dry.installs);
            v["size_mb"] = serde_json::json!(rep.dry.size_mb);
        }
        Err((c, m)) => {
            v["result"] = serde_json::json!("refused");
            v["code"] = serde_json::json!(c);
            v["message"] = serde_json::json!(m);
        }
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{v}");
    }
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
                    assert_eq!(req.server, Some(RequestServer { owner: false, fp: "c41f00aa11bb22cc".into() }), "the server rides the request");
                    let res = PkgResult { id: req.id.clone(), ok: true, message: format!("installed {}", req.packages.join(" ")), code: None };
                    std::fs::write(r2.join(format!("{}.json", req.id)), serde_json::to_vec(&res).unwrap()).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("no request arrived");
        });
        let srv = RequestServer { owner: false, fp: "c41f00aa11bb22cc".into() };
        let r = request_and_wait(&home, &results, &["pkg-config".into()], &srv, Duration::from_secs(10)).await.unwrap();
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
        tc.policy_file = home.join("policy.toml");
        std::fs::write(&tc.packages_file, render_approved(&["pkg-config".into()])).unwrap();
        let ghost = "k2-test-no-such-package-xyz".to_string();
        let e = ensure(&tc, &[ghost.clone()], &mut |_| {}).await.unwrap_err();
        let mgr = detect();
        if tc.os != "macos" && mgr != Some(Mgr::Apt) {
            // No map row and not apt: not satisfiable here, said so.
            assert_eq!(e.0, pcodes::NO_PACKAGE_MAPPING, "{}", e.1);
            assert!(e.1.contains("It can't be requested"), "{}", e.1);
        } else if tc.os == "macos" {
            assert_eq!(e.0, codes::PACKAGE_MISSING);
            assert!(e.1.contains(&format!("brew install {ghost}")), "{}", e.1);
        } else {
            assert_eq!(e.0, codes::PACKAGE_NOT_APPROVED);
            assert!(e.1.contains(&format!("sudo k2-node packages allow {ghost}")), "{}", e.1);
            assert!(e.1.contains(&format!("k2 compute node packages request <node> {ghost}")), "{}", e.1);
        }
    }

    /// A team server's policy covers its profile and nothing else; a
    /// denied name is refused as denied (§3.4), whatever the distro.
    #[tokio::test]
    async fn the_policy_for_the_jobs_server_decides() {
        let home = crate::util::temp_dir("pkg-policy");
        let mut tc = Toolchains::new(&home, vec![("PATH".into(), "/usr/bin:/bin".into())]);
        tc.packages_file = home.join("approved.toml");
        tc.policy_file = home.join("policy.toml");
        let mut f = PolicyFile::default();
        f.servers.insert(
            "acme.k2.dev".into(),
            k2_node_proto::pkgpolicy::ServerLine {
                fp: "c41f00aa11bb22cc".into(),
                policy: k2_node_proto::pkgpolicy::ServerPolicy { level: k2_node_proto::pkgpolicy::Level::Profiles, profiles: vec!["build-base@1".into()], max_install_mb: 10 },
            },
        );
        std::fs::write(&tc.policy_file, f.render()).unwrap();
        tc.server = RequestServer { owner: false, fp: "c41f00aa11bb22cc0011".into() };
        // A test home has no root installer, even on a machine that does.
        tc.pkg_installer = false;
        let h = crate::pkgsys::HostCtx { mgr: Mgr::Apt, distro: "ubuntu".into(), version: "24.04".into(), like: vec!["debian".into()] };
        let b = crate::pkgsys::fake::FakeBackend::new(Mgr::Apt);
        b.add("libpq-dev", &[], 10);
        b.add("cmake", &[], 10);
        b.add("lightdm", &[], 10);
        let e = ensure_with(&tc, &h, &b, &["libpq-dev".to_string()], &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, codes::PACKAGE_NOT_APPROVED, "{}", e.1);
        assert!(e.1.contains("profiles (build-base@1)"), "the refusal names the server's level: {}", e.1);
        let e = ensure_with(&tc, &h, &b, &["lightdm".to_string()], &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, pcodes::PACKAGE_DENIED, "{}", e.1);
        // In the profile: covered, so it goes on to the root installer
        // (absent in a test home: said so, nothing else ran).
        let e = ensure_with(&tc, &h, &b, &["cmake".to_string()], &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, codes::PACKAGE_INSTALL_FAILED, "{}", e.1);
        assert!(e.1.contains("no package installer"), "{}", e.1);
        // Installed already: nothing to do, whatever the policy.
        b.state.lock().unwrap().installed.insert("libpq-dev".into());
        assert!(!ensure_with(&tc, &h, &b, &["libpq-dev".to_string()], &mut |_| {}).await.unwrap());
        // Not satisfiable on this manager: refused before any coverage.
        let arch = crate::pkgsys::HostCtx { mgr: Mgr::Pacman, distro: "arch".into(), version: "rolling".into(), like: vec![] };
        let e = ensure_with(&tc, &arch, &b, &["libfoo-dev".to_string()], &mut |_| {}).await.unwrap_err();
        assert_eq!(e.0, pcodes::NO_PACKAGE_MAPPING, "{}", e.1);
    }
}
