//! The job wall (0.46.3, slice N3 of `prd-compute-shared-queue-v1` §7):
//! what keeps a job away from the node's key and state, from the people
//! on the machine, and, under `network = "none"`, from the network.
//!
//! - **Linux** ([`Wall::JobUser`]): the root executor starts each job as
//!   the `k2job` user in a transient systemd unit (`admin::jobs`). The
//!   separate uid is the boundary (the node's `state/` is 0700 to the node
//!   user); the unit adds the namespaces on top: `ProtectHome=yes`, the
//!   node's `state/` + `run/`, `/etc/k2-node` and the executor's state
//!   inaccessible, the filesystem read-only except the job's own dirs,
//!   `PrivateNetwork=yes` for `network = "none"`, and `MemoryMax` (an OOM
//!   kill is reported as `mem_cap`).
//! - **macOS** ([`Wall::Seatbelt`]): the job runs under `sandbox-exec`
//!   with a deny profile ([`seatbelt_profile`]): the node's `state/` and
//!   `run/`, `/etc/k2-node`, the executor's state and socket, `/Users`,
//!   the keychains; with `network = "none"` every IP connection, bind and
//!   listen beyond loopback, and DNS. It is the same uid as the node, so
//!   it's a file and network wall, not a uid wall: a `_k2job` user needs
//!   the executor to own job processes on macOS (later work, Q5).
//! - **None**: setups without the wall (an old install that never ran
//!   the 0.46.3 setup, `--dev`). Jobs run as the node user as before;
//!   health says so and `network = "none"` takes no jobs.
//!
//! Also here: the `job-exec` shim the Linux unit runs, the race-free
//! tree walkers ([`share_with_jobs`], [`reclaim`]), the home-folder check
//! ([`open_homes_in`], [`make_private`]) and [`probe`] (what a process
//! can read, list and reach; the wall tests use it).

use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use k2_node_proto::health::{WALL_JOB_USER, WALL_NONE, WALL_SEATBELT};
use serde::{Deserialize, Serialize};

/// How this node walls its jobs (decided at start; see the module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wall {
    /// No wall: jobs run as the node user. `why` says what's missing.
    None { why: String },
    /// macOS: `sandbox-exec` with [`seatbelt_profile`].
    Seatbelt,
    /// Linux: the executor runs the job as `user` in a transient unit.
    JobUser { user: String },
}

impl Wall {
    pub fn label(&self) -> &'static str {
        match self {
            Wall::None { .. } => WALL_NONE,
            Wall::Seatbelt => WALL_SEATBELT,
            Wall::JobUser { .. } => WALL_JOB_USER,
        }
    }

    /// The wall can keep a job off the network (`network = "none"`).
    pub fn network_none(&self) -> bool {
        !matches!(self, Wall::None { .. })
    }

    pub fn is_walled(&self) -> bool {
        !matches!(self, Wall::None { .. })
    }
}

/// macOS's sandbox front end (present through macOS 26/27; deprecated in
/// name only, used by Apple's own tools).
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Is Seatbelt usable here?
pub fn seatbelt_available() -> bool {
    cfg!(target_os = "macos") && is_executable(Path::new(SANDBOX_EXEC))
}

