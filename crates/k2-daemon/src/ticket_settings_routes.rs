//! Settings → Tickets (0.45.4): the server-wide rules on agent tickets.
//!
//! - `GET  /cli/tickets/settings` — the rules, the people the default
//!   assignee may be, and whether the caller may change them (`canEdit`).
//!   Any login (Member floor) or the owner token.
//! - `POST /cli/tickets/settings` — change any of the three. Owner token
//!   or an Owner/Admin login (Admin floor in `route_policy`). An agent
//!   passport is refused (`owner_only`): an agent must never loosen the
//!   rules it is held to. `k2 tickets settings` sends the session passport
//!   from inside a K2 terminal for exactly that reason.
//!
//! The values live in `settings.json` under `tickets`
//! (`k2_core::app_settings::TicketSettings`); `feedback_routes` reads
//! them per ticket through `k2_core::feedback_brief::ticket_rules`.
//!
//! Body (JSON, every key optional, at least one):
//! `{"briefPolicy": "off|warn|require", "assigneePolicy": "off|warn|require",
//!   "defaultAssignee": "owner" | "<connect user>" | "none" | "" | null}`.
//!
//! Notifications: a ticket's push already narrows to its assignees, so a
//! default assignee also routes the push. A separate notification switch
//! is later work (no natural switch exists in `push_routes` today).

use crate::cli_response::CliResponse;
use k2_core::feedback_brief::TicketRule;

/// The one path (GET + POST).
pub const PATH: &str = "/cli/tickets/settings";

/// Body cap for the POST (three short strings).
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Who is asking to change the rules, resolved from the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writer {
    /// The owner daemon token (a terminal outside K2, the desktop app).
    Owner,
    /// An Owner- or Admin-role Connect login.
    Manager { username: String },
    /// A Member login.
    Member { username: String },
    /// An agent (or shell) session passport.
    Agent,
    /// No valid credential (or an app pass / API key).
    Invalid,
}

impl Writer {
    pub fn can_edit(&self) -> bool {
        matches!(self, Writer::Owner | Writer::Manager { .. })
    }

    /// Audit label.
    pub fn audit_user(&self) -> String {
        match self {
            Writer::Owner => "owner".to_string(),
            Writer::Manager { username } | Writer::Member { username } => {
                format!("login:{username}")
            }
            Writer::Agent => "agent-passport".to_string(),
            Writer::Invalid => "-".to_string(),
        }
    }
}

/// Classify a TCP caller. `query` is the dispatcher's effective auth
/// query (Bearer / cookie already folded into `token=`).
pub fn writer_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Writer {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    if presented.is_empty() {
        return Writer::Invalid;
    }
    if crate::routes::http::ct_eq_token(presented, owner_token) {
        return Writer::Owner;
    }
    if let Some(username) = k2_core::connect_users::validate_session(presented) {
        let role = k2_core::connect_users::role_for_user(&username)
            .unwrap_or(k2_core::connect_users::Role::Member);
        return if k2_core::connect_users::can_manage_users(role) {
            Writer::Manager { username }
        } else {
            Writer::Member { username }
        };
    }
    if crate::session_token::validate_hook(presented).is_some() {
        return Writer::Agent;
    }
    Writer::Invalid
}

fn refusal(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint.into() },
        })
        .to_string(),
    }
}

/// The people a default assignee may be: the owner (stored as the wire
/// literal `owner`) and every Connect user. `label` is what the picker
/// shows.
fn people() -> Vec<serde_json::Value> {
    let owner_display = crate::workspace_msg::resolve_owner_from();
    let mut out = vec![serde_json::json!({
        "value": "owner",
        "label": if owner_display.trim().is_empty() || owner_display == "owner" {
            "Owner".to_string()
        } else {
            format!("{} (owner)", owner_display.trim())
        },
        "role": "owner",
        "disabled": false,
    })];
    match k2_core::connect_users::list_users() {
        Ok(users) => {
            for u in users {
                // The owner's own Connect login (same name as the display
                // name) is the owner row above.
                if u.username.eq_ignore_ascii_case(owner_display.trim()) {
                    continue;
                }
                let role = serde_json::to_value(u.role).unwrap_or(serde_json::Value::Null);
                out.push(serde_json::json!({
                    "value": u.username,
                    "label": u.username,
                    "role": role,
                    "disabled": u.disabled,
                }));
            }
        }
        Err(e) => {
            k2_core::log_debug!("[tickets-settings] people list failed: {e}");
        }
    }
    out
}

/// Map a requested default assignee onto a stored value: `None` = clear,
/// `Some("owner")` for the owner (the literal or the display name), or the
/// Connect user's stored spelling. `Err` = not a user here.
fn resolve_default_assignee(raw: &str) -> Result<Option<String>, String> {
    let Some(who) = k2_core::feedback_brief::normalize_default_assignee(raw) else {
        return Ok(None);
    };
    let owner_display = crate::workspace_msg::resolve_owner_from();
    if who.eq_ignore_ascii_case("owner") || who.eq_ignore_ascii_case(owner_display.trim()) {
        return Ok(Some("owner".to_string()));
    }
    let users = k2_core::connect_users::list_users().map_err(|e| format!("users: {e}"))?;
    match users.into_iter().find(|u| u.username.eq_ignore_ascii_case(&who)) {
        Some(u) => Ok(Some(u.username)),
        None => Err(who),
    }
}

