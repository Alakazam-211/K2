//! A minimal gRPC client for the Android emulator's controller API
//! (`android.emulation.control.EmulatorController`, loopback only, bearer
//! token from the emulator's discovery file).
//!
//! No tonic and no protoc: gRPC over HTTP/2 is a POST with
//! `content-type: application/grpc` whose body is length-prefixed
//! protobuf messages (1 byte compressed flag + u32 big-endian length), and
//! a `grpc-status` trailer. We hand-write the handful of messages we use
//! with `prost` derives (field numbers from the SDK's
//! `emulator/lib/emulator_controller.proto`, emulator 37.2).
//!
//! The token is sent only in the `authorization` header to 127.0.0.1; it
//! never appears in argv or logs.

use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};

pub const SERVICE: &str = "android.emulation.control.EmulatorController";

/// Largest message we accept (a full-resolution RGBA frame of a big tablet
/// is ~17 MB; we ask for points resolution, so ~1–2 MB is normal).
pub const MAX_MESSAGE: usize = 32 * 1024 * 1024;

// ── messages (hand-written prost) ────────────────────────────────────

#[derive(Clone, PartialEq, prost::Message)]
pub struct Empty {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Rotation {
    /// 0 portrait, 1 landscape, 2 reverse portrait, 3 reverse landscape.
    #[prost(int32, tag = "1")]
    pub rotation: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImageFormat {
    /// 0 PNG, 1 RGBA8888, 2 RGB888.
    #[prost(int32, tag = "1")]
    pub format: i32,
    #[prost(message, optional, tag = "2")]
    pub rotation: Option<Rotation>,
    #[prost(uint32, tag = "3")]
    pub width: u32,
    #[prost(uint32, tag = "4")]
    pub height: u32,
    #[prost(uint32, tag = "5")]
    pub display: u32,
}

pub const FMT_PNG: i32 = 0;
pub const FMT_RGBA8888: i32 = 1;
pub const FMT_RGB888: i32 = 2;

#[derive(Clone, PartialEq, prost::Message)]
pub struct Image {
    #[prost(message, optional, tag = "1")]
    pub format: Option<ImageFormat>,
    #[prost(uint32, tag = "2")]
    pub width: u32,
    #[prost(uint32, tag = "3")]
    pub height: u32,
    #[prost(bytes = "vec", tag = "4")]
    pub image: Vec<u8>,
    #[prost(uint32, tag = "5")]
    pub seq: u32,
    #[prost(uint64, tag = "6")]
    pub timestamp_us: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Touch {
    #[prost(int32, tag = "1")]
    pub x: i32,
    #[prost(int32, tag = "2")]
    pub y: i32,
    #[prost(int32, tag = "3")]
    pub identifier: i32,
    /// 0 = lifted.
    #[prost(int32, tag = "4")]
    pub pressure: i32,
    #[prost(int32, tag = "5")]
    pub touch_major: i32,
    #[prost(int32, tag = "6")]
    pub touch_minor: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TouchEvent {
    #[prost(message, repeated, tag = "1")]
    pub touches: Vec<Touch>,
    #[prost(int32, tag = "2")]
    pub display: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct KeyboardEvent {
    /// 0 Usb, 1 Evdev, 2 XKB, 3 Win, 4 Mac.
    #[prost(int32, tag = "1")]
    pub code_type: i32,
    /// 0 keydown, 1 keyup, 2 keypress.
    #[prost(int32, tag = "2")]
    pub event_type: i32,
    #[prost(int32, tag = "3")]
    pub key_code: i32,
    /// A W3C key name (`Enter`, `Backspace`, `GoHome`…).
    #[prost(string, tag = "4")]
    pub key: String,
    /// Text to type (each character is sent as a key press).
    #[prost(string, tag = "5")]
    pub text: String,
}

pub const KEY_KEYDOWN: i32 = 0;
pub const KEY_KEYUP: i32 = 1;
pub const KEY_KEYPRESS: i32 = 2;

#[derive(Clone, PartialEq, prost::Message)]
pub struct GpsState {
    #[prost(bool, tag = "1")]
    pub passive_update: bool,
    #[prost(double, tag = "2")]
    pub latitude: f64,
    #[prost(double, tag = "3")]
    pub longitude: f64,
    #[prost(double, tag = "4")]
    pub speed: f64,
    #[prost(double, tag = "5")]
    pub bearing: f64,
    #[prost(double, tag = "6")]
    pub altitude: f64,
    #[prost(int32, tag = "7")]
    pub satellites: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct EmulatorStatus {
    #[prost(string, tag = "1")]
    pub version: String,
    #[prost(uint64, tag = "2")]
    pub uptime: u64,
    #[prost(bool, tag = "3")]
    pub booted: bool,
}

// ── framing ──────────────────────────────────────────────────────────

/// Length-prefix one message.
pub fn encode_frame<M: prost::Message>(m: &M) -> Bytes {
    let len = m.encoded_len();
    let mut b = BytesMut::with_capacity(5 + len);
    b.put_u8(0);
    b.put_u32(len as u32);
    m.encode(&mut b).expect("BytesMut grows");
    b.freeze()
}

/// Pulls whole gRPC messages out of a byte stream.
#[derive(Default)]
pub struct Deframer {
    buf: BytesMut,
}

impl Deframer {
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// The next whole message body, if one has arrived.
    pub fn next(&mut self) -> Result<Option<Bytes>, String> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let compressed = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if compressed != 0 {
            return Err("compressed gRPC messages are not supported".into());
        }
        if len > MAX_MESSAGE {
            return Err(format!("gRPC message of {len} bytes is over the {MAX_MESSAGE} byte cap"));
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        self.buf.advance(5);
        Ok(Some(self.buf.split_to(len).freeze()))
    }
}

// ── client ───────────────────────────────────────────────────────────

/// Errors from a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrpcError {
    Connect(String),
    Http(String),
    /// A non-OK `grpc-status` (code, message).
    Status(i32, String),
    Decode(String),
    Timeout,
}

impl std::fmt::Display for GrpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcError::Connect(e) => write!(f, "emulator gRPC connect: {e}"),
            GrpcError::Http(e) => write!(f, "emulator gRPC: {e}"),
            GrpcError::Status(c, m) => write!(f, "emulator gRPC status {c}: {m}"),
            GrpcError::Decode(e) => write!(f, "emulator gRPC decode: {e}"),
            GrpcError::Timeout => write!(f, "emulator gRPC call timed out"),
        }
    }
}

/// The emulator's controller on `127.0.0.1:<port>`.
#[derive(Clone)]
pub struct EmuGrpc {
    port: u16,
    token: Arc<str>,
}

impl std::fmt::Debug for EmuGrpc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmuGrpc").field("port", &self.port).field("token", &"<redacted>").finish()
    }
}

