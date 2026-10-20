//! HTTP over TCP with reqwest (rustls only, no redirects).

use std::error::Error as _;
use std::time::Duration;

use crate::error::{TransportError, TransportErrorKind};

use super::{Wire, WireResp};

/// The reqwest client: rustls, no redirects, the connect timeout. Request timeouts are set per
/// request.
pub(crate) fn build(connect_timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect_timeout)
        .build()
        .map_err(|e| e.to_string())
}

pub(crate) async fn send(
    http: &reqwest::Client,
    base: &str,
    w: &Wire,
) -> Result<WireResp, TransportError> {
    let mut rb = http
        .request(w.method.clone(), format!("{base}{}", w.path))
        .timeout(w.timeout);
    for (k, v) in &w.headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some(b) = &w.body {
        rb = rb.body(b.clone());
    }
    let resp = rb.send().await.map_err(map_error)?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = resp.bytes().await.map_err(map_error)?;
    Ok(WireResp {
        status,
        headers,
        body,
    })
}

/// The whole error chain, without the URL (it may carry a signed-URL `sig`).
fn chain(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

pub(crate) fn map_error(e: reqwest::Error) -> TransportError {
    let e = e.without_url();
    let msg = chain(&e);
    let lower = msg.to_ascii_lowercase();
    let tls = lower.contains("tls") || lower.contains("certificate");
    if e.is_connect() {
        let kind = if tls {
            TransportErrorKind::Tls
        } else if e.is_timeout() {
            TransportErrorKind::Timeout
        } else {
            TransportErrorKind::Connect
        };
        return TransportError::new(kind, false, msg);
    }
    if e.is_builder() {
        return TransportError::new(TransportErrorKind::Protocol, false, msg);
    }
    let kind = if e.is_timeout() {
        TransportErrorKind::Timeout
    } else if lower.contains("reset") || lower.contains("closed") || lower.contains("broken pipe") {
        TransportErrorKind::Reset
    } else if e.is_request() || e.is_body() {
        TransportErrorKind::Io
    } else {
        TransportErrorKind::Protocol
    };
    TransportError::new(kind, true, msg)
}
