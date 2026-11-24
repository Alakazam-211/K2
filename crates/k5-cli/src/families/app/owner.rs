//! Who keeps an App, as the shell CLI answers it (`cli/src/app-doctor.sh`):
//! `app uninstall`, `app expose`, `app handover` (and its `accept`,
//! `decline`, `cancel`, `undo`, `finish`, `rekey`, `status`, `brief` and
//! `plan`), `app unpark`, `app park`, `app release` and `app exception`.
//!
//! - The owner in person: `uninstall --purge` and `exception <app> on`
//!   ask for a confirm token first (`_vault_confirm_token`, over the vault
//!   socket: [`crate::confirm::confirm_token`]); `handover`, `unpark` and
//!   `release` ask for one only when the daemon answers 403
//!   `vault_confirm_required` naming the action and the id it binds
//!   (`_app_post_confirming`), then send again with it. No confirm: exit
//!   3. The token rides in the body (first, as `_compute_json` puts a
//!   token), never in argv or the environment.
//! - A handover offer or card (202) is exit 2; the owner at a terminal
//!   outside a session approves the card right there (`_app_approve`),
//!   unless `NO_APPROVE` is `1`.
//! - `handover`'s usage is its page on stderr, exit 2.

use super::call::{Ctx, Given};
use super::deploy::{post_200, value};
use super::py::{self, Out};
use super::wallet::raw;
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::{py_iter, py_len, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// `handover`'s page (`_app_handover_usage`, on stderr).
pub const HANDOVER_PAGE: &str = "app/handover";

/// The usage lines (exit 2).
pub mod usage {
    use super::cli_name;
    pub const UNINSTALL: &str = concat!(
        "usage: ",
        cli_name!(),
        " app uninstall <app-id> [--purge] [--drop-access]"
    );
    pub const EXPOSE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app expose <app-id> --tunnel [--transfer] | --local"
    );
    pub const EXPOSE_ONE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app expose <app-id> --tunnel|--local (one App)"
    );
    pub const EXPOSE_BOTH: &str = "usage: --tunnel or --local, not both";
    pub const EXPOSE_TRANSFER: &str = "usage: --transfer goes with --tunnel";
    pub const PARK: &str = concat!("usage: ", cli_name!(), " app park <app-id> [--reason TEXT]");
    pub const RELEASE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app release <workspace> --apps handover:<workspace>|uninstall|park [--drop-access]"
    );
    pub const RELEASE_DROP: &str = "usage: --drop-access goes with --apps uninstall";
    pub const EXCEPTION: &str = concat!(
        "usage: ",
        cli_name!(),
        " app exception <app-id> on [--device dri-render128]... [--reason TEXT] | off"
    );
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Uninstall {
        app: String,
        purge: bool,
        drop: bool,
        json: bool,
    },
    Expose {
        body: String,
        json: bool,
    },
    /// `handover`'s page on stderr, exit 2.
    HandoverUsage,
    Handover(Handover),
    Park {
        app: String,
        body: String,
        json: bool,
    },
    Release {
        ws: String,
        apps: String,
        drop: bool,
        json: bool,
    },
    Exception {
        app: String,
        on: bool,
        devices: Vec<String>,
        reason: String,
        json: bool,
    },
}

/// `cmd_app_handover`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Handover {
    /// `handover` or `unpark`.
    pub verb: String,
    /// The sub-act, or empty.
    pub sub: String,
    pub app: String,
    pub to: String,
    /// `--brief FILE` given (its bytes in `brief`).
    pub brief_file: bool,
    pub brief: Vec<u8>,
    pub db: String,
    pub coll: String,
    pub reset_shared: bool,
    pub copy_source: bool,
    pub reason: String,
    pub dry: bool,
    pub json: bool,
    pub detail: bool,
}

/// The verbs here.
pub const VERBS: &[&str] = &[
    "uninstall",
    "expose",
    "handover",
    "unpark",
    "park",
    "release",
    "exception",
];

/// The words after the verb. `None`: a help form, or a brief file that
/// can't be read (the shell's: `cat` fails under `set -e`).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if verb == "handover" && words.is_empty() {
        return Some(Ok(Ask::HandoverUsage));
    }
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    match verb {
        "uninstall" => Some(uninstall(words)),
        "expose" => Some(expose(words)),
        "park" => Some(park(words)),
        "release" => Some(release(words)),
        "exception" => Some(exception(words)),
        _ => handover(verb, words),
    }
}

