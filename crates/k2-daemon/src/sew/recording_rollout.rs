//! The `voice:record` cap rollout (K2 PRD prd-k2-bobbin-in-apps-v1 §5,
//! amendment B17; Sew 0.9.0). The decisions are
//! [`k2_core::tool_hook::recording`]'s; this reads what lives in the main
//! DB (each App's live manifest), widens role lines through the skin API,
//! and writes the audit row and the Thread notice.
//!
//! Runs on every reconcile sweep, before the settings step, with the flags
//! Sew just reported. It does nothing while [`k2_core::recording::in_apps`]
//! is off (the pin slice flips it) or Sew lacks `voice_record_cap`, so it
//! is inert on Sew 0.8.x.

use k2_core::tool_hook::features::RecordingFeatures;
use k2_core::tool_hook::recording::{self as rec, ManifestChat, Step};
use k2_core::tool_hook::{self as hook};
use serde_json::json;

/// What one sweep's rollout did, per App.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Apps whose roles were widened, with the roles.
    pub grandfathered: Vec<(String, Vec<String>)>,
    /// Apps that got `cap_required`.
    pub capped: Vec<String>,
    /// Apps skipped, and why.
    pub skipped: Vec<(String, String)>,
}

/// The App's live manifest's chat caps.
fn manifest_chat(service_id: &str) -> Result<ManifestChat, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(app) = k2_core::deploy::store::get_app_by_service(&conn, service_id).map_err(|e| format!("app lookup: {e}"))? else {
        return Ok(ManifestChat::NotDeclared);
    };
    let Some(seed) = app.live_seed.as_deref() else { return Ok(ManifestChat::NotDeclared) };
    let caps = crate::apps::grants::seed_caps(&conn, seed).map_err(|e| format!("seed caps: {e}"))?.unwrap_or_default();
    let chat: Vec<String> = caps.into_iter().map(|c| c.trim().to_string()).filter(|c| k2_core::skin::is_chat_cap(c)).collect();
    Ok(if chat.is_empty() { ManifestChat::NotDeclared } else { ManifestChat::Declared(chat) })
}

/// Add `voice:record` to every `voice:join` room line of the App's roles.
/// Returns the roles widened, by name.
fn grandfather_roles(service_id: &str) -> Result<Vec<String>, String> {
    let mut widened = Vec::new();
    for role in k2_core::skin::roles_for_app(service_id)? {
        if let Some(policy) = rec::grandfather_policy(&role.room_policy) {
            k2_core::skin::update_role(&role.id, Some(&policy))?;
            widened.push(role.name.clone());
        }
    }
    Ok(widened)
}

/// One `deploy_audit` row and one notice on the maintaining agent's Thread.
fn tell(service_id: &str, roles: &[String]) {
    let (app_id, name, ws) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = k2_core::deploy::store::get_app_by_service(&conn, service_id).ok().flatten();
        let name: Option<String> = k2_core::published_services::get_by_id(&conn, service_id).ok().flatten().map(|s| s.name);
        let payload = json!({"service": service_id, "roles": roles, "cap": k2_core::skin::CAP_VOICE_RECORD});
        let subject = row.as_ref().map(|r| r.app_id.clone()).unwrap_or_else(|| service_id.to_string());
        if let Err(e) = k2_core::deploy::audit::append(&conn, k2_core::deploy::audit::SUBJECT_APP, &subject, rec::AUDIT_GRANDFATHER, rec::ACTOR_ROLLOUT, None, &payload) {
            k2_core::log_debug!("[recording] audit {service_id}: {e}");
        }
        (subject, name.unwrap_or_else(|| service_id.to_string()), row.map(|r| r.owner_workspace).filter(|w| !w.trim().is_empty()))
    };
    let Some(ws) = ws.or_else(|| k2_core::skin::ties::app_workspace(service_id)) else { return };
    let handle = crate::permission::present::workspace_handle(&ws);
    let who = if roles.is_empty() { "No role of it held voice:join, so no role changed".to_string() } else { format!("Roles widened: {}", roles.join(", ")) };
    let text = format!(
        "K2 gave the voice:record cap to every role line of your App {name} that can join calls, so people who could \
         press Record before still can now that this server's chat checks the cap. {who}. Each person in a call still \
         consents for themselves. To narrow it, take voice:record off a role: k2 app role room <role> <workspace> --caps …"
    );
    if let Err(e) = crate::overlay_routes::post_workspace_notice(&handle, &handle, &text) {
        k2_core::log_debug!("[recording] notice {app_id}: {e}");
    }
}

