//! The gix door's guards (research §5, §7.1, §7.2): no git program, no
//! filter or diff driver, and one door only.

use super::*;
use crate::git_fixture::{self as fx, write};
use std::path::Path;

/// Push a file's mtime forward so a status must re-read its content.
fn bump_mtime(p: &Path, secs: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(p).expect("open for mtime");
    let t = std::time::SystemTime::now() + std::time::Duration::from_secs(secs);
    f.set_modified(t).expect("set mtime");
}

/// Every read K2 does, once, on `d` (a repo with tracked, modified,
/// staged and untracked files).
fn exercise_every_read(d: &Path) {
    let repo = discover(d).expect("discover");
    let _ = open(d).expect("open");
    let _ = head_info(&repo);
    let _ = head_shorthand(&repo);
    let _ = state_name(&repo);
    let _ = status::statuses(&repo, status::Options::changes()).expect("status");
    let _ = status::statuses(&repo, status::Options { untracked: status::Untracked::Files, staged_renames: false, submodules: false })
        .expect("status files");
    let tree = head_tree_id(&repo);
    let changes = diff::tree_to_worktree(&repo, tree, true).expect("tree→worktree");
    let mut reader = diff::WorktreeReader::new(&repo).expect("reader");
    for c in &changes {
        let _ = diff::change_stats(&repo, &mut reader, c, 1 << 20);
    }
    let _ = reader.bytes("a.txt", None).expect("worktree bytes");
    let _ = diff::tree_to_tree(&repo, None, tree).expect("tree→tree");
    let mut ex = Excludes::new(discover(d).expect("discover")).expect("excludes");
    let _ = ex.is_ignored("a.txt", false);
    let _ = upstream_of(&repo, "main");
    let _ = resolve_commit(&repo, "HEAD");
}

fn armed_repo(tag: &str) -> std::path::PathBuf {
    let d = fx::init(tag);
    write(&d, "a.txt", "one\ntwo\n");
    write(&d, "b.txt", "bee\n");
    fx::commit_all(&d, "c1");
    write(&d, "b.txt", "bee\nsea\n");
    fx::stage(&d, "b.txt");
    write(&d, "b.txt", "bee\nsea\ndee\n");
    write(&d, "new.txt", "untracked\n");
    d
}

/// §7.1 tripwire: a fake `git` first on PATH records any run. Opening,
/// discovering, status, diffs and the work-tree reader never run it (on a
/// Mac without the Command Line Tools that `git` is Apple's shim, which
/// opens the install dialog).
///
/// The reads run in a child test process with the fake first on PATH, so
/// no other test in this process ever sees the fake.
#[cfg(unix)]
#[test]
fn the_door_never_runs_a_git_program() {
    let d = armed_repo("tripwire-bin");
    let fake = fx::tmp("fake-git-bin");
    let mark = fake.join("git-ran");
    crate::test_env::write_executable(&fake.join("git"), &format!("#!/bin/sh\n: > '{}'\nexit 1\n", mark.display()));
    let path = crate::test_env::path_list_prepend(&fake, std::env::var_os("PATH").unwrap_or_default());
    let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "gitx::tests::tripwire_child_reads_with_a_fake_git", "--ignored", "--nocapture", "--test-threads=1"])
        .env("PATH", &path)
        .env(TRIPWIRE_REPO_ENV, &d)
        .env(TRIPWIRE_FAKE_ENV, &fake)
        .output()
        .expect("run the child test");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "child test failed:\n{said}");
    assert!(said.contains("1 passed"), "the child test ran:\n{said}");
    assert!(!mark.exists(), "a gix read ran the `git` on PATH");
}

#[cfg(unix)]
const TRIPWIRE_REPO_ENV: &str = "K2_GITX_TRIPWIRE_REPO";
#[cfg(unix)]
const TRIPWIRE_FAKE_ENV: &str = "K2_GITX_TRIPWIRE_FAKE";

