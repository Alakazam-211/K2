//! The two starter permission sets: what K2 grants **K5** (K2's own agent)
//! and the person's **Chief of Staff** (K5's first hire) when they are made
//! (prd-setup-checklist-v1 §10.2; contract `k5-first-run-contract.md`).
//!
//! **Rosson-accepted 11-10; changing either list needs his OK.** Each list is
//! one line to change, and each is granted only to its own workspace.
//!
//! Both lists hold **standing V1 action ids only** (`V1_ACTIONS`,
//! `k2-daemon` `permission/walls.rs`; a daemon test checks each one is a
//! standing action in the real catalog). The rules every grant follows
//! (`k2-daemon` `first_run_grants`):
//! - **Standing only.** Only standing switches are granted ahead of time.
//!   These one-time actions stay card asks and are never in a list
//!   ([`CARD_ASKS`]): `heartbeat.pause-other`, `heartbeat.resume-other`,
//!   `sidecar.retire`, `connections.add`.
//! - **`chat.refresh`** is one-time too, and its standing twin, the
//!   `chat_refresh` toggle ("Can refresh its own pinned chat"), has no V1
//!   action id. So it is not in either list: Rosson's sets include it, and
//!   the first-run routes grant the toggle beside the list
//!   ([`EXTRA_STANDING_TOGGLES`]), under the same rules.
//! - **The creator's floor.** A creator grants only what they hold: an Admin
//!   creating K5 leaves out `organization.manage` (Owner floor,
//!   `walls.rs:818`), and K5 asks an Owner for it on a card.
//! - **The hirer holds it.** An agent hirer (K5 hiring the Chief of Staff)
//!   passes on only the standing switches it holds itself, never an
//!   Owner-floor one.
//! - **No silent widening.** K2 never widens either list on upgrade: these
//!   are applied once, when the agent is made.
//!
//! Everything else (every `vault.*`, `mail.*`, `dns.manage`, `apps.*`,
//! `app.*`, `publish.*`, `sew.*`, `db.agent-access`, `docs.share`,
//! `git.land`, `zen.sign*`, `agent.harness-switch`, `chat.account`,
//! `chat.manage`, `sim.use`, every `garden.*` approval and every other
//! `compute.*`) is asked for on a card: K5 reads untrusted input, so
//! secrets, sending as the person, the network edge, Apps and users, money
//! and deletion stay one click away.

/// K5's starter set (granted by its creator: the owner token, an Owner or
/// an Admin).
pub const K5_STARTER_PERMISSIONS: &[&str] = &[
    "agents.manage",
    "projects.manage",
    "connections.create",
    "organization.manage",
    "compute.use",
    "gardens.manage",
];

/// The Chief of Staff's starter set (granted by K5 when it hires it, each
/// only while K5 holds it).
pub const CHIEF_OF_STAFF_STARTER_PERMISSIONS: &[&str] = &[
    "agents.manage",
    "connections.create",
    "projects.manage",
    "compute.use",
];

/// Standing toggles in Rosson's sets that have no V1 action id, granted by
/// the routes beside the lists: `(V1 one-time action it stands for, toggle)`.
/// Both K5 and the Chief of Staff get it.
pub const EXTRA_STANDING_TOGGLES: &[(&str, &str)] = &[("chat.refresh", "chat_refresh")];

/// One-time actions the PRD's first lists named. They have no standing
/// state, so they are never granted ahead of time: the agent asks on a card
/// each time.
pub const CARD_ASKS: &[&str] =
    &["heartbeat.pause-other", "heartbeat.resume-other", "sidecar.retire", "connections.add", "chat.refresh"];

/// A set by the name a template's `suggest = ["set:<name>"]` uses.
pub fn named_set(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "k5-starter" => Some(K5_STARTER_PERMISSIONS),
        "chief-of-staff-starter" => Some(CHIEF_OF_STAFF_STARTER_PERMISSIONS),
        _ => None,
    }
}

/// The standing switch a starter id is granted as: the `/cli/agent-access/set`
/// toggle, or `agents_can_create_connections` (the per-workspace column
/// `POST /cli/agents-create-connections` writes) for `connections.create`.
pub fn standing_switch(action: &str) -> Option<&'static str> {
    Some(match action {
        "agents.manage" => "agents_manage",
        "projects.manage" => "projects_manage",
        "connections.create" => "agents_can_create_connections",
        "organization.manage" => "org_chart_manage",
        "compute.use" => "compute",
        "gardens.manage" => "gardens_manage",
        // The one-time `chat.refresh`'s standing twin (not in the lists; see
        // [`EXTRA_STANDING_TOGGLES`]).
        "chat.refresh" => "chat_refresh",
        _ => return None,
    })
}

/// Only an Owner (or the owner token) may grant it.
pub fn owner_floor(action: &str) -> bool {
    action == "organization.manage"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_id_has_a_standing_switch_and_no_card_ask_is_listed() {
        for id in K5_STARTER_PERMISSIONS.iter().chain(CHIEF_OF_STAFF_STARTER_PERMISSIONS) {
            assert!(standing_switch(id).is_some(), "{id} has no standing switch");
            assert!(!CARD_ASKS.contains(id), "{id} is one-time: a card ask, never in a list");
        }
        for (action, toggle) in EXTRA_STANDING_TOGGLES {
            assert_eq!(standing_switch(action), Some(*toggle));
        }
        // The Chief of Staff's set is inside K5's, so K5 can pass all of it on.
        for id in CHIEF_OF_STAFF_STARTER_PERMISSIONS {
            assert!(K5_STARTER_PERMISSIONS.contains(id), "{id}");
            assert!(!owner_floor(id), "{id}: an agent hirer never passes an Owner-floor switch");
        }
        assert_eq!(named_set("k5-starter"), Some(K5_STARTER_PERMISSIONS));
        assert_eq!(named_set("chief-of-staff-starter"), Some(CHIEF_OF_STAFF_STARTER_PERMISSIONS));
        assert_eq!(named_set("nope"), None);
    }
}
