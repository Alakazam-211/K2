//! Tests for `git_changes` (Changes tab v1, C1). Every test builds its
//! own temp repo; discard tests trash into a temp dir (never the OS Trash).

use super::*;
use std::fs;
use std::path::{Path, PathBuf};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-gitchg-{tag}-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&d).expect("mkdir temp");
    d
}

fn init(dir: &Path) -> Repository {
    let repo = Repository::init(dir).expect("git init");
    let mut cfg = repo.config().expect("config");
    cfg.set_str("user.name", "K2 Test").expect("user.name");
    cfg.set_str("user.email", "k2-test@example.com").expect("user.email");
    repo
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("mkdir parent");
    }
    fs::write(p, body).expect("write file");
}

fn stage(repo: &Repository, rel: &str) {
    let mut idx = repo.index().expect("index");
    idx.add_path(Path::new(rel)).expect("add_path");
    idx.write().expect("write index");
}

fn stage_remove(repo: &Repository, rel: &str) {
    let mut idx = repo.index().expect("index");
    idx.remove_path(Path::new(rel)).expect("remove_path");
    idx.write().expect("write index");
}

fn commit_all(repo: &Repository, msg: &str) -> Oid {
    let mut idx = repo.index().expect("index");
    idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).expect("add_all");
    idx.update_all(["*"].iter(), None).expect("update_all");
    idx.write().expect("write index");
    let tree_id = idx.write_tree().expect("write_tree");
    let tree = repo.find_tree(tree_id).expect("tree");
    let sig = repo.signature().expect("sig");
    let parents: Vec<git2::Commit<'_>> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
    let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs).expect("commit")
}

fn snap(dir: &Path, base: Option<&str>) -> Snapshot {
    snapshot(dir.to_str().expect("utf8"), base).expect("snapshot ok").repo().expect("is a repo")
}

fn row<'a>(s: &'a Snapshot, group: &str, path: &str) -> &'a ChangeRow {
    s.files
        .iter()
        .find(|r| r.group == group && r.path == path)
        .unwrap_or_else(|| panic!("no {group} row for {path} in {:#?}", s.files))
}

fn trash_into(dir: PathBuf) -> impl Fn(&Path) -> Result<(), String> {
    move |p: &Path| {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name = p.file_name().expect("name").to_string_lossy().into_owned();
        fs::rename(p, dir.join(format!("{name}-{}", uuid::Uuid::new_v4().simple()))).map_err(|e| e.to_string())
    }
}

fn trashed_bodies(dir: &Path, stem: &str) -> Vec<String> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).expect("read trash dir") {
        let e = e.expect("entry");
        if e.file_name().to_string_lossy().starts_with(&format!("{stem}-")) && e.path().is_file() {
            out.push(fs::read_to_string(e.path()).expect("read trashed"));
        }
    }
    out
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf8 path")
}

// ── paths ────────────────────────────────────────────────────────────

#[test]
fn normalize_rel_path_accepts_inside_and_refuses_escapes() {
    let wd = tmp("norm");
    assert_eq!(normalize_rel_path(&wd, "src/a.rs").expect("rel"), "src/a.rs");
    assert_eq!(normalize_rel_path(&wd, "./src/a.rs").expect("dot"), "src/a.rs");
    assert_eq!(normalize_rel_path(&wd, "newdir/").expect("dir row"), "newdir");
    let abs = wd.join("src/b.rs");
    assert_eq!(normalize_rel_path(&wd, s(&abs)).expect("abs inside"), "src/b.rs");
    // git2 may report the workdir canonical (/private/var/…) while the
    // client spells it as given (/var/…): still inside.
    let canon_wd = fs::canonicalize(&wd).expect("canon");
    assert_eq!(normalize_rel_path(&canon_wd, s(&abs)).expect("other spelling"), "src/b.rs");
    for bad in ["", "   ", "../x", "src/../../etc/passwd", "/etc/passwd", ".git/config", ".GIT/HEAD", "a\0b", "."] {
        let err = normalize_rel_path(&wd, bad).expect_err(bad);
        assert_eq!(err.code, codes::BAD_PATH, "{bad:?} → {err}");
    }
}

#[test]
fn rev_bumps_and_is_stable_between_bumps() {
    let key = format!("/k2-test-rev/{}", uuid::Uuid::new_v4());
    let a = current_rev(&key);
    assert_eq!(a, current_rev(&key));
    let b = bump_rev(&key);
    assert_ne!(a, b);
    assert_eq!(b, current_rev(&key));
    assert_ne!(bump_rev(&key), b);
}

// ── snapshot ─────────────────────────────────────────────────────────

#[test]
fn snapshot_not_a_repo() {
    let d = tmp("norepo");
    let r = snapshot(s(&d), None).expect("ok");
    assert!(r.clone().repo().is_none());
    let json = serde_json::to_value(&r).expect("json");
    assert_eq!(json["isRepo"], false);
    assert_eq!(json["workspacePath"], s(&d).trim_end_matches('/'));
    assert!(json["rev"].as_str().expect("rev").contains(':'));
}

