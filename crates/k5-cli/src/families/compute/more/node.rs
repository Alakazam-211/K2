//! `compute node health|servers|packages ...`, as `cmd_compute_node` answers them
//! (`cli/src/compute-node.sh`); the other node subverbs are
//! [`super::super::admin::node`]'s:
//!
//! - `health <node> [--json]`: `_compute_render_health`;
//! - `servers <node> [list|ls] [--refresh] [--json]`, `servers <node> add
//!   --url URL --code-stdin|--code C [...]`, `approve <request>`, `deny
//!   <request> [--why T]`, `limits <server> ...`, `pause|resume <server>`,
//!   `revoke <server> [--now] [--wipe]` and `owner <server> [...]`;
//! - `packages|pkg ...`: [`super::packages`].
//!
//! What goes to the shell, before anything is sent: a `--for` in days
//! whose number is not plain digits or passes the shell's 64-bit
//! arithmetic (the shell evaluates that text as arithmetic), and a
//! duration the shell wraps.

use super::super::admin::node::{self as an, flows};
use super::super::lines::py_line;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{dumps, object, print_body, str_value, Cp, Words};
use crate::confirm::text_of_bytes;
use crate::exit::Exit;
use crate::families::{py_space, raised, Raise, Shown};
use crate::out;
use crate::out::json::{Text, Value};
use crate::out::pylib::json as pyjson;
use crate::prompt;
use crate::transport::encode::quote;
use k2_client::{brand::CLI_NAME, cli_name};
use std::path::Path;

/// `cli/src/compute-node.sh`.
const SRC: &str = include_str!("../../../../../../cli/src/compute-node.sh");

macro_rules! line {
    ($after:expr, $marker:literal) => {
        const { py_line(SRC, $after, $marker) }
    };
}

pub const HEALTH_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node health <node> [--json]"
);
pub const SERVERS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …] (",
    cli_name!(),
    " compute --help)"
);
pub const SERVERS_ACT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …]"
);
const ADD_FLAG_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> add --url URL --code-stdin | --code CODE.FP [--trust own|team] [--for 7d|none] [limits…]"
);
pub const ADD_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> add --url URL --code-stdin < code-file (or K2_NODE_ENROLL_CODE) [--trust own|team] [--for 7d|none] (a code on the command line, --code CODE.FP, is visible to other users on this machine)"
);
const APPROVE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> approve <request>"
);
const LIMITS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> limits <server> [--weight N] [--reason TEXT] key=value…"
);
const LIMITS_FLAG_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> limits <server> key=value…"
);
const REVOKE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> revoke <server> [--now] [--wipe]"
);
const OWNER_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node servers <node> owner <server> [--old keep|revoke] [--caches move|wipe] [--now] [--reason TEXT]"
);

/// Which reader prints a POST's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// `add`: a `warning` on stderr, then `_compute_admin_held_or_approve`.
    Add,
    /// `owner`: a `note` on stderr, then `_compute_admin_held_or_approve`.
    Owner,
    /// `limits`: a held change, else `--json` or its line.
    Limits,
    Pause,
    Revoke,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// `health <node>`; `json`: a `--json` anywhere.
    Health { node: String, json: bool },
    /// `servers <node> [list]`.
    List { query: String, json: bool },
    /// `servers <node> add ...`: the body waits for the code.
    Add { node: String, rest: Vec<String> },
    /// A POST over TCP.
    Post {
        path: &'static str,
        body: String,
        after: After,
        json: bool,
    },
    /// `servers <node> approve <request>`.
    Approve(String),
    /// `servers <node> deny ...` (`cmd_compute_node_admin deny`).
    Deny(an::Ask),
    /// `packages ...`.
    Packages(super::packages::Ask),
}

fn usage(line: impl Into<String>) -> Option<Words<Ask>> {
    Some(Words::Usage(line.into()))
}

/// The words after `node`, for `health` and `servers`; `None`: not these
/// (or the shell's).
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let rest = words.get(1..).unwrap_or(&[]);
    match words.first().copied()? {
        "health" => {
            let node = rest.first().copied().unwrap_or("");
            if node.is_empty() {
                return usage(HEALTH_USAGE);
            }
            Some(Words::Ask(Ask::Health {
                node: node.to_string(),
                json: rest.contains(&"--json"),
            }))
        }
        "servers" => servers(rest),
        "packages" | "pkg" => Some(match super::packages::parse(rest)? {
            Words::Ask(a) => Words::Ask(Ask::Packages(a)),
            Words::Usage(l) => Words::Usage(l),
            Words::ShiftFailed(f) => Words::ShiftFailed(f),
        }),
        _ => None,
    }
}

fn post(path: &'static str, body: String, after: After, json: bool) -> Option<Words<Ask>> {
    Some(Words::Ask(Ask::Post {
        path,
        body,
        after,
        json,
    }))
}

