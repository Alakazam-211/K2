//! The grants formula and the App's approved cap set
//! (`prd-k2-app-runtime-v1` §5.3, §5.6; S20).
//!
//! ```text
//! effective = manifest_caps(live Seed)          the ceiling the author asked for
//!           ∩ approved set (apps.approved_caps_json)   what the owner approved for this App
//!           ∩ (guest_role_caps(guest, room)  if actor = asGuest
//!              | app_policy(app, room)       if actor = asApp)
//! ```
//!
//! - The **approved set** is the owner's per-App grant: set by the first
//!   install (held unless it fits a set the owner approved before for the
//!   same Seed id, or it asks for nothing) and widened only by an approved
//!   `deploy` request. Apps that carried a pass before 0.46.1 (guarded,
//!   or the process exception) were grandfathered at their live Seed's caps
//!   by migration 0173. Only Apps that carry a pass have their deploys
//!   held ([`carries_pass`]).
//! - **App policy** (asApp): the caps a link or remote pass can carry
//!   (`skin_link::link_caps`, no `files:agentconfig`), in the App's own
//!   workspace room only.
//! - **Guest** (asGuest, the SDK's default inside a guest request): the
//!   guest's role caps, in the rooms the guest and the App share.
//! - A suspended App gets nothing.
//!
//! Nothing here can widen: every step is an intersection.

use std::collections::BTreeSet;

use k2_core::app_grace::requests as reqs;
use k2_core::deploy::store::{self, AppRow};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value as J};

/// Who a call acts as (§5.2 "three identities, never merged").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor<'a> {
    /// The App itself (its link or remote pass).
    App,
    /// A guest, through the App: their role's caps and rooms.
    Guest { role_caps: &'a [String], rooms: &'a [String] },
}

/// What a pass carries after the formula.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effective {
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    /// Manifest caps no pass can carry (unknown caps, `files:agentconfig`).
    pub dropped: Vec<String>,
    /// Manifest caps the owner hasn't approved (yet).
    pub withheld: Vec<String>,
}

fn uniq(v: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    v.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && seen.insert(s.clone())).collect()
}

/// The pure formula. `approved = None` means nothing is approved (an empty
/// set), never "everything".
pub fn effective(manifest: &[String], approved: Option<&[String]>, workspace_id: &str, suspended: bool, actor: &Actor<'_>) -> Effective {
    if suspended {
        return Effective { withheld: uniq(manifest), ..Default::default() };
    }
    let (ceiling, dropped) = k2_core::skin::link::link_caps(manifest);
    let approved: BTreeSet<&str> = approved.unwrap_or(&[]).iter().map(|s| s.trim()).collect();
    let (mut caps, mut withheld) = (Vec::new(), Vec::new());
    for c in &ceiling {
        if approved.contains(c.as_str()) {
            caps.push(c.clone());
        } else {
            withheld.push(c.clone());
        }
    }
    let rooms = match actor {
        Actor::App => vec![workspace_id.to_string()],
        Actor::Guest { role_caps, rooms } => {
            let role: BTreeSet<&str> = role_caps.iter().map(|s| s.trim()).collect();
            caps.retain(|c| role.contains(c.as_str()));
            // The App lives in its workspace's room; a guest reaches only
            // the rooms both share.
            rooms.iter().filter(|r| r.as_str() == workspace_id).cloned().collect()
        }
    };
    Effective { caps, rooms, dropped, withheld }
}

// ── the approved set ─────────────────────────────────────────────────

/// A Seed's manifest caps (from its stored manifest).
pub fn seed_caps(conn: &Connection, seed_hash: &str) -> rusqlite::Result<Option<Vec<String>>> {
    Ok(store::get_seed(conn, seed_hash)?.map(|s| {
        s.manifest
            .get("caps")
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    }))
}

fn parse_set(raw: Option<String>) -> Option<Vec<String>> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
}

