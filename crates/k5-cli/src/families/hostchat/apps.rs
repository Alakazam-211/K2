//! Chat for Apps (`cli/src/hostchat-voice.sh`): `app-space` (list, bind,
//! unbind, governance, a Space's recording rule and its lock), `home`,
//! `pass-keys` and `meet`.

use super::main_verbs::{show, token};
use super::print::{self, g, gd, ix, load, or, st, sub, sv, t, Py, R};
use super::{cj, usage, Ctx, Plan};
use crate::confirm;
use crate::exit::Exit;
use crate::families::app::py::local_minutes;
use crate::families::voice::sew::{self, Field};
use crate::families::{ljust, py_iter, raised, Shown};
use crate::out;
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// One verb of these, ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Apps {
    SpaceList { json: bool },
    SpacePost { route: String, payload: String, verb: String, json: bool },
    Governance { app: String, space: String, gov: String, json: bool },
    RuleRead { app: String, space: String, json: bool },
    Rule { app: String, space: String, body: String, json: bool },
    Lock { space: String, word: String, body: String, json: bool },
    Home { post: bool, route: &'static str, payload: String, json: bool },
    PassKeys { sub: &'static str, json: bool },
    Meet { route: &'static str, payload: String, verb: &'static str, json: bool },
}

fn s(t: &str) -> Field {
    Field::S(t.to_string())
}

fn bad(hint: impl AsRef<str>) -> Plan {
    Plan::Bad(usage(hint.as_ref()))
}

/// `app-space …` and `app-spaces …`: `noun` is the word typed.
pub fn app_space(noun: &str, words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&"recording", r)) => return rule(r),
        Some((&"recording-lock", r)) => return lock(r),
        Some((&s @ ("bind" | "unbind" | "list" | "governance"), r)) => (s, r),
        _ => ("list", words),
    };
    let (mut json, mut remove) = (false, false);
    let mut pos: Vec<&str> = Vec::new();
    for &w in rest {
        match w {
            "--json" => json = true,
            "--remove-app-members" => remove = true,
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => {
                return bad(format!("unknown flag {w} ({c} hostchat app-space bind|unbind <app-id> <space-id> [--remove-app-members])"))
            }
            w => pos.push(w),
        }
    }
    let p = |n: usize| pos.get(n).copied().unwrap_or("");
    match sub {
        "list" => Plan::Apps(Apps::SpaceList { json }),
        "bind" | "unbind" => {
            if p(0).is_empty() || p(1).is_empty() {
                return bad(format!("{c} hostchat {noun} {sub} <app-id> <space-id>"));
            }
            if remove && sub != "unbind" {
                return bad("--remove-app-members is for unbind");
            }
            let rm = if sub == "unbind" && remove { "1" } else { "" };
            Plan::Apps(Apps::SpacePost {
                route: format!("/cli/sew/app-spaces/{sub}"),
                payload: cj(&[
                    ("app", s(p(0))),
                    ("spaceId", s(p(1))),
                    ("removeAppMembers", Field::B(rm.into())),
                ]),
                verb: format!("app-space-{sub}"),
                json,
            })
        }
        _ => {
            if p(0).is_empty() || p(1).is_empty() || p(2).is_empty() {
                return bad(format!(
                    "{c} hostchat app-space governance <app> <space-id> app|sew"
                ));
            }
            if !matches!(p(2), "app" | "sew") {
                return bad(format!("governance is app or sew ({c} hostchat app-space governance <app> <space-id> app|sew)"));
            }
            if remove {
                return bad("--remove-app-members is for unbind");
            }
            Plan::Apps(Apps::Governance {
                app: p(0).into(),
                space: p(1).into(),
                gov: p(2).into(),
                json,
            })
        }
    }
}

