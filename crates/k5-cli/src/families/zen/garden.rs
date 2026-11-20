//! `zen garden …` (`cmd_zen_garden`, `cli/src/garden-access.sh`), with
//! Garden access (`access`, `history`: `_zen_access_print`), `garden put`
//! (`cli/src/garden-agents.sh`) and `zen proposals`
//! (`cli/src/zen-retired.sh`), whose answers `_zen_agent_print` prints
//! ([`agent_print`], also the widget dev loop's printer).

use super::call::Zen;
use super::print::{
    self, compact, diag, g, human, indented, int, is, items, or, read, s, t, text_of,
    zen_print, Env, Out, Read,
};
use super::verbs::{dir_body, help, read_text, usage, NameKey};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

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

fn dumps(pairs: Vec<(&str, Value)>) -> Vec<u8> {
    let o: Vec<(Text, Value)> = pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect();
    json::to_string(&Value::Object(o), &Options::default())
        .unwrap_or_default()
        .into_bytes()
}

const NO_ANSWER: &str = concat!(
    "Error: no response from the ",
    product_name!(),
    " daemon on this computer"
);

/// `_zen_access_on`: this server's K2 has Garden access. Against this
/// computer's daemon, only when it wrote `garden-access-on` in the K2
/// home; then (or for another host) the probe's answer says.
pub fn access_on(z: &mut Zen) -> Result<bool, Exit> {
    let local = matches!(z.settings.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if local && !z.settings.home.join("garden-access-on").exists() {
        return Ok(false);
    }
    let raw = z.get_soft("/cli/zen/garden/access", &[], true)?;
    let text = String::from_utf8_lossy(&raw);
    Ok(!(text.is_empty() || text.contains("\"unknown zen route\"")))
}

/// Program lines of `_zen_access_print`'s Python where a view's first
/// `.get` raises on an answer that is not a dict.
pub const ACCESS_LINES: &[(&str, u32)] =
    &[("list", 31), ("change", 45), ("ask", 52), ("history", 54)];

/// `when(t)`.
fn when(v: Option<&Value>) -> String {
    match v {
        Some(Value::Int(d)) => d
            .parse::<i64>()
            .ok()
            .and_then(|n| crate::out::pylib::time::strftime_local(n, "%Y-%m-%d %H:%M").ok())
            .unwrap_or_else(|| "-".into()),
        Some(Value::Bool(b)) => {
            crate::out::pylib::time::strftime_local(i64::from(*b), "%Y-%m-%d %H:%M")
                .unwrap_or_else(|_| "-".into())
        }
        _ => "-".into(),
    }
}

/// `_zen_access_print VIEW AS_JSON RAW`.
pub fn access_print(view: &str, as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let d = match read(&raw, NO_ANSWER) {
        Read::Doc(d) => d,
        Read::Stop(s) => return s,
    };
    let mut o = Out::default();
    let dict = matches!(d, Value::Object(_));
    let err = g(&d, "error");
    if as_json {
        o.o(indented(&d));
    }
    if is(err, "unknown zen route") {
        o.e(concat!(
            "Error: this server's ",
            product_name!(),
            " is older than 0.46.3, so it has no Garden access lists. Update ",
            product_name!(),
            " on that server."
        ));
        return o.done(Exit::Error);
    }
    if t(err) {
        let e = err.unwrap_or(&Value::Null);
        if !as_json {
            o.e(format!(
                "Error: {}",
                match g(&d, "message") {
                    Some(m) if py::truthy(m) => py::str_of(m),
                    _ => py::str_of(e),
                }
            ));
        }
        let any = |ws: &[&str]| ws.iter().any(|w| is(err, w));
        if any(&[
            "garden_access_needed",
            "garden_delete_person",
            "owner_only",
            "zen_local_only",
            "session_gone",
            "zen_not_set_up",
        ]) {
            return o.done(Exit::Refused);
        }
        if any(&["unknown_garden", "unknown_agent", "not_found", "unknown_widget"]) {
            return o.done(Exit::NotFound);
        }
        return o.done(Exit::Error);
    }
    if as_json {
        return o.done(Exit::Ok);
    }
    if !dict {
        return match ACCESS_LINES.iter().find(|(v, _)| *v == view) {
            Some((_, at)) => o.raise(&Raise::no_attribute(*at, &d, "get")),
            None => o.done(Exit::Ok),
        };
    }
    let empty = Value::Object(Vec::new());
    match view {
        "list" => {
            let has = matches!(&d, Value::Object(p) if p.iter().any(|(k, _)| k.as_str() == Some("editors")));
            if !has {
                o.o(format!(
                    "You {} edit {}.",
                    if t(g(&d, "listed")) { "can" } else { "can't" },
                    s(g(&d, "name"))
                ));
                if !t(g(&d, "listed")) {
                    o.o(format!(
                        concat!(
                            "Ask the person: ",
                            cli_name!(),
                            " zen garden access \"{}\" ask"
                        ),
                        s(g(&d, "name"))
                    ));
                }
                return o.done(Exit::Ok);
            }
            let m = g(&d, "made");
            o.o(format!(
                "{}{}{}",
                s(g(&d, "name")),
                if t(g(&d, "locked")) { "  (locked)" } else { "" },
                if t(m) {
                    format!("  made by {}", s(m.and_then(|m| g(m, "handle"))))
                } else {
                    String::new()
                }
            ));
            let eds = items(g(&d, "editors"));
            if eds.is_empty() {
                o.o("No agent may edit it (only you).");
            }
            for e in eds {
                let by = g(&e, "addedBy");
                let how = if is(by, "owner") {
                    "added by you".to_string()
                } else if is(by, "card") {
                    "allowed on a card".to_string()
                } else if is(by, "migration") {
                    "added automatically at upgrade".to_string()
                } else {
                    s(by)
                };
                o.o(format!(
                    "  {} {how} · {}",
                    crate::families::ljust(&s(g(&e, "handle")), 24),
                    when(g(&e, "addedAt"))
                ));
            }
            o.o("Sees: everything you can see");
        }
        "change" => {
            let a = s(g(print_obj(g(&d, "agent"), &empty), "handle"));
            let gn = s(g(print_obj(g(&d, "garden"), &empty), "name"));
            let changed = t(g(&d, "changed"));
            if t(g(&d, "listed")) {
                o.o(format!(
                    "{a} may edit {gn}{}.",
                    if changed { "" } else { " (already)" }
                ));
            } else {
                o.o(format!(
                    "{a} no longer edits {gn}{}.",
                    if changed { "" } else { " (wasn't on the list)" }
                ));
            }
        }
        "ask" => o.o(or(g(&d, "message"), "")),
        "history" => {
            let list = items(g(&d, "items"));
            for u in items(g(&d, "unsigned")) {
                o.o(format!(
                    concat!(
                        "not signed: {} (version {}) runs without your data until you or an editor signs it (",
                        cli_name!(),
                        " zen widget sign {})"
                    ),
                    s(g(&u, "widget")),
                    s(g(&u, "hash")),
                    s(g(&u, "widget"))
                ));
            }
            if list.is_empty() {
                o.o("No history yet.");
            }
            for i in list {
                let a = print_obj(g(&i, "actor"), &empty);
                let who = match g(a, "name") {
                    Some(n) if py::truthy(n) => py::str_of(n),
                    _ => s(g(a, "kind")),
                };
                let what = g(&i, "kind");
                let det = print_obj(g(&i, "detail"), &empty);
                let mut extra = String::new();
                if t(g(&i, "widget")) {
                    extra = format!(" {}", s(g(&i, "widget")));
                }
                if g(det, "to").is_some_and(|v| !matches!(v, Value::Null)) && is(what, "renamed") {
                    extra.push_str(&format!(" → {}", s(g(det, "to"))));
                }
                if t(g(det, "workspace")) {
                    extra.push_str(&format!(" {}", s(g(det, "workspace"))));
                }
                o.o(format!(
                    "{}  {} {who}{extra}",
                    when(g(&i, "at")),
                    crate::families::ljust(&s(what), 14)
                ));
            }
            if t(g(&d, "next")) {
                o.o(format!("More: --before {}", s(g(&d, "next"))));
            }
        }
        _ => {}
    }
    o.done(Exit::Ok)
}

/// `x or {}`.
fn print_obj<'a>(v: Option<&'a Value>, empty: &'a Value) -> &'a Value {
    match v {
        Some(x) if py::truthy(x) => x,
        _ => empty,
    }
}

/// Program lines of `_zen_agent_print`'s Python where a view's first
/// `.get` raises on an answer that is not a dict.
pub const AGENT_LINES: &[(&str, u32)] = &[
    ("put", 54),
    ("delete", 67),
    ("folder", 70),
    ("preview", 100),
    ("list", 105),
    ("show", 119),
    ("decide", 134),
];

/// `findings(v)`.
fn findings(o: &mut Out, v: &Value) {
    for e in items(g(v, "errors")) {
        o.o(diag(&e, false));
    }
    for w in items(g(v, "warnings")) {
        o.o(diag(&w, true));
    }
    if let Some(sz @ Value::Object(_)) = g(v, "size") {
        if let Some(c @ (Value::Int(_) | Value::Bool(_))) = g(sz, "code") {
            let code = int(Some(c));
            let warn = g(sz, "codeWarnAt").map_or(code, |w| int(Some(w)));
            let large = if code > warn { " (large)" } else { "" };
            let libs = human(int(g(sz, "libs")))
                + if t(g(sz, "libsUnknown")) {
                    " + CDN files"
                } else {
                    ""
                };
            o.o(format!(
                "size: code {} of {}{large}, page {} of {}, libraries {libs} (outside the limit)",
                human(code),
                human(int(g(sz, "codeMax"))),
                human(int(g(sz, "page"))),
                human(int(g(sz, "pageMax")))
            ));
        }
    }
}

const KEEP_AS_MINE: &str = "This widget was your person’s; its new version is not theirs yet, so it runs without its data until they keep it. ";

fn keep_line(d: &Value, empty: &Value) -> String {
    let kc = print_obj(g(d, "keepCard"), empty);
    let tail = if t(g(kc, "card")) {
        concat!(
            product_name!(),
            " put a Keep as mine card on their Thread with you."
        )
    } else {
        concat!(
            product_name!(),
            " sent them an inbox note (Settings → Gardens → the Garden → Local Data → It’s mine)."
        )
    };
    format!("{KEEP_AS_MINE}{tail}")
}

/// `_zen_agent_print VIEW AS_JSON RAW`.
pub fn agent_print(view: &str, as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let d = match read(&raw, NO_ANSWER) {
        Read::Doc(d) => d,
        Read::Stop(s) => return s,
    };
    let mut o = Out::default();
    let dict = matches!(d, Value::Object(_));
    let err = g(&d, "error");
    if as_json {
        o.o(indented(&d));
    }
    if t(err) {
        if !as_json {
            o.e(format!(
                "Error: {}: {}",
                s(err),
                or(g(&d, "message"), "")
            ));
        }
        let any = |ws: &[&str]| ws.iter().any(|w| is(err, w));
        if any(&[
            "person_only",
            "zen_local_only",
            "garden_locked",
            "owner_only",
            "zen_not_set_up",
            "garden_access_needed",
            "garden_delete_person",
            "session_gone",
        ]) {
            return o.done(Exit::Refused);
        }
        if any(&[
            "proposal_not_found",
            "unknown_garden",
            "unknown_widget",
            "not_found",
        ]) {
            return o.done(Exit::NotFound);
        }
        return o.done(Exit::Error);
    }
    if as_json {
        return o.done(Exit::Ok);
    }
    if !dict {
        return match AGENT_LINES.iter().find(|(v, _)| *v == view) {
            Some((_, at)) => o.raise(&Raise::no_attribute(*at, &d, "get")),
            None => o.done(Exit::Ok),
        };
    }
    let empty = Value::Object(Vec::new());
    match view {
        "put" => {
            if t(g(&d, "proposed")) {
                let p = print_obj(g(&d, "proposal"), &empty);
                o.o(format!(
                    "Proposed {} ({} {}): nothing written yet.",
                    s(g(p, "id")),
                    s(g(p, "kind")),
                    s(g(p, "target"))
                ));
                o.o(or(g(&d, "message"), ""));
            } else {
                let changed = g(&d, "changed").map_or(true, py::truthy);
                o.o(format!(
                    "Wrote {}{}",
                    s(g(&d, "file")),
                    if changed { "" } else { " (no change)" }
                ));
                if t(g(&d, "keepAsMine")) {
                    o.o(keep_line(&d, &empty));
                }
                if t(g(&d, "snapshot")) {
                    let sn = s(g(&d, "snapshot"));
                    o.o(format!(
                        concat!(
                            "Previous version kept: {} (",
                            cli_name!(),
                            " zen reset --to {})"
                        ),
                        sn, sn
                    ));
                }
                findings(&mut o, print_obj(g(&d, "validate"), &empty));
            }
        }
        "delete" => {
            o.o(format!(
                "Deleted {} (kept in history as {}).",
                s(g(&d, "file")),
                s(g(&d, "snapshot"))
            ));
            o.o(format!("Undo: {}", s(g(&d, "undo"))));
        }
        "folder" => {
            if t(g(&d, "proposed")) {
                for p in items(g(&d, "proposals")) {
                    if is(g(&p, "kind"), "folder") {
                        o.o(format!(
                            "Proposed {} (widgets/{}/: {}): nothing written yet.",
                            s(g(&p, "id")),
                            s(g(&p, "target")),
                            print::join(g(&p, "files"), ", ")
                        ));
                    } else {
                        o.o(format!(
                            "Proposed {} (widgets/{}/{})",
                            s(g(&p, "id")),
                            s(g(&p, "target")),
                            s(g(&p, "file"))
                        ));
                    }
                }
                o.o(or(g(&d, "message"), ""));
            } else if matches!(g(&d, "proposed"), Some(Value::Bool(false))) {
                o.o(format!(
                    "No change: widgets/{} matches the folder.",
                    s(g(&d, "widget"))
                ));
            } else {
                let w = g(&d, "written");
                o.o(format!(
                    "Wrote {}: {}",
                    s(g(&d, "file")),
                    if t(w) {
                        print::join(w, ", ")
                    } else {
                        "no change".to_string()
                    }
                ));
                if t(g(&d, "keepAsMine")) {
                    o.o(keep_line(&d, &empty));
                }
                if t(g(&d, "removed")) {
                    o.o(format!("Removed: {}", print::join(g(&d, "removed"), ", ")));
                }
                if t(g(&d, "kept")) {
                    o.o(format!(
                        "Left alone (not in the folder; --prune removes them): {}",
                        print::join(g(&d, "kept"), ", ")
                    ));
                }
                let v = print_obj(g(&d, "validate"), &empty);
                findings(&mut o, v);
                if let Some(rt @ Value::Object(_)) = g(v, "runtime") {
                    if t(g(rt, "note")) {
                        o.o(format!("runtime: {}", s(g(rt, "note"))));
                    }
                }
            }
        }
        "preview" => {
            let has = |k: &str| matches!(&d, Value::Object(p) if p.iter().any(|(n, _)| n.as_str() == Some(k)));
            if has("removed") && !has("preview") {
                let removed = if t(g(&d, "removed")) {
                    let ws: Vec<String> = items(g(&d, "removed"))
                        .iter()
                        .map(|r| format!("widgets/{}", py::str_of(r)))
                        .collect();
                    format!(" and {}", ws.join(", "))
                } else {
                    String::new()
                };
                o.o(format!(
                    "Cleared the preview{}{removed}",
                    if t(g(&d, "garden")) { " Garden" } else { "" }
                ));
            } else {
                let gd = print_obj(g(&d, "garden"), &empty);
                o.o(format!(
                    "Previewing {} as widgets/{} on the \"{}\" Garden ({})",
                    s(g(&d, "widget")),
                    s(g(&d, "preview")),
                    s(g(gd, "name")),
                    s(g(gd, "id"))
                ));
                findings(&mut o, print_obj(g(&d, "validate"), &empty));
                o.o(or(g(&d, "open"), ""));
            }
        }
        "list" => {
            let rows = items(g(&d, "proposals"));
            if rows.is_empty() {
                o.o("No Garden proposals waiting.");
                return o.done(Exit::Ok);
            }
            for p in rows {
                let what = if is(g(&p, "kind"), "folder") {
                    format!(
                        "widgets/{}/ ({} files)",
                        s(g(&p, "target")),
                        items(g(&p, "files")).len()
                    )
                } else if t(g(&p, "file")) {
                    format!("widgets/{}/{}", s(g(&p, "target")), s(g(&p, "file")))
                } else {
                    format!("gardens/{}.toml", s(g(&p, "target")))
                };
                let note = if t(g(&p, "note")) {
                    format!("  — {}", s(g(&p, "note")))
                } else {
                    String::new()
                };
                o.o(format!(
                    "{}  {} {} by {}  {} B{note}",
                    s(g(&p, "id")),
                    crate::families::ljust(&s(g(&p, "status")), 9),
                    crate::families::ljust(&what, 36),
                    s(g(&p, "madeBy")),
                    s(g(&p, "bytes"))
                ));
            }
            o.o("");
            o.o(concat!(
                "Review: ",
                cli_name!(),
                " zen proposals show <id> · approve <id> · reject <id>"
            ));
        }
        "show" => {
            let p = print_obj(g(&d, "proposal"), &empty);
            if is(g(p, "kind"), "folder") {
                let stale = items(g(p, "staleFiles"));
                let st = if stale.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> = stale.iter().map(py::str_of).collect();
                    format!("  (STALE: changed since: {})", names.join(", "))
                };
                o.o(format!(
                    "{}  {}  {}  by {}{st}",
                    s(g(p, "id")),
                    s(g(p, "status")),
                    s(g(p, "path")),
                    s(g(p, "madeBy"))
                ));
                if t(g(p, "note")) {
                    o.o(format!("note: {}", s(g(p, "note"))));
                }
                let unchanged = g(p, "unchanged").map_or_else(|| "0".to_string(), py::str_of);
                o.o(format!(
                    "One proposal for the folder: approving writes every file below at once ({unchanged} other staged file(s) stay as they are)."
                ));
                for f in items(g(p, "content")) {
                    o.o(format!(
                        "--- {} ({}) ---",
                        s(g(&f, "name")),
                        if t(g(&f, "base")) {
                            "changed"
                        } else {
                            "new file"
                        }
                    ));
                    o.o(or(g(&f, "text"), ""));
                }
            } else {
                o.o(format!(
                    "{}  {}  {}  by {}{}",
                    s(g(p, "id")),
                    s(g(p, "status")),
                    s(g(p, "path")),
                    s(g(p, "madeBy")),
                    if t(g(p, "stale")) {
                        "  (STALE: the file changed since)"
                    } else {
                        ""
                    }
                ));
                if t(g(p, "note")) {
                    o.o(format!("note: {}", s(g(p, "note"))));
                }
                o.o("--- proposed content ---");
                o.o(or(g(p, "content"), ""));
            }
        }
        "decide" => {
            o.o(format!("{}: {}", s(g(&d, "id")), s(g(&d, "status"))));
            if t(g(&d, "keepAsMine")) {
                let kc = print_obj(g(&d, "keepCard"), &empty);
                if t(g(kc, "card")) {
                    o.o(format!(
                        "The widget’s new version is not signed as yours yet: tap Keep as mine on the card in your Thread with {}.",
                        or(g(kc, "addr"), "the agent")
                    ));
                } else {
                    o.o(format!(
                        concat!(
                            "The widget’s new version is not signed as yours, so it runs without its data. Keep it: Settings → Gardens → the Garden → Local Data → {} → It’s mine (or ",
                            cli_name!(),
                            " zen widget claim {} outside ",
                            product_name!(),
                            ")."
                        ),
                        s(g(&d, "target")),
                        s(g(&d, "target"))
                    ));
                }
            }
        }
        _ => {}
    }
    o.done(Exit::Ok)
}

