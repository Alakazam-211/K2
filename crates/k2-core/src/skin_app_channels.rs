//! Sew for Apps A9 (0.46.6): `chat.createChannel`, an App makes a text
//! channel in **its own** bound Sew Space and puts its own users in it.
//!
//! The decisions live here (sync, over skin.db); the daemon's route
//! (`chat_channel_routes`) does the two mgmt calls in between:
//!
//! 1. [`prepare`]: the gates (rate, Sew's `app_channel_create` flag, the
//!    chat target), the Space (the App's own binding: named, or its only
//!    one) and the listed users. Each listed username must be this App's
//!    user holding chat as a member: anyone else is `not_found` (the same
//!    answer as nobody), a chat guest `guest`, no chat `no_chat`. A user
//!    the ledger holds banned in that Space is `skipped` and never sent.
//! 2. the daemon sends `POST /mgmt/v1/spaces/members/add` for the users
//!    not yet in the Space (K31's ledger says who is), then
//!    [`after_space_add`] records Sew's answer in the ledger (`added` /
//!    `already` → in the Space; `skipped` → banned, held for good, HC2) and
//!    passes `not_added` through.
//! 3. the daemon sends `POST /mgmt/v1/channels/create` with the users now
//!    in the Space; [`finish`] maps the answer back to usernames and writes
//!    one `deploy_audit` row (`chat_channel`).
//!
//! Sew's create is idempotent on `(space, slug)`: a repeat answers
//! `created: false` with the channel's CURRENT name and visibility (a
//! different `name` or `private` changes nothing) and adds listed users it
//! lacks. Text channels only (Sew 0.5.0).
//!
//! Only the App's own pass with `chat:channels` reaches this (the route
//! checks); guests, the owner token and logins never do. At most
//! [`RATE_PER_MIN`] calls a minute per App.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::json;

use super::{app_grant, normalize_username, principal_by_username, with_conn, HOST_APP_ID};
use crate::tool_hook::app_space::{self, Wanted};
use crate::tool_hook::sew_wire::{self, ChannelCreateResponse, ExternalId, SpaceMembersAddResponse};
use crate::tool_hook::{features, CHAT_KIND};

/// The catalog cap of `chat.createChannel` (the App's own; never a role's).
pub const CAP: &str = super::CAP_CHAT_CHANNELS;
/// Most users one call lists (Sew's channel-create limit).
pub const MEMBERS_MAX: usize = sew_wire::CHANNEL_MEMBERS_MAX;
/// Longest channel name, characters (Sew's `NAME_MAX_CHARS`).
pub const NAME_MAX_CHARS: usize = 100;
/// Calls a minute per App (the guest-rooms bucket's window and limit).
pub const RATE_PER_MIN: usize = super::guest_rooms::RATE_PER_MIN;
/// The `deploy_audit` event.
pub const AUDIT_EVENT: &str = "chat_channel";
/// What `sew_feature_missing` says.
pub const NEEDS_SEW: &str = "this server's Sew can't make channels for Apps yet: it needs Sew 0.5.0+ (feature app_channel_create); the owner upgrades Sew (k2 hostchat upgrade)";

/// K2's own `notAdded` reasons (Sew's pass through as they are:
/// `not_provisioned`, `deactivated`, `assistant`, `guest`,
/// `not_space_member`).
pub const NOT_FOUND: &str = "not_found";
pub const NO_CHAT: &str = "no_chat";
pub const GUEST: &str = "guest";

