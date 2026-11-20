//! `zen theme`, `zen sync`, `zen news`, `zen agents-order`, `zen links`
//! and `zen inspect` (`cli/src/garden-access.sh`): each its own word
//! loop, one request, and `_zen_print` or the verb's own printer.

use super::call::Zen;
use super::print::{
    self, compact, g, indented, is, items, or, read, s, t, text_of, zen_print, Env, Out, Read,
};
use super::verbs::{body_of, help, no_home, usage};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// A verb's work: a usage stop, or its requests and printer.
pub type Step = Result<Exit, Exit>;

fn stop(s: Shown) -> Step {
    Ok(s.print())
}

fn unexpected(w: &str) -> Shown {
    usage(format!(
        concat!("Error: unexpected '{}' (see ", cli_name!(), " zen --help)"),
        w
    ))
}

/// `json.dumps(b)` of a built object.
fn dumps(pairs: Vec<(&str, Value)>) -> Vec<u8> {
    let o: Vec<(Text, Value)> = pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect();
    json::to_string(&Value::Object(o), &Options::default())
        .unwrap_or_default()
        .into_bytes()
}

/// `cmd_zen_theme`.
pub fn theme(z: &mut Zen, words: &[&str]) -> Step {
    let verb = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    let (mut as_json, mut garden, mut from, mut clear, mut name) =
        (false, String::new(), String::new(), false, String::new());
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let next = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => as_json = true,
            "--garden" => {
                if next.is_empty() {
                    return stop(usage("Error: --garden needs a Garden name or id"));
                }
                garden = next.into();
                i += 1;
            }
            "--from" => {
                if next.is_empty() {
                    return stop(usage("Error: --from needs a theme name"));
                }
                from = next.into();
                i += 1;
            }
            "--clear" => clear = true,
            _ if w.starts_with("--garden=") => garden = w["--garden=".len()..].into(),
            _ if w == "--home" || w.starts_with("--home=") => return stop(no_home()),
            _ if w.starts_with("--from=") => from = w["--from=".len()..].into(),
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => {
                if !name.is_empty() {
                    return stop(usage(format!(
                        "Error: one theme name, got '{name}' and '{w}'"
                    )));
                }
                name = w.into();
            }
        }
        i += 1;
    }
    let env = Env::default();
    match verb {
        "list" => {
            if !(name.is_empty() && from.is_empty() && !clear) {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen theme list takes only --garden and --json"
                )));
            }
            let q = format!("garden={}", quote(&garden));
            let pairs: Vec<&str> = if garden.is_empty() { vec![] } else { vec![&q] };
            let raw = z.get("/cli/zen/theme/list", &pairs)?;
            Ok(zen_print("theme-list", as_json, &raw, env).print())
        }
        "next" | "prev" => {
            if !(name.is_empty() && from.is_empty() && !clear) {
                return stop(usage(format!(
                    concat!(
                        "Error: ",
                        cli_name!(),
                        " zen theme {} takes only --garden and --json"
                    ),
                    verb
                )));
            }
            let body = body_of(&[("garden", &garden)]);
            let raw = z.session_post(&format!("/cli/zen/theme/{verb}"), &body)?;
            Ok(zen_print("theme-switch", as_json, &raw, env).print())
        }
        "set" => {
            if !from.is_empty() {
                return stop(usage(concat!(
                    "Error: --from is for ",
                    cli_name!(),
                    " zen theme new"
                )));
            }
            if clear {
                if garden.is_empty() || !name.is_empty() {
                    return stop(usage(
                        "Error: --clear takes --garden <name> and no theme name",
                    ));
                }
            } else if name.is_empty() {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen theme set needs a theme name (see ",
                    cli_name!(),
                    " zen theme list)"
                )));
            }
            let mut b = Vec::new();
            if !name.is_empty() {
                b.push(("name", print::jstr(&name)));
            }
            if !garden.is_empty() {
                b.push(("garden", print::jstr(&garden)));
            }
            if clear {
                b.push(("clear", Value::Bool(true)));
            }
            let raw = z.session_post("/cli/zen/theme/set", &dumps(b))?;
            Ok(zen_print("theme-switch", as_json, &raw, env).print())
        }
        "new" => {
            if !(garden.is_empty() && !clear) {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen theme new takes a name, --from and --json"
                )));
            }
            if name.is_empty() {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen theme new needs a name (lower-case letters, digits, - and _)"
                )));
            }
            let body = body_of(&[("name", &name), ("from", &from)]);
            let raw = z.post("/cli/zen/theme/new", &body)?;
            Ok(zen_print("theme-new", as_json, &raw, env).print())
        }
        "" | "help" => Ok(help(false, Exit::Ok)),
        _ => stop(usage(format!(
            concat!(
                "Unknown '",
                cli_name!(),
                " zen theme' subcommand: {} (list, next, prev, set, new)"
            ),
            verb
        ))),
    }
}