fn unknown(w: &str) -> Stop {
    Stop::usage(format!("Unknown flag: {w}"))
}

fn uninstall(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut purge, mut drop, mut json) = ("", false, false, false);
    for w in words {
        match *w {
            "--purge" => purge = true,
            "--drop-access" => drop = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => app = w,
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::UNINSTALL));
    }
    Ok(Ask::Uninstall {
        app: app.to_string(),
        purge,
        drop,
        json,
    })
}

fn expose(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut mode, mut transfer, mut json) = ("", "", false, false);
    for w in words {
        match *w {
            "--tunnel" | "--local" => {
                let m = &w[2..];
                if !mode.is_empty() && mode != m {
                    return Err(Stop::usage(usage::EXPOSE_BOTH));
                }
                mode = m;
            }
            "--transfer" => transfer = true,
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => {
                if !app.is_empty() {
                    return Err(Stop::usage(usage::EXPOSE_ONE));
                }
                app = w;
            }
        }
    }
    if app.is_empty() || mode.is_empty() {
        return Err(Stop::usage(usage::EXPOSE));
    }
    if transfer && mode != "tunnel" {
        return Err(Stop::usage(usage::EXPOSE_TRANSFER));
    }
    let bit = |b: bool| if b { "1" } else { "0" };
    let body = Given::new()
        .put("app", app)
        .put("tunnel_b", bit(mode == "tunnel"))
        .put("transfer_b", bit(transfer))
        .body();
    Ok(Ask::Expose { body, json })
}

fn handover(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    let mut h = Handover {
        verb: verb.to_string(),
        ..Handover::default()
    };
    let mut rest = words;
    if verb == "handover" {
        if let Some(
            s @ ("accept" | "decline" | "cancel" | "status" | "brief" | "undo" | "finish" | "rekey"
            | "plan"),
        ) = rest.first().copied()
        {
            h.sub = s.to_string();
            rest = &rest[1..];
        }
    }
    let brief_file = match handover_words(&mut h, rest) {
        Ok(f) => f,
        Err(stop) => return Some(Err(stop)),
    };
    if !brief_file.is_empty() {
        let is_file = std::fs::metadata(brief_file).is_ok_and(|m| m.is_file());
        if !is_file {
            return Some(Err(Stop::usage(format!(
                "{{\"error\":\"bad_request\",\"hint\":\"no brief file {brief_file}\"}}"
            ))));
        }
        // `brief="$(cat "$brief_file")"`: the bytes as they are, each NUL
        // dropped (bash 4.4 and later also warn on stderr; this doesn't),
        // then the newlines at the end. Bytes that aren't UTF-8 go to
        // `_compute_json` as they are ([`Given::put_bytes`]).
        let mut bytes = std::fs::read(brief_file).ok()?;
        bytes.retain(|b| *b != 0);
        while bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        h.brief_file = true;
        h.brief = bytes;
    }
    Some(Ok(Ask::Handover(h)))
}

/// `cmd_app_handover`'s loop: the words into `h`, and `--brief`'s file.
fn handover_words<'a>(h: &mut Handover, words: &[&'a str]) -> Result<&'a str, Stop> {
    let mut brief_file = "";
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let take = |slot: &mut String| -> Result<usize, Stop> {
            let (v, n) = value(words, i)?;
            *slot = v.to_string();
            Ok(n)
        };
        i = match *w {
            "--to" => take(&mut h.to)?,
            "--db" => take(&mut h.db)?,
            "--collisions" => take(&mut h.coll)?,
            "--reason" => take(&mut h.reason)?,
            "--brief" => {
                let (v, n) = value(words, i)?;
                brief_file = v;
                n
            }
            "--detail" | "--reset-shared" | "--copy-source" | "--dry-run" | "--json" => {
                match *w {
                    "--detail" => h.detail = true,
                    "--reset-shared" => h.reset_shared = true,
                    "--copy-source" => h.copy_source = true,
                    "--dry-run" => h.dry = true,
                    _ => h.json = true,
                }
                i + 1
            }
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => {
                h.app = w.to_string();
                i + 1
            }
        };
    }
    Ok(brief_file)
}

