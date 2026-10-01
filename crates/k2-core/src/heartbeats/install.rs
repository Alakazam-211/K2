//! Daemon-first cron infrastructure installer.
//!
//! The pre-P5 flow required a user to visit Settings → Wake Scheduler
//! → Apply before any heartbeat would actually fire from cron. New
//! users who created a heartbeat through Settings → Heartbeats → Add
//! got a DB row and a WAKEUP.md but no `heartbeat.sh` and no launchd
//! plist — silent failure with no obvious recovery path.
//!
//! This module is the daemon's self-bootstrap. [`ensure_cron_installed`]
//! is idempotent and called from [`crate::heartbeats::k2so_heartbeat_add`]
//! after a successful row insert. Headless installs (CLI / daemon
//! without Tauri ever launched) get cron working from the first
//! `k2so heartbeat add` onward.
//!
//! Generates `~/.k2/heartbeat.sh` (the bridge that asks the daemon
//! `/cli/heartbeat/active-projects` and ticks each one) and installs
//! the launchd agent (macOS) or crontab entry (Linux).
//!
//! Heartbeat S1 (`prd-heartbeat-firing-v1.md`, HB6–HB11):
//! - [`transport_writes_allowed`] guards every write (HB6);
//! - every `launchctl` / `crontab` call goes through a
//!   [`CommandRunner`] (HB7);
//! - [`transport_state`] reports `ok` / `missing` / `foreign` /
//!   `failing` / `silent` / `unsupported` from what launchd actually has
//!   loaded, not from the plist on disk (HB8);
//! - [`install_from_saved_settings`] is the one installer, driven by
//!   `app_settings.wake_scheduler` (HB11);
//! - [`self_check_and_repair`] boots out and re-bootstraps a job that is
//!   not `ok` (HB10).

use std::fs;
use std::path::{Path, PathBuf};

/// Default tick cadence in seconds. Matches the P5.7 default in
/// `src-tauri::commands::settings::default_wake_interval`. Empty
/// ticks return in microseconds (no-op when no heartbeats are due);
/// full ticks are bounded by the P5.4 spawn pool so the increase
/// over the legacy 300s is safe.
pub const DEFAULT_INTERVAL_SECS: u32 = 60;

/// Opt-out flag. When `=1`, nothing in this module writes, loads, boots
/// out or removes the OS scheduler job (the headless e2e harness and any
/// scratch-HOME daemon set it).
pub const NO_SELF_HEAL_ENV: &str = "K2_HEARTBEAT_NO_SELF_HEAL";

// ── HB6 — one transport guard ──────────────────────────────────────────
//
// 2026-09-30: a `cargo test -p k2-core` run swapped `$HOME` to a temp
// folder on one thread while another thread's `heartbeat add` reached
// `install_macos_if_missing`. It booted out this Mac's real
// `dev.k2.heartbeat` job and bootstrapped a plist from the temp folder,
// which the test then deleted. No scheduled heartbeat fired for ~8h.
//
// Every function that writes, loads, boots out or removes the OS
// scheduler job (launchd plist, crontab line, `heartbeat.sh`) calls
// [`transport_writes_allowed`] FIRST. A refusal is logged and returned
// as `Err` — never `Ok`.

/// May this process touch the OS scheduler job right now?
///
/// Refuses (in this order) when:
/// 1. `K2_HEARTBEAT_NO_SELF_HEAL=1` is set;
/// 2. `$HOME` is not the password-database home for this uid (a test,
///    a scratch-HOME daemon, a sandboxed child);
/// 3. this is a `cfg(test)` build of k2-core.
pub fn transport_writes_allowed() -> Result<(), String> {
    let flag = std::env::var(NO_SELF_HEAL_ENV).ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let pw_home = password_home();
    let verdict = writes_allowed_for(
        flag.as_deref(),
        home.as_deref(),
        pw_home.as_deref(),
        cfg!(test),
    );
    if let Err(e) = &verdict {
        crate::log_debug!("[heartbeat-transport] {e}");
    }
    verdict
}

/// Pure decision behind [`transport_writes_allowed`]. `pw_home = None`
/// (no password entry, or a platform without one) skips the HOME check;
/// the flag and the test check still apply.
pub(crate) fn writes_allowed_for(
    flag: Option<&str>,
    home: Option<&Path>,
    pw_home: Option<&Path>,
    under_test: bool,
) -> Result<(), String> {
    if flag.map(str::trim) == Some("1") {
        return Err(format!(
            "heartbeat transport write refused: {NO_SELF_HEAL_ENV}=1"
        ));
    }
    if let (Some(home), Some(pw_home)) = (home, pw_home) {
        if !same_dir(home, pw_home) {
            return Err(format!(
                "heartbeat transport write refused: HOME is not the user home \
                 (HOME={}, user home={})",
                home.display(),
                pw_home.display()
            ));
        }
    }
    if under_test {
        return Err(
            "heartbeat transport write refused: cfg(test) builds never touch the OS scheduler"
                .to_string(),
        );
    }
    Ok(())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => {
            let ta = a.to_string_lossy();
            let tb = b.to_string_lossy();
            ta.trim_end_matches('/') == tb.trim_end_matches('/')
        }
    }
}

/// The home directory the password database lists for this uid.
#[cfg(unix)]
fn password_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    let uid = unsafe { libc::getuid() };
    let mut size = 4096usize;
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result)
        };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) }
            .to_string_lossy()
            .into_owned();
        return if dir.is_empty() { None } else { Some(PathBuf::from(dir)) };
    }
}

#[cfg(not(unix))]
fn password_home() -> Option<PathBuf> {
    None
}

// ── HB7 — one command runner ───────────────────────────────────────────
//
// Every `launchctl` / `crontab` call goes through a [`CommandRunner`].
// Production uses [`SystemRunner`]. Under `cfg(test)` the system runner
// records the call and runs NOTHING, so even a test that slips past the
// guard cannot reach the real user session. Tests that need launchctl
// output hand a fake runner to the `*_with` functions.

/// Captured output of one scheduler command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `launchctl` / `crontab`. `Err` means the program could not be
/// run at all (not installed, spawn failure, or a `cfg(test)` refusal).
pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput, String>;
}

/// The real runner.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    #[cfg(not(test))]
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(program);
        cmd.args(args);
        let output = if let Some(input) = stdin {
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("spawn {program}: {e}"))?;
            child
                .stdin
                .take()
                .ok_or_else(|| format!("{program}: no stdin"))?
                .write_all(input.as_bytes())
                .map_err(|e| format!("write {program} stdin: {e}"))?;
            child
                .wait_with_output()
                .map_err(|e| format!("wait {program}: {e}"))?
        } else {
            cmd.output().map_err(|e| format!("run {program}: {e}"))?
        };
        Ok(CmdOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    #[cfg(test)]
    fn run(&self, program: &str, args: &[&str], _stdin: Option<&str>) -> Result<CmdOutput, String> {
        test_recorder::record(program, args);
        Err(format!(
            "cfg(test): SystemRunner never runs `{program}` — use a fake runner"
        ))
    }
}

/// Calls that reached [`SystemRunner`] under `cfg(test)`, per thread.
#[cfg(test)]
pub(crate) mod test_recorder {
    use std::cell::RefCell;

    thread_local! {
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn record(program: &str, args: &[&str]) {
        CALLS.with(|c| c.borrow_mut().push(format!("{program} {}", args.join(" "))));
    }

    /// Drain this thread's recorded calls.
    pub(crate) fn take() -> Vec<String> {
        CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }
}

/// `crontab -l`, or empty when the user has no crontab / no `crontab`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn crontab_read(runner: &dyn CommandRunner) -> String {
    runner
        .run("crontab", &["-l"], None)
        .ok()
        .and_then(|o| if o.success { Some(o.stdout) } else { None })
        .unwrap_or_default()
}

/// Replace the user's crontab with `content` (`crontab -`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn crontab_write(runner: &dyn CommandRunner, content: &str) -> Result<(), String> {
    let out = runner
        .run("crontab", &["-"], Some(content))
        .map_err(|e| format!("crontab: {e}"))?;
    if !out.success {
        return Err(format!("crontab - failed: {}", out.stderr.trim()));
    }
    Ok(())
}

fn launchd_target() -> String {
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0u32;
    format!("gui/{uid}/dev.k2.heartbeat")
}

// ── Paths, platform, job spec ──────────────────────────────────────────

/// The launchd label of the heartbeat job.
pub const LAUNCHD_LABEL: &str = "dev.k2.heartbeat";
/// Marker comment on the crontab line.
pub const CRON_MARKER: &str = "k2so-agent-heartbeat";

/// `<home>/Library/LaunchAgents/dev.k2.heartbeat.plist`.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents/dev.k2.heartbeat.plist")
}

/// `<home>/.k2/heartbeat.sh`.
pub fn script_path(home: &Path) -> PathBuf {
    home.join(".k2/heartbeat.sh")
}

fn launchd_domain() -> String {
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0u32;
    format!("gui/{uid}")
}

