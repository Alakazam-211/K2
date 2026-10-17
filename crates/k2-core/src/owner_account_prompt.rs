//! "Your account" setup prompt (0.46.0, Rosson 2026-10-17).
//!
//! An owner with no linked account sends as `[from owner]` (it used to be
//! the old display name), so the desktop asks once which Admin Access login
//! is the owner's. The daemon owns whether it shows, so every window and
//! client agrees (no localStorage gating):
//!
//! - **Linked** (`ownerAccounts` names a user): never shows.
//! - **Pick** (enabled Owner/Admin logins exist): shows until "Not now"
//!   stores the current version in `ownerAccountPrompt.dismissedVersion`;
//!   it asks again after the version changes. Pre-select the obvious one:
//!   the only candidate, or the one whose first/last name matches the old
//!   owner "Your name" (`ownerDisplayName`, read here for MATCHING only —
//!   never a stamp; see the source scan in `workspace_msg`).
//! - **Create** (no enabled Owner/Admin login): shows, and can't be
//!   dismissed; the client creates one with the existing routes
//!   (`users/add`, `users/set-role`) and links it (`owner/accounts`).
//!
//! The routes (`GET /cli/owner/account-prompt`,
//! `POST /cli/owner/account-prompt/dismiss`) are the owner daemon token's
//! only: the prompt is about who the owner is.

use serde::{Deserialize, Serialize};

use crate::connect_users::{ConnectUserView, Role};
use crate::notify_prefs::People;

/// The stored state (`AppSettings::owner_account_prompt`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerAccountPrompt {
    /// The version where the owner chose "Not now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_version: Option<String>,
}

/// One Owner/Admin login the owner may pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub username: String,
    pub role: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub display_name: String,
    /// The login's effective person colour.
    pub color: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Pick,
    Create,
}

/// Why it shows or not (for the CLI and logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Linked,
    Dismissed,
    PickAccount,
    CreateAccount,
}

/// `GET /cli/owner/account-prompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    pub ok: bool,
    pub show: bool,
    pub mode: Mode,
    pub reason: Reason,
    /// The owner's linked account, when one is.
    pub linked: Option<String>,
    pub version: String,
    pub dismissed_version: Option<String>,
    pub candidates: Vec<Candidate>,
    pub preselect: Option<String>,
}