#[test]
fn snapshot_groups_statuses_counts_and_branch() {
    let d = tmp("groups");
    let repo = init(&d);
    write(&d, "keep.txt", "one\ntwo\nthree\n");
    write(&d, "gone.txt", "bye\n");
    write(&d, "both.txt", "a\n");
    write(&d, "old_name.txt", "rename me\nline2\nline3\nline4\n");
    commit_all(&repo, "init");

    write(&d, "keep.txt", "one\nTWO\nthree\nfour\n"); // unstaged M (+2 -1)
    fs::remove_file(d.join("gone.txt")).expect("rm"); // unstaged D
    write(&d, "both.txt", "a\nb\n");
    stage(&repo, "both.txt");
    write(&d, "both.txt", "a\nb\nc\n"); // staged M + unstaged M
    write(&d, "added.txt", "x\ny\n");
    stage(&repo, "added.txt"); // staged A
    fs::rename(d.join("old_name.txt"), d.join("new_name.txt")).expect("mv");
    stage_remove(&repo, "old_name.txt");
    stage(&repo, "new_name.txt"); // staged R
    write(&d, "untracked.txt", "u1\nu2\nu3"); // U, 3 lines (no trailing newline)
    write(&d, "newdir/inner.txt", "z\n"); // collapsed dir row
    write(&d, "bin.dat", "\0\u{1}\u{2}binary");

    let sn = snap(&d, None);
    assert!(sn.is_repo);
    let head_name = repo.head().expect("head").shorthand().expect("name").to_string();
    assert_eq!(sn.branch.as_deref(), Some(head_name.as_str()));
    assert!(!sn.detached && !sn.unborn);
    assert!(sn.head_oid.is_some());
    assert_eq!(sn.state, "clean");
    assert_eq!(sn.upstream, None);
    assert_eq!((sn.ahead, sn.behind), (0, 0));
    assert_eq!(rev_key(Path::new(&sn.repo_root)), rev_key(&d));
    assert!(!sn.repo_root.ends_with('/'));
    assert_eq!(sn.workspace_path, s(&d).trim_end_matches('/'));
    assert!(!sn.truncated);
    assert_eq!(sn.limit, MAX_FILES);

    let k = row(&sn, "unstaged", "keep.txt");
    assert_eq!((k.status, k.additions, k.deletions, k.binary), ("M", Some(2), Some(1), false));
    assert_eq!(row(&sn, "unstaged", "gone.txt").status, "D");
    assert_eq!(row(&sn, "unstaged", "gone.txt").deletions, Some(1));
    let bs = row(&sn, "staged", "both.txt");
    assert_eq!((bs.status, bs.additions, bs.deletions), ("M", Some(1), Some(0)));
    let bu = row(&sn, "unstaged", "both.txt");
    assert_eq!((bu.status, bu.additions, bu.deletions), ("M", Some(1), Some(0)));
    let a = row(&sn, "staged", "added.txt");
    assert_eq!((a.status, a.additions), ("A", Some(2)));
    let r = row(&sn, "staged", "new_name.txt");
    assert_eq!(r.status, "R");
    assert_eq!(r.old_path.as_deref(), Some("old_name.txt"));
    let u = row(&sn, "untracked", "untracked.txt");
    assert_eq!((u.status, u.additions, u.deletions, u.dir), ("U", Some(3), Some(0), false));
    let nd = row(&sn, "untracked", "newdir/");
    assert!(nd.dir);
    assert_eq!(nd.additions, None);
    let b = row(&sn, "untracked", "bin.dat");
    assert!(b.binary);
    assert_eq!(b.additions, None);

    assert_eq!(sn.counts.staged, 3);
    assert_eq!(sn.counts.unstaged, 3);
    assert_eq!(sn.counts.untracked, 3);
    assert_eq!(sn.counts.conflicted, 0);
    assert_eq!(sn.counts.total, 9);
    assert_eq!(sn.files.len(), 9);
    // Sorted: staged, then unstaged, then untracked; by path within.
    let order: Vec<(&str, &str)> = sn.files.iter().map(|r| (r.group, r.path.as_str())).collect();
    assert_eq!(
        order,
        vec![
            ("staged", "added.txt"),
            ("staged", "both.txt"),
            ("staged", "new_name.txt"),
            ("unstaged", "both.txt"),
            ("unstaged", "gone.txt"),
            ("unstaged", "keep.txt"),
            ("untracked", "bin.dat"),
            ("untracked", "newdir/"),
            ("untracked", "untracked.txt"),
        ]
    );
    let json = serde_json::to_value(&sn).expect("json");
    for k in ["isRepo", "workspacePath", "repoRoot", "rev", "branch", "headOid", "detached", "unborn", "upstream", "ahead", "behind", "state", "base", "counts", "truncated", "limit", "files"] {
        assert!(json.get(k).is_some(), "snapshot JSON missing {k}: {json}");
    }
    assert_eq!(json["files"][2]["oldPath"], "old_name.txt");
}

