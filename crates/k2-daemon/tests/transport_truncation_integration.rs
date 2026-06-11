//! 0.39.45 (GH #35/#37/#29) — transport-truncation regression tests.
//!
//! The bug class: the dispatcher parsed the ENTIRE query string from a
//! single 4KB `peek`, and the CLI sent inbox bodies / live-message text
//! URL-encoded on the query string (`curl -sG`). Anything past the peek
//! window was SILENTLY dropped — `success: true` while the durable inbox
//! record lost its tail at ~2.7KB (#35: files clipped within 14 bytes of
//! each other; #37: ~54-line clip).
//!
//! The fix, asserted here end-to-end through the REAL dispatcher
//! (in-process ephemeral listener via `k2_daemon::test_harness`):
//!   1. `/cli/inbox/compose` accepts a form-encoded POST body; long
//!      payloads round-trip byte-exact (no cap).
//!   2. Legacy query-string senders keep working and no longer clip at
//!      the old 4KB peek (the head buffer is 16KB and the peek loops
//!      until the header terminator is visible).
//!   3. A request head that exceeds 16KB is refused LOUDLY with `414`
//!      instead of being silently truncated mid-parameter.
//!
//! Tests fail loudly: no fallback defaults in assertions, no
//! skip-if-missing.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;

use k2_daemon::test_harness;

/// Serialize tests: `$HOME`, the shared in-memory DB, and boot-status
/// are process-wide singletons.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Resp {
    status: u16,
    body: String,
}

