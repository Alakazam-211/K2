//! Tests for `mail::imap_tls`: the trust store, the client config, and
//! real TLS conversations with a loopback IMAP server whose certificate
//! comes from a CA made in the test. No real mail host is ever dialled.

use super::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection};
use std::net::TcpListener;
use std::thread::JoinHandle;

const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// A CA made for one test, and leaf certificates it signs.
struct TestCa {
    cert: rcgen::Certificate,
    key: rcgen::KeyPair,
}

impl TestCa {
    fn new() -> Self {
        // Each CA gets its own subject name, so a chain from one CA can't
        // be matched by name to another CA in a test's trust store.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, format!("imap test CA {n}"));
        let key = rcgen::KeyPair::generate().expect("ca key");
        let cert = params.self_signed(&key).expect("ca cert");
        TestCa { cert, key }
    }

    /// A root store that trusts this CA and nothing else.
    fn roots(&self) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        roots.add(self.cert.der().clone()).expect("add test CA");
        roots
    }

    /// A server config presenting a leaf for `names`, signed by this CA.
    fn server_config(&self, names: &[&str]) -> Arc<ServerConfig> {
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let params = rcgen::CertificateParams::new(names).expect("leaf params");
        let key = rcgen::KeyPair::generate().expect("leaf key");
        let leaf = params
            .signed_by(&key, &self.cert, &self.key)
            .expect("sign leaf");
        let chain: Vec<CertificateDer<'static>> = vec![leaf.der().clone(), self.cert.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("server versions")
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .expect("server cert");
        Arc::new(config)
    }
}

/// What the loopback server saw, for the test to check.
#[derive(Debug, Default)]
struct Seen {
    /// Command lines received after TLS started.
    over_tls: Vec<String>,
    /// Command lines received in plain text.
    plain: Vec<String>,
}

/// How the loopback server answers before TLS (STARTTLS tests only).
#[derive(Clone, Copy)]
enum Plain {
    /// Implicit TLS: TLS from the first byte.
    None,
    /// Greet, offer STARTTLS in CAPABILITY, accept STARTTLS.
    StartTls,
    /// Greet, CAPABILITY without STARTTLS.
    NoStartTlsOffered,
    /// Greet, offer STARTTLS, then refuse it with NO.
    StartTlsRefused,
}

fn read_line<S: Read>(s: &mut S) -> std::io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if s.read(&mut byte)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "client closed",
            ));
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line).trim_end().to_string());
        }
    }
}

fn tag_of(line: &str) -> String {
    line.split_whitespace().next().expect("tagged line").to_string()
}

/// Answer LOGIN and LOGOUT until the client goes away.
fn serve_session<S: Read + Write>(s: &mut S, seen: &mut Vec<String>) -> std::io::Result<()> {
    loop {
        let line = match read_line(s) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        seen.push(line.clone());
        let tag = tag_of(&line);
        let verb = line
            .split_whitespace()
            .nth(1)
            .expect("verb")
            .to_ascii_uppercase();
        match verb.as_str() {
            "LOGIN" => s.write_all(format!("{tag} OK LOGIN completed\r\n").as_bytes())?,
            "LOGOUT" => {
                s.write_all(format!("* BYE bye\r\n{tag} OK LOGOUT completed\r\n").as_bytes())?;
                s.flush()?;
                return Ok(());
            }
            other => s.write_all(format!("{tag} BAD unexpected {other}\r\n").as_bytes())?,
        }
        s.flush()?;
    }
}

