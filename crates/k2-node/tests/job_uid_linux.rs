//! Per-job uids for real (protocol 9 slice C2; prd-ci-on-compute-for-
//! clients-v1 §4.3, vs-live V15/V16/S8): the root executor gives each
//! secret job a uid of its own from its slot's recycled pool
//! (`admin::jobpool`), in a real systemd unit, with real users made by
//! `useradd` and the job root closed with `setfacl`.
//!
//! **Root e2e, gated twice.** It makes and deletes real users, so every
//! test is `#[ignore]` (a plain `cargo test` lists each one as ignored,
//! with the reason), and an `--ignored` run refuses to start (fails, with
//! the reason) unless it runs as root with `K2_NODE_ROOT_E2E=1`. Run it
//! only on a throwaway Linux box (systemd ≥ 248, `acl` installed), one at
//! a time; `scripts/ci/node-root-e2e.sh` builds it and runs it there:
//!
//!   sudo K2_NODE_ROOT_E2E=1 ./job_uid_linux --ignored --test-threads=1
//!
//! The rig is `tenant_wall_linux.rs`'s, driven at the executor's socket
//! (as the node drives it): each test makes its own dir under /var/lib, a
//! node user `k2nodet` (once per box), tenant slots prepared by the
//! executor's own `tenant.prepare`, and wipes them (users, groups, roots,
//! pool users) when it ends. Where the node's part matters (a secret
//! job's tree reclaimed and deleted after it ends), the test does what the
//! node does. Every wall check has a control (root reaches the thing).

// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]
#![cfg(target_os = "linux")]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use k2_node::admin::exec::{ExecPaths, Executor, SystemPlatform};
use k2_node::admin::jobpool::{self, pool_user_name};
use k2_node::admin::jobs::{self, JobAnswer, JobOp, JobStart};
use k2_node::admin::server::{self, PeerRule};
use k2_node::wall::ExecSpec;
use k2_node_proto::share::{self, Network, ServersFile, ShareLimits, ShareState, Slot, TenantPrepare, TenantWipe, Trust};

const GATE: &str = "K2_NODE_ROOT_E2E";
const WHY: &str = "root e2e: makes real users; run as root with K2_NODE_ROOT_E2E=1 on a throwaway Linux box (scripts/ci/node-root-e2e.sh)";

/// The second gate: an `--ignored` run outside a throwaway box fails here,
/// loudly, before anything is made.
fn gate() {
    if std::env::var(GATE).as_deref() != Ok("1") {
        panic!("job_uid_linux: NOT RUN: {GATE}=1 is not set. These tests create and delete real users and systemd units; {WHY}");
    }
    // SAFETY: plain getter.
    assert_eq!(unsafe { libc::geteuid() }, 0, "job_uid_linux: NOT RUN: not root; {WHY}");
}

// ── the rig (tenant_wall_linux.rs's) ─────────────────────────────────

fn node_test_user() -> (String, u32) {
    let name = "k2nodet";
    if server::user_ids(name).is_none() {
        let st = std::process::Command::new("useradd")
            .args(["--system", "--user-group", "--no-create-home", "--home-dir", "/nonexistent", "--shell", "/usr/sbin/nologin", name])
            .status()
            .unwrap();
        assert!(st.success(), "useradd {name}");
    }
    let (_, gid) = server::user_ids(name).unwrap();
    // SAFETY: glibc applies setegid to every thread of the process.
    assert_eq!(unsafe { libc::setegid(gid) }, 0, "setegid {gid}");
    (name.to_string(), gid)
}

struct Rig {
    base: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    exec: Arc<Executor>,
    slots: Vec<Slot>,
}

impl Rig {
    fn layout(&self) -> k2_node::paths::Layout {
        k2_node::paths::Layout::new(self.base.join("home"), &self.config)
    }

    fn op(&self, op: &JobOp) -> JobAnswer {
        server::job_request(&self.socket, op, &[], Duration::from_secs(120)).unwrap_or_else(|e| panic!("{op:?}: {}: {}", e.code, e.message))
    }

