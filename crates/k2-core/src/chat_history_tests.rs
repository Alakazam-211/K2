use super::*;

#[test]
fn resolve_root_project_path_strips_worktree() {
    assert_eq!(resolve_root_project_path("/r/.worktrees/f-x"), "/r");
    assert_eq!(resolve_root_project_path("/r"), "/r");
    assert_eq!(resolve_root_project_path("/r/.k2so/x"), "/r/.k2so/x");
}

#[test]
fn matches_project_family_covers_root_and_worktrees() {
    assert!(matches_project_family("/r", "/r"));
    assert!(matches_project_family("/r/.worktrees/a", "/r"));
    assert!(!matches_project_family("/other", "/r"));
    // Must start with the separator — `/r2` shouldn't match `/r`.
    assert!(!matches_project_family("/r2", "/r"));
}

/// Windows: a harness's cwd matches K2's whichever spelling each used.
#[cfg(windows)]
#[test]
fn windows_cwds_match_across_spellings() {
    let k2 = "D:/work/sales";
    for harness in [r"D:\work\sales", r"d:\work\sales\", r"\\?\D:\work\sales", "D:/Work/Sales/"] {
        assert!(matches_project_family(harness, k2), "{harness}");
        assert!(same_cwd(harness, k2), "{harness}");
    }
    assert!(matches_project_family(r"D:\work\sales\.worktrees\a", k2));
    assert!(!matches_project_family(r"D:\work\sales2", k2));
    assert!(!same_cwd(r"D:\work\sales\sub", k2));
    assert_eq!(hermes_family_params(r"D:\Work\Sales").0, "d:/work/sales");
}

#[test]
fn claude_project_hash_handles_hidden_dirs() {
    // Non-existent path → `canonicalize` fails → literal fallback →
    // char-map. A hidden `/.k2so` segment maps `/`→`-` AND `.`→`-`,
    // producing the `--k2so` double-hyphen run (which Claude also
    // emits). This golden survives the 0.39.44 encoder rewrite (both
    // the old `/.`→`/-` rule and the new per-char rule produce `--`).
    assert_eq!(
        claude_project_hash("/Users/z/proj/.k2so/agents/a"),
        "-Users-z-proj--k2so-agents-a"
    );
    assert_eq!(claude_project_hash("/r"), "-r");
}

#[test]
fn claude_project_hash_handles_spaces() {
    // Claude turns spaces in path components to hyphens. Mirror that so
    // `claude_session_file_exists` resolves the right on-disk dir for
    // workspaces like `/Users/.../Alakazam Labs/...`. (Non-existent
    // fixtures → literal fallback → ` `→`-` char-map. This golden is
    // unchanged by the 0.39.44 encoder rewrite.)
    assert_eq!(
        claude_project_hash("/Users/z/DevProjects/Alakazam Labs/K2SO"),
        "-Users-z-DevProjects-Alakazam-Labs-K2SO"
    );
    assert_eq!(
        claude_project_hash("/Users/z/Some Folder With Spaces/proj"),
        "-Users-z-Some-Folder-With-Spaces-proj"
    );
}

#[test]
fn claude_project_hash_collapses_underscores_and_dots() {
    // GH#25 root cause: the pre-0.39.44 encoder PRESERVED `_` and any
    // mid-component `.`, so K2SO and Claude diverged on these paths and
    // never saw each other's sessions. Claude maps EVERY non-`[a-zA-Z0-9]`
    // char → `-`. These fixtures are non-existent → literal fallback →
    // the char-map alone must reproduce Claude's collapse.
    // Underscore (the #25 case):
    assert_eq!(
        claude_project_hash("/Users/z/proj/scout_v3"),
        "-Users-z-proj-scout-v3"
    );
    // Mid-component dot:
    assert_eq!(claude_project_hash("/Users/z/foo.bar"), "-Users-z-foo-bar");
    // Both together — mirrors the live probe SHAPE (a `_` dir with a
    // dotted child), just without the symlinked `/tmp` prefix:
    assert_eq!(
        claude_project_hash("/k2so_slug_probe_a_b/has_under_score.dotted"),
        "-k2so-slug-probe-a-b-has-under-score-dotted"
    );
}

#[test]
fn claude_project_hash_canonicalizes_symlinks() {
    // Claude canonicalizes the path (resolving symlinks) BEFORE slugging,
    // so a symlinked dir and its real target must produce the SAME slug.
    // Live probe (a real, EXISTING path so canonicalize runs):
    //   /private/tmp/k2so_slug_probe_a_b/has_under_score.dotted
    //   → -private-tmp-k2so-slug-probe-a-b-has-under-score-dotted
    // We reproduce that here with a real tmp dir + a symlink to it and
    // assert both collapse identically.
    let unique = format!(
        "k2so_slug_probe_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    // Real target dir (contains `_` and a dotted child so we also cover
    // the char-map through canonicalize).
    let real = std::env::temp_dir().join(&unique).join("has_under_score.dotted");
    std::fs::create_dir_all(&real).expect("create real probe dir");
    // Symlink whose name differs from its target.
    let link = std::env::temp_dir().join(format!("{unique}_link"));
    let _ = std::fs::remove_file(&link);
    crate::test_env::symlink(std::env::temp_dir().join(&unique), &link).expect("create symlink");

    let via_real = claude_project_hash(&real.to_string_lossy());
    // Through the symlink to the SAME dotted child.
    let via_link = claude_project_hash(
        &link.join("has_under_score.dotted").to_string_lossy(),
    );

    // canonicalize collapses both to the identical real-path slug.
    assert_eq!(
        via_real, via_link,
        "symlinked + real paths must slug identically"
    );
    // And the slug is the char-mapped CANONICAL path: it must contain the
    // real (resolved) basename chain with `_` and `.` collapsed, and must
    // NOT contain the symlink's own `_link` segment.
    assert!(
        via_real.ends_with("-has-under-score-dotted"),
        "canonical basename collapsed: {via_real}"
    );
    assert!(
        !via_link.contains("link"),
        "symlink name resolved away, not in slug: {via_link}"
    );

    // Cleanup (best-effort).
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(&unique));
}

#[test]
fn cursor_project_hash_strips_leading_slash() {
    assert_eq!(
        cursor_project_hash("/Users/z/DevProjects/K2SO"),
        "Users-z-DevProjects-K2SO"
    );
}

#[test]
fn detect_active_session_unknown_provider_returns_none() {
    assert_eq!(detect_active_session("bogus", "/r").unwrap(), None);
}

// ── Phase 2 Unit 6 additions ───────────────────────────────────
//
// Tests for the ~1,700 LoC migrated from the Tauri command into
// k2so-core under "Phase 2 Unit 6 — full IDE-history parsing
// surface" earlier in this file.

/// Each HOME-mutating test grabs this lock so cargo's parallel
/// runner can't see another test's HOME between the set + the
/// inner call. `themes::HOME_LOCK` is a handle to the ONE env lock
/// (`crate::test_env::lock()`), shared by every env-touching test.
use crate::themes::HOME_LOCK as UNIT6_HOME_LOCK;

struct U6TempDir {
    path: std::path::PathBuf,
}

impl U6TempDir {
    fn new(label: &str) -> Self {
        let path = crate::test_env::unique_temp_path(&format!("ch-u6-{label}"));
        std::fs::create_dir_all(&path).expect("create tempdir");
        Self { path }
    }
}

impl Drop for U6TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Temp `$HOME` under the ONE env lock (`crate::test_env::TempHome`):
/// HOME is restored and the dir removed on drop, also on panic.
struct U6HomeGuard {
    _home: crate::test_env::TempHome,
}

impl U6HomeGuard {
    fn new(_label: &str) -> Self {
        Self { _home: crate::test_env::TempHome::new() }
    }
}