fn servers(words: &[&str]) -> Option<Words<Ask>> {
    let node = words.first().copied().unwrap_or("");
    if node.is_empty() || node.starts_with('-') {
        return usage(SERVERS_USAGE);
    }
    let mut rest = &words[1..];
    let mut act = rest
        .first()
        .copied()
        .filter(|a| !a.is_empty())
        .unwrap_or("list");
    if act == "--json" {
        act = "list";
    }
    if !rest.is_empty() && act != "list" {
        rest = &rest[1..];
    }
    if act == "list" && rest.first() == Some(&"list") {
        rest = &rest[1..];
    }
    let json = rest.contains(&"--json");
    let node_v = || str_value(node);
    match act {
        "list" | "ls" => {
            let mut q = format!("node={}", quote(node));
            for a in rest {
                if *a == "--refresh" {
                    q.push_str("&refresh=1");
                }
            }
            Some(Words::Ask(Ask::List { query: q, json }))
        }
        "add" if add_handed(&rest.iter().map(|w| w.to_string()).collect::<Vec<_>>()) => None,
        "add" => Some(Words::Ask(Ask::Add {
            node: node.to_string(),
            rest: rest.iter().map(|w| w.to_string()).collect(),
        })),
        "approve" => match rest.first() {
            Some(id) if !id.is_empty() => Some(Words::Ask(Ask::Approve(id.to_string()))),
            _ => usage(APPROVE_USAGE),
        },
        "deny" => {
            let mut w = vec!["deny"];
            w.extend_from_slice(rest);
            Some(match an::more::admin(&w)? {
                Words::Ask(a) => Words::Ask(Ask::Deny(a)),
                Words::Usage(l) => Words::Usage(l),
                Words::ShiftFailed(f) => Words::ShiftFailed(f),
            })
        }
        "limits" => limits(node, rest, json),
        "pause" | "resume" => {
            let server = rest.first().copied().unwrap_or("");
            if server.is_empty() {
                return usage(format!(
                    "usage: {CLI_NAME} compute node servers <node> {act} <server>"
                ));
            }
            let body = body(vec![
                ("node", node_v()),
                ("server", str_value(server)),
                ("action", str_value(act)),
            ]);
            post("/cli/compute/nodes/servers/pause", body, After::Pause, json)
        }
        "revoke" => {
            let (mut server, mut now, mut wipe) = ("", false, false);
            for w in rest {
                match *w {
                    "--now" => now = true,
                    "--wipe" => wipe = true,
                    "--json" => {}
                    w if w.starts_with('-') => return usage(REVOKE_USAGE),
                    w => server = w,
                }
            }
            if server.is_empty() {
                return usage(REVOKE_USAGE);
            }
            let body = body(vec![
                ("node", node_v()),
                ("server", str_value(server)),
                ("now", Value::Bool(now)),
                ("wipe", Value::Bool(wipe)),
            ]);
            post(
                "/cli/compute/nodes/servers/revoke",
                body,
                After::Revoke,
                json,
            )
        }
        "owner" => owner(node, rest),
        _ => usage(SERVERS_ACT_USAGE),
    }
}

/// `_compute_json`'s body: the pairs whose value is not an empty string,
/// in their order.
fn body(pairs: Vec<(&str, Value)>) -> String {
    dumps(&object(
        pairs
            .into_iter()
            .filter(|(_, v)| !matches!(v, Value::Str(t) if t.as_str() == Some("")))
            .collect(),
    ))
}

/// A value flag's value and the index after it; `None`: `shift 2` with
/// one word left.
fn value<'a>(rest: &[&'a str], i: usize) -> Option<(&'a str, usize)> {
    if i + 2 > rest.len() {
        return None;
    }
    Some((rest[i + 1], i + 2))
}

/// `$(_compute_secs "$v")` in an assignment: the seconds, the usage line
/// (exit 2), or `None` for the shell's.
fn secs(v: &str) -> Option<Result<i64, Words<Ask>>> {
    match an::secs(v)? {
        Ok(n) => Some(Ok(n)),
        Err(Words::Usage(l)) => Some(Err(Words::Usage(l))),
        Err(Words::ShiftFailed(f)) => Some(Err(Words::ShiftFailed(f))),
        Err(Words::Ask(_)) => None,
    }
}

/// The `--for` value as `for_n`: `""`, `0`, days times 86400, or
/// `_compute_secs`. Days that are not plain digits are `bad`'s usage line
/// (exit 2), before anything is sent (0f25406366). `None`: the shell's
/// (days past its arithmetic).
fn for_secs(v: &str, bad: fn(&str) -> String) -> Option<Result<String, Words<Ask>>> {
    Some(Ok(match v {
        "" => String::new(),
        "none" | "never" | "0" => "0".to_string(),
        d if d.ends_with('d') => {
            let n = &d[..d.len() - 1];
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return Some(Err(Words::Usage(bad(v))));
            }
            let n: i64 = n.parse().ok()?;
            n.checked_mul(86400)?.to_string()
        }
        other => match secs(other)? {
            Ok(n) => n.to_string(),
            Err(stop) => return Some(Err(stop)),
        },
    }))
}

