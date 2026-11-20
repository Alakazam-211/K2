//! `zen widget …` (`cmd_zen_widget`, `cli/src/zen-guide-data.sh`) and
//! the widget dev loop (`cli/src/zen-widget-dev.sh`): `new`, `list`,
//! `sign`, `claim`, `show`, `errors`, `put` (one file or `--dir`),
//! `apply`, `delete`, `preview` (`--dir`, `--clear`), `history` and the
//! arms that only print.
//!
//! Still the shell's ([`shells`]): `errors --follow` (it polls until
//! stopped) and `preview --shot` (it runs the Node shot runner).

use super::call::Zen;
use super::garden::{access_on, agent_print, history};
use super::print::{
    self, g, human, indented, int, is, items, or, s, t, text_of, zen_print, Env, Out,
};
use super::sign;
use super::verbs::{dir_body, help, usage, NameKey};
use crate::exit::Exit;
use crate::families::Shown;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

pub type Step = Result<Exit, Exit>;

fn stop(s: Shown) -> Step {
    Ok(s.print())
}

fn dumps(pairs: Vec<(&str, Value)>) -> Vec<u8> {
    let o: Vec<(Text, Value)> = pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect();
    json::to_string(&Value::Object(o), &Options::default())
        .unwrap_or_default()
        .into_bytes()
}

/// The widget lines the binary leaves to the shell: `errors` with
/// `--follow` or `-f`, and `preview` with `--shot` or a flag that goes
/// with it.
pub fn shells(words: &[&str]) -> bool {
    let verb = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match verb {
        "errors" => rest.iter().any(|w| matches!(*w, "--follow" | "-f")),
        "preview" => rest.iter().any(|w| {
            matches!(
                *w,
                "--shot" | "--theme" | "--zen-theme" | "--width" | "--height" | "--data" | "--libs"
                    | "--wait-ms"
            )
        }),
        _ => false,
    }
}

/// The prelude skips the connection gate for these.
pub fn no_gate(words: &[&str]) -> bool {
    matches!(
        words.first().copied(),
        None | Some("grant" | "allow" | "revoke" | "" | "help")
    )
}

