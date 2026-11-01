//! K2 voice rooms (prd-k2-voice-rooms-v1 v1.9): the tunnel preview switch,
//! the room count's bounds and names, and the hosting states the Admin
//! Space's rooms follow. The Space and rooms themselves are
//! [`crate::tool_hook::admin_space`].
//!
//! **Dark by default.** The voice rooms preview is
//! [`crate::voice_preview`] (`voice_rooms_preview` / `K2_VOICE_ROOMS`):
//! with it off nothing is created and nothing changes.
//! `voice_tunnel_preview` / `K2_VOICE_TUNNEL` gates voice through the
//! tunnel (lane A), the same `compute_preview` pattern: a persisted
//! owner-only setting mirrored into the process, or the env var forcing it
//! on. Whether this box can host (Pro) is the daemon's Pro gate
//! (`sew::pro_gate::hosting`), handed to the plan as [`Hosting`].

use std::sync::atomic::{AtomicBool, Ordering};

/// The most rooms a server hosts (§3.1).
pub const ROOMS_MAX: u32 = 10;
/// Rooms stored at setup and on upgrade (§3.1).
pub const ROOMS_DEFAULT: u32 = 1;
/// Each room's `user_limit` (§3.1; the box's call capacity is the real limit).
pub const ROOM_USER_LIMIT: u32 = 25;
/// Longest room name, characters (Sew's channel name limit).
pub const ROOM_NAME_MAX_CHARS: usize = 100;

static TUNNEL_SETTING: AtomicBool = AtomicBool::new(false);

fn env_on(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Mirror the persisted `voice_tunnel_preview` into the process.
pub fn set_tunnel_enabled(on: bool) {
    TUNNEL_SETTING.store(on, Ordering::Relaxed);
}

/// Voice through the tunnel is on: `K2_VOICE_TUNNEL` or the setting.
pub fn tunnel_enabled() -> bool {
    env_on("K2_VOICE_TUNNEL") || TUNNEL_SETTING.load(Ordering::Relaxed)
}

/// Room `n`'s slug (never changes): `room-<n>`.
pub fn room_slug(n: u32) -> String {
    format!("room-{n}")
}

/// Room `n`'s default name: `Room <n>`.
pub fn room_default_name(n: u32) -> String {
    format!("Room {n}")
}

/// `room-<n>` → `n` (1–10).
pub fn room_number(slug: &str) -> Option<u32> {
    slug.strip_prefix("room-")
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=ROOMS_MAX).contains(n) && slug == room_slug(*n))
}

/// A room name an owner may set: 1–100 characters, trimmed, no control
/// characters.
pub fn valid_room_name(name: &str) -> Result<String, String> {
    let t = name.trim();
    let n = t.chars().count();
    if n == 0 || n > ROOM_NAME_MAX_CHARS {
        return Err(format!("a room name is 1-{ROOM_NAME_MAX_CHARS} characters"));
    }
    if t.chars().any(char::is_control) {
        return Err("a room name has no control characters".into());
    }
    Ok(t.to_string())
}

/// Can this box host rooms (PRD §4.5 `hosting.state`, §8)? The daemon reads
/// it from its Pro gate (`sew::pro_gate::hosting`) before each plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hosting {
    /// Not Pro (or the tier isn't known): rooms are dormant (count kept;
    /// live rooms go `closing`, §3.4 "Losing Pro").
    NotPro,
    /// Pro: the stored count of rooms is hosted.
    Ok,
}

/// Tests: a hosting state that stands in for the Pro gate (`None`: the
/// gate decides). Never compiled into a release build.
#[cfg(any(test, feature = "test-util"))]
static HOSTING_OVERRIDE: std::sync::Mutex<Option<Hosting>> = std::sync::Mutex::new(None);

#[cfg(any(test, feature = "test-util"))]
pub fn set_hosting_for_tests(h: Option<Hosting>) {
    *HOSTING_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = h;
}

/// The test stand-in, if a test set one.
pub fn hosting_override() -> Option<Hosting> {
    #[cfg(any(test, feature = "test-util"))]
    {
        *HOSTING_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner())
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_names_and_bounds() {
        assert_eq!(room_slug(1), "room-1");
        assert_eq!(room_default_name(10), "Room 10");
        assert_eq!(room_number("room-10"), Some(10));
        for bad in ["room-0", "room-11", "room-01", "room-x", "Room-1", "room-"] {
            assert_eq!(room_number(bad), None, "{bad}");
        }
        assert_eq!(valid_room_name("  Standup ").unwrap(), "Standup");
        assert!(valid_room_name("   ").is_err());
        assert!(valid_room_name(&"x".repeat(101)).is_err());
        assert!(valid_room_name("a\nb").is_err());
        assert_eq!((ROOMS_DEFAULT, ROOMS_MAX, ROOM_USER_LIMIT), (1, 10, 25));
    }

    #[test]
    fn tunnel_preview_defaults_off_and_follows_env_or_setting() {
        let _env = crate::test_env::lock();
        let _b = crate::test_env::EnvVar::remove("K2_VOICE_TUNNEL");
        set_tunnel_enabled(false);
        assert!(!tunnel_enabled(), "default OFF");
        set_tunnel_enabled(true);
        assert!(tunnel_enabled());
        set_tunnel_enabled(false);
        {
            let _on = crate::test_env::EnvVar::set("K2_VOICE_TUNNEL", "1");
            assert!(tunnel_enabled());
        }
        assert!(!tunnel_enabled());
    }
}