/// `cmd_zen_sync`.
pub fn sync(z: &mut Zen, words: &[&str]) -> Step {
    let (mut as_json, mut part) = (false, "");
    let mut pos: Vec<&str> = Vec::new();
    for w in words {
        match *w {
            "--json" => as_json = true,
            "--page" | "--theme" => {
                if !part.is_empty() {
                    return stop(usage(
                        "Error: pass one of --page or --theme (neither: both)",
                    ));
                }
                part = &w[2..];
            }
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => pos.push(w),
        }
    }
    let env = Env::default();
    if pos.is_empty() {
        if !part.is_empty() {
            return stop(usage(concat!(
                "Error: ",
                cli_name!(),
                " zen sync with no Garden takes only --json"
            )));
        }
        let raw = z.get("/cli/zen/sync", &[])?;
        return Ok(zen_print("sync-list", as_json, &raw, env).print());
    }
    if pos.len() != 2 {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen sync <garden> on|off|preview|undo|keep-previous [--page|--theme]"
        )));
    }
    let (garden, act) = (pos[0], pos[1]);
    let both = if part.is_empty() { "both" } else { part };
    let body = match act {
        "on" | "off" => dumps(vec![
            ("garden", print::jstr(garden)),
            ("part", print::jstr(both)),
            ("sync", Value::Bool(act == "on")),
        ]),
        "undo" => {
            if !part.is_empty() {
                return stop(usage(
                    "Error: undo puts back the last change; it takes no --page or --theme",
                ));
            }
            dumps(vec![
                ("garden", print::jstr(garden)),
                ("undo", Value::Bool(true)),
            ])
        }
        "keep-previous" => dumps(vec![
            ("garden", print::jstr(garden)),
            ("keep", print::jstr("previous")),
            ("part", print::jstr(both)),
        ]),
        "preview" => {
            let gq = format!("garden={}", quote(garden));
            let pq = format!("preview={both}");
            let raw = z.get("/cli/zen/get", &[&gq, &pq])?;
            return Ok(zen_print("sync-preview", as_json, &raw, env).print());
        }
        _ => {
            return stop(usage(format!(
                concat!(
                    "Error: unknown '",
                    cli_name!(),
                    " zen sync' action '{}' (on, off, preview, undo, keep-previous)"
                ),
                act
            )))
        }
    };
    let raw = z.session_post("/cli/zen/garden/sync", &body)?;
    Ok(zen_print("sync-change", as_json, &raw, env).print())
}

/// `cmd_zen_news`.
pub fn news(z: &mut Zen, words: &[&str]) -> Step {
    let (mut as_json, mut all) = (false, false);
    let mut pos: Vec<&str> = Vec::new();
    for w in words {
        match *w {
            "--json" => as_json = true,
            "--all" => all = true,
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => pos.push(w),
        }
    }
    let env = Env::default();
    if pos.is_empty() {
        if all {
            return stop(usage(concat!(
                "Error: --all is for ",
                cli_name!(),
                " zen news seen"
            )));
        }
        let raw = z.get("/cli/zen/news", &[])?;
        return Ok(zen_print("news", as_json, &raw, env).print());
    }
    if pos[0] != "seen" {
        return stop(usage(format!(
            concat!(
                "Error: unknown '",
                cli_name!(),
                " zen news' subcommand '{}' (seen)"
            ),
            pos[0]
        )));
    }
    let ids = &pos[1..];
    if all && !ids.is_empty() {
        return stop(usage("Error: pass ids or --all, not both"));
    }
    if !all && ids.is_empty() {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen news seen needs ids (from ",
            cli_name!(),
            " zen news) or --all"
        )));
    }
    let body = if all {
        dumps(vec![("all", Value::Bool(true))])
    } else {
        dumps(vec![(
            "ids",
            Value::Array(ids.iter().map(|i| print::jstr(i)).collect()),
        )])
    };
    let raw = z.session_post("/cli/zen/news/seen", &body)?;
    Ok(zen_print("news-seen", as_json, &raw, env).print())
}

