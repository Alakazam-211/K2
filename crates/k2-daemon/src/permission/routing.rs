//! A2 — Who hears about a permission card (PRD rev 3.1 §6.7, §17.7; vs-live
//! C26, C28).
//!
//! [`push_card`] is the hook `overlay_routes::present_action_card` calls
//! for `CardAudience::Logins` (frozen signature, C11). It does two things:
//!
//! 1. **The push.** One Thread push per person who may answer (the owner,
//!    plus the person of every login at the floor), to that person's
//!    phones only. It goes through `push_routes::deliver_thread`, the same
//!    door every Thread push uses, so the E2E lanes (sealed / legacy /
//!    `sealed-only` generic) apply exactly as for an agent post: the sealed
//!    plaintext stays `kind:"thread"`. It is never coalesced with the
//!    agent's posts and draws from its own budget
//!    (`ThreadPush.budget = Permission`, 30 an hour per person, taken in
//!    `deliver_thread`). Never an empty list: a person with no phone gets
//!    no push. A person whose bell is off, or who is looking at that Thread
//!    right now, gets none either. Never the agent's `--why`, a note or a
//!    value: the title is "{agent} asks for a permission", the body the
//!    Settings label.
//! 2. **The Tickets item** ([`crate::feedback_routes::file_permission_notice`]),
//!    filed when nobody who may answer looked at that Thread in the last 10
//!    minutes (`thread_viewers::last_visible_at`; empty after a restart, so
//!    the item is filed). Assigned to the eligible logins only; none →
//!    unassigned, no push.
//!
//! A request that joins an open batch card rewrites that card in place and
//! sends nothing new (the card is already out). `k2 permission update`
//! posts the card again, so the people who may answer hear about it again.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use k2_core::notify_prefs::{People, Recipient, OWNER};
use k2_core::permission::request::{limits, PermissionRequest};
use k2_core::push::{PushEvent, ThreadBudget, ThreadMsgKind, ThreadPush};
use k2_core::push_devices::PushDevice;

use super::{present, store};

/// The Tickets item is filed when nobody eligible was visible on the Thread
/// within this window.
pub(crate) const VISIBLE_WINDOW: Duration = Duration::from_secs(limits::VISIBLE_WINDOW_SECS as u64);

/// Push a just-posted permission card to `logins` (the eligible approvers)
/// only, and file the Tickets item when nobody eligible was looking. An
/// empty list still reaches the owner (always eligible); a person with no
/// phone gets no push.
pub(crate) fn push_card(conversation_id: &str, project_id: &str, addr: &str, card_id: &str, seq: i64, logins: &[String]) {
    let Some(req) = request_on(conversation_id, card_id) else {
        k2_core::log_debug!("[permission] push: card {card_id} carries no request");
        return;
    };
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[permission] push: people load failed: {e}");
            People::default()
        }
    };
    let eligible = eligible_people(&people, logins);
    let setting = setting_of(&req);
    let agent = agent_display_name(project_id, addr);
    let now = Instant::now();
    let viewers = crate::thread_viewers::global();

    // 1. The push, per person.
    let devices = k2_core::push_devices::list_devices().unwrap_or_default();
    let bell = |person: &str| bell_on(person);
    let viewing = |person: &str| viewers.is_viewing(person, conversation_id, now);
    let (agent_color, avatar_rev) = k2_core::projects_ops::project_push_look(project_id);
    for r in recipients(&people, &devices, &eligible, &bell, &viewing) {
        let totals = totals_for(&people, &r.person);
        crate::push_routes::deliver_thread(PushEvent::ThreadPost(ThreadPush {
            agent_name: agent.clone(),
            conversation_id: conversation_id.to_string(),
            addr: addr.to_string(),
            seq,
            project_id: project_id.to_string(),
            msg_kind: ThreadMsgKind::Choice,
            count: 1,
            principal: r.person.clone(),
            device_usernames: r.device_usernames,
            unread_total: totals.total.to_string(),
            waiting_total: totals.waiting_total.to_string(),
            avatar_rev: avatar_rev.clone(),
            agent_color: agent_color.clone(),
            preview: String::new(),
            msg: None,
            budget: ThreadBudget::Permission { request_id: req.display_id().to_string(), setting: setting.clone() },
        }));
    }

    // 2. The Tickets item, when nobody who may answer was looking.
    let seen = eligible.iter().any(|p| viewers.visible_within(p, conversation_id, VISIBLE_WINDOW, now));
    if !seen {
        let (title, body) = notice_text(&req, &agent, addr);
        let assignees: Vec<String> = logins.to_vec();
        if let Err(e) = crate::feedback_routes::file_permission_notice(project_id, &agent, &title, &body, &assignees) {
            k2_core::log_debug!("[permission] Tickets item for {}: {e}", req.display_id());
        }
    }
}

