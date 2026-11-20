//! `cmd_agent_harness` and `cmd_agent_refresh` (`cli/src/agent.sh`): move
//! or start again an agent's pinned chat, one `cli_send_json POST` each
//! (`/cli/agent/harness`, `/cli/agent/refresh`). Both are the `id` tool
//! `agent-harness`.

use super::{cli, dumps, err_json, obj, product, s};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::out::pylib::ops;
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::client::Error;
use crate::transport::Client;

pub const HARNESS: &str = "/cli/agent/harness";
pub const REFRESH: &str = "/cli/agent/refresh";
pub const TOOL: &str = "agent-harness";

/// One verb's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// `refresh`: `false`.
    pub harness_verb: bool,
    pub ws: String,
    pub harness: String,
    pub full: bool,
    pub note: Option<String>,
    pub notes_file: String,
    pub model: String,
    pub dry: bool,
    pub json: bool,
}

/// A stop before the request: the `_agent_err` line and its exit.
pub type Stop = (String, Exit);

fn harness_usage() -> String {
    format!(
        "usage: {} agent harness <workspace> <harness> [--full] [--notes-file <path>] [--model <id>] [--dry-run] [--json]  (harness: claude, codex, grok, gemini, cursor, pi or hermes)",
        cli()
    )
}

fn refresh_usage() -> String {
    format!(
        "usage: {} agent refresh [<workspace>] [--note <text> | --notes-file <path>] [--full] [--dry-run] [--json]",
        cli()
    )
}

fn usage_stop(hint: String) -> Stop {
    (err_json("bad_usage", &hint), Exit::Usage)
}

/// The words, up to the checks that need no daemon. `refresh`'s missing
/// workspace check waits for the settings ([`check_refresh_ws`]).
pub fn parse(harness_verb: bool, rest: &[&str]) -> Result<Args, Stop> {
    let usage = if harness_verb {
        harness_usage()
    } else {
        refresh_usage()
    };
    let mut a = Args {
        harness_verb,
        ws: String::new(),
        harness: String::new(),
        full: false,
        note: None,
        notes_file: String::new(),
        model: String::new(),
        dry: false,
        json: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let value = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--full" => a.full = true,
            "--recent" if harness_verb => a.full = false,
            "--notes-file" => {
                if value.is_empty() {
                    return Err(usage_stop(format!("--notes-file takes a path \u{2014} {usage}")));
                }
                a.notes_file = value.to_string();
                i += 1;
            }
            "--model" if harness_verb => {
                if value.is_empty() {
                    return Err(usage_stop(format!("--model takes a model id \u{2014} {usage}")));
                }
                a.model = value.to_string();
                i += 1;
            }
            "--note" if !harness_verb => {
                if value.is_empty() {
                    return Err(usage_stop(format!("--note takes text \u{2014} {usage}")));
                }
                a.note = Some(value.to_string());
                i += 1;
            }
            "--dry-run" => a.dry = true,
            "--json" => a.json = true,
            _ if w.starts_with('-') => {
                return Err(usage_stop(format!("unknown flag '{w}' \u{2014} {usage}")))
            }
            _ if a.ws.is_empty() => a.ws = w.to_string(),
            _ if harness_verb && a.harness.is_empty() => a.harness = w.to_string(),
            _ => {
                return Err(usage_stop(format!(
                    "unexpected extra argument '{w}' \u{2014} {usage}"
                )))
            }
        }
        i += 1;
    }
    if harness_verb && (a.ws.is_empty() || a.harness.is_empty()) {
        return Err(usage_stop(usage));
    }
    if !harness_verb && a.note.is_some() && !a.notes_file.is_empty() {
        return Err(usage_stop(format!(
            "--note and --notes-file are one note: pass one \u{2014} {usage}"
        )));
    }
    if !a.notes_file.is_empty() && !std::path::Path::new(&a.notes_file).is_file() {
        return Err((
            err_json(
                "not_found",
                &format!("notes file not found: {}", a.notes_file),
            ),
            Exit::Error,
        ));
    }
    Ok(a)
}

/// `refresh` with no workspace: `PROJECT_PATH`, and with neither that nor
/// a passport, the usage line. `passport`: `SCOPED_TOKEN` is set.
pub fn check_refresh_ws(a: &mut Args, passport: bool) -> Result<(), Stop> {
    if a.harness_verb || !a.ws.is_empty() {
        return Ok(());
    }
    if let Some((_, v)) = k2_client::env_names::first_nonempty_os(k2_client::env_names::PROJECT_PATH) {
        a.ws = v.to_string_lossy().into_owned();
    }
    if a.ws.is_empty() && !passport {
        return Err(usage_stop(format!(
            "name the workspace \u{2014} {}",
            refresh_usage()
        )));
    }
    Ok(())
}

