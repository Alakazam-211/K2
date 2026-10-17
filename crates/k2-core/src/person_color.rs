//! Person colours (0.46.0, Rosson 2026-10-17). Daemon-owned so every client
//! paints a person the same colour.
//!
//! - **Palette.** The eight hues of the renderer's `PERSON_COLORS`
//!   (`src/renderer/components/Presence/PresenceChip.tsx`), same order.
//!   A chosen colour must be one of them.
//! - **Chosen.** The owner's lives in the `ownerColor` app setting; a
//!   Connect login's on its `connect-users.json` row (`color`). Both are
//!   written only by `POST /cli/people/color`.
//! - **Effective.** For a login whose person key is `owner` (the owner
//!   token, an owner-linked login, the sole Owner-role login): the owner's
//!   chosen colour when set. Otherwise the login's own chosen colour, else
//!   [`hash_color`] of the name — byte-for-byte the renderer's
//!   `personColor`, so nothing changes until someone picks.

use std::collections::BTreeMap;

use crate::notify_prefs::{People, OWNER};

/// The eight person hues, in `PERSON_COLORS` order.
pub const PALETTE: [&str; 8] = [
    "#2563eb", "#7c3aed", "#0d9488", "#db2777", "#4f46e5", "#0891b2", "#64748b", "#c026d3",
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

/// A palette entry for `raw` (case-insensitive, trimmed), or an error
/// naming the allowed values.
pub fn normalize(raw: &str) -> Result<&'static str, String> {
    let v = raw.trim().to_ascii_lowercase();
    PALETTE
        .iter()
        .copied()
        .find(|c| *c == v)
        .ok_or_else(|| format!("color must be one of {} (got {raw:?})", PALETTE.join(", ")))
}

/// A stored value that is still a palette entry (a hand-edited file with a
/// stray value falls back to the hash instead of painting it).
fn valid(stored: Option<&str>) -> Option<&'static str> {
    stored.and_then(|c| normalize(c).ok())
}

/// The effective colour of `login` (`"owner"` or a Connect username).
pub fn effective(login: &str, people: &People, owner_color: Option<&str>) -> String {
    let login = login.trim();
    if people.person_key(login) == OWNER {
        if let Some(c) = valid(owner_color) {
            return c.to_string();
        }
    }
    if let Some(c) = people.user(login).and_then(|u| valid(u.color.as_deref())) {
        return c.to_string();
    }
    hash_color(login).to_string()
}

/// The chosen colour (no fallback) for the PERSON `login` stands for:
/// the owner's for an owner-keyed login, else the login's own.
pub fn chosen(login: &str, people: &People, owner_color: Option<&str>) -> Option<String> {
    if people.person_key(login) == OWNER {
        return valid(owner_color).map(str::to_string);
    }
    people.user(login).and_then(|u| valid(u.color.as_deref())).map(str::to_string)
}

/// Name → effective colour for everyone a client may paint: `owner`,
/// every Connect login, and the owner's display name (ticket assignees
/// and summaries use it) unless it collides with a login.
pub fn color_map(people: &People, owner_color: Option<&str>, owner_display: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let owner = effective(OWNER, people, owner_color);
    out.insert(OWNER.to_string(), owner.clone());
    for u in &people.users {
        out.insert(u.username.clone(), effective(&u.username, people, owner_color));
    }
    if let Some(display) = owner_display.map(str::trim).filter(|d| !d.is_empty()) {
        out.entry(display.to_string()).or_insert(owner);
    }
    out
}

/// [`color_map`] from disk (`connect-users.json` + app settings). An
/// unreadable users store yields the owner entries only.
pub fn load_map() -> BTreeMap<String, String> {
    let settings = crate::app_settings::load();
    let people = People {
        users: crate::connect_users::list_users().unwrap_or_default(),
        linked: settings.owner_accounts.clone(),
    };
    color_map(&people, settings.owner_color.as_deref(), settings.owner_display_name.as_deref())
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
        }
    }

    fn people(users: Vec<ConnectUserView>, linked: &[&str]) -> People {
        People { users, linked: linked.iter().map(|s| s.to_string()).collect() }
    }

    /// Same values the renderer's `personColor` gives (PresenceChip test
    /// pins `owner` → pink).
    #[test]
    fn hash_matches_the_renderer() {
        assert_eq!(hash_color("owner"), "#db2777");
        assert_eq!(hash_color(""), "#2563eb");
        // 'a' = 97 → 97 % 8 = 1.
        assert_eq!(hash_color("a"), "#7c3aed");
        // "ab" = 97*31 + 98 = 3105 → 3105 % 8 = 1.
        assert_eq!(hash_color("ab"), "#7c3aed");
    }

    #[test]
    fn normalize_accepts_palette_only() {
        assert_eq!(normalize(" #2563EB ").expect("palette"), "#2563eb");
        let err = normalize("#ff0000").expect_err("off palette");
        assert!(err.contains("#2563eb"), "{err}");
        assert!(normalize("red").is_err());
        assert!(normalize("").is_err());
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
        let p = people(vec![user("anna", Role::Member, Some("#123456"))], &[]);
        assert_eq!(effective("anna", &p, Some("nope")), hash_color("anna"));
        assert_eq!(effective("owner", &p, Some("nope")), hash_color("owner"));
    }

    #[test]
    fn color_map_covers_owner_logins_and_display_name() {
        let p = people(
            vec![user("z3thon", Role::Admin, None), user("anna", Role::Member, Some("#64748b"))],
            &["z3thon"],
        );
        let m = color_map(&p, Some("#c026d3"), Some("Rosson"));
        assert_eq!(m.get("owner").map(String::as_str), Some("#c026d3"));
        assert_eq!(m.get("z3thon").map(String::as_str), Some("#c026d3"));
        assert_eq!(m.get("Rosson").map(String::as_str), Some("#c026d3"));
        assert_eq!(m.get("anna").map(String::as_str), Some("#64748b"));
        // A display name equal to a login never overrides that login.
        let m = color_map(&p, Some("#c026d3"), Some("anna"));
        assert_eq!(m.get("anna").map(String::as_str), Some("#64748b"));
    }
}
