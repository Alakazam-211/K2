//! A throwaway user per secret job (protocol 9, slice C2;
//! prd-ci-on-compute-for-clients-v1 §4.3, vs-live V15/V16/S8), root side.
//!
//! A secret job from a protocol-9 server runs as a uid of its own, so no
//! other job (another workspace's, or its own workspace's) shares a uid
//! with it: none can read its `/proc/<pid>/environ`, ptrace it or enter
//! its job root. The uid comes from a **recycled pool per slot**:
//! [`JOB_UID_POOL`] users `k2job-<slot>-<n>`, made the first time an index
//! is needed. The PRD's "a fresh uid per job, never reused" would spend the
//! node's 3,999 tenant ids in months (vs-live B10), so an index is reused,
//! but only once it is **proven empty** ([`prove_empty`]):
//! - the unit of its last job is no longer live;
//! - no process runs as its uid (leftovers are killed through a pidfd
//!   first; one that survives keeps the index out);
//! - it owns no file in its last job root, nor in the host's `/tmp`,
//!   `/var/tmp` and `/dev/shm` (its leftovers there are removed first, as
//!   `tenant.wipe` does).
//!
//! Why those places are enough: a pool job may write only its own job root
//! (the executor refuses any other read-write path for it, and binds its
//! `/tmp` and `/var/tmp` inside the root); its `/dev/shm` is the unit's
//! own. So after a job, its uid's files can only be in that root, and the
//! node deletes a secret job's root when it ends (`delete_secret_tree`,
//! which first gives the tree back to the node user). Each earlier root was
//! proven clean before the index was used again, and nothing runs as the
//! uid in between, so the last root is the only one to look at.
//!
//! The pool's uids come from the tenant id range (`tenant-ids.json`, the
//! same high-water mark as tenant users), so a wiped slot's pool never
//! comes back with an old uid (V64), and the K2 daemon's refusal of
//! loopback clients in that range covers them too. Its state is root's
//! `job-uid-pool.json` (the uid of each index, its last lease, why it is
//! held). Exclusive mode (`--exclusive`) stays the other way to run a
//! secret job alone, and the fallback where the pool can't be used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use k2_node_proto::admin::{code, AdminRefusal};
use k2_node_proto::share::{self, Slot, OWNER_SLOT};
use serde::{Deserialize, Serialize};

use super::exec::Executor;
use super::jobs::{self, JobIds};

/// Users per slot ([`k2_node_proto::share::JOB_UID_POOL`]).
pub const JOB_UID_POOL: u32 = share::JOB_UID_POOL;
/// Root's pool state, in the executor's state dir.
pub const POOL_FILE: &str = "job-uid-pool.json";
/// The host temp dirs a pool uid's leftovers are looked for (and removed).
pub const HOST_TEMP: &[&str] = &["/dev/shm", "/tmp", "/var/tmp"];
/// A proof walks at most this many entries; past it the index is held
/// (fail closed) until its root is gone.
pub const PROOF_MAX_ENTRIES: usize = 200_000;

/// `k2job-<slot>-<n>`.
pub fn pool_user_name(slot: Slot, index: u32) -> String {
    format!("{}{slot}-{index}", share::TENANT_USER_PREFIX)
}

fn pool_key(slot: Slot, index: u32) -> String {
    format!("{slot}-{index}")
}

/// One pool user as root keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolUser {
    pub uid: u32,
    /// The last job that ran as it: its unit and its job root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leased_at: Option<i64>,
    /// Why it wasn't proven empty the last time it was looked at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
}

/// [`POOL_FILE`]: pool users by `"<slot>-<index>"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolFile {
    #[serde(default)]
    pub users: BTreeMap<String, PoolUser>,
}

impl PoolFile {
    pub fn get(&self, slot: Slot, index: u32) -> Option<&PoolUser> {
        self.users.get(&pool_key(slot, index))
    }

    /// Every pool uid of `slot`, by index.
    pub fn slot_users(&self, slot: Slot) -> Vec<(u32, PoolUser)> {
        (0..JOB_UID_POOL).filter_map(|i| self.get(slot, i).map(|u| (i, u.clone()))).collect()
    }
}

/// What a proof looks at (the real one is [`SystemProbe`]; tests fake it).
pub trait Probe {
    /// The unit is loaded and its command hasn't ended (unknown = live).
    fn unit_live(&self, unit: &str) -> bool;
    /// Every process running as `uid`.
    fn pids(&self, uid: u32) -> Vec<i32>;
    /// SIGKILL `pid` if it still runs as `uid`.
    fn kill(&self, pid: i32, uid: u32);
    /// Remove what `uid` left at the top of the host temp dirs.
    fn clear_temp(&self, uid: u32);
    /// The first file or folder under `roots` (missing roots are skipped)
    /// owned by `uid`; `Err` when the walk couldn't finish.
    fn owned_file(&self, uid: u32, roots: &[PathBuf]) -> Result<Option<PathBuf>, String>;
}

