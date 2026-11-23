//! Which repo a run ships when the caller runs from inside a git repo that
//! isn't the workspace's (the workspace folder is a repo itself). A
//! different repo never falls back to the workspace's code without a word:
//! one inside the workspace ships instead, with a one-line note; one
//! outside it is refused with the way to make it work. The workspace's own
//! checkout and its worktrees keep the old path.

use std::path::{Path, PathBuf};

use k2_core::compute::proto::frames::Src;
use k2_core::compute::{self as compute, codes};

use crate::compute_routes::{source_for, warm_key, Refusal, SourceInfo, Ws, R, REPO_INVALID};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).expect("write");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", text.trim()]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Fixture {
    d: PathBuf,
    ws: Ws,
    ws_dir: PathBuf,
    main_head: String,
    wt: PathBuf,
    wt_head: String,
    /// A repo outside the workspace.
    outside: PathBuf,
    /// A clone of `outside` inside the workspace, at `.worktrees/other`.
    clone: PathBuf,
    clone_head: String,
    clone_prev: String,
    /// A worktree of `outside` that lives inside the workspace.
    linked: PathBuf,
    /// A worktree of the workspace's own repo that lives outside it.
    own_outside: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.d);
    }
}

fn fixture(tag: &str) -> Fixture {
    let d = std::env::temp_dir().join(format!("k2-compute-caller-{tag}-{}-{}", std::process::id(), compute::now()));
    let _ = std::fs::remove_dir_all(&d);
    let ws_dir = d.join("ws");
    std::fs::create_dir_all(&ws_dir).expect("mkdir");
    git(&ws_dir, &["init", "-q"]);
    std::fs::write(ws_dir.join(".gitignore"), ".worktrees/\n.claude/\n").expect("write");
    let main_head = commit(&ws_dir, "a.txt", "ws one\n");
    let wt = ws_dir.join(".claude/worktrees/x");
    git(&ws_dir, &["worktree", "add", "-q", "-b", "wt", wt.to_str().expect("utf8")]);
    let wt_head = commit(&wt, "a.txt", "ws two\n");
    let outside = d.join("outside");
    std::fs::create_dir_all(&outside).expect("mkdir");
    git(&outside, &["init", "-q"]);
    commit(&outside, "o.txt", "other one\n");
    commit(&outside, "o.txt", "other two\n");
    let clone = ws_dir.join(".worktrees/other");
    git(&d, &["clone", "-q", outside.to_str().expect("utf8"), clone.to_str().expect("utf8")]);
    let clone_prev = git(&clone, &["rev-parse", "HEAD"]);
    let clone_head = commit(&clone, "o.txt", "other three (clone only)\n");
    let linked = ws_dir.join(".worktrees/linked");
    git(&outside, &["worktree", "add", "-q", "-b", "linked", linked.to_str().expect("utf8")]);
    let own_outside = d.join("ws-wt-outside");
    git(&ws_dir, &["worktree", "add", "-q", "-b", "far", own_outside.to_str().expect("utf8")]);
    let ws = Ws { id: "ws-caller".into(), path: ws_dir.to_str().expect("utf8").into(), handle: "ws".into() };
    Fixture { d, ws, ws_dir, main_head, wt, wt_head, outside, clone, clone_head, clone_prev, linked, own_outside }
}

fn go(f: &Fixture, v: serde_json::Value) -> (R<(Src, SourceInfo)>, Vec<String>) {
    let mut w = vec![];
    let r = source_for(&f.ws, &v, &f.d.join("blob.dirty.tar"), &mut w);
    (r, w)
}

fn dir(p: &Path) -> String {
    p.to_str().expect("utf8").to_string()
}

const NOTE: &str = "shipping .worktrees/other (the repo you're in), not the workspace repo";

