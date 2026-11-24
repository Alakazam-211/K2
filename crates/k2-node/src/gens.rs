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
use k2_brand::PRODUCT_NAME;

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
        "# {PRODUCT_NAME} compute cache generations (prd-compute-blacksmith-class-v1 §4.1). The machine owner's switch.\n\
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

// ── the filesystem probe and the clone backends (§4.1.3) ─────────────

const PROBE_DIR: &str = ".probe";

/// The filesystem `dir` is on: Linux's `statfs` magic, macOS's type name.
pub fn fs_name(dir: &Path) -> Result<String, String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).map_err(|_| "NUL in path".to_string())?;
    // SAFETY: zeroed statfs, valid C string.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Err(format!("statfs {}: {}", dir.display(), std::io::Error::last_os_error()));
    }
    #[cfg(target_os = "linux")]
    {
        #[allow(clippy::unnecessary_cast)]
        let magic = st.f_type as i64;
        Ok(match magic {
            0x9123_683E => "btrfs",
            0x5846_5342 => "xfs",
            0xEF53 => "ext4",
            0x0102_1994 => "tmpfs",
            0x2FC1_2FC1 => "zfs",
            0x794C_7630 => "overlay",
            0xF2F5_2010 => "f2fs",
            0x6969 => "nfs",
            other => return Ok(format!("0x{other:x}")),
        }
        .to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: f_fstypename is a NUL-terminated C string.
        let name = unsafe { std::ffi::CStr::from_ptr(st.f_fstypename.as_ptr()) };
        Ok(name.to_string_lossy().into_owned())
    }
}

fn find_tool(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].iter().map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

fn run_tool(prog: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", prog.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{} {}: {}", prog.display(), out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// `FICLONE` one file onto another (Linux reflink: btrfs, XFS with
/// `reflink=1`).
#[cfg(target_os = "linux")]
fn ficlone(src: &Path, dst: &Path) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let s = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let d = std::fs::OpenOptions::new().write(true).create_new(true).open(dst).map_err(|e| e.to_string())?;
    // SAFETY: two open fds.
    if unsafe { libc::ioctl(d.as_raw_fd(), FICLONE as _, s.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn clonefile(src: &Path, dst: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn clonefile(src: *const std::os::raw::c_char, dst: *const std::os::raw::c_char, flags: u32) -> std::os::raw::c_int;
    }
    const CLONE_NOFOLLOW: u32 = 0x0001;
    let s = std::ffi::CString::new(src.as_os_str().as_bytes()).map_err(|_| "NUL in path".to_string())?;
    let d = std::ffi::CString::new(dst.as_os_str().as_bytes()).map_err(|_| "NUL in path".to_string())?;
    // SAFETY: valid C strings. Ownership and modes come with the clone
    // (the node clones its own trees).
    if unsafe { clonefile(s.as_ptr(), d.as_ptr(), CLONE_NOFOLLOW) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// Probe the closed folder `gens` (the owner's gens root): the filesystem
/// and the best clone method that works there, tried for real in a
/// scratch folder that is removed again.
pub fn probe(gens: &Path) -> Result<k2_node_proto::gens::CacheFs, String> {
    use k2_node_proto::gens::CacheFs;
    is_closed(gens)?;
    let fs = fs_name(gens)?;
    let dir = gens.join(PROBE_DIR);
    let _ = remove_tree(&dir);
    std::fs::create_dir(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let method = probe_in(&dir, &fs);
    let _ = remove_tree(&dir);
    Ok(CacheFs { fs, clone: method, volume: volume_marker(gens) })
}

fn probe_in(dir: &Path, fs: &str) -> k2_node_proto::gens::CloneMethod {
    use k2_node_proto::gens::CloneMethod;
    let a = dir.join("a");
    if fs == "btrfs" && new_set_dir(CloneMethod::Subvolume, &a).is_ok() {
        let ok = std::fs::write(a.join("f"), b"k2").is_ok() && clone_tree(CloneMethod::Subvolume, &a, &dir.join("b")).is_ok();
        if ok && std::fs::read(dir.join("b/f")).is_ok_and(|b| b == b"k2") {
            return CloneMethod::Subvolume;
        }
    }
    let _ = remove_tree(&a);
    let _ = std::fs::create_dir(&a);
    if std::fs::write(a.join("f"), b"k2").is_err() {
        return CloneMethod::Copy;
    }
    #[cfg(target_os = "linux")]
    if ficlone(&a.join("f"), &dir.join("f2")).is_ok() && find_tool("cp").is_some() {
        return CloneMethod::Reflink;
    }
    #[cfg(target_os = "macos")]
    if clonefile(&a, &dir.join("c")).is_ok() && std::fs::read(dir.join("c/f")).is_ok_and(|b| b == b"k2") {
        return CloneMethod::Clonefile;
    }
    CloneMethod::Copy
}

/// The marker `k2-node setup --cache-volume` leaves at the volume's top.
pub const VOLUME_MARKER: &str = ".k2-cache-volume";

fn volume_marker(gens: &Path) -> bool {
    gens.parent().is_some_and(|root| root.join(VOLUME_MARKER).is_file())
}

/// Make an empty warm set folder `dst` (a cold fork): a subvolume on
/// btrfs (so its generations snapshot in O(1)), else a plain folder.
pub fn new_set_dir(method: k2_node_proto::gens::CloneMethod, dst: &Path) -> Result<(), String> {
    use k2_node_proto::gens::CloneMethod;
    if method == CloneMethod::Subvolume {
        let btrfs = find_tool("btrfs").ok_or("btrfs not found")?;
        return run_tool(&btrfs, &["subvolume".as_ref(), "create".as_ref(), dst.as_os_str()]);
    }
    std::fs::create_dir(dst).map_err(|e| format!("create {}: {e}", dst.display()))
}

/// Clone the tree `src` to `dst` (which must not exist), never following
/// a link, with `method`. The same filesystem only.
pub fn clone_tree(method: k2_node_proto::gens::CloneMethod, src: &Path, dst: &Path) -> Result<(), String> {
    use k2_node_proto::gens::CloneMethod;
    if std::fs::symlink_metadata(dst).is_ok() {
        return Err(format!("{} exists", dst.display()));
    }
    match std::fs::symlink_metadata(src) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(format!("{} isn't a plain folder", src.display())),
        Err(e) => return Err(format!("{}: {e}", src.display())),
    }
    let cp = || find_tool("cp").ok_or_else(|| "cp not found".to_string());
    match method {
        CloneMethod::Subvolume => {
            let btrfs = find_tool("btrfs").ok_or("btrfs not found")?;
            match run_tool(&btrfs, &["subvolume".as_ref(), "snapshot".as_ref(), src.as_os_str(), dst.as_os_str()]) {
                Ok(()) => Ok(()),
                // A generation that isn't a subvolume (made before the
                // node had btrfs tools): file by file instead.
                Err(_) => run_tool(&cp()?, &["-a".as_ref(), "--reflink=auto".as_ref(), "--".as_ref(), src.as_os_str(), dst.as_os_str()]),
            }
        }
        CloneMethod::Reflink => run_tool(&cp()?, &["-a".as_ref(), "--reflink=always".as_ref(), "--".as_ref(), src.as_os_str(), dst.as_os_str()]),
        #[cfg(target_os = "macos")]
        CloneMethod::Clonefile => clonefile(src, dst),
        #[cfg(not(target_os = "macos"))]
        CloneMethod::Clonefile => Err("clonefile is macOS only".into()),
        CloneMethod::Copy => {
            if cfg!(target_os = "macos") {
                run_tool(&cp()?, &["-pPR".as_ref(), "--".as_ref(), src.as_os_str(), dst.as_os_str()])
            } else {
                run_tool(&cp()?, &["-a".as_ref(), "--".as_ref(), src.as_os_str(), dst.as_os_str()])
            }
        }
    }
}

/// Remove a tree (a link is removed as a link). A btrfs subvolume goes
/// the same way: the kernel lets its owner rmdir it once it's empty.
pub fn remove_tree(p: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p).map_err(|e| format!("remove {}: {e}", p.display())),
        Ok(_) => std::fs::remove_file(p).map_err(|e| format!("remove {}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("stat {}: {e}", p.display())),
    }
}

// ── lineages (§4.1.2) ────────────────────────────────────────────────

/// Where a non-secret job's fork lives while it runs: `<root>/cache/forks/`
/// (like today's warm slots: the slot's group writes there through the
/// inherited ACL; a unit may write only its own fork's folders,
/// `ProtectSystem=strict`; on a Mac the Seatbelt profile closes every other
/// fork to writes).
pub const FORKS_DIR: &str = "forks";
/// A secret job's fork, inside its own job root (handed to its per-job uid
/// with the rest of the tree by `JobOp::Start`, BK-3).
pub const SECRET_FORK_DIR: &str = "warm";
/// The lineage's head record (node-only, in the closed gens root).
pub const HEAD_FILE: &str = "HEAD";
/// The folders a fork holds: the warm set (§4.1.2) for a cargo project.
pub const SET_TARGET: &str = "target";
/// The warm set's name for a project's cargo set.
pub const TARGET_SET: &str = "target";

/// `<root>/cache/forks`.
pub fn forks_root(root: &Path) -> PathBuf {
    root.join("cache").join(FORKS_DIR)
}

/// One warm set of one server: a project's cargo set.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SetId {
    /// The server's job-facing root.
    pub root: PathBuf,
    /// The project key (16 hex).
    pub key: String,
    /// `target`.
    pub name: String,
}

impl SetId {
    pub fn target(root: &Path, project_key: &str) -> Result<Self, String> {
        if !k2_node_proto::frames::valid_project_key(project_key) {
            return Err(format!("{project_key:?} is not a project key"));
        }
        Ok(Self { root: root.to_path_buf(), key: project_key.to_string(), name: TARGET_SET.to_string() })
    }

    pub fn dir(&self) -> PathBuf {
        gens_root(&self.root).join(&self.key).join(&self.name)
    }

    /// A lineage's folder. Never a name the submitter picks: a scratch
    /// ref becomes `s-` + its hash.
    pub fn lineage_dir(&self, lineage: &str) -> Result<PathBuf, String> {
        use k2_node_proto::gens::{parse_lineage, ref_folder, LineageKind};
        let (kind, rest) = parse_lineage(lineage)?;
        let folder = match kind {
            LineageKind::Trusted => "trusted".to_string(),
            LineageKind::Scratch => format!("s-{}", ref_folder(rest)),
            LineageKind::Release => format!("rel-{rest}"),
            LineageKind::Setup => format!("setup-{rest}"),
        };
        Ok(self.dir().join(folder))
    }
}

/// A full commit id: 40 lowercase hex.
pub fn is_commit(c: &str) -> bool {
    c.len() == 40 && c.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn gen_dir(lineage_dir: &Path, seq: u64) -> PathBuf {
    lineage_dir.join(format!("g{seq}"))
}

/// What the node knows about one generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenRecord {
    /// The producer's commit (40 hex).
    pub commit: String,
    pub gen_digest: String,
    pub parent_digest: Option<String>,
    pub producer: k2_node_proto::ci::CacheProducer,
    pub ref_checked: Option<k2_node_proto::ci::RefChecked>,
    /// The `trusted` seq this generation's chain started from: a scratch
    /// head whose base is older than `trusted`'s head is not forked
    /// (§4.1.2 rule 3).
    pub base_trusted: u64,
    pub at: i64,
    pub bytes: u64,
}

/// A lineage's `HEAD` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    pub lineage: String,
    /// The last seq given out (a reset keeps counting).
    pub seq: u64,
    /// The head generation (`g<seq>`), or `None` after a reset: the next
    /// job of the lineage builds cold and starts a new chain.
    pub gen: Option<GenRecord>,
    pub last_used: i64,
}

/// Read a lineage's head. Missing: `None`. Unreadable or malformed: an
/// error (the caller treats the lineage as cold, never as anything else).
pub fn read_head(lineage_dir: &Path) -> Result<Option<Head>, String> {
    match std::fs::read(lineage_dir.join(HEAD_FILE)) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("{}: {e}", lineage_dir.join(HEAD_FILE).display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", lineage_dir.join(HEAD_FILE).display())),
    }
}

fn write_head(lineage_dir: &Path, h: &Head) -> Result<(), String> {
    let body = serde_json::to_vec(h).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&lineage_dir.join(HEAD_FILE), &body, 0o600)
}

/// A job's fork, while it runs. Kept in node-only state (never where a
/// job can write), so a node restart (an adopted job) still finds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fork {
    pub set: SetId,
    /// The fork's folder (holds `target/`, `cargo-home/`, `sccache/`).
    pub dir: PathBuf,
    /// The lineage, seq and record of the generation forked; `None` cold.
    pub from: Option<(String, u64, GenRecord)>,
    /// Where it may commit (`trusted`, `scratch/<ref>`); `None` never.
    pub commit_to: Option<String>,
    /// Why it never commits, when `commit_to` is `None`.
    pub no_commit: Option<String>,
    pub base_trusted: u64,
    pub fork_ms: u64,
    /// The clone fell back to cold (one line), if it did.
    pub note: Option<String>,
}

