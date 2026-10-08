//! Test-time guard for the agent program the daemon is about to spawn.
//!
//! Background (2026-09-12): `cargo test -p k2-daemon` runs in-process daemon
//! harnesses and real `k2-daemon` binaries under a temporary `HOME`
//! (`$TMPDIR/k2-skin-it-*`, `k2-psg-*`, …). Tests install a fake `claude`
//! shim (`#!/bin/sh\nexec cat`) by PREPENDING its dir to the inherited PATH,
//! but [`super::login_path::merge_path`] puts the user's LOGIN-SHELL PATH
//! first, so the real `~/.local/bin/claude` won every lookup. Real Claude
//! Code then started under a fresh HOME → first-run login → it opened the
//! user's browser at `claude.ai/oauth/authorize`. Three times in one
//! evening; it looked like a credential attack.
//!
//! This module is the single choke point every PTY spawn passes through
//! ([`super::daemon_pty::DaemonPtySession::spawn`] — v2 spawn, heartbeat
//! wake, pinned chat, host sessions, remote sessions, skin Thread wakes all
//! funnel into it). Two independent layers:
//!
//! 1. **Shim dir** — when [`SHIM_DIR_ENV`] (`K2_TEST_AGENT_SHIM_DIR`, a
//!    host-separator path list) is set, the program's basename is resolved
//!    ONLY inside those directories, to an absolute path. The login-shell /
//!    known / inherited PATH is never consulted. A missing shim is a loud
//!    spawn error, never a fallback.
//! 2. **Temp-HOME belt** — when the process `HOME` sits under the OS temp
//!    dir (`std::env::temp_dir()`, `$TMPDIR`, `/tmp`) and
//!    [`ALLOW_REAL_ENV`] (`K2_TEST_ALLOW_REAL_AGENT`) is not `1`, any program
//!    that would resolve OUTSIDE the shim dirs is refused. The only
//!    carve-out is the OS-owned system bin dirs (`/bin`, `/usr/bin`,
//!    `/sbin`, `/usr/sbin` — SIP-sealed on macOS; no agent CLI is ever
//!    installed there) so harnesses may keep spawning `cat` / `sh`.
//!
//! 3. **Test-build belt** (0.45.1, quiet-gate PRD §4.12) — in a test build
//!    (`cfg(test)` or k2-core's `test-util` feature) a known agent CLI
//!    ([`AGENT_CLIS`]: `claude`, `codex`, …) is NEVER resolved through PATH,
//!    whatever `HOME` is: without a shim dir the spawn is refused with
//!    "refusing: test tried to spawn real `claude`; install the agent shim".
//!    Two daemon tests used to start the real Claude Code on a dev Mac
//!    because their HOME was not under the temp dir. Tests install shims with
//!    `k2_core::test_env::AgentShim`. The one carve-out is an explicit
//!    path to a stub the test wrote under the OS temp dir (a path, never a
//!    bare name, so PATH is never consulted).
//!
//! Production (no env, real HOME): [`resolve_program`] returns the input
//! string untouched without touching the filesystem — byte-identical to the
//! pre-guard behaviour.

use std::path::{Path, PathBuf};

/// Host-separator list of directories that hold the ONLY acceptable agent
/// binaries while tests run. Set by test harnesses, never by production.
pub const SHIM_DIR_ENV: &str = "K2_TEST_AGENT_SHIM_DIR";

/// `1` disables the temp-HOME belt (explicit opt-in to run a real agent
/// under a temp HOME — e.g. a manual end-to-end check).
pub const ALLOW_REAL_ENV: &str = "K2_TEST_ALLOW_REAL_AGENT";

/// Agent CLI basenames (the built-in presets plus aliases) that a test
/// build must never resolve outside a shim dir.
pub const AGENT_CLIS: &[&str] = &[
    "claude",
    "codex",
    "grok",
    "gemini",
    "cursor-agent",
    "cursor",
    "pi",
    "hermes",
    "opencode",
    "goose",
    "aider",
    "ollama",
    "copilot",
    "interpreter",
];

/// True when `program`'s basename is one of [`AGENT_CLIS`].
pub fn is_agent_cli(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| AGENT_CLIS.contains(&n))
        .unwrap_or(false)
}

