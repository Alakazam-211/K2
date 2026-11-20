//! `k5-cli app shell` against the shell CLI's (`cmd_app_shell` and its
//! Python WebSocket client, `cli/src/app-doctor.sh`), each side against
//! its own run of a stub WebSocket server ([`WsStub`]) that plays a
//! script and logs what the client sent: the handshake's request line
//! and headers (the random `Sec-WebSocket-Key` checked, then masked), and
//! each frame (checked to be masked; binary frames in a row joined, as a
//! pipe's reads may split them anywhere).
//!
//! - The words: the usage line, an unknown flag, help never acts.
//! - The refusals: a non-101 answer with the daemon's error, an error
//!   that is a string, no body, not JSON, a hint missing, a close with
//!   nothing; no daemon (the Python's traceback).
//! - A scripted session with stdin not a terminal: the banner and notice
//!   on stderr, binary frames on stdout (short, 16-bit and 64-bit
//!   lengths), stdin sent then `\x04` at its end, the exit code (in range,
//!   out of it, a boolean, none), an error frame, a ping's pong, frames
//!   that are no JSON, the server hanging up, frames in the handshake's
//!   own packet, the passport as the credential; a text frame that is
//!   JSON but no object, a banner whose text is no string (tracebacks).
//! - At a terminal ([`at_a_terminal`], the pty driver of
//!   `tests/cli/clone_security.sh`): the size in the query, raw mode
//!   while it runs (no echo, no line editing), what is typed sent as it
//!   is, one resize (SIGWINCH) reaching the server as a resize message,
//!   the exit code, and the terminal put back as it was.
//!
//! The credential rides the `Authorization` header and never the URL.

#![cfg(unix)]

mod common;

use common::parity::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VERB: &str = "app";

/// One step of the server's script.
#[derive(Debug, Clone)]
enum Step {
    /// Write these bytes (a handshake answer, frames).
    Raw(Vec<u8>),
    /// Read the client's frames until the binary ones so far hold this.
    Binary(&'static [u8]),
    /// Read the client's frames until a text frame holds this.
    Text(&'static str),
    /// Read the client's frames until a pong.
    Pong,
    /// Close the connection now (no close frame).
    Hangup,
}

/// `101 Switching Protocols`.
const UPGRADE: &[u8] =
    b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";

/// A server frame (never masked).
fn frame(op: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![0x80 | op];
    let n = data.len();
    if n < 126 {
        f.push(n as u8);
    } else if n < 65536 {
        f.push(126);
        f.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        f.push(127);
        f.extend_from_slice(&(n as u64).to_be_bytes());
    }
    f.extend_from_slice(data);
    f
}

fn text(t: &str) -> Step {
    Step::Raw(frame(1, t.as_bytes()))
}

fn binary(b: &[u8]) -> Step {
    Step::Raw(frame(2, b))
}

fn close() -> Step {
    Step::Raw(frame(8, b""))
}

/// What one connection logged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Log {
    /// The request line and headers, the key masked.
    head: Vec<String>,
    /// `(opcode, payload)`, binary frames in a row joined.
    frames: Vec<(u8, Vec<u8>)>,
}

/// A WebSocket server on a port of its own: every connection plays the
/// script set last, and its log is kept.
struct WsStub {
    port: u16,
    script: Arc<Mutex<Vec<Step>>>,
    logs: Arc<Mutex<Vec<Log>>>,
}

impl WsStub {
    fn start() -> WsStub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let port = listener.local_addr().expect("its address").port();
        let script: Arc<Mutex<Vec<Step>>> = Arc::default();
        let logs: Arc<Mutex<Vec<Log>>> = Arc::default();
        let (s, l) = (script.clone(), logs.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let steps = s.lock().expect("the script").clone();
                let log = serve(conn, &steps);
                l.lock().expect("the logs").push(log);
            }
        });
        WsStub { port, script, logs }
    }

    fn play(&self, steps: Vec<Step>) {
        *self.script.lock().expect("the script") = steps;
        self.logs.lock().expect("the logs").clear();
    }

    fn take(&self) -> Vec<Log> {
        // A side's run ends after its last frame went out; the stub may
        // still be reading it.
        std::thread::sleep(Duration::from_millis(200));
        std::mem::take(&mut *self.logs.lock().expect("the logs"))
    }
}

