//! `k5 db run [--db X] [--test [--actor migrator]] [--as NAME] -- <cmd…>`,
//! as `cmd_db run` does it (`cli/src/db-store.sh`): `_db_py`'s `dsn
//! --json --reveal` call, the answer's `dsn`, then the command in this
//! process's place (`exec "$@"`) with the DSN in its environment as
//! `NAME` (`DATABASE_URL` by default).
//!
//! The DSN in the child's environment is the feature (K2's ruling). It is
//! never on argv (ours or the child's), never printed, never in a log
//! line.
//!
//! The binary hands the line to the shell, before it sends anything,
//! when it can't tell which program bash's `exec` would run: a name not
//! found as an executable file on `PATH` (or, with a `/`, not one at
//! that path), a file that is neither a binary nor a `#!` script (bash
//! runs those itself), or a startup setting bash reads (`BASH_ENV` and
//! the others).
//!
//! Where this differs from the shell (named, for K2): the child's
//! environment is the one this process was given plus `NAME`; bash's
//! child also gets bash's own changes (`SHLVL` one higher, `_` set to the
//! program, and `_K2_PY_PROXY_RULE`, the shell CLI's Python proxy rule).

use super::py::{self, Ask, Verb};
use super::{db_value, usage, Form, Words};
use crate::exit::Exit;
use crate::families::{py_get, Raise};
use crate::out::{self, pylib};
use crate::prelude::Closing;
use crate::transport::urllib::Opener;
use crate::transport::Client;
use k2_client::cli_name;
use std::path::{Path, PathBuf};

const UNEXPECTED: &str = concat!(
    cli_name!(),
    " db run [--db <name>] [--test [--actor migrator]] [--as NAME] -- <command…> (unexpected '"
);
const NO_COMMAND: &str = concat!(
    cli_name!(),
    " db run … -- <command…>: name the command after --"
);

/// `run`'s words, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub test: bool,
    pub actor: String,
    pub db: String,
    /// `--as`: the environment name the child reads the DSN from.
    pub env_as: String,
    /// The command after `--`, as typed.
    pub cmd: Vec<String>,
}

/// `run`'s loop and checks, on its words after the verb.
pub fn parse(a: &mut Words) -> Result<Form, Form> {
    let mut r = Run {
        test: false,
        actor: String::new(),
        db: String::new(),
        env_as: "DATABASE_URL".to_string(),
        cmd: Vec::new(),
    };
    while let Some(w) = a.first() {
        match w {
            "--" => {
                a.shift(1);
                break;
            }
            "--test" => {
                r.test = true;
                a.shift(1);
            }
            "--actor" => r.actor = a.value()?.to_string(),
            "--db" => r.db = db_value(a)?.to_string(),
            "--as" => r.env_as = a.value()?.to_string(),
            _ if w.starts_with("--db=") => {
                r.db = w["--db=".len()..].to_string();
                a.shift(1);
            }
            _ => return Err(usage(format!("{UNEXPECTED}{w}')"))),
        }
    }
    if a.is_empty() {
        return Err(usage(NO_COMMAND));
    }
    if !env_name(&r.env_as) {
        return Err(usage("--as takes an env name like DATABASE_URL"));
    }
    if !r.actor.is_empty() && r.actor != "migrator" {
        return Err(usage("--actor must be migrator"));
    }
    r.cmd = a.w.iter().map(|w| w.to_string()).collect();
    Ok(Form::Run(Box::new(r)))
}

/// `[[ "$env_as" =~ ^[A-Z_][A-Z0-9_]*$ ]]`.
fn env_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_uppercase() || b[0] == b'_')
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

/// The program `exec "$@"` runs for `name`, or `None` when the binary
/// can't tell (the shell answers then).
pub fn program(name: &str) -> Option<PathBuf> {
    if crate::offline::STARTUP
        .iter()
        .any(|n| std::env::var_os(n).is_some())
    {
        return None;
    }
    let path = if name.contains('/') {
        let p = PathBuf::from(name);
        crate::daemon::is_executable(&p).then_some(p)?
    } else {
        crate::daemon::find_on_path(name, std::env::var_os("PATH")?)?
    };
    runs_itself(&path).then_some(path)
}

