//! Auto-rotate for LLM subscriptions: the daemon's loop
//! ([`k2_core::llm_accounts::auto_next`] holds the setting and the rule).
//!
//! Every [`TICK`] (and right after a person turns it on), for each tool a
//! person turned it on for: when the live subscription (the server
//! default) is at [`auto_next::THRESHOLD_PERCENT`] of its binding window,
//! press the wallet's Next ([`crate::llm_accounts_routes::run_next`], the
//! button's own code: same switch, same audit line, same events), leaving
//! out the subscriptions that are themselves at their limit, whose usage
//! check is backing off, or that aren't signed in. The rule a subscription
//! is never live in two places holds because Next's pick already skips
//! pinned tokens.
//!
//! - **Single-flight:** one tick at a time (a tick that finds one running
//!   does nothing).
//! - **Cooldown:** no second automatic switch on a tool for
//!   [`auto_next::COOLDOWN_SECS`].
//! - **Nothing left:** no switch, and one line per episode: "All your
//!   Claude subscriptions are at their limit" (an owner notice, the audit
//!   log, the setting's last line).
//! - **The person:** it acts only while the person who turned it on may
//!   still change the server default (the owner token, or an Owner or
//!   Admin login that is enabled).
//!
//! Headless: a daemon thread, no client needed. Never under air-gap (no
//! usage readings then, and the switch's vendor check is off).

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use k2_core::llm_accounts::{self as wallet_core, auto_next, state, wallet, Entry, Tool, WalletError};

use crate::llm_accounts_routes as routes;
use crate::subscription_usage as su;

/// How often the loop looks.
pub const TICK: Duration = Duration::from_secs(30);

/// Who the switch is made by in the wallet (`switchedBy`) and the audit
/// log: `auto-next:<person>`.
pub const KEY_PREFIX: &str = "auto-next:";

/// The notice's sender name.
const NOTICE_FROM: &str = "k2-llm";

/// What one tick did for one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Below the limit, no reading, or no subscription is the default.
    Nothing,
    /// The person who turned it on can't change the server default now.
    PersonCannotAct,
    /// At the limit inside the cooldown.
    Cooldown,
    /// Switched: the line written.
    Switched { to: String, line: String },
    /// Every subscription is at its limit: `told` the first time.
    AllAtLimit { told: bool },
    /// The switch didn't go through (busy, changed, …): tried again at the
    /// next tick.
    Failed(String),
}

/// May the person who turned it on still change the server default? The
/// owner token, or an enabled Owner or Admin login.
pub fn person_may_act(person: &str) -> bool {
    if person == "owner-token" {
        return true;
    }
    person
        .strip_prefix("login:")
        .is_some_and(|u| !u.is_empty() && k2_core::connect_users::is_active_owner_or_admin(u))
}

/// One token's binding-window percent, as the wallet serves it
/// (`maxPercent`: the top-bar chip's number).
fn token_percent(e: &Entry, now: DateTime<Utc>) -> Option<i64> {
    routes::usage_for_at(e, now)["maxPercent"].as_i64()
}

/// The live token's percent: its own reading, and (old usage path) the
/// shared probe's row when that row is this token's.
fn live_percent(e: &Entry, tool: Tool, now: DateTime<Utc>) -> Option<i64> {
    let own = token_percent(e, now);
    if crate::usage_store::single_record_on() {
        return own;
    }
    let shared = su::served_shared_row(tool.as_str(), now)
        .filter(|r| r.account_id.as_deref() == Some(e.id.as_str()))
        .and_then(|r| su::chip_percent(&r, now));
    match (own, shared) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn)
}

fn audit(event: &str, user: &str, outcome: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, user, outcome.to_string(), "daemon", "-", "cli"));
}

#[cfg(test)]
pub(crate) static NOTICES: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());

fn owner_notice(title: &str, body: &str) {
    #[cfg(not(test))]
    if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], NOTICE_FROM, title, body) {
        k2_core::log_debug!("[llm-auto-next] notice not filed: {e}");
    }
    #[cfg(test)]
    {
        let _ = NOTICE_FROM;
        NOTICES.lock().push(format!("{title}\n{body}"));
    }
}

