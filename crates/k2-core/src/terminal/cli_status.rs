//! Which agent CLIs are installed (`GET /cli/agents/cli-status`).
//!
//! Read only: it never installs anything and never touches the network.
//! For each harness K2 can launch ([`launchable_harnesses`]: the built-in
//! agent presets, `db::BUILT_IN_AGENT_PRESETS`, in launch-bar order) it
//! reports:
//!
//! - `path`: the binary K2 found on the spawn PATH, by the rule
//!   [`super::ensure_cli::ensure_cli`] uses to decide "already installed";
//! - `launches`: whether an agent launch would run that same file
//!   ([`super::daemon_pty::preflight_program`] with the default, empty
//!   preset env, the resolution every agent spawn makes);
//! - `version`: what `<path> <flag>` printed ([`version_flag`], `--version`
//!   for all but one), run with stdin closed, the spawn PATH and a
//!   [`PROBE_TIMEOUT`] cap;
//! - `error`: why a binary that exists did not answer its version flag
//!   (it exited non-zero, timed out, could not start, or printed nothing).
//!
//! A harness with no version flag K2 can rely on is never run: installed,
//! `version: null`, no error.
//!
//! The three states:
//!
//! | state     | installed | version   | path   | error  |
//! |-----------|-----------|-----------|--------|--------|
//! | missing   | false     | null      | null   | null   |
//! | installed | true      | "2.0.14"  | "/…"   | null   |
//! | broken    | true      | null      | "/…"   | "…"    |
//!
//! Probe answers (good or broken) are cached per binary for
//! [`CACHE_TTL`], keyed by the canonical file and its size and mtime, so
//! repeated Settings opens don't re-spawn processes and an update
//! (a new file, or a symlink moved to a new version) is probed again.
//! The harnesses are probed in parallel, so one request takes at most
//! about one [`PROBE_TIMEOUT`].
//!
//! The probe crosses the agent spawn guard first
//! ([`super::agent_spawn_guard::resolve_program`]): a test build or a
//! temp HOME never runs a real agent CLI, even just for `--version`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use super::agent_spawn_guard::GuardEnv;

/// How long one probe answer is trusted for the same binary.
pub const CACHE_TTL: Duration = Duration::from_secs(60);

/// Hard cap on one `<bin> --version`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest `version` / `error` text the route returns.
const MAX_TEXT: usize = 160;

/// One harness's status. Every field is always present on the wire
/// (`null`, never omitted), so a client can tell the three states apart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CliStatus {
    pub program: String,
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub launches: bool,
    pub error: Option<String>,
}

/// Size + mtime of the canonical binary: a reinstall changes one of them.
type Fingerprint = (u64, Option<SystemTime>);

struct Entry {
    fp: Fingerprint,
    at: Instant,
    answer: Result<String, String>,
}

/// Probe answers by canonical binary path.
pub struct ProbeCache {
    ttl: Duration,
    entries: Mutex<HashMap<PathBuf, Entry>>,
}

impl ProbeCache {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, entries: Mutex::new(HashMap::new()) }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }
}

fn global_cache() -> &'static ProbeCache {
    static CACHE: OnceLock<ProbeCache> = OnceLock::new();
    CACHE.get_or_init(|| ProbeCache::new(CACHE_TTL))
}

/// Every harness K2 can launch: the program (first word of the command)
/// of each built-in agent preset, in launch-bar order, once each. The same
/// table the boot seed and Reset Built-ins write, so the route never
/// reports a harness K2 can't launch or misses one it can.
pub fn launchable_harnesses() -> Vec<&'static str> {
    let mut rows: Vec<_> = crate::db::BUILT_IN_AGENT_PRESETS.iter().collect();
    rows.sort_by_key(|r| r.4);
    let mut out: Vec<&'static str> = Vec::new();
    for r in rows {
        if let Some(program) = r.2.split_whitespace().next() {
            if !out.contains(&program) {
                out.push(program);
            }
        }
    }
    out
}

