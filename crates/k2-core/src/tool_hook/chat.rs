//! The `chat` provisioner (Sew S27 §6, §10–§14).
//!
//! A `chat` grant means one thing: "this person or agent may use chat". It
//! creates the Sew account, and removing it removes the account. It never
//! sets Sew roles, Spaces or channels.
//!
//! Scope grammar (stored canonical):
//!
//! | Subject | Scope | Sew member |
//! |---|---|---|
//! | principal | `member` | person |
//! | principal | `guest` | person, `guest: true` |
//! | workspace | `coworker` | agent, `agent_kind: coworker` |
//! | workspace | `assistant:<principal-id>` | agent, assistant of `k2:principal:<id>` |
//! | workspace | `assistant:owner` | agent, assistant of `k2:owner` |
//!
//! Subjects are skin principals and workspaces only. Connect users (any
//! role) never get a Sew account from this module (rev 147).

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::sew_wire::{
    self, AgentKind, DeprovisionRequest, ExternalId, MemberKind, MgmtPrincipal, OwnerSetRequest,
    Patch, ProfileInput, ProfilePatch, ProvisionRequest, UpdatePrincipalRequest,
};
use super::{
    grant_rev, outbox, Cause, HookCtx, HookEvent, NewOp, Op, ToolProvisioner, CHAT_KIND,
    CHAT_NOT_ENABLED, CHAT_TARGET_ID, OWNER_GRANT_REV,
};
use crate::skin::{SkinGrant, SkinGrantWrite};

pub const SCOPE_ERR: &str =
    "chat scope must be member|guest (principal) or coworker|assistant:<person> (workspace)";
pub const KIND_FLIP_ERR: &str = "delete the grant and add it again to change agent kind";
const DISPLAY_NAME_MAX: usize = 100;

/// A parsed, canonical chat scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatScope {
    Member,
    Guest,
    Coworker,
    /// Holds the person's `principals.id`.
    AssistantOf(String),
    AssistantOwner,
}

impl ChatScope {
    pub fn as_stored(&self) -> String {
        match self {
            ChatScope::Member => "member".into(),
            ChatScope::Guest => "guest".into(),
            ChatScope::Coworker => "coworker".into(),
            ChatScope::AssistantOf(id) => format!("assistant:{id}"),
            ChatScope::AssistantOwner => "assistant:owner".into(),
        }
    }

    /// Parse a STORED scope (canonical form only).
    pub fn parse_stored(raw: &str) -> Result<ChatScope, String> {
        match raw {
            "member" => Ok(ChatScope::Member),
            "guest" => Ok(ChatScope::Guest),
            "coworker" => Ok(ChatScope::Coworker),
            "assistant:owner" => Ok(ChatScope::AssistantOwner),
            s => match s.strip_prefix("assistant:") {
                Some(id) if sew_wire::is_lower_uuid(id) => Ok(ChatScope::AssistantOf(id.into())),
                _ => Err(format!("stored chat scope '{raw}' is not canonical")),
            },
        }
    }

    pub fn for_principal(&self) -> bool {
        matches!(self, ChatScope::Member | ChatScope::Guest)
    }

    fn is_assistant(&self) -> bool {
        matches!(self, ChatScope::AssistantOf(_) | ChatScope::AssistantOwner)
    }

    /// The person an assistant is pinned to.
    pub fn assistant_person(&self) -> Option<ExternalId> {
        match self {
            ChatScope::AssistantOf(id) => Some(ExternalId::K2Principal(id.clone())),
            ChatScope::AssistantOwner => Some(ExternalId::K2Owner),
            _ => None,
        }
    }
}

/// A principal row as the hook needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalFacts {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub full_name: Option<String>,
}

fn principal_where(
    conn: &Connection,
    sql_where: &str,
    v: &str,
) -> Result<Option<PrincipalFacts>, String> {
    conn.query_row(
        &format!("SELECT id, username, email, full_name FROM principals WHERE {sql_where}"),
        params![v],
        |r| {
            Ok(PrincipalFacts {
                id: r.get(0)?,
                username: r.get(1)?,
                email: r
                    .get::<_, Option<String>>(2)?
                    .filter(|s| !s.trim().is_empty()),
                full_name: r
                    .get::<_, Option<String>>(3)?
                    .filter(|s| !s.trim().is_empty()),
            })
        },
    )
    .optional()
    .map_err(|e| format!("chat principal lookup: {e}"))
}

