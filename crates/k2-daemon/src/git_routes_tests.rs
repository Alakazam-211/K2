//! Route tests for the Changes tab v1 git routes (C1): snapshot,
//! `diff-file?sides=1`, discard (Trash, 405, traversal, conflict), stage
//! hardening, the immediate `git_changed`, and the route-policy rows.
//! Discards go to a temp dir via the `git_changes` trash override.

use super::*;
use crate::session_events::SessionEvent;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-gitroutes-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    d
}

/// One trash dir for this test process (the override is global); every
/// test trashes uniquely named files, so they never collide.
fn trash_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = tmp("trash");
        git_changes::set_trash_override(Some(d.clone()));
        d
    })
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=K2 Test", "-c", "user.email=k2-test@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn repo(tag: &str) -> PathBuf {
    let d = tmp(tag);
    git(&d, &["init", "-q"]);
    // Commits made by the daemon's `git::commit` read the repo config.
    git(&d, &["config", "user.name", "K2 Test"]);
    git(&d, &["config", "user.email", "k2-test@example.com"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\n").expect("a");
    git(&d, &["add", "-A"]);
    git(&d, &["commit", "-q", "-m", "init"]);
    d
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf8").to_string()
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn json(r: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("body is not JSON ({e}): {}", r.body))
}

fn post(path: &str, body: serde_json::Value) -> CliResponse {
    dispatch_unit4_git_post(path, body.to_string().as_bytes())
}

// ── snapshot ─────────────────────────────────────────────────────────

#[test]
fn snapshot_route_answers_the_contract_shape() {
    let d = repo("snap");
    std::fs::write(d.join("a.txt"), "one\nTWO\n").expect("edit");
    std::fs::write(d.join("new.txt"), "n\n").expect("new");
    let r = crate::cli::dispatch("/cli/git/snapshot", &params(&[("path", &s(&d))]));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let j = json(&r);
    assert_eq!(j["isRepo"], true);
    assert_eq!(j["branch"], "main");
    assert_eq!(j["counts"]["unstaged"], 1);
    assert_eq!(j["counts"]["untracked"], 1);
    assert_eq!(j["truncated"], false);
    assert_eq!(j["limit"], 1000);
    let rows = j["files"].as_array().expect("files");
    assert_eq!(rows[0]["path"], "a.txt");
    assert_eq!(rows[0]["group"], "unstaged");
    assert_eq!(rows[0]["status"], "M");
    assert_eq!(rows[0]["additions"], 1);
    assert_eq!(rows[0]["deletions"], 1);
    assert_eq!(rows[1]["status"], "U");
    assert!(j["rev"].as_str().expect("rev").contains(':'));

    let missing = crate::cli::dispatch("/cli/git/snapshot", &params(&[]));
    assert_eq!(missing.status, "400 Bad Request");
    assert_eq!(json(&missing)["code"], "bad_request");

    let not_repo = crate::cli::dispatch("/cli/git/snapshot", &params(&[("path", &s(&tmp("snap-norepo")))]));
    assert_eq!(not_repo.status, "200 OK");
    assert_eq!(json(&not_repo)["isRepo"], false);

    let bad_base = crate::cli::dispatch("/cli/git/snapshot", &params(&[("path", &s(&d)), ("base", "nope")]));
    assert_eq!(bad_base.status, "400 Bad Request");
    assert_eq!(json(&bad_base)["code"], "bad_ref");

    let with_base = crate::cli::dispatch("/cli/git/snapshot", &params(&[("path", &s(&d)), ("base", "main")]));
    assert_eq!(with_base.status, "200 OK", "{}", with_base.body);
    assert_eq!(json(&with_base)["base"]["ref"], "main");
    assert_eq!(json(&with_base)["files"][0]["group"], "base");
}

// ── diff-file ────────────────────────────────────────────────────────

#[test]
fn diff_file_sides_and_the_old_hunks_shape() {
    let d = repo("diff");
    std::fs::write(d.join("a.txt"), "one\nTWO\nthree\n").expect("edit");
    let ws = s(&d);

    let old = crate::cli::dispatch("/cli/git/diff-file", &params(&[("path", &ws), ("file_path", "a.txt")]));
    assert_eq!(old.status, "200 OK");
    assert!(json(&old).is_array(), "without sides=1 the gutter shape is unchanged: {}", old.body);

    let r = crate::cli::dispatch("/cli/git/diff-file", &params(&[("path", &ws), ("file_path", "a.txt"), ("sides", "1")]));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let j = json(&r);
    assert_eq!(j["mode"], "worktree");
    assert_eq!(j["old"]["label"], "HEAD");
    assert_eq!(j["old"]["text"], "one\ntwo\n");
    assert_eq!(j["new"]["label"], "WORKTREE");
    assert_eq!(j["new"]["text"], "one\nTWO\nthree\n");
    assert_eq!(j["additions"], 2);
    assert_eq!(j["deletions"], 1);
    assert_eq!(j["binary"], false);
    assert_eq!(j["tooLarge"], false);

    git(&d, &["add", "a.txt"]);
    let st = crate::cli::dispatch("/cli/git/diff-file", &params(&[("path", &ws), ("file_path", "a.txt"), ("sides", "1"), ("staged", "1")]));
    assert_eq!(json(&st)["mode"], "staged");
    assert_eq!(json(&st)["new"]["label"], "INDEX");

    let cases: [(&[(&str, &str)], &str, &str); 4] = [
        (&[("file_path", "../x"), ("sides", "1")], "400 Bad Request", "bad_path"),
        (&[("file_path", "nope.txt"), ("sides", "1")], "404 Not Found", "not_found"),
        (&[("file_path", "a.txt"), ("sides", "1"), ("mode", "sideways")], "400 Bad Request", "bad_request"),
        (&[("file_path", "a.txt"), ("sides", "1"), ("base", "no-such")], "400 Bad Request", "bad_ref"),
    ];
    for (extra, status, code) in cases {
        let mut p = params(extra);
        p.insert("path".into(), ws.clone());
        let r = crate::cli::dispatch("/cli/git/diff-file", &p);
        assert_eq!(r.status, status, "{extra:?}: {}", r.body);
        assert_eq!(json(&r)["code"], code, "{extra:?}");
    }
}

// ── discard ──────────────────────────────────────────────────────────

#[test]
fn discard_get_is_405_and_policy_rows_are_member() {
    let r = handle_git_discard(false, b"{}");
    assert_eq!(r.status, "405 Method Not Allowed");
    let via_get_chain = crate::cli::dispatch("/cli/git/discard", &params(&[]));
    assert_eq!(via_get_chain.status, "405 Method Not Allowed");

    use crate::routes::route_policy::{get_refused, lookup, post_allowed, Floor};
    let row = lookup("/cli/git/discard").expect("classified");
    assert_eq!((row.get, row.post), (None, Some(Floor::Member)));
    assert!(post_allowed("/cli/git/discard"));
    assert!(get_refused("/cli/git/discard", ""), "the central guard 405s a GET before auth");
    let snap = lookup("/cli/git/snapshot").expect("classified");
    assert_eq!((snap.get, snap.post), (Some(Floor::Member), None));
    assert!(!get_refused("/cli/git/snapshot", "path=/x"));
}

#[test]
fn discard_route_trashes_untracked_restores_tracked_and_emits() {
    let trash = trash_dir();
    let d = repo("discard");
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let junk = format!("junk-{tag}.txt");
    std::fs::write(d.join("a.txt"), "agent edit\n").expect("edit");
    std::fs::write(d.join(&junk), "junk\n").expect("junk");
    let ws = s(&d);
    let mut rx = crate::session_events::subscribe();

    let r = post("/cli/git/discard", serde_json::json!({ "path": ws, "files": ["a.txt", junk] }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let j = json(&r);
    assert_eq!(j["success"], true);
    assert_eq!(j["restored"], serde_json::json!(["a.txt"]));
    assert_eq!(j["trashed"], serde_json::json!(["a.txt", junk]));
    assert_eq!(j["failed"], serde_json::json!([]));
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "one\ntwo\n");
    assert!(!d.join(&junk).exists());
    let in_trash: Vec<String> = std::fs::read_dir(trash)
        .expect("trash")
        .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&format!("junk-{tag}.txt-")))
        .collect();
    assert_eq!(in_trash.len(), 1, "the untracked file is in the trash dir, not deleted");

    // The route emitted git_changed right away with the same rev.
    let rev = j["rev"].as_str().expect("rev").to_string();
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut seen = None;
    while Instant::now() < deadline && seen.is_none() {
        match rx.try_recv() {
            Ok(SessionEvent::GitChanged { workspace_path, rev: r, .. }) if workspace_path == ws => seen = Some(r),
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    assert_eq!(seen.as_deref(), Some(rev.as_str()), "git_changed carries the response rev");
}

#[test]
fn discard_route_refuses_traversal_conflicts_and_bad_bodies() {
    trash_dir();
    let d = repo("discard-bad");
    std::fs::write(d.join("a.txt"), "edited\n").expect("edit");
    let ws = s(&d);
    for bad in ["../../etc/hosts", "/etc/hosts", ".git/config"] {
        let r = post("/cli/git/discard", serde_json::json!({ "path": ws, "files": ["a.txt", bad] }));
        assert_eq!(r.status, "400 Bad Request", "{bad}: {}", r.body);
        assert_eq!(json(&r)["code"], "bad_path");
    }
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "edited\n", "refusal changes nothing");

    let empty = post("/cli/git/discard", serde_json::json!({ "path": ws, "files": [] }));
    assert_eq!((empty.status, json(&empty)["code"].clone()), ("400 Bad Request", serde_json::json!("bad_request")));
    let no_path = post("/cli/git/discard", serde_json::json!({ "path": "", "files": ["a.txt"] }));
    assert_eq!(no_path.status, "400 Bad Request");
    let not_json = dispatch_unit4_git_post("/cli/git/discard", b"nope");
    assert_eq!(not_json.status, "400 Bad Request");
    let norepo = post("/cli/git/discard", serde_json::json!({ "path": s(&tmp("discard-norepo")), "files": ["x"] }));
    assert_eq!(json(&norepo)["code"], "not_a_repo");

    // A real merge conflict → 409 and nothing touched.
    git(&d, &["checkout", "-q", "--", "a.txt"]);
    git(&d, &["checkout", "-q", "-b", "other"]);
    std::fs::write(d.join("a.txt"), "other side\n").expect("o");
    git(&d, &["commit", "-q", "-am", "other"]);
    git(&d, &["checkout", "-q", "main"]);
    std::fs::write(d.join("a.txt"), "main side\n").expect("m");
    git(&d, &["commit", "-q", "-am", "main"]);
    let merge = Command::new("git")
        .args(["-c", "user.name=K2 Test", "-c", "user.email=k2-test@example.com", "merge", "-q", "other"])
        .current_dir(&d)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("merge");
    assert!(!merge.status.success(), "the fixture merge must conflict");
    let before = std::fs::read_to_string(d.join("a.txt")).expect("conflicted a");
    let r = post("/cli/git/discard", serde_json::json!({ "path": ws, "files": ["a.txt"], "includeStaged": true }));
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert_eq!(json(&r)["code"], "conflicted");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), before);
    let snap = json(&crate::cli::dispatch("/cli/git/snapshot", &params(&[("path", &ws)])));
    assert_eq!(snap["state"], "merge");
    assert_eq!(snap["files"][0]["group"], "conflicted");
}

