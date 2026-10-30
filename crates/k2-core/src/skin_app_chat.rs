//! Sew for Apps A2 (0.46.6): `guests:chat`, an App gives (or takes back)
//! chat for **its own guests**, under the owner's grant, so an App with
//! hundreds of users never needs the owner for each one.
//!
//! One rule set, two doors (each door only decides who the caller is):
//! - the App transport: `GET|POST /cli/skin/guests/chat` with the App's own
//!   pass (`guests.chat` / `guests.setChat`), only when the owner granted
//!   this App `guests-chat` ([`grant`]);
//! - the CLI: `k2 app user chat <user> member|guest|none [--app <app>]`,
//!   the owner (any person, no ceiling) or an agent for its own
//!   workspace's App within that App's grant.
//!
//! What a set writes is the person's one `chat` grant (Sew S27: it makes
//! the Sew account, `k2:principal:<id>`; K31 then adds them to the App's
//! bound Spaces). The rules, whoever calls through an App:
//! - only a guest of **this App** (a principal with a `kind=app` grant on
//!   it). A Connect login, another App's guest, a name nobody has are the
//!   same [`Refusal::NotFound`] (no oracle). An agent login (`k2 app user
//!   agent`) is never a chat person ([`Refusal::AgentLogin`]): agents are
//!   their workspace in chat.
//! - a chat grant is per person, box-wide, so an App only changes the grant
//!   **it** made (`grants.created_by = app:<service id>`). A grant the
//!   owner (or another App) made is left alone: the same scope is a no-op,
//!   anything else is [`Refusal::NotAppGranted`]. A grant the owner switched
//!   off stays off ([`Refusal::Disabled`]).
//! - `none` never strands someone: a person who also holds another App's
//!   login (or a box login) keeps chat ([`Refusal::SharedPerson`]); a person
//!   with assistant agents keeps it too ([`Refusal::HasAssistants`]).
//! - a per-App member ceiling: at most the grant's `maxMembers` people hold
//!   an App-made chat grant from this App ([`Refusal::Limit`]).
//! - the username must work as a Sew handle (2–64 of `a-z 0-9 _ -`, not a
//!   mention word): else [`Refusal::Unusable`].
//! - **the safety gate** (Sew 2026-10-28): until the box's Sew reports
//!   `app_scoped_sessions`, a session isn't scoped to its App, so an App
//!   holding a person's chat pass could read all chat that person can. An
//!   App may then not GIVE chat ([`Refusal::NotAppScoped`]); only the
//!   owner (or an Admin) can, and every answer carries [`WARN_UNSCOPED`].
//! - idempotent (the same scope writes nothing), audited (one hash-chained
//!   `deploy_audit` row per change), at most [`RATE_PER_MIN`] calls a
//!   minute per App.
//!
//! The owner's grant lives in skin.db (`app_chat_grants`, created at open:
//! additive, no numbered step, so an older K2 ignores it).

use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

use super::{app_grant, normalize_username, now_secs, principal_by_username, with_conn, HOST_APP_ID};

/// The catalog cap of both verbs (an owner grant per App, never a pass cap).
pub const CAP: &str = "guests:chat";
/// The grant kind `k2 app grant <app> --kind guests-chat` writes.
pub const GRANT_KIND: &str = "guests-chat";
/// The per-App member ceiling when the owner names none.
pub const DEFAULT_MAX_MEMBERS: u32 = 100;
/// The largest ceiling an owner may set.
pub const MAX_MAX_MEMBERS: u32 = 10_000;
/// Calls a minute per App (reads and writes).
pub const RATE_PER_MIN: usize = 60;
/// `grants.created_by` of a chat grant an App made: `app:<service id>`.
pub const CREATED_BY_PREFIX: &str = "app:";

/// What the owner is told while Sew doesn't scope sessions to an App.
pub const WARN_UNSCOPED: &str = "this App can see all chat this person can (this Sew doesn't scope sessions to an App yet: it needs app_scoped_sessions)";

/// Does the box's Sew scope sessions to their App (feature flag
/// `app_scoped_sessions`)?
pub fn app_scoped() -> Result<bool, String> {
    crate::tool_hook::sew_has(crate::tool_hook::features::APP_SCOPED_SESSIONS)
}