type SendReq = h2::client::SendRequest<Bytes>;

impl EmuGrpc {
    pub fn new(port: u16, token: impl Into<Arc<str>>) -> EmuGrpc {
        EmuGrpc { port, token: token.into() }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    async fn connect(&self) -> Result<SendReq, GrpcError> {
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", self.port))
            .await
            .map_err(|e| GrpcError::Connect(e.to_string()))?;
        let _ = tcp.set_nodelay(true);
        let (send, conn) = h2::client::Builder::new()
            .initial_window_size(8 * 1024 * 1024)
            .initial_connection_window_size(16 * 1024 * 1024)
            .max_frame_size(1024 * 1024)
            .handshake::<_, Bytes>(tcp)
            .await
            .map_err(|e| GrpcError::Connect(e.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        Ok(send)
    }

    fn request(&self, method: &str) -> http::Request<()> {
        http::Request::builder()
            .method("POST")
            .uri(format!("http://127.0.0.1:{}/{SERVICE}/{method}", self.port))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .header("authorization", format!("Bearer {}", self.token))
            .body(())
            .expect("static request parts")
    }

    /// Open a call; returns the response body stream.
    async fn open<M: prost::Message>(&self, method: &str, msg: &M) -> Result<h2::RecvStream, GrpcError> {
        let send = self.connect().await?;
        let mut send = send.ready().await.map_err(|e| GrpcError::Http(e.to_string()))?;
        let (resp, mut stream) = send.send_request(self.request(method), false).map_err(|e| GrpcError::Http(e.to_string()))?;
        stream.send_data(encode_frame(msg), true).map_err(|e| GrpcError::Http(e.to_string()))?;
        let resp = resp.await.map_err(|e| GrpcError::Http(e.to_string()))?;
        if resp.status() != http::StatusCode::OK {
            return Err(GrpcError::Http(format!("HTTP {}", resp.status())));
        }
        // A trailers-only error puts grpc-status in the headers.
        if let Some(code) = header_status(resp.headers()) {
            if code.0 != 0 {
                return Err(GrpcError::Status(code.0, code.1));
            }
        }
        Ok(resp.into_body())
    }

    /// A unary call.
    pub async fn unary<M: prost::Message, R: prost::Message + Default>(&self, method: &str, msg: &M) -> Result<R, GrpcError> {
        let mut body = self.open(method, msg).await?;
        let mut de = Deframer::default();
        let mut out: Option<R> = None;
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|e| GrpcError::Http(e.to_string()))?;
            let _ = body.flow_control().release_capacity(chunk.len());
            de.push(&chunk);
            while let Some(m) = de.next().map_err(GrpcError::Decode)? {
                out = Some(R::decode(m).map_err(|e| GrpcError::Decode(e.to_string()))?);
            }
        }
        check_trailers(&mut body).await?;
        out.ok_or_else(|| GrpcError::Decode(format!("{method}: no response message")))
    }

    /// A server-streaming call; `each` gets every message and returns
    /// false to stop.
    pub async fn stream<M: prost::Message, R: prost::Message + Default>(
        &self,
        method: &str,
        msg: &M,
        mut each: impl FnMut(R) -> bool,
    ) -> Result<(), GrpcError> {
        let mut body = self.open(method, msg).await?;
        let mut de = Deframer::default();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|e| GrpcError::Http(e.to_string()))?;
            let _ = body.flow_control().release_capacity(chunk.len());
            de.push(&chunk);
            while let Some(m) = de.next().map_err(GrpcError::Decode)? {
                if !each(R::decode(m).map_err(|e| GrpcError::Decode(e.to_string()))?) {
                    return Ok(());
                }
            }
        }
        check_trailers(&mut body).await
    }

    pub async fn status(&self) -> Result<EmulatorStatus, GrpcError> {
        self.unary("getStatus", &Empty {}).await
    }

    pub async fn send_touch(&self, ev: &TouchEvent) -> Result<(), GrpcError> {
        self.unary::<_, Empty>("sendTouch", ev).await.map(|_| ())
    }

    pub async fn send_key(&self, ev: &KeyboardEvent) -> Result<(), GrpcError> {
        self.unary::<_, Empty>("sendKey", ev).await.map(|_| ())
    }

    pub async fn set_gps(&self, g: &GpsState) -> Result<(), GrpcError> {
        self.unary::<_, Empty>("setGps", g).await.map(|_| ())
    }

    pub async fn screenshot(&self, fmt: &ImageFormat) -> Result<Image, GrpcError> {
        self.unary("getScreenshot", fmt).await
    }

    /// Run `fut` with a deadline.
    pub async fn with_timeout<T>(d: Duration, fut: impl std::future::Future<Output = Result<T, GrpcError>>) -> Result<T, GrpcError> {
        tokio::time::timeout(d, fut).await.map_err(|_| GrpcError::Timeout)?
    }
}

