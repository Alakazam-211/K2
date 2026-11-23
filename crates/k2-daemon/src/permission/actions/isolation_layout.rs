//! `agent.isolation-layout` (Agent Isolation M2 B4): turn on a root-owned
//! scope for every agent session on this server.
//!
//! The switch is `POST /cli/agent-isolation/layout {mode: "on"}`
//! (`crate::agent_isolation_layout`), the owner's, with this computer's
//! owner check (`presence::ISOLATION_LAYOUT`). An agent's own `k2 agent
//! isolation layout on` is refused `owner_only` and ends with "Ask with: k2
//! permission request agent.isolation-layout" (the wall in `walls::ROWS`).
//!
//! - **Who answers:** the owner (the owner token or an Owner login).
//! - **Proof:** a click. The click is not this computer's owner check, and
//!   the card flow has no step that gives one (a Settings confirm is the
//!   vault's, which `presence` never takes, and a tightening takes no
//!   confirm in the catalog). So **Allow** runs the route's entry as the
//!   person who clicked ([`crate::agent_isolation_layout::card_entry`]),
//!   which asks for the owner check again: this computer's own prompt where
//!   it has one (a Mac, a Windows PC), or the answer's `confirmToken` from
//!   `/cli/vault/confirm/finish`. Without it (a Linux server's usual case)
//!   the request fails with the route's own words: run `k2 agent isolation
//!   layout on` yourself. The card says so up front.
//! - **Undo:** none from the card; `k2 agent isolation layout off` is the
//!   owner's own (it loosens).

use k2_core::cli_name;
use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch,
    WallMethod, Words,
};

use super::switches::applied;
use super::{Answerer, Applied, Check, Ctx, Registered};
use k2_core::agent_scope::{self, OwnerChoice};

pub(crate) const ACTION: &str = "agent.isolation-layout";

pub(crate) static ISOLATION_LAYOUT: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Box,
    params: &[],
    floor: Floor::Owner,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "A root-owned scope for every agent session",
        what_changes: "Turn on a root-owned scope for every agent session on this server",
        consequences: concat!(
            "For the whole server, not only {ws}: each new agent session, job run and service starts in its own scope, placed by the root helper. Running sessions keep their place until they restart. ",
            "It needs this computer's owner check, which this card can't give: on a Linux server, run ",
            cli_name!(),
            " agent isolation layout on yourself, from a terminal of your own (it asks for an owner login's password). Turning it off is yours too: ",
            cli_name!(),
            " agent isolation layout off."
        ),
        scope: "this server",
        duration: "until the owner turns it off",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "general", path: concat!(cli_name!(), " agent isolation layout on|off|status (a terminal outside K2)") },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: crate::agent_isolation_layout::ROUTE,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

fn check(_ctx: &Ctx) -> Check {
    if agent_scope::scope_placement() && agent_scope::owner_choice() == OwnerChoice::On {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

fn apply(_ctx: &Ctx, who: &Answerer, confirm: Option<&str>) -> Applied {
    let r = crate::agent_isolation_layout::card_entry(who, confirm);
    applied(
        &r,
        "Agent scopes are on for this server: each new agent session, job run and service gets its own root-owned scope at its next start.",
    )
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &ISOLATION_LAYOUT, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
