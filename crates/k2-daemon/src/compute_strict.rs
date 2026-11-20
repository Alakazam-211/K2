//! `k2 compute run --strict` (0.46.17, agent feedback): a job's exit is
//! its script's last command, so a trailing `grep` or `tee` turns red into
//! green (or green into red), and `sh -c` is dash on Linux nodes, where
//! `set -o pipefail` fails. With `--strict` the server rewrites a
//! `sh -c S` / `bash -c S` job to `bash -euo pipefail -c S`: any failing
//! command, unset variable or failing pipe stage fails the job. bash is on
//! every node (macOS's bash 3.2 has pipefail too). The stored argv is the
//! rewritten one, so `get`, the plan and the job log show what ran, and a
//! retry or a move runs the same. Anything that isn't a shell script is
//! refused: strict means nothing for a plain command.

use serde_json::Value;

/// What `--strict` puts before `-c`.
pub const STRICT: [&str; 3] = ["bash", "-euo", "pipefail"];

/// The submit body's argv, rewritten when it asks for `"strict": true`.
pub fn argv_of(v: &Value) -> Result<Vec<String>, String> {
    let argv: Vec<String> = v.get("argv").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if v.get("strict").and_then(Value::as_bool) == Some(true) {
        strict(&argv)
    } else {
        Ok(argv)
    }
}

/// `sh -c S [args…]` (or bash/dash, by name or path, with other single
/// letter options before the `c`, as in `-lc`) → `bash -euo pipefail
/// [-l] -c S [args…]`.
pub fn strict(argv: &[String]) -> Result<Vec<String>, String> {
    let refuse = || {
        "--strict runs a shell script under bash -euo pipefail: give the command as sh -c '<script>' or bash -c '<script>' (k2 compute run --strict -- bash -c 'cargo test 2>&1 | tee log')".to_string()
    };
    let shell = argv.first().map(|a| a.rsplit('/').next().unwrap_or(a));
    if !matches!(shell, Some("sh" | "bash" | "dash")) {
        return Err(refuse());
    }
    let Some(flags) = argv.get(1).and_then(|f| f.strip_prefix('-')).and_then(|f| f.strip_suffix('c')) else { return Err(refuse()) };
    // Single letters only; `o` takes a value, so it can't be in a cluster.
    if !flags.chars().all(|c| c.is_ascii_alphabetic() && c != 'o') || argv.len() < 3 {
        return Err(refuse());
    }
    let extra: String = flags.chars().filter(|c| !matches!(c, 'e' | 'u')).collect();
    let mut out: Vec<String> = STRICT.iter().map(|s| s.to_string()).collect();
    if !extra.is_empty() {
        out.push(format!("-{extra}"));
    }
    out.push("-c".into());
    out.extend(argv[2..].iter().cloned());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_shell_script_runs_under_bash_euo_pipefail() {
        assert_eq!(strict(&v(&["sh", "-c", "cargo test | tee log"])).unwrap(), v(&["bash", "-euo", "pipefail", "-c", "cargo test | tee log"]));
        assert_eq!(strict(&v(&["bash", "-c", "x", "arg0", "a1"])).unwrap(), v(&["bash", "-euo", "pipefail", "-c", "x", "arg0", "a1"]), "the script's own $0 and args stay");
        assert_eq!(strict(&v(&["/bin/sh", "-ec", "x"])).unwrap(), v(&["bash", "-euo", "pipefail", "-c", "x"]), "-e is already there");
        assert_eq!(strict(&v(&["/usr/bin/bash", "-lc", "x"])).unwrap(), v(&["bash", "-euo", "pipefail", "-l", "-c", "x"]), "a login shell stays one");
        assert_eq!(strict(&v(&["dash", "-c", "x"])).unwrap(), v(&["bash", "-euo", "pipefail", "-c", "x"]));
    }

    #[test]
    fn anything_but_a_shell_script_is_refused() {
        for a in [&["cargo", "test"][..], &["sh", "script.sh"], &["sh", "-c"], &["bash", "--norc", "-c", "x"], &["sh", "-oc", "x"], &["zsh", "-c", "x"], &[]] {
            let e = strict(&v(a)).expect_err(&format!("{a:?}"));
            assert!(e.starts_with("--strict runs a shell script under bash -euo pipefail"), "{e}");
        }
    }

    #[test]
    fn the_body_asks_for_it() {
        let body = serde_json::json!({"argv": ["sh", "-c", "a | b"], "strict": true});
        assert_eq!(argv_of(&body).unwrap(), v(&["bash", "-euo", "pipefail", "-c", "a | b"]));
        let plain = serde_json::json!({"argv": ["sh", "-c", "a | b"]});
        assert_eq!(argv_of(&plain).unwrap(), v(&["sh", "-c", "a | b"]), "without --strict: as given");
        assert!(argv_of(&serde_json::json!({"argv": ["make"], "strict": true})).is_err());
    }
}
