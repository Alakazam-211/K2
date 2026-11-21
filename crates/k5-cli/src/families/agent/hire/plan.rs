//! The block from the folder to the plan (`<stdin>` lines 136 to 604),
//! and the dry run (lines 780 to 817).

use super::super::{abs_user, cli, expanduser, obj, product, py_strip, s};
use super::color::normalize;
use super::{act, dumps_head, exit_of, lower, or, pget, ptrue, raise, Frame, Hire, R};
use crate::exit::Exit;
use crate::families::py_iter;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::{ops, path as pypath};

/// `core_error(resp)`, called from `from`.
pub fn core_error(resp: &Value, from: u32) -> Result<(Value, Value), crate::families::Raise> {
    let err = pget(resp, "error", &[(from, "<module>"), (123, "core_error")])?.cloned();
    let (code, hint) = match &err {
        Some(e @ Value::Object(_)) => (
            act::get(e, "code").cloned().unwrap_or(Value::Null),
            act::get(e, "hint").cloned().unwrap_or(Value::Null),
        ),
        _ => (
            s("daemon_error"),
            or(err.as_ref(), s(&dumps_head(resp, 300))),
        ),
    };
    Ok((or(Some(&code), s("daemon_error")), hint))
}

/// `(code or "").lower()` at `fr`, as `template_exit` and
/// `exit_code_for` read it.
fn code_word(code: &Value, fr: &[Frame]) -> Result<String, crate::families::Raise> {
    if py::truthy(code) {
        lower(code, fr)
    } else {
        Ok(String::new())
    }
}

/// `template_exit(code)`, called from `fr` (its line is 129).
pub fn template_exit(code: &Value, fr: &[Frame]) -> Result<i64, crate::families::Raise> {
    let mut f = fr.to_vec();
    f.push((129, "template_exit"));
    Ok(match code_word(code, &f)?.as_str() {
        "usage" | "bad_template" => 2,
        "unknown_template" | "not_found" => 4,
        "template_conflict" | "folder_exists" | "owner_only" | "system_template" | "hire_off"
        | "main_only" => 3,
        _ => 1,
    })
}

/// `exit_code_for(code)`, called from `fr` (its line is 734).
pub fn exit_code_for(code: &Value, fr: &[Frame]) -> Result<i64, crate::families::Raise> {
    let mut f = fr.to_vec();
    f.push((734, "exit_code_for"));
    Ok(match code_word(code, &f)?.as_str() {
        "bad_usage" | "path_escape" | "catalog_unknown" | "usage" => 2,
        "duplicate_layer" | "refused" | "precondition" | "owner_only" | "hire_off"
        | "main_only" => 3,
        "not_found" | "ambiguous" => 4,
        _ => 1,
    })
}

/// `str.split()`'s first word.
fn first_word(t: &str) -> Option<String> {
    t.split(|c: char| py_strip(&c.to_string()).is_empty())
        .find(|w| !w.is_empty())
        .map(str::to_string)
}

/// `s.strip()` on a value that must be a string.
fn strip_of(v: &Value, fr: &[Frame]) -> Result<String, crate::families::Raise> {
    match v {
        Value::Str(_) => Ok(py_strip(&py::str_of(v)).to_string()),
        other => Err(raise(
            fr,
            format!(
                "AttributeError: '{}' object has no attribute '{}'",
                py::type_name(other),
                "replace"
            ),
        )),
    }
}

/// Members of a set the block keeps (`x in set`, `set.add(x)`).
#[derive(Default)]
pub struct Set(Vec<Value>);

impl Set {
    fn add(&mut self, v: Value, fr: &[Frame]) -> Result<(), crate::families::Raise> {
        if matches!(v, Value::Array(_) | Value::Object(_)) {
            return Err(raise(
                fr,
                format!("TypeError: unhashable type: '{}'", py::type_name(&v)),
            ));
        }
        if !self.has(&v) {
            self.0.push(v);
        }
        Ok(())
    }

    fn has(&self, v: &Value) -> bool {
        self.0.iter().any(|x| ops::py_eq(x, v))
    }

    fn has_str(&self, t: &str) -> bool {
        self.has(&s(t))
    }
}

/// `", ".join(sorted(v))` for the names the block lists.
fn sorted_join(mut v: Vec<Value>) -> String {
    v.sort_by(|a, b| match (a, b) {
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        _ => py::str_of(a).cmp(&py::str_of(b)),
    });
    v.iter().map(py::str_of).collect::<Vec<_>>().join(", ")
}

