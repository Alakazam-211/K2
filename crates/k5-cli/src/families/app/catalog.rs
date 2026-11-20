//! The reads of the suite App catalog, as the shell CLI's `cmd_app_catalog`
//! (`cli/src/app-catalog.sh`) answers them: `app catalog [list]`, `app
//! catalog show <entry>` and `app catalog job|jobs [<id>] [--app <app>]`,
//! with the family's page and its usage lines. Each read is the shell's
//! word loop, one `_compute_call`, `--json` as it came, else the shell's
//! Python line for line. One job's view is `_app_catalog_print_job`, then
//! `_app_catalog_end`: its end, and the exit its state gives (0 done, 1
//! failed, 2 anything else).
//!
//! The shell's still: `catalog check` (it names the workspace through
//! `_compute_ws`) and `catalog retry` (a write).

use super::call::Ctx;
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::out::py as pyfmt;
use crate::transport::encode::quote;
use k2_client::cli_name;
use k2_client::hosts::ZONE_SUFFIX;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const UNKNOWN_TAIL: &str = concat!(
        " (list|show|check|job|retry; ",
        cli_name!(),
        " app catalog --help)"
    );
    pub const UNKNOWN_HEAD: &str = concat!("Unknown: ", cli_name!(), " app catalog ");
    pub const FLAG_TAIL: &str = concat!(" (", cli_name!(), " app catalog --help)");
    pub const SHOW: &str = concat!("usage: ", cli_name!(), " app catalog show <entry> [--json]");
}

/// `"run: k2 app catalog retry %s"`.
const RETRY: &str = concat!("run: ", cli_name!(), " app catalog retry ");
/// `"…; check: k2 app catalog job %s"`.
const CHECK_JOB: &str = concat!("check: ", cli_name!(), " app catalog job ");
/// `"ready: k2 app install --catalog %s"`.
const READY: &str = concat!(
    "             ready: ",
    cli_name!(),
    " app install --catalog "
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The family's page, on stdout.
    Page,
    /// `catalog [list]`.
    List { json: bool },
    /// `catalog show <entry>`: its query.
    Show { query: String, json: bool },
    /// `catalog job|jobs`: its query (`id=…` is one job's view).
    Job { query: String, json: bool },
}

/// The words after `catalog`. `None`: a help form, `check` or `retry`
/// (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = words.first().copied().unwrap_or("list");
    let rest = match sub {
        "help" => return Some(Ok(Ask::Page)),
        "list" => words.get(1..).unwrap_or_default(),
        "--json" => words,
        "show" | "job" | "jobs" => &words[1..],
        "check" | "retry" => return None,
        other => {
            return Some(Err(Stop::usage(format!(
                "{}{other}{}",
                usage::UNKNOWN_HEAD,
                usage::UNKNOWN_TAIL
            ))))
        }
    };
    Some(loop_words(sub, rest))
}

/// The shell's loop over the words, then the sub-verb's checks.
fn loop_words(sub: &str, words: &[&str]) -> Result<Ask, Stop> {
    let (mut json, mut app) = (false, String::new());
    let mut args: Vec<&str> = Vec::new();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--json" => json = true,
            "--id" | "--app" => {
                // `x="${2:-}"; shift 2`: with one word left, set -e ends it.
                let v = rest.get(1).ok_or_else(|| Stop::silent(Exit::Error))?;
                if w == "--app" {
                    app = v.to_string();
                }
                rest = &rest[2..];
                continue;
            }
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!("Unknown flag: {w}{}", usage::FLAG_TAIL)))
            }
            _ => args.push(w),
        }
        rest = &rest[1..];
    }
    match sub {
        "show" => {
            if args.len() != 1 {
                return Err(Stop::usage(usage::SHOW));
            }
            Ok(Ask::Show {
                query: format!("entry={}", quote(args[0])),
                json,
            })
        }
        "job" | "jobs" => {
            let mut query = String::new();
            if let Some(id) = args.first() {
                query = format!("id={}", quote(id));
            }
            if !app.is_empty() {
                query = format!("app={}", quote(&app));
            }
            Ok(Ask::Job { query, json })
        }
        _ => Ok(Ask::List { json }),
    }
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Page => Ok(Shown::out(
            crate::text::help("app/catalog").unwrap_or_default(),
        )),
        Ask::List { json } => {
            let body = ctx.call(false, "/cli/app/catalog", "")?;
            Ok(super::access::answer(&body, *json, list_lines))
        }
        Ask::Show { query, json } => {
            let body = ctx.call(false, "/cli/app/catalog/show", query)?;
            Ok(super::access::answer(&body, *json, show_lines))
        }
        Ask::Job { query, json } => {
            let body = ctx.call(false, "/cli/app/catalog/job", query)?;
            if *json {
                return Ok(super::wallet::raw(&body));
            }
            if query.starts_with("id=") {
                return Ok(one_job(&body));
            }
            Ok(super::access::answer(&body, false, jobs_lines))
        }
    }
}

