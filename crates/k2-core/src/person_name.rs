//! Person names (0.46.0, Rosson 2026-10-17). An optional first and last
//! name for every person, daemon-owned so every client shows the same.
//! Mirrors [`crate::person_color`].
//!
//! - **Stored.** On each Connect login's `connect-users.json` row
//!   (`first_name` / `last_name`). Written by `POST /cli/people/name` (and,
//!   for a new login, `POST /cli/users/add`).
//! - **The owner** has no names of its own: the owner person's names are
//!   those of the owner's linked account — the primary login in
//!   `ownerAccounts` ([`owner_login`]: the first of the sorted list that is
//!   a Connect user). No linked account → no first/last, displayName
//!   `Owner`. `ownerDisplayName` is no longer read for display (it stays on
//!   disk for downgrades; [`carry_over_owner_display_name`] moves it onto
//!   the linked account once).
//! - **Display only.** The username stays the identity everywhere it
//!   matters (senders, `[from …]`, assignees, principals, routing, notify
//!   keys). Names are shown beside it, never instead of it in those paths.
//! - **Plain text.** Trimmed; empty clears; at most [`MAX_CHARS`]
//!   characters (chars, not bytes); no control characters (newlines and
//!   tabs included) and no `<` / `>`.
//! - **Effective.** For a login whose person key is `owner` (an
//!   owner-linked login, the sole Owner-role login): the linked account's
//!   names when either is set, else the login's own.
//! - **displayName** (never empty): `"First Last"` (either alone) when
//!   set, else the username. For `owner` itself: the linked account's
//!   names, else its username, else `Owner`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::notify_prefs::{People, OWNER};

/// The longest first or last name, in characters.
pub const MAX_CHARS: usize = 64;

/// The owner person's displayName when no account is linked.
pub const OWNER_FALLBACK_DISPLAY: &str = "Owner";

/// One person's names on the wire: `{firstName, lastName, displayName}`;
/// the first two are `null` when unset, `displayName` is never empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameEntry {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub display_name: String,
}

impl NameEntry {
    /// True when either name is set (else displayName is a fallback).
    pub fn has_name(&self) -> bool {
        self.first_name.is_some() || self.last_name.is_some()
    }
}

/// Normalize one name: trim; empty → `Ok(None)` (clear); over
/// [`MAX_CHARS`] characters, a control character or `<` / `>` → an error.
pub fn normalize(raw: &str) -> Result<Option<String>, String> {
    let v = raw.trim();
    if v.is_empty() {
        return Ok(None);
    }
    let chars = v.chars().count();
    if chars > MAX_CHARS {
        return Err(format!("a name can be at most {MAX_CHARS} characters (got {chars})"));
    }
    if v.chars().any(char::is_control) {
        return Err("a name must be plain text: no line breaks, tabs or control characters".to_string());
    }
    if v.contains('<') || v.contains('>') {
        return Err("a name must be plain text: no < or >".to_string());
    }
    Ok(Some(v.to_string()))
}

/// `"First Last"`, either alone, or `None` when both are empty.
pub fn full_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = [first, last].into_iter().flatten().map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// A stored value that still passes [`normalize`] (a hand-edited file
/// with a stray value is ignored instead of shown).
fn valid(stored: Option<&str>) -> Option<String> {
    stored.and_then(|s| normalize(s).ok().flatten())
}

/// The owner's primary linked account: the first of the sorted
/// `ownerAccounts` that is a Connect user on this server (its stored
/// username). `None` when nothing is linked.
pub fn owner_login(people: &People) -> Option<String> {
    let mut linked: Vec<String> =
        people.linked.iter().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect();
    linked.sort();
    linked.dedup();
    linked.into_iter().find_map(|l| people.user(&l).map(|u| u.username.clone()))
}

/// [`owner_login`] from disk (`connect-users.json` + `ownerAccounts`).
/// An unreadable users store → `None`.
pub fn owner_login_now() -> Option<String> {
    match People::load() {
        Ok(p) => owner_login(&p),
        Err(e) => {
            crate::log_debug!("[person_name] people load failed; no owner account: {e}");
            None
        }
    }
}