/// Can this machine wall jobs off the network? macOS: Seatbelt. Linux:
/// the job user set up (`/etc/k2-node/jobs.toml` naming a real user). The
/// executor asks before it writes `network = "none"`.
pub fn network_none_supported(config_dir: &Path) -> bool {
    if cfg!(target_os = "macos") {
        return seatbelt_available();
    }
    matches!(crate::admin::jobs::read_jobs_file(config_dir), Ok(Some(f)) if crate::admin::server::user_ids(&f.job_user).is_some())
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// The path Seatbelt matches: canonical when it exists; otherwise macOS's
/// `/var`, `/etc`, `/tmp` → `/private/…` (Seatbelt sees real paths only).
pub fn real_path(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    if cfg!(target_os = "macos") {
        for top in ["/var", "/etc", "/tmp"] {
            if let Ok(rest) = p.strip_prefix(top) {
                return Path::new("/private").join(top.trim_start_matches('/')).join(rest);
            }
        }
    }
    p.to_path_buf()
}

/// A Seatbelt string literal (backslash and quote escaped; no control
/// characters, which a path never needs).
fn sb_str(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The Seatbelt profile around a job (macOS).
///
/// `deny_dirs`: subtrees the job may neither read nor write (the node's
/// `state/` and `run/`, `/etc/k2-node`, the executor's state, `/Users`,
/// the keychains). `deny_sockets`: Unix sockets it may not connect to
/// (the executor's). With `network_none`: no IP connection, bind or
/// listen beyond this machine, and no DNS. Seatbelt's `localhost` matches
/// the Mac's own addresses too, so a job still reaches services on the
/// same machine (as Linux loopback would), never anything off it. Order matters: Seatbelt takes the
/// last matching rule, so the loopback allows come after the denies.
pub fn seatbelt_profile(deny_dirs: &[PathBuf], deny_sockets: &[PathBuf], network_none: bool) -> String {
    let mut p = String::from("(version 1)\n(allow default)\n");
    for d in deny_dirs {
        p.push_str(&format!("(deny file-read* file-write* (subpath {}))\n", sb_str(&real_path(d))));
    }
    for s in deny_sockets {
        p.push_str(&format!("(deny network-outbound (literal {}))\n", sb_str(&real_path(s))));
    }
    if network_none {
        p.push_str("(deny mach-lookup (global-name \"com.apple.dnssd.service\"))\n");
        p.push_str("(deny network-outbound (literal \"/private/var/run/mDNSResponder\"))\n");
        p.push_str("(deny network-outbound (remote ip))\n");
        p.push_str("(deny network-bind (local ip))\n");
        p.push_str("(deny network-inbound (local ip))\n");
        p.push_str("(allow network-outbound (remote ip \"localhost:*\"))\n");
        p.push_str("(allow network-bind (local ip \"localhost:*\"))\n");
        p.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
    }
    p
}

/// Which server's job a Seatbelt profile walls (S3m; macOS 0.46.3 has one
/// uid for every job, so the profile is the whole wall between servers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatbeltTenant {
    pub slot: k2_node_proto::share::Slot,
    /// The node home (`/var/k2node`).
    pub home: PathBuf,
    /// `<home>-t` (every tenant's root is under it).
    pub tenants_root: PathBuf,
}

/// The Seatbelt profile around one server's job (macOS): today's denies
/// ([`seatbelt_profile`]) plus the wall between servers.
///
/// - **The owner's jobs** can't read or write any tenant root, or connect
///   to a socket in one (V61).
/// - **A tenant's jobs** can't read or write the node home (only
///   `toolchains/`, read-only), any other tenant's root, `/private/tmp`
///   or `/private/var/tmp` (their `TMPDIR` is their own), can't connect
///   to a socket in another server's root or the node home.
///
/// One uid means other servers' processes stay visible (no Seatbelt
/// `process-info*` deny: unproven, see below) and signals
/// between servers' jobs are **not** stopped (a
/// Seatbelt `signal` deny would also stop a job signalling its own
/// children; V60): that's why Macs take only your own servers in 0.46.3.
pub fn seatbelt_tenant_profile(deny_dirs: &[PathBuf], deny_sockets: &[PathBuf], network_none: bool, t: &SeatbeltTenant) -> String {
    let mut p = String::from("(version 1)\n(allow default)\n");
    let rw = |p: &mut String, verb: &str, d: &Path| p.push_str(&format!("({verb} file-read* file-write* (subpath {}))\n", sb_str(&real_path(d))));
    for d in deny_dirs {
        rw(&mut p, "deny", d);
    }
    let troot = real_path(&t.tenants_root);
    if t.slot == k2_node_proto::share::OWNER_SLOT {
        rw(&mut p, "deny", &troot);
        p.push_str(&format!("(deny network-outbound (subpath {}))\n", sb_str(&troot)));
    } else {
        let own = troot.join(t.slot.to_string());
        rw(&mut p, "deny", &t.home);
        p.push_str(&format!("(allow file-read* (subpath {}))\n", sb_str(&real_path(&t.home.join("toolchains")))));
        for d in TENANT_DENY_SHARED {
            rw(&mut p, "deny", Path::new(d));
        }
        rw(&mut p, "deny", &troot);
        rw(&mut p, "allow", &own);
        // Unix sockets: none in another server's root or the node home.
        p.push_str(&format!("(deny network-outbound (subpath {}))\n", sb_str(&troot)));
        p.push_str(&format!("(deny network-outbound (subpath {}))\n", sb_str(&real_path(&t.home))));
        p.push_str(&format!("(allow network-outbound (subpath {}))\n", sb_str(&own)));
        // No `process-info*` deny: unproven on macOS 27 (on a mini, `ps`
        // of a job's own child shows nothing inside the node's nested
        // Seatbelt with or without it, so it can't be shown to hide only
        // others), and hiding a job's own children would break build and
        // test tools. Other jobs' processes stay visible on a Mac in
        // 0.46.3 (one uid; own servers only).
    }
    // The node's own sockets (the executor) stay closed whatever the
    // allows above say: after them, so they win.
    for s in deny_sockets {
        p.push_str(&format!("(deny network-outbound (literal {}))\n", sb_str(&real_path(s))));
    }
    if network_none {
        push_network_none(&mut p);
    }
    p
}

/// Shared scratch a tenant's job can't use on a one-uid Mac (another
/// server's job could read or plant files there).
pub const TENANT_DENY_SHARED: &[&str] = &["/private/tmp", "/private/var/tmp", "/Users/Shared"];

fn push_network_none(p: &mut String) {
    p.push_str("(deny mach-lookup (global-name \"com.apple.dnssd.service\"))\n");
    p.push_str("(deny network-outbound (literal \"/private/var/run/mDNSResponder\"))\n");
    p.push_str("(deny network-outbound (remote ip))\n");
    p.push_str("(deny network-bind (local ip))\n");
    p.push_str("(deny network-inbound (local ip))\n");
    p.push_str("(allow network-outbound (remote ip \"localhost:*\"))\n");
    p.push_str("(allow network-bind (local ip \"localhost:*\"))\n");
    p.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
}

/// `sandbox-exec -p <profile> -- argv…`.
pub fn seatbelt_argv(profile: &str, argv: &[String]) -> Vec<String> {
    let mut v = vec![SANDBOX_EXEC.to_string(), "-p".to_string(), profile.to_string(), "--".to_string()];
    v.extend(argv.iter().cloned());
    v
}

// ── the Linux unit's `job-exec` shim ────────────────────────────────

/// What `k2-node job-exec` runs: written by the node into the job dir
/// (0640, group = the node's group, which is the job's group), read and
/// deleted by the shim as `k2job`. Passing the command this way keeps it
/// out of systemd's `$VAR` / `%` expansion and out of D-Bus size limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecSpec {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// Under the job root, in its own group-writable dir so the shim (as the
/// job user) can delete it once read; the root itself stays the node's.
pub const EXEC_SPEC: &str = "exec/exec.json";
const MAX_SPEC_BYTES: u64 = 4 << 20;

pub fn write_exec_spec(root: &Path, spec: &ExecSpec) -> Result<PathBuf, String> {
    let p = root.join(EXEC_SPEC);
    if let Some(d) = p.parent() {
        group_dir(d)?;
    }
    let body = serde_json::to_vec(spec).map_err(|e| format!("encode {EXEC_SPEC}: {e}"))?;
    crate::util::atomic_write(&p, &body, 0o640)?;
    Ok(p)
}

/// The shim: read + delete the spec, then exec the job with exactly its
/// env and cwd (`PATH` from that env finds the program). Returns only on
/// failure (the message for stderr).
pub fn job_exec(spec_path: &Path) -> String {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut body = Vec::new();
    let read = std::fs::File::open(spec_path).and_then(|f| f.take(MAX_SPEC_BYTES).read_to_end(&mut body));
    if let Err(e) = read {
        return format!("k2-node job-exec: read {}: {e}", spec_path.display());
    }
    let _ = std::fs::remove_file(spec_path);
    let spec: ExecSpec = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(e) => return format!("k2-node job-exec: {}: {e}", spec_path.display()),
    };
    let Some(program) = spec.argv.first() else { return "k2-node job-exec: empty argv".into() };
    let mut cmd = std::process::Command::new(program);
    cmd.args(&spec.argv[1..]).env_clear().current_dir(&spec.cwd);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    let e = cmd.exec();
    format!("k2-node job-exec: exec {program}: {e}")
}

