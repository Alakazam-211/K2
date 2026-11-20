//! The chat administration verbs (`cli/src/hostmail-app-cert-backup.sh`):
//! `people`, `approvals`, `on-behalf` and `log`. They ride chat's call,
//! refusal line, usage line and printer ([`crate::families::chat`]), as
//! the shell's ride `_chat_call`, `_chat_fail`, `_chat_usage_err` and
//! `_chat_print`. A held approval is `_hostchat_confirmed_post`: the
//! confirm the daemon names, over the vault socket, then the same body
//! with the token.

use super::main_verbs::token;
use super::print::{g, load, or, st, sub, sv, t, Py, R};
use super::{cj, Ctx, Plan};
use crate::exit::Exit;
use crate::families::voice::sew::{self, Field};
use crate::families::{chat, ljust, py_iter, raised, Shown};
use crate::out::json::Value;
use crate::out;
use crate::transport::encode::quote;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// A chat administration verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admin {
    PeopleList { json: bool },
    People { route: &'static str, payload: String, json: bool },
    Approvals { query: String, json: bool },
    Approve { route: &'static str, payload: String, label: String, json: bool },
    Deny { payload: String, json: bool },
    OnBehalfList { query: String, json: bool },
    OnBehalfApprove { payload: String, json: bool },
    OnBehalfDecline { payload: String, json: bool },
    Log { query: String, json: bool },
}

fn bad(hint: impl AsRef<str>) -> Plan {
    Plan::Bad(chat::bad_line(hint.as_ref()))
}

fn s(t: &str) -> Field {
    Field::S(t.to_string())
}

/// `people …`.
pub fn people(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&"", r)) => ("list", r),
        Some((&s, r)) => (s, r),
        None => ("list", &[]),
    };
    let (mut json, mut guest) = (false, "");
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--json" => json = true,
            "--guest" => match rest.get(i + 1).copied().unwrap_or("") {
                "on" | "true" | "yes" => {
                    guest = "1";
                    i += 1;
                }
                "off" | "false" | "no" => {
                    guest = "0";
                    i += 1;
                }
                _ => guest = "1",
            },
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => return bad(format!("unknown flag {w} ({c} hostchat --help)")),
            w => pos.push(w),
        }
        i += 1;
    }
    let user = pos.first().copied().unwrap_or("");
    let user = user.strip_prefix('@').unwrap_or(user);
    let g = Field::B(guest.to_string());
    let (route, payload) = match sub {
        "list" => return Plan::Admin(Admin::PeopleList { json }),
        "add" | "grant" => {
            if user.is_empty() {
                return bad(format!("{c} hostchat people add <user> [--guest]"));
            }
            ("/cli/sew/admin/people/grant", cj(&[("user", s(user)), ("guest", g)]))
        }
        "set" | "update" => {
            if user.is_empty() || guest.is_empty() {
                return bad(format!("{c} hostchat people set <user> --guest on|off"));
            }
            ("/cli/sew/admin/people/update", cj(&[("user", s(user)), ("guest", g)]))
        }
        "remove" | "revoke" => {
            if user.is_empty() {
                return bad(format!("{c} hostchat people remove <user>"));
            }
            ("/cli/sew/admin/people/revoke", cj(&[("user", s(user))]))
        }
        _ => return bad(format!("{c} hostchat people list|add|remove|set \u{2026}")),
    };
    Plan::Admin(Admin::People {
        route,
        payload,
        json,
    })
}

