//! The cross-over verbs, as the shell CLI answers them
//! (`cli/src/app-updates.sh`, `cli/src/app-door.sh`,
//! `cli/src/app-grace.sh`): `app warnings`, `app clean-mode`, `app key
//! mint|revoke|rm|list|ls`, `app keys`, `app label take|return`, `app
//! test-guests` (and `test-guest`), `app move-plan`, and the pages `app
//! surfaces`, `app door` and `app grace` (and `crossover`).
//!
//! Each is the shell's word loop, one request (`_compute_call` for a read,
//! `_app_rt_post` for a change, then `_app_grace_done`: a held change is
//! its lines and exit 2), `--json` as it came, else the shell's Python
//! line for line.
//!
//! `app key mint` is the owner's confirm when the daemon asks for one
//! outside a session (`vault_confirm_required`): the confirm token is
//! asked over the vault socket and goes only in the second POST's body.
//! The key the daemon shows once (`--show`) is printed as the shell
//! prints it, and nothing else holds it.

use super::call::{Cj, Ctx, Split};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_len, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::ops::py_eq;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const CLEAN_MODE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app clean-mode <app> on|off [--restart]"
    );
    pub const KEY_REVOKE: &str = concat!("usage: ", cli_name!(), " app key revoke <key-id>");
    pub const KEY_MINT: &str = concat!(
        "usage: ",
        cli_name!(),
        " app key mint <app> --scopes users,rooms,fetch,push [--rooms a,b] [--roles r] [--label L] [--show]"
    );
    pub const LABEL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app label take|return <app-id> <label> [--json]"
    );
    pub const LABEL_VERB: &str = concat!("usage: ", cli_name!(), " app label ");
    pub const TEST_GUESTS: &str = concat!(
        "usage: ",
        cli_name!(),
        " app test-guests <app-id> [--json]"
    );
    pub const MOVE_PLAN: &str = concat!(
        "usage: ",
        cli_name!(),
        " app move-plan <service> [--app <app-id>] [--request] [--json]"
    );
}

/// The pages: `_app_surfaces_usage`, `_app_door_usage`,
/// `_app_grace_usage`.
pub const SURFACES_PAGE: &str = "app/surfaces";
pub const DOOR_PAGE: &str = "app/door";
pub const GRACE_PAGE: &str = "app/grace";

/// `"owner-token tracing: %s (Linux only for now; the code scan still
/// names reads of ~/.k2/daemon.token)"`: the path is the daemon's file, a
/// stored name.
const TRACING_TAIL: &str =
    " (Linux only for now; the code scan still names reads of ~/.k2/daemon.token)";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A page on stdout, exit 0.
    Page(&'static str),
    Warnings {
        app: String,
        json: bool,
    },
    CleanMode {
        app: String,
        on: String,
        restart: bool,
        json: bool,
    },
    KeyRevoke {
        id: String,
    },
    KeyMint {
        app: String,
        scopes: String,
        rooms: String,
        roles: String,
        label: String,
        show: bool,
        json: bool,
    },
    Keys {
        app: String,
        json: bool,
    },
    Label {
        verb: &'static str,
        app: String,
        label: String,
        json: bool,
    },
    TestGuests {
        app: String,
        json: bool,
    },
    MovePlan {
        service: String,
        app: String,
        request: bool,
        reason: String,
        json: bool,
    },
}

/// The verbs here.
pub const VERBS: &[&str] = &[
    "warnings",
    "clean-mode",
    "key",
    "keys",
    "label",
    "test-guests",
    "test-guest",
    "move-plan",
    "surfaces",
    "door",
    "grace",
    "crossover",
];

