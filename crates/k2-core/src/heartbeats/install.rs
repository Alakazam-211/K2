//! The retired OS heartbeat tick job, and the guard around touching it.
//!
//! History. Until heartbeat S2 the daemon did not tick itself: a macOS
//! LaunchAgent (`dev.k2.heartbeat`) or a Linux crontab line ran
//! `~/.k2/heartbeat.sh` every minute, which asked the daemon for active
//! projects and ticked each. On 2026-09-30 a test run left that job
//! loaded from a deleted temp folder and nothing fired for ~8h; a stock
//! Arch box has no `crontab` at all; and the plist's `WakeSystem` key is
//! not a launchd key, so it never woke anything.
//!
//! Heartbeat S2 (`prd-heartbeat-firing-v1.md` HB12, HB15; amendment W1):
//! the daemon's own 60 s loop (`k2-daemon/src/heartbeat_monitor.rs`)
//! drives every due scan, and waking the machine is the daemon's power
//! layer (`k2-daemon/src/power`). The OS job adds nothing, so this module
//! now only finds it and removes it — in every wake mode, on upgrade, at
//! boot and every 10 minutes:
//! - macOS: `launchctl bootout gui/<uid>/dev.k2.heartbeat`, then delete
//!   the plist and `~/.k2/heartbeat.sh`;
//! - Linux: drop the `k2so-agent-heartbeat` crontab line (D5), keeping
//!   every other line.
//!
//! The HTTP routes the old job called (`/cli/heartbeat/active-projects`,
//! `/cli/scheduler-tick`) stay, so an older daemon that re-installs its
//! job after a downgrade still works, and a leftover tick that slips in
//! before removal is harmless: per-project single-flight (HB13) and the
//! lease's due re-check (HB14) mean it never double-fires.
//!
//! Kept from S1: [`transport_writes_allowed`] guards every write (HB6),
//! and every `launchctl` / `crontab` call goes through a
//! [`CommandRunner`] that runs nothing under `cfg(test)` (HB7).

use std::fs;
use std::path::{Path, PathBuf};

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

/// Replace the user's crontab with `content` (`crontab -`).
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


/// HB15 / W1 — does any saved wake setting still want the OS tick job?
/// No: the daemon ticks itself in every mode, and waking the machine is
/// the daemon's power layer, not launchd (`WakeSystem` was never a
/// launchd key). Kept as a function so the rule is tested, not implied.
pub fn os_job_wanted(_ws: &crate::app_settings::WakeSchedulerSettings) -> bool {
    false
}

// ── What is left of the old job ────────────────────────────────────────


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


/// What [`transport_state`] found. `Retired` is the healthy state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    /// No OS tick job on this machine: the daemon ticks itself (S2).
    Retired,
    /// An old OS tick job (or its plist / script) is still installed.
    /// The daemon monitor removes it at boot and every 10 minutes.
    Leftover,
}

/// What [`transport_state`] found. Serialised into
/// `/cli/heartbeat/scheduler-status` as `transport`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportReport {
    pub state: TransportState,
    /// The plist the loaded leftover job came from (launchd).
    pub plist_path: Option<String>,
    /// The script the leftover job runs.
    pub program_path: Option<String>,
    /// `None` = never exited / not shown / no job.
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

const RETIRED_DETAIL: &str = "no OS tick job; the daemon checks heartbeats every 60 s itself";

/// Classify what launchd and the disk still hold. `print` is the stdout
/// of a successful `launchctl print`, or `None` when the label is not
/// loaded.
pub fn classify_launchd(
    print: Option<&str>,
    plist_on_disk: bool,
    script_on_disk: bool,
) -> TransportReport {
    if let Some(text) = print {
        let job = parse_launchctl_print(text);
        return TransportReport {
            state: TransportState::Leftover,
            plist_path: job.path.clone(),
            program_path: job.arguments.get(1).cloned().or_else(|| job.program.clone()),
            last_exit_code: match job.last_exit {
                LastExit::Code(c) => Some(c),
                _ => None,
            },
            interval_secs: job.run_interval_secs,
            detail: format!(
                "old tick job {LAUNCHD_LABEL} is still loaded (from {}); it will be removed",
                job.path.as_deref().unwrap_or("<no path>")
            ),
        };
    }
    if plist_on_disk {
        return TransportReport::bare(
            TransportState::Leftover,
            "old tick job plist is still on disk; it will be removed",
        );
    }
    if script_on_disk {
        return TransportReport::bare(
            TransportState::Leftover,
            "old heartbeat.sh is still on disk; it will be removed",
        );
    }
    TransportReport::bare(TransportState::Retired, RETIRED_DETAIL)
}

