//! The Linux tenant wall for real (0.46.3, S3a; prd-compute-shared-queue-v1
//! §8.5 W1–W9, plus Q9 and P7): one node, the owner server (slot 0) and
//! other servers (slots 1, 2), each server's jobs as its own user in its
//! own sandboxed unit.
//!
//! Needs root and systemd ≥ 247, and makes real users (`k2job-1`,
//! `k2t-1`, …) with the executor's own `tenant.prepare`, so every test is
//! `#[ignore]` and runs on purpose in a throwaway Linux box or container,
//! as root, one at a time:
//!
//!   cargo test -p k2-node --test tenant_wall_linux -- --ignored --test-threads=1
//!
//! Each test makes its own dir under /var/lib (the unit hides /tmp, /home
//! and /root) and wipes its tenants (users, groups, roots) when it ends.
//! The node runs in the test process (root) with the group of a real
//! node user, `k2nodet` (made once in the box), so what it creates is
//! `root:k2nodet` as a real node's is `k2node:k2node`; the owner's job
//! user is `nobody` with that group. Every wall check runs a control
//! first: the slot's own job (or root, outside any unit) reaches the thing,
//! so a pass means the wall made the difference. Any byte of another
//! server's secret in a job's output fails the test.

#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use k2_node::admin::exec::{ExecPaths, Executor, SystemPlatform};
use k2_node::admin::jobs::{JobAnswer, JobOp, JobStart};
use k2_node::admin::server::{self, PeerRule};
use k2_node::journal::JournalReader;
use k2_node::node::Tenant;
use k2_node::wall::Wall;
use k2_node_proto::frames::{JobState, LogStream, Plan, Src};
use k2_node_proto::share::{self, Network, ServersFile, ShareLimits, ShareState, Slot, TenantPrepare, TenantWipe, Trust};

// ── the rig ──────────────────────────────────────────────────────────

/// The test node user (a system user with its own group, made once in the
/// throwaway box) and its gid. The test process takes that group as its
/// effective gid, so the files the node writes carry it, as in life.
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
    slots: Vec<(Slot, Trust, Network)>,
    /// The node's real owner once a node runs here (its servers loop
    /// serves only a `servers.toml` naming it).
    owner_fp: std::sync::Mutex<String>,
}

impl Rig {
    fn home(&self) -> PathBuf {
        self.base.join("home")
    }

    fn layout(&self) -> k2_node::paths::Layout {
        k2_node::paths::Layout::new(self.home(), &self.config)
    }

    fn op(&self, op: &JobOp) -> JobAnswer {
        server::job_request(&self.socket, op, &[], Duration::from_secs(120)).unwrap_or_else(|e| panic!("{op:?}: {}: {}", e.code, e.message))
    }

    fn done(&self, op: &JobOp) -> serde_json::Value {
        match self.op(op) {
            JobAnswer::Done { detail } => detail,
            other => panic!("{op:?} answered {other:?}"),
        }
    }

