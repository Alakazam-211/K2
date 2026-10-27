//! Tests for `git_tools` (0.46.4 first run without git). Stub `git` and
//! `xcode-select` binaries stand in for Apple's; nothing here runs the
//! real `xcode-select --install`.

use super::*;
use crate::git_safe::ApplePair;
use std::time::{Duration, SystemTime};

/// A fresh temp dir, canonical (the shim compares `pwd -P` paths).
fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-git-tools-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    dunce::canonicalize(&d).expect("canonical temp")
}

/// Written by a `/bin/sh` child (`test_env::write_executable`), so no
/// forked test thread can hold a write fd when it's exec'd (ETXTBSY).
#[cfg(unix)]
fn write_exec(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir parent");
    crate::test_env::write_executable(path, body);
}

/// A stub git that leaves `mark` and answers like a real git (builtins
/// only, so it runs with any PATH).
#[cfg(unix)]
fn stub_git(dir: &Path, mark: &Path) {
    write_exec(&dir.join("git"), &format!("#!/bin/sh\n: > '{}'\necho \"stub git $*\"\n", mark.display()));
}

#[cfg(unix)]
fn stub_xcode_select(dir: &Path, dev: Option<&Path>) {
    let body = match dev {
        Some(d) => format!("#!/bin/sh\necho '{}'\n", d.display()),
        None => "#!/bin/sh\necho 'xcode-select: error: unable to get active developer directory' >&2\nexit 2\n".to_string(),
    };
    write_exec(&dir.join("xcode-select"), &body);
}

fn ready() -> GitProbe {
    GitProbe::Ready { program: "/usr/bin/git".into(), version: "git version 2.39.5".into() }
}

fn broken() -> GitProbe {
    GitProbe::Broken {
        program: "/usr/bin/git".into(),
        error: "You have not agreed to the Xcode license agreements. Please run 'sudo xcodebuild -license' from within a Terminal window to review and agree to the Xcode and Apple SDKs license. (exit 69)".into(),
    }
}

fn waiting(err: Option<&str>) -> WaitingWorkspace {
    WaitingWorkspace {
        project_id: "p1".into(),
        name: "site".into(),
        path: "/Users/me/site".into(),
        branch: "main".into(),
        note: WAITING_MARK_MAC.into(),
        last_error: err.map(str::to_string),
    }
}

// ── state ───────────────────────────────────────────────────────────────

#[test]
fn install_state_follows_the_click_and_the_installer() {
    let now = SystemTime::now();
    let ago = |s: u64| Some(now - Duration::from_secs(s));
    let clt = GitProbe::NeedsAppleClt;
    assert_eq!(install_state(&ready(), true, None, now, false), InstallState::Ready);
    assert_eq!(install_state(&ready(), false, None, now, false), InstallState::Ready);
    assert_eq!(install_state(&clt, true, None, now, false), InstallState::NotInstalled);
    // Just clicked: installing even before Apple's window is listed.
    assert_eq!(install_state(&clt, true, ago(5), now, false), InstallState::Installing);
    // Later, only while Apple's installer is up.
    assert_eq!(install_state(&clt, true, ago(600), now, true), InstallState::Installing);
    assert_eq!(install_state(&clt, true, ago(600), now, false), InstallState::NotInstalled);
    // Never for more than three hours.
    assert_eq!(install_state(&clt, true, ago(4 * 3600), now, true), InstallState::NotInstalled);
    // Linux has no installer K2 starts.
    assert_eq!(install_state(&GitProbe::Missing, false, ago(5), now, true), InstallState::NotInstalled);
    // A git that is there but fails is its own state, click or not.
    assert_eq!(install_state(&broken(), true, ago(5), now, true), InstallState::GitBroken);
}

#[test]
fn broken_git_gets_its_own_state_words_and_fix() {
    let s = status_from(&broken(), true, None, false, "", vec![]);
    assert_eq!(s.state, InstallState::GitBroken);
    assert_eq!(s.needs, Some("fix_git"));
    assert!(!s.can_install, "the installer can't fix a licence");
    assert!(s.git_error.as_deref().is_some_and(|e| e.ends_with("(exit 69)")), "{:?}", s.git_error);
    assert_eq!(s.how_to, vec!["Accept Apple's licence: run `sudo xcodebuild -license` in Terminal (or open Xcode once)".to_string()]);
    let v = serde_json::to_value(&s).expect("json");
    assert_eq!(v["state"], "git_broken");
    // Linux: reinstall with the distro's package manager.
    let l = status_from(
        &GitProbe::Broken { program: "/usr/bin/git".into(), error: "error while loading shared libraries (exit 127)".into() },
        false,
        None,
        false,
        "ID=ubuntu\n",
        vec![],
    );
    assert_eq!(l.how_to, vec!["Reinstall git: sudo apt install git".to_string()]);
}

