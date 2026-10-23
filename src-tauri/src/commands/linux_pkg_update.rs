//! Linux package updates from the desktop app: download, verify, install
//! with pkexec, restart the local daemon, relaunch. Arch (`pacman -U`),
//! deb (`apt-get install`) and rpm (`dnf install`). AppImage keeps the
//! Tauri updater path; headless servers keep `k2 update`.
//!
//! OS integration (a package install), so it lives in src-tauri. The SUMS
//! parsing and verification is `k2_client::release_sums`.
//!
//! Flow (`install_linux_package_update`):
//!   1. Only for an install this package manager owns: the running exe is
//!      the packaged path (`/usr/lib/k2/k2-gui` for Arch, `/usr/bin/k2` for
//!      deb/rpm) and os-release names the family. Version is digits/dots.
//!   2. Fetch `SHA256SUMS-linux-x86_64.txt` from the same release over
//!      HTTPS (redirects must stay HTTPS). No line for the package: refuse
//!      before downloading it.
//!   3. Download the package into memory, verify its sha256 against that
//!      line. Mismatch: refuse.
//!   4. Write those exact bytes to a fresh `mkdtemp` 0700 dir (create_new,
//!      O_NOFOLLOW, 0600). Just before pkexec, re-check the dir (ours,
//!      0700, a real dir) and the file (ours, regular, one link) and
//!      re-hash it. Another user cannot touch it; only processes running
//!      as this same user could, and those can already raise their own
//!      polkit prompt, so the prompt is the boundary. A root-side copy and
//!      re-hash would need a root helper verb (see the 0.46.2 notes).
//!   5. `pkexec <absolute tool> <args> <that file>`. pkexec or the tool
//!      missing, the user cancelling (126) or no polkit agent (127): keep
//!      the verified file and return the exact command to run instead.
//!   6. Installed: `linux_daemon::reconcile(version)` restarts the local
//!      k2-daemon user service and waits for the new version, then the
//!      renderer calls `relaunch_after_linux_package_update`.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use k2_client::release_sums::{self, LINUX_SUMS_ASSET};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const RELEASE_DOWNLOAD_BASE: &str = "https://github.com/Alakazam-211/K2/releases/download";
pub const PKEXEC: &str = "/usr/bin/pkexec";
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SUMS_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Arch,
    Deb,
    Rpm,
}

impl PackageKind {
    /// Release asset name, exactly as app-linux.yml / arch-package.yml
    /// publish it.
    pub fn asset_name(self, version: &str) -> String {
        match self {
            PackageKind::Arch => format!("k2-{version}-x86_64.pkg.tar.zst"),
            PackageKind::Deb => format!("K2_{version}_amd64.deb"),
            PackageKind::Rpm => format!("K2-{version}-1.x86_64.rpm"),
        }
    }

    pub fn tool(self) -> &'static str {
        match self {
            PackageKind::Arch => "/usr/bin/pacman",
            PackageKind::Deb => "/usr/bin/apt-get",
            PackageKind::Rpm => "/usr/bin/dnf",
        }
    }

    fn tool_args(self) -> &'static [&'static str] {
        match self {
            PackageKind::Arch => &["-U", "--noconfirm"],
            PackageKind::Deb => &["install", "-y"],
            PackageKind::Rpm => &["install", "-y"],
        }
    }

    /// Where the package puts the GUI the running process must be.
    pub fn packaged_exe(self) -> &'static str {
        match self {
            PackageKind::Arch => "/usr/lib/k2/k2-gui",
            PackageKind::Deb | PackageKind::Rpm => "/usr/bin/k2",
        }
    }

    /// What to exec on relaunch (stable across the upgrade).
    pub fn launcher(self) -> &'static str {
        match self {
            PackageKind::Arch => "/usr/bin/k2-gui",
            PackageKind::Deb | PackageKind::Rpm => "/usr/bin/k2",
        }
    }
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// `ID` + `ID_LIKE` tokens from /etc/os-release.
fn os_release_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        if k.trim() == "ID" || k.trim() == "ID_LIKE" {
            ids.extend(unquote(v).split_whitespace().map(|t| unquote(t).to_string()));
        }
    }
    ids
}

