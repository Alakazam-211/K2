//! Unit tests for the Clone-to bundle engine.
//!
//! Each test builds a synthetic workspace + a synthetic
//! `<home>/.claude/projects/<slug>/` under a fresh temp dir and points
//! [`CloneOptions::home_override`] at it, so the real home is NEVER
//! touched and the `~/.claude/...` resolution is fully hermetic. The slug
//! is derived from the canonicalized temp PROJECT path, exactly as the
//! engine computes it.

use super::*;
use crate::chat_history::claude_project_hash;
use std::collections::HashSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

// ── temp dir helper (no top-level tempfile dep; mirrors app_settings) ──
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
        // also mix a per-call counter so two temp dirs in one test differ.
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

/// A fully-wired synthetic agent: a workspace tree + a hermetic
/// `<home>/.claude/projects/<slug>/`.
struct Fixture {
    _root: TempDir,
    project: PathBuf,
    home: PathBuf,
    slug: String,
}

/// Build the synthetic workspace described in the PRD test plan.
fn build_fixture() -> Fixture {
    let root = TempDir::new("k2so-clone-test");
    let project = root.path().join("workspace");
    let home = root.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();

    // ── workspace tree ──────────────────────────────────────────────
    // a real project file
    write(&project.join("README.md"), "# My Agent\nproject docs\n");
    write(&project.join("src/main.rs"), "fn main() {}\n");
    // .k2so config (benign — must NOT be scrubbed)
    write(
        &project.join(".k2so/PROJECT.md"),
        "# Project\nfocus-group: blue\n",
    );
    // a secret env file with a JWT-shaped token
    write(
        &project.join(".env.local"),
        "TOKEN=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payloadpayloadpayload\n",
    );
    // bulk: node_modules at workspace root
    write(
        &project.join("node_modules/left-pad/index.js"),
        "module.exports = () => {};\n",
    );
    // .auth/ dir (secret + bulk)
    write(&project.join(".auth/session.json"), "{\"cookie\":\"abc\"}\n");

    // nested git repo: its own .git, a tracked file, and its own node_modules
    write(&project.join("nested/.git/HEAD"), "ref: refs/heads/main\n");
    write(&project.join("nested/.git/config"), "[core]\n");
    write(&project.join("nested/app.js"), "console.log('hi');\n");
    write(
        &project.join("nested/node_modules/dep/index.js"),
        "module.exports = 1;\n",
    );

    // ── hermetic ~/.claude/projects/<slug>/ ─────────────────────────
    // Slug is computed from the CANONICAL project path (the engine
    // canonicalizes), so canonicalize here too.
    let canon = fs::canonicalize(&project).unwrap();
    let slug = claude_project_hash(&canon.to_string_lossy());
    let slug_dir = home.join(".claude").join("projects").join(&slug);
    fs::create_dir_all(&slug_dir).unwrap();

    // memory dir (MEMORY.md + every *.md live INSIDE memory/)
    write(&slug_dir.join("memory/MEMORY.md"), "## Memory Index\n");
    write(&slug_dir.join("memory/a.md"), "memory a\n");
    write(&slug_dir.join("memory/b.md"), "memory b\n");

    // two session jsonl with DIFFERENT mtimes
    let old_session = slug_dir.join("11111111-1111-1111-1111-111111111111.jsonl");
    let new_session = slug_dir.join("22222222-2222-2222-2222-222222222222.jsonl");
    write(&old_session, "{\"type\":\"old\"}\n");
    write(&new_session, "{\"type\":\"new\"}\n");
    set_mtime(&old_session, 1_000_000);
    set_mtime(&new_session, 2_000_000); // newer

    // a worktree variant dir with its own session (only picked under
    // include_all_history)
    let wt_dir = home
        .join(".claude")
        .join("projects")
        .join(format!("{slug}-feature-x"));
    fs::create_dir_all(&wt_dir).unwrap();
    write(
        &wt_dir.join("33333333-3333-3333-3333-333333333333.jsonl"),
        "{\"type\":\"worktree\"}\n",
    );

    // The user-level credentials file — must NEVER be enumerated.
    write(
        &home.join(".claude").join(".credentials.json"),
        "{\"token\":\"super-secret-user-auth\"}\n",
    );

    Fixture {
        _root: root,
        project: canon,
        home,
        slug,
    }
}

