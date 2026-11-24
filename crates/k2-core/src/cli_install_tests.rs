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
    assert!(fs::symlink_metadata(user_bin_dir(h).join("k2so")).is_err(), "a new install gets no k2so alias");
    assert_eq!(report.user_legacy, None);
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
    assert_eq!(again.user_legacy, None, "still no k2so alias");
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
fn an_existing_system_k2so_alias_is_refreshed_and_a_foreign_one_left_alone() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let sys = h.join("usr-local-bin");
    fs::create_dir_all(&sys).unwrap();
    let old_shim = h.join("Old/K2SO.app/Contents/Resources/_up_/cli/k2so");
    std::os::unix::fs::symlink(&old_shim, sys.join("k2so")).unwrap();

    let report = heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.system_legacy, Some(LinkOutcome::Repointed { from: old_shim }));
    assert_eq!(link_target(&sys.join("k2so")), shim, "the alias an earlier install made keeps working");

    // A k2so in ~/.local/bin that isn't K2's stays as it is.
    let bin = user_bin_dir(h);
    let other = "#!/bin/sh\necho some other k2so\n";
    fs::write(bin.join("k2so"), other).unwrap();
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert!(matches!(report.user_legacy, Some(LinkOutcome::Foreign { .. })), "{:?}", report.user_legacy);
    assert_eq!(read(&bin.join("k2so")), other);
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
    assert!(fs::symlink_metadata(sys.join("k2so")).is_err(), "no new k2so alias in the system bin");
    assert_eq!(report.system_legacy, None);

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

    // A link into a bundle that has since been deleted (K2SO.app → K2.app),
    // with the k2so alias an earlier install left beside it.
    let gone = h.join("Old/K2SO.app/Contents/Resources/_up_/cli/k2");
    let gone_shim = h.join("Old/K2SO.app/Contents/Resources/_up_/cli/k2so");
    std::os::unix::fs::symlink(&gone, bin.join("k2")).unwrap();
    std::os::unix::fs::symlink(&gone_shim, bin.join("k2so")).unwrap();
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.user_link, LinkOutcome::Repointed { from: gone });
    assert_eq!(link_target(&bin.join("k2")), cli);
    assert_eq!(report.user_legacy, Some(LinkOutcome::Repointed { from: gone_shim }), "an existing k2so alias keeps working");
    assert_eq!(link_target(&bin.join("k2so")), shim);

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

// ── New k5 command (Beta): what the links run ───────────────────────────

/// A stand-in for the k5 binary: an ELF header and the version marker the
/// real one carries (`K5_CLI_VERSION=<v>` and a NUL), never run.
fn binary_body(version: &str) -> Vec<u8> {
    let mut b = b"\x7fELF\x02\x01\x01\x00 some code ".to_vec();
    b.extend_from_slice(format!("K5_CLI_VERSION={version}\0").as_bytes());
    b.extend_from_slice(b" more bytes");
    b
}

/// The binary beside the app's executable in a bundle made by
/// [`make_bundle`]: `K2.app/Contents/MacOS/k5-cli`.
fn put_binary(home: &Path, dir: &str, version: &str) -> PathBuf {
    let macos = home.join(dir).join("K2.app/Contents/MacOS");
    fs::create_dir_all(&macos).unwrap();
    let bin = macos.join("k5-cli");
    fs::write(&bin, binary_body(version)).unwrap();
    ensure_executable(&bin);
    bin
}

fn switch(on: bool) {
    let file = crate::paths::k2_home().join("settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, format!("{{\"k5CliBeta\": {on}}}")).unwrap();
}

fn k5(home: &Path) -> PathBuf {
    user_bin_dir(home).join("k5")
}

#[test]
fn the_switch_is_off_by_default_and_the_links_run_the_script() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    put_binary(h, "Applications", "0.46.11");
    assert!(!switch_on(), "a fresh home has the switch off");

    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.runs, Choice::Script { note: None }, "off: the script, and no note");
    assert_eq!(link_target(&user_bin_dir(h).join("k2")), cli);
    assert!(fs::symlink_metadata(k5(h)).is_err(), "no k5 link while the switch is off");
    assert_eq!(report.user_k5, None);
}

