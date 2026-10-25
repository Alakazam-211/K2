//! `servers.owner` as root, for real (prd-macos-tenant-compute-v1 §10:
//! O1, O5, O6, O7): the root executor changes a node's owner server with
//! real tenant users and groups, real ACLs and real leftover processes.
//! The unit tests (`admin::owner::tests`) cover the same steps at the file
//! level without root; the paths that only run as root (the users step,
//! the re-own walk with its ACL strip, stopping leftover processes, the
//! old owner's new user, a revoke's wipe, an abort's re-own) run here.
//!
//! Needs root, systemd and the acl tools, and makes real users (`k2job-1`,
//! `k2t-1`, …, `k2nodet`, `k2ojt`), so every test is `#[ignore]` and runs
//! on purpose in a throwaway Linux box, as root, one at a time:
//!
//!   cargo test -p k2-node --test owner_change_linux -- --ignored --test-threads=1
//!
//! Each test makes its own dir under /var/lib and wipes its tenants
//! (users, groups, roots) when it ends. The executor runs in the test
//! process; its platform is the real one except for the node's restart,
//! which runs the node's half (`node::owner_swap::apply`) as a real
//! restart would. The crash tests run the change in a child process (this
//! same test binary) that exits hard (`_exit`) right after a step file is
//! written, then resume it here as the next executor start would.
//!
//! The node user is `k2nodet` (the test process takes its group, so the
//! files it makes carry it, as a real node's carry `k2node`'s); the
//! owner's job user is `k2ojt`; slot 1 is the target (one of the owner's
//! own servers), slot 2 another server that must stay untouched.

#![cfg(target_os = "linux")]

use std::ffi::CString;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_node::admin::exec::{ExecPaths, Executor, Platform, SystemPlatform};
use k2_node::admin::owner;
use k2_node::admin::server::user_ids;
use k2_node::identity::Pin;
use k2_node::paths::Layout;
use k2_node_proto::admin::code;
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::share::{self, Caches, Network, OldOwner, OwnerArgs, ServersFile, ShareLimits, ShareState, Slot, TenantPrepare, TenantWipe, Trust};

/// The exit code of a crash child that stopped where it was told to.
const CRASHED: i32 = 77;
const NODE_USER: &str = "k2nodet";
const JOB_USER: &str = "k2ojt";
const TARGET: &str = "server-1.test";

// ── the rig ──────────────────────────────────────────────────────────

/// A system user with its own group, made once in the throwaway box.
fn system_user(name: &str) -> (u32, u32) {
    if user_ids(name).is_none() {
        let st = Command::new("useradd")
            .args(["--system", "--user-group", "--no-create-home", "--home-dir", "/nonexistent", "--shell", "/usr/sbin/nologin", name])
            .status()
            .unwrap();
        assert!(st.success(), "useradd {name}");
    }
    user_ids(name).unwrap()
}

/// The node user; the process takes its group as its effective gid.
fn node_user() -> (u32, u32) {
    let (uid, gid) = system_user(NODE_USER);
    // SAFETY: glibc applies setegid to every thread of the process.
    assert_eq!(unsafe { libc::setegid(gid) }, 0, "setegid {gid}");
    (uid, gid)
}

/// The phase the step file says, if there is one.
fn step_phase(state_dir: &Path) -> Option<String> {
    let b = std::fs::read(state_dir.join(owner::STEP_FILE)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
    v["phase"].as_str().map(str::to_string)
}

/// The real platform, but the node's restart runs the node's half of the
/// change, and (in a crash child) the first clock read after the step file
/// names `crash_at` ends the process on the spot.
struct OwnerPlatform {
    layout: Layout,
    state_dir: PathBuf,
    crash_at: Option<String>,
}

impl Platform for OwnerPlatform {
    fn now(&self) -> i64 {
        if let Some(p) = &self.crash_at {
            if step_phase(&self.state_dir).as_deref() == Some(p.as_str()) {
                // SAFETY: ends the process at once, as a kill would: no
                // unwinding, no destructors, no flushing.
                unsafe { libc::_exit(CRASHED) };
            }
        }
        SystemPlatform.now()
    }
    fn os(&self) -> &'static str {
        SystemPlatform.os()
    }
    fn arch(&self) -> &'static str {
        SystemPlatform.arch()
    }
    fn restart_node(&self) -> Result<(), String> {
        Err("restart_node isn't part of an owner change".into())
    }
    fn restart_unit(&self, unit: &str) -> Result<(), String> {
        assert_eq!(unit, "k2-node", "an owner change restarts only k2-node");
        k2_node::node::owner_swap::apply(&self.layout, SystemPlatform.now()).map(|_| ())
    }
    fn node_pid(&self) -> Option<u32> {
        None
    }
    fn preflight(&self, _staged: &Path, _node_user: &str) -> Result<String, String> {
        Err("no update here".into())
    }
}

