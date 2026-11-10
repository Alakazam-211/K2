//! 0.46.11: `k2` works in a new Terminal window after a normal install,
//! with no admin (`k2_client::cli_install`). Every test runs under a temp
//! `$HOME` ([`TempHome`]); `heal` / `remove_path_block_at` read it from
//! there, so nothing here can reach the real `~/.zprofile` or
//! `~/.local/bin`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cli_install::*;
use crate::test_env::TempHome;

const CLI_BODY: &str = "#!/bin/bash\nK2_CLI_VERSION=\"0.46.11\"\necho k2\n";
const SHIM_BODY: &str = "#!/bin/bash\nexec k2 \"$@\"\n";
/// A login PATH without `~/.local/bin` (a fresh Mac's).
const BARE_LOGIN_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// A fake `<home>/<dir>/K2.app` with `Contents/Resources/_up_/cli/{k2,k2so}`.
fn make_bundle(home: &Path, dir: &str) -> (PathBuf, PathBuf) {
    let cli_dir = home.join(dir).join("K2.app/Contents/Resources/_up_/cli");
    fs::create_dir_all(&cli_dir).unwrap();
    let cli = cli_dir.join("k2");
    let shim = cli_dir.join("k2so");
    fs::write(&cli, CLI_BODY).unwrap();
    fs::write(&shim, SHIM_BODY).unwrap();
    ensure_executable(&cli);
    ensure_executable(&shim);
    (cli, shim)
}

fn input<'a>(cli: &'a Path, shim: &'a Path, sys: Option<&'a Path>, login: Option<&'a str>) -> HealInput<'a> {
    HealInput {
        cli,
        legacy_shim: Some(shim),
        system_bin: sys,
        shell: Path::new("/bin/zsh"),
        zdotdir: None,
        login_path: login,
        force_path: false,
    }
}

fn zprofile(home: &Path) -> PathBuf {
    home.join(".zprofile")
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn begin_count(s: &str) -> usize {
    s.lines().filter(|l| *l == BLOCK_BEGIN).count()
}

fn link_target(p: &Path) -> PathBuf {
    fs::read_link(p).unwrap_or_else(|e| panic!("{} must be a link: {e}", p.display()))
}

#[test]
fn fresh_home_gets_the_link_and_exactly_one_block() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    assert!(!user_bin_dir(h).exists(), "a fresh home has no ~/.local/bin");

    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");

    let link = user_bin_dir(h).join("k2");
    assert_eq!(report.user_link, LinkOutcome::Created);
    assert_eq!(report.user_link_path, link);
    assert_eq!(link_target(&link), cli, "~/.local/bin/k2 runs the bundled CLI");
    assert_eq!(link_target(&user_bin_dir(h).join("k2so")), shim, "the k2so alias is linked too");
    assert_eq!(read(&link), CLI_BODY);

    assert_eq!(report.path, PathOutcome::BlockAdded { file: zprofile(h) });
    let z = read(&zprofile(h));
    assert_eq!(z, path_block(ShellKind::Zsh), "a new ~/.zprofile holds only K2's block");
    assert_eq!(begin_count(&z), 1);
    assert!(z.contains("export PATH=\"$HOME/.local/bin:$PATH\""), "{z}");
}

#[test]
fn an_existing_zprofile_keeps_its_lines_and_gets_the_block_at_the_end() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let mine = "eval \"$(/opt/homebrew/bin/brew shellenv)\"";
    fs::write(zprofile(h), mine).unwrap();

    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");

    assert_eq!(read(&zprofile(h)), format!("{mine}\n{}", path_block(ShellKind::Zsh)));
}

#[test]
fn a_second_run_adds_nothing() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("first heal");
    let after_first = read(&zprofile(h));

    // Same login PATH (the probe ran before the block existed).
    let again = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("second heal");
    assert_eq!(again.user_link, LinkOutcome::Current);
    assert_eq!(again.user_legacy, Some(LinkOutcome::Current));
    assert_eq!(again.path, PathOutcome::BlockPresent { file: zprofile(h) });
    assert_eq!(read(&zprofile(h)), after_first, "the second run must not touch ~/.zprofile");

    // The login PATH a new window reports once the block is in.
    let with_bin = format!("{}:{BARE_LOGIN_PATH}", user_bin_dir(h).display());
    let third = heal(&input(&cli, &shim, None, Some(&with_bin))).expect("third heal");
    assert_eq!(third.user_link, LinkOutcome::Current);
    assert_eq!(read(&zprofile(h)), after_first);
    assert_eq!(begin_count(&read(&zprofile(h))), 1);
}

