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
//! `version=` is the hash's first 12+ hex characters (the short form K2
//! shows; a full hash is refused as secret-shaped).
//!
//! The current state (§6.2): the live version must still be `version`
//! (else `stale`: the widget changed since the ask); a version that is
//! already the person's is `already_done`.

use k2_core::garden_data::MadeBy;
use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NeverReason, NotCardable, NotCardableSpec, ParamKind, ParamSpec,
    Proof, SettingsRef, Undo, WallCoverage, Words,
};

use super::switches::{applied, as_caller, param};
use super::{Answerer, Applied, Check, Ctx, Registered};

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

/// A widget version on a card is its hash's first characters (at least
/// this many hex digits): a full 64-hex hash is refused as secret-shaped by
/// the request checks, and the short form is what K2 shows people.
pub(crate) const MIN_VERSION: usize = 12;

fn version_ok(v: &str) -> bool {
    v.len() >= MIN_VERSION && v.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check_keep(ctx: &Ctx) -> Check {
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

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &KEEP_MINE, check: check_keep, apply: apply_keep }]
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
