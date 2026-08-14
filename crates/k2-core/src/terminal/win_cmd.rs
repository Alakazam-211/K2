//! Windows bare-name spawn for ConPTY / CreateProcess.
//!
//! CreateProcess does **not** apply `PATHEXT`. PowerShell/`cmd` do.
//! npm global CLIs (`claude`, `codex`, `gemini`) are `claude.cmd` in
//! `%APPDATA%\npm`. Launch-bar spawn of `"claude"` therefore ENOENTs
//! while `claude --dangerously-skip-permissions` in PowerShell works.
//! Native `grok.exe` already resolves — that is why Grok works.

use std::path::{Path, PathBuf};

/// Rewrite `(program, args)` so CreateProcess can start npm `.cmd`
/// shims. No-op on Unix, and when `program` is already an `.exe`.
pub fn resolve_spawn(program: &str, args: &[String], path: &str) -> (String, Vec<String>) {
    #[cfg(not(windows))]
    {
        let _ = path;
        (program.to_string(), args.to_vec())
    }
    #[cfg(windows)]
    {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        resolve_for_pty(program, args, path, &pathext, &comspec, |p| p.is_file())
    }
}

/// Testable core. `exists` is injected so macOS unit tests can fake a
/// Windows npm layout.
pub fn resolve_for_pty(
    program: &str,
    args: &[String],
    path: &str,
    pathext: &str,
    comspec: &str,
    exists: impl Fn(&Path) -> bool,
) -> (String, Vec<String>) {
    let dirs = split_search_dirs(path);
    let Some(found) = find_on_path(program, &dirs, pathext, &exists) else {
        return (program.to_string(), args.to_vec());
    };
    if is_batch(&found) {
        return (comspec.to_string(), cmd_c_args(&found, args));
    }
    (found.to_string_lossy().into_owned(), args.to_vec())
}

fn is_batch(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("cmd") | Some("bat") => true,
        _ => false,
    }
}

/// `cmd /d /s /c "path" arg…` — `/d` skips AutoRun; `/s` keeps quotes.
fn cmd_c_args(batch: &Path, args: &[String]) -> Vec<String> {
    let mut payload = format!("\"{}\"", batch.display());
    for a in args {
        payload.push(' ');
        if needs_cmd_quotes(a) {
            payload.push('"');
            payload.push_str(&a.replace('"', "\\\""));
            payload.push('"');
        } else {
            payload.push_str(a);
        }
    }
    vec!["/d".into(), "/s".into(), "/c".into(), payload]
}

fn needs_cmd_quotes(s: &str) -> bool {
    s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '"')
}

/// Split a Windows PATH (`;` — and `:` only between entries, not
/// `C:\…`). Used so unit tests on macOS can feed Windows-shaped
/// strings. Live Windows spawn uses `std::env::split_paths` in
/// [`resolve_spawn`].
fn split_search_dirs(path: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        return std::env::split_paths(path).collect();
    }
    #[cfg(not(windows))]
    {
        path.split(';')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }
}

fn find_on_path(
    name: &str,
    dirs: &[PathBuf],
    pathext: &str,
    exists: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let raw = Path::new(name);
    if raw.components().count() > 1 || raw.is_absolute() {
        return if exists(raw) { Some(raw.to_path_buf()) } else { None };
    }

    let has_ext = raw.extension().is_some();
    let exts = parse_pathext(pathext);

    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let exact = dir.join(name);
        if exists(&exact) {
            return Some(exact);
        }
        if has_ext {
            continue;
        }
        for ext in &exts {
            // PATHEXT is usually uppercase (.EXE); npm files are
            // lowercase (.cmd). Real Win32 is_file is case-insensitive;
            // try both so tests and case-sensitive mounts work.
            let lower = ext.to_ascii_lowercase();
            for e in [ext.as_str(), lower.as_str()] {
                let cand = dir.join(format!("{name}{e}"));
                if exists(&cand) {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// PATHEXT entries we will try. Skip `.PS1` — CreateProcess cannot
/// run PowerShell scripts; the npm `.cmd` shim is the one we want.
fn parse_pathext(pathext: &str) -> Vec<String> {
    pathext
        .split(';')
        .filter_map(|raw| {
            let e = raw.trim();
            if e.is_empty() {
                return None;
            }
            let e = if e.starts_with('.') {
                e.to_string()
            } else {
                format!(".{e}")
            };
            if e.eq_ignore_ascii_case(".ps1") {
                return None;
            }
            Some(e)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn exists_in(files: &HashSet<PathBuf>) -> impl Fn(&Path) -> bool + '_ {
        move |p| files.contains(p)
    }

    #[test]
    fn grok_exe_stays_direct() {
        let dir = PathBuf::from(r"C:\tools");
        let grok = dir.join("grok.exe");
        let files: HashSet<_> = [grok.clone()].into_iter().collect();
        let path = dir.to_string_lossy().into_owned();
        let (prog, args) = resolve_for_pty(
            "grok",
            &["--always-approve".into()],
            &path,
            ".COM;.EXE;.BAT;.CMD",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, grok.to_string_lossy());
        assert_eq!(args, vec!["--always-approve"]);
    }

    #[test]
    fn claude_cmd_wraps_comspec() {
        let dir = PathBuf::from(r"C:\Users\x\AppData\Roaming\npm");
        let cmd = dir.join("claude.cmd");
        let files: HashSet<_> = [cmd.clone()].into_iter().collect();
        let path = dir.to_string_lossy().into_owned();
        let (prog, args) = resolve_for_pty(
            "claude",
            &["--dangerously-skip-permissions".into()],
            &path,
            ".COM;.EXE;.BAT;.CMD",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, r"C:\Windows\System32\cmd.exe");
        assert_eq!(args[0], "/d");
        assert_eq!(args[1], "/s");
        assert_eq!(args[2], "/c");
        assert!(args[3].contains("claude.cmd"));
        assert!(args[3].contains("--dangerously-skip-permissions"));
    }

    #[test]
    fn exe_wins_over_cmd_when_both_exist() {
        let dir = PathBuf::from(r"C:\bin");
        let files: HashSet<_> = [dir.join("foo.exe"), dir.join("foo.cmd")]
            .into_iter()
            .collect();
        let (prog, args) = resolve_for_pty(
            "foo",
            &[],
            &dir.to_string_lossy(),
            ".COM;.EXE;.BAT;.CMD",
            r"C:\Windows\System32\cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, dir.join("foo.exe").to_string_lossy());
        assert!(args.is_empty());
    }

    #[test]
    fn missing_name_left_unchanged() {
        let files: HashSet<PathBuf> = HashSet::new();
        let (prog, args) = resolve_for_pty(
            "nope",
            &["-x".into()],
            r"C:\empty",
            ".EXE;.CMD",
            "cmd.exe",
            exists_in(&files),
        );
        assert_eq!(prog, "nope");
        assert_eq!(args, vec!["-x"]);
    }

    #[test]
    fn parse_pathext_skips_ps1() {
        let exts = parse_pathext(".COM;.EXE;.PS1;.CMD");
        assert!(exts.iter().any(|e| e.eq_ignore_ascii_case(".cmd")));
        assert!(!exts.iter().any(|e| e.eq_ignore_ascii_case(".ps1")));
    }
}