/// The flag that makes `program` print its version and exit, or `None`
/// when K2 doesn't run it at all. The presets carry no version flag, so
/// this table is that definition. Every launchable harness answers
/// `--version` (checked against claude, codex, grok, gemini, cursor-agent,
/// pi and ollama; opencode, goose, aider, copilot and interpreter document
/// it) except:
/// - `hermes`: K2 hasn't confirmed a flag that prints a version and exits
///   without starting the agent, so it is never probed and reports
///   installed with `version: null` and no error.
pub fn version_flag(program: &str) -> Option<&'static str> {
    match program {
        "hermes" => None,
        _ => Some("--version"),
    }
}

/// Production entry: every launchable harness, on the spawn PATH, with
/// the shared cache.
pub fn cli_status() -> Vec<CliStatus> {
    let programs = launchable_harnesses();
    let path = super::ensure_cli::spawn_resolve_path();
    let launch = |program: &str| -> Option<PathBuf> {
        super::daemon_pty::preflight_program(program, &HashMap::new()).ok()
    };
    cli_status_with(
        &programs,
        &path,
        &launch,
        global_cache(),
        PROBE_TIMEOUT,
        &GuardEnv::from_process(),
    )
}

/// Inner entry tests call: `search_path` is where binaries are looked up
/// (and the PATH the probe runs with), `launch` is where an agent launch
/// of a program would resolve, and `guard` decides which binaries may be
/// run at all (production: [`GuardEnv::from_process`]).
pub fn cli_status_with(
    programs: &[&str],
    search_path: &str,
    launch: &(dyn Fn(&str) -> Option<PathBuf> + Sync),
    cache: &ProbeCache,
    timeout: Duration,
    guard: &GuardEnv,
) -> Vec<CliStatus> {
    std::thread::scope(|s| {
        let handles: Vec<_> = programs
            .iter()
            .map(|program| s.spawn(move || one(program, search_path, launch, cache, timeout, guard)))
            .collect();
        handles
            .into_iter()
            .zip(programs)
            .map(|(h, program)| {
                h.join().unwrap_or_else(|_| CliStatus {
                    program: program.to_string(),
                    installed: false,
                    version: None,
                    path: None,
                    launches: false,
                    error: Some("status check panicked".to_string()),
                })
            })
            .collect()
    })
}

fn one(
    program: &str,
    search_path: &str,
    launch: &(dyn Fn(&str) -> Option<PathBuf> + Sync),
    cache: &ProbeCache,
    timeout: Duration,
    guard: &GuardEnv,
) -> CliStatus {
    let Some(found) = super::ensure_cli::find_installed(program, search_path) else {
        return CliStatus {
            program: program.to_string(),
            installed: false,
            version: None,
            path: None,
            launches: false,
            error: None,
        };
    };
    let launches = launch(program).is_some_and(|l| same_file(&l, &found));
    let (version, error) = match version_flag(program) {
        // No flag to ask: installed, version unknown, nothing wrong.
        None => (None, None),
        Some(flag) => match probe_cached(cache, &found, flag, search_path, timeout, guard) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        },
    };
    CliStatus {
        program: program.to_string(),
        installed: true,
        version,
        path: Some(found.to_string_lossy().into_owned()),
        launches,
        error,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

fn fingerprint(p: &Path) -> Option<Fingerprint> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()))
}

fn probe_cached(
    cache: &ProbeCache,
    bin: &Path,
    flag: &str,
    search_path: &str,
    timeout: Duration,
    guard: &GuardEnv,
) -> Result<String, String> {
    let key = dunce::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    let Some(fp) = fingerprint(&key) else {
        return Err(format!("can't read {}", key.display()));
    };
    {
        let g = cache.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.get(&key) {
            if e.fp == fp && e.at.elapsed() < cache.ttl {
                return e.answer.clone();
            }
        }
    }
    let answer = probe_version(bin, flag, search_path, timeout, guard);
    cache
        .entries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, Entry { fp, at: Instant::now(), answer: answer.clone() });
    answer
}

