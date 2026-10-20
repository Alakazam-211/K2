//! The websocket connection: TCP (optionally TLS) setup, tungstenite handshake, the
//! `zstd-stream` decoder and frame decoding (S25 §10.3).

use std::io::Write as _;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use sew_proto::gateway::Frame;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use url::Url;

/// The byte stream under the websocket.
pub(crate) enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// The websocket.
pub(crate) type Ws = WebSocketStream<Stream>;

/// Socket options (contracts rev 35: S26 needs a local address and buffer sizes).
#[derive(Debug, Clone, Default)]
pub(crate) struct SocketOpts {
    pub local_addr: Option<SocketAddr>,
    pub recv_buffer_bytes: Option<u32>,
    pub send_buffer_bytes: Option<u32>,
    pub connect_timeout: Duration,
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| format!("tls: {e}"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Opens `url` (`ws://` or `wss://`, with its query) and completes the websocket handshake.
pub(crate) async fn connect(url: &Url, opts: &SocketOpts) -> Result<Ws, String> {
    let fut = async {
        let tls = match url.scheme() {
            "ws" => false,
            "wss" => true,
            other => return Err(format!("gateway URL scheme {other:?} is not ws or wss")),
        };
        let host = url
            .host_str()
            .ok_or_else(|| format!("gateway URL {url} has no host"))?
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| format!("gateway URL {url} has no port"))?;
        let bare = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((bare.as_str(), port))
            .await
            .map_err(|e| format!("resolve {host}: {e}"))?
            .collect();
        let addr = match opts.local_addr {
            Some(local) => addrs
                .iter()
                .find(|a| a.is_ipv4() == local.is_ipv4())
                .copied(),
            None => addrs.first().copied(),
        }
        .ok_or_else(|| format!("resolve {host}: no usable address"))?;
        let sock = if addr.is_ipv4() {
            TcpSocket::new_v4()
        } else {
            TcpSocket::new_v6()
        }
        .map_err(|e| format!("socket: {e}"))?;
        if let Some(n) = opts.recv_buffer_bytes {
            sock.set_recv_buffer_size(n)
                .map_err(|e| format!("SO_RCVBUF: {e}"))?;
        }
        if let Some(n) = opts.send_buffer_bytes {
            sock.set_send_buffer_size(n)
                .map_err(|e| format!("SO_SNDBUF: {e}"))?;
        }
        if let Some(local) = opts.local_addr {
            sock.bind(local).map_err(|e| format!("bind {local}: {e}"))?;
        }
        let tcp = sock
            .connect(addr)
            .await
            .map_err(|e| format!("connect {addr}: {e}"))?;
        let _ = tcp.set_nodelay(true);
        let stream = if tls {
            let name = rustls::pki_types::ServerName::try_from(bare.clone())
                .map_err(|e| format!("tls server name {bare:?}: {e}"))?;
            let connector = tokio_rustls::TlsConnector::from(tls_config()?);
            let s = connector
                .connect(name, tcp)
                .await
                .map_err(|e| format!("tls handshake with {host}: {e}"))?;
            Stream::Tls(Box::new(s))
        } else {
            Stream::Plain(tcp)
        };
        let mut cfg = WebSocketConfig::default();
        cfg.max_message_size = Some(64 << 20);
        cfg.max_frame_size = Some(16 << 20);
        let (ws, _resp) =
            tokio_tungstenite::client_async_with_config(url.as_str(), stream, Some(cfg))
                .await
                .map_err(|e| format!("websocket handshake: {e}"))?;
        Ok(ws)
    };
    match tokio::time::timeout(opts.connect_timeout, fut).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "connect to {} timed out after {:?}",
            url.host_str().unwrap_or("?"),
            opts.connect_timeout
        )),
    }
}

/// One streaming zstd decoder per connection: each binary frame is fed in and must yield whole
/// JSON messages.
pub(crate) struct ZstdStream {
    dec: zstd::stream::write::Decoder<'static, Vec<u8>>,
}

impl ZstdStream {
    pub(crate) fn new() -> Result<ZstdStream, String> {
        let dec = zstd::stream::write::Decoder::new(Vec::new())
            .map_err(|e| format!("zstd decoder: {e}"))?;
        Ok(ZstdStream { dec })
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        self.dec
            .write_all(bytes)
            .and_then(|()| self.dec.flush())
            .map_err(|e| format!("zstd-stream decode: {e}"))?;
        Ok(std::mem::take(self.dec.get_mut()))
    }
}

/// Every JSON frame in `bytes` (one or more back to back).
pub(crate) fn parse_frames(bytes: &[u8]) -> Result<Vec<Frame>, String> {
    let mut out = Vec::new();
    for f in serde_json::Deserializer::from_slice(bytes).into_iter::<Frame>() {
        out.push(f.map_err(|e| format!("frame JSON: {e}"))?);
    }
    if out.is_empty() {
        return Err("a frame with no JSON message".to_owned());
    }
    Ok(out)
}
