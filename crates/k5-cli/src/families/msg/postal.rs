//! `k5 msg <handle::label.postal.bot> "text" [--no-wake] [--json]`: Postal
//! mail, as the shell's `_postal_send_teach` (`cli/src/remote-target.sh`)
//! sends it and `_postal_print send` (`cli/src/postal.sh`) prints it.
//!
//! The address is the first word that doesn't start with `-`; the other
//! words, in order: `--no-wake`, `--wake`, `--json`; a flag only messages
//! to agents on this server take is [`k2_only`], exit 2; any other flag is
//! [`unknown_flag`], exit 2; the rest is the text, joined by spaces. No
//! text is [`usage`], exit 2. Then `cli_send cli_post_json
//! /cli/postal/send` on the left of a pipe into the printer: in a session
//! with a passport and a live cell socket the socket first, then TCP by
//! its rules; else TCP. In a session the credential is the passport (`msg`
//! is an id tool, and `_postal_session_token` keeps it so). The command
//! ends with the printer's code, or curl's when the printer's is 0; no
//! answer, or a POST the socket may have run, ends it by SIGPIPE after its
//! line. In a session an `owner_only` or `postal_people_only` refusal
//! whose hint doesn't already say where ends with
//! [`crate::families::postal::PERSON_LINE`].

use super::super::{end_in_pipe, get, pipefail, raised, Raise, Shown};
use super::py::{self, concat, dumps_indent, eq, get_or_null, index, s, Printed};
use crate::exit::Exit;
use crate::families::postal::{py_in, PERSON_LINE};
use crate::out::json::{Text, Value};
use crate::out::py::{str_of, truthy, type_name};
use crate::out::{self};
use crate::prelude::Closing;
use crate::transport::{Call, Client, Error};
use k2_client::cli_name;

/// The route.
pub const ENDPOINT: &str = "/cli/postal/send";

/// The flags of a message to an agent on this server.
pub const K2_ONLY: &[&str] = &[
    "--summary",
    "--wake-timeout",
    "--session",
    "--inbox",
    "--inbox-wake",
    "--inbox-silent",
    "--signal",
    "--command",
    "--from",
    "--via",
];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub text: String,
    pub wake: bool,
    pub json: bool,
}

/// A flag of [`K2_ONLY`].
pub fn k2_only(flag: &str, to: &str) -> String {
    format!(concat!(cli_name!(), " msg: {} is for messages to agents on this server; Postal mail to {} takes only the text, --no-wake and --json."), flag, to)
}

/// Any other flag.
pub fn unknown_flag(flag: &str) -> String {
    format!(
        concat!(
            cli_name!(),
            " msg: unknown flag '{}' for Postal mail (",
            cli_name!(),
            " msg --help)"
        ),
        flag
    )
}

/// No text.
pub fn usage(to: &str) -> String {
    format!(
        concat!(
            "Usage: ",
            cli_name!(),
            " msg {} \"text\" [--no-wake] [--json]"
        ),
        to
    )
}

/// The words around the address (`at` is its place), read as
/// `_postal_send_teach` reads them.
pub fn parse(words: &[&str], at: usize) -> Result<Mail, String> {
    let to = words[at].to_string();
    let mut m = Mail {
        to: to.clone(),
        text: String::new(),
        wake: true,
        json: false,
    };
    for (i, w) in words.iter().enumerate() {
        if i == at {
            continue;
        }
        match *w {
            "--no-wake" => m.wake = false,
            "--wake" => {}
            "--json" => m.json = true,
            f if K2_ONLY.contains(&f) => return Err(k2_only(f, &to)),
            f if f.starts_with('-') => return Err(unknown_flag(f)),
            t => {
                if m.text.is_empty() {
                    m.text = t.to_string();
                } else {
                    m.text = format!("{} {t}", m.text);
                }
            }
        }
    }
    if m.text.is_empty() {
        return Err(usage(&to));
    }
    Ok(m)
}

