//! An App's hostnames, as the shell CLI answers them
//! (`cli/src/app-updates.sh`): `app domain add|rm|check|primary|external|list`
//! and `app domains`. Each is the shell's word loop, one request
//! (`_app_rt_post` then `_app_grace_done` for a change, `_compute_call`
//! for the list), `--json` as it came, else the shell's Python line for
//! line.
//!
//! `domain add|rm` asks Python first whether the answer is a custom
//! domain's (`"custom" in d or "removed" in d`, in an `if`: a raise there
//! is its traceback and the plain printer runs next), then prints it with
//! `_app_custom_print` or the label's own lines.
//!
//! `app domain` alone prints the page on stdout and exits 2.

use super::call::{Cj, Ctx};
use super::grace_set::{help, walk};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out::json::Value;
use crate::out::py as pyfmt;
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::hosts::{ZONE, ZONE_SUFFIX};
use k2_client::{cli_name, product_name};

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const DOMAIN: &str = concat!(
        "usage: ",
        cli_name!(),
        " app domain add|rm <app-id> <subdomain> [--json]"
    );
    pub const VERB: &str = concat!("usage: ", cli_name!(), " app domain ");
    pub const EXTERNAL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app domain external <app-id> <host> on|off"
    );
    pub const EXTERNAL_ADD: &str = concat!(
        "--external goes with ",
        cli_name!(),
        " app domain add (to switch a bound name: ",
        cli_name!(),
        " app domain external <app-id> <host> on|off)"
    );
    pub const DOMAINS: &str = concat!("usage: ", cli_name!(), " app domains <app-id> [--json]");
}

/// `app domain`'s page.
pub const PAGE: &str = "app/domain";

/// `"K2 Connect"`.
const CONNECT: &str = concat!(product_name!(), " Connect");
/// `"(taken from --cmd service %s; k2 app label return to undo)"`.
const TAKEN_TAIL: &str = concat!("; ", cli_name!(), " app label return to undo)");

/// What `relay.state` says.
const RELAY_WORDS: [(&str, &str); 4] = [
    ("claimed", "claimed, waiting for its own check of the TXT"),
    ("verified", "verified: its relays route this name here"),
    ("released", "released: its relays no longer route this name"),
    ("refused", "refused (see last check)"),
];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `app domain` alone: the page on stdout, exit 2.
    Page,
    /// `domain add|rm`.
    Label {
        verb: &'static str,
        body: String,
        json: bool,
    },
    /// `domain check|primary|external`: printed by `_app_custom_print`.
    Custom {
        path: String,
        body: String,
        json: bool,
    },
    List {
        app: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &["domain", "domains"];

/// The words after the verb. `None`: a help form (the shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(match verb {
        "domains" => list(words),
        _ => domain(words),
    })
}

fn domain(words: &[&str]) -> Result<Ask, Stop> {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "" => Ok(Ask::Page),
        v @ ("add" | "rm") => label(if v == "add" { "add" } else { "rm" }, rest),
        v @ ("check" | "primary") => custom(if v == "check" { "check" } else { "primary" }, rest),
        "external" => external(rest),
        "list" => list(rest),
        _ => Err(Stop::usage(usage::DOMAIN)),
    }
}

/// `app="$1"` first, then `x="$1"` for every later word.
fn two(plain: &[String]) -> (String, String) {
    let (mut a, mut b) = (String::new(), String::new());
    for p in plain {
        if a.is_empty() {
            a = p.clone();
        } else {
            b = p.clone();
        }
    }
    (a, b)
}

fn label(verb: &'static str, words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(words, &[], &["--json", "--transfer", "--external"])?;
    let (app, label) = two(&w.plain);
    if app.is_empty() || label.is_empty() {
        return Err(Stop::usage(format!(
            "{}{verb} <app-id> <subdomain>",
            usage::VERB
        )));
    }
    let external = w.has("--external");
    if external && verb != "add" {
        return Err(Stop::usage(usage::EXTERNAL_ADD));
    }
    let flag = |on: bool| if on { "1" } else { "" };
    let body = Cj::new()
        .s("app", &app)
        .s("label", &label)
        .b("transfer", flag(w.has("--transfer")))
        .b("external", flag(external))
        .body();
    Ok(Ask::Label {
        verb,
        body,
        json: w.has("--json"),
    })
}

