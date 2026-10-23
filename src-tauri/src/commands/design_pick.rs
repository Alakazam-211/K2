//! Design mode picker plumbing (prd-browser-design-mode-v1 D-A11), the pure
//! half: the guest scripts the host evaluates, the per-view design gate and
//! page-load counter, and the parse + clamp of what the page hands back.
//! No Tauri types here so it unit-tests without a window.
//!
//! The page is untrusted. Its drain result is capped before parsing, every
//! pick element goes through `k2_client::browser_notes::clamp_element`, the
//! URL through `clean_page_url` (non-http(s) picks are dropped), and picks
//! from an older page load or another URL than the view shows now are
//! dropped (navigation cancels a pick).

#![cfg_attr(not(feature = "browser-pane"), allow(dead_code))]

use std::collections::{HashMap, HashSet};

use k2_client::browser_notes::{clamp_element, clamp_line, clean_page_url, NoteElement, MAX_TITLE_CHARS};
use serde::Serialize;

/// The guest picker: a function expression the host calls with the page
/// load number (`install_script`).
pub const DESIGN_PICKER_JS: &str = include_str!("design_picker.js");

/// Largest drain result the host will parse (bytes of the callback JSON).
pub const MAX_POLL_RAW: usize = 256 * 1024;
/// Most events one poll returns (the guest queue holds 20 too).
pub const MAX_POLL_EVENTS: usize = 20;
/// The drain sentinel when the picker is not installed in the page (a
/// load the host has not re-injected yet).
pub const MISSING: &str = "__k2_design_missing__";

pub const NOT_DESIGN: &str = "not a design view";

/// Install (idempotent) for page load `nav`.
pub fn install_script(nav: u64) -> String {
    format!("{}({nav})", DESIGN_PICKER_JS.trim_end())
}

/// Install if needed, then arm or disarm.
pub fn arm_script(nav: u64, armed: bool) -> String {
    format!("{};\nwindow.__k2DesignPicker.arm({armed});", install_script(nav))
}