/// May `u`'s uid run another job? `Err(why)` when it isn't proven empty.
pub fn prove_empty(u: &PoolUser, probe: &dyn Probe) -> Result<(), String> {
    if let Some(unit) = &u.unit {
        if probe.unit_live(unit) {
            return Err(format!("its last job's unit {unit} is still live"));
        }
    }
    let pids = probe.pids(u.uid);
    if !pids.is_empty() {
        for p in &pids {
            probe.kill(*p, u.uid);
        }
        let left = probe.pids(u.uid);
        if !left.is_empty() {
            return Err(format!("uid {} still runs process(es) {left:?}", u.uid));
        }
    }
    probe.clear_temp(u.uid);
    let mut roots: Vec<PathBuf> = HOST_TEMP.iter().map(PathBuf::from).collect();
    if let Some(r) = &u.root {
        roots.insert(0, r.clone());
    }
    match probe.owned_file(u.uid, &roots) {
        Ok(None) => Ok(()),
        Ok(Some(p)) => Err(format!("uid {} still owns {}", u.uid, p.display())),
        Err(e) => Err(format!("uid {}: its files can't be proven gone ({e})", u.uid)),
    }
}

/// Which index a secret job gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// A pool user that exists and is proven empty.
    Reuse(u32),
    /// An index with no user yet (made now).
    Fresh(u32),
}

impl Choice {
    pub fn index(self) -> u32 {
        match self {
            Choice::Reuse(i) | Choice::Fresh(i) => i,
        }
    }
}

/// Pick an index of `slot`: the lowest pool user proven empty, else the
/// lowest index with no user yet. Every user looked at gets its `held`
/// reason updated (or cleared). `Err` lists why each one is held.
pub fn pick(f: &mut PoolFile, slot: Slot, probe: &dyn Probe) -> Result<Choice, String> {
    pick_skipping(f, slot, probe, &std::collections::BTreeSet::new())
}

/// Indexes leased whose unit doesn't exist yet (11-07 review M8.1):
/// between a lease and `systemd-run`, an index's last unit isn't live, no
/// process runs as it and its root may be empty, so every proof passes and
/// a second start would get the same uid. A lease stays here until its
/// start is over (the unit then exists and is live, or the start failed).
#[derive(Debug, Default)]
pub struct InFlight(std::collections::BTreeSet<(Slot, u32)>);

impl InFlight {
    /// [`pick`], never an index still being started; the one picked is
    /// held until [`Self::done`].
    pub fn take(&mut self, f: &mut PoolFile, slot: Slot, probe: &dyn Probe) -> Result<Choice, String> {
        let busy: std::collections::BTreeSet<u32> = self.0.iter().filter(|(s, _)| *s == slot).map(|(_, i)| *i).collect();
        let c = pick_skipping(f, slot, probe, &busy)?;
        self.0.insert((slot, c.index()));
        Ok(c)
    }

    /// The start of `index` of `slot` is over.
    pub fn done(&mut self, slot: Slot, index: u32) {
        self.0.remove(&(slot, index));
    }
}

/// The executor's (one process; its pool file is under the ops lock).
static IN_FLIGHT: std::sync::Mutex<InFlight> = std::sync::Mutex::new(InFlight(std::collections::BTreeSet::new()));

/// A per-job uid's start is over (`start_job`, whatever the outcome).
pub(crate) fn lease_started(slot: Slot, index: u32) {
    IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner()).done(slot, index);
}

fn pick_skipping(f: &mut PoolFile, slot: Slot, probe: &dyn Probe, busy: &std::collections::BTreeSet<u32>) -> Result<Choice, String> {
    let mut why = Vec::new();
    let mut fresh = None;
    for i in 0..JOB_UID_POOL {
        if busy.contains(&i) {
            why.push(format!("{}: its job is starting", pool_user_name(slot, i)));
            continue;
        }
        match f.users.get_mut(&pool_key(slot, i)) {
            None => {
                if fresh.is_none() {
                    fresh = Some(i);
                }
            }
            Some(u) => match prove_empty(u, probe) {
                Ok(()) => {
                    u.held = None;
                    return Ok(Choice::Reuse(i));
                }
                Err(e) => {
                    why.push(format!("{}: {e}", pool_user_name(slot, i)));
                    u.held = Some(e);
                }
            },
        }
    }
    fresh.map(Choice::Fresh).ok_or_else(|| format!("every per-job uid of slot {slot} is busy or not proven empty yet ({})", why.join("; ")))
}

