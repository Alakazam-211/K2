//! `k5 notifications`: this server's email notices, as the shell CLI's
//! `cmd_notifications` and its `cmd_notifications_*` verbs
//! (`cli/src/app-auth.sh`). [`super::notify`] uses its readers too.
//!
//! 1. The settings and the gates ([`cp::open`]); an open tool, and even
//!    its help needs the daemon's settings, as the shell's does.
//! 2. `notifications` alone or `help`: the help. `email <verb>`:
//!    `status` (the default), `sender add|list|check|set|resume|rm`,
//!    `system-sender show|set|clear|test`, `relay add|list|rm|test`,
//!    `address [show]|set|remove`, `suppressions [list]|add|rm`, `agent
//!    [list]|on|off`, `log`, `settings`. The reads go over TCP
//!    (`_compute_call … tcp`); the writes are `_email_post` (with its
//!    confirm, [`cp::email_post`]) or `_app_rt_post`. Each answer is
//!    printed by the shell's Python, its tracebacks too.
//! 3. The EXIT trap's lines ([`cp::close`]).
//!
//! A flag that takes a value and comes last is the shell's `_need_value`
//! line, exit 2. `relay add` reads the
//! password from the terminal without echo, else the first line of stdin;
//! it goes only into the request body. A line with `-h` or `--help` in
//! it is the shell's: since 0.46.17 it prints the notifications help and
//! changes nothing (`email sender resume --help` resumed a sender named
//! `--help`, `relay rm --help` removed one), and the front sends it to
//! that page or on ([`crate::front`]), so it never gets here.

use super::app::call::{Cj, Ctx};
use super::app::py::{self as p, Out};
use super::approve::cp::{self, one_line, page, start, stdout_line, usage, Run};
use super::{Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::transport::encode::quote;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["notifications"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

const E: &str = "/cli/notifications/email";

/// `k5 notifications ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let mut r = match start(args) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let exit = match words.first().copied().unwrap_or("") {
        "email" => email(&mut r, &words[1..]),
        "" | "help" => page("notifications"),
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email \u{2026} (",
            cli_name!(),
            " notifications --help)"
        )),
    };
    Some(cp::close(r.ctx, exit))
}

/// What a reader printed, or how it ended.
fn shown(r: Result<Shown, Exit>) -> Exit {
    match r {
        Ok(s) => s.print(),
        Err(e) => e,
    }
}

/// `v.get(k)` as `%s` prints it.
fn gs(v: &Value, k: &str, line: u32) -> Result<String, Raise> {
    Ok(py::str_of(&p::dget(v, k, line)?))
}

/// `v + "<tail>"` where `v` must be a string.
fn plus(v: &Value, tail: &str, line: u32) -> Result<String, Raise> {
    super::remind::shared::plus_str(v, tail, &super::remind::shared::at_line(line))
}

/// `{True: "ok", False: "FAILED"}.get(v, "never")`.
fn test_word(v: &Value, line: u32) -> Result<&'static str, Raise> {
    match v {
        Value::Bool(true) => Ok("ok"),
        Value::Bool(false) => Ok("FAILED"),
        Value::Int(d) if d == "1" => Ok("ok"),
        Value::Int(d) if d == "0" || d == "-0" => Ok("FAILED"),
        Value::Float(f) if *f == 1.0 => Ok("ok"),
        Value::Float(f) if *f == 0.0 => Ok("FAILED"),
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(v)),
        )),
        _ => Ok("never"),
    }
}

/// `v[:n]`.
fn head(v: &Value, n: usize, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v).chars().take(n).collect()),
        Value::Array(items) => Ok(py::str_of(&Value::Array(
            items.iter().take(n).cloned().collect(),
        ))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// A JSON body of string pairs, as `json.dumps` prints it.
fn dumps(pairs: Vec<(&str, Value)>) -> String {
    let v = Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect());
    json::to_string(&v, &Options::default()).expect("json.dumps of JSON values")
}

fn email(r: &mut Run, words: &[&str]) -> Exit {
    let (sub, rest) = match words.split_first() {
        Some((s, rest)) => (*s, rest),
        None => ("status", &[][..]),
    };
    let ctx = &mut r.ctx;
    let home = r.home.as_path();
    match sub {
        "help" => page("notifications"),
        "status" | "--json" => {
            let json_out = sub == "--json" || rest.first() == Some(&"--json");
            shown(cp::call_tcp(ctx, false, E, "").map(|b| {
                if json_out {
                    Shown::out(json_text(&b))
                } else {
                    status_reader(&b)
                }
            }))
        }
        "sender" => sender(ctx, home, rest),
        "system-sender" => system(ctx, home, rest),
        "relay" | "relays" => relay(ctx, home, rest),
        "address" => address(ctx, rest),
        "suppressions" | "suppression" => suppressions(ctx, rest),
        "agent" | "agents" => agent(ctx, home, rest),
        "log" => log(ctx, rest),
        "settings" => settings(ctx, home, rest),
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email status|sender|system-sender|relay|suppressions|address|agent|log|settings (--help)"
        )),
    }
}

