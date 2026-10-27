//! The delivery hub's verbs (S31 K24).
//!
//! **Agent verbs** (`/cli/sew/chat/*`, passports only, act-as the agent):
//! - `unread` — Sew's read states for the channels the agent may use
//!   (Spaces it has access in; DMs under the default Space's access).
//!   Reading never marks anything read.
//! - `mark-read` — one channel, or `all` of them. The only verb that
//!   changes read state (alias `ack` in the CLI).
//! - `follow` / `unfollow` — a channel (`<space>/<channel>`: Sew notify
//!   level `all`, so its new messages become deliveries; `batch`
//!   overrides the prefs' window for it) or a thread (`msg_…`).
//! - `prefs` / `prefs/set` — the agent's OWN delivery prefs: quiet hours,
//!   their timezone, a batch window. Setting them is self-quieting, never
//!   loosening, so no Manage Sew is needed (plan §1 S31 amend 4).
//! - `wait` without an address lives in `routes::wait` (it hands off to
//!   the hub's waiters).
//!
//! **Admin** (`/cli/sew/admin/deliveries*`, the Manage Sew gate of
//! `routes_admin`): every chat workspace's hub state and prefs; set or
//! reset them for a workspace. Each change writes a `sew_chat_audit` row.
//!
//! Nothing here stores chat text: prefs, follow addresses and counts.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::sew_chat::hub::{self as core_hub, Follow, Prefs};

use super::routes::{
    access_for, backend, caller, internal, need_any_access, now, param, parse_body, resolve_channel, sew_err, usage, Caller,
    ChatErr, R,
};
use super::routes_admin::{admin, audit, Admin};
use super::sew::{self, Channel};
use crate::compute_routes::Who;

/// The routes this module serves.
pub const ROUTES: &[&str] = &[
    "/cli/sew/admin/deliveries",
    "/cli/sew/admin/deliveries/reset",
    "/cli/sew/admin/deliveries/set",
    "/cli/sew/chat/follow",
    "/cli/sew/chat/mark-read",
    "/cli/sew/chat/prefs",
    "/cli/sew/chat/prefs/set",
    "/cli/sew/chat/unfollow",
    "/cli/sew/chat/unread",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/sew/admin/deliveries/reset",
    "/cli/sew/admin/deliveries/set",
    "/cli/sew/chat/follow",
    "/cli/sew/chat/mark-read",
    "/cli/sew/chat/prefs/set",
    "/cli/sew/chat/unfollow",
];

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    if path.starts_with("/cli/sew/admin/") {
        let a = admin(who)?;
        return match path {
            "/cli/sew/admin/deliveries" => admin_list(params),
            "/cli/sew/admin/deliveries/set" => admin_set(&a, body),
            "/cli/sew/admin/deliveries/reset" => admin_reset(&a, body),
            _ => Err(ChatErr::new("not_found", 404, "no such route")),
        };
    }
    let c = caller(who)?;
    match path {
        "/cli/sew/chat/unread" => unread(&c, params),
        "/cli/sew/chat/mark-read" => mark_read(&c, body),
        "/cli/sew/chat/follow" => follow(&c, body, true),
        "/cli/sew/chat/unfollow" => follow(&c, body, false),
        "/cli/sew/chat/prefs" => prefs(&c),
        "/cli/sew/chat/prefs/set" => prefs_set(&c, body),
        _ => Err(ChatErr::new("not_found", 404, "no such chat route")),
    }
}

// ── unread / mark-read ───────────────────────────────────────────────

/// The channels the agent may use: those in Spaces it has access in, and
/// DMs when it has access in the default Space.
fn usable_channels(c: &Caller) -> R<Vec<Channel>> {
    let sew = backend()?;
    let rows = need_any_access(c)?;
    let a = c.act_as(None);
    let default = sew.default_space(&a).map_err(sew_err)?.map(|s| s.id);
    let dms_ok = default.as_ref().is_some_and(|d| rows.iter().any(|r| &r.space_id == d));
    Ok(sew
        .channels(&a, None)
        .map_err(sew_err)?
        .into_iter()
        .filter(|ch| match &ch.space_id {
            Some(s) => rows.iter().any(|r| &r.space_id == s),
            None => dms_ok,
        })
        .collect())
}

