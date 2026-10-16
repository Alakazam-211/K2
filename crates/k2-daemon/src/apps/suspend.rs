//! Suspend and resume an App, and its remote passes
//! (`prd-k2-app-runtime-v1` §5.2, §5.4; S19, AR10).
//!
//! ```text
//! POST /cli/app/suspend              {app, reason?}   any Member (an agent: its own workspace's Apps); narrows
//! POST /cli/app/resume               {app, start?}    the owner (passports refused)
//! GET  /cli/app/remote-pass          ?app=            the App's remote passes (never the secret)
//! POST /cli/app/remote-pass/rotate   {app, ttl?, keepOld?}   the owner: a new remote pass, shown once
//! POST /cli/app/remote-pass/revoke   {app, id?}       any Member; narrows
//! ```
//!
//! **Suspend is one step**: mark the App suspended (deploys and starts
//! frozen), revoke every live pass it holds (link and remote, one UPDATE,
//! so the very next call is `401`), then stop it (which closes its link). Anyone
//! who may see the App may suspend it: it only narrows. Only the owner
//! resumes.
//!
//! **A remote App pass** (an App hosted elsewhere) is the App's own pass
//! with an expiry and no link: refused on every link listener, accepted
//! on the main port and the tunnel, carrying the grants formula's caps
//! (manifest ∩ approved set, the App's own room). Minting one is a
//! credential mint: the owner's, never a passport's.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::deploy::{self, codes, store};
use k2_core::skin::link as passes;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, db_err, json_body, need_credential, no, s, Who, R};

pub const ROUTES: &[&str] =
    &["/cli/app/suspend", "/cli/app/resume", "/cli/app/remote-pass", "/cli/app/remote-pass/rotate", "/cli/app/remote-pass/revoke"];
pub const POST_ROUTES: &[&str] = &["/cli/app/suspend", "/cli/app/resume", "/cli/app/remote-pass/rotate", "/cli/app/remote-pass/revoke"];

pub(crate) const APP_SUSPENDED: &str = "app_suspended";

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/suspend" => suspend(who, body),
        "/cli/app/resume" => resume(who, body),
        "/cli/app/remote-pass" => remote_list(who, params),
        "/cli/app/remote-pass/rotate" => remote_rotate(who, body),
        "/cli/app/remote-pass/revoke" => remote_revoke(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

fn is_owner(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

fn owner_only(what: &str) -> super::routes::Refusal {
    no(
        super::egress_routes::codes::OWNER_ONLY,
        403,
        format!("only the server's owner {what} (the owner token or an Owner login; never an agent or a K2 terminal tab)"),
    )
}

/// Is the App suspended? (`false` when it has no row.)
pub fn is_suspended(app_id: &str) -> bool {
    let h = k2_core::db::shared();
    let conn = h.lock();
    super::grants::is_suspended(&conn, app_id).unwrap_or(false)
}

/// Refuse a deploy, start or rollback of a suspended App.
pub(crate) fn refuse_if_suspended(app_id: &str) -> R<()> {
    if is_suspended(app_id) {
        return Err(no(
            APP_SUSPENDED,
            409,
            format!("{app_id} is suspended: deploys and starts are frozen until the owner resumes it (k2 app resume {app_id})"),
        ));
    }
    Ok(())
}

fn suspension(app_id: &str) -> R<J> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT suspended_at, suspended_by, suspended_reason FROM apps WHERE app_id = ?1", params![app_id], |r| {
        let at: Option<i64> = r.get(0)?;
        Ok(match at {
            Some(at) => json!({ "suspended": true, "at": at, "by": r.get::<_, Option<String>>(1)?, "reason": r.get::<_, Option<String>>(2)? }),
            None => json!({ "suspended": false }),
        })
    })
    .map_err(db_err)
}