/// Set an mtime (seconds since epoch) on a file so "newest mtime"
/// selection is deterministic regardless of write order/timing.
fn set_mtime(path: &Path, secs: u64) {
    let ft = filetime_secs(secs);
    set_file_mtime(path, ft);
}

// Minimal mtime setter via libc utimes (no `filetime` crate dep).
#[cfg(unix)]
fn set_file_mtime(path: &Path, secs: u64) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let tv = libc::timeval {
        tv_sec: secs as libc::time_t,
        tv_usec: 0,
    };
    let times = [tv, tv]; // atime, mtime
    let rc = unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) };
    assert_eq!(rc, 0, "utimes failed for {}", path.display());
}
#[cfg(unix)]
fn filetime_secs(secs: u64) -> u64 {
    secs
}

fn opts(home: &Path) -> CloneOptions {
    CloneOptions {
        home_override: Some(home.to_path_buf()),
        ..Default::default()
    }
}

fn rel_paths(inv: &CloneInventory, class: DestinationClass) -> HashSet<String> {
    inv.entries
        .iter()
        .filter(|e| e.class == class)
        .map(|e| e.rel_path.clone())
        .collect()
}

// ── tests ───────────────────────────────────────────────────────────

#[test]
fn slug_and_locations_resolve_at_slug_dir() {
    let fx = build_fixture();
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    // slug computed from the canonical project path
    assert_eq!(inv.slug, fx.slug, "slug must match claude_project_hash");
    assert_eq!(inv.project_path, fx.project.to_string_lossy());

    // memory resolved
    let mem = rel_paths(&inv, DestinationClass::Memory);
    assert!(mem.contains("MEMORY.md"), "MEMORY.md present, got {mem:?}");
    assert!(mem.contains("a.md"), "memory/a.md present");
    assert!(mem.contains("b.md"), "memory/b.md present");
    assert_eq!(mem.len(), 3, "exactly MEMORY.md + a.md + b.md");

    // sessions resolved under the slug dir (rel includes the slug prefix).
    // Default opts now include ALL history (GitHub #21): 2 slug-dir sessions
    // + 1 worktree variant.
    let sessions = rel_paths(&inv, DestinationClass::Session);
    assert_eq!(
        sessions.len(),
        3,
        "default = all history → 2 slug-dir + 1 worktree session, got {sessions:?}"
    );
}

#[test]
fn env_local_scrubbed_by_default_and_listed() {
    let fx = build_fixture();
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    let ws = rel_paths(&inv, DestinationClass::Workspace);
    assert!(
        !ws.contains(".env.local"),
        ".env.local must be scrubbed, got {ws:?}"
    );
    assert!(
        inv.scrubbed_secrets.contains(&".env.local".to_string()),
        "scrubbed list must record .env.local by rel path, got {:?}",
        inv.scrubbed_secrets
    );
    // benign .k2so config is NOT scrubbed
    assert!(
        ws.contains(".k2so/PROJECT.md"),
        "benign .k2so config kept, got {ws:?}"
    );
}

#[test]
fn carry_secrets_includes_env_local() {
    let fx = build_fixture();
    let mut o = opts(&fx.home);
    o.carry_secrets = true;
    let inv = inventory(&fx.project.to_string_lossy(), o).unwrap();

    let ws = rel_paths(&inv, DestinationClass::Workspace);
    assert!(
        ws.contains(".env.local"),
        "carry_secrets must include .env.local, got {ws:?}"
    );
    assert!(
        inv.scrubbed_secrets.is_empty(),
        "nothing scrubbed when carrying, got {:?}",
        inv.scrubbed_secrets
    );
}

