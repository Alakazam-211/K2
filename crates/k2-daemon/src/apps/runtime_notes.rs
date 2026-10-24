//! Loud App runtime failures (prd-app-runtime-update-continuity-v1 R34,
//! R44, A6; the 2026-10-23 Hearth outage wrote **no** daemon log line for
//! any of its failed restarts and woke nobody).
//!
//! - [`error`]: one `ERROR [apps] …` line on stderr (the journal).
//! - [`owner_notice`]: a notice for the server's **owner** on the Tickets
//!   board (`feedback_routes::file_owner_notice`, the auto-prepare path),
//!   not the App's agent (`alerts::notify` stays for per-App agent notes).
//! - [`failed_attempt`] / [`went_live`]: R44. Every failed start, restart or
//!   re-host writes one line with its next try; one owner notice per App per
//!   failure run, after 3 failed system restarts (any cause) and once when it
//!   first waits for the engine. A run ends when the App goes live.
//!
//! Test builds record lines and notices in memory ([`captured`]) and never
//! file a real ticket; tests filter by their own App ids (the record is
//! process-wide).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Failed system restarts in one run before the owner hears (R44).
pub const NOTICE_AFTER_RESTARTS: u32 = 3;
/// Who the notices name.
pub const AGENT_NAME: &str = "k2-apps";

/// When K2 tries again after a failed attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// The crash backoff's step: in this many seconds.
    In(i64),
    /// K2's own fault: when the App engine changes, else every `retry` s.
    OnEngineChange { retry_secs: i64 },
    /// Nothing scheduled (an explicit verb; the breaker; a live instance
    /// keeps serving).
    NotScheduled,
}

impl Next {
    pub fn words(self) -> String {
        match self {
            Next::In(0) => "next try now".into(),
            Next::In(n) => format!("next try in {n}s"),
            Next::OnEngineChange { retry_secs } => format!("next try on engine change (or in {retry_secs}s)"),
            Next::NotScheduled => "next try not scheduled".into(),
        }
    }
}

/// What a test build recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    Line(String),
    Notice { title: String, body: String },
}

#[cfg(test)]
fn record() -> &'static Mutex<Vec<Captured>> {
    static R: OnceLock<Mutex<Vec<Captured>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// Everything a test build recorded so far (process-wide).
