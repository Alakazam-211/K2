//! Voice rooms, daemon side of the K2 Admin Space (prd-k2-voice-rooms-v1
//! v1.9 §2, §3; lane B, slices S1 and S2). The decisions and the ledger
//! are `k2_core::tool_hook::admin_space`; this module gathers the facts
//! that live outside skin.db before each sweep plans:
//!
//! - the Connect logins (role, disabled, owner link), with a `connect`
//!   chat grant written ahead for each that should be in the Space;
//! - whether this box can host rooms ([`k2_core::voice_rooms::Hosting`]);
//! - the room count and the Space's name;
//! - **the fixed default Space** (§2.1, §6.6 2.7): once the Admin Space is
//!   ready, `sew_chat_settings.default_space` becomes `k2-admin`. A box
//!   with another default carries its agents' access rows over to
//!   `k2-admin` first; the switch is audited and `k2 voice status` lists
//!   the agents carried;
//! - the agents with access to the Admin Space (they're added over mgmt).
//!
//! Called from the grant-hook sweep (`tool_hook_worker::sweep_target`),
//! inside its blocking section. Does nothing with the preview off.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::log_debug;
use k2_core::sew_chat::store;
use k2_core::tool_hook::admin_space::{self, Facts};
use k2_core::tool_hook::{self as hook, sew_wire};
use k2_core::voice_rooms::{self, Hosting};
use serde_json::json;

/// How long a tier read stands before the sweep reads it again (lane E's
/// tier watch keeps it current in between).
const TIER_READ_EVERY: Duration = Duration::from_secs(3600);

fn tier_read_at() -> &'static Mutex<Option<Instant>> {
    static AT: Mutex<Option<Instant>> = Mutex::new(None);
    &AT
}

/// This box's hosting state: what the tier watch recorded, else a tier
/// read now (at most hourly). No Connect login (no label or token) → not
/// Pro; a failed read stays unknown (rooms stay dormant, read again next
/// sweep).
fn hosting_now() -> Hosting {
    // A state someone else recorded (the tier watch) stands; one this
    // module read is read again after an hour.
    let ours = tier_read_at().lock().ok().and_then(|g| *g);
    let h = voice_rooms::hosting();
    if h != Hosting::Unknown && ours.is_none_or(|t| t.elapsed() < TIER_READ_EVERY) {
        return h;
    }
    let read = (|| -> Result<Hosting, String> {
        let cfg = k2_core::tunnel::config::load()?;
        if cfg.subdomain.trim().is_empty() || cfg.token.trim().is_empty() {
            return Ok(Hosting::NotPro);
        }
        let acct = k2_core::tunnel::subdomains::fetch_account(&cfg.subdomain, &cfg.token)?;
        Ok(if acct.tier.as_deref() == Some("pro") { Hosting::Ok } else { Hosting::NotPro })
    })();
    match read {
        Ok(h) => {
            voice_rooms::set_hosting(h);
            if let Ok(mut g) = tier_read_at().lock() {
                *g = Some(Instant::now());
            }
            h
        }
        Err(e) => {
            log_debug!("[voice-rooms] tier read failed (rooms stay dormant): {e}");
            voice_rooms::hosting()
        }
    }
}

/// The Space's name: the box's label, else "K2".
fn space_name() -> String {
    k2_core::tunnel::config::load()
        .ok()
        .map(|c| c.subdomain.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "K2".into())
}

/// Before the sweep plans: the facts, plus the grants written ahead and
/// the default-Space switch. `None` with the preview off (nothing is
/// created and nothing changes).
pub fn prepare() -> Result<Option<Facts>, String> {
    if !voice_rooms::enabled() {
        return Ok(None);
    }
    let connect = Facts::load_connect()?;
    match hook::ensure_admin_connect_grants(&connect) {
        Ok(0) => {}
        Ok(n) => log_debug!("[voice-rooms] {n} Connect login(s) got a chat grant for the K2 Admin Space"),
        Err(e) => log_debug!("[voice-rooms] chat grants for the K2 Admin Space: {e}"),
    }
    let space = hook::admin_space_id()?;
    let agents = match space.as_deref() {
        Some(id) => {
            sync_default_space(id)?;
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

/// §6.6 2.7: make `k2-admin` the default Space. A box with another
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
                detail: Some(json!({"from": cur, "carried": carried, "why": "the K2 Admin Space is the fixed default Space (voice rooms)"}).to_string()),
                outcome: "ok".into(),
                reason: None,
                created_at: now,
            },
        )?;
        (cur, carried)
    };
    hook::admin_space_note_default_switch(from.as_deref(), &carried)?;
    log_debug!("[voice-rooms] default Space {} → k2-admin; carried {} agent(s)", from.as_deref().unwrap_or("none"), carried.len());
    Ok(true)
}

/// `set default_space` while the Admin Space holds it: refused (§2.1,
/// `409 default_space_fixed`). `Some(message)` when refused.
pub fn default_space_fixed(requested: Option<&str>) -> Result<Option<String>, String> {
    if !voice_rooms::enabled() || hook::admin_space_id()?.is_none() {
        return Ok(None);
    }
    if requested == Some(sew_wire::ADMIN_SPACE_SLUG) {
        return Ok(None);
    }
    Ok(Some(
        "the default Space is fixed to k2-admin (the K2 Admin Space) while voice rooms are on; give an agent access to k2-admin instead".into(),
    ))
}

/// The sweep's step: facts, then the plan. Errors are the sweep's to log.
pub fn sweep() -> Result<(), String> {
    let Some(facts) = prepare()? else { return Ok(()) };
    let s = hook::apply_admin_space(&facts)?;
    if s.create || s.adds + s.removes + s.levels + s.room_ops + s.closings > 0 {
        log_debug!("[voice-rooms] {}", serde_json::to_value(&s).unwrap_or_default());
    }
    Ok(())
}
