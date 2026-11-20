//! `k5 domain list|add|remove|refresh|name add|name remove`, as the shell
//! CLI's `cmd_domain` answers them (`cli/src/domains-certs.sh`): the
//! custom domains attached to this box (not `k2 hostmail domain`).
//! `k5 domain` and `k5 domain name` alone (or with an empty verb) print
//! their help after the gates, as the shell does. The frame and the
//! reader are [`self::dom`]'s.

pub mod dom;

use self::dom::Act;
use crate::exit::Exit;
use crate::out::json::Value;
use k2_client::brand::CLI_NAME;
use std::ffi::OsString;

/// The routes.
pub const LIST: &str = "/cli/domains";
pub const ADD: &str = "/cli/domains";
pub const REMOVE: &str = "/cli/domains/remove";
pub const REFRESH: &str = "/cli/domains/refresh";
pub const NAME_ADD: &str = "/cli/domains/names";
pub const NAME_REMOVE: &str = "/cli/domains/names/remove";

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["domain"];
/// The binary answers it ([`run`] hands some lines back to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 domain ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    dom::run(args, parse, dom::read)
}

/// The words after `domain`.
pub fn parse(words: &[&str]) -> Act {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "list" => match dom::json_only(rest, &format!("{CLI_NAME} domain list [--json]")) {
            Ok(json) => Act::Get {
                endpoint: LIST,
                query: Vec::new(),
                json,
                verb: "list",
            },
            Err(hint) => Act::Usage(hint),
        },
        "add" => apex(
            rest,
            "add",
            ADD,
            " \u{2014} one <apex> only",
            &format!("domain add requires an <apex> (example.com) \u{2014} this is not {CLI_NAME} hostmail domain"),
        ),
        "remove" | "rm" | "delete" | "del" => apex(
            rest,
            "remove",
            REMOVE,
            "",
            "domain remove requires an <apex>",
        ),
        "refresh" | "check" => refresh(rest),
        "name" => name(rest),
        "" => Act::Help("domain"),
        other => Act::Usage(format!(
            "unknown command '{other}' \u{2014} see {CLI_NAME} domain --help (not {CLI_NAME} hostmail domain)"
        )),
    }
}

/// `cmd_domain_add` and `cmd_domain_remove`.
fn apex(
    rest: &[&str],
    verb: &'static str,
    endpoint: &'static str,
    extra: &str,
    missing: &str,
) -> Act {
    let usage = format!("{CLI_NAME} domain {verb} <apex> [--json]");
    match dom::one_arg(rest, &usage, extra) {
        Err(hint) => Act::Usage(hint),
        Ok((a, _)) if a.is_empty() => Act::Usage(missing.to_string()),
        Ok((a, json)) => Act::Post {
            endpoint,
            body: dom::body(vec![("apex", dom::s(&a))]),
            json,
            verb,
        },
    }
}

/// `cmd_domain_refresh`: the apex is optional, and sent only when it is
/// not blank (stripped, as the shell's Python strips it).
fn refresh(rest: &[&str]) -> Act {
    match dom::one_arg(
        rest,
        &format!("{CLI_NAME} domain refresh [<apex>] [--json]"),
        " \u{2014} one <apex> only",
    ) {
        Err(hint) => Act::Usage(hint),
        Ok((a, json)) => {
            let a = dom::strip(&a);
            let pairs = if a.is_empty() {
                Vec::new()
            } else {
                vec![("apex", dom::s(a))]
            };
            Act::Post {
                endpoint: REFRESH,
                body: dom::body(pairs),
                json,
                verb: "refresh",
            }
        }
    }
}

/// `cmd_domain_name`.
fn name(words: &[&str]) -> Act {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "add" => name_add(rest),
        "remove" | "rm" | "delete" | "del" => {
            match dom::one_arg(
                rest,
                &format!("{CLI_NAME} domain name remove <hostname> [--json]"),
                "",
            ) {
                Err(hint) => Act::Usage(hint),
                Ok((h, _)) if h.is_empty() => {
                    Act::Usage("domain name remove requires a <hostname>".to_string())
                }
                Ok((h, json)) => Act::Post {
                    endpoint: NAME_REMOVE,
                    body: dom::body(vec![("hostname", dom::s(&h))]),
                    json,
                    verb: "name-remove",
                },
            }
        }
        "" => Act::Help("domain/name"),
        other => Act::Usage(format!(
            "unknown name command '{other}' \u{2014} use add | remove (see {CLI_NAME} domain name --help)"
        )),
    }
}

/// The usage line of `domain name add`.
const NAME_ADD_USAGE: &str = concat!(
    k2_client::cli_name!(),
    " domain name add <hostname> [--role mail|publish|direct|other|app:<name>] [--external] [--json]"
);

/// `cmd_domain_name_add`.
fn name_add(rest: &[&str]) -> Act {
    let mut host = String::new();
    let mut role = String::new();
    let mut external = false;
    let mut json = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--role" => {
                let Some(r) = rest.get(i + 1) else {
                    return Act::Usage(
                        "--role requires mail|publish|direct|other|app:<name>".to_string(),
                    );
                };
                role = (*r).to_string();
                i += 2;
                continue;
            }
            "--external" => external = true,
            "--json" => json = true,
            w if w.starts_with('-') => {
                return Act::Usage(format!(
                    "unknown flag '{w}' \u{2014} usage: {NAME_ADD_USAGE}"
                ))
            }
            w if host.is_empty() => host = w.to_string(),
            w => return Act::Usage(format!("unexpected extra argument '{w}'")),
        }
        i += 1;
    }
    if host.is_empty() {
        return Act::Usage("domain name add requires a <hostname>".to_string());
    }
    let mut pairs = vec![("hostname", dom::s(&host))];
    let role = dom::strip(&role);
    if !role.is_empty() {
        pairs.push(("role", dom::s(role)));
    }
    if external {
        pairs.push(("external", Value::Bool(true)));
    }
    Act::Post {
        endpoint: NAME_ADD,
        body: dom::body(pairs),
        json,
        verb: "name-add",
    }
}
