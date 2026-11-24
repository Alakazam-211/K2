//! The connected stream: plain TCP (direct, or through an HTTP or SOCKS
//! proxy), or TLS to an `https://` proxy with the tunnel inside it.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A connection to the target, through the proxy's tunnel when there is
/// one. The caller speaks its own protocol on it (TLS to the target, for
/// an `https://` URL).
#[derive(Debug)]
#[non_exhaustive]
pub enum ProxyStream {
    /// TCP: to the target, or to an HTTP or SOCKS proxy whose tunnel is
    /// open.
    Tcp(TcpStream),
    /// TLS to an `https://` proxy, its tunnel open inside.
    #[cfg(feature = "rustls")]
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl ProxyStream {
    /// The TCP connection underneath.
    pub fn tcp(&self) -> &TcpStream {
        match self {
            ProxyStream::Tcp(s) => s,
            #[cfg(feature = "rustls")]
            ProxyStream::Tls(s) => &s.sock,
        }
    }

    /// Whether the proxy connection is TLS (an `https://` proxy).
    pub fn is_tls(&self) -> bool {
        !matches!(self, ProxyStream::Tcp(_))
    }

    /// The read timeout of the TCP connection underneath.
    pub fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        self.tcp().set_read_timeout(d)
    }

    /// The write timeout of the TCP connection underneath.
    pub fn set_write_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        self.tcp().set_write_timeout(d)
    }
}

impl From<TcpStream> for ProxyStream {
    fn from(s: TcpStream) -> ProxyStream {
        ProxyStream::Tcp(s)
    }
}

impl Read for ProxyStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ProxyStream::Tcp(s) => s.read(buf),
            // A proxy that closes without TLS's close_notify ends the
            // stream, as a closed TCP connection does: what runs inside
            // (the target's own TLS) has its own end to check.
            #[cfg(feature = "rustls")]
            ProxyStream::Tls(s) => match s.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
                r => r,
            },
        }
    }
}

impl Write for ProxyStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            ProxyStream::Tcp(s) => s.write(buf),
            #[cfg(feature = "rustls")]
            ProxyStream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            ProxyStream::Tcp(s) => s.flush(),
            #[cfg(feature = "rustls")]
            ProxyStream::Tls(s) => s.flush(),
        }
    }
}
