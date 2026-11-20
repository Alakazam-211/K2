//! `sessions spawn` and `sessions set-label`, as the shell CLI's
//! `cmd_sessions_spawn` (`cli/src/awareness.sh`) and
//! `cmd_sessions_set_label` (`cli/src/talk-and-workspace.sh`) answer them.
//!
//! - `spawn` is retired: one line on stderr, exit 2, whatever words
//!   follow. The router skips its connection gate; the virtual desktop's
//!   guard and the `sessions-spawn` tool's session checks still stand.
//! - `set-label <id> <label words…> [--no-lock]`: after the gates and the
//!   tool's checks, the usage line (exit 1) with no id or no label; else
//!   `cli_request_post /cli/sessions/label id=…&label=…&lock=…` over TCP,
//!   piped into the shell's Python ([`label_reader`]), with `set -o
//!   pipefail`'s exit. The label is the words joined by spaces; bash
//!   before 4.4 stops on the empty array when there are none (the named
//!   difference `bash32-empty-array`).
//!
//! A request with no answer is the transport's line, then SIGPIPE, as for
//! any request on the left of a pipe ([`super::super::end_in_pipe`]).

use super::super::{get, piped_body, pipefail, Piped, Shown};
use crate::daemon::Settings;
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::json::loads;
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

pub const LABEL: &str = "/cli/sessions/label";

/// `cmd_sessions_spawn`'s line.
pub const SPAWN: &str = concat!(
    cli_name!(),
    " sessions spawn moved to: ",
    cli_name!(),
    " sessions new <name> [--cli <cli>] (see ",
    cli_name!(),
    " sessions --help)"
);

/// `set-label`'s usage line.
pub const LABEL_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " sessions set-label <session-id> <label> [--no-lock]"
);

/// What these verbs ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum More {
    Spawn,
    /// The usage line, exit 1.
    LabelUsage,
    /// The request's one argument.
    Label(String),
}

/// The words after `sessions`, for these verbs. `None`: not one of them
/// (or the shell's).
pub fn parse(words: &[&str]) -> Option<More> {
    let (verb, rest) = words.split_first()?;
    match *verb {
        "spawn" => Some(More::Spawn),
        "set-label" => {
            let Some((id, rest)) = rest.split_first().filter(|(id, _)| !id.is_empty()) else {
                return Some(More::LabelUsage);
            };
            let mut lock = "true";
            let mut parts = Vec::new();
            for w in rest {
                if *w == "--no-lock" {
                    lock = "false";
                } else {
                    parts.push(*w);
                }
            }
            let label = parts.join(" ");
            if label.is_empty() {
                return Some(More::LabelUsage);
            }
            Some(More::Label(format!(
                "id={}&label={}&lock={lock}",
                quote(id),
                quote(&label)
            )))
        }
        _ => None,
    }
}