/// `limits`' line for days that are not plain digits.
fn limits_bad_for(v: &str) -> String {
    format!("usage: {CLI_NAME} compute node servers <node> limits <server> [for=7d|12h|none] key=value… (bad for '{v}')")
}

/// `add`'s line for days that are not plain digits.
fn add_bad_for(v: &str) -> String {
    format!("usage: {CLI_NAME} compute node servers <node> add --url URL --code-stdin [--for 7d|12h|none] (bad --for '{v}')")
}

/// `_compute_kv_json key=value…`: the object, or its usage line (the
/// shell then exits 2).
pub fn kv_json(pairs: &[String]) -> Result<Value, &'static str> {
    let mut out: Vec<(Text, Value)> = vec![];
    for a in pairs {
        let (k, v) = a.split_once('=').unwrap_or((a.as_str(), ""));
        let k = k.trim_matches(py_space);
        if k.is_empty() {
            return Err("usage: key=value");
        }
        let mut val = pyjson::loads(v).unwrap_or_else(|_| str_value(v));
        if matches!(k, "names" | "labels") {
            if let Value::Str(t) = &val {
                let text = p::str_of(&Value::Str(t.clone()));
                val = Value::Array(
                    text.split(',')
                        .filter(|x| !x.is_empty())
                        .map(str_value)
                        .collect(),
                );
            }
        }
        match out.iter_mut().find(|(key, _)| *key == Text::from(k)) {
            Some(slot) => slot.1 = val,
            None => out.push((Text::from(k), val)),
        }
    }
    Ok(Value::Object(out))
}

fn limits(node: &str, rest: &[&str], json: bool) -> Option<Words<Ask>> {
    let server = rest.first().copied().unwrap_or("");
    if server.is_empty() || server.starts_with('-') {
        return usage(LIMITS_USAGE);
    }
    let rest = &rest[1..];
    let (mut reason, mut forv, mut prio, mut pairs) = ("", "", None, Vec::<String>::new());
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--reason" | "--weight" | "--for" => {
                let Some((v, next)) = value(rest, i) else {
                    return Some(Words::ShiftFailed(rest[i].to_string()));
                };
                match w {
                    "--reason" => reason = v,
                    "--weight" => pairs.push(format!("weight={v}")),
                    _ => forv = v,
                }
                i = next;
                continue;
            }
            "priority=true" | "priority=yes" | "priority=1" => prio = Some(true),
            "priority=false" | "priority=no" | "priority=0" => prio = Some(false),
            w if w.starts_with("priority=") => return usage("usage: priority=true|false"),
            w if w.starts_with("for=") => forv = &w[4..],
            w if w.starts_with("max_job_secs=")
                && (w.ends_with('h') || w.ends_with('m') || w.ends_with('s')) =>
            {
                match secs(&w["max_job_secs=".len()..])? {
                    Ok(n) => pairs.push(format!("max_job_secs={n}")),
                    Err(stop) => return Some(stop),
                }
            }
            w if w.starts_with("network=") || w.starts_with("top_class=") => {
                let (k, v) = w.split_once('=').expect("an = in it");
                pairs.push(format!("{k}=\"{v}\""));
            }
            w if w.contains('=') => pairs.push(w.to_string()),
            "--json" => {}
            w => {
                return usage(format!(
                    "usage: {CLI_NAME} compute node servers <node> limits <server> key=value… (unknown '{w}')"
                ))
            }
        }
        i += 1;
    }
    let for_n = match for_secs(forv, limits_bad_for)? {
        Ok(n) => n,
        Err(stop) => return Some(stop),
    };
    if pairs.is_empty() && for_n.is_empty() && prio.is_none() {
        return usage(format!("{LIMITS_FLAG_USAGE} (at least one)"));
    }
    let set = match kv_json(&pairs) {
        Ok(v) => v,
        Err(line) => return usage(line),
    };
    let mut fields = vec![
        ("node", str_value(node)),
        ("server", str_value(server)),
        ("set", set),
    ];
    if !for_n.is_empty() {
        fields.push(("forSecs", Value::Int(for_n)));
    }
    fields.push(("reason", str_value(reason)));
    if let Some(b) = prio {
        fields.push(("priority", Value::Bool(b)));
    }
    post(
        "/cli/compute/nodes/servers/limits",
        body(fields),
        After::Limits,
        json,
    )
}

