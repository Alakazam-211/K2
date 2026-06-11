//! Integration-ish tests for the Clone-to unpack route.
//!
//! Each test builds a synthetic bundle (via the k2so-core engine) from a
//! temp source workspace, then unpacks it into a temp DEST with a temp
//! HOME override so the `~/.claude/projects/<slug>/` placement is hermetic
//! and the real home is never touched. Registration goes through the
//! shared in-memory test DB (k2so-core `test-util`).

use super::*;
use k2_core::clone;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

// ── temp dir (no tempfile dep; mirrors the core clone tests) ──────────
struct TempDir {
    path: PathBuf,
}
impl TempDir {
    fn new(prefix: &str) -> Self {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        static CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}-{n}"));
        fs::create_dir_all(&path).expect("create tempdir");
        Self { path }
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir -p");
    }
    fs::write(path, contents).expect("write file");
}

/// Build a synthetic SOURCE: a workspace tree + a hermetic
/// `<src_home>/.claude/projects/<slug>/` with memory + a session. Returns
/// the bundle path (built with `agent_mode='manager'` settings) and the
/// roots so the caller can keep them alive.
fn build_source_bundle(
    root: &TempDir,
    settings: Option<clone::WorkspaceSettings>,
) -> (PathBuf, String) {
    let project = root.path().join("source").join("My Agent");
    let src_home = root.path().join("src-home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&src_home).unwrap();

    write(&project.join("README.md"), "# My Agent\nproject docs\n");
    write(&project.join(".k2so/PROJECT.md"), "# Project\n");
    write(&project.join("src/main.rs"), "fn main() {}\n");

    let canon = fs::canonicalize(&project).unwrap();
    let slug = k2_core::chat_history::claude_project_hash(&canon.to_string_lossy());
    let slug_dir = src_home.join(".claude").join("projects").join(&slug);
    write(&slug_dir.join("memory/MEMORY.md"), "## Memory Index\nm1\n");
    write(&slug_dir.join("memory/a.md"), "memory a\n");
    write(
        &slug_dir.join("44444444-4444-4444-4444-444444444444.jsonl"),
        "{\"type\":\"live\"}\n",
    );

    let opts = clone::CloneOptions {
        home_override: Some(src_home.clone()),
        ..Default::default()
    };
    let inv = clone::inventory(&canon.to_string_lossy(), opts.clone()).unwrap();
    let bundle = root.path().join("bundle.tar.gz");
    clone::build_bundle(
        &inv,
        &opts,
        "2026-06-05T00:00:00Z".to_string(),
        settings,
        &bundle,
    )
    .unwrap();

    (bundle, slug)
}

fn manager_settings() -> clone::WorkspaceSettings {
    clone::WorkspaceSettings {
        agent_mode: "manager".to_string(),
        agent_enabled: true,
        heartbeat_enabled: true,
        name: "My Agent".to_string(),
        color: "#aa00aa".to_string(),
        worktree_mode: 2,
    }
}

#[test]
fn unpack_places_files_registers_and_applies_settings() {
    let root = TempDir::new("k2so-unpack-test");
    let (bundle, _src_slug) = build_source_bundle(&root, Some(manager_settings()));

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    let (project, dest_path) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack + register must succeed");

    // dest_path = <dest_parent>/My Agent
    let expected_dest = dest_parent.join("My Agent");
    assert_eq!(
        Path::new(&dest_path),
        expected_dest,
        "dest dir named after the source workspace"
    );

    // workspace files landed at dest_path
    assert!(
        expected_dest.join("README.md").is_file(),
        "workspace README at dest"
    );
    assert!(expected_dest.join(".k2so/PROJECT.md").is_file());
    assert!(expected_dest.join("src/main.rs").is_file());

    // memory + session under the RECOMPUTED remote slug dir
    let remote_slug =
        k2_core::chat_history::claude_project_hash(&dest_path);
    let remote_slug_dir = remote_home
        .join(".claude")
        .join("projects")
        .join(&remote_slug);
    assert!(
        remote_slug_dir.join("memory/MEMORY.md").is_file(),
        "memory under recomputed slug, looked at {}",
        remote_slug_dir.display()
    );
    assert!(remote_slug_dir.join("memory/a.md").is_file());
    assert!(
        remote_slug_dir
            .join("44444444-4444-4444-4444-444444444444.jsonl")
            .is_file(),
        "live session re-rooted under recomputed slug"
    );

    // project registered at dest_path with the applied settings
    assert_eq!(project.path, dest_path, "registered at dest_path");
    assert_eq!(project.name, "My Agent");
    assert_eq!(project.color, "#aa00aa");
    assert_eq!(project.agent_mode, "manager");
    assert_eq!(project.agent_enabled, 1, "manager → enabled");
    // d410883: `heartbeat_enabled` on a Project READ is a LIVE aggregate
    // ("≥1 enabled, non-archived heartbeat row?"), not the legacy
    // projects-column flag the clone settings write. The clone does not
    // bundle workspace_heartbeats rows, so the truthful aggregate for a
    // fresh unpack is 0 — asserting the legacy 1 here was stale (the
    // suite's only red since that change).
    assert_eq!(
        project.heartbeat_enabled, 0,
        "no heartbeat rows cloned → live aggregate is off"
    );
    assert_eq!(project.worktree_mode, 2);

    // confirm it's queryable in the DB
    let db = k2_core::db::shared();
    let conn = db.lock();
    let fetched = Project::get(&conn, &project.id).expect("row exists");
    assert_eq!(fetched.agent_mode, "manager");
    drop(conn);

    // cleanup DB row so the shared in-memory DB stays tidy.
    let _ = pops::projects_delete(&project.id);
}

#[test]
fn unpack_is_collision_safe() {
    let root = TempDir::new("k2so-unpack-collide");
    let (bundle, _slug) = build_source_bundle(&root, Some(manager_settings()));

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    // Pre-create the target dir so the first unpack must collision-rename.
    fs::create_dir_all(dest_parent.join("My Agent")).unwrap();

    let (project, dest_path) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack succeeds despite collision");

    assert_eq!(
        Path::new(&dest_path),
        dest_parent.join("My Agent (1)"),
        "collision-safe rename to 'name (1)', got {dest_path}"
    );
    assert!(dest_parent.join("My Agent (1)").join("README.md").is_file());

    let _ = pops::projects_delete(&project.id);
}

#[test]
fn unpack_with_no_settings_registers_with_defaults() {
    let root = TempDir::new("k2so-unpack-nosettings");
    let (bundle, _slug) = build_source_bundle(&root, None);

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    let (project, _dest) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack with no settings still registers");

    // Default registration: agent off, default color, name = folder.
    assert_eq!(project.name, "My Agent");
    assert_eq!(project.agent_mode, "off");

    let _ = pops::projects_delete(&project.id);
}

#[test]
fn unpack_handler_400s_on_missing_bundle() {
    // The token gate lives in the dispatcher, not the handler (the
    // garbage-token → 403 path is covered end-to-end in
    // clone_routes_integration.rs). This asserts the HANDLER itself
    // produces a clean 400 on a nonexistent bundle path rather than
    // panicking.
    let body = serde_json::json!({
        "bundle_path": "/nonexistent/bundle.tar.gz",
        "dest_parent": "/tmp",
    })
    .to_string();
    let resp = super::handle_clone_unpack(body.as_bytes());
    assert_eq!(resp.status, "400 Bad Request", "missing bundle → 400");
}

#[test]
fn bundle_handler_rejects_invalid_json() {
    let resp = super::handle_clone_bundle(b"not json");
    assert_eq!(resp.status, "400 Bad Request");
}

// ── #655 disk-leak cleanup ────────────────────────────────────────────

/// A successfully-unpacked bundle file must be deleted on the destination
/// so `~/.k2so/clone-tmp/*.tar.gz` doesn't accumulate over repeated clones.
#[test]
fn unpack_removes_the_uploaded_bundle_file() {
    let root = TempDir::new("k2so-unpack-cleanup");
    let (bundle, _slug) = build_source_bundle(&root, Some(manager_settings()));

    // Simulate the uploaded bundle living under the dest's clone-tmp dir so
    // we assert the EXACT delete-after-unpack behaviour.
    let remote_home = root.path().join("remote-home");
    let clone_tmp = remote_home.join(".k2").join("clone-tmp");
    fs::create_dir_all(&clone_tmp).unwrap();
    let uploaded = clone_tmp.join("My Agent-20260605-000000.tar.gz");
    fs::copy(&bundle, &uploaded).unwrap();
    assert!(uploaded.is_file(), "precondition: uploaded bundle present");

    let dest_parent = root.path().join("dest");
    fs::create_dir_all(&dest_parent).unwrap();

    let (project, _dest_path) =
        super::unpack_and_register(&uploaded, &dest_parent, &remote_home)
            .expect("unpack + register must succeed");

    assert!(
        !uploaded.exists(),
        "uploaded bundle must be deleted after a successful unpack, still at {}",
        uploaded.display()
    );

    let _ = pops::projects_delete(&project.id);
}

/// The stale-prune deletes an old `*.tar.gz` while leaving a fresh bundle
/// and any non-matching file untouched, and never recurses into subdirs.
#[test]
fn prune_stale_bundles_removes_only_old_tar_gz() {
    let root = TempDir::new("k2so-prune");
    let tmp_dir = root.path().join(".k2").join("clone-tmp");
    fs::create_dir_all(&tmp_dir).unwrap();

    let stale = tmp_dir.join("old-workspace-20200101-000000.tar.gz");
    let fresh = tmp_dir.join("new-workspace-20260605-000000.tar.gz");
    let other = tmp_dir.join("keepme.txt");
    let nested_dir = tmp_dir.join("nested");
    let nested_bundle = nested_dir.join("deep-20200101-000000.tar.gz");
    write(&stale, "stale");
    write(&fresh, "fresh");
    write(&other, "not a bundle");
    fs::create_dir_all(&nested_dir).unwrap();
    write(&nested_bundle, "should not be touched (no recursion)");

    // Backdate the stale bundle's mtime to ~2 hours ago.
    let two_hours_ago = std::time::SystemTime::now()
        - std::time::Duration::from_secs(2 * 60 * 60);
    filetime_set(&stale, two_hours_ago);

    // Prune anything older than 1 hour.
    super::prune_stale_bundles(&tmp_dir, std::time::Duration::from_secs(60 * 60));

    assert!(
        !stale.exists(),
        "stale *.tar.gz must be pruned, still at {}",
        stale.display()
    );
    assert!(
        fresh.exists(),
        "fresh *.tar.gz must be left, missing at {}",
        fresh.display()
    );
    assert!(
        other.exists(),
        "non-matching file must be left untouched, missing at {}",
        other.display()
    );
    assert!(
        nested_bundle.exists(),
        "prune must NOT recurse into subdirs, missing at {}",
        nested_bundle.display()
    );
}

/// Set a file's mtime via `set-file-times`-style raw libc, with no extra
/// crate dep. We re-write the file then use `std::fs` + a filetime shim:
/// here we lean on the `filetime` crate if present, else fall back to
/// touching with a backdated time through a small unsafe utimes call.
fn filetime_set(path: &Path, when: std::time::SystemTime) {
    let dur = when
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time before epoch");
    let secs = dur.as_secs() as libc::time_t;
    let micros = dur.subsec_micros() as libc::suseconds_t;
    let tv = [
        libc::timeval { tv_sec: secs, tv_usec: micros },
        libc::timeval { tv_sec: secs, tv_usec: micros },
    ];
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let rc = unsafe { libc::utimes(cpath.as_ptr(), tv.as_ptr()) };
    assert_eq!(rc, 0, "utimes failed for {}", path.display());
}
