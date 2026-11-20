//! `k5 app <verb> …`, the App family, as the shell CLI's `cmd_app`
//! (`cli/src/app-wallet.sh`) answers it, one verb group at a time. The
//! route table sends a verb here when its group is native; every other
//! verb stays the shell's. `app` alone (and `app --json`) is skin's page,
//! as `cmd_app` hands it to `cmd_skin` (its bare row).
//!
//! Native so far:
//! - [`wallet`]: `app wallet settings|people|remove|personal-agent|personal-agents`.
//! - [`reads`]: `app list` (`--workspace`, `--box`), `app inspect` (not
//!   `--resources`) and `app deployment`.
//! - [`access`]: `app people`, `app discovery`, `app stay-signed-in`,
//!   `app profile settings` and `app set`.
//! - [`door`]: `app suspend`, `app self`, `app usage`, `app members`,
//!   `app remote-pass` and `app account-deletion` (list, complete, cancel).
//! - [`updates`]: `app updates ls|list|stats|fleet`, its page and its
//!   usage line; [`updates_set`]: `key`, `token`, `promote`, `rollback`,
//!   `pull` and `push`.
//! - [`history`]: `app history`, `app errors`, `app logs` (one read, and
//!   `--client` without `--since`).
//! - [`phone`]: the reads of `app links`, `app pairing`, `app push` and
//!   `app email`, each family's page and its unknown verb's usage line.
//! - [`phone_set`]: the writes of `app links`, `app pairing`, `app push`
//!   and `app email`, with their confirm steps.
//! - [`catalog`]: `app catalog [list]`, `show`, `check`, `job|jobs` and
//!   `retry`.
//! - [`walls`]: `app egress [show]|log`, `app box-service <app>` and `app
//!   db status`; [`walls_set`]: `app egress add|rm|mode` and `app
//!   box-service request|revoke`, with [`approve`] when the owner is at
//!   the terminal; [`db_set`]: `app db connect|check|disconnect`.
//! - [`door`] also answers `app resume`; [`traffic`]: `app audit`.
//! - [`runtime`]: `app validate`, `app status` and `app sdk
//!   add|status|kit`; [`recording`]: `app recording [set]` and `app
//!   room-names`.
//! - [`guests`]: `app guest rooms|ceiling|list`, `app grant <app> --kind
//!   …` and `app grant copy-host`, with [`grace`] for a held answer;
//!   [`sign_in`]: `app auth` and `app login settings`; [`chat`]: `app
//!   chat`; [`init`]: `app init` and `app doctor --store`.
//! - [`approve`]: the held requests, `app approvals [revoke]`, `app deny`
//!   and `app approve`.
//! - The verbs `cmd_app` hands to `cmd_skin` (`user`, `role`, `grant`'s
//!   list words, `front-door`, `hydra`: the [`super::skin`] family) and
//!   to `cmd_skin_token` (`token`: the [`super::skin_token`] family).
//!   [`run`] hands them over first; those families run their own prelude.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates, as for every verb that asks the daemon
//!    (`prelude.sh`). `app` is an `id` tool (`skin`): in a session, a
//!    stale passport and no passport at all are refused first
//!    ([`prelude::id_tool`]), and the session's passport is the
//!    credential.
//! 2. The group's own loop over the words: a usage mistake is its line on
//!    stderr and exit 2, before anything is sent.
//! 3. The group's requests ([`call`]): `_compute_call` and its refusal
//!    line, with the App refusal registry's `fix:` line.
//! 4. The printer: the answer as it came with `--json`, else the shell's
//!    Python, line for line (a Python 3.12 traceback where it raises).
//! 5. The EXIT trap's lines.
//!
//! The shell answers these itself ([`run`] returns `None`): a session
//! with `CODEX_*` set (the shared Codex lookup), a word that is not UTF-8, a verb no group here takes, a port curl won't take, and
//! a `python3` the binary can't tell ([`crate::families`], Without
//! Python). Help forms never reach here: the front sends them to their
//! page or to the shell.