/// `app-space recording <app> <space-id> [--start-level …] [--required …]`.
fn rule(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (mut json, mut level, mut required): (bool, Option<&str>, Option<&str>) =
        (false, None, None);
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => json = true,
            "--start-level" | "--required" => {
                // `x="${2:-}"; shift 2 || {usage}`.
                let Some(v) = words.get(i + 1).copied() else {
                    return bad(if w == "--required" {
                        "--required on|off|inherit"
                    } else {
                        "--start-level none|admin|moderator|member|inherit"
                    });
                };
                if w == "--required" {
                    required = Some(v);
                } else {
                    level = Some(v);
                }
                i += 1;
            }
            "-h" | "--help" => return Plan::Text(super::recording::HELP),
            w if w.starts_with("--start-level=") => level = Some(&w["--start-level=".len()..]),
            w if w.starts_with("--required=") => required = Some(&w["--required=".len()..]),
            w if w.starts_with('-') => {
                return bad(format!(
                    "unknown flag {w} ({c} hostchat app-space recording --help)"
                ))
            }
            w => pos.push(w),
        }
        i += 1;
    }
    let (level, required) = (level.unwrap_or(""), required.unwrap_or(""));
    let app = pos.first().copied().unwrap_or("");
    let space = pos.get(1).copied().unwrap_or("");
    if app.is_empty() || space.is_empty() || !pos.get(2).copied().unwrap_or("").is_empty() {
        return bad(format!("{c} hostchat app-space recording <app> <space-id> [--start-level none|admin|moderator|member|inherit] [--required on|off|inherit]"));
    }
    let level_j = match level {
        "" => String::new(),
        "none" | "admin" | "moderator" | "member" => format!("\"{level}\""),
        "inherit" => "null".into(),
        _ => return bad("--start-level is none, admin, moderator, member or inherit"),
    };
    let req_j = match required {
        "" => "",
        "on" | "true" | "yes" => "true",
        "off" | "false" | "no" => "false",
        "inherit" => "null",
        _ => return bad("--required is on, off or inherit"),
    };
    if level.is_empty() && required.is_empty() {
        return Plan::Apps(Apps::RuleRead {
            app: app.into(),
            space: space.into(),
            json,
        });
    }
    Plan::Apps(Apps::Rule {
        app: app.into(),
        space: space.into(),
        body: cj(&[
            ("app", s(app)),
            ("spaceId", s(space)),
            ("startLevel", Field::J(level_j)),
            ("required", Field::J(req_j.into())),
        ]),
        json,
    })
}

/// `app-space recording-lock <space-id> on|off`.
fn lock(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let mut json = false;
    let mut pos: Vec<&str> = Vec::new();
    for &w in words {
        match w {
            "--json" => json = true,
            "-h" | "--help" => return Plan::Text(super::recording::HELP),
            w if w.starts_with('-') => {
                return bad(format!("unknown flag {w} ({c} hostchat app-space recording-lock <space-id> on|off)"))
            }
            w => pos.push(w),
        }
    }
    let space = pos.first().copied().unwrap_or("");
    let word = pos.get(1).copied().unwrap_or("");
    let lock = match word {
        "on" | "true" | "yes" => "1",
        "off" | "false" | "no" => "0",
        _ => "",
    };
    if space.is_empty() || lock.is_empty() || !pos.get(2).copied().unwrap_or("").is_empty() {
        return bad(format!(
            "{c} hostchat app-space recording-lock <space-id> on|off"
        ));
    }
    Plan::Apps(Apps::Lock {
        space: space.into(),
        word: word.into(),
        body: cj(&[("spaceId", s(space)), ("lock", Field::B(lock.into()))]),
        json,
    })
}

