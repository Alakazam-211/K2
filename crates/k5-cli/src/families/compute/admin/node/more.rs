//! `node wall`, `update`, `rollback`, `rollout`, `policy` and `admin`:
//! their words, as `cmd_compute_node` and `cmd_compute_node_admin` read
//! them, and their readers.

use super::{print, usage, Ask, Call, Held, Reader};
use crate::families::compute::lines::py_line;
use crate::families::compute::pyrt::{self as p, s};
use crate::families::compute::render::{self, Cx};
use crate::families::compute::writes::{body_of, dumps, object, str_value, Words};
use crate::families::Raise;
use crate::out::json::{self, Text, Value};
use crate::transport::encode::quote;
use k2_client::{brand::CLI_NAME, cli_name};

/// `cli/src/compute-node.sh`.
const SRC: &str = super::SRC;

macro_rules! line {
    ($after:expr, $marker:literal) => {
        const { py_line(SRC, $after, $marker) }
    };
}

pub const WALL_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node wall <node> [--no-restart] [--reason \"…\"]"
);
pub const ROLLOUT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node rollout [show|pause|resume|stop] [--id ID] [--json]"
);
pub const ALL_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node update --all [--batch N] [--now]   (no node name with --all)"
);
pub const POLICY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node policy <node> show|set key=value..."
);
pub const POLICY_SET_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node policy <node> set key=value... [--reason …]"
);
pub const PROPOSE_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node admin propose <node> <verb> [key=value…]"
);
pub const PROPOSE_NEEDS: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node admin propose <node> <verb> [key=value…] --reason \"…\" (or --plan plan.json)"
);
pub const SCRIPT_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node admin propose <node> script --file fix.sh --reason …"
);
pub const PLAN_USAGE: &str =
    "usage: --plan needs a JSON file: {\"steps\": [{\"verb\": …, \"args\": {…}}]}";
pub const LIST_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node admin list [--node N] [--state open|pending|all] [--limit N] [--json]"
);
pub const APPROVE_USAGE: &str = concat!("usage: ", cli_name!(), " compute node admin approve <id>");
pub const DENY_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node admin deny <id> [--why …]"
);
pub const WITHDRAW_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute node admin withdraw <id>"
);

fn call(post: bool, path: &str, body: String, held: Held, json: bool, reader: Reader) -> Words<Ask> {
    Words::Ask(Ask::Call(Call {
        post,
        path: path.to_string(),
        body,
        tcp: false,
        held,
        json,
        reader,
    }))
}

/// A value flag's `x="${2:-}"; shift 2`: the value, or the shift that
/// fails with it last.
fn value<'a>(rest: &[&'a str], i: usize) -> Result<&'a str, Words<Ask>> {
    if i + 2 > rest.len() {
        return Err(Words::ShiftFailed);
    }
    Ok(rest[i + 1])
}

/// `wall <node> [--no-restart] [--reason …]`.
pub fn wall(rest: &[&str]) -> Option<Words<Ask>> {
    let (mut node, mut restart, mut reason) = ("", true, "");
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--no-restart" => {
                restart = false;
                i += 1;
            }
            "--reason" => {
                reason = match value(rest, i) {
                    Ok(v) => v,
                    Err(stop) => return Some(stop),
                };
                i += 2;
            }
            w if w.starts_with('-') => return usage(WALL_USAGE),
            w => {
                node = w;
                i += 1;
            }
        }
    }
    if node.is_empty() {
        return usage(WALL_USAGE);
    }
    let mut pairs = vec![("node", str_value(node)), ("restart", Value::Bool(restart))];
    if !reason.is_empty() {
        pairs.push(("reason", str_value(reason)));
    }
    let body = dumps(&object(pairs));
    Some(call(true, "/cli/compute/nodes/wall", body, Held::IfHeld, false, Reader::NodeNote))
}

