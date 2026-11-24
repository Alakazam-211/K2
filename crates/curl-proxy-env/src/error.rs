//! What can go wrong, with the exit code the curl command gives for the
//! same failure (`man curl`, EXIT CODES).

use std::fmt;
use std::io;

/// curl's exit codes for the failures this crate reports.
pub mod exit {
    /// 4: a feature this build doesn't have (an `https://` proxy with no
    /// TLS config).
    pub const NOT_BUILT_IN: i32 = 4;
    /// 5: the proxy's name doesn't resolve, or its URL can't be read.
    pub const COULDNT_RESOLVE_PROXY: i32 = 5;
    /// 6: the target's name doesn't resolve (a proxy that takes an
    /// address: SOCKS4 and SOCKS5).
    pub const COULDNT_RESOLVE_HOST: i32 = 6;
    /// 7: nothing accepted the connection, or the proxy's scheme is not
    /// one curl proxies through.
    pub const COULDNT_CONNECT: i32 = 7;
    /// 28: no connection in time.
    pub const OPERATION_TIMEDOUT: i32 = 28;
    /// 35: the TLS handshake with an `https://` proxy failed.
    pub const SSL_CONNECT_ERROR: i32 = 35;
    /// 56: the proxy's answer to CONNECT was not a 2xx, or was cut short.
    pub const RECV_ERROR: i32 = 56;
    /// 60: the `https://` proxy's certificate failed the check.
    pub const PEER_FAILED_VERIFICATION: i32 = 60;
    /// 97: the SOCKS handshake failed.
    pub const PROXY: i32 = 97;
}

/// A proxy setting curl would refuse before connecting anywhere.
///
/// Never holds the proxy URL itself: it may carry the proxy's password.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConfigError {
    /// A scheme curl doesn't proxy through (`ftp://`, or a scheme-less
    /// URL whose host starts `ftp.`, which curl reads as `ftp://`): the
    /// scheme, in lower case. curl exits 7.
    UnsupportedScheme(String),
    /// A URL curl can't read: a blank or control byte, an empty host, a
    /// port that is not a number from 1 to 65535, an IPv6 address that
    /// doesn't parse. What was wrong, in a few words. curl exits 5.
    BadSyntax(&'static str),
}

impl ConfigError {
    /// curl's exit code for this setting.
    pub fn curl_exit_code(&self) -> i32 {
        match self {
            ConfigError::UnsupportedScheme(_) => exit::COULDNT_CONNECT,
            ConfigError::BadSyntax(_) => exit::COULDNT_RESOLVE_PROXY,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnsupportedScheme(s) => write!(f, "unsupported proxy scheme {s:?}"),
            ConfigError::BadSyntax(why) => write!(f, "unsupported proxy syntax: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A connection through a proxy that failed. Nothing of the caller's
/// request was sent in any of these: the tunnel never opened.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The proxy setting itself ([`ConfigError`]).
    Config(ConfigError),
    /// The proxy's name doesn't resolve.
    ResolveProxy(io::Error),
    /// The target's name doesn't resolve (SOCKS4 and SOCKS5 resolve it
    /// here, not at the proxy).
    ResolveHost(io::Error),
    /// Nothing accepted the TCP connection.
    Connect(io::Error),
    /// The deadline passed first.
    TimedOut,
    /// An `https://` proxy, and no TLS config was given
    /// ([`crate::Options::proxy_tls`]).
    TlsNotConfigured,
    /// The TLS handshake with an `https://` proxy failed.
    Tls {
        /// The proxy's certificate failed the check (curl's 60, not 35).
        certificate: bool,
        /// What failed.
        source: io::Error,
    },
    /// The proxy answered CONNECT with this status, not a 2xx.
    TunnelRefused(u16),
    /// The HTTP proxy closed, or answered with something that is not
    /// HTTP, or a read or write failed: what happened.
    HttpProxy(String),
    /// The SOCKS proxy refused, or broke the handshake: what happened.
    Socks(String),
}

impl Error {
    /// curl's exit code for the same failure.
    pub fn curl_exit_code(&self) -> i32 {
        match self {
            Error::Config(c) => c.curl_exit_code(),
            Error::ResolveProxy(_) => exit::COULDNT_RESOLVE_PROXY,
            Error::ResolveHost(_) => exit::COULDNT_RESOLVE_HOST,
            Error::Connect(_) => exit::COULDNT_CONNECT,
            Error::TimedOut => exit::OPERATION_TIMEDOUT,
            Error::TlsNotConfigured => exit::NOT_BUILT_IN,
            Error::Tls { certificate, .. } => {
                if *certificate {
                    exit::PEER_FAILED_VERIFICATION
                } else {
                    exit::SSL_CONNECT_ERROR
                }
            }
            Error::TunnelRefused(_) | Error::HttpProxy(_) => exit::RECV_ERROR,
            Error::Socks(_) => exit::PROXY,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(c) => c.fmt(f),
            Error::ResolveProxy(e) => write!(f, "could not resolve the proxy: {e}"),
            Error::ResolveHost(e) => write!(f, "could not resolve the host: {e}"),
            Error::Connect(e) => write!(f, "could not connect to the proxy: {e}"),
            Error::TimedOut => f.write_str("the proxy connection timed out"),
            Error::TlsNotConfigured => f.write_str("an https:// proxy needs a TLS config"),
            Error::Tls { source, .. } => write!(f, "TLS with the proxy: {source}"),
            Error::TunnelRefused(code) => write!(f, "CONNECT tunnel failed, response {code}"),
            Error::HttpProxy(why) => write!(f, "proxy: {why}"),
            Error::Socks(why) => write!(f, "SOCKS: {why}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Config(c) => Some(c),
            Error::ResolveProxy(e) | Error::ResolveHost(e) | Error::Connect(e) => Some(e),
            Error::Tls { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<ConfigError> for Error {
    fn from(c: ConfigError) -> Error {
        Error::Config(c)
    }
}
