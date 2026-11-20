//! `_vault_handoff` (`cli/src/vault.sh`): send, incoming, sent,
//! accept-from, send-allow, ask, asks and unsend. Inside a K2 terminal
//! a call carries the passport; outside, it goes to the vault socket when
//! there is one, else with the owner token (`_vault_handoff_call`).
//!
//! `send --stdin` (a value a person types in) and `send-allow add` from a
//! person's terminal go to the vault socket only, after the person guard;
//! the value goes only in the request body. Inside a K2 terminal both
//! are refused, as the shell refuses them.

use super::py::{self, get, idx, items, left, or_list, or_obj, render, s, t, Lines, R};
use super::Ctx;
use crate::confirm::{self, dumps, Split};
use crate::exit::Exit;
use crate::out;
use crate::out::json::{Text, Value};
use crate::prompt;
use crate::transport::encode::quote;
use k2_client::brand::CLI_NAME as CLI;

/// Whether the binary answers `vault <sub> <rest>`: every form of these
/// verbs now.
pub fn native(_sub: &str, _rest: &[&str]) -> bool {
    true
}

/// The flags that take the next word.
const VALUE_FLAGS: &[&str] = &[
    "--to",
    "--as",
    "--note",
    "--ttl",
    "--reply",
    "--scope",
    "--workspace",
    "--via",
    "--kind",
    "--for",
    "--delivery",
];

fn usage(line: String) -> Exit {
    out::stderr_line(&line);
    Exit::Usage
}

/// The parsed words.
#[derive(Default)]
struct H {
    to: String,
    as_n: String,
    note: String,
    ttl: String,
    reply: String,
    scope: String,
    workspace: String,
    via: String,
    kind: String,
    hosts: String,
    delivery: String,
    json: bool,
    stdin: bool,
    replace: bool,
    always: bool,
    pos: Vec<String>,
    seen: Vec<String>,
}

fn parse(sub: &str, rest: &[&str]) -> Result<H, Exit> {
    let mut h = H::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--json" | "--stdin" | "--replace" | "--always" => {
                match w {
                    "--json" => h.json = true,
                    "--stdin" => h.stdin = true,
                    "--replace" => h.replace = true,
                    _ => h.always = true,
                }
                h.seen.push(w.into());
            }
            _ if VALUE_FLAGS.contains(&w) => {
                let Some(v) = rest.get(i + 1) else {
                    return Err(usage(format!("usage: {w} needs a value")));
                };
                let v = v.to_string();
                match w {
                    "--to" => h.to = v,
                    "--as" => h.as_n = v,
                    "--note" => h.note = v,
                    "--ttl" => h.ttl = v,
                    "--reply" => h.reply = v,
                    "--scope" => h.scope = v,
                    "--workspace" => h.workspace = v,
                    "--via" => h.via = v,
                    "--kind" => h.kind = v,
                    "--for" => h.hosts = v,
                    _ => h.delivery = v,
                }
                h.seen.push(w.into());
                i += 2;
                continue;
            }
            _ if w == "--value" || w.starts_with("--value=") || w.starts_with("--secret-value") => {
                return Err(usage(
                    "value_on_argv: values are read from stdin, never an argument".into(),
                ));
            }
            "--system" => {
                return Err(usage(format!(
                    "usage: only {CLI} vault list takes --system"
                )))
            }
            _ if w.starts_with("--") => {
                return Err(usage(format!(
                    "usage: unknown flag '{w}' ({CLI} vault send --help)"
                )))
            }
            _ => h.pos.push(w.into()),
        }
        i += 1;
    }
    let p0 = h.pos.first().map(String::as_str);
    let (takes, words) = match sub {
        "send" => ("--to --as --note --ttl --reply --scope --workspace --via --kind --for --delivery --stdin --json", sub.to_string()),
        "incoming" => match p0 {
            Some("accept") => ("--as --replace --always --json", "incoming accept".into()),
            Some("decline") => ("--json", "incoming decline".into()),
            _ => ("--workspace --json", sub.into()),
        },
        "sent" | "asks" | "accept-from" => ("--workspace --json", sub.into()),
        "ask" => ("--note --workspace --json", sub.into()),
        "unsend" => ("--json", sub.into()),
        _ => match p0.unwrap_or("list") {
            w @ ("add" | "remove") => ("--to --workspace --json", format!("send-allow {w}")),
            _ => ("--workspace --json", sub.into()),
        },
    };
    for f in &h.seen {
        if !format!(" {takes} ").contains(&format!(" {f} ")) {
            let shown = takes.replacen(" --via", "", 1);
            return Err(usage(format!(
                "usage: {CLI} vault {words} doesn't take {f} (it takes: {}; {CLI} vault send --help)",
                shown.replace(' ', ", ")
            )));
        }
    }
    if sub == "send" {
        if !h.stdin && !h.hosts.is_empty() {
            return Err(usage(format!("usage: --for goes with --stdin (a named send keeps the secret's own sites; {CLI} vault hosts NAME to change them)")));
        }
        if h.stdin {
            if !h.scope.is_empty() {
                return Err(usage(
                    "usage: --scope picks a secret in the vault; --stdin sends a value you type in"
                        .into(),
                ));
            }
            if !h.workspace.is_empty() {
                return Err(usage("usage: --workspace picks a secret in the vault; --stdin sends a value you type in".into()));
            }
        }
        if confirm::in_k2_terminal() {
            if !h.scope.is_empty() {
                return Err(usage("usage: --scope is for people; an agent sends its own workspace's secret (drop --scope)".into()));
            }
            if !h.workspace.is_empty() {
                return Err(usage("usage: --workspace is for people; an agent sends its own workspace's secret (drop --workspace)".into()));
            }
        }
    }
    Ok(h)
}