use super::{curl_port, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude;
use crate::python;
use std::ffi::OsString;
use std::path::Path;

pub mod access;
pub mod approve;
pub mod call;
pub mod catalog;
pub mod chat;
pub mod db_set;
pub mod door;
pub mod grace;
pub mod guests;
pub mod history;
pub mod init;
pub mod phone;
pub mod phone_set;
pub mod py;
pub mod reads;
pub mod recording;
pub mod runtime;
pub mod sign_in;
pub mod traffic;
pub mod updates;
pub mod updates_set;
pub mod wallet;
pub mod walls;
pub mod walls_set;

use call::Ctx;

/// The catalog tool every `app` verb is (`_cli_tool_id_for_verb app`).
pub const TOOL: &str = "skin";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Wallet(wallet::Ask),
    List(reads::List),
    Deployment(reads::Deployment),
    Inspect(reads::Inspect),
    Access(access::Ask),
    Door(door::Ask),
    Updates(updates::Ask),
    UpdatesSet(updates_set::Ask),
    History(history::Ask),
    Phone(phone::Ask),
    PhoneSet(phone_set::Ask),
    Catalog(catalog::Ask),
    Walls(walls::Ask),
    WallsSet(walls_set::Ask),
    Db(db_set::Ask),
    Approve(approve::Ask),
    Traffic(traffic::Ask),
    Runtime(runtime::Ask),
    Recording(recording::Ask),
    Guests(guests::Ask),
    SignIn(sign_in::Ask),
    Chat(chat::Ask),
    Folder(init::Ask),
}

/// Where a verb's loop stopped before anything was sent: its line on
/// stderr (none when empty), and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub line: String,
    pub exit: Exit,
}

impl Stop {
    /// A usage line, exit 2.
    pub fn usage(line: impl Into<String>) -> Stop {
        Stop {
            line: line.into(),
            exit: Exit::Usage,
        }
    }

    /// Nothing said: `set -e` ended the command (a `shift 2` with one word
    /// left).
    pub fn silent(exit: Exit) -> Stop {
        Stop {
            line: String::new(),
            exit,
        }
    }
}

/// The words a group here takes (each verb row below names some of them).
pub const TAKES: &[&str] = &[
    "wallet",
    "list",
    "inspect",
    "deployment",
    "people",
    "discovery",
    "stay-signed-in",
    "profile",
    "set",
    "suspend",
    "self",
    "usage",
    "members",
    "remote-pass",
    "account-deletion",
    "account-deletions",
    "updates",
    "history",
    "errors",
    "logs",
    "links",
    "pairing",
    "push",
    "email",
    "catalog",
    "egress",
    "box-service",
    "box-services",
    "db",
    "database",
    "approvals",
    "approve",
    "deny",
    "resume",
    "audit",
    "validate",
    "status",
    "sdk",
    "recording",
    "recordings",
    "room-names",
    "guest",
    "auth",
    "login",
    "chat",
    "init",
    "doctor",
];

/// The verbs `cmd_app` hands to `cmd_skin` and `cmd_skin_token` (their
/// verb rows below); [`run`] passes them to those families.
pub const SKIN_VERBS: &[&str] = &[
    "user",
    "users",
    "role",
    "roles",
    "grant",
    "grants",
    "front-door",
    "frontdoor",
    "hydra",
    "token",
    "tokens",
];