/// `cmd_zen_widget VERB …`.
pub fn widget(z: &mut Zen, words: &[&str]) -> Step {
    let verb = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match verb {
        "put" => return put(z, rest),
        "apply" => return apply(z, rest),
        "preview" => return preview(z, rest),
        "errors" => return errors(z, rest),
        "show" => return show(z, rest),
        "delete" => return delete(z, rest),
        "history" if access_on(z)? => return history(z, "widget", rest),
        _ => {}
    }
    let (mut as_json, mut ask) = (false, false);
    let (mut from, mut wsel) = (String::new(), String::new());
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let next = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => as_json = true,
            "--ask" => ask = true,
            "--from" => {
                if next.is_empty() {
                    return stop(usage(
                        "Error: --from needs an example: k2:diary, hello or arcade",
                    ));
                }
                from = next.into();
                i += 1;
            }
            "--widget" => {
                if next.is_empty() {
                    return stop(usage("Error: --widget needs a widget folder name"));
                }
                wsel = next.into();
                i += 1;
            }
            _ if w.starts_with("--from=") => from = w["--from=".len()..].into(),
            _ if w.starts_with("--widget=") => wsel = w["--widget=".len()..].into(),
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!("Error: unexpected '{}' (see ", cli_name!(), " zen --help)"),
                    w
                )))
            }
            _ => pos.push(w),
        }
        i += 1;
    }
    let n = pos.len();
    if verb != "new" && !from.is_empty() {
        return stop(usage(concat!(
            "Error: --from is for ",
            cli_name!(),
            " zen widget new"
        )));
    }
    if !wsel.is_empty() && !matches!(verb, "revoke" | "grant" | "allow") {
        return stop(usage(concat!(
            "Error: --widget is for ",
            cli_name!(),
            " zen validate, history and reset"
        )));
    }
    match verb {
        "new" => {
            if n != 1 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen widget new needs one name (lower-case letters, digits, - and _)"
                )));
            }
            let mut b = vec![("name", print::jstr(pos[0]))];
            if !from.is_empty() {
                b.push(("from", print::jstr(&from)));
            }
            let raw = z.session_post("/cli/zen/widget/new", &dumps(b))?;
            Ok(zen_print("widget-new", as_json, &raw, Env::default()).print())
        }
        "sign" | "claim" => {
            if n != 1 {
                return stop(usage(format!(
                    concat!(
                        "Error: ",
                        cli_name!(),
                        " zen widget {} needs one widget folder name"
                    ),
                    verb
                )));
            }
            if ask && verb != "sign" {
                return stop(usage(concat!(
                    "Error: --ask is for ",
                    cli_name!(),
                    " zen widget sign"
                )));
            }
            let mut b = vec![("widget", print::jstr(pos[0]))];
            if ask {
                b.push(("ask", Value::Bool(true)));
            }
            let body = dumps(b);
            if verb == "sign" {
                let raw = z.session_post("/cli/zen/widget/sign", &body)?;
                return Ok(sign::print("sign", as_json, &raw).print());
            }
            let raw = z.session_post("/cli/zen/widget/claim", &body)?;
            if super::garden::is_retired(&raw) {
                let repl = format!(concat!(cli_name!(), " zen widget sign {}"), pos[0]);
                let name = concat!(cli_name!(), " zen widget claim");
                let mut raw = z.session_post("/cli/zen/widget/sign", &body)?;
                if as_json {
                    raw = super::garden::retired_tag(&raw, name, &repl);
                } else {
                    crate::out::stderr_line(&super::garden::retired_notice(name, &repl));
                }
                return Ok(sign::print("sign", as_json, &raw).print());
            }
            Ok(sign::print("claim", as_json, &raw).print())
        }
        "list" => {
            if n != 0 {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen widget list takes only --json"
                )));
            }
            let raw = z.get("/cli/zen/widgets", &[])?;
            Ok(zen_print("widget-list", as_json, &raw, Env::default()).print())
        }
        "grant" | "allow" | "revoke" => {
            let mut o = Out::default();
            o.o("Widgets in your own Gardens need no permissions: they can talk to your agents right away. To stop one, take its [[widget]] out of the Garden file.");
            Ok(o.done(Exit::Ok).print())
        }
        "" | "help" => Ok(help(false, Exit::Ok)),
        _ => stop(usage(format!(
            concat!(
                "Unknown '",
                cli_name!(),
                " zen widget' subcommand: {} (new, list, show, errors, sign, claim)"
            ),
            verb
        ))),
    }
}

/// `cmd_zen_widget_put`.
fn put(z: &mut Zen, words: &[&str]) -> Step {
    let (mut as_json, mut prune) = (false, false);
    let (mut from, mut note, mut dir) = (String::new(), String::new(), String::new());
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => as_json = true,
            "--from" => {
                if next.is_empty() {
                    return stop(usage("Error: --from needs a file (or - for stdin)"));
                }
                from = next.into();
                i += 1;
            }
            "--dir" => {
                if next.is_empty() {
                    return stop(usage("Error: --dir needs a widget folder"));
                }
                dir = next.into();
                i += 1;
            }
            _ if w.starts_with("--dir=") => dir = w["--dir=".len()..].into(),
            "--prune" => prune = true,
            "--note" => {
                // `note="${2:-}"; shift 2`: a failed shift ends the command.
                if i + 1 >= words.len() {
                    return Err(Exit::Error);
                }
                note = next.into();
                i += 1;
            }
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (",
                        cli_name!(),
                        " zen widget put --help)"
                    ),
                    w
                )))
            }
            _ => pos.push(w),
        }
        i += 1;
    }
    if !dir.is_empty() {
        if !from.is_empty() {
            return stop(usage(
                "Error: --dir puts a whole folder; --from is for one file",
            ));
        }
        if pos.len() > 1 {
            return stop(usage(concat!(
                "Error: ",
                cli_name!(),
                " zen widget put [<name>] --dir <folder> [--prune]"
            )));
        }
        let mut extra = vec![(Text::from("prune"), Value::Bool(prune))];
        if let Some(w) = pos.first().filter(|w| !w.is_empty()) {
            extra.push((Text::from("widget"), print::jstr(w)));
        }
        if !note.is_empty() {
            extra.push((Text::from("note"), print::jstr(&note)));
        }
        let body = match dir_body(&dir, extra, NameKey::Widget) {
            Ok(b) => b,
            Err(s) => return stop(s),
        };
        let raw = z.post_file("/cli/zen/widget/folder/put", &body)?;
        return Ok(agent_print("folder", as_json, &raw).print());
    }
    if prune {
        return stop(usage(concat!(
            "Error: --prune is for ",
            cli_name!(),
            " zen widget put --dir"
        )));
    }
    if pos.len() != 2 || from.is_empty() {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen widget put <widget> <file.html|.js|.css|.json> --from <path>|- [--note \"…\"] (or --dir <folder>; ",
            cli_name!(),
            " zen widget put --help)"
        )));
    }
    let bytes = match read_bytes(&from) {
        Ok(b) => b,
        Err(s) => return stop(s),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return stop(usage(format!(
            "Error: {} is not UTF-8 text; a widget code file is UTF-8",
            pos[1]
        )));
    };
    let mut b = vec![
        (Text::from("widget"), print::jstr(pos[0])),
        (Text::from("file"), print::jstr(pos[1])),
        (Text::from("text"), print::jstr(&text)),
    ];
    if !note.is_empty() {
        b.push((Text::from("note"), print::jstr(&note)));
    }
    let body = json::to_string(
        &Value::Object(b),
        &Options {
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .unwrap_or_default();
    let raw = z.session_post("/cli/zen/widget/put", body.as_bytes())?;
    Ok(agent_print("put", as_json, &raw).print())
}

/// `_zen_read_text SRC`, as bytes.
fn read_bytes(src: &str) -> Result<Vec<u8>, Shown> {
    if src == "-" {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut b);
        return Ok(b);
    }
    if !std::path::Path::new(src).is_file() {
        return Err(usage(format!("Error: no file {src}")));
    }
    Ok(std::fs::read(src).unwrap_or_default())
}

