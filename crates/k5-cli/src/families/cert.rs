//! `k5 cert list|issue|renew`, as the shell CLI's `cmd_cert` answers them
//! (`cli/src/domains-certs.sh`): this box's certificates for its attached
//! hostnames. `k5 cert` alone (or an empty verb) prints its help after
//! the gates, as the shell does. The frame and the reader are
//! [`super::domain::dom`]'s.

use super::domain::dom::{self, Act};
use crate::exit::Exit;
use k2_client::brand::CLI_NAME;
use std::ffi::OsString;

/// The routes.
pub const LIST: &str = "/cli/certs";
pub const ISSUE: &str = "/cli/certs/issue";
pub const RENEW: &str = "/cli/certs/renew";

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["cert"];
/// The binary answers it ([`run`] hands some lines back to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 cert ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    dom::run(args, parse, dom::read)
}

/// The words after `cert`.
pub fn parse(words: &[&str]) -> Act {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "list" => match dom::json_only(rest, &format!("{CLI_NAME} cert list [--json]")) {
            Ok(json) => Act::Get {
                endpoint: LIST,
                query: Vec::new(),
                json,
                verb: "certs",
            },
            Err(hint) => Act::Usage(hint),
        },
        "issue" => host(rest, "issue", ISSUE),
        "renew" => host(rest, "renew", RENEW),
        "" => Act::Help("cert"),
        other => Act::Usage(format!(
            "unknown command '{other}' \u{2014} see {CLI_NAME} cert --help (box inventory; not {CLI_NAME} hostmail cert)"
        )),
    }
}

/// `cmd_cert_issue` and `cmd_cert_renew`.
fn host(rest: &[&str], verb: &'static str, endpoint: &'static str) -> Act {
    let usage = format!("{CLI_NAME} cert {verb} <hostname> [--json]");
    match dom::one_arg(rest, &usage, "") {
        Err(hint) => Act::Usage(hint),
        Ok((h, _)) if h.is_empty() => {
            Act::Usage(format!("cert {verb} requires an attached <hostname>"))
        }
        Ok((h, json)) => Act::Post {
            endpoint,
            body: dom::body(vec![("hostname", dom::s(&h))]),
            json,
            verb,
        },
    }
}