#[test]
fn snapshot_upstream_ahead_behind_and_detached_and_unborn() {
    let d = tmp("upstream");
    let repo = init(&d);
    write(&d, "a.txt", "1\n");
    let c1 = commit_all(&repo, "c1");
    let head_name = repo.head().expect("head").shorthand().expect("name").to_string();
    // A local branch as the upstream, one commit behind us.
    repo.branch("up", &repo.find_commit(c1).expect("c1"), false).expect("branch up");
    write(&d, "a.txt", "2\n");
    commit_all(&repo, "c2");
    repo.find_branch(&head_name, git2::BranchType::Local)
        .expect("local")
        .set_upstream(Some("up"))
        .expect("set upstream");
    let sn = snap(&d, None);
    assert_eq!(sn.branch.as_deref(), Some(head_name.as_str()));
    assert_eq!(sn.upstream.as_deref(), Some("up"));
    assert_eq!((sn.ahead, sn.behind), (1, 0));

    repo.set_head_detached(c1).expect("detach");
    let sn = snap(&d, None);
    assert!(sn.detached);
    assert_eq!(sn.branch, None);
    assert_eq!(sn.head_oid.as_deref(), Some(c1.to_string().as_str()));

    let e = tmp("unborn");
    init(&e);
    write(&e, "first.txt", "hi\n");
    let sn = snap(&e, None);
    assert!(sn.unborn);
    assert_eq!(sn.head_oid, None);
    assert!(sn.branch.is_some(), "unborn branch name comes from HEAD's symbolic target");
    assert_eq!(row(&sn, "untracked", "first.txt").additions, Some(1));
}

#[test]
fn snapshot_caps_at_1000_rows_and_keeps_full_counts() {
    let d = tmp("cap");
    let repo = init(&d);
    write(&d, "seed.txt", "s\n");
    commit_all(&repo, "seed");
    for i in 0..(MAX_FILES + 5) {
        write(&d, &format!("f{i:05}.txt"), "x\n");
    }
    let sn = snap(&d, None);
    assert!(sn.truncated);
    assert_eq!(sn.files.len(), MAX_FILES);
    assert_eq!(sn.counts.untracked, MAX_FILES + 5);
    assert_eq!(sn.counts.total, MAX_FILES + 5);
}

#[test]
fn snapshot_against_base_lists_the_whole_branch() {
    let d = tmp("base");
    let repo = init(&d);
    write(&d, "shared.txt", "base\n");
    write(&d, "doomed.txt", "x\n");
    let c1 = commit_all(&repo, "base");
    let main_name = repo.head().expect("head").shorthand().expect("name").to_string();
    let c1c = repo.find_commit(c1).expect("c1");
    repo.branch("feature", &c1c, false).expect("feature");
    // main moves on (feature is 1 behind).
    write(&d, "main_only.txt", "m\n");
    commit_all(&repo, "main moves");
    repo.checkout_tree(c1c.as_object(), Some(git2::build::CheckoutBuilder::new().force())).expect("checkout");
    repo.set_head("refs/heads/feature").expect("switch");
    assert!(!d.join("main_only.txt").exists(), "feature checkout drops main's file");
    write(&d, "committed.txt", "c\n");
    fs::remove_file(d.join("doomed.txt")).expect("rm");
    commit_all(&repo, "feature work");
    write(&d, "shared.txt", "base\nedited\n"); // uncommitted
    write(&d, "loose.txt", "l\n"); // untracked

    let sn = snap(&d, Some(&main_name));
    let base = sn.base.as_ref().expect("base info");
    assert_eq!(base.ref_name, main_name);
    assert_eq!(base.merge_base.as_deref(), Some(c1.to_string().as_str()));
    assert_eq!((base.ahead, base.behind), (1, 1));
    assert_eq!(row(&sn, "base", "committed.txt").status, "A");
    assert_eq!(row(&sn, "base", "doomed.txt").status, "D");
    let sh = row(&sn, "base", "shared.txt");
    assert_eq!((sh.status, sh.additions, sh.deletions), ("M", Some(1), Some(0)));
    assert_eq!(row(&sn, "untracked", "loose.txt").status, "U");
    assert!(sn.files.iter().all(|r| r.group == "base" || r.group == "untracked"), "{:#?}", sn.files);
    assert!(!sn.files.iter().any(|r| r.path == "main_only.txt"), "main's own commit is not branch work");
    assert_eq!(sn.counts.base, 3);

    let err = snapshot(s(&d), Some("no-such-branch")).expect_err("bad ref");
    assert_eq!(err.code, codes::BAD_REF);
}

// ── diff_sides ───────────────────────────────────────────────────────