/// Why a call was refused. The route maps each to a status and code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A missing or malformed argument: `400 usage`.
    Usage(String),
    /// Sew lacks `app_channel_create`: `409 sew_feature_missing`.
    FeatureMissing,
    /// Sew isn't enabled here: `409 chat_unavailable`.
    ChatOff,
    /// The Space isn't bound to this App (or Sew says it is bound to no
    /// App): `409 space_unbound`.
    SpaceUnbound(String),
    /// The Space belongs to another App: `409 app_mismatch`.
    AppMismatch(String),
    /// Sew has no such Space: `404 not_found`.
    NotFound(String),
    /// Sew 0.6.0: the slug already holds a channel of another type (a voice
    /// channel; K2 makes text ones): `409 channel_type_mismatch`, nothing
    /// written. `existing_type` is Sew's (`voice`), or empty when absent.
    TypeMismatch { slug: String, existing_type: String },
    /// Sew didn't answer (down, 5xx, a refused token): `503 chat_unavailable`.
    Unreachable(String),
    /// Sew refused for another reason (a quota, an archived channel): `409 failed`.
    Sew(String),
    /// Over [`RATE_PER_MIN`]: `429 rate_limited`.
    RateLimited { retry_after_ms: u64 },
    /// Storage: `500 failed`.
    Store(String),
}

impl Refusal {
    pub fn status(&self) -> u16 {
        match self {
            Refusal::Usage(_) => 400,
            Refusal::NotFound(_) => 404,
            Refusal::FeatureMissing
            | Refusal::ChatOff
            | Refusal::SpaceUnbound(_)
            | Refusal::AppMismatch(_)
            | Refusal::TypeMismatch { .. }
            | Refusal::Sew(_) => 409,
            Refusal::RateLimited { .. } => 429,
            Refusal::Store(_) => 500,
            Refusal::Unreachable(_) => 503,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Refusal::Usage(_) => "usage",
            Refusal::FeatureMissing => "sew_feature_missing",
            Refusal::ChatOff | Refusal::Unreachable(_) => "chat_unavailable",
            Refusal::SpaceUnbound(_) => "space_unbound",
            Refusal::AppMismatch(_) => "app_mismatch",
            Refusal::NotFound(_) => "not_found",
            Refusal::TypeMismatch { .. } => "channel_type_mismatch",
            Refusal::Sew(_) | Refusal::Store(_) => "failed",
            Refusal::RateLimited { .. } => "rate_limited",
        }
    }

    pub fn hint(&self) -> String {
        match self {
            Refusal::Usage(m)
            | Refusal::SpaceUnbound(m)
            | Refusal::AppMismatch(m)
            | Refusal::NotFound(m)
            | Refusal::Sew(m)
            | Refusal::Store(m) => m.clone(),
            Refusal::TypeMismatch { slug, existing_type } => format!(
                "{slug} is already a {} channel in this Space, and chat.createChannel makes text channels: use another slug (nothing changed)",
                if existing_type.is_empty() { "non-text" } else { existing_type.as_str() }
            ),
            Refusal::FeatureMissing => NEEDS_SEW.into(),
            Refusal::ChatOff => "Chat / Voice Hosting is not enabled on this server: the owner runs k2 hostchat enable".into(),
            Refusal::Unreachable(m) => format!("Sew didn't answer ({m}); retry in a moment (k2 hostchat status)"),
            Refusal::RateLimited { retry_after_ms } => {
                format!("at most {RATE_PER_MIN} channel calls a minute per App; retry after {retry_after_ms} ms")
            }
        }
    }
}

impl From<String> for Refusal {
    fn from(e: String) -> Self {
        Refusal::Store(e)
    }
}

/// What the App asked for (`chat.createChannel`'s args).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub space: Option<String>,
    pub slug: String,
    pub name: String,
    pub private: bool,
    /// Usernames, de-duplicated (case-insensitive), in the order given.
    pub members: Vec<String>,
}

