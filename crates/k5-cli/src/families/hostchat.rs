//! `k5 hostchat …`, as the shell CLI's `cmd_hostchat` answers it
//! (`cli/src/hostchat-voice.sh`, `cli/src/chat.sh`): the Sew chat and
//! voice server on this box. Each verb is one or more `_hostchat_call`s
//! ([`super::voice::sew`]) and `_hostchat_print`'s lines ([`print`]).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates. `hostchat` alone, `hostchat ""` and
//!    `hostchat help` skip the connection gate; the virtual desktop's guard
//!    still stops them.
//! 2. `hostchat` is the locked `hostchat` tool, an `id` tool: with
//!    `HOOK_SOCK` set, the stale-passport or require-passport lines, exit 3.
//! 3. The words ([`parse`]): the usage lines, written as the shell's
//!    `echo` writes them (no JSON escaping), exit 2.
//! 4. The calls, then the lines, or the answer as it came with `--json`.
//!    The waits poll the status as the shell does (every 2 s for `enable`,
//!    every 1 s for `upgrade`). The steps for a person (`recovery-key`,
//!    `purge`, `rotate-master --drop-prev`, `upgrade --accept-binary`,
//!    `rollback --accept-data-loss`, `open`) refuse in a session
//!    and take a confirm token over the vault socket
//!    ([`crate::confirm::confirm_token`]); the token goes from that answer
//!    into the next request's body and nowhere else.
//!
//! The sub-families are modules of their own: [`recording`], `media`,
//! `users`, `admin` (people, approvals, on-behalf, log) and `apps`
//! (app-space, home, pass-keys, meet). `deliveries` and `agents` ride
//! [`super::chat`], as the shell's do.
//!
//! A value flag with no value or an empty one is one plain line, exit 2
//! ([`needs_value`], K2's D6: the shell stops on `${2:?…}` with a line
//! naming its own path).
//!
//! Handed to the shell, before anything is printed or sent: a word that
//! is not UTF-8, `-h` or `--help` anywhere (help never acts), `open`
//! without `--print` where there are no unix modes for its private page,
//! and a variable `_compute_send`, `_compute_fail` or `_compute_json`
//! reads from the environment.

mod admin;
mod apps;
mod main_verbs;
mod media;
#[cfg(unix)]
mod open;
pub mod print;
mod recording;
mod users;

use super::voice::{self, sew, sew::Field};
use super::{chat, curl_port};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::text;
use crate::transport::{Auth, Client};
use k2_client::brand::CLI_NAME;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The tool's name in the shell's policy.
pub const TOOL: &str = voice::TOOL;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `_hostchat_usage`, exit 0.
    Help,
    /// A line on stderr, exit 2.
    Bad(String),
    /// The shell's (see the module's notes).
    Shell,
    /// A help page by its key ([`text::help`]), exit 0.
    Text(&'static str),
    /// One of `cmd_hostchat`'s own verbs.
    Main(Main),
    /// `recording …`.
    Rec(recording::Rec),
    /// `media …`.
    Media(media::Media),
    /// `users …`.
    Users(users::Users),
    /// `people`, `approvals`, `on-behalf` and `log`.
    Admin(admin::Admin),
    /// `app-space`, `home`, `pass-keys` and `meet`.
    Apps(apps::Apps),
    /// Nothing printed, this exit (the shell's `set -e` on a failed
    /// `shift 2`).
    Silent(Exit),
}

/// The flags and words of `cmd_hostchat`'s own verbs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Main {
    pub sub: String,
    pub json: bool,
    pub tarball: String,
    pub recovery: String,
    pub identity: String,
    pub voice: bool,
    pub voice_url: String,
    pub media_tarball: String,
    pub node_ip: String,
    pub wait: bool,
    pub health: bool,
    pub dry: bool,
    pub confirm: bool,
    pub drop_grants: bool,
    pub drop_prev: bool,
    pub keep_backups: bool,
    pub host: String,
    pub public_url: String,
    pub accept_binary: String,
    pub print_only: bool,
    pub accept_loss: bool,
    pub pos: Vec<String>,
}

/// `{"error":"usage","hint":"<hint>"}`, as `echo` writes it.
pub fn usage(hint: &str) -> String {
    format!("{{\"error\":\"usage\",\"hint\":\"{hint}\"}}")
}

fn bad(hint: impl AsRef<str>) -> Plan {
    Plan::Bad(usage(hint.as_ref()))
}

/// A value flag as the last word or with an empty value, in `cmd_hostchat`,
/// `_hostchat_media` or `_hostchat_users`. The shell stops there on its own
/// `"${2:?<flag> <what>}"` line (the script's path and line in it, exit 1);
/// the binary says what is missing and exits 2, K2's D6 ruling for the
/// class (`scripts/cli-parity.sh`'s named difference `edge-usage-exit-2`).
pub fn needs_value(flag: &str) -> Plan {
    Plan::Bad(format!(
        "Error: {flag} needs a value (try '{CLI_NAME} hostchat --help')"
    ))
}