// ── race-free tree walks (no symlink is ever followed) ──────────────

fn errno() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Open a directory without following a symlink at its last component.
fn open_dir_at(dirfd: libc::c_int, name: &CStr) -> std::io::Result<OwnedFd> {
    // SAFETY: valid C string; the fd is owned by the returned OwnedFd.
    let fd = unsafe { libc::openat(dirfd, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(errno());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Open a directory on the WAY to another one, without following a symlink
/// at its last component. On Linux an `O_PATH` fd: it needs only search
/// (`x`) on the directory, never read, so a walk passes through an
/// ancestor the caller may enter but not list (a job's temp dir under the
/// node's `0710` state dir, `/home/<user>` at `0711`). Such an fd is only
/// ever a dirfd for the next `fstatat`/`readlinkat`/`openat` and an
/// `fstat`; the directory the walk ends at is opened for real by
/// [`open_dir_at`]. Elsewhere (no `O_PATH`) the same as [`open_dir_at`].
fn open_way_at(dirfd: libc::c_int, name: &CStr) -> std::io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: valid C string; the fd is owned by the returned OwnedFd.
        let fd = unsafe { libc::openat(dirfd, name.as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(errno());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
    #[cfg(not(target_os = "linux"))]
    open_dir_at(dirfd, name)
}

/// Open the directory at absolute `p` one component at a time, each with
/// `O_NOFOLLOW` (review A, H2): a link the node or a job planted anywhere
/// on the way fails the open instead of moving root somewhere else, and
/// what's opened is the very directory checked (no re-resolution later).
/// Only a link root owns, in a directory only root may write, is followed
/// (`/var` → `private/var` on macOS; nobody else can plant or swap one),
/// and never at the last component: the directory itself is never a link.
/// Ancestors are only passed through ([`open_way_at`]: search permission,
/// no read); the last component is opened readable, as the walks need.
pub fn open_dir_path(p: &Path) -> Result<OwnedFd, String> {
    use std::collections::VecDeque;
    use std::path::Component;
    let bad = |m: String| format!("open {}: {m}", p.display());
    if !p.is_absolute() {
        return Err(bad("not an absolute path".into()));
    }
    let slash = CString::new("/").unwrap();
    let mut cur = open_dir_at(libc::AT_FDCWD, &slash).map_err(|e| bad(e.to_string()))?;
    let mut todo: VecDeque<std::ffi::OsString> = VecDeque::new();
    let push_path = |todo: &mut VecDeque<std::ffi::OsString>, q: &Path, front: bool| -> Result<(), String> {
        let mut parts = Vec::new();
        for c in q.components() {
            match c {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(n) => parts.push(n.to_os_string()),
                Component::ParentDir => return Err("`..` on the way".into()),
                Component::Prefix(_) => return Err("a path prefix".into()),
            }
        }
        if front {
            for n in parts.into_iter().rev() {
                todo.push_front(n);
            }
        } else {
            todo.extend(parts);
        }
        Ok(())
    };
    push_path(&mut todo, p, false).map_err(bad)?;
    let mut hops = 0;
    while let Some(n) = todo.pop_front() {
        let name = CString::new(n.as_bytes()).map_err(|_| bad("a NUL".into()))?;
        let st = lstat_at(&cur, &name).map_err(|e| bad(format!("{}: {e}", n.to_string_lossy())))?;
        if mode_of(&st) & S_IFMT == S_IFLNK {
            if todo.is_empty() {
                return Err(bad(format!("{} is a link", n.to_string_lossy())));
            }
            let parent = fstat(&cur).map_err(|e| bad(e.to_string()))?;
            if st.st_uid != 0 || parent.st_uid != 0 || mode_of(&parent) & 0o022 != 0 {
                return Err(bad(format!("{} is a link not root's own", n.to_string_lossy())));
            }
            hops += 1;
            if hops > 16 {
                return Err(bad("too many links".into()));
            }
            let mut buf = vec![0u8; 4096];
            // SAFETY: a valid dir fd, C string and buffer.
            let len = unsafe { libc::readlinkat(cur.as_raw_fd(), name.as_ptr(), buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
            if len < 0 || len as usize >= buf.len() {
                return Err(bad(format!("read link {}", n.to_string_lossy())));
            }
            buf.truncate(len as usize);
            let target = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf));
            if target.is_absolute() {
                cur = open_dir_at(libc::AT_FDCWD, &slash).map_err(|e| bad(e.to_string()))?;
            }
            push_path(&mut todo, &target, true).map_err(bad)?;
            continue;
        }
        let open: fn(libc::c_int, &CStr) -> std::io::Result<OwnedFd> = if todo.is_empty() { open_dir_at } else { open_way_at };
        cur = open(cur.as_raw_fd(), &name).map_err(|e| bad(format!("{}: {e}", n.to_string_lossy())))?;
    }
    Ok(cur)
}

fn open_root(p: &Path) -> Result<OwnedFd, String> {
    open_dir_path(p)
}

fn fstat(fd: &OwnedFd) -> std::io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid fd and out-pointer.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(errno());
    }
    Ok(st)
}

fn lstat_at(dirfd: &OwnedFd, name: &CStr) -> std::io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid fd, C string and out-pointer.
    if unsafe { libc::fstatat(dirfd.as_raw_fd(), name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(errno());
    }
    Ok(st)
}

/// Entry names of an open directory (not `.`/`..`).
fn names(dir: &OwnedFd) -> std::io::Result<Vec<CString>> {
    // fdopendir owns the fd it gets: hand it a dup.
    // SAFETY: valid fd; the dup is closed by closedir.
    let dup = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return Err(errno());
    }
    let d = unsafe { libc::fdopendir(dup) };
    if d.is_null() {
        let e = errno();
        unsafe { libc::close(dup) };
        return Err(e);
    }
    let mut out = Vec::new();
    loop {
        // SAFETY: d is a live DIR*.
        let ent = unsafe { libc::readdir(d) };
        if ent.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
        let b = name.to_bytes();
        if b == b"." || b == b".." {
            continue;
        }
        out.push(name.to_owned());
    }
    unsafe { libc::closedir(d) };
    Ok(out)
}

const S_IFMT: u32 = libc::S_IFMT as u32;
const S_IFDIR: u32 = libc::S_IFDIR as u32;
const S_IFREG: u32 = libc::S_IFREG as u32;
const S_IFLNK: u32 = libc::S_IFLNK as u32;

fn mode_of(st: &libc::stat) -> u32 {
    st.st_mode as u32
}

/// What a walk did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WalkStats {
    pub changed: u64,
    pub skipped: u64,
}

