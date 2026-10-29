//! K2's verb catalog (prd-zen-user-widgets-v2 UWA1, UWA9, UWA10, UWB29).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B1.** The row shape below
//! is frozen for the build: B2, B3 and B4 code against these types and the
//! TypeScript mirror (`src/renderer/lib/contract/catalog-types.ts`). Change a
//! field only with the integrator, and change both files in one commit.
//!
//! The catalog is a **verb layer over `ROUTES`**, not a second route table
//! (UWB29): every `http` binding names a `ROUTES` path, rows carry no auth
//! (login floors stay in `route_policy.rs`), and only renderer-only verbs
//! (`gardens.*`, `theme.*`) have no route. When `ROUTES` becomes the catalog
//! later, these rows fold in as its verb columns.
//!
//! The source of truth is the checked-in, language-neutral `catalog.json`
//! next to this file. Everything else is generated from it by B1's
//! `contract-gen` (UWA11): `lib/zen/zen-verbs.generated.ts`,
//! `lib/k2-caps.generated.ts`, `sdk/generated/k2.d.ts`,
//! `sdk/generated/k2-frame.js` and the `k2 zen guide api` block in `cli/k2`.
//! Generated files are never edited by hand.
//!
//! Day 0 ships the shape, the loader, the shared cap table and the shared
//! error list. `verbs` is empty until B1's S1a fills the 37 `ZEN_VERBS` rows
//! plus `theme.changed`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

pub mod app;
pub mod gen;
pub mod libs;
pub mod lint;
pub mod live;
pub mod phone;

use serde::{Deserialize, Serialize};
use serde_json::Value as J;

/// The checked-in catalog (UWA1).
pub const CATALOG_JSON: &str = include_str!("catalog.json");

/// Where [`CATALOG_JSON`] lives, for panic and test messages.
pub const CATALOG_PATH: &str = "crates/k2-core/src/contract/catalog.json";

/// The whole catalog file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    /// Bumped on any change to the file.
    pub catalog_version: u32,
    /// The shared cap table (UWA9): Garden dialog `sentence`s and
    /// Settings → Apps `label`s come from here.
    pub caps: Vec<CapRow>,
    /// The shared error list (UWA10): bridge now, daemon and app gateway in
    /// Cut B.
    pub errors: Vec<ErrorRow>,
    /// Field names no exposed response schema may name (UWA6).
    pub banned_fields: Vec<String>,
    /// One row per verb.
    pub verbs: Vec<VerbRow>,
}

/// Who outside K2's own code may call a verb or hold a cap. An empty or
/// absent list means **none**: K2's built-ins only (UWA1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Exposure {
    /// A custom Garden widget (sealed frame, MessagePort transport).
    Widget,
    /// An App: its server code through its gateway link, or a guest's
    /// browser through the App's helper (runtime S23: one row per live
    /// gateway route; `app_surface.rs` is generated from them).
    App,
    /// A custom Garden widget, through this computer's daemon only
    /// (`POST /cli/zen/call`; prd-gardens-local-services-v1 §3.1,
    /// contract-garden-data-v1 §7.2). Wire name `garden-local`.
    ///
    /// A row exposed to both `app` and `garden-local` is ONE verb with two
    /// transports (room-scope §0.2 "one reveal = both surfaces", first used
    /// by RS-A15 `guests.*`): its `http` binding is the App's live route,
    /// and in a Garden the same verb, args and answer go through
    /// `POST /cli/zen/call`. Such a row is `reach: portable`.
    #[serde(rename = "garden-local")]
    GardenLocal,
}

/// Where a widget verb runs when its Garden is on a paired phone
/// (prd-gardens-on-phone-v1 §4.3, the catalog's `phone` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phone {
    /// The phone's frame host answers it (page verbs: switch, theme,
    /// folds, the native Agents/Conversation on the same page).
    Host,
    /// The phone forwards it to the daemon's remote door
    /// (`POST /cli/zen/remote/call`, or the remote blob routes).
    Door,
    /// Not on a phone in this release: the host answers `verb_unavailable`.
    Later,
    /// Never on a phone: the host answers `not_exposed`.
    Never,
}

impl Phone {
    pub fn as_str(self) -> &'static str {
        match self {
            Phone::Host => "host",
            Phone::Door => "door",
            Phone::Later => "later",
            Phone::Never => "never",
        }
    }
}

