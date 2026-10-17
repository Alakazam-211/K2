//! Person colours (0.46.0, Rosson 2026-10-17). Daemon-owned so every client
//! paints a person the same colour.
//!
//! - **Palette.** The eight hues of the renderer's `PERSON_COLORS`
//!   (`src/renderer/components/Presence/PresenceChip.tsx`), same order —
//!   the AUTOMATIC (hash) palette.
//! - **Any colour.** A chosen colour is any `#RRGGBB` (Rosson
//!   2026-10-17), stored lowercase; not limited to the palette.
//! - **Chosen.** The owner's lives in the `ownerColor` app setting; a
//!   Connect login's on its `connect-users.json` row (`color`). Both are
//!   written only by `POST /cli/people/color`.
//! - **Effective.** For a login whose person key is `owner` (the owner
//!   token or an owner-linked login): the owner's
//!   chosen colour when set. Otherwise the login's own chosen colour, else
//!   [`hash_color`] of the name — byte-for-byte the renderer's
//!   `personColor`, so nothing changes until someone picks.

use std::collections::BTreeMap;

use crate::notify_prefs::{People, OWNER};

/// The eight Automatic person hues, in `PERSON_COLORS` order (Rosson
/// 2026-10-17). The renderer carries the same list so the hash matches.
pub const PALETTE: [&str; 8] = [
    "#3a9e96", "#3a87a8", "#6a6fc8", "#8c62c4", "#b25cb0", "#c45a86", "#9b7a95", "#64778f",
];

/// The renderer's `personColor(name)`: `h = (h * 31 + charCode) >>> 0`
/// over UTF-16 code units, then `PALETTE[h % 8]`.
pub fn hash_color(name: &str) -> &'static str {
    let mut h: u32 = 0;
    for unit in name.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

/// `raw` as a lowercase `#rrggbb`: trimmed, case-insensitive, `#` plus
/// exactly six hex digits. Anything else (`#abc`, `red`, no `#`, eight
/// digits, empty) is an error.
pub fn normalize(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    let ok = v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return Err(format!("color must be #RRGGBB: '#' and six hex digits (got {raw:?})"));
    }
    Ok(v.to_ascii_lowercase())
}

/// A stored value that is still a valid `#rrggbb` (a hand-edited file
/// with a stray value falls back to the hash instead of painting it).
fn valid(stored: Option<&str>) -> Option<String> {
    stored.and_then(|c| normalize(c).ok())
}

/// The effective colour of `login` (`"owner"` or a Connect username).
pub fn effective(login: &str, people: &People, owner_color: Option<&str>) -> String {
    let login = login.trim();
    if people.person_key(login) == OWNER {
        if let Some(c) = valid(owner_color) {
            return c;
        }
    }
    if let Some(c) = people.user(login).and_then(|u| valid(u.color.as_deref())) {
        return c;
    }
    hash_color(login).to_string()
}

/// The chosen colour (no fallback) for the PERSON `login` stands for:
/// the owner's for an owner-keyed login, else the login's own.
pub fn chosen(login: &str, people: &People, owner_color: Option<&str>) -> Option<String> {
    if people.person_key(login) == OWNER {
        return valid(owner_color);
    }
    people.user(login).and_then(|u| valid(u.color.as_deref()))
}

/// Login → effective colour for everyone a client may paint: `owner`
/// and every Connect login. (The old `ownerDisplayName` key is gone: the
/// owner's assignee/identity key is the linked account's username, which
/// is already a login here.)
pub fn color_map(people: &People, owner_color: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert(OWNER.to_string(), effective(OWNER, people, owner_color));
    for u in &people.users {
        out.insert(u.username.clone(), effective(&u.username, people, owner_color));
    }
    out
}