/// `update|rollback <node> [--canary] [--now]`, `update --all [--batch N]
/// [--now]`.
pub fn update(sub: &str, rest: &[&str]) -> Option<Words<Ask>> {
    let (mut node, mut canary, mut now, mut all, mut batch) = ("", false, false, false, None);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--canary" => canary = true,
            "--now" => now = true,
            "--all" => all = true,
            "--batch" => {
                let b = rest.get(i + 1).copied().unwrap_or("");
                if b.is_empty() || !b.bytes().all(|c| c.is_ascii_digit()) {
                    return usage("usage: --batch N (a number of nodes; 0 = all at once)");
                }
                if i + 2 > rest.len() {
                    return Some(Words::ShiftFailed);
                }
                batch = Some(b);
                i += 1;
            }
            w if w.starts_with('-') => {
                return usage(format!(
                    "usage: {CLI_NAME} compute node {sub} <node> [--now]   (update also: --all [--batch N])"
                ))
            }
            w => node = w,
        }
        i += 1;
    }
    if all {
        if sub != "update" || !node.is_empty() {
            return usage(ALL_USAGE);
        }
        // `K2_CJ_batch_j="${batch:-0}"`: json.loads of digits with a
        // leading zero raises in `_compute_json`; that run is the shell's.
        let b = batch.unwrap_or("0");
        if b.len() > 1 && b.starts_with('0') {
            return None;
        }
        let mut pairs = vec![("all", Value::Bool(true))];
        if now {
            pairs.push(("now", Value::Bool(true)));
        }
        pairs.push(("batch", Value::Int(b.to_string())));
        let body = dumps(&object(pairs));
        return Some(call(
            true,
            "/cli/compute/nodes/update",
            body,
            Held::No,
            false,
            Reader::Rollout,
        ));
    }
    if batch.is_some() {
        return usage("usage: --batch goes with --all");
    }
    if node.is_empty() {
        return usage(format!("usage: {CLI_NAME} compute node {sub} <node> [--now]"));
    }
    let mut pairs = vec![("node", str_value(node)), ("canary", Value::Bool(canary))];
    if now {
        pairs.push(("now", Value::Bool(true)));
    }
    let body = dumps(&object(pairs));
    let path = format!("/cli/compute/nodes/{sub}");
    Some(call(true, &path, body, Held::No, false, Reader::Update))
}

/// `rollout [show|pause|resume|stop] [--id ID] [--json]`.
pub fn rollout(rest: &[&str]) -> Option<Words<Ask>> {
    let (mut verb, mut json, mut id) = ("show", false, "");
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            w @ ("show" | "pause" | "resume" | "stop") => verb = w,
            "--json" => json = true,
            "--id" => {
                id = match value(rest, i) {
                    Ok(v) => v,
                    Err(stop) => return Some(stop),
                };
                i += 1;
            }
            _ => return usage(ROLLOUT_USAGE),
        }
        i += 1;
    }
    Some(if verb == "show" {
        call(
            false,
            "/cli/compute/nodes/rollout",
            String::new(),
            Held::No,
            json,
            Reader::Rollout,
        )
    } else {
        let path = format!("/cli/compute/nodes/rollout/{verb}");
        call(true, &path, body_of(&[("id", id)]), Held::No, json, Reader::Rollout)
    })
}

/// `policy <node> show [--json]`, `policy <node> set key=value...
/// [--reason …]`.
pub fn policy(rest: &[&str]) -> Option<Words<Ask>> {
    let node = rest.first().copied().unwrap_or("");
    let act = rest.get(1).copied().unwrap_or("show");
    if node.is_empty() {
        return usage(POLICY_USAGE);
    }
    // `shift 2 2>/dev/null || shift $#`.
    let more = rest.get(2..).unwrap_or(&[]);
    match act {
        "show" => {
            let json = more.contains(&"--json");
            let q = format!("node={}", quote(node));
            Some(call(
                false,
                "/cli/compute/nodes/policy",
                q,
                Held::No,
                json,
                Reader::PolicyShow,
            ))
        }
        "set" => {
            let (mut reason, mut pairs) = ("", vec![]);
            let mut i = 0;
            while i < more.len() {
                match more[i] {
                    "--reason" => {
                        reason = match value(more, i) {
                            Ok(v) => v,
                            Err(stop) => return Some(stop),
                        };
                        i += 2;
                    }
                    w if w.contains('=') => {
                        pairs.push(w);
                        i += 1;
                    }
                    _ => return usage(POLICY_SET_USAGE),
                }
            }
            if pairs.is_empty() {
                return usage(POLICY_USAGE.replace("show|set", "set"));
            }
            let set = match kv_json(&pairs) {
                Ok(v) => v,
                Err(stop) => return Some(stop),
            };
            let mut body = vec![("node", str_value(node)), ("set", set)];
            if !reason.is_empty() {
                body.push(("reason", str_value(reason)));
            }
            let body = dumps(&object(body));
            Some(call(
                true,
                "/cli/compute/nodes/policy/set",
                body,
                Held::IfHeld,
                false,
                Reader::NodeNote,
            ))
        }
        _ => usage(POLICY_USAGE),
    }
}