/// `printf '%s\n' "$_CP_BODY"` as text.
fn json_text(body: &[u8]) -> String {
    let mut b = super::without_nuls(body);
    b.push(b'\n');
    String::from_utf8_lossy(&b).into_owned()
}

/// `email status`'s reader.
pub fn status_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let hosted = if p::truthy(&p::dget(&v, "hostedMail", 4)?) {
            "here"
        } else {
            "not on this server (use a relay)"
        };
        let box_switch = gs(&v, "boxSwitch", 4)?;
        let air = if p::truthy(&p::dget(&v, "airgap", 4)?) {
            "; AIR-GAP ON"
        } else {
            ""
        };
        o.line(&format!(
            "Email Notifications: {box_switch}; hosted mail {hosted}{air}"
        ));
        let sys_s = p::dget(&v, "systemSender", 5)?;
        let line6 = if p::truthy(&sys_s) {
            let a = plus(&p::dget(&sys_s, "address", 6)?, " (", 6)?;
            let st = p::concat(&a, &p::dget(&sys_s, "state", 6)?, 6)?;
            st + ")"
        } else {
            "none".to_string()
        };
        o.line(&format!("system sender: {line6}"));
        for s in p::iter(&p::or(p::dget(&v, "senders", 7)?, p::empty_list()), 7)? {
            let owner = p::or(p::dget(&s, "owner", 8)?, p::text("unassigned"));
            o.line(&format!(
                "  {}  {} {} {} {}",
                gs(&s, "id", 8)?,
                p::left(&p::dget(&s, "address", 8)?, 34),
                p::left(&p::dget(&s, "path", 8)?, 6),
                p::left(&p::dget(&s, "state", 8)?, 11),
                py::str_of(&owner)
            ));
        }
        for r in p::iter(&p::or(p::dget(&v, "relays", 9)?, p::empty_list()), 9)? {
            let fields = ["id", "host", "port", "tls", "username", "fromDomain"]
                .iter()
                .map(|k| gs(&r, k, 10))
                .collect::<Result<Vec<_>, _>>()?;
            let test = test_word(&p::dget(&r, "lastTestOk", 10)?, 10)?;
            o.line(&format!(
                "  relay {}  {}:{} {}  user {}  for {}  test {test}",
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5]
            ));
        }
        let st = p::or(p::dget(&v, "settings", 11)?, Value::Object(vec![]));
        let days = gs(&st, "logRetentionDays", 12)?;
        let owner = p::or(
            p::dget(&st, "ownerEmail", 12)?,
            p::text(concat!(
                "not set (",
                cli_name!(),
                " notifications email settings --owner-email)"
            )),
        );
        o.line(&format!(
            "log kept {days} days; owner address {}",
            py::str_of(&owner)
        ));
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `_notif_sender_print`.
pub fn sender_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let mut s = p::dget(&v, "sender", 4)?;
        if !p::truthy(&s) {
            s = p::dget(&v, "systemSender", 4)?;
        }
        if !p::truthy(&s) {
            o.line("no sender");
            return Ok(Exit::Ok);
        }
        let owner = p::or(p::dget(&s, "owner", 7)?, p::text("unassigned"));
        let f = ["id", "address", "path", "state"]
            .iter()
            .map(|k| gs(&s, k, 8))
            .collect::<Result<Vec<_>, _>>()?;
        o.line(&format!(
            "{}  {}  {}  {}  {} ({})",
            f[0],
            f[1],
            f[2],
            f[3],
            py::str_of(&owner),
            gs(&s, "fromName", 8)?
        ));
        if p::truthy(&p::dget(&s, "envelope", 9)?) {
            o.line(&format!(
                "  sends through {} (bounces and the signature use it; the From is {})",
                gs(&s, "envelope", 10)?,
                gs(&s, "address", 10)?
            ));
        }
        let nr = p::dget(&s, "notReady", 11)?;
        if p::truthy(&nr) {
            o.line(&format!(
                "  not ready: {}: {}",
                gs(&nr, "code", 13)?,
                gs(&nr, "hint", 13)?
            ));
        }
        if p::truthy(&p::dget(&s, "next", 14)?) {
            o.line(&p::concat("  next: ", &p::idx(&s, "next", 15)?, 15)?);
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `_notif_test_print`.
pub fn test_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let mut id = p::dget(&v, "id", 4)?;
        if !p::truthy(&id) {
            id = p::dget(&v, "sender", 4)?;
        }
        o.line(&format!(
            "test email {}: {} (to {})",
            py::str_of(&id),
            gs(&v, "result", 4)?,
            gs(&v, "to", 4)?
        ));
        let c = p::or(p::dget(&v, "checks", 5)?, Value::Object(vec![]));
        let d = p::or(p::dget(&c, "dmarc", 6)?, Value::Object(vec![]));
        let dmarc = if p::truthy(&p::dget(&d, "ok", 7)?) {
            format!("found at {}, p={}", gs(&d, "at", 7)?, gs(&d, "policy", 7)?)
        } else {
            format!("missing: {}", gs(&d, "hint", 7)?)
        };
        o.line(&format!("  DMARC: {dmarc}"));
        if p::dget(&c, "dkimPublished", 8)? != Value::Null {
            o.line(&format!(
                "  DKIM key published: {}; SPF published: {}",
                gs(&c, "dkimPublished", 9)?,
                gs(&c, "spfPublished", 9)?
            ));
        }
        if p::truthy(&p::dget(&c, "note", 10)?) {
            o.line(&p::concat("  ", &p::idx(&c, "note", 11)?, 11)?);
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// A word list's flags: `--name v` and the like. `Err`: the command's end.
struct Flags<'a> {
    words: &'a [&'a str],
    i: usize,
}

