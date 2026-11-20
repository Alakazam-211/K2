//! `k5 settings [--mode <m>]` and `k5 settings set <key> <true|false>`,
//! as the shell CLI's `cmd_settings` and `_settings_set` answer them
//! (`cli/src/wiki.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]). `settings` is
//!    the locked `settings` tool: in a session a stale passport and no
//!    passport at all are refused with exit 3 ([`crate::prelude::id_tool`]).
//! 2. The words ([`parse`]): a stop is its line on stderr, exit 2.
//! 3. The show (`cli_request /cli/settings`) or the mode
//!    (`cli_request_post /cli/mode`), over TCP on the left of a pipe into
//!    `_settings_print` ([`print`]); or `set`'s `cli_send_json POST
//!    /cli/settings/update` and its Python ([`set_reader`]).
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Handed to the shell ([`run`] returns `None`): any `-h` or `--help`
//! (the front's), a word that is not UTF-8, `HOOK_SOCK` set with a
//! `CODEX_` variable exported, a port curl won't take, and a `python3`
//! the binary can't tell.

use super::heartbeat::py::{dget_or, dget_v, Py, R};
use super::{curl_port, hook_sock, pipefail, request_piped_tcp, request_post_piped_tcp, s, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::Error;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["settings"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "settings";

pub const SHOW: &str = "/cli/settings";
pub const MODE: &str = "/cli/mode";
pub const UPDATE: &str = "/cli/settings/update";

/// `_K2_SETTINGS_SET_KEYS`, in help's order.
pub const KEYS: &[&str] = &[
    "voiceRoomsPreview",
    "voiceTunnelPreview",
    "computePreview",
    "ciPreview",
    "boardFilePreview",
    "videoRoomsPreview",
    "usageSingleRecord",
    "agentProcessNames",
    "homeChat",
    "homeChatPush",
];

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Show,
    Mode(String),
    Set {
        key: String,
        value: bool,
    },
    /// A line on stderr, exit 2.
    Usage(String),
}

/// The words after `settings`.
pub fn parse(words: &[&str]) -> Form {
    let c = cli_name!();
    if words.first() == Some(&"set") {
        let rest = &words[1..];
        let keys = KEYS.join(", ");
        if rest.len() != 2 {
            return Form::Usage(format!(
                "Error: usage: {c} settings set <key> <true|false>  (keys: {keys})"
            ));
        }
        let (key, raw) = (rest[0], rest[1]);
        if !KEYS.contains(&key) {
            return Form::Usage(format!(
                "Error: {c} settings set doesn't change '{key}'. It takes: {keys}. Other settings are in the {} app's Settings.",
                product_name!()
            ));
        }
        let value = match raw.to_ascii_lowercase().as_str() {
            "true" | "on" => true,
            "false" | "off" => false,
            _ => {
                return Form::Usage(format!(
                    "Error: {key} takes true or false (on and off work too), not '{raw}'."
                ))
            }
        };
        return Form::Set {
            key: key.to_string(),
            value,
        };
    }
    let mut mode = String::new();
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--mode" => {
                mode = words.get(i + 1).copied().unwrap_or("").to_string();
                i += 1;
            }
            w if w.starts_with('-') => {
                return Form::Usage(format!(
                    "Error: unknown flag '{w}' (supported: --mode <off|custom|manager|k2>)"
                ))
            }
            _ => {}
        }
        i += 1;
    }
    if mode.is_empty() {
        return Form::Show;
    }
    match mode.as_str() {
        "off" | "custom" | "manager" | "k2" | "k2so" => Form::Mode(mode),
        _ => Form::Usage(format!(
            "Error: invalid mode '{mode}' (valid: off, custom, manager, k2)"
        )),
    }
}

