//! The executor's tenant ops (prd-compute-shared-queue-v1 §8.2, §11.1;
//! S3a on Linux, S3m on macOS): `tenant.prepare` and `tenant.wipe`.
//!
//! Root decides everything from root-owned files: a slot is prepared only
//! while `servers.toml` approves it (pending, active or paused) and wiped
//! only once it doesn't (revoked, expired or no line), so the node user
//! asking can't make a user for a server nobody approved, or wipe a live
//! one. Both are idempotent.
//!
//! **Linux** (a user per tenant):
//! - `k2job-<slot>` / group `k2t-<slot>`, a system user with no home and
//!   no shell, with a **new uid every time** (V64): uids come from
//!   `61001…64999` and the high-water mark lives in root's state
//!   (`tenant-ids.json`), written before the user is created, so a slot
//!   reused after a wipe never gets an old uid back.
//! - The tenant root `<home>-t/<slot>/` is the **node user's**, group
//!   `k2t-<slot>`, 2750 (V54); `jobs/`, `cache/`, `tmp/` 2770, `repos/`
//!   2750. The node user isn't in the group (no k2-node restart): an
//!   inherited ACL (`d:g:k2t-<slot>`) gives the tenant's group what the
//!   node creates there, and the node what the tenant's jobs create. The
//!   parent `<home>-t` is root's, 0711, so nothing can swap a slot's root.
//! - The node home gets `g:k2t-<slot>:x` (reach `toolchains/`); the
//!   owner's job-facing dirs lose their "other" bits, and the tenant's
//!   unit hides them anyway (`InaccessiblePaths`).
//! - The slot's slice gets `CPUWeight = 100 × weight`, `TasksMax=16384`
//!   (a runtime drop-in, V75; re-applied on every prepare).
//! - systemd ≥ 247 (`ProtectProc=invisible`) or `team_needs_tenant_wall`;
//!   the job wall itself or `tenant_needs_job_wall` (V59).
//!
//! **macOS 0.46.3** (one uid, S3m): only `own` servers
//! (`team_needs_tenant_wall`); the tenant root is the node user's, 0700,
//! and the per-tenant Seatbelt profile (`wall::seatbelt_tenant_profile`)
//! is the wall between servers.

use std::path::{Path, PathBuf};

use k2_node_proto::admin::{code, AdminRefusal};
use k2_node_proto::share::{self, codes, ServerStatus, Slot, TenantPrepare, TenantWipe, Trust, OWNER_SLOT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::exec::Executor;
use super::jobs::{self, TenantUser};

/// First tenant uid/gid (the PRD's example `61001`).
pub const TENANT_ID_FIRST: u32 = 61001;
/// Last one; past it `tenant.prepare` refuses (thousands of revokes).
pub const TENANT_ID_LAST: u32 = 64999;
/// Root's record of handed-out tenant ids (in the executor's state dir).
pub const IDS_FILE: &str = "tenant-ids.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantIds {
    /// The next id to try. Only ever grows.
    pub next: u32,
    /// The id handed to each slot's current user (written before the user
    /// is created, so a crash mid-prepare is finished, not repeated).
    #[serde(default)]
    pub slots: std::collections::BTreeMap<String, u32>,
}

fn ids_path(e: &Executor) -> PathBuf {
    e.paths.state_dir.join(IDS_FILE)
}

fn read_ids(e: &Executor) -> Result<TenantIds, String> {
    match std::fs::read(ids_path(e)) {
        Ok(b) => serde_json::from_slice(&b).map_err(|x| format!("{IDS_FILE}: {x}")),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok(TenantIds { next: TENANT_ID_FIRST, ..Default::default() }),
        Err(x) => Err(format!("read {IDS_FILE}: {x}")),
    }
}

fn write_ids(e: &Executor, ids: &TenantIds) -> Result<(), String> {
    let b = serde_json::to_vec_pretty(ids).map_err(|x| x.to_string())?;
    crate::util::atomic_write(&ids_path(e), &b, 0o600)
}