// ── stage / unstage / commit ─────────────────────────────────────────

#[test]
fn stage_refuses_traversal_and_mutations_bump_rev() {
    let d = repo("stage");
    let ws = s(&d);
    std::fs::write(d.join("b.txt"), "b\n").expect("b");
    for bad in ["../outside.txt", ".git/HEAD"] {
        for route in ["/cli/git/stage", "/cli/git/unstage"] {
            let r = post(route, serde_json::json!({ "path": ws, "filePath": bad }));
            assert_eq!(r.status, "400 Bad Request", "{route} {bad}");
            assert_eq!(json(&r)["code"], "bad_path", "{route} {bad}");
        }
    }
    let key = git_changes::rev_key(&d);
    let r0 = git_changes::current_rev(&key);
    let st = post("/cli/git/stage", serde_json::json!({ "path": ws, "filePath": "b.txt" }));
    assert_eq!(st.status, "200 OK", "{}", st.body);
    let r1 = git_changes::current_rev(&key);
    assert_ne!(r0, r1, "stage bumps the rev");
    let abs = s(&d.join("b.txt"));
    let un = post("/cli/git/unstage", serde_json::json!({ "path": ws, "filePath": abs }));
    assert_eq!(un.status, "200 OK", "an absolute path inside the repo is accepted: {}", un.body);
    let r2 = git_changes::current_rev(&key);
    assert_ne!(r1, r2);
    assert_eq!(post("/cli/git/stage-all", serde_json::json!({ "path": ws })).status, "200 OK");
    let c = post("/cli/git/commit", serde_json::json!({ "path": ws, "message": "from the tab" }));
    assert_eq!(c.status, "200 OK", "{}", c.body);
    assert_ne!(git_changes::current_rev(&key), r2);
    let snap = json(&crate::cli::dispatch("/cli/git/snapshot", &params(&[("path", &ws)])));
    assert_eq!(snap["counts"]["total"], 0, "{snap}");
}