/// `_postal_body "workspace=…" "to=…" "text=…" "wake=__true|__false"`.
pub fn body(workspace: &str, m: &Mail) -> String {
    let mut pairs = Vec::new();
    for (k, v) in [("workspace", workspace), ("to", &m.to), ("text", &m.text)] {
        if !v.is_empty() {
            pairs.push((Text::from(k), s(v)));
        }
    }
    pairs.push((Text::from("wake"), Value::Bool(m.wake)));
    py::dumps(&Value::Object(pairs))
}

/// The send, then the printer; the exit.
pub fn send(client: &mut Client, m: &Mail) -> Exit {
    let workspace = if client.project.is_empty() {
        crate::daemon::logical_cwd(std::env::var_os("PWD"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        client.project.clone()
    };
    let body = body(&workspace, m);
    let mut closing = Closing::default();
    let passport = client.passport().is_some();
    let in_session = client.session;
    let (raw, call) = match client.send(ENDPOINT, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            (super::super::without_nuls(&r.body), Exit::Ok)
        }
        Err(
            e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. }),
        ) => {
            out::stderr_line(&e.to_string());
            end_in_pipe()
        }
        Err(e) => (Vec::new(), Exit::from_curl(e.curl_code())),
    };
    let shown = reader(&raw, m.json, in_session);
    let printed = shown.print();
    let exit = if printed == shown.exit {
        pipefail(call, shown.exit)
    } else {
        printed
    };
    closing.flush(client, exit);
    exit
}

/// The codes that are a refusal (exit 3), then the others.
const EXITS: &[(&str, i32)] = &[
    ("owner_only", 3),
    ("forbidden", 3),
    ("postal_off", 3),
    ("postal_people_only", 3),
    ("postal_people_decide", 3),
    ("postal_owner_decides", 3),
    ("postal_own_workspace_only", 3),
    ("role_required", 3),
    ("postal_not_friends", 3),
    ("postal_secret_in_text", 3),
    ("postal_route_louder", 3),
    ("postal_owner_link_missing", 3),
    ("postal_rekey_pending", 3),
    ("not_found", 4),
    ("postal_no_address", 4),
    ("postal_no_code", 4),
    ("usage", 2),
    ("bad_postal_address", 2),
    ("postal_code_needed", 2),
    ("too_large", 2),
];

/// `_postal_print send "$as_json"` on what the call printed;
/// `in_session`: `HOOK_SOCK` is set (its `PS`).
pub fn reader(raw: &[u8], as_json: bool, in_session: bool) -> Shown {
    let mut p = Printed::default();
    let Some(d) = py::loads(raw) else {
        return Shown::err(py::daemon_answered(raw) + "\n", Exit::Error);
    };
    match lines(&d, as_json, in_session, &mut p) {
        Ok(code) => Shown {
            stdout: p.stdout,
            stderr: p.stderr,
            exit: Exit::from_curl(code),
        },
        Err(r) => raised(p.stdout, p.stderr, &r),
    }
}

/// `x[0].upper() + x[1:] + "."` for a string, or the exception at `line`.
fn capital(v: &Value, line: u32) -> Result<String, Raise> {
    let Value::Str(_) = v else {
        return Err(Raise::at(
            line,
            format!("TypeError: '{}' object is not subscriptable", type_name(v)),
        ));
    };
    let t = str_of(v);
    let mut chars = t.chars();
    let first: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    Ok(format!("{first}{}.", chars.as_str()))
}

/// The lines of `_postal_print`'s Python (`python3 -c '` starts with a
/// newline, so its first line is empty).
pub mod line {
    /// `print("Error: %s%s" % ((code + ": ") if code else "", hint))`.
    pub const ERROR: u32 = 14;
    /// In a session: `"Postal tab" not in hint and "Settings" not in hint`.
    pub const PERSON: u32 = 15;
    /// `sys.exit(3 if code in perm else {…}.get(code, 1))`.
    pub const EXIT: u32 = 18;
    pub const ASKED: u32 = 24;
    pub const CARD: u32 = 25;
    pub const HINT: u32 = 26;
    pub const CAPITAL: u32 = 27;
    pub const HANDLE: u32 = 33;
    pub const WORKSPACE: u32 = 34;
    pub const STATUS: u32 = 36;
    pub const SENT: u32 = 37;
    pub const BADGE: u32 = 38;
    pub const RECORD: u32 = 39;
    pub const QUEUED: u32 = 40;
}

