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

/// Layer a realistic `.gitignore` + a `.k2/` agent dir onto a fixture —
/// the exact shape that used to make "Clone to server" silently drop the
/// whole agent dir (`/.k2/`) and the env file (`.env*`): the main walk
/// honors `.gitignore`, so both vanished BEFORE the carry_secrets logic
/// could decide.
fn add_gitignored_agent_state(fx: &Fixture) {
    write(
        &fx.project.join(".gitignore"),
        "node_modules/\n.env\n.env.*\n/.k2/\n",
    );
    write(
        &fx.project.join(".k2/agent/ROLE.md"),
        "# Agent\npersona and standing orders\n",
    );
    write(
        &fx.project.join(".k2/skills/x/SKILL.md"),
        "# Skill X\nhow to do the thing\n",
    );
}

/// Rosson 2026-07-07: a gitignored `.k2/` dir must STILL travel — agent
/// state is the whole point of a clone. Also proves the cross-pass dedupe:
/// `.k2so/PROJECT.md` is reachable by both the main walk (not gitignored)
/// and the force-include pass, and must appear exactly once.
#[test]
fn gitignored_k2_dir_force_included() {
    let fx = build_fixture();
    add_gitignored_agent_state(&fx);
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    let ws = rel_paths(&inv, DestinationClass::Workspace);
    assert!(
        ws.contains(".k2/agent/ROLE.md"),
        "gitignored .k2/ agent file must be force-included, got {ws:?}"
    );
    assert!(
        ws.contains(".k2/skills/x/SKILL.md"),
        "gitignored .k2/ skill file must be force-included, got {ws:?}"
    );
    // dedupe: a dot-dir file the main walk ALSO collected appears once.
    let dup_count = inv
        .entries
        .iter()
        .filter(|e| e.rel_path == ".k2so/PROJECT.md")
        .count();
    assert_eq!(
        dup_count, 1,
        ".k2so/PROJECT.md must be deduped across the main + force-include walks"
    );

    // ...and the files survive all the way through the tar bundle.
    let out = fx._root.path().join("k2-force-include-bundle.tar.gz");
    build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-07-07T00:00:00Z".to_string(),
        None,
        None,
        vec![],
        &out,
    )
    .unwrap();
    let extract = fx._root.path().join("k2-force-include-extract");
    fs::create_dir_all(&extract).unwrap();
    let names = untar(&out, &extract);
    assert!(
        names.iter().any(|n| n == "workspace/.k2/agent/ROLE.md"),
        "gitignored .k2/ file must reach the bundle, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "workspace/.k2/skills/x/SKILL.md"),
        "gitignored .k2/ skill must reach the bundle, got {names:?}"
    );
}

/// Rosson 2026-07-07: a gitignored `.env.local` + carry_secrets=false must
/// be scrubbed AND listed in the re-supply report — previously `.gitignore`
/// dropped it before the classifier ever saw it, so it silently vanished
/// from both the bundle and the scrubbed list.
#[test]
fn gitignored_env_local_scrubbed_and_listed() {
    let fx = build_fixture();
    add_gitignored_agent_state(&fx);
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    let ws = rel_paths(&inv, DestinationClass::Workspace);
    assert!(
        !ws.contains(".env.local"),
        "carry_secrets=false: .env.local stays out of the bundle, got {ws:?}"
    );
    assert!(
        inv.scrubbed_secrets.contains(&".env.local".to_string()),
        "gitignored .env.local must STILL appear in scrubbed_secrets, got {:?}",
        inv.scrubbed_secrets
    );
}

/// Rosson 2026-07-07: the "Include secrets" toggle must truthfully control
/// `.env` travel — a gitignored `.env.local` + carry_secrets=true IS
/// bundled (this was the silently-broken case).
#[test]
fn gitignored_env_local_carried_when_opted_in() {
    let fx = build_fixture();
    add_gitignored_agent_state(&fx);
    let mut o = opts(&fx.home);
    o.carry_secrets = true;
    let inv = inventory(&fx.project.to_string_lossy(), o).unwrap();

    let ws = rel_paths(&inv, DestinationClass::Workspace);
    assert!(
        ws.contains(".env.local"),
        "carry_secrets=true must include the gitignored .env.local, got {ws:?}"
    );
    // dedupe sanity: exactly one entry even though the secrets pass also
    // enumerates env files.
    let count = inv
        .entries
        .iter()
        .filter(|e| e.rel_path == ".env.local")
        .count();
    assert_eq!(count, 1, ".env.local must appear exactly once, got {count}");
    assert!(
        inv.scrubbed_secrets.is_empty(),
        "nothing scrubbed when carrying, got {:?}",
        inv.scrubbed_secrets
    );
}

