//! `compute node <sub> ...`, as `cmd_compute_node` answers it
//! (`cli/src/compute-node.sh`). Made native so far: the subverbs that
//! send one POST over TCP and print one answer:
//!
//! - `add <name> [--url URL] [--json]`: `/cli/compute/nodes/enroll-code`,
//!   then `_compute_render enroll` (or the one `--json` object);
//! - `confirm <name> <code>...`, `remove|rm <name>`, `retire <name>`,
//!   `rename <old> <new>`, `pause|drain|resume <name>`, `pinned-only
//!   <name> on|off`, `route|routes add|rm|remove <name> <url>` and `alerts
//!   [<node>] [--after D|off|none] [--to WS|none]`;
//! - `wall`, `update`, `rollback`, `rollout`, `policy` and `admin`
//!   ([`more`]), with the held-change and approve flows and `take-owner`
//!   ([`flows`]);
//! - a missing or unknown subverb: the usage line, exit 2.
//!
//! Every other subverb is the shell's, and so are: a duration whose
//! seconds pass the shell's 64-bit arithmetic (the shell wraps them),
//! `update --all --batch` with a leading zero (`_compute_json` raises),
//! a `--plan` or script `--file` Python can't read, and `take-owner` with
//! no terminal to open (bash prints its own `/dev/tty` error).

pub mod flows;
pub mod more;

use super::super::lines::{py_line, LIB};
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{body_of, dumps, object, print_body, str_value, Cp, Words};
use std::path::Path;
use crate::exit::Exit;
use crate::families::{raised, Raise, Shown};
use crate::out::json::Value;
use k2_client::{brand::CLI_NAME, cli_name, product_name};

/// `cli/src/compute-node.sh`.
const SRC: &str = include_str!("../../../../../../cli/src/compute-node.sh");

/// The line of `marker` in the first `python3 -c` after `after`.
macro_rules! line {
    ($after:expr, $marker:literal) => {
        const { py_line(SRC, $after, $marker) }
    };
}

/// The line of `marker` in `_compute_render`'s Python.
macro_rules! render_line {
    ($marker:literal) => {
        const { py_line(LIB, "\n_compute_render() {", $marker) }
    };
}

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node add|confirm|remove|rename|pause|drain|resume|pinned-only|route|health|policy|update|rollout|rollback|wall|alerts|admin|servers|packages …"
);
pub const ADD_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node add <name> [--url URL] [--json]"
);
pub const CONFIRM_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node confirm <name> <6-digit code>"
);
pub const REMOVE_USAGE: &str = concat!("usage: ", cli_name!(), " compute node remove <name>");
pub const RETIRE_USAGE: &str = concat!("usage: ", cli_name!(), " compute node retire <name>");
pub const RENAME_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node rename <old> <new>"
);
pub const PINNED_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node pinned-only <name> on|off"
);
pub const ROUTE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node route add|rm <name> <url>"
);
pub const TAKE_OWNER_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node take-owner <node>"
);
pub const ALERTS_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node alerts <node> [--after 10m|off] | --to <workspace>|none"
);

/// Which printer reads the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// `add --json`: the one object scripts read.
    AddJson,
    /// `_compute_render enroll`.
    Enroll,
    Confirm,
    Remove,
    Retire,
    /// `echo "renamed to $2"`.
    Renamed(String),
    Pause,
    PinnedOnly,
    Routes,
    Alerts,
    /// `"%s: %s" % (v["node"], v.get("note","ok"))`.
    NodeNote,
    Update,
    /// `_compute_render_rollout 0`.
    Rollout,
    PolicyShow,
    AdminList,
    Denied,
    Withdrew,
}

/// What a held change does after the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    No,
    /// `grep -q '"held": *true'`, then `_compute_admin_held_or_approve`.
    IfHeld,
    /// `_compute_admin_held_or_approve` on every answer (propose).
    Always,
}

