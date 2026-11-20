//! `k5 vault run`, as the shell CLI's `_vault_run` does it
//! (`cli/src/migrate.sh`): read the flags before `--`, ask the daemon
//! where its run helper is (`GET /cli/vault/run-helper`), then the helper
//! in this process's place (`exec`), with only names on its argv.
//!
//! K2's ruling 1b, which this follows:
//!
//! - the passport reaches the helper on fd 3 (`--token-fd 3`), a pipe,
//!   never in its environment: `K2_HOOK_TOKEN`, `K5_HOOK_TOKEN` and
//!   `K2SO_HOOK_TOKEN` are taken out, and the session's socket is
//!   `K2_HOOK_SOCK` (empty when there is none), as bash exports it;
//! - a daemon whose helper doesn't say `"tokenFd": true` is refused
//!   before anything runs (it would take the passport in its
//!   environment, where the command would see it);
//! - the helper clears every K2 credential from the command's
//!   environment and sets `K2_NO_OWNER_FALLBACK=1`: the command gets its
//!   secrets, not the caller's identity.
//!
//! A line with `-h` or `--help` anywhere is the front's (a page, or the
//! shell), so `vault run --secret A -- grep --help`, where the `--help`
//! is the command's, runs in the shell still: the front would have to
//! stop reading at `--` for the binary to run it.
//!
//! Where this differs from the shell (named, for K2): the helper's
//! environment is the one this process was given with the changes above;
//! bash's helper also gets bash's own (`SHLVL` one higher, `_` set to the
//! program, and `_K2_PY_PROXY_RULE`, the shell CLI's Python proxy rule).
//! A helper `execve` can't run (neither a binary nor a `#!` script) is a
//! script bash runs itself; here it is an `exec` error, exit 126. The
//! daemon names its own binary, so neither is met in use.

use super::Ctx;
use crate::exit::Exit;
use crate::out;
use crate::out::json::Value;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::product_name;

/// The daemon named no helper, or one that can't run.
pub const MISSING: &str = concat!(
    "run_helper_missing: the ",
    product_name!(),
    " daemon named no usable helper; restart the ",
    product_name!(),
    " daemon and try again"
);

/// The helper would take the passport in its environment.
pub const TOO_OLD: &str = concat!(
    "run_helper_too_old: this ",
    product_name!(),
    " daemon's run helper takes the passport in its environment, where the command would see it; update the ",
    product_name!(),
    " daemon, then run it again"
);

/// `_vault_run`'s loop and checks, on the words after `run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `--help` or `-h` before `--`: the run page.
    Help,
    /// The lines on stderr, exit 2. Nothing is sent.
    Usage(Vec<String>),
    /// The helper's flags (names only) and the command after `--`.
    Run {
        helper: Vec<String>,
        cmd: Vec<String>,
    },
}

fn usage(line: impl Into<String>) -> Parsed {
    Parsed::Usage(vec![line.into()])
}

/// The words, as bash's `while` reads them.
pub fn parse(rest: &[&str]) -> Parsed {
    let mut helper: Vec<String> = Vec::new();
    let (mut named, mut dashes) = (false, false);
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let next = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--help" | "-h" => return Parsed::Help,
            "--" => {
                dashes = true;
                i += 1;
                break;
            }
            "--secret" | "--env-file" | "--scope" => {
                if next.is_empty() {
                    return usage(match w {
                        "--secret" => "usage: --secret NAME[=ENV_NAME]",
                        "--env-file" => "usage: --env-file <file>",
                        _ => "usage: --scope workspace|server|session",
                    });
                }
                helper.push(w.to_string());
                helper.push(next.to_string());
                named |= w != "--scope";
                i += 2;
            }
            "--no-mask" => {
                helper.push(w.to_string());
                i += 1;
            }
            _ if w == "--value" || w.starts_with("--value=") || w.starts_with("--secret-value") => {
                return usage(format!(
                    "value_on_argv: run takes names, never values ({CLI} vault run --help)"
                ));
            }
            "--system" => return usage(format!("usage: only {CLI} vault list takes --system")),
            _ if w.starts_with("--") => {
                return usage(format!(
                    "usage: unknown flag '{w}' ({CLI} vault run --help)"
                ))
            }
            // NAME like `vault file NAME`; --secret NAME works too.
            _ => {
                helper.push("--secret".to_string());
                helper.push(w.to_string());
                named = true;
                i += 1;
            }
        }
    }
    let cmd: Vec<String> = rest[i.min(rest.len())..]
        .iter()
        .map(|w| w.to_string())
        .collect();
    if !dashes || cmd.is_empty() || !named {
        return Parsed::Usage(vec![
            format!("usage: {CLI} vault run [--secret NAME[=ENV_NAME]]... [--env-file .env.k2] [--scope workspace|server|session] [--no-mask] -- cmd [args…]"),
            format!("Put -- before the command: {CLI} vault run --secret API_KEY -- node app.js"),
        ]);
    }
    Parsed::Run { helper, cmd }
}