/// POST `/cli/app/suspend {app, reason?}`.
fn suspend(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let by = actor(who);
    let reason = s(&v, "reason").map(|r| r.chars().take(500).collect::<String>());
    let already = is_suspended(&app.app_id);
    if !already {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "UPDATE apps SET suspended_at = ?2, suspended_by = ?3, suspended_reason = ?4, updated_at = ?2 WHERE app_id = ?1",
            params![app.app_id, deploy::now(), by, reason],
        )
        .map_err(db_err)?;
    }
    // Every live pass, link and remote, in one statement: the next call is
    // 401 (a connection still open answers `app_suspended`). Stopping the
    // App then closes its link; if a deploy is mid-way the link stays up
    // refusing everything until it can stop.
    let revoked = passes::revoke_app_passes(&app.service_id).map_err(|e| no("db_error", 500, e))?;
    super::quotas::invalidate(&app.app_id);
    let stopped = match super::supervisor::stop_app(&app, &by) {
        Ok(()) => json!(true),
        Err(f) => json!({ "stopped": false, "code": f.code, "hint": f.message }),
    };
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let _ = store::set_state(&conn, &app.app_id, store::STATE_STOPPED, Some("suspended"));
        let _ = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, "suspend", &by, None, &json!({ "reason": reason, "passesRevoked": revoked }));
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-suspend",
        &by,
        format!("{} suspended ({revoked} passes revoked){}", app.app_id, reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default()),
        "cli",
        "-",
        "apps",
    ));
    super::alerts::notify(
        &app.owner_workspace,
        &format!(
            "[k2 apps] {} was suspended by {by}{}. Its passes are revoked and deploys are frozen until the owner resumes it.",
            app.app_id,
            reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()
        ),
    );
    super::routes::mirror(&app.app_id);
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "already": already,
        "passesRevoked": revoked,
        "stopped": stopped,
        "suspension": suspension(&app.app_id)?,
        "hint": format!("only the owner resumes it: k2 app resume {}", app.app_id),
    }))
}

/// POST `/cli/app/resume {app, start?}`: the owner.
fn resume(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    if !is_owner(who) {
        return Err(owner_only("resumes a suspended App"));
    }
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    if !is_suspended(&app.app_id) {
        return Ok(json!({ "ok": true, "app": app.app_id, "already": "not suspended" }));
    }
    let by = actor(who);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "UPDATE apps SET suspended_at = NULL, suspended_by = NULL, suspended_reason = NULL, updated_at = ?2 WHERE app_id = ?1",
            params![app.app_id, deploy::now()],
        )
        .map_err(db_err)?;
        let _ = store::set_state(&conn, &app.app_id, store::STATE_STOPPED, Some("resumed; not started"));
        let _ = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, "resume", &by, None, &json!({}));
    }
    super::quotas::invalidate(&app.app_id);
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("app-resume", &by, format!("{} resumed", app.app_id), "cli", "-", "apps"));
    super::alerts::notify(&app.owner_workspace, &format!("[k2 apps] the owner resumed {}.", app.app_id));
    let mut out = json!({ "ok": true, "app": app.app_id, "suspension": suspension(&app.app_id)? });
    if v.get("start").and_then(J::as_bool) == Some(true) {
        let app = app_for(who, Some(&app.app_id))?;
        match app.live_seed.clone() {
            Some(seed) => {
                let dep = super::supervisor::begin(&app, &seed, store::KIND_START, &by, Some("resume"))?;
                out["deployment"] = json!(dep);
            }
            None => out["note"] = json!("no Seed deployed yet; nothing to start"),
        }
    } else {
        out["hint"] = json!(format!("start it with k2 app start {}", app.app_id));
    }
    Ok(out)
}

// ── remote passes (AR10) ─────────────────────────────────────────────

fn remote_list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let rows = passes::remote_passes(&app.service_id).map_err(|e| no("db_error", 500, e))?;
    let now = deploy::now();
    let live = rows.iter().filter(|r| r.revoked_at.is_none() && r.expires_at > now).count();
    Ok(json!({ "ok": true, "app": app.app_id, "passes": rows, "live": live }))
}

