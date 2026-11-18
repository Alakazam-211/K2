//! A `--skin` site moves onto an App (0.46.16,
//! `.k2/prds/plan-skin-to-app-move-v1.md`): `k2 app label take <app>
//! <site-label>` copies the site's guests onto the App (skin.db,
//! `k2_core::skin::handover::app_move`) right before the hostname moves, so there is
//! no gap; `k2 app label return` points it back at the site, whose own
//! rows were never changed.
//!
//! **Who copies:** the owner, an Admin login, a K2 shell tab; the App's own
//! agent with "Let this agent manage its Apps' users". Without that
//! switch the take is one held `manage` request for the owner whose card
//! names the guests; its body carries them as `expectUsers`, and the
//! approved replay copies only while the list is still exactly that
//! (`site_guests_changed` otherwise). `expectUsers` from an agent's own
//! call never stands in for the switch.
//!
//! **Sessions don't follow:** a site's guest holds an sid whose pass lives
//! only in the site helper's memory; an App signs its guests in itself.
//! Each guest signs in once more with the same username and password.

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::store::AppRow;
use k2_core::published_services as ps;
use k2_core::skin::handover::app_move::{self, AppMoveReport, ExpectError};
use serde_json::{json, Value as J};

use super::routes::{actor, agent_ws, no, Who, R};

pub const GUESTS_CHANGED: &str = "site_guests_changed";
pub const COPY_FAILED: &str = "site_copy_failed";

/// `expectUsers` on a take body (an approved card's list), if any.
pub(crate) fn expect_users(v: &J) -> R<Option<Vec<String>>> {
    match v.get("expectUsers") {
        None | Some(J::Null) => Ok(None),
        Some(J::Array(a)) => a
            .iter()
            .map(|u| u.as_str().map(str::to_string).ok_or_else(|| no(k2_core::deploy::codes::BAD_REQUEST, 400, "expectUsers is a list of usernames")))
            .collect::<R<Vec<String>>>()
            .map(Some),
        Some(_) => Err(no(k2_core::deploy::codes::BAD_REQUEST, 400, "expectUsers is a list of usernames")),
    }
}

/// May `who` copy guests onto `app` without the owner? A person, the
/// App's agent with the users switch, or the replay of the owner's
/// approved card (which carries its list).
fn may_copy(who: &Who, app: &AppRow, expect: Option<&[String]>) -> bool {
    match agent_ws(who) {
        None => true,
        Some(_) => {
            k2_core::workspace::settings::agents_can_manage_app_users(&app.owner_workspace)
                || (super::approvals::manage_approved_now() && expect.is_some())
        }
    }
}

/// The guests a take would copy (a dry run).
fn would_copy(who: &Who, app: &AppRow, site: &ps::PublishedService) -> R<AppMoveReport> {
    app_move::copy_site_grants_to_app(&site.id, &app.service_id, &app.app_id, Some(&app.owner_workspace), &actor(who), true)
        .map_err(|e| no(COPY_FAILED, 500, format!("couldn't read the site's guests: {e}")))
}

/// An agent without "manage its Apps' users": one held `manage` request
/// naming the guests (`None` = go on: a person, the switch, the approved
/// replay, or a site with nobody to copy).
pub(crate) fn hold_without_users_switch(who: &Who, app: &AppRow, label: &str, site: &ps::PublishedService, expect: Option<&[String]>) -> R<Option<J>> {
    if may_copy(who, app, expect) {
        return Ok(None);
    }
    let Some(ws) = agent_ws(who) else { return Ok(None) };
    let dry = would_copy(who, app, site)?;
    if dry.added.is_empty() {
        return Ok(None);
    }
    let summary = format!("k2 app label take {} {label} (copies {} guest(s) of the --skin site onto the App)", app.app_id, dry.added.len());
    let mut lines = vec![summary.clone(), format!("guests: {}", dry.added.join(", "))];
    if !dry.roles_copied.is_empty() {
        lines.push(format!(
            "roles copied for the App: {}",
            dry.roles_copied.iter().map(|(a, b)| format!("{a} → {b}")).collect::<Vec<_>>().join(", ")
        ));
    }
    let r = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: Some(app),
        label: &app.app_id,
        workspace: ws,
        body: json!({
            "route": "/cli/app/label/take",
            "body": { "app": app.app_id, "label": label, "expectUsers": dry.added },
        }),
        diff: json!({ "lines": lines, "route": "/cli/app/label/take", "users": dry.added, "rolesCopied": dry.roles_copied }),
        summary,
        reasons: vec!["\"Let this agent manage its Apps' users\" is off for this workspace, and the move gives the site's guests access to the App".into()],
        reason: None,
    })?;
    let mut out = super::approvals::held_answer(&r);
    out["hint"] = json!(format!(
        "held for the owner: moving {label} copies {} guest(s) onto {}, and \"Let this agent manage its Apps' users\" is off. \
         The owner approves it in person (k2 app approve {} outside {}, or Settings → App Approvals); then the label moves.",
        dry.added.len(),
        app.app_id,
        r.id,
        k2_core::brand::PRODUCT_NAME
    ));
    Ok(Some(out))
}