/// The verb past its checks: the body, the POST and the reader.
pub fn run(client: &mut Client, closing: &mut Closing, a: &Args) -> Exit {
    let mut ws = a.ws.clone();
    if !ws.is_empty() && std::path::Path::new(&ws).is_dir() {
        ws = crate::families::sim::cd_pwd(&crate::families::sim::logical_pwd(), &ws)
            .unwrap_or_default();
    }
    let notes = if a.notes_file.is_empty() {
        a.note.clone().unwrap_or_default()
    } else {
        match std::fs::read(&a.notes_file) {
            Ok(b) => String::from_utf8_lossy(&b)
                .replace("\r\n", "\n")
                .replace('\r', "\n"),
            Err(_) => {
                out::stderr_line(&err_json(
                    "bad_usage",
                    &format!("could not read the notes file: {}", a.notes_file),
                ));
                return Exit::Error;
            }
        }
    };
    let mode = s(if a.full { "full" } else { "recent" });
    let body = if a.harness_verb {
        obj(vec![
            ("workspace", s(&ws)),
            ("harness", s(&a.harness)),
            ("mode", mode),
            ("notes", s(&notes)),
            ("model", s(&a.model)),
            ("dryRun", Value::Bool(a.dry)),
        ])
    } else {
        obj(vec![
            ("workspace", s(&ws)),
            ("mode", mode),
            ("notes", s(&notes)),
            ("dryRun", Value::Bool(a.dry)),
        ])
    };
    let route = if a.harness_verb { HARNESS } else { REFRESH };
    let passport = client.passport().is_some();
    client.send_max_time = Some(std::time::Duration::from_secs(150));
    let (status, body) = match client.send_json(true, route, &dumps(&body)) {
        Ok(r) => {
            closing.answer(&r, passport);
            let mut raw = b"\n000".repeat(client.refused_before_answer() as usize);
            raw.extend(r.body.iter().copied().filter(|b| *b != 0));
            (r.status, raw)
        }
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            return Exit::Error;
        }
        Err(_) => (0, Vec::new()),
    };
    if status == 0 {
        out::stderr_line(&err_json(
            "daemon_unreachable",
            &format!("no answer from the {} daemon", product()),
        ));
        return Exit::Error;
    }
    reader(a.harness_verb, &String::from_utf8_lossy(&body), status, a.json).print()
}

fn get<'a>(d: &'a Value, k: &str) -> Option<&'a Value> {
    super::at(d, k)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `d.get(k) or ""` as `%s` prints it.
fn or_empty(d: &Value, k: &str) -> String {
    match get(d, k) {
        Some(v) if py::truthy(v) => py::str_of(v),
        _ => String::new(),
    }
}

/// The reader on the answer `raw` and its status.
pub fn reader(harness_verb: bool, raw: &str, status: u16, as_json: bool) -> Shown {
    let Ok(d) = crate::out::pylib::json::loads(raw) else {
        return Shown::err(
            err_json(
                "daemon_error",
                &format!("unparseable answer (HTTP {status}): {}", super::cut(raw, 300)),
            ) + "\n",
            Exit::Error,
        );
    };
    if !matches!(d, Value::Object(_)) {
        return Shown::err(
            err_json(
                "daemon_error",
                &format!("unexpected answer (HTTP {status}): {}", super::cut(raw, 300)),
            ) + "\n",
            Exit::Error,
        );
    }
    let l = 16;
    if status >= 300 || truthy(get(&d, "error")) {
        let err = get(&d, "error").cloned().unwrap_or(Value::Null);
        let (code, hint) = match &err {
            Value::Object(_) => (
                match get(&err, "code") {
                    Some(c) if py::truthy(c) => c.clone(),
                    _ => s("error"),
                },
                ["hint", "message"]
                    .iter()
                    .find_map(|k| get(&err, k).filter(|v| py::truthy(v)).cloned())
                    .unwrap_or_else(|| s("")),
            ),
            other => (
                s("error"),
                s(&if py::truthy(other) {
                    py::str_of(other)
                } else {
                    format!("HTTP {status}")
                }),
            ),
        };
        let mut hint_text = py::str_of(&hint);
        if status == 404 {
            if let Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Null = hint {
                return Shown::err(
                    Raise::at(
                        l,
                        format!(
                            "TypeError: argument of type '{}' is not iterable",
                            py::type_name(&hint)
                        ),
                    )
                    .text(),
                    Exit::Error,
                );
            }
        }
        if status == 404
            && matches!(hint, Value::Str(_))
            && hint_text.contains("route")
            && hint_text.contains("not found")
        {
            let cli = cli();
            let product = product();
            hint_text = if harness_verb {
                format!("this {product} daemon has no {cli} agent harness yet (update it); the older door is: {cli} workspace swap-canonical <workspace> --agent <harness>")
            } else {
                format!("this {product} daemon has no {cli} agent refresh yet (update it)")
            };
        }
        let mut stderr = String::new();
        if as_json {
            stderr.push_str(&(dumps(&d) + "\n"));
        } else {
            stderr.push_str(&format!("Error ({}): {hint_text}\n", py::str_of(&code)));
            let path = or_empty(&d, "handoffPath");
            if !path.is_empty() {
                stderr.push_str(&format!("Handoff file: {path}\n"));
            }
        }
        let usage_code = ops::py_eq(&code, &s("usage"))
            || (harness_verb && ops::py_eq(&code, &s("unknown_harness")));
        let exit = match status {
            401 | 403 => Exit::Refused,
            404 => Exit::NotFound,
            _ if usage_code => Exit::Usage,
            _ => Exit::Error,
        };
        return Shown::err(stderr, exit);
    }
    if as_json {
        return Shown::out(dumps(&d) + "\n");
    }
    match lines(harness_verb, &d, l) {
        Ok(t) => Shown::out(t),
        Err((t, r)) => Shown {
            stdout: t,
            stderr: r.text(),
            exit: Exit::Error,
        },
    }
}