// ── Sender identity (the ONE stamp rule) ────────────────────────────────

/// Who sent a message / post / comment, as the daemon resolved it from
/// the request credential (never from a body field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender<'a> {
    /// The owner daemon token.
    OwnerToken,
    /// A Connect login (its username).
    Login(&'a str),
}

impl<'a> Sender<'a> {
    /// From a daemon `session_author` string: `"owner"` (or empty) is the
    /// owner token, anything else a login.
    pub fn from_session_author(session_author: &'a str) -> Self {
        let s = session_author.trim();
        if s.is_empty() || s.eq_ignore_ascii_case(OWNER) {
            Sender::OwnerToken
        } else {
            Sender::Login(s)
        }
    }
}

/// The identity a sender is STAMPED with — `[from …]`, Thread `from`,
/// ticket comment authors, review-note authors, delivery labels (0.46.0,
/// Rosson 2026-10-17: identity = username everywhere it matters).
///
/// - the owner token → the owner's linked account's username
///   ([`owner_login`]), else `owner`;
/// - a Connect login (owner-linked or not) → its own username.
///
/// Real names are never a stamp; they are context only ([`label`]).
pub fn sender_identity(sender: Sender<'_>, people: &People) -> String {
    match sender {
        Sender::OwnerToken => owner_login(people).unwrap_or_else(|| OWNER.to_string()),
        Sender::Login(u) => {
            let u = u.trim();
            if u.is_empty() || u.eq_ignore_ascii_case(OWNER) {
                return sender_identity(Sender::OwnerToken, people);
            }
            people.user(u).map(|row| row.username.clone()).unwrap_or_else(|| u.to_string())
        }
    }
}

/// [`sender_identity`] from disk. An unreadable users store stamps the
/// owner token as `owner` and a login as itself.
pub fn sender_identity_now(sender: Sender<'_>) -> String {
    match People::load() {
        Ok(p) => sender_identity(sender, &p),
        Err(e) => {
            crate::log_debug!("[person_name] people load failed; stamping without the linked account: {e}");
            sender_identity(sender, &People { users: Vec::new(), linked: Vec::new() })
        }
    }
}

/// A login's OWN stored names (no owner substitution).
fn own(login: &str, people: &People) -> (Option<String>, Option<String>) {
    match people.user(login) {
        Some(u) => (valid(u.first_name.as_deref()), valid(u.last_name.as_deref())),
        None => (None, None),
    }
}

/// The effective names of `login` (`"owner"` or a Connect username).
pub fn effective(login: &str, people: &People) -> NameEntry {
    let login = login.trim();
    let primary = owner_login(people);
    if login.eq_ignore_ascii_case(OWNER) {
        return match primary {
            Some(p) => {
                let (first, last) = own(&p, people);
                let display_name = full_name(first.as_deref(), last.as_deref()).unwrap_or(p);
                NameEntry { first_name: first, last_name: last, display_name }
            }
            None => NameEntry { first_name: None, last_name: None, display_name: OWNER_FALLBACK_DISPLAY.to_string() },
        };
    }
    let (mut first, mut last) = (None, None);
    if people.person_key(login) == OWNER {
        if let Some(p) = primary.as_deref() {
            (first, last) = own(p, people);
        }
    }
    if first.is_none() && last.is_none() {
        (first, last) = own(login, people);
    }
    let username = people.user(login).map(|u| u.username.clone()).unwrap_or_else(|| login.to_ascii_lowercase());
    let display_name = full_name(first.as_deref(), last.as_deref()).unwrap_or(username);
    NameEntry { first_name: first, last_name: last, display_name }
}

/// The chosen names for the PERSON `login` stands for, or `None` when
/// they have neither a first nor a last name (no displayName fallback).
pub fn chosen(login: &str, people: &People) -> Option<NameEntry> {
    Some(effective(login, people)).filter(NameEntry::has_name)
}

/// Login → effective names for `owner` and every Connect login.
pub fn name_map(people: &People) -> BTreeMap<String, NameEntry> {
    let mut out = BTreeMap::new();
    out.insert(OWNER.to_string(), effective(OWNER, people));
    for u in &people.users {
        out.insert(u.username.clone(), effective(&u.username, people));
    }
    out
}

