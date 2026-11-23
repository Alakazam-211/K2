//! The oracle suite (research §6.2): K2's gix answers compared with the git
//! CLI's own on CLI-made fixtures. A difference fails loudly, naming the
//! fixture and the path.

use super::diff::{self, ChangeKind, FileChange, LineKind};
use super::status::{self, Flags};
use crate::git_fixture::{self as fx, write};
use std::collections::BTreeMap;
use std::path::Path;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BIG: u64 = 64 * 1024 * 1024;

type Stats = BTreeMap<String, (Option<u32>, Option<u32>, Option<String>)>;

/// `git diff --numstat -z <args>`: path → (adds, dels, old path); binary =
/// (None, None).
fn git_numstat(d: &Path, args: &[&str]) -> Stats {
    let mut a = vec!["diff", "--numstat", "-z", "--no-ext-diff", "--no-textconv"];
    a.extend_from_slice(args);
    let raw = fx::git_raw(d, &a);
    let toks: Vec<String> = raw.split(|b| *b == 0).map(|t| String::from_utf8(t.to_vec()).expect("utf8 numstat")).collect();
    let mut out = Stats::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.is_empty() {
            i += 1;
            continue;
        }
        let mut parts = t.splitn(3, '\t');
        let n = |s: Option<&str>| -> Option<u32> { s.and_then(|v| v.trim().parse().ok()) };
        let (adds, dels) = (n(parts.next()), n(parts.next()));
        let path = parts.next().expect("numstat path field").to_string();
        if path.is_empty() {
            let (old, new) = (toks[i + 1].clone(), toks[i + 2].clone());
            out.insert(new, (adds, dels, Some(old)));
            i += 3;
        } else {
            out.insert(path, (adds, dels, None));
            i += 1;
        }
    }
    out
}

fn k2_stats(repo: &gix::Repository, changes: &[FileChange]) -> Stats {
    let mut reader = diff::WorktreeReader::new(repo).expect("reader");
    let mut out = Stats::new();
    for c in changes {
        let (a, d, _bin) = diff::change_stats(repo, &mut reader, c, BIG);
        out.insert(c.path.clone(), (a, d, c.old_path.clone()));
    }
    out
}

fn assert_same(fixture: &str, what: &str, git: &Stats, k2: &Stats) {
    let mut diffs = Vec::new();
    for (p, g) in git {
        match k2.get(p) {
            Some(k) if k == g => {}
            Some(k) => diffs.push(format!("  {p}: git {g:?} k2 {k:?}")),
            None => diffs.push(format!("  {p}: git {g:?} k2 (missing)")),
        }
    }
    for (p, k) in k2 {
        if !git.contains_key(p) {
            diffs.push(format!("  {p}: git (missing) k2 {k:?}"));
        }
    }
    assert!(diffs.is_empty(), "oracle [{fixture}] {what} differs from git:\n{}", diffs.join("\n"));
}

