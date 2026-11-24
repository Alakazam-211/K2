//! No program from a repository's own git settings runs through K2's git
//! writes in a workspace nobody trusted (0.46.18, Steward).
//!
//! libgit2 never ran outside programs from config. The person's git does:
//! a `.git/config` (or a file it includes, or `config.worktree`) can name
//! a filter (`clean`, `smudge`, `process`) that runs on stage, checkout and
//! restore, a merge driver that runs on merge, and other programs. K2's
//! writes already turn hooks and fsmonitor off ([`crate::git_safe`]); this
//! guard covers the rest: before any write, [`check`] reads the
//! repository side of the config (never global or system config) and,
//! when it names a program, refuses unless the workspace is folder-trusted
//! ([`crate::cli_folder_trust::gate`]) or K2 made the repository on this
//! machine ([`mark_k2_made`]). A clone from another server is never
//! K2-made: the record lives in this machine's K2 home, not in the repo.
//!
//! The refusal names the keys, never their values, and nothing runs.

use std::path::{Path, PathBuf};

use crate::cli_folder_trust::{gate, scan};

/// Includes followed at most this deep (git's own limit is 10).
const MAX_INCLUDE_DEPTH: usize = 10;

/// Does `key` (git's form: section and name lowercased, subsection kept)
/// name a program git runs on its own? `value` tells a boolean
/// `core.fsmonitor` (git's built-in monitor, no program) from a path.
pub fn names_program(key: &str, value: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('.').collect();
    let (section, name, sub) = (parts[0], *parts.last().unwrap_or(&""), parts.len() >= 3);
    match (section, name) {
        ("filter", "clean" | "smudge" | "process") => sub,
        ("merge", "driver") => sub,
        ("diff", "command" | "textconv") => sub,
        ("diff", "external") => !sub,
        ("core", "sshcommand" | "gitproxy" | "askpass" | "editor" | "pager" | "hookspath") => !sub,
        ("core", "fsmonitor") => !sub && !is_git_bool(value),
        ("credential", "helper") => true,
        ("gpg", "program") => true,
        ("sequence", "editor") => !sub,
        _ => false,
    }
}

fn is_git_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "1" | "0" | "")
}

fn is_include(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower == "include.path" || (lower.starts_with("includeif.") && lower.ends_with(".path"))
}

/// An include's path, as git resolves it: `~/` from HOME, relative to the
/// including file's folder.
fn include_target(from: &Path, value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest));
    }
    let p = PathBuf::from(value);
    Some(if p.is_absolute() { p } else { from.parent().unwrap_or(Path::new(".")).join(p) })
}

/// Walk one config file and its includes; push program keys into `out`.
fn read_file(path: &Path, depth: usize, out: &mut Vec<String>, worktree_config: &mut bool) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for (key, value) in scan::git_config_entries(&text) {
        if key.eq_ignore_ascii_case("extensions.worktreeconfig") && matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1") {
            *worktree_config = true;
        }
        if names_program(&key, &value) {
            out.push(key);
        } else if is_include(&key) {
            match include_target(path, &value) {
                Some(t) if depth < MAX_INCLUDE_DEPTH && t.is_file() => read_file(&t, depth + 1, out, worktree_config),
                // An include K2 can't read (missing, too deep): it may name
                // a program, so it counts as one.
                Some(t) if t.exists() || depth >= MAX_INCLUDE_DEPTH => out.push(key),
                _ => {}
            }
        }
    }
}

/// The program-naming keys in `repo`'s own config: `<common dir>/config`,
/// a linked worktree's own `config` and `config.worktree`, the main
/// `config.worktree` when `extensions.worktreeConfig` is on, and every file
/// they include. Sorted, once each. Never global or system config.
pub fn program_keys(repo: &gix::Repository) -> Vec<String> {
    let common = repo.common_dir().to_path_buf();
    let git_dir = repo.git_dir().to_path_buf();
    let mut out = Vec::new();
    let mut worktree_config = false;
    read_file(&common.join("config"), 0, &mut out, &mut worktree_config);
    if git_dir != common {
        read_file(&git_dir.join("config"), 0, &mut out, &mut worktree_config);
    }
    if worktree_config || git_dir != common {
        read_file(&git_dir.join("config.worktree"), 0, &mut out, &mut worktree_config);
    }
    out.sort();
    out.dedup();
    out
}

// ── K2-made repositories ────────────────────────────────────────────────

fn made_list() -> Option<PathBuf> {
    crate::paths::k2_home_opt().map(|h| h.join("git-made-repos"))
}

fn canonical(p: &Path) -> String {
    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_string()
}

/// Record that K2 made the repository whose git dir holds `workdir` (its
/// `git init`, on this machine). Errors are logged and ignored: a missing
/// record only means the guard asks for trust.
pub fn mark_k2_made(workdir: &Path) {
    let Ok(repo) = crate::gitx::open(workdir) else { return };
    let Some(list) = made_list() else { return };
    // A test never writes into the developer's real K2 home.
    #[cfg(any(test, feature = "test-util"))]
    if !list.starts_with(std::env::temp_dir())
        && !list.starts_with(dunce::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir()))
    {
        return;
    }
    let entry = canonical(repo.common_dir());
    let mut lines: Vec<String> = std::fs::read_to_string(&list).unwrap_or_default().lines().map(str::to_string).collect();
    if lines.contains(&entry) {
        return;
    }
    lines.push(entry);
    if let Some(dir) = list.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = crate::fs_atomic::atomic_write_str(&list, &(lines.join("\n") + "\n")) {
        log_debug!("[git] couldn't record a K2-made repository: {e}");
    }
}

