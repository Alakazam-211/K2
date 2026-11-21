//! Settings read from the environment under more than one name: the one
//! registry of those names, and the one reader.
//!
//! A setting is read under its `K5_X` name first, then `K2_X`, then
//! `K2SO_X` (each only where that name exists), so an operator's old line
//! keeps working. Every setting with more than one name is listed here,
//! newest name first, and read with [`first_set`] (or [`first_set_os`] for
//! a path, [`parse`] for a value that must parse).
//!
//! The command line's settings are a section of their own ([`CLI`]). The
//! shell CLI reads every one of them under all three names, and an empty
//! value counts as unset there; the Rust CLI reads them the same way, with
//! [`first_nonempty_os`].
//!
//! Read side only: what K2 *sets* for a child process (an agent's terminal,
//! a job, an App) is decided where it is set, and no `K5_` name is set for
//! a child yet.

use std::ffi::OsString;
use std::fmt;
use std::str::FromStr;

// ── The registry ────────────────────────────────────────────────────────
// Newest name first. A test below holds every entry to the naming rule.

/// The K2 home folder itself (what `~/.k2` is otherwise), for rigs and
/// tests. Absolute; see `paths::k2_home`.
pub const HOME_OVERRIDE: &[&str] = &["K5_HOME_OVERRIDE", "K2_HOME_OVERRIDE"];
/// The `daemon-latest.json` URL the daemon checks for updates.
pub const DAEMON_MANIFEST_URL: &[&str] = &["K5_DAEMON_MANIFEST_URL", "K2_DAEMON_MANIFEST_URL", "K2SO_DAEMON_MANIFEST_URL"];
/// The host name in the "the app isn't running" message.
pub const HOSTNAME: &[&str] = &["K5_HOSTNAME", "K2SO_HOSTNAME"];
/// The per-call perf log switch.
pub const PERF: &[&str] = &["K5_PERF", "K2SO_PERF"];
/// The wake spawn trace switch.
pub const TRACE_WAKE_SPAWN: &[&str] = &["K5_TRACE_WAKE_SPAWN", "K2SO_TRACE_WAKE_SPAWN"];
/// The stand-in command headless wakes run in tests.
pub const WAKE_HEADLESS_TEST_COMMAND: &[&str] = &["K5_WAKE_HEADLESS_TEST_COMMAND", "K2SO_WAKE_HEADLESS_TEST_COMMAND"];
/// The pending-live queue root.
pub const PENDING_LIVE_ROOT: &[&str] = &["K5_PENDING_LIVE_ROOT", "K2SO_PENDING_LIVE_ROOT"];
/// The active reaper's tick, in seconds.
pub const ACTIVE_REAPER_TICK_SECS: &[&str] = &["K5_ACTIVE_REAPER_TICK_SECS", "K2SO_ACTIVE_REAPER_TICK_SECS"];
/// The active reaper's grace, in milliseconds.
pub const ACTIVE_REAPER_GRACE_MS: &[&str] = &["K5_ACTIVE_REAPER_GRACE_MS", "K2SO_ACTIVE_REAPER_GRACE_MS"];
/// The session watchdog's switch and thresholds.
pub const WATCHDOG_DISABLED: &[&str] = &["K5_WATCHDOG_DISABLED", "K2_WATCHDOG_DISABLED", "K2SO_WATCHDOG_DISABLED"];
pub const WATCHDOG_WARN_SECS: &[&str] = &["K5_WATCHDOG_WARN_SECS", "K2SO_WATCHDOG_WARN_SECS"];
pub const WATCHDOG_CTRL_C_SECS: &[&str] = &["K5_WATCHDOG_CTRL_C_SECS", "K2SO_WATCHDOG_CTRL_C_SECS"];
pub const WATCHDOG_KILL_SECS: &[&str] = &["K5_WATCHDOG_KILL_SECS", "K2SO_WATCHDOG_KILL_SECS"];
pub const WATCHDOG_SPAWN_GRACE_SECS: &[&str] = &["K5_WATCHDOG_SPAWN_GRACE_SECS", "K2SO_WATCHDOG_SPAWN_GRACE_SECS"];
pub const WATCHDOG_POLL_SECS: &[&str] = &["K5_WATCHDOG_POLL_SECS", "K2SO_WATCHDOG_POLL_SECS"];