fn owner(node: &str, rest: &[&str]) -> Option<Words<Ask>> {
    let (mut server, mut old, mut caches, mut now, mut reason) = ("", "keep", "move", false, "");
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            f @ ("--old" | "--caches" | "--reason") => {
                let Some((v, next)) = value(rest, i) else {
                    return Some(Words::ShiftFailed(rest[i].to_string()));
                };
                match f {
                    "--old" => old = v,
                    "--caches" => caches = v,
                    _ => reason = v,
                }
                i = next;
                continue;
            }
            "--now" => now = true,
            "--json" => {}
            w if w.starts_with('-') => return usage(OWNER_USAGE),
            w => server = w,
        }
        i += 1;
    }
    if server.is_empty() {
        return usage(OWNER_USAGE);
    }
    if !matches!(old, "keep" | "revoke") {
        return usage(format!("{OWNER_USAGE} (--old is keep or revoke)"));
    }
    if !matches!(caches, "move" | "wipe") {
        return usage(format!("{OWNER_USAGE} (--caches is move or wipe)"));
    }
    let body = body(vec![
        ("node", str_value(node)),
        ("server", str_value(server)),
        ("old", str_value(old)),
        ("caches", str_value(caches)),
        ("now", Value::Bool(now)),
        ("reason", str_value(reason)),
    ]);
    post(
        "/cli/compute/nodes/servers/owner",
        body,
        After::Owner,
        false,
    )
}

/// The words of `add` after the node, read once nothing else stops the
/// command: the body, or what stops it. `None`: the shell's.
fn add_body(node: &str, rest: &[String]) -> Option<Result<String, Words<Ask>>> {
    let (mut url, mut code, mut code_stdin, mut trust, mut forv, mut reason) = (
        String::new(),
        String::new(),
        false,
        "own".to_string(),
        String::new(),
        String::new(),
    );
    let mut pairs: Vec<String> = vec![];
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i].as_str();
        match w {
            "--code-stdin" => code_stdin = true,
            "--gate" => {
                pairs.push("gate=true".into());
                pairs.push("top_class=\"gate\"".into());
            }
            "--exclusive" => pairs.push("exclusive=true".into()),
            "--json" => {}
            "--url" | "--code" | "--trust" | "--for" | "--reason" | "--weight"
            | "--max-running" | "--max-queued" | "--cpu-hours" | "--max-time" | "--cpus"
            | "--mem" | "--disk" | "--network" => {
                if i + 2 > rest.len() {
                    return Some(Err(Words::ShiftFailed(rest[i].to_string())));
                }
                let v = rest[i + 1].as_str();
                match w {
                    "--url" => url = v.into(),
                    "--code" => code = v.into(),
                    "--trust" => trust = v.into(),
                    "--for" => forv = v.into(),
                    "--reason" => reason = v.into(),
                    "--weight" => pairs.push(format!("weight={v}")),
                    "--max-running" => pairs.push(format!("max_running={v}")),
                    "--max-queued" => pairs.push(format!("max_queued={v}")),
                    "--cpu-hours" => pairs.push(format!("cpu_hours_day={v}")),
                    "--max-time" => match secs(v)? {
                        Ok(n) => pairs.push(format!("max_job_secs={n}")),
                        Err(stop) => return Some(Err(stop)),
                    },
                    "--cpus" => pairs.push(format!("max_cpus={v}")),
                    "--mem" => pairs.push(format!("max_mem_gb={}", strip_g(v))),
                    "--disk" => pairs.push(format!("disk_gb={}", strip_g(v))),
                    _ => pairs.push(format!("network=\"{v}\"")),
                }
                i += 2;
                continue;
            }
            w => {
                return Some(Err(Words::Usage(format!(
                    "{ADD_FLAG_USAGE} (unknown '{w}')"
                ))))
            }
        }
        i += 1;
    }
    // The code off argv: --code-stdin (or --code -) reads one line from
    // stdin; no --code at all takes the environment's.
    let mut code_v = str_value(&code);
    if code_stdin {
        if !code.is_empty() {
            return Some(Err(Words::Usage(format!(
                "{CLI_NAME} compute node servers add: --code-stdin or --code, not both"
            ))));
        }
        let typed = prompt::read_secret_stdin("Pairing code: ");
        let b = typed.expose();
        let b = b.strip_suffix(b"\r").unwrap_or(b);
        if b.is_empty() {
            return Some(Err(Words::Usage(format!(
                "{CLI_NAME} compute node servers add: --code-stdin: no code on stdin"
            ))));
        }
        code_v = text_of_bytes(b);
    } else if code == "-" {
        let typed = prompt::read_line(prompt::Echo::Shown, prompt::AtEof::Keep);
        let b = typed.expose();
        code_v = text_of_bytes(b.strip_suffix(b"\r").unwrap_or(b));
    } else if code.is_empty() {
        if let Some((_, v)) = k2_client::env_names::first_nonempty_os_in(
            k2_client::env_names::NODE_ENROLL_CODE,
            |n| std::env::var_os(n),
        ) {
            code_v = text_of_bytes(v.as_encoded_bytes());
        }
    }
    let no_code = matches!(&code_v, Value::Str(t) if t.as_str() == Some(""));
    if url.is_empty() || no_code {
        return Some(Err(Words::Usage(ADD_USAGE.into())));
    }
    let for_n = match for_secs(&forv, add_bad_for)? {
        Ok(n) => n,
        Err(stop) => return Some(Err(stop)),
    };
    let set = match kv_json(&pairs) {
        Ok(v) => v,
        Err(line) => return Some(Err(Words::Usage(line.into()))),
    };
    // The secret first, then the others in their order.
    let mut fields = vec![
        ("code", code_v),
        ("node", str_value(node)),
        ("url", str_value(&url)),
        ("trust", str_value(&trust)),
    ];
    if !for_n.is_empty() {
        fields.push(("forSecs", Value::Int(for_n)));
    }
    fields.push(("set", set));
    fields.push(("reason", str_value(&reason)));
    Some(Ok(body(fields)))
}

