//! The home override (`paths::home_override`): with it set, the K2 home the
//! daemon builds its paths from is that folder, not `$HOME/.k2`.

use crate::test_env::{EnvVar, TempHome};

/// Paths the daemon builds under its home, from several subsystems.
fn daemon_home_paths() -> Vec<std::path::PathBuf> {
    vec![
        crate::paths::k2_home(),
        crate::paths::k2_home_opt().expect("a home"),
        crate::tunnel::subdomains::persisted_path(),
        crate::tunnel::unpair::tombstone_path(),
        crate::tunnel::connector::frpc_log_path(),
        crate::edge_attest::override_path(),
        crate::federation::identity::identity_dir(),
        crate::themes::themes_dir(),
    ]
}

#[test]
fn the_daemons_home_follows_the_override() {
    let home = TempHome::new();
    let _k5 = EnvVar::remove("K5_HOME_OVERRIDE");
    let _k2 = EnvVar::remove("K2_HOME_OVERRIDE");

    // Neither name set: the documented default, $HOME/.k2.
    let default = crate::paths::k2_home_in(home.path());
    for p in daemon_home_paths() {
        assert!(p.starts_with(&default), "{p:?} is not under {default:?}");
    }

    let rig = home.path().join("rig").join("k2-home");
    let _set = EnvVar::set("K2_HOME_OVERRIDE", &rig);
    assert_eq!(crate::paths::k2_home(), rig);
    for p in daemon_home_paths() {
        assert!(p.starts_with(&rig), "{p:?} does not follow the override {rig:?}");
    }
    // A home handed in by the caller never follows it.
    assert_eq!(crate::paths::k2_home_in(home.path()), default);

    // The K5_ name is read first.
    let k5_rig = home.path().join("k5-rig");
    let _new = EnvVar::set("K5_HOME_OVERRIDE", &k5_rig);
    assert_eq!(crate::paths::k2_home(), k5_rig, "K5_HOME_OVERRIDE wins over K2_HOME_OVERRIDE");
}

#[test]
#[should_panic(expected = "K2_HOME_OVERRIDE=\"relative/home\" is not valid: the K2 home override must be an absolute path")]
fn a_relative_override_fails_loudly() {
    let _home = TempHome::new();
    let _k5 = EnvVar::remove("K5_HOME_OVERRIDE");
    let _k2 = EnvVar::set("K2_HOME_OVERRIDE", "relative/home");
    let _ = crate::paths::k2_home();
}

#[test]
fn with_the_override_the_home_migration_touches_nothing_under_home() {
    let home = TempHome::new();
    let _k5 = EnvVar::remove("K5_HOME_OVERRIDE");
    let legacy = home.path().join(".k2so");
    std::fs::create_dir_all(&legacy).expect("legacy home");
    std::fs::remove_dir_all(crate::paths::k2_home_in(home.path())).expect("no .k2 yet");
    let _k2 = EnvVar::set("K2_HOME_OVERRIDE", home.path().join("rig"));
    assert_eq!(crate::migration_home::migrate_home_dir(), Ok(crate::migration_home::HomeMigration::AlreadyDone));
    assert!(legacy.is_dir() && !legacy.is_symlink(), "the legacy folder was not moved");
    assert!(!crate::paths::k2_home_in(home.path()).exists(), "no ~/.k2 was made");
}
