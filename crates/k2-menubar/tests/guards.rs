//! Source guards. Fail if a required file is missing. No skip.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_repo(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn arm_between<'a>(src: &'a str, start_pat: &str, end_pat: &str) -> &'a str {
    let start = src
        .find(start_pat)
        .unwrap_or_else(|| panic!("missing {start_pat}"));
    let rest = &src[start + start_pat.len()..];
    let end = rest
        .find(end_pat)
        .unwrap_or_else(|| panic!("missing {end_pat} after {start_pat}"));
    &rest[..end]
}

#[test]
fn running_check_does_not_read_token_files_or_status() {
    for rel in [
        "crates/k2-menubar/src/lib.rs",
        "crates/k2-menubar/src/main.rs",
    ] {
        let text = read_repo(rel);
        assert!(
            !text.contains("heartbeat.token"),
            "{rel} must not read heartbeat.token"
        );
        assert!(
            !text.contains("daemon.token"),
            "{rel} must not read daemon.token"
        );
        assert!(
            !text.contains("GET /status"),
            "{rel} must not request /status"
        );
        assert!(
            !text.contains("\"/status\""),
            "{rel} must not request /status"
        );
    }
    let lib = read_repo("crates/k2-menubar/src/lib.rs");
    assert!(lib.contains("GET /boot-status"));
    assert!(lib.contains("launchctl"));
    assert!(lib.contains("\"print\""));
    let quit = arm_between(&lib, "fn quit_daemon_plist", "fn current_uid");
    assert!(quit.contains("unload"));
    assert!(!quit.contains("process::exit"));
    assert!(!quit.contains("app.exit"));
    let main_src = read_repo("crates/k2-menubar/src/main.rs");
    let handler = arm_between(&main_src, "fn handle_quit_daemon", "fn main");
    assert!(!handler.contains("app.exit"));
    // Daemon first. Only when it stopped: unload the helper's own job,
    // then exit. A failed daemon quit returns before either.
    let quit_at = handler.find("quit_daemon()").expect("handler quits the daemon");
    let bail_at = handler.find("if !stopped").expect("failed quit returns early");
    let retire_at = handler.find("retire_helper()").expect("handler retires the helper");
    let exit_at = handler.find("process::exit(0)").expect("handler exits");
    assert!(quit_at < bail_at, "{handler}");
    assert!(bail_at < retire_at, "{handler}");
    assert!(retire_at < exit_at, "{handler}");
    assert_eq!(handler.matches("process::exit").count(), 1, "{handler}");
}

#[test]
fn helper_retire_unloads_only_its_own_label() {
    let lib = read_repo("crates/k2-menubar/src/lib.rs");
    let retire = arm_between(&lib, "pub fn retire_helper", "fn current_uid");
    assert!(retire.contains("helper_plist_path()"));
    assert!(!retire.contains("daemon_plist_path"));
    assert!(retire.contains("unload_launchctl_args"));
    // launchd ends this process (and its process group) on the unload;
    // launchctl must be in a group of its own to finish.
    assert!(retire.contains("process_group(0)"), "{retire}");
}

/// Opening K2 after a menu-bar Quit daemon brings back both the daemon and
/// the icon: both go through `ensure_loaded`, whose `load -w` undoes the
/// `unload -w`, and neither is gated on the daemon already running.
#[test]
fn app_open_loads_both_daemon_and_helper() {
    let src = read_repo("src-tauri/src/lib.rs");
    let setup = arm_between(&src, "startup_ensure_daemon_loaded", "Phase 2 Unit 4");
    let daemon_at = setup
        .find("wake::ensure_loaded(&plist)")
        .expect("daemon plist is loaded on open");
    assert!(setup[..daemon_at].contains("DaemonPlist::canonical"));
    let helper_at = setup
        .find("install_menu_bar_helper();")
        .expect("helper plist is loaded on open");
    assert!(daemon_at < helper_at);
    let helper = arm_between(&src, "fn install_menu_bar_helper", "pub fn run");
    assert!(helper.contains("DaemonPlist::menu_bar_helper"));
    assert!(helper.contains("wake::ensure_loaded(&plist)"));
    let wake = read_repo("crates/k2-client/src/wake.rs");
    let ensure = arm_between(&wake, "pub fn ensure_loaded", "pub fn uninstall");
    assert!(ensure.contains("launchctl_load(&target)"));
    let load = arm_between(&wake, "pub fn launchctl_load_args", "pub fn launchctl_load(");
    assert!(load.contains("\"-w\""), "{load}");
    // `k2 daemon start` loads the helper's existing plist too.
    let cli = read_repo("cli/src/daemon.sh");
    let start = arm_between(&cli, "cmd_daemon_start() {", "\n}\n");
    assert!(start.contains("dev.k2.menubar.plist"), "{start}");
    assert!(start.contains("launchctl load -w \"$helper_plist\""), "{start}");
}

