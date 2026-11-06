//! Voice rooms, daemon side of the K5 Space (prd-k2-voice-rooms-v1
//! v1.9 §2, §3; lane B, slices S1 and S2). The decisions and the ledger
//! are `k2_core::tool_hook::admin_space`; this module gathers the facts
//! that live outside skin.db before each sweep plans:
//!
//! - the Connect logins (role, disabled, owner link), with a `connect`
//!   chat grant written ahead for each that should be in the Space;
//! - whether this box can host rooms ([`k2_core::voice_rooms::Hosting`]);
//! - the room count and the Space's name;
//! - **the fixed default Space** (§2.1, §6.6 2.7): once the Admin Space is
//!   ready, `sew_chat_settings.default_space` becomes `k5-admin`. A box
//!   with another default carries its agents' access rows over to
//!   `k5-admin` first; the switch is audited and `k2 voice status` lists
//!   the agents carried;
//! - the agents with access to the Admin Space (they're added over mgmt).
//!
//! Called from the grant-hook sweep (`tool_hook_worker::sweep_target`),
//! inside its blocking section. Does nothing with the preview off.

use k2_core::log_debug;
use k2_core::sew_chat::store;
use k2_core::tool_hook::admin_space::{self, Facts};
use k2_core::tool_hook::{self as hook, sew_wire};
use k2_core::voice_rooms::{self, Hosting};
use serde_json::json;

/// This box's hosting state, from the daemon's cached plan tier (fresh 5 min,
/// a stale reading counts up to 1 h). No reading at all is `Unknown`, never
/// not Pro: a daemon plans before its first fetch. Never reaches the network.
fn hosting_now() -> Hosting {
    if let Some(h) = voice_rooms::hosting_override() {
        return h;
    }
    let env = crate::sew::supervisor::live();
    hosting_of(&env.ops.plan_tier())
}

/// [`hosting_now`]'s rule (pure).
pub fn hosting_of(view: &k2_core::tunnel::subdomains::TierView) -> Hosting {
    match view.reading() {
        None => Hosting::Unknown,
        Some(_) if view.is_pro() => Hosting::Ok,
        Some(_) => Hosting::NotPro,
    }
}

/// The Space's name (Rosson 11-02): the same on every box.
fn space_name() -> String {
    "K5 Space".to_string()
}

/// Before the sweep plans: the facts, plus the grants written ahead and
/// the default-Space switch. `None` with the preview off (nothing is
/// created and nothing changes).
pub fn prepare() -> Result<Option<Facts>, String> {
    if !k2_core::voice_preview::enabled() {
        return Ok(None);
    }
    let connect = Facts::load_connect()?;
    match hook::ensure_admin_connect_grants(&connect) {
        Ok(0) => {}
        Ok(n) => log_debug!("[voice-rooms] {n} Connect login(s) got a chat grant for the K5 Space"),
        Err(e) => log_debug!("[voice-rooms] chat grants for the K5 Space: {e}"),
    }
    let space = hook::admin_space_id()?;
    let agents = match space.as_deref() {
        Some(id) => {
            // A re-linked Space (made again, or K2's id was stale): the
            // agents' access rows follow the id Sew holds.
            let moved = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::sew_chat::install::relink_k5_access(&conn, id, k2_core::tool_hook::outbox::now_secs())?
            };
            if moved > 0 {
                log_debug!("[voice-rooms] {moved} agent access row(s) moved to the K5 Space's id {id}");
            }
            sync_default_space(id)?;
            if hook::admin_space_store_count()? {
                log_debug!("[voice-rooms] voiceRoomsCount stored at set-up");
            }
            agents_with_access(id)?
        }
        None => Vec::new(),
    };
    Ok(Some(Facts {
        enabled: true,
        name: space_name(),
        hosting: hosting_now(),
        count: k2_core::app_settings::load().voice_rooms_count,
        connect,
        agents,
    }))
}

