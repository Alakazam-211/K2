//! `git.land` / `sidecar.retire` end to end: the agent is refused at the
//! route (the refusal names the sidecar and its tip), asks; a Member can't
//! answer; an Admin taps Allow and the card lands the commits on main as
//! that Admin and retires the worktree; a tip that moved is refused at the
//! ask; Retire without discard refuses unlanded work, with discard throws
//! it away.

use std::path::{Path, PathBuf};
use std::process::Command;

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{Floor, Grant, Proof, Undo, WallMatch};
use k2_core::worktree_task::{self as wt, store};
use serde_json::json;

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, seed, Ws};
use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Point the seeded workspace at a fresh repository (on `main`, one commit).
fn repo_for(home: &crate::test_support::TempHome, ws: &Ws) -> PathBuf {
    let root = home.path().join(format!("repo-{}", ws.handle));
    std::fs::create_dir_all(&root).expect("mkdir repo");
    let root = dunce::canonicalize(&root).expect("canonical repo");
    sh(&root, &["init", "-q"]);
    sh(&root, &["config", "user.name", "K2 Test"]);
    sh(&root, &["config", "user.email", "k2-test@example.com"]);
    std::fs::write(root.join("a.txt"), "one\n").expect("a");
    sh(&root, &["add", "-A"]);
    sh(&root, &["commit", "-q", "-m", "init"]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("UPDATE projects SET path = ?1 WHERE id = ?2", rusqlite::params![root.to_string_lossy(), ws.id])
        .expect("repoint project");
    root
}

/// A worktree sidecar `slug` of `ws` (as `sidecar new --worktree` leaves
/// it, minus the process) with one commit on its branch; returns its tip.
fn worktree_sidecar(ws: &Ws, root: &Path, slug: &str) -> (String, PathBuf) {
    let c = wt::create(root, slug, None).expect("create worktree");
    let path = PathBuf::from(&c.path);
    std::fs::write(path.join(format!("{slug}.txt")), "work\n").expect("work");
    sh(&path, &["add", "-A"]);
    sh(&path, &["commit", "-q", "-m", &format!("{slug}: the work")]);
    let tip = sh(&path, &["rev-parse", "HEAD"]);
    let pg = uuid::Uuid::new_v4().to_string();
    let cid = uuid::Uuid::new_v4().to_string();
    let ws_row = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Workspace::create(&conn, &ws_row, &ws.id, None, "worktree", Some(&c.branch), &c.branch, 5, Some(&c.path))
        .expect("workspaces row");
    conn.execute(
        "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at, created_by) \
         VALUES (?1, ?2, ?3, ?4, 'claude', '[]', ?5, 0, 'owner')",
        rusqlite::params![ws.id, pg, format!("tab-{pg}"), cid, c.path],
    )
    .expect("tab row");
    k2_core::sidecar::bind_name(&conn, &ws.id, "claude", &cid, slug, slug).expect("bind name");
    store::stamp_new(&conn, &ws.id, &pg, &ws_row, &c.branch, &c.dir, c.base.onto.as_deref()).expect("stamp");
    (tip, path)
}

fn meta(ws: &Ws, slug: &str) -> store::WorktreeMeta {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let pg: String = conn
        .query_row(
            "SELECT pane_group_id FROM workspace_tab_sessions WHERE project_id = ?1 AND worktree_branch = ?2",
            rusqlite::params![ws.id, format!("k2/{slug}")],
            |r| r.get(0),
        )
        .expect("the sidecar's row");
    store::get(&conn, &ws.id, &pg).expect("worktree columns")
}

fn branch_exists(root: &Path, b: &str) -> bool {
    !sh(root, &["branch", "--list", b]).is_empty()
}

#[test]
fn both_actions_are_registered_at_the_admin_floor_with_their_walls() {
    for (id, route, code, keys) in [
        (LAND_ACTION, LAND_ROUTE, LAND_NEEDS_PERSON, ["sidecar", "tip"]),
        (RETIRE_ACTION, RETIRE_ROUTE, RETIRE_NEEDS_PERSON, ["sidecar", "discard"]),
    ] {
        let r = super::super::find(id).expect("registered");
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant), (Floor::Admin, Proof::Click, Grant::OneTime), "{id}");
        assert!(matches!(r.spec.undo, Undo::None), "{id}");
        assert_eq!(r.spec.params.iter().map(|p| p.key).collect::<Vec<_>>(), keys, "{id}");
        let walls: Vec<(&str, &str)> = r
            .spec
            .walls
            .iter()
            .map(|w| match w.matches {
                WallMatch::Code { code, .. } => (w.route_prefix, code),
                other => panic!("{id}: {other:?}"),
            })
            .collect();
        assert_eq!(walls, [(route, code)], "{id}");
        // The card's floor is the route's own POST floor.
        assert_eq!(
            crate::routes::route_policy::lookup(route).and_then(|x| x.post),
            Some(crate::routes::route_policy::Floor::Admin),
            "{route}"
        );
    }
}