// ── agent passports (cell socket) ────────────────────────────────────

#[test]
fn agent_routes_are_scoped_to_the_own_workspace() {
    let d = repo("agent");
    let own = s(&d);
    std::fs::write(d.join("b.txt"), "b\n").expect("b");

    // No path → the agent's own workspace.
    let r = handle_agent_route("/cli/git/snapshot", false, &params(&[]), b"", &own);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["workspacePath"], own);
    let r = handle_agent_route("/cli/git/diff-file", false, &params(&[("file_path", "b.txt"), ("sides", "1")]), b"", &own);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json(&r)["new"]["text"], "b\n");

    // Another repo on the box → 403, even by `..`.
    let other = repo("agent-other");
    for p in [s(&other), format!("{own}/../{}", other.file_name().expect("name").to_string_lossy())] {
        let r = handle_agent_route("/cli/git/snapshot", false, &params(&[("path", &p)]), b"", &own);
        assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
        assert_eq!(json(&r)["code"], "forbidden");
        let body = serde_json::json!({ "path": p, "files": ["a.txt"] }).to_string();
        let w = handle_agent_route("/cli/git/discard", true, &params(&[]), body.as_bytes(), &own);
        assert_eq!(w.status, "403 Forbidden", "{p}: {}", w.body);
    }
    assert_eq!(std::fs::read_to_string(other.join("a.txt")).expect("a"), "one\ntwo\n");

    // POST with the path left out stages in the own workspace.
    let st = handle_agent_route("/cli/git/stage", true, &params(&[]), br#"{"filePath":"b.txt"}"#, &own);
    assert_eq!(st.status, "200 OK", "{}", st.body);
    let snap = json(&handle_agent_route("/cli/git/snapshot", false, &params(&[]), b"", &own));
    assert_eq!(snap["files"][0]["group"], "staged");

    // Method mismatches are 405; non-agent git routes are not served here.
    assert_eq!(handle_agent_route("/cli/git/stage", false, &params(&[]), b"", &own).status, "405 Method Not Allowed");
    assert_eq!(handle_agent_route("/cli/git/snapshot", true, &params(&[]), b"{}", &own).status, "405 Method Not Allowed");
    assert_eq!(handle_agent_route("/cli/git/create-worktree", true, &params(&[]), b"{}", &own).status, "404 Not Found");
    assert_eq!(handle_agent_route("/cli/git/commit", true, &params(&[]), b"[1]", &own).status, "400 Bad Request");
    assert!(crate::session_token::is_agent_verb("/cli/git/snapshot"));
    assert!(crate::session_token::is_agent_verb("/cli/git/discard"));
    assert!(!crate::session_token::is_agent_verb("/cli/git/create-worktree"));
    assert!(!crate::session_token::is_agent_verb("/cli/git/merge-branch"));
}