#[test]
fn linux_install_line_matches_the_distro() {
    assert_eq!(linux_install_command("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n"), "sudo apt install git");
    assert_eq!(linux_install_command("ID=debian\n"), "sudo apt install git");
    assert_eq!(linux_install_command("ID=\"fedora\"\n"), "sudo dnf install git");
    assert_eq!(linux_install_command("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"), "sudo dnf install git");
    assert_eq!(linux_install_command("ID=arch\n"), "sudo pacman -S git");
    assert_eq!(linux_install_command("ID=endeavouros\nID_LIKE=arch\n"), "sudo pacman -S git");
    assert_eq!(linux_install_command("ID=alpine\n"), "sudo apk add git");
    assert_eq!(linux_install_command(""), "install git with your package manager (apt, dnf or pacman)");
}

#[test]
fn headless_mac_steps_need_admin_and_name_softwareupdate() {
    let steps = apple_headless_steps().join("\n");
    assert!(steps.contains("softwareupdate --list"), "{steps}");
    assert!(steps.contains("sudo softwareupdate -i \"Command Line Tools for Xcode-"), "{steps}");
    assert!(steps.contains("installondemand.in-progress"), "{steps}");
}

// ── the CLI's text ──────────────────────────────────────────────────────

#[test]
fn text_for_a_mac_without_the_tools_names_the_install_and_the_ssh_steps() {
    let s = status_from(&GitProbe::NeedsAppleClt, true, None, false, "", vec![waiting(Some("Failed to initialize git: boom"))]);
    let t = render_text(&s, None, &[]);
    let want = [
        "git        : not installed (needs Apple's command line tools)",
        "waiting    : site (/Users/me/site) git: waiting for Apple's command line tools",
        "  last try : Failed to initialize git: boom",
        "install    : k2 git-tools install   (opens Apple's installer on this Mac)",
        "over SSH   : run these yourself (admin password needed):",
        "  touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress",
        "  softwareupdate --list   # note the \"Command Line Tools for Xcode-NN.N\" label",
        "  sudo softwareupdate -i \"Command Line Tools for Xcode-NN.N\" --verbose",
        "  rm /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress",
    ]
    .join("\n")
        + "\n";
    assert_eq!(t, want);
}

#[test]
fn text_after_the_click_says_what_apples_dialog_wants() {
    let s = status_from(&GitProbe::NeedsAppleClt, true, Some(SystemTime::now()), false, "", vec![]);
    assert_eq!(s.state, InstallState::Installing);
    let t = render_text(&s, Some(InstallStart::Started), &[]);
    assert_eq!(
        t,
        "Apple's installer is open on this Mac's screen (it can be behind the K2 window).\n\
         Click Install, not Get Xcode (that opens the App Store for the full Xcode),\n\
         agree to Apple's licence, then wait for the download (about 5-15 min).\n\
         git        : installing Apple's command line tools\n"
    );
}

#[test]
fn text_for_ready_linux_and_broken() {
    let r = status_from(&ready(), true, None, false, "", vec![]);
    assert_eq!(
        render_text(&r, None, &["/Users/me/site".to_string()]),
        "git        : ready (git version 2.39.5, /usr/bin/git)\nfinished   : /Users/me/site\n"
    );
    assert_eq!(render_text(&r, Some(InstallStart::AlreadyReady), &[]).lines().next(), Some("git already works here; nothing to install."));
    let l = status_from(&GitProbe::Missing, false, None, false, "ID=fedora\n", vec![]);
    // The install line is this OS's own (os-release only matters on Linux).
    let install = if cfg!(windows) { "Install Git for Windows from git-scm.com" } else { "sudo dnf install git" };
    assert_eq!(render_text(&l, None, &[]), format!("git        : not installed (needs git)\ninstall    : run this yourself:\n  {install}\n"));
    let b = status_from(&broken(), true, None, false, "", vec![]);
    let t = render_text(&b, None, &[]);
    assert!(t.starts_with("git        : found but failing (/usr/bin/git): You have not agreed"), "{t}");
    assert!(t.ends_with("fix        :\n  Accept Apple's licence: run `sudo xcodebuild -license` in Terminal (or open Xcode once)\n"), "{t}");
}

// ── branch names ────────────────────────────────────────────────────────