/// Which OS scheduler carries the tick. Tests pass one explicitly so the
/// launchd logic is exercised on Linux CI and the cron logic on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Launchd,
    Cron,
    Unsupported,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Launchd
        } else if cfg!(target_os = "linux") {
            Platform::Cron
        } else {
            Platform::Unsupported
        }
    }
}

/// What the OS job should look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobSpec {
    pub interval_secs: u32,
    pub wake_system: bool,
}

/// HB11 — the job each saved wake-scheduler mode wants.
///
/// - `heartbeat` → the user's interval and `WakeSystem` setting.
/// - `on_demand` → today's de facto job: 60 s, never wakes the machine.
/// - `off` → the same 60 s no-wake job. Rosson decision D2 (2026-10-01):
///   Off means "don't wake the computer from sleep"; heartbeats still
///   fire whenever it is awake. Until the daemon ticks itself (S2), the
///   OS job is the only thing that fires them, so Off keeps it.
///
/// `Ok(None)` ("no OS job") is reserved for S2.
pub fn job_spec_for(
    ws: &crate::app_settings::WakeSchedulerSettings,
) -> Result<Option<JobSpec>, String> {
    match ws.mode.as_str() {
        "heartbeat" => Ok(Some(JobSpec {
            interval_secs: ws.interval_minutes.max(1).saturating_mul(60),
            wake_system: ws.wake_system,
        })),
        "on_demand" | "off" => Ok(Some(JobSpec {
            interval_secs: DEFAULT_INTERVAL_SECS,
            wake_system: false,
        })),
        other => Err(format!(
            "unknown wake scheduler mode '{other}'. Expected 'off', 'on_demand', or 'heartbeat'."
        )),
    }
}

/// The job the SAVED settings (`app_settings.wake_scheduler`) want.
pub fn saved_job_spec() -> Result<Option<JobSpec>, String> {
    job_spec_for(&crate::app_settings::load().wake_scheduler)
}

/// The launchd plist for `spec`, byte-identical to what every earlier
/// installer wrote so an unchanged job is never reloaded.
pub fn plist_for_spec(home: &Path, spec: JobSpec) -> String {
    let wake_key = if spec.wake_system {
        "\n    <key>WakeSystem</key>\n    <true/>"
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.k2.heartbeat</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{script}</string>
    </array>
    <key>StartInterval</key>
    <integer>{interval}</integer>{wake_key}
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2/heartbeat-stderr.log</string>
</dict>
</plist>"#,
        script = script_path(home).to_string_lossy(),
        interval = spec.interval_secs,
        wake_key = wake_key,
        home = home.to_string_lossy(),
    )
}

fn cron_entry(home: &Path) -> String {
    format!("* * * * * {} # {CRON_MARKER}", script_path(home).to_string_lossy())
}

fn describe(spec: JobSpec) -> String {
    let mins = spec.interval_secs.max(60) / 60;
    format!(
        "heartbeat scheduler installed (every {} min{}).",
        mins,
        if spec.wake_system { " — wakes system from sleep" } else { "" }
    )
}

// ── HB8 — an honest transport check ────────────────────────────────────

/// How the loaded job last exited, per `launchctl print`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastExit {
    NeverExited,
    Code(i64),
    NotShown,
}

/// The fields of `launchctl print gui/<uid>/dev.k2.heartbeat` the
/// self-check needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchctlJob {
    /// The plist the LOADED job came from (not the one on disk).
    pub path: Option<String>,
    pub program: Option<String>,
    pub arguments: Vec<String>,
    pub last_exit: LastExit,
    pub run_interval_secs: Option<u64>,
}

/// Parse `launchctl print` output. Only top-level keys are read, so the
/// nested `stderr path`, coalition `state`, etc. never leak in.
pub fn parse_launchctl_print(text: &str) -> LaunchctlJob {
    let mut job = LaunchctlJob {
        path: None,
        program: None,
        arguments: Vec::new(),
        last_exit: LastExit::NotShown,
        run_interval_secs: None,
    };
    let mut depth: i32 = 0;
    let mut in_args = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            depth -= 1;
            if depth <= 1 {
                in_args = false;
            }
            continue;
        }
        if line.ends_with('{') {
            if depth == 1 && line.starts_with("arguments =") {
                in_args = true;
            }
            depth += 1;
            continue;
        }
        if in_args && depth == 2 {
            job.arguments.push(line.to_string());
            continue;
        }
        if depth != 1 {
            continue;
        }
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "path" => job.path = Some(value.to_string()),
            "program" => job.program = Some(value.to_string()),
            "last exit code" => job.last_exit = parse_last_exit(value),
            "run interval" => {
                job.run_interval_secs = value.split_whitespace().next().and_then(|n| n.parse().ok())
            }
            _ => {}
        }
    }
    job
}

fn parse_last_exit(value: &str) -> LastExit {
    if value.contains("never exited") {
        return LastExit::NeverExited;
    }
    let digits: String = value
        .chars()
        .enumerate()
        .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && *c == '-'))
        .map(|(_, c)| c)
        .collect();
    digits.parse().map(LastExit::Code).unwrap_or(LastExit::NotShown)
}

/// HB8 states. `Ok` is the only healthy one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    /// Loaded from the real plist, runs the real script, exits 0 (or has
    /// not run yet), and OS ticks arrive.
    Ok,
    /// Not loaded, no crontab line, or the plist is gone from disk.
    Missing,
    /// Loaded, but from another plist or running another script — the
    /// 2026-09-30 temp-HOME hijack.
    Foreign,
    /// Real paths, nonzero last exit code.
    Failing,
    /// Looks right, but no OS tick in three intervals.
    Silent,
    /// No OS scheduler on this platform.
    Unsupported,
}

/// What [`transport_state`] found. Serialised into
/// `/cli/heartbeat/scheduler-status` as `transport`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportReport {
    pub state: TransportState,
    /// The plist the loaded job came from (launchd) — not the one on disk.
    pub plist_path: Option<String>,
    /// The script the job runs.
    pub program_path: Option<String>,
    /// `None` = never exited / not shown.
    pub last_exit_code: Option<i64>,
    pub interval_secs: Option<u64>,
    /// Human-readable reason.
    pub detail: String,
}

impl TransportReport {
    fn bare(state: TransportState, detail: impl Into<String>) -> Self {
        Self {
            state,
            plist_path: None,
            program_path: None,
            last_exit_code: None,
            interval_secs: None,
            detail: detail.into(),
        }
    }
}

/// The tick evidence the `silent` verdict is judged on.
#[derive(Debug, Clone, Copy)]
pub struct TickEvidence {
    pub now: chrono::DateTime<chrono::Utc>,
    /// When this daemon started watching the job (boot, wake, or the last
    /// reload). launchd does not report when a job was loaded, so "loaded
    /// longer than three intervals" is measured from here.
    pub observed_since: chrono::DateTime<chrono::Utc>,
    /// `scheduler_meta.last_os_tick_at`.
    pub last_os_tick: Option<chrono::DateTime<chrono::Utc>>,
}

/// `Some(reason)` when the job has been watched for more than three
/// intervals and no OS tick arrived within the last three.
fn silent_reason(interval_secs: u64, ev: &TickEvidence) -> Option<String> {
    let window = chrono::Duration::seconds((interval_secs.max(1) as i64).saturating_mul(3));
    if ev.now - ev.observed_since <= window {
        return None;
    }
    match ev.last_os_tick {
        Some(t) if ev.now - t <= window => None,
        Some(t) => Some(format!(
            "no OS tick for {}s (last {}); the job should tick every {}s",
            (ev.now - t).num_seconds(),
            t.to_rfc3339(),
            interval_secs
        )),
        None => Some(format!(
            "no OS tick recorded in {}s of watching; the job should tick every {}s",
            (ev.now - ev.observed_since).num_seconds(),
            interval_secs
        )),
    }
}

/// Classify a launchd job. `print` is the stdout of a successful
/// `launchctl print`, or `None` when the label is not loaded.
pub fn classify_launchd(
    print: Option<&str>,
    home: &Path,
    plist_on_disk: bool,
    fallback_interval_secs: u64,
    ev: &TickEvidence,
) -> TransportReport {
    let want_plist = plist_path(home).to_string_lossy().into_owned();
    let want_script = script_path(home).to_string_lossy().into_owned();
    let Some(text) = print else {
        return TransportReport::bare(
            TransportState::Missing,
            format!("{LAUNCHD_LABEL} is not loaded in launchd"),
        );
    };
    let job = parse_launchctl_print(text);
    let interval = job.run_interval_secs.unwrap_or(fallback_interval_secs);
    let mut r = TransportReport {
        state: TransportState::Ok,
        plist_path: job.path.clone(),
        program_path: job.arguments.get(1).cloned().or_else(|| job.program.clone()),
        last_exit_code: match job.last_exit {
            LastExit::Code(c) => Some(c),
            _ => None,
        },
        interval_secs: Some(interval),
        detail: String::new(),
    };
    let want_args = vec!["/bin/bash".to_string(), want_script.clone()];
    if job.path.as_deref() != Some(want_plist.as_str()) || job.arguments != want_args {
        r.state = TransportState::Foreign;
        r.detail = format!(
            "loaded job comes from {} and runs `{}`; expected {} running `/bin/bash {}`",
            job.path.as_deref().unwrap_or("<no path>"),
            job.arguments.join(" "),
            want_plist,
            want_script
        );
        return r;
    }
    if !plist_on_disk {
        r.state = TransportState::Missing;
        r.detail = format!("{want_plist} is missing on disk");
        return r;
    }
    if let Some(code) = r.last_exit_code {
        if code != 0 {
            r.state = TransportState::Failing;
            r.detail = format!("last exit code {code}");
            return r;
        }
    }
    if let Some(why) = silent_reason(interval, ev) {
        r.state = TransportState::Silent;
        r.detail = why;
        return r;
    }
    r.detail = "loaded from the real plist and ticking".to_string();
    r
}

