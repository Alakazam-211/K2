//! `docs.share`: share a file or folder of another workspace with the
//! asking agent's workspace, as live docs (Markdown multiplayer v1, MI25).
//!
//! An agent in workspace B that opens a doc in workspace A gets
//! `403 doc_not_shared` when the two are connected but nobody shared it;
//! the refusal carries `error.workspace` (A) and `error.path`, and this card
//! asks a person (Admin floor) to share it. Read-only by default;
//! `access=write` and `history=yes` are separate grants on the same card.
//! It applies through the share route's own entry
//! (`docs::routes::share_entry`) as the answerer. People revoke with
//! `k2 doc unshare` (or `POST /cli/docs/unshare`).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, Undo, Wall,
    WallCoverage, WallMatch, WallMethod, Words,
};

use super::switches::{applied, param, AGENT_TAB};
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) static SHARE: ActionSpec = ActionSpec {
    id: "docs.share",
    area: Area::Misc,
    params: &[
        ParamSpec { key: "from", kind: ParamKind::Workspace, required: true, list: false, choices: &[] },
        ParamSpec { key: "path", kind: ParamKind::Path, required: true, list: false, choices: &[] },
        ParamSpec { key: "access", kind: ParamKind::Choice, required: false, list: false, choices: &["read", "write"] },
        ParamSpec { key: "history", kind: ParamKind::Choice, required: false, list: false, choices: &["yes", "no"] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Share files with another workspace",
        what_changes: "Let {ws} open {path} in {from} as a live doc",
        consequences: "Read-only unless access=write; its history only with history=yes. The two workspaces must stay connected.",
        scope: "{from}: {path}",
        duration: "until someone unshares it (k2 doc unshare)",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: AGENT_TAB,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "doc_not_shared" },
        route_prefix: "/cli/docs/",
        method: WallMethod::Any,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

fn wants(ctx: &Ctx) -> (bool, bool) {
    (param(ctx, "access") == Some("write"), param(ctx, "history") == Some("yes"))
}

fn check_share(ctx: &Ctx) -> Check {
    let (Some(from), Some(path)) = (param(ctx, "from"), param(ctx, "path")) else {
        return Check::Moved("name the workspace and the file: from=<workspace> path=<file or folder>".into());
    };
    let (owner, root) = match crate::docs::target::workspace_root(from) {
        Ok(x) => x,
        Err(r) => return Check::Moved(r.message),
    };
    if owner == ctx.workspace_id {
        return Check::Moved(format!("{from} is {} itself", ctx.workspace_handle));
    }
    let (rel, _) = match crate::docs::target::share_rel(&root, path, true) {
        Ok(x) => x,
        Err(r) => return Check::Moved(r.message),
    };
    let Some(store) = crate::docs::runtime().ok().and_then(|rt| rt.store()) else {
        return Check::Moved("live docs are restarting".into());
    };
    let (write, history) = wants(ctx);
    match crate::docs::shares::grant(&store, &owner, &ctx.workspace_id, &rel) {
        Ok(Some((w, h))) if (w || !write) && (h || !history) => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(e) => Check::Moved(e.to_string()),
    }
}

fn apply_share(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (Some(from), Some(path)) = (param(ctx, "from"), param(ctx, "path")) else {
        return Applied::Refused { code: "bad_params".into(), message: "from and path are required".into() };
    };
    // The share route's floor (Admin for logins) runs again at the click.
    let by = match who {
        Answerer::OwnerToken => crate::docs::caller::owner(),
        Answerer::Login { user, .. } => {
            let admin = k2_core::connect_users::role_for_user(user).is_some_and(k2_core::connect_users::can_manage_users);
            if !admin {
                return Applied::Refused {
                    code: "role_required".into(),
                    message: "sharing files with another workspace needs an Admin or Owner".into(),
                };
            }
            crate::docs::caller::login(user)
        }
    };
    let (write, history) = wants(ctx);
    let r = crate::docs::routes::share_entry(&by, from, path, &ctx.workspace_id, write, history);
    applied(&r, format!("Shared {path} in {from} with {}.", ctx.workspace_handle))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &SHARE, check: check_share, apply: apply_share }]
}

/// None: the one docs row is a card.
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}