fn park(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut reason, mut json) = ("", "", false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--to" => (_, i) = value(words, i)?,
            "--reason" => (reason, i) = value(words, i)?,
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => (app, i) = (w, i + 1),
        }
    }
    if app.is_empty() {
        return Err(Stop::usage(usage::PARK));
    }
    let body = Given::new().put("app", app).put("reason", reason).body();
    Ok(Ask::Park {
        app: app.to_string(),
        body,
        json,
    })
}

fn release(words: &[&str]) -> Result<Ask, Stop> {
    let (mut ws, mut apps, mut drop, mut json) = ("", "", false, false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--apps" => (apps, i) = value(words, i)?,
            "--drop-access" => (drop, i) = (true, i + 1),
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            _ => (ws, i) = (w, i + 1),
        }
    }
    if ws.is_empty() || apps.is_empty() {
        return Err(Stop::usage(usage::RELEASE));
    }
    if drop && apps != "uninstall" {
        return Err(Stop::usage(usage::RELEASE_DROP));
    }
    Ok(Ask::Release {
        ws: ws.to_string(),
        apps: apps.to_string(),
        drop,
        json,
    })
}

fn exception(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut mode, mut reason, mut json) = ("", "", "", false);
    let mut devices = Vec::new();
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--device" => {
                let (v, n) = value(words, i)?;
                devices.push(v.to_string());
                i = n;
            }
            "--reason" => (reason, i) = value(words, i)?,
            "--json" => (json, i) = (true, i + 1),
            _ if w.starts_with('-') => return Err(unknown(w)),
            "on" | "off" if !app.is_empty() => (mode, i) = (w, i + 1),
            _ => (app, i) = (w, i + 1),
        }
    }
    if app.is_empty() || mode.is_empty() {
        return Err(Stop::usage(usage::EXCEPTION));
    }
    Ok(Ask::Exception {
        app: app.to_string(),
        on: mode == "on",
        devices,
        reason: reason.to_string(),
        json,
    })
}

/// `_vault_confirm_token … || exit 3`: the token's first line (it rides
/// `_compute_json`'s stdin, one line), or the command's end.
fn confirm(ctx: &mut Ctx, action: &str, id: &str, label: &str) -> Result<String, Exit> {
    let tok = crate::confirm::confirm_token(&mut ctx.confirm(), action, id, label)
        .ok_or(Exit::Refused)?;
    Ok(tok.split('\n').next().unwrap_or("").to_string())
}

/// `_app_post_confirming <route> <label> <pairs>`: the POST, and when the
/// daemon asks for the owner in person, the confirm and the POST again.
fn post_confirming(
    ctx: &mut Ctx,
    route: &str,
    label: &str,
    g: &Given,
) -> Result<super::call::Split, Exit> {
    let s = ctx.send(true, route, &g.body(), false)?;
    if s.status != "403" {
        return Ok(s);
    }
    let Some((action, sid)) = confirm_asked(&s.body) else {
        return Ok(s);
    };
    let tok = confirm(ctx, &action, &sid, label)?;
    let g = g.clone().put("confirmToken", &tok);
    ctx.send(true, route, &g.body(), false)
}

/// What `_app_post_confirming`'s reader prints, as `sed -n 1p` and `2p`
/// read it: the action and the id, when the refusal names both.
fn confirm_asked(body: &[u8]) -> Option<(String, String)> {
    let v = crate::out::pylib::json::loads(std::str::from_utf8(body).ok()?).ok()?;
    let e = match &v {
        Value::Object(p) => py::or(
            crate::families::get(p, "error")
                .cloned()
                .unwrap_or(Value::Null),
            Value::Object(Vec::new()),
        ),
        _ => return None,
    };
    let Value::Object(p) = &e else {
        return None;
    };
    let field = |k: &str| crate::families::get(p, k).cloned().unwrap_or(Value::Null);
    if field("code") != py::text("vault_confirm_required")
        || !py::truthy(&field("action"))
        || !py::truthy(&field("secretId"))
    {
        return None;
    }
    let printed = format!("{}\n{}", py::s(&field("action")), py::s(&field("secretId")));
    let printed = printed.trim_end_matches('\n');
    if printed.is_empty() {
        return None;
    }
    let mut lines = printed.split('\n');
    let action = lines.next().unwrap_or("").to_string();
    let sid = lines.next().unwrap_or("").to_string();
    Some((action, sid))
}

