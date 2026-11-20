//! A board verb on a `.k5board` file through `doc` (`_doc_board_call` and
//! `_doc_board`, `cli/src/doc.sh`): doc's flags that mean the same for a
//! board (`-w` and `--workspace` as `--workspace`), then `cmd_board`'s
//! answer from the board family's planner and readers
//! ([`crate::families::board`]), on doc's client: the line has passed
//! doc's gate (the `docs` tool), as the shell runs `cmd_board` there.
//! `cmd_doc` has already taken a line with `-h` or `--help` to doc's help.

use super::{Ctx, Step};
use crate::daemon;
use crate::exit::Exit;
use crate::families::board::{self as b, file, garden, Target};
use crate::out;

/// `_DOC_BOARD_VERBS`: the verbs `cmd_doc` hands to `cmd_board` on a
/// `.k5board` file.
pub const VERBS: &[&str] = &[
    "new",
    "list",
    "read",
    "search",
    "fields",
    "templates",
    "references",
    "add",
    "update",
    "delete",
    "put-back",
    "connect",
    "react",
    "vote",
    "comment",
    "reply",
    "resolve",
    "meta",
    "lock",
    "check-add",
    "check-set",
    "check-remove",
    "table-cell",
    "table-add-row",
    "table-remove-row",
    "table-add-column",
    "table-remove-column",
    "move",
    "order",
    "frame-order",
    "group",
    "ungroup",
    "align",
    "distribute",
    "tidy",
    "copy-style",
    "duplicate",
    "paste",
    "connected-copy",
    "mind-child",
    "template",
    "clear-blob",
    "apply",
    "inverse",
    "merge",
    "explain",
    "capabilities",
];

/// `_doc_board_call`: a board verb with a word after it that ends
/// `.k5board` (any case; `$(…)` drops its trailing newlines) and doesn't
/// start `--`.
pub fn call<S: AsRef<str>>(words: &[S]) -> bool {
    let Some((verb, rest)) = words.split_first() else {
        return false;
    };
    VERBS.contains(&verb.as_ref())
        && rest.iter().any(|a| {
            let l = a.as_ref().trim_end_matches('\n').to_ascii_lowercase();
            !l.starts_with("--") && l.ends_with(".k5board")
        })
}

/// `_doc_board VERB ARGS...`, then `cmd_board VERB REST...`.
pub fn run(ctx: &mut Ctx, verb: &str, words: &[String]) -> Step {
    let mut rest = vec![verb.to_string()];
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "-w" | "--workspace" => {
                let v = ctx.need(words, i, &format!("{a} needs a workspace"))?;
                rest.push("--workspace".into());
                rest.push(v);
                i += 1;
            }
            _ => match a.strip_prefix("--workspace=") {
                Some(v) => {
                    rest.push("--workspace".into());
                    rest.push(v.to_string());
                }
                None => rest.push(a.to_string()),
            },
        }
        i += 1;
    }
    cmd_board(ctx, &rest)
}

/// `cmd_board` past its help (`cmd_doc` took that): the path last, then
/// a `.k5board` file's request and reader, or a Garden board's.
fn cmd_board(ctx: &mut Ctx, words: &[String]) -> Step {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    let stdin = || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut buf);
        buf
    };
    match b::target(&words, |w| !w.is_empty() && cwd.join(w).is_file()) {
        Target::TwoTargets => {
            out::stderr_line(b::TWO_TARGETS);
            Err(Exit::Usage)
        }
        Target::File(words) => {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let as_json = words.contains(&"--json");
            let pwd = daemon::logical_cwd(std::env::var_os("PWD")).unwrap_or_else(|_| cwd.clone());
            let plan = match file::plan(&pwd.to_string_lossy(), &words, stdin, b::uuid4_hex) {
                Ok(p) => p,
                Err(stop) => return Err(stop.print()),
            };
            let sent = b::send(ctx.client(), plan.post, plan.route, &plan.payload)?;
            Ok(file::answer(&plan, &sent, as_json, &b::random_hex(8)).print())
        }
        Target::Garden { words, named } => {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let as_json = words.contains(&"--json");
            let ask = match garden::parse(&words, stdin) {
                Ok(a) => a,
                Err(stop) => return Err(stop.print()),
            };
            let sent = b::send(ctx.client(), true, garden::ENDPOINT, &garden::payload(&ask))?;
            Ok(garden::answer(&sent, named, as_json, &ask.out, &cwd).print())
        }
    }
}