/// Classify the crontab. `Err` = `crontab` could not run (not installed),
/// `Ok(None)` = no crontab for this user.
pub fn classify_cron(
    crontab: Result<Option<String>, String>,
    home: &Path,
    ev: &TickEvidence,
) -> TransportReport {
    let want_script = script_path(home).to_string_lossy().into_owned();
    let text = match crontab {
        Err(e) => {
            return TransportReport::bare(
                TransportState::Missing,
                format!("crontab is not available: {e}"),
            )
        }
        Ok(None) => {
            return TransportReport::bare(TransportState::Missing, "no crontab for this user")
        }
        Ok(Some(t)) => t,
    };
    let scripts: Vec<String> = text
        .lines()
        .filter(|l| l.contains(CRON_MARKER) && !l.trim_start().starts_with('#'))
        .map(|l| l.split_whitespace().nth(5).unwrap_or("").to_string())
        .collect();
    if scripts.is_empty() {
        return TransportReport::bare(
            TransportState::Missing,
            format!("no {CRON_MARKER} line in the crontab"),
        );
    }
    let interval = DEFAULT_INTERVAL_SECS as u64;
    let mut r = TransportReport {
        state: TransportState::Ok,
        plist_path: None,
        program_path: scripts.first().cloned(),
        last_exit_code: None,
        interval_secs: Some(interval),
        detail: String::new(),
    };
    if let Some(other) = scripts.iter().find(|s| **s != want_script) {
        r.state = TransportState::Foreign;
        r.program_path = Some(other.clone());
        r.detail = format!("crontab line runs {other}; expected {want_script}");
        return r;
    }
    if let Some(why) = silent_reason(interval, ev) {
        r.state = TransportState::Silent;
        r.detail = why;
        return r;
    }
    r.detail = "crontab line points at the real script and ticks arrive".to_string();
    r
}

/// Read the job's state through `runner`. Read-only: `launchctl print`
/// or `crontab -l`, nothing else.
pub(crate) fn state_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    fallback_interval_secs: u64,
    ev: &TickEvidence,
) -> TransportReport {
    match platform {
        Platform::Launchd => {
            let print = runner.run("launchctl", &["print", &launchd_target()], None);
            let text = match &print {
                Ok(o) if o.success => Some(o.stdout.as_str()),
                _ => None,
            };
            let mut r =
                classify_launchd(text, home, plist_path(home).exists(), fallback_interval_secs, ev);
            if let Err(e) = &print {
                r.detail = format!("{} ({e})", r.detail);
            }
            r
        }
        Platform::Cron => {
            let crontab = runner
                .run("crontab", &["-l"], None)
                .map(|o| if o.success { Some(o.stdout) } else { None });
            classify_cron(crontab, home, ev)
        }
        Platform::Unsupported => TransportReport::bare(
            TransportState::Unsupported,
            "no OS scheduler job on this platform",
        ),
    }
}

/// Start of the current watch window (see [`TickEvidence::observed_since`]).
static OBSERVED_SINCE: parking_lot::Mutex<Option<chrono::DateTime<chrono::Utc>>> =
    parking_lot::Mutex::new(None);

/// Restart the "loaded longer than three intervals" clock. Called at
/// daemon boot, after a wake (the job could not tick while the machine
/// slept), and after this module reloads the job.
pub fn reset_observation_window() {
    *OBSERVED_SINCE.lock() = Some(chrono::Utc::now());
}

fn observed_since() -> chrono::DateTime<chrono::Utc> {
    *OBSERVED_SINCE.lock().get_or_insert_with(chrono::Utc::now)
}

fn last_os_tick() -> Option<chrono::DateTime<chrono::Utc>> {
    use crate::db::schema::SchedulerMeta;
    let db = crate::db::shared();
    let conn = db.lock();
    SchedulerMeta::get(&conn, SchedulerMeta::LAST_OS_TICK_AT)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn current_evidence() -> TickEvidence {
    TickEvidence {
        now: chrono::Utc::now(),
        observed_since: observed_since(),
        last_os_tick: last_os_tick(),
    }
}

fn fallback_interval_secs() -> u64 {
    saved_job_spec()
        .ok()
        .flatten()
        .map(|s| s.interval_secs as u64)
        .unwrap_or(DEFAULT_INTERVAL_SECS as u64)
}

/// HB8 — the honest transport check for this machine's real job.
pub fn transport_state() -> TransportReport {
    state_with(
        &SystemRunner,
        Platform::current(),
        &home_dir(),
        fallback_interval_secs(),
        &current_evidence(),
    )
}

/// Old boolean, kept for old callers: `transport_state() == ok`.
pub fn transport_installed() -> bool {
    transport_state().state == TransportState::Ok
}

// ── HB11 — one installer, from saved settings ──────────────────────────

/// Result of an install / reinstall.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InstallOutcome {
    pub changed: bool,
    pub message: String,
}

/// Write `<home>/.k2/heartbeat.sh` when it differs. Returns whether it
/// changed.
fn write_script_if_changed(home: &Path) -> Result<bool, String> {
    let k2_dir = home.join(".k2");
    fs::create_dir_all(&k2_dir).map_err(|e| format!("create {}: {e}", k2_dir.display()))?;
    // Retired pre-P5.6 artifact.
    let _ = fs::remove_file(k2_dir.join("heartbeat-projects.txt"));
    let path = script_path(home);
    let want = heartbeat_script_for(home);
    if fs::read_to_string(&path).ok().as_deref() == Some(want.as_str()) {
        return Ok(false);
    }
    fs::write(&path, &want).map_err(|e| format!("write heartbeat.sh: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod heartbeat.sh: {e}"))?;
    }
    Ok(true)
}

/// Install (or reload) the job for `spec` under `home`, through
/// `runner`. With `force_reload` the job is booted out and bootstrapped
/// even when it already looks right (the HB10 repair).
///
/// Callers that act on the REAL session must call
/// [`transport_writes_allowed`] first; tests pass a fake runner and a
/// temp `home`.
pub(crate) fn install_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    spec: Option<JobSpec>,
    force_reload: bool,
) -> Result<InstallOutcome, String> {
    let Some(spec) = spec else {
        return Ok(InstallOutcome {
            changed: false,
            message: "wake scheduler mode wants no OS job".to_string(),
        });
    };
    let script_changed = write_script_if_changed(home)?;
    let mut out = match platform {
        Platform::Launchd => install_launchd_with(runner, home, spec, force_reload)?,
        Platform::Cron => install_cron_with(runner, home, force_reload)?,
        Platform::Unsupported => InstallOutcome {
            changed: false,
            message: "no OS scheduler on this platform; heartbeat.sh written".to_string(),
        },
    };
    out.changed |= script_changed;
    Ok(out)
}

fn install_launchd_with(
    runner: &dyn CommandRunner,
    home: &Path,
    spec: JobSpec,
    force_reload: bool,
) -> Result<InstallOutcome, String> {
    let plist = plist_path(home);
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create LaunchAgents dir: {e}"))?;
    }
    let want = plist_for_spec(home, spec);
    let plist_changed = fs::read_to_string(&plist).ok().as_deref() != Some(want.as_str());

    let target = launchd_target();
    let print = runner
        .run("launchctl", &["print", &target], None)
        .map_err(|e| format!("launchctl print: {e}"))?;
    let loaded = print.success;
    let loaded_is_ours = loaded && {
        let job = parse_launchctl_print(&print.stdout);
        job.path.as_deref() == Some(plist.to_string_lossy().as_ref())
            && job.arguments
                == vec![
                    "/bin/bash".to_string(),
                    script_path(home).to_string_lossy().into_owned(),
                ]
    };
    if !plist_changed && loaded_is_ours && !force_reload {
        return Ok(InstallOutcome {
            changed: false,
            message: format!("{} Already up to date.", describe(spec)),
        });
    }

    if plist_changed {
        fs::write(&plist, &want).map_err(|e| format!("write plist: {e}"))?;
    }
    // bootout / bootstrap everywhere — the old unload/load path is gone.
    if loaded {
        let out = runner
            .run("launchctl", &["bootout", &target], None)
            .map_err(|e| format!("launchctl bootout: {e}"))?;
        if !out.success {
            crate::log_debug!(
                "[heartbeat-transport] launchctl bootout {target} failed: {}",
                out.stderr.trim()
            );
        }
    }
    let domain = launchd_domain();
    let plist_s = plist.to_string_lossy().into_owned();
    let mut last_err = String::new();
    for attempt in 0..3 {
        let out = runner
            .run("launchctl", &["bootstrap", &domain, &plist_s], None)
            .map_err(|e| format!("launchctl bootstrap: {e}"))?;
        if out.success {
            return Ok(InstallOutcome {
                changed: true,
                message: describe(spec),
            });
        }
        last_err = out.stderr.trim().to_string();
        // A bootout can take a moment to settle before the label is free.
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
    Err(format!("launchctl bootstrap {domain} {plist_s} failed: {last_err}"))
}