    fn write_servers(&self, states: &[(Slot, ShareState)]) {
        let room = |l: ShareLimits| ShareLimits { max_running: 4, max_queued: 100, ..l };
        let owner = self.owner_fp.lock().unwrap().clone();
        let mut f = ServersFile::new(&owner, "owner.test", room(ShareLimits::owner(2, 8, 16, 100, Network::On)), 8, 1000, "local:test");
        for (slot, trust, net) in &self.slots {
            let mut e = f.servers[0].clone();
            e.slot = *slot;
            e.label = format!("server-{slot}.test");
            e.fp_prefix = stand_in_fp(*slot)[..16].to_string();
            e.trust = *trust;
            e.set_limits(&room(ShareLimits::new_server(*trust, false)));
            e.network = *net;
            e.state = states.iter().find(|(s, _)| s == slot).map(|(_, st)| *st).unwrap_or(ShareState::Active);
            f.servers.push(e);
        }
        f.validate().unwrap();
        std::fs::write(self.config.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
    }

    fn prepare(&self, slot: Slot) -> serde_json::Value {
        let (_, trust, network) = *self.slots.iter().find(|s| s.0 == slot).unwrap();
        self.done(&JobOp::TenantPrepare(TenantPrepare { slot, trust, weight: 1, network }))
    }

    fn uid_of(&self, slot: Slot) -> u32 {
        server::user_ids(&format!("k2job-{slot}")).unwrap_or_else(|| panic!("k2job-{slot} exists")).0
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Revoke every tenant, then wipe: users, groups, roots.
        let revoked: Vec<_> = self.slots.iter().map(|s| (s.0, ShareState::Revoked)).collect();
        self.write_servers(&revoked);
        for (slot, ..) in &self.slots {
            if let Err(e) = self.exec.tenant_wipe(&TenantWipe { slot: *slot }) {
                eprintln!("cleanup: wipe slot {slot}: {}: {}", e.code, e.message);
            }
        }
        let _ = std::fs::remove_dir_all(&self.base);
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// A node home + config + executor under /var/lib/k2-tenant-test-<id>,
/// `servers.toml` approving `slots`, each prepared by the executor's own
/// `tenant.prepare` (through the socket, as the node asks it).
fn rig(tag: &str, slots: &[(Slot, Trust, Network)]) -> Rig {
    assert_eq!(unsafe { libc::geteuid() }, 0, "the tenant wall tests run as root (cargo test … -- --ignored)");
    let id = k2_node_proto::crypto::random_id().unwrap();
    let base = PathBuf::from(format!("/var/lib/k2-tenant-test-{tag}-{}", &id[..8]));
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
    // The node's home tree exists before any server is added (as in life).
    k2_node::paths::Layout::new(base.join("home"), &config).ensure().unwrap();
    std::fs::write(config.join("admin.toml"), format!("remote_admin = \"off\"\nnode_user = \"{node_user}\"\n")).unwrap();
    k2_node::admin::jobs::write_jobs_file(&config, "nobody", &node_user).expect("nobody is a job user");
    std::fs::create_dir_all(base.join("bin")).unwrap();
    let bin = base.join("bin/k2-node");
    std::fs::copy(env!("CARGO_BIN_EXE_k2-node"), &bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let socket = PathBuf::from(format!("/run/k2na-tw-{}.sock", &id[..10]));
    let exec = Arc::new(Executor::new(
        ExecPaths { config_dir: config.clone(), state_dir: state, home: base.join("home"), binary: bin },
        Arc::new(SystemPlatform),
    ));
    let l = server::bind(&socket, None).unwrap();
    let e2 = exec.clone();
    std::thread::spawn(move || {
        let _ = server::serve(&e2, &PeerRule { allowed_uids: vec![0] }, l, &|| false);
    });
    let r = Rig { base, socket, config, exec, slots: slots.to_vec(), owner_fp: std::sync::Mutex::new("a".repeat(64)) };
    r.write_servers(&[]);
    for (slot, ..) in slots {
        let d = r.prepare(*slot);
        assert_eq!(d["user"], format!("k2job-{slot}"), "{d}");
    }
    r
}


/// Register every server with the node queue as S1 does (S2): an active
/// share (room for a few jobs), protocol 3, and for each tenant a session
/// stand-in (the owner has the fake's real one). The receivers are kept.
fn shares_up(n: &TestNode, slots: &[Slot]) -> Vec<tokio::sync::mpsc::Receiver<k2_node_proto::frames::Frame>> {
    let limits = ShareLimits { max_running: 4, max_queued: 100, ..ShareLimits::new_server(Trust::Own, cfg!(target_os = "macos")) };
    let mut keep = Vec::new();
    for s in std::iter::once(0u8).chain(slots.iter().copied()) {
        n.node.fairq.set_share(s, Some(k2_node::fairq::ShareInfo { label: format!("server-{s}"), limits: limits.clone(), status: k2_node_proto::share::ServerStatus::Active }));
        n.node.fairq.set_protocol(s, Some(3));
        if s != 0 {
            let (tx, rx) = tokio::sync::mpsc::channel(4096);
            n.node.tenant(s).unwrap().hub.attach(tx);
            keep.push(rx);
        }
    }
    keep
}

async fn node(fake: &Fake, r: &Rig, policy_extra: &str, tweak: impl FnOnce(&mut k2_node::node::NodeOptions)) -> TestNode {
    let sock = r.socket.clone();
    let n = start_node_in(fake, r.base.clone(), policy_extra, move |o| {
        o.wall = Some(Wall::JobUser { user: "nobody".into() });
        o.admin_socket = sock;
        o.unit_poll = Duration::from_millis(100);
        tweak(o);
    })
    .await;
    // The file names the real owner from here on (states kept).
    *r.owner_fp.lock().unwrap() = fake.key.fingerprint();
    let mut f: ServersFile = toml::from_str(&std::fs::read_to_string(r.config.join(share::SERVERS_FILE)).unwrap()).unwrap();
    f.owner = fake.key.fingerprint();
    f.servers[0].fp_prefix = fake.key.fingerprint();
    std::fs::write(r.config.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
    for (slot, ..) in &r.slots {
        k2_node::identity::write_pin(&n.layout.tenant(*slot).pin(), &fake.stand_in_pin(*slot, "node-1", "mini-1")).unwrap();
    }
    for (slot, ..) in &r.slots {
        tenant_opened(&n, *slot).await;
    }
    let slots: Vec<Slot> = r.slots.iter().map(|s| s.0).collect();
    // The stand-in sessions live as long as the test process (tests here
    // run one at a time).
    std::mem::forget(shares_up(&n, &slots));
    n
}

fn tenant(n: &TestNode, slot: Slot) -> Arc<Tenant> {
    if slot == 0 {
        n.node.owner()
    } else {
        n.node.tenant(slot).expect("tenant open")
    }
}

/// One finished job as the node recorded it.
struct Ran {
    state: JobState,
    reason: Option<String>,
    out: String,
    err: String,
    sys: String,
}

fn journal(t: &Tenant, job: &str) -> (String, String, String) {
    let mut r = JournalReader::new(t.paths.journal(job, 1));
    let (mut o, mut e, mut s) = (String::new(), String::new(), String::new());
    for rec in r.read_more(100_000).unwrap() {
        let txt = String::from_utf8_lossy(&rec.data).into_owned();
        match rec.stream {
            LogStream::Out => o.push_str(&txt),
            LogStream::Err => e.push_str(&txt),
            LogStream::Sys => s.push_str(&txt),
        }
    }
    (o, e, s)
}

fn start(n: &TestNode, slot: Slot, p: Plan) {
    let t = tenant(n, slot);
    let job = p.job_id.clone();
    assert_eq!(n.node.on_assign(&t, assign(p, 1)), Some((job.clone(), 1)), "slot {slot} job {job} starts");
}

async fn finished(n: &TestNode, slot: Slot, job: &str) -> Ran {
    let t = tenant(n, slot);
    let t0 = Instant::now();
    loop {
        let row = t.ledger.lock().unwrap().get(job, 1).unwrap();
        if let Some(r) = row.filter(|r| r.state.is_terminal() && r.receipt.is_some()) {
            let (out, err, sys) = journal(&t, job);
            return Ran { state: r.state, reason: r.reason, out, err, sys };
        }
        assert!(t0.elapsed() < Duration::from_secs(180), "slot {slot} job {job} never finished");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run(n: &TestNode, slot: Slot, job: &str, argv: &[&str]) -> Ran {
    start(n, slot, plan(job, argv));
    let r = finished(n, slot, job).await;
    assert_eq!(r.state, JobState::Done, "slot {slot} job {job}: {:?} {:?}\nout={}\nerr={}\nsys={}", r.state, r.reason, r.out, r.err, r.sys);
    r
}

/// Wait until a running job printed `word`.
async fn printed(n: &TestNode, slot: Slot, job: &str, word: &str) {
    let t = tenant(n, slot);
    let t0 = Instant::now();
    while !journal(&t, job).0.contains(word) {
        assert!(t0.elapsed() < Duration::from_secs(60), "slot {slot} job {job} never printed {word}: {:?}", journal(&t, job));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn unit_active(unit: &str) -> bool {
    let out = std::process::Command::new("systemctl").args(["show", "-p", "ActiveState", "--value", unit]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim() == "active"
}

/// `sh -c` that reads each path (`R <path> OK <bytes>` / `R <path> NO …`),
/// lists each `+dir`, and writes each `>path`.
const PROBE_SH: &str = r#"for p in "$@"; do
case "$p" in
  +*) d="${p#+}"; if c=$(ls -A -- "$d" 2>&1); then echo "L $d OK $c"; else echo "L $d NO $c"; fi ;;
  \>*) f="${p#>}"; if (echo poisoned > "$f") 2>/dev/null; then echo "W $f OK"; else echo "W $f NO"; fi ;;
  *) if c=$(cat -- "$p" 2>&1); then echo "R $p OK $c"; else echo "R $p NO $c"; fi ;;
esac
done"#;

fn probe_argv(items: &[String]) -> Vec<String> {
    let mut v = vec!["/bin/sh".to_string(), "-c".to_string(), PROBE_SH.to_string(), "probe".to_string()];
    v.extend(items.iter().cloned());
    v
}

fn as_strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

fn line_ok(out: &str, kind: &str, path: &str) -> bool {
    let want = format!("{kind} {path} ");
    let line = out.lines().find(|l| l.starts_with(&want)).unwrap_or_else(|| panic!("no {kind} line for {path} in:\n{out}"));
    line[want.len()..].starts_with("OK")
}

fn write_secret(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A slot's secrets as the node (root here) and its jobs leave them:
/// mirror, cargo registry, sccache dir, warm target slot, artifacts,
/// journal. (The job dir's secret is written by the slot's own job.)
fn plant(r: &Rig, slot: Slot) -> Vec<(PathBuf, String)> {
    let t = r.layout().tenant(slot);
    let pc = t.project_cache("pk");
    let v = vec![
        (t.mirror("pk").join("HEAD"), format!("SECRET-{slot}-mirror")),
        (pc.join("cargo-home/registry/cache/x.crate"), format!("SECRET-{slot}-registry")),
        (pc.join("sccache/a/b/obj"), format!("SECRET-{slot}-sccache")),
        (pc.join("target-1/release/app"), format!("SECRET-{slot}-target")),
        (t.artifacts().join("old-g1/dist/app.tar"), format!("SECRET-{slot}-artifact")),
        (t.journal("old", 1), format!("SECRET-{slot}-journal")),
    ];
    for (p, s) in &v {
        write_secret(p, s);
    }
    v
}

const SLOTS3: &[(Slot, Trust, Network)] = &[(1, Trust::Team, Network::On), (2, Trust::Own, Network::On)];

// ── W1, W2, W3 ───────────────────────────────────────────────────────

/// W1: no server's job reads another's mirror, cache, warm slot, job dir,
/// artifacts or journal by path (both ways: owner ↔ tenant, tenant ↔
/// tenant). W2: nor writes into another's cargo registry or sccache dir
/// (and the victim's files are unchanged after). W3: no tenant job reads
/// the node key, ledgers, `servers.toml` (or writes it), or a person's
/// home. Controls: each slot's own job reads its own secrets.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn w1_w2_w3_files_are_walled_between_servers_both_ways() {
    let r = rig("w1", SLOTS3);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |_| {}).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    // (a): the wall is really on: the executor's runtime self-test passed
    // and both servers have their users.
    r.exec.wall_selftest().expect("the tenant sandbox self-test passes here");
    assert!(n.node.tenant_wall_on().await, "tenant-wall is on with every server provisioned");
    let all: Vec<Slot> = vec![0, 1, 2];
    let mut secrets: Vec<(Slot, PathBuf, String)> = Vec::new();
    for &s in &all {
        for (p, text) in plant(&r, s) {
            secrets.push((s, p, text));
        }
        // The job dir's secret: written by the slot's own job, as its user.
        let job = format!("writer{s}");
        let w = run(&n, s, &job, &["/bin/sh", "-c", &format!("echo SECRET-{s}-jobdir > \"$HOME/secret\" && id -u")]).await;
        let want_uid = if s == 0 { server::user_ids("nobody").unwrap().0 } else { r.uid_of(s) };
        assert_eq!(w.out.trim(), want_uid.to_string(), "slot {s}'s job runs as its own user");
        secrets.push((s, r.layout().tenant(s).job_dir(&job, 1).join("home/secret"), format!("SECRET-{s}-jobdir")));
    }
    // Make the owner's node files world-readable on purpose: the
    // namespace wall must hold even where permissions wouldn't.
    for (s, p, _) in &secrets {
        if *s == 0 {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }
    for &b in &all {
        let mut items: Vec<String> = secrets.iter().map(|(_, p, _)| p.display().to_string()).collect();
        for &a in &all {
            let ta = r.layout().tenant(a);
            for d in [ta.root(), ta.repos(), ta.cache(), ta.jobs()] {
                items.push(format!("+{}", d.display()));
            }
            if a != b {
                // W2: poison A's registry and sccache dir.
                let pc = ta.project_cache("pk");
                items.push(format!(">{}", pc.join("cargo-home/registry/cache/poison.crate").display()));
                items.push(format!(">{}", pc.join("sccache/a/b/obj").display()));
            }
        }
        let job = format!("reader{b}");
        let got = run(&n, b, &job, &as_strs(&probe_argv(&items))).await;
        for (a, p, text) in &secrets {
            let ps = p.display().to_string();
            if *a == b {
                // The node's own state (artifacts, journal) is never the
                // job's, even its own server's: only its tree is.
                let node_only = p.starts_with(r.layout().state()) || *p == r.layout().tenant(0).journal("old", 1) || p.starts_with(r.home().join("artifacts"));
                if !node_only {
                    assert!(line_ok(&got.out, "R", &ps), "control: slot {b} reads its own {ps}:\n{}", got.out);
                }
            } else {
                assert!(!line_ok(&got.out, "R", &ps), "W1: slot {b}'s job read slot {a}'s {ps}:\n{}", got.out);
                assert!(!got.out.contains(text.as_str()) && !got.err.contains(text.as_str()), "W1: slot {a}'s secret {text} leaked to slot {b}");
            }
        }
        for &a in &all {
            if a == b {
                continue;
            }
            let ta = r.layout().tenant(a);
            for d in [ta.root(), ta.repos(), ta.cache(), ta.jobs()] {
                assert!(!line_ok(&got.out, "L", &d.display().to_string()), "W1: slot {b} listed slot {a}'s {}:\n{}", d.display(), got.out);
            }
            let pc = ta.project_cache("pk");
            for w in [pc.join("cargo-home/registry/cache/poison.crate"), pc.join("sccache/a/b/obj")] {
                assert!(!line_ok(&got.out, "W", &w.display().to_string()), "W2: slot {b} wrote {}:\n{}", w.display(), got.out);
            }
            assert!(!pc.join("cargo-home/registry/cache/poison.crate").exists(), "W2: nothing planted in slot {a}'s registry");
            assert_eq!(std::fs::read_to_string(pc.join("sccache/a/b/obj")).unwrap(), format!("SECRET-{a}-sccache"), "W2: slot {a}'s sccache entry unchanged");
        }
        // Its own: listable.
        let own = r.layout().tenant(b);
        assert!(line_ok(&got.out, "L", &own.cache().display().to_string()), "control: slot {b} lists its own cache:\n{}", got.out);
    }

    // W3: the node key, every ledger, servers.toml (read and write), and
    // a person's home: world-readable/-writable on purpose (the unit's
    // namespace is the wall, not the mode).
    let human = PathBuf::from(format!("/home/k2wall-human-{}", std::process::id()));
    write_secret(&human.join("notes.txt"), "SECRET-human");
    std::fs::set_permissions(&human, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(human.join("notes.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let l = r.layout();
    std::fs::set_permissions(l.state(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(l.state().join("tenants"), std::fs::Permissions::from_mode(0o755)).unwrap();
    for p in [l.key(), l.ledger(), l.tenant(1).ledger(), l.tenant(2).ledger()] {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    for d in [l.tenant(1).state(), l.tenant(2).state()] {
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let servers = r.config.join(share::SERVERS_FILE);
    std::fs::set_permissions(&servers, std::fs::Permissions::from_mode(0o666)).unwrap();
    let before = std::fs::read_to_string(&servers).unwrap();
    let reads = [l.key(), l.ledger(), l.tenant(1).ledger(), l.tenant(2).ledger(), servers.clone(), human.join("notes.txt")];
    let mut items: Vec<String> = reads.iter().map(|p| p.display().to_string()).collect();
    items.push(format!(">{}", servers.display()));
    for b in [1u8, 2] {
        let got = run(&n, b, &format!("w3-{b}"), &as_strs(&probe_argv(&items))).await;
        for p in &reads {
            assert!(!line_ok(&got.out, "R", &p.display().to_string()), "W3: slot {b} read {}:\n{}", p.display(), got.out);
        }
        assert!(!line_ok(&got.out, "W", &servers.display().to_string()), "W3: slot {b} wrote servers.toml:\n{}", got.out);
        assert!(!got.out.contains("SECRET-human") && !got.out.contains("PRIVATE KEY"));
    }
    assert_eq!(std::fs::read_to_string(&servers).unwrap(), before, "servers.toml untouched");
    let _ = std::fs::remove_dir_all(&human);
}

// ── W4, W5 ───────────────────────────────────────────────────────────

/// W4: another server's job's command line (a secret marker) never shows
/// in `ps` or `/proc/*/cmdline`. W5: its process can't be signalled
/// (`kill -0` → EPERM). Both ways, owner ↔ tenant and tenant ↔ tenant.
/// Control: root outside any unit sees the marker and signals it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn w4_w5_other_servers_processes_are_invisible_and_unsignalable() {
    let r = rig("w4", SLOTS3);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |_| {}).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    for (a, b) in [(1u8, 2u8), (0, 1), (1, 0)] {
        let marker = format!("SECRETMARKER{a}x{b}");
        let job = format!("victim{a}{b}");
        start(&n, a, plan(&job, &["/bin/sh", "-c", &format!("echo up; sleep 120; : {marker}")]));
        printed(&n, a, &job, "up").await;
        // Control: root sees it.
        let pid = find_pid(&marker).unwrap_or_else(|| panic!("control: root sees {marker} in /proc"));
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "control: root signals it");
        let script = format!(
            "ps -ax -o pid=,args= 2>&1; for f in /proc/[0-9]*/cmdline; do tr '\\0' ' ' < $f 2>/dev/null; echo; done; \
             if kill -0 {pid} 2>/tmp/k; then echo KILL-OK; else echo \"KILL-NO $(cat /tmp/k)\"; fi; \
             if cat /proc/{pid}/environ >/dev/null 2>&1; then echo ENV-OK; else echo ENV-NO; fi"
        );
        let got = run(&n, b, &format!("spy{a}{b}"), &["/bin/sh", "-c", &script]).await;
        assert!(!got.out.contains(&marker), "W4: slot {b} saw slot {a}'s command line:\n{}", got.out);
        assert!(got.out.contains("KILL-NO") && got.out.contains("Operation not permitted"), "W5: slot {b} could signal slot {a}'s pid {pid}:\n{}", got.out);
        assert!(got.out.contains("ENV-NO"), "slot {b} read slot {a}'s environ:\n{}", got.out);
        assert!(find_pid(&marker).is_some(), "slot {a}'s job is still running after the spy");
        n.node.cancel(&tenant(&n, a), &job, 1, "test_over");
        assert_eq!(finished(&n, a, &job).await.state, JobState::Cancelled);
    }
}

fn find_pid(marker: &str) -> Option<i32> {
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        if let Ok(b) = std::fs::read(e.path().join("cmdline")) {
            if String::from_utf8_lossy(&b).contains(marker) && !String::from_utf8_lossy(&b).contains("ps -ax") {
                return Some(pid);
            }
        }
    }
    None
}

// ── W6 ───────────────────────────────────────────────────────────────

const SEND_PY: &str = r#"
import json, socket, sys
r = {}
for a in sys.argv[1:]:
    kind, h, p = a.split(",")
    try:
        if kind == "udp":
            s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            s.sendto(b"k2", (h, int(p)))
        else:
            s = socket.create_connection((h, int(p)), timeout=5)
        r[a] = "ok"
    except OSError as e:
        r[a] = "errno%d" % (e.errno or 0)
print(json.dumps(r))
"#;

fn send_argv(targets: &[&str]) -> Vec<String> {
    let mut v = vec!["/usr/bin/python3".to_string(), "-c".to_string(), SEND_PY.to_string()];
    v.extend(targets.iter().map(|s| s.to_string()));
    v
}

fn send_direct(targets: &[&str]) -> serde_json::Value {
    let a = send_argv(targets);
    let out = std::process::Command::new(&a[0]).args(&a[1..]).output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// W6 (V62): `no-lan` closes 10/8, 172.16/12, 192.168/16, 169.254/16,
/// 100.64/10 (tailnet) and 224/4 (multicast) with EPERM, while the
/// internet and loopback stay open; `none` closes everything off the
/// machine. Control: root sends to every one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn w6_no_lan_and_none_close_the_network() {
    let r = rig("w6", &[(1, Trust::Team, Network::NoLan), (2, Trust::Team, Network::None)]);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |_| {}).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    let lo = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let lo_t = format!("tcp,127.0.0.1,{}", lo.local_addr().unwrap().port());
    let lan = ["udp,10.77.0.1,9", "udp,172.16.77.1,9", "udp,192.168.77.1,9", "udp,169.254.77.1,9", "udp,100.64.77.1,9", "udp,100.127.1.1,9", "udp,224.0.0.251,5353"];
    let public = ["udp,1.1.1.1,53", "tcp,1.1.1.1,443"];
    let mut all: Vec<&str> = lan.to_vec();
    all.extend(public);
    all.push(&lo_t);
    let control = send_direct(&all);
    for t in &all {
        assert_eq!(control[t], "ok", "control: root reaches {t}: {control}");
    }
    let got = run(&n, 1, "nolan", &as_strs(&send_argv(&all))).await;
    let v: serde_json::Value = serde_json::from_str(got.out.trim()).unwrap_or_else(|e| panic!("{e}: {} {}", got.out, got.err));
    for t in lan {
        assert_eq!(v[t], "errno1", "W6 no-lan: {t} must be EPERM: {v}");
    }
    for t in public {
        assert_eq!(v[t], "ok", "W6 no-lan keeps the internet: {t}: {v}");
    }
    assert_eq!(v[&lo_t], "ok", "no-lan keeps loopback: {v}");
    let got = run(&n, 2, "none", &as_strs(&send_argv(&all[..all.len() - 1]))).await;
    let v: serde_json::Value = serde_json::from_str(got.out.trim()).unwrap();
    for t in &all[..all.len() - 1] {
        assert_ne!(v[*t], "ok", "W6 none: {t} reached: {v}");
    }
    // The owner (network on) still reaches the LAN: the wall is the share's.
    let got = run(&n, 0, "owner-net", &as_strs(&send_argv(&lan))).await;
    let v: serde_json::Value = serde_json::from_str(got.out.trim()).unwrap();
    for t in lan {
        assert_eq!(v[t], "ok", "owner network on: {t}: {v}");
    }
}

// ── W7 ───────────────────────────────────────────────────────────────

fn pipes() -> (Vec<std::os::fd::OwnedFd>, Vec<std::os::fd::OwnedFd>) {
    let (r1, w1) = std::io::pipe().unwrap();
    let (r2, w2) = std::io::pipe().unwrap();
    (vec![r1.into(), r2.into()], vec![w1.into(), w2.into()])
}

fn start_raw(r: &Rig, s: JobStart) -> JobAnswer {
    use std::os::fd::AsRawFd;
    let (_keep, w) = pipes();
    let raw: Vec<i32> = w.iter().map(|f| f.as_raw_fd()).collect();
    server::job_request(&r.socket, &JobOp::Start(s), &raw, Duration::from_secs(60)).unwrap()
}

/// W7 (V48): another server assigns the owner's running job id and
/// generation: the owner's job keeps running (its unit untouched), and the
/// other server's attempt runs apart under its own opaque unit. At the
/// executor, a start naming the owner's key from a tenant slot, a start of
/// a key that's running, and a tenant reusing another tenant's key are all
/// refused; the running unit never stops.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn w7_a_colliding_job_id_never_touches_the_other_servers_job() {
    let r = rig("w7", SLOTS3);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |_| {}).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    start(&n, 0, plan("w7", &["/bin/sh", "-c", "echo owner-up; sleep 60"]));
    printed(&n, 0, "w7", "owner-up").await;
    let owner_unit = "k2job-w7-g1.service";
    assert!(unit_active(owner_unit));
    let got = run(&n, 1, "w7", &["/bin/sh", "-c", "echo tenant-ran"]).await;
    assert_eq!(got.out, "tenant-ran\n");
    assert!(unit_active(owner_unit), "W7: the owner's w7 survived the tenant's w7");
    let o = n.node.owner().ledger.lock().unwrap().get("w7", 1).unwrap().unwrap();
    assert!(!o.state.is_terminal(), "{:?}", o.state);
    // At the executor.
    let l = r.layout();
    let tkey = k2_node::node::job_key(1, "w7", 1);
    let t1_root = l.tenant(1).job_dir("w7", 1);
    let base = |key: &str, root: &Path, tenant: Slot| JobStart {
        key: key.into(),
        root: root.into(),
        rw: vec![],
        network_none: false,
        mem_bytes: None,
        cpu_millis: None,
        stop_grace_secs: 5,
        tenant,
        no_lan: false,
    };
    for (s, why) in [
        (base("w7-g1", &t1_root, 1), "a tenant naming the owner's key"),
        (base("w7-g1", &l.job_dir("w7", 1), 0), "a second start of a running key"),
        (base(&tkey, &l.tenant(2).jobs().join(&tkey), 2), "a tenant reusing another tenant's key"),
        (base("w7-g1", &l.job_dir("w7", 1), 1), "a tenant start in the owner's job dir"),
    ] {
        std::fs::create_dir_all(&s.root).unwrap();
        match start_raw(&r, s) {
            JobAnswer::Refused(e) => eprintln!("refused ({why}): {}: {}", e.code, e.message),
            other => panic!("W7: {why} was not refused: {other:?}"),
        }
        assert!(unit_active(owner_unit), "W7: {why} stopped the owner's unit");
    }
    n.node.cancel(&n.node.owner(), "w7", 1, "test_over");
    assert_eq!(finished(&n, 0, "w7").await.state, JobState::Cancelled);
}

// ── W8 ───────────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn src_plan(job: &str, url: &str, commit: &str) -> Plan {
    let mut p = plan(job, &["/bin/sh", "-c", "cat secret.txt"]);
    p.src = Some(Src { project_key: "pk".into(), remote_url: Some(url.into()), commit: commit.into(), dirty: None, slots: 1 });
    p
}

/// W8 (V49): a tenant's `remote_url` naming the owner's mirror (a path or
/// `file://`), a LAN address, or this machine is never fetched by the
/// node; a `no-lan` tenant never fetches at all: bundles only (here no
/// server answers, so the job fails to sync and the tenant's mirror stays
/// without the owner's commit). Control: the owner fetches the same path.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn w8_a_tenants_remote_is_https_public_or_bundles_only() {
    let r = rig("w8", &[(1, Trust::Team, Network::On), (2, Trust::Team, Network::NoLan)]);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |o| o.transfer_timeout = Duration::from_secs(3)).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    let repo = r.base.join("victim-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("secret.txt"), "SECRET-owner-source\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "secret"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    // Control: the owner's job fetches it from the path.
    let o = run_src(&n, 0, src_plan("own-fetch", &repo.display().to_string(), &sha)).await;
    assert_eq!(o.state, JobState::Done, "{}", o.sys);
    assert!(o.out.contains("SECRET-owner-source"), "control: {}", o.out);
    let owner_mirror = r.layout().tenant(0).mirror("pk");
    assert!(owner_mirror.join("HEAD").exists());
    for (slot, url, says) in [
        (1u8, owner_mirror.display().to_string(), "only https"),
        (1, format!("file://{}", owner_mirror.display()), "only https"),
        (1, format!("file://{}", repo.display()), "only https"),
        (1, "https://192.168.77.1/x.git".to_string(), "isn't a public address"),
        (1, "https://127.0.0.1/x.git".to_string(), "isn't a public address"),
        (1, "https://localhost/x.git".to_string(), "isn't a public address"),
        (1, "ssh://git@192.168.77.1/x.git".to_string(), "only https"),
        (2, "https://github.com/rust-lang/rust.git".to_string(), "bundles only"),
    ] {
        let job = format!("steal-{slot}-{}", k2_node_proto::crypto::random_id().unwrap().get(..6).unwrap().to_string());
        let got = run_src(&n, slot, src_plan(&job, &url, &sha)).await;
        assert_eq!((got.state, got.reason.as_deref()), (JobState::Failed, Some("sync_failed")), "W8 {url}: {}", got.sys);
        assert!(got.sys.contains(says), "W8 {url}: the log says why ({says}): {}", got.sys);
        assert!(!got.out.contains("SECRET-owner-source"), "W8 {url}: the owner's source reached slot {slot}");
        let tm = r.layout().tenant(slot).mirror("pk");
        let has = std::process::Command::new("git").args(["--git-dir", &tm.display().to_string(), "cat-file", "-e", &format!("{sha}^{{commit}}")]).stderr(std::process::Stdio::null()).status().unwrap();
        assert!(!has.success(), "W8 {url}: slot {slot}'s mirror got the owner's commit");
    }
}

async fn run_src(n: &TestNode, slot: Slot, p: Plan) -> Ran {
    let job = p.job_id.clone();
    start(n, slot, p);
    finished(n, slot, &job).await
}

// ── W9 ───────────────────────────────────────────────────────────────

/// W9 (V63): each server's builds get their own sccache server socket. A
/// running sccache of server A is unreachable from server B: not over A's
/// socket (B is refused before it connects), not over TCP 4226 (nobody
/// listens there); B's `sccache --show-stats` reaches only its own.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn w9_sccache_servers_are_per_server() {
    let sccache = ["/usr/local/bin/sccache", "/usr/bin/sccache"].into_iter().find(|p| Path::new(p).exists()).expect("W9 needs sccache ≥ 0.9 (SCCACHE_SERVER_UDS) installed");
    let r = rig("w9", SLOTS3);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |_| {}).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    for (a, b) in [(0u8, 1u8), (1, 0), (1, 2)] {
        let job = format!("sc{a}{b}");
        // A's build goes through its sccache once (its stats say so, and
        // name A's cache dir).
        start(
            &n,
            a,
            plan(&job, &["/bin/sh", "-c", &format!("{sccache} --start-server && echo 'int k2w9;' > t.c && {sccache} cc -c t.c -o t.o && echo started && echo SOCK=$SCCACHE_SERVER_UDS && sleep 60")]),
        );
        printed(&n, a, &job, "started").await;
        let a_sock = r.layout().tenant(a).sccache_socket();
        assert!(journal(&tenant(&n, a), &job).0.contains(&format!("SOCK={}", a_sock.display())), "slot {a}'s job got its slot's socket");
        let a_dir = r.layout().tenant(a).job_dir(&job, 1);
        // Control: root reaches A's server over its socket and sees A's
        // cache and its one compile.
        let ctl = std::process::Command::new(sccache).arg("--show-stats").env_clear().env("SCCACHE_SERVER_UDS", &a_sock).env("PATH", "/usr/bin:/bin").output().unwrap();
        let ctl_out = String::from_utf8_lossy(&ctl.stdout);
        assert!(ctl_out.contains(&a_dir.display().to_string()) && ctl_out.lines().any(|l| l.starts_with("Compile requests ") && l.trim_end().ends_with(" 1")), "control: root reads slot {a}'s stats: {ctl_out}");
        let script = format!(
            "{sccache} --show-stats >/dev/null 2>&1 && echo OWN-OK || echo OWN-NO; \
             echo STEAL; SCCACHE_SERVER_UDS={sock} timeout 20 {sccache} --show-stats 2>&1; echo STEAL-RC=$?; \
             python3 -c 'import socket,sys\ntry:\n  s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); print(\"UDS-OK\")\nexcept OSError as e: print(\"UDS-NO\", e.errno)' {sock}; \
             python3 -c 'import socket\ntry:\n  socket.create_connection((\"127.0.0.1\",4226),timeout=3); print(\"TCP-OK\")\nexcept OSError as e: print(\"TCP-NO\", e.errno)'",
            sock = a_sock.display()
        );
        let got = run(&n, b, &format!("steal{a}{b}"), &["/bin/sh", "-c", &script]).await;
        assert!(got.out.contains("OWN-OK"), "slot {b} has its own sccache: {}{}", got.out, got.err);
        let steal = got.out.split("STEAL").nth(1).unwrap_or("");
        assert!(!steal.contains(&a_dir.display().to_string()), "W9: slot {b} reached slot {a}'s sccache:\n{}", got.out);
        assert!(!steal.lines().any(|l| l.starts_with("Compile requests ") && !l.trim_end().ends_with(" 0")), "W9: slot {b} saw slot {a}'s compiles:\n{}", got.out);
        assert!(got.out.contains("UDS-NO") && got.out.contains("TCP-NO"), "W9: slot {b} connected to slot {a}'s server:\n{}", got.out);
        n.node.cancel(&tenant(&n, a), &job, 1, "test_over");
        finished(&n, a, &job).await;
    }
}

// ── Q9 ───────────────────────────────────────────────────────────────

/// Q9 (V58): one server's job that ignores SIGTERM doesn't delay another
/// server's start by more than 2 s, though the executor is releasing it
/// (with the old one-connection-at-a-time executor and a blocking
/// `systemctl stop`, the start waited the whole stop timeout).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn q9_a_job_ignoring_sigterm_doesnt_hold_up_another_servers_start() {
    let r = rig("q9", SLOTS3);
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |o| o.cancel_grace = Duration::from_secs(20)).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    start(&n, 1, plan("stubborn", &["/bin/sh", "-c", "trap '' TERM; echo up; while :; do sleep 1; done"]));
    printed(&n, 1, "stubborn", "up").await;
    let unit = format!("k2job-{}.service", k2_node::node::job_key(1, "stubborn", 1));
    assert!(unit_active(&unit));
    let sock = r.socket.clone();
    let u2 = unit.clone();
    let rel = std::thread::spawn(move || {
        let t0 = Instant::now();
        let a = server::job_request(&sock, &JobOp::Release { unit: u2 }, &[], Duration::from_secs(120));
        (t0.elapsed(), a)
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let t0 = Instant::now();
    start(&n, 2, plan("quick", &["/bin/sh", "-c", "echo quick"]));
    printed(&n, 2, "quick", "quick").await;
    let took = t0.elapsed();
    assert!(took < Duration::from_secs(2), "Q9: the other server's start took {took:?}");
    let (rel_took, a) = rel.join().unwrap();
    assert!(a.is_ok() && rel_took < Duration::from_secs(2), "release never blocks: {rel_took:?} {a:?}");
    // The stubborn job is killed at its stop timeout.
    let r1 = finished(&n, 1, "stubborn").await;
    assert!(r1.state.is_terminal());
}

// ── P7 and the basics ────────────────────────────────────────────────

/// P7 (V64): a wiped slot's user, group, root and node state are gone,
/// and the next server in that slot gets a new uid and sees nothing of
/// the old one. A live slot is never wiped; a revoked one is never
/// prepared.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn p7_a_wiped_slot_gets_a_new_uid_and_nothing_of_the_old_server() {
    let r = rig("p7", &[(1, Trust::Team, Network::On)]);
    let old = r.uid_of(1);
    let l = r.layout();
    write_secret(&l.tenant(1).cache().join("pk/old.txt"), "SECRET-old");
    l.tenant(1).ensure_state().unwrap();
    write_secret(&l.tenant(1).journal("j", 1), "SECRET-old-journal");
    write_secret(&l.tenant(1).pin(), "{}");
    match r.op(&JobOp::TenantWipe(TenantWipe { slot: 1 })) {
        JobAnswer::Refused(e) => assert_eq!(e.code, k2_node_proto::admin::code::BAD_ARGS, "an active slot is never wiped"),
        other => panic!("{other:?}"),
    }
    r.write_servers(&[(1, ShareState::Revoked)]);
    match r.op(&JobOp::TenantPrepare(TenantPrepare { slot: 1, trust: Trust::Team, weight: 1, network: Network::On })) {
        JobAnswer::Refused(e) => assert_eq!(e.code, share::codes::SHARE_REVOKED),
        other => panic!("a revoked slot was prepared: {other:?}"),
    }
    let d = r.done(&JobOp::TenantWipe(TenantWipe { slot: 1 }));
    eprintln!("wiped: {d}");
    assert!(server::user_ids("k2job-1").is_none() && k2_node::admin::jobs::group_id("k2t-1").is_none(), "user and group gone");
    assert!(!l.tenant(1).root().exists(), "the slot's root is gone");
    // Root never deletes in the node's own dirs (review A M1): the node
    // forgets its slot state itself once the line is gone (servers_e2e).
    for p in [l.tenant(1).state(), l.tenant(1).pin()] {
        assert!(p.exists(), "{} is the node's to forget", p.display());
    }
    assert!(k2_node::admin::jobs::read_jobs_file(&r.config).unwrap().unwrap().tenant(1).is_none());
    r.write_servers(&[]);
    let d = r.prepare(1);
    let new = r.uid_of(1);
    assert!(new > old, "P7: a reused slot gets a new uid ({old} → {new}): {d}");
    assert!(!l.tenant(1).cache().join("pk/old.txt").exists());
    assert_eq!(std::fs::read_dir(l.tenant(1).jobs()).unwrap().count(), 0);
}

/// The basics: a tenant's job runs as its own user, with its own group,
/// in its own slice, writes its own dirs (home, tmp, cwd, caches), and
/// reads the node's toolchains; on a slot nobody prepared, a tenant's job
/// never runs (`tenant_wall_missing`), and the tenant root is the node's
/// (V54).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn a_tenants_job_runs_as_its_own_user_in_its_own_dirs() {
    use std::os::unix::fs::MetadataExt;
    let r = rig("basic", &[(1, Trust::Own, Network::On), (3, Trust::Own, Network::On)]);
    let l = r.layout();
    let root = l.tenant(1).root();
    let m = std::fs::metadata(&root).unwrap();
    let node_uid = server::user_ids("k2nodet").unwrap().0;
    assert_eq!((m.uid(), m.mode() & 0o7777, m.gid()), (node_uid, 0o2750, r.uid_of(1)), "the node user owns the tenant root (V54)");
    assert_eq!(std::fs::metadata(l.tenants_root()).unwrap().mode() & 0o7777, 0o711);
    write_secret(&l.toolchains().join("bin/tool.txt"), "toolchain");
    std::fs::set_permissions(l.toolchains().join("bin"), std::fs::Permissions::from_mode(0o755)).unwrap();
    // Slot 3's user is gone behind the executor's back: its jobs never run.
    let u3 = r.uid_of(3);
    std::process::Command::new("userdel").arg("k2job-3").status().unwrap();
    let mut fake = Fake::start().await;
    let n = node(&fake, &r, "", |_| {}).await;
    let mut _c = fake.next_conn().await;
    ready(&mut _c).await;
    assert!(!n.node.tenant_wall_on().await, "an approved server without its user: tenant-wall is off");
    let got = run(
        &n,
        1,
        "basic",
        &[
            "/bin/sh",
            "-c",
            "id -u; id -g; echo h > \"$HOME/f\" && echo t > \"$TMPDIR/f\" && echo c > ./f && mkdir -p \"$CARGO_HOME/registry\" && echo r > \"$CARGO_HOME/registry/f\" && echo wrote; \
             cat /proc/self/cgroup; echo USER=$USER; echo UDS=$SCCACHE_SERVER_UDS",
        ],
    )
    .await;
    let uid = r.uid_of(1);
    let lines: Vec<&str> = got.out.lines().collect();
    assert_eq!((lines[0], lines[1], lines[2]), (uid.to_string().as_str(), uid.to_string().as_str(), "wrote"), "{}", got.out);
    assert!(got.out.contains("k2-jobs-t1.slice"), "its own slice: {}", got.out);
    assert!(got.out.contains("USER=k2job-1"));
    assert!(got.out.contains(&format!("UDS={}", l.tenant(1).sccache_socket().display())));
    let tc = run(&n, 1, "tool", &["/bin/cat", &l.toolchains().join("bin/tool.txt").display().to_string()]).await;
    assert_eq!(tc.out, "toolchain", "the node's toolchains are readable");
    let w = run(&n, 1, "tool-w", &["/bin/sh", "-c", &format!("echo x > {} && echo WROTE || echo REFUSED", l.toolchains().join("bin/x").display())]).await;
    assert!(w.out.contains("REFUSED"), "toolchains are read-only: {}", w.out);
    start(&n, 3, plan("nouser", &["/bin/echo", "never"]));
    let got = finished(&n, 3, "nouser").await;
    assert_eq!((got.state, got.reason.as_deref()), (JobState::Failed, Some(share::codes::TENANT_WALL_MISSING)), "{}", got.sys);
    assert!(!got.out.contains("never"));
    let _ = u3;
    // The wall watch holds that server's queue (S2 `set_blocked`) and
    // leaves the walled one alone: its next job waits, never runs.
    let held = n.node.tenant_wall_pass().await;
    assert_eq!(held.into_iter().collect::<Vec<_>>(), vec![(3u8, share::codes::TENANT_WALL_MISSING.to_string())]);
    start(&n, 3, plan("held", &["/bin/echo", "never"]));
    tokio::time::sleep(k2_node::fairq::TICK * 2).await;
    let row = tenant(&n, 3).ledger.lock().unwrap().get("held", 1).unwrap().unwrap();
    assert_eq!(row.state, JobState::Queued, "a held server's job waits in the queue");
    assert!(journal(&tenant(&n, 3), "held").0.is_empty());
    let ok = run(&n, 1, "still", &["/bin/echo", "walled"]).await;
    assert_eq!(ok.out, "walled\n", "the other server runs on");
}

/// S1 × S3 (V64): `servers.add` as root has root's `tenant.prepare` make
/// the user and records its uid in the server's `servers.toml` line (the
/// same uid jobs.toml holds); revoke `--wipe` has `tenant.wipe` remove the
/// user; the next server in that slot gets a new uid, recorded too.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd ≥ 247: run in a throwaway Linux box with --ignored"]
async fn servers_add_prepares_the_user_and_records_its_uid() {
    use k2_node::admin::servers;
    let r = rig("add", &[]);
    let (node_user, _) = node_test_user();
    let ctx = servers::Ctx {
        config_dir: r.config.clone(),
        state_dir: r.base.join("admin-state"),
        home: r.home(),
        now: k2_node::util::now(),
        macos: false,
        walled: true,
        tenant_wall: false,
        pinned_owner: None,
        node_user,
        by: "local:test".into(),
        root: true,
    };
    let add = |url: &str, prefix: &str| share::AddArgs {
        url: url.into(),
        code_binding: "binding-b64".into(),
        fp_prefix: prefix.into(),
        trust: Trust::Team,
        for_secs: None,
        limits: Default::default(),
    };
    let a = servers::add(&ctx, &add("https://a.test", "0123456789abcdef")).unwrap();
    let slot = a["slot"].as_u64().unwrap() as Slot;
    let uid = r.uid_of(slot);
    assert_eq!(a["uid"].as_u64(), Some(uid as u64), "{a}");
    let line_uid = |f: &ServersFile| f.entry(slot).and_then(|e| e.uid);
    assert_eq!(line_uid(&servers::read_file(&r.config).unwrap().unwrap()), Some(uid), "servers.toml records the user's uid");
    assert_eq!(k2_node::admin::jobs::read_jobs_file(&r.config).unwrap().unwrap().tenant(slot).map(|t| t.uid), Some(uid));

    servers::revoke(&ctx, &share::RevokeArgs { server: format!("slot:{slot}"), now: true, wipe: true }).unwrap();
    assert!(server::user_ids(&format!("k2job-{slot}")).is_none(), "revoke --wipe removed the user (tenant.wipe)");
    assert!(servers::read_file(&r.config).unwrap().unwrap().entry(slot).is_none());

    let b = servers::add(&ctx, &add("https://b.test", "1123456789abcdef")).unwrap();
    assert_eq!(b["slot"].as_u64(), Some(slot as u64), "the slot is reused");
    let again = r.uid_of(slot);
    assert!(again > uid, "a reused slot never gets an old uid ({uid} → {again})");
    assert_eq!(line_uid(&servers::read_file(&r.config).unwrap().unwrap()), Some(again));
    // The rig's cleanup wipes only its own slots: this one goes here.
    let mut f = servers::read_file(&r.config).unwrap().unwrap();
    f.servers.iter_mut().filter(|e| e.slot == slot).for_each(|e| e.state = ShareState::Revoked);
    servers::write_file(&r.config, &f).unwrap();
    r.exec.tenant_wipe(&TenantWipe { slot }).unwrap();
}