/// Depth cap (a hostile tree can't recurse us out of stack or fds).
const MAX_DEPTH: u32 = 256;

/// Root side (the executor, after a job's unit is gone): give a job tree
/// back to the node user so it can clean up and reuse it. Every entry is
/// chowned to `uid:gid` without following a symlink; directories also get
/// `u+rwx`. Returns what it changed.
pub fn reclaim(root: &Path, uid: u32, gid: u32) -> Result<WalkStats, String> {
    let fd = open_root(root)?;
    let mut s = WalkStats::default();
    chown_dir_fd(&fd, uid, gid, &mut s);
    reclaim_in(&fd, uid, gid, 0, &mut s);
    Ok(s)
}

fn chown_dir_fd(fd: &OwnedFd, uid: u32, gid: u32, s: &mut WalkStats) {
    // SAFETY: valid fd.
    let ok = unsafe { libc::fchown(fd.as_raw_fd(), uid, gid) } == 0;
    let mode = fstat(fd).map(|st| mode_of(&st) & 0o7777).unwrap_or(0o700);
    let ok2 = unsafe { libc::fchmod(fd.as_raw_fd(), (mode | 0o700) as libc::mode_t) } == 0;
    if ok && ok2 {
        s.changed += 1;
    } else {
        s.skipped += 1;
    }
}

fn reclaim_in(dir: &OwnedFd, uid: u32, gid: u32, depth: u32, s: &mut WalkStats) {
    if depth >= MAX_DEPTH {
        s.skipped += 1;
        return;
    }
    let Ok(list) = names(dir) else {
        s.skipped += 1;
        return;
    };
    for n in list {
        let Ok(st) = lstat_at(dir, &n) else {
            s.skipped += 1;
            continue;
        };
        if mode_of(&st) & S_IFMT == S_IFDIR {
            match open_dir_at(dir.as_raw_fd(), &n) {
                Ok(child) => {
                    chown_dir_fd(&child, uid, gid, s);
                    reclaim_in(&child, uid, gid, depth + 1, s);
                }
                Err(_) => s.skipped += 1,
            }
        } else if mode_of(&st) & S_IFMT == S_IFREG && st.st_nlink > 1 {
            // A hard link may be a file from outside the tree (where
            // protected_hardlinks is off): never chown one (review A H2).
            s.skipped += 1;
        } else {
            // SAFETY: valid fd + name; NOFOLLOW changes a link itself.
            let rc = unsafe { libc::fchownat(dir.as_raw_fd(), n.as_ptr(), uid, gid, libc::AT_SYMLINK_NOFOLLOW) };
            if rc == 0 {
                s.changed += 1;
            } else {
                s.skipped += 1;
            }
        }
    }
}