impl<'a> Flags<'a> {
    fn new(words: &'a [&'a str]) -> Self {
        Flags { words, i: 0 }
    }

    /// The next word, if any.
    fn next(&mut self) -> Option<&'a str> {
        let w = self.words.get(self.i).copied();
        self.i += 1;
        w
    }

    /// `x="${2:-}"; shift 2`: the value, or `_need_value`'s line (exit 2)
    /// after the flag just read.
    fn value(&mut self) -> Result<&'a str, Exit> {
        let flag = self.i.checked_sub(1).map_or("", |j| self.words[j]);
        let v = self
            .words
            .get(self.i)
            .copied()
            .ok_or_else(|| out::stop_need_value(flag))?;
        self.i += 1;
        Ok(v)
    }
}

fn sender(ctx: &mut Ctx, home: &Path, words: &[&str]) -> Exit {
    let verb = words.first().copied().unwrap_or("list");
    let rest = words.get(1..).unwrap_or_default();
    match verb {
        "add" => {
            let (mut addr, mut name, mut relay, mut via) = ("", "", "", "");
            let mut lock = false;
            let mut rts: Vec<&str> = Vec::new();
            let mut f = Flags::new(rest);
            while let Some(w) = f.next() {
                let r = match w {
                    "--name" => f.value().map(|v| name = v),
                    "--lock-name" => {
                        lock = true;
                        Ok(())
                    }
                    "--reply-to" => f.value().map(|v| rts.push(v)),
                    "--relay" => f.value().map(|v| relay = v),
                    "--via" => f.value().map(|v| via = v),
                    w if w.starts_with('-') => Err(usage(&format!("Unknown flag: {w}"))),
                    w => {
                        addr = w;
                        Ok(())
                    }
                };
                if let Err(e) = r {
                    return e;
                }
            }
            if addr.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email sender add <address> [--name N] [--lock-name] [--reply-to A]\u{2026} [--relay ID | --via <bounce subdomain>]"
                ));
            }
            let mut b = vec![("address", p::text(addr))];
            if !name.is_empty() {
                b.push(("name", p::text(name)));
            }
            if lock {
                b.push(("nameLocked", Value::Bool(true)));
            }
            if !relay.is_empty() {
                b.push(("relay", p::text(relay)));
            }
            if !via.is_empty() {
                b.push(("via", p::text(via)));
            }
            if !rts.is_empty() {
                b.push((
                    "replyTo",
                    Value::Array(rts.iter().map(|r| p::text(r)).collect()),
                ));
            }
            shown(
                cp::email_post(ctx, home, &format!("{E}/sender/add"), &dumps(b))
                    .map(|b| sender_reader(&b)),
            )
        }
        "list" | "ls" => {
            let (mut q, mut json_out) = ("", false);
            for w in rest {
                match *w {
                    "--all" => q = "all=1",
                    "--json" => json_out = true,
                    w => return usage(&format!("Unknown flag: {w}")),
                }
            }
            shown(
                cp::call_tcp(ctx, false, &format!("{E}/senders"), q).map(|b| {
                    if json_out {
                        Shown::out(json_text(&b))
                    } else {
                        senders_reader(&b)
                    }
                }),
            )
        }
        "resume" => {
            let who = match rest {
                [w] if !w.is_empty() => *w,
                _ => {
                    return usage(concat!(
                        "usage: ",
                        cli_name!(),
                        " notifications email sender resume <sender>"
                    ))
                }
            };
            let body = dumps(vec![("sender", p::text(who))]);
            shown(
                cp::email_post(ctx, home, &format!("{E}/sender/resume"), &body).map(|b| {
                    one_line(&b, |v| {
                        Ok(format!(
                            "resumed {}: its sends go out again (a new 24-hour count starts now)",
                            gs(v, "sender", 1)?
                        ))
                    })
                }),
            )
        }
        "check" | "set" | "rm" | "remove" => {
            let (mut who, mut name) = ("", "");
            let mut lock: Option<bool> = None;
            let mut rts: Vec<&str> = Vec::new();
            let mut f = Flags::new(rest);
            while let Some(w) = f.next() {
                let r = match w {
                    "--name" => f.value().map(|v| name = v),
                    "--lock-name" => {
                        lock = Some(true);
                        Ok(())
                    }
                    "--unlock-name" => {
                        lock = Some(false);
                        Ok(())
                    }
                    "--reply-to" => f.value().map(|v| rts.push(v)),
                    w if w.starts_with('-') => Err(usage(&format!("Unknown flag: {w}"))),
                    w => {
                        who = w;
                        Ok(())
                    }
                };
                if let Err(e) = r {
                    return e;
                }
            }
            if who.is_empty() {
                return usage(&format!(
                    concat!(
                        "usage: ",
                        cli_name!(),
                        " notifications email sender {} <sender>"
                    ),
                    verb
                ));
            }
            let verb = if verb == "remove" { "rm" } else { verb };
            let mut b = vec![("sender", p::text(who))];
            if !name.is_empty() {
                b.push(("name", p::text(name)));
            }
            if let Some(l) = lock {
                b.push(("nameLocked", Value::Bool(l)));
            }
            if !rts.is_empty() {
                b.push((
                    "replyTo",
                    Value::Array(rts.iter().map(|r| p::text(r)).collect()),
                ));
            }
            let answer = cp::email_post(ctx, home, &format!("{E}/sender/{verb}"), &dumps(b));
            shown(answer.map(|b| {
                if verb == "rm" {
                    one_line(&b, |v| {
                        Ok(format!(
                            "removed {} ({}); its address stays reserved {} days",
                            gs(v, "removed", 1)?,
                            gs(v, "address", 1)?,
                            gs(v, "reservedDays", 1)?
                        ))
                    })
                } else {
                    sender_reader(&b)
                }
            }))
        }
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email sender add|list|check|set|resume|rm"
        )),
    }
}

