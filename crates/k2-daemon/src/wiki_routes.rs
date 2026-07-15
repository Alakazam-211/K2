//! Daemon-side `/cli/wiki/*` routes — workspace knowledge base (brain map).
//!
//! GET: index, note, serve status. POST: seed, serve on/off.
//! When serve is enabled, binds a **read-only** localhost site on
//! `127.0.0.1` only (no daemon token): SPA + `/api/index` + `/api/note`.
//! See `.k2/prds/prd-workspace-kb-brain-map-and-publish.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::cli_response::CliResponse;

// ── Serve state (process-wide, per workspace) ──────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServeStatus {
    pub enabled: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub workspace_path: String,
}

struct ServeEntry {
    port: u16,
    /// `true` = cancel requested.
    cancel: watch::Sender<bool>,
}

fn servers() -> &'static Mutex<HashMap<String, ServeEntry>> {
    static S: OnceLock<Mutex<HashMap<String, ServeEntry>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn workspace_key(workspace: &Path) -> String {
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn status_for(workspace: &Path) -> ServeStatus {
    let key = workspace_key(workspace);
    let guard = servers().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = guard.get(&key) {
        ServeStatus {
            enabled: true,
            port: Some(entry.port),
            url: Some(format!("http://127.0.0.1:{}", entry.port)),
            workspace_path: key,
        }
    } else {
        ServeStatus {
            enabled: false,
            port: None,
            url: None,
            workspace_path: key,
        }
    }
}

fn stop_server(workspace: &Path) {
    let key = workspace_key(workspace);
    let mut guard = servers().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = guard.remove(&key) {
        let _ = entry.cancel.send(true);
    }
}

fn start_server(workspace: PathBuf, port: u16) -> Result<ServeStatus, String> {
    stop_server(&workspace);

    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "wiki serve requires the daemon tokio runtime".to_string())?;

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let ws = workspace.clone();

    let (listener, actual_port) = handle.block_on(async {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
        let actual = listener
            .local_addr()
            .map_err(|e| format!("local_addr: {e}"))?
            .port();
        Ok::<_, String>((listener, actual))
    })?;

    let key = workspace_key(&workspace);
    {
        let mut guard = servers().lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(
            key.clone(),
            ServeEntry {
                port: actual_port,
                cancel: cancel_tx,
            },
        );
    }

    handle.spawn(async move {
        run_wiki_site(ws, listener, cancel_rx).await;
    });

    Ok(ServeStatus {
        enabled: true,
        port: Some(actual_port),
        url: Some(format!("http://127.0.0.1:{actual_port}")),
        workspace_path: key,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────

fn need_project_path(params: &HashMap<String, String>) -> Result<PathBuf, CliResponse> {
    // Prefer `project` over `project_path` (same as inbox_routes).
    for key in &["project", "project_path"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Ok(PathBuf::from(v));
            }
        }
    }
    Err(CliResponse::bad_request(
        "Missing project (or project_path) parameter",
    ))
}

fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

fn bool_param(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(
        params.get(key).map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

// ── GET handlers ───────────────────────────────────────────────────────

/// GET /cli/wiki/index?project=<path>
/// GET /cli/wiki/index?scope=k2 — fleet map (all workspace brains; host registry).
pub fn handle_index(params: &HashMap<String, String>) -> CliResponse {
    let scope = str_param(params, "scope").to_ascii_lowercase();
    if scope == "k2" || scope == "host" || scope == "fleet" {
        return match k2_core::wiki::build_fleet_index() {
            Ok(idx) => CliResponse::ok_json(
                serde_json::to_string(&idx).unwrap_or_else(|_| "{}".to_string()),
            ),
            Err(e) => CliResponse::bad_request(e),
        };
    }
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match k2_core::wiki::build_index(&workspace) {
        Ok(idx) => CliResponse::ok_json(
            serde_json::to_string(&idx).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/wiki/note?project=<path>&id=<wiki-rel-id>
/// Fleet notes: id = `{workspaceId}::{noteId}` (project optional).
pub fn handle_note(params: &HashMap<String, String>) -> CliResponse {
    let id = str_param(params, "id");
    if id.is_empty() {
        return CliResponse::bad_request("Missing id");
    }
    let workspace = need_project_path(params).ok();
    match k2_core::wiki::read_note_fleet_or_local(workspace.as_deref(), &id) {
        Ok(note) => CliResponse::ok_json(
            serde_json::to_string(&note).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/wiki/serve/status?project=<path>
/// Also exposed as GET /cli/wiki/status.
pub fn handle_serve_status(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let st = status_for(&workspace);
    CliResponse::ok_json(serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()))
}

// ── POST handlers ──────────────────────────────────────────────────────

/// POST /cli/wiki/seed?project=<path>
pub fn handle_seed_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match k2_core::wiki::seed_wiki(&workspace) {
        Ok(created) => CliResponse::ok_json(
            serde_json::json!({
                "success": true,
                "created": created,
                "wikiRel": k2_core::wiki::WIKI_REL,
            })
            .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/wiki/serve — query/form: enabled=true|false, port? (0 = ephemeral)
pub fn handle_serve_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Accept enabled= / action= / path-style via `mode` param.
    let mode = str_param(params, "mode").to_ascii_lowercase();
    let action = str_param(params, "action").to_ascii_lowercase();
    let enabled = if !mode.is_empty() {
        matches!(mode.as_str(), "on" | "start" | "true" | "1")
    } else if !action.is_empty() {
        matches!(action.as_str(), "on" | "start" | "true" | "1")
    } else if params.contains_key("enabled") {
        bool_param(params, "enabled")
    } else {
        return CliResponse::bad_request("Missing enabled (true/false) or mode=on|off");
    };

    if !enabled {
        stop_server(&workspace);
        let st = status_for(&workspace);
        return CliResponse::ok_json(
            serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()),
        );
    }

    let port: u16 = str_param(params, "port")
        .parse()
        .unwrap_or(0);

    match start_server(workspace, port) {
        Ok(st) => CliResponse::ok_json(
            serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── POST dispatcher ────────────────────────────────────────────────────

pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/wiki/seed" => handle_seed_post(params),
        "/cli/wiki/serve" => handle_serve_post(params),
        // Convenience aliases
        "/cli/wiki/serve/on" => {
            let mut p = params.clone();
            p.insert("enabled".into(), "true".into());
            handle_serve_post(&p)
        }
        "/cli/wiki/serve/off" => {
            let mut p = params.clone();
            p.insert("enabled".into(), "false".into());
            handle_serve_post(&p)
        }
        _ => CliResponse::not_found(),
    }
}

// ── Localhost site (127.0.0.1 only, no token) ──────────────────────────

async fn run_wiki_site(
    workspace: PathBuf,
    listener: TcpListener,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let ws = workspace.clone();
                        tokio::spawn(async move {
                            handle_site_conn(ws, stream).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn handle_site_conn(workspace: PathBuf, mut stream: tokio::net::TcpStream) {
    let mut buf = vec![0u8; 16 * 1024];
    let n = match stream.read(&mut buf).await {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let target = parts.next().unwrap_or("/");

    if method != "GET" && method != "HEAD" {
        write_site_response(
            &mut stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"error":"read-only"}"#,
        )
        .await;
        return;
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };

    match path {
        "/api/index" => {
            let body = match tokio::task::spawn_blocking({
                let ws = workspace.clone();
                move || k2_core::wiki::build_index(&ws)
            })
            .await
            {
                Ok(Ok(idx)) => serde_json::to_string(&idx).unwrap_or_else(|_| "{}".into()),
                Ok(Err(e)) => {
                    write_site_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &serde_json::json!({"error": e}).to_string(),
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    write_site_response(
                        &mut stream,
                        "500 Internal Server Error",
                        "application/json",
                        &serde_json::json!({"error": format!("join: {e}")}).to_string(),
                    )
                    .await;
                    return;
                }
            };
            write_site_response(&mut stream, "200 OK", "application/json", &body).await;
        }
        "/api/note" => {
            let id = query_param(query, "id").unwrap_or_default();
            if id.is_empty() {
                write_site_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"Missing id"}"#,
                )
                .await;
                return;
            }
            // Percent-decode minimal id
            let id = percent_decode(&id);
            let body = match tokio::task::spawn_blocking({
                let ws = workspace.clone();
                let id = id.clone();
                move || k2_core::wiki::read_note(&ws, &id)
            })
            .await
            {
                Ok(Ok(note)) => serde_json::to_string(&note).unwrap_or_else(|_| "{}".into()),
                Ok(Err(e)) => {
                    write_site_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &serde_json::json!({"error": e}).to_string(),
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    write_site_response(
                        &mut stream,
                        "500 Internal Server Error",
                        "application/json",
                        &serde_json::json!({"error": format!("join: {e}")}).to_string(),
                    )
                    .await;
                    return;
                }
            };
            write_site_response(&mut stream, "200 OK", "application/json", &body).await;
        }
        "/" | "/index.html" => {
            write_site_response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                SITE_HTML,
            )
            .await;
        }
        _ => {
            write_site_response(
                &mut stream,
                "404 Not Found",
                "application/json",
                r#"{"error":"not found"}"#,
            )
            .await;
        }
    }
}

async fn write_site_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    ct: &str,
    body: &str,
) {
    let resp = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ct}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v);
            }
        } else if pair == key {
            return Some("");
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = || {
                let a = (bytes[i + 1] as char).to_digit(16)?;
                let b = (bytes[i + 2] as char).to_digit(16)?;
                Some((a * 16 + b) as u8)
            };
            if let Some(b) = h() {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Minimal live SPA: list + simple force graph, no CDN deps. Polls /api/index.
///
/// Stability notes (0.40.47+):
/// - Do **not** rebuild the sim when `generatedAt` changes — that timestamp
///   is regenerated every poll even when the graph is unchanged, which used
///   to re-scatter nodes every 2s.
/// - Merge by node id so positions survive polls; only reheat on structure
///   change. Simulation cools down so layout settles instead of thrashing.
const SITE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>K2 Wiki</title>
<style>
  /* K2 dark system tokens (mirrors app --color-* defaults) */
  :root {
    color-scheme: dark;
    --bg: #0a0a0a;
    --panel: #141414;
    --elevated: #1e1e1e;
    --fg: #e4e4e7;
    --secondary: #a1a1aa;
    --muted: #71717a;
    --accent: #3b82f6;
    --accent-soft: #93c5fd;
    --border: #2a2a2a;
    --miss: #f87171;
    --node: #e4e4e7;
  }
  * { box-sizing: border-box; }
  html, body { height:100%; }
  body {
    margin:0;
    font: 13px/1.45 ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, monospace;
    background:var(--bg); color:var(--fg);
    height:100vh; display:flex; flex-direction:column; overflow:hidden;
  }
  header {
    display:flex; gap:12px; align-items:center;
    padding:8px 12px; border-bottom:1px solid var(--border);
    background:var(--panel); flex-shrink:0;
  }
  header h1 {
    font-size:11px; margin:0; font-weight:700;
    letter-spacing:0.12em; text-transform:uppercase; color:var(--muted);
    flex-shrink:0;
  }
  header .search-wrap { display:flex; align-items:center; gap:10px; flex:1; min-width:0; max-width:520px; }
  input[type=search] {
    flex:1; min-width:120px; max-width:280px; padding:6px 10px;
    border:1px solid var(--border); background:var(--elevated); color:var(--fg);
    outline:none; font:inherit;
  }
  input[type=search]:focus { border-color:var(--accent); }
  header .meta {
    display:flex; align-items:center; gap:8px; flex-shrink:0;
    color:var(--muted); font-size:11px; white-space:nowrap;
  }
  header .meta .count-tag {
    display:inline-flex; align-items:center; min-width:1.25rem;
    padding:2px 6px; border:1px solid var(--border); background:rgba(255,255,255,.06);
    color:var(--secondary); font-variant-numeric:tabular-nums; font-size:10px; font-weight:600;
  }
  header .meta .live {
    color:var(--muted); font-size:10px; text-transform:uppercase; letter-spacing:0.06em;
  }
  header .meta .live::before {
    content:''; display:inline-block; width:6px; height:6px; border-radius:50%;
    background:#4ade80; margin-right:5px; vertical-align:middle;
  }
  header .spacer { flex:1; }
  header button#toggle-reader {
    flex-shrink:0; display:inline-flex; align-items:center; gap:5px;
    padding:5px 9px; font:10px/1 inherit; font-weight:600;
    color:var(--secondary); background:transparent;
    border:1px solid var(--border); cursor:pointer;
  }
  header button#toggle-reader:hover { color:var(--fg); border-color:var(--muted); }
  header button#toggle-reader.collapsed {
    border-color:rgba(59,130,246,.5); background:rgba(59,130,246,.12); color:var(--fg);
  }
  header button#toggle-reader svg { width:10px; height:10px; transition:transform .15s linear; }
  header button#toggle-reader:not(.collapsed) svg { transform:rotate(180deg); }
  main { flex:1; display:grid; grid-template-columns: 1fr minmax(280px, 0.9fr); min-height:0; min-width:0; }
  main.reader-collapsed { grid-template-columns: 1fr; }
  main.reader-collapsed #reader { display:none; }
  main.reader-collapsed #graph-wrap { border-right:0; }
  #graph-wrap { position:relative; min-height:0; min-width:0; height:100%; border-right:1px solid var(--border); overflow:hidden; background:var(--bg); }
  canvas { position:absolute; inset:0; width:100%; height:100%; display:block; cursor:grab; touch-action:none; }
  canvas:active { cursor:grabbing; }
  #reader { overflow:auto; padding:14px 16px; background:var(--panel); min-height:0; }
  #reader > h2.title { margin:0 0 8px; font-size:14px; font-weight:600; color:var(--fg); }
  #reader .tags { color:var(--muted); font-size:10px; margin-bottom:12px; text-transform:uppercase; letter-spacing:0.06em; }
  #reader .md { font:12.5px/1.6 ui-monospace, SFMono-Regular, Menlo, monospace; color:var(--secondary); }
  #reader .md > :first-child { margin-top:0; }
  #reader .md h1 { font-size:18px; font-weight:700; color:var(--fg); margin:1.1em 0 0.45em; border-bottom:1px solid var(--border); padding-bottom:0.25em; }
  #reader .md h2 { font-size:15px; font-weight:650; color:var(--fg); margin:1em 0 0.4em; }
  #reader .md h3 { font-size:13px; font-weight:650; color:var(--fg); margin:0.9em 0 0.35em; }
  #reader .md h4, #reader .md h5, #reader .md h6 { font-size:12px; font-weight:600; color:var(--fg); margin:0.8em 0 0.3em; }
  #reader .md p { margin:0.55em 0; }
  #reader .md ul, #reader .md ol { margin:0.45em 0; padding-left:1.35em; }
  #reader .md li { margin:0.2em 0; }
  #reader .md blockquote {
    margin:0.6em 0; padding:0.35em 0 0.35em 0.85em;
    border-left:2px solid var(--border); color:var(--muted);
  }
  #reader .md hr { border:0; border-top:1px solid var(--border); margin:1em 0; }
  #reader .md a { color:var(--accent); text-decoration:underline; text-decoration-color:rgba(59,130,246,.4); }
  #reader .md a:hover { text-decoration-color:var(--accent); }
  #reader .md a.wiki { cursor:pointer; }
  #reader .md code {
    font:0.92em/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
    background:rgba(255,255,255,.06); border:1px solid var(--border);
    padding:0.1em 0.35em; color:var(--accent-soft);
  }
  #reader .md pre {
    margin:0.7em 0; padding:10px 12px; overflow:auto;
    background:var(--bg); border:1px solid var(--border);
    color:var(--fg); line-height:1.45;
  }
  #reader .md pre code { background:none; border:0; padding:0; color:inherit; font-size:11.5px; }
  #reader .md table { border-collapse:collapse; margin:0.7em 0; width:100%; font-size:11.5px; }
  #reader .md th, #reader .md td { border:1px solid var(--border); padding:5px 8px; text-align:left; }
  #reader .md th { background:rgba(255,255,255,.04); color:var(--fg); font-weight:600; }
  #reader .md strong { color:var(--fg); font-weight:650; }
  #reader .empty { color:var(--muted); font-size:11px; }
  #hint {
    position:absolute; right:10px; bottom:10px; z-index:2;
    font-size:10px; color:var(--muted);
    background:rgba(20,20,20,.85); border:1px solid var(--border);
    padding:4px 8px; pointer-events:none;
  }
  @media (max-width:800px) {
    main:not(.reader-collapsed) { grid-template-columns:1fr; grid-template-rows:1fr 1fr; }
    main:not(.reader-collapsed) #graph-wrap { border-right:0; border-bottom:1px solid var(--border); }
  }
</style>
</head>
<body>
<header>
  <h1>K2 Wiki</h1>
  <div class="search-wrap">
    <input type="search" id="q" placeholder="Search notes…" />
    <div class="meta" id="meta">
      <span>Articles</span>
      <span class="count-tag" id="note-count">—</span>
      <span class="live" id="live-pill">live</span>
    </div>
  </div>
  <div class="spacer"></div>
  <button type="button" id="toggle-reader" title="Collapse article viewer">
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M4 2 L8 6 L4 10" stroke-linecap="round" stroke-linejoin="round"/>
    </svg>
    <span id="toggle-reader-label">Hide article</span>
  </button>
</header>
<main id="layout">
  <div id="graph-wrap">
    <canvas id="c"></canvas>
    <div id="hint">scroll = pan · pinch = zoom · drag node</div>
  </div>
  <aside id="reader"><p class="empty">Select a note.</p></aside>
</main>
<script>
(function(){
  const canvas = document.getElementById('c');
  const wrap = document.getElementById('graph-wrap');
  const layout = document.getElementById('layout');
  const ctx = canvas.getContext('2d');
  const reader = document.getElementById('reader');
  const meta = document.getElementById('meta');
  const noteCountEl = document.getElementById('note-count');
  const livePill = document.getElementById('live-pill');
  const q = document.getElementById('q');
  const toggleReaderBtn = document.getElementById('toggle-reader');
  const toggleReaderLabel = document.getElementById('toggle-reader-label');
  let index = { nodes: [], links: [], generatedAt: '', noteCount: 0 };
  let selected = null;
  let readerCollapsed = false;
  let nodes = []; // sim nodes (stable identity by id)
  let links = [];
  let drag = null;
  let lastFp = '';
  /** Force "temperature" — cools to 0 so layout settles instead of thrashing. */
  let alpha = 1;
  let viewW = 600, viewH = 400;
  // Camera: world point (camX, camY) sits at canvas center; camK is zoom.
  let camX = 300, camY = 200, camK = 1;
  // Multi-touch state for pinch + two-finger pan
  const pointers = new Map(); // id -> {x,y} canvas coords
  let pinch0 = null; // {dist, camK, midWx, midWy, camX, camY}

  function fingerprint(idx){
    const ns = (idx.nodes || []).map(n =>
      n.id + '\t' + (n.title||'') + '\t' + (n.exists?1:0) + '\t' + (n.tags||[]).join(',')
    ).sort().join('\n');
    const ls = (idx.links || []).map(l =>
      l.source + '\t' + l.target + '\t' + (l.missing?1:0)
    ).sort().join('\n');
    return (idx.noteCount||0) + '\n' + ns + '\n' + ls;
  }

  function screenToWorld(sx, sy){
    return {
      x: (sx - viewW/2) / camK + camX,
      y: (sy - viewH/2) / camK + camY,
    };
  }

  function canvasCoords(e){
    const rect = canvas.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  }

  function resize(){
    const r = wrap.getBoundingClientRect();
    const w = Math.max(1, Math.floor(r.width));
    const h = Math.max(1, Math.floor(r.height));
    if (w < 2 || h < 2) return;
    const prevW = viewW, prevH = viewH;
    const first = !nodes.length && viewW === 600 && viewH === 400;
    viewW = w; viewH = h;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const bw = Math.max(1, Math.floor(w * dpr));
    const bh = Math.max(1, Math.floor(h * dpr));
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    // First layout: aim camera at sim center.
    if (first || (camX === 300 && camY === 200 && !nodes.length)) {
      camX = w/2; camY = h/2;
    }
    // Proportional reflow of sim positions only (camera stays put).
    if (nodes.length && prevW > 2 && prevH > 2 && (prevW !== w || prevH !== h)) {
      const sx = w / prevW, sy = h / prevH;
      for (const n of nodes) {
        n.x *= sx; n.y *= sy;
        n.vx = 0; n.vy = 0;
      }
      camX *= sx; camY *= sy;
    }
  }
  if (typeof ResizeObserver !== 'undefined') {
    new ResizeObserver(() => resize()).observe(wrap);
  }
  window.addEventListener('resize', resize);
  resize();

  /** Merge index into sim: keep x/y for known ids; place newcomers nearby. */
  function mergeSim(idx){
    const W = viewW, H = viewH;
    const prev = new Map(nodes.map(n => [n.id, n]));
    const map = new Map();
    const next = [];
    const list = idx.nodes || [];
    list.forEach((n, i) => {
      const old = prev.get(n.id);
      let o;
      if (old) {
        o = old;
        o.title = n.title || n.id;
        o.exists = !!n.exists;
        o.tags = n.tags || [];
      } else {
        const angle = (i / Math.max(1, list.length)) * Math.PI * 2;
        const rad = Math.min(W, H) * 0.28;
        o = {
          id: n.id, title: n.title || n.id, exists: !!n.exists,
          tags: n.tags || [],
          x: W/2 + Math.cos(angle)*rad,
          y: H/2 + Math.sin(angle)*rad,
          vx:0, vy:0
        };
      }
      map.set(n.id, o);
      next.push(o);
    });
    nodes = next;
    links = (idx.links || []).map(l => ({
      source: map.get(l.source), target: map.get(l.target), missing: !!l.missing
    })).filter(l => l.source && l.target);
    alpha = Math.max(alpha, 0.55);
  }

  function step(){
    if (alpha < 0.01) return;
    const W = viewW, H = viewH;
    const a = alpha;
    for (let i=0;i<nodes.length;i++){
      for (let j=i+1;j<nodes.length;j++){
        let dx = nodes[j].x - nodes[i].x;
        let dy = nodes[j].y - nodes[i].y;
        let d2 = dx*dx + dy*dy + 0.01;
        let f = (600 * a) / d2;
        let d = Math.sqrt(d2);
        dx/=d; dy/=d;
        nodes[i].vx -= f*dx; nodes[i].vy -= f*dy;
        nodes[j].vx += f*dx; nodes[j].vy += f*dy;
      }
    }
    for (const l of links){
      let dx = l.target.x - l.source.x;
      let dy = l.target.y - l.source.y;
      let d = Math.sqrt(dx*dx+dy*dy) || 1;
      let f = (d - 90) * 0.015 * a;
      dx/=d; dy/=d;
      l.source.vx += f*dx; l.source.vy += f*dy;
      l.target.vx -= f*dx; l.target.vy -= f*dy;
    }
    // Layout gravity toward world origin of the sim (view center at spawn).
    // Do NOT clamp to viewport — camera pans freely over the world.
    for (const n of nodes){
      if (drag === n) continue;
      n.vx += (W/2 - n.x) * 0.0015 * a;
      n.vy += (H/2 - n.y) * 0.0015 * a;
      n.vx *= 0.82; n.vy *= 0.82;
      n.x += n.vx; n.y += n.vy;
    }
    alpha *= 0.985;
    if (alpha < 0.01) alpha = 0;
  }

  // Match in-app WikiGraph label visibility (pixel diameter of the node).
  const NODE_R = 5;
  const NODE_R_SEL = 6.5;
  const LABEL_FADE_START_PX = 20; // labels start fading in
  const LABEL_MIN_PX = 25;        // full opacity
  /** Soft blue for Home when not selected (matches app HOME_SOFT_BLUE). */
  const HOME_SOFT = '#8fa8ff';

  function labelOpacityForDiameter(diameterPx){
    if (diameterPx <= LABEL_FADE_START_PX) return 0;
    if (diameterPx >= LABEL_MIN_PX) return 1;
    const t = (diameterPx - LABEL_FADE_START_PX) / (LABEL_MIN_PX - LABEL_FADE_START_PX);
    return t * t * (3 - 2 * t); // smoothstep
  }

  /** Match app isWikiHomeNode — real Home note, not missing stubs. */
  function isHomeNode(n){
    if (!n || !n.exists) return false;
    const id = String(n.id || '');
    const title = String(n.title || '').trim().toLowerCase();
    const bare = id.includes('::') ? (id.split('::').pop() || id) : id;
    if (bare.toLowerCase() === 'home.md') return true;
    if (title === 'home') return true;
    return false;
  }

  function draw(){
    const W = viewW, H = viewH;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0,0,W,H);
    ctx.save();
    // Camera: world (camX,camY) → canvas center, scaled by camK
    ctx.translate(W/2, H/2);
    ctx.scale(camK, camK);
    ctx.translate(-camX, -camY);

    for (const l of links){
      ctx.beginPath();
      ctx.strokeStyle = l.missing ? 'rgba(248,113,113,.4)' : 'rgba(42,42,42,.95)';
      ctx.lineWidth = 1 / camK;
      ctx.moveTo(l.source.x, l.source.y);
      ctx.lineTo(l.target.x, l.target.y);
      ctx.stroke();
    }
    // On-screen diameter of a default node at current zoom (matches app math:
    // 2 * r_graph * globalScale).
    const diameterPx = 2 * NODE_R * camK;
    const zoomLabelA = labelOpacityForDiameter(diameterPx);
    const qq = (q.value || '').toLowerCase();
    for (const n of nodes){
      if (qq && !n.title.toLowerCase().includes(qq) && !n.id.toLowerCase().includes(qq)
          && !(n.tags||[]).some(t => String(t).toLowerCase().includes(qq))) continue;
      const sel = selected === n.id;
      const home = isHomeNode(n);
      const r = sel ? NODE_R_SEL : NODE_R;
      ctx.beginPath();
      if (!n.exists) {
        ctx.fillStyle = '#f87171';
        ctx.globalAlpha = 0.55;
      } else if (sel) {
        ctx.fillStyle = '#3b82f6';
        ctx.globalAlpha = 1;
      } else if (home) {
        // Deselected Home stays findable as a lighter blue (in-app UX).
        ctx.fillStyle = HOME_SOFT;
        ctx.globalAlpha = 0.9;
      } else {
        ctx.fillStyle = '#e4e4e7';
        ctx.globalAlpha = 0.88;
      }
      ctx.arc(n.x, n.y, r, 0, Math.PI*2);
      ctx.fill();
      ctx.globalAlpha = 1;
      if (sel || home) {
        ctx.strokeStyle = sel ? '#3b82f6' : HOME_SOFT;
        ctx.lineWidth = (sel ? 1.5 : 1.1) / camK;
        ctx.globalAlpha = sel ? 1 : 0.65;
        ctx.beginPath();
        ctx.arc(n.x, n.y, r + 3/camK, 0, Math.PI*2);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
      // Labels: always on for selection + Home; otherwise fade by zoom.
      const labelA = (sel || home) ? 1 : zoomLabelA;
      if (labelA <= 0.01) continue;
      ctx.fillStyle = sel ? '#93c5fd' : (home ? HOME_SOFT : '#a1a1aa');
      // Keep label roughly constant on-screen (world font = screen_px / camK).
      const fontPx = Math.max(11 / camK, 2.8 / camK);
      ctx.font = fontPx + 'px ui-monospace, Menlo, monospace';
      ctx.globalAlpha = (sel || home ? 1 : 0.9) * labelA * (!n.exists ? 0.55 : 1);
      ctx.fillText(n.title, n.x + r + 4/camK, n.y + 3/camK);
      ctx.globalAlpha = 1;
    }
    ctx.restore();
  }

  function loop(){ step(); draw(); requestAnimationFrame(loop); }
  requestAnimationFrame(loop);

  // ── Pan / pinch (match in-app WikiGraph: wheel=pan, ctrl+wheel=zoom) ──
  wrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const { x: sx, y: sy } = canvasCoords(e);
    if (e.ctrlKey || e.metaKey) {
      // Pinch (browsers synthesize ctrl+wheel)
      const world = screenToWorld(sx, sy);
      const factor = Math.exp(-e.deltaY * 0.01);
      const next = Math.min(8, Math.max(0.15, camK * factor));
      camK = next;
      camX = world.x - (sx - viewW/2) / camK;
      camY = world.y - (sy - viewH/2) / camK;
      return;
    }
    // Two-finger trackpad / mouse wheel → pan in world space
    camX += e.deltaX / camK;
    camY += e.deltaY / camK;
  }, { passive: false });

  function setReaderCollapsed(collapsed){
    readerCollapsed = !!collapsed;
    layout.classList.toggle('reader-collapsed', readerCollapsed);
    toggleReaderBtn.classList.toggle('collapsed', readerCollapsed);
    toggleReaderLabel.textContent = readerCollapsed ? 'Show article' : 'Hide article';
    toggleReaderBtn.title = readerCollapsed ? 'Show article viewer' : 'Collapse article viewer';
    // Let layout settle, then remeasure canvas.
    requestAnimationFrame(() => { resize(); });
  }
  toggleReaderBtn.addEventListener('click', () => setReaderCollapsed(!readerCollapsed));

  function escapeHtml(s){
    return String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  /** Lightweight Markdown → HTML (no CDN). Supports common GFM-ish bits + [[wikilinks]]. */
  function renderMarkdown(src){
    let text = String(src || '').replace(/\r\n/g, '\n');
    // Protect fenced code blocks
    const fences = [];
    text = text.replace(/```([a-zA-Z0-9_-]*)\n([\s\S]*?)```/g, (_, lang, code) => {
      const i = fences.length;
      fences.push({ lang: lang || '', code });
      return `\u0000FENCE${i}\u0000`;
    });
    // Protect inline code
    const inlines = [];
    text = text.replace(/`([^`\n]+)`/g, (_, code) => {
      const i = inlines.length;
      inlines.push(code);
      return `\u0000CODE${i}\u0000`;
    });

    const lines = text.split('\n');
    const out = [];
    let i = 0;
    let inUl = false, inOl = false, inBq = false;

    function closeLists(){
      if (inUl) { out.push('</ul>'); inUl = false; }
      if (inOl) { out.push('</ol>'); inOl = false; }
    }
    function closeBq(){
      if (inBq) { out.push('</blockquote>'); inBq = false; }
    }
    function inlineFmt(s){
      let t = escapeHtml(s);
      // [[wikilink]] or [[title|alias]]
      t = t.replace(/\[\[([^\]|#]+)(?:\|([^\]]+))?\]\]/g, (_, target, label) => {
        const title = target.trim();
        const lab = (label || title).trim();
        return `<a class="wiki" data-title="${escapeHtml(title)}">${escapeHtml(lab)}</a>`;
      });
      // [text](url)
      t = t.replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, (_, lab, url) =>
        `<a href="${escapeHtml(url)}" target="_blank" rel="noreferrer">${lab}</a>`
      );
      // bold / italic (bold first; no lookbehind for older engines)
      t = t.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
      t = t.replace(/__([^_]+)__/g, '<strong>$1</strong>');
      t = t.replace(/\*([^*]+)\*/g, '<em>$1</em>');
      t = t.replace(/_([^_]+)_/g, '<em>$1</em>');
      // restore inline code
      t = t.replace(/\u0000CODE(\d+)\u0000/g, (_, n) =>
        `<code>${escapeHtml(inlines[Number(n)])}</code>`
      );
      return t;
    }

    while (i < lines.length) {
      const line = lines[i];
      // table block
      if (/^\s*\|.+\|\s*$/.test(line) && i + 1 < lines.length && /^\s*\|?\s*:?-{3,}/.test(lines[i+1])) {
        closeLists(); closeBq();
        const rows = [];
        while (i < lines.length && /^\s*\|/.test(lines[i])) {
          rows.push(lines[i]);
          i++;
        }
        // drop separator row
        const bodyRows = rows.filter((r, idx) => idx === 0 || !/^\s*\|?\s*:?-{3,}/.test(r));
        const cells = (r) => r.replace(/^\s*\|/, '').replace(/\|\s*$/, '').split('|').map(c => c.trim());
        if (bodyRows.length) {
          out.push('<table><thead><tr>');
          cells(bodyRows[0]).forEach(c => out.push(`<th>${inlineFmt(c)}</th>`));
          out.push('</tr></thead><tbody>');
          for (let r = 1; r < bodyRows.length; r++) {
            out.push('<tr>');
            cells(bodyRows[r]).forEach(c => out.push(`<td>${inlineFmt(c)}</td>`));
            out.push('</tr>');
          }
          out.push('</tbody></table>');
        }
        continue;
      }
      // fence placeholder alone
      const fenceM = line.match(/^\u0000FENCE(\d+)\u0000$/);
      if (fenceM) {
        closeLists(); closeBq();
        const f = fences[Number(fenceM[1])];
        out.push(`<pre><code>${escapeHtml(f.code.replace(/\n$/, ''))}</code></pre>`);
        i++; continue;
      }
      if (/^\s*---+\s*$/.test(line) || /^\s*\*\*\*+\s*$/.test(line)) {
        closeLists(); closeBq();
        out.push('<hr/>'); i++; continue;
      }
      const hm = line.match(/^(#{1,6})\s+(.+)$/);
      if (hm) {
        closeLists(); closeBq();
        const lvl = hm[1].length;
        out.push(`<h${lvl}>${inlineFmt(hm[2].trim())}</h${lvl}>`);
        i++; continue;
      }
      const bq = line.match(/^\s*>\s?(.*)$/);
      if (bq) {
        closeLists();
        if (!inBq) { out.push('<blockquote>'); inBq = true; }
        out.push(`<p>${inlineFmt(bq[1])}</p>`);
        i++; continue;
      } else {
        closeBq();
      }
      const ul = line.match(/^\s*[-*+]\s+(.+)$/);
      if (ul) {
        closeBq();
        if (inOl) { out.push('</ol>'); inOl = false; }
        if (!inUl) { out.push('<ul>'); inUl = true; }
        out.push(`<li>${inlineFmt(ul[1])}</li>`);
        i++; continue;
      }
      const ol = line.match(/^\s*\d+\.\s+(.+)$/);
      if (ol) {
        closeBq();
        if (inUl) { out.push('</ul>'); inUl = false; }
        if (!inOl) { out.push('<ol>'); inOl = true; }
        out.push(`<li>${inlineFmt(ol[1])}</li>`);
        i++; continue;
      }
      if (/^\s*$/.test(line)) {
        closeLists(); closeBq();
        i++; continue;
      }
      closeLists(); closeBq();
      // paragraph — merge soft-wrapped lines
      let para = line;
      while (i + 1 < lines.length && !/^\s*$/.test(lines[i+1])
        && !/^(#{1,6})\s/.test(lines[i+1])
        && !/^\s*[-*+]\s/.test(lines[i+1])
        && !/^\s*\d+\.\s/.test(lines[i+1])
        && !/^\s*>/.test(lines[i+1])
        && !/^\u0000FENCE/.test(lines[i+1])
        && !/^\s*\|/.test(lines[i+1])
        && !/^\s*---+\s*$/.test(lines[i+1])) {
        i++;
        para += ' ' + lines[i].trim();
      }
      out.push(`<p>${inlineFmt(para)}</p>`);
      i++;
    }
    closeLists(); closeBq();
    // any leftover fence tokens in paragraphs
    let html = out.join('\n');
    html = html.replace(/\u0000FENCE(\d+)\u0000/g, (_, n) => {
      const f = fences[Number(n)];
      return `<pre><code>${escapeHtml(f.code.replace(/\n$/, ''))}</code></pre>`;
    });
    return html;
  }

  function wireWikiLinks(root){
    root.querySelectorAll('a.wiki').forEach(a => {
      a.onclick = (e) => {
        e.preventDefault();
        const title = a.dataset.title;
        const hit = (index.nodes||[]).find(n =>
          (n.title||'').toLowerCase() === title.toLowerCase()
          || (n.id||'').toLowerCase() === title.toLowerCase()
          || (n.aliases||[]).some(x => String(x).toLowerCase() === title.toLowerCase())
        );
        if (hit) selectNote(hit.id);
        else selectNote('missing:' + title);
      };
    });
  }

  async function selectNote(id){
    selected = id;
    // If the reader was collapsed, keep it collapsed — selection still works on the map.
    if (String(id).startsWith('missing:')) {
      reader.innerHTML = `<h2 class="title">${escapeHtml(id.slice(8))}</h2><p class="empty">Note does not exist yet.</p>`;
      return;
    }
    try {
      const r = await fetch('/api/note?id=' + encodeURIComponent(id));
      const j = await r.json();
      if (j.error) {
        reader.innerHTML = `<p class="empty">${escapeHtml(j.error)}</p>`;
        return;
      }
      const tags = (j.tags||[]).map(t => '#'+t).join(' ');
      reader.innerHTML =
        `<h2 class="title">${escapeHtml(j.title||j.id)}</h2>` +
        (tags ? `<div class="tags">${escapeHtml(tags)}</div>` : '') +
        `<div class="md">${renderMarkdown(j.body||'')}</div>`;
      wireWikiLinks(reader);
    } catch (e) {
      reader.innerHTML = `<p class="empty">${escapeHtml(String(e))}</p>`;
    }
  }

  function hitNode(sx, sy){
    const w = screenToWorld(sx, sy);
    let best = null, bd = 14 / camK;
    for (const n of nodes){
      const d = Math.hypot(n.x - w.x, n.y - w.y);
      if (d < bd) { bd = d; best = n; }
    }
    return best;
  }

  canvas.addEventListener('pointerdown', e => {
    const { x: sx, y: sy } = canvasCoords(e);
    pointers.set(e.pointerId, { x: sx, y: sy });
    if (pointers.size === 2) {
      // Start pinch / two-finger pan
      drag = null;
      const pts = [...pointers.values()];
      const dx = pts[1].x - pts[0].x, dy = pts[1].y - pts[0].y;
      const dist = Math.hypot(dx, dy) || 1;
      const mid = { x: (pts[0].x + pts[1].x)/2, y: (pts[0].y + pts[1].y)/2 };
      const world = screenToWorld(mid.x, mid.y);
      pinch0 = { dist, camK, midWx: world.x, midWy: world.y, camX, camY };
      return;
    }
    if (pointers.size === 1) {
      const best = hitNode(sx, sy);
      if (best) {
        drag = best;
        selectNote(best.id);
        canvas.setPointerCapture(e.pointerId);
      }
    }
  });
  canvas.addEventListener('pointermove', e => {
    if (!pointers.has(e.pointerId)) return;
    const { x: sx, y: sy } = canvasCoords(e);
    pointers.set(e.pointerId, { x: sx, y: sy });
    if (pointers.size >= 2 && pinch0) {
      const pts = [...pointers.values()];
      const dx = pts[1].x - pts[0].x, dy = pts[1].y - pts[0].y;
      const dist = Math.hypot(dx, dy) || 1;
      const mid = { x: (pts[0].x + pts[1].x)/2, y: (pts[0].y + pts[1].y)/2 };
      const nextK = Math.min(8, Math.max(0.15, pinch0.camK * (dist / pinch0.dist)));
      camK = nextK;
      // Keep midpoint world point under the fingers; also allow pan via mid movement
      camX = pinch0.midWx - (mid.x - viewW/2) / camK;
      camY = pinch0.midWy - (mid.y - viewH/2) / camK;
      return;
    }
    if (drag && pointers.size === 1) {
      const w = screenToWorld(sx, sy);
      drag.x = w.x; drag.y = w.y;
      drag.vx = drag.vy = 0;
    }
  });
  function endPointer(e){
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch0 = null;
    if (pointers.size === 0) drag = null;
  }
  canvas.addEventListener('pointerup', endPointer);
  canvas.addEventListener('pointercancel', endPointer);

  // Search filters which labels/nodes are drawn (see draw()).
  q.addEventListener('input', () => { /* redraw uses q.value live */ });

  async function refresh(){
    try {
      const r = await fetch('/api/index');
      const j = await r.json();
      if (j.error) { if (meta) meta.title = j.error; return; }
      index = j;
      if (noteCountEl) noteCountEl.textContent = String(j.noteCount || 0);
      if (livePill) livePill.style.opacity = '1';
      const fp = fingerprint(j);
      if (fp !== lastFp) {
        lastFp = fp;
        mergeSim(j);
        if (!selected) {
          const first = (j.nodes||[]).find(n => n.exists);
          if (first) selectNote(first.id);
        }
      }
    } catch (e) {
      if (noteCountEl) noteCountEl.textContent = '—';
      if (livePill) { livePill.textContent = 'offline'; livePill.style.opacity = '0.6'; }
      if (meta) meta.title = String(e);
    }
  }
  refresh();
  setInterval(refresh, 3000);
})();
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_ws(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-wiki-routes-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn index_and_seed_handlers() {
        let ws = temp_ws("seed");
        let mut params = HashMap::new();
        params.insert("project".into(), ws.to_string_lossy().into_owned());

        let empty = handle_index(&params);
        assert_eq!(empty.status, "200 OK");
        assert!(empty.body.contains("\"noteCount\":0") || empty.body.contains("\"note_count\":0") || empty.body.contains("noteCount"));

        let seeded = handle_seed_post(&params);
        assert_eq!(seeded.status, "200 OK", "{}", seeded.body);
        assert!(seeded.body.contains("Home.md"));

        let idx = handle_index(&params);
        assert_eq!(idx.status, "200 OK");
        assert!(idx.body.contains("Home.md"));

        let mut note_params = params.clone();
        note_params.insert("id".into(), "Home.md".into());
        let note = handle_note(&note_params);
        assert_eq!(note.status, "200 OK", "{}", note.body);
        assert!(note.body.contains("Knowledge Base") || note.body.contains("body"));

        let st = handle_serve_status(&params);
        assert_eq!(st.status, "200 OK");
        assert!(st.body.contains("\"enabled\":false") || st.body.contains("enabled"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("Home.md"), "Home.md");
        assert_eq!(percent_decode("Feature%20-%20X.md"), "Feature - X.md");
        assert_eq!(percent_decode("a+b"), "a b");
    }
}
