//! 0.40.4 one-go cutover: rename each workspace's legacy `.k2so/` dot-dir
//! to `.k2/` and re-point the harness fan-out symlinks that targeted it.
//!
//! Why a rename (not a per-file move): `fs::rename` on the directory is an
//! O(1), atomic re-link of the directory entry — it doesn't matter how much
//! lives inside `.k2so/`. It also works whether or not the workspace is a
//! git repo and whether the contents are tracked or gitignored. We do NOT
//! `git mv`: that would stage changes in the user's index (intrusive) and
//! fail on untracked paths; git detects the rename by content at commit /
//! `log --follow` time anyway, so history still follows a plain rename.
//!
//! Why re-point symlinks: the harness fan-out writes symlinks at the
//! workspace root (`CLAUDE.md`, `SKILL.md`, `GEMINI.md`, `AGENT.md`,
//! `.goosehints`) and under `.claude/` `.opencode/` `.pi/` that target the
//! ABSOLUTE canonical path `<root>/.k2so/skills/<name>/SKILL.md`. The
//! instant we rename the dir those symlinks dangle, so we rewrite every one
//! whose target points into the old `.k2so/` at the new `.k2/`.
//!
//! Idempotent + safe to run every boot: a workspace that already has `.k2/`
//! is skipped untouched, so only pure-legacy `.k2so/` workspaces convert,
//! and they convert exactly once.

use std::fs;
use std::path::Path;

/// The harness subdirectories the fan-out writes symlinks into. Scanned
/// (shallow-recursive) for dangling `.k2so/` targets after a rename. We do
/// NOT walk the whole workspace tree — that could wander into node_modules
/// / vendored trees — only these K2SO-owned dirs plus the root (depth 1).
const HARNESS_SYMLINK_SUBDIRS: &[&str] = &[".claude", ".opencode", ".pi", ".cursor"];

/// Outcome of migrating one workspace — for boot logging + tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DotDirMigration {
    /// `.k2so/` was renamed to `.k2/` this call.
    pub renamed: bool,
    /// How many dangling fan-out symlinks were re-pointed to `.k2/`.
    pub symlinks_repointed: usize,
    /// `.gitignore` had `.k2so/` ignore rules rewritten to `.k2/` so git
    /// treats the renamed tree the same way (previously-ignored subdirs
    /// like inbox/sessions/logs stay ignored instead of flooding `git
    /// status`, and the rename of tracked files reads as a clean diff).
    pub gitignore_rewritten: bool,
    /// Set when nothing was done; explains why (already migrated, conflict).
    pub skipped: Option<&'static str>,
}

/// Migrate one workspace root from `.k2so/` → `.k2/` (idempotent).
///
/// - No `.k2so/` dir → nothing to do (already converted, or never had one).
/// - Both `.k2/` and `.k2so/` exist → conflict; we never clobber `.k2/`,
///   so we leave both in place and report the skip (an operator can merge).
/// - Otherwise → rename, then re-point dangling fan-out symlinks.
pub fn migrate_workspace_dot_dir(root: &Path) -> DotDirMigration {
    let old = root.join(".k2so");
    let new = root.join(".k2");

    if !old.is_dir() {
        return DotDirMigration {
            skipped: Some("no legacy .k2so dir"),
            ..Default::default()
        };
    }
    if new.exists() {
        // Already-migrated workspaces don't reach here (old would be gone).
        // Both present = a genuine conflict — don't clobber the new dir.
        return DotDirMigration {
            skipped: Some("both .k2 and .k2so present — left for manual merge"),
            ..Default::default()
        };
    }

    if let Err(e) = fs::rename(&old, &new) {
        crate::log_debug!(
            "[dot-dir-migration] rename {} -> {} failed: {e}",
            old.display(),
            new.display()
        );
        return DotDirMigration {
            skipped: Some("rename failed"),
            ..Default::default()
        };
    }

    let symlinks_repointed = repoint_fanout_symlinks(root, &old, &new);
    let gitignore_rewritten = rewrite_gitignore_dot_dir(root);
    DotDirMigration {
        renamed: true,
        symlinks_repointed,
        gitignore_rewritten,
        skipped: None,
    }
}

/// Per-workspace `.k2/` tempfile hygiene — run every boot for EVERY registered
/// workspace (idempotent, no-op when clean). Reaps atomic-write tempfiles a
/// prior hard-kill orphaned. Returns the count removed.
///
/// NOTE — deliberately does NOT touch git-ignore state. The cutover's ignore
/// policy is behavior-preserving: `.k2/` is ignored ONLY where `.k2so/` was
/// already ignored (twinned by [`rewrite_gitignore_dot_dir`]). A workspace that
/// never ignored its dot-dir keeps showing it after the rename, exactly as
/// before — we don't add ignores the user never had.
pub fn ensure_dot_dir_hygiene(root: &Path) -> usize {
    let dot = root.join(".k2");
    if !dot.is_dir() {
        return 0;
    }
    sweep_stale_tempfiles(&dot)
}