/// In a session, after the error line: [`PERSON_LINE`] for a person's
/// step, unless the daemon's hint already says where.
fn person_line(d: &Value, p: &mut Printed) -> Result<(), Raise> {
    let Value::Object(pairs) = d else {
        return Ok(());
    };
    let Some(Value::Object(err)) = get(pairs, "error") else {
        return Ok(());
    };
    let code = get(err, "code").cloned().unwrap_or_else(|| s(""));
    if !(eq(&code, &s("owner_only")) || eq(&code, &s("postal_people_only"))) {
        return Ok(());
    }
    let hint = get(err, "hint").cloned().unwrap_or_else(|| s(""));
    let says = |needle: &str| py_in(needle, &hint).map_err(|e| Raise::at(line::PERSON, e));
    if !says("Postal tab")? && !says("Settings")? {
        p.err(PERSON_LINE);
    }
    Ok(())
}

fn lines(d: &Value, as_json: bool, in_session: bool, p: &mut Printed) -> Result<i32, Raise> {
    let errs = py::error_branch_at(d, p, EXITS, false, (line::ERROR, line::EXIT));
    if in_session && errs.as_ref().is_ok_and(Option::is_some) {
        // Python's line 15 runs before the exit's line 18: a code it
        // reaches there is a string, so the map can't raise after it.
        person_line(d, p)?;
    }
    if let Some(code) = errs? {
        return Ok(code);
    }
    if as_json {
        p.line(&dumps_indent(d));
        return Ok(0);
    }
    if truthy(&get_or_null(d, "asked", line::ASKED)?) {
        let card = py::or(
            get_or_null(d, "request", line::CARD)?,
            get_or_null(d, "card", line::CARD)?,
        );
        let waiting = if truthy(&get_or_null(d, "waiting", line::CARD)?) {
            ", waiting already"
        } else {
            ""
        };
        p.line(&format!(
            "Asked: a card is on your person's Thread ({}){waiting}.",
            str_of(&card)
        ));
        let hint = py::or(
            get_or_null(d, "hint", line::HINT)?,
            s("a person answers the card; the answer comes back to you as a [permission:] turn"),
        );
        p.line(&capital(&hint, line::CAPITAL)?);
        return Ok(2);
    }
    get_or_null(d, "handle", line::HANDLE)?;
    let ws = py::or(
        get_or_null(d, "workspace", line::WORKSPACE)?,
        Value::Object(Vec::new()),
    );
    get_or_null(&ws, "handle", line::WORKSPACE)?;
    let st = get_or_null(d, "status", line::STATUS)?;
    let failed = eq(&st, &s("failed"));
    let reason = get_or_null(d, "reason", line::SENT)?;
    let why = if truthy(&reason) && failed {
        let r = index(d, "reason", line::SENT)?;
        format!("{})", concat(" (", r, line::SENT)?)
    } else {
        String::new()
    };
    p.line(&format!(
        "{} to {}: {}{why}",
        if failed { "Not sent" } else { "Postal mail" },
        str_of(&get_or_null(d, "to", line::SENT)?),
        str_of(&st)
    ));
    if truthy(&get_or_null(d, "badge", line::BADGE)?) {
        let badge = str_of(index(d, "badge", line::BADGE)?);
        p.line(&format!("Badge:    {badge}"));
    }
    if truthy(&get_or_null(d, "deliveryId", line::RECORD)?) {
        let id = str_of(index(d, "deliveryId", line::RECORD)?);
        p.line(&format!(
            concat!("Record:   {} (", cli_name!(), " msg verify {})"),
            id, id
        ));
    }
    if eq(&st, &s("queued")) {
        let hint = py::or(
            get_or_null(d, "hint", line::QUEUED)?,
            s("Queued on this server; it goes out at the next try."),
        );
        p.line(&str_of(&hint));
    }
    if eq(&st, &s("held")) {
        p.line("Held on the Postal network until their server picks it up (up to 7 days).");
    }
    if eq(&st, &s("delivered")) {
        p.line("Their server stored it.");
    }
    Ok(i32::from(failed))
}
