//! The held App requests, as the shell CLI answers them
//! (`cli/src/app-auth.sh`): `app approvals [<request-id>] [--app A]
//! [--state S|--all]` (the list, each request's permission card, and an
//! agent's line under its own requests),
//! `app approvals revoke <id> | --app <name>`, `app deny <id> [--reason
//! T]` and `app approve <id>`.
//!
//! `_app_approve <id>` ([`approve`]): the owner, at a terminal outside a
//! session, approves one held App request. It is `app approve`, and it
//! runs where the shell runs it after `app egress add` and `app
//! box-service request` were held, when stdin is a terminal and
//! `NO_APPROVE` is not `1`.
//!
//! The steps, as the shell's:
//! 1. In a session: the `owner_only` line, exit 3.
//! 2. `GET /cli/app/approvals?state=all` over TCP (`_compute_call … tcp`).
//! 3. Its Python finds the one request whose id is `id` or starts with
//!    it; none or several (or a raise, its traceback first) is the
//!    `request_not_found` line, exit 3. A request that isn't pending is
//!    the `not_pending` line, exit 3.
//! 4. The request's summary and who asked, on stderr.
//! 5. A confirm token over the vault socket ([`crate::confirm`]); none is
//!    exit 3.
//! 6. `POST /cli/app/approve {request, confirmToken}` over TCP, and its
//!    Python's lines (exit 1 when the answer names an error).
//!
//! The token goes from the confirm's answer into the body and nowhere
//! else.

use super::call::{Cj, Ctx};
use super::py::{self, Out};
use super::{Answer, Stop};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out;
use crate::out::json::Value;
use crate::out::py as pyfmt;
use crate::transport::encode::quote;
use k2_client::{cli_name, product_name};

/// `_app_approve`'s line in a session.
const OWNER_ONLY: &str = concat!(
    "{\"error\":\"owner_only\",\"hint\":\"the owner or an admin approves in person: ",
    "Settings \u{2192} App Approvals, or ",
    cli_name!(),
    " app approve <id> from a terminal outside ",
    product_name!(),
    ". An agent can never approve.\"}"
);

/// The agent's last line under its own requests.
const OWN_REQUESTS: &str = concat!(
    "These are your own requests. An owner or admin answers them; you can withdraw one: ",
    cli_name!(),
    " app deny <request-id>."
);

/// `…matches '<id>' (k2 app approvals)"}`.
const NOT_FOUND_TAIL: &str = concat!("' (", cli_name!(), " app approvals)\"}");

/// The usage lines (exit 2).
pub mod usage {
    use super::{cli_name, product_name};
    pub const APPROVE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app approve <request-id>   (the owner or an admin, in person, from a terminal outside ",
        product_name!(),
        ")"
    );
    /// `cmd_app_approvals`' `$usage`.
    pub const APPROVALS: &str = concat!(
        cli_name!(),
        " app approvals [<request-id>] [--app A] [--state pending|approved|denied|withdrawn|expired|failed|all] [--all] [--json]"
    );
    /// A `--state` no state.
    pub const STATE: &str =
        "usage: --state is pending, approved, denied, withdrawn, expired, failed or all, not '";
    pub const REVOKE: &str = concat!(
        "usage: ",
        cli_name!(),
        " app approvals revoke <request-id> | --app <name> [--json]   (the owner)"
    );
    pub const DENY: &str = concat!(
        "usage: ",
        cli_name!(),
        " app deny <request-id> [--reason TEXT]"
    );
}

/// The verbs here.
pub const VERBS: &[&str] = &["approvals", "approve", "deny"];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `approvals`: its query.
    List { query: String, json: bool },
    /// `approvals revoke`: its body.
    Revoke { body: String, json: bool },
    /// `deny`: its body.
    Deny { body: String, json: bool },
    /// `approve <id>`.
    Approve { id: String },
}

/// The words after the verb. `None`: a help form (the shell's).
pub fn parse(verb: &str, words: &[&str]) -> Option<Result<Ask, Stop>> {
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    Some(match verb {
        "approve" => match words.first() {
            None | Some(&"") => Err(Stop::usage(usage::APPROVE)),
            Some(id) => Ok(Ask::Approve { id: id.to_string() }),
        },
        "deny" => deny(words),
        _ if words.first() == Some(&"revoke") => revoke(&words[1..]),
        _ => list(words),
    })
}