/// The owner at a terminal outside a session, who may approve here.
fn approves_here(ctx: &Ctx) -> bool {
    use std::io::IsTerminal;
    !ctx.in_session()
        && std::io::stdin().is_terminal()
        && k2_client::env_names::first_set(k2_client::env_names::NO_APPROVE)
            .is_none_or(|(_, v)| v != "1")
}

/// What was printed so far, then `_app_approve <id>`: `return 0` after
/// it, unless it ended the command.
fn then_approve(ctx: &mut Ctx, shown: Shown, ids: &[String]) -> Answer {
    let printed = shown.print();
    if printed != Exit::Ok {
        return Err(printed);
    }
    for id in ids {
        let a = super::approve::approve(ctx, id)?;
        let e = a.print();
        if e != Exit::Ok {
            return Err(e);
        }
    }
    Ok(Shown::out(String::new()))
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    match ask {
        Ask::Uninstall {
            app,
            purge,
            drop,
            json,
        } => {
            let bit = |b: bool| if b { "1" } else { "0" };
            let mut tok = String::new();
            if *purge {
                out::stderr_line(&format!(
                    "This deletes App {app} and its data folder (~/.k2/apps/{app}). It can't be undone."
                ));
                tok = confirm(
                    ctx,
                    "app-purge",
                    app,
                    &format!("Delete App {app} and its data"),
                )?;
            }
            let g = Given::new()
                .put("app", app)
                .put("purge_b", bit(*purge))
                .put("dropAccess_b", bit(*drop))
                .put("confirmToken", &tok);
            let body = post_200(ctx, "/cli/app/uninstall", &g.body())?;
            if *json {
                return Ok(raw(&body));
            }
            Ok(Shown::out(if *purge {
                format!("App {app} uninstalled; its data folder is gone.\n")
            } else {
                format!("App {app} uninstalled. Its data folder stays (~/.k2/apps/{app}); its Seeds go with the nightly sweep.\n")
            }))
        }
        Ask::Expose { body, json } => {
            let s = ctx.send(true, "/cli/app/expose", body, false)?;
            let body = match super::grace::done(ctx, s, *json) {
                Ok(b) => b,
                Err(a) => return a,
            };
            Ok(super::access::answer(&body, *json, expose_lines))
        }
        Ask::HandoverUsage => Ok(Shown::err(
            crate::text::help(HANDOVER_PAGE).unwrap_or_default(),
            Exit::Usage,
        )),
        Ask::Handover(h) => handover_run(ctx, h),
        Ask::Park { app, body, json } => {
            let body = post_200(ctx, "/cli/app/park", body)?;
            if *json {
                return Ok(raw(&body));
            }
            Ok(Shown::out(format!(
                concat!(
                    "App {} is parked: stopped and held for the owner (",
                    cli_name!(),
                    " app unpark {} --to <workspace>).\n"
                ),
                app, app
            )))
        }
        Ask::Release {
            ws,
            apps,
            drop,
            json,
        } => {
            let bit = if *drop { "1" } else { "0" };
            let g = Given::new()
                .put("workspace", ws)
                .put("apps", apps)
                .put("dropAccess_b", bit);
            let to = apps.strip_prefix("handover:").unwrap_or(apps);
            let label = format!("Give {ws}'s Apps to {to}");
            let s = post_confirming(ctx, "/cli/app/release", &label, &g)?;
            if !matches!(s.status.as_str(), "200" | "202") {
                return Err(ctx.fail(&s));
            }
            let held = s.status == "202";
            if *json {
                let mut shown = raw(&s.body);
                if held {
                    shown.exit = Exit::Usage;
                }
                return Ok(shown);
            }
            if !held {
                let mut o = Out::default();
                let r = release_lines(&s.body, &mut o);
                return Ok(o.done(r));
            }
            let cards = match release_cards(&s.body) {
                Ok(c) => c,
                Err(e) => return Ok(Out::default().done(Err(e))),
            };
            let mut o = Out::default();
            let r = (|| {
                let v = py::load(&s.body, 1)?;
                o.line(&py::s(&py::dget_or(&v, "hint", py::text(""), 1)?));
                Ok(Exit::Usage)
            })();
            let shown = o.done(r);
            if shown.exit != Exit::Usage || !approves_here(ctx) {
                return Ok(shown);
            }
            // `for c in $cards`: split on blanks.
            let ids: Vec<String> = cards.split_whitespace().map(str::to_string).collect();
            let mut shown = shown;
            shown.exit = Exit::Ok;
            then_approve(ctx, shown, &ids)
        }
        Ask::Exception {
            app,
            on,
            devices,
            reason,
            json,
        } => {
            let mut tok = String::new();
            if *on {
                let with = match devices.first() {
                    Some(d) if !d.is_empty() => format!(" with {}", devices.join(" ")),
                    _ => String::new(),
                };
                out::stderr_line(&format!(
                    "This moves App {app} out of its guarded container into the process exception (a sandboxed system process{with})."
                ));
                out::stderr_line(
                    "Runs as a plain process, with less isolation from your other process Apps than a containerised App.",
                );
                tok = confirm(
                    ctx,
                    "app-process-exception",
                    app,
                    &format!("process exception for App {app}"),
                )?;
            }
            let list = Value::Array(devices.iter().map(|d| py::text(d)).collect());
            let devices_json = crate::confirm::dumps(&list);
            let g = Given::new()
                .put("app", app)
                .put("on_b", if *on { "1" } else { "0" })
                .put("devices_j", &devices_json)
                .put("reason", reason)
                .put("confirmToken", &tok);
            let body = post_200(ctx, "/cli/app/exception", &g.body())?;
            Ok(super::access::answer(&body, *json, exception_lines))
        }
    }
}