/// The verb. Returns only when the helper didn't start: the exit.
pub fn run(ctx: &mut Ctx, rest: &[&str]) -> Result<Exit, Exit> {
    let (helper_args, cmd) = match parse(rest) {
        Parsed::Help => return Ok(super::help_page("vault/run")),
        Parsed::Usage(lines) => {
            for l in lines {
                out::stderr_line(&l);
            }
            return Err(Exit::Usage);
        }
        Parsed::Run { helper, cmd } => (helper, cmd),
    };
    let r = ctx.call(false, "run-helper", "")?;
    // helper="$(… print(json.load(sys.stdin).get("helper") or ""))": the
    // capture fails under `set -e` when Python raises.
    let v = match crate::confirm::load(&r.body) {
        Ok(v) => v,
        Err(e) => return Err(raised(&e)),
    };
    let helper = match super::py::get(&v, "helper") {
        Ok(h) if super::py::t(&h) => super::py::s(&h).trim_end_matches('\n').to_string(),
        Ok(_) => String::new(),
        Err(e) => return Err(raised(&e)),
    };
    let token_fd = matches!(super::py::get(&v, "tokenFd"), Ok(Value::Bool(true)));
    if helper.is_empty() || !x_ok(&helper) {
        out::stderr_line(MISSING);
        return Err(Exit::Error);
    }
    if !token_fd {
        out::stderr_line(TOO_OLD);
        return Err(Exit::Error);
    }
    // `${SCOPED_TOKEN:-${K2_HOOK_TOKEN:-}}`: the session's passport (a
    // healed one after a heal), or none.
    let token = ctx
        .client
        .passport()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let sock = crate::families::hook_sock()
        .map(std::path::PathBuf::into_os_string)
        .unwrap_or_default();
    Err(exec(&helper, &helper_args, &cmd, token.as_bytes(), sock))
}

fn raised(e: &crate::families::Raise) -> Exit {
    out::stderr_line(e.text().trim_end_matches('\n'));
    Exit::Error
}

