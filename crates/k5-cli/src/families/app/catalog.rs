//! The reads of the suite App catalog, as the shell CLI's `cmd_app_catalog`
//! (`cli/src/app-catalog.sh`) answers them: `app catalog [list]`, `app
//! catalog show <entry>` and `app catalog job|jobs [<id>] [--app <app>]`,
//! with the family's page and its usage lines. Each read is the shell's
//! word loop, one `_compute_call`, `--json` as it came, else the shell's
//! Python line for line. One job's view is `_app_catalog_print_job`, then
//! `_app_catalog_end`: its end, and the exit its state gives (0 done, 1
//! failed, 2 anything else).
//!
//! `catalog check <entry> <address>` asks whether an address is free (the
//! workspace as `_compute_ws` names it) and exits 2 when it isn't;
//! `catalog retry <job>` is a write, sent once.
//!
//! `app install --catalog <entry> …` (`cmd_app_install_catalog`) is the
//! one-click install ([`install`]): one POST, sent once; an address
//! problem is its JSON line and exit 2 (`_app_catalog_fail`). `--dry-run`
//! prints the plan. Else the daemon runs a job: `--no-wait` names it, and
//! without, the job is watched every 2 seconds (600 reads at most), each
//! step that is done printed once as it comes, then its end as `catalog
//! job <id>` prints it, and the exit its state gives.

use super::call::{Ctx, Given, Split};
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
    pub const CHECK: &str = concat!(
        "usage: ",
        cli_name!(),
        " app catalog check <entry> <address> [--id ID] [--json]"
    );
    pub const RETRY: &str = concat!("usage: ", cli_name!(), " app catalog retry <job-id>");
    pub const INSTALL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app install --catalog <entry> [--address LABEL|auto] [--dry-run] (entries: ",
        cli_name!(),
        " app catalog)"
    );
    pub const INSTALL_ID: &str = concat!(
        "usage: ",
        cli_name!(),
        " app install --catalog <entry> [--id ID] [--address LABEL|auto] (",
        cli_name!(),
        " app catalog --help)"
    );
    pub const VARIANT: &str = "--variant is k2 or standalone";
}

/// `"Started job %s (it runs in the daemon). Follow it: k2 app catalog job %s"`.
const STARTED: &str = concat!(
    " (it runs in the daemon). Follow it: ",
    cli_name!(),
    " app catalog job "
);

/// `"run: k2 app catalog retry %s"`.
const RETRY: &str = concat!("run: ", cli_name!(), " app catalog retry ");
/// `"Job %s goes on; follow it: k2 app catalog job %s"`.
const FOLLOW: &str = concat!(" goes on; follow it: ", cli_name!(), " app catalog job ");
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
    /// `catalog check <entry> <address> [--id ID]`.
    Check {
        entry: String,
        address: String,
        id: String,
        json: bool,
    },
    /// `catalog retry <job>`.
    Retry { job: String, json: bool },
    /// `app install --catalog <entry> …`.
    Install(Install),
}

/// `app install --catalog <entry> [--address A] [--id ID] [--variant V]
/// [--version V] [--workspace WS] [--domain D] [--dry-run] [--no-wait]
/// [--rebind-meet] [--json]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Install {
    pub entry: String,
    pub address: String,
    pub id: String,
    pub variant: String,
    pub version: String,
    pub ws: String,
    pub domain: String,
    pub dry: bool,
    pub wait: bool,
    pub rebind: bool,
    pub json: bool,
}