/// `install_kind` is `updater::detect_install_kind()` (arch | linux-package |
/// appimage | other). deb/rpm need a family we know the tool for; openSUSE
/// (zypper) and friends get None and keep the release-page flow.
pub fn package_kind_for(install_kind: &str, os_release: &str) -> Option<PackageKind> {
    match install_kind {
        "arch" => Some(PackageKind::Arch),
        "linux-package" => {
            let ids = os_release_ids(os_release);
            let has = |names: &[&str]| ids.iter().any(|i| names.contains(&i.as_str()));
            if has(&["debian", "ubuntu"]) {
                Some(PackageKind::Deb)
            } else if has(&["fedora", "rhel", "centos"]) {
                Some(PackageKind::Rpm)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// In-app install only when the running exe is the packaged one (not a dev
/// build, not an unpacked tree).
pub fn exe_is_packaged(kind: PackageKind, exe_canonical: &Path) -> bool {
    exe_canonical == Path::new(kind.packaged_exe())
}

/// `0.46.1`, `0.40.150a`. Nothing that could leave the URL path segment.
pub fn validate_version(v: &str) -> Result<String, String> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = 0;
    let ok = !v.is_empty()
        && v.len() <= 32
        && v.split('.').all(|p| {
            parts += 1;
            let digits = p.trim_end_matches(|c: char| c.is_ascii_lowercase());
            let suffix = &p[digits.len()..];
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && suffix.len() <= 1
        });
    if ok && (2..=4).contains(&parts) {
        Ok(v.to_string())
    } else {
        Err(format!("not a K2 release version: {v:?}"))
    }
}

pub fn release_asset_url(version: &str, asset: &str) -> String {
    format!("{RELEASE_DOWNLOAD_BASE}/v{version}/{asset}")
}

/// Exact argv: pkexec, the absolute tool, its flags, the staged file.
pub fn pkexec_argv(kind: PackageKind, pkg: &Path) -> Vec<String> {
    let mut v = vec![PKEXEC.to_string(), kind.tool().to_string()];
    v.extend(kind.tool_args().iter().map(|s| s.to_string()));
    v.push(pkg.display().to_string());
    v
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command to paste into a terminal when pkexec cannot run.
pub fn manual_command(kind: PackageKind, pkg: &Path) -> String {
    let tool = match kind {
        PackageKind::Arch => "pacman",
        PackageKind::Deb => "apt-get",
        PackageKind::Rpm => "dnf",
    };
    format!(
        "sudo {tool} {} {} && systemctl --user restart k2-daemon",
        kind.tool_args().join(" "),
        shell_quote(&pkg.display().to_string())
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    Pkexec(Vec<String>),
    Manual { reason: String, command: String },
}

pub fn plan_install(kind: PackageKind, pkg: &Path, pkexec_exists: bool, tool_exists: bool) -> InstallPlan {
    if !tool_exists {
        return InstallPlan::Manual {
            reason: format!("{} was not found, so K2 cannot install the package for you.", kind.tool()),
            command: manual_command(kind, pkg),
        };
    }
    if !pkexec_exists {
        return InstallPlan::Manual {
            reason: format!(
                "{PKEXEC} was not found (install polkit to let K2 ask for your password), so K2 cannot install the package for you."
            ),
            command: manual_command(kind, pkg),
        };
    }
    InstallPlan::Pkexec(pkexec_argv(kind, pkg))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkexecResult {
    Installed,
    /// 126: the user dismissed the password dialog.
    Cancelled,
    /// 127: not authorized, or no polkit agent to ask.
    NotAuthorized,
    /// The package tool itself failed (its exit code), or a signal (None).
    Failed(Option<i32>),
}

pub fn classify_pkexec_exit(code: Option<i32>) -> PkexecResult {
    match code {
        Some(0) => PkexecResult::Installed,
        Some(126) => PkexecResult::Cancelled,
        Some(127) => PkexecResult::NotAuthorized,
        other => PkexecResult::Failed(other),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LinuxInstallOutcome {
    /// `installed` (relaunch next) or `manual` (show message + command).
    pub status: &'static str,
    pub message: String,
    pub manual_command: Option<String>,
    pub daemon: Option<crate::linux_daemon::ReconcileReport>,
}

fn manual(reason: String, command: String) -> LinuxInstallOutcome {
    LinuxInstallOutcome {
        status: "manual",
        message: format!("{reason} The package is downloaded and verified. Run this in a terminal, then reopen K2:"),
        manual_command: Some(command),
        daemon: None,
    }
}

/// The kind for THIS install, or why in-app install is off.
pub fn this_install() -> Result<PackageKind, String> {
    let install_kind = super::updater::detect_install_kind();
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let kind = package_kind_for(install_kind, &os_release)
        .ok_or_else(|| format!("in-app install is not available for this install ({install_kind})"))?;
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| format!("cannot resolve the running app: {e}"))?;
    if !exe_is_packaged(kind, &exe) {
        return Err(format!(
            "this K2 runs from {}, not the packaged {}; update it the way you installed it",
            exe.display(),
            kind.packaged_exe()
        ));
    }
    Ok(kind)
}

fn https_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("K2-LinuxPackageUpdate")
        .timeout(timeout)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.error("refusing a non-HTTPS redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| e.to_string())
}

fn fetch(url: &str, max: u64, timeout: Duration) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let resp = https_client(timeout)?
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download {url}: {e}"))?;
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(format!("download {url}: larger than {max} bytes"));
    }
    let mut buf = Vec::new();
    resp.take(max + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("download {url}: {e}"))?;
    if buf.len() as u64 > max {
        return Err(format!("download {url}: larger than {max} bytes"));
    }
    Ok(buf)
}

/// `mkdtemp(<tmp>/k2-update-XXXXXX)`: a new dir, mode 0700, ours.
#[cfg(unix)]
pub fn make_private_dir(base: &Path) -> std::io::Result<PathBuf> {
    use std::ffi::{CString, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let template = base.join("k2-update-XXXXXX");
    let mut bytes = CString::new(template.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?
        .into_bytes_with_nul();
    // SAFETY: `bytes` is a NUL-terminated, writable buffer that outlives the call.
    let p = unsafe { libc::mkdtemp(bytes.as_mut_ptr() as *mut libc::c_char) };
    if p.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    bytes.pop();
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

/// Write `bytes` as a new 0600 file (never follows or reuses a path).
#[cfg(unix)]
pub fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Right before pkexec: the dir is still a real 0700 dir owned by `uid`,
/// the file a regular single-link file owned by `uid` (no symlink), and its
/// bytes still hash to `expected`.
#[cfg(unix)]
pub fn recheck_staged(dir: &Path, file: &Path, expected: &str, uid: u32) -> Result<(), String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let d = std::fs::symlink_metadata(dir).map_err(|e| format!("staging dir: {e}"))?;
    if !d.file_type().is_dir() || d.uid() != uid || d.mode() & 0o077 != 0 {
        return Err(format!("staging dir {} is not a private dir owned by you", dir.display()));
    }
    if file.parent() != Some(dir) {
        return Err(format!("{} is not inside {}", file.display(), dir.display()));
    }
    let m = std::fs::symlink_metadata(file).map_err(|e| format!("staged package: {e}"))?;
    if !m.file_type().is_file() || m.uid() != uid || m.nlink() != 1 {
        return Err(format!("staged package {} changed type or owner", file.display()));
    }
    let mut buf = Vec::new();
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file)
        .and_then(|mut f| f.read_to_end(&mut buf))
        .map_err(|e| format!("staged package: {e}"))?;
    let actual = release_sums::sha256_hex(&buf);
    if actual != expected {
        return Err(format!(
            "staged package {} changed after verification (sha256 {actual}, expected {expected})",
            file.display()
        ));
    }
    Ok(())
}

/// Blocking: everything after the kind is known.
#[cfg(unix)]
pub fn run_install(kind: PackageKind, version: &str) -> Result<LinuxInstallOutcome, String> {
    let version = validate_version(version)?;
    let asset = kind.asset_name(&version);
    let sums = fetch(
        &release_asset_url(&version, LINUX_SUMS_ASSET),
        MAX_SUMS_BYTES,
        Duration::from_secs(30),
    )?;
    let sums = String::from_utf8(sums).map_err(|_| format!("{LINUX_SUMS_ASSET} is not text"))?;
    // Refuse before the big download when the release has no line for it.
    release_sums::expected_sha256(&sums, &asset)
        .map_err(|e| format!("Refusing to install v{version}: {e}"))?;
    let bytes = fetch(
        &release_asset_url(&version, &asset),
        MAX_PACKAGE_BYTES,
        Duration::from_secs(900),
    )?;
    let hash = release_sums::verify_package(&sums, &asset, &bytes)
        .map_err(|e| format!("Refusing to install v{version}: {e}"))?;

    // Write exactly the bytes just verified.
    let dir = make_private_dir(&std::env::temp_dir()).map_err(|e| format!("staging dir: {e}"))?;
    let pkg = dir.join(&asset);
    write_new_file(&pkg, &bytes).map_err(|e| format!("write {}: {e}", pkg.display()))?;
    drop(bytes);

    let plan = plan_install(
        kind,
        &pkg,
        Path::new(PKEXEC).is_file(),
        Path::new(kind.tool()).is_file(),
    );
    let argv = match plan {
        InstallPlan::Manual { reason, command } => return Ok(manual(reason, command)),
        InstallPlan::Pkexec(argv) => argv,
    };
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if let Err(e) = recheck_staged(&dir, &pkg, &hash, uid) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("Refusing to install: {e}"));
    }
    log_debug!("[linux-pkg-update] running {}", argv.join(" "));
    let out = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{PKEXEC}: {e}"))?;
    let command = manual_command(kind, &pkg);
    match classify_pkexec_exit(out.status.code()) {
        PkexecResult::Installed => {
            let _ = std::fs::remove_dir_all(&dir);
            let daemon = crate::linux_daemon::reconcile(&version);
            Ok(LinuxInstallOutcome {
                status: "installed",
                message: format!("K2 v{version} is installed."),
                manual_command: None,
                daemon: Some(daemon),
            })
        }
        PkexecResult::Cancelled => Ok(manual("The password prompt was cancelled.".into(), command)),
        PkexecResult::NotAuthorized => Ok(manual(
            "pkexec could not get permission (not authorized, or no polkit agent is running).".into(),
            command,
        )),
        PkexecResult::Failed(code) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            Ok(manual(
                format!(
                    "{} exited with {}: {}.",
                    kind.tool(),
                    code.map(|c| c.to_string()).unwrap_or_else(|| "a signal".into()),
                    tail.join(" ").trim()
                ),
                command,
            ))
        }
    }
}

/// Download, verify, install with pkexec, restart the local daemon.
/// `Err` = refused (no SUMS line, hash mismatch, download failure, not a
/// packaged install). `Ok(manual)` = verified file ready, run the command.
#[tauri::command]
pub async fn install_linux_package_update(version: String) -> Result<LinuxInstallOutcome, String> {
    if k2_client::airgap::enabled_for_client() {
        return Err(k2_client::airgap::TEACHING.to_string());
    }
    let kind = this_install()?;
    #[cfg(unix)]
    {
        tokio::task::spawn_blocking(move || run_install(kind, &version))
            .await
            .map_err(|e| e.to_string())?
    }
    #[cfg(not(unix))]
    {
        let _ = (kind, version);
        Err("in-app package install is Linux only".to_string())
    }
}

/// The gate's view of the Linux local-daemon restart (`linux_daemon`).
/// Always `idle` on macOS/Windows.
#[tauri::command]
pub fn local_daemon_update_state() -> crate::linux_daemon::LocalDaemonUpdateState {
    crate::linux_daemon::current_state()
}

/// Shell for the relaunch helper: wait for this pid, then exec the launcher.
pub fn relaunch_helper_argv(pid: u32, launcher: &str) -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; sleep 1; exec "$2""#.into(),
        "k2-relaunch".into(),
        pid.to_string(),
        launcher.into(),
    ]
}

/// After an `installed` outcome: start the packaged launcher once this
/// process is gone, then exit. The path is the package's, never
/// /proc/self/exe (that is "(deleted)" after the upgrade).
#[tauri::command]
pub fn relaunch_after_linux_package_update() -> Result<(), String> {
    let kind = this_install_after_upgrade()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let argv = relaunch_helper_argv(std::process::id(), kind.launcher());
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // SAFETY: setsid is async-signal-safe; detaches the helper from our
        // session so it survives our exit.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn().map_err(|e| format!("relaunch helper: {e}"))?;
        crate::RELAUNCH_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: same hard exit as relaunch_via_open (skips GPU teardown).
        unsafe { libc::_exit(0) }
    }
    #[cfg(not(unix))]
    {
        let _ = kind;
        Err("relaunch after a package install is Linux only".to_string())
    }
}