#[test]
fn switching_on_points_k2_and_k5_at_the_binary_and_off_points_them_back() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let binary = put_binary(h, "Applications", "0.46.11");
    let sys = h.join("usr-local-bin");
    fs::create_dir_all(&sys).unwrap();
    let k2 = user_bin_dir(h).join("k2");
    heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("first heal");
    assert_eq!(link_target(&k2), cli);

    // Script to binary.
    switch(true);
    let on = heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal on");
    assert_eq!(on.runs, Choice::Binary { binary: binary.clone() });
    assert_eq!(on.user_link, LinkOutcome::Repointed { from: cli.clone() });
    assert_eq!(on.user_k5, Some(LinkOutcome::Created));
    assert_eq!(link_target(&k2), binary);
    assert_eq!(link_target(&k5(h)), binary);
    assert_eq!(on.system_link, Some(LinkOutcome::Repointed { from: cli.clone() }));
    assert_eq!(on.system_k5, Some(LinkOutcome::Created));
    assert_eq!(link_target(&sys.join("k5")), binary);
    assert!(on.log_lines().iter().any(|l| l.starts_with("k5 linked at")), "{:?}", on.log_lines());
    // Both are K2's, and a new window finds the K2 CLI through them.
    assert!(is_k2_cli_target(&binary));
    assert!(is_k2_cli_script(&k2), "the binary counts as the K2 CLI");
    assert_eq!(link_state(&k2, &cli), LinkState::Stale { target: binary.clone() }, "ours, not foreign");
    let status = path_status_at(h, Path::new("/bin/zsh"), None, Some(BARE_LOGIN_PATH));
    assert_eq!(status.new_terminals_run, Some(k2.clone()));
    // Idempotent.
    let again = heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal again");
    assert_eq!((again.user_link, again.user_k5), (LinkOutcome::Current, Some(LinkOutcome::Current)));

    // Binary back to script: both re-pointed, k5 kept (never removed).
    switch(false);
    let off = heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal off");
    assert_eq!(off.runs, Choice::Script { note: None });
    assert_eq!(off.user_link, LinkOutcome::Repointed { from: binary.clone() });
    assert_eq!(off.user_k5, Some(LinkOutcome::Repointed { from: binary.clone() }));
    assert_eq!(link_target(&k2), cli);
    assert_eq!(link_target(&k5(h)), cli);
    assert_eq!(link_target(&sys.join("k2")), cli);
    assert_eq!(link_target(&sys.join("k5")), cli);
    assert_eq!(begin_count(&read(&zprofile(h))), 1, "flips never add a second PATH block");
}

#[test]
fn a_missing_binary_means_the_script_with_a_note() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    switch(true);
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    match &report.runs {
        Choice::Script { note: Some(n) } => assert!(n.contains("no k5 program") && n.contains("is missing"), "{n}"),
        other => panic!("a missing binary must mean the script: {other:?}"),
    }
    assert_eq!(link_target(&user_bin_dir(h).join("k2")), cli);
    assert!(fs::symlink_metadata(k5(h)).is_err(), "no k5 link without the binary");
    assert!(report.log_lines().iter().any(|l| l.contains("links run the script")), "{:?}", report.log_lines());
}