/// `v + suffix` for a string suffix.
fn plus(v: &Value, suffix: &str, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{}{suffix}", py::s(v))),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                pyfmt::type_name(other)
            ),
        )),
    }
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `cmd_app_catalog list`'s Python.
pub fn list_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let entries = py::dget_or(&v, "entries", py::empty_list(), 4)?;
    for e in py::iter(&entries, 4)? {
        let a = py::or(py::dget(&e, "defaultAddress", 5)?, empty_dict());
        let ver = py::or(py::dget(&e, "version", 6)?, py::text("not pinned yet"));
        let entry = py::idx(&e, "entry", 7)?;
        let name = py::idx(&e, "name", 7)?;
        let url = py::dget(&a, "url", 7)?;
        let url = if py::truthy(&url) {
            py::s(&url)
        } else {
            let label = py::dget_or(&a, "label", py::text(""), 7)?;
            plus(&label, &format!(".<box>{ZONE_SUFFIX}"), 7)?
        };
        let verified = py::or(
            py::dget(&e, "verified", 7)?,
            py::text("not verified yet: nothing pinned"),
        );
        let license = py::dget(&e, "license", 7)?;
        o.line(&format!(
            "{} {} {} {url}  ({}, {})",
            py::left(&entry, 12),
            py::left(&name, 7),
            py::left(&ver, 14),
            py::s(&verified),
            py::s(&license)
        ));
        let summary = py::dget_or(&e, "summary", py::text(""), 8)?;
        o.line(&format!("             {}", py::s(&summary)));
        if py::truthy(&py::dget(&e, "alreadyDeployed", 9)?) {
            let d = py::idx(&e, "alreadyDeployed", 10)?;
            o.line(&format!(
                "             already deployed as {} (one per box)",
                py::s(&d)
            ));
        } else if py::truthy(&py::dget(&e, "canDeploy", 11)?) {
            o.line(&format!("{READY}{}", py::s(&py::idx(&e, "entry", 12)?)));
        }
        let reasons = py::dget_or(&e, "reasons", py::empty_list(), 13)?;
        for r in py::iter(&reasons, 13)? {
            reason(&r, "             ", 14, o)?;
        }
        let installed = py::dget_or(&e, "installed", py::empty_list(), 17)?;
        for i in py::iter(&installed, 17)? {
            let app = py::idx(&i, "app", 18)?;
            let version = py::idx(&i, "version", 18)?;
            let variant = py::idx(&i, "variant", 18)?;
            o.line(&format!(
                "             installed: {} {} ({})",
                py::s(&app),
                py::s(&version),
                py::s(&variant)
            ));
        }
    }
    o.line(&format!("Trust: {}", py::s(&py::dget(&v, "trust", 19)?)));
    Ok(Exit::Ok)
}

/// `can't yet: <text> (<code>)`, then `run: <hint>` two spaces in, from
/// `line` on.
fn reason(r: &Value, indent: &str, line: u32, o: &mut Out) -> Result<(), Raise> {
    let text = py::dget(r, "text", line)?;
    let code = py::dget(r, "code", line)?;
    o.line(&format!(
        "{indent}can't yet: {} ({})",
        py::s(&text),
        py::s(&code)
    ));
    if py::truthy(&py::dget(r, "hint", line + 1)?) {
        let hint = py::idx(r, "hint", line + 2)?;
        o.line(&format!("{indent}  run: {}", py::s(&hint)));
    }
    Ok(())
}