fn handover_run(ctx: &mut Ctx, h: &Handover) -> Answer {
    let usage = || {
        Ok(Shown::err(
            crate::text::help(HANDOVER_PAGE).unwrap_or_default(),
            Exit::Usage,
        ))
    };
    let app_q = format!("app={}", quote(&h.app));
    match h.sub.as_str() {
        "status" => {
            let q = if h.app.is_empty() {
                String::new()
            } else {
                app_q
            };
            let body = ctx.call(false, "/cli/app/handover/status", &q)?;
            if h.json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = status_lines(&body, &mut o, h.detail);
            return Ok(o.done(r));
        }
        "brief" => {
            if h.app.is_empty() {
                return usage();
            }
            let body = ctx.call(false, "/cli/app/handover/brief", &app_q)?;
            if h.json {
                return Ok(raw(&body));
            }
            let mut o = Out::default();
            let r = (|| {
                let v = py::load(&body, 1)?;
                let b = py::or(py::dget(&v, "brief", 1)?, py::text("(no brief)"));
                o.line(&py::s(&b));
                Ok(Exit::Ok)
            })();
            return Ok(o.done(r));
        }
        sub @ ("accept" | "decline" | "cancel" | "undo" | "finish" | "rekey") => {
            if h.app.is_empty() {
                return usage();
            }
            let b = Given::new()
                .put("app", &h.app)
                .put("reason", &h.reason)
                .body();
            let s = ctx.send(true, &format!("/cli/app/handover/{sub}"), &b, false)?;
            if !matches!(s.status.as_str(), "200" | "202") {
                return Err(ctx.fail(&s));
            }
            let held = s.status == "202";
            if h.json {
                let mut shown = raw(&s.body);
                if held {
                    shown.exit = Exit::Usage;
                }
                return Ok(shown);
            }
            let mut o = Out::default();
            let r = handover_lines(&s.body, &mut o);
            let mut shown = o.done(r);
            if shown.exit != Exit::Ok || !held {
                return Ok(shown);
            }
            if sub == "undo" && approves_here(ctx) {
                let (id, err) = match request_id(&s.body) {
                    Ok(id) => (id, String::new()),
                    Err(e) => (String::new(), e.text()),
                };
                shown.stderr.push_str(&err);
                return then_approve(ctx, shown, &[id]);
            }
            shown.exit = Exit::Usage;
            return Ok(shown);
        }
        _ => {}
    }
    if h.app.is_empty() || h.to.is_empty() {
        return usage();
    }
    if h.dry || h.sub == "plan" {
        let mut q = format!("app={}&to={}", quote(&h.app), quote(&h.to));
        if !h.db.is_empty() {
            q.push_str(&format!("&db={}", quote(&h.db)));
        }
        if !h.coll.is_empty() {
            q.push_str(&format!("&collisions={}", quote(&h.coll)));
        }
        if h.reset_shared {
            q.push_str("&resetShared=1");
        }
        if h.copy_source {
            q.push_str("&copySource=1");
        }
        let body = ctx.call(false, "/cli/app/handover/plan", &q)?;
        if h.json {
            return Ok(raw(&body));
        }
        let mut o = Out::default();
        let r = handover_lines(&body, &mut o).and_then(|_| {
            let v = py::load(&body, 1)?;
            o.line(if py::truthy(&py::dget(&v, "ready", 1)?) {
                "ready"
            } else {
                "not ready: see blocked lines"
            });
            Ok(Exit::Ok)
        });
        return Ok(o.done(r));
    }
    let one = |b: bool| if b { "1" } else { "" };
    let g = Given::new()
        .put("app", &h.app)
        .put("to", &h.to)
        .put_bytes("brief", &h.brief)
        .put("db", &h.db)
        .put("collisions", &h.coll)
        .put("resetShared_b", one(h.reset_shared))
        .put("copySource_b", one(h.copy_source))
        .put("reason", &h.reason);
    let label = format!("Give App {} to {}", h.app, h.to);
    let s = post_confirming(ctx, &format!("/cli/app/{}", h.verb), &label, &g)?;
    if !matches!(s.status.as_str(), "200" | "202") {
        return Err(ctx.fail(&s));
    }
    let held = s.status == "202";
    if h.json {
        let mut shown = raw(&s.body);
        if held {
            shown.exit = Exit::Usage;
        }
        return Ok(shown);
    }
    let mut o = Out::default();
    if !held {
        let r = (|| {
            let v = py::load(&s.body, 1)?;
            o.line(&format!(
                "App {} now belongs to {}.",
                py::s(&py::dget(&v, "app", 1)?),
                py::s(&py::dget(&v, "workspace", 1)?)
            ));
            Ok(Exit::Ok)
        })();
        return Ok(o.done(r));
    }
    if let Err(e) = handover_lines(&s.body, &mut o) {
        return Ok(o.done(Err(e)));
    }
    if !h.brief_file && ctx.in_session() {
        o.err(&format!(
            concat!(
                "tip: give the next agent a brief: ",
                cli_name!(),
                " app handover {} --to {} --brief NOTES.md (once this offer is cancelled)"
            ),
            h.app, h.to
        ));
    }
    let card = match card_id(&s.body) {
        Ok(c) => c,
        Err(e) => return Ok(o.done(Err(e))),
    };
    if !card.is_empty() && approves_here(ctx) {
        let shown = o.done(Ok(Exit::Ok));
        return then_approve(ctx, shown, &[card]);
    }
    Ok(o.done(Ok(Exit::Usage)))
}

