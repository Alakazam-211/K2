//! 0.43.2: on macOS the daemon runs from a nested helper app,
//! `K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon`, so
//! Activity Monitor shows the K2 icon and "K2 Daemon".
//!
//! These tests pin the pure path logic in `k2_core::daemon_lifecycle` and
//! `k2_core::tunnel::lease` that every caller relies on: where the app
//! looks for the daemon, where the daemon looks for its sidecars (`frpc`,
//! `k2-power-helper`, the `k2` app binary), and how an existing LaunchAgent
//! plist that still points at `K2.app/Contents/MacOS/k2-daemon` migrates.
//! They live in k2-daemon (not k2-core) so they run with the daemon suite.

use std::path::{Path, PathBuf};

use k2_core::daemon_lifecycle as dl;

const APP_EXE: &str = "/Applications/K2.app/Contents/MacOS/k2";
const NESTED_DAEMON: &str =
    "/Applications/K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon";
const LEGACY_DAEMON: &str = "/Applications/K2.app/Contents/MacOS/k2-daemon";
const HOST_MACOS: &str = "/Applications/K2.app/Contents/MacOS";

#[test]
fn helper_identity_constants() {
    assert_eq!(dl::DAEMON_HELPER_APP_NAME, "K2 Daemon.app");
    assert_eq!(dl::DAEMON_BINARY_NAME, "k2-daemon");
    assert_eq!(dl::DAEMON_HELPER_BUNDLE_ID, "dev.k2.app.daemon");
    // The bundle id must not collide with the launchd label.
    assert_ne!(dl::DAEMON_HELPER_BUNDLE_ID, dl::DAEMON_LAUNCH_AGENT_LABEL);
}