/// The child half of [`the_door_never_runs_a_git_program`].
#[cfg(unix)]
#[test]
#[ignore = "run by the_door_never_runs_a_git_program in its own process"]
fn tripwire_child_reads_with_a_fake_git() {
    let repo = std::env::var_os(TRIPWIRE_REPO_ENV).expect("run by the parent test");
    let fake = std::path::PathBuf::from(std::env::var_os(TRIPWIRE_FAKE_ENV).expect("run by the parent test"));
    let found = std::env::split_paths(&std::env::var_os("PATH").expect("PATH")).next().expect("first PATH dir");
    assert_eq!(found, fake, "the fake is the first git on PATH");
    exercise_every_read(Path::new(&repo));
}

/// §7.2 tripwire: a repo-local filter driver and textconv never run
/// through K2's reads (status, diffs, the work-tree reader). Plain git
/// runs them on the same repo, so the fixture is armed.
#[cfg(unix)]
#[test]
fn repo_filter_and_diff_drivers_never_run() {
    let d = fx::init("tripwire-filter");
    write(&d, ".gitattributes", "*.txt filter=evil diff=evil\n");
    write(&d, "a.txt", "one\ntwo\n");
    write(&d, "b.txt", "bee\n");
    fx::commit_all(&d, "c1");
    let marks = fx::tmp("tripwire-filter-marks");
    let prog = |name: &str| {
        let p = marks.join(format!("{name}.sh"));
        crate::test_env::write_executable(&p, &format!("#!/bin/sh\n: > '{}'\ncat\n", marks.join(format!("{name}.ran")).display()));
        p.display().to_string()
    };
    let (clean, smudge, textconv) = (prog("clean"), prog("smudge"), prog("textconv"));
    fx::git(&d, &["config", "filter.evil.clean", &clean]);
    fx::git(&d, &["config", "filter.evil.smudge", &smudge]);
    fx::git(&d, &["config", "filter.evil.process", &clean]);
    fx::git(&d, &["config", "diff.evil.textconv", &textconv]);
    let ran = || -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&marks)
            .expect("marks")
            .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".ran"))
            .collect();
        v.sort();
        v
    };

    // Armed: plain git runs the clean filter when it must re-read a file.
    bump_mtime(&d.join("a.txt"), 5);
    fx::git(&d, &["status", "--porcelain"]);
    assert!(!ran().is_empty(), "fixture: plain git must run the repo's filter");
    for n in ran() {
        std::fs::remove_file(marks.join(n)).expect("clear mark");
    }

    // K2's reads: the door strips the drivers, and nothing runs.
    let repo = discover(&d).expect("discover");
    assert!(driver_sections(&repo).is_empty(), "drivers left: {:?}", driver_sections(&repo));
    bump_mtime(&d.join("a.txt"), 10);
    write(&d, "b.txt", "bee\nmore\n");
    exercise_every_read(&d);
    assert!(ran().is_empty(), "repo-named programs ran through gix: {:?}", ran());
}

#[test]
fn not_a_repo_is_refused_with_its_kind() {
    let d = fx::tmp("door-norepo");
    assert!(matches!(discover(&d), Err(OpenError::NotARepo(_))));
    assert!(matches!(open(&d), Err(OpenError::NotARepo(_))));
}

#[test]
fn open_needs_the_root_and_discover_searches_up() {
    let d = fx::init("door-sub");
    write(&d, "sub/x.txt", "x\n");
    fx::commit_all(&d, "c1");
    let sub = d.join("sub");
    let r = discover(&sub).expect("discover from a sub-folder");
    assert_eq!(
        dunce::canonicalize(r.workdir().expect("workdir")).expect("canon"),
        dunce::canonicalize(&d).expect("canon")
    );
    assert!(open(&sub).is_err(), "open is exact, like git2's Repository::open");
    assert!(open(&d).is_ok());
}

#[test]
fn head_info_branch_detached_unborn() {
    let d = fx::init("door-head");
    let h = head_info(&discover(&d).expect("repo"));
    assert_eq!(h, HeadInfo { branch: Some("main".into()), oid: None, detached: false, unborn: true });
    assert_eq!(head_shorthand(&discover(&d).expect("repo")), None);
    write(&d, "a.txt", "a\n");
    let c1 = fx::commit_all(&d, "c1");
    let repo = discover(&d).expect("repo");
    let h = head_info(&repo);
    assert_eq!((h.branch.as_deref(), h.oid.map(|o| o.to_string()), h.detached, h.unborn), (Some("main"), Some(c1.clone()), false, false));
    fx::git(&d, &["checkout", "-q", "--detach"]);
    let repo = discover(&d).expect("repo");
    let h = head_info(&repo);
    assert!(h.detached && h.branch.is_none());
    assert_eq!(head_shorthand(&repo).as_deref(), Some("HEAD"));
}

