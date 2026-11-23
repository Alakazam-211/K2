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
//! `inspect` ([`more`]); `widget` and the widget dev loop ([`widget`]),
//! with `preview --shot` and its Node runner ([`shot`]); `guide`, its
//! static pages ([`guide`]).
//!
//! Still the shell's ([`run`] returns `None` before anything is printed
//! or sent): a line with `-h` or `--help` before any bare `--` (the
//! router's help arms), the
//! shot lines [`shot::shells`] names (no runner beside the program, and
//! the like), and the edge hand-backs every family keeps
//! (a word that is not UTF-8, `HOOK_SOCK` with a `CODEX_` variable, a
//! port curl won't take, a `python3` the binary can't tell); a line that
//! would run `mktemp` where the temp folder can't be used, when there is
//! no `mktemp` on `PATH` the binary can run as the shell does. With one,
//! the binary runs it there, so its line is the shell's ([`temp_fails`]).
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
//! the named difference `env-answer-large` (`scripts/cli-parity.sh`).

pub mod call;
pub mod data;
pub mod garden;
pub mod guide;
pub mod more;
pub mod print;
pub mod shot;
pub mod sign;
pub mod verbs;
pub mod widget;

use super::curl_port;
use crate::daemon;
use crate::exit::Exit;
use crate::offline::Env;
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
        "" | "data" | "help" | "pages" | "grant" | "grants" | "allow" => true,
        s if verbs::is_core(s) => {
            !(s == "validate"
                && words
                    .iter()
                    .any(|w| *w == "--dir" || w.starts_with("--dir="))
                && temp_shell())
        }
        "garden" | "gardens" => !(words.get(1) == Some(&"new") && temp_shell()),
        "widget" | "widgets" => {
            let w = &words[1..];
            let dir = w.iter().any(|a| *a == "--dir" || a.starts_with("--dir="))
                || w.first() == Some(&"apply");
            !widget::shells(w) && !(dir && temp_shell())
        }
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
    // The router's `_wants_help` scans stop at a bare `--` (`garden`
    // takes one).
    if crate::dashdash::wants_help(&words) || !native(&words) {
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
    if matches!(sub, "" | "help" | "guide") {
        if settings.no_owner_fallback && settings.auth == Auth::None {
            return Some(Gate::VirtualDesktop.stop());
        }
        changes_hint(&words);
        if sub == "guide" {
            return Some(guide::guide(&words[1..]).print());
        }
        return Some(verbs::help(false, Exit::Ok));
    }
    let widget_no_gate = matches!(sub, "widget" | "widgets") && widget::no_gate(&words[1..]);
    if widget_no_gate {
        // These print only; no request, so no port is needed.
        if settings.no_owner_fallback && settings.auth == Auth::None {
            return Some(Gate::VirtualDesktop.stop());
        }
        changes_hint(&words);
        let mut z = call::Zen::new(settings, false);
        let exit = match widget::widget(&mut z, &words[1..]) {
            Ok(e) | Err(e) => e,
        };
        return Some(z.finish(exit));
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
        "widget" | "widgets" => widget::widget(&mut z, rest),
        s if verbs::is_core(s) => verbs::run(&mut z, verbs::plan(s, rest)),
        _ => Ok(verbs::unknown(sub)),
    };
    let exit = match step {
        Ok(e) | Err(e) => e,
    };
    Some(z.finish(exit))
}

/// A line that may run `mktemp` where `${TMPDIR:-/tmp}` can't be used,
/// with no `mktemp` the shell would run as the binary runs it
/// ([`super::update::shell_program`]): the shell's.
fn temp_shell() -> bool {
    crate::transport::client::tmpdir_unusable()
        && super::update::shell_program(&crate::offline::Env::current(), "mktemp").is_none()
}