/// One tenant op at a time (they edit `jobs.toml`, `tenant-ids.json` and
/// the user database), across the executor's connection threads, `sudo
/// k2-node servers …` (its add and wipe call these too) and setup's
/// `jobs.toml` write. Beside `jobs.toml`.
pub const OPS_LOCK_FILE: &str = ".tenant-ops.lock";

fn ops_lock(e: &Executor) -> Result<crate::util::FileLock, AdminRefusal> {
    crate::util::FileLock::exclusive(&e.paths.config_dir.join(OPS_LOCK_FILE)).map_err(failed)
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

fn failed(m: impl Into<String>) -> AdminRefusal {
    refuse(code::FAILED, m)
}

fn uid_taken(id: u32) -> bool {
    // SAFETY: getpwuid/getgrgid read static buffers; only null-checked.
    unsafe { !libc::getpwuid(id).is_null() || !libc::getgrgid(id).is_null() }
}

fn tool(name: &str) -> Result<String, AdminRefusal> {
    ["/usr/sbin", "/usr/bin", "/sbin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
        .ok_or_else(|| refuse(code::NOT_CONFIGURED, format!("{name} not found (install it, then try again)")))
}

fn run(program: &str, args: &[&str]) -> Result<(), AdminRefusal> {
    super::exec::run_fixed(program, args).map_err(failed)
}

/// SIGKILL `pid` only if it is still `uid`'s: the pidfd pins the process,
/// then its uid is read again (Linux; no pidfd → nothing is sent).
#[cfg(target_os = "linux")]
fn kill_if_still(pid: i32, uid: u32) {
    // SAFETY: pidfd_open takes a pid and flags; the fd is closed below.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
    if fd < 0 {
        return;
    }
    let st = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let real = st.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|r| r.split_whitespace().next()?.parse::<u32>().ok());
    if real == Some(uid) {
        // SAFETY: a valid pidfd; no siginfo.
        unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, libc::SIGKILL, std::ptr::null::<libc::c_void>(), 0) };
    }
    // SAFETY: our own fd.
    unsafe { libc::close(fd) };
}

#[cfg(not(target_os = "linux"))]
fn kill_if_still(_pid: i32, _uid: u32) {}

/// Every process of `uid` (from /proc; Linux).
fn pids_of(uid: u32) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let Ok(st) = std::fs::read_to_string(e.path().join("status")) else { continue };
        let real = st.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|r| r.split_whitespace().next()?.parse::<u32>().ok());
        if real == Some(uid) {
            out.push(pid);
        }
    }
    out
}

impl Executor {
    /// `servers.toml`'s line for a slot the node asks about.
    fn approved_entry(&self, slot: Slot) -> Result<share::ServerEntry, AdminRefusal> {
        if slot == OWNER_SLOT || !share::valid_slot(slot) {
            return Err(refuse(code::BAD_ARGS, format!("slot {slot} isn't a tenant slot (1–7)")));
        }
        jobs::tenant_entry(&self.paths.config_dir, slot)
    }

    /// `tenant.prepare` (see the module doc). Audited.
    pub fn tenant_prepare(&self, p: &TenantPrepare) -> Result<Value, AdminRefusal> {
        let _g = ops_lock(self)?;
        let r = self.tenant_prepare_locked(p);
        self.audit_tenant_op("tenant.prepare", p.slot, &r);
        r
    }

    fn audit_tenant_op(&self, event: &str, slot: Slot, r: &Result<Value, AdminRefusal>) {
        let (code, detail) = match r {
            Ok(v) => (None, v.clone()),
            Err(e) => (Some(e.code.clone()), json!({"slot": slot, "message": e.message})),
        };
        let _ = self.audit().append(super::store::AuditEntry { event: event.into(), code, detail, ..Default::default() }, self.now());
    }