#[test]
fn ahead_behind_and_upstream_match_git() {
    let d = fx::init("door-ab");
    write(&d, "a.txt", "1\n");
    fx::commit_all(&d, "c1");
    fx::git(&d, &["branch", "up"]);
    write(&d, "a.txt", "2\n");
    fx::commit_all(&d, "c2");
    write(&d, "a.txt", "3\n");
    fx::commit_all(&d, "c3");
    fx::git(&d, &["branch", "--set-upstream-to=up"]);
    let repo = discover(&d).expect("repo");
    let (up, tip) = upstream_of(&repo, "main").expect("upstream");
    assert_eq!(up, "up");
    assert_eq!(tip.to_string(), fx::git(&d, &["rev-parse", "up"]));
    let head = head_info(&repo).oid.expect("head");
    let counts = fx::git(&d, &["rev-list", "--left-right", "--count", "main...up"]);
    let want: Vec<usize> = counts.split_whitespace().map(|n| n.parse().expect("count")).collect();
    assert_eq!(ahead_behind(&repo, head, tip), Some((want[0], want[1])));
    assert_eq!(merge_base(&repo, head, tip).map(|o| o.to_string()), Some(fx::git(&d, &["merge-base", "main", "up"])));
    assert_eq!(upstream_of(&repo, "up"), None, "no upstream configured");
}

#[test]
fn branch_names_validate_like_git() {
    for ok in ["main", "feat/x", "a.b", "v1.2"] {
        assert!(valid_branch_name(ok), "{ok}");
    }
    for bad in ["a..b", "a b", "x.lock", "a~1", "a^", "a:b", "-x/..", "a/", "@{x", "a\\b", ""] {
        assert!(!valid_branch_name(bad), "{bad}");
    }
}

#[test]
fn blob_ids_match_git_hash_object() {
    let d = fx::init("door-hash");
    write(&d, "x.bin", [0u8, 1, 2, 3, 255]);
    write(&d, "t.txt", "line\r\nline2\n");
    for f in ["x.bin", "t.txt"] {
        let want = fx::git(&d, &["hash-object", "--no-filters", f]);
        let bytes = std::fs::read(d.join(f)).expect("read");
        assert_eq!(hash_blob(&bytes).expect("hash").to_string(), want, "{f}");
        assert_eq!(hash_file(&d.join(f)).expect("hash file").to_string(), want, "{f} streamed");
    }
}

/// One door: no `gix::open`/`gix::discover`/`ThreadSafeRepository` outside
/// `crate::gitx` in k2-core or k2-daemon production code.
#[test]
fn no_gix_open_outside_the_door() {
    use crate::git_safe::tests::{is_test_file, non_test_lines, rs_files};
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for root in ["k2-core/src", "k2-daemon/src"] {
        rs_files(&crates.join(root), &mut files);
    }
    assert!(files.len() > 100, "the scan found the sources ({} files)", files.len());
    let banned = [
        concat!("gix::", "open("),
        concat!("gix::", "open_opts("),
        concat!("gix::", "discover("),
        concat!("gix::", "discover_opts("),
        concat!("ThreadSafe", "Repository::"),
        concat!("gix::", "init("),
    ];
    let mut bad = Vec::new();
    for f in &files {
        let in_door = f.components().any(|c| c.as_os_str() == "gitx");
        if is_test_file(f) || in_door {
            continue;
        }
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        for (n, line) in non_test_lines(f, &src) {
            if banned.iter().any(|b| line.contains(b)) {
                bad.push(format!("{}:{n}: {}", f.display(), line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "gix opened outside crate::gitx (use gitx::discover / gitx::open):\n{}", bad.join("\n"));
}