/// `cmd_zen_garden_access`.
fn access(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let mut reason = String::new();
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => as_json = true,
            "--reason" => {
                let next = words.get(i + 1).copied().unwrap_or("");
                if next.is_empty() {
                    return stop(usage("Error: --reason needs words"));
                }
                reason = next.into();
                i += 1;
            }
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => pos.push(w),
        }
        i += 1;
    }
    const USE: &str = concat!(
        "Error: ",
        cli_name!(),
        " zen garden access <garden> [add|remove <agent> | ask]"
    );
    if pos.is_empty() {
        return stop(usage(USE));
    }
    let garden = pos[0];
    let act = pos.get(1).copied().unwrap_or("");
    match act {
        "" => {
            if pos.len() != 1 {
                return stop(usage(USE));
            }
            let q = format!("garden={}", quote(garden));
            let raw = z.session_get("/cli/zen/garden/access", &[&q])?;
            Ok(access_print("list", as_json, &raw).print())
        }
        "add" | "remove" => {
            if pos.len() != 3 {
                return stop(usage(format!(
                    concat!("Error: ", cli_name!(), " zen garden access <garden> {} <agent>"),
                    act
                )));
            }
            let body = dumps(vec![
                ("garden", print::jstr(garden)),
                ("agent", print::jstr(pos[2])),
                ("allow", Value::Bool(act == "add")),
            ]);
            let raw = z.session_post("/cli/zen/garden/access", &body)?;
            Ok(access_print("change", as_json, &raw).print())
        }
        "ask" => {
            if pos.len() != 2 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen garden access <garden> ask [--reason \"…\"]"
                )));
            }
            let mut b = vec![("garden", print::jstr(garden))];
            if !reason.is_empty() {
                b.push(("reason", print::jstr(&reason)));
            }
            let raw = z.session_post("/cli/zen/garden/access/ask", &dumps(b))?;
            Ok(access_print("ask", as_json, &raw).print())
        }
        _ => stop(usage(format!(
            concat!(
                "Error: unknown '{}': ",
                cli_name!(),
                " zen garden access <garden> [add|remove <agent> | ask]"
            ),
            act
        ))),
    }
}