#[test]
fn diff_sides_modes_return_both_sides() {
    let d = tmp("sides");
    let repo = init(&d);
    write(&d, "f.txt", "head\n");
    commit_all(&repo, "c1");
    write(&d, "f.txt", "index\n");
    stage(&repo, "f.txt");
    write(&d, "f.txt", "work\nmore\n");

    let w = diff_sides(s(&d), "f.txt", None, DiffMode::Worktree).expect("worktree");
    assert_eq!((w.old.label.as_str(), w.old.text.as_deref()), ("HEAD", Some("head\n")));
    assert_eq!((w.new.label.as_str(), w.new.text.as_deref()), ("WORKTREE", Some("work\nmore\n")));
    assert_eq!((w.additions, w.deletions), (Some(2), Some(1)));
    assert_eq!(w.mode, "worktree");
    assert!(!w.binary && !w.too_large);

    let u = diff_sides(s(&d), "f.txt", None, DiffMode::Unstaged).expect("unstaged");
    assert_eq!((u.old.label.as_str(), u.old.text.as_deref()), ("INDEX", Some("index\n")));
    assert_eq!(u.new.text.as_deref(), Some("work\nmore\n"));

    let st = diff_sides(s(&d), "f.txt", None, DiffMode::Staged).expect("staged");
    assert_eq!(st.old.text.as_deref(), Some("head\n"));
    assert_eq!((st.new.label.as_str(), st.new.text.as_deref()), ("INDEX", Some("index\n")));
    assert_eq!(st.old.size, 5);

    let json = serde_json::to_value(&w).expect("json");
    for k in ["path", "oldPath", "mode", "baseRef", "rev", "old", "new", "binary", "tooLarge", "additions", "deletions"] {
        assert!(json.get(k).is_some(), "diff JSON missing {k}");
    }
    for k in ["label", "exists", "text", "size", "binary", "tooLarge"] {
        assert!(json["old"].get(k).is_some(), "side JSON missing {k}");
    }
}

#[test]
fn diff_sides_added_deleted_renamed_and_base() {
    let d = tmp("sides2");
    let repo = init(&d);
    write(&d, "del.txt", "gone\n");
    write(&d, "orig.txt", "same\n");
    let c1 = commit_all(&repo, "c1");
    let main_name = repo.head().expect("head").shorthand().expect("n").to_string();
    write(&d, "new.txt", "fresh\n");
    fs::remove_file(d.join("del.txt")).expect("rm");

    let a = diff_sides(s(&d), "new.txt", None, DiffMode::Worktree).expect("added");
    assert!(!a.old.exists);
    assert_eq!(a.old.text.as_deref(), Some(""));
    assert!(a.new.exists);
    let del = diff_sides(s(&d), "del.txt", None, DiffMode::Worktree).expect("deleted");
    assert!(del.old.exists && !del.new.exists);
    assert_eq!(del.deletions, Some(1));

    fs::rename(d.join("orig.txt"), d.join("moved.txt")).expect("mv");
    let r = diff_sides(s(&d), "moved.txt", Some("orig.txt"), DiffMode::Worktree).expect("rename");
    assert_eq!(r.old_path.as_deref(), Some("orig.txt"));
    assert_eq!(r.old.text.as_deref(), Some("same\n"));
    assert_eq!((r.additions, r.deletions), (Some(0), Some(0)));

    let c1c = repo.find_commit(c1).expect("c1");
    repo.branch("topic", &c1c, false).expect("topic");
    let b = diff_sides(s(&d), "new.txt", None, DiffMode::Base(main_name.clone())).expect("base");
    assert_eq!(b.mode, "base");
    assert_eq!(b.base_ref.as_deref(), Some(main_name.as_str()));
    assert_eq!(b.old.label, main_name);

    let nf = diff_sides(s(&d), "never.txt", None, DiffMode::Worktree).expect_err("missing");
    assert_eq!(nf.code, codes::NOT_FOUND);
    let br = diff_sides(s(&d), "new.txt", None, DiffMode::Base("nope".into())).expect_err("bad ref");
    assert_eq!(br.code, codes::BAD_REF);
    let bp = diff_sides(s(&d), "../outside.txt", None, DiffMode::Worktree).expect_err("traversal");
    assert_eq!(bp.code, codes::BAD_PATH);
    let nr = diff_sides(s(&tmp("sides-norepo")), "x", None, DiffMode::Worktree).expect_err("no repo");
    assert_eq!(nr.code, codes::NOT_A_REPO);
    assert_eq!(DiffMode::parse(Some("bogus"), false, None).expect_err("mode").code, codes::BAD_REQUEST);
    assert_eq!(DiffMode::parse(None, true, None).expect("staged"), DiffMode::Staged);
    assert_eq!(DiffMode::parse(Some("staged"), false, Some("main")).expect("base"), DiffMode::Base("main".into()));
}