/// One request and its printer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// POST, else GET.
    pub post: bool,
    pub path: String,
    /// The body (a POST) or the query (a GET).
    pub body: String,
    /// `_compute_call`'s `tcp`.
    pub tcp: bool,
    pub held: Held,
    /// `--json`: the answer as it is.
    pub json: bool,
    pub reader: Reader,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Call(Call),
    /// `admin approve <id>`.
    Approve(String),
    /// `take-owner <node>`.
    TakeOwner(String),
}

/// A POST over TCP.
fn tcp_post(path: &str, body: String, reader: Reader) -> Option<Words<Ask>> {
    Some(Words::Ask(Ask::Call(Call {
        post: true,
        path: path.to_string(),
        body,
        tcp: true,
        held: Held::No,
        json: false,
        reader,
    })))
}

fn usage(line: impl Into<String>) -> Option<Words<Ask>> {
    Some(Words::Usage(line.into()))
}

/// The words after `node`. `None`: the shell answers them.
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let sub = words.first().copied().unwrap_or("");
    // `shift 2>/dev/null || true`.
    let rest = words.get(1..).unwrap_or(&[]);
    let arg = |i: usize| rest.get(i).copied().unwrap_or("");
    match sub {
        "add" => add(rest),
        "confirm" => {
            if rest.len() < 2 {
                return usage(CONFIRM_USAGE);
            }
            let sas = rest[1..].join(" ");
            let body = body_of(&[("node", rest[0]), ("sas", &sas)]);
            tcp_post("/cli/compute/nodes/confirm", body, Reader::Confirm)
        }
        "remove" | "rm" => {
            if rest.is_empty() {
                return usage(REMOVE_USAGE);
            }
            let body = body_of(&[("node", rest[0])]);
            tcp_post("/cli/compute/nodes/remove", body, Reader::Remove)
        }
        "retire" => {
            if rest.is_empty() {
                return usage(RETIRE_USAGE);
            }
            let body = body_of(&[("node", rest[0])]);
            tcp_post("/cli/compute/nodes/retire", body, Reader::Retire)
        }
        "rename" => {
            if rest.len() < 2 {
                return usage(RENAME_USAGE);
            }
            let body = body_of(&[("node", rest[0]), ("name", rest[1])]);
            let reader = Reader::Renamed(rest[1].to_string());
            tcp_post("/cli/compute/nodes/rename", body, reader)
        }
        "pause" | "drain" | "resume" => {
            if rest.is_empty() {
                return usage(format!("usage: {CLI_NAME} compute node {sub} <name>"));
            }
            let body = body_of(&[("node", rest[0]), ("action", sub)]);
            tcp_post("/cli/compute/nodes/pause", body, Reader::Pause)
        }
        "pinned-only" => {
            let on = match arg(1) {
                "on" => true,
                "off" => false,
                _ => return usage(PINNED_USAGE),
            };
            if rest.len() != 2 {
                return usage(PINNED_USAGE);
            }
            let mut pairs = vec![];
            if !rest[0].is_empty() {
                pairs.push(("node", str_value(rest[0])));
            }
            pairs.push(("on", Value::Bool(on)));
            let body = dumps(&object(pairs));
            tcp_post("/cli/compute/nodes/pinned-only", body, Reader::PinnedOnly)
        }
        "route" | "routes" => {
            if rest.len() < 3 {
                return usage(ROUTE_USAGE);
            }
            let key = match rest[0] {
                "add" => "add",
                "rm" | "remove" => "remove",
                _ => return usage(ROUTE_USAGE),
            };
            let body = body_of(&[("node", rest[1]), (key, rest[2])]);
            tcp_post("/cli/compute/nodes/routes", body, Reader::Routes)
        }
        "alerts" => alerts(rest),
        "wall" => more::wall(rest),
        "update" | "rollback" => more::update(sub, rest),
        "rollout" => more::rollout(rest),
        "policy" => more::policy(rest),
        "admin" => more::admin(rest),
        "take-owner" => {
            if rest.len() != 1 || rest[0].starts_with('-') {
                return usage(TAKE_OWNER_USAGE);
            }
            // `[ -r /dev/tty ]` holds but there is no terminal to read:
            // bash prints its own redirect error there (the shell's).
            if flows::dev_tty_r_ok() && !crate::prompt::dev_tty_readable() {
                return None;
            }
            Some(Words::Ask(Ask::TakeOwner(rest[0].to_string())))
        }
        "servers" | "packages" | "pkg" | "health" => None,
        _ => usage(USAGE),
    }
}