/// `sender list`'s reader.
pub fn senders_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rows = p::or(p::dget(&v, "senders", 3)?, p::empty_list());
        if !p::truthy(&rows) {
            o.line(concat!(
                "No senders yet: ",
                cli_name!(),
                " notifications email sender add noreply@<your domain>"
            ));
            return Ok(Exit::Ok);
        }
        for s in p::iter(&rows, 6)? {
            let pz = p::dget(&s, "paused", 7)?;
            let paused = if p::truthy(&pz) {
                format!(
                    concat!(
                        "  PAUSED ({}; resume: ",
                        cli_name!(),
                        " notifications email sender resume {})"
                    ),
                    gs(&pz, "reason", 8)?,
                    gs(&s, "address", 8)?
                )
            } else {
                String::new()
            };
            let owner = p::or(p::dget(&s, "owner", 9)?, p::text("unassigned"));
            o.line(&format!(
                "{}  {} {} {} {} {}{paused}",
                gs(&s, "id", 9)?,
                p::left(&p::dget(&s, "address", 9)?, 34),
                p::left(&p::dget(&s, "path", 9)?, 6),
                p::left(&p::dget(&s, "state", 9)?, 11),
                p::left(&owner, 18),
                gs(&s, "fromName", 9)?
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn system(ctx: &mut Ctx, home: &Path, words: &[&str]) -> Exit {
    let path = format!("{E}/system-sender");
    match words.first().copied().unwrap_or("show") {
        "show" => shown(cp::call_tcp(ctx, false, &path, "").map(|b| sender_reader(&b))),
        "set" => {
            let Some(s) = words.get(1).filter(|s| !s.is_empty()) else {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email system-sender set <sender>"
                ));
            };
            let body = Cj::new().s("sender", s).body();
            shown(
                cp::email_post(ctx, home, &format!("{path}/set"), &body).map(|b| sender_reader(&b)),
            )
        }
        "clear" => match cp::email_post(ctx, home, &format!("{path}/clear"), "{}") {
            Ok(_) => stdout_line("no system sender"),
            Err(e) => e,
        },
        "test" => {
            shown(cp::email_post(ctx, home, &format!("{path}/test"), "{}").map(|b| test_reader(&b)))
        }
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email system-sender show|set|clear|test"
        )),
    }
}

/// `relay add`'s password: on a terminal, asked without echo; else the
/// first line of stdin.
fn password() -> crate::prompt::Secret {
    use crate::prompt::{self, AtEof, Echo};
    if prompt::stdin_is_tty() {
        prompt::ask_hidden("Relay password (SMTP password or API key): ")
    } else {
        prompt::read_line(Echo::Shown, AtEof::Keep)
    }
}