    fn write_servers(&self, states: &[(Slot, ShareState)]) {
        let room = |l: ShareLimits| ShareLimits { max_running: 8, max_queued: 100, ..l };
        let mut f = ServersFile::new(&"a".repeat(64), "owner.test", room(ShareLimits::owner(2, 8, 16, 100, Network::On)), 8, 1000, "local:test");
        for slot in &self.slots {
            let mut e = f.servers[0].clone();
            e.slot = *slot;
            e.label = format!("server-{slot}.test");
            e.fp_prefix = format!("{:016x}", u64::from(*slot) + 0xb0);
            e.trust = Trust::Team;
            e.set_limits(&room(ShareLimits::new_server(Trust::Team, false)));
            e.network = Network::On;
            e.state = states.iter().find(|(s, _)| s == slot).map(|(_, st)| *st).unwrap_or(ShareState::Active);
            f.servers.push(e);
        }
        f.validate().unwrap();
        std::fs::write(self.config.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
    }

    fn prepare(&self, slot: Slot) {
        match self.op(&JobOp::TenantPrepare(TenantPrepare { slot, trust: Trust::Team, weight: 1, network: Network::On })) {
            JobAnswer::Done { detail } => assert_eq!(detail["user"], format!("k2job-{slot}"), "{detail}"),
            other => panic!("prepare {slot}: {other:?}"),
        }
    }

    fn pool(&self) -> jobpool::PoolFile {
        jobpool::read_pool(&self.exec.paths.state_dir.join(jobpool::POOL_FILE)).unwrap()
    }

    fn tenant_ids_next(&self) -> u32 {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(self.exec.paths.state_dir.join("tenant-ids.json")).unwrap()).unwrap();
        v["next"].as_u64().unwrap() as u32
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let revoked: Vec<_> = self.slots.iter().map(|s| (*s, ShareState::Revoked)).collect();
        self.write_servers(&revoked);
        for slot in &self.slots {
            if let Err(e) = self.exec.tenant_wipe(&TenantWipe { slot: *slot }) {
                eprintln!("cleanup: wipe slot {slot}: {}: {}", e.code, e.message);
            }
        }
        let _ = std::fs::remove_dir_all(&self.base);
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn rig(tag: &str, slots: &[Slot]) -> Rig {
    gate();
    let id = k2_node_proto::crypto::random_id().unwrap();
    let base = PathBuf::from(format!("/var/lib/k2-jobuid-test-{tag}-{}", &id[..8]));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = base.join("config");
    let state = base.join("admin-state");
    for d in [&config, &state, &base.join("home")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (node_user, node_gid) = node_test_user();
    std::os::unix::fs::chown(base.join("home"), Some(0), Some(node_gid)).unwrap();
    std::fs::set_permissions(base.join("home"), std::fs::Permissions::from_mode(0o750)).unwrap();
    k2_node::paths::Layout::new(base.join("home"), &config).ensure().unwrap();
    std::fs::write(config.join("admin.toml"), format!("remote_admin = \"off\"\nnode_user = \"{node_user}\"\n")).unwrap();
    jobs::write_jobs_file(&config, "nobody", &node_user).expect("nobody is a job user");
    std::fs::create_dir_all(base.join("bin")).unwrap();
    let bin = base.join("bin/k2-node");
    // The k2-node the units run (`job-exec`): the one cargo built beside
    // this test, or, for a test binary copied to a throwaway box
    // (scripts/ci/node-root-e2e.sh), the copy named by K2_NODE_E2E_BIN.
    let src = std::env::var_os("K2_NODE_E2E_BIN").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_k2-node")));
    std::fs::copy(&src, &bin).unwrap_or_else(|e| panic!("copy k2-node from {}: {e}", src.display()));
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let socket = PathBuf::from(format!("/run/k2na-ju-{}.sock", &id[..10]));
    let exec = Arc::new(Executor::new(
        ExecPaths { config_dir: config.clone(), state_dir: state, home: base.join("home"), binary: bin },
        Arc::new(SystemPlatform),
    ));
    let l = server::bind(&socket, None).unwrap();
    let e2 = exec.clone();
    std::thread::spawn(move || {
        let _ = server::serve(&e2, &PeerRule { allowed_uids: vec![0] }, l, &|| false);
    });
    let r = Rig { base, socket, config, exec, slots: slots.to_vec() };
    r.write_servers(&[]);
    for s in slots {
        r.prepare(*s);
    }
    match r.op(&JobOp::Ready) {
        JobAnswer::Ready { job_uid: true, .. } => {}
        other => panic!("this box can't give per-job uids (systemd ≥ 247, useradd, userdel, setfacl): {other:?}"),
    }
    r
}

// ── jobs at the executor ─────────────────────────────────────────────

struct Job {
    unit: String,
    root: PathBuf,
    index: Option<u32>,
}

/// Lay out a job root as the node does and start `script` in it (`sh -c`)
/// as slot `slot`'s job; `job_uid` asks for a per-job uid. `Err` = the
/// executor's refusal.
fn start(r: &Rig, slot: Slot, name: &str, script: &str, env: &[(&str, &str)], job_uid: bool) -> Result<Job, String> {
    start_rw(r, slot, name, script, env, job_uid, vec![])
}

fn start_rw(r: &Rig, slot: Slot, name: &str, script: &str, env: &[(&str, &str)], job_uid: bool, rw: Vec<PathBuf>) -> Result<Job, String> {
    use std::os::fd::AsRawFd;
    let key = k2_node::node::job_key(slot, name, 1);
    let root = r.layout().tenant(slot).jobs().join(&key);
    for d in [root.clone(), root.join(k2_node::runner::JOB_TMP), root.join(k2_node::runner::JOB_VAR_TMP)] {
        std::fs::create_dir_all(&d).unwrap();
    }
    let mut e: Vec<(String, String)> = vec![("PATH".into(), "/usr/bin:/bin".into()), ("HOME".into(), root.display().to_string()), ("R".into(), root.display().to_string())];
    e.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    k2_node::wall::write_exec_spec(&root, &ExecSpec { argv: vec!["/bin/sh".into(), "-c".into(), script.into()], env: e, cwd: root.clone(), stdin_env: false }).unwrap();
    let (r1, w1) = std::io::pipe().unwrap();
    let (r2, w2) = std::io::pipe().unwrap();
    let raw = vec![w1.as_raw_fd(), w2.as_raw_fd()];
    let s = JobStart {
        key,
        root: root.clone(),
        rw,
        network_none: false,
        mem_bytes: None,
        cpu_millis: None,
        stop_grace_secs: 5,
        tenant: slot,
        no_lan: false,
        stdin: false,
        max_secs: Some(300),
        job_uid,
    };
    let a = server::job_request(&r.socket, &JobOp::Start(s), &raw, Duration::from_secs(60)).map_err(|e| format!("{}: {}", e.code, e.message))?;
    drop((w1, w2));
    // Nobody reads the job's pipes (its results go to files in its root):
    // drain them so a chatty job never blocks.
    for p in [r1, r2] {
        std::thread::spawn(move || {
            let mut p = p;
            let _ = std::io::copy(&mut p, &mut std::io::sink());
        });
    }
    match a {
        JobAnswer::Started { unit, uid_index } => {
            assert_eq!(uid_index.is_some(), job_uid, "{unit}: a uid index exactly for a job_uid start");
            Ok(Job { unit, root, index: uid_index })
        }
        JobAnswer::Refused(e) => Err(format!("{}: {}", e.code, e.message)),
        other => panic!("start answered {other:?}"),
    }
}

fn status(r: &Rig, unit: &str) -> jobs::UnitStatus {
    match r.op(&JobOp::Status { unit: unit.into() }) {
        JobAnswer::Status(s) => s,
        other => panic!("status {unit}: {other:?}"),
    }
}

/// Wait for the job's command to end (a minute at most).
fn ended(r: &Rig, j: &Job) {
    let t0 = Instant::now();
    while !status(r, &j.unit).ended() {
        assert!(t0.elapsed() < Duration::from_secs(60), "{} never ended", j.unit);
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stop (if needed), release, and wait until systemd forgot the unit.
fn release(r: &Rig, j: &Job) {
    r.op(&JobOp::Release { unit: j.unit.clone() });
    let t0 = Instant::now();
    while status(r, &j.unit).loaded {
        assert!(t0.elapsed() < Duration::from_secs(60), "{} never went", j.unit);
        std::thread::sleep(Duration::from_millis(100));
        r.op(&JobOp::Release { unit: j.unit.clone() });
    }
}

/// What the node does after a secret job: its whole tree back to the node
/// user (`reclaim`, root included), then deleted.
fn node_cleanup(r: &Rig, slot: Slot, j: &Job) {
    match r.op(&JobOp::Reclaim { path: j.root.clone(), slot }) {
        JobAnswer::Done { .. } => {}
        other => panic!("reclaim {}: {other:?}", j.root.display()),
    }
    std::fs::remove_dir_all(&j.root).unwrap();
}

fn finish(r: &Rig, slot: Slot, j: &Job) {
    ended(r, j);
    release(r, j);
    node_cleanup(r, slot, j);
}

/// Wait for a file the job writes (a minute at most).
fn wait_file(p: &Path) -> String {
    let t0 = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(p) {
            if s.ends_with('\n') {
                return s.trim().to_string();
            }
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "{} never written", p.display());
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pool_uid(slot: Slot, index: u32) -> (u32, u32) {
    server::user_ids(&pool_user_name(slot, index)).unwrap_or_else(|| panic!("{} exists", pool_user_name(slot, index)))
}

/// Every process running as `uid` (from /proc).
fn pids_of(uid: u32) -> Vec<i32> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let Ok(st) = std::fs::read_to_string(e.path().join("status")) else { continue };
        let real = st.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|r| r.split_whitespace().next()?.parse::<u32>().ok());
        if real == Some(uid) {
            out.push(pid);
        }
    }
    out
}

fn proc_is(pid: i32, comm: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == comm)
}

fn held(r: &Rig, slot: Slot, index: u32) -> String {
    r.pool().get(slot, index).and_then(|u| u.held.clone()).unwrap_or_default()
}

/// `sh` that probes each argument: `R <path> OK|NO` (read), `L <dir>
/// OK|NO` (list), one per line, into `$R/res`.
const PROBE: &str = r#"for p in "$@"; do
case "$p" in
  +*) d="${p#+}"; if ls -A -- "$d" >/dev/null 2>&1; then echo "L $d OK"; else echo "L $d NO"; fi ;;
  *) if c=$(cat -- "$p" 2>/dev/null) && [ -n "$c" ]; then echo "R $p OK $c"; else echo "R $p NO"; fi ;;