// ── GH#24: newest-on-disk session resolution ───────────────────
//
// `newest_session_in_projects_dir` is the convergence helper the
// pinned-chat resolver uses to RESUME a real session instead of
// minting a throwaway `--session-id` (which a reused PTY never runs
// → endless re-mint loop on remote clients). These exercise the
// inner, HOME-free core directly.

/// Set a file's mtime so tests can order "newest" deterministically
/// (filesystem write order alone isn't reliable at coarse mtime
/// granularity). Requires the file opened with write perms.
fn set_mtime(path: &std::path::Path, t: std::time::SystemTime) {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_mtime");
    f.set_modified(t).expect("set_modified");
}

#[test]
fn newest_session_picks_latest_nonempty_and_skips_empty() {
    let tmp = U6TempDir::new("newest-session");
    let projects = tmp.path.join(".claude").join("projects");
    let hash = "-Users-z-proj-acme";
    let dir = projects.join(hash);
    std::fs::create_dir_all(&dir).unwrap();

    let old = dir.join("old-aaaa.jsonl");
    let new = dir.join("new-bbbb.jsonl");
    let empty = dir.join("empty-cccc.jsonl");
    std::fs::write(&old, b"{\"cwd\":\"/x\"}\n").unwrap();
    std::fs::write(&new, b"{\"cwd\":\"/x\"}\n").unwrap();
    std::fs::write(&empty, b"").unwrap();

    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    set_mtime(&old, base);
    set_mtime(&new, base + std::time::Duration::from_secs(10));
    // `empty` is the NEWEST by mtime but 0-byte → must be ignored
    // (a never-run `--session-id` pre-allocation; `--resume` would
    // say "No conversation found").
    set_mtime(&empty, base + std::time::Duration::from_secs(20));

    assert_eq!(
        newest_session_in_projects_dir(&projects, hash).map(|(_, id)| id).as_deref(),
        Some("new-bbbb")
    );
}

#[test]
fn newest_session_includes_worktree_siblings_and_ignores_other_projects() {
    let tmp = U6TempDir::new("newest-session-wt");
    let projects = tmp.path.join(".claude").join("projects");
    let hash = "-Users-z-proj-acme";
    let wt_dir = projects.join(format!("{hash}-feature-x"));
    let other_dir = projects.join("-Users-z-proj-OTHER");
    std::fs::create_dir_all(&wt_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();

    // A worktree-sibling session (older) and an UNRELATED project's
    // session (newest overall — must NOT be picked).
    let wt = wt_dir.join("wt-1111.jsonl");
    let other = other_dir.join("other-9999.jsonl");
    std::fs::write(&wt, b"{\"cwd\":\"/x\"}\n").unwrap();
    std::fs::write(&other, b"{\"cwd\":\"/y\"}\n").unwrap();
    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000);
    set_mtime(&wt, base);
    set_mtime(&other, base + std::time::Duration::from_secs(99));

    assert_eq!(
        newest_session_in_projects_dir(&projects, hash).map(|(_, id)| id).as_deref(),
        Some("wt-1111")
    );
}

#[test]
fn newest_session_none_for_brand_new_workspace() {
    let tmp = U6TempDir::new("newest-session-empty");
    let projects = tmp.path.join(".claude").join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    // No slug dir for this hash at all → genuinely brand-new → None
    // (the resolver then mints a fresh `--session-id`, preserving the
    // brand-new-workspace behavior from #681).
    assert_eq!(
        newest_session_in_projects_dir(&projects, "-Users-z-proj-acme"),
        None
    );
}

// ── Grok on-disk discovery (slice 3 ProviderResume backing) ─────
//
// Fixture layout mirrors `.k2/notes/grok-session-storage-study.md`:
// `<sessions_root>/<encoded-cwd>/<uuid>/summary.json`. The inner
// `_in_root` cores are exercised directly (HOME-free); the adapter
// wrappers get a $HOME-honoring test in `provider_resume.rs`.

fn write_grok_session(
    sessions_root: &std::path::Path,
    cwd_dir: &str,
    session_id: &str,
    cwd: &str,
    last_active_at: &str,
    session_kind: Option<&str>,
) {
    let dir = sessions_root.join(cwd_dir).join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut summary = serde_json::json!({
        "info": { "id": session_id, "cwd": cwd },
        "generated_title": "test session",
        "created_at": last_active_at,
        "updated_at": last_active_at,
        "last_active_at": last_active_at,
        "num_messages": 4,
    });
    if let Some(kind) = session_kind {
        summary["session_kind"] = serde_json::json!(kind);
    }
    std::fs::write(dir.join("summary.json"), summary.to_string()).unwrap();
    std::fs::write(dir.join("chat_history.jsonl"), b"{\"type\":\"user\"}\n").unwrap();
}

#[test]
fn grok_exists_matches_family_and_skips_subagents() {
    let tmp = U6TempDir::new("grok-exists");
    let root = tmp.path.join(".grok").join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    // Decoy non-dir entries the walk must skip (study: sqlite index
    // + .lock files live as siblings of the cwd dirs).
    std::fs::write(root.join("session_search.sqlite"), b"sqlite").unwrap();
    std::fs::write(root.join("active_sessions.lock"), b"").unwrap();

    let project = "/Users/z/proj-grok";
    let user_sid = "01920000-aaaa-7000-8000-000000000001";
    let sub_sid = "01920000-aaaa-7000-8000-000000000002";
    let other_sid = "01920000-aaaa-7000-8000-000000000003";
    write_grok_session(&root, "%2FUsers%2Fz%2Fproj-grok", user_sid, project,
        "2026-07-03T10:00:00Z", None);
    // Subagent session with the RIGHT cwd — must never match.
    write_grok_session(&root, "%2FUsers%2Fz%2Fproj-grok", sub_sid, project,
        "2026-07-03T11:00:00Z", Some("subagent"));
    // Right id shape, WRONG project — must never match.
    write_grok_session(&root, "%2FUsers%2Fz%2Fother", other_sid, "/Users/z/other",
        "2026-07-03T12:00:00Z", None);

    assert!(grok_session_exists_in_root(&root, user_sid, project));
    assert!(
        !grok_session_exists_in_root(&root, sub_sid, project),
        "subagent sessions must be invisible to the exists check"
    );
    assert!(!grok_session_exists_in_root(&root, other_sid, project));
    assert!(!grok_session_exists_in_root(&root, "no-such-id", project));
    // Worktree family: a worktree path resolves to the root project.
    assert!(grok_session_exists_in_root(
        &root,
        user_sid,
        "/Users/z/proj-grok/.worktrees/feature-x"
    ));
}

#[test]
fn grok_newest_picks_last_active_and_skips_subagents() {
    let tmp = U6TempDir::new("grok-newest");
    let root = tmp.path.join(".grok").join("sessions");
    std::fs::create_dir_all(&root).unwrap();

    let project = "/Users/z/proj-grok-newest";
    let older = "01920000-bbbb-7000-8000-000000000001";
    let newest = "01920000-bbbb-7000-8000-000000000002";
    let sub = "01920000-bbbb-7000-8000-000000000003";
    let foreign = "01920000-bbbb-7000-8000-000000000004";
    write_grok_session(&root, "%2Fproj", older, project, "2026-07-03T09:00:00Z", None);
    // Worktree sibling counts as family AND is the newest user session.
    write_grok_session(
        &root,
        "%2Fproj-wt",
        newest,
        "/Users/z/proj-grok-newest/.worktrees/f",
        "2026-07-03T10:30:00Z",
        None,
    );
    // Subagent is newest overall — must be skipped.
    write_grok_session(&root, "%2Fproj", sub, project, "2026-07-03T11:00:00Z",
        Some("subagent"));
    // Unrelated project, newest of all — must be ignored.
    write_grok_session(&root, "%2Fother", foreign, "/Users/z/other",
        "2026-07-03T12:00:00Z", None);

    assert_eq!(
        newest_grok_session_in_root(&root, project).map(|(_, id)| id).as_deref(),
        Some(newest),
        "newest must be picked by last_active_at across the project family, \
             skipping subagents + foreign projects"
    );
}

