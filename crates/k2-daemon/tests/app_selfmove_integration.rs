//! An agent moves its own legacy `k2 publish run --cmd` service to an App
//! by itself (0.46.3, Rosson 2026-10-20: "he can migrate how his current
//! app works into the runtime"), end to end through the REAL dispatcher on
//! real legacy services (a python HTTP server per test):
//!
//! - `k2 app label take|return`: the hostname moves from the legacy
//!   service to a healthy App of the same workspace and back (one write to
//!   a fake K2 Connect control plane each way); an unhealthy App is
//!   refused; another workspace's label is refused; the legacy service
//!   keeps running, unrouted, the whole time;
//! - `k2 app data put`: workspace files land in the App's data folder;
//!   outside the workspace is refused; a switch-off agent is held;
//! - new Apps default to a container even where the owner opted into the
//!   process engine;
//! - no cut-over is set and nothing enforces one (Rosson 2026-10-26).
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB, a fake control plane
//! on loopback (`K2_CONNECT_BASE`), no real agent CLI (alerts to a sink).

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use app_fixture::*;
use k2_core::test_env::EnvVar;
use serde_json::{json, Value};

fn quiet_alerts() {
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
}

// ── a fake K2 Connect control plane (/subdomains) ────────────────────

type Map = Arc<Mutex<HashMap<String, String>>>;

/// GET /subdomains, POST /subdomains {label,target}, PUT
/// /subdomains/<label> {target}, DELETE /subdomains/<label>. Every write
/// is recorded.
struct FakeCp {
    port: u16,
    map: Map,
    writes: Arc<Mutex<Vec<String>>>,
}

fn fake_cp(primary: &str) -> FakeCp {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let map: Map = Arc::default();
    let writes: Arc<Mutex<Vec<String>>> = Arc::default();
    let (m, w, primary) = (map.clone(), writes.clone(), primary.to_string());
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (m, w, primary) = (m.clone(), w.clone(), primary.clone());
            std::thread::spawn(move || serve(s, &m, &w, &primary));
        }
    });
    FakeCp { port, map, writes }
}

fn serve(mut s: std::net::TcpStream, map: &Map, writes: &Arc<Mutex<Vec<String>>>, primary: &str) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head, body) = loop {
        let n = s.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_string();
            let len: usize = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                .unwrap_or(0);
            while buf.len() < i + 4 + len {
                let n = s.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            break (head, String::from_utf8_lossy(&buf[i + 4..]).to_string());
        }
    };
    let line = head.lines().next().unwrap_or("").to_string();
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let (status, out) = match (method, path) {
        ("GET", "/subdomains") => {
            let m = map.lock().unwrap();
            let mut rows = vec![json!({"label": primary, "target": "", "primary": true})];
            rows.extend(m.iter().map(|(l, t)| json!({"label": l, "target": t, "primary": false})));
            (200, json!({ "subdomains": rows }))
        }
        ("POST", "/subdomains") => {
            let label = v["label"].as_str().unwrap_or("").to_string();
            let mut m = map.lock().unwrap();
            if m.contains_key(&label) {
                (409, json!({"error": "label_taken"}))
            } else {
                m.insert(label.clone(), v["target"].as_str().unwrap_or("").to_string());
                writes.lock().unwrap().push(format!("create {label} {}", v["target"].as_str().unwrap_or("")));
                (200, json!({"ok": true}))
            }
        }
        ("PUT", p) if p.starts_with("/subdomains/") => {
            let label = p.trim_start_matches("/subdomains/").to_string();
            map.lock().unwrap().insert(label.clone(), v["target"].as_str().unwrap_or("").to_string());
            writes.lock().unwrap().push(format!("point {label} {}", v["target"].as_str().unwrap_or("")));
            (200, json!({"ok": true}))
        }
        ("DELETE", p) if p.starts_with("/subdomains/") => {
            let label = p.trim_start_matches("/subdomains/").to_string();
            map.lock().unwrap().remove(&label);
            writes.lock().unwrap().push(format!("delete {label}"));
            (200, json!({"ok": true}))
        }
        _ => (404, json!({"error": "not_found"})),
    };
    let b = out.to_string();
    let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()).as_bytes());
}