/// A repo with most of what git diffs can hold: CRLF, an eol=crlf
/// attribute, a staged rename with an edit, a binary, a unicode name, a
/// deletion, a multi-hunk edit, a staged-then-edited file, a new staged
/// file, and on Unix an exec-bit flip and a retargeted symlink.
fn rich(tag: &str) -> std::path::PathBuf {
    let d = fx::init(tag);
    fx::git(&d, &["config", "core.autocrlf", "false"]);
    let lines = |n: usize, tag: &str| -> String { (1..=n).map(|i| format!("{tag} line {i}\n")).collect() };
    write(&d, ".gitattributes", "eol.txt text eol=crlf\n");
    write(&d, "crlf.txt", "a\r\nb\r\nc\r\nd\r\n");
    write(&d, "eol.txt", "one\ntwo\nthree\n");
    write(&d, "keep.txt", lines(40, "keep"));
    write(&d, "gone.txt", "bye\nnow\n");
    write(&d, "rename_src.txt", lines(12, "moving"));
    write(&d, "bin.dat", b"\0\x01\x02binary\0".to_vec());
    write(&d, "unicodé 名前.txt", "héllo\nwörld\n");
    write(&d, "big.txt", lines(2000, "big"));
    write(&d, "no_newline.txt", "first\nlast without newline");
    #[cfg(unix)]
    {
        write(&d, "exec.sh", "#!/bin/sh\necho hi\n");
        std::os::unix::fs::symlink("keep.txt", d.join("link")).expect("symlink");
    }
    fx::commit_all(&d, "base");
    // Re-checkout eol.txt so the work tree holds CRLF as git writes it.
    std::fs::remove_file(d.join("eol.txt")).expect("rm eol");
    fx::git(&d, &["checkout", "--", "eol.txt"]);

    write(&d, "crlf.txt", "a\r\nB\r\nc\r\nd\r\ne\r\n");
    write(&d, "eol.txt", "one\r\nTWO\r\nthree\r\nfour\r\n");
    let mut keep: Vec<String> = lines(40, "keep").lines().map(str::to_string).collect();
    keep[2] = "keep CHANGED 3".into();
    keep.insert(30, "keep inserted".into());
    keep.remove(10);
    write(&d, "keep.txt", keep.join("\n") + "\n");
    std::fs::remove_file(d.join("gone.txt")).expect("rm gone");
    fx::git(&d, &["mv", "rename_src.txt", "renamed.txt"]);
    let mut moved = lines(12, "moving");
    moved.push_str("moving tail\n");
    write(&d, "renamed.txt", &moved);
    fx::stage(&d, "renamed.txt");
    write(&d, "bin.dat", b"\0\x01\x03binary changed\0".to_vec());
    write(&d, "unicodé 名前.txt", "héllo\nwörld\n!\n");
    let mut big: Vec<String> = lines(2000, "big").lines().map(str::to_string).collect();
    big[100] = "big staged".into();
    write(&d, "big.txt", big.join("\n") + "\n");
    fx::stage(&d, "big.txt");
    big[1500] = "big unstaged".into();
    write(&d, "big.txt", big.join("\n") + "\n");
    write(&d, "no_newline.txt", "first\nlast changed without newline");
    write(&d, "new_staged.txt", "fresh\nfile\n");
    fx::stage(&d, "new_staged.txt");
    write(&d, "untracked.txt", "never in a diff\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.join("exec.sh"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::remove_file(d.join("link")).expect("rm link");
        std::os::unix::fs::symlink("gone.txt", d.join("link")).expect("symlink");
    }
    d
}

#[test]
fn oracle_numstat_matches_git_on_a_rich_repo() {
    let d = rich("oracle-rich");
    let repo = super::discover(&d).expect("repo");
    let head = super::head_tree_id(&repo);

    let k = k2_stats(&repo, &diff::tree_to_worktree(&repo, head, false).expect("t2w"));
    assert_same("rich", "HEAD→worktree (no renames)", &git_numstat(&d, &["--no-renames", "HEAD"]), &k);

    let k = k2_stats(&repo, &diff::tree_to_worktree(&repo, head, true).expect("t2w renames"));
    assert_same("rich", "HEAD→worktree (renames)", &git_numstat(&d, &["-M", "HEAD"]), &k);

    let k = k2_stats(&repo, &diff::tree_to_index(&repo, head, true).expect("t2i"));
    assert_same("rich", "staged (renames)", &git_numstat(&d, &["--cached", "-M"]), &k);

    let k = k2_stats(&repo, &diff::tree_to_index(&repo, head, false).expect("t2i"));
    assert_same("rich", "staged (no renames)", &git_numstat(&d, &["--cached", "--no-renames"]), &k);

    let k = k2_stats(&repo, &diff::index_to_worktree(&repo).expect("i2w"));
    assert_same("rich", "unstaged", &git_numstat(&d, &["--no-renames"]), &k);

    let first = fx::git(&d, &["rev-list", "--max-parents=0", "HEAD"]);
    fx::commit_all(&d, "second");
    let repo = super::discover(&d).expect("repo");
    let (a, b) = (super::commit_tree_id(&repo, first.parse().expect("oid")), super::head_tree_id(&repo));
    let k = k2_stats(&repo, &diff::tree_to_tree(&repo, a, b).expect("t2t"));
    assert_same("rich", "tree→tree", &git_numstat(&d, &["--no-renames", &first, "HEAD"]), &k);
}