/// `cmd_app_install_catalog`'s loop and checks (a help form never comes
/// here: [`super::deploy::parse`] hands it to the shell first).
pub fn parse_install(words: &[&str]) -> Result<Ask, Stop> {
    let mut a = Install {
        wait: true,
        ..Install::default()
    };
    let mut i = 0;
    while let Some(w) = words.get(i).copied() {
        let mut take = |slot: &mut String| -> Result<(), Stop> {
            let (v, n) = super::deploy::value(words, i)?;
            *slot = v.to_string();
            i = n;
            Ok(())
        };
        match w {
            "--catalog" => take(&mut a.entry)?,
            "--address" => take(&mut a.address)?,
            "--id" => take(&mut a.id)?,
            "--variant" => take(&mut a.variant)?,
            "--version" => take(&mut a.version)?,
            "--workspace" => take(&mut a.ws)?,
            "--domain" => take(&mut a.domain)?,
            _ => {
                if let Some(v) = w.strip_prefix("--catalog=") {
                    a.entry = v.to_string();
                } else if let Some(v) = w.strip_prefix("--address=") {
                    a.address = v.to_string();
                } else if let Some(v) = w.strip_prefix("--id=") {
                    a.id = v.to_string();
                } else {
                    match w {
                        "--dry-run" => a.dry = true,
                        "--no-wait" => a.wait = false,
                        "--rebind-meet" => a.rebind = true,
                        "--json" => a.json = true,
                        _ if w.starts_with('-') => {
                            return Err(Stop::usage(format!(
                                "Unknown flag: {w}{}",
                                usage::FLAG_TAIL
                            )))
                        }
                        _ if !a.id.is_empty() => return Err(Stop::usage(usage::INSTALL_ID)),
                        _ => a.id = w.to_string(),
                    }
                }
                i += 1;
            }
        }
    }
    if a.entry.is_empty() {
        return Err(Stop::usage(usage::INSTALL));
    }
    if !matches!(a.variant.as_str(), "" | "k2" | "standalone") {
        return Err(Stop::usage(usage::VARIANT));
    }
    Ok(Ask::Install(a))
}

/// The words after `catalog`. `None`: a help form (the shell's).
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let sub = words.first().copied().unwrap_or("list");
    let rest = match sub {
        "help" => return Some(Ok(Ask::Page)),
        "list" => words.get(1..).unwrap_or_default(),
        "--json" => words,
        "show" | "job" | "jobs" | "check" | "retry" => &words[1..],
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
    let (mut json, mut app, mut id) = (false, String::new(), String::new());
    let mut args: Vec<&str> = Vec::new();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--json" => json = true,
            "--id" | "--app" => {
                // `x="${2:-}"; shift 2`: with no word left, `_need_value`'s line.
                let v = rest.get(1).ok_or_else(|| Stop::need_value(w))?;
                if w == "--app" {
                    app = v.to_string();
                } else {
                    id = v.to_string();
                }
                rest = &rest[2..];
                continue;
            }
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown flag: {w}{}",
                    usage::FLAG_TAIL
                )))
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
        "check" => {
            if args.len() != 2 {
                return Err(Stop::usage(usage::CHECK));
            }
            Ok(Ask::Check {
                entry: args[0].to_string(),
                address: args[1].to_string(),
                id,
                json,
            })
        }
        "retry" => {
            if args.len() != 1 {
                return Err(Stop::usage(usage::RETRY));
            }
            Ok(Ask::Retry {
                job: args[0].to_string(),
                json,
            })
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
        Ask::Check {
            entry,
            address,
            id,
            json,
        } => {
            let mut q = format!("entry={}&address={}", quote(entry), quote(address));
            if !id.is_empty() {
                q.push_str(&format!("&id={}", quote(id)));
            }
            let ws = ctx.ws("");
            if !ws.is_empty() {
                q.push_str(&format!("&workspace={}", quote(&ws)));
            }
            let body = ctx.call(false, "/cli/app/catalog/check-address", &q)?;
            // `ok="$(… print("1" if json.load(sys.stdin).get("ok") else "0"))"`:
            // a raise there ends the command (set -e), exit 1.
            let ok = (|| -> Result<bool, Raise> {
                Ok(py::truthy(&py::dget(&py::load(&body, 1)?, "ok", 1)?))
            })();
            let ok = match ok {
                Ok(ok) => ok,
                Err(e) => return Ok(Out::default().done(Err(e))),
            };
            let mut shown = super::access::answer(&body, *json, check_lines);
            // `[ "$ok" = 1 ] || exit 2`, after a printer that didn't raise.
            if shown.exit == Exit::Ok && !ok {
                shown.exit = Exit::Usage;
            }
            Ok(shown)
        }
        Ask::Install(a) => install(ctx, a),
        Ask::Retry { job, json } => {
            let body = super::call::Cj::new().s("job", job).body();
            let s = ctx.send(true, "/cli/app/catalog/retry", &body, false)?;
            if !s.ok() {
                return Err(ctx.fail(&s));
            }
            if *json {
                return Ok(super::wallet::raw(&s.body));
            }
            Ok(Shown::out(format!("Job {job}{FOLLOW}{job}\n")))
        }
    }
}