#[test]
fn node_modules_excluded_workspace_and_nested_but_git_kept() {
    let fx = build_fixture();
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();
    let ws = rel_paths(&inv, DestinationClass::Workspace);

    // node_modules excluded at workspace root
    assert!(
        !ws.iter().any(|p| p.starts_with("node_modules/")),
        "workspace node_modules excluded, got {ws:?}"
    );
    // ...and inside the nested repo
    assert!(
        !ws.iter().any(|p| p.starts_with("nested/node_modules/")),
        "nested node_modules excluded, got {ws:?}"
    );
    // nested repo working tree IS present
    assert!(ws.contains("nested/app.js"), "nested tracked file kept");
    // nested .git IS preserved (direct copy, not a git op)
    assert!(
        ws.contains("nested/.git/HEAD"),
        "nested .git/HEAD must be kept, got {ws:?}"
    );
    assert!(
        ws.contains("nested/.git/config"),
        "nested .git/config must be kept, got {ws:?}"
    );
    // .auth/ excluded (bulk + secret)
    assert!(
        !ws.iter().any(|p| p.starts_with(".auth/")),
        ".auth/ excluded, got {ws:?}"
    );
}

#[test]
fn live_only_picks_newest_session() {
    let fx = build_fixture();
    // Opt OUT of the all-history default to get the slim live-only bundle.
    let mut o = opts(&fx.home);
    o.include_all_history = false;
    let inv = inventory(&fx.project.to_string_lossy(), o).unwrap();
    let sessions = rel_paths(&inv, DestinationClass::Session);
    assert_eq!(sessions.len(), 1);
    let only = sessions.iter().next().unwrap();
    assert!(
        only.ends_with("22222222-2222-2222-2222-222222222222.jsonl"),
        "live = newest mtime session, got {only}"
    );
    assert!(
        only.starts_with(&fx.slug),
        "session rel path keeps the slug-dir prefix, got {only}"
    );
}

#[test]
fn include_all_history_picks_both_and_worktree() {
    let fx = build_fixture();
    let mut o = opts(&fx.home);
    o.include_all_history = true;
    let inv = inventory(&fx.project.to_string_lossy(), o).unwrap();
    let sessions = rel_paths(&inv, DestinationClass::Session);
    assert_eq!(
        sessions.len(),
        3,
        "all-history = 2 slug-dir sessions + 1 worktree, got {sessions:?}"
    );
    assert!(sessions
        .iter()
        .any(|p| p.ends_with("11111111-1111-1111-1111-111111111111.jsonl")));
    assert!(sessions
        .iter()
        .any(|p| p.ends_with("22222222-2222-2222-2222-222222222222.jsonl")));
    // worktree variant keeps its `<slug>-<branch>/` prefix
    assert!(
        sessions
            .iter()
            .any(|p| p.starts_with(&format!("{}-feature-x/", fx.slug))),
        "worktree session under <slug>-feature-x/, got {sessions:?}"
    );
}

#[test]
fn credentials_json_never_enumerated() {
    let fx = build_fixture();
    let mut o = opts(&fx.home);
    o.carry_secrets = true; // even when carrying, this user-level file is out
    let inv = inventory(&fx.project.to_string_lossy(), o).unwrap();

    let all_paths: Vec<&String> = inv.entries.iter().map(|e| &e.rel_path).collect();
    assert!(
        !all_paths.iter().any(|p| p.contains(".credentials.json")),
        "~/.claude/.credentials.json must never be enumerated, got {all_paths:?}"
    );
    assert!(
        !inv.scrubbed_secrets
            .iter()
            .any(|p| p.contains(".credentials.json")),
        ".credentials.json must not even appear in the scrubbed list"
    );
}

#[test]
fn manifest_entries_have_correct_classes() {
    let fx = build_fixture();
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();
    let m = inv.manifest(&opts(&fx.home), "2026-06-05T00:00:00Z".to_string(), None);

    assert_eq!(m.source_slug, fx.slug);
    assert_eq!(m.created_at, "2026-06-05T00:00:00Z");
    assert!(!m.carry_secrets);
    // Default opts now carry all history (GitHub #21).
    assert!(m.include_all_history);

    // every class present
    let has = |c: DestinationClass| m.entries.iter().any(|e| e.class == c);
    assert!(has(DestinationClass::Workspace));
    assert!(has(DestinationClass::Memory));
    assert!(has(DestinationClass::Session));

    // re-supply: scrubbed secret recorded + standing re-auth items present
    assert!(m.reauth.secret_paths.contains(&".env.local".to_string()));
    assert!(
        m.reauth.items.iter().any(|i| i.contains("Claude Code auth")),
        "re-auth checklist includes remote Claude Code auth"
    );
    assert!(m.reauth.items.iter().any(|i| i.contains("MCP")));
}