/// What a word loop read: each value flag's last value (by its first
/// name), the switches seen, and the plain words in order.
#[derive(Debug, Default)]
pub struct Words {
    values: Vec<(&'static str, String)>,
    switches: Vec<&'static str>,
    pub plain: Vec<String>,
}

impl Words {
    /// The flag's last value; empty when it was not given.
    pub fn v(&self, flag: &str) -> String {
        self.values
            .iter()
            .rev()
            .find(|(k, _)| *k == flag)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// Whether the flag was given (with any value).
    pub fn given(&self, flag: &str) -> bool {
        self.values.iter().any(|(k, _)| *k == flag)
    }

    /// Every value the flag was given, in order.
    pub fn all(&self, flag: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(k, _)| *k == flag)
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn has(&self, switch: &str) -> bool {
        self.switches.contains(&switch)
    }

    /// The last plain word (`*) app="$1"`), empty when none.
    pub fn last(&self) -> String {
        self.plain.last().cloned().unwrap_or_default()
    }
}

/// The shell's usual loop: `--<flag>) x="${2:-}"; shift 2` for each of
/// `values` (each a name and the names that mean it too), the `switches`,
/// `-*` an unknown flag (exit 2), any other word plain. A value flag given
/// last ends the command as `set -e` does (`shift 2` with one word left:
/// exit 1, nothing said).
pub fn walk(
    words: &[&str],
    values: &[&[&'static str]],
    switches: &[&'static str],
) -> Result<Words, Stop> {
    let mut out = Words::default();
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        if let Some(names) = values.iter().find(|n| n.iter().any(|x| *x == w)) {
            let Some(v) = rest.get(1) else {
                return Err(Stop::silent(Exit::Error));
            };
            out.values.push((names[0], v.to_string()));
            rest = &rest[2..];
            continue;
        }
        if let Some(s) = switches.iter().find(|s| **s == w) {
            out.switches.push(*s);
        } else if w.starts_with('-') {
            return Err(Stop::usage(format!("Unknown flag: {w}")));
        } else {
            out.plain.push(w.to_string());
        }
        rest = &rest[1..];
    }
    Ok(out)
}

/// A help word anywhere: the shell's (its page, and no request).
pub fn help(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// The words after the verb. `None`: a help form (the shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if help(words) {
        return None;
    }
    Some(match verb {
        "surfaces" => Ok(Ask::Page(SURFACES_PAGE)),
        "door" => Ok(Ask::Page(DOOR_PAGE)),
        "grace" | "crossover" => Ok(Ask::Page(GRACE_PAGE)),
        "warnings" => walk(words, &[], &["--json"]).map(|w| Ask::Warnings {
            app: w.last(),
            json: w.has("--json"),
        }),
        "clean-mode" => clean_mode(words),
        "key" => key(words),
        "keys" => keys(words),
        "label" => label(words),
        "test-guests" | "test-guest" => test_guests(words),
        "move-plan" => move_plan(words),
        _ => return None,
    })
}

fn clean_mode(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut on, mut restart, mut json) = (String::new(), String::new(), false, false);
    for w in words {
        match *w {
            "--restart" => restart = true,
            "--json" => json = true,
            "on" | "off" => on = w.to_string(),
            _ if w.starts_with('-') => return Err(Stop::usage(format!("Unknown flag: {w}"))),
            _ => app = w.to_string(),
        }
    }
    if app.is_empty() || on.is_empty() {
        return Err(Stop::usage(usage::CLEAN_MODE));
    }
    Ok(Ask::CleanMode {
        app,
        on,
        restart,
        json,
    })
}

/// `_app_grace_usage >&2; exit 2`.
fn grace_page_stop() -> Stop {
    let page = crate::text::help(GRACE_PAGE).unwrap_or_default();
    Stop::usage(page.strip_suffix('\n').unwrap_or(page))
}

fn key(words: &[&str]) -> Result<Ask, Stop> {
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match sub {
        "mint" => key_mint(rest),
        "revoke" | "rm" => {
            let id = rest.first().copied().unwrap_or("");
            if id.is_empty() {
                return Err(Stop::usage(usage::KEY_REVOKE));
            }
            Ok(Ask::KeyRevoke { id: id.to_string() })
        }
        "list" | "ls" => keys(rest),
        _ => Err(grace_page_stop()),
    }
}

fn key_mint(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(
        words,
        &[
            &["--scopes", "--scope"],
            &["--rooms", "--room"],
            &["--roles", "--role"],
            &["--label"],
        ],
        &["--show", "--json"],
    )?;
    let (app, scopes) = (w.last(), w.v("--scopes"));
    if app.is_empty() || scopes.is_empty() {
        return Err(Stop::usage(usage::KEY_MINT));
    }
    Ok(Ask::KeyMint {
        app,
        scopes,
        rooms: w.v("--rooms"),
        roles: w.v("--roles"),
        label: w.v("--label"),
        show: w.has("--show"),
        json: w.has("--json"),
    })
}

fn keys(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(words, &[], &["--json"])?;
    Ok(Ask::Keys {
        app: w.last(),
        json: w.has("--json"),
    })
}

fn label(words: &[&str]) -> Result<Ask, Stop> {
    let verb = match words.first().copied() {
        Some("take") => "take",
        Some("return") => "return",
        _ => return Err(Stop::usage(usage::LABEL)),
    };
    let w = walk(&words[1..], &[], &["--json"])?;
    let (mut app, mut label) = (String::new(), String::new());
    for p in &w.plain {
        if app.is_empty() {
            app = p.clone();
        } else {
            label = p.clone();
        }
    }
    if app.is_empty() || label.is_empty() {
        return Err(Stop::usage(format!(
            "{}{verb} <app-id> <label>",
            usage::LABEL_VERB
        )));
    }
    Ok(Ask::Label {
        verb,
        app,
        label,
        json: w.has("--json"),
    })
}

fn test_guests(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(words, &[], &["--json"])?;
    let app = w.last();
    if app.is_empty() {
        return Err(Stop::usage(usage::TEST_GUESTS));
    }
    Ok(Ask::TestGuests {
        app,
        json: w.has("--json"),
    })
}

fn move_plan(words: &[&str]) -> Result<Ask, Stop> {
    let w = walk(
        words,
        &[&["--app"], &["--reason"]],
        &["--request", "--json"],
    )?;
    let service = w.last();
    if service.is_empty() {
        return Err(Stop::usage(usage::MOVE_PLAN));
    }
    Ok(Ask::MovePlan {
        service,
        app: w.v("--app"),
        request: w.has("--request"),
        reason: w.v("--reason"),
        json: w.has("--json"),
    })
}

/// `--json`: the answer as it came; else the printer.
fn printed(body: &[u8], json: bool, lines: fn(&[u8], &mut Out) -> Result<(), Raise>) -> Shown {
    if json {
        return raw(body);
    }
    let mut o = Out::default();
    let r = lines(body, &mut o).map(|()| Exit::Ok);
    o.done(r)
}

/// `_app_rt_post <path> <body>; _app_grace_done <json>`: the 200's body,
/// or the answer the command ends with.
fn post_done(ctx: &mut Ctx, path: &str, body: &str, json: bool) -> Result<Vec<u8>, Answer> {
    let s = ctx.send(true, path, body, false).map_err(Err)?;
    super::grace::done(ctx, s, json)
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Page(p) => Ok(Shown::out(crate::text::help(p).unwrap_or_default())),
        Ask::Warnings { app, json } => {
            let q = if app.is_empty() {
                String::new()
            } else {
                format!("app={}", quote(app))
            };
            let body = ctx.call(false, "/cli/app/warnings", &q)?;
            Ok(printed(&body, *json, warnings_lines))
        }
        Ask::CleanMode {
            app,
            on,
            restart,
            json,
        } => {
            let b = Cj::new()
                .s("app", app)
                .s("on", on)
                .b("restart", if *restart { "1" } else { "0" })
                .body();
            match post_done(ctx, "/cli/app/clean-mode", &b, *json) {
                Ok(body) => Ok(printed(&body, *json, clean_mode_lines)),
                Err(a) => a,
            }
        }
        Ask::KeyRevoke { id } => {
            let b = Cj::new().s("id", id).body();
            match post_done(ctx, "/cli/app/key/revoke", &b, false) {
                Ok(body) => Ok(printed(&body, false, revoke_lines)),
                Err(a) => a,
            }
        }
        Ask::KeyMint { .. } => key_mint_run(ctx, ask),
        Ask::Keys { app, json } => {
            let q = if app.is_empty() {
                String::new()
            } else {
                format!("app={}", quote(app))
            };
            let body = ctx.call(false, "/cli/app/keys", &q)?;
            Ok(printed(&body, *json, keys_lines))
        }
        Ask::Label {
            verb,
            app,
            label,
            json,
        } => {
            let b = Cj::new().s("app", app).s("label", label).body();
            match post_done(ctx, &format!("/cli/app/label/{verb}"), &b, *json) {
                Ok(body) => Ok(printed(&body, *json, label_lines)),
                Err(a) => a,
            }
        }
        Ask::TestGuests { app, json } => {
            let body = ctx.call(false, "/cli/app/test-guests", &format!("app={}", quote(app)))?;
            Ok(printed(&body, *json, test_guests_lines))
        }
        Ask::MovePlan {
            service,
            app,
            request,
            reason,
            json,
        } => move_plan_run(ctx, service, app, *request, reason, *json),
    }
}

/// `[x.strip() for x in s.split(",") if x.strip()]`.
pub fn split_strip(s: &str) -> Vec<Value> {
    s.split(',')
        .map(|x| x.trim_matches(crate::families::py_space))
        .filter(|x| !x.is_empty())
        .map(py::text)
        .collect()
}

fn key_mint_run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let Ask::KeyMint {
        app,
        scopes,
        rooms,
        roles,
        label,
        show,
        json,
    } = ask
    else {
        return Err(Exit::Error);
    };
    const PATH: &str = "/cli/app/key/mint";
    let mut pairs = vec![
        (Text::from("app"), py::text(app)),
        (Text::from("scopes"), Value::Array(split_strip(scopes))),
        (Text::from("rooms"), Value::Array(split_strip(rooms))),
        (Text::from("roles"), Value::Array(split_strip(roles))),
        (
            Text::from("deliver"),
            py::text(if *show { "show" } else { "vault" }),
        ),
    ];
    if !label.is_empty() {
        pairs.push((Text::from("label"), py::text(label)));
    }
    let body = json::to_string(&Value::Object(pairs), &Options::default())
        .unwrap_or_else(|_| "{}".to_string());
    let mut s = ctx.send(true, PATH, &body, false)?;
    if wants_confirm(ctx, &s) {
        let svc = super::phone_set::error_field(&s.body, &["serviceId"])?;
        if svc.is_empty() {
            return Err(ctx.fail(&s));
        }
        let Some(tok) = crate::confirm::confirm_token(
            &mut ctx.confirm(),
            "app-key-mint",
            &svc,
            &format!("App key for {app}"),
        ) else {
            return Err(Exit::Refused);
        };
        let Some(next) = crate::confirm::with_confirm_token(&body, &tok, None) else {
            return Err(Exit::Error);
        };
        drop(tok);
        s = ctx.send(true, PATH, &next, false)?;
    }
    match super::grace::done(ctx, s, *json) {
        Ok(body) => Ok(printed(&body, *json, mint_lines)),
        Err(a) => a,
    }
}

