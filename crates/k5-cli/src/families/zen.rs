//! `k5 zen`, as the shell CLI's router arm and `cmd_zen` answer it
//! (`cli/src/zen-retired.sh`, `cli/src/garden-data.sh`,
//! `cli/src/garden-access.sh`, `cli/src/zen-guide-data.sh`).
//!
//! Native: `zen data`, the person's Garden data admin (`cmd_zen_data`,
//! [`data`]), which shares its calls, flags and readers with `store
//! --widget` ([`super::store::widget`]); `zen` alone and `zen help`; the
//! arms that only print (`pages`, `grant`, an unknown verb); and
//! `cmd_zen`'s own verbs ([`verbs`]): `validate`, `history`, `reload`,
//! `reset`, `doctor` and `path`, printed by `_zen_print` ([`print`]);
//! `garden` (with Garden access, History and `put`) and `proposals`
//! ([`garden`]); `theme`, `sync`, `news`, `links`, `agents-order` and
//! `inspect` ([`more`]).
//!
//! Still the shell's ([`run`] returns `None` before anything is printed
//! or sent): a line with `-h` or `--help` (the router's help arms), the
//! verbs not yet listed above, and the edge hand-backs every family keeps
//! (a word that is not UTF-8, `HOOK_SOCK` with a `CODEX_` variable, a
//! port curl won't take, a `python3` the binary can't tell); `validate
//! --dir` when the temp folder can't be used (`mktemp`'s own line).
//!
//! What runs, in the shell's order: the settings and the two gates (`zen`
//! alone and `zen help` skip the connection gate, not the virtual
//! desktop's guard); then the router arm's `_zen_garden_changes_hint`
//! ([`changes_hint`]); then the verb, its requests ([`call`]) and its
//! printer; then the EXIT trap's lines. `zen` is no id tool: no passport
//! check before the call.
//!
//! The printers get the answer in an environment variable. An answer
//! above what one may hold (128 KiB on Linux) stops bash there
//! ("Argument list too long", exit 126); the binary prints it. That is
//! the named difference `zen-large-answer` (`scripts/cli-parity.sh`).

pub mod call;
pub mod verbs;
pub mod data;
pub mod print;
pub mod sign;

use super::curl_port;
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use crate::transport::Auth;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["zen"];
/// The binary answers what [`run`] takes; the rest goes to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The verb the binary answers, if it does.
fn native(words: &[&str]) -> bool {
    let sub = words.first().copied().unwrap_or("");
    match sub {
        "data" => !data::shells(&words[1..]),
        "" | "help" | "pages" | "grant" | "grants" | "allow" => true,
        s if verbs::is_core(s) => {
            !(s == "validate"
                && words.iter().any(|w| *w == "--dir" || w.starts_with("--dir="))
                && crate::transport::client::tmpdir_unusable())
        }
        "garden" | "gardens" => {
            !(words.get(1) == Some(&"new") && crate::transport::client::tmpdir_unusable())
        }
        "widget" | "widgets" | "guide" => false,
        _ => true,
    }
}

/// `k5 zen …`: `args` is the whole command line after `argv[0]`. `None`:
/// the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) || !native(&words) {
        return None;
    }
    // A `_K2_NOANSWER_SOFT` from outside changes `_compute_send`.
    if std::env::var_os("_K2_NOANSWER_SOFT").is_some_and(|v| !v.is_empty()) {
        return None;
    }
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
    let sub = words.first().copied().unwrap_or("");
    // The prelude skips the connection gate for `zen` alone and `zen
    // help`, not the virtual desktop's guard.
    if matches!(sub, "" | "help") {
        if settings.no_owner_fallback && settings.auth == Auth::None {
            return Some(Gate::VirtualDesktop.stop());
        }
        changes_hint(&words);
        return Some(verbs::help(false, Exit::Ok));
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    settings.port_text.as_deref().and_then(curl_port)?;
    let would_refuse = passport::current(args).would_refuse();
    changes_hint(&words);
    if sub == "data" {
        let mut client = match settings.client(would_refuse) {
            Ok(c) => c,
            Err(e) => {
                out::stderr_line(&format!("Error: {e}"));
                return Some(Exit::Error);
            }
        };
        return Some(data::run(&mut client, &words[1..]));
    }
    if let Some(exit) = verbs::only_prints(sub) {
        return Some(exit);
    }
    let rest = &words[1..];
    let mut z = call::Zen::new(settings, would_refuse);
    let step = match sub {
        "garden" | "gardens" => garden::garden(&mut z, rest),
        "proposals" | "proposal" => garden::proposals(&mut z, rest),
        "theme" | "themes" => more::theme(&mut z, rest),
        "sync" => more::sync(&mut z, rest),
        "news" => more::news(&mut z, rest),
        "links" => more::links(&mut z, rest),
        "agents-order" => more::agents_order(&mut z, rest),
        "inspect" => more::inspect(&mut z, rest),
        s if verbs::is_core(s) => verbs::run(&mut z, verbs::plan(s, rest)),
        _ => Ok(verbs::unknown(sub)),
    };
    let exit = match step {
        Ok(e) | Err(e) => e,
    };
    Some(z.finish(exit))
}

/// `_ws_dot_dir ROOT`.
fn dot_dir(root: &str) -> PathBuf {
    let k2 = Path::new(root).join(".k2");
    if k2.is_dir() {
        k2
    } else if Path::new(root).join(".k2so").is_dir() || k2.exists() {
        Path::new(root).join(".k2so")
    } else {
        k2
    }
}

/// `_zen_garden_changes_hint WORDS…`: once per workspace, the line about
/// Gardens' access lists, with a marker in that workspace's own `.k2`.
/// Never with `--json`; best effort.
pub fn changes_hint(words: &[&str]) {
    if words
        .iter()
        .any(|w| *w == "--json" || w.starts_with("--json="))
    {
        return;
    }
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let Some(root) = var("K5_PROJECT_PATH")
        .or_else(|| var("K2_PROJECT_PATH"))
        .or_else(|| var("K2SO_PROJECT_PATH"))
    else {
        return;
    };
    if std::env::var("HOME").ok().as_deref() == Some(root.as_str()) {
        return;
    }
    let dot = dot_dir(&root);
    if !dot.is_dir() {
        return;
    }
    // The prelude's `K2_HOME`: `$HOME/.k2`, else `$HOME/.k2so`.
    if let Some(h) = std::env::var_os("HOME").map(PathBuf::from) {
        let home = if h.join(".k2").is_dir() {
            h.join(".k2")
        } else {
            h.join(".k2so")
        };
        if home.is_dir() {
            let same = match (std::fs::canonicalize(&dot), std::fs::canonicalize(&home)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if same {
                return;
            }
        }
    }
    let marker = dot.join(".garden-changes-hint");
    if marker.exists() {
        return;
    }
    if std::fs::write(&marker, "0.46.3\n").is_err() {
        return;
    }
    out::stderr_line(concat!(
        "Gardens changed in 0.46.3: see `",
        cli_name!(),
        " study garden-changes` (update before 0.47.0)"
    ));
}
