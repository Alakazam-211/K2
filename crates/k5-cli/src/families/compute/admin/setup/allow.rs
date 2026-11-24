//! `compute allow-nodes [<node>[,<node>…] | --add NODE[,…] | --remove
//! NODE[,…] | --clear | --fixed on|off] [--workspace WS] [--json]`, as
//! `cmd_compute_allow_nodes` answers it (`cli/src/compute-access.sh`):
//! no change asked is one GET of `/cli/compute/allow-nodes` (this
//! checkout's workspace outside a session); a change is one POST of
//! `/cli/compute/allow-nodes/set`. Both go the cell socket first. Then the
//! answer as it came (`--json`) or the list in words.

use super::super::super::lines::py_line;
use super::super::super::pyrt::{self as p, s};
use super::super::super::writes::{dumps, load, object, print_body, stopped, str_value, Cp, Words};
use super::{at, ACCESS_SRC};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::Value;
use crate::transport::encode::quote;
use k2_client::cli_name;

pub const GET: &str = "/cli/compute/allow-nodes";
pub const SET: &str = "/cli/compute/allow-nodes/set";

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute allow-nodes [<node>[,<node>…] | --add NODE[,NODE…] | --remove NODE[,NODE…] | --clear | --fixed on|off] [--workspace WS] [--json]"
);

macro_rules! line {
    ($m:literal) => {
        const { py_line(ACCESS_SRC, "\ncmd_compute_allow_nodes() {", $m) }
    };
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub nodes: String,
    pub add: String,
    pub remove: String,
    pub clear: bool,
    /// `--fixed on` / `--fixed off` (0.46.18).
    pub fixed: Option<bool>,
    pub workspace: String,
    pub json: bool,
}

/// `"${x:+$x,}$w"`.
fn append(x: &mut String, w: &str) {
    if !x.is_empty() {
        x.push(',');
    }
    x.push_str(w);
}

/// The words after `allow-nodes`.
pub fn parse(words: &[&str]) -> Words<Ask> {
    let mut a = Ask::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--add" | "--remove" | "--workspace" | "--fixed" => {
                let v = match words.get(i + 1) {
                    Some(v) if !v.is_empty() => *v,
                    _ => return Words::Usage(format!("{USAGE} ({w} needs a value)")),
                };
                match w {
                    "--add" => append(&mut a.add, v),
                    "--remove" => append(&mut a.remove, v),
                    "--fixed" => {
                        a.fixed = match v {
                            "on" => Some(true),
                            "off" => Some(false),
                            _ => return Words::Usage(format!("{USAGE} (--fixed takes on or off)")),
                        }
                    }
                    _ => a.workspace = v.to_string(),
                }
                i += 2;
                continue;
            }
            "--clear" => a.clear = true,
            "--json" => a.json = true,
            w if w.starts_with('-') => {
                return Words::Usage(format!("{USAGE} (unknown flag '{w}')"));
            }
            w => append(&mut a.nodes, w),
        }
        i += 1;
    }
    Words::Ask(a)
}

/// The POST's body (`_compute_json`).
pub fn body(a: &Ask, ws: &str) -> String {
    let mut pairs = vec![];
    for (k, v) in [
        ("workspace", ws),
        ("nodes", &a.nodes),
        ("add", &a.add),
        ("remove", &a.remove),
    ] {
        if !v.is_empty() {
            pairs.push((k, str_value(v)));
        }
    }
    if a.clear {
        pairs.push(("clear", Value::Bool(true)));
    }
    if let Some(f) = a.fixed {
        pairs.push(("fixed", Value::Bool(f)));
    }
    dumps(&object(pairs))
}

/// The request and what it prints.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let ws = cp.ws(&a.workspace);
    let answer = if a.nodes.is_empty()
        && a.add.is_empty()
        && a.remove.is_empty()
        && !a.clear
        && a.fixed.is_none()
    {
        let q = if ws.is_empty() {
            String::new()
        } else {
            format!("workspace={}", quote(&ws))
        };
        cp.call(false, GET, &q, false)
    } else {
        cp.call(true, SET, &body(a, &ws), false)
    };
    let answer = match answer {
        Ok(b) => b,
        Err(e) => return e,
    };
    if a.json {
        return print_body(&answer);
    }
    reader(&answer).print()
}

