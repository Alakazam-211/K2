//! Who is calling a docs route, and the participant K2 stamps for them
//! (Markdown multiplayer v1, K2; vs-live MI8, PRD K-ID-1, K-ID-7).
//!
//! The four ways in (the `dispatcher.rs` resolver order, MI8):
//! 1. an App pass (`k2skn_`): refused for now (App guests come with the
//!    App surface, K6: `files:read` / `files:write` in the doc's room);
//! 2. the owner token, or a Connect login: a **person**;
//! 3. an agent passport, on Loopback TCP or its cell socket: an **agent**
//!    for its own workspace, or the **person** when the passport is a
//!    plain shell tab (a human typing in K2, attributed "via terminal");
//!    a sandboxed cell or an API cell is refused;
//! 4. anything else: 403.
//!
//! Identity is never taken from the request: the pid, name, colour and
//! kind come from K2's own records.

use k2_multiplayer_base::session::{Participant, ParticipantKind};

use super::host::{Principal, AGENT_PREFIX, PERSON_PREFIX};
use crate::cli_response::CliResponse;
use crate::session_token::HookPrincipal;

/// How a person or agent reached the route (logged per change as `via`).
pub const VIA_CLI: &str = "cli";
pub const VIA_TERMINAL: &str = "terminal";

/// A resolved caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocCaller {
    /// The owner token, a login (by its person key: `owner` for the owner
    /// and the logins the owner linked), or a shell tab's passport.
    Person { key: String, via: &'static str },
    /// An agent, by its workspace's project id.
    Agent { project_id: String },
}

impl DocCaller {
    pub fn principal(&self) -> Principal {
        match self {
            DocCaller::Person { key, .. } => Principal::Person(key.clone()),
            DocCaller::Agent { project_id } => Principal::Agent(project_id.clone()),
        }
    }

    pub fn pid(&self) -> String {
        self.principal().pid()
    }

    /// The workspace a relative path resolves in when the call names none.
    pub fn default_workspace(&self) -> Option<&str> {
        match self {
            DocCaller::Agent { project_id } => Some(project_id),
            DocCaller::Person { .. } => None,
        }
    }

    pub fn is_person(&self) -> bool {
        matches!(self, DocCaller::Person { .. })
    }

    /// The participant, stamped from K2's records. `on_behalf_of` (a
    /// person's pid, already checked by [`on_behalf_pid`]) marks an agent
    /// acting for that person; it is ignored for people.
    pub fn participant(&self, on_behalf_of: Option<&str>) -> Participant {
        match self {
            DocCaller::Person { key, via } => {
                let people = k2_core::notify_prefs::People::load().unwrap_or_default();
                let owner_color = k2_core::app_settings::load().owner_color;
                Participant::new(
                    format!("{PERSON_PREFIX}{key}"),
                    k2_core::person_name::effective(key, &people).display_name,
                    k2_core::person_color::effective(key, &people, owner_color.as_deref()),
                    ParticipantKind::Person,
                    *via,
                )
            }
            DocCaller::Agent { project_id } => {
                let (path, color) = {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    k2_core::db::schema::Project::get(&conn, project_id)
                        .map(|p| (p.path, p.color))
                        .unwrap_or_default()
                };
                let name = if path.is_empty() {
                    "agent".to_string()
                } else {
                    k2_core::workspace::display::agent_display_name(&path)
                };
                let colour = k2_core::workspace::agent_look::normalize_color(&color)
                    .unwrap_or_else(|_| k2_core::person_color::hash_color(project_id).to_string());
                let p = Participant::new(format!("{AGENT_PREFIX}{project_id}"), name, colour, ParticipantKind::Agent, VIA_CLI);
                match on_behalf_of {
                    Some(person) => p.on_behalf_of(person),
                    None => p,
                }
            }
        }
    }
}

/// `onBehalfOf` (K-ID-10): only an agent may act for someone, and only for
/// a person on this box (`owner`, or a Connect login; `person:<key>` too).
/// Returns that person's pid. Acting for someone never widens the agent's
/// rights (the kit asks about the agent's own pid).
pub fn on_behalf_pid(caller: &DocCaller, named: Option<&str>) -> Result<Option<String>, CliResponse> {
    let Some(named) = named.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if caller.is_person() {
        return Err(refusal(
            "400 Bad Request",
            "bad_request",
            "only an agent acts on someone's behalf; a person edits as themselves",
        ));
    }
    let login = named.strip_prefix(PERSON_PREFIX).unwrap_or(named);
    let key = k2_core::notify_prefs::People::load()
        .map(|p| p.person_key(login))
        .unwrap_or_else(|_| login.to_ascii_lowercase());
    if !super::host::person_exists(&key) {
        return Err(refusal("404 Not Found", "person_not_found", format!("no person '{named}' on this server")));
    }
    Ok(Some(format!("{PERSON_PREFIX}{key}")))
}

/// The owner daemon token.
pub fn owner() -> DocCaller {
    DocCaller::Person { key: k2_core::notify_prefs::OWNER.to_string(), via: VIA_CLI }
}

/// A live Connect login (its role already cleared the route floor).
pub fn login(username: &str) -> DocCaller {
    let key = k2_core::notify_prefs::People::load()
        .map(|p| p.person_key(username))
        .unwrap_or_else(|_| username.trim().to_ascii_lowercase());
    DocCaller::Person { key, via: VIA_CLI }
}

/// `{"ok":false,"error":{"code","hint"}}`, the comms refusal shape the
/// refusal wrapper (`permission/walls.rs`) reads.
pub fn refusal(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

/// An agent passport's session (cell socket, or Loopback TCP checked by
/// the dispatcher first).
pub fn from_passport(session_id: &str, principal: &HookPrincipal) -> Result<DocCaller, CliResponse> {
    use crate::sidecar_routes::Caller;
    match crate::sidecar_routes::caller_from_session(session_id, principal) {
        Caller::Shell { .. } => Ok(DocCaller::Person { key: k2_core::notify_prefs::OWNER.to_string(), via: VIA_TERMINAL }),
        Caller::Agent(a) if a.sandboxed => Err(refusal(
            "403 Forbidden",
            "sandboxed_agent",
            "a sandboxed session can't open host files as live docs",
        )),
        Caller::Agent(a) if a.project_id.trim().is_empty() => Err(refusal(
            "403 Forbidden",
            "forbidden",
            "scoped session principal has no resolvable workspace",
        )),
        Caller::Agent(a) => Ok(DocCaller::Agent { project_id: a.project_id }),
        Caller::WrongCredential => Err(refusal(
            "403 Forbidden",
            "wrong_credential",
            "this session's passport can't use k2 doc",
        )),
        Caller::Owner | Caller::Login { .. } | Caller::Invalid => Err(CliResponse::forbidden()),
    }
}

/// An App pass: not yet (K6).
pub fn app_pass_refused() -> CliResponse {
    refusal("403 Forbidden", "app_docs_not_available", "live docs aren't on the App surface yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_follow_the_principal() {
        assert_eq!(owner().pid(), "person:owner");
        assert_eq!(DocCaller::Agent { project_id: "p1".into() }.pid(), "agent:p1");
        assert_eq!(DocCaller::Agent { project_id: "p1".into() }.default_workspace(), Some("p1"));
        assert_eq!(owner().default_workspace(), None);
    }

    #[test]
    fn refusals_carry_a_code_the_walls_can_read() {
        let r = refusal("403 Forbidden", "not_connected", "x");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "not_connected");
        assert_eq!(v["ok"], false);
    }
}