/// Record that `index` of `slot` now runs `unit` in `root`.
pub fn lease(f: &mut PoolFile, slot: Slot, index: u32, uid: u32, unit: &str, root: &Path, now: i64) {
    f.users.insert(pool_key(slot, index), PoolUser { uid, unit: Some(unit.to_string()), root: Some(root.to_path_buf()), leased_at: Some(now), held: None });
}

pub fn read_pool(path: &Path) -> Result<PoolFile, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PoolFile::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

pub fn write_pool(path: &Path, f: &PoolFile) -> Result<(), String> {
    let b = serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?;
    crate::util::atomic_write(path, &b, 0o600)
}

/// The machine, as root sees it.
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn unit_live(&self, unit: &str) -> bool {
        match jobs::unit_status(unit) {
            Ok(s) => s.loaded && !s.ended(),
            Err(_) => true,
        }
    }

    fn pids(&self, uid: u32) -> Vec<i32> {
        super::tenants::pids_of(uid)
    }

    fn kill(&self, pid: i32, uid: u32) {
        super::tenants::kill_if_still(pid, uid);
        // Give it a moment to go before the recount.
        for _ in 0..20 {
            if !super::tenants::pids_of(uid).contains(&pid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn clear_temp(&self, uid: u32) {
        for d in HOST_TEMP {
            let _ = crate::wall::remove_owned_by(Path::new(d), uid);
        }
    }

    fn owned_file(&self, uid: u32, roots: &[PathBuf]) -> Result<Option<PathBuf>, String> {
        let mut seen = 0usize;
        for r in roots {
            if let Some(p) = owned_under(r, uid, &mut seen)? {
                return Ok(Some(p));
            }
        }
        Ok(None)
    }
}

/// The first entry under `root` (itself included) owned by `uid`, never
/// following a link; a missing root has none.
pub fn owned_under(root: &Path, uid: u32, seen: &mut usize) -> Result<Option<PathBuf>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", p.display())),
        };
        *seen += 1;
        if *seen > PROOF_MAX_ENTRIES {
            return Err(format!("over {PROOF_MAX_ENTRIES} entries to look at"));
        }
        if m.uid() == uid {
            return Ok(Some(p));
        }
        if m.is_dir() {
            let rd = match std::fs::read_dir(&p) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", p.display())),
            };
            for e in rd {
                stack.push(e.map_err(|e| format!("{}: {e}", p.display()))?.path());
            }
        }
    }
    Ok(None)
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

/// Hand a secret job's tree to its per-job uid (`uid:gid`, never through a
/// link) and close it to everyone but that uid and the node user
/// (`node_user`, a name or a numeric uid), who writes, reads and finally
/// deletes it. `setfacl` is the acl package's.
///
/// **Every entry, at every depth** (11-07 review H3): each dir made under
/// the slot's `jobs/` inherited `g:k2t-<slot>` and its default, so a file
/// the job writes deep in `target/` would be the slot group's to read the
/// moment it leaves the closed root (an artifact, a moved cache). So the
/// inherited defaults go everywhere, the slot group and others get nothing
/// on any entry (the group's mode bits included: `g::---`), and every dir
/// gets the closed default, so what the job makes later is closed too.
/// `-P`: links are never followed (a link itself has no ACL).
pub fn close_job_tree(setfacl: &Path, root: &Path, uid: u32, gid: u32, node_user: &str) -> Result<(), AdminRefusal> {
    crate::wall::reclaim(root, uid, gid).map_err(|e| refuse(code::FAILED, e))?;
    let n = node_user;
    let prog = setfacl.display().to_string();
    let r = root.display().to_string();
    let run = |args: &[&str]| super::exec::run_fixed(&prog, args).map_err(|e| refuse(code::FAILED, e));
    // Inherited defaults off every dir (a file has none; setfacl skips it
    // below the top).
    run(&["-R", "-P", "-k", &r])?;
    // The access ACL of every entry: the job's uid and the node user only
    // (`X`: execute on dirs and on what was already executable).
    let access = format!("u::rwX,g::---,o::---,u:{n}:rwX,m::rwX");
    run(&["-R", "-P", "--set", &access, &r])?;
    // The closed default on every dir.
    let default = format!("d:u::rwx,d:g::---,d:o::---,d:u:{n}:rwx,d:m::rwx");
    run(&["-R", "-P", "-m", &default, &r])
}

