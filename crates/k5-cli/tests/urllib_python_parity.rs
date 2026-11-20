//! The urllib adapter (`k5_cli::transport::urllib`) against Python 3's own
//! `urllib.request.urlopen`: for each row, python3 and the adapter make
//! the same request to one logging stub daemon ([`common::stub`]), which
//! answers both the same way. The raw requests must be the same bytes, but
//! for the `User-Agent` line (`Python-urllib/3.x` there, ours here, each
//! counted once), and what each got must be the same: the status and the
//! body, or the exception's class and `str(e)`, and the kind the shell's
//! `_is_timeout` and `_restart_kind` (`cli/src/calendar.sh`, `mail.sh`)
//! make of it.
//!
//! The hand-over headers are given to both as one plain header, after the
//! caller's, so their place is compared too.

#![cfg(unix)]

mod common;

use common::stub::{dead_port, Reply, Stub};
use k5_cli::transport::client::{Auth, Client};
use k5_cli::transport::urllib::{Kind, Method, Opener, Outcome, Request};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// The Python side: one `urlopen` as the shell's blocks make it, and one
/// line saying what came of it.
const PYTHON: &str = r#"
import json, socket, sys, urllib.error, urllib.request
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))
spec = json.loads(sys.stdin.read())

def _is_timeout(exc):
    cur = exc
    for _ in range(4):
        if isinstance(cur, (TimeoutError, socket.timeout)):
            return True
        nxt = getattr(cur, "reason", None)
        if nxt is None or nxt is cur:
            break
        cur = nxt
    return False

def _restart_kind(exc):
    import http.client
    cur = exc
    for _ in range(4):
        if isinstance(cur, ConnectionRefusedError):
            return "refused"
        if isinstance(cur, (ConnectionResetError, BrokenPipeError, http.client.IncompleteRead)):
            return "dropped"
        nxt = getattr(cur, "reason", None)
        if nxt is None or nxt is cur:
            break
        cur = nxt
    return None

def raised(e):
    m, n = type(e).__module__, type(e).__qualname__
    kind = "timeout" if _is_timeout(e) else (_restart_kind(e) or "other")
    return "raised %s: %s [%s]" % (n if m == "builtins" else m + "." + n, e, kind)

def body(f):
    try:
        return "body " + f.read().hex()
    except Exception as e:
        return raised(e)

data = bytes.fromhex(spec["data"]) if spec["data"] is not None else None
req = urllib.request.Request(spec["url"], data=data, method=spec["method"],
    headers=dict(spec["init"]))
for name, value in spec["add"]:
    req.add_header(name, value)
try:
    with urllib.request.urlopen(req, timeout=spec["timeout"]) as r:
        out = "answer %d %s" % (r.status, body(r))
except urllib.error.HTTPError as e:
    out = "http_error %d %s" % (e.code, body(e))
except Exception as e:
    out = raised(e)
print(out)
"#;

/// One row: the request, and how the stub answers it.
struct Row {
    name: &'static str,
    method: Method,
    query: &'static [(&'static str, &'static str)],
    data: Option<&'static [u8]>,
    /// Headers given to `Request(..., headers=)`.
    init: &'static [(&'static str, &'static str)],
    /// Headers added after, with `add_header`.
    add: &'static [(&'static str, &'static str)],
    timeout_ms: u64,
    reply: Option<Reply>,
}

const AUTH: (&str, &str) = ("Authorization", "Bearer owner-tok-7171");
const JSON: (&str, &str) = ("Content-Type", "application/json");
const VIA: (&str, &str) = ("X-K5-Cli-Via", "fallthrough");

fn raw(bytes: &[u8]) -> Option<Reply> {
    Some(Reply::Raw(bytes.to_vec()))
}