pub fn principal_by_id(conn: &Connection, id: &str) -> Result<Option<PrincipalFacts>, String> {
    principal_where(conn, "id = ?1", id)
}

/// A principal id, else a username (case-insensitive like the rest of skin).
pub fn principal_by_token(
    conn: &Connection,
    token: &str,
) -> Result<Option<PrincipalFacts>, String> {
    let t = token.trim();
    if let Some(p) = principal_by_id(conn, t)? {
        return Ok(Some(p));
    }
    principal_where(conn, "username = ?1", &t.to_ascii_lowercase())
}

/// Parse an INPUT scope for a subject kind. `assistant:<username|id|owner>`
/// is resolved to the stored `assistant:<principal-id>` form.
pub fn parse_input_scope(
    conn: &Connection,
    subject_kind: &str,
    raw: Option<&str>,
) -> Result<ChatScope, String> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(raw) = raw else {
        return match subject_kind {
            "principal" => Ok(ChatScope::Member),
            _ => {
                Err("chat scope is required for a workspace: coworker or assistant:<person>".into())
            }
        };
    };
    let (kw, rest) = match raw.split_once(':') {
        Some((k, r)) => (k.trim().to_ascii_lowercase(), Some(r.trim())),
        None => (raw.to_ascii_lowercase(), None),
    };
    match (subject_kind, kw.as_str(), rest) {
        ("principal", "member", None) => Ok(ChatScope::Member),
        ("principal", "guest", None) => Ok(ChatScope::Guest),
        ("workspace", "coworker", None) => Ok(ChatScope::Coworker),
        ("workspace", "assistant", Some(person)) if !person.is_empty() => {
            if person.eq_ignore_ascii_case("owner") {
                return Ok(ChatScope::AssistantOwner);
            }
            match principal_by_token(conn, person)? {
                Some(p) => Ok(ChatScope::AssistantOf(p.id)),
                None => Err(format!("unknown assistant person '{person}'")),
            }
        }
        _ => Err(SCOPE_ERR.into()),
    }
}

/// `k2:principal:<id>` / `k2:workspace:<id>`; refuses non-uuid ids (§6.3.3).
pub fn external_id(subject_kind: &str, subject_id: &str) -> Result<ExternalId, String> {
    if !sew_wire::is_lower_uuid(subject_id) {
        return Err(format!("id '{subject_id}' is not a UUID; chat needs one"));
    }
    match subject_kind {
        "principal" => Ok(ExternalId::K2Principal(subject_id.to_string())),
        "workspace" => Ok(ExternalId::K2Workspace(subject_id.to_string())),
        other => Err(format!(
            "chat subjects are principals or workspaces, not '{other}'"
        )),
    }
}

/// §6.3.4: the name must fit Sew's handle grammar.
pub fn check_username(name: &str) -> Result<(), String> {
    if name.len() > 64 {
        return Err(format!(
            "username '{name}' is longer than 64 characters; chat cannot use it"
        ));
    }
    if !sew_wire::is_valid_handle(name) {
        return Err(format!(
            "username '{name}' must be 2–64 characters of a-z, 0-9, _ or -; chat cannot use it"
        ));
    }
    if RESERVED_USERNAMES.contains(&name) {
        return Err(format!(
            "username '{name}' is a reserved chat mention (@{name}); chat cannot use it"
        ));
    }
    Ok(())
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Sew's reserved mention words (`@here` etc.): never a username.
pub const RESERVED_USERNAMES: [&str; 4] = ["here", "channel", "everyone", "all"];

/// Moves a login off a reserved word (`all` → `all-k2`).
const RESERVED_SUFFIX: &str = "-k2";

/// A K2 login as a Sew username (Sew's rules for K2, 2026-10-17):
/// lowercase ASCII, `.` → `-`, anything else outside `[a-z0-9_-]`
/// stripped, at most 64 chars; a reserved word (`here`, `channel`,
/// `everyone`, `all`) gets `-k2`. `None` when fewer than 2 chars are left.
pub fn login_username(login: &str) -> Option<String> {
    let norm: String = login
        .trim()
        .chars()
        .filter_map(|c| {
            let c = c.to_ascii_lowercase();
            if c == '.' {
                Some('-')
            } else if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                Some(c)
            } else {
                None
            }
        })
        .take(64)
        .collect();
    if norm.len() < 2 {
        return None;
    }
    if RESERVED_USERNAMES.contains(&norm.as_str()) {
        return Some(format!("{norm}{RESERVED_SUFFIX}"));
    }
    Some(norm)
}

