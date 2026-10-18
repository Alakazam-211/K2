//! A small blocking client: connect, handshake, request/response with event
//! buffering, SCM_RIGHTS, and data-stream attach. Used by the daemon's
//! `ptyd_client` and by the tests.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::auth::{self, KEY_LEN};
use crate::paths::Paths;
use crate::proto::{self, Frame, FrameDecoder, Reply, Request, Role, PROTO_MAX};
use crate::sys;

/// What the handshake learned.
#[derive(Debug, Clone)]
pub struct Hello {
    pub ptyd_version: String,
    pub proto: u32,
    pub boot_id: String,
    pub sessions: usize,
}

#[derive(Debug)]
pub enum ConnectError {
    Io(io::Error),
    /// ptyd refused: `bad_mac|proto|busy|uid`.
    Refused { code: String, detail: String },
    Protocol(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Io(e) => write!(f, "io: {e}"),
            ConnectError::Refused { code, detail } => write!(f, "refused: {code} {detail}"),
            ConnectError::Protocol(s) => write!(f, "protocol: {s}"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> Self {
        ConnectError::Io(e)
    }
}

/// Raw framed connection with fd-aware reads.
pub struct Conn {
    pub stream: UnixStream,
    dec: FrameDecoder,
    fds: VecDeque<OwnedFd>,
}

impl Conn {
    pub fn connect(sock: &Path) -> io::Result<Conn> {
        let stream = UnixStream::connect(sock)?;
        Ok(Conn { stream, dec: FrameDecoder::default(), fds: VecDeque::new() })
    }

    pub fn set_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(t)?;
        self.stream.set_write_timeout(t)
    }

    pub fn send<T: serde::Serialize>(&mut self, v: &T) -> io::Result<()> {
        proto::write_frame(&mut self.stream, v)
    }

    pub fn send_with_fd<T: serde::Serialize>(&mut self, v: &T, fd: &impl AsRawFd) -> io::Result<()> {
        let bytes = proto::encode(v)?;
        let n = sys::send_with_fd(self.stream.as_raw_fd(), &bytes, Some(fd.as_raw_fd()))?;
        self.stream.write_all(&bytes[n..])
    }

    /// Next frame, blocking.
    pub fn recv(&mut self) -> io::Result<Frame<Reply>> {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match self.dec.next_frame() {
                Ok(Some(f)) => return serde_json::from_slice(&f).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
                Ok(None) => {}
                Err(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "frame_too_large")),
            }
            let (n, fds) = sys::recv_with_fds(self.stream.as_raw_fd(), &mut buf)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "ptyd closed the connection"));
            }
            self.fds.extend(fds);
            self.dec.push(&buf[..n]);
        }
    }

    pub fn take_fd(&mut self) -> Option<OwnedFd> {
        self.fds.pop_front()
    }

    /// Bytes the decoder read past the last frame (raw stream data).
    pub fn take_rest(&mut self) -> Vec<u8> {
        self.dec.take_rest()
    }

    /// Run the handshake on a fresh connection.
    pub fn handshake(&mut self, key: &[u8; KEY_LEN], client_version: &str, role: Role, takeover: bool) -> Result<Hello, ConnectError> {
        let ch = self.recv()?;
        let Reply::Challenge { nonce, ptyd_version, proto_min, proto_max, boot_id } = ch.body else {
            return Err(ConnectError::Protocol(format!("expected challenge, got {:?}", ch.body)));
        };
        let nonce = auth::unhex(&nonce).ok_or_else(|| ConnectError::Protocol("bad nonce".into()))?;
        let proto = PROTO_MAX.min(proto_max);
        if proto < proto_min {
            return Err(ConnectError::Refused { code: "proto".into(), detail: format!("ptyd {proto_min}..{proto_max}") });
        }
        self.send(&Frame::new(
            None,
            Request::Hello {
                proto,
                client: "k2-daemon".into(),
                client_version: client_version.into(),
                mac: auth::mac(key, &nonce, client_version),
                takeover,
                role,
            },
        ))?;
        match self.recv()?.body {
            Reply::Welcome { proto, sessions } => Ok(Hello { ptyd_version, proto, boot_id, sessions }),
            Reply::Refused { code, detail } => Err(ConnectError::Refused { code, detail }),
            other => Err(ConnectError::Protocol(format!("expected welcome, got {other:?}"))),
        }
    }
}

