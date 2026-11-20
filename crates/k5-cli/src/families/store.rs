//! `k5 store`, as the shell CLI's `cmd_store` answers it
//! (`cli/src/board.sh`): a workspace's document collections, through
//! `_db_py` ([`super::db::py`]).
//!
//! What runs, in the shell's order: the settings, the gates and the
//! `store` id tool's check (as for [`super::db`]); no words, the help
//! page; `--workspace`/`--project` anywhere on the line
//! ([`super::db::take_workspace`]); then the verb (`create`, `list`,
//! `put`, `get`, `query`, `rm`, `drop`) and one call. Every verb but
//! `rm` and `drop` takes `--json`; those two read `D_JSON` from the
//! caller's environment, as the shell's `_db_py` does.
//!
//! The binary hands these to the shell before it prints or sends
//! anything: a line with `--widget` anywhere (Garden data,
//! `cmd_store_widget`, shared with `zen data`), and the cases
//! [`super::db`] hands back.

use super::db::py::{err_line, Ask, Verb};
use super::db::{self, Form, Line, Words};
use crate::exit::Exit;
use crate::out;
use crate::prelude::Closing;
use k2_client::cli_name;
use std::ffi::OsString;

#[cfg(test)]
mod tests;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["store"];
/// The binary answers; a form [`run`] hands back goes to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "store";

/// `D_JSON` as the shell's `_db_py` reads it when the verb sets none.
fn env_json() -> bool {
    std::env::var_os("D_JSON").is_some_and(|v| v == "true")
}

/// `cmd_store`'s words, after `store`; `env_json` is the caller's
/// `D_JSON`.
pub fn parse(words: &[&str], env_json: bool) -> Line {
    if words.is_empty() || words.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return Line {
            sels: Vec::new(),
            form: Form::Help,
        };
    }
    if words.contains(&"--widget") {
        return Line {
            sels: Vec::new(),
            form: Form::Shell,
        };
    }
    let taken = db::take_workspace(words);
    let form = match taken.error {
        Some(e) => Form::Usage(e),
        None if taken.rest.is_empty() => Form::Help,
        None => verb(&taken.rest, env_json).unwrap_or_else(|f| f),
    };
    Line {
        sels: taken.sels,
        form,
    }
}

fn unexpected(w: &str) -> Form {
    Form::Usage(format!("unexpected '{w}'"))
}

/// `while [ $# -gt 0 ]; do case "$1" in --json) …; *) unexpected; esac; done`.
fn json_only(a: &mut Words, ask: &mut Ask) -> Result<(), Form> {
    while let Some(w) = a.first() {
        match w {
            "--json" => ask.json = true,
            _ => return Err(unexpected(w)),
        }
        a.shift(1);
    }
    Ok(())
}

fn verb(words: &[&str], env_json: bool) -> Result<Form, Form> {
    let sub = words[0];
    let mut a = Words::new(&words[1..]);
    let mut k = Ask::new(match sub {
        "create" => Verb::StoreCreate,
        "list" => Verb::StoreList,
        "put" => Verb::StorePut,
        "get" => Verb::StoreGet,
        "query" => Verb::StoreQuery,
        "rm" => Verb::StoreRm,
        "drop" => Verb::StoreDrop,
        _ => {
            return Ok(Form::Usage(format!(
                concat!(
                    "unknown store command '{}' — see ",
                    cli_name!(),
                    " store --help"
                ),
                sub
            )))
        }
    });
    match sub {
        "create" => {
            k.name = a.at(0).to_string();
            a.shift(1);
            json_only(&mut a, &mut k)?;
        }
        "list" => json_only(&mut a, &mut k)?,
        "put" => {
            k.name = a.at(0).to_string();
            a.shift(1);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        let v = a.at(1);
                        if !v.is_empty() && !v.starts_with('-') {
                            k.doc_json = v.to_string();
                            a.shift(2);
                        } else {
                            k.json = true;
                            a.shift(1);
                        }
                    }
                    "--id" => k.doc_id = a.value()?.to_string(),
                    _ => return Err(unexpected(w)),
                }
            }
        }
        "get" => {
            k.name = a.at(0).to_string();
            k.doc_id = a.at(1).to_string();
            a.shift(2);
            json_only(&mut a, &mut k)?;
        }
        "query" => {
            k.name = a.at(0).to_string();
            a.shift(1);
            while let Some(w) = a.first() {
                match w {
                    "--json" => {
                        k.json = true;
                        a.shift(1);
                    }
                    "--limit" => k.limit = a.value()?.to_string(),
                    "--where" => {
                        a.value()?;
                    }
                    _ => return Err(unexpected(w)),
                }
            }
        }
        "rm" => {
            k.name = a.at(0).to_string();
            k.doc_id = a.at(1).to_string();
            k.json = env_json;
        }
        _ => {
            k.name = a.at(0).to_string();
            k.json = env_json;
        }
    }
    Ok(Form::Py(Box::new(k)))
}

/// `k5 store …`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    if db::codex_session() {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let line = parse(&words, env_json());
    let settings = match db::settings_for(&line.sels, line.form == Form::Shell)? {
        Ok(s) => s,
        Err(e) => return Some(e),
    };
    let would_refuse = match db::prelude_for(args, &settings, TOOL) {
        Ok(w) => w,
        Err(e) => return Some(e),
    };
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let mut closing = Closing::default();
    let mut project = settings.project.clone();
    for sel in &line.sels {
        db::apply_project(&mut client, &mut closing, &mut project, sel);
    }
    client.project = project.clone();
    let exit = match line.form {
        Form::Py(ask) => {
            let port_text = settings.port_text.clone().unwrap_or_default();
            return Some(db::run_py(
                &mut client,
                closing,
                &project,
                &ask,
                &port_text,
                &settings.host,
            ));
        }
        Form::Help => db::help_page("store"),
        Form::Usage(hint) => {
            out::stderr_line(&err_line("usage", &hint));
            Exit::Usage
        }
        Form::Shift => Exit::Error,
        Form::Home { .. } | Form::Cap { .. } | Form::Shell => {
            unreachable!("store has no such form")
        }
    };
    closing.flush(&mut client, exit);
    Some(exit)
}