/// Did K2 make this repository on this machine?
pub fn is_k2_made(repo: &gix::Repository) -> bool {
    let Some(list) = made_list() else { return false };
    let entry = canonical(repo.common_dir());
    std::fs::read_to_string(list).unwrap_or_default().lines().any(|l| l == entry)
}

/// Has a person trusted the workspace at `workdir` (folder trust)?
pub fn is_trusted(workdir: &Path) -> bool {
    let path = workdir.to_string_lossy().to_string();
    match gate::check(&path) {
        // Risky, and a person's decision covers it.
        gate::Outcome::Granted { clean: false, .. } => true,
        // The scan saw nothing risky (it reads only `.git/config`): trusted
        // only when a person decided anyway.
        gate::Outcome::Granted { scope, clean: true } => {
            let db = crate::db::shared();
            let conn = db.lock();
            matches!(
                gate::load_row(&conn, &scope).ok().flatten().and_then(|r| r.decision),
                Some(gate::Decision::Once | gate::Decision::Always)
            )
        }
        _ => false,
    }
}

/// `Ok` when K2's git may write in the repository holding `dir`; else
/// `git_config_runs_programs: …` naming the keys and how to trust the
/// workspace. A folder that isn't a repository has nothing to check.
pub fn check(dir: &Path) -> Result<(), String> {
    let Ok(repo) = crate::gitx::discover(dir) else { return Ok(()) };
    let keys = program_keys(&repo);
    if keys.is_empty() || is_k2_made(&repo) {
        return Ok(());
    }
    let workdir = repo.workdir().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
    if is_trusted(&workdir) {
        return Ok(());
    }
    let folder = workdir.to_string_lossy().to_string();
    Err(format!(
        "{}: this repository's own git settings run programs ({}), and this workspace isn't trusted, so K2 won't run git here. \
         Trust the workspace first: on its trust card, or {}",
        crate::git::refusal::GIT_CONFIG_RUNS_PROGRAMS,
        keys.join(", "),
        gate::decide_hint(&folder)
    ))
}

#[cfg(test)]
mod tests {
    use super::names_program;
    use crate::git_fixture as fx;

    /// The K2-made record and the trust lookup compare one path form on
    /// every system: marked through one spelling (a `..` detour, or the
    /// temp dir as given: /var on macOS), found through another (the
    /// canonical one), and never stored as a Windows verbatim `\\?\` path.
    #[test]
    fn made_and_trusted_compare_one_path_form() {
        let _home = crate::test_env::TempHome::new();
        crate::db::init_for_tests();
        let d = fx::init("guard-forms");
        std::fs::create_dir_all(d.join("sub")).expect("sub");
        let detour = d.join("sub").join("..");
        let canonical = dunce::canonicalize(&d).expect("canonical");

        super::mark_k2_made(&detour);
        let list = std::fs::read_to_string(super::made_list().expect("K2 home")).expect("the K2-made record");
        assert_eq!(list.lines().count(), 1, "{list}");
        assert!(!list.contains(r"\\?\"), "no verbatim path: {list}");
        assert!(!list.contains(".."), "resolved: {list}");
        for spelling in [&d, &detour, &canonical] {
            let repo = crate::gitx::discover(spelling).expect("repo");
            assert!(super::is_k2_made(&repo), "found through {}", spelling.display());
        }
        super::mark_k2_made(&canonical);
        let again = std::fs::read_to_string(super::made_list().expect("K2 home")).expect("record");
        assert_eq!(again, list, "the same repository is recorded once, in one form");

        // Trust decided through one spelling holds through the others.
        std::fs::write(d.join(".git/config"), {
            let mut t = std::fs::read_to_string(d.join(".git/config")).expect("config");
            t.push_str("[filter \"x\"]\n\tclean = cat\n");
            t
        })
        .expect("config");
        for spelling in [&d, &detour, &canonical] {
            assert!(!super::is_trusted(spelling), "untrusted through {}", spelling.display());
        }
        crate::cli_folder_trust::gate::decide(&d.to_string_lossy(), crate::cli_folder_trust::gate::Decision::Always, "owner", None)
            .expect("trust");
        for spelling in [&d, &detour, &canonical] {
            assert!(super::is_trusted(spelling), "trusted through {}", spelling.display());
        }
    }

    #[test]
    fn every_program_key_counts_in_any_case() {
        for k in [
            "filter.x.clean", "filter.x.smudge", "filter.x.process", "Filter.x.CLEAN",
            "merge.x.driver", "diff.x.command", "diff.x.textconv", "diff.external",
            "core.sshCommand", "core.gitProxy", "core.askPass", "core.editor", "core.pager", "core.hooksPath",
            "credential.helper", "credential.https://example.com.helper",
            "gpg.program", "gpg.ssh.program", "gpg.x509.program", "sequence.editor",
        ] {
            assert!(names_program(k, "/some/program"), "{k}");
        }
        assert!(names_program("core.fsmonitor", "/some/hook"));
        for (k, v) in [
            ("core.fsmonitor", "true"),
            ("core.fsmonitor", "false"),
            ("filter.lfs.required", "true"),
            ("merge.ff", "only"),
            ("diff.renames", "true"),
            ("user.name", "x"),
            ("core.autocrlf", "true"),
        ] {
            assert!(!names_program(k, v), "{k} = {v}");
        }
    }
}
