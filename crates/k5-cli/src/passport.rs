//! The passport rule (PRD 6.3): one rule the front applies before any
//! route. In this slice it only counts; it never refuses.
//!
//! In order, first that applies:
//!
//! 0. An offline verb ([`OFFLINE_FIRST`], or `-h`/`--help` anywhere)
//!    never reaches the daemon: it runs and is never counted.
//! 1. A passport is set (`HOOK_TOKEN` under its K5_, K2_ and K2SO_ names,
//!    empty counts as unset): the call uses it.
//! 2. No passport and `NO_OWNER_FALLBACK=1`: the shell CLI refuses it
//!    today (the virtual-desktop guard); the front leaves that to the
//!    shell.
//! 3. No passport, inside an agent session ([`SESSION_MARKERS`], or a
//!    session process among the ancestors, [`SESSION_PROCESSES`]): the
//!    call would be refused. For now the front only says so: the shell
//!    gets `K5_CLI_WOULD_REFUSE=1` and its daemon requests carry
//!    `X-K5-Would-Refuse: 1`. Nothing else changes.
//! 4. No passport, outside any session: the owner path, as today.
//!
//! "Inside an agent session" by the process tree: an ancestor (at most 64
//! hops up, by k2-client's one walk) is the daemon, the pty daemon or the
//! App engine. A `k2 job` run is the daemon's own child (its runner lives
//! in the daemon, `crates/k2-daemon/src/jobs/runner.rs`), so the daemon
//! covers it. A process is named by the kernel's command name
//! ([`k2_client::proc_ancestry::LiveProcessTable::command_name`]). On
//! Windows the table answers nothing, so the tree's answer is
//! [`Ancestry::Unknown`] there and only the markers count.

use std::ffi::{OsStr, OsString};

use k2_client::env_names;
use k2_client::proc_ancestry::LiveProcessTable;
use k2_client::proc_tree::{ancestry, ProcessTable};

/// A first argument that never reaches the daemon. The shell CLI's list
/// for the virtual-desktop guard (`_k2_offline_verb` in
/// `cli/src/cli-tools-policy.sh`); a test holds the two the same. The
/// empty string is a bare `k5`.
pub const OFFLINE_FIRST: &[&str] = &["", "study", "help", "--help", "-h", "--schema", "version", "--version"];

/// An argument that makes any command offline, wherever it is.
pub const OFFLINE_ANYWHERE: &[&str] = &["-h", "--help"];

/// The settings that say a harness or a K2 session started this process.
/// Set and not empty, any one of them is enough. `PANE_ID` is what the
/// shell CLI's `hooks emit` takes for a session (`cli/src/remind.sh`),
/// `CELL` is set in every cell, `CLAUDECODE` by Claude Code and
/// `CODEX_THREAD_ID` by Codex on its tool runs (the shell's stale-passport
/// check in `cli/src/cli-tools-policy.sh` names it).
pub const SESSION_MARKERS: &[&[&str]] = &[env_names::PANE_ID, env_names::CELL, &["CLAUDECODE"], &["CODEX_THREAD_ID"]];

/// A session process among the ancestors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProcess {
    /// The daemon, `k2-daemon` (`DAEMON_BINARY_NAME`). A `k2 job` run is
    /// its child.
    Daemon,
    /// The pty daemon, `k2-ptyd`: every terminal and agent tab.
    Ptyd,
    /// The App engine, `k2-app-engine`.
    AppEngine,
}

/// The session processes by command name: each `[[bin]]` name in its
/// crate. All are shorter than the 15 bytes Linux keeps.
pub const SESSION_PROCESSES: &[(&str, SessionProcess)] = &[
    (k2_client::daemon_lifecycle::DAEMON_BINARY_NAME, SessionProcess::Daemon),
    ("k2-ptyd", SessionProcess::Ptyd),
    ("k2-app-engine", SessionProcess::AppEngine),
];

/// The most parents the walk looks at, this process included.
pub const MAX_HOPS: usize = 64;

/// What the process tree says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ancestry {
    /// A session process is an ancestor.
    Found(SessionProcess),
    /// None is.
    NotFound,
    /// The OS can't say (Windows, for now).
    Unknown,
}

