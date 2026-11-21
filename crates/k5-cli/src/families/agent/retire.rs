//! `cmd_agent_retire` and `cmd_agent_restart` (`cli/src/agent.sh`): the
//! conf, then one urllib POST of the block's own (K2's ruling D4).

use super::conf::fetch;
use super::{abs_user, at, cli, dumps, err_json, head, obj, product, py_strip, raised, s, str_or};
use crate::exit::Exit;
use crate::families::{py_get, py_iter, Raise, Shown};
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::{json as pyjson, ops};
use crate::prelude::Closing;
use crate::transport::urllib::{decode, Method, Opener, Outcome, Refused, Request};
use crate::transport::Client;
use std::time::Duration;

/// Both blocks' `urlopen(…, timeout=120)`.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The blocks' lines (`<stdin>`).
mod line {
    pub const RETIRE_PATH: u32 = 18;
    pub const RETIRE_ERR: u32 = 40;
    pub const RETIRE_REFUSED: u32 = 43;
    pub const RETIRE_EXIT4: u32 = 55;
    pub const RETIRE_ACTIONS: u32 = 67;
    pub const RESTART_PATH: u32 = 12;
    pub const RESTART_LIVE: u32 = 15;
    pub const RESTART_READ: u32 = 44;
    pub const RESTART_CHOICES: u32 = 54;
    pub const RESTART_ANSWER: u32 = 70;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetireArgs {
    pub name: String,
    pub archive_to: String,
    pub force: bool,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartArgs {
    pub name: String,
    pub harness: String,
    pub dry_run: bool,
    pub json: bool,
}

/// `cmd_agent_retire`'s words.
pub fn parse_retire(rest: &[&str]) -> Result<RetireArgs, String> {
    let cli = cli();
    let mut a = RetireArgs {
        name: String::new(),
        archive_to: String::new(),
        force: false,
        dry_run: false,
        json: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--archive-to" => {
                let Some(dir) = rest.get(i + 1) else {
                    return Err("--archive-to requires a directory".to_string());
                };
                a.archive_to = (*dir).to_string();
                i += 1;
            }
            "--force" => a.force = true,
            "--dry-run" => a.dry_run = true,
            "--json" => a.json = true,
            _ if w.starts_with('-') => {
                return Err(format!(
                    "unknown flag '{w}' \u{2014} usage: {cli} agent retire <name> [--archive-to <dir>] [--force] [--dry-run] [--json]"
                ))
            }
            _ if a.name.is_empty() => a.name = w.to_string(),
            _ => {
                return Err(format!(
                    "unexpected extra argument '{w}' \u{2014} usage: {cli} agent retire <name> [flags]"
                ))
            }
        }
        i += 1;
    }
    if a.name.is_empty() {
        return Err(format!(
            "missing <name> \u{2014} usage: {cli} agent retire <name> [flags]"
        ));
    }
    Ok(a)
}

/// `cmd_agent_restart`'s words.
pub fn parse_restart(rest: &[&str]) -> Result<RestartArgs, String> {
    let cli = cli();
    let mut a = RestartArgs {
        name: String::new(),
        harness: String::new(),
        dry_run: false,
        json: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--dry-run" => a.dry_run = true,
            "--json" => a.json = true,
            "--harness" => {
                match rest.get(i + 1) {
                    Some(v) if !v.is_empty() && !v.starts_with('-') => {
                        a.harness = (*v).to_string();
                    }
                    _ => {
                        return Err(format!(
                            "--harness needs a value (claude, codex, grok, \u{2026}) \u{2014} usage: {cli} agent restart <name> --harness <harness>"
                        ))
                    }
                }
                i += 1;
            }
            _ if w.starts_with("--harness=") => a.harness = w["--harness=".len()..].to_string(),
            _ if w.starts_with('-') => {
                return Err(format!(
                    "unknown flag '{w}' \u{2014} usage: {cli} agent restart <name> [--harness <harness>] [--dry-run] [--json]"
                ))
            }
            _ if a.name.is_empty() => a.name = w.to_string(),
            _ => {
                return Err(format!(
                    "unexpected extra argument '{w}' \u{2014} usage: {cli} agent restart <name> [flags]"
                ))
            }
        }
        i += 1;
    }
    if a.name.is_empty() {
        return Err(format!(
            "missing <name> \u{2014} usage: {cli} agent restart <name> [flags]"
        ));
    }
    Ok(a)
}