/// The current rules as the wire shape (GET and the POST echo).
pub fn view(can_edit: bool) -> serde_json::Value {
    let t = k2_core::app_settings::load().tickets;
    let rules = k2_core::feedback_brief::TicketRules::from_settings(&t);
    let default_known = match rules.default_assignee.as_deref() {
        None => true,
        Some(who) => matches!(resolve_default_assignee(who), Ok(Some(_))),
    };
    serde_json::json!({
        "ok": true,
        "briefPolicy": rules.brief.as_wire(),
        "assigneePolicy": rules.assignee.as_wire(),
        "defaultAssignee": rules.default_assignee,
        "defaultAssigneeKnown": default_known,
        "values": ["off", "warn", "require"],
        "people": people(),
        "canEdit": can_edit,
    })
}

/// `GET /cli/tickets/settings`. The dispatcher already checked the token.
pub fn handle_get(can_edit: bool) -> CliResponse {
    CliResponse::ok_json(view(can_edit).to_string())
}

/// The parsed POST body: only the keys that were sent.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub brief_policy: Option<TicketRule>,
    pub assignee_policy: Option<TicketRule>,
    /// `Some(None)` = clear the default assignee.
    pub default_assignee: Option<Option<String>>,
}

/// Parse and check a POST body. Pure (no settings or user store), except
/// that the default assignee is looked up by [`handle_post`].
pub fn parse_change(body: &[u8]) -> Result<(Change, Option<String>), CliResponse> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| {
        refusal("400 Bad Request", "bad_request", format!("invalid JSON body: {e}"))
    })?;
    let Some(obj) = v.as_object() else {
        return Err(refusal("400 Bad Request", "bad_request", "expected a JSON object"));
    };
    if let Some(k) = obj
        .keys()
        .find(|k| !matches!(k.as_str(), "briefPolicy" | "assigneePolicy" | "defaultAssignee"))
    {
        return Err(refusal(
            "400 Bad Request",
            "bad_request",
            format!("unknown key {k:?}: use briefPolicy, assigneePolicy or defaultAssignee"),
        ));
    }
    if obj.is_empty() {
        return Err(refusal(
            "400 Bad Request",
            "bad_request",
            "nothing to change: send briefPolicy, assigneePolicy or defaultAssignee",
        ));
    }
    let rule = |key: &str, flag: &str| -> Result<Option<TicketRule>, CliResponse> {
        match obj.get(key) {
            None => Ok(None),
            Some(serde_json::Value::String(s)) => TicketRule::from_wire(s).map(Some).ok_or_else(|| {
                refusal(
                    "400 Bad Request",
                    "bad_value",
                    format!("{flag} must be off, warn or require (got {s:?})"),
                )
            }),
            Some(other) => Err(refusal(
                "400 Bad Request",
                "bad_value",
                format!("{flag} must be off, warn or require (got {other})"),
            )),
        }
    };
    let brief_policy = rule("briefPolicy", "--brief")?;
    let assignee_policy = rule("assigneePolicy", "--assignee-required")?;
    let raw_default = match obj.get("defaultAssignee") {
        None => None,
        Some(serde_json::Value::Null) => Some(String::new()),
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return Err(refusal(
                "400 Bad Request",
                "bad_value",
                format!("--default-assignee must be a user name or none (got {other})"),
            ))
        }
    };
    Ok((
        Change { brief_policy, assignee_policy, default_assignee: None },
        raw_default,
    ))
}

