//! Official Dannon skin gateway helper (`k2-daemon --skin-gateway`).
//!
//! Child of k2-daemon (`kind=skin`). Serves static UI + HttpOnly cookie
//! on this origin + allowlisted Thread proxy. Holds `k2skn_` in **this
//! process memory only**. Never binds inside the parent daemon.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

pub const COOKIE_NAME: &str = "k2_skin_ui";
pub const GATEWAY_FORBIDDEN_JSON: &str = r#"{"error":"not allowed"}"#;
const LOGIN_HTML: &str = include_str!("login.html");
const APP_HTML: &str = include_str!("app.html");
const APP_CSS: &str = include_str!("app.css");
const APP_JS: &str = include_str!("app.js");

#[derive(Clone)]
struct Session {
    token: String,
}

struct Gateway {
    upstream_host: String,
    root: Option<PathBuf>,
    sessions: Mutex<HashMap<String, Session>>,
}

#[derive(Debug)]
struct Args {
    listen: SocketAddr,
    upstream: String,
    root: Option<PathBuf>,
}

/// Entry from `k2-daemon --skin-gateway …`. Does **not** boot the daemon.
pub fn run_from_args(args: &[String]) -> i32 {
    match parse_args(args) {
        Ok(a) => {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("skin-gateway runtime: {e}");
                    return 1;
                }
            };
            match rt.block_on(serve(a)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("skin-gateway: {e}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("{e}");
            2
        }
    }
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut listen: Option<SocketAddr> = None;
    let mut upstream: Option<String> = None;
    let mut root: Option<PathBuf> = None;
    let mut i = 1usize;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--skin-gateway" {
            i += 1;
            continue;
        }
        let (key, inline) = if let Some(rest) = a.strip_prefix("--listen=") {
            ("--listen", Some(rest.to_string()))
        } else if let Some(rest) = a.strip_prefix("--upstream=") {
            ("--upstream", Some(rest.to_string()))
        } else if let Some(rest) = a.strip_prefix("--root=") {
            ("--root", Some(rest.to_string()))
        } else {
            (a, None)
        };
        match key {
            "--listen" => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or("--listen needs 127.0.0.1:PORT")?
                    }
                };
                let addr: SocketAddr = v
                    .parse()
                    .map_err(|_| format!("invalid --listen {v}"))?;
                if !addr.ip().is_loopback() {
                    return Err("--listen must be loopback 127.0.0.1".into());
                }
                listen = Some(addr);
            }
            "--upstream" => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or("--upstream needs http://127.0.0.1:PORT")?
                    }
                };
                if !v.starts_with("http://127.0.0.1:") && !v.starts_with("http://localhost:") {
                    return Err("--upstream must be http://127.0.0.1:<daemon-port>".into());
                }
                upstream = Some(v.trim_end_matches('/').to_string());
            }
            "--root" => {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i).cloned().ok_or("--root needs a directory")?
                    }
                };
                root = Some(PathBuf::from(v));
            }
            "--help" | "-h" => {
                return Err(
                    "k2-daemon --skin-gateway --listen 127.0.0.1:N --upstream http://127.0.0.1:DAEMON [--root DIR]"
                        .into(),
                );
            }
            _ => return Err(format!("unknown skin-gateway flag {a}")),
        }
        i += 1;
    }
    let listen = listen.ok_or("missing --listen 127.0.0.1:N")?;
    let upstream = upstream.ok_or("missing --upstream http://127.0.0.1:DAEMON")?;
    Ok(Args {
        listen,
        upstream,
        root,
    })
}

/// Synthesize helper argv (no exe). Daemon port is **current**, never persisted.
pub fn helper_argv(listen_port: u16, daemon_port: u16, root: Option<&Path>) -> Vec<String> {
    let mut v = vec![
        "--skin-gateway".into(),
        "--listen".into(),
        format!("127.0.0.1:{listen_port}"),
        "--upstream".into(),
        format!("http://127.0.0.1:{daemon_port}"),
    ];
    if let Some(root) = root {
        v.push("--root".into());
        v.push(root.display().to_string());
    }
    v
}

pub fn never_proxy(path: &str) -> bool {
    let p = path_only(path);
    matches!(
        p,
        "/cli/sessions/grid"
            | "/cli/sessions/bytes"
            | "/cli/sessions/events"
            | "/cli/sessions/subscribe"
            | "/cli/grid"
            | "/cli/pty"
            | "/cli/auth/login"
            | "/events"
    ) || p.starts_with("/cli/terminal/")
        || p == "/cli/terminal"
        || p.starts_with("/v1/")
        || p == "/v1"
}

