//! `compute roster|seats|trust|owners ...`, as `cmd_compute_trust`
//! answers them (`cli/src/compute-local-trust.sh`; `lanes` is not here):
//!
//! - `roster [show|--json]`, `roster key`, `roster set|attest <node>
//!   [...]`, `roster retire|revoke <node> [--reason R]` and `roster
//!   publish [--resign]`;
//! - `seats [list|--json]`, `seats request <owner> [...]`, `seats offer
//!   <request> --nodes a,b` and `seats end|suspend|resume <seat> [--why
//!   T]` (the owner's confirm);
//! - `trust [requests|list|ls|--json|--all]`, `trust approve <id>` (a
//!   person's confirm), `trust deny <id> [--why T]` and `trust withdraw
//!   <id>`;
//! - `owners [--json]` and `owners consent <owner> on|off` (on: a
//!   person's confirm).
//!
//! The confirm token goes from the vault socket's answer into the next
//! request's body, never into argv or the environment.

use super::super::admin::node::flows::{captured, confirm, stderr_text, to_stderr};
use super::super::lines::py_line;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{dumps, in_session, object, str_value, Cp, Words};
use crate::confirm::confirm_token;
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use k2_client::{brand::CLI_NAME, cli_name, product_name};
use std::path::Path;
use std::time::Duration;

/// `cli/src/compute-local-trust.sh`.
pub const SRC: &str = include_str!("../../../../../../cli/src/compute-local-trust.sh");

/// The line of `marker` in the first `python3 -c` after `after`.
macro_rules! line {
    ($after:expr, $marker:literal) => {
        const { py_line(SRC, $after, $marker) }
    };
}

/// The `line!` lookup for a Python with the shell's `'"'"'` in it.
macro_rules! line_q {
    ($after:expr, $marker:literal) => {
        const { super::py_line_q(SRC, $after, $marker) }
    };
}

pub const ROSTER_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute roster [show|key|set|retire|revoke|publish] (",
    cli_name!(),
    " compute --help)"
);
pub const ROSTER_SET_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute roster set <node> [--host H] [--provider P] [--site S] [--admin-reach a,b] [--attest TEXT]"
);
pub const SEATS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute seats [list|request|offer|end|suspend|resume] (",
    cli_name!(),
    " compute --help)"
);
pub const SEATS_REQUEST_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute seats request <owner-host> [--lane L] [--seat ID] [--secrets] [--why TEXT]"
);
pub const SEATS_OFFER_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute seats offer <request> --nodes a,b,c"
);
pub const TRUST_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute trust [requests|approve|deny|withdraw]"
);
pub const APPROVE_USAGE: &str = concat!("usage: ", cli_name!(), " compute trust approve <ctr_…>");
pub const DENY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute trust deny <id> [--why TEXT]"
);
pub const WITHDRAW_USAGE: &str = concat!("usage: ", cli_name!(), " compute trust withdraw <id>");
pub const CONSENT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute owners consent <owner> on|off"
);

const ROSTER_KEY_PERSON: &str = concat!(
    "{\"error\":\"person_only\",\"hint\":\"the roster key is minted and shown to a person: run it from a terminal outside ",
    product_name!(),
    "\"}"
);
const SEAT_PERSON: &str = concat!(
    "{\"error\":\"person_only\",\"hint\":\"the node owner ends, suspends or resumes a seat in person, from a terminal outside ",
    product_name!(),
    " (Touch ID / vault password)\"}"
);
const APPROVE_PERSON: &str = concat!(
    "{\"error\":\"person_only\",\"hint\":\"a person approves this, from a terminal outside ",
    product_name!(),
    " (Touch ID / vault password). An agent never approves.\"}"
);
const CONSENT_PERSON: &str = concat!(
    "{\"error\":\"person_only\",\"hint\":\"sending vault secrets to another owner's nodes is a person's choice: a terminal outside ",
    product_name!(),
    "\"}"
);

/// Which reader prints a GET's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Roster,
    Seats,
    Requests,
    Owners,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A GET, then its reader (`--json`: the answer, pretty).
    Get {
        path: &'static str,
        query: String,
        list: List,
    },
    /// A POST, then `_compute_trust_print`. `long`: `K2_CP_MAX_TIME=300`.
    Post {
        path: &'static str,
        body: String,
        tcp: bool,
        long: bool,
    },
    /// `roster key`.
    RosterKey,
    /// `seats end|suspend|resume <seat> [--why T]`.
    Seat {
        sub: String,
        seat: String,
        why: String,
    },
    /// `trust approve <id>`.
    Approve(String),
    /// `owners consent <owner> on`.
    ConsentOn(String),
}