/// Owner client: request/response multiplexing; events are queued.
pub struct Client {
    pub conn: Conn,
    pub hello: Hello,
    next_id: u64,
    pub events: VecDeque<Reply>,
    paths: Paths,
    key: [u8; KEY_LEN],
    client_version: String,
}

impl Client {
    /// Connect as the owner.
    pub fn connect_owner(paths: &Paths, client_version: &str, takeover: bool) -> Result<Client, ConnectError> {
        let key = auth::read_key(&paths.key)?;
        let mut conn = Conn::connect(&paths.client_sock())?;
        conn.set_timeout(Some(Duration::from_secs(10)))?;
        let hello = conn.handshake(&key, client_version, Role::Owner, takeover)?;
        Ok(Client {
            conn,
            hello,
            next_id: 1,
            events: VecDeque::new(),
            paths: paths.clone(),
            key,
            client_version: client_version.to_string(),
        })
    }

    pub fn request(&mut self, body: Request) -> io::Result<Reply> {
        let id = self.next_id;
        self.next_id += 1;
        self.conn.send(&Frame::new(Some(id), body))?;
        self.wait(id)
    }

    /// Request carrying an fd (`hold_fd`).
    pub fn request_with_fd(&mut self, body: Request, fd: &impl AsRawFd) -> io::Result<Reply> {
        let id = self.next_id;
        self.next_id += 1;
        self.conn.send_with_fd(&Frame::new(Some(id), body), fd)?;
        self.wait(id)
    }

    fn wait(&mut self, id: u64) -> io::Result<Reply> {
        loop {
            let f = self.conn.recv()?;
            match f.id {
                Some(i) if i == id => return Ok(f.body),
                Some(_) => continue,
                None => self.events.push_back(f.body),
            }
        }
    }

    /// Block for the next event (or return a queued one).
    pub fn next_event(&mut self) -> io::Result<Reply> {
        if let Some(e) = self.events.pop_front() {
            return Ok(e);
        }
        loop {
            let f = self.conn.recv()?;
            if f.id.is_none() {
                return Ok(f.body);
            }
        }
    }

    /// The fd that rode on the last fd-carrying reply.
    pub fn take_fd(&mut self) -> Option<OwnedFd> {
        self.conn.take_fd()
    }

    /// Open a data stream for `sid` from offset `from`.
    pub fn attach(&self, sid: &str, from: u64) -> Result<DataStream, ConnectError> {
        let mut conn = Conn::connect(&self.paths.client_sock())?;
        conn.set_timeout(Some(Duration::from_secs(10)))?;
        conn.handshake(&self.key, &self.client_version, Role::Data, false)?;
        conn.send(&Frame::new(Some(1), Request::Attach { sid: sid.into(), from }))?;
        match conn.recv()?.body {
            Reply::Attached { start, end } => {
                let pending = conn.take_rest();
                conn.set_timeout(None)?;
                Ok(DataStream { stream: conn.stream, start, end, pending })
            }
            Reply::Error { code, detail } => Err(ConnectError::Refused { code, detail }),
            other => Err(ConnectError::Protocol(format!("expected attached, got {other:?}"))),
        }
    }
}

/// A raw byte stream for one session after `attached`.
pub struct DataStream {
    pub stream: UnixStream,
    /// First offset this stream delivers.
    pub start: u64,
    /// Ring end at attach time (bytes below it are backlog / replay).
    pub end: u64,
    /// Stream bytes read together with the `attached` frame.
    pending: Vec<u8>,
}

impl Read for DataStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.pending.is_empty() {
            let n = buf.len().min(self.pending.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        self.stream.read(buf)
    }
}

impl Write for DataStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl DataStream {
    /// Split into a reader that first yields the pending bytes, and the raw socket.
    pub fn into_parts(self) -> (Vec<u8>, UnixStream) {
        (self.pending, self.stream)
    }
}
