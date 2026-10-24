//! The Linux job wall, root side (0.46.3, N3; `eval-compute-shape-A` S1).
//!
//! k2-node runs under `NoNewPrivileges` and must never own the jobs' user,
//! so the root executor starts each job for it: one fixed-template verb
//! family on the local socket, asked by the node user only (the peer
//! check), never by the controller. The node chooses the job's command,
//! env and limits (it can run any job anyway); root chooses everything
//! that makes the wall: the user (`job_user` in root-owned
//! `/etc/k2-node/jobs.toml`), the unit's sandbox properties, which paths
//! are writable (only under the node home's `jobs/` and `cache/`, checked
//! after resolving symlinks) and which are gone (the node's `state/` and
//! `run/`, `/etc/k2-node`, the executor's state).
//!
//! A job is `systemd-run --pipe --wait` of `k2-node job-exec <root>/exec.json`
//! as `k2job:<node group>` with the node's stdout/stderr pipes (passed
//! here over the socket with `SCM_RIGHTS`). `RemainAfterExit=yes` keeps
//! the unit loaded after the command ends, so the node reads its result
//! (`status`: exit code, signal, `oom-kill`) and then `release`s it.
//!
//! `jobs.toml` is a separate file (not `admin.toml`, which is strict and
//! read by older executors after a rollback): an older k2-node simply
//! never reads it, and without it there is no wall (jobs run as the node
//! user, as before).
//!
//! **Tenants** (prd-compute-shared-queue-v1 §8.2, S3a): a non-owner
//! server's job (`JobStart.tenant` = its slot 1–7) runs as that slot's own
//! user `k2job-<slot>` with group `k2t-<slot>` (`[tenants]` in `jobs.toml`,
//! written by `tenant.prepare`, see `admin::tenants`), in its own slice,
//! with its own dirs under `<home>-t/<slot>/`, and with every other
//! tenant's root and the owner's job-facing dirs gone. Root decides all of
//! it from root-owned files (`jobs.toml`, `servers.toml`); the node only
//! names the slot. The slot is also in the unit name (`k2job-t<slot>-<16
//! hex>`, V48), so one slot's ops can never address another slot's unit,
//! and a start never stops a running unit.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use k2_node_proto::admin::{code, AdminRefusal};
use k2_node_proto::share::{self, Network, Slot, OWNER_SLOT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::exec::Executor;

/// Root-owned, 0644, in the config dir. Written by `k2-node setup`.
pub const JOBS_FILE: &str = "jobs.toml";
/// The default job user (Linux).
pub const DEFAULT_JOB_USER: &str = "k2job";
/// Every job unit's name starts with this.
pub const UNIT_PREFIX: &str = "k2job-";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobsFile {
    /// The user jobs run as. Never root, never the node user. The owner
    /// server's jobs (slot 0) always run as this user.
    pub job_user: String,
    /// Non-owner servers' job users, by slot (`[tenants] 1 = { user =
    /// "k2job-1", uid = 61001, gid = 61001 }`), written by the executor's
    /// `tenant.prepare` (prd-compute-shared-queue-v1 §8.2, §11.1). A reused
    /// slot always gets a new uid (V64). Absent = no tenant users.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tenants: std::collections::BTreeMap<String, TenantUser>,
}

/// One tenant's job user and group (Linux).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantUser {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
}

impl JobsFile {
    /// Slot `slot`'s job user; `None` for the owner (it's [`Self::job_user`])
    /// and for a slot with no user yet (its jobs wait, `tenant_wall_missing`).
    pub fn tenant(&self, slot: u8) -> Option<&TenantUser> {
        if slot == k2_node_proto::share::OWNER_SLOT {
            return None;
        }
        self.tenants.get(&slot.to_string())
    }
}

/// `Ok(None)` when missing (no wall set up).
pub fn read_jobs_file(config_dir: &Path) -> Result<Option<JobsFile>, String> {
    let p = config_dir.join(JOBS_FILE);
    let s = match read_root_trusted(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", p.display())),
    };
    toml::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", p.display()))
}

/// Read a file root trusts (who jobs run as): never through a link, and,
/// read as root, only when root owns it and nobody else may write it. The
/// config dir is the machine owner's group's to write (review A M5): a
/// member could swap the file, never make root trust their copy.
pub fn read_root_trusted(p: &Path) -> std::io::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // O_NONBLOCK: a FIFO planted in the group-writable config dir must
    // never wedge a reader (security review A-N1); a regular file ignores it.
    let mut f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(p)?;
    let m = f.metadata()?;
    if !m.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} is not a regular file", p.display())));
    }
    // SAFETY: plain getter.
    if unsafe { libc::geteuid() } == 0 && (m.uid() != 0 || m.mode() & 0o022 != 0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} must be root's and writable by root only (uid {}, mode {:o})", p.display(), m.uid(), m.mode() & 0o7777),
        ));
    }
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

/// Check a would-be job user against the node user.
pub fn check_job_user(job_user: &str, node_user: &str) -> Result<(u32, u32), String> {
    if job_user.is_empty() || !job_user.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        return Err(format!("{job_user:?} is not a plain user name"));
    }
    let (uid, gid) = super::server::user_ids(job_user).ok_or_else(|| format!("{job_user} is not a user on this machine"))?;
    if uid == 0 {
        return Err("jobs never run as root".into());
    }
    if job_user == node_user || super::server::user_ids(node_user).is_some_and(|(n, _)| n == uid) {
        return Err(format!("{job_user} is the node user; jobs need their own user"));
    }
    check_job_account(job_user, gid)?;
    Ok((uid, gid))
}