struct Rig {
    base: PathBuf,
    exec: Executor,
    owner: SigningKey,
    target: SigningKey,
    other: SigningKey,
    node: (u32, u32),
    job: (u32, u32),
}

fn paths(base: &Path) -> ExecPaths {
    ExecPaths { config_dir: base.join("config"), state_dir: base.join("admin-state"), home: base.join("home"), binary: base.join("bin/k2-node") }
}

fn executor(base: &Path, crash_at: Option<String>) -> Executor {
    let p = paths(base);
    let platform = OwnerPlatform { layout: Layout::new(&p.home, &p.config_dir), state_dir: p.state_dir.clone(), crash_at };
    let mut e = Executor::new(p, Arc::new(platform));
    e.drain_settle = Duration::from_millis(200);
    e.node_swap_wait = Duration::from_secs(20);
    e
}

fn pin(k: &SigningKey, url: &str) -> Pin {
    Pin {
        controller_fp: k.fingerprint(),
        controller_public_key_pem: k.spki_pem(),
        routes: vec![url.into()],
        node_id: "node-1".into(),
        name: "mini-1".into(),
        enrolled_at: 200,
        sas: "000 000".into(),
        revoked: false,
        revoked_reason: None,
        labels: Default::default(),
        confirmed: true,
    }
}

fn servers_file(owner_fp: &str, lines: &[(Slot, &SigningKey, ShareState)]) -> ServersFile {
    let mut f = ServersFile::new(owner_fp, "owner.test", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 100, "local:test");
    for (slot, k, state) in lines {
        let mut e = f.servers[0].clone();
        e.slot = *slot;
        e.label = format!("server-{slot}.test");
        e.fp_prefix = k.fingerprint()[..16].to_string();
        e.trust = Trust::Own;
        e.set_limits(&ShareLimits::new_server(Trust::Own, false));
        e.network = Network::On;
        e.state = *state;
        e.added_at = 100;
        f.servers.push(e);
    }
    f.validate().unwrap();
    f
}

fn write_servers(config: &Path, f: &ServersFile) {
    std::fs::write(config.join(share::SERVERS_FILE), toml::to_string(f).unwrap()).unwrap();
}

fn read_servers(config: &Path) -> ServersFile {
    toml::from_str(&std::fs::read_to_string(config.join(share::SERVERS_FILE)).unwrap()).unwrap()
}

fn chown(p: &Path, (u, g): (u32, u32)) {
    std::os::unix::fs::lchown(p, Some(u), Some(g)).unwrap();
}

fn setfacl(args: &[&str]) {
    let st = Command::new("setfacl").args(args).status().unwrap();
    assert!(st.success(), "setfacl {args:?}");
}

/// Does the entry carry a POSIX access ACL?
fn has_acl(p: &Path) -> bool {
    let c = CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: valid C strings; a size query only.
    unsafe { libc::lgetxattr(c.as_ptr(), c"system.posix_acl_access".as_ptr(), std::ptr::null_mut(), 0) >= 0 }
}

fn uid_exists(uid: u32) -> bool {
    // SAFETY: getpwuid reads the passwd database.
    !unsafe { libc::getpwuid(uid) }.is_null()
}