/// `"username — First Last"` when the person has a name, else the
/// username alone (agent rosters, CLI lists).
pub fn label(username: &str, entry: Option<&NameEntry>) -> String {
    match entry.filter(|e| e.has_name()) {
        Some(e) => format!("{username} — {}", e.display_name),
        None => username.to_string(),
    }
}

fn load_people() -> People {
    let settings = crate::app_settings::load();
    People { users: crate::connect_users::list_users().unwrap_or_default(), linked: settings.owner_accounts }
}

/// [`name_map`] from disk (`connect-users.json` + `ownerAccounts`). An
/// unreadable users store yields the owner entry only.
pub fn load_map() -> BTreeMap<String, NameEntry> {
    name_map(&load_people())
}

/// [`effective`] from disk.
pub fn load_effective(login: &str) -> NameEntry {
    effective(login, &load_people())
}

// ── One-time carry-over of the old ownerDisplayName ─────────────────────

/// Split an old display name into (first, last): the first
/// whitespace-separated word is the first name, the rest (joined by one
/// space) the last name. `None` when nothing is left after trimming.
pub fn split_display_name(raw: &str) -> Option<(String, Option<String>)> {
    let mut words = raw.split_whitespace();
    let first = words.next()?.to_string();
    let rest: Vec<&str> = words.collect();
    let last = if rest.is_empty() { None } else { Some(rest.join(" ")) };
    Some((first, last))
}

/// What [`carry_over_owner_display_name`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarryOver {
    /// The flag was already set: nothing to do, ever again.
    AlreadyDone,
    /// No linked account yet: try again later (flag left false).
    NoLinkedAccount,
    /// Saved the old display name on this login; flag set.
    Carried { login: String },
    /// Nothing to carry (empty display name, the account already has a
    /// name, or the old name failed the plain-text rules); flag set.
    Skipped,
}

