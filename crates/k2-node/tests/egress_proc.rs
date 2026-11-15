//! `k2-node job-egress` as a process (prd-compute-mac-no-lan-v1 §10.2):
//! the real binary against a local upstream that a debug build's test hook
//! admits as public, and a scripted resolver. No sandbox here (these run
//! inside walled Mac compute jobs too); the Mac e2e covers the proxy's own
//! profile and the job's.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_node::egress::fold;
use k2_node::egress::proc::{self as ep, EgressSpec, SpawnError, TestHooks};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-node"))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// An upstream that says hello, then echoes upper-cased bytes.
fn upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            std::thread::spawn(move || {
                let _ = s.write_all(b"hello\n");
                let mut b = [0u8; 4096];
                loop {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            if s.write_all(&b[..n].to_ascii_uppercase()).is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    a
}

fn spec(port: u16, up: SocketAddr, deadline: i64) -> EgressSpec {
    EgressSpec {
        v: ep::EGRESS_V,
        port,
        deadline,
        job_key: "job-1".into(),
        lan: vec![],
        test: Some(TestHooks {
            public: vec![up],
            resolve: BTreeMap::from([
                ("up.example.com".to_string(), vec![vec!["127.0.0.1".parse().unwrap()]]),
                ("rebind.example.com".to_string(), vec![vec!["127.0.0.1".parse().unwrap()], vec!["192.168.1.20".parse().unwrap()]]),
            ]),
            idle_ms: None,
        }),
    }
}

fn start(dir: &Path, s: &EgressSpec, fresh: bool) -> ep::EgressFacts {
    ep::spawn(&bin(), dir, s, None, fresh, Duration::from_secs(10)).unwrap_or_else(|e| panic!("job-egress didn't start: {e}"))
}

fn connect(port: u16, target: &str) -> (TcpStream, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(s.read(&mut b).unwrap(), 1, "closed before an answer: {:?}", String::from_utf8_lossy(&head));
        head.push(b[0]);
    }
    (s, String::from_utf8(head).unwrap())
}

fn read_n(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).unwrap();
    b
}

