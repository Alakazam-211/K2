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
//! allowed; it never turns one off, and never asks twice on a card. What
//! was asked or declined is the daemon's record (`hire::permission_records`),
//! never the agent's own `template.lock`.

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
    /// What the person unticked (action ids, or [`CARDS_ROW`]). A decline
    /// from an earlier hire is the daemon's record, not this list.
    pub declined: &'a [String],
    /// Suggestions a person names again on a re-apply (`--allow`): a
    /// decline recorded at an earlier hire no longer holds for these.
    pub allowed: &'a [String],
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

/// The installed step. An error stops the hire: the daemon's record of
/// earlier declines couldn't be read, and deciding without it could grant
/// what a person declined.
pub type Step = fn(&StepIn<'_>) -> Result<StepOut, HireError>;

static STEP: OnceLock<Step> = OnceLock::new();

/// The daemon installs the real step at start. Later calls are ignored.
pub fn install(f: Step) {
    let _ = STEP.set(f);
}

/// Run the installed step, or report every suggestion as `suggest`.
pub(crate) fn run(inp: &StepIn<'_>) -> Result<StepOut, HireError> {
    match STEP.get() {
        Some(f) => f(inp),
        None => Ok(suggest_only(inp, None)),
    }
}

/// Every suggestion as a `suggest` row (declined ones as `declined`):
/// nothing is decided or granted here.
pub fn suggest_only(inp: &StepIn<'_>, reason: Option<&str>) -> StepOut {
    let mut out = StepOut::default();
    for target in targets(inp.cards, inp.suggested) {
        let declined = inp.declined.contains(&target);
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
    check_ids(tpl, declined, "--decline")
}

/// `--allow` ids: suggested by the template, and not declined in the same
/// call.
pub fn check_allowed(tpl: &Template, allowed: &[String], declined: &[String]) -> Result<(), HireError> {
    check_ids(tpl, allowed, "--allow")?;
    if let Some(both) = allowed.iter().find(|a| declined.contains(a)) {
        return Err(HireError::usage(format!("{both} is both --allow and --decline: pick one")));
    }
    Ok(())
}

fn check_ids(tpl: &Template, ids: &[String], flag: &str) -> Result<(), HireError> {
    let suggested = targets(tpl.meta.permissions.cards, &tpl.suggested());
    for d in ids {
        if !suggested.iter().any(|a| a == d.trim()) {
            let list = if suggested.is_empty() { "nothing".to_string() } else { suggested.join(", ") };
            return Err(HireError::usage(format!(
                "{flag} {d}: the {} template doesn't suggest it; it suggests: {list}",
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
                allowed: &[],
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
