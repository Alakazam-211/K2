//! The macOS tenant wall for real (0.46.3, S3m; prd-compute-shared-queue-v1
//! §8.2, §8.5 W1–W3, W6, W8, W9): one uid for every job, and a Seatbelt
//! profile per server so each server's jobs reach only their own tenant
//! dirs. Macs take only your own servers in 0.46.3 (`team` refused).
//!
//! No root needed: the node, its home and the tenant roots are temp dirs
//! of the user running the tests, and `sandbox-exec` is the real one.
//! Every check runs a control (the slot's own job, or the test process
//! outside any sandbox, reaches the thing), and any byte of another
//! server's secret in a job's output fails the test.
//!
//! Seatbelt doesn't nest: inside a `k2 compute` job (already under the
//! node's profile) `sandbox-exec` fails with `sandbox_apply: Operation not
//! permitted`. Run these on a Mac outside a job.
//!
//!   cargo test -p k2-node --test tenant_wall_macos
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

#![cfg(target_os = "macos")]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use k2_node::journal::JournalReader;
use k2_node::node::Tenant;
use k2_node::wall::Wall;
use k2_node_proto::frames::{JobState, LogStream, Plan};
use k2_node_proto::share::{self, codes, Network, ServersFile, ShareLimits, Slot, Trust};