#[test]
fn grok_newest_none_for_unknown_project_and_missing_root() {
    let tmp = U6TempDir::new("grok-none");
    let root = tmp.path.join(".grok").join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(newest_grok_session_in_root(&root, "/nope"), None);
    // Missing sessions root entirely.
    assert_eq!(
        newest_grok_session_in_root(&tmp.path.join("absent"), "/nope"),
        None
    );
}

#[test]
fn grok_summary_timestamp_falls_back_to_mtime() {
    let tmp = U6TempDir::new("grok-mtime");
    let root = tmp.path.join(".grok").join("sessions");
    let project = "/Users/z/proj-grok-mtime";
    let a = "01920000-cccc-7000-8000-000000000001";
    let b = "01920000-cccc-7000-8000-000000000002";
    // Neither summary carries last_active_at/updated_at → mtime decides.
    for (sid, secs) in [(a, 1_000_000u64), (b, 2_000_000u64)] {
        let dir = root.join("%2Fproj").join(sid);
        std::fs::create_dir_all(&dir).unwrap();
        let summary = serde_json::json!({ "info": { "id": sid, "cwd": project } });
        let path = dir.join("summary.json");
        std::fs::write(&path, summary.to_string()).unwrap();
        set_mtime(
            &path,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        );
    }
    assert_eq!(newest_grok_session_in_root(&root, project).map(|(_, id)| id).as_deref(), Some(b));
}

// ── Slice 6: grok/hermes full discovery (chat/list + detect) ────
//
// Grok fixtures extend the slice-3 tree with the title/count
// sources (`signals.json`, `chat_history.jsonl`, the per-cwd
// `prompt_history.jsonl`); hermes fixtures build a real WAL-mode
// `state.db` via rusqlite per the storage study's schema.

/// Like `write_grok_session` but with full control over the title
/// fields (both absent = exercise the prompt-history/"Untitled"
/// fallbacks). Returns the session dir.
fn write_grok_summary_custom(
    sessions_root: &std::path::Path,
    cwd_dir: &str,
    session_id: &str,
    cwd: &str,
    last_active_at: &str,
    generated_title: Option<&str>,
    session_summary: Option<&str>,
) -> std::path::PathBuf {
    let dir = sessions_root.join(cwd_dir).join(session_id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut summary = serde_json::json!({
        "info": { "id": session_id, "cwd": cwd },
        "created_at": last_active_at,
        "updated_at": last_active_at,
        "last_active_at": last_active_at,
    });
    if let Some(t) = generated_title {
        summary["generated_title"] = serde_json::json!(t);
    }
    if let Some(s) = session_summary {
        summary["session_summary"] = serde_json::json!(s);
    }
    std::fs::write(dir.join("summary.json"), summary.to_string()).unwrap();
    dir
}

#[test]
fn grok_parse_lists_user_sessions_skips_subagents_and_nonuuid() {
    let tmp = U6TempDir::new("grok-parse");
    let root = tmp.path.join(".grok").join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("session_search.sqlite"), b"sqlite").unwrap();
    std::fs::write(root.join("active_sessions.lock"), b"").unwrap();

    let project = "/Users/z/proj-parse";
    let user_sid = "01920000-eeee-7000-8000-000000000001";
    let sub_sid = "01920000-eeee-7000-8000-000000000002";
    let foreign_sid = "01920000-eeee-7000-8000-000000000003";
    let dir = write_grok_summary_custom(
        &root, "%2Fproj", user_sid, project, "2026-07-03T10:00:00Z",
        Some("Fix the walk"), None,
    );
    // signals.json is the message-count SSOT: 2 user + 3 assistant.
    std::fs::write(
        dir.join("signals.json"),
        r#"{"userMessageCount":2,"assistantMessageCount":3,"turnCount":2}"#,
    )
    .unwrap();
    write_grok_session(&root, "%2Fproj", sub_sid, project,
        "2026-07-03T11:00:00Z", Some("subagent"));
    write_grok_session(&root, "%2Fother", foreign_sid, "/Users/z/other",
        "2026-07-03T12:00:00Z", None);
    // A non-UUID dir with a perfectly valid summary must be skipped.
    write_grok_summary_custom(
        &root, "%2Fproj", "not-a-uuid-dir", project,
        "2026-07-03T13:00:00Z", Some("decoy"), None,
    );

    let sessions = parse_grok_sessions_in_root(&root, Some(project));
    assert_eq!(sessions.len(), 1, "got: {sessions:?}");
    let s = &sessions[0];
    assert_eq!(s.session_id, user_sid);
    assert_eq!(s.provider, "grok");
    assert_eq!(s.project, project);
    assert_eq!(s.title, "Fix the walk");
    assert_eq!(s.message_count, 5, "signals user+assistant counts");
    assert_eq!(
        s.timestamp,
        parse_rfc3339_to_ms("2026-07-03T10:00:00Z").unwrap()
    );
    assert_eq!(s.origin_branch, None);

    // Unfiltered: the foreign user session appears, subagent +
    // non-UUID decoy still never do.
    let all = parse_grok_sessions_in_root(&root, None);
    let ids: Vec<&str> = all.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(all.len(), 2, "got: {all:?}");
    assert!(ids.contains(&user_sid) && ids.contains(&foreign_sid));
}

#[test]
fn grok_parse_title_fallback_chain() {
    let tmp = U6TempDir::new("grok-titles");
    let root = tmp.path.join(".grok").join("sessions");
    let project = "/Users/z/proj-titles";
    let a = "01920000-ffff-7000-8000-00000000000a"; // session_summary
    let b = "01920000-ffff-7000-8000-00000000000b"; // prompt_history
    let c = "01920000-ffff-7000-8000-00000000000c"; // Untitled
    write_grok_summary_custom(&root, "%2Fproj", a, project,
        "2026-07-03T10:00:00Z", None, Some("Summary Title"));
    write_grok_summary_custom(&root, "%2Fproj", b, project,
        "2026-07-03T10:01:00Z", None, None);
    write_grok_summary_custom(&root, "%2Fproj", c, project,
        "2026-07-03T10:02:00Z", None, None);
    // Per-cwd prompt_history.jsonl — FIRST prompt for an id wins.
    let prompt_history = format!(
        "{}\n{}\n",
        serde_json::json!({
            "timestamp": "2026-07-03T10:01:00Z", "session_id": b,
            "prompt": "first prompt for b", "is_bash": false }),
        serde_json::json!({
            "timestamp": "2026-07-03T10:01:30Z", "session_id": b,
            "prompt": "second prompt for b", "is_bash": false }),
    );
    std::fs::write(
        root.join("%2Fproj").join("prompt_history.jsonl"),
        prompt_history,
    )
    .unwrap();

    let sessions = parse_grok_sessions_in_root(&root, Some(project));
    let title_of = |sid: &str| {
        sessions
            .iter()
            .find(|s| s.session_id == sid)
            .unwrap_or_else(|| panic!("{sid} missing: {sessions:?}"))
            .title
            .clone()
    };
    assert_eq!(title_of(a), "Summary Title", "session_summary fallback");
    assert_eq!(title_of(b), "first prompt for b", "prompt_history fallback");
    assert_eq!(title_of(c), "Untitled", "final fallback");
}