/// `_compute_kv_json key=value...`: each value as `json.loads` reads it,
/// else the string; `names` and `labels` as a comma list. An empty key is
/// Python's `usage: key=value` and exit 2.
pub fn kv_json(pairs: &[&str]) -> Result<Value, Words<Ask>> {
    let mut out: Vec<(String, Value)> = vec![];
    for a in pairs {
        let (k, v) = a.split_once('=').unwrap_or((*a, ""));
        let k = k.trim_matches(crate::families::py_space);
        if k.is_empty() {
            return Err(Words::Usage("usage: key=value".into()));
        }
        let mut val = json::parse(v).unwrap_or_else(|_| str_value(v));
        if matches!(k, "names" | "labels") {
            if let Value::Str(_) = &val {
                let text = p::str_of(&val);
                val = Value::Array(
                    text.split(',')
                        .filter(|x| !x.is_empty())
                        .map(str_value)
                        .collect(),
                );
            }
        }
        match out.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = val,
            None => out.push((k.to_string(), val)),
        }
    }
    Ok(object(out.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()))
}

/// `admin [propose|list|ls|approve|deny|withdraw] ...`.
pub fn admin(rest: &[&str]) -> Option<Words<Ask>> {
    let sub = rest.first().copied().unwrap_or("");
    let (sub, rest) = match sub {
        "propose" | "list" | "ls" | "approve" | "deny" | "withdraw" => (sub, &rest[1..]),
        "" | "help" | "--help" | "-h" => return None,
        _ => ("propose", rest),
    };
    let first = rest.first().copied().unwrap_or("");
    match sub {
        "propose" => propose(rest),
        "list" | "ls" => {
            let (mut q, mut json) = (String::new(), false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    k @ ("--node" | "--state" | "--limit") => {
                        let v = match value(rest, i) {
                            Ok(v) => v,
                            Err(stop) => return Some(stop),
                        };
                        q.push_str(&format!("&{}={}", &k[2..], quote(v)));
                        i += 2;
                    }
                    "--json" => {
                        json = true;
                        i += 1;
                    }
                    _ => return usage(LIST_USAGE),
                }
            }
            let q = q.strip_prefix('&').unwrap_or(&q).to_string();
            Some(call(
                false,
                "/cli/compute/admin/requests",
                q,
                Held::No,
                json,
                Reader::AdminList,
            ))
        }
        "approve" => {
            if first.is_empty() {
                return usage(APPROVE_USAGE);
            }
            Some(Words::Ask(Ask::Approve(first.to_string())))
        }
        "deny" => {
            let (mut id, mut why) = ("", "");
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--why" {
                    why = match value(rest, i) {
                        Ok(v) => v,
                        Err(stop) => return Some(stop),
                    };
                    i += 2;
                } else {
                    id = rest[i];
                    i += 1;
                }
            }
            if id.is_empty() {
                return usage(DENY_USAGE);
            }
            let body = body_of(&[("request", id), ("why", why)]);
            Some(Words::Ask(Ask::Call(Call {
                post: true,
                path: "/cli/compute/admin/deny".into(),
                body,
                tcp: true,
                held: Held::No,
                json: false,
                reader: Reader::Denied,
            })))
        }
        _ => {
            if first.is_empty() {
                return usage(WITHDRAW_USAGE);
            }
            let body = body_of(&[("request", first)]);
            let path = "/cli/compute/admin/withdraw";
            Some(call(true, path, body, Held::No, false, Reader::Withdrew))
        }
    }
}

