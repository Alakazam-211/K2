//! A recording proxy and a TLS stub server, for the rows that send a
//! token to another machine through the person's proxy (K2's ruling on
//! the shared transport).
//!
//! - [`Pki`]: a throwaway CA, written as PEM for `SSL_CERT_FILE` (both
//!   sides trust it that way), and a server certificate signed by it.
//! - [`TlsStub`]: an `https://` server on loopback: each connection is
//!   one request, logged whole, and one fixed answer.
//! - [`ProxyStub`]: an HTTP proxy on loopback that records every
//!   connection's CONNECT head (or whatever came instead), then answers
//!   as its [`ProxyMode`] says: open the tunnel to the TLS stub at the
//!   CONNECT's port, or refuse with a 407.
//!
//! The target name is under `.invalid` (RFC 6761): it never resolves, so
//! a side that goes direct where it should have used the proxy fails to
//! connect, and one that uses the proxy never needs it to resolve.

#![cfg(unix)]

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The control plane's (or the remote K2's) name: never resolves.
pub const HOST: &str = "cp.example.invalid";

/// The CA and a certificate for [`HOST`].
pub struct Pki {
    /// The CA's PEM file, for `SSL_CERT_FILE`.
    pub ca_file: PathBuf,
    leaf: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Pki {
    pub fn new(dir: &Path) -> Pki {
        use rcgen::{
            BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
            KeyUsagePurpose,
        };
        // Names of their own: a leaf whose subject is its issuer's reads
        // as self-signed to OpenSSL (curl's UnknownCA).
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Acme throwaway test CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_key = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let leaf_key = KeyPair::generate().unwrap();
        let mut leaf_params = CertificateParams::new(vec![HOST.to_string()]).unwrap();
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, HOST);
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf_params.use_authority_key_identifier_extension = true;
        let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();
        std::fs::create_dir_all(dir).unwrap();
        let ca_file = dir.join("throwaway-ca.pem");
        std::fs::write(&ca_file, ca.pem()).unwrap();
        Pki {
            ca_file,
            leaf: leaf.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        }
    }
}

/// Each request a server got, as it came.
pub type Log = Arc<Mutex<Vec<String>>>;

/// Takes what `log` holds, leaving it empty.
pub fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().unwrap())
}

/// An `https://` server for [`HOST`] on loopback.
pub struct TlsStub {
    pub port: u16,
    pub log: Log,
    /// A connection that ended before a whole request: why.
    pub errors: Log,
}

impl TlsStub {
    /// Answers every request `status` with `body` (JSON).
    pub fn start(pki: &Pki, status: u16, body: &'static str) -> TlsStub {
        let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![pki.leaf.clone()], pki.key.clone_key())
        .unwrap();
        let cfg = Arc::new(cfg);
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let log: Log = Arc::default();
        let errors: Log = Arc::default();
        let (keep, errs) = (Arc::clone(&log), Arc::clone(&errors));
        std::thread::spawn(move || {
            for tcp in l.incoming() {
                let Ok(tcp) = tcp else { continue };
                let cfg = Arc::clone(&cfg);
                let (keep, errs) = (Arc::clone(&keep), Arc::clone(&errs));
                std::thread::spawn(move || serve_tls(cfg, tcp, &keep, &errs, status, body));
            }
        });
        TlsStub { port, log, errors }
    }
}

fn serve_tls(
    cfg: Arc<rustls::ServerConfig>,
    tcp: TcpStream,
    log: &Log,
    errors: &Log,
    status: u16,
    body: &str,
) {
    tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let conn = rustls::ServerConnection::new(cfg).unwrap();
    let mut s = rustls::StreamOwned::new(conn, tcp);
    let (got, why) = read_request(&mut s);
    if got.is_empty() {
        errors.lock().unwrap().push(why);
        return;
    }
    log.lock()
        .unwrap()
        .push(String::from_utf8_lossy(&got).into_owned());
    let reason = if status == 200 { "OK" } else { "Created" };
    let answer = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(answer.as_bytes());
    s.conn.send_close_notify();
    let _ = s.flush();
}