/// `deploy_audit` events.
pub const AUDIT_CHANGE: &str = "guest_chat";
pub const AUDIT_GRANT: &str = "guest_chat_grant";
pub const AUDIT_REVOKE: &str = "guest_chat_revoke";

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_chat_grants (
            service_id TEXT PRIMARY KEY NOT NULL,
            max_members INTEGER NOT NULL,
            granted_by TEXT NOT NULL,
            granted_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db app_chat_grants: {e}"))
}

/// The `created_by` an App's own chat grant carries.
pub fn app_marker(service_id: &str) -> String {
    format!("{CREATED_BY_PREFIX}{}", service_id.trim())
}

// ── the owner's per-App grant ────────────────────────────────────────

/// One App's `guests-chat` grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGrant {
    pub service_id: String,
    pub kind: String,
    /// People this App may give chat to at once (its App-made grants).
    pub max_members: u32,
    pub granted_by: String,
    pub granted_at: i64,
}

fn grant_in(conn: &Connection, service_id: &str) -> Result<Option<ChatGrant>, String> {
    conn.query_row(
        "SELECT service_id, max_members, granted_by, granted_at FROM app_chat_grants WHERE service_id = ?1",
        params![service_id],
        |r| {
            Ok(ChatGrant {
                service_id: r.get(0)?,
                kind: GRANT_KIND.to_string(),
                max_members: r.get::<_, i64>(1)?.clamp(0, MAX_MAX_MEMBERS as i64) as u32,
                granted_by: r.get(2)?,
                granted_at: r.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("app chat grant lookup: {e}"))
}

/// The App's `guests-chat` grant, if the owner gave one.
pub fn get_grant(service_id: &str) -> Result<Option<ChatGrant>, String> {
    with_conn(|conn| grant_in(conn, service_id.trim()))
}

/// Give (or re-give, replacing its ceiling) the App `guests-chat`.
/// Returns `(before, after)`.
pub fn grant(service_id: &str, max_members: Option<u32>, by: &str) -> Result<(Option<ChatGrant>, ChatGrant), String> {
    let svc = service_id.trim();
    if svc.is_empty() || svc == HOST_APP_ID {
        return Err("name a published App (host is the box login, not an App)".into());
    }
    let max = max_members.unwrap_or(DEFAULT_MAX_MEMBERS);
    if !(1..=MAX_MAX_MEMBERS).contains(&max) {
        return Err(format!("maxMembers is 1..={MAX_MAX_MEMBERS}"));
    }
    let r = with_conn(|conn| {
        let before = grant_in(conn, svc)?;
        conn.execute(
            "INSERT INTO app_chat_grants (service_id, max_members, granted_by, granted_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(service_id) DO UPDATE SET max_members = excluded.max_members, granted_by = excluded.granted_by,
               granted_at = excluded.granted_at",
            params![svc, max as i64, by, now_secs()],
        )
        .map_err(|e| format!("app chat grant write: {e}"))?;
        let after = grant_in(conn, svc)?.ok_or_else(|| "app chat grant vanished".to_string())?;
        Ok((before, after))
    })?;
    audit(svc, AUDIT_GRANT, by, json!({ "kind": GRANT_KIND, "before": r.0.as_ref().map(|g| g.max_members), "after": r.1.max_members }));
    Ok(r)
}

/// Take the App's `guests-chat` grant away. The chat grants it made stay
/// (people keep chat); the App just can't change any more.
pub fn revoke(service_id: &str, by: &str) -> Result<Option<ChatGrant>, String> {
    let svc = service_id.trim();
    let before = with_conn(|conn| {
        let before = grant_in(conn, svc)?;
        conn.execute("DELETE FROM app_chat_grants WHERE service_id = ?1", params![svc])
            .map_err(|e| format!("app chat grant delete: {e}"))?;
        Ok(before)
    })?;
    if let Some(g) = &before {
        audit(svc, AUDIT_REVOKE, by, json!({ "kind": GRANT_KIND, "before": g.max_members }));
    }
    Ok(before)
}

// ── the rules ────────────────────────────────────────────────────────

/// What a set asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Member,
    Guest,
    /// Take chat away.
    None,
}

impl Scope {
    pub fn parse(raw: &str) -> Option<Scope> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "member" => Some(Scope::Member),
            "guest" => Some(Scope::Guest),
            "none" | "off" => Some(Scope::None),
            _ => None,
        }
    }

    fn stored(self) -> Option<&'static str> {
        match self {
            Scope::Member => Some("member"),
            Scope::Guest => Some("guest"),
            Scope::None => None,
        }
    }
}