/// Owner handle: the owner's linked K2 login ([`login_username`]), else
/// `owner` (no linked account, or nothing usable left of the login). The
/// display name never feeds the handle (0.46.0: the username is the
/// identity, "First Last" is only the display name).
pub fn owner_username(login: Option<&str>) -> String {
    login
        .and_then(login_username)
        .unwrap_or_else(|| "owner".to_string())
}

pub fn owner_profile_display_name(display: Option<&str>) -> String {
    match display.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => truncate_chars(d, DISPLAY_NAME_MAX),
        None => "Owner".to_string(),
    }
}

/// `<base>-<hhhh>` (first 4 hex of SHA-256(external id)), ≤ 64 chars.
/// Deterministic, so replays and reconcile converge (§10.4).
pub fn suffixed_username(base: &str, ext: &str) -> String {
    let h = Sha256::digest(ext.as_bytes());
    let hex = format!("{:02x}{:02x}", h[0], h[1]);
    let keep: String = base.chars().take(64 - 5).collect();
    format!("{keep}-{hex}")
}

/// Sew's handle is right when it is K2's username or its deterministic
/// conflict fallback ([`suffixed_username`]). So a fallback sticks until
/// K2's username itself changes (Sew, 2026-10-17: each rename is an audit
/// row and a member event).
pub fn username_matches(want: &str, have: &str, ext: &ExternalId) -> bool {
    have == want || have == suffixed_username(want, &ext.to_string())
}

fn chat_grants_of(conn: &Connection, sql_where: &str, v: &str) -> Result<Vec<SkinGrant>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{} WHERE kind = '{CHAT_KIND}' AND {sql_where} ORDER BY created_at, id",
            crate::skin::GRANT_SELECT
        ))
        .map_err(|e| format!("chat grant lookup: {e}"))?;
    let rows = stmt
        .query_map(params![v], crate::skin::map_grant_row)
        .map_err(|e| format!("chat grant lookup: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("chat grant row: {e}"))?);
    }
    Ok(out)
}

/// The principal's chat grant, if any.
pub fn person_grant(conn: &Connection, principal_id: &str) -> Result<Option<SkinGrant>, String> {
    Ok(chat_grants_of(
        conn,
        "subject_kind = 'principal' AND subject_id = ?1",
        principal_id,
    )?
    .into_iter()
    .next())
}

/// Workspace chat grants pinned to this principal (`assistant:<id>`).
pub fn assistants_of(conn: &Connection, principal_id: &str) -> Result<Vec<SkinGrant>, String> {
    chat_grants_of(
        conn,
        "subject_kind = 'workspace' AND scope = ?1",
        &format!("assistant:{principal_id}"),
    )
}

fn assistant_refusal(conn: &Connection, principal_id: &str) -> Result<(), String> {
    if assistants_of(conn, principal_id)?.is_empty() {
        return Ok(());
    }
    let name = principal_by_id(conn, principal_id)?
        .map(|p| p.username)
        .unwrap_or_else(|| principal_id.to_string());
    Err(format!("remove the assistant grants for '{name}' first"))
}

/// §6.3.6: the assistant's person holds an enabled `member` chat grant,
/// or is the owner.
fn check_assistant_person(conn: &Connection, scope: &ChatScope) -> Result<(), String> {
    let ChatScope::AssistantOf(pid) = scope else {
        return Ok(());
    };
    let name = principal_by_id(conn, pid)?
        .map(|p| p.username)
        .unwrap_or_else(|| pid.clone());
    match person_grant(conn, pid)? {
        Some(g) if g.enabled && g.scope.as_deref() == Some("member") => Ok(()),
        Some(g) if g.scope.as_deref() == Some("guest") => Err(format!(
            "assistant person '{name}' is a chat guest; guests cannot have assistants"
        )),
        _ => Err(format!(
            "assistant person '{name}' needs a member chat grant first"
        )),
    }
}

