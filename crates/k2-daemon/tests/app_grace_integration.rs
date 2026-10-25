//! The 0.46.0 App grace release, end to end through the REAL dispatcher
//! (Rosson's cross-over plan, 2026-10-14: 0.46.0 adds the new ways and
//! removes nothing; the cut-over is now 0.46.4, k2_core::app_grace::CUTOVER_RELEASE).
//!
//! - warnings per published `--cmd` service (env reads, K2 files, PATH);
//! - clean mode: on narrows (an agent may), off is held for the owner;
//! - the App-scoped key: the owner mints with a confirm, an agent's mint is
//!   held and the owner approves in person; the `/v1/apps/<id>/…` door
//!   (users, rooms within the ceiling, the only-this-App rule, fetch with
//!   a secret by name, the uniform 404);
//! - `/cli/publish/run` mapping (§8.2): a notice in 0.46.0, held when the
//!   gate is enforced; the admin switches through `agent-access/set`;
//! - S13 `/cli/app/config`: narrowing applies now, widening is held.
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB, a vault on a file key
//! under the temp home. No real agent CLI runs (alerts go to a sink).

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use app_fixture::*;
use k2_core::test_env::EnvVar;
use serde_json::{json, Value};

/// A `--cmd` service row (never started).
fn service(ws: &Ws, name: &str, cmd: &str, folder: &std::path::Path) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, cmd, &folder.to_string_lossy(), 47_001, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

fn quiet_alerts() -> Arc<std::sync::Mutex<Vec<(String, String)>>> {
    let seen: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
    let s = seen.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |ws: &str, text: &str| {
        s.lock().unwrap().push((ws.to_string(), text.to_string()));
    })));
    seen
}

async fn v1(port: u16, method: &str, path: &str, key: &str, body: Option<Value>) -> Resp {
    let path = path.to_string();
    let key = key.to_string();
    let method = method.to_string();
    tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let payload = body.map(|b| b.to_string());
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {key}\r\n");
        if let Some(p) = &payload {
            head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", p.len()));
        } else if method == "POST" {
            head.push_str("Content-Length: 0\r\n");
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).unwrap();
        if let Some(p) = payload {
            s.write_all(p.as_bytes()).unwrap();
        }
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).expect("status");
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        Resp { status, body }
    })
    .await
    .unwrap()
}

