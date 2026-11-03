//! `[phone]` in a Garden file: its mobile layout (contract-gardens-phone
//! rev 2 §A). The walk ([`check_phone`]) sets `page.phone` in the layer's
//! wire shape, `{screens: [{label?, canvas?, widgets: [{id, height?,
//! config?}]}]}`; what needs the page's widgets is [`cross_check_phone`].
//!
//! One Garden, two versions (Rosson 2026-11-03): this table never decides
//! whether a Garden shows on a phone. It is the layout inside the Garden.
//!
//! Against the file's own `[[widget]]` blocks every finding is an error.
//! Against a template's widgets, what the template decides (is that id on
//! it, what kind is it) is a warning for live text and a refusal for new
//! text ([`Checked::authoring`](super::Checked)): a K2 update must never
//! turn a good Garden file into an error, and the page leaves out what no
//! longer fits ([`crate::zen::phone::settle`]).

use serde_json::{json, Map, Value as J};
use toml_edit::{Item, TableLike, Value};

use super::{as_number, config_values, item_pos, key_pos, num_json, require_table, table_pos, tables_of, unknown_key, value_pos, Ctx, CANVASES, CUSTOM_KIND, MAX_CONFIG_BYTES};
use crate::zen::phone::{self, HEIGHT_AUTO, HEIGHT_FILL, HEIGHT_MAX, HEIGHT_MIN, LABEL_MAX, MAX_SCREENS};

const NEEDS_A_SCREEN: &str = "[phone] needs at least one [[phone.screen]]";
const NEEDS_AN_ID: &str = "each [[phone.screen.widget]] needs an id";

fn height_message() -> String {
    format!("widget height is \"{HEIGHT_FILL}\", \"{HEIGHT_AUTO}\" or a number of px, {HEIGHT_MIN} to {HEIGHT_MAX}")
}

/// A screen `label`: 1 to [`LABEL_MAX`] characters, not all blank, no
/// control characters.
fn valid_label(label: &str) -> bool {
    let n = label.chars().count();
    (1..=LABEL_MAX).contains(&n) && !label.trim().is_empty() && !label.chars().any(char::is_control)
}

fn screen_pos_key(screen: usize, widget: usize) -> String {
    format!("page.phone.screen.{screen}.widget.{widget}")
}

/// `[phone]`: walk it, report every shape problem, set `page.phone`.
pub(super) fn check_phone(ctx: &mut Ctx, item: &Item, at: (usize, usize)) {
    let Some(t) = require_table(ctx, item, at, "phone") else { return };
    let mut list: Option<&Item> = None;
    for (k, v) in t.iter() {
        if k == "screen" {
            list = Some(v);
        } else {
            let p = key_pos(ctx, t, k, v, at);
            ctx.error(p, unknown_key(k, "[phone]", &["screen"]));
        }
    }
    let Some(list) = list else {
        ctx.error(at, NEEDS_A_SCREEN);
        return;
    };
    let Some(tables) = tables_of(list) else {
        let p = item_pos(ctx, list, at);
        ctx.error(p, "phone.screen is a list of screens: one [[phone.screen]] block per screen");
        return;
    };
    if tables.is_empty() {
        ctx.error(at, NEEDS_A_SCREEN);
        return;
    }
    if tables.len() > MAX_SCREENS {
        ctx.error(at, format!("a phone layout has 1 to {MAX_SCREENS} screens; this one has {}", tables.len()));
        return;
    }
    let mut screens = Vec::with_capacity(tables.len());
    for (i, s) in tables.iter().enumerate() {
        screens.push(check_screen(ctx, *s, i, at));
    }
    ctx.set("page.phone".into(), json!({ "screens": screens }), at);
}