fn unread(c: &Caller, params: &HashMap<String, String>) -> R<Value> {
    let all = matches!(param(params, "all"), Some("1" | "true" | "yes"));
    let channels = usable_channels(c)?;
    let sew = backend()?;
    let states = sew.read_states(&c.act_as(None)).map_err(sew_err)?;
    let mut out = Vec::new();
    let (mut n_unread, mut n_mentions) = (0u32, 0u32);
    for ch in &channels {
        let Some(st) = states.iter().find(|s| s.channel_id == ch.id) else { continue };
        if !all && !st.unread && st.mentions == 0 {
            continue;
        }
        if st.unread {
            n_unread += 1;
        }
        n_mentions += st.mentions;
        out.push(json!({
            "addr": sew::channel_addr(ch),
            "channelId": ch.id,
            "kind": ch.kind.as_str(),
            "unread": st.unread,
            "mentions": st.mentions,
            "muted": st.muted,
            "lastReadId": st.last_read_id.as_deref().map(sew::wire_msg_id),
            "lastMessageId": st.last_message_id.as_deref().map(sew::wire_msg_id),
        }));
    }
    Ok(json!({ "ok": true, "channels": out, "unreadChannels": n_unread, "mentions": n_mentions }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MarkBody {
    #[serde(default)]
    r#in: Option<String>,
    #[serde(default)]
    all: bool,
}

fn mark_read(c: &Caller, body: &[u8]) -> R<Value> {
    let b: MarkBody = parse_body(body)?;
    let sew = backend()?;
    let a = c.act_as(None);
    let ids: Vec<(String, String)> = match (b.r#in.as_deref().map(str::trim).filter(|s| !s.is_empty()), b.all) {
        (Some(_), true) | (None, false) => {
            return Err(usage("name one channel (in=<space>/<channel>) or all=true"));
        }
        (Some(raw), false) => {
            need_any_access(c)?;
            let ch = resolve_channel(sew.as_ref(), &a, raw)?;
            access_for(c, sew.as_ref(), &a, &ch)?;
            vec![(ch.id.clone(), sew::channel_addr(&ch))]
        }
        (None, true) => {
            let channels = usable_channels(c)?;
            let states = sew.read_states(&a).map_err(sew_err)?;
            channels
                .iter()
                .filter(|ch| states.iter().any(|s| s.channel_id == ch.id && (s.unread || s.mentions > 0)))
                .map(|ch| (ch.id.clone(), sew::channel_addr(ch)))
                .collect()
        }
    };
    let only: Vec<String> = ids.iter().map(|(i, _)| i.clone()).collect();
    sew.mark_read(&a, &only).map_err(sew_err)?;
    Ok(json!({ "ok": true, "marked": ids.iter().map(|(_, a)| a.clone()).collect::<Vec<_>>() }))
}

// ── follow ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FollowBody {
    /// `<space>/<channel>` or `msg_…` (a thread).
    to: String,
    #[serde(default)]
    batch: Option<String>,
}

fn follow(c: &Caller, body: &[u8], on: bool) -> R<Value> {
    let b: FollowBody = parse_body(body)?;
    let sew = backend()?;
    need_any_access(c)?;
    let a = c.act_as(None);
    let to = b.to.trim();
    if to.starts_with("msg_") || (!to.is_empty() && to.bytes().all(|b| b.is_ascii_digit())) {
        if b.batch.is_some() {
            return Err(usage("--batch is for a channel follow (<space>/<channel>); a thread follow uses your prefs"));
        }
        let id = sew::sew_msg_id(to).ok_or_else(|| usage("a thread is msg_…"))?;
        let m = sew.message(&a, &id).map_err(sew_err)?;
        let ch = sew.channel(&a, &m.channel_id).map_err(sew_err)?;
        access_for(c, sew.as_ref(), &a, &ch)?;
        let root = m.reply_to.clone().unwrap_or(m.id.clone());
        sew.follow_thread(&a, &root, on).map_err(sew_err)?;
        return Ok(json!({ "ok": true, "thread": sew::wire_msg_id(&root), "addr": sew::channel_addr(&ch), "following": on }));
    }
    let batch = match b.batch.as_deref() {
        Some(x) if on => Some(core_hub::parse_batch(x).map_err(usage)?),
        Some(_) => return Err(usage("--batch goes with follow, not unfollow")),
        None => None,
    };
    let ch = resolve_channel(sew.as_ref(), &a, to)?;
    if ch.kind.is_dm() {
        return Err(usage("DMs always reach you; follow a Space channel (<space>/<channel>) or a thread (msg_…)"));
    }
    access_for(c, sew.as_ref(), &a, &ch)?;
    sew.follow_channel(&a, &ch.id, on).map_err(sew_err)?;
    let addr = sew::channel_addr(&ch);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if on {
            let f = Follow { channel_id: ch.id.clone(), addr: addr.clone(), batch_secs: batch };
            core_hub::set_follow(&conn, &c.project_id, &f, &format!("agent:{}", c.handle), now()).map_err(internal)?;
        } else {
            core_hub::remove_follow(&conn, &c.project_id, &ch.id).map_err(internal)?;
        }
    }
    let mut v = json!({ "ok": true, "addr": addr, "following": on });
    if let Some(s) = batch {
        v["batch"] = core_hub::duration_text(s).into();
    }
    Ok(v)
}

// ── prefs (the agent's own) ──────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PrefsBody {
    #[serde(default)]
    quiet_hours: Option<String>,
    #[serde(default)]
    tz: Option<String>,
    #[serde(default)]
    batch: Option<String>,
}

impl PrefsBody {
    fn is_empty(&self) -> bool {
        self.quiet_hours.is_none() && self.tz.is_none() && self.batch.is_none()
    }

