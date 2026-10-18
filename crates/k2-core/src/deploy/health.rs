//! The health gate (§7.2 step 6): one implementation above the engine
//! trait. `GET <healthPath>` on the new instance's port every interval,
//! each probe bounded, until a 2xx or the gate's deadline.
//!
//! Litmus L4: a probe may run **logged in as a test guest**, at deploy
//! time and periodically after. Stage 1 ships the hook
//! ([`set_test_guest_provider`]); test guests arrive in stage 2 (S25). With
//! no provider, a manifest that asks for it fails the gate with
//! `test_guest_unavailable`: never a silent anonymous probe instead.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Who a probe runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAs {
    Anonymous,
    TestGuest,
}

/// Mints the request headers for a logged-in probe of `app_id` (stage 2
/// installs one that signs a short-lived test-guest pass in).
pub type TestGuestProvider = Box<dyn Fn(&str) -> Result<Vec<(String, String)>, String> + Send + Sync>;

static PROVIDER: RwLock<Option<TestGuestProvider>> = RwLock::new(None);

pub fn set_test_guest_provider(p: Option<TestGuestProvider>) {
    *PROVIDER.write().unwrap_or_else(|e| e.into_inner()) = p;
}

/// The headers a probe sends, or why it can't run.
pub fn probe_headers(app_id: &str, as_: ProbeAs) -> Result<Vec<(String, String)>, (&'static str, String)> {
    match as_ {
        ProbeAs::Anonymous => Ok(vec![]),
        ProbeAs::TestGuest => {
            let g = PROVIDER.read().unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                None => Err((
                    super::codes::TEST_GUEST_UNAVAILABLE,
                    "this App's health check runs as a logged-in test guest (host.health.asTestGuest), and test guests \
                     arrive in a later 0.46 release; drop asTestGuest or wait for it"
                        .into(),
                )),
                Some(f) => f(app_id).map_err(|e| (super::codes::TEST_GUEST_UNAVAILABLE, e)),
            }
        }
    }
}

/// One probe's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// An HTTP status came back.
    Status(u16),
    /// Nothing answered (refused, reset, timed out, not HTTP).
    NoAnswer(String),
}

impl Probe {
    pub fn healthy(&self) -> bool {
        matches!(self, Probe::Status(s) if (200..300).contains(s))
    }
}

/// One `GET path` on `127.0.0.1:port`, bounded by `timeout` overall.
pub fn probe_once(port: u16, path: &str, headers: &[(String, String)], timeout: Duration) -> Probe {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let deadline = Instant::now() + timeout;
    let mut s = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(e) => return Probe::NoAnswer(format!("connect: {e}")),
    };
    let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(10));
    let _ = s.set_read_timeout(Some(left));
    let _ = s.set_write_timeout(Some(left));
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: k2-health/1\r\nConnection: close\r\n");
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
            return Probe::NoAnswer("refused a header with a line break".into());
        }
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    if let Err(e) = s.write_all(req.as_bytes()) {
        return Probe::NoAnswer(format!("write: {e}"));
    }
    let mut buf = [0u8; 64];
    let mut got = Vec::new();
    while got.len() < 16 && !got.contains(&b'\n') {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) => return Probe::NoAnswer(format!("read: {e}")),
        }
    }
    let line = String::from_utf8_lossy(&got).to_string();
    let code = line
        .strip_prefix("HTTP/1.")
        .and_then(|r| r.get(2..))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|c| c.parse::<u16>().ok());
    match code {
        Some(c) => Probe::Status(c),
        None => Probe::NoAnswer(format!("not an HTTP answer: {:?}", line.chars().take(40).collect::<String>())),
    }
}

/// The open path's probe for an old `--cmd` service (K5): something
/// listens on the port, as `k2 publish run` always checked. Those services
/// weren't written for an HTTP health check (an API may answer `/` with
/// 404), so a manifest synthesized for one asks for this
/// (`host.health.tcp`).
///
/// A container's published port accepts on the host side before the App
/// listens inside (pasta answers, then closes when the App's port
/// refuses), so an accept alone proves nothing: the probe sends a plain
/// `GET /` and counts any byte back, or a connection still open and quiet
/// after a moment, as listening; a close or reset before any byte is not.
pub fn probe_tcp(port: u16, timeout: Duration) -> Probe {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(e) => return Probe::NoAnswer(format!("connect: {e}")),
    };
    let quiet = timeout.min(Duration::from_millis(1500));
    let _ = s.set_read_timeout(Some(quiet));
    let _ = s.set_write_timeout(Some(quiet));
    let _ = s.write_all(format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: k2-health/1\r\nConnection: close\r\n\r\n").as_bytes());
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Ok(n) if n > 0 => Probe::Status(200),
        Ok(_) => Probe::NoAnswer("the port accepted and closed at once (nothing listens behind it yet)".into()),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Probe::Status(200),
        Err(e) => Probe::NoAnswer(format!("read: {e}")),
    }
}

/// One probe the way the manifest asks: TCP accept or `GET path` 2xx.
pub fn probe_as(tcp: bool, port: u16, path: &str, headers: &[(String, String)], timeout: Duration) -> Probe {
    if tcp {
        probe_tcp(port, timeout)
    } else {
        probe_once(port, path, headers, timeout)
    }
}

/// Why the gate failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateFailure {
    pub code: &'static str,
    pub message: String,
}

/// Probe until healthy, the instance dies (`alive` false) or `deadline`.
/// Returns the number of probes it took.
pub fn wait_healthy(
    port: u16,
    path: &str,
    headers: &[(String, String)],
    every: Duration,
    probe_timeout: Duration,
    gate: Duration,
    alive: impl FnMut() -> bool,
) -> Result<u32, GateFailure> {
    wait_healthy_as(false, port, path, headers, every, probe_timeout, gate, alive)
}

