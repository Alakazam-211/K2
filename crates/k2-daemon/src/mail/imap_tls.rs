//! TLS for the linked-inbox IMAP client, on the daemon's own rustls.
//!
//! The `imap` crate takes any `Read + Write` stream, so this module dials
//! the server, runs the TLS handshake with rustls and hands the finished
//! stream to [`imap::Client::new`]. The `imap` crate's own TLS feature is
//! not used.
//!
//! What it keeps from the connector it replaces:
//! - Trust roots: the operating system's store (`rustls-native-certs`),
//!   loaded once per process.
//! - Certificate and server-name checks are always on. There is no switch
//!   that turns them off.
//! - SNI is the host name the inbox row names. No ALPN is offered.
//! - Two modes: implicit TLS (port 993 style) and STARTTLS (port 143 style).
//!   STARTTLS fails when the server does not offer it; it never falls back
//!   to plain text.
//!
//! Timeouts (0.46.18). A linked inbox once stayed `inbox_busy` for about an
//! hour and a half: reads that stalled on the provider blocked their
//! threads forever, each holding one of the inbox's session places. So
//! every connection now has bounds:
//! - The TCP connect gives each resolved address [`CONNECT_TIMEOUT`]
//!   (at most [`MAX_DIAL_ADDRS`] addresses are tried). Name resolution is
//!   the system resolver's and keeps its own limits.
//! - Every read and every write waits at most [`IO_TIMEOUT`], from the
//!   first byte (the greeting, STARTTLS and the TLS handshake included) to
//!   the last (LOGOUT).
//! - A read or write that times out ends the connection: the socket is
//!   shut down and every later read or write on it fails at once, so a
//!   caller's LOGOUT never waits again and nothing is retried on a socket
//!   whose state is unknown. The session then drops and frees its place.
//! - The `imap` crate's IDLE sets its own read timeout through
//!   [`SetReadTimeout`]; while that is set, a timeout is IDLE's normal
//!   "time to re-IDLE" and passes through untouched. Clearing it (`None`)
//!   puts [`IO_TIMEOUT`] back, never "wait forever".

use imap::extensions::idle::SetReadTimeout;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How long the TCP connect waits for each resolved address.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one read or one write may wait. Generous next to an IMAP
/// server's normal answer time (well under a second; a big FETCH streams
/// steadily), short next to "forever".
pub const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// Most resolved addresses one dial tries, so a dial ends within
/// `MAX_DIAL_ADDRS * CONNECT_TIMEOUT`.
pub const MAX_DIAL_ADDRS: usize = 4;

/// The bounds one connection runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    pub io: Duration,
}

impl Timeouts {
    /// What every production connection uses.
    pub const DEFAULT: Timeouts = Timeouts { connect: CONNECT_TIMEOUT, io: IO_TIMEOUT };
}

impl Default for Timeouts {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How the connection becomes encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapTlsMode {
    /// TLS from the first byte (port 993 style).
    Implicit,
    /// Plain greeting, then `STARTTLS`, then TLS (port 143 style).
    StartTls,
}

/// The tag this module uses for its own `STARTTLS` command.
const STARTTLS_TAG: &str = "k2tls";
/// Longest pre-TLS response line accepted, in bytes.
const MAX_PRE_TLS_LINE: usize = 8 * 1024;
/// Most untagged lines accepted before the `STARTTLS` reply.
const MAX_PRE_TLS_LINES: usize = 64;

/// Is `e` a socket timeout? (`SO_RCVTIMEO`/`SO_SNDTIMEO` answer
/// `WouldBlock` on Unix and `TimedOut` on Windows.)
pub fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
}

/// The words a timed-out connection fails with. They name no credential
/// and match none of the "refused login" or "provider throttle" patterns,
/// so a timeout never counts toward a cool-down or a re-link.
pub fn timed_out_error(io: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "the mail server did not answer within {}s, so {} closed this connection",
            io.as_secs().max(1),
            k2_core::brand::PRODUCT_NAME,
        ),
    )
}

/// The timeout rules every IMAP stream here shares (see the module doc).
#[derive(Debug)]
struct IoGate {
    /// The bound outside IDLE.
    io: Duration,
    /// The `imap` crate set its own read timeout (IDLE): timeouts are its.
    crate_timeout: bool,
    /// A read or write timed out: the connection is over.
    dead: bool,
}