fn rows() -> Vec<Row> {
    let get = |name, reply| Row {
        name,
        method: Method::Get,
        query: &[("project", "/Users/a b/w"), ("q", "x&y=é~")],
        data: None,
        init: &[],
        add: &[AUTH],
        timeout_ms: 5000,
        reply,
    };
    vec![
        get("an answer", Some(Reply::Answer(200, r#"{"ok":true,"n":"é"}"#.into()))),
        get("an empty answer", Some(Reply::Answer(200, String::new()))),
        get("a refusal", Some(Reply::Answer(403, r#"{"error":"no"}"#.into()))),
        get(
            "migrating",
            Some(Reply::Answer(503, r#"{"ok":false,"state":"migrating"}"#.into())),
        ),
        get("no content", raw(b"HTTP/1.1 204 No Content\r\n\r\n")),
        get("closed with nothing", Some(Reply::Close)),
        get("a reset", Some(Reply::Reset)),
        get("not HTTP", raw(b"garbage\r\n\r\n")),
        get(
            "cut short",
            raw(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"),
        ),
        get(
            "an error cut short",
            raw(b"HTTP/1.1 502 X\r\nContent-Length: 10\r\n\r\nabc"),
        ),
        get(
            "chunked",
            raw(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\nT: 1\r\n\r\n"),
        ),
        get(
            "chunked, broken",
            raw(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n9\r\nde"),
        ),
        get(
            "chunked, no size",
            raw(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"),
        ),
        get(
            "no length: to the end",
            raw(b"HTTP/1.1 200 OK\r\nX: y\r\n\r\nall of it"),
        ),
        get(
            "continue, then the answer",
            raw(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}"),
        ),
        get("HTTP/1.0, no reason", raw(b"HTTP/1.0 200\r\nContent-Length: 2\r\n\r\nok")),
        get("a status too low", raw(b"HTTP/1.1 42 X\r\n\r\n")),
        get("bad bytes", raw(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n\xff\xfe!")),
        Row {
            timeout_ms: 400,
            ..get("no answer in time", Some(Reply::Hang(Duration::from_secs(2))))
        },
        Row {
            name: "a JSON POST",
            method: Method::Post,
            query: &[],
            data: Some(r#"{"project":"/w","name":"é"}"#.as_bytes()),
            init: &[JSON],
            add: &[AUTH],
            timeout_ms: 5000,
            reply: Some(Reply::Answer(200, r#"{"ok":true}"#.into())),
        },
        Row {
            name: "a POST with an empty body and no type",
            method: Method::Post,
            query: &[("id", "7")],
            data: Some(b""),
            init: &[AUTH],
            add: &[],
            timeout_ms: 5000,
            reply: Some(Reply::Answer(200, r#"{"ok":true}"#.into())),
        },
        Row {
            name: "a GET with a type and no credential",
            method: Method::Get,
            query: &[],
            data: None,
            init: &[JSON],
            add: &[],
            timeout_ms: 5000,
            reply: Some(Reply::Answer(200, r#"{"ok":true}"#.into())),
        },
    ]
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A JSON string, for the spec on python3's stdin.
fn js(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn pairs(p: &[(&str, &str)]) -> String {
    let items: Vec<String> = p
        .iter()
        .map(|(n, v)| format!("[{},{}]", js(n), js(v)))
        .collect();
    format!("[{}]", items.join(","))
}

fn python(row: &Row, port: u16, scratch: &std::path::Path) -> String {
    let url = format!(
        "http://127.0.0.1:{port}/cli/x{}",
        if row.query.is_empty() {
            String::new()
        } else {
            format!("?{}", k5_cli::transport::urllib::urlencode(row.query))
        }
    );
    let spec = format!(
        r#"{{"url":{},"method":{},"data":{},"init":{},"add":{},"timeout":{}}}"#,
        js(&url),
        js(row.method.as_str()),
        row.data.map_or("null".to_string(), |d| js(&hex(d))),
        pairs(row.init),
        pairs(&[row.add, &[VIA]].concat()),
        row.timeout_ms as f64 / 1000.0,
    );
    let mut child = Command::new("python3")
        .args(["-I", "-c", PYTHON])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("HOME", scratch)
        .current_dir(scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(spec.as_bytes())
        .expect("write the spec");
    let out = child.wait_with_output().expect("python3's output");
    assert!(
        out.status.success(),
        "{}: python3 failed: {}",
        row.name,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .strip_suffix('\n')
        .expect("python3 ends its line")
        .to_string()
}

/// The python3 the rows run: older than 3.10? There `socket.timeout` is
/// its own class, so a timeout prints as `socket.timeout`; from 3.10 on
/// it is `TimeoutError`, which the adapter always prints (the coordinator's
/// ruling: the binary has no Python version to copy).
fn python_before_310(scratch: &std::path::Path) -> bool {
    let out = Command::new("python3")
        .args([
            "-I",
            "-c",
            "import sys; print('%d %d' % sys.version_info[:2])",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("HOME", scratch)
        .current_dir(scratch)
        .output()
        .expect("run python3 for its version");
    assert!(out.status.success(), "python3's version: {out:?}");
    let text = String::from_utf8(out.stdout).expect("utf-8");
    let nums: Vec<u32> = text
        .split_whitespace()
        .map(|n| n.parse().expect("a version number"))
        .collect();
    assert_eq!(nums.len(), 2, "python3's version: {text:?}");
    (nums[0], nums[1]) < (3, 10)
}

/// Python's line and the adapter's agree: the same, or, on a python3
/// older than 3.10, the same but for `socket.timeout` where the adapter
/// says `TimeoutError`.
fn same_outcome(py: &str, rs: &str, before_310: bool) -> bool {
    if py == rs {
        return true;
    }
    before_310
        && rs.contains("raised TimeoutError: ")
        && py == rs.replacen("raised TimeoutError: ", "raised socket.timeout: ", 1)
}

fn kind(k: Kind) -> &'static str {
    match k {
        Kind::Timeout => "timeout",
        Kind::Refused => "refused",
        Kind::Dropped => "dropped",
        Kind::Other => "other",
    }
}

fn ours(row: &Row, port: u16) -> String {
    let mut o = Opener::from_client(&Client::new("127.0.0.1", port, "/p", Auth::None));
    o.user_agent = "<ua>".into();
    o.handover = vec![(VIA.0.into(), VIA.1.into())];
    let mut req = Request::new(row.method, "/cli/x", Duration::from_millis(row.timeout_ms))
        .query(row.query.iter().copied());
    if let Some(d) = row.data {
        req = req.data(d.to_vec());
    }
    for (n, v) in row.init.iter().chain(row.add) {
        req = req.header(*n, *v);
    }
    let body = |b: &Result<Vec<u8>, k5_cli::transport::urllib::Raised>| match b {
        Ok(bytes) => format!("body {}", hex(bytes)),
        Err(r) => format!("raised {} [{}]", r.traceback_line(), kind(r.kind)),
    };
    match o.open(&req) {
        Outcome::Answer { status, body: b } => format!("answer {status} {}", body(&b)),
        Outcome::HttpError { code, body: b } => format!("http_error {code} {}", body(&b)),
        Outcome::Raised(r) => format!("raised {} [{}]", r.traceback_line(), kind(r.kind)),
    }
}

/// A request's text with its `User-Agent` line masked (exactly one).
fn masked(bytes: &[u8], name: &str) -> String {
    let text = String::from_utf8(bytes.to_vec()).expect("a UTF-8 request");
    let mut n = 0;
    let lines: Vec<String> = text
        .split("\r\n")
        .map(|l| {
            if l.starts_with("User-Agent: ") {
                n += 1;
                "User-Agent: <ua>".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    assert_eq!(n, 1, "{name}: User-Agent lines in {text:?}");
    lines.join("\r\n")
}

#[test]
fn the_adapter_is_python_s_urlopen() {
    let scratch = std::env::temp_dir().join(format!("k5-urllib-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch");
    let stub = Stub::start(&scratch.join("s.sock"));
    let before_310 = python_before_310(&scratch);
    let mut failures = Vec::new();
    for row in rows() {
        let replies: Vec<Reply> = row.reply.iter().cloned().collect();
        stub.reset(&replies);
        let py = python(&row, stub.port, &scratch);
        let py_log = stub.take_log();
        stub.reset(&replies);
        let rs = ours(&row, stub.port);
        let rs_log = stub.take_log();
        assert_eq!(
            py_log.len(),
            1,
            "{}: python3 sent {}",
            row.name,
            py_log.len()
        );
        assert_eq!(
            rs_log.len(),
            1,
            "{}: the adapter sent {}",
            row.name,
            rs_log.len()
        );
        let (a, b) = (
            masked(&py_log[0].bytes, row.name),
            masked(&rs_log[0].bytes, row.name),
        );
        if a != b {
            failures.push(format!(
                "{}: requests differ\n  python: {a:?}\n  ours:   {b:?}",
                row.name
            ));
        }
        if !same_outcome(&py, &rs, before_310) {
            failures.push(format!(
                "{}: outcomes differ\n  python: {py}\n  ours:   {rs}",
                row.name
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_refused_connect_is_python_s() {
    let scratch = std::env::temp_dir().join(format!("k5-urllib-dead-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch");
    let port = dead_port();
    let row = Row {
        name: "refused",
        method: Method::Get,
        query: &[],
        data: None,
        init: &[],
        add: &[AUTH],
        timeout_ms: 2000,
        reply: None,
    };
    assert_eq!(python(&row, port, &scratch), ours(&row, port));
}

#[test]
fn only_an_old_python_may_name_the_timeout_socket_timeout() {
    let ours = "raised TimeoutError: timed out [timeout]";
    let old = "raised socket.timeout: timed out [timeout]";
    assert!(same_outcome(ours, ours, false));
    assert!(same_outcome(old, ours, true));
    assert!(!same_outcome(old, ours, false));
    assert!(!same_outcome(
        "raised socket.timeout: timed out [other]",
        ours,
        true
    ));
    assert!(!same_outcome(
        "raised socket.timeout: Connection refused [refused]",
        "raised ConnectionRefusedError: Connection refused [refused]",
        true
    ));
}
