//! The `compute` admin verbs, as the shell CLI's `cmd_compute` answers
//! them (`cli/src/compute-command.sh` and the files it calls). Made native
//! one verb at a time:
//!
//! - [`switch`]: `revoke <node> --workspace WS`, `preview on|off`,
//!   `placement v1|v2`, `access on|off`, `operate on|off` and `manage
//!   on|off` (the owner's switches; `preview` and `placement` with no word
//!   or `status` are the read verbs').
//! - [`retier`]: `retier|tier <job> --tier T [--why …]`.
//! - [`moves`]: `move <job>... --auto|--to <node> [...]` and `move
//!   --all-mine|--workspace-all [...]`.
//! - [`cache`]: `cache [list] [...]`, `cache --lineage L` and `cache
//!   clear`.
//! - [`node`]: the `node` subverbs made native so far.
//! - [`setup`]: `compute` alone (the help page), `allow-nodes`,
//!   `git-access`, `lanes`, `local` and `attach`.
//!
//! What runs is what [`super::writes`] runs, in the shell's order: the
//! settings, a re-minted passport, the two gates, the `id` tool's lines,
//! the verb's words (usage: exit 2; a `shift 2` with one word left: exit
//! 1, nothing printed), then `_compute_call` ([`Cp::call`]) and the
//! verb's reader, then the EXIT trap's lines.
//!
//! What the binary hands to the shell, before it prints or sends anything,
//! is what [`super::writes::run`] hands it, and every form not made native
//! here.
//!
//! This driver is the same as `writes`' (its `run` and `run_with`); the
//! two can share it once the compute lanes have landed.

use super::writes::{self, Cp, Words};
use crate::daemon;
use crate::exit::Exit;
use crate::families::{curl_port, Raise};
use crate::out::json::{Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use std::ffi::OsString;

pub mod cache;
pub mod moves;
pub mod node;
pub mod retier;
pub mod setup;
pub mod switch;

#[cfg(test)]
mod tests;

/// The second words this module answers (some forms of `preview` and
/// `placement` are [`super::reads`]').
pub const ANSWERS: &[&str] = &[
    "revoke",
    "preview",
    "placement",
    "access",
    "operate",
    "manage",
    "retier",
    "tier",
    "move",
    "cache",
    "node",
    "allow-nodes",
    "git-access",
    "lanes",
    "local",
    "attach",
];

/// A verb this module answers, with its words read.
enum Ask {
    Switch(switch::Ask),
    Retier(retier::Ask),
    Move(moves::Ask),
    Cache(cache::Ask),
    Node(node::Ask),
    Setup(setup::Ask),
}

/// Which verb: `None` for a form the binary leaves to another module or
/// to the shell.
#[derive(Clone, Copy)]
enum Which {
    Switch,
    Retier,
    Move,
    Cache,
    Node,
    Setup,
}

fn which(words: &[&str]) -> Option<Which> {
    if setup::answers(words) {
        return Some(Which::Setup);
    }
    let first = words.get(1).copied().unwrap_or("");
    match *words.first()? {
        "revoke" | "access" | "operate" | "manage" => Some(Which::Switch),
        "preview" if matches!(first, "on" | "off") => Some(Which::Switch),
        "placement" if matches!(first, "v1" | "v2") => Some(Which::Switch),
        "retier" | "tier" => Some(Which::Retier),
        "move" => Some(Which::Move),
        "cache" => Some(Which::Cache),
        "node" => Some(Which::Node),
        _ => None,
    }
}

/// `k5 compute <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: another module or the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let which = which(&words)?;
    // The node subverbs not made native are the shell's.
    let node_words = match which {
        Which::Node => Some(node::parse(&words[1..])?),
        _ => None,
    };
    let setup_words = match which {
        Which::Setup => Some(setup::parse(&words)?),
        _ => None,
    };
    let max_time = writes::post_max_time(std::env::var_os("K2_CP_MAX_TIME"))?;
    if writes::shell_settings(std::env::vars_os()) {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let gate = prelude::gate(&settings);
    if gate.is_none() && settings.port_text.as_deref().and_then(curl_port).is_none() {
        return None;
    }
    // `compute` alone only prints the help page: the shell skips its
    // connection gate for it (`cli/src/cli-tools-policy.sh`), not the
    // virtual desktop's guard.
    let help_only = matches!(setup_words, Some(Words::Ask(setup::Ask::Usage)));
    if let Some(gate) = gate.filter(|g| !(help_only && *g == prelude::Gate::CannotConnect)) {
        return Some(gate.stop());
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) =
        prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, writes::TOOL)
    {
        return Some(stop);
    }
    // Nothing to send, and no client to build with no port or token.
    if help_only {
        return Some(setup::usage());
    }
    // `compute` alone has no words after the verb.
    let rest = words.get(1..).unwrap_or_default();
    let ask = match which {
        Which::Switch => ready(switch::parse(words[0], rest)).map(Ask::Switch),
        Which::Retier => ready(retier::parse(rest)).map(Ask::Retier),
        Which::Move => ready(moves::parse(rest)).map(Ask::Move),
        Which::Cache => {
            ready(cache::parse(rest, writes::in_session(&settings.auth))).map(Ask::Cache)
        }
        Which::Node => ready(node_words.expect("parsed above")).map(Ask::Node),
        Which::Setup => ready(setup_words.expect("parsed above")).map(Ask::Setup),
    };
    let ask = match ask {
        Ok(a) => a,
        Err(exit) => return Some(exit),
    };
    let vault_sock = crate::confirm::sock_path(&settings.home);
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    client.compute_post_max_time = max_time;
    let as_json = words.iter().skip(1).any(|w| *w == "--json");
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json,
    };
    let exit = match &ask {
        Ask::Switch(a) => switch::send(&mut cp, a),
        Ask::Retier(a) => retier::send(&mut cp, a),
        Ask::Move(a) => moves::send(&mut cp, a),
        Ask::Cache(a) => cache::send(&mut cp, a),
        Ask::Node(a) => node::send(&mut cp, a, &vault_sock),
        Ask::Setup(a) => setup::send(&mut cp, a),
    };
    let closing = std::mem::take(&mut cp.closing);
    closing.flush(&mut client, exit);
    Some(exit)
}

/// `{word: text}.get(p, p)` on a value `json.load` gave: the table's text
/// for a string in it, else `str(p)`; a list or dict raises (unhashable).
pub fn table_get(table: &[(&str, &str)], p: &Value, line: u32) -> Result<String, Raise> {
    match p {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(p)),
        )),
        Value::Str(t) => Ok(table
            .iter()
            .find(|(k, _)| *t == Text::from(*k))
            .map_or_else(|| py::str_of(p), |(_, text)| text.to_string())),
        other => Ok(py::str_of(other)),
    }
}

/// `"text" + x`: `x` must be a string.
pub fn str_plus(text: &str, x: &Value, line: u32) -> Result<String, Raise> {
    match x {
        Value::Str(_) => Ok(format!("{text}{}", py::str_of(x))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// The ask, or what stopping prints and the exit.
fn ready<T>(w: Words<T>) -> Result<T, Exit> {
    match w {
        Words::Ask(a) => Ok(a),
        Words::Usage(line) => {
            out::stderr_line(&line);
            Err(Exit::Usage)
        }
        Words::ShiftFailed(flag) => Err(out::stop_need_value(&flag)),
    }
}