/// `home …`.
pub fn home(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&s @ ("client" | "app"), r)) => (s, r),
        None => ("show", &[]),
        Some((w, _)) if w.is_empty() || w.starts_with('-') => ("show", words),
        Some(_) => {
            return bad(format!("{c} hostchat home [client <app-id>|none] | home app <app-id> --hidden on|off"))
        }
    };
    let mut json = false;
    let mut hidden = "";
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--json" => json = true,
            "--hidden" => {
                // `shift 2` with one word left fails: `set -e` ends it, 1.
                let Some(v) = rest.get(i + 1).copied() else {
                    return Plan::Silent(Exit::Error);
                };
                hidden = v;
                i += 1;
            }
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => {
                return bad(format!("unknown flag {w} ({c} hostchat home --help)"))
            }
            w => pos.push(w),
        }
        i += 1;
    }
    let app = pos.first().copied().unwrap_or("");
    let (post, route, payload) = match sub {
        "show" => (false, "/cli/sew/home", String::new()),
        "client" => {
            if app.is_empty() {
                return bad(format!("{c} hostchat home client <app-id>|none"));
            }
            let payload = if app == "none" {
                r#"{"app":null}"#.to_string()
            } else {
                cj(&[("app", s(app))])
            };
            (true, "/cli/sew/home/client", payload)
        }
        _ => {
            let h = match hidden {
                "on" | "true" | "yes" => "1",
                "off" | "false" | "no" => "0",
                _ => return bad("--hidden on|off"),
            };
            if app.is_empty() {
                return bad(format!("{c} hostchat home app <app-id> --hidden on|off"));
            }
            (
                true,
                "/cli/sew/home/app",
                cj(&[("app", s(app)), ("hidden", Field::B(h.into()))]),
            )
        }
    };
    Plan::Apps(Apps::Home {
        post,
        route,
        payload,
        json,
    })
}

/// `pass-keys [rotate|drop-previous] [--json]`.
pub fn pass_keys(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (mut sub, mut json) = ("show", false);
    for &w in words {
        match w {
            "--json" => json = true,
            "-h" | "--help" => return Plan::Help,
            "rotate" => sub = "rotate",
            "drop-previous" => sub = "drop-previous",
            w if w.starts_with('-') => {
                return bad(format!(
                    "unknown flag {w} ({c} hostchat pass-keys [rotate|drop-previous] [--json])"
                ))
            }
            _ => {
                return bad(format!(
                    "{c} hostchat pass-keys [rotate|drop-previous] [--json]"
                ))
            }
        }
    }
    Plan::Apps(Apps::PassKeys { sub, json })
}

/// `meet bind <app-id> | unbind`.
pub fn meet(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&s, r)) => (s, r),
        None => ("", &[]),
    };
    let mut json = false;
    let mut pos: Vec<&str> = Vec::new();
    for &w in rest {
        match w {
            "--json" => json = true,
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => {
                return bad(format!(
                    "unknown flag {w} ({c} hostchat meet bind <app-id> | unbind)"
                ))
            }
            w => pos.push(w),
        }
    }
    match sub {
        "bind" => {
            let app = pos.first().copied().unwrap_or("");
            if app.is_empty() {
                return bad(format!("{c} hostchat meet bind <app-id>"));
            }
            Plan::Apps(Apps::Meet {
                route: "/cli/sew/meet/bind",
                payload: cj(&[("app", s(app))]),
                verb: "meet-bind",
                json,
            })
        }
        "unbind" => Plan::Apps(Apps::Meet {
            route: "/cli/sew/meet/unbind",
            payload: "{}".into(),
            verb: "meet-unbind",
            json,
        }),
        "" | "help" => Plan::Help,
        _ => Plan::Bad(format!(
            "{{\"error\":\"usage\",\"hint\":\"unknown meet command '{sub}' ({c} hostchat meet bind <app-id> | unbind)\"}}"
        )),
    }
}

/// A reader's lines and its exit, or where it raised.
fn lines(r: R<Exit>, p: Py) -> Exit {
    match r {
        Ok(exit) => Shown {
            stdout: p.out,
            stderr: p.err,
            exit,
        }
        .print(),
        Err(e) => raised(p.out, p.err, &e).print(),
    }
}

fn raw(body: &str) -> Exit {
    show(body, "", true)
}

pub fn run(ctx: &mut Ctx, a: &Apps) -> Exit {
    match step(ctx, a) {
        Ok(e) | Err(e) => e,
    }
}