/// Reap atomic-write tempfiles orphaned under `dot_dir` by a daemon killed
/// mid-write. The temp survives a hard kill by design (so the target is never
/// left half-written), but nothing cleans it up afterward, so they pile up
/// across crashes. Matches both the current `.k2-tmp.` infix and the legacy
/// `.k2so-tmp.` one. Bounded walk — the dot-dir is small and never holds
/// vendored trees. Returns the count removed.
fn sweep_stale_tempfiles(dot_dir: &Path) -> usize {
    fn is_orphan_temp(name: &str) -> bool {
        name.contains(".k2-tmp.") || name.contains(".k2so-tmp.")
    }
    fn walk(dir: &Path, depth: u8, removed: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, removed);
                }
            } else if entry.file_name().to_str().map_or(false, is_orphan_temp)
                && fs::remove_file(&path).is_ok()
            {
                *removed += 1;
            }
        }
    }
    let mut removed = 0;
    walk(dot_dir, 3, &mut removed);
    removed
}

/// Make the repo-root `.gitignore` cover `.k2/` for every `.k2so/` ignore
/// rule. ADDITIVE (belt-and-suspenders): the original `.k2so/` rule is KEPT
/// and a `.k2/` twin is added right after it, so BOTH dot-dir names stay
/// ignored through any transition — matching the resolver, which tolerates
/// either `.k2/` or `.k2so/` indefinitely. Without this, the
/// previously-ignored `.k2/inbox`/`sessions`/`logs` would flood `git
/// status` the instant the dir is renamed.
///
/// SURGICAL: only entries whose path is the workspace dot-dir (`.k2so`,
/// `.k2so/`, `.k2so/<sub>`, optionally `/`-anchored) get a twin. Refs that
/// merely contain `k2so` — `.cursor/rules/k2so.mdc`,
/// `.opencode/agent/k2so*.md`, `crates/k2so-core/...` — are LEFT ALONE (the
/// skill/crate is still named k2so; only the dot-dir moved). Twins that the
/// file already has are not duplicated. Returns true if anything was added.
fn rewrite_gitignore_dot_dir(root: &Path) -> bool {
    let gi = root.join(".gitignore");
    let Ok(content) = fs::read_to_string(&gi) else {
        return false;
    };
    // Lines already in the file (trimmed) — so we never add a duplicate twin.
    let existing: std::collections::HashSet<String> =
        content.lines().map(|l| l.trim().to_string()).collect();
    let mut added: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for line in content.lines() {
        out.push(line.to_string());
        if line_is_dot_dir_rule(line) {
            let twin = line.replacen(".k2so", ".k2", 1);
            let key = twin.trim().to_string();
            if !existing.contains(&key) && added.insert(key) {
                out.push(twin);
            }
        }
    }
    if added.is_empty() {
        return false;
    }
    let mut joined = out.join("\n");
    if content.ends_with('\n') {
        joined.push('\n');
    }
    if let Err(e) = crate::fs_atomic::atomic_write_str(&gi, &joined) {
        crate::log_debug!("[dot-dir-migration] rewrite .gitignore failed: {e}");
        return false;
    }
    true
}

/// True iff a `.gitignore` line's pattern is the `.k2so` workspace dot-dir
/// (exact, or a path under it), so it should retarget to `.k2`. Tolerates a
/// leading `/` anchor and a trailing `/`. Does NOT match `.k2so`-containing
/// substrings that are really skill/crate names (`.../k2so.mdc`,
/// `crates/k2so-core/...`).
fn line_is_dot_dir_rule(line: &str) -> bool {
    let pat = line.trim();
    if pat.is_empty() || pat.starts_with('#') {
        return false;
    }
    let pat = pat.strip_prefix('/').unwrap_or(pat);
    pat == ".k2so" || pat == ".k2so/" || pat.starts_with(".k2so/")
}

/// Re-point every K2SO fan-out symlink whose target points into `old_dir`
/// at the matching path under `new_dir`. Targeted scan: the workspace root
/// (depth 1) plus the known harness subdirs — the only places the fan-out
/// ever writes symlinks.
fn repoint_fanout_symlinks(root: &Path, old_dir: &Path, new_dir: &Path) -> usize {
    let mut fixed = 0;
    repoint_in_dir_shallow(root, old_dir, new_dir, &mut fixed);
    for sub in HARNESS_SYMLINK_SUBDIRS {
        repoint_in_dir_recursive(&root.join(sub), old_dir, new_dir, &mut fixed);
    }
    fixed
}