/// Who acts, and with which limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The App itself (its pass), or an agent for its own workspace's App:
    /// this App's guests only, its own grants only, under the grant's
    /// ceiling.
    App,
    /// The owner: any person (with `app`, that App's guests), any chat
    /// grant, no ceiling.
    Owner,
}

/// Why a call was refused. The doors map each to a status and code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a guest of this App (or no such name): `404 not_found`.
    NotFound,
    /// A missing or malformed argument: `400 usage`.
    Usage(String),
    /// An agent login: `409 agent_login`.
    AgentLogin,
    /// The username can't be a Sew handle: `422 username_unusable`.
    Unusable(String),
    /// The person's chat grant isn't this App's: `409 not_app_granted`.
    NotAppGranted,
    /// The owner switched the person's chat off: `409 chat_off_for_person`.
    Disabled,
    /// The person uses chat through another App or the box login too:
    /// `409 shared_person`.
    SharedPerson,
    /// The person has assistant agents in chat: `409 has_assistants`.
    HasAssistants,
    /// Over the App's member ceiling: `409 chat_limit`.
    Limit { max: u32 },
    /// Sew isn't enabled on this box: `409 chat_unavailable`.
    ChatOff,
    /// The safety gate: Sew doesn't scope sessions to an App yet, so an App
    /// may not give chat: `409 chat_not_app_scoped`.
    NotAppScoped,
    /// Over [`RATE_PER_MIN`]: `429 rate_limited`.
    RateLimited { retry_after_ms: u64 },
    /// This App's user has no Sew member yet (A6): `404 no_chat_member`.
    NoChatMember,
    /// Storage: `500 failed`.
    Store(String),
}

impl Refusal {
    pub fn status(&self) -> u16 {
        match self {
            Refusal::NotFound | Refusal::NoChatMember => 404,
            Refusal::Usage(_) => 400,
            Refusal::Unusable(_) => 422,
            Refusal::AgentLogin
            | Refusal::NotAppGranted
            | Refusal::Disabled
            | Refusal::SharedPerson
            | Refusal::HasAssistants
            | Refusal::Limit { .. }
            | Refusal::ChatOff
            | Refusal::NotAppScoped => 409,
            Refusal::RateLimited { .. } => 429,
            Refusal::Store(_) => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NotFound => "not_found",
            Refusal::Usage(_) => "usage",
            Refusal::AgentLogin => "agent_login",
            Refusal::Unusable(_) => "username_unusable",
            Refusal::NotAppGranted => "not_app_granted",
            Refusal::Disabled => "chat_off_for_person",
            Refusal::SharedPerson => "shared_person",
            Refusal::HasAssistants => "has_assistants",
            Refusal::Limit { .. } => "chat_limit",
            Refusal::ChatOff => "chat_unavailable",
            Refusal::NotAppScoped => "chat_not_app_scoped",
            Refusal::RateLimited { .. } => "rate_limited",
            Refusal::NoChatMember => "no_chat_member",
            Refusal::Store(_) => "failed",
        }
    }

