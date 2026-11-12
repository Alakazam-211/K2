//! `POST /cli/desktop/env {release?, desktop?}` (prd-linux-virtual-desktop-v1
//! L8, §3.9): an agent's own desktop to put windows on.
//!
//! - **Its own gate** (LD-19), checked ahead of the people-only refusal
//!   every other Desktop route gives an agent: an agent passport, the
//!   Owner's `virtual` switch, and its workspace's "Allow a virtual desktop"
//!   (0301). People, App passes and API keys get `desktop_env_agents_only`.
//! - **Always the caller's own desktop** (LD-33, LD-34):
//!   `virtual:<its workspace id>`, from the passport. A sidecar carries its
//!   workspace's passport, so it resolves to the same desktop (LD-39).
//!   Naming any other desktop is `not_your_desktop`. Created, started (at
//!   1280×720) or woken as needed; safe to repeat (LD-32).
//! - **One holder at a time** (LD-29, LD-25): each call takes or renews a
//!   30-minute hold for the calling session's address (`scientist`,
//!   `scientist/helper`); another session of the workspace is refused with
//!   who and until when (`desktop_in_use`). `{"release": true}` drops it
//!   (also with the switch off).
//! - **What to use**: `DISPLAY`, `XAUTHORITY` and `K2_DESKTOP_PROFILE` (the
//!   desktop's own browser profile folder, LD-31: never shared or copied).
//! - **Audit**: the start or wake it asked for, and every hold taken,
//!   renewed, released or refused, with the session's address.

use std::collections::HashMap;
use std::sync::RwLock;

use k2_core::desktop::audit::{self, AuditLine, Event};
use k2_core::desktop::store;
use k2_core::desktop::virtual_desktop::{self as vd, Asker, State};
use serde_json::{json, Map, Value};

use super::routes::{audit_first, body_json, linux_here, linux_only, ok, refuse, refuse_with, str_field, Req};
use super::virtual_desktop::{manager, StartError};
use super::virtual_routes::{hold_name, setting_up};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const ROUTE: &str = "/cli/desktop/env";

// ── the calling session's address ────────────────────────────────────

static ADDRESSES_FOR_TEST: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// Tests: session id → address, until the guard drops.
pub fn addresses_for_test(m: HashMap<String, String>) -> AddressGuard {
    *ADDRESSES_FOR_TEST.write().unwrap_or_else(|p| p.into_inner()) = Some(m);
    AddressGuard
}

pub struct AddressGuard;