#[test]
fn macos_setup_does_not_call_tray_install() {
    let src = read_repo("src-tauri/src/lib.rs");
    assert!(
        src.contains("mod tray;"),
        "tray.rs stays in the crate for Linux and Windows"
    );
    let call = "tray::install(";
    assert_eq!(
        src.matches(call).count(),
        1,
        "one install call, cfg'd off macOS"
    );
    let idx = src.find(call).unwrap();
    let cfg_at = src[..idx].rfind("#[cfg").expect("cfg before tray::install");
    let header = &src[cfg_at..idx];
    assert!(
        header.contains("not(target_os = \"macos\")"),
        "tray::install call must be non-mac only, header:\n{header}"
    );
    let tray = read_repo("src-tauri/src/tray.rs");
    assert!(tray.contains("k2so-main"));
    assert!(tray.contains("fn install"));
    let manifest = read_repo("src-tauri/Cargo.toml");
    assert!(manifest.contains("tray-icon"));
    assert!(manifest.contains("image-png"));
    assert!(manifest.contains("macos-private-api"));
    let tauri_conf = read_repo("src-tauri/tauri.conf.json");
    assert!(tauri_conf.contains("\"macOSPrivateApi\": true"));
}

#[test]
fn exit_requested_does_not_unload_but_red_close_does() {
    let src = read_repo("src-tauri/src/lib.rs");
    let exit_arm = arm_between(
        &src,
        "tauri::RunEvent::ExitRequested",
        "tauri::RunEvent::Exit =>",
    );
    assert!(
        !exit_arm.contains("launchctl_unload"),
        "Cmd+Q must not unload the daemon:\n{exit_arm}"
    );
    let close_arm = arm_between(
        &src,
        "WindowEvent::CloseRequested",
        "tauri::RunEvent::ExitRequested",
    );
    assert!(
        close_arm.contains("launchctl_unload"),
        "keep-daemon-off red close must still unload"
    );
    assert!(close_arm.contains("keep_running"));
}

#[test]
fn headless_installers_do_not_install_the_helper() {
    for rel in ["scripts/install-daemon.sh", "cli/k2"] {
        let text = read_repo(rel);
        assert!(!text.is_empty(), "{rel} empty");
        assert!(
            !text.contains("k2-menubar"),
            "{rel} must not install the helper"
        );
        assert!(
            !text.contains("install_menu_bar_helper"),
            "{rel} must not call the helper installer"
        );
    }
    let installer = read_repo("scripts/install-daemon.sh");
    assert!(
        !installer.contains("dev.k2.menubar"),
        "install-daemon.sh must not reference the helper label"
    );
    // The CLI names the label once: `k2 daemon start` re-loads a plist the
    // desktop app already wrote, behind an existence check. It never writes one.
    let cli = read_repo("cli/k2");
    assert_eq!(cli.matches("dev.k2.menubar").count(), 1, "one mention, in daemon start");
    let start = arm_between(&cli, "cmd_daemon_start() {", "\n}\n");
    assert!(start.contains("dev.k2.menubar.plist"));
    assert!(start.contains("if [ -f \"$helper_plist\" ]"), "{start}");
    for write in ["cat >", "cp ", "tee ", "> \"$helper_plist\""] {
        assert!(!start.contains(write), "daemon start must not write the helper plist: {write}");
    }
}

#[test]
fn helper_installer_is_the_gui_open_path() {
    let lib = read_repo("src-tauri/src/lib.rs");
    assert!(lib.contains("fn install_menu_bar_helper"));
    assert!(lib.contains("fn stage_bundled_menubar"));
    assert!(lib.contains("install_menu_bar_helper();"));
    let body = arm_between(&lib, "fn install_menu_bar_helper", "pub fn run");
    assert!(body.contains("is_transient_exe_location"));
    assert!(body.contains("menu_bar_helper"));
    assert!(
        !body.contains("Contents/MacOS"),
        "plist must point at the staged copy, not the app bundle"
    );
    let daemon = read_repo("src-tauri/src/commands/daemon.rs");
    assert!(
        daemon.contains("install_menu_bar_helper"),
        "Settings daemon_install must stage the helper too"
    );
    let daemon_crate = read_repo("crates/k2-daemon/Cargo.toml");
    for forbidden in ["objc2-app-kit", "tray-icon", "cocoa"] {
        assert!(
            !daemon_crate.contains(forbidden),
            "k2-daemon must not link AppKit via {forbidden}"
        );
    }
    let _ = Path::new("src-tauri/src/tray.rs");
}