#[cfg(test)]
pub fn captured() -> Vec<Captured> {
    record().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// One `ERROR [apps] <text>` line.
pub fn error(text: &str) {
    k2_core::log_debug!("ERROR [apps] {text}");
    #[cfg(test)]
    record().lock().unwrap_or_else(|e| e.into_inner()).push(Captured::Line(format!("ERROR [apps] {text}")));
}

/// A notice for the server's owner (A6).
pub fn owner_notice(title: &str, body: &str) {
    #[cfg(test)]
    {
        record().lock().unwrap_or_else(|e| e.into_inner()).push(Captured::Notice { title: title.into(), body: body.into() });
    }
    #[cfg(not(test))]
    {
        if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], AGENT_NAME, title, body) {
            k2_core::log_debug!("[apps] owner notice '{title}': {e}");
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Run {
    restarts: u32,
    restart_noticed: bool,
    waiting_noticed: bool,
}

fn runs() -> &'static Mutex<HashMap<String, Run>> {
    static R: OnceLock<Mutex<HashMap<String, Run>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One failed attempt.
#[derive(Debug, Clone)]
pub struct Attempt<'a> {
    pub app_id: &'a str,
    /// `deploy`, `restart`, `start`, `rollback`, `rehost`, …
    pub kind: &'a str,
    pub deployment: &'a str,
    /// The structured code (`deployments.error_code`).
    pub code: &'a str,
    pub message: &'a str,
    /// A K2 system restart (the ones R44 counts toward the notice).
    pub system_restart: bool,
    pub next: Next,
}

/// The R44 line text (without the `ERROR [apps]` prefix).
pub fn line_for(a: &Attempt<'_>) -> String {
    let msg = a.message.replace(['\n', '\r'], " ");
    format!("{} {} {} failed ({}): {msg}; {}", a.app_id, a.kind, a.deployment, a.code, a.next.words())
}

/// R44: the line, and the owner notice when this attempt crosses a line.
/// Returns the notice it filed (title), if any.
pub fn failed_attempt(a: &Attempt<'_>) -> Option<String> {
    error(&line_for(a));
    let waiting = matches!(a.next, Next::OnEngineChange { .. });
    let (restart_notice, waiting_notice) = {
        let mut r = runs().lock().unwrap_or_else(|e| e.into_inner());
        let run = r.entry(a.app_id.to_string()).or_default();
        if a.system_restart {
            run.restarts += 1;
        }
        let restart_notice = a.system_restart && run.restarts >= NOTICE_AFTER_RESTARTS && !run.restart_noticed;
        if restart_notice {
            run.restart_noticed = true;
        }
        let waiting_notice = waiting && !run.waiting_noticed;
        if waiting_notice {
            run.waiting_noticed = true;
        }
        (restart_notice.then_some(run.restarts), waiting_notice)
    };
    let mut filed = None;
    if waiting_notice {
        let title = format!("App {} is waiting for K2's App engine", a.app_id);
        owner_notice(
            &title,
            &format!(
                "K2 couldn't start the App {} because of K2's own App engine ({}: {}). This is not the App's fault and \
                 doesn't count as a crash. K2 starts it as soon as the engine answers (it checks every few seconds and \
                 retries every minute). If it doesn't come back, check `k2 app engine status`; the owner can run \
                 `k2 app engine install`. History: k2 app history {}",
                a.app_id, a.code, a.message, a.app_id
            ),
        );
        filed = Some(title);
    }
    if let Some(n) = restart_notice {
        let title = format!("App {} failed to restart {n} times", a.app_id);
        owner_notice(
            &title,
            &format!(
                "K2 tried to restart the App {} {n} times and it isn't running. Last attempt {} ({}): {}. K2's plan: {}. \
                 Read why: k2 app errors {} ; history: k2 app history {}",
                a.app_id,
                a.deployment,
                a.code,
                a.message,
                a.next.words(),
                a.app_id,
                a.app_id
            ),
        );
        filed = Some(title);
    }
    filed
}

/// The App went live: its failure run is over.
pub fn went_live(app_id: &str) {
    runs().lock().unwrap_or_else(|e| e.into_inner()).remove(app_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notices_for(app: &str) -> Vec<String> {
        captured()
            .into_iter()
            .filter_map(|c| match c {
                Captured::Notice { title, .. } if title.contains(&format!("App {app} ")) => Some(title),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_line_per_attempt_and_one_notice_per_run() {
        let app = "rn-run-a";
        let at = |code: &'static str, next: Next| Attempt {
            app_id: app,
            kind: "restart",
            deployment: "d-000000000001",
            code,
            message: "boom\nsecond line",
            system_restart: true,
            next,
        };
        assert_eq!(failed_attempt(&at("health_failed", Next::In(5))), None);
        assert_eq!(failed_attempt(&at("health_failed", Next::In(30))), None);
        let third = failed_attempt(&at("health_failed", Next::In(120))).expect("the third failed restart files one notice");
        assert!(third.contains("3 times"), "{third}");
        assert_eq!(failed_attempt(&at("health_failed", Next::In(600))), None, "never twice in a run");
        let lines: Vec<String> = captured()
            .into_iter()
            .filter_map(|c| match c {
                Captured::Line(l) if l.contains(app) => Some(l),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(lines[1], format!("ERROR [apps] {app} restart d-000000000001 failed (health_failed): boom second line; next try in 30s"));
        went_live(app);
        failed_attempt(&at("health_failed", Next::In(0)));
        failed_attempt(&at("health_failed", Next::In(5)));
        assert!(failed_attempt(&at("health_failed", Next::In(30))).is_some(), "a new run notices again");
        assert_eq!(notices_for(app).len(), 2);
    }

    #[test]
    fn entering_waiting_engine_notices_once() {
        let app = "rn-wait-a";
        let at = Attempt {
            app_id: app,
            kind: "restart",
            deployment: "d-000000000002",
            code: k2_core::deploy::codes::ENGINE_PROTOCOL_MISMATCH,
            message: "protocol 2 vs 3",
            system_restart: true,
            next: Next::OnEngineChange { retry_secs: 60 },
        };
        let first = failed_attempt(&at).expect("the first wait for the engine notices");
        assert!(first.contains("waiting for K2's App engine"), "{first}");
        assert_eq!(failed_attempt(&at), None);
        // The third restart (same run) still gets its own notice once.
        assert!(failed_attempt(&at).expect("third").contains("3 times"));
        assert_eq!(failed_attempt(&at), None);
        assert_eq!(notices_for(app).len(), 2);
        assert!(line_for(&at).ends_with("next try on engine change (or in 60s)"), "{}", line_for(&at));
    }
}