impl Fork {
    /// `target/` in the fork.
    pub fn target(&self) -> PathBuf {
        self.dir.join(SET_TARGET)
    }
    /// The fork's cache dir: `cargo-home/` and `sccache/` in it
    /// (`runner::cache_paths`).
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    /// The log line: `target dir: gen trusted #41 (forked in 0.8 s)`.
    pub fn line(&self) -> String {
        let from = match &self.from {
            Some((l, seq, _)) => format!("gen {l} #{seq} (forked in {}.{} s)", self.fork_ms / 1000, (self.fork_ms % 1000) / 100),
            None => "gen cold (nothing to fork yet)".to_string(),
        };
        let to = match (&self.commit_to, &self.no_commit) {
            (Some(l), _) => format!("; saves to {l} if it qualifies"),
            (None, Some(w)) => format!("; saves nothing ({w})"),
            (None, None) => "; saves nothing".to_string(),
        };
        format!("target dir: {from}{to}{}", self.note.as_deref().map(|n| format!("; {n}")).unwrap_or_default())
    }
}

/// How a job ended, for its fork's commit (§4.1.4).
pub struct Ended<'a> {
    /// `done`, exit 0, no JUnit test failed.
    pub ok: bool,
    /// Cancelled (scratch keeps every end but this).
    pub cancelled: bool,
    /// The end in a few words when not ok (`exit 101`, `stalled`).
    pub end: String,
    /// The job's commit (40 hex).
    pub commit: String,
    pub dirty: bool,
    pub with_repos: bool,
    pub secret: bool,
    pub producer: k2_node_proto::ci::CacheProducer,
    pub ref_checked: Option<k2_node_proto::ci::RefChecked>,
    /// Rule 5 (§4.1.4): when the node fetched the remote itself, whether the
    /// commit is on `origin/<trusted branch>` after that fetch; `None` when
    /// the node didn't fetch it (the controller's word stands).
    pub on_branch: Option<Result<bool, String>>,
    /// `git merge-base --is-ancestor <a> <b>` in the node's mirror.
    pub is_ancestor: &'a (dyn Fn(&str, &str) -> Result<bool, String> + Sync),
    pub now: i64,
}

/// The node's generations: the probed filesystem and what is in flight.
#[derive(Debug)]
pub struct Gens {
    pub fs: k2_node_proto::gens::CacheFs,
    inner: std::sync::Mutex<Inflight>,
    commit: std::sync::Mutex<()>,
}

#[derive(Debug, Default)]
struct Inflight {
    /// Generation folders being cloned now (never pruned under a clone).
    cloning: std::collections::HashMap<PathBuf, u32>,
    /// Forks running now, by the lineage folder they forked or will save
    /// to (a lineage with a running fork is never evicted).
    forks: std::collections::HashMap<PathBuf, u32>,
}

/// A generation picked to fork: its lineage, seq and record.
pub type Source = (String, u64, GenRecord);
/// [`Gens::pick`]: the source (none: cold), where it may save, and why it
/// never saves when it may not.
pub type Picked = (Option<Source>, Option<String>, Option<String>);