esac
done > "$R/res.tmp"; echo done >> "$R/res.tmp"; mv "$R/res.tmp" "$R/res""#;

fn probe_script(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("set -- {}; {PROBE}", quoted.join(" "))
}

fn line<'a>(res: &'a str, kind: &str, path: &str) -> &'a str {
    let want = format!("{kind} {path} ");
    res.lines().find(|l| l.starts_with(&want)).unwrap_or_else(|| panic!("no {kind} line for {path} in:\n{res}"))
}

// ── the tests ────────────────────────────────────────────────────────

/// A secret job runs as a pool user the executor made for it (a bare
/// account in its slot's group, uid from the tenant range), writes only
/// its own root, and its root is closed to everyone but itself and the
/// node user. Another job of the same server (the slot's own job user,
/// what a same-workspace job ran as before), and a second per-job-uid
/// job, can read neither its `/proc/<pid>/environ` nor its files; root
/// can (the control).
#[test]
#[ignore = "root e2e: makes real users; run as root with K2_NODE_ROOT_E2E=1 on a throwaway Linux box (scripts/ci/node-root-e2e.sh)"]
fn a_secret_job_runs_as_its_own_uid_and_nothing_else_reads_it() {
    let r = rig("own", &[1]);
    let l = r.layout();
    let cache = l.tenant(1).cache();
    // A per-job uid start never takes a read-write path outside its root.
    // (A folder of the slot's cache: a plain job may have it read-write.)
    std::fs::create_dir_all(cache.join("pk")).unwrap();
    let e = start_rw(&r, 1, "rwcache", "true", &[], true, vec![cache.join("pk")]).map(|_| ()).unwrap_err();
    assert!(e.contains("writes only its job root"), "{e}");
    let a = start(
        &r,
        1,
        "secret-a",
        &format!(
            "id -u > \"$R/uid\"; id -g > \"$R/gid\"; echo SECRET-a-file > \"$R/secret.txt\"; \
             if (echo x > '{}/a-wrote') 2>/dev/null; then echo CACHE-OK; else echo CACHE-NO; fi > \"$R/cache\"; \
             echo x > /var/tmp/a; if [ -f \"$R/var-tmp/a\" ]; then echo VT-IN; else echo VT-OUT; fi > \"$R/vt\"; \
             echo $$ > \"$R/pid\"; exec sleep 120",
            cache.display()
        ),
        &[("SECRET_A", "SECRET-a-env-value")],
        true,
    )
    .unwrap();
    assert_eq!(a.index, Some(0), "a fresh pool hands out index 0");
    // The pool user: made now, its slot's group, no login, a tenant uid.
    let (uid, gid) = pool_uid(1, 0);
    assert_eq!(gid, jobs::group_id("k2t-1").unwrap(), "its slot's group");
    assert!((share::TENANT_UID_FIRST..=share::TENANT_UID_LAST).contains(&uid), "{uid}");
    assert_ne!(uid, server::user_ids("k2job-1").unwrap().0, "not the slot's job user");
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap();
    let entry = passwd.lines().find(|l| l.starts_with(&format!("{}:", pool_user_name(1, 0)))).unwrap();
    assert!(entry.ends_with("nologin") || entry.ends_with("/bin/false"), "{entry}");
    assert_eq!(r.pool().get(1, 0).unwrap().uid, uid, "recorded in root's pool file");
    // It runs as that uid, writes only its root (/var/tmp is inside it).
    let pid: i32 = wait_file(&a.root.join("pid")).parse().unwrap();
    assert_eq!(wait_file(&a.root.join("uid")), uid.to_string());
    assert_eq!(wait_file(&a.root.join("gid")), gid.to_string());
    assert_eq!(wait_file(&a.root.join("cache")), "CACHE-NO", "the slot's cache isn't its to write");
    assert_eq!(wait_file(&a.root.join("vt")), "VT-IN");
    assert!(!cache.join("a-wrote").exists());
    // Its root: its own, closed (no group, no other; the node user by name).
    let m = std::fs::metadata(&a.root).unwrap();
    assert_eq!(m.uid(), uid);
    let acl = String::from_utf8(std::process::Command::new("getfacl").args(["-p", "-c"]).arg(&a.root).output().unwrap().stdout).unwrap();
    assert!(acl.lines().any(|l| l == "group::---") && acl.lines().any(|l| l == "other::---"), "{acl}");
    assert!(acl.lines().any(|l| l == "user:k2nodet:rwx"), "{acl}");
    assert!(!acl.contains("group:k2t-1"), "the slot's group has no way in, nor into what it makes there: {acl}");
    assert!(acl.lines().any(|l| l == "default:group::---") && acl.lines().any(|l| l == "default:user:k2nodet:rwx"), "{acl}");
    // What it made at its top: the node user reads it, the slot's group doesn't.
    let facl = String::from_utf8(std::process::Command::new("getfacl").args(["-p", "-c"]).arg(a.root.join("secret.txt")).output().unwrap().stdout).unwrap();
    assert!(!facl.contains("group:k2t-1") && facl.lines().any(|l| l.starts_with("user:k2nodet:")), "{facl}");
    // Control: root reads its env and its file.
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    assert!(environ.windows(18).any(|w| w == b"SECRET-a-env-value"), "control: the value is in its environ");
    assert_eq!(std::fs::read_to_string(a.root.join("secret.txt")).unwrap(), "SECRET-a-file\n");
    // Another job of the same server, as the slot's job user, and another
    // secret job with its own uid: neither reads it.
    let items = vec![format!("/proc/{pid}/environ"), a.root.join("secret.txt").display().to_string(), format!("+{}", a.root.display()), "/proc/self/environ".to_string()];
    let b = start(&r, 1, "same-ws-b", &probe_script(&items), &[], false).unwrap();
    let c = start(&r, 1, "secret-c", &probe_script(&items), &[], true).unwrap();
    assert_eq!(c.index, Some(1), "index 0 is busy: a second pool user");
    assert_ne!(pool_uid(1, 1).0, uid);
    for j in [&b, &c] {
        let res = wait_file(&j.root.join("res"));
        assert!(res.ends_with("done"), "{res}");
        assert!(line(&res, "R", &format!("/proc/{pid}/environ")).contains(" NO"), "{}: environ read: {res}", j.unit);
        assert!(line(&res, "R", &a.root.join("secret.txt").display().to_string()).contains(" NO"), "{}: file read: {res}", j.unit);
        assert!(line(&res, "L", &a.root.display().to_string()).contains(" NO"), "{}: root listed: {res}", j.unit);
        assert!(!res.contains("SECRET-a"), "{}: {res}", j.unit);
        assert!(line(&res, "R", "/proc/self/environ").contains(" OK"), "control: its own environ: {res}");
    }
    finish(&r, 1, &b);
    finish(&r, 1, &c);
    r.op(&JobOp::Stop { unit: a.unit.clone() });
    finish(&r, 1, &a);
}

