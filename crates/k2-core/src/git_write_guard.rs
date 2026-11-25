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
//! The config is read with gix-config, as git reads it (any bytes, a BOM,
//! quoted subsections), and the guard fails closed: a config file or
//! include it can't read, parse or resolve counts as naming a program, and
//! a repository it can't open (one git may still open) is refused too.
//!
//! The refusal names the keys, never their values, and nothing runs.

use std::path::{Path, PathBuf};

use gix::bstr::{BStr, BString, ByteSlice};

use crate::cli_folder_trust::gate;

/// Includes followed at most this deep (git's own limit is 10).
const MAX_INCLUDE_DEPTH: usize = 10;

/// Does `key` (git's form: `section[.subsection].name`) name a program git
/// runs on its own? `value` tells a boolean `core.fsmonitor` (git's
/// built-in monitor, no program) from a path, and a boolean `pager.<cmd>`
/// from a pager command.
pub fn names_program(key: &str, value: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('.').collect();
    let (section, name, sub) = (parts[0], *parts.last().unwrap_or(&""), parts.len() >= 3);
    match (section, name) {
        ("filter", "clean" | "smudge" | "process") => sub,
        ("merge", "driver") => sub,
        ("diff", "command" | "textconv") => sub,
        ("diff", "external") => !sub,
        ("core", "sshcommand" | "gitproxy" | "askpass" | "editor" | "pager" | "hookspath" | "alternaterefscommand") => !sub,
        ("core", "fsmonitor") => !sub && !is_git_bool(value),
        ("credential", "helper") => true,
        ("gpg", "program" | "defaultkeycommand") => true,
        ("sequence", "editor") => !sub,
        ("interactive", "difffilter") => !sub,
        ("pager", _) => parts.len() == 2 && !is_git_bool(value),
        ("trailer", "command" | "cmd") => sub,
        ("difftool" | "mergetool", "cmd") => sub,
        ("remote", "uploadpack" | "receivepack") => sub,
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

/// The `(section[.subsection].name, value)` entries of one git config file,
/// parsed by gix-config as git parses it: a UTF-8 BOM skipped, any bytes
/// (not only UTF-8), quoted subsections with `]` or escapes in them, keys
/// on the header's own line, values continued over lines. Section and
/// name are lowercased, the subsection kept; a key with no `=` reads
/// `true`. A file gix-config can't parse is an `Err`: the guard counts it
/// as naming a program, the trust scan reports it.
pub(crate) fn config_entries(bytes: &[u8]) -> Result<Vec<(String, BString)>, String> {
    let file = gix::config::File::from_bytes_no_includes(bytes, gix::config::file::Metadata::api(), Default::default())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for section in file.sections() {
        let header = section.header();
        let name = header.name().to_str_lossy().to_ascii_lowercase();
        // git joins a dotted section name and a quoted subsection the same
        // way (`[filter.a "b"]` is driver `a.b`), so the key is built as
        // git builds it and [`names_program`] splits it at the first and
        // last dot.
        let prefix = match header.subsection_name() {
            Some(sub) => format!("{name}.{}", sub.to_str_lossy()),
            None => name,
        };
        let body = section.body();
        let mut seen: Vec<String> = Vec::new();
        for value_name in body.value_names() {
            let lower = value_name.to_ascii_lowercase();
            if seen.contains(&lower) {
                continue;
            }
            seen.push(lower.clone());
            let key = format!("{prefix}.{lower}");
            let values = body.values(&lower);
            if values.is_empty() {
                out.push((key, BString::from("true")));
            } else {
                out.extend(values.into_iter().map(|v| (key.clone(), v)));
            }
        }
    }
    Ok(out)
}

/// An include's path, as git resolves it: `~/` (or `~`) from HOME,
/// relative to the including file's folder. `Err` when K2 can't resolve it
/// the way git would (`~user/`, `%(prefix)/`, an empty or unrepresentable
/// path): such an include counts as naming a program.
fn include_target(from: &Path, value: &BStr) -> Result<PathBuf, ()> {
    if value.is_empty() || value.starts_with(b"%(") {
        return Err(());
    }
    let path = bytes_path(value)?;
    let text = path.to_string_lossy().to_string();
    if text == "~" || text.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or(())?;
        return Ok(PathBuf::from(home).join(text[1..].trim_start_matches('/')));
    }
    if text.starts_with('~') {
        return Err(());
    }
    Ok(if path.is_absolute() { path } else { from.parent().unwrap_or(Path::new(".")).join(path) })
}

#[cfg(unix)]
fn bytes_path(v: &BStr) -> Result<PathBuf, ()> {
    use std::os::unix::ffi::OsStrExt;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(v)))
}

#[cfg(not(unix))]
fn bytes_path(v: &BStr) -> Result<PathBuf, ()> {
    v.to_str().map(PathBuf::from).map_err(|_| ())
}