/// The verb. Called only for a form [`native`] takes.
pub fn run(ctx: &mut Ctx, sub: &str, rest: &[&str]) -> Exit {
    match verb(ctx, sub, rest) {
        Ok(e) | Err(e) => e,
    }
}

/// `_vault_handoff_call METHOD <route> [payload]`.
fn call(ctx: &mut Ctx, post: bool, route: &str, payload: &str) -> Result<Split, Exit> {
    if !confirm::in_k2_terminal() && crate::families::live_socket(Some(&ctx.sock)) {
        if !post && !payload.is_empty() {
            return Ok(ctx.sock(false, &format!("{route}?{payload}"), b""));
        }
        return Ok(ctx.sock(post, route, payload.as_bytes()));
    }
    ctx.send(post, &format!("/cli/vault/{route}"), payload)
}

/// `_vault_handoff_code`.
fn code(body: &[u8]) -> String {
    let Ok(Value::Object(v)) = confirm::load(body) else {
        return String::new();
    };
    let v = Value::Object(v);
    let c = match get(&v, "error") {
        Ok(Value::Object(e)) => get(&Value::Object(e), "code").unwrap_or(Value::Null),
        _ => get(&v, "code").unwrap_or(Value::Null),
    };
    if t(&c) {
        s(&c)
    } else {
        String::new()
    }
}

