//! `k2 terminal spawn --command "<command line>"` (0.46.18).
//!
//! `/cli/terminal/spawn` takes its command as one string. Before 0.46.18 the
//! whole string became the program, so `--command "/bin/echo hi"` failed
//! with `ptyd spawn: enoent` (or ENOENT in-process): only a one-word command
//! ever worked, although the help invites a full command line.
//!
//! [`spawn_parts`] decides how that string starts:
//! - one word (a bare name or a path, or `$SHELL`) starts exactly as before:
//!   it is the program, with no arguments;
//! - a command line (whitespace or shell syntax: pipes, quotes, `;`, `$`…)
//!   runs as `<shell> -lc '<command line>'`, the shell being this user's
//!   login shell from passwd, or `/bin/sh` when that is `nologin`, `false`
//!   or missing ([`pick_shell`]).
//!
//! The JSON `command` + `args` of `/cli/sessions/v2/spawn` never come here.
//! Windows keeps the one-string behaviour (no passwd; cmd.exe quoting is
//! `win_cmd`'s business).

use std::path::Path;

/// Characters that make a `--command` value a shell command line.
const SHELL_SYNTAX: &[char] = &['|', '&', ';', '<', '>', '(', ')', '$', '`', '"', '\'', '\\', '*', '?'];

/// The shell used when the passwd shell can't run a command line.
pub const FALLBACK_SHELL: &str = "/bin/sh";

/// True when `cmd` is a command line for a shell, not one program: it holds
/// whitespace or shell syntax. A single word (`claude`, `/bin/echo`,
/// `./run.sh`), `$SHELL`, and an existing file named whole by its absolute
/// path (a path with a space in it) are one program.
pub fn is_command_line(cmd: &str) -> bool {
    let c = cmd.trim();
    if c.is_empty() || c == "$SHELL" {
        return false;
    }
    if !c.chars().any(|ch| ch.is_whitespace() || SHELL_SYNTAX.contains(&ch)) {
        return false;
    }
    let p = Path::new(c);
    !(p.is_absolute() && p.is_file())
}

/// The shell that runs a command line, from the passwd entry's shell:
/// that shell when it is an absolute path to an executable file and not a
/// "no login" shell (`nologin`, `false`), else [`FALLBACK_SHELL`].
pub fn pick_shell(passwd_shell: Option<&str>) -> String {
    match passwd_shell.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if usable_shell(s) => s.to_string(),
        _ => FALLBACK_SHELL.to_string(),
    }
}

fn usable_shell(shell: &str) -> bool {
    let p = Path::new(shell);
    if !p.is_absolute() {
        return false;
    }
    let base = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if matches!(base, "nologin" | "false" | "true") {
        return false;
    }
    is_executable_file(p)
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// This user's shell for a command line ([`pick_shell`] over passwd).
pub fn login_shell() -> String {
    #[cfg(unix)]
    {
        pick_shell(crate::terminal::ptyd::env::passwd_shell().as_deref())
    }
    #[cfg(not(unix))]
    {
        pick_shell(None)
    }
}

/// `(program, args)` that run the command line `cmd` in `shell`:
/// `shell -lc cmd` (csh/tcsh take `-l` only alone, so they get `-c`).
pub fn wrap_in(shell: &str, cmd: &str) -> (String, Vec<String>) {
    let base = Path::new(shell).file_name().and_then(|n| n.to_str()).unwrap_or(shell);
    let flag = if matches!(base, "csh" | "tcsh") { "-c" } else { "-lc" };
    (shell.to_string(), vec![flag.to_string(), cmd.trim().to_string()])
}

/// How a terminal spawn's `command` starts: `None` when it is one program
/// (it starts as given, as before 0.46.18), else the login shell and its
/// `-lc <command line>` args. Unix only; Windows always `None`.
pub fn spawn_parts(cmd: &str) -> Option<(String, Vec<String>)> {
    if !cfg!(unix) || !is_command_line(cmd) {
        return None;
    }
    Some(wrap_in(&login_shell(), cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_word_is_one_program() {
        for one in ["claude", "/bin/echo", "./run.sh", "zsh", "$SHELL", " cat ", "~/bin/tool", "k2-agent_1.2"] {
            assert!(!is_command_line(one), "{one:?} is one program");
            assert_eq!(spawn_parts(one), None, "{one:?} starts as given");
        }
        assert!(!is_command_line(""));
    }

    #[test]
    fn whitespace_and_shell_syntax_are_a_command_line() {
        for line in [
            "/bin/echo hi",
            "echo a | wc -c",
            "printf '%s\\n' \"a b\"",
            "ls;pwd",
            "a&&b",
            "echo $HOME",
            "cat <in >out",
            "claude --foo",
            "echo `id`",
            "ls *.rs",
        ] {
            assert!(is_command_line(line), "{line:?} is a command line");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_absolute_path_with_a_space_is_one_program() {
        let dir = std::env::temp_dir().join(format!("k2-cmdline-space-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("my tool");
        std::fs::write(&tool, "#!/bin/sh\necho ok\n").unwrap();
        let s = tool.to_string_lossy().into_owned();
        assert!(tool.is_absolute() && tool.is_file(), "{s:?}");
        assert!(!is_command_line(&s), "{s:?} names an existing file");
        assert!(is_command_line(&format!("{s} --flag")), "with an argument it is a command line");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_command_line_runs_in_the_login_shell_with_lc() {
        let (prog, args) = spawn_parts("/bin/echo hi | wc -c").expect("a command line is wrapped");
        assert_eq!(prog, login_shell());
        assert!(Path::new(&prog).is_absolute(), "{prog}");
        assert_eq!(args, vec!["-lc".to_string(), "/bin/echo hi | wc -c".to_string()]);
    }

    #[test]
    fn wrap_in_uses_lc_except_for_csh() {
        assert_eq!(wrap_in("/bin/zsh", " a b "), ("/bin/zsh".to_string(), vec!["-lc".to_string(), "a b".to_string()]));
        assert_eq!(wrap_in("/bin/sh", "a | b").1, vec!["-lc".to_string(), "a | b".to_string()]);
        assert_eq!(wrap_in("/bin/tcsh", "a b").1, vec!["-c".to_string(), "a b".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_nologin_false_or_missing_passwd_shell_falls_back_to_bin_sh() {
        for no in [
            Some("/usr/sbin/nologin"),
            Some("/sbin/nologin"),
            Some("/usr/bin/false"),
            Some("/bin/false"),
            Some("/nonexistent/k2-no-such-shell"),
            Some("zsh"),
            Some(""),
            Some("   "),
            None,
        ] {
            assert_eq!(pick_shell(no), FALLBACK_SHELL, "{no:?}");
        }
        assert_eq!(pick_shell(Some("/bin/sh")), "/bin/sh");
        // A real login shell is kept as is.
        let real = ["/bin/bash", "/bin/zsh", "/usr/bin/bash"].into_iter().find(|p| Path::new(p).is_file()).expect("a bash or zsh");
        assert_eq!(pick_shell(Some(real)), real);
    }
}