/// The reader on the answer.
pub fn reader(body: &[u8]) -> Shown {
    let mut out = String::new();
    let r = read(body, &mut out);
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => stopped(out, &e),
    }
}

fn read(body: &[u8], out: &mut String) -> Result<(), Raise> {
    let v = load(body, line!("v = json.load"))?;
    let l = line!("if v.get(\"change\")");
    if p::eq_s(&p::get(&v, "change").map_err(at(l))?, "same") {
        out.push_str("no change\n");
    }
    let l = line!("if v.get(\"list\")");
    if p::truthy(&p::get(&v, "list").map_err(at(l))?) {
        let l = line!("for n in v.get(\"nodes\")");
        let nodes = p::get(&v, "nodes")
            .and_then(p::iter_or_empty)
            .map_err(at(l))?;
        let mut names = Vec::with_capacity(nodes.len());
        for n in &nodes {
            let l = line!("if not n.get(\"name\")");
            if !p::truthy(&p::get(n, "name").map_err(at(l))?) {
                let l = line!("names.append(\"%s (removed)\"");
                let id = p::get(n, "id").map_err(at(l))?;
                names.push(s(&format!("{} (removed)", p::str_of(&id))));
                continue;
            }
            let mut name = p::idx(n, "name").map_err(at(line!("name = n[\"name\"]")))?;
            let l = line!("if n.get(\"state\") not in");
            let state = p::get(n, "state").map_err(at(l))?;
            if !(p::is_none(&state) || p::eq_s(&state, "active")) {
                let l = line!("name += \" (%s)\"");
                let st = p::idx(n, "state").map_err(at(l))?;
                let tail = p::pct(" (%s)", &[st]).map_err(at(l))?;
                name = p::iadd(&name, &s(&tail)).map_err(at(l))?;
            }
            let l = line!("if not n.get(\"granted\")");
            if !p::truthy(&p::get(n, "granted").map_err(at(l))?) {
                let l = line!("name += \" (no grant)\"");
                name = p::iadd(&name, &s(" (no grant)")).map_err(at(l))?;
            }
            names.push(name);
        }
        let l = line!("print(\"%s: jobs go only to %s\"");
        let ws = p::get(&v, "workspace").map_err(at(l))?;
        let joined = p::join(", ", &Value::Array(names)).map_err(at(l))?;
        out.push_str(&format!("{}: jobs go only to {joined}\n", p::str_of(&ws)));
        out.push_str("  a node off the list is refused (node_not_allowed); --any, moves and retries stay on the list\n");
        let l = line!("if v.get(\"fixed\")");
        if p::truthy(&p::get(&v, "fixed").map_err(at(l))?) {
            out.push_str(concat!(
                "  fixed: a node newly added to this server does not join it (",
                cli_name!(),
                " compute allow-nodes --fixed off lets new nodes join)\n"
            ));
        } else {
            out.push_str(concat!(
                "  follows new nodes: a node newly added to this server joins it (",
                cli_name!(),
                " compute allow-nodes --fixed on stops that)\n"
            ));
        }
    } else {
        let l = line!("print(\"%s: no node list");
        let ws = p::get(&v, "workspace").map_err(at(l))?;
        out.push_str(&format!(
            "{}: no node list (its jobs may go to any node granted to it)\n",
            p::str_of(&ws)
        ));
    }
    let l = line!("if v.get(\"failedQueued\")");
    if p::truthy(&p::get(&v, "failedQueued").map_err(at(l))?) {
        let l = line!("print(\"  ended %d queued");
        let fq = p::idx(&v, "failedQueued").map_err(at(l))?;
        let n = p::len(&fq).map_err(at(l))?;
        let fq = p::idx(&v, "failedQueued").map_err(at(l))?;
        let mut short = vec![];
        for j in p::iter(&fq).map_err(at(l))? {
            short.push(p::slice(&j, None, Some(8)).map_err(at(l))?);
        }
        let joined = p::join(" ", &Value::Array(short)).map_err(at(l))?;
        out.push_str(&format!(
            "  ended {n} queued job(s) on nodes that left the list (failed/node_not_allowed): {joined}\n"
        ));
    }
    let l = line!("if v.get(\"withdrawn\")");
    if p::truthy(&p::get(&v, "withdrawn").map_err(at(l))?) {
        let l = line!("print(\"  asked %d job(s)");
        let w = p::idx(&v, "withdrawn").map_err(at(l))?;
        let text = p::pct(
            "  asked %d job(s) back from a shared node's queue (they never start there)",
            &[w],
        )
        .map_err(at(l))?;
        out.push_str(&text);
        out.push('\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        let Words::Ask(a) = parse(&["a", "b", "--add", "c", "--add", "d", "--json"]) else {
            panic!("not an ask");
        };
        assert_eq!(
            (a.nodes.as_str(), a.add.as_str(), a.json),
            ("a,b", "c,d", true)
        );
        assert_eq!(
            parse(&["--add"]),
            Words::Usage(format!("{USAGE} (--add needs a value)"))
        );
        assert_eq!(
            parse(&["--workspace", ""]),
            Words::Usage(format!("{USAGE} (--workspace needs a value)"))
        );
        assert_eq!(
            parse(&["-x"]),
            Words::Usage(format!("{USAGE} (unknown flag '-x')"))
        );
        // 0.46.18: --fixed on|off, nothing else.
        let Words::Ask(a) = parse(&["--fixed", "on"]) else {
            panic!("not an ask");
        };
        assert_eq!(a.fixed, Some(true));
        let Words::Ask(a) = parse(&["--fixed", "off", "--workspace", "w"]) else {
            panic!("not an ask");
        };
        assert_eq!((a.fixed, a.workspace.as_str()), (Some(false), "w"));
        assert_eq!(
            parse(&["--fixed", "yes"]),
            Words::Usage(format!("{USAGE} (--fixed takes on or off)"))
        );
        assert_eq!(
            parse(&["--fixed"]),
            Words::Usage(format!("{USAGE} (--fixed needs a value)"))
        );
        // `--add --json`: the flag's value, so no --json.
        let Words::Ask(a) = parse(&["--add", "--json"]) else {
            panic!("not an ask");
        };
        assert!(!a.json);
        assert_eq!(a.add, "--json");
    }

    #[test]
    fn the_body() {
        let a = Ask {
            nodes: "a".into(),
            clear: true,
            ..Ask::default()
        };
        assert_eq!(
            body(&a, "acme"),
            r#"{"workspace": "acme", "nodes": "a", "clear": true}"#
        );
        let a = Ask {
            fixed: Some(false),
            ..Ask::default()
        };
        assert_eq!(body(&a, "acme"), r#"{"workspace": "acme", "fixed": false}"#);
    }

    #[test]
    fn the_reader() {
        let r = reader(
            br#"{"change":"same","list":true,"workspace":"acme","nodes":[{"name":"mini","state":"paused","granted":true},{"id":"n9"},{"name":"big"}],"failedQueued":["0123456789ab"],"withdrawn":2}"#,
        );
        assert_eq!(
            r.stdout,
            "no change\nacme: jobs go only to mini (paused), n9 (removed), big (no grant)\n  a node off the list is refused (node_not_allowed); --any, moves and retries stay on the list\n  follows new nodes: a node newly added to this server joins it (k2 compute allow-nodes --fixed on stops that)\n  ended 1 queued job(s) on nodes that left the list (failed/node_not_allowed): 01234567\n  asked 2 job(s) back from a shared node's queue (they never start there)\n"
        );
        let r = reader(br#"{"list":true,"workspace":"acme","nodes":[{"name":"mini","granted":true}],"fixed":true}"#);
        assert!(
            r.stdout.ends_with("  fixed: a node newly added to this server does not join it (k2 compute allow-nodes --fixed off lets new nodes join)\n"),
            "{}",
            r.stdout
        );
        let r = reader(br#"{"list":true,"nodes":[{"name":5,"granted":true}]}"#);
        assert_eq!(r.exit, Exit::Error);
        assert!(r
            .stderr
            .ends_with("TypeError: sequence item 0: expected str instance, int found\n"));
    }
}
