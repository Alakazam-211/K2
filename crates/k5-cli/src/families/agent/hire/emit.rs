//! The block's `emit(ok, changed)` and `finish(changed)` (`<stdin>` lines
//! 606 to 757): every step's line, or the JSON, then the exit.

use super::super::{dumps, obj, s};
use super::plan::exit_code_for;
use super::{act, exit_of, pget, raise, Frame, Hire, Stop, R};
use crate::exit::Exit;
use crate::families::{ljust, py_iter};
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::ops;

fn line_of(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `", ".join(items)` at `fr`: a `TypeError` for an item that is not a
/// string.
fn join(items: &[Value], fr: &[Frame]) -> Result<String, crate::families::Raise> {
    let mut out = Vec::new();
    for (i, v) in items.iter().enumerate() {
        match v {
            Value::Str(_) => out.push(py::str_of(v)),
            other => {
                return Err(raise(
                    fr,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(out.join(", "))
}

/// `emit(ok, changed)`, its frames below `from`.
pub fn emit(h: &mut Hire, ok: bool, changed: bool, from: &[Frame]) -> R<()> {
    let fr = |line: u32| {
        let mut f = from.to_vec();
        f.push((line, "emit"));
        f
    };
    let dry_run = h.a.dry_run;
    let name = h.display_name.clone();
    if h.a.json {
        let mut out = obj(vec![
            ("ok", Value::Bool(ok)),
            ("changed", Value::Bool(changed)),
            ("name", name),
            ("path", s(&h.path)),
            ("actions", Value::Array(h.actions.clone())),
        ]);
        if dry_run {
            act::set(&mut out, "dryRun", Value::Bool(true));
        }
        h.stdout.push_str(&dumps(&out));
        h.stdout.push('\n');
        return Ok(());
    }
    let name_s = py::str_of(&name);
    if dry_run {
        h.stdout.push_str(&format!(
            "DRY RUN \u{2014} no changes made. Plan for {name_s} ({}):\n",
            h.path
        ));
    }
    let actions = h.actions.clone();
    for a in &actions {
        let step = act::step(a);
        let status = act::get(a, "status").cloned().unwrap_or(Value::Null);
        let g = |k: &str| act::get(a, k);
        let mut line = match step.as_str() {
            "create-dir" => format!("create-dir: {}", h.path),
            "write-persona" => {
                if let Some(t) = g("template") {
                    format!("write-persona: template '{}'", py::str_of(t))
                } else if g("source").is_some_and(|v| ops::py_eq(v, &s("default-scaffold"))) {
                    "write-persona: default scaffold".to_string()
                } else {
                    format!("write-persona: from {}", line_of(g("source")))
                }
            }
            "register-workspace" => {
                let mut l = "register-workspace".to_string();
                if g("wiki").is_some_and(|v| ops::py_eq(v, &s("skip"))) {
                    l.push_str(" (no wiki seed)");
                } else if g("wiki").is_some_and(|v| ops::py_eq(v, &s("seed"))) {
                    l.push_str(" (seed wiki)");
                }
                l
            }
            "set-mode" => format!("mode: {}", line_of(g("mode"))),
            "set-enabled" => format!(
                "enabled: {}",
                if truthy(g("enabled")) { "true" } else { "false" }
            ),
            "set-name" => format!("name: {}", line_of(g("name"))),
            "set-default-agent" => format!(
                "default-agent: {} ({})",
                line_of(g("label")),
                line_of(g("preset"))
            ),
            "set-db-access" => format!("db-access: {}", line_of(g("access"))),
            "connect" => {
                let mut l = format!("connect: {} (bidirectional)", line_of(g("peer")));
                if truthy(g("warn")) {
                    l.push_str(&format!("  (warn: {})", line_of(g("warn"))));
                }
                l
            }
            "join-project" => {
                let mut l = format!("join-project: {}", line_of(g("project")));
                if truthy(g("becamePoc")) {
                    l.push_str(" (first member \u{2014} Point of Contact)");
                }
                l
            }
            "set-color" => format!("color: {}", line_of(g("color"))),
            "assign-focus-group" => format!(
                "focus-group: {}",
                py::str_of(&super::or(g("group"), s("none")))
            ),
            "seed-context" => {
                let mut l = if g("kind").is_some_and(|v| ops::py_eq(v, &s("catalog"))) {
                    let c = g("catalog").or(g("context"));
                    format!("context: catalog {}", line_of(c))
                } else {
                    let p = super::or(g("path"), g("context").cloned().unwrap_or(Value::Null));
                    format!("context: {}", py::str_of(&p))
                };
                if truthy(g("warn")) {
                    l.push_str(&format!("  (warn: {})", line_of(g("warn"))));
                }
                l
            }
            "launch" => {
                let mut l = "launch".to_string();
                if truthy(g("sessionId")) {
                    let id: String = line_of(g("sessionId")).chars().take(8).collect();
                    l.push_str(&format!(" (session {id})"));
                }
                l
            }
            "apply-template" => {
                let t = super::or(g("info"), obj(vec![]));
                let mut l = format!("template: {}", line_of(g("template")));
                if pget(&t, "id", &fr(669))?.is_some_and(py::truthy) {
                    l.push_str(&format!(
                        " ({} v{})",
                        line_of(act::get(&t, "id")),
                        line_of(act::get(&t, "version"))
                    ));
                }
                l
            }
            "hello" => "hello: the agent says hello on its Thread".to_string(),
            "generate-agents-md" => "agents-md: generate AGENTS.md".to_string(),
            "fanout" => "fanout: leftover harness names (CLAUDE.md, GEMINI.md\u{2026})".to_string(),
            "take-app" => {
                let mut l = format!("take-app: hand {} to this agent", line_of(g("app")));
                let hv = super::or(g("handover"), obj(vec![]));
                if act::get(&hv, "id").is_some_and(py::truthy) {
                    let card = match act::get(&hv, "request") {
                        Some(r) if py::truthy(r) => match r {
                            Value::Str(_) => format!(", card {}", py::str_of(r)),
                            other => {
                                return Err(raise(
                                    &fr(681),
                                    format!(
                                        "TypeError: can only concatenate str (not \"{}\") to str",
                                        py::type_name(other)
                                    ),
                                )
                                .into())
                            }
                        },
                        _ => String::new(),
                    };
                    l.push_str(&format!(
                        " (handover {} {}{card})",
                        line_of(act::get(&hv, "id")),
                        line_of(act::get(&hv, "state"))
                    ));
                }
                l
            }
            _ => "onboard: deliver first message".to_string(),
        };
        let is = |t: &str| ops::py_eq(&status, &s(t));
        if (is("skipped") || is("not-run")) && truthy(g("reason")) {
            line.push_str(&format!("  ({})", line_of(g("reason"))));
        }
        if is("failed") && truthy(g("error")) {
            let e = g("error").cloned().unwrap_or(Value::Null);
            line.push_str(&format!(
                "  ({}: {})",
                line_of(act::get(&e, "code")),
                line_of(act::get(&e, "hint"))
            ));
        }
        let marker = match status.clone() {
            Value::Str(t) => match t.as_str() {
                Some("pending") => "would apply",
                Some("done") => "done",
                Some("skipped") => "skipped",
                Some("failed") => "FAILED",
                Some("not-run") => "not run",
                _ => {
                    return Err(raise(&fr(688), format!("KeyError: {}", py::repr(&status))).into())
                }
            },
            Value::Array(_) | Value::Object(_) => {
                return Err(raise(
                    &fr(688),
                    format!("TypeError: unhashable type: '{}'", py::type_name(&status)),
                )
                .into())
            }
            other => {
                return Err(raise(&fr(688), format!("KeyError: {}", py::repr(&other))).into())
            }
        };
        h.stdout.push_str(&format!(
            "  {} {line}\n",
            ljust(&format!("[{marker}]"), 14)
        ));
        if step == "apply-template" {
            template_rows(h, a, &fr)?;
        }
    }
    if !dry_run {
        let n = actions.iter().filter(|a| act::is(a, "status", "done")).count();
        let tail = if n > 0 {
            format!("{n} change(s) applied.")
        } else {
            "no changes \u{2014} already converged.".to_string()
        };
        h.stdout.push_str(&format!("{name_s}: {tail}\n"));
    }
    let nf = actions
        .iter()
        .filter(|a| act::is(a, "status", "failed"))
        .count();
    if nf > 0 {
        h.stdout.push_str(&format!(
            "{name_s}: {nf} step(s) {}.\n",
            if dry_run { "would fail" } else { "failed" }
        ));
    }
    Ok(())
}

/// The template's own rows under its line (lines 691 to 723).
fn template_rows(h: &mut Hire, a: &Value, fr: &dyn Fn(u32) -> Vec<Frame>) -> R<()> {
    let rows = super::or(act::get(a, "rows"), Value::Array(vec![]));
    let rows = py_iter(&rows, 695).map_err(|mut e| {
        e.frames = fr(695);
        e
    })?;
    for r in &rows {
        let step = pget(r, "step", &fr(696))?.cloned().unwrap_or(Value::Null);
        if ops::py_eq(&step, &s("folder")) || ops::py_eq(&step, &s("register")) {
            continue;
        }
        let mut txt = format!(
            "{} {}",
            py::str_of(&step),
            line_of(act::get(r, "target"))
        );
        if act::get(r, "reason").is_some_and(py::truthy) {
            txt.push_str(&format!("  ({})", line_of(act::get(r, "reason"))));
        }
        let st = act::get(r, "status").cloned().unwrap_or(Value::Null);
        let word = match &st {
            Value::Str(t) => match t.as_str() {
                Some("write") => "would write".to_string(),
                Some("done") => "wrote".to_string(),
                Some("skip") => "skip".to_string(),
                Some("yours") => "yours, kept".to_string(),
                Some("suggest") => "suggested".to_string(),
                _ => py::str_of(&st),
            },
            Value::Array(_) | Value::Object(_) => {
                return Err(raise(
                    &fr(701),
                    format!("TypeError: unhashable type: '{}'", py::type_name(&st)),
                )
                .into())
            }
            other => py::str_of(other),
        };
        h.stdout
            .push_str(&format!("      {} {txt}\n", ljust(&word, 12)));
    }
    if act::get(a, "permissionsNote").is_some_and(py::truthy) {
        h.stdout.push_str(&format!(
            "      {}\n",
            line_of(act::get(a, "permissionsNote"))
        ));
    }
    if let Some(hs) = act::get(a, "hireSwitch").filter(|v| py::truthy(v)) {
        let code = super::or(act::get(hs, "code"), s("hire_off"));
        h.stdout.push_str(&format!(
            "      {} 'Let this agent hire agents': it is off, so the real hire is refused ({})\n",
            ljust("needs", 12),
            py::str_of(&code)
        ));
        if act::get(hs, "command").is_some_and(py::truthy) {
            h.stdout.push_str(&format!(
                "      {} {}\n",
                ljust("ask with", 12),
                line_of(act::get(hs, "command"))
            ));
        } else if act::get(hs, "hint").is_some_and(py::truthy) {
            h.stdout.push_str(&format!(
                "      {} {}\n",
                ljust("", 12),
                line_of(act::get(hs, "hint"))
            ));
        }
    }
    let qs = super::or(act::get(a, "questions"), Value::Array(vec![]));
    let writes_role = rows.iter().any(|r| {
        act::get(r, "step").is_some_and(|v| ops::py_eq(v, &s("role")))
            && act::get(r, "status")
                .is_some_and(|v| ops::py_eq(v, &s("write")) || ops::py_eq(v, &s("done")))
    });
    if py::truthy(&qs) && writes_role {
        let items = py_iter(&qs, 716).map_err(|mut e| {
            e.frames = fr(716);
            e
        })?;
        let mut got = Vec::new();
        for q in &items {
            if pget(q, "answered", &fr(716))?.is_some_and(py::truthy) {
                got.push(index_id(q, &fr(716))?);
            }
        }
        let mut left = Vec::new();
        for q in &items {
            if !pget(q, "answered", &fr(717))?.is_some_and(py::truthy) {
                left.push(q.clone());
            }
        }
        if !got.is_empty() {
            let j = join(&got, &fr(719))?;
            h.stdout
                .push_str(&format!("      {} {j}\n", ljust("answered", 12)));
        }
        if !left.is_empty() {
            let mut ids = Vec::new();
            let mut inner = fr(721);
            inner.push((721, "<genexpr>"));
            for q in &left {
                ids.push(index_id(q, &inner)?);
            }
            let j = join(&ids, &fr(721))?;
            h.stdout.push_str(&format!(
                "      {} {j}  (the agent asks its person on its first start)\n",
                ljust("open", 12)
            ));
        } else {
            h.stdout.push_str(&format!(
                "      {} none: every question was answered\n",
                ljust("open", 12)
            ));
        }
    }
    Ok(())
}

/// `q["id"]` on a dict.
fn index_id(q: &Value, fr: &[Frame]) -> Result<Value, crate::families::Raise> {
    act::get(q, "id")
        .cloned()
        .ok_or_else(|| raise(fr, "KeyError: 'id'"))
}

/// `finish(changed)`, called from module line `from`: every step, then
/// exit 1 to 4 with the first failure's code when one failed.
pub fn finish(h: &mut Hire, changed: bool, from: u32) -> R<()> {
    let failed: Vec<Value> = h
        .actions
        .iter()
        .filter(|a| act::is(a, "status", "failed"))
        .cloned()
        .collect();
    emit(h, failed.is_empty(), changed, &[(from, "<module>"), (747, "finish")])?;
    let Some(first_failed) = failed.first() else {
        return Err(Stop::Exit(Exit::Ok));
    };
    let first = act::get(first_failed, "error")
        .cloned()
        .unwrap_or(Value::Null);
    let what = failed
        .iter()
        .map(|a| {
            let who = ["peer", "project", "context", "name"]
                .iter()
                .find_map(|k| act::get(a, k).filter(|v| py::truthy(v)))
                .map(py::str_of)
                .unwrap_or_default();
            let e = act::get(a, "error").cloned().unwrap_or(Value::Null);
            format!(
                "{} {who}: {}",
                act::step(a),
                line_of(act::get(&e, "hint"))
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let code_v = act::get(&first, "code").cloned().unwrap_or(Value::Null);
    let code = super::or(Some(&code_v), s("daemon_error"));
    let hint = format!(
        "{} step(s) {}: {what}",
        failed.len(),
        if h.a.dry_run { "would fail" } else { "failed" }
    );
    let exit = match act::get(first_failed, "exitCode").filter(|v| py::truthy(v)) {
        Some(Value::Int(n)) => n.parse().unwrap_or(1),
        _ => exit_code_for(&code_v, &[(from, "<module>"), (754, "finish")])?,
    };
    Err(h.fail(code, s(&hint), exit_of(exit)))
}

/// `step_failed(action, code, hint, exit_code, fatal)`, called from module
/// line `from`.
pub fn step_failed(
    h: &mut Hire,
    i: usize,
    code: Value,
    hint: Value,
    exit: Option<i64>,
    fatal: bool,
    from: u32,
) -> R<()> {
    let a = &mut h.actions[i];
    step_failed_on(a, code.clone(), hint.clone(), exit);
    if !fatal {
        return Ok(());
    }
    for a in h.actions.iter_mut() {
        if act::pending(a) {
            act::set(a, "status", s("not-run"));
            act::set(a, "reason", s("the workspace step failed"));
        }
    }
    let changed = h.changed;
    emit(h, false, changed, &[(from, "<module>"), (834, "step_failed")])?;
    let exit = match exit {
        Some(n) => n,
        None => exit_code_for(&code, &[(from, "<module>"), (835, "step_failed")])?,
    };
    Err(h.fail(code, hint, exit_of(exit)))
}

/// `step_failed`'s marks on one action (also one not in the list yet).
pub fn step_failed_on(a: &mut Value, code: Value, hint: Value, exit: Option<i64>) {
    act::set(a, "status", s("failed"));
    act::set(a, "error", obj(vec![("code", code), ("hint", hint)]));
    if let Some(n) = exit {
        act::set(a, "exitCode", Value::Int(n.to_string()));
    }
}