/// The rule's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 0: an offline verb.
    Offline,
    /// 1: a passport is set.
    Passport,
    /// 2: no passport and no owner fallback; the shell refuses it.
    NoOwnerFallback,
    /// 3: no passport inside a session. Counted, not refused.
    WouldRefuse,
    /// 4: the owner path.
    Owner,
}

impl Verdict {
    /// True only when the call is counted.
    pub fn would_refuse(self) -> bool {
        self == Verdict::WouldRefuse
    }
}

/// The facts the rule reads, but the process tree, which is looked at
/// only when nothing else decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub offline: bool,
    pub passport: bool,
    pub no_owner_fallback: bool,
    /// The first marker found, by the name it was set under.
    pub marker: Option<&'static str>,
}

/// The rule. `tree` runs only when no other fact decides.
pub fn decide(facts: Facts, tree: impl FnOnce() -> Ancestry) -> Verdict {
    if facts.offline {
        return Verdict::Offline;
    }
    if facts.passport {
        return Verdict::Passport;
    }
    if facts.no_owner_fallback {
        return Verdict::NoOwnerFallback;
    }
    if facts.marker.is_some() {
        return Verdict::WouldRefuse;
    }
    match tree() {
        Ancestry::Found(_) => Verdict::WouldRefuse,
        Ancestry::NotFound | Ancestry::Unknown => Verdict::Owner,
    }
}

/// True when the arguments name an offline verb.
pub fn is_offline(args: &[OsString]) -> bool {
    let first = args.first().map(OsString::as_os_str).unwrap_or(OsStr::new(""));
    OFFLINE_FIRST.iter().any(|v| first == *v) || args.iter().any(|a| OFFLINE_ANYWHERE.iter().any(|v| a == *v))
}

/// The facts, from the arguments and an environment lookup.
pub fn facts(args: &[OsString], lookup: impl Fn(&str) -> Option<OsString>) -> Facts {
    let passport = env_names::first_nonempty_os_in(env_names::HOOK_TOKEN, &lookup).is_some();
    let no_owner_fallback =
        env_names::first_nonempty_os_in(env_names::NO_OWNER_FALLBACK, &lookup).is_some_and(|(_, v)| v == "1");
    let marker = SESSION_MARKERS.iter().find_map(|names| env_names::first_nonempty_os_in(names, &lookup).map(|(n, _)| n));
    Facts { offline: is_offline(args), passport, no_owner_fallback, marker }
}

/// The session process among `start`'s ancestors (itself included), by
/// `table`'s walk and `name`'s answer for each pid. [`Ancestry::Unknown`]
/// when the table doesn't know `start`.
pub fn session_ancestor(table: &dyn ProcessTable, name: impl Fn(i32) -> Option<String>, start: i32) -> Ancestry {
    let Some(me) = table.info(start) else {
        return Ancestry::Unknown;
    };
    for p in ancestry(table, me, MAX_HOPS) {
        if let Some(n) = name(p.pid) {
            if let Some((_, kind)) = SESSION_PROCESSES.iter().find(|(want, _)| *want == n) {
                return Ancestry::Found(*kind);
            }
        }
    }
    Ancestry::NotFound
}

/// [`session_ancestor`] of this process, over the live table. Timed as
/// its own phase ([`crate::trace`]).
pub fn live_session_ancestor() -> Ancestry {
    crate::trace::mark("passport facts");
    let table = LiveProcessTable;
    let Ok(me) = i32::try_from(std::process::id()) else {
        return Ancestry::Unknown;
    };
    let found = session_ancestor(&table, |pid| table.command_name(pid), me);
    crate::trace::mark("passport ancestry walk");
    found
}

/// The verdict for this process and its arguments.
pub fn current(args: &[OsString]) -> Verdict {
    decide(facts(args, |name| std::env::var_os(name)), live_session_ancestor)
}