/// `_vault_handoff_exit <status> <code>`.
pub fn exit_for(status: &str, code: &str) -> Exit {
    const THREE: &[&str] = &[
        "pending",
        "held",
        "declined",
        "not_connected",
        "send_not_allowed",
        "handoff_person_only",
        "confirm_required",
        "owner_only",
        "too_many_pending",
        "rate_limited",
        "vault_not_yours",
        "vault_owner_confirm_required",
        "not_yours",
    ];
    const TWO: &[&str] = &[
        "peer_too_old",
        "federation_off",
        "never_sendable",
        "send_by_name_only",
        "use_vault_set",
        "note_has_value",
        "note_too_long",
        "value_too_large",
        "same_workspace",
        "name_taken",
        "not_pending",
        "not_unsendable",
        "no_wildcard",
        "bad_sender",
        "bad_ttl",
        "bad_name",
        "bad_address",
        "to_required",
        "name_required",
        "bad_json",
        "bad_action",
        "workspace_required",
        "secret_not_active",
        "secret_expired",
    ];
    if THREE.contains(&code) {
        return Exit::Refused;
    }
    if TWO.contains(&code) {
        return Exit::Usage;
    }
    match status {
        "200" => Exit::Ok,
        "400" | "413" | "422" => Exit::Usage,
        "401" | "403" | "429" => Exit::Refused,
        "404" => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `_vault_handoff_fail <status> <body>`.
fn fail(ctx: &mut Ctx, r: &Split) -> Exit {
    let c = code(&r.body);
    let raw = String::from_utf8_lossy(&r.body);
    let head = || {
        let h: String = raw.chars().take(300).collect();
        if h.is_empty() {
            "the daemon did not answer".to_string()
        } else {
            h
        }
    };
    let line = match confirm::load(&r.body) {
        Ok(Value::Object(p)) => {
            let v = Value::Object(p);
            match get(&v, "error").unwrap_or(Value::Null) {
                Value::Object(e) => {
                    let e = Value::Object(e);
                    format!(
                        "{}: {}",
                        s(&py::get_or(&e, "code", py::text("error")).unwrap_or(Value::Null)),
                        s(&py::get_or(&e, "hint", py::text("")).unwrap_or(Value::Null))
                    )
                }
                e if t(&e) => s(&e),
                _ => raw.chars().take(300).collect(),
            }
        }
        _ => head(),
    };
    out::stderr_line(&line);
    let passport = ctx.passport();
    ctx.closing.answer(&r.response(), passport);
    exit_for(&r.status, &c)
}

fn print_body(r: &Split) -> Exit {
    let mut b = r.body.clone();
    b.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &b) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

fn kv(k: &str, v: &str) -> (Text, Value) {
    (Text::from(k), py::text(v))
}

fn verb(ctx: &mut Ctx, sub: &str, rest: &[&str]) -> Result<Exit, Exit> {
    let h = parse(sub, rest)?;
    let q = if h.workspace.is_empty() {
        String::new()
    } else {
        format!("workspace={}", quote(&h.workspace))
    };
    let p = |i: usize| h.pos.get(i).cloned().unwrap_or_default();
    let ok_or_fail = |ctx: &mut Ctx, r: Split| if r.ok() { Ok(r) } else { Err(fail(ctx, &r)) };
    match sub {
        "send" => send(ctx, &h),
        "incoming" | "sent"
            if sub == "incoming" && matches!(p(0).as_str(), "accept" | "decline") =>
        {
            let verb = p(0);
            let id = p(1);
            if id.is_empty() {
                return Err(usage(format!(
                    "usage: {CLI} vault incoming {verb} <delivery-id>"
                )));
            }
            if confirm::in_k2_terminal() {
                out::stderr_line(&format!("owner_only: a person accepts or declines {id} on the card {} put on your Thread (Allow accepts, Deny declines; it shows on their phone too). Nothing for you to run.", k2_client::brand::PRODUCT_NAME));
                out::stderr_line(&format!("No card there? {CLI} vault incoming puts it up, or ask: {CLI} permission request vault.incoming.accept id={id} --why \"<what you need it for>\""));
                return Err(Exit::Refused);
            }
            ctx.person_guard()?;
            let mut o = vec![kv("id", &id), kv("action", &verb)];
            if !h.as_n.is_empty() {
                o.push(kv("as", &h.as_n));
            }
            if h.replace {
                o.push((Text::from("replace"), Value::Bool(true)));
            }
            if h.always {
                o.push((Text::from("always"), Value::Bool(true)));
            }
            let r = ctx.sock(true, "incoming/decide", dumps(&Value::Object(o)).as_bytes());
            let r = ok_or_fail(ctx, r)?;
            if h.json {
                return Ok(print_body(&r));
            }
            Ok(render(&r.body, decided).print())
        }
        "incoming" | "sent" => {
            if !p(0).is_empty() {
                return Err(usage(format!("usage: {CLI} vault {sub} [--workspace <ws>] [--json]  (accept/decline: {CLI} vault incoming accept <id>)")));
            }
            let r = call(ctx, false, sub, &q)?;
            let r = ok_or_fail(ctx, r)?;
            if h.json {
                return Ok(print_body(&r));
            }
            let incoming = sub == "incoming";
            Ok(render(&r.body, |v, l| deliveries(v, l, incoming)).print())
        }
        "ask" => {
            let (to, name) = (p(0), p(1));
            if to.is_empty() || name.is_empty() {
                return Err(usage(format!(
                    "usage: {CLI} vault ask <agent>|<agent>::<host> NAME [--note …]"
                )));
            }
            let mut o = vec![kv("to", &to), kv("name", &name)];
            if !h.note.is_empty() {
                o.push(kv("note", &h.note));
            }
            if !h.workspace.is_empty() {
                o.push(kv("workspace", &h.workspace));
            }
            let r = call(ctx, true, "ask", &dumps(&Value::Object(o)))?;
            let r = ok_or_fail(ctx, r)?;
            if h.json {
                return Ok(print_body(&r));
            }
            Ok(render(&r.body, |v, l| {
                l.p(format!(
                    "asked {} for {} ({}){}; open {} days",
                    s(&get(v, "to")?),
                    s(&get(v, "name")?),
                    s(&get(v, "askId")?),
                    if t(&get(v, "delivered")?) {
                        ""
                    } else {
                        " (they get it when they wake)"
                    },
                    s(&get(v, "expiresInDays")?)
                ));
                Ok(0)
            })
            .print())
        }
        "asks" => {
            let r = call(ctx, false, "asks", &q)?;
            let r = ok_or_fail(ctx, r)?;
            if h.json {
                return Ok(print_body(&r));
            }
            Ok(render(&r.body, |v, l| {
                let its = items(&or_list(get(v, "items")?))?;
                if its.is_empty() {
                    l.p("no asks");
                }
                for a in its {
                    let arrow = if py::is(&get(&a, "direction")?, "out") {
                        "asked"
                    } else {
                        "asked by"
                    };
                    l.p(format!(
                        "{} {} {arrow} {} {}",
                        left(&get(&a, "name")?, 22),
                        left(&get(&a, "status")?, 9),
                        left(&get(&a, "peer")?, 28),
                        s(&get(&a, "askId")?)
                    ));
                }
                Ok(0)
            })
            .print())
        }
        "unsend" => {
            let id = p(0);
            if id.is_empty() {
                return Err(usage(format!(
                    "usage: {CLI} vault unsend <delivery-id> ({CLI} vault sent)"
                )));
            }
            let r = call(
                ctx,
                true,
                "unsend",
                &dumps(&Value::Object(vec![kv("id", &id)])),
            )?;
            let r = ok_or_fail(ctx, r)?;
            if h.json {
                return Ok(print_body(&r));
            }
            Ok(render(&r.body, |v, l| {
                let d = s(&get(v, "deliveryId")?);
                l.p(if t(&get(v, "disabled")?) {
                    format!("took back {d}: their copy is disabled")
                } else if t(&get(v, "keptReplaced")?) {
                    format!("took back {d}: their owner had replaced it, so it was left alone")
                } else {
                    format!("took back {d}")
                });
                l.e(s(&get(v, "honest")?));
                Ok(0)
            })
            .print())
        }
        "accept-from" => {
            let verb = if h.pos.is_empty() {
                "list".to_string()
            } else {
                p(0)
            };
            let r = match verb.as_str() {
                "list" => {
                    let r = call(ctx, false, "accept-from", &q)?;
                    ok_or_fail(ctx, r)?
                }
                "add" | "remove" => {
                    let sender = p(1);
                    if sender.is_empty() || h.workspace.is_empty() {
                        return Err(usage(format!(
                            "usage: {CLI} vault accept-from {verb} <sender> --workspace <ws>"
                        )));
                    }
                    ctx.person_guard()?;
                    let b = vec![
                        kv("workspace", &h.workspace),
                        kv("sender", &sender),
                        kv("action", &verb),
                    ];
                    let r = ctx.sock(true, "accept-from", dumps(&Value::Object(b)).as_bytes());
                    ok_or_fail(ctx, r)?
                }
                _ => {
                    return Err(usage(format!(
                        "usage: {CLI} vault accept-from list|add|remove …"
                    )))
                }
            };
            if h.json {
                return Ok(print_body(&r));
            }
            Ok(render(&r.body, |v, l| {
                let e = items(&or_list(get(v, "entries")?))?;
                if e.is_empty() {
                    l.p("accepts from no one yet (every secret waits for a person)");
                }
                for a in e {
                    let lab = get(&a, "senderLabel")?;
                    let who = if t(&lab) {
                        let x = py::text(&format!(
                            "{} ({})",
                            s(&idx(&a, "sender")?),
                            s(&idx(&a, "senderLabel")?)
                        ));
                        if t(&x) {
                            x
                        } else {
                            get(&a, "sender")?
                        }
                    } else {
                        get(&a, "sender")?
                    };
                    l.p(format!(
                        "{} added by {} at {}",
                        left(&who, 40),
                        s(&get(&a, "addedBy")?),
                        s(&get(&a, "addedAt")?)
                    ));
                }
                Ok(0)
            })
            .print())
        }
        _ => {
            // send-allow
            let verb = if h.pos.is_empty() {
                "list".to_string()
            } else {
                p(0)
            };
            let r = match verb.as_str() {
                "list" => {
                    let r = call(ctx, false, "send-allow", &q)?;
                    ok_or_fail(ctx, r)?
                }
                "add" | "remove" => {
                    let sname = p(1);
                    if sname.is_empty() || h.to.is_empty() || h.workspace.is_empty() {
                        return Err(usage(format!("usage: {CLI} vault send-allow {verb} NAME --to <agent>::<host> --workspace <ws>")));
                    }
                    if confirm::in_k2_terminal() {
                        out::stderr_line(&format!("handoff_person_only: an Owner or Admin decides which secrets you may send to another server. To ask, run {CLI} vault send {sname} --to {} and run the Ask with: line it prints.", h.to));
                        return Err(Exit::Refused);
                    }
                    ctx.person_guard()?;
                    let mut b = vec![
                        kv("workspace", &h.workspace),
                        kv("name", &sname),
                        kv("to", &h.to),
                        kv("action", &verb),
                    ];
                    let mut sbody = dumps(&Value::Object(b.clone()));
                    if verb == "add" {
                        // The same vault confirm as sending it
                        // (handoff-send, bound to the secret).
                        let r = ctx.sock(false, &format!("send-allow?{q}"), b"");
                        let r = ok_or_fail(ctx, r)?;
                        let ssid = secret_id(&r.body, &sname)?;
                        if ssid.is_empty() {
                            out::stderr_line(&format!("not_found: {} has no secret a person gave named {sname} ({CLI} vault list --scope ws:{})", h.workspace, h.workspace));
                            return Err(Exit::NotFound);
                        }
                        let label = format!("let {} send {sname} to {}", h.workspace, h.to);
                        let Some(tok) = confirm::confirm_token(
                            &mut ctx.confirm(),
                            "handoff-send",
                            &ssid,
                            &label,
                        ) else {
                            return Err(Exit::Refused);
                        };
                        b.push(kv("secretId", &ssid));
                        sbody = confirm::with_confirm_token(&dumps(&Value::Object(b)), &tok, None)
                            .unwrap_or_default();
                    }
                    let r = ctx.sock(true, "send-allow", sbody.as_bytes());
                    ok_or_fail(ctx, r)?
                }
                _ => {
                    return Err(usage(format!(
                        "usage: {CLI} vault send-allow list|add|remove …"
                    )))
                }
            };
            if h.json {
                return Ok(print_body(&r));
            }
            Ok(render(&r.body, |v, l| {
                let e = items(&or_list(get(v, "entries")?))?;
                let ws = get(v, "workspace")?;
                let ws = if t(&ws) { s(&ws) } else { "this agent".into() };
                if e.is_empty() {
                    l.p(format!(
                        "{ws} may send no person-given secret to another server without asking yet"
                    ));
                }
                for a in e {
                    l.p(format!(
                        "may send {} to {} added by {} at {}",
                        left(&get(&a, "name")?, 28),
                        left(&get(&a, "to")?, 36),
                        s(&get(&a, "addedBy")?),
                        s(&get(&a, "addedAt")?)
                    ));
                }
                Ok(0)
            })
            .print())
        }
    }
}

/// `ssid="$(… next((s["id"] for s in json.load(sys.stdin).get("secrets") or
/// [] if s.get("name") == n), ""))"`: a raise prints its traceback, and
/// `set -e` ends the command with exit 1.
fn secret_id(body: &[u8], n: &str) -> Result<String, Exit> {
    let found = confirm::load(body).and_then(|v| {
        for x in items(&or_list(get(&v, "secrets")?))? {
            if py::is(&get(&x, "name")?, n) {
                return Ok(s(&idx(&x, "id")?));
            }
        }
        Ok(String::new())
    });
    match found {
        Ok(id) => Ok(id.trim_end_matches('\n').to_string()),
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            Err(Exit::Error)
        }
    }
}

/// `send NAME --to …`, and `--stdin`: a value a person types in.
fn send(ctx: &mut Ctx, h: &H) -> Result<Exit, Exit> {
    let name = h.pos.first().cloned().unwrap_or_default();
    if name.is_empty() || h.to.is_empty() {
        return Err(usage(format!(
            "usage: {CLI} vault send NAME --to <agent>|<agent>::<host> [--note …]"
        )));
    }
    if h.pos.len() != 1 {
        return Err(usage(
            "value_on_argv: name the secret only; values are read from stdin".into(),
        ));
    }
    let mut o = vec![kv("name", &name), kv("to", &h.to)];
    if !h.as_n.is_empty() {
        o.push(kv("as", &h.as_n));
    }
    if !h.note.is_empty() {
        o.push(kv("note", &h.note));
    }
    if !h.ttl.is_empty() {
        let Some(secs) = ttl_secs(&h.ttl) else {
            return Err(usage("usage: --ttl 7d | 12h | 3600".into()));
        };
        o.push((Text::from("ttlSecs"), Value::Int(secs)));
    }
    if !h.reply.is_empty() {
        o.push(kv("replyTo", &h.reply));
    }
    let set = |o: &mut Vec<(Text, Value)>, k: &str, v: &str| match o
        .iter_mut()
        .find(|(ok, _)| ok.as_str() == Some(k))
    {
        Some((_, ov)) => *ov = py::text(v),
        None => o.push(kv(k, v)),
    };
    let sc = &h.scope;
    if let Some(ws) = sc
        .strip_prefix("ws:")
        .or_else(|| sc.strip_prefix("workspace:"))
    {
        set(&mut o, "scope", "workspace");
        set(&mut o, "workspace", ws);
    } else if !sc.is_empty() {
        set(&mut o, "scope", sc);
    }
    if !h.workspace.is_empty() {
        let cur_ws = o
            .iter()
            .find(|(k, _)| k.as_str() == Some("workspace"))
            .map(|(_, v)| s(v));
        let cur_scope = o
            .iter()
            .find(|(k, _)| k.as_str() == Some("scope"))
            .map(|(_, v)| s(v));
        if cur_ws.as_deref().unwrap_or(&h.workspace) != h.workspace
            || (!sc.is_empty() && cur_scope.as_deref() != Some("workspace"))
        {
            return Err(usage(format!(
                "usage: --workspace {} and --scope {} name different secrets; give one",
                h.workspace, sc
            )));
        }
        set(&mut o, "workspace", &h.workspace);
    }
    if !h.via.is_empty() {
        o.push(kv("via", &h.via));
    }
    if !h.kind.is_empty() {
        o.push(kv("kind", &h.kind));
    }
    if !h.hosts.is_empty() {
        let hs = h
            .hosts
            .split(',')
            .map(|x| x.trim_matches(crate::families::py_space))
            .filter(|x| !x.is_empty())
            .map(py::text)
            .collect();
        o.push((Text::from("hosts"), Value::Array(hs)));
    }
    if !h.delivery.is_empty() {
        o.push(kv("delivery", &h.delivery));
    }
    let (body, mut r) = if h.stdin {
        // A value a person types in: the person channel only (H15).
        ctx.person_guard()?;
        let b = super::verbs::body_with_value(o)?;
        let r = ctx.sock(true, "send", b.expose());
        (b, r)
    } else {
        let b = prompt::Secret::new(dumps(&Value::Object(o)).into_bytes());
        let r = call(
            ctx,
            true,
            "send",
            std::str::from_utf8(b.expose()).unwrap_or(""),
        )?;
        (b, r)
    };
    let body = std::str::from_utf8(body.expose()).unwrap_or("").to_string();
    if r.status == "403"
        && code(&r.body) == "confirm_required"
        && !confirm::in_k2_terminal()
        && crate::families::live_socket(Some(&ctx.sock))
    {
        let sid = match confirm::load(&r.body).and_then(|v| get(&v, "secretId")) {
            Ok(x) if t(&x) => s(&x),
            Ok(_) => String::new(),
            Err(e) => {
                out::stderr_line(e.text().trim_end_matches('\n'));
                String::new()
            }
        };
        let label = format!("send {name} to {}", h.to);
        let Some(tok) = confirm::confirm_token(&mut ctx.confirm(), "handoff-send", &sid, &label)
        else {
            return Err(Exit::Refused);
        };
        let Some(b) = confirm::with_confirm_token(&body, &tok, None) else {
            return Err(Exit::Error);
        };
        r = ctx.sock(true, "send", b.as_bytes());
    }
    if !r.ok() {
        return Err(fail(ctx, &r));
    }
    // Under `set -e` a printer that fails ends the command, and so does a
    // `st="$(…)"` whose Python raises.
    let e = if h.json {
        print_body(&r)
    } else {
        render(&r.body, sent_printer).print()
    };
    if e != Exit::Ok {
        return Ok(e);
    }
    let st = match confirm::load(&r.body).and_then(|v| get(&v, "status")) {
        Ok(x) if t(&x) => s(&x),
        Ok(_) => String::new(),
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            return Ok(Exit::Error);
        }
    };
    Ok(if st == "landed" {
        Exit::Ok
    } else {
        Exit::Refused
    })
}

