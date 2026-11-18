//! A logging stub daemon: loopback TCP and a Unix socket. It keeps every
//! request's raw bytes (head and body, by `Content-Length`) and answers
//! from a queue, then with a plain 200. No real daemon, no K2 home.
//!
//! The no-answer replies, as `tests/cli/cell_socket_post_no_resend.sh`
//! (0.46.15) makes them: [`Reply::Hang`] (curl 28), [`Reply::Close`]
//! (52), [`Reply::Reset`] (56), an empty 200 ([`Reply::Answer`] with no
//! body) and [`Reply::Raw`] bytes that are not HTTP (status 000, curl 1).

#![cfg(unix)]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One answer.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A status and a JSON body.
    Answer(u16, String),
    /// The same, with these header lines too, after `Content-Type`.
    Headed(u16, Vec<(String, String)>, String),
    /// Writes `content` to `file`, then answers.
    WriteThen { file: PathBuf, content: String, status: u16, body: String },
    /// Reads the request, waits, and closes with nothing sent.
    Hang(Duration),
    /// Reads the request and closes with nothing sent.
    Close,
    /// Logs the request without taking it off the connection, then
    /// closes: the unread bytes make the close a reset (TCP), and on
    /// Linux the peer's next read ECONNRESET (a Unix socket; macOS reads
    /// an end of stream there).
    Reset,
    /// Reads the request, sends these bytes as they are and closes.
    Raw(Vec<u8>),
}

/// A request as it came in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    /// `tcp` or `uds`.
    pub channel: &'static str,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
struct State {
    log: Vec<Logged>,
    queue: VecDeque<Reply>,
}

/// The stub. Its threads live as long as the test process.
pub struct Stub {
    pub port: u16,
    pub sock: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Stub {
    /// Starts one, its socket at `sock` (keep it short: macOS allows
    /// about 104 bytes).
    pub fn start(sock: &Path) -> Stub {
        let state = Arc::new(Mutex::new(State::default()));
        let tcp = TcpListener::bind("127.0.0.1:0").expect("bind the stub's TCP port");
        let port = tcp.local_addr().expect("the stub's port").port();
        if sock.exists() {
            std::fs::remove_file(sock).expect("remove an old stub socket");
        }
        let uds = UnixListener::bind(sock).unwrap_or_else(|e| panic!("bind {}: {e}", sock.display()));
        let s = Arc::clone(&state);
        std::thread::spawn(move || {
            for conn in tcp.incoming() {
                let conn = conn.expect("accept on the stub's TCP port");
                let s = Arc::clone(&s);
                std::thread::spawn(move || serve(conn, "tcp", &s));
            }
        });
        let s = Arc::clone(&state);
        std::thread::spawn(move || {
            for conn in uds.incoming() {
                let conn = conn.expect("accept on the stub's socket");
                let s = Arc::clone(&s);
                std::thread::spawn(move || serve(conn, "uds", &s));
            }
        });
        Stub { port, sock: sock.to_path_buf(), state }
    }

    /// Empties the log and sets the answers to come.
    pub fn reset(&self, replies: &[Reply]) {
        let mut s = self.state.lock().expect("the stub's lock");
        s.log.clear();
        s.queue = replies.iter().cloned().collect();
    }

    /// The requests so far.
    pub fn log(&self) -> Vec<Logged> {
        self.state.lock().expect("the stub's lock").log.clone()
    }

    /// Answers not used yet.
    pub fn left(&self) -> usize {
        self.state.lock().expect("the stub's lock").queue.len()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sock);
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The whole request is in `got`: its head, and its body by
/// `Content-Length`.
fn complete(got: &[u8]) -> bool {
    let Some(at) = find(got, b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&got[..at]);
    let length: usize = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().to_string()))
        .map_or(0, |v| v.parse().expect("a numeric Content-Length"));
    got.len() >= at + 4 + length
}

/// The request as it waits on `fd`, read with `MSG_PEEK`: nothing is
/// taken off the connection.
fn peek_request(fd: RawFd) -> Vec<u8> {
    let mut buf = vec![0u8; 256 * 1024];
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        // SAFETY: `fd` is this thread's open connection and `buf` is
        // writable for its whole length.
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_PEEK) };
        let Ok(n) = usize::try_from(n) else {
            return Vec::new();
        };
        if n == 0 {
            return Vec::new();
        }
        if complete(&buf[..n]) || Instant::now() >= until {
            return buf[..n].to_vec();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn serve<S: Read + Write + AsRawFd>(mut conn: S, channel: &'static str, state: &Mutex<State>) {
    if matches!(state.lock().expect("the stub's lock").queue.front(), Some(Reply::Reset)) {
        let got = peek_request(conn.as_raw_fd());
        let mut s = state.lock().expect("the stub's lock");
        s.log.push(Logged { channel, bytes: got });
        s.queue.pop_front();
        drop(s);
        drop(conn);
        return;
    }
    let mut got = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let head_end = loop {
        if let Some(at) = find(&got, b"\r\n\r\n") {
            break at + 4;
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&got[..head_end]).to_string();
    let length: usize = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().to_string()))
        .map(|v| v.parse().expect("a numeric Content-Length"))
        .unwrap_or(0);
    while got.len() < head_end + length {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    }
    let reply = {
        let mut s = state.lock().expect("the stub's lock");
        s.log.push(Logged { channel, bytes: got });
        s.queue.pop_front().unwrap_or(Reply::Answer(200, r#"{"ok":true}"#.to_string()))
    };
    let (status, extra, body) = match reply {
        Reply::Answer(status, body) => (status, Vec::new(), body),
        Reply::Headed(status, extra, body) => (status, extra, body),
        Reply::WriteThen { file, content, status, body } => {
            std::fs::write(&file, content).unwrap_or_else(|e| panic!("write {}: {e}", file.display()));
            (status, Vec::new(), body)
        }
        Reply::Hang(d) => {
            std::thread::sleep(d);
            return;
        }
        Reply::Close | Reply::Reset => return,
        Reply::Raw(bytes) => {
            let _ = conn.write_all(&bytes);
            return;
        }
    };
    let extra: String = extra.iter().map(|(n, v)| format!("{n}: {v}\r\n")).collect();
    let out = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = conn.write_all(out.as_bytes());
}