/// Whether the same call and shape will work in an App (UWA4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
    Portable,
    /// Garden only; an App build throws `verb_local`.
    Local,
}

/// Read or write. A `write` row's `http` binding must be POST (UWA3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Read,
    Write,
}

/// How a verb answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerbKind {
    /// One request, one answer (a Promise in the frame).
    Call,
    /// Pushes values until unsubscribed (`k2.subscribe`, returns unsubscribe).
    Subscribe,
    /// Pushed by the host with no request (`theme.changed`).
    Event,
}

/// One verb (UWA1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerbRow {
    /// `surface.action`, e.g. `thread.post`.
    pub verb: String,
    /// A cap from [`Catalog::caps`], or null for a no-cap verb.
    pub cap: Option<String>,
    /// More caps the verb ALSO needs, checked like `cap` (0.46.5 activity
    /// SDK: the Thread ear needs `thread:read` beside `activity:read`, as
    /// its route always checked). Absent = `cap` alone. Never on a no-cap
    /// row, never repeats `cap`.
    #[serde(default, rename = "alsoCaps", skip_serializing_if = "Vec::is_empty")]
    pub also_caps: Vec<String>,
    /// Empty or absent = none (K2 built-ins only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposure: Vec<Exposure>,
    /// Where the row runs for a Garden on a paired phone
    /// (prd-gardens-on-phone-v1 §4.3): set on every row exposed to
    /// `widget` or `garden-local`, absent on every other row. See [`phone`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<Phone>,
    /// Where the row runs for a Garden that became an App
    /// (plan-gardens-tauri-compatible-v1 GTC-1): `door`, `host`, `later` or
    /// `never`, the same words as `phone`. Set on the Workbench's rows
    /// (catalog v40) first; the lint that requires it everywhere is GT-1's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<Phone>,
    pub reach: Reach,
    pub effect: Effect,
    pub kind: VerbKind,
    /// The `/boot-status` feature key that brought the verb (`zen-v1`,
    /// `zen-gardens-v1`, `zen-widgets-v1`, …).
    pub feature: String,
    /// At least one binding.
    pub bindings: Bindings,
    /// JSON Schema of the arguments, as an array (one schema per argument).
    pub request: J,
    /// JSON Schema of the answer (or of each pushed value). Closed
    /// (`additionalProperties: false` at every level) on exposed rows.
    pub response: J,
    /// Codes from [`Catalog::errors`].
    pub errors: Vec<String>,
    /// One line.
    pub doc: String,
    /// One line.
    pub example: String,
    /// 0.46.5: the row's place on `k2 study live` (a live surface), or
    /// absent. See [`live`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveRow>,
}

/// A row's place on `k2 study live` (0.46.5 activity SDK, review §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRow {
    /// The surface, a `live-topics.json` topic (`thread-turn`).
    pub topic: String,
    /// Other surfaces the same row also feeds (`thread.stream` carries
    /// messages and, with the ear, the Thread turn).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_topics: Vec<String>,
    /// Which line of the block the row is on.
    pub role: LiveRole,
    /// A live row: the read verbs to call on every (re)open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<String>,
    /// The counts the answer carries, name → what it counts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, String>,
    /// At most two use ideas, ≤ 80 characters each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<String>,
    /// What never to infer, ≤ 100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub never: Option<String>,
    /// A ready-made helper for this row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveRole {
    /// The state now (a call).
    Read,
    /// What changes (a subscribe row).
    Subscribe,
    /// A helper row.
    Ready,
}

impl VerbRow {
    /// True when `who` may call this verb.
    pub fn exposed_to(&self, who: Exposure) -> bool {
        self.exposure.contains(&who)
    }

    /// Every cap a caller needs: `cap`, then `alsoCaps`. Empty for a no-cap row.
    pub fn all_caps(&self) -> Vec<&str> {
        self.cap.iter().chain(self.also_caps.iter()).map(String::as_str).collect()
    }
}

/// Where a verb is implemented. At least one is set.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bindings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<RendererBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<SocketBinding>,
}

impl Bindings {
    pub fn is_empty(&self) -> bool {
        self.renderer.is_none() && self.http.is_none() && self.socket.is_none()
    }
}

