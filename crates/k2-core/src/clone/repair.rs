//! GH#23 self-heal — repair workspaces that were Clone-to'd on 0.39.38 /
//! 0.39.39, BEFORE the unpack-time embedded-path rewrite landed.
//!
//! Those clones got the session `.jsonl` files into the correct
//! recomputed slug dir under `~/.claude/projects/`, but left every
//! embedded `cwd` (and `originalCwd`, worktree paths, tool-call
//! `file_path` args) pointing at the SOURCE machine's absolute workspace
//! path. Claude `/resume` filters by matching that embedded `cwd` against
//! the process cwd, so the picker comes up EMPTY on the destination even
//! though the history is sitting right there.
//!
//! This repair derives BOTH paths per-machine — no hardcoded shapes, no
//! assumptions about the source layout:
//!
//!   * **DEST** path = the registered project's `path` (authoritative on
//!     THIS machine).
//!   * **SOURCE** path = the STALE `cwd` embedded in the first parseable
//!     `.jsonl` line of the project's slug dir (whatever machine it was
//!     cloned from).
//!
//! When `embedded_cwd != project.path`, every `.jsonl` under the slug dir
//! gets a raw SOURCE→DEST substring rewrite (same one-pass byte replace
//! the unpack path uses). Fully idempotent: once the embedded cwd matches
//! the project path, the mismatch check short-circuits and nothing is
//! touched. The check reads only the first line per slug dir, so a clean
//! boot is cheap.
//!
//! Safe + non-fatal: a parse error or read error on one file skips that
//! file (the repair moves on); a bad slug dir skips that project. Nothing
//! here can abort daemon boot.

use crate::chat_history::claude_project_hash;
use std::path::{Path, PathBuf};

/// The PRE-0.39.44 ("legacy") Claude slug encoder, kept ONLY to detect
/// stale dirs that an earlier K2SO build wrote/unpacked into.
///
/// The old rule was `replace("/.","/-").replace('/',"-").replace(' ',"-")`:
/// it PRESERVED `_` and any mid-component `.`, and never canonicalized
/// symlinks. The current [`claude_project_hash`] now matches Claude exactly
/// (every non-`[a-zA-Z0-9-]` char → `-`, plus symlink canonicalization), so
/// for a workspace path with `_`, a dotted component, or a symlinked prefix,
/// the OLD and NEW slugs DIFFER — and a prior clone may have unpacked
/// sessions into the OLD (wrong) dir. [`migrate_legacy_slug_dirs`] uses this
/// to find + merge those stale dirs into the correct new one.
///
/// This intentionally does NOT canonicalize (it reproduces the literal-path
/// behavior of the old encoder, which is what was actually written to disk).
fn claude_project_hash_legacy(project_path: &str) -> String {
    project_path
        .replace("/.", "/-")
        .replace('/', "-")
        .replace(' ', "-")
}

/// Per-project legacy-slug migration outcome (for logging).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlugMigration {
    /// The registered project path (DEST, this machine).
    pub project_path: String,
    /// The OLD-encoder slug dir name that was found stale on disk.
    pub legacy_slug: String,
    /// The NEW-encoder (correct) slug dir the files were moved into.
    pub new_slug: String,
    /// Number of `.jsonl` files moved/merged into the new dir.
    pub files_moved: usize,
}

/// Aggregate result of a legacy-slug migration sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlugMigrationReport {
    /// Projects whose OLD slug dir existed and was migrated (one entry each).
    pub migrated: Vec<SlugMigration>,
}

impl SlugMigrationReport {
    /// Total `.jsonl` files moved across all migrated projects.
    pub fn total_files_moved(&self) -> usize {
        self.migrated.iter().map(|m| m.files_moved).sum()
    }
}

