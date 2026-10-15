//! `POST /cli/app/exception {app, on, devices?, confirmToken?, reason?}`:
//! the `process` exception (`prd-k2-app-runtime-v1` §4.1, §4.3).
//!
//! Turning it **on** loosens the App's wall (a sandboxed system unit
//! instead of the guarded container, plus device grants): only the
//! server's owner, in person, with a fresh owner confirm bound to this App
//! (vault action `app-process-exception`; "lowering to process always
//! needs a per-event approval"). Agents never: they get `owner_only` with
//! the command to hand to their human. Turning it **off** (back to the
//! guarded container) is a tightening: anyone who may manage the App.
//!
//! Either way the next deploy or restart uses the new wall; both are
//! audited (deploy audit + Access Audit).

use k2_core::connect_users::Role;
use k2_core::deploy::{self, codes, store};
use k2_root_proto::apps::{device, AppId, MAX_DEVICES};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, mirror, need_credential, need_manage, no, s, R};
use crate::compute_routes::Who;

/// The vault confirm action (`vault_routes::CONFIRM_ACTIONS`); the token
/// is bound to the App id.
pub const CONFIRM_ACTION: &str = "app-process-exception";
pub const OWNER_CONFIRM_REQUIRED: &str = "owner_confirm_required";

fn confirm_stamp(who: &Who) -> Option<String> {
    match who {
        Who::Owner => Some("owner".into()),
        Who::Login { username, role: Role::Owner } => Some(format!("connect:{username}")),
        _ => None,
    }
}

/// Device tokens from the body: known tokens only, no repeats, at most
/// [`MAX_DEVICES`].
pub(crate) fn devices_of(v: &J) -> R<Vec<String>> {
    let Some(a) = v.get("devices") else { return Ok(vec![]) };
    let a = a.as_array().ok_or_else(|| no(codes::BAD_REQUEST, 400, "devices is a list of device names (dri-render128, nvidia0, …)"))?;
    let mut out: Vec<String> = Vec::new();
    for d in a {
        let t = d.as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "devices holds names only"))?;
        if device(t).is_none() {
            return Err(no(
                codes::BAD_REQUEST,
                400,
                format!("'{t}' isn't a device K2 grants (dri-render128..191, dri-card0..15, nvidia0..15, nvidiactl, nvidia-uvm, nvidia-uvm-tools, nvidia-modeset)"),
            ));
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    if out.len() > MAX_DEVICES {
        return Err(no(codes::BAD_REQUEST, 400, format!("at most {MAX_DEVICES} devices")));
    }
    Ok(out)
}

fn audit(app: &str, event: &str, who: &Who, payload: J) {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app, event, &actor(who), None, &payload) {
            k2_core::log_debug!("[apps] audit {app} {event}: {e}");
        }
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        &format!("app-{}", event.replace('_', "-")),
        &actor(who),
        format!("app={app}"),
        "cli",
        "-",
        "cli",
    ));
}

/// The exception's warning, shown on the way in.
pub const WARNING: &str = "This App leaves the guarded container. It runs as a sandboxed system process (user k2appx): \
read-only system, no homes, no capabilities, loopback only, its own caps, the k2-app AppArmor profile where AppArmor is on, \
and only the devices you granted. Weaker than the container: process Apps share one user, so they can see each other; \
a deploy is a restart (no warm previous version); k2 app exec and shell don't reach it yet. \
Use it only for a device a container can't reach. Undo: k2 app exception <app> off.";