/// `print(json.load(sys.stdin)["request"]["id"])` held in `$(…)`.
fn request_id(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 1)?;
    let id = py::s(&py::idx(&py::idx(&v, "request", 1)?, "id", 1)?);
    Ok(id.trim_end_matches('\n').to_string())
}

/// `r=…get("request"); print(r["id"] if isinstance(r, dict) else "")`.
fn card_id(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 1)?;
    let r = py::dget(&v, "request", 1)?;
    let id = if matches!(r, Value::Object(_)) {
        py::s(&py::idx(&r, "id", 1)?)
    } else {
        String::new()
    };
    Ok(id.trim_end_matches('\n').to_string())
}

/// `_app_handover_print`.
fn handover_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let p = py::dget(&v, "plan", 4)?;
    let plan = if matches!(p, Value::Object(_)) {
        p
    } else if py::contains(&v, "lines", 4)? {
        v.clone()
    } else {
        Value::Null
    };
    let h = py::or(py::dget(&v, "handover", 5)?, Value::Object(Vec::new()));
    if py::truthy(&py::dget(&v, "already", 6)?) {
        let mut w = py::dget(&v, "workspace", 7)?;
        if !py::truthy(&w) {
            w = py::or(py::dget(&h, "toHandle", 7)?, py::text("this workspace"));
        }
        o.line(&format!("Already: {}", py::s(&w)));
    }
    if py::truthy(&plan) && py::dget(&v, "status", 8)? != py::text("held") {
        for b in py_iter(
            &py::or(py::dget(&plan, "blockers", 9)?, py::empty_list()),
            9,
        )? {
            o.line(&format!(
                "blocked ({}): {}",
                py::s(&py::dget(&b, "code", 10)?),
                py::s(&py::dget(&b, "hint", 10)?)
            ));
        }
    }
    if py::truthy(&plan) {
        for l in py_iter(&py::or(py::dget(&plan, "lines", 12)?, py::empty_list()), 12)? {
            o.line(&py::concat("  - ", &l, 13)?);
        }
    }
    if py::truthy(&h) {
        o.line(&format!(
            "handover {}: {} ({} -> {})",
            py::s(&py::dget(&h, "id", 15)?),
            py::s(&py::dget(&h, "state", 15)?),
            py::s(&py::or(py::dget(&h, "fromHandle", 15)?, py::text("parked"))),
            py::s(&py::dget(&h, "toHandle", 15)?)
        ));
    }
    let r = py::dget(&v, "request", 16)?;
    if matches!(r, Value::Object(_)) && py::truthy(&py::dget(&r, "id", 17)?) {
        o.line(&format!("card: {}", py::s(&py::idx(&r, "id", 18)?)));
    }
    if py::truthy(&py::dget(&v, "hint", 19)?) {
        o.line(&py::s(&py::idx(&v, "hint", 20)?));
    }
    Ok(Exit::Ok)
}