/// One `[[phone.screen]]`. Reports what it finds and returns what's usable.
fn check_screen(ctx: &mut Ctx, s: &dyn TableLike, index: usize, at: (usize, usize)) -> J {
    let spos = table_pos(ctx, s, at);
    let mut out = Map::new();
    let mut full_at: Option<(usize, usize)> = None;
    let mut widgets: Option<&Item> = None;
    for (k, v) in s.iter() {
        let p = key_pos(ctx, s, k, v, spos);
        match k {
            "label" => match v.as_value().and_then(Value::as_str).filter(|l| valid_label(l)) {
                Some(l) => {
                    out.insert("label".into(), json!(l));
                }
                None => {
                    let vp = item_pos(ctx, v, p);
                    ctx.error(vp, format!("screen label is text, 1 to {LABEL_MAX} characters, with no control characters (it names the screen's tab)"));
                }
            },
            // Absent on the wire means framed: only `full` is ever sent.
            "canvas" => match v.as_value().and_then(Value::as_str).and_then(|c| CANVASES.iter().find(|x| **x == c)) {
                Some(&"full") => {
                    out.insert("canvas".into(), json!("full"));
                    full_at = Some(item_pos(ctx, v, p));
                }
                Some(_) => {}
                None => {
                    let vp = item_pos(ctx, v, p);
                    ctx.error(vp, format!("screen canvas must be one of: {}", CANVASES.join(", ")));
                }
            },
            "widget" => widgets = Some(v),
            other => ctx.error(p, unknown_key(other, "[[phone.screen]]", &["label", "canvas", "widget"])),
        }
    }
    let n = index + 1;
    let no_widgets = format!("phone screen {n} has no widgets; add [[phone.screen.widget]] or drop the screen");
    let rows = match widgets.map(|w| (w, tables_of(w))) {
        None => {
            ctx.error(spos, no_widgets);
            Vec::new()
        }
        Some((w, None)) => {
            let p = item_pos(ctx, w, spos);
            ctx.error(p, "phone.screen.widget is a list of widgets: one [[phone.screen.widget]] block per widget");
            Vec::new()
        }
        Some((_, Some(t))) if t.is_empty() => {
            ctx.error(spos, no_widgets);
            Vec::new()
        }
        Some((_, Some(t))) => {
            if let Some(p) = full_at.filter(|_| t.len() > 1) {
                ctx.error(p, format!("a full-canvas screen holds one widget; this one has {}", t.len()));
            }
            let alone = t.len() == 1;
            t.iter().enumerate().filter_map(|(j, w)| check_widget(ctx, *w, index, j, alone, spos)).collect()
        }
    };
    out.insert("widgets".into(), J::Array(rows));
    J::Object(out)
}

/// One `[[phone.screen.widget]]`; `None` with no id (reported).
fn check_widget(ctx: &mut Ctx, w: &dyn TableLike, screen: usize, index: usize, alone: bool, at: (usize, usize)) -> Option<J> {
    let wpos = table_pos(ctx, w, at);
    let mut out = Map::new();
    let mut id: Option<String> = None;
    let mut config: Option<&Item> = None;
    for (k, v) in w.iter() {
        let p = key_pos(ctx, w, k, v, wpos);
        match k {
            "id" => match v.as_value().and_then(Value::as_str).map(str::trim).filter(|i| !i.is_empty()) {
                Some(i) => {
                    let vp = item_pos(ctx, v, p);
                    ctx.pos.insert(format!("{}.id", screen_pos_key(screen, index)), vp);
                    id = Some(i.to_string());
                }
                None => ctx.error(p, NEEDS_AN_ID),
            },
            "height" => check_height(ctx, v, p, alone, &mut out),
            "config" => config = Some(v),
            other => ctx.error(p, unknown_key(other, "[[phone.screen.widget]]", &["id", "height", "config"])),
        }
    }
    let Some(id) = id else {
        if !w.contains_key("id") {
            ctx.error(wpos, NEEDS_AN_ID);
        }
        return None;
    };
    if let Some(c) = config {
        let cpos = key_pos(ctx, w, "config", c, wpos);
        if let Some(values) = config_values(ctx, c, cpos, "phone.screen.widget.config") {
            ctx.pos.insert(format!("{}.config", screen_pos_key(screen, index)), cpos);
            if let Some(t) = c.as_table_like() {
                for (k, item) in t.iter() {
                    let kp = key_pos(ctx, t, k, item, cpos);
                    ctx.pos.insert(phone::config_pos_key(&id, k), kp);
                }
            }
            out.insert("config".into(), J::Object(values));
        }
    }
    out.insert("id".into(), json!(id));
    Some(J::Object(out))
}

fn check_height(ctx: &mut Ctx, v: &Item, p: (usize, usize), alone: bool, out: &mut Map<String, J>) {
    let vp = item_pos(ctx, v, p);
    let Some(val) = v.as_value() else {
        ctx.error(vp, height_message());
        return;
    };
    match val {
        Value::String(s) if s.value() == HEIGHT_FILL => {
            if alone {
                out.insert("height".into(), json!(HEIGHT_FILL));
            } else {
                ctx.error(vp, "height = \"fill\" needs the widget alone on its screen; use \"auto\" or a px height");
            }
        }
        Value::String(s) if s.value() == HEIGHT_AUTO => {
            out.insert("height".into(), json!(HEIGHT_AUTO));
        }
        other => match as_number(other).filter(|n| (f64::from(HEIGHT_MIN)..=f64::from(HEIGHT_MAX)).contains(n)) {
            Some(n) => {
                out.insert("height".into(), num_json(n));
            }
            None => {
                let at = value_pos(ctx, other, vp);
                ctx.error(at, height_message());
            }
        },
    }
}