/// One sweep's rollout, with the flags Sew just reported.
pub fn run(flags: &[String]) -> Summary {
    let mut s = Summary::default();
    if !k2_core::recording::in_apps() {
        return s;
    }
    let record_cap = RecordingFeatures::from_flags(flags).voice_record_cap;
    if !record_cap {
        return s;
    }
    let apps = match hook::recording_apps() {
        Ok(a) => a,
        Err(e) => {
            k2_core::log_debug!("[recording] rollout: {e}");
            return s;
        }
    };
    for app in apps {
        let manifest = match manifest_chat(&app) {
            Ok(m) => m,
            Err(e) => {
                s.skipped.push((app.clone(), e));
                continue;
            }
        };
        // At most two steps per App per sweep: grandfather, then the cap.
        for _ in 0..2 {
            match hook::recording_rollout_step(&app, &manifest, record_cap) {
                Ok(Step::Grandfather) => match grandfather_roles(&app) {
                    Ok(roles) => {
                        if let Err(e) = hook::recording_note_grandfathered(&app, &roles) {
                            s.skipped.push((app.clone(), e));
                            break;
                        }
                        tell(&app, &roles);
                        s.grandfathered.push((app.clone(), roles));
                    }
                    Err(e) => {
                        s.skipped.push((app.clone(), e));
                        break;
                    }
                },
                Ok(Step::CapRequired) => {
                    match hook::recording_set_cap_required(&app) {
                        Ok(_) => s.capped.push(app.clone()),
                        Err(e) => s.skipped.push((app.clone(), e)),
                    }
                    break;
                }
                Ok(Step::Done) => break,
                Ok(Step::Skip { why }) => {
                    s.skipped.push((app.clone(), why));
                    break;
                }
                Err(e) => {
                    s.skipped.push((app.clone(), e));
                    break;
                }
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVC: &str = "7d4e2a10-1111-4222-8333-444455556666";
    const VOICE_RECORD_CAP: &str = k2_core::tool_hook::features::VOICE_RECORD_CAP;

    /// PRD §5 step 4 (Q1, B17): with the pin's const on and Sew's flag, a
    /// role line holding `voice:join` gets `voice:record` once, then the
    /// App gets `cap_required`; nothing happens before the pin or on Sew
    /// 0.8.x.
    #[test]
    fn the_rollout_grandfathers_then_caps_and_is_inert_before() {
        let _home = k2_core::test_env::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let mut policy = k2_core::skin::RoomPolicy::new();
        policy.insert("room-a".into(), vec!["thread:read".into(), "voice:join".into()]);
        policy.insert("room-b".into(), vec!["thread:read".into()]);
        k2_core::skin::create_role_for_app("crew", &policy, Some(SVC)).expect("role");
        hook::set_app_home(SVC, false, "Jobs", "owner").expect("settings row");
        let flags = vec![VOICE_RECORD_CAP.to_string()];

        // The pin's const is on (K2 0.46.11): no override, and still inert
        // on a box whose Sew doesn't report voice_record_cap (0.8.x).
        k2_core::recording::set_for_tests(false, false);
        assert!(k2_core::recording::RECORDING_IN_APPS);
        assert_eq!(run(&[]), Summary::default(), "Sew 0.8.x: inert");

        let s = run(&flags);
        assert_eq!(s.grandfathered, vec![(SVC.to_string(), vec!["crew".to_string()])], "{s:?}");
        assert_eq!(s.capped, vec![SVC.to_string()]);
        let role = k2_core::skin::find_role("crew").unwrap().unwrap();
        assert_eq!(role.room_policy["room-a"], vec!["thread:read", "voice:join", "voice:record"]);
        assert_eq!(role.room_policy["room-b"], vec!["thread:read"], "a line without voice:join is untouched");
        let again = run(&flags);
        assert!(again.grandfathered.is_empty() && again.capped.is_empty(), "once: {again:?}");
        let audit: i64 = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT COUNT(*) FROM deploy_audit WHERE event = ?1", [rec::AUDIT_GRANDFATHER], |r| r.get(0)).unwrap()
        };
        assert_eq!(audit, 1, "one audit row per App");
        k2_core::recording::set_for_tests(false, false);
    }
}