fn install_cron_with(
    runner: &dyn CommandRunner,
    home: &Path,
    force_reload: bool,
) -> Result<InstallOutcome, String> {
    let entry = cron_entry(home);
    let existing = match runner.run("crontab", &["-l"], None) {
        Ok(o) if o.success => o.stdout,
        Ok(_) => String::new(),
        Err(e) => return Err(format!("crontab is not available: {e}")),
    };
    let ours: Vec<&str> = existing.lines().filter(|l| l.contains(CRON_MARKER)).collect();
    if !force_reload && ours.len() == 1 && ours[0] == entry {
        return Ok(InstallOutcome {
            changed: false,
            message: "heartbeat crontab entry already installed.".to_string(),
        });
    }
    let mut lines: Vec<&str> = existing.lines().filter(|l| !l.contains(CRON_MARKER)).collect();
    lines.push(&entry);
    crontab_write(runner, &(lines.join("\n") + "\n"))?;
    Ok(InstallOutcome {
        changed: true,
        message: "heartbeat crontab entry installed.".to_string(),
    })
}

/// Remove the job and the script under `home`.
pub(crate) fn uninstall_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
) -> Result<(), String> {
    match platform {
        Platform::Launchd => {
            let target = launchd_target();
            let print = runner
                .run("launchctl", &["print", &target], None)
                .map_err(|e| format!("launchctl print: {e}"))?;
            if print.success {
                let out = runner
                    .run("launchctl", &["bootout", &target], None)
                    .map_err(|e| format!("launchctl bootout: {e}"))?;
                if !out.success {
                    return Err(format!("launchctl bootout failed: {}", out.stderr.trim()));
                }
            }
            let plist = plist_path(home);
            if plist.exists() {
                fs::remove_file(&plist).map_err(|e| format!("remove plist: {e}"))?;
            }
        }
        Platform::Cron => {
            let existing = crontab_read(runner);
            if existing.lines().any(|l| l.contains(CRON_MARKER)) {
                let kept: Vec<&str> =
                    existing.lines().filter(|l| !l.contains(CRON_MARKER)).collect();
                crontab_write(runner, &(kept.join("\n") + "\n"))?;
            }
        }
        Platform::Unsupported => {}
    }
    let _ = fs::remove_file(script_path(home));
    let _ = fs::remove_file(home.join(".k2/heartbeat-projects.txt"));
    Ok(())
}

/// HB11 — THE installer. Add, self-heal, Settings Apply, the
/// install-launchd route and boot all land here. Reads
/// `app_settings.wake_scheduler`; never invents an interval or a wake
/// value of its own.
pub fn install_from_saved_settings() -> Result<InstallOutcome, String> {
    transport_writes_allowed()?;
    let spec = saved_job_spec()?;
    let out = install_with(&SystemRunner, Platform::current(), &home_dir(), spec, false)?;
    if out.changed {
        reset_observation_window();
    }
    Ok(out)
}

/// Called by `heartbeat add`. Same as [`install_from_saved_settings`];
/// returns whether anything changed.
pub fn ensure_cron_installed() -> Result<bool, String> {
    install_from_saved_settings().map(|o| o.changed)
}

/// Settings → Apply (and boot after a label migration). Settings are
/// saved to the daemon before Apply is called, so this reads them back
/// rather than trusting the request body.
pub fn apply_wake_scheduler() -> Result<String, String> {
    transport_writes_allowed()?;
    let mode = crate::app_settings::load().wake_scheduler.mode;
    let out = install_from_saved_settings()?;
    let note = match mode.as_str() {
        "heartbeat" => "",
        _ => " Heartbeats fire while this computer is awake; it is not woken from sleep.",
    };
    Ok(format!("wake scheduler '{mode}': {}{note}", out.message))
}

/// Remove the job and `heartbeat.sh`. Idempotent.
pub fn uninstall_heartbeat_scheduler() -> Result<(), String> {
    transport_writes_allowed()?;
    uninstall_with(&SystemRunner, Platform::current(), &home_dir())
}

// ── HB10 — repair, not trust ───────────────────────────────────────────

/// One self-check that found the job not `ok`. Persisted as JSON in
/// `scheduler_meta.last_transport_repair` and surfaced by
/// `/cli/heartbeat/scheduler-status`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairRecord {
    /// RFC3339 UTC.
    pub at: String,
    /// `boot`, `wake` or `periodic`.
    pub context: String,
    pub before: TransportState,
    pub before_detail: String,
    /// `repaired`, `failed`, or `refused` (the HB6 guard said no).
    pub action: String,
    pub after: Option<TransportState>,
    pub detail: String,
}

/// Repair `before` through `runner`: boot out the label and bootstrap
/// `home`'s real plist for `spec`. `None` when nothing needs doing (the
/// job is `ok`, the platform has no job, or the mode wants no job).
///
/// Callers acting on the real session must pass the HB6 guard first.
pub(crate) fn repair_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    spec: Option<JobSpec>,
    before: &TransportReport,
    context: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<RepairRecord> {
    if matches!(before.state, TransportState::Ok | TransportState::Unsupported) {
        return None;
    }
    let spec = spec?;
    let mut rec = RepairRecord {
        at: now.to_rfc3339(),
        context: context.to_string(),
        before: before.state,
        before_detail: before.detail.clone(),
        action: "failed".to_string(),
        after: None,
        detail: String::new(),
    };
    match install_with(runner, platform, home, Some(spec), true) {
        Ok(out) => {
            // A just-reloaded job cannot have ticked yet: judge it on a
            // fresh watch window.
            let ev = TickEvidence { now, observed_since: now, last_os_tick: None };
            let after = state_with(runner, platform, home, spec.interval_secs as u64, &ev);
            rec.after = Some(after.state);
            if after.state == TransportState::Ok {
                rec.action = "repaired".to_string();
                rec.detail = out.message;
            } else {
                rec.detail = format!("{} — still {:?}: {}", out.message, after.state, after.detail);
            }
        }
        Err(e) => rec.detail = e,
    }
    Some(rec)
}

/// HB10 — check this machine's job and repair it when it is not `ok`.
/// Run by the daemon monitor at boot, after a wake, and every 10
/// minutes. Returns the check and, when one was attempted (or refused),
/// the repair record, which is also logged and saved to
/// `scheduler_meta.last_transport_repair`.
pub fn self_check_and_repair(context: &str) -> (TransportReport, Option<RepairRecord>) {
    let before = transport_state();
    if matches!(before.state, TransportState::Ok | TransportState::Unsupported) {
        return (before, None);
    }
    let now = chrono::Utc::now();
    let refused = |action: &str, detail: String| RepairRecord {
        at: now.to_rfc3339(),
        context: context.to_string(),
        before: before.state,
        before_detail: before.detail.clone(),
        action: action.to_string(),
        after: None,
        detail,
    };
    let record = if let Err(e) = transport_writes_allowed() {
        Some(refused("refused", e))
    } else {
        match saved_job_spec() {
            Err(e) => Some(refused("failed", e)),
            Ok(spec) => {
                let rec = repair_with(
                    &SystemRunner,
                    Platform::current(),
                    &home_dir(),
                    spec,
                    &before,
                    context,
                    now,
                );
                if rec.as_ref().is_some_and(|r| r.action == "repaired") {
                    reset_observation_window();
                }
                rec
            }
        }
    };
    if let Some(rec) = &record {
        crate::log_debug!(
            "[heartbeat-transport] self-check ({context}): {:?} ({}) → {} {}",
            rec.before,
            rec.before_detail,
            rec.action,
            rec.detail
        );
        match serde_json::to_string(rec) {
            Ok(json) => {
                use crate::db::schema::SchedulerMeta;
                let db = crate::db::shared();
                let conn = db.lock();
                if let Err(e) = SchedulerMeta::set(&conn, SchedulerMeta::LAST_TRANSPORT_REPAIR, &json)
                {
                    crate::log_debug!("[heartbeat-transport] persist last_transport_repair: {e}");
                }
            }
            Err(e) => crate::log_debug!("[heartbeat-transport] serialise repair record: {e}"),
        }
    }
    (before, record)
}