/// `cmd_zen_agents_order`.
pub fn agents_order(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let mut garden = "";
    for w in words {
        match *w {
            "--json" => as_json = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (see ",
                        cli_name!(),
                        " zen agents-order --help)"
                    ),
                    w
                )))
            }
            _ => {
                if !garden.is_empty() {
                    return stop(usage(concat!(
                        "Error: ",
                        cli_name!(),
                        " zen agents-order takes one Garden (see --help)"
                    )));
                }
                garden = w;
            }
        }
    }
    let q = format!("garden={}", quote(garden));
    let pairs: Vec<&str> = if garden.is_empty() { vec![] } else { vec![&q] };
    let raw = z.session_get("/cli/zen/agents/order", &pairs)?;
    Ok(agents_order_print(as_json, &raw).print())
}

/// The program line where `d.get("message")` raises on an answer that is
/// not a dict.
pub const AGENTS_ORDER_GET: u32 = 26;

fn agents_order_print(as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let d = match read(
        &raw,
        concat!(
            "Error: no response from the ",
            product_name!(),
            " daemon on this computer"
        ),
    ) {
        Read::Doc(d) => d,
        Read::Stop(s) => return s,
    };
    let mut o = Out::default();
    let dict = matches!(d, Value::Object(_));
    let err = if dict {
        g(&d, "error").cloned().unwrap_or(Value::Null)
    } else {
        print::jstr("not an object")
    };
    if !matches!(err, Value::Null) {
        if as_json {
            o.o(compact(&d));
        }
        let e = Some(&err);
        if is(e, "zen_not_set_up") {
            o.e(or(g(&d, "message"), "Zen isn't set up on this computer."));
            return o.done(Exit::Refused);
        }
        if is(e, "unknown zen route") {
            o.e(concat!(
                "Error: this computer's ",
                product_name!(),
                " is older than 0.46.4 and keeps no Agents order or pins. Update ",
                product_name!(),
                "."
            ));
            return o.done(Exit::Error);
        }
        if is(e, "zen_local_only") {
            o.e(concat!(
                "Error: ",
                cli_name!(),
                " zen talks only to the ",
                product_name!(),
                " on this computer, and this token is not its owner or a session on it."
            ));
            return o.done(Exit::Refused);
        }
        if !dict {
            return o.raise(&Raise::no_attribute(AGENTS_ORDER_GET, &d, "get"));
        }
        let m = match g(&d, "message") {
            Some(m) if py::truthy(m) => py::str_of(m),
            _ => py::str_of(&err),
        };
        o.e(format!("Error: {m}"));
        return o.done(Exit::Error);
    }
    if as_json {
        o.o(indented(&d));
        return o.done(Exit::Ok);
    }
    let rows = items(g(&d, "gardens"));
    for (i, gd) in rows.iter().enumerate() {
        if i > 0 {
            o.o("");
        }
        o.o(format!("{} ({})", s(g(gd, "name")), s(g(gd, "id"))));
        let order = g(gd, "order");
        let pins = g(gd, "pins");
        o.o(format!(
            "  order: {}",
            if t(order) {
                print::join(order, ", ")
            } else {
                "the widget's own (never dragged)".to_string()
            }
        ));
        if pins.is_none() || matches!(pins, Some(Value::Null)) {
            o.o("  pins:  each agent's workspace pin (never pinned or unpinned here)");
        } else {
            o.o(format!(
                "  pins:  {}",
                if t(pins) {
                    print::join(pins, ", ")
                } else {
                    "none in this Garden".to_string()
                }
            ));
        }
        if t(g(gd, "updatedAt")) {
            o.o(format!("  set:   {}", s(g(gd, "updatedAt"))));
        }
    }
    if !rows.is_empty() {
        o.o("");
    }
    o.o("Only the person changes these, in Zen. Agents can read them, never set them.");
    o.done(Exit::Ok)
}