/// A file's text as Python's `open(f).read()` gives it: UTF-8, its line
/// ends made `\n`. `None` where Python would raise (the shell's).
fn read_text(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `[ -f "$f" ]`.
fn is_file(path: &str) -> bool {
    !path.is_empty() && std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

fn propose(rest: &[&str]) -> Option<Words<Ask>> {
    let (mut node, mut verb, mut reason, mut file, mut interp, mut plan) =
        ("", "", "", "", "", "");
    let mut timeout: Option<i64> = None;
    let mut pairs = vec![];
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--timeout" => {
                // `timeout="$(_compute_secs "${2:-}")"` before the shift.
                let v = rest.get(i + 1).copied().unwrap_or("");
                match super::secs(v)? {
                    Ok(n) => timeout = Some(n),
                    Err(stop) => return Some(stop),
                }
                if i + 2 > rest.len() {
                    return Some(Words::ShiftFailed);
                }
                i += 2;
            }
            flag @ ("--reason" | "--file" | "--interpreter" | "--plan") => {
                let v = match value(rest, i) {
                    Ok(v) => v,
                    Err(stop) => return Some(stop),
                };
                match flag {
                    "--reason" => reason = v,
                    "--file" => file = v,
                    "--interpreter" => interp = v,
                    _ => plan = v,
                }
                i += 2;
            }
            w if w.contains('=') => {
                pairs.push(w);
                i += 1;
            }
            w => {
                if node.is_empty() {
                    node = w;
                } else if verb.is_empty() {
                    verb = w;
                } else {
                    return usage(PROPOSE_USAGE);
                }
                i += 1;
            }
        }
    }
    if node.is_empty() || (verb.is_empty() && plan.is_empty()) {
        return usage(PROPOSE_NEEDS);
    }
    let body = if !plan.is_empty() {
        if !is_file(plan) {
            return usage(PLAN_USAGE);
        }
        // Python's json.load and p["steps"]; where it raises, the shell's.
        let doc = json::parse(&read_text(plan)?).ok()?;
        let steps = match &doc {
            Value::Object(_) => p::idx(&doc, "steps").ok()?,
            _ => doc.clone(),
        };
        let reason = if reason.is_empty() {
            Value::Null
        } else {
            str_value(reason)
        };
        dumps(&object(vec![
            ("node", str_value(node)),
            ("kind", s("plan")),
            ("steps", steps),
            ("reason", reason),
        ]))
    } else {
        let mut args = match kv_json(&pairs) {
            Ok(v) => v,
            Err(stop) => return Some(stop),
        };
        if verb == "script" {
            if !is_file(file) {
                return usage(SCRIPT_USAGE);
            }
            let text = read_text(file)?;
            let Value::Object(a) = &mut args else {
                unreachable!("kv_json gives a dict")
            };
            let mut set = |k: &str, v: Value| {
                let k = Text::from(k);
                match a.iter_mut().find(|(key, _)| *key == k) {
                    Some(slot) => slot.1 = v,
                    None => a.push((k, v)),
                }
            };
            set("text", str_value(&text));
            if !interp.is_empty() {
                set("interpreter", str_value(interp));
            }
            if let Some(t) = timeout {
                set("timeout_secs", Value::Int(t.to_string()));
            }
        }
        let kind = if verb.starts_with("svc.") {
            "service"
        } else {
            "command"
        };
        let mut body = vec![
            ("node", str_value(node)),
            ("kind", s(kind)),
            ("verb", str_value(verb)),
            ("args", args),
        ];
        if !reason.is_empty() {
            body.push(("reason", str_value(reason)));
        }
        dumps(&object(body))
    };
    Some(call(
        true,
        "/cli/compute/admin/propose",
        body,
        Held::Always,
        false,
        Reader::NodeNote,
    ))
}

// ---- readers ---------------------------------------------------------------

/// `print("%s: %s" % (v["node"], v.get("note","ok")))` (wall, policy set).
pub fn node_note(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    let v = render::load(cx, body, 1)?;
    let node = cx.at(1, p::idx(&v, "node"))?;
    let note = cx.at(1, p::get_or(&v, "note", s("ok")))?;
    print(cx, 1, "%s: %s", &[node, note])
}

/// `update|rollback <node>`'s line.
pub fn update_reader(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "/cli/compute/nodes/$sub\"";
    let l = line!(AFTER, "v=json.load(sys.stdin); u=");
    let v = render::load(cx, body, l)?;
    let u = p::or(cx.at(l, p::get(&v, "update"))?, Value::Object(vec![]));
    let l = line!(AFTER, "print(\"%s: %s%s. %s\"");
    let node = cx.at(l, p::idx(&v, "node"))?;
    let state = cx.at(l, p::get(&u, "state"))?;
    let version = if p::truthy(&cx.at(l, p::get(&u, "version"))?) {
        let x = cx.at(l, p::idx(&u, "version"))?;
        s(&cx.at(l, p::add(&s(" "), &x).map(|v| p::str_of(&v)))?)
    } else {
        s("")
    };
    let note = cx.at(l, p::get_or(&v, "note", s("")))?;
    print(cx, l, "%s: %s%s. %s", &[node, state, version, note])
}