/// `handover status`'s Python.
fn status_lines(body: &[u8], o: &mut Out, detail: bool) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let rows = py::or(py::dget(&v, "handovers", 3)?, py::empty_list());
    if !py::truthy(&rows) {
        o.line("No handovers.");
        return Ok(Exit::Ok);
    }
    for h in py_iter(&rows, 7)? {
        let mut extra = String::new();
        if py::truthy(&py::dget(&h, "inWindow", 9)?) {
            extra = format!(" (undo until {})", py::s(&py::dget(&h, "windowUntil", 10)?));
        }
        if py::truthy(&py::dget(&h, "rekeyState", 11)?) {
            extra.push_str(&format!(
                " re-key: {}",
                py::s(&py::idx(&h, "rekeyState", 12)?)
            ));
        }
        let id = py::idx(&h, "id", 13)?;
        let state = py::idx(&h, "state", 13)?;
        let app = py::idx(&h, "appId", 13)?;
        o.line(&format!(
            "{}  {} {}  {} -> {}{extra}",
            py::s(&id),
            py::left(&state, 9),
            py::s(&app),
            py::s(&py::or(py::dget(&h, "fromHandle", 13)?, py::text("parked"))),
            py::s(&py::dget(&h, "toHandle", 13)?)
        ));
        if detail {
            o.line(&format!(
                "  card: {}",
                py::s(&py::or(
                    py::dget(&h, "requestId", 15)?,
                    py::text("none yet (the card is filed once the new agent accepts)")
                ))
            ));
            let p = py::dget(&h, "plan", 16)?;
            let plan = if matches!(p, Value::Object(_)) {
                p
            } else {
                Value::Object(Vec::new())
            };
            let lines = py::or(py::dget(&plan, "lines", 17)?, py::empty_list());
            if py::truthy(&lines) {
                o.line("  what moves (the card's own words, to the owner):");
                for l in py_iter(&lines, 20)? {
                    o.line(&py::concat("    - ", &l, 21)?);
                }
            } else {
                o.line(&format!(
                    concat!(
                        "  what moves: not worked out yet (",
                        cli_name!(),
                        " app handover {} --to {} --dry-run)"
                    ),
                    py::s(&py::idx(&h, "appId", 23)?),
                    py::s(&py::dget(&h, "toHandle", 23)?)
                ));
            }
        }
    }
    Ok(Exit::Ok)
}

fn expose_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    if py::truthy(&py::dget(&v, "changed", 4)?) {
        if py::dget(&v, "expose", 5)? == py::text("tunnel") {
            let url = if py::truthy(&py::dget(&v, "url", 6)?) {
                py::concat(": ", &py::idx(&v, "url", 6)?, 6)?
            } else {
                String::new()
            };
            o.line(&format!(
                "App {} now has its public URL{url}",
                py::s(&py::dget(&v, "app", 6)?)
            ));
        } else {
            o.line(&format!(
                "App {} is local only now (no public URL)",
                py::s(&py::dget(&v, "app", 8)?)
            ));
        }
    }
    if py::truthy(&py::dget(&v, "hostnameLeft", 9)?) {
        o.line(&py::s(&py::idx(&v, "hostnameLeft", 10)?));
    }
    o.line(&py::s(&py::dget_or(&v, "next", py::text(""), 11)?));
    Ok(Exit::Ok)
}