fn relay(ctx: &mut Ctx, home: &Path, words: &[&str]) -> Exit {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("list") {
        "add" => {
            let (mut host, mut port, mut tls, mut user, mut dom) = ("", "", "", "", "");
            let mut f = Flags::new(rest);
            while let Some(w) = f.next() {
                let slot = match w {
                    "--host" => &mut host,
                    "--port" => &mut port,
                    "--tls" => &mut tls,
                    "--username" => &mut user,
                    "--from-domain" => &mut dom,
                    w => return usage(&format!("Unknown flag: {w}")),
                };
                match f.value() {
                    Ok(v) => *slot = v,
                    Err(e) => return e,
                }
            }
            if host.is_empty() || user.is_empty() || dom.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email relay add --host H [--port P] [--tls implicit|starttls] --username U --from-domain D (password on stdin)"
                ));
            }
            if !port.is_empty() && !port.bytes().all(|b| b.is_ascii_digit()) {
                return usage("--port is a number");
            }
            // Typed or piped: never over plain http to another computer.
            if !crate::prelude::plain_http::password_transport_ok() {
                return Exit::Usage;
            }
            let pw = password();
            if pw.is_empty() {
                return usage("no password given");
            }
            let mut b = vec![
                ("host", p::text(host)),
                ("username", p::text(user)),
                ("fromDomain", p::text(dom)),
                ("password", cp::env_text(pw.expose())),
            ];
            if !port.is_empty() {
                let digits = port.trim_start_matches('0');
                b.push((
                    "port",
                    Value::Int(if digits.is_empty() { "0" } else { digits }.to_string()),
                ));
            }
            if !tls.is_empty() {
                b.push(("tls", p::text(tls)));
            }
            let body = dumps(b);
            let answer = cp::email_post(ctx, home, &format!("{E}/relay/add"), &body);
            drop(body);
            shown(answer.map(|b| {
                let mut o = Out::default();
                let r = (|| {
                    let v = p::load(&b, 1)?;
                    let r = p::or(p::dget(&v, "relay", 1)?, Value::Object(vec![]));
                    let f = ["id", "host", "port", "tls", "fromDomain"]
                        .iter()
                        .map(|k| gs(&r, k, 1))
                        .collect::<Result<Vec<_>, _>>()?;
                    o.line(&format!(
                        "relay {}: {}:{} {} for {}",
                        f[0], f[1], f[2], f[3], f[4]
                    ));
                    let next = p::dget_or(&v, "next", p::text(""), 1)?;
                    o.line(&p::concat("  next: ", &next, 1)?);
                    Ok(Exit::Ok)
                })();
                o.done(r)
            }))
        }
        "list" | "ls" => shown(
            cp::call_tcp(ctx, false, &format!("{E}/relays"), "").map(|b| {
                let mut o = Out::default();
                let r = (|| {
                    let v = p::load(&b, 3)?;
                    let rows = p::or(p::dget(&v, "relays", 3)?, p::empty_list());
                    if !p::truthy(&rows) {
                        o.line("No relays.");
                        return Ok(Exit::Ok);
                    }
                    for r in p::iter(&rows, 6)? {
                        let f = ["id", "host", "port", "tls", "username", "fromDomain"]
                            .iter()
                            .map(|k| gs(&r, k, 7))
                            .collect::<Result<Vec<_>, _>>()?;
                        let t = test_word(&p::dget(&r, "lastTestOk", 7)?, 7)?;
                        o.line(&format!(
                            "{}  {}:{} {}  user {}  for {}  last test {t}",
                            f[0], f[1], f[2], f[3], f[4], f[5]
                        ));
                    }
                    Ok(Exit::Ok)
                })();
                o.done(r)
            }),
        ),
        "rm" | "remove" => {
            let Some(id) = rest.first().filter(|s| !s.is_empty()) else {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email relay rm <relay>"
                ));
            };
            let body = Cj::new().s("relay", id).body();
            match cp::email_post(ctx, home, &format!("{E}/relay/rm"), &body) {
                Ok(_) => stdout_line(&format!("removed relay {id}")),
                Err(e) => e,
            }
        }
        "test" => {
            let (mut relay, mut from) = ("", "");
            let mut f = Flags::new(rest);
            while let Some(w) = f.next() {
                let r = match w {
                    "--from" => f.value().map(|v| from = v),
                    w if w.starts_with('-') => Err(usage(&format!("Unknown flag: {w}"))),
                    w => {
                        relay = w;
                        Ok(())
                    }
                };
                if let Err(e) = r {
                    return e;
                }
            }
            if relay.is_empty() || from.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email relay test <relay> --from <sender>"
                ));
            }
            let body = Cj::new().s("relay", relay).s("from", from).body();
            shown(
                cp::email_post(ctx, home, &format!("{E}/relay/test"), &body)
                    .map(|b| test_reader(&b)),
            )
        }
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email relay add|list|rm|test"
        )),
    }
}