/// `POST /cli/tickets/settings`. Returns the response and an audit
/// outcome string.
pub fn handle_post(body: &[u8], writer: &Writer) -> (CliResponse, String) {
    match writer {
        Writer::Owner | Writer::Manager { .. } => {}
        Writer::Agent => {
            return (
                refusal(
                    "403 Forbidden",
                    "owner_only",
                    "Ticket rules are owner settings: an agent can't change them. Ask your \
                     human to use Settings → Tickets, or to run `k2 tickets settings` from a \
                     terminal outside K2 (ran as agent; run as owner).",
                ),
                "refused:agent".to_string(),
            )
        }
        Writer::Member { .. } => {
            return (
                refusal(
                    "403 Forbidden",
                    "role_required",
                    "only the owner or an admin may change the ticket rules",
                ),
                "refused:member".to_string(),
            )
        }
        Writer::Invalid => {
            return (
                refusal("403 Forbidden", "forbidden", "invalid or missing token"),
                "refused:invalid".to_string(),
            )
        }
    }
    let (mut change, raw_default) = match parse_change(body) {
        Ok(p) => p,
        Err(r) => return (r, "refused:bad_request".to_string()),
    };
    if let Some(raw) = raw_default {
        match resolve_default_assignee(&raw) {
            Ok(who) => change.default_assignee = Some(who),
            Err(who) => {
                return (
                    refusal(
                        "400 Bad Request",
                        "unknown_user",
                        format!(
                            "`{who}` is not a user on this server. The default assignee is \
                             the owner (`owner`) or a Connect user: k2 connections list --users"
                        ),
                    ),
                    "refused:unknown_user".to_string(),
                )
            }
        }
    }
    let mut partial = serde_json::Map::new();
    if let Some(r) = change.brief_policy {
        partial.insert("briefPolicy".into(), serde_json::json!(r.as_wire()));
    }
    if let Some(r) = change.assignee_policy {
        partial.insert("assigneePolicy".into(), serde_json::json!(r.as_wire()));
    }
    if let Some(who) = &change.default_assignee {
        partial.insert(
            "defaultAssignee".into(),
            serde_json::json!(who.clone().unwrap_or_default()),
        );
    }
    let outcome = format!(
        "ok {}",
        partial
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join(" ")
    );
    if let Err(e) = k2_core::app_settings::update(serde_json::json!({ "tickets": partial })) {
        return (
            refusal("400 Bad Request", "bad_value", e),
            "refused:update".to_string(),
        );
    }
    // Open Settings windows re-read (`sync:settings`).
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        serde_json::Value::Null,
    );
    (CliResponse::ok_json(view(true).to_string()), outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_code(r: &CliResponse) -> String {
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        v["error"]["code"].as_str().expect("error code").to_string()
    }

    fn ok<T>(r: Result<T, CliResponse>) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("expected Ok, got {} {}", e.status, e.body),
        }
    }

    fn refused<T>(r: Result<T, CliResponse>) -> CliResponse {
        match r {
            Ok(_) => panic!("expected a refusal"),
            Err(e) => e,
        }
    }

    #[test]
    fn parse_change_reads_each_key() {
        let (c, d) = ok(parse_change(br#"{"briefPolicy":"require"}"#));
        assert_eq!(c.brief_policy, Some(TicketRule::Require));
        assert_eq!(c.assignee_policy, None);
        assert_eq!(d, None);
        let (c, d) = ok(parse_change(br#"{"assigneePolicy":"OFF","defaultAssignee":"julie"}"#));
        assert_eq!(c.assignee_policy, Some(TicketRule::Off));
        assert_eq!(d.as_deref(), Some("julie"));
        let (_, d) = ok(parse_change(br#"{"defaultAssignee":null}"#));
        assert_eq!(d.as_deref(), Some(""), "null clears");
    }

    #[test]
    fn parse_change_refuses_bad_input() {
        for (body, code) in [
            (&br#"{"briefPolicy":"sometimes"}"#[..], "bad_value"),
            (&br#"{"assigneePolicy":true}"#[..], "bad_value"),
            (&br#"{"defaultAssignee":7}"#[..], "bad_value"),
            (&br#"{"brief":"off"}"#[..], "bad_request"),
            (&br#"{}"#[..], "bad_request"),
            (&br#"[]"#[..], "bad_request"),
            (&b"not json"[..], "bad_request"),
        ] {
            let r = refused(parse_change(body));
            assert_eq!(r.status, "400 Bad Request", "{}", r.body);
            assert_eq!(err_code(&r), code, "{}", String::from_utf8_lossy(body));
        }
        let r = refused(parse_change(br#"{"briefPolicy":"sometimes"}"#));
        assert!(r.body.contains("--brief must be off, warn or require"), "{}", r.body);
    }

    /// Refusals by caller run before any settings read or write.
    #[test]
    fn only_owner_or_admin_may_write() {
        let body = br#"{"briefPolicy":"off"}"#;
        let (r, outcome) = handle_post(body, &Writer::Agent);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert_eq!(err_code(&r), "owner_only");
        assert!(r.body.contains("Settings → Tickets"), "{}", r.body);
        assert_eq!(outcome, "refused:agent");
        let (r, _) = handle_post(body, &Writer::Member { username: "m".into() });
        assert_eq!(err_code(&r), "role_required", "{}", r.body);
        let (r, _) = handle_post(body, &Writer::Invalid);
        assert_eq!(err_code(&r), "forbidden", "{}", r.body);
        assert!(Writer::Owner.can_edit());
        assert!(Writer::Manager { username: "a".into() }.can_edit());
        assert!(!Writer::Member { username: "m".into() }.can_edit());
        assert!(!Writer::Agent.can_edit());
    }

    #[test]
    fn writer_from_tcp_owner_and_invalid() {
        assert_eq!(writer_from_tcp("token=own", None, "own"), Writer::Owner);
        assert_eq!(writer_from_tcp("", Some("own"), "own"), Writer::Owner);
        assert_eq!(writer_from_tcp("", None, "own"), Writer::Invalid);
        assert_eq!(writer_from_tcp("token=", None, "own"), Writer::Invalid);
    }
}