/// A Garden verb's renderer side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RendererBinding {
    #[serde(rename = "impl")]
    pub implementation: RendererImpl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererImpl {
    /// Built into `zen-bridge.ts` (`BUILTIN_VERBS`).
    Builtin,
    /// Registered by `registerZenVerb` (`zen-data.ts`, `zen-app-nav.ts`,
    /// `zen-add-agent.ts`).
    Registered,
    /// Handled only by the custom-widget frame host (`theme.changed`,
    /// `canvas.setHitRegions`, `window.startDrag`).
    /// Never in the generated `ZEN_VERBS`.
    Frame,
}

/// The daemon route a verb reaches. `route` must be a `ROUTES` row (UWA2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpBinding {
    pub method: HttpMethod,
    pub route: String,
    /// Argument name → query or body name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// `app` rows (S23, room-scope RS7): response field names the catalog
    /// renames from the live wire, catalog name → wire name, at any depth
    /// (`relPath` ← `path`). The server SDK renames back; the lint checks
    /// only the catalog names.
    #[serde(default, rename = "responseNames", skip_serializing_if = "BTreeMap::is_empty")]
    pub response_names: BTreeMap<String, String>,
    /// `app` POST rows: arguments (catalog names) the route reads from the
    /// query string; every other argument goes in the JSON body. A GET
    /// sends every argument in the query.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<String>,
    /// `app` rows: which door forwards the route. Absent = both (the
    /// browser helper and the App's gateway link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub door: Option<Door>,
    /// `app` GET rows: `false` = no HEAD twin (the route is one exact GET).
    /// Absent = a GET row also opens HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<bool>,
}

/// The one door an `app` row is limited to (absent = both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Door {
    /// Only the browser helper (`--skin-gateway`): the route acts on a
    /// guest's own login session, which an App pass never is.
    Helper,
    /// Only the App's gateway link: a BFF's server-side call (a guest's
    /// login through the App) that the browser origin never forwards.
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
}

/// A socket a `subscribe` row listens on. Sockets are read-only (UWA3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocketBinding {
    pub path: String,
    /// The frame kind on that socket (`thread`, `activity_changed`, …).
    pub frame: String,
    /// The verb to refetch with after a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refetch: Option<String>,
    /// `app` rows: the query parameter the socket must carry (GA1: a
    /// socket can't be App-facing without naming it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// `app` rows: fixed query parameters the SDK always adds (0.46.3:
    /// `ear=1`, the guest ear's per-connection opt-in on the Thread
    /// sockets; a raw client that omits it gets today's frames).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
}

/// One cap (UWA9, UW23).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapRow {
    /// `agents:read`, `thread:post`, …
    pub name: String,
    /// Empty = only K2's built-ins hold it (`gardens:manage`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposure: Vec<Exposure>,
    /// Short words, read by Settings → Apps (today's `SKIN_CAP_LABELS`,
    /// unchanged for app caps).
    pub label: String,
    /// K2's sentence in the Garden review dialog, with a `{where}` slot (a
    /// scope's words in a Garden, a room in an App). Set on every cap
    /// exposed to `widget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence: Option<String>,
}

/// One shared error code (UWA10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorRow {
    pub code: String,
    /// Extra fields the error carries (`cap`, `room`, `feature`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    /// Worth trying again unchanged (0.46.1, catalog 11): a handler-object
    /// subscription re-subscribes on its own after one of these.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retryable: bool,
    pub doc: String,
}

/// Parse a catalog text. Strict: unknown keys are errors.
pub fn parse(src: &str) -> Result<Catalog, serde_json::Error> {
    serde_json::from_str(src)
}

/// The checked-in catalog, parsed once. A bad file panics with its path and
/// the serde error; the contract tests run this load, so a bad file fails
/// `cargo test` before it ever reaches a daemon.
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| parse(CATALOG_JSON).unwrap_or_else(|e| panic!("{CATALOG_PATH} does not parse: {e}")))
}

impl Catalog {
    pub fn verb(&self, verb: &str) -> Option<&VerbRow> {
        self.verbs.iter().find(|v| v.verb == verb)
    }

    pub fn cap(&self, name: &str) -> Option<&CapRow> {
        self.caps.iter().find(|c| c.name == name)
    }