pub fn allowlisted_http(method: &str, path: &str) -> bool {
    let p = path_only(path);
    let m = method.to_ascii_uppercase();
    matches!(
        (m.as_str(), p),
        ("GET", "/cli/skin/agents")
            | ("HEAD", "/cli/skin/agents")
            | ("GET", "/cli/thread")
            | ("HEAD", "/cli/thread")
            | ("POST", "/cli/thread/post")
            | ("POST", "/cli/thread/answer")
            | ("POST", "/cli/thread/void")
    )
}

pub fn allowlisted_ws(path: &str) -> bool {
    path_only(path) == "/cli/overlay/events"
}

pub fn is_static_path(path: &str) -> bool {
    let p = path_only(path);
    p == "/" || p == "/login" || p == "/index.html" || p.starts_with("/assets")
}

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

async fn serve(args: Args) -> Result<(), String> {
    if let Some(root) = args.root.as_ref() {
        eprintln!(
            "skin-gateway: {} is PUBLIC (no cookie). Login only guards Thread. Do not put wiki, contracts, or other private files in this folder.",
            root.display()
        );
    }
    let listener = TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("bind {}: {e}", args.listen))?;
    let host = args
        .upstream
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();
    let gw = Arc::new(Gateway {
        upstream_host: host,
        root: args.root,
        sessions: Mutex::new(HashMap::new()),
    });
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| format!("accept: {e}"))?;
        let gw = Arc::clone(&gw);
        tokio::spawn(async move {
            let _ = handle_conn(gw, stream).await;
        });
    }
}