    fn tenant_prepare_locked(&self, p: &TenantPrepare) -> Result<Value, AdminRefusal> {
        let entry = self.approved_entry(p.slot)?;
        match entry.status(self.now()) {
            ServerStatus::Revoked => return Err(refuse(codes::SHARE_REVOKED, format!("slot {} is revoked; wipe it, then add the server again", p.slot))),
            // Past its end date (or a pending add past its 10 minutes):
            // nobody gets a user for it (review A L5).
            ServerStatus::Expired => return Err(refuse(codes::SHARE_EXPIRED, format!("slot {}'s share has ended", p.slot))),
            _ => {}
        }
        if entry.trust != p.trust || entry.network != p.network {
            return Err(refuse(code::BAD_ARGS, "trust or network differ from servers.toml (root reads servers.toml, not the ask)"));
        }
        let layout = self.paths.layout();
        let node_user = self.node_user();
        let (node_uid, node_gid) = super::server::user_ids(&node_user).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        // `<home>-t` sits in a root-only directory, so nobody but root can
        // swap it (review A L6).
        let troot = layout.tenants_root();
        let above = troot.parent().ok_or_else(|| refuse(code::NOT_CONFIGURED, "the tenants root has no parent"))?;
        crate::wall::open_root_owned_dir(above).map_err(|e| refuse(code::NOT_CONFIGURED, e))?;
        if cfg!(target_os = "macos") {
            if entry.trust == Trust::Team {
                return Err(refuse(codes::TEAM_NEEDS_TENANT_WALL, "Macs take only your own servers until per-tenant users (0.46.4)"));
            }
            if !crate::wall::seatbelt_available() {
                return Err(refuse(codes::TENANT_NEEDS_JOB_WALL, "sandbox-exec is missing: no Seatbelt wall"));
            }
            let root = layout.tenant(p.slot).root();
            crate::wall::secure_dir(&layout.tenants_root(), 0, 0, 0o711).map_err(failed)?;
            crate::wall::secure_dir(&root, node_uid, node_gid, 0o700).map_err(failed)?;
            for d in ["jobs", "cache", "repos", "tmp"] {
                crate::wall::secure_dir(&root.join(d), node_uid, node_gid, 0o700).map_err(failed)?;
            }
            return Ok(json!({"slot": p.slot, "root": root, "wall": "seatbelt", "user": node_user}));
        }
        self.tenant_wall_ready()?;
        let setfacl = tool("setfacl").map_err(|e| refuse(code::NOT_CONFIGURED, format!("{} (the acl package)", e.message)))?;
        let (user, adopted) = self.tenant_user(p.slot)?;
        let tgid = user.gid;
        // Dirs (V54): root's parent, the node's tenant root, its subdirs.
        let root = layout.tenant(p.slot).root();
        crate::wall::secure_dir(&layout.tenants_root(), 0, 0, 0o711).map_err(failed)?;
        crate::wall::secure_dir(&root, node_uid, tgid, 0o2750).map_err(failed)?;
        let nu = format!("u:{node_user}:rwx");
        let g = jobs::tenant_group_name(p.slot);
        for (d, mode, acl) in [
            ("jobs", 0o2770, format!("g:{g}:rwx,d:g:{g}:rwx,d:{nu},d:u::rwx,d:g::---,d:o::---,d:m::rwx")),
            ("cache", 0o2770, format!("g:{g}:rwx,d:g:{g}:rwx,d:{nu},d:u::rwx,d:g::---,d:o::---,d:m::rwx")),
            ("tmp", 0o2770, format!("g:{g}:rwx,d:g:{g}:rwx,d:{nu},d:u::rwx,d:g::---,d:o::---,d:m::rwx")),
            ("repos", 0o2750, format!("g:{g}:r-x,d:g:{g}:r-x,d:u::rwx,d:g::---,d:o::---,d:m::r-x")),
        ] {
            let path = root.join(d);
            crate::wall::secure_dir(&path, node_uid, tgid, mode).map_err(failed)?;
            run(&setfacl, &["-P", "-m", &acl, &path.display().to_string()])?;
        }
        // The node home: the tenant reaches toolchains/ (read-only) and
        // nothing else (its unit hides the rest; the "other" bits go too).
        let home = self.paths.home.display().to_string();
        run(&setfacl, &["-P", "-m", &format!("g:{g}:x"), &home])?;
        if let Ok(rd) = std::fs::read_dir(&self.paths.home) {
            for e in rd.flatten() {
                if e.file_name() != "toolchains" && e.file_type().is_ok_and(|t| t.is_dir()) {
                    let _ = crate::wall::drop_other_bits(&e.path());
                }
            }
        }
        // The slice (V75): weight and a task cap for the whole tenant.
        let weight = (entry.weight.max(1) as u64 * 100).min(10000);
        let slice = jobs::slice_name(p.slot);
        let systemctl = tool("systemctl")?;
        run(&systemctl, &["set-property", "--runtime", &slice, &format!("CPUWeight={weight}"), "TasksMax=16384"])?;
        Ok(json!({
            "slot": p.slot, "user": user.user, "uid": user.uid, "gid": user.gid, "adopted": adopted,
            "root": root, "slice": slice, "cpu_weight": weight, "wall": "job_user",
        }))
    }

