//! The rest of the router's `agent)` arm (`cli/src/router.sh`): the
//! family alone, the 0.39.0f internal verbs `update` and `complete`, and
//! a word it has no arm for.
//!
//! - `agent` (or `agent ''`): `cmd_help_agent` on stdout, exit 0, after
//!   the gates.
//! - `agent template` alone (or `templates`, or with `''`): the template
//!   help page, exit 0, with no gate (the shell skips it there).
//! - `update` (`cmd_agent_update`, `cli/src/commands-core.sh`): the words
//!   ([`parse_update`]), then `_ws_request cli_request_post
//!   /cli/agent/update` (the cell socket first), `cli_print_or_fail` to
//!   `/dev/null`, and the line when the answer's `success` is `true`. In a
//!   session it sends the passport, or no credential when there is none
//!   (`_cli_verb_sends_session_passport`), as the shell does.
//! - `complete` (`cmd_agent_complete`, `cli/src/talk-and-workspace.sh`):
//!   `cli_request_post /cli/agent/complete agent=…&file=…`, the answer
//!   printed as it came. An open verb: in a session with a passport the
//!   shell sends the owner token from disk, the binary the passport (the
//!   named difference `owner-token-with-passport`); the daemon admits a
//!   passport for its own work only (`not_this_agent` for another name).
//! - Any other word: a retired verb's line (`_retired_verb_exit`, exit 1),
//!   else `Unknown agent subcommand` and the help page on stderr, exit 2.

use super::cli;
use crate::exit::Exit;
use crate::families::setup::{cut_200, utf8_locale};
use crate::families::workspace::agent::AGENT_UPDATE;
use crate::families::workspace::{refused_or, verdict, ws_request, Verdict, Ws};
use crate::families::{get, without_nuls};
use crate::out;
use crate::out::json::Value;
use crate::out::pylib::json as pyjson;
use crate::prelude::Closing;
use crate::transport::client::{Call, Channel, Error};
use crate::transport::encode::quote;
use crate::transport::{Auth, Client};

/// `cmd_agent_complete`'s route.
pub const COMPLETE: &str = "/cli/agent/complete";

/// `cmd_agent_update`'s words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub name: String,
    pub field: String,
    pub value: String,
}

/// `cmd_agent_update`'s usage line.
fn update_usage() -> String {
    format!(
        "Usage: {} agent update --name <agent> --field <field> --value \"...\"",
        cli()
    )
}

/// The words after `update`: the three values, or the lines and exit 2.
pub fn parse_update(words: &[&str]) -> Result<Update, Vec<String>> {
    let (mut name, mut field, mut value) = (String::new(), String::new(), String::new());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--name" | "--field" | "--value" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(vec![format!("Error: {w} needs a value"), update_usage()]);
                };
                let slot = match w {
                    "--name" => &mut name,
                    "--field" => &mut field,
                    _ => &mut value,
                };
                *slot = (*v).to_string();
                i += 2;
            }
            _ => {
                if name.is_empty() {
                    name = w.to_string();
                }
                i += 1;
            }
        }
    }
    if name.is_empty() || field.is_empty() || value.is_empty() {
        return Err(vec![
            update_usage(),
            "  field: frontmatter key (role, type) or section name (Work Sources, Capabilities)"
                .to_string(),
        ]);
    }
    Ok(Update { name, field, value })
}