fn confirm(action: &str, bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint(action, Some(bound), "owner", "local-auth")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warnings_name_what_breaks_and_clean_mode_narrows_now_widens_later() {
    let env = setup(&[]);
    let _db_url = EnvVar::set("DATABASE_URL", "postgres://daemon-only");
    let seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "nsi");
    let other = seed_ws(&env, "elsewhere");
    let app = ws.path.join("jobs");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("server.mjs"),
        "const db = process.env.DATABASE_URL;\nconst tok = process.env.K2_OWNER_TOKEN;\nconst port = readFileSync(`${os.homedir()}/.k2/heartbeat.port`);\nconst p = process.env.PORT;\n",
    )
    .unwrap();
    let svc = service(&ws, "jobsw", "node server.mjs", &app);
    k2_daemon::vault::publish_env::set(&svc, Some("legacy"), None).unwrap();

    // The owner sees what breaks.
    let r = get(d.port, "/cli/app/warnings", OWNER, "app=nsi/jobsw").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["cutover"], "0.46.4");
    assert_eq!(v["readyForCutover"], false);
    let codes: Vec<String> = v["warnings"].as_array().unwrap().iter().map(|w| w["code"].as_str().unwrap().to_string()).collect();
    for want in ["legacy_env", "inherited_env_read", "k2_internal_env", "daemon_port_file"] {
        assert!(codes.iter().any(|c| c == want), "{want} missing from {codes:?}");
    }
    let db_warning = v["warnings"].as_array().unwrap().iter().find(|w| w["code"] == "inherited_env_read").unwrap();
    assert!(db_warning["message"].as_str().unwrap().contains("DATABASE_URL"), "{db_warning}");
    assert!(
        !v["warnings"].as_array().unwrap().iter().any(|w| w["message"].as_str().unwrap().contains(" PORT,")),
        "PORT is provided in clean mode: no warning"
    );
    // Owner-token tracing is Linux only; a Mac says so (Rosson 2026-10-16).
    let want_tracing = if cfg!(target_os = "linux") { "on" } else if cfg!(target_os = "macos") { "not available on macOS" } else { "not available on this OS (Linux only)" };
    assert_eq!(v["ownerTokenAttribution"], want_tracing, "{v}");
    assert_eq!(v.get("ownerTokenAttributionNote").is_some(), !cfg!(target_os = "linux"), "{v}");
    // Its own agent sees it; another workspace's agent gets app_not_found.
    let mine = passport(&ws);
    let r = get(d.port, "/cli/app/warnings", &mine, "app=jobsw").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let theirs = passport(&other);
    let r = get(d.port, "/cli/app/warnings", &theirs, "app=jobsw").await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    let r = get(d.port, "/cli/app/warnings", &theirs, "").await;
    assert_eq!(r.json()["services"].as_array().unwrap().len(), 0, "an agent lists only its own workspace's services");
    assert_eq!(r.json()["ownerTokenAttribution"], want_tracing, "an empty list still says where tracing stands");
    let r = get(d.port, "/cli/app/warnings", OWNER, "").await;
    assert!(r.json()["services"].as_array().unwrap().iter().any(|s| s["app"]["serviceId"] == svc.as_str()));

    // Clean mode on: a narrowing, so the agent may.
    let r = post(d.port, "/cli/app/clean-mode", &mine, json!({"app": "jobsw", "on": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!((r.json()["cleanMode"].clone(), r.json()["envMode"].clone()), (json!(true), json!("clean")));
    assert_eq!(r.json()["ownerTokenAttribution"], want_tracing, "{}", r.body);
    assert!(k2_daemon::vault::publish_env::is_clean_mode(&svc));
    // Off widens: the agent's request is held for the owner.
    let r = post(d.port, "/cli/app/clean-mode", &mine, json!({"app": "jobsw", "on": false})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(r.json()["status"], "held");
    let req = r.json()["request"]["id"].as_str().unwrap().to_string();
    assert!(k2_daemon::vault::publish_env::is_clean_mode(&svc), "held: nothing changed");
    let r = get(d.port, "/cli/app/approvals", &mine, "").await;
    assert_eq!(r.json()["requests"][0]["id"], req.as_str());
    let r = get(d.port, "/cli/app/approvals", &theirs, "").await;
    assert!(r.json()["requests"].as_array().unwrap().is_empty(), "another workspace sees nothing");
    // An agent can't approve; the owner denies, the agent is told.
    let r = post(d.port, "/cli/app/approve", &mine, json!({"request": req})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = post(d.port, "/cli/app/deny", OWNER, json!({"request": req, "reason": "keep it clean"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["request"]["status"], "denied");
    assert!(seen.lock().unwrap().iter().any(|(w, t)| w == &ws.id && t.contains("denied") && t.contains("keep it clean")));
    let r = post(d.port, "/cli/app/deny", OWNER, json!({"request": req})).await;
    assert_eq!(r.status, 409, "decided once: {}", r.body);
    // The owner turns it off directly.
    let r = post(d.port, "/cli/app/clean-mode", OWNER, json!({"app": "nsi/jobsw", "on": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!k2_daemon::vault::publish_env::is_clean_mode(&svc));
    assert_eq!(k2_daemon::vault::publish_env::get(&svc).mode, "clean", "off never gives the inherited env back");
    // POST-only.
    for p in k2_daemon::apps::grace::POST_ROUTES {
        let r = get(d.port, p, OWNER, "app=jobsw").await;
        assert_eq!(r.status, 405, "{p}: {}", r.body);
    }
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_key_is_minted_in_person_held_for_agents_and_opens_only_its_app() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "nsikey");
    let room_a = seed_ws(&env, "rooma");
    let room_b = seed_ws(&env, "roomb");
    let app = ws.path.join("jobs");
    std::fs::create_dir_all(&app).unwrap();
    let svc = service(&ws, "jobs", "node server.mjs", &app);
    let other_svc = service(&ws, "sister", "node other.mjs", &app);
    let mint = json!({"app": svc, "scopes": ["users", "rooms"], "rooms": [room_a.handle], "deliver": "show"});

    // Owner token alone: refused, and told which App the confirm binds.
    let r = post(d.port, "/cli/app/key/mint", OWNER, mint.clone()).await;
    assert_eq!((r.status, r.code()), (403, "vault_confirm_required".to_string()), "{}", r.body);
    assert_eq!(r.json()["error"]["serviceId"], svc.as_str());
    // A confirm for another App doesn't fit.
    let mut wrong = mint.clone();
    wrong["confirmToken"] = json!(confirm("app-key-mint", &other_svc));
    let r = post(d.port, "/cli/app/key/mint", OWNER, wrong).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let mut ok = mint.clone();
    ok["confirmToken"] = json!(confirm("app-key-mint", &svc));
    let r = post(d.port, "/cli/app/key/mint", OWNER, ok).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let key = r.json()["key"].as_str().unwrap().to_string();
    let key_id = r.json()["id"].as_str().unwrap().to_string();
    assert!(key.starts_with("k2sk_"));
    assert!(k2_core::api_keys::resolve_api_key(&key).is_none(), "an App key is never an ordinary /v1 principal");

    // An agent's mint is held; the owner approves in person; K2 delivers
    // it to the vault (nobody sees it).
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let r = post(d.port, "/cli/app/key/mint", &agent, json!({"app": "jobs", "scopes": "users,fetch"})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let req = r.json()["request"]["id"].as_str().unwrap().to_string();
    let r = post(d.port, "/cli/app/key/mint", &agent, json!({"app": "jobs", "scopes": "users", "deliver": "show"})).await;
    assert_eq!(r.status, 403, "an agent never sees a key: {}", r.body);
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": req})).await;
    assert_eq!((r.status, r.code()), (403, "vault_confirm_required".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": req, "confirmToken": confirm("app-approve", &req)})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["status"], "approved");
    assert!(r.json()["result"]["key"].is_null(), "approved keys go to the vault, never the response");
    let secret = r.json()["result"]["delivered"]["vaultSecret"].as_str().unwrap().to_string();
    let vault = k2_daemon::vault::get().unwrap();
    let grants = vault.grants_to(&k2_vault::Consumer::published_service(svc.clone())).unwrap();
    assert!(grants.iter().any(|(g, n)| n == &secret && g.file_name.as_deref() == Some("K2_APP_KEY")), "{grants:?}");
    let r = get(d.port, "/cli/app/keys", &agent, "app=jobs").await;
    assert_eq!(r.json()["keys"].as_array().unwrap().len(), 2, "{}", r.body);
    assert!(!r.body.contains("k2sk_"), "a key list never carries a key");

    // The door: users and rooms within the ceiling.
    let base = format!("/v1/apps/{svc}");
    let r = v1(d.port, "GET", &format!("{base}/whoami"), &key, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = v1(d.port, "POST", &format!("{base}/users"), &key, Some(json!({"username": "guest1", "rooms": [room_a.handle]}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["user"]["rooms"][0], room_a.id.as_str());
    // Round-2 review B1: a guest an App key made is tied (the key holder
    // may set its password): not "a person" for card answers.
    let tie_of = |name: &str| {
        let id = k2_core::skin::list_principals().unwrap().into_iter().find(|p| p.username == name).expect("the guest").id;
        k2_core::skin::creators::agent_tie(&id).expect("tie lookup")
    };
    assert_eq!(tie_of("guest1"), Some(ws.id.clone()), "an App key's guest is tied to the App's workspace");
    let r = v1(d.port, "POST", &format!("{base}/users/rooms"), &key, Some(json!({"username": "guest1", "rooms": [room_b.handle]}))).await;
    assert_eq!((r.status, r.code()), (403, "outside_ceiling".to_string()), "{}", r.body);
    let r = v1(d.port, "POST", &format!("{base}/users/rooms"), &key, Some(json!({"username": "guest1", "rooms": []}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = v1(d.port, "GET", &format!("{base}/users"), &key, None).await;
    assert_eq!(r.json()["users"].as_array().unwrap().len(), 1, "{}", r.body);
    let r = v1(d.port, "POST", &format!("{base}/users/disable"), &key, Some(json!({"username": "guest1"}))).await;
    assert_eq!(r.json()["enabled"], false, "{}", r.body);

    // Only-this-App: a box login that also has this App can't be reset or
    // removed as a person through the key.
    k2_core::skin::add_principal_with_email("boxperson", Some("box@example.com")).unwrap();
    k2_core::skin::set_principal_password("boxperson", Some("correct horse battery 1")).unwrap();
    k2_core::skin::set_app_grant_rooms("boxperson", &svc, &[], false, "owner").unwrap();
    let r = v1(d.port, "POST", &format!("{base}/users/password-reset"), &key, Some(json!({"username": "boxperson"}))).await;
    assert_eq!((r.status, r.code()), (403, "not_only_this_app".to_string()), "{}", r.body);
    let r = v1(d.port, "POST", &format!("{base}/users/remove"), &key, Some(json!({"username": "boxperson"}))).await;
    assert_eq!(r.json()["removed"], "app_access", "{}", r.body);
    assert!(k2_core::skin::principal_exists("boxperson").unwrap(), "the person keeps their box login");
    let r = v1(d.port, "POST", &format!("{base}/users"), &key, Some(json!({"username": "guest2", "email": "g2@example.com", "password": "another long pass 9"}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // agent-app-users review item 4: the key never gets the token; with no
    // set-password page or mail path nothing is minted or sent.
    let r = v1(d.port, "POST", &format!("{base}/users/password-reset"), &key, Some(json!({"username": "guest2"}))).await;
    assert_eq!((r.status, r.code()), (409, "no_set_password_page".to_string()), "{}", r.body);
    assert!(!r.body.contains("k2inv_") && r.json()["token"].is_null(), "{}", r.body);
    let r = v1(d.port, "POST", &format!("{base}/users/remove"), &key, Some(json!({"username": "guest2"}))).await;
    assert_eq!(r.json()["removed"], "person", "{}", r.body);

    // The uniform 404: another App, a scope the key lacks, the owner token,
    // an ordinary key, no key, an unknown verb.
    let (_, ordinary) = k2_core::api_keys::create_api_key("plain", None, Some("*"), None, None, None).unwrap();
    for (path, cred, method) in [
        (format!("/v1/apps/{other_svc}/whoami"), key.as_str(), "GET"),
        (format!("{base}/fetch"), key.as_str(), "POST"),
        (format!("{base}/whoami"), OWNER, "GET"),
        (format!("{base}/whoami"), ordinary.as_str(), "GET"),
        (format!("{base}/whoami"), "", "GET"),
        (format!("{base}/roles"), key.as_str(), "GET"),
    ] {
        let r = v1(d.port, method, &path, cred, if method == "POST" { Some(json!({})) } else { None }).await;
        assert_eq!(r.status, 404, "{path} with {cred:.8}: {}", r.body);
    }
    // Revoke: narrowing; the key stops at once.
    let r = post(d.port, "/cli/app/key/revoke", &agent, json!({"id": key_id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = v1(d.port, "GET", &format!("{base}/whoami"), &key, None).await;
    assert_eq!(r.status, 404, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_calls_out_with_a_secret_by_name_and_the_app_never_sees_it() {
    const VAL: &str = "sk_live_grace_0460_never_shown";
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "fetchws");
    let app = ws.path.join("bff");
    std::fs::create_dir_all(&app).unwrap();
    let svc = service(&ws, "bff", "node bff.mjs", &app);
    // An upstream that echoes the request head.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let up = l.local_addr().unwrap().port();
    let conns = Arc::new(AtomicUsize::new(0));
    let c2 = conns.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            c2.fetch_add(1, Ordering::SeqCst);
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            let head = String::from_utf8_lossy(&got).to_string();
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", head.len(), head);
            let _ = s.write_all(resp.as_bytes());
        }
    });
    {
        use k2_vault::{Actor, Consumer, Delivery, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore, Selector};
        let v = k2_daemon::vault::get().unwrap();
        let mut p = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "STRIPE_KEY", Kind::ApiKey);
        p.hosts = vec![format!("http://127.0.0.1:{up}")];
        v.put(p, SecretBytes::from_slice(VAL.as_bytes()), &Actor::Owner).unwrap();
        v.grant(
            GrantRequest {
                selector: Selector::user(Scope::workspace(ws.id.clone()), "STRIPE_KEY"),
                consumer: Consumer::published_service(svc.clone()),
                delivery: Delivery::Fetch,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .unwrap();
        let mut q = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "NOT_GRANTED", Kind::ApiKey);
        q.hosts = vec![format!("http://127.0.0.1:{up}")];
        v.put(q, SecretBytes::from_slice(b"sk_not_granted_value"), &Actor::Owner).unwrap();
    }
    let r = post(
        d.port,
        "/cli/app/key/mint",
        OWNER,
        json!({"app": svc, "scopes": ["fetch"], "deliver": "show", "confirmToken": confirm("app-key-mint", &svc)}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let key = r.json()["key"].as_str().unwrap().to_string();
    let url = format!("http://127.0.0.1:{up}/v1/charges");
    let r = v1(d.port, "POST", &format!("/v1/apps/{svc}/fetch"), &key, Some(json!({"secret": "STRIPE_KEY", "url": url}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains(VAL), "the value never comes back: {}", r.body);
    let echoed = r.json()["body"].as_str().unwrap().to_string();
    assert!(echoed.contains("[k2v:STRIPE_KEY]") || echoed.to_ascii_lowercase().contains("[k2v:stripe_key]"), "{echoed}");
    assert_eq!(conns.load(Ordering::SeqCst), 1);
    // A secret not granted to this App: refused before any connection.
    let r = v1(d.port, "POST", &format!("/v1/apps/{svc}/fetch"), &key, Some(json!({"secret": "NOT_GRANTED", "url": url}))).await;
    assert_eq!((r.status, r.code()), (403, "not_granted".to_string()), "{}", r.body);
    // Another host: refused before any connection.
    let r = v1(d.port, "POST", &format!("/v1/apps/{svc}/fetch"), &key, Some(json!({"secret": "STRIPE_KEY", "url": "http://127.0.0.1:1/x"}))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(conns.load(Ordering::SeqCst), 1);

    // One host policy for both ways out (S15): once the service is a walled
    // runtime App, the key proxy passes the App's egress allowlist first.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: "bff",
                service_id: &svc,
                seed_id: None,
                owner_workspace: &ws.id,
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
    }
    let r = v1(d.port, "POST", &format!("/v1/apps/{svc}/fetch"), &key, Some(json!({"secret": "STRIPE_KEY", "url": url}))).await;
    assert_eq!((r.status, r.code()), (403, "egress_denied".to_string()), "not on the App's allowlist: {}", r.body);
    assert!(r.body.contains("k2 app egress add bff"), "{}", r.body);
    assert_eq!(conns.load(Ordering::SeqCst), 1, "refused before any connection");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let spec = k2_core::deploy::egress::parse_spec(&format!("127.0.0.1:{up}")).unwrap();
        k2_core::deploy::egress::allow(&conn, "bff", &[spec], k2_core::deploy::egress::SOURCE_REQUEST, "owner").unwrap();
    }
    k2_daemon::apps::egress_proxy::reload("bff");
    // The real proxy resolver refuses loopback; the fake upstream lives
    // there, so the proxy's vetting step answers with it (the policy step
    // above is the one under test).
    k2_daemon::apps::egress_proxy::set_resolver(Some(Arc::new(|_: &str, p: u16| Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], p))]))));
    let r = v1(d.port, "POST", &format!("/v1/apps/{svc}/fetch"), &key, Some(json!({"secret": "STRIPE_KEY", "url": url}))).await;
    k2_daemon::apps::egress_proxy::set_resolver(None);
    assert_eq!(r.status, 200, "on the allowlist now: {}", r.body);
    assert_eq!(conns.load(Ordering::SeqCst), 2);
    let hits: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::egress::list_hosts(&conn, "bff").unwrap().iter().map(|h| h.hits).sum()
    };
    assert_eq!(hits, 1, "the key proxy's call counts on the same host list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_run_mapping_notice_now_held_when_enforced_and_the_admin_switches() {
    require_python3();
    let env = setup(&[("K2_PUBLISH_PROBE_MS", "8000")]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "pubws");
    let agent = passport(&ws);
    // Unique, outside the ephemeral range (not bind(:0)-then-drop).
    let free = k2_core::test_env::loopback_test_port;
    let run = |name: &str, port: u16| json!({"name": name, "cmd": "exec python3 -m http.server $PORT --bind 127.0.0.1", "port": port, "noTunnel": true});

    // Switches: only the owner raises; switch 2 needs switch 1; the old
    // "…and publish open Apps" is retired (N1).
    let r = post(d.port, "/cli/agent-access/set", &agent, json!({"workspace": ws.id, "toggle": "apps_build", "value": true})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_manage_full", "value": true})).await;
    assert_eq!((r.status, r.code()), (409, "parent_switch_off".to_string()), "{}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_open", "value": true})).await;
    assert_eq!((r.status, r.code()), (410, "open_switch_retired".to_string()), "{}", r.body);

    // No open path in this harness (no engine, no root helper): an agent's
    // new --cmd runs as in 0.46.0, with the notice of what the gate asks
    // for (it holds only where the run would be an open-path App).
    let r = post(d.port, "/cli/publish/run", &agent, run("svc-a", free())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["crossover"]["wouldHold"], true, "{}", r.body);
    assert_eq!(r.json()["crossover"]["from"], "0.46.1");
    assert_eq!((r.json()["crossover"]["mode"].clone(), r.json()["crossover"]["held"].clone()), (json!("warn"), json!(false)), "{}", r.body);
    assert!(
        r.json()["crossover"]["message"].as_str().unwrap().starts_with("svc-a ran unwalled, without the open-path gate"),
        "{}",
        r.body
    );
    assert_eq!(r.json()["wall"]["path"], "legacy", "it says why it isn't walled: {}", r.body);
    // A re-run of the now existing service without "Build and run Apps"
    // waits for the owner; with it, it runs at once.
    let r = post(d.port, "/cli/publish/run", &agent, run("svc-a", free())).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "{}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_build", "value": true})).await;
    assert_eq!((r.status, r.json()["level"].clone()), (200, json!("build")), "{}", r.body);
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "svc-a"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/publish/run", &agent, run("svc-a", free())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json().get("crossover").is_none(), "a legacy re-run under switch 1 is free: {}", r.body);
    let r = post(d.port, "/cli/publish/rm", &agent, json!({"name": "svc-a"})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // K2_OPEN_APPS_GATE=enforce (the owner's choice): held even without
    // an open path; the owner approves and it runs as before.
    {
        let _gate = EnvVar::set("K2_OPEN_APPS_GATE", "enforce");
        let port = free();
        let r = post(d.port, "/cli/publish/run", &agent, run("svc-b", port)).await;
        assert_eq!(r.status, 202, "{}", r.body);
        assert_eq!(r.json()["kind"], "publish_run");
        let req = r.json()["request"]["id"].as_str().unwrap().to_string();
        let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": req, "confirmToken": confirm("app-approve", &req)})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["status"], "approved", "{}", r.body);
        assert_eq!(r.json()["result"]["name"], "svc-b");
        let r = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "svc-b", "project": ws.id})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        // A human runs at once.
        let r = post(d.port, "/cli/publish/run", OWNER, json!({"name": "svc-c", "project": ws.id, "cmd": "exec python3 -m http.server $PORT --bind 127.0.0.1", "port": free(), "noTunnel": true})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.json().get("crossover").is_none());
        let _ = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "svc-c", "project": ws.id})).await;
        // Both switches on: a new service from the agent still waits for
        // the person (no standing approval for the open path any more).
        let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_manage_full", "value": true})).await;
        assert_eq!((r.status, r.json()["level"].clone()), (200, json!("full")), "{}", r.body);
        let r = post(d.port, "/cli/publish/run", &agent, run("svc-d", free())).await;
        assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "{}", r.body);
    }
    // Switch 1 off turns switch 2 off.
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_build", "value": false})).await;
    assert_eq!((r.json()["agentsCanBuildApps"].clone(), r.json()["agentsCanManageAppsFull"].clone()), (json!(false), json!(false)), "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// Lane D's reported hole: an agent with "Allow this agent to manage Apps"
/// moving a guarded App to the unwalled process engine. A deploy flag
/// never lowers the wall (for anyone); the config route holds it for the
/// owner's in-person approval.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deploy_engine_process_never_lowers_a_guarded_apps_wall() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "engws");
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "eng-app", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "eng-app", "isolation": "container"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["app"]["isolation"], "container");
    allow_manage(&ws, true);
    let agent = passport(&ws);
    for who in [agent.as_str(), OWNER] {
        let r = post(d.port, "/cli/app/deploy", who, json!({"app": "eng-app", "engine": "process", "folder": ws.path})).await;
        assert_eq!((r.status, r.code()), (409, "widening_held".to_string()), "{}", r.body);
        assert!(r.body.contains("k2 app config eng-app --isolation process"), "{}", r.body);
    }
    // The agent's config ask is held for the owner (always per event).
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "eng-app", "isolation": "process"})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(r.json()["held"]["request"]["kind"], "config", "{}", r.body);
    let req = r.json()["held"]["request"]["id"].as_str().unwrap().to_string();
    let r = post(d.port, "/cli/app/approve", &agent, json!({"request": req, "confirmToken": confirm("app-approve", &req)})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "an agent never approves: {}", r.body);
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=eng-app").await;
    assert_eq!(r.json()["app"]["isolation"], "container", "still walled: {}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s13_config_applies_narrowing_now_and_holds_widening() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "cfgws");
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "cfg-app", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    allow_manage(&ws, true);
    let agent = passport(&ws);

    // One call: the memory change applies, the new egress host is held as
    // its own egress request (the S15 host list is the one egress truth).
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "cfg-app", "memory": "1G", "egressAdd": ["api.stripe.com:443"]})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(r.json()["app"]["memoryLimitBytes"], 1i64 << 30, "narrowing applied now: {}", r.body);
    let held = r.json()["egressHeld"]["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(r.json()["egressHeld"]["request"]["kind"], "egress", "{}", r.body);
    assert_eq!(r.json()["egressHeld"]["request"]["hosts"], json!(["api.stripe.com:443"]), "{}", r.body);
    assert!(r.json().get("held").is_none(), "nothing else was held: {}", r.body);
    // Open-path features loosen the wall: held for the person from every
    // caller, with any switch (the open switch is retired, N1); the
    // owner's approval applies it.
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "isolation": "container"})).await;
    assert_eq!(r.status, 200, "raising the wall narrows: {}", r.body);
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "cfg-app", "path": "open", "portAdd": "5000/udp"})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_open", "value": true})).await;
    assert_eq!(r.status, 410, "{}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_manage_full", "value": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "cfg-app", "path": "open"})).await;
    assert_eq!(r.status, 202, "switch 2 never covers the open path: {}", r.body);
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "path": "open"})).await;
    assert_eq!(r.status, 202, "nor does the owner token alone: {}", r.body);
    let open_req = r.json()["held"]["request"]["id"].as_str().unwrap().to_string();
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": open_req, "confirmToken": confirm("app-approve", &open_req)})).await;
    assert_eq!(r.json()["status"], "approved", "{}", r.body);
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=cfg-app").await;
    assert_eq!(r.json()["app"]["path"], "open", "{}", r.body);
    let r = post(d.port, "/cli/agent-access/set", OWNER, json!({"workspace": ws.id, "toggle": "apps_manage_full", "value": false})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Path open + isolation process is the process exception: never a
    // config change, even for the owner (k2 app exception, in person).
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "isolation": "process"})).await;
    assert_eq!((r.status, r.code()), (409, "engine_unavailable".to_string()), "{}", r.body);
    assert!(r.body.contains("k2 app exception"), "{}", r.body);
    // The per-event list waits even for the owner token.
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "dbRaw": "on"})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let raw_req = r.json()["held"]["request"]["id"].as_str().unwrap().to_string();
    // An owner-only widening: the agent proposes, the owner applies.
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "cfg-app", "dbPrivate": "on"})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "dbPrivate": "on"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Approving applies the held ops.
    for req in [&held, &raw_req] {
        let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": req, "confirmToken": confirm("app-approve", req)})).await;
        assert_eq!(r.json()["status"], "approved", "{}", r.body);
    }
    let (cfg, allowed) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let allowed: Vec<String> =
            k2_core::deploy::egress::policy(&conn, "cfg-app").unwrap().allowed.iter().map(k2_core::deploy::egress::HostSpec::display).collect();
        (k2_core::app_grace::config::load(&conn, "cfg-app").unwrap(), allowed)
    };
    assert_eq!(allowed, vec!["api.stripe.com:443"], "the approved host is on the enforced list");
    assert!(cfg.db.raw && cfg.db.private, "{cfg:?}");
    assert!(cfg.ports.is_empty(), "the held port request was never approved");
    // Disk ceilings: an agent tightens at once; its raise waits for the
    // owner; the owner raises at once.
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "cfg-app", "io": {"wbps": "20M"}})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["io"]["wbps"], 20i64 << 20, "{}", r.body);
    let r = post(d.port, "/cli/app/config", &agent, json!({"app": "cfg-app", "io": {"wbps": "80M"}})).await;
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(r.json()["held"]["request"]["kind"], "config", "{}", r.body);
    assert_eq!(r.json()["io"]["wbps"], 20i64 << 20, "held: nothing changed: {}", r.body);
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "io": "default"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["io"]["wbps"], 40i64 << 20, "K2's default ceiling: {}", r.body);
    // Bad values are refused before anything changes.
    let r = post(d.port, "/cli/app/config", OWNER, json!({"app": "cfg-app", "memory": "2G", "egressAdd": ["10.0.0.5:443"]})).await;
    assert_eq!((r.status, r.code()), (400, "egress_host_invalid".to_string()), "{}", r.body);
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=cfg-app").await;
    assert_eq!(r.json()["app"]["memoryLimitBytes"], 1i64 << 30, "nothing changed: {}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

/// Linux: a running `--cmd` service that calls K2 with the owner token is
/// traced to its service (counted in the warnings), and refused once the
/// service runs in clean mode. Run on the Linux harness.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linux_owner_token_calls_are_counted_then_refused_in_clean_mode() {
    require_python3();
    let env = setup(&[("K2_PUBLISH_PROBE_MS", "8000")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    // A booted vault, like a real box: a new service starts in the clean
    // env (with PORT), not the pre-vault inherited one (which has none).
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "tokws");
    let app = ws.path.join("bff");
    std::fs::create_dir_all(&app).unwrap();
    // Like a BFF that reads ~/.k2/daemon.token: the token and port from files.
    std::fs::write(app.join("token.txt"), OWNER).unwrap();
    std::fs::write(app.join("port.txt"), d.port.to_string()).unwrap();
    std::fs::write(
        app.join("bff.py"),
        r#"import http.server, os, urllib.request, urllib.error
D = os.path.dirname(os.path.abspath(__file__))
TOK = open(os.path.join(D, "token.txt")).read().strip()
PORT = open(os.path.join(D, "port.txt")).read().strip()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        # Only /call reaches K2 (the daemon's start probe hits / and must
        # not count as a use).
        if self.path != "/call":
            code = "ok"
        else:
            try:
                code = urllib.request.urlopen("http://127.0.0.1:%s/cli/app/keys?token=%s" % (PORT, TOK)).status
            except urllib.error.HTTPError as e:
                code = e.code
        body = str(code).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
http.server.HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
"#,
    )
    .unwrap();
    let port = k2_core::test_env::loopback_test_port();
    // Absolute: clean mode's system PATH must still find it.
    let python = String::from_utf8(std::process::Command::new("sh").arg("-c").arg("command -v python3").output().unwrap().stdout).unwrap();
    let cmd = format!("exec {} bff.py", python.trim());
    let r = post(d.port, "/cli/publish/run", OWNER, json!({"name": "tokbff", "project": ws.id, "cmd": cmd, "cwd": app, "port": port, "noTunnel": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let bff_log = k2_daemon::publish_runtime::log_path(&ws.id, "tokbff");
    let (_, code) = tokio::task::spawn_blocking(move || http_get(port, "/call"))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("bff answers; its log:\n{}", std::fs::read_to_string(&bff_log).unwrap_or_default()));
    assert_eq!(code.trim(), "200", "the owner token still works in 0.46.0");
    let r = get(d.port, "/cli/app/warnings", OWNER, "app=tokbff").await;
    assert_eq!(r.json()["ownerTokenUses"][0]["route"], "/cli/app/keys", "{}", r.body);
    assert!(r.json()["warnings"].as_array().unwrap().iter().any(|w| w["code"] == "owner_token_use"));
    let r = post(d.port, "/cli/app/clean-mode", OWNER, json!({"app": "tokbff", "on": true, "restart": true})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let (_, code) = tokio::task::spawn_blocking(move || http_get(port, "/call")).await.unwrap().expect("bff answers after restart");
    assert_eq!(code.trim(), "403", "clean mode refuses the owner token from the App");
    let r = get(d.port, "/cli/app/warnings", OWNER, "app=tokbff").await;
    assert_eq!(r.json()["ownerTokenUses"][0]["refused"], 1, "{}", r.body);
    let _ = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "tokbff", "project": ws.id})).await;
}