/// `${m%[Gg]}`.
fn strip_g(v: &str) -> &str {
    v.strip_suffix(['G', 'g']).unwrap_or(v)
}

/// Whether `add`'s words go to the shell (read before anything is sent:
/// a `--for` or `--max-time` the shell evaluates as arithmetic or wraps).
pub fn add_handed(rest: &[String]) -> bool {
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i].as_str();
        if matches!(
            w,
            "--url"
                | "--code"
                | "--trust"
                | "--for"
                | "--reason"
                | "--weight"
                | "--max-running"
                | "--max-queued"
                | "--cpu-hours"
                | "--max-time"
                | "--cpus"
                | "--mem"
                | "--disk"
                | "--network"
        ) {
            let v = rest.get(i + 1).map_or("", String::as_str);
            if w == "--for" && for_secs(v, add_bad_for).is_none() {
                return true;
            }
            if w == "--max-time" && secs(v).is_none() {
                return true;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    false
}

/// The requests and what they print. `sock`: the vault socket, for the
/// owner's confirm.
pub fn send(cp: &mut Cp<'_>, a: &Ask, sock: &Path) -> Exit {
    match a {
        Ask::Health { node, json } => {
            let q = format!("node={}", quote(node));
            let answer = match cp.call(false, "/cli/compute/nodes/health", &q, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if *json {
                return print_body(&answer);
            }
            let mut cx = Cx::new(p::now());
            let r = health(&mut cx, &answer);
            shown(cx, r).print()
        }
        Ask::List { query, json } => {
            let answer = match cp.call(false, "/cli/compute/nodes/servers", query, true) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if *json {
                return print_body(&answer);
            }
            render::render("servers", &answer, false, p::now()).print()
        }
        Ask::Add { node, rest } => {
            let payload = match add_body(node, rest) {
                Some(Ok(b)) => b,
                Some(Err(Words::Usage(l))) => {
                    out::stderr_line(&l);
                    return Exit::Usage;
                }
                Some(Err(Words::ShiftFailed(flag))) => return out::stop_need_value(&flag),
                Some(Err(_)) => return Exit::Error,
                None => unreachable!("handed to the shell before"),
            };
            let answer = match cp.call(true, "/cli/compute/nodes/servers/add", &payload, true) {
                Ok(b) => b,
                Err(e) => return e,
            };
            drop(payload);
            note_then_held(cp, sock, &answer, "warning")
        }
        Ask::Post {
            path,
            body,
            after,
            json,
        } => {
            let answer = match cp.call(true, path, body, true) {
                Ok(b) => b,
                Err(e) => return e,
            };
            match after {
                After::Add => note_then_held(cp, sock, &answer, "warning"),
                After::Owner => note_then_held(cp, sock, &answer, "note"),
                After::Limits => {
                    if flows::held(&answer) {
                        return flows::held_or_approve(cp, sock, &answer);
                    }
                    if *json {
                        return print_body(&answer);
                    }
                    one_line(&answer, *after).print()
                }
                After::Pause | After::Revoke => {
                    if *json {
                        return print_body(&answer);
                    }
                    one_line(&answer, *after).print()
                }
            }
        }
        Ask::Approve(id) => flows::approve(cp, sock, id),
        Ask::Deny(a) => an::send(cp, a, sock),
        Ask::Packages(a) => super::packages::send(cp, a, sock),
    }
}

fn shown(mut cx: Cx, r: Result<(), Raise>) -> Shown {
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e),
    }
}

/// `if v.get(key): print(key_word + ": " + v[key], file=sys.stderr)`, then
/// `_compute_admin_held_or_approve`.
fn note_then_held(cp: &mut Cp<'_>, sock: &Path, answer: &[u8], key: &str) -> Exit {
    let cx = Cx::new(0);
    let r = render::load(&cx, answer, 2).and_then(|v| {
        if p::truthy(&cx.at(3, p::get(&v, key))?) {
            let x = cx.at(3, p::idx(&v, key))?;
            let t = cx.at(3, p::add(&s("note: "), &x))?;
            flows::stderr_text(&format!("{}\n", p::str_of(&t)));
        }
        Ok(())
    });
    if let Err(e) = r {
        flows::stderr_text(&e.text());
        return Exit::Error;
    }
    flows::held_or_approve(cp, sock, answer)
}