async fn handle_conn(gw: Arc<Gateway>, mut stream: TcpStream) -> Result<(), ()> {
    let mut buf = vec![0u8; 64 * 1024];
    let n = match stream.read(&mut buf).await {
        Ok(0) | Err(_) => return Ok(()),
        Ok(n) => n,
    };
    let raw = &buf[..n];
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let (header_bytes, early_body) = match header_end {
        Some(end) => (&raw[..end], &raw[end..]),
        None => (raw, &raw[0..0]),
    };
    let head = String::from_utf8_lossy(header_bytes);
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let path = path_only(&target).to_string();
    let content_length = head.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    }).unwrap_or(0);
    let mut body = early_body.to_vec();
    if body.len() < content_length {
        let need = content_length - body.len();
        let mut more = vec![0u8; need];
        let mut got = 0;
        while got < need {
            match stream.read(&mut more[got..]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        body.extend_from_slice(&more[..got]);
    }
    if body.len() > content_length {
        body.truncate(content_length);
    }
    let upgrade = header_has_upgrade(&head);
    let cookie = extract_cookie(&head, COOKIE_NAME);
    let xf_proto = header_value(&head, "x-forwarded-proto");
    let secure = xf_proto
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false);

    if never_proxy(&path) {
        write_json(&mut stream, "403 Forbidden", GATEWAY_FORBIDDEN_JSON).await;
        return Ok(());
    }

    if upgrade {
        if !allowlisted_ws(&path) {
            write_json(&mut stream, "403 Forbidden", GATEWAY_FORBIDDEN_JSON).await;
            return Ok(());
        }
        let Some(sid) = cookie else {
            write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
            return Ok(());
        };
        let token = {
            let g = gw.sessions.lock().await;
            g.get(&sid).map(|s| s.token.clone())
        };
        let Some(token) = token else {
            write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
            return Ok(());
        };
        proxy_upgrade(&gw, &mut stream, &head, &target, &token).await;
        return Ok(());
    }

    match (method.as_str(), path.as_str()) {
        ("POST", "/login") => {
            handle_login(&gw, &mut stream, &head, &body, secure).await;
        }
        ("POST", "/logout") => {
            handle_logout(&gw, &mut stream, cookie.as_deref()).await;
        }
        (m, p) if (m == "GET" || m == "HEAD") && is_static_path(p) => {
            serve_static(&gw, &mut stream, p, m == "HEAD").await;
        }
        (m, p) if allowlisted_http(m, p) => {
            let Some(sid) = cookie else {
                write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
                return Ok(());
            };
            let token = {
                let g = gw.sessions.lock().await;
                g.get(&sid).map(|s| s.token.clone())
            };
            let Some(token) = token else {
                write_json(&mut stream, "401 Unauthorized", r#"{"error":"not logged in"}"#).await;
                return Ok(());
            };
            proxy_http(&gw, &mut stream, &method, &target, &head, &body, &token).await;
        }
        _ => {
            write_json(&mut stream, "404 Not Found", r#"{"error":"not found"}"#).await;
        }
    }
    Ok(())
}

fn header_has_upgrade(head: &str) -> bool {
    head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("upgrade:") && l.contains("websocket")
    })
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.lines() {
        let Some(colon) = line.find(':') else { continue };
        if line[..colon].eq_ignore_ascii_case(name) {
            let v = line[colon + 1..].trim();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn extract_cookie(head: &str, name: &str) -> Option<String> {
    let raw = header_value(head, "cookie")?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(name).and_then(|s| s.strip_prefix('=')) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn set_cookie_value(id: &str, secure: bool) -> String {
    let mut v = format!("{COOKIE_NAME}={id}; HttpOnly; SameSite=Lax; Path=/");
    if secure {
        v.push_str("; Secure");
    }
    v
}

fn clear_cookie_value(secure: bool) -> String {
    let mut v = format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        v.push_str("; Secure");
    }
    v
}

async fn handle_login(
    gw: &Gateway,
    stream: &mut TcpStream,
    head: &str,
    body: &[u8],
    secure: bool,
) {
    let ct = header_value(head, "content-type").unwrap_or("");
    let (username, password) = parse_login_body(body, ct);
    if username.is_empty() || password.is_empty() {
        write_json(
            stream,
            "401 Unauthorized",
            r#"{"error":"invalid username or password"}"#,
        )
        .await;
        return;
    }
    let payload = serde_json::json!({
        "username": username,
        "password": password,
    })
    .to_string();
    match upstream_json(
        gw,
        "POST",
        "/cli/skin/login",
        Some(payload.as_bytes()),
        None,
    )
    .await
    {
        Ok((status, resp_body)) => {
            if status != 200 {
                let body = if resp_body.contains("k2skn_") {
                    r#"{"error":"invalid username or password"}"#.to_string()
                } else {
                    resp_body
                };
                write_json(stream, status_line(status), &body).await;
                return;
            }
            let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&resp_body) else {
                write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
                return;
            };
            let token = v
                .get("token")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if !token.starts_with("k2skn_") {
                write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
                return;
            }
            if let Some(obj) = v.as_object_mut() {
                obj.remove("token");
            }
            let out = v.to_string();
            if out.contains("k2skn_") {
                write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
                return;
            }
            let sid = opaque_session_id();
            gw.sessions.lock().await.insert(sid.clone(), Session { token });
            write_json_cookie(stream, "200 OK", &out, &set_cookie_value(&sid, secure)).await;
        }
        Err(_) => {
            write_json(stream, "502 Bad Gateway", r#"{"error":"login upstream"}"#).await;
        }
    }
}

fn parse_login_body(body: &[u8], content_type: &str) -> (String, String) {
    if content_type.to_ascii_lowercase().contains("application/json")
        || body.first().copied() == Some(b'{')
    {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
            let username = v
                .get("username")
                .or_else(|| v.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let password = v
                .get("password")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            return (username, password);
        }
    }
    let s = String::from_utf8_lossy(body);
    let mut username = String::new();
    let mut password = String::new();
    for pair in s.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let k = k.trim();
            let v = urlencoding_lite(v);
            if k.eq_ignore_ascii_case("username") || k.eq_ignore_ascii_case("name") {
                username = v;
            } else if k.eq_ignore_ascii_case("password") {
                password = v;
            }
        }
    }
    (username, password)
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(' '),
            b'%' if i + 2 < b.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v as char);
                    i += 3;
                    continue;
                }
                out.push('%');
            }
            c => out.push(c as char),
        }
        i += 1;
    }
    out
}

fn opaque_session_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn handle_logout(gw: &Gateway, stream: &mut TcpStream, cookie: Option<&str>) {
    if let Some(sid) = cookie {
        let token = {
            let mut g = gw.sessions.lock().await;
            g.remove(sid).map(|s| s.token)
        };
        if let Some(token) = token {
            let _ = upstream_json(gw, "POST", "/cli/skin/logout", Some(b"{}"), Some(&token)).await;
        }
    }
    write_json_cookie(
        stream,
        "200 OK",
        r#"{"ok":true}"#,
        &clear_cookie_value(false),
    )
    .await;
}