/// Workspaces with an access row on the Admin Space.
fn agents_with_access(space_id: &str) -> Result<Vec<String>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out: Vec<String> = store::list_all_access(&conn)?
        .into_iter()
        .filter(|a| a.space_id == space_id)
        .map(|a| a.project_id)
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// §6.6 2.7: make `k5-admin` the default Space. A box with another
/// default first carries its agents' access over (an agent that could DM
/// before still can), audited. Returns whether it switched now.
pub fn sync_default_space(space_id: &str) -> Result<bool, String> {
    let now = k2_core::tool_hook::outbox::now_secs();
    let (from, carried) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let cur = store::default_space(&conn)?;
        if cur.as_deref() == Some(sew_wire::ADMIN_SPACE_SLUG) {
            return Ok(false);
        }
        let mut carried = Vec::new();
        if let Some(old) = cur.as_deref() {
            for a in store::list_all_access(&conn)?.into_iter().filter(|a| a.space_slug.eq_ignore_ascii_case(old)) {
                if store::get_access(&conn, &a.project_id, space_id)?.is_some() {
                    continue;
                }
                store::set_access(
                    &conn,
                    &store::Access {
                        space_id: space_id.to_string(),
                        space_slug: sew_wire::ADMIN_SPACE_SLUG.into(),
                        updated_by: admin_space::ACTOR.into(),
                        updated_at: now,
                        ..a.clone()
                    },
                )?;
                carried.push(a.project_id);
            }
        }
        store::set_default_space(&conn, Some(sew_wire::ADMIN_SPACE_SLUG), admin_space::ACTOR, now)?;
        store::insert_audit(
            &conn,
            &store::AuditRow {
                id: k2_core::sew_chat::new_id("aud_"),
                project_id: None,
                actor: admin_space::ACTOR.into(),
                actor_kind: "system".into(),
                op: "default_space".into(),
                space_id: Some(space_id.to_string()),
                target: Some(sew_wire::ADMIN_SPACE_SLUG.into()),
                detail: Some(json!({"from": cur, "carried": carried, "why": "the K5 Space is the fixed default Space (voice rooms)"}).to_string()),
                outcome: "ok".into(),
                reason: None,
                created_at: now,
            },
        )?;
        (cur, carried)
    };
    hook::admin_space_note_default_switch(from.as_deref(), &carried)?;
    log_debug!("[voice-rooms] default Space {} → k5-admin; carried {} agent(s)", from.as_deref().unwrap_or("none"), carried.len());
    Ok(true)
}

/// `set default_space` while the Admin Space holds it: refused (§2.1,
/// `409 default_space_fixed`). `Some(message)` when refused.
pub fn default_space_fixed(requested: Option<&str>) -> Result<Option<String>, String> {
    if !k2_core::voice_preview::enabled() || hook::admin_space_id()?.is_none() {
        return Ok(None);
    }
    if requested == Some(sew_wire::ADMIN_SPACE_SLUG) {
        return Ok(None);
    }
    Ok(Some(
        "the default Space is fixed to k5-admin (the K5 Space) while voice rooms are on; give an agent access to k5-admin instead".into(),
    ))
}

/// The sweep's step: facts, then the plan. Errors are the sweep's to log.
pub fn sweep() -> Result<(), String> {
    let Some(facts) = prepare()? else {
        hook::admin_space_note_preview_off();
        return Ok(());
    };
    let s = hook::apply_admin_space(&facts)?;
    if s.create || s.adds + s.removes + s.levels + s.room_ops + s.closings > 0 {
        log_debug!("[voice-rooms] {}", serde_json::to_value(&s).unwrap_or_default());
    }
    Ok(())
}

// ── a lost K5 Space: the check and `k2 voice repair` ──────────────────

/// Something saw Sew answer `not_found` for the K5 Space or a room (the
/// voice feed, a join). Asks for a check by mgmt (`admin_space::mark_verify`:
/// at most once per 10 minutes, never with the preview off). The check is
/// the confirmation: a masked 404 alone never re-makes anything.
pub fn suspect(why: &str) {
    if !k2_core::voice_preview::enabled() {
        return;
    }
    match hook::admin_space_check(admin_space::VERIFY_SPACE, why, false) {
        Ok(true) => log_debug!("[voice-rooms] {why}: K2 is checking the K5 Space with Sew (k2 voice repair runs it now)"),
        Ok(false) => {}
        Err(e) => log_debug!("[voice-rooms] asking for a K5 Space check: {e}"),
    }
}

/// What `k2 voice repair` saw before it asked for the check.
#[derive(Debug, Clone)]
pub struct RepairStart {
    pub started_at: i64,
    pub before: admin_space::View,
    pub install: Option<k2_core::sew_chat::install::InstallReport>,
    /// A check was asked for (`false`: no K5 Space to check).
    pub checking: bool,
}

