//! The shell's port check (`_k2_port_ok` and `_k2_port_refuse` in
//! `cli/src/prelude.sh`): every port the command line sends to is a
//! number from 1 to 65535 written in digits only (no sign, no blank, no
//! leading zero).
//!
//! The shell checks the port setting (`PORT` under its K5_, K2_ and
//! K2SO_ names) as its prelude reads it, before anything else runs, and
//! the saved `heartbeat.port` when no setting is there. One that isn't a
//! port stops the command before any request: one line on stderr, exit 2.
//! [`check`] does the same before the front routes a command, so a
//! native verb and a hand-off alike stop there:
//!
//! ```text
//! Error: K2_PORT must be a number from 1 to 65535 (got: <value>)
//! Error: the port in <home>/.k2/heartbeat.port must be a number from 1 to 65535 (got: <value>)
//! ```
//!
//! Where the shell would print something else first, the command goes to
//! the shell ([`Check::Shell`]): bash's own start-up settings (`BASH_ENV`
//! and the others in [`crate::offline::STARTUP`]), Windows, and, for the
//! saved port, anything the prelude stops on or warns about before it
//! reads the file ([`crate::offline::prelude`]: a NUL in a file it reads,
//! a program missing from `PATH`).

use crate::offline::{self, Env};
use k2_client::env_names;
use std::path::Path;

/// `_k2_port_ok`: the port, when `text` is one.
pub fn parse(text: &[u8]) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || text[0] == b'0' {
        return None;
    }
    if !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n: u32 = std::str::from_utf8(text).ok()?.parse().ok()?;
    u16::try_from(n).ok()
}

/// `_k2_port_refuse WHAT VALUE`'s line, newline and all.
pub fn refusal(what: &[u8], value: &[u8]) -> Vec<u8> {
    let mut line = b"Error: ".to_vec();
    line.extend_from_slice(what);
    line.extend_from_slice(b" must be a number from 1 to 65535 (got: ");
    line.extend_from_slice(value);
    line.extend_from_slice(b")\n");
    line
}

/// What [`check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// No port, or a port: go on.
    Go,
    /// Print this on stderr and exit 2; nothing is sent.
    Refuse(Vec<u8>),
    /// The shell answers the command (it prints the refusal its own way).
    Shell,
}

#[cfg(unix)]
fn bytes(s: &std::ffi::OsStr) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes()
}