/// GET `/login`: `<dir>/login.html` when `--root` has it; else bundled.
/// Missing file is bundled, never 404 (130 regression).
fn login_static_bytes(root: Option<&Path>) -> Vec<u8> {
    if let Some(root) = root {
        if let Ok((_, bytes)) = read_static_file(root, "/login") {
            return bytes;
        }
    }
    LOGIN_HTML.as_bytes().to_vec()
}

async fn serve_static(gw: &Gateway, stream: &mut TcpStream, path: &str, head_only: bool) {
    if path == "/login" {
        let body = login_static_bytes(gw.root.as_deref());
        write_bytes(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            &body,
            head_only,
        )
        .await;
        return;
    }
    if let Some(root) = gw.root.as_ref() {
        match read_static_file(root, path) {
            Ok((ct, bytes)) => {
                write_bytes(stream, "200 OK", &ct, &bytes, head_only).await;
            }
            Err(_) => {
                write_json(stream, "404 Not Found", r#"{"error":"not found"}"#).await;
            }
        }
        return;
    }
    let (ct, body) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", APP_HTML.as_bytes()),
        "/login" => ("text/html; charset=utf-8", LOGIN_HTML.as_bytes()),
        "/assets/app.css" => ("text/css; charset=utf-8", APP_CSS.as_bytes()),
        "/assets/app.js" => ("text/javascript; charset=utf-8", APP_JS.as_bytes()),
        _ => {
            write_json(stream, "404 Not Found", r#"{"error":"not found"}"#).await;
            return;
        }
    };
    write_bytes(stream, "200 OK", ct, body, head_only).await;
}

fn read_static_file(root: &Path, url_path: &str) -> Result<(String, Vec<u8>), ()> {
    let rel = match url_path {
        "/" | "/index.html" => "index.html",
        "/login" => "login.html",
        p if p.starts_with('/') => &p[1..],
        p => p,
    };
    if rel.contains('\0') || rel.split('/').any(|c| c == "..") {
        return Err(());
    }
    let joined = root.join(rel);
    let canon = joined.canonicalize().map_err(|_| ())?;
    let root_c = root.canonicalize().map_err(|_| ())?;
    if !canon.starts_with(&root_c) {
        return Err(());
    }
    let bytes = std::fs::read(&canon).map_err(|_| ())?;
    let ct = match canon.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    Ok((ct.to_string(), bytes))
}

fn upstream_addr(gw: &Gateway) -> Result<String, String> {
    Ok(gw.upstream_host.clone())
}

async fn connect_upstream(gw: &Gateway) -> Result<TcpStream, String> {
    let addr = upstream_addr(gw)?;
    tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr))
        .await
        .map_err(|_| "upstream timeout".to_string())?
        .map_err(|e| format!("upstream connect: {e}"))
}

async fn upstream_json(
    gw: &Gateway,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    bearer: Option<&str>,
) -> Result<(u16, String), String> {
    let mut up = connect_upstream(gw).await?;
    let payload = body.unwrap_or(&[]);
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        gw.upstream_host
    );
    if let Some(t) = bearer {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    if !payload.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    } else if method == "POST" {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    up.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    if !payload.is_empty() {
        up.write_all(payload).await.map_err(|e| e.to_string())?;
    }
    up.flush().await.map_err(|e| e.to_string())?;
    let (_head, status, body) = read_http_message(&mut up).await?;
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

async fn read_http_message(stream: &mut TcpStream) -> Result<(String, u16, Vec<u8>), String> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
            .await
            .map_err(|_| "upstream header timeout".to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break Some(i + 4);
        }
        if raw.len() > 64 * 1024 {
            return Err("upstream headers too large".into());
        }
    };
    let Some(end) = header_end else {
        return Err("upstream closed".into());
    };
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(502);
    let clen = head.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    let mut body = raw[end..].to_vec();
    if let Some(need) = clen {
        while body.len() < need {
            let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
                .await
                .map_err(|_| "upstream body timeout".to_string())?
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        if body.len() > need {
            body.truncate(need);
        }
    }
    Ok((head, status, body))
}