/// The secret content-gate applies to the CURRENT `.k2/` dot-dir (not just
/// the legacy `.k2so/`): a credential-bearing file inside a force-included
/// `.k2/` is still scrubbed when not carrying secrets, while its benign
/// siblings travel.
#[test]
fn k2_credential_file_scrubbed_despite_force_include() {
    let fx = build_fixture();
    add_gitignored_agent_state(&fx);
    // A JWT-shaped token inside the gitignored .k2/ dir.
    write(
        &fx.project.join(".k2/creds/token.txt"),
        "TOKEN=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payloadpayloadpayload\n",
    );
    let inv = inventory(&fx.project.to_string_lossy(), opts(&fx.home)).unwrap();

    let ws = rel_paths(&inv, DestinationClass::Workspace);
    assert!(
        !ws.contains(".k2/creds/token.txt"),
        "credential-bearing .k2/ file must be scrubbed, got {ws:?}"
    );
    assert!(
        inv.scrubbed_secrets
            .contains(&".k2/creds/token.txt".to_string()),
        "scrubbed .k2/ credential listed for re-supply, got {:?}",
        inv.scrubbed_secrets
    );
    // benign neighbors still travel.
    assert!(
        ws.contains(".k2/agent/ROLE.md"),
        "benign .k2/ files still included, got {ws:?}"
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
    let m = inv.manifest(
        &opts(&fx.home),
        "2026-06-05T00:00:00Z".to_string(),
        None,
        None,
        vec![],
    );

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
        None,
        vec![],
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
        None,
        vec![],
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
        None,
        vec![],
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
    build_bundle(
        &inv,
        &o,
        "2026-06-05T00:00:00Z".to_string(),
        None,
        None,
        vec![],
        &out,
    )
    .unwrap();

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
        pinned_chat: None,
        chat_pins: vec![],
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
        None,                          // default_agent
        None,                          // default_model
        None,                          // force_model_on_resume
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
        None,                          // default_agent
        None,                          // default_model
        None,                          // force_model_on_resume
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
        None,
        vec![],
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

// ── pinned-chat identity capture + apply ────────────────────────────

/// Seed a projects row + workspace_sessions session_id, then assert
/// `capture_pinned_chat` returns the identity (and path normalization
/// + empty/missing cases are graceful).
#[test]
fn capture_pinned_chat_from_workspace_sessions() {
    use crate::db::schema::{Project, WorkspaceSession};

    let conn = crate::db::isolated_test_connection();
    let project_path = "/tmp/pinned-chat-agent";

    Project::create(
        &conn,
        "proj-pin-1",
        "Pinned Agent",
        project_path,
        "#00aa88",
        0,
        0,
        None,
        None,
    )
    .expect("create project");

    // No workspace_sessions row yet → None.
    let none = capture_pinned_chat(&conn, project_path).expect("no error");
    assert!(none.is_none(), "no session row → None, got {none:?}");

    WorkspaceSession::upsert(
        &conn,
        "ws-row-1",
        "proj-pin-1",
        None,
        Some("sess-uuid-abc"),
        "claude",
        "system",
        "stopped",
    )
    .expect("upsert workspace session");

    let pin = capture_pinned_chat(&conn, project_path)
        .expect("capture must not error")
        .expect("session_id present → Some");
    assert_eq!(pin.session_id, "sess-uuid-abc");
    assert_eq!(pin.harness, "claude");

    // Trailing-slash normalization.
    let via_slash = capture_pinned_chat(&conn, "/tmp/pinned-chat-agent/")
        .expect("no error")
        .expect("trailing slash resolves");
    assert_eq!(via_slash, pin);

    // Null/cleared session_id → None.
    WorkspaceSession::clear_session_id(&conn, "proj-pin-1").expect("clear session_id");
    let after_clear = capture_pinned_chat(&conn, project_path).expect("no error");
    assert!(
        after_clear.is_none(),
        "cleared session_id → None, got {after_clear:?}"
    );

    // Unregistered path → None.
    let unreg = capture_pinned_chat(&conn, "/tmp/not-registered").expect("no error");
    assert!(unreg.is_none());
}

/// `capture_chat_pins` returns only pinned or named rows for the given
/// session ids; unmentioned / blank rows are excluded.
#[test]
fn capture_chat_pins_filters_by_session_and_meaning() {
    let conn = crate::db::isolated_test_connection();

    // Seed three chat_session_names rows:
    // 1. pinned + named (keep)
    // 2. unpinned + named (keep)
    // 3. unpinned + empty name (drop)
    // 4. pinned for a session_id we won't query (drop — not in list)
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES ('claude', 'sid-a', 'Alpha', 1, unixepoch())",
        [],
    )
    .expect("insert pin a");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES ('claude', 'sid-b', 'Bravo', 0, unixepoch())",
        [],
    )
    .expect("insert name b");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES ('claude', 'sid-c', '', 0, unixepoch())",
        [],
    )
    .expect("insert blank c");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES ('grok', 'sid-other', 'Other', 1, unixepoch())",
        [],
    )
    .expect("insert other");

    let pins = capture_chat_pins(
        &conn,
        &["sid-a".into(), "sid-b".into(), "sid-c".into()],
    )
    .expect("capture pins");

    assert_eq!(
        pins.len(),
        2,
        "only pinned/named among queried ids, got {pins:?}"
    );
    let a = pins.iter().find(|p| p.session_id == "sid-a").expect("sid-a");
    assert_eq!(a.custom_name, "Alpha");
    assert!(a.pinned);
    assert_eq!(a.provider, "claude");
    let b = pins.iter().find(|p| p.session_id == "sid-b").expect("sid-b");
    assert_eq!(b.custom_name, "Bravo");
    assert!(!b.pinned);

    // Empty session list → empty pins (no error).
    let empty = capture_chat_pins(&conn, &[]).expect("empty ok");
    assert!(empty.is_empty());
}