#[test]
fn diff_sides_binary_and_too_large() {
    let d = tmp("caps");
    let repo = init(&d);
    write(&d, "seed.txt", "s\n");
    commit_all(&repo, "seed");
    fs::write(d.join("img.bin"), [0u8, 1, 2, 3, 0, 9]).expect("bin");
    let b = diff_sides(s(&d), "img.bin", None, DiffMode::Worktree).expect("binary");
    assert!(b.binary && b.new.binary);
    assert_eq!(b.new.text, None);
    assert_eq!((b.additions, b.deletions), (None, None));

    let many: String = (0..(MAX_LINES + 1)).map(|i| format!("{i}\n")).collect();
    write(&d, "long.txt", &many);
    let l = diff_sides(s(&d), "long.txt", None, DiffMode::Worktree).expect("long");
    assert!(l.too_large && l.new.too_large);
    assert_eq!(l.new.text, None);

    let big = vec![b'a'; (MAX_BLOB_BYTES + 1) as usize];
    fs::write(d.join("big.txt"), &big).expect("big");
    let g = diff_sides(s(&d), "big.txt", None, DiffMode::Worktree).expect("big");
    assert!(g.too_large);
    assert_eq!(g.new.size, MAX_BLOB_BYTES + 1);
    assert_eq!(g.new.text, None);
}

#[cfg(unix)]
#[test]
fn diff_sides_never_follows_symlinks_out_of_the_repo() {
    let d = tmp("symlink");
    let outside = tmp("symlink-outside");
    fs::write(outside.join("secret.txt"), "top secret\n").expect("secret");
    let repo = init(&d);
    write(&d, "seed.txt", "s\n");
    commit_all(&repo, "seed");
    std::os::unix::fs::symlink(outside.join("secret.txt"), d.join("link.txt")).expect("link file");
    let l = diff_sides(s(&d), "link.txt", None, DiffMode::Worktree).expect("link");
    assert_eq!(l.new.text.as_deref(), Some(s(&outside.join("secret.txt"))), "a symlink side is its target path");
    std::os::unix::fs::symlink(&outside, d.join("linkdir")).expect("link dir");
    let e = diff_sides(s(&d), "linkdir/secret.txt", None, DiffMode::Worktree).expect_err("escape");
    assert_eq!(e.code, codes::BAD_PATH);
}

// ── discard ──────────────────────────────────────────────────────────

#[test]
fn discard_untracked_goes_to_trash_and_tracked_is_backed_up_then_restored() {
    let d = tmp("discard");
    let trash = tmp("discard-trash");
    let repo = init(&d);
    write(&d, "tracked.txt", "orig\n");
    write(&d, "staged.txt", "s0\n");
    commit_all(&repo, "c1");
    write(&d, "tracked.txt", "agent edit\n");
    write(&d, "staged.txt", "s1\n");
    stage(&repo, "staged.txt");
    write(&d, "staged.txt", "s2\n");
    write(&d, "junk.txt", "untracked junk\n");
    write(&d, "junkdir/a.txt", "a\n");

    let files: Vec<String> = ["tracked.txt", "staged.txt", "junk.txt", "junkdir/", "clean-missing.txt"].iter().map(|s| s.to_string()).collect();
    let out = discard(s(&d), &files, false, &trash_into(trash.clone())).expect("discard");

    assert_eq!(fs::read_to_string(d.join("tracked.txt")).expect("read"), "orig\n");
    assert_eq!(fs::read_to_string(d.join("staged.txt")).expect("read"), "s1\n", "default mode keeps the staged copy");
    assert!(!d.join("junk.txt").exists());
    assert!(!d.join("junkdir").exists());
    assert_eq!(trashed_bodies(&trash, "junk.txt"), vec!["untracked junk\n".to_string()]);
    assert_eq!(trashed_bodies(&trash, "tracked.txt"), vec!["agent edit\n".to_string()], "the lost edit is in the Trash");
    assert_eq!(trashed_bodies(&trash, "staged.txt"), vec!["s2\n".to_string()]);
    assert!(trash.read_dir().expect("trash").any(|e| e.expect("e").file_name().to_string_lossy().starts_with("junkdir-")));
    assert_eq!(out.restored, vec!["tracked.txt".to_string(), "staged.txt".to_string()]);
    assert_eq!(out.trashed.len(), 4);
    assert!(out.unstaged.is_empty());
    assert_eq!(out.skipped, vec![DiscardSkip { path: "clean-missing.txt".into(), reason: "not found".into() }]);
    assert!(out.failed.is_empty());
    // Index still holds the staged version.
    let sn = snap(&d, None);
    assert_eq!(row(&sn, "staged", "staged.txt").status, "M");
    assert!(sn.files.iter().all(|r| r.group == "staged"), "{:#?}", sn.files);
}