fn address(ctx: &mut Ctx, words: &[&str]) -> Exit {
    let mut verb = words.first().copied().unwrap_or("show");
    let mut rest = words.get(1..).unwrap_or_default();
    let (mut person, mut json_out, mut addr) = (String::new(), false, "");
    match verb {
        "help" => return page("notifications"),
        "--json" => {
            verb = "show";
            json_out = true;
        }
        "--person" => {
            verb = "show";
            person = rest.first().copied().unwrap_or("").to_string();
            rest = rest.get(1..).unwrap_or_default();
        }
        _ => {}
    }
    let mut f = Flags::new(rest);
    while let Some(w) = f.next() {
        match w {
            "--person" => {
                let v = f.words.get(f.i).copied().unwrap_or("");
                if v.is_empty() {
                    return usage("--person needs a login");
                }
                person = v.to_string();
                f.i += 1;
            }
            "--json" => json_out = true,
            w if w.starts_with('-') => return usage(&format!("Unknown flag: {w}")),
            w => {
                if !addr.is_empty() {
                    return usage("one address at a time");
                }
                addr = w;
            }
        }
    }
    let path = "/cli/notify/email/address";
    match verb {
        "show" => {
            if !addr.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email address [show] [--person <login>] [--json]"
                ));
            }
            let q = if person.is_empty() {
                String::new()
            } else {
                format!("person={}", quote(&person))
            };
            shown(cp::call_tcp(ctx, false, path, &q).map(|b| {
                if json_out {
                    Shown::out(json_text(&b))
                } else {
                    address_reader(&b)
                }
            }))
        }
        "set" => {
            if addr.is_empty() || !person.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email address set <you@example.com> (your own only)"
                ));
            }
            let body = dumps(vec![("address", p::text(addr))]);
            shown(cp::rt_post_ok(ctx, &format!("{path}/set"), &body, &["200", "202"]).map(|b| {
                if json_out {
                    return Shown::out(json_text(&b));
                }
                let mut o = Out::default();
                let r = (|| {
                    let v = p::load(&b, 3)?;
                    let a = p::or(p::dget(&v, "address", 3)?, Value::Object(vec![]));
                    let a = p::dget(&a, "address", 3)?;
                    o.line(&format!(
                        "Sent a confirm link to {}. It works for 24 hours; nothing else goes there until it is confirmed.",
                        py::str_of(&a)
                    ));
                    Ok(Exit::Ok)
                })();
                o.done(r)
            }))
        }
        "remove" | "rm" => {
            let mut b = Vec::new();
            if !addr.is_empty() {
                b.push(("address", p::text(addr)));
            }
            if !person.is_empty() {
                b.push(("person", p::text(&person)));
            }
            shown(cp::rt_post_ok(ctx, &format!("{path}/remove"), &dumps(b), &["200"]).map(|b| {
                if json_out {
                    return Shown::out(json_text(&b));
                }
                one_line(&b, |v| {
                    Ok(format!(
                        "Removed {} notice address(es).",
                        gs(v, "removed", 1)?
                    ))
                })
            }))
        }
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email address [show] | set <address> | remove [<address>] [--person <login>]"
        )),
    }
}