/// `_compute_json`'s body: the pairs whose value is not an empty
/// string, in their order (none of these keys is a secret's).
fn body(pairs: Vec<(&str, Value)>) -> String {
    dumps(&object(
        pairs
            .into_iter()
            .filter(|(_, v)| !matches!(v, Value::Str(t) if t.as_str() == Some("")))
            .collect(),
    ))
}

fn usage<T>(line: impl Into<String>) -> Words<T> {
    Words::Usage(line.into())
}

/// Reads `rest` as the shell's `while [ $# -gt 0 ]` loops do: `flags`
/// take a value (`shift 2`), `bare` take none; anything else is
/// `Err(word)`. A value flag last: `Ok(None)` (`_need_value`'s line).
#[allow(clippy::type_complexity)]
fn flags<'a>(
    rest: &[&'a str],
    with_value: &[&str],
    bare: &[&str],
) -> Result<Option<Vec<(&'a str, &'a str)>>, &'a str> {
    let mut got = vec![];
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        if with_value.contains(&w) {
            if i + 2 > rest.len() {
                return Ok(None);
            }
            got.push((w, rest[i + 1]));
            i += 2;
        } else if bare.contains(&w) {
            got.push((w, ""));
            i += 1;
        } else {
            return Err(w);
        }
    }
    Ok(Some(got))
}

/// The last value given for any of `names` (`""` when none).
fn last<'a>(got: &[(&str, &'a str)], names: &[&str]) -> &'a str {
    got.iter()
        .rev()
        .find(|(f, _)| names.contains(f))
        .map_or("", |(_, v)| *v)
}

/// `local x="${1:-}"; [ -n "$x" ] && shift`: the word and what is left.
fn first<'a, 'b>(rest: &'b [&'a str]) -> (&'a str, &'b [&'a str]) {
    match rest.first() {
        Some(w) if !w.is_empty() => (*w, &rest[1..]),
        _ => ("", rest),
    }
}

/// `local sub="${1:-<default>}"; [ $# -gt 0 ] && shift`.
fn sub<'a, 'b>(rest: &'b [&'a str], default: &'a str) -> (&'a str, &'b [&'a str]) {
    match rest.first() {
        Some(w) => (if w.is_empty() { default } else { *w }, &rest[1..]),
        None => (default, rest),
    }
}

/// The words after `noun` (`roster`, `seats`, `trust` or `owners`).
pub fn parse(noun: &str, rest: &[&str]) -> Words<Ask> {
    match noun {
        "roster" => roster(rest),
        "seats" => seats(rest),
        "trust" => trust(rest),
        _ => owners(rest),
    }
}

fn roster(rest: &[&str]) -> Words<Ask> {
    let (sub, r) = sub(rest, "show");
    match sub {
        "show" | "--json" => Words::Ask(Ask::Get {
            path: "/cli/compute/roster",
            query: String::new(),
            list: List::Roster,
        }),
        "key" => Words::Ask(Ask::RosterKey),
        "set" | "attest" => {
            let (node, r) = first(r);
            if node.is_empty() {
                return usage(ROSTER_SET_USAGE);
            }
            let got = match flags(
                r,
                &[
                    "--host",
                    "--provider",
                    "--site",
                    "--admin-reach",
                    "--attest",
                    "--why",
                    "--reason",
                ],
                &["--json"],
            ) {
                Ok(Some(g)) => g,
                Ok(None) => return Words::ShiftFailed(r.last().map_or_else(String::new, |w| w.to_string())),
                Err(_) => return usage(ROSTER_SET_USAGE),
            };
            let v = |names: &[&str]| str_value(last(&got, names));
            Words::Ask(Ask::Post {
                path: "/cli/compute/roster/propose",
                body: body(vec![
                    ("op", s("set")),
                    ("node", str_value(node)),
                    ("host", v(&["--host"])),
                    ("provider", v(&["--provider"])),
                    ("site", v(&["--site"])),
                    ("adminReach", v(&["--admin-reach"])),
                    ("attest", v(&["--attest"])),
                    ("reason", v(&["--why", "--reason"])),
                ]),
                tcp: false,
                long: false,
            })
        }
        "retire" | "revoke" => {
            let (node, r) = first(r);
            if node.is_empty() {
                return usage(format!("usage: {CLI_NAME} compute roster {sub} <node>"));
            }
            let got = match flags(r, &["--reason"], &["--json"]) {
                Ok(Some(g)) => g,
                Ok(None) => return Words::ShiftFailed(r.last().map_or_else(String::new, |w| w.to_string())),
                Err(_) => {
                    return usage(format!(
                    "usage: {CLI_NAME} compute roster {sub} <node> [--reason compromised|key_lost|other]"
                ))
                }
            };
            Words::Ask(Ask::Post {
                path: "/cli/compute/roster/propose",
                body: body(vec![
                    ("op", str_value(sub)),
                    ("node", str_value(node)),
                    ("reason", str_value(last(&got, &["--reason"]))),
                ]),
                tcp: false,
                long: false,
            })
        }
        "publish" => {
            let mut pairs = vec![];
            if r.contains(&"--resign") {
                pairs.push(("resign", Value::Bool(true)));
            }
            Words::Ask(Ask::Post {
                path: "/cli/compute/roster/publish",
                body: body(pairs),
                tcp: false,
                long: true,
            })
        }
        _ => usage(ROSTER_USAGE),
    }
}