/// Round-trip: capture → build_bundle → read_manifest → apply_clone_identity
/// re-stamps workspace_sessions + chat_session_names on a dest project.
#[test]
fn identity_round_trip_through_bundle_and_apply() {
    use crate::db::schema::{Project, WorkspaceSession};

    let fx = build_fixture();
    let conn = crate::db::isolated_test_connection();
    let path_str = fx.project.to_string_lossy().to_string();

    Project::create(
        &conn,
        "proj-id-rt",
        "Identity RT",
        &path_str,
        "#abcdef",
        0,
        0,
        None,
        None,
    )
    .expect("create source project");

    // Fixture has sessions: 1111…, 2222…, and a worktree session.
    // Pin the live-ish one.
    let pinned_sid = "11111111-1111-1111-1111-111111111111";
    WorkspaceSession::upsert(
        &conn,
        "ws-rt",
        "proj-id-rt",
        None,
        Some(pinned_sid),
        "claude",
        "system",
        "stopped",
    )
    .expect("seed pinned chat");
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES ('claude', ?1, 'Live Chat', 1, unixepoch())",
        rusqlite::params![pinned_sid],
    )
    .expect("seed chat pin");

    let pinned = capture_pinned_chat(&conn, &path_str)
        .expect("capture pin")
        .expect("Some");
    let inv = inventory(&path_str, opts(&fx.home)).unwrap();
    let session_ids = session_ids_from_entries(
        inv.entries
            .iter()
            .map(|e| (e.class, e.rel_path.clone())),
    );
    assert!(
        session_ids.iter().any(|id| id == pinned_sid),
        "inventory must include pinned session stem, got {session_ids:?}"
    );
    let pins = capture_chat_pins(&conn, &session_ids).expect("capture pins");
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].custom_name, "Live Chat");

    let out = fx._root.path().join("bundle-identity.tar.gz");
    build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-06-05T00:00:00Z".to_string(),
        None,
        Some(pinned.clone()),
        pins.clone(),
        &out,
    )
    .unwrap();

    let m = read_manifest_from_bundle(&out).unwrap();
    assert_eq!(m.pinned_chat.as_ref(), Some(&pinned));
    assert_eq!(m.chat_pins, pins);

    // Simulate a clean dest register on the SAME path (slug matches fixture
    // sessions). Cannot insert a second projects row with the same path
    // (UNIQUE) — clear identity rows then re-apply like unpack would.
    conn.execute(
        "DELETE FROM workspace_sessions WHERE project_id = 'proj-id-rt'",
        [],
    )
    .expect("clear workspace_sessions");
    conn.execute(
        "DELETE FROM chat_session_names WHERE session_id = ?1",
        rusqlite::params![pinned_sid],
    )
    .expect("clear chat pin for re-apply");

    apply_clone_identity(
        &conn,
        "proj-id-rt",
        &path_str,
        Some(&fx.home),
        &m,
    )
    .expect("apply identity");

    let dest_ws = WorkspaceSession::get(&conn, "proj-id-rt")
        .expect("db ok")
        .expect("workspace_sessions row stamped");
    assert_eq!(
        dest_ws.session_id.as_deref(),
        Some(pinned_sid),
        "pinned session_id applied"
    );
    assert_eq!(dest_ws.harness, "claude");

    let name: String = conn
        .query_row(
            "SELECT custom_name FROM chat_session_names \
             WHERE provider = 'claude' AND session_id = ?1",
            rusqlite::params![pinned_sid],
            |row| row.get(0),
        )
        .expect("chat pin row exists");
    assert_eq!(name, "Live Chat");
    let pinned_flag: i64 = conn
        .query_row(
            "SELECT pinned FROM chat_session_names \
             WHERE provider = 'claude' AND session_id = ?1",
            rusqlite::params![pinned_sid],
            |row| row.get(0),
        )
        .expect("pinned flag");
    assert_eq!(pinned_flag, 1);
}