impl Ask {
    /// Read and check the args: `{space?, slug, name, private?, members?}`.
    pub fn parse(v: &serde_json::Value) -> Result<Ask, Refusal> {
        let Some(o) = v.as_object() else {
            return Err(Refusal::Usage("the body is a JSON object {space?, slug, name, private?, members?}".into()));
        };
        if let Some(k) = o.keys().find(|k| !["space", "slug", "name", "private", "members"].contains(&k.as_str())) {
            return Err(Refusal::Usage(format!("unknown field '{k}' (space, slug, name, private, members)")));
        }
        let space = match o.get("space") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) if s.trim().is_empty() => None,
            Some(serde_json::Value::String(s)) if sew_wire::is_sew_id(s.trim()) => Some(s.trim().to_string()),
            Some(_) => return Err(Refusal::Usage("space is a Sew Space id (k2 hostchat app-space list), or absent for the App's own".into())),
        };
        let slug = o.get("slug").and_then(|s| s.as_str()).map(str::trim).unwrap_or("");
        if !sew_wire::is_channel_slug(slug) {
            return Err(Refusal::Usage(format!(
                "slug '{slug}' is not a channel slug: 1-80 of a-z 0-9 and -, starting with a letter or digit (e.g. job-1042)"
            )));
        }
        let name = o.get("name").and_then(|s| s.as_str()).map(str::trim).unwrap_or("");
        let n = name.chars().count();
        if n == 0 || n > NAME_MAX_CHARS || name.chars().any(char::is_control) {
            return Err(Refusal::Usage(format!("name is 1-{NAME_MAX_CHARS} characters, no control characters")));
        }
        let private = match o.get("private") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err(Refusal::Usage("private is true or false".into())),
        };
        let mut members: Vec<String> = Vec::new();
        match o.get("members") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(a)) => {
                for m in a {
                    let Some(u) = m.as_str().map(str::trim).filter(|u| !u.is_empty()) else {
                        return Err(Refusal::Usage("members is a list of this App's usernames".into()));
                    };
                    if !members.iter().any(|x| x.eq_ignore_ascii_case(u)) {
                        members.push(u.to_string());
                    }
                }
            }
            Some(_) => return Err(Refusal::Usage("members is a list of this App's usernames".into())),
        }
        if members.len() > MEMBERS_MAX {
            return Err(Refusal::Usage(format!("at most {MEMBERS_MAX} members in one call")));
        }
        Ok(Ask { space, slug: slug.to_string(), name: name.to_string(), private, members })
    }
}

/// A listed user who wasn't put in the channel, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotAdded {
    pub username: String,
    pub reason: String,
}

/// One listed user K2 may put in the Space.
#[derive(Debug, Clone)]
struct Listed {
    username: String,
    want: Wanted,
}

/// The state between the steps (built by [`prepare`]).
#[derive(Debug, Clone)]
pub struct Prep {
    pub app: String,
    pub space_id: String,
    pub ask: Ask,
    /// Listed users to add to the Space first (not yet in it).
    to_add: Vec<Listed>,
    /// Listed users the Space holds (ledger, or Sew's members/add answer).
    in_space: Vec<Listed>,
    pub not_added: Vec<NotAdded>,
    /// Listed users banned in the Space (never sent).
    pub skipped: Vec<String>,
    /// Every listed user K2 sent or will send, external id → username.
    names: HashMap<String, String>,
}

impl Prep {
    /// The `spaces/members/add` body, or `None` when nobody needs adding.
    pub fn space_add(&self) -> Option<sew_wire::SpaceMembersAddRequest> {
        (!self.to_add.is_empty()).then(|| sew_wire::SpaceMembersAddRequest {
            space_id: self.space_id.clone(),
            app_id: Some(self.app.clone()),
            external_ids: self.to_add.iter().map(|l| l.want.ext.clone()).collect(),
        })
    }

    /// The `channels/create` body: the listed users now in the Space, in
    /// the order the App listed them.
    pub fn channel_create(&self) -> sew_wire::ChannelCreateRequest {
        let order: HashMap<&str, usize> = self.ask.members.iter().enumerate().map(|(i, u)| (u.as_str(), i)).collect();
        let mut ins: Vec<&Listed> = self.in_space.iter().collect();
        ins.sort_by_key(|l| order.get(l.username.as_str()).copied().unwrap_or(usize::MAX));
        sew_wire::ChannelCreateRequest {
            space_id: self.space_id.clone(),
            app_id: self.app.clone(),
            slug: self.ask.slug.clone(),
            name: self.ask.name.clone(),
            private: self.ask.private,
            members: ins.iter().map(|l| l.want.ext.to_string()).collect(),
        }
    }
}

