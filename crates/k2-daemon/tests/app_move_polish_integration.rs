//! 0.46.4 App-move polish (Agent NSI's list after moving a live Next.js BFF
//! from a `--cmd` service onto the App runtime), end to end through the
//! REAL dispatcher:
//!
//! - #4 warnings: the App's own environment (`K2_PASS_FILE`, `K2_APP_DATA`)
//!   is expected, never a break; App ids: a bare id works on the owner
//!   verbs when it is unique on the box, an ambiguous one lists the
//!   candidates, and `<workspace>/<app id>` works on the runtime verbs.
//! - #5 quiet failures: `pass_required` names the App pass file and says
//!   login needs it too; `k2 app audit` reads oldest first on request.
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB, alerts to a sink. No
//! real agent CLI, no real `~/.k2`, no running daemon.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_fixture::*;
use k2_daemon::apps::gateway::{self, LinkSpec};
use serde_json::{json, Value};

fn parse(text: &str) -> (u16, String) {
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

/// One request on a fresh link connection (`Connection: close`).
fn on_link(port: u16, req: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut got = Vec::new();
    let _ = s.read_to_end(&mut got);
    parse(&String::from_utf8_lossy(&got))
}

fn link_get(path: &str, pass: Option<&str>) -> String {
    let auth = pass.map(|p| format!("Authorization: Bearer {p}\r\n")).unwrap_or_default();
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\n{auth}Connection: close\r\n\r\n")
}

fn link_post(path: &str, pass: Option<&str>, body: &Value) -> String {
    let auth = pass.map(|p| format!("Authorization: Bearer {p}\r\n")).unwrap_or_default();
    let b = body.to_string();
    format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8730\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    )
}

fn err(body: &str) -> Value {
    let v: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("JSON ({e}): {body}"));
    v["error"].clone()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

/// Open `app`'s gateway link the way the guarded engine does; returns
/// (App pass, link port).
fn open_link(app: &str, svc: &str, ws: &Ws) -> (String, u16) {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE apps SET approved_caps_json = ?2 WHERE app_id = ?1",
            rusqlite::params![app, serde_json::to_string(&["thread:read"]).unwrap()],
        )
        .unwrap();
    }
    let pass = gateway::open(
        LinkSpec {
            app_id: app.into(),
            service_id: svc.into(),
            workspace_id: ws.id.clone(),
            port: 0,
            caps: vec!["thread:read".into()],
            rooms: vec![ws.id.clone()],
        },
        Some(Arc::new(|_: &str| Ok(()))),
    )
    .expect("open link");
    (pass, gateway::port_of(app).expect("link port"))
}

/// Wait (bounded) until the App's audit holds at least `n` rows.
async fn audit_rows(port: u16, app: &str, query: &str, n: usize) -> Value {
    let mut a = Value::Null;
    for _ in 0..60 {
        let r = get(port, "/cli/app/audit", OWNER, &format!("app={app}&limit=50{query}")).await;
        assert_eq!(r.status, 200, "{}", r.body);
        a = r.json();
        if a["calls"].as_array().map(Vec::len).unwrap_or(0) >= n {
            return a;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("fewer than {n} audit rows for {app}: {a}");
}

type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn quiet_alerts() -> Seen {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |ws: &str, text: &str| {
        s.lock().unwrap().push((ws.to_string(), text.to_string()));
    })));
    seen
}

/// A published service row (never started): `cmd` for a `--cmd` service,
/// the App sentinel for an App's own row.
fn service(ws: &Ws, name: &str, cmd: &str, folder: &std::path::Path, port: u16) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, cmd, &folder.to_string_lossy(), port, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

/// A runtime App row (never started) with its own service row.
fn app_row(ws: &Ws, app: &str) -> String {
    let svc = service(ws, app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path, 0);
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id: app,
            service_id: &svc,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "process",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .expect("insert app");
    svc
}

fn codes(v: &Value) -> Vec<(String, String)> {
    v["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("no warnings: {v}"))
        .iter()
        .map(|w| (w["code"].as_str().unwrap().to_string(), w["severity"].as_str().unwrap().to_string()))
        .collect()
}

/// #4: App-mode code reading the App's own env is expected, not breaking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warnings_mark_the_apps_own_env_expected_not_breaking() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "warnws");
    let code = ws.path.join("bff");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(
        code.join("server.mjs"),
        "const pass = readFileSync(process.env.K2_PASS_FILE);\nconst data = process.env.K2_APP_DATA;\nconst tok = process.env.K2_OWNER_TOKEN;\n",
    )
    .unwrap();
    let svc = service(&ws, "warnagents", "python3 -m http.server", &code, 47_101);
    k2_daemon::vault::publish_env::set(&svc, Some("clean"), None).unwrap();

    let r = get(d.port, "/cli/app/warnings", OWNER, "app=warnagents").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let got = codes(&v);
    let expected: Vec<&(String, String)> = got.iter().filter(|(c, _)| c == "app_env").collect();
    assert_eq!(expected.len(), 2, "K2_PASS_FILE and K2_APP_DATA each once: {got:?}");
    assert!(expected.iter().all(|(_, s)| s == "expected"), "{got:?}");
    let msgs: Vec<String> = v["warnings"].as_array().unwrap().iter().map(|w| w["message"].as_str().unwrap().to_string()).collect();
    for n in ["K2_PASS_FILE", "K2_APP_DATA"] {
        let m = msgs.iter().find(|m| m.contains(n)).unwrap_or_else(|| panic!("{n} not named: {msgs:?}"));
        assert!(m.contains("Expected inside an App"), "{m}");
        let breaks_on_it = v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["severity"] == "breaks" && w["message"].as_str().unwrap().contains(&format!("reads {n}")));
        assert!(!breaks_on_it, "{n} is not a break: {v}");
    }
    // The owner token read still breaks; only it counts.
    assert!(got.contains(&("k2_internal_env".to_string(), "breaks".to_string())), "{got:?}");
    assert_eq!(v["breaks"], 1, "only K2_OWNER_TOKEN breaks: {v}");
    assert_eq!(v["readyForCutover"], false);
    k2_daemon::apps::alerts::set_sink(None);
}