/// The request a just-posted card carries: the row is not linked to the
/// card yet (`store::set_card` runs after the post), so read the id from
/// the card's own `choice.action.params`.
fn request_on(conversation_id: &str, card_id: &str) -> Option<PermissionRequest> {
    if let Some(r) = store::for_card(conversation_id, card_id).ok().and_then(|v| v.into_iter().next()) {
        return Some(r);
    }
    let action = k2_core::overlay::card_action(conversation_id, card_id).ok().flatten()?;
    let id = action
        .params
        .get("request")
        .and_then(|v| v.as_str())
        .or_else(|| action.params.get("requests").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()))?
        .to_string();
    store::get_by_display_id(&id).ok().flatten()
}

/// The Settings label (`view.setting`): the push body.
fn setting_of(req: &PermissionRequest) -> String {
    present::find_action(&req.action)
        .map(|r| present::words(r.spec, &present::ctx_of(req)).setting)
        .unwrap_or_else(|| req.action.clone())
}

/// The workspace's display name (`projects.name`, else its handle, else
/// the address): the push title, as for any Thread push.
fn agent_display_name(project_id: &str, addr: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row("SELECT name, handle FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    row.and_then(|(name, handle)| {
        [name, handle].into_iter().flatten().map(|s| s.trim().to_string()).find(|s| !s.is_empty())
    })
    .unwrap_or_else(|| addr.to_string())
}

/// The people who may answer: the owner (the owner token is at every
/// floor) and the person of every eligible login (owner-linked logins fold
/// into `owner`).
pub(crate) fn eligible_people(people: &People, logins: &[String]) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = logins.iter().map(|l| people.person_key(l)).collect();
    out.insert(OWNER.to_string());
    out
}

/// Who gets the push: every eligible person with at least one phone whose
/// login is live (TP11), bell on, not looking at the Thread right now.
/// Pure (tests drive it).
pub(crate) fn recipients(
    people: &People,
    devices: &[PushDevice],
    eligible: &BTreeSet<String>,
    bell: &dyn Fn(&str) -> bool,
    viewing: &dyn Fn(&str) -> bool,
) -> Vec<Recipient> {
    let mut by_person: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for d in devices {
        if let Some(person) = people.device_person(&d.username) {
            if eligible.contains(&person) {
                by_person.entry(person).or_default().insert(d.username.clone());
            }
        }
    }
    by_person
        .into_iter()
        .filter(|(person, logins)| !logins.is_empty() && bell(person) && !viewing(person))
        .map(|(person, logins)| Recipient { person, device_usernames: logins.into_iter().collect() })
        .collect()
}

fn bell_on(person: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !k2_core::notify_prefs::tables_exist(&conn) {
        return true;
    }
    match k2_core::notify_prefs::PrefsSnapshot::load(&conn) {
        Ok(p) => p.bell(person).on,
        Err(e) => {
            k2_core::log_debug!("[permission] push: prefs load failed: {e}");
            true
        }
    }
}

fn totals_for(people: &People, person: &str) -> k2_core::read_state::Totals {
    let p = k2_core::read_state::Person::of_login(people, person);
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !k2_core::read_state::tables_exist(&conn) {
        return Default::default();
    }
    k2_core::read_state::totals(&conn, &p, k2_core::read_state::now_secs()).unwrap_or_default()
}