#[test]
fn branch_names_are_checked_before_they_are_deferred() {
    assert_eq!(valid_branch("  trunk \n"), Ok("trunk".to_string()));
    assert_eq!(valid_branch("feature/x"), Ok("feature/x".to_string()));
    for bad in ["", "   ", "-x", "--upload-pack=evil", "a..b", "a b", "a~1", "x.lock", "a:b", "/x", "x/"] {
        assert!(valid_branch(bad).is_err(), "{bad:?} must be refused");
    }
    let d = temp("badbranch");
    assert!(defer_init(&d, "-x").is_err());
    assert_eq!(read_deferred(&d), None, "no marker for a refused name");
    defer_init(&d, "  trunk ").expect("defer");
    assert_eq!(read_deferred(&d).expect("marker").branch, "trunk", "stored trimmed");
    let _ = std::fs::remove_dir_all(&d);
}

// ── deferred steps ──────────────────────────────────────────────────────

#[test]
fn deferred_marker_round_trips_and_clears() {
    let d = temp("marker");
    assert_eq!(read_deferred(&d), None);
    defer_init(&d, "trunk").expect("defer");
    let got = read_deferred(&d).expect("marker written");
    assert_eq!(got.branch, "trunk");
    assert!(got.deferred_at > 0, "{got:?}");
    assert!(marker_path(&d).ends_with(".k2/git-deferred.json"));
    clear_deferred(&d);
    assert_eq!(read_deferred(&d), None);
    let _ = std::fs::remove_dir_all(&d);
}

/// 0.46.5 (first-run review item 14): a typo'd path is refused and never
/// created; a file isn't a folder either.
#[test]
fn a_missing_folder_is_refused_and_never_created() {
    let d = temp("typo");
    let typo = d.join("sitee").join("nested");
    let err = defer_init(&typo, "main").expect_err("a missing folder is refused");
    assert!(err.contains("There's no folder at") && err.contains("sitee"), "{err}");
    assert!(!d.join("sitee").exists(), "nothing was created at the typo");
    let file = d.join("a-file");
    std::fs::write(&file, "x").expect("write file");
    let err = defer_init(&file, "main").expect_err("a file is refused");
    assert!(err.contains("isn't a folder"), "{err}");
    assert!(!marker_path(&file).exists());
    let _ = std::fs::remove_dir_all(&d);
}

/// 0.46.5 (first-run review item 9): a marker written while the watch
/// loop's scan finds none is not lost. The old flag was set back to
/// "none" after that write, so the loop skipped the marker.
#[test]
fn a_marker_written_during_an_empty_scan_is_not_lost() {
    let f = PendingFlag::new();
    let seen = f.begin_scan().expect("pending at start (markers from before this run)");
    f.found_none(seen);
    assert_eq!(f.begin_scan(), None, "an empty scan clears it");
    // The race: the scan reads, a defer lands, the scan finds nothing.
    f.mark();
    let seen = f.begin_scan().expect("a marker was written");
    f.mark(); // defer_init during the scan
    f.found_none(seen); // that scan saw no marker
    assert!(f.begin_scan().is_some(), "the marker written during the scan is looked for again");
    let seen = f.begin_scan().expect("pending");
    f.found_none(seen);
    assert_eq!(f.begin_scan(), None);
    // An older scan finishing late never moves `clean` backwards.
    f.mark();
    let old = f.begin_scan().expect("pending");
    f.mark();
    let new = f.begin_scan().expect("pending");
    f.found_none(new);
    f.found_none(old);
    assert_eq!(f.begin_scan(), None);
}

/// 0.46.5 (first-run review item 12): a spawn that finishes a folder's
/// waiting git steps tells the listener (the daemon emits
/// `ProjectsChanged`), like the watch loop and a recheck do.
#[cfg(unix)]
#[test]
fn a_spawn_that_finishes_the_waiting_steps_tells_the_listener() {
    static SEEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    fn listener(p: &Path) {
        SEEN.lock().unwrap_or_else(|e| e.into_inner()).push(p.to_path_buf());
    }
    set_done_listener(listener);
    assert!(crate::git_safe::availability().is_ready(), "this test needs git on the test machine");
    let d = temp("spawn-done");
    // A repo already (no init or identity write on the test machine).
    std::fs::create_dir_all(d.join(".git")).expect("mkdir .git");
    defer_init(&d, "main").expect("defer");
    assert_eq!(before_spawn(Some(&d)), None, "git runs: no shim");
    assert_eq!(read_deferred(&d), None, "the steps ran");
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.iter().filter(|p| **p == d).count(), 1, "told once: {seen:?}");
    // Nothing waiting: nothing to tell.
    assert_eq!(before_spawn(Some(&d)), None);
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(seen.iter().filter(|p| **p == d).count(), 1, "not told again: {seen:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn run_deferred_without_a_marker_does_nothing() {
    let d = temp("nomarker");
    assert_eq!(run_deferred(&d, true), Ok(DeferredRun::NothingWaiting));
    assert!(!d.join(".git").exists());
    let _ = std::fs::remove_dir_all(&d);
}