/// `cmd_hostchat`'s own verbs, after its flags.
const VERBS_OWN: &[&str] = &[
    "status",
    "preflight",
    "enable",
    "disable",
    "rollback",
    "doctor",
    "reconfigure",
    "restart",
    "adopt",
    "backup",
    "backups",
    "job",
    "upgrade",
    "restore",
    "rotate-master",
    "recovery-key",
    "config",
    "uninstall",
    "app-url",
    "ship",
    "mirror",
    "open",
    "purge",
];

/// The words after `hostchat`, for `cmd_hostchat`'s own verbs (the
/// sub-families are [`run`]'s).
pub fn parse(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let Some((&sub, rest)) = words.split_first() else {
        return Plan::Help;
    };
    if matches!(sub, "-h" | "--help" | "help") {
        return Plan::Help;
    }
    if sub == "act-as" {
        return bad(format!("act-as is gone: assistants post on a person's behalf at once and the person reviews it later: {c} hostchat on-behalf"));
    }
    let mut m = Main {
        sub: sub.to_string(),
        wait: true,
        ..Main::default()
    };
    let mut voice_flag = false;
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let slot = match w {
            "--json" => {
                m.json = true;
                None
            }
            "--tarball" => Some(&mut m.tarball),
            "--recovery-key-file" => Some(&mut m.recovery),
            "--identity" => Some(&mut m.identity),
            "--voice" => {
                m.voice = true;
                voice_flag = true;
                None
            }
            "--voice-url" => {
                m.voice = true;
                voice_flag = true;
                Some(&mut m.voice_url)
            }
            "--media-tarball" => {
                m.voice = true;
                Some(&mut m.media_tarball)
            }
            "--node-ip" => Some(&mut m.node_ip),
            "--no-wait" => {
                m.wait = false;
                None
            }
            "--health" => {
                m.health = true;
                None
            }
            "--dry-run" => {
                m.dry = true;
                None
            }
            "--confirm" => {
                m.confirm = true;
                None
            }
            "--drop-grants" => {
                m.drop_grants = true;
                None
            }
            "--drop-prev" => {
                m.drop_prev = true;
                None
            }
            "--accept-binary" => Some(&mut m.accept_binary),
            "--accept-data-loss" => {
                m.accept_loss = true;
                None
            }
            "--keep-backups" => {
                m.keep_backups = true;
                None
            }
            "--confirm-hostname" => Some(&mut m.host),
            "--public-url" => Some(&mut m.public_url),
            "--print" => {
                m.print_only = true;
                None
            }
            "--setup-url" => {
                return bad(format!("Sew builds setup links as <public-url>/setup: {c} hostchat config --public-url https://HOST"));
            }
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => {
                return bad(format!("unknown flag {w} ({c} hostchat --help)"));
            }
            w => {
                m.pos.push(w.to_string());
                i += 1;
                continue;
            }
        };
        match slot {
            None => i += 1,
            Some(slot) => {
                let Some(v) = rest.get(i + 1).filter(|v| !v.is_empty()) else {
                    return needs_value(w);
                };
                *slot = v.to_string();
                i += 2;
            }
        }
    }
    if !matches!(m.identity.as_str(), "" | "k2" | "standalone") {
        return bad("--identity is k2 or standalone");
    }
    let sub = m.sub.as_str();
    if sub != "enable" && (voice_flag || !m.node_ip.is_empty()) {
        return bad(format!("--voice, --voice-url and --node-ip only apply to {c} hostchat enable. {c} hostchat {sub} does not set up voice; to install or replace voice from a tarball: {c} hostchat media install --tarball FILE"));
    }
    if !m.media_tarball.is_empty() && !matches!(sub, "enable" | "upgrade" | "rollback") {
        return bad(format!("--media-tarball only applies to {c} hostchat enable, upgrade and rollback. To install or replace voice from a tarball: {c} hostchat media install --tarball FILE"));
    }
    if m.accept_loss && sub != "rollback" {
        return bad(format!(
            "--accept-data-loss only applies to {c} hostchat rollback"
        ));
    }
    if !VERBS_OWN.contains(&sub) {
        return Plan::Bad(format!(
            "{{\"error\":\"usage\",\"hint\":\"unknown hostchat command '{sub}' ({c} hostchat --help)\"}}"
        ));
    }
    // The page `open` hands the browser is made with unix modes.
    if sub == "open" && !m.print_only && !cfg!(unix) {
        return Plan::Shell;
    }
    Plan::Main(m)
}