/// [`wait_healthy`] with the probe kind ([`probe_as`]).
#[allow(clippy::too_many_arguments)]
pub fn wait_healthy_as(
    tcp: bool,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    every: Duration,
    probe_timeout: Duration,
    gate: Duration,
    mut alive: impl FnMut() -> bool,
) -> Result<u32, GateFailure> {
    let what = if tcp { format!("a connection on port {port}") } else { format!("GET {path}") };
    let deadline = Instant::now() + gate;
    let mut n = 0u32;
    let mut last = Probe::NoAnswer("no probe yet".into());
    loop {
        if !alive() {
            return Err(GateFailure {
                code: super::codes::HEALTH_FAILED,
                message: format!("the instance exited before it answered {what} (last probe: {})", describe(&last)),
            });
        }
        n += 1;
        last = probe_as(tcp, port, path, headers, probe_timeout);
        if last.healthy() {
            return Ok(n);
        }
        if Instant::now() + every >= deadline {
            return Err(GateFailure {
                code: super::codes::HEALTH_FAILED,
                message: if tcp {
                    format!("nothing accepted {what} within {}s (last probe: {})", gate.as_secs(), describe(&last))
                } else {
                    format!("{what} never answered 2xx within {}s (last probe: {})", gate.as_secs(), describe(&last))
                },
            });
        }
        std::thread::sleep(every);
    }
}

fn describe(p: &Probe) -> String {
    match p {
        Probe::Status(s) => format!("HTTP {s}"),
        Probe::NoAnswer(e) => e.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn serve(status_line: &'static str) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let mut c = c;
                let mut b = [0u8; 1024];
                let _ = c.read(&mut b);
                let _ = c.write_all(format!("{status_line}\r\nContent-Length: 0\r\n\r\n").as_bytes());
            }
        });
        port
    }

    #[test]
    fn probe_reads_the_status() {
        assert!(probe_once(serve("HTTP/1.1 200 OK"), "/", &[], Duration::from_secs(2)).healthy());
        assert_eq!(probe_once(serve("HTTP/1.0 503 Busy"), "/", &[], Duration::from_secs(2)), Probe::Status(503));
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(probe_once(dead, "/", &[], Duration::from_millis(300)), Probe::NoAnswer(_)));
    }

    #[test]
    fn gate_passes_fails_and_stops_when_the_instance_dies() {
        let ok = serve("HTTP/1.1 204 No Content");
        assert_eq!(wait_healthy(ok, "/h", &[], Duration::from_millis(20), Duration::from_secs(1), Duration::from_secs(2), || true), Ok(1));
        let sick = serve("HTTP/1.1 500 Oops");
        let e = wait_healthy(sick, "/h", &[], Duration::from_millis(20), Duration::from_secs(1), Duration::from_millis(200), || true).unwrap_err();
        assert_eq!(e.code, super::super::codes::HEALTH_FAILED);
        assert!(e.message.contains("HTTP 500"), "{e:?}");
        let e = wait_healthy(sick, "/h", &[], Duration::from_millis(20), Duration::from_secs(1), Duration::from_secs(5), || false).unwrap_err();
        assert!(e.message.contains("exited"), "{e:?}");
    }

    /// K5: an old `--cmd` service on the open path is healthy when its port
    /// accepts (a 404 on `/` still passes), and the gate says so in words.
    #[test]
    fn the_tcp_probe_for_open_path_folders() {
        let api = serve("HTTP/1.1 404 Not Found");
        assert!(!probe_as(false, api, "/", &[], Duration::from_secs(2)).healthy());
        assert!(probe_as(true, api, "/", &[], Duration::from_secs(2)).healthy());
        assert_eq!(wait_healthy_as(true, api, "/", &[], Duration::from_millis(20), Duration::from_secs(1), Duration::from_secs(2), || true), Ok(1));
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(!probe_tcp(dead, Duration::from_millis(300)).healthy());
        let e = wait_healthy_as(true, dead, "/", &[], Duration::from_millis(20), Duration::from_millis(300), Duration::from_millis(200), || true).unwrap_err();
        assert!(e.message.starts_with(&format!("nothing accepted a connection on port {dead}")), "{e:?}");
        // pasta in front of an App that isn't listening yet: accept, then
        // close before any byte. Not healthy.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let shut = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                drop(c);
            }
        });
        let p = probe_tcp(shut, Duration::from_secs(2));
        assert!(matches!(&p, Probe::NoAnswer(m) if m.contains("closed at once") || m.starts_with("read:")), "{p:?}");
        // A raw TCP service that waits for the client to speak first: open
        // and quiet is listening.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let quiet = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut keep = Vec::new();
            for c in l.incoming().flatten() {
                keep.push(c);
            }
        });
        assert!(probe_tcp(quiet, Duration::from_millis(400)).healthy());
    }

    #[test]
    fn test_guest_probe_needs_a_provider() {
        set_test_guest_provider(None);
        assert_eq!(probe_headers("jim", ProbeAs::Anonymous), Ok(vec![]));
        let e = probe_headers("jim", ProbeAs::TestGuest).unwrap_err();
        assert_eq!(e.0, super::super::codes::TEST_GUEST_UNAVAILABLE);
        set_test_guest_provider(Some(Box::new(|app| Ok(vec![("Cookie".into(), format!("k2_guest={app}"))]))));
        assert_eq!(probe_headers("jim", ProbeAs::TestGuest).unwrap()[0].1, "k2_guest=jim");
        set_test_guest_provider(None);
    }
}