/// The last saved [`RepairRecord`], for the status route.
pub fn last_transport_repair() -> Option<serde_json::Value> {
    use crate::db::schema::SchedulerMeta;
    let db = crate::db::shared();
    let conn = db.lock();
    SchedulerMeta::get(&conn, SchedulerMeta::LAST_TRANSPORT_REPAIR)
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Bash script written to `~/.k2/heartbeat.sh` for this HOME.
pub fn generate_heartbeat_script() -> String {
    heartbeat_script_for(&home_dir())
}

/// Bash script written to `<home>/.k2/heartbeat.sh`. Asks the daemon
/// for active projects on every tick and forwards each to
/// `/cli/scheduler-tick`. P5.6 retired the `heartbeat-projects.txt`
/// dependency — the DB is the only source of truth for which
/// workspaces have heartbeats.
pub fn heartbeat_script_for(home: &Path) -> String {
    let home = home.to_string_lossy().to_string();

    format!(r##"#!/bin/bash
# K2SO Agent Heartbeat — DO NOT EDIT (managed by K2SO daemon)
# Asks the daemon which projects have active heartbeats, then ticks each.

PORT_FILE="{home}/.k2/heartbeat.port"
LOG_FILE="{home}/.k2/heartbeat.log"
TOKEN_FILE="{home}/.k2/heartbeat.token"

ts() {{ date '+%Y-%m-%d %H:%M:%S'; }}

urlencode() {{
    local string="$1" length="${{#1}}" i c
    local encoded=""
    for (( i = 0; i < length; i++ )); do
        c="${{string:i:1}}"
        case "$c" in
            [a-zA-Z0-9._~-]) encoded+="$c" ;;
            *) encoded+=$(printf '%%%02X' "'$c") ;;
        esac
    done
    printf '%s' "$encoded"
}}

if [ ! -f "$PORT_FILE" ]; then
    exit 0
fi
PORT=$(cat "$PORT_FILE" 2>/dev/null)
if [ -z "$PORT" ] || ! [[ "$PORT" =~ ^[0-9]+$ ]]; then
    exit 0
fi

HEALTH=$(curl -s --connect-timeout 2 "http://127.0.0.1:$PORT/health" 2>/dev/null)
if ! echo "$HEALTH" | grep -q '"ok"'; then
    exit 0
fi

TOKEN=""
if [ -f "$TOKEN_FILE" ]; then
    TOKEN=$(cat "$TOKEN_FILE" 2>/dev/null)
fi

if [ -z "$TOKEN" ]; then
    echo "$(ts) ERROR: No auth token available — skipping heartbeat" >> "$LOG_FILE"
    exit 0
fi

# Ask the daemon for the current list of projects with active heartbeats.
PROJECTS=$(curl -s --connect-timeout 2 --max-time 5 \
    "http://127.0.0.1:$PORT/cli/heartbeat/active-projects?token=$TOKEN" 2>>"$LOG_FILE")
if [ -z "$PROJECTS" ]; then
    if [ -f "$LOG_FILE" ]; then
        tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
    fi
    exit 0
fi

while IFS= read -r project_path; do
    [ -z "$project_path" ] && continue
    ENCODED_PATH=$(urlencode "$project_path")
    RESULT=$(curl -sG "http://127.0.0.1:$PORT/cli/scheduler-tick?token=$TOKEN&project=$ENCODED_PATH" --connect-timeout 5 --max-time 30 2>>"$LOG_FILE")
    CURL_EXIT=$?
    if [ "$CURL_EXIT" -ne 0 ]; then
        echo "$(ts) ERROR curl exit=$CURL_EXIT project=$project_path" >> "$LOG_FILE"
        continue
    fi
    COUNT=$(echo "$RESULT" | grep -o '"count":[0-9]*' | grep -o '[0-9]*' | head -1 || echo 0)
    SKIPPED=$(echo "$RESULT" | grep -o '"skipped":"[^"]*"' | sed 's/"skipped":"\([^"]*\)"/\1/')
    if [ -n "$SKIPPED" ]; then
        echo "$(ts) tick project=$project_path skipped=$SKIPPED" >> "$LOG_FILE"
    elif [ -n "$COUNT" ] && [ "$COUNT" -gt 0 ] 2>/dev/null; then
        echo "$(ts) tick project=$project_path launched=$COUNT" >> "$LOG_FILE"
    else
        echo "$(ts) tick project=$project_path launched=0" >> "$LOG_FILE"
    fi
done <<< "$PROJECTS"

if [ -f "$LOG_FILE" ]; then
    tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
fi
"##, home = home)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Test scaffolding: run `f` with `$HOME` pointed at a fresh temp folder,
/// holding the crate-wide `themes::HOME_LOCK` so no other HOME-mutating
/// test races it. Restores HOME and removes the folder afterwards.
#[cfg(test)]
pub(crate) fn with_temp_home<R>(label: &str, f: impl FnOnce(&Path) -> R) -> R {
    let _lock = crate::themes::HOME_LOCK.lock();
    let home = std::env::temp_dir().join(format!(
        "k2-hb-transport-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&home).expect("create temp HOME");
    let prev = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&home)));
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(&home);
    match out {
        Ok(v) => v,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod transport_guard_tests {
    use super::*;

    fn assert_no_scheduler_files(home: &Path) {
        let plist = home.join("Library/LaunchAgents/dev.k2.heartbeat.plist");
        let script = home.join(".k2/heartbeat.sh");
        assert!(!plist.exists(), "guard let a plist be written at {}", plist.display());
        assert!(!script.exists(), "guard let heartbeat.sh be written at {}", script.display());
    }

    #[test]
    fn pure_guard_refuses_flag_foreign_home_and_test_builds() {
        let real = Path::new("/Users/someone");
        let tmp = Path::new("/private/var/folders/xx/T/k2so-tunnel-test-1-2");

        let e = writes_allowed_for(Some("1"), Some(real), Some(real), false).unwrap_err();
        assert!(e.contains("K2_HEARTBEAT_NO_SELF_HEAL=1"), "{e}");

        let e = writes_allowed_for(None, Some(tmp), Some(real), false).unwrap_err();
        assert!(e.contains("HOME is not the user home"), "{e}");

        let e = writes_allowed_for(None, Some(real), Some(real), true).unwrap_err();
        assert!(e.contains("cfg(test)"), "{e}");

        // The only allowed shape: no flag, HOME == user home, not a test build.
        writes_allowed_for(None, Some(real), Some(real), false).expect("real session allowed");
        writes_allowed_for(Some("0"), Some(real), Some(real), false).expect("flag=0 allowed");
        writes_allowed_for(None, Some(Path::new("/Users/someone/")), Some(real), false)
            .expect("trailing slash is the same home");
    }

    #[cfg(unix)]
    #[test]
    fn password_home_resolves_on_unix() {
        let pw = password_home().expect("getpwuid_r must resolve this uid's home");
        assert!(pw.is_absolute(), "password home not absolute: {}", pw.display());
    }

    /// T1 — the 2026-09-30 hijack path. Under a temp HOME, a real
    /// `k2so_heartbeat_add` runs no launchctl/crontab and writes nothing
    /// under that HOME; the installer itself returns the HOME refusal.
    #[test]
    fn heartbeat_add_under_temp_home_never_reaches_the_scheduler() {
        crate::db::init_for_tests();
        let project = std::env::temp_dir().join(format!(
            "k2-hb-guard-add-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&project).unwrap();
        let path = project.to_string_lossy().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'hb-guard', ?2)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), path],
            )
            .unwrap();
        }
        with_temp_home("add", |home| {
            test_recorder::take();
            crate::heartbeats::k2so_heartbeat_add(
                path.clone(),
                "guarded".into(),
                "daily".into(),
                "{}".into(),
            )
            .expect("add succeeds without a scheduler");
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_no_scheduler_files(home);

            let e = ensure_cron_installed().unwrap_err();
            #[cfg(unix)]
            assert!(e.contains("HOME is not the user home"), "{e}");
            #[cfg(not(unix))]
            assert!(e.contains("cfg(test)"), "{e}");
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_no_scheduler_files(home);
        });
        let _ = fs::remove_dir_all(&project);
    }

    /// Every public writer refuses under a temp HOME — none returns Ok.
    #[test]
    fn every_public_writer_refuses_under_temp_home() {
        with_temp_home("writers", |home| {
            test_recorder::take();
            let results: Vec<(&str, Result<(), String>)> = vec![
                ("ensure_cron_installed", ensure_cron_installed().map(|_| ())),
                ("install_from_saved_settings", install_from_saved_settings().map(|_| ())),
                ("apply_wake_scheduler", apply_wake_scheduler().map(|_| ())),
                ("uninstall_heartbeat_scheduler", uninstall_heartbeat_scheduler()),
            ];
            for (name, r) in results {
                let e = r.expect_err(&format!("{name} must refuse under a temp HOME"));
                assert!(e.contains("refused"), "{name}: {e}");
            }
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_no_scheduler_files(home);
        });
    }

    /// The opt-out flag refuses even with the real HOME.
    #[test]
    fn opt_out_flag_refuses_apply() {
        let _lock = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os(NO_SELF_HEAL_ENV);
        std::env::set_var(NO_SELF_HEAL_ENV, "1");
        test_recorder::take();
        let r = apply_wake_scheduler();
        match prev {
            Some(v) => std::env::set_var(NO_SELF_HEAL_ENV, v),
            None => std::env::remove_var(NO_SELF_HEAL_ENV),
        }
        let e = r.expect_err("flag must refuse");
        assert!(e.contains("K2_HEARTBEAT_NO_SELF_HEAL=1"), "{e}");
        assert_eq!(test_recorder::take(), Vec::<String>::new());
    }

    /// With the real HOME and no flag, a cfg(test) build still refuses.
    #[test]
    fn real_home_still_refused_in_test_builds() {
        let _lock = crate::themes::HOME_LOCK.lock();
        test_recorder::take();
        let e = ensure_cron_installed().expect_err("cfg(test) must refuse");
        assert!(e.contains("refused"), "{e}");
        assert_eq!(test_recorder::take(), Vec::<String>::new());
    }

    /// The seam itself: under cfg(test) the system runner records and
    /// runs nothing.
    #[test]
    fn system_runner_records_and_runs_nothing_under_test() {
        test_recorder::take();
        let e = SystemRunner
            .run("launchctl", &["print", "gui/0/dev.k2.heartbeat"], None)
            .expect_err("cfg(test) runner must not run");
        assert!(e.contains("never runs"), "{e}");
        assert_eq!(
            test_recorder::take(),
            vec!["launchctl print gui/0/dev.k2.heartbeat".to_string()]
        );
    }
}

