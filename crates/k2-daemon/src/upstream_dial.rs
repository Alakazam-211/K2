//! Dial a nested-subdomain / published-service target (`localhost:3000`,
//! `127.0.0.1:8080`, `[::1]:4000`, `host:port`).
//!
//! ## Why not just `TcpStream::connect(target)`
//!
//! `localhost` goes through the system resolver, and boxes disagree on what
//! it returns. On some Linux images `localhost` resolves to `::1` only (or
//! `::1` first with no IPv4 entry). An app that listens on IPv4 only
//! (`127.0.0.1`, the common default) then refuses `[::1]:PORT`, and the
//! visitor got no page at all.
//!
//! `localhost` means loopback (RFC 6761), so for that host we skip the
//! resolver and try **both loopbacks ourselves: `127.0.0.1` first, then
//! `::1`**. A refused loopback dial fails instantly, so trying them in
//! order costs nothing and needs no Happy-Eyeballs racing. The stored
//! target stays exactly what the user typed (`localhost:PORT`); only the
//! dial changes.
//!
//! Other hosts keep the resolver, but every resolved address is tried and
//! each failure is recorded, so the log says which addresses were tried.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

/// A target split into host + port. `host` has IPv6 brackets removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetAddr {
    pub host: String,
    pub port: u16,
}

/// Parse a stored target. Accepts `host:port`, `[v6]:port`, and tolerates a
/// leading `http://` / `https://` and a trailing `/path` (people paste URLs).
pub fn parse_target(raw: &str) -> Result<TargetAddr, String> {
    let t = raw.trim();
    let t = t
        .strip_prefix("http://")
        .or_else(|| t.strip_prefix("https://"))
        .unwrap_or(t);
    let t = t.split('/').next().unwrap_or(t);
    if t.is_empty() {
        return Err(format!("empty target {raw:?}"));
    }
    let (host, port) = if let Some(rest) = t.strip_prefix('[') {
        let (h, after) = rest
            .split_once(']')
            .ok_or_else(|| format!("target {raw:?}: unclosed '['"))?;
        let p = after
            .strip_prefix(':')
            .ok_or_else(|| format!("target {raw:?}: missing port"))?;
        (h, p)
    } else {
        t.rsplit_once(':')
            .ok_or_else(|| format!("target {raw:?}: missing port (want host:port)"))?
    };
    if host.is_empty() {
        return Err(format!("target {raw:?}: missing host"));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("target {raw:?}: bad port {port:?}"))?;
    if port == 0 {
        return Err(format!("target {raw:?}: port 0"));
    }
    Ok(TargetAddr {
        host: host.to_string(),
        port,
    })
}

/// True for `localhost` (any case, optional trailing dot).
pub fn is_localhost(host: &str) -> bool {
    host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
}

/// The fixed dial order for a `localhost` target: IPv4 loopback first (the
/// usual bind for dev servers), then IPv6 loopback. Never the resolver.
pub fn localhost_candidates(port: u16) -> Vec<SocketAddr> {
    vec![
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
    ]
}

/// Every address to try for `target`, in order. `localhost` and IP
/// literals never touch the resolver.
pub async fn candidate_addrs(target: &TargetAddr) -> Result<Vec<SocketAddr>, String> {
    if is_localhost(&target.host) {
        return Ok(localhost_candidates(target.port));
    }
    if let Ok(ip) = target.host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, target.port)]);
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((target.host.as_str(), target.port))
        .await
        .map_err(|e| format!("resolve {}: {e}", target.host))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("resolve {}: no addresses", target.host));
    }
    Ok(addrs)
}

/// Why a dial failed: the raw target plus each address tried and its error.
#[derive(Debug, Clone)]
pub struct DialFailure {
    pub target: String,
    /// `(address, error)` per attempt, in order. Empty when the target did
    /// not parse or resolve (see `reason`).
    pub tried: Vec<(SocketAddr, String)>,
    pub reason: Option<String>,
}

impl std::fmt::Display for DialFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} not reachable", self.target)?;
        if let Some(r) = &self.reason {
            write!(f, ": {r}")?;
        }
        if !self.tried.is_empty() {
            let parts: Vec<String> = self
                .tried
                .iter()
                .map(|(a, e)| format!("{a} ({e})"))
                .collect();
            write!(f, "; tried {}", parts.join(", "))?;
        }
        Ok(())
    }
}

/// Dial `addrs` in order. Each attempt gets an equal share of `budget`
/// (refused loopback returns at once, so the share only matters for a
/// black-holed address). First success wins.
pub async fn dial_addrs(
    target: &str,
    addrs: &[SocketAddr],
    budget: Duration,
) -> Result<TcpStream, DialFailure> {
    let n = addrs.len().max(1) as u32;
    let share = budget / n;
    let deadline = Instant::now() + budget;
    let mut tried = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let left = deadline.saturating_duration_since(Instant::now());
        let wait = share.max(Duration::from_millis(1)).min(left.max(Duration::from_millis(1)));
        match tokio::time::timeout(wait, TcpStream::connect(addr)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => tried.push((*addr, e.to_string())),
            Err(_) => tried.push((*addr, format!("timed out after {wait:?}"))),
        }
    }
    Err(DialFailure {
        target: target.to_string(),
        tried,
        reason: None,
    })
}

