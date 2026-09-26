//! GET `/cli/chat/transcript` is a websocket, not a POST route, and a
//! missing provider conversation id does not open a tail.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn post_is_405_bad_token_and_skin_are_403_missing_conversation_is_400() {
    let daemon = k2_daemon::test_harness::start("owner-token-chat-overlay").await;
    let port = daemon.port;
    let path = k2_daemon::chat_overlay_ws::CHAT_TRANSCRIPT_WS_PATH;

    let post = http(
        port,
        "POST",
        &format!("{path}?token=owner-token-chat-overlay&provider=claude&conversation=abc"),
    )
    .await;
    assert!(post.starts_with("HTTP/1.1 405"), "{post}");

    let bad = http(
        port,
        "GET",
        &format!("{path}?token=nope&provider=claude&conversation=abc"),
    )
    .await;
    assert!(bad.starts_with("HTTP/1.1 403"), "{bad}");
    assert!(!bad.contains("101 Switching"), "{bad}");

    let skin = http(
        port,
        "GET",
        &format!("{path}?token=k2skn_not-a-pass&provider=claude&conversation=abc"),
    )
    .await;
    assert!(skin.starts_with("HTTP/1.1 403"), "{skin}");
    assert!(!skin.contains("101 Switching"), "{skin}");

    let missing = http(
        port,
        "GET",
        &format!("{path}?token=owner-token-chat-overlay&provider=claude"),
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 400"), "{missing}");
    assert!(missing.contains("conversation required"), "{missing}");
    assert!(!missing.contains("101 Switching"), "{missing}");
}

async fn http(port: u16, method: &str, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    String::from_utf8_lossy(&buf).into_owned()
}