fn wait_exit(dir: &Path, within: Duration) -> ep::EgressExit {
    let t0 = Instant::now();
    loop {
        if let Some(x) = ep::read_exit(dir) {
            return x;
        }
        assert!(t0.elapsed() < within, "the proxy didn't end within {within:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_lines(dir: &Path, n: usize) -> Vec<k2_node::egress::Decision> {
    let t0 = Instant::now();
    loop {
        let l = ep::read_all(dir);
        if l.len() >= n {
            return l;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "only {} of {n} decision lines: {l:?}", l.len());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A process group standing in for the job.
fn job_group() -> std::process::Child {
    use std::os::unix::process::CommandExt;
    std::process::Command::new("/bin/sleep").arg("60").process_group(0).spawn().unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[test]
fn it_carries_a_tunnel_and_ends_with_its_job() {
    let (dir, up, port) = (k2_node::util::temp_dir("egress-job"), upstream(), free_port());
    let f = start(&dir, &spec(port, up, 0), true);
    assert_eq!(f.port, port);
    assert!(ep::alive(&f));
    // Its own session: no k2-node process group or session holds it.
    // SAFETY: plain getters.
    let (its_pgid, ours) = unsafe { (libc::getpgid(f.pid as i32), libc::getpgrp()) };
    assert_ne!(its_pgid, ours, "the proxy is in no process group of its starter's");
    assert!(!dir.join(ep::SPEC_FILE).exists(), "the spec is read once and deleted");
    let (mut s, head) = connect(port, &format!("up.example.com:{}", up.port()));
    assert_eq!(head, "HTTP/1.1 200 Connection Established\r\n\r\n");
    assert_eq!(read_n(&mut s, 6), b"hello\n");
    s.write_all(b"cargo").unwrap();
    assert_eq!(read_n(&mut s, 5), b"CARGO");
    drop(s);
    // The rebinding name: the first answer is the one dialed, the second
    // request gets the private one and is refused (live, in the log).
    let (s1, head) = connect(port, &format!("rebind.example.com:{}", up.port()));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    drop(s1);
    let (_, head) = connect(port, &format!("rebind.example.com:{}", up.port()));
    assert!(head.starts_with("HTTP/1.1 403") && head.contains("X-K2-Refused: private_address"), "{head}");
    let lines = wait_lines(&dir, 5);
    assert!(lines.iter().any(|d| d.ev == "refused" && d.host == "rebind.example.com" && d.addr.as_deref() == Some("192.168.1.20")), "{lines:?}");
    // The job's group ends: so does the proxy, with its log complete.
    let mut job = job_group();
    let job_start = k2_node::util::process_start_time(job.id());
    ep::write_job(&dir, job.id() as i32, job_start).unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    assert!(ep::alive(&f), "a running job keeps its proxy");
    job.kill().unwrap();
    job.wait().unwrap();
    let x = wait_exit(&dir, Duration::from_secs(8));
    assert_eq!((x.why.as_str(), x.error), ("job_ended", None));
    let t0 = Instant::now();
    while ep::alive(&f) {
        assert!(t0.elapsed() < Duration::from_secs(5), "the proxy process outlived its exit.json");
        std::thread::sleep(Duration::from_millis(20));
    }
    let l = fold::fold(&ep::read_all(&dir));
    let up_row = l.rows.iter().find(|r| r.host == "up.example.com").expect("its row");
    assert_eq!((up_row.verdict.as_str(), up_row.conns, up_row.bytes_out, up_row.bytes_in), ("allowed", 1, 5, 11));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "nothing listens on its port any more");
}

#[test]
fn sigterm_closes_open_tunnels_and_writes_their_bytes() {
    let (dir, up, port) = (k2_node::util::temp_dir("egress-term"), upstream(), free_port());
    let f = start(&dir, &spec(port, up, 0), true);
    let (mut s, _) = connect(port, &up.to_string());
    assert_eq!(read_n(&mut s, 6), b"hello\n");
    s.write_all(b"ab").unwrap();
    assert_eq!(read_n(&mut s, 2), b"AB");
    assert!(ep::terminate(&f));
    assert_eq!(wait_exit(&dir, Duration::from_secs(8)).why, "stopped");
    let mut b = [0u8; 1];
    assert_eq!(s.read(&mut b).unwrap_or(0), 0, "the tunnel closed with the proxy");
    let close = ep::read_all(&dir).into_iter().find(|d| d.ev == "close").expect("a close line");
    assert_eq!((close.bytes_out, close.bytes_in), (2, 8));
}

#[test]
fn it_ends_at_its_deadline() {
    let (dir, up, port) = (k2_node::util::temp_dir("egress-deadline"), upstream(), free_port());
    let f = start(&dir, &spec(port, up, now() + 2), true);
    assert_eq!(wait_exit(&dir, Duration::from_secs(10)).why, "deadline");
    let t0 = Instant::now();
    while ep::alive(&f) {
        assert!(t0.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_killed_proxy_restarts_on_the_same_port_and_keeps_its_log() {
    let (dir, up, port) = (k2_node::util::temp_dir("egress-restart"), upstream(), free_port());
    let s1 = spec(port, up, 0);
    let f = start(&dir, &s1, true);
    let (_, head) = connect(port, "10.0.0.5:22");
    assert!(head.contains("X-K2-Refused: private_address"), "{head}");
    wait_lines(&dir, 1);
    // SAFETY: SIGKILL to the proxy we started (pid + start time checked).
    assert!(ep::alive(&f));
    unsafe { libc::kill(f.pid as i32, libc::SIGKILL) };
    let t0 = Instant::now();
    while ep::alive(&f) {
        assert!(t0.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ep::read_exit(&dir).is_none(), "a killed proxy says nothing: the node sees it gone");
    let f2 = start(&dir, &s1, false);
    assert_eq!(f2.port, port, "the job's env already points there");
    assert_ne!(f2.pid, f.pid);
    let (mut s, head) = connect(port, &up.to_string());
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(read_n(&mut s, 6), b"hello\n");
    drop(s);
    let lines = wait_lines(&dir, 3);
    assert_eq!(lines[0].host, "10.0.0.5", "the first proxy's lines are kept");
    assert!(ep::terminate(&f2));
    wait_exit(&dir, Duration::from_secs(8));
}

#[test]
fn a_taken_port_is_reported_as_taken() {
    let (dir, up) = (k2_node::util::temp_dir("egress-taken"), upstream());
    let holder = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = holder.local_addr().unwrap().port();
    match ep::spawn(&bin(), &dir, &spec(port, up, 0), None, true, Duration::from_secs(10)) {
        Err(SpawnError::PortTaken) => {}
        other => panic!("expected PortTaken, got {other:?}"),
    }
    let bad = EgressSpec { v: 99, ..spec(free_port(), up, 0) };
    match ep::spawn(&bin(), &dir, &bad, None, true, Duration::from_secs(10)) {
        Err(SpawnError::Failed(m)) => assert!(m.contains("job-egress failed to start") || m.contains("spec"), "{m}"),
        other => panic!("a spec of an unknown format must fail: {other:?}"),
    }
}

/// §7.6 / §10.3 item 11: nothing here touches the Mac's packet filter (it
/// is shared with other people's VM networking): no k2-node source names
/// `pfctl` or `/etc/pf.conf`.
#[test]
fn k2_node_never_runs_pfctl() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    let mut stack = vec![src];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                for (i, l) in text.lines().enumerate() {
                    if l.contains("pfctl") || l.contains("pf.conf") {
                        hits.push(format!("{}:{}: {l}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(hits.is_empty(), "k2-node must never run pfctl or touch pf.conf:\n{}", hits.join("\n"));
}

/// A restarted k2-node finds the proxy again from its files: the facts
/// once, then the log on from where the last node got to.
#[test]
fn a_second_node_adopts_it_from_its_files() {
    let (dir, up, port) = (k2_node::util::temp_dir("egress-adopt"), upstream(), free_port());
    let f = start(&dir, &spec(port, up, 0), true);
    let (_, _) = connect(port, "10.0.0.1:443");
    let mut first = ep::Tail::new(&dir, 0);
    let t0 = Instant::now();
    let mut seen = Vec::new();
    while seen.is_empty() {
        seen = first.read_new();
        assert!(t0.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    let saved = first.offset;
    // "Another process": only the files.
    let again = ep::read_facts(&dir).expect("egress.json");
    assert_eq!(again, f);
    assert!(ep::alive(&again));
    let (_, _) = connect(port, "nas.local:443");
    let mut second = ep::Tail::new(&dir, saved);
    let t0 = Instant::now();
    let mut more = Vec::new();
    while more.is_empty() {
        more = second.read_new();
        assert!(t0.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(more.iter().map(|d| d.host.as_str()).collect::<Vec<_>>(), vec!["nas.local"], "only what came after the saved offset");
    assert!(ep::terminate(&again));
    wait_exit(&dir, Duration::from_secs(8));
}
