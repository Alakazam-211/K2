//! Cache generations on the node (prd-compute-blacksmith-class-v1 §4.1,
//! slice B1). Behind the machine owner's `cache-gens` switch, off by
//! default.
//!
//! A **warm set** is what a job reuses (a project's cargo `target/`, its
//! `CARGO_HOME` and its sccache dir, kept together). A **lineage** is a
//! chain of **generations** of one warm set; a job never writes one. It
//! gets a **fork** (a clone of the head) and the node commits the fork as
//! the next generation, or discards it, when the job ends.
//!
//! **Walls first** (vs-live BK-2). Generations live in `<root>/gens/` of
//! each server's job-facing root (the node home for the owner,
//! `<home>-t/<slot>` for a tenant). No job may reach it:
//! - Linux: every job unit has it in `InaccessiblePaths`
//!   (`admin::jobs::gone_for`), and the executor says so (`Ready.gens_wall`);
//! - macOS: every Seatbelt profile denies it
//!   (`wall::seatbelt_tenant_profile`);
//! - everywhere: it is the node user's, mode 0700, with no ACL
//!   ([`ensure_closed`], checked again before every fork by [`is_closed`]).
//!
//! A node with no job wall (Linux wall `none`, `--dev`) refuses the switch
//! (`cache_gens_needs_wall`) and never forks.

use std::path::{Path, PathBuf};

use k2_node_proto::gens::codes::CACHE_GENS_NEEDS_WALL;
use serde::{Deserialize, Serialize};

/// The folder under a server's job-facing root that holds its generations.
pub const GENS_DIR: &str = "gens";

/// `<root>/gens`.
pub fn gens_root(root: &Path) -> PathBuf {
    root.join(GENS_DIR)
}

// ── the switch (BK-16) ───────────────────────────────────────────────

/// The machine owner's switch: its own root-owned file in the config dir,
/// not a key in the strict `admin.toml` (a rolled-back executor would
/// refuse an unknown key there; `jobs.toml` did the same).
pub const SWITCH_FILE: &str = "cache-gens.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchFile {
    /// Keep cache generations on this node.
    #[serde(default)]
    pub on: bool,
    /// A trusted lineage whose head grows past this many GB resets (its
    /// next trusted check builds cold). Absent: half the warm budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage_max_gb: Option<u64>,
}

/// Read the switch. Missing, unreadable or malformed: off (fail closed).
pub fn read_switch(config_dir: &Path) -> SwitchFile {
    match std::fs::read_to_string(config_dir.join(SWITCH_FILE)) {
        Ok(s) => toml::from_str(&s).unwrap_or_default(),
        Err(_) => SwitchFile::default(),
    }
}

/// Write the switch (root, 0644).
pub fn write_switch(config_dir: &Path, f: &SwitchFile) -> Result<(), String> {
    let body = toml::to_string(f).map_err(|e| format!("encode {SWITCH_FILE}: {e}"))?;
    let text = format!(
        "# K2 compute cache generations (prd-compute-blacksmith-class-v1 §4.1). The machine owner's switch.\n\
         # Change it only at this machine: sudo k2-node admin cache-gens on|off\n{body}"
    );
    crate::util::atomic_write(&config_dir.join(SWITCH_FILE), text.as_bytes(), 0o644)
}

/// Why this node may not keep generations, if it may not: no job wall
/// (Linux `none`, `--dev`), or a Linux executor that doesn't hide the
/// gens root from its units (an older one: `Ready.gens_wall` false).
pub fn wall_refusal(wall: &crate::wall::Wall, dev_unwalled: bool, exec_gens_wall: bool) -> Option<String> {
    use crate::wall::Wall;
    match wall {
        _ if dev_unwalled => Some(format!("{CACHE_GENS_NEEDS_WALL}: a --dev node runs jobs without the job wall")),
        Wall::None { why } => Some(format!("{CACHE_GENS_NEEDS_WALL}: this node has no job wall ({why})")),
        Wall::JobUser { .. } if !exec_gens_wall => {
            Some(format!("{CACHE_GENS_NEEDS_WALL}: the node's executor is older and doesn't hide the cache generations from jobs (update k2-node-admin)"))
        }
        Wall::JobUser { .. } | Wall::Seatbelt => None,
    }
}

// ── the closed folder ────────────────────────────────────────────────