/// `cmd_zen_widget_apply`: `put --dir`.
fn apply(z: &mut Zen, words: &[&str]) -> Step {
    let mut args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--from" => {
                let next = words.get(i + 1).copied().unwrap_or("");
                if next.is_empty() {
                    return stop(usage("Error: --from needs a widget folder"));
                }
                args.push("--dir".into());
                args.push(next.into());
                i += 1;
            }
            _ if w.starts_with("--from=") => {
                args.push("--dir".into());
                args.push(w["--from=".len()..].into());
            }
            "--dir" => {
                return stop(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen widget apply takes --from <folder> (or use ",
                    cli_name!(),
                    " zen widget put --dir)"
                )))
            }
            _ => args.push(w.into()),
        }
        i += 1;
    }
    if !format!(" {} ", args.join(" ")).contains(" --dir ") {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen widget apply --from <folder> [<name>] [--prune]"
        )));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    put(z, &args)
}

/// `cmd_zen_widget_delete`.
fn delete(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let mut pos: Vec<&str> = Vec::new();
    for w in words {
        match *w {
            "--json" => as_json = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (",
                        cli_name!(),
                        " zen widget delete --help)"
                    ),
                    w
                )))
            }
            _ => pos.push(w),
        }
    }
    if pos.len() != 1 || pos[0].is_empty() {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen widget delete <name> [--json]"
        )));
    }
    let raw = z.session_post(
        "/cli/zen/widget/delete",
        &dumps(vec![("widget", print::jstr(pos[0]))]),
    )?;
    Ok(agent_print("delete", as_json, &raw).print())
}