fn add(rest: &[&str]) -> Option<Words<Ask>> {
    let (mut name, mut url, mut json) = ("", "", false);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--url" => {
                url = rest.get(i + 1).copied().unwrap_or("");
                if i + 2 > rest.len() {
                    return Some(Words::ShiftFailed);
                }
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            w => {
                name = w;
                i += 1;
            }
        }
    }
    if name.is_empty() {
        return usage(ADD_USAGE);
    }
    let body = body_of(&[("name", name), ("url", url)]);
    let reader = if json {
        Reader::AddJson
    } else {
        Reader::Enroll
    };
    tcp_post("/cli/compute/nodes/enroll-code", body, reader)
}

/// `_compute_secs`'s two shapes: plain digits, or `<n>h|m|s` parts.
pub fn duration_shaped(v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    if v.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    let mut digits = false;
    for b in v.bytes() {
        match b {
            b'0'..=b'9' => digits = true,
            b'h' | b'm' | b's' if digits => digits = false,
            _ => return false,
        }
    }
    !digits
}

/// `x="$(_compute_secs "$v")"` under `set -e`: the seconds, or the usage
/// line (exit 2). `None`: a number past the shell's 64-bit arithmetic,
/// which the shell wraps (the shell's).
pub fn secs(v: &str) -> Option<Result<i64, Words<Ask>>> {
    if !duration_shaped(v) {
        return Some(Err(Words::Usage(format!(
            "usage: bad duration '{v}' (use 2h, 90m, 1h30m, 300s)"
        ))));
    }
    super::super::events::secs(v).map(Ok)
}

fn alerts(rest: &[&str]) -> Option<Words<Ask>> {
    let (mut node, mut after, mut to, mut have_to) = ("", "", "", false);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--after" | "--to" => {
                let v = rest.get(i + 1).copied().unwrap_or("");
                if rest[i] == "--after" {
                    after = v;
                } else {
                    to = v;
                    have_to = true;
                }
                if i + 2 > rest.len() {
                    return Some(Words::ShiftFailed);
                }
                i += 2;
            }
            w => {
                node = w;
                i += 1;
            }
        }
    }
    if node.is_empty() && !have_to {
        return usage(ALERTS_USAGE);
    }
    let mut pairs = vec![];
    if !node.is_empty() {
        pairs.push(("node", str_value(node)));
        match after {
            "" => {}
            "off" | "none" => pairs.push(("after", s("off"))),
            d => match secs(d)? {
                Ok(n) => pairs.push(("after", Value::Int(n.to_string()))),
                Err(stop) => return Some(stop),
            },
        }
    }
    if have_to {
        let to = if to.is_empty() { "none" } else { to };
        pairs.push(("to", str_value(to)));
    }
    let body = dumps(&object(pairs));
    tcp_post("/cli/compute/nodes/alerts", body, Reader::Alerts)
}

/// The request and what it prints. `sock`: the vault socket, for the
/// owner's confirm.
pub fn send(cp: &mut Cp<'_>, a: &Ask, sock: &Path) -> Exit {
    let c = match a {
        Ask::Call(c) => c,
        Ask::Approve(id) => return flows::approve(cp, sock, id),
        Ask::TakeOwner(node) => return flows::take_owner(cp, sock, node),
    };
    let answer = match cp.call(c.post, &c.path, &c.body, c.tcp) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if c.held == Held::Always || (c.held == Held::IfHeld && flows::held(&answer)) {
        return flows::held_or_approve(cp, sock, &answer);
    }
    if c.json {
        return print_body(&answer);
    }
    reader(&c.reader, &answer).print()
}

