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
use k2_brand::PRODUCT_NAME;

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
pub const PROFILE_TEXT: &str = concat!(
    "# ",
    k2_brand::product_name!(),
    " compute: a --userns job's unit (the node owner's own server's job,
# granted by the node's owner) may create user namespaces on kernels that
# restrict them for unconfined processes
# (kernel.apparmor_restrict_unprivileged_userns=1). Otherwise unconfined, as
# every job unit is. Attached to nothing: only a unit naming it
# (AppArmorProfile=k2-job-userns) runs under it. Written by k2-node.
abi <abi/4.0>,

profile k2-job-userns flags=(unconfined) {
  userns,
}
"
);

/// Whether the kernel restricts user namespaces for unconfined processes
/// (the sysctl's text; `None` = no such sysctl).
pub fn restricts(sysctl: Option<&str>) -> bool {
    sysctl.is_some_and(|s| s.trim() == "1")
}

/// [`restricts`] for this machine.
pub fn kernel_restricts() -> bool {
    restricts(std::fs::read_to_string(RESTRICT_SYSCTL).ok().as_deref())
}

/// Whether a unit gets the profile: a `--userns` unit, on a kernel that
/// restricts user namespaces. Only on demand: nothing is written or loaded
/// for any other unit, and never ahead of one (Steward rev 3, 2).
pub fn needs_profile(userns: bool, restricts: bool) -> bool {
    userns && restricts
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

/// Root: unload the profile (when loaded) and delete its file (a link is
/// removed, never followed). An `Err` says what couldn't be done.
pub fn unload_and_remove() -> Result<(), String> {
    unload_and_remove_at(Path::new(PROFILE_PATH))
}

/// [`unload_and_remove`] for a profile file at `path` (tests).
pub fn unload_and_remove_at(path: &Path) -> Result<(), String> {
    let loaded = std::fs::read_to_string(LOADED).map(|t| loaded_in(&t)).unwrap_or(false);
    if loaded {
        parser(&["-R"])?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove {}: {e}", path.display())),
    }
    if loaded {
        crate::nlog!("job user namespaces: unloaded the AppArmor profile {PROFILE} (no --userns unit is left)");
    }
    Ok(())
}

/// The `--userns` units that run under the profile right now. The profile
/// lives exactly as long as one does: loaded before the first starts,
/// unloaded and its file deleted when the last one is released (so with
/// no `--userns` job on the node, or `--userns` turned off, there's none).
#[derive(Debug, Default)]
pub struct Live {
    units: std::collections::BTreeSet<String>,
}

impl Live {
    /// A unit about to start under the profile.
    pub fn start(&mut self, unit: &str) {
        self.units.insert(unit.to_string());
    }
    /// A unit released (or that never started): true when it was one and
    /// none is left, so the profile goes now.
    pub fn end(&mut self, unit: &str) -> bool {
        self.units.remove(unit) && self.units.is_empty()
    }
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}

/// This executor's [`Live`] set (one per process; every load, unload and
/// change of the set happens under it).
pub fn live() -> &'static std::sync::Mutex<Live> {
    static L: std::sync::OnceLock<std::sync::Mutex<Live>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

/// Root, once per executor process (its first `Ready`): a profile left
/// loaded or on disk by an executor that ended (a crash, an update) goes
/// unless a `--userns` unit still runs under it; those still running are
/// counted again. `running` names the job units with this profile.
pub fn sweep(running: &[String]) {
    let mut l = live().lock().unwrap_or_else(|e| e.into_inner());
    for u in running {
        l.start(u);
    }
    if l.is_empty() {
        let loaded = std::fs::read_to_string(LOADED).map(|t| loaded_in(&t)).unwrap_or(false);
        if loaded || Path::new(PROFILE_PATH).exists() {
            if let Err(e) = unload_and_remove() {
                crate::nlog!("job user namespaces: couldn't remove the leftover AppArmor profile {PROFILE}: {e}");
            }
        }
    }
}

/// The node doctor's line while the profile is loaded (or on disk), else
/// none. Readable without root: the file is 0644 and the loaded list is
/// world-readable where AppArmor runs.
pub fn doctor_line() -> Option<String> {
    let loaded = std::fs::read_to_string(LOADED).map(|t| loaded_in(&t)).unwrap_or(false);
    let on_disk = Path::new(PROFILE_PATH).exists();
    doctor_line_for(loaded, on_disk)
}

/// [`doctor_line`] from the two facts.
pub fn doctor_line_for(loaded: bool, on_disk: bool) -> Option<String> {
    match (loaded, on_disk) {
        (true, _) => Some(format!("AppArmor profile {PROFILE} is loaded: this node's --userns job units may make user namespaces on this kernel ({PROFILE_PATH}; it goes when the last one ends)")),
        (false, true) => Some(format!("AppArmor profile file {PROFILE_PATH} is on disk but not loaded (a --userns unit starting writes and loads it again; it goes when the last one ends)")),
        (false, false) => None,
    }
}

/// The refusal text when it can't be loaded: why, and the fix.
pub fn unavailable(why: &str) -> String {
    format!(
        "this node's kernel restricts user namespaces (kernel.apparmor_restrict_unprivileged_userns=1) and {PRODUCT_NAME} couldn't load its AppArmor profile for --userns jobs ({why}). The node's owner can fix it: install AppArmor's tools (apparmor_parser) and run the job again, or load the profile by hand with: sudo apparmor_parser -r {PROFILE_PATH}",
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

    /// Fail-first (Steward rev 3, 3): the profile lives as long as a
    /// `--userns` unit: the last one's release takes it away, an earlier
    /// one's doesn't, a unit that never had it changes nothing.
    #[test]
    fn the_last_userns_units_release_removes_the_profile() {
        let mut l = Live::default();
        l.start("k2job-a-g1.service");
        l.start("k2job-b-g1.service");
        assert!(!l.end("k2job-a-g1.service"), "one is left");
        assert!(!l.end("k2job-plain-g1.service"), "never had it");
        assert!(l.end("k2job-b-g1.service"), "the last one: unload now");
        assert!(!l.end("k2job-b-g1.service"), "already gone");
        assert!(l.is_empty());
        let line = doctor_line_for(true, true).unwrap();
        assert!(line.contains("k2-job-userns is loaded") && !line.contains('\n'), "{line}");
        assert!(doctor_line_for(false, true).unwrap().contains("on disk but not loaded"));
        assert_eq!(doctor_line_for(false, false), None, "no profile: no line");
    }

    /// Fail-first (Steward rev 3, 2): on demand only. A unit without
    /// `--userns` never gets it, on any kernel; a `--userns` unit only where
    /// the kernel restricts.
    #[test]
    fn the_profile_is_for_userns_units_on_restricting_kernels_only() {
        assert!(needs_profile(true, true));
        assert!(!needs_profile(false, true), "no --userns unit: nothing written or loaded");
        assert!(!needs_profile(true, false), "an unrestricted kernel needs none");
        assert!(!needs_profile(false, false));
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