/// A catalog id's source tag (the block's `src_guess`).
fn src_guess(ctx: &str) -> Option<&'static str> {
    Some(match ctx {
        "wiki:index" => "catalog:wiki-index",
        "wiki:home" => "catalog:wiki-home",
        "wiki:hygiene" | "wiki-hygiene" | "hygiene" => "catalog:wiki-hygiene",
        "subagents:pack" | "subagents" | "always-use-subagents" | "use-subagents" => {
            "catalog:subagents"
        }
        "browser:pack" | "browser" | "browser-and-computer-use" | "catalog:browser" => {
            "catalog:browser"
        }
        "manager:pack" | "manager" | "workspace-manager" => "catalog:manager",
        "k2:pack" | "kessel:pack" | "kessel" | "k2-agent" | "k2" => "catalog:kessel",
        "connections:roster" | "connections" | "roster" => "catalog:connections-roster",
        "heartbeats:roster" | "heartbeats" => "catalog:heartbeats-roster",
        "skills:roster" | "skills" | "skills-index" => "catalog:skills-roster",
        "users:roster"
        | "users"
        | "users-roster"
        | "people"
        | "people-roster"
        | "user-roster"
        | "catalog:users-roster" => "catalog:users-roster",
        _ => return None,
    })
}

/// `context_is_path(token)`.
fn context_is_path(t: &str) -> bool {
    t.contains('/')
        || t.starts_with('.')
        || t.ends_with(".md")
        || t.ends_with(".markdown")
        || t.starts_with('~')
}

/// `str.lstrip("./")`.
fn lstrip_dot_slash(t: &str) -> &str {
    t.trim_start_matches(['.', '/'])
}

fn pending(step: &str) -> Value {
    obj(vec![("step", s(step)), ("status", s("pending"))])
}

fn skipped(step: &str, reason: &str) -> Value {
    obj(vec![
        ("step", s(step)),
        ("status", s("skipped")),
        ("reason", s(reason)),
    ])
}

