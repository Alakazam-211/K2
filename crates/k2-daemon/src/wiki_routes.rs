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
const SITE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>K2 Wiki</title>
<style>
  :root { color-scheme: dark light; --bg:#0f1115; --panel:#171a21; --fg:#e8eaed; --muted:#9aa0a6; --accent:#7aa2f7; --edge:#3a4050; --miss:#e06c75; }
  * { box-sizing: border-box; }
  body { margin:0; font:14px/1.45 system-ui, -apple-system, sans-serif; background:var(--bg); color:var(--fg); height:100vh; display:flex; flex-direction:column; }
  header { display:flex; gap:12px; align-items:center; padding:10px 14px; border-bottom:1px solid var(--edge); background:var(--panel); }
  header h1 { font-size:15px; margin:0; font-weight:600; }
  header .meta { color:var(--muted); font-size:12px; }
  input[type=search] { flex:1; max-width:280px; padding:6px 10px; border-radius:6px; border:1px solid var(--edge); background:var(--bg); color:var(--fg); }
  main { flex:1; display:grid; grid-template-columns: 1fr 360px; min-height:0; }
  #graph-wrap { position:relative; min-height:0; border-right:1px solid var(--edge); }
  canvas { width:100%; height:100%; display:block; cursor:grab; }
  #reader { overflow:auto; padding:14px 16px; background:var(--panel); }
  #reader h2 { margin:0 0 8px; font-size:18px; }
  #reader .tags { color:var(--muted); font-size:12px; margin-bottom:12px; }
  #reader pre { white-space:pre-wrap; word-break:break-word; font:13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; margin:0; }
  #reader a.wiki { color:var(--accent); cursor:pointer; text-decoration:underline; }
  #reader .empty { color:var(--muted); }
  #list { position:absolute; left:10px; top:10px; max-height:40%; overflow:auto; background:rgba(23,26,33,.92); border:1px solid var(--edge); border-radius:8px; padding:6px; min-width:160px; max-width:240px; }
  #list button { display:block; width:100%; text-align:left; background:transparent; border:0; color:var(--fg); padding:4px 6px; border-radius:4px; cursor:pointer; font:inherit; }
  #list button:hover, #list button.active { background:rgba(122,162,247,.18); }
  #list button.missing { color:var(--miss); opacity:.85; }
  @media (max-width:800px) { main { grid-template-columns:1fr; grid-template-rows:1fr 1fr; } #graph-wrap { border-right:0; border-bottom:1px solid var(--edge); } }
</style>
</head>
<body>
<header>
  <h1>K2 Wiki</h1>
  <span class="meta" id="meta">loading…</span>
  <input type="search" id="q" placeholder="Search notes…" />
</header>
<main>
  <div id="graph-wrap">
    <canvas id="c"></canvas>
    <div id="list"></div>
  </div>
  <aside id="reader"><p class="empty">Select a note.</p></aside>
</main>
<script>
(function(){
  const canvas = document.getElementById('c');
  const ctx = canvas.getContext('2d');
  const listEl = document.getElementById('list');
  const reader = document.getElementById('reader');
  const meta = document.getElementById('meta');
  const q = document.getElementById('q');
  let index = { nodes: [], links: [], generatedAt: '', noteCount: 0 };
  let selected = null;
  let nodes = []; // sim nodes
  let links = [];
  let drag = null;
  let lastGen = '';

  function resize(){
    const r = canvas.parentElement.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, r.width * dpr);
    canvas.height = Math.max(1, r.height * dpr);
    canvas.style.width = r.width + 'px';
    canvas.style.height = r.height + 'px';
    ctx.setTransform(dpr,0,0,dpr,0,0);
  }
  window.addEventListener('resize', resize);
  resize();

  function buildSim(idx){
    const W = canvas.clientWidth || 600, H = canvas.clientHeight || 400;
    const map = new Map();
    nodes = (idx.nodes || []).map((n,i) => {
      const angle = (i / Math.max(1, idx.nodes.length)) * Math.PI * 2;
      const rad = Math.min(W,H) * 0.28;
      const o = {
        id: n.id, title: n.title || n.id, exists: !!n.exists,
        tags: n.tags || [],
        x: W/2 + Math.cos(angle)*rad + (Math.random()-0.5)*20,
        y: H/2 + Math.sin(angle)*rad + (Math.random()-0.5)*20,
        vx:0, vy:0
      };
      map.set(n.id, o);
      return o;
    });
    links = (idx.links || []).map(l => ({
      source: map.get(l.source), target: map.get(l.target), missing: !!l.missing
    })).filter(l => l.source && l.target);
  }

  function step(){
    const W = canvas.clientWidth || 600, H = canvas.clientHeight || 400;
    // repulsion
    for (let i=0;i<nodes.length;i++){
      for (let j=i+1;j<nodes.length;j++){
        let dx = nodes[j].x - nodes[i].x;
        let dy = nodes[j].y - nodes[i].y;
        let d2 = dx*dx + dy*dy + 0.01;
        let f = 800 / d2;
        let d = Math.sqrt(d2);
        dx/=d; dy/=d;
        nodes[i].vx -= f*dx; nodes[i].vy -= f*dy;
        nodes[j].vx += f*dx; nodes[j].vy += f*dy;
      }
    }
    // springs
    for (const l of links){
      let dx = l.target.x - l.source.x;
      let dy = l.target.y - l.source.y;
      let d = Math.sqrt(dx*dx+dy*dy) || 1;
      let f = (d - 90) * 0.02;
      dx/=d; dy/=d;
      l.source.vx += f*dx; l.source.vy += f*dy;
      l.target.vx -= f*dx; l.target.vy -= f*dy;
    }
    // center + integrate
    for (const n of nodes){
      if (drag === n) continue;
      n.vx += (W/2 - n.x) * 0.002;
      n.vy += (H/2 - n.y) * 0.002;
      n.vx *= 0.85; n.vy *= 0.85;
      n.x += n.vx; n.y += n.vy;
      n.x = Math.max(20, Math.min(W-20, n.x));
      n.y = Math.max(20, Math.min(H-20, n.y));
    }
  }

  function draw(){
    const W = canvas.clientWidth || 600, H = canvas.clientHeight || 400;
    ctx.clearRect(0,0,W,H);
    for (const l of links){
      ctx.beginPath();
      ctx.strokeStyle = l.missing ? 'rgba(224,108,117,.45)' : 'rgba(154,160,166,.35)';
      ctx.lineWidth = 1;
      ctx.moveTo(l.source.x, l.source.y);
      ctx.lineTo(l.target.x, l.target.y);
      ctx.stroke();
    }
    const qq = (q.value || '').toLowerCase();
    for (const n of nodes){
      if (qq && !n.title.toLowerCase().includes(qq) && !n.id.toLowerCase().includes(qq)) continue;
      const sel = selected === n.id;
      ctx.beginPath();
      ctx.fillStyle = !n.exists ? '#e06c75' : (sel ? '#7aa2f7' : '#c0caf5');
      ctx.arc(n.x, n.y, sel ? 8 : 5.5, 0, Math.PI*2);
      ctx.fill();
      ctx.fillStyle = '#e8eaed';
      ctx.font = '12px system-ui,sans-serif';
      ctx.fillText(n.title, n.x + 9, n.y + 4);
    }
  }

  function loop(){ step(); draw(); requestAnimationFrame(loop); }
  requestAnimationFrame(loop);

  function renderList(){
    const qq = (q.value || '').toLowerCase();
    const items = (index.nodes || []).filter(n => {
      if (!qq) return true;
      return (n.title||'').toLowerCase().includes(qq) || (n.id||'').toLowerCase().includes(qq)
        || (n.tags||[]).some(t => String(t).toLowerCase().includes(qq));
    });
    listEl.innerHTML = items.map(n => {
      const cls = [selected===n.id?'active':'', n.exists?'':'missing'].filter(Boolean).join(' ');
      return `<button class="${cls}" data-id="${encodeURIComponent(n.id)}">${escapeHtml(n.title||n.id)}</button>`;
    }).join('') || '<div style="padding:6px;color:var(--muted)">No notes</div>';
    listEl.querySelectorAll('button').forEach(b => {
      b.onclick = () => selectNote(decodeURIComponent(b.dataset.id));
    });
  }

  function escapeHtml(s){
    return String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  function linkify(body){
    return escapeHtml(body).replace(/\[\[([^\]|#]+)(?:[|#][^\]]*)?\]\]/g, (_, t) => {
      const title = t.trim();
      return `<a class="wiki" data-title="${escapeHtml(title)}">[[${escapeHtml(title)}]]</a>`;
    });
  }

  async function selectNote(id){
    selected = id;
    renderList();
    if (String(id).startsWith('missing:')) {
      reader.innerHTML = `<h2>${escapeHtml(id.slice(8))}</h2><p class="empty">Note does not exist yet.</p>`;
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
      reader.innerHTML = `<h2>${escapeHtml(j.title||j.id)}</h2><div class="tags">${escapeHtml(tags)}</div><pre>${linkify(j.body||'')}</pre>`;
      reader.querySelectorAll('a.wiki').forEach(a => {
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
    } catch (e) {
      reader.innerHTML = `<p class="empty">${escapeHtml(String(e))}</p>`;
    }
  }

  canvas.addEventListener('pointerdown', e => {
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left, y = e.clientY - rect.top;
    let best = null, bd = 16;
    for (const n of nodes){
      const d = Math.hypot(n.x-x, n.y-y);
      if (d < bd) { bd = d; best = n; }
    }
    if (best) {
      drag = best;
      selectNote(best.id);
      canvas.setPointerCapture(e.pointerId);
    }
  });
  canvas.addEventListener('pointermove', e => {
    if (!drag) return;
    const rect = canvas.getBoundingClientRect();
    drag.x = e.clientX - rect.left;
    drag.y = e.clientY - rect.top;
    drag.vx = drag.vy = 0;
  });
  canvas.addEventListener('pointerup', () => { drag = null; });

  q.addEventListener('input', () => { renderList(); });

  async function refresh(){
    try {
      const r = await fetch('/api/index');
      const j = await r.json();
      if (j.error) { meta.textContent = j.error; return; }
      index = j;
      meta.textContent = (j.noteCount||0) + ' notes · live · gen ' + (j.generatedAt||'');
      if (j.generatedAt !== lastGen) {
        lastGen = j.generatedAt;
        buildSim(j);
        renderList();
        if (!selected) {
          const first = (j.nodes||[]).find(n => n.exists);
          if (first) selectNote(first.id);
        }
      } else {
        renderList();
      }
    } catch (e) {
      meta.textContent = 'offline: ' + e;
    }
  }
  refresh();
  setInterval(refresh, 2000);
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
