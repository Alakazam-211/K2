//! Which Codex a K2 tab runs, and whether K2 has checked that version's
//! conversation ids.
//!
//! The daemon's per-tab passport check (`k2-daemon`'s
//! `session_token::conversation`) compares the conversation a Codex tool
//! call names (`X-K2-Conversation`, which the `k2` CLI fills from
//! `CODEX_SESSION_ID`) with the conversation K2 knows the tab is on (the
//! `codex resume <id>` argv or the rollout log K2 adopted). That only holds
//! when the Codex version sets those ids the way K2 read them in Codex's
//! source, so the check refuses a Codex version that is not in
//! [`SUPPORTED`].
//!
//! The spawner records each Codex tab's program and PATH
//! ([`note_session`]); the check asks [`session_version`], which runs
//! `codex --version` once per installed binary (cached like the
//! `--no-daemon` probe) and never at spawn.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::codex_no_daemon::{fingerprint, probe, Fingerprint};

/// What K2 read in Codex's source for a range of releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexIdFacts {
    /// First release the facts hold for (inclusive).
    pub first: (u32, u32, u32),
    /// Last release K2 checked (inclusive). A later one is unknown.
    pub last: (u32, u32, u32),
    /// The env var that holds the conversation's ROOT thread id on every
    /// tool call (a subagent's call names its parent conversation).
    pub root_env: &'static str,
    /// The env var that holds the id of the thread that made the call.
    pub thread_env: &'static str,
    /// A top-level conversation's root id equals its own thread id.
    pub top_level_root_is_thread: bool,
    /// The thread id is the rollout log's `<uuid>` and its
    /// `session_meta.payload.id` (what K2's adoption reads).
    pub thread_is_rollout_id: bool,
    /// `codex resume <id>` takes that same id.
    pub resume_takes_thread_id: bool,
}

/// The Codex releases K2's passport check accepts. One entry per range of
/// releases whose ids behave the same.
///
/// To add a release (`.k2/agent/work/codex-per-tab-passport-plan.md`,
/// "The thread-id question SETTLED"), read its source and confirm:
/// 1. tool calls get `CODEX_THREAD_ID` (the thread) and `CODEX_SESSION_ID`
///    (the root thread) in their env (unified_exec's process manager and
///    user_shell);
/// 2. a top-level conversation's root equals its thread, and a subagent's
///    root is its parent conversation's thread;
/// 3. the thread id is the rollout file's `<uuid>`, its header
///    `session_meta.payload.id`, and the id `codex resume <id>` takes;
/// 4. `/new`, `/clear` and fork start a NEW thread id (a new rollout log).
///
/// Then extend `last` (or add an entry) and the version pin test.
pub const SUPPORTED: &[CodexIdFacts] = &[CodexIdFacts {
    first: (0, 155, 0),
    last: (0, 171, 0),
    root_env: "CODEX_SESSION_ID",
    thread_env: "CODEX_THREAD_ID",
    top_level_root_is_thread: true,
    thread_is_rollout_id: true,
    resume_takes_thread_id: true,
}];

