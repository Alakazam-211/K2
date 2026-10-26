//! First run without git (0.46.4, Rosson 2026-10-26): the daemon side of
//! `k2_core::git_tools`.
//!
//! - `GET /cli/git-tools`: is git usable, the install state
//!   (`not_installed` / `installing` / `ready`), how to install by hand,
//!   and the workspaces whose git steps wait.
//! - `POST /cli/git-tools/install` (owner only): `xcode-select --install`,
//!   which opens Apple's installer dialog on this Mac. Never sudo.
//! - `POST /cli/git-tools/recheck`: look for git again now and, if it
//!   runs, do the waiting git steps.
//! - [`spawn_loop`]: the same check every few seconds, headless, so the
//!   waiting steps run as soon as the install finishes with no client
//!   attached.

use std::time::Duration;

use crate::cli_response::CliResponse;
use k2_core::git_tools;

fn ok<T: serde::Serialize>(v: T) -> CliResponse {
    match serde_json::to_string(&v) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => CliResponse::internal_error(e.to_string()),
    }
}

/// Run the waiting git steps (when git runs) and tell clients the
/// workspace list changed when any finished.
fn run_waiting(force: bool) -> Vec<String> {
    let done = git_tools::run_all_deferred(force);
    if !done.is_empty() {
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::ProjectsChanged {});
        crate::fs_live::resync_watches();
    }
    done
}

/// `GET /cli/git-tools`.
pub fn handle_status() -> CliResponse {
    ok(git_tools::status(true))
}

/// `POST /cli/git-tools/install`.
pub fn handle_install(is_post: bool) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    if !cfg!(target_os = "macos") {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: serde_json::json!({
                "error": "K2 installs git only on macOS (Apple's command line tools); install it with your package manager",
                "code": "not_macos",
                "howTo": git_tools::status(false).how_to,
            })
            .to_string(),
        };
    }
    match git_tools::start_apple_install() {
        Ok(started) => {
            if started == git_tools::InstallStart::AlreadyReady {
                run_waiting(true);
            }
            let mut v = serde_json::to_value(git_tools::status(false)).unwrap_or_default();
            v["started"] = serde_json::json!(started);
            ok(v)
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `POST /cli/git-tools/recheck`.
pub fn handle_recheck(is_post: bool) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let ready = k2_core::git_safe::refresh().is_ready();
    let done = if ready { run_waiting(true) } else { Vec::new() };
    let mut v = serde_json::to_value(git_tools::status(false)).unwrap_or_default();
    v["finished"] = serde_json::json!(done);
    ok(v)
}

/// How often the headless loop looks again.
const EVERY: Duration = Duration::from_secs(10);

/// The headless watch: while git doesn't run, probe again every few
/// seconds; once it does, do every workspace's waiting git steps.
pub fn spawn_loop() {
    let _ = std::thread::Builder::new().name("git-tools".into()).spawn(|| loop {
        std::thread::sleep(EVERY);
        // `availability` re-probes on its own while git is missing.
        if k2_core::git_safe::availability().is_ready() {
            run_waiting(false);
        }
    });
}