/// GH#25 self-heal — migrate session dirs that an EARLIER K2SO build wrote
/// under the OLD (underscore/literal-path-preserving) Claude slug into the
/// correct NEW-encoder slug dir.
///
/// For each registered project we compute BOTH slugs. When they differ (a
/// `_`/dotted/symlinked path — exactly the #25 cases) and the OLD dir exists
/// under `home/.claude/projects/`, we move/merge its `.jsonl` files into the
/// NEW dir. After this runs, the existing #23 embedded-cwd repair
/// ([`repair_cloned_session_paths`]) — which keys off the NEW slug — picks the
/// migrated sessions up on this and future boots.
///
/// Idempotent + non-fatal:
/// - If OLD == NEW (plain path, no symlink) → nothing to do, skipped.
/// - If the OLD dir doesn't exist → skipped (the common/clean case).
/// - If the NEW dir already has the files (a prior run) → re-running finds
///   the OLD dir empty/gone and does nothing.
/// - A file collision in the NEW dir (same `<id>.jsonl`) is left alone (the
///   NEW dir is authoritative — Claude wrote it); only files NOT already
///   present are moved. A copy/remove error on one file skips it.
/// - On a clean install the OLD dir never exists, so this is a cheap
///   per-project `is_dir()` check.
pub fn migrate_legacy_slug_dirs(home: &Path, project_paths: &[String]) -> SlugMigrationReport {
    let projects_dir = home.join(".claude").join("projects");
    let mut report = SlugMigrationReport::default();

    for project_path in project_paths {
        let new_slug = claude_project_hash(project_path);
        let legacy_slug = claude_project_hash_legacy(project_path);
        // Same slug → the path has no `_`/dot/symlink divergence; no stale
        // dir is possible. (Also guards the degenerate equal-string case.)
        if legacy_slug == new_slug {
            continue;
        }

        let legacy_dir = projects_dir.join(&legacy_slug);
        if !legacy_dir.is_dir() {
            continue;
        }
        let new_dir = projects_dir.join(&new_slug);

        let files_moved = move_jsonl_files(&legacy_dir, &new_dir);
        if files_moved > 0 {
            report.migrated.push(SlugMigration {
                project_path: project_path.clone(),
                legacy_slug,
                new_slug,
                files_moved,
            });
        }
    }

    report
}

/// Move every `.jsonl` file directly under `from_dir` into `to_dir`,
/// creating `to_dir` if needed. A file whose name already exists in
/// `to_dir` is left in place (the dest dir is authoritative — Claude wrote
/// it). Returns the count actually moved. Copy/remove errors on one file
/// skip it (non-fatal).
fn move_jsonl_files(from_dir: &Path, to_dir: &Path) -> usize {
    let files = jsonl_files(from_dir);
    if files.is_empty() {
        return 0;
    }
    if std::fs::create_dir_all(to_dir).is_err() {
        return 0;
    }
    let mut moved = 0usize;
    for src in files {
        let Some(name) = src.file_name() else {
            continue;
        };
        let dest = to_dir.join(name);
        if dest.exists() {
            // The correct dir already has this session (Claude's own write
            // or a prior migration) — don't clobber it; leave the stale copy.
            continue;
        }
        // Prefer a rename (same filesystem); fall back to copy+remove across
        // filesystems. Either failure skips this file.
        if std::fs::rename(&src, &dest).is_ok() {
            moved += 1;
            continue;
        }
        if std::fs::copy(&src, &dest).is_ok() && std::fs::remove_file(&src).is_ok() {
            moved += 1;
        }
    }
    moved
}

/// Per-project repair outcome (for logging).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectRepair {
    /// The registered project path (DEST, this machine).
    pub project_path: String,
    /// The stale source path discovered in the embedded `cwd`.
    pub stale_source_path: String,
    /// Number of `.jsonl` files actually rewritten.
    pub files_rewritten: usize,
}

/// Aggregate result of a repair sweep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Projects inspected (had a slug dir on disk).
    pub projects_inspected: usize,
    /// Projects that needed + got a rewrite (one entry each).
    pub repaired: Vec<ProjectRepair>,
}

impl RepairReport {
    /// Total `.jsonl` files rewritten across all repaired projects.
    pub fn total_files_rewritten(&self) -> usize {
        self.repaired.iter().map(|r| r.files_rewritten).sum()
    }
}