/// `[ -x "$helper" ]`.
#[cfg(unix)]
fn x_ok(p: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(p) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn x_ok(_p: &str) -> bool {
    false
}

/// The names the passport goes by, taken out of the helper's environment
/// (bash's `unset K2_HOOK_TOKEN K5_HOOK_TOKEN K2SO_HOOK_TOKEN`).
pub const UNSET: &[&str] = &["K2_HOOK_TOKEN", "K5_HOOK_TOKEN", "K2SO_HOOK_TOKEN"];

/// `exec 3< <(printf '%s' "$_vr_tok")`, `export K2_HOOK_SOCK=…`, the
/// `unset`, then `exec "$helper" --vault-run --token-fd 3 <names> -- <cmd>`.
/// Returns only when the helper didn't start.
#[cfg(unix)]
fn exec(
    helper: &str,
    helper_args: &[String],
    cmd: &[String],
    token: &[u8],
    sock: std::ffi::OsString,
) -> Exit {
    use std::os::unix::process::CommandExt;
    let Some(read_end) = token_pipe(token) else {
        out::stderr_line(MISSING);
        return Exit::Error;
    };
    // Rust starts with SIGPIPE ignored; bash's child starts with it as
    // the default.
    // SAFETY: setting a signal's disposition to its default.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let mut c = std::process::Command::new(helper);
    c.arg("--vault-run")
        .args(["--token-fd", "3"])
        .args(helper_args)
        .arg("--")
        .args(cmd)
        .env("K2_HOOK_SOCK", sock);
    for n in UNSET {
        c.env_remove(n);
    }
    // SAFETY: only dup2, close and fcntl, which are async-signal-safe.
    unsafe {
        c.pre_exec(move || {
            if read_end != 3 {
                if libc::dup2(read_end, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(read_end);
            }
            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let err = c.exec();
    out::stderr_line(&format!("exec: {helper}: {err}"));
    Exit::from_curl(if err.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    })
}

/// A pipe holding `token` and then the end: its read end, as bash's
/// process substitution gives it. A child process writes it, so a token
/// of any length can't fill the pipe and stop this one.
#[cfg(unix)]
fn token_pipe(token: &[u8]) -> Option<libc::c_int> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    // SAFETY: a two-int array for pipe(2).
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    let (r, w) = (fds[0], fds[1]);
    // SAFETY: the child only writes, closes and exits (async-signal-safe
    // calls); the parent closes its write end.
    match unsafe { libc::fork() } {
        -1 => {
            unsafe {
                libc::close(r);
                libc::close(w);
            }
            None
        }
        0 => unsafe {
            libc::close(r);
            let mut off = 0;
            while off < token.len() {
                let n = libc::write(w, token[off..].as_ptr().cast(), token.len() - off);
                if n <= 0 {
                    break;
                }
                off += n as usize;
            }
            libc::close(w);
            libc::_exit(0);
        },
        _ => {
            unsafe {
                libc::close(w);
            }
            Some(r)
        }
    }
}

#[cfg(not(unix))]
fn exec(
    _helper: &str,
    _helper_args: &[String],
    _cmd: &[String],
    _token: &[u8],
    _sock: std::ffi::OsString,
) -> Exit {
    Exit::Error
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(h: &[&str], c: &[&str]) -> Parsed {
        Parsed::Run {
            helper: h.iter().map(|w| w.to_string()).collect(),
            cmd: c.iter().map(|w| w.to_string()).collect(),
        }
    }

    #[test]
    fn the_words_are_read_as_bash_reads_them() {
        assert_eq!(
            parse(&[
                "--secret",
                "A",
                "--secret",
                "B=OTHER",
                "--env-file",
                ".env.k2",
                "--scope",
                "server",
                "--no-mask",
                "--",
                "grep",
                "--help",
                "--secret",
                "x"
            ]),
            run(
                &[
                    "--secret",
                    "A",
                    "--secret",
                    "B=OTHER",
                    "--env-file",
                    ".env.k2",
                    "--scope",
                    "server",
                    "--no-mask"
                ],
                &["grep", "--help", "--secret", "x"]
            )
        );
        assert_eq!(
            parse(&["API_KEY", "--", "ls"]),
            run(&["--secret", "API_KEY"], &["ls"])
        );
        // A single dash is a name, as bash's `*)` takes it.
        assert_eq!(
            parse(&["-s", "--", "ls"]),
            run(&["--secret", "-s"], &["ls"])
        );
        // --secret takes the next word whatever it is.
        assert_eq!(
            parse(&["--secret", "-h", "--", "ls"]),
            run(&["--secret", "-h"], &["ls"])
        );
        assert_eq!(parse(&["--secret", "A", "-h"]), Parsed::Help);
        assert_eq!(
            parse(&["--bogus", "-h"]),
            usage(format!(
                "usage: unknown flag '--bogus' ({CLI} vault run --help)"
            ))
        );
    }

    #[test]
    fn what_is_refused() {
        for words in [
            &["--secret", "A"][..],
            &["--secret", "A", "--"][..],
            &["--", "ls"][..],
            &["node", "app.js"][..],
            &[][..],
            &["--scope", "server", "--", "ls"][..],
        ] {
            match parse(words) {
                Parsed::Usage(l) => assert_eq!(l.len(), 2, "{words:?}"),
                other => panic!("{words:?}: {other:?}"),
            }
        }
        assert_eq!(
            parse(&["--secret", ""]),
            usage("usage: --secret NAME[=ENV_NAME]")
        );
        assert_eq!(parse(&["--env-file"]), usage("usage: --env-file <file>"));
        assert_eq!(
            parse(&["--value=hunter2", "--", "ls"]),
            usage(format!(
                "value_on_argv: run takes names, never values ({CLI} vault run --help)"
            ))
        );
        assert_eq!(
            parse(&["--secret-valueX"]),
            usage(format!(
                "value_on_argv: run takes names, never values ({CLI} vault run --help)"
            ))
        );
        assert_eq!(
            parse(&["--system"]),
            usage(format!("usage: only {CLI} vault list takes --system"))
        );
    }
}