/// `_compute_render_rollout 0`.
pub fn rollout_reader(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "_compute_render_rollout() {";
    let l = line!(AFTER, "v=json.load(sys.stdin); r=");
    let v = render::load(cx, body, l)?;
    let r = cx.at(l, p::get(&v, "rollout"))?;
    if !p::truthy(&r) {
        let l = line!(AFTER, "print(\"no rollout yet");
        let follow = cx.at(l, p::get_or(&v, "follow", Value::Bool(true)))?;
        let off = if p::truthy(&follow) {
            ""
        } else {
            "; auto-follow (compute.nodes.follow) is off"
        };
        cx.print(&format!(
            "no rollout yet ({CLI_NAME} compute node update --all starts one){off}"
        ));
        return Ok(());
    }
    let l = line!(AFTER, "p=r.get(\"position\")");
    let pos = p::or(cx.at(l, p::get(&r, "position"))?, Value::Object(vec![]));
    let l = line!(AFTER, "how=");
    let how = if p::truthy(&cx.at(l, p::get(&r, "now"))?) {
        "all at once".to_string()
    } else if p::truthy(&cx.at(l, p::get(&r, "batchSize"))?) {
        let b = cx.at(l, p::idx(&r, "batchSize"))?;
        let t = cx.at(l, p::pct(", at most %d in all", &[b]))?;
        format!("a few per OS/arch at a time{t}")
    } else {
        "a few per OS/arch at a time".to_string()
    };
    let l = line!(AFTER, "print(\"rollout %s:");
    let id = cx.at(l, p::idx(&r, "id").and_then(|x| p::slice(&x, None, Some(8))))?;
    let mut args = vec![id];
    for k in ["toVersion", "state", "trigger"] {
        args.push(cx.at(l, p::idx(&r, k))?);
    }
    args.push(cx.at(l, p::get(&pos, "settled"))?);
    args.push(cx.at(l, p::get(&pos, "total"))?);
    args.push(s(&how));
    print(
        cx,
        l,
        "rollout %s: k2-node %s, %s, %s (%s of %s settled; %s)",
        &args,
    )?;
    let l = line!(AFTER, "if r.get(\"note\"):");
    if p::truthy(&cx.at(l, p::get(&r, "note"))?) {
        let n = cx.at(l, p::idx(&r, "note"))?;
        let t = cx.at(l, p::add(&s("  "), &n))?;
        cx.print(&p::str_of(&t));
    }
    let l = line!(AFTER, "for n in r.get(\"nodes\")");
    let nodes = p::or(cx.at(l, p::get(&r, "nodes"))?, Value::Array(vec![]));
    for n in cx.at(l, p::iter(&nodes))? {
        let l = line!(AFTER, "why=(");
        let why = if p::truthy(&cx.at(l, p::get(&n, "code"))?) {
            let c = cx.at(l, p::idx(&n, "code"))?;
            cx.at(l, p::add(&s(" "), &c))?
        } else {
            s("")
        };
        let l = line!(AFTER, "print(\"  %-14s %s%s%s\"");
        let node = cx.at(l, p::idx(&n, "node"))?;
        let state = cx.at(l, p::idx(&n, "state"))?;
        let mut detail = s("");
        if p::truthy(&cx.at(l, p::get(&n, "detail"))?) {
            let st = cx.at(l, p::idx(&n, "state"))?;
            let failed = ["skipped", "failed", "refused", "timed_out"]
                .iter()
                .any(|w| p::eq_s(&st, w));
            let waiting = failed || {
                let c = p::or(cx.at(l, p::get(&n, "code"))?, s(""));
                cx.at(l, p::startswith(&c, "waiting_"))?
            };
            if waiting {
                let d = cx.at(l, p::idx(&n, "detail"))?;
                detail = cx.at(l, p::add(&s(": "), &d))?;
            }
        }
        print(cx, l, "  %-14s %s%s%s", &[node, state, why, detail])?;
    }
    let l = line!(AFTER, "if v.get(\"note\"): print(v[\"note\"])");
    if p::truthy(&cx.at(l, p::get(&v, "note"))?) {
        let n = cx.at(l, p::idx(&v, "note"))?;
        cx.print(&p::str_of(&n));
    }
    Ok(())
}

