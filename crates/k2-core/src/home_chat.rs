//! People and channels on Home (prd-home-people-channels-v1): the switch
//! and the rules that need no daemon.
//!
//! **The switch** (§14.5, HP-1): `home_chat` (an owner-only setting) or
//! `K2_HOME_CHAT=1`, default **off** while it is built, the
//! [`crate::voice_preview`] pattern. On, the K5 Space is planned even with
//! the voice rooms preview off ([`k5_space_wanted`]); voice rooms still
//! wait for the voice preview.
//!
//! **Sew chat is for people.** Agents never DM a box person and never reach
//! a K5 channel until the channel grants of S8 exist. Every agent decision
//! about a K5 channel goes through [`agent_may_use_k5_channel`] and every
//! decision about a DM through [`agent_may_read_dm`]; both answer `false`
//! in this build (deny by default).
//!
//! **Badges** (§15.1, HP-26): [`badge`].
//!
//! **Slugs** (HP-25): mgmt's channel create is idempotent on the slug, so a
//! reused slug would hand back someone else's channel. K2 makes every Home
//! channel's slug itself, random ([`random_slug`]); the name is separate.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// The env var that turns Home chat on (`1`/`true`/`yes`/`on`).
pub const ENV: &str = "K2_HOME_CHAT";
/// The feature key `/boot-status` lists while Home chat is on and Sew runs.
pub const FEATURE: &str = "home-chat-v1";
/// Every Home channel slug starts with this (Sew slugs: `[a-z0-9][a-z0-9-]*`).
pub const SLUG_PREFIX: &str = "c-";
/// Random characters after the prefix (base 36: about 52 bits).
pub const SLUG_RANDOM_CHARS: usize = 10;
/// The grant kind a future personal-assistant agent would hold to read a
/// person's DMs, made by that person in person (§13.2). Reserved: nothing
/// writes or reads it in this build.
pub const DM_READ_GRANT_KIND: &str = "chat-dm-read";
/// The grant kind of an agent's access to one K5 channel (S8). Reserved
/// here so the deny-by-default gate names what S8 will check.
pub const CHANNEL_GRANT_KIND: &str = "chat-channel";
/// A count shown as "99+" (Sew caps `unread_count` at 99).
pub const BADGE_CAP: u32 = 99;

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `home_chat` setting into this process.
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// True when Home chat is on: [`ENV`] or the persisted switch.
pub fn enabled() -> bool {
    if let Ok(v) = std::env::var(ENV) {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    SETTING_ENABLED.load(Ordering::Relaxed)
}

/// The K5 Space is planned when either switch is on (HP-1).
pub fn k5_space_wanted() -> bool {
    crate::voice_preview::enabled() || enabled()
}

/// A fresh random Home channel slug, `c-` and ten base-36 characters.
pub fn random_slug() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let mut n = u128::from_be_bytes(bytes);
    let mut out = String::with_capacity(SLUG_PREFIX.len() + SLUG_RANDOM_CHARS);
    out.push_str(SLUG_PREFIX);
    for _ in 0..SLUG_RANDOM_CHARS {
        out.push(ALPHABET[(n % 36) as usize] as char);
        n /= 36;
    }
    out
}

/// Is `s` a slug [`random_slug`] could have made?
pub fn is_home_slug(s: &str) -> bool {
    s.strip_prefix(SLUG_PREFIX)
        .is_some_and(|r| r.len() == SLUG_RANDOM_CHARS && r.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()))
}

/// A channel name as Home takes it: trimmed, no control characters, 1–100
/// characters.
pub fn valid_channel_name(name: &str) -> Result<String, String> {
    let t: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    match t.chars().count() {
        0 => Err("a channel needs a name".into()),
        n if n > 100 => Err("a channel name is at most 100 characters".into()),
        _ => Ok(t),
    }
}

// ── badges ───────────────────────────────────────────────────────────

/// What one conversation's Home row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Badge {
    None,
    /// Unread, no number.
    Dot,
    /// A number, at most [`BADGE_CAP`] (shown "99+" at the cap).
    Count { n: u32 },
}

/// The badge rule (§15.1, HP-26):
/// - with Sew's `unread_count` (flag `read_state_unread_count`): its number
///   for DMs and channels alike;
/// - before it: a channel shows a dot; a DM shows its `mention_count`,
///   which Sew raises for every DM message (a muted DM counts only direct
///   mentions, so it can be unread with a count of 0: a dot).
///
/// A channel's `mention_count` is never shown: v1 has no @mentions (D18).
pub fn badge(dm: bool, unread: bool, mention_count: u32, unread_count: Option<u32>, flag_unread_count: bool) -> Badge {
    if flag_unread_count {
        if let Some(n) = unread_count {
            return match n.min(BADGE_CAP) {
                0 if unread => Badge::Dot,
                0 => Badge::None,
                n => Badge::Count { n },
            };
        }
    }
    if !unread {
        return Badge::None;
    }
    if dm && mention_count > 0 {
        Badge::Count { n: mention_count.min(BADGE_CAP) }
    } else {
        Badge::Dot
    }
}

// ── agents (§13.1, §13.2) ────────────────────────────────────────────

/// Is `ext` (a Sew external id) one of this box's people: the owner or a
/// Connect login? Agents (`k2:workspace:`), App users (`k2:principal:`)
/// and anything else are not.
pub fn is_box_person(ext: &str) -> bool {
    ext == "k2:owner" || ext.starts_with("k2:connect:")
}