/// A request's head and its `Content-Length` body; what came before a
/// failure or the end.
fn read_request(s: &mut impl Read) -> (Vec<u8>, String) {
    let mut got = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|h| h.strip_prefix("content-length: "))
                .map_or(0, |v| v.trim().parse::<usize>().unwrap());
            if got.len() >= end + 4 + len {
                return (got, String::new());
            }
        }
        match s.read(&mut chunk) {
            Ok(0) => return (got, "closed".into()),
            Err(e) => return (got, format!("read: {e}")),
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    }
}

/// What the proxy does after a CONNECT head.
#[derive(Clone, Copy)]
pub enum ProxyMode {
    /// `200 Connection established`, then the bytes both ways to
    /// `127.0.0.1:<the CONNECT's port>`.
    Tunnel,
    /// `407 Proxy Authentication Required`, then close.
    Refuse,
}

/// A recording HTTP proxy on loopback.
pub struct ProxyStub {
    pub port: u16,
    /// Each connection's head (a CONNECT's, or whatever came), whole.
    pub heads: Log,
}

impl ProxyStub {
    pub fn start(mode: ProxyMode) -> ProxyStub {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let heads: Log = Arc::default();
        let keep = Arc::clone(&heads);
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(c) = c else { continue };
                let keep = Arc::clone(&keep);
                std::thread::spawn(move || proxy_one(c, &keep, mode));
            }
        });
        ProxyStub { port, heads }
    }

    /// `http://<user>:<pass>@127.0.0.1:<port>`, or with no user part.
    pub fn url(&self, auth: Option<(&str, &str)>) -> String {
        match auth {
            Some((u, p)) => format!("http://{u}:{p}@127.0.0.1:{}", self.port),
            None => format!("http://127.0.0.1:{}", self.port),
        }
    }
}

fn proxy_one(mut c: TcpStream, heads: &Log, mode: ProxyMode) {
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut got = Vec::new();
    let mut b = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        match c.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(_) => got.push(b[0]),
        }
    }
    let head = String::from_utf8_lossy(&got).into_owned();
    heads.lock().unwrap().push(head.clone());
    let target_port = head
        .strip_prefix("CONNECT ")
        .and_then(|r| r.split(' ').next())
        .and_then(|a| a.rsplit_once(':'))
        .and_then(|(_, p)| p.parse::<u16>().ok());
    let (ProxyMode::Tunnel, Some(port)) = (mode, target_port) else {
        let _ = c.write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"acme\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    };
    let Ok(up) = TcpStream::connect(("127.0.0.1", port)) else {
        let _ = c.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    let _ = c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
    c.set_read_timeout(None).unwrap();
    let (mut c2, mut up2) = (c.try_clone().unwrap(), up.try_clone().unwrap());
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut up2);
        let _ = up2.shutdown(Shutdown::Write);
    });
    let (mut up, mut c) = (up, c);
    let _ = std::io::copy(&mut up, &mut c);
    let _ = c.shutdown(Shutdown::Write);
    let _ = t.join();
}

/// A request with the two CLIs' own lines masked: the `User-Agent`
/// (`k5/…` or `curl/…`), the hand-over `X-K5-Cli-Via` (masked) and the
/// side's own `X-K5-Cli-Version` and `X-K5-Cli-Noun` (dropped). Also
/// returns whether the binary sent it (`User-Agent: k5/`).
pub fn masked(request: &str) -> (String, bool) {
    let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut native = false;
    let mut out = Vec::new();
    let mut lines = head.split("\r\n");
    // curl 8.5 and 8.7 write a query's %XX escapes in lower case.
    out.push(super::parity::fold_query_escapes(
        lines.next().unwrap_or(""),
    ));
    for line in lines {
        if line.starts_with("User-Agent: k5/") {
            native = true;
            out.push("User-Agent: <ua>".to_string());
        } else if line.starts_with("User-Agent: curl/") {
            out.push("User-Agent: <ua>".to_string());
        } else if line.starts_with("X-K5-Cli-Via: ") {
            out.push("X-K5-Cli-Via: <via>".to_string());
        } else if line.starts_with("X-K5-Cli-Version: ") || line.starts_with("X-K5-Cli-Noun: ") {
            continue;
        } else {
            out.push(line.to_string());
        }
    }
    (format!("{}\r\n\r\n{body}", out.join("\r\n")), native)
}

