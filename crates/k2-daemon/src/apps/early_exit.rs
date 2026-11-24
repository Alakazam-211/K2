//! A start that failed after the App's own command ran (it exited before
//! it was healthy, failed its gate, or K2 couldn't start it): the App's
//! state (`k2 app status`, `inspect`, `list`: "why") says how the instance
//! ended and where its output is. Every engine keeps that output from the
//! first byte of stdout and stderr, also for a start that never went live
//! (the process engine's log file, the walled engine's kept `k8s-file`).
use k2_core::brand::CLI_NAME;

/// How an instance ended, in words: "exited with code 1", "was killed by
/// SIGKILL", "exited". `None` when it didn't exit.
pub fn exit_text(exited: bool, code: Option<i32>, signal: Option<i32>) -> Option<String> {
    exited.then(|| super::failures::exit_words(code, signal))
}

/// The command that shows a deployment's output.
pub fn logs_hint(app_id: &str, deployment: &str) -> String {
    format!("{CLI_NAME} app logs {app_id} --deployment {deployment}")
}

/// The App's state reason for a failed start: K2's message, how the
/// instance ended (when it exited), and the logs command.
pub fn state_reason(app_id: &str, deployment: &str, msg: &str, exit: Option<&str>) -> String {
    let mut s = msg.trim_end_matches(['.', ' ']).to_string();
    if let Some(e) = exit {
        s.push_str(&format!("; it {e}"));
    }
    s.push_str(&format!(". Its output: {}", logs_hint(app_id, deployment)));
    s
}

#[cfg(test)]
#[path = "early_exit_tests.rs"]
mod tests;