/// `cmd_app_catalog check`'s Python.
pub fn check_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let t = py::or(py::dget(&v, "tier", 4)?, empty_dict());
    if py::truthy(&py::dget(&v, "error", 5)?) {
        let e = py::idx(&v, "error", 6)?;
        let code = py::dget(&e, "code", 7)?;
        let hint = py::dget(&e, "hint", 7)?;
        o.line(&format!("not free: {}: {}", py::s(&code), py::s(&hint)));
        if py::truthy(&py::dget(&e, "suggestion", 8)?) {
            o.line(&format!(
                "suggestion: {}",
                py::s(&py::idx(&e, "suggestion", 9)?)
            ));
        }
    } else {
        let a = py::idx(&v, "address", 11)?;
        let mut at = py::dget(&a, "url", 12)?;
        if !py::truthy(&at) {
            at = py::idx(&a, "label", 12)?;
        }
        let kind = py::idx(&a, "kind", 12)?;
        let app = py::dget(&v, "app", 12)?;
        o.line(&format!(
            "free: {} ({}) for App {}",
            py::s(&at),
            py::s(&kind),
            py::s(&app)
        ));
    }
    if py::truthy(&py::dget(&t, "hint", 13)?) {
        o.line(&format!("pro: {}", py::s(&py::idx(&t, "hint", 14)?)));
    }
    Ok(Exit::Ok)
}

/// `v + suffix` for a string suffix.
pub(super) fn plus(v: &Value, suffix: &str, line: u32) -> Result<String, Raise> {
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
    if let Err(e) = print_job(body, "", &mut o) {
        return o.done(Err(e));
    }
    let end = job_end(body, false);
    Shown {
        stdout: o.stdout + &end.stdout,
        stderr: o.stderr + &end.stderr,
        exit: end.exit,
    }
}

