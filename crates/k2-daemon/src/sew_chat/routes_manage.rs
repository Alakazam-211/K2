//! `/cli/sew/chat/manage/*` — community management through act-as (S30
//! K20): channels, channel permissions, Space members, roles, bans,
//! timeouts, and new Spaces.
//!
//! **Two K2 gates, then Sew's.** The caller is an agent (or K2 shell-tab)
//! passport whose workspace has "Manage Sew" (`chat_manage`) on AND level
//! `manage` in that Space (`sew_chat_access`). Then Sew does the call AS
//! the agent, so Sew S12 enforces its own ladder: MANAGE_* permissions,
//! no ADMINISTRATOR for agents, masked 404s for what the agent can't see.
//!
//! **New Spaces** (`space/create`) go through Sew's mgmt `spaces/create`
//! (SW11): the owner is the box owner (`k2:owner`) or a named person,
//! never an agent; the acting agent becomes a manager. Needs Manage Sew
//! only (there is no Space yet). K2 then gives the creator level `manage`
//! there, with approvals required if any of its existing Spaces requires
//! them (builder decision, provisional).
//!
//! An agent never adds a role to itself here (raising its own Sew role).
//! Every step writes a `sew_chat_audit` row.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::sew_chat::{self as core, store, ApprovalMode, Level};

use super::routes::{backend, caller, internal, now, parse_body, resolve_channel, sew_err, usage, Caller, ChatErr, R};
use super::routes_admin::{audit, Admin};
use super::sew::{self, ActAs, ChannelPerm, MemberLookup, MemberRef, NewSpace, Overwrite};
use crate::compute_routes::Who;

pub const ROUTES: &[&str] = &[
    "/cli/sew/chat/manage/ban",
    "/cli/sew/chat/manage/channel/create",
    "/cli/sew/chat/manage/channel/perms",
    "/cli/sew/chat/manage/channel/set",
    "/cli/sew/chat/manage/role/add",
    "/cli/sew/chat/manage/role/create",
    "/cli/sew/chat/manage/role/remove",
    "/cli/sew/chat/manage/space/create",
    "/cli/sew/chat/manage/space/members",
    "/cli/sew/chat/manage/space/members/add",
    "/cli/sew/chat/manage/space/members/remove",
    "/cli/sew/chat/manage/timeout",
    "/cli/sew/chat/manage/unban",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/sew/chat/manage/ban",
    "/cli/sew/chat/manage/channel/create",
    "/cli/sew/chat/manage/channel/perms",
    "/cli/sew/chat/manage/channel/set",
    "/cli/sew/chat/manage/role/add",
    "/cli/sew/chat/manage/role/create",
    "/cli/sew/chat/manage/role/remove",
    "/cli/sew/chat/manage/space/create",
    "/cli/sew/chat/manage/space/members/add",
    "/cli/sew/chat/manage/space/members/remove",
    "/cli/sew/chat/manage/timeout",
    "/cli/sew/chat/manage/unban",
];

/// Longest timeout (Sew's ceiling, like Discord's).
pub const TIMEOUT_MAX_SECS: i64 = 28 * 86_400;

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    let c = caller(who)?;
    let m = manager(&c)?;
    match path {
        "/cli/sew/chat/manage/space/create" => space_create(&c, &m, body),
        "/cli/sew/chat/manage/space/members" => members_list(&c, &m, params),
        "/cli/sew/chat/manage/space/members/add" => members_change(&c, &m, body, true),
        "/cli/sew/chat/manage/space/members/remove" => members_change(&c, &m, body, false),
        "/cli/sew/chat/manage/channel/create" => channel_create(&c, &m, body),
        "/cli/sew/chat/manage/channel/set" => channel_set(&c, &m, body),
        "/cli/sew/chat/manage/channel/perms" => channel_perms(&c, &m, body),
        "/cli/sew/chat/manage/role/create" => role_create(&c, &m, body),
        "/cli/sew/chat/manage/role/add" => role_member(&c, &m, body, true),
        "/cli/sew/chat/manage/role/remove" => role_member(&c, &m, body, false),
        "/cli/sew/chat/manage/ban" => ban(&c, &m, body),
        "/cli/sew/chat/manage/unban" => unban(&c, &m, body),
        "/cli/sew/chat/manage/timeout" => timeout(&c, &m, body),
        _ => Err(ChatErr::new("not_found", 404, "no such manage route")),
    }
}