pub(crate) fn route(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let on = v.get("on").and_then(|x| x.as_bool()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "on is true or false"))?;
    if !on {
        need_manage(who)?;
        let app = app_for(who, s(&v, "app"))?;
        if !app.is_process_exception() {
            return Ok(json!({ "ok": true, "app": app.app_id, "changed": false, "label": super::routes::label(&app) }));
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_process_exception(&conn, &app.app_id, None).map_err(super::routes::db_err)?;
        }
        audit(&app.app_id, "exception_off", who, json!({ "was": app.exception_json, "devices": app.devices() }));
        mirror(&app.app_id);
        let row = app_for(who, Some(&app.app_id))?;
        return Ok(json!({
            "ok": true,
            "app": app.app_id,
            "changed": true,
            "label": super::routes::label(&row),
            "next": format!("its next deploy or restart runs it in the guarded container: k2 app restart {}", app.app_id),
        }));
    }
    // On: the owner, in person.
    if !who.is_owner() {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "moving an App out of its guarded container is the server owner's call, in person: ask your owner to run \
             `k2 app exception <app> on [--device …]` from a terminal outside K2 (it asks for an owner confirm)",
        ));
    }
    let app = app_for(who, s(&v, "app"))?;
    if !cfg!(target_os = "linux") {
        return Err(no(codes::ENGINE_UNAVAILABLE, 409, "the process exception runs on Linux boxes only"));
    }
    if AppId::parse(&app.app_id).is_none() {
        return Err(no(codes::APP_ID_INVALID, 409, format!("App id '{}' doesn't fit a unit name (at most 32 characters, no '--')", app.app_id)));
    }
    let devices = devices_of(&v)?;
    let Some(token) = s(&v, "confirmToken") else {
        return Err(no(
            OWNER_CONFIRM_REQUIRED,
            403,
            "this needs a fresh owner confirm (Touch ID, your login or the vault password): k2 app exception <app> on asks for it",
        ));
    };
    let stamp = confirm_stamp(who).ok_or_else(|| no(codes::OWNER_ONLY, 403, "owner only"))?;
    let vault = crate::vault::get().map_err(|e| no(codes::ENGINE_UNAVAILABLE, 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))?;
    let proof = vault
        .confirm
        .take(token, CONFIRM_ACTION, Some(&app.app_id), &stamp)
        .map_err(|_| no(OWNER_CONFIRM_REQUIRED, 403, "confirm again: a confirm works once, for this App, within 60 seconds"))?;
    let via = match proof {
        k2_vault::HumanProof::Confirmed { method } => method,
        k2_vault::HumanProof::None => return Err(no(OWNER_CONFIRM_REQUIRED, 403, "no owner confirm")),
    };
    let reason = s(&v, "reason").map(|r| r.chars().take(500).collect::<String>());
    let approval = json!({ "by": actor(who), "at": deploy::now(), "via": via, "reason": reason, "devices": devices }).to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_process_exception(&conn, &app.app_id, Some((&approval, &devices))).map_err(super::routes::db_err)?;
    }
    audit(
        &app.app_id,
        "exception_on",
        who,
        json!({ "devices": devices, "via": via, "reason": reason, "was": { "path": app.path, "isolation": app.isolation } }),
    );
    mirror(&app.app_id);
    let row = app_for(who, Some(&app.app_id))?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "changed": true,
        "label": super::routes::label(&row),
        "devices": devices,
        "warning": WARNING,
        "next": format!("its next deploy or restart runs it as the process exception: k2 app restart {}", app.app_id),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_lists_are_tokens_without_repeats() {
        assert_eq!(devices_of(&json!({})).unwrap(), Vec::<String>::new());
        assert_eq!(devices_of(&json!({ "devices": ["dri-render128", "dri-render128", "nvidia0"] })).unwrap(), vec!["dri-render128", "nvidia0"]);
        for bad in [json!({ "devices": "dri-render128" }), json!({ "devices": ["/dev/sda"] }), json!({ "devices": ["kvm"] }), json!({ "devices": [1] })] {
            assert!(devices_of(&bad).is_err(), "{bad}");
        }
        let nine: Vec<String> = (0..9).map(|i| format!("nvidia{i}")).collect();
        assert!(devices_of(&json!({ "devices": nine })).is_err());
    }

    #[test]
    fn only_the_owner_turns_it_on() {
        for who in [
            Who::Agent { project_id: "w".into(), session_id: "s".into() },
            Who::Shell { project_id: "w".into(), session_id: "s".into() },
            Who::Login { username: "ann".into(), role: Role::Admin },
            Who::Login { username: "bob".into(), role: Role::Member },
        ] {
            let e = route(&who, br#"{"app":"jobs","on":true}"#).unwrap_err();
            assert_eq!((e.code, e.status), (codes::OWNER_ONLY, 403), "{who:?}");
        }
        assert_eq!(route(&Who::Wrong, br#"{"app":"jobs","on":false}"#).unwrap_err().code, codes::WRONG_CREDENTIAL);
        assert_eq!(confirm_stamp(&Who::Owner).as_deref(), Some("owner"));
        assert_eq!(confirm_stamp(&Who::Login { username: "z".into(), role: Role::Owner }).as_deref(), Some("connect:z"));
        assert!(confirm_stamp(&Who::Login { username: "z".into(), role: Role::Admin }).is_none());
    }
}