#[test]
fn session_ids_from_entries_takes_file_stems() {
    let ids = session_ids_from_entries([
        (
            DestinationClass::Session,
            "slug/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl".into(),
        ),
        (
            DestinationClass::Session,
            "slug-branch/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb.jsonl".into(),
        ),
        (DestinationClass::Workspace, "README.md".into()),
        (DestinationClass::Memory, "MEMORY.md".into()),
        // Dedup same stem.
        (
            DestinationClass::Session,
            "slug/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl".into(),
        ),
    ]);
    assert_eq!(
        ids,
        vec![
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string(),
            "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".to_string(),
        ]
    );
}

// ── Multi-provider session clone (C1–C3) ─────────────────────────────

/// Seed Gemini + Pi + Codex + Grok + Cursor (+ hermes) under a hermetic
/// HOME for the fixture workspace path, then assert inventory picks them
/// up as Provider entries, never auth/credentials, and a full bundle →
/// unpack rewrites paths and re-keys Cursor MD5 / Grok encode.
fn seed_provider_fixtures(home: &Path, project: &str) {
    // Gemini: projects.json + tmp/<slug>/chats/*.jsonl
    let gemini_slug = "ws-slug-abc";
    write(
        &home.join(".gemini/projects.json"),
        &format!(
            r#"{{"projects":{{"{project}":"{gemini_slug}","/other/proj":"other-slug"}}}}"#
        ),
    );
    write(
        &home
            .join(".gemini/tmp")
            .join(gemini_slug)
            .join("chats")
            .join("session-1.jsonl"),
        &format!(
            r#"{{"sessionId":"gem-uuid-1","cwd":"{project}","startTime":"2026-07-01T00:00:00Z"}}
{{"type":"user","content":"hello from {project}"}}
"#
        ),
    );
    // Foreign gemini project — must NOT be inventoried.
    write(
        &home
            .join(".gemini/tmp/other-slug/chats/foreign.jsonl"),
        r#"{"sessionId":"gem-foreign","cwd":"/other/proj"}
"#,
    );

    // Pi: line-1 cwd filter
    write(
        &home
            .join(".pi/agent/sessions/some-slug")
            .join("2026-07-01T00-00-00_pi-uuid-1.jsonl"),
        &format!(
            r#"{{"type":"session","id":"pi-uuid-1","cwd":"{project}","timestamp":"2026-07-01T00:00:00Z"}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"hi"}}]}}}}
"#
        ),
    );
    write(
        &home
            .join(".pi/agent/sessions/other-slug")
            .join("other.jsonl"),
        r#"{"type":"session","id":"pi-other","cwd":"/other/proj","timestamp":"2026-07-01T00:00:00Z"}
"#,
    );

    // Codex: payload.cwd; do NOT seed history.jsonl as required inventory
    write(
        &home
            .join(".codex/sessions/2026/07/01")
            .join("rollout-2026-07-01T00-00-00-codex-uuid-1.jsonl"),
        &format!(
            r#"{{"type":"session_meta","payload":{{"id":"codex-uuid-1","cwd":"{project}","timestamp":"2026-07-01T00:00:00Z"}}}}
{{"type":"event","payload":{{"text":"hi"}}}}
"#
        ),
    );
    // global history index — must never be bundled
    write(
        &home.join(".codex/history.jsonl"),
        r#"{"session_id":"codex-uuid-1","ts":1,"text":"title"}
"#,
    );

    // Grok: percent-encoded cwd dir + summary; skip subagent + auth
    let encoded = super::providers::grok_percent_encode_cwd(project);
    let grok_sid = "01920000-cccc-7000-8000-000000000001";
    write(
        &home
            .join(".grok/sessions")
            .join(&encoded)
            .join(grok_sid)
            .join("summary.json"),
        &format!(
            r#"{{"info":{{"id":"{grok_sid}","cwd":"{project}"}},"generated_title":"Grok chat","last_active_at":"2026-07-03T10:00:00Z"}}"#
        ),
    );
    write(
        &home
            .join(".grok/sessions")
            .join(&encoded)
            .join(grok_sid)
            .join("chat_history.jsonl"),
        &format!(r#"{{"type":"user","content":"path was {project}"}}"#),
    );
    // auth.json at grok root — NEVER inventory
    write(
        &home.join(".grok/auth.json"),
        r#"{"token":"must-not-travel"}"#,
    );
    // subagent session — skip
    let sub_sid = "01920000-cccc-7000-8000-000000000099";
    write(
        &home
            .join(".grok/sessions")
            .join(&encoded)
            .join(sub_sid)
            .join("summary.json"),
        &format!(
            r#"{{"info":{{"id":"{sub_sid}","cwd":"{project}"}},"session_kind":"subagent","last_active_at":"2026-07-03T12:00:00Z"}}"#
        ),
    );

    // Cursor: md5(project)/uuid/store.db
    let md5 = crate::chat_history::md5_hex(project.as_bytes());
    let cursor_uuid = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    write(
        &home
            .join(".cursor/chats")
            .join(&md5)
            .join(cursor_uuid)
            .join("store.db"),
        &format!("cursor-sqlite-placeholder containing {project}"),
    );
    // IDE account DB must never be inventoried (we never walk Application Support)
    write(
        &home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
        "account-db-must-not-travel",
    );

    // Hermes: RO state.db with family + foreign sessions
    let db_path = home.join(".hermes/state.db");
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.pragma_update(None, "journal_mode", "wal").unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
             id TEXT PRIMARY KEY,
             source TEXT NOT NULL,
             parent_session_id TEXT,
             started_at REAL NOT NULL,
             ended_at REAL,
             end_reason TEXT,
             message_count INTEGER DEFAULT 0,
             title TEXT,
             cwd TEXT,
             git_branch TEXT,
             git_repo_root TEXT,
             archived INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE messages (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             session_id TEXT NOT NULL REFERENCES sessions(id),
             role TEXT NOT NULL,
             content TEXT,
             tool_calls TEXT,
             timestamp REAL NOT NULL
         );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, source, cwd, title, started_at, archived) \
         VALUES ('20260701_090000_aaaaaa', 'cli', ?1, 'Hermes chat', 1000.0, 0)",
        rusqlite::params![project],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) \
         VALUES ('20260701_090000_aaaaaa', 'user', ?1, 1001.0)",
        rusqlite::params![format!("work in {project}")],
    )
    .unwrap();
    // Foreign project row — must not export
    conn.execute(
        "INSERT INTO sessions (id, source, cwd, title, started_at, archived) \
         VALUES ('20260629_070000_eeeeee', 'cli', '/other/proj', 'Foreign', 500.0, 0)",
        [],
    )
    .unwrap();
}

