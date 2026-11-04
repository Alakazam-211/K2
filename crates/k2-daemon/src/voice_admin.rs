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