fn article(kind: &str) -> &'static str {
    if kind.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

/// A finding about a placement: an error against the file's own widgets;
/// against a template's, a warning that refuses only new text.
fn finding(ctx: &mut Ctx, from_file: bool, pos: (usize, usize), message: String) {
    if from_file {
        ctx.error(pos, message);
    } else {
        ctx.warn(pos, message);
        if let Some(d) = ctx.out.warnings.last().cloned() {
            ctx.out.authoring.push(d);
        }
    }
}

/// The rules that need the page: every id names a content widget, once;
/// `config` only on a custom widget and small enough merged over the
/// desktop's. `content` is the page's content widgets (the file's, or the
/// template's when the file has none), `chrome` its controls.
pub(super) fn cross_check_phone(ctx: &mut Ctx, at: (usize, usize), content: &[J], chrome: &[J], from_file: bool) {
    let Some(screens) = ctx.out.layer.get("page.phone").and_then(|p| p["screens"].as_array()).cloned() else { return };
    let is_control = |id: &str| chrome.iter().any(|c| c["id"].as_str().or(c["kind"].as_str()) == Some(id));
    let mut seen: Vec<(String, usize)> = Vec::new();
    for (i, s) in screens.iter().enumerate() {
        let n = i + 1;
        for (j, row) in s["widgets"].as_array().into_iter().flatten().enumerate() {
            let id = row["id"].as_str().unwrap_or_default().to_string();
            let key = screen_pos_key(i, j);
            let pos = ctx.pos.get(&format!("{key}.id")).copied().unwrap_or(at);
            match seen.iter().find(|(x, _)| *x == id).map(|(_, first)| *first) {
                Some(first) if first == n => ctx.error(pos, format!("widget '{id}' is twice on phone screen {n}; a widget sits on one screen")),
                Some(first) => ctx.error(pos, format!("widget '{id}' is on phone screen {first} and screen {n}; a widget sits on one screen")),
                None => seen.push((id.clone(), n)),
            }
            let Some(w) = content.iter().find(|w| w["id"] == id.as_str()) else {
                if is_control(&id) {
                    finding(ctx, from_file, pos, format!("'{id}' is a control, not a widget; the phone draws its own controls"));
                } else {
                    finding(ctx, from_file, pos, format!("phone screen {n} names widget '{id}', which is not on this page"));
                }
                continue;
            };
            let kind = w["kind"].as_str().unwrap_or_default();
            if let Some(config) = row.get("config") {
                let p = ctx.pos.get(&format!("{key}.config")).copied().unwrap_or(pos);
                if kind != CUSTOM_KIND {
                    finding(ctx, from_file, p, format!("config is only for a custom widget; '{id}' is {} {kind} widget", article(kind)));
                } else {
                    let size = phone::merge_config(w, Some(config)).to_string().len();
                    if size > MAX_CONFIG_BYTES {
                        finding(ctx, from_file, p, format!("config is {size} bytes as JSON with the desktop config merged in; a custom widget's config is at most {MAX_CONFIG_BYTES} (4 KB)"));
                    }
                }
            }
            if phone::run_of_kind(kind) == phone::Run::Placeholder {
                ctx.warn(pos, format!("'{id}' is {} {kind} widget: the phone shows it as \"open this on your computer\"", article(kind)));
            }
        }
    }
    // A Conversation listed without the Agents widget it follows has no way
    // to pick an agent on the phone.
    let mut linked = content.to_vec();
    super::super::link_conversations(&mut linked);
    let listed = |id: &str| seen.iter().any(|(x, _)| x == id);
    for w in linked.iter().filter(|w| w["kind"] == "conversation") {
        let id = w["id"].as_str().unwrap_or_default();
        let Some(follows) = w["props"]["agents"].as_str() else { continue };
        if listed(id) && !listed(follows) {
            ctx.warn(at, format!("conversation '{id}' follows '{follows}', which isn't in [phone]: the phone can't pick its agent. Add '{follows}' to a screen"));
        }
    }
}