/// The cards of a held `release`, one a line, as `$(…)` holds them.
fn release_cards(body: &[u8]) -> Result<String, Raise> {
    let v = py::load(body, 3)?;
    let mut out = String::new();
    for r in py_iter(&py::dget_or(&v, "results", py::empty_list(), 4)?, 4)? {
        if py::truthy(&py::dget(&r, "request", 5)?) {
            out.push_str(&py::s(&py::idx(&r, "request", 6)?));
            out.push('\n');
        }
    }
    Ok(out)
}

fn release_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::dget_or(&v, "results", py::empty_list(), 4)?;
    let ws = py::dget(&v, "workspace", 5)?;
    let n = py_len(&r, 5)?;
    o.line(&format!(
        "{}: {n} App{} released ({}).",
        py::s(&ws),
        if n == 1 { "" } else { "s" },
        py::s(&py::dget(&v, "way", 5)?)
    ));
    for x in py_iter(&r, 6)? {
        o.line(&py::concat("  ", &py::idx(&x, "app", 7)?, 7)?);
    }
    Ok(Exit::Ok)
}

fn exception_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let app = py::idx(&v, "app", 4)?;
    let label = py::dget_or(&v, "label", py::text(""), 4)?;
    let change = if py::truthy(&py::dget(&v, "changed", 4)?) {
        ""
    } else {
        " (no change)"
    };
    o.line(&format!("{}: {}{change}", py::s(&app), py::s(&label)));
    for (k, line) in [("warning", 5), ("next", 7)] {
        if py::truthy(&py::dget(&v, k, line)?) {
            o.line(&py::s(&py::idx(&v, k, line + 1)?));
        }
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(verb: &str, w: &[&str]) -> Stop {
        match parse(verb, w) {
            Some(Err(s)) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_words_as_the_shell_reads_them() {
        assert_eq!(parse("handover", &[]), Some(Ok(Ask::HandoverUsage)));
        assert_eq!(parse("handover", &["a", "-h"]), None);
        assert_eq!(stop("uninstall", &[]).line, usage::UNINSTALL);
        assert_eq!(
            stop("expose", &["a", "--tunnel", "--local"]).line,
            usage::EXPOSE_BOTH
        );
        assert_eq!(stop("expose", &["a", "b"]).line, usage::EXPOSE_ONE);
        assert_eq!(
            stop("expose", &["a", "--local", "--transfer"]).line,
            usage::EXPOSE_TRANSFER
        );
        assert_eq!(stop("park", &["--reason"]), Stop::need_value("--reason"));
        assert_eq!(
            stop("release", &["ws", "--apps", "park", "--drop-access"]).line,
            usage::RELEASE_DROP
        );
        assert_eq!(stop("exception", &["on"]).line, usage::EXCEPTION);
        match parse("exception", &["on", "on"]) {
            Some(Ok(Ask::Exception { app, on, .. })) => {
                assert_eq!((app.as_str(), on), ("on", true))
            }
            other => panic!("{other:?}"),
        }
        match parse("handover", &["status", "acme", "--detail"]) {
            Some(Ok(Ask::Handover(h))) => {
                assert_eq!(
                    (h.sub.as_str(), h.app.as_str(), h.detail),
                    ("status", "acme", true)
                )
            }
            other => panic!("{other:?}"),
        }
        assert!(stop("handover", &["a", "--brief", "/no/such/file"])
            .line
            .contains("no brief file"));
    }

    #[test]
    fn the_confirm_the_daemon_asks_for() {
        let asked = |b: &str| confirm_asked(b.as_bytes());
        assert_eq!(
            asked(
                r#"{"error":{"code":"vault_confirm_required","action":"app-handover","secretId":"acme"}}"#
            ),
            Some(("app-handover".into(), "acme".into()))
        );
        assert_eq!(
            asked(r#"{"error":{"code":"vault_confirm_required","action":"a"}}"#),
            None
        );
        assert_eq!(asked(r#"{"error":"owner_only"}"#), None);
        assert_eq!(asked("[1]"), None);
    }
}