fn seats(rest: &[&str]) -> Words<Ask> {
    let (sub, r) = sub(rest, "list");
    match sub {
        "list" | "--json" => Words::Ask(Ask::Get {
            path: "/cli/compute/seats",
            query: String::new(),
            list: List::Seats,
        }),
        "request" => {
            let (owner, r) = first(r);
            if owner.is_empty() {
                return usage(SEATS_REQUEST_USAGE);
            }
            let got = match flags(
                r,
                &["--lane", "--seat", "--why"],
                &["--secrets", "--no-secrets", "--json"],
            ) {
                Ok(Some(g)) => g,
                Ok(None) => return Words::ShiftFailed(r.last().map_or_else(String::new, |w| w.to_string())),
                Err(_) => return usage(SEATS_REQUEST_USAGE),
            };
            let secrets = got
                .iter()
                .rev()
                .find(|(f, _)| matches!(*f, "--secrets" | "--no-secrets"))
                .is_some_and(|(f, _)| *f == "--secrets");
            Words::Ask(Ask::Post {
                path: "/cli/compute/seats/request",
                body: body(vec![
                    ("owner", str_value(owner)),
                    ("lane", str_value(last(&got, &["--lane"]))),
                    ("seat", str_value(last(&got, &["--seat"]))),
                    ("secrets", Value::Bool(secrets)),
                    ("why", str_value(last(&got, &["--why"]))),
                ]),
                tcp: false,
                long: false,
            })
        }
        "offer" => {
            let (req, r) = first(r);
            let got = match flags(r, &["--nodes"], &["--json"]) {
                Ok(Some(g)) => g,
                Ok(None) => return Words::ShiftFailed(r.last().map_or_else(String::new, |w| w.to_string())),
                Err(_) => return usage(SEATS_OFFER_USAGE),
            };
            let nodes = last(&got, &["--nodes"]);
            if req.is_empty() || nodes.is_empty() {
                return usage(SEATS_OFFER_USAGE);
            }
            Words::Ask(Ask::Post {
                path: "/cli/compute/seats/offer",
                body: body(vec![
                    ("request", str_value(req)),
                    ("nodes", str_value(nodes)),
                ]),
                tcp: false,
                long: false,
            })
        }
        "end" | "suspend" | "resume" => {
            let (seat, r) = first(r);
            let line = format!("usage: {CLI_NAME} compute seats {sub} <seat> [--why TEXT]");
            let got = match flags(r, &["--why"], &["--json"]) {
                Ok(Some(g)) => g,
                Ok(None) => return Words::ShiftFailed(r.last().map_or_else(String::new, |w| w.to_string())),
                Err(_) => return usage(line),
            };
            if seat.is_empty() {
                return usage(line);
            }
            Words::Ask(Ask::Seat {
                sub: sub.to_string(),
                seat: seat.to_string(),
                why: last(&got, &["--why"]).to_string(),
            })
        }
        _ => usage(SEATS_USAGE),
    }
}