/// Fire one raw HTTP request (full request text supplied by the caller)
/// and parse the status + body.
fn http_raw(port: u16, req: &str) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set read timeout");
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some((status, body, complete)) = try_parse(&raw) {
            if complete {
                return Resp { status, body };
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    match try_parse(&raw) {
        Some((status, body, _)) => Resp { status, body },
        None => panic!(
            "no parseable HTTP response, got: {:?}",
            String::from_utf8_lossy(&raw)
        ),
    }
}

fn try_parse(raw: &[u8]) -> Option<(u16, String, bool)> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_len = headers.lines().find_map(|l| {
        let lower = l.to_ascii_lowercase();
        lower
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

/// Percent-encode every non-alphanumeric byte — mirrors the CLI's
/// `urlencode` (python `quote(..., safe='')`).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Fresh tempdir to act as the target workspace; returns its path.
fn temp_workspace(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "k2so-trunc-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

/// Read the single inbox item written under `<ws>/.k2so/inbox/` and
/// return its full file content. Panics (loudly) when the inbox dir is
/// missing or holds anything but exactly one `.md` file.
fn read_single_inbox_item(ws: &std::path::Path) -> String {
    let inbox = ws.join(".k2so").join("inbox");
    let mut mds: Vec<std::path::PathBuf> = std::fs::read_dir(&inbox)
        .unwrap_or_else(|e| panic!("inbox dir missing at {}: {e}", inbox.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
        .collect();
    assert_eq!(
        mds.len(),
        1,
        "expected exactly one inbox .md, found {:?}",
        mds
    );
    std::fs::read_to_string(mds.remove(0)).expect("read inbox item")
}

/// #35/#37 regression — a 12KB body sent as a form-encoded POST body
/// round-trips byte-exact into the durable inbox record. This payload is
/// >4× the size that used to be silently clipped.
#[tokio::test(flavor = "multi_thread")]
async fn inbox_compose_post_body_round_trips_long_payload() {
    let _g = lock();
    let daemon = test_harness::start("trunc-owner-1").await;
    let ws = temp_workspace("postbody");

    // 200 lines × ~60 chars ≈ 12KB, with a unique tail marker so a
    // truncation can't accidentally pass.
    let mut body = String::new();
    for i in 0..200 {
        body.push_str(&format!(
            "line {i:03} — the quick brown fox jumps over the lazy daemon\n"
        ));
    }
    body.push_str("FINAL-TAIL-MARKER-7f3a");
    assert!(body.len() > 12_000, "test payload should exceed 12KB");

    let form = format!(
        "title={}&body={}",
        urlencode("long memo"),
        urlencode(&body)
    );
    let req = format!(
        "POST /cli/inbox/compose?token={}&project={} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\n\r\n{}",
        daemon.owner_token,
        urlencode(&ws.to_string_lossy()),
        form.len(),
        form
    );
    let resp = http_raw(daemon.port, &req);
    assert_eq!(resp.status, 200, "compose failed: {}", resp.body);

    let content = read_single_inbox_item(&ws);
    assert!(
        content.contains("FINAL-TAIL-MARKER-7f3a"),
        "inbox record lost its tail — last 120 chars: {:?}",
        &content[content.len().saturating_sub(120)..]
    );
    assert!(
        content.contains("line 199"),
        "inbox record missing final body line"
    );
    // The full body must be embedded verbatim (frontmatter + body + \n).
    assert!(
        content.contains(&body),
        "inbox record does not contain the body verbatim"
    );
}

/// Back-compat — a legacy query-string-only sender (old CLI's
/// `curl -sG -X POST`) with a ~3KB body now round-trips instead of
/// clipping at the old 4KB peek window. This is the EXACT #35 shape:
/// ~2.7KB used to survive, the rest vanished.
#[tokio::test(flavor = "multi_thread")]
async fn inbox_compose_legacy_query_string_no_longer_clips() {
    let _g = lock();
    let daemon = test_harness::start("trunc-owner-2").await;
    let ws = temp_workspace("querystring");

    let mut body = String::new();
    for i in 0..60 {
        body.push_str(&format!("qline {i:02} sphinx of black quartz judge my vow\n"));
    }
    body.push_str("QUERY-TAIL-MARKER-2b9c");
    assert!(
        body.len() > 2_500,
        "payload must exceed the historical ~2.7KB clip point"
    );

    let req = format!(
        "POST /cli/inbox/compose?token={}&project={}&title={}&body={} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Length: 0\r\n\r\n",
        daemon.owner_token,
        urlencode(&ws.to_string_lossy()),
        urlencode("legacy query memo"),
        urlencode(&body)
    );
    let resp = http_raw(daemon.port, &req);
    assert_eq!(resp.status, 200, "compose failed: {}", resp.body);

    let content = read_single_inbox_item(&ws);
    assert!(
        content.contains("QUERY-TAIL-MARKER-2b9c"),
        "legacy query-string body clipped — last 120 chars: {:?}",
        &content[content.len().saturating_sub(120)..]
    );
    assert!(content.contains(&body), "query body not embedded verbatim");
}

/// Oversize guard — a request head past 16KB is refused with an explicit
/// `414`, never silently truncated mid-parameter.
#[tokio::test(flavor = "multi_thread")]
async fn oversized_request_head_returns_414() {
    let _g = lock();
    let daemon = test_harness::start("trunc-owner-3").await;
    let ws = temp_workspace("oversize");

    // ~24KB of urlencoded payload on the URL — way past the 16KB head cap.
    let huge = "x".repeat(24_000);
    let req = format!(
        "POST /cli/inbox/compose?token={}&project={}&title=t&body={} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Length: 0\r\n\r\n",
        daemon.owner_token,
        urlencode(&ws.to_string_lossy()),
        huge
    );
    let resp = http_raw(daemon.port, &req);
    assert_eq!(
        resp.status, 414,
        "expected loud 414 for oversized head, got {}: {}",
        resp.status, resp.body
    );
    // Nothing must have been written: silent partial delivery is the
    // failure mode this guard exists to kill.
    assert!(
        !ws.join(".k2so").join("inbox").exists(),
        "an inbox item was written despite the 414"
    );
}

/// Live-message POST form — `/cli/workspace/msg` accepts the text in the
/// POST body and reaches the real handler (which reports
/// workspace_not_found for an unregistered workspace, proving the params
/// made it through body parsing into the handler).
#[tokio::test(flavor = "multi_thread")]
async fn workspace_msg_post_form_reaches_handler() {
    let _g = lock();
    let daemon = test_harness::start("trunc-owner-4").await;

    let long_text = format!("hello {}", "y".repeat(8_000));
    let form = format!(
        "workspace={}&text={}&from={}",
        urlencode("no-such-workspace-trunc-test"),
        urlencode(&long_text),
        urlencode("tester")
    );
    let req = format!(
        "POST /cli/workspace/msg?token={} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\n\r\n{}",
        daemon.owner_token,
        form.len(),
        form
    );
    let resp = http_raw(daemon.port, &req);
    assert_eq!(resp.status, 200, "msg route errored: {}", resp.body);
    let v: serde_json::Value =
        serde_json::from_str(&resp.body).expect("msg response must be JSON");
    assert_eq!(
        v["success"], false,
        "unregistered workspace must not report success: {}",
        resp.body
    );
    assert_eq!(
        v["reason"], "workspace_not_found",
        "expected workspace_not_found (proves body params reached the \
         handler), got: {}",
        resp.body
    );
}