/// `cmd_app_catalog show`'s Python.
pub fn show_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let e = py::load(body, 3)?;
    let a = py::or(py::dget(&e, "defaultAddress", 4)?, empty_dict());
    let name = py::idx(&e, "name", 5)?;
    let summary = py::idx(&e, "summary", 5)?;
    let entry = py::idx(&e, "entry", 5)?;
    o.line(&format!(
        "{} \u{2014} {} ({})",
        py::s(&name),
        py::s(&summary),
        py::s(&entry)
    ));
    let publisher = py::idx(&py::idx(&e, "publisher", 6)?, "name", 6)?;
    let license = py::idx(&e, "license", 6)?;
    let verified = py::or(
        py::dget(&e, "verified", 6)?,
        py::text("not verified yet: nothing pinned"),
    );
    o.line(&format!(
        "  publisher {} \u{b7} licence {} \u{b7} {}",
        py::s(&publisher),
        py::s(&license),
        py::s(&verified)
    ));
    let install = py::idx(&e, "installId", 7)?;
    let url = py::dget(&a, "url", 7)?;
    let address = if py::truthy(&url) {
        url
    } else {
        py::dget(&a, "label", 7)?
    };
    let one = if py::truthy(&py::dget(&e, "onePerBox", 7)?) {
        " \u{b7} one per box"
    } else {
        ""
    };
    o.line(&format!(
        "  installs as App {} \u{b7} default address {}{one}",
        py::s(&install),
        py::s(&address)
    ));
    let version = py::or(py::dget(&e, "version", 8)?, py::text("none yet"));
    let versions = py::or(py::dget(&e, "versions", 8)?, py::empty_list());
    let pinned = py::join(", ", &versions, 8)?;
    let pinned = if pinned.is_empty() {
        "none".to_string()
    } else {
        pinned
    };
    o.line(&format!("  version {} (pinned: {pinned})", py::s(&version)));
    o.line("  it will:");
    let will = py::dget_or(&e, "willDo", py::empty_list(), 10)?;
    for line in py::iter(&will, 10)? {
        o.line(&format!("    - {}", py::s(&line)));
    }
    let approves = py::dget_or(&e, "approves", py::empty_list(), 12)?;
    for x in py::iter(&approves, 12)? {
        let kind = if py::idx(&x, "kind", 13)? == py::text("boxService") {
            "box service"
        } else {
            "cap"
        };
        let name = py::idx(&x, "name", 13)?;
        let why = if py::truthy(&py::dget(&x, "why", 13)?) {
            py::concat(": ", &py::idx(&x, "why", 13)?, 13)?
        } else {
            String::new()
        };
        o.line(&format!(
            "  deploying approves {kind} {}{why}",
            py::s(&name)
        ));
    }
    let reasons = py::dget_or(&e, "reasons", py::empty_list(), 14)?;
    for r in py::iter(&reasons, 14)? {
        reason(&r, "  ", 15, o)?;
    }
    Ok(Exit::Ok)
}

/// `cmd_app_catalog jobs`' Python (the list).
pub fn jobs_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let jobs = py::dget_or(&v, "jobs", py::empty_list(), 3)?;
    for j in py::iter(&jobs, 3)? {
        let waiting = py::or(py::dget(&j, "waiting", 4)?, empty_dict());
        let mut w = py::dget(&waiting, "reason", 4)?;
        if !py::truthy(&w) {
            let error = py::or(py::dget(&j, "error", 4)?, empty_dict());
            w = py::or(py::dget(&error, "code", 4)?, py::text(""));
        }
        let id = py::idx(&j, "id", 5)?;
        let entry = py::idx(&j, "entry", 5)?;
        let app = py::idx(&j, "app", 5)?;
        let state = py::idx(&j, "state", 5)?;
        let step = py::idx(&j, "step", 5)?;
        o.line(&format!(
            "{}  {} {} {} {} {}",
            py::s(&id),
            py::left(&entry, 11),
            py::left(&app, 8),
            py::left(&state, 9),
            py::left(&step, 10),
            py::s(&w)
        ));
    }
    Ok(Exit::Ok)
}