fn trust(rest: &[&str]) -> Words<Ask> {
    let (sub, r) = sub(rest, "requests");
    match sub {
        "requests" | "list" | "ls" | "--json" | "--all" => {
            let all = sub == "--all" || r.contains(&"--all");
            Words::Ask(Ask::Get {
                path: "/cli/compute/trust/requests",
                query: format!("state={}", if all { "all" } else { "open" }),
                list: List::Requests,
            })
        }
        "approve" => match r.first() {
            Some(id) if !id.is_empty() => Words::Ask(Ask::Approve(id.to_string())),
            _ => usage(APPROVE_USAGE),
        },
        "deny" => {
            let (id, r) = first(r);
            let why = if r.first() == Some(&"--why") {
                r.get(1).copied().unwrap_or("")
            } else {
                ""
            };
            if id.is_empty() {
                return usage(DENY_USAGE);
            }
            Words::Ask(Ask::Post {
                path: "/cli/compute/trust/deny",
                body: body(vec![("request", str_value(id)), ("why", str_value(why))]),
                tcp: false,
                long: false,
            })
        }
        "withdraw" => match r.first() {
            Some(id) if !id.is_empty() => Words::Ask(Ask::Post {
                path: "/cli/compute/trust/withdraw",
                body: body(vec![("request", str_value(id))]),
                tcp: false,
                long: false,
            }),
            _ => usage(WITHDRAW_USAGE),
        },
        _ => usage(TRUST_USAGE),
    }
}

fn owners(rest: &[&str]) -> Words<Ask> {
    if rest.first() != Some(&"consent") {
        return Words::Ask(Ask::Get {
            path: "/cli/compute/owners",
            query: String::new(),
            list: List::Owners,
        });
    }
    let owner = rest.get(1).copied().unwrap_or("");
    match rest.get(2).copied().unwrap_or("") {
        "on" => Words::Ask(Ask::ConsentOn(owner.to_string())),
        "off" => Words::Ask(Ask::Post {
            path: "/cli/compute/owners/consent",
            body: body(vec![
                ("owner", str_value(owner)),
                ("on", Value::Bool(false)),
            ]),
            tcp: false,
            long: false,
        }),
        _ => usage(CONSENT_USAGE),
    }
}

/// `K2_CP_MAX_TIME=300 _compute_call POST ...`.
fn long_post(cp: &mut Cp<'_>, path: &str, payload: &str, tcp: bool) -> Result<Vec<u8>, Exit> {
    let before = cp.client.compute_post_max_time;
    cp.client.compute_post_max_time = Some(Duration::from_secs(300));
    let r = cp.call(true, path, payload, tcp);
    cp.client.compute_post_max_time = before;
    r
}

/// The requests and what they print. `sock`: the vault socket, for a
/// person's confirm.
pub fn send(cp: &mut Cp<'_>, a: &Ask, sock: &Path) -> Exit {
    match a {
        Ask::Get { path, query, list } => {
            let answer = match cp.call(false, path, query, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if cp.as_json {
                return pretty(&answer).print();
            }
            lister(*list, &answer).print()
        }
        Ask::Post {
            path,
            body,
            tcp,
            long,
        } => {
            let answer = if *long {
                long_post(cp, path, body, *tcp)
            } else {
                cp.call(true, path, body, *tcp)
            };
            match answer {
                Ok(b) => pretty(&b).print(),
                Err(e) => e,
            }
        }
        Ask::RosterKey => {
            if in_session(&cp.client.auth) {
                out::stderr_line(ROSTER_KEY_PERSON);
                return Exit::Refused;
            }
            match cp.call(true, "/cli/compute/roster/key", "{}", true) {
                Ok(b) => pretty(&b).print(),
                Err(e) => e,
            }
        }
        Ask::Seat { sub, seat, why } => {
            if in_session(&cp.client.auth) {
                out::stderr_line(SEAT_PERSON);
                return Exit::Refused;
            }
            let secret_id = format!("seat-{sub}:{seat}");
            let Some(tok) = confirm_token(&mut confirm(cp, sock), "compute-trust", &secret_id, "")
            else {
                return Exit::Refused;
            };
            let payload = body(vec![
                ("confirmToken", str_value(&tok)),
                ("seat", str_value(seat)),
                ("why", str_value(why)),
            ]);
            drop(tok);
            match long_post(cp, &format!("/cli/compute/seats/{sub}"), &payload, true) {
                Ok(b) => pretty(&b).print(),
                Err(e) => e,
            }
        }
        Ask::Approve(id) => approve(cp, sock, id),
        Ask::ConsentOn(owner) => consent_on(cp, sock, owner),
    }
}

/// `_compute_trust_print`: the answer as `json.dumps(…, indent=2)`.
pub fn pretty(body: &[u8]) -> Shown {
    const AFTER: &str = "\n_compute_trust_print() {";
    let mut cx = Cx::new(0);
    let l = line!(AFTER, "print(json.dumps(");
    let r = render::load(&cx, body, l).and_then(|v| cx.at(l, p::dumps(&v, false, Some(2))));
    let r = r.map(|t| cx.print(&t));
    shown(cx, r)
}

fn shown(mut cx: Cx, r: Result<(), Raise>) -> Shown {
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e),
    }
}

