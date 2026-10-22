//! E1 — `garden.keep-mine widget= version=` (PRD rev 3.1 §5.2, §5.2.1):
//! keep an agent-changed widget version as the person's, so it reads its
//! data with the person's rights again. K2 already presents a Keep-as-mine
//! card of its own (`action_cards::KEEP_WIDGET`); this is the same act as a
//! catalog action an agent may ask for.
//!
//! **Floor: `GardenOwner`**, the owner of that Garden from any signed-in
//! surface (the owner token, or an Owner login from the phone, the web or
//! another server). `apply` goes through `zen_routes::claim_entry`, which
//! runs `zen_routes::garden_owner_may`, the one resolver Settings' "It's
//! mine" uses too, so card and Settings stay one door (§6.4, C15).
//!
//! `version=` is the hash, or its first 12+ hex characters (the short
//! form K2 shows).
//!
//! The current state (§6.2): the live version must still be `version`
//! (else `stale`: the widget changed since the ask); a version that is
//! already the person's is `already_done`.

//!
//! **Garden access (A4, prd-garden-access-v1 G16–G18).** Three more rows,
//! listed in every switch state (G0a item 4):
//! - `garden.access garden=` (standing, with its `garden.access-off`
//!   twin): put the asking agent on the Garden's list, through
//!   `garden_cards::access_entry`, the one door Settings → Gardens uses;
//! - `garden.delete garden=` (one time): delete the Garden as the person,
//!   through `garden_cards::delete_entry` (the Settings route's own
//!   `garden/delete`).
//!
//! While `gardenAccess` is off their checks answer `not_available`; while
//! it is on `garden.keep-mine` (and E2's `garden.proposal.approve`) answer
//! `retired`. K2 presents the two cards itself (`garden_cards::present`).

use k2_core::garden_data::MadeBy;
use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NeverReason, NotCardable, NotCardableSpec, ParamFrom, ParamKind,
    ParamSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};