/// What `reader` prints for `body`.
pub fn reader(r: &Reader, body: &[u8]) -> Shown {
    if let Reader::Renamed(name) = r {
        return Shown::out(format!("renamed to {name}\n"));
    }
    let mut cx = Cx::new(p::now());
    let done = match r {
        Reader::AddJson => add_json(&mut cx, body),
        Reader::Enroll => enroll(&mut cx, body),
        Reader::Confirm => confirm(&mut cx, body),
        Reader::Remove => remove(&mut cx, body),
        Reader::Retire => retire(&mut cx, body),
        Reader::Pause => pause(&mut cx, body),
        Reader::PinnedOnly => pinned_only(&mut cx, body),
        Reader::Routes => routes(&mut cx, body),
        Reader::Alerts => alerts_reader(&mut cx, body),
        Reader::NodeNote => more::node_note(&mut cx, body),
        Reader::Update => more::update_reader(&mut cx, body),
        Reader::Rollout => more::rollout_reader(&mut cx, body),
        Reader::PolicyShow => more::policy_reader(&mut cx, body),
        Reader::AdminList => more::admin_list(&mut cx, body),
        Reader::Denied => more::request_line(&mut cx, body, true),
        Reader::Withdrew => more::request_line(&mut cx, body, false),
        Reader::Renamed(_) => unreachable!("printed above"),
    };
    match done {
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

fn add_json(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "add_json\" = \"1\" ]; then";
    let v = render::load(cx, body, line!(AFTER, "v=json.load(sys.stdin)"))?;
    let l = line!(AFTER, "print(json.dumps(");
    let mut pairs = vec![];
    for (key, from) in [
        ("code", "enroll"),
        ("expiresAt", "expiresAt"),
        ("name", "name"),
        ("controller", "controllerUrl"),
    ] {
        pairs.push((key, cx.at(l, p::idx(&v, from))?));
    }
    let t = cx.at(l, p::dumps(&object(pairs), false, None))?;
    cx.print(&t);
    Ok(())
}

/// `_compute_render enroll`.
fn enroll(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = render::load(cx, body, render_line!("v = json.load(sys.stdin)"))?;
    let l = render_line!("print(\"Enroll code for %s (expires in 10 minutes):\"");
    let name = cx.at(l, p::idx(&v, "name"))?;
    print(cx, l, "Enroll code for %s (expires in 10 minutes):", &[name])?;
    let l = render_line!("print(\"  %s\" % v[\"enroll\"])");
    let code = cx.at(l, p::idx(&v, "enroll"))?;
    print(cx, l, "  %s", &[code])?;
    cx.print("Keep it off command lines (ps shows them to every user): put it in a file only you");
    cx.print(concat!(
        "can read, or in the vault (",
        cli_name!(),
        " vault set NAME --kind generic --delivery file), and pipe it in."
    ));
    cx.print("On the node machine, as an admin (code-file holds the code):");
    // `v["install"][key]` at line `l`.
    let install = |cx: &Cx, l: u32, key: &str| {
        let x = cx.at(l, p::idx(&v, "install"))?;
        cx.at(l, p::idx(&x, key))
    };
    let l = render_line!("print(\"  Linux: %s\" % v[\"install\"][\"linux\"])");
    let x = install(cx, l, "linux")?;
    print(cx, l, "  Linux: %s", &[x])?;
    let l = render_line!("print(\"  macOS: %s\" % v[\"install\"][\"macos\"])");
    let x = install(cx, l, "macos")?;
    print(cx, l, "  macOS: %s", &[x])?;
    let l = render_line!("if v[\"install\"].get(\"servers\"):");
    let x = cx.at(l, p::idx(&v, "install"))?;
    if p::truthy(&cx.at(l, p::get(&x, "servers"))?) {
        let l = render_line!("print(\"  Already a node for another ");
        let x = install(cx, l, "servers")?;
        let fmt = concat!("  Already a node for another ", product_name!(), " server: %s");
        print(cx, l, fmt, &[x])?;
    }
    cx.print("The node prints a 6-digit code. Check it, then here:");
    let l = render_line!("print(\"  %s\" % v[\"confirm\"])");
    let x = cx.at(l, p::idx(&v, "confirm"))?;
    print(cx, l, "  %s", &[x])?;
    let l = render_line!("print(\"Scripts: ");
    let name = cx.at(l, p::idx(&v, "name"))?;
    let fmt = concat!(
        "Scripts: ",
        cli_name!(),
        " compute node add %s --json prints {code, expiresAt, name, controller}."
    );
    print(cx, l, fmt, &[name])
}

/// The one-line readers: `json.load` and the print on line 1.
fn one_line(cx: &Cx, body: &[u8]) -> Result<Value, Raise> {
    render::load(cx, body, 1)
}

fn confirm(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = one_line(cx, body)?;
    let args = [cx.at(1, p::idx(&v, "node"))?, cx.at(1, p::idx(&v, "next"))?];
    print(cx, 1, "%s is active. %s", &args)
}

fn remove(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = one_line(cx, body)?;
    let args = [
        cx.at(1, p::idx(&v, "node"))?,
        cx.at(1, p::idx(&v, "jobsCancelled"))?,
    ];
    print(cx, 1, "removed %s (%d jobs cancelled)", &args)
}

fn retire(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = one_line(cx, body)?;
    let node = cx.at(1, p::idx(&v, "node"))?;
    let what = if p::eq_s(&cx.at(1, p::idx(&v, "state"))?, "revoked") {
        s("removed")
    } else {
        let running = cx.at(1, p::get_or(&v, "running", p::int(0)))?;
        s(&cx.at(
            1,
            p::pct(
                "draining; removed once its %d running job(s) end",
                &[running],
            ),
        )?)
    };
    print(cx, 1, "%s: %s", &[node, what])
}

fn pause(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = one_line(cx, body)?;
    let node = cx.at(1, p::idx(&v, "node"))?;
    let state = p::or(cx.at(1, p::get(&v, "controllerPause"))?, s("active"));
    let note = cx.at(1, p::idx(&v, "note"))?;
    print(cx, 1, "%s: server-side %s. %s", &[node, state, note])
}

fn pinned_only(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = one_line(cx, body)?;
    let node = cx.at(1, p::idx(&v, "node"))?;
    let on = if p::truthy(&cx.at(1, p::idx(&v, "pinnedOnly"))?) {
        "on"
    } else {
        "off"
    };
    let was = if p::truthy(&cx.at(1, p::get(&v, "changed"))?) {
        ""
    } else {
        " (it already was)"
    };
    let note = cx.at(1, p::idx(&v, "note"))?;
    print(cx, 1, "%s: pinned-only %s%s. %s.", &[node, s(on), s(was), note])
}

fn routes(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = one_line(cx, body)?;
    let node = cx.at(1, p::idx(&v, "node"))?;
    let joined = cx.at(1, p::idx(&v, "routes").and_then(|r| p::join(" ", &r)))?;
    let shown = if joined.is_empty() {
        "(none)".to_string()
    } else {
        joined
    };
    print(cx, 1, "routes for %s: %s", &[node, s(&shown)])
}

fn alerts_reader(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "nodes/alerts \"$(_compute_json";
    let v = render::load(cx, body, line!(AFTER, "v=json.load(sys.stdin)"))?;
    let l = line!(AFTER, "if \"alertsTo\" in v:");
    if cx.at(l, p::has(&v, "alertsTo"))? {
        let to = p::or(cx.at(l, p::idx(&v, "alertsTo"))?, s("nobody (unset)"));
        print(cx, l, "alerts go to: %s", &[to])?;
    }
    if cx.at(line!(AFTER, "if \"node\" in v:"), p::has(&v, "node"))? {
        let a = cx.at(line!(AFTER, "a=v.get("), p::get(&v, "alertAfterSecs"))?;
        let l = line!(AFTER, "print(\"%s: %s\" % (v[\"node\"]");
        let node = cx.at(l, p::idx(&v, "node"))?;
        let what = if p::truthy(&a) {
            let m = cx.at(l, p::floordiv(&a, &p::int(60)))?;
            s(&cx.at(l, p::pct("offline alert after %dm", &[m]))?)
        } else {
            s("no offline alert")
        };
        print(cx, l, "%s: %s", &[node, what])?;
    }
    Ok(())
}
