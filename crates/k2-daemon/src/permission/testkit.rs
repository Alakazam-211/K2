//! A1 test kit: catalog actions that exist only under test, so the core
//! (request, answer, CAS, batches, expiry, limits) is tested without any
//! E-lane action. Their "Settings" state is an in-memory set per
//! (workspace, action); `apply` counts how often it ran.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NeverReason, NotCardable, NotCardableSpec, ParamKind,
    ParamSpec, Proof, SettingsRef, Undo, WallCoverage, Words,
};

use super::actions::{Answerer, Applied, Check, Ctx, Registered};

static ON: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static APPLIED: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
static MOVED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static LAST_ACTOR: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn key(ws: &str, action: &str) -> String {
    format!("{ws}|{action}")
}

/// Turn the fake setting on or off (as if in Settings).
pub(crate) fn set_on(ws: &str, action: &str, on: bool) {
    let mut g = ON.lock().expect("testkit on");
    let s = g.get_or_insert_with(HashSet::new);
    if on {
        s.insert(key(ws, action));
    } else {
        s.remove(&key(ws, action));
    }
}

pub(crate) fn is_on(ws: &str, action: &str) -> bool {
    ON.lock().expect("testkit on").as_ref().is_some_and(|s| s.contains(&key(ws, action)))
}

/// How many times `apply` ran for (workspace, action).
pub(crate) fn applied(ws: &str, action: &str) -> usize {
    APPLIED.lock().expect("testkit applied").as_ref().and_then(|m| m.get(&key(ws, action)).copied()).unwrap_or(0)
}

/// Who the last apply ran as (`owner` / `connect:<user>`).
pub(crate) fn last_actor(ws: &str, action: &str) -> Option<String> {
    LAST_ACTOR.lock().expect("testkit actor").as_ref().and_then(|m| m.get(&key(ws, action)).cloned())
}

/// Make `test.moved`'s values move (widget changed, held request decided).
pub(crate) fn set_moved(ws: &str, moved: bool) {
    let mut g = MOVED.lock().expect("testkit moved");
    let s = g.get_or_insert_with(HashSet::new);
    if moved {
        s.insert(ws.to_string());
    } else {
        s.remove(ws);
    }
}

fn count(ws: &str, action: &str, who: &Answerer) {
    let mut g = APPLIED.lock().expect("testkit applied");
    *g.get_or_insert_with(HashMap::new).entry(key(ws, action)).or_insert(0) += 1;
    LAST_ACTOR.lock().expect("testkit actor").get_or_insert_with(HashMap::new).insert(key(ws, action), who.stamp());
}

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{ws}/agent", path: "Settings → Workspaces → {ws} → Agent" };

const fn words(setting: &'static str, what: &'static str, duration: &'static str) -> Words {
    Words {
        setting,
        what_changes: what,
        consequences: "Test consequences for {ws}.",
        scope: "{ws}",
        duration,
        retry: "run your command again",
    }
}

const fn switch(id: &'static str, floor: Floor, setting: &'static str, what: &'static str) -> ActionSpec {
    ActionSpec {
        id,
        area: Area::Switches,
        params: &[],
        floor,
        direction: Direction::Loosen,
        grant: Grant::Standing,
        proof: Proof::Click,
        expiry: Expiry::STANDARD,
        words: words(setting, what, "until someone turns it off"),
        undo: Undo::Twin { action: id },
        settings: SETTINGS,
        walls: &[],
        coverage: WallCoverage::None,
        machine_hints: &[],
    }
}