/// `cmd_zen_links`.
pub fn links(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let mut pos: Vec<&str> = Vec::new();
    for w in words {
        match *w {
            "--json" => as_json = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (see ",
                        cli_name!(),
                        " zen links --help)"
                    ),
                    w
                )))
            }
            _ => pos.push(w),
        }
    }
    let raw = if pos.first() == Some(&"forget") {
        if pos.len() != 3 {
            return stop(usage(concat!(
                "Error: usage: ",
                cli_name!(),
                " zen links forget <garden> <site>"
            )));
        }
        let body = dumps(vec![
            ("garden", print::jstr(pos[1])),
            ("site", print::jstr(pos[2])),
        ]);
        z.session_post("/cli/zen/links/forget", &body)?
    } else if pos.len() <= 1 {
        let q = pos.first().map(|g| format!("garden={}", quote(g)));
        let pairs: Vec<&str> = q.iter().map(String::as_str).collect();
        z.get("/cli/zen/links", &pairs)?
    } else {
        return stop(usage(concat!(
            "Error: usage: ",
            cli_name!(),
            " zen links [<garden>] | ",
            cli_name!(),
            " zen links forget <garden> <site>"
        )));
    };
    Ok(links_print(as_json, &raw).print())
}

/// The program line of `g = d.get("gardens")` in `cmd_zen_links`'s Python.
pub const LINKS_GARDENS: u32 = 24;
/// The line of `d["removed"]`.
pub const LINKS_REMOVED: u32 = 20;

fn links_print(as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let mut o = Out::default();
    let d = match crate::out::pylib::json::loads(&raw) {
        Ok(d) => d,
        Err(_) => {
            o.e(format!(
                "Error: the daemon answered something that is not JSON: {}",
                print::head200(&raw)
            ));
            return o.done(Exit::Error);
        }
    };
    if as_json {
        o.o(compact(&d));
    }
    let err = g(&d, "error");
    if t(err) {
        let e = err.unwrap_or(&Value::Null);
        let msg = match g(&d, "message") {
            Some(m) if py::truthy(m) => py::str_of(m),
            _ => match (e, g(e, "hint")) {
                (Value::Object(_), Some(h)) if py::truthy(h) => py::str_of(h),
                _ => py::str_of(e),
            },
        };
        o.e(format!("Error: {msg}"));
        let code = match e {
            Value::Str(_) => Some(e),
            Value::Object(_) => g(e, "code"),
            _ => None,
        };
        let refused = is(code, "owner_only") || is(code, "zen_local_only");
        return o.done(if refused { Exit::Refused } else { Exit::Error });
    }
    if as_json {
        return o.done(Exit::Ok);
    }
    let has = |k: &str| match &d {
        Value::Object(p) => p.iter().any(|(n, _)| n.as_str() == Some(k)),
        Value::Array(a) => a.iter().any(|v| is(Some(v), k)),
        Value::Str(st) => py::str_of(&Value::Str(st.clone())).contains(k),
        _ => false,
    };
    if has("removed") {
        let get = |k: &str| crate::out::pylib::ops::index(&d, k, LINKS_REMOVED);
        let line = (|| -> Result<String, Raise> {
            let removed = get("removed")?;
            let (site, garden) = (get("site")?, get("garden")?);
            Ok(if py::truthy(removed) {
                format!(
                    concat!("Forgot {} for {}: ", product_name!(), " asks next time."),
                    py::str_of(site),
                    py::str_of(garden)
                )
            } else {
                format!(
                    "{} was not kept for {}.",
                    py::str_of(site),
                    py::str_of(garden)
                )
            })
        })();
        match line {
            Ok(l) => o.o(l),
            Err(r) => return o.raise(&r),
        }
    } else if has("sites") {
        let sites = g(&d, "sites");
        if t(sites) {
            o.o(print::join(sites, "\n"));
        } else {
            o.o(concat!(
                "No sites: ",
                product_name!(),
                " asks the first time for each one."
            ));
        }
    } else {
        if !matches!(d, Value::Object(_)) {
            return o.raise(&Raise::no_attribute(LINKS_GARDENS, &d, "get"));
        }
        let gs = g(&d, "gardens");
        match gs {
            Some(Value::Object(pairs)) if !pairs.is_empty() => {
                let mut keys: Vec<&(Text, Value)> = pairs.iter().collect();
                keys.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in keys {
                    o.o(format!(
                        "{}: {}",
                        py::str_of(&Value::Str(k.clone())),
                        print::join(Some(v), ", ")
                    ));
                }
            }
            _ if t(gs) => {}
            _ => o.o("No Garden opens a site without asking."),
        }
    }
    o.done(Exit::Ok)
}