/// `print(fmt % (args...))` at `line`.
fn print(cx: &mut Cx, line: u32, fmt: &str, args: &[Value]) -> Result<(), Raise> {
    let t = cx.at(line, p::pct(fmt, args))?;
    cx.print(&t);
    Ok(())
}

/// The reader of a list's answer.
pub fn lister(list: List, body: &[u8]) -> Shown {
    let mut cx = Cx::new(0);
    let r = match list {
        List::Roster => roster_list(&mut cx, body),
        List::Seats => seats_list(&mut cx, body),
        List::Requests => requests_list(&mut cx, body),
        List::Owners => owners_list(&mut cx, body),
    };
    shown(cx, r)
}

fn roster_list(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "GET /cli/compute/roster \"\"";
    let v = render::load(cx, body, line!(AFTER, "v = json.load(sys.stdin)"))?;
    let l = line!(AFTER, "l = v.get(\"latest\")");
    let latest = cx.at(l, p::get(&v, "latest"))?;
    if p::truthy(&latest) {
        let l = line!(AFTER, "print(\"node roster seq %s");
        let seq = cx.at(l, p::idx(&latest, "seq"))?;
        let key = cx.at(
            l,
            p::idx(&latest, "keyFingerprint").and_then(|k| p::slice(&k, None, Some(16))),
        )?;
        let issued = cx.at(l, p::idx(&latest, "issuedAt"))?;
        let stale = if p::truthy(&cx.at(l, p::get(&v, "stale"))?) {
            concat!(", STALE: ", cli_name!(), " compute roster publish --resign")
        } else {
            ""
        };
        print(
            cx,
            l,
            "node roster seq %s (key %s…), issued %s%s",
            &[seq, key, issued, s(stale)],
        )?;
        let l = line!(AFTER, "for e in l[\"entries\"]:");
        let entries = cx.at(l, p::idx(&latest, "entries").and_then(|e| p::iter(&e)))?;
        for e in entries {
            let l = line!(AFTER, "print(\"  %-16s %-8s %-15s");
            let name = cx.at(l, p::idx(&e, "name"))?;
            let status = cx.at(l, p::idx(&e, "status"))?;
            let taint = p::or(cx.at(l, p::get(&e, "taintBasis"))?, s("-"));
            let host = cx.at(l, p::idx(&e, "host"))?;
            let fp = cx.at(
                l,
                p::idx(&e, "nodeFp").and_then(|k| p::slice(&k, None, Some(12))),
            )?;
            print(
                cx,
                l,
                "  %-16s %-8s %-15s host %s  key %s…",
                &[name, status, taint, host, fp],
            )?;
        }
    } else {
        cx.print(concat!(
            "no node roster yet: ",
            cli_name!(),
            " compute roster set <node> …, then a person approves it"
        ));
    }
    let l = line!(AFTER, "for n in v.get(\"nodes\", []):");
    let nodes = cx.at(
        l,
        p::get_or(&v, "nodes", Value::Array(vec![])).and_then(|n| p::iter(&n)),
    )?;
    for n in nodes {
        let l = line!(AFTER, "st = n.get(\"statement\") or {}");
        let st = p::or(cx.at(l, p::get(&n, "statement"))?, Value::Object(vec![]));
        let l = line!(AFTER, "print(\"node %-16s statement");
        let name = cx.at(l, p::idx(&n, "name"))?;
        let has = if p::truthy(&st) {
            "yes"
        } else {
            "none (k2-node older than protocol 9)"
        };
        let taint = cx.at(l, p::get_or(&st, "taint", s("-")))?;
        let on = p::or(cx.at(l, p::get(&n, "roster"))?, s("no"));
        print(
            cx,
            l,
            "node %-16s statement %s, taint %s, on roster: %s",
            &[name, s(has), taint, on],
        )?;
    }
    let l = line!(AFTER, "for r in v.get(\"pending\", []):");
    let pending = cx.at(
        l,
        p::get_or(&v, "pending", Value::Array(vec![])).and_then(|n| p::iter(&n)),
    )?;
    for r in pending {
        let l = line!(AFTER, "print(\"waiting: %s  %s\"");
        let id = cx.at(l, p::idx(&r, "id"))?;
        let summary = cx.at(l, p::idx(&r, "summary"))?;
        print(cx, l, "waiting: %s  %s", &[id, summary])?;
    }
    Ok(())
}