#[test]
fn agent_route_refuses_a_workspace_inside_a_bigger_repo() {
    let d = repo("agent-mono");
    let pkg = d.join("pkg");
    std::fs::create_dir_all(&pkg).expect("pkg");
    let r = handle_agent_route("/cli/git/snapshot", false, &params(&[]), b"", &s(&pkg));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(json(&r)["error"].as_str().expect("hint").contains("reaches outside"));
}

// ── remove-worktree only removes a worktree of that workspace ───────

/// Register `d` as a K2 workspace (projects row) in this thread's DB.
fn register(d: &Path) -> String {
    let id = format!("p-{}", uuid::Uuid::new_v4().simple());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)", rusqlite::params![id, s(d)])
        .expect("project row");
    id
}

fn code(r: &CliResponse) -> String {
    json(r)["code"].as_str().unwrap_or_else(|| panic!("no code: {} {}", r.status, r.body)).to_string()
}

#[test]
fn remove_worktree_route_refuses_folders_that_are_not_worktrees() {
    let d = repo("rm-refuse");
    register(&d);
    let elsewhere = tmp("rm-elsewhere");
    std::fs::write(elsewhere.join("keep.txt"), "precious\n").expect("keep");

    let r = post("/cli/git/remove-worktree", serde_json::json!({ "projectPath": s(&d), "worktreePath": s(&elsewhere) }));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(code(&r), "not_a_worktree_of_project");
    assert_eq!(std::fs::read_to_string(elsewhere.join("keep.txt")).expect("intact"), "precious\n");

    // The workspace's own (main) checkout is refused too.
    let r = post("/cli/git/remove-worktree", serde_json::json!({ "projectPath": s(&d), "worktreePath": s(&d) }));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(code(&r), "not_a_worktree_of_project");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("intact"), "one\ntwo\n");
}