/// `cmd_app`'s `grant|grants)` arm: the third words it hands to
/// `cmd_skin` (the [`super::skin`] family, but `copy-host`, which
/// [`guests`] answers). Any other is `cmd_app_grant_kind` ([`guests`]) or
/// `cmd_app_grants` (the shell's).
pub const SKIN_GRANT: &[&str] = &[
    "",
    "list",
    "add",
    "create",
    "rm",
    "remove",
    "delete",
    "revoke",
    "set",
    "update",
    "role",
    "hooks",
    "hook",
    "copy-host",
    "copyhost",
    "-h",
    "--help",
];

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["app"];
/// The shell's but for [`VERBS`]: every other verb is the shell's (`app`
/// alone is skin's page, by its bare row).
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// One verb row per native group of `cmd_app`; each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[
    crate::route::Verb {
        words: &["wallet"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["list"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["inspect"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["deployment"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["people"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["discovery"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["stay-signed-in"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["profile"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["set"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["suspend"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["self"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["usage"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["members"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["remote-pass"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["account-deletion", "account-deletions"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["updates"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["history"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["errors"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["logs"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["links"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["pairing"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["push"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["email"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["catalog"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["egress"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["box-service", "box-services"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["db", "database"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["approvals"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["approve"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["deny"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["resume"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["audit"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["validate"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["status"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["sdk"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["recording", "recordings"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["room-names"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["guest"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["auth"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["login"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["chat"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["init"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["doctor"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["user", "users"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["role", "roles"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["grant", "grants"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["front-door", "frontdoor"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["hydra"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["token", "tokens"],
        owner: crate::route::Owner::Native,
    },
    // `app` alone and `app --json`: skin's page ([`super::skin::run_app`]).
    crate::route::Verb::bare(crate::route::Owner::Native),
];

/// The words after `app`, read by the group the verb names. `None`: the
/// shell's. `Err`: where the verb's loop stopped.
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let verb = *words.first()?;
    let rest = &words[1..];
    match verb {
        "wallet" => Some(wallet::parse(rest).map(Ask::Wallet).map_err(Stop::usage)),
        "list" => Some(reads::parse_list(rest)?.map(Ask::List)),
        "deployment" => Some(reads::parse_deployment(rest)?.map(Ask::Deployment)),
        "inspect" => Some(reads::parse_inspect(rest)?.map(Ask::Inspect)),
        "updates" => match updates_set::parse(rest) {
            Some(r) => Some(r.map(Ask::UpdatesSet)),
            None => Some(updates::parse(rest)?.map(Ask::Updates)),
        },
        "catalog" => Some(catalog::parse(rest)?.map(Ask::Catalog)),
        v if walls::VERBS.contains(&v) => {
            if let Some(r) = walls_set::parse(v, rest) {
                return Some(r.map(Ask::WallsSet));
            }
            if matches!(v, "db" | "database") {
                if let Some(r) = db_set::parse(rest) {
                    return Some(r.map(Ask::Db));
                }
            }
            Some(walls::parse(v, rest)?.map(Ask::Walls))
        }
        "audit" => Some(traffic::parse(rest)?.map(Ask::Traffic)),
        v if runtime::VERBS.contains(&v) => Some(runtime::parse(v, rest)?.map(Ask::Runtime)),
        v if recording::VERBS.contains(&v) => Some(recording::parse(v, rest)?.map(Ask::Recording)),
        "guest" => Some(guests::parse_guest(rest)?.map(Ask::Guests)),
        "auth" => Some(sign_in::parse_auth(rest)?.map(Ask::SignIn)),
        "login" => Some(sign_in::parse_login(rest)?.map(Ask::SignIn)),
        "chat" => Some(chat::parse(rest)?.map(Ask::Chat)),
        "init" => Some(init::parse_init(rest)?.map(Ask::Folder)),
        "doctor" => Some(init::parse_doctor(rest)?.map(Ask::Folder)),
        "grant" | "grants" => Some(guests::parse_grant(rest)?.map(Ask::Guests)),
        v if approve::VERBS.contains(&v) => Some(approve::parse(v, rest)?.map(Ask::Approve)),
        v if access::VERBS.contains(&v) => Some(access::parse(v, rest)?.map(Ask::Access)),
        v if door::VERBS.contains(&v) => Some(door::parse(v, rest)?.map(Ask::Door)),
        v if history::VERBS.contains(&v) => Some(history::parse(v, rest)?.map(Ask::History)),
        v if phone::VERBS.contains(&v) => match phone_set::parse(v, rest) {
            Some(r) => Some(r.map(Ask::PhoneSet)),
            None => Some(phone::parse(v, rest)?.map(Ask::Phone)),
        },
        _ => None,
    }
}

/// `k5 app …`. `None`: the shell's (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    // `app` alone (or with `--json`): `cmd_app` hands it to `cmd_skin`,
    // which prints the family's page (the bare row below).
    if crate::route::is_bare(args) {
        return super::skin::run_app(args);
    }
    match args.get(1).and_then(|a| a.to_str()) {
        Some("token" | "tokens") => return super::skin_token::run_app(args),
        Some(v @ ("grant" | "grants")) => {
            let third = args.get(2).map_or(Some(""), |a| a.to_str())?;
            if !SKIN_GRANT.contains(&third) {
                // `grants <app>` is `cmd_app_grants`: the shell's.
                if v == "grants" {
                    return None;
                }
            } else if !matches!(third, "copy-host" | "copyhost") {
                return super::skin::run_app(args);
            }
        }
        Some(v) if SKIN_VERBS.contains(&v) => return super::skin::run_app(args),
        _ => {}
    }
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let ask = parse(words.get(1..).unwrap_or_default())?;
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
    if prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's.
        curl_port(settings.port_text.as_deref()?)?;
    }
    Some(run_with(args, &words, ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    ask: Result<Ask, Stop>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match ask {
        Ok(a) => a,
        Err(stop) => {
            if !stop.line.is_empty() {
                out::stderr_line(&stop.line);
            }
            return stop.exit;
        }
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut ctx = Ctx::new(client, call::args_json(words));
    ctx.vault_sock = crate::confirm::sock_path(&settings.home);
    ctx.project = settings.project.clone();
    let shown = match &ask {
        Ask::Wallet(a) => wallet::run(&mut ctx, a),
        Ask::List(l) => reads::list(&mut ctx, l),
        Ask::Deployment(d) => reads::deployment(&mut ctx, d),
        Ask::Inspect(i) => reads::inspect(&mut ctx, i),
        Ask::Access(a) => access::run(&mut ctx, a),
        Ask::Door(d) => door::run(&mut ctx, d),
        Ask::Updates(u) => updates::run(&mut ctx, u),
        Ask::UpdatesSet(u) => updates_set::run(&mut ctx, u),
        Ask::History(h) => history::run(&mut ctx, h),
        Ask::Phone(p) => phone::run(&mut ctx, p),
        Ask::PhoneSet(p) => phone_set::run(&mut ctx, p),
        Ask::Catalog(c) => catalog::run(&mut ctx, c),
        Ask::Walls(w) => walls::run(&mut ctx, w),
        Ask::WallsSet(w) => walls_set::run(&mut ctx, w),
        Ask::Db(d) => db_set::run(&mut ctx, d),
        Ask::Approve(a) => approve::run(&mut ctx, a),
        Ask::Traffic(t) => traffic::run(&mut ctx, t),
        Ask::Runtime(r) => runtime::run(&mut ctx, r),
        Ask::Recording(r) => recording::run(&mut ctx, r),
        Ask::Guests(g) => guests::run(&mut ctx, g),
        Ask::SignIn(s) => sign_in::run(&mut ctx, s),
        Ask::Chat(c) => chat::run(&mut ctx, c),
        Ask::Folder(f) => init::run(&mut ctx, f),
    };
    let exit = match shown {
        Ok(s) => s.print(),
        Err(exit) => exit,
    };
    let Ctx {
        mut client,
        closing,
        ..
    } = ctx;
    closing.flush(&mut client, exit);
    exit
}

/// A group's answer, printed: a [`Shown`], or the exit the requests ended
/// with (their lines printed).
pub type Answer = Result<Shown, Exit>;

#[cfg(test)]
mod tests;