impl IoGate {
    fn new(io: Duration) -> Self {
        Self { io, crate_timeout: false, dead: false }
    }

    fn check(&self) -> std::io::Result<()> {
        if self.dead {
            return Err(timed_out_error(self.io));
        }
        Ok(())
    }

    /// After a failed read or write on `sock`: a timeout outside IDLE ends
    /// the connection.
    fn failed(&mut self, e: std::io::Error, sock: &TcpStream) -> std::io::Error {
        if !is_timeout(&e) || self.crate_timeout {
            return e;
        }
        self.dead = true;
        let _ = sock.shutdown(Shutdown::Both);
        timed_out_error(self.io)
    }

    /// The `imap` crate's hook: `Some` is IDLE's own wait, `None` puts the
    /// normal bound back.
    fn set_read_timeout(&mut self, sock: &TcpStream, timeout: Option<Duration>) -> imap::Result<()> {
        sock.set_read_timeout(Some(timeout.unwrap_or(self.io)))
            .map_err(imap::Error::Io)?;
        self.crate_timeout = timeout.is_some();
        Ok(())
    }
}

/// A rustls client stream over TCP that the `imap` crate can drive.
pub struct ImapTlsStream {
    inner: StreamOwned<ClientConnection, TcpStream>,
    gate: IoGate,
}

impl ImapTlsStream {
    /// The underlying TLS connection (protocol version, peer certs, …).
    pub fn connection(&self) -> &ClientConnection {
        &self.inner.conn
    }
}

impl Read for ImapTlsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.gate.check()?;
        self.inner.read(buf).map_err(|e| self.gate.failed(e, &self.inner.sock))
    }
}

impl Write for ImapTlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.gate.check()?;
        self.inner.write(buf).map_err(|e| self.gate.failed(e, &self.inner.sock))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.gate.check()?;
        self.inner.flush().map_err(|e| self.gate.failed(e, &self.inner.sock))
    }
}

impl SetReadTimeout for ImapTlsStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::Result<()> {
        self.gate.set_read_timeout(&self.inner.sock, timeout)
    }
}

/// Plain TCP under the same timeout rules, for the loopback test stubs
/// only: production has no plaintext path.
#[cfg(test)]
pub struct ImapPlainStream {
    tcp: TcpStream,
    gate: IoGate,
}

#[cfg(test)]
impl Read for ImapPlainStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.gate.check()?;
        self.tcp.read(buf).map_err(|e| self.gate.failed(e, &self.tcp))
    }
}

#[cfg(test)]
impl Write for ImapPlainStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.gate.check()?;
        self.tcp.write(buf).map_err(|e| self.gate.failed(e, &self.tcp))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.gate.check()?;
        self.tcp.flush().map_err(|e| self.gate.failed(e, &self.tcp))
    }
}

#[cfg(test)]
impl SetReadTimeout for ImapPlainStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::Result<()> {
        self.gate.set_read_timeout(&self.tcp, timeout)
    }
}

/// Dial `host:port`: each resolved address in turn (at most
/// [`MAX_DIAL_ADDRS`]), each for `t.connect`. The connected socket has
/// `t.io` read and write timeouts from its first byte.
pub fn dial(host: &str, port: u16, t: Timeouts) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("connect {host}:{port}: {e}"))?
        .take(MAX_DIAL_ADDRS)
        .collect();
    dial_addrs(host, port, &addrs, t)
}

/// [`dial`] over addresses already resolved.
pub fn dial_addrs(host: &str, port: u16, addrs: &[SocketAddr], t: Timeouts) -> Result<TcpStream, String> {
    let mut last = None;
    for addr in addrs.iter().take(MAX_DIAL_ADDRS) {
        match TcpStream::connect_timeout(addr, t.connect) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(t.io))
                    .and_then(|()| tcp.set_write_timeout(Some(t.io)))
                    .map_err(|e| format!("connect {host}:{port}: setting timeouts: {e}"))?;
                return Ok(tcp);
            }
            Err(e) if is_timeout(&e) => {
                last = Some(format!("no answer from {addr} within {}s", t.connect.as_secs().max(1)));
            }
            Err(e) => last = Some(format!("{addr}: {e}")),
        }
    }
    Err(match last {
        Some(why) => format!("connect {host}:{port}: {why}"),
        None => format!("connect {host}:{port}: the name resolved to no address"),
    })
}