/// Node side (11-07 review H3, "create closed"): a secret job's root is made
/// by the node user under the slot's `jobs/`, whose default ACL hands the
/// slot group every dir made below it. Strip what the root inherited (its
/// named entries and its default) before anything else is made in it, so
/// nothing the node puts there (the checkout, `target/`) names the slot
/// group. The root stays the node user's alone until the executor hands it
/// to the per-job uid ([`close_job_tree`]).
pub fn make_closed(setfacl: &Path, root: &Path) -> Result<(), String> {
    let r = root.display().to_string();
    super::exec::run_fixed(&setfacl.display().to_string(), &["-P", "-b", "-k", &r])
}

/// Refusal code: no per-job uid can be given now (every pool user busy or
/// not proven empty, or the pool can't be set up). The job fails; it never
/// runs as a shared uid instead.
pub const JOB_UID_UNAVAILABLE: &str = "job_uid_unavailable";

/// Why there's no per-job uid pool while the machine owner hasn't turned
/// it on (0.46.10 ships it off).
pub const JOB_UID_POOL_OFF: &str =
    "the per-job uid pool is off on this machine (CI on compute preview): the machine owner turns it on with sudo k2-node admin job-uid on (admin.toml job_uid_pool = true)";

impl Executor {
    fn pool_path(&self) -> PathBuf {
        self.paths.state_dir.join(POOL_FILE)
    }

    /// Can secret jobs get a per-job uid here? Linux, the job wall,
    /// systemd ≥ 247 (`ProtectProc=invisible`: other uids' processes are
    /// hidden), and the user tools. The node asks through `Ready`.
    pub fn job_uid_ready(&self) -> Result<(), AdminRefusal> {
        // 0.46.10: the machine owner's opt-in, off by default (admin.toml
        // `job_uid_pool`). Off, `Ready` says no `job_uid`, the node offers
        // no `job-uid`, and every start asking for one is refused here.
        if !super::config::ci_opt_ins(&self.paths.config_dir).0 {
            return Err(refuse(code::NOT_CONFIGURED, JOB_UID_POOL_OFF));
        }
        if !cfg!(target_os = "linux") {
            return Err(refuse(code::NOT_CONFIGURED, "per-job uids are Linux only"));
        }
        self.job_user()?;
        let v = jobs::systemd();
        if v < jobs::MIN_SYSTEMD_PROTECT_PROC {
            return Err(refuse(code::NOT_CONFIGURED, format!("systemd {v} is older than {}: per-job uids need ProtectProc=invisible", jobs::MIN_SYSTEMD_PROTECT_PROC)));
        }
        for t in ["useradd", "userdel", "setfacl"] {
            if jobs::find_program_in_sbin(t).is_none() {
                return Err(refuse(code::NOT_CONFIGURED, format!("{t} not found (per-job uids need it)")));
            }
        }
        Ok(())
    }

