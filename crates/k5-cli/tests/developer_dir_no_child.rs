//! On a Mac, finding the developer tools folder starts no program: not
//! `/usr/bin/xcode-select` (about 3 ms a run), and not one named
//! `xcode-select` on `PATH`. The three places that ask are each called:
//! [`daemon::developer_dir`] (behind `usable_git` and the rename
//! heads-up), [`python::in_env`] with Apple's `python3` stand-in on
//! `PATH`, and the page prelude's git check ([`offline::prelude`]).
//!
//! The process's children are counted by SIGCHLD, so this file holds
//! this one test: another test's child would count. A child started on
//! purpose at the end shows the count works.

#![cfg(target_os = "macos")]

use k5_cli::offline::{self, Env};
use k5_cli::{daemon, python};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static CHILDREN: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_child(_: libc::c_int) {
    CHILDREN.fetch_add(1, Ordering::SeqCst);
}

fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("k5-devdir-{}", std::process::id()));
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn finding_the_developer_tools_starts_no_program() {
    // The stand-ins this is about: without them nothing asks.
    for f in ["/usr/bin/python3", "/usr/bin/git", "/usr/bin/xcode-select"] {
        assert!(Path::new(f).is_file(), "{f} is not on this Mac");
    }
    let d = scratch();
    // An `xcode-select` first on PATH that leaves a mark if run.
    let ran = d.join("ran");
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("xcode-select");
    std::fs::write(&stub, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", s(&ran))).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", s(&bin));
    // A developer folder with no git or Python in it, and a workspace.
    let empty = d.join("empty-tools");
    std::fs::create_dir_all(&empty).unwrap();
    let ws = d.join("ws");
    std::fs::create_dir_all(ws.join(".k2")).unwrap();

    // SAFETY: installs a handler that only adds to an atomic counter.
    let old = unsafe { libc::signal(libc::SIGCHLD, on_child as *const () as libc::sighandler_t) };
    assert_ne!(old, libc::SIG_ERR, "the SIGCHLD handler is set");

    // 1. The cached lookup, this process's DEVELOPER_DIR.
    let dev = daemon::developer_dir().map(Path::to_path_buf);
    // 2. Apple's python3 stand-in on PATH: the answer needs the folder.
    let has_python = dev
        .as_deref()
        .is_some_and(|d| daemon::is_executable(&d.join("usr/bin/python3")));
    let found = python::in_env(&Env::of(&[("PATH", &path)]));
    let want = if has_python {
        python::Python3::Present
    } else {
        python::Python3::Missing
    };
    assert_eq!(found, want, "python3 with the tools at {dev:?}");
    let none = python::in_env(&Env::of(&[("PATH", &path), ("DEVELOPER_DIR", s(&empty))]));
    assert_eq!(
        none,
        python::Python3::Missing,
        "a tools folder with no python3"
    );
    // 3. The page prelude in a workspace: its git check reads the folder,
    //    which has no git, so git never runs either.
    let env = Env::of(&[
        ("HOME", s(&d)),
        ("PATH", &path),
        ("DEVELOPER_DIR", s(&empty)),
    ]);
    assert!(offline::prelude(&env, &ws).is_ok(), "the page prelude");

    // Any child has been waited for; give its signal time to land.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(CHILDREN.load(Ordering::SeqCst), 0, "a program was started");
    assert!(!ran.exists(), "xcode-select on PATH ran");

    // The count works: one child on purpose is seen.
    let st = std::process::Command::new("/usr/bin/true")
        .status()
        .unwrap();
    assert!(st.success());
    let start = Instant::now();
    while CHILDREN.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(CHILDREN.load(Ordering::SeqCst), 1, "the child on purpose");
    std::fs::remove_dir_all(&d).unwrap();
}