/// `chat.createChannel`'s answer (the catalog's `AppChatChannel`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub ok: bool,
    pub app: String,
    pub space_id: String,
    pub channel_id: String,
    pub slug: String,
    /// The channel's current name (a repeat never changes it).
    pub name: String,
    /// Whether it is private (current; a repeat never changes it).
    pub private: bool,
    /// `false` = it was there already (same Space and slug).
    pub created: bool,
    /// Listed users in the channel now (added by this call or already in).
    pub added: Vec<String>,
    pub not_added: Vec<NotAdded>,
    /// Listed users banned in the Space: K2 never adds them again.
    pub skipped: Vec<String>,
}

fn rate_take(app: &str) -> Result<(), Refusal> {
    super::guest_rooms::rate_take(&format!("chat-channels:{}", app.trim())).map_err(|e| match e {
        super::guest_rooms::Refusal::RateLimited { retry_after_ms } => Refusal::RateLimited { retry_after_ms },
        other => Refusal::Store(other.hint()),
    })
}

/// Step 1: gates, the Space and the listed users. Counts against the rate.
pub fn prepare(app: &str, ask: Ask) -> Result<Prep, Refusal> {
    let app = app.trim().to_string();
    if app.is_empty() || app == HOST_APP_ID {
        return Err(Refusal::SpaceUnbound("the box login is not an App with its own Space".into()));
    }
    rate_take(&app)?;
    if !crate::tool_hook::sew_has(features::APP_CHANNEL_CREATE)? {
        return Err(Refusal::FeatureMissing);
    }
    if !crate::tool_hook::get_target(CHAT_KIND)?.is_some_and(|t| t.enabled) {
        return Err(Refusal::ChatOff);
    }
    // App-governed Spaces S0: names are never identities. Only this App's
    // principals are looked up below, so a Connect login (the owner
    // included) is never reached; an App user who shares a box login's
    // name is its own account (`k2:principal:<id>`).
    let agent_logins: HashSet<String> = {
        let mut out = HashSet::new();
        let ids: Vec<(String, String)> = with_conn(|conn| {
            let mut v = Vec::new();
            for u in &ask.members {
                if let Ok(n) = normalize_username(u) {
                    if let Some(p) = principal_by_username(conn, &n)? {
                        v.push((n, p.id));
                    }
                }
            }
            Ok(v)
        })?;
        for (n, id) in ids {
            if super::principal_agent_workspace(&id)?.is_some() {
                out.insert(n);
            }
        }
        out
    };
    with_conn(|conn| Ok(build(conn, &app, &ask, &agent_logins)))?
}