#[test]
fn a_foreign_k2_file_in_local_bin_is_left_alone_and_no_path_line_is_added() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();
    let other = "#!/bin/sh\necho some other k2\n";
    fs::write(bin.join("k2"), other).unwrap();

    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");

    assert!(matches!(report.user_link, LinkOutcome::Foreign { .. }), "{:?}", report.user_link);
    assert_eq!(read(&bin.join("k2")), other, "a k2 that isn't K2's is never overwritten");
    assert!(!fs::symlink_metadata(bin.join("k2")).unwrap().file_type().is_symlink());
    assert!(matches!(report.path, PathOutcome::Skipped { .. }), "{:?}", report.path);
    assert!(!zprofile(h).exists(), "no PATH line for a k2 that isn't K2's");
    assert!(
        report.log_lines().iter().any(|l| l.contains("left") && l.contains("alone")),
        "one log line says it was left: {:?}",
        report.log_lines()
    );
}

#[test]
fn a_foreign_k2_link_is_left_alone_in_both_bin_folders() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let elsewhere = h.join("opt/other-tool/bin/k2");
    fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    fs::write(&elsewhere, "#!/bin/sh\n").unwrap();

    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&elsewhere, bin.join("k2")).unwrap();

    // A writable "/usr/local/bin" with someone else's k2 in it.
    let sys = h.join("usr-local-bin");
    fs::create_dir_all(&sys).unwrap();
    std::os::unix::fs::symlink(&elsewhere, sys.join("k2")).unwrap();

    let report = heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal");

    assert!(matches!(report.user_link, LinkOutcome::Foreign { .. }), "{:?}", report.user_link);
    assert!(matches!(report.system_link, Some(LinkOutcome::Foreign { .. })), "{:?}", report.system_link);
    assert_eq!(link_target(&bin.join("k2")), elsewhere);
    assert_eq!(link_target(&sys.join("k2")), elsewhere);
    assert!(!sys.join("k2so").exists(), "no k2so beside a k2 that isn't K2's");
}

#[test]
fn a_link_reaching_the_cli_by_an_equivalent_path_is_current() {
    // #56: `/Applications` vs `/System/Volumes/Data/Applications` (firmlinks)
    // are the same file by two paths. Comparing raw paths re-healed every
    // launch; the canonical compare leaves a correct link alone.
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let alias = h.join("Data-Applications");
    std::os::unix::fs::symlink(h.join("Applications"), &alias).unwrap();
    let via_alias = alias.join("K2.app/Contents/Resources/_up_/cli/k2");
    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&via_alias, bin.join("k2")).unwrap();

    assert_eq!(link_state(&bin.join("k2"), &cli), LinkState::Current);
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Current);
    assert_eq!(link_target(&bin.join("k2")), via_alias, "a correct link is never rewritten");
}

#[test]
fn a_writable_system_bin_is_linked_and_a_missing_one_is_never_made() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");

    let sys = h.join("usr-local-bin");
    fs::create_dir_all(&sys).unwrap();
    let report = heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.system_link, Some(LinkOutcome::Created));
    assert_eq!(link_target(&sys.join("k2")), cli);
    assert_eq!(link_target(&sys.join("k2so")), shim);

    let missing = h.join("no-such-bin");
    let report = heal(&input(&cli, &shim, Some(&missing), Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.system_link, None);
    assert!(!missing.exists(), "the heal never creates the system bin folder");
}

#[test]
fn the_undo_removes_only_the_block_and_the_boot_heal_leaves_it_out() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let before = "# my setup\nexport EDITOR=vim\n";
    fs::write(zprofile(h), before).unwrap();
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    // The person adds a line after K2's block.
    let after = "alias gs='git status'\n";
    let mut z = read(&zprofile(h));
    z.push_str(after);
    fs::write(zprofile(h), &z).unwrap();

    let changed = remove_path_block_at(h, None).expect("undo");

    assert_eq!(changed, vec![zprofile(h)]);
    assert_eq!(read(&zprofile(h)), format!("{before}{after}"), "only K2's lines go");
    assert!(path_opted_out(h));

    // The next boot heal keeps the link but doesn't add the line back.
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal after undo");
    assert_eq!(report.user_link, LinkOutcome::Current);
    assert_eq!(report.path, PathOutcome::OptedOut);
    assert_eq!(read(&zprofile(h)), format!("{before}{after}"));

    // An explicit install from Settings adds it again, once.
    let forced = heal(&HealInput { force_path: true, ..input(&cli, &shim, None, Some(BARE_LOGIN_PATH)) })
        .expect("forced heal");
    assert_eq!(forced.path, PathOutcome::BlockAdded { file: zprofile(h) });
    assert!(!path_opted_out(h));
    assert_eq!(begin_count(&read(&zprofile(h))), 1);
}

