//! `k5 reserve <paths…> --agent <name>` and `k5 release [paths…] --agent
//! <name>`: the retiring path reservations, as the shell CLI's
//! `cmd_reserve` and `cmd_release` answer them (`cli/src/usage.sh`).
//! [`super::release`] hands its command lines here.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). Both words are the `connections` tool, an `id`
//!    tool: in a session with `HOOK_SOCK` set, the stale-passport check
//!    and the refusal with no passport ([`prelude::id_tool`]).
//! 2. `_reserve_args`: `--agent <name>` (a next word that is there and
//!    doesn't start with `-`, else `k2 <verb>: '--agent' needs a name.`,
//!    exit 2); any other word starting with `-` is `k2 <verb>: '<word>' is
//!    not a path. …` (exit 2); every other word is a path. The agent
//!    starts as `AGENT_NAME` (its K5_, K2_ and K2SO_ names, empty counts
//!    as unset); none is `Error: --agent required (or set
//!    K2_AGENT_NAME)`, exit 1. `reserve` with no path is `Error: at least
//!    one path required`, exit 1.
//! 3. One `cli_request_post /cli/reserve` (or `/cli/release`) over TCP:
//!    `agent=<the agent, escaped as urlencode does>`, then
//!    `&paths=<the paths joined by commas, escaped the same way>`
//!    (`release` sends no `paths` when it has none).
//! 4. The answer's bytes on stdout as they came, NULs dropped, and exit 0
//!    whatever the status.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer is the transport's line and exit 1; any other
//! failure ends with the code the shell's call returns, with nothing
//! printed. A port that is no port is curl's 3 (or 75 when `TMPDIR` can't
//! be used).
//!
//! The shell answers ([`run`] returns `None`): `-h` or `--help` (the front
//! sends those on; `_reserve_args` reads them as words that are no path),
//! paths whose joined text ends in a newline (`$(…)` drops it), a word that
//! is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported, and a
//! `python3` the binary can't tell.

use super::{curl_port, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::client::{tmpdir_unusable, TMPDIR_UNWRITABLE};
use crate::transport::encode::quote;
use crate::transport::wire::curl;
use crate::transport::{Call, Channel, Error};
use k2_client::{cli_name, env_names};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["reserve"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool both words are in the shell's policy.
pub const TOOL: &str = "connections";

/// No agent from the flags or the environment.
pub const AGENT_REQUIRED: &str = "Error: --agent required (or set K2_AGENT_NAME)";
/// `reserve` with no path.
pub const PATH_REQUIRED: &str = "Error: at least one path required";

/// Which of the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Reserve,
    Release,
}

impl Verb {
    pub fn word(self) -> &'static str {
        match self {
            Verb::Reserve => "reserve",
            Verb::Release => "release",
        }
    }

    pub fn endpoint(self) -> &'static str {
        match self {
            Verb::Reserve => "/cli/reserve",
            Verb::Release => "/cli/release",
        }
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The request's one argument.
    Send(String),
    /// A line on stderr, and the exit.
    Stop(String, Exit),
}

/// `_reserve_args <verb> <words>` and the checks after it, with `agent`
/// from the environment (empty when unset). `None`: the shell's.
pub fn parse(verb: Verb, words: &[&str], agent: &str) -> Option<Ask> {
    let mut agent = agent.to_string();
    let mut paths: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w == "--agent" {
            match words.get(i + 1) {
                Some(next) if !next.starts_with('-') => {
                    agent = next.to_string();
                    i += 2;
                }
                _ => {
                    return Some(Ask::Stop(
                        format!(
                            concat!(cli_name!(), " {}: '--agent' needs a name."),
                            verb.word()
                        ),
                        Exit::Usage,
                    ))
                }
            }
        } else if w.starts_with('-') {
            if matches!(w, "-h" | "--help") {
                return None;
            }
            return Some(Ask::Stop(
                format!(
                    concat!(
                        cli_name!(),
                        " {v}: '{w}' is not a path. ",
                        cli_name!(),
                        " {v} takes paths and --agent <name> only, and is being retired."
                    ),
                    v = verb.word(),
                    w = w
                ),
                Exit::Usage,
            ));
        } else {
            paths.push(w);
            i += 1;
        }
    }
    if agent.is_empty() {
        return Some(Ask::Stop(AGENT_REQUIRED.into(), Exit::Error));
    }
    if verb == Verb::Reserve && paths.is_empty() {
        return Some(Ask::Stop(PATH_REQUIRED.into(), Exit::Error));
    }
    let mut param = format!("agent={}", quote(&agent));
    if !paths.is_empty() {
        let joined = paths.join(",");
        if joined.ends_with('\n') {
            return None;
        }
        param.push_str("&paths=");
        param.push_str(&quote(&joined));
    }
    Some(Ask::Send(param))
}

/// `k5 reserve ...`.
pub fn run(args: &[OsString]) -> Option<Exit> {
    run_verb(Verb::Reserve, args)
}