/// T-S2d — the crontab with every `k2so-agent-heartbeat` line removed.
/// `None` when there was no such line (nothing to write). Every other
/// line is kept as is.
pub fn crontab_without_heartbeat(text: &str) -> Option<String> {
    if !text.lines().any(|l| l.contains(CRON_MARKER)) {
        return None;
    }
    let kept: Vec<&str> = text.lines().filter(|l| !l.contains(CRON_MARKER)).collect();
    Some(if kept.is_empty() { String::new() } else { kept.join("\n") + "\n" })
}

/// Classify the crontab. `Err` = `crontab` could not run (not
/// installed), `Ok(None)` = no crontab for this user. Neither has a job.
pub fn classify_cron(crontab: Result<Option<String>, String>) -> TransportReport {
    let text = match crontab {
        Err(_) | Ok(None) => return TransportReport::bare(TransportState::Retired, RETIRED_DETAIL),
        Ok(Some(t)) => t,
    };
    let line = text
        .lines()
        .find(|l| l.contains(CRON_MARKER))
        .map(str::to_string);
    match line {
        None => TransportReport::bare(TransportState::Retired, RETIRED_DETAIL),
        Some(l) => {
            let mut r = TransportReport::bare(
                TransportState::Leftover,
                format!("old {CRON_MARKER} crontab line is still installed; it will be removed"),
            );
            r.program_path = l.split_whitespace().nth(5).map(str::to_string);
            r.interval_secs = Some(60);
            r
        }
    }
}

/// Read the job's state through `runner`. Read-only: `launchctl print`
/// or `crontab -l`, nothing else.
pub(crate) fn state_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
) -> TransportReport {
    match platform {
        Platform::Launchd => {
            let print = runner.run("launchctl", &["print", &launchd_target()], None);
            let text = match &print {
                Ok(o) if o.success => Some(o.stdout.as_str()),
                _ => None,
            };
            classify_launchd(text, plist_path(home).exists(), script_path(home).exists())
        }
        Platform::Cron => {
            let crontab = runner
                .run("crontab", &["-l"], None)
                .map(|o| if o.success { Some(o.stdout) } else { None });
            let mut r = classify_cron(crontab);
            if r.state == TransportState::Retired && script_path(home).exists() {
                r = TransportReport::bare(
                    TransportState::Leftover,
                    "old heartbeat.sh is still on disk; it will be removed",
                );
            }
            r
        }
        Platform::Unsupported => {
            TransportReport::bare(TransportState::Retired, "no OS tick job on this platform")
        }
    }
}

/// What is left of the old tick job on this machine.
pub fn transport_state() -> TransportReport {
    state_with(&SystemRunner, Platform::current(), &home_dir())
}

/// Old boolean, kept for old clients' "transport installed" check. Old
/// clients read it as "something ticks heartbeats"; after S2 the daemon
/// does, so this is true once no leftover job remains.
pub fn transport_installed() -> bool {
    transport_state().state == TransportState::Retired
}

// ── Removal ────────────────────────────────────────────────────────────

/// Result of a removal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RetireOutcome {
    pub changed: bool,
    pub message: String,
}