/// #4: a bare App id works on the owner verbs when it is unique; two
/// services with one name are refused naming both; `<workspace>/<app id>`
/// works on the runtime verbs and must match the App's workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bare_app_ids_resolve_when_unique_and_ambiguous_ones_list_candidates() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let nsi = seed_ws(&env, "agent-nsi");
    let other = seed_ws(&env, "bareother");
    let svc = app_row(&nsi, "agentnsi");

    // The owner's copy-host (dry run) by the bare App id: the App's service.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "agentnsi", "dryRun": true})).await;
    assert_eq!(r.status, 200, "bare App id: {}", r.body);
    assert_eq!(r.json()["app"], svc.as_str(), "{}", r.body);
    // The long form still works and names the same App.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "agent-nsi/agentnsi", "dryRun": true})).await;
    assert_eq!((r.status, r.json()["app"].clone()), (200, json!(svc)), "{}", r.body);
    // A bare publish name unique on the box resolves too.
    let jobs = service(&other, "jobsbff", "node s.js", &other.path, 47_102);
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "jobsbff", "dryRun": true})).await;
    assert_eq!((r.status, r.json()["app"].clone()), (200, json!(jobs)), "{}", r.body);
    // The same name in two workspaces: refused, both candidates named.
    service(&nsi, "jobsbff", "node s.js", &nsi.path, 47_103);
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "jobsbff", "dryRun": true})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    for want in ["ambiguous", "agent-nsi/jobsbff", "bareother/jobsbff"] {
        assert!(r.body.contains(want), "{want} missing: {}", r.body);
    }
    // Unknown stays a plain refusal.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "nosuch", "dryRun": true})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("unknown app"), "{}", r.body);

    // Runtime verbs: `<workspace>/<app id>` names the App …
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=agent-nsi%2Fagentnsi").await;
    assert_eq!(r.status, 200, "{}", r.body);
    // … but only in its own workspace.
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=bareother%2Fagentnsi").await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=agentnsi").await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// #5: a call without the App pass (login included) is told where the pass
/// is; the audit reads oldest first on request and says its order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pass_required_names_the_pass_file_and_audit_reads_oldest_first() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "passws");
    let svc = app_row(&ws, "passapp");
    let (pass, port) = open_link("passapp", &svc, &ws);

    let (no_pass, login_no_pass, me) = blocking(move || {
        let a = on_link(port, &link_get("/cli/skin/agents", None));
        std::thread::sleep(Duration::from_millis(20));
        let b = on_link(port, &link_post("/cli/skin/login", None, &json!({"username": "ada", "password": "x"})));
        std::thread::sleep(Duration::from_millis(20));
        let c = on_link(port, &link_get("/gw/me", Some(&pass)));
        (a, b, c)
    })
    .await;
    for (what, (st, body)) in [("a read", &no_pass), ("the guest login", &login_no_pass)] {
        assert_eq!(*st, 401, "{what}: {body}");
        let e = err(body);
        assert_eq!(e["code"], "pass_required", "{what}: {e}");
        let hint = e["hint"].as_str().unwrap();
        for want in ["K2_PASS_FILE", "POST /cli/skin/login", "Authorization: Bearer"] {
            assert!(hint.contains(want), "{what}: {want} missing from {hint}");
        }
    }
    assert_eq!(me.0, 200, "{}", me.1);

    let newest = audit_rows(d.port, "passapp", "", 3).await;
    assert_eq!(newest["order"], "newest first", "{newest}");
    let oldest = audit_rows(d.port, "passapp", "&oldest=1", 3).await;
    assert_eq!(oldest["order"], "oldest first", "{oldest}");
    let at = |v: &Value| -> Vec<i64> { v["calls"].as_array().unwrap().iter().map(|c| c["atMs"].as_i64().unwrap()).collect() };
    let (n, o) = (at(&newest), at(&oldest));
    assert!(n.windows(2).all(|w| w[0] >= w[1]), "newest first: {n:?}");
    assert!(o.windows(2).all(|w| w[0] <= w[1]), "oldest first: {o:?}");
    let mut back = o.clone();
    back.reverse();
    assert_eq!(back, n, "the same rows, flipped");
    let first = &oldest["calls"][0];
    assert_eq!((first["verb"].as_str(), first["code"].as_str()), (Some("GET /cli/skin/agents"), Some("pass_required")), "{first}");
    gateway::close("passapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}

/// #3: a `.k2appignore` keeps local state, env files and caches out of the
/// Seed a build packs; the answer says the size and the heaviest paths.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k2appignore_keeps_state_out_and_build_reports_the_seed_size() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "sizews");
    let f = app_folder(&ws.path.join("bff"), "sizeapp", "1.0.0", "v1");
    std::fs::create_dir_all(f.join("var")).unwrap();
    std::fs::write(f.join("var/answers.json"), "x".repeat(300_000)).unwrap();
    std::fs::write(f.join(".env.local"), "SECRET=1").unwrap();
    std::fs::create_dir_all(f.join("node_modules/.cache")).unwrap();
    std::fs::write(f.join("node_modules/.cache/big"), "c".repeat(200_000)).unwrap();

    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": f.to_string_lossy()})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let all = r.json();
    let m = &all["seedSize"];
    assert_eq!(m["ignoreFile"], false, "{m}");
    assert_eq!(m["largest"][0]["path"], "var/answers.json", "the heaviest path first: {m}");
    assert!(m["bytes"].as_u64().unwrap() > 500_000, "{m}");

    std::fs::write(f.join(".k2appignore"), "var/\n.env*\nnode_modules/.cache\n").unwrap();
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": f.to_string_lossy()})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let m = &v["seedSize"];
    assert_eq!(m["ignoreFile"], true, "{m}");
    assert!(m["bytes"].as_u64().unwrap() < 100_000, "state and cache left out: {m}");
    let hash = v["seed"]["hash"].as_str().unwrap().to_string();
    assert_ne!(hash, all["seed"]["hash"].as_str().unwrap());
    let tree = k2_core::deploy::seed::seed_path(&hash);
    for gone in ["var", ".env.local", "node_modules/.cache"] {
        assert!(!tree.join(gone).exists(), "{gone} is not in the Seed");
    }
    for kept in ["manifest.json", "server.py", ".k2appignore"] {
        assert!(tree.join(kept).exists(), "{kept} is in the Seed");
    }
    // A broken pattern refuses the build, naming the file.
    std::fs::write(f.join(".k2appignore"), "var/\n[oops\n").unwrap();
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": f.to_string_lossy()})).await;
    assert_eq!(r.status, 422, "{}", r.body);
    assert!(r.body.contains(".k2appignore"), "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// #6: `k2 app domain add|rm` — which labels an App may take, who may, and
/// what the list shows. (The control-plane write itself needs a K2 Connect
/// tunnel; every refusal before it runs here.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn domain_add_takes_only_the_workspaces_own_labels_and_lists_them() {
    let env = setup(&[]);
    let seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let nsi = seed_ws(&env, "domnsi");
    let other = seed_ws(&env, "domother");
    app_row(&nsi, "domapp");
    app_row(&other, "domotherapp");
    service(&nsi, "domagents", "node server.js", &nsi.path, 47_201);
    service(&other, "domtheirs", "node server.js", &other.path, 47_202);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::claim(&conn, "domtheirextra", &other.id).unwrap();
        k2_core::db::schema::SubdomainWorkspace::claim(&conn, "domextra", &nsi.id).unwrap();
    }
    let mine = passport(&nsi);
    allow_manage(&nsi, true);
    let add = |who: String, label: &'static str| {
        let port = d.port;
        async move { post(port, "/cli/app/domain/add", &who, json!({"app": "domapp", "label": label})).await }
    };

    // The --cmd service's own label: that's label take.
    let r = add(mine.clone(), "domagents").await;
    assert_eq!((r.status, r.code()), (409, "use_label_take".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap().contains("k2 app label take domapp domagents"), "{}", r.body);
    // Another workspace's service label or attributed subdomain: refused, naming its agent.
    for label in ["domtheirs", "domtheirextra"] {
        let r = add(mine.clone(), label).await;
        assert_eq!((r.status, r.code()), (403, "label_owned_by_other_workspace".to_string()), "{label}: {}", r.body);
    }
    // Another App's own label; a reserved door.
    let r = add(mine.clone(), "domotherapp").await;
    assert_eq!((r.status, r.code()), (409, "label_serves_another_app".to_string()), "{}", r.body);
    let r = add(mine.clone(), "meet").await;
    assert_eq!((r.status, r.code()), (400, "reserved_label".to_string()), "{}", r.body);
    // A bad label; the App's own label is already its own.
    let r = add(mine.clone(), "no_underscores").await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = add(mine.clone(), "domapp").await;
    assert_eq!((r.status, r.json()["already"].clone()), (200, json!(true)), "{}", r.body);
    // Its own extra subdomain (or a free one) gets as far as the App's
    // health: this one never started.
    for label in ["domextra", "freshlabel"] {
        let r = add(mine.clone(), label).await;
        assert_eq!((r.status, r.code()), (409, "app_not_healthy".to_string()), "{label}: {}", r.body);
    }
    // An existing hostname nobody holds (a hand-made one): an agent never
    // repoints it, transfer or not; the owner only with transfer. One its
    // workspace holds, or a new one, goes on (here as far as the health).
    k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap {
        primary: "box".into(),
        targets: [("domgrafana", "127.0.0.1:3000"), ("domextra", "127.0.0.1:47201")].into_iter().map(|(l, t)| (l.to_string(), t.to_string())).collect(),
    });
    let r = add(mine.clone(), "domgrafana").await;
    assert_eq!((r.status, r.code()), (403, "label_exists_elsewhere".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/app/domain/add", &mine, json!({"app": "domapp", "label": "domgrafana", "transfer": true})).await;
    assert_eq!((r.status, r.code()), (403, "label_exists_elsewhere".to_string()), "an agent's transfer: {}", r.body);
    let r = post(d.port, "/cli/app/domain/add", OWNER, json!({"app": "domapp", "label": "domgrafana"})).await;
    assert_eq!((r.status, r.code()), (409, "label_exists_elsewhere".to_string()), "the owner without transfer: {}", r.body);
    let r = post(d.port, "/cli/app/domain/add", OWNER, json!({"app": "domapp", "label": "domgrafana", "transfer": true})).await;
    assert_eq!((r.status, r.code()), (409, "app_not_healthy".to_string()), "the owner's transfer goes on: {}", r.body);
    let r = add(mine.clone(), "domextra").await;
    assert_eq!((r.status, r.code()), (409, "app_not_healthy".to_string()), "its own existing label: {}", r.body);
    // App install's own label: the same rule.
    let r = post(d.port, "/cli/app/install", &mine, json!({"app": "domgrafana", "tunnel": true})).await;
    assert_eq!((r.status, r.code()), (403, "label_exists_elsewhere".to_string()), "agent install: {}", r.body);
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "domgrafana", "tunnel": true, "workspace": nsi.id})).await;
    assert_eq!((r.status, r.code()), (409, "label_exists_elsewhere".to_string()), "owner install without transfer: {}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::deploy::store::get_app(&conn, "domgrafana").unwrap().is_none(), "nothing installed");
    }
    // A full host name names the same label.
    let r = post(d.port, "/cli/app/domain/add", &mine, json!({"app": "domapp", "label": format!("domtheirs.box.{}", k2_core::tunnel::config::SUBDOMAIN_HOST)})).await;
    assert_eq!(r.code(), "label_owned_by_other_workspace", "{}", r.body);
    // Another workspace's agent never sees this App.
    allow_manage(&other, true);
    let r = post(d.port, "/cli/app/domain/add", &passport(&other), json!({"app": "domapp", "label": "x1"})).await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    // Without "Build and run Apps" the change is held for the owner.
    allow_manage(&nsi, false);
    let r = add(mine.clone(), "domextra").await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    // The owner's card names the hostname.
    let v = r.json();
    assert!(v["request"]["summary"].as_str().expect("summary").ends_with("domain add domapp domextra"), "{v}");
    let lines = v["diff"]["lines"].as_array().expect("diff lines");
    assert!(lines.iter().any(|l| l.as_str().expect("line").starts_with("hostname: domextra.")), "{v}");
    allow_manage(&nsi, true);

    // rm of something never attached; of a taken label (that's label return).
    let r = post(d.port, "/cli/app/domain/rm", &mine, json!({"app": "domapp", "label": "domextra"})).await;
    assert_eq!((r.status, r.code()), (404, "domain_not_attached".to_string()), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut cfg = k2_core::app_grace::config::load(&conn, "domapp").unwrap();
        cfg.labels.push(k2_core::app_grace::config::TakenLabel {
            label: "domagents".into(),
            service_id: "svc-domagents".into(),
            legacy_port: 47_201,
            taken_at: 1,
            taken_by: "agent:domnsi".into(),
        });
        cfg.subdomains.push(k2_core::app_grace::config::AttachedSubdomain {
            label: "domextra".into(),
            added_at: 2,
            added_by: "agent:domnsi".into(),
            previous_target: Some("127.0.0.1:47201".into()),
        });
        conn.execute("UPDATE apps SET config_json = ?2 WHERE app_id = ?1", rusqlite::params!["domapp", serde_json::to_string(&cfg).unwrap()]).unwrap();
    }
    let r = post(d.port, "/cli/app/domain/rm", &mine, json!({"app": "domapp", "label": "domagents"})).await;
    assert_eq!(r.code(), "domain_not_attached");
    assert!(r.body.contains("k2 app label return domapp domagents"), "{}", r.body);
    // The list: its own label, the taken one, the attached one.
    let r = get(d.port, "/cli/app/domains", &mine, "app=domapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["own"]["label"], "domapp");
    assert_eq!(v["taken"][0]["label"], "domagents");
    assert_eq!((v["attached"][0]["label"].clone(), v["attached"][0]["previousTarget"].clone()), (json!("domextra"), json!("127.0.0.1:47201")));
    // Attached hostnames join the App's public origins (with its labels).
    let mut want = vec!["domagents".to_string(), "domextra".to_string()];
    want.sort();
    let mut got = k2_daemon::apps::labels::taken("domapp");
    got.sort();
    assert_eq!(got, want);
    // POST-only.
    for p in ["/cli/app/domain/add", "/cli/app/domain/rm"] {
        let r = get(d.port, p, OWNER, "app=domapp&label=x").await;
        assert_eq!(r.status, 405, "{p}: {}", r.body);
    }

    // A hostname moved to another workspace since: rm only drops it from
    // the App (it is never removed or repointed), and so does uninstall.
    let holder = |label: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).unwrap()
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::db::schema::SubdomainWorkspace::transfer(&conn, "domextra", &nsi.id, &other.id, None).unwrap());
    }
    let r = post(d.port, "/cli/app/domain/rm", &mine, json!({"app": "domapp", "label": "domextra"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["now"].as_str().expect("now").starts_with("left as it is"), "{}", r.body);
    assert_eq!(holder("domextra").as_deref(), Some(other.id.as_str()));
    let r = get(d.port, "/cli/app/domains", &mine, "app=domapp").await;
    assert_eq!(r.json()["attached"], json!([]), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::claim(&conn, "domextra2", &other.id).unwrap();
        let mut cfg = k2_core::app_grace::config::load(&conn, "domapp").unwrap();
        cfg.subdomains.push(k2_core::app_grace::config::AttachedSubdomain {
            label: "domextra2".into(),
            added_at: 3,
            added_by: "agent:domnsi".into(),
            previous_target: None,
        });
        conn.execute("UPDATE apps SET config_json = ?2 WHERE app_id = ?1", rusqlite::params!["domapp", serde_json::to_string(&cfg).unwrap()]).unwrap();
    }
    let r = post(d.port, "/cli/app/uninstall", OWNER, json!({"app": "domapp"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(holder("domextra2").as_deref(), Some(other.id.as_str()), "uninstall leaves another workspace's hostname");
    k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap::default());
    drop(seen);
    k2_daemon::apps::alerts::set_sink(None);
}

fn confirm(action: &str, bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint(action, Some(bound), "owner", "local-auth")
}

/// A box-login guest (the host grant), as `k2 app user add <u>` makes.
fn box_login_user(name: &str) {
    k2_core::skin::add_principal(name).expect("add principal");
    k2_core::skin::set_host_login(name, true).expect("host login");
}

fn step<'a>(plan: &'a Value, id: &str) -> &'a Value {
    plan["steps"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap_or_else(|| panic!("no step {id}: {plan}"))
}

/// #2: `k2 app move-plan` lists every step with what's done, and ONE held
/// request carries every owner step; the owner approves once, the steps run
/// in order as the owner, audited; an agent never approves; the approval
/// copies exactly the users the request saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn move_plan_lists_every_step_and_one_approval_runs_the_owner_steps() {
    let env = setup(&[]);
    let seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    // A vault on a file key under the temp home (the approvals' confirms).
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let nsi = seed_ws(&env, "mpnsi");
    let rooms = seed_ws(&env, "mpsales");
    let stranger = seed_ws(&env, "mpstranger");
    let old = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &nsi.id, "agents", "node server.js", &nsi.path.to_string_lossy(), 47_301, "tunnel", "running", "cmd", "")
            .unwrap()
            .id
    };
    let app_svc = app_row(&nsi, "mpapp");
    // The old BFF hands its guests the `sales` room (its guests-rooms grant).
    k2_core::skin::guest_rooms::grant(&old, &[rooms.id.clone()], "owner").unwrap();
    box_login_user("ada");
    box_login_user("bo");
    let agent = passport(&nsi);
    allow_manage(&nsi, false);

    // The plan, as the service's own agent (no switch yet).
    let r = get(d.port, "/cli/app/move-plan", &agent, "service=agents&app=mpapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let p = r.json();
    let ids: Vec<&str> = p["steps"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        ["box_prep", "engine", "build_switch", "install", "deploy", "copy_host", "guest_rooms", "data_put", "label_take", "stop_old"],
        "{p}"
    );
    assert_eq!(step(&p, "box_prep")["state"], "n/a", "a process-engine App has no wall to prepare");
    assert_eq!(step(&p, "build_switch")["state"], "todo");
    assert_eq!(step(&p, "build_switch")["who"], "owner");
    assert_eq!(step(&p, "install")["state"], "done");
    assert_eq!(step(&p, "deploy")["state"], "todo");
    let ch = step(&p, "copy_host");
    assert_eq!((ch["state"].clone(), ch["userCount"].clone()), (json!("todo"), json!(2)), "{ch}");
    assert!(ch.get("users").is_none(), "an agent without the switch sees the count, not the names: {ch}");
    let gr = step(&p, "guest_rooms");
    assert_eq!((gr["state"].clone(), gr["rooms"].clone(), gr["roomHandles"].clone()), (json!("todo"), json!([rooms.id]), json!(["mpsales"])), "{gr}");
    assert_eq!(step(&p, "label_take")["state"], "blocked", "waits for a live App");
    assert_eq!(step(&p, "stop_old")["command"], "k2 publish stop agents");
    assert_eq!(p["ownerStepsTodo"], json!(["build_switch", "copy_host", "guest_rooms"]));
    // The owner sees the names.
    let r = get(d.port, "/cli/app/move-plan", OWNER, "service=mpnsi%2Fagents").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(step(&r.json(), "copy_host")["users"], json!(["ada", "bo"]));
    assert_eq!(r.json()["app"]["id"], "mpapp", "the workspace's only App by default");
    // A Member login sees the count, never the names; an Admin login sees them.
    let session = |user: &str, role: k2_core::connect_users::Role| {
        k2_core::connect_users::add_user(user, "password-for-mp-int-9").expect("add_user");
        if role != k2_core::connect_users::Role::Member {
            k2_core::connect_users::set_role(user, role).expect("set_role");
        }
        k2_core::connect_users::create_session(user)
    };
    let member = session("mpmember", k2_core::connect_users::Role::Member);
    let r = get(d.port, "/cli/app/move-plan", &member, "service=mpnsi%2Fagents").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let ch = step(&r.json(), "copy_host").clone();
    assert_eq!(ch["userCount"], json!(2), "{ch}");
    assert!(ch.get("users").is_none(), "a Member never sees the names: {ch}");
    let admin = session("mpadmin", k2_core::connect_users::Role::Admin);
    let r = get(d.port, "/cli/app/move-plan", &admin, "service=mpnsi%2Fagents").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(step(&r.json(), "copy_host")["users"], json!(["ada", "bo"]), "{}", r.body);
    // Another workspace's agent: no such service.
    let r = get(d.port, "/cli/app/move-plan", &passport(&stranger), "service=agents").await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    // An App is not a --cmd service.
    let r = get(d.port, "/cli/app/move-plan", OWNER, "service=mpapp").await;
    assert_eq!((r.status, r.code()), (409, "not_a_cmd_service".to_string()), "{}", r.body);

    // ONE request for the three owner steps.
    let r = post(d.port, "/cli/app/move-plan/request", &agent, json!({"service": "agents", "app": "mpapp", "reason": "moving the BFF"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    let req = r.json()["request"]["id"].as_str().unwrap().to_string();
    let lines = r.json()["request"]["diff"]["lines"].as_array().unwrap().len();
    assert_eq!(lines, 3, "{}", r.body);
    let r = get(d.port, "/cli/app/approvals", &agent, "").await;
    assert_eq!(r.json()["requests"].as_array().unwrap().iter().filter(|q| q["id"] == req.as_str()).count(), 1, "one card: {}", r.body);
    // The agent can't approve its own plan (with or without a confirm).
    let r = post(d.port, "/cli/app/approve", &agent, json!({"request": req, "confirmToken": confirm("app-approve", &req)})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    // GET is refused on the request route.
    let r = get(d.port, "/cli/app/move-plan/request", OWNER, "service=agents").await;
    assert_eq!(r.status, 405, "{}", r.body);

    // The owner approves once: every step runs, in order, as the owner.
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": req, "confirmToken": confirm("app-approve", &req)})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["status"], "approved", "{v}");
    let ran: Vec<&str> = v["result"]["steps"].as_array().unwrap().iter().map(|s| s["step"].as_str().unwrap()).collect();
    assert_eq!(ran, ["build_switch", "copy_host", "guest_rooms"], "{v}");
    assert!(v["result"]["steps"].as_array().unwrap().iter().all(|s| s["result"] == "ran"), "{v}");
    assert_eq!(k2_daemon::apps::access::level(&nsi.id), k2_daemon::apps::access::Level::Build);
    let grant = k2_core::skin::guest_rooms::get_grant(&app_svc).unwrap().expect("the App's guests-rooms grant");
    assert_eq!(grant.rooms, vec![rooms.id.clone()]);
    let after = k2_core::skin::copy_host_grants_to_app(&app_svc, "test", true).unwrap();
    assert!(after.added.is_empty(), "ada and bo hold the App now: {after:?}");
    // Each step is in the App's audit.
    let audited = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, "mpapp", 200)
            .unwrap()
            .into_iter()
            .filter(|a| a.event == "move_plan_step")
            .count()
    };
    assert_eq!(audited, 3);
    assert!(seen.lock().unwrap().iter().any(|(w, t)| w == &nsi.id && t.contains(&req) && t.contains("approved")), "the agent hears");
    // Nothing left for the owner.
    let r = post(d.port, "/cli/app/move-plan/request", &agent, json!({"service": "agents", "app": "mpapp"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("nothing_to_request")), "{}", r.body);

    // The approval copies exactly what the request saw: a user added in
    // between stops the step, nothing is copied.
    box_login_user("cy");
    let r = post(d.port, "/cli/app/move-plan/request", &agent, json!({"service": "agents", "app": "mpapp"})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let req2 = r.json()["request"]["id"].as_str().unwrap().to_string();
    assert!(r.body.contains("cy"), "an agent with the switch sees the names: {}", r.body);
    box_login_user("dan");
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": req2, "confirmToken": confirm("app-approve", &req2)})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["status"], "failed", "{}", r.body);
    assert!(r.body.contains("plan_changed"), "{}", r.body);
    let still = k2_core::skin::copy_host_grants_to_app(&app_svc, "test", true).unwrap();
    assert_eq!(still.added, vec!["cy".to_string(), "dan".to_string()], "nothing copied");
    k2_daemon::apps::alerts::set_sink(None);
}

/// A: a refused gateway call is visible — its audit row carries K2's code
/// (a handler's refusal included, not only the gate's), and the App's log
/// gets one `[k2] refused …` line per call, rate-limited, never a pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_gateway_calls_carry_their_code_and_reach_the_apps_log() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "logws");
    let svc = app_row(&ws, "logapp");
    let dep = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let dep = k2_core::deploy::store::insert_deployment(
            &conn,
            &k2_core::deploy::store::NewDeployment { app_id: "logapp", kind: "deploy", from_seed: None, to_seed: None, commit_ref: None, actor: "owner", reason: None },
        )
        .unwrap();
        k2_core::deploy::store::set_live(&conn, "logapp", None, Some(&dep)).unwrap();
        dep
    };
    let (pass, port) = open_link("logapp", &svc, &ws);
    let p = pass.clone();
    let (login, no_pass) = blocking(move || {
        let a = on_link(port, &link_post("/cli/skin/login", Some(&p), &json!({"username": "nobody", "password": "wrong"})));
        let b = on_link(port, &link_get("/cli/skin/agents", None));
        (a, b)
    })
    .await;
    assert!(login.0 >= 400, "a bad login is refused by its handler: {login:?}");
    assert_eq!(no_pass.0, 401);
    let a = audit_rows(d.port, "logapp", "&denied=1", 2).await;
    let rows = a["calls"].as_array().unwrap();
    let login_row = rows.iter().find(|c| c["verb"] == "POST /cli/skin/login").unwrap_or_else(|| panic!("{a}"));
    let code = login_row["code"].as_str().unwrap_or_else(|| panic!("the handler's refusal has a code: {login_row}"));
    assert!(!code.is_empty(), "{login_row}");
    assert_eq!(rows.iter().find(|c| c["verb"] == "GET /cli/skin/agents").unwrap()["code"], "pass_required");

    // The App's log: one line each, K2's words, no pass.
    k2_daemon::apps::audit::flush();
    let log_path = k2_core::deploy::deployment_log_path("logapp", &dep);
    let log = std::fs::read_to_string(&log_path).unwrap_or_else(|e| panic!("{}: {e}", log_path.display()));
    assert!(log.contains(&format!("[k2] refused POST /cli/skin/login: {code}")), "{log}");
    assert!(log.contains("[k2] refused GET /cli/skin/agents: pass_required (every call on the gateway link carries"), "{log}");
    assert!(!log.contains("k2skn_"), "never a pass in the App's log: {log}");

    // A flood: at most REFUSED_LINES_PER_MIN lines a minute per App.
    blocking(move || {
        for _ in 0..30 {
            on_link(port, &link_get("/cli/skin/agents", None));
        }
    })
    .await;
    audit_rows(d.port, "logapp", "&denied=1", 32).await;
    k2_daemon::apps::audit::flush();
    let log = std::fs::read_to_string(&log_path).unwrap();
    let lines = log.lines().filter(|l| l.starts_with("[k2] refused ")).count();
    assert_eq!(lines, k2_daemon::apps::audit::REFUSED_LINES_PER_MIN, "rate-limited: {log}");
    gateway::close("logapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}

/// B: test guests are a person's to make (agent test guests are parked,
/// Rosson 2026-10-24): one App's only, its own roles, a short life, at
/// most 5, shown as test, audited; they sign in only through that App's
/// link. An agent passport is refused `owner_only` on add and remove,
/// with the not-requestable teaching line while permission cards are on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_guests_are_one_apps_short_lived_and_sign_in_only_on_its_link() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "tgws");
    let other = seed_ws(&env, "tgother");
    let svc = app_row(&ws, "tgapp");
    let other_svc = app_row(&other, "tgotherapp");
    let mut policy = k2_core::skin::RoomPolicy::new();
    policy.insert(ws.id.clone(), vec!["thread:read".to_string()]);
    k2_core::skin::create_role_for_app("tester", &policy, Some(&svc)).unwrap();
    k2_core::skin::create_role_for_app("boxwide", &policy, None).unwrap();
    k2_core::skin::create_role_for_app("theirs", &policy, Some(&other_svc)).unwrap();
    let agent = passport(&ws);
    allow_manage(&ws, true);
    let add = |who: String, body: Value| {
        let port = d.port;
        async move { post(port, "/cli/app/test-guest/add", &who, body).await }
    };

    let before = k2_core::deploy::now();
    let r = add(OWNER.to_string(), json!({"app": "tgapp", "username": "qa1", "role": "tester", "expires": "2h"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let pw = v["password"].as_str().expect("K2 makes the password and shows it once").to_string();
    assert_eq!((v["app"].clone(), v["role"].clone(), v["createdBy"].clone()), (json!("tgapp"), json!("tester"), json!("owner")), "{v}");
    let ends = v["expiresAt"].as_i64().expect("expiresAt");
    assert!((before + 7200..=before + 7210).contains(&ends), "{v}");
    assert!(v.get("ownerNote").is_some(), "{v}");
    // Roles: only the App's own.
    for (role, why) in [("boxwide", "a box-wide role"), ("theirs", "another App's role"), ("admin", "a role nobody defined")] {
        let r = add(OWNER.to_string(), json!({"app": "tgapp", "username": "qa-x", "role": role})).await;
        assert_eq!((r.status, r.code()), (400, "role_not_this_apps".to_string()), "{why}: {}", r.body);
    }
    // At most 7 days.
    let r = add(OWNER.to_string(), json!({"app": "tgapp", "username": "qa-x", "expires": "8d"})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    // At most 5 live per App.
    for n in 2..=5 {
        let r = add(OWNER.to_string(), json!({"app": "tgapp", "username": format!("qa{n}")})).await;
        assert_eq!(r.status, 200, "qa{n}: {}", r.body);
    }
    let r = add(OWNER.to_string(), json!({"app": "tgapp", "username": "qa6"})).await;
    assert_eq!((r.status, r.code()), (409, "too_many_test_guests".to_string()), "{}", r.body);

    // Agents make no guests (parked): not the App's own agent with "Build
    // and run Apps" on, not another workspace's; the same code either way
    // (nothing said about whose App it is). Below the cap first, so the
    // refusal is the agent's, not the cap's.
    let r = post(d.port, "/cli/app/test-guest/rm", OWNER, json!({"username": "qa5"})).await;
    assert_eq!((r.status, r.json()["removed"].clone()), (200, json!(true)), "{}", r.body);
    allow_manage(&other, true);
    for (who, label) in [(agent.clone(), "the App's own agent"), (passport(&other), "another workspace's agent")] {
        let r = add(who.clone(), json!({"app": "tgapp", "username": "qa-agent"})).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{label} add: {}", r.body);
        assert!(r.json()["error"]["hint"].as_str().expect("hint").contains("only a person makes or removes guests"), "{label}: {}", r.body);
        let r = post(d.port, "/cli/app/test-guest/rm", &who, json!({"username": "qa1"})).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{label} rm: {}", r.body);
    }
    // Nor through the guest roster (the skin-manage switch is on): the
    // agent can't reset an owner-made test guest's password and sign in
    // as it, or change its email, rooms or role, unlock or remove it.
    let ws_handle = ws.handle.clone();
    for (path, body) in [
        ("/cli/skin/users/password", json!({"username": "qa1", "password": "agent-chosen-pw-1"})),
        ("/cli/skin/users/email", json!({"username": "qa1", "email": "agent@example.com"})),
        ("/cli/skin/users/rooms", json!({"username": "qa1", "rooms": [ws_handle]})),
        ("/cli/skin/users/unlock", json!({"username": "qa1"})),
        ("/cli/skin/roles/assign", json!({"username": "qa1", "role": "tester"})),
        ("/cli/skin/roles/unassign", json!({"username": "qa1"})),
        ("/cli/skin/users/remove", json!({"username": "qa1"})),
    ] {
        let r = post(d.port, path, &agent, body).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{path}: {}", r.body);
        assert!(r.json()["error"]["hint"].as_str().expect("hint").contains("is a test guest"), "{path}: {}", r.body);
    }
    // With permission cards on for the agent, the refusal says it can't be
    // requested (a person does it), and is otherwise the same.
    k2_core::permission::gate::set_server_switch(true);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn.execute("UPDATE projects SET agents_can_request_permissions = 1 WHERE id = ?1", [&ws.id]).expect("agent switch");
        assert_eq!(n, 1);
    }
    let r = add(agent.clone(), json!({"app": "tgapp", "username": "qa-agent"})).await;
    k2_core::permission::gate::set_server_switch(false);
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert_eq!(r.json()["permission"]["requestable"], json!(false), "not requestable: {}", r.body);
    assert!(r.json()["permission"].get("command").is_none(), "no Ask-with command: {}", r.body);
    // Nothing was made by any of it.
    let r = get(d.port, "/cli/app/test-guests", OWNER, "app=tgapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mut names: Vec<String> =
        r.json()["testGuests"].as_array().expect("testGuests").iter().map(|t| t["username"].as_str().expect("username").to_string()).collect();
    // Listed oldest first; guests made in the same second tie on time.
    names.sort();
    assert_eq!(names, ["qa1", "qa2", "qa3", "qa4"], "{}", r.body);
    assert!(k2_core::skin::list_principals().unwrap().iter().all(|p| p.username != "qa-agent"), "no guest from an agent");
    // The App's own agent may read the list; another workspace's agent
    // never sees the App.
    let r = get(d.port, "/cli/app/test-guests", &agent, "app=tgapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["testGuests"].as_array().expect("testGuests").len(), 4, "{}", r.body);
    let r = get(d.port, "/cli/app/test-guests", &passport(&other), "app=tgapp").await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);

    // The roster says test.
    let r = get(d.port, "/cli/skin/users", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let users = r.json()["users"].as_array().expect("users").clone();
    let qa1 = users.iter().find(|u| u["username"] == "qa1").unwrap_or_else(|| panic!("{users:?}"));
    assert_eq!((qa1["test"]["app"].clone(), qa1["test"]["expiresAt"].clone()), (json!("tgapp"), json!(ends)), "{qa1}");

    // Sign-in: only through tgapp's link.
    let (pass, port) = open_link("tgapp", &svc, &ws);
    let (other_pass, other_port) = open_link("tgotherapp", &other_svc, &other);
    let (p1, p2, pw1, pw2) = (pass.clone(), other_pass.clone(), pw.clone(), pw.clone());
    let (own_link, other_link) = blocking(move || {
        let a = on_link(port, &link_post("/cli/skin/login", Some(&p1), &json!({"username": "qa1", "password": pw1})));
        let b = on_link(other_port, &link_post("/cli/skin/login", Some(&p2), &json!({"username": "qa1", "password": pw2})));
        (a, b)
    })
    .await;
    assert_eq!(own_link.0, 200, "its own App link: {own_link:?}");
    let session: Value = serde_json::from_str(&own_link.1).unwrap();
    assert!(session["token"].as_str().expect("token").starts_with("k2skn_"), "{session}");
    assert_eq!(other_link.0, 401, "another App link: {other_link:?}");
    let main_port = |body: Value| {
        let port = d.port;
        async move { tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body.to_string()))).await.unwrap() }
    };
    let r = main_port(json!({"username": "qa1", "password": pw, "app": svc})).await;
    assert_eq!(r.status, 401, "the main port (the App named): {}", r.body);
    let r = main_port(json!({"username": "qa1", "password": pw})).await;
    assert_eq!(r.status, 401, "never the box login: {}", r.body);

    // Remove: the owner can.
    let r = post(d.port, "/cli/app/test-guest/rm", OWNER, json!({"username": "qa4"})).await;
    assert_eq!((r.status, r.json()["removed"].clone()), (200, json!(true)), "{}", r.body);
    let r = get(d.port, "/cli/app/test-guests", OWNER, "app=tgapp").await;
    assert_eq!(r.json()["testGuests"].as_array().expect("testGuests").len(), 3, "{}", r.body);
    // A real guest is never "removed" as a test guest.
    k2_core::skin::add_principal_for_app("realguest", None, &svc, None, "owner").unwrap();
    let r = post(d.port, "/cli/app/test-guest/rm", OWNER, json!({"username": "realguest"})).await;
    assert_eq!((r.status, r.code()), (404, "not_a_test_guest".to_string()), "{}", r.body);
    // Audited on the App.
    let adds = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, "tgapp", 200)
            .unwrap()
            .into_iter()
            .filter(|a| a.event == "test_guest_add")
            .count()
    };
    assert_eq!(adds, 5);
    gateway::close("tgapp", None, true);
    gateway::close("tgotherapp", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}

/// The guest roster, for an agent with the skin-manage switch: it acts
/// only on guests wholly inside its own workspace, and hands them only
/// its own rooms and roles (security review, the older hole). Anything
/// else is owner_only and touches nothing; the owner is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_manages_only_its_own_workspaces_guests() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let a = seed_ws(&env, "scopea");
    let b = seed_ws(&env, "scopeb");
    let svc_a = app_row(&a, "scopeappa");
    let svc_b = app_row(&b, "scopeappb");
    let room = |ws: &Ws| {
        let mut policy = k2_core::skin::RoomPolicy::new();
        policy.insert(ws.id.clone(), vec!["thread:read".to_string()]);
        policy
    };
    k2_core::skin::create_role_for_app("scope-ra", &room(&a), None).unwrap();
    k2_core::skin::create_role_for_app("scope-rb", &room(&b), None).unwrap();
    // The owner's guests: one of each workspace's App, a box guest in B's room.
    k2_core::skin::add_principal_for_app("scope-ga", None, &svc_a, None, "owner").unwrap();
    k2_core::skin::add_principal_for_app("scope-gb", None, &svc_b, None, "owner").unwrap();
    k2_core::skin::add_principal("scope-boxb").unwrap();
    k2_core::skin::assign_role("scope-boxb", "scope-rb").unwrap();
    allow_manage(&a, true);
    let agent = passport(&a);
    let user = |name: &str| {
        k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == name).unwrap_or_else(|| panic!("no guest {name}"))
    };

    // Another workspace's guests: refused, nothing touched.
    for (path, body) in [
        ("/cli/skin/users/password", json!({"username": "scope-gb", "password": "agent-chosen-pw-1"})),
        ("/cli/skin/users/email", json!({"username": "scope-gb", "email": "agent@example.com"})),
        ("/cli/skin/users/unlock", json!({"username": "scope-gb"})),
        ("/cli/skin/users/remove", json!({"username": "scope-gb"})),
        ("/cli/skin/users/password", json!({"username": "scope-boxb", "password": "agent-chosen-pw-1"})),
        ("/cli/skin/users/rooms", json!({"username": "scope-boxb", "rooms": [a.handle]})),
        ("/cli/skin/roles/assign", json!({"username": "scope-boxb", "role": "scope-ra"})),
        ("/cli/skin/roles/unassign", json!({"username": "scope-boxb"})),
        ("/cli/skin/users/remove", json!({"username": "scope-boxb"})),
    ] {
        let r = post(d.port, path, &agent, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{path} {body}: {}", r.body);
        assert!(r.json()["error"]["hint"].as_str().expect("hint").contains("reaches outside your workspace"), "{path}: {}", r.body);
    }
    let (gb, boxb) = (user("scope-gb"), user("scope-boxb"));
    assert_eq!((gb.has_password, gb.email.clone()), (false, None), "{gb:?}");
    assert_eq!((boxb.has_password, boxb.role_name.as_deref()), (false, Some("scope-rb")), "{boxb:?}");

    // The owner's guest with nothing yet (no room, role or App): still the
    // owner's, on every route (security review B).
    k2_core::skin::add_principal("scope-alice").unwrap();
    for (path, body) in [
        ("/cli/skin/users/password", json!({"username": "scope-alice", "password": "agent-chosen-pw-3"})),
        ("/cli/skin/users/email", json!({"username": "scope-alice", "email": "alice@example.com"})),
        ("/cli/skin/users/rooms", json!({"username": "scope-alice", "rooms": [a.handle]})),
        ("/cli/skin/users/unlock", json!({"username": "scope-alice"})),
        ("/cli/skin/roles/assign", json!({"username": "scope-alice", "role": "scope-ra"})),
        ("/cli/skin/roles/unassign", json!({"username": "scope-alice"})),
        ("/cli/skin/users/remove", json!({"username": "scope-alice"})),
    ] {
        let r = post(d.port, path, &agent, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "owner's empty guest {path}: {}", r.body);
        assert!(r.json()["error"]["hint"].as_str().expect("hint").contains("or it is the owner's"), "{path}: {}", r.body);
    }
    // Unlock by email resolves the guest as unlock does (security review C);
    // a name that is no guest is never the agent's to unlock.
    let r = post(d.port, "/cli/skin/users/email", OWNER, json!({"username": "scope-boxb", "email": "boxb@example.com"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    for name in ["boxb@example.com", "BoxB@Example.com", "nobody-at-all", "nobody@example.com"] {
        let r = post(d.port, "/cli/skin/users/unlock", &agent, json!({"username": name})).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "unlock {name}: {}", r.body);
    }
    let r = post(d.port, "/cli/skin/users/unlock", OWNER, json!({"username": "boxb@example.com"})).await;
    assert_eq!(r.status, 200, "the owner unlocks by email: {}", r.body);
    let alice = user("scope-alice");
    assert_eq!((alice.has_password, alice.email.clone(), alice.default_rooms.clone(), alice.role_id.clone()), (false, None, Vec::<String>::new(), None), "{alice:?}");

    // Its own App's guest: allowed.
    let r = post(d.port, "/cli/skin/users/password", &agent, json!({"username": "scope-ga", "password": "agent-chosen-pw-2"})).await;
    assert_eq!(r.status, 200, "its own App's guest: {}", r.body);
    assert!(user("scope-ga").has_password);

    // A box guest it may not add (agent-app-users S2: App users only).
    let r = post(d.port, "/cli/skin/users", &agent, json!({"username": "scope-fresh"})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "no box login from an agent: {}", r.body);
    // An App user it adds (stamped as its workspace's): only its own rooms and roles.
    let r = post(d.port, "/cli/skin/users", &agent, json!({"username": "scope-fresh", "app": svc_a})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(k2_core::skin::creators::workspace_of(&user("scope-fresh").id).unwrap().as_deref(), Some(a.id.as_str()), "stamped");
    let r = post(d.port, "/cli/skin/users/password", &agent, json!({"username": "scope-fresh", "password": "agent-chosen-pw-4"})).await;
    assert_eq!(r.status, 200, "its own new guest, still empty: {}", r.body);
    let r = post(d.port, "/cli/skin/users/rooms", &agent, json!({"username": "scope-fresh", "rooms": [b.handle]})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "another workspace's room: {}", r.body);
    let r = post(d.port, "/cli/skin/roles/assign", &agent, json!({"username": "scope-fresh", "role": "scope-rb"})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "another workspace's role: {}", r.body);
    assert_eq!((user("scope-fresh").default_rooms, user("scope-fresh").role_id), (Vec::<String>::new(), None));
    let r = post(d.port, "/cli/skin/users/rooms", &agent, json!({"username": "scope-fresh", "rooms": [a.handle]})).await;
    assert_eq!(r.status, 200, "its own room: {}", r.body);
    let r = post(d.port, "/cli/skin/roles/assign", &agent, json!({"username": "scope-fresh", "role": "scope-ra"})).await;
    assert_eq!(r.status, 200, "its own role: {}", r.body);
    assert_eq!(user("scope-fresh").role_name.as_deref(), Some("scope-ra"));

    // The owner is unchanged.
    let r = post(d.port, "/cli/skin/users/password", OWNER, json!({"username": "scope-gb", "password": "owner-chosen-pw-1"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/skin/roles/assign", OWNER, json!({"username": "scope-fresh", "role": "scope-rb"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// Role writes, for an agent with the skin-manage switch: it makes or
/// edits only roles whose rooms are all in its own workspace (security
/// review). A room elsewhere, or a role that already reaches elsewhere, is
/// owner_only and nothing changes; its own roles still work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_makes_and_edits_only_its_own_workspaces_roles() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let a = seed_ws(&env, "rolea");
    let b = seed_ws(&env, "roleb");
    allow_manage(&a, true);
    let agent = passport(&a);
    let mut policy_b = k2_core::skin::RoomPolicy::new();
    policy_b.insert(b.id.clone(), vec!["thread:read".to_string()]);
    k2_core::skin::create_role_for_app("rw-owner-b", &policy_b, None).unwrap();
    let role = |name: &str| k2_core::skin::list_roles().unwrap().into_iter().find(|r| r.name == name);
    let refused = |r: &Resp, what: &str| {
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{what}: {}", r.body);
        assert!(r.json()["error"]["hint"].as_str().expect("hint").contains("outside your workspace"), "{what}: {}", r.body);
    };
    let line = |ws: &Ws| json!([{"handle": ws.handle, "caps": ["thread:read"]}]);

    // Its own room: made and edited.
    let r = post(d.port, "/cli/skin/roles", &agent, json!({"name": "rw-mine", "roomAccess": line(&a)})).await;
    assert_eq!(r.status, 200, "its own room: {}", r.body);
    let r = post(d.port, "/cli/skin/roles/update", &agent, json!({"name": "rw-mine", "roomAccess": [{"handle": a.handle, "caps": ["thread:read", "thread:post"]}]})).await;
    assert_eq!(r.status, 200, "its own role's update: {}", r.body);
    let r = post(d.port, "/cli/skin/roles/room", &agent, json!({"name": "rw-mine", "handle": a.handle, "caps": ["thread:read"]})).await;
    assert_eq!(r.status, 200, "its own room line: {}", r.body);

    // Another workspace's room: refused on every role write.
    let r = post(d.port, "/cli/skin/roles", &agent, json!({"name": "rw-theirs", "roomAccess": line(&b)})).await;
    refused(&r, "create with another workspace's room");
    let r = post(d.port, "/cli/skin/roles", &agent, json!({"name": "rw-mix", "rooms": [a.handle, b.handle]})).await;
    refused(&r, "create with a room list reaching elsewhere");
    let r = post(d.port, "/cli/skin/roles/from-template", &agent, json!({"template": "owner", "app": "host", "name": "rw-tpl", "rooms": [b.handle]})).await;
    refused(&r, "from-template with another workspace's room");
    assert!(role("rw-theirs").is_none() && role("rw-mix").is_none() && role("rw-tpl").is_none(), "nothing made");
    let r = post(d.port, "/cli/skin/roles/update", &agent, json!({"name": "rw-mine", "roomAccess": line(&b)})).await;
    refused(&r, "update to another workspace's room");
    let r = post(d.port, "/cli/skin/roles/room", &agent, json!({"name": "rw-mine", "handle": b.handle, "caps": ["thread:read"]})).await;
    refused(&r, "a room line elsewhere");
    assert_eq!(role("rw-mine").expect("rw-mine").rooms, vec![a.id.clone()], "its role still reaches only its room");
    // A role that already reaches elsewhere: not the agent's to edit or remove.
    let r = post(d.port, "/cli/skin/roles/update", &agent, json!({"name": "rw-owner-b", "roomAccess": line(&a)})).await;
    refused(&r, "update another workspace's role");
    let r = post(d.port, "/cli/skin/roles/room", &agent, json!({"name": "rw-owner-b", "handle": a.handle, "caps": ["thread:read"]})).await;
    refused(&r, "add its room to another workspace's role");
    let r = post(d.port, "/cli/skin/roles/paths", &agent, json!({"name": "rw-owner-b", "handle": b.handle, "read": ["docs"]})).await;
    refused(&r, "folders on another workspace's role");
    let r = post(d.port, "/cli/skin/roles/remove", &agent, json!({"name": "rw-owner-b"})).await;
    refused(&r, "remove another workspace's role");
    assert_eq!(role("rw-owner-b").expect("rw-owner-b").rooms, vec![b.id.clone()], "the owner's role is unchanged");

    // The owner is unchanged.
    let r = post(d.port, "/cli/skin/roles/room", OWNER, json!({"name": "rw-mine", "handle": b.handle, "caps": ["thread:read"]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// Platform passes, for an agent with the skin-manage switch: it mints a
/// pass only for its own workspace's rooms, and changes the rooms of, or
/// revokes, only a pass wholly inside its workspace; a guest's sign-in
/// session outside it is never its to revoke or re-room (security review).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_mints_and_edits_only_its_own_workspaces_platform_passes() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let a = seed_ws(&env, "passa");
    let b = seed_ws(&env, "passb");
    allow_manage(&a, true);
    let agent = passport(&a);
    let refused = |r: &Resp, what: &str| {
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{what}: {}", r.body);
        assert!(r.json()["error"]["hint"].as_str().expect("hint").contains("outside your workspace"), "{what}: {}", r.body);
    };
    let pass = |id: &str| {
        k2_core::skin::list_tokens().unwrap().into_iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no pass {id}"))
    };
    let names = || k2_core::skin::list_tokens().unwrap().into_iter().map(|t| t.name).collect::<Vec<_>>();

    // Its own room: minted, re-roomed within it, revoked.
    let r = post(d.port, "/cli/skin-tokens", &agent, json!({"name": "pt-mine", "caps": ["thread:read"], "rooms": [a.handle]})).await;
    assert_eq!(r.status, 200, "its own room: {}", r.body);
    let mine = r.json()["id"].as_str().expect("id").to_string();
    let r = post(d.port, "/cli/skin-tokens/rooms", &agent, json!({"id": mine, "rooms": [a.handle]})).await;
    assert_eq!(r.status, 200, "its own pass, its own room: {}", r.body);

    // Another workspace's room: never minted, never added.
    let r = post(d.port, "/cli/skin-tokens", &agent, json!({"name": "pt-theirs", "caps": ["thread:read"], "rooms": [b.handle]})).await;
    refused(&r, "mint for another workspace's room");
    let r = post(d.port, "/cli/skin-tokens", &agent, json!({"name": "pt-mix", "caps": ["thread:read"], "rooms": [a.handle, b.handle]})).await;
    refused(&r, "mint reaching elsewhere");
    assert!(!names().iter().any(|n| n == "pt-theirs" || n == "pt-mix"), "nothing minted: {:?}", names());
    let r = post(d.port, "/cli/skin-tokens/rooms", &agent, json!({"id": mine, "rooms": [a.handle, b.handle]})).await;
    refused(&r, "widen its pass elsewhere");
    assert_eq!(pass(&mine).rooms, vec![a.id.clone()], "its pass is unchanged");

    // The owner's pass for another workspace: not the agent's to change or revoke.
    let r = post(d.port, "/cli/skin-tokens", OWNER, json!({"name": "pt-owner-b", "caps": ["thread:read"], "rooms": [b.handle]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let theirs = r.json()["id"].as_str().expect("id").to_string();
    let r = post(d.port, "/cli/skin-tokens/rooms", &agent, json!({"id": theirs, "rooms": [a.handle]})).await;
    refused(&r, "re-room another workspace's pass");
    let r = post(d.port, "/cli/skin-tokens/revoke", &agent, json!({"id": theirs})).await;
    refused(&r, "revoke another workspace's pass");
    let t = pass(&theirs);
    assert_eq!((t.rooms, t.revoked_at), (vec![b.id.clone()], None), "the owner's pass is unchanged");

    // A guest's sign-in outside its workspace: never re-roomed, never revoked.
    let mut policy_b = k2_core::skin::RoomPolicy::new();
    policy_b.insert(b.id.clone(), vec!["thread:read".to_string()]);
    k2_core::skin::create_role_for_app("pt-rb", &policy_b, None).unwrap();
    box_login_user("pt-boxb");
    k2_core::skin::assign_role("pt-boxb", "pt-rb").unwrap();
    let (session, _raw) = k2_core::skin::create_session_token("pt-boxb").unwrap();
    let before = k2_core::skin::token_scope(&session.id).unwrap().expect("the session row");
    assert!(before.session, "{before:?}");
    for (path, body) in [
        ("/cli/skin-tokens/rooms", json!({"id": session.id, "rooms": [a.handle]})),
        ("/cli/skin-tokens/revoke", json!({"id": session.id})),
    ] {
        let r = post(d.port, path, &agent, body).await;
        refused(&r, path);
    }
    assert_eq!(k2_core::skin::token_scope(&session.id).unwrap().expect("the session row"), before, "the sign-in is unchanged");
    let r = post(d.port, "/cli/skin-tokens/revoke", OWNER, json!({"id": session.id})).await;
    assert_eq!((r.status, r.json()["success"].clone()), (200, json!(true)), "still live until the owner revokes it: {}", r.body);

    // A pass with no rooms, or rooms that can't be read: never the agent's.
    let r = post(d.port, "/cli/skin-tokens", OWNER, json!({"name": "pt-dark", "caps": ["thread:read"], "rooms": [a.handle]})).await;
    let dark = r.json()["id"].as_str().expect("id").to_string();
    let r = post(d.port, "/cli/skin-tokens/rooms", OWNER, json!({"id": dark, "rooms": []})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/skin-tokens", OWNER, json!({"name": "pt-garbled", "caps": ["thread:read"], "rooms": [a.handle]})).await;
    let garbled = r.json()["id"].as_str().expect("id").to_string();
    {
        let conn = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db")).expect("skin.db");
        let n = conn.execute("UPDATE tokens SET rooms = 'not json' WHERE id = ?1", rusqlite::params![garbled]).expect("garble");
        assert_eq!(n, 1);
    }
    for id in [&dark, &garbled] {
        let r = post(d.port, "/cli/skin-tokens/rooms", &agent, json!({"id": id, "rooms": [a.handle]})).await;
        refused(&r, "re-room a pass naming no room");
        let r = post(d.port, "/cli/skin-tokens/revoke", &agent, json!({"id": id})).await;
        refused(&r, "revoke a pass naming no room");
        assert_eq!(pass(id).revoked_at, None, "still live");
    }

    // Its own pass: revoked. The owner is unchanged.
    let r = post(d.port, "/cli/skin-tokens/revoke", &agent, json!({"id": mine})).await;
    assert_eq!((r.status, r.json()["success"].clone()), (200, json!(true)), "{}", r.body);
    let r = post(d.port, "/cli/skin-tokens/rooms", OWNER, json!({"id": theirs, "rooms": [a.handle, b.handle]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// A decoy role named like another role's id never stands in for it: the
/// checks resolve a role as the handlers do (id first, then name), and a
/// new role's name may not look like an id (security review A).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_role_named_like_another_roles_id_is_never_the_one_checked() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let a = seed_ws(&env, "decoya");
    let b = seed_ws(&env, "decoyb");
    allow_manage(&a, true);
    let agent = passport(&a);
    let mut policy_b = k2_core::skin::RoomPolicy::new();
    policy_b.insert(b.id.clone(), vec!["thread:read".to_string()]);
    let victim = k2_core::skin::create_role_for_app("decoy-victim", &policy_b, None).unwrap();
    // No new role may be named like an id, for anyone.
    for who in [agent.as_str(), OWNER] {
        let r = post(d.port, "/cli/skin/roles", who, json!({"name": victim.id, "roomAccess": [{"handle": a.handle, "caps": ["thread:read"]}]})).await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert!(r.body.contains("can't look like a role id"), "{}", r.body);
    }
    // A decoy left from before (written straight into skin.db): its own
    // workspace's rooms, named like the victim's id.
    {
        let conn = rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db")).expect("skin.db");
        let mut policy = serde_json::Map::new();
        policy.insert(a.id.clone(), json!(["thread:read"]));
        let policy = Value::Object(policy).to_string();
        conn.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy, app_id) VALUES (?1, ?2, '[\"thread:read\"]', ?3, 1, ?4, NULL)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), victim.id, json!([a.id.clone()]).to_string(), policy],
        )
        .expect("insert the decoy");
    }
    let r = post(d.port, "/cli/skin/users", &agent, json!({"username": "decoy-guest", "app": svc_a})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    for (path, body) in [
        ("/cli/skin/roles/assign", json!({"username": "decoy-guest", "role": victim.id})),
        ("/cli/skin/roles/update", json!({"name": victim.id, "roomAccess": [{"handle": a.handle, "caps": ["thread:read"]}]})),
        ("/cli/skin/roles/room", json!({"name": victim.id, "handle": a.handle, "caps": ["thread:read"]})),
        ("/cli/skin/roles/paths", json!({"name": victim.id, "handle": b.handle, "read": ["docs"]})),
        ("/cli/skin/roles/remove", json!({"name": victim.id})),
    ] {
        let r = post(d.port, path, &agent, body).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{path}: {}", r.body);
        assert!(r.body.contains("decoy-victim"), "the refusal names the real role: {path}: {}", r.body);
    }
    let after = k2_core::skin::find_role(&victim.id).unwrap().expect("the victim role");
    assert_eq!((after.name.as_str(), after.rooms.clone()), ("decoy-victim", vec![b.id.clone()]), "the victim is unchanged");
    let guest = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == "decoy-guest").expect("guest");
    assert_eq!(guest.role_id, None, "no role handed out");
    k2_daemon::apps::alerts::set_sink(None);
}

/// Copy of box logins onto an App leaves out the box guests another
/// workspace's agent added (security review E); its own workspace's Apps
/// get them, the owner's guests go everywhere as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn copy_host_leaves_out_box_guests_another_workspaces_agent_added() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let a = seed_ws(&env, "cpa");
    let b = seed_ws(&env, "cpb");
    let svc_a = app_row(&a, "cpappa");
    let svc_b = app_row(&b, "cpappb");
    allow_manage(&a, true);
    let r = post(d.port, "/cli/skin/users", &passport(&a), json!({"username": "cp-agentguest"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_core::skin::set_host_login("cp-agentguest", true).expect("host login");
    box_login_user("cp-ownerguest");
    let names = |v: &Value, key: &str| -> Vec<String> {
        v[key].as_array().unwrap_or_else(|| panic!("no {key}: {v}")).iter().map(|x| x.as_str().expect("name").to_string()).collect()
    };
    // Another workspace's App: the owner's guest only.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": svc_b, "dryRun": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let added = names(&v, "added");
    assert!(added.contains(&"cp-ownerguest".to_string()) && !added.contains(&"cp-agentguest".to_string()), "{v}");
    assert_eq!(names(&v, "skippedOtherWorkspaceAgent"), ["cp-agentguest"], "{v}");
    // Its own workspace's App: both.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": svc_a, "dryRun": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let added = names(&v, "added");
    assert!(added.contains(&"cp-ownerguest".to_string()) && added.contains(&"cp-agentguest".to_string()), "{v}");
    assert!(v.get("skippedOtherWorkspaceAgent").is_none(), "{v}");
    // No workspace known: none of the agents' guests.
    let rep = k2_core::skin::copy_host_grants_to_app(&svc_a, "test", true).unwrap();
    assert!(rep.skipped_other_workspace_agent.contains(&"cp-agentguest".to_string()) && !rep.added.contains(&"cp-agentguest".to_string()), "{rep:?}");
    k2_daemon::apps::alerts::set_sink(None);
}

/// C: first write verbs are one digest per App per window, and nothing at
/// all for what one of the App's test guests did (someone testing it; the
/// owner made the guest: agents make none).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_write_verbs_fold_into_one_digest_and_skip_the_agents_own_tests() {
    let env = setup(&[("K2_APPS_FIRST_USE_DIGEST_SECS", "1")]);
    let seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "digws");
    let svc = app_row(&ws, "digapp");
    let svc2 = app_row(&ws, "digapp2");
    k2_core::skin::add_principal_for_app("realu", None, &svc, None, "owner").unwrap();
    k2_core::skin::set_principal_password("realu", Some("real-guest-pw-1")).unwrap();
    let (pass, port) = open_link("digapp", &svc, &ws);
    let p = pass.clone();
    let (login, catchup) = blocking(move || {
        let a = on_link(port, &link_post("/cli/skin/login", Some(&p), &json!({"username": "realu", "password": "real-guest-pw-1"})));
        let b = on_link(port, &link_post("/cli/thread/catchup", Some(&p), &json!({"threads": []})));
        (a, b)
    })
    .await;
    assert_eq!(login.0, 200, "{login:?}");
    assert_eq!(catchup.0, 200, "{catchup:?}");
    let digests = |app: &'static str| {
        let s = seen.clone();
        move || -> Vec<String> {
            s.lock().unwrap().iter().filter(|(_, t)| t.starts_with(&format!("[k2 apps] {app} used"))).map(|(_, t)| t.clone()).collect()
        }
    };
    let mine = digests("digapp");
    wait_until("the digest for digapp", 20, || !mine().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let got = mine();
    assert_eq!(got.len(), 1, "one wake for both first verbs: {got:?}");
    for want in ["2 write verbs", "POST /cli/skin/login", "POST /cli/thread/catchup", "k2 app audit digapp"] {
        assert!(got[0].contains(want), "{want} missing: {}", got[0]);
    }

    // Testing as one of the App's test guests (the owner's): no wake.
    let r = post(d.port, "/cli/app/test-guest/add", OWNER, json!({"app": "digapp2", "username": "qa-dig"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let pw = r.json()["password"].as_str().expect("password").to_string();
    let (pass2, port2) = open_link("digapp2", &svc2, &ws);
    let login = blocking(move || on_link(port2, &link_post("/cli/skin/login", Some(&pass2), &json!({"username": "qa-dig", "password": pw})))).await;
    assert_eq!(login.0, 200, "{login:?}");
    let a = audit_rows(d.port, "digapp2", "", 1).await;
    assert!(
        a["calls"][0]["onBehalfOf"].as_str().unwrap_or_else(|| panic!("no onBehalfOf: {a}")).starts_with("guest:"),
        "the login names who signed in: {a}"
    );
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let theirs = digests("digapp2")();
    assert!(theirs.is_empty(), "no wake for a test guest's calls: {theirs:?}");
    gateway::close("digapp", None, true);
    gateway::close("digapp2", None, true);
    k2_daemon::apps::alerts::set_sink(None);
}