fn custom(verb: &'static str, words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(words, &[], &["--json"])?;
    let (app, host) = two(&w.plain);
    if app.is_empty() || host.is_empty() {
        return Err(Stop::usage(format!("{}{verb} <app-id> <host>", usage::VERB)));
    }
    Ok(Ask::Custom {
        path: format!("/cli/app/domain/{verb}"),
        body: Cj::new().s("app", &app).s("host", &host).body(),
        json: w.has("--json"),
    })
}

fn external(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(words, &[], &["--json"])?;
    let (mut app, mut host, mut mode) = (String::new(), String::new(), String::new());
    for p in &w.plain {
        if app.is_empty() {
            app = p.clone();
        } else if host.is_empty() {
            host = p.clone();
        } else {
            mode = p.clone();
        }
    }
    let on = match mode.as_str() {
        "on" => "1",
        "off" => "0",
        _ => return Err(Stop::usage(usage::EXTERNAL)),
    };
    if app.is_empty() || host.is_empty() {
        return Err(Stop::usage(usage::EXTERNAL));
    }
    Ok(Ask::Custom {
        path: "/cli/app/domain/external".to_string(),
        body: Cj::new()
            .s("app", &app)
            .s("host", &host)
            .b("external", on)
            .body(),
        json: w.has("--json"),
    })
}

fn list(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(words, &[], &["--json"])?;
    let app = w.last();
    if app.is_empty() {
        return Err(Stop::usage(usage::DOMAINS));
    }
    Ok(Ask::List {
        app,
        json: w.has("--json"),
    })
}

fn printed(body: &[u8], lines: fn(&[u8], &mut Out) -> Result<(), Raise>) -> Shown {
    let mut o = Out::default();
    let r = lines(body, &mut o).map(|()| Exit::Ok);
    o.done(r)
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Page => {
            let mut page = Shown::out(crate::text::help(PAGE).unwrap_or_default());
            page.exit = Exit::Usage;
            Ok(page)
        }
        Ask::Label { verb, body, json } => {
            let s = ctx.send(true, &format!("/cli/app/domain/{verb}"), body, false)?;
            let body = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            if *json {
                return Ok(raw(&body));
            }
            // `if printf … | python3 -c '…'; then`: a raise is its
            // traceback, and the `else` side runs.
            let mut lead = String::new();
            let is_custom = match custom_or_removed(&body) {
                Ok(b) => b,
                Err(e) => {
                    lead = raised(String::new(), String::new(), &e).stderr;
                    false
                }
            };
            let mut shown = if is_custom {
                printed(&body, custom_lines)
            } else {
                printed(&body, label_lines)
            };
            shown.stderr = lead + &shown.stderr;
            Ok(shown)
        }
        Ask::Custom { path, body, json } => {
            let s = ctx.send(true, path, body, false)?;
            let body = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            if *json {
                return Ok(raw(&body));
            }
            Ok(printed(&body, custom_lines))
        }
        Ask::List { app, json } => {
            let body = ctx.call(false, "/cli/app/domains", &format!("app={}", quote(app)))?;
            if *json {
                return Ok(raw(&body));
            }
            Ok(printed(&body, domains_lines))
        }
    }
}

/// `d = json.load(sys.stdin); sys.exit(0 if ("custom" in d or "removed"
/// in d) else 1)`.
fn custom_or_removed(body: &[u8]) -> Result<bool, Raise> {
    let d = py::load(body, 1)?;
    Ok(py::contains(&d, "custom", 1)? || py::contains(&d, "removed", 1)?)
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

fn is_dict(v: &Value) -> bool {
    matches!(v, Value::Object(_))
}

/// `st not in ("live", "external")`.
fn not_live(st: &Value) -> bool {
    !(py_eq(st, &py::text("live")) || py_eq(st, &py::text("external")))
}

/// `"external (served by your own server)" if st == "external" else st`.
fn state_word(st: &Value) -> String {
    if py_eq(st, &py::text("external")) {
        "external (served by your own server)".to_string()
    } else {
        py::s(st)
    }
}

/// `domain add|rm`'s own lines.
fn label_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "already", 4)?) {
        let app = py::dget(&v, "app", 5)?;
        let label = py::dget(&v, "label", 5)?;
        let note = if py::truthy(&py::dget(&v, "note", 5)?) {
            format!("{})", py::concat(" (", &py::idx(&v, "note", 5)?, 5)?)
        } else {
            String::new()
        };
        o.line(&format!(
            "{} already serves {}{note}",
            py::s(&app),
            py::s(&label)
        ));
    } else if py::truthy(&py::dget(&v, "now", 6)?) {
        o.line(&format!(
            "{} no longer serves {} (now: {})",
            py::s(&py::dget(&v, "app", 7)?),
            py::s(&py::dget(&v, "label", 7)?),
            py::s(&py::dget(&v, "now", 7)?)
        ));
    } else {
        o.line(&format!(
            "{} now serves App {} (front port {})",
            py::s(&py::dget(&v, "label", 9)?),
            py::s(&py::dget(&v, "app", 9)?),
            py::s(&py::dget(&v, "frontPort", 9)?)
        ));
        if py::truthy(&py::dget(&v, "note", 10)?) {
            o.line(&py::s(&py::idx(&v, "note", 11)?));
        }
    }
    Ok(())
}