/// A job user is a bare account (review A L3, M5): no login shell, and no
/// group but its own (systemd gives a unit its user's supplementary
/// groups: group 0 would also lift `ProtectProc`, `adm`/`docker` more).
pub fn check_job_account(user: &str, gid: u32) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let c = std::ffi::CString::new(user).map_err(|_| format!("{user:?} has a NUL"))?;
    // SAFETY: getpwnam with a valid C string; the shell is copied out at once.
    let shell = unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned()
        }
    };
    if !(shell.ends_with("/nologin") || shell == "/bin/false" || shell == "/usr/bin/false") {
        return Err(format!("{user} has a login shell ({shell}); a job user gets nologin"));
    }
    let groups = supplementary_groups(&c, gid)?;
    if groups.iter().any(|g| *g != gid) {
        return Err(format!("{user} is in other groups ({groups:?}); a job user has only its own"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn supplementary_groups(user: &std::ffi::CStr, gid: u32) -> Result<Vec<u32>, String> {
    let mut n: libc::c_int = 64;
    loop {
        let mut v: Vec<libc::gid_t> = vec![0; n as usize];
        let want = n;
        // SAFETY: a valid C string and a buffer of `n` gids.
        let rc = unsafe { libc::getgrouplist(user.as_ptr(), gid, v.as_mut_ptr(), &mut n) };
        if rc >= 0 {
            v.truncate(n.max(0) as usize);
            return Ok(v);
        }
        if n <= want || n > 65536 {
            return Err("getgrouplist failed".into());
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn supplementary_groups(_user: &std::ffi::CStr, gid: u32) -> Result<Vec<u32>, String> {
    Ok(vec![gid])
}

/// gid of a group name.
pub fn group_id(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getgrnam reads a static buffer; we copy the id out at once.
    unsafe {
        let gr = libc::getgrnam(c.as_ptr());
        if gr.is_null() {
            return None;
        }
        Some((*gr).gr_gid)
    }
}

/// `sudo k2-node admin job-user <name>` (setup): validate and write.
pub fn write_jobs_file(config_dir: &Path, job_user: &str, node_user: &str) -> Result<String, String> {
    check_job_user(job_user, node_user)?;
    // One jobs.toml change at a time with tenant.prepare/wipe (review A M4).
    let _g = crate::util::FileLock::exclusive(&config_dir.join(super::tenants::OPS_LOCK_FILE))?;
    // Keep the tenants `tenant.prepare` wrote; setup only sets the owner's user.
    let tenants = read_jobs_file(config_dir).ok().flatten().map(|f| f.tenants).unwrap_or_default();
    write_jobs(config_dir, &JobsFile { job_user: job_user.to_string(), tenants })?;
    Ok(format!("jobs run as {job_user}"))
}

/// Write `jobs.toml` (root, 0644: the node reads which slots have users).
pub fn write_jobs(config_dir: &Path, f: &JobsFile) -> Result<(), String> {
    let body = toml::to_string(f).map_err(|e| e.to_string())?;
    let text = format!(
        "# K2 compute node: jobs run as this user in a sandboxed systemd unit (0.46.3).\n\
         # Written by k2-node setup and tenant.prepare; remove the file to go back to jobs as the node user.\n{body}"
    );
    crate::util::atomic_write(&config_dir.join(JOBS_FILE), text.as_bytes(), 0o644)
}

// ── the wire (node ↔ executor) ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum JobOp {
    /// Is the wall set up? (job user + systemd).
    Ready,
    /// Start a job. The request is followed by one `SCM_RIGHTS` message
    /// carrying exactly two fds: the job's stdout and stderr.
    Start(JobStart),
    Status { unit: String },
    /// SIGTERM everything in the unit, SIGKILL after its stop timeout
    /// (returns at once).
    Stop { unit: String },
    /// Stop (waits), forget the unit.
    Release { unit: String },
    /// Give a job tree back to the node user (chown, never following a
    /// symlink) so it can delete or reuse it. Only a tree of `slot` (a
    /// 0.46.2 node sends none: the owner's, the only slot it has).
    Reclaim {
        path: PathBuf,
        #[serde(default)]
        slot: Slot,
    },
    /// Can this executor wall tenants from each other, and which slots
    /// have a user? Asked before every tenant start: an older executor
    /// doesn't know this op (it refuses it), so a tenant's job never runs
    /// as the owner's job user there.
    Tenants,
    /// `tenant.prepare` (V54, V64, V75): only for a slot `servers.toml`
    /// approves (pending or active). Idempotent.
    TenantPrepare(share::TenantPrepare),
    /// `tenant.wipe` (V64): only for a slot `servers.toml` no longer
    /// approves (revoked, or no line), with no unit of that slot loaded.
    TenantWipe(share::TenantWipe),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStart {
    /// The owner's `<job_id>-g<generation>`, a tenant's `t<slot>-<16 hex>`
    /// (`node::job_key`); the unit is `k2job-<key>.service`.
    pub key: String,
    /// The job's dir: `<root>/jobs/<key>` (holds `exec.json`), where
    /// `<root>` is the node home (owner) or `<home>-t/<slot>` (tenant).
    pub root: PathBuf,
    /// More writable dirs: under the job root, the tenant's `cache/`, or
    /// the tenant's `tmp/` itself.
    pub rw: Vec<PathBuf>,
    pub network_none: bool,
    pub mem_bytes: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub stop_grace_secs: u64,
    /// The server's slot on this node (0 = the owner; absent = 0).
    #[serde(default)]
    pub tenant: Slot,
    /// `no-lan`: no private, link-local, tailnet/CGNAT or multicast
    /// address (V62). Root also applies the share's own network.
    #[serde(default)]
    pub no_lan: bool,
}

/// What `Tenants` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantWall {
    /// Tenant jobs can run here (Linux, systemd ≥ [`MIN_SYSTEMD_TENANT`],
    /// the job wall set up).
    pub ready: bool,
    /// Why not, when not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub systemd: u32,
    /// Slots with a user in `jobs.toml` (`"1" = "k2job-1"`).
    #[serde(default)]
    pub users: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum JobAnswer {
    Ready { job_user: String },
    Started { unit: String },
    Status(UnitStatus),
    Done { detail: Value },
    Refused(AdminRefusal),
    Tenants(TenantWall),
}

/// What `systemctl show` says about a job unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitStatus {
    pub loaded: bool,
    pub active_state: String,
    pub sub_state: String,
    /// `success` | `exit-code` | `signal` | `oom-kill` | `timeout` | …
    pub result: String,
    /// `ExecMainCode`: 1 exited, 2 killed, 3 dumped (0 = not yet).
    pub main_code: i32,
    /// The exit status or the signal number.
    pub main_status: i32,
    /// `oom_kill` from the unit's `memory.events`.
    pub oom_kills: u64,
}

impl UnitStatus {
    /// The command is over (the unit may still hold leftovers until
    /// `stop`). Asked only about a unit that started (`start` waits for it
    /// to load), so one systemd no longer knows ended and was collected:
    /// a stopped unit is unloaded at once, `RemainAfterExit` or not.
    pub fn ended(&self) -> bool {
        !self.loaded || self.active_state == "failed" || self.active_state == "inactive" || (self.active_state == "active" && self.sub_state == "exited")
    }

    /// (exit code, signal) of the main process, when it ended.
    pub fn exit(&self) -> Option<(Option<i32>, Option<i32>)> {
        match self.main_code {
            1 => Some((Some(self.main_status), None)),
            2 | 3 => Some((None, Some(self.main_status))),
            _ => None,
        }
    }

    pub fn oom(&self) -> bool {
        self.result == "oom-kill" || self.oom_kills > 0
    }
}

/// `systemctl show` key=value lines → status.
pub fn parse_show(text: &str) -> UnitStatus {
    let mut s = UnitStatus::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "LoadState" => s.loaded = v == "loaded",
            "ActiveState" => s.active_state = v.to_string(),
            "SubState" => s.sub_state = v.to_string(),
            "Result" => s.result = v.to_string(),
            "ExecMainCode" => s.main_code = v.parse().unwrap_or(0),
            "ExecMainStatus" => s.main_status = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    s
}

/// cgroup v2 `memory.events` → `oom_kill` count.
pub fn parse_oom_kills(text: &str) -> u64 {
    text.lines().find_map(|l| l.strip_prefix("oom_kill ")).and_then(|n| n.trim().parse().ok()).unwrap_or(0)
}

/// Oldest systemd whose unit sandbox has every property the template uses
/// (`ProtectClock`, 245: Ubuntu 20.04, Debian 11, current Arch/Fedora).
pub const MIN_SYSTEMD: u32 = 245;
/// Tenant units also need `ProtectProc=invisible` (247; kernel 5.8), so
/// one tenant's jobs can't see another's processes or command lines (V75),
/// and `PrivateIPC=` (248), always on for them (review A L2).
pub const MIN_SYSTEMD_TENANT: u32 = 248;
/// `PrivateIPC=` (248): used when there, on every unit.
pub const MIN_SYSTEMD_PRIVATE_IPC: u32 = 248;
/// `ProtectProc=invisible` (247): used when there, on every unit.
pub const MIN_SYSTEMD_PROTECT_PROC: u32 = 247;

/// `systemctl --version` → `255` from "systemd 255 (255.4-1ubuntu8)".
pub fn systemd_version(text: &str) -> Option<u32> {
    text.lines().next()?.split_whitespace().nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn valid_key(k: &str) -> bool {
    (1..=160).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn unit_name(key: &str) -> Option<String> {
    valid_key(key).then(|| format!("{UNIT_PREFIX}{key}.service"))
}

pub fn valid_unit(u: &str) -> bool {
    u.strip_prefix(UNIT_PREFIX).and_then(|r| r.strip_suffix(".service")).is_some_and(valid_key)
}

/// The slot a job key belongs to (V48): exactly `t<1-7>-<16 lowercase
/// hex>` is that tenant's (`node::job_key`); anything else is the
/// owner's. An owner key always ends in `-g<digits>` and `g` isn't hex,
/// so it can never read as a tenant's.
pub fn key_slot(key: &str) -> Slot {
    let b = key.as_bytes();
    if b.len() == 3 + 16 && b[0] == b't' && b[2] == b'-' && b[3..].iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        let s = b[1].wrapping_sub(b'0');
        if s != OWNER_SLOT && share::valid_slot(s) {
            return s;
        }
    }
    OWNER_SLOT
}

/// The slot of a job unit (`k2job-<key>.service`), when it is one.
pub fn unit_slot(unit: &str) -> Option<Slot> {
    unit.strip_prefix(UNIT_PREFIX).and_then(|r| r.strip_suffix(".service")).filter(|k| valid_key(k)).map(key_slot)
}

/// A tenant's job user and group names.
pub fn tenant_user_name(slot: Slot) -> String {
    format!("{}{slot}", share::TENANT_USER_PREFIX)
}
pub fn tenant_group_name(slot: Slot) -> String {
    format!("k2t-{slot}")
}

/// The slice a slot's units run in: today's `k2-jobs.slice` for the
/// owner; a child slice per tenant (`k2-jobs-t1.slice` is under
/// `k2-jobs.slice` by systemd's naming), with its `CPUWeight` and
/// `TasksMax` set by `tenant.prepare` (V75).
pub fn slice_name(slot: Slot) -> String {
    if slot == OWNER_SLOT {
        "k2-jobs.slice".into()
    } else {
        format!("k2-jobs-t{slot}.slice")
    }
}

/// What `tenant.prepare` / `tenant.wipe` run (the wall isn't offered
/// without them).
pub const TENANT_TOOLS: &[&str] = &["setfacl", "useradd", "groupadd", "userdel", "groupdel"];

/// `no-lan` (V62): every non-public range through which a job could reach
/// a machine on the LAN, the tailnet or a multicast group, and the IPv6
/// encodings of private IPv4 (6to4, NAT64; the fetch path's
/// `is_public_ip` treats them the same, review B3). Loopback stays. At
/// start the unit also gets every on-link prefix and address of this
/// machine ([`lan_deny_extra`]: a dual-stack LAN's global IPv6, a LAN on
/// public IPv4; review A M3).
pub const NO_LAN_DENY: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.0.0.0/24",
    "192.168.0.0/16",
    "198.18.0.0/15",
    "224.0.0.0/4",
    "240.0.0.0/4",
    "2002::/16",
    "64:ff9b::/96",
    "64:ff9b:1::/48",
    "fc00::/7",
    "fe80::/10",
    "fec0::/10",
    "ff00::/8",
];

/// `addr` with only its first `len` bits, as `addr/len`.
pub fn prefix_of(addr: std::net::IpAddr, len: u32) -> String {
    use std::net::IpAddr;
    match addr {
        IpAddr::V4(a) => {
            let len = len.min(32);
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            format!("{}/{len}", std::net::Ipv4Addr::from(u32::from(a) & mask))
        }
        IpAddr::V6(a) => {
            let len = len.min(128);
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            format!("{}/{len}", std::net::Ipv6Addr::from(u128::from(a) & mask))
        }
    }
}

/// This machine's own LAN for `no-lan`: every non-loopback interface's
/// on-link prefix and its own address (sorted, no duplicates).
pub fn lan_deny_extra() -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    for (addr, len) in interface_addrs() {
        if addr.is_loopback() || addr.is_unspecified() {
            continue;
        }
        out.insert(prefix_of(addr, len));
        out.insert(prefix_of(addr, if addr.is_ipv4() { 32 } else { 128 }));
    }
    out.into_iter().collect()
}

/// (address, prefix length) of every interface address (getifaddrs).
fn interface_addrs() -> Vec<(std::net::IpAddr, u32)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list we free below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: a node of the list getifaddrs returned.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() {
            continue;
        }
        // SAFETY: sockaddr pointers from getifaddrs, read per family.
        unsafe {
            match (*ifa.ifa_addr).sa_family as i32 {
                libc::AF_INET => {
                    let a = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let m = &*(ifa.ifa_netmask as *const libc::sockaddr_in);
                    let addr = std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
                    let len = u32::from_be(m.sin_addr.s_addr).count_ones();
                    out.push((std::net::IpAddr::V4(addr), len));
                }
                libc::AF_INET6 => {
                    let a = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    let m = &*(ifa.ifa_netmask as *const libc::sockaddr_in6);
                    let addr = std::net::Ipv6Addr::from(a.sin6_addr.s6_addr);
                    let len: u32 = m.sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum();
                    out.push((std::net::IpAddr::V6(addr), len));
                }
                _ => {}
            }
        }
    }
    // SAFETY: the list getifaddrs returned.
    unsafe { libc::freeifaddrs(head) };
    out
}

/// Everything root decides about one job unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPlan {
    pub unit: String,
    pub job_uid: u32,
    pub group_gid: u32,
    pub root: PathBuf,
    pub rw: Vec<PathBuf>,
    /// InaccessiblePaths (missing ones are skipped: `-` prefix).
    pub gone: Vec<PathBuf>,
    /// The job's effective network (the node's ask; for a tenant also its
    /// share's, the narrower wins).
    pub network: Network,
    pub mem_bytes: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub stop_grace_secs: u64,
    /// The installed `k2-node` (its `job-exec` shim).
    pub binary: PathBuf,
    pub slice: String,
    /// `ProtectProc=invisible` (systemd ≥ 247; required for tenants).
    pub protect_proc: bool,
    /// `PrivateIPC=yes` (systemd ≥ 248).
    pub private_ipc: bool,
    /// `no-lan` only: this machine's own on-link prefixes and addresses,
    /// denied beside [`NO_LAN_DENY`].
    pub lan_extra: Vec<String>,
}

/// The fixed template (argv[0] is `systemd-run`).
pub fn systemd_run_argv(p: &UnitPlan) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "systemd-run".into(),
        format!("--unit={}", p.unit),
        format!("--description=K2 compute job {}", p.unit.trim_end_matches(".service")),
        "--quiet".into(),
        "--pipe".into(),
        "--wait".into(),
        format!("--slice={}", p.slice),
        format!("--uid={}", p.job_uid),
        format!("--gid={}", p.group_gid),
        format!("--working-directory={}", p.root.display()),
    ];
    let mut prop = |s: String| {
        a.push("-p".into());
        a.push(s);
    };
    for s in [
        "RemainAfterExit=yes",
        "KillMode=control-group",
        "OOMPolicy=stop",
        "UMask=0002",
        "NoNewPrivileges=yes",
        "ProtectSystem=strict",
        "ProtectHome=yes",
        "PrivateDevices=yes",
        "ProtectKernelTunables=yes",
        "ProtectKernelModules=yes",
        "ProtectKernelLogs=yes",
        "ProtectControlGroups=yes",
        "ProtectClock=yes",
        "RestrictSUIDSGID=yes",
        "RestrictRealtime=yes",
        "LockPersonality=yes",
        "TasksMax=8192",
        "LimitNOFILE=65536",
        // The job's own `/dev/shm` (PrivateDevices keeps the host's): no
        // shared-memory name squatting between jobs or tenants.
        "TemporaryFileSystem=/dev/shm:mode=1777",
        // SysV/POSIX IPC objects of the job's user go when its last unit
        // ends (a reused slot never inherits them, V64).
        "RemoveIPC=yes",
    ] {
        prop(s.to_string());
    }
    if p.protect_proc {
        // Other users' processes and command lines are invisible (T4).
        // Not `ProcSubset=pid`: build tools read /proc/meminfo (V75).
        prop("ProtectProc=invisible".into());
    }
    if p.private_ipc {
        prop("PrivateIPC=yes".into());
    }
    prop(format!("TimeoutStopSec={}", p.stop_grace_secs.clamp(1, 600)));
    // The job's /tmp and /var/tmp are its own folders in its job root (not
    // PrivateTmp, whose /var/tmp/systemd-private-* sits outside the slot):
    // what a job writes there is counted in its server's disk use and
    // goes with the job (security review B, F1). Still the job's alone:
    // no other job's unit binds them. No `-`: a missing folder fails the
    // start instead of leaving the host's /tmp in place.
    prop(format!(
        "BindPaths={}:/tmp {}:/var/tmp",
        p.root.join(crate::runner::JOB_TMP).display(),
        p.root.join(crate::runner::JOB_VAR_TMP).display()
    ));
    let mut rw = vec![p.root.display().to_string()];
    rw.extend(p.rw.iter().map(|x| x.display().to_string()));
    prop(format!("ReadWritePaths={}", rw.join(" ")));
    let gone: Vec<String> = p.gone.iter().map(|x| format!("-{}", x.display())).collect();
    if !gone.is_empty() {
        prop(format!("InaccessiblePaths={}", gone.join(" ")));
    }
    if let Some(m) = p.mem_bytes.filter(|m| *m > 0) {
        prop(format!("MemoryMax={m}"));
        prop("MemorySwapMax=0".into());
    }
    if let Some(c) = p.cpu_millis.filter(|c| *c > 0) {
        // thousandths of a core → percent of one core (≥ 1%).
        prop(format!("CPUQuota={}%", (c / 10).max(1)));
    }
    match p.network {
        Network::None => {
            prop("PrivateNetwork=yes".into());
            prop("IPAddressDeny=any".into());
            // The resolver's local sockets would still answer from outside
            // the namespace (DNS as a side channel).
            prop("InaccessiblePaths=-/run/systemd/resolve -/run/nscd -/var/run/nscd".into());
        }
        Network::NoLan => {
            let mut deny: Vec<String> = NO_LAN_DENY.iter().map(|s| s.to_string()).collect();
            deny.extend(p.lan_extra.iter().cloned());
            prop(format!("IPAddressDeny={}", deny.join(" ")));
        }
        Network::On => {}
    }
    a.push("--".into());
    a.push(p.binary.display().to_string());
    a.push("job-exec".into());
    a.push(p.root.join(crate::wall::EXEC_SPEC).display().to_string());
    a
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