/// `k2 voice repair`, first half: set aside what an earlier Sew install
/// left in the main DB, and ask for a full check of the K5 Space (members
/// re-sent too; a blocked Space is tried again). The worker does the rest.
pub fn repair_begin(why: &str) -> Result<RepairStart, String> {
    let started_at = k2_core::tool_hook::outbox::now_secs();
    let before = hook::admin_space_view()?;
    let install = match hook::get_target(hook::CHAT_KIND)?.and_then(|t| t.install_id) {
        Some(id) => Some(crate::sew_chat::install::sync(&id)?),
        None => None,
    };
    let checking = hook::admin_space_check(admin_space::VERIFY_FULL, why, true)?;
    Ok(RepairStart { started_at, before, install, checking })
}

/// Has the repair settled: the check answered, nothing of the K5 Space is
/// still queued, and every open room has its channel again?
pub fn repair_settled(start: &RepairStart, v: &admin_space::View) -> bool {
    if !start.checking {
        return true;
    }
    let Some(sp) = v.space.as_ref() else { return false };
    let hosts = hosting_now() == Hosting::Ok;
    if sp.verify.is_some() || (sp.verify_rooms && hosts) || hook::reconcile_requested() {
        return false;
    }
    if sp.state == admin_space::SPACE_BLOCKED {
        return true;
    }
    if v.holds.iter().any(|h| h.state == "pending") {
        return false;
    }
    let count = k2_core::app_settings::load().voice_rooms_count;
    let rooms_wait = hosts && v.rooms.iter().any(|r| r.n <= count && r.state == admin_space::ROOM_PENDING);
    let members_wait = v.members.iter().any(|m| m.state == admin_space::PENDING_ADD && !m.why.as_deref().is_some_and(|w| w.starts_with(admin_space::HELD_MARK)));
    !rooms_wait && !members_wait
}

/// What Sew can't bring back when it makes the K5 Space again (Sew's mgmt
/// docs, "The K5 Space").
pub const NOT_RESTORED: &str = "Sew made the K5 Space again under a new id: its rooms are back and everyone is in it again, \
    but Sew can't bring back the old Space's message history, uploads, recordings, people's read positions and \
    preferences, or channels and roles someone made in it by hand. DMs are not affected.";

