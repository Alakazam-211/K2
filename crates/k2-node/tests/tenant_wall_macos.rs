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
//!   cargo test -p k2-node --test tenant_wall_macos

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


fn write_servers(config: &Path, slots: &[(Slot, Trust, Network)]) {
    let mut f = ServersFile::new(&"a".repeat(64), "owner.test", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1000, "local:test");
    for (slot, trust, net) in slots {
        let mut e = f.servers[0].clone();
        e.slot = *slot;
        e.label = format!("server-{slot}.test");
        e.fp_prefix = format!("{:016x}", *slot as u64 + 1);
        e.trust = *trust;
        e.set_limits(&ShareLimits::new_server(*trust, true));
        e.network = *net;
        f.servers.push(e);
    }
    f.validate().unwrap();
    std::fs::write(config.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
}

/// A Seatbelt node with the owner and `slots`, each tenant root made as
/// `tenant.prepare` makes it on macOS (the node user's, 0700).
async fn mac(tag: &str, slots: &[(Slot, Trust, Network)], prepare: bool) -> Mac {
    assert!(k2_node::wall::seatbelt_available(), "sandbox-exec is missing");
    let mut fake = Fake::start().await;
    let n = start_node(&fake, tag, "", |o| o.wall = Some(Wall::Seatbelt)).await;
    let conn = fake.next_conn().await;
    write_servers(&n.layout.config, slots);
    for (slot, ..) in slots {
        if prepare {
            let t = n.layout.tenant(*slot);
            for d in [t.root(), t.jobs(), t.cache(), t.repos(), t.tmp()] {
                k2_node::util::mkdir_mode(&d, 0o700).unwrap();
            }
        }
        k2_node::identity::write_pin(&n.layout.tenant(*slot).pin(), &fake.pin_for("node-1", "mini-1")).unwrap();
        n.node.add_tenant(*slot).await.unwrap();
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
    let m = mac("mac-w9", OWN2, true).await;
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
    let lo_a = lo.local_addr().unwrap().port();
    let script = format!(
        "if /usr/bin/nc -z -w 2 192.0.2.1 9 2>/tmp/e1; then echo OFF-OK; else echo \"OFF-NO $(cat /tmp/e1)\"; fi; \
         if /usr/bin/nc -z -w 2 127.0.0.1 {lo_a}; then echo LO-OK; else echo LO-NO; fi"
    )
    .replace("/tmp/e1", "$TMPDIR/e1");
    let got = run(n, 1, "nolan", &["/bin/sh", "-c", &script]).await;
    assert!(got.out.contains("OFF-NO") && got.out.contains("Operation not permitted"), "W6: no-lan let a socket leave the Mac: {}", got.out);
    assert!(got.out.contains("LO-OK"), "loopback stays: {}", got.out);
    let got = run(n, 0, "owner-net", &["/bin/sh", "-c", &script]).await;
    assert!(!got.out.contains("Operation not permitted"), "the owner's network is on: {}", got.out);
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

/// T4 on one uid (best effort; W4 is Linux's): a tenant's job can't read
/// another process's info (`ps` of a process outside its sandbox shows
/// nothing), while its own children stay visible.
#[tokio::test(flavor = "multi_thread")]
async fn w4_other_processes_are_hidden_from_a_tenants_job() {
    let m = mac("mac-w4", OWN2, true).await;
    let n = &m.n;
    let mut other = std::process::Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let pid = other.id();
    let ctl = std::process::Command::new("/bin/ps").args(["-p", &pid.to_string(), "-o", "command="]).output().unwrap();
    assert!(String::from_utf8_lossy(&ctl.stdout).contains("sleep"), "control: ps sees {pid}");
    let script = format!(
        "ps -p {pid} -o command= 2>&1 | grep -q sleep && echo OTHER-SEEN || echo OTHER-HIDDEN; \
         /bin/sleep 30 & c=$!; ps -p $c -o command= 2>&1 | grep -q sleep && echo CHILD-SEEN || echo CHILD-HIDDEN; kill $c"
    );
    let got = run(n, 1, "ps", &["/bin/sh", "-c", &script]).await;
    let _ = other.kill();
    let _ = other.wait();
    assert!(got.out.contains("OTHER-HIDDEN"), "T4: a tenant's job saw another process: {}\n{}", got.out, got.err);
    assert!(got.out.contains("CHILD-SEEN"), "its own children stay visible: {}\n{}", got.out, got.err);
}
