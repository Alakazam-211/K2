//! What a custom Garden widget may do (prd-zen-user-widgets-v2, as changed
//! by Rosson's 0.45.1 smoke, 2026-10-08: "If they make a widget, just let
//! it send stuff. This is their personal garden, there's no need for
//! permissions.").
//!
//! **No permissions for your own widgets.** A widget that exists in your
//! own Garden on this computer (a folder under `~/.k2/zen/widgets/` or a
//! built-in `k2:<name>@<n>`) is [`WidgetOrigin::Local`] and gets every
//! Garden-safe cap its manifest asks for ([`effective_caps`]: requested ∩
//! [`USER_WIDGET_CAPS`]). There is no review card, grant dialog, scope
//! picker or Sending switch. Which agents it reaches is the Garden's
//! reach, resolved by the renderer (this computer's agents plus the rows
//! on your Homes; `zen-custom-scope.ts`), and every server still checks
//! your role.
//!
//! **The v4 seam.** Widgets imported from other people are not built.
//! When they are, they get another [`WidgetOrigin`], and
//! [`effective_caps`] gives them nothing until a review exists. Today
//! every widget the daemon can see is `Local`.
//!
//! **Rails that stay** (invisible, not permissions): the sealed frame; the
//! runaway guard (the renderer counts posts; the daemon holds the pause in
//! memory, [`WidgetPauses`], so every window sees it and one Resume clears
//! it); an audit line per widget post (`overlay_routes`); owner-only routes
//! stay owner-only.
//!
//! **What happened to grants.** Table `zen_widget_grants` (migration 0138)
//! stays in the schema so existing databases keep their migration list,
//! but nothing reads or writes it, and `~/.k2/zen-grant.key` is no longer
//! made or read. `grants.json` stays refused as a target (`ZenFile::parse`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::USER_WIDGET_CAPS;

/// Files agents must never write (UWB26, as amended by
/// prd-zen-garden-sync-defaults-v1 SD8/GT1): the sync state, its news and
/// saved defaults sets. Paths under `~/.k2/zen/`.
pub const NEVER_WRITE: &[&str] = &["sync.json", "news.json", ".defaults/"];

/// Where a widget came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetOrigin {
    /// In your own Garden on this computer: a widget folder or a built-in.
    /// Runs with every Garden-safe cap it asks for, no review.
    Local,
}

impl WidgetOrigin {
    /// The origin of a placement's `widget` (folder name or `k2:` ref).
    /// Every widget the daemon can see today is local; v4's imported
    /// widgets will be told apart here.
    pub fn of(_widget: &str) -> Self {
        WidgetOrigin::Local
    }
}

/// The caps a widget runs with: requested ∩ [`USER_WIDGET_CAPS`], in
/// [`USER_WIDGET_CAPS`] order, for a [`WidgetOrigin::Local`] widget. Pure.
pub fn effective_caps(origin: WidgetOrigin, requested: &[String]) -> Vec<String> {
    match origin {
        WidgetOrigin::Local => USER_WIDGET_CAPS
            .iter()
            .filter(|c| requested.iter().any(|r| r == *c))
            .map(|c| c.to_string())
            .collect(),
    }
}

/// Why posting is paused (R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PauseReason {
    /// The renderer's runaway guard tripped: more than 120 posts in 10
    /// minutes, or 20 identical texts to one agent in 10 minutes.
    Runaway,
}

/// A runaway pause on one placement. While set, the widget keeps running
/// but its posts are refused (`sending_off`) and K2 shows "Paused: too
/// many posts. [Resume]" on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pause {
    pub at: String,
    pub reason: PauseReason,
}

/// The daemon's runaway pauses, by `(garden, placement)`. Held in memory
/// (a daemon restart clears them, like the renderer's post counters), read
/// once per request and passed to `resolve` and `refresh`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidgetPauses {
    map: BTreeMap<(String, String), Pause>,
}

impl WidgetPauses {
    /// No pauses; `const`, for the daemon's static.
    pub const fn new() -> Self {
        Self { map: BTreeMap::new() }
    }

    /// No pauses (tests; the sync code's comparisons).
    pub fn empty() -> Self {
        Self::new()
    }