/// The `x.y.z` in a `codex --version` answer (`codex-cli 0.163.0`).
/// A pre-release or build suffix (`0.163.0-alpha.1`) is not a release
/// K2 checked: `None`.
pub fn parse_version(out: &str) -> Option<(u32, u32, u32)> {
    let word = out.split_whitespace().last()?;
    let word = word.strip_prefix('v').unwrap_or(word);
    let mut parts = word.split('.');
    let mut next = || -> Option<u32> {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

/// The facts for a version K2 checked, or `None` (unknown: refuse).
pub fn facts_for(version: (u32, u32, u32)) -> Option<&'static CodexIdFacts> {
    SUPPORTED.iter().find(|f| f.first <= version && version <= f.last)
}

/// How long a version answer is trusted before asking the binary again.
const VERSION_TTL: Duration = Duration::from_secs(60 * 60);

struct Cached {
    fp: Fingerprint,
    answer: Result<String, String>,
    at: Instant,
}

fn version_cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static C: std::sync::OnceLock<Mutex<HashMap<PathBuf, Cached>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `codex --version`'s answer (trimmed) for the binary `program` resolves
/// to on `search_path`. Cached per binary, like the `--no-daemon` probe.
pub fn binary_version(program: &str, search_path: &str) -> Result<String, String> {
    let found = crate::terminal::agent_spawn_guard::locate_on_path(program, search_path)
        .ok_or_else(|| format!("{program} not found on the tab's PATH"))?;
    let key = dunce::canonicalize(&found).unwrap_or_else(|_| found.clone());
    let fp = fingerprint(&key).ok_or_else(|| format!("cannot stat {}", key.display()))?;
    {
        let g = version_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.get(&key) {
            if c.fp == fp && c.at.elapsed() < VERSION_TTL {
                return c.answer.clone();
            }
        }
    }
    let answer = probe(&found, "--version", search_path).map(|s| s.trim().to_string());
    version_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(
        key,
        Cached { fp, answer: answer.clone(), at: Instant::now() },
    );
    answer
}

/// Most tab launches remembered; the oldest is forgotten first (a
/// forgotten tab's version is read with the daemon's PATH instead).
const LAUNCHES_CAP: usize = 4096;

#[derive(Default)]
struct Launches {
    by_session: HashMap<String, (String, String)>,
    order: VecDeque<String>,
}

fn launches() -> &'static Mutex<Launches> {
    static L: std::sync::OnceLock<Mutex<Launches>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(Launches::default()))
}

/// The spawner records a Codex tab's program and the PATH it was started
/// with, so the version is read from the same binary later. Nothing runs.
pub fn note_session(session_id: &str, program: &str, search_path: &str) {
    let mut g = launches().lock().unwrap_or_else(|e| e.into_inner());
    let entry = (program.to_string(), search_path.to_string());
    if g.by_session.insert(session_id.to_string(), entry).is_none() {
        g.order.push_back(session_id.to_string());
    }
    while g.order.len() > LAUNCHES_CAP {
        if let Some(old) = g.order.pop_front() {
            g.by_session.remove(&old);
        }
    }
}

/// The Codex version a tab runs: its recorded launch, else `program` on
/// the daemon's own PATH (a tab started before a daemon restart).
pub fn session_version(session_id: &str, program: &str) -> Result<String, String> {
    let recorded = launches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .by_session
        .get(session_id)
        .cloned();
    match recorded {
        Some((prog, path)) => binary_version(&prog, &path),
        None => binary_version(program, &std::env::var("PATH").unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_answers_parse_only_as_plain_releases() {
        assert_eq!(parse_version("codex-cli 0.163.0\n"), Some((0, 163, 0)));
        assert_eq!(parse_version("codex-cli v0.171.0"), Some((0, 171, 0)));
        assert_eq!(parse_version("0.155.0"), Some((0, 155, 0)));
        assert_eq!(parse_version("codex-cli 0.163.0-alpha.1"), None);
        assert_eq!(parse_version("codex-cli 0.163"), None);
        assert_eq!(parse_version("codex-cli 0.163.0.1"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("error: unknown flag"), None);
    }

    #[test]
    fn only_checked_releases_have_facts() {
        for v in [(0, 155, 0), (0, 160, 3), (0, 171, 0)] {
            assert!(facts_for(v).is_some(), "{v:?} is in the checked range");
        }
        for v in [(0, 154, 9), (0, 171, 1), (0, 172, 0), (1, 0, 0)] {
            assert!(facts_for(v).is_none(), "{v:?} was never checked");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_recorded_launch_is_asked_once_per_binary() {
        let dir = std::env::temp_dir().join(format!(
            "k2-codex-ver-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let bin = dir.join("codex");
        crate::test_env::write_executable(
            &bin,
            "#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/calls.log\"\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 0.163.0'; exit 0; fi\nexit 2\n",
        );
        let sid = format!("ver-{}", std::process::id());
        note_session(&sid, &bin.to_string_lossy(), "/usr/bin:/bin");
        assert_eq!(session_version(&sid, "codex").as_deref(), Ok("codex-cli 0.163.0"));
        assert_eq!(session_version(&sid, "codex").as_deref(), Ok("codex-cli 0.163.0"));
        let calls = std::fs::read_to_string(dir.join("calls.log")).expect("calls.log");
        assert_eq!(calls, "--version\n", "one probe, and only --version");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