/// `approvals …`.
pub fn approvals(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&"", r)) => ("list", r),
        Some((s, _)) if s.starts_with("out_") => ("list", words),
        Some((&s, r)) => (s, r),
        None => ("list", &[]),
    };
    let (mut json, mut note, mut ws) = (false, "", "");
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let next = rest.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => json = true,
            "--note" => {
                if next.is_empty() {
                    return bad("--note \"why\"");
                }
                note = next;
                i += 1;
            }
            "--workspace" => {
                if next.is_empty() {
                    return bad("--workspace <workspace>");
                }
                ws = next;
                i += 1;
            }
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => return bad(format!("unknown flag {w} ({c} hostchat --help)")),
            w => pos.push(w),
        }
        i += 1;
    }
    match sub {
        "list" => {
            let query = if ws.is_empty() {
                String::new()
            } else {
                format!("workspace={}", quote(ws))
            };
            Plan::Admin(Admin::Approvals { query, json })
        }
        "approve" => {
            if pos.is_empty() {
                return bad(format!("{c} hostchat approvals approve <out_\u{2026}> [<out_\u{2026}>\u{2026}] [--note \"\u{2026}\"]"));
            }
            let (route, payload, label) = if pos.len() == 1 {
                (
                    "/cli/sew/approvals/approve",
                    cj(&[("id", s(pos[0])), ("note", s(note))]),
                    "Approve a queued chat post".to_string(),
                )
            } else {
                let ids = Value::Array(pos.iter().map(|p| st(p)).collect());
                (
                    "/cli/sew/approvals/approve-batch",
                    cj(&[("ids", Field::J(sew::dumps_default(&ids))), ("note", s(note))]),
                    format!("Approve {} queued chat posts", pos.len()),
                )
            };
            Plan::Admin(Admin::Approve {
                route,
                payload,
                label,
                json,
            })
        }
        "deny" => {
            if pos.is_empty() || pos[0].is_empty() || note.is_empty() {
                return bad(format!("{c} hostchat approvals deny <out_\u{2026}> --note \"why\" (the agent sees it)"));
            }
            Plan::Admin(Admin::Deny {
                payload: cj(&[("id", s(pos[0])), ("note", s(note))]),
                json,
            })
        }
        _ => bad(format!("{c} hostchat approvals [list] | approve <out_\u{2026}>\u{2026} | deny <out_\u{2026}> --note \"\u{2026}\"")),
    }
}

/// `on-behalf …`.
pub fn on_behalf(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&"", r)) => ("list", r),
        Some((s, _)) if s.starts_with('-') => ("list", words),
        Some((&s, r)) => (s, r),
        None => ("list", &[]),
    };
    let (mut json, mut state, mut limit, mut note) = (false, "", "", "");
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let next = rest.get(i + 1).copied().unwrap_or("");
        let slot = match w {
            "--json" => {
                json = true;
                i += 1;
                continue;
            }
            "--state" => (&mut state, "--state pending|approved|declined|all"),
            "--limit" => (&mut limit, "--limit N"),
            "--note" => (&mut note, "--note \"\u{2026}\""),
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => return bad(format!("unknown flag {w} ({c} hostchat --help)")),
            w => {
                pos.push(w);
                i += 1;
                continue;
            }
        };
        if next.is_empty() {
            return bad(slot.1);
        }
        *slot.0 = next;
        i += 2;
    }
    let id = pos.first().copied().unwrap_or("");
    match sub {
        "list" => {
            let mut query = String::new();
            if !state.is_empty() {
                query = format!("state={}", quote(state));
            }
            if !limit.is_empty() {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("limit={}", quote(limit)));
            }
            Plan::Admin(Admin::OnBehalfList { query, json })
        }
        "approve" | "decline" => {
            if id.is_empty() {
                return bad(format!("{c} hostchat on-behalf {sub} <act_\u{2026}|msg_\u{2026}> [--note \"\u{2026}\"]"));
            }
            let payload = cj(&[("id", s(id)), ("decision", s(sub)), ("note", s(note))]);
            Plan::Admin(if sub == "approve" {
                Admin::OnBehalfApprove { payload, json }
            } else {
                Admin::OnBehalfDecline { payload, json }
            })
        }
        _ => bad(format!("{c} hostchat on-behalf [list] | approve <act_\u{2026}|msg_\u{2026}> | decline <act_\u{2026}|msg_\u{2026}>")),
    }
}

/// `log …`.
pub fn log(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (mut json, mut limit) = (false, "");
    let mut i = 0;
    while i < words.len() {
        let next = words.get(i + 1).copied().unwrap_or("");
        match words[i] {
            "--json" => json = true,
            "--limit" => {
                if next.is_empty() {
                    return bad("--limit N");
                }
                limit = next;
                i += 1;
            }
            "-h" | "--help" => return Plan::Help,
            _ => return bad(format!("{c} hostchat log [--limit N]")),
        }
        i += 1;
    }
    let query = if limit.is_empty() {
        String::new()
    } else {
        format!("limit={}", quote(limit))
    };
    Plan::Admin(Admin::Log { query, json })
}