/// The App's recorded approved set (`None` = never recorded).
pub fn recorded(conn: &Connection, app_id: &str) -> rusqlite::Result<Option<Vec<String>>> {
    let raw: Option<Option<String>> =
        conn.query_row("SELECT approved_caps_json FROM apps WHERE app_id = ?1", params![app_id], |r| r.get(0)).optional()?;
    Ok(parse_set(raw.flatten()))
}

/// Record the App's approved set (sorted, de-duplicated).
pub fn record(conn: &Connection, app_id: &str, caps: &[String]) -> rusqlite::Result<()> {
    let mut set: Vec<String> = uniq(caps);
    set.sort();
    conn.execute(
        "UPDATE apps SET approved_caps_json = ?2, updated_at = ?3 WHERE app_id = ?1",
        params![app_id, serde_json::to_string(&set).unwrap_or_else(|_| "[]".into()), k2_core::deploy::now()],
    )?;
    Ok(())
}

/// The App's approved set. `None` = nothing approved yet: the next
/// deploy is a first install. (Apps that carried a pass before 0.46.1
/// were grandfathered at their live Seed's caps by migration 0173.)
pub fn approved(conn: &Connection, app: &AppRow) -> rusqlite::Result<Option<Vec<String>>> {
    recorded(conn, &app.app_id)
}

/// Does this App get a pass (a link, or the process exception's)? Only
/// then do its caps reach anything, so only then is a deploy's cap set
/// held. The stage-1 process engine (preview, no link) carries no pass;
/// moving it behind the wall makes its next deploy a first install.
pub fn carries_pass(app: &AppRow) -> bool {
    app.isolation == "container" || app.is_process_exception()
}