/// `address show`'s reader.
pub fn address_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let a = p::dget(&v, "address", 4)?;
        let shown = if p::truthy(&a) {
            plus(&p::dget(&a, "address", 5)?, " (confirmed)", 5)?
        } else {
            concat!(
                "none (",
                cli_name!(),
                " notifications email address set <you@example.com>)"
            )
            .to_string()
        };
        o.line(&format!("notice address: {shown}"));
        for pd in p::iter(&p::or(p::dget(&v, "pending", 6)?, p::empty_list()), 6)? {
            let at = p::or(p::dget(&pd, "expiresAt", 8)?, Value::Int("0".into()));
            let until = p::local_time(&at, "%Y-%m-%d %H:%M", 8)?;
            o.line(&format!(
                "  waiting for a confirm: {} (link sent; works until {until})",
                gs(&pd, "address", 9)?
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn suppressions(ctx: &mut Ctx, words: &[&str]) -> Exit {
    const USAGE: &str = concat!(
        "usage: ",
        cli_name!(),
        " notifications email suppressions [list] [<address>] [--limit N] [--json] | add <address> [--sender S] | rm <address> [--sender S|--all]"
    );
    let (mut verb, mut addr, mut sender, mut limit) = ("", "", "", "");
    let (mut all, mut json_out) = (false, false);
    let mut f = Flags::new(words);
    while let Some(w) = f.next() {
        let r = match w {
            "--sender" => f.value().map(|v| sender = v),
            "--all" => {
                all = true;
                Ok(())
            }
            "--limit" => f.value().map(|v| limit = v),
            "--json" => {
                json_out = true;
                Ok(())
            }
            w if w.starts_with('-') => Err(usage(&format!("Unknown flag: {w}"))),
            w => {
                if verb.is_empty() && addr.is_empty() {
                    if matches!(w, "list" | "ls" | "add" | "rm" | "remove") {
                        verb = w;
                        continue;
                    }
                    if !w.contains('@') {
                        return usage(USAGE);
                    }
                }
                if !addr.is_empty() {
                    return usage("one address at a time");
                }
                addr = w;
                Ok(())
            }
        };
        if let Err(e) = r {
            return e;
        }
    }
    let path = format!("{E}/suppressions");
    match if verb.is_empty() { "list" } else { verb } {
        "list" | "ls" => {
            let mut q = String::new();
            if !addr.is_empty() {
                q = format!("address={}", quote(addr));
            }
            if !limit.is_empty() {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str(&format!("limit={}", quote(limit)));
            }
            shown(cp::call_tcp(ctx, false, &path, &q).map(|b| {
                if json_out {
                    Shown::out(json_text(&b))
                } else {
                    suppressions_reader(&b, addr)
                }
            }))
        }
        verb => {
            if addr.is_empty() {
                return usage(&format!(
                    concat!(
                        "usage: ",
                        cli_name!(),
                        " notifications email suppressions {} <address> [--sender <sender>]"
                    ),
                    verb
                ));
            }
            let verb = if verb == "remove" { "rm" } else { verb };
            let mut b = vec![("address", p::text(addr))];
            if !sender.is_empty() {
                b.push(("sender", p::text(sender)));
            }
            if all {
                b.push(("all", Value::Bool(true)));
            }
            shown(
                cp::rt_post_ok(ctx, &format!("{path}/{verb}"), &dumps(b), &["200"]).map(|b| {
                    if json_out {
                        return Shown::out(json_text(&b));
                    }
                    one_line(&b, |v| {
                        if verb == "add" {
                            Ok(if p::truthy(&p::dget(v, "added", 1)?) {
                                format!("suppressed ({})", gs(v, "scope", 1)?)
                            } else {
                                "already suppressed".to_string()
                            })
                        } else {
                            Ok(format!("removed {} suppression(s)", gs(v, "removed", 1)?))
                        }
                    })
                }),
            )
        }
    }
}

/// `suppressions list`'s reader; `addr` is its `sys.argv[1]`.
pub fn suppressions_reader(body: &[u8], addr: &str) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rows = p::or(p::dget(&v, "suppressions", 3)?, p::empty_list());
        if !p::truthy(&rows) {
            o.line(if addr.is_empty() {
                "No suppressed addresses."
            } else {
                "No suppressed addresses matching that one."
            });
        }
        for x in p::iter(&rows, 6)? {
            let where_ = if p::dget(&x, "scope", 7)? == p::text("box") {
                "every sender".to_string()
            } else {
                let r = p::or(p::dget(&x, "scopeRef", 7)?, p::text("?"));
                p::concat("sender ", &r, 7)?
            };
            let at = p::or(p::dget(&x, "createdAt", 8)?, Value::Int("0".into()));
            let when = p::local_time(&at, "%Y-%m-%d", 8)?;
            let domain = p::dget(&x, "domain", 8)?;
            let reason = p::dget(&x, "reason", 8)?;
            let sha = head(&p::dget(&x, "addressSha256", 8)?, 12, 8)?;
            o.line(&format!(
                "{when}  @{} {} {} {sha}",
                p::left(&domain, 28),
                p::left(&reason, 12),
                super::ljust(&where_, 16)
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn agent(ctx: &mut Ctx, home: &Path, words: &[&str]) -> Exit {
    let verb = words.first().copied().unwrap_or("list");
    let rest = words.get(1..).unwrap_or_default();
    let (mut ws, mut sender, mut json_out) = ("", "", false);
    let mut f = Flags::new(rest);
    while let Some(w) = f.next() {
        let r = match w {
            "--sender" => f.value().map(|v| sender = v),
            "--json" => {
                json_out = true;
                Ok(())
            }
            w if w.starts_with('-') => Err(usage(&format!("Unknown flag: {w}"))),
            w => {
                if !ws.is_empty() {
                    return usage("one workspace at a time");
                }
                ws = w;
                Ok(())
            }
        };
        if let Err(e) = r {
            return e;
        }
    }
    match verb {
        "list" | "ls" | "--json" => shown(cp::call_tcp(ctx, false, &format!("{E}/agents"), "").map(|b| {
            if json_out || verb == "--json" {
                Shown::out(json_text(&b))
            } else {
                agents_reader(&b)
            }
        })),
        "on" => {
            if ws.is_empty() || sender.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email agent on <workspace> --sender <address>"
                ));
            }
            let body = dumps(vec![("workspace", p::text(ws)), ("sender", p::text(sender))]);
            shown(cp::email_post(ctx, home, &format!("{E}/agent/on"), &body).map(|b| {
                one_line(&b, |v| {
                    Ok(format!(
                        "email notices on for {} from {}",
                        gs(v, "workspace", 1)?,
                        gs(v, "sender", 1)?
                    ))
                })
            }))
        }
        "off" => {
            if ws.is_empty() {
                return usage(concat!(
                    "usage: ",
                    cli_name!(),
                    " notifications email agent off <workspace>"
                ));
            }
            let body = dumps(vec![("workspace", p::text(ws))]);
            shown(cp::email_post(ctx, home, &format!("{E}/agent/off"), &body).map(|b| {
                one_line(&b, |v| {
                    Ok(format!("email notices off for {}", gs(v, "workspace", 1)?))
                })
            }))
        }
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notifications email agent [list] | on <workspace> --sender <address> | off <workspace>"
        )),
    }
}

/// `agent list`'s reader.
pub fn agents_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rows = p::or(p::dget(&v, "agents", 3)?, p::empty_list());
        if !p::truthy(&rows) {
            o.line(concat!(
                "No workspace has email notices (",
                cli_name!(),
                " notifications email agent on <workspace> --sender <address>)."
            ));
        }
        for r in p::iter(&rows, 6)? {
            let ws = p::dget(&r, "workspace", 7)?;
            let on = if p::truthy(&p::dget(&r, "on", 7)?) {
                "on"
            } else {
                "off"
            };
            let sender = p::or(p::dget(&r, "sender", 7)?, p::text("(sender removed)"));
            let off = if p::truthy(&p::dget(&r, "offReason", 7)?) {
                p::concat("  (", &p::idx(&r, "offReason", 7)?, 7)? + ")"
            } else {
                String::new()
            };
            o.line(&format!(
                "{} {} {}{off}",
                p::left(&ws, 28),
                super::ljust(on, 3),
                py::str_of(&sender)
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

fn log(ctx: &mut Ctx, words: &[&str]) -> Exit {
    let (mut app, mut limit, mut json_out) = ("", "", false);
    let mut f = Flags::new(words);
    while let Some(w) = f.next() {
        let r = match w {
            "--app" => f.value().map(|v| app = v),
            "--limit" => f.value().map(|v| limit = v),
            "--json" => {
                json_out = true;
                Ok(())
            }
            w => Err(usage(&format!("Unknown flag: {w}"))),
        };
        if let Err(e) = r {
            return e;
        }
    }
    let mut q = String::new();
    if !app.is_empty() {
        q = format!("app={}", quote(app));
    }
    if !limit.is_empty() {
        if !q.is_empty() {
            q.push('&');
        }
        q.push_str(&format!("limit={}", quote(limit)));
    }
    shown(cp::call_tcp(ctx, false, &format!("{E}/log"), &q).map(|b| {
        if json_out {
            Shown::out(json_text(&b))
        } else {
            log_reader(&b)
        }
    }))
}

/// `_email_log_print`.
pub fn log_reader(body: &[u8]) -> Shown {
    let mut o = Out::default();
    let r = (|| {
        let v = p::load(body, 3)?;
        let rows = p::or(p::dget(&v, "items", 4)?, p::empty_list());
        if !p::truthy(&rows) {
            o.line("No email in the log (it keeps 30 days).");
            return Ok(Exit::Ok);
        }
        for r in p::iter(&rows, 7)? {
            let at = p::or(p::dget(&r, "createdAt", 8)?, Value::Int("0".into()));
            let at = p::local_time(&at, "%Y-%m-%d %H:%M", 8)?;
            let to = p::or(p::dget(&r, "to", 9)?, p::empty_list());
            let who = if p::truthy(&to) {
                let first = first_item(&to, 10)?;
                let n = super::py_len(&to, 10)?;
                let more = if n > 1 {
                    format!(" +{}", n - 1)
                } else {
                    String::new()
                };
                plus(&first, &more, 10)?
            } else {
                "-".to_string()
            };
            let extra = if p::truthy(&p::dget(&r, "error", 11)?) {
                p::concat(" ", &p::idx(&r, "error", 11)?, 11)?
            } else {
                String::new()
            };
            let mut what = p::dget(&r, "subject", 12)?;
            if !p::truthy(&what) {
                what = p::or(p::dget(&r, "purpose", 12)?, p::text(""));
            }
            let id = gs(&r, "id", 13)?;
            let status = p::dget(&r, "status", 13)?;
            o.line(&format!(
                "{at}  {id}  {}{extra}  {who}  {}",
                p::left(&status, 9),
                head(&what, 60, 13)?
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `to[0]`.
fn first_item(v: &Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => Ok(items[0].clone()),
        Value::Str(_) => Ok(p::text(&py::str_of(v).chars().take(1).collect::<String>())),
        Value::Object(_) => Err(Raise::at(line, "KeyError: 0")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `int(word)` as Python reads a decimal word: white space around it, a
/// sign, and `_` between digits. `None`: the `ValueError`.
pub fn py_int(word: &str) -> Option<String> {
    let t = word.trim_matches(super::py_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let d: String = digits.chars().filter(|c| *c != '_').collect();
    let d = d.trim_start_matches('0');
    let d = if d.is_empty() { "0" } else { d };
    Some(if neg && d != "0" {
        format!("-{d}")
    } else {
        d.to_string()
    })
}

fn settings(ctx: &mut Ctx, home: &Path, words: &[&str]) -> Exit {
    let (mut days, mut owner, mut set_owner) = ("", "", false);
    let mut f = Flags::new(words);
    while let Some(w) = f.next() {
        let r = match w {
            "--retention-days" => f.value().map(|v| days = v),
            "--owner-email" => f.value().map(|v| {
                owner = v;
                set_owner = true;
            }),
            w => Err(usage(&format!("Unknown flag: {w}"))),
        };
        if let Err(e) = r {
            return e;
        }
    }
    let mut b = Vec::new();
    if !days.is_empty() {
        match py_int(days) {
            Some(n) => b.push(("logRetentionDays", Value::Int(n))),
            None => {
                let t = super::traceback(
                    &[(6, "<module>")],
                    &format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        py::repr(&p::text(days))
                    ),
                );
                let _ = out::write_flushed(&mut std::io::stderr().lock(), t.as_bytes());
                return usage("--retention-days is a number");
            }
        }
    }
    if set_owner {
        b.push(("ownerEmail", p::text(owner)));
    }
    shown(
        cp::email_post(ctx, home, &format!("{E}/settings"), &dumps(b)).map(|b| {
            one_line(&b, |v| {
                let s = p::or(p::dget(v, "settings", 1)?, Value::Object(vec![]));
                let owner = p::or(p::dget(&s, "ownerEmail", 1)?, p::text("not set"));
                Ok(format!(
                    "log kept {} days; owner address {}",
                    gs(&s, "logRetentionDays", 1)?,
                    py::str_of(&owner)
                ))
            })
        }),
    )
}

#[cfg(test)]
mod tests;