/// Reads one client frame: it must be masked.
fn read_frame(c: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    c.read_exact(&mut h).ok()?;
    assert!(h[1] & 0x80 != 0, "a client frame that is not masked");
    let mut n = u64::from(h[1] & 0x7F);
    if n == 126 {
        let mut b = [0u8; 2];
        c.read_exact(&mut b).ok()?;
        n = u64::from(u16::from_be_bytes(b));
    } else if n == 127 {
        let mut b = [0u8; 8];
        c.read_exact(&mut b).ok()?;
        n = u64::from_be_bytes(b);
    }
    let mut m = [0u8; 4];
    c.read_exact(&mut m).ok()?;
    let mut data = vec![0u8; usize::try_from(n).ok()?];
    c.read_exact(&mut data).ok()?;
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= m[i % 4];
    }
    Some((h[0] & 0x0F, data))
}

fn push(frames: &mut Vec<(u8, Vec<u8>)>, op: u8, data: Vec<u8>) {
    match frames.last_mut() {
        Some((2, last)) if op == 2 => last.extend(data),
        _ => frames.push((op, data)),
    }
}

fn holds(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len().max(1)).any(|w| w == needle)
}

fn serve(mut c: TcpStream, steps: &[Step]) -> Log {
    c.set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a read timeout");
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match c.read(&mut b) {
            Ok(1) => head.push(b[0]),
            _ => break,
        }
    }
    let head: Vec<String> = String::from_utf8_lossy(&head)
        .split("\r\n")
        .filter(|l| !l.is_empty())
        .map(|l| match l.strip_prefix("Sec-WebSocket-Key: ") {
            Some(key) => {
                assert_eq!(key.len(), 24, "a 16-byte key in base64: {key}");
                assert!(key.ends_with("=="), "a 16-byte key in base64: {key}");
                "Sec-WebSocket-Key: <key>".to_string()
            }
            None => l.to_string(),
        })
        .collect();
    let mut frames = Vec::new();
    let mut bin = Vec::new();
    for step in steps {
        match step {
            Step::Raw(bytes) => {
                if c.write_all(bytes).is_err() {
                    break;
                }
            }
            Step::Hangup => return Log { head, frames },
            Step::Binary(want) => {
                while !holds(&bin, want) {
                    let Some((op, data)) = read_frame(&mut c) else {
                        panic!("the client ended before sending {want:?}; it sent {frames:?}");
                    };
                    if op == 2 {
                        bin.extend_from_slice(&data);
                    }
                    push(&mut frames, op, data);
                }
            }
            Step::Text(want) => {
                loop {
                    let Some((op, data)) = read_frame(&mut c) else {
                        panic!("the client ended before a text frame with {want:?}; it sent {frames:?}");
                    };
                    let hit = op == 1 && holds(&data, want.as_bytes());
                    if op == 2 {
                        bin.extend_from_slice(&data);
                    }
                    push(&mut frames, op, data);
                    if hit {
                        break;
                    }
                }
            }
            Step::Pong => loop {
                let Some((op, data)) = read_frame(&mut c) else {
                    panic!("the client ended before a pong; it sent {frames:?}");
                };
                push(&mut frames, op, data);
                if op == 10 {
                    break;
                }
            },
        }
    }
    // The rest, until the client closes the connection.
    c.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read timeout");
    while let Some((op, data)) = read_frame(&mut c) {
        push(&mut frames, op, data);
    }
    Log { head, frames }
}

/// The rig's settings for this stub: the owner token on disk, `PORT` its.
fn on(ws: &WsStub) -> Vec<(&'static str, String)> {
    vec![("K2_PORT", ws.port.to_string())]
}

/// A row against the stub: `code` and stdin as given; no daemon request.
fn row(
    ws: &WsStub,
    name: &'static str,
    args: &'static [&'static str],
    stdin: Option<&'static [u8]>,
    code: i32,
) -> Case {
    Case {
        env: on(ws),
        stdin,
        ..case(name, args, &[], vec![], owner_only, code, 0)
    }
}