/// `x="$(mktemp [-d] "${TMPDIR:-/tmp}/<name>XXXXXX")"` failing where
/// `${TMPDIR:-/tmp}` can't be used: the system's `mktemp` (the one on
/// `PATH`, as the shell runs it) says why on stderr in its own words, then
/// the shell's `line`, exit 1. `None` when the folder can be used (the
/// binary needs no file there), or when `mktemp` made one after all (it
/// goes again).
pub fn temp_fails(folder: bool, name: &str, line: &str) -> Option<crate::families::Shown> {
    if !crate::transport::client::tmpdir_unusable() {
        return None;
    }
    let mut template = std::env::var_os("TMPDIR")
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| OsString::from("/tmp"));
    template.push("/");
    template.push(name);
    template.push("XXXXXX");
    let mut cmd = std::process::Command::new("mktemp");
    if folder {
        cmd.arg("-d");
    }
    let made = cmd
        .arg(&template)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .output()
        .ok()
        .filter(|o| o.status.success());
    if let Some(o) = made {
        let text = String::from_utf8_lossy(&o.stdout);
        let path = Path::new(text.trim_end_matches('\n'));
        let _ = if folder {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        return None;
    }
    Some(crate::families::Shown::err(
        format!("{line}\n"),
        Exit::Error,
    ))
}

/// `_zen_dir_call`'s and the shot's line when `mktemp` fails.
pub const NO_TEMP_FILE: &str = "Error: can't make a temp file";

/// `_ws_dot_dir ROOT`: `.k2` when it is a folder, else `.k2so` when that
/// is one (or a `.k2` file is in the way), else `.k2`.
fn dot_dir(root: &Path) -> PathBuf {
    crate::home::workspace_dot_dir(root)
}

/// `_zen_garden_changes_hint WORDS…`: once per workspace, the line about
/// Gardens' access lists, with a marker in that workspace's own `.k2`.
/// Never with `--json`; best effort.
pub fn changes_hint(words: &[&str]) {
    let _ = changes_hint_in(&Env::current(), words);
}

/// [`changes_hint`] in `env`. The router arm runs it before every `zen`
/// line, its help too ([`crate::pages::help_page`]). `Err` only where the
/// line could not be written to stderr: the shell's `echo` fails there,
/// the function with it, and `set -e` ends the run with 1.
///
/// The shell's steps: the workspace is the first of `K5_PROJECT_PATH`,
/// `K2_PROJECT_PATH` and `K2SO_PROJECT_PATH` that is set and not empty,
/// and not `$HOME`; its dot folder (`_ws_dot_dir`) must be a folder and
/// not the K2 home (`$HOME/.k2`, else `$HOME/.k2so`, compared by real
/// path); the marker `.garden-changes-hint` must not exist (`[ ! -e ]`,
/// through a link). Then `0.46.3` and a newline go to the marker, and only
/// when that write worked, the line goes to stderr.
pub fn changes_hint_in(env: &Env, words: &[&str]) -> std::io::Result<()> {
    if words
        .iter()
        .any(|w| *w == "--json" || w.starts_with("--json="))
    {
        return Ok(());
    }
    let Some(root) = env.first(&["K5_PROJECT_PATH", "K2_PROJECT_PATH", "K2SO_PROJECT_PATH"]) else {
        return Ok(());
    };
    if env.get("HOME") == Some(root) {
        return Ok(());
    }
    let dot = dot_dir(Path::new(root));
    if !dot.is_dir() {
        return Ok(());
    }
    if let Some(home) = crate::offline::shell_k2_home(env) {
        if home.is_dir() {
            let same = match (std::fs::canonicalize(&dot), std::fs::canonicalize(&home)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if same {
                return Ok(());
            }
        }
    }
    let marker = dot.join(".garden-changes-hint");
    if marker.exists() {
        return Ok(());
    }
    if std::fs::write(&marker, "0.46.3\n").is_err() {
        return Ok(());
    }
    out::write_flushed(
        &mut std::io::stderr().lock(),
        concat!(
            "Gardens changed in 0.46.3: see `",
            cli_name!(),
            " study garden-changes` (update before 0.47.0)\n"
        )
        .as_bytes(),
    )
}