/// `k5 settings ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook = hook_sock();
    if hook.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = parse(&words);
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
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, form, hook.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    form: Form,
    hook: Option<&std::path::Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook, would_refuse, TOOL) {
        return stop;
    }
    if let Form::Usage(line) = &form {
        out::stderr_line(line);
        return Exit::Usage;
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match form {
        Form::Show => {
            let p = request_piped_tcp(&mut client, &mut closing, SHOW, &[]);
            let shown = print(&String::from_utf8_lossy(&p.body), false);
            pipefail(p.exit, shown.print())
        }
        Form::Mode(m) => {
            let param = format!("set={}", quote(&m));
            let p = request_post_piped_tcp(&mut client, &mut closing, MODE, &[&param]);
            let shown = print(&String::from_utf8_lossy(&p.body), true);
            pipefail(p.exit, shown.print())
        }
        Form::Set { key, value } => {
            let payload = format!("{{\"{key}\":{value}}}");
            let passport = client.passport().is_some();
            match client.send_json(true, UPDATE, &payload) {
                Ok(r) => {
                    closing.answer(&r, passport);
                    let body = super::without_nuls(&r.body);
                    let status = format!("{:03}", r.status);
                    set_reader(&String::from_utf8_lossy(&body), &status, &key, value).print()
                }
                Err(
                    e @ (Error::MaybeApplied { .. }
                    | Error::NoOwnerInSession { .. }
                    | Error::NoAnswer(_)),
                ) => {
                    out::stderr_line(&e.to_string());
                    Exit::Error
                }
                Err(_) => set_reader("", "000", &key, value).print(),
            }
        }
        Form::Usage(_) => Exit::Usage,
    };
    closing.flush(&mut client, exit);
    exit
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `print("Error: %s%s" % ((code + ": ") if code and code not in hint
/// else "", hint))`, then `low = (code + " " + hint).lower()`: the line
/// goes out before `low` can raise.
fn err_text(p: &mut Py, code: &Value, hint: &Value) -> R<String> {
    let head = if py::truthy(code) {
        let Value::Str(_) = code else {
            return Err(format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(code)
            ));
        };
        let inside = match hint {
            Value::Str(_) => st(hint).contains(&st(code)),
            other => super::heartbeat::py::has(other, &st(code))?,
        };
        if inside {
            String::new()
        } else {
            format!("{}: ", st(code))
        }
    } else {
        String::new()
    };
    p.err(format!("Error: {head}{}", st(hint)));
    let code_s = match code {
        Value::Str(_) => st(code),
        other => {
            return Err(format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ))
        }
    };
    let Value::Str(_) = hint else {
        return Err(format!(
            "TypeError: can only concatenate str (not \"{}\") to str",
            py::type_name(hint)
        ));
    };
    Ok(format!("{code_s} {}", st(hint)).to_lowercase())
}

/// `_settings_print` on what the reader read; `set`: the mode's view.
pub fn print(raw: &str, set: bool) -> Shown {
    let mut p = Py::default();
    let d = match json::parse(raw) {
        Ok(d) => d,
        Err(_) => {
            p.err(format!(
                "Error: the {} daemon gave no usable answer: {}",
                product_name!(),
                super::heartbeat::py::repr_head(raw, 200)
            ));
            return p.caught(Ok(Exit::Error));
        }
    };
    let r = (|| -> R<Exit> {
        let err = match &d {
            Value::Object(_) => dget_v(&d, "error")?,
            _ => s("unexpected answer"),
        };
        if err != Value::Null {
            let (code, hint) = match &err {
                Value::Object(_) => {
                    let code = dget_or(&err, "code", s(""))?;
                    let hint = super::heartbeat::py::or(
                        super::heartbeat::py::or(dget_v(&err, "hint")?, dget_v(&err, "message")?),
                        code.clone(),
                    );
                    (code, hint)
                }
                other => (s(""), s(&st(other))),
            };
            let low = err_text(&mut p, &code, &hint)?;
            let refused = [
                "owner",
                "admin",
                "forbidden",
                "token",
                "not allowed",
                "refused",
                "passport",
            ]
            .iter()
            .any(|w| low.contains(w));
            if refused {
                p.err(format!("Workspace settings are the owner's or an admin's: ask your human, or run it from a terminal outside a {} agent session.", product_name!()));
                return Ok(Exit::Refused);
            }
            return Ok(Exit::Error);
        }
        if set {
            p.out(format!("Mode set to {}", st(&dget_or(&d, "mode", s("?"))?)));
            return Ok(Exit::Ok);
        }
        p.out("Workspace Settings:");
        p.out(format!(
            "  Name:      {}",
            st(&dget_or(&d, "name", s("?"))?)
        ));
        p.out(format!(
            "  Mode:      {}",
            st(&dget_or(&d, "mode", s("off"))?)
        ));
        let on = if py::truthy(&dget_v(&d, "heartbeatEnabled")?) {
            "on"
        } else {
            "off"
        };
        p.out(format!("  Heartbeat: {on}"));
        let pinned = if py::truthy(&dget_v(&d, "pinned")?) {
            "yes"
        } else {
            "no"
        };
        p.out(format!("  Pinned:    {pinned}"));
        Ok(Exit::Ok)
    })();
    p.raised(r, 9)
}

