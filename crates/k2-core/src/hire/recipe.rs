//! The inline recipe (the client's code is the template; nothing is stored
//! on the box as a template). Every struct refuses unknown fields: a field
//! this daemon doesn't understand never half-applies.

use serde::Deserialize;

use super::HireError;

/// Hard caps (the key's own limits may be lower).
pub const MAX_MEMBERS: usize = 50;
pub const MAX_PERSONA_BYTES: usize = 64 * 1024;
pub const MAX_BRIEF_BYTES: usize = 16 * 1024;
pub const MAX_NOTE_BYTES: usize = 64 * 1024;
pub const MAX_NOTES: usize = 40;
pub const MAX_SKILLS: usize = 20;
pub const MAX_HEARTBEATS: usize = 12;
pub const MAX_LAYERS: usize = 20;
pub const MAX_LINKS: usize = 20;
pub const MAX_ON_BEHALF_OF: usize = 200;
pub const MAX_IDEM_KEY: usize = 128;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WikiNote {
    pub id: String,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InlineLayer {
    pub label: String,
    pub markdown: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillIn {
    pub name: String,
    /// Full SKILL.md text (frontmatter optional).
    pub body: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatIn {
    pub name: String,
    /// `hourly` | `daily` | `weekly` | `monthly` | `yearly` (the scheduler's
    /// frequencies; `k2 heartbeat schedule add` builds the same spec).
    pub frequency: String,
    /// Scheduler spec object (e.g. `{"time":"02:00"}`); validated by the
    /// heartbeat core.
    #[serde(default)]
    pub spec: Option<serde_json::Value>,
    #[serde(default)]
    pub instructions: Option<String>,
}

/// S5: an App login for a hired agent — an App grant on ONE published App
/// (never the box login), whole rooms (T17), marked as the agent.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppLoginIn {
    /// Published service id or `<workspace>/<publish name>`.
    pub app: String,
    /// Login name (default: the member's handle).
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Whole rooms (workspace handles/ids; `self` = this member). Default
    /// `["self"]`. Exclusive with `role`.
    #[serde(default)]
    pub rooms: Option<Vec<String>>,
    /// An App role (its rooms and caps). Exclusive with `rooms`.
    #[serde(default)]
    pub role: Option<String>,
    /// Set a random password and return it once in the response.
    #[serde(default)]
    pub password: Option<bool>,
}

/// One member of the recipe. `persona` is the ROLE.md body.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemberRecipe {
    /// Role key (`librarian`, `field`): a short slug. Default `agent`.
    #[serde(default)]
    pub role: Option<String>,
    /// The team lead (PoC of the team's project group). At most one.
    #[serde(default)]
    pub lead: Option<bool>,
    /// Handle seed. The folder and the handle take it (suffixed when taken;
    /// the plan returns the predicted handle).
    #[serde(default)]
    pub handle: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// `#rrggbb`.
    #[serde(default)]
    pub color: Option<String>,
    /// ROLE.md body. Exclusive with `template`.
    #[serde(default)]
    pub persona: Option<String>,
    /// One of today's four hire types: worker | manager | qa | researcher.
    #[serde(default)]
    pub template: Option<String>,
    /// Harness preset id (`claude`, `codex`, …).
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Seed the `.k2/wiki/` vault (default on).
    #[serde(default)]
    pub wiki: Option<bool>,
    #[serde(default)]
    pub wiki_notes: Option<Vec<WikiNote>>,
    /// Context catalog ids (`wiki:hygiene`).
    #[serde(default)]
    pub context: Option<Vec<String>>,
    #[serde(default)]
    pub layers: Option<Vec<InlineLayer>>,
    #[serde(default)]
    pub skills: Option<Vec<SkillIn>>,
    #[serde(default)]
    pub heartbeats: Option<Vec<HeartbeatIn>>,
    /// First brief, delivered after launch (implies `launch`).
    #[serde(default)]
    pub brief: Option<String>,
    /// Start the agent's chat session after the hire.
    #[serde(default)]
    pub launch: Option<bool>,
    /// `off` | `read` | `write` (capped by the key's DB door).
    #[serde(default)]
    pub db_access: Option<String>,
    /// App login for the agent (S5).
    #[serde(default)]
    pub app_login: Option<AppLoginIn>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamIn {
    /// Project-group name (unique on the box, case-insensitive).
    pub name: String,
    /// Internal messaging: `star` (lead ↔ each member, default), `mesh`
    /// (everyone ↔ everyone) or `none`.
    #[serde(default)]
    pub topology: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkIn {
    /// The workspace on the other side (handle, name or id).
    pub workspace: String,
    /// `serves` (this team serves that workspace) or `helper_of` (this
    /// team helps that workspace's agent).
    #[serde(default)]
    pub kind: Option<String>,
    /// Free JSON the two sides agree on (e.g. `{"planSets":["ps-4471"]}`).
    #[serde(default)]
    pub scope: Option<serde_json::Value>,
}

/// `POST /v1/agents/hire` body.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HireRequest {
    /// Idempotency key (the `Idempotency-Key` header wins when both are set).
    #[serde(default)]
    pub key: Option<String>,
    /// App-asserted, never verified, never used for a decision (≤ 200).
    #[serde(default)]
    pub on_behalf_of: Option<String>,
    /// One agent (shorthand for `members: [agent]`).
    #[serde(default)]
    pub agent: Option<MemberRecipe>,
    #[serde(default)]
    pub members: Option<Vec<MemberRecipe>>,
    /// Make the members a team (project group). Required for 2+ members.
    #[serde(default)]
    pub team: Option<TeamIn>,
    /// Team links to existing workspaces (managed connections).
    #[serde(default)]
    pub link_to: Option<Vec<LinkIn>>,
    /// Plan only: validate, check the key, predict handles and paths;
    /// write nothing.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

impl HireRequest {
    /// The members, whichever spelling the caller used.
    pub fn member_list(&self) -> Result<Vec<MemberRecipe>, HireError> {
        match (&self.agent, &self.members) {
            (Some(_), Some(_)) => Err(HireError::usage("pass `agent` or `members`, not both")),
            (Some(a), None) => Ok(vec![a.clone()]),
            (None, Some(m)) => Ok(m.clone()),
            (None, None) => Err(HireError::usage("missing `agent` (one agent) or `members` (a team)")),
        }
    }
}

pub const TEMPLATES: &[&str] = &["worker", "manager", "qa", "researcher"];
pub const DB_ACCESS: &[&str] = &["off", "read", "write"];
pub const FREQUENCIES: &[&str] = &["hourly", "daily", "weekly", "monthly", "yearly"];

/// `off` < `read` < `write`.
pub fn db_rank(mode: &str) -> u8 {
    match mode {
        "read" => 1,
        "write" => 2,
        _ => 0,
    }
}

fn role_ok(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 40
        && role.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn color_ok(c: &str) -> bool {
    let c = c.trim();
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
}

fn skill_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validate one member's fields (shape and sizes; authority is checked by
/// the plan against the caller).
pub fn validate_member(m: &MemberRecipe, idx: usize) -> Result<(), HireError> {
    let at = |field: &str| format!("members[{idx}].{field}");
    if let Some(r) = m.role.as_deref() {
        if !role_ok(r.trim()) {
            return Err(HireError::usage(format!("{}: a short slug (letters, digits, - _; ≤ 40)", at("role"))));
        }
    }
    if let Some(h) = m.handle.as_deref() {
        crate::workspace_session_handles::slugify_address_token(h)
            .map_err(|e| HireError::usage(format!("{}: {e}", at("handle"))))?;
        if h.trim().len() > 60 {
            return Err(HireError::usage(format!("{}: at most 60 characters", at("handle"))));
        }
    }
    if let Some(n) = m.name.as_deref() {
        if n.trim().is_empty() || n.chars().count() > 80 || n.chars().any(|c| c.is_control()) {
            return Err(HireError::usage(format!("{}: 1–80 characters, no control characters", at("name"))));
        }
    }
    if let Some(c) = m.color.as_deref() {
        if !color_ok(c) {
            return Err(HireError::usage(format!("{}: a hex color like #4f46e5", at("color"))));
        }
    }
    if m.persona.is_some() && m.template.is_some() {
        return Err(HireError::usage(format!("{}: pass persona or template, not both", at("persona"))));
    }
    if let Some(p) = m.persona.as_deref() {
        if p.len() > MAX_PERSONA_BYTES {
            return Err(HireError::usage(format!("{}: at most {MAX_PERSONA_BYTES} bytes", at("persona"))));
        }
    }
    if let Some(t) = m.template.as_deref() {
        if !TEMPLATES.contains(&t.trim()) {
            return Err(HireError::usage(format!("{}: one of {}", at("template"), TEMPLATES.join(", "))));
        }
    }
    for (field, v) in [("preset", &m.preset), ("model", &m.model)] {
        if let Some(s) = v.as_deref() {
            if s.trim().is_empty() || s.len() > 120 || s.chars().any(|c| c.is_control()) {
                return Err(HireError::usage(format!("{}: 1–120 characters", at(field))));
            }
        }
    }
    if let Some(notes) = m.wiki_notes.as_ref() {
        if notes.len() > MAX_NOTES {
            return Err(HireError::usage(format!("{}: at most {MAX_NOTES} notes", at("wikiNotes"))));
        }
        for n in notes {
            if n.body.len() > MAX_NOTE_BYTES {
                return Err(HireError::usage(format!("{}: note '{}' is over {MAX_NOTE_BYTES} bytes", at("wikiNotes"), n.id)));
            }
            crate::wiki::validate_note_id(&n.id)
                .map_err(|e| HireError::usage(format!("{}: {e}", at("wikiNotes"))))?;
        }
    }
    if let Some(ctx) = m.context.as_ref() {
        if ctx.len() > MAX_LAYERS || ctx.iter().any(|c| c.trim().is_empty()) {
            return Err(HireError::usage(format!("{}: up to {MAX_LAYERS} non-empty catalog ids", at("context"))));
        }
    }
    if let Some(layers) = m.layers.as_ref() {
        if layers.len() > MAX_LAYERS {
            return Err(HireError::usage(format!("{}: at most {MAX_LAYERS}", at("layers"))));
        }
        for l in layers {
            super::steps::sanitize_layer_label(&l.label).map_err(|e| HireError::usage(format!("{}: {e}", at("layers"))))?;
            if l.markdown.len() > MAX_NOTE_BYTES {
                return Err(HireError::usage(format!("{}: layer '{}' is too large", at("layers"), l.label)));
            }
        }
    }
    if let Some(skills) = m.skills.as_ref() {
        if skills.len() > MAX_SKILLS {
            return Err(HireError::usage(format!("{}: at most {MAX_SKILLS}", at("skills"))));
        }
        let mut seen = std::collections::HashSet::new();
        for sk in skills {
            if !skill_name_ok(&sk.name) || !seen.insert(sk.name.to_ascii_lowercase()) {
                return Err(HireError::usage(format!("{}: '{}' must be a unique slug", at("skills"), sk.name)));
            }
            if sk.body.len() > MAX_NOTE_BYTES {
                return Err(HireError::usage(format!("{}: '{}' is too large", at("skills"), sk.name)));
            }
        }
    }
    if let Some(hbs) = m.heartbeats.as_ref() {
        if hbs.len() > MAX_HEARTBEATS {
            return Err(HireError::usage(format!("{}: at most {MAX_HEARTBEATS}", at("heartbeats"))));
        }
        let mut seen = std::collections::HashSet::new();
        for hb in hbs {
            crate::db::schema::AgentHeartbeat::validate_name(&hb.name)
                .map_err(|e| HireError::usage(format!("{}: {e}", at("heartbeats"))))?;
            if !seen.insert(hb.name.clone()) {
                return Err(HireError::usage(format!("{}: '{}' twice", at("heartbeats"), hb.name)));
            }
            if !FREQUENCIES.contains(&hb.frequency.as_str()) {
                return Err(HireError::usage(format!("{}: frequency is one of {}", at("heartbeats"), FREQUENCIES.join(", "))));
            }
            let spec = heartbeat_spec_json(hb);
            crate::heartbeats::validate_spec_json(&hb.frequency, &spec)
                .map_err(|e| HireError::usage(format!("{}: '{}': {e}", at("heartbeats"), hb.name)))?;
            if hb.instructions.as_deref().map(str::len).unwrap_or(0) > MAX_BRIEF_BYTES {
                return Err(HireError::usage(format!("{}: instructions too large", at("heartbeats"))));
            }
        }
    }
    if let Some(b) = m.brief.as_deref() {
        if b.trim().is_empty() || b.len() > MAX_BRIEF_BYTES {
            return Err(HireError::usage(format!("{}: 1–{MAX_BRIEF_BYTES} bytes", at("brief"))));
        }
    }
    if let Some(d) = m.db_access.as_deref() {
        if !DB_ACCESS.contains(&d.trim()) {
            return Err(HireError::usage(format!("{}: off, read or write", at("dbAccess"))));
        }
    }
    if let Some(a) = m.app_login.as_ref() {
        if a.app.trim().is_empty() {
            return Err(HireError::usage(format!("{}: missing app", at("appLogin"))));
        }
        if let Some(u) = a.username.as_deref() {
            crate::skin::normalize_username(u).map_err(|e| HireError::usage(format!("{}: {e}", at("appLogin.username"))))?;
        }
        if let Some(e) = a.email.as_deref() {
            crate::skin::normalize_email(e).map_err(|e| HireError::usage(format!("{}: {e}", at("appLogin.email"))))?;
        }
        if a.rooms.is_some() && a.role.is_some() {
            return Err(HireError::usage(format!("{}: rooms or role, not both", at("appLogin"))));
        }
        if a.rooms.as_ref().is_some_and(|r| r.is_empty() || r.len() > 20) {
            return Err(HireError::usage(format!("{}: 1–20 rooms", at("appLogin.rooms"))));
        }
    }
    Ok(())
}

/// The scheduler spec string for a heartbeat (frequency folded in).
pub fn heartbeat_spec_json(hb: &HeartbeatIn) -> String {
    let mut obj = match hb.spec.as_ref() {
        Some(serde_json::Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    obj.insert("frequency".into(), serde_json::Value::String(hb.frequency.clone()));
    serde_json::Value::Object(obj).to_string()
}

/// Validate the whole request (shape only).
pub fn validate_request(req: &HireRequest) -> Result<Vec<MemberRecipe>, HireError> {
    let members = req.member_list()?;
    if members.is_empty() {
        return Err(HireError::usage("`members` is empty"));
    }
    if members.len() > MAX_MEMBERS {
        return Err(HireError::usage(format!("at most {MAX_MEMBERS} members per hire")));
    }
    for (i, m) in members.iter().enumerate() {
        validate_member(m, i)?;
    }
    let leads = members.iter().filter(|m| m.lead.unwrap_or(false)).count();
    if leads > 1 {
        return Err(HireError::usage("at most one member may be the lead"));
    }
    if members.len() > 1 && req.team.is_none() {
        return Err(HireError::usage("two or more members make a team: pass `team: {name}`"));
    }
    if let Some(t) = req.team.as_ref() {
        let n = t.name.trim();
        if n.is_empty() || n.chars().count() > 80 || n.chars().any(|c| c.is_control()) {
            return Err(HireError::usage("team.name: 1–80 characters"));
        }
        if let Some(top) = t.topology.as_deref() {
            if !["star", "mesh", "none"].contains(&top) {
                return Err(HireError::usage("team.topology: star, mesh or none"));
            }
        }
    }
    if let Some(o) = req.on_behalf_of.as_deref() {
        if o.chars().count() > MAX_ON_BEHALF_OF || o.chars().any(|c| c.is_control()) {
            return Err(HireError::usage(format!("onBehalfOf: at most {MAX_ON_BEHALF_OF} characters, one line")));
        }
    }
    if let Some(k) = req.key.as_deref() {
        validate_idem_key(k)?;
    }
    if let Some(links) = req.link_to.as_ref() {
        if links.len() > MAX_LINKS {
            return Err(HireError::usage(format!("linkTo: at most {MAX_LINKS}")));
        }
        for l in links {
            if l.workspace.trim().is_empty() {
                return Err(HireError::usage("linkTo[].workspace is empty"));
            }
            super::links::parse_kind(l.kind.as_deref())?;
        }
    }
    Ok(members)
}

/// Idempotency keys: 1–128 printable ASCII, no spaces.
pub fn validate_idem_key(k: &str) -> Result<(), HireError> {
    let k = k.trim();
    if k.is_empty() || k.len() > MAX_IDEM_KEY || !k.chars().all(|c| c.is_ascii_graphic()) {
        return Err(HireError::usage(format!("idempotency key: 1–{MAX_IDEM_KEY} printable characters, no spaces")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(v: serde_json::Value) -> Result<HireRequest, serde_json::Error> {
        serde_json::from_value(v)
    }

    #[test]
    fn unknown_fields_are_refused_everywhere() {
        assert!(req(serde_json::json!({"agent": {"role": "x"}, "template": "plans-desk"})).is_err());
        assert!(req(serde_json::json!({"agent": {"role": "x", "mailSend": true}})).is_err());
        assert!(req(serde_json::json!({"agent": {"wikiNotes": [{"id": "a.md", "body": "b", "x": 1}]}})).is_err());
        assert!(req(serde_json::json!({"agent": {"role": "x"}})).is_ok());
    }

    #[test]
    fn team_rules() {
        let two = req(serde_json::json!({"members": [{"role": "a"}, {"role": "b"}]})).unwrap();
        assert_eq!(validate_request(&two).unwrap_err().code, "usage");
        let two_leads = req(serde_json::json!({"team": {"name": "T"}, "members": [{"role": "a", "lead": true}, {"role": "b", "lead": true}]})).unwrap();
        assert!(validate_request(&two_leads).unwrap_err().hint.contains("lead"));
        let ok = req(serde_json::json!({"team": {"name": "T"}, "members": [{"role": "a", "lead": true}, {"role": "b"}]})).unwrap();
        assert_eq!(validate_request(&ok).unwrap().len(), 2);
        let both = req(serde_json::json!({"agent": {}, "members": [{}]})).unwrap();
        assert!(validate_request(&both).is_err());
    }

    #[test]
    fn member_field_rules() {
        let bad = |v: serde_json::Value| {
            let r = req(serde_json::json!({ "agent": v })).unwrap();
            validate_request(&r).unwrap_err()
        };
        assert_eq!(bad(serde_json::json!({"role": "has space"})).code, "usage");
        assert_eq!(bad(serde_json::json!({"handle": "a/b"})).code, "usage");
        assert_eq!(bad(serde_json::json!({"color": "red"})).code, "usage");
        assert_eq!(bad(serde_json::json!({"persona": "x", "template": "worker"})).code, "usage");
        assert_eq!(bad(serde_json::json!({"template": "boss"})).code, "usage");
        assert_eq!(bad(serde_json::json!({"dbAccess": "admin"})).code, "usage");
        assert_eq!(bad(serde_json::json!({"wikiNotes": [{"id": "../x.md", "body": "b"}]})).code, "usage");
        assert_eq!(bad(serde_json::json!({"skills": [{"name": "a b", "body": "x"}]})).code, "usage");
        assert_eq!(bad(serde_json::json!({"heartbeats": [{"name": "n", "frequency": "secondly"}]})).code, "usage");
        assert_eq!(bad(serde_json::json!({"appLogin": {"app": "a", "rooms": ["self"], "role": "r"}})).code, "usage");
        assert_eq!(bad(serde_json::json!({"appLogin": {"app": "a", "username": "Bad Name!"}})).code, "usage");
        assert_eq!(bad(serde_json::json!({"appLogin": {"app": "a", "rooms": []}})).code, "usage");
        // Folder lines aren't built (T17): a `folders` field is unknown → refused.
        assert!(req(serde_json::json!({"agent": {"appLogin": {"app": "a", "folders": ["plans/"]}}})).is_err());
        assert_eq!(bad(serde_json::json!({"brief": "  "})).code, "usage");
    }

    #[test]
    fn idem_key_rules() {
        assert!(validate_idem_key("job-J1-desk").is_ok());
        assert!(validate_idem_key("has space").is_err());
        assert!(validate_idem_key(&"x".repeat(200)).is_err());
    }
}