/// `k2 voice repair`, second half: what it found and what it fixed.
pub fn repair_report(start: &RepairStart, after: &admin_space::View, settled: bool) -> Result<serde_json::Value, String> {
    let before_space = start.before.space.clone();
    let after_space = after.space.clone();
    // The check this repair asked for (or none: nothing to check).
    let check = after_space
        .as_ref()
        .and_then(|s| s.last_check.clone())
        .filter(|c| c["at"].as_i64().is_some_and(|at| at >= start.started_at));
    let outcome = check.as_ref().and_then(|c| c["outcome"].as_str().map(str::to_string));
    let before_rooms: std::collections::BTreeMap<String, Option<String>> =
        start.before.rooms.iter().map(|r| (r.slug.clone(), r.channel_id.clone())).collect();
    let rooms: Vec<serde_json::Value> = after
        .rooms
        .iter()
        .filter_map(|r| {
            let was = before_rooms.get(&r.slug).cloned().flatten();
            (was != r.channel_id && r.channel_id.is_some()).then(|| json!({"room": r.slug, "from": was, "to": r.channel_id}))
        })
        .collect();
    let held = |m: &admin_space::MemberRow| m.why.as_deref().is_some_and(|w| w.starts_with(admin_space::HELD_MARK));
    let members = json!({
        "added": after.members.iter().filter(|m| m.state == admin_space::ADDED).count(),
        "pending": after.members.iter().filter(|m| m.state == admin_space::PENDING_ADD && !held(m)).count(),
        "held": after.members.iter().filter(|m| held(m)).map(|m| json!({"externalId": m.external_id, "why": m.why})).collect::<Vec<_>>(),
    });
    let blocked_before = start.before.holds.iter().filter(|h| h.state == "blocked").count();
    // Each set-aside Space with its agents by workspace handle (who made it
    // again, and who gets access back).
    let stale: Vec<serde_json::Value> = crate::sew_chat::install::stale_spaces()?
        .into_iter()
        .map(|s| {
            let access: Vec<serde_json::Value> = s
                .access
                .iter()
                .map(|a| json!({"projectId": a.project_id, "workspace": crate::compute_routes::ws_handle(&a.project_id), "level": a.level, "approval": a.approval}))
                .collect();
            json!({"spaceId": s.space_id, "slug": s.slug, "access": access, "install": s.install, "since": s.since, "why": s.why})
        })
        .collect();
    let moved_now: Vec<String> = start.install.as_ref().map(|r| r.stale.iter().map(|s| s.slug.clone()).collect()).unwrap_or_default();
    let space_line = match (after_space.as_ref(), outcome.as_deref()) {
        (None, _) => "no K5 Space to check: voice rooms haven't made one on this server (k2 voice status says why)".to_string(),
        (Some(s), _) if s.state == admin_space::SPACE_BLOCKED => {
            format!("the K5 Space is blocked: {} (fix that, then run k2 voice repair again)", s.why.as_deref().unwrap_or("Sew refused it"))
        }
        (Some(_), Some("same")) => "the K5 Space is there; K2 retried what Sew had refused and re-sent every member".to_string(),
        (Some(s), Some("remade")) => format!(
            "the K5 Space was gone from Sew; Sew made it again as {} (was {})",
            s.space_id.as_deref().unwrap_or("?"),
            check.as_ref().and_then(|c| c["from"].as_str()).unwrap_or("?")
        ),
        (Some(s), Some("relinked")) => format!(
            "K2 held a stale id; the K5 Space Sew holds is {} (was {})",
            s.space_id.as_deref().unwrap_or("?"),
            check.as_ref().and_then(|c| c["from"].as_str()).unwrap_or("?")
        ),
        (Some(_), _) if !start.checking => "no K5 Space to check".to_string(),
        (Some(_), _) => "the check hasn't answered yet: K2 keeps at it (k2 voice status shows progress)".to_string(),
    };
    let mut fixes: Vec<String> = Vec::new();
    if !rooms.is_empty() {
        fixes.push(format!("{} room(s) re-linked to new channels", rooms.len()));
    }
    if blocked_before > 0 && matches!(outcome.as_deref(), Some("same" | "remade" | "relinked")) {
        fixes.push(format!("{blocked_before} blocked change(s) retried or replaced"));
    }
    if !moved_now.is_empty() {
        fixes.push(format!("{} agent Space(s) from an earlier Sew install set aside ({})", moved_now.len(), moved_now.join(", ")));
    }
    if let Some(n) = start.install.as_ref().map(|r| r.cursors_reset).filter(|n| *n > 0) {
        fixes.push(format!("{n} agent chat feed(s) restarted"));
    }
    Ok(json!({
        "ok": true,
        "settled": settled,
        "found": {
            "space": before_space.as_ref().map(|s| json!({"state": s.state, "spaceId": s.space_id, "why": s.why})),
            "blocked": blocked_before,
            "pending": start.before.holds.iter().filter(|h| h.state == "pending").count(),
        },
        "space": space_line,
        "check": check,
        "rooms": rooms,
        "members": members,
        "fixed": fixes,
        "staleSpaces": stale,
        "staleHint": (!stale.is_empty()).then_some(
            "these agent Spaces were on a Sew install that is gone, and Sew can't give a Space its id back. \
             One of the agents listed (the one at level manage made it) makes it again with k2 chat space create <slug> --name \"<Name>\"; \
             then the owner or an Admin gives the others back the level shown: k2 chat access grant <slug> <workspace> --level <level>."
        ),
        "notRestored": (outcome.as_deref() == Some("remade")).then_some(NOT_RESTORED),
        "holds": after.holds,
        "next": if settled { serde_json::Value::Null } else { json!("still working: k2 voice status shows progress") },
    }))
}

#[cfg(test)]
mod hosting_tests {
    use super::*;
    use k2_core::tunnel::subdomains::{TierReading, TierView};

    fn reading(tier: Option<&str>) -> TierReading {
        TierReading { primary: "box".into(), tier: tier.map(str::to_string), fetched_at: 1 }
    }

    /// No reading is Unknown (rooms left as they are), never not Pro; a reading
    /// decides, stale or fresh.
    #[test]
    fn no_reading_is_unknown_and_a_reading_decides() {
        assert_eq!(hosting_of(&TierView::Unknown), Hosting::Unknown);
        assert_eq!(hosting_of(&TierView::Fresh(reading(Some("pro")))), Hosting::Ok);
        assert_eq!(hosting_of(&TierView::Stale(reading(Some("pro")))), Hosting::Ok);
        assert_eq!(hosting_of(&TierView::Fresh(reading(Some("single")))), Hosting::NotPro);
        assert_eq!(hosting_of(&TierView::Fresh(reading(None))), Hosting::NotPro, "the control plane named no tier");
    }
}