/// `re.fullmatch(r"(\d+)([smhdw]?)", ttl.strip())` times its unit.
fn ttl_secs(ttl: &str) -> Option<String> {
    let t = ttl.trim_matches(crate::families::py_space);
    let unit = t.chars().last()?;
    let (digits, mult): (&str, u128) = match unit {
        's' => (&t[..t.len() - 1], 1),
        'm' => (&t[..t.len() - 1], 60),
        'h' => (&t[..t.len() - 1], 3600),
        'd' => (&t[..t.len() - 1], 86400),
        'w' => (&t[..t.len() - 1], 604800),
        _ => (t, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u128 = digits.trim_start_matches('0').parse().unwrap_or(0);
    Some(n.checked_mul(mult)?.to_string())
}

fn sent_printer(v: &Value, l: &mut Lines) -> R<i32> {
    let st = get(v, "status")?;
    let mut line = format!(
        "sent {} to {} ({}): {}",
        s(&get(v, "name")?),
        s(&get(v, "to")?),
        s(&get(v, "deliveryId")?),
        s(&st)
    );
    if py::is(&st, "landed") && t(&get(v, "ref")?) {
        line += &py::cat(" → ", &idx(v, "ref")?)?;
    }
    let ln = get(v, "landedName")?;
    if t(&ln) && ln != get(v, "name")? {
        line += &py::cat(" as ", &idx(v, "landedName")?)?;
    }
    l.p(line);
    let la = or_obj(get(v, "landsAs")?);
    if t(&get(&la, "delivery")?) {
        l.p(format!(
            "they get it as {} (at most {})",
            s(&idx(&la, "delivery")?),
            s(&get(&la, "ceiling")?)
        ));
    }
    if t(&get(v, "who")?) {
        l.e(s(&idx(v, "who")?));
    }
    if t(&get(v, "addressNote")?) {
        l.e(py::cat("note: ", &idx(v, "addressNote")?)?);
    }
    Ok(0)
}

fn decided(v: &Value, l: &mut Lines) -> R<i32> {
    let d = or_obj(get(v, "delivery")?);
    let ln = get(&d, "landedName")?;
    let name = if t(&ln) { ln } else { get(&d, "name")? };
    let r = if t(&get(v, "ref")?) {
        py::cat(" → ", &idx(v, "ref")?)?
    } else {
        String::new()
    };
    l.p(format!(
        "{} {} from {} ({}){r}",
        s(&get(&d, "status")?),
        s(&name),
        s(&get(&d, "peer")?),
        s(&get(&d, "deliveryId")?)
    ));
    let m = or_obj(get(v, "secret")?);
    if t(&m) {
        let until = if t(&get(&m, "expiresAt")?) {
            py::cat(", until ", &idx(&m, "expiresAt")?)?
        } else {
            String::new()
        };
        l.p(format!(
            "delivered to the agent as {} (at most {}){until}",
            s(&get(&m, "delivery")?),
            s(&get(&m, "ceiling")?)
        ));
        l.p(format!("it uses it with: {}", s(&get(&m, "use")?)));
    }
    if t(&get(v, "more")?) {
        l.p(s(&idx(v, "more")?));
    }
    if t(&get(v, "alwaysAdded")?) {
        l.p(py::cat("always accepting from ", &idx(v, "alwaysAdded")?)? + " now");
    }
    Ok(0)
}

fn deliveries(v: &Value, l: &mut Lines, incoming: bool) -> R<i32> {
    let its = items(&or_list(get(v, "items")?))?;
    if its.is_empty() {
        l.p(format!(
            "nothing {} yet",
            if incoming { "received" } else { "sent" }
        ));
    }
    for d in its {
        let peer = get(&d, "peer")?;
        let who = format!(
            "{}{}",
            if incoming { "from " } else { "to " },
            if t(&peer) { s(&peer) } else { "?".into() }
        );
        let mut extra = String::new();
        if py::is(&get(&d, "status")?, "pending") && incoming {
            let c = or_obj(get(&d, "card")?);
            if t(&get(&c, "card")?) {
                let a = get(&c, "addr")?;
                let a = if t(&a) { a } else { get(&d, "workspace")? };
                let a = if t(&a) { s(&a) } else { "the agent".into() };
                extra = format!("  (waiting for a person: the card on the Thread of {a})");
            } else {
                let ask = get(&d, "ask")?;
                let ask = if t(&ask) {
                    s(&ask)
                } else {
                    format!(
                        "{CLI} permission request vault.incoming.accept id={}",
                        s(&get(&d, "deliveryId")?)
                    )
                };
                extra = format!("  (no card yet: {ask})");
            }
        }
        if t(&get(&d, "ref")?) {
            extra = py::cat("  ", &idx(&d, "ref")?)?;
        }
        let ln = get(&d, "landedName")?;
        let name = if t(&ln) { ln } else { get(&d, "name")? };
        let ca = get(&d, "createdAt")?;
        let ca = if t(&ca) { ca } else { py::text("") };
        l.p(format!(
            "{} {} {} {} {}{extra}",
            left(&name, 22),
            left(&get(&d, "status")?, 10),
            left(&py::text(&who), 28),
            left(&ca, 24),
            s(&get(&d, "deliveryId")?)
        ));
        if t(&get(&d, "note")?) {
            l.p(format!("    note: {}", s(&idx(&d, "note")?)));
        }
        if incoming {
            let m = get(&d, "secret")?;
            if t(&m) {
                let until = if t(&get(&m, "expiresAt")?) {
                    py::cat(" until ", &idx(&m, "expiresAt")?)?
                } else {
                    String::new()
                };
                l.p(format!(
                    "    yours as {}{until}; use: {}",
                    s(&get(&m, "delivery")?),
                    s(&get(&m, "use")?)
                ));
            } else if py::is(&get(&d, "status")?, "pending") && t(&get(&d, "landsAs")?) {
                let la = idx(&d, "landsAs")?;
                let kept = if t(&get(&la, "expiresAt")?) {
                    py::cat(", kept until ", &idx(&la, "expiresAt")?)?
                } else {
                    String::new()
                };
                l.p(format!(
                    "    lands as {} (at most {}){kept}",
                    s(&get(&la, "delivery")?),
                    s(&get(&la, "ceiling")?)
                ));
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ttl_as_python_reads_it() {
        assert_eq!(ttl_secs("7d").as_deref(), Some("604800"));
        assert_eq!(ttl_secs(" 12h ").as_deref(), Some("43200"));
        assert_eq!(ttl_secs("3600").as_deref(), Some("3600"));
        assert_eq!(ttl_secs("007s").as_deref(), Some("7"));
        for bad in ["", "h", "1.5h", "1y", "-1"] {
            assert_eq!(ttl_secs(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_exit_for_a_code_first() {
        assert_eq!(exit_for("200", "pending"), Exit::Refused);
        assert_eq!(exit_for("500", "bad_ttl"), Exit::Usage);
        assert_eq!(exit_for("429", ""), Exit::Refused);
        assert_eq!(exit_for("404", ""), Exit::NotFound);
        assert_eq!(exit_for("000", ""), Exit::Error);
    }
}
