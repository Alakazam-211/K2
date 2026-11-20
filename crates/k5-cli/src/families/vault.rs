//! `k5 vault`, as the shell CLI's `cmd_vault` answers it
//! (`cli/src/migrate.sh`, `cli/src/vault.sh`, and `cli/src/job.sh` for
//! `systemd-cred`, the confirm bound to a plan and `placement`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]).
//! 2. `vault` is an `id` tool, so with `HOOK_SOCK` set: the stale-passport
//!    or require-passport lines, exit 3 ([`crate::prelude::id_tool`]).
//! 3. `cmd_vault`: a word not in [`VAULT_VERBS`] is an unknown verb ([`unknown_verb`],
//!    exit 2); then the verb, in one of the modules below.
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The two channels ([`crate::confirm`]): an agent's verbs go through
//! `_sidecar_send` ([`Ctx::call`]); the steps for people only go to the
//! vault socket, `<K2 home>/vault.sock` ([`Ctx::sock`]), after
//! `_vault_person_guard`.
//!
//! The modules:
//!
//! - [`verbs`]: the verbs `cmd_vault`'s own flag parser reads (status,
//!   list, show, set, rotate, hosts, sign, public-key, agent-limit,
//!   delivery, grant, revoke, open, disable, enable, destroy, get, audit,
//!   requests, presets, doctor, env-file, file, proxy-url, import,
//!   service, reveal, recovery, confirm-password, kek, rekey, rollback,
//!   recipient, carry, backup, restore).
//! - [`person`]: the steps for people only that verbs hands on: the
//!   recovery key's `show-once` and `rotate`, `rollback`, `backup` and
//!   `restore`.
//! - [`carry`]: `carry export` and `carry import`.
//! - [`handoff`]: `_vault_handoff` (send, incoming, sent, accept-from,
//!   send-allow, ask, asks, unsend).
//! - [`tools`]: the verbs with parsers of their own: fetch, run,
//!   systemd-cred, placement, placements and sync.
//!
//! - [`run`](mod@run): `_vault_run`, the run helper in this process's
//!   place, the passport on fd 3.
//!
//! The help lines ([`help_line`]): `vault` alone, `help`, and any command
//! line with `-h` or `--help` in it. The shell skips its connection gate
//! for those, and its no-owner refusal too when `-h` or `--help` is in
//! it; then the page ([`sub_page`]), with nothing sent. A `run` line
//! whose `-h` is the command's own (after `--`) runs, gate or none: the
//! binary runs it when the gate would let it through and hands it to the
//! shell otherwise.
//!
//! The shell CLI answers ([`run`] returns `None`): a word that is not
//! UTF-8; `HOOK_SOCK` set with a `CODEX_` variable exported; a port that
//! is set but is no port curl takes; a `python3` the binary can't tell;
//! and every form a module doesn't answer yet ([`native`]).

use super::curl_port;
use crate::confirm::{self, Confirm, Split};
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::Client;
use std::ffi::OsString;
use std::path::PathBuf;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["vault"];
/// The binary answers it; [`run`] hands back a form only the shell answers.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub mod carry;
pub mod handoff;
pub mod person;
pub mod py;
pub mod run;
pub mod systemd;
pub mod tools;
pub mod verbs;

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "vault";

/// `_VAULT_VERBS` (`cli/src/migrate.sh`): every verb `k2 vault` knows.
pub const VAULT_VERBS: &[&str] = &[
    "status",
    "list",
    "ls",
    "show",
    "set",
    "rotate",
    "hosts",
    "sign",
    "public-key",
    "agent-limit",
    "delivery",
    "grant",
    "revoke",
    "open",
    "disable",
    "enable",
    "destroy",
    "get",
    "audit",
    "requests",
    "presets",
    "doctor",
    "env-file",
    "file",
    "proxy-url",
    "sync",
    "import",
    "service",
    "reveal",
    "recovery",
    "confirm-password",
    "kek",
    "rekey",
    "rollback",
    "recipient",
    "carry",
    "backup",
    "restore",
    "systemd-cred",
    "placement",
    "placements",
    "fetch",
    "send",
    "incoming",
    "sent",
    "accept-from",
    "send-allow",
    "ask",
    "asks",
    "unsend",
    "run",
];