/// Gate 1: Manage Sew on the caller's workspace.
fn manager(c: &Caller) -> R<Admin> {
    if !k2_core::workspace::settings::agents_can_manage_chat(&c.project_id) {
        return Err(ChatErr::new(
            "chat_manage_off",
            403,
            "community management needs the Manage Chat / Voice Hosting switch on your workspace — your owner or an admin turns it on \
             (k2 agent access set <workspace> chat_manage on)",
        ));
    }
    Ok(Admin::Agent { project_id: c.project_id.clone(), label: format!("agent:{}", c.handle) })
}

/// Gate 2: level `manage` in the Space named by `slug`. Returns the row.
fn manage_space(c: &Caller, slug: &str) -> R<store::Access> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_access(&conn, &c.project_id).map_err(internal)?
    };
    let row = rows.into_iter().find(|r| r.space_slug.eq_ignore_ascii_case(slug.trim())).ok_or_else(|| {
        ChatErr::new("not_granted", 403, format!("you have no chat access in '{}' (k2 chat spaces lists yours)", slug.trim()))
    })?;
    if row.level < Level::Manage {
        return Err(ChatErr::new(
            "manage_level_required",
            403,
            format!(
                "your chat level in {} is '{}': managing it needs level manage — ask your human (k2 chat access set)",
                row.space_slug,
                row.level.as_str()
            ),
        )
        .with("level", row.level.as_str()));
    }
    Ok(row)
}

fn act(c: &Caller, m: &Admin) -> ActAs {
    ActAs { actor: Some(m.label().to_string()), ..c.act_as(None) }
}

/// `<user>` / `@user` → a K2 username; a workspace name → its agent.
fn lookup(member: &str) -> R<MemberLookup> {
    let t = member.trim();
    if t.is_empty() || t == "@" {
        return Err(usage("name a member: a K2 username (@user) or a workspace"));
    }
    if let Some(u) = t.strip_prefix('@') {
        return Ok(MemberLookup::Username(u.to_string()));
    }
    Ok(match crate::compute_routes::ws_by_token(t) {
        Some(w) => MemberLookup::External(format!("k2:workspace:{}", w.id.to_ascii_lowercase())),
        None => MemberLookup::Username(t.to_string()),
    })
}

fn member_json(m: &MemberRef) -> Value {
    json!({ "id": m.member_id, "username": m.username, "kind": if m.is_agent { "agent" } else { "person" } })
}

/// Run a Sew call; on refusal or failure record it, then pass Sew's answer
/// through (a 404 stays masked).
#[allow(clippy::too_many_arguments)]
fn sew_step<T>(
    c: &Caller,
    m: &Admin,
    op: &str,
    space_id: Option<&str>,
    target: Option<&str>,
    detail: Option<String>,
    f: impl FnOnce() -> Result<T, sew::SewError>,
) -> R<T> {
    match f() {
        Ok(v) => {
            audit(m, Some(&c.project_id), op, space_id, target, detail, "ok", None)?;
            Ok(v)
        }
        Err(e) => {
            let outcome = if matches!(e.status, 403 | 404 | 409 | 422) { "refused" } else { "failed" };
            audit(m, Some(&c.project_id), op, space_id, target, detail, outcome, Some(&e.code))?;
            Err(sew_err(e))
        }
    }
}

fn manage_key(key: Option<&str>) -> R<String> {
    let k = match key {
        Some(k) => {
            core::check_key(k).map_err(usage)?;
            k.to_string()
        }
        None => core::generated_key(),
    };
    Ok(format!("k2c-{k}"))
}

// ── members ──────────────────────────────────────────────────────────