/// A 403 that asks for the owner's confirm, outside a session.
fn wants_confirm(ctx: &Ctx, s: &Split) -> bool {
    const WORD: &[u8] = b"\"vault_confirm_required\"";
    s.status == "403" && s.body.windows(WORD.len()).any(|w| w == WORD) && !ctx.in_session()
}

fn move_plan_run(
    ctx: &mut Ctx,
    service: &str,
    app: &str,
    request: bool,
    reason: &str,
    json: bool,
) -> Answer {
    let body = if request {
        let b = Cj::new()
            .s("service", service)
            .s("app", app)
            .s("reason", reason)
            .body();
        let s = ctx.send(true, "/cli/app/move-plan/request", &b, false)?;
        if !matches!(s.status.as_str(), "200" | "202") {
            return Err(ctx.fail(&s));
        }
        s.body
    } else {
        let mut q = format!("service={}", quote(service));
        if !app.is_empty() {
            q.push_str(&format!("&app={}", quote(app)));
        }
        ctx.call(false, "/cli/app/move-plan", &q)?
    };
    let mut shown = printed(&body, json, move_plan_lines);
    if !request || shown.exit != Exit::Ok {
        return Ok(shown);
    }
    // `sys.exit(2 if json.load(sys.stdin).get("status") == "held" else 0)
    // || exit $?`.
    let r = (|| -> Result<Exit, Raise> {
        let v = py::load(&body, 1)?;
        let held = py_eq(&py::dget(&v, "status", 1)?, &py::text("held"));
        Ok(if held { Exit::Usage } else { Exit::Ok })
    })();
    match r {
        Ok(exit) => shown.exit = exit,
        Err(e) => {
            let tail = crate::families::raised(String::new(), String::new(), &e);
            shown.stderr.push_str(&tail.stderr);
            shown.exit = tail.exit;
        }
    }
    Ok(shown)
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

fn on_off(v: &Value) -> &'static str {
    if py::truthy(v) {
        "on"
    } else {
        "off"
    }
}

