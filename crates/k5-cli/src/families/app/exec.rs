//! `app exec <app-id> [--timeout 60|10m] [--json] [--no-stdin] -- <command…>`,
//! as the shell CLI answers it (`cmd_app_exec`, `cli/src/app-doctor.sh`):
//! one command run inside the App's guarded container, as the App.
//!
//! - A pipe or a file on stdin is the command's stdin, base64 in the body
//!   (64 MiB at most: past it, the shell's line and exit 2); a terminal,
//!   nothing, or `--no-stdin` sends none. The body never goes to argv or
//!   the environment; the shell writes it to a temp file, this holds it
//!   (a TMPDIR that can't be written still ends the command first, as
//!   `mktemp -d` ends the shell's).
//! - curl's `--max-time` is the timeout (60 by default) plus 120 with a
//!   stdin, plus 60 without.
//! - The exit is the command's own (0 to 255), 124 when it was stopped at
//!   its timeout, 1 for anything else; a refusal is `_compute_fail`'s.

use super::call::Ctx;
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::{Text, Value};
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const EXEC: &str = concat!(
        "usage: ",
        cli_name!(),
        " app exec <app-id> [--timeout 60] [--no-stdin] -- <command\u{2026}>"
    );
    pub const FLAG_TAIL: &str = " (put the command after --)";
    pub const TOO_BIG: &str = concat!(
        cli_name!(),
        " app exec: stdin is over 64 MiB, the most one exec carries (split it, or move files in with ",
        cli_name!(),
        " app data put)"
    );
    pub const TIMED_OUT: &str =
        concat!("[", cli_name!(), "] the command was stopped at its timeout");
}

/// The most stdin one exec carries.
const CAP: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub app: String,
    pub argv: Vec<String>,
    /// `--timeout` in seconds, or empty.
    pub timeout: String,
    pub json: bool,
    pub no_stdin: bool,
}

/// The words after `exec`. `None`: a help form.
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let (mut app, mut timeout, mut json, mut no_stdin) =
        (String::new(), String::new(), false, false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--timeout" => {
                let v = words.get(i + 1).copied().unwrap_or("");
                match super::db_set::compute_secs(v) {
                    Ok(s) => timeout = s.to_string(),
                    Err(stop) => return Some(Err(stop)),
                }
                if i + 2 > words.len() {
                    return Some(Err(Stop::silent(Exit::Error)));
                }
                i += 2;
            }
            "--json" => (json, i) = (true, i + 1),
            "--no-stdin" => (no_stdin, i) = (true, i + 1),
            "-h" | "--help" => return None,
            "--" => {
                i += 1;
                break;
            }
            _ if w.starts_with('-') => {
                return Some(Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::FLAG_TAIL
                ))))
            }
            _ if app.is_empty() => (app, i) = (w.to_string(), i + 1),
            _ => break,
        }
    }
    let argv: Vec<String> = words[i.min(words.len())..]
        .iter()
        .map(|w| w.to_string())
        .collect();
    if app.is_empty() || argv.is_empty() {
        return Some(Err(Stop::usage(usage::EXEC)));
    }
    Some(Ok(Ask {
        app,
        argv,
        timeout,
        json,
        no_stdin,
    }))
}

/// `[ ! -t 0 ] && { [ -p /dev/stdin ] || [ -f /dev/stdin ]; }`.
fn stdin_is_data() -> bool {
    use std::os::unix::fs::FileTypeExt;
    if crate::prompt::stdin_is_tty() {
        return false;
    }
    std::fs::metadata("/dev/stdin").is_ok_and(|m| m.file_type().is_fifo() || m.is_file())
}