/// Lines 136 to 604: the folder, the look-ups, the conf and the plan.
pub fn plan(h: &mut Hire) -> R<()> {
    let cli = cli();
    let a = h.a;
    // 136: the folder.
    if !a.dir.is_empty() {
        h.path = abs_user(&a.dir);
    } else {
        let probe = h.post_json(
            "/cli/agents/hire",
            &h.template_body(vec![("dryRun", Value::Bool(true))]),
        )?;
        let err = ptrue(&probe, "error", &[(140, "<module>")])?;
        if err || !ptrue(&probe, "path", &[(140, "<module>")])? {
            h.note_permission(&probe);
            let (code, hint) = core_error(&probe, 142)?;
            let exit = template_exit(&code, &[(143, "<module>")])?;
            return Err(h.fail(code, hint, exit_of(exit)));
        }
        h.path = pget(&probe, "path", &[(144, "<module>")])?
            .map(py::str_of)
            .unwrap_or_default();
    }
    let path = h.path.clone();
    if pypath::exists(&path) && !is_dir(&path) {
        return Err(h.fail(
            s("bad_usage"),
            s(&format!("{path} exists and is not a directory")),
            Exit::Usage,
        ));
    }

    // 181: --agent against the preset roster.
    if !a.agent_preset.is_empty() {
        let presets = h.get("/cli/presets/list", vec![])?;
        let Value::Array(list) = &presets else {
            let err = match &presets {
                Value::Object(_) => act::get(&presets, "error").cloned(),
                _ => None,
            };
            let why = or(err.as_ref(), s("unexpected response"));
            return Err(h.fail(
                s("daemon_error"),
                s(&format!("cannot list agent presets: {}", py::str_of(&why))),
                Exit::Error,
            ));
        };
        let want = py_strip(&a.agent_preset).to_string();
        let fr = [(193, "<module>"), (189, "preset_matches")];
        let mut hits = Vec::new();
        for p in list {
            let command = or(pget(p, "command", &fr)?, s(""));
            let first = match &command {
                Value::Str(_) => first_word(&py::str_of(&command)),
                other => {
                    return Err(raise(
                        &fr,
                        format!(
                            "AttributeError: '{}' object has no attribute 'split'",
                            py::type_name(other)
                        ),
                    )
                    .into())
                }
            };
            let fr2 = [(193, "<module>"), (190, "preset_matches")];
            let by_id = pget(p, "id", &fr2)?.is_some_and(|v| ops::py_eq(v, &s(&want)));
            let hit = by_id || {
                let fr3 = [(193, "<module>"), (191, "preset_matches")];
                let label = or(pget(p, "label", &fr3)?, s(""));
                lower(&label, &fr3)? == want.to_lowercase()
                    || first.as_deref() == Some(want.as_str())
            };
            if hit {
                hits.push(p.clone());
            }
        }
        if hits.is_empty() {
            return Err(h.fail(
                s("not_found"),
                s(&format!(
                    "no agent preset matches '{want}' \u{2014} run `{cli} preset list` (id, label, or command token)"
                )),
                Exit::NotFound,
            ));
        }
        let mut ids = Set::default();
        if hits.len() > 1 {
            for p in &hits {
                ids.add(
                    act::get(p, "id").cloned().unwrap_or(Value::Null),
                    &[(198, "<module>")],
                )?;
            }
        }
        if hits.len() > 1 && ids.0.len() > 1 {
            let names = hits.iter().map(|p| or(act::get(p, "id"), s("?"))).collect();
            return Err(h.fail(
                s("bad_usage"),
                s(&format!(
                    "'{want}' is ambiguous across presets: {} \u{2014} pass the preset id",
                    sorted_join(names)
                )),
                Exit::Usage,
            ));
        }
        let first = hits.swap_remove(0);
        if !act::get(&first, "enabled").is_some_and(py::truthy) {
            let id = act::get(&first, "id").cloned().unwrap_or(Value::Null);
            return Err(h.fail(
                s("bad_usage"),
                s(&format!(
                    "preset '{want}' is disabled \u{2014} enable it first: {cli} preset set {} --enable",
                    py::str_of(&id)
                )),
                Exit::Usage,
            ));
        }
        h.resolved_preset = Some(first);
    }

    // 213: --project selectors.
    let mut resolved_projects: Vec<(Value, Value, bool)> = Vec::new();
    for sel in &a.projects {
        let d = h.get("/cli/project-group/show", vec![("group", sel.clone())])?;
        let err = pget(&d, "error", &[(215, "<module>")])?.cloned();
        if let Some(err) = err.filter(py::truthy) {
            let err = match err {
                Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
                other => other,
            };
            let mut code = pget(&err, "code", &[(219, "<module>")])?
                .cloned()
                .unwrap_or_else(|| s("error"));
            if ops::py_eq(&code, &s("ambiguous_id")) {
                code = s("ambiguous");
            }
            let hint = act::get(&err, "hint").cloned();
            if ops::py_eq(&code, &s("not_found")) || ops::py_eq(&code, &s("ambiguous")) {
                let hint = or(
                    hint.as_ref(),
                    s(&format!(
                        "no project matches '{sel}' \u{2014} see: {cli} project list"
                    )),
                );
                return Err(h.fail(code, hint, Exit::NotFound));
            }
            let why = or(hint.as_ref(), code);
            return Err(h.fail(
                s("daemon_error"),
                s(&format!(
                    "cannot resolve project '{sel}': {}",
                    py::str_of(&why)
                )),
                Exit::Error,
            ));
        }
        if !ptrue(&d, "ok", &[(227, "<module>")])? || !ptrue(&d, "id", &[(227, "<module>")])? {
            return Err(h.fail(
                s("daemon_error"),
                s(&format!(
                    "unexpected response resolving project '{sel}': {}",
                    dumps_head(&d, 200)
                )),
                Exit::Error,
            ));
        }
        let id = act::get(&d, "id").cloned().unwrap_or(Value::Null);
        if resolved_projects
            .iter()
            .all(|(p, _, _)| !ops::py_eq(p, &id))
        {
            let name = or(act::get(&d, "name"), s(sel));
            let memberless = !act::get(&d, "members").is_some_and(py::truthy);
            resolved_projects.push((id, name, memberless));
        }
    }

    // 234: --color.
    let mut wanted_color = None;
    if !a.color.is_empty() {
        wanted_color = normalize(&a.color);
        if wanted_color.is_none() {
            return Err(h.fail(
                s("bad_usage"),
                s(&format!(
                    "invalid --color '{}' \u{2014} use #rrggbb, #rgb, rgb(r,g,b), or r,g,b",
                    a.color
                )),
                Exit::Usage,
            ));
        }
    }

    // 241: --focus-group.
    let mut wanted_focus: Option<(Value, Value)> = None;
    if !a.focus_group.is_empty() {
        let sel = py_strip(&a.focus_group).to_string();
        let low = sel.to_lowercase();
        if matches!(low.as_str(), "none" | "off" | "-") {
            wanted_focus = Some((s(""), s("none")));
        } else {
            let groups = h.get("/cli/focus-groups/list", vec![])?;
            let Value::Array(list) = &groups else {
                let err = match &groups {
                    Value::Object(_) => act::get(&groups, "error").cloned(),
                    _ => None,
                };
                let why = or(err.as_ref(), s("unexpected response"));
                return Err(h.fail(
                    s("daemon_error"),
                    s(&format!("cannot list focus groups: {}", py::str_of(&why))),
                    Exit::Error,
                ));
            };
            let mut hits = Vec::new();
            for g in list {
                let fr = [(256, "<module>"), (254, "group_matches")];
                let by_id = pget(g, "id", &fr)?.is_some_and(|v| ops::py_eq(v, &s(&sel)));
                let hit = by_id || {
                    let fr = [(256, "<module>"), (255, "group_matches")];
                    let name = or(pget(g, "name", &fr)?, s(""));
                    lower(&name, &fr)? == low
                };
                if hit {
                    hits.push(g.clone());
                }
            }
            if hits.is_empty() {
                let mut prefixes = Vec::new();
                for g in list {
                    let fr = [(259, "<module>")];
                    let name = or(pget(g, "name", &fr)?, s(""));
                    if lower(&name, &fr)?.starts_with(&low) {
                        prefixes.push(g.clone());
                    }
                }
                if prefixes.len() == 1 {
                    hits = prefixes;
                }
            }
            if hits.is_empty() {
                return Err(h.fail(
                    s("not_found"),
                    s(&format!(
                        "no focus group matches '{sel}' \u{2014} create one in Settings \u{2192} Workspaces / Agents"
                    )),
                    Exit::NotFound,
                ));
            }
            if hits.len() > 1 {
                let names = hits
                    .iter()
                    .map(|g| {
                        or(
                            act::get(g, "name")
                                .filter(|v| py::truthy(v))
                                .or(act::get(g, "id")),
                            s("?"),
                        )
                    })
                    .collect();
                return Err(h.fail(
                    s("ambiguous"),
                    s(&format!(
                        "'{sel}' matches multiple focus groups: {} \u{2014} pass the id",
                        sorted_join(names)
                    )),
                    Exit::NotFound,
                ));
            }
            let id = or(act::get(&hits[0], "id"), s(""));
            let label = or(act::get(&hits[0], "name"), s(&sel));
            wanted_focus = Some((id, label));
        }
    }

    // 273: the conf (what makes hire idempotent).
    let conf = h.get(super::super::CONF, vec![("q", path.clone())])?;
    let probe_err = pget(&conf, "error", &[(274, "<module>")])?.cloned();
    if let Some(e) = probe_err.filter(py::truthy) {
        let not_found = matches!(&e, Value::Object(_))
            && act::get(&e, "code").is_some_and(|c| ops::py_eq(c, &s("not_found")));
        if !not_found {
            let hint = match &e {
                Value::Object(_) => act::get(&e, "hint").cloned().unwrap_or(Value::Null),
                other => s(&py::str_of(other)),
            };
            return Err(h.fail(
                s("daemon_error"),
                s(&format!("cannot probe {path}: {}", py::str_of(&hint))),
                Exit::Error,
            ));
        }
    }
    let registered = ptrue(&conf, "ok", &[(281, "<module>")])?;
    h.registered = registered;
    let dir_exists = is_dir(&path);
    let trimmed = path.trim_end_matches('/');
    let basename = {
        let b = trimmed.rsplit('/').next().unwrap_or("");
        if b.is_empty() {
            "agent".to_string()
        } else {
            b.to_string()
        }
    };
    let mut persona_path = if registered {
        act::get(&conf, "personaPath").cloned()
    } else {
        None
    };
    if !persona_path.as_ref().is_some_and(py::truthy) {
        'dots: for dot in [".k2", ".k2so"] {
            for fname in ["ROLE.md", "AGENT.md"] {
                let cand = pypath::join(&pypath::join(&pypath::join(&path, dot), "agent"), fname);
                if is_file(&cand) {
                    persona_path = Some(s(&cand));
                    break 'dots;
                }
            }
        }
    }
    let persona_exists = persona_path.as_ref().is_some_and(py::truthy);
    if !a.persona.is_empty() {
        match std::fs::read(&a.persona) {
            Ok(bytes) => {
                h.persona_content = Some(universal_newlines(&String::from_utf8_lossy(&bytes)))
            }
            Err(e) => {
                let why = crate::out::pylib::json::os_error(&e, &a.persona).message;
                return Err(h.fail(
                    s("bad_usage"),
                    s(&format!("cannot read persona file {}: {why}", a.persona)),
                    Exit::Usage,
                ));
            }
        }
    }
    let desired_mode = match a.mode.as_str() {
        "" => "custom",
        "k2so" => "k2",
        m => m,
    }
    .to_string();

    let mut existing_peers = Set::default();
    let mut existing_group_ids = Set::default();
    let mut existing_context_paths = Set::default();
    let mut existing_context_sources = Set::default();
    let mut live = false;
    if registered {
        let conns = or(act::get(&conf, "connections"), Value::Array(vec![]));
        for c in py_iter(&conns, 322)? {
            let p = pget(&c, "peer", &[(323, "<module>")])?.cloned();
            if let Some(p) = p.filter(py::truthy) {
                existing_peers.add(s(&lower(&p, &[(325, "<module>")])?), &[])?;
            }
        }
        let projs = or(act::get(&conf, "projects"), Value::Array(vec![]));
        for p in py_iter(&projs, 326)? {
            if ptrue(&p, "id", &[(327, "<module>")])? {
                let id = act::get(&p, "id").cloned().unwrap_or(Value::Null);
                existing_group_ids.add(id, &[(328, "<module>")])?;
            }
        }
        let l = or(act::get(&conf, "live"), obj(vec![]));
        live = ptrue(&l, "active", &[(329, "<module>")])?;
        let layers = h.get("/cli/context/layers", vec![("project", path.clone())])?;
        let has_layers =
            ptrue(&layers, "ok", &[(332, "<module>")])? || act::get(&layers, "layers").is_some();
        if has_layers {
            let list = or(act::get(&layers, "layers"), Value::Array(vec![]));
            for l in py_iter(&list, 333)? {
                let p = or(pget(&l, "path", &[(334, "<module>")])?, s(""));
                let lp = strip_of(&p, &[(334, "<module>")])?;
                let lp = py_strip(&lp.replace('\\', "/")).to_string();
                let lp = lstrip_dot_slash(&lp).to_string();
                if !lp.is_empty() {
                    existing_context_paths.add(s(&lp), &[])?;
                }
                let src = or(pget(&l, "source", &[(337, "<module>")])?, s(""));
                let src = match &src {
                    Value::Str(_) => py_strip(&py::str_of(&src)).to_string(),
                    other => {
                        return Err(raise(
                            &[(337, "<module>")],
                            format!(
                                "AttributeError: '{}' object has no attribute 'strip'",
                                py::type_name(other)
                            ),
                        )
                        .into())
                    }
                };
                if !src.is_empty() {
                    existing_context_sources.add(s(&src), &[])?;
                }
            }
        }
    }

    // 353: the plan.
    let mut actions = Vec::new();
    actions.push(if dir_exists {
        skipped("create-dir", "exists")
    } else {
        pending("create-dir")
    });
    let mut wp = obj(vec![("step", s("write-persona"))]);
    if !h.template.is_empty() {
        act::set(&mut wp, "template", s(&h.template));
    } else if !a.persona.is_empty() {
        act::set(&mut wp, "source", s(&a.persona));
    } else {
        act::set(&mut wp, "source", s("default-scaffold"));
    }
    if persona_exists {
        act::set(&mut wp, "status", s("skipped"));
        act::set(&mut wp, "reason", s("exists"));
    } else {
        act::set(&mut wp, "status", s("pending"));
    }
    actions.push(wp);
    if !h.template.is_empty() {
        actions.push(obj(vec![
            ("step", s("apply-template")),
            ("template", s(&h.template)),
            ("status", s("pending")),
        ]));
    }
    actions.push(if registered {
        skipped("register-workspace", "already-registered")
    } else {
        obj(vec![
            ("step", s("register-workspace")),
            ("status", s("pending")),
            ("wiki", s(if a.no_wiki { "skip" } else { "seed" })),
        ])
    });
    actions.push(if a.no_agents_md {
        skipped("generate-agents-md", "no-agents-md")
    } else if registered {
        skipped("generate-agents-md", "already")
    } else {
        pending("generate-agents-md")
    });
    actions.push(if !a.fanout {
        skipped("fanout", "off")
    } else if registered {
        skipped("fanout", "already")
    } else {
        pending("fanout")
    });
    let conf_mode = act::get(&conf, "mode").cloned().unwrap_or(Value::Null);
    let same_mode = ops::py_eq(&conf_mode, &s(&desired_mode));
    actions.push(if registered && same_mode {
        obj(vec![
            ("step", s("set-mode")),
            ("mode", s(&desired_mode)),
            ("status", s("skipped")),
            ("reason", s(&format!("already {desired_mode}"))),
        ])
    } else {
        obj(vec![
            ("step", s("set-mode")),
            ("mode", s(&desired_mode)),
            ("status", s("pending")),
        ])
    });
    if !a.enabled.is_empty() {
        let want = a.enabled == "true";
        let conf_enabled = act::get(&conf, "enabled").is_some_and(py::truthy);
        actions.push(if registered && conf_enabled == want && same_mode {
            obj(vec![
                ("step", s("set-enabled")),
                ("enabled", Value::Bool(want)),
                ("status", s("skipped")),
                ("reason", s(&format!("already {}", a.enabled))),
            ])
        } else {
            obj(vec![
                ("step", s("set-enabled")),
                ("enabled", Value::Bool(want)),
                ("status", s("pending")),
            ])
        });
    }
    if !a.name.is_empty() {
        let same = act::get(&conf, "name").is_some_and(|n| ops::py_eq(n, &s(&a.name)));
        actions.push(if registered && same {
            obj(vec![
                ("step", s("set-name")),
                ("name", s(&a.name)),
                ("status", s("skipped")),
                ("reason", s(&format!("already named {}", a.name))),
            ])
        } else {
            obj(vec![
                ("step", s("set-name")),
                ("name", s(&a.name)),
                ("status", s("pending")),
            ])
        });
    }
    if let Some(p) = &h.resolved_preset {
        let id = act::get(p, "id").cloned().unwrap_or(Value::Null);
        let label = or(act::get(p, "label"), id.clone());
        actions.push(obj(vec![
            ("step", s("set-default-agent")),
            ("preset", id),
            ("label", label),
            ("status", s("pending")),
        ]));
    }
    if !a.db_access.is_empty() {
        actions.push(obj(vec![
            ("step", s("set-db-access")),
            ("access", s(&a.db_access)),
            ("status", s("pending")),
        ]));
    }
    for peer in &a.connects {
        if existing_peers.has_str(&peer.to_lowercase()) {
            actions.push(obj(vec![
                ("step", s("connect")),
                ("peer", s(peer)),
                ("status", s("skipped")),
                ("reason", s("edge exists")),
                ("bidirectional", Value::Bool(true)),
            ]));
            continue;
        }
        let mut c = obj(vec![
            ("step", s("connect")),
            ("peer", s(peer)),
            ("status", s("pending")),
            ("bidirectional", Value::Bool(true)),
        ]);
        let r = h.get(
            "/cli/connections",
            vec![("action", "resolve".to_string()), ("target", peer.clone())],
        )?;
        let is_dict = matches!(r, Value::Object(_));
        let rerr = if is_dict {
            act::get(&r, "error").cloned()
        } else {
            None
        };
        if rerr.as_ref().is_some_and(py::truthy)
            || !(is_dict && act::get(&r, "ok").is_some_and(py::truthy))
        {
            let rerr = match rerr {
                Some(e @ Value::Object(_)) => e,
                other => {
                    let why = or(other.as_ref(), s(&dumps_head(&r, 200)));
                    obj(vec![
                        ("code", s("daemon_error")),
                        ("hint", s(&py::str_of(&why))),
                    ])
                }
            };
            let code = or(act::get(&rerr, "code"), s("daemon_error"));
            let hint = or(
                act::get(&rerr, "hint"),
                s(&format!("cannot resolve '{peer}'")),
            );
            act::set(&mut c, "status", s("failed"));
            act::set(&mut c, "error", obj(vec![("code", code), ("hint", hint)]));
        } else {
            let kind = act::get(&r, "kind").cloned().unwrap_or(Value::Null);
            let local = ops::py_eq(&kind, &s("local"));
            let rpath = act::get(&r, "path").cloned().unwrap_or(Value::Null);
            if local && ops::py_eq(&rpath, &s(&path)) {
                act::set(&mut c, "status", s("failed"));
                act::set(
                    &mut c,
                    "error",
                    obj(vec![
                        ("code", s("bad_usage")),
                        ("hint", s(&format!("cannot connect {peer} to itself"))),
                    ]),
                );
            } else {
                act::set(&mut c, "kind", kind.clone());
                let resolved = if local {
                    rpath
                } else {
                    act::get(&r, "address").cloned().unwrap_or(Value::Null)
                };
                act::set(&mut c, "resolved", resolved);
                if ops::py_eq(&kind, &s("remote")) {
                    let addr = or(act::get(&r, "address"), s(""));
                    if existing_peers.has_str(&lower(&addr, &[(460, "<module>")])?) {
                        act::set(&mut c, "status", s("skipped"));
                        act::set(&mut c, "reason", s("edge exists"));
                    }
                }
                if let Some(w) = act::get(&r, "warning").filter(|w| py::truthy(w)) {
                    act::set(&mut c, "warn", w.clone());
                }
            }
        }
        actions.push(c);
    }
    for (id, name, memberless) in &resolved_projects {
        let mut j = obj(vec![
            ("step", s("join-project")),
            ("project", name.clone()),
            ("projectId", id.clone()),
        ]);
        if existing_group_ids.has(id) {
            act::set(&mut j, "status", s("skipped"));
            act::set(&mut j, "reason", s("already a member"));
        } else {
            act::set(&mut j, "status", s("pending"));
            if *memberless {
                act::set(&mut j, "becamePoc", Value::Bool(true));
            }
        }
        actions.push(j);
    }
    if let Some(color) = &wanted_color {
        let current = if registered {
            let c = or(act::get(&conf, "color"), s(""));
            lower(&c, &[(481, "<module>")])?
        } else {
            String::new()
        };
        let mut c = obj(vec![("step", s("set-color")), ("color", s(color))]);
        if registered && current == *color {
            act::set(&mut c, "status", s("skipped"));
            act::set(&mut c, "reason", s(&format!("already {color}")));
        } else {
            act::set(&mut c, "status", s("pending"));
        }
        actions.push(c);
    }
    if let Some((id, label)) = &wanted_focus {
        let current = if registered {
            act::get(&conf, "focusGroupId")
                .filter(|v| !matches!(v, Value::Null))
                .cloned()
                .unwrap_or_else(|| s(""))
        } else {
            s("")
        };
        let id_or_none = if py::truthy(id) {
            id.clone()
        } else {
            Value::Null
        };
        let mut f = obj(vec![
            ("step", s("assign-focus-group")),
            ("group", label.clone()),
            ("focusGroupId", id_or_none),
        ]);
        if registered && ops::py_eq(&current, &or(Some(id), s(""))) {
            let l = or(Some(label), s("none"));
            act::set(&mut f, "status", s("skipped"));
            act::set(&mut f, "reason", s(&format!("already {}", py::str_of(&l))));
        } else {
            act::set(&mut f, "status", s("pending"));
        }
        actions.push(f);
    }
    for ctx in &a.contexts {
        let ctx = py_strip(ctx).to_string();
        if ctx.is_empty() {
            return Err(h.fail(
                s("bad_usage"),
                s("--context value must not be empty"),
                Exit::Usage,
            ));
        }
        let mut c = obj(vec![("step", s("seed-context")), ("context", s(&ctx))]);
        if context_is_path(&ctx) {
            act::set(&mut c, "kind", s("path"));
            let user = expanduser(&ctx);
            let abs_ctx = if user.starts_with('/') {
                pypath::abspath(&user)
            } else {
                pypath::abspath(&pypath::join(&path, &ctx))
            };
            act::set(&mut c, "resolvedPath", s(&abs_ctx));
            if !is_file(&abs_ctx) {
                return Err(h.fail(
                    s("not_found"),
                    s(&format!(
                        "context path not found: {ctx} (explicit --context paths must exist at hire time)"
                    )),
                    Exit::NotFound,
                ));
            }
            let rel = pypath::relpath(&abs_ctx, &path).replace('\\', "/");
            if rel.starts_with("..") {
                return Err(h.fail(
                    s("bad_usage"),
                    s(&format!("context path escapes hire workspace: {ctx}")),
                    Exit::Usage,
                ));
            }
            act::set(&mut c, "path", s(&rel));
            if existing_context_paths.has_str(lstrip_dot_slash(&rel))
                || existing_context_paths.has_str(&rel)
            {
                act::set(&mut c, "status", s("skipped"));
                act::set(&mut c, "reason", s("already stacked"));
            } else {
                act::set(&mut c, "status", s("pending"));
            }
        } else {
            act::set(&mut c, "kind", s("catalog"));
            act::set(&mut c, "catalog", s(&ctx));
            match src_guess(&ctx) {
                Some(src) if existing_context_sources.has_str(src) => {
                    act::set(&mut c, "status", s("skipped"));
                    act::set(&mut c, "reason", s("already stacked"));
                }
                _ => act::set(&mut c, "status", s("pending")),
            }
        }
        actions.push(c);
    }
    if a.launch {
        actions.push(if live {
            skipped("launch", "already live")
        } else {
            pending("launch")
        });
    }
    if !a.onboard.is_empty() {
        actions.push(pending("onboard"));
    }
    if !h.take_app.is_empty() {
        actions.push(obj(vec![
            ("step", s("take-app")),
            ("status", s("pending")),
            ("app", s(&h.take_app)),
        ]));
    }
    h.display_name = if !a.name.is_empty() {
        s(&a.name)
    } else {
        let n = if registered {
            act::get(&conf, "name").cloned().unwrap_or(Value::Null)
        } else {
            s(&basename)
        };
        or(Some(&n), s(&basename))
    };
    h.actions = actions;
    h.conf = conf;
    Ok(())
}