/// The verbs `_vault_handoff` answers.
pub const HANDOFF: &[&str] = &[
    "send",
    "incoming",
    "sent",
    "accept-from",
    "send-allow",
    "ask",
    "asks",
    "unsend",
];

/// The verbs with parsers of their own.
pub const TOOLS: &[&str] = &[
    "fetch",
    "run",
    "systemd-cred",
    "placement",
    "placements",
    "sync",
];

/// `_vault_unknown_verb`'s lines: the usage line, and for a word that
/// names a command that needs its secrets, the verb to use.
pub fn unknown_verb(sub: &str) -> Vec<String> {
    let mut lines = vec![format!(
        "usage: unknown vault verb '{sub}' ({} vault --help)",
        k2_client::brand::CLI_NAME
    )];
    if matches!(sub, "exec" | "env" | "inject") {
        lines.push(format!(
            "did you mean run? ({} vault run --help)",
            k2_client::brand::CLI_NAME
        ));
    }
    lines
}

/// What every verb works with: the client, the EXIT trap's lines and the
/// vault socket.
pub struct Ctx {
    pub client: Client,
    pub closing: Closing,
    /// `<K2 home>/vault.sock` ([`confirm::sock_path`]).
    pub sock: PathBuf,
    /// The command line, for the verbs that read the passport rule again.
    pub args: Vec<OsString>,
}

impl Ctx {
    /// The calls carry a passport (for the EXIT trap's dead-passport probe).
    pub fn passport(&self) -> bool {
        self.client.passport().is_some()
    }

    /// `_sidecar_split "$(_sidecar_send METHOD <path> <payload>)"`, no fail:
    /// `Err` only for the transport's own end (its line printed, exit 1).
    pub fn send(&mut self, post: bool, path: &str, payload: &str) -> Result<Split, Exit> {
        confirm::sidecar_call(&mut self.client, post, path, payload)
    }

    /// `_vault_call GET|POST <route> [payload]`: `/cli/vault/<route>`
    /// through `_sidecar_send`; a status that is not 200 is `_vault_fail`
    /// (`Err` with its exit).
    pub fn call(&mut self, post: bool, route: &str, payload: &str) -> Result<Split, Exit> {
        let s = self.send(post, &format!("/cli/vault/{route}"), payload)?;
        if s.ok() {
            Ok(s)
        } else {
            Err(self.fail(&s))
        }
    }

    /// `_vault_sock_call GET|POST <route> [json]`: never ends the command.
    pub fn sock(&mut self, post: bool, route: &str, payload: &[u8]) -> Split {
        let sock = self.sock.clone();
        confirm::sock_call(&mut self.client, &sock, post, route, payload)
    }

    /// `_vault_fail <status> <body>`: the line, the closing line noted,
    /// the exit.
    pub fn fail(&mut self, s: &Split) -> Exit {
        let passport = self.passport();
        confirm::fail(s, &mut self.closing, passport)
    }

    /// `_vault_person_guard`.
    pub fn person_guard(&self) -> Result<(), Exit> {
        confirm::person_guard(&self.sock)
    }

    /// For [`confirm::confirm_token`] and [`confirm::confirm_token_plan`].
    pub fn confirm(&mut self) -> Confirm<'_> {
        let passport = self.passport();
        Confirm {
            client: &mut self.client,
            sock: &self.sock,
            closing: &mut self.closing,
            passport,
        }
    }
}

/// Whether the binary answers `vault <sub> <rest>` (no I/O). Each module
/// says for its own verbs; an unknown verb is answered here.
pub fn native(sub: &str, rest: &[&str]) -> bool {
    if !VAULT_VERBS.contains(&sub) {
        return true;
    }
    if HANDOFF.contains(&sub) {
        handoff::native(sub, rest)
    } else if TOOLS.contains(&sub) {
        tools::native(sub, rest)
    } else {
        verbs::native(sub, rest)
    }
}

