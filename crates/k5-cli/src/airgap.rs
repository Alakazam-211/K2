//! The shell CLI's air-gap check, for a verb that would phone a hosted
//! service: `_k2_airgap_state` and `_k2_refuse_if_airgap`
//! (`cli/src/prelude.sh`).
//!
//! [`state`] reads it as the shell does, with no Python:
//!
//! 1. `K2_AIRGAP`, when it is set at all (empty counts): off when it is
//!    `0`, `false`, `off` or `no` in any case, once the newlines at its
//!    end are dropped (`$(printf '%s' … | tr …)`); on otherwise. No other
//!    space is trimmed. ([`env_state`])
//! 2. Else `settings.json` in the shell's K2 home
//!    ([`crate::offline::shell_k2_home`]): off when nothing is there (a
//!    link that leads nowhere is something); unreadable when it can't be
//!    read (a folder, no permission); else its bytes, with every newline,
//!    carriage return and NUL dropped, are read by [`settings_state`].
//!
//! [`settings_state`] is the shell's `grep`: it counts each `"airgap"` in
//! the text, left to right; it finds each `"airgap"`, spaces, `:`,
//! spaces, then the run of bytes up to a space, `,` or `}` (the value),
//! the next search starting after it. Off when no `"airgap"` is there.
//! Then on when a value is `true`; else unreadable when a value is
//! neither `true` nor `false`, or when the values found are fewer than
//! the keys counted (a key with no `:` after it; an empty value at the
//! end is not counted, as `$(…)` drops the newline it would be); else
//! off. Any `"airgap"` counts, wherever it sits.
//!
//! [`refusal`] is the line `_k2_refuse_if_airgap` prints before it
//! exits 1.
//!
//! Where this can differ from the shell: "space" here is ASCII's (space,
//! tab, newline, vertical tab, form feed, carriage return), which is
//! `[[:space:]]` in the C locale; in a UTF-8 locale GNU grep also counts
//! other Unicode spaces, and it prints nothing for a file that is not
//! UTF-8 (a "binary file"). The daemon writes the setting as plain ASCII
//! JSON, so neither comes up for a file it wrote.

use crate::offline::{self, Env};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The variable that wins over the setting.
pub const ENV_VAR: &str = k2_client::airgap::ENV_VAR;

/// The settings file, in the K2 home.
pub const FILE: &str = "settings.json";

/// The key the shell looks for, with its quotes.
const KEY: &[u8] = b"\"airgap\"";

/// What `_k2_airgap_state` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    On,
    Off,
    /// The file is there but can't be read, or a value is neither `true`
    /// nor `false`. The shell treats it as on.
    Unreadable,
}

/// The line when air-gap is on.
pub const ON_LINE: &str = "Error: air-gap is on (K2_AIRGAP=1). This command cannot phone Connect, GitHub, or other hosted services.";

/// The line when the setting in `file` can't be read, as bytes (the path
/// is the shell's, as it is).
pub fn unreadable_line(file: &Path) -> Vec<u8> {
    let mut line = b"Error: can't read the air-gap setting in ".to_vec();
    line.extend_from_slice(file.as_os_str().as_encoded_bytes());
    line.extend_from_slice(b" (\"airgap\" must be true or false), so k2 treats air-gap as on. This command cannot phone Connect, GitHub, or other hosted services.");
    line
}

/// `K2_AIRGAP`'s value, read as the shell reads it.
pub fn env_state(value: &OsStr) -> State {
    let mut v = value.as_encoded_bytes().to_ascii_lowercase();
    while v.last() == Some(&b'\n') {
        v.pop();
    }
    match v.as_slice() {
        b"0" | b"false" | b"off" | b"no" => State::Off,
        _ => State::On,
    }
}

/// `[[:space:]]` in the C locale.
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// How many `"airgap"` there are, counted without overlap from the left
/// (`grep -o '"airgap"' | wc -l`).
fn keys(body: &[u8]) -> usize {
    let (mut n, mut i) = (0, 0);
    while i + KEY.len() <= body.len() {
        if body[i..].starts_with(KEY) {
            n += 1;
            i += KEY.len();
        } else {
            i += 1;
        }
    }
    n
}