    pub fn hint(&self) -> String {
        match self {
            Refusal::NotFound => "no such guest of this App".into(),
            Refusal::Usage(m) | Refusal::Store(m) | Refusal::Unusable(m) => m.clone(),
            Refusal::AgentLogin => "an agent login is never a chat person (agents are their workspace in chat: k2 chat)".into(),
            Refusal::NotAppGranted => {
                "this person's chat was given by the owner (or another App); only the owner changes it (k2 app user chat)".into()
            }
            Refusal::Disabled => "the owner switched this person's chat off; only the owner turns it back on".into(),
            Refusal::SharedPerson => {
                "this person also signs in to another App (or the box); their chat stays (the owner: k2 app user chat <user> none)".into()
            }
            Refusal::HasAssistants => "this person has assistant agents in chat; the owner removes those first".into(),
            Refusal::Limit { max } => format!(
                "this App already gave chat to {max} people, its ceiling (the owner raises it: k2 app grant <app> --kind guests-chat --max N)"
            ),
            Refusal::ChatOff => "Chat / Voice Hosting is not enabled on this server: k2 hostchat enable".into(),
            Refusal::NotAppScoped => "an App can't give chat yet: this server's Sew doesn't keep an App's sessions to that App's own Spaces \
                 (it needs app_scoped_sessions). Until it does, the owner gives chat: k2 app user chat <user> member --app <app> \
                 (and this App can see all chat that person can)"
                .into(),
            Refusal::NoChatMember => {
                "this user has no chat member yet: give them chat (guests.setChat); the member appears within seconds".into()
            }
            Refusal::RateLimited { retry_after_ms } => {
                format!("at most {RATE_PER_MIN} guest-chat calls a minute per App; retry after {retry_after_ms} ms")
            }
        }
    }
}

impl From<String> for Refusal {
    fn from(e: String) -> Self {
        Refusal::Store(e)
    }
}

/// One person's chat as an App (or the owner) sees it (the catalog's
/// `AppGuestChat`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestChat {
    pub ok: bool,
    /// The App's service id (`None` for an owner call without one).
    pub app: Option<String>,
    pub username: String,
    /// The stable key in K2 and Sew: `k2:principal:<id>`.
    pub principal_id: String,
    /// `member` | `guest`; `None` = no chat.
    pub scope: Option<String>,
    /// The chat grant is on (a grant the owner switched off is `false`).
    pub enabled: bool,
    /// `app` = this App made the grant; `owner` = the owner (or another
    /// App) did; `None` = no grant.
    pub managed_by: Option<String>,
    /// People this App gave chat to (its own grants), and its ceiling.
    pub members: Option<u32>,
    pub max_members: Option<u32>,
    /// `true` only when a set wrote something.
    pub changed: bool,
    /// The box's Sew keeps this App's sessions to its own Spaces
    /// (`app_scoped_sessions`).
    pub app_scoped: bool,
    /// [`WARN_UNSCOPED`] when an App is named and Sew doesn't scope yet.
    pub warning: Option<String>,
}

struct Person {
    id: String,
    username: String,
}

/// A person by username: through an App, only that App's guest.
fn person_of(app: Option<&str>, username: &str) -> Result<Person, Refusal> {
    let app = app.map(str::trim).filter(|a| !a.is_empty());
    if app == Some(HOST_APP_ID) {
        return Err(Refusal::NotFound);
    }
    let Ok(username) = normalize_username(username) else { return Err(Refusal::NotFound) };
    // A Connect login (the owner included) is never a chat principal here.
    if crate::connect_users::role_for_user(&username).is_some() {
        return Err(Refusal::NotFound);
    }
    let p = with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else { return Ok(None) };
        if let Some(a) = app {
            if app_grant(conn, &p.id, a)?.is_none() {
                return Ok(None);
            }
        }
        Ok(Some(Person { id: p.id, username: p.username }))
    })?
    .ok_or(Refusal::NotFound)?;
    if super::principal_agent_workspace(&p.id)?.is_some() {
        return Err(Refusal::AgentLogin);
    }
    Ok(p)
}

/// The App-made chat grants of `app` that are on (its ceiling's count).
fn app_members(conn: &Connection, app: &str) -> Result<u32, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM grants WHERE kind = ?1 AND subject_kind = 'principal' AND created_by = ?2 AND enabled = 1",
        params![crate::tool_hook::CHAT_KIND, app_marker(app)],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u32)
    .map_err(|e| format!("app chat count: {e}"))
}