/// `cmd_app_warnings`' Python.
fn warnings_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let reports = if py::contains(&v, "services", 4)? {
        py::dget(&v, "services", 4)?
    } else {
        Value::Array(vec![v.clone()])
    };
    if !py::truthy(&reports) {
        o.line("No server Apps (--cmd services) to check.");
    }
    let mut tracing = py::dget(&v, "ownerTokenAttribution", 7)?;
    if !py::truthy(&tracing) {
        tracing = Value::Null;
        for r in py::iter(&reports, 7)? {
            let t = py::dget(&r, "ownerTokenAttribution", 7)?;
            if py::truthy(&t) {
                tracing = py::dget(&r, "ownerTokenAttribution", 7)?;
                break;
            }
        }
    }
    for r in py::iter(&reports, 8)? {
        let a = py::dget_or(&r, "app", empty_dict(), 9)?;
        let mut mode = py::or(py::dget(&a, "envMode", 10)?, py::text("-"));
        if py::truthy(&py::dget(&a, "cleanMode", 11)?) {
            mode = py::text("clean mode");
        }
        let status = if py::truthy(&py::dget(&r, "readyForCutover", 14)?) {
            "ready to move to an App".to_string()
        } else {
            let n = py::dget_or(&r, "breaks", Value::Int("0".into()), 17)?;
            format!(
                "{} thing(s) break when it moves to an App",
                crate::families::py_d(&n, 17)?
            )
        };
        let name = py::or(py::dget(&a, "handle", 18)?, py::dget(&a, "name", 18)?);
        o.line(&format!("{}  [{}]  {status}", py::s(&name), py::s(&mode)));
        let warnings = py::dget_or(&r, "warnings", py::empty_list(), 19)?;
        for w in py::iter(&warnings, 19)? {
            let sev = py::dget(&w, "severity", 20)?;
            let word = py::word_for(
                &[("breaks", "BREAKS"), ("expected", "expected")],
                &sev,
                py::text("check"),
                21,
            )?;
            let msg = py::dget(&w, "message", 21)?;
            o.line(&format!("  {} {}", py::left(&word, 8), py::s(&msg)));
            if py::truthy(&py::dget(&w, "fix", 22)?) {
                o.line(&format!(
                    "           fix: {}",
                    py::s(&py::idx(&w, "fix", 23)?)
                ));
            }
        }
        let sc = py::or(py::dget(&r, "scan", 24)?, empty_dict());
        if py::truthy(&py::dget(&sc, "truncated", 25)?) {
            o.line(&format!(
                "  (the code scan stopped early: {} files)",
                py::s(&py::dget(&sc, "filesScanned", 26)?)
            ));
        }
    }
    if py::truthy(&tracing) && !py_eq(&tracing, &py::text("on")) {
        o.line(&format!(
            "owner-token tracing: {}{TRACING_TAIL}",
            py::s(&tracing)
        ));
    }
    Ok(())
}