/// The `ProvisionRequest` for a chat grant's current state.
pub fn provision_request(
    conn: &Connection,
    grant: &SkinGrant,
    ctx: &HookCtx,
) -> Result<(ProvisionRequest, Option<String>), String> {
    let scope = ChatScope::parse_stored(grant.scope.as_deref().unwrap_or(""))?;
    let ext = external_id(&grant.subject_kind, &grant.subject_id)?;
    let req = match grant.subject_kind.as_str() {
        "principal" => {
            let p = principal_by_id(conn, &grant.subject_id)?
                .ok_or_else(|| format!("unknown principal id '{}'", grant.subject_id))?;
            check_username(&p.username)?;
            ProvisionRequest {
                external_id: ext,
                kind: MemberKind::Person,
                agent_kind: None,
                assistant_of: None,
                guest: scope == ChatScope::Guest,
                profile: ProfileInput {
                    display_name: truncate_chars(
                        p.full_name.as_deref().unwrap_or(&p.username),
                        DISPLAY_NAME_MAX,
                    ),
                    username: p.username,
                    email: p.email,
                    title: None,
                    pronouns: None,
                },
            }
        }
        _ => {
            let w = ctx
                .workspaces
                .get(&grant.subject_id)
                .ok_or_else(|| format!("unknown workspace '{}'", grant.subject_id))?;
            let handle = w
                .handle
                .clone()
                .ok_or_else(|| format!("workspace '{}' has no handle; chat needs one", w.id))?;
            check_username(&handle)?;
            let display = [w.display_name.as_str(), w.name.as_str(), handle.as_str()]
                .into_iter()
                .map(str::trim)
                .find(|s| !s.is_empty())
                .unwrap_or("agent")
                .to_string();
            ProvisionRequest {
                external_id: ext,
                kind: MemberKind::Agent,
                agent_kind: Some(if scope.is_assistant() {
                    AgentKind::Assistant
                } else {
                    AgentKind::Coworker
                }),
                assistant_of: scope.assistant_person(),
                guest: false,
                profile: ProfileInput {
                    display_name: truncate_chars(&display, DISPLAY_NAME_MAX),
                    username: handle,
                    email: None,
                    title: None,
                    pronouns: None,
                },
            }
        }
    };
    sew_wire::validate_provision(&req)?;
    let after = req.assistant_of.as_ref().map(ExternalId::to_string);
    Ok((req, after))
}

pub fn provision_op(
    conn: &Connection,
    grant: &SkinGrant,
    ctx: &HookCtx,
    cause: Cause,
) -> Result<NewOp, String> {
    let (req, after) = provision_request(conn, grant, ctx)?;
    Ok(NewOp {
        tool: CHAT_KIND,
        op: Op::Provision,
        subject_key: req.external_id.to_string(),
        payload: serde_json::to_value(&req).map_err(|e| format!("chat payload: {e}"))?,
        grant_id: Some(grant.id.clone()),
        grant_rev: Some(grant_rev(grant)),
        cause,
        after_subject: after,
    })
}

pub fn deprovision_op(grant: &SkinGrant, cause: Cause) -> Result<NewOp, String> {
    let ext = external_id(&grant.subject_kind, &grant.subject_id)?;
    Ok(NewOp {
        tool: CHAT_KIND,
        op: Op::Deprovision,
        subject_key: ext.to_string(),
        payload: serde_json::to_value(DeprovisionRequest { external_id: ext })
            .map_err(|e| format!("chat payload: {e}"))?,
        grant_id: Some(grant.id.clone()),
        grant_rev: Some(grant_rev(grant)),
        cause,
        after_subject: None,
    })
}

/// The owner's provision body (person, never a guest).
/// A8 (Sew's answer, 2026-10-15): `principals/provision` is declarative —
/// an absent optional profile field CLEARS it. Before a provision reaches
/// an EXISTING member (guest flip, assistant re-pin, reconcile repair,
/// owner re-provision), K2 copies back the fields it does not own from
/// Sew's member list (`MgmtPrincipal`, SW14):
/// - `title`, `pronouns`: always Sew-local;
/// - `email`: only for agents and `k2:owner` (a principal's email is K2's,
///   `principals.email`, so K2's value — set or absent — wins).
///
/// Fields the list does not carry (`None`: an older Sew) are unknown and
/// left absent; nothing K2 can do there until SW14 ships.
///
/// The username is sticky: when the member already holds the conflict
/// fallback of K2's username ([`username_matches`]), the body keeps it, so
/// a re-provision never flips the handle back to the bare login.
///
/// Returns whether the body changed. Pure.
pub fn fill_provision_from_member(req: &mut ProvisionRequest, m: &MgmtPrincipal) -> bool {
    let mut changed = false;
    if m.username != req.profile.username
        && username_matches(&req.profile.username, &m.username, &req.external_id)
    {
        req.profile.username = m.username.clone();
        changed = true;
    }
    let mut take = |slot: &mut Option<String>, have: &Option<Option<String>>| {
        if slot.is_none() {
            if let Some(Some(v)) = have {
                *slot = Some(v.clone());
                changed = true;
            }
        }
    };
    take(&mut req.profile.title, &m.title);
    take(&mut req.profile.pronouns, &m.pronouns);
    if !matches!(req.external_id, ExternalId::K2Principal(_)) {
        take(&mut req.profile.email, &m.email);
    }
    changed
}

