//! Sew chat is for people (prd-home-people-channels-v1 §13.1, §13.2;
//! HP-31, HP-32, HP-34, HP-36): what `k2 chat` refuses an agent in the K5
//! Space and in DMs with this server's people.
//!
//! - **DMs with box people** (`k2:owner`, `k2:connect:`): every agent
//!   action on such a DM is refused `403 dm_not_for_agents`: opening one
//!   (`k2 chat dm`), sending, reading, reacting, listing, waiting. That
//!   covers DMs an agent opened before this build. Sew's own policy
//!   (`agents.dm_with_box_people`, set false by the sweep) refuses only
//!   opening a new one, so this gate is what holds. The one decision point
//!   for a future personal-assistant agent is
//!   [`k2_core::home_chat::agent_may_read_dm`] (false in this build).
//! - **K5 channels**: an agent reaches one only through a per-channel grant,
//!   which is a later slice (S8). Until then every K5 channel answers
//!   `404 not_granted`, and lists leave them out (deny by default).
//! - **No notifications from channels** (D17): `follow` and `wait --in` on a
//!   K5 channel answer `403 no_channel_notifications`; the hub drops every
//!   K5 delivery and every DM delivery from a box person before injection.
//!
//! A DM member K2 can't name (no row in the identity map) is not treated as
//! a box person: K2 maps every member it provisions (the owner and each
//! Connect login included) when Sew answers the provision, and the sweep
//! repairs the map from Sew's member list.

use k2_core::home_chat;
use k2_core::sew_chat::hub::{Delivery, Place};
use k2_core::tool_hook::{self as hook, sew_wire};

use super::routes::{Caller, ChatErr, R};
use super::sew::{ActAs, Channel, MemberLookup, SewChat};

pub const DM_NOT_FOR_AGENTS: &str = "dm_not_for_agents";
pub const NOT_GRANTED: &str = "not_granted";
pub const NO_CHANNEL_NOTIFICATIONS: &str = "no_channel_notifications";

fn dm_refusal() -> ChatErr {
    ChatErr::new(
        DM_NOT_FOR_AGENTS,
        403,
        "DMs with this server's people are for people: an agent never opens, reads or writes them. \
         Talk with your person in your own Thread (k2 thread)",
    )
}

fn k5_refusal() -> ChatErr {
    ChatErr::new(
        NOT_GRANTED,
        404,
        "K5 Space channels are for this server's people: an agent reads or posts in one only when a person in it \
         grants that agent the channel, and nobody has granted you this one",
    )
}

/// The K5 Space's Sew id, when K2 holds it.
fn k5_space_id() -> Option<String> {
    hook::admin_space_id().ok().flatten()
}

/// Is `ch` a channel of the K5 Space (by id, else by slug)?
pub fn is_k5_channel(ch: &Channel) -> bool {
    if ch.kind.is_dm() {
        return false;
    }
    match (ch.space_id.as_deref(), k5_space_id()) {
        (Some(s), Some(k5)) if s == k5 => true,
        _ => ch.space_slug.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(sew_wire::ADMIN_SPACE_SLUG)),
    }
}

/// The box person behind Sew member `member_id`, if it is one.
fn box_person(member_id: &str) -> Option<String> {
    let row = hook::chat_member_by_id(member_id).ok().flatten()?;
    home_chat::is_box_person(&row.external_id).then_some(row.external_id)
}

/// Is `ch` a DM the agent may not touch (a box person is in it)?
pub fn is_box_person_dm(a: &ActAs, ch: &Channel) -> bool {
    ch.kind.is_dm()
        && ch
            .dm_member_ids
            .iter()
            .filter_map(|m| box_person(m))
            .any(|ext| !home_chat::agent_may_read_dm(&a.workspace_uuid, &ext))
}

/// Every channel-scoped agent verb checks this first: a DM with a box
/// person is refused, a K5 channel is not granted.
pub fn check(c: &Caller, a: &ActAs, ch: &Channel) -> R<()> {
    if is_box_person_dm(a, ch) {
        return Err(dm_refusal());
    }
    if is_k5_channel(ch) && !home_chat::agent_may_use_k5_channel(&c.project_id, &ch.id) {
        return Err(k5_refusal());
    }
    Ok(())
}

/// May a list show `ch` to the agent?
pub fn visible(c: &Caller, a: &ActAs, ch: &Channel) -> bool {
    check(c, a, ch).is_ok()
}

/// `k2 chat dm <user>`: refused BEFORE Sew opens anything when the user is
/// a box person (Sew's open is find-or-create).
pub fn check_dm_target(sew: &dyn SewChat, a: &ActAs, username: &str) -> R<()> {
    let m = match sew.member(a, &MemberLookup::Username(username.to_string())) {
        Ok(m) => m,
        // Sew's masked not-found: open_dm answers the same way next.
        Err(_) => return Ok(()),
    };
    match box_person(&m.member_id) {
        Some(ext) if !home_chat::agent_may_read_dm(&a.workspace_uuid, &ext) => Err(dm_refusal()),
        _ => Ok(()),
    }
}

/// `follow` and `wait --in` on a K5 channel: agents get no notifications
/// from channels (D17), granted or not.
pub fn refuse_notifications(ch: &Channel) -> R<()> {
    if is_k5_channel(ch) {
        return Err(ChatErr::new(
            NO_CHANNEL_NOTIFICATIONS,
            403,
            "agents get no notifications from K5 channels: read with k2 chat messages when you need to",
        ));
    }
    Ok(())
}

/// The hub keeps a delivery only when it is neither from a K5 channel (any,
/// granted or not) nor a DM from a box person.
pub fn hub_keeps(d: &Delivery) -> bool {
    if d.place() == Place::SpaceChannel {
        let k5 = k5_space_id();
        let in_k5 = match (d.space_id.as_deref(), k5.as_deref()) {
            (Some(s), Some(k)) => s == k,
            _ => d.addr.split('/').next().is_some_and(|s| s.eq_ignore_ascii_case(sew_wire::ADMIN_SPACE_SLUG)),
        };
        return !in_k5;
    }
    !(d.place().is_dm() && !d.author_is_agent && box_person(&d.author_id).is_some())
}