/// The test machine has git; a folder that became a repo some other way
/// just loses its marker (no init over it, no identity write).
#[test]
fn run_deferred_on_a_folder_that_is_already_a_repo_only_clears_the_marker() {
    assert!(crate::git_safe::availability().is_ready(), "this test needs git on the test machine");
    let d = temp("already");
    std::fs::create_dir_all(d.join(".git")).expect("mkdir .git");
    defer_init(&d, "main").expect("defer");
    assert_eq!(run_deferred(&d, true), Ok(DeferredRun::AlreadyARepo));
    assert_eq!(read_deferred(&d), None);
    let _ = std::fs::remove_dir_all(&d);
}

/// An old marker with a name git refuses fails loudly, keeps the marker,
/// records the error for `waiting[].lastError`, and backs off the loop
/// (a forced run still tries). Fails before any git or identity write.
#[test]
fn a_bad_marker_records_its_error_and_backs_off() {
    assert!(crate::git_safe::availability().is_ready(), "this test needs git on the test machine");
    let d = temp("badmarker");
    std::fs::create_dir_all(d.join(".k2")).expect("mkdir .k2");
    std::fs::write(marker_path(&d), r#"{"branch":"-x","deferredAt":1}"#).expect("write marker");
    let err = run_deferred(&d, true).expect_err("refused");
    assert!(err.contains("can't start with -"), "{err}");
    assert!(read_deferred(&d).is_some(), "the marker stays");
    assert_eq!(last_failure(&d).map(|(_, e)| e), Some(err.clone()));
    assert_eq!(run_deferred(&d, false), Ok(DeferredRun::Backoff));
    assert!(run_deferred(&d, true).is_err(), "a forced run tries again");
    assert!(!d.join(".git").exists());
    set_failure(&d, None);
    let _ = std::fs::remove_dir_all(&d);
}

/// Two callers at once (the loop and a spawn) run the steps once: the
/// second waits on the lock, re-reads the marker and finds it gone.
#[test]
fn concurrent_runs_init_once() {
    assert!(crate::git_safe::availability().is_ready(), "this test needs git on the test machine");
    let d = temp("race");
    std::fs::create_dir_all(d.join(".git")).expect("mkdir .git");
    defer_init(&d, "main").expect("defer");
    let results: Vec<DeferredRun> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4).map(|_| s.spawn(|| run_deferred(&d, true).expect("run"))).collect();
        hs.into_iter().map(|h| h.join().expect("join")).collect()
    });
    assert_eq!(results.iter().filter(|r| **r == DeferredRun::AlreadyARepo).count(), 1, "{results:?}");
    assert!(results.iter().all(|r| matches!(r, DeferredRun::AlreadyARepo | DeferredRun::NothingWaiting)), "{results:?}");
    let _ = std::fs::remove_dir_all(&d);
}

// ── the agents' git-wait shim ───────────────────────────────────────────
//
// The shim uses shell builtins only, so these run it with a PATH of just
// the shim and stub folders (no system git can sneak in), with a stub
// Apple pair standing in for /usr/bin, on every OS.

/// Run through `/bin/sh <shim>` (not exec'd: the staged file was just
/// written by this process, see `test_env::write_executable`); `$0` is
/// still the shim's path.
#[cfg(unix)]
fn run_shim(shim_dir: &Path, path: &str) -> std::process::Output {
    std::process::Command::new("/bin/sh")
        .arg(shim_dir.join("git"))
        .arg("status")
        .env_clear()
        .env("PATH", path)
        .output()
        .expect("run the git-wait shim")
}