/// `_app_catalog_print_job "$seen"`'s Python: each done step not in
/// `seen` (the steps `_app_catalog_done_steps` printed, by commas).
fn print_job(body: &[u8], seen: &str, o: &mut Out) -> Result<(), Raise> {
    let seen: Vec<&str> = seen.split(',').filter(|s| !s.is_empty()).collect();
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
        if matches!(&step, Value::Str(t) if t.as_str().is_some_and(|t| seen.contains(&t))) {
            continue;
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
        o.line(&format!("waiting at {}: {}", py::s(&step), py::s(&reason)));
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

fn bit(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

/// `cmd_app_install_catalog` after its loop.
fn install(ctx: &mut Ctx, a: &Install) -> Answer {
    let ws = ctx.ws(&a.ws);
    let body = Given::new()
        .put("entry", &a.entry)
        .put("address", &a.address)
        .put("id", &a.id)
        .put("variant", &a.variant)
        .put("version", &a.version)
        .put("workspace", &ws)
        .put("domain", &a.domain)
        .put("dryRun_b", bit(a.dry))
        .put("rebindMeet_b", bit(a.rebind))
        .body();
    let sp = ctx.send(true, "/cli/app/catalog/install", &body, false)?;
    if !sp.ok() {
        return Err(catalog_fail(ctx, &sp));
    }
    if a.dry {
        if a.json {
            return Ok(super::wallet::raw(&sp.body));
        }
        let mut o = Out::default();
        let r = plan_lines(&sp.body, &mut o).map(|()| Exit::Ok);
        return Ok(o.done(r));
    }
    // `job="$(… print(json.load(sys.stdin)["job"]["id"]))"`.
    let job = match one_value(&sp.body, "id") {
        Ok(j) => j,
        Err(e) => return Ok(Out::default().done(Err(e))),
    };
    if !a.wait {
        if a.json {
            return Ok(super::wallet::raw(&sp.body));
        }
        return Ok(Shown::out(format!("Started job {job}{STARTED}{job}\n")));
    }
    if !a.json {
        let lead = Shown::out(format!(
            "Job {job}: the daemon runs it; Ctrl-C stops watching, not the job.\n"
        ));
        let printed = lead.print();
        if printed != Exit::Ok {
            return Err(printed);
        }
    }
    let q = format!("id={}", quote(&job));
    let mut seen = String::new();
    let mut n = 0;
    let body = loop {
        let body = ctx.call(false, "/cli/app/catalog/job", &q)?;
        if !a.json {
            let mut o = Out::default();
            let r = print_job(&body, &seen, &mut o).map(|()| Exit::Ok);
            let printed = o.done(r).print();
            if printed != Exit::Ok {
                return Err(printed);
            }
            seen = match done_steps(&body) {
                Ok(s) => s,
                Err(e) => return Ok(Out::default().done(Err(e))),
            };
        }
        let state = match one_value(&body, "state") {
            Ok(s) => s,
            Err(e) => return Ok(Out::default().done(Err(e))),
        };
        if matches!(state.as_str(), "done" | "failed" | "waiting") {
            break body;
        }
        n += 1;
        if n >= 600 {
            break body;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    };
    Ok(job_end(&body, a.json))
}

/// `$(… print(json.load(sys.stdin)["job"][key]))`: the value as `print`
/// writes it, the newlines at its end gone.
fn one_value(body: &[u8], key: &str) -> Result<String, Raise> {
    let v = py::load(body, 1)?;
    let j = py::idx(&v, "job", 1)?;
    let x = py::idx(&j, key, 1)?;
    Ok(py::s(&x).trim_end_matches('\n').to_string())
}

/// `_app_catalog_fail`: an address the daemon won't give (taken, one per
/// box, reserved, not valid) is its JSON line on stderr and exit 2; any
/// other is `_compute_fail`.
fn catalog_fail(ctx: &Ctx, sp: &Split) -> Exit {
    // `try: print((json.load(sys.stdin).get("error") or {}).get("code",""))
    // except Exception: print("")`.
    let code = (|| -> Result<String, Raise> {
        let v = py::load(&sp.body, 2)?;
        let e = py::or(py::dget(&v, "error", 2)?, empty_dict());
        Ok(py::s(&py::dget_or(&e, "code", py::text(""), 2)?))
    })()
    .unwrap_or_default();
    let code = code.trim_end_matches('\n');
    if !matches!(
        code,
        "address_taken" | "one_per_box" | "address_reserved" | "address_invalid"
    ) {
        return ctx.fail(sp);
    }
    let line = (|| -> Result<String, Raise> {
        let v = py::load(&sp.body, 3)?;
        let e = py::idx(&v, "error", 3)?;
        let mut out = vec![
            (
                crate::out::json::Text::from("error"),
                py::dget(&e, "code", 4)?,
            ),
            (
                crate::out::json::Text::from("hint"),
                py::dget(&e, "hint", 4)?,
            ),
        ];
        for k in ["suggestion", "holder", "existing"] {
            if py::truthy(&py::dget(&e, k, 6)?) {
                out.push((crate::out::json::Text::from(k), py::idx(&e, k, 7)?));
            }
        }
        Ok(crate::confirm::dumps(&Value::Object(out)))
    })();
    match line {
        Ok(l) => {
            crate::out::stderr_line(&l);
            Exit::Usage
        }
        Err(e) => {
            let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
            Exit::Error
        }
    }
}

/// The dry run's Python: the plan.
fn plan_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let p = py::idx(&v, "plan", 4)?;
    let a = py::idx(&p, "address", 5)?;
    o.line("Dry run: nothing changed.");
    let name = py::idx(&p, "name", 7)?;
    let version = py::idx(&p, "version", 7)?;
    let variant = py::idx(&p, "variant", 7)?;
    let app = py::idx(&p, "app", 7)?;
    let url = py::dget(&a, "url", 7)?;
    let at = if py::truthy(&url) {
        url
    } else {
        py::idx(&a, "label", 7)?
    };
    o.line(&format!(
        "{} {} ({} variant) as App {} at {}",
        py::s(&name),
        py::s(&version),
        py::s(&variant),
        py::s(&app),
        py::s(&at)
    ));
    o.line("It will:");
    for line in py::iter(&py::dget_or(&p, "willDo", py::empty_list(), 9)?, 9)? {
        o.line(&format!("  - {}", py::s(&line)));
    }
    if py::truthy(&py::dget(&p, "approves", 11)?) {
        o.line("Deploying approves:");
        for x in py::iter(&py::idx(&p, "approves", 13)?, 13)? {
            let what = if py::idx(&x, "kind", 14)? == py::text("boxService") {
                "box service"
            } else {
                "cap"
            };
            let name = py::idx(&x, "name", 14)?;
            let why = if py::truthy(&py::dget(&x, "why", 14)?) {
                py::concat(": ", &py::idx(&x, "why", 14)?, 14)?
            } else {
                String::new()
            };
            o.line(&format!("  - {what} {}{why}", py::s(&name)));
        }
    }
    if py::truthy(&py::dget(&p, "setup", 15)?) {
        o.line(&format!(
            "Sew setup: {}",
            py::join(", ", &py::idx(&p, "setup", 16)?, 16)?
        ));
    }
    let steps = py::dget_or(&p, "steps", py::empty_list(), 17)?;
    o.line(&format!("Steps: {}", py::join(" \u{2192} ", &steps, 17)?));
    for w in py::iter(&py::dget_or(&p, "waits", py::empty_list(), 18)?, 18)? {
        let reason = py::idx(&w, "reason", 19)?;
        let text = py::or(py::dget(&w, "text", 19)?, py::dget(&w, "hint", 19)?);
        o.line(&format!(
            "It will wait at: {}: {}",
            py::s(&reason),
            py::s(&text)
        ));
        if py::truthy(&py::dget(&w, "text", 20)?) && py::truthy(&py::dget(&w, "hint", 20)?) {
            o.line(&format!("  run: {}", py::s(&py::idx(&w, "hint", 21)?)));
        }
    }
    o.line(&format!("Pro: {}", py::s(&py::dget(&v, "tier", 22)?)));
    let f = py::or(py::dget(&p, "files", 23)?, empty_dict());
    let tag = py::dget(&f, "tag", 24)?;
    let file = py::dget(&f, "file", 24)?;
    let sha = py::dget(&f, "sha256", 24)?;
    o.line(&format!(
        "File: {}/{} sha256 {}",
        py::s(&tag),
        py::s(&file),
        py::s(&sha)
    ));
    Ok(())
}

/// `_app_catalog_done_steps`: the done steps, as `$(…)` holds the line.
fn done_steps(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 3)?;
    let j = the_job(&v, 4)?;
    let steps = py::dget_or(&j, "steps", py::empty_list(), 5)?;
    let mut done = Vec::new();
    for s in py::iter(&steps, 5)? {
        if py::dget(&s, "state", 5)? == py::text("done") {
            done.push(py::idx(&s, "step", 5)?);
        }
    }
    Ok(py::join(",", &Value::Array(done), 5)?
        .trim_end_matches('\n')
        .to_string())
}

/// `_app_catalog_end`: the job's end (the answer as it came with
/// `--json`), and the exit its state gives: done 0, failed 1, else 2.
fn job_end(body: &[u8], json: bool) -> Shown {
    let mut o = Out::default();
    let state = match job_state(body) {
        Ok(s) => s,
        Err(e) => return o.done(Err(e)),
    };
    if json {
        o.stdout.push_str(&super::wallet::raw(body).stdout);
    } else if let Err(e) = end_lines(body, &mut o) {
        return o.done(Err(e));
    }
    let exit = match state.as_str() {
        "done" => Exit::Ok,
        "failed" => Exit::Error,
        _ => Exit::Usage,
    };
    o.done(Ok(exit))
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
        assert_eq!(
            p(&["list", "x", "--json"]),
            Some(Ok(Ask::List { json: true }))
        );
        assert_eq!(p(&["help"]), Some(Ok(Ask::Page)));
        assert_eq!(
            p(&["check", "a b", "c", "--id", "i"]),
            Some(Ok(Ask::Check {
                entry: "a b".into(),
                address: "c".into(),
                id: "i".into(),
                json: false
            }))
        );
        assert_eq!(p(&["check", "a"]), Some(Err(Stop::usage(usage::CHECK))));
        assert_eq!(
            p(&["retry", "j", "--json"]),
            Some(Ok(Ask::Retry {
                job: "j".into(),
                json: true
            }))
        );
        assert_eq!(p(&["retry"]), Some(Err(Stop::usage(usage::RETRY))));
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
        assert_eq!(p(&["job", "--id"]), Some(Err(Stop::need_value("--id"))));
        assert!(matches!(p(&["zap"]), Some(Err(s)) if s.line.ends_with("app catalog --help)")));
        assert!(
            matches!(p(&["list", "--x"]), Some(Err(s)) if s.line.starts_with("Unknown flag: --x"))
        );
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
        assert!(raised
            .stderr
            .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    }
}
