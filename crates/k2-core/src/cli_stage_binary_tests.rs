//! The Linux stage with the new k5 command (Beta): `k2` (and `k5`) become
//! links to the k5 binary and go back to the script copy, one rename at a
//! time. Every test works in its own temp folder; nothing here reaches
//! `/usr/local/bin` or `~/.local/bin`.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
#[cfg(not(k2_no_bash))]
use k2_client::cli_install::{begin_link_swap, is_k2_cli_script};

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("k2-cli-stage-bin-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A stand-in k5 binary beside a stand-in daemon: `<dir>/daemon/k5-cli`
/// with this release's marker.
fn put_binary(dir: &Path) -> PathBuf {
    let d = dir.join("daemon");
    fs::create_dir_all(&d).unwrap();
    let bin = d.join("k5-cli");
    let mut body = b"\x7fELF\x02\x01\x01\x00".to_vec();
    body.extend_from_slice(format!("K5_CLI_VERSION={}\0", embedded_version().expect("version")).as_bytes());
    fs::write(&bin, body).unwrap();
    bin
}

fn target_of(p: &Path) -> PathBuf {
    fs::read_link(p).unwrap_or_else(|e| panic!("{} must be a link: {e}", p.display()))
}

#[cfg(not(k2_no_bash))]
fn is_copy(p: &Path) -> bool {
    let m = fs::symlink_metadata(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    m.file_type().is_file() && Some(fs::read_to_string(p).unwrap().as_str()) == CLI_CONTENT
}

#[test]
fn the_binary_beside_the_daemon_passes_only_when_on_and_of_this_release() {
    let dir = Dir::new("choice");
    let bin = put_binary(&dir.0);
    let d = bin.parent().unwrap();
    let v = embedded_version();
    assert_eq!(k2_client::cli_install::binary_in_dir_if_on(true, d, v.as_deref()), Ok(bin.clone()));
    assert!(k2_client::cli_install::binary_in_dir_if_on(false, d, v.as_deref()).is_err());
    let err = k2_client::cli_install::binary_in_dir_if_on(true, d, Some("0.0.1")).unwrap_err();
    assert!(err.contains("0.0.1"), "{err}");
    let err = k2_client::cli_install::binary_in_dir_if_on(true, &dir.0, v.as_deref()).unwrap_err();
    assert!(err.contains("is missing"), "{err}");
}

#[test]
#[cfg(not(k2_no_bash))]
fn script_to_binary_and_back_for_k2_and_k5() {
    let dir = Dir::new("flip");
    let bin = put_binary(&dir.0);
    let local = dir.0.join("bin");
    let k2 = local.join("k2");
    let k5 = local.join("k5");

    // Today's: the copy, and no k5.
    assert_eq!(stage_cli_with_fallback_as(&k2, None, StageAs::Script).unwrap(), Some(k2.clone()));
    assert!(is_copy(&k2));
    assert!(fs::symlink_metadata(&k5).is_err(), "no k5 while the links run the script");

    // On: both link to the binary.
    assert_eq!(stage_cli_with_fallback_as(&k2, None, StageAs::Binary(&bin)).unwrap(), Some(k2.clone()));
    assert_eq!(target_of(&k2), bin);
    assert_eq!(target_of(&k5), bin);
    assert!(is_k2_cli_script(&k2), "the binary is the K2 CLI");
    // Again: nothing changes.
    assert_eq!(stage_cli_with_fallback_as(&k2, None, StageAs::Binary(&bin)).unwrap(), Some(k2.clone()));
    assert_eq!(target_of(&k2), bin);

    // Off: both back to the copy (k5 kept, never removed).
    assert_eq!(stage_cli_with_fallback_as(&k2, None, StageAs::Script).unwrap(), Some(k2.clone()));
    assert!(is_copy(&k2));
    assert!(is_copy(&k5));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&k2).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(fs::metadata(&k5).unwrap().permissions().mode() & 0o777, 0o755);
    }
    assert_eq!(&fs::read(&bin).unwrap()[..4], b"\x7fELF", "the binary itself is never written");
}

#[test]
#[cfg(not(k2_no_bash))]
fn turning_off_reaches_a_link_left_at_the_other_place() {
    let dir = Dir::new("other");
    let bin = put_binary(&dir.0);
    // The primary can't be written (its folder is a file): the binary goes
    // to the fallback.
    let blocker = dir.0.join("ro-bin");
    fs::write(&blocker, b"not a directory").unwrap();
    let primary = blocker.join("k2");
    let fallback = dir.0.join("home-local-bin").join("k2");
    let out = stage_cli_with_fallback_as(&primary, Some(&fallback), StageAs::Binary(&bin)).unwrap();
    assert_eq!(out, Some(fallback.clone()));
    assert_eq!(target_of(&fallback), bin);

    // Later the primary is writable and holds today's copy; off, the stage
    // lands there and still puts the fallback's links back to the copy.
    let primary2 = dir.0.join("usr-local-bin").join("k2");
    stage_cli_at(&primary2).unwrap();
    let out = stage_cli_with_fallback_as(&primary2, Some(&fallback), StageAs::Script).unwrap();
    assert_eq!(out, Some(primary2.clone()));
    assert!(is_copy(&fallback), "the binary's link at the other place goes back too");
    assert!(is_copy(&fallback.with_file_name("k5")));
    assert_eq!(fs::read(&blocker).unwrap(), b"not a directory");
}