#[test]
fn discard_include_staged_resets_to_head_and_unstages_adds() {
    let d = tmp("discard-all");
    let trash = tmp("discard-all-trash");
    let repo = init(&d);
    write(&d, "f.txt", "head\n");
    write(&d, "only_staged.txt", "h\n");
    commit_all(&repo, "c1");
    write(&d, "f.txt", "staged\n");
    stage(&repo, "f.txt");
    write(&d, "f.txt", "worktree\n");
    write(&d, "only_staged.txt", "h2\n");
    stage(&repo, "only_staged.txt");
    write(&d, "brand_new.txt", "new\n");
    stage(&repo, "brand_new.txt");

    let files = vec!["f.txt".to_string(), "only_staged.txt".to_string(), "brand_new.txt".to_string()];
    let out = discard(s(&d), &files, true, &trash_into(trash.clone())).expect("discard");
    assert_eq!(fs::read_to_string(d.join("f.txt")).expect("f"), "head\n");
    assert_eq!(fs::read_to_string(d.join("only_staged.txt")).expect("o"), "h\n");
    assert!(!d.join("brand_new.txt").exists());
    assert_eq!(trashed_bodies(&trash, "brand_new.txt"), vec!["new\n".to_string()]);
    assert_eq!(trashed_bodies(&trash, "f.txt"), vec!["worktree\n".to_string()]);
    assert_eq!(out.unstaged.len(), 3, "{out:?}");
    assert_eq!(out.restored, vec!["f.txt".to_string(), "only_staged.txt".to_string()]);
    let sn = snap(&d, None);
    assert_eq!(sn.counts.total, 0, "everything discarded: {:#?}", sn.files);
}

#[test]
fn discard_refuses_bad_paths_and_conflicts_without_touching_anything() {
    let d = tmp("discard-refuse");
    let trash = tmp("discard-refuse-trash");
    let repo = init(&d);
    write(&d, "a.txt", "a\n");
    commit_all(&repo, "c1");
    write(&d, "a.txt", "edited\n");
    let t = trash_into(trash.clone());

    for bad in ["../escape.txt", "/etc/hosts", ".git/config"] {
        let files = vec!["a.txt".to_string(), bad.to_string()];
        let e = discard(s(&d), &files, false, &t).expect_err(bad);
        assert_eq!(e.code, codes::BAD_PATH, "{bad}");
        assert_eq!(fs::read_to_string(d.join("a.txt")).expect("a"), "edited\n", "nothing changed on refusal");
    }
    assert_eq!(discard(s(&d), &[], false, &t).expect_err("empty").code, codes::BAD_REQUEST);
    let too_many: Vec<String> = (0..=MAX_DISCARD_FILES).map(|i| format!("f{i}")).collect();
    assert_eq!(discard(s(&d), &too_many, false, &t).expect_err("cap").code, codes::BAD_REQUEST);
    assert_eq!(
        discard(s(&tmp("discard-norepo")), &["x".to_string()], false, &t).expect_err("norepo").code,
        codes::NOT_A_REPO
    );

    // A conflicted index entry (stages 1–3) refuses the whole request.
    let mut idx = repo.index().expect("index");
    let blob = repo.blob(b"x\n").expect("blob");
    let entry = |stage_n: u16| git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o100644,
        uid: 0,
        gid: 0,
        file_size: 2,
        id: blob,
        flags: stage_n << 12,
        flags_extended: 0,
        path: b"conf.txt".to_vec(),
    };
    for stage_n in 1..=3u16 {
        idx.add(&entry(stage_n)).expect("conflict stage entry");
    }
    idx.write().expect("write");
    assert!(repo.index().expect("reread").has_conflicts(), "fixture holds a conflict");
    write(&d, "conf.txt", "<<<<<<<\n");
    let e = discard(s(&d), &["a.txt".to_string(), "conf.txt".to_string()], false, &t).expect_err("conflict");
    assert_eq!(e.code, codes::CONFLICTED);
    assert_eq!(fs::read_to_string(d.join("a.txt")).expect("a"), "edited\n");
    assert_eq!(fs::read_dir(&trash).expect("trash").count(), 0, "nothing went to the trash");
    let sn = snap(&d, None);
    assert_eq!(row(&sn, "conflicted", "conf.txt").status, "C");
}

#[test]
fn discard_restores_a_glob_named_file_literally() {
    let d = tmp("discard-glob");
    let trash = tmp("discard-glob-trash");
    let repo = init(&d);
    write(&d, "star*.txt", "s0\n");
    write(&d, "starX.txt", "x0\n");
    commit_all(&repo, "c1");
    write(&d, "star*.txt", "s1\n");
    write(&d, "starX.txt", "x1\n");
    let out = discard(s(&d), &["star*.txt".to_string()], false, &trash_into(trash)).expect("discard");
    assert_eq!(out.restored, vec!["star*.txt".to_string()]);
    assert_eq!(fs::read_to_string(d.join("star*.txt")).expect("star"), "s0\n");
    assert_eq!(fs::read_to_string(d.join("starX.txt")).expect("x"), "x1\n", "the glob must not match a sibling");
}