#[test]
fn oracle_status_matches_git_porcelain() {
    let d = rich("oracle-status");
    write(&d, "newdir/a.txt", "a\n");
    write(&d, "newdir/deep/b.txt", "b\n");
    write(&d, ".gitignore", "*.log\n");
    write(&d, "noise.log", "ignored\n");
    let repo = super::discover(&d).expect("repo");
    for (mode, flag) in [(status::Untracked::Collapsed, "-unormal"), (status::Untracked::Files, "-uall")] {
        let rows = status::statuses(&repo, status::Options { untracked: mode, staged_renames: true, submodules: true }).expect("status");
        let mut k2: BTreeMap<String, String> = BTreeMap::new();
        for r in &rows {
            let x = if r.flags.contains(Flags::INDEX_NEW) {
                'A'
            } else if r.flags.contains(Flags::INDEX_DELETED) {
                'D'
            } else if r.flags.contains(Flags::INDEX_RENAMED) {
                'R'
            } else if r.flags.contains(Flags::INDEX_TYPECHANGE) {
                'T'
            } else if r.flags.contains(Flags::INDEX_MODIFIED) {
                'M'
            } else {
                '.'
            };
            let y = if r.flags.contains(Flags::WT_DELETED) {
                'D'
            } else if r.flags.contains(Flags::WT_TYPECHANGE) {
                'T'
            } else if r.flags.contains(Flags::WT_MODIFIED) {
                'M'
            } else {
                '.'
            };
            let v = if r.flags.contains(Flags::WT_NEW) { "??".to_string() } else { format!("{x}{y}") };
            k2.insert(r.path.clone(), v);
        }
        // git status --porcelain=v2 -z: "1 XY …path", "2 XY … path\0orig", "? path".
        let raw = fx::git_raw(&d, &["status", "--porcelain=v2", "-z", flag, "--ignored=no"]);
        let toks: Vec<String> = raw.split(|b| *b == 0).map(|t| String::from_utf8(t.to_vec()).expect("utf8")).collect();
        let mut git: BTreeMap<String, String> = BTreeMap::new();
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            if let Some(p) = t.strip_prefix("? ") {
                git.insert(p.to_string(), "??".into());
            } else if t.starts_with("1 ") {
                let f: Vec<&str> = t.splitn(9, ' ').collect();
                git.insert(f[8].to_string(), f[1].to_string());
            } else if t.starts_with("2 ") {
                let f: Vec<&str> = t.splitn(10, ' ').collect();
                git.insert(f[9].to_string(), f[1].to_string());
                i += 1; // the original path
            }
            i += 1;
        }
        assert_eq!(git, k2, "oracle [rich] status {flag} differs from git");
    }
}

