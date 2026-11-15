//! no-lan on a Mac node, for real (prd-compute-mac-no-lan-v1 §10.3): a
//! Seatbelt node whose jobs are held by the real `k2-node`, a server whose
//! share is `no-lan`, and jobs that try every way off the machine. Only
//! what goes through the job's own egress proxy reaches anything; the
//! proxy refuses the local network (and says so in the job log), and
//! loopback stays open.
//!
//! Needs a Mac with internet, on a LAN (192.168.1.1 and 10.0.0.1 are only
//! ever refused, never reached). Seatbelt doesn't nest: inside a `k2
//! compute` job `sandbox-exec` fails with `sandbox_apply: Operation not
//! permitted`, so run these on a Mac outside a job, from a checkout outside
//! `/Users` (the proxy's own profile denies people's homes):
//!
//!   cargo test -p k2-node --test no_lan_macos -- --test-threads=1
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only.
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
use k2_node_proto::frames::{JobState, LogStream};
use k2_node_proto::share::{self, Network, ServersFile, ShareLimits, Slot, Trust};

/// One at a time: the pool ports are the machine's.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

const NO_LAN: Slot = 1;
const NONE: Slot = 2;

struct Mac {
    n: TestNode,
    _fake: Fake,
    _conn: Conn,
    _sessions: Vec<tokio::sync::mpsc::Receiver<k2_node_proto::frames::Frame>>,
}

fn shares_up(n: &TestNode, slots: &[Slot]) -> Vec<tokio::sync::mpsc::Receiver<k2_node_proto::frames::Frame>> {
    let limits = ShareLimits { max_running: 4, max_queued: 100, ..ShareLimits::new_server(Trust::Own, true) };
    let mut keep = Vec::new();
    for s in std::iter::once(0u8).chain(slots.iter().copied()) {
        n.node.fairq.set_share(s, Some(k2_node::fairq::ShareInfo { label: format!("server-{s}"), limits: limits.clone(), status: k2_node_proto::share::ServerStatus::Active }));
        n.node.fairq.set_protocol(s, Some(k2_node_proto::frames::PROTOCOL));
        if s != 0 {
            let (tx, rx) = tokio::sync::mpsc::channel(4096);
            let t = n.node.tenant(s).unwrap();
            t.hub.attach(tx);
            // A current server: its jobs get the easy-mode lines (the
            // network line among them) and outputs.
            t.protocol.store(k2_node_proto::frames::PROTOCOL, std::sync::atomic::Ordering::SeqCst);
            keep.push(rx);
        }
    }
    keep
}

fn write_servers(config: &Path, owner_fp: &str, slots: &[(Slot, Network)]) {
    let room = |l: ShareLimits| ShareLimits { max_running: 4, max_queued: 100, ..l };
    let mut f = ServersFile::new(owner_fp, "owner.test", room(ShareLimits::owner(2, 8, 16, 100, Network::On)), 8, 1000, "local:test");
    for (slot, net) in slots {
        let mut e = f.servers[0].clone();
        e.slot = *slot;
        e.label = format!("server-{slot}.test");
        e.fp_prefix = stand_in_fp(*slot)[..16].to_string();
        e.trust = Trust::Own;
        e.set_limits(&room(ShareLimits::new_server(Trust::Own, true)));
        e.network = *net;
        f.servers.push(e);
    }
    f.validate().unwrap();
    std::fs::write(config.join(share::SERVERS_FILE), toml::to_string(&f).unwrap()).unwrap();
}