// ── The command line's settings ─────────────────────────────────────────
// What the shell CLI (`cli/src`) reads with `_envn_into`, `_envn`,
// `_envn_any`, or spelled inline as `${K5_X:-${K2_X:-${K2SO_X:-}}}`. Its
// reader tries K5_, K2_ and K2SO_ for every name, whether or not a K2SO_
// spelling was ever set by anything, so each entry here lists all three:
// the names the CLI tries, in its order. An empty value counts as unset
// (see [`first_nonempty_os`]). `crates/k5-cli/tests/env_names_parity.rs`
// reads cli/src and fails when a read there and an entry here differ.
//
// Not here: `K2_AIRGAP`, the one setting the shell reads under one name
// and counts as set even when empty (`crate::airgap`), and the
// `K5_CLI_*` names the Rust front hands the shell (one name each).

/// The daemon's port, for a terminal K2 started.
pub const PORT: &[&str] = &["K5_PORT", "K2_PORT", "K2SO_PORT"];
/// The daemon's host (the shell's default is `127.0.0.1`).
pub const HOST: &[&str] = &["K5_HOST", "K2_HOST", "K2SO_HOST"];
/// The session's token (its passport).
pub const HOOK_TOKEN: &[&str] = &["K5_HOOK_TOKEN", "K2_HOOK_TOKEN", "K2SO_HOOK_TOKEN"];
/// The session's own socket to the daemon.
pub const HOOK_SOCK: &[&str] = &["K5_HOOK_SOCK", "K2_HOOK_SOCK", "K2SO_HOOK_SOCK"];
/// The workspace the session runs in.
pub const PROJECT_PATH: &[&str] = &["K5_PROJECT_PATH", "K2_PROJECT_PATH", "K2SO_PROJECT_PATH"];
/// The session's private secrets folder (a re-minted passport lands there).
pub const SECRETS_DIR: &[&str] = &["K5_SECRETS_DIR", "K2_SECRETS_DIR", "K2SO_SECRETS_DIR"];
/// `1`: never fall back to the owner token on disk.
pub const NO_OWNER_FALLBACK: &[&str] = &["K5_NO_OWNER_FALLBACK", "K2_NO_OWNER_FALLBACK", "K2SO_NO_OWNER_FALLBACK"];
/// The agent the session belongs to.
pub const AGENT_NAME: &[&str] = &["K5_AGENT_NAME", "K2_AGENT_NAME", "K2SO_AGENT_NAME"];
/// The session's id.
pub const SESSION_ID: &[&str] = &["K5_SESSION_ID", "K2_SESSION_ID", "K2SO_SESSION_ID"];
/// The session's cell kind (`canonical`, `sidecar`).
pub const CELL: &[&str] = &["K5_CELL", "K2_CELL", "K2SO_CELL"];
/// `1` in an API cell.
pub const API_CELL: &[&str] = &["K5_API_CELL", "K2_API_CELL", "K2SO_API_CELL"];
/// The primary a sidecar belongs to.
pub const PRIMARY: &[&str] = &["K5_PRIMARY", "K2_PRIMARY", "K2SO_PRIMARY"];
/// A sidecar's name.
pub const SIDECAR_NAME: &[&str] = &["K5_SIDECAR_NAME", "K2_SIDECAR_NAME", "K2SO_SIDECAR_NAME"];
/// The terminal pane the session runs in.
pub const PANE_ID: &[&str] = &["K5_PANE_ID", "K2_PANE_ID", "K2SO_PANE_ID"];
/// The token for a remote daemon.
pub const REMOTE_TOKEN: &[&str] = &["K5_REMOTE_TOKEN", "K2_REMOTE_TOKEN", "K2SO_REMOTE_TOKEN"];
/// The Connect service's base URL.
pub const CONNECT_BASE: &[&str] = &["K5_CONNECT_BASE", "K2_CONNECT_BASE", "K2SO_CONNECT_BASE"];
/// `1`: skip the approval prompt.
pub const NO_APPROVE: &[&str] = &["K5_NO_APPROVE", "K2_NO_APPROVE", "K2SO_NO_APPROVE"];
/// The approval helper to run instead of the installed one.
pub const APPROVE_BIN: &[&str] = &["K5_APPROVE_BIN", "K2_APPROVE_BIN", "K2SO_APPROVE_BIN"];
/// A compute node's enrolment code.
pub const NODE_ENROLL_CODE: &[&str] = &["K5_NODE_ENROLL_CODE", "K2_NODE_ENROLL_CODE", "K2SO_NODE_ENROLL_CODE"];
/// How the vault asks to confirm (`vault-password`).
pub const VAULT_CONFIRM_METHOD: &[&str] = &["K5_VAULT_CONFIRM_METHOD", "K2_VAULT_CONFIRM_METHOD", "K2SO_VAULT_CONFIRM_METHOD"];
/// The poll interval of `ci` waits, in seconds.
pub const CI_WAIT_SECS: &[&str] = &["K5_CI_WAIT_SECS", "K2_CI_WAIT_SECS", "K2SO_CI_WAIT_SECS"];
/// The poll interval of `llm` waits, in seconds.
pub const LLMA_POLL_SECS: &[&str] = &["K5_LLMA_POLL_SECS", "K2_LLMA_POLL_SECS", "K2SO_LLMA_POLL_SECS"];
/// `1`: feedback's environment detection runs in test mode.
pub const TEST_FEEDBACK_DETECT: &[&str] = &["K5_TEST_FEEDBACK_DETECT", "K2_TEST_FEEDBACK_DETECT", "K2SO_TEST_FEEDBACK_DETECT"];
/// How long `hostchat open`'s redirect page stays, in seconds (a test
/// shortens it; never above 30).
pub const TEST_HOSTCHAT_OPEN_WAIT: &[&str] =
    &["K5_TEST_HOSTCHAT_OPEN_WAIT", "K2_TEST_HOSTCHAT_OPEN_WAIT", "K2SO_TEST_HOSTCHAT_OPEN_WAIT"];