/// `cmd_app_approvals`' loop and checks.
fn list(words: &[&str]) -> Result<Ask, Stop> {
    let (mut app, mut all, mut json) = (String::new(), false, false);
    let (mut state, mut id) = (String::new(), String::new());
    let whole = || Stop::usage(format!("usage: {}", usage::APPROVALS));
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--app" => {
                // `app="${2:-}"; shift 2`: with no word left, `_need_value`'s line.
                app = rest.get(1).ok_or_else(|| Stop::need_value(w))?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--state" => {
                state = rest.get(1).ok_or_else(whole)?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--all" => all = true,
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!(
                    "Unknown argument: {w} (usage: {})",
                    usage::APPROVALS
                )))
            }
            // `[ -z "$id" ]`: an empty id is no id yet.
            _ if id.is_empty() => id = w.to_string(),
            _ => return Err(whole()),
        }
        rest = &rest[1..];
    }
    if !matches!(
        state.as_str(),
        "" | "pending" | "approved" | "denied" | "withdrawn" | "expired" | "failed" | "all"
    ) {
        return Err(Stop::usage(format!("{}{state}'", usage::STATE)));
    }
    let mut query = if all {
        "state=all".to_string()
    } else if state.is_empty() {
        "state=pending".to_string()
    } else {
        format!("state={state}")
    };
    if !app.is_empty() {
        query.push_str(&format!("&app={}", quote(&app)));
    }
    if !id.is_empty() {
        query.push_str(&format!("&request={}", quote(&id)));
    }
    Ok(Ask::List { query, json })
}

/// `_app_approvals_revoke`'s loop and checks.
fn revoke(words: &[&str]) -> Result<Ask, Stop> {
    let (mut id, mut app, mut json) = (String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--app" => {
                app = rest
                    .get(1)
                    .ok_or_else(|| Stop::usage(usage::REVOKE))?
                    .to_string();
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Stop::usage(format!("Unknown flag: {w}\n{}", usage::REVOKE)))
            }
            _ if id.is_empty() => id = w.to_string(),
            _ => return Err(Stop::usage(usage::REVOKE)),
        }
        rest = &rest[1..];
    }
    if id.is_empty() == app.is_empty() {
        return Err(Stop::usage(usage::REVOKE));
    }
    let body = Cj::new().s("request", &id).s("app", &app).body();
    Ok(Ask::Revoke { body, json })
}