/// A connected client plus a second handle on its socket, so the
/// per-inbox guard can shut a stuck session down from outside
/// ([`crate::mail::imap_guard::Permit::attach`]).
pub struct Dialed {
    pub client: imap::Client<imap::Connection>,
    pub socket: TcpStream,
}

fn socket_handle(tcp: &TcpStream, host: &str) -> Result<TcpStream, String> {
    tcp.try_clone().map_err(|e| format!("connect {host}: {e}"))
}

/// Load the operating system's trust roots. Certificates the platform
/// returns that rustls can't parse are skipped, as before; an empty
/// result is an error, because no server could be verified against it.
pub fn load_native_roots() -> Result<RootCertStore, String> {
    let loaded = rustls_native_certs::load_native_certs();
    let mut store = RootCertStore::empty();
    let (added, _skipped) = store.add_parsable_certificates(loaded.certs);
    if added == 0 {
        let why = loaded
            .errors
            .first()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default();
        return Err(format!(
            "no trusted root certificates were found on this system{why}; \
             IMAP over TLS can't verify the server"
        ));
    }
    Ok(store)
}

/// A rustls client config that trusts `roots` and nothing else, on the
/// daemon's crypto provider (aws-lc-rs), with no client certificate and
/// no ALPN. Certificate and name checks are rustls's defaults (on).
pub fn client_config(roots: RootCertStore) -> Result<Arc<ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS setup: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The process-wide config over the system's trust roots, built on first
/// use. A failed load is not cached, so a later call tries again.
fn native_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if let Some(config) = CONFIG.get() {
        return Ok(config.clone());
    }
    let config = client_config(load_native_roots()?)?;
    Ok(CONFIG.get_or_init(|| config).clone())
}

/// Run the TLS handshake over `tcp` for `host`, verifying the server's
/// chain against `config`'s roots and its name against `host`. The
/// handshake waits as long as the socket's own timeouts allow; the
/// finished stream keeps [`IO_TIMEOUT`].
pub fn handshake(
    config: Arc<ClientConfig>,
    host: &str,
    tcp: TcpStream,
) -> Result<ImapTlsStream, String> {
    handshake_with(config, host, tcp, IO_TIMEOUT)
}

fn handshake_with(
    config: Arc<ClientConfig>,
    host: &str,
    mut tcp: TcpStream,
    io: Duration,
) -> Result<ImapTlsStream, String> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| format!("invalid server name '{host}': {e}"))?;
    let mut conn =
        ClientConnection::new(config, name).map_err(|e| format!("TLS setup for {host}: {e}"))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).map_err(|e| {
            let e = if is_timeout(&e) { timed_out_error(io) } else { e };
            format!("TLS handshake with {host}: {e}")
        })?;
    }
    tcp.set_read_timeout(Some(io))
        .and_then(|()| tcp.set_write_timeout(Some(io)))
        .map_err(|e| format!("TLS with {host}: setting timeouts: {e}"))?;
    Ok(ImapTlsStream {
        inner: StreamOwned::new(conn, tcp),
        gate: IoGate::new(io),
    })
}

/// Connect to an IMAP server over TLS, trusting the system's roots.
/// The returned client has read the server's greeting and is ready for
/// `login` / `authenticate`.
pub fn connect(
    host: &str,
    port: u16,
    mode: ImapTlsMode,
) -> Result<imap::Client<imap::Connection>, String> {
    open(host, port, mode).map(|d| d.client)
}

/// [`connect`], keeping a handle on the socket (see [`Dialed`]).
pub fn open(host: &str, port: u16, mode: ImapTlsMode) -> Result<Dialed, String> {
    open_with(native_config()?, host, port, mode, Timeouts::DEFAULT)
}

/// [`connect`] with a caller-chosen config (tests pass one that trusts a
/// test CA; production always goes through [`connect`]).
pub fn connect_with_config(
    config: Arc<ClientConfig>,
    host: &str,
    port: u16,
    mode: ImapTlsMode,
) -> Result<imap::Client<imap::Connection>, String> {
    open_with(config, host, port, mode, Timeouts::DEFAULT).map(|d| d.client)
}