    pub fn error(&self, code: &str) -> Option<&ErrorRow> {
        self.errors.iter().find(|e| e.code == code)
    }

    /// Cap names exposed to `who`, in file order. For `Widget` this is
    /// `USER_WIDGET_CAPS` (asserted equal in `zen::widget_access` tests).
    pub fn caps_exposed_to(&self, who: Exposure) -> Vec<&str> {
        self.caps.iter().filter(|c| c.exposure.contains(&who)).map(|c| c.name.as_str()).collect()
    }

    /// Verb rows exposed to `who`, in file order.
    pub fn verbs_exposed_to(&self, who: Exposure) -> Vec<&VerbRow> {
        self.verbs.iter().filter(|v| v.exposed_to(who)).collect()
    }

    /// Every row a custom widget may call, in file order: rows exposed to
    /// `widget` (the bridge) or to `garden-local` (this computer's daemon
    /// through `/cli/zen/call`, contract-garden-data-v1 §7.2). This is
    /// `ZEN_CUSTOM_VERBS`, the frame's table and the guide's list.
    pub fn widget_callable_verbs(&self) -> Vec<&VerbRow> {
        self.verbs.iter().filter(|v| v.exposed_to(Exposure::Widget) || v.exposed_to(Exposure::GardenLocal)).collect()
    }