fn role_wire(r: Role) -> &'static str {
    match r {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

/// Enabled Owner/Admin logins, sorted by username.
pub fn candidate_users(users: &[ConnectUserView]) -> Vec<&ConnectUserView> {
    let mut out: Vec<&ConnectUserView> =
        users.iter().filter(|u| !u.disabled && matches!(u.role, Role::Owner | Role::Admin)).collect();
    out.sort_by(|a, b| a.username.cmp(&b.username));
    out
}

/// The obvious pick: the only candidate, else the one candidate whose
/// "First Last" matches the old owner display name (trimmed, any case,
/// inner whitespace collapsed). Several matches or none → `None`.
pub fn preselect(candidates: &[&ConnectUserView], legacy_display: Option<&str>) -> Option<String> {
    if candidates.len() == 1 {
        return Some(candidates[0].username.clone());
    }
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let want = legacy_display.map(squash).filter(|s| !s.is_empty())?;
    let mut hits = candidates.iter().filter(|u| {
        crate::person_name::full_name(u.first_name.as_deref(), u.last_name.as_deref())
            .is_some_and(|n| squash(&n) == want)
    });
    let first = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    Some(first.username.clone())
}

/// The prompt, from the people, the stored state and this version.
pub fn view(
    people: &People,
    state: &OwnerAccountPrompt,
    legacy_display: Option<&str>,
    owner_color: Option<&str>,
    version: &str,
) -> PromptView {
    let linked = crate::person_name::owner_login(people);
    let cands = candidate_users(&people.users);
    let candidates: Vec<Candidate> = cands
        .iter()
        .map(|u| Candidate {
            username: u.username.clone(),
            role: role_wire(u.role).to_string(),
            first_name: u.first_name.clone(),
            last_name: u.last_name.clone(),
            display_name: crate::person_name::full_name(u.first_name.as_deref(), u.last_name.as_deref())
                .unwrap_or_else(|| u.username.clone()),
            color: crate::person_color::effective(&u.username, people, owner_color),
        })
        .collect();
    let mode = if candidates.is_empty() { Mode::Create } else { Mode::Pick };
    let dismissed = state.dismissed_version.as_deref() == Some(version);
    let (show, reason) = if linked.is_some() {
        (false, Reason::Linked)
    } else if mode == Mode::Create {
        // Required: "Not now" doesn't apply when there is nothing to pick.
        (true, Reason::CreateAccount)
    } else if dismissed {
        (false, Reason::Dismissed)
    } else {
        (true, Reason::PickAccount)
    };
    let preselect = if mode == Mode::Pick { preselect(&cands, legacy_display) } else { None };
    PromptView {
        ok: true,
        show,
        mode,
        reason,
        linked,
        version: version.to_string(),
        dismissed_version: state.dismissed_version.clone(),
        candidates,
        preselect,
    }
}

/// [`view`] from disk.
pub fn load_view(version: &str) -> Result<PromptView, String> {
    let settings = crate::app_settings::load();
    let people = People { users: crate::connect_users::list_users()?, linked: settings.owner_accounts.clone() };
    Ok(view(
        &people,
        &settings.owner_account_prompt,
        settings.owner_display_name.as_deref(),
        settings.owner_color.as_deref(),
        version,
    ))
}

/// "Not now": remember this version.
pub fn dismiss(version: &str) -> Result<(), String> {
    crate::app_settings::update(serde_json::json!({ "ownerAccountPrompt": { "dismissedVersion": version } }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: &str, role: Role, first: Option<&str>, last: Option<&str>, disabled: bool) -> ConnectUserView {
        ConnectUserView {
            username: name.to_string(),
            created_at: chrono::Utc::now(),
            disabled,
            role,
            was_viewer: false,
            can_clone: true,
            color: None,
            first_name: first.map(str::to_string),
            last_name: last.map(str::to_string),
            display_name: name.to_string(),
        }
    }

    fn people(users: Vec<ConnectUserView>, linked: &[&str]) -> People {
        People { users, linked: linked.iter().map(|s| s.to_string()).collect() }
    }

    fn state(dismissed: Option<&str>) -> OwnerAccountPrompt {
        OwnerAccountPrompt { dismissed_version: dismissed.map(str::to_string) }
    }

    #[test]
    fn linked_never_shows() {
        let p = people(vec![user("z3thon", Role::Admin, None, None, false)], &["z3thon"]);
        let v = view(&p, &state(None), None, None, "0.46.0");
        assert_eq!((v.show, v.reason, v.linked.as_deref()), (false, Reason::Linked, Some("z3thon")));
    }

    #[test]
    fn a_link_to_a_removed_user_does_not_count() {
        let p = people(vec![user("julie", Role::Admin, None, None, false)], &["ghost"]);
        let v = view(&p, &state(None), None, None, "0.46.0");
        assert_eq!((v.show, v.reason, v.linked), (true, Reason::PickAccount, None));
    }

    #[test]
    fn pick_lists_enabled_owner_and_admin_logins_only_sorted() {
        let p = people(
            vec![
                user("zed", Role::Admin, Some("Zed"), None, false),
                user("anna", Role::Member, None, None, false),
                user("julie", Role::Owner, None, None, false),
                user("old", Role::Admin, None, None, true),
            ],
            &[],
        );
        let v = view(&p, &state(None), None, Some("#123456"), "0.46.0");
        assert_eq!((v.show, v.mode, v.reason), (true, Mode::Pick, Reason::PickAccount));
        let names: Vec<(&str, &str, &str)> =
            v.candidates.iter().map(|c| (c.username.as_str(), c.role.as_str(), c.display_name.as_str())).collect();
        assert_eq!(names, vec![("julie", "owner", "julie"), ("zed", "admin", "Zed")]);
        // Each row carries its own effective colour: nothing is linked, so
        // even the sole Owner-role login is just itself (not the owner's).
        assert_eq!(v.candidates[0].color, crate::person_color::hash_color("julie"));
        assert_eq!(v.candidates[1].color, crate::person_color::hash_color("zed"));
    }

    #[test]
    fn not_now_hides_it_until_the_version_changes() {
        let p = people(vec![user("a", Role::Admin, None, None, false), user("b", Role::Admin, None, None, false)], &[]);
        let v = view(&p, &state(Some("0.46.0")), None, None, "0.46.0");
        assert_eq!((v.show, v.reason), (false, Reason::Dismissed));
        let v = view(&p, &state(Some("0.46.0")), None, None, "0.46.1");
        assert_eq!((v.show, v.reason), (true, Reason::PickAccount));
    }

    #[test]
    fn no_admins_means_create_and_it_cannot_be_dismissed() {
        let p = people(vec![user("anna", Role::Member, None, None, false)], &[]);
        let v = view(&p, &state(Some("0.46.0")), Some("Rosson Long"), None, "0.46.0");
        assert_eq!((v.show, v.mode, v.reason), (true, Mode::Create, Reason::CreateAccount));
        assert!(v.candidates.is_empty());
        assert_eq!(v.preselect, None);
        let empty = people(vec![], &[]);
        assert_eq!(view(&empty, &state(None), None, None, "0.46.0").mode, Mode::Create);
    }

    #[test]
    fn preselect_the_only_admin() {
        let p = people(vec![user("solo", Role::Admin, None, None, false)], &[]);
        assert_eq!(view(&p, &state(None), None, None, "0.46.0").preselect.as_deref(), Some("solo"));
    }

    #[test]
    fn preselect_the_old_your_name_match_only_when_unique() {
        let users = vec![
            user("z3thon", Role::Admin, Some("Rosson"), Some("Long"), false),
            user("julie", Role::Owner, Some("Julie"), None, false),
        ];
        let p = people(users.clone(), &[]);
        assert_eq!(view(&p, &state(None), Some("  rosson   LONG "), None, "0.46.0").preselect.as_deref(), Some("z3thon"));
        assert_eq!(view(&p, &state(None), Some("Someone Else"), None, "0.46.0").preselect, None);
        assert_eq!(view(&p, &state(None), None, None, "0.46.0").preselect, None);
        assert_eq!(view(&p, &state(None), Some(""), None, "0.46.0").preselect, None);
        let mut twins = users;
        twins.push(user("rosson2", Role::Admin, Some("Rosson"), Some("Long"), false));
        let p = people(twins, &[]);
        assert_eq!(view(&p, &state(None), Some("Rosson Long"), None, "0.46.0").preselect, None, "two matches → nothing");
    }

    #[test]
    fn the_settings_field_round_trips_and_dismiss_persists() {
        crate::tunnel::test_support::with_temp_home(|| {
            assert_eq!(crate::app_settings::load().owner_account_prompt, OwnerAccountPrompt::default());
            dismiss("0.46.0").expect("dismiss");
            let s = crate::app_settings::load();
            assert_eq!(s.owner_account_prompt.dismissed_version.as_deref(), Some("0.46.0"));
            let json = serde_json::to_value(&s).expect("json");
            assert_eq!(json["ownerAccountPrompt"], serde_json::json!({ "dismissedVersion": "0.46.0" }));
            // A store without the key loads the default.
            let parsed: OwnerAccountPrompt = serde_json::from_value(serde_json::json!({})).expect("parse");
            assert_eq!(parsed, OwnerAccountPrompt::default());
            crate::connect_users::add_user("solo", "password1").expect("add");
            crate::connect_users::set_role("solo", Role::Admin).expect("role");
            let v = load_view("0.46.0").expect("view");
            assert_eq!((v.show, v.reason), (false, Reason::Dismissed));
            let v = load_view("0.46.1").expect("view");
            assert_eq!((v.show, v.preselect.as_deref()), (true, Some("solo")));
        });
    }
}
