//! T11 (prd-security-sweep-0464-v1 SS9–SS11), headless: a registered
//! workspace whose repo carries a `.claude/settings.json` hook. A harness
//! spawn through the real spawn door (`spawn_agent_session_v2_blocking`)
//! is refused with 409 `folder_trust_pending` (the body names the owner
//! ticket, because this workspace has no Thread, and says how to decide),
//! the spawn is held, nothing is granted. The owner decides through
//! `POST /cli/folder-trust/decide`; the held spawn starts by itself and
//! `~/.claude.json` holds the grant. "Don't trust" then stops the session
//! and removes K2's grant. Temp HOME; a fake `claude` (sleep) stands in
//! for the real CLI.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_daemon::folder_trust;
use k2_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2_daemon::v2_session_map;

const HOOK: &str = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node .claude/setup.mjs"}]}]}}"#;

fn claude_trusted(home: &Path, key: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(home.join(".claude.json")) else { return false };
    let v: serde_json::Value = serde_json::from_str(&text).expect(".claude.json parses");
    v["projects"][key]["hasTrustDialogAccepted"] == serde_json::Value::Bool(true)
}

#[test]
fn held_folder_spawn_waits_for_the_owner_then_starts() {
    let t = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    folder_trust::install();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
    let _enter = rt.enter();

    let home = t.path().canonicalize().unwrap();
    // The test build's agent belt only runs shims (never a real `claude`).
    let shim = k2_core::test_env::AgentShim::with_body("exec sleep 60");
    let fake: PathBuf = shim.dir().join("claude");

    let ws = home.join("hired-repo");
    std::fs::create_dir_all(ws.join(".claude")).unwrap();
    std::fs::write(ws.join(".claude/settings.json"), HOOK).unwrap();
    let ws = ws.canonicalize().unwrap();
    let key = ws.to_string_lossy().to_string();
    let pid = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, 'ft-hired')",
            rusqlite::params![pid, key],
        )
        .unwrap();
    }
    let req = SpawnWorkspaceSessionRequest {
        agent_name: "hired".into(),
        project_id: Some(pid.clone()),
        cwd: key.clone(),
        command: Some(fake.to_string_lossy().to_string()),
        args: Some(vec!["-p".into(), "hello".into()]),
        cols: 80,
        rows: 24,
        ..Default::default()
    };

    // 1. The spawn is refused: 409 folder_trust_pending, owner ticket, how to decide.
    let err = spawn_agent_session_v2_blocking(req.clone()).expect_err("a held folder starts no harness, print mode included");
    let resp = folder_trust::spawn_failed_response(&err);
    assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body["error"], "folder_trust_pending");
    assert_eq!(body["findings"][0]["file"], ".claude/settings.json");
    assert!(body["ticket"].as_str().is_some_and(|t| !t.is_empty()), "no Thread → owner ticket: {body}");
    assert_eq!(body["decide"], format!("k2 workspace trust {key} --yes|--always|--no (owner)"));
    assert!(v2_session_map::lookup_by_agent_name(&pid).is_none(), "nothing started");
    assert!(!claude_trusted(&home, &key), "no grant while held");
    let scope = body["scope"].as_str().unwrap().to_string();
    assert_eq!(folder_trust::held_count(&scope), 1, "the spawn is held");

    // 2. Status over the route.
    let q: HashMap<String, String> = [("path".to_string(), key.clone())].into();
    let st = folder_trust::dispatch(false, folder_trust::STATUS_PATH, &q, b"", "owner");
    assert_eq!(st.status, "200 OK");
    let stv: serde_json::Value = serde_json::from_str(&st.body).unwrap();
    assert_eq!(stv["state"], "pending");

    // 3. The owner decides; the held spawn starts by itself.
    let p: HashMap<String, String> = [("path".to_string(), key.clone()), ("decision".to_string(), "once".to_string())].into();
    let d = folder_trust::dispatch(true, folder_trust::DECIDE_PATH, &p, b"", "owner");
    assert_eq!(d.status, "200 OK", "{}", d.body);
    let deadline = Instant::now() + Duration::from_secs(15);
    while v2_session_map::lookup_by_agent_name(&pid).is_none() {
        if Instant::now() >= deadline {
            // Say why: the same spawn, run here, shows its error.
            let direct = spawn_agent_session_v2_blocking(req.clone()).map(|o| o.session_id.to_string());
            panic!("the held spawn didn't start within 15 s; the same spawn run directly gives: {direct:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(claude_trusted(&home, &key), "~/.claude.json holds the grant");
    assert_eq!(folder_trust::held_count(&scope), 0);
    let ticket = body["ticket"].as_str().unwrap();
    assert_eq!(k2_core::feedback::get_item(ticket).expect("ticket").status, "resolved", "the ticket closes with the decision");

    // 4. "Don't trust" stops the running session and removes K2's grant.
    let p: HashMap<String, String> = [("path".to_string(), key.clone()), ("decision".to_string(), "deny".to_string())].into();
    let d = folder_trust::dispatch(true, folder_trust::DECIDE_PATH, &p, b"", "owner");
    assert_eq!(d.status, "200 OK", "{}", d.body);
    let dv: serde_json::Value = serde_json::from_str(&d.body).unwrap();
    assert_eq!(dv["stopped"], 1, "{dv}");
    assert!(v2_session_map::lookup_by_agent_name(&pid).is_none());
    assert!(!claude_trusted(&home, &key), "K2's grant is gone");
    let err = spawn_agent_session_v2_blocking(req).expect_err("denied");
    let resp = folder_trust::spawn_failed_response(&err);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body["error"], "folder_trust_denied");
}