/// `k5 reserve ...` or `k5 release ...`: `args` is the whole command line
/// after `argv[0]`. `None`: the shell CLI answers it.
pub fn run_verb(verb: Verb, args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let env = |name: &str| std::env::var_os(name);
    let agent = match env_names::first_nonempty_os_in(env_names::AGENT_NAME, env) {
        Some((_, v)) => v.into_string().ok()?,
        None => String::new(),
    };
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    // The words are read after the gates; whether the shell answers is
    // known first.
    let ask = parse(verb, &words, &agent)?;
    if !python::family_native(python::here()) {
        return None;
    }
    Some(match daemon::current() {
        Ok(settings) => run_with(verb, args, ask, hook_sock.as_deref(), settings),
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            Exit::Error
        }
    })
}

fn run_with(
    verb: Verb,
    args: &[OsString],
    ask: Ask,
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
    let param = match ask {
        Ask::Send(p) => p,
        Ask::Stop(line, exit) => {
            out::stderr_line(&line);
            return exit;
        }
    };
    let Some(port) = settings.port_text.as_deref().and_then(curl_port) else {
        return Exit::from_curl(if tmpdir_unusable() {
            TMPDIR_UNWRITABLE
        } else {
            curl::URL_MALFORMAT
        });
    };
    settings.port = Some(port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let exit = match client.call(
        Channel::Tcp,
        verb.endpoint(),
        Call::RequestPost(&[param.as_str()]),
    ) {
        Ok(r) => {
            closing.answer(&r, passport);
            match out::write_flushed(&mut std::io::stdout().lock(), &without_nuls(&r.body)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/usage.sh");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Each line and request as `_reserve_args`, `cmd_reserve` and
    /// `cmd_release` have them.
    #[test]
    fn the_shell_s_lines() {
        let text = source();
        for shell in [
            r#"[[ $# -ge 2 && "$2" != -* ]] || { echo "k2 $verb: '--agent' needs a name." >&2; exit 2; }"#,
            r#"echo "k2 $verb: '$1' is not a path. k2 $verb takes paths and --agent <name> only, and is being retired." >&2"#,
            r#"[[ -z "$_RS_AGENT" ]] && { echo "Error: --agent required (or set K2_AGENT_NAME)" >&2; exit 1; }"#,
            r#"[[ ${#_RS_PATHS[@]} -eq 0 ]] && { echo "Error: at least one path required" >&2; exit 1; }"#,
            r#"cli_request_post "/cli/reserve" "agent=$(urlencode "$_RS_AGENT")&paths=$(urlencode "$joined")""#,
            r#"local params="agent=$(urlencode "$_RS_AGENT")""#,
            r#"params="$params&paths=$(urlencode "$joined")""#,
            r#"cli_request_post "/cli/release" "$params""#,
        ] {
            assert!(text.contains(shell), "usage.sh no longer has {shell:?}");
        }
    }

    #[test]
    fn the_words_as_the_shell_reads_them() {
        let send = |s: &str| Some(Ask::Send(s.to_string()));
        assert_eq!(
            parse(Verb::Reserve, &["a", "b c", "--agent", "x"], ""),
            send("agent=x&paths=a%2Cb%20c")
        );
        assert_eq!(
            parse(Verb::Reserve, &["a"], "env"),
            send("agent=env&paths=a")
        );
        assert_eq!(parse(Verb::Release, &[], "env"), send("agent=env"));
        assert_eq!(
            parse(Verb::Reserve, &[], "env"),
            Some(Ask::Stop(PATH_REQUIRED.into(), Exit::Error))
        );
        assert_eq!(
            parse(Verb::Release, &["p"], ""),
            Some(Ask::Stop(AGENT_REQUIRED.into(), Exit::Error))
        );
        assert_eq!(
            parse(Verb::Release, &["--agent", "-x"], ""),
            Some(Ask::Stop(
                concat!(cli_name!(), " release: '--agent' needs a name.").into(),
                Exit::Usage
            ))
        );
        assert_eq!(
            parse(Verb::Reserve, &["--json"], "a"),
            Some(Ask::Stop(
                format!(
                    "{c} reserve: '--json' is not a path. {c} reserve takes paths and --agent <name> only, and is being retired.",
                    c = cli_name!()
                ),
                Exit::Usage
            ))
        );
        // The agent is escaped as urlencode escapes it.
        assert_eq!(
            parse(Verb::Reserve, &["p", "--agent", "a b&c=d#e%41+é"], ""),
            send("agent=a%20b%26c%3Dd%23e%2541%2B%C3%A9&paths=p")
        );
        assert_eq!(parse(Verb::Release, &[], "x&y"), send("agent=x%26y"));
        assert_eq!(parse(Verb::Reserve, &["p\n"], "a"), None);
        assert_eq!(parse(Verb::Reserve, &["-h"], "a"), None);
    }
}