#[test]
fn grok_parse_message_count_falls_back_to_chat_history() {
    let tmp = U6TempDir::new("grok-count");
    let root = tmp.path.join(".grok").join("sessions");
    let project = "/Users/z/proj-count";
    let sid = "01920000-abab-7000-8000-000000000001";
    let dir = write_grok_summary_custom(&root, "%2Fproj", sid, project,
        "2026-07-03T10:00:00Z", Some("t"), None);
    // No signals.json → count chat_history.jsonl user/assistant
    // lines WITHOUT synthetic_reason: expect 2 (user + assistant).
    let chat_history = concat!(
        "{\"type\":\"system\",\"content\":\"boot\"}\n",
        "{\"type\":\"user\",\"content\":\"<user_query>hi</user_query>\"}\n",
        "{\"type\":\"user\",\"synthetic_reason\":\"injected_context\",\"content\":\"ctx\"}\n",
        "{\"type\":\"assistant\",\"content\":\"hello\"}\n",
        "{\"type\":\"reasoning\",\"content\":\"thinking\"}\n",
    );
    std::fs::write(dir.join("chat_history.jsonl"), chat_history).unwrap();

    let sessions = parse_grok_sessions_in_root(&root, Some(project));
    assert_eq!(sessions.len(), 1, "got: {sessions:?}");
    assert_eq!(
        sessions[0].message_count, 2,
        "user+assistant minus synthetic lines"
    );
}

// ── Hermes SQLite fixtures (study schema subset) ────────────────

fn seed_hermes_db(db_path: &std::path::Path) -> rusqlite::Connection {
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(db_path).expect("open fixture db");
    // Real hermes runs WAL — fixtures do too, so the read-only
    // URI open path is exercised against a WAL db.
    conn.pragma_update(None, "journal_mode", "wal").expect("WAL");
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
    .expect("create schema");
    conn
}

#[allow(clippy::too_many_arguments)]
fn insert_hermes_session(
    conn: &rusqlite::Connection,
    id: &str,
    source: &str,
    cwd: Option<&str>,
    title: Option<&str>,
    started_at: f64,
    ended_at: Option<f64>,
    archived: i64,
) {
    conn.execute(
        "INSERT INTO sessions (id, source, cwd, title, started_at, ended_at, archived) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![id, source, cwd, title, started_at, ended_at, archived],
    )
    .expect("insert session");
}

fn insert_hermes_message(
    conn: &rusqlite::Connection,
    session_id: &str,
    role: &str,
    content: &str,
    timestamp: f64,
) {
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) \
             VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![session_id, role, content, timestamp],
    )
    .expect("insert message");
}

const HERMES_PROJ: &str = "/Users/z/proj-hermes";
const H_TITLED: &str = "20260701_090000_aaaaaa";
const H_UNTITLED_MSG: &str = "20260702_100000_bbbbbb";
const H_SUBAGENT: &str = "20260703_110000_cccccc";
const H_ARCHIVED: &str = "20260630_080000_dddddd";
const H_FOREIGN: &str = "20260629_070000_eeeeee";
const H_EMPTY: &str = "20260628_060000_ffffff";

/// Standard hermes fixture: titled cli session, title-less cli
/// session in a worktree (first-user-message fallback), a
/// source='subagent' sibling (newest of all — must stay invisible),
/// an archived cli row, a foreign-project row, and an empty
/// message-less row.
fn seed_standard_hermes_fixture(db_path: &std::path::Path) -> rusqlite::Connection {
    let conn = seed_hermes_db(db_path);
    insert_hermes_session(&conn, H_TITLED, "cli", Some(HERMES_PROJ),
        Some("Fix the heartbeats"), 1000.0, None, 0);
    insert_hermes_message(&conn, H_TITLED, "user", "hello", 1001.0);
    insert_hermes_message(&conn, H_TITLED, "assistant", "hi!", 1002.0);
    insert_hermes_message(&conn, H_TITLED, "tool", "ls output", 1003.5);

    let worktree_cwd = format!("{HERMES_PROJ}/.worktrees/f");
    insert_hermes_session(&conn, H_UNTITLED_MSG, "cli", Some(&worktree_cwd),
        None, 2000.0, None, 0);
    insert_hermes_message(&conn, H_UNTITLED_MSG, "user", "  make it fast  ", 2001.0);

    insert_hermes_session(&conn, H_SUBAGENT, "subagent", Some(HERMES_PROJ),
        Some("delegated research"), 3000.0, None, 0);
    insert_hermes_session(&conn, H_ARCHIVED, "cli", Some(HERMES_PROJ),
        Some("old archived work"), 2500.0, Some(2600.0), 1);
    insert_hermes_session(&conn, H_FOREIGN, "cli", Some("/Users/z/other"),
        Some("other project"), 1500.0, None, 0);
    insert_hermes_session(&conn, H_EMPTY, "cli", Some(HERMES_PROJ),
        None, 500.0, Some(600.0), 0);
    conn
}

#[test]
fn hermes_parse_filters_predicate_and_maps_fields() {
    let tmp = U6TempDir::new("hermes-parse");
    let db_path = tmp.path.join(".hermes").join("state.db");
    let _writer = seed_standard_hermes_fixture(&db_path);

    let sessions = parse_hermes_sessions_in_db(&db_path, Some(HERMES_PROJ));
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(sessions.len(), 3, "cli+unarchived+family only; got: {sessions:?}");
    assert!(ids.contains(&H_TITLED) && ids.contains(&H_UNTITLED_MSG) && ids.contains(&H_EMPTY));

    let by_id = |sid: &str| sessions.iter().find(|s| s.session_id == sid).unwrap();
    let titled = by_id(H_TITLED);
    assert_eq!(titled.title, "Fix the heartbeats");
    assert_eq!(titled.provider, "hermes");
    assert_eq!(titled.project, HERMES_PROJ);
    assert_eq!(titled.message_count, 2, "user+assistant only, tool rows excluded");
    assert_eq!(titled.timestamp, 1_003_500, "MAX(messages.timestamp) × 1000 → ms");
    assert_eq!(titled.origin_branch, None);

    let untitled = by_id(H_UNTITLED_MSG);
    assert_eq!(untitled.title, "make it fast", "first user message, trimmed");
    assert_eq!(untitled.origin_branch.as_deref(), Some("f"));
    assert_eq!(untitled.timestamp, 2_001_000);
    assert_eq!(untitled.message_count, 1);

    let empty = by_id(H_EMPTY);
    assert_eq!(empty.title, "Untitled");
    assert_eq!(empty.timestamp, 600_000, "no messages → ended_at fallback");
    assert_eq!(empty.message_count, 0);

    // Unfiltered: the foreign cli session joins; subagent +
    // archived rows stay invisible either way.
    let all = parse_hermes_sessions_in_db(&db_path, None);
    assert_eq!(all.len(), 4, "got: {all:?}");
    assert!(all.iter().any(|s| s.session_id == H_FOREIGN));
    assert!(!all.iter().any(|s| s.session_id == H_SUBAGENT));
    assert!(!all.iter().any(|s| s.session_id == H_ARCHIVED));
}

#[test]
fn hermes_detect_newest_cli_by_started_at() {
    let tmp = U6TempDir::new("hermes-detect");
    let db_path = tmp.path.join(".hermes").join("state.db");
    let _writer = seed_standard_hermes_fixture(&db_path);

    // Newest by started_at among source='cli' AND archived=0 in the
    // family — the subagent (3000.0) and archived (2500.0) rows are
    // newer and must lose to the worktree cli session (2000.0).
    assert_eq!(
        detect_hermes_session_in_db(&db_path, HERMES_PROJ).as_deref(),
        Some(H_UNTITLED_MSG)
    );
    // A worktree path resolves to the same family.
    assert_eq!(
        detect_hermes_session_in_db(
            &db_path,
            &format!("{HERMES_PROJ}/.worktrees/other")
        )
        .as_deref(),
        Some(H_UNTITLED_MSG)
    );
    assert_eq!(detect_hermes_session_in_db(&db_path, "/Users/z/nope"), None);
    // Absent DB → None, never an error.
    assert_eq!(
        detect_hermes_session_in_db(&tmp.path.join("absent.db"), HERMES_PROJ),
        None
    );
}