/// Re-point symlinks among the immediate entries of `dir` (no recursion).
fn repoint_in_dir_shallow(dir: &Path, old_dir: &Path, new_dir: &Path, fixed: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            if repoint_one(&path, old_dir, new_dir) {
                *fixed += 1;
            }
        }
    }
}

/// Re-point symlinks under `dir`, recursing into real subdirectories
/// (bounded to the K2SO-owned harness trees — they're tiny).
fn repoint_in_dir_recursive(dir: &Path, old_dir: &Path, new_dir: &Path, fixed: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            if repoint_one(&path, old_dir, new_dir) {
                *fixed += 1;
            }
        } else if path.is_dir() {
            repoint_in_dir_recursive(&path, old_dir, new_dir, fixed);
        }
    }
}

/// If `link`'s target starts with `old_dir`, rewrite it to point under
/// `new_dir`. Returns true when the symlink was re-pointed.
fn repoint_one(link: &Path, old_dir: &Path, new_dir: &Path) -> bool {
    let Ok(target) = fs::read_link(link) else {
        return false;
    };
    let Ok(rel) = target.strip_prefix(old_dir) else {
        return false;
    };
    let new_target = new_dir.join(rel);
    // Replace atomically-ish: remove the dangling link, recreate it.
    if fs::remove_file(link).is_err() {
        return false;
    }
    match std::os::unix::fs::symlink(&new_target, link) {
        Ok(()) => true,
        Err(e) => {
            crate::log_debug!(
                "[dot-dir-migration] re-point {} -> {} failed: {e}",
                link.display(),
                new_target.display()
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-dotmig-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn symlink(target: &Path, link: &Path) {
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn renames_k2so_to_k2_and_is_idempotent() {
        let root = tmp("rename");
        fs::create_dir_all(root.join(".k2so/inbox")).unwrap();
        fs::write(root.join(".k2so/PROJECT.md"), b"hello").unwrap();

        let r1 = migrate_workspace_dot_dir(&root);
        assert!(r1.renamed, "first run renames");
        assert!(root.join(".k2").is_dir(), ".k2/ exists after migrate");
        assert!(!root.join(".k2so").exists(), ".k2so/ is gone");
        assert_eq!(
            fs::read_to_string(root.join(".k2/PROJECT.md")).unwrap(),
            "hello",
            "contents moved intact"
        );

        // Second run is a no-op.
        let r2 = migrate_workspace_dot_dir(&root);
        assert!(!r2.renamed, "second run does nothing");
        assert_eq!(r2.skipped, Some("no legacy .k2so dir"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_k2so_is_a_noop() {
        let root = tmp("nolegacy");
        let r = migrate_workspace_dot_dir(&root);
        assert!(!r.renamed);
        assert_eq!(r.skipped, Some("no legacy .k2so dir"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn does_not_clobber_existing_k2() {
        let root = tmp("conflict");
        fs::create_dir_all(root.join(".k2so")).unwrap();
        fs::create_dir_all(root.join(".k2")).unwrap();
        fs::write(root.join(".k2/keep"), b"precious").unwrap();

        let r = migrate_workspace_dot_dir(&root);
        assert!(!r.renamed, "must not rename over an existing .k2/");
        assert!(root.join(".k2so").exists(), ".k2so/ left in place");
        assert_eq!(
            fs::read_to_string(root.join(".k2/keep")).unwrap(),
            "precious",
            ".k2/ untouched"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rewrites_only_dot_dir_gitignore_rules() {
        let root = tmp("gitignore");
        fs::create_dir_all(root.join(".k2so")).unwrap();
        // A realistic .gitignore: dot-dir rules (must retarget) mixed with
        // skill-name / crate refs (must be untouched).
        let gi = "\
# K2 workspace
.k2so/inbox/
.k2so/sessions/
/.k2so/logs/
.k2so/.archive/
.k2so
.cursor/rules/k2so.mdc
.opencode/agent/k2so*.md
.pi/skills/k2so/
crates/k2so-core/drizzle_sql/meta/
node_modules/
";
        fs::write(root.join(".gitignore"), gi).unwrap();

        let r = migrate_workspace_dot_dir(&root);
        assert!(r.renamed);
        assert!(r.gitignore_rewritten, "gitignore should be rewritten");

        let after = fs::read_to_string(root.join(".gitignore")).unwrap();
        // ADDITIVE: a `.k2/` twin is added for every `.k2so/` dot-dir rule,
        // and the original `.k2so/` rule is KEPT.
        assert!(after.contains(".k2/inbox/"), "k2 inbox twin added");
        assert!(after.contains(".k2so/inbox/"), "k2so inbox rule kept");
        assert!(after.contains(".k2/sessions/") && after.contains(".k2so/sessions/"));
        assert!(after.contains("/.k2/logs/") && after.contains("/.k2so/logs/"));
        assert!(after.contains(".k2/.archive/") && after.contains(".k2so/.archive/"));
        // The bare `.k2so` line is kept and a bare `.k2` twin added.
        assert!(after.lines().any(|l| l == ".k2so"), "bare .k2so kept");
        assert!(after.lines().any(|l| l == ".k2"), "bare .k2 twin added");
        // Skill-name / crate refs LEFT ALONE (no twins, no edits).
        assert!(after.contains(".cursor/rules/k2so.mdc"), "skill mdc untouched");
        assert!(after.contains(".opencode/agent/k2so*.md"), "opencode untouched");
        assert!(after.contains(".pi/skills/k2so/"), "pi skill untouched");
        assert!(!after.contains(".pi/skills/k2/"), "no spurious twin for skill ref");
        assert!(
            after.contains("crates/k2so-core/drizzle_sql/meta/"),
            "crate path untouched"
        );
        // Running again is a no-op (twins already present).
        assert!(
            !rewrite_gitignore_dot_dir(&root),
            "idempotent — no new twins on second pass"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repoints_root_and_subdir_symlinks() {
        let root = tmp("symlinks");
        let canonical = root.join(".k2so/skills/k2so/SKILL.md");
        fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        fs::write(&canonical, b"# skill").unwrap();

        // Root-level fan-out symlinks → absolute canonical (as force_symlink writes).
        symlink(&canonical, &root.join("CLAUDE.md"));
        symlink(&canonical, &root.join("SKILL.md"));
        // Subdir fan-out symlinks.
        symlink(&canonical, &root.join(".claude/skills/k2so/SKILL.md"));
        symlink(&canonical, &root.join(".opencode/agent/k2so.md"));
        symlink(
            &root.join(".k2so/skills/k2so/SKILL.md"),
            &root.join(".pi/skills/k2so/SKILL.md"),
        );
        // A non-K2SO symlink that must be LEFT ALONE.
        fs::write(root.join("README.md"), b"readme").unwrap();
        symlink(&root.join("README.md"), &root.join("README.link"));

        let r = migrate_workspace_dot_dir(&root);
        assert!(r.renamed);
        assert_eq!(r.symlinks_repointed, 5, "all 5 fan-out symlinks re-pointed");

        // Every fan-out symlink now resolves under .k2/ and is NOT dangling.
        for link in [
            "CLAUDE.md",
            "SKILL.md",
            ".claude/skills/k2so/SKILL.md",
            ".opencode/agent/k2so.md",
            ".pi/skills/k2so/SKILL.md",
        ] {
            let p = root.join(link);
            let target = fs::read_link(&p).unwrap();
            assert!(
                target.starts_with(root.join(".k2")),
                "{link} should target .k2/, got {}",
                target.display()
            );
            assert!(p.exists(), "{link} must resolve (not dangling)");
        }

        // The unrelated symlink is untouched.
        assert_eq!(
            fs::read_link(root.join("README.link")).unwrap(),
            root.join("README.md")
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hygiene_sweeps_orphan_tempfiles_keeps_real_files() {
        let root = tmp("sweep");
        let dot = root.join(".k2");
        fs::create_dir_all(dot.join("skills/qa")).unwrap();
        // Orphaned atomic-write temps (current `.k2-tmp.` + legacy `.k2so-tmp.`).
        fs::write(dot.join("..last-skill-regen.k2-tmp.1.2.3"), b"x").unwrap();
        fs::write(dot.join("..regen-in-flight.k2so-tmp.4.5.6"), b"x").unwrap();
        fs::write(dot.join("skills/qa/.SKILL.md.k2so-tmp.7.8.9"), b"x").unwrap();
        // Real files that MUST survive the sweep.
        fs::write(dot.join(".last-skill-regen"), b"marker").unwrap();
        fs::write(dot.join("PROJECT.md"), b"ctx").unwrap();

        let swept = ensure_dot_dir_hygiene(&root);
        assert_eq!(swept, 3, "exactly the 3 orphan temps removed");
        assert!(!dot.join("..last-skill-regen.k2-tmp.1.2.3").exists());
        assert!(!dot.join("..regen-in-flight.k2so-tmp.4.5.6").exists());
        assert!(!dot.join("skills/qa/.SKILL.md.k2so-tmp.7.8.9").exists());
        assert!(dot.join(".last-skill-regen").exists(), "real marker kept");
        assert!(dot.join("PROJECT.md").exists(), "real file kept");
        fs::remove_dir_all(&root).ok();
    }
}