pub(crate) static SWITCH: ActionSpec = switch("test.switch", Floor::Admin, "Allow test things", "Let {ws} do test things");
pub(crate) static OWNER_SWITCH: ActionSpec = switch("test.owner-switch", Floor::Owner, "Allow owner things", "Let {ws} do owner things");
pub(crate) static SLOW: ActionSpec = switch("test.slow", Floor::Admin, "Allow slow things", "Let {ws} do slow things");
pub(crate) static FAIL: ActionSpec = switch("test.fail", Floor::Admin, "Allow failing things", "Let {ws} do failing things");
pub(crate) static MOVES: ActionSpec = switch("test.moved", Floor::Admin, "Allow moving things", "Let {ws} do moving things");
pub(crate) static CONFIRM: ActionSpec = ActionSpec {
    id: "test.confirm",
    area: Area::Held,
    params: &[ParamSpec { key: "request", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] }],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::AppApprove },
    expiry: Expiry::STANDARD,
    words: words("App Approvals", "Run {ws}'s held request {request}", "once"),
    undo: Undo::None,
    settings: SettingsRef { page: "app-approvals", path: "Settings → Apps → Approvals" },
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};
pub(crate) static MODE: ActionSpec = ActionSpec {
    id: "test.mode",
    params: &[
        ParamSpec { key: "mode", kind: ParamKind::Choice, required: true, list: false, choices: &["approval", "free"] },
        ParamSpec { key: "cores", kind: ParamKind::Core, required: false, list: true, choices: &[] },
    ],
    ..switch("test.mode", Floor::Admin, "Agent sending", "Let {ws} send {mode}")
};

pub(crate) static NEVER: NotCardableSpec =
    NotCardableSpec { id: "test.never", area: Area::Box, why: NotCardable::Never { reason: NeverReason::BoxLevel }, r#where: "a terminal outside K2" };
pub(crate) static COVERED: NotCardableSpec =
    NotCardableSpec { id: "test.covered", area: Area::Domains, why: NotCardable::Covered { by: "test.switch" }, r#where: "Settings → Domains" };

fn check_on(ctx: &Ctx, id: &str) -> Check {
    if is_on(&ctx.workspace_id, id) {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

fn apply_on(ctx: &Ctx, who: &Answerer, id: &str) -> Applied {
    count(&ctx.workspace_id, id, who);
    set_on(&ctx.workspace_id, id, true);
    Applied::Done { message: String::new() }
}

fn reg(spec: &'static ActionSpec, check: fn(&Ctx) -> Check, apply: fn(&Ctx, &Answerer, Option<&str>) -> Applied) -> Registered {
    Registered { spec, check, apply }
}

pub(crate) fn all() -> Vec<Registered> {
    vec![
        reg(&SWITCH, |c| check_on(c, "test.switch"), |c, w, _| apply_on(c, w, "test.switch")),
        reg(&OWNER_SWITCH, |c| check_on(c, "test.owner-switch"), |c, w, _| apply_on(c, w, "test.owner-switch")),
        reg(
            &SLOW,
            |c| check_on(c, "test.slow"),
            |c, w, _| {
                std::thread::sleep(std::time::Duration::from_millis(150));
                apply_on(c, w, "test.slow")
            },
        ),
        reg(
            &FAIL,
            |_| Check::Needed,
            |c, w, _| {
                count(&c.workspace_id, "test.fail", w);
                Applied::Refused { code: "gone".into(), message: "the App shop no longer exists".into() }
            },
        ),
        reg(
            &MOVES,
            |c| {
                if MOVED.lock().expect("testkit moved").as_ref().is_some_and(|s| s.contains(&c.workspace_id)) {
                    Check::Moved("the widget changed before the click".into())
                } else {
                    Check::Needed
                }
            },
            |c, w, _| apply_on(c, w, "test.moved"),
        ),
        reg(
            &CONFIRM,
            |c| check_on(c, "test.confirm"),
            |c, w, token| {
                if token != Some("good-token") {
                    return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm again".into() };
                }
                count(&c.workspace_id, "test.confirm", w);
                set_on(&c.workspace_id, "test.confirm", true);
                Applied::Done { message: "It is running now.".into() }
            },
        ),
        reg(&MODE, |c| check_on(c, "test.mode"), |c, w, _| apply_on(c, w, "test.mode")),
    ]
}

pub(crate) fn find(id: &str) -> Option<Registered> {
    // Lane F: while the golden-sample run is active on THIS thread, its
    // catalog actions (the real spec when registered, sample check/apply)
    // win. Every other test and thread is unaffected.
    if let Some(r) = super::permission_samples::stand_in(id) {
        return Some(r);
    }
    all().into_iter().find(|r| r.spec.id == id)
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    vec![&NEVER, &COVERED]
}

#[test]
fn the_test_kit_specs_validate() {
    for r in all() {
        k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    }
}
