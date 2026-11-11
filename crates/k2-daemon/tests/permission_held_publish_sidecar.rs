//! client papercut 12 (0.46.4 field report, fixed for 0.46.6), headless: the
//! real dispatcher in process on a temp `$HOME`, permission cards on.
//!
//! An agent working in a **sidecar** (`<ws>/<name>`, a second harness tab
//! of the workspace) runs `k2 publish run` and is held for the owner (a
//! `publish_run` App request):
//! - the hold raises the `apps.approve` card at once (it used to raise
//!   none: `/cli/publish/run` never ran `held_note`), on the **sidecar's**
//!   Thread (it used to go on the main one), and says where;
//! - the agent's own `k2 permission request apps.approve request=w_…`
//!   finds that same card (`already_pending`), on the sidecar's Thread;
//! - the owner answers Allow on the sidecar's Thread with the app-approve
//!   confirm: the permission request and the App request are approved by
//!   the owner, the service runs, and the `[permission:]` turn goes to the
//!   sidecar;
//! - a hold whose card can't go up (no Thread) carries the Ask-with
//!   `permission` object (`/cli/publish/run` had no wall row).
//!
//! client papercut 13: one owner approval covers identical re-runs of the
//! same App (command, folder, port, tunnel); any change asks again.
//!
//! No real agent CLI runs (a `claude` that is `cat`), no real `~/.k2`, no
//! keychain.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::Arc;

use app_fixture::*;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession, LabelSource};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use serde_json::json;

fn pin_chat(ws: &Ws) -> String {
    let conv = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    conv
}

/// The sidecar Thread `<ws>/<slug>`: provider conversation `conv` in a
/// `tab-pane-<conv>` harness tab.
fn sidecar(ws: &Ws, conv: &str, slug: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace_session_handles::allocate_ordinal(&conn, &ws.id, conv).expect("ordinal");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) VALUES ('claude', ?1, ?2, 0, unixepoch())",
        rusqlite::params![conv, slug],
    )
    .expect("name");
    conn.execute(
        "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
         VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
        rusqlite::params![ws.id, format!("pane-{conv}"), format!("tab-pane-{conv}"), conv],
    )
    .expect("tab");
}

/// The sidecar's live PTY (`claude`, resolved to the `exec cat` test shim
/// the caller installed) under its tab's agent name, and a passport bound
/// to that PTY's daemon session id (what a real sidecar's `k2` carries).
/// Ends the sidecar's PTY and turns the server switch off, even when the
/// test fails (no PTY or switch leaks into the next test).
struct Cleanup {
    agent_name: String,
    session: Arc<DaemonPtySession>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
        k2_daemon::v2_session_map::unregister(&self.agent_name);
        self.session.kill();
        k2_daemon::apps::alerts::set_sink(None);
    }
}

fn live_sidecar(ws: &Ws, conv: &str) -> (String, Arc<DaemonPtySession>, String) {
    let sid = SessionId::new();
    let session = DaemonPtySession::spawn(DaemonPtyConfig {
        session_id: sid,
        cols: 80,
        rows: 24,
        cwd: Some(ws.path.clone()),
        program: Some("claude".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: String::new(),
        label_source: LabelSource::Pty,
        sandbox: k2_core::terminal::SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        survive: false,
        ptyd_meta: None,
    })
    .expect("spawn the fake claude");
    let agent_name = format!("tab-pane-{conv}");
    k2_daemon::v2_session_map::register(agent_name.clone(), session.clone());
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    (agent_name, session, passport)
}

fn cards_on(ws: &Ws) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn.execute("UPDATE projects SET agents_can_request_permissions = 1 WHERE id = ?1", [&ws.id]).expect("agent switch");
    assert_eq!(n, 1);
}

fn permission_row(held: &str) -> (String, Option<String>, Option<String>, String, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT state, conversation_id, addr, card_id, answered_by FROM permission_requests \
         WHERE held_ref = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
        [held],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Option<String>>(3)?.unwrap_or_default(), r.get(4)?)),
    )
    .unwrap_or_else(|e| panic!("no permission request for {held}: {e}"))
}

fn confirm(bound: &str) -> String {
    k2_daemon::vault::get().expect("vault open").confirm.mint("app-approve", Some(bound), "owner", "local-auth")
}

