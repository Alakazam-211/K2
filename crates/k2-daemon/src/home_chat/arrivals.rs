//! Arrivals into the People group (homes-people-group-v1, 0.46.16; spec
//! `k2-design/.k2/prds/home-people-group-spec.md` §2).
//!
//! A DM or a channel that **arrives** on its own lands in the person's
//! People group, and the daemon adds the row (clients only pull it):
//! - **someone DMs the person**: their first message in a DM K2 keeps
//!   ([`dm_sent`], from `send`); the row is a `person` row for the sender;
//! - **someone adds the person to a channel**: `add` ([`channel_added`]), or
//!   a new channel Sew just made with them in it (the ledger's
//!   `after_done` → [`k2_core::home_chat::channel_arrived`] → [`install`]'s
//!   sink); the row is a `channel` row.
//!
//! What the person adds themselves (Add to Home's People and Channels tabs,
//! opening a DM, making a channel, joining an open one) is the client's and
//! lands in the open group, as agents do. A person who has any row for it
//! already, live or removed, in any group, is left alone
//! ([`homes_sync::land_arrival`]); existing rows never move. Nothing lands
//! while the switch is off, and nothing for a person who syncs no Homes here.

use std::collections::BTreeSet;
use std::sync::Mutex;

use k2_core::homes_sync::{self, Kind, Landing};

use crate::session_events::SessionEvent;

/// One DM or channel arriving for one person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    /// Who it arrived for (`k2:owner` or `k2:connect:<uuid>`).
    pub to_ext: String,
    /// [`Kind::Person`] (a DM) or [`Kind::Channel`].
    pub kind: Kind,
    /// The DM's other person's external id, or the Sew channel id.
    pub handle: String,
    /// The row's label (the person's name, the channel's name).
    pub label: String,
}