#[test]
fn remove_worktree_route_refuses_a_project_that_is_not_a_workspace() {
    let d = repo("rm-unregistered");
    let wt = git::create_worktree(&s(&d), "feature/unregistered").expect("worktree");
    let r = post("/cli/git/remove-worktree", serde_json::json!({ "projectPath": s(&d), "worktreePath": wt.path }));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    assert_eq!(code(&r), "not_found");
    assert!(Path::new(&wt.path).join("a.txt").is_file(), "worktree left in place");
}

#[test]
fn remove_worktree_route_removes_a_real_worktree_and_its_row() {
    let d = repo("rm-real");
    let project_id = register(&d);
    let wt = git::create_worktree(&s(&d), "feature/real").expect("worktree");
    let wt_path = wt.path.clone();
    let ws_id = format!("w-{}", uuid::Uuid::new_v4().simple());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        Workspace::create(&conn, &ws_id, &project_id, None, "worktree", Some(&wt.branch), &wt.branch, 1, Some(&wt.path))
            .expect("workspace row");
    }

    let r = post(
        "/cli/git/remove-worktree",
        serde_json::json!({ "projectPath": s(&d), "worktreePath": wt_path, "workspaceId": ws_id }),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(!Path::new(&wt_path).exists(), "worktree moved away");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = Workspace::list(&conn, &project_id).expect("list");
    assert!(rows.iter().all(|w| w.id != ws_id), "workspace row deleted");
}

// ── merge-branch never fast-forwards over uncommitted changes ───────

#[test]
fn merge_branch_route_refuses_a_fast_forward_over_uncommitted_changes() {
    let d = repo("merge-dirty");
    git(&d, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(d.join("a.txt"), "from feature\n").expect("feature a");
    git(&d, &["commit", "-q", "-am", "feature"]);
    git(&d, &["checkout", "-q", "-"]);
    std::fs::write(d.join("a.txt"), "uncommitted on main\n").expect("dirty a");

    let r = post("/cli/git/merge-branch", serde_json::json!({ "path": s(&d), "branch": "feature" }));
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert_eq!(code(&r), "main_has_uncommitted_changes");
    assert!(json(&r)["error"].as_str().expect("error").contains("a.txt"), "{}", r.body);
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "uncommitted on main\n");

    // With the change put back, the same fast-forward goes through.
    git(&d, &["checkout", "--", "a.txt"]);
    let ok = post("/cli/git/merge-branch", serde_json::json!({ "path": s(&d), "branch": "feature" }));
    assert_eq!(ok.status, "200 OK", "{}", ok.body);
    assert_eq!(json(&ok)["success"], true);
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "from feature\n");
}

// ── abort-merge only aborts a merge, on a workspace ─────────────────

