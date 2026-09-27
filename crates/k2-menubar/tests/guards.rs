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
    assert!(!handler.contains("process::exit"));
    assert!(!handler.contains("app.exit"));
    assert!(handler.contains("quit_daemon"));
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
            !text.contains("dev.k2.menubar"),
            "{rel} must not reference the helper label"
        );
        assert!(
            !text.contains("install_menu_bar_helper"),
            "{rel} must not call the helper installer"
        );
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