/// Copy the site's guests onto the App (exactly `expect` when given).
pub(crate) fn copy_guests(who: &Who, app: &AppRow, site: &ps::PublishedService, expect: Option<&[String]>) -> R<AppMoveReport> {
    if !may_copy(who, app, expect) {
        // hold_without_users_switch runs first; never reached in order.
        return Err(no(
            k2_core::deploy::codes::FORBIDDEN,
            403,
            "\"Let this agent manage its Apps' users\" is off: the move is held for the owner",
        ));
    }
    let by = actor(who);
    match expect {
        Some(list) => app_move::copy_site_grants_to_app_expect(&site.id, &app.service_id, &app.app_id, Some(&app.owner_workspace), &by, list).map_err(|e| match e {
            ExpectError::Changed { now } => {
                let mut r = no(
                    GUESTS_CHANGED,
                    409,
                    format!("the site's guests changed since the owner's card (now: {}): nothing moved; run the take again", if now.is_empty() { "none".into() } else { now.join(", ") }),
                );
                r.extra = Some(json!({ "now": now }));
                r
            }
            ExpectError::Store(e) => no(COPY_FAILED, 500, format!("copying the site's guests failed, nothing moved: {e}")),
        }),
        None => app_move::copy_site_grants_to_app(&site.id, &app.service_id, &app.app_id, Some(&app.owner_workspace), &by, false)
            .map_err(|e| no(COPY_FAILED, 500, format!("copying the site's guests failed, nothing moved: {e}"))),
    }
}

/// A take of a `--skin` site: what moved and what the guests do next.
pub(crate) fn take_answer(app: &AppRow, label: &str, front: u16, site: &ps::PublishedService, c: &AppMoveReport) -> J {
    let rooms_grant = k2_core::skin::guest_rooms::get_grant(&site.id).ok().flatten().is_some();
    let mut left: Vec<String> = Vec::new();
    if rooms_grant {
        left.push(format!(
            "the site's guests-rooms grant (the owner gives the App its own: k2 app grant {} --kind {})",
            app.app_id,
            k2_core::skin::guest_rooms::GRANT_KIND
        ));
    }
    json!({
        "ok": true,
        "app": app.app_id,
        "label": label,
        "frontPort": front,
        "site": true,
        "from": { "service": site.id, "port": site.port, "kind": ps::KIND_SKIN, "running": "yes, unrouted" },
        "copied": {
            "added": c.added,
            "keptExisting": c.kept_existing.iter().map(|(u, on)| json!({ "user": u, "enabled": on })).collect::<Vec<_>>(),
            "skippedOtherAppRole": c.skipped_other_app_role,
            "skippedOtherWorkspaceAgent": c.skipped_other_workspace_agent,
            "rolesCopied": c.roles_copied.iter().map(|(a, b)| json!({ "from": a, "to": b })).collect::<Vec<_>>(),
        },
        "notCopied": left,
        "note": format!(
            "https://{label}.<your box>{zone} now serves {app}, and the site's {n} guest(s) can sign in to it with the same username and \
             password (each signs in once more: a site's sign-ins don't carry over to an App). The --skin site keeps running without a \
             public label; stop it when you're sure (k2 publish stop {label}), or move the label back: k2 app label return {app} {label}. \
             The App sees the label in K2_APP_ORIGINS from its next start (k2 app restart {app}).",
            app = app.app_id,
            n = c.added.len() + c.kept_existing.len(),
            zone = k2_core::hosts::ZONE_SUFFIX,
        ),
    })
}

#[cfg(test)]
#[path = "skin_move_tests.rs"]
mod tests;