#[test]
fn discard_reports_trash_failure_and_leaves_the_file() {
    let d = tmp("discard-fail");
    let repo = init(&d);
    write(&d, "a.txt", "a\n");
    commit_all(&repo, "c1");
    write(&d, "a.txt", "precious\n");
    write(&d, "u.txt", "u\n");
    let failing = |_: &Path| -> Result<(), String> { Err("trash unavailable".into()) };
    let out = discard(s(&d), &["a.txt".to_string(), "u.txt".to_string()], false, &failing).expect("200 with failures");
    assert_eq!(out.failed.len(), 2);
    assert!(out.restored.is_empty() && out.trashed.is_empty());
    assert_eq!(fs::read_to_string(d.join("a.txt")).expect("a"), "precious\n", "never overwritten without a backup");
    assert!(d.join("u.txt").exists());
}

#[test]
fn discard_skips_ignored_and_clean_files() {
    let d = tmp("discard-skip");
    let trash = tmp("discard-skip-trash");
    let repo = init(&d);
    write(&d, ".gitignore", "*.log\n");
    write(&d, "clean.txt", "c\n");
    commit_all(&repo, "c1");
    write(&d, "x.log", "log\n");
    let out = discard(s(&d), &["x.log".to_string(), "clean.txt".to_string()], false, &trash_into(trash)).expect("ok");
    assert_eq!(
        out.skipped,
        vec![
            DiscardSkip { path: "x.log".into(), reason: "ignored".into() },
            DiscardSkip { path: "clean.txt".into(), reason: "nothing to discard".into() },
        ]
    );
    assert!(d.join("x.log").exists());
}

#[test]
fn trash_path_override_moves_into_dir() {
    let d = tmp("trash-ovr");
    let dest = tmp("trash-ovr-dest");
    fs::write(d.join("f.txt"), "keep me\n").expect("w");
    // Not the global override (tests run in parallel): the seam is the
    // same rename the override uses.
    trash_into(dest.clone())(&d.join("f.txt")).expect("trash");
    assert_eq!(trashed_bodies(&dest, "f.txt"), vec!["keep me\n".to_string()]);
    assert!(trash_path(&d.join("does-not-exist")).is_ok());
}

// ── watcher helpers ──────────────────────────────────────────────────

#[test]
fn classify_git_paths_for_main_and_linked_worktree() {
    let d = tmp("classify");
    let repo = init(&d);
    write(&d, "a.txt", "a\n");
    commit_all(&repo, "c1");
    let dirs = repo_dirs(s(&d)).expect("dirs");
    let g = dirs.gitdir.to_string_lossy().trim_end_matches('/').to_string();
    assert_eq!(classify_git_path(&format!("{g}/index"), &dirs), Some(GitSignal::Local));
    assert_eq!(classify_git_path(&format!("{g}/index.lock"), &dirs), Some(GitSignal::Local));
    assert_eq!(classify_git_path(&format!("{g}/HEAD"), &dirs), Some(GitSignal::Local));
    assert_eq!(classify_git_path(&format!("{g}/MERGE_HEAD"), &dirs), Some(GitSignal::Local));
    assert_eq!(classify_git_path(&format!("{g}/refs/heads/main"), &dirs), Some(GitSignal::Shared));
    assert_eq!(classify_git_path(&format!("{g}/refs/heads/feat/x.lock"), &dirs), Some(GitSignal::Shared));
    assert_eq!(classify_git_path(&format!("{g}/packed-refs"), &dirs), Some(GitSignal::Shared));
    assert_eq!(classify_git_path(&format!("{g}/objects/ab/cdef"), &dirs), None);
    assert_eq!(classify_git_path(&format!("{g}/logs/HEAD"), &dirs), None);
    assert_eq!(classify_git_path(&format!("{g}/config"), &dirs), None);
    assert_eq!(classify_git_path(s(&d.join("a.txt")), &dirs), None);
    // Canonical spelling (macOS /private/var) also matches.
    let canon = std::fs::canonicalize(&dirs.gitdir).expect("canon");
    assert_eq!(classify_git_path(&format!("{}/index", canon.to_string_lossy().trim_end_matches('/')), &dirs), Some(GitSignal::Local));
    assert!(is_git_internal(&format!("{g}/index")));
    assert!(!is_git_internal(s(&d.join("a.txt"))));
    assert_eq!(worktree_rel(s(&d.join("src/x.rs")), &dirs), Some("src/x.rs"));
    assert_eq!(worktree_rel("/elsewhere/x.rs", &dirs), None);

    // Linked worktree: its own index/HEAD under .git/worktrees/<name>,
    // shared refs under the main .git.
    let wt_path = tmp("classify-wt").join("checkout");
    repo.worktree("wt1", &wt_path, None).expect("add worktree");
    let wd = repo_dirs(s(&wt_path)).expect("wt dirs");
    assert_ne!(wd.gitdir, wd.commondir);
    let wg = wd.gitdir.to_string_lossy().trim_end_matches('/').to_string();
    let cg = wd.commondir.to_string_lossy().trim_end_matches('/').to_string();
    assert_eq!(classify_git_path(&format!("{wg}/index"), &wd), Some(GitSignal::Local));
    assert_eq!(classify_git_path(&format!("{wg}/HEAD"), &wd), Some(GitSignal::Local));
    assert_eq!(classify_git_path(&format!("{cg}/refs/heads/wt1"), &wd), Some(GitSignal::Shared));
    // The main checkout's index is not this worktree's.
    assert_eq!(classify_git_path(&format!("{cg}/index"), &wd), None);
    // And snapshot works from the linked checkout.
    write(&wt_path, "wt_new.txt", "w\n");
    let sn = snap(&wt_path, None);
    assert_eq!(sn.branch.as_deref(), Some("wt1"));
    assert_eq!(row(&sn, "untracked", "wt_new.txt").status, "U");
}