fn lines(harness_verb: bool, d: &Value, l: u32) -> Result<String, (String, Raise)> {
    let mut out = String::new();
    let ws = or_empty(d, "workspace");
    let h = or_empty(d, "harness");
    let own = match get(d, "ownSessionHeartbeats") {
        Some(v) if py::truthy(v) => Some(v.clone()),
        _ => None,
    };
    let own_note = |out: &mut String| -> Result<(), Raise> {
        if let Some(own) = &own {
            out.push_str(&format!(
                "Heartbeats with their own session (they do not follow the pinned chat): {}.\n",
                ops::join(own, ", ", l)?
            ));
            out.push_str(&format!(
                "  Move one with: {} heartbeat use-pinned-session on <name>\n",
                cli()
            ));
        }
        Ok(())
    };
    let fail = |out: String, r: Raise| (out, r);
    if harness_verb && truthy(get(d, "alreadyOn")) {
        out.push_str(&format!(
            "{ws}: the pinned chat is already on {h}. Nothing changed.\n"
        ));
        return Ok(out);
    }
    if truthy(get(d, "dryRun")) {
        if harness_verb {
            let from = get(d, "fromHarness");
            if from.is_some_and(|f| ops::py_eq(f, &s(&h))) {
                out.push_str(&format!(
                    "DRY RUN: would start {ws}'s pinned chat again on {h}.\n"
                ));
            } else {
                let from = match from {
                    Some(f) if py::truthy(f) => py::str_of(f),
                    _ => "?".to_string(),
                };
                out.push_str(&format!(
                    "DRY RUN: would move {ws}'s pinned chat from {from} to {h}.\n"
                ));
            }
            if let Some(m) = get(d, "model").filter(|v| py::truthy(v)) {
                out.push_str(&format!("Model: {}\n", py::str_of(m)));
            }
            if let Some(x) = get(d, "launches").filter(|v| py::truthy(v)) {
                out.push_str(&format!("{h} is installed: {}\n", py::str_of(x)));
            }
            out.push_str("The old chat would stay in Chat history; heartbeats, Threads and messages would go to the new chat.\n");
        } else {
            out.push_str(&format!(
                "DRY RUN: would start a new {h} pinned chat for {ws}.\n"
            ));
            if truthy(get(d, "needsPerson")) {
                out.push_str("You can't run it yourself: a person has to allow this. Save your work, then run this again without --dry-run, with your note, for what to do next.\n");
            }
            if let Some(x) = get(d, "launches").filter(|v| py::truthy(v)) {
                out.push_str(&format!("{h} is installed: {}\n", py::str_of(x)));
            }
            out.push_str(&format!(
                "Note: {} characters.\n",
                get(d, "noteChars").map_or_else(|| "0".to_string(), py::str_of)
            ));
            out.push_str("Anything running in the current chat would stop. The old chat would stay in Chat history; the Thread, heartbeats and messages would carry on in the new chat; sidecars would keep running.\n");
        }
        if let Err(r) = own_note(&mut out) {
            return Err(fail(out, r));
        }
        return Ok(out);
    }
    if harness_verb {
        let from = match get(d, "fromHarness") {
            Some(f) if py::truthy(f) => py::str_of(f),
            _ => "?".to_string(),
        };
        out.push_str(&format!("Moved {ws}'s pinned chat: {from} -> {h}.\n"));
        if let Some(m) = get(d, "model").filter(|v| py::truthy(v)) {
            out.push_str(&format!(
                "Model: {} (for this start; a later resume uses the preset or workspace model again)\n",
                py::str_of(m)
            ));
        }
    } else {
        out.push_str(&format!("Refreshed {ws}'s pinned chat on {h}.\n"));
    }
    let conv = or_empty(d, "claudeSessionId");
    let sid = or_empty(d, "sessionId");
    if !conv.is_empty() || !sid.is_empty() {
        let first = if conv.is_empty() {
            "(id assigned when it starts)".to_string()
        } else {
            conv
        };
        let term = if sid.is_empty() {
            String::new()
        } else {
            format!(" (terminal {sid})")
        };
        out.push_str(&format!("New chat: {first}{term}\n"));
    }
    let old = or_empty(d, "fromSessionId");
    if !old.is_empty() {
        if get(d, "oldChatInHistory") == Some(&Value::Bool(false)) {
            out.push_str(&format!(
                "Old chat: {old} (not listed in Chat history: it had no saved conversation).\n"
            ));
        } else {
            out.push_str(&format!("Old chat: {old} (still in Chat history).\n"));
        }
    }
    if harness_verb {
        out.push_str(&format!(
            "Heartbeats on the pinned chat, Threads and messages now go to the {h} chat.\n"
        ));
    } else {
        out.push_str("The Thread, heartbeats on the pinned chat and messages carry on in the new chat.\n");
    }
    if let Err(r) = own_note(&mut out) {
        return Err(fail(out, r));
    }
    if truthy(get(d, "messageDelivered")) {
        out.push_str(if harness_verb {
            "The handover summary was sent into the new chat.\n"
        } else {
            "The summary and note were sent into the new chat.\n"
        });
    } else {
        let why = match get(d, "messageReason") {
            Some(v) if py::truthy(v) => py::str_of(v),
            _ => "not delivered".to_string(),
        };
        let what = if harness_verb {
            "the handover summary was not"
        } else {
            "the summary and note were not"
        };
        out.push_str(&format!("The new chat is up, but {what} sent ({why}).\n"));
        if let Some(hint) = get(d, "messageHint").filter(|v| py::truthy(v)) {
            out.push_str(&format!("{}\n", py::str_of(hint)));
        }
        if let Some(p) = get(d, "handoffPath").filter(|v| py::truthy(v)) {
            let lead = if harness_verb { "It is" } else { "They are" };
            out.push_str(&format!("{lead} on disk: {}\n", py::str_of(p)));
        }
    }
    if let Some(e) = get(d, "seedError").filter(|v| py::truthy(v)) {
        out.push_str(&format!(
            "Note: the old transcript was not copied ({}).\n",
            py::str_of(e)
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert!(parse(true, &["ws"]).is_err());
        let a = parse(true, &["ws", "codex", "--full", "--model", "o3"]).unwrap();
        assert_eq!((a.harness.as_str(), a.full, a.model.as_str()), ("codex", true, "o3"));
        assert!(parse(false, &["--note", "x", "--notes-file", "y"]).is_err());
        assert!(parse(false, &["--model", "x"]).is_err());
        assert_eq!(parse(true, &["a", "b", "--notes-file", "/no/such"]).unwrap_err().1, Exit::Error);
    }

    #[test]
    fn a_move() {
        let r = reader(
            true,
            r#"{"workspace":"qa","harness":"codex","fromHarness":"claude","claudeSessionId":"c1","sessionId":"t1","fromSessionId":"old","messageDelivered":false,"messageReason":"busy","handoffPath":"/h.md"}"#,
            200,
            false,
        );
        assert_eq!(
            r.stdout,
            "Moved qa's pinned chat: claude -> codex.\nNew chat: c1 (terminal t1)\nOld chat: old (still in Chat history).\nHeartbeats on the pinned chat, Threads and messages now go to the codex chat.\nThe new chat is up, but the handover summary was not sent (busy).\nIt is on disk: /h.md\n"
        );
    }

    #[test]
    fn a_refusal() {
        let r = reader(false, r#"{"error":{"code":"needs_person","hint":"ask"},"handoffPath":"/h"}"#, 403, false);
        assert_eq!((r.stderr.as_str(), r.exit), ("Error (needs_person): ask\nHandoff file: /h\n", Exit::Refused));
    }
}