#[test]
fn the_workspace_checkout_and_its_worktrees_ship_as_before() {
    let f = fixture("own");
    let (r, w) = go(&f, serde_json::json!({ "repoDir": dir(&f.ws_dir) }));
    let (src, info) = r.expect("from the workspace checkout");
    assert_eq!((src.commit.as_str(), info.repo.as_deref(), info.checkout), (f.main_head.as_str(), None, "workspace"));
    assert!(w.iter().all(|x| !x.contains("shipping")), "{w:?}");
    let (r, w) = go(&f, serde_json::json!({ "repoDir": dir(&f.wt) }));
    let (src, info) = r.expect("from the workspace's worktree");
    assert_eq!((src.commit.as_str(), info.repo.as_deref(), info.checkout), (f.wt_head.as_str(), None, "worktree"));
    assert!(w.iter().all(|x| !x.contains("shipping")), "{w:?}");
    // A worktree of the workspace's own repo outside the workspace: the
    // workspace's checkout, with the note it always had.
    let (r, w) = go(&f, serde_json::json!({ "repoDir": dir(&f.own_outside) }));
    let (src, info) = r.expect("from the workspace's worktree outside it");
    assert_eq!((src.commit.as_str(), info.repo.as_deref()), (f.main_head.as_str(), None));
    assert!(w.iter().any(|x| x.contains("outside the workspace") && x.contains("own checkout")), "{w:?}");
}

#[test]
fn another_repo_inside_the_workspace_ships_itself_with_a_note() {
    let f = fixture("in");
    let (r, w) = go(&f, serde_json::json!({ "repoDir": dir(&f.clone), "cwd": "" }));
    let (src, info) = r.expect("from the clone inside the workspace");
    assert_eq!(src.commit, f.clone_head, "the clone's HEAD, not the workspace's");
    assert_eq!(info.repo.as_deref(), Some(".worktrees/other"));
    assert!(!info.caller_elsewhere, "the caller's folder is in the shipped repo");
    assert_eq!(src.project_key, compute::sync::project_key_in("ws-caller", compute::sync::origin_url(&f.clone).as_deref(), Some(".worktrees/other")));
    assert_eq!(w.iter().filter(|x| x.as_str() == NOTE).count(), 1, "{w:?}");
    // The warm-slot match looks at the same repo.
    let (pk, c) = warm_key(&f.ws, &serde_json::json!({ "repoDir": dir(&f.clone) })).expect("warm key");
    assert_eq!((pk, c.as_deref()), (src.project_key.clone(), Some(f.clone_head.as_str())));
    // --dirty: the clone's changes, not the workspace's.
    std::fs::write(f.clone.join("new.txt"), "n\n").expect("write");
    let (r, _) = go(&f, serde_json::json!({ "repoDir": dir(&f.clone), "dirty": true }));
    let (_, info) = r.expect("dirty from the clone");
    assert_eq!(info.dirty.map(|(c, u, _)| (c, u)), Some((0, 1)));
}

#[test]
fn sha_resolves_in_the_repo_shipped() {
    let f = fixture("sha");
    // A commit of the clone's repo, from inside the clone: found there.
    let (r, _) = go(&f, serde_json::json!({ "repoDir": dir(&f.clone), "sha": f.clone_prev }));
    assert_eq!(r.expect("the clone's older commit").0.commit, f.clone_prev);
    let short = &f.clone_head[..10];
    let (r, _) = go(&f, serde_json::json!({ "repoDir": dir(&f.clone), "sha": short }));
    assert_eq!(r.expect("a short sha of the clone").0.commit, f.clone_head);
    // The workspace's commit from inside the clone: not in the repo shipped.
    let (r, _) = go(&f, serde_json::json!({ "repoDir": dir(&f.clone), "sha": f.main_head }));
    assert_eq!(r.err().map(|e| e.code), Some(codes::BAD_REQUEST), "a workspace sha is not looked up for the clone");
    // And the clone's commit from the workspace checkout: not there either.
    let (r, _) = go(&f, serde_json::json!({ "repoDir": dir(&f.ws_dir), "sha": f.clone_head }));
    assert_eq!(r.err().map(|e| e.code), Some(codes::BAD_REQUEST));
}