/// `cmd_zen_access_history WHAT …` (`garden` or `widget`).
pub fn history(z: &mut Zen, what: &str, words: &[&str]) -> Step {
    let mut as_json = false;
    let (mut kind, mut limit, mut before) = (String::new(), String::new(), String::new());
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => as_json = true,
            "--kind" => {
                if next.is_empty() {
                    return stop(usage(
                        "Error: --kind needs kinds (changes, access, asks or event kinds)",
                    ));
                }
                kind = next.into();
                i += 1;
            }
            "--limit" => {
                if next.is_empty() || !next.bytes().all(|b| b.is_ascii_digit()) {
                    return stop(usage("Error: --limit needs a number (1-200)"));
                }
                limit = next.into();
                i += 1;
            }
            "--before" => {
                if next.is_empty() {
                    return stop(usage("Error: --before needs the cursor History printed"));
                }
                before = next.into();
                i += 1;
            }
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => pos.push(w),
        }
        i += 1;
    }
    if pos.len() != 1 {
        return stop(usage(format!(
            concat!(
                "Error: ",
                cli_name!(),
                " zen {} history <{}> [--kind …] [--limit n] [--before cursor] [--json]"
            ),
            what, what
        )));
    }
    let mut q = vec![format!("{what}={}", quote(pos[0]))];
    if !kind.is_empty() {
        q.push(format!("kind={}", quote(&kind)));
    }
    if !limit.is_empty() {
        q.push(format!("limit={limit}"));
    }
    if !before.is_empty() {
        q.push(format!("before={}", quote(&before)));
    }
    let q: Vec<&str> = q.iter().map(String::as_str).collect();
    let raw = z.get(&format!("/cli/zen/{what}/history"), &q)?;
    Ok(access_print("history", as_json, &raw).print())
}

