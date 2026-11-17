//! The Linux app's copy of the script beside a link to the k5 binary: the
//! heal reads the link as installed, and the copy never writes through a
//! link. Every test works in its own temp folder.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use super::*;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("k2-cli-copy-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).expect("create temp dir");
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SCRIPT: &str = "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\necho bundled\n";

/// The bundled script, at `<dir>/bundle/cli/k2`.
fn put_script(dir: &Path) -> PathBuf {
    let cli = dir.join("bundle").join("cli");
    fs::create_dir_all(&cli).expect("create bundle");
    let s = cli.join("k2");
    fs::write(&s, SCRIPT).expect("write script");
    s
}

/// A stand-in k5 binary beside a stand-in daemon: `<dir>/daemon/k5-cli`.
fn put_binary(dir: &Path) -> (PathBuf, Vec<u8>) {
    let d = dir.join("daemon");
    fs::create_dir_all(&d).expect("create daemon dir");
    let bin = d.join("k5-cli");
    let mut body = b"\x7fELF\x02\x01\x01\x00".to_vec();
    body.extend_from_slice(b"K5_CLI_VERSION=9.9.9\0 the rest of the program");
    fs::write(&bin, &body).expect("write binary");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod binary");
    (bin, body)
}

fn target_of(p: &Path) -> PathBuf {
    fs::read_link(p).unwrap_or_else(|e| panic!("{} must be a link: {e}", p.display()))
}

#[test]
fn search_order_matches_the_login_path() {
    let home = PathBuf::from("/home/z");
    assert_eq!(
        search_paths(Some(&home)),
        vec![PathBuf::from("/usr/local/bin/k2"), PathBuf::from("/usr/bin/k2"), home.join(".local/bin/k2")]
    );
    assert_eq!(search_paths(None).len(), 2);
    assert_eq!(search_paths(Some(Path::new(""))).len(), 2);
    assert!(is_distro_package(Path::new("/usr/bin/k2")));
    assert!(!is_distro_package(Path::new("/usr/local/bin/k2")));
    assert!(!is_distro_package(Path::new("/home/z/.local/bin/k2")));
}

#[test]
fn a_gui_binary_named_k2_is_not_the_cli_but_the_k5_binary_is() {
    // 0.46.11: the .deb installs the GUI at /usr/bin/k2; it must not read as
    // an installed CLI. The k5 binary (its version mark) does.
    let dir = Dir::new("first");
    let gui = dir.0.join("usr-bin-k2");
    fs::write(&gui, b"\x7fELF\x02\x01\x01\x00").expect("write gui");
    let no_version = dir.0.join("other-k2");
    fs::write(&no_version, "#!/bin/sh\necho hi\n").expect("write other");
    let script = put_script(&dir.0);
    let (bin, _) = put_binary(&dir.0);
    let link = dir.0.join("link-k2");
    symlink(&bin, &link).expect("link to the binary");

    assert_eq!(first_k2_cli(&[gui.clone(), no_version.clone(), script.clone()]), Some(script));
    assert_eq!(first_k2_cli(&[gui.clone(), no_version.clone(), link.clone()]), Some(link));
    assert_eq!(first_k2_cli(&[gui, no_version]), None);
}

/// K2's ruling (slice 10c(iii)): with the switch on, `~/.local/bin/k2` is a
/// link to the binary. The heal reads it as installed, and the copy step,
/// run on it anyway, leaves it: the binary's bytes are unchanged and the
/// link still points at it.
#[test]
fn a_link_to_the_binary_is_never_overwritten() {
    let dir = Dir::new("binary-link");
    let script = put_script(&dir.0);
    let (bin, body) = put_binary(&dir.0);
    let bin_dir = dir.0.join("home").join(".local").join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin");
    let k2 = bin_dir.join("k2");
    symlink(&bin, &k2).expect("link k2");
    let alias = bin_dir.join("k2so");
    symlink(&bin, &alias).expect("link k2so");
    let search = vec![dir.0.join("no-usr-local-k2"), dir.0.join("no-usr-k2"), k2.clone()];

    assert_eq!(heal_plan(&search, &k2), HealPlan::Installed(k2.clone()));

    for link in [&k2, &alias] {
        match copy_script(&script, link).expect("copy step") {
            CopyOutcome::Left { why } => assert!(why.contains("a link to the"), "{why}"),
            other => panic!("the copy step wrote over {}: {other:?}", link.display()),
        }
        assert_eq!(target_of(link), bin, "{} still points at the binary", link.display());
    }
    assert_eq!(fs::read(&bin).expect("read binary"), body, "the binary's bytes are unchanged");
    let left: Vec<_> = fs::read_dir(&bin_dir).expect("list bin").flatten().map(|e| e.file_name()).collect();
    assert_eq!(left.len(), 2, "no temp file is left beside the links: {left:?}");
}