/// What a fork asks for.
pub struct ForkAsk<'a> {
    pub set: &'a SetId,
    pub commit: k2_node_proto::gens::CacheCommit,
    pub cache_ref: Option<&'a str>,
    /// Where the fork goes (must not exist; its parent must).
    pub dst: &'a Path,
    pub now: i64,
}

impl Gens {
    pub fn new(fs: k2_node_proto::gens::CacheFs) -> Self {
        Self { fs, inner: Default::default(), commit: Default::default() }
    }

    fn head_of(&self, set: &SetId, lineage: &str) -> Option<(PathBuf, Head)> {
        let dir = set.lineage_dir(lineage).ok()?;
        match read_head(&dir) {
            Ok(Some(h)) => Some((dir, h)),
            Ok(None) => None,
            Err(e) => {
                crate::nlog!("cache generations: {e}; the lineage counts as empty");
                None
            }
        }
    }

    /// Which generation a job forks and where it may save (§4.1.2):
    /// a trusted check forks `trusted` and saves there; any other job forks
    /// `scratch/<ref>` when its base is at least as new as `trusted`'s head,
    /// else `trusted`, and saves to `scratch/<ref>`; a secret job under its
    /// own uid (`none`) forks `trusted` and saves nothing. Release builds
    /// keep no lineage in this release (B3): cold, nothing saved.
    pub fn pick(&self, set: &SetId, commit: k2_node_proto::gens::CacheCommit, cache_ref: Option<&str>) -> Result<Picked, String> {
        use k2_node_proto::gens::{check_plan_cache, CacheCommit, LINEAGE_SCRATCH, LINEAGE_TRUSTED};
        check_plan_cache(Some(commit), cache_ref)?;
        let gen_of = |l: &str| self.head_of(set, l).and_then(|(_, h)| h.gen.map(|g| (l.to_string(), h.seq, g)));
        let trusted = || gen_of(LINEAGE_TRUSTED);
        Ok(match commit {
            CacheCommit::Trusted => (trusted(), Some(LINEAGE_TRUSTED.to_string()), None),
            CacheCommit::None => (trusted(), None, Some("a secret job saves nothing".to_string())),
            CacheCommit::Release => (None, None, Some("release builds keep no warm copy on this node yet".to_string())),
            CacheCommit::Scratch => {
                let lineage = format!("{LINEAGE_SCRATCH}{}", cache_ref.unwrap_or_default());
                let t = trusted();
                let s = gen_of(&lineage);
                let from = match (s, t) {
                    (Some(s), Some(t)) if s.2.base_trusted >= t.1 => Some(s),
                    (Some(s), None) => Some(s),
                    (_, t) => t,
                };
                (from, Some(lineage), None)
            }
        })
    }

    /// Fork for a job: clone the picked generation to `ask.dst` (or make
    /// an empty set there), refusing first if the gens root isn't closed.
    pub fn fork(&self, ask: &ForkAsk) -> Result<Fork, String> {
        use k2_node_proto::gens::LINEAGE_TRUSTED;
        is_closed(&gens_root(&ask.set.root))?;
        let (from, commit_to, no_commit) = self.pick(ask.set, ask.commit, ask.cache_ref)?;
        let t0 = std::time::Instant::now();
        let mut note = None;
        let mut from = from;
        if let Some((l, seq, _)) = &from {
            let src = gen_dir(&ask.set.lineage_dir(l)?, *seq);
            *self.inner.lock().unwrap().cloning.entry(src.clone()).or_default() += 1;
            let r = clone_tree(self.fs.clone, &src, ask.dst);
            {
                let mut g = self.inner.lock().unwrap();
                if let Some(n) = g.cloning.get_mut(&src) {
                    *n -= 1;
                    if *n == 0 {
                        g.cloning.remove(&src);
                    }
                }
            }
            if let Err(e) = r {
                let _ = remove_tree(ask.dst);
                note = Some(format!("cloning {l} #{seq} failed ({e}): cold"));
                from = None;
            }
        }
        if from.is_none() {
            new_set_dir(self.fs.clone, ask.dst)?;
        }
        for d in [SET_TARGET, "cargo-home", "sccache"] {
            let p = ask.dst.join(d);
            if !p.is_dir() {
                std::fs::create_dir(&p).map_err(|e| format!("create {}: {e}", p.display()))?;
            }
        }
        let trusted_seq = self.head_of(ask.set, LINEAGE_TRUSTED).map(|(_, h)| h.seq).unwrap_or(0);
        let base_trusted = match &from {
            Some((l, seq, _)) if l == LINEAGE_TRUSTED => *seq,
            Some((_, _, g)) => g.base_trusted,
            None => trusted_seq,
        };
        if let Some((l, _, _)) = &from {
            // Under the commit lock: a head written here never undoes a
            // commit that landed in between.
            let _g = self.commit.lock().unwrap();
            if let Ok(d) = ask.set.lineage_dir(l) {
                if let Ok(Some(mut h)) = read_head(&d) {
                    h.last_used = ask.now;
                    let _ = write_head(&d, &h);
                }
            }
        }
        let fork = Fork {
            set: ask.set.clone(),
            dir: ask.dst.to_path_buf(),
            from,
            commit_to,
            no_commit,
            base_trusted,
            fork_ms: t0.elapsed().as_millis() as u64,
            note,
        };
        self.track(&fork, 1);
        Ok(fork)
    }

    fn track_dir(fork: &Fork) -> Option<PathBuf> {
        let l = fork.commit_to.as_deref().or(fork.from.as_ref().map(|f| f.0.as_str()))?;
        fork.set.lineage_dir(l).ok()
    }

    fn track(&self, fork: &Fork, add: i32) {
        let Some(d) = Self::track_dir(fork) else { return };
        let mut g = self.inner.lock().unwrap();
        let n = g.forks.entry(d.clone()).or_default();
        *n = n.saturating_add_signed(add);
        if *n == 0 {
            g.forks.remove(&d);
        }
    }

    /// Re-register a fork found in node-only state after a restart (an
    /// adopted job still runs on it).
    pub fn adopt(&self, fork: &Fork) {
        self.track(fork, 1);
    }