#[test]
fn other_links_and_files_are_never_touched_in_either_mode() {
    let dir = Dir::new("foreign");
    let bin = put_binary(&dir.0);
    let local = dir.0.join("bin");
    fs::create_dir_all(&local).unwrap();
    let k2 = local.join("k2");
    // A developer's link at a checkout: left as it is, and no k5 is made.
    let checkout = dir.0.join("repo-cli-k2");
    fs::write(&checkout, "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\n").unwrap();
    std::os::unix::fs::symlink(&checkout, &k2).unwrap();
    for as_ in [StageAs::Binary(&bin), StageAs::Script] {
        assert_eq!(stage_cli_with_fallback_as(&k2, None, as_).unwrap(), None);
        assert_eq!(target_of(&k2), checkout);
        assert!(fs::symlink_metadata(local.join("k5")).is_err());
    }
    // Another program's k5 beside K2's k2: left alone.
    fs::remove_file(&k2).unwrap();
    let other = b"#!/bin/sh\necho another k5\n";
    fs::write(local.join("k5"), other).unwrap();
    stage_cli_with_fallback_as(&k2, None, StageAs::Binary(&bin)).unwrap();
    assert_eq!(target_of(&k2), bin);
    assert_eq!(fs::read(local.join("k5")).unwrap(), other);
}

#[test]
#[cfg(not(k2_no_bash))]
fn an_interrupted_swap_leaves_a_working_command() {
    let dir = Dir::new("interrupt");
    let bin = put_binary(&dir.0);
    let local = dir.0.join("bin");
    let k2 = local.join("k2");
    stage_cli_at(&k2).unwrap();

    // Stopped after the temp link, before the rename: k2 is still the copy.
    let tmp = begin_link_swap(&bin, &k2).unwrap();
    assert!(is_copy(&k2));
    assert!(is_k2_cli_script(&tmp));
    let dead = local.join(".k2.k2-link-2147483647");
    fs::rename(&tmp, &dead).unwrap();

    // Stopped between k2 and k5: k2 on the binary, k5 not made yet; both
    // names that exist work. The next stage finishes and sweeps the temp.
    assert_eq!(stage_at(&k2, StageAs::Binary(&bin), true).unwrap(), Some(k2.clone()));
    assert!(fs::symlink_metadata(&dead).is_err(), "the dead process's temp is swept");
    assert!(fs::symlink_metadata(local.join("k5")).is_err());
    stage_cli_with_fallback_as(&k2, None, StageAs::Binary(&bin)).unwrap();
    assert_eq!(target_of(&local.join("k5")), bin);

    // Off, stopped between the two: k2 the copy, k5 still the binary.
    assert_eq!(stage_at(&k2, StageAs::Script, true).unwrap(), Some(k2.clone()));
    assert!(is_copy(&k2) && is_k2_cli_script(&local.join("k5")));
    stage_cli_with_fallback_as(&k2, None, StageAs::Script).unwrap();
    assert!(is_copy(&local.join("k5")));
}

#[test]
fn the_switch_is_off_by_default() {
    let _home = crate::test_env::TempHome::new();
    assert!(!k2_client::cli_install::switch_on());
    let got = current_choice();
    if NO_BASH {
        // On by build, and never the script: the test binary's folder has
        // no k5 program, so nothing, and why.
        match &got {
            Choice::None { note } => assert!(note.contains("is missing"), "{note}"),
            other => panic!("a build with no shell CLI and no binary: {other:?}"),
        }
    } else {
        assert_eq!(got, Choice::Script { note: None });
    }
}

/// A build with no shell CLI over what an earlier build left: the script
/// copy at `k2` becomes a link to the binary and `k5` joins it, through
/// the same stage entry the boot uses; no script is written.
#[test]
fn no_bash_the_binary_replaces_a_script_copy_left_before() {
    let dir = Dir::new("over-copy");
    let bin = put_binary(&dir.0);
    let local = dir.0.join("bin");
    fs::create_dir_all(&local).unwrap();
    let k2 = local.join("k2");
    fs::write(&k2, "#!/bin/bash\nK2_CLI_VERSION=\"0.0.1\"\n").unwrap();
    let runs = Choice::Binary {
        binary: bin.clone(),
    };
    assert_eq!(
        stage_choice_with_fallback(&k2, None, &runs).unwrap(),
        Some(k2.clone())
    );
    assert_eq!(target_of(&k2), bin);
    assert_eq!(target_of(&local.join("k5")), bin);
    // Again: nothing changes.
    assert_eq!(
        stage_choice_with_fallback(&k2, None, &runs).unwrap(),
        Some(k2.clone())
    );
    assert_eq!(target_of(&k2), bin);
    let names: Vec<_> = fs::read_dir(&local)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "k2 and k5 only, no temp left: {names:?}");
}