/// `cmd_app_clean_mode`'s Python.
fn clean_mode_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let handle = py::dget(&py::idx(&v, "app", 4)?, "handle", 4)?;
    let mode = on_off(&py::dget(&v, "cleanMode", 4)?);
    let env = py::dget(&v, "envMode", 4)?;
    let restart = py::dget(&v, "restart", 4)?;
    o.line(&format!(
        "{}: clean mode {mode} (env {}); {}",
        py::s(&handle),
        py::s(&env),
        py::s(&restart)
    ));
    if py::truthy(&py::dget(&v, "ownerTokenAttributionNote", 5)?) {
        let t = py::dget(&v, "ownerTokenAttribution", 6)?;
        let note = py::idx(&v, "ownerTokenAttributionNote", 6)?;
        o.line(&format!(
            "owner-token tracing: {} ({})",
            py::s(&t),
            py::s(&note)
        ));
    }
    Ok(())
}

/// `cmd_app_key revoke`'s Python.
fn revoke_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let word = if py::truthy(&py::dget(&v, "revoked", 4)?) {
        "revoked"
    } else {
        "already revoked:"
    };
    let id = py::dget(&v, "id", 4)?;
    let handle = py::dget(&py::idx(&v, "app", 4)?, "handle", 4)?;
    o.line(&format!("{word} {} ({})", py::s(&id), py::s(&handle)));
    Ok(())
}

