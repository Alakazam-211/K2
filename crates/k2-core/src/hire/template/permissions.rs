//! The permissions step of applying a template (prd-agent-templates-v1 §3
//! "Permissions", §9; slice T3).
//!
//! A template carries no authority: it only *suggests* standing switches
//! (and "Can ask for permission cards" when `cards = true`). The hirer's
//! own reach decides each one:
//!
//! - the person ticked it off on the Review step (or `--decline`):
//!   `declined`, nothing happens;
//! - it is already on for the new agent: `skip` ("already on");
//! - the hirer may give it: `grant` in a dry run, `done` when turned on;
//! - otherwise, while the server's permission cards (Beta) are on: `card`,
//!   a permission card for a person (filed on the agent's Thread, or when
//!   the agent first starts if it has no Thread yet);
//! - otherwise `refused`, with a `hint` that says who can turn it on and
//!   how. A refusal never fails the hire.
//!
//! The switch catalog, the writers, cards and the audit live in the daemon
//! (`k2-daemon` `hire_permissions`), which installs the step at start
//! ([`install`]). Without it (k2-core's own tests) every suggestion is
//! reported as `suggest`, and nothing is granted.
//!
//! Re-applying a template only turns on switches that are still off and
//! allowed; it never turns one off, and never asks twice on a card.

use std::sync::OnceLock;

use serde::Serialize;

use super::super::{HireCaller, HireError};
use super::format::Template;

/// The row target for the template's `cards = true`.
pub const CARDS_ROW: &str = "permission-cards";

/// One permission row: `target` is an action id or [`CARDS_ROW`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub target: String,
    /// `grant` (dry run), `done`, `card`, `refused`, `skip`, `declined`,
    /// or `suggest` (no step installed, or the door grants its own set).
    pub status: &'static str,
    pub reason: Option<String>,
    /// On `refused`: who can turn it on, and how.
    pub hint: Option<String>,
}

impl Decision {
    pub fn new(target: impl Into<String>, status: &'static str, reason: Option<String>, hint: Option<String>) -> Self {
        Self { target: target.into(), status, reason, hint }
    }
}

/// A switch the hire couldn't give and couldn't ask for.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Refusal {
    pub action: String,
    pub code: String,
    pub hint: String,
}

/// What the step works from.
#[derive(Debug, Clone, Copy)]
pub struct StepIn<'a> {
    pub caller: &'a HireCaller,
    /// The new agent's folder (registered unless this is a dry run of a
    /// new folder).
    pub path: &'a str,
    /// The template's display name, for the card's words.
    pub template_name: &'a str,
    /// The template turns on "Can ask for permission cards".
    pub cards: bool,
    /// `suggest`, sets expanded.
    pub suggested: &'a [String],
    /// What the person unticked (action ids, or [`CARDS_ROW`]).
    pub declined: &'a [String],
    /// Asked on a card by an earlier apply (the lock's `carded`): never
    /// asked again.
    pub asked_before: &'a [String],
    pub dry_run: bool,
}

/// What the step did (or, in a dry run, would do).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepOut {
    pub rows: Vec<Decision>,
    /// Turned on by this apply.
    pub granted: Vec<String>,
    /// Asked on a card by this apply (filed, or waiting for the Thread).
    pub carded: Vec<String>,
    /// Cards still to file when the agent first starts (no Thread yet).
    pub cards_pending: Vec<String>,
    pub refused: Vec<Refusal>,
}

static STEP: OnceLock<fn(&StepIn<'_>) -> StepOut> = OnceLock::new();

/// The daemon installs the real step at start. Later calls are ignored.
pub fn install(f: fn(&StepIn<'_>) -> StepOut) {
    let _ = STEP.set(f);
}

/// Run the installed step, or report every suggestion as `suggest`.
pub(crate) fn run(inp: &StepIn<'_>) -> StepOut {
    match STEP.get() {
        Some(f) => f(inp),
        None => suggest_only(inp, None),
    }
}

/// Every suggestion as a `suggest` row (declined ones as `declined`):
/// nothing is decided or granted here.
pub fn suggest_only(inp: &StepIn<'_>, reason: Option<&str>) -> StepOut {
    let mut out = StepOut::default();
    for target in targets(inp.cards, inp.suggested) {
        let declined = inp.declined.iter().any(|d| *d == target);
        out.rows.push(Decision::new(
            target,
            if declined { "declined" } else { "suggest" },
            if declined { Some("unticked at hire".into()) } else { reason.map(str::to_string) },
            None,
        ));
    }
    out
}

/// The rows a template's permissions make, in order: the cards switch
/// first (it decides whether the agent may ask later), then `suggest`.
pub fn targets(cards: bool, suggested: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if cards {
        out.push(CARDS_ROW.to_string());
    }
    out.extend(suggested.iter().cloned());
    out
}

/// A declined id must be one this template suggests (a typo would
/// otherwise grant what the person meant to leave off).
pub fn check_declined(tpl: &Template, declined: &[String]) -> Result<(), HireError> {
    let allowed = targets(tpl.meta.permissions.cards, &tpl.suggested());
    for d in declined {
        if !allowed.iter().any(|a| a == d.trim()) {
            let list = if allowed.is_empty() { "nothing".to_string() } else { allowed.join(", ") };
            return Err(HireError::usage(format!(
                "--decline {d}: the {} template doesn't suggest it; it suggests: {list}",
                tpl.meta.name
            )));
        }
    }
    Ok(())
}

/// Trimmed, empty ones dropped, no repeats.
pub fn clean_declined(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in raw.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !out.iter().any(|o| o == d) {
            out.push(d.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_only_marks_declined_and_keeps_order() {
        let suggested = vec!["compute.use".to_string(), "apps.build".to_string()];
        let declined = vec!["apps.build".to_string()];
        let caller = HireCaller::Owner;
        let out = suggest_only(
            &StepIn {
                caller: &caller,
                path: "/x",
                template_name: "App owner",
                cards: true,
                suggested: &suggested,
                declined: &declined,
                asked_before: &[],
                dry_run: true,
            },
            None,
        );
        let got: Vec<(&str, &str)> = out.rows.iter().map(|r| (r.target.as_str(), r.status)).collect();
        assert_eq!(got, vec![(CARDS_ROW, "suggest"), ("compute.use", "suggest"), ("apps.build", "declined")]);
        assert!(out.granted.is_empty() && out.carded.is_empty());
        assert_eq!(clean_declined(&[" a ".into(), "a".into(), "".into(), "b".into()]), vec!["a", "b"]);
    }
}