/// Whether the shell's connection gate lets the line by.
fn gate_skipped(words: &[&str]) -> bool {
    matches!(words.first(), None | Some(&("" | "help")))
}

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["hostchat"];
/// The binary answers it ([`run`] hands some lines back to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 hostchat ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(n, _)| n)) {
        return None;
    }
    if sew::env_shell(std::env::vars_os().map(|(n, _)| n))
        || std::env::var_os("K2_CP_MAX_TIME").is_some()
    {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let skip = gate_skipped(&words);
    match words.first().copied().unwrap_or("") {
        "deliveries" | "delivery" => {
            return chat::start(args, &words, chat::deliveries(&words[1..]), skip, TOOL)
        }
        "agents" | "agent" => {
            return chat::start(args, &words, chat::access(&words[1..]), skip, TOOL)
        }
        _ => {}
    }
    let plan = match words.first().copied() {
        Some("recording") => recording::parse(&words[1..]),
        Some("media") => media::parse(&words[1..]),
        Some("users" | "user") => users::parse(&words[1..]),
        Some("people" | "person") => admin::people(&words[1..]),
        Some("approvals" | "approval") => admin::approvals(&words[1..]),
        Some("on-behalf" | "behalf") => admin::on_behalf(&words[1..]),
        Some("owner-sessions" | "owner-session") => admin::owner_sessions(&words[1..]),
        Some("log") => admin::log(&words[1..]),
        Some(w @ ("app-space" | "app-spaces")) => apps::app_space(w, &words[1..]),
        Some("home") => apps::home(&words[1..]),
        Some("pass-keys") => apps::pass_keys(&words[1..]),
        Some("meet") => apps::meet(&words[1..]),
        _ => parse(&words),
    };
    if plan == Plan::Shell || !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if !skip
        && prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, plan, skip, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    plan: Plan,
    skip: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    match prelude::gate(&settings) {
        Some(g @ Gate::VirtualDesktop) => return g.stop(),
        Some(g @ Gate::CannotConnect) if !skip => return g.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let page = |key: &str| match out::stdout(text::help(key).unwrap_or_default()) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    };
    let m = match plan {
        Plan::Help => return page("hostchat"),
        Plan::Text(key) => return page(key),
        Plan::Silent(exit) => return exit,
        Plan::Bad(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
        Plan::Shell => unreachable!("run hands it to the shell"),
        p @ (Plan::Main(_)
        | Plan::Rec(_)
        | Plan::Media(_)
        | Plan::Users(_)
        | Plan::Admin(_)
        | Plan::Apps(_)) => p,
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut ctx = Ctx {
        in_session: matches!(settings.auth, Auth::Passport { .. }),
        client,
        closing: Closing::default(),
        sock: crate::confirm::sock_path(&settings.home),
    };
    let exit = match &m {
        Plan::Main(m) => main_verbs::run(&mut ctx, m),
        Plan::Rec(r) => recording::run(&mut ctx, r),
        Plan::Media(m) => media::run(&mut ctx, m),
        Plan::Users(u) => users::run(&mut ctx, u),
        Plan::Admin(a) => admin::run(&mut ctx, a),
        Plan::Apps(a) => apps::run(&mut ctx, a),
        _ => unreachable!("printed above"),
    };
    ctx.closing.flush(&mut ctx.client, exit);
    exit
}

/// What every call of a line needs.
pub struct Ctx {
    pub client: Client,
    pub closing: Closing,
    /// The vault socket, for a person's confirm.
    pub sock: PathBuf,
    /// `_compute_in_session`: the calls carry a session passport.
    pub in_session: bool,
}

impl Ctx {
    /// `_hostchat_call`: the body of a 200, or the exit after
    /// `_compute_fail`'s line.
    pub fn call(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<String, Exit> {
        sew::call(
            &mut self.client,
            &mut self.closing,
            post,
            path,
            payload,
            tcp,
        )
        .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

/// `_compute_json`, with its one rule [`sew::json`] has no room for: an
/// argument `K2_CJ_<name>=<value>` with `Token=` in it rides stdin, so
/// it comes first, as text, under its whole name.
pub fn cj(fields: &[(&'static str, Field)]) -> String {
    let mut first: Vec<(String, Field)> = Vec::new();
    let mut rest: Vec<(String, Field)> = Vec::new();
    for (key, f) in fields {
        let (suffix, text) = match f {
            Field::S(t) => ("", t),
            Field::N(t) => ("_n", t),
            Field::B(t) => ("_b", t),
            Field::J(t) => ("_j", t),
            Field::V(_) => {
                rest.push((key.to_string(), f.clone()));
                continue;
            }
        };
        let arg = format!("{key}{suffix}={text}");
        if !arg.contains("Token=") {
            rest.push((key.to_string(), f.clone()));
            continue;
        }
        for line in arg.split('\n') {
            if let Some((k, v)) = line.split_once('=') {
                if !k.is_empty() && !v.is_empty() {
                    first.push((k.to_string(), Field::S(v.to_string())));
                }
            }
        }
    }
    first.extend(rest);
    let borrowed: Vec<(&str, Field)> = first.iter().map(|(k, f)| (k.as_str(), f.clone())).collect();
    sew::json(&borrowed).unwrap_or_default()
}

#[cfg(test)]
mod tests;