struct Mac {
    n: TestNode,
    _fake: Fake,
    _conn: Conn,
    _sessions: Vec<tokio::sync::mpsc::Receiver<k2_node_proto::frames::Frame>>,
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


/// `servers.toml` as the node owner wrote it: the real owner (`owner_fp`)
/// and each stand-in server's line (its pin names [`stand_in_fp`]), with
/// the stand-ins' queue room, so the node's servers loop keeps them.
fn write_servers(config: &Path, owner_fp: &str, slots: &[(Slot, Trust, Network)]) {
    let room = |l: ShareLimits| ShareLimits { max_running: 4, max_queued: 100, ..l };
    let mut f = ServersFile::new(owner_fp, "owner.test", room(ShareLimits::owner(2, 8, 16, 100, Network::On)), 8, 1000, "local:test");
    for (slot, trust, net) in slots {
        let mut e = f.servers[0].clone();
        e.slot = *slot;
        e.label = format!("server-{slot}.test");
        e.fp_prefix = stand_in_fp(*slot)[..16].to_string();
        e.trust = *trust;
        e.set_limits(&room(ShareLimits::new_server(*trust, true)));
        e.network = *net;
        f.servers.push(e);
    }
    f.validate().unwrap();
    std::fs::write(config.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
}

/// A Seatbelt node with the owner and `slots`, each tenant root made as
/// `tenant.prepare` makes it on macOS (the node user's, 0700).
async fn mac(tag: &str, slots: &[(Slot, Trust, Network)], prepare: bool) -> Mac {
    mac_in(k2_node::util::temp_dir(tag), slots, prepare).await
}

/// A short base dir (this user's `DARWIN_USER_TEMP_DIR`): socket paths
/// under it fit `sun_path` (104 bytes), as the real `/var/k2node-t/<n>/…` do.
fn short_dir() -> PathBuf {
    let out = std::process::Command::new("/usr/bin/getconf").arg("DARWIN_USER_TEMP_DIR").output().unwrap();
    let base = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let d = base.join(format!("k2w-{}", &k2_node_proto::crypto::random_id().unwrap()[..4]));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

async fn mac_in(dir: PathBuf, slots: &[(Slot, Trust, Network)], prepare: bool) -> Mac {
    assert!(k2_node::wall::seatbelt_available(), "sandbox-exec is missing");
    let nests = std::process::Command::new(k2_node::wall::SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)(deny file-read* (subpath \"/nonexistent-k2\"))", "--", "/usr/bin/true"])
        .output()
        .unwrap();
    assert!(nests.status.success(), "these tests can't run under another sandbox (a k2 compute job?): {}", String::from_utf8_lossy(&nests.stderr));
    let mut fake = Fake::start().await;
    let n = start_node_in(&fake, dir, "", |o| o.wall = Some(Wall::Seatbelt)).await;
    let mut conn = fake.next_conn().await;
    ready(&mut conn).await;
    write_servers(&n.layout.config, &fake.key.fingerprint(), slots);
    for (slot, ..) in slots {
        if prepare {
            let t = n.layout.tenant(*slot);
            for d in [t.root(), t.jobs(), t.cache(), t.repos(), t.tmp()] {
                k2_node::util::mkdir_mode(&d, 0o700).unwrap();
            }
        }
        k2_node::identity::write_pin(&n.layout.tenant(*slot).pin(), &fake.stand_in_pin(*slot, "node-1", "mini-1")).unwrap();
    }
    for (slot, ..) in slots {
        tenant_opened(&n, *slot).await;
    }
    let slots: Vec<Slot> = slots.iter().map(|s| s.0).collect();
    let sessions = shares_up(&n, &slots);
    Mac { n, _fake: fake, _conn: conn, _sessions: sessions }
}

fn tenant(n: &TestNode, slot: Slot) -> Arc<Tenant> {
    if slot == 0 {
        n.node.owner()
    } else {
        n.node.tenant(slot).unwrap()
    }
}

struct Ran {
    state: JobState,
    reason: Option<String>,
    out: String,
    err: String,
}

fn journal(t: &Tenant, job: &str) -> (String, String) {
    let mut r = JournalReader::new(t.paths.journal(job, 1));
    let (mut o, mut e) = (String::new(), String::new());
    for rec in r.read_more(100_000).unwrap() {
        let s = String::from_utf8_lossy(&rec.data).into_owned();
        match rec.stream {
            LogStream::Out => o.push_str(&s),
            LogStream::Err => e.push_str(&s),
            LogStream::Sys => e.push_str(&s),
        }
    }
    (o, e)
}

async fn finished(n: &TestNode, slot: Slot, job: &str) -> Ran {
    let t = tenant(n, slot);
    let t0 = Instant::now();
    loop {
        if let Some(r) = t.ledger.lock().unwrap().get(job, 1).unwrap().filter(|r| r.state.is_terminal() && r.receipt.is_some()) {
            let (out, err) = journal(&t, job);
            return Ran { state: r.state, reason: r.reason, out, err };
        }
        assert!(t0.elapsed() < Duration::from_secs(120), "slot {slot} job {job} never finished");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run_plan(n: &TestNode, slot: Slot, p: Plan) -> Ran {
    let job = p.job_id.clone();
    assert_eq!(n.node.on_assign(&tenant(n, slot), assign(p, 1)), Some((job.clone(), 1)));
    finished(n, slot, &job).await
}

async fn run(n: &TestNode, slot: Slot, job: &str, argv: &[&str]) -> Ran {
    let r = run_plan(n, slot, plan(job, argv)).await;
    assert_eq!(r.state, JobState::Done, "slot {slot} job {job}: {:?} {:?}\nout={}\nerr={}", r.state, r.reason, r.out, r.err);
    r
}

const PROBE_SH: &str = r#"for p in "$@"; do
case "$p" in
  +*) d="${p#+}"; if c=$(ls -A -- "$d" 2>&1); then echo "L $d OK $c"; else echo "L $d NO $c"; fi ;;
  \>*) f="${p#>}"; if (echo poisoned > "$f") 2>/dev/null; then echo "W $f OK"; else echo "W $f NO"; fi ;;
  *) if c=$(cat -- "$p" 2>&1); then echo "R $p OK $c"; else echo "R $p NO $c"; fi ;;
esac
done"#;

fn probe(items: &[String]) -> Vec<String> {
    let mut v = vec!["/bin/sh".to_string(), "-c".to_string(), PROBE_SH.to_string(), "probe".to_string()];
    v.extend(items.iter().cloned());
    v
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

fn line_ok(out: &str, kind: &str, path: &str) -> bool {
    let want = format!("{kind} {path} ");
    let line = out.lines().find(|l| l.starts_with(&want)).unwrap_or_else(|| panic!("no {kind} line for {path} in:\n{out}"));
    line[want.len()..].starts_with("OK")
}

fn put(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

/// The k2-node binary (its `wall-probe`), copied out of the build tree.
fn probe_bin() -> PathBuf {
    let d = k2_node::util::temp_dir("probe-bin");
    let p = d.join("k2-node");
    std::fs::copy(env!("CARGO_BIN_EXE_k2-node"), &p).unwrap();
    p
}

const OWN2: &[(Slot, Trust, Network)] = &[(1, Trust::Own, Network::On), (2, Trust::Own, Network::On)];

/// W1 (and V61): the owner's jobs can't read a tenant root; a tenant's
/// can't read the owner's (node home: mirrors, caches, jobs) or another
/// tenant's, only its own. W2: no writes into another server's cargo
/// registry or sccache dir. W3: no tenant job reads the node key, the
/// ledgers or the config, or lists /Users; `/private/tmp` is closed to
/// tenants. Controls: each slot's own job reads its own.
#[tokio::test(flavor = "multi_thread")]
async fn w1_w2_w3_the_profile_walls_each_servers_dirs_both_ways() {
    let m = mac("mac-w1", OWN2, true).await;
    let n = &m.n;
    assert!(!n.node.tenant_wall_on().await, "macOS 0.46.3 never offers tenant-wall (one uid)");
    let l = &n.layout;
    let mut secrets: Vec<(Slot, PathBuf, String, bool)> = Vec::new();
    for s in [0u8, 1, 2] {
        let t = l.tenant(s);
        let pc = t.project_cache("pk");
        for (p, text) in [
            (t.mirror("pk").join("HEAD"), format!("SECRET-{s}-mirror")),
            (pc.join("cargo-home/registry/cache/x.crate"), format!("SECRET-{s}-registry")),
            (pc.join("sccache/a/obj"), format!("SECRET-{s}-sccache")),
            (pc.join("target-1/app"), format!("SECRET-{s}-target")),
        ] {
            put(&p, &text);
            secrets.push((s, p, text, true));
        }
        for (p, text) in [(t.artifacts().join("old-g1/a.tar"), format!("SECRET-{s}-artifact")), (t.journal("old", 1), format!("SECRET-{s}-journal"))] {
            put(&p, &text);
            // The owner's journal and artifacts are in its home tree; a
            // tenant's are node-only state. Only the owner's own job may
            // still reach its home's copies (today's behaviour).
            secrets.push((s, p, text, false));
        }
        let job = format!("writer{s}");
        run(n, s, &job, &["/bin/sh", "-c", &format!("echo SECRET-{s}-jobdir > \"$HOME/secret\"")]).await;
        secrets.push((s, t.job_dir(&job, 1).join("home/secret"), format!("SECRET-{s}-jobdir"), true));
    }
    for b in [0u8, 1, 2] {
        let mut items: Vec<String> = secrets.iter().map(|(_, p, ..)| p.display().to_string()).collect();
        for a in [0u8, 1, 2] {
            let ta = l.tenant(a);
            for d in [ta.repos(), ta.cache(), ta.jobs()] {
                items.push(format!("+{}", d.display()));
            }
            if a != b {
                let pc = ta.project_cache("pk");
                items.push(format!(">{}", pc.join("cargo-home/registry/cache/poison.crate").display()));
                items.push(format!(">{}", pc.join("sccache/a/obj").display()));
            }
        }
        let got = run(n, b, &format!("reader{b}"), &strs(&probe(&items))).await;
        for (a, p, text, own_reads) in &secrets {
            let ps = p.display().to_string();
            if *a == b {
                if *own_reads {
                    assert!(line_ok(&got.out, "R", &ps), "control: slot {b} reads its own {ps}:\n{}", got.out);
                }
            } else {
                assert!(!line_ok(&got.out, "R", &ps), "W1: slot {b} read slot {a}'s {ps}:\n{}", got.out);
                assert!(!got.out.contains(text.as_str()), "W1: {text} leaked to slot {b}");
            }
        }
        for a in [0u8, 1, 2].into_iter().filter(|a| *a != b) {
            let ta = l.tenant(a);
            for d in [ta.repos(), ta.cache(), ta.jobs()] {
                assert!(!line_ok(&got.out, "L", &d.display().to_string()), "W1: slot {b} listed slot {a}'s {}:\n{}", d.display(), got.out);
            }
            let pc = ta.project_cache("pk");
            assert!(!pc.join("cargo-home/registry/cache/poison.crate").exists(), "W2: slot {b} planted into slot {a}'s registry");
            assert_eq!(std::fs::read_to_string(pc.join("sccache/a/obj")).unwrap(), format!("SECRET-{a}-sccache"), "W2");
        }
        assert!(line_ok(&got.out, "L", &l.tenant(b).cache().display().to_string()), "control: slot {b} lists its own cache:\n{}", got.out);
    }
    // W3 (tenants): the node's key, ledgers and config, /Users, /private/tmp.
    let scratch = PathBuf::from(format!("/private/tmp/k2-mac-w3-{}", std::process::id()));
    put(&scratch.join("f"), "SECRET-shared-tmp");
    let reads = [l.key(), l.ledger(), l.tenant(2).ledger(), l.config.join(share::SERVERS_FILE), scratch.join("f")];
    let mut items: Vec<String> = reads.iter().map(|p| p.display().to_string()).collect();
    items.push("+/Users".into());
    items.push(format!(">{}", l.config.join(share::SERVERS_FILE).display()));
    items.push(format!(">{}", scratch.join("planted").display()));
    let got = run(n, 1, "w3", &strs(&probe(&items))).await;
    for p in &reads {
        assert!(!line_ok(&got.out, "R", &p.display().to_string()), "W3: slot 1 read {}:\n{}", p.display(), got.out);
    }
    assert!(!line_ok(&got.out, "L", "/Users"), "W3: slot 1 listed /Users");
    assert!(!line_ok(&got.out, "W", &l.config.join(share::SERVERS_FILE).display().to_string()), "W3: servers.toml written");
    assert!(!scratch.join("planted").exists(), "a tenant can't plant into /private/tmp");
    let _ = std::fs::remove_dir_all(&scratch);
    // Toolchains: readable, not writable.
    put(&l.toolchains().join("bin/tool.txt"), "toolchain");
    let got = run(n, 1, "tool", &strs(&probe(&[l.toolchains().join("bin/tool.txt").display().to_string(), format!(">{}", l.toolchains().join("bin/x").display())]))).await;
    assert!(line_ok(&got.out, "R", &l.toolchains().join("bin/tool.txt").display().to_string()), "toolchains readable: {}", got.out);
    assert!(!l.toolchains().join("bin/x").exists(), "toolchains read-only for tenants");
}

/// W9 (V63) on one uid: a server's job connects to its own sccache socket
/// only; the profile closes every socket under another server's root (or
/// the node home). Control: the test process connects to all of them.
#[tokio::test(flavor = "multi_thread")]
async fn w9_sockets_in_another_servers_root_are_closed() {
    let m = mac_in(short_dir(), OWN2, true).await;
    let n = &m.n;
    let l = &n.layout;
    k2_node::util::mkdir_mode(&l.tenant(0).tmp(), 0o700).unwrap();
    let mut keep = Vec::new();
    for s in [0u8, 1, 2] {
        let sock = l.tenant(s).sccache_socket();
        let _ = std::fs::remove_file(&sock);
        keep.push(std::os::unix::net::UnixListener::bind(&sock).unwrap());
        assert!(std::os::unix::net::UnixStream::connect(&sock).is_ok(), "control connects to {}", sock.display());
    }
    for b in [0u8, 1, 2] {
        let mut script = String::from("echo UDS=$SCCACHE_SERVER_UDS; ");
        for a in [0u8, 1, 2] {
            script.push_str(&format!("if /usr/bin/nc -U -w 1 {p} </dev/null 2>/dev/null; then echo C{a}-OK; else echo C{a}-NO; fi; ", p = l.tenant(a).sccache_socket().display()));
        }
        let got = run(n, b, &format!("uds{b}"), &["/bin/sh", "-c", &script]).await;
        assert!(got.out.contains(&format!("UDS={}", l.tenant(b).sccache_socket().display())), "slot {b}'s own socket: {}", got.out);
        for a in [0u8, 1, 2] {
            let want = if a == b { format!("C{a}-OK") } else { format!("C{a}-NO") };
            assert!(got.out.contains(&want), "W9: slot {b} → slot {a}'s socket: {}\n{}", got.out, got.err);
        }
    }
}

/// W6: a tenant's `no-lan` share is `none` on macOS (Seatbelt can't close
/// only the LAN): a socket off this machine is denied, loopback stays.
/// The owner (network on) still connects. And `team` is refused on Macs
/// (`team_needs_tenant_wall`); an unprepared slot never runs.
#[tokio::test(flavor = "multi_thread")]
async fn w6_no_lan_is_none_and_team_is_refused_on_a_mac() {
    let m = mac("mac-w6", &[(1, Trust::Own, Network::NoLan), (2, Trust::Team, Network::None), (3, Trust::Own, Network::On)], true).await;
    let n = &m.n;
    let lo = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let lo_a = lo.local_addr().unwrap().to_string();
    // TEST-NET-1: off this machine, never a real host.
    let off = "192.0.2.1:9".to_string();
    let bin = probe_bin();
    let argv = vec![bin.display().to_string(), "wall-probe".into(), "--connect".into(), off.clone(), "--connect".into(), lo_a.clone()];
    let denied = |v: &serde_json::Value, a: &str| v["connect"][a].as_str().unwrap_or("").contains("Operation not permitted");
    let got = run(n, 1, "nolan", &strs(&argv)).await;
    let v: serde_json::Value = serde_json::from_str(got.out.trim()).unwrap_or_else(|e| panic!("{e}: {} {}", got.out, got.err));
    assert!(denied(&v, &off), "W6: no-lan let a socket leave the Mac: {v}");
    assert_eq!(v["connect"][&lo_a], "ok", "loopback stays: {v}");
    let got = run(n, 0, "owner-net", &strs(&argv)).await;
    let v: serde_json::Value = serde_json::from_str(got.out.trim()).unwrap();
    assert!(!denied(&v, &off), "the owner's network is on: {v}");
    let got = run_plan(n, 2, plan("team", &["/bin/echo", "never"])).await;
    assert_eq!((got.state, got.reason.as_deref()), (JobState::Failed, Some(codes::TEAM_NEEDS_TENANT_WALL)), "{}", got.err);
    assert!(!got.out.contains("never"));
    let t3 = n.layout.tenant(3);
    std::fs::remove_dir_all(t3.root()).unwrap();
    let got = run_plan(n, 3, plan("unprepared", &["/bin/echo", "never"])).await;
    assert_eq!((got.state, got.reason.as_deref()), (JobState::Failed, Some(codes::TENANT_WALL_MISSING)), "{}", got.err);
    // The wall watch holds both servers' queues (S2 `set_blocked`).
    let held = n.node.tenant_wall_pass().await;
    assert_eq!(
        held.into_iter().collect::<Vec<_>>(),
        vec![(2u8, codes::TEAM_NEEDS_TENANT_WALL.to_string()), (3, codes::TENANT_WALL_MISSING.to_string())],
        "slot 1 (own, prepared) runs on"
    );
}

/// T4 on a Mac (W4 is Linux's): one uid and no `process-info*` rule in
/// the tenant profile, so it changes process visibility no more than the
/// owner's (N3) profile: both see (or both don't see) a child the same
/// way, and both signal their own children. Other jobs' processes are not
/// hidden on a Mac in 0.46.3 (in the report; own servers only). (On a
/// mini under the node's own sandbox, `ps -p` of a child says nothing in
/// either profile, so `ps` can't show more here.)
#[tokio::test(flavor = "multi_thread")]
async fn w4_a_tenants_profile_hides_no_more_than_the_owners_on_a_mac() {
    let m = mac("mac-w4", OWN2, true).await;
    let n = &m.n;
    let script = "/bin/sleep 30 & c=$!; ps -p $c -o command= 2>&1 | grep -q sleep && echo CHILD-SEEN || echo CHILD-HIDDEN; kill $c && echo KILLED";
    let owner = run(n, 0, "ps-owner", &["/bin/sh", "-c", script]).await;
    let tenant = run(n, 1, "ps-tenant", &["/bin/sh", "-c", script]).await;
    eprintln!("owner: {:?} tenant: {:?}", owner.out, tenant.out);
    assert!(tenant.out.contains("KILLED") && owner.out.contains("KILLED"), "a job signals its own children: {} / {}", owner.out, tenant.out);
    assert_eq!(tenant.out, owner.out, "the tenant profile changes process visibility no more than the owner's");
}

/// One shell check per line of the output: `<tag> OK <output>` or
/// `<tag> NO <output>`.
fn check_script(checks: &[(&str, Vec<String>)]) -> String {
    let mut s = String::from("c() { tag=$1; shift; if o=$(\"$@\" 2>&1); then echo \"$tag OK $o\"; else echo \"$tag NO $o\"; fi; }\n");
    for (tag, argv) in checks {
        s.push_str(&format!("c {tag}"));
        for a in argv {
            assert!(!a.contains('\''), "a test path with a quote: {a}");
            s.push_str(&format!(" '{a}'"));
        }
        s.push('\n');
    }
    s
}

fn checked<'a>(out: &'a str, tag: &str) -> (bool, &'a str) {
    let line = out.lines().find(|l| l.split_whitespace().next() == Some(tag)).unwrap_or_else(|| panic!("no check {tag} in:\n{out}"));
    let rest = &line[tag.len() + 1..];
    (rest.starts_with("OK"), rest[2..].trim())
}

fn argv(args: &[&str], paths: &[&Path]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).chain(paths.iter().map(|p| p.display().to_string())).collect()
}

/// A tenant's job walks its own paths through the two directories above
/// them (a real node: `/var/k2node-t/<slot>/…`, `/var/k2node/toolchains/…`):
/// `realpath`, `mkdir -p`, `cd` and `stat` work, in both spellings of the
/// path. Those two directories give it their own metadata and nothing else:
/// no listing, nothing inside another slot or the node home is read or even
/// stat'ed, and nothing is written. Control: the test process, outside the
/// sandbox, reaches everything the job is refused.
#[tokio::test(flavor = "multi_thread")]
async fn w10_a_tenant_walks_its_own_paths_and_learns_nothing_else_from_the_parents() {
    // Under /var/folders: `/var` is a symlink to `/private/var`, as on a node.
    let m = mac_in(short_dir(), OWN2, true).await;
    let n = &m.n;
    let l = &n.layout;
    let troot = l.tenants_root();
    let home = l.home.clone();
    assert!(troot.starts_with("/private/var/"), "the test needs the /var spelling: {}", troot.display());
    let var = |p: &Path| PathBuf::from("/var").join(p.strip_prefix("/private/var").unwrap());

    let own = l.tenant(1).job_dir("w10", 1).join("home");
    let other = troot.join("2");
    let secret = l.tenant(2).cache().join("secret");
    put(&secret, "SECRET-slot-2");
    let loose = home.join("loose");
    put(&loose, "SECRET-home");
    let tool = l.toolchains().join("bin/tool.txt");
    put(&tool, "toolchain");
    let mode = |p: &Path| std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(p).unwrap().permissions());
    let (root_mode, home_mode) = (mode(&troot), mode(&home));
    let peek = own.join("peek");

    let stat = ["/usr/bin/stat", "-f", "%N"];
    let works: Vec<(&str, Vec<String>)> = vec![
        ("realpath-own", argv(&["/bin/realpath"], &[&own])),
        ("realpath-own-var", argv(&["/bin/realpath"], &[&var(&own)])),
        ("realpath-tool", argv(&["/bin/realpath"], &[&tool])),
        ("realpath-tool-var", argv(&["/bin/realpath"], &[&var(&tool)])),
        ("stat-own", argv(&stat, &[&own])),
        ("stat-root", argv(&stat, &[&troot])),
        ("stat-home", argv(&stat, &[&home])),
        ("mkdir-own", argv(&["/bin/mkdir", "-p"], &[&own.join("a/b/c")])),
        ("mkdir-own-var", argv(&["/bin/mkdir", "-p"], &[&var(&own).join("d/e/f")])),
        ("cd-own", argv(&["/bin/sh", "-c", "cd \"$1\" && /bin/pwd -P", "sh"], &[&var(&own)])),
    ];
    // Read-only, so the control runs them too.
    let reads: Vec<(&str, Vec<String>)> = vec![
        ("ls-root", argv(&["/bin/ls", "-A"], &[&troot])),
        ("ls-home", argv(&["/bin/ls", "-A"], &[&home])),
        ("ls-slot", argv(&["/bin/ls", "-A"], &[&other])),
        ("stat-slot", argv(&stat, &[&other])),
        ("stat-slot-cache", argv(&stat, &[&other.join("cache")])),
        ("stat-secret", argv(&stat, &[&secret])),
        ("cat-secret", argv(&["/bin/cat"], &[&secret])),
        ("realpath-secret", argv(&["/bin/realpath"], &[&secret])),
        ("stat-state", argv(&stat, &[&l.state()])),
        ("ls-state", argv(&["/bin/ls", "-A"], &[&l.state()])),
        ("stat-key", argv(&stat, &[&l.key()])),
        ("stat-jobs", argv(&stat, &[&l.jobs()])),
        ("ls-repos", argv(&["/bin/ls", "-A"], &[&l.repos()])),
        ("stat-loose", argv(&stat, &[&loose])),
        ("cat-loose", argv(&["/bin/cat"], &[&loose])),
    ];
    let writes: Vec<(&str, Vec<String>)> = vec![
        ("touch-root-file", argv(&["/usr/bin/touch"], &[&troot.join("planted")])),
        ("mkdir-root-slot", argv(&["/bin/mkdir"], &[&troot.join("3")])),
        ("touch-root", argv(&["/usr/bin/touch", "-c"], &[&troot])),
        ("chmod-root", argv(&["/bin/chmod", "777"], &[&troot])),
        ("mv-slot", argv(&["/bin/mv"], &[&other, &troot.join("2x")])),
        ("rm-secret", argv(&["/bin/rm"], &[&secret])),
        ("write-secret", argv(&["/bin/sh", "-c", "echo poisoned > \"$1\"", "sh"], &[&secret])),
        ("touch-home-file", argv(&["/usr/bin/touch"], &[&home.join("planted")])),
        ("mkdir-home-dir", argv(&["/bin/mkdir"], &[&home.join("planted-dir")])),
        ("touch-home", argv(&["/usr/bin/touch", "-c"], &[&home])),
        ("chmod-home", argv(&["/bin/chmod", "777"], &[&home])),
        ("touch-tools", argv(&["/usr/bin/touch"], &[&l.toolchains().join("planted")])),
        ("cat-through-link", argv(&["/bin/sh", "-c", "ln -s \"$1\" \"$2\" && cat \"$2/cache/secret\"", "sh"], &[&other, &peek])),
    ];

    let control = std::process::Command::new("/bin/sh").arg("-c").arg(check_script(&reads)).output().unwrap();
    let control = String::from_utf8_lossy(&control.stdout).into_owned();
    for (tag, _) in &reads {
        assert!(checked(&control, tag).0, "control: the test process reaches {tag}:\n{control}");
    }

    let script = [works.as_slice(), reads.as_slice(), writes.as_slice()].concat();
    let got = run(n, 1, "w10", &["/bin/sh", "-c", &check_script(&script)]).await;
    for (tag, _) in &works {
        assert!(checked(&got.out, tag).0, "slot 1's own path, {tag}:\n{}\n{}", got.out, got.err);
    }
    let real = own.display().to_string();
    assert_eq!(checked(&got.out, "realpath-own").1, real, "{}", got.out);
    assert_eq!(checked(&got.out, "realpath-own-var").1, real, "{}", got.out);
    assert_eq!(checked(&got.out, "cd-own").1, real, "{}", got.out);
    assert_eq!(checked(&got.out, "realpath-tool-var").1, tool.display().to_string(), "{}", got.out);
    assert!(own.join("a/b/c").is_dir() && own.join("d/e/f").is_dir(), "mkdir -p made them");
    for (tag, _) in reads.iter().chain(&writes) {
        assert!(!checked(&got.out, tag).0, "the wall let slot 1 do {tag}:\n{}", got.out);
    }
    assert!(!got.out.contains("SECRET") && !got.err.contains("SECRET"), "a secret leaked to slot 1:\n{}\n{}", got.out, got.err);

    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "SECRET-slot-2", "the other slot's file is untouched");
    assert!(other.is_dir() && !troot.join("2x").exists());
    assert!(!troot.join("planted").exists() && !troot.join("3").exists(), "nothing planted in the tenants root");
    assert!(!home.join("planted").exists() && !home.join("planted-dir").exists(), "nothing planted in the node home");
    assert!(!l.toolchains().join("planted").exists(), "toolchains stay read-only");
    assert_eq!((mode(&troot), mode(&home)), (root_mode, home_mode), "metadata is read-only: the modes didn't change");