#[test]
fn a_binary_of_another_version_means_the_script_and_a_link_to_it_goes_back() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let binary = put_binary(h, "Applications", "0.46.11");
    switch(true);
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal on");
    assert_eq!(link_target(&user_bin_dir(h).join("k2")), binary);

    // The app updated its script but this binary is the old release's.
    for (wrong, why) in [
        (binary_body("0.46.10"), "version 0.46.10"),
        (binary_body("0.46.1"), "version 0.46.1,"),
        (binary_body("0.46.110"), "version 0.46.110"),
        (b"\x7fELF no marker at all K5_CLI_VERSION=0.46.11".to_vec(), "no k5 version mark"),
    ] {
        fs::write(&binary, &wrong).unwrap();
        let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
        match &report.runs {
            Choice::Script { note: Some(n) } => assert!(n.contains(why), "{why}: {n}"),
            other => panic!("{why}: a wrong version must mean the script: {other:?}"),
        }
        assert_eq!(link_target(&user_bin_dir(h).join("k2")), cli, "{why}");
        assert_eq!(link_target(&k5(h)), cli, "{why}: k5 follows k2");
    }
}

#[test]
fn a_k5_that_is_not_k2_s_is_left_alone() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    put_binary(h, "Applications", "0.46.11");
    let bin = user_bin_dir(h);
    fs::create_dir_all(&bin).unwrap();
    let other = "#!/bin/sh\necho another k5 tool\n";
    fs::write(bin.join("k5"), other).unwrap();
    switch(true);
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert!(matches!(report.user_k5, Some(LinkOutcome::Foreign { .. })), "{:?}", report.user_k5);
    assert_eq!(read(&bin.join("k5")), other);
    switch(false);
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.user_k5, None, "off: someone else's k5 isn't even mentioned");
    assert_eq!(read(&bin.join("k5")), other);
}

#[test]
fn an_interrupted_swap_leaves_a_working_link_at_every_step() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let binary = put_binary(h, "Applications", "0.46.11");
    let bin = user_bin_dir(h);
    let k2 = bin.join("k2");
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    switch(true);

    // 1. Stopped after the temp link, before the rename: k2 still runs the
    //    script; the temp sits beside it.
    let tmp = begin_link_swap(&binary, &k2).expect("temp link");
    assert_eq!(link_target(&k2), cli, "the old link is untouched");
    assert!(is_k2_cli_script(&k2));
    assert_eq!(link_target(&tmp), binary);
    // The process that made it is gone (a pid no system hands out).
    let dead = bin.join(".k2.k2-link-2147483647");
    fs::rename(&tmp, &dead).unwrap();

    // 2. Stopped between the two links: k2 on the binary, k5 not made yet.
    assert_eq!(ensure_link(&k2, &binary, true), LinkOutcome::Repointed { from: cli.clone() });
    assert!(is_k2_cli_script(&k2), "k2 runs the binary");
    assert!(fs::symlink_metadata(k5(h)).is_err());

    // The next heal finishes both and removes the dead process's temp.
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(report.swept, vec![dead.clone()]);
    assert!(fs::symlink_metadata(&dead).is_err());
    assert_eq!(link_target(&k2), binary);
    assert_eq!(link_target(&k5(h)), binary);

    // Turning off, stopped between the two: k2 on the script, k5 on the
    // binary. Both work; the next heal finishes.
    switch(false);
    assert_eq!(ensure_link(&k2, &cli, true), LinkOutcome::Repointed { from: binary.clone() });
    assert!(is_k2_cli_script(&k2) && is_k2_cli_script(&k5(h)), "both run a K2 CLI");
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert_eq!(link_target(&k5(h)), cli);

    // A live process's temp is its own: never swept.
    let live = begin_link_swap(&cli, &k2).expect("temp link");
    let report = heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal");
    assert!(report.swept.is_empty(), "{:?}", report.swept);
    assert!(fs::symlink_metadata(&live).is_ok());
    finish_swap(&live, &k2).expect("rename");
    assert_eq!(link_target(&k2), cli);
}