#[test]
fn hermes_exists_counts_any_row_even_superseded() {
    let tmp = U6TempDir::new("hermes-exists");
    let db_path = tmp.path.join(".hermes").join("state.db");
    let _writer = seed_standard_hermes_fixture(&db_path);

    assert!(hermes_session_exists_in_db(&db_path, H_TITLED));
    // Compression-chain rule: a stored id can be superseded, and
    // hermes redirects resume to the chain tip — archived and even
    // subagent ids count as existing.
    assert!(hermes_session_exists_in_db(&db_path, H_ARCHIVED));
    assert!(hermes_session_exists_in_db(&db_path, H_SUBAGENT));
    assert!(!hermes_session_exists_in_db(&db_path, "20990101_000000_zzzzzz"));
    assert!(!hermes_session_exists_in_db(&db_path, ""));
    assert!(!hermes_session_exists_in_db(&tmp.path.join("absent.db"), H_TITLED));
}

#[test]
fn hermes_like_pattern_escapes_wildcards() {
    // `_` in a project path is a LIKE single-char wildcard — without
    // ESCAPE, /Users/z/my_proj would claim /Users/z/myxproj's
    // sessions.
    let tmp = U6TempDir::new("hermes-escape");
    let db_path = tmp.path.join(".hermes").join("state.db");
    let conn = seed_hermes_db(&db_path);
    insert_hermes_session(&conn, "20260701_000000_111111", "cli",
        Some("/Users/z/myxproj/.worktrees/w"), Some("decoy"), 100.0, None, 0);
    insert_hermes_session(&conn, "20260702_000000_222222", "cli",
        Some("/Users/z/my_proj/.worktrees/w"), Some("real"), 50.0, None, 0);
    assert_eq!(
        detect_hermes_session_in_db(&db_path, "/Users/z/my_proj").as_deref(),
        Some("20260702_000000_222222"),
        "underscore must not wildcard-match the decoy"
    );
}

#[test]
fn hermes_wal_reads_coexist_with_open_writer() {
    // The live gotcha: a running hermes holds state.db open in WAL
    // mode. Keep the writer connection open, add a row AFTER the
    // fixture, and confirm a fresh read-only URI connection sees the
    // uncheckpointed WAL data.
    let tmp = U6TempDir::new("hermes-wal");
    let db_path = tmp.path.join(".hermes").join("state.db");
    let writer = seed_standard_hermes_fixture(&db_path);

    insert_hermes_session(&writer, "20260704_120000_abcdef", "cli",
        Some(HERMES_PROJ), Some("live row in WAL"), 5000.0, None, 0);

    // Writer still open — read-only connection must see the row.
    assert_eq!(
        detect_hermes_session_in_db(&db_path, HERMES_PROJ).as_deref(),
        Some("20260704_120000_abcdef"),
        "ro+URI connection must read uncheckpointed WAL rows"
    );
    let sessions = parse_hermes_sessions_in_db(&db_path, Some(HERMES_PROJ));
    assert!(
        sessions.iter().any(|s| s.title == "live row in WAL"),
        "got: {sessions:?}"
    );
}

#[test]
fn detect_active_session_dispatches_grok_and_hermes() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("dispatch-grok-hermes");
    let home = crate::paths::home_dir().unwrap();

    let project = "/Users/z/proj-dispatch";
    let grok_sid = "01920000-cdcd-7000-8000-000000000001";
    write_grok_session(
        &home.join(".grok").join("sessions"),
        "%2Fproj", grok_sid, project, "2026-07-03T10:00:00Z", None,
    );
    let conn = seed_hermes_db(&home.join(".hermes").join("state.db"));
    insert_hermes_session(&conn, "20260703_140000_dedede", "cli",
        Some(project), Some("dispatch me"), 1000.0, None, 0);

    assert_eq!(
        detect_active_session("grok", project).unwrap().as_deref(),
        Some(grok_sid)
    );
    assert_eq!(
        detect_active_session("hermes", project).unwrap().as_deref(),
        Some("20260703_140000_dedede")
    );
}

#[test]
fn list_all_sessions_includes_grok_and_hermes() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("list-grok-hermes");
    let home = crate::paths::home_dir().unwrap();

    let project = "/Users/z/proj-list";
    write_grok_session(
        &home.join(".grok").join("sessions"),
        "%2Fproj", "01920000-efef-7000-8000-000000000001",
        project, "2026-07-03T10:00:00Z", None,
    );
    let conn = seed_hermes_db(&home.join(".hermes").join("state.db"));
    insert_hermes_session(&conn, "20260703_150000_fafafa", "cli",
        Some(project), Some("aggregated"), 1000.0, None, 0);

    let sessions = list_all_sessions(Some(project)).expect("list");
    assert!(
        sessions.iter().any(|s| s.provider == "grok"),
        "grok missing from aggregate: {sessions:?}"
    );
    assert!(
        sessions.iter().any(|s| s.provider == "hermes"),
        "hermes missing from aggregate: {sessions:?}"
    );
}

#[test]
fn get_storage_paths_reports_grok_dirs_and_hermes_db() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("storage-grok-hermes");
    let home = crate::paths::home_dir().unwrap();

    let project = "/Users/z/proj-storage";
    write_grok_session(
        &home.join(".grok").join("sessions"),
        "%2Fproj-storage", "01920000-bcbc-7000-8000-000000000001",
        project, "2026-07-03T10:00:00Z", None,
    );
    // A foreign grok cwd dir must NOT be reported.
    write_grok_session(
        &home.join(".grok").join("sessions"),
        "%2Fother", "01920000-bcbc-7000-8000-000000000002",
        "/Users/z/other", "2026-07-03T10:00:00Z", None,
    );
    let _conn = seed_hermes_db(&home.join(".hermes").join("state.db"));

    let paths = get_storage_paths(project).expect("get_storage_paths");
    assert_eq!(paths.grok_sessions_dirs.len(), 1, "got: {paths:?}");
    assert!(paths.grok_sessions_dirs[0].ends_with("%2Fproj-storage"));
    assert!(
        paths
            .hermes_state_db
            .as_ref()
            .is_some_and(|p| p.replace('\\', "/").ends_with(".hermes/state.db")),
        "got: {paths:?}"
    );
}

#[test]
fn extract_worktree_branch_picks_up_branch_suffix() {
    assert_eq!(
        extract_worktree_branch("/repo/.worktrees/feature-x"),
        Some("feature-x".to_string())
    );
    assert_eq!(extract_worktree_branch("/repo"), None);
    assert_eq!(extract_worktree_branch(""), None);
}

#[test]
fn parse_claude_sessions_returns_empty_when_history_file_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("claude-empty");
    // fresh HOME → no ~/.claude/history.jsonl → empty Vec, not Err
    let sessions = parse_claude_sessions(None).expect("must not error");
    assert!(sessions.is_empty(), "got: {sessions:?}");
}

