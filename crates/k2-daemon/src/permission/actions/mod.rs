//! The catalog's daemon half (PRD rev 3.1 §5, §6.4, §12.3): one file per
//! E-lane area, each pairing its `k2_core::permission::catalog::ActionSpec`s
//! with `check` and `apply`.
//!
//! **Frozen at P0 (lane A1).** Every area file is pre-declared below and
//! exports `actions()` / `not_cardable()`; lanes fill only their own files
//! and never edit this one. Contract changes go through A1.
//!
//! | File | Lane | v1 rows |
//! |---|---|---|
//! | `switches.rs` | E1 | the Agent tab's switches (vault, DNS and mail included) + `to=off` |
//! | `connections.rs` | E1 | `connections.create`, `connections.add` |
//! | `gardens.rs` | E1, Garden access A4 | `garden.keep-mine`; `garden.access` (+ `-off`), `garden.delete` |
//! | `held.rs` | E2 | `apps.approve`, `mail.approve`, `garden.proposal.approve`, `sources.approve` |
//! | `compute.rs` | E2 | `compute.admin-approve` (cut if late) |
//! | `sew.rs` | Sew (Rosson 2026-10-28) | `sew.owner-handoff`, `sew.rollback-data-loss` |
//! | `harness.rs` | 0.46.6 | `agent.harness-switch` (move the pinned chat to another harness) |
//! | `vault.rs` | 0.46.6 | `vault.delivery` (let the agent read one of its secrets as file or env) |
//! | `heartbeats.rs` | 0.46.6 | `heartbeat.pause-other`, `heartbeat.resume-other` (another workspace's heartbeat) |
//! | `password_reset.rs` | 0.46.6 | `apps.user-password-reset` (reset an App user's password; the secret goes to the approver only) |
//! | `workbench.rs` | 0.46.7 | `git.land`, `sidecar.retire` (a person lands / retires an agent's worktree sidecar) |
//!
//! Rules for every `apply` (§6.4, C15): call the Settings route's **entry**
//! with the answerer as the actor, so the route's own floor check runs
//! again at the click. Never the inner writer, never a second
//! implementation.

use k2_core::permission::catalog::{ActionSpec, NotCardableSpec};
use serde_json::{Map, Value};

pub(crate) mod compute;
pub(crate) mod connections;
pub(crate) mod gardens;
pub(crate) mod harness;
pub(crate) mod heartbeats;
pub(crate) mod held;
pub(crate) mod password_reset;
pub(crate) mod sew;
pub(crate) mod switches;
pub(crate) mod vault;
pub(crate) mod workbench;

/// The asking side, fixed at request time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ctx {
    /// The asking workspace id (always from the passport).
    pub workspace_id: String,
    /// Its handle, for the card words.
    pub workspace_handle: String,
    /// The action's values, as stored on the request.
    pub params: Map<String, Value>,
}

/// Who clicked, as the route entry's own caller (§6.4, §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answerer {
    /// The owner token (desktop app, terminal outside K2). Audited `via: owner-token`.
    OwnerToken,
    /// A Connect login with its role (`owner`, `admin`, `member`).
    Login { user: String, role: String },
}

impl Answerer {
    /// The confirm-token stamp the writer checks (`owner` / `connect:<user>`).
    pub fn stamp(&self) -> String {
        match self {
            Answerer::OwnerToken => "owner".to_string(),
            Answerer::Login { user, .. } => format!("connect:{user}"),
        }
    }
}

/// The result of `check`: at request time, at render (cached 2 s) and at
/// the click (§4.2, §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Check {
    /// The change is still needed and the values hold.
    Needed,
    /// The current Settings state already matches (`already_done`).
    AlreadyDone,
    /// Values that moved: widget version, held request decided elsewhere,
    /// workspace gone (`bad_params` at request time; `stale` at the click).
    Moved(String),
    /// The action isn't offered on this server right now, whatever the
    /// values: `not_available` (Garden access lists are off) or `retired`
    /// (Gardens use access lists now). 409 with `code` at request time;
    /// `stale` with `why` at the click; drawn as stored at render.
    Unavailable { code: &'static str, why: String },
}

/// The result of `apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Applied; the message goes on the card and into the agent's turn.
    Done { message: String },
    /// The route entry refused (floor, confirm, state): nothing applied,
    /// the request goes to `failed`.
    Refused { code: String, message: String },
}

/// One catalog action with its daemon half.
pub(crate) struct Registered {
    pub spec: &'static ActionSpec,
    pub check: fn(&Ctx) -> Check,
    /// `confirm_token` is the Settings confirm the answer carried, if any.
    pub apply: fn(&Ctx, &Answerer, Option<&str>) -> Applied,
}

/// Every card-able action, in lane order.
pub(crate) fn all() -> Vec<Registered> {
    let mut out = Vec::new();
    out.extend(switches::actions());
    out.extend(connections::actions());
    out.extend(gardens::actions());
    out.extend(held::actions());
    out.extend(compute::actions());
    out.extend(sew::actions());
    out.extend(harness::actions());
    out.extend(vault::actions());
    out.extend(heartbeats::actions());
    out.extend(password_reset::actions());
    out.extend(workbench::actions());
    out
}

/// Every row the lanes name as not card-able in v1 (§5.4, §9).
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    let mut out = Vec::new();
    out.extend(switches::not_cardable());
    out.extend(connections::not_cardable());
    out.extend(gardens::not_cardable());
    out.extend(held::not_cardable());
    out.extend(compute::not_cardable());
    out.extend(sew::not_cardable());
    out.extend(harness::not_cardable());
    out.extend(vault::not_cardable());
    out.extend(heartbeats::not_cardable());
    out.extend(password_reset::not_cardable());
    out.extend(workbench::not_cardable());
    out
}

/// The registered action with this id.
pub(crate) fn find(id: &str) -> Option<Registered> {
    all().into_iter().find(|r| r.spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Holds for every lane as it fills its file: each spec validates, ids
    /// are unique, and no id is both card-able and not.
    #[test]
    fn the_registry_is_well_formed() {
        let all = all();
        for r in &all {
            k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        }
        let mut ids: Vec<&str> = all.iter().map(|r| r.spec.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate action ids");
        for nc in not_cardable() {
            assert!(!ids.contains(&nc.id), "{} is both card-able and not", nc.id);
        }
        assert!(find("no.such-action").is_none());
    }

    #[test]
    fn the_answerer_stamp_matches_the_confirm_broker() {
        assert_eq!(Answerer::OwnerToken.stamp(), "owner");
        assert_eq!(Answerer::Login { user: "rosson".into(), role: "owner".into() }.stamp(), "connect:rosson");
    }
}