    /// The slot's user: the one `jobs.toml` names when it's intact, a
    /// half-made one from a crashed prepare, or a new one with a new uid.
    fn tenant_user(&self, slot: Slot) -> Result<(TenantUser, bool), AdminRefusal> {
        let name = jobs::tenant_user_name(slot);
        let group = jobs::tenant_group_name(slot);
        let mut f = jobs::read_jobs_file(&self.paths.config_dir)
            .map_err(|e| refuse(code::NOT_CONFIGURED, e))?
            .ok_or_else(|| refuse(codes::TENANT_NEEDS_JOB_WALL, "no jobs.toml (the job wall)"))?;
        let have_user = super::server::user_ids(&name);
        let have_group = jobs::group_id(&group);
        if let Some(t) = f.tenant(slot) {
            if have_user == Some((t.uid, t.gid)) && have_group == Some(t.gid) {
                return Ok((t.clone(), true));
            }
            return Err(failed(format!("{name} no longer matches jobs.toml; tenant.wipe slot {slot} first")));
        }
        let mut ids = read_ids(self).map_err(failed)?;
        let id = match (have_user, have_group, ids.slots.get(&slot.to_string())) {
            // A prepare that died after making the user: finish it.
            (Some((u, gu)), Some(gg), Some(&want)) if u == want && gu == want && gg == want => want,
            (None, None, _) => {
                let mut id = ids.next.max(TENANT_ID_FIRST);
                while id <= TENANT_ID_LAST && uid_taken(id) {
                    id += 1;
                }
                if id > TENANT_ID_LAST {
                    return Err(failed("no tenant uids left (61001–64999)"));
                }
                // Recorded before the user exists: never handed out again.
                ids.next = id + 1;
                ids.slots.insert(slot.to_string(), id);
                write_ids(self, &ids).map_err(failed)?;
                let ids_s = id.to_string();
                run(&tool("groupadd")?, &["--system", "--gid", &ids_s, &group])?;
                let nologin = ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("/bin/false");
                let comment = format!("K2 compute jobs (server slot {slot})");
                run(
                    &tool("useradd")?,
                    &["--system", "--uid", &ids_s, "--gid", &group, "--home-dir", "/nonexistent", "--no-create-home", "--shell", nologin, "--comment", &comment, &name],
                )?;
                id
            }
            _ => return Err(failed(format!("a leftover {name} or {group} exists; tenant.wipe slot {slot} first"))),
        };
        let t = TenantUser { user: name, uid: id, gid: id };
        f.tenants.insert(slot.to_string(), t.clone());
        jobs::write_jobs(&self.paths.config_dir, &f).map_err(failed)?;
        Ok((t, false))
    }

    /// `tenant.wipe` (see the module doc; V64). Everything of the slot
    /// goes: its units and processes, its files (root, journals,
    /// artifacts, ledger, pin) and on Linux its user and group. The uid is
    /// never handed out again.
    pub fn tenant_wipe(&self, w: &TenantWipe) -> Result<Value, AdminRefusal> {
        let _g = ops_lock(self)?;
        let r = self.tenant_wipe_locked(w);
        self.audit_tenant_op("tenant.wipe", w.slot, &r);
        r
    }