/// Node side (Linux job wall): make a tree the node built usable by the
/// job (its group is the node's group): `g+rw` on files, `g+rwx` on
/// directories, `g+x` where the owner has `x`, for entries the node owns.
/// Files are changed through an fd opened with `O_NOFOLLOW`, so a symlink
/// a job planted never redirects the chmod.
pub fn share_with_jobs(root: &Path) -> Result<WalkStats, String> {
    let fd = open_root(root)?;
    let me = unsafe { libc::geteuid() };
    let mut s = WalkStats::default();
    share_fd(&fd, me, true, &mut s);
    share_in(&fd, me, 0, &mut s);
    Ok(s)
}

fn share_fd(fd: &OwnedFd, me: u32, is_dir: bool, s: &mut WalkStats) {
    let Ok(st) = fstat(fd) else {
        s.skipped += 1;
        return;
    };
    if st.st_uid != me {
        s.skipped += 1;
        return;
    }
    let mode = mode_of(&st) & 0o7777;
    let mut want = mode | 0o060;
    if is_dir || mode & 0o100 != 0 {
        want |= 0o010;
    }
    if want == mode {
        return;
    }
    // SAFETY: valid fd.
    if unsafe { libc::fchmod(fd.as_raw_fd(), want as libc::mode_t) } == 0 {
        s.changed += 1;
    } else {
        s.skipped += 1;
    }
}

fn share_in(dir: &OwnedFd, me: u32, depth: u32, s: &mut WalkStats) {
    if depth >= MAX_DEPTH {
        s.skipped += 1;
        return;
    }
    let Ok(list) = names(dir) else {
        s.skipped += 1;
        return;
    };
    for n in list {
        let Ok(st) = lstat_at(dir, &n) else {
            s.skipped += 1;
            continue;
        };
        let kind = mode_of(&st) & S_IFMT;
        if kind == S_IFDIR {
            match open_dir_at(dir.as_raw_fd(), &n) {
                Ok(child) => {
                    share_fd(&child, me, true, s);
                    share_in(&child, me, depth + 1, s);
                }
                Err(_) => s.skipped += 1,
            }
        } else if kind == S_IFREG && st.st_uid == me {
            // SAFETY: valid fd + name; O_NOFOLLOW refuses a swapped-in link.
            let fd = unsafe {
                libc::openat(dir.as_raw_fd(), n.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            };
            if fd < 0 {
                s.skipped += 1;
                continue;
            }
            let f = unsafe { OwnedFd::from_raw_fd(fd) };
            share_fd(&f, me, false, s);
        }
    }
}

/// Make a directory usable by the node's jobs (Linux wall): the node
/// user's group gets `rwx`. Opened with `O_NOFOLLOW`; a dir a job already
/// owns (it made it in an earlier run) is left as it is.
pub fn group_dir(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    let fd = open_root(p)?;
    let st = fstat(&fd).map_err(|e| format!("stat {}: {e}", p.display()))?;
    if st.st_uid != unsafe { libc::geteuid() } {
        return Ok(());
    }
    let mode = mode_of(&st) & 0o7777;
    // SAFETY: valid fd.
    if unsafe { libc::fchmod(fd.as_raw_fd(), (mode | 0o770) as libc::mode_t) } != 0 {
        return Err(format!("chmod {}: {}", p.display(), errno()));
    }
    Ok(())
}

/// Marks a warm target dir whose node-made contents were shared once.
pub const SHARED_MARK: &str = ".k2-job-wall";

// ── tenant roots (root side) ────────────────────────────────────────

/// Root: make `path` a real directory (created if missing; never through
/// a symlink at its last component) owned by `uid:gid` with `mode`
/// (setgid allowed). Refuses anything that isn't a plain directory.
pub fn secure_dir(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(format!("{} exists and isn't a plain folder", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).map_err(|e| format!("create {}: {e}", path.display()))?;
        }
        Err(e) => return Err(format!("stat {}: {e}", path.display())),
    }
    let fd = open_root(path)?;
    // SAFETY: valid fd.
    if unsafe { libc::fchown(fd.as_raw_fd(), uid, gid) } != 0 {
        return Err(format!("chown {}: {}", path.display(), errno()));
    }
    // SAFETY: valid fd.
    if unsafe { libc::fchmod(fd.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(format!("chmod {}: {}", path.display(), errno()));
    }
    Ok(())
}

/// Root: remove `name` in the open directory `parent` and, for a
/// directory, everything in it, through fds only: no link is followed
/// anywhere (a link is removed as a link), nothing is reached by a path
/// another user could swap (review A, M1). Bounded depth.
pub fn remove_tree_at(parent: &OwnedFd, name: &CStr) -> Result<(), String> {
    fn rm(parent: &OwnedFd, name: &CStr, depth: u32) -> std::io::Result<()> {
        let st = match lstat_at(parent, name) {
            Ok(st) => st,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if mode_of(&st) & S_IFMT == S_IFDIR {
            if depth >= MAX_DEPTH {
                return Err(std::io::Error::other("too deep"));
            }
            let dir = open_dir_at(parent.as_raw_fd(), name)?;
            for n in names(&dir)? {
                rm(&dir, &n, depth + 1)?;
            }
            // SAFETY: valid dir fd and C string.
            if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                return Err(errno());
            }
        } else {
            // SAFETY: valid dir fd and C string; a link goes as a link.
            if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(errno());
            }
        }
        Ok(())
    }
    rm(parent, name, 0).map_err(|e| format!("remove {}: {e}", name.to_string_lossy()))
}