/// `policy <node> show`.
pub fn policy_reader(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "/cli/compute/nodes/policy \"node=";
    let v = render::load(cx, body, line!(AFTER, "v=json.load(sys.stdin)"))?;
    let l = line!(AFTER, "print(\"%s (%s, remote admin %s)\"");
    let node = cx.at(l, p::idx(&v, "node"))?;
    let online = if p::truthy(&cx.at(l, p::idx(&v, "online"))?) {
        "online"
    } else {
        "offline"
    };
    let admin = p::or(cx.at(l, p::get(&v, "remoteAdmin"))?, s("unknown"));
    print(cx, l, "%s (%s, remote admin %s)", &[node, s(online), admin])?;
    let pol = cx.at(line!(AFTER, "p=v.get(\"policy\")"), p::get(&v, "policy"))?;
    if !p::truthy(&pol) {
        let l = line!(AFTER, "if not p: print(");
        let hint = cx.at(l, p::get_or(&v, "hint", s("")))?;
        let t = cx.at(l, p::add(&s("  "), &hint))?;
        cx.print(&p::str_of(&t));
        return Ok(());
    }
    let l = line!(AFTER, "for k in sorted(p):");
    let items = match &pol {
        Value::Object(_) => cx.at(l, p::sorted_items(&pol))?,
        Value::Str(_) => {
            return Err(cx.raise(
                l,
                "TypeError: string indices must be integers, not 'str'".into(),
            ))
        }
        Value::Array(_) => {
            return Err(cx.raise(
                l,
                "TypeError: list indices must be integers or slices, not str".into(),
            ))
        }
        other => {
            return Err(cx.raise(
                l,
                format!("TypeError: '{}' object is not iterable", p::tname(other)),
            ))
        }
    };
    for (k, x) in items {
        let d = cx.at(l, p::dumps(&x, false, None))?;
        print(cx, l, "  %-18s %s", &[k, s(&d)])?;
    }
    Ok(())
}

/// `admin list`.
pub fn admin_list(cx: &mut Cx, body: &[u8]) -> Result<(), Raise> {
    const AFTER: &str = "_compute_call GET /cli/compute/admin/requests \"${q#&}\"";
    let v = render::load(cx, body, line!(AFTER, "v=json.load(sys.stdin)"))?;
    let l = line!(AFTER, "rows=v[\"requests\"]");
    let rows = cx.at(l, p::idx(&v, "requests"))?;
    if !p::truthy(&rows) {
        cx.print("no admin requests");
    }
    let l = line!(AFTER, "for r in rows:");
    for r in cx.at(l, p::iter(&rows))? {
        let l = line!(AFTER, "print(\"%-36s %-9s %-10s %s\"");
        let id = cx.at(l, p::idx(&r, "id"))?;
        let state = cx.at(l, p::idx(&r, "state"))?;
        let node = p::or(cx.at(l, p::get(&r, "node"))?, s("?"));
        let summary = cx.at(l, p::idx(&r, "summary"))?;
        print(cx, l, "%-36s %-9s %-10s %s", &[id, state, node, summary])?;
        for (key, fmt, l) in [
            (
                "reason",
                "%36s reason: %s",
                line!(AFTER, "if r.get(\"reason\"):"),
            ),
            (
                "denyReason",
                "%36s denied: %s",
                line!(AFTER, "if r.get(\"denyReason\"):"),
            ),
        ] {
            if p::truthy(&cx.at(l, p::get(&r, key))?) {
                let x = cx.at(l, p::idx(&r, key))?;
                print(cx, l, fmt, &[s(""), x])?;
            }
        }
    }
    Ok(())
}

/// `r=json.load(sys.stdin)["request"]` and one line (deny, withdraw).
pub fn request_line(cx: &mut Cx, body: &[u8], denied: bool) -> Result<(), Raise> {
    let v = render::load(cx, body, 1)?;
    let r = cx.at(1, p::idx(&v, "request"))?;
    let id = cx.at(1, p::idx(&r, "id"))?;
    if denied {
        let summary = cx.at(1, p::idx(&r, "summary"))?;
        print(cx, 1, "denied %s: %s", &[id, summary])
    } else {
        print(cx, 1, "withdrew %s", &[id])
    }
}