#[test]
fn bundle_round_trips_secrets_absent_by_default() {
    let fx = build_fixture();
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    let out = fx._root.path().join("bundle.tar.gz");
    let built = build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-06-05T00:00:00Z".to_string(),
        None,
        &out,
    )
    .unwrap();
    assert!(built.exists(), "bundle written");

    // read manifest back
    let m = read_manifest_from_bundle(&out).unwrap();
    assert_eq!(m.source_slug, fx.slug);

    // untar into a fresh dir and inspect the layout
    let extract = fx._root.path().join("extract");
    fs::create_dir_all(&extract).unwrap();
    let names = untar(&out, &extract);

    assert!(names.contains("manifest.json"), "manifest at root");
    assert!(
        names.iter().any(|n| n == "workspace/README.md"),
        "workspace file under workspace/, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "memory/MEMORY.md"),
        "memory under memory/, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("sessions/")),
        "session under sessions/, got {names:?}"
    );
    // nested .git preserved through the bundle
    assert!(names.iter().any(|n| n == "workspace/nested/.git/HEAD"));
    // secrets absent by default
    assert!(
        !names.iter().any(|n| n.ends_with(".env.local")),
        ".env.local must be absent from the bundle, got {names:?}"
    );
    // node_modules absent
    assert!(!names.iter().any(|n| n.contains("node_modules")));

    // the extracted README content matches
    let readme = extract.join("workspace/README.md");
    let body = fs::read_to_string(readme).unwrap();
    assert!(body.contains("project docs"));
}

/// GitHub #21 regression: the DEFAULT bundle (no opt-out) must carry EVERY
/// session `.jsonl` — both slug-dir sessions AND the worktree variant —
/// through tar, not just the newest live one. A workspace with multiple
/// sessions is the migration case the default now serves.
#[test]
fn default_bundle_carries_all_sessions_through_tar() {
    let fx = build_fixture();
    // Plain default opts — the all-history default is what we're verifying.
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    let out = fx._root.path().join("all-sessions-bundle.tar.gz");
    build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-06-05T00:00:00Z".to_string(),
        None,
        &out,
    )
    .unwrap();

    let extract = fx._root.path().join("all-sessions-extract");
    fs::create_dir_all(&extract).unwrap();
    let names = untar(&out, &extract);

    let session_files: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("sessions/") && n.ends_with(".jsonl"))
        .collect();
    assert_eq!(
        session_files.len(),
        3,
        "default bundle carries all 3 sessions (2 slug-dir + 1 worktree), got {session_files:?}"
    );
    // Both slug-dir sessions present (the OLD one was previously dropped).
    assert!(
        names
            .iter()
            .any(|n| n.ends_with("11111111-1111-1111-1111-111111111111.jsonl")),
        "older slug-dir session must be bundled, got {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.ends_with("22222222-2222-2222-2222-222222222222.jsonl")),
        "newest slug-dir session must be bundled, got {names:?}"
    );
    // The worktree-variant session is bundled too.
    assert!(
        names
            .iter()
            .any(|n| n.ends_with("33333333-3333-3333-3333-333333333333.jsonl")),
        "worktree-variant session must be bundled, got {names:?}"
    );

    // The manifest records the all-history default.
    let m = read_manifest_from_bundle(&out).unwrap();
    assert!(
        m.include_all_history,
        "manifest must record all-history as the default"
    );
}