#[test]
fn parse_claude_sessions_parses_seeded_history() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("claude-seeded");
    // Build ~/.claude/history.jsonl with two entries — same
    // session_id repeated to exercise the SessionAccumulator
    // collapse logic.
    let claude_dir = crate::paths::home_dir().unwrap().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let history = "\
{\"sessionId\":\"s1\",\"project\":\"/repo\",\"display\":\"first prompt\",\"timestamp\":100}
{\"sessionId\":\"s1\",\"project\":\"/repo\",\"display\":\"second prompt\",\"timestamp\":200}
{\"sessionId\":\"s2\",\"project\":\"/other\",\"display\":\"only one\",\"timestamp\":50}
";
    std::fs::write(claude_dir.join("history.jsonl"), history).unwrap();
    let mut sessions = parse_claude_sessions(None).expect("parse");
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    assert_eq!(sessions.len(), 2, "got: {sessions:?}");
    let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
    assert_eq!(s1.message_count, 2, "two entries collapse into one");
    assert_eq!(s1.timestamp, 200, "last_timestamp wins");
    assert_eq!(s1.title, "first prompt", "first_display wins (lowest ts)");
    assert_eq!(s1.project, "/repo");
    assert_eq!(s1.provider, "claude");
}

#[test]
fn parse_claude_sessions_filters_by_project() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("claude-filter");
    let claude_dir = crate::paths::home_dir().unwrap().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let history = "\
{\"sessionId\":\"a\",\"project\":\"/repo\",\"display\":\"x\",\"timestamp\":1}
{\"sessionId\":\"b\",\"project\":\"/repo/.worktrees/feature\",\"display\":\"y\",\"timestamp\":2}
{\"sessionId\":\"c\",\"project\":\"/other\",\"display\":\"z\",\"timestamp\":3}
";
    std::fs::write(claude_dir.join("history.jsonl"), history).unwrap();
    let sessions = parse_claude_sessions(Some("/repo")).expect("parse");
    // /repo and /repo/.worktrees/feature both match the project
    // family — should yield 2 sessions, not the /other one.
    assert_eq!(sessions.len(), 2, "got: {sessions:?}");
    assert!(sessions.iter().any(|s| s.session_id == "a"));
    assert!(sessions.iter().any(|s| s.session_id == "b"));
    // The worktree session carries an origin_branch tag.
    let b = sessions.iter().find(|s| s.session_id == "b").unwrap();
    assert_eq!(b.origin_branch.as_deref(), Some("feature"));
}

/// Clone-to: history.jsonl has no dest-path lines, but session
/// `.jsonl` files already live under `~/.claude/projects/<slug>/`.
/// Disk union must surface them when a project filter is set.
#[test]
fn parse_claude_sessions_unions_disk_when_history_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("claude-disk-only");
    let home = crate::paths::home_dir().unwrap();
    let project = "/Users/z/proj-disk-only";
    let hash = claude_project_hash(project);
    let sess_dir = home
        .join(".claude")
        .join("projects")
        .join(&hash);
    std::fs::create_dir_all(&sess_dir).unwrap();
    let sid = "disk-session-aaaaaaaa";
    std::fs::write(
        sess_dir.join(format!("{sid}.jsonl")),
        b"{\"type\":\"user\",\"message\":{\"content\":\"hello from disk\"}}\n",
    )
    .unwrap();
    // No history.jsonl at all.
    assert!(!home.join(".claude").join("history.jsonl").exists());

    let sessions = parse_claude_sessions(Some(project)).expect("parse");
    assert_eq!(sessions.len(), 1, "got: {sessions:?}");
    let s = &sessions[0];
    assert_eq!(s.session_id, sid);
    assert_eq!(s.provider, "claude");
    assert_eq!(s.project, project);
    assert!(
        s.title.contains("hello from disk") || s.title.starts_with("Session "),
        "title should be content or short fallback, got {:?}",
        s.title
    );
    // Unfiltered path must NOT walk disk (history missing → empty).
    let unfiltered = parse_claude_sessions(None).expect("unfiltered");
    assert!(
        unfiltered.is_empty(),
        "disk union only when filter is Some; got: {unfiltered:?}"
    );
}

/// History has one session; disk has another under the same slug → both.
#[test]
fn parse_claude_sessions_unions_history_and_disk() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("claude-hist-disk");
    let home = crate::paths::home_dir().unwrap();
    let project = "/Users/z/proj-hist-disk";
    let hash = claude_project_hash(project);

    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let history = format!(
        "{{\"sessionId\":\"hist-sid-11111111\",\"project\":{project:?},\"display\":\"from history\",\"timestamp\":100}}\n"
    );
    std::fs::write(claude_dir.join("history.jsonl"), history).unwrap();

    let sess_dir = claude_dir.join("projects").join(&hash);
    std::fs::create_dir_all(&sess_dir).unwrap();
    let disk_sid = "disk-sid-22222222";
    std::fs::write(
        sess_dir.join(format!("{disk_sid}.jsonl")),
        b"{\"type\":\"user\",\"message\":{\"content\":\"only on disk\"}}\n",
    )
    .unwrap();
    // Empty file must be ignored (never-run session id pre-allocation).
    std::fs::write(sess_dir.join("empty-sid-00000000.jsonl"), b"").unwrap();

    let sessions = parse_claude_sessions(Some(project)).expect("parse");
    assert!(
        sessions.iter().any(|s| s.session_id == "hist-sid-11111111"),
        "history session missing: {sessions:?}"
    );
    assert!(
        sessions.iter().any(|s| s.session_id == disk_sid),
        "disk session missing: {sessions:?}"
    );
    assert!(
        !sessions.iter().any(|s| s.session_id == "empty-sid-00000000"),
        "empty jsonl must be skipped: {sessions:?}"
    );
    assert_eq!(sessions.len(), 2, "got: {sessions:?}");
    let hist = sessions
        .iter()
        .find(|s| s.session_id == "hist-sid-11111111")
        .unwrap();
    assert_eq!(hist.title, "from history");
}

#[test]
fn parse_cursor_sessions_returns_empty_when_chats_dir_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("cursor-empty");
    let sessions = parse_cursor_sessions(None).expect("must not error");
    assert!(sessions.is_empty());
}

#[test]
fn parse_cursor_ide_sessions_returns_empty_when_workspace_storage_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("cursor-ide-empty");
    let sessions = parse_cursor_ide_sessions(None).expect("must not error");
    assert!(sessions.is_empty());
}

#[test]
fn parse_gemini_sessions_returns_empty_when_tmp_dir_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("gemini-empty");
    let sessions = parse_gemini_sessions(None).expect("must not error");
    assert!(sessions.is_empty());
}

#[test]
fn parse_codex_sessions_returns_empty_when_sessions_dir_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("codex-empty");
    let sessions = parse_codex_sessions(None).expect("must not error");
    assert!(sessions.is_empty());
}

#[test]
fn list_all_sessions_returns_empty_for_clean_home() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("all-empty");
    let sessions = list_all_sessions(None).expect("must not error");
    assert!(
        sessions.is_empty(),
        "no provider data → empty; got: {sessions:?}"
    );
}

#[test]
fn get_storage_paths_reports_none_for_clean_home() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("storage-empty");
    let paths = get_storage_paths("/some/project").expect("get_storage_paths");
    assert!(paths.claude_history_file.is_none(), "got: {paths:?}");
    assert!(paths.codex_history_file.is_none());
    assert!(paths.claude_sessions_dirs.is_empty());
    assert!(paths.cursor_chats_dirs.is_empty());
    assert!(paths.gemini_chats_dirs.is_empty());
    assert!(paths.pi_chats_dirs.is_empty());
    assert!(paths.codex_sessions_dirs.is_empty());
    assert!(paths.grok_sessions_dirs.is_empty());
    assert!(paths.hermes_state_db.is_none());
}