/// `cmd_zen_garden_put`.
fn put(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let (mut from, mut note, mut garden) = (String::new(), String::new(), String::new());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => as_json = true,
            "--from" => {
                let next = words.get(i + 1).copied().unwrap_or("");
                if next.is_empty() {
                    return stop(usage("Error: --from needs a file (or - for stdin)"));
                }
                from = next.into();
                i += 1;
            }
            "--note" => {
                // `note="${2:-}"; shift 2`: a failed shift ends the command.
                if i + 1 >= words.len() {
                    return Err(Exit::Error);
                }
                note = words[i + 1].into();
                i += 1;
            }
            "-" => from = "-".into(),
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => {
                if !garden.is_empty() {
                    return stop(usage(concat!(
                        "Error: ",
                        cli_name!(),
                        " zen garden put <garden> --from <file>|-"
                    )));
                }
                garden = w.into();
            }
        }
        i += 1;
    }
    if garden.is_empty() || from.is_empty() {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen garden put <garden> --from <file>|- [--note \"…\"]"
        )));
    }
    let text = match read_text(&from) {
        Ok(t) => t,
        Err(s) => return stop(s),
    };
    let mut b = vec![
        ("garden", print::jstr(&garden)),
        ("text", print::jstr(&text)),
    ];
    if !note.is_empty() {
        b.push(("note", print::jstr(&note)));
    }
    let raw = z.session_post("/cli/zen/garden/put", &dumps(b))?;
    Ok(agent_print("put", as_json, &raw).print())
}