/// Does `principal` hold a login grant (`kind=app`) on anything but `app`?
fn other_login(conn: &Connection, principal_id: &str, app: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app'
           AND target_id != ?2 AND enabled = 1)",
        params![principal_id, app],
        |r| r.get::<_, bool>(0),
    )
    .map_err(|e| format!("other login lookup: {e}"))
}

fn view(app: Option<&str>, p: &Person, changed: bool) -> Result<GuestChat, Refusal> {
    let app = app.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
    let (g, members, max) = with_conn(|conn| {
        let g = crate::tool_hook::chat::person_grant(conn, &p.id)?;
        let (members, max) = match app.as_deref() {
            Some(a) => (Some(app_members(conn, a)?), grant_in(conn, a)?.map(|g| g.max_members)),
            None => (None, None),
        };
        Ok((g, members, max))
    })?;
    let managed_by = g.as_ref().map(|g| {
        let mine = app.as_deref().is_some_and(|a| g.created_by.as_deref() == Some(app_marker(a).as_str()));
        if mine { "app" } else { "owner" }.to_string()
    });
    let app_scoped = app_scoped()?;
    let warning = (app.is_some() && !app_scoped).then(|| WARN_UNSCOPED.to_string());
    Ok(GuestChat {
        app_scoped,
        warning,
        ok: true,
        app,
        username: p.username.clone(),
        principal_id: p.id.clone(),
        scope: g.as_ref().and_then(|g| g.scope.clone()),
        enabled: g.as_ref().is_some_and(|g| g.enabled),
        managed_by,
        members,
        max_members: max,
        changed,
    })
}

fn rate_take(app: &str) -> Result<(), Refusal> {
    super::guest_rooms::rate_take(&format!("guest-chat:{}", app.trim())).map_err(|e| match e {
        super::guest_rooms::Refusal::RateLimited { retry_after_ms } => Refusal::RateLimited { retry_after_ms },
        other => Refusal::Store(other.hint()),
    })
}

/// `guests.chat`: one guest of `app`, its chat. Counts against the rate.
pub fn read(app: &str, username: &str) -> Result<GuestChat, Refusal> {
    rate_take(app)?;
    let p = person_of(Some(app), username)?;
    view(Some(app), &p, false)
}

/// The owner's read (`k2 app user chat <user>`): any person, or `app`'s
/// guest when `app` is named.
pub fn owner_read(app: Option<&str>, username: &str) -> Result<GuestChat, Refusal> {
    let p = person_of(app, username)?;
    view(app, &p, false)
}

fn sets() -> &'static Mutex<()> {
    static M: Mutex<()> = Mutex::new(());
    &M
}

fn hook_err(e: String) -> Refusal {
    if e == crate::tool_hook::CHAT_NOT_ENABLED {
        Refusal::ChatOff
    } else if e.contains("chat cannot use it") {
        Refusal::Unusable(e)
    } else if e.starts_with("remove the assistant grants") {
        Refusal::HasAssistants
    } else {
        Refusal::Store(e)
    }
}