/// A short base dir (socket paths fit `sun_path`).
fn short_dir() -> PathBuf {
    let out = std::process::Command::new("/usr/bin/getconf").arg("DARWIN_USER_TEMP_DIR").output().unwrap();
    let base = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let d = base.join(format!("k2nl-{}", &k2_node_proto::crypto::random_id().unwrap()[..4]));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

/// A Seatbelt node holding its jobs with the real k2-node, with a no-lan
/// server and a none server.
async fn mac(policy_extra: &str) -> Mac {
    assert!(k2_node::wall::seatbelt_available(), "sandbox-exec is missing");
    let nests = std::process::Command::new(k2_node::wall::SANDBOX_EXEC).args(["-p", "(version 1)(allow default)", "--", "/usr/bin/true"]).output().unwrap();
    assert!(nests.status.success(), "these tests can't run under another sandbox (a k2 compute job?): {}", String::from_utf8_lossy(&nests.stderr));
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_k2-node"));
    assert!(!bin.starts_with("/Users"), "run from a checkout outside /Users: the proxy's own profile denies people's homes ({})", bin.display());
    let mut fake = Fake::start().await;
    let n = start_node_in(&fake, short_dir(), policy_extra, |o| {
        o.wall = Some(Wall::Seatbelt);
        o.job_holder = Some(bin);
    })
    .await;
    let mut conn = fake.next_conn().await;
    ready(&mut conn).await;
    let slots = [(NO_LAN, Network::NoLan), (NONE, Network::None)];
    write_servers(&n.layout.config, &fake.key.fingerprint(), &slots);
    for (slot, _) in slots {
        let t = n.layout.tenant(slot);
        for d in [t.root(), t.jobs(), t.cache(), t.repos(), t.tmp()] {
            k2_node::util::mkdir_mode(&d, 0o700).unwrap();
        }
        k2_node::identity::write_pin(&n.layout.tenant(slot).pin(), &fake.stand_in_pin(slot, "node-1", "mini-1")).unwrap();
    }
    for (slot, _) in slots {
        tenant_opened(&n, slot).await;
    }
    let sessions = shares_up(&n, &[NO_LAN, NONE]);
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
    out: String,
    sys: String,
}

fn journal(t: &Tenant, job: &str) -> (String, String) {
    let mut r = JournalReader::new(t.paths.journal(job, 1));
    let (mut o, mut s) = (String::new(), String::new());
    for rec in r.read_more(1_000_000).unwrap() {
        let txt = String::from_utf8_lossy(&rec.data).into_owned();
        match rec.stream {
            LogStream::Out | LogStream::Err => o.push_str(&txt),
            LogStream::Sys => s.push_str(&txt),
        }
    }
    (o, s)
}

async fn finished(n: &TestNode, slot: Slot, job: &str) -> Ran {
    let t = tenant(n, slot);
    let t0 = Instant::now();
    loop {
        if let Some(r) = t.ledger.lock().unwrap().get(job, 1).unwrap().filter(|r| r.state.is_terminal() && r.receipt.is_some()) {
            let (out, sys) = journal(&t, job);
            return Ran { state: r.state, out, sys };
        }
        assert!(t0.elapsed() < Duration::from_secs(300), "slot {slot} job {job} never finished");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn start(n: &TestNode, slot: Slot, job: &str, argv: &[&str]) {
    let mut p = plan(job, argv);
    p.limits.max_secs = 600;
    assert_eq!(n.node.on_assign(&tenant(n, slot), assign(p, 1)), Some((job.to_string(), 1)));
}

/// Every way off the machine a job might try, one `CHECK <name> <result>`
/// line each.
const PROBE: &str = r#"
import errno, os, socket, subprocess
def out(name, res):
    print("CHECK %s %s" % (name, res), flush=True)
def sock(name, fn):
    try:
        r = fn()
        out(name, "ok" if r is None else "ok:" + str(r))
    except OSError as e:
        out(name, "err:" + (errno.errorcode.get(e.errno, str(e.errno)) if e.errno else str(e)))
    except Exception as e:
        out(name, "exc:" + type(e).__name__)
def tcp(h, p, fam=socket.AF_INET):
    s = socket.socket(fam, socket.SOCK_STREAM)
    s.settimeout(5)
    try:
        s.connect((h, p))
    finally:
        s.close()
def udp(h, p, fam=socket.AF_INET):
    s = socket.socket(fam, socket.SOCK_DGRAM)
    try:
        s.sendto(b"\0" * 12, (h, p))
    finally:
        s.close()
def bind(h, p):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        s.bind((h, p))
    finally:
        s.close()
def echo(h, fam, typ):
    srv = socket.socket(fam, typ)
    srv.bind((h, 0))
    port = srv.getsockname()[1]
    c = socket.socket(fam, typ)
    c.settimeout(3)
    srv.settimeout(3)
    if typ == socket.SOCK_STREAM:
        srv.listen(1)
        c.connect((h, port))
        a, _ = srv.accept()
        c.sendall(b"ping")
        got = a.recv(4)
        a.close()
    else:
        c.sendto(b"ping", (h, port))
        got, _ = srv.recvfrom(16)
    c.close()
    srv.close()
    assert got == b"ping", got
def gai(n):
    return ",".join(sorted({a[4][0] for a in socket.getaddrinfo(n, 443, proto=socket.IPPROTO_TCP)}))
def run(name, argv):
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=90)
        tail = (p.stdout + p.stderr).strip().replace("\n", " | ")[-300:]
        out(name, "rc=%d %s" % (p.returncode, tail))
    except Exception as e:
        out(name, "exc:" + repr(e))
def via_proxy(target):
    port = int(os.environ["K2_JOB_PROXY"].rsplit(":", 1)[1])
    s = socket.create_connection(("127.0.0.1", port), timeout=20)
    s.sendall(("CONNECT %s HTTP/1.1\r\nHost: %s\r\n\r\n" % (target, target)).encode())
    b = b""
    while b"\r\n\r\n" not in b:
        d = s.recv(4096)
        if not d:
            break
        b += d
    s.close()
    return b.split(b"\r\n")[0].decode()
env = os.environ
out("proxy", env.get("K2_JOB_PROXY", "-"))
out("https_proxy", env.get("HTTPS_PROXY", "-"))
out("no_proxy", env.get("NO_PROXY", "-"))
curl = ["/usr/bin/curl", "-sS", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "30"]
if env.get("K2_JOB_PROXY"):
    run("curl crates", curl + ["https://index.crates.io/config.json"])
    run("git ls-remote", ["/usr/bin/git", "ls-remote", "https://github.com/octocat/Hello-World.git", "HEAD"])
    run("curl lan", curl + ["https://192.168.1.1/"])
    run("curl ten", curl + ["https://10.0.0.1/"])
    run("curl localtest", curl + ["https://localtest.me/"])
    run("curl plain", curl + ["http://example.com/"])
    sock("connect lan dns", lambda: via_proxy("192.168.1.1:53"))
    sock("connect v6 ula", lambda: via_proxy("[fd00::1]:443"))
    run("doh via proxy", ["/usr/bin/curl", "-sS", "--max-time", "30", "-H", "accept: application/dns-json", "https://cloudflare-dns.com/dns-query?name=github.com&type=A"])
run("curl noproxy", ["/usr/bin/curl", "-sS", "--noproxy", "*", "--max-time", "10", "https://1.1.1.1/"])
run("doh direct", ["/usr/bin/curl", "-sS", "--noproxy", "*", "--max-time", "10", "https://1.1.1.1/dns-query?name=github.com"])
run("nc public", ["/usr/bin/nc", "-z", "-w", "3", "1.1.1.1", "443"])
run("nc lan", ["/usr/bin/nc", "-z", "-w", "3", "192.168.1.1", "80"])
sock("tcp public", lambda: tcp("1.1.1.1", 443))
sock("tcp lan", lambda: tcp("192.168.1.1", 80))
sock("tcp v6 public", lambda: tcp("2606:4700:4700::1111", 443, socket.AF_INET6))
sock("tcp dns", lambda: tcp("1.1.1.1", 53))
sock("tcp dot", lambda: tcp("1.1.1.1", 853))
sock("udp public dns", lambda: udp("1.1.1.1", 53))
sock("udp lan dns", lambda: udp("192.168.1.1", 53))
sock("udp mdns", lambda: udp("224.0.0.251", 5353))
sock("udp v6 public", lambda: udp("2606:4700:4700::1111", 53, socket.AF_INET6))
run("dig", ["/usr/bin/dig", "+time=2", "+tries=1", "github.com"])
run("dig at", ["/usr/bin/dig", "+time=2", "+tries=1", "@1.1.1.1", "github.com"])
run("dig tcp", ["/usr/bin/dig", "+tcp", "+time=2", "+tries=1", "@1.1.1.1", "github.com"])
run("dig tcp lan", ["/usr/bin/dig", "+tcp", "+time=2", "+tries=1", "@192.168.1.1", "github.com"])
run("dscacheutil", ["/usr/bin/dscacheutil", "-q", "host", "-a", "name", "github.com"])
sock("gai github", lambda: gai("github.com"))
sock("gai localhost", lambda: gai("localhost"))
sock("tcp echo v4", lambda: echo("127.0.0.1", socket.AF_INET, socket.SOCK_STREAM))
sock("tcp echo v6", lambda: echo("::1", socket.AF_INET6, socket.SOCK_STREAM))
sock("udp echo v4", lambda: echo("127.0.0.1", socket.AF_INET, socket.SOCK_DGRAM))
sock("udp echo v6", lambda: echo("::1", socket.AF_INET6, socket.SOCK_DGRAM))
own = int(env["K2_JOB_PROXY"].rsplit(":", 1)[1]) if env.get("K2_JOB_PROXY") else 0
other = 19881 if own == 19880 else 19880
sock("tcp other pool", lambda: tcp("127.0.0.1", other))
sock("bind pool", lambda: bind("127.0.0.1", other))
"#;

/// `CHECK <name> <result>` lines: name → result.
fn checks(out: &str) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    for l in out.lines() {
        let Some(rest) = l.strip_prefix("CHECK ") else { continue };
        let names = [
            "proxy", "https_proxy", "no_proxy", "curl crates", "git ls-remote", "curl lan", "curl ten", "curl localtest", "curl plain", "connect lan dns", "connect v6 ula", "doh via proxy", "curl noproxy", "doh direct",
            "nc public", "nc lan", "tcp public", "tcp lan", "tcp v6 public", "tcp dns", "tcp dot", "udp public dns", "udp lan dns", "udp mdns", "udp v6 public", "dig", "dig at", "dig tcp", "dig tcp lan", "dscacheutil",
            "gai github", "gai localhost", "tcp echo v4", "tcp echo v6", "udp echo v4", "udp echo v6", "tcp other pool", "bind pool",
        ];
        let name = names.iter().filter(|n| rest.starts_with(&format!("{n} "))).max_by_key(|n| n.len()).unwrap_or_else(|| panic!("unknown check line {l:?}"));
        m.insert(name.to_string(), rest[name.len() + 1..].to_string());
    }
    m
}

/// The direct paths: none of them leaves the machine.
fn assert_no_side_door(c: &std::collections::BTreeMap<String, String>) {
    let get = |k: &str| c.get(k).unwrap_or_else(|| panic!("no {k} check: {c:#?}")).as_str();
    for k in ["tcp public", "tcp lan", "tcp v6 public", "tcp dns", "tcp dot", "udp public dns", "udp lan dns", "udp mdns", "udp v6 public"] {
        assert_eq!(get(k), "err:EPERM", "{k} must be refused by the sandbox: {c:#?}");
    }
    for k in ["curl noproxy", "doh direct"] {
        assert!(get(k).starts_with("rc=7 "), "{k}: curl can't connect: {}", get(k));
    }
    for k in ["nc public", "nc lan"] {
        assert!(!get(k).starts_with("rc=0"), "{k}: {}", get(k));
    }
    for k in ["dig", "dig at", "dig tcp", "dig tcp lan"] {
        assert!(!get(k).starts_with("rc=0"), "{k}: no DNS of its own: {}", get(k));
    }
    assert!(!get("dscacheutil").contains("ip_address"), "no lookup through the system resolver: {}", get("dscacheutil"));
    assert!(!get("gai github").starts_with("ok"), "getaddrinfo can't resolve a name: {}", get("gai github"));
    assert!(get("gai localhost").starts_with("ok") && get("gai localhost").contains("127.0.0.1"), "localhost still resolves (hosts file): {}", get("gai localhost"));
    for k in ["tcp echo v4", "tcp echo v6", "udp echo v4", "udp echo v6"] {
        assert_eq!(get(k), "ok", "{k}: loopback stays open: {c:#?}");
    }
    assert_eq!(get("tcp other pool"), "err:EPERM", "another pool port is closed (not merely refused): {c:#?}");
    assert_eq!(get("bind pool"), "err:EPERM", "no job squats a proxy port");
}

/// §10.3 items 3–7, 13–15 (and 1 with curl + git over https): through the
/// proxy only, the LAN refused with a live line, the host list at the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_no_lan_job_reaches_the_internet_only_through_its_own_proxy() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let m = mac("").await;
    start(&m.n, NO_LAN, "nl1", &["/usr/bin/python3", "-c", PROBE]);
    let r = finished(&m.n, NO_LAN, "nl1").await;
    assert_eq!(r.state, JobState::Done, "out={}\nsys={}", r.out, r.sys);
    let c = checks(&r.out);
    let get = |k: &str| c.get(k).unwrap_or_else(|| panic!("no {k} check:\n{}\n{}", r.out, r.sys)).as_str();
    let url = get("proxy").to_string();
    let port: u16 = url.strip_prefix("http://127.0.0.1:").and_then(|p| p.parse().ok()).unwrap_or_else(|| panic!("K2_JOB_PROXY {url:?}"));
    assert!((19880..20000).contains(&port), "a pool port: {port}");
    assert_eq!((get("https_proxy"), get("no_proxy")), (url.as_str(), "localhost,127.0.0.1,::1,[::1]"));
    assert_eq!(get("curl crates"), "rc=0 200", "public HTTPS through the proxy");
    assert!(get("git ls-remote").starts_with("rc=0 ") && get("git ls-remote").contains("HEAD"), "git over https honours HTTPS_PROXY: {}", get("git ls-remote"));
    for k in ["curl lan", "curl ten", "curl localtest"] {
        assert!(!get(k).starts_with("rc=0") && get(k).contains("403"), "{k}: refused by the proxy: {}", get(k));
    }
    assert_eq!(get("curl plain"), "rc=0 403", "plain HTTP gets the proxy's 403");
    assert_eq!(get("connect lan dns"), "ok:HTTP/1.1 403 Forbidden", "DNS to the LAN resolver through the proxy");
    assert_eq!(get("connect v6 ula"), "ok:HTTP/1.1 403 Forbidden");
    assert!(get("doh via proxy").starts_with("rc=0 ") && get("doh via proxy").contains("\"Answer\""), "DoH to a public resolver is a public HTTPS host: {}", get("doh via proxy"));
    assert_no_side_door(&c);
    // The job log: the start line, live refusals, the summary and the list.
    let sys = &r.sys;
    assert!(sys.contains(&format!("[k2-node] network: no-lan: this job reaches the internet over TCP through the node's proxy (HTTPS_PROXY={url}), not the node's local network.")), "{sys}");
    for l in [
        "[k2-node] network: refused 192.168.1.1:443: private address (no-lan keeps jobs off the local network)",
        "[k2-node] network: refused 10.0.0.1:443: private address",
        "[k2-node] network: refused localtest.me:443 → 127.0.0.1: loopback address",
        "[k2-node] network: refused http://example.com/: plain HTTP isn't carried; use https",
        "[k2-node] network: refused 192.168.1.1:53: private address",
        "[k2-node] network: refused [fd00::1]:443: private address",
    ] {
        assert!(sys.contains(l), "missing {l:?} in:\n{sys}");
    }
    let summary = sys.lines().find(|l| l.starts_with("[k2-node] network: no-lan proxy: ")).unwrap_or_else(|| panic!("no summary:\n{sys}"));
    assert!(summary.contains("refused ") && summary.contains("192.168.1.1:443 (private address)") && summary.contains("localtest.me:443 (127.0.0.1, loopback address)"), "{summary}");
    let list: Vec<&str> = sys.lines().skip_while(|l| !l.starts_with("[k2-node] network: hosts this job reached or tried")).collect();
    assert!(!list.is_empty(), "no host list:\n{sys}");
    let row = |verdict: &str, hp: &str| list.iter().find(|l| l.starts_with(&format!("[k2-node]   {verdict}  {hp} "))).copied();
    for hp in ["index.crates.io:443", "github.com:443", "cloudflare-dns.com:443"] {
        let l = row("allowed", hp).unwrap_or_else(|| panic!("no allowed row for {hp}:\n{}", list.join("\n")));
        assert!(!l.contains(" in 0 B "), "bytes came in from {hp}: {l}");
    }
    for hp in ["192.168.1.1:443", "10.0.0.1:443", "localtest.me:443", "192.168.1.1:53", "[fd00::1]:443"] {
        assert!(row("refused", hp).is_some(), "no refused row for {hp}:\n{}", list.join("\n"));
    }
    assert!(!list.iter().any(|l| l.contains("1.1.1.1") || l.contains("2606:4700")), "a direct attempt never reaches the proxy, so never its list:\n{}", list.join("\n"));
    // The result: the summary as a note, the whole list as network_hosts.
    let row = tenant(&m.n, NO_LAN).ledger.lock().unwrap().get("nl1", 1).unwrap().expect("its row");
    let o = row.outputs.expect("an easy-mode server gets outputs");
    assert!(o.notes.iter().any(|n| n.starts_with("network: no-lan proxy: ")), "{:?}", o.notes);
    let crates = o.network_hosts.iter().find(|h| h.host == "index.crates.io" && h.port == 443 && h.verdict == "allowed").unwrap_or_else(|| panic!("{:?}", o.network_hosts));
    assert!(crates.conns >= 1 && crates.bytes_in > 0 && crates.bytes_out > 0, "{crates:?}");
    assert!(
        o.network_hosts.iter().any(|h| h.host == "192.168.1.1" && h.port == 443 && h.verdict == "refused" && h.reason.as_deref() == Some("private address")),
        "{:?}",
        o.network_hosts
    );
    assert!(!o.network_hosts.iter().any(|h| h.host == "1.1.1.1"), "{:?}", o.network_hosts);
    // The proxy is gone with the job, its port free, its folder removed.
    let egress = tenant(&m.n, NO_LAN).paths.state().join("egress");
    assert!(std::fs::read_dir(&egress).map(|d| d.count()).unwrap_or(0) == 0, "the proxy's folder is removed");
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "nothing listens on {port} any more");
}