/// Recycling (V15): an index is reused only once proven empty. Its last
/// root still holding its files, a leftover process that won't die (a
/// zombie), a file of its in a folder under /tmp: each keeps it out and
/// the next secret job gets another index; once gone, the same index and
/// uid come back. A leftover that can be killed is killed, and a file of
/// its at the top of /tmp is removed, before the proof.
#[test]
#[ignore = "root e2e: makes real users; run as root with K2_NODE_ROOT_E2E=1 on a throwaway Linux box (scripts/ci/node-root-e2e.sh)"]
fn an_index_is_reused_only_once_proven_empty() {
    let r = rig("recycle", &[1]);
    let quick = "id -u > \"$R/uid\"";
    let j1 = start(&r, 1, "j1", quick, &[], true).unwrap();
    assert_eq!(j1.index, Some(0));
    let (uid0, gid0) = pool_uid(1, 0);
    ended(&r, &j1);
    release(&r, &j1);
    // 1. Its root still holds its files (the node hasn't deleted it yet).
    let j2 = start(&r, 1, "j2", quick, &[], true).unwrap();
    assert_eq!(j2.index, Some(1), "index 0 still owns its root");
    assert!(held(&r, 1, 0).contains("still owns"), "{}", held(&r, 1, 0));
    finish(&r, 1, &j2);
    node_cleanup(&r, 1, &j1);
    // Recycled: the same index, the same uid.
    let j3 = start(&r, 1, "j3", quick, &[], true).unwrap();
    assert_eq!(j3.index, Some(0), "proven empty: index 0 again");
    assert_eq!(wait_file(&j3.root.join("uid")), uid0.to_string());
    assert_eq!(held(&r, 1, 0), "");
    finish(&r, 1, &j3);
    // 2. A leftover process that a kill doesn't end (a zombie of its uid).
    let mut zombie = std::process::Command::new("/bin/true").uid(uid0).gid(gid0).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let j4 = start(&r, 1, "j4", quick, &[], true).unwrap();
    assert_ne!(j4.index, Some(0), "a process of its uid keeps index 0 out");
    assert!(held(&r, 1, 0).contains("still runs process"), "{}", held(&r, 1, 0));
    finish(&r, 1, &j4);
    zombie.wait().unwrap();
    let j5 = start(&r, 1, "j5", quick, &[], true).unwrap();
    assert_eq!(j5.index, Some(0), "the process is gone: index 0 again");
    finish(&r, 1, &j5);
    // 3. A leftover that can be killed (detached, as a job's escapee
    // would be: init reaps it) is killed by the proof.
    let st = std::process::Command::new("/bin/sh")
        .args(["-c", "/bin/sleep 300 </dev/null >/dev/null 2>&1 &"])
        .current_dir("/")
        .uid(uid0)
        .gid(gid0)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(pids_of(uid0).iter().any(|p| proc_is(*p, "sleep")), "control: the leftover runs as uid {uid0}");
    let j6 = start(&r, 1, "j6", quick, &[], true).unwrap();
    assert_eq!(j6.index, Some(0), "its leftover was killed, then proven gone");
    assert!(pids_of(uid0).iter().all(|p| !proc_is(*p, "sleep")), "the leftover is dead");
    finish(&r, 1, &j6);
    // 4. A file of its in a folder under /tmp keeps it out.
    let id = k2_node_proto::crypto::random_id().unwrap();
    let nested = PathBuf::from(format!("/tmp/k2-jobuid-e2e-{}", &id[..8]));
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("left"), "x").unwrap();
    std::os::unix::fs::chown(nested.join("left"), Some(uid0), Some(gid0)).unwrap();
    let j7 = start(&r, 1, "j7", quick, &[], true).unwrap();
    assert_ne!(j7.index, Some(0), "a file of its uid in /tmp keeps index 0 out");
    assert!(held(&r, 1, 0).contains(&nested.join("left").display().to_string()), "{}", held(&r, 1, 0));
    finish(&r, 1, &j7);
    std::fs::remove_dir_all(&nested).unwrap();
    let j8 = start(&r, 1, "j8", quick, &[], true).unwrap();
    assert_eq!(j8.index, Some(0));
    finish(&r, 1, &j8);
    // 5. A file of its at the top of /tmp is removed first (as tenant.wipe
    // does), so it doesn't hold the index.
    let top = PathBuf::from(format!("/tmp/k2-jobuid-e2e-top-{}", &id[..8]));
    std::fs::write(&top, "x").unwrap();
    std::os::unix::fs::chown(&top, Some(uid0), Some(gid0)).unwrap();
    let j9 = start(&r, 1, "j9", quick, &[], true).unwrap();
    assert_eq!(j9.index, Some(0));
    assert!(!top.exists(), "its top-level leftover in /tmp was removed");
    finish(&r, 1, &j9);
    // The pool never grew past what was needed.
    assert!(r.pool().slot_users(1).len() <= 2, "{:?}", r.pool());
}