/// A link to a binary that is gone (an old bundle, a removed package) is
/// K2's: the link itself is replaced by the copy. The old place is never
/// written: before, the copy opened the link and made a file there.
#[test]
fn a_link_to_a_binary_that_is_gone_is_replaced_never_written_through() {
    let dir = Dir::new("dangling");
    let script = put_script(&dir.0);
    // The old folder is still there, so a copy through the link would
    // make a file in it.
    let gone = dir.0.join("old-daemon").join("k5-cli");
    fs::create_dir_all(gone.parent().expect("parent")).expect("create old folder");
    let k2 = dir.0.join("bin").join("k2");
    fs::create_dir_all(k2.parent().expect("parent")).expect("create bin");
    symlink(&gone, &k2).expect("dangling link");

    assert_eq!(heal_plan(std::slice::from_ref(&k2), &k2), HealPlan::Copy(k2.clone()));
    assert_eq!(copy_script(&script, &k2).expect("copy"), CopyOutcome::LinkReplaced { was: gone.clone() });
    let meta = fs::symlink_metadata(&k2).expect("stat k2");
    assert!(meta.file_type().is_file(), "k2 is now the copy, not a link");
    assert_eq!(fs::read_to_string(&k2).expect("read k2"), SCRIPT);
    assert_eq!(meta.permissions().mode() & 0o777, 0o755);
    assert!(!gone.exists() && !gone.is_symlink(), "nothing was written at the old place");
}

#[test]
fn a_link_that_is_not_k2_s_is_left_and_its_target_untouched() {
    let dir = Dir::new("foreign-link");
    let script = put_script(&dir.0);
    let other = dir.0.join("opt").join("tool");
    fs::create_dir_all(other.parent().expect("parent")).expect("create opt");
    fs::write(&other, b"\x7fELF someone else's program").expect("write other");
    let k2 = dir.0.join("k2");
    symlink(&other, &k2).expect("link");
    match copy_script(&script, &k2).expect("copy") {
        CopyOutcome::Left { why } => assert!(why.contains("not K2's"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(target_of(&k2), other);
    assert_eq!(fs::read(&other).expect("read other"), b"\x7fELF someone else's program");

    // A dangling link to something that isn't K2's is left too.
    let gone = dir.0.join("k2-gone");
    symlink(dir.0.join("nowhere").join("tool"), &gone).expect("dangling");
    assert!(matches!(copy_script(&script, &gone).expect("copy"), CopyOutcome::Left { .. }));
    assert!(fs::symlink_metadata(&gone).expect("stat").file_type().is_symlink());
}

#[test]
fn a_copy_refreshes_an_old_script_and_leaves_other_files() {
    let dir = Dir::new("files");
    let script = put_script(&dir.0);
    let dest = dir.0.join("bin").join("k2");
    // Nothing there: written, executable.
    assert_eq!(copy_script(&script, &dest).expect("copy"), CopyOutcome::Written);
    assert_eq!(fs::read_to_string(&dest).expect("read"), SCRIPT);
    assert_eq!(fs::metadata(&dest).expect("stat").permissions().mode() & 0o111, 0o111);
    // An older script: refreshed.
    fs::write(&dest, "#!/bin/bash\nK2_CLI_VERSION=\"0.0.1\"\n").expect("old");
    assert!(copy_script(&script, &dest).expect("copy").is_written());
    assert_eq!(fs::read_to_string(&dest).expect("read"), SCRIPT);
    // An empty file (a write that died): K2's to fill.
    fs::write(&dest, b"").expect("empty");
    assert!(copy_script(&script, &dest).expect("copy").is_written());
    // Another program named k2: left byte for byte.
    for other in [&b"\x7fELF\x02\x01\x01\x00"[..], b"#!/bin/sh\necho some other k2\n"] {
        fs::write(&dest, other).expect("other");
        assert!(matches!(copy_script(&script, &dest).expect("copy"), CopyOutcome::Left { .. }));
        assert_eq!(fs::read(&dest).expect("read"), other);
    }
    // A folder: left.
    let folder = dir.0.join("k2-folder");
    fs::create_dir_all(&folder).expect("folder");
    assert_eq!(copy_script(&script, &folder).expect("copy"), CopyOutcome::Left { why: "a folder".to_string() });
}

#[test]
fn a_live_link_to_the_script_is_kept() {
    // A developer's link at a checkout's cli/k2 runs the K2 CLI: kept.
    let dir = Dir::new("script-link");
    let script = put_script(&dir.0);
    let k2 = dir.0.join("k2");
    symlink(&script, &k2).expect("link");
    assert_eq!(heal_plan(std::slice::from_ref(&k2), &k2), HealPlan::Installed(k2.clone()));
    assert!(matches!(copy_script(&script, &k2).expect("copy"), CopyOutcome::Left { .. }));
    assert_eq!(target_of(&k2), script);
}