/// Walk one config file and every file it includes (each include followed
/// whatever its `includeIf` condition: a superset of what git reads), and
/// push program keys into `out`. Fails closed: a file that exists but
/// can't be read or parsed, an include K2 can't resolve, and one past
/// git's depth each push an entry, so the guard refuses. A missing file is
/// skipped, as git skips it.
fn read_file(path: &Path, label: &str, depth: usize, out: &mut Vec<String>) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            out.push(format!("{label} (can't be read)"));
            return;
        }
    };
    let Ok(entries) = config_entries(&bytes) else {
        out.push(format!("{label} (can't be parsed)"));
        return;
    };
    for (key, value) in entries {
        if names_program(&key, &value.to_str_lossy()) {
            out.push(key);
        } else if is_include(&key) {
            match include_target(path, value.as_bstr()) {
                Err(()) => out.push(format!("{key} (can't be followed)")),
                Ok(_) if depth >= MAX_INCLUDE_DEPTH => out.push(format!("{key} (too deep)")),
                Ok(t) => read_file(&t, &key, depth + 1, out),
            }
        }
    }
}

/// The program-naming keys in `repo`'s own config: `<common dir>/config`,
/// the git dir's own `config` (a linked worktree's), `config.worktree` in
/// either (read whether or not `extensions.worktreeConfig` is on), and
/// every file they include. Sorted, once each. Never global or system
/// config. A file that can't be read or parsed is listed too (fail closed).
pub fn program_keys(repo: &gix::Repository) -> Vec<String> {
    let common = repo.common_dir().to_path_buf();
    let git_dir = repo.git_dir().to_path_buf();
    let mut files: Vec<PathBuf> = Vec::new();
    for f in [common.join("config"), git_dir.join("config"), git_dir.join("config.worktree"), common.join("config.worktree")] {
        if !files.contains(&f) {
            files.push(f);
        }
    }
    let mut out = Vec::new();
    for f in &files {
        let label = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "config".into());
        read_file(f, &label, 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// What the guard found at a folder.
enum Found {
    /// No repository at or above the folder: nothing to check.
    NotARepo,
    Repo(gix::Repository),
    /// A repository is there, but K2 can't open it the way git will (gix
    /// refuses it, or git would take a nearer `.git` than gix did). The
    /// folder is the one holding the nearest `.git`, else the one asked.
    Unopenable(PathBuf),
}

/// The folder at or above `dir` that holds a `.git` entry (a directory, a
/// worktree's `.git` file, or anything else by that name), canonical.
fn nearest_dot_git(dir: &Path) -> Option<PathBuf> {
    let start = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    start.ancestors().find(|a| std::fs::symlink_metadata(a.join(".git")).is_ok()).map(Path::to_path_buf)
}

/// Tell "not a repository" (nothing to check) from "a repository K2 can't
/// open" (refused, see [`check`]).
fn find(dir: &Path) -> Found {
    let nearest = nearest_dot_git(dir);
    match crate::gitx::discover(dir) {
        Ok(repo) => {
            // git takes the nearest `.git`; gix opening one further up (or
            // a bare repo) means it skipped the one git will use.
            if let Some(n) = &nearest {
                let workdir = repo.workdir().and_then(|w| dunce::canonicalize(w).ok());
                if workdir.as_ref() != Some(n) {
                    return Found::Unopenable(n.clone());
                }
            }
            Found::Repo(repo)
        }
        Err(_) => match nearest {
            None if !crate::gitx::repository_above(dir) => Found::NotARepo,
            Some(n) => Found::Unopenable(n),
            None => Found::Unopenable(dir.to_path_buf()),
        },
    }
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
        log_debug!("[git] couldn't record a repository made by {}: {e}", crate::brand::PRODUCT_NAME);
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
/// workspace. A folder that isn't a repository has nothing to check; a
/// repository K2 can't open (git may still open it and run what it names)
/// is refused until the workspace is trusted, like one whose config names
/// a program or can't be read.
pub fn check(dir: &Path) -> Result<(), String> {
    let (workdir, what) = match find(dir) {
        Found::NotARepo => return Ok(()),
        Found::Unopenable(folder) => (folder, format!("{} can't open this repository to read them", crate::brand::PRODUCT_NAME)),
        Found::Repo(repo) => {
            let keys = program_keys(&repo);
            if keys.is_empty() || is_k2_made(&repo) {
                return Ok(());
            }
            let workdir = repo.workdir().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
            (workdir, keys.join(", "))
        }
    };
    if is_trusted(&workdir) {
        return Ok(());
    }
    let folder = workdir.to_string_lossy().to_string();
    Err(format!(
        "{}: this repository's own git settings may run programs ({what}), and this workspace isn't trusted, so {} won't run git here. \
         Trust the workspace first: on its trust card, or {}",
        crate::git::refusal::GIT_CONFIG_RUNS_PROGRAMS,
        crate::brand::PRODUCT_NAME,
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
            "gpg.ssh.defaultKeyCommand", "core.alternateRefsCommand", "interactive.diffFilter",
            "pager.log", "trailer.sign.command", "trailer.sign.cmd", "difftool.x.cmd", "mergetool.x.cmd",
            "remote.origin.uploadpack", "remote.origin.receivepack", "filter.a]b.clean", "filter.a.b.clean",
        ] {
            assert!(names_program(k, "/some/program"), "{k}");
        }
        assert!(names_program("core.fsmonitor", "/some/hook"));
        for (k, v) in [
            ("core.fsmonitor", "true"),
            ("core.fsmonitor", "false"),
            ("pager.log", "false"),
            ("filter.lfs.required", "true"),
            ("merge.ff", "only"),
            ("diff.renames", "true"),
            ("user.name", "x"),
            ("core.autocrlf", "true"),
            ("remote.origin.url", "https://example.com/x.git"),
        ] {
            assert!(!names_program(k, v), "{k} = {v}");
        }
    }

    fn entries(bytes: &[u8]) -> Vec<(String, String)> {
        super::config_entries(bytes).expect("parses").into_iter().map(|(k, v)| (k, v.to_string())).collect()
    }

    /// The config is read as git reads it, where a line-by-line text reader
    /// went wrong: a BOM, a non-UTF-8 byte, a `]` in a quoted subsection
    /// with the key on the header's line, a dotted section name with a
    /// quoted subsection, a key with no value, a value continued over
    /// lines, an escaped quote in a subsection.
    #[test]
    fn config_entries_read_as_git_reads() {
        let kv = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(entries(b"\xef\xbb\xbf[filter \"x\"]\n\tclean = c\n"), vec![kv("filter.x.clean", "c")]);
        assert_eq!(entries(b"[core]\n# caf\xe9\n[filter \"x\"]\n\tclean = c\n"), vec![kv("filter.x.clean", "c")]);
        assert_eq!(entries(b"[filter \"a]b\"] clean = c\n"), vec![kv("filter.a]b.clean", "c")]);
        assert_eq!(entries(b"[Filter.A \"b\"]\n\tCLEAN = c\n"), vec![kv("filter.a.b.clean", "c")]);
        assert_eq!(entries(b"[core]\n\tfsmonitor\n"), vec![kv("core.fsmonitor", "true")]);
        let continued = entries(b"[core]\n\tsshCommand = a \\\n b\n");
        assert_eq!(continued.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec!["core.sshcommand"], "{continued:?}");
        let escaped = entries(b"[filter \"a\\\"]b\"]\n\tclean = c\n");
        assert_eq!(escaped.len(), 1, "{escaped:?}");
        assert!(names_program(&escaped[0].0, &escaped[0].1), "{escaped:?}");
        assert_eq!(
            entries(b"[filter \"x\"]\n\tclean = one\n\tclean = two\n"),
            vec![kv("filter.x.clean", "one"), kv("filter.x.clean", "two")]
        );
        assert!(super::config_entries(b"[filter \"x\"\n\tclean = c\n").is_err(), "an unclosed header doesn't parse");
    }

    /// Fail closed: a config file or include that can't be read, parsed or
    /// resolved is listed as a program key; a missing include is skipped
    /// (git skips it too); every include is followed whatever its
    /// `includeIf` condition.
    #[test]
    fn program_keys_fail_closed() {
        let d = fx::init("guard-keys");
        let cfg = d.join(".git/config");
        let base = std::fs::read(&cfg).expect("config");
        let with = |extra: &[u8]| {
            let mut b = base.clone();
            b.extend_from_slice(extra);
            std::fs::write(&cfg, b).expect("write config");
            super::program_keys(&crate::gitx::discover(&d).expect("repo"))
        };
        assert!(with(b"").is_empty(), "a fresh repo names no program");
        assert!(with(b"[include]\n\tpath = missing.cfg\n").is_empty(), "a missing include is skipped");
        std::fs::create_dir_all(d.join(".git/adir")).expect("dir");
        assert_eq!(with(b"[include]\n\tpath = adir\n"), vec!["include.path (can't be read)"]);
        std::fs::write(d.join(".git/bad.cfg"), b"[x\n").expect("bad");
        assert_eq!(with(b"[include]\n\tpath = bad.cfg\n"), vec!["include.path (can't be parsed)"]);
        assert_eq!(with(b"[include]\n\tpath = ~nobody/x.cfg\n"), vec!["include.path (can't be followed)"]);
        assert_eq!(with(b"[include]\n\tpath = %(prefix)/x.cfg\n"), vec!["include.path (can't be followed)"]);
        std::fs::write(d.join(".git/f.cfg"), b"[filter \"x\"]\n\tsmudge = s\n").expect("f");
        assert_eq!(with(b"[includeIf \"onbranch:never\"]\n\tpath = f.cfg\n"), vec!["filter.x.smudge"]);
        std::fs::write(d.join(".git/loop.cfg"), b"[include]\n\tpath = loop.cfg\n").expect("loop");
        assert_eq!(with(b"[include]\n\tpath = loop.cfg\n"), vec!["include.path (too deep)"]);
        std::fs::write(&cfg, &base).expect("restore");
        std::fs::write(d.join(".git/config.worktree"), b"[core]\n\tfsmonitor = ./w\n").expect("worktree config");
        assert_eq!(super::program_keys(&crate::gitx::discover(&d).expect("repo")), vec!["core.fsmonitor"]);
    }
}