    /// The job ended: commit its fork as the lineage's next generation if
    /// every rule holds (§4.1.4), else discard it. Returns the result and
    /// the folders to delete (outside any lock, in the background).
    pub fn finish(&self, fork: Fork, end: &Ended) -> (k2_node_proto::gens::CacheResult, Vec<PathBuf>) {
        use k2_node_proto::gens::{CacheResult, LINEAGE_TRUSTED};
        self.track(&fork, -1);
        let mut res = CacheResult {
            lineage: fork.from.as_ref().map(|f| f.0.clone()),
            seq: fork.from.as_ref().map(|f| f.1),
            commit: fork.from.as_ref().map(|f| f.2.commit.clone()),
            fork_ms: fork.fork_ms,
            committed: false,
            commit_lineage: None,
            why: String::new(),
        };
        let discard = |mut res: CacheResult, why: String| {
            res.why = why;
            (res, vec![fork.dir.clone()])
        };
        let Some(lineage) = fork.commit_to.clone() else {
            let why = fork.no_commit.clone().unwrap_or_else(|| "saves nothing".into());
            return discard(res, why);
        };
        let trusted = lineage == LINEAGE_TRUSTED;
        if end.secret {
            return discard(res, "a secret job saves nothing".into());
        }
        if trusted {
            if !end.ok {
                return discard(res, format!("not saved: {}", end.end));
            }
            if end.dirty || end.with_repos {
                return discard(res, "not saved: --dirty or --with never saves the trusted copy".into());
            }
            match &end.on_branch {
                None | Some(Ok(true)) => {}
                Some(Ok(false)) => return discard(res, "not saved: the node's own fetch doesn't have this commit on the trusted branch".into()),
                Some(Err(e)) => return discard(res, format!("not saved: can't check the trusted branch after the node's fetch ({e})")),
            }
        } else if end.cancelled {
            return discard(res, "not saved: cancelled".into());
        }
        if !is_commit(&end.commit) {
            return discard(res, "not saved: the job has no commit".into());
        }
        let Ok(ldir) = fork.set.lineage_dir(&lineage) else {
            return discard(res, format!("not saved: {lineage:?} is not a lineage"));
        };
        // Measured before the lock: a walk of a big target takes a while.
        let bytes = crate::node::disk_usage(&fork.dir);
        let _g = self.commit.lock().unwrap();
        let head = match read_head(&ldir) {
            Ok(h) => h,
            Err(e) => return discard(res, format!("not saved: {e}")),
        };
        if let Some(Head { gen: Some(g), .. }) = &head {
            if trusted && g.commit != end.commit {
                match (end.is_ancestor)(&g.commit, &end.commit) {
                    Ok(true) => {}
                    Ok(false) => {
                        crate::nlog!("cache generations: {} from {} is older than the head ({}); not saved", fork.dir.display(), end.commit, g.commit);
                        return discard(res, format!("not saved: older than the head ({})", &g.commit[..12.min(g.commit.len())]));
                    }
                    Err(e) => return discard(res, format!("not saved: can't tell whether it is newer than the head ({e})")),
                }
            }
        }
        let seq = head.as_ref().map(|h| h.seq).unwrap_or(0) + 1;
        let parent_digest = fork.from.as_ref().map(|f| f.2.gen_digest.clone());
        let gen_digest = match k2_node_proto::ci::gen_digest(&lineage, seq, parent_digest.as_deref(), &end.producer.receipt_digest) {
            Ok(d) => d,
            Err(e) => return discard(res, format!("not saved: {e}")),
        };
        if let Err(e) = std::fs::create_dir_all(&ldir) {
            return discard(res, format!("not saved: {e}"));
        }
        let dst = gen_dir(&ldir, seq);
        let _ = remove_tree(&dst);
        if let Err(e) = std::fs::rename(&fork.dir, &dst) {
            return discard(res, format!("not saved: moving the fork in: {e}"));
        }
        let rec = GenRecord {
            commit: end.commit.clone(),
            gen_digest,
            parent_digest,
            producer: end.producer.clone(),
            ref_checked: if trusted {
                match end.on_branch {
                    Some(Ok(true)) => Some(k2_node_proto::ci::RefChecked::Node),
                    _ => end.ref_checked,
                }
            } else {
                None
            },
            base_trusted: if trusted { seq } else { fork.base_trusted },
            at: end.now,
            bytes,
        };
        if let Err(e) = write_head(&ldir, &Head { lineage: lineage.clone(), seq, gen: Some(rec), last_used: end.now }) {
            // The folder is in but no head names it: it goes, the old head stays.
            res.why = format!("not saved: {e}");
            return (res, vec![dst]);
        }
        res.committed = true;
        res.commit_lineage = Some(lineage.clone());
        res.why = format!("saved as {lineage} #{seq}");
        let old = self.prune(&ldir, seq);
        (res, old)
    }

    /// The generations of a lineage below the newest
    /// [`k2_node_proto::gens::KEEP_GENERATIONS`] that no clone is reading,
    /// renamed aside (`.del-g<n>`) for deletion.
    fn prune(&self, ldir: &Path, head: u64) -> Vec<PathBuf> {
        let keep = k2_node_proto::gens::KEEP_GENERATIONS as u64;
        let cloning = self.inner.lock().unwrap().cloning.clone();
        let mut out = Vec::new();
        for (seq, p) in gen_dirs(ldir) {
            if seq + keep <= head && !cloning.contains_key(&p) {
                let aside = ldir.join(format!(".del-g{seq}"));
                if std::fs::rename(&p, &aside).is_ok() {
                    out.push(aside);
                }
            }
        }
        out
    }

    /// Forks running from or into `lineage_dir` now.
    pub fn forks_of(&self, lineage_dir: &Path) -> u32 {
        self.inner.lock().unwrap().forks.get(lineage_dir).copied().unwrap_or(0)
    }

    /// The `lineage_status` frame for one server's root (protocol 10): every
    /// lineage with a head, scratch capped at
    /// [`k2_node_proto::gens::MAX_SCRATCH_LINEAGES`] per project (newest
    /// first), at most [`k2_node_proto::gens::MAX_STATUS_LINEAGES`].
    pub fn status(&self, root: &Path, now: i64) -> k2_node_proto::gens::LineageStatus {
        use k2_node_proto::gens::{LineageInfo, LineageStatus, LINEAGE_SCRATCH, MAX_SCRATCH_LINEAGES, MAX_STATUS_LINEAGES};
        let mut all = lineages_in(root);
        all.sort_by(|a, b| (&a.set.key, b.head.last_used).cmp(&(&b.set.key, a.head.last_used)));
        let mut out = Vec::new();
        let mut scratch: std::collections::HashMap<String, usize> = Default::default();
        let mut truncated = false;
        for e in all {
            let Some(g) = &e.head.gen else { continue };
            if e.head.lineage.starts_with(LINEAGE_SCRATCH) {
                let n = scratch.entry(e.set.key.clone()).or_default();
                *n += 1;
                if *n > MAX_SCRATCH_LINEAGES {
                    truncated = true;
                    continue;
                }
            }
            if out.len() >= MAX_STATUS_LINEAGES {
                truncated = true;
                break;
            }
            out.push(LineageInfo {
                key: e.set.key.clone(),
                name: e.set.name.clone(),
                lineage: e.head.lineage.clone(),
                head_seq: e.head.seq,
                head_commit: g.commit.clone(),
                bytes: g.bytes,
                last_used: e.head.last_used,
                forks: self.forks_of(&e.dir),
            });
        }
        LineageStatus { at: now, lineages: out, truncated }
    }

    /// Keep one server's generations within `p` (§4.1.3): leftovers of
    /// earlier deletes go; every lineage keeps its head and the one before
    /// it; a scratch lineage idle [`k2_node_proto::gens::SCRATCH_IDLE_SECS`]
    /// goes; a lineage whose head is over `lineage_max_bytes` (or, copying,
    /// over [`k2_node_proto::gens::COPY_LINEAGE_MAX_BYTES`]) resets (its
    /// next job builds cold); over the budget (or under disk pressure),
    /// scratch goes first, least recently used first, then the others
    /// reset. A lineage with a running fork is never touched. Returns what
    /// it did (one line each) and the folders to delete.
    pub fn evict(&self, root: &Path, p: &EvictPolicy) -> (Vec<String>, Vec<PathBuf>) {
        use k2_node_proto::gens::{CloneMethod, COPY_LINEAGE_MAX_BYTES, LINEAGE_SCRATCH, SCRATCH_IDLE_SECS};
        let _g = self.commit.lock().unwrap();
        let mut lines = Vec::new();
        let mut doomed = leftovers(root);
        let cap = if self.fs.clone == CloneMethod::Copy { p.lineage_max_bytes.min(COPY_LINEAGE_MAX_BYTES) } else { p.lineage_max_bytes };
        let mut live: Vec<(LineageEntry, u64)> = Vec::new();
        for e in lineages_in(root) {
            if self.forks_of(&e.dir) > 0 {
                continue;
            }
            let head_seq = e.head.seq;
            doomed.extend(self.prune(&e.dir, head_seq));
            let scratch = e.head.lineage.starts_with(LINEAGE_SCRATCH);
            if scratch && p.now - e.head.last_used > SCRATCH_IDLE_SECS {
                lines.push(format!("{} of {}: unused for 3 days, removed", e.head.lineage, e.set.key));
                doomed.extend(aside(&e.dir));
                continue;
            }
            let bytes = e.head.gen.as_ref().map(|g| g.bytes).unwrap_or(0);
            if bytes > cap {
                lines.push(format!("{} of {}: head is {} GB, over {} GB, reset (its next job builds cold)", e.head.lineage, e.set.key, bytes >> 30, cap >> 30));
                doomed.extend(self.reset(&e));
                continue;
            }
            let kept = gen_dirs(&e.dir).len() as u64;
            live.push((e, bytes.saturating_mul(kept.max(1))));
        }
        let mut total: u64 = live.iter().map(|(_, b)| *b).sum();
        // Scratch first, least recently used first; then the rest the same way.
        live.sort_by_key(|(e, _)| (!e.head.lineage.starts_with(LINEAGE_SCRATCH), e.head.last_used));
        for (e, b) in live {
            let scratch = e.head.lineage.starts_with(LINEAGE_SCRATCH);
            let over = total > p.budget_bytes;
            if !(over || (p.pressure && scratch)) {
                continue;
            }
            if scratch {
                lines.push(format!("{} of {}: removed ({})", e.head.lineage, e.set.key, if over { "over the cache budget" } else { "disk low" }));
                doomed.extend(aside(&e.dir));
            } else if e.head.gen.is_some() {
                lines.push(format!("{} of {}: reset, over the cache budget (its next job builds cold)", e.head.lineage, e.set.key));
                doomed.extend(self.reset(&e));
            } else {
                continue;
            }
            total = total.saturating_sub(b);
        }
        (lines, doomed)
    }