/// Repair every project in `project_paths`, resolving the slug dir under
/// `home/.claude/projects/`. `home` is `dirs::home_dir()` in production, a
/// temp dir in tests.
///
/// Idempotent + non-fatal — see module docs.
pub fn repair_cloned_session_paths(home: &Path, project_paths: &[String]) -> RepairReport {
    let projects_dir = home.join(".claude").join("projects");
    let mut report = RepairReport::default();

    for project_path in project_paths {
        // A bare/sentinel "path" (e.g. the `_orphan` audit row) won't have
        // a real slug dir — the existence check below skips it.
        let slug = claude_project_hash(project_path);
        let slug_dir = projects_dir.join(&slug);
        if !slug_dir.is_dir() {
            continue;
        }
        report.projects_inspected += 1;

        // STALE source path = embedded cwd from the first parseable line of
        // any `.jsonl` in this slug dir.
        let Some(stale_source) = first_embedded_cwd(&slug_dir) else {
            // No parseable cwd → nothing we can key the rewrite off. Skip.
            continue;
        };

        // Idempotent gate: already correct → no-op (the common case on a
        // clean boot, and on every boot after the first repair).
        if stale_source == *project_path {
            continue;
        }

        // Mismatch → rewrite SOURCE→DEST across every `.jsonl` here.
        let files_rewritten = rewrite_slug_dir(&slug_dir, &stale_source, project_path);
        if files_rewritten > 0 {
            report.repaired.push(ProjectRepair {
                project_path: project_path.clone(),
                stale_source_path: stale_source,
                files_rewritten,
            });
        }
    }

    report
}

/// Read the embedded `cwd` from the first parseable JSON line across the
/// `.jsonl` files in `slug_dir`. Returns the first non-empty `cwd` found.
///
/// Only the FIRST line of each file is examined (the mismatch check is
/// meant to be cheap); files are tried in directory order until one
/// yields a `cwd`. A file that fails to open / has no parseable first
/// line is skipped.
fn first_embedded_cwd(slug_dir: &Path) -> Option<String> {
    for path in jsonl_files(slug_dir) {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Parse error on this line → skip the line (non-fatal).
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(cwd) = value.get("cwd").and_then(|c| c.as_str()) {
                if !cwd.is_empty() {
                    return Some(cwd.to_string());
                }
            }
            // First parseable line had no usable `cwd` — move to next file
            // rather than scanning the whole (potentially huge) file.
            break;
        }
    }
    None
}

/// Rewrite every `.jsonl` file directly under `slug_dir`, replacing `from`
/// with `to` (raw byte substring). Returns the count of files that
/// actually changed. A read/write error on one file skips it (non-fatal).
fn rewrite_slug_dir(slug_dir: &Path, from: &str, to: &str) -> usize {
    if from.is_empty() || from == to {
        return 0;
    }
    let from_b = from.as_bytes();
    let to_b = to.as_bytes();
    let mut rewritten = 0usize;

    for path in jsonl_files(slug_dir) {
        let Ok(buf) = std::fs::read(&path) else {
            continue;
        };
        if let Some(new_buf) = super::unpack::replace_bytes(&buf, from_b, to_b) {
            if std::fs::write(&path, &new_buf).is_ok() {
                rewritten += 1;
            }
        }
    }
    rewritten
}