    /// A per-job uid for `unit` in `root`, of `slot` (whose jobs run with
    /// `ids`' group): an index proven empty, or a new pool user. Recorded
    /// before it is returned. Under the tenant ops lock (it may add a user
    /// and spend a tenant id).
    pub(crate) fn job_uid_lease(&self, slot: Slot, ids: &JobIds, unit: &str, root: &Path) -> Result<(u32, u32), AdminRefusal> {
        self.job_uid_ready()?;
        let _g = super::tenants::ops_lock_for(self)?;
        let path = self.pool_path();
        // An unreadable pool file holds every index (fail closed).
        let mut f = read_pool(&path).map_err(|e| refuse(JOB_UID_UNAVAILABLE, e))?;
        // M8.1: never an index whose start is still under way.
        let choice = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner()).take(&mut f, slot, &SystemProbe);
        let picked = choice.as_ref().ok().copied();
        let r = match choice {
            Ok(Choice::Reuse(i)) => {
                let u = f.get(slot, i).cloned().expect("picked");
                self.check_pool_user(slot, i, u.uid, ids).map(|_| (i, u.uid))
            }
            Ok(Choice::Fresh(i)) => self.make_pool_user(&mut f, slot, i, ids).map(|uid| (i, uid)),
            Err(why) => Err(refuse(JOB_UID_UNAVAILABLE, why)),
        };
        if let Ok((i, uid)) = &r {
            lease(&mut f, slot, *i, *uid, unit, root, self.now());
        }
        // The held reasons are written either way (`status` shows them).
        let written = write_pool(&path, &f).map_err(|e| refuse(JOB_UID_UNAVAILABLE, e));
        // Picked but not given (refused, or not recorded): not in flight.
        if r.is_err() || written.is_err() {
            if let Some(c) = picked {
                lease_started(slot, c.index());
            }
        }
        written?;
        r
    }

    /// A pool user as `useradd` made it: its name, uid and group, a bare
    /// account, never an id of root, the node, the owner's jobs or the
    /// slot's own job user.
    fn check_pool_user(&self, slot: Slot, index: u32, uid: u32, ids: &JobIds) -> Result<(), AdminRefusal> {
        let name = pool_user_name(slot, index);
        let bad = |m: String| refuse(JOB_UID_UNAVAILABLE, m);
        let (u, g) = super::server::user_ids(&name).ok_or_else(|| bad(format!("{name} is gone (tenant.wipe, or by hand)")))?;
        if u != uid || g != ids.gid {
            return Err(bad(format!("{name} isn't the user the pool made (uid {u}/{uid}, group {g}/{})", ids.gid)));
        }
        let node_uid = super::server::user_ids(&self.node_user()).map(|(u, _)| u);
        let owner_uid = self.job_user().ok().map(|(_, u)| u);
        if uid == 0 || Some(uid) == node_uid || Some(uid) == owner_uid || uid == ids.uid || !(share::TENANT_UID_FIRST..=share::TENANT_UID_LAST).contains(&uid) {
            return Err(bad(format!("{name} shares an id with root, the node or another job user")));
        }
        jobs::check_job_account(&name, g).map_err(bad)
    }

    /// Make `k2job-<slot>-<index>`: a tenant id (recorded in
    /// `tenant-ids.json` and here before the user exists, so a crash never
    /// hands it out twice), the slot's job group, no home, no shell.
    fn make_pool_user(&self, f: &mut PoolFile, slot: Slot, index: u32, ids: &JobIds) -> Result<u32, AdminRefusal> {
        let name = pool_user_name(slot, index);
        if super::server::user_ids(&name).is_some() {
            return Err(refuse(JOB_UID_UNAVAILABLE, format!("a {name} the pool doesn't know exists; remove it (userdel {name})")));
        }
        let uid = super::tenants::next_tenant_id(self).map_err(|e| refuse(JOB_UID_UNAVAILABLE, e.message))?;
        f.users.insert(pool_key(slot, index), PoolUser { uid, held: Some("being made".into()), ..Default::default() });
        write_pool(&self.pool_path(), f).map_err(|e| refuse(JOB_UID_UNAVAILABLE, e))?;
        let nologin = ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("/bin/false");
        let comment = format!("K2 compute secret jobs (server slot {slot}, pool {index})");
        let useradd = jobs::find_program_in_sbin("useradd").ok_or_else(|| refuse(code::NOT_CONFIGURED, "useradd not found"))?;
        super::exec::run_fixed(
            &useradd.display().to_string(),
            &[
                "--system",
                "--uid",
                &uid.to_string(),
                "--gid",
                &ids.gid.to_string(),
                "--home-dir",
                "/nonexistent",
                "--no-create-home",
                "--shell",
                nologin,
                "--comment",
                &comment,
                &name,
            ],
        )
        .map_err(|e| refuse(JOB_UID_UNAVAILABLE, e))?;
        self.check_pool_user(slot, index, uid, ids)?;
        let _ = self.audit().append(
            super::store::AuditEntry { event: "job_uid.user".into(), detail: serde_json::json!({"slot": slot, "index": index, "uid": uid, "user": name}), ..Default::default() },
            self.now(),
        );
        Ok(uid)
    }

    /// Hand a job tree to its per-job uid before the unit starts: every
    /// entry chowned to `uid:gid` (never through a link), and the root
    /// itself closed to everyone but that uid and the node user (who
    /// writes, reads and finally deletes it), so no other job of the slot,
    /// which shares the group, can enter it.
    pub(crate) fn job_uid_take_tree(&self, root: &Path, uid: u32, gid: u32) -> Result<(), AdminRefusal> {
        let setfacl = jobs::find_program_in_sbin("setfacl").ok_or_else(|| refuse(code::NOT_CONFIGURED, "setfacl not found"))?;
        close_job_tree(&setfacl, root, uid, gid, &self.node_user())
    }

    /// `tenant.wipe` / `tenant.unuser`: every pool user of `slot` goes
    /// (after its processes; their uids stay spent).
    pub(crate) fn remove_pool_users(&self, slot: Slot, removed: &mut Vec<String>, errors: &mut Vec<String>) {
        if !cfg!(target_os = "linux") || slot == OWNER_SLOT {
            return;
        }
        let path = self.pool_path();
        let mut f = match read_pool(&path) {
            Ok(f) => f,
            Err(e) => {
                errors.push(e);
                return;
            }
        };
        let userdel = jobs::find_program_in_sbin("userdel").map(|p| p.display().to_string());
        for (i, u) in f.slot_users(slot) {
            for _ in 0..20 {
                let pids = super::tenants::pids_of(u.uid);
                if pids.is_empty() {
                    break;
                }
                for p in pids {
                    super::tenants::kill_if_still(p, u.uid);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            for d in HOST_TEMP {
                match crate::wall::remove_owned_by(Path::new(d), u.uid) {
                    Ok(n) => removed.extend(n),
                    Err(e) => errors.push(e),
                }
            }
            let name = pool_user_name(slot, i);
            if super::server::user_ids(&name).is_some() {
                match userdel.as_deref().map(|ud| super::exec::run_fixed(ud, &[&name])) {
                    Some(Ok(())) => removed.push(format!("user {name}")),
                    Some(Err(e)) => errors.push(e),
                    None => errors.push("userdel not found".into()),
                }
            }
            f.users.remove(&pool_key(slot, i));
        }
        if let Err(e) = write_pool(&path, &f) {
            errors.push(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A machine in a test: live units, processes by uid (some immortal),
    /// files by uid.
    #[derive(Default)]
    struct Fake {
        live: Vec<String>,
        procs: RefCell<BTreeMap<u32, Vec<i32>>>,
        immortal: Vec<i32>,
        files: BTreeMap<u32, PathBuf>,
        unprovable: Vec<u32>,
        cleared: RefCell<Vec<u32>>,
    }

    impl Probe for Fake {
        fn unit_live(&self, unit: &str) -> bool {
            self.live.iter().any(|u| u == unit)
        }
        fn pids(&self, uid: u32) -> Vec<i32> {
            self.procs.borrow().get(&uid).cloned().unwrap_or_default()
        }
        fn kill(&self, pid: i32, uid: u32) {
            if !self.immortal.contains(&pid) {
                if let Some(v) = self.procs.borrow_mut().get_mut(&uid) {
                    v.retain(|p| *p != pid);
                }
            }
        }
        fn clear_temp(&self, uid: u32) {
            self.cleared.borrow_mut().push(uid);
        }
        fn owned_file(&self, uid: u32, _roots: &[PathBuf]) -> Result<Option<PathBuf>, String> {
            if self.unprovable.contains(&uid) {
                return Err("too many".into());
            }
            Ok(self.files.get(&uid).cloned())
        }
    }

    fn used(f: &mut PoolFile, slot: Slot, i: u32, uid: u32) {
        lease(f, slot, i, uid, &format!("k2job-t{slot}-{i:016x}.service"), Path::new("/nonexistent/jobs/x"), 1);
    }

    #[test]
    fn a_fresh_pool_hands_out_index_0_then_recycles_it_once_proven_empty() {
        let mut f = PoolFile::default();
        let probe = Fake::default();
        assert_eq!(pick(&mut f, 1, &probe), Ok(Choice::Fresh(0)));
        used(&mut f, 1, 0, 61010);
        // Its job still runs: the next secret job gets a new index.
        let busy = Fake { live: vec!["k2job-t1-0000000000000000.service".into()], ..Default::default() };
        assert_eq!(pick(&mut f, 1, &busy), Ok(Choice::Fresh(1)));
        assert!(f.get(1, 0).unwrap().held.as_deref().unwrap().contains("still live"));
        // It ended, nothing of it is left: index 0 again (recycled, the
        // same uid), and the held reason is gone.
        assert_eq!(pick(&mut f, 1, &probe), Ok(Choice::Reuse(0)));
        assert_eq!(f.get(1, 0).unwrap().held, None);
        assert_eq!(f.get(1, 0).unwrap().uid, 61010);
        assert_eq!(probe.cleared.borrow().as_slice(), &[61010], "its temp leftovers went first");
        // Other slots are their own pools.
        assert_eq!(pick(&mut f, 2, &probe), Ok(Choice::Fresh(0)));
    }

    /// M8.1 (11-07 review): between a lease and its unit, an index passes
    /// every proof (no live unit, no process, an empty root); a second
    /// start in that window never gets the same uid.
    #[test]
    fn an_index_being_started_is_never_handed_out_twice() {
        let mut f = PoolFile::default();
        let probe = Fake::default();
        let mut fl = InFlight::default();
        assert_eq!(fl.take(&mut f, 1, &probe), Ok(Choice::Fresh(0)));
        used(&mut f, 1, 0, 61010);
        assert_eq!(pick(&mut f, 1, &probe), Ok(Choice::Reuse(0)), "control: in the window every proof passes");
        assert_eq!(fl.take(&mut f, 1, &probe), Ok(Choice::Fresh(1)), "the second start gets another uid");
        assert_eq!(fl.take(&mut f, 2, &probe), Ok(Choice::Fresh(0)), "another slot is its own pool");
        fl.done(1, 0);
        assert_eq!(fl.take(&mut f, 1, &probe), Ok(Choice::Reuse(0)), "once its start is over, the proofs decide again");
    }

    #[test]
    fn a_leftover_process_blocks_reuse_until_it_is_gone() {
        let mut f = PoolFile::default();
        used(&mut f, 1, 0, 61010);
        used(&mut f, 1, 1, 61011);
        // 61010 left a process that a kill doesn't end (stuck in D state);
        // 61011 left one that a kill ends.
        let probe = Fake { procs: RefCell::new(BTreeMap::from([(61010, vec![4242]), (61011, vec![4343])])), immortal: vec![4242], ..Default::default() };
        assert_eq!(pick(&mut f, 1, &probe), Ok(Choice::Reuse(1)), "61011's leftover was killed, so it is empty; 61010 is held");
        let held = f.get(1, 0).unwrap().held.clone().unwrap();
        assert!(held.contains("still runs process(es) [4242]"), "{held}");
        assert!(probe.pids(61011).is_empty());
        // While 61010's process lives it is never reused, even when it's the
        // only user left that could be.
        let only0 = Fake { procs: RefCell::new(BTreeMap::from([(61010, vec![4242])])), immortal: vec![4242], live: vec!["k2job-t1-0000000000000001.service".into()], ..Default::default() };
        assert_eq!(pick(&mut f, 1, &only0), Ok(Choice::Fresh(2)), "a new index, never the held one");
        // Gone: reusable again.
        assert_eq!(pick(&mut f, 1, &Fake::default()), Ok(Choice::Reuse(0)));
    }

    #[test]
    fn a_leftover_file_or_an_unfinished_proof_blocks_reuse() {
        let mut f = PoolFile::default();
        used(&mut f, 3, 0, 61020);
        let file = Fake { files: BTreeMap::from([(61020, PathBuf::from("/var/lib/k2node-t/3/jobs/x/target/secret.o"))]), ..Default::default() };
        assert_eq!(pick(&mut f, 3, &file), Ok(Choice::Fresh(1)));
        assert!(f.get(3, 0).unwrap().held.as_deref().unwrap().contains("still owns /var/lib/k2node-t/3/jobs/x/target/secret.o"));
        let unprovable = Fake { unprovable: vec![61020], ..Default::default() };
        assert_eq!(pick(&mut f, 3, &unprovable), Ok(Choice::Fresh(1)));
        assert!(f.get(3, 0).unwrap().held.as_deref().unwrap().contains("can't be proven gone"));
    }

    #[test]
    fn a_full_pool_refuses_never_shares() {
        let mut f = PoolFile::default();
        let mut live = Vec::new();
        for i in 0..JOB_UID_POOL {
            used(&mut f, 1, i, 61100 + i);
            live.push(format!("k2job-t1-{i:016x}.service"));
        }
        let e = pick(&mut f, 1, &Fake { live, ..Default::default() }).unwrap_err();
        assert!(e.contains("every per-job uid of slot 1 is busy"), "{e}");
        assert!(e.contains(&pool_user_name(1, 7)), "{e}");
    }

    #[test]
    fn the_pool_file_round_trips_and_refuses_unknown_keys() {
        let mut f = PoolFile::default();
        used(&mut f, 1, 0, 61010);
        let d = crate::util::temp_dir("pool-file");
        let p = d.join(POOL_FILE);
        assert_eq!(read_pool(&p).unwrap(), PoolFile::default(), "missing = empty");
        write_pool(&p, &f).unwrap();
        assert_eq!(read_pool(&p).unwrap(), f);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(&p, r#"{"users":{},"extra":1}"#).unwrap();
        assert!(read_pool(&p).is_err(), "unknown keys: refused (the executor then holds every index)");
        assert_eq!(pool_user_name(1, 3), "k2job-1-3");
        assert_ne!(pool_user_name(1, 0), jobs::tenant_user_name(1));
    }

    /// The slot group a test pretends `jobs/` belongs to (`nogroup`): any
    /// group works, the test user owns the files so it may name one.
    #[cfg(target_os = "linux")]
    const SLOT_GID: u32 = 65534;

    /// A slot's `jobs/` as `tenant.prepare` sets it (tenants.rs): the slot
    /// group may write, and every dir made under it inherits that.
    #[cfg(target_os = "linux")]
    fn slot_jobs_dir(tag: &str) -> PathBuf {
        let setfacl = jobs::find_program_in_sbin("setfacl").expect("setfacl (the acl package) must be installed: this test never skips");
        let d = crate::util::temp_dir(tag).join("jobs");
        std::fs::create_dir_all(&d).unwrap();
        let g = SLOT_GID;
        let acl = format!("g:{g}:rwx,d:g:{g}:rwx,d:u::rwx,d:g::---,d:o::---,d:m::rwx");
        super::super::exec::run_fixed(&setfacl.display().to_string(), &["-P", "-m", &acl, &d.display().to_string()]).unwrap();
        d
    }

    /// Every ACL line under `root` (numeric ids, no headers).
    #[cfg(target_os = "linux")]
    fn acls(root: &Path) -> String {
        let getfacl = jobs::find_program_in_sbin("getfacl").expect("getfacl (the acl package) must be installed: this test never skips");
        let out = std::process::Command::new(getfacl).args(["-R", "-P", "-p", "-n"]).arg(root).output().unwrap();
        assert!(out.status.success(), "getfacl: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    /// H3 (11-07 review): the node makes a secret job's dirs closed: none
    /// inherits the slot group's entries from `jobs/`, so nothing the node
    /// or the job later makes inside does either. Real ACLs on the test
    /// user's own files (no root needed).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_secret_jobs_dirs_are_made_closed() {
        let jobs_dir = slot_jobs_dir("pool-made-closed");
        let control = jobs_dir.join("plain");
        crate::runner::JobDirs::new(control.clone()).create_for(true).unwrap();
        assert!(acls(&control).contains(&format!("group:{SLOT_GID}:")), "control: a plain job's root inherits the slot group");
        let root = jobs_dir.join("secret");
        let dirs = crate::runner::JobDirs::new(root.clone());
        dirs.create_secret(true).unwrap();
        std::fs::create_dir_all(dirs.src.join("crate")).unwrap();
        let all = acls(&root);
        assert!(!all.contains(&format!("group:{SLOT_GID}:")), "a secret job's dirs name no slot group:\n{all}");
    }

    /// H3: closing the tree for the per-job uid strips every inherited
    /// entry at every depth, not only the root's: a file deep in `target/`
    /// keeps no `g:k2t-<slot>` (it is readable by the slot group again the
    /// moment it leaves the closed root: an artifact, a moved cache).
    #[cfg(target_os = "linux")]
    #[test]
    fn closing_a_secret_job_tree_strips_the_slot_group_at_every_depth() {
        use std::os::unix::fs::MetadataExt;
        let setfacl = jobs::find_program_in_sbin("setfacl").expect("setfacl (the acl package) must be installed: this test never skips");
        let jobs_dir = slot_jobs_dir("pool-close-deep");
        // A tree made the old way: every dir inherited the slot group.
        let root = jobs_dir.join("j1");
        crate::runner::JobDirs::new(root.clone()).create_for(true).unwrap();
        let deep = root.join("src/crate/target/release");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("out.bin"), "built with a secret").unwrap();
        assert!(acls(&deep).contains(&format!("group:{SLOT_GID}:")), "control: the deep dir inherited the slot group");
        let m = std::fs::metadata(&root).unwrap();
        close_job_tree(&setfacl, &root, m.uid(), m.gid(), &m.uid().to_string()).unwrap();
        let all = acls(&root);
        assert!(!all.contains(&format!("group:{SLOT_GID}:")), "no slot group left anywhere in the tree:\n{all}");
        // The root is still the node user's to write and delete.
        std::fs::write(root.join("log"), "x").unwrap();
    }

    #[test]
    fn owned_under_finds_a_file_never_follows_links_and_is_bounded() {
        use std::os::unix::fs::MetadataExt;
        let d = crate::util::temp_dir("pool-owned");
        let me = std::fs::metadata(&d).unwrap().uid();
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/b/f"), "x").unwrap();
        let mut seen = 0;
        assert!(owned_under(&d, me, &mut seen).unwrap().is_some(), "my own files are mine");
        let mut seen = 0;
        assert_eq!(owned_under(&d, me.wrapping_add(7), &mut seen).unwrap(), None);
        assert!(seen >= 4, "{seen}");
        let mut seen = 0;
        assert_eq!(owned_under(&d.join("missing"), me, &mut seen).unwrap(), None, "a missing root has none");
        // A link to elsewhere is the link, not its target.
        let other = crate::util::temp_dir("pool-owned-other");
        std::os::unix::fs::symlink(&other, d.join("link")).unwrap();
        let mut seen = PROOF_MAX_ENTRIES;
        assert!(owned_under(&d, me.wrapping_add(7), &mut seen).is_err(), "past the bound: not proven");
    }
}