/// `cmd_app_deny`'s loop.
fn deny(words: &[&str]) -> Result<Ask, Stop> {
    let (mut id, mut reason, mut json) = (String::new(), String::new(), false);
    let mut rest = words;
    while let Some(w) = rest.first().copied() {
        match w {
            "--reason" => {
                reason = rest.get(1).ok_or_else(|| Stop::need_value(w))?.to_string();
                rest = &rest[2..];
                continue;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(Stop::usage(format!("Unknown flag: {w}"))),
            _ => id = w.to_string(),
        }
        rest = &rest[1..];
    }
    if id.is_empty() {
        return Err(Stop::usage(usage::DENY));
    }
    let body = Cj::new().s("request", &id).s("reason", &reason).body();
    Ok(Ask::Deny { body, json })
}

/// Runs it: what it printed, or the exit it ended with.
pub fn run(ctx: &mut Ctx, ask: &Ask) -> Answer {
    let answer = super::access::answer;
    match ask {
        Ask::List { query, json } => {
            let body = ctx.call(false, "/cli/app/approvals", query)?;
            if *json {
                return Ok(super::wallet::raw(&body));
            }
            // `K2_AP_AGENT="$(if _compute_in_session; then echo 1; fi)"`.
            let agent = ctx.in_session();
            let mut o = Out::default();
            let r = list_lines(&body, agent, &mut o);
            Ok(o.done(r))
        }
        Ask::Revoke { body, json } => {
            let body = ctx.call(true, "/cli/app/approvals/revoke", body)?;
            Ok(answer(&body, *json, revoke_lines))
        }
        Ask::Deny { body, json } => {
            let body = ctx.call(true, "/cli/app/deny", body)?;
            Ok(answer(&body, *json, deny_lines))
        }
        Ask::Approve { id } => approve(ctx, id),
    }
}

/// `cmd_app_approvals`' Python; `agent` is its `K2_AP_AGENT` (`1` in a
/// session).
fn list_lines(body: &[u8], agent: bool, o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let rs = py::or(py::dget(&v, "requests", 4)?, py::empty_list());
    if !py::truthy(&rs) {
        o.line("No held App requests (--state all lists every state).");
        return Ok(Exit::Ok);
    }
    for r in py::iter(&rs, 7)? {
        let id = py::idx(&r, "id", 8)?;
        let status = py::idx(&r, "status", 8)?;
        let kind = py::idx(&r, "kind", 8)?;
        let label = py::idx(&r, "appLabel", 8)?;
        let ws = py::dget(&r, "workspaceHandle", 8)?;
        let by = py::idx(&r, "requestedBy", 8)?;
        o.line(&format!(
            "{}  {} {} {} ({}) by {}",
            py::s(&id),
            py::left(&status, 9),
            py::left(&kind, 11),
            py::s(&label),
            py::s(&ws),
            py::s(&by)
        ));
        o.line(&format!("    {}", py::s(&py::idx(&r, "summary", 9)?)));
        if py::truthy(&py::dget(&r, "reason", 10)?) {
            o.line(&format!("    why: {}", py::s(&py::idx(&r, "reason", 11)?)));
        }
        let reasons = py::or(py::dget(&r, "reasons", 12)?, py::empty_list());
        for why in py::iter(&reasons, 12)? {
            o.line(&format!("    held: {}", py::s(&why)));
        }
        if py::truthy(&py::dget(&r, "decidedBy", 14)?) {
            let status = py::idx(&r, "status", 15)?;
            let by = py::idx(&r, "decidedBy", 15)?;
            let via = if py::truthy(&py::dget(&r, "confirmVia", 15)?) {
                py::concat(" via ", &py::idx(&r, "confirmVia", 15)?, 15)?
            } else {
                String::new()
            };
            o.line(&format!("    {} by {}{via}", py::s(&status), py::s(&by)));
        }
        // `c = r.get("card") if isinstance(r.get("card"), dict) else None`
        let card = py::dget(&r, "card", 16)?;
        let c = matches!(card, Value::Object(_)).then_some(card);
        if let Some(c) = c.filter(py::truthy) {
            let routed = py::or(py::dget(&c, "routedTo", 18)?, py::empty_list());
            let mut logins = Vec::new();
            for x in py::iter(&routed, 18)? {
                if matches!(x, Value::Object(_)) {
                    logins.push(py::dget_or(&x, "login", py::text(""), 18)?);
                }
            }
            let joined = py::join(", ", &Value::Array(logins), 18)?;
            let to = if joined.is_empty() {
                "every owner and admin".to_string()
            } else {
                joined
            };
            let thread = py::or(py::dget(&c, "thread", 19)?, py::text("?"));
            o.line(&format!(
                "    card {}: {}, on the Thread of {}, sent to {to}",
                py::s(&py::dget(&c, "id", 19)?),
                py::s(&py::dget(&c, "state", 19)?),
                py::s(&thread)
            ));
        } else if matches!(py::idx(&r, "status", 20)?, Value::Str(t) if t.as_str() == Some("pending"))
        {
            o.line("    no permission card (it waits in Settings \u{2192} App Approvals)");
        }
    }
    if agent && !py::truthy(&py::dget(&v, "canApprove", 22)?) {
        o.line(OWN_REQUESTS);
    }
    Ok(Exit::Ok)
}

/// `_app_approvals_revoke`'s Python.
fn revoke_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let revoked = py::or(py::dget(&v, "revoked", 4)?, py::empty_list());
    for r in py::iter(&revoked, 4)? {
        let id = py::idx(&r, "id", 5)?;
        let summary = py::idx(&r, "summary", 5)?;
        o.line(&format!("{} revoked: {}", py::s(&id), py::s(&summary)));
    }
    o.line(&py::s(&py::or(py::dget(&v, "note", 6)?, py::text(""))));
    Ok(Exit::Ok)
}