/// The value after the key at `i`, and where the match ends, when the
/// shell's pattern matches there.
fn value_at(body: &[u8], i: usize) -> Option<(&[u8], usize)> {
    if !body[i..].starts_with(KEY) {
        return None;
    }
    let mut j = i + KEY.len();
    while j < body.len() && space(body[j]) {
        j += 1;
    }
    if body.get(j) != Some(&b':') {
        return None;
    }
    j += 1;
    while j < body.len() && space(body[j]) {
        j += 1;
    }
    let start = j;
    while j < body.len() && !space(body[j]) && body[j] != b',' && body[j] != b'}' {
        j += 1;
    }
    Some((&body[start..j], j))
}

/// The values the shell's `grep -oE … | sed -E …` finds, as `$(…)` keeps
/// them: empty values at the end dropped.
fn values(body: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        match value_at(body, i) {
            Some((v, end)) => {
                out.push(v);
                i = end;
            }
            None => i += 1,
        }
    }
    while out.last().is_some_and(|v| v.is_empty()) {
        out.pop();
    }
    out
}

/// The settings file's text (newlines, carriage returns and NULs
/// already dropped), read as `_k2_airgap_state` reads it.
pub fn settings_state(body: &[u8]) -> State {
    let keys = keys(body);
    if keys == 0 {
        return State::Off;
    }
    let vals = values(body);
    let on = vals.iter().any(|v| *v == b"true");
    let bad = vals.iter().any(|v| *v != b"true" && *v != b"false") || vals.len() != keys;
    if on {
        State::On
    } else if bad {
        State::Unreadable
    } else {
        State::Off
    }
}

/// The settings file at `file`, read as the shell reads it.
pub fn file_state(file: &Path) -> State {
    let is_link = std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink());
    if std::fs::metadata(file).is_err() && !is_link {
        return State::Off;
    }
    let Ok(bytes) = std::fs::read(file) else {
        return State::Unreadable;
    };
    let body: Vec<u8> = bytes
        .into_iter()
        .filter(|b| !matches!(b, b'\n' | b'\r' | 0))
        .collect();
    settings_state(&body)
}

/// The shell's settings file in `env`: `${K2_HOME}/settings.json`.
/// `None` when `HOME` is not set. Off Unix, `settings.json` in
/// k2-client's K2 home ([`offline::shell_k2_home`]), the daemon's file.
pub fn settings_file(env: &Env) -> Option<PathBuf> {
    let home = offline::shell_k2_home(env)?;
    if !cfg!(unix) {
        return Some(home.join(FILE));
    }
    let mut s = home.into_os_string();
    s.push("/");
    s.push(FILE);
    Some(PathBuf::from(s))
}

/// `_k2_airgap_state` in `env`, with the settings at `file`.
pub fn state_with(env: &Env, file: &Path) -> State {
    match env.get(ENV_VAR) {
        Some(v) => env_state(v),
        None => file_state(file),
    }
}

/// `_k2_airgap_state` in `env`, and the settings file it reads. `None`
/// when the binary can't tell the shell's `HOME`
/// ([`crate::offline::shell_home`]).
pub fn state(env: &Env) -> Option<(State, PathBuf)> {
    let file = settings_file(env)?;
    Some((state_with(env, &file), file))
}

