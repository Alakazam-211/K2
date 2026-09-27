//! POST /cli/agents/ensure-cli is POST-only.
//!
//! GET must be 405 `{"error":"POST required"}` from the exact dispatcher
//! arm (`require_post`, same shape as `/cli/agents/archive-orphans`), not
//! a 404 from the `/cli/` catchall. The read-chain twin in
//! `agents_routes::dispatch` is what `cli::dispatch` hits.

#![cfg(unix)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::time::Duration;

use k2_daemon::test_harness;

const OWNER_TOKEN: &str = "owner-token-ensure-cli-route";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = format!(
        "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
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
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = match text.split_once("\r\n\r\n") {
        Some((_h, b)) => b.to_string(),
        None => String::new(),
    };
    Resp { status, body }
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
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let complete = match content_len {
        Some(clen) => body.len() >= clen,
        None => true,
    };
    Some((status, body.to_string(), complete))
}

#[test]
fn ensure_cli_arm_is_exact_post_only() {
    let src = include_str!("../src/routes/dispatcher.rs");
    let start = src
        .find("let post_allowed = matches!")
        .expect("post_allowed");
    let end_rel = src[start..]
        .find("if method != \"GET\"")
        .expect("method gate");
    let block = &src[start..start + end_rel];
    assert!(
        block.contains("\"/cli/agents/ensure-cli\""),
        "ensure-cli must be on post_allowed or POST is rejected before the arm"
    );
    let marker = "\"/cli/agents/ensure-cli\" =>";
    let arm_at = src.find(marker).expect("exact ensure-cli arm");
    let window = &src[arm_at..arm_at + 280];
    assert!(
        window.contains("require_post"),
        "exact arm must call require_post: {window}"
    );
    assert!(
        !window.contains("is_post &&"),
        "arm must be exact, not `is_post &&`: {window}"
    );
}

#[test]
fn ensure_cli_get_dispatch_is_405() {
    let resp = k2_daemon::cli::dispatch("/cli/agents/ensure-cli", &HashMap::new());
    assert_eq!(resp.status, "405 Method Not Allowed");
    assert_eq!(resp.body, r#"{"error":"POST required"}"#);
    assert!(
        !resp.body.contains("route not found"),
        "GET must not 404: {}",
        resp.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_cli_get_http_is_405() {
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let resp = http(
        daemon.port,
        "GET",
        &format!("/cli/agents/ensure-cli?token={OWNER_TOKEN}"),
    );
    assert_eq!(
        resp.status, 405,
        "GET /cli/agents/ensure-cli must be 405; body={}",
        resp.body
    );
    assert!(
        resp.body.contains("\"error\":\"POST required\""),
        "body={}",
        resp.body
    );
}