pub fn owner_provision_request(ctx: &HookCtx) -> ProvisionRequest {
    ProvisionRequest {
        external_id: ExternalId::K2Owner,
        kind: MemberKind::Person,
        agent_kind: None,
        assistant_of: None,
        guest: false,
        profile: ProfileInput {
            display_name: owner_profile_display_name(ctx.owner_name.as_deref()),
            username: owner_username(ctx.owner_login.as_deref()),
            email: None,
            title: None,
            pronouns: None,
        },
    }
}

pub fn owner_ops(ctx: &HookCtx, cause: Cause) -> Result<Vec<NewOp>, String> {
    let req = owner_provision_request(ctx);
    let set = OwnerSetRequest {
        external_id: ExternalId::K2Owner,
        approval: None,
        force: false,
    };
    let subject = ExternalId::K2Owner.to_string();
    Ok(vec![
        NewOp {
            tool: CHAT_KIND,
            op: Op::Provision,
            subject_key: subject.clone(),
            payload: serde_json::to_value(&req).map_err(|e| format!("chat payload: {e}"))?,
            grant_id: None,
            grant_rev: Some(OWNER_GRANT_REV.into()),
            cause,
            after_subject: None,
        },
        NewOp {
            tool: CHAT_KIND,
            op: Op::OwnerSet,
            subject_key: subject,
            payload: serde_json::to_value(&set).map_err(|e| format!("chat payload: {e}"))?,
            grant_id: None,
            grant_rev: Some(OWNER_GRANT_REV.into()),
            cause,
            after_subject: None,
        },
    ])
}

/// A profile `update` (display name / username / email).
pub fn update_op(
    ext: &ExternalId,
    patch: ProfilePatch,
    grant: Option<&SkinGrant>,
    cause: Cause,
) -> Result<NewOp, String> {
    let req = UpdatePrincipalRequest {
        external_id: ext.clone(),
        profile: patch,
    };
    Ok(NewOp {
        tool: CHAT_KIND,
        op: Op::Update,
        subject_key: ext.to_string(),
        payload: serde_json::to_value(&req).map_err(|e| format!("chat payload: {e}"))?,
        grant_id: grant.map(|g| g.id.clone()),
        grant_rev: grant
            .map(grant_rev)
            .or_else(|| matches!(ext, ExternalId::K2Owner).then(|| OWNER_GRANT_REV.to_string())),
        cause,
        after_subject: None,
    })
}

pub struct ChatProvisioner;