/// §10.3 item 7: a `none` job started while a no-lan job runs can't reach
/// that job's proxy; neither job has DNS or a direct path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_none_job_cant_borrow_a_running_no_lan_jobs_proxy() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let m = mac("").await;
    start(&m.n, NO_LAN, "hold", &["/bin/sh", "-c", "echo \"PORT ${K2_JOB_PROXY##*:}\"; sleep 10"]);
    let t = tenant(&m.n, NO_LAN);
    let t0 = Instant::now();
    let port: u16 = loop {
        let (out, _) = journal(&t, "hold");
        if let Some(p) = out.lines().find_map(|l| l.strip_prefix("PORT ")).and_then(|p| p.trim().parse().ok()) {
            break p;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "the no-lan job never printed its proxy port: {out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(), "its proxy listens (control)");
    let script = format!("import socket\ns = socket.socket()\ntry:\n    s.connect(('127.0.0.1', {port}))\n    print('CONNECT ok')\nexcept OSError as e:\n    print('CONNECT', e.errno)\n");
    start(&m.n, NONE, "borrow", &["/usr/bin/python3", "-c", &script]);
    let r = finished(&m.n, NONE, "borrow").await;
    assert_eq!(r.out.trim(), format!("CONNECT {}", libc::EPERM), "the sandbox refuses another job's proxy port: {}\n{}", r.out, r.sys);
    assert!(r.sys.contains("network: none: the node's owner keeps this server's jobs off the network here"), "{}", r.sys);
    let held = finished(&m.n, NO_LAN, "hold").await;
    assert_eq!(held.state, JobState::Done);
}

/// §5.7: the owner's `mac_no_lan = "none"` gives back the old mapping:
/// no proxy, no `K2_JOB_PROXY`, the old line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mac_no_lan_none_gives_no_network_and_no_proxy() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let m = mac("mac_no_lan = \"none\"\n").await;
    start(&m.n, NO_LAN, "off", &["/usr/bin/python3", "-c", PROBE]);
    let r = finished(&m.n, NO_LAN, "off").await;
    assert_eq!(r.state, JobState::Done, "{}\n{}", r.out, r.sys);
    let c = checks(&r.out);
    assert_eq!((c["proxy"].as_str(), c["https_proxy"].as_str()), ("-", "-"), "K2_JOB_PROXY only behind the proxy");
    assert_no_side_door(&c);
    assert!(r.sys.contains("network: none: the node's owner set this server's share here to no-lan, and this Mac gives no-lan jobs no network (the node's owner set mac_no_lan = \"none\")"), "{}", r.sys);
    assert!(!r.sys.contains("network: no-lan proxy"), "no proxy, no summary: {}", r.sys);
}

/// §5.2: no proxy can start (every port it tries is taken): the job runs
/// with `none`, never `on`, and says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_proxy_that_cant_start_means_none_never_on() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let m = mac("").await;
    // Every free pool port held by something else (a port already in use
    // is taken anyway).
    let taken: Vec<std::net::TcpListener> = (19880..20000).filter_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok()).collect();
    assert!(!taken.is_empty(), "no pool port was free to hold");
    start(&m.n, NO_LAN, "fallback", &["/usr/bin/python3", "-c", PROBE]);
    let r = finished(&m.n, NO_LAN, "fallback").await;
    drop(taken);
    assert_eq!(r.state, JobState::Done, "{}\n{}", r.out, r.sys);
    let c = checks(&r.out);
    assert_eq!(c["proxy"].as_str(), "-", "no K2_JOB_PROXY on the fallback");
    assert_no_side_door(&c);
    assert!(r.sys.contains("network: none: the no-lan proxy couldn't start (every proxy port tried was taken"), "{}", r.sys);
}