/// The `Proxy-Authorization` line a proxy URL's user part gives (the
/// test's own base64, not the binary's).
pub fn basic_line(user: &str, pass: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{user}:{pass}").into_bytes();
    let mut b64 = String::new();
    for c in raw.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= c.len() {
                b64.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                b64.push('=');
            }
        }
    }
    format!("Proxy-Authorization: Basic {b64}")
}

/// One side's run of a row: its output, its requests to the stub daemon
/// (normalised, the stub's port as `{port}`), the proxy's records and the
/// TLS stub's requests (masked, with who sent each).
#[derive(Debug, Clone)]
pub struct SideRun {
    pub out: super::parity::Out,
    pub daemon: Vec<String>,
    pub proxy: Vec<String>,
    pub tls: Vec<(String, bool)>,
    /// The TLS stub's connections that ended with no whole request.
    pub tls_errors: Vec<String>,
}

/// Runs `c` on bash, then on the binary, each against its own stub
/// daemon and both against `proxy` and `tls`, whose logs are taken
/// between the runs. The binary's daemon requests must be its own (a
/// command handed to the shell fails here).
pub fn both(
    rig: &super::parity::Rig,
    c: &super::parity::Case,
    proxy: &ProxyStub,
    tls: &TlsStub,
) -> [SideRun; 2] {
    use super::parity::{normalise, Side};
    let mut runs = Vec::new();
    for (bash, side) in [(true, Side::Bash), (false, Side::Rust)] {
        take(&proxy.heads);
        take(&tls.log);
        take(&tls.errors);
        rig.prepare(side, c);
        let out = rig.run(bash, c);
        let port = format!(":{}", rig.stub_of(side).port);
        let daemon = rig
            .read(side, !bash, c.name)
            .iter()
            .map(|r| normalise(r, !bash, rig.noun(), c.name).replace(&port, ":{port}"))
            .collect();
        // The proxy and the TLS stub log on their own threads: let a
        // request that ended with the process land.
        std::thread::sleep(Duration::from_millis(200));
        runs.push(SideRun {
            out,
            daemon,
            proxy: take(&proxy.heads),
            tls: take(&tls.log).iter().map(|r| masked(r)).collect(),
            tls_errors: take(&tls.errors),
        });
        println!(
            "-- {} side of {}: {:?}",
            if bash { "bash" } else { "the binary's" },
            c.name,
            runs.last()
        );
    }
    let rust = runs.pop().unwrap();
    let bash = runs.pop().unwrap();
    [bash, rust]
}