/// `cmd_zen_widget_preview` (no `--shot`).
fn preview(z: &mut Zen, words: &[&str]) -> Step {
    let (mut as_json, mut clear) = (false, false);
    let mut dir = String::new();
    let mut pos: Vec<&str> = Vec::new();
    let mut cfg: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => as_json = true,
            "--dir" => {
                if next.is_empty() {
                    return stop(usage("Error: --dir needs a widget folder"));
                }
                dir = next.into();
                i += 1;
            }
            _ if w.starts_with("--dir=") => dir = w["--dir=".len()..].into(),
            "--config" => {
                if next.is_empty() {
                    return stop(usage("Error: --config needs key=value"));
                }
                cfg.push(next.into());
                i += 1;
            }
            _ if w.starts_with("--config=") => cfg.push(w["--config=".len()..].into()),
            "--clear" => clear = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (",
                        cli_name!(),
                        " zen widget preview --help)"
                    ),
                    w
                )))
            }
            _ => pos.push(w),
        }
        i += 1;
    }
    if clear {
        if !dir.is_empty() || !pos.is_empty() || !cfg.is_empty() {
            return stop(usage(concat!(
                "Error: ",
                cli_name!(),
                " zen widget preview --clear takes only --json"
            )));
        }
        let raw = z.session_post("/cli/zen/widget/preview", br#"{"clear":true}"#)?;
        return Ok(agent_print("preview", as_json, &raw).print());
    }
    if dir.is_empty() || pos.len() > 1 {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen widget preview --dir <folder> [<name>] [--config key=value]..."
        )));
    }
    let mut config: Vec<(Text, Value)> = Vec::new();
    for kv in &cfg {
        let Some((k, v)) = kv.split_once('=') else {
            return stop(usage(format!(
                "Error: --config takes key=value, not {}",
                py::repr(&print::jstr(kv))
            )));
        };
        let key = Text::from(py_strip(k));
        match config.iter_mut().find(|(n, _)| *n == key) {
            Some(slot) => slot.1 = print::jstr(v),
            None => config.push((key, print::jstr(v))),
        }
    }
    let mut extra = vec![(Text::from("config"), Value::Object(config))];
    if let Some(w) = pos.first().filter(|w| !w.is_empty()) {
        extra.push((Text::from("widget"), print::jstr(w)));
    }
    let body = match dir_body(&dir, extra, NameKey::Fallback) {
        Ok(b) => b,
        Err(s) => return stop(s),
    };
    let raw = z.post_file("/cli/zen/widget/preview", &body)?;
    Ok(agent_print("preview", as_json, &raw).print())
}

/// `str.strip()`.
fn py_strip(s: &str) -> String {
    s.trim_matches(crate::families::py_space).to_string()
}

/// `Error: <message or raw[:200]>`.
fn err_line(d: Option<&Value>, raw: &str) -> String {
    let m = d.and_then(|d| g(d, "message")).filter(|m| py::truthy(m));
    format!(
        "Error: {}",
        m.map_or_else(|| print::head200(raw), py::str_of)
    )
}

const NO_USABLE: &str = concat!(
    "Error: no usable answer from the ",
    product_name!(),
    " daemon on this computer"
);

/// `cmd_zen_widget_errors` (no `--follow`).
fn errors(z: &mut Zen, words: &[&str]) -> Step {
    let (mut as_json, mut clear, mut all) = (false, false, false);
    let mut since = String::new();
    let mut name = "";
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--since" => {
                since = words.get(i + 1).copied().unwrap_or("").into();
                i += 1;
            }
            _ if w.starts_with("--since=") => since = w["--since=".len()..].into(),
            "--clear" => clear = true,
            "--all" => all = true,
            "--json" => as_json = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (",
                        cli_name!(),
                        " zen widget errors --help)"
                    ),
                    w
                )))
            }
            _ => {
                if name.is_empty() {
                    name = w;
                } else {
                    return stop(usage("Error: one widget name"));
                }
            }
        }
        i += 1;
    }
    if name.is_empty() {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen widget errors needs a widget name"
        )));
    }
    if !since.is_empty() && !since.bytes().all(|b| b.is_ascii_digit()) {
        return stop(usage("Error: --since is a sequence number"));
    }
    if all && !clear {
        return stop(usage("Error: --all goes with --clear"));
    }
    if clear {
        if !since.is_empty() {
            return stop(usage("Error: --clear takes no --since or --follow"));
        }
        let body = dumps(vec![
            ("widget", print::jstr(name)),
            ("all", Value::Bool(all)),
        ]);
        let raw = z.session_post_soft("/cli/zen/widget/errors/clear", &body)?;
        return Ok(cleared_print(as_json, &raw).print());
    }
    let mut q = vec![format!("widget={}", quote(name))];
    if !since.is_empty() {
        q.push(format!("since={since}"));
    }
    let q: Vec<&str> = q.iter().map(String::as_str).collect();
    let raw = z.get_soft("/cli/zen/widget/errors", &q, false)?;
    let shown = errors_print(as_json, &since, &raw);
    let exit = shown.print();
    Ok(if exit == Exit::Refused { Exit::Error } else { exit })
}