/// `cmd_app_key_mint`'s Python.
fn mint_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let id = py::dget(&v, "id", 4)?;
    let handle = py::dget(&py::idx(&v, "app", 4)?, "handle", 4)?;
    let scopes = py::join(",", &py::or(py::dget(&v, "scopes", 4)?, py::empty_list()), 4)?;
    o.line(&format!(
        "App key {} for {}: scopes {scopes}",
        py::s(&id),
        py::s(&handle)
    ));
    o.line(&format!(
        "  calls: {} (Authorization: Bearer <key>)",
        py::s(&py::dget(&v, "routes", 5)?)
    ));
    let d = py::dget(&v, "delivered", 6)?;
    if py::truthy(&d) {
        let secret = py::dget(&d, "vaultSecret", 8)?;
        let file = py::dget(&d, "file", 8)?;
        o.line(&format!(
            "  in the vault as {}; the App reads $K2_SECRETS_DIR/{}",
            py::s(&secret),
            py::s(&file)
        ));
        o.line(&format!("  {}", py::s(&py::dget(&d, "note", 9)?)));
    }
    if py::truthy(&py::dget(&v, "key", 10)?) {
        o.line(&format!(
            "  key (shown once): {}",
            py::s(&py::idx(&v, "key", 11)?)
        ));
    }
    Ok(())
}

/// `cmd_app_keys`' Python.
fn keys_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let ks = py::or(py::dget(&v, "keys", 4)?, py::empty_list());
    if !py::truthy(&ks) {
        o.line("No App keys.");
        return Ok(());
    }
    for k in py::iter(&ks, 7)? {
        let id = py::idx(&k, "id", 8)?;
        let state = py::idx(&k, "state", 8)?;
        let label = py::head(&py::or(py::dget(&k, "label", 8)?, py::text("-")), 24, 8)?;
        let scopes = py::join(",", &py::or(py::dget(&k, "scopes", 8)?, py::empty_list()), 8)?;
        let rooms = py::join(",", &py::or(py::dget(&k, "rooms", 8)?, py::empty_list()), 8)?;
        let rooms = if rooms.is_empty() { "-".to_string() } else { rooms };
        o.line(&format!(
            "{}  {} {} scopes={scopes} rooms={rooms}",
            py::s(&id),
            py::left(&state, 8),
            py::left(&label, 24)
        ));
    }
    Ok(())
}

/// `", ".join(<values>)` of a list Python built.
fn join_values(items: Vec<Value>, line: u32) -> Result<String, Raise> {
    py::join(", ", &Value::Array(items), line)
}