/// `cmd_zen_garden VERB …`.
pub fn garden(z: &mut Zen, words: &[&str]) -> Step {
    let verb = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    if verb == "put" {
        return put(z, rest);
    }
    if (verb == "access" || verb == "history") && access_on(z)? {
        return if verb == "access" {
            access(z, rest)
        } else {
            history(z, "garden", rest)
        };
    }
    let (mut as_json, mut texting, mut force) = (false, false, false);
    let (mut at, mut new_from) = (String::new(), String::new());
    let mut pos: Vec<&str> = Vec::new();
    let mut new_widgets: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let next = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => as_json = true,
            "--from" => {
                if next.is_empty() {
                    return stop(usage(
                        "Error: --from needs the first page (a Garden .toml file, or - for stdin)",
                    ));
                }
                new_from = next.into();
                i += 1;
            }
            "--widgets" => {
                if next.is_empty() {
                    return stop(usage("Error: --widgets needs a staged widget folder"));
                }
                i += 1;
                while i < rest.len() && !rest[i].starts_with('-') {
                    new_widgets.push(rest[i]);
                    i += 1;
                }
                continue;
            }
            "--texting" => texting = true,
            "--force" => force = true,
            "--at" => {
                if next.is_empty() {
                    return stop(usage("Error: --at needs a position (1 is the first)"));
                }
                at = next.into();
                i += 1;
            }
            _ if w.starts_with("--at=") => at = w["--at=".len()..].into(),
            "--" => {
                pos.extend(rest[i + 1..].iter().copied());
                break;
            }
            _ if w.starts_with('-') => return stop(unexpected(w)),
            _ => pos.push(w),
        }
        i += 1;
    }
    let digits = |v: &str| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit());
    if !at.is_empty() && !digits(&at) {
        return stop(usage("Error: --at needs a whole number (1 is the first)"));
    }
    if verb != "new" && (texting || !at.is_empty() || !new_from.is_empty() || !new_widgets.is_empty())
    {
        return stop(usage(concat!(
            "Error: --texting, --at, --from and --widgets are for ",
            cli_name!(),
            " zen garden new"
        )));
    }
    if verb != "template" && force {
        return stop(usage(concat!(
            "Error: --force is for ",
            cli_name!(),
            " zen garden template"
        )));
    }
    let n = pos.len();
    let env = Env::default();
    let one = |what: &str| -> Shown {
        usage(format!(
            concat!(
                "Error: ",
                cli_name!(),
                " zen garden {} needs one Garden (an id or a name)"
            ),
            what
        ))
    };
    match verb {
        "list" => {
            if n != 0 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen garden list takes only --json"
                )));
            }
            let raw = z.get("/cli/zen/gardens", &[])?;
            let sync = z.get_soft("/cli/zen/sync", &[], true)?;
            let sync = String::from_utf8_lossy(&sync).into_owned();
            Ok(zen_print(
                "garden-list",
                as_json,
                &raw,
                Env {
                    strict: false,
                    sync_raw: Some(&sync),
                },
            )
            .print())
        }
        "new" => {
            if n == 0 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen garden new needs a name"
                )));
            }
            let name = pos.join(" ");
            let v2 = !new_from.is_empty()
                || !new_widgets.is_empty()
                || (z.settings.session && access_on(z)?);
            if v2 {
                if !access_on(z)? {
                    return stop(usage(concat!(
                        "Error: --from and --widgets need ",
                        product_name!(),
                        " 0.46.3 or newer on this server; this server's ",
                        product_name!(),
                        " is older. Update ",
                        product_name!(),
                        " on that server."
                    )));
                }
                let body = match new_v2_body(&name, texting, &at, &new_from, &new_widgets) {
                    Ok(b) => b,
                    Err(s) => return stop(s),
                };
                let raw = z.post_file("/cli/zen/garden/new", &body)?;
                return Ok(zen_print("garden-new", as_json, &raw, env).print());
            }
            let mut b = vec![("name", print::jstr(&name))];
            if texting {
                b.push(("template", print::jstr("texting")));
            }
            if !at.is_empty() {
                b.push(("at", int_value(&at)));
            }
            let raw = z.post("/cli/zen/garden/new", &dumps(b))?;
            Ok(zen_print("garden-new", as_json, &raw, env).print())
        }
        "rename" => {
            if n < 2 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen garden rename needs a Garden and a new name"
                )));
            }
            let body = dumps(vec![
                ("garden", print::jstr(pos[0])),
                ("name", print::jstr(&pos[1..].join(" "))),
            ]);
            let raw = z.session_post("/cli/zen/garden/rename", &body)?;
            Ok(zen_print("garden-rename", as_json, &raw, env).print())
        }
        "reorder" => {
            if n != 2 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen garden reorder needs a Garden and a position (1 is the first)"
                )));
            }
            if !digits(pos[1]) {
                return stop(usage(
                    "Error: the position must be a whole number (1 is the first)",
                ));
            }
            let body = dumps(vec![
                ("garden", print::jstr(pos[0])),
                ("to", int_value(pos[1])),
            ]);
            let raw = z.session_post("/cli/zen/garden/reorder", &body)?;
            Ok(zen_print("garden-reorder", as_json, &raw, env).print())
        }
        "template" => {
            if n != 2 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen garden template needs a Garden and a template: texting (the default) or blank"
                )));
            }
            if !matches!(pos[1], "texting" | "blank") {
                return stop(usage(format!(
                    "Error: unknown template '{}'; use texting (the default) or blank",
                    pos[1]
                )));
            }
            let mut b = vec![
                ("garden", print::jstr(pos[0])),
                ("template", print::jstr(pos[1])),
            ];
            if force {
                b.push(("force", Value::Bool(true)));
            }
            let raw = z.session_post("/cli/zen/garden/template", &dumps(b))?;
            Ok(zen_print("garden-template", as_json, &raw, env).print())
        }
        "delete" => {
            if n != 1 {
                return stop(one("delete"));
            }
            let body = dumps(vec![("garden", print::jstr(pos[0]))]);
            let raw = z.session_post("/cli/zen/garden/delete", &body)?;
            Ok(zen_print("garden-delete", as_json, &raw, env).print())
        }
        "lock" | "unlock" => {
            if n != 1 {
                return stop(one(verb));
            }
            let body = dumps(vec![
                ("garden", print::jstr(pos[0])),
                ("locked", Value::Bool(verb == "lock")),
            ]);
            let raw = z.session_post("/cli/zen/garden/lock", &body)?;
            Ok(zen_print("garden-lock", as_json, &raw, env).print())
        }
        "" | "help" => Ok(help(false, Exit::Ok)),
        _ => {
            let long = verb != "access" && verb != "history" && access_on(z)?;
            let line = if long {
                format!(
                    concat!(
                        "Unknown '",
                        cli_name!(),
                        " zen garden' subcommand: {} (list, new, rename, reorder, template, delete, lock, unlock, access, history)"
                    ),
                    verb
                )
            } else {
                format!(
                    concat!(
                        "Unknown '",
                        cli_name!(),
                        " zen garden' subcommand: {} (list, new, rename, reorder, template, delete, lock, unlock)"
                    ),
                    verb
                )
            };
            stop(usage(line))
        }
    }
}