fn cleared_print(as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let mut o = Out::default();
    let Ok(d) = crate::out::pylib::json::loads(&raw) else {
        o.e(NO_USABLE);
        return o.done(Exit::Error);
    };
    if !matches!(d, Value::Object(_)) || g(&d, "error").is_some_and(|e| !matches!(e, Value::Null)) {
        o.e(err_line(Some(&d), &raw));
        return o.done(Exit::Error);
    }
    if as_json {
        o.o(indented(&d));
        return o.done(Exit::Ok);
    }
    o.o(format!(
        "cleared {} runtime error(s) from widgets/{}{}; {} left",
        s(g(&d, "cleared")),
        s(g(&d, "widget")),
        if t(g(&d, "all")) { " (all)" } else { "" },
        s(g(&d, "left"))
    ));
    if t(g(&d, "left")) {
        o.o(format!(
            concat!(
                "the ones left still count: a placed widget, this version, the last 24 h (",
                cli_name!(),
                " zen widget errors {})"
            ),
            s(g(&d, "widget"))
        ));
    }
    o.done(Exit::Ok)
}

/// `e.get(k, default)`.
fn get_or<'a>(e: &'a Value, k: &str, default: Option<&'a Value>) -> Option<&'a Value> {
    match e {
        Value::Object(p) if p.iter().any(|(n, _)| n.as_str() == Some(k)) => g(e, k),
        _ => default,
    }
}

/// The runtime error's text (`text(e)`).
fn error_text(e: &Value) -> String {
    if let Some(m) = g(e, "message").filter(|m| !matches!(m, Value::Null)) {
        return py::str_of(m);
    }
    let class = or(g(e, "class"), "");
    let mut t_ = if is(g(e, "kind"), "refusal") {
        format!("refused {class}")
    } else if class.is_empty() {
        "error".to_string()
    } else {
        class
    };
    if t(g(e, "code")) {
        t_.push_str(&format!(" ({})", s(g(e, "code"))));
    }
    if t(g(e, "address")) {
        t_.push_str(&format!(" for {}", s(g(e, "address"))));
    } else if t(g(e, "addressWithheld")) {
        t_.push_str(" for an address no placement of this widget names (withheld)");
    }
    if g(e, "line").is_some_and(|l| !matches!(l, Value::Null)) {
        t_.push_str(&format!(" at line {}", s(g(e, "line"))));
    }
    if t(g(e, "redacted")) {
        t_.push_str(" [text withheld: this widget reads data sources]");
    }
    t_
}

/// `cmd_zen_widget_errors`'s Python: exit 3 (the shell's 1) when there
/// is no answer to read.
fn errors_print(as_json: bool, since: &str, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let mut o = Out::default();
    let Ok(d) = crate::out::pylib::json::loads(&raw) else {
        o.e(NO_USABLE);
        return o.done(Exit::Refused);
    };
    if !matches!(d, Value::Object(_)) || g(&d, "error").is_some_and(|e| !matches!(e, Value::Null)) {
        o.e(err_line(Some(&d), &raw));
        return o.done(Exit::Refused);
    }
    let errs = items(g(&d, "errors"));
    if as_json {
        o.o(indented(&d));
    } else {
        for e in &errs {
            let reason = g(e, "staleReason");
            let mark = if is(reason, "older-version") {
                " (older version)"
            } else if is(reason, "unplaced") {
                " (not placed: not counted)"
            } else if is(reason, "old") {
                " (over 24 h old: not counted)"
            } else if t(g(e, "current")) {
                ""
            } else {
                " (older version)"
            };
            o.o(format!(
                "#{} {}{mark} {}",
                s(g(e, "seq")),
                s(g(e, "at")),
                error_text(e)
            ));
            let stack = or(g(e, "stack"), "");
            for line in crate::out::pylib::ops::splitlines(&stack).iter().take(6) {
                o.o(format!("    {line}"));
            }
            if t(g(e, "suggestion")) {
                o.o(format!("  suggestion: {}", s(g(e, "suggestion"))));
            }
        }
        if errs.is_empty() {
            o.o(if since.is_empty() {
                "no runtime errors logged".to_string()
            } else {
                format!("no runtime errors after #{since}")
            });
        }
        if errs
            .iter()
            .any(|e| !get_or(e, "counted", g(e, "current")).is_some_and(py::truthy))
        {
            o.o(format!(
                concat!(
                    "old ones aren't counted by validate; drop them with ",
                    cli_name!(),
                    " zen widget errors {} --clear"
                ),
                s(g(&d, "widget"))
            ));
        }
    }
    let counted = errs
        .iter()
        .any(|e| get_or(e, "counted", g(e, "current")).is_some_and(py::truthy));
    o.done(if counted { Exit::Error } else { Exit::Ok })
}