#[test]
fn the_check_reads_the_marker_without_running_the_binary() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, _shim) = make_bundle(h, "Applications");
    let binary = put_binary(h, "Applications", "0.46.11");
    // Not executable at all: a check that ran it would fail.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(check_binary(&binary, Some("0.46.11")), Ok(()));
    assert_eq!(script_version(&cli).as_deref(), Some("0.46.11"));
    assert_eq!(binary_for_script_if_on(true, &cli), Ok(binary.clone()));
    assert!(binary_for_script_if_on(false, &cli).is_err(), "off never names it");
    // A checkout's script has no bundle beside it.
    let repo = h.join("repo/cli");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("k2"), CLI_BODY).unwrap();
    assert!(binary_for_script_if_on(true, &repo.join("k2")).is_err());
    assert!(!is_k5_binary(&cli), "the script is not the binary");
    assert!(is_k5_binary(&binary));
}

// ── Uninstall (Settings) ────────────────────────────────────────────────

/// K2's ruling (slice 10c(iii)): the uninstall takes a `k5` link too, but
/// only one that is K2's (to the script or the binary); someone else's `k5`
/// stays, and the binary itself is never touched.
#[test]
fn the_uninstall_removes_k2_s_k5_links_and_never_someone_else_s() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let binary = put_binary(h, "Applications", "0.46.11");
    let body = fs::read(&binary).unwrap();
    let sys = h.join("usr-local-bin");
    fs::create_dir_all(&sys).unwrap();
    let bin = user_bin_dir(h);

    // On: k2 and k5 run the binary in both folders.
    switch(true);
    heal(&input(&cli, &shim, Some(&sys), Some(BARE_LOGIN_PATH))).expect("heal on");
    for d in [&bin, &sys] {
        assert_eq!(link_target(&d.join("k5")), binary);
        assert!(is_our_link(&d.join("k2")) && is_our_link(&d.join("k5")));
    }
    assert_eq!(our_links_in(&sys, &[CLI_NAME, K5_NAME]), vec![sys.join("k2"), sys.join("k5")]);
    assert_eq!(remove_our_links(&bin, &UNINSTALL_NAMES).expect("uninstall"), vec![bin.join("k2"), bin.join("k5")]);
    assert_eq!(remove_our_links(&sys, &[CLI_NAME, K5_NAME]).expect("uninstall"), vec![sys.join("k2"), sys.join("k5")]);
    for d in [&bin, &sys] {
        assert!(fs::symlink_metadata(d.join("k2")).is_err() && fs::symlink_metadata(d.join("k5")).is_err());
    }
    assert_eq!(fs::read(&binary).unwrap(), body, "the binary itself stays");
    assert_eq!(read(&cli), CLI_BODY, "the script itself stays");

    // Off after on: k5 runs the script, and is still K2's; so is a k2so
    // alias and a link to a bundle that is gone.
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal on again");
    switch(false);
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal off");
    assert_eq!(link_target(&k5(h)), cli);
    std::os::unix::fs::symlink(&shim, bin.join("k2so")).unwrap();
    let old = h.join("Old/K2.app/Contents/MacOS/k5-cli");
    std::os::unix::fs::symlink(&old, sys.join("k5")).unwrap();
    assert_eq!(
        remove_our_links(&bin, &UNINSTALL_NAMES).expect("uninstall"),
        vec![bin.join("k2"), bin.join("k5"), bin.join("k2so")]
    );
    assert_eq!(remove_our_links(&sys, &[CLI_NAME, K5_NAME]).expect("uninstall"), vec![sys.join("k5")]);

    // Someone else's k5: a file, a link to another tool, a folder. Kept.
    let other = h.join("opt/other/bin/k5");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, "#!/bin/sh\necho another k5 tool\n").unwrap();
    fs::write(bin.join("k5"), "#!/bin/sh\necho mine\n").unwrap();
    std::os::unix::fs::symlink(&other, sys.join("k5")).unwrap();
    for d in [&bin, &sys] {
        assert!(!is_our_link(&d.join("k5")));
        assert_eq!(remove_our_links(d, &UNINSTALL_NAMES).expect("uninstall"), Vec::<PathBuf>::new());
    }
    assert_eq!(read(&bin.join("k5")), "#!/bin/sh\necho mine\n");
    assert_eq!(link_target(&sys.join("k5")), other);
    fs::remove_file(sys.join("k5")).unwrap();
    fs::create_dir_all(sys.join("k5")).unwrap();
    assert!(our_links_in(&sys, &UNINSTALL_NAMES).is_empty());
    assert!(sys.join("k5").is_dir());
}