/// One-connection loopback IMAP server. Returns the port and a handle
/// that yields what it saw, or the error that ended it (a refused
/// handshake ends the server with an error).
fn spawn_server(config: Arc<ServerConfig>, plain: Plain) -> (u16, JoinHandle<Result<Seen, String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (mut tcp, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
        tcp.set_read_timeout(Some(IO_TIMEOUT)).expect("server read timeout");
        tcp.set_write_timeout(Some(IO_TIMEOUT)).expect("server write timeout");
        let mut seen = Seen::default();
        if !matches!(plain, Plain::None) {
            let io = |e: std::io::Error| format!("plain: {e}");
            tcp.write_all(b"* OK test IMAP4rev1 ready\r\n").map_err(io)?;
            let cap = read_line(&mut tcp).map_err(io)?;
            seen.plain.push(cap.clone());
            let offer = if matches!(plain, Plain::NoStartTlsOffered) { "" } else { " STARTTLS" };
            tcp.write_all(
                format!("* CAPABILITY IMAP4rev1{offer}\r\n{} OK done\r\n", tag_of(&cap)).as_bytes(),
            )
            .map_err(io)?;
            let next = match read_line(&mut tcp) {
                Ok(l) => l,
                // The client hung up without sending STARTTLS.
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(seen),
                Err(e) => return Err(io(e)),
            };
            seen.plain.push(next.clone());
            if matches!(plain, Plain::StartTlsRefused) {
                tcp.write_all(format!("{} NO not now\r\n", tag_of(&next)).as_bytes())
                    .map_err(io)?;
                return Ok(seen);
            }
            if !matches!(plain, Plain::StartTls) || !next.ends_with("STARTTLS") {
                // Anything else in plain text (a LOGIN, say) is recorded
                // for the test to fail on.
                return Ok(seen);
            }
            tcp.write_all(format!("{} OK begin TLS\r\n", tag_of(&next)).as_bytes())
                .map_err(io)?;
        }
        let conn = ServerConnection::new(config).map_err(|e| format!("server conn: {e}"))?;
        let mut tls = StreamOwned::new(conn, tcp);
        if matches!(plain, Plain::None) {
            tls.write_all(b"* OK test IMAP4rev1 ready\r\n")
                .and_then(|()| tls.flush())
                .map_err(|e| format!("tls: {e}"))?;
        }
        serve_session(&mut tls, &mut seen.over_tls).map_err(|e| format!("tls: {e}"))?;
        Ok(seen)
    });
    (port, handle)
}

fn join(handle: JoinHandle<Result<Seen, String>>) -> Result<Seen, String> {
    handle.join().expect("server thread panicked")
}

fn connect_err(r: Result<imap::Client<imap::Connection>, String>) -> String {
    match r {
        Ok(_) => panic!("connect succeeded; it must be refused"),
        Err(e) => e,
    }
}

// ── trust store and config ─────────────────────────────────────────────

#[test]
fn the_system_trust_store_loads() {
    let roots = load_native_roots().expect("system trust roots");
    assert!(!roots.is_empty(), "the system trust store has no roots");
}

#[test]
fn the_config_offers_no_alpn_and_sends_sni() {
    let config = client_config(TestCa::new().roots()).expect("config");
    assert!(config.alpn_protocols.is_empty(), "ALPN must stay empty");
    assert!(config.enable_sni, "SNI must be sent");
}

#[test]
fn an_invalid_server_name_is_refused_before_any_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let tcp = TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
    let config = client_config(TestCa::new().roots()).expect("config");
    let err = match handshake(config, "not a host name", tcp) {
        Ok(_) => panic!("handshake accepted an invalid server name"),
        Err(e) => e,
    };
    assert!(err.contains("invalid server name"), "{err}");
}

// ── implicit TLS ───────────────────────────────────────────────────────

#[test]
fn implicit_tls_connects_and_logs_in_over_tls() {
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["localhost"]), Plain::None);
    let config = client_config(ca.roots()).expect("config");
    let client =
        connect_with_config(config, "localhost", port, ImapTlsMode::Implicit).expect("connect");
    let mut session = client.login("user", "pass").map_err(|(e, _)| e).expect("login");
    session.logout().expect("logout");
    let seen = join(server).expect("server");
    assert!(seen.plain.is_empty(), "nothing may cross in plain text: {:?}", seen.plain);
    assert_eq!(seen.over_tls.len(), 2, "{:?}", seen.over_tls);
    assert!(seen.over_tls[0].contains("LOGIN"), "{:?}", seen.over_tls);
    assert!(seen.over_tls[1].contains("LOGOUT"), "{:?}", seen.over_tls);
}

#[test]
fn a_certificate_for_another_name_is_refused() {
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["imap.example.test"]), Plain::None);
    let config = client_config(ca.roots()).expect("config");
    let err = connect_err(connect_with_config(config, "localhost", port, ImapTlsMode::Implicit));
    assert!(err.contains("invalid peer certificate"), "{err}");
    assert!(err.contains("not valid for name"), "{err}");
    let server = join(server);
    assert!(server.is_err(), "the server must not have served a session: {server:?}");
}

#[test]
fn a_certificate_from_an_untrusted_ca_is_refused() {
    let trusted = TestCa::new();
    let other = TestCa::new();
    let (port, server) = spawn_server(other.server_config(&["localhost"]), Plain::None);
    let config = client_config(trusted.roots()).expect("config");
    let err = connect_err(connect_with_config(config, "localhost", port, ImapTlsMode::Implicit));
    assert!(err.contains("invalid peer certificate"), "{err}");
    assert!(err.contains("UnknownIssuer"), "{err}");
    let server = join(server);
    assert!(server.is_err(), "the server must not have served a session: {server:?}");
}