/// `cmd_app_deny`'s Python.
fn deny_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let r = py::idx(&py::load(body, 3)?, "request", 3)?;
    let id = py::idx(&r, "id", 4)?;
    let status = py::idx(&r, "status", 4)?;
    let summary = py::idx(&r, "summary", 4)?;
    o.line(&format!(
        "{} {}: {}",
        py::s(&id),
        py::s(&status),
        py::s(&summary)
    ));
    Ok(Exit::Ok)
}

/// `_app_approve <id>`: what it printed, or the exit it ended with.
pub fn approve(ctx: &mut Ctx, id: &str) -> Answer {
    if ctx.in_session() {
        out::stderr_line(OWNER_ONLY);
        return Err(Exit::Refused);
    }
    let body = ctx.call_on(false, "/cli/app/approvals", "state=all", true)?;
    let found = match find(&body, id) {
        Ok(Some(text)) => text,
        other => {
            if let Err(e) = other {
                // The traceback, as the reader in `$(…)` printed it.
                let shown = Out::default().done(Err(e));
                let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
            }
            out::stderr_line(&format!(
                "{{\"error\":\"request_not_found\",\"hint\":\"no single App request matches '{id}{NOT_FOUND_TAIL}"
            ));
            return Err(Exit::Refused);
        }
    };
    // `$(…)` took the trailing newlines; `sed -n Np` reads its lines.
    let held = found.trim_end_matches('\n');
    let lines: Vec<&str> = held.split('\n').collect();
    let line = |n: usize| lines.get(n).copied().unwrap_or("");
    let (full, state, label) = (line(0), line(1), line(2));
    if state != "pending" {
        out::stderr_line(&format!(
            "{{\"error\":\"not_pending\",\"hint\":\"{full} is {state}\"}}"
        ));
        return Err(Exit::Refused);
    }
    for l in lines.iter().skip(4) {
        out::stderr_line(l);
    }
    let Some(tok) = crate::confirm::confirm_token(&mut ctx.confirm(), "app-approve", full, label)
    else {
        return Err(Exit::Refused);
    };
    // `_compute_json` puts a token first (it rides stdin, one line), then
    // the rest.
    let first = tok.split('\n').next().unwrap_or("");
    let body = Cj::new().s("confirmToken", first).s("request", full).body();
    drop(tok);
    let answer = ctx.call_on(true, "/cli/app/approve", &body, true)?;
    let mut o = Out::default();
    let r = approved_lines(&answer, &mut o);
    Ok(o.done(r))
}

/// The finder's Python: its lines, `None` when it exits 1 (not exactly
/// one request).
fn find(body: &[u8], want: &str) -> Result<Option<String>, Raise> {
    let v = py::load(body, 4)?;
    let reqs = py::or(py::dget(&v, "requests", 4)?, py::empty_list());
    let mut rows = Vec::new();
    for r in py::iter(&reqs, 4)? {
        let rid = py::idx(&r, "id", 4)?;
        if rid == py::text(want) {
            rows.push(r);
            continue;
        }
        match &rid {
            Value::Str(_) => {
                if py::s(&rid).starts_with(want) {
                    rows.push(r);
                }
            }
            other => {
                return Err(Raise::at(
                    4,
                    format!(
                        "AttributeError: '{}' object has no attribute 'startswith'",
                        pyfmt::type_name(other)
                    ),
                ))
            }
        }
    }
    if rows.len() != 1 {
        return Ok(None);
    }
    let r = &rows[0];
    let mut text = String::new();
    let mut print = |s: String| {
        text.push_str(&s);
        text.push('\n');
    };
    print(py::s(&py::idx(r, "id", 8)?));
    print(py::s(&py::idx(r, "status", 8)?));
    print(py::s(&py::head(&py::idx(r, "summary", 8)?, 64, 8)?));
    print("---".to_string());
    print(py::s(&py::idx(r, "summary", 9)?));
    let by = py::idx(r, "requestedBy", 10)?;
    let reason = if py::truthy(&py::dget(r, "reason", 10)?) {
        py::concat(": ", &py::idx(r, "reason", 10)?, 10)?
    } else {
        String::new()
    };
    print(format!("asked by {}{reason}", py::s(&by)));
    Ok(Some(text))
}