/// A file `execve` runs: a binary (ELF or Mach-O) or a `#!` script.
fn runs_itself(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let Ok(n) = f.read(&mut head) else {
        return false;
    };
    let head = &head[..n];
    head.starts_with(b"#!")
        || head == b"\x7fELF"
        || matches!(
            head,
            [0xfe, 0xed, 0xfa, 0xce | 0xcf]
                | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
        )
}

/// `_db_py`'s `dsn --json --reveal` (its stdout held), the answer's `dsn`,
/// then the command with it in its environment. Returns only when the
/// command didn't start: the exit.
pub fn run(client: &mut Client, closing: Closing, project: &str, r: &Run, netloc: String) -> Exit {
    let mut ask = Ask::new(Verb::Dsn);
    ask.json = true;
    ask.test = r.test;
    ask.actor = r.actor.clone();
    ask.db = r.db.clone();
    ask.reveal = true;
    let mut opener = Opener::from_client(client);
    opener.netloc = netloc;
    let (shown, note) = py::Py::new(&opener, project, &ask).shown();
    if !shown.stderr.is_empty() {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
    }
    if shown.exit != Exit::Ok {
        // `_run_out="$(…)" || exit $?`: what it printed on stdout is gone.
        match note {
            Some(n) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), n.as_bytes());
            }
            None => closing.flush(client, shown.exit),
        }
        return shown.exit;
    }
    let held = shown.stdout.trim_end_matches('\n');
    let url = match dsn_of(held) {
        Ok(u) => u,
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            closing.flush(client, Exit::Error);
            return Exit::Error;
        }
    };
    if url.is_empty() {
        out::stderr_line(&py::err_line("engine", "the daemon answered no DSN"));
        closing.flush(client, Exit::Error);
        return Exit::Error;
    }
    exec(r, &url)
}

/// `print(json.load(sys.stdin).get("dsn") or "")`, held in `$(…)`.
fn dsn_of(held: &str) -> Result<String, Raise> {
    let v = pylib::json::loads(held).map_err(|f| Raise::at(1, f.last_line()))?;
    let dsn = py_get(&v, "dsn", 1)?
        .filter(|d| crate::out::py::truthy(d))
        .map(crate::out::py::str_of)
        .unwrap_or_default();
    Ok(dsn.trim_end_matches('\n').to_string())
}

/// `export NAME=<dsn>; exec "$@"`.
#[cfg(unix)]
fn exec(r: &Run, url: &str) -> Exit {
    use std::os::unix::process::CommandExt;
    let Some(name) = r.cmd.first() else {
        return Exit::Error;
    };
    let Some(path) = program(name) else {
        // It was there when the line was read; bash says this on a miss.
        out::stderr_line(&format!("exec: {name}: not found"));
        return Exit::from_curl(127);
    };
    // Rust starts with SIGPIPE ignored; bash's child starts with it as
    // the default.
    // SAFETY: setting a signal's disposition to its default.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let err = std::process::Command::new(&path)
        .arg0(name)
        .args(&r.cmd[1..])
        .env(&r.env_as, url)
        .exec();
    out::stderr_line(&format!("exec: {name}: {err}"));
    Exit::from_curl(126)
}

#[cfg(not(unix))]
fn exec(_r: &Run, _url: &str) -> Exit {
    Exit::Error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_are_the_shell_s_pattern() {
        for n in ["DATABASE_URL", "_X", "A1"] {
            assert!(env_name(n), "{n}");
        }
        for n in ["", "1A", "a", "A-B", "AÉ"] {
            assert!(!env_name(n), "{n}");
        }
    }

    #[test]
    fn the_dsn_is_python_s_get() {
        assert_eq!(dsn_of(r#"{"dsn":"postgres://x"}"#).unwrap(), "postgres://x");
        assert_eq!(dsn_of(r#"{"dsn":null}"#).unwrap(), "");
        assert_eq!(dsn_of(r#"{"dsn":5}"#).unwrap(), "5");
        assert!(dsn_of("[]").is_err());
        assert!(dsn_of("postgres://x").is_err());
    }
}