/// Collect the `.jsonl` files directly under `slug_dir` (non-recursive —
/// session files live flat in the slug dir; worktree variants live under
/// SIBLING `<slug>-<branch>/` dirs, which are separate registered
/// projects or handled by their own slug). Returns an empty vec on any
/// read error.
fn jsonl_files(slug_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(slug_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Write a session `.jsonl` with the given embedded `cwd` on every line.
    fn write_session(slug_dir: &Path, name: &str, cwd: &str, extra_lines: usize) {
        fs::create_dir_all(slug_dir).expect("mkdir slug");
        let mut s = String::new();
        for i in 0..=extra_lines {
            s.push_str(&format!(
                r#"{{"type":"user","cwd":{cwd:?},"originalCwd":{cwd:?},"i":{i},"toolUse":{{"file_path":{file:?}}}}}"#,
                file = format!("{cwd}/src/main.rs"),
            ));
            s.push('\n');
        }
        fs::write(slug_dir.join(name), s).expect("write session");
    }

    fn read_first_cwd(slug_dir: &Path, name: &str) -> String {
        let contents = fs::read_to_string(slug_dir.join(name)).expect("read");
        let line = contents.lines().next().expect("first line");
        let v: serde_json::Value = serde_json::from_str(line).expect("parse");
        v.get("cwd").and_then(|c| c.as_str()).unwrap().to_string()
    }

    #[test]
    fn repairs_stale_slug_dir_and_is_idempotent() {
        let home = TempDir::new("gh23-repair");
        // DEST path on THIS machine (arbitrary, with spaces).
        let dest = "/Users/alakazamlabs/AI Projects/nsi-plan01".to_string();
        // SOURCE the file was cloned from (different machine/user/layout).
        let source = "/Users/z3thon/DevProjects/Nelson Specialty Industrial/nsi-plan01";

        let slug = claude_project_hash(&dest);
        let slug_dir = home.path.join(".claude").join("projects").join(&slug);
        write_session(&slug_dir, "a.jsonl", source, 5);
        write_session(&slug_dir, "b.jsonl", source, 3);

        // First run: detects mismatch, rewrites both files.
        let report = repair_cloned_session_paths(&home.path, &[dest.clone()]);
        assert_eq!(report.projects_inspected, 1);
        assert_eq!(report.repaired.len(), 1, "one project repaired");
        assert_eq!(report.repaired[0].files_rewritten, 2, "both files");
        assert_eq!(report.repaired[0].stale_source_path, source);
        assert_eq!(read_first_cwd(&slug_dir, "a.jsonl"), dest);
        assert_eq!(read_first_cwd(&slug_dir, "b.jsonl"), dest);
        // originalCwd + tool file_path also rewritten (whole-file pass).
        let a = fs::read_to_string(slug_dir.join("a.jsonl")).unwrap();
        assert!(!a.contains(source), "no stale source path remains: {a}");
        assert!(a.contains(&format!("{dest}/src/main.rs")), "tool path rewritten");

        // Second run: idempotent no-op.
        let report2 = repair_cloned_session_paths(&home.path, &[dest.clone()]);
        assert_eq!(report2.projects_inspected, 1);
        assert_eq!(report2.repaired.len(), 0, "idempotent: nothing to repair");
        assert_eq!(read_first_cwd(&slug_dir, "a.jsonl"), dest);
    }

    #[test]
    fn skips_correct_dir() {
        let home = TempDir::new("gh23-repair-ok");
        let dest = "/Users/someone/My Workspaces/proj".to_string();
        let slug = claude_project_hash(&dest);
        let slug_dir = home.path.join(".claude").join("projects").join(&slug);
        // Already-correct: embedded cwd == project path.
        write_session(&slug_dir, "a.jsonl", &dest, 2);

        let report = repair_cloned_session_paths(&home.path, &[dest.clone()]);
        assert_eq!(report.projects_inspected, 1);
        assert_eq!(report.repaired.len(), 0, "no mismatch → no repair");
    }

    #[test]
    fn skips_project_without_slug_dir() {
        let home = TempDir::new("gh23-repair-missing");
        // No slug dir on disk for this project at all.
        let report = repair_cloned_session_paths(
            &home.path,
            &["/Users/x/nonexistent".to_string(), "_orphan".to_string()],
        );
        assert_eq!(report.projects_inspected, 0);
        assert_eq!(report.repaired.len(), 0);
    }

    // ── GH#25 legacy-slug migration ──────────────────────────────────

    /// Write a flat `.jsonl` file under `dir` with a single line.
    fn write_jsonl(dir: &Path, name: &str, line: &str) {
        fs::create_dir_all(dir).expect("mkdir");
        fs::write(dir.join(name), format!("{line}\n")).expect("write jsonl");
    }

    #[test]
    fn migrates_legacy_underscore_slug_dir() {
        let home = TempDir::new("gh25-migrate");
        // A `_`-containing path → OLD encoder preserved `_`, NEW collapses it.
        let project = "/Users/z/proj/scout_v3".to_string();
        let legacy = claude_project_hash_legacy(&project);
        let new = claude_project_hash(&project);
        assert_ne!(legacy, new, "this path must diverge between encoders");
        assert_eq!(legacy, "-Users-z-proj-scout_v3");
        assert_eq!(new, "-Users-z-proj-scout-v3");

        let projects = home.path.join(".claude").join("projects");
        let legacy_dir = projects.join(&legacy);
        write_jsonl(&legacy_dir, "a.jsonl", r#"{"cwd":"/Users/z/proj/scout_v3"}"#);
        write_jsonl(&legacy_dir, "b.jsonl", r#"{"cwd":"/Users/z/proj/scout_v3"}"#);

        let report = migrate_legacy_slug_dirs(&home.path, &[project.clone()]);
        assert_eq!(report.migrated.len(), 1, "one project migrated");
        assert_eq!(report.migrated[0].files_moved, 2);
        assert_eq!(report.migrated[0].legacy_slug, legacy);
        assert_eq!(report.migrated[0].new_slug, new);

        // Files now live under the NEW dir; the OLD dir's files are gone.
        let new_dir = projects.join(&new);
        assert!(new_dir.join("a.jsonl").is_file());
        assert!(new_dir.join("b.jsonl").is_file());
        assert!(!legacy_dir.join("a.jsonl").exists());

        // Idempotent: a second run finds the OLD dir empty → no-op.
        let report2 = migrate_legacy_slug_dirs(&home.path, &[project.clone()]);
        assert_eq!(report2.migrated.len(), 0, "idempotent: nothing left to move");
    }

    #[test]
    fn migrate_skips_plain_paths_with_no_divergence() {
        let home = TempDir::new("gh25-noop");
        // No `_`/dot — OLD and NEW encoders agree, so even though a slug dir
        // exists there's nothing to migrate.
        let project = "/Users/z/proj/plain".to_string();
        assert_eq!(
            claude_project_hash_legacy(&project),
            claude_project_hash(&project)
        );
        let slug = claude_project_hash(&project);
        write_jsonl(
            &home.path.join(".claude").join("projects").join(&slug),
            "a.jsonl",
            "{}",
        );
        let report = migrate_legacy_slug_dirs(&home.path, &[project]);
        assert_eq!(report.migrated.len(), 0);
    }

    #[test]
    fn migrate_does_not_clobber_existing_new_dir_files() {
        let home = TempDir::new("gh25-collision");
        let project = "/Users/z/proj/scout_v3".to_string();
        let projects = home.path.join(".claude").join("projects");
        let legacy_dir = projects.join(claude_project_hash_legacy(&project));
        let new_dir = projects.join(claude_project_hash(&project));
        // Same filename in both: OLD is stale, NEW is Claude's authoritative.
        write_jsonl(&legacy_dir, "dup.jsonl", "STALE");
        write_jsonl(&legacy_dir, "fresh.jsonl", "FROM_LEGACY");
        write_jsonl(&new_dir, "dup.jsonl", "AUTHORITATIVE");

        let report = migrate_legacy_slug_dirs(&home.path, &[project]);
        assert_eq!(report.migrated.len(), 1);
        // Only `fresh.jsonl` moved; `dup.jsonl` was NOT clobbered.
        assert_eq!(report.migrated[0].files_moved, 1);
        assert_eq!(
            fs::read_to_string(new_dir.join("dup.jsonl")).unwrap(),
            "AUTHORITATIVE\n",
            "existing new-dir file preserved"
        );
        assert_eq!(
            fs::read_to_string(new_dir.join("fresh.jsonl")).unwrap(),
            "FROM_LEGACY\n"
        );
        // The un-moved stale dup is left in the legacy dir (not deleted).
        assert!(legacy_dir.join("dup.jsonl").is_file());
    }

    #[test]
    fn migrate_skips_when_legacy_dir_absent() {
        let home = TempDir::new("gh25-absent");
        // Divergent path but no OLD dir on disk → nothing to do.
        let report = migrate_legacy_slug_dirs(&home.path, &["/Users/z/proj/scout_v3".to_string()]);
        assert_eq!(report.migrated.len(), 0);
    }

    #[test]
    fn non_fatal_on_unparseable_first_lines() {
        let home = TempDir::new("gh23-repair-garbage");
        let dest = "/Users/y/Spaced Dir/proj".to_string();
        let source = "/Users/z/other/proj";
        let slug = claude_project_hash(&dest);
        let slug_dir = home.path.join(".claude").join("projects").join(&slug);
        fs::create_dir_all(&slug_dir).unwrap();
        // File 1: garbage first line, no recoverable cwd → contributes no
        // source path but must not crash.
        fs::write(slug_dir.join("garbage.jsonl"), "not json at all\n").unwrap();
        // File 2: valid, carries the stale source path.
        write_session(&slug_dir, "good.jsonl", source, 1);

        let report = repair_cloned_session_paths(&home.path, &[dest.clone()]);
        assert_eq!(report.projects_inspected, 1);
        // The good file's cwd drives the rewrite; garbage file untouched.
        assert_eq!(report.repaired.len(), 1);
        assert_eq!(read_first_cwd(&slug_dir, "good.jsonl"), dest);
        assert_eq!(
            fs::read_to_string(slug_dir.join("garbage.jsonl")).unwrap(),
            "not json at all\n",
            "unparseable file left as-is"
        );
    }
}