/// The two sides gave the same output, sent the same requests to the
/// daemon, the proxy and the TLS stub (masked), and the TLS stub's
/// requests came from the side that ran (the binary's own, not the
/// shell's after a hand-off).
pub fn assert_same(name: &str, b: &SideRun, r: &SideRun) {
    assert_eq!(
        (b.out.code, &b.out.stdout, &b.out.stderr),
        (r.out.code, &r.out.stdout, &r.out.stderr),
        "{name}: bash, then the binary"
    );
    assert_eq!(b.daemon, r.daemon, "{name}: the daemon's requests");
    let mask_ua = |heads: &[String]| -> Vec<String> {
        heads
            .iter()
            .map(|h| {
                h.split("\r\n")
                    .map(|l| {
                        if l.starts_with("User-Agent: ") {
                            "User-Agent: <ua>"
                        } else {
                            l
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\r\n")
            })
            .collect()
    };
    assert_eq!(
        mask_ua(&b.proxy),
        mask_ua(&r.proxy),
        "{name}: the CONNECT heads"
    );
    for h in &r.proxy {
        assert!(
            h.contains("\r\nUser-Agent: k5/"),
            "{name}: the binary's CONNECT: {h:?}"
        );
    }
    let only = |t: &[(String, bool)]| t.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
    assert_eq!(
        only(&b.tls),
        only(&r.tls),
        "{name}: the requests in the tunnel"
    );
    assert!(b.tls.iter().all(|(_, native)| !native), "{name}: bash's");
    assert!(
        r.tls.iter().all(|(_, native)| *native),
        "{name}: the binary's own"
    );
}

/// [`assert_same`] for a proxy that refuses the tunnel: curl 8.5 fails
/// such a CONNECT with exit 56, which `_k2_curl` never tries again (the
/// binary's code); newer curl (8.23 on one node) fails it with exit 7,
/// which `_k2_curl` tries 4 times. So bash's heads are the binary's,
/// each once or each 4 times in a row; everything else is the same.
pub fn assert_same_retried(name: &str, b: &SideRun, r: &SideRun) {
    let k = b.proxy.len() / r.proxy.len().max(1);
    assert!(
        !r.proxy.is_empty() && (k == 1 || k == 4) && b.proxy.len() == k * r.proxy.len(),
        "{name}: bash's CONNECTs are not the binary's once or 4 times: {b:?} {r:?}"
    );
    let mut once = b.clone();
    once.proxy = b
        .proxy
        .chunks(k)
        .map(|tries| {
            assert!(
                tries.iter().all(|t| *t == tries[0]),
                "{name}: bash's tries differ: {tries:?}"
            );
            tries[0].clone()
        })
        .collect();
    assert_same(name, &once, r);
}

/// K2's rule for a token and a proxy: the proxy saw a CONNECT for
/// `target` with `basic` (the proxy's own credential, when given) and
/// never the token; the token went to the TLS stub, inside the tunnel,
/// as the `Authorization` header, once per request.
pub fn assert_tunnel_only(
    name: &str,
    run: &SideRun,
    target: &str,
    basic: Option<&str>,
    token: &str,
) {
    assert!(
        !run.proxy.is_empty(),
        "{name}: the proxy saw nothing: {run:?}"
    );
    for h in &run.proxy {
        assert!(
            h.starts_with(&format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n")),
            "{name}: not a CONNECT for {target}: {h:?}"
        );
        assert!(
            !h.contains(token),
            "{name}: the token reached the proxy: {h:?}"
        );
        match basic {
            Some(line) => assert!(
                h.split("\r\n").any(|l| l == line),
                "{name}: no {line:?} in {h:?}"
            ),
            None => assert!(!h.contains("Proxy-Authorization"), "{name}: {h:?}"),
        }
        assert!(
            h.ends_with("\r\nProxy-Connection: Keep-Alive\r\n\r\n"),
            "{name}: {h:?}"
        );
    }
    assert!(
        !run.tls.is_empty(),
        "{name}: nothing came through the tunnel: {run:?}"
    );
    for (r, _) in &run.tls {
        let with_token: Vec<&str> = r.split("\r\n").filter(|l| l.contains(token)).collect();
        assert_eq!(
            with_token,
            [format!("Authorization: Bearer {token}").as_str()],
            "{name}: the token in the tunnel: {r:?}"
        );
    }
}

/// None of `secrets` in what either side printed.
pub fn assert_unprinted(name: &str, runs: &[SideRun], secrets: &[&str]) {
    for run in runs {
        for s in secrets {
            assert!(
                !run.out.stdout.contains(s) && !run.out.stderr.contains(s),
                "{name}: {s:?} was printed: {:?}",
                run.out
            );
        }
    }
}

/// The fixed bash refuses `line` already: the row compares both sides.
/// Until the bash fix lands, the row checks the binary's side alone and
/// says so (it flips by itself when cli/k2 carries the line).
pub fn bash_has(line_start: &str) -> bool {
    let k2 = super::parity::cli_k2();
    let text =
        std::fs::read_to_string(&k2).unwrap_or_else(|e| panic!("read {}: {e}", k2.display()));
    text.contains(line_start)
}
