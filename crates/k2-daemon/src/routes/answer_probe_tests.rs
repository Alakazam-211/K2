//! The unread-request probe: what it sees before an answer, and that a
//! request sent again after the answer is never taken for the old one.

use super::answer_probe::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A connected (client, server) pair on loopback.
async fn loopback_pair() -> (TcpStream, TcpStream) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind loopback");
    let addr = listener.local_addr().expect("listener addr");
    let client = TcpStream::connect(addr).await.expect("connect loopback");
    let (server, _) = listener.accept().await.expect("accept loopback");
    (client, server)
}

/// Wait until the server can see `len` bytes (a peek, nothing read).
async fn wait_for_bytes(server: &TcpStream, len: usize) {
    let mut probe = vec![0u8; len];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n = server.peek(&mut probe).await.expect("peek");
        if n >= len {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "only {n} of {len} request bytes arrived");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// Read a request head off the server side: until its blank line, or the
/// end of the stream. One read may return part of it.
async fn read_head(server: &mut TcpStream) -> Vec<u8> {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = server.read(&mut buf).await.expect("read the head");
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    head
}

const GIT_TOOLS_HEAD: &[u8] = b"GET /cli/git-tools?token=t HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
const ANSWER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";

#[tokio::test]
async fn a_head_left_unread_when_the_answer_goes_out_is_seen() {
    scope(async {
        let (mut client, server) = loopback_pair().await;
        client.write_all(GIT_TOOLS_HEAD).await.expect("send request");
        wait_for_bytes(&server, GIT_TOOLS_HEAD.len()).await;
        start(GIT_TOOLS_HEAD);
        before_answer(&server).await;
        assert!(answered_unread(), "the head is still at the front of the socket");
        start(b"");
        assert!(!answered_unread(), "the next request starts with no probe");
    })
    .await;
}

#[tokio::test]
async fn a_consumed_head_is_not_seen_and_the_probe_never_waits() {
    scope(async {
        let (mut client, mut server) = loopback_pair().await;
        client.write_all(GIT_TOOLS_HEAD).await.expect("send request");
        wait_for_bytes(&server, GIT_TOOLS_HEAD.len()).await;
        start(GIT_TOOLS_HEAD);
        let mut buf = [0u8; 1024];
        let n = server.read(&mut buf).await.expect("consume the head");
        assert_eq!(n, GIT_TOOLS_HEAD.len());
        let started = std::time::Instant::now();
        before_answer(&server).await;
        assert!(started.elapsed() < std::time::Duration::from_millis(100), "the probe waited {:?}", started.elapsed());
        assert!(!answered_unread(), "nothing is left to read");
    })
    .await;
}

/// The regression: the arm consumed its request and answered, and the
/// client sent the very same request again on the socket. Those bytes are a
/// new request; the probe, taken before the answer, never sees them.
#[tokio::test]
async fn the_same_request_sent_again_after_the_answer_is_a_new_request() {
    scope(async {
        let (mut client, mut server) = loopback_pair().await;
        client.write_all(GIT_TOOLS_HEAD).await.expect("send request");
        wait_for_bytes(&server, GIT_TOOLS_HEAD.len()).await;
        start(GIT_TOOLS_HEAD);
        assert_eq!(read_head(&mut server).await, GIT_TOOLS_HEAD, "the arm consumes its whole request");
        before_answer(&server).await;
        server.write_all(ANSWER).await.expect("answer");
        let mut got = vec![0u8; ANSWER.len()];
        client.read_exact(&mut got).await.expect("the client reads its answer");
        client.write_all(GIT_TOOLS_HEAD).await.expect("send the same request again");
        wait_for_bytes(&server, GIT_TOOLS_HEAD.len()).await;
        // A second writer in the same request doesn't look again.
        before_answer(&server).await;
        assert!(!answered_unread(), "the client's next request is not the answered one left unread");
    })
    .await;
}

#[tokio::test]
async fn a_different_request_or_no_head_is_never_the_answered_one() {
    scope(async {
        let (mut client, server) = loopback_pair().await;
        let next = b"POST /cli/daemon/update/check?token=t HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        client.write_all(next).await.expect("send request");
        wait_for_bytes(&server, next.len()).await;
        start(GIT_TOOLS_HEAD);
        before_answer(&server).await;
        assert!(!answered_unread());
        start(b"");
        before_answer(&server).await;
        assert!(!answered_unread(), "an empty head is never checked");
    })
    .await;
}

#[tokio::test]
async fn outside_a_connection_scope_the_probe_does_nothing() {
    let (mut client, server) = loopback_pair().await;
    client.write_all(GIT_TOOLS_HEAD).await.expect("send request");
    wait_for_bytes(&server, GIT_TOOLS_HEAD.len()).await;
    start(GIT_TOOLS_HEAD);
    before_answer(&server).await;
    assert!(!answered_unread());
}

/// The client gets the whole answer, then a clean end of stream: no
/// second copy of it, and no reset.
#[tokio::test]
async fn closing_an_unread_request_delivers_the_answer_then_eof() {
    let (mut client, mut server) = loopback_pair().await;
    client.write_all(GIT_TOOLS_HEAD).await.expect("send request");
    wait_for_bytes(&server, GIT_TOOLS_HEAD.len()).await;
    server.write_all(ANSWER).await.expect("answer");
    close_unread_request(&mut server, GIT_TOOLS_HEAD.len()).await;
    let mut got = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), client.read_to_end(&mut got))
        .await
        .expect("the client sees the end of the stream")
        .expect("a clean close, not a reset");
    assert_eq!(got, ANSWER, "{}", String::from_utf8_lossy(&got));
}