/// Once per server (flag `ownerDisplayNameCarried`): when the old
/// `ownerDisplayName` is set, an account is linked and that account has
/// neither a first nor a last name, save the split display name on it.
/// Runs at daemon boot and after the linked accounts change, only while
/// the flag is false.
pub fn carry_over_owner_display_name() -> Result<CarryOver, String> {
    let settings = crate::app_settings::load();
    if settings.owner_display_name_carried {
        return Ok(CarryOver::AlreadyDone);
    }
    let people = People { users: crate::connect_users::list_users()?, linked: settings.owner_accounts.clone() };
    let Some(login) = owner_login(&people) else {
        return Ok(CarryOver::NoLinkedAccount);
    };
    let display = settings.owner_display_name.as_deref().map(str::trim).unwrap_or_default();
    let (has_name, split) = {
        let (f, l) = own(&login, &people);
        (f.is_some() || l.is_some(), split_display_name(display))
    };
    let mut outcome = CarryOver::Skipped;
    if !has_name {
        if let Some((first, last)) = split {
            let checked = normalize(&first).and_then(|f| match &last {
                Some(l) => normalize(l).map(|l| (f, l)),
                None => Ok((f, None)),
            });
            match checked {
                Ok((f, l)) => {
                    crate::connect_users::set_names(&login, Some(f.as_deref().unwrap_or("")), Some(l.as_deref().unwrap_or("")))?;
                    outcome = CarryOver::Carried { login: login.clone() };
                }
                Err(e) => {
                    crate::log_debug!("[person_name] ownerDisplayName not carried to '{login}': {e}");
                }
            }
        }
    }
    crate::app_settings::update(serde_json::json!({ "ownerDisplayNameCarried": true }))?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect_users::{ConnectUserView, Role};

    fn user(name: &str, role: Role, first: Option<&str>, last: Option<&str>) -> ConnectUserView {
        ConnectUserView {
            username: name.to_string(),
            created_at: chrono::Utc::now(),
            disabled: false,
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

    #[test]
    fn normalize_trims_and_clears() {
        assert_eq!(normalize("  Rosson ").expect("ok").as_deref(), Some("Rosson"));
        assert_eq!(normalize("").expect("ok"), None);
        assert_eq!(normalize("   ").expect("ok"), None);
        assert_eq!(normalize("Mary Ann").expect("ok").as_deref(), Some("Mary Ann"), "inner spaces stay");
    }

    #[test]
    fn normalize_caps_at_64_characters() {
        let at_cap = "a".repeat(64);
        assert_eq!(normalize(&at_cap).expect("64 ok").as_deref(), Some(at_cap.as_str()));
        let err = normalize(&"a".repeat(65)).expect_err("65 refused");
        assert!(err.contains("64"), "{err}");
        // Multi-byte characters count as one each: 64 × 'é' (128 bytes) is fine.
        let accents = "é".repeat(64);
        assert!(accents.len() > 64);
        assert_eq!(normalize(&accents).expect("64 chars").as_deref(), Some(accents.as_str()));
        assert!(normalize(&"é".repeat(65)).is_err());
        // Surrounding whitespace doesn't count.
        assert!(normalize(&format!("  {at_cap}  ")).is_ok());
    }

    #[test]
    fn normalize_refuses_control_characters_and_angle_brackets() {
        for bad in ["Ro\nsson", "Ro\tsson", "Ro\rsson", "Ro\u{0}sson", "Ro\u{7f}sson", "<b>Ro</b>", "a>b", "a<b"] {
            let err = normalize(bad).expect_err(bad);
            assert!(err.contains("plain text"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn full_name_joins_either_or_both() {
        assert_eq!(full_name(Some("Rosson"), Some("Long")).as_deref(), Some("Rosson Long"));
        assert_eq!(full_name(Some("Rosson"), None).as_deref(), Some("Rosson"));
        assert_eq!(full_name(None, Some("Long")).as_deref(), Some("Long"));
        assert_eq!(full_name(None, None), None);
        assert_eq!(full_name(Some(" "), Some("")), None);
        assert_eq!(full_name(Some(" Rosson "), Some(" Long ")).as_deref(), Some("Rosson Long"));
    }

    #[test]
    fn owner_login_is_the_first_sorted_linked_user() {
        let p = people(
            vec![user("zed", Role::Admin, None, None), user("amy", Role::Admin, None, None)],
            &["Zed", "ghost", "amy"],
        );
        assert_eq!(owner_login(&p).as_deref(), Some("amy"));
        let p = people(vec![user("zed", Role::Admin, None, None)], &["ghost", "zed"]);
        assert_eq!(owner_login(&p).as_deref(), Some("zed"), "a linked name that is not a user is skipped");
        assert_eq!(owner_login(&people(vec![user("zed", Role::Admin, None, None)], &[])), None);
    }

    /// The one stamp rule, table-driven.
    #[test]
    fn sender_identity_table() {
        let linked = people(
            vec![
                user("z3thon", Role::Admin, Some("Rosson"), Some("Long")),
                user("anna", Role::Member, Some("Anna"), None),
                user("wes", Role::Owner, None, None),
            ],
            &["z3thon"],
        );
        let unlinked = people(vec![user("anna", Role::Member, None, None)], &[]);
        let cases: &[(&str, Sender<'_>, &People, &str)] = &[
            ("owner token + linked → the linked username", Sender::OwnerToken, &linked, "z3thon"),
            ("owner token, none linked → owner", Sender::OwnerToken, &unlinked, "owner"),
            ("owner-linked Connect login → its username", Sender::Login("z3thon"), &linked, "z3thon"),
            ("other Connect user → its username", Sender::Login("anna"), &linked, "anna"),
            ("an Owner-role login that is not linked → its username", Sender::Login("wes"), &linked, "wes"),
            ("case-folded to the stored username", Sender::Login("ANNA"), &linked, "anna"),
            ("session author 'owner' is the owner token", Sender::from_session_author("owner"), &linked, "z3thon"),
            ("empty session author is the owner token", Sender::from_session_author(" "), &unlinked, "owner"),
        ];
        for (label, sender, p, want) in cases {
            assert_eq!(sender_identity(*sender, p), *want, "{label}");
        }
        // Real names never become the stamp.
        assert!(!sender_identity(Sender::OwnerToken, &linked).contains("Rosson"));
    }

    #[test]
    fn no_names_falls_back_to_the_username() {
        let p = people(vec![user("anna", Role::Member, None, None)], &[]);
        let e = effective("anna", &p);
        assert_eq!((e.first_name.clone(), e.last_name.clone()), (None, None));
        assert_eq!(e.display_name, "anna");
        let o = effective("owner", &p);
        assert_eq!((o.first_name, o.last_name), (None, None));
        assert_eq!(o.display_name, "Owner", "no linked account");
        assert_eq!(chosen("anna", &p), None);
    }

    #[test]
    fn effective_uses_the_logins_own_names() {
        let p = people(vec![user("anna", Role::Member, Some("Anna"), None)], &[]);
        let e = effective("anna", &p);
        assert_eq!(e.first_name.as_deref(), Some("Anna"));
        assert_eq!(e.last_name, None);
        assert_eq!(e.display_name, "Anna");
    }

    #[test]
    fn the_owner_shows_the_linked_accounts_names() {
        let p = people(
            vec![user("z3thon", Role::Admin, Some("Rosson"), Some("Long")), user("anna", Role::Member, None, None)],
            &["z3thon"],
        );
        let o = effective("owner", &p);
        assert_eq!(o.first_name.as_deref(), Some("Rosson"));
        assert_eq!(o.last_name.as_deref(), Some("Long"));
        assert_eq!(o.display_name, "Rosson Long");
        assert_eq!(effective("z3thon", &p).display_name, "Rosson Long");
        assert_eq!(effective("anna", &p).display_name, "anna", "others are untouched");
        // A nameless linked account: the owner shows its username.
        let p = people(vec![user("z3thon", Role::Admin, None, None)], &["z3thon"]);
        assert_eq!(effective("owner", &p).display_name, "z3thon");
        assert_eq!(effective("owner", &p).first_name, None);
    }

    #[test]
    fn a_second_linked_login_shows_the_primarys_names_else_its_own() {
        let p = people(
            vec![user("amy", Role::Admin, Some("Rosson"), None), user("zed", Role::Admin, Some("Zed"), None)],
            &["amy", "zed"],
        );
        assert_eq!(effective("zed", &p).display_name, "Rosson");
        let p = people(
            vec![user("amy", Role::Admin, None, None), user("zed", Role::Admin, Some("Zed"), None)],
            &["amy", "zed"],
        );
        assert_eq!(effective("zed", &p).display_name, "Zed", "the primary has no name: its own");
        assert_eq!(effective("owner", &p).display_name, "amy");
    }

    #[test]
    fn stray_stored_values_are_ignored() {
        let p = people(vec![user("anna", Role::Member, Some("<script>"), Some("Ok"))], &[]);
        let e = effective("anna", &p);
        assert_eq!(e.first_name, None);
        assert_eq!(e.display_name, "Ok");
    }

    #[test]
    fn name_map_covers_owner_and_every_login() {
        let p = people(
            vec![
                user("z3thon", Role::Admin, Some("Rosson"), Some("Long")),
                user("anna", Role::Member, Some("Anna"), Some("Smith")),
                user("bob", Role::Member, None, None),
            ],
            &["z3thon"],
        );
        let m = name_map(&p);
        let dn = |k: &str| m.get(k).map(|e| e.display_name.clone()).unwrap_or_else(|| panic!("no entry for {k}: {m:?}"));
        assert_eq!(dn("owner"), "Rosson Long");
        assert_eq!(dn("z3thon"), "Rosson Long");
        assert_eq!(dn("anna"), "Anna Smith");
        assert_eq!(dn("bob"), "bob", "every login has an entry");
        assert_eq!(m.get("bob").expect("bob").first_name, None);
        assert_eq!(m.len(), 4, "no display-name keys: {m:?}");
    }

    #[test]
    fn label_shows_the_name_beside_the_username() {
        let named = NameEntry { first_name: Some("Rosson".into()), last_name: Some("Long".into()), display_name: "Rosson Long".into() };
        assert_eq!(label("z3thon", Some(&named)), "z3thon — Rosson Long");
        let nameless = NameEntry { first_name: None, last_name: None, display_name: "z3thon".into() };
        assert_eq!(label("z3thon", Some(&nameless)), "z3thon", "a fallback displayName is not a name");
        assert_eq!(label("z3thon", None), "z3thon");
    }

    #[test]
    fn wire_shape_is_first_last_display() {
        let e = NameEntry { first_name: None, last_name: Some("Long".into()), display_name: "Long".into() };
        let v = serde_json::to_value(&e).expect("json");
        assert_eq!(v, serde_json::json!({ "firstName": null, "lastName": "Long", "displayName": "Long" }));
    }

    #[test]
    fn split_display_name_first_word_then_the_rest() {
        assert_eq!(split_display_name("Rosson Long"), Some(("Rosson".into(), Some("Long".into()))));
        assert_eq!(split_display_name("Cher"), Some(("Cher".into(), None)));
        assert_eq!(split_display_name("Mary Ann Smith"), Some(("Mary".into(), Some("Ann Smith".into()))));
        assert_eq!(split_display_name("  Rosson   Long  "), Some(("Rosson".into(), Some("Long".into()))));
        assert_eq!(split_display_name(""), None);
        assert_eq!(split_display_name("   "), None);
    }

    fn set_display(name: Option<&str>) {
        let mut s = crate::app_settings::load();
        s.owner_display_name = name.map(str::to_string);
        crate::app_settings::save(&s).expect("save settings");
    }

    fn names_of(login: &str) -> (Option<String>, Option<String>) {
        let u = crate::connect_users::list_users().expect("users").into_iter().find(|u| u.username == login).expect("user");
        (u.first_name, u.last_name)
    }

    #[test]
    fn carry_over_waits_for_a_linked_account_then_runs_once() {
        crate::tunnel::test_support::with_temp_home(|| {
            set_display(Some("Rosson Long"));
            crate::connect_users::add_user("z3thon", "password1").expect("add");
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::NoLinkedAccount);
            assert!(!crate::app_settings::load().owner_display_name_carried, "flag stays false until linked");
            crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["z3thon"] })).expect("link");
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::Carried { login: "z3thon".into() });
            assert_eq!(names_of("z3thon"), (Some("Rosson".into()), Some("Long".into())));
            let s = crate::app_settings::load();
            assert!(s.owner_display_name_carried);
            assert_eq!(s.owner_display_name.as_deref(), Some("Rosson Long"), "kept on disk for downgrades");
            // Never again, even after the names are cleared.
            crate::connect_users::set_names("z3thon", Some(""), Some("")).expect("clear");
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::AlreadyDone);
            assert_eq!(names_of("z3thon"), (None, None));
        });
    }

    #[test]
    fn carry_over_never_overwrites_an_existing_name() {
        crate::tunnel::test_support::with_temp_home(|| {
            set_display(Some("Mary Ann Smith"));
            crate::connect_users::add_user("mary", "password1").expect("add");
            crate::connect_users::set_names("mary", None, Some("Jones")).expect("names");
            crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["mary"] })).expect("link");
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::Skipped);
            assert_eq!(names_of("mary"), (None, Some("Jones".into())));
            assert!(crate::app_settings::load().owner_display_name_carried);
        });
    }

    #[test]
    fn carry_over_skips_empty_and_invalid_names_and_sets_the_flag() {
        crate::tunnel::test_support::with_temp_home(|| {
            crate::connect_users::add_user("solo", "password1").expect("add");
            crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["solo"] })).expect("link");
            set_display(Some("   "));
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::Skipped);
            assert_eq!(names_of("solo"), (None, None));
            assert!(crate::app_settings::load().owner_display_name_carried);

            crate::app_settings::update(serde_json::json!({ "ownerDisplayNameCarried": false })).expect("reset");
            set_display(Some("<b>Bad</b> Name"));
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::Skipped);
            assert_eq!(names_of("solo"), (None, None), "an invalid old name is skipped");
            assert!(crate::app_settings::load().owner_display_name_carried);

            crate::app_settings::update(serde_json::json!({ "ownerDisplayNameCarried": false })).expect("reset");
            set_display(Some("Cher"));
            assert_eq!(carry_over_owner_display_name().expect("carry"), CarryOver::Carried { login: "solo".into() });
            assert_eq!(names_of("solo"), (Some("Cher".into()), None));
        });
    }
}
