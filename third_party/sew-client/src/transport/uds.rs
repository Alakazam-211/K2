//! HTTP/1.1 over the mgmt Unix socket with hyper: one connection per request (the socket is
//! local; pooling is not worth the code). The connection future is driven inside the call, so
//! nothing is spawned.

use std::path::Path;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

use crate::error::{TransportError, TransportErrorKind};

use super::{Wire, WireResp};

fn chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

fn hyper_error(e: &hyper::Error) -> TransportError {
    let msg = chain(e);
    let kind = if e.is_timeout() {
        TransportErrorKind::Timeout
    } else if e.is_incomplete_message() || e.is_closed() || e.is_canceled() {
        TransportErrorKind::Reset
    } else if e.is_parse() || e.is_user() {
        TransportErrorKind::Protocol
    } else {
        TransportErrorKind::Io
    };
    TransportError::new(kind, true, msg)
}

async fn exchange(socket: &Path, w: &Wire) -> Result<WireResp, TransportError> {
    let stream = UnixStream::connect(socket).await.map_err(|e| {
        TransportError::new(
            TransportErrorKind::Connect,
            false,
            format!("connect {}: {e}", socket.display()),
        )
    })?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| TransportError::new(TransportErrorKind::Connect, false, chain(&e)))?;
    let mut req = http::Request::builder()
        .method(w.method.clone())
        .uri(w.path.as_str())
        .header(http::header::HOST, "sew-mgmt");
    for (k, v) in &w.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let req = req
        .body(Full::new(w.body.clone().unwrap_or_default()))
        .map_err(|e| TransportError::new(TransportErrorKind::Protocol, false, e.to_string()))?;
    let call = async {
        let resp = sender
            .send_request(req)
            .await
            .map_err(|e| hyper_error(&e))?;
        let (parts, body) = resp.into_parts();
        let body: Bytes = body
            .collect()
            .await
            .map_err(|e| hyper_error(&e))?
            .to_bytes();
        let headers = parts
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        Ok::<WireResp, TransportError>(WireResp {
            status: parts.status.as_u16(),
            headers,
            body,
        })
    };
    tokio::pin!(call);
    tokio::pin!(conn);
    tokio::select! {
        biased;
        r = &mut call => r,
        c = &mut conn => match c {
            // The connection ended cleanly (the server closed after answering): the response
            // is already buffered in the call.
            Ok(()) => call.await,
            // A peer that closes after answering can make the connection's shutdown fail
            // (ENOTCONN on macOS): the answer, if complete, still stands.
            Err(e) => call.await.map_err(|_| hyper_error(&e)),
        },
    }
}

pub(crate) async fn send(socket: &Path, w: &Wire) -> Result<WireResp, TransportError> {
    match tokio::time::timeout(w.timeout, exchange(socket, w)).await {
        Ok(r) => r,
        Err(_) => Err(TransportError::new(
            TransportErrorKind::Timeout,
            true,
            format!("no answer from {} within {:?}", socket.display(), w.timeout),
        )),
    }
}