/// Regression: a symlink that points at a DIRECTORY inside the workspace
/// (the real-world `.k2so/external/agent-skills/.opencode/skills` case) must
/// be skipped, not appended as a file. Before the fix the walker — running
/// with follow_links(false) — saw a symlink (not a dir), let it through, and
/// `append_file`'s `File::open` followed it into the directory, crashing the
/// whole clone with EISDIR ("Is a directory", os error 21). A symlink to a
/// real FILE must still be copied (it resolves to a regular file).
#[test]
#[cfg(unix)]
fn symlink_to_dir_skipped_symlink_to_file_copied() {
    use std::os::unix::fs::symlink;
    let fx = build_fixture();

    // symlink-to-dir: the production crash path.
    let ext_dir = fx
        .project
        .join(".k2so/external/agent-skills/.opencode/realskills");
    fs::create_dir_all(&ext_dir).unwrap();
    write(&ext_dir.join("note.md"), "skill\n");
    symlink(
        &ext_dir,
        fx.project
            .join(".k2so/external/agent-skills/.opencode/skills"),
    )
    .unwrap();

    // symlink-to-file: must still be copied (resolves to a regular file).
    let real_file = fx.project.join("real-config.toml");
    write(&real_file, "key = 1\n");
    symlink(&real_file, fx.project.join("linked-config.toml")).unwrap();

    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    // The bundle MUST build (previously: EISDIR on the symlinked dir).
    let out = fx._root.path().join("symlink-bundle.tar.gz");
    build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-06-05T00:00:00Z".to_string(),
        None,
        &out,
    )
    .expect("bundle must build despite a symlink-to-directory in the tree");

    let extract = fx._root.path().join("symlink-extract");
    fs::create_dir_all(&extract).unwrap();
    let names = untar(&out, &extract);

    // The symlink-to-dir itself is NOT bundled.
    assert!(
        !names.iter().any(|n| n.ends_with(".opencode/skills")),
        "symlink-to-dir must be skipped, got {names:?}"
    );
    // The symlink-to-file IS copied, with its target's bytes.
    assert!(
        names.iter().any(|n| n == "workspace/linked-config.toml"),
        "symlink-to-file must be copied, got {names:?}"
    );
    let copied =
        fs::read_to_string(extract.join("workspace/linked-config.toml")).unwrap();
    assert!(copied.contains("key = 1"), "symlink-to-file content copied");
}

#[test]
fn bundle_carries_secrets_when_opted_in() {
    let fx = build_fixture();
    let mut o = opts(&fx.home);
    o.carry_secrets = true;
    let inv = inventory(&fx.project.to_string_lossy(), o.clone()).unwrap();
    let out = fx._root.path().join("bundle-carry.tar.gz");
    build_bundle(&inv, &o, "2026-06-05T00:00:00Z".to_string(), None, &out).unwrap();

    let extract = fx._root.path().join("extract-carry");
    fs::create_dir_all(&extract).unwrap();
    let names = untar(&out, &extract);
    assert!(
        names.iter().any(|n| n == "workspace/.env.local"),
        "carry_secrets bundles .env.local, got {names:?}"
    );
}

// ── GH#23: unpack-time embedded-path rewrite ────────────────────────
//
// These build a bundle BY HAND (manifest.json + one session entry) so the
// test fully controls `source_project_path` — including an arbitrary
// source path WITH SPACES and the back-compat empty-source case — then
// runs the real `unpack_bundle` and inspects the on-disk session `.jsonl`.