fn header_status(h: &http::HeaderMap) -> Option<(i32, String)> {
    let code = h.get("grpc-status")?.to_str().ok()?.parse::<i32>().ok()?;
    let msg = h.get("grpc-message").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    Some((code, msg))
}

async fn check_trailers(body: &mut h2::RecvStream) -> Result<(), GrpcError> {
    match body.trailers().await {
        Ok(Some(t)) => match header_status(&t) {
            Some((0, _)) | None => Ok(()),
            Some((c, m)) => Err(GrpcError::Status(c, m)),
        },
        Ok(None) => Ok(()),
        Err(e) => Err(GrpcError::Http(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn frames_round_trip_and_split_across_chunks() {
        let img = Image { width: 3, height: 2, image: vec![7u8; 18], seq: 9, ..Default::default() };
        let a = encode_frame(&img);
        let b = encode_frame(&Empty {});
        let mut all = a.to_vec();
        all.extend_from_slice(&b);
        let mut de = Deframer::default();
        de.push(&all[..4]);
        assert_eq!(de.next().expect("ok"), None);
        de.push(&all[4..10]);
        assert_eq!(de.next().expect("ok"), None);
        de.push(&all[10..]);
        let m1 = de.next().expect("ok").expect("first");
        assert_eq!(Image::decode(m1).expect("decodes"), img);
        let m2 = de.next().expect("ok").expect("second");
        assert!(m2.is_empty());
        assert_eq!(de.next().expect("ok"), None);
    }

    #[test]
    fn deframer_refuses_compressed_and_oversized() {
        let mut de = Deframer::default();
        de.push(&[1, 0, 0, 0, 1, 0]);
        assert!(de.next().is_err());
        let mut de = Deframer::default();
        de.push(&[0, 0xff, 0xff, 0xff, 0xff]);
        assert!(de.next().is_err());
    }

    #[test]
    fn debug_never_shows_the_token() {
        let g = EmuGrpc::new(8554, "s3cr3t-token");
        assert!(!format!("{g:?}").contains("s3cr3t"));
    }
}