/// `tmpd="$(mktemp -d "${TMPDIR:-/tmp}/k2-exec-XXXXXX")"`: the shell
/// stages the body in a temp folder first, so a TMPDIR it can't write
/// ends the command there (mktemp's line, exit 1) before anything is
/// sent. This holds the body in memory; it makes and removes the same
/// folder so that end is the same.
fn stage() -> Result<(), Exit> {
    let dir = std::env::var_os("TMPDIR")
        .filter(|d| !d.is_empty())
        .map_or_else(|| std::path::PathBuf::from("/tmp"), std::path::PathBuf::from);
    let template = format!("{}/k2-exec-XXXXXX", dir.display());
    let mut err = None;
    for n in 0..16u32 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let tag: String = format!("{:08x}", t ^ (std::process::id() << 8) ^ n)
            .chars()
            .take(6)
            .collect();
        let path = dir.join(format!("k2-exec-{tag}"));
        let made = {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&path)
        };
        match made {
            Ok(()) => {
                let _ = std::fs::remove_dir(&path);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    let why = err.map_or_else(
        || "File exists".to_string(),
        |e| {
            let t = e.to_string();
            t.split(" (os error").next().unwrap_or(&t).to_string()
        },
    );
    if cfg!(target_os = "macos") {
        crate::out::stderr_line(&format!("mktemp: mkdtemp failed on {template}: {why}"));
    } else {
        crate::out::stderr_line(&format!(
            "mktemp: failed to create directory via template '{template}': {why}"
        ));
    }
    Err(Exit::Error)
}

/// Python's `base64.b64encode(data).decode("ascii")`.
fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (k, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if k <= c.len() {
                out.push(char::from(A[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, a: &Ask) -> Answer {
    let s = |t: &str| Value::Str(Text::from(t));
    let mut pairs = vec![
        (Text::from("app"), s(&a.app)),
        (
            Text::from("argv"),
            Value::Array(a.argv.iter().map(|w| s(w)).collect()),
        ),
    ];
    let base: u64 = if a.timeout.is_empty() {
        60
    } else {
        a.timeout.parse().unwrap_or(60)
    };
    let with_stdin = !a.no_stdin && stdin_is_data();
    if with_stdin {
        stage()?;
        let mut data = Vec::new();
        {
            use std::io::Read;
            let _ = std::io::stdin()
                .lock()
                .take(CAP as u64 + 1)
                .read_to_end(&mut data);
        }
        if data.len() > CAP {
            crate::out::stderr_line(usage::TOO_BIG);
            return Err(Exit::Usage);
        }
        pairs.push((Text::from("stdin"), s(&b64(&data))));
    }
    if !a.timeout.is_empty() {
        pairs.push((Text::from("timeout"), Value::Int(a.timeout.clone())));
    }
    let body = crate::confirm::dumps(&Value::Object(pairs));
    let extra = if with_stdin { 120 } else { 60 };
    let sp = ctx.post_max("/cli/app/exec", &body, base + extra)?;
    if !sp.ok() {
        return Err(ctx.fail(&sp));
    }
    let mut o = Out::default();
    if a.json {
        o.stdout.push_str(&raw(&sp.body).stdout);
    } else if let Err(e) = exec_lines(&sp.body, &mut o) {
        return Ok(o.done(Err(e)));
    }
    let r = exit_code(&sp.body);
    Ok(o.done(r))
}

fn exec_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    o.line(&py::s(&py::idx(&v, "framed", 4)?));
    if py::truthy(&py::dget(&v, "timedOut", 5)?) {
        o.err(usage::TIMED_OUT);
    }
    Ok(())
}

/// `print(124 if v.get("timedOut") else (c if isinstance(c, int) and 0
/// <= c <= 255 else 1))`, then `return "${code:-1}"`.
fn exit_code(body: &[u8]) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let c = py::dget(&v, "exitCode", 4)?;
    if py::truthy(&py::dget(&v, "timedOut", 5)?) {
        return Ok(Exit::from_curl(124));
    }
    Ok(match c {
        Value::Int(d) => match d.parse::<i64>() {
            Ok(n @ 0..=255) => Exit::from_curl(n as i32),
            _ => Exit::Error,
        },
        Value::Bool(false) => Exit::Ok,
        Value::Bool(true) => Exit::Error,
        _ => Exit::Error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_as_the_shell_reads_them() {
        let ask = |w: &[&str]| match parse(w) {
            Some(Ok(a)) => a,
            other => panic!("{other:?}"),
        };
        let a = ask(&["acme", "--timeout", "2m", "--", "ls", "-l"]);
        assert_eq!((a.timeout.as_str(), a.argv.len()), ("120", 2));
        let a = ask(&["acme", "ls", "--json"]);
        assert_eq!(a.argv, vec!["ls", "--json"]);
        assert!(!a.json);
        assert!(matches!(parse(&["acme"]), Some(Err(s)) if s.line == usage::EXEC));
        assert!(
            matches!(parse(&["acme", "--zap", "x"]), Some(Err(s)) if s.line.ends_with("after --)"))
        );
        assert!(
            matches!(parse(&["acme", "--timeout", "x", "ls"]), Some(Err(s)) if s.exit == Exit::Usage)
        );
        assert_eq!(parse(&["-h"]), None);
    }

    #[test]
    fn base64_as_python_writes_it() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob\xff"), "Zm9vYv8=");
    }
}
