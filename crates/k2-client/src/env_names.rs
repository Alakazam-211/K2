//! Settings read from the environment under more than one name: the one
//! registry of those names, and the one reader.
//!
//! A setting is read under its `K5_X` name first, then `K2_X`, then
//! `K2SO_X` (each only where that name exists), so an operator's old line
//! keeps working. Every setting with more than one name is listed here,
//! newest name first, and read with [`first_set`] (or [`first_set_os`] for
//! a path, [`parse`] for a value that must parse).
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
