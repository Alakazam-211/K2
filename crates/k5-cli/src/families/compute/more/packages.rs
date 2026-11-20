//! `compute node packages|pkg ...`, as `cmd_compute_node_packages`
//! answers it (`cli/src/compute-node.sh`):
//!
//! - `packages` or `packages help`: the page (`cli/text/help/compute/node/packages.txt`);
//! - `packages <node> [--json]`: what the node can install for this server;
//! - `packages request <nodes> <names>... --why T [--job ID]`, then a
//!   held request's lines (exit 2);
//! - `packages requests|list|ls [--node N] [--state S] [--limit N]`;
//! - `packages approve|deny <request> [--add] [--note T]` (the owner,
//!   outside a session);
//! - `packages policy <node> [<server>] [key=value…] [--reason T]`.
//!
//! What goes to the shell: a `max_install_mb` value with a character past
//! ASCII (Python's `isdigit` reads other scripts' digits).

use super::super::admin::node::flows;
use super::super::lines::py_line;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{dumps, in_session, object, print_body, str_value, Cp, Words};
use crate::exit::Exit;
use crate::families::{py_space, raised, Raise, Shown};
use crate::out;
use crate::out::json::Value;
use crate::transport::encode::quote;
use crate::transport::permission;
use k2_client::{brand::CLI_NAME, cli_name, product_name};
use std::path::Path;

const SRC: &str = include_str!("../../../../../../cli/src/compute-node.sh");

macro_rules! line {
    ($after:expr, $marker:literal) => {
        const { py_line(SRC, $after, $marker) }
    };
}

pub const REQUEST_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node packages request <node>[,<node>…] <name>[,<name>…] --why \"…\" [--job ID]"
);
const REQUESTS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node packages requests [--node N] [--state pending|approved|installed|denied|failed|expired|all] [--limit N] [--json]"
);
const POLICY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node packages policy <node> [<server>] [level=list|profiles|official] [profiles=a,b] [max_install_mb=N] [--reason \"…\"]"
);
const FIRST_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node packages <node> | request | requests | approve | deny | policy … (",
    cli_name!(),
    " compute node packages --help)"
);
const SHOW_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node packages <node> [--json]"
);
const OWNER_ONLY: &str = concat!(
    "{\"error\":\"owner_only\",\"hint\":\"only the owner or an Admin decides package requests, in person: the card in chat, Settings → Cores → Compute → Machine settings → Waiting for you, or this command from a terminal outside ",
    product_name!(),
    ". An agent can never approve.\"}"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Page,
    Show {
        node: String,
        json: bool,
    },
    Request {
        nodes: Value,
        names: Value,
        why: String,
        job: String,
        json: bool,
    },
    Requests {
        query: String,
        json: bool,
    },
    Decide {
        approve: bool,
        id: String,
        add: bool,
        note: String,
        json: bool,
    },
    PolicyShow {
        node: String,
        server: String,
        json: bool,
    },
    PolicySet {
        body: String,
        json: bool,
    },
}

fn usage(line: impl Into<String>) -> Option<Words<Ask>> {
    Some(Words::Usage(line.into()))
}

/// `[x.strip() for x in a.split(",") if x.strip()]`.
fn parts(a: &str) -> Vec<Value> {
    a.split(',')
        .map(|x| x.trim_matches(py_space))
        .filter(|x| !x.is_empty())
        .map(str_value)
        .collect()
}