    /// Caps a custom widget may ask for, in file order: exposed to
    /// `widget` or `garden-local`. Equal to `zen::USER_WIDGET_CAPS`.
    pub fn widget_caps(&self) -> Vec<&str> {
        self.caps
            .iter()
            .filter(|c| c.exposure.contains(&Exposure::Widget) || c.exposure.contains(&Exposure::GardenLocal))
            .map(|c| c.name.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_catalog_loads() {
        let c = catalog();
        assert!(c.catalog_version >= 1, "catalogVersion starts at 1");
        assert!(!c.caps.is_empty());
        assert!(!c.errors.is_empty());
    }

    /// TUWA1/TUWA10 (UWA9): the Rust cap lists agree with the catalog.
    /// `BRIDGE_CAPS` = the caps of every row with a builtin or registered
    /// renderer binding; `USER_WIDGET_CAPS` = the caps marked `widget`;
    /// the app pass's accepted caps = the caps marked `app`.
    #[test]
    fn rust_cap_lists_match_the_catalog() {
        let c = catalog();
        let mut bridge: Vec<&str> = gen::bridge_rows(c).iter().filter_map(|v| v.cap.as_deref()).collect();
        bridge.sort_unstable();
        bridge.dedup();
        let mut want: Vec<&str> = crate::zen::BRIDGE_CAPS.to_vec();
        want.sort_unstable();
        assert_eq!(bridge, want, "BRIDGE_CAPS vs the catalog's bridge rows");
        assert_eq!(c.widget_caps(), crate::zen::USER_WIDGET_CAPS);
        for cap in ["gardens:manage", "gardens:template", "agents:add", "app:navigate"] {
            assert!(!crate::zen::USER_WIDGET_CAPS.contains(&cap), "{cap} is K2's own (UW69)");
            assert!(c.cap(cap).is_some_and(|r| r.exposure.is_empty()), "{cap} is exposed to no one");
        }
        // `skin::ACCEPTED_CAPS` is private; its refusal message lists it.
        let err = crate::skin::parse_caps(Some(&["nope:nope".to_string()])).expect_err("unknown cap refused");
        let listed = err.split_once("accepted: ").map(|(_, l)| l).unwrap_or_else(|| panic!("no list in {err:?}"));
        let accepted: Vec<&str> = listed.split(", ").collect();
        // Caps an owner grants an App itself (RS-A15) are never a pass's.
        // So is `wallet:self` (0.46.3): an owner setting per App.
        let pass_caps: Vec<&str> = c
            .caps_exposed_to(Exposure::App)
            .into_iter()
            // `push:send` (0.46.4) is the App's own cap (its link pass),
            // never a guest role's.
            .filter(|cap| {
                *cap != crate::skin::guest_rooms::CAP
                    && *cap != crate::skin::app_chat::CAP
                    && *cap != crate::person_wallet::CAP
                    && !crate::skin::APP_ONLY_CAPS.contains(cap)
            })
            .collect();
        assert_eq!(pass_caps, accepted, "ACCEPTED_CAPS vs the catalog's app caps");
        assert!(!accepted.contains(&crate::skin::guest_rooms::CAP), "guests:rooms is an owner grant, never a pass cap");
        assert!(!accepted.contains(&crate::skin::app_chat::CAP), "guests:chat is an owner grant, never a pass cap");
        assert!(!accepted.contains(&crate::person_wallet::CAP), "wallet:self is an owner setting per App, never a pass cap");
    }

    /// TUWA1: every row's error codes and caps come from the shared tables,
    /// and every renderer verb the bridge knows today has a row.
    #[test]
    fn every_widget_row_is_closed_and_known() {
        let c = catalog();
        let probs = lint::lint(c);
        assert!(probs.is_empty(), "{}", probs.join("\n"));
        for v in c.widget_callable_verbs() {
            assert!(v.cap.as_deref().is_none_or(|cap| crate::zen::USER_WIDGET_CAPS.contains(&cap)), "{}", v.verb);
        }
        // Garden data (contract-garden-data-v1 §7.2): every garden-local-only
        // row is local, reaches the widget door, and has no renderer binding.
        // One reveal = both surfaces: RS-A15's guests.rooms / guests.setRooms
        // and room-scope 0.46.1's room.members, app.self and app.usage are on
        // both transports (portable, their App route); widget.self is the Garden's.
        // 0.46.3 (garden-person-rows-v1, research I + B): the Garden bridge's
        // person rows join both transports with the App rows they share:
        // thread.catchup / unread / mute, tickets.list/show/create/comment/
        // resolve, activity.snapshot and files.list/read/readBinary (12).
        // No project chat row (Rosson 2026-10-20).
        let local = c.verbs_exposed_to(Exposure::GardenLocal);
        assert_eq!(
            local.len(),
            53,
            "store.* ×14 (store.patch since catalog v21, 0.46.4; store.history, store.restore and store.snapshot since catalog v32, 0.46.6), save.* ×4, viewer.get, guests.rooms, guests.setRooms, app.rooms, guests.list (catalog v22, 0.46.4), room.members, app.self, app.usage, widget.self, the 12 person rows, box usage's app.resources (0.46.4), blobs.info / blobs.list (0.46.5), tickets.attachment (catalog v31, 0.46.6), and the Workbench's 10 (catalog v40, 0.46.7: work.list, git.snapshot/diffFile/log/land/note, compute.jobs, agents.start/stop/settle)"
        );
        let both: Vec<&str> = local.iter().filter(|v| v.exposed_to(Exposure::App)).map(|v| v.verb.as_str()).collect();
        assert_eq!(
            both,
            [
                "guests.rooms",
                "guests.setRooms",
                // prd-app-caps-0464-v1 AC1/AC2 (catalog v22): the App's own
                // ceiling and guest list, under RS-A15's feature.
                "app.rooms",
                "guests.list",
                "thread.catchup",
                "thread.unread",
                "thread.mute",
                "files.list",
                "files.read",
                "files.readBinary",
                "tickets.list",
                "tickets.show",
                // Catalog v31 (0.46.6): a ticket message's file.
                "tickets.attachment",
                "tickets.create",
                "tickets.comment",
                "tickets.resolve",
                "activity.snapshot",
                "room.members",
                "app.self",
                "app.usage"
            ]
        );
        assert!(!both.iter().any(|v| v.starts_with("projects.")), "no project chat on the Garden bridge");
        // The rows each joined both transports under their own feature;
        // the Garden transport's key is garden-person-rows-v1 (boot-status).
        let person_rows: [(&str, &str); 13] = [
            ("thread.catchup", "inbox-catchup-v1"),
            ("thread.unread", "app-thread-parity-v1"),
            ("thread.mute", "app-read-state-v1"),
            ("files.list", "app-api-v1"),
            ("files.read", "app-api-v1"),
            ("files.readBinary", "app-api-v1"),
            ("tickets.list", "app-api-v1"),
            ("tickets.show", "app-api-v1"),
            ("tickets.create", "app-api-v1"),
            ("tickets.comment", "app-api-v1"),
            ("tickets.resolve", "app-api-v1"),
            // 0.46.6: a ticket message's file, under its own feature key.
            ("tickets.attachment", "ticket-attachments-v1"),
            ("activity.snapshot", "app-api-v1"),
        ];
        for (verb, feature) in person_rows {
            let v = c.verb(verb).unwrap_or_else(|| panic!("{verb} row"));
            assert_eq!(v.feature, feature, "{verb}");
            // Tickets, activity and files check the widget's maker (an
            // agent-made widget: its own room); the person's own Thread state
            // doesn't.
            let maker_rule = !verb.starts_with("thread.");
            assert_eq!(v.errors.iter().any(|e| e == "author_unknown"), maker_rule, "{verb}: author_unknown");
            assert!(maker_rule || v.errors.iter().any(|e| e == "not_found"), "{verb}: an unknown address");
            assert!(!maker_rule || v.errors.iter().any(|e| e == "not_bound"), "{verb}: a room out of reach");
        }
        for v in local {
            if v.exposed_to(Exposure::App) {
                assert_eq!(v.reach, Reach::Portable, "{}", v.verb);
                let person_row = person_rows.iter().any(|(n, _)| *n == v.verb);
                assert!(person_row || ["guests-rooms-v1", "room-members-v1", "app-self-v1"].contains(&v.feature.as_str()), "{}", v.verb);
                continue;
            }
            assert_eq!(v.reach, Reach::Local, "{}", v.verb);
            let room_scope = v.verb == "widget.self" && v.feature == "app-self-v1";
            // 0.46.4: box usage's Garden card, its own feature key.
            let box_card = v.verb == "app.resources" && v.feature == "app-resources-v1";
            // 0.46.5 (prd-garden-board-needs-v1): blobs and history keep
            // their own feature keys.
            let board_needs = [crate::garden_data::blobs::FEATURE, "garden-history-v1"].contains(&v.feature.as_str());
            assert!(room_scope || box_card || board_needs || v.feature == crate::garden_data::FEATURE, "{}", v.verb);
        }
        assert!(c.verb("theme.changed").is_some_and(|v| v.kind == VerbKind::Event));
        // Rosson 2026-10-20 (Thread parity, 0.46.3) replaces UW51: a widget
        // marks the person's own read cursor, as reading in K2 does.
        assert!(
            c.verb("thread.markRead").is_some_and(|v| v.exposure == [Exposure::Widget] && v.feature == "thread-read-v1"),
            "thread.markRead is a widget row since 0.46.3"
        );
    }

    #[test]
    fn shared_error_list_is_the_uwa10_list_plus_sending_off() {
        let codes: Vec<&str> = catalog().errors.iter().map(|e| e.code.as_str()).collect();
        // UWA10's list, minus `not_open` (UWB10 drops one-open-conversation),
        // plus `sending_off` (UWB9), plus the Garden v3 sources codes
        // (contract-sources-v1 §6).
        assert_eq!(
            codes,
            [
                "cap_not_granted",
                "not_bound",
                "rate_limited",
                "too_large",
                "unknown_verb",
                "verb_unavailable",
                "verb_local",
                "not_exposed",
                "sending_off",
                "server_not_saved",
                "unreachable",
                "removed",
                "no_history",
                "failed",
                // Garden data (contract-garden-data-v1 §7.2).
                "conflict",
                "space_full",
                "value_too_large",
                "bad_key",
                "not_granted",
                "resync",
                "author_unknown",
                "stale_widget",
                "store_full",
                "bad_tx",
                "tx_expired",
                "busy",
                "paused",
                // 0.46.5 (prd-garden-board-needs-v1 GB3): a value names a blob its
                // space doesn't hold (with the Garden data codes).
                "blob_missing",
                // The widget SDK's guards (0.46.1, catalog 11).
                "bad_args",
                "bad_shape",
                // The App transport (runtime S23): the gateway link's and
                // the App routes' refusals, plus the server SDK's own two.
                "pass_required",
                "pass_invalid",
                "pass_wrong_link",
                "not_on_link",
                "json_required",
                "skin_room",
                "usage",
                "not_found",
                "bad_login",
                "locked",
                "card_not_pending",
                "guest_required",
                "no_app_pass",
                "no_app_key",
                // Sew's chat pass (S28 §9) through the App transport.
                "no_chat_grant",
                // Sew for Apps (0.46.6): A1, decision 13, A2, A6.
                "no_chat_caps",
                "app_claim_invalid",
                "not_app_granted",
                "chat_off_for_person",
                "shared_person",
                "has_assistants",
                "chat_limit",
                "username_unusable",
                "chat_not_app_scoped",
                "no_chat_member",
                // Sew for Apps A9 (0.46.6): chat.createChannel.
                "sew_feature_missing",
                "space_unbound",
                "app_mismatch",
                "chat_unavailable",
                // RS-A15 guest rooms (both transports).
                "app_pass_required",
                "grant_required",
                "outside_app_rooms",
                "above_role",
                "has_role",
                "confirm_required",
                "declined",
                // app-mobile-access-v1 (0.46.3): device sign-in.
                "refresh_invalid",
                "refresh_reused",
                // App users' personal wallets (0.46.3).
                "person_pass_required",
                "wallet_off",
                // prd-custom-app-push-v1 (0.46.4): App push. Before
                // duplicate_label so `retired` stays the widget guide's last
                // code (the Garden access edit anchors on it).
                "push_off",
                "not_for_host",
                "channel_unavailable",
                "push_devices_full",
                "push_not_configured",
                "too_many_recipients",
                "push_queue_full",
                "airgap",
                "vault_closed",
                "token_in_use",
                // prd-app-phone-pairing-v1 (0.46.5): App phone pairing.
                "pairing_off",
                "pairing_from_device",
                "reauth_required",
                "device_signin_off",
                "host_not_paired",
                "no_public_host",
                "pairing_not_served",
                "pair_code_invalid",
                "pairing_not_from_browser",
                "duplicate_label",
                // Garden access lists (prd-garden-access-v1 G22): the
                // signer gate's refusal (switch on only).
                "needs_editor",
                // 0.46.4: a verb K2 retired answers this until its removal
                // (k2.sources.ask until 0.47.0).
                "retired",
                // 0.46.4 (prd-app-caps-0464-v1 AC24/AC28): only a person
                // answers a card.
                "card_person_only",
                // 0.46.5 (prd-app-user-profile-v1, U19): a person's own
                // profile edits and pictures.
                "edit_off",
                "type_refused",
                "type_mismatch",
                "bad_image",
                "agent_login",
                // Email Notifications (0.46.5, catalog v26).
                "email_off",
                "no_sender",
                "sender_not_granted",
                "system_sender",
                "recipient_out_of_scope",
                "header_not_allowed",
                "message_too_large",
                "domain_not_ready",
                "dmarc_missing",
                "relay_not_ready",
                "idempotency_conflict",
                "no_mail_path",
                // Catalog v35 (email notifications part 2, N6).
                "email_paused",
                "all_suppressed",
                // Catalog v38 (must-change at next sign-in, 0.46.6).
                "password_change_required",
                "password_unchanged",
            ]
        );
        let retry: Vec<&str> = catalog().errors.iter().filter(|e| e.retryable).map(|e| e.code.as_str()).collect();
        assert_eq!(retry, ["not_bound", "rate_limited", "unreachable", "busy"], "what a handler-object subscription retries");
    }

    /// 0.46.3: the guest ear is opt-in per connection. Only the room
    /// stream's row carries `ear=1` (the SDK adds it). `thread.events`
    /// never asks: its frames stay Thread items (live legacy Apps read
    /// `item.doc` on every frame); a raw client may still opt in.
    #[test]
    fn only_the_room_stream_asks_for_the_ear() {
        let c = catalog();
        for v in &c.verbs {
            let Some(s) = &v.bindings.socket else { continue };
            let want: BTreeMap<String, String> = if v.verb == "thread.stream" {
                [("ear".to_string(), "1".to_string())].into_iter().collect()
            } else {
                BTreeMap::new()
            };
            assert_eq!(s.query, want, "{}", v.verb);
        }
    }

    #[test]
    fn widget_caps_are_the_seven_and_each_has_a_sentence() {
        let c = catalog();
        assert_eq!(
            c.caps_exposed_to(Exposure::Widget),
            [
                "agents:read",
                "presence:read",
                "thread:read",
                "thread:post",
                "sources:read",
                "links:open",
                "downloads:save",
                // 0.46.6: k2.pick / k2.onDrop (uploads.pick, uploads.dropped).
                "uploads:pick",
                // 0.46.5: the byte rows (blobs.put / blobs.get) are frame rows.
                "store:read",
                "store:write",
                // 0.46.5 activity SDK: activity:read reaches the bridge too
                // (`thread.onActivity`, with thread:read, as the App ear).
                "activity:read"
            ]
        );
        // 0.46.3 (garden-person-rows-v1): files:read, tickets:read,
        // tickets:post and activity:read join through the Garden door.
        assert_eq!(
            c.widget_caps(),
            [
                "agents:read",
                "presence:read",
                "thread:read",
                "thread:post",
                "sources:read",
                // 0.46.4: k2.open(url), answered by the custom layer.
                "links:open",
                "downloads:save",
                "uploads:pick",
                "files:read",
                "tickets:read",
                "tickets:post",
                "store:read",
                "store:write",
                "guests:rooms",
                "activity:read",
                "room:members"
            ]
        );
        for name in ["files:read", "tickets:read", "tickets:post"] {
            let row = c.cap(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(row.exposure, [Exposure::App, Exposure::GardenLocal], "{name}: App and Garden, never the widget bridge");
            assert!(row.sentence.as_deref().is_some_and(|s| s.contains("on this computer")), "{name}: its sentence says where");
        }
        // 0.46.5 activity SDK: activity:read also reaches the bridge
        // (`thread.onActivity`, any agent in the widget's scope), so its
        // sentence names the scope, and says counts only.
        let activity = c.cap("activity:read").expect("activity:read");
        assert_eq!(activity.exposure, [Exposure::Widget, Exposure::App, Exposure::GardenLocal]);
        let s = activity.sentence.as_deref().expect("a widget cap has a sentence");
        assert!(s.contains("{where}") && s.contains("counts only"), "{s}");
        let local_scope = [
            "presence:read",
            "sources:read",
            "links:open",
            "downloads:save",
            "uploads:pick",
            "files:read",
            "tickets:read",
            "tickets:post",
            "store:read",
            "store:write",
            "guests:rooms",
            "activity:read",
            "room:members",
        ];
        for name in c.widget_caps() {
            let s = c.cap(name).and_then(|r| r.sentence.as_deref());
            let s = s.unwrap_or_else(|| panic!("widget cap {name} has no sentence"));
            assert!(s.contains("{where}") || local_scope.contains(&name), "{name}: sentence names its scope: {s}");
        }
    }

    #[test]
    fn unknown_keys_are_refused() {
        let bad = r#"{"catalogVersion":1,"caps":[],"errors":[],"bannedFields":[],"verbs":[],"extra":1}"#;
        assert!(parse(bad).is_err());
        let bad_row = r#"{"catalogVersion":1,"caps":[],"errors":[],"bannedFields":[],"verbs":[
            {"verb":"a.b","cap":null,"reach":"local","effect":"read","kind":"call","feature":"zen-v1",
             "bindings":{"renderer":{"impl":"builtin"}},"request":[],"response":{},"errors":[],
             "doc":"d","example":"e","auth":"owner"}]}"#;
        assert!(parse(bad_row).is_err(), "rows carry no auth (UWB29)");
    }

    #[test]
    fn a_row_round_trips() {
        let src = r#"{"catalogVersion":1,"caps":[],"errors":[],"bannedFields":[],"verbs":[
            {"verb":"thread.post","cap":"thread:post","exposure":["widget"],"reach":"portable",
             "effect":"write","kind":"call","feature":"zen-v1",
             "bindings":{"renderer":{"impl":"registered"},
                         "http":{"method":"POST","route":"/cli/thread/post","params":{"text":"text"}}},
             "request":[{"type":"string"},{"type":"string"}],"response":{"type":"null"},
             "errors":["cap_not_granted","not_bound","sending_off"],
             "doc":"Post text as you.","example":"await k2.thread.post(a, 'hi')"}]}"#;
        let c = parse(src).expect("row parses");
        let v = c.verb("thread.post").expect("row present");
        assert!(v.exposed_to(Exposure::Widget));
        assert!(!v.exposed_to(Exposure::App));
        assert_eq!(v.bindings.http.as_ref().map(|h| h.method), Some(HttpMethod::Post));
        let back = parse(&serde_json::to_string(&c).expect("serialises")).expect("re-parses");
        assert_eq!(back, c);
    }
}
