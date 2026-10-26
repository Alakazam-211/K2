//! Tests for `git_tools` (0.46.4 first run without git). Stub `git` and
//! `xcode-select` binaries stand in for Apple's; nothing here runs the
//! real `xcode-select --install`.

use super::*;
use std::time::{Duration, SystemTime};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-git-tools-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    d
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

// ── install state ───────────────────────────────────────────────────────

#[test]
fn install_state_follows_the_click_and_the_installer() {
    let now = SystemTime::now();
    let ago = |s: u64| Some(now - Duration::from_secs(s));
    assert_eq!(install_state(true, true, None, now, false), InstallState::Ready);
    assert_eq!(install_state(true, false, None, now, false), InstallState::Ready);
    assert_eq!(install_state(false, true, None, now, false), InstallState::NotInstalled);
    // Just clicked: installing even before Apple's window is listed.
    assert_eq!(install_state(false, true, ago(5), now, false), InstallState::Installing);
    // Later, only while Apple's installer is up.
    assert_eq!(install_state(false, true, ago(600), now, true), InstallState::Installing);
    assert_eq!(install_state(false, true, ago(600), now, false), InstallState::NotInstalled);
    // Never for more than three hours.
    assert_eq!(install_state(false, true, ago(4 * 3600), now, true), InstallState::NotInstalled);
    // Linux has no installer K2 starts.
    assert_eq!(install_state(false, false, ago(5), now, true), InstallState::NotInstalled);
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

// ── the agents' git-wait shim ───────────────────────────────────────────
//
// The shim uses shell builtins only, so these run it with a PATH of just
// the shim and stub folders (no system git can sneak in), and with the
// Mac rules on, on every OS.

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
    let shim_dir = stage_wait_shim_at(&d.join("home"), &wait_shim_script(AGENT_NOTE_MAC, true)).expect("stage");
    assert_eq!(shim_dir, wait_shim_dir_at(&d.join("home")));
    // The stub folder stands in for /usr/bin: Apple's git shim beside an
    // xcode-select that says the tools aren't installed.
    let usr_bin = d.join("usr-bin");
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
    let shim_dir = stage_wait_shim_at(&d.join("home"), &wait_shim_script(AGENT_NOTE_MAC, true)).expect("stage");
    // Apple's shim once the tools are installed.
    let usr_bin = d.join("usr-bin");
    let dev = d.join("CommandLineTools");
    write_exec(&dev.join("usr/bin/git"), "#!/bin/sh\nexit 0\n");
    stub_git(&usr_bin, &d.join("apple-shim-ran"));
    stub_xcode_select(&usr_bin, Some(&dev));
    let out = run_shim(&shim_dir, &format!("{}:{}", shim_dir.display(), usr_bin.display()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "stub git status");
    assert!(d.join("apple-shim-ran").exists());

    // A Homebrew-style git (no xcode-select beside it), after a shim
    // folder whose tools are still missing.
    let no_tools = d.join("no-tools");
    stub_git(&no_tools, &d.join("no-tools-ran"));
    stub_xcode_select(&no_tools, None);
    let brew = d.join("brew-bin");
    stub_git(&brew, &d.join("brew-ran"));
    let out = run_shim(&shim_dir, &format!("{}:{}:{}", shim_dir.display(), no_tools.display(), brew.display()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(d.join("brew-ran").exists());
    assert!(!d.join("no-tools-ran").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn wait_shim_off_mac_ignores_xcode_select_and_restages_only_on_change() {
    let d = temp("shim-linux");
    let note = "git isn't installed on this machine yet; the owner can install it with: sudo apt install git";
    let home = d.join("home");
    let shim_dir = stage_wait_shim_at(&home, &wait_shim_script(note, false)).expect("stage");
    let out = run_shim(&shim_dir, &shim_dir.display().to_string());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), note);
    // Not a Mac: a git beside an xcode-select is just a git.
    let bin = d.join("bin");
    stub_git(&bin, &d.join("ran"));
    stub_xcode_select(&bin, None);
    let out = run_shim(&shim_dir, &format!("{}:{}", shim_dir.display(), bin.display()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(d.join("ran").exists());

    // Staging the same text leaves the file alone; new text rewrites it.
    let path = shim_dir.join("git");
    let before = std::fs::metadata(&path).expect("stat").modified().expect("mtime");
    std::thread::sleep(Duration::from_millis(20));
    stage_wait_shim_at(&home, &wait_shim_script(note, false)).expect("stage again");
    assert_eq!(std::fs::metadata(&path).expect("stat").modified().expect("mtime"), before);
    stage_wait_shim_at(&home, &wait_shim_script(AGENT_NOTE_MAC, true)).expect("stage new note");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), wait_shim_script(AGENT_NOTE_MAC, true));
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777, 0o755);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn shim_quotes_a_note_with_an_apostrophe() {
    let s = wait_shim_script("isn't", true);
    assert!(s.contains("echo 'isn'\\''t' >&2"), "{s}");
    assert!(s.contains("\nmac=1\n"), "{s}");
    assert!(wait_shim_script("x", false).contains("\nmac=0\n"));
    assert!(s.contains("self_dir=$(cd \"${0%/*}\" && pwd -P)"), "{s}");
}