/// The words after `packages` (or `pkg`). `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let first = words.first().copied().unwrap_or("");
    let json = words.contains(&"--json");
    let rest = words.get(1..).unwrap_or(&[]);
    match first {
        "" | "help" | "--help" | "-h" => Some(Words::Ask(Ask::Page)),
        "request" => {
            let (mut nodes, mut names, mut why, mut job) = ("", vec![], "", "");
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    f @ ("--why" | "--job") => {
                        if i + 2 > rest.len() {
                            return Some(Words::ShiftFailed);
                        }
                        if f == "--why" {
                            why = rest[i + 1];
                        } else {
                            job = rest[i + 1];
                        }
                        i += 2;
                        continue;
                    }
                    "--json" => {}
                    w if w.starts_with('-') => {
                        return usage(format!("{REQUEST_USAGE} (unknown flag '{w}')"))
                    }
                    w if nodes.is_empty() => nodes = w,
                    w => names.push(w),
                }
                i += 1;
            }
            if nodes.is_empty() || names.is_empty() {
                return usage(REQUEST_USAGE);
            }
            if why.is_empty() {
                return usage(format!("{REQUEST_USAGE} (--why: one line the owner reads on the card, e.g. \"tauri build needs webkit2gtk\")"));
            }
            Some(Words::Ask(Ask::Request {
                nodes: Value::Array(parts(nodes)),
                names: Value::Array(names.iter().flat_map(|a| parts(a)).collect()),
                why: why.into(),
                job: job.into(),
                json,
            }))
        }
        "requests" | "list" | "ls" => {
            let mut q = String::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    f @ ("--node" | "--state" | "--limit") => {
                        if i + 2 > rest.len() {
                            return Some(Words::ShiftFailed);
                        }
                        q.push_str(&format!("&{}={}", &f[2..], quote(rest[i + 1])));
                        i += 2;
                        continue;
                    }
                    "--json" => {}
                    _ => return usage(REQUESTS_USAGE),
                }
                i += 1;
            }
            Some(Words::Ask(Ask::Requests {
                query: q.trim_start_matches('&').to_string(),
                json,
            }))
        }
        "approve" | "deny" => {
            let line = if first == "approve" {
                format!("usage: {CLI_NAME} compute node packages approve <request> [--add] [--note \"…\"]")
            } else {
                format!("usage: {CLI_NAME} compute node packages deny <request> [--note \"…\"]")
            };
            let (mut id, mut note, mut add) = ("", "", false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--note" | "--why" => {
                        if i + 2 > rest.len() {
                            return Some(Words::ShiftFailed);
                        }
                        note = rest[i + 1];
                        i += 2;
                        continue;
                    }
                    "--add" => add = true,
                    "--json" => {}
                    w if w.starts_with('-') => {
                        return usage(format!("{line} (unknown flag '{w}')"))
                    }
                    w => {
                        if !id.is_empty() {
                            return usage(format!("{line} (one request)"));
                        }
                        id = w;
                    }
                }
                i += 1;
            }
            if id.is_empty() {
                return usage(line);
            }
            if first == "deny" && add {
                return usage("usage: --add goes with approve (it puts the names on the server's approved list)");
            }
            Some(Words::Ask(Ask::Decide {
                approve: first == "approve",
                id: id.into(),
                add,
                note: note.into(),
                json,
            }))
        }
        "policy" => policy(rest, json),
        w if w.starts_with('-') => usage(FIRST_USAGE),
        node => {
            if rest.iter().any(|a| *a != "--json") {
                return usage(SHOW_USAGE);
            }
            Some(Words::Ask(Ask::Show {
                node: node.into(),
                json,
            }))
        }
    }
}

fn policy(rest: &[&str], json: bool) -> Option<Words<Ask>> {
    let (mut node, mut server, mut reason, mut pairs) = ("", "", "", vec![]);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--reason" => {
                if i + 2 > rest.len() {
                    return Some(Words::ShiftFailed);
                }
                reason = rest[i + 1];
                i += 2;
                continue;
            }
            "--json" => {}
            w if w.starts_with('-') => {
                return usage(format!("{POLICY_USAGE} (unknown flag '{w}')"))
            }
            w if w.contains('=') => pairs.push(w),
            w if node.is_empty() => node = w,
            w if server.is_empty() => server = w,
            _ => return usage(POLICY_USAGE),
        }
        i += 1;
    }
    if node.is_empty() {
        return usage(POLICY_USAGE);
    }
    if pairs.is_empty() {
        return Some(Words::Ask(Ask::PolicyShow {
            node: node.into(),
            server: server.into(),
            json,
        }));
    }
    let mut out = vec![
        ("node", str_value(node)),
        (
            "server",
            str_value(if server.is_empty() { "own" } else { server }),
        ),
    ];
    if !reason.is_empty() {
        out.push(("reason", str_value(reason)));
    }
    // A key given twice keeps its first place and its last value.
    let mut put = |k: &'static str, v: Value| match out.iter_mut().find(|(key, _)| *key == k) {
        Some(slot) => slot.1 = v,
        None => out.push((k, v)),
    };
    for kv in pairs {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "level" => {
                if !matches!(v, "list" | "profiles" | "official") {
                    return usage("usage: level=list|profiles|official");
                }
                put("level", str_value(v));
            }
            "profiles" => put("profiles", Value::Array(parts(v))),
            "max_install_mb" => {
                if !v.is_ascii() {
                    return None;
                }
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return usage("usage: max_install_mb=N (megabytes per request, 1 to 65536)");
                }
                let n = v.trim_start_matches('0');
                put(
                    "maxInstallMb",
                    Value::Int(if n.is_empty() { "0" } else { n }.into()),
                );
            }
            k => {
                return usage(format!(
                    "usage: {k}: the keys are level, profiles and max_install_mb"
                ))
            }
        }
    }
    Some(Words::Ask(Ask::PolicySet {
        body: dumps(&object(out)),
        json,
    }))
}

