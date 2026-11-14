//! The keep-alive loop's unread-request backstop.
//!
//! The dispatcher only peeks a request head before routing, and each route
//! arm consumes it. An arm that forgets leaves it at the front of the
//! socket: the next loop pass answers it again, and every later request the
//! client sends on that socket reads one of those answers instead of its own
//! (the `/cli/power/status` and `/cli/git-tools` bugs). The source ratchet
//! (`keep_alive_ratchet.rs`) catches such arms at build time; this catches
//! one that slips past it, at run time, and the loop closes the socket.
//!
//! **It looks right before the answer is written, never after.** Until the
//! client has its answer it sends nothing new, so the answered head at the
//! front of the socket then can only be that same request, still unread.
//! After the answer, the same bytes may be the client's next request: one
//! that sends the same request again on the same socket (a poll, a retry,
//! the same put twice). A check after the answer can't tell the two apart,
//! and closing on the second drops a request the client already sent.
//!
//! The response writers in `http.rs` call [`before_answer`]. An arm that
//! writes its answer some other way leaves no probe; the loop then keeps
//! the socket and relies on the source ratchet.

use std::cell::RefCell;
use std::future::Future;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Default)]
struct Probe {
    /// The current request's head (request line + headers, through the
    /// blank line); empty when it never ended.
    head: Vec<u8>,
    /// Whether the head was still unread when the first answer went out;
    /// `None` until a response writer looked.
    unread: Option<bool>,
}

tokio::task_local! {
    static PROBE: RefCell<Probe>;
}

/// Run one connection's keep-alive loop with its probe.
pub(crate) fn scope<F: Future>(f: F) -> impl Future<Output = F::Output> {
    PROBE.scope(RefCell::new(Probe::default()), f)
}

/// A new request: `head` is its head (empty when it never ended). Forgets
/// the last request's probe.
pub(crate) fn start(head: &[u8]) {
    let _ = PROBE.try_with(|p| {
        let mut p = p.borrow_mut();
        p.head.clear();
        p.head.extend_from_slice(head);
        p.unread = None;
    });
}

/// Called by a response writer right before it writes: is this request's
/// head still at the front of the socket? Only the first answer of a
/// request looks; outside a connection's [`scope`] it does nothing.
pub(crate) async fn before_answer(stream: &TcpStream) {
    let head = PROBE.try_with(|p| {
        let p = p.borrow();
        (p.unread.is_none() && !p.head.is_empty()).then(|| p.head.clone())
    });
    let Ok(Some(head)) = head else { return };
    let unread = head_at_front(stream, &head).await;
    let _ = PROBE.try_with(|p| p.borrow_mut().unread = Some(unread));
}

/// True when this request was answered with its head still unread.
pub(crate) fn answered_unread() -> bool {
    PROBE.try_with(|p| p.borrow().unread == Some(true)).unwrap_or(false)
}

/// Are exactly `head`'s bytes at the front of `stream`? Never waits: a
/// peek that isn't ready at once means nothing is there.
async fn head_at_front(stream: &TcpStream, head: &[u8]) -> bool {
    let mut front = vec![0u8; head.len()];
    match tokio::time::timeout(std::time::Duration::ZERO, stream.peek(&mut front)).await {
        Ok(Ok(n)) => n == head.len() && front == head,
        _ => false,
    }
}

/// How long, and how much past the head, [`close_unread_request`] drains.
const UNREAD_REQUEST_DRAIN_TIME: std::time::Duration = std::time::Duration::from_millis(200);
const UNREAD_REQUEST_DRAIN_BYTES: usize = 64 * 1024;

/// Close a socket whose answered request was left unread
/// ([`answered_unread`]). The client already has its answer; its next
/// request goes on a new connection. Reads the stale head (and up to
/// [`UNREAD_REQUEST_DRAIN_BYTES`] of any body) before closing, so the close
/// is a clean FIN, not a reset that could cost the client the answer it
/// hasn't read yet.
pub(crate) async fn close_unread_request(stream: &mut TcpStream, head_len: usize) {
    let mut head = vec![0u8; head_len];
    let _ = tokio::time::timeout(UNREAD_REQUEST_DRAIN_TIME, stream.read_exact(&mut head)).await;
    let _ = stream.shutdown().await;
    let deadline = tokio::time::Instant::now() + UNREAD_REQUEST_DRAIN_TIME;
    let mut scratch = [0u8; 8 * 1024];
    let mut drained = 0usize;
    while drained < UNREAD_REQUEST_DRAIN_BYTES {
        match tokio::time::timeout_at(deadline, stream.read(&mut scratch)).await {
            Ok(Ok(n)) if n > 0 => drained += n,
            _ => break,
        }
    }
}