/// Root: the open, root-owned directory at `p` that only root may write
/// (`<home>-t`, `/var/lib`): `Err` when it's anyone else's.
pub fn open_root_owned_dir(p: &Path) -> Result<OwnedFd, String> {
    let fd = open_dir_path(p)?;
    let st = fstat(&fd).map_err(|e| format!("stat {}: {e}", p.display()))?;
    if st.st_uid != 0 || mode_of(&st) & 0o022 != 0 {
        return Err(format!("{} must be root's and writable by root only (uid {}, mode {:o})", p.display(), st.st_uid, mode_of(&st) & 0o7777));
    }
    Ok(fd)
}

/// Root: `chmod o-rwx` one directory (never through a link).
pub fn drop_other_bits(path: &Path) -> Result<(), String> {
    let fd = open_root(path)?;
    let st = fstat(&fd).map_err(|e| format!("stat {}: {e}", path.display()))?;
    let mode = mode_of(&st) & 0o7777;
    if mode & 0o007 == 0 {
        return Ok(());
    }
    // SAFETY: valid fd.
    if unsafe { libc::fchmod(fd.as_raw_fd(), (mode & !0o007) as libc::mode_t) } != 0 {
        return Err(format!("chmod {}: {}", path.display(), errno()));
    }
    Ok(())
}

/// Root (`tenant.wipe`): remove the entries directly in `dir` owned by
/// `uid` (a wiped tenant's leftovers in `/dev/shm`, `/tmp`, `/var/tmp`).
/// Links are removed as links; trees are removed without following any.
pub fn remove_owned_by(dir: &Path, uid: u32) -> Result<Vec<String>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(format!("read {}: {e}", dir.display())),
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.uid() != uid {
            continue;
        }
        let r = if m.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        r.map_err(|e| format!("remove {}: {e}", p.display()))?;
        out.push(p.display().to_string());
    }
    Ok(out)
}

// ── people's home folders ───────────────────────────────────────────

/// Where people's homes live on this OS.
pub fn homes_root() -> &'static Path {
    if cfg!(target_os = "macos") {
        Path::new("/Users")
    } else {
        Path::new("/home")
    }
}

/// Home folders under `root` that other users can read or enter (the
/// "other" bits; a job without the wall, or another local user, gets
/// them). `/Users/Shared` and dot entries aren't anyone's home.
pub fn open_homes_in(root: &Path) -> Vec<(PathBuf, u32)> {
    use std::os::unix::fs::PermissionsExt;
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    for e in rd.flatten() {
        let name = e.file_name();
        let n = name.to_string_lossy();
        if n.starts_with('.') || n == "Shared" || n == "Guest" {
            continue;
        }
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if !m.is_dir() {
            continue;
        }
        let mode = m.permissions().mode() & 0o7777;
        if mode & 0o005 != 0 {
            out.push((e.path(), mode));
        }
    }
    out.sort();
    out
}

/// Root: make one home private (`chmod o-rwx` and `g-rwx`: only its
/// owner). Refuses anything that isn't a real directory directly under
/// [`homes_root`]-style `root`. Returns (old mode, new mode).
pub fn make_private(root: &Path, home: &Path) -> Result<(u32, u32), String> {
    let parent = home.parent().ok_or_else(|| format!("{} has no parent", home.display()))?;
    if parent != root || home.file_name().is_none() {
        return Err(format!("{} is not a home folder under {}", home.display(), root.display()));
    }
    let fd = open_root(home)?;
    let st = fstat(&fd).map_err(|e| format!("stat {}: {e}", home.display()))?;
    let old = mode_of(&st) & 0o7777;
    let new = old & 0o7700;
    // SAFETY: valid fd.
    if unsafe { libc::fchmod(fd.as_raw_fd(), new as libc::mode_t) } != 0 {
        return Err(format!("chmod {}: {}", home.display(), errno()));
    }
    Ok((old, new))
}

// ── probe: what can this process read, list, reach? ─────────────────