fn build(
    conn: &rusqlite::Connection,
    app: &str,
    ask: &Ask,
    agent_logins: &HashSet<String>,
) -> Result<Prep, Refusal> {
    let space_id = pick_space(conn, app, ask.space.as_deref())?;
    let desired: HashMap<String, Wanted> = app_space::build_desired(conn)?
        .into_iter()
        .filter(|w| w.app_id == app && w.space_id == space_id)
        .map(|w| (w.ext.to_string(), w))
        .collect();
    let mut prep = Prep {
        app: app.to_string(),
        space_id: space_id.clone(),
        ask: ask.clone(),
        to_add: Vec::new(),
        in_space: Vec::new(),
        not_added: Vec::new(),
        skipped: Vec::new(),
        names: HashMap::new(),
    };
    let mut seen: Vec<String> = Vec::new();
    for raw in &ask.members {
        let Ok(user) = normalize_username(raw) else {
            prep.not_added.push(NotAdded { username: raw.clone(), reason: NOT_FOUND.into() });
            seen.push(raw.clone());
            continue;
        };
        if seen.contains(&user) {
            continue;
        }
        seen.push(user.clone());
        let not = |reason: &str| NotAdded { username: user.clone(), reason: reason.to_string() };
        if agent_logins.contains(&user) {
            prep.not_added.push(not(NOT_FOUND));
            continue;
        }
        let Some(p) = principal_by_username(conn, &user)? else {
            prep.not_added.push(not(NOT_FOUND));
            continue;
        };
        if app_grant(conn, &p.id, app)?.is_none() {
            prep.not_added.push(not(NOT_FOUND));
            continue;
        }
        let ext = ExternalId::K2Principal(p.id.clone()).to_string();
        let Some(want) = desired.get(&ext).cloned() else {
            let guest = crate::tool_hook::chat::person_grant(conn, &p.id)?
                .is_some_and(|g| g.enabled && g.scope.as_deref() == Some("guest"));
            prep.not_added.push(not(if guest { GUEST } else { NO_CHAT }));
            continue;
        };
        let l = Listed { username: user.clone(), want };
        match app_space::ledger_state(conn, &space_id, &ext)? {
            Some((_, true)) => prep.skipped.push(user.clone()),
            Some((state, false)) if state == app_space::ADDED => {
                prep.names.insert(ext, user.clone());
                prep.in_space.push(l);
            }
            _ => {
                prep.names.insert(ext, user.clone());
                prep.to_add.push(l);
            }
        }
    }
    // Answers name users as normalized, in the App's order.
    prep.ask.members = seen;
    Ok(prep)
}

fn pick_space(conn: &rusqlite::Connection, app: &str, named: Option<&str>) -> Result<String, Refusal> {
    let mine: Vec<String> =
        app_space::bindings(conn)?.into_iter().filter(|b| b.app_id == app).map(|b| b.space_id).collect();
    match named {
        Some(s) if mine.iter().any(|m| m == s) => Ok(s.to_string()),
        Some(s) => match app_space::bound_app(conn, s)? {
            Some(_) => Err(Refusal::AppMismatch(format!("Space {s} belongs to another App; an App makes channels in its own Space only"))),
            None => Err(Refusal::SpaceUnbound(format!(
                "Space {s} isn't bound to this App (the owner or an agent with Manage Chat / Voice Hosting binds it: k2 hostchat app-space bind <app> <space-id>)"
            ))),
        },
        None => match mine.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(Refusal::SpaceUnbound(
                "this App has no Sew Space yet (k2 chat space create, then k2 hostchat app-space bind <app> <space-id>; k2 study app-chat)".into(),
            )),
            many => Err(Refusal::Usage(format!("this App has {} Spaces: name one (space: {})", many.len(), many.join(", ")))),
        },
    }
}

/// Step 2: Sew answered `spaces/members/add`. `added`/`already` are in the
/// Space now (the ledger says so); `skipped` (banned) are held for good;
/// `not_added` pass through with Sew's reason.
pub fn after_space_add(prep: &mut Prep, resp: &SpaceMembersAddResponse) -> Result<(), Refusal> {
    let inside: HashSet<String> = resp.added.iter().chain(resp.already.iter()).map(|m| m.external_id.to_string()).collect();
    let refused: HashMap<String, String> =
        resp.not_added.iter().map(|n| (n.external_id.to_string(), n.reason.clone())).collect();
    let now = super::now_secs();
    let sent = std::mem::take(&mut prep.to_add);
    let mut banned: Vec<Listed> = Vec::new();
    for l in sent {
        let ext = l.want.ext.to_string();
        if inside.contains(&ext) {
            prep.in_space.push(l);
        } else if let Some(reason) = refused.get(&ext) {
            prep.not_added.push(NotAdded { username: l.username.clone(), reason: reason.clone() });
        } else if !resp.skipped.is_empty() {
            banned.push(l);
        } else {
            prep.not_added.push(NotAdded { username: l.username.clone(), reason: "not_listed_by_sew".into() });
        }
    }
    with_conn(|conn| {
        for l in &prep.in_space {
            if inside.contains(&l.want.ext.to_string()) {
                app_space::note_direct_add(conn, &l.want, true, now)?;
            }
        }
        for l in &banned {
            app_space::note_direct_add(conn, &l.want, false, now)?;
        }
        Ok(())
    })?;
    prep.skipped.extend(banned.into_iter().map(|l| l.username));
    Ok(())
}