/// The one-line readers of `limits`, `pause|resume` and `revoke`.
fn one_line(body: &[u8], after: After) -> Shown {
    let mut cx = Cx::new(0);
    let r = (|| {
        let v = render::load(&cx, body, 1)?;
        let t = match after {
            After::Limits => {
                let args = [
                    cx.at(1, p::get(&v, "server"))?,
                    cx.at(1, p::idx(&v, "node"))?,
                    cx.at(1, p::get_or(&v, "note", s("ok")))?,
                ];
                cx.at(1, p::pct("%s on %s: %s", &args))?
            }
            After::Pause => {
                let server = cx.at(1, p::idx(&v, "server"))?;
                let node = cx.at(1, p::idx(&v, "node"))?;
                let action = cx.at(1, p::idx(&v, "action"))?;
                let Value::Str(_) = &action else {
                    return Err(cx.raise(
                        1,
                        format!(
                            "AttributeError: '{}' object has no attribute 'split'",
                            p::tname(&action)
                        ),
                    ));
                };
                let text = p::str_of(&action);
                let last = text.rsplit('.').next().unwrap_or("");
                let note = cx.at(1, p::idx(&v, "note"))?;
                let did = s(&format!("{last}d"));
                cx.at(1, p::pct("%s on %s: %s. %s", &[server, node, did, note]))?
            }
            _ => {
                let args = [
                    cx.at(1, p::idx(&v, "server"))?,
                    cx.at(1, p::idx(&v, "node"))?,
                    cx.at(1, p::idx(&v, "note"))?,
                ];
                cx.at(1, p::pct("revoked %s on %s: %s", &args))?
            }
        };
        cx.print(&t);
        Ok(())
    })();
    shown(cx, r)
}

/// `gb(b)`: whole GB of an integer (a bool is one), else `?`.
fn gb(cx: &Cx, l: u32, b: &Value) -> Result<Value, Raise> {
    match b {
        Value::Int(_) | Value::Bool(_) => {
            let n = cx.at(l, p::floordiv(b, &p::int(1 << 30)))?;
            Ok(s(&cx.at(l, p::pct("%d GB", &[n]))?))
        }
        _ => Ok(s("?")),
    }
}

fn print(cx: &mut Cx, line: u32, fmt: &str, args: &[Value]) -> Result<(), Raise> {
    let t = cx.at(line, p::pct(fmt, args))?;
    cx.print(&t);
    Ok(())
}

/// `_compute_render_health`.
fn health(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const A: &str = "\n_compute_render_health() {";
    let v = render::load(cx, body, line!(A, "v = json.load(sys.stdin)"))?;
    let l = line!(A, "f = v.get(\"facts\") or {}");
    let f = p::or(cx.at(l, p::get(&v, "facts"))?, Value::Object(vec![]));
    let seen = cx.at(
        line!(A, "seen = v.get(\"lastSeen\")"),
        p::get(&v, "lastSeen"),
    )?;
    let l = line!(A, "print(\"%s  %s%s\" % (v[\"node\"]");
    let node = cx.at(l, p::idx(&v, "node"))?;
    let online = p::truthy(&cx.at(l, p::idx(&v, "online"))?);
    let state = if online { "online" } else { "OFFLINE" };
    let since = if p::truthy(&cx.at(l, p::idx(&v, "online"))?) || !p::truthy(&seen) {
        String::new()
    } else {
        format!(
            " since {}",
            cx.at(l, p::strftime("%H:%M", &seen, cx.now()))?
        )
    };
    print(cx, l, "%s  %s%s", &[node, s(state), s(&since)])?;
    let l = line!(A, "print(\"  k2-node %s, remote admin");
    let args = [
        p::or(cx.at(l, p::get(&v, "nodeVersion"))?, s("?")),
        p::or(cx.at(l, p::get(&v, "remoteAdmin"))?, s("unknown")),
        cx.at(l, p::get_or(&v, "pendingApprovals", p::int(0)))?,
    ];
    print(
        cx,
        l,
        "  k2-node %s, remote admin %s, pending approvals %s",
        &args,
    )?;
    if p::truthy(&cx.at(line!(A, "if v.get(\"needsPin\"):"), p::get(&v, "needsPin"))?) {
        cx.print(
            "  NEEDS ONE-TIME PIN: admin commands are refused until someone runs, at that machine:",
        );
        let l = line!(A, "print(\"      %s\" % (v.get(\"pinCommand\")");
        let pin = p::or(
            cx.at(l, p::get(&v, "pinCommand"))?,
            s("sudo k2-node admin pin"),
        );
        print(cx, l, "      %s", &[pin])?;
    }
    if !p::truthy(&f) {
        cx.print("  no health report yet (needs a k2-node with health)");
    } else {
        facts(cx, &v, &f)?;
    }
    let l = line!(A, "u = v.get(\"update\")");
    let u = cx.at(l, p::get(&v, "update"))?;
    if p::truthy(&u) {
        let l = line!(A, "if u: print(\"  update        %s %s%s\"");
        let state = cx.at(l, p::get(&u, "state"))?;
        let version = p::or(cx.at(l, p::get(&u, "version"))?, s(""));
        let reason = if p::truthy(&cx.at(l, p::get(&u, "reason"))?) {
            let r = cx.at(l, p::idx(&u, "reason"))?;
            cx.at(l, p::add(&s(" ("), &r).and_then(|t| p::add(&t, &s(")"))))?
        } else {
            s("")
        };
        print(cx, l, "  update        %s %s%s", &[state, version, reason])?;
    }
    let a = cx.at(
        line!(A, "a = v.get(\"alertAfterSecs\")"),
        p::get(&v, "alertAfterSecs"),
    )?;
    let l = line!(A, "print(\"  alerts        %s, to %s\"");
    let when = if p::truthy(&a) {
        let m = cx.at(l, p::floordiv(&a, &p::int(60)))?;
        s(&cx.at(l, p::pct("offline after %dm", &[m]))?)
    } else {
        s("no offline alert")
    };
    let to = p::or(
        cx.at(l, p::get(&v, "alertsTo"))?,
        s(concat!(
            "nobody (",
            cli_name!(),
            " compute node alerts --to <ws>)"
        )),
    );
    print(cx, l, "  alerts        %s, to %s", &[when, to])
}

