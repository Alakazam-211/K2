//! The real run (`<stdin>` lines 819 to 1244): each step in turn, a
//! failed one reported and the rest going on, unless the workspace itself
//! couldn't be made.

use super::super::{cli, obj, product, s};
use super::emit::{finish, step_failed, step_failed_on};
use super::plan::{is_file, take_template_rows, template_exit};
use super::{act, dumps_head, or, pget, ptrue, raise, Hire, R};
use crate::families::py_iter;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::{ops, path as pypath};

const CORE_STEPS: [&str; 4] = [
    "create-dir",
    "write-persona",
    "register-workspace",
    "set-db-access",
];

/// `err if isinstance(err, str) else (err or {}).get("hint") or
/// json.dumps(resp)[:300]`, at `line`.
fn hint_of(resp: &Value, line: u32) -> R<Value> {
    let err = act::get(resp, "error").cloned().unwrap_or(Value::Null);
    if let Value::Str(_) = err {
        return Ok(err);
    }
    let e = or(Some(&err), obj(vec![]));
    let hint = pget(&e, "hint", &[(line, "<module>")])?.cloned();
    Ok(or(hint.as_ref(), s(&dumps_head(resp, 300))))
}

/// `resp.get(key, json.dumps(resp)[:300])`.
fn get_or_dump(resp: &Value, key: &str) -> Value {
    act::get(resp, key)
        .cloned()
        .unwrap_or_else(|| s(&dumps_head(resp, 300)))
}

fn pending_at(h: &Hire, i: Option<usize>) -> Option<usize> {
    i.filter(|&i| act::pending(&h.actions[i]))
}