/// Browser → gateway cookie; gateway → daemon Bearer. Never Cookie, never ?token=,
/// never forward browser Authorization.
async fn proxy_http(
    gw: &Gateway,
    client: &mut TcpStream,
    method: &str,
    target: &str,
    client_head: &str,
    body: &[u8],
    token: &str,
) {
    let path_q = if target.starts_with('/') {
        target
    } else {
        "/"
    };
    // Strip any incoming token= from the official origin.
    let (path, query) = match path_q.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_q, ""),
    };
    let query = strip_token_query(query);
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let mut up = match connect_upstream(gw).await {
        Ok(s) => s,
        Err(_) => {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
            return;
        }
    };
    let mut req = format!(
        "{method} {target} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n",
        gw.upstream_host
    );
    if !body.is_empty() {
        let ct = header_value(client_head, "content-type").unwrap_or("application/json");
        req.push_str(&format!("Content-Type: {ct}\r\n"));
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    } else if method.eq_ignore_ascii_case("POST") {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    if up.write_all(req.as_bytes()).await.is_err() {
        write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        return;
    }
    if !body.is_empty() && up.write_all(body).await.is_err() {
        write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        return;
    }
    let _ = up.flush().await;
    match read_http_message(&mut up).await {
        Ok((head, _status, body)) => {
            // Rebuild so Content-Length matches the body we actually hold.
            let first = head.lines().next().unwrap_or("HTTP/1.1 200 OK");
            let mut out = format!("{first}\r\n");
            for line in head.lines().skip(1) {
                let l = line.to_ascii_lowercase();
                if l.starts_with("content-length:")
                    || l.starts_with("transfer-encoding:")
                    || l.starts_with("connection:")
                    || line.is_empty()
                {
                    continue;
                }
                out.push_str(line);
                out.push_str("\r\n");
            }
            out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
            let _ = client.write_all(out.as_bytes()).await;
            let _ = client.write_all(&body).await;
            let _ = client.flush().await;
        }
        Err(_) => {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        }
    }
}

fn strip_token_query(query: &str) -> String {
    let mut out = String::new();
    for pair in query.split('&') {
        if pair.is_empty() || pair.starts_with("token=") {
            continue;
        }
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(pair);
    }
    out
}