/// A scripted `launchctl` / `crontab`: records every call and answers
/// from fixed text. Never runs anything.
#[cfg(test)]
pub(crate) mod fake_runner {
    use super::*;
    use std::cell::RefCell;

    pub(crate) struct FakeRunner {
        /// `launchctl print` stdout; `None` = label not loaded.
        pub print: RefCell<Option<String>>,
        /// What `print` becomes after a successful bootstrap.
        pub print_after_bootstrap: Option<String>,
        pub bootstrap_ok: bool,
        pub bootstrap_stderr: String,
        /// `crontab -l` stdout; `None` = no crontab.
        pub crontab: RefCell<Option<String>>,
        pub crontab_missing: bool,
        pub calls: RefCell<Vec<String>>,
    }

    impl FakeRunner {
        pub(crate) fn launchd(print: Option<String>, after: Option<String>) -> Self {
            Self {
                print: RefCell::new(print),
                print_after_bootstrap: after,
                bootstrap_ok: true,
                bootstrap_stderr: String::new(),
                crontab: RefCell::new(None),
                crontab_missing: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        pub(crate) fn cron(crontab: Option<String>) -> Self {
            let r = Self::launchd(None, None);
            *r.crontab.borrow_mut() = crontab;
            r
        }

        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        /// Calls other than the read-only `print` / `crontab -l`.
        pub(crate) fn writes(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|c| !c.starts_with("launchctl print") && c != "crontab -l")
                .collect()
        }
    }

    fn ok(stdout: &str) -> CmdOutput {
        CmdOutput { success: true, stdout: stdout.to_string(), stderr: String::new() }
    }

    fn fail(stderr: &str) -> CmdOutput {
        CmdOutput { success: false, stdout: String::new(), stderr: stderr.to_string() }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput, String> {
            self.calls.borrow_mut().push(format!("{program} {}", args.join(" ")));
            match (program, args.first().copied()) {
                ("launchctl", Some("print")) => Ok(match &*self.print.borrow() {
                    Some(t) => ok(t),
                    None => fail("Could not find service \"dev.k2.heartbeat\" in domain"),
                }),
                ("launchctl", Some("bootout")) => {
                    *self.print.borrow_mut() = None;
                    Ok(ok(""))
                }
                ("launchctl", Some("bootstrap")) => {
                    if self.bootstrap_ok {
                        *self.print.borrow_mut() = self.print_after_bootstrap.clone();
                        Ok(ok(""))
                    } else {
                        Ok(fail(&self.bootstrap_stderr))
                    }
                }
                ("crontab", _) if self.crontab_missing => {
                    Err("run crontab: No such file or directory".to_string())
                }
                ("crontab", Some("-l")) => Ok(match &*self.crontab.borrow() {
                    Some(t) => ok(t),
                    None => fail("no crontab for user"),
                }),
                ("crontab", Some("-")) => {
                    *self.crontab.borrow_mut() = stdin.map(str::to_string);
                    Ok(ok(""))
                }
                _ => Err(format!("FakeRunner: unexpected {program} {args:?}")),
            }
        }
    }
}

#[cfg(test)]
mod transport_state_tests {
    use super::fake_runner::FakeRunner;
    use super::*;
    use chrono::{Duration, Utc};

    const FOREIGN: &str = include_str!("fixtures/launchctl-print-foreign.txt");
    const OK: &str = include_str!("fixtures/launchctl-print-ok.txt");
    const NEVER: &str = include_str!("fixtures/launchctl-print-never-exited.txt");
    const FIXTURE_HOME: &str = "/Users/k2fixture";
    const TEMP_HOME: &str = "/private/var/folders/9x/abc123/T/k2so-tunnel-test-96642-1790837486533357000";
    const TEMP_HOME_SHORT: &str = "/var/folders/9x/abc123/T/k2so-tunnel-test-96642-1790837486533357000";

    /// Watching just started: `silent` cannot apply yet.
    fn fresh() -> TickEvidence {
        let now = Utc::now();
        TickEvidence { now, observed_since: now - Duration::seconds(30), last_os_tick: None }
    }

    fn home() -> PathBuf {
        PathBuf::from(FIXTURE_HOME)
    }

    #[test]
    fn parses_the_fields_the_check_needs() {
        let job = parse_launchctl_print(FOREIGN);
        assert_eq!(
            job.path.as_deref(),
            Some(format!("{TEMP_HOME}/Library/LaunchAgents/dev.k2.heartbeat.plist").as_str())
        );
        assert_eq!(job.program.as_deref(), Some("/bin/bash"));
        assert_eq!(
            job.arguments,
            vec!["/bin/bash".to_string(), format!("{TEMP_HOME_SHORT}/.k2/heartbeat.sh")]
        );
        assert_eq!(job.last_exit, LastExit::Code(127));
        assert_eq!(job.run_interval_secs, Some(60));

        let ok = parse_launchctl_print(OK);
        assert_eq!(ok.last_exit, LastExit::Code(0));
        assert_eq!(ok.run_interval_secs, Some(300));
        assert_eq!(parse_launchctl_print(NEVER).last_exit, LastExit::NeverExited);
    }

    /// T2 — the hijacked job from 2026-09-30 is `foreign`.
    #[test]
    fn temp_home_job_is_foreign() {
        let r = classify_launchd(Some(FOREIGN), &home(), true, 60, &fresh());
        assert_eq!(r.state, TransportState::Foreign, "{}", r.detail);
        assert_eq!(r.last_exit_code, Some(127));
        assert!(r.plist_path.as_deref().unwrap_or("").contains("k2so-tunnel-test"));
    }

    /// T2 — the same job with the real paths is `failing` (exit 127).
    #[test]
    fn real_paths_with_nonzero_exit_are_failing() {
        let text = FOREIGN.replace(TEMP_HOME, FIXTURE_HOME).replace(TEMP_HOME_SHORT, FIXTURE_HOME);
        let r = classify_launchd(Some(&text), &home(), true, 60, &fresh());
        assert_eq!(r.state, TransportState::Failing, "{}", r.detail);
        assert_eq!(r.last_exit_code, Some(127));
        assert_eq!(
            r.plist_path.as_deref(),
            Some("/Users/k2fixture/Library/LaunchAgents/dev.k2.heartbeat.plist")
        );
        assert_eq!(r.program_path.as_deref(), Some("/Users/k2fixture/.k2/heartbeat.sh"));
    }

    /// T2 — a clean job and a never-run job are `ok`.
    #[test]
    fn clean_and_never_exited_are_ok() {
        let r = classify_launchd(Some(OK), &home(), true, 60, &fresh());
        assert_eq!(r.state, TransportState::Ok, "{}", r.detail);
        assert_eq!(r.interval_secs, Some(300));
        let r = classify_launchd(Some(NEVER), &home(), true, 60, &fresh());
        assert_eq!(r.state, TransportState::Ok, "{}", r.detail);
        assert_eq!(r.last_exit_code, None);
    }

    #[test]
    fn not_loaded_or_plist_gone_is_missing() {
        assert_eq!(
            classify_launchd(None, &home(), true, 60, &fresh()).state,
            TransportState::Missing
        );
        assert_eq!(
            classify_launchd(Some(OK), &home(), false, 60, &fresh()).state,
            TransportState::Missing
        );
    }

    /// No OS tick within three intervals, once watched longer than three
    /// intervals, is `silent`. The OK fixture ticks every 300 s.
    #[test]
    fn stale_os_tick_is_silent() {
        let now = Utc::now();
        let stale = TickEvidence {
            now,
            observed_since: now - Duration::minutes(20),
            last_os_tick: Some(now - Duration::minutes(16)),
        };
        let r = classify_launchd(Some(OK), &home(), true, 60, &stale);
        assert_eq!(r.state, TransportState::Silent, "{}", r.detail);

        let never = TickEvidence { last_os_tick: None, ..stale };
        assert_eq!(
            classify_launchd(Some(OK), &home(), true, 60, &never).state,
            TransportState::Silent
        );

        let recent = TickEvidence { last_os_tick: Some(now - Duration::minutes(5)), ..stale };
        assert_eq!(
            classify_launchd(Some(OK), &home(), true, 60, &recent).state,
            TransportState::Ok
        );

        // Watched for less than three intervals: too early to call silent.
        let young = TickEvidence { observed_since: now - Duration::minutes(10), ..never };
        assert_eq!(
            classify_launchd(Some(OK), &home(), true, 60, &young).state,
            TransportState::Ok
        );
    }