/// `_app_rt_post`, `_rec_needs_confirm`, `_rec_send_confirmed`, then
/// `[ "$_CP_STATUS" = "200" ] || _compute_fail`: the body of a 200.
fn held_post(
    ctx: &mut Ctx,
    route: &str,
    body: &str,
    confirm_for: impl FnOnce(&mut Ctx) -> Result<(String, String, String), Exit>,
) -> Result<String, Exit> {
    let mut sent = sew::send(&mut ctx.client, true, route, body, false)?;
    sew::note(&mut ctx.closing, &sent.body, ctx.client.passport().is_some());
    let held = sent.status == "403"
        && [&b"\"vault_confirm_required\""[..], b"\"owner_confirm_required\""]
            .iter()
            .any(|w| sent.body.windows(w.len()).any(|x| x == *w))
        && !ctx.in_session;
    if held {
        let (action, id, label) = confirm_for(ctx)?;
        let tok = token(ctx, &action, &id, &label)?;
        let b = confirm::with_confirm_token(body, &tok, None).unwrap_or_default();
        drop(tok);
        sent = sew::send(&mut ctx.client, true, route, &b, true)?;
        sew::note(&mut ctx.closing, &sent.body, ctx.client.passport().is_some());
    }
    if sent.status != "200" {
        let (line, exit) = sew::fail_line(&sent.status, &sent.body);
        out::stderr_line(&line);
        return Err(exit);
    }
    Ok(String::from_utf8_lossy(&sent.body).into_owned())
}