/// `int(word)` of a word of digits, as JSON.
fn int_value(digits: &str) -> Value {
    let t = digits.trim_start_matches('0');
    Value::Int(if t.is_empty() { "0".into() } else { t.into() })
}

/// `_zen_garden_new_v2`'s body: the name, `template`, `at`, the first
/// page's text and each staged widget folder's files.
fn new_v2_body(
    name: &str,
    texting: bool,
    at: &str,
    from: &str,
    widgets: &[&str],
) -> Result<Vec<u8>, Shown> {
    let page = if from.is_empty() {
        None
    } else {
        Some(read_text(from)?)
    };
    let mut ws = Vec::new();
    for dir in widgets {
        let body = dir_body(dir, Vec::new(), NameKey::Widget)?;
        let v = crate::out::pylib::json::loads(&String::from_utf8_lossy(&body))
            .unwrap_or(Value::Null);
        ws.push(Value::Object(vec![
            (
                Text::from("widget"),
                g(&v, "widget").cloned().unwrap_or(Value::Null),
            ),
            (
                Text::from("files"),
                g(&v, "files").cloned().unwrap_or(Value::Null),
            ),
        ]));
    }
    let mut b = vec![("name", print::jstr(name))];
    if texting {
        b.push(("template", print::jstr("texting")));
    }
    if !at.is_empty() {
        b.push(("at", int_value(at)));
    }
    if let Some(p) = page {
        if !p.is_empty() || !from.is_empty() {
            b.push(("page", print::jstr(&p)));
        }
    }
    if !ws.is_empty() {
        b.push(("widgets", Value::Array(ws)));
    }
    Ok(dumps(b))
}