/// The environment facts the guard decides on. Built from the live process
/// by [`GuardEnv::from_process`]; unit tests construct it directly.
#[derive(Debug, Clone)]
pub struct GuardEnv {
    /// `None` when [`SHIM_DIR_ENV`] is unset or empty.
    pub shim_dirs: Option<Vec<PathBuf>>,
    /// The process `HOME` (`None` when unset).
    pub home: Option<PathBuf>,
    /// Candidate OS temp roots (`std::env::temp_dir()`, `$TMPDIR`, `/tmp`).
    pub temp_dirs: Vec<PathBuf>,
    /// [`ALLOW_REAL_ENV`] == `1`.
    pub allow_real: bool,
    /// This is a test build (`cfg(test)` / `test-util`): agent CLIs need a
    /// shim dir. [`GuardEnv::from_process`] sets it; tests of the
    /// production rules construct `GuardEnv` with `false`.
    pub test_build: bool,
}

impl GuardEnv {
    /// Snapshot the live process environment.
    pub fn from_process() -> Self {
        let shim_dirs = std::env::var_os(SHIM_DIR_ENV).and_then(|raw| {
            let dirs: Vec<PathBuf> = std::env::split_paths(&raw)
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
            if dirs.is_empty() {
                None
            } else {
                Some(dirs)
            }
        });
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut temp_dirs = vec![std::env::temp_dir()];
        if let Some(t) = std::env::var_os("TMPDIR") {
            if !t.is_empty() {
                temp_dirs.push(PathBuf::from(t));
            }
        }
        #[cfg(unix)]
        temp_dirs.push(PathBuf::from("/tmp"));
        let allow_real = std::env::var(ALLOW_REAL_ENV)
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        Self {
            shim_dirs,
            home,
            temp_dirs,
            allow_real,
            test_build: cfg!(any(test, feature = "test-util")),
        }
    }

    /// True when `HOME` lives under one of the temp roots (raw or
    /// canonicalized prefix match — macOS `/var` → `/private/var`).
    pub fn home_is_temp(&self) -> bool {
        let Some(home) = self.home.as_deref() else {
            return false;
        };
        if home.as_os_str().is_empty() {
            return false;
        }
        let home_forms = path_forms(home);
        self.temp_dirs.iter().any(|t| {
            if t.as_os_str().is_empty() {
                return false;
            }
            let temp_forms = path_forms(t);
            home_forms
                .iter()
                .any(|h| temp_forms.iter().any(|tf| h.starts_with(tf)))
        })
    }
}

impl GuardEnv {
    /// `program` is an explicit absolute path under one of the temp roots
    /// (a stub a test wrote), not a bare name looked up on PATH.
    pub fn is_temp_stub(&self, program: &str) -> bool {
        let p = Path::new(program);
        if !p.is_absolute() {
            return false;
        }
        let forms = path_forms(p);
        self.temp_dirs.iter().filter(|t| !t.as_os_str().is_empty()).any(|t| {
            let temp_forms = path_forms(t);
            forms.iter().any(|f| temp_forms.iter().any(|tf| f.starts_with(tf)))
        })
    }
}

/// Raw + canonical spellings of a path (canonical only when it resolves).
fn path_forms(p: &Path) -> Vec<PathBuf> {
    let mut v = vec![p.to_path_buf()];
    if let Ok(c) = p.canonicalize() {
        if c != p {
            v.push(c);
        }
    }
    v
}

/// Decide what program the PTY layer may exec for `program`.
///
/// * Shim mode ([`GuardEnv::shim_dirs`] set): `Ok(<shim_dir>/<basename>)`
///   for the first shim dir holding a regular file of that name, else
///   `Err` (loud; never PATH).
/// * Temp-HOME belt (no shim dirs, `HOME` under temp, not `allow_real`):
///   locate `program` on `search_path` (the enriched child PATH); refuse
///   unless it lives in an OS-owned system bin dir.
/// * Test-build belt ([`GuardEnv::test_build`], no shim dirs, not
///   `allow_real`): an [`AGENT_CLIS`] name is refused, whatever `HOME` is.
/// * Otherwise: `Ok(program)` unchanged — no I/O.
pub fn resolve_program(program: &str, search_path: &str, env: &GuardEnv) -> Result<String, String> {
    if let Some(dirs) = env.shim_dirs.as_deref() {
        return resolve_in_shim_dirs(program, dirs);
    }
    if env.test_build && !env.allow_real && is_agent_cli(program) && !env.is_temp_stub(program) {
        return Err(format!(
            "[spawn] refusing: test tried to spawn real `{program}`; install the agent shim \
             (k2_core::test_env::AgentShim, or set {SHIM_DIR_ENV})"
        ));
    }
    if env.allow_real || !env.home_is_temp() {
        return Ok(program.to_string());
    }
    let resolved = locate_on_path(program, search_path);
    match resolved {
        Some(p) if is_os_system_bin(&p) => Ok(program.to_string()),
        Some(p) => Err(refusal(&p.display().to_string())),
        None => Err(refusal(program)),
    }
}