pub fn apply(h: &mut Hire) -> R<()> {
    let path = h.path.clone();
    let a = h.a;

    // 1-3: the hire core.
    let pending_core: Vec<usize> = CORE_STEPS
        .iter()
        .filter_map(|st| pending_at(h, h.one(st)))
        .collect();
    let tpl = h.one("apply-template");
    let mut template_hello = Value::Null;
    let mut hello_by_daemon = false;
    let mut onboard_by_daemon = false;
    if !pending_core.is_empty() || tpl.is_some() {
        let mut body = if tpl.is_some() {
            let mut b = h.template_body(vec![("path", s(&path))]);
            if let Some(p) = &h.resolved_preset {
                act::set(&mut b, "preset", act::get(p, "id").cloned().unwrap_or(Value::Null));
            }
            b
        } else {
            obj(vec![
                ("path", s(&path)),
                ("noWiki", Value::Bool(a.no_wiki)),
                ("noAgentsMd", Value::Bool(a.no_agents_md)),
                ("fanout", Value::Bool(a.fanout)),
                ("role", s("")),
            ])
        };
        if let (Some(content), Some(_)) = (&h.persona_content, pending_at(h, h.one("write-persona"))) {
            act::set(&mut body, "personaRaw", s(content));
        }
        let dba = pending_at(h, h.one("set-db-access"));
        if let Some(i) = dba {
            let access = act::get(&h.actions[i], "access").cloned().unwrap_or(Value::Null);
            act::set(&mut body, "dbAccess", access);
        }
        if !h.take_app.is_empty() {
            act::set(&mut body, "takeApp", s(&h.take_app));
            if let Some(b) = a.brief.as_ref().filter(|b| !b.code_points().is_empty()) {
                act::set(&mut body, "brief", Value::Str(b.clone()));
            }
        }
        if tpl.is_some() {
            act::set(&mut body, "launch", Value::Bool(a.launch));
            if a.launch && !a.onboard.is_empty() {
                act::set(&mut body, "onboard", s(&a.onboard));
            }
        }
        let resp = h.post_json("/cli/agents/hire", &body);
        let err = pget(&resp, "error", &[(882, "<module>")])?.cloned();
        if err.as_ref().is_some_and(py::truthy) || !ptrue(&resp, "ok", &[(883, "<module>")])? {
            h.note_permission(&resp);
            let (code, hint) = match &err {
                Some(e @ Value::Object(_)) => (
                    act::get(e, "code").cloned().unwrap_or(Value::Null),
                    act::get(e, "hint").cloned().unwrap_or(Value::Null),
                ),
                other => (
                    s("daemon_error"),
                    or(other.as_ref(), s(&dumps_head(&resp, 300))),
                ),
            };
            let owner_only = ops::py_eq(&code, &s("owner_only"));
            let bad = match dba {
                Some(i) if owner_only => i,
                _ => pending_core
                    .first()
                    .copied()
                    .or(tpl)
                    .expect("the core runs for a pending step or a template"),
            };
            let refused = ["owner_only", "hire_off", "main_only"]
                .iter()
                .any(|c| ops::py_eq(&code, &s(c)));
            let exit_for = if refused {
                Some(3)
            } else if tpl.is_some() {
                Some(template_exit(&code, &[(889, "<module>")])?)
            } else {
                None
            };
            let code = or(Some(&code), s("daemon_error"));
            return step_failed(h, bad, code, hint, exit_for, true, 890);
        }
        let steps = act::get(&resp, "steps")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let mut by_core: Vec<(Value, Value)> = Vec::new();
        for x in py_iter(&steps, 891)? {
            let k = pget(&x, "step", &[(891, "<module>")])?
                .cloned()
                .unwrap_or(Value::Null);
            if matches!(k, Value::Array(_) | Value::Object(_)) {
                return Err(raise(
                    &[(891, "<module>")],
                    format!("TypeError: unhashable type: '{}'", py::type_name(&k)),
                )
                .into());
            }
            match by_core.iter_mut().find(|(key, _)| ops::py_eq(key, &k)) {
                Some(slot) => slot.1 = x,
                None => by_core.push((k, x)),
            }
        }
        if let Some(i) = tpl {
            take_template_rows(h, i, &resp, true, 893)?;
            h.changed = h.changed || act::is(&h.actions[i], "status", "done");
            template_hello = act::get(&resp, "hello").cloned().unwrap_or(Value::Null);
            hello_by_daemon = matches!(act::get(&resp, "helloDelivered"), Some(Value::Bool(true)));
            onboard_by_daemon =
                hello_by_daemon && matches!(act::get(&resp, "onboardQueued"), Some(Value::Bool(true)));
        }
        if let (Some(ta), Some(hv @ Value::Object(_))) = (h.one("take-app"), act::get(&resp, "handover")) {
            let hv = hv.clone();
            act::set(&mut h.actions[ta], "handover", hv.clone());
            match act::get(&hv, "error") {
                Some(e @ Value::Object(_)) => {
                    let code = or(act::get(e, "code"), s("daemon_error"));
                    let hint = or(act::get(e, "hint"), s("the handover was not filed"));
                    step_failed_on(&mut h.actions[ta], code, hint, None);
                }
                _ => act::set(&mut h.actions[ta], "status", s("done")),
            }
        }
        for st in CORE_STEPS {
            if let Some(i) = pending_at(h, h.one(st)) {
                let got = by_core
                    .iter()
                    .find(|(k, _)| ops::py_eq(k, &s(st)))
                    .map(|(_, v)| v.clone());
                let got = or(got.as_ref(), obj(vec![]));
                let status = or(act::get(&got, "status"), s("done"));
                act::set(&mut h.actions[i], "status", status);
                if let Some(r) = act::get(&got, "reason").filter(|r| py::truthy(r)) {
                    act::set(&mut h.actions[i], "reason", r.clone());
                }
                h.changed = h.changed || act::is(&h.actions[i], "status", "done");
            }
        }
        if h.one("register-workspace").is_some_and(|i| act::is(&h.actions[i], "status", "done")) {
            if let Some(i) = pending_at(h, h.one("generate-agents-md")) {
                let agents = pypath::join(&path, "AGENTS.md");
                let link = std::fs::symlink_metadata(&agents).is_ok_and(|m| m.file_type().is_symlink());
                let mut text = String::new();
                if is_file(&agents) && !link {
                    if let Ok(b) = std::fs::read(&agents) {
                        text = String::from_utf8_lossy(&b).into_owned();
                    }
                }
                if link || text.contains("GENERATED by K2") {
                    act::set(&mut h.actions[i], "status", s("done"));
                } else {
                    act::set(&mut h.actions[i], "status", s("skipped"));
                    act::set(&mut h.actions[i], "reason", s("user AGENTS.md present"));
                }
            }
            if let Some(i) = pending_at(h, h.one("fanout")) {
                act::set(&mut h.actions[i], "status", s("done"));
            }
        }
    }

    // 4: mode, enabled, default agent and DB access in one settings batch.
    let ma = pending_at(h, h.one("set-mode"));
    let ea = pending_at(h, h.one("set-enabled"));
    let da = pending_at(h, h.one("set-default-agent"));
    let dba = pending_at(h, h.one("set-db-access"));
    let field = |i: Option<usize>, key: &str| i.and_then(|i| act::get(&h.actions[i], key).cloned());
    let mut fields = Vec::new();
    if let Some(m) = field(ma, "mode") {
        fields.push(("agent_mode", m));
    }
    if ea.is_some() {
        let on = field(ea, "enabled").is_some_and(|v| py::truthy(&v));
        fields.push(("agent_enabled", s(if on { "1" } else { "0" })));
    }
    if let Some(p) = field(da, "preset") {
        fields.push(("default_agent", p));
    }
    if let Some(x) = field(dba, "access") {
        fields.push(("db_agent_access", x));
    }
    if !fields.is_empty() {
        let resp = h.post_json(
            "/cli/workspace/set",
            &obj(vec![("project", s(&path)), ("fields", obj(fields))]),
        );
        let bad = ptrue(&resp, "error", &[(954, "<module>")])? || !ptrue(&resp, "ok", &[(954, "<module>")])?;
        if bad {
            let hint = hint_of(&resp, 956)?;
            for i in [ma, ea, da, dba].into_iter().flatten() {
                if act::pending(&h.actions[i]) {
                    step_failed_on(
                        &mut h.actions[i],
                        s("daemon_error"),
                        s(&format!("settings write failed: {}", py::str_of(&hint))),
                        None,
                    );
                }
            }
        }
        let list = act::get(&resp, "actions")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let mut by_field: Vec<(Value, Value)> = Vec::new();
        for x in py_iter(&list, 961)? {
            let k = pget(&x, "field", &[(961, "<module>")])?
                .cloned()
                .unwrap_or(Value::Null);
            let v = act::get(&x, "status").cloned().unwrap_or(Value::Null);
            match by_field.iter_mut().find(|(key, _)| ops::py_eq(key, &k)) {
                Some(slot) => slot.1 = v,
                None => by_field.push((k, v)),
            }
        }
        let status_of = |key: &str| {
            by_field
                .iter()
                .find(|(k, _)| ops::py_eq(k, &s(key)))
                .map_or_else(|| s("done"), |(_, v)| v.clone())
        };
        for (i, key) in [
            (ma, "agent_mode"),
            (ea, "agent_enabled"),
            (da, "default_agent"),
            (dba, "db_agent_access"),
        ] {
            let Some(i) = i.filter(|&i| act::pending(&h.actions[i])) else {
                continue;
            };
            act::set(&mut h.actions[i], "status", status_of(key));
            if act::is(&h.actions[i], "status", "skipped") {
                let reason = if key == "agent_mode" {
                    let m = act::get(&h.actions[i], "mode").map(py::str_of).unwrap_or_default();
                    format!("already {m}")
                } else {
                    "already set".to_string()
                };
                act::set(&mut h.actions[i], "reason", s(&reason));
            }
            h.changed = h.changed || act::is(&h.actions[i], "status", "done");
        }
    }

    // 5: the name (both name stores).
    if let Some(i) = pending_at(h, h.one("set-name")) {
        let name = act::get(&h.actions[i], "name").map(py::str_of).unwrap_or_default();
        let resp = h.post_query(
            "/cli/workspace/set-agent-display-name",
            vec![("project_path", path.clone()), ("name", name)],
            super::http::DEFAULT,
        );
        let bad = ptrue(&resp, "error", &[(989, "<module>")])? || !ptrue(&resp, "success", &[(989, "<module>")])?;
        if bad {
            let why = get_or_dump(&resp, "error");
            step_failed_on(
                &mut h.actions[i],
                s("daemon_error"),
                s(&format!("rename failed: {}", py::str_of(&why))),
                None,
            );
        } else {
            act::set(&mut h.actions[i], "status", s("done"));
            h.changed = true;
        }
    }

    // 5b: color and focus group, by the workspace's id.
    let ca = h.one("set-color");
    let fa = h.one("assign-focus-group");
    let need_id = pending_at(h, ca).is_some() || pending_at(h, fa).is_some();
    let mut workspace_id = if h.registered {
        act::get(&h.conf, "id").cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    if need_id && !py::truthy(&workspace_id) {
        let conf2 = h.get(super::super::CONF, vec![("q", path.clone())]);
        if ptrue(&conf2, "ok", &[(1003, "<module>")])? && ptrue(&conf2, "id", &[(1003, "<module>")])? {
            workspace_id = act::get(&conf2, "id").cloned().unwrap_or(Value::Null);
            h.conf = conf2;
        }
    }
    if need_id && !py::truthy(&workspace_id) {
        let listing = h.get("/cli/projects/list", vec![]);
        if let Value::Array(rows) = &listing {
            for p in rows {
                if matches!(p, Value::Object(_))
                    && act::get(p, "path").is_some_and(|v| ops::py_eq(v, &s(&path)))
                    && act::get(p, "id").is_some_and(py::truthy)
                {
                    workspace_id = act::get(p, "id").cloned().unwrap_or(Value::Null);
                    break;
                }
            }
        }
    }
    if need_id && !py::truthy(&workspace_id) {
        for i in [ca, fa].into_iter().flatten() {
            if act::pending(&h.actions[i]) {
                step_failed_on(
                    &mut h.actions[i],
                    s("daemon_error"),
                    s("cannot resolve workspace id after register"),
                    None,
                );
            }
        }
    }
    if let Some(i) = pending_at(h, ca) {
        let color = act::get(&h.actions[i], "color").cloned().unwrap_or(Value::Null);
        let resp = h.post_json(
            "/cli/projects/update",
            &obj(vec![("id", workspace_id.clone()), ("color", color)]),
        );
        if ptrue(&resp, "error", &[(1021, "<module>")])? {
            let hint = hint_of(&resp, 1023)?;
            step_failed_on(
                &mut h.actions[i],
                s("daemon_error"),
                s(&format!("color write failed: {}", py::str_of(&hint))),
                None,
            );
        } else {
            act::set(&mut h.actions[i], "status", s("done"));
            h.changed = true;
        }
    }
    if let Some(i) = pending_at(h, fa) {
        let group = act::get(&h.actions[i], "focusGroupId").cloned().unwrap_or(Value::Null);
        let resp = h.post_json(
            "/cli/focus-groups/assign",
            &obj(vec![("projectId", workspace_id.clone()), ("focusGroupId", group)]),
        );
        if ptrue(&resp, "error", &[(1033, "<module>")])? {
            let hint = hint_of(&resp, 1035)?;
            step_failed_on(
                &mut h.actions[i],
                s("daemon_error"),
                s(&format!("focus-group assign failed: {}", py::str_of(&hint))),
                None,
            );
        } else {
            act::set(&mut h.actions[i], "status", s("done"));
        }
    }

    // 6: one connection add per peer.
    for i in h.all("connect") {
        if !act::pending(&h.actions[i]) {
            continue;
        }
        let peer = act::get(&h.actions[i], "peer").map(py::str_of).unwrap_or_default();
        let target = or(act::get(&h.actions[i], "resolved"), s(&peer));
        let resp = h.post_query(
            "/cli/connections",
            vec![
                ("action", "add".to_string()),
                ("project", path.clone()),
                ("target", py::str_of(&target)),
            ],
            super::http::DEFAULT,
        );
        let err = pget(&resp, "error", &[(1053, "<module>")])?.cloned();
        if let Some(err) = err.filter(py::truthy) {
            let (code, hint) = match &err {
                Value::Object(_) => (
                    act::get(&err, "code").cloned().unwrap_or(Value::Null),
                    act::get(&err, "hint").cloned().unwrap_or(Value::Null),
                ),
                other => (Value::Null, s(&py::str_of(other))),
            };
            let exit = ops::py_eq(&code, &s("agents_create_connections_disabled")).then_some(3);
            step_failed_on(
                &mut h.actions[i],
                or(Some(&code), s("daemon_error")),
                s(&format!("connect {peer} failed: {}", py::str_of(&hint))),
                exit,
            );
            continue;
        }
        if act::get(&resp, "noop").is_some_and(py::truthy) {
            act::set(&mut h.actions[i], "status", s("skipped"));
            act::set(&mut h.actions[i], "reason", s("edge exists"));
            continue;
        }
        for k in ["bindWarning", "warning"] {
            if let Some(w) = act::get(&resp, k).filter(|w| py::truthy(w)) {
                act::set(&mut h.actions[i], "warn", w.clone());
            }
        }
        act::set(&mut h.actions[i], "status", s("done"));
        h.changed = true;
    }

    // 6b: project membership.
    for i in h.all("join-project") {
        if !act::pending(&h.actions[i]) {
            continue;
        }
        let group = act::get(&h.actions[i], "projectId").cloned().unwrap_or(Value::Null);
        let resp = h.post_json(
            "/cli/project-group/add-member",
            &obj(vec![("group", group), ("workspace", s(&path))]),
        );
        let err = pget(&resp, "error", &[(1077, "<module>")])?.cloned();
        if err.as_ref().is_some_and(py::truthy) || !act::get(&resp, "ok").is_some_and(py::truthy) {
            let hint = hint_of(&resp, 1079)?;
            let project = act::get(&h.actions[i], "project").map(py::str_of).unwrap_or_default();
            step_failed_on(
                &mut h.actions[i],
                s("daemon_error"),
                s(&format!(
                    "join project {project} failed: {}",
                    py::str_of(&hint)
                )),
                None,
            );
            continue;
        }
        let a = &mut h.actions[i];
        if act::get(&resp, "alreadyMember").is_some_and(py::truthy) {
            act::set(a, "status", s("skipped"));
            act::set(a, "reason", s("already a member"));
            act::pop(a, "becamePoc");
        } else {
            act::set(a, "status", s("done"));
            if act::get(&resp, "becamePoc").is_some_and(py::truthy) {
                act::set(a, "becamePoc", Value::Bool(true));
            } else {
                act::pop(a, "becamePoc");
            }
            h.changed = true;
        }
    }

    // 6c: context layers.
    for i in h.all("seed-context") {
        seed_context(h, i, &path)?;
    }

    // 7: the launch.
    if let Some(i) = pending_at(h, h.one("launch")) {
        let resp = h.post_query(
            "/cli/workspace/ensure-canonical-session",
            vec![("project", path.clone())],
            120,
        );
        let bad = ptrue(&resp, "error", &[(1159, "<module>")])? || !ptrue(&resp, "success", &[(1159, "<module>")])?;
        if bad {
            let why = get_or_dump(&resp, "error");
            step_failed_on(
                &mut h.actions[i],
                s("daemon_error"),
                s(&format!("launch failed: {}", py::str_of(&why))),
                None,
            );
        } else {
            let a = &mut h.actions[i];
            act::set(a, "status", s("done"));
            act::set(
                a,
                "sessionId",
                act::get(&resp, "session_id").cloned().unwrap_or(Value::Null),
            );
            if act::get(&resp, "reused").is_some_and(py::truthy) {
                act::set(a, "reused", Value::Bool(true));
            } else {
                h.changed = true;
            }
        }
    }

    // 7b: the template's hello.
    if py::truthy(&template_hello) || hello_by_daemon {
        let mut ha = obj(vec![("step", s("hello")), ("status", s("pending"))]);
        let la = h.one("launch");
        if hello_by_daemon {
            act::set(&mut ha, "status", s("done"));
            act::set(
                &mut ha,
                "reason",
                s(&format!("sent by {} when it started the agent", product())),
            );
            h.changed = true;
        } else if la.is_none() {
            act::set(&mut ha, "status", s("skipped"));
            act::set(
                &mut ha,
                "reason",
                s("not launched: it goes the first time the agent starts"),
            );
        } else if la.is_some_and(|i| act::is(&h.actions[i], "status", "failed")) {
            act::set(&mut ha, "status", s("not-run"));
            act::set(&mut ha, "reason", s("launch failed"));
        } else {
            let resp = h.post_query("/cli/workspace/msg", msg_params(&path, &template_hello), 180);
            let e = pget(&resp, "error", &[(1191, "<module>")])?.cloned();
            let str_err = matches!(&e, Some(v @ Value::Str(_)) if py::truthy(v));
            if str_err || !act::get(&resp, "success").is_some_and(py::truthy) {
                let why = e
                    .filter(|v| py::truthy(v))
                    .or_else(|| act::get(&resp, "message").filter(|v| py::truthy(v)).cloned())
                    .unwrap_or_else(|| s(&dumps_head(&resp, 300)));
                step_failed_on(
                    &mut ha,
                    s("daemon_error"),
                    s(&format!("hello delivery failed: {}", py::str_of(&why))),
                    None,
                );
            } else {
                act::set(&mut ha, "status", s("done"));
                h.changed = true;
            }
        }
        let at = h
            .actions
            .iter()
            .position(|x| matches!(act::step(x).as_str(), "onboard" | "take-app"))
            .unwrap_or(h.actions.len());
        h.actions.insert(at, ha);
    }

    // 8: the onboard message.
    let la = h.one("launch");
    if let Some(i) = pending_at(h, h.one("onboard")) {
        if onboard_by_daemon {
            act::set(&mut h.actions[i], "status", s("done"));
            act::set(
                &mut h.actions[i],
                "reason",
                s(&format!("sent by {} after the hello", product())),
            );
            h.changed = true;
        } else if la.is_some_and(|l| act::is(&h.actions[l], "status", "failed")) {
            act::set(&mut h.actions[i], "status", s("not-run"));
            act::set(&mut h.actions[i], "reason", s("launch failed"));
        } else {
            let resp = h.post_query("/cli/workspace/msg", msg_params(&path, &s(&a.onboard)), 180);
            let e = pget(&resp, "error", &[(1216, "<module>")])?.cloned();
            match e {
                Some(v @ Value::Str(_)) if py::truthy(&v) => step_failed_on(
                    &mut h.actions[i],
                    s("daemon_error"),
                    s(&format!("onboard delivery failed: {}", py::str_of(&v))),
                    None,
                ),
                _ if !act::get(&resp, "success").is_some_and(py::truthy) => {
                    let why = or(act::get(&resp, "message"), s(&dumps_head(&resp, 300)));
                    step_failed_on(
                        &mut h.actions[i],
                        s("daemon_error"),
                        s(&format!("onboard delivery failed: {}", py::str_of(&why))),
                        None,
                    );
                }
                _ => {
                    act::set(&mut h.actions[i], "status", s("done"));
                    h.changed = true;
                }
            }
        }
    }

    // 9: the App's handover to an agent that was there already.
    if let Some(i) = pending_at(h, h.one("take-app")) {
        let mut body = obj(vec![("app", s(&h.take_app)), ("to", s(&path))]);
        if let Some(b) = a.brief.as_ref().filter(|b| !b.code_points().is_empty()) {
            act::set(&mut body, "brief", Value::Str(b.clone()));
        }
        let resp = h.post_json("/cli/app/handover", &body);
        match pget(&resp, "error", &[(1233, "<module>")])?.cloned() {
            Some(e @ Value::Object(_)) => {
                act::set(&mut h.actions[i], "handover", obj(vec![("error", e.clone())]));
                let code = or(act::get(&e, "code"), s("daemon_error"));
                let hint = or(act::get(&e, "hint"), s("the handover was not filed"));
                step_failed_on(&mut h.actions[i], code, hint, None);
            }
            _ => {
                let hv = or(act::get(&resp, "handover"), obj(vec![]));
                let r = act::get(&resp, "request").cloned().unwrap_or(Value::Null);
                let fr = [(1239, "<module>")];
                let id = pget(&hv, "id", &fr)?.cloned().unwrap_or(Value::Null);
                let state = pget(&hv, "state", &fr)?.cloned().unwrap_or(Value::Null);
                let request = match &r {
                    Value::Object(_) => act::get(&r, "id").cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                act::set(
                    &mut h.actions[i],
                    "handover",
                    obj(vec![("id", id), ("state", state), ("request", request)]),
                );
                act::set(&mut h.actions[i], "status", s("done"));
            }
        }
        h.changed = true;
    }

    let changed = h.changed;
    finish(h, changed, 1244)
}

/// `/cli/workspace/msg`'s query, as the hello and the onboard message
/// send it.
fn msg_params(path: &str, text: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("workspace", path.to_string()),
        ("text", py::str_of(text)),
        ("from", "owner".to_string()),
        ("wake", "true".to_string()),
        ("wake_timeout", "120".to_string()),
    ]
}

/// One `--context` layer (lines 1097 to 1152).
fn seed_context(h: &mut Hire, i: usize, path: &str) -> R<()> {
    let cli = cli();
    if !act::pending(&h.actions[i]) {
        return Ok(());
    }
    let a = h.actions[i].clone();
    let context = act::get(&a, "context").cloned().unwrap_or(Value::Null);
    let catalog = act::get(&a, "kind").is_some_and(|k| ops::py_eq(k, &s("catalog")));
    let mut body = obj(vec![("project", s(path))]);
    if catalog {
        act::set(&mut body, "catalog", or(act::get(&a, "catalog"), context.clone()));
    } else {
        act::set(&mut body, "path", or(act::get(&a, "path"), context.clone()));
    }
    let resp = h.post_json("/cli/context/add", &body);
    let err = pget(&resp, "error", &[(1106, "<module>")])?.cloned();
    let ctx = py::str_of(&context);
    if let Some(err) = err.filter(py::truthy) {
        let err = match err {
            Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
            other => other,
        };
        let fr = [(1110, "<module>")];
        let code_v = or(pget(&err, "code", &fr)?, s("error"));
        let code = super::lower(&code_v, &fr)?;
        let hint = act::get(&err, "hint")
            .filter(|v| py::truthy(v))
            .or_else(|| act::get(&err, "message").filter(|v| py::truthy(v)))
            .cloned()
            .unwrap_or_else(|| s(&code));
        let hint_s = py::str_of(&hint);
        let path_kind = act::get(&a, "kind").is_some_and(|k| ops::py_eq(k, &s("path")));
        let x = &mut h.actions[i];
        if code == "duplicate_layer" {
            act::set(x, "status", s("skipped"));
            act::set(x, "reason", s("already stacked"));
        } else if code == "catalog_unknown" {
            step_failed_on(
                x,
                s("catalog_unknown"),
                s(&format!(
                    "unknown context catalog id '{ctx}': {hint_s} \u{2014} see: {cli} agent context catalog"
                )),
                None,
            );
        } else if path_kind {
            let c = if matches!(code.as_str(), "not_found" | "path_escape" | "bad_usage") {
                code.clone()
            } else {
                "daemon_error".to_string()
            };
            step_failed_on(
                x,
                s(&c),
                s(&format!("context seed failed for {ctx}: {hint_s}")),
                None,
            );
        } else {
            h.stderr.push_str(&format!(
                "warning: context seed skipped for catalog id '{ctx}': {hint_s}\n"
            ));
            let x = &mut h.actions[i];
            act::set(x, "status", s("skipped"));
            act::set(x, "reason", hint.clone());
            act::set(x, "warn", hint);
        }
        return Ok(());
    }
    let ok = act::get(&resp, "ok").is_some_and(py::truthy);
    if !ok && act::get(&resp, "layer").is_none() {
        let hint = dumps_head(&resp, 300);
        if act::get(&a, "kind").is_some_and(|k| ops::py_eq(k, &s("path"))) {
            step_failed_on(
                &mut h.actions[i],
                s("daemon_error"),
                s(&format!("context seed failed: {hint}")),
                None,
            );
            return Ok(());
        }
        h.stderr.push_str(&format!(
            "warning: context seed unexpected response for '{ctx}': {hint}\n"
        ));
        let x = &mut h.actions[i];
        act::set(x, "status", s("skipped"));
        act::set(x, "reason", s("unexpected response"));
        return Ok(());
    }
    let layer = or(act::get(&resp, "layer"), obj(vec![]));
    let fr = [(1143, "<module>")];
    let id = pget(&layer, "id", &fr)?.cloned().unwrap_or(Value::Null);
    let lpath = act::get(&layer, "path").filter(|v| py::truthy(v)).cloned();
    let x = &mut h.actions[i];
    act::set(x, "layerId", id);
    let new_path = lpath
        .clone()
        .unwrap_or_else(|| act::get(&a, "path").cloned().unwrap_or(Value::Null));
    act::set(x, "path", new_path);
    let exists = act::get(&layer, "exists").cloned();
    if catalog && matches!(exists, Some(Value::Bool(false))) {
        let shown = lpath.unwrap_or(context);
        let warn = format!(
            "catalog pack file missing on disk: {} (layer stacked; fill file or regen later)",
            py::str_of(&shown)
        );
        h.stderr.push_str(&format!("warning: {warn}\n"));
        act::set(&mut h.actions[i], "warn", s(&warn));
    }
    act::set(&mut h.actions[i], "status", s("done"));
    h.changed = true;
    Ok(())
}