/// `<bin> <flag>` → the version it printed, or a short reason it didn't.
/// Refused (never started) when the agent spawn guard would refuse to
/// launch `bin`.
fn probe_version(
    bin: &Path,
    flag: &str,
    search_path: &str,
    timeout: Duration,
    guard: &GuardEnv,
) -> Result<String, String> {
    let program = bin.to_string_lossy().into_owned();
    match super::agent_spawn_guard::resolve_program(&program, "", guard) {
        // A shim dir maps the program elsewhere: not this file, don't run it.
        Ok(p) if p == program => {}
        Ok(_) => return Err("not checked: tests run agent shims only".to_string()),
        Err(e) => return Err(format!("not checked: {}", clip(&e))),
    }
    let (prog, args) =
        super::win_cmd::resolve_spawn(&program, &[flag.to_string()], search_path);
    let mut cmd = crate::bg_proc::command(&prog);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !search_path.is_empty() {
        cmd.env("PATH", search_path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    // `wait_with_output` drains both pipes; the thread is what lets the
    // deadline kill the process group instead of blocking the caller.
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => {
            read_answer(flag, out.status.success(), out.status.code(), &out.stdout, &out.stderr)
        }
        Ok(Err(e)) => Err(format!("could not run: {e}")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_tree(pid);
            Err(format!("{flag} timed out after {}", human(timeout)))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!("{flag}: waiter dropped")),
    }
}

fn read_answer(
    flag: &str,
    ok: bool,
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<String, String> {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let first_line = |s: &str| s.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string);
    let line = first_line(&out).or_else(|| first_line(&err));
    if !ok {
        let why = match code {
            Some(c) => format!("{flag} exited {c}"),
            None => format!("{flag} was killed by a signal"),
        };
        return Err(match first_line(&err).or(line) {
            Some(l) => clip(&format!("{why}: {l}")),
            None => why,
        });
    }
    match line {
        Some(l) => Ok(parse_version(&l).unwrap_or_else(|| clip(&l))),
        None => Err(format!("{flag} printed nothing")),
    }
}

/// The first version-looking token of a `--version` line:
/// `2.0.14 (Claude Code)` → `2.0.14`, `codex-cli 0.46.0` → `0.46.0`,
/// `v1.2.3` → `1.2.3`. `None` when the line has none.
pub fn parse_version(line: &str) -> Option<String> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')))
        .map(|t| t.strip_prefix(|c: char| matches!(c, 'v' | 'V')).unwrap_or(t))
        .find_map(|t| {
            let digits = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
            if digits == 0 || !t[digits..].starts_with('.') {
                return None;
            }
            let v = t.trim_end_matches(|c: char| matches!(c, '.' | '-' | '+'));
            v[digits..].chars().nth(1).filter(char::is_ascii_digit).map(|_| v.to_string())
        })
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.len() <= MAX_TEXT {
        return s.to_string();
    }
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn human(d: Duration) -> String {
    if d.as_millis() % 1000 == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(unix)]
fn kill_tree(pid: u32) {
    let pgid = pid as i32;
    unsafe {
        if libc::killpg(pgid, libc::SIGKILL) != 0 {
            libc::kill(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn kill_tree(pid: u32) {
    let _ = crate::bg_proc::command("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn kill_tree(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;

    /// Production rules (not a test build) under a home outside the temp
    /// dir: temp-dir fakes may run.
    #[cfg(unix)]
    fn open_guard() -> GuardEnv {
        GuardEnv {
            shim_dirs: None,
            home: Some(PathBuf::from("/nonexistent-k2-home")),
            temp_dirs: Vec::new(),
            allow_real: false,
            test_build: false,
        }
    }

    #[test]
    fn version_is_the_first_version_token() {
        assert_eq!(parse_version("2.0.14 (Claude Code)").as_deref(), Some("2.0.14"));
        assert_eq!(parse_version("codex-cli 0.46.0").as_deref(), Some("0.46.0"));
        assert_eq!(parse_version("grok v1.2.3-beta.1").as_deref(), Some("1.2.3-beta.1"));
        assert_eq!(parse_version("gemini/0.1.5.").as_deref(), Some("0.1.5"));
        assert_eq!(parse_version("tool 3 build 7"), None);
        assert_eq!(parse_version("x86_64 1.x"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn exit_codes_and_empty_output_are_broken() {
        assert_eq!(read_answer("--version", true, Some(0), b"\n  1.4.0 \n", b""), Ok("1.4.0".to_string()));
        assert_eq!(read_answer("--version", true, Some(0), b"", b"Mytool 2.1\n"), Ok("2.1".to_string()));
        assert_eq!(read_answer("--version", true, Some(0), b"dev build\n", b""), Ok("dev build".to_string()));
        assert_eq!(read_answer("--version", true, Some(0), b"", b""), Err("--version printed nothing".to_string()));
        assert_eq!(
            read_answer("--version", false, Some(3), b"", b"node: not found\n"),
            Err("--version exited 3: node: not found".to_string())
        );
        assert_eq!(read_answer("--version", false, None, b"", b""), Err("--version was killed by a signal".to_string()));
        let long = "e".repeat(400);
        let err = read_answer("--version", false, Some(1), b"", long.as_bytes()).unwrap_err();
        assert!(err.chars().count() <= MAX_TEXT + 1, "clipped: {}", err.len());
    }

    /// A temp `bin/` holding fake CLIs; returns (dir, bin).
    #[cfg(unix)]
    fn bin_dir(tag: &str) -> (PathBuf, PathBuf) {
        let dir = crate::test_env::unique_temp_path(&format!("cli-status-{tag}"));
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        (dir, bin)
    }

    /// A fake CLI that counts its runs in `<bin>/<name>.runs`.
    #[cfg(unix)]
    fn fake(bin: &Path, name: &str, body: &str) -> PathBuf {
        let p = bin.join(name);
        let runs = bin.join(format!("{name}.runs"));
        let script = format!(
            "#!/bin/sh\nprintf x >> '{}'\n{body}\n",
            runs.to_string_lossy()
        );
        crate::test_env::write_executable(&p, &script);
        p
    }

    #[cfg(unix)]
    fn runs(bin: &Path, name: &str) -> usize {
        match fs::read_to_string(bin.join(format!("{name}.runs"))) {
            Ok(s) => s.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => panic!("read runs: {e}"),
        }
    }

    #[cfg(unix)]
    fn launch_here(bin: PathBuf) -> impl Fn(&str) -> Option<PathBuf> + Sync {
        move |p: &str| Some(bin.join(p)).filter(|f| f.is_file())
    }

    #[cfg(unix)]
    fn status(
        programs: &[&str],
        bin: &Path,
        cache: &ProbeCache,
        timeout: Duration,
    ) -> Vec<CliStatus> {
        let path = bin.to_string_lossy().into_owned();
        let launch = launch_here(bin.to_path_buf());
        cli_status_with(programs, &path, &launch, cache, timeout, &open_guard())
    }

    #[cfg(unix)]
    #[test]
    fn missing_installed_and_broken() {
        let _lock = crate::test_env::lock();
        let (dir, bin) = bin_dir("states");
        let claude = fake(&bin, "claude", "echo '2.0.14 (Claude Code)'");
        fake(&bin, "codex", "echo 'boom' >&2; exit 2");
        // The probe's PATH is only `bin`, so the full path.
        fake(&bin, "grok", "exec /bin/sleep 30");
        let cache = ProbeCache::new(CACHE_TTL);
        let started = Instant::now();
        let got = status(&["claude", "codex", "grok", "gemini"], &bin, &cache, Duration::from_millis(400));
        assert!(started.elapsed() < Duration::from_secs(10), "a hung probe must not hold the request");
        let by: HashMap<&str, &CliStatus> = got.iter().map(|s| (s.program.as_str(), s)).collect();

        assert_eq!(
            *by["claude"],
            CliStatus {
                program: "claude".into(),
                installed: true,
                version: Some("2.0.14".into()),
                path: Some(claude.to_string_lossy().into_owned()),
                launches: true,
                error: None,
            }
        );
        let codex = by["codex"];
        assert!(codex.installed && codex.version.is_none(), "{codex:?}");
        assert_eq!(codex.error.as_deref(), Some("--version exited 2: boom"));
        let grok = by["grok"];
        assert!(grok.installed && grok.version.is_none(), "{grok:?}");
        assert_eq!(grok.error.as_deref(), Some("--version timed out after 400ms"));
        assert_eq!(
            *by["gemini"],
            CliStatus {
                program: "gemini".into(),
                installed: false,
                version: None,
                path: None,
                launches: false,
                error: None,
            }
        );
        let json = serde_json::to_value(by["gemini"]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"program":"gemini","installed":false,"version":null,"path":null,"launches":false,"error":null})
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file that isn't executable is not installed (the ensure-cli rule).
    #[cfg(unix)]
    #[test]
    fn a_non_executable_file_is_missing() {
        let _lock = crate::test_env::lock();
        let (dir, bin) = bin_dir("noexec");
        fs::write(bin.join("claude"), "#!/bin/sh\necho 1.0.0\n").unwrap();
        let got = status(&["claude"], &bin, &ProbeCache::new(CACHE_TTL), PROBE_TIMEOUT);
        assert!(!got[0].installed && got[0].path.is_none(), "{:?}", got[0]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn launches_is_false_when_a_launch_runs_another_file() {
        let _lock = crate::test_env::lock();
        let (dir, bin) = bin_dir("launch");
        fake(&bin, "claude", "echo 1.0.0");
        let other = dir.join("other");
        fs::create_dir_all(&other).unwrap();
        let other_claude = fake(&other, "claude", "echo 9.9.9");
        let path = bin.to_string_lossy().into_owned();
        let elsewhere = move |_: &str| Some(other_claude.clone());
        let cache = ProbeCache::new(CACHE_TTL);
        let got = cli_status_with(&["claude"], &path, &elsewhere, &cache, PROBE_TIMEOUT, &open_guard());
        assert!(got[0].installed && !got[0].launches, "{:?}", got[0]);
        assert_eq!(got[0].version.as_deref(), Some("1.0.0"), "the found binary is probed");
        let nowhere = |_: &str| -> Option<PathBuf> { None };
        let got = cli_status_with(&["claude"], &path, &nowhere, &cache, PROBE_TIMEOUT, &open_guard());
        assert!(got[0].installed && !got[0].launches, "{:?}", got[0]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn answers_are_cached_per_binary_until_ttl_or_change() {
        let _lock = crate::test_env::lock();
        let (dir, bin) = bin_dir("cache");
        fake(&bin, "claude", "echo 1.0.0");
        fake(&bin, "codex", "exit 4");
        let cache = ProbeCache::new(CACHE_TTL);
        for _ in 0..3 {
            let got = status(&["claude", "codex"], &bin, &cache, PROBE_TIMEOUT);
            assert_eq!(got[0].version.as_deref(), Some("1.0.0"));
            assert_eq!(got[1].error.as_deref(), Some("--version exited 4"));
        }
        assert_eq!(runs(&bin, "claude"), 1, "a good answer is cached");
        assert_eq!(runs(&bin, "codex"), 1, "a broken answer is cached too");

        // A reinstall (new size) is probed again at once.
        fake(&bin, "claude", "echo 1.1.0 # a newer build");
        let got = status(&["claude"], &bin, &cache, PROBE_TIMEOUT);
        assert_eq!(got[0].version.as_deref(), Some("1.1.0"));
        assert_eq!(runs(&bin, "claude"), 2);

        // Past the TTL the same binary is probed again.
        let expired = ProbeCache::new(Duration::ZERO);
        status(&["claude"], &bin, &expired, PROBE_TIMEOUT);
        status(&["claude"], &bin, &expired, PROBE_TIMEOUT);
        assert_eq!(runs(&bin, "claude"), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A test build never runs a real agent CLI, even for `--version`:
    /// a binary outside the temp dir is reported, not probed.
    #[cfg(unix)]
    #[test]
    fn the_spawn_guard_refuses_a_real_cli() {
        let test_build = GuardEnv {
            shim_dirs: None,
            home: Some(PathBuf::from("/nonexistent-k2-home")),
            temp_dirs: vec![std::env::temp_dir()],
            allow_real: false,
            test_build: true,
        };
        let real = Path::new("/usr/local/bin/claude");
        let got = probe_version(real, "--version", "", PROBE_TIMEOUT, &test_build).unwrap_err();
        assert!(got.starts_with("not checked:"), "{got}");
        let shims = GuardEnv { shim_dirs: Some(vec![std::env::temp_dir()]), ..test_build };
        let got = probe_version(real, "--version", "", PROBE_TIMEOUT, &shims).unwrap_err();
        assert!(got.starts_with("not checked:"), "{got}");
    }

    #[test]
    fn the_route_reports_every_launchable_harness_in_launch_bar_order() {
        assert_eq!(
            launchable_harnesses(),
            [
                "claude", "codex", "grok", "gemini", "cursor-agent", "pi", "hermes", "opencode",
                "goose", "aider", "ollama", "copilot", "interpreter",
            ]
        );
        // Stable: the same list, in the same order, every call.
        assert_eq!(launchable_harnesses(), launchable_harnesses());
        // The spawn guard's agent list is the presets plus the `cursor`
        // alias: the two never drift apart.
        let mut guard: Vec<&str> = super::super::agent_spawn_guard::AGENT_CLIS
            .iter()
            .copied()
            .filter(|p| *p != "cursor")
            .collect();
        let mut ours = launchable_harnesses();
        guard.sort_unstable();
        ours.sort_unstable();
        assert_eq!(ours, guard);
        // Everything ensure-cli can install is launchable.
        for p in super::super::ensure_cli::HARNESSES {
            assert!(launchable_harnesses().contains(p), "{p}");
        }
        assert_eq!(CACHE_TTL, Duration::from_secs(60));
        assert_eq!(PROBE_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn every_harness_but_hermes_answers_dash_dash_version() {
        for p in launchable_harnesses() {
            let want = if p == "hermes" { None } else { Some("--version") };
            assert_eq!(version_flag(p), want, "{p}");
        }
    }

    /// A harness with no version flag is installed with an unknown
    /// version and no error, and is never run.
    #[cfg(unix)]
    #[test]
    fn a_harness_without_a_version_flag_is_never_run() {
        let _lock = crate::test_env::lock();
        let (dir, bin) = bin_dir("noflag");
        let hermes = fake(&bin, "hermes", "echo 9.9.9");
        let got = status(&["hermes"], &bin, &ProbeCache::new(CACHE_TTL), PROBE_TIMEOUT);
        assert_eq!(
            got[0],
            CliStatus {
                program: "hermes".into(),
                installed: true,
                version: None,
                path: Some(hermes.to_string_lossy().into_owned()),
                launches: true,
                error: None,
            }
        );
        assert_eq!(runs(&bin, "hermes"), 0, "no version flag: never started");
        let _ = fs::remove_dir_all(&dir);
    }
}