/// The words after `vault` make a help line: the shell's connection gate
/// skips it (`cli/src/cli-tools-policy.sh`): `vault` alone, `help`, `-h`
/// or `--help` first, or `-h` or `--help` anywhere.
pub fn help_line(words: &[&str]) -> bool {
    matches!(words.first(), None | Some(&("" | "help" | "--help" | "-h")))
        || words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// The page `_vault_sub_usage <sub>` prints (`cli/src/vault.sh`): the
/// verb's own, or the top page for a verb with none.
pub fn sub_page(sub: &str) -> &'static str {
    match sub {
        "open" => "vault/open",
        "status" => "vault/status",
        "list" | "ls" => "vault/list",
        "show" => "vault/show",
        "set" => "vault/set",
        "rotate" => "vault/rotate",
        "hosts" => "vault/hosts",
        "sign" => "vault/sign",
        "delivery" => "vault/delivery",
        "grant" => "vault/grant",
        "revoke" => "vault/revoke",
        "disable" | "enable" => "vault/disable",
        "destroy" => "vault/destroy",
        "get" => "vault/get",
        "audit" => "vault/audit",
        "requests" => "vault/requests",
        "presets" => "vault/presets",
        "doctor" => "vault/doctor",
        "env-file" => "vault/env-file",
        "run" => "vault/run",
        "proxy-url" => "vault/proxy-url",
        "file" => "vault/file",
        "sync" => "vault/sync",
        "import" => "vault/import",
        "service" => "vault/service",
        "reveal" => "vault/reveal",
        "recovery" => "vault/recovery",
        "confirm-password" => "vault/confirm-password",
        "kek" | "rekey" => "vault/kek",
        "rollback" => "vault/rollback",
        "backup" => "vault/backup",
        "restore" => "vault/restore",
        "recipient" => "vault/recipient",
        "public-key" => "vault/public-key",
        "agent-limit" => "vault/agent-limit",
        "systemd-cred" => "vault/systemd-cred",
        "placement" | "placements" => "vault/placement",
        "carry" => "vault/carry",
        _ => "vault",
    }
}

/// `cat <<'EOF'` of a help page on stdout, exit 0.
pub fn help_page(key: &str) -> Exit {
    match crate::text::help(key) {
        Some(page) => match out::stdout(page) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        },
        None => {
            out::stderr_line(&format!("Error: no help page {key}"));
            Exit::Error
        }
    }
}

/// What `cmd_vault` prints for a help line, after the gates: `None` for a
/// `run` line whose `-h` is the command's (it runs).
fn help_answer(sub: &str, rest: &[&str]) -> Option<Exit> {
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Some(help_page("vault"));
    }
    if !VAULT_VERBS.contains(&sub) {
        for l in unknown_verb(sub) {
            out::stderr_line(&l);
        }
        return Some(Exit::Usage);
    }
    Some(match sub {
        // `_vault_fetch`: `-h` anywhere is `--help`.
        "fetch" => help_page("vault/fetch"),
        "run" => match run::parse(rest) {
            run::Parsed::Help => help_page("vault/run"),
            run::Parsed::Usage(lines) => {
                for l in lines {
                    out::stderr_line(&l);
                }
                Exit::Usage
            }
            run::Parsed::Run { .. } => return None,
        },
        // `_vault_handoff_usage`.
        _ if HANDOFF.contains(&sub) => help_page("vault/send"),
        _ => help_page(sub_page(sub)),
    })
}