/// `talk`'s settle times, in seconds, its submit key and its question cap.
pub const TALK_NAV_SETTLE: &[&str] = &["K5_TALK_NAV_SETTLE", "K2_TALK_NAV_SETTLE", "K2SO_TALK_NAV_SETTLE"];
pub const TALK_ANSWER_SETTLE: &[&str] = &["K5_TALK_ANSWER_SETTLE", "K2_TALK_ANSWER_SETTLE", "K2SO_TALK_ANSWER_SETTLE"];
pub const TALK_SUBMIT_KEY: &[&str] = &["K5_TALK_SUBMIT_KEY", "K2_TALK_SUBMIT_KEY", "K2SO_TALK_SUBMIT_KEY"];
pub const TALK_SCOUT_MAX: &[&str] = &["K5_TALK_SCOUT_MAX", "K2_TALK_SCOUT_MAX", "K2SO_TALK_SCOUT_MAX"];
/// The mail commands' HTTP timeout, in seconds.
pub const MAIL_HTTP_TIMEOUT: &[&str] = &["K5_MAIL_HTTP_TIMEOUT", "K2_MAIL_HTTP_TIMEOUT", "K2SO_MAIL_HTTP_TIMEOUT"];
/// The mail server upgrade wait: its poll and when to give up, in seconds.
pub const HOSTMAIL_UPGRADE_POLL: &[&str] = &["K5_HOSTMAIL_UPGRADE_POLL", "K2_HOSTMAIL_UPGRADE_POLL", "K2SO_HOSTMAIL_UPGRADE_POLL"];
pub const HOSTMAIL_UPGRADE_LOST_SECS: &[&str] =
    &["K5_HOSTMAIL_UPGRADE_LOST_SECS", "K2_HOSTMAIL_UPGRADE_LOST_SECS", "K2SO_HOSTMAIL_UPGRADE_LOST_SECS"];
/// `1`: no warning for a plain http:// --host (`_k2_plain_http_warn`, `k2 clone`).
pub const PLAIN_HTTP_OK: &[&str] = &["K5_PLAIN_HTTP_OK", "K2_PLAIN_HTTP_OK", "K2SO_PLAIN_HTTP_OK"];

/// The command line's section of the registry.
pub const CLI: &[&[&str]] = &[
    PORT,
    HOST,
    HOOK_TOKEN,
    HOOK_SOCK,
    PROJECT_PATH,
    SECRETS_DIR,
    NO_OWNER_FALLBACK,
    AGENT_NAME,
    SESSION_ID,
    CELL,
    API_CELL,
    PRIMARY,
    SIDECAR_NAME,
    PANE_ID,
    REMOTE_TOKEN,
    CONNECT_BASE,
    NO_APPROVE,
    APPROVE_BIN,
    NODE_ENROLL_CODE,
    VAULT_CONFIRM_METHOD,
    CI_WAIT_SECS,
    LLMA_POLL_SECS,
    TEST_FEEDBACK_DETECT,
    TEST_HOSTCHAT_OPEN_WAIT,
    TALK_NAV_SETTLE,
    TALK_ANSWER_SETTLE,
    TALK_SUBMIT_KEY,
    TALK_SCOUT_MAX,
    MAIL_HTTP_TIMEOUT,
    HOSTMAIL_UPGRADE_POLL,
    HOSTMAIL_UPGRADE_LOST_SECS,
    PLAIN_HTTP_OK,
];