    #[test]
    fn cron_states() {
        let h = home();
        let line = format!("* * * * * {FIXTURE_HOME}/.k2/heartbeat.sh # {CRON_MARKER}");
        assert_eq!(
            classify_cron(Err("run crontab: not found".into()), &h, &fresh()).state,
            TransportState::Missing
        );
        assert_eq!(classify_cron(Ok(None), &h, &fresh()).state, TransportState::Missing);
        assert_eq!(
            classify_cron(Ok(Some("0 1 * * * backup\n".into())), &h, &fresh()).state,
            TransportState::Missing
        );
        let foreign = format!("* * * * * /tmp/x/.k2/heartbeat.sh # {CRON_MARKER}\n");
        assert_eq!(
            classify_cron(Ok(Some(foreign)), &h, &fresh()).state,
            TransportState::Foreign
        );
        let r = classify_cron(Ok(Some(format!("0 1 * * * backup\n{line}\n"))), &h, &fresh());
        assert_eq!(r.state, TransportState::Ok, "{}", r.detail);
        assert_eq!(r.program_path.as_deref(), Some("/Users/k2fixture/.k2/heartbeat.sh"));
    }

    #[test]
    fn state_with_reads_only() {
        let runner = FakeRunner::launchd(Some(OK.to_string()), None);
        let r = state_with(&runner, Platform::Launchd, &home(), 60, &fresh());
        // /Users/k2fixture's plist does not exist on this machine.
        assert_eq!(r.state, TransportState::Missing, "{}", r.detail);
        assert_eq!(runner.writes(), Vec::<String>::new());

        let runner = FakeRunner::cron(None);
        let r = state_with(&runner, Platform::Cron, &home(), 60, &fresh());
        assert_eq!(r.state, TransportState::Missing);
        assert_eq!(runner.calls(), vec!["crontab -l".to_string()]);

        let runner = FakeRunner::launchd(None, None);
        let r = state_with(&runner, Platform::Unsupported, &home(), 60, &fresh());
        assert_eq!(r.state, TransportState::Unsupported);
        assert_eq!(runner.calls(), Vec::<String>::new());
    }

    #[test]
    fn transport_state_serialises_for_the_status_route() {
        let r = classify_launchd(Some(FOREIGN), &home(), true, 60, &fresh());
        let v = serde_json::to_value(&r).expect("serialise");
        assert_eq!(v["state"], "foreign");
        assert_eq!(v["lastExitCode"], 127);
        assert!(v["plistPath"].as_str().expect("plistPath").ends_with("dev.k2.heartbeat.plist"));
        assert!(v["programPath"].as_str().expect("programPath").ends_with(".k2/heartbeat.sh"));
    }
}

#[cfg(test)]
mod installer_tests {
    use super::fake_runner::FakeRunner;
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-hb-installer-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// `launchctl print` text for a job loaded from `home`'s real plist.
    fn print_for(home: &Path, exit: &str) -> String {
        include_str!("fixtures/launchctl-print-ok.txt")
            .replace("/Users/k2fixture", &home.to_string_lossy())
            .replace("last exit code = 0", &format!("last exit code = {exit}"))
    }

    fn ws(mode: &str, minutes: u32, wake: bool) -> crate::app_settings::WakeSchedulerSettings {
        crate::app_settings::WakeSchedulerSettings {
            mode: mode.to_string(),
            interval_minutes: minutes,
            wake_system: wake,
        }
    }

    #[test]
    fn job_spec_per_mode() {
        assert_eq!(
            job_spec_for(&ws("heartbeat", 5, true)).unwrap(),
            Some(JobSpec { interval_secs: 300, wake_system: true })
        );
        // on_demand keeps today's de facto 60 s no-wake job (HB11).
        assert_eq!(
            job_spec_for(&ws("on_demand", 5, true)).unwrap(),
            Some(JobSpec { interval_secs: 60, wake_system: false })
        );
        // Off = never wake the machine, still fire while awake (D2).
        assert_eq!(
            job_spec_for(&ws("off", 5, true)).unwrap(),
            Some(JobSpec { interval_secs: 60, wake_system: false })
        );
        let e = job_spec_for(&ws("bogus", 5, false)).unwrap_err();
        assert!(e.contains("unknown wake scheduler mode 'bogus'"), "{e}");
    }

    /// T-S1c — heartbeat mode, 5 min, Wake on → 300 + WakeSystem, and the
    /// installer writes exactly those bytes.
    #[test]
    fn plist_follows_saved_settings_and_install_writes_the_same_bytes() {
        let home = temp_dir("plist");
        let spec = job_spec_for(&ws("heartbeat", 5, true)).unwrap();
        let want = plist_for_spec(&home, spec.expect("a job"));
        assert!(want.contains("<integer>300</integer>"), "{want}");
        assert!(want.contains("<key>WakeSystem</key>"), "{want}");
        assert!(want.contains(&format!("{}/.k2/heartbeat.sh", home.display())));

        let runner = FakeRunner::launchd(None, Some(print_for(&home, "(never exited)")));
        let out = install_with(&runner, Platform::Launchd, &home, spec, false).expect("install");
        assert!(out.changed);
        assert!(out.message.contains("every 5 min") && out.message.contains("wakes system"));
        assert_eq!(fs::read_to_string(plist_path(&home)).unwrap(), want);
        assert_eq!(
            fs::read_to_string(script_path(&home)).unwrap(),
            heartbeat_script_for(&home)
        );
        assert_eq!(
            runner.writes(),
            vec![format!(
                "launchctl bootstrap {} {}",
                launchd_domain(),
                plist_path(&home).display()
            )]
        );

        // Same settings again: nothing to do, no launchctl writes.
        let runner = FakeRunner::launchd(Some(print_for(&home, "0")), None);
        let out = install_with(&runner, Platform::Launchd, &home, spec, false).expect("reinstall");
        assert!(!out.changed, "{}", out.message);
        assert_eq!(runner.writes(), Vec::<String>::new());
        assert_eq!(fs::read_to_string(plist_path(&home)).unwrap(), want);
        let _ = fs::remove_dir_all(&home);
    }

    /// HB11 — what `heartbeat add` does (minus the guard, which refuses
    /// in tests): read the SAVED settings and install them. A user's
    /// 5 min + Wake-on job already in place is left alone — no rewrite
    /// to the 60 s default, no launchctl writes. Pre-S1 the add path
    /// hard-coded 60 s / no wake and reloaded the job.
    #[test]
    fn add_keeps_the_saved_interval_and_wake_setting() {
        with_temp_home("saved-spec", |home| {
            let mut settings = crate::app_settings::load();
            settings.wake_scheduler = ws("heartbeat", 5, true);
            crate::app_settings::save(&settings).expect("save settings");
            let spec = saved_job_spec().expect("saved spec");
            assert_eq!(spec, Some(JobSpec { interval_secs: 300, wake_system: true }));

            // The user's job, as Settings → Apply left it.
            let user_plist = plist_for_spec(home, spec.unwrap());
            fs::create_dir_all(plist_path(home).parent().unwrap()).unwrap();
            fs::write(plist_path(home), &user_plist).unwrap();
            write_script_if_changed(home).unwrap();

            let runner = FakeRunner::launchd(Some(print_for(home, "0")), None);
            let out = install_with(&runner, Platform::Launchd, home, spec, false).expect("add");
            assert!(!out.changed, "{}", out.message);
            assert_eq!(runner.writes(), Vec::<String>::new());
            let after = fs::read_to_string(plist_path(home)).unwrap();
            assert_eq!(after, user_plist);
            assert!(after.contains("<integer>300</integer>") && after.contains("WakeSystem"));

            // The add itself, in this test build, never reaches the job.
            test_recorder::take();
            let e = ensure_cron_installed().expect_err("guard refuses in tests");
            assert!(e.contains("refused"), "{e}");
            assert_eq!(test_recorder::take(), Vec::<String>::new());
            assert_eq!(fs::read_to_string(plist_path(home)).unwrap(), user_plist);
        });
    }