/// `open(...).read()` in text mode: `\r\n` and `\r` read as `\n`.
fn universal_newlines(t: &str) -> String {
    t.replace("\r\n", "\n").replace('\r', "\n")
}

/// `os.path.isdir`.
pub fn is_dir(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// `os.path.isfile`.
pub fn is_file(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// `TEMPLATE_PERMS_NOTE`.
pub fn perms_note() -> String {
    format!(
        "suggested switches are not turned on by this {}: a person turns them on (Settings \u{2192} Workspaces / Agents \u{2192} the agent \u{2192} Permissions), or the agent asks on a card",
        product()
    )
}

/// `take_template_rows(a, resp, applied)`, called from module line `from`.
pub fn take_template_rows(h: &mut Hire, i: usize, resp: &Value, applied: bool, from: u32) -> R<()> {
    let fr = |line: u32| [(from, "<module>"), (line, "take_template_rows")];
    let rows = or(pget(resp, "rows", &fr(762))?, Value::Array(vec![]));
    let info = or(act::get(resp, "template"), obj(vec![]));
    let perms = or(act::get(resp, "permissions"), obj(vec![]));
    let qs = or(act::get(resp, "questions"), Value::Array(vec![]));
    let a = &mut h.actions[i];
    act::set(a, "rows", rows.clone());
    act::set(a, "info", info);
    act::set(a, "permissions", perms.clone());
    act::set(a, "questions", qs);
    if let Some(hs @ Value::Object(_)) = act::get(resp, "hireSwitch") {
        act::set(a, "hireSwitch", hs.clone());
    }
    let note = (ptrue(&perms, "suggested", &fr(770))? || ptrue(&perms, "cards", &fr(770))?)
        && !ptrue(&perms, "granted", &fr(770))?;
    if note {
        act::set(a, "permissionsNote", s(&perms_note()));
    }
    let mut busy = false;
    for r in py_iter(&rows, 772).map_err(|mut e| {
        e.frames = fr(772).to_vec();
        e
    })? {
        let st = pget(&r, "status", &fr(772))?
            .cloned()
            .unwrap_or(Value::Null);
        if ops::py_eq(&st, &s("write")) || ops::py_eq(&st, &s("done")) {
            let step = pget(&r, "step", &fr(772))?.cloned().unwrap_or(Value::Null);
            if !(ops::py_eq(&step, &s("folder")) || ops::py_eq(&step, &s("register"))) {
                busy = true;
            }
        }
    }
    let status = match (applied, busy) {
        (true, true) => "done",
        (false, true) => "pending",
        _ => "skipped",
    };
    act::set(a, "status", s(status));
    if status == "skipped" {
        act::set(a, "reason", s("already applied"));
    }
    Ok(())
}

/// Lines 780 to 817: the dry run's question to the hire core, then the
/// plan.
pub fn dry_run(h: &mut Hire) -> R<()> {
    if let Some(i) = h.one("apply-template") {
        let mut pb = h.template_body(vec![("path", s(&h.path)), ("dryRun", Value::Bool(true))]);
        if let Some(p) = &h.resolved_preset {
            act::set(
                &mut pb,
                "preset",
                act::get(p, "id").cloned().unwrap_or(Value::Null),
            );
        }
        let resp = h.post_json("/cli/agents/hire", &pb)?;
        let bad = ptrue(&resp, "error", &[(787, "<module>")])?
            || !ptrue(&resp, "ok", &[(787, "<module>")])?;
        if bad {
            h.note_permission(&resp);
            let (code, hint) = core_error(&resp, 789)?;
            let exit = template_exit(&code, &[(792, "<module>")])?;
            let a = &mut h.actions[i];
            act::set(a, "status", s("failed"));
            act::set(a, "error", obj(vec![("code", code), ("hint", hint)]));
            act::set(a, "exitCode", Value::Int(exit.to_string()));
        } else {
            take_template_rows(h, i, &resp, false, 794)?;
        }
    } else if h.op.token().is_some() {
        let core: Vec<usize> = (0..h.actions.len())
            .filter(|&i| {
                let st = act::step(&h.actions[i]);
                matches!(
                    st.as_str(),
                    "create-dir" | "write-persona" | "register-workspace" | "set-db-access"
                ) && act::pending(&h.actions[i])
            })
            .collect();
        if let Some(&first) = core.first() {
            let resp = h.post_json(
                "/cli/agents/hire",
                &obj(vec![("dryRun", Value::Bool(true))]),
            )?;
            let gerr = match &resp {
                Value::Object(_) => act::get(&resp, "error").cloned(),
                _ => None,
            };
            if let Some(g @ Value::Object(_)) = gerr {
                let code = act::get(&g, "code").cloned().unwrap_or(Value::Null);
                if ops::py_eq(&code, &s("hire_off")) || ops::py_eq(&code, &s("main_only")) {
                    h.note_permission(&resp);
                    let hint = or(act::get(&g, "hint"), code.clone());
                    let a = &mut h.actions[first];
                    act::set(a, "status", s("failed"));
                    act::set(a, "error", obj(vec![("code", code), ("hint", hint)]));
                    act::set(a, "exitCode", Value::Int("3".to_string()));
                    for a in h.actions.iter_mut() {
                        if act::pending(a) {
                            act::set(a, "status", s("not-run"));
                            act::set(a, "reason", s("the hire would be refused"));
                        }
                    }
                }
            }
        }
    }
    let changed = h.actions.iter().any(act::pending);
    super::emit::finish(h, changed, 817)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_the_block_splits_on() {
        assert_eq!(first_word("  claude --x"), Some("claude".to_string()));
        assert_eq!(first_word("\u{1f}a"), Some("a".to_string()));
        assert_eq!(first_word("   "), None);
        assert_eq!(lstrip_dot_slash("./.k2/x.md"), "k2/x.md");
        assert!(context_is_path("~x") && context_is_path("a.markdown"));
        assert!(!context_is_path("wiki:index"));
        assert_eq!(src_guess("k2"), Some("catalog:kessel"));
    }

    #[test]
    fn exits_by_code() {
        assert_eq!(template_exit(&s("Folder_Exists"), &[]).ok(), Some(3));
        assert_eq!(exit_code_for(&s("AMBIGUOUS"), &[]).ok(), Some(4));
        assert_eq!(exit_code_for(&Value::Null, &[]).ok(), Some(1));
        assert!(exit_code_for(&Value::Int("5".into()), &[(1, "<module>")]).is_err());
    }
}
