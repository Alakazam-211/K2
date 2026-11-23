//! An API key's hire through converge (`POST /v1/w`): a template hire and a
//! plain one. The folder is made the way an agent's is (no links below the
//! key's root, made in one step) or opened through a walk that refuses
//! links, and a folder another user could change is never taken on. A test
//! hook changes the filesystem between the checks and the work.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use super::test_hook::{self, Stage};
use super::*;

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

/// A temp HOME (only its owner can write it), the shared test DB, a key's
/// root inside the home and a folder outside the root.
struct Setup {
    _home: crate::test_env::TempHome,
    root: PathBuf,
    outside: PathBuf,
}

fn setup() -> Setup {
    let home = crate::test_env::TempHome::new();
    crate::db::init_for_tests();
    chmod(home.path(), 0o700);
    let base = dunce::canonicalize(home.path()).expect("canonical home");
    let root = base.join(uniq("hire-root"));
    fs::create_dir_all(&root).expect("mkdir root");
    chmod(&root, 0o700);
    let outside = base.join(uniq("outside"));
    fs::create_dir_all(&outside).expect("mkdir outside");
    Setup { _home: home, root, outside }
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap_or_else(|e| panic!("chmod {}: {e}", p.display()));
}

fn mode_of(p: &Path) -> u32 {
    fs::symlink_metadata(p).expect("stat").permissions().mode() & 0o7777
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A key's hire, as `/v1/w` makes it: the key's root, `new_only` for a
/// template hire.
fn key_hire(root: &Path, path: &Path, template: bool) -> Result<ConvergeOutcome, HireError> {
    let body = if template {
        serde_json::json!({ "path": s(path), "template": "developer" })
    } else {
        serde_json::json!({ "path": s(path) })
    };
    let req: ConvergeRequest = serde_json::from_value(body).expect("a converge body");
    let caller = HireCaller::ApiKey { id: uniq("key"), label: None };
    let auth = ConvergeAuthority {
        new_workspace_root: Some(root.to_path_buf()),
        may_create: true,
        max_db: Some("off"),
        create_refusal: None,
        create_only: false,
        new_only: template,
    };
    let out = converge(&req, &caller, &auth);
    test_hook::set(None);
    out
}

/// At `stage`, put a link to `target` where `at` is (a folder there goes).
fn link_at(stage: Stage, at: PathBuf, target: PathBuf) {
    test_hook::set(Some(Box::new(move |st, _| {
        if st != stage || fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return;
        }
        if at.is_dir() {
            fs::remove_dir(&at).expect("remove the folder");
        }
        symlink(&target, &at).expect("symlink");
    })));
}

fn registered(p: &Path) -> bool {
    lookup_project_row(&s(p)).is_some()
}

fn entries(p: &Path) -> usize {
    fs::read_dir(p).expect("read_dir").count()
}

fn assert_left_alone(st: &Setup, path: &Path, what: &str) {
    assert!(!registered(path), "{what}: {} is not registered", path.display());
    assert_eq!(entries(&st.outside), 0, "{what}: nothing was made in the folder outside the root");
}

#[test]
fn a_key_template_hire_refuses_a_link_in_place_of_a_parent_folder() {
    let st = setup();
    let path = st.root.join("a").join("b");
    link_at(Stage::AfterChecks, st.root.join("a"), st.outside.clone());
    let e = key_hire(&st.root, &path, true).expect_err("a link in place of a parent is refused");
    assert_eq!(e.status, 403, "{e}");
    assert_left_alone(&st, &path, "parent");
    assert!(!registered(&st.outside.join("b")));
}

#[test]
fn a_key_template_hire_refuses_a_link_in_place_of_its_empty_folder() {
    let st = setup();
    let path = st.root.join("c");
    fs::create_dir(&path).expect("mkdir");
    link_at(Stage::AfterChecks, path.clone(), st.outside.clone());
    let e = key_hire(&st.root, &path, true).expect_err("a link in place of the folder is refused");
    assert_eq!(e.status, 403, "{e}");
    assert_left_alone(&st, &path, "final folder");
    assert!(!registered(&st.outside));
}

#[test]
fn a_key_plain_hire_refuses_a_link_where_its_new_folder_goes() {
    let st = setup();
    let path = st.root.join("d");
    link_at(Stage::AfterChecks, path.clone(), st.outside.clone());
    let e = key_hire(&st.root, &path, false).expect_err("a link where the new folder goes is refused");
    assert_eq!(e.status, 403, "{e}");
    assert_left_alone(&st, &path, "new folder");
    assert!(!registered(&st.outside));
}