/// Parse + expand + dial a stored target. See the module docs for the
/// `localhost` rule.
pub async fn dial_target(raw: &str, budget: Duration) -> Result<TcpStream, DialFailure> {
    let fail = |reason: String| DialFailure {
        target: raw.to_string(),
        tried: Vec::new(),
        reason: Some(reason),
    };
    let target = parse_target(raw).map_err(fail)?;
    let addrs = candidate_addrs(&target).await.map_err(fail)?;
    dial_addrs(raw, &addrs, budget).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn parse_target_shapes() {
        let p = |s: &str| parse_target(s).expect(s);
        assert_eq!(p("localhost:3000"), TargetAddr { host: "localhost".into(), port: 3000 });
        assert_eq!(p("127.0.0.1:8080"), TargetAddr { host: "127.0.0.1".into(), port: 8080 });
        assert_eq!(p("[::1]:4000"), TargetAddr { host: "::1".into(), port: 4000 });
        assert_eq!(p("http://localhost:3000/"), TargetAddr { host: "localhost".into(), port: 3000 });
        assert_eq!(p(" app.internal:9000 "), TargetAddr { host: "app.internal".into(), port: 9000 });
        for bad in ["", "localhost", "localhost:", "localhost:0", "localhost:99999", ":3000", "[::1:3000", "[::1]3000"] {
            assert!(parse_target(bad).is_err(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn localhost_order_is_ipv4_then_ipv6_and_skips_resolver() {
        assert!(is_localhost("localhost"));
        assert!(is_localhost("LocalHost."));
        assert!(!is_localhost("localhost.example"));
        assert!(!is_localhost("127.0.0.1"));
        let c = localhost_candidates(3000);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], "127.0.0.1:3000".parse::<SocketAddr>().unwrap());
        assert_eq!(c[1], "[::1]:3000".parse::<SocketAddr>().unwrap());
    }

    #[tokio::test]
    async fn candidate_addrs_localhost_is_both_loopbacks_whatever_the_resolver_says() {
        let t = parse_target("localhost:4321").unwrap();
        let got = candidate_addrs(&t).await.expect("candidates");
        assert_eq!(got, localhost_candidates(4321));
        let lit = parse_target("[::1]:4321").unwrap();
        assert_eq!(
            candidate_addrs(&lit).await.expect("literal"),
            vec!["[::1]:4321".parse::<SocketAddr>().unwrap()]
        );
    }

    async fn echo_tag(l: TcpListener, tag: &'static [u8]) {
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let _ = s.write_all(tag).await;
            }
        });
    }

    async fn read_tag(mut s: TcpStream) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        let n = s.read(&mut b).await.expect("read tag");
        b.truncate(n);
        b
    }

    /// The reported bug: app on 127.0.0.1 only, target `localhost:PORT`.
    #[tokio::test]
    async fn localhost_target_reaches_ipv4_only_listener() {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind v4");
        let port = l.local_addr().unwrap().port();
        echo_tag(l, b"V4").await;
        let s = dial_target(&format!("localhost:{port}"), Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| panic!("localhost must reach a v4-only app: {e}"));
        assert_eq!(read_tag(s).await, b"V4");
    }

    /// Resolver order cannot matter: even an address list that puts `::1`
    /// FIRST (what the bad box returned) falls through to 127.0.0.1.
    #[tokio::test]
    async fn ipv6_first_order_still_reaches_ipv4_only_listener() {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind v4");
        let port = l.local_addr().unwrap().port();
        echo_tag(l, b"V4").await;
        let mut order = localhost_candidates(port);
        order.reverse(); // ::1 first
        let s = dial_addrs("localhost", &order, Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| panic!("::1-first order must still reach v4: {e}"));
        assert_eq!(read_tag(s).await, b"V4");
    }

    /// The other half: an app on `::1` only (e.g. a dev server that bound
    /// `localhost` on a v6-first box). Needs IPv6 loopback, which macOS and
    /// the Linux CI hosts have; a box without it fails here loudly.
    #[tokio::test]
    async fn localhost_target_reaches_ipv6_only_listener() {
        let l = TcpListener::bind(("::1", 0))
            .await
            .expect("bind [::1]:0 (this test needs IPv6 loopback)");
        let port = l.local_addr().unwrap().port();
        echo_tag(l, b"V6").await;
        let s = dial_target(&format!("localhost:{port}"), Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| panic!("localhost must reach a ::1-only app: {e}"));
        assert_eq!(read_tag(s).await, b"V6");
    }

    #[tokio::test]
    async fn unreachable_localhost_reports_both_addresses_tried() {
        // A port reserved on both loopbacks that nothing listens on.
        let closed = k2_core::test_env::ClosedPort::dual();
        let port = closed.port();
        let err = dial_target(&format!("localhost:{port}"), Duration::from_secs(5))
            .await
            .expect_err("nothing listens there");
        let tried: Vec<SocketAddr> = err.tried.iter().map(|(a, _)| *a).collect();
        assert_eq!(tried, localhost_candidates(port), "both loopbacks, v4 first: {err}");
        let msg = err.to_string();
        assert!(msg.contains("127.0.0.1") && msg.contains("::1"), "{msg}");
    }

    #[tokio::test]
    async fn bad_target_is_a_dial_failure_with_reason() {
        let err = dial_target("no-port-here", Duration::from_secs(1))
            .await
            .expect_err("unparseable");
        assert!(err.tried.is_empty());
        assert!(err.reason.as_deref().unwrap().contains("missing port"), "{err}");
    }
}
