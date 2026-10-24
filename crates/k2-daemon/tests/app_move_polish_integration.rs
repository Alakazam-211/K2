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
    let ws = seed_ws(&env, "agent-nsi");
    let code = ws.path.join("bff");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(
        code.join("server.mjs"),
        "const pass = readFileSync(process.env.K2_PASS_FILE);\nconst data = process.env.K2_APP_DATA;\nconst tok = process.env.K2_OWNER_TOKEN;\n",
    )
    .unwrap();
    let svc = service(&ws, "agents", "node server.mjs", &code, 47_101);
    k2_daemon::vault::publish_env::set(&svc, Some("clean"), None).unwrap();

    let r = get(d.port, "/cli/app/warnings", OWNER, "app=agents").await;
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
        assert!(!got.iter().any(|(c, s)| c == "k2_internal_env" && s == "breaks" && m.contains(n)), "{n} is not a break");
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
    let other = seed_ws(&env, "other");
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
    for want in ["ambiguous", "agent-nsi/jobsbff", "other/jobsbff"] {
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
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=other%2Fagentnsi").await;
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
    let nsi = seed_ws(&env, "nsi");
    let other = seed_ws(&env, "other");
    app_row(&nsi, "agentnsi");
    app_row(&other, "otherapp");
    service(&nsi, "agents", "node server.js", &nsi.path, 47_201);
    service(&other, "theirs", "node server.js", &other.path, 47_202);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::claim(&conn, "theirextra", &other.id).unwrap();
        k2_core::db::schema::SubdomainWorkspace::claim(&conn, "nsiextra", &nsi.id).unwrap();
    }
    let mine = passport(&nsi);
    allow_manage(&nsi, true);
    let add = |who: String, label: &'static str| {
        let port = d.port;
        async move { post(port, "/cli/app/domain/add", &who, json!({"app": "agentnsi", "label": label})).await }
    };

    // The --cmd service's own label: that's label take.
    let r = add(mine.clone(), "agents").await;
    assert_eq!((r.status, r.code()), (409, "use_label_take".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap().contains("k2 app label take agentnsi agents"), "{}", r.body);
    // Another workspace's service label or attributed subdomain: refused, naming its agent.
    for label in ["theirs", "theirextra"] {
        let r = add(mine.clone(), label).await;
        assert_eq!((r.status, r.code()), (403, "label_owned_by_other_workspace".to_string()), "{label}: {}", r.body);
    }
    // Another App's own label; a reserved door.
    let r = add(mine.clone(), "otherapp").await;
    assert_eq!((r.status, r.code()), (409, "label_serves_another_app".to_string()), "{}", r.body);
    let r = add(mine.clone(), "meet").await;
    assert_eq!((r.status, r.code()), (400, "reserved_label".to_string()), "{}", r.body);
    // A bad label; the App's own label is already its own.
    let r = add(mine.clone(), "no_underscores").await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = add(mine.clone(), "agentnsi").await;
    assert_eq!((r.status, r.json()["already"].clone()), (200, json!(true)), "{}", r.body);
    // Its own extra subdomain (or a free one) gets as far as the App's
    // health: this one never started.
    for label in ["nsiextra", "freshlabel"] {
        let r = add(mine.clone(), label).await;
        assert_eq!((r.status, r.code()), (409, "app_not_healthy".to_string()), "{label}: {}", r.body);
    }
    // A full host name names the same label.
    let r = post(d.port, "/cli/app/domain/add", &mine, json!({"app": "agentnsi", "label": format!("theirs.box.{}", k2_core::tunnel::config::SUBDOMAIN_HOST)})).await;
    assert_eq!(r.code(), "label_owned_by_other_workspace", "{}", r.body);
    // Another workspace's agent never sees this App.
    allow_manage(&other, true);
    let r = post(d.port, "/cli/app/domain/add", &passport(&other), json!({"app": "agentnsi", "label": "x1"})).await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    // Without "Build and run Apps" the change is held for the owner.
    allow_manage(&nsi, false);
    let r = add(mine.clone(), "nsiextra").await;
    assert_eq!((r.status, r.json()["status"].clone()), (202, json!("held")), "{}", r.body);
    allow_manage(&nsi, true);

    // rm of something never attached; of a taken label (that's label return).
    let r = post(d.port, "/cli/app/domain/rm", &mine, json!({"app": "agentnsi", "label": "nsiextra"})).await;
    assert_eq!((r.status, r.code()), (404, "domain_not_attached".to_string()), "{}", r.body);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut cfg = k2_core::app_grace::config::load(&conn, "agentnsi").unwrap();
        cfg.labels.push(k2_core::app_grace::config::TakenLabel {
            label: "agents".into(),
            service_id: "svc-agents".into(),
            legacy_port: 47_201,
            taken_at: 1,
            taken_by: "agent:nsi".into(),
        });
        cfg.subdomains.push(k2_core::app_grace::config::AttachedSubdomain {
            label: "nsiextra".into(),
            added_at: 2,
            added_by: "agent:nsi".into(),
            previous_target: Some("127.0.0.1:47201".into()),
        });
        conn.execute("UPDATE apps SET config_json = ?2 WHERE app_id = ?1", rusqlite::params!["agentnsi", serde_json::to_string(&cfg).unwrap()]).unwrap();
    }
    let r = post(d.port, "/cli/app/domain/rm", &mine, json!({"app": "agentnsi", "label": "agents"})).await;
    assert_eq!(r.code(), "domain_not_attached");
    assert!(r.body.contains("k2 app label return agentnsi agents"), "{}", r.body);
    // The list: its own label, the taken one, the attached one.
    let r = get(d.port, "/cli/app/domains", &mine, "app=agentnsi").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["own"]["label"], "agentnsi");
    assert_eq!(v["taken"][0]["label"], "agents");
    assert_eq!((v["attached"][0]["label"].clone(), v["attached"][0]["previousTarget"].clone()), (json!("nsiextra"), json!("127.0.0.1:47201")));
    // Attached hostnames join the App's public origins (with its labels).
    let mut want = vec!["agents".to_string(), "nsiextra".to_string()];
    want.sort();
    let mut got = k2_daemon::apps::labels::taken("agentnsi");
    got.sort();
    assert_eq!(got, want);
    // POST-only.
    for p in ["/cli/app/domain/add", "/cli/app/domain/rm"] {
        let r = get(d.port, p, OWNER, "app=agentnsi&label=x").await;
        assert_eq!(r.status, 405, "{p}: {}", r.body);
    }
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
    let nsi = seed_ws(&env, "nsi");
    let rooms = seed_ws(&env, "sales");
    let stranger = seed_ws(&env, "stranger");
    let old = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &nsi.id, "agents", "node server.js", &nsi.path.to_string_lossy(), 47_301, "tunnel", "running", "cmd", "")
            .unwrap()
            .id
    };
    let app_svc = app_row(&nsi, "agentnsi");
    // The old BFF hands its guests the `sales` room (its guests-rooms grant).
    k2_core::skin::guest_rooms::grant(&old, &[rooms.id.clone()], "owner").unwrap();
    box_login_user("ada");
    box_login_user("bo");
    let agent = passport(&nsi);
    allow_manage(&nsi, false);

    // The plan, as the service's own agent (no switch yet).
    let r = get(d.port, "/cli/app/move-plan", &agent, "service=agents&app=agentnsi").await;
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
    assert_eq!((gr["state"].clone(), gr["rooms"].clone(), gr["roomHandles"].clone()), (json!("todo"), json!([rooms.id]), json!(["sales"])), "{gr}");
    assert_eq!(step(&p, "label_take")["state"], "blocked", "waits for a live App");
    assert_eq!(step(&p, "stop_old")["command"], "k2 publish stop agents");
    assert_eq!(p["ownerStepsTodo"], json!(["build_switch", "copy_host", "guest_rooms"]));
    // The owner sees the names.
    let r = get(d.port, "/cli/app/move-plan", OWNER, "service=nsi%2Fagents").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(step(&r.json(), "copy_host")["users"], json!(["ada", "bo"]));
    assert_eq!(r.json()["app"]["id"], "agentnsi", "the workspace's only App by default");
    // Another workspace's agent: no such service.
    let r = get(d.port, "/cli/app/move-plan", &passport(&stranger), "service=agents").await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    // An App is not a --cmd service.
    let r = get(d.port, "/cli/app/move-plan", OWNER, "service=agentnsi").await;
    assert_eq!((r.status, r.code()), (409, "not_a_cmd_service".to_string()), "{}", r.body);

    // ONE request for the three owner steps.
    let r = post(d.port, "/cli/app/move-plan/request", &agent, json!({"service": "agents", "app": "agentnsi", "reason": "moving the BFF"})).await;
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
        k2_core::deploy::audit::list_for(&conn, k2_core::deploy::audit::SUBJECT_APP, "agentnsi", 200)
            .unwrap()
            .into_iter()
            .filter(|a| a.event == "move_plan_step")
            .count()
    };
    assert_eq!(audited, 3);
    assert!(seen.lock().unwrap().iter().any(|(w, t)| w == &nsi.id && t.contains(&req) && t.contains("approved")), "the agent hears");
    // Nothing left for the owner.
    let r = post(d.port, "/cli/app/move-plan/request", &agent, json!({"service": "agents", "app": "agentnsi"})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("nothing_to_request")), "{}", r.body);

    // The approval copies exactly what the request saw: a user added in
    // between stops the step, nothing is copied.
    box_login_user("cy");
    let r = post(d.port, "/cli/app/move-plan/request", &agent, json!({"service": "agents", "app": "agentnsi"})).await;
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