/// `print("    %-5s %-40s %s" % (r.get("type"), r.get("name"),
/// r.get("value")))`.
fn record_line(r: &Value, line: u32) -> Result<String, Raise> {
    let t = py::dget(r, "type", line)?;
    let n = py::dget(r, "name", line)?;
    let v = py::dget(r, "value", line)?;
    Ok(format!(
        "    {} {} {}",
        py::left(&t, 5),
        py::left(&n, 40),
        py::s(&v)
    ))
}

/// `_app_custom_print`.
fn custom_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "removed", 4)?) {
        let app = py::dget(&v, "app", 5)?;
        let removed = py::idx(&v, "removed", 5)?;
        let note = py::or(py::dget(&v, "note", 5)?, py::text(""));
        o.line(&format!(
            "{} no longer serves {} ({})",
            py::s(&app),
            py::s(&removed),
            py::s(&note)
        ));
        return Ok(());
    }
    if py::truthy(&py::dget(&v, "primary", 7)?) {
        o.line(&format!("primary: {}", py::s(&py::idx(&v, "primary", 8)?)));
        o.line(&py::s(&py::or(py::dget(&v, "note", 9)?, py::text(""))));
        return Ok(());
    }
    let c = py::or(py::dget(&v, "custom", 11)?, empty_dict());
    let st = py::dget(&c, "state", 12)?;
    let mut already = "";
    if py::truthy(&py::dget(&v, "already", 14)?) {
        already = if py::contains(&v, "external", 15)? {
            "  (no change)"
        } else {
            "  (already bound)"
        };
    }
    o.line(&format!(
        "{}   {}{already}",
        py::s(&py::dget(&c, "hostname", 16)?),
        state_word(&st)
    ));
    o.line(&format!("  {}", py::s(&py::dget(&c, "words", 17)?)));
    if py::truthy(&py::dget(&c, "lastError", 18)?) {
        o.line(&format!(
            "  last check: {}",
            py::s(&py::idx(&c, "lastError", 19)?)
        ));
    }
    let relay = py::or(py::dget(&c, "relay", 20)?, empty_dict());
    let rs = py::dget(&relay, "state", 20)?;
    if py::truthy(&rs) {
        let word = py::word_for(&RELAY_WORDS, &rs, rs.clone(), 23)?;
        o.line(&format!("  {CONNECT}: {}", py::s(&word)));
    }
    let t = py::dget(&v, "txt", 24)?;
    if is_dict(&t) && py::truthy(&py::dget(&t, "planted", 25)?) {
        o.line(&format!(
            "  {} set the TXT record in your {ZONE} zone",
            product_name!()
        ));
    } else if is_dict(&t) && py::truthy(&py::dget(&t, "why", 27)?) {
        o.line(&format!(
            "  TXT not set by {}: {}",
            product_name!(),
            py::s(&py::idx(&t, "why", 28)?)
        ));
    }
    let su = py::or(py::dget(&c, "setup", 29)?, empty_dict());
    if py::truthy(&py::dget(&su, "message", 30)?) && not_live(&st) {
        o.line(&format!("  {}", py::s(&py::idx(&su, "message", 31)?)));
    }
    if not_live(&st) && !py_eq(&py::dget(&su, "records", 32)?, &py::text("none")) {
        if py_eq(&py::dget(&su, "records", 33)?, &py::text("advanced")) {
            let adv = py::or(py::dget(&su, "advanced", 34)?, py::text("Advanced:"));
            o.line(&format!("  {}", py::s(&adv)));
        }
        let records = py::or(py::dget(&c, "records", 35)?, py::empty_list());
        for r in py::iter(&records, 35)? {
            o.line(&record_line(&r, 36)?);
            if py_eq(&py::dget(&r, "type", 37)?, &py::text("ALIAS"))
                && py::truthy(&py::dget(&r, "note", 37)?)
            {
                o.line(&format!("          {}", py::s(&py::idx(&r, "note", 38)?)));
            }
        }
    }
    if py::truthy(&py::dget(&c, "next", 39)?) {
        o.line(&format!("  {}", py::s(&py::idx(&c, "next", 40)?)));
    }
    if py::truthy(&py::dget(&c, "ownServer", 41)?) {
        o.line(&format!("  {}", py::s(&py::idx(&c, "ownServer", 42)?)));
    }
    if py::truthy(&py::dget(&v, "note", 43)?) && !py::truthy(&py::dget(&v, "already", 43)?) {
        o.line(&format!("  {}", py::s(&py::idx(&v, "note", 44)?)));
    }
    Ok(())
}