// ── Status (Settings) ───────────────────────────────────────────────────

/// K2's ruling (slice 10c(iii)): Settings shows a link to the binary as the
/// binary with its version, not "stale". A link is judged against what the
/// links should run (`Choice::target`), and `installed_cli` reads what runs
/// there and its version from the file, never by running it.
#[test]
fn settings_shows_a_link_to_the_binary_as_the_binary_with_its_version() {
    let home = TempHome::new();
    let h = home.path();
    let (cli, shim) = make_bundle(h, "Applications");
    let binary = put_binary(h, "Applications", "0.46.11");
    let k2 = user_bin_dir(h).join("k2");

    // Off: the links run the script; it is current, a script of 0.46.11.
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal off");
    let runs = choice_for_script(switch_on(), &cli);
    assert_eq!(runs.target(&cli), Some(cli.as_path()));
    assert_eq!(
        link_state(&k2, runs.target(&cli).expect("a target")),
        LinkState::Current
    );
    let script = InstalledCli { runs: CliKind::Script, version: Some("0.46.11".to_string()) };
    assert_eq!(installed_cli(&k2, Some("0.46.11")), Some(script));

    // On: k2 is a link to the binary. Against the choice it is current, and
    // what runs is the binary, version 0.46.11.
    switch(true);
    heal(&input(&cli, &shim, None, Some(BARE_LOGIN_PATH))).expect("heal on");
    let runs = choice_for_script(switch_on(), &cli);
    assert_eq!(runs.target(&cli), Some(binary.as_path()));
    assert_eq!(link_target(&k2), binary);
    assert_eq!(
        link_state(&k2, runs.target(&cli).expect("a target")),
        LinkState::Current,
        "not stale"
    );
    let bin = InstalledCli { runs: CliKind::Binary, version: Some("0.46.11".to_string()) };
    assert_eq!(installed_cli(&k2, Some("0.46.11")), Some(bin.clone()));
    assert_eq!(installed_cli(&k5(h), None), Some(bin), "k5 too, the version from its mark");
    assert_eq!(
        serde_json::to_value(installed_cli(&k2, None).expect("installed")).expect("json"),
        serde_json::json!({"runs": "binary", "version": "0.46.11"}),
        "the shape the renderer reads"
    );
    // A link to the script left elsewhere is now the stale one.
    let left = h.join("usr-local-bin-k2");
    std::os::unix::fs::symlink(&cli, &left).unwrap();
    assert_eq!(
        link_state(&left, runs.target(&cli).expect("a target")),
        LinkState::Stale {
            target: cli.clone()
        }
    );

    // A binary of another release: its own version, so Settings offers the
    // update, and the heal goes back to the script.
    fs::write(&binary, binary_body("0.46.10")).unwrap();
    let old = InstalledCli { runs: CliKind::Binary, version: Some("0.46.10".to_string()) };
    assert_eq!(installed_cli(&k2, Some("0.46.11")), Some(old));

    // A pre-rename script reads its old marker; a GUI program named k2 is
    // not the K2 CLI at all.
    let legacy = h.join("legacy-k2");
    fs::write(&legacy, "#!/bin/bash\nK2SO_CLI_VERSION=\"0.39.0\"\n").unwrap();
    assert_eq!(installed_cli(&legacy, None), Some(InstalledCli { runs: CliKind::Script, version: Some("0.39.0".to_string()) }));
    let gui = h.join("usr-bin-k2");
    fs::write(&gui, b"\x7fELF\x02\x01\x01\x00 no mark").unwrap();
    assert_eq!(installed_cli(&gui, None), None);
}