#[test]
fn abort_merge_route_needs_a_workspace_and_a_merge_in_progress() {
    // Not a registered workspace: refused, nothing reset.
    let other = repo("abort-unregistered");
    std::fs::write(other.join("a.txt"), "unsaved work\n").expect("dirty");
    let r = post("/cli/git/abort-merge", serde_json::json!({ "path": s(&other) }));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    assert_eq!(code(&r), "not_found");
    assert_eq!(std::fs::read_to_string(other.join("a.txt")).expect("a"), "unsaved work\n");

    // A workspace with no merge in progress: refused, nothing reset.
    let d = repo("abort-nomerge");
    register(&d);
    std::fs::write(d.join("a.txt"), "unsaved work\n").expect("dirty");
    let r = post("/cli/git/abort-merge", serde_json::json!({ "path": s(&d) }));
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert_eq!(code(&r), "no_merge_in_progress");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "unsaved work\n");
}

// ── resolve only touches a conflicted file, on a workspace ──────────

#[test]
fn resolve_route_needs_a_workspace_and_a_conflicted_file() {
    let other = repo("resolve-unregistered");
    std::fs::write(other.join("a.txt"), "unsaved work\n").expect("dirty");
    let body = |d: &Path| serde_json::json!({ "path": s(d), "filePath": "a.txt", "resolution": "theirs" });
    let r = post("/cli/git/resolve", body(&other));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    assert_eq!(code(&r), "not_found");
    assert_eq!(std::fs::read_to_string(other.join("a.txt")).expect("a"), "unsaved work\n");

    let d = repo("resolve-noconflict");
    register(&d);
    std::fs::write(d.join("a.txt"), "unsaved work\n").expect("dirty");
    let r = post("/cli/git/resolve", body(&d));
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert_eq!(code(&r), "not_in_conflict");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).expect("a"), "unsaved work\n");
}

// ── delete-branch and create-worktree need a workspace ──────────────

fn has_branch(d: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .current_dir(d)
        .status()
        .expect("spawn git")
        .success()
}

#[test]
fn delete_branch_route_needs_a_workspace() {
    let other = repo("delbranch-unregistered");
    git(&other, &["branch", "keep-me"]);
    let r = post("/cli/git/delete-branch", serde_json::json!({ "path": s(&other), "branch": "keep-me" }));
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    assert_eq!(code(&r), "not_found");
    assert!(has_branch(&other, "keep-me"), "branch kept");

    let d = repo("delbranch-ws");
    register(&d);
    git(&d, &["branch", "done-work"]);
    let r = post("/cli/git/delete-branch", serde_json::json!({ "path": s(&d), "branch": "done-work" }));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(!has_branch(&d, "done-work"), "branch deleted on a workspace");
}

#[test]
fn create_worktree_route_needs_its_own_workspace() {
    let other = repo("mkwt-unregistered");
    let other_id = format!("p-{}", uuid::Uuid::new_v4().simple());
    let r = post(
        "/cli/git/create-worktree",
        serde_json::json!({ "projectPath": s(&other), "branch": "feature/x", "projectId": other_id }),
    );
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
    assert_eq!(code(&r), "not_found");
    assert!(!other.join(".worktrees").exists() && !other.join(".gitignore").exists(), "nothing created");

    let d = repo("mkwt-ws");
    let id = register(&d);
    let elsewhere = repo("mkwt-elsewhere");
    let elsewhere_id = register(&elsewhere);
    // The row would land under another workspace: refused.
    let r = post(
        "/cli/git/create-worktree",
        serde_json::json!({ "projectPath": s(&d), "branch": "feature/y", "projectId": elsewhere_id }),
    );
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    assert_eq!(code(&r), "project_mismatch");
    assert!(!d.join(".worktrees").exists(), "nothing created");

    let ok = post("/cli/git/create-worktree", serde_json::json!({ "projectPath": s(&d), "branch": "feature/y", "projectId": id }));
    assert_eq!(ok.status, "200 OK", "{}", ok.body);
    let path = json(&ok)["path"].as_str().expect("path").to_string();
    assert!(Path::new(&path).join("a.txt").is_file(), "worktree checked out");
}