#[test]
fn get_storage_paths_picks_up_claude_history_file_when_present() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("storage-claude");
    let claude_dir = crate::paths::home_dir().unwrap().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("history.jsonl"), "").unwrap();
    let paths = get_storage_paths("/proj").expect("get_storage_paths");
    assert!(paths.claude_history_file.is_some(), "got: {paths:?}");
    assert!(
        paths
            .claude_history_file
            .as_ref()
            .unwrap()
            .replace('\\', "/")
            .ends_with(".claude/history.jsonl")
    );
}

#[test]
fn discover_ide_sessions_returns_empty_when_cursor_workspace_storage_missing() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("ide-empty");
    let sessions = discover_ide_sessions("/proj").expect("must not error");
    assert!(sessions.is_empty());
}

// ── DB-backed custom names + pin (chat_session_names table) ────
//
// The in-memory test DB is shared across the whole test binary, so
// we use unique session-ID prefixes to keep these from colliding
// with each other. Lock is intentionally separate from the HOME
// lock — these tests don't mutate HOME.

static UNIT6_DB_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[test]
fn rename_session_and_get_custom_names_round_trip() {
    let _g = UNIT6_DB_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let sid = format!("u6-rn-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    rename_session("claude", &sid, "My Pinned Chat").expect("rename");
    let names = get_custom_names().expect("get_custom_names");
    let key = format!("claude:{}", sid);
    assert_eq!(names.get(&key).map(|s| s.as_str()), Some("My Pinned Chat"));
    // ON CONFLICT updates the existing row → not a new one.
    rename_session("claude", &sid, "Renamed Again").expect("rename 2");
    let names2 = get_custom_names().expect("get_custom_names 2");
    assert_eq!(names2.get(&key).map(|s| s.as_str()), Some("Renamed Again"));
    rename_session("claude", &sid, "  Trimmed Name  ").expect("rename padded");
    let names3 = get_custom_names().expect("get_custom_names 3");
    assert_eq!(
        names3.get(&key).map(|s| s.as_str()),
        Some("Trimmed Name"),
        "rename_session must store the trimmed display name"
    );
}

#[test]
fn rename_session_rejects_slug_collision_with_disk_only_chat() {
    let _g = UNIT6_DB_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let a = format!("u6-disk-a-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let b = format!("u6-disk-b-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    rename_session("claude", &a, "Disk Only Collider").expect("first disk-only rename");
    let err = rename_session("claude", &b, "Disk Only Collider")
        .expect_err("second slug must fail loud");
    assert!(
        err.contains("disk-only-collider") || err.to_lowercase().contains("already"),
        "collision error must name the slug, got: {err}"
    );
    let names = get_custom_names().expect("names");
    assert_eq!(
        names.get(&format!("claude:{a}")).map(|s| s.as_str()),
        Some("Disk Only Collider")
    );
    assert!(
        names.get(&format!("claude:{b}")).is_none(),
        "losing rename must not write a colliding custom_name"
    );
}

#[test]
fn rename_session_rejects_slash_and_colon() {
    let _g = UNIT6_DB_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let sid = format!("u6-rn-slash-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let err = rename_session("claude", &sid, "sales/reviewer")
        .expect_err("slash in custom_name must fail loud");
    assert!(
        err.contains('/') || err.to_lowercase().contains("path"),
        "error must mention path/slash, got: {err}"
    );
    let err2 = rename_session("claude", &sid, "sales:reviewer")
        .expect_err("colon in custom_name must fail loud");
    assert!(
        err2.contains(':') || err2.to_lowercase().contains("path"),
        "error must mention path/colon, got: {err2}"
    );
}

#[test]
fn list_all_sessions_overlays_custom_name_without_mutating_title() {
    let _home = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _db = UNIT6_DB_LOCK.lock();
    let _h = U6HomeGuard::new("list-custom-name");
    let home = crate::paths::home_dir().expect("home");

    let project = "/Users/z/proj-custom-name";
    let sid = format!("01920000-cccc-7000-8000-{:012x}", std::process::id() as u64);
    write_grok_session(
        &home.join(".grok").join("sessions"),
        "%2Fproj-custom-name",
        &sid,
        project,
        "2026-07-03T10:00:00Z",
        None,
    );
    rename_session("grok", &sid, "  My Custom Name  ").expect("rename padded");

    let sessions = list_all_sessions(Some(project)).expect("list");
    let found = sessions
        .iter()
        .find(|s| s.session_id == sid)
        .expect("listed grok session");
    assert_eq!(found.title.as_str(), "test session");
    assert_eq!(found.custom_name.as_deref(), Some("My Custom Name"));

    rename_session("grok", &sid, "   ").expect("blank rename");
    let sessions_blank = list_all_sessions(Some(project)).expect("list blank");
    let found_blank = sessions_blank
        .iter()
        .find(|s| s.session_id == sid)
        .expect("listed grok session after blank");
    assert_eq!(found_blank.title.as_str(), "test session");
    assert_eq!(found_blank.custom_name.as_deref(), None);

    let arch_sid = format!("u6-arch-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO chat_session_names \
                 (provider, session_id, custom_name, pinned, updated_at, archived, \
                  archive_title, archive_project_path) \
                 VALUES ('claude', ?1, 'Custom Archive Name', 0, unixepoch(), 1, \
                         'Transcript Archive Title', ?2)",
            rusqlite::params![arch_sid, project],
        )
        .expect("insert archived row");
    }
    let sessions_arch = list_all_sessions(Some(project)).expect("list archive");
    let archived = sessions_arch
        .iter()
        .find(|s| s.session_id == arch_sid)
        .expect("listed archived session");
    assert_eq!(archived.title.as_str(), "Transcript Archive Title");
    assert_eq!(archived.custom_name.as_deref(), Some("Custom Archive Name"));
    assert!(archived.archived);
    // Leave no archived row behind: `list_all_sessions(None)` lists every
    // archived row in the shared test DB, so the clean-home test failed
    // whenever this one ran first.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "DELETE FROM chat_session_names WHERE provider = 'claude' AND session_id = ?1",
            rusqlite::params![arch_sid],
        )
        .expect("clean archived row");
    }
}

