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
/// **Static taint (Garden v3, GV48).** A widget that asks for
/// `sources:read` never gets `thread:post` (so no `thread.post`,
/// `thread.answer` or `compose.draft`): a card that reads a server's data
/// could otherwise post it to an agent anywhere on your Homes. K2's own
/// `sources.ask` still opens the Ask box, with a draft K2 writes. The
/// renderer applies the same rule (`zen-custom-bridge.ts`), and
/// `k2 zen validate` warns `sources_drops_thread_post`.
///
/// **Garden data taint** (prd-gardens-local-services-v1 §5.6,
/// contract-garden-data-v1 §7.1 step 4): the same widget never gets
/// `store:write` either, so it can't copy source values into a keyspace an
/// agent can read. It keeps `store:read`, `save.*` and `viewer.get`
/// (`save:` is never granted to anyone). Validate warns
/// `sources_drops_store_write`.
pub fn effective_caps(origin: WidgetOrigin, requested: &[String]) -> Vec<String> {
    let reads_sources = requested.iter().any(|r| r == SOURCES_READ);
    match origin {
        WidgetOrigin::Local => USER_WIDGET_CAPS
            .iter()
            .filter(|c| requested.iter().any(|r| r == *c))
            .filter(|c| !(reads_sources && TAINTED_BY_SOURCES_ALL.contains(*c)))
            .map(|c| c.to_string())
            .collect(),
    }
}

/// The cap that lets a card read data sources (contract-sources-v1 §6).
pub const SOURCES_READ: &str = crate::sources::SOURCES_READ_CAP;

/// The cap a `sources:read` widget never runs with (GV48).
pub const TAINTED_BY_SOURCES: &str = "thread:post";

/// The Garden data cap a `sources:read` widget never runs with.
pub const STORE_WRITE_TAINTED: &str = "store:write";

/// Every cap a `sources:read` widget loses.
pub const TAINTED_BY_SOURCES_ALL: &[&str] = &[TAINTED_BY_SOURCES, STORE_WRITE_TAINTED];

/// The `k2 zen validate` warning code for a manifest that asks for both.
pub const SOURCES_DROPS_THREAD_POST: &str = "sources_drops_thread_post";

/// The `k2 zen validate` warning for `sources:read` + `store:write`.
pub const SOURCES_DROPS_STORE_WRITE: &str = "sources_drops_store_write";

/// 403 code: the post names a placement this computer's Gardens don't
/// hold, a widget that isn't the one placed there, or a hash that isn't the
/// placement's live (approved) bundle.
pub const WIDGET_NOT_APPROVED: &str = "widget_not_approved";

/// 403 code: the placement's approved caps ([`effective_caps`] of its live
/// manifest, after the Sources taint) don't include the cap the call needs.
pub const WIDGET_CAP_MISSING: &str = "widget_cap_missing";

/// Why the daemon refused a widget's portable call (a Thread post or
/// answer) on the placement it claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementRefusal {
    /// [`WIDGET_NOT_APPROVED`] or [`WIDGET_CAP_MISSING`].
    pub code: &'static str,
    pub message: String,
}

/// Does this computer's Zen folder hold the Garden with this exact id?
/// (A widget post naming another computer's Garden was checked by that
/// computer's daemon; see `overlay_routes::widget_post_gate`.)
pub fn holds_garden(f: &super::ZenFiles, garden: &str) -> bool {
    f.is_set_up() && f.gardens().iter().any(|g| g.id == garden)
}

/// The daemon's own check of a widget's portable call (0.46.1 polish):
/// the placement `garden`/`placement` must exist on this computer, place
/// `widget`, and run the live bundle `hash` (a locked Garden's frozen
/// copy); its approved caps ([`effective_caps`] of that manifest) must
/// include `cap`. Returns the approved caps. Same resolution as the Garden
/// data door (`/cli/zen/call`), so a widget is judged the same way
/// whichever door it uses. K2's built-in `k2:<name>@<n>` widgets are
/// placements like any other and pass on their own manifests' caps.
pub fn check_placement_cap(
    f: &super::ZenFiles,
    garden: &str,
    placement: &str,
    widget: &str,
    hash: &str,
    cap: &str,
) -> Result<Vec<String>, PlacementRefusal> {
    let not_approved = |message: String| PlacementRefusal { code: WIDGET_NOT_APPROVED, message };
    if !holds_garden(f, garden) {
        return Err(not_approved(format!("no Garden '{garden}' on this computer holds that widget")));
    }
    let p = f
        .custom_placements()
        .into_iter()
        .find(|p| p.garden == garden && p.placement == placement)
        .ok_or_else(|| not_approved(format!("Garden '{garden}' has no custom widget '{placement}'")))?;
    if p.widget != widget {
        return Err(not_approved(format!("'{placement}' places '{}', not '{widget}'", p.widget)));
    }
    let st = f.garden_widget_status(garden, &p.widget);
    let live = st.hash();
    if live.is_empty() {
        return Err(not_approved(format!("'{widget}' has no good version now")));
    }
    if live != hash {
        return Err(not_approved(format!("'{widget}' changed since its frame loaded; K2 reloads the widget")));
    }
    let caps = effective_caps(WidgetOrigin::of(&p.widget), &st.requested());
    if !caps.iter().any(|c| c == cap) {
        let why = if cap == TAINTED_BY_SOURCES && st.requested().iter().any(|r| r == SOURCES_READ) {
            format!("'{widget}' reads data sources, so it runs without {cap}")
        } else {
            format!("'{widget}' needs {cap} in its manifest caps")
        };
        return Err(PlacementRefusal { code: WIDGET_CAP_MISSING, message: why });
    }
    Ok(caps)
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

    /// GV48: asking for sources:read drops thread:post, whatever else is asked.
    #[test]
    fn sources_read_drops_thread_post() {
        let caps = effective_caps(WidgetOrigin::Local, &s(&["thread:post", "sources:read", "thread:read", "agents:read"]));
        assert_eq!(caps, s(&["agents:read", "thread:read", "sources:read"]));
        let plain = effective_caps(WidgetOrigin::Local, &s(&["thread:post"]));
        assert_eq!(plain, s(&["thread:post"]), "without sources:read nothing changes");
    }

    /// Garden data: asking for sources:read drops store:write too, and keeps
    /// store:read (contract-garden-data-v1 §7.1 step 4).
    #[test]
    fn sources_read_drops_store_write_and_keeps_store_read() {
        let caps = effective_caps(WidgetOrigin::Local, &s(&["store:write", "store:read", "sources:read"]));
        assert_eq!(caps, s(&["sources:read", "store:read"]));
        let plain = effective_caps(WidgetOrigin::Local, &s(&["store:write", "store:read"]));
        assert_eq!(plain, s(&["store:read", "store:write"]), "without sources:read a widget keeps both");
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