async fn proxy_upgrade(
    gw: &Gateway,
    client: &mut TcpStream,
    client_head: &str,
    target: &str,
    token: &str,
) {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let query = strip_token_query(query);
    if !query.contains("conversation=") {
        write_json(
            client,
            "400 Bad Request",
            r#"{"error":"missing conversation query parameter"}"#,
        )
        .await;
        return;
    }
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let mut up = match connect_upstream(gw).await {
        Ok(s) => s,
        Err(_) => {
            write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
            return;
        }
    };
    let mut req = format!(
        "GET {target} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
        gw.upstream_host
    );
    for name in [
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
        "sec-websocket-extensions",
        "origin",
    ] {
        if let Some(v) = header_value(client_head, name) {
            req.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    req.push_str("\r\n");
    if up.write_all(req.as_bytes()).await.is_err() {
        write_json(client, "502 Bad Gateway", r#"{"error":"upstream"}"#).await;
        return;
    }
    let _ = up.flush().await;
    let _ = tokio::io::copy_bidirectional(client, &mut up).await;
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        400 => "400 Bad Request",
        502 => "502 Bad Gateway",
        _ => "500 Internal Server Error",
    }
}

async fn write_json(stream: &mut TcpStream, status: &str, body: &str) {
    write_bytes(stream, status, "application/json", body.as_bytes(), false).await;
}

async fn write_json_cookie(stream: &mut TcpStream, status: &str, body: &str, cookie: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nSet-Cookie: {cookie}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

fn http_headers(status: &str, ct: &str, len: usize) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    )
}

async fn write_bytes(
    stream: &mut TcpStream,
    status: &str,
    ct: &str,
    body: &[u8],
    head_only: bool,
) {
    let resp = http_headers(status, ct, body.len());
    let _ = stream.write_all(resp.as_bytes()).await;
    if !head_only {
        let _ = stream.write_all(body).await;
    }
    let _ = stream.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_proxy_covers_prd_g8() {
        for p in [
            "/cli/sessions/grid",
            "/cli/sessions/bytes",
            "/cli/sessions/events",
            "/cli/sessions/subscribe",
            "/cli/grid",
            "/cli/pty",
            "/cli/terminal/foo",
            "/cli/auth/login",
            "/v1/w",
            "/events",
        ] {
            assert!(never_proxy(p), "{p}");
        }
        assert!(!never_proxy("/cli/thread"));
        assert!(!never_proxy("/cli/thread/post"));
        assert!(!never_proxy("/cli/overlay/events"));
        assert!(!never_proxy("/cli/skin/agents"));
    }

    #[test]
    fn allowlist_is_exact_not_thread_star() {
        assert!(allowlisted_http("GET", "/cli/thread"));
        assert!(allowlisted_http("POST", "/cli/thread/post"));
        assert!(allowlisted_http("POST", "/cli/thread/answer"));
        assert!(allowlisted_http("POST", "/cli/thread/void"));
        assert!(!allowlisted_http("GET", "/cli/thread/post"));
        assert!(!allowlisted_http("GET", "/cli/thread/foo"));
        assert!(!allowlisted_http("GET", "/cli/thread/answer"));
        assert!(!allowlisted_http("POST", "/cli/thread"));
        assert!(!allowlisted_http("POST", "/cli/thread/ask"));
        assert!(!allowlisted_http("POST", "/cli/thread/secret"));
        assert!(allowlisted_ws("/cli/overlay/events?conversation=abc"));
        assert!(!allowlisted_ws("/cli/sessions/events"));
        assert!(!allowlisted_ws("/cli/fs/events"));
        assert!(!allowlisted_http("GET", "/cli/fs/read-dir"));
        assert!(!allowlisted_http("GET", "/cli/fs/read-file"));
        assert!(!allowlisted_http("POST", "/cli/fs/write-file"));
    }

    #[test]
    fn login_html_uses_root_file_else_bundled_never_404() {
        let bundled = login_static_bytes(None);
        let bundled_s = String::from_utf8_lossy(&bundled);
        assert!(
            bundled_s.contains("Sign in — K2"),
            "no --root must be bundled: {bundled_s}"
        );

        let dir = std::env::temp_dir().join(format!(
            "k2-skin-login-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp skin dir");
        let missing = login_static_bytes(Some(&dir));
        let missing_s = String::from_utf8_lossy(&missing);
        assert!(
            missing_s.contains("Sign in — K2"),
            "missing login.html must stay bundled, never 404: {missing_s}"
        );
        assert_eq!(missing, bundled);

        let custom = "<!DOCTYPE html><title>Custom Skin Login 2.1</title>";
        std::fs::write(dir.join("login.html"), custom).expect("write login.html");
        let served = login_static_bytes(Some(&dir));
        let served_s = String::from_utf8_lossy(&served);
        assert!(
            served_s.contains("Custom Skin Login 2.1"),
            "present login.html must be served: {served_s}"
        );
        assert!(
            !served_s.contains("Sign in — K2"),
            "custom login must not be the bundled K2 title: {served_s}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn static_and_json_headers_are_no_store() {
        let h = http_headers("200 OK", "text/javascript; charset=utf-8", 12);
        assert!(h.contains("Cache-Control: no-store"), "{h}");
        assert!(h.contains("Content-Type: text/javascript; charset=utf-8"), "{h}");
    }

    #[test]
    fn login_path_is_static_and_not_never_proxy() {
        assert!(is_static_path("/login"));
        assert!(!never_proxy("/login"));
        assert_eq!(
            include_str!("login.html").contains("Sign in"),
            true,
            "bundled GET /login must be a form, not a 404"
        );
    }

    #[test]
    fn cookie_name_is_k2_skin_ui_not_session() {
        assert_eq!(COOKIE_NAME, "k2_skin_ui");
        assert_ne!(COOKIE_NAME, "k2_skin_session");
        let v = set_cookie_value("abc", false);
        assert!(v.contains("HttpOnly"));
        assert!(v.contains("SameSite=Lax"));
        assert!(v.contains("Path=/"));
        assert!(!v.contains("Secure"), "{v}");
        let s = set_cookie_value("abc", true);
        assert!(s.contains("Secure"), "{s}");
        assert!(!v.contains("k2skn_"));
    }

    #[test]
    fn helper_argv_does_not_embed_cmd_shell() {
        let a = helper_argv(8788, 4242, None);
        assert_eq!(a[0], "--skin-gateway");
        assert!(a.contains(&"127.0.0.1:8788".to_string()));
        assert!(a.contains(&"http://127.0.0.1:4242".to_string()));
        assert!(!a.iter().any(|s| s.contains("(skin)")));
    }

    #[test]
    fn parse_listen_rejects_non_loopback() {
        let err = parse_args(&[
            "k2-daemon".into(),
            "--skin-gateway".into(),
            "--listen".into(),
            "0.0.0.0:9".into(),
            "--upstream".into(),
            "http://127.0.0.1:8".into(),
        ])
        .unwrap_err();
        assert!(err.contains("loopback"), "{err}");
    }
}