impl Rig {
    fn layout(&self) -> Layout {
        Layout::new(self.base.join("home"), self.base.join("config"))
    }
    fn config(&self) -> PathBuf {
        self.base.join("config")
    }
    fn state(&self) -> PathBuf {
        self.base.join("admin-state")
    }
    fn slot_user(&self, slot: Slot) -> Option<(u32, u32)> {
        user_ids(&format!("k2job-{slot}"))
    }
    fn slot_gid(&self, slot: Slot) -> Option<u32> {
        k2_node::admin::jobs::group_id(&k2_node::admin::jobs::tenant_group_name(slot))
    }
    fn args(&self, old: OldOwner) -> OwnerArgs {
        OwnerArgs { server: TARGET.into(), fp: Some(self.target.fingerprint()), old, caches: Caches::Move, now: false }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Every tenant revoked, then wiped: users, groups, roots.
        write_servers(&self.config(), &servers_file(&read_servers(&self.config()).owner, &[(1, &self.target, ShareState::Revoked), (2, &self.other, ShareState::Revoked)]));
        for slot in [1, 2] {
            if let Err(e) = self.exec.tenant_wipe(&TenantWipe { slot }) {
                eprintln!("cleanup: wipe slot {slot}: {}: {}", e.code, e.message);
            }
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// A node shared by its owner (slot 0), one of the owner's own servers
/// (slot 1, the target) and another server (slot 2): real tenant users
/// from the executor's own `tenant.prepare`, pins, root's key records,
/// both online, and a file in each server's trees (slot 1's with its
/// group's ACL, as its jobs leave them).
fn rig(tag: &str) -> Rig {
    assert_eq!(unsafe { libc::geteuid() }, 0, "the owner change root tests run as root (cargo test … -- --ignored)");
    let id = crypto::random_id().unwrap();
    let base = PathBuf::from(format!("/var/lib/k2-owner-test-{tag}-{}", &id[..8]));
    let p = paths(&base);
    for d in [&base, &p.config_dir, &p.state_dir, &p.home, &base.join("bin")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&p.state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let node = node_user();
    let job = system_user(JOB_USER);
    chown(&p.home, (0, node.1));
    std::fs::set_permissions(&p.home, std::fs::Permissions::from_mode(0o750)).unwrap();
    let layout = Layout::new(&p.home, &p.config_dir);
    layout.ensure().unwrap();
    let (owner, target, other) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
    std::fs::write(
        p.config_dir.join("admin.toml"),
        format!(
            "remote_admin = \"off\"\nnode_user = \"{NODE_USER}\"\ncontroller_fp = \"{}\"\ncontroller_key = \"{}\"\n",
            owner.fingerprint(),
            crypto::b64(owner.spki_der())
        ),
    )
    .unwrap();
    k2_node::admin::jobs::write_jobs_file(&p.config_dir, JOB_USER, NODE_USER).expect("the job user");
    write_servers(&p.config_dir, &servers_file(&owner.fingerprint(), &[(1, &target, ShareState::Active), (2, &other, ShareState::Active)]));
    let r = Rig { base: base.clone(), exec: executor(&base, None), owner, target, other, node, job };
    for slot in [1, 2] {
        let d = r.exec.tenant_prepare(&TenantPrepare { slot, trust: Trust::Own, weight: 1, network: Network::On }).unwrap_or_else(|e| panic!("prepare {slot}: {}: {}", e.code, e.message));
        assert_eq!(d["user"], format!("k2job-{slot}"), "{d}");
    }
    // The node's files: the owner's pin, each server's pin, ledgers.
    k2_node::identity::write_pin(&layout.pin(), &pin(&r.owner, "https://owner.test")).unwrap();
    std::fs::write(layout.ledger(), "LEDGER-0").unwrap();
    for (slot, k) in [(1, &r.target), (2, &r.other)] {
        let t = layout.tenant(slot);
        t.ensure_state().unwrap();
        k2_node::identity::write_tenant_pin(&t.pin(), &pin(k, &format!("https://server-{slot}.test")), &Default::default()).unwrap();
        std::fs::write(t.ledger(), format!("LEDGER-{slot}")).unwrap();
    }
    // Root's key records, as the confirming prune takes them (F5).
    let keys = serde_json::json!({"slots": {"1": [100, r.target.fingerprint()], "2": [100, r.other.fingerprint()]}});
    std::fs::write(p.state_dir.join(k2_node::admin::servers::KEYS_FILE), serde_json::to_vec(&keys).unwrap()).unwrap();
    let online = |slot| k2_node::status::ServerLine { slot, link: "online".into(), ..Default::default() };
    k2_node::status::write(&layout.status(), &k2_node::status::Status { servers: vec![online(1), online(2)], ..Default::default() }).unwrap();
    // The trees: the owner's (its job user's, the node's group), slot 1's
    // (its user and group, with the group's ACL), slot 2's.
    for (dir, name) in [(layout.jobs(), "owner-job"), (layout.cache(), "owner-cache")] {
        let e = dir.join(name);
        std::fs::create_dir_all(&e).unwrap();
        std::fs::write(e.join("f"), "owner").unwrap();
        chown(&e, (job.0, node.1));
        chown(&e.join("f"), (job.0, node.1));
    }
    let (u1, g1) = r.slot_user(1).unwrap();
    for t in ["jobs", "cache"] {
        let e = layout.tenant(1).root().join(t).join(format!("slot1-{t}"));
        std::fs::create_dir_all(&e).unwrap();
        std::fs::write(e.join("f"), "slot1").unwrap();
        std::fs::set_permissions(e.join("f"), std::fs::Permissions::from_mode(0o640)).unwrap();
        chown(&e, (u1, g1));
        chown(&e.join("f"), (u1, g1));
        let acl = format!("g:{}:rwx", k2_node::admin::jobs::tenant_group_name(1));
        setfacl(&["-m", &acl, &e.display().to_string()]);
        setfacl(&["-m", &acl, &e.join("f").display().to_string()]);
        assert!(has_acl(&e.join("f")), "control: slot 1's file carries its group's ACL");
    }
    let e2 = layout.tenant(2).root().join("jobs/slot2-job");
    std::fs::create_dir_all(&e2).unwrap();
    std::fs::write(e2.join("f"), "slot2").unwrap();
    r
}

/// Run the change in this process and wait for it to end.
fn change(r: &Rig, old: OldOwner) {
    let res = owner::start(&r.exec, &r.args(old), "chg1", "test", true).unwrap_or_else(|e| panic!("start: {}: {}", e.code, e.message));
    assert_eq!(res["phase"], "draining", "{res}");
    let t0 = Instant::now();
    loop {
        let done = r.exec.audit().verify().unwrap().into_iter().rev().find(|l| l.event == "owner_change");
        if let Some(l) = done {
            assert!(l.code.is_none(), "the change failed: {l:?}");
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(120), "the owner change never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    while r.exec.draining.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(t0.elapsed() < Duration::from_secs(120), "the drain never ended");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run the change in a child process that exits hard right after the step
/// file says `phase`.
fn crash_at(r: &Rig, phase: &str, old: OldOwner) {
    let st = Command::new(std::env::current_exe().unwrap())
        .args(["owner_change_crash_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("OWNER_CRASH_BASE", &r.base)
        .env("OWNER_CRASH_PHASE", phase)
        .env("OWNER_CRASH_OLD", old.as_str())
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(CRASHED), "the child should have died after the {phase} step file: {st:?}");
    assert_eq!(step_phase(&r.state()).as_deref(), Some(phase), "the step file stays at {phase}");
}

/// Resume as the next executor start does (past `busy` answers).
fn resume(r: &Rig) {
    for _ in 0..40 {
        match owner::resume(&r.exec, true) {
            Some(Ok(_)) => {
                k2_node::admin::update::recover_drain(&r.exec);
                return;
            }
            Some(Err(e)) if e.code == code::BUSY => std::thread::sleep(Duration::from_millis(300)),
            other => panic!("resume: {other:?}"),
        }
    }
    panic!("the resume stayed busy");
}

/// A sleeping process as `(uid, gid)`: a job's leftover.
fn leftover(id: (u32, u32)) -> Child {
    Command::new("sleep").arg("600").uid(id.0).gid(id.1).spawn().unwrap()
}

fn killed(c: &mut Child) -> bool {
    let t0 = Instant::now();
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            return st.signal() == Some(libc::SIGKILL);
        }
        if t0.elapsed() > Duration::from_secs(10) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn control_active(r: &Rig) -> bool {
    k2_node::config::read_control(&r.layout().control()).unwrap().control == k2_node_proto::frames::Control::Active
}

/// What a finished change (old = keep) leaves, checked as root sees it.
fn assert_changed(r: &Rig, old_uid: u32, what: &str) {
    let layout = r.layout();
    // servers.toml and admin.toml name the new owner.
    let f = read_servers(&r.config());
    assert_eq!(f.owner, r.target.fingerprint(), "{what}");
    let line = f.entry(1).unwrap();
    assert_eq!((line.label.as_str(), line.fp_prefix.as_str(), line.state), ("owner.test", &r.owner.fingerprint()[..16], ShareState::Active), "{what}");
    assert_eq!(f.entry(2).unwrap().label, "server-2.test", "{what}");
    let admin = std::fs::read_to_string(r.config().join("admin.toml")).unwrap();
    assert!(admin.contains(&r.target.fingerprint()) && !admin.contains(&r.owner.fingerprint()), "{what}: admin.toml re-pinned: {admin}");
    // The target's user went (its uid burned); the old owner got a new
    // user in slot 1, recorded in jobs.toml and its line.
    assert!(!uid_exists(old_uid), "{what}: the target's old user {old_uid} is gone");
    let (u1, g1) = r.slot_user(1).expect("the old owner has a user in slot 1");
    assert!(u1 > old_uid && u1 == g1, "{what}: a new uid ({u1}, gid {g1}) after the burned {old_uid}");
    assert_eq!(r.slot_gid(1), Some(g1), "{what}");
    let jobs = k2_node::admin::jobs::read_jobs_file(&r.config()).unwrap().unwrap();
    assert_eq!(jobs.tenant(1).map(|t| t.uid), Some(u1), "{what}: jobs.toml has the new user");
    assert_eq!(line.uid, Some(u1), "{what}: its line records the uid");
    // The new owner's trees are the home's: the node user's and group's,
    // group-writable, no ACL from the old slot left.
    for t in ["jobs", "cache"] {
        let e = layout.home.join(t).join(format!("slot1-{t}"));
        for p in [e.clone(), e.join("f")] {
            let m = std::fs::symlink_metadata(&p).unwrap_or_else(|x| panic!("{what}: {}: {x}", p.display()));
            assert_eq!((m.uid(), m.gid()), r.node, "{what}: {} re-owned to the node user", p.display());
            assert!(!has_acl(&p), "{what}: {} keeps no ACL of the old slot", p.display());
        }
        let dm = std::fs::metadata(&e).unwrap().mode() & 0o7777;
        assert_eq!(dm & 0o770, 0o770, "{what}: a directory is u+rwx,g+rwx ({dm:o})");
        assert_eq!(dm & 0o4000, 0, "{what}: no setuid on a directory ({dm:o})");
        let fm = std::fs::metadata(e.join("f")).unwrap().mode() & 0o7777;
        assert_eq!(fm & 0o6000, 0, "{what}: no setuid/setgid on a file ({fm:o})");
        assert_eq!(fm & 0o660, 0o660, "{what}: a file is u+rw,g+rw ({fm:o})");
    }
    // The old owner's trees are slot 1's: the node user and slot 1's new
    // group.
    for t in ["jobs", "cache"] {
        let e = layout.tenant(1).root().join(t).join(format!("owner-{}", if t == "jobs" { "job" } else { "cache" }));
        for p in [e.clone(), e.join("f")] {
            let m = std::fs::symlink_metadata(&p).unwrap_or_else(|x| panic!("{what}: {}: {x}", p.display()));
            assert_eq!((m.uid(), m.gid()), (r.node.0, g1), "{what}: {} re-owned to slot 1's new group", p.display());
        }
    }
    assert!(!layout.home.join("jobs/owner-job").exists() && !layout.tenant(1).root().join("jobs/slot1-jobs").exists(), "{what}: swapped exactly once");
    // Slot 2 untouched; its user stays.
    assert!(layout.tenant(2).root().join("jobs/slot2-job/f").exists(), "{what}");
    assert!(r.slot_user(2).is_some(), "{what}");
    // The node's half ran (pins and ledgers traded places).
    assert_eq!(k2_node::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, r.target.fingerprint(), "{what}");
    assert_eq!(std::fs::read_to_string(layout.ledger()).unwrap(), "LEDGER-1", "{what}");
    assert!(!r.state().join(owner::STEP_FILE).exists(), "{what}: the step file goes");
    assert!(control_active(r), "{what}: control is back");
}

// ── the tests ────────────────────────────────────────────────────────

/// O1 + O7: the whole change as root: the leftover processes of both
/// servers' jobs are stopped (slot 2's are not), the target's user goes,
/// the old owner gets a new one, every moved tree is re-owned for its new
/// slot (the ACL of the old slot stripped, modes right).
#[test]
#[ignore = "root, systemd, real users: run in a throwaway box"]
fn the_owner_changes_with_real_users() {
    let r = rig("o1");
    let (old_uid, old_gid) = r.slot_user(1).unwrap();
    let mut slot1 = leftover((old_uid, old_gid));
    let mut owner_job = leftover(r.job);
    let (u2, g2) = r.slot_user(2).unwrap();
    let mut slot2 = leftover((u2, g2));
    let (slot1_pid, owner_pid) = (slot1.id(), owner_job.id());
    change(&r, OldOwner::Keep);
    assert!(killed(&mut slot1), "slot 1's leftover process is stopped");
    assert!(killed(&mut owner_job), "the owner's job leftover is stopped");
    // It was the trees step that stopped them (before anything moved, and
    // before the users step, which would stop slot 1's too).
    let audit = r.exec.audit().verify().unwrap();
    let stop = audit.iter().position(|l| l.event == "tenant.stop").expect("the trees step stopped the leftovers (tenant.stop)");
    let unuser = audit.iter().position(|l| l.event == "tenant.unuser").expect("the users step (tenant.unuser)");
    assert!(stop < unuser, "tenant.stop comes first");
    assert!(audit[stop].code.is_none(), "{:?}", audit[stop]);
    let removed: Vec<String> = audit[stop].detail["removed"].as_array().expect("its removed list").iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert!(removed.contains(&format!("slot 1 pid {slot1_pid}")), "slot 1's leftover was stopped by the trees step: {removed:?}");
    assert!(removed.contains(&format!("owner job pid {owner_pid}")), "and the owner's: {removed:?}");
    assert!(!removed.iter().any(|x| x.starts_with("slot 2 ")), "never another server's: {removed:?}");
    assert!(slot2.try_wait().unwrap().is_none(), "another server's process is left alone");
    let _ = slot2.kill();
    let _ = slot2.wait();
    assert_changed(&r, old_uid, "o1");
}

/// O6: `old = revoke` with a real slot: the old owner's line is revoked,
/// it gets no user, and the slot (its moved trees included) is wiped for
/// real.
#[test]
#[ignore = "root, systemd, real users: run in a throwaway box"]
fn a_revoked_old_owner_is_wiped_for_real() {
    let r = rig("o6");
    let (old_uid, _) = r.slot_user(1).unwrap();
    change(&r, OldOwner::Revoke);
    let f = read_servers(&r.config());
    assert_eq!(f.owner, r.target.fingerprint());
    let line = f.entry(1).unwrap();
    assert_eq!((line.label.as_str(), line.state), ("owner.test", ShareState::Revoked));
    assert!(!uid_exists(old_uid), "the target's old user is gone");
    assert!(r.slot_user(1).is_none(), "a revoked old owner gets no user");
    let slot1 = r.layout().tenant(1).root();
    assert!(slot1.join("jobs/owner-job/f").exists(), "its trees wait in slot 1 for the wipe");
    // The wipe the prune runs 24 h later, now.
    r.exec.tenant_wipe(&TenantWipe { slot: 1 }).unwrap_or_else(|e| panic!("wipe: {}: {}", e.code, e.message));
    assert!(!slot1.exists(), "slot 1's root is gone");
    assert!(r.slot_gid(1).is_none(), "and its group");
    assert!(r.slot_user(2).is_some() && r.layout().tenant(2).root().join("jobs/slot2-job/f").exists(), "slot 2 untouched");
}

/// O5: an executor killed right after each step file is written resumes
/// on its next start and ends exactly as one change does.
#[test]
#[ignore = "root, systemd, real users: run in a throwaway box"]
fn a_crash_after_each_step_resumes() {
    for phase in ["trees", "users", "restart", "files"] {
        let r = rig(&format!("o5-{phase}"));
        let (old_uid, _) = r.slot_user(1).unwrap();
        crash_at(&r, phase, OldOwner::Keep);
        assert!(!control_active(&r), "{phase}: control stays draining while the change is unfinished");
        resume(&r);
        assert_changed(&r, old_uid, phase);
    }
}

/// O7 (abort): a change that died before the node's swap is undone with
/// `--abort`: the trees go back, the target gets a user again (a new uid)
/// and its trees are re-owned for that user's group; nothing else changed.
#[test]
#[ignore = "root, systemd, real users: run in a throwaway box"]
fn abort_rehomes_back_with_real_users() {
    let r = rig("abort");
    let (old_uid, _) = r.slot_user(1).unwrap();
    crash_at(&r, "restart", OldOwner::Keep);
    assert!(!uid_exists(old_uid), "control: the users step ran");
    let v = owner::abort(&r.exec).unwrap_or_else(|e| panic!("abort: {}: {}", e.code, e.message));
    assert_eq!(v["undone"], serde_json::json!(["trees", "user"]), "{v}");
    let layout = r.layout();
    let f = read_servers(&r.config());
    assert_eq!(f.owner, r.owner.fingerprint(), "servers.toml untouched");
    assert_eq!(f.entry(1).unwrap().label, TARGET);
    let admin = std::fs::read_to_string(r.config().join("admin.toml")).unwrap();
    assert!(admin.contains(&r.owner.fingerprint()), "admin.toml untouched");
    // The owner's trees are home again, as they were.
    for p in [layout.home.join("jobs/owner-job/f"), layout.home.join("cache/owner-cache/f")] {
        let m = std::fs::symlink_metadata(&p).unwrap_or_else(|x| panic!("{}: {x}", p.display()));
        assert_eq!((m.uid(), m.gid()), (r.job.0, r.node.1), "{} as it was", p.display());
    }
    // The target has a user again, and its trees are its new group's.
    let (u1, g1) = r.slot_user(1).expect("the target has a user again");
    assert!(u1 > old_uid, "a new uid ({u1}) after the burned {old_uid}");
    for t in ["jobs", "cache"] {
        let e = layout.tenant(1).root().join(t).join(format!("slot1-{t}"));
        for p in [e.clone(), e.join("f")] {
            let m = std::fs::symlink_metadata(&p).unwrap_or_else(|x| panic!("{}: {x}", p.display()));
            assert_eq!((m.uid(), m.gid()), (r.node.0, g1), "{} re-owned for the target's new group", p.display());
        }
    }
    assert!(!layout.home.join("jobs/slot1-jobs").exists());
    assert!(!r.state().join(owner::STEP_FILE).exists(), "the step file goes");
    assert!(!owner::change_in_progress(&r.config()), "and the marker");
    assert!(owner::read_node_change(&r.config()).is_none(), "the node never swaps later");
    assert!(control_active(&r), "control is back");
    assert_eq!(k2_node::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, r.owner.fingerprint(), "the node's pins untouched");
}

/// The crash tests' child: runs the change and exits hard after the step
/// file `OWNER_CRASH_PHASE` is written. Does nothing when run on its own.
#[test]
#[ignore = "the crash tests' child process"]
fn owner_change_crash_child() {
    let (Ok(base), Ok(phase)) = (std::env::var("OWNER_CRASH_BASE"), std::env::var("OWNER_CRASH_PHASE")) else {
        return;
    };
    let old = OldOwner::parse(&std::env::var("OWNER_CRASH_OLD").unwrap()).unwrap();
    node_user();
    let base = PathBuf::from(base);
    let exec = executor(&base, Some(phase.clone()));
    let target_fp = std::fs::read_to_string(base.join("admin-state").join(k2_node::admin::servers::KEYS_FILE)).unwrap();
    let keys: serde_json::Value = serde_json::from_str(&target_fp).unwrap();
    let fp = keys["slots"]["1"][1].as_str().unwrap().to_string();
    let args = OwnerArgs { server: TARGET.into(), fp: Some(fp), old, caches: Caches::Move, now: false };
    owner::start(&exec, &args, "chg1", "test", true).unwrap_or_else(|e| panic!("start: {}: {}", e.code, e.message));
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the change never reached the {phase} step file");
}