#[cfg(unix)]
#[test]
fn wait_shim_prints_the_note_and_never_runs_apples_shim() {
    let d = temp("shim");
    let usr_bin = d.join("usr-bin");
    let apple = ApplePair::in_dir(&usr_bin);
    let shim_dir = stage_wait_shim_at(&d.join("home"), &wait_shim_script(AGENT_NOTE_MAC, Some(&apple))).expect("stage");
    assert_eq!(shim_dir, wait_shim_dir_at(&d.join("home")));
    let shim_mark = d.join("apple-shim-ran");
    stub_git(&usr_bin, &shim_mark);
    stub_xcode_select(&usr_bin, None);
    // The shim folder twice: it must skip itself, not loop.
    let out = run_shim(&shim_dir, &format!("{0}:{0}:{1}", shim_dir.display(), usr_bin.display()));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), AGENT_NOTE_MAC);
    assert!(!shim_mark.exists(), "the git-wait shim ran Apple's shim (it opens the install dialog)");
    // A developer folder with no git in it doesn't count either.
    let dev = d.join("dev");
    std::fs::create_dir_all(&dev).expect("mkdir dev");
    stub_xcode_select(&usr_bin, Some(&dev));
    let out = run_shim(&shim_dir, &format!("{}:{}", shim_dir.display(), usr_bin.display()));
    assert_eq!(out.status.code(), Some(1));
    assert!(!shim_mark.exists(), "the git-wait shim ran Apple's shim with no git in the developer folder");
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn wait_shim_hands_over_once_a_real_git_exists() {
    let d = temp("shim-ready");
    let usr_bin = d.join("usr-bin");
    let apple = ApplePair::in_dir(&usr_bin);
    let shim_dir = stage_wait_shim_at(&d.join("home"), &wait_shim_script(AGENT_NOTE_MAC, Some(&apple))).expect("stage");
    // Apple's shim once the tools are installed.
    let dev = d.join("CommandLineTools");
    write_exec(&dev.join("usr/bin/git"), "#!/bin/sh\nexit 0\n");
    stub_git(&usr_bin, &d.join("apple-shim-ran"));
    stub_xcode_select(&usr_bin, Some(&dev));
    let out = run_shim(&shim_dir, &format!("{}:{}", shim_dir.display(), usr_bin.display()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "stub git status");
    assert!(d.join("apple-shim-ran").exists());

    // A Homebrew-style git, after Apple's shim whose tools are missing.
    stub_xcode_select(&usr_bin, None);
    let _ = std::fs::remove_file(d.join("apple-shim-ran"));
    let brew = d.join("brew-bin");
    stub_git(&brew, &d.join("brew-ran"));
    let out = run_shim(&shim_dir, &format!("{}:{}:{}", shim_dir.display(), usr_bin.display(), brew.display()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(d.join("brew-ran").exists());
    assert!(!d.join("apple-shim-ran").exists());

    // Only Apple's exact path is the shim: a git beside another
    // xcode-select is an ordinary git.
    let other = d.join("other");
    stub_git(&other, &d.join("other-ran"));
    stub_xcode_select(&other, None);
    let out = run_shim(&shim_dir, &format!("{}:{}", shim_dir.display(), other.display()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(d.join("other-ran").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn wait_shim_skips_any_git_wait_folder_and_restages_only_on_change() {
    let d = temp("shim-linux");
    let note = "git isn't installed on this machine yet; the owner can install it with: sudo apt install git";
    let home = d.join("home");
    let shim_dir = stage_wait_shim_at(&home, &wait_shim_script(note, None)).expect("stage");
    // Another user's wrapper folder on PATH is skipped too.
    let other_home = d.join("other-home");
    let other_dir = stage_wait_shim_at(&other_home, &wait_shim_script("other note", None)).expect("stage other");
    let out = run_shim(&shim_dir, &format!("{}:{}", shim_dir.display(), other_dir.display()));
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), note);

    // Staging the same text leaves the file alone; new text rewrites it.
    let path = shim_dir.join("git");
    let before = std::fs::metadata(&path).expect("stat").modified().expect("mtime");
    std::thread::sleep(Duration::from_millis(20));
    stage_wait_shim_at(&home, &wait_shim_script(note, None)).expect("stage again");
    assert_eq!(std::fs::metadata(&path).expect("stat").modified().expect("mtime"), before);
    stage_wait_shim_at(&home, &wait_shim_script(AGENT_NOTE_MAC, None)).expect("stage new note");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), wait_shim_script(AGENT_NOTE_MAC, None));
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777, 0o755);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn shim_quotes_its_values() {
    let apple = ApplePair { git: "/usr/bin/git".into(), xcode_select: "/usr/bin/xcode-select".into() };
    let s = wait_shim_script("isn't", Some(&apple));
    assert!(s.contains("echo 'isn'\\''t' >&2"), "{s}");
    assert!(s.contains("\napple_git='/usr/bin/git'\napple_xs='/usr/bin/xcode-select'\n"), "{s}");
    assert!(wait_shim_script("x", None).contains("\napple_git=''\n"));
    assert!(s.contains("self_dir=$(cd \"${0%/*}\" && pwd -P)"), "{s}");
}