/// `k2-node wall-probe --read P --list D --connect HOST:PORT …`: one JSON
/// object, `"ok"` or the error for each. Never fails itself; the wall
/// tests run it inside the wall and assert on the answers.
pub fn probe(read: &[String], list: &[String], connect: &[String]) -> serde_json::Value {
    use std::io::Read;
    let mut r = serde_json::Map::new();
    for p in read {
        let v = match std::fs::File::open(p).and_then(|mut f| {
            let mut b = [0u8; 64];
            f.read(&mut b)
        }) {
            Ok(_) => "ok".to_string(),
            Err(e) => format!("error: {e}"),
        };
        r.insert(p.clone(), v.into());
    }
    let mut l = serde_json::Map::new();
    for d in list {
        let v = match std::fs::read_dir(d) {
            Ok(rd) => format!("ok: {} entries", rd.count()),
            Err(e) => format!("error: {e}"),
        };
        l.insert(d.clone(), v.into());
    }
    let mut c = serde_json::Map::new();
    for a in connect {
        let v = match a.parse::<std::net::SocketAddr>() {
            Ok(sa) => match std::net::TcpStream::connect_timeout(&sa, std::time::Duration::from_secs(3)) {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("error: {e}"),
            },
            Err(e) => format!("error: bad address: {e}"),
        };
        c.insert(a.clone(), v.into());
    }
    serde_json::json!({ "read": r, "list": l, "connect": c, "uid": unsafe { libc::geteuid() } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_denies_paths_and_keeps_loopback_for_network_none() {
        let p = seatbelt_profile(&[PathBuf::from("/nonexistent-k2/state"), PathBuf::from("/Users")], &[PathBuf::from("/var/run/k2-node-admin.sock")], true);
        assert!(p.starts_with("(version 1)\n(allow default)\n"));
        assert!(p.contains("(deny file-read* file-write* (subpath \"/nonexistent-k2/state\"))"), "{p}");
        assert!(p.contains("(deny file-read* file-write* (subpath \"/Users\"))"));
        if cfg!(target_os = "macos") {
            assert!(p.contains("(deny network-outbound (literal \"/private/var/run/k2-node-admin.sock\"))"), "macOS real path: {p}");
        }
        // Seatbelt takes the last match: loopback allows after the denies.
        let deny = p.find("(deny network-outbound (remote ip))").unwrap();
        let allow = p.find("(allow network-outbound (remote ip \"localhost:*\"))").unwrap();
        assert!(deny < allow);
        // Unix-socket denies must come before the loopback allow (a path
        // rule after it shadows loopback connects; found by hand on 27.0).
        assert!(p.find("mDNSResponder").unwrap() < allow);
        let full = seatbelt_profile(&[], &[], false);
        assert!(!full.contains("network"), "network = full adds no network rule");
    }

    #[test]
    fn profile_strings_are_escaped() {
        let p = seatbelt_profile(&[PathBuf::from("/a\"b\\c\nd")], &[], false);
        assert!(p.contains("(subpath \"/a\\\"b\\\\cd\")"), "{p}");
    }

    #[test]
    fn wall_labels_and_network() {
        assert_eq!(Wall::Seatbelt.label(), "seatbelt");
        assert_eq!(Wall::JobUser { user: "k2job".into() }.label(), "job_user");
        let none = Wall::None { why: "x".into() };
        assert_eq!(none.label(), "none");
        assert!(!none.network_none() && !none.is_walled());
        assert!(Wall::Seatbelt.network_none());
    }

    #[test]
    fn exec_spec_round_trips_and_is_group_readable() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("exec-spec");
        let s = ExecSpec { argv: vec!["cargo".into(), "test".into(), "$HOME".into()], env: vec![("PATH".into(), "/bin".into())], cwd: d.clone() };
        let p = write_exec_spec(&d, &s).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
        let back: ExecSpec = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(back, s, "argv is opaque: no $ or % expansion anywhere");
    }

    #[test]
    fn job_exec_reports_a_bad_spec_and_deletes_it() {
        let d = crate::util::temp_dir("exec-bad");
        let p = d.join(EXEC_SPEC);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{not json").unwrap();
        let e = job_exec(&p);
        assert!(e.starts_with("k2-node job-exec:"), "{e}");
        assert!(!p.exists(), "read once, then gone");
        assert!(job_exec(&d.join("missing.json")).contains("read"));
    }

    #[test]
    fn share_with_jobs_adds_group_bits_and_never_follows_links() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("share");
        let outside = d.join("outside.pem");
        std::fs::write(&outside, "secret").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600)).unwrap();
        let t = d.join("tree");
        std::fs::create_dir_all(t.join("a/b")).unwrap();
        std::fs::write(t.join("a/f.rs"), "x").unwrap();
        std::fs::set_permissions(t.join("a/f.rs"), std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(t.join("a/run.sh"), "x").unwrap();
        std::fs::set_permissions(t.join("a/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Every fixture mode explicit: the test's umask (0002 in a walled
        // K2 Compute job) must not pre-share anything.
        for dir in [t.clone(), t.join("a"), t.join("a/b")] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::os::unix::fs::symlink(&outside, t.join("a/link.pem")).unwrap();
        let s = share_with_jobs(&t).unwrap();
        assert_eq!(s, WalkStats { changed: 5, skipped: 0 }, "tree, a, a/b, f.rs, run.sh; the link is not touched");
        let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&t), 0o775);
        assert_eq!(mode(&t.join("a")), 0o775);
        assert_eq!(mode(&t.join("a/f.rs")), 0o664);
        assert_eq!(mode(&t.join("a/run.sh")), 0o775);
        assert_eq!(mode(&t.join("a/b")), 0o775);
        assert_eq!(std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777, 0o600, "a planted symlink never widens its target");
    }

    #[test]
    fn reclaim_walks_without_following_links() {
        // As a normal user, chown to our own ids is allowed: the walk
        // must visit everything, fix dir modes, and leave link targets.
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("reclaim");
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o500)).unwrap();
        let t = d.join("tree");
        std::fs::create_dir_all(t.join("x/y")).unwrap();
        std::fs::write(t.join("x/y/f"), "1").unwrap();
        std::fs::set_permissions(t.join("x/y"), std::fs::Permissions::from_mode(0o500)).unwrap();
        std::os::unix::fs::symlink(&outside, t.join("x/out")).unwrap();
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let s = reclaim(&t, uid, gid).unwrap();
        assert!(s.changed >= 5, "{s:?}");
        assert_eq!(std::fs::metadata(t.join("x/y")).unwrap().permissions().mode() & 0o700, 0o700, "dirs get u+rwx back");
        assert_eq!(std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777, 0o500, "the link's target is untouched");
        std::fs::remove_dir_all(&t).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// H2: a link anywhere on the way to a reclaim root (not only its last
    /// component) fails the open: the race that swapped `cache/x` for a
    /// link to `/` can't move root's chown. A hard-linked file is never
    /// chowned. Root's own links in root's dirs still resolve (macOS /var).
    #[test]
    fn reclaim_never_follows_a_link_on_the_way() {
        use std::os::unix::fs::MetadataExt;
        let d = crate::util::temp_dir("reclaim-race");
        let real = d.join("real");
        std::fs::create_dir_all(real.join("etc")).unwrap();
        std::os::unix::fs::symlink(&real, d.join("x")).unwrap();
        // The planted link is the node user's (as root here, say so).
        if unsafe { libc::geteuid() } == 0 {
            std::os::unix::fs::lchown(d.join("x"), Some(65534), Some(65534)).unwrap();
        }
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let e = reclaim(&d.join("x/etc"), uid, gid).unwrap_err();
        assert!(e.contains("a link not root's own") || e.contains("Too many levels") || e.contains("not a directory"), "{e}");
        assert!(open_dir_path(&d.join("x")).is_err(), "the last component too");
        assert!(open_dir_path(&real.join("etc")).is_ok());
        assert!(open_dir_path(Path::new("relative/path")).is_err());
        assert!(open_dir_path(&real.join("../real")).is_err(), "no `..` on the way");
        // The system's own links (root's, in root's dirs) still resolve.
        let tmp = std::env::temp_dir();
        let via = open_dir_path(&tmp).unwrap();
        assert_eq!(fstat(&via).unwrap().st_ino as u64, std::fs::metadata(&tmp).unwrap().ino());
        // Hard links are skipped.
        let t = d.join("tree");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(d.join("victim"), "v").unwrap();
        std::fs::hard_link(d.join("victim"), t.join("hl")).unwrap();
        let s = reclaim(&t, uid, gid).unwrap();
        assert!(s.skipped >= 1, "{s:?}");
    }

    /// A walk needs only search permission on the directories on its way
    /// (K2 Compute's job wall: the job's temp dir sits under the node's
    /// state dir, which the job user may enter but not list). The test
    /// builds that shape in its own temp dir: an ancestor this user may
    /// enter (`--x`) but not read; the walk still opens the directory past
    /// it, reads it and changes modes in it, and a link planted past the
    /// unreadable ancestor is still refused. (Linux: `O_PATH`.)
    #[cfg(target_os = "linux")]
    #[test]
    fn walks_pass_an_ancestor_they_may_enter_but_not_list() {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::util::temp_dir("search-only");
        let gate = d.join("gate");
        let inner = gate.join("inner");
        std::fs::create_dir_all(inner.join("sub")).unwrap();
        std::fs::set_permissions(inner.join("sub"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(inner.join("f"), "x").unwrap();
        std::os::unix::fs::symlink(&inner, gate.join("alias")).unwrap();
        std::fs::set_permissions(&gate, std::fs::Permissions::from_mode(0o111)).unwrap();
        // SAFETY: no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            assert!(std::fs::read_dir(&gate).is_err(), "the fixture: this user can't list {}", gate.display());
        }
        let fd = open_dir_path(&inner).expect("through a search-only ancestor");
        let mut got: Vec<String> = names(&fd).unwrap().iter().map(|n| n.to_string_lossy().into_owned()).collect();
        got.sort();
        assert_eq!(got, vec!["f", "sub"], "the directory the walk ends at is opened readable");
        let s = share_with_jobs(&inner).unwrap();
        assert!(s.changed >= 1, "{s:?}");
        assert_eq!(std::fs::metadata(inner.join("sub")).unwrap().permissions().mode() & 0o770, 0o770);
        let e = open_dir_path(&gate.join("alias/sub")).map(|_| ()).unwrap_err();
        assert!(e.contains("a link not root's own"), "{e}");
        std::fs::set_permissions(&gate, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn open_homes_and_make_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = crate::util::temp_dir("homes");
        for (n, m) in [("open", 0o755), ("enter", 0o711), ("closed", 0o700), ("group", 0o750), ("Shared", 0o777)] {
            std::fs::create_dir_all(root.join(n)).unwrap();
            std::fs::set_permissions(root.join(n), std::fs::Permissions::from_mode(m)).unwrap();
        }
        std::os::unix::fs::symlink(root.join("open"), root.join("alias")).unwrap();
        let open: Vec<String> = open_homes_in(&root).iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(open, vec!["enter", "open"], "other-readable or enterable; never Shared or a link");
        assert_eq!(make_private(&root, &root.join("open")).unwrap(), (0o755, 0o700));
        assert_eq!(std::fs::metadata(root.join("open")).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(make_private(&root, &root.join("alias")).is_err(), "never through a symlink");
        assert!(make_private(&root, Path::new("/etc")).is_err(), "only homes under the root");
        assert!(make_private(&root, &root.join("open/sub")).is_err());
    }

    #[test]
    fn probe_reports_reads_lists_and_connects() {
        let d = crate::util::temp_dir("probe");
        std::fs::write(d.join("f"), "x").unwrap();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let f = d.join("f").display().to_string();
        let missing = d.join("nope").display().to_string();
        let v = probe(&[f.clone(), missing.clone()], &[d.display().to_string()], std::slice::from_ref(&addr));
        assert_eq!(v["read"][&f], "ok");
        assert!(v["read"][&missing].as_str().unwrap().starts_with("error:"));
        assert_eq!(v["list"][&d.display().to_string()], "ok: 1 entries");
        assert_eq!(v["connect"][&addr], "ok");
    }
}
