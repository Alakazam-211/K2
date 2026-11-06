//! `/cli/chat/transcript` (the Chat view's transcript socket, LIVE 0.41.0)
//! was deleted in 0.46.5 with the Chat view (Rosson 2026-10-27, wiki
//! `Plan - Retirement ledger`). It must answer exactly like a `/cli` route
//! that never existed: every method, with and without a token, with an App
//! pass, and as a WebSocket upgrade. Never a `101 Switching Protocols`.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OWNER: &str = "owner-token-chat-gone";
const GONE: &str = "/cli/chat/transcript";
const NEVER_WAS: &str = "/cli/chat/never-a-route";

#[tokio::test]
async fn the_removed_chat_transcript_route_answers_like_an_unknown_route() {
    let daemon = k2_daemon::test_harness::start(OWNER).await;
    let port = daemon.port;

    let queries = [
        format!("?token={OWNER}&provider=claude&conversation=abc"),
        "?token=nope&provider=claude&conversation=abc".to_string(),
        "?token=k2skn_not-a-pass&provider=claude&conversation=abc".to_string(),
        "?provider=claude&conversation=abc".to_string(),
        format!("?token={OWNER}&provider=claude"),
    ];
    let mut compared = 0;
    for method in ["GET", "POST", "PUT", "DELETE"] {
        for q in &queries {
            for upgrade in [false, true] {
                let gone = http(port, method, &format!("{GONE}{q}"), upgrade).await;
                let never = http(port, method, &format!("{NEVER_WAS}{q}"), upgrade).await;
                assert!(gone.starts_with("HTTP/1.1 "), "{method} {GONE}{q}: no answer: {gone:?}");
                assert!(!gone.contains("101 Switching"), "{method} {GONE}{q} upgraded: {gone}");
                assert!(!gone.contains("conversation required"), "{method} {GONE}{q}: {gone}");
                assert!(!gone.contains("host session logs"), "{method} {GONE}{q}: {gone}");
                assert_eq!(
                    status_type_body(&gone),
                    status_type_body(&never),
                    "{method} {GONE}{q} (upgrade {upgrade}) must answer like {NEVER_WAS}",
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 4 * queries.len() * 2);
}

#[test]
fn the_route_policy_has_no_row_for_it() {
    assert!(
        k2_daemon::routes::route_policy::lookup(GONE).is_none(),
        "{GONE} still has a route-policy row",
    );
    assert!(!k2_daemon::routes::route_policy::post_allowed(GONE));
}

/// Status line, Content-Type and body. Drops headers that change between
/// two requests (`Date:` and the like).
fn status_type_body(raw: &str) -> String {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head.lines().next().unwrap_or("").to_string();
    let content_type = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
        .unwrap_or("")
        .to_string();
    format!("{status}\n{content_type}\n{body}")
}

async fn http(port: u16, method: &str, path: &str, upgrade: bool) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let ws = if upgrade {
        "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
    } else {
        "Connection: close\r\n"
    };
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n{ws}\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    // Read one whole answer: the head, then Content-Length bytes of body
    // (or to EOF). A stalled socket is itself a failure: the caller's
    // assert sees no status line.
    let mut buf = Vec::new();
    let read = async {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(end) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>()))
                    .and_then(Result::ok);
                if let Some(len) = len {
                    if buf.len() >= end + 4 + len {
                        buf.truncate(end + 4 + len);
                        return;
                    }
                }
            }
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), read).await;
    String::from_utf8_lossy(&buf).into_owned()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