/// `_chat_call`: the body of a 200, or the exit after `_chat_fail`'s line.
fn call(ctx: &mut Ctx, post: bool, path: &str, payload: &str, tcp: bool) -> Result<String, Exit> {
    chat::call(&mut ctx.client, &mut ctx.closing, post, path, payload, tcp)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// `_chat_print <verb> <as_json>`.
fn chat_show(body: &str, verb: &str, json: bool) -> Exit {
    if json {
        return match out::stdout(&format!("{body}\n")) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    chat::print(body, verb).print()
}

/// A reader's lines, or where it raised.
fn lines(r: R<()>, p: Py) -> Exit {
    match r {
        Ok(()) => Shown::out(p.out).print(),
        Err(e) => raised(p.out, p.err, &e).print(),
    }
}

pub fn run(ctx: &mut Ctx, a: &Admin) -> Exit {
    match step(ctx, a) {
        Ok(e) | Err(e) => e,
    }
}

fn step(ctx: &mut Ctx, a: &Admin) -> Result<Exit, Exit> {
    match a {
        Admin::PeopleList { json } => {
            let body = call(ctx, false, "/cli/sew/admin/people", "", false)?;
            if *json {
                return Ok(chat_show(&body, "", true));
            }
            let mut p = Py::default();
            let r = (|| -> R<()> {
                let v = load(&body, 3)?;
                let rows = or(g(&v, "people", 3)?, Value::Array(Vec::new()));
                if !t(&rows) {
                    p.print("(no people have chat here)");
                }
                for r in py_iter(&rows, 6)? {
                    let off = if t(&super::print::gd(&r, "enabled", Value::Bool(true), 7)?) {
                        ""
                    } else {
                        "  (off)"
                    };
                    p.print(&format!(
                        "{} {} {}{off}",
                        ljust(&sv(&g(&r, "user", 7)?), 24),
                        ljust(&sv(&g(&r, "subjectKind", 7)?), 8),
                        ljust(&sv(&g(&r, "scope", 7)?), 6)
                    ));
                }
                Ok(())
            })();
            Ok(lines(r, p))
        }
        Admin::People {
            route,
            payload,
            json,
        } => {
            let body = call(ctx, true, route, payload, false)?;
            Ok(chat_show(&body, "people", *json))
        }
        Admin::Approvals { query, json } => {
            let body = call(ctx, false, "/cli/sew/approvals", query, false)?;
            Ok(chat_show(&body, "approvals", *json))
        }
        Admin::Approve {
            route,
            payload,
            label,
            json,
        } => {
            let body = confirmed_post(ctx, route, payload, label)?;
            Ok(chat_show(&body, "approve", *json))
        }
        Admin::Deny { payload, json } => {
            let body = call(ctx, true, "/cli/sew/approvals/deny", payload, false)?;
            Ok(chat_show(&body, "deny", *json))
        }
        Admin::OnBehalfList { query, json } => {
            let body = call(ctx, false, "/cli/sew/on-behalf", query, false)?;
            if *json {
                return Ok(chat_show(&body, "", true));
            }
            let mut p = Py::default();
            let r = (|| -> R<()> {
                let v = load(&body, 3)?;
                let rows = or(g(&v, "items", 4)?, Value::Array(Vec::new()));
                p.print(&format!("person: {}", sv(&g(&v, "person", 5)?)));
                if !t(&rows) {
                    p.print("(no posts made on your behalf)");
                }
                for r in py_iter(&rows, 8)? {
                    let mut line = format!(
                        "{}  {}  {}  {}  by {}",
                        sv(&g(&r, "write", 9)?),
                        sv(&or(g(&r, "messageId", 9)?, st("-"))),
                        sv(&g(&r, "action", 9)?),
                        sv(&g(&r, "state", 9)?),
                        sv(&g(&r, "workspace", 9)?)
                    );
                    if t(&g(&r, "decidedBy", 10)?) {
                        line.push_str(&format!(
                            "  reviewed by {} ({})",
                            sv(&super::print::ix(&r, "decidedBy", 11)?),
                            sv(&or(g(&r, "confirmMethod", 11)?, g(&r, "decidedKind", 11)?))
                        ));
                    }
                    if t(&g(&r, "note", 12)?) {
                        line.push_str(&format!("  note: {}", sv(&super::print::ix(&r, "note", 13)?)));
                    }
                    p.print(&line);
                }
                Ok(())
            })();
            Ok(lines(r, p))
        }
        Admin::OnBehalfApprove { payload, json } => {
            if ctx.in_session {
                out::stderr_line(&format!("{{\"error\":\"person_only\",\"hint\":\"only the person an assistant spoke for approves it, from a terminal outside {PRODUCT_NAME}\"}}"));
                return Err(Exit::Refused);
            }
            let body = confirmed_post(
                ctx,
                "/cli/sew/on-behalf/review",
                payload,
                "Approve what an assistant said on your behalf",
            )?;
            Ok(chat_show(&body, "on-behalf", *json))
        }
        Admin::OnBehalfDecline { payload, json } => {
            let body = call(ctx, true, "/cli/sew/on-behalf/review", payload, false)?;
            Ok(chat_show(&body, "on-behalf", *json))
        }
        Admin::Log { query, json } => {
            let body = call(ctx, false, "/cli/sew/admin/log", query, false)?;
            if *json {
                return Ok(chat_show(&body, "", true));
            }
            let mut p = Py::default();
            let r = (|| -> R<()> {
                let v = load(&body, 3)?;
                let rows = or(g(&v, "entries", 3)?, Value::Array(Vec::new()));
                if !t(&rows) {
                    p.print("(no chat administration yet)");
                }
                for e in py_iter(&rows, 6)? {
                    let at = g(&e, "at", 8)?;
                    let when = chat::when(&at);
                    let reason = if t(&g(&e, "reason", 12)?) {
                        let r = super::print::ix(&e, "reason", 12)?;
                        format!("{})", super::print::cat("  (", &r, 12)?)
                    } else {
                        String::new()
                    };
                    let blank = |k: &str| -> R<String> { Ok(sv(&or(g(&e, k, 11)?, st("")))) };
                    p.print(&format!(
                        "{when}  {} {} {} {} {} {}{reason}",
                        ljust(&sv(&g(&e, "outcome", 11)?), 8),
                        ljust(&sv(&g(&e, "actor", 11)?), 22),
                        ljust(&sv(&g(&e, "op", 11)?), 24),
                        blank("workspace")?,
                        blank("space")?,
                        blank("target")?
                    ));
                }
                Ok(())
            })();
            Ok(lines(r, p))
        }
    }
}

/// `_hostchat_confirmed_post <path> <payload> <label>`: the answer's body.
fn confirmed_post(ctx: &mut Ctx, path: &str, payload: &str, label: &str) -> Result<String, Exit> {
    let sent = sew::send(&mut ctx.client, true, path, payload, false)?;
    sew::note(&mut ctx.closing, &sent.body, ctx.client.passport().is_some());
    if sent.status == "200" {
        return Ok(String::from_utf8_lossy(&sent.body).into_owned());
    }
    let text = String::from_utf8_lossy(&sent.body).into_owned();
    let fail = |status: &str, body: &[u8]| -> Exit {
        let (line, exit) = chat::fail_line(status, body);
        out::stderr_line(&line);
        exit
    };
    // The code, with any failure read as none (`try: … except: print("")`).
    let code = load(&text, 2)
        .and_then(|v| sub(&v, "error", 2))
        .and_then(|e| g(&e, "code", 2))
        .map(|c| sv(&or(c, st(""))))
        .unwrap_or_default();
    if sent.status != "403" || code != "confirm_required" {
        return Err(fail(&sent.status, &sent.body));
    }
    if ctx.in_session {
        out::stderr_line(&format!("{{\"error\":\"confirm_required\",\"hint\":\"a person confirms this from a terminal outside {PRODUCT_NAME} (Touch ID or the vault password), never an agent\"}}"));
        return Err(Exit::Refused);
    }
    let detail = |k: &str| -> Result<String, Exit> {
        let r = load(&text, 1)
            .and_then(|v| sub(&v, "error", 1))
            .and_then(|e| sub(&e, "details", 1))
            .and_then(|d| g(&d, k, 1));
        match r {
            Ok(x) => {
                let mut x = sv(&or(x, st("")));
                while x.ends_with('\n') {
                    x.pop();
                }
                Ok(x)
            }
            Err(e) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), e.text().as_bytes());
                Err(Exit::Error)
            }
        }
    };
    let action = detail("action")?;
    let secret = detail("secretId")?;
    if action.is_empty() {
        return Err(fail(&sent.status, &sent.body));
    }
    let tok = token(ctx, &action, &secret, label)?;
    let b = crate::confirm::with_confirm_token(payload, &tok, None).unwrap_or_default();
    drop(tok);
    call(ctx, true, path, &b, true)
}
