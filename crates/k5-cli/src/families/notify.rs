//! `k5 notify`: an agent's email notice to a person here, as the shell
//! CLI's `cmd_notify` and `cmd_notify_email` (`cli/src/app-auth.sh`).
//!
//! 1. The settings and the gates ([`super::notifications::start`]); an
//!    open tool.
//! 2. `notify` alone or `help`, and `notify email` alone or `help`: the
//!    help. `email off` (this workspace's notices off), `email mute|unmute
//!    <workspace>` (over TCP), and `email <login|owner> --subject s --text
//!    t|--text-file f`: one POST each (`_app_rt_post`, a 202 is an answer
//!    too for a notice), printed by the shell's Python.
//! 3. The EXIT trap's lines.
//!
//! A flag that takes a value and comes last ends the command with 1 and
//! no line, as the shell's `shift 2` does. `-h` and `--help` are the
//! front's.

use super::app::py as p;
use super::approve::cp::{self, env_text, one_line, page, start, stdout_line, usage};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["notify"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 notify ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let mut r = match start(args) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let exit = match words.first().copied().unwrap_or("") {
        "email" => email(&mut r.ctx, &words[1..]),
        "" | "help" => page("notify"),
        _ => usage(concat!(
            "usage: ",
            cli_name!(),
            " notify email \u{2026} (",
            cli_name!(),
            " notify --help)"
        )),
    };
    Some(cp::close(r.ctx, exit))
}

fn dumps(pairs: Vec<(&str, Value)>) -> String {
    let v = Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect());
    json::to_string(&v, &Options::default()).expect("json.dumps of JSON values")
}

fn email(ctx: &mut super::app::call::Ctx, words: &[&str]) -> Exit {
    let first = words.first().copied().unwrap_or("");
    match first {
        "" | "help" => return page("notify"),
        "off" => {
            if words.len() != 1 {
                return usage(concat!("usage: ", cli_name!(), " notify email off"));
            }
            return match cp::rt_post_ok(ctx, "/cli/notify/email/off", "{}", &["200"]) {
                Ok(_) => stdout_line(
                    "email notices off for this workspace (the owner turns them back on)",
                ),
                Err(e) => e,
            };
        }
        "mute" | "unmute" => {
            let ws = match words {
                [_, ws] if !ws.is_empty() => *ws,
                _ => {
                    return usage(&format!(
                        concat!("usage: ", cli_name!(), " notify email {} <workspace>"),
                        first
                    ))
                }
            };
            let body = dumps(vec![
                ("workspace", p::text(ws)),
                ("muted", Value::Bool(first == "mute")),
            ]);
            let s = match ctx.send(true, "/cli/notify/email/mute", &body, true) {
                Ok(s) => s,
                Err(e) => return e,
            };
            if s.status != "200" {
                return cp::fail(&s);
            }
            return one_line(&s.body, |v| {
                let muted = if p::truthy(&p::dget(v, "muted", 1)?) {
                    "muted"
                } else {
                    "unmuted"
                };
                Ok(format!(
                    "{muted} email notices from {}",
                    py::str_of(&p::dget(v, "workspace", 1)?)
                ))
            })
            .print();
        }
        _ => {}
    }
    let person = first;
    let (mut subject, mut text, mut text_file): (&str, Vec<u8>, &str) = ("", Vec::new(), "");
    let mut have_text = false;
    let mut i = 1;
    while i < words.len() {
        let w = words[i];
        if !matches!(w, "--subject" | "--text" | "--text-file") {
            return usage(&format!(
                concat!("Unknown argument: {} (", cli_name!(), " notify --help)"),
                w
            ));
        }
        let Some(v) = words.get(i + 1) else {
            return Exit::Error;
        };
        match w {
            "--subject" => subject = v,
            "--text" => {
                text = v.as_bytes().to_vec();
                have_text = true;
            }
            _ => text_file = v,
        }
        i += 2;
    }
    if !text_file.is_empty() {
        if have_text {
            return usage("--text or --text-file, not both");
        }
        if !std::path::Path::new(text_file).is_file() {
            return usage(&format!("no file {text_file}"));
        }
        // `"$(cat "$file")"`: NULs and trailing newlines dropped.
        text = std::fs::read(text_file).unwrap_or_default();
        text.retain(|b| *b != 0);
        while text.last() == Some(&b'\n') {
            text.pop();
        }
    }
    if subject.is_empty() || text.is_empty() {
        return usage(concat!(
            "usage: ",
            cli_name!(),
            " notify email <login|owner> --subject \"\u{2026}\" --text \"\u{2026}\""
        ));
    }
    let body = dumps(vec![
        ("to", p::text(person)),
        ("subject", p::text(subject)),
        ("text", env_text(&text)),
    ]);
    match cp::rt_post_ok(ctx, "/cli/notify/email", &body, &["200", "202"]) {
        Ok(b) => one_line(&b, |v| {
            Ok(format!(
                "sent to {} ({}, {})",
                py::str_of(&p::dget(v, "to", 1)?),
                py::str_of(&p::dget(v, "id", 1)?),
                py::str_of(&p::dget(v, "status", 1)?)
            ))
        })
        .print(),
        Err(e) => e,
    }
}