/// One tool, one look. `vendor` is the switch's account check (the real
/// one in the daemon, a fake in tests).
fn tool_tick(s: &auto_next::Setting, vendor: &dyn wallet::Refresher, now: DateTime<Utc>) -> Result<Outcome, WalletError> {
    let Some(tool) = Tool::parse(&s.tool) else { return Ok(Outcome::Nothing) };
    if !person_may_act(&s.person) {
        return Ok(Outcome::PersonCannotAct);
    }
    // The server default, when it is a subscription that is the live one.
    let (live, candidates) = with_conn(|conn| -> Result<_, WalletError> {
        let active = wallet_core::active_id(conn, tool)?;
        let live = wallet_core::live_owner(conn, tool)?;
        let live = match (active, live) {
            (Some(a), Some(l)) if a == l => wallet_core::get(conn, &l)?.filter(|e| !e.is_api_key() && e.removed_at.is_none()),
            _ => None,
        };
        let candidates = if live.is_some() { wallet_core::next_candidates(conn, tool)? } else { Vec::new() };
        Ok((live, candidates))
    })?;
    let Some(live) = live else { return Ok(Outcome::Nothing) };
    // Readings outside the database lock.
    let percent = live_percent(&live, tool, now);
    let cands: Vec<auto_next::Candidate> = candidates
        .iter()
        .map(|e| auto_next::Candidate {
            id: e.id.clone(),
            label: e.label.clone(),
            percent: token_percent(e, now),
            backing_off: su::backing_off(&format!("acct:{}", e.id), now).is_some(),
            signed_in: e.state == state::SIGNED_IN,
        })
        .collect();
    let ts = now.timestamp();
    match auto_next::decide(percent, &cands, s.last_switch_at, ts) {
        auto_next::Decision::Below => {
            if s.exhausted_at.is_some() {
                with_conn(|conn| auto_next::clear_exhausted(conn, tool))?;
            }
            Ok(Outcome::Nothing)
        }
        auto_next::Decision::Cooldown { .. } => Ok(Outcome::Cooldown),
        auto_next::Decision::AllAtLimit => all_at_limit(tool, &s.person, ts),
        auto_next::Decision::Switch { exclude, .. } => {
            let key = format!("{KEY_PREFIX}{}", s.person);
            match routes::run_next(tool, &key, "auto-next", vendor, &exclude) {
                Ok((out, _, _)) if !out.already_active => {
                    let line = auto_next::switched_line(&out.to_label, out.from_label.as_deref(), percent.unwrap_or(auto_next::THRESHOLD_PERCENT));
                    with_conn(|conn| auto_next::note_switch(conn, tool, &line, ts))?;
                    audit("llm_accounts.auto_next", &key, &format!("{}: {line}", tool.as_str()));
                    k2_core::log_debug!("llm: {} {line}", tool.as_str());
                    crate::session_events::emit_llm_accounts_changed(Some(tool.as_str()));
                    Ok(Outcome::Switched { to: out.to, line })
                }
                Ok(_) => Ok(Outcome::Nothing),
                // Next found nobody after all (a token moved meanwhile).
                Err(WalletError::NotFound(k)) if k == "no_next" => all_at_limit(tool, &s.person, ts),
                Err(e) => {
                    k2_core::log_debug!("[llm-auto-next] {} switch not made: {}", tool.as_str(), e.code());
                    Ok(Outcome::Failed(e.code().to_string()))
                }
            }
        }
    }
}

fn all_at_limit(tool: Tool, person: &str, ts: i64) -> Result<Outcome, WalletError> {
    let line = auto_next::exhausted_line(tool);
    let told = with_conn(|conn| auto_next::note_exhausted(conn, tool, &line, ts))?;
    if told {
        audit("llm_accounts.auto_next_all_at_limit", &format!("{KEY_PREFIX}{person}"), tool.as_str());
        k2_core::log_debug!("llm: {line}");
        owner_notice(
            &line,
            &format!(
                "Auto-rotate is on for {}, and every signed-in {} subscription on this server is at 99% or more of its limit, so K2 didn't switch. \
                 It switches again once one has room (when a limit resets), or add another subscription on Settings → Language Models → Subs / Tokens.",
                tool.display(),
                tool.display()
            ),
        );
        crate::session_events::emit_llm_accounts_changed(Some(tool.as_str()));
    }
    Ok(Outcome::AllAtLimit { told })
}

/// One look at every tool with auto-rotate on. Single-flight: `None` when
/// another tick is running.
pub(crate) fn tick_with(vendor: &dyn wallet::Refresher, now: DateTime<Utc>) -> Option<Vec<(String, Outcome)>> {
    static FLIGHT: Mutex<()> = Mutex::new(());
    let _g = FLIGHT.try_lock().ok()?;
    if k2_core::airgap::enabled() {
        return Some(Vec::new());
    }
    let rows = match with_conn(auto_next::list) {
        Ok(r) => r,
        Err(e) => {
            k2_core::log_debug!("[llm-auto-next] settings not read: {e}");
            return Some(Vec::new());
        }
    };
    let mut out = Vec::new();
    for s in rows {
        let o = tool_tick(&s, vendor, now).unwrap_or_else(|e| Outcome::Failed(e.code().to_string()));
        out.push((s.tool.clone(), o));
    }
    Some(out)
}

/// Look now, off the caller's thread (a person just turned it on).
pub fn kick() {
    #[cfg(not(test))]
    {
        let _ = std::thread::Builder::new().name("llm-auto-next-kick".into()).spawn(|| {
            let _ = tick_with(&crate::llm_accounts_runtime::DaemonRefresher, Utc::now());
        });
    }
}

/// The loop (daemon boot). `K2_LLM_ACCOUNTS_LOOP=0` (the wallet's own
/// switch for its background work) keeps it off too.
pub fn spawn() {
    if std::env::var("K2_LLM_ACCOUNTS_LOOP").is_ok_and(|v| v == "0") {
        return;
    }
    let _ = std::thread::Builder::new().name("llm-auto-next".into()).spawn(|| loop {
        std::thread::sleep(TICK);
        let _ = tick_with(&crate::llm_accounts_runtime::DaemonRefresher, Utc::now());
    });
}

#[cfg(test)]
#[path = "llm_auto_next_tests.rs"]
mod tests;