fn members_list(c: &Caller, m: &Admin, params: &HashMap<String, String>) -> R<Value> {
    let slug = params.get("space").map(|s| s.trim()).filter(|s| !s.is_empty()).ok_or_else(|| usage("name a Space: space=<slug>"))?;
    let sew = backend()?;
    let row = manage_space(c, slug)?;
    let list = sew.space_members(&act(c, m), &row.space_id).map_err(sew_err)?;
    Ok(json!({ "ok": true, "space": row.space_slug, "members": list.iter().map(member_json).collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MemberBody {
    space: String,
    member: String,
}

/// Sew 0.6.0 (Rosson's ruling 1): box users (the owner, Connect logins)
/// are never members of a Space K2 switched (or is switching) to App
/// governance. Sew refuses it too; K2 refuses first.
pub(crate) fn refuse_box_user_in_governed(space_id: &str, member_id: &str) -> R<()> {
    let Some(m) = k2_core::tool_hook::chat_member_by_id(member_id).map_err(internal)? else { return Ok(()) };
    let Ok(ext) = k2_core::tool_hook::sew_wire::ExternalId::parse(&m.external_id) else { return Ok(()) };
    if k2_core::tool_hook::governed_refuses_box_user(space_id, &ext).map_err(internal)? {
        return Err(ChatErr::new(
            "box_user_not_allowed",
            409,
            "this Space is governed by its App: the server's owner and admins aren't members of App Spaces (they run things from K2); add App users through the App",
        ));
    }
    Ok(())
}

fn members_change(c: &Caller, m: &Admin, body: &[u8], add: bool) -> R<Value> {
    let b: MemberBody = parse_body(body)?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let who = sew.member(&a, &lookup(&b.member)?).map_err(sew_err)?;
    if add {
        refuse_box_user_in_governed(&row.space_id, &who.member_id)?;
    }
    let op = if add { "space.members.add" } else { "space.members.remove" };
    sew_step(c, m, op, Some(&row.space_id), Some(&who.username), None, || {
        if add {
            sew.space_member_add(&a, &row.space_id, &who.member_id)
        } else {
            sew.space_member_remove(&a, &row.space_id, &who.member_id)
        }
    })?;
    Ok(json!({ "ok": true, "space": row.space_slug, "member": member_json(&who), "added": add }))
}

// ── channels ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ChannelCreateBody {
    space: String,
    name: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    key: Option<String>,
}

fn channel_out(ch: &sew::Channel) -> Value {
    json!({ "addr": sew::channel_addr(ch), "id": ch.id, "name": ch.name, "private": ch.private })
}

fn channel_create(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: ChannelCreateBody = parse_body(body)?;
    let name = b.name.trim().trim_start_matches('#');
    if name.is_empty() || name.contains('/') || name.chars().count() > 100 {
        return Err(usage("a channel name is 1–100 characters, no '/'"));
    }
    let key = manage_key(b.key.as_deref())?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let ch = sew_step(c, m, "channel.create", Some(&row.space_id), Some(name), Some(format!("private={}", b.private)), || {
        sew.channel_create(&a, &row.space_id, name, b.private, &key)
    })?;
    Ok(json!({ "ok": true, "channel": channel_out(&ch) }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ChannelSetBody {
    to: String,
    private: bool,
}

/// The channel at `to` and the caller's `manage` row for its Space.
fn managed_channel(c: &Caller, a: &ActAs, to: &str) -> R<(sew::Channel, store::Access)> {
    let sew = backend()?;
    let ch = resolve_channel(sew.as_ref(), a, to)?;
    let slug = ch.space_slug.clone().ok_or_else(|| usage("a DM has no channel settings"))?;
    let row = manage_space(c, &slug)?;
    if ch.space_id.as_deref() != Some(row.space_id.as_str()) {
        return Err(ChatErr::new("not_found", 404, "channel not found"));
    }
    Ok((ch, row))
}

fn channel_set(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: ChannelSetBody = parse_body(body)?;
    let sew = backend()?;
    let a = act(c, m);
    let (ch, row) = managed_channel(c, &a, &b.to)?;
    let out = sew_step(c, m, "channel.set", Some(&row.space_id), Some(&sew::channel_addr(&ch)), Some(format!("private={}", b.private)), || {
        sew.channel_set_private(&a, &ch.id, b.private)
    })?;
    Ok(json!({ "ok": true, "channel": channel_out(&out) }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PermsBody {
    to: String,
    role: String,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

fn perms(words: &[String]) -> R<Vec<ChannelPerm>> {
    let mut v = Vec::new();
    for w in words.iter().flat_map(|w| w.split(',')).map(str::trim).filter(|w| !w.is_empty()) {
        let p = ChannelPerm::parse(w).map_err(usage)?;
        if !v.contains(&p) {
            v.push(p);
        }
    }
    Ok(v)
}

fn channel_perms(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: PermsBody = parse_body(body)?;
    let allow = perms(&b.allow)?;
    let deny = perms(&b.deny)?;
    if allow.is_empty() && deny.is_empty() {
        return Err(usage("say what to change: --allow view,send and/or --deny view,send"));
    }
    if allow.iter().any(|p| deny.contains(p)) {
        return Err(usage("a permission can't be both allowed and denied"));
    }
    let sew = backend()?;
    let a = act(c, m);
    let (ch, row) = managed_channel(c, &a, &b.to)?;
    let role_word = b.role.trim().trim_start_matches('@');
    let role_id = if role_word.eq_ignore_ascii_case("everyone") {
        None
    } else {
        let roles = sew.roles(&a, &row.space_id).map_err(sew_err)?;
        Some(
            roles
                .into_iter()
                .find(|r| r.name.eq_ignore_ascii_case(role_word))
                .ok_or_else(|| ChatErr::new("not_found", 404, format!("no role '{role_word}' in {}", row.space_slug)))?
                .id,
        )
    };
    let o = Overwrite { role_id, allow: allow.clone(), deny: deny.clone() };
    let words = |v: &[ChannelPerm]| v.iter().map(|p| p.as_str()).collect::<Vec<_>>();
    let addr = sew::channel_addr(&ch);
    sew_step(
        c,
        m,
        "channel.perms",
        Some(&row.space_id),
        Some(&addr),
        Some(format!("{role_word}: allow {:?} deny {:?}", words(&allow), words(&deny))),
        || sew.channel_perms(&a, &ch.id, &o),
    )?;
    Ok(json!({ "ok": true, "addr": addr, "role": role_word, "allow": words(&allow), "deny": words(&deny) }))
}

// ── roles ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RoleCreateBody {
    space: String,
    role: String,
    #[serde(default)]
    key: Option<String>,
}

fn role_create(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: RoleCreateBody = parse_body(body)?;
    let name = b.role.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("everyone") || name.chars().count() > 100 {
        return Err(usage("a role name is 1–100 characters and not 'everyone'"));
    }
    let key = manage_key(b.key.as_deref())?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let r = sew_step(c, m, "role.create", Some(&row.space_id), Some(name), None, || sew.role_create(&a, &row.space_id, name, &key))?;
    Ok(json!({ "ok": true, "space": row.space_slug, "role": { "id": r.id, "name": r.name } }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RoleMemberBody {
    space: String,
    role: String,
    member: String,
}

fn role_member(c: &Caller, m: &Admin, body: &[u8], add: bool) -> R<Value> {
    let b: RoleMemberBody = parse_body(body)?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let who = sew.member(&a, &lookup(&b.member)?).map_err(sew_err)?;
    let me = sew.me(&a).map_err(sew_err)?;
    if add && who.member_id == me.member_id {
        audit(m, Some(&c.project_id), "role.add", Some(&row.space_id), Some(&who.username), Some(b.role.clone()), "refused", Some("self_loosening"))?;
        return Err(ChatErr::new(
            "self_loosening",
            403,
            "an agent never gives itself a role — ask a person or another manager",
        ));
    }
    let roles = sew.roles(&a, &row.space_id).map_err(sew_err)?;
    let role = roles
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(b.role.trim()))
        .ok_or_else(|| ChatErr::new("not_found", 404, format!("no role '{}' in {}", b.role.trim(), row.space_slug)))?;
    let op = if add { "role.add" } else { "role.remove" };
    sew_step(c, m, op, Some(&row.space_id), Some(&who.username), Some(role.name.clone()), || {
        sew.role_member(&a, &row.space_id, &role.id, &who.member_id, add)
    })?;
    Ok(json!({ "ok": true, "space": row.space_slug, "role": role.name, "member": member_json(&who), "added": add }))
}

// ── bans and timeouts ────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BanBody {
    space: String,
    member: String,
    #[serde(default)]
    reason: Option<String>,
}

fn reason(r: Option<&str>) -> R<Option<String>> {
    match r.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) if r.chars().count() > 500 => Err(usage("a reason is at most 500 characters")),
        r => Ok(r.map(str::to_string)),
    }
}

fn ban(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: BanBody = parse_body(body)?;
    let why = reason(b.reason.as_deref())?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let who = sew.member(&a, &lookup(&b.member)?).map_err(sew_err)?;
    sew_step(c, m, "ban", Some(&row.space_id), Some(&who.username), why.clone(), || sew.ban(&a, &row.space_id, &who.member_id, why.as_deref()))?;
    Ok(json!({ "ok": true, "space": row.space_slug, "member": member_json(&who), "banned": true }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UnbanBody {
    space: String,
    member: String,
}

fn unban(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: UnbanBody = parse_body(body)?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let who = sew.member(&a, &lookup(&b.member)?).map_err(sew_err)?;
    sew_step(c, m, "unban", Some(&row.space_id), Some(&who.username), None, || sew.unban(&a, &row.space_id, &who.member_id))?;
    Ok(json!({ "ok": true, "space": row.space_slug, "member": member_json(&who), "banned": false }))
}

/// `30s`, `10m`, `2h`, `1d` → seconds, 1 s ..= [`TIMEOUT_MAX_SECS`].
pub(crate) fn parse_duration(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let bad = || format!("a duration is like 10m, 2h or 1d (got '{t}')");
    let (num, mult) = match t.char_indices().last() {
        Some((i, 's')) => (&t[..i], 1),
        Some((i, 'm')) => (&t[..i], 60),
        Some((i, 'h')) => (&t[..i], 3600),
        Some((i, 'd')) => (&t[..i], 86_400),
        _ => return Err(bad()),
    };
    let n: i64 = num.parse().map_err(|_| bad())?;
    let secs = n.checked_mul(mult).ok_or_else(bad)?;
    if !(1..=TIMEOUT_MAX_SECS).contains(&secs) {
        return Err(format!("a timeout is 1s to 28d (got '{t}')"));
    }
    Ok(secs)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TimeoutBody {
    space: String,
    member: String,
    #[serde(rename = "for")]
    duration: String,
    #[serde(default)]
    reason: Option<String>,
}

fn timeout(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: TimeoutBody = parse_body(body)?;
    let secs = parse_duration(&b.duration).map_err(usage)?;
    let why = reason(b.reason.as_deref())?;
    let sew = backend()?;
    let row = manage_space(c, &b.space)?;
    let a = act(c, m);
    let who = sew.member(&a, &lookup(&b.member)?).map_err(sew_err)?;
    let until = now() + secs;
    sew_step(c, m, "timeout", Some(&row.space_id), Some(&who.username), Some(format!("{secs}s")), || {
        sew.timeout(&a, &row.space_id, &who.member_id, until, why.as_deref())
    })?;
    Ok(json!({ "ok": true, "space": row.space_slug, "member": member_json(&who), "until": until }))
}

// ── new Spaces (SW11) ────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SpaceCreateBody {
    slug: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    key: Option<String>,
    /// Sew 0.6.0 (§3a): an App-owned Space, owned by the App's service
    /// owner `app:<service id>`, bound to the App in the same step.
    #[serde(default)]
    app: Option<String>,
}

/// `--app`: the App's service id, when the caller may make Spaces for it
/// (an agent: only its own workspace's App). Needs Sew's
/// `app_governed_spaces`.
fn app_for_space(m: &Admin, raw: &str) -> R<String> {
    if !k2_core::tool_hook::sew_has(k2_core::tool_hook::features::APP_GOVERNED_SPACES).map_err(internal)? {
        return Err(ChatErr::new(
            "sew_feature_missing",
            409,
            "this box's Sew can't make App-owned Spaces yet (it needs Sew 0.6.0: app_governed_spaces); leave out --app",
        ));
    }
    let (svc, _name) = crate::sew::home::resolve_app(raw).map_err(|f| ChatErr::new(&f.code, f.status, &f.hint))?;
    if let Admin::Agent { project_id, .. } = m {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let own = k2_core::deploy::store::get_app_by_service(&conn, &svc).map_err(internal)?.is_some_and(|a| a.owner_workspace == *project_id);
        if !own {
            return Err(ChatErr::new("app_not_yours", 403, "an agent makes App Spaces only for its own workspace's App"));
        }
    }
    Ok(svc)
}

/// A Space slug: 2–64 of `a-z 0-9 -`, starting with a letter or digit.
fn check_slug(s: &str) -> Result<(), String> {
    let ok = (2..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(format!("a Space slug is 2–64 of a-z, 0-9 and - (got '{s}')"))
    }
}

/// The Space owner's Sew external id: the box owner unless `--owner`
/// names a person. A workspace (an agent) never owns a Space.
fn owner_external_id(owner: Option<&str>) -> R<(String, String)> {
    let Some(raw) = owner.map(str::trim).filter(|o| !o.is_empty()) else {
        return Ok((core::person::OWNER_EXTERNAL_ID.into(), "owner".into()));
    };
    let u = raw.trim_start_matches('@');
    if !raw.starts_with('@') && crate::compute_routes::ws_by_token(u).is_some() {
        return Err(ChatErr::new("agent_owner", 400, "an agent never owns a Space: --owner names a person (or leave it out for the box owner)"));
    }
    if u.eq_ignore_ascii_case("owner") {
        return Ok((core::person::OWNER_EXTERNAL_ID.into(), "owner".into()));
    }
    if let Some(p) = k2_core::skin::list_principals().map_err(internal)?.into_iter().find(|p| p.username.eq_ignore_ascii_case(u)) {
        return Ok((format!("k2:principal:{}", p.id), p.username));
    }
    if let Some(id) = k2_core::connect_users::user_uuid(u).map_err(internal)? {
        if core::person::login_is_owner(u) {
            return Ok((core::person::OWNER_EXTERNAL_ID.into(), u.to_string()));
        }
        return Ok((format!("k2:connect:{id}"), u.to_string()));
    }
    Err(ChatErr::new("not_found", 404, format!("no person '{u}' on this box")))
}

fn space_create(c: &Caller, m: &Admin, body: &[u8]) -> R<Value> {
    let b: SpaceCreateBody = parse_body(body)?;
    let slug = b.slug.trim().to_ascii_lowercase();
    check_slug(&slug).map_err(usage)?;
    let name = b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&slug).to_string();
    if name.chars().count() > 100 {
        return Err(usage("a Space name is at most 100 characters"));
    }
    let key = manage_key(b.key.as_deref())?;
    let app = match b.app.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        Some(_) if b.owner.as_deref().is_some_and(|o| !o.trim().is_empty()) => {
            return Err(usage("--app and --owner don't go together: an App's Space is owned by the App itself"));
        }
        Some(raw) => Some(app_for_space(m, raw)?),
        None => None,
    };
    let (owner_ext, owner_name) = match &app {
        // Sew 0.6.0 §3a: always explicit, never Sew's default.
        Some(svc) => (format!("app:{svc}"), format!("app:{svc}")),
        None => owner_external_id(b.owner.as_deref())?,
    };
    let sew = backend()?;
    let a = act(c, m);
    let ns = NewSpace { slug: slug.clone(), name, owner_external_id: owner_ext.clone(), idempotency_key: key, app_id: app.clone() };
    let sp = sew_step(c, m, "space.create", None, Some(&slug), Some(format!("owner={owner_ext}")), || sew.space_create(&a, &ns))?;
    if let Some(svc) = &app {
        // Sew wrote the governing link with the create: K2 records the
        // binding (no separate bind call). It starts Sew-governed.
        k2_core::tool_hook::app_space_bind(svc, &sp.id, m.label()).map_err(internal)?;
        crate::sew::s32::emit("app-space");
    }
    // The creator manages what it created; it keeps its strictest
    // approval mode from the Spaces it already has.
    let approval = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let rows = store::list_access(&conn, &c.project_id).map_err(internal)?;
        let approval = if rows.iter().any(|r| r.approval == ApprovalMode::Required) { ApprovalMode::Required } else { ApprovalMode::None };
        store::set_access(
            &conn,
            &store::Access {
                project_id: c.project_id.clone(),
                space_id: sp.id.clone(),
                space_slug: sp.slug.clone(),
                level: Level::Manage,
                approval,
                updated_by: m.label().to_string(),
                updated_at: now(),
            },
        )
        .map_err(internal)?;
        approval
    };
    if let Some(ws) = crate::compute_routes::ws_by_id(&c.project_id) {
        super::routes_admin::teach_refresh(&ws.path);
    }
    Ok(json!({
        "ok": true,
        "space": { "id": sp.id, "slug": sp.slug, "name": sp.name },
        "owner": owner_name,
        "app": app,
        "access": { "level": "manage", "approval": approval.as_str() },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_slugs() {
        assert_eq!(parse_duration("90s").unwrap(), 90);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert_eq!(parse_duration("28d").unwrap(), TIMEOUT_MAX_SECS);
        for bad in ["29d", "0m", "2", "soon", "-1h", ""] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
        assert!(check_slug("eng-team").is_ok());
        for bad in ["E", "a", "-x", "has space", "under_score"] {
            assert!(check_slug(bad).is_err(), "{bad}");
        }
    }
}