fn seats_list(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "GET /cli/compute/seats \"\"";
    let v = render::load(cx, body, line!(AFTER, "v = json.load(sys.stdin)"))?;
    let l = line!(AFTER, "for s in v[\"offered\"]:");
    for x in cx.at(l, p::idx(&v, "offered").and_then(|o| p::iter(&o)))? {
        let l = line!(AFTER, "print(\"offered %-24s");
        let args = [
            cx.at(l, p::idx(&x, "id"))?,
            cx.at(l, p::idx(&x, "clientLabel"))?,
            cx.at(l, p::idx(&x, "lane"))?,
            cx.at(l, p::idx(&x, "state"))?,
            s(&cx.at(l, p::idx(&x, "nodes").and_then(|n| p::join(",", &n)))?),
        ];
        print(
            cx,
            l,
            "offered %-24s to %-24s lane %-12s %-9s nodes %s",
            &args,
        )?;
    }
    let l = line!(AFTER, "for s in v[\"held\"]:");
    for x in cx.at(l, p::idx(&v, "held").and_then(|o| p::iter(&o)))? {
        let l = line!(AFTER, "print(\"held    %-24s");
        let id = cx.at(l, p::idx(&x, "id"))?;
        let owner = cx.at(l, p::idx(&x, "ownerId"))?;
        let lane = cx.at(l, p::idx(&x, "lane"))?;
        let state = cx.at(l, p::idx(&x, "state"))?;
        let nodes = cx.at(l, p::idx(&x, "nodes").and_then(|n| p::join(",", &n)))?;
        let nodes = if nodes.is_empty() { s("-") } else { s(&nodes) };
        let note = if p::truthy(&cx.at(l, p::get(&x, "note"))?) {
            let n = cx.at(l, p::idx(&x, "note"))?;
            cx.at(l, p::add(&s("  ("), &n).and_then(|t| p::add(&t, &s(")"))))?
        } else {
            s("")
        };
        print(
            cx,
            l,
            "held    %-24s on %-24s lane %-12s %-9s nodes %s%s",
            &[id, owner, lane, state, nodes, note],
        )?;
    }
    let l = line!(AFTER, "if not v[\"offered\"] and not v[\"held\"]:");
    if !p::truthy(&cx.at(l, p::idx(&v, "offered"))?) && !p::truthy(&cx.at(l, p::idx(&v, "held"))?) {
        cx.print("no seats");
    }
    Ok(())
}

fn requests_list(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "GET /cli/compute/trust/requests \"state=$st\"";
    let v = render::load(cx, body, line!(AFTER, "v = json.load(sys.stdin)"))?;
    let l = line!(AFTER, "for r in v[\"requests\"]:");
    for r in cx.at(l, p::idx(&v, "requests").and_then(|o| p::iter(&o)))? {
        let l = line!(AFTER, "print(\"%s  %-9s %-12s %s\"");
        let args = [
            cx.at(l, p::idx(&r, "id"))?,
            cx.at(l, p::idx(&r, "state"))?,
            cx.at(l, p::idx(&r, "kind"))?,
            cx.at(l, p::idx(&r, "summary"))?,
        ];
        print(cx, l, "%s  %-9s %-12s %s", &args)?;
    }
    let l = line!(AFTER, "if not v[\"requests\"]:");
    if !p::truthy(&cx.at(l, p::idx(&v, "requests"))?) {
        cx.print("nothing waits for a person");
    }
    Ok(())
}

fn owners_list(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "GET /cli/compute/owners \"\"\n";
    let v = render::load(cx, body, line!(AFTER, "v = json.load(sys.stdin)"))?;
    let l = line!(AFTER, "s = v.get(\"self\")");
    let me = cx.at(l, p::get(&v, "self"))?;
    if p::truthy(&me) {
        let l = line!(AFTER, "print(\"this server (%s)");
        let owner = cx.at(l, p::idx(&me, "ownerId"))?;
        let fp = cx.at(
            l,
            p::idx(&me, "key").and_then(|k| p::idx(&k, "fingerprint")),
        )?;
        print(cx, l, "this server (%s): node-roster key %s", &[owner, fp])?;
    }
    let l = line!(AFTER, "for t in v[\"trusted\"]:");
    for t in cx.at(l, p::idx(&v, "trusted").and_then(|o| p::iter(&o)))? {
        let l = line!(AFTER, "l = t.get(\"latest\") or {}");
        let latest = p::or(cx.at(l, p::get(&t, "latest"))?, Value::Object(vec![]));
        let l = line!(AFTER, "print(\"%s: key %s, roster seq");
        let owner = cx.at(l, p::idx(&t, "ownerId"))?;
        let fp = cx.at(l, p::idx(&t, "keyFp"))?;
        let seq = cx.at(l, p::get_or(&latest, "seq", s("-")))?;
        let on = if p::truthy(&cx.at(l, p::idx(&t, "secretsConsent"))?) {
            "on"
        } else {
            "off"
        };
        print(
            cx,
            l,
            "%s: key %s, roster seq %s, secrets %s",
            &[owner, fp, seq, s(on)],
        )?;
    }
    let l = line!(AFTER, "if not s and not v[\"trusted\"]:");
    if !p::truthy(&me) && !p::truthy(&cx.at(l, p::idx(&v, "trusted"))?) {
        cx.print("no node owners pinned, and this server has no node-roster key yet");
    }
    Ok(())
}

