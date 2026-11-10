//! The quiet feed (prd-org-chart-v1 §6.3, vs-live A3, B3): while the Org
//! Chart is on and an agent hasn't read today's bulletin, every successful
//! JSON-object answer it gets carries one more top-level field:
//!
//! ```json
//! "bulletin": {"day": "2026-11-10", "unread": true, "key": "3f2a…"}
//! ```
//!
//! The `k2` CLI turns it into one stderr line at exit, at most every 30
//! minutes per agent (`key` names the CLI's stamp file). It never carries
//! the bulletin's text. People (the owner token, logins, a K2 shell tab)
//! never get it, and nothing changes while the Org Chart is off: same bytes.
//!
//! Cost while on: one indexed lookup for an agent passport's success.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use k2_core::org_chart::{self as oc, store};

use crate::session_token::HookPrincipal;

fn status_ok(status: &str) -> bool {
    status.trim_start().starts_with('2')
}

/// The field's JSON for this agent, or `None` (read today, or a person).
/// Callers check that the Org Chart is on first.
fn field_for(session_id: &str, principal: &HookPrincipal) -> Option<String> {
    let caller = super::caller::from_passport(session_id, principal).ok()?;
    let super::caller::OrgCaller::Agent { project_id, agent_address, .. } = caller else {
        return None;
    };
    let day_key = super::routes::today(oc::now());
    let read = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::has_read(&conn, &project_id, &agent_address, &day_key).ok()?
    };
    if read {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (project_id.as_str(), agent_address.as_str()).hash(&mut h);
    Some(format!(r#""bulletin":{{"day":"{day_key}","unread":true,"key":"{:016x}"}}"#, h.finish()))
}

/// `body` with the field spliced in after its opening brace. Only a JSON
/// object that doesn't already carry a top-level `bulletin`.
fn splice(body: &str, field: &str) -> Option<String> {
    let t = body.trim_start();
    if !t.starts_with('{') || !body.trim_end().ends_with('}') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("bulletin").is_some() {
        return None;
    }
    let lead = body.len() - t.len();
    let rest = t[1..].trim_start();
    let sep = if rest.starts_with('}') { "" } else { "," };
    Some(format!("{}{{{field}{sep}{}", &body[..lead], &t[1..]))
}

/// Cell socket: the dispatch result, for this cell's passport.
pub(crate) fn stamp_cell(status: &str, body: &str, session_id: &str, principal: &HookPrincipal) -> Option<String> {
    if !oc::enabled() || !status_ok(status) {
        return None;
    }
    splice(body, &field_for(session_id, principal)?)
}

/// TCP: the response about to go out, for the request's passport (the
/// request `routes::pass_gate` noted; its query has the token folded in).
pub(crate) fn stamp_tcp<'a>(status: &str, body: &'a str) -> Cow<'a, str> {
    if !oc::enabled() || !status_ok(status) {
        return Cow::Borrowed(body);
    }
    let stamped = crate::routes::pass_gate::with_request(|r| {
        let token = crate::routes::http::extract_token(&r.query).filter(|t| !t.is_empty())?;
        let v = crate::session_token::validate_hook(token)?;
        let field = field_for(&v.session_id, &v.principal)?;
        splice(body, &field)
    })
    .flatten();
    match stamped {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed(body),
    }
}

#[cfg(test)]
mod tests {
    use super::splice;

    #[test]
    fn the_field_goes_first_in_an_object_and_nowhere_else() {
        let f = r#""bulletin":{"day":"d","unread":true,"key":"k"}"#;
        let out = splice(r#"{"ok":true}"#, f).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["bulletin"]["unread"], true);
        let out = splice("{}", f).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok(), "{out}");
        assert!(splice("[1,2]", f).is_none());
        assert!(splice("plain text", f).is_none());
        assert!(splice(r#"{"bulletin":{"day":"x"}}"#, f).is_none(), "never twice");
    }
}