/// Make `dir` the node's closed folder: a real directory (never through a
/// link), owned by this process's uid, mode 0700, with every ACL entry
/// removed (Linux POSIX access + default ACLs; macOS extended ACLs). Then
/// check it with [`is_closed`].
pub fn ensure_closed(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(format!("{} exists and isn't a plain folder", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        Err(e) => return Err(format!("stat {}: {e}", dir.display())),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("chmod {}: {e}", dir.display()))?;
    acl::strip(dir)?;
    is_closed(dir)
}

/// Is `dir` closed: a real directory owned by this uid, no group or other
/// bits, no ACL? Every fork asks first; a no is a refusal, never a warning.
pub fn is_closed(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(dir).map_err(|e| format!("stat {}: {e}", dir.display()))?;
    if !m.is_dir() {
        return Err(format!("{} isn't a plain folder", dir.display()));
    }
    // SAFETY: no arguments.
    let me = unsafe { libc::geteuid() };
    if m.uid() != me {
        return Err(format!("{} is owned by uid {}, not the node's {me}", dir.display(), m.uid()));
    }
    if m.mode() & 0o077 != 0 {
        return Err(format!("{} is mode {:o}: group or other may enter it", dir.display(), m.mode() & 0o7777));
    }
    if acl::has_any(dir)? {
        return Err(format!("{} carries an ACL", dir.display()));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod acl {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const NAMES: [&std::ffi::CStr; 2] = [c"system.posix_acl_access", c"system.posix_acl_default"];

    fn cpath(p: &Path) -> Result<CString, String> {
        CString::new(p.as_os_str().as_bytes()).map_err(|_| format!("{}: NUL in path", p.display()))
    }

    pub fn strip(p: &Path) -> Result<(), String> {
        let c = cpath(p)?;
        for n in NAMES {
            // SAFETY: valid C strings.
            if unsafe { libc::lremovexattr(c.as_ptr(), n.as_ptr()) } != 0 {
                let e = std::io::Error::last_os_error();
                if !matches!(e.raw_os_error(), Some(libc::ENODATA) | Some(libc::EOPNOTSUPP)) {
                    return Err(format!("remove the ACL of {}: {e}", p.display()));
                }
            }
        }
        Ok(())
    }

    pub fn has_any(p: &Path) -> Result<bool, String> {
        let c = cpath(p)?;
        for n in NAMES {
            // SAFETY: valid C strings; a size query (no buffer).
            let r = unsafe { libc::lgetxattr(c.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0) };
            if r >= 0 {
                return Ok(true);
            }
            let e = std::io::Error::last_os_error();
            if !matches!(e.raw_os_error(), Some(libc::ENODATA) | Some(libc::EOPNOTSUPP)) {
                return Err(format!("read the ACL of {}: {e}", p.display()));
            }
        }
        Ok(false)
    }
}

#[cfg(target_os = "macos")]
mod acl {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
    const ACL_FIRST_ENTRY: c_int = 0;

    extern "C" {
        fn acl_get_link_np(path: *const c_char, ty: c_int) -> *mut c_void;
        fn acl_set_link_np(path: *const c_char, ty: c_int, acl: *mut c_void) -> c_int;
        fn acl_init(count: c_int) -> *mut c_void;
        fn acl_free(obj: *mut c_void) -> c_int;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry_p: *mut *mut c_void) -> c_int;
    }

    fn cpath(p: &Path) -> Result<CString, String> {
        CString::new(p.as_os_str().as_bytes()).map_err(|_| format!("{}: NUL in path", p.display()))
    }

    pub fn strip(p: &Path) -> Result<(), String> {
        let c = cpath(p)?;
        // SAFETY: acl_init returns an owned ACL we free; valid C string.
        unsafe {
            let empty = acl_init(0);
            if empty.is_null() {
                return Err(format!("acl_init: {}", std::io::Error::last_os_error()));
            }
            let r = acl_set_link_np(c.as_ptr(), ACL_TYPE_EXTENDED, empty);
            let e = std::io::Error::last_os_error();
            acl_free(empty);
            if r != 0 {
                return Err(format!("remove the ACL of {}: {e}", p.display()));
            }
        }
        Ok(())
    }

    pub fn has_any(p: &Path) -> Result<bool, String> {
        let c = cpath(p)?;
        // SAFETY: valid C string; the returned ACL is freed.
        unsafe {
            let a = acl_get_link_np(c.as_ptr(), ACL_TYPE_EXTENDED);
            if a.is_null() {
                let e = std::io::Error::last_os_error();
                return match e.raw_os_error() {
                    Some(libc::ENOENT) => Ok(false),
                    _ => Err(format!("read the ACL of {}: {e}", p.display())),
                };
            }
            let mut entry: *mut c_void = std::ptr::null_mut();
            let any = acl_get_entry(a, ACL_FIRST_ENTRY, &mut entry) == 0;
            acl_free(a);
            Ok(any)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod acl {
    use std::path::Path;
    pub fn strip(_p: &Path) -> Result<(), String> {
        Ok(())
    }
    pub fn has_any(_p: &Path) -> Result<bool, String> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wall::Wall;

    #[test]
    fn the_switch_is_off_unless_the_file_says_on() {
        let d = crate::util::temp_dir("gens-switch");
        assert!(!read_switch(&d).on, "no file: off");
        std::fs::write(d.join(SWITCH_FILE), "on = true\nbogus = 1\n").unwrap();
        assert!(!read_switch(&d).on, "a malformed file: off");
        write_switch(&d, &SwitchFile { on: true, lineage_max_gb: Some(40) }).unwrap();
        assert_eq!(read_switch(&d), SwitchFile { on: true, lineage_max_gb: Some(40) });
        write_switch(&d, &SwitchFile::default()).unwrap();
        assert!(!read_switch(&d).on);
    }

    /// `sudo k2-node admin cache-gens on` on a machine with no job wall is
    /// refused by name and writes nothing; off always works.
    #[test]
    fn the_switch_is_refused_without_a_wall() {
        use crate::admin::cli::cache_gens_unchecked;
        let d = crate::util::temp_dir("gens-switch-wall");
        let e = cache_gens_unchecked(&d, true, false).unwrap_err();
        assert!(e.starts_with(CACHE_GENS_NEEDS_WALL), "{e}");
        assert!(!d.join(SWITCH_FILE).exists(), "nothing written");
        assert!(cache_gens_unchecked(&d, true, true).unwrap().contains("off → on"));
        assert!(read_switch(&d).on);
        assert!(cache_gens_unchecked(&d, false, false).unwrap().contains("on → off"), "turning it off needs no wall");
        assert!(!read_switch(&d).on);
    }

    #[test]
    fn no_wall_no_generations() {
        let none = Wall::None { why: "no job user".into() };
        let r = wall_refusal(&none, false, true).unwrap();
        assert!(r.starts_with(CACHE_GENS_NEEDS_WALL), "{r}");
        assert!(wall_refusal(&Wall::Seatbelt, true, true).unwrap().starts_with(CACHE_GENS_NEEDS_WALL), "--dev runs unwalled");
        let ju = Wall::JobUser { user: "k2job".into() };
        assert!(wall_refusal(&ju, false, false).unwrap().contains("executor"), "an executor that doesn't hide gens/");
        assert_eq!(wall_refusal(&ju, false, true), None);
        assert_eq!(wall_refusal(&Wall::Seatbelt, false, false), None, "macOS: the node writes the Seatbelt deny itself");
    }

    #[test]
    fn the_gens_root_is_closed_and_a_loosened_one_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("gens-closed");
        let g = gens_root(&d);
        std::fs::create_dir(&g).unwrap();
        std::fs::set_permissions(&g, std::fs::Permissions::from_mode(0o2770)).unwrap();
        assert!(is_closed(&g).is_err(), "group-writable is not closed");
        ensure_closed(&g).unwrap();
        assert_eq!(std::fs::metadata(&g).unwrap().permissions().mode() & 0o7777, 0o700);
        is_closed(&g).unwrap();
        std::fs::set_permissions(&g, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(is_closed(&g).unwrap_err().contains("mode 750"));
        // A link where the folder should be is refused, never followed.
        let l = d.join("gens-link");
        std::os::unix::fs::symlink(&g, &l).unwrap();
        assert!(ensure_closed(&l).unwrap_err().contains("isn't a plain folder"));
        assert!(is_closed(&l).is_err());
    }

    /// An ACL on the gens root (here a default ACL a parent handed down, as
    /// a tenant's `cache/` does) is stripped, and one put back is refused.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_acl_on_the_gens_root_is_stripped_and_refused() {
        let setfacl = ["/usr/bin/setfacl", "/bin/setfacl"].into_iter().map(Path::new).find(|p| p.is_file());
        let setfacl = setfacl.expect("setfacl is needed for this test (the acl package)");
        let d = crate::util::temp_dir("gens-acl");
        let me = unsafe { libc::geteuid() };
        let parent = d.join("p");
        std::fs::create_dir(&parent).unwrap();
        let ok = std::process::Command::new(setfacl).args(["-m", &format!("d:u:{me}:rwx,d:g::rwx,d:m::rwx")]).arg(&parent).status().unwrap();
        assert!(ok.success(), "setfacl on a temp dir");
        let g = gens_root(&parent);
        std::fs::create_dir(&g).unwrap();
        assert!(acl::has_any(&g).unwrap(), "the default ACL was inherited");
        ensure_closed(&g).unwrap();
        assert!(!acl::has_any(&g).unwrap());
        let ok = std::process::Command::new(setfacl).args(["-m", &format!("u:{me}:rwx")]).arg(&g).status().unwrap();
        assert!(ok.success());
        assert!(is_closed(&g).unwrap_err().contains("ACL"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_extended_acl_on_the_gens_root_is_stripped_and_refused() {
        let d = crate::util::temp_dir("gens-acl");
        let g = gens_root(&d);
        std::fs::create_dir(&g).unwrap();
        let ok = std::process::Command::new("/bin/chmod").args(["+a", "everyone allow list,search"]).arg(&g).status().unwrap();
        assert!(ok.success(), "chmod +a on a temp dir");
        assert!(acl::has_any(&g).unwrap());
        assert!(is_closed(&g).unwrap_err().contains("ACL"));
        ensure_closed(&g).unwrap();
        assert!(!acl::has_any(&g).unwrap());
    }
}