/// `guests.setChat` (and the CLI): set one person's chat to `scope`.
/// `app` is the App acting (required for [`Caller::App`]); `actor` and
/// `via` (`app` | `cli`) go in the audit row.
pub fn set(caller: &Caller, app: Option<&str>, username: &str, scope: Scope, actor: &str, via: &str) -> Result<GuestChat, Refusal> {
    let app = app.map(str::trim).filter(|a| !a.is_empty());
    if *caller == Caller::App {
        let Some(a) = app else { return Err(Refusal::NotFound) };
        rate_take(a)?;
    }
    // One set at a time: the ceiling's count and the write must not race.
    let _one = sets().lock().unwrap_or_else(|p| p.into_inner());
    let p = person_of(app, username)?;
    if scope != Scope::None {
        // The safety gate: never an App-given chat Sew can't keep to the App.
        if *caller == Caller::App && !app_scoped()? {
            return Err(Refusal::NotAppScoped);
        }
        crate::tool_hook::chat::check_username(&p.username).map_err(Refusal::Unusable)?;
    }
    let (current, ceiling, members, shared) = with_conn(|conn| {
        let g = crate::tool_hook::chat::person_grant(conn, &p.id)?;
        let (ceiling, members, shared) = match app {
            Some(a) => (grant_in(conn, a)?, app_members(conn, a)?, other_login(conn, &p.id, a)?),
            None => (None, 0, false),
        };
        Ok((g, ceiling, members, shared))
    })?;
    let mine = |g: &crate::skin::SkinGrant| app.is_some_and(|a| g.created_by.as_deref() == Some(app_marker(a).as_str()));
    let by_app = *caller == Caller::App;
    let changed = match (scope.stored(), current) {
        (None, None) => false,
        (Some(want), None) => {
            if by_app {
                let max = ceiling.as_ref().map_or(0, |g| g.max_members);
                if members >= max {
                    return Err(Refusal::Limit { max });
                }
            }
            let created_by = if by_app { app_marker(app.unwrap_or_default()) } else { actor.to_string() };
            super::create_grant(&super::SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: p.id.clone(),
                kind: crate::tool_hook::CHAT_KIND.into(),
                target_id: String::new(),
                role: None,
                scope: Some(want.into()),
                created_by: Some(created_by),
            })
            .map_err(hook_err)?;
            true
        }
        (Some(want), Some(g)) => {
            let same = g.scope.as_deref() == Some(want);
            if by_app && !mine(&g) {
                if same && g.enabled {
                    false
                } else if !g.enabled {
                    return Err(Refusal::Disabled);
                } else {
                    return Err(Refusal::NotAppGranted);
                }
            } else if by_app && !g.enabled {
                return Err(Refusal::Disabled);
            } else {
                let mut wrote = false;
                if !same {
                    super::update_grant_scope(&g.id, want, actor).map_err(hook_err)?;
                    wrote = true;
                }
                if !g.enabled {
                    // Only the owner gets here (an App is refused above).
                    super::set_grant_enabled_as(&g.id, true, actor).map_err(hook_err)?;
                    wrote = true;
                }
                wrote
            }
        }
        (None, Some(g)) => {
            if by_app {
                if !mine(&g) {
                    return Err(Refusal::NotAppGranted);
                }
                if shared {
                    return Err(Refusal::SharedPerson);
                }
            }
            let has_assistants = with_conn(|conn| crate::tool_hook::chat::assistants_of(conn, &p.id))?;
            if !has_assistants.is_empty() {
                return Err(Refusal::HasAssistants);
            }
            super::delete_grant_by_id_with_cascade(&g.id, actor).map_err(hook_err)?;
            true
        }
    };
    let out = view(app, &p, changed)?;
    if changed {
        if let Some(a) = app {
            audit(
                a,
                AUDIT_CHANGE,
                actor,
                json!({ "username": out.username, "after": out.scope, "via": via, "managedBy": out.managed_by }),
            );
        }
    }
    Ok(out)
}

// ── the identity map (Sew for Apps A6) ───────────────────────────────

/// Most member ids one `chat.whois` call resolves.
pub const WHOIS_MAX: usize = 100;

/// `chat.member`: one of this App's users and their Sew member (the
/// catalog's `AppChatMember`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMember {
    pub ok: bool,
    pub app: String,
    pub username: String,
    pub principal_id: String,
    /// The stable key in Sew: `k2:principal:<principalId>`.
    pub external_id: String,
    /// Sew's member id (for DMs, calls, mentions, member adds).
    pub member_id: String,
    /// `active` | `deactivated` (chat taken away; the same id comes back
    /// when chat is given again).
    pub state: String,
}

/// One answer of `chat.whois` (the catalog's `AppChatWhoisRow`). A member
/// who isn't this App's user shows only `memberId`, `kind` and
/// `isThisAppUser: false`: never their principal, name or other Apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhoisRow {
    pub member_id: String,
    /// `person` | `agent` | `unknown` (no member K2 knows by that id).
    pub kind: String,
    pub is_this_app_user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The K2 profile picture's hash (fetch the picture through K2's own
    /// profile verbs; Sew shows the same one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_hash: Option<String>,
    /// The person was deleted in K2 (a tombstone: show "deleted user").
    pub deleted: bool,
}