    /// A job loaded from another plist is replaced: bootout, then
    /// bootstrap of THIS home's plist.
    #[test]
    fn foreign_job_is_booted_out_and_rebootstrapped() {
        let home = temp_dir("foreign");
        let spec = Some(JobSpec { interval_secs: 60, wake_system: false });
        let runner = FakeRunner::launchd(
            Some(include_str!("fixtures/launchctl-print-foreign.txt").to_string()),
            Some(print_for(&home, "(never exited)")),
        );
        let out = install_with(&runner, Platform::Launchd, &home, spec, false).expect("install");
        assert!(out.changed);
        assert_eq!(
            runner.writes(),
            vec![
                format!("launchctl bootout {}", launchd_target()),
                format!(
                    "launchctl bootstrap {} {}",
                    launchd_domain(),
                    plist_path(&home).display()
                ),
            ]
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn bootstrap_failure_is_an_error() {
        let home = temp_dir("bootfail");
        let mut runner = FakeRunner::launchd(None, None);
        runner.bootstrap_ok = false;
        runner.bootstrap_stderr = "Bootstrap failed: 5: Input/output error".into();
        let e = install_with(
            &runner,
            Platform::Launchd,
            &home,
            Some(JobSpec { interval_secs: 60, wake_system: false }),
            false,
        )
        .expect_err("bootstrap failure must be Err");
        assert!(e.contains("Input/output error"), "{e}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn cron_install_keeps_other_lines_and_replaces_ours() {
        let home = temp_dir("cron");
        let runner = FakeRunner::cron(Some(format!(
            "0 1 * * * backup\n* * * * * /tmp/old/.k2/heartbeat.sh # {CRON_MARKER}\n"
        )));
        let out = install_with(
            &runner,
            Platform::Cron,
            &home,
            Some(JobSpec { interval_secs: 60, wake_system: false }),
            false,
        )
        .expect("install");
        assert!(out.changed);
        let tab = runner.crontab.borrow().clone().expect("crontab written");
        assert_eq!(tab, format!("0 1 * * * backup\n{}\n", cron_entry(&home)));

        let mut missing = FakeRunner::cron(None);
        missing.crontab_missing = true;
        let e = install_with(
            &missing,
            Platform::Cron,
            &home,
            Some(JobSpec { interval_secs: 60, wake_system: false }),
            false,
        )
        .expect_err("no crontab binary must be Err");
        assert!(e.contains("crontab is not available"), "{e}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_uses_bootout_not_unload() {
        let home = temp_dir("uninstall");
        fs::create_dir_all(plist_path(&home).parent().unwrap()).unwrap();
        fs::write(plist_path(&home), "x").unwrap();
        let runner = FakeRunner::launchd(Some(print_for(&home, "0")), None);
        uninstall_with(&runner, Platform::Launchd, &home).expect("uninstall");
        assert_eq!(runner.writes(), vec![format!("launchctl bootout {}", launchd_target())]);
        assert!(!plist_path(&home).exists());
        let _ = fs::remove_dir_all(&home);
    }
}

/// HB10 — the self-check detects a wrong plist path, a nonzero exit or a
/// stale tick, and repairs it by bootout + bootstrap of the real plist.
/// Fake launchctl only.
#[cfg(test)]
mod repair_tests {
    use super::fake_runner::FakeRunner;
    use super::*;
    use chrono::{Duration, Utc};

    const SPEC: Option<JobSpec> = Some(JobSpec { interval_secs: 60, wake_system: false });

    fn temp_home(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-hb-repair-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(plist_path(&d).parent().unwrap()).unwrap();
        d
    }

    fn print_for(home: &Path, exit: &str) -> String {
        include_str!("fixtures/launchctl-print-ok.txt")
            .replace("/Users/k2fixture", &home.to_string_lossy())
            .replace("run interval = 300 seconds", "run interval = 60 seconds")
            .replace("last exit code = 0", &format!("last exit code = {exit}"))
    }

    fn fresh() -> TickEvidence {
        let now = Utc::now();
        TickEvidence { now, observed_since: now, last_os_tick: None }
    }

    fn expected_repair_writes(home: &Path) -> Vec<String> {
        vec![
            format!("launchctl bootout {}", launchd_target()),
            format!("launchctl bootstrap {} {}", launchd_domain(), plist_path(home).display()),
        ]
    }

    /// Run the check through the fake, then the repair; return the
    /// before-state and the record.
    fn check_and_repair(
        runner: &FakeRunner,
        home: &Path,
        ev: &TickEvidence,
    ) -> (TransportReport, Option<RepairRecord>) {
        let before = state_with(runner, Platform::Launchd, home, 60, ev);
        let rec = repair_with(runner, Platform::Launchd, home, SPEC, &before, "periodic", ev.now);
        (before, rec)
    }

    #[test]
    fn wrong_plist_path_is_repaired() {
        let home = temp_home("foreign");
        fs::write(plist_path(&home), plist_for_spec(&home, SPEC.unwrap())).unwrap();
        let runner = FakeRunner::launchd(
            Some(include_str!("fixtures/launchctl-print-foreign.txt").to_string()),
            Some(print_for(&home, "(never exited)")),
        );
        let (before, rec) = check_and_repair(&runner, &home, &fresh());
        assert_eq!(before.state, TransportState::Foreign, "{}", before.detail);
        let rec = rec.expect("a foreign job must be repaired");
        assert_eq!(rec.action, "repaired", "{}", rec.detail);
        assert_eq!(rec.after, Some(TransportState::Ok));
        assert_eq!(runner.writes(), expected_repair_writes(&home));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn nonzero_exit_is_repaired() {
        let home = temp_home("failing");
        fs::write(plist_path(&home), plist_for_spec(&home, SPEC.unwrap())).unwrap();
        let runner = FakeRunner::launchd(
            Some(print_for(&home, "127")),
            Some(print_for(&home, "(never exited)")),
        );
        let (before, rec) = check_and_repair(&runner, &home, &fresh());
        assert_eq!(before.state, TransportState::Failing, "{}", before.detail);
        let rec = rec.expect("a failing job must be repaired");
        assert_eq!(rec.action, "repaired", "{}", rec.detail);
        // Plist already correct: force reload anyway.
        assert_eq!(runner.writes(), expected_repair_writes(&home));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn stale_tick_is_repaired() {
        let home = temp_home("silent");
        fs::write(plist_path(&home), plist_for_spec(&home, SPEC.unwrap())).unwrap();
        let runner =
            FakeRunner::launchd(Some(print_for(&home, "0")), Some(print_for(&home, "0")));
        let now = Utc::now();
        let stale = TickEvidence {
            now,
            observed_since: now - Duration::minutes(30),
            last_os_tick: Some(now - Duration::minutes(10)),
        };
        let (before, rec) = check_and_repair(&runner, &home, &stale);
        assert_eq!(before.state, TransportState::Silent, "{}", before.detail);
        let rec = rec.expect("a silent job must be repaired");
        assert_eq!(rec.action, "repaired", "{}", rec.detail);
        assert_eq!(runner.writes(), expected_repair_writes(&home));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_job_is_bootstrapped_without_bootout() {
        let home = temp_home("missing");
        let runner = FakeRunner::launchd(None, Some(print_for(&home, "(never exited)")));
        let (before, rec) = check_and_repair(&runner, &home, &fresh());
        assert_eq!(before.state, TransportState::Missing);
        let rec = rec.expect("a missing job must be installed");
        assert_eq!(rec.action, "repaired", "{}", rec.detail);
        assert_eq!(
            runner.writes(),
            vec![format!(
                "launchctl bootstrap {} {}",
                launchd_domain(),
                plist_path(&home).display()
            )]
        );
        assert_eq!(
            fs::read_to_string(plist_path(&home)).unwrap(),
            plist_for_spec(&home, SPEC.unwrap())
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn ok_job_is_left_alone() {
        let home = temp_home("ok");
        fs::write(plist_path(&home), plist_for_spec(&home, SPEC.unwrap())).unwrap();
        let runner = FakeRunner::launchd(Some(print_for(&home, "0")), None);
        let (before, rec) = check_and_repair(&runner, &home, &fresh());
        assert_eq!(before.state, TransportState::Ok, "{}", before.detail);
        assert_eq!(rec, None);
        assert_eq!(runner.writes(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn failed_bootstrap_is_recorded_as_failed() {
        let home = temp_home("bootfail");
        let mut runner = FakeRunner::launchd(
            Some(include_str!("fixtures/launchctl-print-foreign.txt").to_string()),
            None,
        );
        runner.bootstrap_ok = false;
        runner.bootstrap_stderr = "Bootstrap failed: 5: Input/output error".into();
        let (_before, rec) = check_and_repair(&runner, &home, &fresh());
        let rec = rec.expect("a record");
        assert_eq!(rec.action, "failed");
        assert!(rec.detail.contains("Input/output error"), "{}", rec.detail);
        let _ = fs::remove_dir_all(&home);
    }

    /// The real entry point never repairs in a test build: it either
    /// finds nothing to do or records the guard's refusal. Either way
    /// the system runner is never asked to write.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn self_check_in_test_build_never_writes() {
        crate::db::init_for_tests();
        with_temp_home("selfcheck", |home| {
            test_recorder::take();
            let (before, rec) = self_check_and_repair("periodic");
            // cfg(test): `launchctl print` / `crontab -l` is refused, so
            // the job reads as missing.
            assert_eq!(before.state, TransportState::Missing, "{}", before.detail);
            let rec = rec.expect("a not-ok job gets a record");
            assert_eq!(rec.action, "refused");
            assert!(rec.detail.contains("HOME is not the user home"), "{}", rec.detail);
            let saved = last_transport_repair().expect("record persisted");
            assert_eq!(saved["action"], "refused");
            let calls = test_recorder::take();
            assert!(
                calls.iter().all(|c| c.starts_with("launchctl print") || c == "crontab -l"),
                "self-check ran a write: {calls:?}"
            );
            assert!(!plist_path(home).exists());
        });
    }
}