/// `_compute_trust_approve <id>`.
fn approve(cp: &mut Cp<'_>, sock: &Path, want: &str) -> Exit {
    if in_session(&cp.client.auth) {
        out::stderr_line(APPROVE_PERSON);
        return Exit::Refused;
    }
    let list = match cp.call(false, "/cli/compute/trust/requests", "state=all", true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut cx = Cx::new(0);
    let found = find(&mut cx, &list, want);
    let found = match found {
        Ok(true) => cx.out.trim_end_matches('\n').to_string(),
        Ok(false) | Err(_) => {
            if let Err(e) = &found {
                stderr_text(&e.text());
            }
            out::stderr_line(&format!(
                "{{\"error\":\"request_not_found\",\"hint\":\"no single request matches '{want}' ({CLI_NAME} compute trust)\"}}"
            ));
            return Exit::Refused;
        }
    };
    let lines: Vec<&str> = found.split('\n').collect();
    let nth = |i: usize| lines.get(i).copied().unwrap_or("");
    let (full, state, label, digest) = (nth(0), nth(1), nth(2), nth(3));
    if state != "pending" {
        out::stderr_line(&format!(
            "{{\"error\":\"not_pending\",\"hint\":\"{full} is {state}\"}}"
        ));
        return Exit::Refused;
    }
    for l in lines.iter().skip(5) {
        out::stderr_line(l);
    }
    let Some(tok) = confirm_token(&mut confirm(cp, sock), "compute-trust", full, label) else {
        return Exit::Refused;
    };
    let payload = body(vec![
        ("confirmToken", str_value(&tok)),
        ("request", str_value(full)),
        ("expectedDigest", str_value(digest)),
    ]);
    drop(tok);
    let answer = match long_post(cp, "/cli/compute/trust/approve", &payload, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let shown = pretty(&answer);
    let exit = shown.print();
    if exit != Exit::Ok {
        return exit;
    }
    // `next="$(… .get("result") or {}).get("consentNext")) or "")')"`.
    let cx = Cx::new(0);
    let next = captured(render::load(&cx, &answer, 1).and_then(|v| {
        let result = p::or(cx.at(1, p::get(&v, "result"))?, Value::Object(vec![]));
        let n = cx.at(1, p::get(&result, "consentNext"))?;
        Ok(p::or(n, s("")))
    }));
    match next {
        Ok(n) if !n.is_empty() => {
            out::stderr_line(&format!(
                "The seat asked to send vault secrets to that owner's nodes. That is a separate yes: {n}"
            ));
            Exit::Ok
        }
        Ok(_) => Exit::Ok,
        Err(e) => e,
    }
}

/// The `found` Python of `_compute_trust_approve`: the one request
/// matching `want` printed as the shell's lines (`Ok(false)`: `sys.exit(1)`).
fn find(cx: &mut Cx, body: &[u8], want: &str) -> Result<bool, Raise> {
    const AFTER: &str = "\n_compute_trust_approve() {";
    let l = line!(AFTER, "rows = [r for r in");
    let v = render::load(cx, body, l)?;
    let all = cx.at(l, p::idx(&v, "requests"))?;
    let mut rows = vec![];
    for r in cx.at(l, p::iter(&all))? {
        let id = cx.at(l, p::idx(&r, "id"))?;
        if p::eq_s(&id, want) || cx.at(l, p::idx(&r, "id").and_then(|i| p::startswith(&i, want)))? {
            rows.push(r);
        }
    }
    if rows.len() != 1 {
        return Ok(false);
    }
    let r = rows.remove(0);
    let l = line!(AFTER, "print(r[\"id\"]); print(r[\"state\"])");
    for key in ["id", "state"] {
        let x = cx.at(l, p::idx(&r, key))?;
        cx.print(&p::str_of(&x));
    }
    let label = cx.at(
        l,
        p::idx(&r, "summary").and_then(|x| p::slice(&x, None, Some(64))),
    )?;
    cx.print(&p::str_of(&label));
    let digest = cx.at(l, p::idx(&r, "digest"))?;
    cx.print(&p::str_of(&digest));
    cx.print("---");
    let summary = cx.at(l, p::idx(&r, "summary"))?;
    cx.print(&p::str_of(&summary));
    let l = line!(AFTER, "if r.get(\"reason\"):");
    if p::truthy(&cx.at(l, p::get(&r, "reason"))?) {
        let x = cx.at(l, p::idx(&r, "reason"))?;
        let t = cx.at(l, p::add(&s("reason: "), &x))?;
        cx.print(&p::str_of(&t));
    }
    Ok(true)
}

/// `owners consent <owner> on`: the owner's pinned key, the daemon's
/// words for the yes (stderr), a person's confirm, then the consent.
fn consent_on(cp: &mut Cp<'_>, sock: &Path, owner: &str) -> Exit {
    if in_session(&cp.client.auth) {
        out::stderr_line(CONSENT_PERSON);
        return Exit::Refused;
    }
    let list = match cp.call(false, "/cli/compute/owners", "", true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let cx = Cx::new(0);
    let kfp = captured(key_of(&cx, &list, owner));
    let kfp = match kfp {
        Ok(k) => k,
        Err(e) => return e,
    };
    if kfp.is_empty() {
        out::stderr_line(&format!(
            "{{\"error\":\"node_owner_not_trusted\",\"hint\":\"no pinned node owner {owner} ({CLI_NAME} compute owners)\"}}"
        ));
        return Exit::Refused;
    }
    let preview = body(vec![
        ("owner", str_value(owner)),
        ("on", Value::Bool(true)),
        ("preview", Value::Bool(true)),
    ]);
    let answer = match cp.call(true, "/cli/compute/owners/consent", &preview, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    const TEXT: &str = "K2_CJ_preview_j=true)\" tcp";
    let mut cx = Cx::new(0);
    let r = (|| {
        let l = line!(TEXT, "v=json.load(sys.stdin)");
        let v = render::load(&cx, &answer, l)?;
        let l = line!(TEXT, "print(v[\"text\"])");
        let t = cx.at(l, p::idx(&v, "text"))?;
        cx.print(&p::str_of(&t));
        let l = line_q!(TEXT, "node-roster key: %s");
        let o = cx.at(l, p::idx(&v, "owner"))?;
        let k = cx.at(l, p::idx(&v, "keyFp"))?;
        print(&mut cx, l, "  %s's node-roster key: %s", &[o, k])
    })();
    if let Err(e) = to_stderr(cx, r) {
        return e;
    }
    let label = format!("Send vault secrets to {owner}'s nodes");
    let Some(tok) = confirm_token(
        &mut confirm(cp, sock),
        "compute-trust",
        &format!("consent:{kfp}"),
        &label,
    ) else {
        return Exit::Refused;
    };
    let payload = body(vec![
        ("confirmToken", str_value(&tok)),
        ("owner", str_value(owner)),
        ("on", Value::Bool(true)),
    ]);
    drop(tok);
    match cp.call(true, "/cli/compute/owners/consent", &payload, true) {
        Ok(b) => pretty(&b).print(),
        Err(e) => e,
    }
}

/// The `kfp` Python: the pinned owner's key fingerprint, or `""`.
fn key_of(cx: &Cx, body: &[u8], owner: &str) -> Result<Value, Raise> {
    const AFTER: &str = "GET /cli/compute/owners \"\" tcp";
    let l = line!(AFTER, "r=[t for t in");
    let v = render::load(cx, body, l)?;
    let trusted = cx.at(l, p::idx(&v, "trusted").and_then(|t| p::iter(&t)))?;
    let mut rows = vec![];
    for t in trusted {
        let id = cx.at(l, p::idx(&t, "ownerId"))?;
        if p::eq_s(&id, owner) || p::eq_s(&cx.at(l, p::idx(&t, "keyFp"))?, owner) {
            rows.push(t);
        }
    }
    let l = line!(AFTER, "print(r[0][\"keyFp\"]");
    if rows.len() == 1 {
        cx.at(l, p::idx(&rows[0], "keyFp"))
    } else {
        Ok(s(""))
    }
}