/// `chat.whois` (the catalog's `AppChatWhois`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Whois {
    pub ok: bool,
    pub app: String,
    pub members: Vec<WhoisRow>,
}

/// A person by username or principal id, through `app`.
fn person_by_token(app: &str, token: &str) -> Result<Person, Refusal> {
    let t = token.trim();
    if crate::tool_hook::sew_wire::is_lower_uuid(t) {
        let name = with_conn(|conn| crate::tool_hook::chat::principal_by_id(conn, t))?.map(|p| p.username);
        return match name {
            Some(n) => person_of(Some(app), &n),
            None => Err(Refusal::NotFound),
        };
    }
    person_of(Some(app), t)
}

/// `chat.member`: this App's user (by username or principal id) → their
/// Sew member. Anyone not this App's user is the same 404 as nobody. A
/// user without a member yet (no chat, or its provision still queued) is
/// [`Refusal::NoChatMember`].
pub fn member_of(app: &str, who: &str) -> Result<ChatMember, Refusal> {
    rate_take(app)?;
    let p = person_by_token(app, who)?;
    let ext = crate::tool_hook::sew_wire::ExternalId::K2Principal(p.id.clone()).to_string();
    let row = with_conn(|conn| crate::tool_hook::members::by_external_id(conn, &ext))?;
    match row {
        Some(r) if r.state != crate::tool_hook::members::DELETED => Ok(ChatMember {
            ok: true,
            app: app.trim().to_string(),
            username: p.username,
            principal_id: p.id,
            external_id: ext,
            member_id: r.member_id,
            state: r.state,
        }),
        _ => Err(Refusal::NoChatMember),
    }
}