/// `cmd_zen_inspect`.
pub fn inspect(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let mut target = "";
    for w in words {
        match *w {
            "--json" => as_json = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unknown flag '{}' — usage: ",
                        cli_name!(),
                        " zen inspect <bundle-or-folder> [--json]"
                    ),
                    w
                )))
            }
            _ => {
                if target.is_empty() {
                    target = w;
                } else {
                    return stop(usage(format!("Error: unexpected argument '{w}'")));
                }
            }
        }
    }
    if target.is_empty() {
        return stop(usage(concat!(
            "Error: usage: ",
            cli_name!(),
            " zen inspect <bundle-or-folder> [--json]"
        )));
    }
    let abs = crate::out::pylib::path::abspath(target);
    let q = format!("path={}", quote(&abs));
    let raw = z.get_soft("/cli/zen/inspect", &[&q], false)?;
    Ok(inspect_print(as_json, &raw).print())
}

/// `json.dumps(s if s is not None else "", ensure_ascii=False)`.
fn q(v: Option<&Value>) -> String {
    let v = match v {
        None | Some(Value::Null) => print::jstr(""),
        Some(x) => x.clone(),
    };
    json::to_string(
        &v,
        &Options {
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

fn section(o: &mut Out, title: &str, rows: Vec<String>) {
    o.o(format!("\n{title}:"));
    if rows.is_empty() {
        o.o("  (none)");
    }
    for r in rows {
        o.o(format!("  {r}"));
    }
}

/// `",".join(x or []) or "-"`.
fn joined_or_dash(v: Option<&Value>) -> String {
    let j = print::join(v, ",");
    if j.is_empty() {
        "-".into()
    } else {
        j
    }
}

fn inspect_print(as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let mut o = Out::default();
    if print::blank(&raw) {
        o.e(concat!(
            "Error: no response from the ",
            product_name!(),
            " daemon on this computer"
        ));
        return o.done(Exit::Error);
    }
    let d = match crate::out::pylib::json::loads(&raw) {
        Ok(d) => d,
        Err(_) => {
            o.e("Error: the daemon answered something that is not JSON");
            return o.done(Exit::Error);
        }
    };
    let dict = matches!(d, Value::Object(_));
    if !dict || g(&d, "error").is_some_and(|e| !matches!(e, Value::Null)) {
        if as_json {
            o.o(compact(&d));
        }
        let msg = if dict { g(&d, "message") } else { None };
        let m = match msg {
            Some(m) if py::truthy(m) => py::str_of(m),
            _ => print::head200(&raw),
        };
        o.e(format!("Error: {m}"));
        return o.done(Exit::Error);
    }
    if as_json {
        o.o(indented(&d));
        return o.done(Exit::Ok);
    }
    let empty = Value::Object(Vec::new());
    let obj = |v: Option<&Value>| -> Value {
        match v {
            Some(x) if py::truthy(x) => x.clone(),
            _ => empty.clone(),
        }
    };
    o.o("=== BEGIN UNTRUSTED INSPECT REPORT ===");
    o.o(s(g(&d, "notice")));
    let src = obj(g(&d, "source"));
    let sm = obj(g(&d, "summary"));
    o.o(format!(
        "source: {} {}",
        s(g(&src, "kind")),
        q(g(&src, "path"))
    ));
    let f = |k: &str| s(g(&sm, k));
    o.o(format!(
        "summary: {} files, {} widget(s), {} error(s), {} warning(s), {} verb(s) ({} not open to widgets), {} network, {} obfuscation, {} refused",
        f("files"),
        f("widgets"),
        f("errors"),
        f("warnings"),
        f("verbs"),
        f("verbsNotOpenToWidgets"),
        f("network"),
        f("obfuscation"),
        f("refused")
    ));
    let rows = |k: &str, each: &dyn Fn(&Value) -> String| -> Vec<String> {
        items(g(&d, k)).iter().map(each).collect()
    };
    section(
        &mut o,
        "widgets",
        rows("widgets", &|w| {
            format!(
                "{} id={} name={} caps={} libs={} {}",
                q(g(w, "folder")),
                q(g(w, "id")),
                q(g(w, "name")),
                joined_or_dash(g(w, "caps")),
                joined_or_dash(g(w, "libs")),
                if t(g(w, "ok")) { "ok" } else { "HAS ERRORS" }
            )
        }),
    );
    section(
        &mut o,
        "verbs",
        rows("verbs", &|v| {
            format!(
                "{}{} cap={} calls={} at {}{}",
                q(g(v, "verb")),
                if t(g(v, "known")) { "" } else { " UNKNOWN" },
                or(g(v, "cap"), "-"),
                s(g(v, "calls")),
                print::join(g(v, "at"), ", "),
                if t(g(v, "widget")) {
                    ""
                } else {
                    " (NOT OPEN TO WIDGETS)"
                }
            )
        }),
    );
    let members = g(&d, "k2Members");
    section(
        &mut o,
        "k2 object members",
        if t(members) {
            vec![print::join(members, ", ")]
        } else {
            vec![]
        },
    );
    let kind_rows = |k: &str| -> Vec<String> {
        items(g(&d, k))
            .iter()
            .map(|h| {
                format!(
                    "{} {} at {}",
                    s(g(h, "kind")),
                    q(g(h, "detail")),
                    q(g(h, "at"))
                )
            })
            .collect()
    };
    section(&mut o, "libraries", kind_rows("libraries"));
    section(&mut o, "network", kind_rows("network"));
    section(
        &mut o,
        "urls",
        rows("urls", &|h| {
            format!("{} at {}", q(g(h, "detail")), q(g(h, "at")))
        }),
    );
    section(&mut o, "obfuscation", kind_rows("obfuscation"));
    let fd = obj(g(&d, "findings"));
    let finding = |e: &Value| {
        format!(
            "{}:{}:{}: {} | suggestion: {}",
            s(g(e, "file")),
            s(g(e, "line")),
            s(g(e, "col")),
            q(g(e, "message")),
            or(g(e, "suggestion"), "-")
        )
    };
    section(
        &mut o,
        "errors",
        items(g(&fd, "errors")).iter().map(finding).collect(),
    );
    section(
        &mut o,
        "warnings",
        items(g(&fd, "warnings")).iter().map(finding).collect(),
    );
    section(
        &mut o,
        "refused",
        rows("refused", &|r| {
            format!("{}: {}", q(g(r, "path")), s(g(r, "why")))
        }),
    );
    let cut: Vec<String> = match obj(g(&d, "truncated")) {
        Value::Object(p) => p
            .iter()
            .filter(|(_, v)| py::truthy(v))
            .map(|(k, _)| py::str_of(&Value::Str(k.clone())))
            .collect(),
        _ => Vec::new(),
    };
    if !cut.is_empty() {
        o.o(format!(
            "\ntruncated lists: {} (use --json for the capped lists)",
            cut.join(", ")
        ));
    }
    o.o("=== END UNTRUSTED INSPECT REPORT ===");
    o.done(Exit::Ok)
}