/// `tenant.wipe` and `tenant.unuser` delete a slot's pool users with its
/// own user and group; their uids are spent for good: the slot's next
/// pool user gets a new one.
#[test]
#[ignore = "root e2e: makes real users; run as root with K2_NODE_ROOT_E2E=1 on a throwaway Linux box (scripts/ci/node-root-e2e.sh)"]
fn wipe_and_unuser_delete_the_pool_and_never_reuse_its_uids() {
    let r = rig("wipe", &[1, 2]);
    let long = start(&r, 1, "w-long", "exec sleep 120", &[], true).unwrap();
    let short = start(&r, 1, "w-short", "true", &[], true).unwrap();
    let other = start(&r, 2, "w-other", "true", &[], true).unwrap();
    assert_eq!((long.index, short.index, other.index), (Some(0), Some(1), Some(0)));
    let old: Vec<u32> = [pool_uid(1, 0), pool_uid(1, 1), pool_uid(2, 0)].iter().map(|u| u.0).collect();
    r.op(&JobOp::Stop { unit: long.unit.clone() });
    finish(&r, 1, &long);
    finish(&r, 1, &short);
    finish(&r, 2, &other);
    assert!(r.tenant_ids_next() > *old.iter().max().unwrap(), "spent ids are behind the high-water mark");
    // Wipe slot 1 (revoked first: only a revoked slot is wiped).
    r.write_servers(&[(1, ShareState::Revoked)]);
    match r.op(&JobOp::TenantWipe(TenantWipe { slot: 1 })) {
        JobAnswer::Done { detail } => eprintln!("wiped: {detail}"),
        other => panic!("wipe: {other:?}"),
    }
    for i in [0, 1] {
        assert!(server::user_ids(&pool_user_name(1, i)).is_none(), "{} is gone", pool_user_name(1, i));
    }
    assert!(server::user_ids("k2job-1").is_none() && jobs::group_id("k2t-1").is_none());
    assert!(r.pool().slot_users(1).is_empty(), "{:?}", r.pool());
    assert_eq!(r.pool().slot_users(2).len(), 1, "another slot's pool is untouched");
    // Unuser slot 2 (servers.owner's user step).
    let d = r.exec.tenant_unuser(2).unwrap_or_else(|e| panic!("unuser: {}: {}", e.code, e.message));
    eprintln!("unuser: {d}");
    assert!(server::user_ids(&pool_user_name(2, 0)).is_none() && server::user_ids("k2job-2").is_none());
    assert!(r.pool().slot_users(2).is_empty());
    // Slot 1 again: its new pool user's uid is a new one.
    r.write_servers(&[]);
    r.prepare(1);
    let again = start(&r, 1, "w-again", "true", &[], true).unwrap();
    assert_eq!(again.index, Some(0));
    let (new, _) = pool_uid(1, 0);
    assert!(!old.contains(&new) && new > *old.iter().max().unwrap(), "a new uid ({new}), never one of {old:?}");
    finish(&r, 1, &again);
}