/// `git diff -U3` hunks of one file (index → work tree).
fn git_hunks(d: &Path, file: &str) -> Vec<(u32, u32, u32, u32, Vec<(LineKind, Vec<u8>)>)> {
    let raw = fx::git_raw(d, &["diff", "-U3", "--no-ext-diff", "--no-textconv", "--no-color", "--", file]);
    let mut out: Vec<(u32, u32, u32, u32, Vec<(LineKind, Vec<u8>)>)> = Vec::new();
    let lines: Vec<&[u8]> = raw.split_inclusive(|b| *b == b'\n').collect();
    let num = |s: &str| -> (u32, u32) {
        let mut it = s.splitn(2, ',');
        let a: u32 = it.next().expect("start").parse().expect("num");
        let b: u32 = it.next().map(|v| v.parse().expect("count")).unwrap_or(1);
        (a, b)
    };
    let mut in_body = false;
    for l in lines {
        if l.starts_with(b"@@ ") {
            let s = String::from_utf8_lossy(l);
            let f: Vec<&str> = s.split(' ').collect();
            let (os, oc) = num(f[1].trim_start_matches('-'));
            let (ns, nc) = num(f[2].trim_start_matches('+'));
            out.push((os, oc, ns, nc, Vec::new()));
            in_body = true;
            continue;
        }
        if !in_body {
            continue;
        }
        let body = &mut out.last_mut().expect("hunk").4;
        match l.first() {
            Some(b' ') => body.push((LineKind::Context, l[1..].to_vec())),
            Some(b'+') => body.push((LineKind::Add, l[1..].to_vec())),
            Some(b'-') => body.push((LineKind::Remove, l[1..].to_vec())),
            Some(b'\\') => {
                let last = body.last_mut().expect("line before no-newline marker");
                if last.1.last() == Some(&b'\n') {
                    last.1.pop();
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn oracle_hunks_match_git_diff_u3() {
    let d = rich("oracle-hunks");
    let repo = super::discover(&d).expect("repo");
    let index = repo.index_or_empty().expect("index");
    let mut reader = diff::WorktreeReader::new(&repo).expect("reader");
    for f in ["keep.txt", "crlf.txt", "eol.txt", "big.txt", "no_newline.txt", "unicodé 名前.txt"] {
        let (_, id) = super::index_entry(&index, f).unwrap_or_else(|| panic!("{f} in index"));
        let old = super::blob_bytes(&repo, id).expect("blob");
        let new = reader.bytes(f, None).expect("worktree bytes");
        let k2: Vec<_> = diff::hunks(&old, &new, 3)
            .into_iter()
            .map(|h| (h.old_start, h.old_count, h.new_start, h.new_count, h.lines.into_iter().map(|l| (l.kind, l.content)).collect::<Vec<_>>()))
            .collect();
        let git = git_hunks(&d, f);
        assert!(!git.is_empty(), "fixture: {f} has a diff");
        assert_eq!(git, k2, "oracle [rich] hunks of {f} differ from git diff -U3");
    }
}

#[test]
fn oracle_unborn_and_empty_repos() {
    let d = fx::init("oracle-unborn");
    let repo = super::discover(&d).expect("repo");
    assert!(diff::tree_to_index(&repo, None, true).expect("t2i").is_empty());
    assert!(status::statuses(&repo, status::Options::changes()).expect("status").is_empty());
    write(&d, "a.txt", "1\n2\n");
    write(&d, "b/c.txt", "c\n");
    fx::stage(&d, "a.txt");
    let repo = super::discover(&d).expect("repo");
    let k = k2_stats(&repo, &diff::tree_to_index(&repo, None, true).expect("t2i"));
    assert_same("unborn", "staged", &git_numstat(&d, &["--cached", EMPTY_TREE]), &k);
    let rows = status::statuses(&repo, status::Options::changes()).expect("status");
    let got: Vec<(&str, bool)> = rows.iter().map(|r| (r.path.as_str(), r.flags.contains(Flags::WT_NEW))).collect();
    assert_eq!(got, vec![("a.txt", false), ("b/", true)]);
}

#[test]
fn stats_stop_at_the_size_cap_and_flag_binary() {
    let d = fx::init("oracle-cap");
    write(&d, "huge.txt", "x\n".repeat(10));
    write(&d, "bin.dat", b"\0\0\0".to_vec());
    fx::commit_all(&d, "c1");
    write(&d, "huge.txt", "y\n".repeat(600_000));
    write(&d, "bin.dat", b"\0\0\0\0".to_vec());
    let repo = super::discover(&d).expect("repo");
    let changes = diff::index_to_worktree(&repo).expect("i2w");
    let mut reader = diff::WorktreeReader::new(&repo).expect("reader");
    for c in &changes {
        let s = diff::change_stats(&repo, &mut reader, c, 1024 * 1024);
        match c.path.as_str() {
            "huge.txt" => assert_eq!(s, (None, None, false), "past the cap: no counts"),
            "bin.dat" => assert_eq!(s, (None, None, true), "binary"),
            other => panic!("unexpected change {other}"),
        }
        assert_eq!(c.kind, ChangeKind::Modified);
    }
    assert_eq!(changes.len(), 2);
}

#[test]
fn line_stats_and_hunks_basics() {
    assert_eq!(diff::line_stats(b"a\nb\nc\n", b"a\nB\nc\nd\n"), (2, 1));
    assert_eq!(diff::line_stats(b"", b"x\n"), (1, 0));
    assert_eq!(diff::line_stats(b"x", b""), (0, 1));
    let h = diff::hunks(b"", b"x\ny\n", 3);
    assert_eq!((h[0].old_start, h[0].old_count, h[0].new_start, h[0].new_count), (0, 0, 1, 2));
    assert!(diff::is_binary(b"a\0b") && !diff::is_binary(b"plain"));
}