/// Did the owner approve `caps` before for an App of the same Seed id
/// (§5.6: "unless the caps fit a set the owner approved before")?
pub fn fits_earlier_approval(conn: &Connection, seed_id: &str, caps: &[String]) -> rusqlite::Result<bool> {
    let mut st = conn.prepare("SELECT approved_caps_json FROM apps WHERE seed_id = ?1 AND approved_caps_json IS NOT NULL")?;
    let sets = st.query_map(params![seed_id], |r| r.get::<_, Option<String>>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let want: BTreeSet<String> = uniq(caps).into_iter().collect();
    Ok(sets.into_iter().filter_map(parse_set).any(|s| want.is_subset(&s.into_iter().collect())))
}

/// Is the App suspended (`apps.suspended_at`)?
pub fn is_suspended(conn: &Connection, app_id: &str) -> rusqlite::Result<bool> {
    let at: Option<Option<i64>> = conn.query_row("SELECT suspended_at FROM apps WHERE app_id = ?1", params![app_id], |r| r.get(0)).optional()?;
    Ok(at.flatten().is_some())
}

/// The link-pass caps for an open link (`gateway::open`): the manifest
/// ceiling the engine computed, narrowed by the approved set. A suspended
/// App gets nothing.
pub fn narrow_for_link(app_id: &str, ceiling: &[String], workspace_id: &str) -> Effective {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let passthrough = || {
        let (caps, dropped) = k2_core::skin::link::link_caps(ceiling);
        Effective { caps, rooms: vec![workspace_id.to_string()], dropped, withheld: Vec::new() }
    };
    // No row, or an App whose config carries no pass (the stage-1 process
    // engine): the link is the daemon's own (tests, adopt), no approval
    // regime applies, the ceiling stands.
    let Ok(Some(app)) = store::get_app(&conn, app_id) else { return passthrough() };
    let suspended = is_suspended(&conn, app_id).unwrap_or(true);
    if !carries_pass(&app) && !suspended {
        return passthrough();
    }
    let set = approved(&conn, &app).unwrap_or(None);
    effective(ceiling, set.as_deref(), workspace_id, suspended, &Actor::App)
}

// ── the deploy hold (S20) ────────────────────────────────────────────

/// What a deploy of `seed` asks for beyond the App's approved set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployCheck {
    /// Fits the approved set (or only narrows): deploy now.
    Fits,
    /// First install that asks for nothing, or fits an earlier approval
    /// for the same Seed id: record this set and deploy now.
    FirstFits(Vec<String>),
    /// Held: `kind` (`install` | `deploy`), the caps it adds.
    Held { kind: &'static str, add: Vec<String>, set: Vec<String> },
}

/// Classify a deploy (the always-per-event list, §5.6: every caller, the
/// owner token included).
pub fn check_deploy(conn: &Connection, app: &AppRow, seed: &str) -> rusqlite::Result<DeployCheck> {
    if !carries_pass(app) {
        return Ok(DeployCheck::Fits);
    }
    let row = store::get_seed(conn, seed)?;
    let caps = seed_caps(conn, seed)?.unwrap_or_default();
    let (ceiling, _) = k2_core::skin::link::link_caps(&caps);
    match approved(conn, app)? {
        None => {
            let seed_id = row.as_ref().map(|r| r.seed_id.clone()).or_else(|| app.seed_id.clone()).unwrap_or_default();
            if ceiling.is_empty() || (!seed_id.is_empty() && fits_earlier_approval(conn, &seed_id, &ceiling)?) {
                Ok(DeployCheck::FirstFits(ceiling))
            } else {
                Ok(DeployCheck::Held { kind: reqs::KIND_INSTALL, add: ceiling.clone(), set: ceiling })
            }
        }
        Some(set) => {
            let have: BTreeSet<&str> = set.iter().map(String::as_str).collect();
            let add: Vec<String> = ceiling.iter().filter(|c| !have.contains(c.as_str())).cloned().collect();
            if add.is_empty() {
                Ok(DeployCheck::Fits)
            } else {
                let mut all = set.clone();
                all.extend(add.iter().cloned());
                Ok(DeployCheck::Held { kind: reqs::KIND_DEPLOY, add, set: uniq(&all) })
            }
        }
    }
}

/// The plain-words diff a held deploy/install request carries.
pub fn diff_json(kind: &str, add: &[String], set: &[String]) -> J {
    json!({
        "caps": { "add": add, "approvedAfter": set },
        "lines": add.iter().map(|c| format!("+ cap {c}")).collect::<Vec<_>>(),
        "first": kind == reqs::KIND_INSTALL,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_formula_only_narrows() {
        let m = v(&["thread:read", "files:write", "files:agentconfig", "bogus:cap", "thread:read"]);
        // Nothing approved: nothing carried, everything withheld.
        let e = effective(&m, None, "ws-1", false, &Actor::App);
        assert!(e.caps.is_empty());
        assert_eq!(e.withheld, v(&["thread:read", "files:write"]));
        assert_eq!(e.dropped, v(&["files:agentconfig", "bogus:cap"]));
        assert_eq!(e.rooms, v(&["ws-1"]));
        // Approved ∩ manifest; an approved cap the manifest no longer asks
        // for is not carried; approving `files:agentconfig` doesn't carry it.
        let ap = v(&["thread:read", "store:read", "files:agentconfig"]);
        let e = effective(&m, Some(&ap), "ws-1", false, &Actor::App);
        assert_eq!(e.caps, v(&["thread:read"]));
        assert_eq!(e.withheld, v(&["files:write"]));
        // Guest: ∩ role caps, rooms ∩ the App's room.
        let role = v(&["files:write", "thread:read"]);
        let rooms = v(&["ws-1", "ws-9"]);
        let all = v(&["thread:read", "files:write"]);
        let e = effective(&m, Some(&all), "ws-1", false, &Actor::Guest { role_caps: &role, rooms: &rooms });
        assert_eq!(e.caps, v(&["thread:read", "files:write"]));
        assert_eq!(e.rooms, v(&["ws-1"]));
        let none: Vec<String> = vec![];
        let e = effective(&m, Some(&all), "ws-1", false, &Actor::Guest { role_caps: &v(&["thread:read"]), rooms: &none });
        assert_eq!(e.caps, v(&["thread:read"]));
        assert!(e.rooms.is_empty(), "a guest outside the App's room reaches no room");
        // Suspended: nothing.
        let e = effective(&m, Some(&all), "ws-1", true, &Actor::App);
        assert!(e.caps.is_empty() && e.rooms.is_empty());
    }
}