#[test]
fn the_production_connect_trusts_only_the_system_roots() {
    // A certificate from a CA made here is in no system store, so the
    // path every inbox uses must refuse it.
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["localhost"]), Plain::None);
    let err = connect_err(connect("localhost", port, ImapTlsMode::Implicit));
    assert!(err.contains("invalid peer certificate"), "{err}");
    assert!(err.contains("UnknownIssuer"), "{err}");
    let server = join(server);
    assert!(server.is_err(), "the server must not have served a session: {server:?}");
}

// ── STARTTLS ───────────────────────────────────────────────────────────

#[test]
fn starttls_upgrades_then_logs_in_over_tls() {
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["localhost"]), Plain::StartTls);
    let config = client_config(ca.roots()).expect("config");
    let client =
        connect_with_config(config, "localhost", port, ImapTlsMode::StartTls).expect("connect");
    let mut session = client.login("user", "pass").map_err(|(e, _)| e).expect("login");
    session.logout().expect("logout");
    let seen = join(server).expect("server");
    assert_eq!(seen.plain.len(), 2, "{:?}", seen.plain);
    assert!(seen.plain[0].ends_with("CAPABILITY"), "{:?}", seen.plain);
    assert_eq!(seen.plain[1], "k2tls STARTTLS");
    assert!(seen.over_tls[0].contains("LOGIN"), "LOGIN must be over TLS: {:?}", seen.over_tls);
}

#[test]
fn starttls_checks_the_certificate_name_too() {
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["imap.example.test"]), Plain::StartTls);
    let config = client_config(ca.roots()).expect("config");
    let err = connect_err(connect_with_config(config, "localhost", port, ImapTlsMode::StartTls));
    assert!(err.contains("not valid for name"), "{err}");
    let server = join(server);
    assert!(server.is_err(), "the server must not have served a session: {server:?}");
}

#[test]
fn starttls_not_offered_is_an_error_and_nothing_else_is_sent() {
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["localhost"]), Plain::NoStartTlsOffered);
    let config = client_config(ca.roots()).expect("config");
    let err = connect_err(connect_with_config(config, "localhost", port, ImapTlsMode::StartTls));
    assert!(err.contains("does not offer STARTTLS"), "{err}");
    let seen = join(server).expect("server");
    assert_eq!(seen.plain.len(), 1, "only CAPABILITY may be sent: {:?}", seen.plain);
    assert!(seen.over_tls.is_empty(), "{:?}", seen.over_tls);
}

#[test]
fn starttls_refused_by_the_server_is_an_error() {
    let ca = TestCa::new();
    let (port, server) = spawn_server(ca.server_config(&["localhost"]), Plain::StartTlsRefused);
    let config = client_config(ca.roots()).expect("config");
    let err = connect_err(connect_with_config(config, "localhost", port, ImapTlsMode::StartTls));
    assert!(err.contains("refused STARTTLS"), "{err}");
    let seen = join(server).expect("server");
    assert_eq!(seen.plain.len(), 2, "{:?}", seen.plain);
    assert!(seen.over_tls.is_empty(), "{:?}", seen.over_tls);
}

// ── timeouts (0.46.18) ─────────────────────────────────────────────────
//
// Each client call runs under `within`, so a client that blocks with no
// timeout fails the test instead of hanging it.

/// Short bounds, so a timeout test takes seconds.
const SHORT: Timeouts = Timeouts { connect: Duration::from_secs(2), io: Duration::from_secs(1) };

fn within<T: Send + 'static>(
    bound: Duration,
    what: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> (T, Duration) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let out = f();
        let _ = tx.send((out, t0.elapsed()));
    });
    match rx.recv_timeout(bound) {
        Ok(v) => v,
        Err(_) => panic!("{what} did not return within {bound:?}: it blocks with no timeout"),
    }
}

/// A listener that accepts one connection and then says nothing.
fn spawn_silent_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("accept");
        std::thread::sleep(Duration::from_secs(10));
        drop(tcp);
    });
    port
}

#[test]
fn implicit_tls_to_a_server_that_never_answers_times_out() {
    let port = spawn_silent_server();
    let config = client_config(TestCa::new().roots()).expect("config");
    let (r, took) = within(Duration::from_secs(8), "an implicit-TLS connect", move || {
        open_with(config, "127.0.0.1", port, ImapTlsMode::Implicit, SHORT).map(|_| ())
    });
    let err = r.expect_err("a silent server must not connect");
    assert!(err.contains("did not answer within 1s"), "{err}");
    assert!(took < Duration::from_secs(5), "bounded by the read timeout: {took:?}");
}

