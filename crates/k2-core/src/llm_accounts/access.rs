//! How a subscription token's access token stands right now, for the list,
//! `status` and Settings.
//!
//! The stored `state` says whether the sign-in is good (`signed_in`) or
//! not (`needs_login`, `not_set_up`). It never said that the short-lived
//! ACCESS token had expired, so a server default whose CLIs hadn't run
//! for a day read plain `signed_in` while its access token was hours old.
//! That is not an outage: the sign-in (the refresh token) is still good,
//! and whoever uses the login next renews it first. But people and agents
//! should see it, and see that nothing needs doing.
//!
//! Three answers ([`Access`]):
//! - `fine`: signed in, the access token not yet expired (or no expiry
//!   recorded);
//! - `expired`: signed in, the access token expired at `expires_at`; the
//!   note says who renews it: the CLI on its next run for a login a CLI
//!   holds (the server default, or a token a running session uses), which
//!   K2 never refreshes itself (LT-A10, pins.rs "Rotation safety"); K2's
//!   keep-warm pass or a switch for an idle token;
//! - `signed_out`: `needs_login` / `not_set_up` (the token's `detail` says
//!   why: an ended sign-in, a refused refresh token, ...).
//!
//! `None` for API keys, a sign-in in progress and `unknown`. Read from the
//! row only: no credential is read here, and no token value is ever in the
//! answer. The server default's `expires_at` follows the live store on
//! every reconcile ([`super::wallet::reconcile_live`]).

use serde::Serialize;

use super::store::utc_words;
use super::{state, Entry, Tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Fine,
    Expired,
    SignedOut,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Fine => "fine",
            Access::Expired => "expired",
            Access::SignedOut => "signed_out",
        }
    }
}

/// [`access_of`]'s answer: the state, and for `expired` the one line that
/// says when and who renews it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessView {
    pub access: Option<Access>,
    pub note: Option<String>,
}

/// How `e`'s access token stands at `now` (unix seconds). `cli_owned`: a
/// CLI holds this login (it is the tool's live login, the server default,
/// or a running session's pinned token), so the CLI renews it, not K2.
pub fn access_of(e: &Entry, now: i64, cli_owned: bool) -> AccessView {
    let none = AccessView { access: None, note: None };
    if e.is_api_key() {
        return none;
    }
    let Some(tool) = e.tool() else { return none };
    match e.state.as_str() {
        state::NEEDS_LOGIN | state::NOT_SET_UP => AccessView { access: Some(Access::SignedOut), note: None },
        state::SIGNED_IN => {
            // Grok's sign-in carries no refresh by expiry (keep-warm runs
            // its own warm-up instead): its expiry is never "expired" here.
            let expired_at = e.expires_at.filter(|x| tool != Tool::Grok && *x <= now);
            match expired_at {
                None => AccessView { access: Some(Access::Fine), note: None },
                Some(at) => AccessView { access: Some(Access::Expired), note: Some(expired_note(tool, at, cli_owned)) },
            }
        }
        _ => none,
    }
}

/// The line for an expired access token whose sign-in is still good.
pub fn expired_note(tool: Tool, at: i64, cli_owned: bool) -> String {
    let who = if cli_owned {
        format!("{} renews it by itself the next time it runs on this token", tool.display())
    } else {
        "the next keep-warm pass (every 30 minutes) renews it, or a switch to it does".to_string()
    };
    format!("Its access token expired on {}. Its sign-in is still good: {who}. Nothing to do.", utc_words(at))
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