/// The Tickets item's title and body: K2's words only (never the agent's
/// `--why`, a note or a secret), and where to answer.
pub(crate) fn notice_text(req: &PermissionRequest, agent: &str, addr: &str) -> (String, String) {
    let (setting, what, path) = match present::find_action(&req.action) {
        Some(r) => {
            let w = present::words(r.spec, &present::ctx_of(req));
            (w.setting, w.what_changes, w.settings_path)
        }
        None => (req.action.clone(), String::new(), String::new()),
    };
    let floor = present::floor_of(&req.floor);
    let title = format!("{agent} asks for a permission: {setting}");
    let mut body = String::new();
    if !what.is_empty() {
        body.push_str(&what);
        body.push_str("\n\n");
    }
    body.push_str(&format!(
        "Request {} waits for an {} on {addr}'s Thread (Allow or Deny there, or under Waiting on you).",
        req.display_id(),
        if floor.as_str() == "owner" { "owner" } else { "owner or admin" },
    ));
    if !path.is_empty() {
        body.push_str(&format!(" The same switch is in {path}."));
    }
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::connect_users::{ConnectUserView, Role};

    fn user(name: &str, role: Role, disabled: bool) -> ConnectUserView {
        ConnectUserView {
            username: name.to_string(),
            created_at: chrono::Utc::now(),
            disabled,
            role,
            was_viewer: false,
            can_clone: true,
            color: None,
            first_name: None,
            last_name: None,
            display_name: name.to_string(),
        }
    }

    fn device(id: &str, username: &str) -> PushDevice {
        PushDevice {
            device_id: id.to_string(),
            platform: "apns".to_string(),
            token: format!("tok-{id}"),
            username: username.to_string(),
            created_at: 1,
            updated_at: 1,
            last_seen_at: 1,
            previews: true,
        }
    }

    fn people() -> People {
        People {
            users: vec![
                user("rosson", Role::Owner, false),
                user("ada", Role::Admin, false),
                user("dana", Role::Member, false),
                user("gone", Role::Admin, true),
                user("phone-owner", Role::Member, false),
            ],
            linked: vec!["phone-owner".into()],
        }
    }

    /// §6.7: the eligible set only; the owner always; never a Member; a
    /// disabled login's phone never; nobody with no phone; bell off and
    /// "looking now" skip.
    #[test]
    fn the_push_goes_to_eligible_people_with_phones_only() {
        let p = people();
        let devices = vec![
            device("d1", "rosson"),
            device("d2", "ada"),
            device("d3", "dana"),
            device("d4", "gone"),
            device("d5", "owner"),
            device("d6", "phone-owner"),
        ];
        let yes = |_: &str| true;
        let no = |_: &str| false;
        // Admin floor: rosson + ada + gone (disabled) are the logins.
        let admin = eligible_people(&p, &["rosson".into(), "ada".into(), "gone".into()]);
        assert_eq!(admin.iter().map(String::as_str).collect::<Vec<_>>(), ["ada", "gone", "owner", "rosson"]);
        let got = recipients(&p, &devices, &admin, &yes, &no);
        let people_got: Vec<(&str, Vec<&str>)> =
            got.iter().map(|r| (r.person.as_str(), r.device_usernames.iter().map(String::as_str).collect())).collect();
        assert_eq!(
            people_got,
            vec![("ada", vec!["ada"]), ("owner", vec!["owner", "phone-owner"]), ("rosson", vec!["rosson"])],
            "the owner person gets the owner-token and linked phones; dana (member) and gone (disabled) never"
        );
        // Owner floor: no Admin.
        let owner = eligible_people(&p, &["rosson".into()]);
        let got = recipients(&p, &devices, &owner, &yes, &no);
        assert!(got.iter().all(|r| r.person != "ada"), "{got:?}");
        // Nobody eligible has a phone: nothing at all, never "everyone".
        let only_dana_phone = vec![device("d3", "dana")];
        assert!(recipients(&p, &only_dana_phone, &admin, &yes, &no).is_empty());
        assert!(recipients(&p, &[], &admin, &yes, &no).is_empty());
        // Bell off / looking now.
        let bell = |person: &str| person != "ada";
        assert!(recipients(&p, &devices, &admin, &bell, &no).iter().all(|r| r.person != "ada"));
        let viewing = |person: &str| person == "owner";
        assert!(recipients(&p, &devices, &admin, &yes, &viewing).iter().all(|r| r.person != "owner"));
        assert!(recipients(&p, &devices, &admin, &yes, &viewing).iter().all(|r| !r.device_usernames.is_empty()));
    }

    #[test]
    fn the_visible_window_is_ten_minutes() {
        assert_eq!(VISIBLE_WINDOW, Duration::from_secs(600));
        assert_eq!(k2_core::push::budget::PERMISSION_PER_HOUR as u32, limits::PUSHES_PER_PERSON_PER_HOUR);
    }
}