#[test]
fn the_undo_with_no_block_changes_no_file() {
    let home = TempHome::new();
    let h = home.path();
    let mine = "export A=1\n";
    fs::write(zprofile(h), mine).unwrap();
    let changed = remove_path_block_at(h, None).expect("undo");
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(read(&zprofile(h)), mine);
}

#[test]
fn a_stale_link_to_an_old_bundle_is_repointed() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();

    // A link into a bundle that has since been deleted (K2SO.app → K2.app).
    let gone = h.join("Old/K2SO.app/Contents/Resources/_up_/cli/k2");
    std::os::unix::fs::symlink(&gone, bin.join("k2")).unwrap();
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Repointed { from: gone });
    assert_eq!(link_target(&bin.join("k2")), cli);

    // The app moved: the link still points at the old (existing) copy.
    let (moved_cli, moved_shim) = make_bundle(h, "Moved");
    let report = heal(&input(&moved_cli, &moved_shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Repointed { from: cli.clone() });
    assert_eq!(link_target(&bin.join("k2")), moved_cli);
    assert_eq!(link_target(&bin.join("k2so")), moved_shim);
    assert_eq!(begin_count(&read(&zprofile(h))), 1, "re-pointing never adds a second block");
}

#[test]
fn no_block_when_the_login_path_already_has_local_bin_or_runs_a_k2_cli() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");

    let with_bin = format!("/usr/bin:{}/:/bin", user_bin_dir(h).display());
    let report = heal(&input(&cli, &shim, None, Some(&with_bin))).expect("heal");
    assert_eq!(report.path, PathOutcome::OnPath);
    assert!(!zprofile(h).exists());

    // /usr/local/bin/k2 already runs K2 (an older install): no line needed.
    let home2 = TempHome::new();
    let h2 = home2.path();
    let (cli2, shim2) = make_bundle(h2, "Applications");
    let sys = h2.join("usr-local-bin");
    fs::create_dir_all(&sys).unwrap();
    std::os::unix::fs::symlink(&cli2, sys.join("k2")).unwrap();
    let login = format!("{}:{BARE_LOGIN_PATH}", sys.display());
    let report = heal(&input(&cli2, &shim2, Some(&sys), Some(&login))).expect("heal");
    assert_eq!(report.system_link, Some(LinkOutcome::Current));
    assert_eq!(report.path, PathOutcome::FoundElsewhere { path: sys.join("k2") });
    assert!(!zprofile(h2).exists());
}

#[test]
fn a_k2_on_path_that_is_not_the_cli_does_not_count() {
    // The Linux .deb puts the GUI binary at /usr/bin/k2: a `k2` on PATH
    // that isn't the CLI script must not stop the PATH line.
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let gui_bin = h.join("usr-bin");
    fs::create_dir_all(&gui_bin).unwrap();
    fs::write(gui_bin.join("k2"), b"\x7fELF\x02\x01\x01").unwrap();
    let login = format!("{}:{BARE_LOGIN_PATH}", gui_bin.display());

    let report = heal(&input(&cli, &shim, None, Some(&login))).expect("heal");
    assert_eq!(report.path, PathOutcome::BlockAdded { file: zprofile(h) });
}

#[test]
fn no_login_path_writes_nothing_unless_installed_from_settings() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");

    let report = heal(&input(&cli, &shim, None, None)).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Created);
    assert!(matches!(report.path, PathOutcome::Skipped { .. }), "{:?}", report.path);
    assert!(!zprofile(h).exists(), "an unanswered probe never edits a startup file");

    let forced = heal(&HealInput { force_path: true, ..input(&cli, &shim, None, None) }).expect("forced");
    assert_eq!(forced.path, PathOutcome::BlockAdded { file: zprofile(h) });
}