fn canon(p: &Path) -> Result<PathBuf, AdminRefusal> {
    std::fs::canonicalize(p).map_err(|e| refuse(code::BAD_ARGS, format!("{}: {e}", p.display())))
}

/// `child` strictly inside `parent` (both canonical).
fn inside(child: &Path, parent: &Path) -> bool {
    child != parent && child.starts_with(parent)
}

/// Where one slot's job-facing dirs are, as root sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRoots {
    pub jobs: PathBuf,
    pub cache: PathBuf,
    /// The slot's `tmp/` (its sccache socket, V63); may be missing for the
    /// owner (an older node), then nothing matches it.
    pub tmp: Option<PathBuf>,
}

/// One slot's roots, canonical: the node home's for the owner;
/// `<home>-t/<slot>/` for a tenant, which must be exactly that directory
/// (no link anywhere on the way: the parent is root's, 0711).
pub fn slot_roots(home: &Path, slot: Slot) -> Result<SlotRoots, AdminRefusal> {
    let layout = crate::paths::Layout::new(home, "/nonexistent");
    if slot == OWNER_SLOT {
        let tmp = std::fs::canonicalize(layout.tenant(OWNER_SLOT).tmp()).ok();
        return Ok(SlotRoots { jobs: canon(&home.join("jobs"))?, cache: canon(&home.join("cache"))?, tmp });
    }
    if !share::valid_slot(slot) {
        return Err(refuse(code::BAD_ARGS, format!("slot {slot} is out of range")));
    }
    let parent = canon(&layout.tenants_root())?;
    let base = canon(&layout.tenant(slot).root())?;
    if base != parent.join(slot.to_string()) {
        return Err(refuse(code::BAD_ARGS, "a tenant root is a plain folder under the tenants root"));
    }
    Ok(SlotRoots { jobs: canon(&base.join("jobs"))?, cache: canon(&base.join("cache"))?, tmp: Some(canon(&base.join("tmp"))?) })
}

/// The job root and writable dirs, resolved and confined to the slot's
/// `jobs/<one dir>`, `cache/…` and `tmp/` (the owner's under the node
/// home, a tenant's under `<home>-t/<slot>/`, V54).
pub fn check_paths(home: &Path, slot: Slot, root: &Path, rw: &[PathBuf]) -> Result<(PathBuf, Vec<PathBuf>), AdminRefusal> {
    let s = slot_roots(home, slot)?;
    let r = canon(root)?;
    if r.parent() != Some(s.jobs.as_path()) {
        return Err(refuse(code::BAD_ARGS, "a job root is one folder under its server's jobs/"));
    }
    let mut out = Vec::new();
    for p in rw {
        let c = canon(p)?;
        if !(inside(&c, &r) || c == r || inside(&c, &s.cache) || s.tmp.as_ref() == Some(&c)) {
            return Err(refuse(code::BAD_ARGS, format!("{} is outside the job's own folders", p.display())));
        }
        out.push(c);
    }
    Ok((r, out))
}

/// A path `reclaim` may take back for `slot`: a job root or anything under
/// one, or anything under `cache/` (not `cache/` itself), of that slot
/// only, with no link anywhere below the slot's `jobs/` or `cache/`
/// (security review A-disk 1: a link a job planted made root re-own what
/// it pointed at, another server's live tree). Returns the path, link
/// free (root then opens it one component at a time, never following).
pub fn check_reclaim(home: &Path, path: &Path, slot: Slot) -> Result<PathBuf, AdminRefusal> {
    let s = slot_roots(home, slot)?;
    let t = crate::paths::Layout::new(home, "/nonexistent").tenant(slot);
    let c = canon(path)?;
    let no_link = [(t.jobs(), &s.jobs), (t.cache(), &s.cache)].iter().any(|(given, real)| {
        let rel = path.strip_prefix(given).or_else(|_| path.strip_prefix(real));
        match rel {
            Ok(rel) if !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) => c == real.join(rel),
            _ => false,
        }
    });
    if no_link && (inside(&c, &s.jobs) || inside(&c, &s.cache)) {
        return Ok(c);
    }
    Err(refuse(code::BAD_ARGS, format!("only job folders and caches of server slot {slot}, with no link on the way")))
}

/// The folders bound over a job's /tmp and /var/tmp (`BindPaths`) must be
/// real directories in the checked job root, not links, made by the node
/// (its uid, or root's): never another path bound read-write over the
/// job's /tmp (security review C, L1).
pub fn check_bind_sources(root: &Path, node_uid: u32) -> Result<(), AdminRefusal> {
    use std::os::unix::fs::MetadataExt;
    for name in [crate::runner::JOB_TMP, crate::runner::JOB_VAR_TMP] {
        let p = root.join(name);
        let m = std::fs::symlink_metadata(&p).map_err(|e| refuse(code::BAD_ARGS, format!("{}: {e}", p.display())))?;
        if m.file_type().is_symlink() || !m.is_dir() {
            return Err(refuse(code::BAD_ARGS, format!("{} must be a plain folder (not a link)", p.display())));
        }
        if m.uid() != node_uid && m.uid() != 0 {
            return Err(refuse(code::BAD_ARGS, format!("{} must be the node's (uid {}, not {})", p.display(), node_uid, m.uid())));
        }
    }
    Ok(())
}

/// Root: [`check_reclaim`], then the chown walk (fd based, never
/// following a link, from the very path checked).
pub fn reclaim_checked(home: &Path, path: &Path, slot: Slot, uid: u32, gid: u32) -> Result<crate::wall::WalkStats, AdminRefusal> {
    let p = check_reclaim(home, path, slot)?;
    crate::wall::reclaim(&p, uid, gid).map_err(|e| refuse(code::FAILED, e))
}

fn find_program(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].iter().map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

pub(crate) fn systemctl(args: &[&str]) -> Result<String, String> {
    let prog = find_program("systemctl").ok_or("systemctl not found")?;
    let out = std::process::Command::new(&prog)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir("/")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("systemctl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// This machine's systemd version (0 when unknown), asked once.
pub fn systemd() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| systemctl(&["--version"]).ok().and_then(|t| systemd_version(&t)).unwrap_or(0))
}

/// Read one unit's status (root).
pub fn unit_status(unit: &str) -> Result<UnitStatus, String> {
    let text = systemctl(&["show", "--property=LoadState,ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,ControlGroup", unit])?;
    let mut s = parse_show(&text);
    if let Some(cg) = text.lines().find_map(|l| l.strip_prefix("ControlGroup=")).filter(|c| c.starts_with('/')) {
        if let Ok(ev) = std::fs::read_to_string(Path::new("/sys/fs/cgroup").join(cg.trim_start_matches('/')).join("memory.events")) {
            s.oom_kills = parse_oom_kills(&ev);
        }
    }
    Ok(s)
}