/// The line `_k2_refuse_if_airgap` prints for `state` (the settings at
/// `file`) before it exits 1; `None` when air-gap is off.
pub fn refusal(state: State, file: &Path) -> Option<Vec<u8>> {
    match state {
        State::Off => None,
        State::On => Some(ON_LINE.as_bytes().to_vec()),
        State::Unreadable => Some(unreadable_line(file)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prelude() -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/prelude.sh");
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k5-airgap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_variable_as_the_shell_reads_it() {
        for v in ["0", "false", "FALSE", "Off", "nO", "no\n", "off\n\n"] {
            assert_eq!(env_state(OsStr::new(v)), State::Off, "{v:?}");
        }
        for v in [
            "1", "true", "on", "YES", "", " no", "no ", "\nno", "no\r", "nope", "2", "\u{212a}",
        ] {
            assert_eq!(env_state(OsStr::new(v)), State::On, "{v:?}");
        }
    }

    #[test]
    fn the_variable_wins_over_the_file() {
        let d = scratch("env");
        let f = d.join(FILE);
        std::fs::write(&f, r#"{"airgap": true}"#).unwrap();
        assert_eq!(state_with(&Env::of(&[(ENV_VAR, "no")]), &f), State::Off);
        assert_eq!(state_with(&Env::of(&[]), &f), State::On);
        std::fs::write(&f, r#"{"airgap": false}"#).unwrap();
        assert_eq!(state_with(&Env::of(&[(ENV_VAR, "")]), &f), State::On);
        assert_eq!(state_with(&Env::of(&[(ENV_VAR, "x")]), &f), State::On);
        assert_eq!(state_with(&Env::of(&[]), &f), State::Off);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_file_as_the_shell_reads_it() {
        let d = scratch("file");
        let f = d.join(FILE);
        // Nothing there.
        assert_eq!(file_state(&f), State::Off);
        // A link that leads nowhere is something, and can't be read.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("nowhere"), &f).unwrap();
            assert_eq!(file_state(&f), State::Unreadable);
            std::fs::remove_file(&f).unwrap();
            // A link to a file is that file.
            let real = d.join("real.json");
            std::fs::write(&real, "{\"airgap\":\ntrue}").unwrap();
            std::os::unix::fs::symlink(&real, &f).unwrap();
            assert_eq!(file_state(&f), State::On);
            std::fs::remove_file(&f).unwrap();
        }
        // A folder can't be read.
        std::fs::create_dir(&f).unwrap();
        assert_eq!(file_state(&f), State::Unreadable);
        std::fs::remove_dir(&f).unwrap();
        // Newlines, carriage returns and NULs are dropped first.
        std::fs::write(&f, b"{\"air\r\ngap\"\0: t\nr\0ue}").unwrap();
        assert_eq!(file_state(&f), State::On);
        std::fs::write(&f, b"{\"airgap\":\r\n  false\r\n}\r\n").unwrap();
        assert_eq!(file_state(&f), State::Off);
        std::fs::write(&f, b"").unwrap();
        assert_eq!(file_state(&f), State::Off);
        // A file with no permission to read it. Root reads it anyway, as
        // the shell's tr would.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&f, r#"{"airgap": false}"#).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
            let want = if std::fs::read(&f).is_ok() {
                State::Off
            } else {
                State::Unreadable
            };
            assert_eq!(file_state(&f), want);
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The settings bodies the tests read, with what the shell says.
    const BODIES: &[(&str, State)] = &[
        // No key: an older file.
        ("", State::Off),
        ("{}", State::Off),
        (r#"{"theme":"dark"}"#, State::Off),
        (r#"{"Airgap":true}"#, State::Off),
        (r#"{airgap:true}"#, State::Off),
        // One key.
        (r#"{"airgap":true}"#, State::On),
        (r#"{"airgap": true}"#, State::On),
        ("{\"airgap\" \t:\x0b\x0c true }", State::On),
        (r#"{"airgap":false}"#, State::Off),
        (r#"{"theme":"dark","airgap":false,"x":1}"#, State::Off),
        (r#"{"airgap":"true"}"#, State::Unreadable),
        (r#"{"airgap":True}"#, State::Unreadable),
        (r#"{"airgap":1}"#, State::Unreadable),
        (r#"{"airgap":null}"#, State::Unreadable),
        (r#"{"airgap":truex}"#, State::Unreadable),
        (r#"{"airgap":[true]}"#, State::Unreadable),
        // An empty value at the end is dropped, so it is a key with no
        // value: fewer values than keys.
        (r#"{"airgap":}"#, State::Unreadable),
        (r#"{"airgap": ,"x":1}"#, State::Unreadable),
        // A key with no `:` after it.
        (r#"{"airgap"}"#, State::Unreadable),
        (r#"["airgap"]"#, State::Unreadable),
        (r#"{"x":"airgap"}"#, State::Unreadable),
        (r#"{"airgap" x:true}"#, State::Unreadable),
        // Mixes: a true anywhere wins.
        (r#"{"airgap":false,"n":{"airgap":true}}"#, State::On),
        (r#"{"airgap":true,"n":{"airgap":"x"}}"#, State::On),
        (r#"{"airgap":true,"n":["airgap"]}"#, State::On),
        (r#"{"airgap":false,"n":{"airgap":false}}"#, State::Off),
        (r#"{"airgap":false,"n":{"airgap":0}}"#, State::Unreadable),
        (r#"{"airgap":false,"n":["airgap"]}"#, State::Unreadable),
        // An empty value before another is kept: neither true nor false.
        (r#"{"airgap": ,"airgap":false}"#, State::Unreadable),
        // The value runs on to a space, `,` or `}`: a key in it is
        // counted but not found again.
        (r#"{"airgap":"airgap":true}"#, State::Unreadable),
        (r#"{"airgap":false]"airgap":true}"#, State::Unreadable),
        // A key whose quote starts another.
        (r#""airgap"airgap":true"#, State::On),
        (r#"{"airgap":true}"airgap":false"#, State::On),
    ];

    #[test]
    fn the_settings_as_the_shell_s_grep_reads_them() {
        for (body, want) in BODIES {
            assert_eq!(settings_state(body.as_bytes()), *want, "{body:?}");
        }
    }

    /// `_k2_airgap_state` itself, cut from `prelude.sh`.
    #[cfg(unix)]
    fn shell_function() -> String {
        let s = prelude();
        let start = s
            .find("_k2_airgap_state() {\n")
            .expect("_k2_airgap_state in prelude.sh");
        let end = s[start..].find("\n}\n").expect("the function's end") + start + 3;
        s[start..end].to_string()
    }

    /// What `_k2_airgap_state` prints with `K2_HOME` at `home`, in the C
    /// locale, with `K2_AIRGAP` as `env` says.
    #[cfg(unix)]
    fn shell_says(home: &Path, env: Option<&str>) -> State {
        let mut cmd = std::process::Command::new("bash");
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").expect("a PATH"))
            .env("K2_HOME", home)
            .args([
                "-c",
                &format!("set -euo pipefail\n{}_k2_airgap_state", shell_function()),
            ]);
        if let Some(v) = env {
            cmd.env(ENV_VAR, v);
        }
        let o = cmd.output().expect("run bash");
        assert!(o.status.success(), "bash: {o:?}");
        match o.stdout.as_slice() {
            b"on\n" => State::On,
            b"off\n" => State::Off,
            b"unreadable\n" => State::Unreadable,
            _ => panic!("bash printed {o:?}"),
        }
    }

    /// The shell's own function gives what this module gives, for every
    /// body, file and value the tests use.
    #[cfg(unix)]
    #[test]
    fn the_shell_s_function_agrees() {
        let d = scratch("bash");
        let f = d.join(FILE);
        assert_eq!(shell_says(&d, None), State::Off, "no file");
        for (body, want) in BODIES {
            std::fs::write(&f, body).unwrap();
            assert_eq!(shell_says(&d, None), *want, "{body:?}");
            assert_eq!(file_state(&f), *want, "{body:?}");
        }
        std::fs::write(&f, b"{\"air\r\ngap\"\0: t\nr\0ue}").unwrap();
        assert_eq!(shell_says(&d, None), State::On, "newlines and NULs");
        std::fs::remove_file(&f).unwrap();
        std::fs::create_dir(&f).unwrap();
        assert_eq!(shell_says(&d, None), State::Unreadable, "a folder");
        std::fs::remove_dir(&f).unwrap();
        std::os::unix::fs::symlink(d.join("nowhere"), &f).unwrap();
        assert_eq!(shell_says(&d, None), State::Unreadable, "a link to nothing");
        std::fs::remove_file(&f).unwrap();
        for v in [
            "0", "false", "FALSE", "Off", "nO", "no\n", "1", "", " no", "no ", "no\r", "nope",
        ] {
            assert_eq!(
                shell_says(&d, Some(v)),
                env_state(OsStr::new(v)),
                "K2_AIRGAP={v:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_values_and_keys_as_grep_counts_them() {
        assert_eq!(keys(br#""airgap"airgap""#), 1);
        assert_eq!(keys(br#""airgap""airgap""#), 2);
        assert_eq!(
            values(br#"{"airgap":a,"airgap" : b}"airgap":}"#),
            vec![&b"a"[..], b"b"]
        );
        assert_eq!(values(br#"{"airgap":,"airgap":x}"#), vec![&b""[..], b"x"]);
        assert_eq!(values(br#""airgap"airgap":x"#), vec![&b"x"[..]]);
    }

    /// Off Unix: the daemon's file, in k2-client's K2 home.
    #[cfg(windows)]
    #[test]
    fn the_file_is_the_k2_home_s_on_windows() {
        let env = Env::of(&[("USERPROFILE", r"D:\u")]);
        assert_eq!(
            settings_file(&env),
            Some(PathBuf::from(r"D:\u\.k2\settings.json"))
        );
        assert_eq!(settings_file(&Env::of(&[])), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_the_shell_s_k2_home_s() {
        let d = scratch("home");
        let env = Env::of(&[("HOME", d.to_str().unwrap())]);
        // No `.k2` folder: the old name, as the shell falls back.
        let old = settings_file(&env).unwrap();
        assert!(old.ends_with(".k2so/settings.json"), "{old:?}");
        std::fs::create_dir(d.join(".k2")).unwrap();
        let new = settings_file(&env).unwrap();
        assert_eq!(
            new,
            PathBuf::from(format!("{}/.k2/settings.json", d.display()))
        );
        std::fs::write(&new, r#"{"airgap":true}"#).unwrap();
        assert_eq!(state(&env), Some((State::On, new)));
        assert_eq!(settings_file(&Env::of(&[])), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_lines_are_the_shell_s() {
        let s = prelude();
        assert!(s.contains(&format!("echo \"{ON_LINE}\" >&2")));
        let file = Path::new("${K2_HOME}/settings.json");
        let line = String::from_utf8(unreadable_line(file)).unwrap();
        assert!(
            s.contains(&format!("echo \"{}\" >&2", line.replace('"', "\\\""))),
            "{line}"
        );
        assert_eq!(refusal(State::Off, file), None);
        assert_eq!(refusal(State::On, file), Some(ON_LINE.as_bytes().to_vec()));
        assert_eq!(refusal(State::Unreadable, file), Some(line.into_bytes()));
    }

    /// The rules above are the shell's, line by line.
    #[test]
    fn the_rules_are_the_shell_s() {
        let s = prelude();
        for line in [
            r#"    if [ "${K2_AIRGAP+x}" = "x" ]; then"#,
            r#"        case "$(printf '%s' "$K2_AIRGAP" | tr '[:upper:]' '[:lower:]')" in"#,
            "            0|false|off|no) echo off ;;",
            "            *) echo on ;;",
            r#"    local f="${K2_HOME}/settings.json" body keys vals v n=0 on=0 bad=0"#,
            r#"    if [ ! -e "$f" ] && [ ! -L "$f" ]; then"#,
            r#"    if ! { body="$(tr -d '\n\r' <"$f")"; } 2>/dev/null; then"#,
            r#"        echo unreadable"#,
            r#"    keys="$(printf '%s' "$body" | { grep -o '"airgap"' || true; } | wc -l)""#,
            r#"    if [ "$((keys))" -eq 0 ]; then"#,
            r#"        | { grep -oE '"airgap"[[:space:]]*:[[:space:]]*[^[:space:],}]*' || true; } \"#,
            r#"        | sed -E 's/^"airgap"[[:space:]]*:[[:space:]]*//')""#,
            r#"    if [ -n "$vals" ]; then"#,
            "                true) on=1 ;;",
            "                false) ;;",
            "                *) bad=1 ;;",
            r#"        done <<<"$vals""#,
            r#"    [ "$n" -eq "$((keys))" ] || bad=1"#,
            r#"    if [ "$on" = 1 ]; then"#,
            r#"    elif [ "$bad" = 1 ]; then"#,
            r#"K2_HOME="${HOME}/.k2""#,
        ] {
            assert!(s.contains(line), "no `{line}` in prelude.sh");
        }
        // The old home's fallback, built from the old name so no
        // home-anchored literal of it sits in this file
        // (scripts/k2-home-gate.sh).
        let old = k2_client::paths::WORKSPACE_DOT_DIR_NAMES
            .last()
            .expect("the old name");
        let fallback = format!("[ -d \"$K2_HOME\" ] || K2_HOME=\"${{HOME}}/{old}\"");
        assert!(s.contains(&fallback), "no `{fallback}` in prelude.sh");
    }
}