/// Tar+gz a `manifest.json` + the given session archive entries into
/// `out`. Each session entry is `(archive_rel, contents)` where
/// `archive_rel` starts with `sessions/`.
fn build_manual_bundle(out: &Path, manifest: &CloneManifest, sessions: &[(&str, &str)]) {
    let f = fs::File::create(out).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);

    let manifest_json = serde_json::to_vec(manifest).unwrap();
    let mut hdr = tar::Header::new_gnu();
    hdr.set_size(manifest_json.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_cksum();
    builder
        .append_data(&mut hdr, "manifest.json", &manifest_json[..])
        .unwrap();

    for (rel, contents) in sessions {
        let bytes = contents.as_bytes();
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        builder.append_data(&mut h, rel, bytes).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

/// A minimal manifest for the hand-built bundle. `source_project_path` is
/// arbitrary (the rewrite source); `source_slug` is its claude hash.
fn manual_manifest(source_project_path: &str) -> CloneManifest {
    CloneManifest {
        version: 1,
        source_slug: claude_project_hash(source_project_path),
        source_project_path: source_project_path.to_string(),
        created_at: "2026-06-05T00:00:00Z".to_string(),
        carry_secrets: false,
        include_all_history: true,
        entries: vec![],
        reauth: ReauthChecklist {
            secret_paths: vec![],
            items: vec![],
        },
        settings: None,
    }
}

#[test]
fn unpack_rewrites_embedded_cwd_for_arbitrary_spaced_paths() {
    let root = TempDir::new("gh23-unpack");
    // SOURCE machine path — arbitrary user/parent, WITH SPACES.
    let source = "/Users/z3thon/DevProjects/Nelson Specialty Industrial/nsi-plan01";
    // DEST machine: a real temp parent (also containing a space) + home.
    let dest_parent = root.path().join("AI Projects");
    fs::create_dir_all(&dest_parent).unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();

    // Session lines embed the SOURCE path in cwd, originalCwd, and a
    // tool-call file_path — the slug dir name uses the SOURCE slug shape.
    let session_line = format!(
        r#"{{"type":"user","cwd":{source:?},"originalCwd":{source:?},"tool":{{"file_path":{file:?}}}}}"#,
        file = format!("{source}/src/main.rs"),
    );
    let source_slug = claude_project_hash(source);
    let session_rel = format!("sessions/{source_slug}/abcd.jsonl");
    let worktree_rel = format!("sessions/{source_slug}-feature-x/efgh.jsonl");

    let manifest = manual_manifest(source);
    let bundle = root.path().join("bundle.tar.gz");
    build_manual_bundle(
        &bundle,
        &manifest,
        &[
            (&session_rel, &session_line),
            (&worktree_rel, &session_line),
        ],
    );

    let (res, _m) = unpack_bundle(&bundle, &dest_parent, &home).unwrap();
    let dest = res.dest_path.to_string_lossy().to_string();
    assert!(dest.contains("AI Projects"), "dest under spaced parent");
    assert_eq!(res.dest_path.file_name().unwrap(), "nsi-plan01");

    // The slug dir is the DEST slug (original→dest remap handled).
    let projects_dir = home.join(".claude").join("projects");
    let dest_slug = res.remote_slug.clone();
    let main_session = projects_dir.join(&dest_slug).join("abcd.jsonl");
    let wt_session = projects_dir
        .join(format!("{dest_slug}-feature-x"))
        .join("efgh.jsonl");
    assert!(main_session.exists(), "main session landed at dest slug dir");
    assert!(wt_session.exists(), "worktree session landed at dest slug dir");

    // Embedded paths rewritten SOURCE → DEST in BOTH the slug-dir session
    // and the worktree-variant session.
    for f in [&main_session, &wt_session] {
        let body = fs::read_to_string(f).unwrap();
        assert!(
            !body.contains(source),
            "no stale source path remains in {f:?}: {body}"
        );
        assert!(body.contains(&dest), "cwd rewritten to dest in {f:?}: {body}");
        assert!(
            body.contains(&format!("{dest}/src/main.rs")),
            "tool-call file_path rewritten in {f:?}: {body}"
        );
    }
}

#[test]
fn unpack_no_rewrite_when_source_equals_dest() {
    // When the source path happens to equal the dest path (same-machine
    // clone), the rewrite is a no-op — content is byte-identical.
    let root = TempDir::new("gh23-unpack-same");
    let dest_parent = root.path().join("parent");
    fs::create_dir_all(&dest_parent).unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();

    // Make source == the dest path the unpack WILL compute.
    let source = dest_parent.join("proj").to_string_lossy().to_string();
    let source_slug = claude_project_hash(&source);
    let line = format!(r#"{{"cwd":{source:?},"x":1}}"#);
    let rel = format!("sessions/{source_slug}/s.jsonl");

    let bundle = root.path().join("b.tar.gz");
    build_manual_bundle(&bundle, &manual_manifest(&source), &[(&rel, &line)]);

    let (res, _m) = unpack_bundle(&bundle, &dest_parent, &home).unwrap();
    let f = home
        .join(".claude")
        .join("projects")
        .join(&res.remote_slug)
        .join("s.jsonl");
    let body = fs::read_to_string(&f).unwrap();
    assert!(body.contains(&source), "same path retained, got {body}");
}

#[test]
fn unpack_back_compat_skips_when_manifest_lacks_source_path() {
    // A bundle whose manifest has an EMPTY source_project_path (older /
    // hand-built) must NOT crash and must NOT rewrite — the session file's
    // embedded cwd is left as-is (degrades to pre-fix behavior). The slug
    // remap still happens so the file still lands in the right dir.
    let root = TempDir::new("gh23-unpack-backcompat");
    let dest_parent = root.path().join("parent");
    fs::create_dir_all(&dest_parent).unwrap();
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let stale = "/Users/whoever/old/place/proj";
    let stale_slug = claude_project_hash(stale);
    let line = format!(r#"{{"cwd":{stale:?},"x":1}}"#);
    let rel = format!("sessions/{stale_slug}/s.jsonl");

    // Empty source_project_path → rewrite disabled.
    let mut manifest = manual_manifest("");
    // basename of "" → falls back to "workspace"; give a real source_slug
    // so the slug-remap branch is exercised against the stale dir name.
    manifest.source_slug = stale_slug.clone();
    manifest.source_project_path = String::new();

    let bundle = root.path().join("b.tar.gz");
    build_manual_bundle(&bundle, &manifest, &[(&rel, &line)]);

    let (res, _m) = unpack_bundle(&bundle, &dest_parent, &home).unwrap();
    let f = home
        .join(".claude")
        .join("projects")
        .join(&res.remote_slug)
        .join("s.jsonl");
    assert!(f.exists(), "session still placed at recomputed slug dir");
    let body = fs::read_to_string(&f).unwrap();
    assert!(
        body.contains(stale),
        "back-compat: no source path on manifest → embedded cwd left untouched, got {body}"
    );
}

/// Untar a `.tar.gz` into `dest`, returning the set of archived entry
/// names (`/`-joined, relative to the archive root).
fn untar(bundle: &Path, dest: &Path) -> HashSet<String> {
    let f = fs::File::open(bundle).unwrap();
    let dec = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(dec);
    let mut names = HashSet::new();
    for entry in ar.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_path_buf();
        names.insert(path.to_string_lossy().replace('\\', "/"));
        let out = dest.join(&path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        fs::write(&out, &buf).unwrap();
    }
    names
}

// ── direct credential-scanner unit checks ──────────────────────────
#[test]
fn credential_scanner_catches_expected_patterns() {
    use super::scrub::content_has_credential_str as scan;
    assert!(scan("x = eyJabcdefghijklmnopqrstuvwxyz0123"), "JWT");
    assert!(scan("GH=ghp_abcdefghijklmnopqrstuvwxyz12"), "ghp_ token");
    assert!(scan("key: ghs_ABCDEFGHIJKLMNOPQRSTuvwx9999"), "ghs_ token");
    assert!(scan("role = service_role"), "service_role");
    assert!(scan("-----BEGIN PRIVATE KEY-----"), "private key");
    assert!(scan("password: hunter2"), "password assignment");
    assert!(scan("secret = topsecret"), "secret assignment");
    assert!(scan("api_key: abc123"), "api_key assignment");
    assert!(scan("API-KEY = abc123"), "api-key case/sep variant");

    // negatives
    assert!(!scan("just some prose about passwords in general"));
    assert!(!scan("the password field was empty:"), "empty value → no hit");
    assert!(!scan("gh_short_token"), "gh token below length floor");
    assert!(!scan("focus-group: blue"), "benign k2so config");
}

// ── settings capture + manifest round-trip ──────────────────────────

/// Insert a synthetic `projects` row, capture its USER-meaningful
/// settings, and assert the machine-specific fields are excluded.
#[test]
fn settings_captured_from_project_row() {
    use crate::db::schema::Project;

    let conn = crate::db::isolated_test_connection();
    let project_path = "/tmp/some-cloned-agent";

    Project::create(
        &conn,
        "proj-clone-1",
        "Cloned Agent",
        project_path,
        "#ff8800",
        0,
        2, // worktree_mode
        None,
        None,
    )
    .expect("create synthetic project");
    // agent_mode is set via update (create defaults it to "off"); this
    // also syncs agent_enabled = 1.
    Project::update(
        &conn,
        "proj-clone-1",
        None,                          // name
        None,                          // path
        None,                          // color
        None,                          // tab_order
        None,                          // worktree_mode
        None,                          // icon_url
        None,                          // focus_group_id
        None,                          // pinned
        None,                          // manually_active
        None,                          // agent_enabled (synced via agent_mode)
        Some(1),                       // heartbeat_enabled
        Some("manager".to_string()),   // agent_mode → agent_enabled = 1
        None,                          // state_id
        None,                          // heartbeat_mode
        None,                          // heartbeat_schedule
    )
    .expect("set agent_mode + heartbeat");

    // d410883: `heartbeat_enabled` is a LIVE aggregate over
    // workspace_heartbeats — the legacy projects column (set to 1 by
    // the update above, deliberately kept) must be IGNORED by capture.
    // Seed a real enabled heartbeat so the captured flag is truthfully
    // ON via the aggregate, not the stale column.
    crate::db::schema::AgentHeartbeat::insert(
        &conn,
        "hb-clone-1",
        "proj-clone-1",
        "daily-check",
        "daily",
        "{}",
        "/tmp/some-cloned-agent/.k2so/heartbeats/daily-check/WAKEUP.md",
        true,
    )
    .expect("seed heartbeat row");

    let captured = capture_settings(&conn, project_path)
        .expect("capture must not error")
        .expect("row exists → Some");

    assert_eq!(captured.agent_mode, "manager");
    assert!(captured.agent_enabled, "manager → enabled");
    assert!(
        captured.heartbeat_enabled,
        "live aggregate: enabled heartbeat row → captured ON"
    );
    assert_eq!(captured.name, "Cloned Agent");
    assert_eq!(captured.color, "#ff8800");
    assert_eq!(captured.worktree_mode, 2);

    // Trailing-slash normalization still finds the row.
    let via_slash = capture_settings(&conn, "/tmp/some-cloned-agent/")
        .expect("no error")
        .expect("trailing-slash path resolves");
    assert_eq!(via_slash, captured);

    // Unregistered path → None (graceful).
    let none = capture_settings(&conn, "/tmp/not-a-project").expect("no error");
    assert!(none.is_none(), "unregistered path yields None, got {none:?}");
}

/// Settings round-trip: capture → build_bundle → read_manifest_from_bundle
/// → the same `WorkspaceSettings` come back.
#[test]
fn settings_round_trip_through_bundle() {
    use crate::db::schema::Project;

    let fx = build_fixture();

    // Register the synthetic workspace as a project so capture finds it.
    let conn = crate::db::isolated_test_connection();
    let path_str = fx.project.to_string_lossy().to_string();
    Project::create(
        &conn,
        "proj-rt-1",
        "Round Trip",
        &path_str,
        "#112233",
        0,
        1,
        None,
        None,
    )
    .expect("create project");
    Project::update(
        &conn,
        "proj-rt-1",
        None,                          // name
        None,                          // path
        None,                          // color
        None,                          // tab_order
        None,                          // worktree_mode
        None,                          // icon_url
        None,                          // focus_group_id
        None,                          // pinned
        None,                          // manually_active
        None,                          // agent_enabled
        Some(0),                       // heartbeat_enabled = false
        Some("pod".to_string()),       // agent_mode
        None,                          // state_id
        None,                          // heartbeat_mode
        None,                          // heartbeat_schedule
    )
    .expect("set agent_mode");

    let settings = capture_settings(&conn, &path_str)
        .expect("no error")
        .expect("Some");

    let inv = inventory(&path_str, opts(&fx.home)).unwrap();
    let out = fx._root.path().join("bundle-settings.tar.gz");
    build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-06-05T00:00:00Z".to_string(),
        Some(settings.clone()),
        &out,
    )
    .unwrap();

    let m = read_manifest_from_bundle(&out).unwrap();
    let got = m.settings.expect("manifest carries settings");
    assert_eq!(got, settings, "settings round-trip intact");
    assert_eq!(got.agent_mode, "pod");
    assert!(got.agent_enabled, "pod → enabled");
    // No workspace_heartbeats rows seeded → the d410883 live aggregate
    // is OFF regardless of the legacy projects column.
    assert!(!got.heartbeat_enabled);
    assert_eq!(got.name, "Round Trip");
    assert_eq!(got.color, "#112233");
    assert_eq!(got.worktree_mode, 1);
}