/// What `set` says next, by key.
fn next(key: &str) -> String {
    let c = cli_name!();
    let p = product_name!();
    match key {
        "voiceRoomsPreview" => format!("{c} voice status says whether this server can host rooms, and why not."),
        "voiceTunnelPreview" => format!("{c} hostchat media tier tunnel moves voice onto {p}'s tunnel."),
        "computePreview" => format!("{c} compute nodes lists the machines."),
        "ciPreview" => format!("{c} study ci (CI on {p} Compute: {c} ci, node rosters, lanes, seats, signer pairing; needs computePreview on too)."),
        "boardFilePreview" => format!("Board files open in a tab in the {p} app."),
        "videoRoomsPreview" => format!("Video in voice rooms shows in the {p} desktop app's call panel (preview)."),
        "usageSingleRecord" => "One usage percent per token: the top bar, Settings and the phone read one record. Off goes straight back.".into(),
        "agentProcessNames" => "Claude Code sessions started from now on show their agent name in Activity Monitor (ps -c shows it too). Running sessions keep theirs until they restart.".into(),
        "homeChat" => format!("People and channels on Home (needs chat on this server: {c} hostchat status). The {p} app and the phone show them once they reconnect."),
        _ => format!("Phones get Home chat DMs, @mentions and replies (needs homeChat on). Message text is sealed to each phone; the owner turns it off with homeChatPushPreviews in the {p} app's Settings."),
    }
}

/// `_settings_set`'s Python on the answer.
pub fn set_reader(raw: &str, status: &str, key: &str, want: bool) -> Shown {
    let mut p = Py::default();
    let d = json::parse(raw).ok();
    let r = (|| -> R<Exit> {
        let bad = status != "200"
            || !matches!(d, Some(Value::Object(_)))
            || dget_v(d.as_ref().unwrap_or(&Value::Null), "error")? != Value::Null;
        if bad {
            let err = match &d {
                Some(v @ Value::Object(_)) => dget_v(v, "error")?,
                _ => Value::Null,
            };
            let (code, mut hint) = match &err {
                Value::Object(_) => {
                    let code = dget_or(&err, "code", s(""))?;
                    let hint = super::heartbeat::py::or(
                        super::heartbeat::py::or(dget_v(&err, "hint")?, dget_v(&err, "message")?),
                        code.clone(),
                    );
                    (code, hint)
                }
                Value::Str(_) => (s(""), err.clone()),
                _ => (s(""), s("")),
            };
            if !py::truthy(&hint) {
                hint = s(&if status == "000" {
                    format!("no answer from the {} daemon", product_name!())
                } else {
                    format!(
                        "the daemon answered HTTP {status}: {}",
                        super::heartbeat::py::repr_head(raw, 200)
                    )
                });
            }
            let low = err_text(&mut p, &code, &hint)?;
            let refused = matches!(status, "401" | "403")
                || [
                    "owner",
                    "admin",
                    "forbidden",
                    "not allowed",
                    "refused",
                    "passport",
                ]
                .iter()
                .any(|w| low.contains(w));
            if refused {
                let perm = match &d {
                    Some(v @ Value::Object(_)) => dget_v(v, "permission")?,
                    _ => Value::Null,
                };
                let card =
                    matches!(perm, Value::Object(_)) && py::truthy(&dget_v(&perm, "command")?);
                if card {
                    p.err("Server settings are the owner's: ask the owner on a permission card.");
                } else {
                    p.err(format!("Server settings are the owner's: ask your human, or run it from a terminal outside a {} agent session.", product_name!()));
                }
                return Ok(Exit::Refused);
            }
            return Ok(if status == "400" {
                Exit::Usage
            } else {
                Exit::Error
            });
        }
        let d = d.clone().unwrap_or(Value::Null);
        let got = dget_v(&d, key)?;
        if got != Value::Bool(want) {
            let dumped = json::to_string(&got, &Options::default()).unwrap_or_default();
            p.err(format!(
                "Error: the daemon answered, but {key} is {dumped}, not {want}. Run {} settings set again, or check the daemon log.",
                cli_name!()
            ));
            return Ok(Exit::Error);
        }
        p.out(format!("{key} is now {want} on this server."));
        if want {
            p.out(format!("Next: {}", next(key)));
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forms() {
        assert_eq!(parse(&[]), Form::Show);
        assert_eq!(parse(&["x", "--mode"]), Form::Show);
        assert_eq!(parse(&["--mode", "k2so"]), Form::Mode("k2so".into()));
        assert!(matches!(parse(&["--mode", "x"]), Form::Usage(_)));
        assert!(matches!(parse(&["-x"]), Form::Usage(_)));
        assert_eq!(
            parse(&["set", "homeChat", "ON"]),
            Form::Set {
                key: "homeChat".into(),
                value: true
            }
        );
        assert!(matches!(parse(&["set", "nope", "on"]), Form::Usage(_)));
    }

    #[test]
    fn the_readers() {
        let r = print(r#"{"name":"w","mode":"k2","pinned":true}"#, false);
        assert_eq!(
            r.stdout,
            "Workspace Settings:\n  Name:      w\n  Mode:      k2\n  Heartbeat: off\n  Pinned:    yes\n"
        );
        let r = set_reader(r#"{"homeChat":true}"#, "200", "homeChat", true);
        assert_eq!(r.exit, Exit::Ok);
        assert!(r
            .stdout
            .starts_with("homeChat is now true on this server.\nNext: "));
    }
}