#[test]
fn a_linked_zprofile_is_written_at_its_target_and_stays_a_link() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let real = h.join("dotfiles/zprofile");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "export A=1\n").unwrap();
    std::os::unix::fs::symlink(&real, zprofile(h)).unwrap();

    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");

    assert!(fs::symlink_metadata(zprofile(h)).unwrap().file_type().is_symlink(), "~/.zprofile stays a link");
    assert_eq!(read(&real), format!("export A=1\n{}", path_block(ShellKind::Zsh)));

    remove_path_block_at(h, None).expect("undo");
    assert!(fs::symlink_metadata(zprofile(h)).unwrap().file_type().is_symlink());
    assert_eq!(read(&real), "export A=1\n");
}

#[test]
fn bash_uses_the_login_file_bash_reads_and_fish_gets_its_own_conf() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    // Ubuntu: .profile but no .bash_profile; creating one would hide .profile.
    fs::write(h.join(".profile"), "# ubuntu\n").unwrap();
    let report = heal(&HealInput { shell: Path::new("/bin/bash"), ..input(&cli, &shim, None, Some(BARE_LOGIN_PATH)) })
        .expect("bash heal");
    assert_eq!(report.path, PathOutcome::BlockAdded { file: h.join(".profile") });
    assert!(!h.join(".bash_profile").exists(), "never create a .bash_profile over .profile");
    assert_eq!(read(&h.join(".profile")), format!("# ubuntu\n{}", path_block(ShellKind::Bash)));

    let home2 = TempHome::new();
    let h2 = home2.path();
    let (cli2, shim2) = make_bundle(h2, "Applications");
    let report = heal(&HealInput { shell: Path::new("/usr/bin/fish"), ..input(&cli2, &shim2, None, Some(BARE_LOGIN_PATH)) })
        .expect("fish heal");
    let conf = h2.join(".config/fish/conf.d/k2-cli.fish");
    assert_eq!(report.path, PathOutcome::BlockAdded { file: conf.clone() });
    assert_eq!(read(&conf), path_block(ShellKind::Fish));
    remove_path_block_at(h2, None).expect("undo");
    assert!(!conf.exists(), "the fish file was all K2's, so the undo deletes it");
}

#[test]
fn an_unknown_shell_gets_the_link_and_no_startup_file() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let report = heal(&HealInput { shell: Path::new("/bin/tcsh"), ..input(&cli, &shim, None, Some(BARE_LOGIN_PATH)) })
        .expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Created);
    assert!(matches!(report.path, PathOutcome::Skipped { .. }), "{:?}", report.path);
    assert!(!h.join(".cshrc").exists() && !zprofile(h).exists());
}

#[test]
fn path_status_says_whether_new_terminal_windows_find_k2() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let zsh = Path::new("/bin/zsh");

    let before = path_status_at(h, zsh, None, Some(BARE_LOGIN_PATH));
    assert!(!before.new_terminals_find_it);
    assert!(!before.block_present);
    assert_eq!(before.on_login_path, Some(false));
    assert_eq!(before.startup_file, Some(zprofile(h)));

    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    let after = path_status_at(h, zsh, None, Some(BARE_LOGIN_PATH));
    assert!(after.block_present);
    assert!(after.new_terminals_find_it, "{after:?}");
    assert_eq!(after.new_terminals_run, Some(user_bin_dir(h).join("k2")));

    remove_path_block_at(h, None).expect("undo");
    let undone = path_status_at(h, zsh, None, Some(BARE_LOGIN_PATH));
    assert!(!undone.new_terminals_find_it);
    assert!(undone.opted_out);
}

#[test]
fn the_packaged_cli_is_found_from_the_app_and_the_daemon_but_never_a_dev_build() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, _shim) = make_bundle(h, "Applications");
    let app = h.join("Applications/K2.app");

    let app_exe = app.join("Contents/MacOS/k2");
    let daemon_exe = app.join("Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon");
    assert_eq!(packaged_cli_for_exe(&app_exe, "k2"), Some(cli.clone()));
    assert_eq!(packaged_cli_for_exe(&daemon_exe, "k2"), Some(cli));

    let repo = h.join("repo");
    fs::create_dir_all(repo.join("cli")).unwrap();
    fs::write(repo.join("cli/k2"), CLI_BODY).unwrap();
    assert_eq!(packaged_cli_for_exe(&repo.join("target/debug/k2-daemon"), "k2"), None);
}
