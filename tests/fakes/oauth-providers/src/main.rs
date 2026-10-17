//! k2-provider-fakes — TEST ONLY (prd-app-mail-calendar-surfaces-v1 §8.2).
//!
//! A tiny std-only HTTP/1.1 server that speaks the subset of Google and
//! Microsoft that K2's Email Link uses, so a headless `k2-daemon` built with
//! `--features provider-fakes` and started with
//! `K2_PROVIDER_FAKES_URL=http://127.0.0.1:<port>` links, checks accounts,
//! refreshes, reads, drafts and sends without touching a real provider.
//! Release builds never compile the override, so this server can't be
//! pointed at by a shipped daemon.
//!
//! Endpoints (paths are what the daemon's `provider-fakes` bases produce):
//!   Microsoft  POST /ms/login/{tenant}/oauth2/v2.0/devicecode
//!              POST /ms/login/{tenant}/oauth2/v2.0/token   (device + refresh)
//!   Graph      /ms/graph/v1.0/me, …/mailFolders/{f}/messages,
//!              …/messages[/{id}[/attachments|/createReply|/send|/move]],
//!              …/sendMail
//!   Google     POST /google/token, POST /google/revoke,
//!              GET /google/gmail/v1/users/me/profile
//!   Control    POST /_fake/config (merge JSON), GET /_fake/log
//!
//! `/_fake/config` keys: `msAccount`, `msTid`, `msTokenError` (an AADSTS
//! description the token endpoint answers until cleared), `pendingPolls`
//! (authorization_pending answers before a device code succeeds),
//! `googleAccount`, `googleHd`.
//!
//! Every Graph send is recorded under `sent` in `/_fake/log` so a test
//! asserts who would have been emailed. Nothing here is a real credential.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Value};

/// Graph's per-attachment inline limit and its request limit.
const MAX_INLINE_ATTACHMENT: usize = 3 * 1024 * 1024;
const MAX_REQUEST: usize = 4 * 1024 * 1024;

struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct State {
    cfg: Value,
    n: u64,
    device_scopes: HashMap<String, String>,
    device_polls: HashMap<String, u64>,
    log: Vec<Value>,
    sent: Vec<Value>,
    drafts: HashMap<String, Value>,
    messages: HashMap<String, Value>,
    revoked: Vec<String>,
}

impl State {
    fn new() -> State {
        let cfg = json!({
            "msAccount": "rosson@contoso.example",
            "msTid": "11111111-2222-3333-4444-555555555555",
            "msTokenError": "",
            "pendingPolls": 1,
            "googleAccount": "rosson@gmail.example",
            "googleHd": "",
        });
        let mut messages = HashMap::new();
        messages.insert(
            "msg-1".to_string(),
            json!({
                "id": "msg-1",
                "subject": "Hello from Alice",
                "receivedDateTime": "2026-10-17T09:00:00Z",
                "isRead": false,
                "hasAttachments": false,
                "conversationId": "conv-1",
                "from": {"emailAddress": {"name": "Alice", "address": "alice@example.com"}},
                "toRecipients": [{"emailAddress": {"address": "rosson@contoso.example"}}],
                "body": {"contentType": "text", "content": "Hi Rosson, lunch Friday?"},
                "internetMessageHeaders": [],
            }),
        );
        State {
            cfg,
            n: 0,
            device_scopes: HashMap::new(),
            device_polls: HashMap::new(),
            log: Vec::new(),
            sent: Vec::new(),
            drafts: HashMap::new(),
            messages,
            revoked: Vec::new(),
        }
    }

    fn next(&mut self) -> u64 {
        self.n += 1;
        self.n
    }

    fn cfg_str(&self, k: &str) -> String {
        self.cfg
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_pairs(s: &str) -> HashMap<String, String> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect()
}

fn read_req(stream: &mut TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let (path, q) = target.split_once('?').unwrap_or((target.as_str(), ""));
    Some(Req {
        method,
        path: path.to_string(),
        query: parse_pairs(q),
        headers,
        body,
    })
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Status",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn id_token(claims: Value) -> String {
    let b64 = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.fake",
        b64(&json!({"alg": "none", "typ": "JWT"})),
        b64(&claims)
    )
}

fn graph_error(code: &str, msg: &str) -> String {
    json!({"error": {"code": code, "message": msg}}).to_string()
}

