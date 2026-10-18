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
/// saved defaults sets, and locked Gardens' frozen copies (prd-garden-lock-
/// and-sync-v1 §5.1). Paths under `~/.k2/zen/`.
pub const NEVER_WRITE: &[&str] = &["sync.json", "news.json", ".defaults/", ".locks/"];

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
///
/// **Sources widgets keep what they ask for (Rosson 2026-10-17, 0.46.1).**
/// Garden v3 (GV48) dropped `thread:post` and `store:write` from a widget
/// that asks for `sources:read`. That split Fleet Ops cards in two (the
/// Decisions card couldn't read the k2-watch list and keep its Draft button
/// and editable lists). Rosson's ruling: a widget that reads sources may
/// also post to the Thread and write its own store, **within the caps its
/// manifest asks for**: the manifest is the per-widget cap list the owner
/// approves by placing the widget in their own Garden. What stays:
///
/// - a cap the manifest doesn't ask for is refused (`cap_not_granted`);
/// - the rails: the runaway guard and its pause, the per-placement write
///   budgets and the runaway-writes pause (`garden_data::budget`), one
///   audit line per widget post (`zen.widget.post`), and the maker's rights
///   on its keyspace;
/// - `k2 zen validate` and `k2 zen widget show` say plainly that source
///   values can reach the Thread or the widget's shared data
///   ([`sources_reach_note`]), so the person sees it before placing it.
pub fn effective_caps(origin: WidgetOrigin, requested: &[String]) -> Vec<String> {
    match origin {
        WidgetOrigin::Local => USER_WIDGET_CAPS
            .iter()
            .filter(|c| requested.iter().any(|r| r == *c))
            .map(|c| c.to_string())
            .collect(),
    }
}

/// The cap that lets a card read data sources (contract-sources-v1 §6).
pub const SOURCES_READ: &str = crate::sources::SOURCES_READ_CAP;

/// The cap that lets a widget post to the Thread.
pub const THREAD_POST: &str = "thread:post";

/// The Garden data write cap.
pub const STORE_WRITE: &str = "store:write";

/// The `k2 zen validate` note for `sources:read` + `thread:post` (allowed
/// since 0.46.1; the note makes the reach plain).
pub const SOURCES_REACH_THREAD: &str = "sources_reach_thread";

/// The `k2 zen validate` note for `sources:read` + `store:write`.
pub const SOURCES_REACH_STORE: &str = "sources_reach_store";

/// What a sources widget can do with the values it reads, for
/// `widget show`: `None` when it reads no sources or asks for neither cap.
pub fn sources_reach_note(caps: &[String]) -> Option<String> {
    let has = |c: &str| caps.iter().any(|x| x == c);
    if !has(SOURCES_READ) {
        return None;
    }
    match (has(THREAD_POST), has(STORE_WRITE)) {
        (true, true) => Some(
            "reads data sources and may post to the Thread and save to its shared data: source values it shows can reach agents (it asked for thread:post and store:write)".into(),
        ),
        (true, false) => Some(
            "reads data sources and may post to the Thread: source values it shows can reach the agents it posts to (it asked for thread:post)".into(),
        ),
        (false, true) => Some(
            "reads data sources and may save to its shared data: agents granted that data can read what it saves (it asked for store:write)".into(),
        ),
        (false, false) => None,
    }
}

/// Why posting is paused (R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PauseReason {
    /// The renderer's runaway guard tripped: more than 120 posts in 10
    /// minutes, or 20 identical texts to one agent in 10 minutes.
    Runaway,
    /// Garden data's runaway guard tripped (the daemon's, PRD §6.3): the
    /// same key and value written over and over. Its data writes are
    /// refused (`paused`) until the person's Resume.
    Writes,
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

    /// 0.46.1 (Rosson 2026-10-17): a sources reader keeps thread:post and
    /// store:write when its manifest asks for them, and nothing it didn't ask for.
    #[test]
    fn sources_read_keeps_thread_post_and_store_write_it_asked_for() {
        let caps = effective_caps(WidgetOrigin::Local, &s(&["thread:post", "sources:read", "thread:read", "agents:read"]));
        assert_eq!(caps, s(&["agents:read", "thread:read", "thread:post", "sources:read"]));
        let caps = effective_caps(WidgetOrigin::Local, &s(&["store:write", "store:read", "sources:read"]));
        assert_eq!(caps, s(&["sources:read", "store:read", "store:write"]));
        let only = effective_caps(WidgetOrigin::Local, &s(&["sources:read"]));
        assert_eq!(only, s(&["sources:read"]), "a sources reader gets no post or write it didn't ask for");
    }

    #[test]
    fn the_reach_note_names_what_source_values_can_reach() {
        assert_eq!(sources_reach_note(&s(&["thread:post", "store:write"])), None, "no sources, no note");
        assert_eq!(sources_reach_note(&s(&["sources:read", "store:read"])), None);
        let t = sources_reach_note(&s(&["sources:read", "thread:post"])).expect("thread note");
        assert!(t.contains("post to the Thread") && !t.contains("shared data"), "{t}");
        let w = sources_reach_note(&s(&["sources:read", "store:write"])).expect("store note");
        assert!(w.contains("shared data") && !w.contains("Thread"), "{w}");
        let both = sources_reach_note(&s(&["sources:read", "thread:post", "store:write"])).expect("both");
        assert!(both.contains("Thread") && both.contains("shared data"), "{both}");
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
        assert_eq!(catalog().widget_caps(), USER_WIDGET_CAPS);
        // Every widget cap is a bridge cap, the cap of verbs only the custom
        // layer answers (renderer impl "frame": sources:read), or a Garden
        // data cap the daemon answers (garden-local: store:*).
        let frame_caps: Vec<&str> = catalog()
            .verbs
            .iter()
            .filter(|v| v.bindings.renderer.as_ref().map(|r| r.implementation) == Some(crate::contract::RendererImpl::Frame))
            .filter_map(|v| v.cap.as_deref())
            .collect();
        let local_caps = catalog().caps_exposed_to(Exposure::GardenLocal);
        for c in USER_WIDGET_CAPS {
            assert!(
                super::super::BRIDGE_CAPS.contains(c) || frame_caps.contains(c) || local_caps.contains(c),
                "{c} is a bridge, frame or garden-local cap"
            );
        }
    }

    #[test]
    fn never_write_lists_the_daemon_owned_files() {
        assert_eq!(NEVER_WRITE, &["sync.json", "news.json", ".defaults/", ".locks/"]);
    }
}