/// Arrivals already on a Home this run (landed, or there before): each
/// message of a DM asks again, and only the first needs the store.
fn seen() -> &'static Mutex<BTreeSet<(String, &'static str, String)>> {
    static S: Mutex<BTreeSet<(String, &'static str, String)>> = Mutex::new(BTreeSet::new());
    &S
}

/// Tests: forget [`seen`].
#[doc(hidden)]
pub fn forget_for_tests() {
    seen().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// The Home person key of box person `ext`: `owner` for the owner, else
/// the login's own key (`None`: no such enabled login).
pub fn person_of_ext(ext: &str) -> Option<String> {
    if ext == "k2:owner" {
        return Some(k2_core::notify_prefs::OWNER.to_string());
    }
    let id = ext.strip_prefix("k2:connect:")?;
    let login = super::people::logins().ok()?.into_iter().find(|l| l.id.eq_ignore_ascii_case(id) && !l.disabled)?;
    if login.owner_linked {
        return Some(k2_core::notify_prefs::OWNER.to_string());
    }
    k2_core::notify_prefs::People::load().ok()?.login_person_key(&login.username)
}

/// How this server names box person `ext` (a DM row's label).
pub fn name_of_ext(ext: &str) -> String {
    if ext == "k2:owner" {
        return crate::workspace_msg::resolve_owner_from();
    }
    let Some(id) = ext.strip_prefix("k2:connect:") else { return String::new() };
    super::people::logins().ok().and_then(|all| all.into_iter().find(|l| l.id.eq_ignore_ascii_case(id))).map(|l| l.display_name).unwrap_or_default()
}

/// Land each arrival now (on this thread: it takes the main database's
/// lock, so never call it holding that or the ledger's). Each person whose
/// Homes changed hears `homes_changed`. Never fails its caller: what goes
/// wrong is logged.
pub fn land(arrivals: &[Arrival]) {
    if arrivals.is_empty() || !homes_sync::people_group_on() {
        return;
    }
    let Some(server_id) = crate::boot_status::server_id() else {
        k2_core::log_debug!("[homes] People arrivals skipped: no serverId on this box");
        return;
    };
    let Some(db) = k2_core::db::try_shared() else { return };
    let now = chrono::Utc::now().timestamp_millis();
    for a in arrivals {
        let key = (a.to_ext.clone(), a.kind.as_str(), a.handle.to_ascii_lowercase());
        if seen().lock().unwrap_or_else(|p| p.into_inner()).contains(&key) {
            continue;
        }
        let Some(person) = person_of_ext(&a.to_ext) else {
            k2_core::log_debug!("[homes] People arrival for {}: not a person here", a.to_ext);
            continue;
        };
        let address = format!("{}::{server_id}", a.handle);
        let landed = {
            let conn = db.lock();
            if !homes_sync::tables_exist(&conn) {
                return;
            }
            homes_sync::land_arrival(&conn, &person, a.kind, &address, &a.label, now)
        };
        match landed {
            Ok(Landing::Landed { cursor }) => {
                seen().lock().unwrap_or_else(|p| p.into_inner()).insert(key);
                let _ = crate::session_events::emit(SessionEvent::HomesChanged { person, cursor });
            }
            Ok(Landing::AlreadyOnHome) => {
                seen().lock().unwrap_or_else(|p| p.into_inner()).insert(key);
            }
            // No device syncs Homes for them yet: it may start to, so ask again.
            Ok(Landing::NoGroups) => {}
            Err(e) => k2_core::log_debug!("[homes] People arrival for {} ({} {}): {e}", a.to_ext, a.kind.as_str(), a.handle),
        }
    }
}

/// [`land`] on its own thread: for callers holding a lock (the ledger's
/// `after_done` runs inside the chat store's connection).
pub fn land_later(arrivals: Vec<Arrival>) {
    if arrivals.is_empty() || !homes_sync::people_group_on() {
        return;
    }
    let spawned = std::thread::Builder::new().name("k2-people-arrivals".into()).spawn(move || land(&arrivals));
    if let Err(e) = spawned {
        k2_core::log_debug!("[homes] People arrivals: no thread: {e}");
    }
}

/// The ledger's sink for channels Sew just made (installed at start).
fn channel_sink(v: Vec<k2_core::home_chat::ChannelArrival>) {
    land_later(
        v.into_iter()
            .map(|c| Arrival { to_ext: c.ext, kind: Kind::Channel, handle: c.channel_id, label: c.name })
            .collect(),
    );
}

/// Daemon start: new channels arrive in their people's People groups.
pub fn install() {
    k2_core::home_chat::set_arrival_sink(Some(channel_sink));
}

/// `sender_ext` posted in DM `conv`: it arrives for the DM's other person
/// (when K2 keeps the DM, whichever of the two opened it).
pub fn dm_sent(sender_ext: &str, conv: &str) {
    if !homes_sync::people_group_on() {
        return;
    }
    let other = k2_core::tool_hook::with_home_chat_db(|conn| k2_core::tool_hook::admin_channels::dm_other(conn, sender_ext, conv));
    match other {
        Ok(Some(to)) if to != sender_ext && k2_core::home_chat::is_box_person(&to) => land(&[Arrival {
            to_ext: to,
            kind: Kind::Person,
            handle: sender_ext.to_string(),
            label: name_of_ext(sender_ext),
        }]),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[homes] People arrival for DM {conv}: {e}"),
    }
}

/// `adder_ext` added `added` to channel `channel_id` (`name`): it arrives
/// for each of them but the adder.
pub fn channel_added(adder_ext: &str, channel_id: &str, name: &str, added: &[String]) {
    let v: Vec<Arrival> = added
        .iter()
        .filter(|e| e.as_str() != adder_ext && k2_core::home_chat::is_box_person(e))
        .map(|e| Arrival { to_ext: e.clone(), kind: Kind::Channel, handle: channel_id.to_string(), label: name.to_string() })
        .collect();
    land(&v);
}