fn handle(st: &Mutex<State>, req: Req) -> (u16, String) {
    let mut s = st.lock().unwrap_or_else(|p| p.into_inner());
    let form = parse_pairs(&String::from_utf8_lossy(&req.body));
    let json_body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    // Log every request (form fields minus anything secret-shaped).
    let logged_form: HashMap<&String, &String> = form
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "client_secret" | "code_verifier" | "refresh_token" | "token"
            )
        })
        .collect();
    let entry = json!({
        "method": req.method,
        "path": req.path,
        "query": req.query,
        "form": logged_form,
        "bearer": req.headers.get("authorization").map(|a| a.starts_with("Bearer ")),
    });
    s.log.push(entry);
    let p = req.path.as_str();
    let m = req.method.as_str();

    // ── control ──
    if p == "/_fake/config" && m == "POST" {
        if let (Some(cfg), Some(upd)) = (s.cfg.as_object_mut(), json_body.as_object()) {
            for (k, v) in upd {
                cfg.insert(k.clone(), v.clone());
            }
        }
        return (200, s.cfg.to_string());
    }
    if p == "/_fake/log" && m == "GET" {
        let drafts: Vec<&Value> = s.drafts.values().collect();
        return (
            200,
            json!({"requests": s.log, "sent": s.sent, "drafts": drafts, "revoked": s.revoked})
                .to_string(),
        );
    }

    // ── Microsoft identity platform ──
    if let Some(rest) = p.strip_prefix("/ms/login/") {
        let (tenant, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if tail == "oauth2/v2.0/devicecode" && m == "POST" {
            let n = s.next();
            let dc = format!("fake-device-{n}");
            s.device_scopes
                .insert(dc.clone(), form.get("scope").cloned().unwrap_or_default());
            return (
                200,
                json!({
                    "device_code": dc,
                    "user_code": format!("FAKE-{n:04}"),
                    "verification_uri": format!("https://microsoft.example/devicelogin?tenant={tenant}"),
                    "expires_in": 900,
                    "interval": 1,
                })
                .to_string(),
            );
        }
        if tail == "oauth2/v2.0/token" && m == "POST" {
            let err = s.cfg_str("msTokenError");
            if !err.is_empty() {
                return (
                    400,
                    json!({"error": "invalid_grant", "error_description": err}).to_string(),
                );
            }
            let grant = form.get("grant_type").cloned().unwrap_or_default();
            if grant == "urn:ietf:params:oauth:grant-type:device_code" {
                let dc = form.get("device_code").cloned().unwrap_or_default();
                let Some(scope) = s.device_scopes.get(&dc).cloned() else {
                    return (400, json!({"error": "expired_token"}).to_string());
                };
                let pending = s
                    .cfg
                    .get("pendingPolls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let polls = s.device_polls.entry(dc.clone()).or_insert(0);
                *polls += 1;
                if *polls <= pending {
                    return (400, json!({"error": "authorization_pending"}).to_string());
                }
                let n = s.next();
                let claims = json!({
                    "preferred_username": s.cfg_str("msAccount"),
                    "tid": s.cfg_str("msTid"),
                    "oid": format!("oid-{n}"),
                });
                return (
                    200,
                    json!({
                        "token_type": "Bearer",
                        "access_token": format!("ms-at-{n}"),
                        "refresh_token": format!("ms-rt-{n}"),
                        "expires_in": 3600,
                        "scope": scope,
                        "id_token": id_token(claims),
                    })
                    .to_string(),
                );
            }
            if grant == "refresh_token" {
                let n = s.next();
                return (
                    200,
                    json!({
                        "token_type": "Bearer",
                        "access_token": format!("ms-at-{n}"),
                        "refresh_token": format!("ms-rt-{n}"),
                        "expires_in": 3600,
                    })
                    .to_string(),
                );
            }
            return (400, json!({"error": "unsupported_grant_type"}).to_string());
        }
        return (404, json!({"error": "not_found"}).to_string());
    }

    // ── Microsoft Graph ──
    if let Some(rest) = p.strip_prefix("/ms/graph/v1.0") {
        let authed = req
            .headers
            .get("authorization")
            .map(|a| a.starts_with("Bearer ms-at-"))
            .unwrap_or(false);
        if !authed {
            return (
                401,
                graph_error("InvalidAuthenticationToken", "missing or bad bearer"),
            );
        }
        let segs: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
        match (m, segs.as_slice()) {
            ("GET", ["me"]) => {
                let a = s.cfg_str("msAccount");
                return (
                    200,
                    json!({"mail": a, "userPrincipalName": a, "id": "me-1"}).to_string(),
                );
            }
            ("GET", ["me", "mailFolders", _f, "messages"]) => {
                let list: Vec<Value> = s.messages.values().cloned().collect();
                return (200, json!({"value": list}).to_string());
            }
            ("GET", ["me", "mailFolders"]) => {
                return (
                    200,
                    json!({"value": [{"id": "inbox", "displayName": "Inbox"},
                                     {"id": "drafts", "displayName": "Drafts"}]})
                    .to_string(),
                );
            }
            ("POST", ["me", "sendMail"]) => {
                if req.body.len() > MAX_REQUEST {
                    return (413, graph_error("RequestBodyTooLarge", "over 4 MB"));
                }
                let msg = json_body.get("message").cloned().unwrap_or(Value::Null);
                let to_ok = msg
                    .get("toRecipients")
                    .and_then(|t| t.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                if !to_ok {
                    return (400, graph_error("ErrorInvalidRecipients", "no recipients"));
                }
                for a in msg
                    .get("attachments")
                    .and_then(|a| a.as_array())
                    .into_iter()
                    .flatten()
                {
                    let len = a
                        .get("contentBytes")
                        .and_then(|c| c.as_str())
                        .and_then(|c| base64::engine::general_purpose::STANDARD.decode(c).ok())
                        .map(|b| b.len())
                        .unwrap_or(0);
                    if len > MAX_INLINE_ATTACHMENT {
                        return (
                            413,
                            graph_error("ErrorAttachmentSizeLimitExceeded", "over 3 MB"),
                        );
                    }
                }
                let save = json_body
                    .get("saveToSentItems")
                    .cloned()
                    .unwrap_or(Value::Null);
                s.sent
                    .push(json!({"via": "sendMail", "saveToSentItems": save, "message": msg}));
                return (202, String::new());
            }
            ("POST", ["me", "messages"]) => {
                let n = s.next();
                let id = format!("draft-{n}");
                let mut d = json_body.clone();
                d["id"] = json!(id);
                d["isDraft"] = json!(true);
                d["attachments"] = json!([]);
                s.drafts.insert(id.clone(), d.clone());
                return (201, d.to_string());
            }
            ("GET", ["me", "messages", id]) => {
                let id = pct_decode(id);
                if let Some(msg) = s.messages.get(&id).or_else(|| s.drafts.get(&id)) {
                    return (200, msg.to_string());
                }
                return (404, graph_error("ErrorItemNotFound", "not found"));
            }
            ("GET", ["me", "messages", id, "attachments"]) => {
                let id = pct_decode(id);
                let list = s
                    .drafts
                    .get(&id)
                    .and_then(|d| d.get("attachments").cloned());
                return (200, json!({"value": list.unwrap_or(json!([]))}).to_string());
            }
            ("POST", ["me", "messages", id, "attachments"]) => {
                let id = pct_decode(id);
                let bytes = json_body
                    .get("contentBytes")
                    .and_then(|c| c.as_str())
                    .and_then(|c| base64::engine::general_purpose::STANDARD.decode(c).ok());
                let Some(bytes) = bytes else {
                    return (
                        400,
                        graph_error("ErrorInvalidRequest", "contentBytes missing"),
                    );
                };
                if bytes.len() > MAX_INLINE_ATTACHMENT {
                    return (
                        413,
                        graph_error("ErrorAttachmentSizeLimitExceeded", "over 3 MB"),
                    );
                }
                let n = s.next();
                let Some(d) = s.drafts.get_mut(&id) else {
                    return (404, graph_error("ErrorItemNotFound", "no such draft"));
                };
                let att = json!({
                    "id": format!("att-{n}"),
                    "name": json_body.get("name").cloned().unwrap_or(Value::Null),
                    "contentType": json_body.get("contentType").cloned().unwrap_or(Value::Null),
                    "size": bytes.len(),
                });
                if let Some(list) = d.get_mut("attachments").and_then(|a| a.as_array_mut()) {
                    list.push(att.clone());
                }
                return (201, att.to_string());
            }
            ("POST", ["me", "messages", id, "createReply"]) => {
                let id = pct_decode(id);
                let Some(src) = s.messages.get(&id).cloned() else {
                    return (404, graph_error("ErrorItemNotFound", "no such message"));
                };
                let n = s.next();
                let rid = format!("reply-{n}");
                let subj = src.get("subject").and_then(|v| v.as_str()).unwrap_or("");
                let d = json!({
                    "id": rid,
                    "isDraft": true,
                    "subject": format!("RE: {subj}"),
                    "toRecipients": [src.get("from").cloned().unwrap_or(Value::Null)],
                    "conversationId": src.get("conversationId").cloned().unwrap_or(Value::Null),
                    "replyTo": id,
                    "attachments": [],
                });
                s.drafts.insert(rid, d.clone());
                return (201, d.to_string());
            }
            ("PATCH", ["me", "messages", id]) => {
                let id = pct_decode(id);
                let target = if s.drafts.contains_key(&id) {
                    s.drafts.get_mut(&id)
                } else {
                    s.messages.get_mut(&id)
                };
                let Some(t) = target else {
                    return (404, graph_error("ErrorItemNotFound", "not found"));
                };
                if let (Some(obj), Some(upd)) = (t.as_object_mut(), json_body.as_object()) {
                    for (k, v) in upd {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                return (200, t.to_string());
            }
            ("POST", ["me", "messages", id, "send"]) => {
                let id = pct_decode(id);
                let Some(d) = s.drafts.remove(&id) else {
                    return (404, graph_error("ErrorItemNotFound", "no such draft"));
                };
                s.sent.push(json!({"via": "send", "message": d}));
                return (202, String::new());
            }
            ("POST", ["me", "messages", _id, "move"]) => {
                return (201, json!({"id": "moved"}).to_string());
            }
            _ => {
                return (
                    404,
                    graph_error("ErrorInvalidRequest", "fake has no such route"),
                )
            }
        }
    }

    // ── Google ──
    match (m, p) {
        ("POST", "/google/token") => {
            let grant = form.get("grant_type").cloned().unwrap_or_default();
            let n = s.next();
            if grant == "authorization_code" {
                let mut claims =
                    json!({"email": s.cfg_str("googleAccount"), "email_verified": true});
                let hd = s.cfg_str("googleHd");
                if !hd.is_empty() {
                    claims["hd"] = json!(hd);
                }
                return (
                    200,
                    json!({
                        "token_type": "Bearer",
                        "access_token": format!("g-at-{n}"),
                        "refresh_token": format!("g-rt-{n}"),
                        "expires_in": 3599,
                        "scope": "openid https://www.googleapis.com/auth/userinfo.email https://mail.google.com/",
                        "id_token": id_token(claims),
                    })
                    .to_string(),
                );
            }
            if grant == "refresh_token" {
                return (
                    200,
                    json!({"token_type": "Bearer", "access_token": format!("g-at-{n}"), "expires_in": 3599})
                        .to_string(),
                );
            }
            (400, json!({"error": "unsupported_grant_type"}).to_string())
        }
        ("POST", "/google/revoke") => {
            let tok = form.get("token").cloned().unwrap_or_default();
            // Record only the token's kind, never its value.
            let kind = if tok.starts_with("g-rt-") {
                "refresh"
            } else {
                "access"
            };
            s.revoked.push(kind.to_string());
            (200, "{}".to_string())
        }
        ("GET", "/google/gmail/v1/users/me/profile") => (
            200,
            json!({"emailAddress": s.cfg_str("googleAccount")}).to_string(),
        ),
        _ => (404, json!({"error": "not_found", "path": p}).to_string()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port_file = args
        .iter()
        .position(|a| a == "--port-file")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind 127.0.0.1");
    let port = listener.local_addr().expect("local addr").port();
    if let Some(f) = port_file {
        std::fs::write(&f, port.to_string()).expect("write port file");
    }
    println!("LISTENING {port}");
    let state = Arc::new(Mutex::new(State::new()));
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let st = state.clone();
        std::thread::spawn(move || {
            if let Some(req) = read_req(&mut stream) {
                let (status, body) = handle(&st, req);
                respond(&mut stream, status, &body);
            }
        });
    }
}