use super::switches::{applied, as_caller, param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::zen_garden_cards as cards;
use k2_core::zen::garden_access::{AskKind, Via};

/// Where Settings' "It's mine" lives.
const LOCAL_DATA: SettingsRef = SettingsRef { page: "gardens/local-data", path: "Settings → Gardens → Local Data" };

pub(crate) static KEEP_MINE: ActionSpec = ActionSpec {
    id: "garden.keep-mine",
    area: Area::Gardens,
    params: &[
        ParamSpec { key: "widget", kind: ParamKind::Widget, required: true, list: false, choices: &[] },
        ParamSpec { key: "version", kind: ParamKind::Version, required: true, list: false, choices: &[] },
    ],
    floor: Floor::GardenOwner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Keep as mine",
        what_changes: "Keep {ws}'s change to the {widget} widget as yours",
        consequences: "This version of {widget} reads its data with your rights. Undo in Settings → Gardens → Local Data.",
        scope: "the {widget} widget",
        duration: "once",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: LOCAL_DATA,
    // K2-presented and asked for; no refusal names it (the claim route is
    // the person's own, an agent can't reach it).
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

/// A widget version on a card is its hash or the hash's first characters
/// (at least this many hex digits: the short form K2 shows people).
pub(crate) const MIN_VERSION: usize = 12;

fn version_ok(v: &str) -> bool {
    v.len() >= MIN_VERSION && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `retired` while Garden access lists are on (G18): Keep as mine and
/// proposals give way to the list. The rows stay registered while the old
/// path exists (GA10).
pub(crate) fn retired() -> Option<Check> {
    cards::garden_access_on().then(|| Check::Unavailable { code: "retired", why: cards::RETIRED_WORDS.to_string() })
}

fn check_keep(ctx: &Ctx) -> Check {
    if let Some(r) = retired() {
        return r;
    }
    let (Some(widget), Some(version)) = (param(ctx, "widget"), param(ctx, "version")) else {
        return Check::Moved("name the widget and its version: widget=<name> version=<hash>".into());
    };
    if !version_ok(version) {
        return Check::Moved(format!("version= is the first {MIN_VERSION}+ characters of {widget}'s version (k2 zen widgets shows it)"));
    }
    match crate::zen_routes::widget_live_version(widget) {
        Err(m) => Check::Moved(m),
        Ok((live, _)) if !live.starts_with(version) => {
            Check::Moved(format!("{widget} changed since this was asked (its live version is now {}); ask again", &live[..live.len().min(12)]))
        }
        Ok((_, MadeBy::Owner)) => Check::AlreadyDone,
        Ok(_) => Check::Needed,
    }
}

fn apply_keep(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (Some(widget), Some(version)) = (param(ctx, "widget"), param(ctx, "version")) else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no widget or version".into() };
    };
    let r = crate::zen_routes::claim_entry(&as_caller(who), widget, Some(version), "permission-card");
    applied(&r, "Kept as yours. Undo in Settings → Gardens → Local Data.")
}

// ── garden.access, its twin, garden.delete (A4, G16) ────────────────

/// Settings → Gardens, where the list lives (A5's master-detail page).
const GARDENS: SettingsRef = SettingsRef { page: "zen-gardens", path: "Settings \u{2192} Gardens \u{2192} {garden}" };

const GARDEN: &[ParamSpec] = &[ParamSpec { key: "garden", kind: ParamKind::Garden, required: true, list: false, choices: &[] }];

pub(crate) static ACCESS: ActionSpec = ActionSpec {
    id: cards::ACCESS,
    area: Area::Gardens,
    params: GARDEN,
    floor: Floor::GardenOwner,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Edit {garden}",
        what_changes: "Let {ws} edit {garden}",
        consequences: "{ws} can change {garden}'s layout and widgets, and use their data. Remove {ws} in Settings \u{2192} Gardens.",
        scope: "the {garden} Garden",
        duration: "until you remove it",
        retry: "run your command again",
    },
    undo: Undo::Twin { action: cards::ACCESS_OFF },
    settings: GARDENS,
    // A2's refusal for an unlisted passport on a Garden or widget write
    // names the Garden (`garden`; null for the computer-wide look).
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "garden_access_needed" },
        route_prefix: "/cli/zen/",
        method: WallMethod::Any,
        params: &[("garden", ParamFrom::Response("/garden"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The tighten twin: asked only (no wall), and "Turn off again" on an
/// approved `garden.access` card runs it.
pub(crate) static ACCESS_OFF: ActionSpec = ActionSpec {
    id: cards::ACCESS_OFF,
    area: Area::Gardens,
    params: GARDEN,
    floor: Floor::GardenOwner,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Edit {garden}",
        what_changes: "Stop {ws} editing {garden}",
        consequences: "{ws} can't change {garden} any more. Add {ws} again in Settings \u{2192} Gardens.",
        scope: "the {garden} Garden",
        duration: "until you add it again",
        retry: "",
    },
    undo: Undo::None,
    settings: GARDENS,
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

pub(crate) static DELETE: ActionSpec = ActionSpec {
    id: cards::DELETE,
    area: Area::Gardens,
    params: GARDEN,
    floor: Floor::GardenOwner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Delete Garden",
        what_changes: "Delete {garden}",
        consequences: "{garden} leaves your Gardens. K2 keeps a copy in history.",
        scope: "the {garden} Garden",
        duration: "once",
        // PRD G16 lists "ask again after 24 hours" here; that is the Keep
        // it rule (the denied turn says it), not a tail for an approval.
        retry: "",
    },
    undo: Undo::None,
    settings: GARDENS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "garden_delete_person" },
        route_prefix: "/cli/zen/garden/delete",
        method: WallMethod::Post,
        params: &[("garden", ParamFrom::Body("/garden"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The Garden every Garden card names. `Err` is the check to answer:
/// `not_available` while the switch is off, or no Garden named.
fn garden_of(ctx: &Ctx) -> Result<String, Check> {
    if !cards::garden_access_on() {
        return Err(Check::Unavailable { code: "not_available", why: cards::OFF_WORDS.to_string() });
    }
    let Some(g) = param(ctx, "garden") else {
        return Err(Check::Moved("name the Garden: garden=<its id> (k2 zen gardens)".into()));
    };
    Ok(g.to_string())
}

fn gone(garden: &str) -> Check {
    Check::Moved(format!("the Garden {garden} is gone"))
}

fn check_access(ctx: &Ctx) -> Check {
    let g = match garden_of(ctx) {
        Ok(g) => g,
        Err(c) => return c,
    };
    if cards::garden_name(&g).is_none() {
        return gone(&g);
    }
    if cards::is_listed(&g, &ctx.workspace_id) {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

fn check_access_off(ctx: &Ctx) -> Check {
    let g = match garden_of(ctx) {
        Ok(g) => g,
        Err(c) => return c,
    };
    if cards::garden_name(&g).is_none() {
        return gone(&g);
    }
    if cards::is_listed(&g, &ctx.workspace_id) {
        Check::Needed
    } else {
        Check::AlreadyDone
    }
}

/// §1.5a: a Garden that's gone is `already_done`; only an agent on the
/// list, or the Garden's maker, may ask (else the values moved); the last
/// Garden can't go.
fn check_delete(ctx: &Ctx) -> Check {
    let g = match garden_of(ctx) {
        Ok(g) => g,
        Err(c) => return c,
    };
    let Some(name) = cards::garden_name(&g) else { return Check::AlreadyDone };
    if !cards::may_ask_delete(&ctx.workspace_id, &g) {
        return Check::Moved(format!(
            "{} isn't on {name}'s list and didn't make it; ask for garden.access first",
            ctx.workspace_handle
        ));
    }
    if cards::is_last_garden() {
        return Check::Moved(format!("{name} is the last Garden; K2 always keeps one"));
    }
    Check::Needed
}

fn no_garden() -> Applied {
    Applied::Refused { code: "bad_params".into(), message: "the request names no Garden".into() }
}

/// Allow: the asking agent goes on the list through A2's one door
/// (`list_change`: the GardenOwner check, A1's store, the ask cleared),
/// with the card that asked.
fn apply_access(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let Some(g) = param(ctx, "garden") else { return no_garden() };
    let card = cards::asked_card(g, &ctx.workspace_id, AskKind::Access);
    let r = crate::zen_access_routes::list_change(&as_caller(who), g, &ctx.workspace_id, true, Via::Card, card.as_deref());
    let name = cards::garden_name(g).unwrap_or_else(|| g.to_string());
    applied(&r, format!("Allowed. {} can edit {name} now. Remove it in Settings \u{2192} Gardens.", ctx.workspace_handle))
}

/// The twin ("Turn off again", or asked): off the list through the same door.
fn apply_access_off(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let Some(g) = param(ctx, "garden") else { return no_garden() };
    let r = crate::zen_access_routes::list_change(&as_caller(who), g, &ctx.workspace_id, false, Via::Twin, None);
    let name = cards::garden_name(g).unwrap_or_else(|| g.to_string());
    applied(&r, format!("Removed. {} can't edit {name} any more.", ctx.workspace_handle))
}

fn apply_delete(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let Some(g) = param(ctx, "garden") else { return no_garden() };
    let r = cards::delete_entry(&as_caller(who), g, "permission-card");
    applied(&r, "Deleted. K2 keeps a copy in history.")
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &KEEP_MINE, check: check_keep, apply: apply_keep },
        Registered { spec: &ACCESS, check: check_access, apply: apply_access },
        Registered { spec: &ACCESS_OFF, check: check_access_off, apply: apply_access_off },
        Registered { spec: &DELETE, check: check_delete, apply: apply_delete },
    ]
}

/// The Garden and data-source rows that aren't v1 cards (§5.4, §9;
/// Appendix A11).
pub(crate) static NOT_CARDABLE: [NotCardableSpec; 6] = [
    // Settings widened to GardenOwner now; the card row is "later" (trivial).
    NotCardableSpec {
        id: "garden.widget.resume",
        area: Area::Gardens,
        why: NotCardable::Later,
        r#where: "the K2 app (click Resume on the widget) or the phone, signed in as the owner",
    },
    // Ruling 6: the lock is the person's own tool, owner token only.
    NotCardableSpec {
        id: "garden.lock",
        area: Area::Gardens,
        why: NotCardable::Never { reason: NeverReason::PersonalTool },
        r#where: "Settings → Gardens, on this computer",
    },
    // RS-A15's own sheet.
    NotCardableSpec {
        id: "garden.guests",
        area: Area::Gardens,
        why: NotCardable::Never { reason: NeverReason::Secret },
        r#where: "the widget's own guest sheet, by the owner",
    },
    NotCardableSpec {
        id: "sources.allow-lan",
        area: Area::Gardens,
        why: NotCardable::Dropped,
        r#where: "Settings → Gardens → External Data (owner)",
    },
    NotCardableSpec {
        id: "sources.bind-secret",
        area: Area::Gardens,
        why: NotCardable::Dropped,
        r#where: "Settings → Gardens → External Data",
    },
    NotCardableSpec {
        id: "sources.field-limits",
        area: Area::Gardens,
        why: NotCardable::Dropped,
        r#where: "Settings → Gardens → External Data",
    },
];

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    NOT_CARDABLE.iter().collect()
}