/// `cmd_zen_proposals`.
pub fn proposals(z: &mut Zen, words: &[&str]) -> Step {
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
    let verb = pos.first().copied().unwrap_or("list");
    if !matches!(verb, "list" | "show" | "approve" | "reject") {
        return stop(usage(format!(
            concat!(
                "Error: unknown '",
                cli_name!(),
                " zen proposals' command '{}' (show, approve, reject)"
            ),
            verb
        )));
    }
    let (view, raw) = match verb {
        "list" => {
            let raw = if all {
                z.get("/cli/zen/proposals", &["all=1"])?
            } else {
                z.get("/cli/zen/proposals", &[])?
            };
            ("list", raw)
        }
        "show" => {
            let Some(id) = pos.get(1).filter(|i| !i.is_empty()) else {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen proposals show <id>"
                )));
            };
            let q = format!("id={}", quote(id));
            ("show", z.get("/cli/zen/proposals/show", &[&q])?)
        }
        _ => {
            let Some(id) = pos.get(1).filter(|i| !i.is_empty()) else {
                return stop(usage(format!(
                    concat!("Error: ", cli_name!(), " zen proposals {} <id>"),
                    verb
                )));
            };
            let body = dumps(vec![("id", print::jstr(id))]);
            (
                "decide",
                z.session_post(&format!("/cli/zen/proposals/{verb}"), &body)?,
            )
        }
    };
    let name = if verb == "list" {
        concat!(cli_name!(), " zen proposals").to_string()
    } else {
        format!(concat!(cli_name!(), " zen proposals {}"), verb)
    };
    let repl = concat!(
        cli_name!(),
        " zen garden access <garden> (edit when you're on its list, else ask)"
    );
    if is_retired(&raw) {
        return Ok(retired_refuse(as_json, &name, repl).print());
    }
    let mut raw = raw;
    if access_on(z)? {
        if as_json {
            raw = retired_tag(&raw, &name, repl);
        } else {
            crate::out::stderr_line(&retired_notice(&name, repl));
        }
    }
    Ok(agent_print(view, as_json, &raw).print())
}