/// What the shell CLI gets for `verdict`: [`WOULD_REFUSE_VAR`]`=1` when the
/// call is counted, and the name taken out of the environment otherwise,
/// so a value from the caller never stands.
///
/// [`WOULD_REFUSE_VAR`]: crate::called_as::WOULD_REFUSE_VAR
pub fn hand_off_env(verdict: Verdict) -> (&'static str, Option<&'static str>) {
    (crate::called_as::WOULD_REFUSE_VAR, verdict.would_refuse().then_some("1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use k2_client::proc_tree::ProcInfo;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn facts(offline: bool, passport: bool, no_owner_fallback: bool, marker: Option<&'static str>) -> Facts {
        Facts { offline, passport, no_owner_fallback, marker }
    }

    /// Every combination: (offline, passport, no-owner-fallback, marker,
    /// tree) and the verdict; the tree is read only when it decides.
    #[test]
    fn the_rule_over_every_combination() {
        use Ancestry::*;
        use Verdict::*;
        let trees = [Found(SessionProcess::Daemon), Found(SessionProcess::Ptyd), Found(SessionProcess::AppEngine), NotFound, Unknown];
        let mut rows = 0;
        for offline in [false, true] {
            for passport in [false, true] {
                for nofb in [false, true] {
                    for marker in [None, Some("CLAUDECODE"), Some("K2_PANE_ID")] {
                        for tree in trees {
                            let read = std::cell::Cell::new(false);
                            let got = decide(facts(offline, passport, nofb, marker), || {
                                read.set(true);
                                tree
                            });
                            let want = if offline {
                                Offline
                            } else if passport {
                                Passport
                            } else if nofb {
                                NoOwnerFallback
                            } else if marker.is_some() || matches!(tree, Found(_)) {
                                WouldRefuse
                            } else {
                                Owner
                            };
                            let row = format!("offline {offline}, passport {passport}, nofb {nofb}, marker {marker:?}, tree {tree:?}");
                            assert_eq!(got, want, "{row}");
                            let tree_decides = !offline && !passport && !nofb && marker.is_none();
                            assert_eq!(read.get(), tree_decides, "tree read: {row}");
                            assert_eq!(got.would_refuse(), want == WouldRefuse, "{row}");
                            rows += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(rows, 2 * 2 * 2 * 3 * 5);
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn the_facts_from_the_environment() {
        let none = super::facts(&args(&["msg"]), env(&[]));
        assert_eq!(none, facts(false, false, false, None));
        // A passport under any of its three names; empty is unset.
        for name in env_names::HOOK_TOKEN.iter().copied() {
            assert!(super::facts(&args(&["msg"]), env(&[(name, "t")])).passport, "{name}");
            assert!(!super::facts(&args(&["msg"]), env(&[(name, "")])).passport, "{name} empty");
        }
        // No owner fallback is exactly 1, the first non-empty name wins.
        for (pairs, want) in [
            (&[("K2_NO_OWNER_FALLBACK", "1")][..], true),
            (&[("K5_NO_OWNER_FALLBACK", "1")][..], true),
            (&[("K2SO_NO_OWNER_FALLBACK", "1")][..], true),
            (&[("K2_NO_OWNER_FALLBACK", "yes")][..], false),
            (&[("K2_NO_OWNER_FALLBACK", "")][..], false),
            (&[("K5_NO_OWNER_FALLBACK", "0"), ("K2_NO_OWNER_FALLBACK", "1")][..], false),
            (&[("K5_NO_OWNER_FALLBACK", ""), ("K2_NO_OWNER_FALLBACK", "1")][..], true),
        ] {
            assert_eq!(super::facts(&args(&["msg"]), env(pairs)).no_owner_fallback, want, "{pairs:?}");
        }
        // Each marker, by the name it was set under; empty is unset.
        for names in SESSION_MARKERS {
            for name in names.iter().copied() {
                assert_eq!(super::facts(&args(&["msg"]), env(&[(name, "x")])).marker, Some(name));
                assert_eq!(super::facts(&args(&["msg"]), env(&[(name, "")])).marker, None, "{name} empty");
            }
        }
        // Not markers: Codex's home, a session id, a hook socket alone.
        for name in ["CODEX_HOME", "CODEX_SANDBOX", "K2_SESSION_ID", "K2_HOOK_SOCK", "CLAUDE_CODE_ENTRYPOINT"] {
            assert_eq!(super::facts(&args(&["msg"]), env(&[(name, "x")])).marker, None, "{name}");
        }
    }

    #[test]
    fn offline_verbs() {
        let offline: &[&[&str]] = &[
            &[],
            &[""],
            &["help"],
            &["--help"],
            &["-h"],
            &["study"],
            &["study", "x"],
            &["--schema"],
            &["version"],
            &["--version"],
            &["msg", "--help"],
            &["msg", "a", "-h"],
            // As the shell's loop: `--` doesn't stop it.
            &["msg", "--", "-h"],
        ];
        for row in offline {
            assert!(is_offline(&args(row)), "{row:?}");
        }
        let online: &[&[&str]] =
            &[&["msg"], &["schema"], &["explain"], &["glossary"], &["-v"], &["msg", "--helpx"], &["msg", "help"], &["Help"], &[" help"]];
        for row in online {
            assert!(!is_offline(&args(row)), "{row:?}");
        }
    }

    /// A process table made up for the test.
    struct Fake(HashMap<i32, (i32, &'static str)>);

    impl ProcessTable for Fake {
        fn info(&self, pid: i32) -> Option<ProcInfo> {
            self.0.get(&pid).map(|(ppid, _)| ProcInfo { pid, ppid: *ppid, start_time: 0 })
        }
    }

    impl Fake {
        fn name(&self, pid: i32) -> Option<String> {
            self.0.get(&pid).map(|(_, n)| n.to_string())
        }
    }

    fn chain(names: &[&'static str]) -> Fake {
        // pid 100 is names[0]; each next name is its parent; the last is
        // a child of init.
        let mut map = HashMap::new();
        for (i, n) in names.iter().enumerate() {
            let pid = 100 + i as i32;
            let ppid = if i + 1 == names.len() { 1 } else { pid + 1 };
            map.insert(pid, (ppid, *n));
        }
        Fake(map)
    }

    #[test]
    fn the_name_matcher_over_a_made_up_tree() {
        let rows: &[(&[&'static str], Ancestry)] = &[
            (&["k5-cli", "zsh", "k2-ptyd", "launchd-ish"], Ancestry::Found(SessionProcess::Ptyd)),
            (&["k5-cli", "bash", "claude", "zsh", "k2-ptyd", "k2-daemon"], Ancestry::Found(SessionProcess::Ptyd)),
            (&["k5-cli", "sh", "k2-daemon"], Ancestry::Found(SessionProcess::Daemon)),
            (&["k5-cli", "node", "k2-app-engine"], Ancestry::Found(SessionProcess::AppEngine)),
            (&["k5-cli", "zsh", "Terminal"], Ancestry::NotFound),
            (&["k5-cli", "zsh", "k2-ptyd-test-agent", "k2-daemonx", "K2-PTYD", "k2"], Ancestry::NotFound),
            (&["k2-ptyd"], Ancestry::Found(SessionProcess::Ptyd)),
        ];
        for (names, want) in rows {
            let t = chain(names);
            assert_eq!(session_ancestor(&t, |p| t.name(p), 100), *want, "{names:?}");
        }
        // A pid the table doesn't know: the tree can't say.
        let t = chain(&["k5-cli"]);
        assert_eq!(session_ancestor(&t, |p| t.name(p), 7), Ancestry::Unknown);
        // A session process more than MAX_HOPS up is not seen.
        let mut long: Vec<&'static str> = vec!["sh"; MAX_HOPS];
        long.push("k2-ptyd");
        let t = chain(&long);
        assert_eq!(session_ancestor(&t, |p| t.name(p), 100), Ancestry::NotFound);
        long.remove(0);
        let t = chain(&long);
        assert_eq!(session_ancestor(&t, |p| t.name(p), 100), Ancestry::Found(SessionProcess::Ptyd));
    }

    #[test]
    fn the_names_are_the_binaries() {
        assert_eq!(k2_client::daemon_lifecycle::DAEMON_BINARY_NAME, "k2-daemon");
        for (name, _) in SESSION_PROCESSES {
            assert!(name.len() <= 15, "{name} is longer than Linux's command name");
        }
    }

    #[test]
    fn the_hand_off_is_one_name_set_or_taken_out() {
        assert_eq!(hand_off_env(Verdict::WouldRefuse), ("K5_CLI_WOULD_REFUSE", Some("1")));
        for v in [Verdict::Offline, Verdict::Passport, Verdict::NoOwnerFallback, Verdict::Owner] {
            assert_eq!(hand_off_env(v), ("K5_CLI_WOULD_REFUSE", None), "{v:?}");
        }
    }
}
