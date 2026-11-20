//! `k5 msg <handle::label.postal.bot> "text" [--no-wake] [--json]`: Postal
//! mail, as the shell's `_postal_send_teach` (`cli/src/remote-target.sh`)
//! sends it and `_postal_print send` (`cli/src/postal.sh`) prints it.
//!
//! The address is the first word that doesn't start with `-`; the other
//! words, in order: `--no-wake`, `--wake`, `--json`; a flag only messages
//! to agents on this server take is [`k2_only`], exit 2; any other flag is
//! [`unknown_flag`], exit 2; the rest is the text, joined by spaces. No
//! text is [`usage`], exit 2. Then `cli_post_json /cli/postal/send` over
//! TCP on the left of a pipe into the printer: the command ends with the
//! printer's code, or curl's when the printer's is 0; no answer ends it by
//! SIGPIPE.

use super::super::{end_in_pipe, pipefail, raised, Raise, Shown};
use super::py::{self, concat, dumps_indent, eq, get_or_null, index, s, Printed};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::py::{str_of, truthy, type_name};
use crate::out::{self};
use crate::prelude::Closing;
use crate::transport::{Call, Channel, Client, Error};
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
    let (raw, call) = match client.call(Channel::Tcp, ENDPOINT, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            (super::super::without_nuls(&r.body), Exit::Ok)
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            end_in_pipe()
        }
        Err(e) => (Vec::new(), Exit::from_curl(e.curl_code())),
    };
    let shown = reader(&raw, m.json);
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

/// `_postal_print send "$as_json"` on what the call printed.
pub fn reader(raw: &[u8], as_json: bool) -> Shown {
    let mut p = Printed::default();
    let Some(d) = py::loads(raw) else {
        return Shown::err(py::daemon_answered(raw) + "\n", Exit::Error);
    };
    match lines(&d, as_json, &mut p) {
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

fn lines(d: &Value, as_json: bool, p: &mut Printed) -> Result<i32, Raise> {
    if let Some(code) = py::error_branch(d, p, EXITS, false)? {
        return Ok(code);
    }
    if as_json {
        p.line(&dumps_indent(d));
        return Ok(0);
    }
    if truthy(&get_or_null(d, "asked", 18)?) {
        let card = py::or(get_or_null(d, "request", 19)?, get_or_null(d, "card", 19)?);
        let waiting = if truthy(&get_or_null(d, "waiting", 19)?) {
            ", waiting already"
        } else {
            ""
        };
        p.line(&format!(
            "Asked: a card is on your person's Thread ({}){waiting}.",
            str_of(&card)
        ));
        let hint = py::or(
            get_or_null(d, "hint", 20)?,
            s("a person answers the card; the answer comes back to you as a [permission:] turn"),
        );
        p.line(&capital(&hint, 21)?);
        return Ok(2);
    }
    get_or_null(d, "handle", 28)?;
    let ws = py::or(get_or_null(d, "workspace", 29)?, Value::Object(Vec::new()));
    get_or_null(&ws, "handle", 29)?;
    let st = get_or_null(d, "status", 31)?;
    let failed = eq(&st, &s("failed"));
    let reason = get_or_null(d, "reason", 32)?;
    let why = if truthy(&reason) && failed {
        format!("{})", concat(" (", index(d, "reason", 32)?, 32)?)
    } else {
        String::new()
    };
    p.line(&format!(
        "{} to {}: {}{why}",
        if failed { "Not sent" } else { "Postal mail" },
        str_of(&get_or_null(d, "to", 32)?),
        str_of(&st)
    ));
    if truthy(&get_or_null(d, "badge", 33)?) {
        p.line(&format!("Badge:    {}", str_of(index(d, "badge", 33)?)));
    }
    if truthy(&get_or_null(d, "deliveryId", 34)?) {
        let id = str_of(index(d, "deliveryId", 34)?);
        p.line(&format!(
            concat!("Record:   {} (", cli_name!(), " msg verify {})"),
            id, id
        ));
    }
    if eq(&st, &s("queued")) {
        let hint = py::or(
            get_or_null(d, "hint", 35)?,
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
