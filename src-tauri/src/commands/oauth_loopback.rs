//! Client-local OAuth loopback capture (remote Gmail link).
//!
//! When the desktop app is talking to a **remote** K2 daemon, Google's
//! auth-code redirect must land on **this machine** (the one with a
//! browser), not on the daemon host. The flow:
//!
//! 1. `oauth_loopback_bind` — bind `127.0.0.1:<ephemeral>/` and return
//!    `redirectUri` + a capture id.
//! 2. Renderer POSTs `mail/link/oauth/start` with `clientCapture` + that
//!    URI; daemon returns `authorizationUrl` + `state`.
//! 3. Renderer opens the system browser (`openUrl`) and calls
//!    `oauth_loopback_wait` with the expected `state`.
//! 4. This module serves the redirect, validates `state`, returns
//!    `{ code?, error?, state }`.
//! 5. Renderer POSTs `mail/link/oauth/complete` to the daemon (PKCE
//!    verifier never leaves the daemon).
//!
//! Same class of HOST-side exception as `local_upload` / `local_download`:
//! the socket must live on the user's machine.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use uuid::Uuid;

struct BoundListener {
    listener: TcpListener,
    created: Instant,
}

fn pending() -> &'static Mutex<HashMap<String, BoundListener>> {
    static REG: OnceLock<Mutex<HashMap<String, BoundListener>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn purge_stale(map: &mut HashMap<String, BoundListener>) {
    let now = Instant::now();
    map.retain(|_, b| now.duration_since(b.created) < Duration::from_secs(600));
}

/// Bind an ephemeral 127.0.0.1 listener for Google's OAuth redirect.
/// Returns `{ captureId, redirectUri, port }` — never a code.
#[tauri::command]
pub async fn oauth_loopback_bind() -> Result<OauthLoopbackBind, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| format!("bind loopback: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("loopback local_addr: {e}"))?;
        let redirect_uri = format!("http://{addr}/cb");
        let capture_id = Uuid::new_v4().to_string();
        let mut map = pending().lock().map_err(|e| e.to_string())?;
        purge_stale(&mut map);
        map.insert(
            capture_id.clone(),
            BoundListener {
                listener,
                created: Instant::now(),
            },
        );
        Ok(OauthLoopbackBind {
            capture_id,
            redirect_uri,
            port: addr.port(),
        })
    })
    .await
    .map_err(|e| format!("bind task failed: {e}"))?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthLoopbackBind {
    pub capture_id: String,
    pub redirect_uri: String,
    pub port: u16,
}

/// Wait (up to `timeout_secs`, default 300) for a state-valid redirect on
/// the capture bound by [`oauth_loopback_bind`]. Returns the query
/// `code` / `error` / `state` only — never talks to Google.
#[tauri::command]
pub async fn oauth_loopback_wait(
    capture_id: String,
    expected_state: String,
    timeout_secs: Option<u64>,
) -> Result<OauthLoopbackResult, String> {
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(300).clamp(5, 600));
    tauri::async_runtime::spawn_blocking(move || {
        if capture_id.is_empty() {
            return Err("captureId is empty".into());
        }
        if expected_state.is_empty() {
            return Err("expectedState is empty".into());
        }
        let bound = {
            let mut map = pending().lock().map_err(|e| e.to_string())?;
            map.remove(&capture_id)
                .ok_or_else(|| "unknown captureId — call oauth_loopback_bind first".to_string())?
        };
        serve_loopback(&bound.listener, &expected_state, timeout)
    })
    .await
    .map_err(|e| format!("wait task failed: {e}"))?
}

/// Drop a bind that will not be waited on (cancel / error path).
#[tauri::command]
pub async fn oauth_loopback_cancel(capture_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut map = pending().lock().map_err(|e| e.to_string())?;
        map.remove(&capture_id);
        Ok(())
    })
    .await
    .map_err(|e| format!("cancel task failed: {e}"))?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthLoopbackResult {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

fn write_page(stream: &mut std::net::TcpStream, page: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        page.len(),
        page
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_request(req: &str) -> OauthLoopbackResult {
    let mut cap = OauthLoopbackResult {
        code: None,
        state: None,
        error: None,
    };
    let first = req.lines().next().unwrap_or("");
    let target = first.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let val = percent_decode(v);
        let val = if val.is_empty() { None } else { Some(val) };
        match k {
            "code" => cap.code = val,
            "state" => cap.state = val,
            "error" => cap.error = val,
            _ => {}
        }
    }
    cap
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Keep listening past wrong-state probes (mirrors daemon serve_loopback).
fn serve_loopback(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<OauthLoopbackResult, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let mut data: Vec<u8> = Vec::new();
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            data.extend_from_slice(&buf[..n]);
                            if data.windows(4).any(|w| w == b"\r\n\r\n") || data.len() > 16_384 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let req = String::from_utf8_lossy(&data);
                let capture = parse_request(&req);
                let got = capture.state.as_deref().unwrap_or("");
                if expected_state.is_empty()
                    || !constant_time_eq(expected_state.as_bytes(), got.as_bytes())
                {
                    write_page(
                        &mut stream,
                        "This authorization request could not be matched. You can close this tab.",
                    );
                    if Instant::now() >= deadline {
                        return Err("timed out waiting for browser approval".into());
                    }
                    continue;
                }
                write_page(
                    &mut stream,
                    "You can close this tab and return to K2.",
                );
                return Ok(capture);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("timed out waiting for browser approval".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("accept: {e}")),
        }
    }
}