/// Runs `more` after the settings: the gates, the tool's checks, then the
/// verb.
pub fn run(
    args: &[OsString],
    more: More,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    match (&more, prelude::gate(&settings)) {
        (More::Spawn, Some(Gate::VirtualDesktop)) => return Gate::VirtualDesktop.stop(),
        (More::Spawn, _) => {}
        (_, Some(gate)) => return gate.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, super::TOOL) {
        return stop;
    }
    let param = match more {
        More::Spawn => {
            out::stderr_line(SPAWN);
            return Exit::Usage;
        }
        More::LabelUsage => {
            out::stderr_line(LABEL_USAGE);
            return Exit::Error;
        }
        More::Label(p) => p,
    };
    settings.port = settings
        .port_text
        .as_deref()
        .and_then(super::super::curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let piped = match client.call(Channel::Tcp, LABEL, Call::RequestPost(&[param.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(e @ Error::NoAnswer(_)) => {
            out::stderr_line(&e.to_string());
            super::super::end_in_pipe()
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    };
    let shown = label_reader(&String::from_utf8_lossy(&piped.body));
    let exit = pipefail(piped.exit, shown.print());
    closing.flush(&mut client, exit);
    exit
}

fn err(why: String) -> Shown {
    Shown {
        stdout: String::new(),
        stderr: format!("Error: {why}\n"),
        exit: Exit::Error,
    }
}

/// `set-label`'s Python: `Set label to: <label> (locked=<locked>)`, or
/// `Error: <the daemon's error, or the exception>` on stderr, exit 1. The
/// exception texts are Python 3.12's.
pub fn label_reader(raw: &str) -> Shown {
    let d = match loads(raw) {
        Ok(d) => d,
        Err(f) => return err(f.message),
    };
    let has_error = match &d {
        Value::Object(pairs) => get(pairs, "error").is_some(),
        Value::Array(items) => items
            .iter()
            .any(|v| matches!(v, Value::Str(t) if t.as_str() == Some("error"))),
        Value::Str(_) => py::str_of(&d).contains("error"),
        other => {
            return err(format!(
                "argument of type '{}' is not iterable",
                py::type_name(other)
            ))
        }
    };
    let pairs = match &d {
        Value::Object(pairs) => pairs,
        Value::Array(_) if has_error => {
            return err("list indices must be integers or slices, not str".into())
        }
        Value::Str(_) if has_error => {
            return err("string indices must be integers, not 'str'".into())
        }
        other => {
            return err(format!(
                "'{}' object has no attribute 'get'",
                py::type_name(other)
            ))
        }
    };
    if let Some(e) = get(pairs, "error") {
        return err(py::str_of(e));
    }
    let label = get(pairs, "label").map_or_else(String::new, py::str_of);
    let locked = get(pairs, "locked").map_or_else(|| "?".to_string(), py::str_of);
    Shown {
        stdout: format!("Set label to: {label} (locked={locked})\n"),
        stderr: String::new(),
        exit: Exit::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_s_lines() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src");
        let read = |f: &str| {
            std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"))
        };
        assert!(read("awareness.sh").contains(
            r#"echo "k2 sessions spawn moved to: k2 sessions new <name> [--cli <cli>] (see k2 sessions --help)" >&2
    exit 2"#
        ));
        let tw = read("talk-and-workspace.sh");
        for shell in [
            r#"[[ -z "$session_id" ]] && { echo "Usage: k2 sessions set-label <session-id> <label> [--no-lock]" >&2; exit 1; }"#,
            r#"--no-lock) lock="false"; shift ;;"#,
            r#"local label="${label_parts[*]}""#,
            r#"local params="id=$(urlencode "$session_id")&label=$(urlencode "$label")&lock=$lock""#,
            r#"cli_request_post "/cli/sessions/label" "$params" \"#,
            r#"print("Set label to: %s (locked=%s)" % (d.get("label",""), d.get("locked","?")))"#,
        ] {
            assert!(tw.contains(shell), "set-label no longer has {shell:?}");
        }
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&["spawn", "x"]), Some(More::Spawn));
        assert_eq!(parse(&["set-label"]), Some(More::LabelUsage));
        assert_eq!(parse(&["set-label", ""]), Some(More::LabelUsage));
        assert_eq!(
            parse(&["set-label", "s1", "--no-lock"]),
            Some(More::LabelUsage)
        );
        assert_eq!(
            parse(&["set-label", "s 1", "a", "--no-lock", "b&c"]),
            Some(More::Label("id=s%201&label=a%20b%26c&lock=false".into()))
        );
        assert_eq!(parse(&["list"]), None);
    }

    #[test]
    fn the_reader() {
        let r = label_reader(r#"{"label":"x","locked":true}"#);
        assert_eq!(r.stdout, "Set label to: x (locked=True)\n");
        let r = label_reader("{}");
        assert_eq!(r.stdout, "Set label to:  (locked=?)\n");
        assert_eq!(label_reader(r#"{"error":"no"}"#).stderr, "Error: no\n");
        assert_eq!(
            label_reader("").stderr,
            "Error: Expecting value: line 1 column 1 (char 0)\n"
        );
        assert_eq!(
            label_reader("[1]").stderr,
            "Error: 'list' object has no attribute 'get'\n"
        );
    }
}