/// The facts' lines of `_compute_render_health`.
fn facts(cx: &mut Cx, v: &Value, f: &Value) -> Result<(), Raise> {
    const A: &str = "\n_compute_render_health() {";
    let l = line!(A, "if f.get(\"uptime_secs\") is not None:");
    let up = cx.at(l, p::get(f, "uptime_secs"))?;
    if !p::is_none(&up) {
        let u = cx.at(l, p::idx(f, "uptime_secs"))?;
        let days = cx.at(l, p::floordiv(&u, &p::int(86400)))?;
        let u = cx.at(l, p::idx(f, "uptime_secs"))?;
        let hours = cx.at(
            l,
            p::rem(&u, &p::int(86400)).and_then(|r| p::floordiv(&r, &p::int(3600))),
        )?;
        print(cx, l, "  uptime        %dd %dh", &[days, hours])?;
    }
    let l = line!(A, "for d in f.get(\"disks\") or []:");
    let disks = cx.at(l, p::get(f, "disks").and_then(p::iter_or_empty))?;
    for d in disks {
        let name = p::or(cx.at(l, p::get(&d, "path"))?, cx.at(l, p::get(&d, "name"))?);
        let free = cx.at(l, p::idx(&d, "free_bytes"))?;
        let free = gb(cx, l, &free)?;
        let total = cx.at(l, p::idx(&d, "total_bytes"))?;
        let total = gb(cx, l, &total)?;
        print(cx, l, "  disk %-8s %s free of %s", &[name, free, total])?;
    }
    let l = line!(A, "if f.get(\"mem_total_bytes\"):");
    if p::truthy(&cx.at(l, p::get(f, "mem_total_bytes"))?) {
        let avail = cx.at(l, p::get(f, "mem_available_bytes"))?;
        let avail = gb(cx, l, &avail)?;
        let total = cx.at(l, p::idx(f, "mem_total_bytes"))?;
        let total = gb(cx, l, &total)?;
        let pressure = p::or(cx.at(l, p::get(f, "memory_pressure"))?, s("?"));
        print(
            cx,
            l,
            "  memory        %s available of %s, pressure %s",
            &[avail, total, pressure],
        )?;
    }
    let l = line!(A, "if f.get(\"disk_encryption\"):");
    if p::truthy(&cx.at(l, p::get(f, "disk_encryption"))?) {
        let enc = cx.at(l, p::idx(f, "disk_encryption"))?;
        let tail = if p::eq_s(&cx.at(l, p::idx(f, "disk_encryption"))?, "on") {
            " (a power cut needs someone at the machine)"
        } else {
            ""
        };
        print(cx, l, "  FileVault     %s%s", &[enc, s(tail)])?;
    }
    let l = line!(A, "ar = (f.get(\"sleep\") or {})");
    let sleep = p::or(cx.at(l, p::get(f, "sleep"))?, Value::Object(vec![]));
    let ar = cx.at(l, p::get(&sleep, "autorestart"))?;
    if !p::is_none(&ar) {
        let on = if p::eq(&ar, &p::int(1)) { "on" } else { "off" };
        print(
            cx,
            line!(A, "if ar is not None:"),
            "  autorestart   %s",
            &[s(on)],
        )?;
    }
    let l = line!(A, "up = f.get(\"updates\") or {}");
    let up = p::or(cx.at(l, p::get(f, "updates"))?, Value::Object(vec![]));
    let l = line!(A, "if up.get(\"pending\") is not None:");
    if !p::is_none(&cx.at(l, p::get(&up, "pending"))?) {
        let pending = cx.at(l, p::idx(&up, "pending"))?;
        let sec = p::or(cx.at(l, p::get(&up, "security"))?, p::int(0));
        print(
            cx,
            l,
            "  updates       %s pending (%s security)",
            &[pending, sec],
        )?;
    }
    let l = line!(A, "if up.get(\"reboot_required\"):");
    if p::truthy(&cx.at(l, p::get(&up, "reboot_required"))?) {
        cx.print("  reboot        required");
    }
    let l = line!(A, "ex = f.get(\"executor\") or {}");
    let ex = p::or(cx.at(l, p::get(f, "executor"))?, Value::Object(vec![]));
    let l = line!(A, "if ex and not ex.get(\"reachable\"):");
    if p::truthy(&ex) && !p::truthy(&cx.at(l, p::get(&ex, "reachable"))?) {
        cx.print("  executor      not answering");
    }
    let l = line!(A, "if ex.get(\"wall_restart_pending\"):");
    if p::truthy(&cx.at(l, p::get(&ex, "wall_restart_pending"))?) {
        let node = cx.at(l, p::idx(v, "node"))?;
        let fmt = concat!(
            "  wall restart  owed: a restart onto the job wall is owed; run wall.enable again (or approve the held restart): ",
            cli_name!(),
            " compute node wall %s"
        );
        print(cx, l, fmt, &[node])?;
    }
    let l = line!(A, "for s in f.get(\"stuck_jobs\") or []:");
    for j in cx.at(l, p::get(f, "stuck_jobs").and_then(p::iter_or_empty))? {
        let id = cx.at(
            l,
            p::idx(&j, "job_id").and_then(|x| p::slice(&x, None, Some(8))),
        )?;
        let quiet = cx.at(
            l,
            p::get_or(&j, "quiet_secs", p::int(0)).and_then(|q| p::floordiv(&q, &p::int(60))),
        )?;
        print(cx, l, "  stuck job     %s (quiet %dm)", &[id, quiet])?;
    }
    let lo = cx.at(
        line!(A, "lo = f.get(\"leftovers\")"),
        p::get(f, "leftovers"),
    )?;
    if p::is_dict(&lo) {
        let l = line!(A, "print(\"  leftovers     %s\"");
        let what = if !p::truthy(&cx.at(l, p::get(&lo, "count"))?) {
            s("none (no job-user process outlives its job)")
        } else {
            let n = cx.at(l, p::idx(&lo, "count"))?;
            s(&cx.at(
                l,
                p::pct(
                    "%d process(es) of the job user with no live job (not killed by the check)",
                    &[n],
                ),
            )?)
        };
        print(cx, l, "  leftovers     %s", &[what])?;
        let l = line!(A, "for p in lo.get(\"procs\") or []:");
        for pr in cx.at(l, p::get(&lo, "procs").and_then(p::iter_or_empty))? {
            let pid = cx.at(l, p::get(&pr, "pid"))?;
            let age = cx.at(l, p::get_or(&pr, "age_secs", s("?")))?;
            let from = if p::truthy(&cx.at(l, p::get(&pr, "job_id"))?) {
                let id = cx.at(
                    l,
                    p::idx(&pr, "job_id").and_then(|x| p::slice(&x, None, Some(8))),
                )?;
                s(&cx.at(l, p::pct(", from job %s", &[id]))?)
            } else {
                s("")
            };
            let cmd = p::or(cx.at(l, p::get(&pr, "cmd"))?, s(""));
            let cmd = cx.at(l, p::slice(&cmd, None, Some(100)))?;
            print(
                cx,
                l,
                "  leftover      pid %s, %s s old%s: %s",
                &[pid, age, from, cmd],
            )?;
        }
    }
    let l = line!(A, "sec = f.get(\"security\")");
    let sec = cx.at(l, p::get(f, "security"))?;
    if p::truthy(&sec) {
        let l = line!(A, "print(\"  job wall      %s%s\"");
        let wall = cx.at(l, p::get(&sec, "wall"))?;
        let user = if p::eq_s(&cx.at(l, p::get(&sec, "wall"))?, "job_user")
            && p::truthy(&cx.at(l, p::get(&sec, "job_user"))?)
        {
            let u = cx.at(l, p::idx(&sec, "job_user"))?;
            s(&cx.at(l, p::pct(" (jobs run as %s)", &[u]))?)
        } else {
            s("")
        };
        print(cx, l, "  job wall      %s%s", &[wall, user])?;
        let l = line!(A, "print(\"  network none  %s\"");
        let net = if p::truthy(&cx.at(l, p::get(&sec, "network_none"))?) {
            "enforced"
        } else {
            "not enforced: network = none takes no jobs here"
        };
        print(cx, l, "  network none  %s", &[s(net)])?;
        let l = line!(A, "for h in sec.get(\"open_homes\") or []:");
        for h in cx.at(l, p::get(&sec, "open_homes").and_then(p::iter_or_empty))? {
            print(cx, l, "  open home     %s", &[h])?;
        }
        let l = line!(A, "for n in sec.get(\"notes\") or []:");
        for n in cx.at(l, p::get(&sec, "notes").and_then(p::iter_or_empty))? {
            print(cx, l, "  WARNING       %s", &[n])?;
        }
    } else {
        cx.print("  job wall      none reported (k2-node before 0.46.3: jobs run as the node user and can read its key)");
    }
    Ok(())
}