/// `conf["path"]`, or the `KeyError`.
fn conf_path(conf: &Value, line: u32) -> Result<Value, Raise> {
    at(conf, "path")
        .cloned()
        .ok_or_else(|| Raise::at(line, "KeyError: 'path'"))
}

/// `conf.get("name") or os.path.basename(path)`.
fn label_of(conf: &Value, path: &Value, line: u32) -> Result<Value, Raise> {
    if let Some(n) = at(conf, "name").filter(|n| py::truthy(n)) {
        return Ok(n.clone());
    }
    match path {
        Value::Str(_) => {
            let p = py::str_of(path);
            Ok(s(p.rsplit('/').next().unwrap_or("")))
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: expected str, bytes or os.PathLike object, not {}",
                py::type_name(other)
            ),
        )),
    }
}

/// The POST both blocks make: JSON, then the token as Bearer. A redirect
/// is `_K2NoRedirect`'s refusal.
fn post(opener: &Opener, route: &str, body: &Value) -> Result<Outcome, Refused> {
    let mut req = Request::new(Method::Post, route, TIMEOUT)
        .data(dumps(body).into_bytes())
        .header("Content-Type", "application/json");
    if let Some(t) = opener.token() {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    opener.open_ruled(&req)
}

/// `_K2NoRedirect`: the rule's line, then `SystemExit(3)`.
fn refused(r: Refused) -> Shown {
    Shown::err(format!("{}\n", r.line), Refused::EXIT)
}

/// The `daemon_unreachable` line both blocks print for anything urlopen
/// or the answer's reading raised.
fn no_response(why: &str) -> Shown {
    Shown::err(
        err_json(
            "daemon_unreachable",
            &format!("no response from the {} daemon: {why}", product()),
        ) + "\n",
        Exit::Error,
    )
}

/// `cmd_agent_retire`.
pub fn retire(client: &mut Client, closing: &mut Closing, a: &RetireArgs) -> Exit {
    let conf = match fetch(client, closing, &a.name) {
        Ok(d) => d,
        Err(exit) => return exit,
    };
    let opener = Opener::from_client(client);
    retire_block(&opener, &conf, a).print()
}

fn retire_block(opener: &Opener, conf: &Value, a: &RetireArgs) -> Shown {
    let archive_to = if a.archive_to.is_empty() {
        String::new()
    } else {
        abs_user(&a.archive_to)
    };
    let path = match conf_path(conf, line::RETIRE_PATH) {
        Ok(p) => p,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let label = match label_of(conf, &path, line::RETIRE_PATH + 1) {
        Ok(l) => l,
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let body = obj(vec![
        ("q", path.clone()),
        ("force", Value::Bool(a.force)),
        ("dryRun", Value::Bool(a.dry_run)),
        ("archiveTo", s(&archive_to)),
    ]);
    let out = match post(opener, super::RETIRE, &body) {
        Ok(out) => out,
        Err(r) => return refused(r),
    };
    let d = match out {
        Outcome::Answer { body: Ok(b), .. } => match pyjson::loads(&decode(&b)) {
            Ok(d) => d,
            Err(f) => return no_response(&f.message),
        },
        Outcome::Answer { body: Err(r), .. } => return no_response(&r.text()),
        Outcome::HttpError { code, body } => {
            match body.ok().and_then(|b| pyjson::loads(&decode(&b)).ok()) {
                Some(d) => d,
                None => {
                    return Shown::err(
                        err_json("daemon_error", &format!("HTTP {code} on {}", super::RETIRE))
                            + "\n",
                        Exit::Error,
                    )
                }
            }
        }
        Outcome::Raised(r) => return no_response(&r.text()),
    };
    match retire_answer(&d, a.json, &py::str_of(&label), &py::str_of(&path)) {
        Ok(shown) => shown,
        Err((stdout, stderr, r)) => raised(stdout, stderr, &r),
    }
}

type Partial = (String, String, Raise);

fn retire_answer(d: &Value, as_json: bool, label: &str, path: &str) -> Result<Shown, Partial> {
    let none = |r| (String::new(), String::new(), r);
    let err = py_get(d, "error", line::RETIRE_ERR)
        .map_err(none)?
        .cloned()
        .unwrap_or(Value::Null);
    if at(d, "refused").is_some_and(py::truthy) {
        let e = if py::truthy(&err) {
            err.clone()
        } else {
            obj(vec![])
        };
        let hint = py_get(&e, "hint", line::RETIRE_REFUSED)
            .map_err(none)?
            .filter(|h| py::truthy(h))
            .map_or_else(
                || "retire refused by a safety guard".to_string(),
                py::str_of,
            );
        let stderr = if as_json {
            dumps(d) + "\n"
        } else {
            let mut t = format!("REFUSED: {hint}\n");
            if at(d, "dryRun").is_some_and(py::truthy) {
                t.push_str("(dry-run: this is the verdict a real run would produce)\n");
            }
            t
        };
        return Ok(Shown::err(stderr, Exit::Refused));
    }
    if py::truthy(&err) {
        let err = match err {
            Value::Str(_) => obj(vec![("code", s("daemon_error")), ("hint", err)]),
            other => other,
        };
        let line = dumps(&obj(vec![("error", err.clone())])) + "\n";
        let code = py_get(&err, "code", line::RETIRE_EXIT4)
            .map_err(|r| (String::new(), line.clone(), r))?;
        let found =
            code.is_some_and(|c| ops::py_eq(c, &s("not_found")) || ops::py_eq(c, &s("ambiguous")));
        return Ok(Shown::err(
            line,
            if found { Exit::NotFound } else { Exit::Error },
        ));
    }
    if !at(d, "ok").is_some_and(py::truthy) {
        let shown: String = dumps(d).chars().take(300).collect();
        return Ok(Shown::err(
            err_json("daemon_error", &format!("unexpected response: {shown}")) + "\n",
            Exit::Error,
        ));
    }
    if as_json {
        return Ok(Shown::out(dumps(d) + "\n"));
    }
    let dry = at(d, "dryRun").is_some_and(py::truthy);
    let mut out = String::new();
    if dry {
        out.push_str(&format!(
            "DRY RUN \u{2014} no changes made. Plan for retiring {label} ({path}):\n"
        ));
    }
    let actions = at(d, "actions")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let items = match py_iter(&actions, line::RETIRE_ACTIONS) {
        Ok(i) => i,
        Err(r) => return Err((out, String::new(), r)),
    };
    for a in items {
        match action_line(&a) {
            Ok(l) => out.push_str(&l),
            Err(r) => return Err((out, String::new(), r)),
        }
    }
    if !dry {
        match at(d, "archivedTo").filter(|v| py::truthy(v)) {
            Some(dest) => out.push_str(&format!(
                "{label} retired \u{2014} folder archived to {} (nothing was deleted).\n",
                py::str_of(dest)
            )),
            None => out.push_str(&format!("{label} retired.\n")),
        }
    }
    Ok(Shown::out(out))
}

/// One action's line.
fn action_line(a: &Value) -> Result<String, Raise> {
    let l = line::RETIRE_ACTIONS;
    let step = py_get(a, "step", l)?.cloned().unwrap_or_else(|| s("?"));
    let status = py_get(a, "status", l)?.cloned().unwrap_or_else(|| s("?"));
    let is = |v: &Value, w: &str| ops::py_eq(v, &s(w));
    let some = |k: &str| at(a, k).filter(|v| **v != Value::Null);
    let mut detail = String::new();
    if is(&step, "stop-session") && at(a, "sessionId").is_some_and(py::truthy) {
        detail = format!(
            " (session {})",
            head(&at(a, "sessionId").cloned().unwrap_or(Value::Null), 8)
        );
    } else if is(&step, "remove-connections") && some("removed").is_some() {
        detail = format!(" ({} edge(s))", str_or(some("removed"), ""));
    } else if is(&step, "remove-connections") && some("edges").is_some() {
        detail = format!(" ({} edge(s))", str_or(some("edges"), ""));
    } else if is(&step, "clean-dependents") && some("chatSessionNames").is_some() {
        detail = format!(
            " ({} chat-session row(s))",
            str_or(some("chatSessionNames"), "")
        );
    } else if is(&step, "archive-folder") && at(a, "to").is_some_and(py::truthy) {
        detail = format!(" \u{2192} {}", str_or(at(a, "to"), ""));
    }
    if is(&status, "skipped") && at(a, "reason").is_some_and(py::truthy) {
        detail.push_str(&format!("  ({})", str_or(at(a, "reason"), "")));
    }
    let marker = match &status {
        v if is(v, "pending") => s("would apply"),
        v if is(v, "done") => s("done"),
        v if is(v, "skipped") => s("skipped"),
        v if is(v, "failed") => s("FAILED"),
        v => v.clone(),
    };
    let Value::Str(_) = marker else {
        return Err(Raise::at(
            l,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(&marker)
            ),
        ));
    };
    let boxed = format!("[{}]", py::str_of(&marker));
    Ok(format!(
        "  {} {}{detail}\n",
        crate::families::ljust(&boxed, 14),
        py::str_of(&step)
    ))
}

/// `cmd_agent_restart`.
pub fn restart(client: &mut Client, closing: &mut Closing, a: &RestartArgs) -> Exit {
    let conf = match fetch(client, closing, &a.name) {
        Ok(d) => d,
        Err(exit) => return exit,
    };
    let opener = Opener::from_client(client);
    restart_block(&opener, &conf, a).print()
}

fn restart_block(opener: &Opener, conf: &Value, a: &RestartArgs) -> Shown {
    let fail = |r: Raise| raised(String::new(), String::new(), &r);
    let harness = py_strip(&a.harness).to_string();
    let path = match conf_path(conf, line::RESTART_PATH) {
        Ok(p) => p,
        Err(r) => return fail(r),
    };
    let label = match label_of(conf, &path, line::RESTART_PATH + 1) {
        Ok(l) => l,
        Err(r) => return fail(r),
    };
    let live = match at(conf, "live") {
        Some(v) if py::truthy(v) => v.clone(),
        _ => obj(vec![]),
    };
    let label_value = label;
    let label = py::str_of(&label_value);
    let old_id = match py_get(&live, "active", line::RESTART_LIVE) {
        Ok(Some(v)) if py::truthy(v) => at(&live, "sessionId").cloned().unwrap_or(Value::Null),
        Ok(_) => Value::Null,
        Err(r) => return fail(r),
    };
    if a.dry_run {
        let out = obj(vec![
            ("ok", Value::Bool(true)),
            ("dryRun", Value::Bool(true)),
            ("name", label_value.clone()),
            ("path", path.clone()),
            ("oldSessionId", old_id.clone()),
            (
                "actions",
                Value::Array(vec![obj(vec![
                    ("step", s("restart-session")),
                    ("status", s("pending")),
                    ("sessionId", old_id.clone()),
                ])]),
            ),
        ]);
        return Shown::out(if a.json {
            dumps(&out) + "\n"
        } else if py::truthy(&old_id) {
            format!(
                "DRY RUN \u{2014} would restart {label} (session {}) on the same conversation.\n",
                py::str_of(&old_id)
            )
        } else {
            format!("DRY RUN \u{2014} {label} has no live session; restart would start one.\n")
        });
    }
    let mut body = vec![
        ("project", path.clone()),
        ("forceRespawn", Value::Bool(true)),
    ];
    if !harness.is_empty() {
        body.push(("harness", s(&harness)));
    }
    let out = match post(opener, super::ENSURE_PINNED_CHAT, &obj(body)) {
        Ok(out) => out,
        Err(r) => return refused(r),
    };
    let d = match out {
        Outcome::Answer { body: Ok(b), .. } => match pyjson::loads(&decode(&b)) {
            Ok(d) => d,
            Err(f) => return no_response(&f.message),
        },
        Outcome::Answer { body: Err(r), .. } => return no_response(&r.text()),
        Outcome::HttpError { code, body } => {
            let raw = match body {
                Ok(b) => decode(&b),
                Err(r) => return fail(Raise::at(line::RESTART_READ, r.traceback_line())),
            };
            return match restart_refused(code, &raw, &label) {
                Ok(shown) => shown,
                Err(r) => fail(r),
            };
        }
        Outcome::Raised(r) => return no_response(&r.text()),
    };
    match restart_answer(&d, a.json, &label_value, &path, &old_id) {
        Ok(shown) => shown,
        Err(r) => fail(r),
    }
}

/// The block's `except urllib.error.HTTPError` on the body `raw`.
fn restart_refused(code: u16, raw: &str, label: &str) -> Result<Shown, Raise> {
    let msg = match pyjson::loads(raw) {
        Ok(v @ Value::Object(_)) => at(&v, "error").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    if let Value::Object(_) = msg {
        if at(&msg, "code").is_some_and(|c| ops::py_eq(c, &s("harness_choice_needed"))) {
            let choices = match at(&msg, "harnesses") {
                Some(c) if py::truthy(c) => c.clone(),
                _ => Value::Array(Vec::new()),
            };
            let mut names = Vec::new();
            for c in py_iter(&choices, line::RESTART_CHOICES)? {
                let h = py_get(&c, "harness", line::RESTART_CHOICES)?;
                if !h.is_some_and(py::truthy) {
                    continue;
                }
                let installed = at(&c, "installed").is_some_and(py::truthy);
                names.push(format!(
                    "{}{}",
                    str_or(h, "None"),
                    if installed { " (installed)" } else { "" }
                ));
            }
            let names = names.join(", ");
            let names = if names.is_empty() {
                "claude, codex or grok".to_string()
            } else {
                names
            };
            let line = dumps(&obj(vec![(
                "error",
                obj(vec![
                    ("code", s("harness_choice_needed")),
                    (
                        "hint",
                        s(&format!(
                            "{label} has never had a chat, and the first chat on this server waits for you to pick its LLM. Run again with --harness <harness>: {names}. Nothing has started yet."
                        )),
                    ),
                    ("harnesses", choices),
                ]),
            )]));
            return Ok(Shown::err(line + "\n", Exit::Error));
        }
    }
    let msg = match &msg {
        Value::Object(_) => match at(&msg, "hint").filter(|h| py::truthy(h)) {
            Some(h) => h.clone(),
            None => at(&msg, "message").cloned().unwrap_or(Value::Null),
        },
        other => other.clone(),
    };
    let shown = if py::truthy(&msg) {
        py::str_of(&msg)
    } else {
        raw.chars().take(300).collect()
    };
    Ok(Shown::err(
        err_json(
            "daemon_error",
            &format!("restart failed (HTTP {code}): {shown}"),
        ) + "\n",
        Exit::Error,
    ))
}

fn restart_answer(
    d: &Value,
    as_json: bool,
    label_value: &Value,
    path: &Value,
    old_id: &Value,
) -> Result<Shown, Raise> {
    let new_id = py_get(d, "sessionId", line::RESTART_ANSWER)?
        .cloned()
        .unwrap_or(Value::Null);
    if !py::truthy(&new_id) {
        let shown: String = dumps(d).chars().take(300).collect();
        return Ok(Shown::err(
            err_json("daemon_error", &format!("unexpected response: {shown}")) + "\n",
            Exit::Error,
        ));
    }
    let conversation = match at(d, "claudeSessionId") {
        Some(c) if py::truthy(c) => c.clone(),
        _ => Value::Null,
    };
    let resumed = at(d, "resumedExisting").is_some_and(py::truthy);
    let label = py::str_of(label_value);
    if as_json {
        let out = obj(vec![
            ("ok", Value::Bool(true)),
            ("changed", Value::Bool(true)),
            ("name", label_value.clone()),
            ("path", path.clone()),
            ("oldSessionId", old_id.clone()),
            ("sessionId", new_id),
            ("conversationId", conversation),
            ("resumedExisting", Value::Bool(resumed)),
            (
                "provider",
                at(d, "provider").cloned().unwrap_or(Value::Null),
            ),
        ]);
        return Ok(Shown::out(dumps(&out) + "\n"));
    }
    let mut out = if py::truthy(old_id) {
        format!(
            "{label} restarted: session {} \u{2192} {}.\n",
            py::str_of(old_id),
            py::str_of(&new_id)
        )
    } else {
        format!(
            "{label} had no live session; started {}.\n",
            py::str_of(&new_id)
        )
    };
    if py::truthy(&conversation) {
        out.push_str(&format!(
            "Conversation {} ({}).\n",
            py::str_of(&conversation),
            if resumed { "resumed" } else { "new" }
        ));
    }
    Ok(Shown::out(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::parse;

    #[test]
    fn retire_words() {
        assert_eq!(
            parse_retire(&["--archive-to"]).unwrap_err(),
            "--archive-to requires a directory"
        );
        let a = parse_retire(&["qa", "--archive-to", "--force", "--dry-run"]).unwrap();
        assert_eq!(a.archive_to, "--force");
        assert!(a.dry_run && !a.force);
        assert!(parse_retire(&[]).unwrap_err().starts_with("missing <name>"));
    }

    #[test]
    fn restart_words() {
        assert!(parse_restart(&["qa", "--harness", "-x"]).is_err());
        assert!(parse_restart(&["qa", "--harness", ""]).is_err());
        let a = parse_restart(&["qa", "--harness=codex"]).unwrap();
        assert_eq!(a.harness, "codex");
    }

    #[test]
    fn the_retire_plan() {
        let d = parse(
            r#"{"ok":true,"dryRun":true,"actions":[
            {"step":"stop-session","status":"pending","sessionId":"abcdefghij"},
            {"step":"remove-connections","status":"done","removed":2},
            {"step":"archive-folder","status":"skipped","to":"/a","reason":"dirty"}]}"#,
        )
        .unwrap();
        let shown = retire_answer(&d, false, "qa", "/srv/qa").unwrap();
        assert_eq!(
            shown.stdout,
            "DRY RUN \u{2014} no changes made. Plan for retiring qa (/srv/qa):\n  [would apply]  stop-session (session abcdefgh)\n  [done]         remove-connections (2 edge(s))\n  [skipped]      archive-folder \u{2192} /a  (dirty)\n"
        );
    }

    #[test]
    fn a_refusal_is_exit_3() {
        let d = parse(r#"{"refused":true,"error":{"hint":"dirty tree"}}"#).unwrap();
        let shown = retire_answer(&d, false, "qa", "/p").unwrap();
        assert_eq!(shown.exit, Exit::Refused);
        assert_eq!(shown.stderr, "REFUSED: dirty tree\n");
    }

    #[test]
    fn the_harness_choice() {
        let shown = restart_refused(
            409,
            r#"{"error":{"code":"harness_choice_needed","harnesses":[{"harness":"claude","installed":true},{"harness":"codex"}]}}"#,
            "qa",
        )
        .unwrap();
        assert!(shown
            .stderr
            .contains("Run again with --harness <harness>: claude (installed), codex. Nothing"));
        let shown = restart_refused(500, "oops", "qa").unwrap();
        assert_eq!(
            shown.stderr,
            "{\"error\": {\"code\": \"daemon_error\", \"hint\": \"restart failed (HTTP 500): oops\"}}\n"
        );
    }
}