/// Every entry of the registry.
pub const ALL: &[&[&str]] = &[
    HOME_OVERRIDE,
    DAEMON_MANIFEST_URL,
    HOSTNAME,
    PERF,
    TRACE_WAKE_SPAWN,
    WAKE_HEADLESS_TEST_COMMAND,
    PENDING_LIVE_ROOT,
    ACTIVE_REAPER_TICK_SECS,
    ACTIVE_REAPER_GRACE_MS,
    WATCHDOG_DISABLED,
    WATCHDOG_WARN_SECS,
    WATCHDOG_CTRL_C_SECS,
    WATCHDOG_KILL_SECS,
    WATCHDOG_SPAWN_GRACE_SECS,
    WATCHDOG_POLL_SECS,
    // The command line's section, as in [`CLI`].
    PORT,
    HOST,
    HOOK_TOKEN,
    HOOK_SOCK,
    PROJECT_PATH,
    SECRETS_DIR,
    NO_OWNER_FALLBACK,
    AGENT_NAME,
    SESSION_ID,
    CELL,
    API_CELL,
    PRIMARY,
    SIDECAR_NAME,
    PANE_ID,
    REMOTE_TOKEN,
    CONNECT_BASE,
    NO_APPROVE,
    APPROVE_BIN,
    NODE_ENROLL_CODE,
    VAULT_CONFIRM_METHOD,
    CI_WAIT_SECS,
    LLMA_POLL_SECS,
    TEST_FEEDBACK_DETECT,
    TEST_HOSTCHAT_OPEN_WAIT,
    TALK_NAV_SETTLE,
    TALK_ANSWER_SETTLE,
    TALK_SUBMIT_KEY,
    TALK_SCOUT_MAX,
    MAIL_HTTP_TIMEOUT,
    HOSTMAIL_UPGRADE_POLL,
    HOSTMAIL_UPGRADE_LOST_SECS,
    PLAIN_HTTP_OK,
];

/// The prefixes a name can have, newest first.
pub const PREFIXES: &[&str] = &["K5_", "K2_", "K2SO_"];

// ── The reader ──────────────────────────────────────────────────────────