/// `cmd_app_label`'s Python.
fn label_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "already", 4)?) {
        o.line(&format!(
            "{} already serves {}",
            py::s(&py::dget(&v, "app", 5)?),
            py::s(&py::dget(&v, "label", 5)?)
        ));
        return Ok(());
    }
    if py::truthy(&py::dget(&v, "returnedTo", 6)?) {
        let what = if py::truthy(&py::dget(&v, "site", 7)?) {
            "--skin site"
        } else {
            "legacy service"
        };
        let label = py::dget(&v, "label", 8)?;
        let port = py::dget(&py::idx(&v, "returnedTo", 8)?, "port", 8)?;
        let app = py::dget(&v, "app", 8)?;
        o.line(&format!(
            "{} serves the {what} again (port {}); App {} no longer has it",
            py::s(&label),
            py::s(&port),
            py::s(&app)
        ));
        if py::truthy(&py::dget(&v, "note", 9)?) {
            o.line(&py::s(&py::idx(&v, "note", 10)?));
        }
        return Ok(());
    }
    o.line(&format!(
        "{} now serves App {} (front port {})",
        py::s(&py::dget(&v, "label", 12)?),
        py::s(&py::dget(&v, "app", 12)?),
        py::s(&py::dget(&v, "frontPort", 12)?)
    ));
    let c = py::dget(&v, "copied", 13)?;
    if c != Value::Null {
        let added = py::or(py::dget(&c, "added", 15)?, py::empty_list());
        let n = py_len(&added, 15)?;
        let added = py::or(py::dget(&c, "added", 15)?, py::empty_list());
        let names = py::join(", ", &added, 15)?;
        let names = if names.is_empty() {
            "none".to_string()
        } else {
            names
        };
        o.line(&format!("guests copied onto the App: {n} ({names})"));
        if py::truthy(&py::dget(&c, "keptExisting", 16)?) {
            let mut users = Vec::new();
            for k in py::iter(&py::idx(&c, "keptExisting", 17)?, 17)? {
                users.push(py::dget_or(&k, "user", py::text(""), 17)?);
            }
            o.line(&format!(
                "already on the App (kept as they were): {}",
                join_values(users, 17)?
            ));
        }
        for (k, why) in [
            ("skippedOtherAppRole", "their role is another App\u{2019}s"),
            (
                "skippedOtherWorkspaceAgent",
                "another workspace\u{2019}s agent added them",
            ),
        ] {
            if py::truthy(&py::dget(&c, k, 19)?) {
                o.line(&format!(
                    "not copied ({why}): {}",
                    py::join(", ", &py::idx(&c, k, 20)?, 20)?
                ));
            }
        }
        let roles = py::or(py::dget(&c, "rolesCopied", 21)?, py::empty_list());
        for r in py::iter(&roles, 21)? {
            o.line(&format!(
                "role {} copied for the App as {}",
                py::s(&py::dget(&r, "from", 22)?),
                py::s(&py::dget(&r, "to", 22)?)
            ));
        }
    }
    let not_copied = py::or(py::dget(&v, "notCopied", 23)?, py::empty_list());
    for n in py::iter(&not_copied, 23)? {
        o.line(&format!("not copied: {}", py::s(&n)));
    }
    if py::truthy(&py::dget(&v, "note", 25)?) {
        o.line(&py::s(&py::idx(&v, "note", 26)?));
    }
    Ok(())
}

/// `cmd_app_test_guests`' Python.
fn test_guests_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let g = py::or(py::dget(&v, "testGuests", 4)?, py::empty_list());
    let app = py::idx(&v, "app", 5)?;
    let n = py_len(&g, 5)?;
    let max = py::dget(&v, "max", 5)?;
    o.line(&format!(
        "{}: {n} test guest{} (at most {})",
        py::s(&app),
        if n == 1 { "" } else { "s" },
        py::s(&max)
    ));
    for t in py::iter(&g, 6)? {
        let user = py::idx(&t, "username", 7)?;
        let when = if py::truthy(&py::dget(&t, "expired", 7)?) {
            "expired".to_string()
        } else {
            format!(
                "ends {}",
                py::local_minutes(&py::idx(&t, "expiresAt", 7)?, 7)?
            )
        };
        let by = py::dget(&t, "createdBy", 7)?;
        o.line(&format!("  {}  {when}  by {}", py::s(&user), py::s(&by)));
    }
    Ok(())
}

