//! `k5 app <verb> …`, the App family, as the shell CLI's `cmd_app`
//! (`cli/src/app-wallet.sh`) answers it, one verb group at a time. The
//! route table sends a verb here when its group is native; every other
//! verb stays the shell's.
//!
//! Native so far:
//! - [`wallet`]: `app wallet settings|people|remove|personal-agent|personal-agents`.
//! - [`reads`]: `app list` (`--workspace`, `--box`), `app inspect` (with
//!   [`resources`] for `--resources`) and `app deployment`.
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
//!   `retry`, and `app install --catalog`.
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
//! - [`deploy`]: `app build`, `app install` (`--catalog` is
//!   [`catalog`]'s), `app deploy`, `app rollback`, `app stop|start|restart`, `app link`, `app
//!   seal` and `app config`.
//! - [`owner`]: `app uninstall`, `app expose`, `app handover` (every
//!   sub-act), `app unpark`, `app park`, `app release` and `app
//!   exception`.
//! - [`boxes`]: `app doctor` (the box and reach doctors), `app box`
//!   (every sub-verb), `app engine`, `app rehost` and `app sessions`.
//! - [`exec`]: `app exec`; [`history`] also answers `app logs --follow`.
//! - [`shell`]: `app shell`, the interactive terminal over a WebSocket.
//! - [`approve`]: the held requests, `app approvals [revoke]`, `app deny`
//!   and `app approve`.
//! - [`grace_set`]: `app warnings`, `app clean-mode`, `app key|keys`, `app
//!   label`, `app test-guests`, `app move-plan` and the pages `app
//!   surfaces|door|grace`; [`door_set`]: `app requests`, `app quota` and
//!   `app token rotate`; [`grants`]: `app grants <app>`;
//!   [`agent_access`]: `app access`; [`sign_in`] also answers `app
//!   invite-settings`; [`domains`]: `app domain` (every sub-verb) and
//!   `app domains`; [`data`]: `app data put|ls|get|rm|mv`; [`box_use`]:
//!   `app resources` (the view, `settings`, `clean` and `trends`).
//! - `app` alone (and `app --json`): `cmd_skin ""` prints the App page
//!   through the permission cards' filter (`_skin_usage`, `_perm_lines`)
//!   on stderr, exit 1 ([`bare`]).
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
pub mod agent_access;
pub mod approve;
pub mod box_use;
pub mod boxes;
pub mod call;
pub mod catalog;
pub mod chat;
pub mod data;
pub mod db_set;
pub mod deploy;
pub mod door;
pub mod door_set;
pub mod domains;
pub mod exec;
pub mod grace;
pub mod grace_set;
pub mod grants;
pub mod guests;
pub mod history;
pub mod init;
pub mod owner;
pub mod phone;
pub mod phone_set;
pub mod py;
pub mod reads;
pub mod recording;
pub mod resources;
pub mod runtime;
pub mod shell;
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
    Change(deploy::Ask),
    Owner(owner::Ask),
    Boxes(boxes::Ask),
    Exec(exec::Ask),
    Shell(shell::Ask),
    GraceSet(grace_set::Ask),
    DoorSet(door_set::Ask),
    Grants(grants::Ask),
    AgentAccess(agent_access::Ask),
    Domains(domains::Ask),
    Data(data::Ask),
    BoxUse(box_use::Ask),
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
    "build",
    "install",
    "deploy",
    "rollback",
    "stop",
    "start",
    "restart",
    "link",
    "seal",
    "config",
    "uninstall",
    "expose",
    "handover",
    "unpark",
    "park",
    "release",
    "exception",
    "box",
    "engine",
    "rehost",
    "sessions",
    "exec",
    "shell",
    "warnings",
    "clean-mode",
    "key",
    "keys",
    "label",
    "test-guests",
    "test-guest",
    "move-plan",
    "surfaces",
    "door",
    "grace",
    "crossover",
    "requests",
    "quota",
    "quotas",
    "access",
    "invite-settings",
    "domain",
    "domains",
    "data",
    "resources",
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
/// The shell's but for [`VERBS`]: every other verb is the shell's.
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
        words: &["build"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["install"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["deploy"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["rollback"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["stop", "start", "restart"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["link"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["seal"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["config"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["uninstall"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["expose"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["handover", "unpark"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["park"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["release"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["exception"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["box"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["engine"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["rehost"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["sessions"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["exec"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["shell"],
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
    // `app` alone and `app --json` ([`bare`]).
    crate::route::Verb::bare(crate::route::Owner::Native),
    crate::route::Verb {
        words: &["warnings"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["clean-mode"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["key"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["keys"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["label"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["test-guests", "test-guest"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["move-plan"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["surfaces"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["door"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["grace", "crossover"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["requests"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["quota", "quotas"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["access"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["invite-settings"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["domain"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["domains"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["data"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["resources"],
        owner: crate::route::Owner::Native,
    },
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
        "doctor" => doctor(rest),
        "exec" => Some(exec::parse(rest)?.map(Ask::Exec)),
        "shell" => Some(shell::parse(rest)?.map(Ask::Shell)),
        "box" => {
            if rest.iter().any(|w| matches!(*w, "-h" | "--help")) {
                return None;
            }
            match rest.first().copied() {
                None | Some("" | "doctor" | "status") => doctor(rest.get(1..).unwrap_or_default()),
                _ => Some(boxes::parse("box", rest)?.map(Ask::Boxes)),
            }
        }
        v if boxes::VERBS.contains(&v) => Some(boxes::parse(v, rest)?.map(Ask::Boxes)),
        "install" if deploy::catalog_install(rest) => {
            if rest.iter().any(|w| matches!(*w, "-h" | "--help")) {
                return None;
            }
            Some(catalog::parse_install(rest).map(Ask::Catalog))
        }
        v if deploy::VERBS.contains(&v) => Some(deploy::parse(v, rest)?.map(Ask::Change)),
        v if owner::VERBS.contains(&v) => Some(owner::parse(v, rest)?.map(Ask::Owner)),
        "grants" if !SKIN_GRANT.contains(rest.first().unwrap_or(&"")) => {
            Some(grants::parse(rest)?.map(Ask::Grants))
        }
        "grant" | "grants" => Some(guests::parse_grant(rest)?.map(Ask::Guests)),
        "token" | "tokens" if rest.first() == Some(&"rotate") => {
            Some(door_set::parse_rotate(&rest[1..])?.map(Ask::DoorSet))
        }
        "access" => Some(agent_access::parse(rest)?.map(Ask::AgentAccess)),
        "invite-settings" => Some(sign_in::parse_invite_settings(rest)?.map(Ask::SignIn)),
        v if grace_set::VERBS.contains(&v) => Some(grace_set::parse(v, rest)?.map(Ask::GraceSet)),
        v if door_set::VERBS.contains(&v) => Some(door_set::parse(v, rest)?.map(Ask::DoorSet)),
        v if domains::VERBS.contains(&v) => Some(domains::parse(v, rest)?.map(Ask::Domains)),
        "data" => Some(data::parse(rest)?.map(Ask::Data)),
        "resources" => Some(box_use::parse(rest)?.map(Ask::BoxUse)),
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

/// `cmd_app_doctor`: `--store` is [`init`]'s, the box and reach doctors
/// [`boxes`]'.
fn doctor(rest: &[&str]) -> Option<Result<Ask, Stop>> {
    match init::parse_doctor(rest) {
        Some(r) => Some(r.map(Ask::Folder)),
        None => Some(boxes::parse_doctor(rest)?.map(Ask::Boxes)),
    }
}

/// `k5 app …`. `None`: the shell's (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    // `app` alone (or with `--json`): the bare row ([`bare`]).
    if crate::route::is_bare(args) {
        return bare(args);
    }
    match args.get(1).and_then(|a| a.to_str()) {
        // `token rotate` is `cmd_app_token_rotate` ([`door_set`]).
        Some("token" | "tokens") => {
            if args.get(2).and_then(|a| a.to_str()) != Some("rotate") {
                return super::skin_token::run_app(args);
            }
        }
        Some("grant" | "grants") => {
            // `grants <app>` is `cmd_app_grants` ([`grants`]), `grant <app>
            // --kind …` `cmd_app_grant_kind` ([`guests`]).
            let third = args.get(2).map_or(Some(""), |a| a.to_str())?;
            if SKIN_GRANT.contains(&third) && !matches!(third, "copy-host" | "copyhost") {
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

/// `app` alone, or `app --json`: `cmd_app` hands the word to `cmd_skin`,
/// whose `_skin_usage` prints the App page on stderr through
/// `_perm_lines` (a line tagged ` #if-cards` only while permission cards
/// are on for this caller, ` #if-no-cards` only while off), then exit 1.
/// Whether cards are on is the shell's probe ([`crate::cards`]): asked
/// only with a passport. After the same settings and gates as every verb.
/// `None` (before anything is printed): the shell's, where the binary
/// can't be sure to ask or read the probe as the shell does.
fn bare(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let page = crate::text::help("app")?;
    let (env, p) = super::help::shell_prelude()?;
    let would_refuse = passport::current(args).would_refuse();
    let cards = crate::cards::ask(&env, &p, || would_refuse)?;
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    // A port curl won't take is the shell's.
    curl_port(settings.port_text.as_deref()?)?;
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    let text = crate::cards::perm_lines(page, crate::cards::on(cards));
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
    Some(Exit::Error)
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
        Ask::Change(c) => deploy::run(&mut ctx, c),
        Ask::Owner(o) => owner::run(&mut ctx, o),
        Ask::Boxes(b) => boxes::run(&mut ctx, b),
        Ask::Exec(e) => exec::run(&mut ctx, e),
        Ask::Shell(sh) => shell::run(&mut ctx, sh),
        Ask::GraceSet(g) => grace_set::run(&mut ctx, g),
        Ask::DoorSet(d) => door_set::run(&mut ctx, d),
        Ask::Grants(g) => grants::run(&mut ctx, g),
        Ask::AgentAccess(a) => agent_access::run(&mut ctx, a),
        Ask::Domains(d) => domains::run(&mut ctx, d),
        Ask::Data(d) => data::run(&mut ctx, d),
        Ask::BoxUse(b) => box_use::run(&mut ctx, b),
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