/// The value of the first of `names` that is set, with the name it was
/// read under. A name that is set wins over the ones after it whatever
/// its value: an operator who sets the new name means it.
pub fn first_set(names: &[&'static str]) -> Option<(&'static str, String)> {
    first_set_in(names, |name| std::env::var(name).ok())
}

/// [`first_set`] over any lookup, so the rule can be tested without
/// touching the process environment.
pub fn first_set_in(names: &[&'static str], lookup: impl Fn(&str) -> Option<String>) -> Option<(&'static str, String)> {
    names.iter().find_map(|name| lookup(name).map(|v| (*name, v)))
}

/// [`first_set`] for a value that need not be UTF-8 (a path).
pub fn first_set_os(names: &[&'static str]) -> Option<(&'static str, OsString)> {
    first_set_os_in(names, |name| std::env::var_os(name))
}

/// [`first_set_os`] over any lookup.
pub fn first_set_os_in(
    names: &[&'static str],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Option<(&'static str, OsString)> {
    names.iter().find_map(|name| lookup(name).map(|v| (*name, v)))
}

/// The command line's reader: the value of the first of `names` that is
/// set and not empty, with the name it was read under. An empty value
/// counts as unset and the next name is tried, as the shell CLI's `_envn`
/// (`cli/src/prelude.sh`) has always done. The Rust CLI reads its settings
/// ([`CLI`]) with this so it answers as the shell does; everything else
/// keeps [`first_set`], where a name that is set wins whatever its value.
///
/// The value need not be UTF-8: the shell takes any bytes.
pub fn first_nonempty_os(names: &[&'static str]) -> Option<(&'static str, OsString)> {
    first_nonempty_os_in(names, |name| std::env::var_os(name))
}

/// [`first_nonempty_os`] over any lookup.
pub fn first_nonempty_os_in(
    names: &[&'static str],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Option<(&'static str, OsString)> {
    names.iter().find_map(|name| lookup(name).filter(|v| !v.is_empty()).map(|v| (*name, v)))
}

/// [`first_nonempty_os`]'s rule over a lookup of `String`s.
pub fn first_nonempty_in(names: &[&'static str], lookup: impl Fn(&str) -> Option<String>) -> Option<(&'static str, String)> {
    names.iter().find_map(|name| lookup(name).filter(|v| !v.is_empty()).map(|v| (*name, v)))
}

/// A setting that is set but can't be used: the name it was read under,
/// its value and why. Shown as is, so it names the variable to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    pub name: &'static str,
    pub value: String,
    pub why: String,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={:?} is not valid: {}", self.name, self.value, self.why)
    }
}

impl std::error::Error for Malformed {}

/// The setting parsed as `T`: `Ok(None)` when no name is set (the caller's
/// documented default applies), `Err` naming the variable when the name
/// that is set holds something that doesn't parse. Never falls through to
/// an older name on a bad value: the name that is set is the one meant.
pub fn parse<T: FromStr>(names: &[&'static str]) -> Result<Option<T>, Malformed>
where
    T::Err: fmt::Display,
{
    parse_in(names, |name| std::env::var_os(name))
}

/// [`parse`] over any lookup.
pub fn parse_in<T: FromStr>(
    names: &[&'static str],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<T>, Malformed>
where
    T::Err: fmt::Display,
{
    let Some((name, raw)) = first_set_os_in(names, lookup) else { return Ok(None) };
    let Some(value) = raw.to_str() else {
        return Err(Malformed { name, value: raw.to_string_lossy().into_owned(), why: "not UTF-8".to_string() });
    };
    value
        .parse::<T>()
        .map(Some)
        .map_err(|e| Malformed { name, value: value.to_string(), why: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &["K5_X", "K2_X", "K2SO_X"];

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    fn env_os(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn nothing_set_is_none() {
        assert_eq!(first_set_in(NAMES, env(&[])), None);
        assert_eq!(first_set_in(NAMES, env(&[("K5_Y", "1")])), None);
        assert_eq!(first_set_in(&[], env(&[("K5_X", "1")])), None);
        assert_eq!(first_set_os_in(NAMES, env_os(&[])), None);
    }

    #[test]
    fn each_name_is_read_and_reported() {
        assert_eq!(first_set_in(NAMES, env(&[("K5_X", "a")])), Some(("K5_X", "a".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2_X", "b")])), Some(("K2_X", "b".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "c")])), Some(("K2SO_X", "c".to_string())));
        assert_eq!(first_set_os_in(NAMES, env_os(&[("K2_X", "/p")])), Some(("K2_X", OsString::from("/p"))));
    }

    #[test]
    fn the_earlier_name_wins_whatever_its_value() {
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "old"), ("K5_X", "new")])), Some(("K5_X", "new".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "old"), ("K2_X", "mid")])), Some(("K2_X", "mid".to_string())));
        assert_eq!(first_set_in(NAMES, env(&[("K2SO_X", "9"), ("K5_X", "")])), Some(("K5_X", String::new())), "set and empty still wins");
        assert_eq!(first_set_os_in(NAMES, env_os(&[("K2_X", "/old"), ("K5_X", "/new")])), Some(("K5_X", OsString::from("/new"))));
    }

    #[test]
    fn parse_reads_the_new_name_first_and_the_old_ones_after() {
        assert_eq!(parse_in::<u64>(NAMES, env_os(&[("K5_X", "7"), ("K2_X", "8"), ("K2SO_X", "9")])), Ok(Some(7)));
        assert_eq!(parse_in::<u64>(NAMES, env_os(&[("K2_X", "8"), ("K2SO_X", "9")])), Ok(Some(8)));
        assert_eq!(parse_in::<u64>(NAMES, env_os(&[("K2SO_X", "9")])), Ok(Some(9)));
    }

    #[test]
    fn parse_with_no_name_set_leaves_the_default_to_the_caller() {
        assert_eq!(parse_in::<u64>(NAMES, env_os(&[])), Ok(None));
        let secs: u64 = parse_in(NAMES, env_os(&[])).expect("absent is not an error").unwrap_or(600);
        assert_eq!(secs, 600, "the documented default applies when no name is set");
    }

    #[test]
    fn a_malformed_value_fails_loudly_and_names_the_variable() {
        let err = parse_in::<u64>(NAMES, env_os(&[("K5_X", "ten"), ("K2_X", "10")])).expect_err("a bad value is an error");
        assert_eq!(err.name, "K5_X", "the name that is set is the one reported, never skipped for an older one");
        assert_eq!(err.value, "ten");
        assert!(err.to_string().starts_with("K5_X=\"ten\" is not valid: "), "{err}");
        let err = parse_in::<u64>(NAMES, env_os(&[("K2SO_X", "")])).expect_err("empty is not a number");
        assert_eq!(err.name, "K2SO_X");
    }

    #[cfg(unix)]
    #[test]
    fn a_value_that_is_not_utf8_fails_loudly() {
        use std::os::unix::ffi::OsStringExt;
        let lookup = |name: &str| (name == "K2_X").then(|| OsString::from_vec(vec![0x66, 0xff]));
        let err = parse_in::<String>(NAMES, lookup).expect_err("not UTF-8");
        assert_eq!((err.name, err.why.as_str()), ("K2_X", "not UTF-8"));
    }

    #[test]
    fn the_command_line_reader_skips_an_empty_name() {
        assert_eq!(first_nonempty_in(NAMES, env(&[])), None);
        assert_eq!(first_nonempty_in(NAMES, env(&[("K5_X", ""), ("K2_X", ""), ("K2SO_X", "")])), None);
        assert_eq!(first_nonempty_in(NAMES, env(&[("K5_X", ""), ("K2SO_X", "c")])), Some(("K2SO_X", "c".to_string())));
        assert_eq!(first_nonempty_in(NAMES, env(&[("K5_X", ""), ("K2_X", "b"), ("K2SO_X", "c")])), Some(("K2_X", "b".to_string())));
        assert_eq!(first_nonempty_in(NAMES, env(&[("K5_X", "a"), ("K2_X", "b")])), Some(("K5_X", "a".to_string())));
        assert_eq!(first_nonempty_in(NAMES, env(&[("K2SO_X", " ")])), Some(("K2SO_X", " ".to_string())), "a space is not empty");
        assert_eq!(first_nonempty_os_in(NAMES, env_os(&[("K5_X", ""), ("K2_X", "/p")])), Some(("K2_X", OsString::from("/p"))));
        assert_eq!(first_nonempty_os_in(NAMES, env_os(&[("K5_X", "")])), None);
        // The daemon's reader keeps the empty K5_ name; this one does not.
        let both = env(&[("K5_X", ""), ("K2_X", "b")]);
        assert_eq!(first_set_in(NAMES, &both), Some(("K5_X", String::new())));
        assert_eq!(first_nonempty_in(NAMES, &both), Some(("K2_X", "b".to_string())));
    }

    #[cfg(unix)]
    #[test]
    fn the_command_line_reader_takes_any_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let lookup = |name: &str| (name == "K2_X").then(|| OsString::from_vec(vec![0x66, 0xff]));
        assert_eq!(first_nonempty_os_in(NAMES, lookup), Some(("K2_X", OsString::from_vec(vec![0x66, 0xff]))));
    }

    /// The command line's section is in [`ALL`], so the naming rule below
    /// holds it too.
    #[test]
    fn the_command_line_section_is_in_the_registry() {
        assert!(ALL.ends_with(CLI), "ALL ends with CLI, in CLI's order");
        for names in CLI {
            assert_eq!(names.len(), PREFIXES.len(), "{names:?}: the shell tries all of {PREFIXES:?}");
        }
    }

    /// Every entry: a `K5_` name first, then older names with the same
    /// rest, in prefix order (`K2_` before `K2SO_`); no name in two entries.
    #[test]
    fn every_registry_entry_is_newest_first_with_one_suffix() {
        let mut seen = std::collections::HashSet::new();
        for names in ALL {
            assert!(names.len() >= 2, "{names:?}: one name needs no registry entry");
            let rest = names[0].strip_prefix("K5_").unwrap_or_else(|| panic!("{names:?}: the K5_ name comes first"));
            let mut last = 0;
            for name in names.iter() {
                let at = PREFIXES
                    .iter()
                    .position(|p| name.strip_prefix(p) == Some(rest))
                    .unwrap_or_else(|| panic!("{name}: not one of {PREFIXES:?} + {rest}"));
                assert!(at >= last, "{names:?}: newest first");
                last = at;
                assert!(seen.insert(*name), "{name} is in two entries");
            }
        }
    }
}