#[test]
fn starttls_to_a_server_that_never_greets_times_out() {
    let port = spawn_silent_server();
    let config = client_config(TestCa::new().roots()).expect("config");
    let (r, took) = within(Duration::from_secs(8), "a STARTTLS connect", move || {
        open_with(config, "127.0.0.1", port, ImapTlsMode::StartTls, SHORT).map(|_| ())
    });
    let err = r.expect_err("a silent server must not connect");
    assert!(err.contains("greeting from 127.0.0.1"), "{err}");
    assert!(err.contains("did not answer within 1s"), "{err}");
    assert!(took < Duration::from_secs(5), "bounded by the read timeout: {took:?}");
}

/// TLS, greeting, LOGIN answered; every other command is read and never
/// answered. Returns every line the client sent, until it hangs up.
fn spawn_stalling_tls_server(config: Arc<ServerConfig>) -> (u16, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("accept");
        tcp.set_read_timeout(Some(IO_TIMEOUT)).expect("server read timeout");
        let conn = ServerConnection::new(config).expect("server conn");
        let mut tls = StreamOwned::new(conn, tcp);
        let mut lines = Vec::new();
        if tls.write_all(b"* OK test IMAP4rev1 ready\r\n").and_then(|()| tls.flush()).is_err() {
            return lines;
        }
        while let Ok(line) = read_line(&mut tls) {
            lines.push(line.clone());
            if line.split_whitespace().nth(1).is_some_and(|v| v.eq_ignore_ascii_case("LOGIN")) {
                let ok = format!("{} OK LOGIN completed\r\n", tag_of(&line));
                if tls.write_all(ok.as_bytes()).and_then(|()| tls.flush()).is_err() {
                    break;
                }
            }
        }
        lines
    });
    (port, handle)
}

#[test]
fn a_stalled_command_ends_the_session_and_nothing_more_is_sent() {
    let ca = TestCa::new();
    let (port, server) = spawn_stalling_tls_server(ca.server_config(&["localhost"]));
    let config = client_config(ca.roots()).expect("config");
    let (out, took) = within(Duration::from_secs(8), "a stalled NOOP", move || {
        let d = open_with(config, "localhost", port, ImapTlsMode::Implicit, SHORT)?;
        let mut session = d.client.login("user", "pass").map_err(|(e, _)| e.to_string())?;
        let noop = session.noop().map_err(|e| e.to_string());
        let t = std::time::Instant::now();
        let logout = session.logout().map_err(|e| e.to_string());
        Ok::<_, String>((noop, logout, t.elapsed()))
    });
    let (noop, logout, logout_took) = out.expect("connect and login");
    let err = noop.expect_err("an unanswered NOOP must time out");
    assert!(err.contains("did not answer within 1s"), "{err}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let logout_err = logout.expect_err("LOGOUT on a timed-out connection must fail");
    assert!(logout_err.contains("did not answer within 1s"), "{logout_err}");
    assert!(logout_took < Duration::from_millis(500), "LOGOUT must not wait again: {logout_took:?}");
    let lines = server.join().expect("server thread panicked");
    assert!(lines.iter().any(|l| l.contains("NOOP")), "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains("LOGOUT")),
        "nothing may be sent on a timed-out connection: {lines:?}"
    );
}

#[test]
fn a_connect_to_an_address_that_never_answers_times_out_and_the_next_address_is_tried() {
    // A listener that never accepts: once its queue is full, a new
    // connect gets no answer at all (the kernel drops the SYN).
    let silent = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let silent_addr = silent.local_addr().expect("addr");
    let mut held = Vec::new();
    let mut full = false;
    for _ in 0..4096 {
        match TcpStream::connect_timeout(&silent_addr, Duration::from_millis(300)) {
            Ok(s) => held.push(s),
            Err(e) if is_timeout(&e) => {
                full = true;
                break;
            }
            Err(e) => panic!("filling the listen queue: {e}"),
        }
    }
    assert!(full, "the listen queue never filled ({} connections)", held.len());

    let (r, took) = within(Duration::from_secs(8), "a dial to a silent address", move || {
        dial_addrs("silent.test", silent_addr.port(), &[silent_addr], SHORT).map(|_| ())
    });
    let err = r.expect_err("a silent address must not connect");
    assert!(err.contains("no answer from") && err.contains("within 2s"), "{err}");
    assert!(took >= Duration::from_millis(1500) && took < Duration::from_secs(5), "{took:?}");

    // With a second address that answers, the dial moves on to it.
    let live = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let live_addr = live.local_addr().expect("addr");
    let (r, took) = within(Duration::from_secs(8), "a dial over two addresses", move || {
        dial_addrs("two.test", 0, &[silent_addr, live_addr], SHORT).map(|tcp| {
            (tcp.read_timeout().expect("read timeout"), tcp.write_timeout().expect("write timeout"))
        })
    });
    let (read_t, write_t) = r.expect("the second address connects");
    assert!(took >= Duration::from_millis(1500), "the first address had its full wait: {took:?}");
    assert_eq!(read_t, Some(SHORT.io), "reads are bounded from the first byte");
    assert_eq!(write_t, Some(SHORT.io), "writes are bounded from the first byte");
    drop(held);
}

/// Plain IMAP for the IDLE test: LOGIN; IDLE answers `+`, and on the
/// second IDLE reports `* 3 EXISTS`; DONE is answered; the first NOOP is
/// answered and later ones are not. Returns every line received.
fn spawn_idle_stub() -> (u16, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (mut tcp, _) = listener.accept().expect("accept");
        tcp.set_read_timeout(Some(IO_TIMEOUT)).expect("server read timeout");
        let mut lines = Vec::new();
        let (mut idles, mut noops, mut idle_tag) = (0, 0, String::new());
        if tcp.write_all(b"* OK stub IMAP4rev1 ready\r\n").is_err() {
            return lines;
        }
        while let Ok(line) = read_line(&mut tcp) {
            lines.push(line.clone());
            let reply = if line == "DONE" {
                format!("{idle_tag} OK IDLE terminated\r\n")
            } else {
                let tag = tag_of(&line);
                match line.split_whitespace().nth(1).unwrap_or("").to_ascii_uppercase().as_str() {
                    "LOGIN" => format!("{tag} OK LOGIN completed\r\n"),
                    "IDLE" => {
                        idles += 1;
                        idle_tag = tag;
                        if tcp.write_all(b"+ idling\r\n").is_err() {
                            break;
                        }
                        if idles < 2 {
                            continue;
                        }
                        std::thread::sleep(Duration::from_millis(300));
                        "* 3 EXISTS\r\n".to_string()
                    }
                    "NOOP" => {
                        noops += 1;
                        if noops > 1 {
                            continue; // stall
                        }
                        format!("{tag} OK NOOP completed\r\n")
                    }
                    "LOGOUT" => format!("* BYE bye\r\n{tag} OK LOGOUT completed\r\n"),
                    other => format!("{tag} BAD unexpected {other}\r\n"),
                }
            };
            if tcp.write_all(reply.as_bytes()).is_err() {
                break;
            }
        }
        lines
    });
    (port, handle)
}

