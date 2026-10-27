//! First run without git (0.46.4, Rosson 2026-10-26): the daemon side of
//! `k2_core::git_tools`.
//!
//! - `GET /cli/git-tools`: is git usable, the state (`not_installed` /
//!   `installing` / `git_broken` / `ready`), how to install or fix it by
//!   hand, and the workspaces whose git steps wait (with their last error).
//! - `POST /cli/git-tools/install` (owner token only): `/usr/bin/xcode-select
//!   --install`, which opens Apple's installer dialog on this Mac. Never
//!   sudo. A quick refusal ("already installed") is a 409 with its words.
//! - `POST /cli/git-tools/recheck`: look for git again now and, if it
//!   runs, do the waiting git steps.
//! - `?format=text` on any of them: plain lines for `k2 git-tools`, so the
//!   CLI needs no python3 (on a fresh Mac `/usr/bin/python3` is one of
//!   Apple's shims too).
//! - [`spawn_loop`]: the same check every few seconds, headless, so the
//!   waiting steps run as soon as the install finishes with no client
//!   attached; after an hour of no git (and no install started) it looks
//!   only every 10 minutes.

use std::time::{Duration, Instant};

use crate::cli_response::CliResponse;
use k2_core::git_tools::{self, InstallStart};

fn ok_json<T: serde::Serialize>(v: T) -> CliResponse {
    match serde_json::to_string(&v) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => CliResponse::internal_error(e.to_string()),
    }
}

fn text(status: &'static str, body: String) -> CliResponse {
    CliResponse { status, content_type: "text/plain; charset=utf-8", body }
}

/// Run the waiting git steps (when git runs) and tell clients the
/// workspace list changed when any finished.
fn run_waiting(force: bool) -> Vec<String> {
    let done = git_tools::run_all_deferred(force);
    if !done.is_empty() {
        tell_clients();
    }
    done
}

fn tell_clients() {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::ProjectsChanged {});
    crate::fs_live::resync_watches();
}

fn spawn_finished_git_steps(_folder: &std::path::Path) {
    tell_clients();
}

/// Register k2-core's listener for a PTY spawn that finished a folder's
/// waiting git steps (0.46.5, first-run review item 12): clients hear
/// `ProjectsChanged` at once, as after the loop or a recheck. Idempotent;
/// called by [`spawn_loop`] and the test harness.
pub fn install_listener() {
    git_tools::set_done_listener(spawn_finished_git_steps);
}

fn respond(as_text: bool, started: Option<InstallStart>, finished: Option<Vec<String>>) -> CliResponse {
    let s = git_tools::status(false);
    if as_text {
        return text("200 OK", git_tools::render_text(&s, started, finished.as_deref().unwrap_or(&[])));
    }
    let mut v = serde_json::to_value(&s).unwrap_or_default();
    if let Some(st) = started {
        v["started"] = serde_json::json!(st);
    }
    if let Some(f) = finished {
        v["finished"] = serde_json::json!(f);
    }
    ok_json(v)
}

fn refused(as_text: bool, code: &str, message: String, how_to: Vec<String>) -> CliResponse {
    if as_text {
        let mut body = format!("k2 git-tools: {message}\n");
        for line in &how_to {
            body.push_str(&format!("  {line}\n"));
        }
        return text("409 Conflict", body);
    }
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({ "error": message, "code": code, "howTo": how_to }).to_string(),
    }
}

/// The 403 body an agent's passport gets on `POST /cli/git-tools/install`
/// (0.46.5, first-run review item 13): the owner's step, and what the agent
/// can do instead. `k2 git-tools` prints the hint and exits 3.
pub fn agent_install_refusal() -> String {
    serde_json::json!({
        "error": "owner_only",
        "hint": "Installing git is your owner's step (on a Mac it opens Apple's installer on this computer's screen). Ask them to click Install in Settings → General → Git, or run `k2 git-tools install` in their own terminal. `k2 git-tools` shows the state; the waiting git steps finish by themselves once git runs.",
    })
    .to_string()
}

/// `GET /cli/git-tools`.
pub fn handle_status(as_text: bool) -> CliResponse {
    let _ = k2_core::git_safe::refresh();
    respond(as_text, None, None)
}

/// `POST /cli/git-tools/install`.
pub fn handle_install(is_post: bool, as_text: bool) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    if !cfg!(target_os = "macos") {
        return refused(
            as_text,
            "not_macos",
            "K2 installs git only on macOS (Apple's command line tools); install it with your package manager".into(),
            git_tools::status(false).how_to,
        );
    }
    match git_tools::start_apple_install() {
        Ok(started) => {
            if started == InstallStart::AlreadyReady {
                run_waiting(true);
            }
            respond(as_text, Some(started), None)
        }
        Err(e) => refused(as_text, "install_refused", e, Vec::new()),
    }
}

/// `POST /cli/git-tools/recheck`.
pub fn handle_recheck(is_post: bool, as_text: bool) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let ready = k2_core::git_safe::refresh().is_ready();
    let done = if ready { run_waiting(true) } else { Vec::new() };
    respond(as_text, None, Some(done))
}

/// How often the headless loop looks again.
const EVERY: Duration = Duration::from_secs(10);
/// After this long without git (and no install started)…
const SLOW_AFTER: Duration = Duration::from_secs(60 * 60);
/// …look only this often.
const SLOW_EVERY: Duration = Duration::from_secs(10 * 60);

/// Pure: should the loop probe now?
pub fn loop_should_probe(missing_for: Duration, since_last_probe: Duration, install_started_recently: bool) -> bool {
    if missing_for < SLOW_AFTER || install_started_recently {
        return true;
    }
    since_last_probe >= SLOW_EVERY
}

/// The headless watch: while git doesn't run, probe again every few
/// seconds (every 10 minutes after an hour, unless an install was
/// started); once it does, do every workspace's waiting git steps.
pub fn spawn_loop() {
    install_listener();
    let _ = std::thread::Builder::new().name("git-tools".into()).spawn(|| {
        let mut missing_since: Option<Instant> = None;
        let mut last_probe = Instant::now();
        loop {
            std::thread::sleep(EVERY);
            let ready_now = k2_core::git_safe::cached().is_some_and(|p| p.is_ready());
            if ready_now {
                missing_since = None;
                run_waiting(false);
                continue;
            }
            let since = *missing_since.get_or_insert_with(Instant::now);
            let recent_install = git_tools::install_started_at()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|d| d < SLOW_AFTER);
            if !loop_should_probe(since.elapsed(), last_probe.elapsed(), recent_install) {
                continue;
            }
            last_probe = Instant::now();
            if k2_core::git_safe::refresh().is_ready() {
                missing_since = None;
                run_waiting(false);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loop_slows_down_after_an_hour_unless_an_install_was_started() {
        let m = |s: u64| Duration::from_secs(s);
        assert!(loop_should_probe(m(30), m(10), false), "first hour: every tick");
        assert!(loop_should_probe(m(59 * 60), m(10), false));
        assert!(!loop_should_probe(m(61 * 60), m(10), false), "after an hour: not every tick");
        assert!(loop_should_probe(m(61 * 60), m(10 * 60), false), "…but every 10 minutes");
        assert!(loop_should_probe(m(5 * 60 * 60), m(10), true), "an install in progress: every tick");
    }
}