impl Drop for AddressGuard {
    fn drop(&mut self) {
        *ADDRESSES_FOR_TEST.write().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// The session's K2 address: `<workspace>` for its main session,
/// `<workspace>/<sidecar>` for a sidecar (the rules of `k2 whoami`).
fn address(project_id: &str, session_id: &str) -> String {
    if let Some(m) = ADDRESSES_FOR_TEST.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
        if let Some(a) = m.get(session_id) {
            return a.clone();
        }
    }
    crate::cell_identity::sidecar_thread_addr(project_id, session_id).unwrap_or_else(|| crate::compute_routes::ws_handle(project_id))
}

fn agents_only() -> CliResponse {
    refuse(
        403,
        "desktop_env_agents_only",
        "k2 desktop env is for agents: it gives an agent DISPLAY and XAUTHORITY for a desktop of its own. People open a Desktop tab, or run k2 desktop start.",
    )
}

fn line(event: Event, id: &str, addr: &str, outcome: &str) -> AuditLine {
    AuditLine::new(event, id, &format!("agent:{addr}")).role("agent").ingress("cell").outcome(outcome)
}

fn log_line(l: &AuditLine) {
    if let Err(e) = audit::write(l) {
        k2_core::log_debug!("[desktop] audit write failed ({}): {e}", l.event.as_str());
    }
}

/// `POST /cli/desktop/env` (the method is already checked).
pub fn handle(who: &Who, req: &Req<'_>) -> CliResponse {
    let (project_id, session_id) = match who {
        Who::Agent { project_id, session_id } => (project_id.as_str(), session_id.as_str()),
        Who::Invalid => return CliResponse::forbidden(),
        _ => return agents_only(),
    };
    if !linux_here() {
        return linux_only();
    }
    let body = match body_json(req.body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(id) = vd::agent_desktop(project_id) else {
        return refuse(404, "workspace_not_found", "your workspace is not registered");
    };
    if let Some(asked) = str_field(&body, "desktop") {
        if asked != id {
            return refuse(
                403,
                "not_your_desktop",
                format!("k2 desktop env gives you your own workspace's desktop ({id}) and no other: run it without --desktop"),
            );
        }
    }
    let addr = address(project_id, session_id);
    let m = manager();
    if body.get("release").and_then(Value::as_bool) == Some(true) {
        let released = m.release(&id, &addr);
        if released {
            log_line(&line(Event::AgentHold, &id, &addr, "released"));
        }
        return ok(json!({"ok": true, "desktop": id, "released": released}));
    }

    let (name, allowed) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let name = store::desktop_workspace(&conn, &id).map(|(_, n)| n);
        (name, store::agents_can_use_desktop(&conn, project_id))
    };
    let Some(name) = name else {
        return refuse(404, "workspace_not_found", "your workspace is not registered");
    };
    match store::is_enabled(vd::VIRTUAL) {
        Ok(true) => {}
        Ok(false) => {
            return refuse(403, "desktop_off", "Virtual desktops are off on this server: the owner turns them on (Settings → Remote Desktop, or k2 desktop enable outside K2).")
        }
        Err(e) => return refuse(500, "store_error", e),
    }
    if !allowed {
        return refuse(
            403,
            "desktop_switch_off",
            format!("{name}'s \"Allow a virtual desktop\" switch is off: an Owner or Admin turns it on (Settings → Workspaces → {name} → Agent → Desktop, or k2 desktop allow on --workspace {name})."),
        );
    }
    if setting_up() {
        return refuse(409, "setting_up", "The desktop packages are being installed on this server: try again when that ends.");
    }

    // One holder at a time (LD-29): refuse before waking anything.
    let now = m.now();
    let prior = m.status(&id).hold;
    if let Some(h) = prior.as_ref().filter(|h| h.by != addr && h.until > now) {
        log_line(&line(Event::AgentHold, &id, &addr, "refused_in_use"));
        let holder = hold_name(&h.by);
        return refuse_with(
            403,
            "desktop_in_use",
            format!(
                "{holder} ({}) is using {name}'s desktop until {} (Unix time). It frees when that session runs k2 desktop env --release or the hold ends.",
                h.by, h.until
            ),
            json!({"holder": {"by": h.by, "name": holder, "until": h.until}}),
        );
    }
    let renewed = prior.as_ref().is_some_and(|h| h.by == addr && h.until > now);
    if let Err(r) = audit_first(&line(Event::AgentHold, &id, &addr, if renewed { "renewed" } else { "taken" })) {
        return r;
    }
    let hold = match m.hold(&id, &addr) {
        Ok(h) => h,
        Err(h) => {
            return refuse_with(
                403,
                "desktop_in_use",
                format!("{} is using {name}'s desktop until {} (Unix time).", hold_name(&h.by), h.until),
                json!({"holder": {"by": h.by, "name": hold_name(&h.by), "until": h.until}}),
            )
        }
    };

    // Start a closed desktop (1280×720), wake an asleep one, or return it.
    let started_by_agent = match m.state(&id) {
        State::Closed => Some(Event::DesktopStart),
        State::Asleep => Some(Event::DesktopWake),
        _ => None,
    };
    if let Some(ev) = started_by_agent {
        if let Err(r) = audit_first(&line(ev, &id, &addr, "agent")) {
            m.release(&id, &addr);
            return r;
        }
    }
    if let Err(e) = m.start(&id, Asker::Agent, None) {
        m.release(&id, &addr);
        let extra = match &e {
            StartError::TooMany { closable } => json!({"closable": closable}),
            StartError::NotSetUp { missing, manual } => json!({"missing": missing, "manual": manual}),
            StartError::Failed(d) => json!({"detail": d}),
            _ => json!({}),
        };
        let mut hint = e.hint(&m.limits());
        if matches!(e, StartError::NotSetUp { .. }) {
            hint.push_str(" Setting it up is the Owner's: ask your person.");
        }
        return refuse_with(e.status(), e.code(), hint, extra);
    }
    let profile = match m.profile_dir(&id) {
        Ok(p) => p.display().to_string(),
        Err(e) => return refuse(500, "profile_failed", format!("K2 couldn't make the desktop's browser profile folder: {e}")),
    };
    let st = m.status(&id);
    let screen: HashMap<String, String> = m.screen_env(&id).unwrap_or_default().into_iter().collect();
    let display = screen.get("DISPLAY").cloned();
    let xauthority = screen.get("XAUTHORITY").cloned();
    let mut env = Map::new();
    if let (Some(d), Some(x)) = (&display, &xauthority) {
        env.insert("DISPLAY".into(), json!(d));
        env.insert("XAUTHORITY".into(), json!(x));
        env.insert("K2_DESKTOP_PROFILE".into(), json!(profile));
    }
    ok(json!({
        "ok": true,
        "desktop": id,
        "name": name,
        "state": st.wire_state(),
        "message": st.message(),
        "display": display,
        "xauthority": xauthority,
        "profile": profile,
        "env": env,
        "hold": {"by": hold.by, "until": hold.until},
        "holdFor": vd::HOLD_FOR,
        "label": vd::SEPARATE_LABEL,
    }))
}

#[cfg(test)]
#[path = "env_routes_tests.rs"]
mod tests;