fn refusal(what: &str) -> String {
    format!(
        "[spawn] refusing real agent {what} under temp HOME (set {ALLOW_REAL_ENV}=1 to override)"
    )
}

fn resolve_in_shim_dirs(program: &str, dirs: &[PathBuf]) -> Result<String, String> {
    let joined = std::env::join_paths(dirs)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{dirs:?}"));
    let Some(name) = Path::new(program).file_name() else {
        return Err(format!(
            "[spawn] agent program {program:?} has no basename; refusing under {SHIM_DIR_ENV}={joined}"
        ));
    };
    for dir in dirs {
        let cand = dir.join(name);
        if cand.is_file() {
            return Ok(cand.to_string_lossy().into_owned());
        }
    }
    Err(format!(
        "[spawn] agent shim {:?} (for program {program:?}) not found in {SHIM_DIR_ENV}={joined}; refusing to resolve via PATH",
        name.to_string_lossy()
    ))
}

/// Where `program` would exec from: itself when it carries a directory
/// component, else the first `search_path` dir holding a regular file of
/// that name (Windows also tries the common PATHEXT suffixes).
pub fn locate_on_path(program: &str, search_path: &str) -> Option<PathBuf> {
    let raw = Path::new(program);
    if raw.is_absolute() || raw.components().count() > 1 {
        return Some(raw.to_path_buf());
    }
    for dir in std::env::split_paths(search_path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let exact = dir.join(program);
        if exact.is_file() {
            return Some(exact);
        }
        #[cfg(windows)]
        {
            for ext in [".exe", ".cmd", ".bat", ".com"] {
                let cand = dir.join(format!("{program}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// OS-owned system bin dirs — the ONLY belt carve-out. Agent CLIs
/// (`claude`, `cursor`, `gemini`, `grok`, `codex`, …) install to
/// `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, npm/nvm dirs —
/// never here (SIP-sealed on macOS, package-owned on Linux).
fn is_os_system_bin(p: &Path) -> bool {
    #[cfg(unix)]
    {
        const SYSTEM_BIN: [&str; 4] = ["/bin", "/usr/bin", "/sbin", "/usr/sbin"];
        let Some(parent) = p.parent() else {
            return false;
        };
        SYSTEM_BIN.iter().any(|d| Path::new(d) == parent)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let d = crate::test_env::unique_temp_path(&format!("agent-guard-{label}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_exec(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        crate::test_env::write_executable(&p, "#!/bin/sh\nexec cat\n");
        p
    }

    fn real_home_env() -> GuardEnv {
        GuardEnv {
            shim_dirs: None,
            home: Some(PathBuf::from("/Users/someone")),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: false,
            test_build: false,
        }
    }

    fn temp_home_env(home: &Path) -> GuardEnv {
        GuardEnv {
            shim_dirs: None,
            home: Some(home.to_path_buf()),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: false,
            test_build: false,
        }
    }

    #[test]
    fn shim_dir_wins_over_a_real_binary_on_path() {
        // A "real" claude sits on the search PATH; the shim dir holds the
        // fake. Shim mode must return the shim's absolute path and never
        // look at PATH.
        let real_dir = scratch("real");
        let real = write_exec(&real_dir, "claude");
        let shim_dir = scratch("shim");
        let shim = write_exec(&shim_dir, "claude");
        let env = GuardEnv {
            shim_dirs: Some(vec![shim_dir.clone()]),
            home: Some(PathBuf::from("/Users/someone")),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: false,
            test_build: false,
        };
        let search = std::env::join_paths([real_dir.clone()])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let got = resolve_program("claude", &search, &env).expect("shim resolves");
        assert_eq!(got, shim.to_string_lossy());
        assert_ne!(got, real.to_string_lossy());
        // An absolute path to the REAL binary is redirected by basename too.
        let got_abs = resolve_program(&real.to_string_lossy(), &search, &env).expect("abs → shim");
        assert_eq!(got_abs, shim.to_string_lossy());
        let _ = std::fs::remove_dir_all(&real_dir);
        let _ = std::fs::remove_dir_all(&shim_dir);
    }

    #[test]
    fn shim_dir_list_first_hit_wins() {
        let a = scratch("list-a");
        let b = scratch("list-b");
        let in_b = write_exec(&b, "grok");
        let env = GuardEnv {
            shim_dirs: Some(vec![a.clone(), b.clone()]),
            home: None,
            temp_dirs: vec![],
            allow_real: false,
            test_build: false,
        };
        let got = resolve_program("grok", "/usr/bin:/bin", &env).expect("second dir");
        assert_eq!(got, in_b.to_string_lossy());
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn missing_shim_is_a_loud_error_not_a_path_fallback() {
        let shim_dir = scratch("missing");
        let real_dir = scratch("missing-real");
        write_exec(&real_dir, "claude");
        let env = GuardEnv {
            shim_dirs: Some(vec![shim_dir.clone()]),
            home: Some(PathBuf::from("/Users/someone")),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: true, // allow_real must NOT rescue shim mode
            test_build: false,
        };
        let search = real_dir.to_string_lossy().into_owned();
        let err = resolve_program("claude", &search, &env).expect_err("must refuse");
        assert!(err.contains("not found in K2_TEST_AGENT_SHIM_DIR="), "{err}");
        assert!(err.contains(&shim_dir.to_string_lossy().into_owned()), "{err}");
        assert!(err.contains("refusing to resolve via PATH"), "{err}");
        // Also for /bin/cat: shim mode is strict, no system-dir carve-out.
        let err2 = resolve_program("/bin/cat", &search, &env).expect_err("strict");
        assert!(err2.contains("\"cat\""), "{err2}");
        let _ = std::fs::remove_dir_all(&shim_dir);
        let _ = std::fs::remove_dir_all(&real_dir);
    }

    #[test]
    fn temp_home_refuses_a_real_agent_resolved_via_path() {
        let home = scratch("home");
        let real_dir = scratch("real-agent");
        let real = write_exec(&real_dir, "claude");
        let env = temp_home_env(&home);
        assert!(env.home_is_temp(), "scratch HOME must be seen as temp");
        let search = std::env::join_paths([real_dir.clone(), PathBuf::from("/usr/bin")])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let err = resolve_program("claude", &search, &env).expect_err("refused");
        assert_eq!(
            err,
            format!(
                "[spawn] refusing real agent {} under temp HOME (set K2_TEST_ALLOW_REAL_AGENT=1 to override)",
                real.display()
            )
        );
        // Absolute path outside the system dirs: refused with that path.
        let err_abs = resolve_program(&real.to_string_lossy(), "", &env).expect_err("refused abs");
        assert!(err_abs.contains(&real.display().to_string()), "{err_abs}");
        // Unresolvable bare name: still refused (never ENOENTs into PATH).
        let err_none = resolve_program("claude", "", &env).expect_err("refused unresolved");
        assert!(err_none.contains("refusing real agent claude under temp HOME"), "{err_none}");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&real_dir);
    }

    #[cfg(unix)]
    #[test]
    fn temp_home_belt_carves_out_only_os_system_bins() {
        let home = scratch("home-sys");
        let env = temp_home_env(&home);
        // `cat` on the real system PATH → /bin/cat or /usr/bin/cat: allowed,
        // and the ORIGINAL spelling is returned (no rewrite).
        assert_eq!(resolve_program("cat", "/usr/bin:/bin", &env).unwrap(), "cat");
        assert_eq!(resolve_program("/bin/cat", "", &env).unwrap(), "/bin/cat");
        assert_eq!(resolve_program("/usr/bin/env", "", &env).unwrap(), "/usr/bin/env");
        // /usr/local/bin, /opt/homebrew/bin, ~/.local/bin are NOT system dirs.
        for p in ["/usr/local/bin/claude", "/opt/homebrew/bin/claude", "/Users/x/.local/bin/claude"] {
            let err = resolve_program(p, "", &env).expect_err(p);
            assert!(err.starts_with("[spawn] refusing real agent "), "{err}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn temp_home_allow_real_passes_through_unchanged() {
        let home = scratch("home-allow");
        let mut env = temp_home_env(&home);
        env.allow_real = true;
        assert_eq!(
            resolve_program("/Users/x/.local/bin/claude", "", &env).unwrap(),
            "/Users/x/.local/bin/claude"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn real_home_no_env_is_byte_identical_passthrough() {
        let env = real_home_env();
        assert!(!env.home_is_temp());
        // Even a nonexistent program comes back verbatim — the unguarded
        // path let the PTY layer ENOENT it; we must not change that.
        for p in [
            "claude",
            "/Users/x/.local/bin/claude",
            "definitely-not-installed-xyz",
            "cat",
        ] {
            assert_eq!(resolve_program(p, "/usr/bin:/bin", &env).unwrap(), p);
        }
        // HOME unset is also "not temp".
        let mut no_home = env.clone();
        no_home.home = None;
        assert!(!no_home.home_is_temp());
        assert_eq!(resolve_program("claude", "", &no_home).unwrap(), "claude");
    }

    #[test]
    fn home_is_temp_matches_canonical_prefix() {
        // macOS: temp_dir() is `/var/folders/...` but a canonical HOME is
        // `/private/var/folders/...` — both spellings must match.
        let raw = scratch("canon");
        let canon = raw.canonicalize().unwrap();
        let env_raw = temp_home_env(&raw);
        let env_canon = temp_home_env(&canon);
        assert!(env_raw.home_is_temp());
        assert!(env_canon.home_is_temp());
        let _ = std::fs::remove_dir_all(&raw);
    }

    #[test]
    fn from_process_parses_shim_list_and_allow_flag() {
        // Env guards hold the crate-wide env lock and restore on drop.
        use crate::test_env::EnvVar;
        let a = PathBuf::from("/tmp/k2-guard-a");
        let b = PathBuf::from("/tmp/k2-guard-b");
        {
            let _shim = EnvVar::set(SHIM_DIR_ENV, std::env::join_paths([&a, &b]).unwrap());
            let _allow = EnvVar::set(ALLOW_REAL_ENV, "1");
            let env = GuardEnv::from_process();
            assert_eq!(env.shim_dirs.as_deref(), Some(&[a.clone(), b.clone()][..]));
            assert!(env.allow_real);
        }
        let _shim = EnvVar::set(SHIM_DIR_ENV, "");
        let _allow = EnvVar::set(ALLOW_REAL_ENV, "0");
        let env2 = GuardEnv::from_process();
        assert!(env2.shim_dirs.is_none(), "empty list is unset");
        assert!(!env2.allow_real);
    }

    fn test_build_env(home: &str) -> GuardEnv {
        GuardEnv {
            shim_dirs: None,
            home: Some(PathBuf::from(home)),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: false,
            test_build: true,
        }
    }

    #[test]
    fn test_build_refuses_agent_clis_even_with_a_real_home() {
        // The 0.45.0 gap: a test whose HOME is NOT under the temp dir
        // passed the temp-HOME belt and spawned the real Claude Code.
        let env = test_build_env("/Users/someone");
        for prog in ["claude", "/opt/homebrew/bin/claude", "codex", "cursor-agent", "grok", "bin/claude"] {
            let err = resolve_program(prog, "/usr/bin:/bin", &env).expect_err(prog);
            assert!(err.contains("test tried to spawn real"), "{prog}: {err}");
            assert!(err.contains("refusing"), "{prog}: {err}");
            assert!(err.contains("AgentShim"), "{prog}: {err}");
        }
        // An explicit stub path the test wrote under the temp dir may run.
        let dir = scratch("belt-stub");
        let stub = write_exec(&dir, "codex");
        let stub = stub.to_string_lossy().into_owned();
        assert_eq!(resolve_program(&stub, "/usr/bin", &env).unwrap(), stub);
        std::fs::remove_dir_all(&dir).unwrap();
        // Non-agent programs are untouched.
        assert_eq!(resolve_program("cat", "/usr/bin:/bin", &env).unwrap(), "cat");
        assert_eq!(resolve_program("bash", "/usr/bin:/bin", &env).unwrap(), "bash");
    }

    #[test]
    fn test_build_belt_yields_to_shims_and_explicit_opt_in() {
        let shims = scratch("belt-shim");
        let shim = write_exec(&shims, "claude");
        let env = GuardEnv { shim_dirs: Some(vec![shims.clone()]), ..test_build_env("/Users/someone") };
        assert_eq!(
            resolve_program("claude", "/usr/bin", &env).unwrap(),
            shim.to_string_lossy()
        );
        let env = GuardEnv { allow_real: true, ..test_build_env("/Users/someone") };
        assert_eq!(resolve_program("claude", "/usr/bin", &env).unwrap(), "claude");
        std::fs::remove_dir_all(&shims).unwrap();
    }

    #[test]
    fn from_process_marks_a_test_build() {
        assert!(GuardEnv::from_process().test_build);
    }

    #[test]
    fn agent_cli_names_match_basenames_only() {
        assert!(is_agent_cli("claude"));
        assert!(is_agent_cli("/usr/local/bin/codex"));
        assert!(!is_agent_cli("claude-wrapper"));
        assert!(!is_agent_cli("cat"));
        assert!(!is_agent_cli(""));
    }
}