fn publish_run_requests(ws: &Ws, name: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn
        .prepare("SELECT id FROM app_widenings WHERE kind = 'publish_run' AND workspace_id = ?1 AND app_label = ?2 ORDER BY requested_at, id")
        .expect("prepare");
    let ids = st.query_map(rusqlite::params![ws.id, name], |r| r.get::<_, String>(0)).expect("query");
    ids.collect::<rusqlite::Result<Vec<_>>>().expect("rows")
}

/// client papercut 13: one owner approval covers identical re-runs of the
/// same App (same command, folder, port, tunnel); any change asks again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_identical_publish_runs_take_one_approval_and_a_change_asks_again() {
    require_python3();
    let env = setup(&[("K2_PUBLISH_PROBE_MS", "8000"), ("K2_OPEN_APPS_GATE", "enforce")]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let ws = seed_ws(&env, "acmererun");
    let agent = passport(&ws);
    let port = k2_core::test_env::loopback_test_port();
    let cmd = "exec python3 -m http.server $PORT --bind 127.0.0.1";
    let run = |port: u16| json!({"name": "rerun-sales", "cmd": cmd, "port": port, "noTunnel": true});

    // Run 1: held; the owner approves it once.
    let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "{}", r.body);
    let first = r.json()["request"]["id"].as_str().expect("held id").to_string();
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": first, "confirmToken": confirm(&first)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);

    // Runs 2 and 3, identical: they run, naming the approval they repeat.
    for n in 2..=3 {
        let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "rerun-sales"})).await;
        assert_eq!(r.status, 200, "stop before run {n}: {}", r.body);
        let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
        assert_eq!(r.status, 200, "identical run {n} goes through: {}", r.body);
        assert_eq!(r.json()["approvedRun"]["request"], first.as_str(), "run {n}: {}", r.body);
    }
    assert_eq!(publish_run_requests(&ws, "rerun-sales"), vec![first.clone()], "three identical runs, one approval");

    // A different port asks again (a new request), and so does a new command.
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "rerun-sales"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/publish/run", &agent, run(k2_core::test_env::loopback_test_port())).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "a new port is held: {}", r.body);
    let changed = json!({"name": "rerun-sales", "cmd": "exec python3 -m http.server $PORT --bind 0.0.0.0", "port": port, "noTunnel": true});
    let r = post(d.port, "/cli/publish/run", &agent, changed).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "a new command is held: {}", r.body);
    assert_eq!(publish_run_requests(&ws, "rerun-sales").len(), 3, "each change is its own request");

    // The owner revokes the standing approval (an agent can't): the next
    // identical run is held again.
    let r = post(d.port, "/cli/app/approvals/revoke", &agent, json!({"request": first})).await;
    assert_eq!(r.status, 403, "an agent never revokes: {}", r.body);
    assert_eq!(publish_run_requests(&ws, "rerun-sales").len(), 3);
    let r = post(d.port, "/cli/app/approvals/revoke", OWNER, json!({"request": first})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["revoked"][0]["id"], first.as_str(), "{}", r.body);
    let r = post(d.port, "/cli/app/approvals/revoke", OWNER, json!({"request": first})).await;
    assert_eq!(r.status, 409, "once: {}", r.body);
    let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "revoked: asks again: {}", r.body);
    let again = r.json()["request"]["id"].as_str().expect("held id").to_string();
    assert_ne!(again, first);

    // Approved again, it stands, with its lapse date (30 days)…
    let r = post(d.port, "/cli/app/approve", OWNER, json!({"request": again, "confirmToken": confirm(&again)})).await;
    assert_eq!((r.status, r.json()["status"].clone()), (200, json!("approved")), "{}", r.body);
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "rerun-sales"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["approvedRun"]["request"], again.as_str(), "{v}");
    let decided: i64 = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT decided_at FROM app_widenings WHERE id = ?1", [&again], |r| r.get(0)).expect("decided_at")
    };
    assert_eq!(v["approvedRun"]["expiresAt"].as_i64(), Some(decided + 30 * 86_400), "{v}");

    // … and lapses after 30 days.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn.execute("UPDATE app_widenings SET decided_at = decided_at - 31 * 86400 WHERE id = ?1", [&again]).expect("backdate");
        assert_eq!(n, 1);
        // The service is older still, so only the 30 days can end it.
        let n = conn
            .execute(
                "UPDATE published_services SET created_at = created_at - 32 * 86400 WHERE project_id = ?1 AND name = 'rerun-sales'",
                [&ws.id],
            )
            .expect("backdate the service");
        assert_eq!(n, 1);
    }
    let r = post(d.port, "/cli/publish/stop", &agent, json!({"name": "rerun-sales"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "lapsed: asks again: {}", r.body);

    let r = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "rerun-sales", "project": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    // Removed, the App's approvals stand for nothing: the same run is new.
    let r = post(d.port, "/cli/publish/run", &agent, run(port)).await;
    assert_eq!((r.status, r.json()["kind"].clone()), (202, json!("publish_run")), "after rm: asks again: {}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sidecars_held_publish_run_gets_its_card_on_the_sidecar_and_allow_runs_it() {
    require_python3();
    // The owner's gate choice: an agent's new --cmd service is held even
    // where this harness has no open path.
    let env = setup(&[("K2_PUBLISH_PROBE_MS", "8000"), ("K2_OPEN_APPS_GATE", "enforce")]);
    k2_daemon::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());

    let ws = seed_ws(&env, "acmeside");
    let main = pin_chat(&ws);
    let conv = uuid::Uuid::new_v4().to_string();
    sidecar(&ws, &conv, "sales");
    let side = format!("{}/sales", ws.handle);
    // The shim only for the sidecar's own spawn (the App's python3 later is
    // no agent CLI).
    let (agent_name, session, agent) = {
        let _agents = k2_core::test_env::AgentShim::install();
        live_sidecar(&ws, &conv)
    };
    let _cleanup = Cleanup { agent_name, session };
    k2_core::permission::gate::set_server_switch(true);
    cards_on(&ws);

    // 1. The hold raises the card, on the sidecar's Thread.
    let port = k2_core::test_env::loopback_test_port();
    let run = json!({"name": "acme-sales", "cmd": "exec python3 -m http.server $PORT --bind 127.0.0.1", "port": port, "noTunnel": true});
    let r = post(d.port, "/cli/publish/run", &agent, run).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let v = r.json();
    assert_eq!(v["kind"], "publish_run", "{v}");
    let held = v["request"]["id"].as_str().expect("held id").to_string();
    assert_eq!((v["card"]["raised"].as_bool(), v["card"]["new"].as_bool()), (Some(true), Some(true)), "the hold raises its card: {v}");
    assert_eq!(v["card"]["thread"], side.as_str(), "on the sidecar's Thread: {v}");
    // The note's `id` is the request's own (the held w_… id, §3.2).
    assert_eq!(v["card"]["id"], held.as_str(), "{v}");
    let (state, on_conv, addr, card, by) = permission_row(&held);
    assert_eq!(state, "pending");
    assert_eq!(on_conv.as_deref(), Some(conv.as_str()), "the sidecar's conversation, not the main one ({main})");
    assert_eq!(addr.as_deref(), Some(side.as_str()));
    assert!(!card.is_empty(), "the request carries its card");
    assert_eq!(by, None);

    // 2. The agent's own ask finds that card.
    let r = post(
        d.port,
        "/cli/permission/request",
        &agent,
        json!({"action": "apps.approve", "params": {"request": held}, "why": "start the sales App"}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let a = r.json();
    assert_eq!((a["status"].as_str(), a["card"].as_str(), a["addr"].as_str()), (Some("already_pending"), Some(card.as_str()), Some(side.as_str())), "{a}");

    // 3. The owner answers Allow on the sidecar's Thread, with the confirm.
    let r = post(
        d.port,
        "/cli/thread/answer",
        OWNER,
        json!({"addr": side, "id": card, "answer": "Allow", "client": "desktop", "confirmToken": confirm(&held)}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["action"]["result"]["outcome"], "approved", "{}", r.body);

    // 4. Resolved: both requests approved by the owner, and the service runs.
    let (state, _, _, _, by) = permission_row(&held);
    assert_eq!((state.as_str(), by.as_deref()), ("approved", Some("owner")));
    let r = get(d.port, "/cli/app/approvals", OWNER, "state=all").await;
    let reqs = r.json()["requests"].as_array().cloned().unwrap_or_default();
    let app_req = reqs.iter().find(|x| x["id"] == held.as_str()).unwrap_or_else(|| panic!("{held} listed: {}", r.body)).clone();
    assert_eq!(app_req["status"], "approved", "{app_req}");
    wait_until("the approved service answers", 20, || http_get(port, "/").is_some_and(|(s, _)| s == 200)).await;

    let r = post(d.port, "/cli/publish/rm", OWNER, json!({"name": "acme-sales", "project": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // 5. No Thread at all: the card can't go up, so the hold teaches.
    let lone = seed_ws(&env, "acmelone");
    cards_on(&lone);
    let lone_agent = passport(&lone);
    let run = json!({"name": "lone-svc", "cmd": "exec python3 -m http.server $PORT --bind 127.0.0.1", "port": k2_core::test_env::loopback_test_port(), "noTunnel": true});
    let r = post(d.port, "/cli/publish/run", &lone_agent, run).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let v = r.json();
    let lone_held = v["request"]["id"].as_str().expect("held id");
    assert_eq!(v["card"]["raised"], false, "{v}");
    assert_eq!(v["permission"]["action"], "apps.approve", "the hold ends with its Ask-with object: {v}");
    assert_eq!(
        v["permission"]["command"],
        format!("k2 permission request apps.approve request={lone_held} --why \"<what you need it for>\""),
        "{v}"
    );
}

/// A real passport file per spawned harness: the shim writes the
/// `K2_HOOK_TOKEN` K2 put in its env to `<dir>/<cell>-<sidecar name>.token`
/// (the canonical cell's name is empty), then becomes `cat`.
fn token_shim(dir: &std::path::Path) -> k2_core::test_env::AgentShim {
    std::fs::create_dir_all(dir).expect("token dir");
    k2_core::test_env::AgentShim::with_body(&format!(
        "if [ \"$1\" = \"--help\" ]; then printf '      --no-daemon\\n'; exit 0; fi\n\
         if [ \"$1\" = \"create-chat\" ]; then /usr/bin/uuidgen | /usr/bin/tr 'A-Z' 'a-z'; exit 0; fi\n\
         f=\"{dir}/$K2_CELL-$K2_SIDECAR_NAME\"\n\
         printf '%s' \"$K2_HOOK_TOKEN\" > \"$f.tmp\"\n\
         /bin/mv \"$f.tmp\" \"$f.token\"\n\
         exec cat",
        dir = dir.display()
    ))
}

async fn wait_token(dir: &std::path::Path, file: &str) -> String {
    let path = dir.join(file);
    let mut found = None;
    wait_until(&format!("the harness to write {file}"), 20, || {
        found = std::fs::read_to_string(&path).ok().filter(|t| !t.is_empty());
        found.is_some()
    })
    .await;
    found.expect("a passport")
}

/// A user's report (0.46.10): a sidecar's `k2 permission request` card went
/// to the workspace's main Thread. Here everything is real: the sidecars
/// come from `k2 sidecar new` (claude, and codex, whose conversation id K2
/// learns later), the main agent from a real spawn, and each asks with the
/// passport K2 put in its own env. Each card lands on the Thread of the
/// session that asked, and nowhere else. Before the fix, the main agent's
/// open card for the same change answered the sidecar's ask
/// (`already_pending` with the main card): the person found nothing on the
/// sidecar's Thread and the answer went to the main session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_k2_sidecar_new_sidecars_ask_lands_on_its_own_thread_and_the_mains_on_main() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "asker");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), ws.id],
        )
        .expect("workspaces row");
    }
    let peer = seed_ws(&env, "askerpeer");
    let main = pin_chat(&ws);
    let tokens = env.path().join("tok");
    let _shim = token_shim(&tokens);
    struct Off(Vec<String>);
    impl Drop for Off {
        fn drop(&mut self) {
            k2_core::permission::gate::set_server_switch(false);
            for key in k2_daemon::v2_session_map::list_agents() {
                if self.0.iter().any(|k| *k == key) || key.starts_with("tab-") {
                    if let Some(s) = k2_daemon::v2_session_map::unregister(&key) {
                        s.kill();
                    }
                }
            }
        }
    }
    let _off = Off(vec![ws.id.clone()]);
    k2_core::permission::gate::set_server_switch(true);
    cards_on(&ws);

    let peer2 = seed_ws(&env, "askerpeer2");
    // Ask for `connections.add to=<to>`; the answer, which must be a 200.
    let ask = |passport: String, to: String| async move {
        let r = post(port, "/cli/permission/request", &passport, json!({"action": "connections.add", "params": {"to": to}, "why": "Talk to the peer"})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.json()
    };
    let row = |id: &str| -> (Option<String>, Option<String>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT conversation_id, addr FROM permission_requests WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("the request row")
    };

    // 1. The main agent, spawned for real, asks first: its card is on main.
    k2_daemon::spawn::spawn_agent_session_v2_blocking(k2_daemon::spawn::SpawnWorkspaceSessionRequest {
        agent_name: ws.id.clone(),
        project_id: Some(ws.id.clone()),
        cwd: ws.path.to_string_lossy().into_owned(),
        command: Some("claude".to_string()),
        args: Some(vec!["--resume".to_string(), main.clone()]),
        cols: 80,
        rows: 24,
        canonical_key: Some(ws.id.clone()),
        env: Default::default(),
        env_owner_set: false,
        launch_prompt: None,
        label: None,
    })
    .expect("spawn the main agent");
    let main_passport = wait_token(&tokens, "canonical-.token").await;
    let m = ask(main_passport, peer.handle.clone()).await;
    assert_eq!((m["status"].as_str(), m["addr"].as_str()), (Some("pending"), Some(ws.handle.as_str())), "{m}");
    let main_card = m["card"].as_str().expect("card").to_string();
    assert_eq!(row(m["id"].as_str().expect("id")).0.as_deref(), Some(main.as_str()));

    // 2. Each sidecar asks on its own Thread. The claude one asks for the
    //    same change the main card waits on (the user's report: it got the
    //    main card back); the codex one for another peer (the workspace's
    //    ask limit, 2 a week for one change, is not what this tests).
    for (harness, to) in [("claude", peer.handle.clone()), ("codex", peer2.handle.clone())] {
        let name = format!("rev{harness}");
        let r = post(port, "/cli/sidecar/new", OWNER, json!({"workspace": ws.handle, "name": name, "harness": harness})).await;
        assert_eq!(r.status, 200, "sidecar new {harness}: {}", r.body);
        let v = r.json();
        let side = format!("{}/{name}", ws.handle);
        assert_eq!(v["address"], side.as_str(), "{v}");
        let passport = wait_token(&tokens, &format!("sidecar-{name}.token")).await;
        let thread = k2_daemon::overlay_routes::conversation_for_addr(&side).unwrap_or_else(|| panic!("{side} has a Thread"));
        assert_ne!(thread, main, "{side}'s Thread is its own");

        let a = ask(passport.clone(), to.clone()).await;
        assert_eq!(a["status"], "pending", "{harness}: a card of its own, not another Thread's: {a}");
        assert_eq!(a["addr"], side.as_str(), "{harness}: the card names the sidecar's Thread: {a}");
        let card = a["card"].as_str().expect("card").to_string();
        assert_ne!(card, main_card, "{harness}");
        let (conv, addr) = row(a["id"].as_str().expect("id"));
        assert_eq!(conv.as_deref(), Some(thread.as_str()), "{harness}: on {side}'s Thread, not the main one ({main})");
        assert_eq!(addr.as_deref(), Some(side.as_str()));
        assert!(k2_core::overlay::get_thread_doc(&thread, &card).expect("read").is_some(), "{harness}: the card is on the sidecar's Thread");
        assert!(k2_core::overlay::get_thread_doc(&main, &card).expect("read").is_none(), "{harness}: and not on the main Thread");

        // Asked again from the same Thread: the same card, nothing new.
        let again = ask(passport, to).await;
        assert_eq!(
            (again["status"].as_str(), again["card"].as_str(), again["addr"].as_str()),
            (Some("already_pending"), Some(card.as_str()), Some(side.as_str())),
            "{again}"
        );
    }
    assert!(k2_core::overlay::get_thread_doc(&main, &main_card).expect("read").is_some(), "the main card stays on main");
}