impl ToolProvisioner for ChatProvisioner {
    fn kind(&self) -> &'static str {
        CHAT_KIND
    }

    fn default_target(&self) -> &'static str {
        CHAT_TARGET_ID
    }

    fn validate(
        &self,
        conn: &Connection,
        w: &SkinGrantWrite,
        ctx: &HookCtx,
    ) -> Result<SkinGrantWrite, String> {
        let target = w.target_id.trim();
        if !target.is_empty() && !target.eq_ignore_ascii_case(CHAT_TARGET_ID) {
            return Err(format!("chat target must be '{CHAT_TARGET_ID}'"));
        }
        if !outbox::target_enabled(conn, CHAT_KIND)? {
            return Err(CHAT_NOT_ENABLED.to_string());
        }
        if w.role
            .as_deref()
            .map(str::trim)
            .is_some_and(|r| !r.is_empty())
        {
            return Err("roleId is only set for kind=app".to_string());
        }
        let subject_kind = w.subject_kind.trim().to_ascii_lowercase();
        let token = w.subject_id.trim();
        let (subject_id, username) = match subject_kind.as_str() {
            "principal" => {
                let p = principal_by_token(conn, token)?
                    .ok_or_else(|| format!("unknown principal id '{token}'"))?;
                (p.id, p.username)
            }
            "workspace" => {
                let f = ctx
                    .workspaces
                    .get(token)
                    .ok_or_else(|| format!("unknown workspace '{token}'"))?;
                let handle = f
                    .handle
                    .clone()
                    .ok_or_else(|| format!("workspace '{token}' has no handle; chat needs one"))?;
                (f.id.clone(), handle)
            }
            other => {
                return Err(format!(
                    "chat subjects are principals or workspaces, not '{other}' (Connect users never get chat from a grant)"
                ))
            }
        };
        external_id(&subject_kind, &subject_id)?;
        check_username(&username)?;
        let scope = parse_input_scope(conn, &subject_kind, w.scope.as_deref())?;
        if scope.for_principal() != (subject_kind == "principal") {
            return Err(SCOPE_ERR.into());
        }
        check_assistant_person(conn, &scope)?;
        Ok(SkinGrantWrite {
            subject_kind,
            subject_id,
            kind: CHAT_KIND.to_string(),
            target_id: CHAT_TARGET_ID.to_string(),
            role: None,
            scope: Some(scope.as_stored()),
            created_by: w.created_by.clone(),
        })
    }

    fn validate_update(
        &self,
        conn: &Connection,
        old: &SkinGrant,
        scope: &str,
        _ctx: &HookCtx,
    ) -> Result<String, String> {
        let old_scope = ChatScope::parse_stored(old.scope.as_deref().unwrap_or(""))?;
        let new_scope = parse_input_scope(conn, &old.subject_kind, Some(scope))?;
        if new_scope.for_principal() != (old.subject_kind == "principal") {
            return Err(SCOPE_ERR.into());
        }
        if old_scope.is_assistant() != new_scope.is_assistant() {
            return Err(KIND_FLIP_ERR.into());
        }
        if old_scope == ChatScope::Member && new_scope == ChatScope::Guest {
            assistant_refusal(conn, &old.subject_id)?;
        }
        check_assistant_person(conn, &new_scope)?;
        Ok(new_scope.as_stored())
    }

    fn validate_disable(&self, conn: &Connection, grant: &SkinGrant) -> Result<(), String> {
        if grant.subject_kind == "principal" {
            assistant_refusal(conn, &grant.subject_id)?;
        }
        Ok(())
    }

    fn plan(
        &self,
        conn: &Connection,
        ev: &HookEvent<'_>,
        ctx: &HookCtx,
    ) -> Result<Vec<NewOp>, String> {
        match *ev {
            HookEvent::GrantCreated { grant } if grant.enabled => {
                Ok(vec![provision_op(conn, grant, ctx, Cause::GrantCreate)?])
            }
            HookEvent::GrantEnabled { grant } => {
                Ok(vec![provision_op(conn, grant, ctx, Cause::GrantEnable)?])
            }
            // G3 upsert. S27-A8 caveat: Sew clears omitted title/pronouns on
            // an upsert of an active member (see the PRD amendments).
            HookEvent::GrantUpdated { new, .. } if new.enabled => {
                Ok(vec![provision_op(conn, new, ctx, Cause::GrantUpdate)?])
            }
            HookEvent::GrantDeleted { grant } => {
                Ok(vec![deprovision_op(grant, Cause::GrantDelete)?])
            }
            HookEvent::GrantDisabled { grant } => {
                Ok(vec![deprovision_op(grant, Cause::GrantDisable)?])
            }
            HookEvent::PrincipalChanged { principal } => {
                let Some(g) = person_grant(conn, &principal.id)? else {
                    return Ok(Vec::new());
                };
                if !g.enabled {
                    return Ok(Vec::new());
                }
                let ext = external_id("principal", &principal.id)?;
                let display = principal
                    .full_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&principal.username);
                let patch = ProfilePatch {
                    display_name: Some(truncate_chars(display, DISPLAY_NAME_MAX)),
                    email: match principal
                        .email
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        Some(e) => Patch::Set(e.to_string()),
                        None => Patch::Clear,
                    },
                    ..Default::default()
                };
                Ok(vec![update_op(
                    &ext,
                    patch,
                    Some(&g),
                    Cause::PrincipalUpdate,
                )?])
            }
            HookEvent::TargetEnabled => owner_ops(ctx, Cause::TargetEnable),
            _ => Ok(Vec::new()),
        }
    }

    fn cascade(&self, conn: &Connection, ev: &HookEvent<'_>) -> Result<Vec<SkinGrant>, String> {
        match *ev {
            HookEvent::GrantDeleted { grant } if grant.subject_kind == "principal" => {
                assistants_of(conn, &grant.subject_id)
            }
            _ => Ok(Vec::new()),
        }
    }
}