/// `cmd_zen_widget_show`.
fn show(z: &mut Zen, words: &[&str]) -> Step {
    let mut as_json = false;
    let mut name = "";
    for w in words {
        match *w {
            "--json" => as_json = true,
            _ if w.starts_with('-') => {
                return stop(usage(format!(
                    concat!(
                        "Error: unexpected '{}' (",
                        cli_name!(),
                        " zen widget show --help)"
                    ),
                    w
                )))
            }
            _ => {
                if name.is_empty() {
                    name = w;
                } else {
                    return stop(usage("Error: one widget name"));
                }
            }
        }
    }
    if name.is_empty() {
        return stop(usage(concat!(
            "Error: ",
            cli_name!(),
            " zen widget show needs a widget name"
        )));
    }
    let q = format!("widget={}", quote(name));
    let raw = z.get_soft("/cli/zen/widget/show", &[&q], false)?;
    Ok(show_print(as_json, &raw).print())
}

fn show_print(as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let mut o = Out::default();
    let Ok(d) = crate::out::pylib::json::loads(&raw) else {
        o.e(NO_USABLE);
        return o.done(Exit::Error);
    };
    let dict = matches!(d, Value::Object(_));
    if !dict || g(&d, "error").is_some_and(|e| !matches!(e, Value::Null)) {
        o.e(err_line(Some(&d), &raw));
        let nf = dict && is(g(&d, "error"), "not_found");
        return o.done(if nf { Exit::NotFound } else { Exit::Error });
    }
    if as_json {
        o.o(indented(&d));
        return o.done(Exit::Ok);
    }
    let nothing = |v: Option<&Value>| {
        let j = print::join(v, ", ");
        if j.is_empty() {
            "nothing".to_string()
        } else {
            j
        }
    };
    o.o(format!(
        "{} ({}): {}",
        s(g(&d, "widget")),
        s(g(&d, "name")),
        s(g(&d, "state"))
    ));
    o.o(format!("  asks for: {}", nothing(g(&d, "requested"))));
    o.o(format!("  runs with: {}", nothing(g(&d, "caps"))));
    if t(g(&d, "reach")) {
        o.o(format!("  note: {}", s(g(&d, "reach"))));
    }
    if let Some(src) = g(&d, "sources").filter(|v| !matches!(v, Value::Null)) {
        let text = match src {
            Value::Str(_) => py::str_of(src),
            other => items(Some(other))
                .iter()
                .map(py::str_of)
                .collect::<Vec<_>>()
                .join(", "),
        };
        o.o(format!("  sources: {text}"));
    }
    if t(g(&d, "reads")) {
        o.o(format!(
            "  reads data of: {} (read-only)",
            print::join(g(&d, "reads"), ", ")
        ));
    }
    if let Some(rt @ Value::Object(_)) = g(&d, "runtime") {
        let errs = get_or(rt, "errorsSinceSave", None)
            .map_or_else(|| "0".to_string(), py::str_of);
        o.o(format!(
            "  this version: {}, {errs} runtime error(s) since saved",
            if t(g(rt, "ran")) {
                "has run"
            } else {
                "not run yet"
            }
        ));
    }
    if let Some(sz @ Value::Object(_)) = g(&d, "size") {
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
                "  size: code {} of {}{large}, page {} of {}, libraries {libs} (outside the limit)",
                human(code),
                human(int(g(sz, "codeMax"))),
                human(int(g(sz, "page"))),
                human(int(g(sz, "pageMax")))
            ));
        }
    }
    let pl = items(g(&d, "placements"));
    if pl.is_empty() {
        o.o("  not placed in any Garden");
    }
    for p in pl {
        let mut flags = Vec::new();
        if t(g(&p, "locked")) {
            flags.push("locked");
        }
        if t(g(&p, "paused")) {
            flags.push("posting paused");
        }
        let f = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };
        o.o(format!(
            "  - {} / {}{f}: {}",
            s(g(&p, "gardenName")),
            s(g(&p, "placement")),
            s(g(&p, "note"))
        ));
        if t(g(&p, "sources")) {
            o.o(format!("      reads: {}", print::join(g(&p, "sources"), ", ")));
        }
    }
    o.done(Exit::Ok)
}