const RETIRED_IN: &str = "0.47.0";
const RETIRED_SEE: &str = concat!(cli_name!(), " study garden-changes");

/// `_zen_is_retired RAW`.
pub fn is_retired(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Ok(d) = crate::out::pylib::json::loads(&text) else {
        return false;
    };
    let e = g(&d, "error");
    let code = match e {
        Some(v @ Value::Object(_)) => g(v, "code"),
        other => other,
    };
    is(code, "retired")
}

/// `_zen_retired_notice VERB REPLACEMENT`.
pub fn retired_notice(verb: &str, repl: &str) -> String {
    format!(
        "Notice: `{verb}` is retired (Gardens use access lists since 0.46.3) and is removed in {RETIRED_IN}. Use: {repl}. See `{RETIRED_SEE}`."
    )
}

fn retired_object(verb: &str, repl: &str) -> Value {
    Value::Object(vec![
        (Text::from("verb"), print::jstr(verb)),
        (Text::from("replacement"), print::jstr(repl)),
        (Text::from("removedIn"), print::jstr(RETIRED_IN)),
        (Text::from("see"), print::jstr(RETIRED_SEE)),
    ])
}

/// `_zen_retired_tag RAW VERB REPLACEMENT`.
pub fn retired_tag(raw: &[u8], verb: &str, repl: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(raw).into_owned();
    match crate::out::pylib::json::loads(&text) {
        Ok(Value::Object(mut d)) => {
            let k = Text::from("retired");
            let v = retired_object(verb, repl);
            match d.iter_mut().find(|(n, _)| *n == k) {
                Some(slot) => slot.1 = v,
                None => d.push((k, v)),
            }
            json::to_string(&Value::Object(d), &Options::default())
                .unwrap_or_default()
                .into_bytes()
        }
        _ => text.into_bytes(),
    }
}

/// `_zen_retired_refuse AS_JSON VERB REPLACEMENT`: exit 1.
fn retired_refuse(as_json: bool, verb: &str, repl: &str) -> Shown {
    let mut o = Out::default();
    if as_json {
        let mut v = vec![(Text::from("error"), print::jstr("retired"))];
        if let Value::Object(p) = retired_object(verb, repl) {
            v.extend(p);
        }
        o.o(compact(&Value::Object(v)));
    } else {
        o.e(retired_notice(verb, repl));
    }
    o.done(Exit::Error)
}