#[test]
fn helper_bundle_id_matches_build_script() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/macos-daemon-helper-app.sh");
    let body = std::fs::read_to_string(&script)
        .unwrap_or_else(|e| panic!("read {}: {e}", script.display()));
    assert!(
        body.contains(&format!("K2_DAEMON_HELPER_BUNDLE_ID=\"{}\"", dl::DAEMON_HELPER_BUNDLE_ID)),
        "scripts/macos-daemon-helper-app.sh bundle id drifted from DAEMON_HELPER_BUNDLE_ID"
    );
    assert!(
        body.contains(&format!("K2_DAEMON_HELPER_APP=\"{}\"", dl::DAEMON_HELPER_APP_NAME)),
        "scripts/macos-daemon-helper-app.sh app name drifted from DAEMON_HELPER_APP_NAME"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn app_resolves_nested_daemon_on_macos() {
    assert_eq!(
        dl::bundled_daemon_path(Path::new(APP_EXE)),
        Some(PathBuf::from(NESTED_DAEMON))
    );
}

#[test]
fn bare_dev_build_keeps_sibling_daemon() {
    #[cfg(not(windows))]
    assert_eq!(
        dl::bundled_daemon_path(Path::new("/Users/x/dev/K2/target/debug/k2")),
        Some(PathBuf::from("/Users/x/dev/K2/target/debug/k2-daemon"))
    );
}

#[test]
fn app_contents_detection() {
    assert_eq!(
        dl::app_contents_of_bundle_exe(Path::new(APP_EXE)),
        Some(Path::new("/Applications/K2.app/Contents"))
    );
    assert_eq!(dl::app_contents_of_bundle_exe(Path::new("/usr/local/bin/k2-daemon")), None);
    assert_eq!(
        dl::app_contents_of_bundle_exe(Path::new("/opt/Contents/MacOS/k2")),
        None,
        "a Contents/MacOS that is not inside a .app is not a bundle"
    );
}

#[test]
fn nested_daemon_finds_sidecars_in_host_app() {
    assert_eq!(
        dl::bundle_sidecar_dir(Path::new(NESTED_DAEMON)),
        Some(PathBuf::from(HOST_MACOS))
    );
    // The app binary itself, a dev build, and a Linux/standalone daemon
    // keep exe.parent().
    assert_eq!(dl::bundle_sidecar_dir(Path::new(APP_EXE)), Some(PathBuf::from(HOST_MACOS)));
    assert_eq!(
        dl::bundle_sidecar_dir(Path::new("/Users/x/dev/K2/target/release/k2-daemon")),
        Some(PathBuf::from("/Users/x/dev/K2/target/release"))
    );
    assert_eq!(
        dl::bundle_sidecar_dir(Path::new("/usr/bin/k2-daemon")),
        Some(PathBuf::from("/usr/bin"))
    );
    // A .app nested somewhere other than Contents/Helpers is not ours.
    assert_eq!(
        dl::bundle_sidecar_dir(Path::new(
            "/Applications/K2.app/Contents/Resources/X.app/Contents/MacOS/k2-daemon"
        )),
        Some(PathBuf::from("/Applications/K2.app/Contents/Resources/X.app/Contents/MacOS"))
    );
}

/// Real files on disk: the daemon in the nested helper finds `frpc` and
/// `k2-power-helper` in the host app, the way `resolve_frpc` and
/// `install_wake_helper` join them.
#[test]
fn sidecar_lookup_on_a_real_bundle_tree() {
    let root = std::env::temp_dir().join(format!(
        "k2-helper-layout-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let host_macos = root.join("K2.app/Contents/MacOS");
    let daemon = dl::nested_daemon_path(&root.join("K2.app/Contents"));
    std::fs::create_dir_all(&host_macos).expect("mkdir host MacOS");
    std::fs::create_dir_all(daemon.parent().expect("daemon dir")).expect("mkdir helper MacOS");
    for f in ["k2", "frpc", "k2-power-helper"] {
        std::fs::write(host_macos.join(f), b"x").expect("write sidecar");
    }
    std::fs::write(&daemon, b"x").expect("write daemon");

    let dir = dl::bundle_sidecar_dir(&daemon).expect("sidecar dir");
    assert_eq!(dir, host_macos);
    assert!(dir.join("frpc").is_file(), "frpc not found from nested daemon");
    assert!(dir.join("k2-power-helper").is_file(), "power helper not found from nested daemon");
    // Without the fix, the helper's own MacOS dir has no sidecars.
    assert!(!daemon.parent().unwrap().join("frpc").exists());

    #[cfg(target_os = "macos")]
    assert_eq!(
        dl::bundled_daemon_path(&host_macos.join("k2")).as_deref(),
        Some(daemon.as_path())
    );

    std::fs::remove_dir_all(&root).expect("cleanup");
}

#[cfg(target_os = "macos")]
#[test]
fn keychain_peers_match_from_app_and_daemon() {
    let from_daemon = k2_core::tunnel::lease::bundle_peer_binaries(Path::new(NESTED_DAEMON));
    let from_app = k2_core::tunnel::lease::bundle_peer_binaries(Path::new(APP_EXE));
    let want = vec![PathBuf::from(APP_EXE), PathBuf::from(NESTED_DAEMON)];
    assert_eq!(from_daemon, want);
    assert_eq!(from_app, want);
}

#[test]
fn legacy_path_of_nested_daemon() {
    assert_eq!(
        dl::legacy_bundled_daemon_path(Path::new(NESTED_DAEMON)),
        Some(PathBuf::from(LEGACY_DAEMON))
    );
    assert_eq!(
        dl::legacy_bundled_daemon_path(Path::new("/Users/x/dev/K2/target/release/k2-daemon")),
        None
    );
    assert_eq!(dl::legacy_bundled_daemon_path(Path::new(LEGACY_DAEMON)), None);
}

// ── plist migration (heal_daemon_plist_program decision) ───────────────

#[test]
fn plist_on_legacy_path_migrates_even_if_file_still_exists() {
    // `ditto` over an old install leaves Contents/MacOS/k2-daemon behind.
    assert!(dl::should_rewrite_plist(
        Path::new(LEGACY_DAEMON),
        Path::new(NESTED_DAEMON),
        /* recorded_exists */ true,
        /* current_is_transient */ false,
    ));
}

#[test]
fn plist_on_legacy_path_migrates_after_full_replace() {
    // The app updater replaced the whole bundle: old path is gone.
    assert!(dl::should_rewrite_plist(
        Path::new(LEGACY_DAEMON),
        Path::new(NESTED_DAEMON),
        false,
        false,
    ));
}

#[test]
fn plist_already_on_nested_path_is_left_alone() {
    assert!(!dl::should_rewrite_plist(
        Path::new(NESTED_DAEMON),
        Path::new(NESTED_DAEMON),
        true,
        false,
    ));
}

#[test]
fn plist_on_other_apps_legacy_path_is_left_alone() {
    // A different, still-present install (~/Applications) is not this
    // app's legacy path; same "don't churn a stable path" rule as dev boxes.
    assert!(!dl::should_rewrite_plist(
        Path::new("/Users/x/Applications/K2.app/Contents/MacOS/k2-daemon"),
        Path::new(NESTED_DAEMON),
        true,
        false,
    ));
}

#[test]
fn plist_on_dev_path_is_left_alone() {
    assert!(!dl::should_rewrite_plist(
        Path::new("/Users/x/dev/K2/target/release/k2-daemon"),
        Path::new(NESTED_DAEMON),
        true,
        false,
    ));
}

#[test]
fn plist_never_migrates_from_transient_app() {
    assert!(!dl::should_rewrite_plist(
        Path::new("/Volumes/K2/K2.app/Contents/MacOS/k2-daemon"),
        Path::new("/Volumes/K2/K2.app/Contents/Helpers/K2 Daemon.app/Contents/MacOS/k2-daemon"),
        true,
        true,
    ));
}

#[test]
fn plist_program_with_space_round_trips() {
    let xml = dl::generate_plist_content(PathBuf::from(NESTED_DAEMON));
    assert!(
        xml.contains(&format!("<string>{NESTED_DAEMON}</string>")),
        "plist must carry the nested path verbatim: {xml}"
    );
    assert_eq!(dl::parse_plist_program(&xml), Some(PathBuf::from(NESTED_DAEMON)));
}