fn step(ctx: &mut Ctx, a: &Apps) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    match a {
        Apps::SpaceList { json } => {
            let body = ctx.call(false, "/cli/sew/app-spaces", "", false)?;
            Ok(show(&body, "app-spaces", *json))
        }
        Apps::SpacePost {
            route,
            payload,
            verb,
            json,
        } => {
            let body = ctx.call(true, route, payload, false)?;
            Ok(show(&body, verb, *json))
        }
        Apps::Governance {
            app,
            space,
            gov,
            json,
        } => {
            let (tok, tcp) = if ctx.in_session {
                (String::new(), false)
            } else {
                let tok = token(
                    ctx,
                    "chat-governance",
                    &format!("{app}:{space}:{gov}"),
                    &format!("Space {space} governed by: {gov}"),
                )?;
                (tok, true)
            };
            let payload = cj(&[
                ("app", s(app)),
                ("spaceId", s(space)),
                ("governance", s(gov)),
                ("confirmToken", Field::S(tok)),
            ]);
            let body = ctx.call(true, "/cli/sew/app-spaces/governance", &payload, tcp)?;
            Ok(show(&body, "app-space-governance", *json))
        }
        Apps::RuleRead { app, space, json } => {
            let q = format!("app={}", quote(app));
            let body = ctx.call(false, "/cli/app/recording", &q, false)?;
            if *json {
                return Ok(raw(&body));
            }
            let mut p = Py::default();
            let r = (|| -> R<Exit> {
                let v = load(&body, 3)?;
                let a = sub(&v, "app", 5)?;
                let mut found = None;
                for x in py_iter(&or(g(&v, "spaces", 6)?, Value::Array(Vec::new())), 6)? {
                    if sv(&g(&x, "spaceId", 6)?) == *space {
                        found = Some(x);
                        break;
                    }
                }
                let Some(sp) = found else {
                    let o = confirm::object(vec![
                        ("error", st("space_unbound")),
                        (
                            "hint",
                            st(&format!(
                                "Space {space} is not bound to {} ({c} hostchat app-spaces)",
                                sv(&g(&a, "name", 8)?)
                            )),
                        ),
                    ]);
                    p.eprint(&sew::dumps_default(&o));
                    return Ok(Exit::Refused);
                };
                let req = g(&sp, "required", 10)?;
                p.print(&format!(
                    "Space {space} (App {}, governed by {})",
                    sv(&g(&a, "name", 11)?),
                    sv(&or(g(&sp, "governance", 11)?, st("sew")))
                ));
                if t(&g(&sp, "locked", 12)?) {
                    p.print("  LOCKED: no recording here (CUI/FCI). Only the server's owner lifts it, in person.");
                }
                p.print(&format!(
                    "  start level  {}",
                    sv(&or(g(&sp, "startLevel", 14)?, st("inherit (the App's rule)")))
                ));
                let r = match req {
                    Value::Null => "inherit (the App's rule)",
                    r if t(&r) => "on",
                    _ => "off",
                };
                p.print(&format!("  required     {r}"));
                Ok(Exit::Ok)
            })();
            Ok(lines(r, p))
        }
        Apps::Rule {
            app,
            space,
            body,
            json,
        } => {
            let answer = held_post(ctx, "/cli/sew/app-spaces/recording", body, |ctx| {
                let q = format!("app={}", quote(app));
                let b = ctx.call(false, "/cli/app/recording", &q, false)?;
                let svc = load(&b, 1)
                    .and_then(|v| sub(&v, "app", 1))
                    .and_then(|a| g(&a, "serviceId", 1));
                let svc = match svc {
                    Ok(x) => {
                        let mut x = sv(&or(x, st("")));
                        while x.ends_with('\n') {
                            x.pop();
                        }
                        x
                    }
                    Err(e) => {
                        let _ =
                            out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
                        return Err(Exit::Error);
                    }
                };
                if svc.is_empty() {
                    out::stderr_line(&format!("{{\"error\":\"not_found\",\"hint\":\"{PRODUCT_NAME} found no App by that name ({c} app list)\"}}"));
                    return Err(Exit::Refused);
                }
                Ok((
                    "recording-space".into(),
                    format!("{svc}:{space}"),
                    format!("Recording in Space {space}"),
                ))
            })?;
            if *json {
                return Ok(raw(&answer));
            }
            let mut p = Py::default();
            let r = (|| -> R<Exit> {
                let v = load(&answer, 3)?;
                let req = g(&v, "required", 4)?;
                p.print(&format!(
                    "Space {} (App {}): {}",
                    sv(&g(&v, "spaceId", 5)?),
                    sv(&g(&v, "app", 5)?),
                    if t(&g(&v, "changed", 5)?) {
                        "saved"
                    } else {
                        "nothing changed (already so)"
                    }
                ));
                p.print(&format!(
                    "  start level  {}",
                    sv(&or(g(&v, "startLevel", 6)?, st("inherit")))
                ));
                let r = match req {
                    Value::Null => "inherit",
                    r if t(&r) => "on",
                    _ => "off",
                };
                p.print(&format!("  required     {r}"));
                if t(&g(&v, "locked", 8)?) {
                    p.print("  LOCKED: no recording here (CUI/FCI); the rule above waits until the owner lifts the lock");
                }
                Ok(Exit::Ok)
            })();
            Ok(lines(r, p))
        }
        Apps::Lock {
            space,
            word,
            body,
            json,
        } => {
            let answer = held_post(ctx, "/cli/sew/app-spaces/recording-lock", body, |_| {
                Ok((
                    "recording-lock".into(),
                    space.clone(),
                    format!("Recording lock {word} for Space {space}"),
                ))
            })?;
            if *json {
                return Ok(raw(&answer));
            }
            let mut p = Py::default();
            let r = (|| -> R<Exit> {
                let v = load(&answer, 3)?;
                p.print(&format!(
                    "Space {}: recording lock {}{}",
                    sv(&g(&v, "spaceId", 4)?),
                    if t(&g(&v, "locked", 4)?) { "on" } else { "off" },
                    if t(&g(&v, "changed", 4)?) {
                        ""
                    } else {
                        " (already so)"
                    }
                ));
                if t(&g(&v, "hint", 5)?) {
                    p.print(&print::cat("  ", &ix(&v, "hint", 6)?, 6)?);
                }
                Ok(Exit::Ok)
            })();
            Ok(lines(r, p))
        }
        Apps::Home {
            post,
            route,
            payload,
            json,
        } => {
            let body = ctx.call(*post, route, payload, false)?;
            if *json {
                return Ok(raw(&body));
            }
            let mut p = Py::default();
            let r = (|| -> R<Exit> {
                let v = load(&body, 3)?;
                if !t(&gd(&v, "ok", Value::Bool(true), 4)?) {
                    p.print(&sew::dumps_default(&v));
                    return Ok(Exit::Ok);
                }
                let sw = sub(&v, "sew", 6)?;
                let yn = |k: &str| -> R<&'static str> {
                    Ok(if t(&g(&sw, k, 9)?) { "yes" } else { "no" })
                };
                p.print(&format!(
                    "Sew flags:    app-scoped sessions {} \u{b7} home client {} \u{b7} app origin (voice for Apps) {} \u{b7} every App origin {} \u{b7} {PRODUCT_NAME}-managed avatar {}",
                    yn("appScopedSessions")?,
                    yn("homeClient")?,
                    yn("appOrigin")?,
                    yn("appOrigins")?,
                    yn("k2ManagedAvatar")?
                ));
                if !t(&g(&sw, "appScopedSessions", 10)?) {
                    p.print(&format!("              not app-scoped yet: only the owner gives chat ({c} app user chat <user> member --app <app>),"));
                    p.print("              and an App can see all chat its people can");
                }
                let hc = g(&v, "homeClient", 13)?;
                let tail = if t(&gd(&v, "homeClientDelivered", Value::Bool(true), 14)?) {
                    String::new()
                } else if t(&g(&v, "homeClientError", 14)?) {
                    format!(
                        "  (wanted, not set: {})",
                        sv(&ix(&v, "homeClientError", 14)?)
                    )
                } else {
                    "  (sending)".into()
                };
                p.print(&format!("home client:  {}{tail}", sv(&or(hc, st("none")))));
                for a in py_iter(&or(g(&v, "apps", 15)?, Value::Array(Vec::new())), 15)? {
                    let origin = if t(&g(&a, "origin", 16)?) {
                        print::cat("  origin ", &ix(&a, "origin", 16)?, 16)?
                    } else {
                        String::new()
                    };
                    p.print(&format!(
                        "  {} {} {}{origin}{}",
                        ljust(&sv(&g(&a, "app", 16)?), 38),
                        ljust(&sv(&or(g(&a, "name", 16)?, st(""))), 20),
                        if t(&g(&a, "hiddenFromHome", 16)?) {
                            "App-only"
                        } else {
                            "shown in the home client"
                        },
                        if t(&g(&a, "delivered", 17)?) {
                            ""
                        } else {
                            "  (sending)"
                        }
                    ));
                }
                Ok(Exit::Ok)
            })();
            Ok(lines(r, p))
        }
        Apps::PassKeys { sub: which, json } => {
            let body = match *which {
                "rotate" => ctx.call(true, "/cli/sew/keys/rotate", "{}", false)?,
                "drop-previous" => ctx.call(true, "/cli/sew/keys/drop-previous", "{}", false)?,
                _ => ctx.call(false, "/cli/sew/keys", "", false)?,
            };
            if *json {
                return Ok(raw(&body));
            }
            let mut p = Py::default();
            let r = (|| -> R<Exit> {
                let v = load(&body, 3)?;
                let when = |x: &Value, line: u32| -> R<String> {
                    if t(x) {
                        local_minutes(x, line)
                    } else {
                        Ok("-".into())
                    }
                };
                match *which {
                    "rotate" => p.print("rotated: the next key signs chat passes now"),
                    "drop-previous" => p.print("dropped the retiring key"),
                    _ => {}
                }
                let keys = or(g(&v, "keys", 10)?, Value::Array(Vec::new()));
                if !t(&keys) && *which == "show" {
                    p.print(&format!(
                        "no chat-pass keys yet ({PRODUCT_NAME} makes them on the first chat pass)"
                    ));
                }
                for k in py_iter(&keys, 13)? {
                    let mut extra = String::new();
                    if is_current(&g(&k, "state", 15)?) && t(&g(&k, "rotateDueAt", 15)?) {
                        extra = format!("  rotates by {}", when(&ix(&k, "rotateDueAt", 16)?, 16)?);
                    }
                    if t(&g(&k, "retireAfter", 17)?) {
                        extra = format!("  retires after {}", when(&ix(&k, "retireAfter", 18)?, 18)?);
                    }
                    p.print(&format!(
                        "{} {}  published {}{extra}",
                        ljust(&sv(&g(&k, "state", 19)?), 9),
                        sv(&g(&k, "kid", 19)?),
                        when(&g(&k, "publishedAt", 19)?, 19)?
                    ));
                }
                Ok(Exit::Ok)
            })();
            Ok(lines(r, p))
        }
        Apps::Meet {
            route,
            payload,
            verb,
            json,
        } => {
            let body = ctx.call(true, route, payload, false)?;
            Ok(show(&body, verb, *json))
        }
    }
}

fn is_current(v: &Value) -> bool {
    matches!(v, Value::Str(s) if s.as_str() == Some("current"))
}