#[test]
fn toggle_pin_writes_and_clears_pinned_flag() {
    let _g = UNIT6_DB_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let sid = format!("u6-pin-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    let key = format!("cursor:{}", sid);
    // Pin it.
    toggle_pin("cursor", &sid, true).expect("pin");
    let pinned = get_pinned().expect("get_pinned");
    assert!(pinned.iter().any(|k| k == &key), "got: {pinned:?}");
    // Unpin it.
    toggle_pin("cursor", &sid, false).expect("unpin");
    let pinned2 = get_pinned().expect("get_pinned 2");
    assert!(
        !pinned2.iter().any(|k| k == &key),
        "still pinned after unpin: {pinned2:?}"
    );
}

// ── Cross-provider dedup tests (closes #551 / audit #555 gap) ──
//
// Phase 2.5 fix #550 added a dedupe pass to `parse_gemini_sessions`
// after observing that the Gemini CLI checkpoints a single logical
// session into multiple `.jsonl` files (every file in the chats/
// dir carries the SAME `sessionId` header). Without dedup the
// history list surfaces duplicate React keys.
//
// The Cursor parser carries an equivalent `best_by_id` collapse
// (see line ~906: `match best_by_id.get(&chat_id) { ... }`). This
// test exercises that path with the same chat_id appearing under
// two different project-hash directories.
//
// 0.39.0 follow-up: Pi, Codex, and Cursor IDE parsers all received
// the same `HashMap<session_id, ChatSession>` dedup tail. Each
// provider has its own quirk that produces dupes — Pi/Codex
// checkpoint resumed sessions into multiple files, Cursor IDE can
// mirror a composer across workspaceStorage dirs. Tests below cover
// all three.

#[test]
fn parse_cursor_sessions_dedupes_chat_id_across_hash_dirs() {
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("cursor-dedup");
    let cursor_chats = crate::paths::home_dir().unwrap().join(".cursor").join("chats");
    // Two project-hash dirs, same chat_id under each.
    let hash_a = cursor_chats.join("00000000000000000000000000000001");
    let hash_b = cursor_chats.join("00000000000000000000000000000002");
    let chat_id = "shared-chat-id-xyz";
    let chat_a = hash_a.join(chat_id);
    let chat_b = hash_b.join(chat_id);
    std::fs::create_dir_all(&chat_a).unwrap();
    std::fs::create_dir_all(&chat_b).unwrap();
    // Empty (non-sqlite) store.db files trigger the file_ts
    // fallback path (read_cursor_chat_meta returns None →
    // generic "Cursor session ..." title). The parser still
    // includes them in best_by_id, which is exactly what we
    // need to exercise the dedup branch.
    std::fs::write(chat_a.join("store.db"), b"").unwrap();
    std::fs::write(chat_b.join("store.db"), b"").unwrap();

    let sessions = parse_cursor_sessions(None).expect("parse");
    // Same chat_id under two hash dirs → 1 session in output.
    let matching: Vec<_> = sessions
        .iter()
        .filter(|s| s.session_id == chat_id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected dedup by chat_id; got: {sessions:?}"
    );
    assert_eq!(matching[0].provider, "cursor");
}

#[test]
fn parse_pi_sessions_dedupes_session_id_across_slug_dirs() {
    // Pi checkpoints a resumed session into a fresh .jsonl under a
    // (potentially) different slug directory, but the header `id`
    // is the same logical session. Without dedup we'd surface the
    // same session twice. Build two slug dirs each containing a
    // .jsonl with the same session id and an early vs. late
    // timestamp — the late one must win.
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("pi-dedup");
    let sessions_root = crate::paths::home_dir()
        .unwrap()
        .join(".pi")
        .join("agent")
        .join("sessions");
    let slug_a = sessions_root.join("slug-a");
    let slug_b = sessions_root.join("slug-b");
    std::fs::create_dir_all(&slug_a).unwrap();
    std::fs::create_dir_all(&slug_b).unwrap();

    let session_id = "pi-shared-session-zzz";
    // Earlier checkpoint under slug-a (timestamp 2026-01-01).
    let early = format!(
        "{{\"type\":\"session\",\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-01-01T00:00:00Z\"}}\n\
             {{\"type\":\"message\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"first\"}}]}}}}\n"
    );
    std::fs::write(slug_a.join("checkpoint-1.jsonl"), early).unwrap();

    // Later checkpoint under slug-b (timestamp 2026-05-01).
    let late = format!(
        "{{\"type\":\"session\",\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-05-01T00:00:00Z\"}}\n\
             {{\"type\":\"message\",\"timestamp\":\"2026-05-01T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"second resume\"}}]}}}}\n"
    );
    std::fs::write(slug_b.join("checkpoint-2.jsonl"), late).unwrap();

    let sessions = parse_pi_sessions(None).expect("parse");
    let matching: Vec<_> = sessions
        .iter()
        .filter(|s| s.session_id == session_id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected dedup by session_id across slug dirs; got: {sessions:?}",
    );
    assert_eq!(matching[0].provider, "pi");
    // Latest checkpoint wins → its title is "second resume".
    assert_eq!(
        matching[0].title, "second resume",
        "latest-timestamp entry must win",
    );
}

#[test]
fn parse_codex_sessions_dedupes_session_id_across_day_dirs() {
    // Codex stores each resume slice under a fresh
    // YYYY/MM/DD/...jsonl path, but the session_meta header carries
    // the SAME `payload.id`. Without dedup the history list shows
    // one row per resume.
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("codex-dedup");
    let sessions_root = crate::paths::home_dir().unwrap().join(".codex").join("sessions");
    let day_a = sessions_root.join("2026").join("01").join("01");
    let day_b = sessions_root.join("2026").join("05").join("01");
    std::fs::create_dir_all(&day_a).unwrap();
    std::fs::create_dir_all(&day_b).unwrap();

    let session_id = "codex-shared-session-abc12345";
    let header_early = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-01-01T00:00:00Z\"}}}}\n"
    );
    let header_late = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-05-01T00:00:00Z\"}}}}\n"
    );

    // Write the two .jsonl files with naturally-staggered mtimes —
    // the parser keys `timestamp` on file mtime when present, so as
    // long as `late.mtime > early.mtime` the dedup tail keeps the
    // later one. Sleeping between writes is enough on every filesystem
    // we ship to (APFS / ext4 / btrfs all carry sub-second mtime).
    let path_early = day_a.join("rollout-early.jsonl");
    let path_late = day_b.join("rollout-late.jsonl");
    std::fs::write(&path_early, header_early).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path_late, header_late).unwrap();

    let sessions = parse_codex_sessions(None).expect("parse");
    let matching: Vec<_> = sessions
        .iter()
        .filter(|s| s.session_id == session_id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected dedup by session_id across day dirs; got: {sessions:?}",
    );
    assert_eq!(matching[0].provider, "codex");
}

#[test]
fn parse_cursor_ide_sessions_dedupes_composer_id_across_workspaces() {
    // A single Cursor IDE composer can appear in multiple
    // workspaceStorage directories — e.g. when Cursor migrates
    // state between workspace IDs or the user opens the same
    // project from two different folder URIs. The state.vscdb
    // SQLite blob is the source of truth; we build two minimal
    // ones with the same composerId but different lastUpdatedAt
    // values to confirm dedup keeps the latest.
    let _g = UNIT6_HOME_LOCK.lock();
    let _scoped_db = crate::db::scoped_for_test();
    let _h = U6HomeGuard::new("cursor-ide-dedup");
    let workspace_dir = crate::paths::home_dir()
        .unwrap()
        .join("Library/Application Support/Cursor/User/workspaceStorage");
    let ws_a = workspace_dir.join("ws-a");
    let ws_b = workspace_dir.join("ws-b");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();

    // Both workspaces point at the same folder URI so neither
    // gets filtered (filter is None anyway, but we still need
    // valid JSON for the parser to read past workspace.json).
    let ws_json = r#"{"folder":"file:///repo"}"#;
    std::fs::write(ws_a.join("workspace.json"), ws_json).unwrap();
    std::fs::write(ws_b.join("workspace.json"), ws_json).unwrap();

    let composer_id = "ide-shared-composer-xyz";
    // Helper: build a state.vscdb with one composer row.
    let seed_db = |path: &std::path::Path, name: &str, last_updated: i64| {
        let conn = rusqlite::Connection::open(path).expect("open state.vscdb");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value TEXT);",
        )
        .expect("create ItemTable");
        let composer_json = format!(
            r#"{{"allComposers":[{{"composerId":"{composer_id}","name":"{name}","lastUpdatedAt":{last_updated},"createdAt":1000}}]}}"#
        );
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('composer.composerData', ?1)",
            rusqlite::params![composer_json],
        )
        .expect("insert composer row");
    };

    seed_db(&ws_a.join("state.vscdb"), "Older Name", 1_000_000);
    seed_db(&ws_b.join("state.vscdb"), "Newer Name", 2_000_000);

    let sessions = parse_cursor_ide_sessions(None).expect("parse");
    let matching: Vec<_> = sessions
        .iter()
        .filter(|s| s.session_id == composer_id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected dedup by composer_id; got: {sessions:?}",
    );
    assert_eq!(matching[0].provider, "cursor");
    // Latest lastUpdatedAt wins → its name was "Newer Name".
    assert_eq!(
        matching[0].title, "Newer Name",
        "latest-lastUpdatedAt entry must win",
    );
}