/// `v.get("job") or v`.
fn the_job(v: &Value, line: u32) -> Result<Value, Raise> {
    Ok(py::or(py::dget(v, "job", line)?, v.clone()))
}

/// One job: `_app_catalog_print_job ""`, then `_app_catalog_end 0`, each
/// Python its own step (a raise ends the command, exit 1).
fn one_job(body: &[u8]) -> Shown {
    let mut o = Out::default();
    if let Err(e) = print_job(body, &mut o) {
        return o.done(Err(e));
    }
    // `state="$(… print((v.get("job") or v).get("state","")))"`.
    let state = match job_state(body) {
        Ok(s) => s,
        Err(e) => return o.done(Err(e)),
    };
    if let Err(e) = end_lines(body, &mut o) {
        return o.done(Err(e));
    }
    let exit = match state.as_str() {
        "done" => Exit::Ok,
        "failed" => Exit::Error,
        _ => Exit::Usage,
    };
    o.done(Ok(exit))
}

/// `_app_catalog_print_job`'s Python, with no step seen yet.
fn print_job(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let j = the_job(&v, 4)?;
    let steps = py::dget_or(&j, "steps", py::empty_list(), 6)?;
    for s in py::iter(&steps, 6)? {
        if py::dget(&s, "state", 7)? != py::text("done") {
            continue;
        }
        let step = py::idx(&s, "step", 7)?;
        if matches!(step, Value::Array(_) | Value::Object(_)) {
            // `x not in seen` hashes `x`.
            return Err(Raise::at(
                7,
                format!("TypeError: unhashable type: '{}'", pyfmt::type_name(&step)),
            ));
        }
        let note = if py::truthy(&py::dget(&s, "note", 8)?) {
            py::concat("  ", &py::idx(&s, "note", 8)?, 8)?
        } else {
            String::new()
        };
        let note: String = note.chars().take(100).collect();
        let step = py::idx(&s, "step", 9)?;
        o.line(&format!("  ok   {}{note}", py::left(&step, 10)));
    }
    Ok(())
}

/// The state line `_app_catalog_end` reads, as `$(…)` holds it.
fn job_state(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 1)?;
    let j = the_job(&v, 1)?;
    let st = py::dget_or(&j, "state", py::text(""), 1)?;
    Ok(py::s(&st).trim_end_matches('\n').to_string())
}