/// `host(r)`, called from line `from`.
fn host(r: &Value, from: u32) -> Result<String, Raise> {
    let h = py::dget(r, "host", 5).map_err(|e| Raise::inside(from, 5, "host", e.exception))?;
    if py::truthy(&h) {
        return Ok(py::s(&h));
    }
    let label = py::dget(r, "label", 5).map_err(|e| Raise::inside(from, 5, "host", e.exception))?;
    let tail = format!(".<box>{ZONE_SUFFIX}");
    match &label {
        Value::Str(_) => Ok(format!("{}{tail}", py::s(&label))),
        Value::Array(_) => Err(Raise::inside(
            from,
            5,
            "host",
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::inside(
            from,
            5,
            "host",
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                pyfmt::type_name(other)
            ),
        )),
    }
}

/// `cmd_app_domains`' Python.
fn domains_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    o.line(&format!(
        "{}  (its own label)",
        host(&py::idx(&v, "own", 6)?, 6)?
    ));
    let taken = py::or(py::dget(&v, "taken", 7)?, py::empty_list());
    for t in py::iter(&taken, 7)? {
        let h = host(&t, 8)?;
        let from = py::dget(&t, "from", 8)?;
        o.line(&format!(
            "{h}  (taken from --cmd service {}{TAKEN_TAIL}",
            py::s(&from)
        ));
    }
    let attached = py::or(py::dget(&v, "attached", 9)?, py::empty_list());
    for a in py::iter(&attached, 9)? {
        let h = host(&a, 10)?;
        let by = py::dget(&a, "addedBy", 10)?;
        let was = if py::truthy(&py::dget(&a, "previousTarget", 10)?) {
            py::concat("; was ", &py::idx(&a, "previousTarget", 10)?, 10)?
        } else {
            String::new()
        };
        o.line(&format!("{h}  (attached by {}{was})", py::s(&by)));
    }
    let customs = py::or(py::dget(&v, "custom", 11)?, py::empty_list());
    for c in py::iter(&customs, 11)? {
        let st = py::dget(&c, "state", 12)?;
        let name = py::dget(&c, "hostname", 13)?;
        let primary = if py::truthy(&py::dget(&c, "primary", 13)?) {
            "  primary"
        } else {
            ""
        };
        o.line(&format!(
            "{}   {}{primary}",
            py::s(&name),
            state_word(&st)
        ));
        o.line(&format!("  {}", py::s(&py::dget(&c, "words", 14)?)));
        if py::truthy(&py::dget(&c, "lastError", 15)?) {
            o.line(&format!(
                "  last check: {}",
                py::s(&py::idx(&c, "lastError", 16)?)
            ));
        }
        if py::truthy(&py::dget(&c, "path", 17)?) {
            let shows = if py_eq(&py::idx(&c, "path", 18)?, &py::text("connect")) {
                format!("Path B, through {CONNECT}")
            } else {
                "Path A, this box".to_string()
            };
            o.line(&format!("  DNS shows: {shows}"));
        }
        let setup = py::or(py::dget(&c, "setup", 19)?, empty_dict());
        let path_b = py::or(py::dget(&setup, "pathB", 19)?, empty_dict());
        let pr = py::or(py::dget(&path_b, "proxy", 19)?, empty_dict());
        if py_eq(&py::dget(&c, "path", 20)?, &py::text("connect"))
            && py_eq(&py::dget(&pr, "state", 20)?, &py::text("waiting"))
        {
            o.line(&format!("  {}", py::s(&py::dget(&pr, "words", 21)?)));
        }
        let relay = py::or(py::dget(&c, "relay", 22)?, empty_dict());
        let rs = py::dget(&relay, "state", 22)?;
        if py::truthy(&rs) {
            let word = py::word_for(&RELAY_WORDS, &rs, rs.clone(), 25)?;
            let relay = py::idx(&c, "relay", 25)?;
            let err = py::dget(&relay, "error", 25)?;
            let tail = if py::truthy(&err)
                && !py_eq(&py::idx(&relay, "error", 25)?, &py::dget(&c, "lastError", 25)?)
            {
                format!(
                    "{})",
                    py::concat(" (", &py::idx(&relay, "error", 25)?, 25)?
                )
            } else {
                String::new()
            };
            o.line(&format!("  {CONNECT}: {}{tail}", py::s(&word)));
        }
        let su = py::or(py::dget(&c, "setup", 26)?, empty_dict());
        if not_live(&st) {
            if py::truthy(&py::dget(&su, "message", 28)?) {
                o.line(&format!("  {}", py::s(&py::idx(&su, "message", 29)?)));
            }
            if py_eq(&py::dget(&su, "records", 30)?, &py::text("advanced")) {
                let adv = py::or(py::dget(&su, "advanced", 31)?, py::text("Advanced:"));
                o.line(&format!("  {}", py::s(&adv)));
            }
            if !py_eq(&py::dget(&su, "records", 32)?, &py::text("none")) {
                let records = py::or(py::dget(&c, "records", 33)?, py::empty_list());
                for r in py::iter(&records, 33)? {
                    o.line(&record_line(&r, 34)?);
                }
            }
        }
        if py::truthy(&py::dget(&c, "next", 35)?) {
            o.line(&format!("  {}", py::s(&py::idx(&c, "next", 36)?)));
        }
        if py::truthy(&py::dget(&c, "ownServer", 37)?) {
            o.line(&format!("  {}", py::s(&py::idx(&c, "ownServer", 38)?)));
        }
    }
    let pb = py::or(py::dget(&v, "pathB", 39)?, empty_dict());
    if py::truthy(&py::dget(&pb, "available", 40)?) {
        o.line(&format!(
            "Path B (through {CONNECT}): {} ({} custom names at most per box, both paths)",
            py::s(&py::dget(&pb, "meter", 41)?),
            py::s(&py::dget(&v, "maxCustomNames", 41)?)
        ));
    } else if py::truthy(&py::dget(&pb, "why", 42)?) {
        o.line(&format!(
            "Path B (through {CONNECT}): not open here: {}",
            py::s(&py::idx(&pb, "why", 43)?)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loops_read_the_words_as_the_shell_does() {
        assert_eq!(parse("domain", &[]), Some(Ok(Ask::Page)));
        assert!(crate::text::help(PAGE).is_some());
        assert_eq!(
            parse("domain", &["rm", "a", "b", "--external"]),
            Some(Err(Stop::usage(usage::EXTERNAL_ADD)))
        );
        assert_eq!(
            parse("domain", &["external", "a", "h", "maybe"]),
            Some(Err(Stop::usage(usage::EXTERNAL)))
        );
        assert_eq!(
            parse("domain", &["add", "a", "shop", "--transfer"]),
            Some(Ok(Ask::Label {
                verb: "add",
                body: r#"{"app": "a", "label": "shop", "transfer": true}"#.into(),
                json: false
            }))
        );
        assert_eq!(
            parse("domain", &["list"]),
            Some(Err(Stop::usage(usage::DOMAINS)))
        );
    }

    #[test]
    fn a_host_is_its_label_on_the_box() {
        let r = crate::out::json::parse(r#"{"label":"shop"}"#).unwrap();
        assert_eq!(host(&r, 6).unwrap(), format!("shop.<box>{ZONE_SUFFIX}"));
        let r = crate::out::json::parse(r#"{}"#).unwrap();
        assert!(host(&r, 6).is_err());
    }
}