/// `cmd_agent_complete`'s words: `(agent, file)`. `Err`: a `--agent` or
/// `--file` with nothing after it, `_need_value`'s line (exit 2).
pub fn parse_complete(words: &[&str]) -> Result<(String, String), String> {
    let (mut agent, mut file) = (String::new(), String::new());
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            w @ ("--agent" | "--file") => {
                let Some(v) = words.get(i + 1) else {
                    return Err(out::need_value(w));
                };
                if w == "--agent" {
                    agent = (*v).to_string();
                } else {
                    file = (*v).to_string();
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    Ok((agent, file))
}

/// `complete`, after the gates.
pub fn complete(client: &mut Client, closing: &mut Closing, agent: &str, file: &str) -> Exit {
    if agent.is_empty() || file.is_empty() {
        out::stderr_line(&format!(
            "Usage: {} agent complete --agent <name> --file <filename>",
            cli()
        ));
        return Exit::Error;
    }
    let q = format!("agent={}&file={}", quote(agent), quote(file));
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, COMPLETE, Call::RequestPost(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(e @ Error::NoAnswer(_)) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    }
}

/// `update`, after the gates: `make` gives a client as the shell's state
/// is now (each request is its own `$(…)`), the first for the EXIT trap.
pub fn update(
    mut make: impl FnMut() -> Result<Client, String>,
    base_url: String,
    session: bool,
    u: &Update,
) -> Exit {
    let mut fresh = || {
        make().map(|mut c| {
            // `_cli_verb_sends_session_passport`: never the owner token.
            if session && matches!(c.auth, Auth::Owner(_)) {
                c.auth = Auth::None;
                c.no_token = true;
            }
            c
        })
    };
    let (mut main, mut first) = match (fresh(), fresh()) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut ws = Ws {
        closing: Closing::default(),
        base_url,
    };
    let pairs = [
        format!("agent={}", quote(&u.name)),
        format!("field={}", quote(&u.field)),
        format!("value={}", quote(&u.value)),
    ];
    let refs: Vec<&str> = pairs.iter().map(String::as_str).collect();
    let exit = match ws_request(
        &mut ws,
        &mut first,
        AGENT_UPDATE,
        Call::RequestPost(&refs),
        false,
    ) {
        Err(exit) => exit,
        Ok(answer) => match verdict(&answer, AGENT_UPDATE) {
            Verdict::Error(err) => {
                out::stderr_line(&format!("Error: {err}"));
                refused_or(&answer, Exit::Error)
            }
            Verdict::Print if succeeded(&answer) => {
                let line = format!("Updated {} for agent {}\n", u.field, u.name);
                match out::write_flushed(&mut std::io::stdout().lock(), line.as_bytes()) {
                    Ok(()) => Exit::Ok,
                    Err(e) => out::after_stdout_error(&e),
                }
            }
            Verdict::Print => {
                let mut line = format!(
                    "Error: {} did not say it updated {} for agent {}: ",
                    super::product(),
                    u.field,
                    u.name
                )
                .into_bytes();
                line.extend_from_slice(cut_200(&answer, utf8_locale));
                line.push(b'\n');
                let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
                Exit::Error
            }
        },
    };
    ws.closing.flush(&mut main, exit);
    exit
}

/// `json.load(sys.stdin).get("success") is True`.
fn succeeded(answer: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(answer) else {
        return false;
    };
    match pyjson::loads(text) {
        Ok(Value::Object(pairs)) => get(&pairs, "success") == Some(&Value::Bool(true)),
        _ => false,
    }
}

/// `cmd_help_agent`: the family's help page.
pub fn page() -> &'static str {
    crate::text::help("agent").expect("the agent help page is embedded")
}

/// `cmd_help_agent_template`: the template help page.
pub fn template_page() -> &'static str {
    crate::text::help("agent/template").expect("the agent template help page is embedded")
}

/// The `*)` arm for `word`: a retired verb's line and exit 1, else
/// `Unknown agent subcommand` and the help page on stderr, exit 2.
pub fn unknown(word: &str) -> Exit {
    let key = format!("agent {word}");
    if let Some(new) = crate::retired::find(&key) {
        out::stderr_line(&format!(
            "Error: '{key}' is retired. Use instead: {}",
            branded(new)
        ));
        return Exit::Error;
    }
    let mut text = format!("Unknown agent subcommand: {word}\n");
    text.push_str(page());
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
    Exit::Usage
}

/// A retired verb's pointer with the command named by the brand: the
/// shell's table names it `k2` at the start of each command.
fn branded(new: &str) -> String {
    const SHELL: &str = "k2 ";
    match new.strip_prefix(SHELL) {
        Some(rest) => format!("{} {rest}", cli()),
        None => new.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_update_words() {
        assert_eq!(
            parse_update(&["qa", "--field", "role", "--value", "x", "y"]),
            Ok(Update {
                name: "qa".into(),
                field: "role".into(),
                value: "x".into()
            })
        );
        assert_eq!(
            parse_update(&["--name", "a", "b", "--field", "f", "--value", "v"]).map(|u| u.name),
            Ok("a".to_string())
        );
        assert_eq!(parse_update(&["--value"]).unwrap_err().len(), 2);
        assert_eq!(parse_update(&["qa"]).unwrap_err().len(), 2);
    }

    #[test]
    fn the_complete_words() {
        assert_eq!(
            parse_complete(&["x", "--file", "f.md", "--agent", "qa"]),
            Ok(("qa".into(), "f.md".into()))
        );
        assert_eq!(
            parse_complete(&["--agent"]),
            Err("Error: --agent takes a value".to_string())
        );
        assert_eq!(
            parse_complete(&["--file", "f.md", "--agent"]),
            Err("Error: --agent takes a value".to_string())
        );
        assert_eq!(parse_complete(&[]), Ok((String::new(), String::new())));
    }

    #[test]
    fn a_retired_pointer_names_the_brand() {
        assert_eq!(
            branded("k2 agent retire <name>"),
            format!("{} agent retire <name>", cli())
        );
        assert_eq!(branded("(nothing)"), "(nothing)");
    }
}