/// `_app_catalog_end`'s Python (not `--json`).
fn end_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let j = the_job(&v, 4)?;
    let a = py::or(py::dget(&j, "address", 5)?, empty_dict());
    let st = py::dget(&j, "state", 6)?;
    if st == py::text("done") {
        let entry = py::dget(&j, "entry", 8)?;
        let url = py::dget(&a, "url", 8)?;
        let at = if py::truthy(&url) {
            url
        } else {
            py::dget(&a, "label", 8)?
        };
        let app = py::dget(&j, "app", 8)?;
        let variant = py::dget(&j, "variant", 8)?;
        let version = py::dget(&j, "version", 8)?;
        o.line(&format!(
            "{} is live at {} (App {}, {} {})",
            py::s(&entry),
            py::s(&at),
            py::s(&app),
            py::s(&variant),
            py::s(&version)
        ));
        let next = py::or(py::dget(&j, "next", 9)?, py::empty_list());
        for line in py::iter(&next, 9)? {
            o.line(&format!("next: {}", py::s(&line)));
        }
    } else if st == py::text("failed") {
        let e = py::or(py::dget(&j, "error", 12)?, empty_dict());
        let mut text = py::dget(&e, "text", 15)?;
        if !py::truthy(&text) {
            text = py::or(py::dget(&e, "hint", 15)?, py::text(""));
        }
        let cmd = if py::contains(&e, "text", 16)? {
            py::dget(&e, "hint", 16)?
        } else {
            Value::Null
        };
        let step = py::dget(&j, "step", 17)?;
        let code = py::dget(&e, "code", 17)?;
        o.line(&format!(
            "failed at {} ({}): {}",
            py::s(&step),
            py::s(&code),
            py::s(&text)
        ));
        if py::truthy(&cmd) {
            o.line(&format!("run: {}", py::s(&cmd)));
        } else if py::truthy(&py::dget(&e, "retry", 20)?) {
            o.line(&format!("{RETRY}{}", py::s(&py::dget(&j, "id", 21)?)));
        }
    } else if st == py::text("waiting") {
        let w = py::or(py::dget(&j, "waiting", 23)?, empty_dict());
        let step = py::dget(&j, "step", 24)?;
        let reason = py::dget(&w, "reason", 24)?;
        o.line(&format!(
            "waiting at {}: {}",
            py::s(&step),
            py::s(&reason)
        ));
        if py::truthy(&py::dget(&w, "requests", 25)?) {
            let requests = py::join(", ", &py::idx(&w, "requests", 26)?, 26)?;
            o.line(&format!("  request(s): {requests}"));
        }
        let mut text = py::dget(&w, "text", 27)?;
        if !py::truthy(&text) {
            text = py::dget(&w, "hint", 27)?;
        }
        if py::truthy(&text) {
            o.line(&format!("  {}", py::s(&text)));
        }
        if py::contains(&w, "text", 30)? && py::truthy(&py::dget(&w, "hint", 30)?) {
            o.line(&format!("  run: {}", py::s(&py::idx(&w, "hint", 31)?)));
        }
        let id = py::dget(&j, "id", 32)?;
        o.line(&format!(
            "The job waits in the daemon and goes on by itself; {CHECK_JOB}{}",
            py::s(&id)
        ));
    } else {
        let step = py::dget(&j, "step", 34)?;
        let id = py::dget(&j, "id", 34)?;
        o.line(&format!(
            "still {} at {}; {CHECK_JOB}{}",
            py::s(&st),
            py::s(&step),
            py::s(&id)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(words: &[&str]) -> Option<Result<Ask, Stop>> {
        parse(words)
    }

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        assert_eq!(p(&[]), Some(Ok(Ask::List { json: false })));
        assert_eq!(p(&["--json"]), Some(Ok(Ask::List { json: true })));
        assert_eq!(p(&["list", "x", "--json"]), Some(Ok(Ask::List { json: true })));
        assert_eq!(p(&["help"]), Some(Ok(Ask::Page)));
        assert_eq!(p(&["check", "a", "b"]), None);
        assert_eq!(p(&["retry", "j"]), None);
        assert_eq!(p(&["list", "-h"]), None);
        assert_eq!(
            p(&["show", "a b"]),
            Some(Ok(Ask::Show {
                query: "entry=a%20b".into(),
                json: false
            }))
        );
        assert_eq!(p(&["show"]), Some(Err(Stop::usage(usage::SHOW))));
        assert_eq!(
            p(&["jobs", "j1", "--app", "acme"]),
            Some(Ok(Ask::Job {
                query: "app=acme".into(),
                json: false
            }))
        );
        assert_eq!(
            p(&["job", "--id"]),
            Some(Err(Stop::silent(Exit::Error)))
        );
        assert!(matches!(p(&["zap"]), Some(Err(s)) if s.line.ends_with("app catalog --help)")));
        assert!(matches!(p(&["list", "--x"]), Some(Err(s)) if s.line.starts_with("Unknown flag: --x")));
    }

    #[test]
    fn one_job_exits_by_its_state() {
        let done = one_job(br#"{"job":{"id":"j","state":"done","entry":"e","steps":[{"step":"pull","state":"done","note":"ok"},{"step":"x"}],"address":{"label":"l"},"next":["n"]}}"#);
        assert_eq!(done.exit, Exit::Ok);
        assert_eq!(
            done.stdout,
            "  ok   pull        ok\ne is live at l (App None, None None)\nnext: n\n"
        );
        let failed = one_job(br#"{"state":"failed","error":{"code":"c","hint":"h"}}"#);
        assert_eq!(failed.exit, Exit::Error);
        assert_eq!(failed.stdout, "failed at None (c): h\n");
        let waiting = one_job(br#"{"state":"waiting","id":"j"}"#);
        assert_eq!(waiting.exit, Exit::Usage);
        let raised = one_job(br#"[1]"#);
        assert_eq!(raised.exit, Exit::Error);
        assert!(raised.stderr.ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    }
}