/// Runs `c` on every side with `steps` as the server's script; every
/// side's log must be the first's. The logs, for the row's own checks.
fn check(rig: &Rig, ws: &WsStub, c: &Case, steps: Vec<Step>, connects: usize) -> Vec<Log> {
    ws.play(steps);
    rig.check(c);
    let logs = ws.take();
    assert_eq!(
        logs.len(),
        connects,
        "{}: the connections the sides made: {logs:?}",
        c.name
    );
    for l in &logs[1..] {
        assert_eq!(l, &logs[0], "{}: a side's WebSocket log", c.name);
    }
    for l in &logs {
        assert!(
            !l.head[0].contains(OWNER) && !l.head[0].contains(PASSPORT),
            "{}: a credential in the URL: {}",
            c.name,
            l.head[0]
        );
    }
    logs
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn shell_words_and_refusals_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "shellno");
    let ws = WsStub::start();
    // native: app shell
    rig.check(&case(
        "no App: the usage line",
        &["shell"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    ));
    // native: app shell
    rig.check(&case(
        "a flag before the command",
        &["shell", "acme", "-x", "ls"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    ));
    for args in [
        &["shell", "acme", "-h"][..],
        &["shell", "--help", "acme", "--", "ls"],
    ] {
        // native: app shell
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
    assert!(ws.take().is_empty(), "help or a usage line connected");
    let refused = |status: &str, body: &str| {
        let mut b = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        b.extend_from_slice(body.as_bytes());
        vec![Step::Raw(b), Step::Hangup]
    };
    for (name, steps) in [
        (
            "refused: the daemon's error",
            refused(
                "403 Forbidden",
                r#"{"error":{"code":"owner_only","hint":"Only the owner é."}}"#,
            ),
        ),
        (
            "refused: an error that is a string",
            refused(
                "401 Unauthorized",
                r#"{"error":"Invalid or missing auth token"}"#,
            ),
        ),
        ("refused: no body", refused("500 Internal Server Error", "")),
        (
            "refused: not JSON",
            refused("502 Bad Gateway", "<html>no</html>"),
        ),
        (
            "refused: no hint",
            refused("404 Not Found", r#"{"error":{"code":"no_such_app"}}"#),
        ),
        (
            "refused: a list",
            refused("400 Bad Request", r#"[{"error":1}]"#),
        ),
        ("closed with nothing", vec![Step::Hangup]),
    ] {
        // native: app shell
        let logs = check(
            &rig,
            &ws,
            &row(&ws, name, &["shell", "acme"], None, 3),
            steps,
            3,
        );
        assert_eq!(
            logs[0].head,
            vec![
                "GET /cli/app/shell?app=acme&cols=80&rows=24 HTTP/1.1".to_string(),
                "Host: 127.0.0.1".into(),
                "Upgrade: websocket".into(),
                "Connection: Upgrade".into(),
                "Sec-WebSocket-Key: <key>".into(),
                "Sec-WebSocket-Version: 13".into(),
                format!("Authorization: Bearer {OWNER}"),
            ],
            "{name}"
        );
    }
    // native: app shell
    rig.check(&traceback(Case {
        env: vec![("K2_PORT", dead_port())],
        ..case(
            "no daemon",
            &["shell", "acme"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        )
    }));
}

#[test]
fn a_session_with_stdin_not_a_terminal() {
    let rig = Rig::new(VERB, "shellrun");
    let ws = WsStub::start();
    let exit = |code: &str| text(&format!(r#"{{"type":"exit","code":{code}}}"#));
    let eot = Step::Binary(b"\x04");
    let session = |middle: Vec<Step>| {
        let mut s = vec![Step::Raw(UPGRADE.to_vec())];
        s.extend(middle);
        s
    };
    let big: &'static [u8] = Box::leak(vec![b'x'; 70_000].into_boxed_slice());
    let mid: &'static [u8] = Box::leak(vec![b'y'; 300].into_boxed_slice());
    let rows: Vec<(Case, Vec<Step>)> = vec![
        (
            row(
                &ws,
                "a command: its output, then its exit code",
                &["shell", "acme", "--", "ls", "-l", "é"],
                None,
                5,
            ),
            session(vec![
                text(r#"{"type":"banner","text":"acme é: a shell"}"#),
                binary(b"line1\nline2 \xc3\xa9\n"),
                eot.clone(),
                exit("5"),
                close(),
            ]),
        ),
        (
            row(
                &ws,
                "stdin sent, then its end",
                &["shell", "acme", "cat"],
                Some(b"echo hi\nbye"),
                0,
            ),
            session(vec![
                Step::Binary(b"echo hi\nbye\x04"),
                binary(b"hi\n"),
                exit("0"),
                close(),
            ]),
        ),
        (
            row(
                &ws,
                "an error, then the server closes",
                &["shell", "acme"],
                None,
                3,
            ),
            session(vec![
                eot.clone(),
                text(r#"{"type":"error","code":"no_such_app","hint":"gone é"}"#),
                close(),
            ]),
        ),
        (
            row(&ws, "an error, then exit 0", &["shell", "acme"], None, 0),
            session(vec![
                eot.clone(),
                text(r#"{"type":"error","code":null}"#),
                exit("0"),
                close(),
            ]),
        ),
        (
            row(
                &ws,
                "an exit code out of range",
                &["shell", "acme"],
                None,
                1,
            ),
            session(vec![eot.clone(), exit("300"), close()]),
        ),
        (
            row(
                &ws,
                "an exit code that is true",
                &["shell", "acme"],
                None,
                1,
            ),
            session(vec![eot.clone(), exit("true"), close()]),
        ),
        (
            row(
                &ws,
                "an exit code that is a string",
                &["shell", "acme"],
                None,
                1,
            ),
            session(vec![eot.clone(), exit("\"2\""), close()]),
        ),
        (
            row(
                &ws,
                "a ping, its pong; a notice",
                &["shell", "acme"],
                None,
                0,
            ),
            session(vec![
                eot.clone(),
                Step::Raw(frame(9, b"p1")),
                Step::Pong,
                text(r#"{"type":"notice"}"#),
                close(),
            ]),
        ),
        (
            row(
                &ws,
                "frames that are no JSON, or not UTF-8, or no message",
                &["shell", "acme"],
                None,
                0,
            ),
            session(vec![
                eot.clone(),
                text("not json"),
                Step::Raw(frame(1, b"\xff\xfe")),
                text(r#"{"type":"other"}"#),
                Step::Raw(frame(0, b"continued")),
                close(),
            ]),
        ),
        (
            row(
                &ws,
                "long frames: 16-bit and 64-bit lengths",
                &["shell", "acme"],
                None,
                0,
            ),
            session(vec![binary(mid), binary(big), eot.clone(), close()]),
        ),
        (
            row(
                &ws,
                "the server hangs up, no close",
                &["shell", "acme"],
                None,
                0,
            ),
            session(vec![eot.clone(), binary(b"bye\n"), Step::Hangup]),
        ),
        (
            row(
                &ws,
                "frames in the handshake's own packet",
                &["shell", "acme"],
                None,
                7,
            ),
            vec![
                Step::Raw(
                    [
                        UPGRADE.to_vec(),
                        frame(1, br#"{"type":"banner","text":"early"}"#),
                        frame(2, b"out\n"),
                    ]
                    .concat(),
                ),
                eot.clone(),
                exit("7"),
                close(),
            ],
        ),
        (
            Case {
                env: vec![
                    ("K2_PORT", ws.port.to_string()),
                    ("K2_HOOK_TOKEN", PASSPORT.to_string()),
                    ("K2_SECRETS_DIR", "{secrets}".to_string()),
                ],
                setup: nothing,
                ..row(
                    &ws,
                    "the passport is the credential",
                    &["shell", "acme"],
                    None,
                    0,
                )
            },
            session(vec![eot.clone(), close()]),
        ),
        (
            traceback(row(
                &ws,
                "a text frame that is JSON but no object",
                &["shell", "acme"],
                None,
                1,
            )),
            session(vec![eot.clone(), text("[1]"), close()]),
        ),
        (
            traceback(row(
                &ws,
                "a banner whose text is a number",
                &["shell", "acme"],
                None,
                1,
            )),
            session(vec![
                eot.clone(),
                text(r#"{"type":"banner","text":5}"#),
                close(),
            ]),
        ),
    ];
    for (c, steps) in rows {
        // native: app shell
        let logs = check(&rig, &ws, &c, steps, 3);
        let last = logs[0].frames.last().expect("frames");
        if c.name != "the server hangs up, no close" {
            assert_eq!(last, &(8u8, Vec::new()), "{}: the close frame last", c.name);
        }
        if c.name == "the passport is the credential" {
            assert!(logs[0]
                .head
                .contains(&format!("Authorization: Bearer {PASSPORT}")));
        }
        if c.name == "a command: its output, then its exit code" {
            assert_eq!(
                logs[0].head[0],
                "GET /cli/app/shell?app=acme&cols=80&rows=24&argv=%5B%22ls%22%2C+%22-l%22%2C+%22%5Cu00e9%22%5D HTTP/1.1"
            );
        }
        if c.name == "a ping, its pong; a notice" {
            assert!(logs[0].frames.contains(&(10, b"p1".to_vec())));
        }
    }
}

/// `tests/cli/clone_security.sh`'s pty driver, with two more steps: the
/// terminal's raw state (`probe`) and a window resize (`resize`). The
/// command runs in a new session whose controlling terminal is the pty
/// (stdin and stderr); stdout goes to `W/out`, what the terminal showed
/// to `W/tty`; `W/raw` holds each probe's flags, `W/after` whether the
/// terminal is as it was before the command ran.
const PTYRUN: &str = r#"
import fcntl, json, os, select, signal, struct, subprocess, sys, termios, time
steps, W = json.loads(sys.argv[1]), sys.argv[2]
cmd = sys.argv[sys.argv.index("--") + 1:]
m, s = os.openpty()
fcntl.ioctl(s, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
before = termios.tcgetattr(m)
def pre():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)
with open(os.path.join(W, "out"), "wb") as out:
    p = subprocess.Popen(cmd, stdin=s, stdout=out, stderr=s, preexec_fn=pre)
def stop():
    try:
        os.killpg(p.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
seen = b""
def pump(t):
    global seen
    r, _, _ = select.select([m], [], [], t)
    if r:
        try:
            seen += os.read(m, 65536)
        except OSError:
            pass
raw, flags = [], []
for step in steps:
    if step[0] == "wait":
        want = step[1]
        end = time.time() + 60
        while want.encode() not in seen:
            if time.time() > end or p.poll() is not None:
                pump(0.2)
                if want.encode() in seen:
                    break
                open(os.path.join(W, "tty"), "wb").write(seen)
                stop()
                sys.exit("ptyrun: never saw %r on the terminal" % want)
            pump(0.1)
        seen = seen.replace(want.encode(), b"<<" + want.encode() + b">>", 1)
    elif step[0] == "type":
        os.write(m, step[1].encode())
    elif step[0] == "probe":
        a = termios.tcgetattr(m)
        raw.append("echo=%s icanon=%s isig=%s" % tuple(
            "on" if a[3] & f else "off" for f in (termios.ECHO, termios.ICANON, termios.ISIG)))
        flags.append("iflag=%x oflag=%x cflag=%x lflag=%x vmin=%r vtime=%r" % (
            a[0], a[1], a[2], a[3], a[6][termios.VMIN], a[6][termios.VTIME]))
    elif step[0] == "resize":
        fcntl.ioctl(m, termios.TIOCSWINSZ, struct.pack("HHHH", step[1], step[2], 0, 0))
end = time.time() + 120
while p.poll() is None:
    if time.time() > end:
        stop()
        sys.exit("ptyrun: the command never finished")
    pump(0.1)
pump(0.2)
open(os.path.join(W, "tty"), "wb").write(seen)
open(os.path.join(W, "raw"), "w").write("\n".join(raw))
open(os.path.join(W, "flags"), "w").write("\n".join(flags))
after = termios.tcgetattr(m)
open(os.path.join(W, "after"), "w").write("as before" if after[:6] == before[:6] else "changed: %r -> %r" % (before[:6], after[:6]))
sys.exit(p.returncode if p.returncode >= 0 else 128 - p.returncode)
"#;

/// What a run at the terminal left.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AtTty {
    code: Option<i32>,
    out: Vec<u8>,
    tty: String,
    raw: String,
    /// The whole flag set at the probe.
    flags: String,
    after: String,
}

fn pty_run(rig: &Rig, ws: &WsStub, bash: bool, steps: &str, args: &[&str]) -> AtTty {
    let driver = rig.dir.join("ptyrun.py");
    std::fs::write(&driver, PTYRUN).expect("write the pty driver");
    let w = rig.dir.join(if bash { "pty-bash" } else { "pty-rust" });
    let _ = std::fs::remove_dir_all(&w);
    std::fs::create_dir_all(&w).expect("the pty folder");
    let tmp = rig.dir.join(if bash { "tmp-bash" } else { "tmp-rust" });
    let mut cmd = std::process::Command::new("python3");
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("HOME", &rig.home)
        .env("TMPDIR", &tmp)
        .env("K2_PORT", ws.port.to_string())
        .current_dir(&rig.proj)
        .arg(&driver)
        .arg(steps)
        .arg(&w)
        .arg("--");
    if bash {
        cmd.env("K5_CLI_VIA", "rust").arg("bash").arg(cli_k2());
    } else {
        cmd.arg(rig.bin);
    }
    cmd.arg(VERB).args(args);
    let o = run_within(&mut cmd, None, Duration::from_secs(150), "the pty driver");
    let read = |f: &str| {
        std::fs::read(w.join(f))
            .unwrap_or_else(|e| panic!("{f}: {e}; driver: {}", String::from_utf8_lossy(&o.stderr)))
    };
    AtTty {
        code: o.status.code(),
        out: read("out"),
        tty: String::from_utf8_lossy(&read("tty")).into_owned(),
        raw: String::from_utf8_lossy(&read("raw")).into_owned(),
        flags: String::from_utf8_lossy(&read("flags")).into_owned(),
        after: String::from_utf8_lossy(&read("after")).into_owned(),
    }
}

#[test]
fn at_a_terminal() {
    let rig = Rig::new(VERB, "shelltty");
    rig.owner_on_disk();
    let ws = WsStub::start();
    let steps = r#"[["wait","welcome"],["probe"],["type","ls\r"],["wait","ran"],["resize",40,120],["wait","sized"]]"#;
    let script = vec![
        Step::Raw(UPGRADE.to_vec()),
        text(r#"{"type":"banner","text":"welcome"}"#),
        Step::Binary(b"ls\r"),
        binary(b"file1\r\n"),
        text(r#"{"type":"notice","text":"ran"}"#),
        Step::Text("resize"),
        text(r#"{"type":"notice","text":"sized"}"#),
        text(r#"{"type":"exit","code":7}"#),
        close(),
    ];
    let mut runs = Vec::new();
    for bash in [true, false] {
        ws.play(script.clone());
        let run = pty_run(&rig, &ws, bash, steps, &["shell", "acme"]);
        let logs = ws.take();
        assert_eq!(logs.len(), 1, "one connection: {logs:?}");
        runs.push((run, logs.into_iter().next().expect("its log")));
    }
    let (bash, rust) = (&runs[0], &runs[1]);
    assert_eq!(rust.1, bash.1, "the WebSocket log");
    // `scripts/cli-parity.sh`'s named difference shell-raw-py311: before
    // Python 3.12, `tty.setraw` clears fewer flags than the binary (which
    // sets 3.12's); the whole set is compared where python3 is 3.12 or
    // newer, and what raw mode turns off everywhere else.
    if python3_version() >= (3, 12) {
        assert_eq!(rust.0, bash.0, "the terminal");
    } else {
        let b = AtTty {
            flags: String::new(),
            ..bash.0.clone()
        };
        let r = AtTty {
            flags: String::new(),
            ..rust.0.clone()
        };
        assert_eq!(r, b, "the terminal (but the flag set)");
    }
    // What the shell does, checked too, so a change on both sides shows.
    let (run, log) = bash;
    assert_eq!(run.code, Some(7), "{run:?}");
    assert_eq!(run.out, b"file1\r\n");
    assert_eq!(
        run.raw, "echo=off icanon=off isig=off",
        "raw mode while it runs"
    );
    assert_eq!(run.after, "as before", "the terminal put back");
    assert!(
        !run.tty.contains("ls"),
        "what was typed was echoed: {:?}",
        run.tty
    );
    assert_eq!(
        log.head[0],
        "GET /cli/app/shell?app=acme&cols=100&rows=30 HTTP/1.1"
    );
    assert_eq!(
        log.frames,
        vec![
            (2, b"ls\r".to_vec()),
            (
                1,
                br#"{"type": "resize", "cols": 120, "rows": 40}"#.to_vec()
            ),
            (8, Vec::new()),
        ]
    );
}

/// The usage lines say the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn shell_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "shellbrand");
    let cli = k2_client::brand::CLI_NAME;
    let c = case("brand", &["shell"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: {cli} app shell <app-id> [-- <command\u{2026}>]\n")
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
}
