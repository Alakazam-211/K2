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
//! granted by the node's owner). Before every such unit on a restricting
//! kernel root checks the profile file is exactly ours (root's, 0644, this
//! build's bytes by SHA-256; else it writes ours over it), loads the profile
//! from its own bytes and names it in that unit (`AppArmorProfile=`); no
//! other unit, job or process gets it, and the node's own sysctl never
//! changes.
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

/// What root checks about the profile file before every use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    /// A regular file, opened without following a link.
    pub regular: bool,
    pub uid: u32,
    /// The permission bits (`st_mode & 0o7777`).
    pub mode: u32,
}

/// The SHA-256 of [`PROFILE_TEXT`] (what the file must hash to).
pub fn profile_sha256() -> String {
    k2_node_proto::crypto::sha256_hex(PROFILE_TEXT.as_bytes())
}

/// Whether the file is ours, exactly: a regular file (never a link),
/// root's, 0644, and byte for byte [`PROFILE_TEXT`] (compared by
/// SHA-256). An `Err` says what's wrong; such a file is never loaded.
pub fn check_file(f: FileFacts, bytes: &[u8]) -> Result<(), String> {
    if !f.regular {
        return Err(format!("{PROFILE_PATH} isn't a regular file"));
    }
    if f.uid != 0 {
        return Err(format!("{PROFILE_PATH} is owned by uid {}, not root", f.uid));
    }
    if f.mode != 0o644 {
        return Err(format!("{PROFILE_PATH} has mode {:o}, not 644", f.mode));
    }
    let got = k2_node_proto::crypto::sha256_hex(bytes);
    if got != profile_sha256() {
        return Err(format!("{PROFILE_PATH} isn't the profile this k2-node carries (sha256 {got})"));
    }
    Ok(())
}

/// The file's facts and bytes, opened with `O_NOFOLLOW` (`None`: no file).
fn read_file(path: &Path) -> std::io::Result<Option<(FileFacts, Vec<u8>)>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut f = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // A link (ELOOP) or anything else unreadable: not ours.
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(Some((FileFacts { regular: false, uid: u32::MAX, mode: 0 }, Vec::new()))),
        Err(e) => return Err(e),
    };
    let m = f.metadata()?;
    let facts = FileFacts { regular: m.is_file(), uid: m.uid(), mode: m.mode() & 0o7777 };
    let mut bytes = Vec::new();
    if facts.regular {
        f.by_ref().take(64 * 1024).read_to_end(&mut bytes)?;
    }
    Ok(Some((facts, bytes)))
}

/// Root, before every `--userns` unit that names the profile: the file
/// must be ours ([`check_file`]); one that isn't (missing, a link, another
/// owner or mode, changed bytes: an attachment path or a rule added) is
/// replaced with ours, checked again, and only then is the profile loaded,
/// from this build's own bytes (never from the file), so a changed file is
/// never loaded as ours. An `Err` refuses the start.
pub fn ensure_loaded() -> Result<(), String> {
    ensure_loaded_at(Path::new(PROFILE_PATH))
}

/// [`ensure_loaded`] for a profile file at `path` (tests).
pub fn ensure_loaded_at(path: &Path) -> Result<(), String> {
    let dir = path.parent().ok_or("no profile folder")?;
    if !dir.is_dir() {
        return Err(format!("{} doesn't exist (AppArmor isn't installed)", dir.display()));
    }
    let state = read_file(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let ok = state.as_ref().is_some_and(|(f, b)| check_file(*f, b).is_ok());
    if !ok {
        if let Some((f, b)) = &state {
            crate::nlog!("job user namespaces: {} isn't ours ({}); writing this build's profile over it", path.display(), check_file(*f, b).err().unwrap_or_default());
        }
        crate::util::atomic_write(path, PROFILE_TEXT.as_bytes(), 0o644).map_err(|e| format!("write {}: {e}", path.display()))?;
        let again = read_file(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        match again {
            Some((f, b)) => check_file(f, &b)?,
            None => return Err(format!("{} is gone right after it was written", path.display())),
        }
    }
    parser(&["-r"])?;
    let now = std::fs::read_to_string(LOADED).map_err(|e| format!("read {LOADED}: {e}"))?;
    if !loaded_in(&now) {
        return Err(format!("{PROFILE} isn't loaded after apparmor_parser"));
    }
    Ok(())
}

/// `apparmor_parser <args>` with this build's profile on stdin.
fn parser(args: &[&str]) -> Result<(), String> {
    use std::io::Write;
    let bin = ["/usr/sbin/apparmor_parser", "/sbin/apparmor_parser", "/usr/bin/apparmor_parser"]
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
        .ok_or("apparmor_parser not found")?;
    let mut child = std::process::Command::new(bin)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("apparmor_parser: {e}"))?;
    if let Some(mut i) = child.stdin.take() {
        i.write_all(PROFILE_TEXT.as_bytes()).map_err(|e| format!("apparmor_parser stdin: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("apparmor_parser: {e}"))?;
    if !out.status.success() {
        return Err(format!("apparmor_parser {}: {}: {}", args.join(" "), out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
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

    /// Fail-first (Steward rev 3, 1): only this build's file, root's, 0644,
    /// is ours; a tampered one (an attachment path, an extra rule, another
    /// owner or mode, a link) never is.
    #[test]
    fn only_this_builds_root_owned_0644_file_is_ours() {
        let ours = FileFacts { regular: true, uid: 0, mode: 0o644 };
        assert_eq!(check_file(ours, PROFILE_TEXT.as_bytes()), Ok(()));
        let attached = PROFILE_TEXT.replace("profile k2-job-userns flags", "profile k2-job-userns /usr/bin/** flags");
        let extra = PROFILE_TEXT.replace("  userns,\n", "  userns,\n  capability,\n");
        assert_ne!(attached, PROFILE_TEXT);
        assert_ne!(extra, PROFILE_TEXT);
        for (why, f, bytes) in [
            ("an attachment path", ours, attached.as_bytes()),
            ("an extra rule", ours, extra.as_bytes()),
            ("empty", ours, b"".as_slice()),
            ("owned by a user", FileFacts { uid: 1000, ..ours }, PROFILE_TEXT.as_bytes()),
            ("group-writable", FileFacts { mode: 0o664, ..ours }, PROFILE_TEXT.as_bytes()),
            ("setuid bit", FileFacts { mode: 0o4644, ..ours }, PROFILE_TEXT.as_bytes()),
            ("a link", FileFacts { regular: false, ..ours }, PROFILE_TEXT.as_bytes()),
        ] {
            assert!(check_file(f, bytes).is_err(), "{why} must not pass as ours");
        }
        assert_eq!(profile_sha256().len(), 64);
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