fn assert_guided(e: &Refusal, what: &str) {
    assert_eq!((e.code, e.status), (REPO_INVALID, 400), "{what}: {}", e.hint);
    assert!(e.hint.contains(".worktrees/<name>") && e.hint.contains("--repo .worktrees/<name>"), "{what}: {}", e.hint);
    assert!(e.hint.contains("run from inside"), "{what}: {}", e.hint);
}

#[test]
fn a_repo_outside_the_workspace_is_refused_with_the_way_in() {
    let f = fixture("out");
    let (r, w) = go(&f, serde_json::json!({ "repoDir": dir(&f.outside), "sha": "HEAD" }));
    let e = r.err().unwrap_or_else(|| panic!("from a repo outside the workspace must be refused, warned {w:?}"));
    assert_guided(&e, "cwd outside");
    assert!(e.hint.contains(&dir(&f.outside)) || e.hint.contains("outside the workspace"), "{}", e.hint);
    // Inside the workspace folder, but a worktree of the outside repo.
    let (r, w) = go(&f, serde_json::json!({ "repoDir": dir(&f.linked) }));
    let e = r.err().unwrap_or_else(|| panic!("a worktree of an outside repo must be refused, warned {w:?}"));
    assert_guided(&e, "cwd in a worktree of an outside repo");
    // The same one named with --repo: refused as before, now with the way in.
    let (r, _) = go(&f, serde_json::json!({ "repo": ".worktrees/linked" }));
    assert_guided(&r.err().expect("--repo naming a worktree of an outside repo"), "--repo linked");
    // --repo naming the clone inside the workspace still works, from anywhere.
    let (r, _) = go(&f, serde_json::json!({ "repo": ".worktrees/other", "repoDir": dir(&f.outside) }));
    assert_eq!(r.expect("--repo the clone").0.commit, f.clone_head);
}

/// M2 review D1: a fenced workspace ships no source (403
/// `fenced_no_source`, with `--no-source` named) until its git steps run
/// under its fence. Off again, it ships as before.
#[test]
fn a_fenced_agent_ships_no_source_yet() {
    use crate::compute_fenced::{refusal as fenced_source_refusal, requested_by_passport, FENCED_NO_SOURCE};
    use crate::compute_routes::Who;
    use k2_core::connect_users::Role;
    let f = fixture("fenced");
    let set = |mode: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, name, path, isolation_mode) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![f.ws.id, f.ws.handle, f.ws.path, mode],
        )
        .expect("project row");
    };
    let agent = Who::Agent { project_id: f.ws.id.clone(), session_id: "s-agent".into() };
    let shell = Who::Shell { project_id: f.ws.id.clone(), session_id: "s-shell".into() };
    let login = Who::Login { username: "pat".into(), role: Role::Admin };
    set("fence");
    for who in [&agent, &shell] {
        let e = fenced_source_refusal(who, &f.ws).err().expect("a fenced workspace's session ships no source");
        assert_eq!((e.code, e.status), (FENCED_NO_SOURCE, 403), "{}: {}", who.label(), e.hint);
        assert!(e.hint.contains("--no-source"), "{}", e.hint);
        assert!(requested_by_passport(&who.label()), "{}", who.label());
    }
    for who in [&Who::Owner, &login] {
        assert!(fenced_source_refusal(who, &f.ws).is_ok(), "{}: the owner's own run ships as before", who.label());
        assert!(!requested_by_passport(&who.label()), "{}", who.label());
    }
    let (r, _) = go(&f, serde_json::json!({ "repoDir": dir(&f.ws_dir) }));
    assert!(r.is_ok(), "fenced, the owner's run: as before: {:?}", r.err().map(|e| e.hint));
    set("off");
    assert!(fenced_source_refusal(&agent, &f.ws).is_ok(), "unfenced: an agent's run as before");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM projects WHERE id = ?1", [&f.ws.id]).expect("cleanup");
}
