//! `k5 capabilities [--json] [--node <name>]`: what this server can do for
//! Apps, as the shell CLI's `cmd_capabilities` prints it
//! (`cli/src/explain.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates, as for every verb that asks the daemon
//!    (`prelude.sh`). `capabilities` is the `id` tool `skin`, as `app` is:
//!    in a session, a stale passport and no passport at all are refused
//!    first ([`prelude::id_tool`]), and the session's passport is the
//!    credential.
//! 2. The words: `--json`; `--node <name>` (with no word after it, the
//!    usage line and exit 2; the word after it is the name, whatever it
//!    is); any other word is the usage line, exit 2.
//! 3. `_compute_call GET /cli/capabilities` with `node=<escaped>` when the
//!    name is not empty, and `_K2_REFUSAL_AREA=app`: a status but 200 is
//!    `_compute_fail`'s line with the App refusal registry's `fix:`
//!    ([`super::app::call`]).
//! 4. `--json`: the answer as it came and a newline. Else the shell's
//!    Python ([`lines`]): the box's rows, the compute nodes' rows and the
//!    box setup line, a Python 3.12 traceback where it raises.
//! 5. The EXIT trap's lines.
//!
//! The shell answers these itself ([`run`] returns `None`): a session with
//! `CODEX_*` set (the shared Codex lookup), a word that is not UTF-8,
//! `_CP_404_EXIT` set (which `_compute_fail` reads from the environment
//! for a 404), a port curl won't take, and a `python3` the binary can't
//! tell ([`crate::families`], Without Python). Help forms never get here:
//! the front prints `capabilities --help` and `capabilities -h` as pages
//! and hands any other line with a help flag to the shell.

use super::app::access::answer;
use super::app::call::{self, Ctx};
use super::app::py::{dget, dget_or, empty_list, idx, iter, join, left, or, s, text, truthy, Out};
use super::{curl_port, Raise};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::transport::encode::quote;
use crate::{out, passport, prelude, python};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["capabilities"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The route.
pub const ENDPOINT: &str = "/cli/capabilities";
/// The session check's tool (`_cli_tool_for_verb`).
pub const TOOL: &str = "skin";
/// The setting `_compute_fail` reads for a 404.
pub const CP_404_EXIT: &str = "_CP_404_EXIT";
/// The usage line.
pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " capabilities [--json] [--node <name>]"
);
/// The compute heading's tail when no node may be used.
const NO_NODES: &str = concat!(": none (", cli_name!(), " study compute)");
/// The box setup line's head.
const BOX_SETUP: &str = concat!("Box setup (", cli_name!(), " app doctor): ");
/// A missing capability's explain hint's head.
const EXPLAIN: &str = concat!(" (", cli_name!(), " explain ");

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Run { json: bool, node: String },
    /// The usage line, exit 2.
    Usage,
}

/// The words after `capabilities`, as `cmd_capabilities`' loop reads them.
/// `None` for a help flag, which only the front's routes reach.
pub fn parse(words: &[&str]) -> Option<Ask> {
    let (mut json, mut node) = (false, String::new());
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--json" => {
                json = true;
                i += 1;
            }
            "--node" => {
                let Some(name) = words.get(i + 1) else {
                    return Some(Ask::Usage);
                };
                node = name.to_string();
                i += 2;
            }
            "-h" | "--help" => return None,
            _ => return Some(Ask::Usage),
        }
    }
    Some(Ask::Run { json, node })
}