/// Units being started right now (two starts of one key never race).
fn starting() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Who a job of one slot runs as, as root decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobIds {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
}

/// `servers.toml` as root reads it (`None` = no file: solo mode).
pub fn read_servers(config_dir: &Path) -> Result<Option<share::ServersFile>, String> {
    let p = config_dir.join(share::SERVERS_FILE);
    match std::fs::read_to_string(&p) {
        Ok(s) => {
            let f: share::ServersFile = toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
            f.validate().map_err(|e| format!("{}: {e}", p.display()))?;
            Ok(Some(f))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

/// A tenant slot's `servers.toml` line, fail closed: no file, a broken
/// file or no line for the slot all refuse.
pub fn tenant_entry(config_dir: &Path, slot: Slot) -> Result<share::ServerEntry, AdminRefusal> {
    let f = read_servers(config_dir)
        .map_err(|e| refuse(share::codes::TENANT_WALL_MISSING, e))?
        .ok_or_else(|| refuse(share::codes::SERVER_NOT_APPROVED, "no servers.toml: this node serves only its owner"))?;
    f.entry(slot).cloned().ok_or_else(|| refuse(share::codes::SERVER_NOT_APPROVED, format!("no server in slot {slot}")))
}

impl Executor {
    pub(crate) fn node_user(&self) -> String {
        super::config::read(&self.paths.admin())
            .ok()
            .flatten()
            .map(|c| c.node_user)
            .unwrap_or_else(|| super::config::default_node_user().to_string())
    }

    /// The job user (jobs.toml) and its uid, or why there's no wall.
    pub fn job_user(&self) -> Result<(String, u32), AdminRefusal> {
        if !cfg!(target_os = "linux") {
            return Err(refuse(code::NOT_CONFIGURED, "the job user wall is Linux only (macOS uses the Seatbelt profile)"));
        }
        let f = read_jobs_file(&self.paths.config_dir)
            .map_err(|e| refuse(code::NOT_CONFIGURED, e))?
            .ok_or_else(|| refuse(code::NOT_CONFIGURED, "no job user set up (sudo k2-node setup creates k2job)"))?;
        let (uid, _) = check_job_user(&f.job_user, &self.node_user()).map_err(|e| refuse(code::NOT_CONFIGURED, e))?;
        let home = self.paths.home.display().to_string();
        if home.starts_with("/home/") || home.starts_with("/root/") || home.starts_with("/run/user/") {
            return Err(refuse(code::NOT_CONFIGURED, "the node home is under /home or /root, which the job unit hides"));
        }
        if find_program("systemd-run").is_none() {
            return Err(refuse(code::NOT_CONFIGURED, "systemd-run not found"));
        }
        let v = systemd();
        if v < MIN_SYSTEMD {
            return Err(refuse(code::NOT_CONFIGURED, format!("systemd {v} is older than {MIN_SYSTEMD} (the job unit's sandbox needs it)")));
        }
        Ok((f.job_user, uid))
    }

    /// Can this machine run tenant jobs at all? (`team_needs_tenant_wall`
    /// otherwise.) The owner's wall plus systemd ≥ 247.
    pub fn tenant_wall_ready(&self) -> Result<(), AdminRefusal> {
        self.job_user().map_err(|e| refuse(share::codes::TENANT_NEEDS_JOB_WALL, e.message))?;
        let v = systemd();
        if v < MIN_SYSTEMD_TENANT {
            return Err(refuse(
                share::codes::TEAM_NEEDS_TENANT_WALL,
                format!("systemd {v} is older than {MIN_SYSTEMD_TENANT}: tenant jobs need ProtectProc=invisible"),
            ));
        }
        for t in TENANT_TOOLS {
            if find_program(t).is_none() {
                return Err(refuse(share::codes::TEAM_NEEDS_TENANT_WALL, format!("{t} not found (tenant users and ACLs need it)")));
            }
        }
        self.wall_selftest().map_err(|e| refuse(share::codes::TEAM_NEEDS_TENANT_WALL, format!("the tenant wall's self-test failed: {e}")))
    }

    /// The wall's runtime check, as root, once per executor process (only
    /// a pass is remembered): a throwaway unit with the tenant template's
    /// sandbox (`ProtectProc=invisible`, `PrivateIPC`, a private
    /// `/dev/shm`, the `no-lan` address filter, `ProtectSystem=strict`)
    /// starts as the owner's job user, and from inside it PID 1 (root's)
    /// is invisible and `/dev/shm` is its own. A kernel without hidepid
    /// (< 5.8), cgroup BPF or IPC namespaces, or a systemd that drops a
    /// property, fails it, and the node offers no `tenant-wall`.
    pub fn wall_selftest(&self) -> Result<(), String> {
        static PASSED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if PASSED.get().is_some() {
            return Ok(());
        }
        let ids = self.job_ids(OWNER_SLOT).map_err(|e| e.message)?;
        let prog = find_program("systemd-run").ok_or("systemd-run not found")?;
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unit = format!("{UNIT_PREFIX}selftest-{}-{}.service", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let mut a: Vec<String> = vec![
            format!("--unit={unit}"),
            "--quiet".into(),
            "--wait".into(),
            "--collect".into(),
            "--pipe".into(),
            format!("--uid={}", ids.uid),
            format!("--gid={}", ids.gid),
        ];
        for p in [
            "NoNewPrivileges=yes".to_string(),
            "ProtectSystem=strict".into(),
            "PrivateTmp=yes".into(),
            "PrivateDevices=yes".into(),
            "ProtectProc=invisible".into(),
            "PrivateIPC=yes".into(),
            "TemporaryFileSystem=/dev/shm:mode=1777".into(),
            format!("IPAddressDeny={}", NO_LAN_DENY.join(" ")),
            "RuntimeMaxSec=20".into(),
        ] {
            a.push("-p".into());
            a.push(p);
        }
        a.extend(["--".into(), "/bin/sh".into(), "-c".into(), "test ! -e /proc/1/cmdline && test -z \"$(ls -A /dev/shm)\" && echo k2-wall-ok".into()]);
        let mut child = std::process::Command::new(prog)
            .args(&a)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("systemd-run: {e}"))?;
        let t0 = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if t0.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                let _ = systemctl(&["stop", &unit]);
                return Err("the self-test unit didn't finish within 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if status.success() && stdout.trim() == "k2-wall-ok" {
            let _ = PASSED.set(());
            return Ok(());
        }
        Err(format!("{status}: {} {}", stdout.trim(), String::from_utf8_lossy(&out.stderr).trim()))
    }

    /// Who slot `slot`'s jobs run as. The owner: `jobs.toml`'s job user
    /// with the node's group (today's wall). A tenant: its own user and
    /// group from `jobs.toml [tenants]`, checked against the machine's
    /// user database (name, uid and group all as `tenant.prepare` made
    /// them, never root, the node user or the owner's job user), and only
    /// while `servers.toml` says that server is usable. Anything off →
    /// `tenant_wall_missing`: the job waits, it never runs as another user.
    pub fn job_ids(&self, slot: Slot) -> Result<JobIds, AdminRefusal> {
        let (owner_user, owner_uid) = self.job_user()?;
        let node_user = self.node_user();
        let (node_uid, node_gid) = super::server::user_ids(&node_user).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        if node_gid == 0 {
            // Group 0 sees every process through `ProtectProc=invisible`
            // (procfs exempts its `gid=`, 0) and reads root-group files.
            return Err(refuse(code::NOT_CONFIGURED, format!("the node user {node_user}'s group is root's; jobs never run in group 0")));
        }
        if slot == OWNER_SLOT {
            return Ok(JobIds { user: owner_user, uid: owner_uid, gid: node_gid });
        }
        self.tenant_wall_ready()?;
        let missing = |m: String| refuse(share::codes::TENANT_WALL_MISSING, m);
        let entry = tenant_entry(&self.paths.config_dir, slot)?;
        if !entry.usable(self.now()) {
            return Err(refuse(share::codes::SERVER_NOT_APPROVED, format!("the server in slot {slot} is {}", entry.status(self.now()).as_str())));
        }
        let f = read_jobs_file(&self.paths.config_dir).map_err(missing)?.ok_or_else(|| missing("no jobs.toml".into()))?;
        let t = f.tenant(slot).ok_or_else(|| missing(format!("slot {slot} has no job user (tenant.prepare)")))?;
        if t.user != tenant_user_name(slot) {
            return Err(missing(format!("slot {slot}'s job user must be {}", tenant_user_name(slot))));
        }
        let (uid, pgid) = super::server::user_ids(&t.user).ok_or_else(|| missing(format!("{} is not a user on this machine", t.user)))?;
        let gid = group_id(&tenant_group_name(slot)).ok_or_else(|| missing(format!("group {} is missing", tenant_group_name(slot))))?;
        if uid != t.uid || gid != t.gid || pgid != gid {
            return Err(missing(format!("{} isn't the user tenant.prepare made (uid/gid changed)", t.user)));
        }
        if uid == 0 || gid == 0 || uid == node_uid || uid == owner_uid || gid == node_gid {
            return Err(missing(format!("{} shares an id with root, the node or the owner's jobs", t.user)));
        }
        Ok(JobIds { user: t.user.clone(), uid, gid })
    }

    /// What the job ops tell the node about tenants.
    pub fn tenant_wall(&self) -> TenantWall {
        let mut w = TenantWall { systemd: systemd(), ..Default::default() };
        match self.tenant_wall_ready() {
            Ok(()) => w.ready = true,
            Err(e) => w.why = Some(format!("{}: {}", e.code, e.message)),
        }
        if let Ok(Some(f)) = read_jobs_file(&self.paths.config_dir) {
            w.users = f.tenants.iter().map(|(k, v)| (k.clone(), v.user.clone())).collect();
        }
        w
    }

    /// The job ops (the server checked the peer: root or the node user).
    pub fn handle_job(&self, op: &JobOp, fds: Vec<OwnedFd>) -> JobAnswer {
        let r = match op {
            JobOp::Ready => self.job_user().map(|(u, _)| JobAnswer::Ready { job_user: u }),
            JobOp::Start(s) => self.start_job(s, fds).map(|unit| JobAnswer::Started { unit }),
            JobOp::Status { unit } => self.with_unit(unit, |u| unit_status(u).map(JobAnswer::Status)),
            JobOp::Stop { unit } => self.with_unit(unit, |u| systemctl(&["stop", "--no-block", u]).map(|_| JobAnswer::Done { detail: json!({"stopping": u}) })),
            // Never blocks (V58): a job that ignores SIGTERM holds only its
            // own unit until `TimeoutStopSec`; the node polls `status` and
            // releases again once it ended (`reset-failed` forgets it).
            JobOp::Release { unit } => self.with_unit(unit, |u| {
                let stopped = systemctl(&["stop", "--no-block", u]);
                let _ = systemctl(&["reset-failed", u]);
                Ok(JobAnswer::Done { detail: json!({"released": u, "stop": stopped.err()}) })
            }),
            JobOp::Reclaim { path, slot } => self.reclaim_path(path, *slot),
            JobOp::Tenants => Ok(JobAnswer::Tenants(self.tenant_wall())),
            JobOp::TenantPrepare(p) => self.tenant_prepare(p).map(|detail| JobAnswer::Done { detail }),
            JobOp::TenantWipe(w) => self.tenant_wipe(w).map(|detail| JobAnswer::Done { detail }),
        };
        r.unwrap_or_else(JobAnswer::Refused)
    }

    /// A K2 job unit (the slot is in its name, so an op can only ever
    /// reach the slot that unit belongs to).
    fn with_unit(&self, unit: &str, f: impl FnOnce(&str) -> Result<JobAnswer, String>) -> Result<JobAnswer, AdminRefusal> {
        if !valid_unit(unit) {
            return Err(refuse(code::BAD_ARGS, "not a K2 job unit"));
        }
        self.job_user()?;
        f(unit).map_err(|e| refuse(code::FAILED, e))
    }

    fn reclaim_path(&self, path: &Path, slot: Slot) -> Result<JobAnswer, AdminRefusal> {
        self.job_user()?;
        let (uid, node_gid) = super::server::user_ids(&self.node_user()).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        // A tenant's trees keep the tenant's group (its next job uses
        // them); the node gets them back as their owner.
        let gid = if slot == OWNER_SLOT {
            node_gid
        } else {
            group_id(&tenant_group_name(slot)).ok_or_else(|| refuse(share::codes::TENANT_WALL_MISSING, "the tenant's group is missing"))?
        };
        let s = reclaim_checked(&self.paths.home, path, slot, uid, gid)?;
        Ok(JobAnswer::Done { detail: json!({"reclaimed": path, "changed": s.changed, "skipped": s.skipped}) })
    }

    /// What a slot's unit may not see beyond the node's own state: the
    /// owner's never sees any tenant root; a tenant's sees no other
    /// tenant's root and nothing of the node home but `toolchains/`
    /// (read-only; the owner's jobs, mirrors, caches, logs and artifacts
    /// are gone, T1/V61).
    pub fn gone_for(&self, slot: Slot) -> Vec<PathBuf> {
        let home = &self.paths.home;
        let layout = crate::paths::Layout::new(home, &self.paths.config_dir);
        let mut gone = vec![home.join("state"), home.join("run"), self.paths.config_dir.clone(), self.paths.state_dir.clone()];
        let troot = layout.tenants_root();
        if slot == OWNER_SLOT {
            gone.push(troot);
            return gone;
        }
        if let Ok(rd) = std::fs::read_dir(home) {
            let mut names: Vec<_> = rd.flatten().map(|e| e.file_name()).filter(|n| n != "toolchains").collect();
            names.sort();
            for n in names {
                let p = home.join(n);
                if !gone.contains(&p) {
                    gone.push(p);
                }
            }
        }
        for other in 0..share::MAX_SLOTS {
            if other != slot && other != OWNER_SLOT {
                gone.push(troot.join(other.to_string()));
            }
        }
        gone
    }

    fn start_job(&self, s: &JobStart, fds: Vec<OwnedFd>) -> Result<String, AdminRefusal> {
        let ids = self.job_ids(s.tenant)?;
        if key_slot(&s.key) != s.tenant {
            return Err(refuse(code::BAD_ARGS, format!("job key {:?} isn't a slot-{} key", s.key, s.tenant)));
        }
        let unit = unit_name(&s.key).ok_or_else(|| refuse(code::BAD_ARGS, "job key is not a plain name"))?;
        let (root, rw) = check_paths(&self.paths.home, s.tenant, &s.root, &s.rw)?;
        let node_uid = super::server::user_ids(&self.node_user()).map(|(u, _)| u).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        check_bind_sources(&root, node_uid)?;
        let [out, err]: [OwnedFd; 2] = fds.try_into().map_err(|_| refuse(code::BAD_ARGS, "start needs exactly the job's stdout and stderr"))?;
        // The node's ask, narrowed by a tenant's share (V65): root never
        // widens what the node asked, and a tenant never gets more than
        // its line in servers.toml.
        let asked = if s.network_none {
            Network::None
        } else if s.no_lan {
            Network::NoLan
        } else {
            Network::On
        };
        let network = if s.tenant == OWNER_SLOT { asked } else { asked.min(tenant_entry(&self.paths.config_dir, s.tenant)?.network) };
        let v = systemd();
        if s.tenant != OWNER_SLOT && v < MIN_SYSTEMD_TENANT {
            return Err(refuse(share::codes::TENANT_WALL_MISSING, format!("systemd {v} is older than {MIN_SYSTEMD_TENANT}: no tenant jobs here")));
        }
        let plan = UnitPlan {
            unit: unit.clone(),
            job_uid: ids.uid,
            group_gid: ids.gid,
            root,
            rw,
            gone: self.gone_for(s.tenant),
            network,
            mem_bytes: s.mem_bytes,
            cpu_millis: s.cpu_millis,
            stop_grace_secs: s.stop_grace_secs,
            binary: self.paths.binary.clone(),
            slice: slice_name(s.tenant),
            protect_proc: v >= MIN_SYSTEMD_PROTECT_PROC,
            private_ipc: v >= MIN_SYSTEMD_PRIVATE_IPC,
            lan_extra: if network == Network::NoLan { lan_deny_extra() } else { Vec::new() },
        };
        {
            let mut g = starting().lock().unwrap_or_else(|e| e.into_inner());
            if !g.insert(unit.clone()) {
                return Err(refuse(code::BUSY, format!("{unit} is already starting")));
            }
        }
        let r = self.spawn_unit(&plan, &unit, out, err);
        starting().lock().unwrap_or_else(|e| e.into_inner()).remove(&unit);
        let _ = self.audit().append(
            super::store::AuditEntry {
                event: "job".into(),
                detail: json!({"unit": unit, "tenant": s.tenant, "network": network.as_str(), "ok": r.is_ok()}),
                ..Default::default()
            },
            self.now(),
        );
        r.map(|_| unit)
    }

    fn spawn_unit(&self, plan: &UnitPlan, unit: &str, out: OwnedFd, err: OwnedFd) -> Result<(), AdminRefusal> {
        // A leftover unit of the same name (a node restart) goes first, but
        // only once its command is over: a start never stops a running job
        // (V48). The name carries the slot, so it's never another slot's.
        if let Ok(st) = unit_status(unit) {
            if st.loaded {
                if !st.ended() {
                    return Err(refuse(code::FAILED, format!("{unit} is still running; it isn't started twice")));
                }
                let _ = systemctl(&["stop", unit]);
                let _ = systemctl(&["reset-failed", unit]);
            }
        }
        let argv = systemd_run_argv(plan);
        let prog = find_program("systemd-run").ok_or_else(|| refuse(code::NOT_CONFIGURED, "systemd-run not found"))?;
        let mut child = std::process::Command::new(prog)
            .args(&argv[1..])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| refuse(code::FAILED, format!("systemd-run: {e}")))?;
        // `--wait` keeps systemd-run until the unit is released; started
        // = the unit is loaded. A refused start ends systemd-run early.
        let t0 = Instant::now();
        let started = loop {
            if let Ok(Some(st)) = child.try_wait() {
                if !unit_status(unit).is_ok_and(|s| s.loaded) {
                    break Err(refuse(code::FAILED, format!("systemd-run ended before the job started ({st}); its message is in the job's log")));
                }
                break Ok(());
            }
            if unit_status(unit).is_ok_and(|s| s.loaded) {
                break Ok(());
            }
            if t0.elapsed() > Duration::from_secs(15) {
                let _ = child.kill();
                break Err(refuse(code::FAILED, "the job unit didn't appear within 15 s"));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        // Reap systemd-run whenever it ends (no zombies in the executor).
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        started
    }

    /// `wall.enable` (Linux; operate at `policy` since 0.46.4, owner-approved
    /// before): what `sudo k2-node setup` does for the wall, for a node
    /// installed without it: the job user (no login, no home, its own
    /// group), a traverse-only ACL on the node home, and `jobs.toml`.
    /// k2-node picks it up when it restarts. With `"restart": true` the
    /// executor restarts it here, but only while a restart is owed (this
    /// call changed the wall files, or an earlier one did and its restart
    /// hasn't happened: the root-only [`WALL_RESTART_PENDING_FILE`]), and
    /// always through a settling drain, even with nothing running: control
    /// says `draining`, every running job ends, one node status write
    /// passes, the count is still 0, then the restart. Never past the drain
    /// limit: there it gives up and says so, and the restart stays owed.
    /// Turning the wall on never kills a job (A-H1 / B-M1, round 4 item 1).
    ///
    /// The owner's own choice stands (A-L2 / B-L8): after `setup
    /// --no-job-wall`, or with a job user other than `k2job`, the executor
    /// classes this verb privileged ([`wall_needs_owner`]); `approved` is
    /// then true and clears the opt-out.
    pub fn wall_enable(&self, args: &Value, cmd_id: &str, approved: bool) -> Result<Value, AdminRefusal> {
        super::exec::only_keys(args, &["restart"])?;
        let restart = match args.get("restart") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(refuse(code::BAD_ARGS, "restart is true or false")),
        };
        if self.platform.os() != "linux" {
            return Err(refuse(code::BAD_ARGS, "macOS walls jobs with Seatbelt on its own; nothing to enable"));
        }
        let job_user = read_jobs_file(&self.paths.config_dir).ok().flatten().map(|f| f.job_user).unwrap_or_else(|| DEFAULT_JOB_USER.to_string());
        let setup = self.platform.enable_job_wall(&self.paths, &self.node_user(), &job_user)?;
        if approved {
            let _ = std::fs::remove_file(self.paths.state_dir.join(WALL_OPT_OUT_FILE));
        }
        let pending_path = self.paths.state_dir.join(WALL_RESTART_PENDING_FILE);
        if setup.changed() {
            // Owed until a restart happens, across retries and executor restarts.
            crate::util::atomic_write(&pending_path, cmd_id.as_bytes(), 0o600).map_err(|x| refuse(code::FAILED, format!("restart-pending flag: {x}")))?;
        }
        let pending = pending_path.exists();
        let mut out = json!({
            "job_user": job_user,
            "created": setup.created_user,
            "changed": setup.changed(),
            "home_acl": setup.home_acl,
            "jobs": setup.jobs_msg,
            "restart_needed": "k2-node",
        });
        // The node's own word (its status file) only decides whether a
        // restart is still needed, never whether the wall exists.
        let walled_now = crate::status::read(&self.paths.layout().status()).is_ok_and(|s| s.wall == "job_user");
        if walled_now {
            // The node's word (status.json is the node user's) never clears
            // root's owed-restart flag: only a restart this executor made
            // does (round 5, item 1). No restart from here.
            out["restart_needed"] = Value::Null;
            out["restart_pending"] = pending.into();
            out["note"] = "k2-node says it already runs jobs behind the wall".into();
            return Ok(out);
        }
        out["restart_pending"] = pending.into();
        if !restart {
            return Ok(out);
        }
        if !setup.changed() && !pending {
            // Nothing owed: a repeat call never restarts the node again (B-M1).
            out["note"] = "the wall's files were already in place, so this call doesn't restart k2-node; it takes effect at the next update or an owner-approved restart".into();
            return Ok(out);
        }
        // Always the settling drain, even with nothing running now.
        let (_, running) = super::update::decide(self, false, true);
        let cmd_id = cmd_id.to_string();
        let opts = super::update::DrainOpts { give_up_at_limit: true, settle: true };
        let r = super::update::run_decided(self, (super::update::Decision::Drain, running), opts, "wall.enable", &cmd_id.clone(), move |e| {
            // Control has said `draining` since before the last count, so
            // the node took nothing new; one more look all the same.
            let now_running = super::update::running_jobs(e);
            if now_running > 0 {
                return Err(refuse(code::BUSY, format!("{now_running} job(s) started meanwhile; k2-node wasn't restarted (ask again: the restart is still owed)")));
            }
            super::update::restart_marked(e, "wall.enable", &cmd_id, |e| e.platform.restart_unit("k2-node")).map_err(|x| refuse(code::FAILED, x))?;
            let _ = std::fs::remove_file(e.paths.state_dir.join(WALL_RESTART_PENDING_FILE));
            Ok(json!({"restarted": "k2-node"}))
        })?;
        out["restart"] = r;
        Ok(out)
    }
}

/// `<state>/wall-restart-pending`: `wall.enable` changed the wall's files
/// and k2-node hasn't restarted on them yet. Root-only; cleared by the
/// restart (or when the node already reports the wall).
pub const WALL_RESTART_PENDING_FILE: &str = "wall-restart-pending";

/// `<state>/job-wall-off`: the machine owner ran `setup --no-job-wall`.
/// Root-only (the admin state dir), so neither the owner's group nor its
/// agents can clear it; only an owner-approved `wall.enable` or a setup
/// without the flag does.
pub const WALL_OPT_OUT_FILE: &str = "job-wall-off";

/// `wall.enable` needs the owner's approval when the owner opted out of
/// the wall at the machine, or set a job user other than `k2job`.
pub fn wall_needs_owner(paths: &super::exec::ExecPaths) -> bool {
    paths.state_dir.join(WALL_OPT_OUT_FILE).exists()
        || read_jobs_file(&paths.config_dir).ok().flatten().is_some_and(|f| f.job_user != DEFAULT_JOB_USER)
}

/// What setting the wall up did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WallSetup {
    pub created_user: bool,
    /// `jobs.toml` was written with different content (or created).
    pub jobs_changed: bool,
    pub home_acl: String,
    pub jobs_msg: String,
}

impl WallSetup {
    /// Did this call change anything the node reads at start?
    pub fn changed(&self) -> bool {
        self.created_user || self.jobs_changed
    }
}

/// The root half of `wall.enable` on a real machine: the job user, the
/// home ACL, jobs.toml.
pub fn system_enable_job_wall(paths: &super::exec::ExecPaths, node_user: &str, user: &str) -> Result<WallSetup, AdminRefusal> {
    let fail = |e: String| refuse(code::FAILED, e);
    let mut created = false;
    if super::server::user_ids(user).is_none() {
        let nologin = ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("/bin/false");
        let useradd = find_program("useradd").ok_or_else(|| fail("useradd not found".into()))?;
        super::exec::run_fixed(
            &useradd.display().to_string(),
            &["--system", "--user-group", "--home-dir", "/nonexistent", "--no-create-home", "--shell", nologin, "--comment", "K2 compute jobs", user],
        )
        .map_err(fail)?;
        created = true;
    }
    let setfacl = find_program("setfacl").ok_or_else(|| refuse(code::NOT_CONFIGURED, "setfacl is missing: install the acl package, then try again"))?;
    let home = paths.home.display().to_string();
    super::exec::run_fixed(&setfacl.display().to_string(), &["-m", &format!("u:{user}:x"), &home]).map_err(fail)?;
    let before = read_jobs_file(&paths.config_dir).ok().flatten();
    let msg = write_jobs_file(&paths.config_dir, user, node_user).map_err(fail)?;
    let after = read_jobs_file(&paths.config_dir).ok().flatten();
    Ok(WallSetup { created_user: created, jobs_changed: before != after, home_acl: format!("u:{user}:x {home}"), jobs_msg: msg })
}

impl Executor {
    /// `homes.private` (owner-approved): chmod people's homes to owner-only.
    pub fn homes_private(&self, args: &Value) -> Result<Value, AdminRefusal> {
        super::exec::only_keys(args, &["homes"])?;
        let root = crate::wall::homes_root();
        let targets: Vec<PathBuf> = match args.get("homes") {
            None => crate::wall::open_homes_in(root).into_iter().map(|(p, _)| p).collect(),
            Some(v) => {
                let list = v.as_array().ok_or_else(|| refuse(code::BAD_ARGS, "homes is a list of paths"))?;
                let mut out = Vec::new();
                for h in list {
                    out.push(PathBuf::from(h.as_str().ok_or_else(|| refuse(code::BAD_ARGS, "homes is a list of paths"))?));
                }
                out
            }
        };
        let mut done = Vec::new();
        let mut errors = Vec::new();
        for h in &targets {
            match crate::wall::make_private(root, h) {
                Ok((was, now)) => done.push(json!({"home": h, "was": format!("{was:o}"), "now": format!("{now:o}")})),
                Err(e) => errors.push(json!({"home": h, "error": e})),
            }
        }
        if !errors.is_empty() && done.is_empty() {
            return Err(refuse(code::FAILED, format!("no home changed: {}", Value::Array(errors))));
        }
        Ok(json!({"made_private": done, "errors": errors}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> UnitPlan {
        UnitPlan {
            unit: "k2job-j1-g1.service".into(),
            job_uid: 990,
            group_gid: 991,
            root: PathBuf::from("/var/lib/k2node/jobs/j1-g1"),
            rw: vec![PathBuf::from("/var/lib/k2node/cache/pk/target-1")],
            gone: vec![PathBuf::from("/var/lib/k2node/state"), PathBuf::from("/etc/k2-node")],
            network: Network::On,
            mem_bytes: Some(2 << 30),
            cpu_millis: Some(4000),
            stop_grace_secs: 10,
            binary: PathBuf::from("/usr/local/libexec/k2-node"),
            slice: slice_name(0),
            protect_proc: false,
            private_ipc: false,
            lan_extra: Vec::new(),
        }
    }

    /// Review A M3 / B3: no-lan denies the IPv6 encodings of private IPv4,
    /// the benchmark and IETF ranges, and this machine's own on-link
    /// prefixes and addresses (a dual-stack LAN's global IPv6).
    #[test]
    fn no_lan_denies_the_whole_lan() {
        for r in ["2002::/16", "64:ff9b::/96", "198.18.0.0/15", "192.0.0.0/24", "10.0.0.0/8", "fc00::/7"] {
            assert!(NO_LAN_DENY.contains(&r), "{r}");
        }
        use std::net::IpAddr;
        assert_eq!(prefix_of("192.168.7.42".parse::<IpAddr>().unwrap(), 24), "192.168.7.0/24");
        assert_eq!(prefix_of("2a01:4f8:1:2:3:4:5:6".parse::<IpAddr>().unwrap(), 64), "2a01:4f8:1:2::/64");
        assert_eq!(prefix_of("203.0.113.9".parse::<IpAddr>().unwrap(), 32), "203.0.113.9/32");
        let extra = lan_deny_extra();
        assert!(extra.iter().all(|p| !p.starts_with("127.") && !p.starts_with("::1/")), "loopback stays: {extra:?}");
        let mut p = plan();
        p.network = Network::NoLan;
        p.lan_extra = vec!["2a01:4f8:1:2::/64".into(), "2a01:4f8:1:2::9/128".into()];
        let a = systemd_run_argv(&p);
        let deny = a.iter().find(|x| x.starts_with("IPAddressDeny=")).unwrap();
        assert!(deny.contains("2002::/16") && deny.contains("2a01:4f8:1:2::/64") && deny.contains("2a01:4f8:1:2::9/128"), "{deny}");
    }

    #[test]
    fn template_walls_the_job() {
        let a = systemd_run_argv(&plan());
        let has = |s: &str| a.iter().any(|x| x == s);
        assert_eq!(a[0], "systemd-run");
        for s in [
            "--unit=k2job-j1-g1.service",
            "--uid=990",
            "--gid=991",
            "--pipe",
            "--wait",
            "RemainAfterExit=yes",
            "ProtectHome=yes",
            "ProtectSystem=strict",
            "NoNewPrivileges=yes",
            "BindPaths=/var/lib/k2node/jobs/j1-g1/tmp:/tmp /var/lib/k2node/jobs/j1-g1/var-tmp:/var/tmp",
            "UMask=0002",
            "ProtectClock=yes",
            "MemoryMax=2147483648",
            "MemorySwapMax=0",
            "CPUQuota=400%",
            "TimeoutStopSec=10",
            "ReadWritePaths=/var/lib/k2node/jobs/j1-g1 /var/lib/k2node/cache/pk/target-1",
            "InaccessiblePaths=-/var/lib/k2node/state -/etc/k2-node",
        ] {
            assert!(has(s), "{s} missing from {a:?}");
        }
        assert!(!has("PrivateTmp=yes"), "tmp is the job's own folders (review B F1)");
        assert!(!has("PrivateNetwork=yes"), "network = full");
        assert!(has("--slice=k2-jobs.slice"), "the owner keeps today's slice");
        assert!(has("TemporaryFileSystem=/dev/shm:mode=1777") && has("RemoveIPC=yes"), "own /dev/shm, IPC gone with the user");
        assert!(!a.iter().any(|x| x.starts_with("IPAddressDeny")), "network = on adds no address rule");
        assert!(!has("ProtectProc=invisible") && !has("PrivateIPC=yes"), "only when systemd has them");
        let tail = &a[a.len() - 4..];
        assert_eq!(tail, ["--", "/usr/local/libexec/k2-node", "job-exec", "/var/lib/k2node/jobs/j1-g1/exec/exec.json"], "root's binary runs root's shim; argv never on the command line");
        let mut p = plan();
        p.network = Network::None;
        p.mem_bytes = None;
        p.cpu_millis = Some(5);
        let a = systemd_run_argv(&p);
        assert!(a.iter().any(|x| x == "PrivateNetwork=yes") && a.iter().any(|x| x == "IPAddressDeny=any"));
        assert!(a.iter().any(|x| x.contains("-/run/systemd/resolve")), "no DNS side channel");
        assert!(!a.iter().any(|x| x.starts_with("MemoryMax")));
        assert!(a.iter().any(|x| x == "CPUQuota=1%"), "never below 1%");
    }

    /// A tenant's unit (S3a): its own uid/gid and slice, other users'
    /// processes invisible, its own IPC, and `no-lan` closing every
    /// private, link-local, tailnet/CGNAT and multicast range (V62).
    #[test]
    fn a_tenant_unit_is_walled_from_the_lan_and_other_users() {
        let mut p = plan();
        p.unit = "k2job-t3-0123456789abcdef.service".into();
        p.job_uid = 61003;
        p.group_gid = 61003;
        p.slice = slice_name(3);
        p.protect_proc = true;
        p.private_ipc = true;
        p.network = Network::NoLan;
        let a = systemd_run_argv(&p);
        let has = |s: &str| a.iter().any(|x| x == s);
        for s in ["--uid=61003", "--gid=61003", "--slice=k2-jobs-t3.slice", "ProtectProc=invisible", "PrivateIPC=yes"] {
            assert!(has(s), "{s} missing from {a:?}");
        }
        assert!(!a.iter().any(|x| x.starts_with("ProcSubset")), "V75: /proc/meminfo stays readable");
        let deny = a.iter().find(|x| x.starts_with("IPAddressDeny=")).expect("no-lan denies addresses");
        for r in ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "100.64.0.0/10", "0.0.0.0/8", "224.0.0.0/4", "fc00::/7", "fe80::/10", "ff00::/8"] {
            assert!(deny.split([' ', '=']).any(|x| x == r), "{r} missing from {deny}");
        }
        assert!(!deny.contains("127.") && !deny.contains("::1"), "loopback stays: {deny}");
        assert!(!has("PrivateNetwork=yes"), "no-lan keeps the internet");
    }

    /// V48: the slot is in the key, and an owner key can never read as a
    /// tenant's (it always ends `-g<digits>`; `g` isn't hex).
    #[test]
    fn keys_and_units_carry_their_slot() {
        for slot in 1..8u8 {
            let k = crate::node::job_key(slot, "j", 1);
            assert_eq!(key_slot(&k), slot, "{k}");
            assert_eq!(unit_slot(&unit_name(&k).unwrap()), Some(slot));
        }
        for owner in ["j-g1", "t1-0123456789abcdef-g1", "t1-0123456789ABCDEF", "t0-0123456789abcdef", "t8-0123456789abcdef", "t1-0123456789abcde", "t1_0123456789abcdef"] {
            assert_eq!(key_slot(owner), 0, "{owner}");
        }
        assert_eq!(unit_slot("sshd.service"), None);
        assert_eq!((tenant_user_name(2), tenant_group_name(2), slice_name(2)), ("k2job-2".into(), "k2t-2".into(), "k2-jobs-t2.slice".into()));
    }

    #[test]
    fn show_parsing_and_endings() {
        let running = parse_show("LoadState=loaded\nActiveState=active\nSubState=running\nResult=success\nExecMainCode=0\nExecMainStatus=0\n");
        assert!(running.loaded && !running.ended() && running.exit().is_none());
        let done = parse_show("LoadState=loaded\nActiveState=active\nSubState=exited\nResult=success\nExecMainCode=1\nExecMainStatus=0\n");
        assert!(done.ended());
        assert_eq!(done.exit(), Some((Some(0), None)));
        let failed = parse_show("LoadState=loaded\nActiveState=failed\nSubState=failed\nResult=exit-code\nExecMainCode=1\nExecMainStatus=3\n");
        assert_eq!(failed.exit(), Some((Some(3), None)));
        let oom = parse_show("LoadState=loaded\nActiveState=failed\nSubState=failed\nResult=oom-kill\nExecMainCode=2\nExecMainStatus=9\n");
        assert!(oom.ended() && oom.oom());
        assert_eq!(oom.exit(), Some((None, Some(9))));
        // A started unit systemd no longer knows was stopped and collected
        // (found on Ubuntu 24.04, systemd 255): ended, with no exit to tell
        // and never a made-up "success" exit code.
        let gone = parse_show("LoadState=not-found\nActiveState=inactive\nSubState=dead\nResult=success\nExecMainCode=0\nExecMainStatus=0\n");
        assert!(gone.ended());
        assert_eq!(gone.exit(), None);
        assert!(!gone.oom());
        assert_eq!(parse_oom_kills("low 0\nhigh 0\nmax 12\noom 1\noom_kill 2\noom_group_kill 0\n"), 2);
        assert_eq!(parse_oom_kills(""), 0);
        assert_eq!(systemd_version("systemd 255 (255.4-1ubuntu8.18)\n+PAM +AUDIT"), Some(255));
        assert_eq!(systemd_version("systemd 257.2-3-arch (257.2-3-arch)"), Some(257));
        assert_eq!(systemd_version("nonsense"), None);
    }

    #[test]
    fn unit_names_are_plain() {
        assert_eq!(unit_name("abc-g1").as_deref(), Some("k2job-abc-g1.service"));
        for bad in ["", "a/b", "a b", "../x", "a.service", "a;b"] {
            assert!(unit_name(bad).is_none(), "{bad}");
        }
        assert!(valid_unit("k2job-abc-g1.service"));
        for bad in ["sshd.service", "k2job-.service", "k2job-a/b.service", "k2-node.service", "k2job-a.socket"] {
            assert!(!valid_unit(bad), "{bad}");
        }
    }

    #[test]
    fn paths_are_confined_after_resolving_links() {
        let home = crate::util::temp_dir("jobpaths").join("home");
        for d in ["jobs/j1-g1/src", "cache/pk/target-1", "state", "etc"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let home = std::fs::canonicalize(&home).unwrap();
        let (r, rw) = check_paths(&home, 0, &home.join("jobs/j1-g1"), &[home.join("jobs/j1-g1/src"), home.join("cache/pk/target-1")]).unwrap();
        assert_eq!(r, home.join("jobs/j1-g1"));
        assert_eq!(rw.len(), 2);
        // A symlink in a node-writable dir can't widen the job.
        std::os::unix::fs::symlink(home.join("state"), home.join("cache/pk/sneaky")).unwrap();
        std::os::unix::fs::symlink(home.join("etc"), home.join("jobs/j1-g1/out")).unwrap();
        for bad in [home.join("cache/pk/sneaky"), home.join("jobs/j1-g1/out"), home.join("state"), home.join("cache"), PathBuf::from("/etc")] {
            assert!(check_paths(&home, 0, &home.join("jobs/j1-g1"), std::slice::from_ref(&bad)).is_err(), "{}", bad.display());
        }
        assert!(check_paths(&home, 0, &home.join("jobs"), &[]).is_err(), "jobs/ itself isn't a job root");
        assert!(check_paths(&home, 0, &home.join("jobs/j1-g1/src"), &[]).is_err(), "one level under jobs/");
        assert!(check_paths(&home, 0, &home.join("state"), &[]).is_err());
        assert_eq!(check_reclaim(&home, &home.join("jobs/j1-g1"), 0).unwrap(), home.join("jobs/j1-g1"));
        assert_eq!(check_reclaim(&home, &home.join("cache/pk/target-1"), 0).unwrap(), home.join("cache/pk/target-1"));
        for bad in [home.join("jobs"), home.join("cache"), home.join("state"), home.join("cache/pk/sneaky"), home.join("jobs/j1-g1/out"), home.join("jobs/j1-g1/../j1-g1")] {
            assert!(check_reclaim(&home, &bad, 0).is_err(), "{}", bad.display());
        }
    }

    /// V54: a tenant's job root is under `<home>-t/<slot>/jobs/`, and
    /// never under the owner's or another tenant's; the root itself must
    /// be the real folder (a link planted in its place is refused).
    #[test]
    fn tenant_paths_are_confined_to_the_tenant_root() {
        let base = std::fs::canonicalize(crate::util::temp_dir("tenantpaths")).unwrap();
        let home = base.join("home");
        let l = crate::paths::Layout::new(&home, "/nonexistent");
        for d in [home.join("jobs/o-g1"), home.join("cache/pk")] {
            std::fs::create_dir_all(d).unwrap();
        }
        for slot in [1u8, 2] {
            let t = l.tenant(slot);
            for d in [t.jobs().join("k"), t.cache().join("pk/target-1"), t.tmp()] {
                std::fs::create_dir_all(d).unwrap();
            }
        }
        let t1 = l.tenant(1);
        let (r, rw) = check_paths(&home, 1, &t1.jobs().join("k"), &[t1.cache().join("pk/target-1"), t1.tmp()]).unwrap();
        assert_eq!((r, rw.len()), (t1.jobs().join("k"), 2));
        let t2 = l.tenant(2);
        for (root, rw) in [
            (t2.jobs().join("k"), vec![]),
            (home.join("jobs/o-g1"), vec![]),
            (t1.jobs().join("k"), vec![t2.cache().join("pk/target-1")]),
            (t1.jobs().join("k"), vec![home.join("cache/pk")]),
            (t1.jobs().join("k"), vec![t2.tmp()]),
            (t1.jobs().join("k"), vec![t1.cache()]),
        ] {
            assert!(check_paths(&home, 1, &root, &rw).is_err(), "{} {rw:?}", root.display());
        }
        assert!(check_paths(&home, 0, &t1.jobs().join("k"), &[]).is_err(), "the owner's job never runs in a tenant root");
        assert_eq!(check_reclaim(&home, &t1.jobs().join("k"), 1).unwrap(), t1.jobs().join("k"));
        assert_eq!(check_reclaim(&home, &t2.cache().join("pk/target-1"), 2).unwrap(), t2.cache().join("pk/target-1"));
        assert!(check_reclaim(&home, &t1.root(), 1).is_err() && check_reclaim(&home, &t1.repos(), 1).is_err());
        // Only the slot the caller names (review A-disk 1).
        assert!(check_reclaim(&home, &t2.cache().join("pk/target-1"), 1).is_err(), "slot 2's cache isn't slot 1's");
        assert!(check_reclaim(&home, &home.join("jobs/o-g1"), 1).is_err(), "nor the owner's");
        assert!(check_reclaim(&home, &t1.jobs().join("k"), 0).is_err(), "the owner's reclaim stays in its own home");
        // Slot 3's root is a link to slot 1's: refused.
        std::os::unix::fs::symlink(t1.root(), l.tenant(3).root()).unwrap();
        assert!(check_paths(&home, 3, &l.tenant(3).jobs().join("k"), &[]).is_err());
        assert!(slot_roots(&home, 3).is_err() && slot_roots(&home, 8).is_err());
    }

    /// Review A-disk 1: a link a job planted in its own job dir or cache,
    /// pointing at another server's live job dir, is refused (root would
    /// re-own what it points at), wherever on the path it sits.
    #[test]
    fn a_planted_link_to_another_servers_tree_is_refused() {
        let base = std::fs::canonicalize(crate::util::temp_dir("reclaimlink")).unwrap();
        let home = base.join("home");
        let l = crate::paths::Layout::new(&home, "/nonexistent");
        std::fs::create_dir_all(home.join("jobs")).unwrap();
        std::fs::create_dir_all(home.join("cache")).unwrap();
        for slot in [1u8, 2] {
            let t = l.tenant(slot);
            for d in [t.jobs().join("k-g1/home"), t.cache().join("pk/target-1"), t.tmp()] {
                std::fs::create_dir_all(d).unwrap();
            }
        }
        let (t1, t2) = (l.tenant(1), l.tenant(2));
        std::fs::write(t2.jobs().join("k-g1/home/secret"), "b").unwrap();
        std::os::unix::fs::symlink(t2.jobs().join("k-g1"), t1.jobs().join("k-g1/target")).unwrap();
        std::os::unix::fs::symlink(t2.cache().join("pk"), t1.cache().join("pk2")).unwrap();
        std::os::unix::fs::symlink(t2.jobs().join("k-g1"), t1.jobs().join("evil-g1")).unwrap();
        for bad in [t1.jobs().join("k-g1/target"), t1.jobs().join("k-g1/target/home"), t1.cache().join("pk2/target-1"), t1.jobs().join("evil-g1")] {
            let e = check_reclaim(&home, &bad, 1).unwrap_err();
            assert!(e.message.contains("no link"), "{}: {e:?}", bad.display());
            // Through root's whole path, too: nothing is chowned (the
            // walk is never reached; uid/gid are this user's own here).
            let me = unsafe { (libc::geteuid(), libc::getegid()) };
            assert!(reclaim_checked(&home, &bad, 1, me.0, me.1).is_err(), "{}", bad.display());
        }
        // The real trees stay reclaimable by their own slot.
        assert!(check_reclaim(&home, &t1.jobs().join("k-g1/home"), 1).is_ok());
        assert!(check_reclaim(&home, &t2.jobs().join("k-g1"), 2).is_ok());
    }

    /// Review C L1: what root binds over a job's /tmp and /var/tmp must be
    /// the job root's own plain folders, made by the node: a planted link
    /// (to another tenant's tree, or anywhere) or a file is refused.
    #[test]
    fn the_tmp_bind_sources_are_plain_folders_of_the_node() {
        use std::os::unix::fs::MetadataExt;
        let base = std::fs::canonicalize(crate::util::temp_dir("bindsrc")).unwrap();
        let me = std::fs::metadata(&base).unwrap().uid();
        let ok = base.join("ok-g1");
        let elsewhere = base.join("elsewhere");
        for d in [ok.join("tmp"), ok.join("var-tmp"), elsewhere.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        check_bind_sources(&ok, me).unwrap();
        let linked = base.join("linked-g1");
        std::fs::create_dir_all(linked.join("tmp")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, linked.join("var-tmp")).unwrap();
        let e = check_bind_sources(&linked, me).unwrap_err();
        assert!(e.message.contains("not a link"), "{e:?}");
        let file = base.join("file-g1");
        std::fs::create_dir_all(file.join("var-tmp")).unwrap();
        std::fs::write(file.join("tmp"), "x").unwrap();
        assert!(check_bind_sources(&file, me).is_err(), "a file isn't a folder");
        let missing = base.join("missing-g1");
        std::fs::create_dir_all(missing.join("tmp")).unwrap();
        assert!(check_bind_sources(&missing, me).is_err(), "a missing folder fails the start");
        if me != 0 {
            assert!(check_bind_sources(&ok, me + 1).is_err(), "another user's folders are refused");
        }
    }

    /// Review A-disk 1, as root (Linux container / CI root runs): a link
    /// planted in slot 1's job dir pointing at slot 2's job dir is
    /// refused, and slot 2's files keep their owner.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "root only: chowns"]
    fn as_root_a_planted_link_leaves_the_other_servers_owner_alone() {
        use std::os::unix::fs::chown;
        assert_eq!(unsafe { libc::geteuid() }, 0, "run as root (--include-ignored in the root container)");
        use std::os::unix::fs::MetadataExt;
        let base = std::fs::canonicalize(crate::util::temp_dir("reclaimroot")).unwrap();
        let home = base.join("home");
        let l = crate::paths::Layout::new(&home, "/nonexistent");
        std::fs::create_dir_all(home.join("jobs")).unwrap();
        std::fs::create_dir_all(home.join("cache")).unwrap();
        for slot in [1u8, 2] {
            let t = l.tenant(slot);
            for d in [t.jobs().join("k-g1/home"), t.cache(), t.tmp()] {
                std::fs::create_dir_all(d).unwrap();
            }
        }
        let (t1, t2) = (l.tenant(1), l.tenant(2));
        let secret = t2.jobs().join("k-g1/home/secret");
        std::fs::write(&secret, "b").unwrap();
        chown(&secret, Some(4242), Some(4242)).unwrap();
        chown(t2.jobs().join("k-g1/home"), Some(4242), Some(4242)).unwrap();
        std::os::unix::fs::symlink(t2.jobs().join("k-g1"), t1.jobs().join("k-g1/target")).unwrap();
        assert!(reclaim_checked(&home, &t1.jobs().join("k-g1/target"), 1, 4343, 4343).is_err());
        assert!(reclaim_checked(&home, &t2.jobs().join("k-g1"), 1, 4343, 4343).is_err(), "named slot 1, slot 2's tree");
        assert_eq!(std::fs::metadata(&secret).unwrap().uid(), 4242, "slot 2's file keeps its owner");
        // Control: slot 1's own tree is reclaimed (the link itself, not
        // what it points at).
        chown(t1.jobs().join("k-g1/home"), Some(4242), Some(4242)).unwrap();
        reclaim_checked(&home, &t1.jobs().join("k-g1"), 1, 4343, 4343).unwrap();
        assert_eq!(std::fs::metadata(t1.jobs().join("k-g1/home")).unwrap().uid(), 4343);
        assert_eq!(std::fs::metadata(&secret).unwrap().uid(), 4242, "still: the walk never follows the link");
    }

    #[test]
    fn job_user_is_never_root_or_the_node_user() {
        assert!(check_job_user("root", "k2node").is_err());
        assert!(check_job_user("no-such-user-k2x", "k2node").is_err());
        assert!(check_job_user("a b", "k2node").is_err());
        let me = crate::node::current_user();
        assert!(check_job_user(&me, &me).is_err(), "the node user can't be the job user");
    }

    #[test]
    fn jobs_file_round_trip_and_strict() {
        let d = crate::util::temp_dir("jobsfile");
        assert_eq!(read_jobs_file(&d).unwrap(), None);
        std::fs::write(d.join(JOBS_FILE), "job_user = \"k2job\"\n").unwrap();
        assert_eq!(read_jobs_file(&d).unwrap(), Some(JobsFile { job_user: "k2job".into(), tenants: Default::default() }));
        std::fs::write(d.join(JOBS_FILE), "job_user = \"k2job\"\nextra = 1\n").unwrap();
        assert!(read_jobs_file(&d).is_err(), "fail closed");
        // Tenant users by slot; the owner (slot 0) is always `job_user`.
        std::fs::write(
            d.join(JOBS_FILE),
            "job_user = \"k2job\"\n\n[tenants]\n1 = { user = \"k2job-1\", uid = 61001, gid = 61001 }\n",
        )
        .unwrap();
        let f = read_jobs_file(&d).unwrap().unwrap();
        assert_eq!(f.tenant(1), Some(&TenantUser { user: "k2job-1".into(), uid: 61001, gid: 61001 }));
        assert_eq!((f.tenant(0), f.tenant(2)), (None, None));
        let back: JobsFile = toml::from_str(&toml::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        std::fs::write(d.join(JOBS_FILE), "job_user = \"k2job\"\n[tenants]\n1 = { user = \"k2job-1\", uid = 1, gid = 1, shell = \"x\" }\n").unwrap();
        assert!(read_jobs_file(&d).is_err(), "tenant lines are strict too");
    }

    #[test]
    fn ops_round_trip_on_the_wire() {
        let ops = [
            JobOp::Ready,
            JobOp::Start(JobStart { key: "j-g1".into(), root: "/r".into(), rw: vec![], network_none: true, mem_bytes: Some(1), cpu_millis: None, stop_grace_secs: 10, tenant: 2, no_lan: true }),
            JobOp::Tenants,
            JobOp::TenantPrepare(share::TenantPrepare { slot: 1, trust: share::Trust::Own, weight: 1, network: Network::On }),
            JobOp::TenantWipe(share::TenantWipe { slot: 1 }),
            JobOp::Status { unit: "k2job-j-g1.service".into() },
            JobOp::Reclaim { path: "/r".into(), slot: 2 },
        ];
        for op in ops {
            let s = serde_json::to_string(&op).unwrap();
            assert_eq!(serde_json::from_str::<JobOp>(&s).unwrap(), op);
        }
        // A 0.46.2 node's reclaim (no slot) is the owner's.
        assert_eq!(serde_json::from_str::<JobOp>(r#"{"do":"reclaim","path":"/r"}"#).unwrap(), JobOp::Reclaim { path: "/r".into(), slot: OWNER_SLOT });
        // A 0.46.2 node's start (no tenant, no no_lan) is the owner's.
        let old: JobOp = serde_json::from_str(r#"{"do":"start","key":"j-g1","root":"/r","rw":[],"network_none":false,"mem_bytes":null,"cpu_millis":null,"stop_grace_secs":10}"#).unwrap();
        let JobOp::Start(s) = old else { panic!() };
        assert_eq!((s.tenant, s.no_lan), (0, false));
    }
}