    /// Pause one placement (replaces an older pause).
    pub fn insert(&mut self, garden: &str, placement: &str, pause: Pause) {
        self.map.insert((garden.to_string(), placement.to_string()), pause);
    }

    /// Clear one placement's pause. Returns whether it was paused.
    pub fn remove(&mut self, garden: &str, placement: &str) -> bool {
        self.map.remove(&(garden.to_string(), placement.to_string())).is_some()
    }

    /// The placement's pause, if any.
    pub fn get(&self, garden: &str, placement: &str) -> Option<&Pause> {
        self.map.get(&(garden.to_string(), placement.to_string()))
    }

    /// Drop pauses whose placement `keep` says is gone. Returns how many.
    pub fn retain(&mut self, keep: impl Fn(&str, &str) -> bool) -> usize {
        let before = self.map.len();
        self.map.retain(|(g, p), _| keep(g, p));
        before - self.map.len()
    }

    /// Changes whenever a pause is set or cleared (UW12: one `zen_changed`).
    pub fn fingerprint(&self) -> J {
        let rows: Vec<J> = self.map.iter().map(|((g, p), pause)| json!([g, p, pause.at])).collect();
        json!(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_local_widget_gets_every_garden_safe_cap_it_asks_for() {
        let caps = effective_caps(WidgetOrigin::Local, &s(&["thread:post", "agents:read", "thread:read"]));
        assert_eq!(caps, s(&["agents:read", "thread:read", "thread:post"]), "in USER_WIDGET_CAPS order");
        assert_eq!(effective_caps(WidgetOrigin::Local, &[]), Vec::<String>::new(), "asks for nothing, gets nothing");
    }

    #[test]
    fn effective_caps_never_include_a_non_widget_cap() {
        let caps = effective_caps(WidgetOrigin::Local, &s(&["agents:read", "files:write", "zen:exit", "tickets:read"]));
        assert_eq!(caps, s(&["agents:read"]));
    }

    #[test]
    fn every_widget_today_is_local() {
        assert_eq!(WidgetOrigin::of("agent-arcade"), WidgetOrigin::Local);
        assert_eq!(WidgetOrigin::of("k2:diary@1"), WidgetOrigin::Local);
        assert_eq!(serde_json::to_value(WidgetOrigin::Local).expect("json"), json!("local"));
    }

    #[test]
    fn pauses_set_clear_and_move_the_fingerprint() {
        let mut p = WidgetPauses::empty();
        let empty = p.fingerprint();
        p.insert("g-1", "arcade", Pause { at: "2026-10-08T00:00:00Z".into(), reason: PauseReason::Runaway });
        assert_eq!(p.get("g-1", "arcade").expect("paused").reason, PauseReason::Runaway);
        assert!(p.get("g-1", "other").is_none());
        assert_ne!(p.fingerprint(), empty, "a pause moves the fingerprint");
        assert_eq!(
            serde_json::to_value(p.get("g-1", "arcade").expect("paused")).expect("json"),
            json!({"at": "2026-10-08T00:00:00Z", "reason": "runaway"})
        );
        assert!(p.remove("g-1", "arcade"));
        assert!(!p.remove("g-1", "arcade"), "already clear");
        assert_eq!(p.fingerprint(), empty);
    }

    #[test]
    fn retain_drops_pauses_of_gone_placements() {
        let mut p = WidgetPauses::empty();
        let pause = Pause { at: "t".into(), reason: PauseReason::Runaway };
        p.insert("g-1", "a", pause.clone());
        p.insert("g-2", "b", pause);
        assert_eq!(p.retain(|g, _| g == "g-1"), 1);
        assert!(p.get("g-1", "a").is_some());
        assert!(p.get("g-2", "b").is_none());
    }

    #[test]
    fn user_widget_caps_match_the_catalog() {
        use crate::contract::{catalog, Exposure};
        assert_eq!(catalog().caps_exposed_to(Exposure::Widget), USER_WIDGET_CAPS);
        for c in USER_WIDGET_CAPS {
            assert!(super::super::BRIDGE_CAPS.contains(c), "{c} is a bridge cap");
        }
    }

    #[test]
    fn never_write_lists_the_daemon_owned_files() {
        assert_eq!(NEVER_WRITE, &["sync.json", "news.json", ".defaults/"]);
    }
}
