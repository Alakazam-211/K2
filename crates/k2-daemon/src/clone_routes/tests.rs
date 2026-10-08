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
    build_source_bundle_with_identity(root, settings, None, vec![])
}

fn build_source_bundle_with_identity(
    root: &TempDir,
    settings: Option<clone::WorkspaceSettings>,
    pinned_chat: Option<clone::PinnedChatIdentity>,
    chat_pins: Vec<clone::ChatPinEntry>,
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
        pinned_chat,
        chat_pins,
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

/// Pinned chat + chat pins in the manifest are applied AFTER settings,
/// including when settings is Some (the early-return path that used to
/// skip identity apply).
#[test]
fn unpack_applies_pinned_chat_and_chat_pins() {
    let root = TempDir::new("k2so-unpack-identity");
    let sid = "44444444-4444-4444-4444-444444444444";
    let (bundle, _slug) = build_source_bundle_with_identity(
        &root,
        Some(manager_settings()),
        Some(clone::PinnedChatIdentity {
            session_id: sid.to_string(),
            harness: "claude".to_string(),
        }),
        vec![clone::ChatPinEntry {
            provider: "claude".to_string(),
            session_id: sid.to_string(),
            custom_name: "Migrated Chat".to_string(),
            pinned: true,
        }],
    );

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    let (project, dest_path) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack + identity must succeed");

    // Settings still applied (proves we didn't break that path).
    assert_eq!(project.agent_mode, "manager");
    assert_eq!(project.name, "My Agent");

    // Session file landed under remote home.
    let remote_slug = k2_core::chat_history::claude_project_hash(&dest_path);
    assert!(
        remote_home
            .join(".claude")
            .join("projects")
            .join(&remote_slug)
            .join(format!("{sid}.jsonl"))
            .is_file(),
        "session must exist on dest for pin apply"
    );

    let db = k2_core::db::shared();
    let conn = db.lock();
    let ws = k2_core::db::schema::WorkspaceSession::get(&conn, &project.id)
        .expect("db ok")
        .expect("workspace_sessions row after identity apply");
    assert_eq!(
        ws.session_id.as_deref(),
        Some(sid),
        "pinned session_id stamped on dest"
    );
    assert_eq!(ws.harness, "claude");

    let (custom_name, pinned): (String, i64) = conn
        .query_row(
            "SELECT custom_name, pinned FROM chat_session_names \
             WHERE provider = 'claude' AND session_id = ?1",
            rusqlite::params![sid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("chat pin row applied");
    assert_eq!(custom_name, "Migrated Chat");
    assert_eq!(pinned, 1);
    // Leave the shared test DB tidy.
    conn.execute(
        "DELETE FROM chat_session_names WHERE session_id = ?1",
        rusqlite::params![sid],
    )
    .expect("cleanup chat pin");
    drop(conn);

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
    let resp = super::handle_clone_unpack(body.as_bytes(), None);
    assert_eq!(resp.status, "400 Bad Request", "missing bundle → 400");
}

#[test]
fn bundle_handler_rejects_invalid_json() {
    let resp = super::handle_clone_bundle(b"not json", None);
    assert_eq!(resp.status, "400 Bad Request");
}

// ── 0.40.22 pull-pack job ("Clone to this computer") ──────────────────

/// The pack-job map is process-global and `insert_pack_job` evicts
/// finished jobs — tests that insert/read jobs must serialize or a
/// parallel insert can evict another test's just-finished job.
static PACK_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn pack_lock() -> std::sync::MutexGuard<'static, ()> {
    PACK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn pack_handler_rejects_invalid_json() {
    let resp = super::handle_clone_pack(b"not json", None);
    assert_eq!(resp.status, "400 Bad Request");
}

/// The pull-pack gate: a path that exists but is NOT a registered
/// workspace must 400 before any job is created.
#[test]
fn pack_rejects_unregistered_workspace() {
    let root = TempDir::new("k2so-pack-unregistered");
    let body = serde_json::json!({ "project_path": root.path().to_string_lossy() })
        .to_string();
    let resp = super::handle_clone_pack(body.as_bytes(), None);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    assert!(
        resp.body.contains("not a registered workspace"),
        "must say WHY it was rejected, got: {}",
        resp.body
    );
}

/// Nonexistent path → the shared `validate_path` 400, not a failed job.
#[test]
fn pack_rejects_nonexistent_path() {
    let resp = super::handle_clone_pack(
        br#"{"project_path":"/nonexistent/definitely-not-here-k2/ws"}"#,
        None,
    );
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

/// Worker happy path, hermetic: the bundle lands under `<home>/.k2/
/// clone-tmp/`, the job flips to `done` with a path + summary fields, and
/// `pack-cleanup` then removes the bundle (idempotently).
#[test]
fn pack_job_builds_bundle_and_cleanup_removes_it() {
    let _g = pack_lock();
    let root = TempDir::new("k2so-pack-ok");
    let project = root.path().join("Pack WS");
    let home = root.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    write(&project.join("README.md"), "# Pack WS\n");

    let job_id = "pack-test-ok".to_string();
    super::insert_pack_job(super::PackJob {
        job_id: job_id.clone(),
        phase: "running",
        bundle_path: None,
        size_bytes: None,
        entry_count: None,
        scrubbed_secret_count: None,
        error: None,
        vault_carry_b64: None,
        vault_carry: None,
    });
    let opts = clone::CloneOptions {
        home_override: Some(home.clone()),
        ..Default::default()
    };
    super::run_pack_job(
        &job_id,
        &project.to_string_lossy(),
        &opts,
        &home,
        k2_core::fs_commands::MAX_TRANSFER_SIZE,
        None,
    );

    let job = super::get_pack_job(&job_id).expect("job stays in the map");
    assert_eq!(job.phase, "done", "error: {:?}", job.error);
    let bundle_path = job.bundle_path.expect("done job carries bundle_path");
    let bundle = Path::new(&bundle_path);
    assert!(bundle.is_file(), "bundle missing at {bundle_path}");
    assert!(
        bundle.starts_with(home.join(".k2").join("clone-tmp")),
        "bundle must land in <home>/.k2/clone-tmp, got {bundle_path}"
    );
    assert_eq!(job.size_bytes, Some(fs::metadata(bundle).unwrap().len()));
    assert!(job.entry_count.unwrap_or(0) >= 1, "README must be bundled");

    // Cleanup removes the bundle; a second call is a no-op success.
    let body = serde_json::json!({ "job_id": job_id }).to_string();
    let resp = super::handle_clone_pack_cleanup(body.as_bytes());
    assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
    assert!(!bundle.exists(), "cleanup must delete the bundle");
    let resp = super::handle_clone_pack_cleanup(body.as_bytes());
    assert_eq!(resp.status, "200 OK", "idempotent; body: {}", resp.body);
}

/// A bundle over the transfer ceiling flips the job to `failed` AND is
/// deleted from disk — no terminal "success" the download would refuse.
#[test]
fn pack_job_enforces_transfer_ceiling() {
    let _g = pack_lock();
    let root = TempDir::new("k2so-pack-oversize");
    let project = root.path().join("Big WS");
    let home = root.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    write(&project.join("README.md"), "# Big WS — bigger than 1 byte\n");

    let job_id = "pack-test-oversize".to_string();
    super::insert_pack_job(super::PackJob {
        job_id: job_id.clone(),
        phase: "running",
        bundle_path: None,
        size_bytes: None,
        entry_count: None,
        scrubbed_secret_count: None,
        error: None,
        vault_carry_b64: None,
        vault_carry: None,
    });
    let opts = clone::CloneOptions {
        home_override: Some(home.clone()),
        ..Default::default()
    };
    // 1-byte ceiling: any real bundle is over it.
    super::run_pack_job(&job_id, &project.to_string_lossy(), &opts, &home, 1, None);

    let job = super::get_pack_job(&job_id).expect("job stays in the map");
    assert_eq!(job.phase, "failed");
    assert!(
        job.error.as_deref().unwrap_or("").contains("transfer ceiling"),
        "error must name the ceiling, got {:?}",
        job.error
    );
    assert!(job.bundle_path.is_none(), "failed job must not expose a path");
    let leftovers: Vec<_> = fs::read_dir(home.join(".k2").join("clone-tmp"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().to_string_lossy().ends_with(".tar.gz"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "over-ceiling bundle must be deleted, found {leftovers:?}"
    );
}

#[test]
fn pack_status_rejects_unknown_job_id() {
    let params = std::collections::HashMap::from([(
        "job_id".to_string(),
        "no-such-job".to_string(),
    )]);
    let resp = super::handle_clone_pack_status(&params);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn pack_cleanup_rejects_unknown_job_and_running_job() {
    let _g = pack_lock();
    let resp = super::handle_clone_pack_cleanup(br#"{"job_id":"no-such-job"}"#);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);

    super::insert_pack_job(super::PackJob {
        job_id: "pack-test-running".to_string(),
        phase: "running",
        bundle_path: None,
        size_bytes: None,
        entry_count: None,
        scrubbed_secret_count: None,
        error: None,
        vault_carry_b64: None,
        vault_carry: None,
    });
    let resp = super::handle_clone_pack_cleanup(br#"{"job_id":"pack-test-running"}"#);
    assert_eq!(
        resp.status, "400 Bad Request",
        "running job must not be cleanable; body: {}",
        resp.body
    );
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

// ── vault secrets on the owner's own move (0.45.2, clone_vault) ──────

mod vault_carry {
    use super::*;
    use crate::clone_vault::{self, CarryRequest, VaultTarget, OWN_MOVE_ONLY};
    use crate::vault::actor::{Channel, VaultCaller};
    use k2_vault::confirm::KdfCost;
    use k2_vault::fsutil::TestDir;
    use k2_vault::keystore::fake::FakeKeyStore;
    use k2_vault::{
        Actor, KeyStore, Kind, ListFilter, LocalVault, Namespace, OpenOptions, PutRequest,
        Requester, Role, Scope, ScopeKind, SecretBytes, SecretStore, Selector, Use,
    };
    use std::sync::Arc;

    const SENTINEL: &str = "sk_test_CLONECARRY_7f6e5d4c3b2a19087";
    const SERVER_SENTINEL: &str = "sk_test_CLONECARRY_SERVER_0a1b2c3d4e";

    struct V {
        _dir: TestDir,
        v: Arc<LocalVault>,
    }

    fn temp_vault(tag: &str) -> V {
        let dir = TestDir::new(tag);
        let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
        let v = Arc::new(
            LocalVault::open(dir.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor })
                .expect("open temp vault"),
        );
        V { _dir: dir, v }
    }

    fn owner() -> VaultCaller {
        VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None }
    }

    fn member() -> VaultCaller {
        VaultCaller {
            actor: Actor::Login { username: "lee".into(), role: Role::Member },
            channel: Channel::Login,
            session_id: None,
        }
    }

    fn target_of(v: &LocalVault) -> VaultTarget {
        VaultTarget {
            recipient: v.carry_recipient().expect("recipient").encode(),
            vault_id: v.vault_id().to_string(),
        }
    }

    fn put(v: &LocalVault, scope: Scope, name: &str, value: &str) {
        let mut r = PutRequest::new(Namespace::User, scope, name, Kind::ApiKey);
        r.hosts = vec!["https://api.stripe.com".into()];
        v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put");
    }

    /// Workspace-scope user secrets in `v`, as `(scope id, name)`.
    fn workspace_secrets(v: &LocalVault) -> Vec<(String, String)> {
        v.list(&ListFilter::default(), &Actor::System("test".into()))
            .expect("list")
            .into_iter()
            .filter(|m| m.namespace == Namespace::User && m.scope.kind == ScopeKind::Workspace)
            .map(|m| (m.scope.id.clone(), m.name.clone()))
            .collect()
    }

    fn value_of(v: &LocalVault, scope_id: &str, name: &str) -> String {
        let m = v
            .list(&ListFilter::default(), &Actor::System("test".into()))
            .expect("list")
            .into_iter()
            .find(|m| m.scope.id == scope_id && m.name == name)
            .unwrap_or_else(|| panic!("no secret {name} in scope {scope_id}"));
        let got = v
            .resolve(&Selector::Id(m.id.clone()), Use::System, &Requester::System("test".into()))
            .expect("resolve");
        String::from_utf8(got.value.expose().to_vec()).expect("utf8")
    }

    /// A registered source workspace (from the shared bundle fixture) with
    /// one workspace secret, one server secret and one secret of another
    /// workspace in `src`.
    fn source_with_secrets(root: &TempDir, src: &V) -> (PathBuf, String, Project) {
        let (bundle, _slug) = build_source_bundle(root, Some(manager_settings()));
        let canon = fs::canonicalize(root.path().join("source").join("My Agent")).unwrap();
        let path = canon.to_string_lossy().to_string();
        let project = pops::projects_add_without_git(&path).expect("register source");
        put(&src.v, Scope::workspace(project.id.clone()), "STRIPE_KEY", SENTINEL);
        put(&src.v, Scope::server(), "SERVER_ONLY", SERVER_SENTINEL);
        put(&src.v, Scope::workspace("some-other-workspace"), "OTHER_KEY", SERVER_SENTINEL);
        (bundle, path, project)
    }

    #[test]
    fn a_vault_target_without_own_box_move_is_refused_with_a_teaching_code() {
        let dst = temp_vault("clone-refuse-dst");
        let body = serde_json::json!({
            "project_path": "/nonexistent/clone-vault-refusal",
            "vault_target": { "recipient": target_of(&dst.v).recipient, "vault_id": dst.v.vault_id() },
            "own_box_move": false,
        })
        .to_string();
        for resp in [
            super::super::handle_clone_bundle(body.as_bytes(), Some(owner())),
            super::super::handle_clone_pack(body.as_bytes(), Some(owner())),
        ] {
            assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert_eq!(v["code"], OWN_MOVE_ONLY, "body: {}", resp.body);
            assert!(
                v["error"].as_str().expect("error text").contains("your own server"),
                "the refusal teaches why: {}",
                resp.body
            );
        }
    }

    #[test]
    fn the_carry_lands_in_the_renamed_destination_project_and_never_shows_a_value() {
        let root = TempDir::new("k2-clone-vault-roundtrip");
        let src = temp_vault("clone-rt-src");
        let dst = temp_vault("clone-rt-dst");
        let (bundle, src_path, src_project) = source_with_secrets(&root, &src);

        // Source side: only this workspace's secret is sealed.
        let req = CarryRequest { target: target_of(&dst.v), caller: Some(owner()), vault: Ok(src.v.clone()) };
        let sealed = clone_vault::seal_for_workspace(&req, &src_path);
        assert_eq!(sealed.report["sealed"], true, "report: {}", sealed.report);
        assert_eq!(sealed.report["count"], 1, "report: {}", sealed.report);
        assert_eq!(sealed.report["secrets"], serde_json::json!(["STRIPE_KEY"]));
        let carry = sealed.carry_b64.expect("a carry is sealed");
        for s in [sealed.report.to_string(), carry.clone()] {
            assert!(!s.contains(SENTINEL) && !s.contains(SERVER_SENTINEL), "no value in {s}");
        }

        // Destination: the folder name is taken, so the clone lands renamed
        // as a new project with its own id.
        let dest_parent = root.path().join("dest");
        let remote_home = root.path().join("remote-home");
        fs::create_dir_all(dest_parent.join("My Agent")).unwrap();
        fs::create_dir_all(&remote_home).unwrap();
        let (project, dest_path) =
            super::super::unpack_and_register(&bundle, &dest_parent, &remote_home).expect("unpack");
        assert_eq!(Path::new(&dest_path), dest_parent.join("My Agent (1)"));
        assert_ne!(project.id, src_project.id, "the destination row is a new project");
        assert!(workspace_secrets(&dst.v).is_empty());

        let out = clone_vault::import_into_project(Ok(dst.v.clone()), Some(&owner()), &carry, &project.id);
        assert!(out.get("vault_error").is_none(), "out: {out}");
        assert_eq!(out["vault_imported"], serde_json::json!(["STRIPE_KEY"]), "out: {out}");
        assert_eq!(out["vault_skipped"], serde_json::json!([]), "out: {out}");
        assert!(!out.to_string().contains(SENTINEL), "no value in {out}");

        // Mapped to the NEW project id — not the source label, which names
        // the source row in this shared test DB.
        assert_eq!(
            workspace_secrets(&dst.v),
            vec![(project.id.clone(), "STRIPE_KEY".to_string())],
            "only the cloned workspace's secret, under the new project"
        );
        assert_eq!(value_of(&dst.v, &project.id, "STRIPE_KEY"), SENTINEL);
        let meta = dst
            .v
            .list(&ListFilter::default(), &Actor::System("test".into()))
            .expect("list")
            .into_iter()
            .find(|m| m.name == "STRIPE_KEY")
            .expect("imported meta");
        assert!(meta.human_given, "the owner's own move keeps person-given (VS20)");
        assert_eq!(dst.v.user_secret_count(), 1, "server and other-workspace secrets stay home");

        // A second import keeps what is there.
        let again = clone_vault::import_into_project(Ok(dst.v.clone()), Some(&owner()), &carry, &project.id);
        assert_eq!(again["vault_imported"], serde_json::json!([]), "again: {again}");
        assert_eq!(again["vault_skipped"].as_array().expect("skipped").len(), 1, "again: {again}");

        let _ = pops::projects_delete(&project.id);
        let _ = pops::projects_delete(&src_project.id);
    }

    #[test]
    fn the_source_says_why_when_nothing_is_sealed() {
        let root = TempDir::new("k2-clone-vault-skips");
        let src = temp_vault("clone-skip-src");
        let dst = temp_vault("clone-skip-dst");
        let (_bundle, src_path, src_project) = source_with_secrets(&root, &src);

        let closed = CarryRequest {
            target: target_of(&dst.v),
            caller: Some(owner()),
            vault: Err(k2_vault::VaultError::new("vault_not_open", 503, "the vault isn't open yet")),
        };
        let s = clone_vault::seal_for_workspace(&closed, &src_path);
        assert!(s.carry_b64.is_none());
        assert_eq!(s.report["sealed"], false);
        assert_eq!(s.report["reason"], "vault_not_open", "report: {}", s.report);

        let by_member = CarryRequest { target: target_of(&dst.v), caller: Some(member()), vault: Ok(src.v.clone()) };
        let s = clone_vault::seal_for_workspace(&by_member, &src_path);
        assert!(s.carry_b64.is_none(), "a Member can clone but not move vault secrets");
        assert_eq!(s.report["reason"], "vault_admin_required", "report: {}", s.report);

        let empty_vault = temp_vault("clone-skip-empty");
        let none = CarryRequest { target: target_of(&dst.v), caller: Some(owner()), vault: Ok(empty_vault.v.clone()) };
        let s = clone_vault::seal_for_workspace(&none, &src_path);
        assert!(s.carry_b64.is_none());
        assert_eq!(s.report["reason"], "no_secrets", "report: {}", s.report);
        assert!(s.report["note"].as_str().expect("note").contains("no vault secrets"));

        let _ = pops::projects_delete(&src_project.id);
    }

    #[test]
    fn the_destination_refuses_a_carry_not_made_for_an_own_move() {
        let src = temp_vault("clone-notown-src");
        let dst = temp_vault("clone-notown-dst");
        put(&src.v, Scope::workspace("ws-not-own"), "STRIPE_KEY", SENTINEL);
        let mut ws = std::collections::HashMap::new();
        ws.insert("ws-not-own".to_string(), "ws-not-own".to_string());
        let t = target_of(&dst.v);
        let sealed = crate::vault_use_routes::seal_carry(&src.v, &t.recipient, &t.vault_id, &ws, &[], false, None)
            .expect("seal");
        let carry = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &sealed.file);

        let out = clone_vault::import_into_project(Ok(dst.v.clone()), Some(&owner()), &carry, "new-project-id");
        assert_eq!(out["vault_error"]["code"], OWN_MOVE_ONLY, "out: {out}");
        assert_eq!(dst.v.user_secret_count(), 0);

        let own = crate::vault_use_routes::seal_carry(&src.v, &t.recipient, &t.vault_id, &ws, &[], true, None)
            .expect("seal");
        let carry = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &own.file);
        let out = clone_vault::import_into_project(Ok(dst.v.clone()), Some(&member()), &carry, "new-project-id");
        assert_eq!(out["vault_error"]["code"], "vault_admin_required", "out: {out}");
        assert_eq!(dst.v.user_secret_count(), 0);

        // Another vault can't open a carry bound to this one.
        let other = temp_vault("clone-notown-other");
        let out = clone_vault::import_into_project(Ok(other.v.clone()), Some(&owner()), &carry, "new-project-id");
        assert!(out["vault_error"]["code"].is_string(), "out: {out}");
        assert_eq!(other.v.user_secret_count(), 0);
    }

    /// Through the route: a broken carry never fails the unpack, and a good
    /// one is reported by name only.
    #[test]
    fn unpack_route_reports_the_import_and_survives_a_bad_carry() {
        crate::test_support::with_temp_home(|| {
            let dst = crate::vault::ensure_test_vault();
            let root = TempDir::new("k2-clone-vault-route");
            let src = temp_vault("clone-route-src");
            let (bundle, src_path, src_project) = source_with_secrets(&root, &src);
            let req = CarryRequest { target: target_of(&dst), caller: Some(owner()), vault: Ok(src.v.clone()) };
            let carry = clone_vault::seal_for_workspace(&req, &src_path).carry_b64.expect("sealed");

            // Bad carry first: the unpack still succeeds and says why.
            let dest_parent = root.path().join("dest-bad");
            fs::create_dir_all(&dest_parent).unwrap();
            let bad_bundle = root.path().join("bundle-copy.tar.gz");
            fs::copy(&bundle, &bad_bundle).unwrap();
            let body = serde_json::json!({
                "bundle_path": bad_bundle.to_string_lossy(),
                "dest_parent": dest_parent.to_string_lossy(),
                "vault_carry_b64": "bm90IGEgY2Fycnk=",
            })
            .to_string();
            let resp = super::super::handle_clone_unpack(body.as_bytes(), Some(owner()));
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert!(v["dest_path"].is_string(), "body: {}", resp.body);
            assert!(v["vault_error"]["code"].is_string(), "body: {}", resp.body);
            let bad_id = v["project"]["id"].as_str().expect("project id").to_string();

            // Good carry: imported into the new project, value never echoed.
            let dest_parent = root.path().join("dest-good");
            fs::create_dir_all(&dest_parent).unwrap();
            let body = serde_json::json!({
                "bundle_path": bundle.to_string_lossy(),
                "dest_parent": dest_parent.to_string_lossy(),
                "vault_carry_b64": carry,
            })
            .to_string();
            let resp = super::super::handle_clone_unpack(body.as_bytes(), Some(owner()));
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
            assert!(!resp.body.contains(SENTINEL), "no value in {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert_eq!(v["vault_imported"], serde_json::json!(["STRIPE_KEY"]), "body: {}", resp.body);
            let id = v["project"]["id"].as_str().expect("project id").to_string();
            assert_eq!(value_of(&dst, &id, "STRIPE_KEY"), SENTINEL);

            let _ = pops::projects_delete(&id);
            let _ = pops::projects_delete(&bad_id);
            let _ = pops::projects_delete(&src_project.id);
        });
    }

    /// The pull pack job seals the carry into its status, and cleanup
    /// drops it with the bundle.
    #[test]
    fn pack_job_carries_the_sealed_secrets_until_cleanup() {
        let _g = pack_lock();
        let root = TempDir::new("k2-clone-vault-pack");
        let src = temp_vault("clone-pack-src");
        let dst = temp_vault("clone-pack-dst");
        let (_bundle, src_path, src_project) = source_with_secrets(&root, &src);
        let home = root.path().join("pack-home");
        fs::create_dir_all(&home).unwrap();

        let job_id = "pack-test-vault".to_string();
        super::super::insert_pack_job(super::super::PackJob::running(job_id.clone()));
        let opts = clone::CloneOptions { home_override: Some(home.clone()), ..Default::default() };
        let req = CarryRequest { target: target_of(&dst.v), caller: Some(owner()), vault: Ok(src.v.clone()) };
        super::super::run_pack_job(
            &job_id,
            &src_path,
            &opts,
            &home,
            k2_core::fs_commands::MAX_TRANSFER_SIZE,
            Some(&req),
        );
        let params = std::collections::HashMap::from([("job_id".to_string(), job_id.clone())]);
        let status = super::super::handle_clone_pack_status(&params);
        assert_eq!(status.status, "200 OK", "body: {}", status.body);
        assert!(!status.body.contains(SENTINEL), "no value in {}", status.body);
        let v: serde_json::Value = serde_json::from_str(&status.body).expect("json");
        assert_eq!(v["phase"], "done", "body: {}", status.body);
        assert_eq!(v["vault_carry"]["secrets"], serde_json::json!(["STRIPE_KEY"]));
        let carry = v["vault_carry_b64"].as_str().expect("carry in status").to_string();
        let file = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &carry).expect("b64");
        dst.v.carry_open(&file).expect("the destination vault opens the carry");

        let body = serde_json::json!({ "job_id": job_id }).to_string();
        let resp = super::super::handle_clone_pack_cleanup(body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let job = super::super::get_pack_job(&job_id).expect("job");
        assert!(job.vault_carry_b64.is_none(), "cleanup drops the carry");

        let _ = pops::projects_delete(&src_project.id);
    }
}