/// The one connect every path above runs: dial under `t`, then TLS.
pub fn open_with(
    config: Arc<ClientConfig>,
    host: &str,
    port: u16,
    mode: ImapTlsMode,
    t: Timeouts,
) -> Result<Dialed, String> {
    let tcp = dial(host, port, t)?;
    let socket = socket_handle(&tcp, host)?;
    let client = match mode {
        ImapTlsMode::Implicit => {
            let tls = handshake_with(config, host, tcp, t.io)?;
            let mut client = imap::Client::new(Box::new(tls) as imap::Connection);
            client
                .read_greeting()
                .map_err(|e| format!("greeting from {host}: {e}"))?;
            client
        }
        ImapTlsMode::StartTls => {
            let tcp = starttls_upgrade(host, tcp, t.io)?;
            let tls = handshake_with(config, host, tcp, t.io)?;
            let mut client = imap::Client::new(Box::new(tls) as imap::Connection);
            // The greeting came in plain text before STARTTLS.
            client.greeting_read = true;
            client
        }
    };
    Ok(Dialed { client, socket })
}

/// A plaintext connect for the loopback test stubs, under the same dial
/// and timeout rules as production. Never compiled into a release build.
#[cfg(test)]
pub fn open_plaintext(host: &str, port: u16, t: Timeouts) -> Result<Dialed, String> {
    let tcp = dial(host, port, t)?;
    let socket = socket_handle(&tcp, host)?;
    let plain = ImapPlainStream { tcp, gate: IoGate::new(t.io) };
    let mut client = imap::Client::new(Box::new(plain) as imap::Connection);
    client
        .read_greeting()
        .map_err(|e| format!("greeting from {host}: {e}"))?;
    Ok(Dialed { client, socket })
}

/// The plain-text part of STARTTLS: read the greeting, check CAPABILITY
/// offers STARTTLS, send it and wait for the tagged OK. Returns the bare
/// TCP stream, ready for the handshake. A timeout here ends the
/// connection like one after TLS does.
fn starttls_upgrade(host: &str, tcp: TcpStream, io: Duration) -> Result<TcpStream, String> {
    let socket = socket_handle(&tcp, host)?;
    let io_err = |e: std::io::Error| -> String {
        if is_timeout(&e) {
            let _ = socket.shutdown(Shutdown::Both);
            timed_out_error(io).to_string()
        } else {
            e.to_string()
        }
    };
    let imap_err = |e: imap::Error| -> String {
        match e {
            imap::Error::Io(e) => io_err(e),
            other => other.to_string(),
        }
    };
    let mut pre = imap::Client::new(tcp);
    pre.read_greeting()
        .map_err(|e| format!("greeting from {host}: {}", imap_err(e)))?;
    let caps = pre
        .capabilities()
        .map_err(|e| format!("CAPABILITY on {host}: {}", imap_err(e)))?;
    if !caps.has_str("STARTTLS") {
        return Err(format!(
            "{host} does not offer STARTTLS; use implicit TLS (port 993) for this inbox"
        ));
    }
    let mut tcp = pre
        .into_inner()
        .map_err(|e| format!("STARTTLS on {host}: {e}"))?;
    tcp.write_all(format!("{STARTTLS_TAG} STARTTLS\r\n").as_bytes())
        .and_then(|()| tcp.flush())
        .map_err(|e| format!("STARTTLS on {host}: {}", io_err(e)))?;
    for _ in 0..=MAX_PRE_TLS_LINES {
        let line = read_line_unbuffered(&mut tcp)
            .map_err(|e| format!("STARTTLS on {host}: {}", io_err(e)))?;
        let Some(rest) = line.strip_prefix(&format!("{STARTTLS_TAG} ")) else {
            continue; // an untagged line before the reply
        };
        if rest.as_bytes().get(..2).is_some_and(|w| w.eq_ignore_ascii_case(b"OK")) {
            return Ok(tcp);
        }
        return Err(format!("{host} refused STARTTLS: {}", rest.trim_end()));
    }
    Err(format!("{host} sent too many lines before answering STARTTLS"))
}

/// Read one CRLF line a byte at a time, so nothing after it is taken off
/// the socket: bytes the server sends after its STARTTLS reply belong to
/// the TLS handshake and must reach rustls untouched.
fn read_line_unbuffered(tcp: &mut TcpStream) -> std::io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if tcp.read(&mut byte)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            ));
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        if line.len() > MAX_PRE_TLS_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "response line too long",
            ));
        }
    }
}

#[cfg(test)]
#[path = "imap_tls_tests.rs"]
mod tests;