// ── fixtures ─────────────────────────────────────────────────────────

/// The legacy service: `GET /` answers `legacy <tag>`; `/env` lists the
/// names it got; `/call` calls K2 with the owner token from `token.txt`.
const LEGACY_PY: &str = r#"import http.server, os, sys, socketserver, urllib.request, urllib.error
D = os.path.dirname(os.path.abspath(__file__))
TAG = open(os.path.join(D, "tag.txt")).read().strip()
PORT = int(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_GET(self):
        if self.path == "/env":
            body = "\n".join(sorted(os.environ.keys()))
        elif self.path == "/call":
            tok = open(os.path.join(D, "token.txt")).read().strip()
            kport = open(os.path.join(D, "kport.txt")).read().strip()
            try:
                body = str(urllib.request.urlopen("http://127.0.0.1:%s/cli/app/keys?token=%s" % (kport, tok)).status)
            except urllib.error.HTTPError as e:
                body = str(e.code)
        else:
            body = "legacy " + TAG
        b = (body + "\n").encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
S(("127.0.0.1", PORT), H).serve_forever()
"#;

/// A port the App binds later: unique, outside the ephemeral range (not
/// `bind(:0)`-then-drop, which another test can be handed meanwhile).
fn free_port() -> u16 {
    k2_core::test_env::loopback_test_port()
}

struct Legacy {
    id: String,
    port: u16,
    folder: PathBuf,
}

/// A real legacy `--cmd` service (the owner's `k2 publish run`; this
/// harness has no open path, so it is a plain process) on the inherited
/// environment, like a pre-vault service.
async fn legacy(d: u16, ws: &Ws, name: &str, tag: &str) -> Legacy {
    let folder = ws.path.join(format!("{name}-legacy"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("server.py"), LEGACY_PY).unwrap();
    std::fs::write(folder.join("tag.txt"), tag).unwrap();
    let port = free_port();
    let r = post(d, "/cli/publish/run", OWNER, json!({"name": name, "project": ws.id, "cmd": format!("exec python3 server.py {port}"), "cwd": folder, "port": port, "noTunnel": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_ne!(r.json()["cmd"], "(app)", "a legacy service, not an App: {}", r.body);
    let id = r.json()["id"].as_str().unwrap().to_string();
    k2_daemon::vault::publish_env::set(&id, Some("legacy"), None).unwrap();
    let r = post(d, "/cli/publish/stop", OWNER, json!({"name": name, "project": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d, "/cli/publish/start", OWNER, json!({"name": name, "project": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_until(&format!("{name} answers"), 20, || body(port, "/") == format!("legacy {tag}")).await;
    Legacy { id, port, folder }
}

/// Make a local legacy service a public one: its row `tunnel`, its label on
/// the control plane at its port (as `k2 publish run` without --no-tunnel
/// leaves it), attributed to its workspace.
fn publish_label(cp: &FakeCp, ws: &Ws, l: &Legacy, label: &str) {
    cp.map.lock().unwrap().insert(label.into(), format!("127.0.0.1:{}", l.port));
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::set_expose(&conn, &l.id, "tunnel").unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO subdomain_workspaces (label, project_id) VALUES (?1, ?2)",
        rusqlite::params![label, ws.id],
    )
    .unwrap();
}

/// An App folder with no K2 caps (its first install needs no approval):
/// a python server on $PORT answering `app <tag>` and `/healthz`.
fn app_folder_nocaps(dir: &std::path::Path, id: &str, tag: &str) -> PathBuf {
    app_folder(dir, id, "1.0.0", tag);
    std::fs::write(
        dir.join("manifest.json"),
        format!(r#"{{"schema":1,"kind":"app","id":"{id}","name":"{id}","version":"1.0.0","caps":[],"host":{{"mode":"cmd","start":["python3","server.py"],"health":{{"path":"/healthz"}},"drainSecs":2}}}}"#),
    )
    .unwrap();
    dunce::canonicalize(&dir).unwrap()
}

fn body(port: u16, path: &str) -> String {
    http_get(port, path).map(|(_, b)| b.trim().to_string()).unwrap_or_default()
}

async fn aget(port: u16, path: &str) -> String {
    let p = path.to_string();
    tokio::task::spawn_blocking(move || body(port, &p)).await.unwrap()
}

fn row(id: &str) -> k2_core::published_services::PublishedService {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::get_by_id(&conn, id).unwrap().expect("service row")
}

async fn listed(d: u16, ws: &Ws, name: &str) -> Value {
    let r = get(d, "/cli/publish/list", OWNER, &format!("project={}", ws.id)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["services"].as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap_or(Value::Null)
}

/// The owner installs a process-engine App in `ws` (this harness has no
/// container engine); its own agent deploys it.
async fn live_app(d: u16, ws: &Ws, agent: &str, app: &str, tag: &str) -> u16 {
    let r = post(d, "/cli/app/install", OWNER, json!({"app": app, "workspace": ws.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["app"]["isolation"], "process", "{}", r.body);
    let folder = app_folder_nocaps(&ws.path.join(format!("{app}-src")), app, tag);
    let r = post(d, "/cli/app/deploy", agent, json!({"app": app, "folder": folder, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
    r.json()["deployment"]["port"].as_u64().unwrap();
    let r = get(d, "/cli/app/inspect", agent, &format!("app={app}")).await;
    r.json()["app"]["frontPort"].as_u64().unwrap() as u16
}

// ── B: the label ─────────────────────────────────────────────────────

/// The agent (switch 1) moves its legacy service's public label to its own
/// healthy App and back: one control-plane write each way, the legacy
/// service runs on unrouted, an unhealthy App is refused, another
/// workspace's label is refused, a switch-off agent is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_moves_its_label_to_its_healthy_app_and_back() {
    require_python3();
    let env = setup(&[]);
    quiet_alerts();
    let cp = fake_cp("box");
    let _cp = EnvVar::set("K2_CONNECT_BASE", format!("http://127.0.0.1:{}", cp.port));
    let mut cfg = k2_core::tunnel::config::TunnelConfig::default();
    cfg.subdomain = "box".into();
    cfg.token = "tok-test".into();
    k2_core::tunnel::config::save(&cfg).expect("save tunnel.json");
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "jobsws");
    let other = seed_ws(&env, "otherws");
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let old = legacy(d.port, &ws, "jobs", "j1").await;
    publish_label(&cp, &ws, &old, "jobs");
    let theirs = legacy(d.port, &other, "theirs", "t1").await;
    publish_label(&cp, &other, &theirs, "theirs");

    // Not healthy yet: installed, nothing deployed.
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "jobsapp", "workspace": ws.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/label/take", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!((r.status, r.code()), (409, "app_not_healthy".to_string()), "{}", r.body);
    assert!(cp.writes.lock().unwrap().is_empty(), "nothing moved");
    let folder = app_folder_nocaps(&ws.path.join("jobsapp-src"), "jobsapp", "a1");
    let r = post(d.port, "/cli/app/deploy", &agent, json!({"app": "jobsapp", "folder": folder, "wait": 60})).await;
    assert_eq!((r.status, r.json()["deployment"]["result"].clone()), (200, json!("live")), "{}", r.body);
    let front = get(d.port, "/cli/app/inspect", &agent, "app=jobsapp").await.json()["app"]["frontPort"].as_u64().unwrap() as u16;
    assert_eq!(aget(front, "/").await, "a1", "the App answers on its front port");

    // Another workspace's label: never across workspaces.
    let r = post(d.port, "/cli/app/label/take", &agent, json!({"app": "jobsapp", "label": "theirs"})).await;
    assert_eq!((r.status, r.code()), (403, "label_owned_by_other_workspace".to_string()), "{}", r.body);
    // N6: it names the agent that holds it and its address, never its folder.
    assert_eq!(r.json()["error"]["owner"]["kind"], "agent", "{}", r.body);
    assert!(r.json()["error"]["owner"]["address"].is_string(), "{}", r.body);
    assert!(!r.body.contains(&*other.path.to_string_lossy()), "no folder path: {}", r.body);
    // Another workspace's agent can't name this App at all.
    allow_manage(&other, true);
    let r = post(d.port, "/cli/app/label/take", &passport(&other), json!({"app": "jobsapp", "label": "theirs"})).await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);

    // Take: the hostname points at the App's front port.
    let r = post(d.port, "/cli/app/label/take", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["frontPort"], front);
    assert_eq!(cp.map.lock().unwrap().get("jobs").cloned(), Some(format!("127.0.0.1:{front}")));
    assert_eq!(*cp.writes.lock().unwrap(), vec![format!("point jobs 127.0.0.1:{front}")], "one control-plane write");
    assert_eq!(aget(old.port, "/").await, "legacy j1", "the legacy service keeps running");
    assert_eq!(row(&old.id).expose, "local", "unrouted: a restart never takes the label back");
    let s = listed(d.port, &ws, "jobs").await;
    assert_eq!(s["labelMovedTo"], "jobsapp", "{s}");
    assert_eq!(cp.map.lock().unwrap().get("theirs").cloned(), Some(format!("127.0.0.1:{}", theirs.port)), "theirs untouched");
    let r = post(d.port, "/cli/app/label/take", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!(r.json()["already"], true, "{}", r.body);
    // A restart of the legacy service doesn't point the label back.
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "jobs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Return refuses while the legacy service is down…
    let r = post(d.port, "/cli/app/label/return", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!((r.status, r.code()), (409, "legacy_not_running".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/publish/start", &agent, json!({"name": "jobs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_until("jobs answers again", 20, || body(old.port, "/") == "legacy j1").await;
    assert_eq!(cp.map.lock().unwrap().get("jobs").cloned(), Some(format!("127.0.0.1:{front}")), "still the App's");

    // A switch-off agent's move is held for the owner.
    allow_manage(&ws, false);
    let r = post(d.port, "/cli/app/label/return", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!((r.status, r.json()["status"].clone(), r.json()["kind"].clone()), (202, json!("held"), json!("manage")), "{}", r.body);
    allow_manage(&ws, true);

    // …and moves it back once it answers.
    let r = post(d.port, "/cli/app/label/return", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(cp.map.lock().unwrap().get("jobs").cloned(), Some(format!("127.0.0.1:{}", old.port)));
    assert_eq!(row(&old.id).expose, "tunnel");
    assert_eq!(listed(d.port, &ws, "jobs").await["labelMovedTo"], Value::Null);
    let r = post(d.port, "/cli/app/label/return", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!((r.status, r.code()), (404, "label_not_taken".to_string()), "{}", r.body);

    // Uninstalling an App that holds a label gives it back.
    let r = post(d.port, "/cli/app/label/take", &agent, json!({"app": "jobsapp", "label": "jobs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/uninstall", &agent, json!({"app": "jobsapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(cp.map.lock().unwrap().get("jobs").cloned(), Some(format!("127.0.0.1:{}", old.port)), "uninstall gave the label back");
    assert_eq!(row(&old.id).expose, "tunnel");
    let _ = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "jobs", "project": ws.id, "keepHostname": true})).await;
    let _ = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "theirs", "project": other.id, "keepHostname": true})).await;
    k2_daemon::apps::alerts::set_sink(None);
}

// ── C: data in ───────────────────────────────────────────────────────

/// The agent copies its legacy data from its workspace into its App's data
/// folder ($K2_APP_DATA); outside the workspace and `..` are refused; a
/// switch-off agent is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_puts_its_workspace_data_into_its_app() {
    require_python3();
    let env = setup(&[]);
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "dataws");
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let _front = live_app(d.port, &ws, &agent, "dataapp", "d1").await;
    let src = ws.path.join("old-data");
    std::fs::create_dir_all(src.join("uploads")).unwrap();
    std::fs::write(src.join("jobs.sqlite3"), b"SQLite format 3\0rows").unwrap();
    std::fs::write(src.join("uploads/cv.pdf"), b"%PDF-1.7").unwrap();
    let r = post(d.port, "/cli/app/data/put", &agent, json!({"app": "dataapp", "from": src, "to": "imported"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["files"], 2, "{}", r.body);
    let data = k2_core::deploy::app_dir("dataapp").join("data/imported");
    assert_eq!(std::fs::read(data.join("jobs.sqlite3")).unwrap(), b"SQLite format 3\0rows");
    assert_eq!(std::fs::read(data.join("uploads/cv.pdf")).unwrap(), b"%PDF-1.7");
    assert!(src.join("jobs.sqlite3").exists(), "the workspace copy stays");
    // One file, relative to the workspace, to the data root.
    let r = post(d.port, "/cli/app/data/put", &agent, json!({"app": "dataapp", "from": "old-data/jobs.sqlite3"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::deploy::app_dir("dataapp").join("data/jobs.sqlite3").exists());
    // Outside the workspace (another folder on the box, a climb out).
    let outside = env.path().join("not-mine.txt");
    std::fs::write(&outside, "x").unwrap();
    for (from, to, code) in [
        (outside.to_string_lossy().to_string(), "", "folder_outside_workspace"),
        ("../not-mine.txt".to_string(), "", "folder_outside_workspace"),
        ("old-data".to_string(), "../../etc", "bad_request"),
    ] {
        let r = post(d.port, "/cli/app/data/put", &agent, json!({"app": "dataapp", "from": from, "to": to})).await;
        assert_eq!(r.code(), code, "{from} {to}: {}", r.body);
    }
    // Switch off: held for the owner, nothing copied.
    allow_manage(&ws, false);
    std::fs::write(src.join("late.txt"), "late").unwrap();
    let r = post(d.port, "/cli/app/data/put", &agent, json!({"app": "dataapp", "from": "old-data/late.txt"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    assert!(!k2_core::deploy::app_dir("dataapp").join("data/late.txt").exists());
    k2_daemon::apps::alerts::set_sink(None);
}

// ── default isolation; 0.46.3 enforces nothing ───────────────────────

/// On a box whose owner opted into the unwalled process engine, a new App
/// still starts in a container; the process engine only on an explicit
/// ask: at once for the owner, held for an agent, refused where it's off.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_apps_default_to_a_container_even_where_the_process_engine_is_on() {
    let env = setup(&[]);
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    assert!(k2_core::deploy::process_engine_enabled(), "the fixture opts this box into the process engine");
    let ws = seed_ws(&env, "isows");
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "dflt", "workspace": ws.id})).await;
    assert_eq!((r.status, r.json()["app"]["isolation"].clone()), (200, json!("container")), "{}", r.body);
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "procy", "workspace": ws.id, "engine": "process"})).await;
    assert_eq!((r.status, r.json()["app"]["isolation"].clone()), (200, json!("process")), "{}", r.body);
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let r = post(d.port, "/cli/app/install", &agent, json!({"app": "agdflt"})).await;
    assert_eq!((r.status, r.json()["app"]["isolation"].clone()), (200, json!("container")), "{}", r.body);
    let r = post(d.port, "/cli/app/install", &agent, json!({"app": "agproc", "engine": "process"})).await;
    assert_eq!((r.status, r.json()["status"].clone(), r.json()["kind"].clone()), (202, json!("held"), json!("manage")), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::deploy::store::get_app(&conn, "agproc").unwrap().is_none());
    }
    {
        let _off = EnvVar::remove("K2_APPS_PROCESS");
        k2_core::deploy::set_process_engine(false);
        let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "procoff", "workspace": ws.id, "engine": "process"})).await;
        assert_eq!((r.status, r.code()), (409, "engine_unavailable".to_string()), "{}", r.body);
    }
    k2_daemon::apps::alerts::set_sink(None);
}

/// No cut-over is set (Rosson 2026-10-26) and nothing enforces one: a
/// legacy service keeps the daemon's environment, its owner-token calls
/// work, it runs unwalled, and it shows the legacy label (publish list, app
/// list, doctor) pointing at k2 study app-move, with no release named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_cutover_is_set_and_a_legacy_service_runs_untouched() {
    require_python3();
    assert_eq!(k2_core::app_grace::CUTOVER_RELEASE, None);
    let env = setup(&[]);
    let _mark = EnvVar::set("SELFMOVE_TEST_INHERITED", "1");
    quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "oldws");
    let svc = legacy(d.port, &ws, "oldbff", "o1").await;
    std::fs::write(svc.folder.join("token.txt"), OWNER).unwrap();
    std::fs::write(svc.folder.join("kport.txt"), d.port.to_string()).unwrap();
    let envs = aget(svc.port, "/env").await;
    assert!(envs.lines().any(|l| l == "SELFMOVE_TEST_INHERITED"), "it keeps the daemon's environment: {envs}");
    assert_eq!(aget(svc.port, "/call").await, "200", "its owner-token calls still work");
    assert_eq!(aget(svc.port, "/").await, "legacy o1", "unwalled: a plain process, not an App");
    // It keeps running; move it when ready. No release is named.
    let label = k2_core::app_grace::legacy_label();
    assert_eq!(label, "legacy (--cmd): keeps running; move it to an App when ready (k2 study app-move)");
    let label = label.as_str();
    let s = listed(d.port, &ws, "oldbff").await;
    assert_eq!(
        (s["envMode"].clone(), s["envRemovedIn"].clone(), s["legacyLabel"].clone(), s["legacyHint"].clone(), s["status"].clone()),
        (json!("legacy"), json!(null), json!(label), json!("k2 study app-move"), json!("running")),
        "{s}"
    );
    assert!(s["envDeprecation"].as_str().unwrap().contains("k2 study app-move"), "{s}");
    assert!(!s["envDeprecation"].as_str().unwrap().contains("0.46.4"), "no end release named: {s}");
    assert!(!s.to_string().contains("adopt"), "no adopt anywhere: {s}");
    let r = get(d.port, "/cli/app/list", OWNER, "").await;
    let l = r.json()["legacy"].as_array().unwrap().iter().find(|l| l["name"] == "oldbff").cloned().unwrap_or_else(|| panic!("{}", r.body));
    assert_eq!((l["label"].clone(), l["hint"].clone()), (json!(label), json!("k2 study app-move")));
    let r = get(d.port, "/cli/app/box", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["cutover"], json!(null), "no cut-over set (kept for compatibility)");
    assert!(r.json()["legacyServices"].as_array().unwrap().iter().any(|l| l["name"] == "oldbff"), "{}", r.body);
    assert_eq!(r.json()["autoPrepare"]["setting"], "appsAutoPrepare", "{}", r.body);
    // An agent sees only its own workspace's legacy services.
    let other = seed_ws(&env, "otherws2");
    let r = get(d.port, "/cli/app/list", &passport(&other), "").await;
    assert_eq!(r.json()["legacy"], json!([]), "{}", r.body);
    // A re-run under "Build and run Apps" stays free (only a NEW agent
    // --cmd service is gated).
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "oldbff"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/publish/start", &agent, json!({"name": "oldbff"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_until("oldbff answers again", 20, || body(svc.port, "/") == "legacy o1").await;
    let _ = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "oldbff", "project": ws.id})).await;
    k2_daemon::apps::alerts::set_sink(None);
}
