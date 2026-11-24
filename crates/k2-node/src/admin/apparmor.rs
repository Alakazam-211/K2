//! The AppArmor profile for `--userns` job units (0.46.18).
//!
//! Ubuntu 23.10 and later restrict user namespaces for unconfined
//! processes (`kernel.apparmor_restrict_unprivileged_userns=1`): a user
//! namespace is still made, but the process lands in the
//! `unprivileged_userns` profile without capabilities, so a `--userns` job
//! can't map itself, mount or run a container (`map:EACCES`). Ubuntu lets
//! its own tools through with one profile each that is otherwise
//! unconfined and allows `userns`; K2 does the same for exactly one kind
//! of process: the unit of a `--userns` job (the node owner's own server's,
//! granted by the node's owner). Root loads the profile the first time such
//! a unit starts on a restricting kernel and names it in that unit
//! (`AppArmorProfile=`); no other unit, job or process gets it, and the
//! node's own sysctl never changes.
//!
//! The profile attaches to nothing by path: only a unit that names it runs
//! under it. Without the restriction (Arch, Debian, older Ubuntu) nothing
//! is loaded and no unit names it.

use std::path::Path;

/// The profile's name (what `AppArmorProfile=` names).
pub const PROFILE: &str = "k2-job-userns";
/// Where root writes it (loaded again by apparmor.service at boot).
pub const PROFILE_PATH: &str = "/etc/apparmor.d/k2-job-userns";
/// The kernel's restriction switch (absent: no restriction).
pub const RESTRICT_SYSCTL: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
/// The loaded profiles.
pub const LOADED: &str = "/sys/kernel/security/apparmor/profiles";

/// The whole profile: unconfined but for one more permission, `userns`
/// (abi 4.0 is the first that knows the rule). No attachment path, no
/// local include: nothing can widen it or attach it to anything else.
pub const PROFILE_TEXT: &str = "# K2 compute: a --userns job's unit (the node owner's own server's job,
# granted by the node's owner) may create user namespaces on kernels that
# restrict them for unconfined processes
# (kernel.apparmor_restrict_unprivileged_userns=1). Otherwise unconfined, as
# every job unit is. Attached to nothing: only a unit naming it
# (AppArmorProfile=k2-job-userns) runs under it. Written by k2-node.
abi <abi/4.0>,

profile k2-job-userns flags=(unconfined) {
  userns,
}
";

/// Whether the kernel restricts user namespaces for unconfined processes
/// (the sysctl's text; `None` = no such sysctl).
pub fn restricts(sysctl: Option<&str>) -> bool {
    sysctl.is_some_and(|s| s.trim() == "1")
}

/// [`restricts`] for this machine.
pub fn kernel_restricts() -> bool {
    restricts(std::fs::read_to_string(RESTRICT_SYSCTL).ok().as_deref())
}

/// Whether the profile is loaded (the `profiles` file: `name (mode)` lines).
pub fn loaded_in(profiles: &str) -> bool {
    profiles.lines().any(|l| l.split(" (").next() == Some(PROFILE))
}

/// Root: make sure the profile is loaded, writing and loading it when it
/// isn't (or when the file on disk isn't this build's). An `Err` says why
/// it can't be; the `--userns` start is then refused (the job never runs
/// without the namespace it asked for, and never under another profile).
pub fn ensure_loaded() -> Result<(), String> {
    let on_disk = std::fs::read_to_string(PROFILE_PATH).ok();
    let loaded = std::fs::read_to_string(LOADED).map(|t| loaded_in(&t)).unwrap_or(false);
    if loaded && on_disk.as_deref() == Some(PROFILE_TEXT) {
        return Ok(());
    }
    let dir = Path::new(PROFILE_PATH).parent().ok_or("no profile folder")?;
    if !dir.is_dir() {
        return Err(format!("{} doesn't exist (AppArmor isn't installed)", dir.display()));
    }
    if on_disk.as_deref() != Some(PROFILE_TEXT) {
        crate::util::atomic_write(Path::new(PROFILE_PATH), PROFILE_TEXT.as_bytes(), 0o644).map_err(|e| format!("write {PROFILE_PATH}: {e}"))?;
    }
    let parser = ["/usr/sbin/apparmor_parser", "/sbin/apparmor_parser", "/usr/bin/apparmor_parser"]
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
        .ok_or("apparmor_parser not found")?;
    let out = std::process::Command::new(parser)
        .args(["-r", "-W", PROFILE_PATH])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("apparmor_parser: {e}"))?;
    if !out.status.success() {
        return Err(format!("apparmor_parser -r {PROFILE_PATH}: {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
    let now = std::fs::read_to_string(LOADED).map_err(|e| format!("read {LOADED}: {e}"))?;
    if !loaded_in(&now) {
        return Err(format!("{PROFILE} isn't loaded after apparmor_parser"));
    }
    crate::nlog!("job user namespaces: loaded the AppArmor profile {PROFILE} for --userns units (this kernel restricts user namespaces)");
    Ok(())
}

/// The refusal text when it can't be loaded: why, and the fix.
pub fn unavailable(why: &str) -> String {
    format!(
        "this node's kernel restricts user namespaces (kernel.apparmor_restrict_unprivileged_userns=1) and K2 couldn't load its AppArmor profile for --userns jobs ({why}). The node's owner can fix it: install AppArmor's tools (apparmor_parser) and run the job again, or load the profile by hand with: sudo apparmor_parser -r {PROFILE_PATH}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_allows_userns_and_nothing_else() {
        let body: Vec<&str> = PROFILE_TEXT.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        assert_eq!(body, ["abi <abi/4.0>,", "profile k2-job-userns flags=(unconfined) {", "userns,", "}"], "{PROFILE_TEXT}");
        assert!(!PROFILE_TEXT.contains("include"), "nothing can widen it");
    }

    #[test]
    fn the_restriction_and_the_loaded_list_are_read_exactly() {
        assert!(restricts(Some("1\n")));
        assert!(!restricts(Some("0\n")));
        assert!(!restricts(None));
        assert!(loaded_in("unprivileged_userns (enforce)\nk2-job-userns (unconfined)\n"));
        assert!(!loaded_in("k2-job-userns-old (unconfined)\nchrome (unconfined)\n"));
        assert!(!loaded_in(""));
        let u = unavailable("apparmor_parser not found");
        assert!(u.contains("apparmor_restrict_unprivileged_userns=1") && u.contains("sudo apparmor_parser -r /etc/apparmor.d/k2-job-userns"), "{u}");
    }
}