    /// Reset a lineage: every generation aside, the head says none (the
    /// seq keeps counting). Under the commit lock.
    fn reset(&self, e: &LineageEntry) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (seq, p) in gen_dirs(&e.dir) {
            let a = e.dir.join(format!(".del-g{seq}"));
            if std::fs::rename(&p, &a).is_ok() {
                out.push(a);
            }
        }
        let _ = write_head(&e.dir, &Head { gen: None, ..e.head.clone() });
        out
    }
}

/// [`Gens::evict`]'s limits.
#[derive(Debug, Clone, Copy)]
pub struct EvictPolicy {
    pub now: i64,
    /// All of this server's generations together.
    pub budget_bytes: u64,
    /// One lineage's head.
    pub lineage_max_bytes: u64,
    /// The disk is low: scratch lineages go whatever the budget says.
    pub pressure: bool,
}

/// One lineage on disk.
#[derive(Debug, Clone)]
pub struct LineageEntry {
    pub set: SetId,
    pub dir: PathBuf,
    pub head: Head,
}

/// Every lineage with a readable head under one server's root.
pub fn lineages_in(root: &Path) -> Vec<LineageEntry> {
    let mut out = Vec::new();
    let g = gens_root(root);
    let Ok(keys) = std::fs::read_dir(&g) else { return out };
    for k in keys.flatten() {
        let key = k.file_name().to_string_lossy().into_owned();
        if !k2_node_proto::frames::valid_project_key(&key) {
            continue;
        }
        let Ok(set) = SetId::target(root, &key) else { continue };
        let Ok(ls) = std::fs::read_dir(set.dir()) else { continue };
        for l in ls.flatten() {
            if l.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if let Ok(Some(head)) = read_head(&l.path()) {
                // The folder must be the one its head's name maps to.
                if set.lineage_dir(&head.lineage).ok().as_deref() == Some(l.path().as_path()) {
                    out.push(LineageEntry { set: set.clone(), dir: l.path(), head });
                }
            }
        }
    }
    out
}

/// Leftovers of an earlier delete (a crash mid-delete): `.del-*` under any
/// set or lineage folder, and the probe's scratch folder.
fn leftovers(root: &Path) -> Vec<PathBuf> {
    let g = gens_root(root);
    let mut out = Vec::new();
    if g.join(PROBE_DIR).exists() {
        out.push(g.join(PROBE_DIR));
    }
    let dels = |d: &Path, out: &mut Vec<PathBuf>| {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(".del-") {
                    out.push(e.path());
                }
            }
        }
    };
    let Ok(keys) = std::fs::read_dir(&g) else { return out };
    for k in keys.flatten() {
        let Ok(sets) = std::fs::read_dir(k.path()) else { continue };
        for s in sets.flatten() {
            dels(&s.path(), &mut out);
            if let Ok(ls) = std::fs::read_dir(s.path()) {
                for l in ls.flatten() {
                    dels(&l.path(), &mut out);
                }
            }
        }
    }
    out
}

/// A whole lineage folder renamed aside in its set folder, for deletion.
fn aside(dir: &Path) -> Vec<PathBuf> {
    let Some(parent) = dir.parent() else { return vec![] };
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let a = parent.join(format!(".del-{name}-{}", std::process::id()));
    let a = if a.exists() { parent.join(format!(".del-{name}-{}-{}", std::process::id(), crate::util::now())) } else { a };
    match std::fs::rename(dir, &a) {
        Ok(()) => vec![a],
        Err(_) => vec![],
    }
}

/// The `g<seq>` folders of a lineage.
fn gen_dirs(ldir: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(ldir) else { return vec![] };
    let mut v: Vec<(u64, PathBuf)> =
        rd.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix('g')).and_then(|n| n.parse().ok()).map(|s| (s, e.path()))).collect();
    v.sort();
    v
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
        std::fs::set_permissions(&g, std::fs::Permissions::from_mode(0o770)).unwrap();
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
        assert!(acl::has_any(&g).unwrap());
        // A named entry also shows as the mask in the group bits: refused
        // whichever check names it first.
        let e = is_closed(&g).unwrap_err();
        assert!(e.contains("ACL") || e.contains("mode 770"), "{e}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_extended_acl_on_the_gens_root_is_stripped_and_refused() {
        let d = crate::util::temp_dir("gens-acl");
        let g = gens_root(&d);
        std::fs::create_dir(&g).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&g, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let ok = std::process::Command::new("/bin/chmod").args(["+a", "everyone allow list,search"]).arg(&g).status().unwrap();
        assert!(ok.success(), "chmod +a on a temp dir");
        assert!(acl::has_any(&g).unwrap());
        assert!(is_closed(&g).unwrap_err().contains("ACL"));
        ensure_closed(&g).unwrap();
        assert!(!acl::has_any(&g).unwrap());
    }
}

/// The clone backends and lineages on this machine's real filesystem
/// (prd-compute-blacksmith-class-v1 §10, B1 at the node: the PR-poison,
/// failed-check, older-check, two-at-once and secret rules).
#[cfg(test)]
mod lineage_tests {
    use super::*;
    use k2_node_proto::ci::CacheProducer;
    use k2_node_proto::gens::{CacheCommit, CloneMethod};

    type Anc = dyn Fn(&str, &str) -> Result<bool, String> + Sync;

    fn sha(c: char) -> String {
        c.to_string().repeat(40)
    }

    fn setup(tag: &str) -> (PathBuf, Gens, SetId) {
        let root = crate::util::temp_dir(tag);
        let g = gens_root(&root);
        ensure_closed(&g).unwrap();
        std::fs::create_dir_all(forks_root(&root)).unwrap();
        let fs = probe(&g).unwrap();
        eprintln!("cache_fs here: {fs:?}");
        let set = SetId::target(&root, "0123456789abcdef").unwrap();
        (root, Gens::new(fs), set)
    }

    fn fork(g: &Gens, set: &SetId, commit: CacheCommit, r: Option<&str>, name: &str) -> Fork {
        let dst = forks_root(&set.root).join(name);
        g.fork(&ForkAsk { set, commit, cache_ref: r, dst: &dst, now: 100 }).unwrap()
    }

    fn producer(job: &str, commit: &str) -> CacheProducer {
        CacheProducer { job_id: job.into(), attempt: 1, receipt_digest: format!("sha256:{}", "e".repeat(64)), commit: commit.into() }
    }