/// Step 3: Sew made (or found) the channel. Maps its member lists back to
/// usernames and writes the audit row.
pub fn finish(prep: Prep, resp: ChannelCreateResponse, actor: &str, via: &str) -> Result<Created, Refusal> {
    let name_of = |m: &str| prep.names.get(m).cloned();
    let mut added: Vec<String> = Vec::new();
    for m in resp.added.iter().chain(resp.already.iter()) {
        if let Some(u) = name_of(&m.external_id.to_string()) {
            if !added.contains(&u) {
                added.push(u);
            }
        }
    }
    let mut not_added = prep.not_added.clone();
    let mut listed: HashSet<String> = added.iter().cloned().collect();
    for n in &resp.not_added {
        if let Some(u) = name_of(&n.member) {
            listed.insert(u.clone());
            not_added.push(NotAdded { username: u, reason: n.reason.clone() });
        }
    }
    // Sew's `skipped` are member ids: the sent users it named nowhere else.
    let mut skipped = prep.skipped.clone();
    if !resp.skipped.is_empty() {
        for m in prep.channel_create().members {
            if let Some(u) = name_of(&m) {
                if !listed.contains(&u) && !skipped.contains(&u) {
                    skipped.push(u);
                }
            }
        }
    }
    // Keep the App's order.
    let order: HashMap<&str, usize> = prep.ask.members.iter().enumerate().map(|(i, u)| (u.as_str(), i)).collect();
    let pos = |u: &str| order.get(u).copied().unwrap_or(usize::MAX);
    added.sort_by_key(|u| pos(u));
    skipped.sort_by_key(|u| pos(u));
    let out = Created {
        ok: true,
        app: prep.app.clone(),
        space_id: resp.space_id.clone(),
        channel_id: resp.channel_id.clone(),
        slug: resp.slug.clone(),
        name: resp.name.clone(),
        private: resp.private,
        created: resp.created,
        added,
        not_added,
        skipped,
    };
    audit(
        &prep.app,
        actor,
        json!({
            "spaceId": out.space_id,
            "channelId": out.channel_id,
            "slug": out.slug,
            "private": out.private,
            "created": out.created,
            "added": out.added.len(),
            "notAdded": out.not_added.len(),
            "skipped": out.skipped.len(),
            "via": via,
        }),
    );
    Ok(out)
}

fn audit(service_id: &str, actor: &str, mut payload: serde_json::Value) {
    payload["service"] = json!(service_id);
    let subject = super::guest_rooms::audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    if let Err(e) = crate::deploy::audit::append(&conn, crate::deploy::audit::SUBJECT_APP, &subject, AUDIT_EVENT, actor, None, &payload) {
        crate::log_debug!("[skin] chat channel audit {service_id}: {e}");
    }
}

/// The App's channel audit rows, newest first (tests, `k2 app history`).
pub fn audit_rows(service_id: &str, limit: usize) -> Result<Vec<crate::deploy::audit::AuditRow>, String> {
    let subject = super::guest_rooms::audit_subject(service_id);
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = crate::deploy::audit::list_for(&conn, crate::deploy::audit::SUBJECT_APP, &subject, limit.max(1) * 4)
        .map_err(|e| format!("audit read: {e}"))?;
    Ok(rows
        .into_iter()
        .filter(|r| r.event == AUDIT_EVENT)
        .filter(|r| r.payload.get("service").and_then(|s| s.as_str()) == Some(service_id))
        .take(limit)
        .collect())
}

#[cfg(test)]
#[path = "skin_app_channels_tests.rs"]
mod tests;