/// `cmd_app_move_plan`'s Python.
fn move_plan_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let p = py::or(py::dget(&v, "plan", 4)?, v.clone());
    let s = py::idx(&p, "service", 5)?;
    let a = py::idx(&p, "app", 5)?;
    let ws = py::idx(&s, "workspaceHandle", 6)?;
    let name = py::idx(&s, "name", 6)?;
    let port = py::idx(&s, "port", 6)?;
    let id = py::idx(&a, "id", 6)?;
    let tail = if py::truthy(&py::idx(&a, "installed", 7)?) {
        ""
    } else {
        " (not installed; a suggested id: --app to pick)"
    };
    o.line(&format!(
        "Move {}/{} (port {}) into App {}{tail}",
        py::s(&ws),
        py::s(&name),
        py::s(&port),
        py::s(&id)
    ));
    const MARK: [(&str, &str); 4] = [
        ("done", "[x]"),
        ("todo", "[ ]"),
        ("blocked", "[.]"),
        ("n/a", "[-]"),
    ];
    for st in py::iter(&py::idx(&p, "steps", 9)?, 9)? {
        let state = py::idx(&st, "state", 10)?;
        let mark = py::word_for(&MARK, &state, py::text("[?]"), 10)?;
        let who = py::idx(&st, "who", 10)?;
        let title = py::idx(&st, "title", 10)?;
        let detail = py::idx(&st, "detail", 10)?;
        o.line(&format!(
            "{} {} {}: {}",
            py::s(&mark),
            py::left(&who, 6),
            py::s(&title),
            py::s(&detail)
        ));
        if py::truthy(&py::dget(&st, "users", 11)?) {
            o.line(&format!(
                "             users: {}",
                py::join(", ", &py::idx(&st, "users", 12)?, 12)?
            ));
        }
        if py::truthy(&py::dget(&st, "roomHandles", 13)?) {
            o.line(&format!(
                "             rooms: {}",
                py::join(", ", &py::idx(&st, "roomHandles", 14)?, 14)?
            ));
        }
        let state = py::idx(&st, "state", 15)?;
        let open = py_eq(&state, &py::text("todo")) || py_eq(&state, &py::text("blocked"));
        if open && py::truthy(&py::dget(&st, "command", 15)?) {
            o.line(&format!(
                "             {}",
                py::s(&py::idx(&st, "command", 16)?)
            ));
        }
    }
    o.line("[x] done  [ ] to do  [.] waits on an earlier step  [-] n/a");
    let status = py::dget(&v, "status", 18)?;
    if py_eq(&status, &py::text("held")) {
        let id = py::idx(&py::idx(&v, "request", 19)?, "id", 19)?;
        let todo = py::or(py::dget(&p, "ownerStepsTodo", 19)?, py::empty_list());
        let n = py_len(&todo, 19)?;
        let hint = py::dget_or(&v, "hint", py::text(""), 19)?;
        o.line(&format!(
            "Held for the owner: request {} (one approval for {n} step(s)). {}",
            py::s(&id),
            py::s(&hint)
        ));
    } else if py_eq(&py::dget(&v, "status", 20)?, &py::text("nothing_to_request")) {
        o.line(&py::s(&py::dget(&v, "note", 21)?));
    } else if py::truthy(&py::dget(&p, "request", 22)?) {
        let todo = py::join(", ", &py::idx(&p, "ownerStepsTodo", 23)?, 23)?;
        let req = py::idx(&p, "request", 23)?;
        o.line(&format!(
            "Owner steps to do: {todo}. One approval for all of them: {}",
            py::s(&req)
        ));
    } else if py::truthy(&py::dget(&p, "done", 24)?) {
        o.line("The move is done.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(verb: &str, words: &[&str]) -> Result<Ask, Stop> {
        parse(verb, words).expect("not a help form")
    }

    #[test]
    fn the_loops_read_the_words_as_the_shell_does() {
        assert_eq!(
            parsed("key", &["mint", "a", "--scope", "users", "--label"]),
            Err(Stop::silent(Exit::Error))
        );
        assert_eq!(
            parsed("key", &[]).unwrap_err().exit,
            Exit::Usage,
            "the page on stderr"
        );
        assert_eq!(
            parsed("clean-mode", &["off", "a", "--restart"]),
            Ok(Ask::CleanMode {
                app: "a".into(),
                on: "off".into(),
                restart: true,
                json: false
            })
        );
        assert_eq!(
            parsed("label", &["take", "a", "b", "c"]),
            Ok(Ask::Label {
                verb: "take",
                app: "a".into(),
                label: "c".into(),
                json: false
            })
        );
        assert!(parse("door", &["x", "-h"]).is_none());
        assert_eq!(parsed("crossover", &["x"]), Ok(Ask::Page(GRACE_PAGE)));
        assert_eq!(
            split_strip(" a, ,b ,\u{a0}"),
            vec![py::text("a"), py::text("b")]
        );
    }

    #[test]
    fn the_pages_are_there() {
        for p in [SURFACES_PAGE, DOOR_PAGE, GRACE_PAGE] {
            assert!(crate::text::help(p).is_some(), "{p}");
        }
    }
}