/// The approve answer's Python.
fn approved_lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = py::load(body, 3)?;
    let r = py::idx(&v, "request", 4)?;
    let id = py::idx(&r, "id", 5)?;
    let status = py::dget(&v, "status", 5)?;
    let summary = py::idx(&r, "summary", 5)?;
    o.line(&format!(
        "{} {}: {}",
        py::s(&id),
        py::s(&status),
        py::s(&summary)
    ));
    if py::truthy(&py::dget(&v, "note", 6)?) {
        o.line(&format!("  {}", py::s(&py::idx(&v, "note", 7)?)));
    }
    let result = py::dget(&v, "result", 8)?;
    let boxes = if matches!(result, Value::Object(_)) {
        py::dget(&py::or(result, Value::Object(Vec::new())), "boxServices", 8)?
    } else {
        Value::Null
    };
    let refused = py::dget(&py::or(boxes, Value::Object(Vec::new())), "refused", 9)?;
    for x in py::iter(&py::or(refused, py::empty_list()), 9)? {
        let name = py::dget(&x, "name", 10)?;
        let hint = py::dget(&x, "hint", 10)?;
        o.line(&format!(
            "  box service {} not allowed: {}",
            py::s(&name),
            py::s(&hint)
        ));
    }
    if py::truthy(&py::dget(&v, "error", 11)?) {
        let hint = py::dget(&py::idx(&v, "error", 12)?, "hint", 12)?;
        o.line(&format!("  {}", py::s(&hint)));
        return Ok(Exit::Error);
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finder_takes_one_request_by_its_id_or_a_prefix() {
        let body = br#"{"requests":[{"id":"req-12","status":"pending","summary":"reach api.example","requestedBy":"ann","reason":"sync"},{"id":"req-2","status":"done","summary":"s","requestedBy":"bo"}]}"#;
        assert_eq!(
            find(body, "req-1").unwrap().unwrap(),
            "req-12\npending\nreach api.example\n---\nreach api.example\nasked by ann: sync\n"
        );
        assert_eq!(find(body, "req").unwrap(), None);
        assert_eq!(find(br#"{}"#, "x").unwrap(), None);
        let e = find(br#"{"requests":[{"id":5}]}"#, "x").unwrap_err();
        assert_eq!(
            e.exception,
            "AttributeError: 'int' object has no attribute 'startswith'"
        );
    }

    #[test]
    fn the_answer_exits_1_when_it_names_an_error() {
        let mut o = Out::default();
        let r = approved_lines(
            br#"{"status":"approved","request":{"id":"r","summary":"s"},"note":"n","result":{"boxServices":{"refused":[{"name":"pg","hint":"h"}]}},"error":{"hint":"e"}}"#,
            &mut o,
        );
        assert_eq!(r.unwrap(), Exit::Error);
        assert_eq!(
            o.stdout,
            "r approved: s\n  n\n  box service pg not allowed: h\n  e\n"
        );
    }

    #[test]
    fn the_words_read_as_the_shell_reads_them() {
        assert_eq!(
            parse("approvals", &["--app", "a b", "--all"]),
            Some(Ok(Ask::List {
                query: "state=all&app=a%20b".into(),
                json: false
            }))
        );
        assert!(matches!(
            parse("approvals", &["-x"]),
            Some(Err(s)) if s.line.starts_with("Unknown argument: -x (usage: ")
        ));
        // 0.46.17: one request by its id, a state, and their usage lines.
        assert_eq!(
            parse("approvals", &["w a", "--state", "denied", "--app", "b"]),
            Some(Ok(Ask::List {
                query: "state=denied&app=b&request=w%20a".into(),
                json: false
            }))
        );
        assert_eq!(
            parse("approvals", &["--state", "", "", "w"]),
            Some(Ok(Ask::List {
                query: "state=pending&request=w".into(),
                json: false
            }))
        );
        assert_eq!(
            parse("approvals", &["--state", "denied", "--all"]),
            Some(Ok(Ask::List {
                query: "state=all".into(),
                json: false
            }))
        );
        let whole = Stop::usage(format!("usage: {}", usage::APPROVALS));
        assert_eq!(parse("approvals", &["a", "b"]), Some(Err(whole.clone())));
        assert_eq!(parse("approvals", &["--state"]), Some(Err(whole)));
        assert_eq!(
            parse("approvals", &["--state", "nope", "--all"]),
            Some(Err(Stop::usage(format!("{}nope'", usage::STATE))))
        );
        assert_eq!(
            parse("approvals", &["--app"]),
            Some(Err(Stop::need_value("--app")))
        );
        assert_eq!(
            parse("approvals", &["revoke", "--app", "acme"]),
            Some(Ok(Ask::Revoke {
                body: r#"{"app": "acme"}"#.into(),
                json: false
            }))
        );
        assert_eq!(
            parse("approvals", &["revoke", "r", "--app", "a"]),
            Some(Err(Stop::usage(usage::REVOKE)))
        );
        assert_eq!(
            parse("approvals", &["revoke", "--app"]),
            Some(Err(Stop::usage(usage::REVOKE)))
        );
        assert_eq!(
            parse("deny", &["a", "b", "--reason", "r"]),
            Some(Ok(Ask::Deny {
                body: r#"{"request": "b", "reason": "r"}"#.into(),
                json: false
            }))
        );
        assert_eq!(
            parse("deny", &["--x"]),
            Some(Err(Stop::usage("Unknown flag: --x")))
        );
        assert_eq!(
            parse("approve", &[]),
            Some(Err(Stop::usage(usage::APPROVE)))
        );
        assert_eq!(
            parse("approve", &["r", "x"]),
            Some(Ok(Ask::Approve { id: "r".into() }))
        );
        assert_eq!(parse("approve", &["r", "-h"]), None);
    }

    #[test]
    fn the_lines_name_the_brand() {
        assert!(OWNER_ONLY.contains(concat!(cli_name!(), " app approve <id>")));
        assert!(NOT_FOUND_TAIL.contains(concat!(cli_name!(), " app approvals")));
        assert!(usage::APPROVALS.starts_with(concat!(cli_name!(), " app approvals [")));
        assert!(OWN_REQUESTS.ends_with(concat!(cli_name!(), " app deny <request-id>.")));
    }

    #[test]
    fn each_request_says_where_its_card_is() {
        let body = br#"{"canApprove":false,"requests":[{"id":"w1","status":"pending","kind":"manage","appLabel":"a","requestedBy":"agent:a","summary":"s","card":{"id":"w1","state":"pending","thread":"a","routedTo":[{"login":"ada"},"x",{"login":"bo"}]}},{"id":"w2","status":"pending","kind":"k","appLabel":"b","requestedBy":"r","summary":"t","card":{}},{"id":"w3","status":"denied","kind":"k","appLabel":"c","requestedBy":"r","summary":"u","card":null},{"id":"w4","status":"pending","kind":"k","appLabel":"d","requestedBy":"r","summary":"v","card":{"routedTo":[]}}]}"#;
        let mut o = Out::default();
        assert_eq!(list_lines(body, true, &mut o).unwrap(), Exit::Ok);
        let want = [
            "w1  pending   manage      a (None) by agent:a",
            "    s",
            "    card w1: pending, on the Thread of a, sent to ada, bo",
            "w2  pending   k           b (None) by r",
            "    t",
            "    no permission card (it waits in Settings \u{2192} App Approvals)",
            "w3  denied    k           c (None) by r",
            "    u",
            "w4  pending   k           d (None) by r",
            "    v",
            "    card None: None, on the Thread of ?, sent to every owner and admin",
            OWN_REQUESTS,
        ];
        assert_eq!(o.stdout, want.join("\n") + "\n");
        // The owner (or canApprove) gets no last line.
        let mut o = Out::default();
        list_lines(body, false, &mut o).unwrap();
        assert!(!o.stdout.contains(OWN_REQUESTS));
        // A login that isn't a string: join's TypeError, at line 18.
        let mut o = Out::default();
        let r = list_lines(
            br#"{"requests":[{"id":"w","status":"p","kind":"k","appLabel":"a","requestedBy":"r","summary":"s","card":{"routedTo":[{"login":7}]}}]}"#,
            false,
            &mut o,
        );
        assert!(r.is_err());
    }
}