/// May the agent `workspace_uuid` read the DMs of the box person
/// `person_ext`? The hook for future personal-assistant agents (a
/// [`DM_READ_GRANT_KIND`] grant the person makes in person). Always false
/// in this build: every agent DM decision goes through here.
pub fn agent_may_read_dm(workspace_uuid: &str, person_ext: &str) -> bool {
    let _ = (workspace_uuid, person_ext);
    false
}

/// May the agent `workspace_uuid` read or post in the K5 channel
/// `channel_id`? S8 turns this into a [`CHANNEL_GRANT_KIND`] grant check.
/// Until then: never (deny by default).
pub fn agent_may_use_k5_channel(workspace_uuid: &str, channel_id: &str) -> bool {
    let _ = (workspace_uuid, channel_id);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_on_by_env_or_setting() {
        let _g = crate::themes::HOME_LOCK.lock();
        let env = crate::test_env::EnvVar::remove(ENV);
        let voice = crate::test_env::EnvVar::remove(crate::voice_preview::ENV);
        set_enabled(false);
        crate::voice_preview::set_enabled(false);
        assert!(!enabled(), "off by default");
        assert!(!k5_space_wanted(), "no switch: no K5 Space");
        for on in ["1", "true", "YES", " on "] {
            let _e = crate::test_env::EnvVar::set(ENV, on);
            assert!(enabled(), "{on}");
            assert!(k5_space_wanted(), "Home chat alone brings the K5 Space up ({on})");
        }
        set_enabled(true);
        assert!(enabled() && k5_space_wanted(), "the setting alone");
        set_enabled(false);
        crate::voice_preview::set_enabled(true);
        assert!(!enabled(), "the voice preview is not Home chat");
        assert!(k5_space_wanted(), "the voice preview alone still plans the Space");
        crate::voice_preview::set_enabled(false);
        drop(voice);
        drop(env);
    }

    #[test]
    fn random_slugs_are_well_formed_and_never_repeat() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20_000 {
            let s = random_slug();
            assert!(is_home_slug(&s), "{s}");
            assert!(crate::tool_hook::sew_wire::is_channel_slug(&s), "{s} is a Sew slug");
            assert!(seen.insert(s.clone()), "slug {s} repeated");
        }
        assert!(!is_home_slug("general"));
        assert!(!is_home_slug("c-short"));
        assert!(!is_home_slug("c-ABCDEFGHIJ"));
    }

    #[test]
    fn channel_names_are_checked() {
        assert_eq!(valid_channel_name("  Design  ").unwrap(), "Design");
        assert!(valid_channel_name(" \n ").is_err());
        assert_eq!(valid_channel_name(&"x".repeat(100)).unwrap().len(), 100);
        assert!(valid_channel_name(&"x".repeat(101)).is_err());
        assert_eq!(valid_channel_name("a\u{7}b").unwrap(), "ab");
    }

    /// The badges rule: a channel's dot, a DM's mention_count, and Sew's
    /// unread_count for both once the flag shows. A channel's mention_count
    /// never shows.
    #[test]
    fn badges_follow_the_flag() {
        // Before `read_state_unread_count`.
        assert_eq!(badge(false, true, 7, None, false), Badge::Dot, "a channel shows a dot, never its mentions");
        assert_eq!(badge(false, false, 7, None, false), Badge::None);
        assert_eq!(badge(true, true, 3, None, false), Badge::Count { n: 3 }, "a DM counts every message in mention_count");
        assert_eq!(badge(true, true, 0, None, false), Badge::Dot, "a muted DM: unread with no number");
        assert_eq!(badge(true, false, 0, None, false), Badge::None);
        assert_eq!(badge(true, true, 250, None, false), Badge::Count { n: 99 });
        // A count Sew sends before K2 saw the flag is not used.
        assert_eq!(badge(false, true, 0, Some(5), false), Badge::Dot);
        // With the flag.
        assert_eq!(badge(false, true, 9, Some(5), true), Badge::Count { n: 5 }, "a channel's number, mentions ignored");
        assert_eq!(badge(true, true, 9, Some(2), true), Badge::Count { n: 2 }, "a DM's number is unread_count now");
        assert_eq!(badge(false, true, 0, Some(140), true), Badge::Count { n: 99 }, "99 = 99+");
        assert_eq!(badge(false, false, 0, Some(0), true), Badge::None);
        assert_eq!(badge(false, true, 0, Some(0), true), Badge::Dot, "marked unread with nothing new");
        // The flag without a count on this read state: today's rule.
        assert_eq!(badge(true, true, 4, None, true), Badge::Count { n: 4 });
    }

    #[test]
    fn agents_reach_no_dm_and_no_k5_channel() {
        for ext in ["k2:owner", "k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e"] {
            assert!(is_box_person(ext), "{ext}");
            assert!(!agent_may_read_dm("0f8fad5b-d9cb-469f-a165-70867728950e", ext), "{ext}");
        }
        for ext in ["k2:workspace:0f8fad5b-d9cb-469f-a165-70867728950e", "k2:principal:abc", "k2:server", "local:x"] {
            assert!(!is_box_person(ext), "{ext}");
        }
        assert!(!agent_may_use_k5_channel("0f8fad5b-d9cb-469f-a165-70867728950e", "9001"));
    }
}