fn shown(mut cx: Cx, r: Result<(), Raise>) -> Shown {
    match r {
        Ok(()) => Shown::out(std::mem::take(&mut cx.out)),
        Err(e) => raised(std::mem::take(&mut cx.out), String::new(), &e),
    }
}

fn print(cx: &mut Cx, line: u32, fmt: &str, args: &[Value]) -> Result<(), Raise> {
    let t = cx.at(line, p::pct(fmt, args))?;
    cx.print(&t);
    Ok(())
}

/// `" ".join(x.get(key) or [])`.
fn joined(cx: &Cx, l: u32, x: &Value, key: &str) -> Result<Value, Raise> {
    let v = p::or(cx.at(l, p::get(x, key))?, Value::Array(vec![]));
    Ok(s(&cx.at(l, p::join(" ", &v))?))
}

/// `(", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else ""`.
fn size_mb(cx: &Cx, l: u32, d: &Value) -> Result<Value, Raise> {
    if p::is_none(&cx.at(l, p::get(d, "size_mb"))?) {
        return Ok(s(""));
    }
    let mb = cx.at(l, p::idx(d, "size_mb"))?;
    Ok(s(&cx.at(l, p::pct(", %s MB", &[mb]))?))
}

/// The requests and what they print. `sock`: the vault socket.
pub fn send(cp: &mut Cp<'_>, a: &Ask, sock: &Path) -> Exit {
    match a {
        Ask::Page => {
            let page = crate::text::help("compute/node/packages")
                .expect("the node packages page is embedded");
            match out::stdout(page) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Ask::Show { node, json } => {
            let q = format!("node={}", quote(node));
            let answer = match cp.call(false, "/cli/compute/nodes/packages", &q, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if *json {
                return print_body(&answer);
            }
            let mut cx = Cx::new(0);
            let r = show(&mut cx, &answer);
            shown(cx, r).print()
        }
        Ask::Request {
            nodes,
            names,
            why,
            job,
            json,
        } => {
            let ws = cp.ws("");
            let mut fields = vec![("nodes", nodes.clone()), ("names", names.clone())];
            for (k, v) in [("why", why.as_str()), ("job", job), ("workspace", &ws)] {
                if !v.is_empty() {
                    fields.push((k, str_value(v)));
                }
            }
            let body = dumps(&object(fields));
            let answer = match cp.call(true, "/cli/compute/nodes/packages/request", &body, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if *json {
                let e = print_body(&answer);
                if e != Exit::Ok {
                    return e;
                }
                return held_exit(&answer).unwrap_or(Exit::Ok);
            }
            let mut cx = Cx::new(0);
            let r = requested(&mut cx, &answer);
            let e = shown(cx, r).print();
            if e != Exit::Ok {
                return e;
            }
            held_exit(&answer).unwrap_or(Exit::Ok)
        }
        Ask::Requests { query, json } => {
            let tcp = !in_session(&cp.client.auth);
            let answer = match cp.call(false, "/cli/compute/nodes/packages/requests", query, tcp) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if *json {
                return print_body(&answer);
            }
            let mut cx = Cx::new(p::now());
            let r = requests(&mut cx, &answer);
            shown(cx, r).print()
        }
        Ask::Decide {
            approve,
            id,
            add,
            note,
            json,
        } => decide(cp, *approve, id, *add, note, *json),
        Ask::PolicyShow { node, server, json } => {
            let q = format!("node={}", quote(node));
            let answer = match cp.call(false, "/cli/compute/nodes/packages", &q, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if *json {
                return print_body(&answer);
            }
            let mut cx = Cx::new(0);
            let r = policy_show(&mut cx, &answer, server);
            shown(cx, r).print()
        }
        Ask::PolicySet { body, json } => {
            let answer = match cp.call(true, "/cli/compute/nodes/packages/policy", body, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if flows::held(&answer) {
                return flows::held_or_approve(cp, sock, &answer);
            }
            if *json {
                return print_body(&answer);
            }
            let mut cx = Cx::new(0);
            let r = (|| {
                let v = render::load(&cx, &answer, 1)?;
                let node = cx.at(1, p::get(&v, "node"))?;
                let note = p::or(cx.at(1, p::get(&v, "note"))?, s("ok"));
                print(&mut cx, 1, "%s: %s", &[node, note])
            })();
            shown(cx, r).print()
        }
    }
}

/// `grep -q '"pkgHeld": *true'` on the answer.
fn pkg_held(body: &[u8]) -> bool {
    let key = b"\"pkgHeld\":";
    body.windows(key.len()).enumerate().any(|(i, w)| {
        w == key && {
            let rest = &body[i + key.len()..];
            let n = rest.iter().take_while(|b| **b == b' ').count();
            rest[n..].starts_with(b"true")
        }
    })
}

/// `_compute_pkg_held_exit`: `None` when nothing is held (it returns 0).
pub fn held_exit(body: &[u8]) -> Option<Exit> {
    if !pkg_held(body) {
        return None;
    }
    let cx = Cx::new(0);
    let id = flows::captured(render::load(&cx, body, 1).and_then(|v| {
        let r = p::or(cx.at(1, p::get(&v, "request"))?, Value::Object(vec![]));
        let id = cx.at(1, p::get(&r, "id"))?;
        Ok(p::or(id, s("")))
    }));
    let id = match id {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    out::stderr_line(&format!("held for the owner: package request {id}"));
    out::stderr_line(&format!(
        concat!(
            "  the owner approves it on the card, in Settings → Cores → Compute → Machine settings → Waiting for you, or outside ",
            product_name!(),
            ": {} compute node packages approve {}"
        ),
        CLI_NAME, id
    ));
    if permission::note(body).is_some_and(|n| !n.lines().is_empty()) {
        out::stderr_line("  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card.");
    }
    out::stderr_line("  you get a [compute] message when it's decided; then run your job again");
    Some(Exit::Usage)
}

fn decide(cp: &mut Cp<'_>, approve: bool, id: &str, add: bool, note: &str, json: bool) -> Exit {
    if in_session(&cp.client.auth) {
        out::stderr_line(OWNER_ONLY);
        return Exit::Refused;
    }
    let list = match cp.call(
        false,
        "/cli/compute/nodes/packages/requests",
        "state=pending&limit=500",
        true,
    ) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut cx = Cx::new(0);
    let mut err = String::new();
    let full = match batch(&mut cx, &mut err, &list, id) {
        Ok(Some(b)) => {
            flows::stderr_text(&err);
            b
        }
        Ok(None) => {
            flows::stderr_text(&err);
            id.to_string()
        }
        Err(e) => {
            flows::stderr_text(&err);
            flows::stderr_text(&e.text());
            id.to_string()
        }
    };
    let mut fields = vec![
        ("request", str_value(&full)),
        ("approve", Value::Bool(approve)),
    ];
    if add {
        fields.push(("add", Value::Bool(true)));
    }
    if !note.is_empty() {
        fields.push(("note", str_value(note)));
    }
    let body = dumps(&object(fields));
    let answer = match cp.call(true, "/cli/compute/nodes/packages/decide", &body, true) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if json {
        return print_body(&answer);
    }
    let mut cx = Cx::new(0);
    const A: &str = "/cli/compute/nodes/packages/decide";
    let r = (|| {
        let v = render::load(&cx, &answer, line!(A, "v = json.load(sys.stdin)"))?;
        let l = line!(A, "for r in v.get(\"rows\") or []:");
        for r in cx.at(l, p::get(&v, "rows").and_then(p::iter_or_empty))? {
            let l = line!(A, "print(\"%s %s on %s%s\"");
            let state = cx.at(l, p::get(&r, "state"))?;
            let req = cx.at(l, p::get(&r, "request"))?;
            let node = cx.at(l, p::get(&r, "node"))?;
            let tail = if p::truthy(&cx.at(l, p::get(&r, "note"))?) {
                let n = cx.at(l, p::idx(&r, "note"))?;
                cx.at(l, p::add(&s(": "), &n))?
            } else {
                s("")
            };
            print(&mut cx, l, "%s %s on %s%s", &[state, req, node, tail])?;
        }
        Ok(())
    })();
    shown(cx, r).print()
}

/// The `full` Python of approve|deny: the one batch matching `want`
/// (`Ok(None)`: `sys.exit(1)`), its rows' lines in `err` (stderr).
fn batch(cx: &mut Cx, err: &mut String, body: &[u8], want: &str) -> Result<Option<String>, Raise> {
    const A: &str = "local full\n";
    let l = line!(A, "rows = [r for r in");
    let v = render::load(cx, body, l)?;
    let all = p::or(cx.at(l, p::get(&v, "requests"))?, Value::Array(vec![]));
    let mut rows = vec![];
    for r in cx.at(l, p::iter(&all))? {
        if !p::eq_s(&cx.at(l, p::get(&r, "origin"))?, "sent") {
            rows.push(r);
        }
    }
    let l = line!(A, "ids = sorted(");
    let mut ids: Vec<Value> = vec![];
    for r in &rows {
        let b = cx.at(l, p::idx(r, "batchId"))?;
        let hit = p::eq_s(&b, want)
            || p::eq_s(&cx.at(l, p::idx(r, "id"))?, want)
            || cx.at(
                l,
                p::idx(r, "batchId").and_then(|x| p::startswith(&x, want)),
            )?;
        if hit && !ids.iter().any(|x| p::eq(x, &b)) {
            if matches!(b, Value::Array(_) | Value::Object(_)) {
                return Err(cx.raise(l, format!("TypeError: unhashable type: '{}'", p::tname(&b))));
            }
            ids.push(b);
        }
    }
    // `sorted` of a set mixing strings and numbers raises.
    let strs = ids.iter().filter(|x| matches!(x, Value::Str(_))).count();
    if strs != 0 && strs != ids.len() {
        return Err(cx.raise(
            l,
            "TypeError: '<' not supported between instances of 'int' and 'str'".to_string(),
        ));
    }
    if ids.len() != 1 {
        return Ok(None);
    }
    let b = ids.remove(0);
    for r in &rows {
        let l = line!(A, "if r[\"batchId\"] != b:");
        if !p::eq(&cx.at(l, p::idx(r, "batchId"))?, &b) {
            continue;
        }
        let l = line!(A, "d = r.get(\"dry\") or {}");
        let d = p::or(cx.at(l, p::get(r, "dry"))?, Value::Object(vec![]));
        let l = line!(A, "sys.stderr.write(\"  %s on %s (%s), for %s%s");
        let names = joined(cx, l, r, "names")?;
        let node = cx.at(l, p::get(r, "node"))?;
        let host = p::or(cx.at(l, p::get(r, "host"))?, s("?"));
        let asked = p::or(cx.at(l, p::get(r, "askedBy"))?, s("?"));
        let via = if p::eq_s(&cx.at(l, p::get(r, "origin"))?, "fwd") {
            let sv = cx.at(l, p::idx(r, "server"))?;
            cx.at(l, p::add(&s(" via "), &sv))?
        } else {
            s("")
        };
        err.push_str(&cx.at(
            l,
            p::pct(
                "  %s on %s (%s), for %s%s\n",
                &[names, node, host, asked, via],
            ),
        )?);
        let l = line!(A, "if r.get(\"why\"):");
        if p::truthy(&cx.at(l, p::get(r, "why"))?) {
            let w = cx.at(l, p::idx(r, "why"))?;
            err.push_str(&cx.at(l, p::pct("    why: %s\n", &[w]))?);
        }
        let l = line!(A, "if d.get(\"installs\"):");
        if p::truthy(&cx.at(l, p::get(&d, "installs"))?) {
            let ins = cx.at(l, p::idx(&d, "installs"))?;
            let n = p::int(cx.at(l, p::len(&ins))? as i128);
            let mb = size_mb(cx, l, &d)?;
            let list = s(&cx.at(l, p::join(" ", &ins))?);
            err.push_str(&cx.at(
                l,
                p::pct("    installs %d package(s)%s: %s\n", &[n, mb, list]),
            )?);
        }
    }
    Ok(Some(p::str_of(&b).trim_end_matches('\n').to_string()))
}

fn requested(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const A: &str = "/cli/compute/nodes/packages/request \"$(";
    let v = render::load(cx, body, line!(A, "v = json.load(sys.stdin)"))?;
    let l = line!(A, "r = v.get(\"request\") or {}");
    let r = p::or(cx.at(l, p::get(&v, "request"))?, Value::Object(vec![]));
    let l = line!(A, "print(\"request %s: %s\"");
    let id = cx.at(l, p::get(&r, "id"))?;
    let summary = p::or(cx.at(l, p::get(&r, "summary"))?, s(""));
    print(cx, l, "request %s: %s", &[id, summary])?;
    let l = line!(A, "for row in r.get(\"rows\") or []:");
    for row in cx.at(l, p::get(&r, "rows").and_then(p::iter_or_empty))? {
        let l = line!(A, "d = row.get(\"dry\") or {}");
        let d = p::or(cx.at(l, p::get(&row, "dry"))?, Value::Object(vec![]));
        let mut extra = s("");
        let l = line!(A, "if d.get(\"installs\"):");
        if p::truthy(&cx.at(l, p::get(&d, "installs"))?) {
            let l = line!(A, "extra = \" · installs %d package(s)%s\"");
            let ins = cx.at(l, p::idx(&d, "installs"))?;
            let n = p::int(cx.at(l, p::len(&ins))? as i128);
            let mb = size_mb(cx, l, &d)?;
            extra = s(&cx.at(l, p::pct(" · installs %d package(s)%s", &[n, mb]))?);
        }
        let l = line!(A, "if d.get(\"refusal\"):");
        if p::truthy(&cx.at(l, p::get(&d, "refusal"))?) {
            let l = line!(A, "extra += \" · would be refused: \"");
            let why = cx.at(l, p::idx(&d, "refusal"))?;
            let t = cx.at(l, p::add(&s(" · would be refused: "), &why))?;
            extra = cx.at(l, p::add(&extra, &t))?;
        }
        let l = line!(A, "print(\"  %-14s %-9s %s%s\"");
        let node = cx.at(l, p::get(&row, "node"))?;
        let state = cx.at(l, p::get(&row, "state"))?;
        let names = joined(cx, l, &row, "names")?;
        print(cx, l, "  %-14s %-9s %s%s", &[node, state, names, extra])?;
    }
    let l = line!(A, "for c in v.get(\"cant\") or []:");
    for c in cx.at(l, p::get(&v, "cant").and_then(p::iter_or_empty))? {
        let t = cx.at(l, p::add(&s("  not asked: "), &c))?;
        cx.print(&p::str_of(&t));
    }
    let l = line!(A, "if v.get(\"hint\"):");
    if p::truthy(&cx.at(l, p::get(&v, "hint"))?) {
        let h = cx.at(l, p::idx(&v, "hint"))?;
        cx.print(&p::str_of(&h));
    }
    Ok(())
}

/// `ago(t)`.
fn ago(cx: &Cx, l: u32, t: &Value) -> Result<Value, Raise> {
    let t = p::or(t.clone(), p::int(0));
    let t = cx.at(l, p::to_int(&t))?;
    let d = (i128::from(cx.now()) - t).max(0);
    Ok(s(&if d < 60 {
        "now".to_string()
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }))
}

fn requests(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const A: &str = "packages/requests \"${q#&}\" tcp\n";
    let v = render::load(cx, body, line!(A, "v = json.load(sys.stdin)"))?;
    let rows = p::or(
        cx.at(
            line!(A, "rows = v.get(\"requests\") or []"),
            p::get(&v, "requests"),
        )?,
        Value::Array(vec![]),
    );
    if !p::truthy(&rows) {
        cx.print("no package requests");
    }
    let l = line!(A, "for r in rows:");
    let rows = cx.at(l, p::iter(&rows))?;
    for r in &rows {
        let l = line!(A, "origin = {\"own\"");
        let server = cx.at(l, p::get(r, "server"))?;
        let asked = s(&cx.at(l, p::pct("asked %s", std::slice::from_ref(&server)))?);
        let from = s(&cx.at(l, p::pct("from %s", &[server]))?);
        let o = cx.at(l, p::get(r, "origin"))?;
        let default = p::or(cx.at(l, p::get(r, "origin"))?, s(""));
        let origin = match &o {
            Value::Array(_) | Value::Object(_) => {
                return Err(cx.raise(l, format!("TypeError: unhashable type: '{}'", p::tname(&o))))
            }
            x if p::eq_s(x, "own") => s(""),
            x if p::eq_s(x, "sent") => asked,
            x if p::eq_s(x, "fwd") => from,
            _ => default,
        };
        let l = line!(A, "print(\"%-21s %-9s %-14s %-8s %s%s\"");
        let batch = cx.at(l, p::get(r, "batchId"))?;
        let state = cx.at(l, p::get(r, "state"))?;
        let node = cx.at(l, p::get(r, "node"))?;
        let created = cx.at(l, p::get(r, "createdAt"))?;
        let when = cx.call(l, "ago", |cx| {
            ago(cx, line!(A, "d = max(int(time.time())"), &created)
        })?;
        let names = joined(cx, l, r, "names")?;
        let tail = if p::truthy(&origin) {
            cx.at(
                l,
                p::add(&s("  ("), &origin).and_then(|t| p::add(&t, &s(")"))),
            )?
        } else {
            s("")
        };
        print(
            cx,
            l,
            "%-21s %-9s %-14s %-8s %s%s",
            &[batch, state, node, when, names, tail],
        )?;
        let who = p::or(
            cx.at(
                line!(A, "who = r.get(\"askedBy\") or \"?\""),
                p::get(r, "askedBy"),
            )?,
            s("?"),
        );
        let l = line!(A, "print(\"%21s asked by %s%s%s\"");
        let job = if p::truthy(&cx.at(l, p::get(r, "jobId"))?) {
            let j = cx.at(
                l,
                p::idx(r, "jobId").and_then(|x| p::slice(&x, None, Some(8))),
            )?;
            cx.at(l, p::add(&s(" for job "), &j))?
        } else {
            s("")
        };
        let why = if p::truthy(&cx.at(l, p::get(r, "why"))?) {
            let w = cx.at(l, p::idx(r, "why"))?;
            cx.at(l, p::add(&s(": "), &w))?
        } else {
            s("")
        };
        print(cx, l, "%21s asked by %s%s%s", &[s(""), who, job, why])?;
        let d = p::or(
            cx.at(line!(A, "d = r.get(\"dry\") or {}"), p::get(r, "dry"))?,
            Value::Object(vec![]),
        );
        let l = line!(A, "if d.get(\"installs\"):");
        if p::truthy(&cx.at(l, p::get(&d, "installs"))?) {
            let l = line!(A, "print(\"%21s installs %d package(s)%s\"");
            let ins = cx.at(l, p::idx(&d, "installs"))?;
            let n = p::int(cx.at(l, p::len(&ins))? as i128);
            let mb = size_mb(cx, l, &d)?;
            print(cx, l, "%21s installs %d package(s)%s", &[s(""), n, mb])?;
        }
        let l = line!(A, "if d.get(\"refusal\"):");
        if p::truthy(&cx.at(l, p::get(&d, "refusal"))?) {
            let x = cx.at(l, p::idx(&d, "refusal"))?;
            print(cx, l, "%21s would be refused: %s", &[s(""), x])?;
        }
        let l = line!(A, "if r.get(\"note\") or r.get(\"decidedBy\"):");
        if p::truthy(&cx.at(l, p::get(r, "note"))?) || p::truthy(&cx.at(l, p::get(r, "decidedBy"))?)
        {
            let l = line!(A, "print(\"%21s %s%s\"");
            let by = if p::truthy(&cx.at(l, p::get(r, "decidedBy"))?) {
                let b = cx.at(l, p::idx(r, "decidedBy"))?;
                cx.at(l, p::add(&s("by "), &b).and_then(|t| p::add(&t, &s(" "))))?
            } else {
                s("")
            };
            let note = p::or(cx.at(l, p::get(r, "note"))?, s(""));
            print(cx, l, "%21s %s%s", &[s(""), by, note])?;
        }
    }
    let l = line!(A, "if v.get(\"canDecide\") and any(");
    if p::truthy(&cx.at(l, p::get(&v, "canDecide"))?) {
        let mut any = false;
        for r in &rows {
            if p::eq_s(&cx.at(l, p::get(r, "state"))?, "pending")
                && !p::eq_s(&cx.at(l, p::get(r, "origin"))?, "sent")
            {
                any = true;
                break;
            }
        }
        if any {
            cx.print(concat!(
                "decide: ",
                cli_name!(),
                " compute node packages approve|deny <request> [--add] [--note \"…\"]"
            ));
        }
    }
    Ok(())
}

fn policy_show(cx: &mut Cx, body: &[u8], server: &str) -> Result<(), Raise> {
    const A: &str = "K2_CP_SERVER=\"$server\" python3";
    let v = render::load(cx, body, line!(A, "v = json.load(sys.stdin)"))?;
    let l = line!(A, "p = v.get(\"packages\")");
    let pk = cx.at(l, p::get(&v, "packages"))?;
    if !p::is_dict(&pk) {
        let l = line!(A, "print(\"%s: %s\" % (v.get(\"node\"), v.get(\"note\")");
        let node = cx.at(l, p::get(&v, "node"))?;
        let note = p::or(cx.at(l, p::get(&v, "note"))?, s("no package status"));
        return print(cx, l, "%s: %s", &[node, note]);
    }
    let l = line!(A, "print(\"%s: %s for this server\"");
    let node = cx.at(l, p::get(&v, "node"))?;
    let pol = cx.at(l, p::get(&pk, "policy"))?;
    print(cx, l, "%s: %s for this server", &[node, pol])?;
    if !matches!(server, "" | "own") {
        cx.print("  (a node reports only the asking server's policy; every server's, at the machine: k2-node packages list)");
    }
    Ok(())
}

fn show(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const A: &str = "local node=\"$first\"";
    let v = render::load(cx, body, line!(A, "v = json.load(sys.stdin)"))?;
    let l = line!(A, "print(\"%s%s\" % (v.get(\"node\")");
    let node = cx.at(l, p::get(&v, "node"))?;
    let host = if p::truthy(&cx.at(l, p::get(&v, "hostLine"))?) {
        let h = cx.at(l, p::idx(&v, "hostLine"))?;
        cx.at(l, p::add(&s("  host: "), &h))?
    } else {
        s("")
    };
    print(cx, l, "%s%s", &[node, host])?;
    let pk = cx.at(line!(A, "p = v.get(\"packages\")"), p::get(&v, "packages"))?;
    if !p::is_dict(&pk) {
        let l = line!(A, "print(\"  \" + (v.get(\"note\")");
        let note = p::or(
            cx.at(l, p::get(&v, "note"))?,
            s("no package status from this node"),
        );
        let t = cx.at(l, p::add(&s("  "), &note))?;
        cx.print(&p::str_of(&t));
    } else {
        let l = line!(A, "print(\"  manager    %s\"");
        let m = p::or(cx.at(l, p::get(&pk, "manager"))?, s("?"));
        print(cx, l, "  manager    %s", &[m])?;
        let l = line!(A, "print(\"  policy     %s");
        let pol = cx.at(l, p::get(&pk, "policy"))?;
        let owner = p::or(cx.at(l, p::get(&v, "owner"))?, s("its owner"));
        print(
            cx,
            l,
            "  policy     %s (for this server; set by %s)",
            &[pol, owner],
        )?;
        for (key, fmt, none, marker) in [
            (
                "approved",
                "  approved   %s",
                "(none)",
                line!(A, "print(\"  approved   %s\""),
            ),
            (
                "installed",
                "  installed  %s",
                "(none of those reported)",
                line!(A, "print(\"  installed  %s\""),
            ),
        ] {
            let list = p::or(cx.at(marker - 1, p::get(&pk, key))?, Value::Array(vec![]));
            let shown = if p::truthy(&list) {
                s(&cx.at(marker, p::join(" ", &list))?)
            } else {
                s(none)
            };
            print(cx, marker, fmt, &[shown])?;
        }
        let age = cx.at(
            line!(A, "age = p.get(\"indexAgeSecs\")"),
            p::get(&pk, "indexAgeSecs"),
        )?;
        let l = line!(A, "if isinstance(age, (int, float)) and age > 86400:");
        if matches!(age, Value::Int(_) | Value::Float(_) | Value::Bool(_))
            && cx.at(l, p::cmp(">", &age, &p::int(86400)))?
        {
            let l = line!(A, "print(\"  index      %dd old");
            let days = cx.at(l, p::floordiv(&age, &p::int(86400)))?;
            let fmt = concat!(
                "  index      %dd old (",
                product_name!(),
                " refreshes apt/zypper before an install; an Arch node needs its owner to upgrade first)"
            );
            print(cx, l, fmt, &[days])?;
        }
        let l = line!(A, "if p.get(\"openRequests\"):");
        if p::truthy(&cx.at(l, p::get(&pk, "openRequests"))?) {
            let n = cx.at(l, p::idx(&pk, "openRequests"))?;
            print(cx, l, "  open       %s request(s) wait for its owner", &[n])?;
        }
    }
    let l = line!(A, "if not v.get(\"takesRequests\"):");
    if !p::truthy(&cx.at(l, p::get(&v, "takesRequests"))?) {
        cx.print(concat!(
            "  requests   this node takes none from ",
            product_name!(),
            " (older k2-node, or a Mac): its owner installs at the machine"
        ));
    }
    let l = line!(A, "rows = v.get(\"requests\") or []");
    let rows = p::or(cx.at(l, p::get(&v, "requests"))?, Value::Array(vec![]));
    let l = line!(A, "for r in rows:");
    for r in cx.at(l, p::iter(&rows))? {
        let l = line!(A, "print(\"  %-21s %-9s %s\"");
        let b = cx.at(l, p::get(&r, "batchId"))?;
        let st = cx.at(l, p::get(&r, "state"))?;
        let names = joined(cx, l, &r, "names")?;
        print(cx, l, "  %-21s %-9s %s", &[b, st, names])?;
    }
    let l = line!(A, "print(\"ask for more:");
    let node = cx.at(l, p::get(&v, "node"))?;
    let fmt = concat!(
        "ask for more: ",
        cli_name!(),
        " compute node packages request %s <name>[,<name>…] --why \"…\""
    );
    print(cx, l, fmt, &[node])
}