#[test]
fn idle_waits_past_the_read_timeout_and_the_bound_comes_back_after() {
    use imap::extensions::idle::WaitOutcome;
    let (port, server) = spawn_idle_stub();
    let (out, _) = within(Duration::from_secs(10), "IDLE then a stalled NOOP", move || {
        let d = open_plaintext("127.0.0.1", port, SHORT)?;
        let mut s = d.client.login("u", "p").map_err(|(e, _)| e.to_string())?;
        let t0 = std::time::Instant::now();
        // IDLE's own 1.5 s wait outlasts the 1 s read bound: the first wait
        // times out (IDLE re-issues itself), the second sees new mail.
        let outcome = {
            let mut idle = s.idle();
            idle.timeout(Duration::from_millis(1500));
            idle.wait_while(|_| false).map_err(|e| e.to_string())?
        };
        let waited = t0.elapsed();
        let first_noop = s.noop().map_err(|e| e.to_string());
        let stalled_noop = s.noop().map_err(|e| e.to_string());
        Ok::<_, String>((outcome, waited, first_noop, stalled_noop))
    });
    let (outcome, waited, first_noop, stalled_noop) = out.expect("IDLE conversation");
    assert_eq!(outcome, WaitOutcome::MailboxChanged);
    assert!(waited >= Duration::from_millis(1400), "IDLE waited past the 1 s read bound: {waited:?}");
    first_noop.expect("the session still works after IDLE");
    let err = stalled_noop.expect_err("after IDLE the read bound is back");
    assert!(err.contains("did not answer within 1s"), "{err}");
    let lines = server.join().expect("server thread panicked");
    let count = |w: &str| lines.iter().filter(|l| l.split_whitespace().nth(1) == Some(w)).count();
    assert_eq!(count("IDLE"), 2, "one re-IDLE after IDLE's own timeout: {lines:?}");
    assert_eq!(lines.iter().filter(|l| *l == "DONE").count(), 2, "{lines:?}");
    assert_eq!(count("LOGOUT"), 0, "{lines:?}");
}