/// `None`: the shell CLI answers it (see the module's notes).
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
    let help = help_line(&words);
    let (sub, rest) = match words.split_first() {
        Some((sub, rest)) => (*sub, rest),
        None => ("", &[][..]),
    };
    if !(help || native(sub, rest)) || !python::family_native(python::here()) {
        return None;
    }
    // A run line whose -h is the command's: it runs with no gate in the
    // shell, the same as with one when the gate lets it through.
    let runs = help
        && sub == "run"
        && cfg!(unix)
        && matches!(run::parse(rest), run::Parsed::Run { .. });
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if help && !runs {
        // `_k2_offline_verb`: -h or --help anywhere skips the no-owner
        // refusal too; `vault` alone and `vault help` don't.
        let offline = words.iter().any(|w| matches!(*w, "-h" | "--help"));
        if !offline && prelude::gate(&settings) == Some(prelude::Gate::VirtualDesktop) {
            return Some(prelude::Gate::VirtualDesktop.stop());
        }
    } else {
        if prelude::gate(&settings).is_none()
            && settings.port_text.as_deref().and_then(curl_port).is_none()
        {
            return None;
        }
        if let Some(gate) = prelude::gate(&settings) {
            if runs {
                return None;
            }
            return Some(gate.stop());
        }
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    if help && !runs {
        return help_answer(sub, rest);
    }
    if !VAULT_VERBS.contains(&sub) {
        for l in unknown_verb(sub) {
            out::stderr_line(&l);
        }
        return Some(Exit::Usage);
    }
    let mut settings = settings;
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let mut ctx = Ctx {
        client,
        closing: Closing::default(),
        sock: confirm::sock_path(&settings.home),
        args: args.to_vec(),
    };
    let exit = if HANDOFF.contains(&sub) {
        handoff::run(&mut ctx, sub, rest)
    } else if TOOLS.contains(&sub) {
        tools::run(&mut ctx, sub, rest)
    } else {
        verbs::run(&mut ctx, sub, rest)
    };
    ctx.closing.flush(&mut ctx.client, exit);
    Some(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `_VAULT_VERBS`, word for word.
    #[test]
    fn the_verbs_are_the_shell_s() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/migrate.sh"),
        )
        .unwrap();
        let line = src
            .lines()
            .find(|l| l.starts_with("_VAULT_VERBS=\""))
            .expect("_VAULT_VERBS");
        let shell: Vec<&str> = line
            .trim_start_matches("_VAULT_VERBS=\"")
            .trim_end_matches('"')
            .split_whitespace()
            .collect();
        let mut ours = VAULT_VERBS.to_vec();
        let mut theirs = shell.clone();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(ours, theirs);
        for v in HANDOFF.iter().chain(TOOLS) {
            assert!(VAULT_VERBS.contains(v), "{v}");
        }
    }

    /// `_vault_sub_usage`'s arms, page for page, and every verb it can be
    /// given has a page the binary holds.
    #[test]
    fn the_sub_pages_are_the_shell_s() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/vault.sh"),
        )
        .unwrap();
        let start = src.find("_vault_sub_usage() {").expect("_vault_sub_usage");
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").expect("its end")];
        let lines: Vec<&str> = body.lines().collect();
        let mut arms = 0;
        for (i, l) in lines.iter().enumerate() {
            let Some(labels) = l.trim().strip_suffix(") cat <<'EOF'") else {
                continue;
            };
            let page = lines[i + 1]
                .strip_prefix("%include cli/text/help/")
                .and_then(|p| p.strip_suffix(".txt"))
                .unwrap_or_else(|| panic!("no page after {l}"));
            for verb in labels.split('|') {
                assert_eq!(sub_page(verb), page, "{verb}");
                arms += 1;
            }
        }
        assert!(arms > 30, "{arms} arms");
        assert!(body.contains("*) _vault_usage ;;"));
        assert_eq!(sub_page("nope"), "vault");
        for v in VAULT_VERBS {
            assert!(crate::text::help(sub_page(v)).is_some(), "{v}");
        }
        for p in ["vault", "vault/fetch", "vault/send", "vault/run"] {
            assert!(crate::text::help(p).is_some(), "{p}");
        }
    }

    #[test]
    fn help_lines_are_the_gate_s() {
        assert!(help_line(&[]));
        assert!(help_line(&[""]));
        assert!(help_line(&["help", "x"]));
        assert!(help_line(&["list", "--json", "-h"]));
        assert!(help_line(&["run", "--secret", "A", "--", "ls", "--help"]));
        assert!(!help_line(&["list", "--json"]));
        assert!(!help_line(&["run", "--secret", "A", "--", "ls"]));
    }

    #[test]
    fn an_unknown_verb_says_so() {
        let cli = k2_client::brand::CLI_NAME;
        assert_eq!(
            unknown_verb("nope"),
            [format!(
                "usage: unknown vault verb 'nope' ({cli} vault --help)"
            )]
        );
        assert_eq!(unknown_verb("exec").len(), 2);
        assert_eq!(
            unknown_verb("env")[1],
            format!("did you mean run? ({cli} vault run --help)")
        );
    }
}