fn member_id_ok(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `chat.whois`: Sew member ids → who they are, as far as THIS App may
/// know. One call against the rate, at most [`WHOIS_MAX`] ids.
pub fn whois(app: &str, member_ids: &[String]) -> Result<Whois, Refusal> {
    let app = app.trim();
    if app.is_empty() || app == HOST_APP_ID {
        return Err(Refusal::NotFound);
    }
    if member_ids.is_empty() || member_ids.len() > WHOIS_MAX {
        return Err(Refusal::Usage(format!("memberIds is a list of 1..={WHOIS_MAX} member ids")));
    }
    if let Some(bad) = member_ids.iter().find(|m| !member_id_ok(m.trim())) {
        return Err(Refusal::Usage(format!("'{bad}' is not a member id")));
    }
    rate_take(app)?;
    // The App's own room agent is the one agent it may name.
    let own_room = {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::published_services::get_by_id(&conn, app).map_err(|e| format!("app lookup: {e}"))?.map(|r| r.project_id)
    };
    let own_agent = own_room.as_ref().map(|ws| crate::tool_hook::sew_wire::ExternalId::K2Workspace(ws.clone()).to_string());
    let agent_display = own_room.as_deref().and_then(crate::tool_hook::workspace_facts).map(|f| f.display_name);
    let members = with_conn(|conn| {
        let mut out = Vec::with_capacity(member_ids.len());
        for id in member_ids.iter().map(|m| m.trim()) {
            let masked = |kind: &str| WhoisRow {
                member_id: id.to_string(),
                kind: kind.to_string(),
                is_this_app_user: false,
                principal_id: None,
                username: None,
                display_name: None,
                image_hash: None,
                deleted: false,
            };
            let Some(r) = crate::tool_hook::members::by_member_id(conn, id)? else {
                out.push(masked("unknown"));
                continue;
            };
            if let Some(pid) = r.external_id.strip_prefix("k2:principal:") {
                if r.state == crate::tool_hook::members::DELETED {
                    out.push(WhoisRow { deleted: true, ..masked("person") });
                    continue;
                }
                let facts = crate::tool_hook::chat::principal_by_id(conn, pid)?;
                match facts {
                    Some(f) if app_grant(conn, pid, app)?.is_some() => {
                        let display = crate::tool_hook::chat::principal_display_name(conn, pid, f.full_name.as_deref(), &f.username)?;
                        let image_hash = crate::tool_hook::avatar::picture_hash(conn, pid)?;
                        out.push(WhoisRow {
                            is_this_app_user: true,
                            principal_id: Some(pid.to_string()),
                            username: Some(f.username),
                            display_name: Some(display),
                            image_hash,
                            ..masked("person")
                        });
                    }
                    _ => out.push(masked("person")),
                }
            } else if r.external_id.starts_with("k2:workspace:") {
                if own_agent.as_deref() == Some(r.external_id.as_str()) {
                    out.push(WhoisRow {
                        username: Some(r.username.clone()),
                        display_name: agent_display.clone().or_else(|| Some(r.username.clone())),
                        ..masked("agent")
                    });
                } else {
                    out.push(masked("agent"));
                }
            } else {
                out.push(masked("person"));
            }
        }
        Ok(out)
    })?;
    Ok(Whois { ok: true, app: app.to_string(), members })
}

/// The App behind `service_id` was removed (Rosson 2026-10-28, Q12): a
/// person whose only login was this App gets their chat grant switched off
/// (their Sew member is deactivated, never deleted: the same member id
/// comes back if chat is given again). People who also sign in to another
/// App or the box keep chat. The App's `guests-chat` grant goes. Returns
/// how many people lost chat.
pub fn app_removed(service_id: &str, actor: &str) -> Result<usize, String> {
    let svc = service_id.trim();
    if svc.is_empty() || svc == HOST_APP_ID {
        return Ok(0);
    }
    let off: Vec<String> = with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT DISTINCT subject_id FROM grants WHERE subject_kind = 'principal' AND kind = 'app' AND target_id = ?1",
            )
            .map_err(|e| format!("app users: {e}"))?;
        let ids: Vec<String> = st
            .query_map(params![svc], |r| r.get::<_, String>(0))
            .map_err(|e| format!("app users: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("app users: {e}"))?;
        drop(st);
        let mut off = Vec::new();
        for pid in ids {
            if other_login(conn, &pid, svc)? {
                continue;
            }
            if let Some(g) = crate::tool_hook::chat::person_grant(conn, &pid)? {
                if g.enabled && crate::tool_hook::chat::assistants_of(conn, &pid)?.is_empty() {
                    off.push(g.id);
                }
            }
        }
        Ok(off)
    })?;
    for id in &off {
        super::set_grant_enabled_as(id, false, actor)?;
    }
    revoke(svc, actor)?;
    if !off.is_empty() {
        crate::log_debug!("[skin] App {svc} removed: chat switched off for {} people whose only App it was", off.len());
    }
    // Its Sew Spaces: K2 forgets the bindings (members stay) and Sew
    // unbinds them (read-only orphans) once it reports `space_unbind`.
    crate::tool_hook::app_spaces_app_removed(svc, actor)?;
    Ok(off.len())
}

// ── audit ────────────────────────────────────────────────────────────

fn audit(service_id: &str, event: &str, actor: &str, mut payload: serde_json::Value) {
    payload["service"] = json!(service_id);
    let subject = super::guest_rooms::audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    if let Err(e) = crate::deploy::audit::append(&conn, crate::deploy::audit::SUBJECT_APP, &subject, event, actor, None, &payload) {
        crate::log_debug!("[skin] guest chat audit {service_id} {event}: {e}");
    }
}

/// The App's guest-chat audit rows, newest first (tests, `k2 app history`).
pub fn audit_rows(service_id: &str, limit: usize) -> Result<Vec<crate::deploy::audit::AuditRow>, String> {
    let subject = super::guest_rooms::audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = crate::deploy::audit::list_for(&conn, crate::deploy::audit::SUBJECT_APP, &subject, limit.max(1) * 4)
        .map_err(|e| format!("audit read: {e}"))?;
    Ok(rows
        .into_iter()
        .filter(|r| [AUDIT_CHANGE, AUDIT_GRANT, AUDIT_REVOKE].contains(&r.event.as_str()))
        .filter(|r| r.payload.get("service").and_then(|s| s.as_str()) == Some(service_id))
        .take(limit)
        .collect())
}

#[cfg(test)]
#[path = "skin_app_chat_tests.rs"]
mod tests;