    fn ok_end<'a>(commit: &str, anc: &'a Anc) -> Ended<'a> {
        Ended {
            ok: true,
            cancelled: false,
            end: "done".into(),
            commit: commit.into(),
            dirty: false,
            with_repos: false,
            secret: false,
            producer: producer("j1", commit),
            ref_checked: Some(k2_node_proto::ci::RefChecked::Controller),
            on_branch: None,
            is_ancestor: anc,
            now: 200,
        }
    }

    fn yes(_: &str, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn no(_: &str, _: &str) -> Result<bool, String> {
        Ok(false)
    }

    fn done(g: &Gens, f: Fork, e: &Ended) -> k2_node_proto::gens::CacheResult {
        let (r, del) = g.finish(f, e);
        for d in del {
            remove_tree(&d).unwrap();
        }
        r
    }

    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// Every method this machine has (the probed one and the copy
    /// fallback): the clone is equal, keeps links as links and modes, and
    /// neither side's later writes reach the other.
    #[test]
    fn clones_are_equal_and_independent() {
        use std::os::unix::fs::PermissionsExt;
        let (root, g, _) = setup("gens-clone");
        let gens = gens_root(&root);
        let mut methods = vec![CloneMethod::Copy, g.fs.clone];
        methods.dedup();
        for (i, m) in methods.into_iter().enumerate() {
            let src = gens.join(format!("src{i}"));
            new_set_dir(m, &src).unwrap();
            put(&src.join("target/debug/app"), "v1");
            std::fs::set_permissions(src.join("target/debug/app"), std::fs::Permissions::from_mode(0o755)).unwrap();
            put(&src.join("cargo-home/registry/src/x/lib.rs"), "pub fn x() {}");
            std::os::unix::fs::symlink("/etc/passwd", src.join("target/link")).unwrap();
            let dst = gens.join(format!("dst{i}"));
            clone_tree(m, &src, &dst).unwrap_or_else(|e| panic!("{m:?}: {e}"));
            assert_eq!(std::fs::read_to_string(dst.join("target/debug/app")).unwrap(), "v1", "{m:?}");
            assert_eq!(std::fs::metadata(dst.join("target/debug/app")).unwrap().permissions().mode() & 0o777, 0o755, "{m:?}: mode kept");
            assert_eq!(std::fs::read_link(dst.join("target/link")).unwrap(), Path::new("/etc/passwd"), "{m:?}: a link stays a link");
            std::fs::write(dst.join("target/debug/app"), "poison").unwrap();
            std::fs::write(dst.join("cargo-home/registry/src/x/lib.rs"), "poison").unwrap();
            assert_eq!(std::fs::read_to_string(src.join("target/debug/app")).unwrap(), "v1", "{m:?}: the clone's write never reaches its source");
            assert_eq!(std::fs::read_to_string(src.join("cargo-home/registry/src/x/lib.rs")).unwrap(), "pub fn x() {}");
            std::fs::write(src.join("target/debug/app"), "v2").unwrap();
            assert_eq!(std::fs::read_to_string(dst.join("target/debug/app")).unwrap(), "poison", "{m:?}: and the other way");
            assert!(clone_tree(m, &src, &dst).unwrap_err().contains("exists"), "never over an existing folder");
        }
        assert!(!gens.join(PROBE_DIR).exists(), "the probe cleans up");
    }

    /// The probe names what it found and only a method that worked here.
    #[test]
    fn the_probe_names_the_filesystem() {
        let (_root, g, _) = setup("gens-probe");
        assert!(!g.fs.fs.is_empty());
        if cfg!(target_os = "macos") && g.fs.fs == "apfs" {
            assert_eq!(g.fs.clone, CloneMethod::Clonefile);
        }
        if g.fs.fs == "ext4" || g.fs.fs == "tmpfs" {
            assert_eq!(g.fs.clone, CloneMethod::Copy, "no clone on {}", g.fs.fs);
        }
        let open = crate::util::temp_dir("gens-probe-open");
        assert!(probe(&open).is_err(), "never probes a folder that isn't closed");
    }

    /// §10: a PR job writes `target/poison` and edits an extracted crate in
    /// `CARGO_HOME`; the next trusted check on the node sees neither. A
    /// later run of the same PR does (its own scratch lineage). A plain
    /// run at `main` saves to `scratch/main`, never `trusted`.
    #[test]
    fn a_pr_jobs_writes_never_reach_a_trusted_check() {
        let (_root, g, set) = setup("gens-poison");
        let t1 = fork(&g, &set, CacheCommit::Trusted, None, "t1");
        assert!(t1.from.is_none(), "nothing to fork yet");
        put(&t1.target().join("debug/app"), "good");
        put(&t1.dir.join("cargo-home/registry/src/x/lib.rs"), "pub fn x() {}");
        let r = done(&g, t1, &ok_end(&sha('a'), &yes));
        assert!(r.committed && r.commit_lineage.as_deref() == Some("trusted"), "{r:?}");
        let pr = fork(&g, &set, CacheCommit::Scratch, Some("pr-1"), "pr");
        assert_eq!(pr.from.as_ref().map(|f| (f.0.as_str(), f.1)), Some(("trusted", 1)));
        put(&pr.target().join("poison"), "planted");
        std::fs::write(pr.dir.join("cargo-home/registry/src/x/lib.rs"), "planted").unwrap();
        let r = done(&g, pr, &ok_end(&sha('b'), &yes));
        assert_eq!(r.commit_lineage.as_deref(), Some("scratch/pr-1"), "{r:?}");
        let t2 = fork(&g, &set, CacheCommit::Trusted, None, "t2");
        assert!(!t2.target().join("poison").exists(), "the PR's file never reaches a trusted check");
        assert_eq!(std::fs::read_to_string(t2.dir.join("cargo-home/registry/src/x/lib.rs")).unwrap(), "pub fn x() {}", "nor its CARGO_HOME edit");
        assert_eq!(std::fs::read_to_string(t2.target().join("debug/app")).unwrap(), "good");
        let pr2 = fork(&g, &set, CacheCommit::Scratch, Some("pr-1"), "pr2");
        assert_eq!(pr2.from.as_ref().map(|f| f.0.as_str()), Some("scratch/pr-1"), "the same PR forks its own scratch head");
        assert!(pr2.target().join("poison").exists());
        let other = fork(&g, &set, CacheCommit::Scratch, Some("pr-2"), "other");
        assert!(!other.target().join("poison").exists(), "another branch never sees it");
        let main = fork(&g, &set, CacheCommit::Scratch, Some("main"), "main");
        let r = done(&g, main, &ok_end(&sha('c'), &yes));
        assert_eq!(r.commit_lineage.as_deref(), Some("scratch/main"), "a plain run at main: {r:?}");
        let h = read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap();
        assert_eq!(h.seq, 1, "trusted unchanged by anything but a trusted check");
        for f in [t2, pr2, other] {
            done(&g, f, &Ended { cancelled: true, ok: false, ..ok_end(&sha('d'), &yes) });
        }
    }

    /// §10: a trusted check that fails commits nothing (the head stays),
    /// and its fork goes. Scratch keeps a failed build (helps the retry),
    /// never a cancelled one.
    #[test]
    fn a_failed_trusted_check_commits_nothing() {
        let (_root, g, set) = setup("gens-fail");
        let t = fork(&g, &set, CacheCommit::Trusted, None, "t");
        done(&g, t, &ok_end(&sha('a'), &yes));
        let bad = fork(&g, &set, CacheCommit::Trusted, None, "bad");
        put(&bad.target().join("half"), "x");
        let dir = bad.dir.clone();
        let r = done(&g, bad, &Ended { ok: false, end: "exit 101".into(), ..ok_end(&sha('b'), &yes) });
        assert!(!r.committed && r.why.contains("exit 101"), "{r:?}");
        assert!(!dir.exists(), "the fork is deleted");
        assert_eq!(read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().seq, 1);
        let s = fork(&g, &set, CacheCommit::Scratch, Some("feat"), "s");
        assert!(done(&g, s, &Ended { ok: false, end: "exit 101".into(), ..ok_end(&sha('b'), &yes) }).committed, "scratch keeps a failed build");
        let s = fork(&g, &set, CacheCommit::Scratch, Some("feat"), "s2");
        let r = done(&g, s, &Ended { ok: false, cancelled: true, end: "cancelled".into(), ..ok_end(&sha('b'), &yes) });
        assert!(!r.committed, "never a cancelled one: {r:?}");
        let d = fork(&g, &set, CacheCommit::Trusted, None, "dirty");
        assert!(!done(&g, d, &Ended { dirty: true, ..ok_end(&sha('b'), &yes) }).committed, "--dirty never saves trusted");
    }

    /// §10, rule 4: a trusted check from an older commit that ends after a
    /// newer one is discarded; the same commit again is fine; an ancestry
    /// the node can't tell never commits.
    #[test]
    fn an_older_trusted_check_is_discarded() {
        let (_root, g, set) = setup("gens-older");
        let slow = fork(&g, &set, CacheCommit::Trusted, None, "slow");
        let fast = fork(&g, &set, CacheCommit::Trusted, None, "fast");
        assert!(done(&g, fast, &ok_end(&sha('b'), &yes)).committed);
        let r = done(&g, slow, &ok_end(&sha('a'), &no));
        assert!(!r.committed && r.why.contains("older than the head"), "{r:?}");
        let again = fork(&g, &set, CacheCommit::Trusted, None, "again");
        assert!(done(&g, again, &ok_end(&sha('b'), &no)).committed, "the head's own commit is not older");
        let unknown = fork(&g, &set, CacheCommit::Trusted, None, "unk");
        let err = |_: &str, _: &str| -> Result<bool, String> { Err("no such commit in the mirror".into()) };
        let r = done(&g, unknown, &ok_end(&sha('c'), &err));
        assert!(!r.committed && r.why.contains("can't tell"), "{r:?}");
    }

    /// Rule 5: a node that fetched the remote itself checks the commit is on
    /// `origin/<branch>`; on it, the head says `ref_checked: node`; off it
    /// (or unknown), nothing is saved. Without its own fetch the
    /// controller's word stands (`controller`).
    #[test]
    fn rule_5_the_nodes_own_fetch_decides_ref_checked() {
        use k2_node_proto::ci::RefChecked;
        let (_root, g, set) = setup("gens-rule5");
        let rc = || read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().gen.unwrap().ref_checked;
        done(&g, fork(&g, &set, CacheCommit::Trusted, Some("main"), "a"), &ok_end(&sha('a'), &yes));
        assert_eq!(rc(), Some(RefChecked::Controller));
        let r = done(&g, fork(&g, &set, CacheCommit::Trusted, Some("main"), "b"), &Ended { on_branch: Some(Ok(true)), ..ok_end(&sha('b'), &yes) });
        assert!(r.committed);
        assert_eq!(rc(), Some(RefChecked::Node));
        let r = done(&g, fork(&g, &set, CacheCommit::Trusted, Some("main"), "c"), &Ended { on_branch: Some(Ok(false)), ..ok_end(&sha('c'), &yes) });
        assert!(!r.committed && r.why.contains("trusted branch"), "{r:?}");
        let r = done(&g, fork(&g, &set, CacheCommit::Trusted, Some("main"), "d"), &Ended { on_branch: Some(Err("no ref".into())), ..ok_end(&sha('d'), &yes) });
        assert!(!r.committed, "{r:?}");
        assert_eq!(read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().seq, 2);
    }

    /// §10: two jobs of one project fork at once and both start warm; the
    /// head and the one before it stay, older generations go.
    #[test]
    fn two_forks_at_once_and_two_generations_kept() {
        let (_root, g, set) = setup("gens-two");
        let t = fork(&g, &set, CacheCommit::Trusted, None, "t");
        put(&t.target().join("a"), "1");
        done(&g, t, &ok_end(&sha('a'), &yes));
        let a = fork(&g, &set, CacheCommit::Scratch, Some("x"), "a");
        let b = fork(&g, &set, CacheCommit::Scratch, Some("y"), "b");
        assert!(a.from.is_some() && b.from.is_some(), "both warm");
        assert_eq!(g.forks_of(&set.lineage_dir("scratch/x").unwrap()), 1);
        for (i, c) in ['b', 'c', 'd'].into_iter().enumerate() {
            let f = fork(&g, &set, CacheCommit::Trusted, None, &format!("n{i}"));
            assert!(done(&g, f, &ok_end(&sha(c), &yes)).committed);
        }
        let l = set.lineage_dir("trusted").unwrap();
        let left: Vec<u64> = gen_dirs(&l).into_iter().map(|(s, _)| s).collect();
        assert_eq!(left, vec![3, 4], "the head and the one before it");
        done(&g, a, &ok_end(&sha('e'), &yes));
        done(&g, b, &ok_end(&sha('e'), &yes));
        assert_eq!(g.forks_of(&set.lineage_dir("scratch/x").unwrap()), 0);
    }

    /// A scratch head is forked only while its base is as new as
    /// `trusted`'s head (§4.1.2 rule 3).
    #[test]
    fn a_scratch_head_older_than_trusted_is_passed_over() {
        let (_root, g, set) = setup("gens-base");
        done(&g, fork(&g, &set, CacheCommit::Trusted, None, "t1"), &ok_end(&sha('a'), &yes));
        let s = fork(&g, &set, CacheCommit::Scratch, Some("feat"), "s1");
        assert_eq!(s.base_trusted, 1);
        done(&g, s, &ok_end(&sha('b'), &yes));
        let s = fork(&g, &set, CacheCommit::Scratch, Some("feat"), "s2");
        assert_eq!(s.from.as_ref().map(|f| f.0.as_str()), Some("scratch/feat"));
        done(&g, s, &ok_end(&sha('b'), &yes));
        done(&g, fork(&g, &set, CacheCommit::Trusted, None, "t2"), &ok_end(&sha('c'), &yes));
        let s = fork(&g, &set, CacheCommit::Scratch, Some("feat"), "s3");
        assert_eq!(s.from.as_ref().map(|f| (f.0.as_str(), f.1)), Some(("trusted", 2)), "trusted moved past the scratch base");
    }

    /// A secret job (under its own uid) forks `trusted` and saves nothing;
    /// a release build keeps nothing yet; a loosened gens root forks
    /// nothing.
    #[test]
    fn secret_and_release_never_commit_and_an_open_root_refuses() {
        use std::os::unix::fs::PermissionsExt;
        let (root, g, set) = setup("gens-secret");
        done(&g, fork(&g, &set, CacheCommit::Trusted, None, "t"), &ok_end(&sha('a'), &yes));
        let s = fork(&g, &set, CacheCommit::None, None, "sec");
        assert_eq!(s.from.as_ref().map(|f| f.0.as_str()), Some("trusted"));
        let r = done(&g, s, &ok_end(&sha('b'), &yes));
        assert!(!r.committed && r.why.contains("secret"), "{r:?}");
        let rel = fork(&g, &set, CacheCommit::Release, Some("linux-x86_64"), "rel");
        assert!(rel.from.is_none() && rel.commit_to.is_none());
        let s = fork(&g, &set, CacheCommit::Scratch, Some("x"), "sx");
        assert!(!done(&g, s, &Ended { secret: true, ..ok_end(&sha('b'), &yes) }).committed, "a secret end never saves");
        assert_eq!(read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().seq, 1);
        std::fs::set_permissions(gens_root(&root), std::fs::Permissions::from_mode(0o750)).unwrap();
        let dst = forks_root(&root).join("open");
        let e = g.fork(&ForkAsk { set: &set, commit: CacheCommit::Trusted, cache_ref: None, dst: &dst, now: 1 }).unwrap_err();
        assert!(e.contains("mode 750"), "{e}");
        assert!(!dst.exists());
    }

    /// A measurement on a node, not a check of behaviour (ignored by
    /// default): fork, commit and discard times for a real cargo target.
    ///
    ///   K2_GENS_MEASURE=<a target dir> cargo test -p k2-node --lib \
    ///     gens::lineage_tests::measure -- --ignored --nocapture
    #[test]
    #[ignore]
    fn measure_fork_commit_discard_of_a_real_target() {
        let src = PathBuf::from(std::env::var("K2_GENS_MEASURE").or_else(|_| std::env::var("CARGO_TARGET_DIR")).expect("K2_GENS_MEASURE (or CARGO_TARGET_DIR) = a cargo target dir"));
        assert!(src.is_dir(), "{} is not a folder", src.display());
        let (root, g, _) = setup("gens-measure");
        let gens = gens_root(&root);
        let secs = |t: std::time::Instant| t.elapsed().as_secs_f64();
        let seed = gens.join("seed");
        let t = std::time::Instant::now();
        if g.fs.clone == CloneMethod::Subvolume {
            new_set_dir(CloneMethod::Subvolume, &seed).unwrap();
            let mut a = src.as_os_str().to_owned();
            a.push("/.");
            run_tool(&find_tool("cp").unwrap(), &["-a".as_ref(), "--reflink=auto".as_ref(), a.as_os_str(), seed.as_os_str()]).unwrap();
        } else {
            clone_tree(CloneMethod::Copy, &src, &seed).unwrap();
        }
        let seeded = secs(t);
        let bytes = crate::node::disk_usage(&seed);
        let files = walk_count(&seed);
        let mut line = format!("MEASURE fs={} method={:?} files={files} bytes={bytes} ({:.1} GB) seed_copy={seeded:.2}s", g.fs.fs, g.fs.clone, bytes as f64 / (1u64 << 30) as f64);
        for (i, m) in [g.fs.clone, CloneMethod::Copy].into_iter().enumerate() {
            if i == 1 && m == g.fs.clone {
                break;
            }
            let f = gens.join(format!("fork{i}"));
            let t = std::time::Instant::now();
            clone_tree(m, &seed, &f).unwrap();
            let fork = secs(t);
            assert_eq!(walk_count(&f), files, "{m:?}: the fork has every entry");
            let gdir = gens.join(format!("g{i}"));
            let t = std::time::Instant::now();
            std::fs::rename(&f, &gdir).unwrap();
            let commit = secs(t);
            let t = std::time::Instant::now();
            let usage = crate::node::disk_usage(&gdir);
            let measure = secs(t);
            let t = std::time::Instant::now();
            remove_tree(&gdir).unwrap();
            let discard = secs(t);
            line.push_str(&format!(" | {m:?}: fork={fork:.2}s commit(rename)={commit:.4}s measure(du {usage})={measure:.2}s discard={discard:.2}s"));
        }
        eprintln!("{line}");
    }

    fn walk_count(p: &Path) -> u64 {
        let mut n = 0;
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            n += 1;
            if std::fs::symlink_metadata(&d).is_ok_and(|m| m.is_dir()) {
                if let Ok(rd) = std::fs::read_dir(&d) {
                    stack.extend(rd.flatten().map(|e| e.path()));
                }
            }
        }
        n
    }

    fn policy(now: i64) -> EvictPolicy {
        EvictPolicy { now, budget_bytes: u64::MAX, lineage_max_bytes: u64::MAX, pressure: false }
    }

    fn evict(g: &Gens, root: &Path, p: &EvictPolicy) -> Vec<String> {
        let (lines, doomed) = g.evict(root, p);
        for d in doomed {
            remove_tree(&d).unwrap();
        }
        lines
    }

    /// §4.1.3 eviction: an idle scratch lineage goes after 3 days, never
    /// one with a running fork; trusted stays; leftovers of a crash go.
    #[test]
    fn idle_scratch_goes_and_running_forks_are_never_touched() {
        let (root, g, set) = setup("gens-evict-idle");
        done(&g, fork(&g, &set, CacheCommit::Trusted, None, "t"), &ok_end(&sha('a'), &yes));
        for r in ["old", "busy"] {
            done(&g, fork(&g, &set, CacheCommit::Scratch, Some(r), r), &ok_end(&sha('b'), &yes));
        }
        let running = fork(&g, &set, CacheCommit::Scratch, Some("busy"), "running");
        let crash = set.dir().join(".del-crashed");
        std::fs::create_dir_all(crash.join("x")).unwrap();
        let later = 200 + k2_node_proto::gens::SCRATCH_IDLE_SECS + 1;
        let lines = evict(&g, &root, &policy(later));
        assert!(lines.iter().any(|l| l.contains("scratch/old") && l.contains("3 days")), "{lines:?}");
        assert!(!set.lineage_dir("scratch/old").unwrap().exists());
        assert!(set.lineage_dir("scratch/busy").unwrap().join(HEAD_FILE).exists(), "a lineage with a running fork stays");
        assert!(set.lineage_dir("trusted").unwrap().join(HEAD_FILE).exists());
        assert!(!crash.exists(), "a crashed delete's leftover goes");
        done(&g, running, &ok_end(&sha('c'), &yes));
    }

    /// A trusted head over the lineage cap resets: the next trusted check
    /// builds cold and starts a new chain (no parent), the seq keeps
    /// counting. Over the budget, scratch goes before trusted resets.
    #[test]
    fn over_the_caps_scratch_goes_first_and_trusted_resets() {
        let (root, g, set) = setup("gens-evict-cap");
        let t = fork(&g, &set, CacheCommit::Trusted, None, "t");
        put(&t.target().join("big"), &"x".repeat(64 << 10));
        done(&g, t, &ok_end(&sha('a'), &yes));
        done(&g, fork(&g, &set, CacheCommit::Scratch, Some("pr"), "pr"), &ok_end(&sha('b'), &yes));
        let trusted_bytes = read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().gen.unwrap().bytes;
        assert!(trusted_bytes >= 64 << 10, "{trusted_bytes}");
        // Budget fits trusted alone: only scratch goes.
        let lines = evict(&g, &root, &EvictPolicy { budget_bytes: trusted_bytes * 3 / 2, ..policy(300) });
        assert!(lines.iter().any(|l| l.contains("scratch/pr") && l.contains("budget")), "{lines:?}");
        assert!(read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().gen.is_some(), "trusted kept: {lines:?}");
        // A head over the lineage cap resets.
        let lines = evict(&g, &root, &EvictPolicy { lineage_max_bytes: 1024, ..policy(300) });
        assert!(lines.iter().any(|l| l.starts_with("trusted") && l.contains("reset")), "{lines:?}");
        let h = read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap();
        assert!(h.gen.is_none() && h.seq == 1);
        assert!(gen_dirs(&set.lineage_dir("trusted").unwrap()).is_empty());
        let t = fork(&g, &set, CacheCommit::Trusted, None, "t2");
        assert!(t.from.is_none(), "after a reset the next trusted check builds cold");
        done(&g, t, &ok_end(&sha('c'), &yes));
        let h = read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap();
        assert_eq!(h.seq, 2, "the seq keeps counting");
        assert_eq!(h.gen.unwrap().parent_digest, None, "a new chain");
    }

    /// Disk pressure: every scratch lineage without a running fork goes,
    /// whatever the budget.
    #[test]
    fn disk_pressure_drops_scratch() {
        let (root, g, set) = setup("gens-evict-pressure");
        done(&g, fork(&g, &set, CacheCommit::Trusted, None, "t"), &ok_end(&sha('a'), &yes));
        done(&g, fork(&g, &set, CacheCommit::Scratch, Some("a"), "a"), &ok_end(&sha('b'), &yes));
        let lines = evict(&g, &root, &EvictPolicy { pressure: true, ..policy(300) });
        assert!(lines.iter().any(|l| l.contains("disk low")), "{lines:?}");
        assert!(!set.lineage_dir("scratch/a").unwrap().exists());
        assert!(read_head(&set.lineage_dir("trusted").unwrap()).unwrap().unwrap().gen.is_some());
    }

    /// The `lineage_status` frame: heads only, scratch capped per project
    /// (newest first), and it checks.
    #[test]
    fn status_lists_heads_and_caps_scratch() {
        let (root, g, set) = setup("gens-status");
        done(&g, fork(&g, &set, CacheCommit::Trusted, None, "t"), &ok_end(&sha('a'), &yes));
        let n = k2_node_proto::gens::MAX_SCRATCH_LINEAGES + 2;
        for i in 0..n {
            let f = fork(&g, &set, CacheCommit::Scratch, Some(&format!("b{i}")), &format!("s{i}"));
            done(&g, f, &Ended { now: 1000 + i as i64, ..ok_end(&sha('b'), &yes) });
        }
        let s = g.status(&root, 5000);
        s.check().unwrap();
        assert!(s.truncated);
        let scratch: Vec<&str> = s.lineages.iter().filter(|l| l.lineage.starts_with("scratch/")).map(|l| l.lineage.as_str()).collect();
        assert_eq!(scratch.len(), k2_node_proto::gens::MAX_SCRATCH_LINEAGES);
        assert!(scratch.contains(&format!("scratch/b{}", n - 1).as_str()) && !scratch.contains(&"scratch/b0"), "newest first: {scratch:?}");
        let t = s.lineages.iter().find(|l| l.lineage == "trusted").unwrap();
        assert_eq!((t.head_seq, t.head_commit.as_str()), (1, sha('a').as_str()));
    }
}
