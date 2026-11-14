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
//! - No timeouts: the TCP connect and the handshake block as before.

use imap::extensions::idle::SetReadTimeout;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

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

/// A rustls client stream over TCP that the `imap` crate can drive.
pub struct ImapTlsStream {
    inner: StreamOwned<ClientConnection, TcpStream>,
}

impl ImapTlsStream {
    /// The underlying TLS connection (protocol version, peer certs, …).
    pub fn connection(&self) -> &ClientConnection {
        &self.inner.conn
    }
}

impl Read for ImapTlsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for ImapTlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl SetReadTimeout for ImapTlsStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::Result<()> {
        self.inner
            .sock
            .set_read_timeout(timeout)
            .map_err(imap::Error::Io)
    }
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
/// chain against `config`'s roots and its name against `host`.
pub fn handshake(
    config: Arc<ClientConfig>,
    host: &str,
    mut tcp: TcpStream,
) -> Result<ImapTlsStream, String> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| format!("invalid server name '{host}': {e}"))?;
    let mut conn =
        ClientConnection::new(config, name).map_err(|e| format!("TLS setup for {host}: {e}"))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| format!("TLS handshake with {host}: {e}"))?;
    }
    Ok(ImapTlsStream {
        inner: StreamOwned::new(conn, tcp),
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
    connect_with_config(native_config()?, host, port, mode)
}

/// [`connect`] with a caller-chosen config (tests pass one that trusts a
/// test CA; production always goes through [`connect`]).
pub fn connect_with_config(
    config: Arc<ClientConfig>,
    host: &str,
    port: u16,
    mode: ImapTlsMode,
) -> Result<imap::Client<imap::Connection>, String> {
    let tcp = TcpStream::connect((host, port)).map_err(|e| format!("connect {host}:{port}: {e}"))?;
    match mode {
        ImapTlsMode::Implicit => {
            let tls = handshake(config, host, tcp)?;
            let mut client = imap::Client::new(Box::new(tls) as imap::Connection);
            client
                .read_greeting()
                .map_err(|e| format!("greeting from {host}: {e}"))?;
            Ok(client)
        }
        ImapTlsMode::StartTls => {
            let tcp = starttls_upgrade(host, tcp)?;
            let tls = handshake(config, host, tcp)?;
            let mut client = imap::Client::new(Box::new(tls) as imap::Connection);
            // The greeting came in plain text before STARTTLS.
            client.greeting_read = true;
            Ok(client)
        }
    }
}

/// The plain-text part of STARTTLS: read the greeting, check CAPABILITY
/// offers STARTTLS, send it and wait for the tagged OK. Returns the bare
/// TCP stream, ready for the handshake.
fn starttls_upgrade(host: &str, tcp: TcpStream) -> Result<TcpStream, String> {
    let mut pre = imap::Client::new(tcp);
    pre.read_greeting()
        .map_err(|e| format!("greeting from {host}: {e}"))?;
    let caps = pre
        .capabilities()
        .map_err(|e| format!("CAPABILITY on {host}: {e}"))?;
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
        .map_err(|e| format!("STARTTLS on {host}: {e}"))?;
    for _ in 0..=MAX_PRE_TLS_LINES {
        let line = read_line_unbuffered(&mut tcp)
            .map_err(|e| format!("STARTTLS on {host}: {e}"))?;
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