/// The folder is opened by a walk from the root that refuses links, right
/// before it is registered: a link put there after every check is refused
/// too, for a folder that was there and for one the hire made.
#[test]
fn a_key_hire_registers_only_the_folder_its_walk_opened() {
    let st = setup();
    for (tag, template, premade) in [("e1", true, true), ("e2", false, true), ("e3", false, false), ("e4", true, false)] {
        let path = st.root.join(tag);
        if premade {
            fs::create_dir(&path).expect("mkdir");
        }
        link_at(Stage::BeforeRegister, path.clone(), st.outside.clone());
        let e = key_hire(&st.root, &path, template).expect_err("a link put in before the register is refused");
        assert_eq!(e.status, 403, "{tag}: {e}");
        assert_left_alone(&st, &path, tag);
        assert!(!registered(&st.outside), "{tag}");
    }
}

#[test]
fn a_key_plain_hire_refuses_a_folder_others_can_write() {
    let st = setup();
    for (tag, mode) in [("group", 0o775), ("world", 0o777), ("sticky", 0o1777)] {
        let path = st.root.join(tag);
        fs::create_dir(&path).expect("mkdir");
        fs::write(path.join("notes.txt"), b"theirs").expect("write");
        chmod(&path, mode);
        let e = key_hire(&st.root, &path, false).expect_err("a folder others can write is not taken on");
        assert_eq!((e.status, e.code), (403, "hire_root_unsafe"), "{tag}: {e}");
        assert!(e.hint.contains("can be changed by another user") && e.hint.contains("chmod go-w"), "{tag}: {e}");
        assert!(!registered(&path), "{tag}: not registered");
        assert_eq!(mode_of(&path), mode, "{tag}: its mode is as it was");
        assert_eq!(entries(&path), 1, "{tag}: nothing written into it");
    }
}

/// An owner who made the folder first: a plain key hire still takes it on,
/// files and all, when only its owner can write it.
#[test]
fn a_key_plain_hire_takes_on_a_folder_only_its_owner_can_write() {
    let st = setup();
    let path = st.root.join("prepared");
    fs::create_dir(&path).expect("mkdir");
    chmod(&path, 0o755);
    fs::write(path.join("notes.txt"), b"mine").expect("write");
    let out = key_hire(&st.root, &path, false).expect("a safe prepared folder is taken on");
    assert!(out.created, "registered by this hire");
    assert!(registered(&path));
    assert_eq!(fs::read(path.join("notes.txt")).expect("read"), b"mine");
    // A template hire still takes on an empty one.
    let empty = st.root.join("empty");
    fs::create_dir(&empty).expect("mkdir");
    let out = key_hire(&st.root, &empty, true).expect("an empty folder is new");
    assert!(out.created && registered(&empty));
}

// ── The hire root's owner and mode ──────────────────────────────────

fn assert_unsafe(e: &HireError, dir: &Path, what: &str) {
    assert_eq!((e.status, e.code), (403, "hire_root_unsafe"), "{what}: {e}");
    assert!(e.hint.contains(&s(dir)), "{what}: the hint names {}: {e}", dir.display());
    assert!(e.hint.contains("can be changed by another user") && e.hint.contains("chmod go-w"), "{what}: {e}");
}

/// A key's hire in a root a group can write: refused with
/// `hire_root_unsafe` before anything else, so a folder someone else put
/// there first is never taken on, and no new folder is made.
#[test]
fn a_key_hire_in_a_group_writable_root_is_refused_before_any_folder_is_taken_on() {
    let st = setup();
    let theirs = st.root.join("theirs");
    fs::create_dir(&theirs).expect("mkdir");
    fs::write(theirs.join("notes.txt"), b"theirs").expect("write");
    let empty = st.root.join("empty");
    fs::create_dir(&empty).expect("mkdir");
    chmod(&st.root, 0o770);
    for (path, template, what) in [
        (&theirs, true, "template, a folder with files"),
        (&theirs, false, "plain, a folder with files"),
        (&empty, true, "template, an empty folder"),
        (&empty, false, "plain, an empty folder"),
    ] {
        let e = key_hire(&st.root, path, template).expect_err(what);
        assert_unsafe(&e, &st.root, what);
        assert!(!registered(path), "{what}: not registered");
    }
    let fresh = st.root.join("fresh");
    let e = key_hire(&st.root, &fresh, false).expect_err("a new folder");
    assert_unsafe(&e, &st.root, "a new folder");
    assert!(!fresh.exists(), "no folder made");
    assert_eq!(entries(&theirs), 1, "their folder is as it was");
}