/// Remove the old job under `home`, through `runner`. Boots out a
/// loaded launchd label (never bootstraps), deletes the plist and
/// `heartbeat.sh`, and drops only the K2 crontab line.
///
/// Callers that act on the REAL session must call
/// [`transport_writes_allowed`] first; tests pass a fake runner and a
/// temp `home`.
pub(crate) fn retire_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
) -> Result<RetireOutcome, String> {
    let mut removed: Vec<String> = Vec::new();
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
                    return Err(format!("launchctl bootout {target} failed: {}", out.stderr.trim()));
                }
                removed.push(format!("booted out {LAUNCHD_LABEL}"));
            }
            let plist = plist_path(home);
            if plist.exists() {
                fs::remove_file(&plist).map_err(|e| format!("remove {}: {e}", plist.display()))?;
                removed.push("deleted the plist".to_string());
            }
        }
        Platform::Cron => {
            // No `crontab` program (stock Arch) = no line to remove.
            if let Ok(out) = runner.run("crontab", &["-l"], None) {
                if out.success {
                    if let Some(kept) = crontab_without_heartbeat(&out.stdout) {
                        crontab_write(runner, &kept)?;
                        removed.push(format!("removed the {CRON_MARKER} crontab line"));
                    }
                }
            }
        }
        Platform::Unsupported => {}
    }
    let script = script_path(home);
    if script.exists() {
        fs::remove_file(&script).map_err(|e| format!("remove {}: {e}", script.display()))?;
        removed.push("deleted heartbeat.sh".to_string());
    }
    let _ = fs::remove_file(home.join(".k2/heartbeat-projects.txt"));
    Ok(if removed.is_empty() {
        RetireOutcome { changed: false, message: "no OS tick job to remove.".to_string() }
    } else {
        RetireOutcome {
            changed: true,
            message: format!("old OS tick job removed: {}.", removed.join(", ")),
        }
    })
}

/// HB15 — remove this machine's old tick job. Guarded (HB6).
pub fn retire_os_tick_job() -> Result<RetireOutcome, String> {
    transport_writes_allowed()?;
    retire_with(&SystemRunner, Platform::current(), &home_dir())
}

/// Called by `heartbeat add`. Heartbeat S2: adding a heartbeat no longer
/// touches the OS at all — the daemon ticks itself. Always `Ok(false)`.
pub fn ensure_cron_installed() -> Result<bool, String> {
    Ok(false)
}

/// Settings → Apply (older clients) and boot after a label migration.
/// Removes any leftover tick job and reports what the daemon does now.
pub fn apply_wake_scheduler() -> Result<String, String> {
    transport_writes_allowed()?;
    let out = retire_os_tick_job()?;
    let ws = crate::app_settings::load().wake_scheduler;
    let wake = if ws.wake_wanted() {
        "Wake this computer for heartbeats is on."
    } else {
        "This computer is not woken from sleep for heartbeats."
    };
    Ok(format!(
        "Heartbeats fire whenever this computer is awake; the daemon checks every minute. {wake} {}",
        out.message
    ))
}

/// Remove the old job and `heartbeat.sh`. Idempotent.
pub fn uninstall_heartbeat_scheduler() -> Result<(), String> {
    retire_os_tick_job().map(|_| ())
}

// ── Self-check ─────────────────────────────────────────────────────────

/// One self-check that found a leftover job. Persisted as JSON in
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
    /// `retired`, `failed`, or `refused` (the HB6 guard said no).
    pub action: String,
    pub after: Option<TransportState>,
    pub detail: String,
}