/// [`color_map`] from disk (`connect-users.json` + app settings). An
/// unreadable users store yields the owner entry only.
pub fn load_map() -> BTreeMap<String, String> {
    let settings = crate::app_settings::load();
    let people = People {
        users: crate::connect_users::list_users().unwrap_or_default(),
        linked: settings.owner_accounts.clone(),
    };
    color_map(&people, settings.owner_color.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect_users::{ConnectUserView, Role};

    fn user(name: &str, role: Role, color: Option<&str>) -> ConnectUserView {
        ConnectUserView {
            username: name.to_string(),
            created_at: chrono::Utc::now(),
            disabled: false,
            role,
            was_viewer: false,
            can_clone: true,
            color: color.map(str::to_string),
            first_name: None,
            last_name: None,
            display_name: name.to_string(),
        }
    }

    fn people(users: Vec<ConnectUserView>, linked: &[&str]) -> People {
        People { users, linked: linked.iter().map(|s| s.to_string()).collect() }
    }

    /// Same values the renderer's `personColor` gives (PresenceChip test
    /// pins `owner` → pink).
    #[test]
    fn hash_matches_the_renderer() {
        // "owner" → index 3.
        assert_eq!(hash_color("owner"), "#8c62c4");
        assert_eq!(hash_color(""), "#3a9e96");
        // 'a' = 97 → 97 % 8 = 1.
        assert_eq!(hash_color("a"), "#3a87a8");
        // "ab" = 97*31 + 98 = 3105 → 3105 % 8 = 1.
        assert_eq!(hash_color("ab"), "#3a87a8");
    }

    #[test]
    fn normalize_accepts_any_rrggbb() {
        assert_eq!(normalize(" #0D9488 ").expect("hex"), "#0d9488");
        assert_eq!(normalize("#ff0000").expect("off the palette is fine"), "#ff0000");
        assert_eq!(normalize("#123ABC").expect("hex"), "#123abc");
        for bad in ["#abc", "red", "ff0000", "#ff00000", "#gg0000", "", "#ff00008f", "# ff0000", "#ff 000"] {
            let err = normalize(bad).expect_err(bad);
            assert!(err.contains("#RRGGBB"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn effective_falls_back_to_the_hash() {
        let p = people(vec![user("anna", Role::Member, None)], &[]);
        assert_eq!(effective("anna", &p, None), hash_color("anna"));
        assert_eq!(effective("owner", &p, None), hash_color("owner"));
        assert_eq!(chosen("anna", &p, None), None);
    }

    #[test]
    fn effective_uses_the_chosen_colour() {
        let p = people(vec![user("anna", Role::Member, Some("#0d9488"))], &[]);
        assert_eq!(effective("anna", &p, None), "#0d9488");
        assert_eq!(effective("owner", &p, Some("#4f46e5")), "#4f46e5");
        assert_eq!(chosen("anna", &p, None).as_deref(), Some("#0d9488"));
    }

    #[test]
    fn linked_accounts_show_the_owner_colour() {
        let p = people(
            vec![user("z3thon", Role::Admin, Some("#0d9488")), user("anna", Role::Member, None)],
            &["z3thon"],
        );
        assert_eq!(effective("z3thon", &p, Some("#4f46e5")), "#4f46e5");
        assert_eq!(chosen("z3thon", &p, Some("#4f46e5")).as_deref(), Some("#4f46e5"));
        // No owner colour yet: the linked login keeps its own.
        assert_eq!(effective("z3thon", &p, None), "#0d9488");
        // Others are untouched.
        assert_eq!(effective("anna", &p, Some("#4f46e5")), hash_color("anna"));
    }

    #[test]
    fn stray_stored_values_fall_back() {
        let p = people(vec![user("anna", Role::Member, Some("#12345"))], &[]);
        assert_eq!(effective("anna", &p, Some("nope")), hash_color("anna"));
        assert_eq!(chosen("anna", &p, Some("#abc")), None);
        assert_eq!(effective("owner", &p, Some("nope")), hash_color("owner"));
    }

    #[test]
    fn color_map_covers_owner_and_logins() {
        let p = people(
            vec![user("z3thon", Role::Admin, None), user("anna", Role::Member, Some("#64748b"))],
            &["z3thon"],
        );
        let m = color_map(&p, Some("#c026d3"));
        assert_eq!(m.get("owner").map(String::as_str), Some("#c026d3"));
        assert_eq!(m.get("z3thon").map(String::as_str), Some("#c026d3"));
        assert_eq!(m.get("anna").map(String::as_str), Some("#64748b"));
        assert_eq!(m.len(), 3, "no display-name keys: {m:?}");
    }
}