    /// `old` with this body's changes.
    fn apply(&self, old: &Prefs) -> R<Prefs> {
        let mut p = old.clone();
        if let Some(q) = &self.quiet_hours {
            p.quiet = core_hub::parse_quiet(q).map_err(usage)?;
        }
        if let Some(t) = &self.tz {
            p.tz = core_hub::parse_tz(t).map_err(usage)?;
        }
        if let Some(b) = &self.batch {
            p.batch_secs = core_hub::parse_batch(b).map_err(usage)?;
        }
        Ok(p)
    }
}

/// One workspace's hub view: prefs, follows, the poller's state.
fn hub_view(project_id: &str) -> R<Value> {
    let (prefs, follows) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            core_hub::load_prefs(&conn, project_id).map_err(internal)?,
            core_hub::list_follows(&conn, project_id).map_err(internal)?,
        )
    };
    let quiet_until = core_hub::quiet_until(&prefs, now());
    Ok(json!({
        "prefs": prefs.to_json(),
        "quietNow": quiet_until.is_some(),
        "quietUntil": quiet_until,
        "follows": follows.iter().map(|f| json!({
            "addr": f.addr, "channelId": f.channel_id, "batch": f.batch_secs.map(core_hub::duration_text),
        })).collect::<Vec<_>>(),
        "hub": super::hub::status_of(project_id).unwrap_or_default(),
    }))
}

fn prefs(c: &Caller) -> R<Value> {
    need_any_access(c)?;
    let mut v = hub_view(&c.project_id)?;
    v["ok"] = true.into();
    v["workspace"] = c.handle.clone().into();
    Ok(v)
}

fn prefs_set(c: &Caller, body: &[u8]) -> R<Value> {
    let b: PrefsBody = parse_body(body)?;
    if b.is_empty() {
        return Err(usage("set --quiet-hours, --tz and/or --batch"));
    }
    need_any_access(c)?;
    let new = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let old = core_hub::load_prefs(&conn, &c.project_id).map_err(internal)?;
        let new = b.apply(&old)?;
        core_hub::save_prefs(&conn, &c.project_id, &new, &format!("agent:{}", c.handle), now()).map_err(internal)?;
        new
    };
    Ok(json!({ "ok": true, "workspace": c.handle, "prefs": new.to_json() }))
}

// ── admin ────────────────────────────────────────────────────────────

fn workspace(token: &str) -> R<crate::compute_routes::Ws> {
    crate::compute_routes::ws_by_token(token)
        .ok_or_else(|| ChatErr::new("not_found", 404, format!("no workspace '{}' on this box", token.trim())))
}

fn admin_list(params: &HashMap<String, String>) -> R<Value> {
    let ids = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core_hub::hub_workspaces(&conn).map_err(internal)?
    };
    let only = match param(params, "workspace") {
        Some(t) => Some(workspace(t)?.id),
        None => None,
    };
    let mut out = Vec::new();
    for id in ids.iter().filter(|i| only.as_deref().is_none_or(|o| o == i.as_str())) {
        let mut v = hub_view(id)?;
        v["workspace"] = crate::compute_routes::ws_handle(id).into();
        v["workspaceId"] = id.clone().into();
        out.push(v);
    }
    if only.is_some() && out.is_empty() {
        return Err(ChatErr::new("not_found", 404, "that workspace has no chat access, so no deliveries"));
    }
    Ok(json!({ "ok": true, "deliveries": out }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AdminSetBody {
    workspace: String,
    #[serde(default)]
    quiet_hours: Option<String>,
    #[serde(default)]
    tz: Option<String>,
    #[serde(default)]
    batch: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WsBody {
    workspace: String,
}

fn admin_set(a: &Admin, body: &[u8]) -> R<Value> {
    let b: AdminSetBody = parse_body(body)?;
    let p = PrefsBody { quiet_hours: b.quiet_hours, tz: b.tz, batch: b.batch };
    if p.is_empty() {
        return Err(usage("set --quiet-hours, --tz and/or --batch"));
    }
    let ws = workspace(&b.workspace)?;
    let new = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let old = core_hub::load_prefs(&conn, &ws.id).map_err(internal)?;
        let new = p.apply(&old)?;
        core_hub::save_prefs(&conn, &ws.id, &new, a.label(), now()).map_err(internal)?;
        new
    };
    audit(a, Some(&ws.id), "deliveries.set", None, None, Some(new.to_json().to_string()), "ok", None)?;
    Ok(json!({ "ok": true, "workspace": ws.handle, "prefs": new.to_json() }))
}

fn admin_reset(a: &Admin, body: &[u8]) -> R<Value> {
    let b: WsBody = parse_body(body)?;
    let ws = workspace(&b.workspace)?;
    let cleared = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        core_hub::save_prefs(&conn, &ws.id, &Prefs::default(), a.label(), now()).map_err(internal)?;
        core_hub::clear_follow_batches(&conn, &ws.id, a.label(), now()).map_err(internal)?
    };
    audit(a, Some(&ws.id), "deliveries.reset", None, None, Some(format!("follow batches cleared: {cleared}")), "ok", None)?;
    Ok(json!({ "ok": true, "workspace": ws.handle, "prefs": Prefs::default().to_json(), "followBatchesCleared": cleared }))
}