#[test]
fn multi_provider_inventory_finds_sessions_skips_credentials() {
    let fx = build_fixture();
    let project = fx.project.to_string_lossy().to_string();
    seed_provider_fixtures(&fx.home, &project);

    let inv = inventory(&project, opts(&fx.home)).unwrap();
    let providers = rel_paths(&inv, DestinationClass::Provider);

    // Gemini
    assert!(
        providers.iter().any(|p| p.starts_with("gemini/tmp/") && p.ends_with(".jsonl")),
        "gemini session inventoried, got {providers:?}"
    );
    assert!(
        !providers.iter().any(|p| p.contains("other-slug")),
        "foreign gemini project must not be inventoried"
    );

    // Pi
    assert!(
        providers.iter().any(|p| p.contains("pi/agent/sessions/") && p.contains("pi-uuid")),
        "pi session inventoried, got {providers:?}"
    );

    // Codex
    assert!(
        providers
            .iter()
            .any(|p| p.starts_with("codex/sessions/") && p.contains("rollout-")),
        "codex rollout inventoried, got {providers:?}"
    );
    assert!(
        !providers.iter().any(|p| {
            // Exact filename history.jsonl only — not Grok's chat_history.jsonl.
            std::path::Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                == Some("history.jsonl")
        }),
        "codex history.jsonl must not travel, got {providers:?}"
    );

    // Grok
    assert!(
        providers.iter().any(|p| p.contains("grok/sessions/") && p.ends_with("summary.json")),
        "grok summary inventoried, got {providers:?}"
    );
    assert!(
        !providers.iter().any(|p| p.contains("auth")),
        "grok auth must never be inventoried, got {providers:?}"
    );
    assert!(
        !providers.iter().any(|p| p.contains("000000000099")),
        "grok subagent must be skipped"
    );

    // Cursor
    let md5 = crate::chat_history::md5_hex(project.as_bytes());
    assert!(
        providers
            .iter()
            .any(|p| p.contains(&format!("cursor/chats/{md5}/")) && p.ends_with("store.db")),
        "cursor store.db inventoried, got {providers:?}"
    );
    assert!(
        !providers.iter().any(|p| p.contains("globalStorage") || p.contains("state.vscdb")),
        "Cursor IDE account DB must never be inventoried"
    );

    // Hermes export (not whole state.db)
    assert!(
        providers.iter().any(|p| p == "hermes/export.json"),
        "hermes export.json inventoried, got {providers:?}"
    );
    assert!(
        !providers.iter().any(|p| p.ends_with("state.db")),
        "hermes state.db must never ship wholesale"
    );

    // Claude credentials still out
    let all: Vec<_> = inv.entries.iter().map(|e| e.rel_path.as_str()).collect();
    assert!(!all.iter().any(|p| p.contains("credentials") || p.contains("auth.json")));
}