    // Slot 2 still reads its own file (the refusals above were the wall's).
    let own_read = run(n, 2, "w10-own", &["/bin/cat", &secret.display().to_string()]).await;
    assert_eq!(own_read.out, "SECRET-slot-2", "control: slot 2 reads its own");
}

/// The bug this guards (Sew's macOS builds, 0.46.7): a real `cargo build` in
/// a tenant's job, with a path dependency and a build script, the toolchain
/// where a node keeps it (`<home>/toolchains/cargo`). cargo rewrites each
/// unit's dep-info and canonicalizes paths through the tenants root, and
/// rustc finds its sysroot by canonicalizing its own path through the node
/// home; both failed with EPERM. Its `TMPDIR` is its own, and `/private/tmp`
/// stays closed.
#[tokio::test(flavor = "multi_thread")]
async fn w11_a_real_cargo_build_with_a_build_script_passes_in_a_tenants_job() {
    let m = mac("mac-w11", OWN2, true).await;
    let n = &m.n;
    let l = &n.layout;
    let out = std::process::Command::new("rustc").args(["--print", "sysroot"]).output().expect("this test needs rustc on PATH");
    assert!(out.status.success(), "rustc --print sysroot: {}", String::from_utf8_lossy(&out.stderr));
    let sysroot = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let tools = l.toolchains().join("cargo");
    assert!(!tools.exists(), "{} is the clone's", tools.display());
    let cp = std::process::Command::new("/bin/cp").arg("-Rc").arg(&sysroot).arg(&tools).status().unwrap();
    assert!(cp.success(), "clone {} into the node home", sysroot.display());

    let script = r#"set -e
p="$HOME/proj"
mkdir -p "$p/src" "$p/dep/src"
cd "$p"
cat > Cargo.toml <<'T'
[package]
name = "app"
version = "0.1.0"
edition = "2021"
[dependencies]
dep = { path = "dep" }
[workspace]
T
cat > src/main.rs <<'T'
fn main() { println!("app says {}", dep::said()); }
T
cat > dep/Cargo.toml <<'T'
[package]
name = "dep"
version = "0.1.0"
edition = "2021"
build = "build.rs"
T
cat > dep/build.rs <<'T'
use std::{env, fs};
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = fs::canonicalize(env::var("OUT_DIR").unwrap()).unwrap();
    let cwd = env::current_dir().unwrap().canonicalize().unwrap();
    assert!(cwd.join("build.rs").exists());
    fs::write(out.join("said.rs"), "pub fn said() -> &'static str { \"built by a build script\" }").unwrap();
}
T
cat > dep/src/lib.rs <<'T'
include!(concat!(env!("OUT_DIR"), "/said.rs"));
T
cargo build --offline
"$CARGO_TARGET_DIR/debug/app"
touch src/main.rs dep/src/lib.rs
cargo build --offline
"$CARGO_TARGET_DIR/debug/app"
echo "cargo=$(command -v cargo)"
echo "sysroot=$(rustc --print sysroot)"
"#;
    let mut p = plan("w11", &["/bin/sh", "-c", script]);
    p.limits.max_secs = 110;
    let got = run_plan(n, 1, p).await;
    assert_eq!(got.state, JobState::Done, "a tenant's cargo build: {:?}\nout={}\nerr={}", got.reason, got.out, got.err);
    assert_eq!(got.out.matches("app says built by a build script").count(), 2, "out={}\nerr={}", got.out, got.err);
    let real = std::fs::canonicalize(&tools).unwrap();
    assert!(got.out.contains(&format!("sysroot={}", real.display())), "rustc ran from the node home's toolchains: {}", got.out);
    assert!(got.out.contains(&format!("cargo={}", tools.join("bin/cargo").display())), "cargo ran from the node home's toolchains: {}", got.out);
}