#[cfg(not(unix))]
fn bytes(s: &std::ffi::OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// bash would run something of its own before the prelude.
fn shell_starts_otherwise(env: &Env) -> bool {
    !cfg!(unix)
        || offline::STARTUP.iter().any(|n| env.get(n).is_some())
        || env.any_named("BASH_FUNC_")
}

/// The shell prelude's port check for `env`, run from `cwd` (the shell's
/// `pwd`, when the front could tell it).
pub fn check(env: &Env, cwd: Option<&Path>) -> Check {
    if let Some((name, value)) = env_names::PORT
        .iter()
        .find_map(|n| env.get(n).filter(|v| !v.is_empty()).map(|v| (*n, v)))
    {
        if parse(bytes(value)).is_some() {
            return Check::Go;
        }
        if shell_starts_otherwise(env) {
            return Check::Shell;
        }
        return Check::Refuse(refusal(name.as_bytes(), bytes(value)));
    }
    let Some(k2_home) = offline::shell_k2_home(env) else {
        return Check::Go;
    };
    let file = k2_home.join("heartbeat.port");
    if !file.is_file() {
        return Check::Go;
    }
    // `$(cat … 2>/dev/null)`: a file that can't be read is the shell's
    // own stop, as before.
    let Ok(mut text) = std::fs::read(&file) else {
        return Check::Go;
    };
    let nul = text.contains(&0);
    text.retain(|b| *b != 0);
    while text.last() == Some(&b'\n') {
        text.pop();
    }
    if text.is_empty() || parse(&text).is_some() {
        return Check::Go;
    }
    let quiet_before = cwd.is_some_and(|cwd| offline::prelude(env, cwd).is_ok());
    if nul || shell_starts_otherwise(env) || !quiet_before {
        return Check::Shell;
    }
    let mut what = b"the port in ".to_vec();
    what.extend_from_slice(bytes(k2_home.as_os_str()));
    what.extend_from_slice(b"/heartbeat.port");
    Check::Refuse(refusal(&what, &text))
}

/// [`check`] for this process.
pub fn check_current() -> Check {
    let env = Env::current();
    let cwd = offline::shell_pwd(&env).ok();
    check(&env, cwd.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_is_digits_from_1_to_65535() {
        for (text, want) in [
            ("1", Some(1)),
            ("80", Some(80)),
            ("65535", Some(65535)),
            ("65536", None),
            ("131072", None),
            ("0", None),
            ("00", None),
            ("0080", None),
            ("", None),
            (" 80", None),
            ("80 ", None),
            ("+80", None),
            ("-1", None),
            ("8x", None),
            ("99999999999999999999", None),
            ("\u{0668}\u{0660}", None),
        ] {
            assert_eq!(parse(text.as_bytes()), want, "{text:?}");
        }
    }

    /// The shell's own words, from its source.
    #[test]
    fn the_rule_and_the_line_are_the_shell_s() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/prelude.sh");
        let prelude = std::fs::read_to_string(p).unwrap();
        for line in [
            "        ''|0*|*[!0123456789]*) return 1 ;;",
            "    [ \"${#1}\" -le 5 ] && [ \"$1\" -le 65535 ]",
            "    printf 'Error: %s must be a number from 1 to 65535 (got: %s)\\n' \"$1\" \"$2\" >&2",
            "    for _k2_port_name in K5 K2 K2SO; do",
            "        _k2_port_name=\"${_k2_port_name}_PORT\"",
            "    _k2_port_refuse \"$_k2_port_name\" \"$PORT\"",
            "    _k2_port_refuse \"the port in ${K2_HOME}/heartbeat.port\" \"$PORT\"",
        ] {
            assert!(prelude.lines().any(|l| l == line), "prelude.sh changed: no line {line:?}");
        }
        assert_eq!(env_names::PORT, ["K5_PORT", "K2_PORT", "K2SO_PORT"]);
        assert_eq!(
            refusal(b"K2_PORT", b"70000"),
            b"Error: K2_PORT must be a number from 1 to 65535 (got: 70000)\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_setting_then_the_saved_port() {
        let dir = std::env::temp_dir().join(format!("k5-port-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".k2")).unwrap();
        let home = dir.to_str().unwrap();
        let path = std::env::var("PATH").unwrap();
        let env = |pairs: &[(&str, &str)]| {
            let mut all = vec![("HOME", home), ("PATH", path.as_str())];
            all.extend_from_slice(pairs);
            Env::of(&all)
        };
        let cwd = Some(dir.as_path());
        assert_eq!(check(&env(&[]), cwd), Check::Go, "no port");
        assert_eq!(check(&env(&[("K2_PORT", "4100")]), cwd), Check::Go);
        assert_eq!(
            check(&env(&[("K2_PORT", "70000")]), cwd),
            Check::Refuse(
                b"Error: K2_PORT must be a number from 1 to 65535 (got: 70000)\n".to_vec()
            )
        );
        assert_eq!(
            check(&env(&[("K5_PORT", "0080"), ("K2_PORT", "4100")]), cwd),
            Check::Refuse(
                b"Error: K5_PORT must be a number from 1 to 65535 (got: 0080)\n".to_vec()
            )
        );
        assert_eq!(
            check(&env(&[("K5_PORT", ""), ("K2SO_PORT", "x")]), cwd),
            Check::Refuse(b"Error: K2SO_PORT must be a number from 1 to 65535 (got: x)\n".to_vec())
        );
        assert_eq!(
            check(&env(&[("K2_PORT", "70000"), ("BASH_ENV", "/x")]), cwd),
            Check::Shell
        );
        let file = dir.join(".k2/heartbeat.port");
        std::fs::write(&file, "4100\n").unwrap();
        assert_eq!(check(&env(&[]), cwd), Check::Go);
        std::fs::write(&file, "\n\n").unwrap();
        assert_eq!(check(&env(&[]), cwd), Check::Go, "empty is no port");
        std::fs::write(&file, "70000\n").unwrap();
        let want = format!(
            "Error: the port in {home}/.k2/heartbeat.port must be a number from 1 to 65535 (got: 70000)\n"
        );
        assert_eq!(check(&env(&[]), cwd), Check::Refuse(want.into_bytes()));
        assert_eq!(
            check(&env(&[("K2_PORT", "4100")]), cwd),
            Check::Go,
            "the setting wins"
        );
        assert_eq!(check(&env(&[]), None), Check::Shell, "no pwd to tell");
        std::fs::write(&file, "70\x00000").unwrap();
        assert_eq!(
            check(&env(&[]), cwd),
            Check::Shell,
            "a NUL: bash warns first"
        );
        std::fs::write(&file, "41\x0000").unwrap();
        assert_eq!(
            check(&env(&[]), cwd),
            Check::Go,
            "a NUL dropped leaves a port"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
