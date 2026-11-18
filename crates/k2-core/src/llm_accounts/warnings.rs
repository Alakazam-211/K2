//! Warn the server's owners before a Claude sign-in reaches its time
//! limit, so they can sign in again in time (Rosson 2026-11-18).
//!
//! Claude Code's sign-ins end after a fixed time (`refreshTokenExpiresAt`
//! in newer files; K2 keeps it as `llm_accounts.sign_in_ends_at`). At 7
//! days, 1 day and 2 hours before, and once it has ended, each owner
//! login sees one pop-up per token and step until they put it away
//! (`llm_sign_in_warnings`, migration 0315). The daemon owns all of it:
//! which step is due, and who put what away. A new sign-in has a new
//! `epoch`, so its warnings start over.

use rusqlite::{params, Connection};
use serde::Serialize;

use super::{list_for, state, Entry, Tool, WalletError};

/// The steps before the end, most urgent last: (name, seconds before).
pub const STEPS: &[(&str, i64)] = &[("7d", 7 * 86_400), ("1d", 86_400), ("2h", 2 * 3_600)];
/// The step once the sign-in has ended.
pub const ENDED: &str = "ended";
/// The earliest step: a sign-in ending within it gets its "sign in again
/// by" note on the token, and may be signed in again while still live.
pub const WARN_WITHIN_SECS: i64 = 7 * 86_400;

/// One warning due for one token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub account_id: String,
    pub tool: String,
    pub label: String,
    /// `7d`, `1d`, `2h` or `ended`.
    pub step: String,
    /// When the sign-in ends (or ended), unix seconds, when known.
    pub ends_at: Option<i64>,
    /// Which sign-in of the token this is about (see the module doc).
    pub epoch: i64,
}

/// The step due for a sign-in ending at `ends_at`, at `now`.
pub fn step_for(ends_at: i64, now: i64) -> Option<&'static str> {
    let left = ends_at - now;
    if left <= 0 {
        return Some(ENDED);
    }
    STEPS.iter().rev().find(|(_, before)| left <= *before).map(|(name, _)| *name)
}

/// The one warning due for `e` now, if any: the most urgent step reached.
/// A Claude subscription marked "sign in again" because its sign-in ended
/// (its end time passed, or its detail says so) gets the `ended` one.
pub fn due_for(e: &Entry, now: i64) -> Option<Warning> {
    if e.tool != Tool::Claude.as_str() || e.is_api_key() || e.removed_at.is_some() || e.state == state::SIGNING_IN || e.state == state::NOT_SET_UP {
        return None;
    }
    let step = if e.state == state::NEEDS_LOGIN {
        let ended = e.sign_in_ends_at.is_some_and(|x| x <= now) || e.detail.as_deref().is_some_and(super::store::ClaudeEnded::said_in);
        if !ended {
            return None;
        }
        ENDED
    } else {
        step_for(e.sign_in_ends_at?, now)?
    };
    let epoch = e.sign_in_ends_at.unwrap_or_else(|| e.refreshed_at.unwrap_or(e.created_at));
    Some(Warning {
        account_id: e.id.clone(),
        tool: e.tool.clone(),
        label: e.label.clone(),
        step: step.into(),
        ends_at: e.sign_in_ends_at,
        epoch,
    })
}

/// Every warning due now, one per token at most.
pub fn due(conn: &Connection, now: i64) -> Result<Vec<Warning>, WalletError> {
    Ok(list_for(conn, Tool::Claude)?.iter().filter_map(|e| due_for(e, now)).collect())
}

/// The warnings `user_key` (an owner login) hasn't put away.
pub fn pending_for(conn: &Connection, user_key: &str, now: i64) -> Result<Vec<Warning>, WalletError> {
    let mut out = Vec::new();
    for w in due(conn, now)? {
        let put_away: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM llm_sign_in_warnings WHERE account_id = ?1 AND threshold = ?2 AND epoch = ?3 AND user_key = ?4)",
            params![w.account_id, w.step, w.epoch, user_key],
            |r| r.get(0),
        )?;
        if !put_away {
            out.push(w);
        }
    }
    Ok(out)
}

/// `user_key` put this warning away ("Later", or "Sign in now"). Kept
/// across restarts; the same step of the same sign-in never shows again
/// for them.
pub fn dismiss(conn: &Connection, user_key: &str, account_id: &str, step: &str, epoch: i64, now: i64) -> Result<(), WalletError> {
    if step != ENDED && !STEPS.iter().any(|(s, _)| *s == step) {
        return Err(WalletError::InvalidLabel(format!("unknown warning step {step:?}")));
    }
    conn.execute(
        "INSERT OR IGNORE INTO llm_sign_in_warnings (account_id, threshold, epoch, user_key, dismissed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account_id, step, epoch, user_key, now],
    )?;
    Ok(())
}

/// "Sign in again by <when>": the token's sign-in ends within the first
/// step's window (and hasn't ended yet).
pub fn ends_soon(e: &Entry, now: i64) -> bool {
    e.sign_in_ends_at.is_some_and(|x| x > now && x - now <= WARN_WITHIN_SECS)
}