/// POST `/cli/app/remote-pass/rotate {app, ttl?, keepOld?}`: the owner.
fn remote_rotate(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    if !is_owner(who) {
        return Err(owner_only("mints a remote App pass (a long-lived credential)"));
    }
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    refuse_if_suspended(&app.app_id)?;
    let ttl = match v.get("ttl") {
        None | Some(J::Null) => passes::REMOTE_PASS_TTL_SECS,
        Some(J::Number(n)) => n.as_i64().ok_or_else(|| no(codes::BAD_REQUEST, 400, "ttl is seconds"))?,
        Some(J::String(t)) => parse_ttl(t).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("ttl is like 30d, 12h or seconds, not '{t}'")))?,
        _ => return Err(no(codes::BAD_REQUEST, 400, "ttl is seconds or 30d / 12h")),
    };
    let (manifest, approved) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let manifest = match app.live_seed.as_deref() {
            Some(seed) => super::grants::seed_caps(&conn, seed).map_err(db_err)?.unwrap_or_default(),
            None => Vec::new(),
        };
        (manifest, super::grants::approved(&conn, &app).map_err(db_err)?)
    };
    let mut eff = super::grants::effective(&manifest, approved.as_deref(), &app.owner_workspace, false, &super::grants::Actor::App);
    // The owner may name the caps outright (an App hosted elsewhere has no
    // Seed here): minting is the owner's own act, so it is their approval.
    if let Some(list) = v.get("caps").and_then(J::as_array) {
        let asked: Vec<String> = list.iter().filter_map(J::as_str).map(str::to_string).collect();
        let (keep, dropped) = passes::link_caps(&asked);
        if !dropped.is_empty() {
            return Err(no(codes::BAD_REQUEST, 400, format!("caps a remote App pass can't carry: {}", dropped.join(", "))));
        }
        eff.caps = keep;
        eff.withheld.clear();
    }
    let old: Vec<String> = passes::remote_passes(&app.service_id)
        .map_err(|e| no("db_error", 500, e))?
        .into_iter()
        .filter(|p| p.revoked_at.is_none())
        .map(|p| p.id)
        .collect();
    let m = passes::mint_remote_pass(&app.service_id, &app.app_id, &eff.caps, &eff.rooms, ttl).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let mut revoked = 0;
    if v.get("keepOld").and_then(J::as_bool) != Some(true) {
        for id in &old {
            revoked += passes::revoke_remote_passes(&app.service_id, Some(id)).map_err(|e| no("db_error", 500, e))?;
        }
    }
    let by = actor(who);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let _ = deploy::audit::append(
            &conn,
            deploy::audit::SUBJECT_APP,
            &app.app_id,
            "remote_pass",
            &by,
            None,
            &json!({ "id": m.id, "caps": m.caps, "rooms": m.rooms, "expiresAt": m.expires_at, "revoked": revoked }),
        );
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-remote-pass",
        &by,
        format!("{}: remote pass {} minted ({revoked} old revoked)", app.app_id, m.prefix),
        "cli",
        "-",
        "apps",
    ));
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "pass": m.raw,
        "id": m.id,
        "prefix": m.prefix,
        "caps": m.caps,
        "rooms": m.rooms,
        "withheld": eff.withheld,
        "expiresAt": m.expires_at,
        "revoked": revoked,
        "note": "shown once: store it in the remote App's secrets. It works on the main port and the tunnel only, never on a gateway link.",
    }))
}

fn parse_ttl(raw: &str) -> Option<i64> {
    let t = raw.trim();
    if let Ok(n) = t.parse::<i64>() {
        return Some(n);
    }
    if !t.is_ascii() || t.len() < 2 {
        return None;
    }
    let (n, unit) = t.split_at(t.len() - 1);
    let n: i64 = n.parse().ok()?;
    Some(match unit {
        "d" => n * 86_400,
        "h" => n * 3_600,
        "m" => n * 60,
        _ => return None,
    })
}

/// POST `/cli/app/remote-pass/revoke {app, id?}`: narrows; any Member.
fn remote_revoke(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let n = passes::revoke_remote_passes(&app.service_id, s(&v, "id")).map_err(|e| no("db_error", 500, e))?;
    let by = actor(who);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let _ = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, "remote_pass_revoke", &by, None, &json!({ "id": s(&v, "id"), "revoked": n }));
    }
    Ok(json!({ "ok": true, "app": app.app_id, "revoked": n }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_parses() {
        assert_eq!(parse_ttl("30d"), Some(30 * 86_400));
        assert_eq!(parse_ttl("12h"), Some(12 * 3_600));
        assert_eq!(parse_ttl("3600"), Some(3600));
        assert_eq!(parse_ttl("x"), None);
        assert_eq!(parse_ttl("5w"), None);
    }
}