/// The stored hire root, put back after a test that may set it.
struct KeepRoot(Option<String>);

impl KeepRoot {
    fn new() -> Self {
        let db = crate::db::shared();
        let conn = db.lock();
        Self(super::super::root::stored_root(&conn))
    }
}

impl Drop for KeepRoot {
    fn drop(&mut self) {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE hire_settings SET root = ?1 WHERE id = 1", rusqlite::params![self.0]).expect("put the root back");
    }
}

fn stored() -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    super::super::root::stored_root(&conn)
}

#[test]
fn set_hire_root_refuses_a_world_writable_folder() {
    let st = setup();
    let _keep = KeepRoot::new();
    let before = stored();
    chmod(&st.root, 0o777);
    let e = super::super::root::set_hire_root(&s(&st.root)).expect_err("world writable");
    assert_unsafe(&e, &st.root, "world writable");
    assert_eq!(stored(), before, "the root is unchanged");
}

#[test]
fn set_hire_root_refuses_a_group_writable_folder() {
    let st = setup();
    let _keep = KeepRoot::new();
    let before = stored();
    chmod(&st.root, 0o770);
    let e = super::super::root::set_hire_root(&s(&st.root)).expect_err("group writable");
    assert_unsafe(&e, &st.root, "group writable");
    assert_eq!(stored(), before, "the root is unchanged");
}

/// The sticky bit stops another user deleting a name, not making the name
/// the next hire is about to use: `/tmp` is refused too.
#[test]
fn set_hire_root_refuses_a_sticky_world_writable_folder() {
    let st = setup();
    let _keep = KeepRoot::new();
    let before = stored();
    chmod(&st.root, 0o1777);
    let e = super::super::root::set_hire_root(&s(&st.root)).expect_err("sticky world writable");
    assert_unsafe(&e, &st.root, "sticky world writable");
    // A root not made yet under such a folder names that folder.
    let below = st.root.join("agents");
    let e = super::super::root::set_hire_root(&s(&below)).expect_err("below a sticky folder");
    assert_unsafe(&e, &st.root, "below a sticky folder");
    assert_eq!(stored(), before, "the root is unchanged");
}

#[test]
fn set_hire_root_takes_a_folder_only_its_owner_can_write() {
    let st = setup();
    let _keep = KeepRoot::new();
    for mode in [0o700, 0o755] {
        chmod(&st.root, mode);
        let set = super::super::root::set_hire_root(&s(&st.root)).unwrap_or_else(|e| panic!("{mode:o}: {e}"));
        assert_eq!(set, st.root, "{mode:o}");
    }
}

/// The doctor names the folder another user could change on the way to the
/// hire root, and each workspace already under it, one line each; a root
/// others can only read gets one line saying 0700 is tighter.
#[test]
fn the_doctor_names_an_unsafe_hire_root_and_the_workspaces_under_it() {
    let st = setup();
    let _keep = KeepRoot::new();
    chmod(&st.root, 0o700);
    super::super::root::set_hire_root(&s(&st.root)).expect("set the root");
    assert!(super::super::root::doctor_notes(true).is_empty(), "a 0700 root has nothing to say");
    let ws = st.root.join(uniq("ws"));
    let req: ConvergeRequest = serde_json::from_value(serde_json::json!({ "path": s(&ws), "name": "Under" })).expect("body");
    converge(&req, &HireCaller::Owner, &ConvergeAuthority::unrestricted()).expect("the owner's hire");
    chmod(&ws, 0o755);
    chmod(&st.root, 0o755);
    let notes = super::super::root::doctor_notes(true);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains(&s(&st.root)) && notes[0].contains("0700 is tighter"), "{notes:?}");
    chmod(&st.root, 0o770);
    let notes = super::super::root::doctor_notes(true);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].starts_with(&format!("hire root {}", s(&st.root))) && notes[0].contains("can be changed by another user"), "{notes:?}");
    assert!(notes[1].contains(&s(&ws)) && notes[1].contains(&format!("{} can be changed by another user", s(&st.root))), "{notes:?}");
    assert_eq!(super::super::root::doctor_notes(false).len(), 1, "no workspace lines without them asked for");
    chmod(&st.root, 0o700);
}