/// Remove a leftover job through `runner`. `None` when there is nothing
/// to remove.
pub(crate) fn repair_with(
    runner: &dyn CommandRunner,
    platform: Platform,
    home: &Path,
    before: &TransportReport,
    context: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<RepairRecord> {
    if before.state == TransportState::Retired {
        return None;
    }
    let mut rec = RepairRecord {
        at: now.to_rfc3339(),
        context: context.to_string(),
        before: before.state,
        before_detail: before.detail.clone(),
        action: "failed".to_string(),
        after: None,
        detail: String::new(),
    };
    match retire_with(runner, platform, home) {
        Ok(out) => {
            let after = state_with(runner, platform, home);
            rec.after = Some(after.state);
            if after.state == TransportState::Retired {
                rec.action = "retired".to_string();
                rec.detail = out.message;
            } else {
                rec.detail = format!("{} — still {:?}: {}", out.message, after.state, after.detail);
            }
        }
        Err(e) => rec.detail = e,
    }
    Some(rec)
}

/// HB15 — check this machine for a leftover tick job and remove it. Run
/// by the daemon monitor at boot, after a wake, and every 10 minutes.
/// Returns the check and, when a removal was attempted (or refused), the
/// record, which is also logged and saved to
/// `scheduler_meta.last_transport_repair`.
pub fn self_check_and_repair(context: &str) -> (TransportReport, Option<RepairRecord>) {
    let before = transport_state();
    if before.state == TransportState::Retired {
        return (before, None);
    }
    let now = chrono::Utc::now();
    let record = match transport_writes_allowed() {
        Err(e) => Some(RepairRecord {
            at: now.to_rfc3339(),
            context: context.to_string(),
            before: before.state,
            before_detail: before.detail.clone(),
            action: "refused".to_string(),
            after: None,
            detail: e,
        }),
        Ok(()) => repair_with(&SystemRunner, Platform::current(), &home_dir(), &before, context, now),
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

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Test scaffolding: run `f` with `$HOME` pointed at a fresh temp folder,
/// holding the ONE env lock (`crate::test_env`) so no other env-mutating
/// test races it. Restores HOME and removes the folder afterwards (also on
/// panic, via the `TempHome` guard).
#[cfg(test)]
pub(crate) fn with_temp_home<R>(_label: &str, f: impl FnOnce(&Path) -> R) -> R {
    crate::test_env::with_temp_home(f)
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
    /// under that HOME. After S2, add does not touch the OS at all.
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

            assert_eq!(ensure_cron_installed(), Ok(false), "add no longer installs anything");
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
                ("retire_os_tick_job", retire_os_tick_job().map(|_| ())),
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
        let flag = crate::test_env::EnvVar::set(NO_SELF_HEAL_ENV, "1");
        test_recorder::take();
        let r = apply_wake_scheduler();
        drop(flag);
        let e = r.expect_err("flag must refuse");
        assert!(e.contains("K2_HEARTBEAT_NO_SELF_HEAL=1"), "{e}");
        assert_eq!(test_recorder::take(), Vec::<String>::new());
    }

    /// With the real HOME and no flag, a cfg(test) build still refuses.
    #[test]
    fn real_home_still_refused_in_test_builds() {
        let _lock = crate::themes::HOME_LOCK.lock();
        test_recorder::take();
        let e = retire_os_tick_job().expect_err("cfg(test) must refuse");
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

    const FOREIGN: &str = include_str!("fixtures/launchctl-print-foreign.txt");
    const OK: &str = include_str!("fixtures/launchctl-print-ok.txt");
    const NEVER: &str = include_str!("fixtures/launchctl-print-never-exited.txt");
    const TEMP_HOME: &str = "/private/var/folders/9x/abc123/T/k2so-tunnel-test-96642-1790837486533357000";
    const TEMP_HOME_SHORT: &str = "/var/folders/9x/abc123/T/k2so-tunnel-test-96642-1790837486533357000";

    fn temp_home(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-hb-state-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&d).expect("create temp home");
        d
    }

    #[test]
    fn parses_the_fields_the_report_needs() {
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

    /// Any loaded job — healthy or the 2026-09-30 hijack — is now a
    /// leftover to remove.
    #[test]
    fn any_loaded_job_is_leftover() {
        for fixture in [FOREIGN, OK, NEVER] {
            let r = classify_launchd(Some(fixture), false, false);
            assert_eq!(r.state, TransportState::Leftover, "{}", r.detail);
            assert!(r.plist_path.as_deref().expect("plist path").ends_with("dev.k2.heartbeat.plist"));
        }
        assert_eq!(classify_launchd(Some(FOREIGN), true, true).last_exit_code, Some(127));
    }

    #[test]
    fn files_left_on_disk_are_leftover_and_nothing_is_retired() {
        assert_eq!(classify_launchd(None, true, false).state, TransportState::Leftover);
        assert_eq!(classify_launchd(None, false, true).state, TransportState::Leftover);
        let r = classify_launchd(None, false, false);
        assert_eq!(r.state, TransportState::Retired, "{}", r.detail);
    }

    #[test]
    fn cron_states() {
        let line = "* * * * * /home/k/.k2/heartbeat.sh # k2so-agent-heartbeat";
        let r = classify_cron(Ok(Some(format!("0 1 * * * backup\n{line}\n"))));
        assert_eq!(r.state, TransportState::Leftover, "{}", r.detail);
        assert_eq!(r.program_path.as_deref(), Some("/home/k/.k2/heartbeat.sh"));
        assert_eq!(classify_cron(Ok(Some("0 1 * * * backup\n".into()))).state, TransportState::Retired);
        assert_eq!(classify_cron(Ok(None)).state, TransportState::Retired);
        // Stock Arch: no `crontab` program at all.
        assert_eq!(classify_cron(Err("not installed".into())).state, TransportState::Retired);
    }

    /// T-S2d — the pure crontab filter keeps every other line.
    #[test]
    fn crontab_filter_drops_only_the_k2_line() {
        let input = "MAILTO=me\n0 1 * * * /usr/bin/backup\n* * * * * /home/k/.k2/heartbeat.sh # k2so-agent-heartbeat\n";
        assert_eq!(
            crontab_without_heartbeat(input),
            Some("MAILTO=me\n0 1 * * * /usr/bin/backup\n".to_string())
        );
        assert_eq!(crontab_without_heartbeat("0 1 * * * /usr/bin/backup\n"), None);
        assert_eq!(
            crontab_without_heartbeat("* * * * * x # k2so-agent-heartbeat\n"),
            Some(String::new())
        );
    }

    #[test]
    fn state_with_reads_only() {
        let home = temp_home("read");
        let runner = FakeRunner::launchd(Some(OK.to_string()), None);
        let r = state_with(&runner, Platform::Launchd, &home);
        assert_eq!(r.state, TransportState::Leftover, "{}", r.detail);
        assert_eq!(runner.writes(), Vec::<String>::new());

        let runner = FakeRunner::cron(None);
        let r = state_with(&runner, Platform::Cron, &home);
        assert_eq!(r.state, TransportState::Retired);
        assert_eq!(runner.calls(), vec!["crontab -l".to_string()]);

        let runner = FakeRunner::launchd(None, None);
        let r = state_with(&runner, Platform::Unsupported, &home);
        assert_eq!(r.state, TransportState::Retired);
        assert_eq!(runner.calls(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn transport_state_serialises_for_the_status_route() {
        let r = classify_launchd(Some(FOREIGN), true, true);
        let v = serde_json::to_value(&r).expect("serialise");
        assert_eq!(v["state"], "leftover");
        assert_eq!(v["lastExitCode"], 127);
        assert!(v["plistPath"].as_str().expect("plistPath").ends_with("dev.k2.heartbeat.plist"));
        assert!(v["programPath"].as_str().expect("programPath").ends_with(".k2/heartbeat.sh"));
        let v = serde_json::to_value(classify_launchd(None, false, false)).expect("serialise");
        assert_eq!(v["state"], "retired");
    }
}

#[cfg(test)]
mod retire_tests {
    use super::fake_runner::FakeRunner;
    use super::*;

    const OK: &str = include_str!("fixtures/launchctl-print-ok.txt");

    fn temp_home(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-hb-retire-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(d.join("Library/LaunchAgents")).expect("create LaunchAgents");
        fs::create_dir_all(d.join(".k2")).expect("create .k2");
        d
    }

    fn ws(mode: &str, wake: bool) -> crate::app_settings::WakeSchedulerSettings {
        crate::app_settings::WakeSchedulerSettings {
            mode: mode.to_string(),
            interval_minutes: 5,
            wake_system: wake,
            wake_for_heartbeats: Some(wake),
            wake_on_battery: false,
        }
    }

    /// T-W1 — in every saved mode the OS job is unwanted, and removing a
    /// loaded job is exactly one `bootout` and zero `bootstrap`.
    #[test]
    fn every_mode_boots_out_once_and_never_bootstraps() {
        for (mode, wake) in [("off", false), ("on_demand", false), ("heartbeat", false), ("heartbeat", true)] {
            assert!(!os_job_wanted(&ws(mode, wake)), "{mode}/{wake} must not want an OS job");
            let home = temp_home(mode);
            fs::write(plist_path(&home), "<plist/>").expect("seed plist");
            fs::write(script_path(&home), "#!/bin/bash\n").expect("seed script");
            let runner = FakeRunner::launchd(Some(OK.to_string()), None);
            let out = retire_with(&runner, Platform::Launchd, &home).expect("retire");
            assert!(out.changed, "{}", out.message);
            let calls = runner.calls();
            assert_eq!(calls.iter().filter(|c| c.starts_with("launchctl bootout")).count(), 1, "{calls:?}");
            assert_eq!(calls.iter().filter(|c| c.starts_with("launchctl bootstrap")).count(), 0, "{calls:?}");
            assert!(!plist_path(&home).exists(), "plist must be deleted");
            assert!(!script_path(&home).exists(), "heartbeat.sh must be deleted");
            let _ = fs::remove_dir_all(&home);
        }
    }

    #[test]
    fn nothing_loaded_means_no_bootout() {
        let home = temp_home("none");
        let runner = FakeRunner::launchd(None, None);
        let out = retire_with(&runner, Platform::Launchd, &home).expect("retire");
        assert!(!out.changed, "{}", out.message);
        assert_eq!(runner.writes(), Vec::<String>::new());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn failed_bootout_is_an_error() {
        struct FailingBootout;
        impl CommandRunner for FailingBootout {
            fn run(&self, _p: &str, args: &[&str], _s: Option<&str>) -> Result<CmdOutput, String> {
                Ok(CmdOutput {
                    success: args.first() == Some(&"print"),
                    stdout: String::new(),
                    stderr: "Boot-out failed: 5: Input/output error".into(),
                })
            }
        }
        let home = temp_home("bootout-fail");
        let e = retire_with(&FailingBootout, Platform::Launchd, &home).expect_err("must fail loudly");
        assert!(e.contains("bootout"), "{e}");
        let _ = fs::remove_dir_all(&home);
    }

    /// D5 — the Linux upgrade drops the K2 crontab line and keeps the rest.
    #[test]
    fn cron_line_removed_other_lines_kept() {
        let home = temp_home("cron");
        let runner = FakeRunner::cron(Some(
            "0 1 * * * /usr/bin/backup\n* * * * * /h/.k2/heartbeat.sh # k2so-agent-heartbeat\n".into(),
        ));
        let out = retire_with(&runner, Platform::Cron, &home).expect("retire");
        assert!(out.changed);
        assert_eq!(runner.crontab.borrow().as_deref(), Some("0 1 * * * /usr/bin/backup\n"));
        assert!(runner.calls().contains(&"crontab -".to_string()));
        let _ = fs::remove_dir_all(&home);
    }

    /// Stock Arch: no `crontab` binary — nothing to do, not an error.
    #[test]
    fn missing_crontab_is_not_an_error() {
        let home = temp_home("no-cron");
        let mut runner = FakeRunner::cron(None);
        runner.crontab_missing = true;
        let out = retire_with(&runner, Platform::Cron, &home).expect("retire");
        assert!(!out.changed);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn repair_record_says_retired() {
        let home = temp_home("repair");
        fs::write(plist_path(&home), "<plist/>").expect("seed plist");
        let runner = FakeRunner::launchd(Some(OK.to_string()), None);
        let before = state_with(&runner, Platform::Launchd, &home);
        assert_eq!(before.state, TransportState::Leftover);
        let rec = repair_with(&runner, Platform::Launchd, &home, &before, "boot", chrono::Utc::now())
            .expect("a leftover job gets a record");
        assert_eq!(rec.action, "retired", "{}", rec.detail);
        assert_eq!(rec.after, Some(TransportState::Retired));
        let calls = runner.calls();
        assert_eq!(calls.iter().filter(|c| c.starts_with("launchctl bootstrap")).count(), 0);

        let retired = state_with(&runner, Platform::Launchd, &home);
        assert!(repair_with(&runner, Platform::Launchd, &home, &retired, "periodic", chrono::Utc::now()).is_none());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn self_check_in_test_build_never_writes() {
        with_temp_home("self-check", |home| {
            test_recorder::take();
            let (report, rec) = self_check_and_repair("boot");
            // The cfg(test) system runner fails every call, so launchctl
            // print reads as "not loaded": nothing to do, nothing written.
            if let Some(rec) = rec {
                assert_eq!(rec.action, "refused", "{}", rec.detail);
            } else {
                assert_eq!(report.state, TransportState::Retired);
            }
            for call in test_recorder::take() {
                assert!(
                    call.starts_with("launchctl print") || call == "crontab -l",
                    "self-check ran a write under test: {call}"
                );
            }
            assert!(!plist_path(home).exists());
        });
    }
}