    fn tenant_wipe_locked(&self, w: &TenantWipe) -> Result<Value, AdminRefusal> {
        if w.slot == OWNER_SLOT || !share::valid_slot(w.slot) {
            return Err(refuse(code::BAD_ARGS, format!("slot {} isn't a tenant slot (1–7)", w.slot)));
        }
        // Only what the owner decided: a revoked or expired line (review A
        // M1). A slot with no line is never wiped on anyone's say.
        let servers = jobs::read_servers(&self.paths.config_dir).map_err(|e| refuse(codes::TENANT_WALL_MISSING, e))?;
        let Some(e) = servers.as_ref().and_then(|f| f.entry(w.slot)) else {
            return Err(refuse(code::BAD_ARGS, format!("slot {} has no line in servers.toml; only a revoked or expired server is wiped", w.slot)));
        };
        let st = e.status(self.now());
        if !matches!(st, ServerStatus::Revoked | ServerStatus::Expired) {
            return Err(refuse(code::BAD_ARGS, format!("slot {} is {}; only a revoked or expired server is wiped", w.slot, st.as_str())));
        }
        let layout = self.paths.layout();
        let mut removed: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        if cfg!(target_os = "linux") {
            // Units first: the slot is in their names.
            if let Ok(list) = jobs::systemctl(&["list-units", "--all", "--plain", "--no-legend", &format!("{}t{}-*", jobs::UNIT_PREFIX, w.slot)]) {
                for line in list.lines() {
                    let Some(u) = line.split_whitespace().next() else { continue };
                    if jobs::unit_slot(u) == Some(w.slot) {
                        let _ = jobs::systemctl(&["kill", "--signal=SIGKILL", u]);
                        let _ = jobs::systemctl(&["stop", u]);
                        let _ = jobs::systemctl(&["reset-failed", u]);
                        removed.push(u.to_string());
                    }
                }
            }
            // Everything left in the slot's slice, by cgroup (review A L4).
            let _ = jobs::systemctl(&["kill", "--signal=SIGKILL", &jobs::slice_name(w.slot)]);
            let f = jobs::read_jobs_file(&self.paths.config_dir).map_err(|e| refuse(code::NOT_CONFIGURED, e))?;
            let name = jobs::tenant_user_name(w.slot);
            let uid = f.as_ref().and_then(|f| f.tenant(w.slot)).map(|t| t.uid).or_else(|| super::server::user_ids(&name).map(|(u, _)| u));
            if let Some(uid) = uid.filter(|u| *u >= TENANT_ID_FIRST && *u <= TENANT_ID_LAST) {
                // Anything of its uid outside the slice: signalled through a
                // pidfd taken first and checked after, so a reused pid is
                // never hit.
                for _ in 0..20 {
                    let pids = pids_of(uid);
                    if pids.is_empty() {
                        break;
                    }
                    for p in pids {
                        kill_if_still(p, uid);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                // Leftovers outside the tenant root (V64).
                for dir in ["/dev/shm", "/tmp", "/var/tmp"] {
                    match crate::wall::remove_owned_by(Path::new(dir), uid) {
                        Ok(n) => removed.extend(n),
                        Err(e) => errors.push(e),
                    }
                }
            }
        }
        // Files: root removes only the slot's root (job dirs, mirrors,
        // caches) under its own `<home>-t`, through fds (review A M1). The
        // node-only state (journals, artifacts, ledger, pin) is the node's
        // data in the node's dirs: the node forgets it once the line is
        // gone.
        let troot = layout.tenants_root();
        match crate::wall::open_root_owned_dir(&troot) {
            Ok(fd) => {
                let name = std::ffi::CString::new(w.slot.to_string()).expect("digits");
                match crate::wall::remove_tree_at(&fd, &name) {
                    Ok(()) => removed.push(layout.tenant(w.slot).root().display().to_string()),
                    Err(e) => errors.push(e),
                }
            }
            Err(_) if !troot.exists() => {}
            Err(e) => errors.push(e),
        }
        if cfg!(target_os = "linux") {
            let name = jobs::tenant_user_name(w.slot);
            let group = jobs::tenant_group_name(w.slot);
            if let Ok(setfacl) = tool("setfacl") {
                if jobs::group_id(&group).is_some() {
                    let _ = super::exec::run_fixed(&setfacl, &["-P", "-x", &format!("g:{group}"), &self.paths.home.display().to_string()]);
                }
            }
            if super::server::user_ids(&name).is_some() {
                match super::exec::run_fixed(&tool("userdel")?, &[&name]) {
                    Ok(()) => removed.push(format!("user {name}")),
                    Err(e) => errors.push(e),
                }
            }
            if jobs::group_id(&group).is_some() {
                match super::exec::run_fixed(&tool("groupdel")?, &[&group]) {
                    Ok(()) => removed.push(format!("group {group}")),
                    Err(e) => errors.push(e),
                }
            }
            if let Ok(Some(mut f)) = jobs::read_jobs_file(&self.paths.config_dir) {
                if f.tenants.remove(&w.slot.to_string()).is_some() {
                    jobs::write_jobs(&self.paths.config_dir, &f).map_err(failed)?;
                }
            }
            let _ = jobs::systemctl(&["revert", &jobs::slice_name(w.slot)]);
            let mut ids = read_ids(self).map_err(failed)?;
            if ids.slots.remove(&w.slot.to_string()).is_some() {
                write_ids(self, &ids).map_err(failed)?;
            }
        }
        if !errors.is_empty() {
            return Err(failed(format!("slot {} only partly wiped: {}", w.slot, errors.join("; "))));
        }
        Ok(json!({"slot": w.slot, "removed": removed}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_file_round_trips_and_starts_at_the_first_id() {
        let d = crate::util::temp_dir("tenant-ids");
        let e = Executor::new(
            super::super::exec::ExecPaths { config_dir: d.join("cfg"), state_dir: d.join("state"), home: d.join("home"), binary: d.join("bin") },
            std::sync::Arc::new(super::super::exec::SystemPlatform),
        );
        assert_eq!(read_ids(&e).unwrap(), TenantIds { next: TENANT_ID_FIRST, slots: Default::default() });
        std::fs::create_dir_all(d.join("state")).unwrap();
        let ids = TenantIds { next: 61005, slots: [("2".to_string(), 61004)].into() };
        write_ids(&e, &ids).unwrap();
        assert_eq!(read_ids(&e).unwrap(), ids);
    }

    /// Root reads servers.toml, not the node's ask: no file, the owner's
    /// slot, or a slot with no line are all refused before anything is
    /// made; a live slot is never wiped.
    #[test]
    fn prepare_and_wipe_follow_servers_toml() {
        let d = crate::util::temp_dir("tenant-gate");
        let cfg = d.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let e = Executor::new(
            super::super::exec::ExecPaths { config_dir: cfg.clone(), state_dir: d.join("state"), home: d.join("home"), binary: d.join("bin") },
            std::sync::Arc::new(super::super::exec::SystemPlatform),
        );
        let ask = |slot| TenantPrepare { slot, trust: Trust::Own, weight: 1, network: share::Network::On };
        assert_eq!(e.tenant_prepare(&ask(1)).unwrap_err().code, codes::SERVER_NOT_APPROVED, "no servers.toml");
        assert_eq!(e.tenant_prepare(&ask(0)).unwrap_err().code, code::BAD_ARGS);
        assert_eq!(e.tenant_wipe(&TenantWipe { slot: 0 }).unwrap_err().code, code::BAD_ARGS);
        let mut f = share::ServersFile::new(&"a".repeat(64), "owner.k2.dev", share::ShareLimits::owner(2, 8, 16, 100, share::Network::On), 8, 1000, "local:t");
        let mut b = f.servers[0].clone();
        b.slot = 1;
        b.label = "b.k2.dev".into();
        b.fp_prefix = "0123456789abcdef".into();
        b.set_limits(&share::ShareLimits::new_server(Trust::Own, false));
        f.servers.push(b);
        std::fs::write(cfg.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
        assert_eq!(e.tenant_prepare(&ask(2)).unwrap_err().code, codes::SERVER_NOT_APPROVED, "no line for slot 2");
        let mut team = ask(1);
        team.trust = Trust::Team;
        assert_eq!(e.tenant_prepare(&team).unwrap_err().code, code::BAD_ARGS, "the ask must match servers.toml");
        assert_eq!(e.tenant_wipe(&TenantWipe { slot: 1 }).unwrap_err().code, code::BAD_ARGS, "an active server is never wiped");
    }
}