/// `k5 capabilities ...`. `None`: the shell's (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    if std::env::var_os(CP_404_EXIT).is_some() {
        return None;
    }
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let ask = parse(words.get(1..).unwrap_or_default())?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's.
        curl_port(settings.port_text.as_deref()?)?;
    }
    Some(run_with(args, &words, ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    ask: Ask,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let Ask::Run { json, node } = ask else {
        out::stderr_line(USAGE);
        return Exit::Usage;
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut ctx = Ctx::new(client, call::args_json(words));
    let q = if node.is_empty() {
        String::new()
    } else {
        format!("node={}", quote(&node))
    };
    let exit = match ctx.call(false, ENDPOINT, &q) {
        Ok(body) => answer(&body, json, lines).print(),
        Err(exit) => exit,
    };
    let Ctx {
        mut client,
        closing,
        ..
    } = ctx;
    closing.flush(&mut client, exit);
    exit
}

/// `cmd_capabilities`' Python on the answer (its lines, from 2):
///
/// ```text
///  2 import json, sys
///  3 v = json.load(sys.stdin)
///  4 def row(r):
///  5     mark = "ok" if r.get("ok") else "no"
///  6     print("  %-2s  %-22s %s" % (mark, r.get("cap", ""), r.get("detail") or ""))
///  7     m = r.get("missing")
///  8     if m:
///  9         code = (" (k2 explain %s)" % m["code"]) if m.get("code") else ""
/// 10         print("        %s: %s%s" % (m.get("reason", ""), m.get("fix", ""), code))
/// 11 print("This box")
/// 12 for r in v.get("box") or []:
/// 13     row(r)
/// 14 comp = v.get("compute") or []
/// 15 print()
/// 16 print("Compute nodes your workspace may use" + ("" if comp else ": none (k2 study compute)"))
/// 17 for r in comp:
/// 18     row(r)
/// 19 d = v.get("doctor") or {}
/// 20 if d:
/// 21     print()
/// 22     bad = ", ".join(d.get("failing") or [])
/// 23     print("Box setup (k2 app doctor): %s%s" % ("ready" if d.get("ready") else "not ready", (" — failing: " + bad) if bad else ""))
/// ```
pub fn lines(body: &[u8], o: &mut Out) -> Result<Exit, Raise> {
    let v = call_load(body)?;
    o.line("This box");
    for r in iter(&or(dget(&v, "box", 12)?, empty_list()), 12)? {
        row(o, &r).map_err(|e| in_row(13, e))?;
    }
    let comp = or(dget(&v, "compute", 14)?, empty_list());
    o.line("");
    let tail = if truthy(&comp) { "" } else { NO_NODES };
    o.line(&format!("Compute nodes your workspace may use{tail}"));
    for r in iter(&comp, 17)? {
        row(o, &r).map_err(|e| in_row(18, e))?;
    }
    let d = or(dget(&v, "doctor", 19)?, Value::Object(Vec::new()));
    if truthy(&d) {
        o.line("");
        let bad = join(", ", &or(dget(&d, "failing", 22)?, empty_list()), 22)?;
        let ready = if truthy(&dget(&d, "ready", 23)?) {
            "ready"
        } else {
            "not ready"
        };
        let failing = if bad.is_empty() {
            String::new()
        } else {
            format!(" \u{2014} failing: {bad}")
        };
        o.line(&format!("{BOX_SETUP}{ready}{failing}"));
    }
    Ok(Exit::Ok)
}

/// `json.load(sys.stdin)` at line 3.
fn call_load(body: &[u8]) -> Result<Value, Raise> {
    super::app::py::load(body, 3)
}

/// An exception raised in `row` (at its own line), called from `from`.
fn in_row(from: u32, e: Raise) -> Raise {
    let line = e.frames.last().map_or(0, |(l, _)| *l);
    Raise::inside(from, line, "row", e.exception)
}

/// `row(r)`, lines 5 to 10.
fn row(o: &mut Out, r: &Value) -> Result<(), Raise> {
    let mark = if truthy(&dget(r, "ok", 5)?) { "ok" } else { "no" };
    let cap = dget_or(r, "cap", text(""), 6)?;
    let detail = or(dget(r, "detail", 6)?, text(""));
    o.line(&format!("  {mark}  {} {}", left(&cap, 22), s(&detail)));
    let m = dget(r, "missing", 7)?;
    if truthy(&m) {
        let code = if truthy(&dget(&m, "code", 9)?) {
            format!("{EXPLAIN}{})", s(&idx(&m, "code", 9)?))
        } else {
            String::new()
        };
        let reason = dget_or(&m, "reason", text(""), 10)?;
        let fix = dget_or(&m, "fix", text(""), 10)?;
        o.line(&format!("        {}: {}{code}", s(&reason), s(&fix)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(body: &str) -> (String, String, Exit) {
        let s = answer(body.as_bytes(), false, lines);
        (s.stdout, s.stderr, s.exit)
    }

    #[test]
    fn the_words() {
        let run = |json, node: &str| {
            Some(Ask::Run {
                json,
                node: node.to_string(),
            })
        };
        assert_eq!(parse(&[]), run(false, ""));
        assert_eq!(parse(&["--json"]), run(true, ""));
        assert_eq!(parse(&["--node", "n1", "--json"]), run(true, "n1"));
        assert_eq!(parse(&["--node", "--json"]), run(false, "--json"));
        assert_eq!(parse(&["--node", ""]), run(false, ""));
        assert_eq!(parse(&["--node"]), Some(Ask::Usage));
        assert_eq!(parse(&["x"]), Some(Ask::Usage));
        assert_eq!(parse(&["--json", "-j"]), Some(Ask::Usage));
        assert_eq!(parse(&["x", "--help"]), Some(Ask::Usage));
        assert_eq!(parse(&["--help"]), None);
        assert_eq!(parse(&["--node", "-h"]), run(false, "-h"));
    }

    #[test]
    fn the_rows() {
        let body = r#"{"box":[{"ok":true,"cap":"podman","detail":"5.0"},
            {"cap":"tls","missing":{"reason":"no cert","fix":"add one","code":"tls_missing"}},
            {"ok":0,"cap":"x","missing":{"reason":"r"}}],
            "compute":[],"doctor":{"ready":false,"failing":["a","b"]}}"#;
        let (out, err, exit) = shown(body);
        let want = [
            "This box\n".to_string(),
            format!("  ok  {:<22} 5.0\n", "podman"),
            format!("  no  {:<22} \n", "tls"),
            format!("        no cert: add one{EXPLAIN}tls_missing)\n"),
            format!("  no  {:<22} \n", "x"),
            "        r: \n\n".to_string(),
            format!("Compute nodes your workspace may use{NO_NODES}\n\n"),
            format!("{BOX_SETUP}not ready \u{2014} failing: a, b\n"),
        ]
        .concat();
        assert_eq!((out.as_str(), err.as_str(), exit), (want.as_str(), "", Exit::Ok));
    }

    #[test]
    fn where_python_raises() {
        let (out, err, exit) = shown(r#"{"box":[1]}"#);
        assert_eq!(out, "This box\n");
        assert!(err.contains("line 13, in <module>\n"), "{err}");
        assert!(err.contains("line 5, in row\n"), "{err}");
        assert!(err.ends_with("AttributeError: 'int' object has no attribute 'get'\n"));
        assert_eq!(exit, Exit::Error);
        let (out, err, _) = shown(r#"{"doctor":[1]}"#);
        assert!(out.ends_with("\n\n"), "{out:?}");
        assert!(err.contains("line 22, in <module>"), "{err}");
        let (out, err, _) = shown("[]");
        assert_eq!(out, "This box\n");
        assert!(err.ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
        let (out, err, _) = shown("");
        assert_eq!(out, "");
        assert!(err.contains("JSONDecodeError"), "{err}");
    }

    /// Every name a line prints is the brand's (a `K2_BRAND_DIR=brand/xy`
    /// build says `xy`).
    #[test]
    fn the_lines_name_the_brand() {
        let cli = k2_client::brand::CLI_NAME;
        assert_eq!(USAGE, format!("usage: {cli} capabilities [--json] [--node <name>]"));
        let (out, _, _) = shown(r#"{"box":[{"missing":{"code":"c"}}],"doctor":{"ready":1}}"#);
        assert!(out.contains(&format!("({cli} explain c)")), "{out}");
        assert!(out.contains(&format!("({cli} study compute)")), "{out}");
        assert!(out.contains(&format!("Box setup ({cli} app doctor): ready")), "{out}");
        if cli != "k2" {
            assert!(!out.contains("k2") && !USAGE.contains("k2"), "{out}");
        }
    }
}