/// Like [`this_install`] but tolerant of the upgraded exe: after the
/// package replaced it, canonicalize of /proc/self/exe fails or ends in
/// " (deleted)". The family still comes from os-release.
fn this_install_after_upgrade() -> Result<PackageKind, String> {
    let install_kind = super::updater::detect_install_kind();
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let kind = package_kind_for(install_kind, &os_release)
        .ok_or_else(|| format!("relaunch is not available for this install ({install_kind})"))?;
    if !Path::new(kind.launcher()).exists() {
        return Err(format!("{} is missing after the install", kind.launcher()));
    }
    Ok(kind)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const PKG: &str = "/tmp/k2-update-AbC123/k2-0.46.1-x86_64.pkg.tar.zst";

    #[test]
    fn exact_pkexec_argv_per_distro() {
        assert_eq!(
            pkexec_argv(PackageKind::Arch, Path::new(PKG)),
            ["/usr/bin/pkexec", "/usr/bin/pacman", "-U", "--noconfirm", PKG]
        );
        let deb = "/tmp/k2-update-AbC123/K2_0.46.1_amd64.deb";
        assert_eq!(
            pkexec_argv(PackageKind::Deb, Path::new(deb)),
            ["/usr/bin/pkexec", "/usr/bin/apt-get", "install", "-y", deb]
        );
        let rpm = "/tmp/k2-update-AbC123/K2-0.46.1-1.x86_64.rpm";
        assert_eq!(
            pkexec_argv(PackageKind::Rpm, Path::new(rpm)),
            ["/usr/bin/pkexec", "/usr/bin/dnf", "install", "-y", rpm]
        );
    }

    #[test]
    fn asset_names_match_the_release_workflows() {
        assert_eq!(PackageKind::Arch.asset_name("0.46.1"), "k2-0.46.1-x86_64.pkg.tar.zst");
        assert_eq!(PackageKind::Deb.asset_name("0.46.1"), "K2_0.46.1_amd64.deb");
        assert_eq!(PackageKind::Rpm.asset_name("0.46.1"), "K2-0.46.1-1.x86_64.rpm");
        assert_eq!(
            release_asset_url("0.46.1", LINUX_SUMS_ASSET),
            "https://github.com/Alakazam-211/K2/releases/download/v0.46.1/SHA256SUMS-linux-x86_64.txt"
        );
        for k in [PackageKind::Arch, PackageKind::Deb, PackageKind::Rpm] {
            assert!(k2_client::release_sums::is_plain_file_name(&k.asset_name("0.46.1")));
        }
    }

    #[test]
    fn pkexec_absent_falls_back_with_the_exact_command() {
        match plan_install(PackageKind::Arch, Path::new(PKG), false, true) {
            InstallPlan::Manual { reason, command } => {
                assert!(reason.contains("/usr/bin/pkexec was not found"), "{reason}");
                assert_eq!(
                    command,
                    format!("sudo pacman -U --noconfirm '{PKG}' && systemctl --user restart k2-daemon")
                );
            }
            other => panic!("{other:?}"),
        }
        match plan_install(PackageKind::Deb, Path::new("/tmp/x/K2_1.2.3_amd64.deb"), true, false) {
            InstallPlan::Manual { reason, command } => {
                assert!(reason.contains("/usr/bin/apt-get was not found"), "{reason}");
                assert_eq!(
                    command,
                    "sudo apt-get install -y '/tmp/x/K2_1.2.3_amd64.deb' && systemctl --user restart k2-daemon"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            plan_install(PackageKind::Rpm, Path::new("/tmp/x/K2-1.2.3-1.x86_64.rpm"), true, true),
            InstallPlan::Pkexec(pkexec_argv(PackageKind::Rpm, Path::new("/tmp/x/K2-1.2.3-1.x86_64.rpm")))
        );
    }

    #[test]
    fn manual_command_quotes_odd_paths() {
        let cmd = manual_command(PackageKind::Rpm, Path::new("/tmp/it's here/K2-1.2.3-1.x86_64.rpm"));
        assert_eq!(
            cmd,
            r"sudo dnf install -y '/tmp/it'\''s here/K2-1.2.3-1.x86_64.rpm' && systemctl --user restart k2-daemon"
        );
    }

    #[test]
    fn pkexec_exit_codes() {
        assert_eq!(classify_pkexec_exit(Some(0)), PkexecResult::Installed);
        assert_eq!(classify_pkexec_exit(Some(126)), PkexecResult::Cancelled);
        assert_eq!(classify_pkexec_exit(Some(127)), PkexecResult::NotAuthorized);
        assert_eq!(classify_pkexec_exit(Some(1)), PkexecResult::Failed(Some(1)));
        assert_eq!(classify_pkexec_exit(None), PkexecResult::Failed(None));
    }

    #[test]
    fn package_kind_per_distro() {
        assert_eq!(package_kind_for("arch", ""), Some(PackageKind::Arch));
        assert_eq!(
            package_kind_for("linux-package", "ID=ubuntu\nID_LIKE=debian\n"),
            Some(PackageKind::Deb)
        );
        assert_eq!(package_kind_for("linux-package", "ID=debian\n"), Some(PackageKind::Deb));
        assert_eq!(package_kind_for("linux-package", "ID=pop\nID_LIKE=\"ubuntu debian\"\n"), Some(PackageKind::Deb));
        assert_eq!(package_kind_for("linux-package", "ID=fedora\n"), Some(PackageKind::Rpm));
        assert_eq!(
            package_kind_for("linux-package", "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"),
            Some(PackageKind::Rpm)
        );
        assert_eq!(package_kind_for("linux-package", "ID=opensuse-tumbleweed\nID_LIKE=\"opensuse suse\"\n"), None);
        assert_eq!(package_kind_for("appimage", "ID=arch\n"), None);
        assert_eq!(package_kind_for("other", "ID=ubuntu\n"), None);
    }

    #[test]
    fn only_the_packaged_exe_may_install() {
        assert!(exe_is_packaged(PackageKind::Arch, Path::new("/usr/lib/k2/k2-gui")));
        assert!(!exe_is_packaged(PackageKind::Arch, Path::new("/usr/bin/k2-gui")));
        assert!(!exe_is_packaged(PackageKind::Arch, Path::new("/home/r/K2/target/release/k2")));
        assert!(exe_is_packaged(PackageKind::Deb, Path::new("/usr/bin/k2")));
        assert!(!exe_is_packaged(PackageKind::Rpm, Path::new("/opt/K2/k2")));
    }

    #[test]
    fn version_validation() {
        assert_eq!(validate_version("0.46.1").unwrap(), "0.46.1");
        assert_eq!(validate_version("v0.46.1").unwrap(), "0.46.1");
        assert_eq!(validate_version("0.40.150a").unwrap(), "0.40.150a");
        assert_eq!(validate_version(" 0.46.1 ").unwrap(), "0.46.1");
        for bad in [
            "", "0", "0.46.1/../../x", "0.46.1?x", "0.46 .1", "0..1", "a.b.c", "0.46.1-rc1",
            "0.46.1ab", "1.2.3.4.5", "0.46.1%2f..",
        ] {
            assert!(validate_version(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn relaunch_helper_is_exact() {
        assert_eq!(
            relaunch_helper_argv(4242, "/usr/bin/k2-gui"),
            [
                "/bin/sh",
                "-c",
                r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; sleep 1; exec "$2""#,
                "k2-relaunch",
                "4242",
                "/usr/bin/k2-gui"
            ]
        );
    }

    fn uid() -> u32 {
        // SAFETY: no preconditions.
        unsafe { libc::getuid() }
    }

    #[test]
    fn staging_dir_is_private_and_file_is_new() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = make_private_dir(base.path()).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let pkg = dir.join("K2_1.2.3_amd64.deb");
        write_new_file(&pkg, b"verified").unwrap();
        assert_eq!(std::fs::metadata(&pkg).unwrap().permissions().mode() & 0o777, 0o600);
        // create_new: never overwrite or reuse.
        assert!(write_new_file(&pkg, b"again").is_err());
        let hash = release_sums::sha256_hex(b"verified");
        recheck_staged(&dir, &pkg, &hash, uid()).unwrap();
    }

    #[test]
    fn swapped_or_tampered_staged_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = make_private_dir(base.path()).unwrap();
        let pkg = dir.join("K2_1.2.3_amd64.deb");
        write_new_file(&pkg, b"verified").unwrap();
        let hash = release_sums::sha256_hex(b"verified");

        // Content changed after verification.
        std::fs::write(&pkg, b"evil").unwrap();
        let e = recheck_staged(&dir, &pkg, &hash, uid()).unwrap_err();
        assert!(e.contains("changed after verification"), "{e}");

        // Swapped for a symlink to another file.
        std::fs::remove_file(&pkg).unwrap();
        let elsewhere = base.path().join("elsewhere.deb");
        std::fs::write(&elsewhere, b"verified").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &pkg).unwrap();
        let e = recheck_staged(&dir, &pkg, &hash, uid()).unwrap_err();
        assert!(e.contains("changed type or owner"), "{e}");
        std::fs::remove_file(&pkg).unwrap();

        // Hard link (a second name someone else controls).
        write_new_file(&pkg, b"verified").unwrap();
        std::fs::hard_link(&pkg, base.path().join("second-name")).unwrap();
        let e = recheck_staged(&dir, &pkg, &hash, uid()).unwrap_err();
        assert!(e.contains("changed type or owner"), "{e}");

        // Dir loosened to group/other.
        let dir2 = make_private_dir(base.path()).unwrap();
        let pkg2 = dir2.join("K2_1.2.3_amd64.deb");
        write_new_file(&pkg2, b"verified").unwrap();
        std::fs::set_permissions(&dir2, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = recheck_staged(&dir2, &pkg2, &hash, uid()).unwrap_err();
        assert!(e.contains("not a private dir"), "{e}");

        // A path outside the staging dir (dir private again).
        std::fs::set_permissions(&dir2, std::fs::Permissions::from_mode(0o700)).unwrap();
        recheck_staged(&dir2, &pkg2, &hash, uid()).unwrap();
        let e = recheck_staged(&dir2, &elsewhere, &hash, uid()).unwrap_err();
        assert!(e.contains("is not inside"), "{e}");

        // Wrong owner.
        let e = recheck_staged(&dir2, &pkg2, &hash, uid().wrapping_add(1)).unwrap_err();
        assert!(e.contains("not a private dir owned by you"), "{e}");
    }
}
