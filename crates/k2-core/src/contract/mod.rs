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

pub mod gen;
pub mod lint;

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
    /// An App (Cut B; no `app` verb rows in this cut).
    App,
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
    /// Empty or absent = none (K2 built-ins only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposure: Vec<Exposure>,
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
}

impl VerbRow {
    /// True when `who` may call this verb.
    pub fn exposed_to(&self, who: Exposure) -> bool {
        self.exposure.contains(&who)
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
    /// Handled only by the custom-widget frame host (`theme.changed`).
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
    /// `USER_WIDGET_CAPS` (asserted equal in `zen::grants` tests).
    pub fn caps_exposed_to(&self, who: Exposure) -> Vec<&str> {
        self.caps.iter().filter(|c| c.exposure.contains(&who)).map(|c| c.name.as_str()).collect()
    }

    /// Verb rows exposed to `who`, in file order (`ZEN_CUSTOM_VERBS` for
    /// `Widget`).
    pub fn verbs_exposed_to(&self, who: Exposure) -> Vec<&VerbRow> {
        self.verbs.iter().filter(|v| v.exposed_to(who)).collect()
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
        assert_eq!(c.caps_exposed_to(Exposure::Widget), crate::zen::USER_WIDGET_CAPS);
        for cap in ["gardens:manage", "gardens:template", "agents:add", "app:navigate"] {
            assert!(!crate::zen::USER_WIDGET_CAPS.contains(&cap), "{cap} is K2's own (UW69)");
            assert!(c.cap(cap).is_some_and(|r| r.exposure.is_empty()), "{cap} is exposed to no one");
        }
        // `skin::ACCEPTED_CAPS` is private; its refusal message lists it.
        let err = crate::skin::parse_caps(Some(&["nope:nope".to_string()])).expect_err("unknown cap refused");
        let listed = err.split_once("accepted: ").map(|(_, l)| l).unwrap_or_else(|| panic!("no list in {err:?}"));
        let accepted: Vec<&str> = listed.split(", ").collect();
        assert_eq!(c.caps_exposed_to(Exposure::App), accepted, "ACCEPTED_CAPS vs the catalog's app caps");
    }

    /// TUWA1: every row's error codes and caps come from the shared tables,
    /// and every renderer verb the bridge knows today has a row.
    #[test]
    fn every_widget_row_is_closed_and_known() {
        let c = catalog();
        let probs = lint::lint(c);
        assert!(probs.is_empty(), "{}", probs.join("\n"));
        for v in c.verbs_exposed_to(Exposure::Widget) {
            assert!(v.cap.as_deref().is_none_or(|cap| crate::zen::USER_WIDGET_CAPS.contains(&cap)), "{}", v.verb);
        }
        assert!(c.verb("theme.changed").is_some_and(|v| v.kind == VerbKind::Event));
        assert!(c.verb("thread.markRead").is_some_and(|v| v.exposure.is_empty()), "UW51: markRead stays none");
    }

    #[test]
    fn shared_error_list_is_the_uwa10_list_plus_sending_off() {
        let codes: Vec<&str> = catalog().errors.iter().map(|e| e.code.as_str()).collect();
        // UWA10's list, minus `not_open` (UWB10 drops one-open-conversation),
        // plus `sending_off` (UWB9).
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
                "failed",
            ]
        );
    }

    #[test]
    fn widget_caps_are_the_four_and_each_has_a_sentence() {
        let c = catalog();
        assert_eq!(c.caps_exposed_to(Exposure::Widget), ["agents:read", "presence:read", "thread:read", "thread:post"]);
        for name in c.caps_exposed_to(Exposure::Widget) {
            let s = c.cap(name).and_then(|r| r.sentence.as_deref());
            let s = s.unwrap_or_else(|| panic!("widget cap {name} has no sentence"));
            assert!(s.contains("{where}") || name == "presence:read", "{name}: sentence names its scope: {s}");
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