/// C5: `init_repo` makes an unborn repo (no commit, `main`); stage, unstage
/// and the first commit work with no HEAD; init never re-initializes.
#[test]
fn init_then_first_commit_and_unstage_without_head() {
    let d = tmp("init-first");
    write(&d, "readme.md", "hi\n");
    write(&d, "notes.txt", "n\n");
    crate::git::init_repo(s(&d)).expect("init");
    let repo = Repository::open(&d).expect("open");
    assert!(repo.head().is_err(), "no commits yet");
    assert!(crate::git::init_repo(s(&d)).is_err(), "no re-init");
    let mut cfg = repo.config().expect("config");
    cfg.set_str("user.name", "K2 Test").expect("user.name");
    cfg.set_str("user.email", "k2-test@example.com").expect("user.email");
    let sn = snap(&d, None);
    assert!(sn.unborn, "unborn snapshot");
    let want = git2::Config::open_default()
        .ok()
        .and_then(|c| c.get_string("init.defaultBranch").ok())
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "main".to_string());
    assert_eq!(sn.branch.as_deref(), Some(want.as_str()));

    crate::git::stage_file(s(&d), "readme.md").expect("stage");
    crate::git::stage_file(s(&d), "notes.txt").expect("stage");
    crate::git::unstage_file(s(&d), "notes.txt").expect("unstage with no HEAD");
    let sn = snap(&d, None);
    assert_eq!(row(&sn, "staged", "readme.md").status, "A");
    assert_eq!(row(&sn, "untracked", "notes.txt").status, "U");

    let c = crate::git::commit(s(&d), "first").expect("first commit");
    let head = repo.head().expect("head now").peel_to_commit().expect("commit");
    assert_eq!(head.id().to_string(), c.oid);
    assert_eq!(head.parent_count(), 0);
    assert_eq!(repo.head().expect("head").shorthand(), Some(want.as_str()));
    let sn = snap(&d, None);
    assert!(!sn.unborn);
    assert_eq!(sn.counts.staged, 0);
}

/// C5 gap 3: the untracked folder row (`x/`) stages like `git add x/`:
/// every file under it, recursively, minus what .gitignore excludes, and
/// nothing outside it (a sibling `x-other` or `x*` is left alone).
#[test]
fn stage_file_adds_an_untracked_folder_recursively_respecting_gitignore() {
    let d = tmp("stage-dir");
    let repo = init(&d);
    write(&d, "a.txt", "a\n");
    write(&d, ".gitignore", "*.log\n");
    commit_all(&repo, "init");
    write(&d, "x/one.txt", "1\n");
    write(&d, "x/deep/two.txt", "2\n");
    write(&d, "x/deep/noise.log", "ignored\n");
    write(&d, "x-other/three.txt", "3\n");
    write(&d, "x*/star.txt", "*\n");

    let sn = snap(&d, None);
    assert!(row(&sn, "untracked", "x/").dir, "collapsed folder row");

    for spelling in ["x/", "x"] {
        crate::git::stage_file(s(&d), spelling).expect("stage folder");
    }
    let sn = snap(&d, None);
    let staged: Vec<&str> = sn.files.iter().filter(|f| f.group == "staged").map(|f| f.path.as_str()).collect();
    assert_eq!(staged, vec!["x/deep/two.txt", "x/one.txt"], "{:?}", sn.files);
    assert!(sn.files.iter().any(|f| f.group == "untracked" && f.path == "x-other/"));
    assert!(sn.files.iter().any(|f| f.group == "untracked" && f.path == "x*/"));
    let mut idx = repo.index().expect("index");
    idx.read(true).expect("reload index");
    assert!(idx.get_path(Path::new("x/one.txt"), 0).is_some());
    assert!(idx.get_path(Path::new("x/deep/noise.log"), 0).is_none(), "ignored file stays out");

    // A plain file still stages as before.
    write(&d, "a.txt", "a2\n");
    crate::git::stage_file(s(&d), "a.txt").expect("stage file");
    assert_eq!(row(&snap(&d, None), "staged", "a.txt").status, "M");
}