/// Drain the guest queue as one JSON string (or [`MISSING`]).
pub fn drain_script() -> String {
    format!(
        "(function () {{ var p = window.__k2DesignPicker; return p && typeof p.drain === 'function' ? p.drain() : '{MISSING}'; }})()"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct Badge {
    pub n: u32,
    pub selector: String,
}

/// Draw numbered badges at these selectors (replaces the previous set).
pub fn badges_script(nav: u64, badges: &[Badge]) -> String {
    let clamped: Vec<Badge> = badges
        .iter()
        .take(50)
        .map(|b| Badge { n: b.n, selector: clamp_line(&b.selector, k2_client::browser_notes::MAX_SELECTOR_CHARS) })
        .collect();
    format!(
        "{};\nwindow.__k2DesignPicker.setBadges({});",
        install_script(nav),
        js_literal(&serde_json::to_string(&clamped).unwrap_or_else(|_| "[]".into()))
    )
}

/// JSON is a JS literal except U+2028 / U+2029 in older engines.
fn js_literal(json: &str) -> String {
    json.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// Which registry keys are Design views, each one's page-load number, and
/// whether Select is on. Plain Browser views are never in here, so the
/// picker refuses them. Select is on by default in a Design tab, so a new
/// page load (navigation, reload) re-arms the fresh picker from here.
#[derive(Debug, Default)]
pub struct DesignRegistry {
    nav: HashMap<String, u64>,
    armed: HashSet<String>,
}

impl DesignRegistry {
    /// `browser_create(design)`: mark or clear the key. A key that stays a
    /// design view keeps its counter.
    pub fn set_design(&mut self, key: &str, design: bool) {
        if design {
            self.nav.entry(key.to_string()).or_insert(1);
        } else {
            self.nav.remove(key);
            self.armed.remove(key);
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.nav.remove(key);
        self.armed.remove(key);
    }

    /// Remember Select for this view (only for a design view).
    pub fn set_armed(&mut self, key: &str, armed: bool) {
        if armed && self.nav.contains_key(key) {
            self.armed.insert(key.to_string());
        } else {
            self.armed.remove(key);
        }
    }

    /// Select is on for this view (a page load installs the picker armed).
    pub fn is_armed(&self, key: &str) -> bool {
        self.armed.contains(key)
    }

    /// The current page-load number, or [`NOT_DESIGN`].
    pub fn nav(&self, key: &str) -> Result<u64, String> {
        self.nav.get(key).copied().ok_or_else(|| NOT_DESIGN.to_string())
    }

    /// A new page started loading: picks from the old one are void.
    pub fn bump(&mut self, key: &str) -> Option<u64> {
        let n = self.nav.get_mut(key)?;
        *n += 1;
        Some(*n)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickViewport {
    pub width: f64,
    pub height: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub dpr: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PickEvent {
    #[serde(rename_all = "camelCase")]
    Pick { pick_id: String, url: String, title: String, viewport: PickViewport, element: NoteElement },
    Key { key: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickPoll {
    pub events: Vec<PickEvent>,
    pub nav_seq: u64,
}

/// What a drain told the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Drained {
    /// The picker is not in the page: re-inject it.
    Missing,
    /// Nothing usable (empty, stale, malformed, oversize).
    Nothing,
    Events(PickPoll),
}

fn num(v: &serde_json::Value, key: &str) -> f64 {
    v.get(key).and_then(serde_json::Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0)
}

/// Parse an `eval_with_callback` result for [`drain_script`]. `raw` is the
/// engine's JSON for the returned value (a JSON string holding JSON).
/// `current_url` is what the view shows now (None: unknown, don't filter).
pub fn parse_drain(raw: &str, nav: u64, current_url: Option<&str>) -> Drained {
    if raw.len() > MAX_POLL_RAW {
        return Drained::Nothing;
    }
    let inner = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::String(s)) => s,
        _ => return Drained::Nothing,
    };
    if inner == MISSING {
        return Drained::Missing;
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&inner) else {
        return Drained::Nothing;
    };
    // A queue from another page load (the guest's own number) is stale.
    if body.get("nav").and_then(serde_json::Value::as_u64) != Some(nav) {
        return Drained::Nothing;
    }
    let current = current_url.and_then(|u| clean_page_url(u).ok());
    let mut events = Vec::new();
    for ev in body.get("events").and_then(serde_json::Value::as_array).into_iter().flatten() {
        if events.len() >= MAX_POLL_EVENTS {
            break;
        }
        match ev.get("type").and_then(serde_json::Value::as_str) {
            Some("key") => match ev.get("key").and_then(serde_json::Value::as_str) {
                Some(k @ "escape") => events.push(PickEvent::Key { key: k.to_string() }),
                _ => {}
            },
            Some("pick") => {
                if let Some(p) = parse_pick(ev, current.as_deref()) {
                    events.push(p);
                }
            }
            _ => {}
        }
    }
    if events.is_empty() {
        Drained::Nothing
    } else {
        Drained::Events(PickPoll { events, nav_seq: nav })
    }
}

fn parse_pick(ev: &serde_json::Value, current_url: Option<&str>) -> Option<PickEvent> {
    let url = clean_page_url(ev.get("url")?.as_str()?).ok()?;
    if let Some(cur) = current_url {
        if cur != url {
            return None;
        }
    }
    let element: NoteElement = serde_json::from_value(ev.get("element")?.clone()).ok()?;
    let element = clamp_element(&element);
    let pick_id = clamp_line(ev.get("pickId").and_then(serde_json::Value::as_str).unwrap_or(""), 64);
    if pick_id.is_empty() {
        return None;
    }
    let vp = ev.get("viewport").cloned().unwrap_or(serde_json::Value::Null);
    Some(PickEvent::Pick {
        pick_id,
        url,
        title: clamp_line(ev.get("title").and_then(serde_json::Value::as_str).unwrap_or(""), MAX_TITLE_CHARS),
        viewport: PickViewport {
            width: num(&vp, "width"),
            height: num(&vp, "height"),
            scroll_x: num(&vp, "scrollX"),
            scroll_y: num(&vp, "scrollY"),
            dpr: num(&vp, "dpr"),
        },
        element,
    })
}

#[cfg(test)]
mod design_pick_tests {
    use super::*;
    use serde_json::json;

    fn raw(body: serde_json::Value) -> String {
        serde_json::to_string(&body.to_string()).unwrap()
    }

    fn pick(url: &str) -> serde_json::Value {
        json!({
            "type": "pick",
            "pickId": "p1",
            "url": url,
            "title": "Pricing\n page",
            "viewport": {"width": 1280, "height": 800, "scrollX": 0, "scrollY": 40, "dpr": 2},
            "element": {
                "selector": "main > button.cta",
                "tag": "BUTTON",
                "text": "Buy",
                "html": "<button onclick=\"x()\" data-token=\"abc\">Buy</button><script>alert(1)</script>",
                "styles": {"color": "red", "background-image": "url(x)"},
                "rect": {"x": 1, "y": 2, "width": 3, "height": 4}
            }
        })
    }

    #[test]
    fn design_pick_gate_and_nav_counter() {
        let mut r = DesignRegistry::default();
        assert_eq!(r.nav("w\0a").unwrap_err(), NOT_DESIGN);
        r.set_design("w\0a", true);
        assert_eq!(r.nav("w\0a").unwrap(), 1);
        assert_eq!(r.bump("w\0a"), Some(2));
        // Re-create as design keeps the counter; as plain Browser clears it.
        r.set_design("w\0a", true);
        assert_eq!(r.nav("w\0a").unwrap(), 2);
        r.set_design("w\0a", false);
        assert!(r.nav("w\0a").is_err(), "a plain Browser view is not a design view");
        assert_eq!(r.bump("w\0a"), None, "a plain view never gets a counter");
        r.set_design("w\0b", true);
        r.remove("w\0b");
        assert!(r.nav("w\0b").is_err());
    }

    #[test]
    fn design_pick_parses_and_clamps_a_pick() {
        let d = parse_drain(&raw(json!({"nav": 3, "events": [pick("http://localhost:5173/p?token=1#x")]})), 3, Some("http://localhost:5173/p?other=2"));
        let Drained::Events(poll) = d else { panic!("expected events, got {d:?}") };
        assert_eq!(poll.nav_seq, 3);
        assert_eq!(poll.events.len(), 1);
        let PickEvent::Pick { url, title, element, viewport, pick_id } = &poll.events[0] else { panic!() };
        assert_eq!(url, "http://localhost:5173/p");
        assert_eq!(title, "Pricing page");
        assert_eq!(pick_id, "p1");
        assert_eq!(viewport.scroll_y, 40.0);
        assert_eq!(element.tag, "button");
        assert!(!element.html.contains("onclick") && !element.html.contains("abc") && !element.html.contains("alert"), "{}", element.html);
        assert!(!element.styles.contains_key("background-image"));
        let v = serde_json::to_value(&poll).unwrap();
        assert_eq!(v["navSeq"], 3);
        assert_eq!(v["events"][0]["type"], "pick");
        assert_eq!(v["events"][0]["pickId"], "p1");
        assert_eq!(v["events"][0]["viewport"]["scrollY"], 40.0);
        assert_eq!(v["events"][0]["element"]["selector"], "main > button.cta");
    }

    #[test]
    fn design_pick_drops_stale_navigation_and_other_urls() {
        let body = json!({"nav": 2, "events": [pick("http://localhost/a"), {"type": "key", "key": "escape"}]});
        assert_eq!(parse_drain(&raw(body.clone()), 3, None), Drained::Nothing, "an older page load is void");
        let Drained::Events(p) = parse_drain(&raw(body), 2, Some("http://localhost/b")) else { panic!() };
        assert_eq!(p.events, vec![PickEvent::Key { key: "escape".into() }], "a pick from another URL is dropped");
        // The S key is gone (Rosson 2026-10-16): an old page's toggle is ignored.
        let s = json!({"nav": 1, "events": [{"type": "key", "key": "toggle-select"}]});
        assert_eq!(parse_drain(&raw(s), 1, None), Drained::Nothing);
    }

    #[test]
    fn design_pick_refuses_junk() {
        assert_eq!(parse_drain(&serde_json::to_string(MISSING).unwrap(), 1, None), Drained::Missing);
        assert_eq!(parse_drain("null", 1, None), Drained::Nothing);
        assert_eq!(parse_drain("\"not json\"", 1, None), Drained::Nothing);
        assert_eq!(parse_drain(&raw(json!({"nav": 1, "events": []})), 1, None), Drained::Nothing);
        let huge = raw(json!({"nav": 1, "events": [], "pad": "x".repeat(MAX_POLL_RAW)}));
        assert_eq!(parse_drain(&huge, 1, None), Drained::Nothing);
        // Non-http page, unknown key, missing pick id, bad element shape.
        let mut no_id = pick("http://h/");
        no_id["pickId"] = json!("");
        let mut bad_el = pick("http://h/");
        bad_el["element"] = json!("<b>");
        let body = json!({"nav": 1, "events": [
            pick("file:///etc/passwd"), {"type": "key", "key": "rm -rf"}, no_id, bad_el, {"type": "other"}
        ]});
        assert_eq!(parse_drain(&raw(body), 1, None), Drained::Nothing);
        // At most MAX_POLL_EVENTS.
        let many: Vec<_> = (0..40).map(|_| json!({"type": "key", "key": "escape"})).collect();
        let Drained::Events(p) = parse_drain(&raw(json!({"nav": 1, "events": many})), 1, None) else { panic!() };
        assert_eq!(p.events.len(), MAX_POLL_EVENTS);
    }

    #[test]
    fn design_pick_remembers_select_per_design_view() {
        let mut r = DesignRegistry::default();
        r.set_armed("w\0a", true);
        assert!(!r.is_armed("w\0a"), "not a design view yet: nothing remembered");
        r.set_design("w\0a", true);
        r.set_armed("w\0a", true);
        assert!(r.is_armed("w\0a"));
        r.bump("w\0a");
        assert!(r.is_armed("w\0a"), "a navigation keeps Select on");
        r.set_armed("w\0a", false);
        assert!(!r.is_armed("w\0a"));
        r.set_armed("w\0a", true);
        r.remove("w\0a");
        assert!(!r.is_armed("w\0a"));
    }

    #[test]
    fn design_pick_scripts_embed_the_picker() {
        assert!(install_script(7).ends_with("})(7)"), "install calls the function expression");
        assert!(arm_script(7, true).contains("window.__k2DesignPicker.arm(true);"));
        assert!(drain_script().contains(MISSING));
        let s = badges_script(1, &[Badge { n: 1, selector: "a\u{2028}b".into() }]);
        assert!(s.contains("setBadges([{\"n\":1,\"selector\":\"a"), "{s}");
        assert!(!s.contains('\u{2028}'));
        assert!(!DESIGN_PICKER_JS.contains("innerHTML"), "Trusted Types pages: never innerHTML");
        assert!(DESIGN_PICKER_JS.contains("mode: 'closed'"));
    }
}