#[test]
fn the_agent_asks_a_member_cant_answer_and_an_admin_allow_lands_and_retires() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws: Ws = seed("wbland");
    let root = repo_for(&home, &ws);
    let (tip, wt_path) = worktree_sidecar(&ws, &root, "fix");
    let main_before = sh(&root, &["rev-parse", "main"]);

    // The agent itself is refused at the route, told the sidecar and tip.
    let me = Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: None, args: Vec::new(), sandboxed: false });
    let body = json!({ "workspace": ws.id, "name": "fix", "expectTip": tip });
    let r = crate::sidecar_routes::handle(LAND_ROUTE, me.clone(), body.to_string().as_bytes(), &Default::default(), "test");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let e: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(
        (&e["error"]["code"], &e["error"]["sidecar"], &e["error"]["tip"]),
        (&json!(LAND_NEEDS_PERSON), &json!("fix"), &json!(tip)),
        "{e}"
    );
    assert_eq!(sh(&root, &["rev-parse", "main"]), main_before, "an agent never lands");

    // A tip that isn't the branch's is refused at the ask.
    let hint = ask_refused(&ws, LAND_ACTION, json!({ "sidecar": "fix", "tip": "0000000000" }));
    assert!(hint.contains("moved since the ask"), "{hint}");

    let v = ask_ok(&ws, LAND_ACTION, json!({ "sidecar": "fix", "tip": tip }));
    let card = card_of(&v);

    // A Member is below the floor; nothing changes.
    let member = login("wbm", Role::Member);
    below_floor(answer_as(&member, &ws, &card, json!({ "answer": "Allow" })), "admin");
    assert_eq!(sh(&root, &["rev-parse", "main"]), main_before);

    // An Admin taps Allow: landed on main, then retired.
    let admin = login("wba", Role::Admin);
    let (s, a) = answer_as(&admin, &ws, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    assert_ne!(sh(&root, &["rev-parse", "main"]), main_before, "main moved");
    assert_eq!(std::fs::read_to_string(root.join("fix.txt")).expect("landed"), "work\n");
    assert_eq!(sh(&root, &["log", "-1", "--format=%s", "main"]), "fix: the work");
    assert!(!wt_path.exists(), "the worktree went to the Trash");
    assert!(!branch_exists(&root, "k2/fix"), "a landed branch is deleted");
    let m = meta(&ws, "fix");
    assert!(m.landed_at.is_some() && m.retired_at.is_some(), "{m:?}");
    assert_eq!(m.retired_how.as_deref(), Some(store::RETIRED_LANDED));
    assert_eq!(m.worktree_ws_id, None);
    let turn = last_turn(&ws);
    assert!(turn.contains("Landed 1 commit on main"), "{turn}");
    let audit = std::fs::read_to_string(home.path().join(".k2").join("auth-audit.jsonl")).expect("audited");
    assert!(audit.contains("sidecar-land") && audit.contains(&format!("login:{admin}")), "{audit}");

    // Asking again: the sidecar is retired, so the ask is refused.
    let hint = ask_refused(&ws, LAND_ACTION, json!({ "sidecar": "fix", "tip": tip }));
    assert!(hint.contains("retired"), "{hint}");
}

#[test]
fn retire_refuses_unlanded_work_until_discard() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let ws: Ws = seed("wbret");
    let root = repo_for(&home, &ws);
    let (tip, wt_path) = worktree_sidecar(&ws, &root, "spike");
    std::fs::write(wt_path.join("scratch.txt"), "unsaved\n").expect("scratch");

    // The agent is refused at the route with the sidecar and its choice.
    let me = Caller::Agent(AgentCaller { project_id: ws.id.clone(), map_key: None, args: Vec::new(), sandboxed: false });
    let body = json!({ "workspace": ws.id, "name": "spike", "discard": true });
    let r = crate::sidecar_routes::handle(RETIRE_ROUTE, me, body.to_string().as_bytes(), &Default::default(), "test");
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains(RETIRE_NEEDS_PERSON) && r.body.contains("\"discard\":\"yes\""), "{}", r.body);
    assert!(wt_path.exists());

    // discard=no: the owner's Allow fails on the unlanded work; nothing moves.
    let v = ask_ok(&ws, RETIRE_ACTION, json!({ "sidecar": "spike", "discard": "no" }));
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "failed", "{a}");
    assert!(wt_path.join("scratch.txt").exists(), "unlanded work stays");
    assert!(branch_exists(&root, "k2/spike"));
    assert_eq!(meta(&ws, "spike").retired_at, None);

    // discard=yes: stopped, worktree to the Trash, branch deleted.
    let v = ask_ok(&ws, RETIRE_ACTION, json!({ "sidecar": "spike", "discard": "yes" }));
    let (s, a) = answer_as("owner", &ws, &card_of(&v), json!({ "answer": "Allow" }));
    assert_eq!((s.as_str(), &a["action"]["result"]["outcome"]), ("200 OK", &json!("approved")), "{a}");
    assert!(!wt_path.exists(), "the worktree went to the Trash");
    assert!(!branch_exists(&root, "k2/spike"), "a discarded branch is deleted");
    let m = meta(&ws, "spike");
    assert_eq!(m.retired_how.as_deref(), Some(store::RETIRED_DISCARDED));
    let turn = last_turn(&ws);
    assert!(turn.contains("Retired") && turn.contains("branch is deleted"), "{turn}");
    // The tip that was thrown away is still a commit git knows (until gc).
    assert_eq!(sh(&root, &["cat-file", "-t", &tip]), "commit");

    // Asking again: already done.
    let (s, v) = ask(&ws, RETIRE_ACTION, json!({ "sidecar": "spike", "discard": "yes" }));
    assert_eq!((s.as_str(), &v["status"]), ("200 OK", &json!("already_done")), "{v}");
}