#[test]
fn multi_provider_bundle_unpack_round_trip() {
    let fx = build_fixture();
    let project = fx.project.to_string_lossy().to_string();
    seed_provider_fixtures(&fx.home, &project);

    let inv = inventory(&project, opts(&fx.home)).unwrap();
    let bundle = fx._root.path().join("providers-bundle.tar.gz");
    build_bundle(
        &inv,
        &opts(&fx.home),
        "2026-07-21T00:00:00Z".to_string(),
        None,
        None,
        vec![],
        &bundle,
    )
    .unwrap();

    // Unpack into a fresh home + dest parent
    let dest_parent = fx._root.path().join("dest-parent");
    fs::create_dir_all(&dest_parent).unwrap();
    let dest_home = fx._root.path().join("dest-home");
    fs::create_dir_all(&dest_home).unwrap();

    let (res, _m) = unpack_bundle(&bundle, &dest_parent, &dest_home).unwrap();
    let dest = res.dest_path.to_string_lossy().to_string();

    // Gemini: session file under dest home with rewritten path + projects.json merge
    let gemini_files: Vec<_> = walk_files(&dest_home.join(".gemini"))
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    assert!(
        !gemini_files.is_empty(),
        "gemini jsonl landed under dest home"
    );
    for f in &gemini_files {
        let body = fs::read_to_string(f).unwrap();
        assert!(
            !body.contains(&project) || project == dest,
            "gemini jsonl must rewrite SOURCE path, body={body}"
        );
        assert!(
            body.contains(&dest) || project == dest,
            "gemini jsonl must contain DEST path"
        );
    }
    let projects_json = fs::read_to_string(dest_home.join(".gemini/projects.json")).unwrap();
    assert!(
        projects_json.contains(&dest),
        "gemini projects.json must map dest path, got {projects_json}"
    );

    // Pi
    let pi_files = walk_files(&dest_home.join(".pi"));
    assert!(
        pi_files
            .iter()
            .any(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")),
        "pi session on dest"
    );
    for f in pi_files
        .iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
    {
        let body = fs::read_to_string(f).unwrap();
        assert!(!body.contains(&project) || project == dest, "pi cwd rewritten");
        assert!(body.contains(&dest) || project == dest);
    }

    // Codex
    let codex_files = walk_files(&dest_home.join(".codex"));
    assert!(
        codex_files
            .iter()
            .any(|p| p.to_string_lossy().contains("rollout-")),
        "codex rollout on dest"
    );
    assert!(
        !codex_files
            .iter()
            .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("history.jsonl")),
        "codex history.jsonl must not appear on dest from clone"
    );

    // Grok: re-keyed percent-encoded dest cwd
    let dest_encoded = super::providers::grok_percent_encode_cwd(&dest);
    let grok_summary = dest_home
        .join(".grok/sessions")
        .join(&dest_encoded)
        .join("01920000-cccc-7000-8000-000000000001")
        .join("summary.json");
    assert!(
        grok_summary.exists(),
        "grok summary under dest-encoded cwd, missing at {}",
        grok_summary.display()
    );
    let summary = fs::read_to_string(&grok_summary).unwrap();
    assert!(
        !summary.contains(&project) || project == dest,
        "grok summary cwd rewritten"
    );
    assert!(summary.contains(&dest) || project == dest);
    assert!(
        !dest_home.join(".grok/auth.json").exists(),
        "grok auth must not land on dest"
    );

    // Cursor: re-keyed md5(DEST)
    let dest_md5 = crate::chat_history::md5_hex(dest.as_bytes());
    let src_md5 = crate::chat_history::md5_hex(project.as_bytes());
    let cursor_db = dest_home
        .join(".cursor/chats")
        .join(&dest_md5)
        .join("cccccccc-cccc-cccc-cccc-cccccccccccc")
        .join("store.db");
    assert!(
        cursor_db.exists(),
        "cursor store.db under md5(DEST), missing at {}",
        cursor_db.display()
    );
    if src_md5 != dest_md5 {
        assert!(
            !dest_home
                .join(".cursor/chats")
                .join(&src_md5)
                .exists(),
            "cursor must not keep src md5 dir on dest"
        );
    }
    let cursor_body = fs::read_to_string(&cursor_db).unwrap();
    assert!(
        !cursor_body.contains(&project) || project == dest,
        "cursor store.db best-effort rewrite"
    );

    // Hermes: rows merged into dest state.db with rewritten cwd
    let dest_db = dest_home.join(".hermes/state.db");
    assert!(dest_db.exists(), "hermes state.db created on dest");
    let conn = rusqlite::Connection::open(&dest_db).unwrap();
    let cwd: String = conn
        .query_row(
            "SELECT cwd FROM sessions WHERE id = '20260701_090000_aaaaaa'",
            [],
            |row| row.get(0),
        )
        .expect("hermes session row present");
    assert_eq!(cwd, dest, "hermes cwd rewritten to dest");
    let foreign: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = '20260629_070000_eeeeee'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(foreign, 0, "foreign hermes session must not import");
    let msg: String = conn
        .query_row(
            "SELECT content FROM messages WHERE session_id = '20260701_090000_aaaaaa' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        msg.contains(&dest) || !msg.contains(&project),
        "hermes message content rewritten, got {msg}"
    );
}

/// Collect all files under `root` as absolute paths (best-effort).
fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .standard_filters(false)
        .follow_links(false)
        .build();
    for entry in walker.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            out.push(entry.path().to_path_buf());
        }
    }
    out
}
