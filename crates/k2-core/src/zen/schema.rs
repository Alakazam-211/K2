//! Zen schema v1 (prd-zen-mode-v1 Z9, Z13, Z21–Z24, amendments Z50/Z51).
//!
//! A Zen file is parsed with spans (`toml_edit`) and walked by hand, so
//! every error and warning carries `file:line:col`. A clean file becomes a
//! flat [`Layer`] (`"colors.light.canvas" → "#fff"`); layers stack
//! builtin → `zen.toml` → `pages/<home>.toml` and resolve into the JSON
//! `/cli/zen/get` returns.
//!
//! Every token, range and preset lives in the constants below. The `k2-zen`
//! skill is generated from the same constants, so a token added here without
//! docs fails its content test.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as J};
use toml_edit::{Item, TableLike, Value};

/// The only schema this K2 reads.
pub const SCHEMA_VERSION: i64 = 1;
/// The only page template Zen v1 has.
pub const TEMPLATE_ID: &str = "k2.texting@1";
/// Files over this size are refused (they are themes, not data).
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// Minimum contrast of `text` and `accent` against `canvas` (Z13).
pub const MIN_CONTRAST: f64 = 3.0;

/// Colour tokens, the same keys in `[colors.light]` and `[colors.dark]`.
/// Decision 9 (2026-10-04): no unread tracking, so there is no `unread`
/// colour; `idle` colours an agent that is not working.
pub const COLOR_TOKENS: &[&str] = &[
    "canvas",
    "surface",
    "surface-raised",
    "border",
    "text",
    "text-muted",
    "accent",
    "accent-text",
    "bubble-me",
    "bubble-me-text",
    "bubble-agent",
    "bubble-agent-text",
    "working",
    "idle",
    "needs-you",
    "danger",
];

pub const SCHEMES: &[&str] = &["auto", "light", "dark"];
pub const FAMILIES: &[&str] = &["system", "rounded", "serif", "mono"];
/// `[chrome] corners` (Z51: only the two values K2 has ever sent).
pub const CORNERS: &[&str] = &["system", "square"];
/// `[chrome] stoplights` (Z50: macOS does not let K2 hide them).
pub const STOPLIGHTS: &[&str] = &["round", "square"];
/// `[chrome] stoplight-offset = [x, y]`, each 0..=this many px.
pub const STOPLIGHT_OFFSET_MAX: f64 = 24.0;

/// A numeric token with its inclusive range.
#[derive(Debug, Clone, Copy)]
pub struct NumToken {
    pub table: &'static str,
    pub key: &'static str,
    pub min: f64,
    pub max: f64,
}

pub const NUM_TOKENS: &[NumToken] = &[
    NumToken { table: "type", key: "size", min: 12.0, max: 20.0 },
    NumToken { table: "type", key: "line-height", min: 1.2, max: 1.8 },
    NumToken { table: "shape", key: "radius", min: 0.0, max: 28.0 },
    NumToken { table: "shape", key: "bubble-radius", min: 0.0, max: 28.0 },
    NumToken { table: "shape", key: "gap", min: 4.0, max: 24.0 },
    NumToken { table: "shape", key: "list-width", min: 240.0, max: 420.0 },
];

/// The animation tree (Z22): `(name, parent)`. A child takes its parent's
/// line until it is set.
pub const ANIMATION_TREE: &[(&str, Option<&str>)] = &[
    ("global", None),
    ("zen", Some("global")),
    ("zenIn", Some("zen")),
    ("zenOut", Some("zen")),
    ("rows", Some("global")),
    ("rowIn", Some("rows")),
    ("rowMove", Some("rows")),
    ("messages", Some("global")),
    ("messageIn", Some("messages")),
    ("conversation", Some("global")),
    ("conversationSwitch", Some("conversation")),
    ("working", Some("global")),
    ("workingPulse", Some("working")),
];

/// Fixed animation style presets. `popin` takes a percentage (`popin 92%`).
pub const ANIMATION_STYLES: &[&str] = &["fade", "slide", "slidefade", "popin"];
/// `speed` is in tenths of a second (Hyprland's unit), 0..=this.
pub const SPEED_MAX_DS: f64 = 100.0;
/// Bezier control-point y range (y may leave 0–1 for overshoot).
pub const BEZIER_Y_MIN: f64 = -2.0;
pub const BEZIER_Y_MAX: f64 = 3.0;

/// Curves every file can name without defining them.
pub const BUILTIN_BEZIERS: &[(&str, [f64; 4])] = &[
    ("linear", [0.0, 0.0, 1.0, 1.0]),
    ("ease", [0.25, 0.1, 0.25, 1.0]),
    ("glide", [0.22, 1.0, 0.36, 1.0]),
    ("soft", [0.45, 0.0, 0.55, 1.0]),
    ("overshot", [0.34, 1.56, 0.64, 1.0]),
];

/// Top-level tables a theme may carry.
pub const THEME_TABLES: &[&str] =
    &["theme", "colors", "type", "shape", "chrome", "bezier", "animation"];
/// Top-level keys that open in Zen v2: warned and ignored in v1.
pub const V2_TABLES: &[&str] = &["layout", "widget", "widgets", "control", "controls"];
/// The warning for [`V2_TABLES`] (Z9).
pub const V2_WARNING: &str = "layout and widgets open in Zen v2; this table is ignored";

/// One error or warning with its position (1-based line and column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl Diagnostic {
    /// `zen.toml:7:3: unknown key 'acent'`.
    pub fn render(&self) -> String {
        format!("{}:{}:{}: {}", self.file, self.line, self.col, self.message)
    }
}

/// Which kind of file is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// `zen.toml`: the theme for every Home.
    Zen,
    /// `pages/<home>.toml`: the template line plus an optional theme override.
    Page,
}

/// A flat, validated theme layer: `"colors.light.canvas" → "#fff"`,
/// `"animation.zenIn" → {on, speed, curve, style?}`.
pub type Layer = BTreeMap<String, J>;

/// The result of checking one file's text.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    pub layer: Layer,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

impl Checked {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

struct Ctx<'a> {
    file: &'a str,
    src: &'a str,
    line_starts: Vec<usize>,
    out: Checked,
    /// Where each layer key was set, for cross-checks after the walk.
    pos: BTreeMap<String, (usize, usize)>,
}

impl<'a> Ctx<'a> {
    fn new(file: &'a str, src: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Ctx { file, src, line_starts, out: Checked::default(), pos: BTreeMap::new() }
    }

    fn at_offset(&self, off: usize) -> (usize, usize) {
        let off = off.min(self.src.len());
        let line_idx = match self.line_starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let start = self.line_starts[line_idx];
        let col = self.src.get(start..off).map(|s| s.chars().count()).unwrap_or(0) + 1;
        (line_idx + 1, col)
    }

    fn at(&self, span: Option<Range<usize>>, fallback: (usize, usize)) -> (usize, usize) {
        span.map(|r| self.at_offset(r.start)).unwrap_or(fallback)
    }

    fn error(&mut self, pos: (usize, usize), message: impl Into<String>) {
        self.out.errors.push(Diagnostic {
            file: self.file.to_string(),
            line: pos.0,
            col: pos.1,
            message: message.into(),
        });
    }

    fn warn(&mut self, pos: (usize, usize), message: impl Into<String>) {
        self.out.warnings.push(Diagnostic {
            file: self.file.to_string(),
            line: pos.0,
            col: pos.1,
            message: message.into(),
        });
    }

    fn set(&mut self, key: String, value: J, pos: (usize, usize)) {
        self.pos.insert(key.clone(), pos);
        self.out.layer.insert(key, value);
    }
}

/// Levenshtein distance, for "did you mean" hints.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn unknown_key(key: &str, where_: &str, allowed: &[&str]) -> String {
    let best = allowed
        .iter()
        .map(|a| (distance(key, a), *a))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d);
    match best {
        Some((_, a)) => format!("unknown key '{key}' in {where_}; did you mean '{a}'?"),
        None => format!("unknown key '{key}' in {where_}; allowed: {}", allowed.join(", ")),
    }
}

fn key_pos(ctx: &Ctx, t: &dyn TableLike, key: &str, item: &Item, fallback: (usize, usize)) -> (usize, usize) {
    let span = t.key(key).and_then(|k| k.span()).or_else(|| item.span());
    ctx.at(span, fallback)
}

fn item_pos(ctx: &Ctx, item: &Item, fallback: (usize, usize)) -> (usize, usize) {
    ctx.at(item.span(), fallback)
}

fn value_pos(ctx: &Ctx, v: &Value, fallback: (usize, usize)) -> (usize, usize) {
    ctx.at(v.span(), fallback)
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i.value() as f64),
        Value::Float(f) => Some(*f.value()),
        _ => None,
    }
}

fn num_json(n: f64) -> J {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// Parse a colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`,
/// `rgba(r, g, b, a)`. Returns `[r, g, b, a]` with rgb 0–255 and a 0–1.
pub fn parse_color(s: &str) -> Option<[f64; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let d = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
        let expand = |c: u8| (c * 17) as f64;
        return match hex.len() {
            3 => Some([expand(d(0, 1)?), expand(d(1, 1)?), expand(d(2, 1)?), 1.0]),
            4 => Some([
                expand(d(0, 1)?),
                expand(d(1, 1)?),
                expand(d(2, 1)?),
                expand(d(3, 1)?) / 255.0,
            ]),
            6 => Some([d(0, 2)? as f64, d(2, 2)? as f64, d(4, 2)? as f64, 1.0]),
            8 => Some([d(0, 2)? as f64, d(2, 2)? as f64, d(4, 2)? as f64, d(6, 2)? as f64 / 255.0]),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    let (inner, want) = if let Some(r) = lower.strip_prefix("rgba(") {
        (r.strip_suffix(')')?, 4)
    } else if let Some(r) = lower.strip_prefix("rgb(") {
        (r.strip_suffix(')')?, 3)
    } else {
        return None;
    };
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != want {
        return None;
    }
    let mut out = [0.0, 0.0, 0.0, 1.0];
    for (i, p) in parts.iter().enumerate().take(3) {
        let v: f64 = p.parse().ok()?;
        if !(0.0..=255.0).contains(&v) {
            return None;
        }
        out[i] = v;
    }
    if want == 4 {
        let a: f64 = parts[3].parse().ok()?;
        if !(0.0..=1.0).contains(&a) {
            return None;
        }
        out[3] = a;
    }
    Some(out)
}

fn channel(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(rgb: [f64; 3]) -> f64 {
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

fn over(fg: [f64; 4], bg: [f64; 3]) -> [f64; 3] {
    let a = fg[3];
    [
        fg[0] * a + bg[0] * (1.0 - a),
        fg[1] * a + bg[1] * (1.0 - a),
        fg[2] * a + bg[2] * (1.0 - a),
    ]
}

/// WCAG contrast of `fg` on `canvas`. A translucent canvas sits on white in
/// the light scheme and on black in the dark one; a translucent `fg` sits on
/// the canvas.
pub fn contrast(fg: [f64; 4], canvas: [f64; 4], dark: bool) -> f64 {
    let base = if dark { [0.0, 0.0, 0.0] } else { [255.0, 255.0, 255.0] };
    let bg = over(canvas, base);
    let fg = over(fg, bg);
    let (l1, l2) = (luminance(fg), luminance(bg));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

fn valid_curve_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `fade` | `slide` | `slidefade` | `popin <n>%` (n 0–100).
pub fn parse_style(s: &str) -> Result<String, String> {
    let t = s.trim();
    match t {
        "fade" | "slide" | "slidefade" => return Ok(t.to_string()),
        "popin" => return Ok("popin 80%".to_string()),
        _ => {}
    }
    if let Some(rest) = t.strip_prefix("popin ") {
        if let Some(n) = rest.trim().strip_suffix('%') {
            if let Ok(n) = n.trim().parse::<u32>() {
                if n <= 100 {
                    return Ok(format!("popin {n}%"));
                }
            }
        }
        return Err(format!("'{t}' needs a percentage from 0 to 100, like 'popin 92%'"));
    }
    Err(format!("unknown style '{t}'; use fade, slide, slidefade or popin <n>%"))
}

fn check_scalar_enum(ctx: &mut Ctx, key: String, item: &Item, pos: (usize, usize), allowed: &[&str], what: &str) {
    match item.as_value().and_then(Value::as_str) {
        Some(s) if allowed.contains(&s) => ctx.set(key, json!(s), pos),
        Some(s) => {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("{what} '{s}' is not one of: {}", allowed.join(", ")));
        }
        None => {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("{what} must be a string: {}", allowed.join(", ")));
        }
    }
}

fn require_table<'i>(ctx: &mut Ctx, item: &'i Item, pos: (usize, usize), name: &str) -> Option<&'i dyn TableLike> {
    match item.as_table_like() {
        Some(t) => Some(t),
        None => {
            ctx.error(pos, format!("`{name}` must be a table, like [{name}]"));
            None
        }
    }
}

fn check_theme(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (k, item) in t.iter() {
        let pos = key_pos(ctx, t, k, item, at);
        match k {
            "scheme" => check_scalar_enum(ctx, "theme.scheme".into(), item, pos, SCHEMES, "scheme"),
            other => ctx.error(pos, unknown_key(other, "[theme]", &["scheme"])),
        }
    }
}

fn check_colors(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (scheme, item) in t.iter() {
        let pos = key_pos(ctx, t, scheme, item, at);
        if scheme != "light" && scheme != "dark" {
            ctx.error(pos, unknown_key(scheme, "[colors]", &["light", "dark"]));
            continue;
        }
        let name = format!("colors.{scheme}");
        let Some(sub) = require_table(ctx, item, pos, &name) else { continue };
        for (tok, v) in sub.iter() {
            let tpos = key_pos(ctx, sub, tok, v, pos);
            if !COLOR_TOKENS.contains(&tok) {
                ctx.error(tpos, unknown_key(tok, &format!("[{name}]"), COLOR_TOKENS));
                continue;
            }
            match v.as_value().and_then(Value::as_str) {
                Some(s) if parse_color(s).is_some() => {
                    ctx.set(format!("{name}.{tok}"), json!(s.trim()), tpos)
                }
                Some(s) => {
                    let p = item_pos(ctx, v, tpos);
                    ctx.error(p, format!("'{s}' is not a colour; use #rrggbb, #rgb, rgb(r, g, b) or rgba(r, g, b, a)"));
                }
                None => {
                    let p = item_pos(ctx, v, tpos);
                    ctx.error(p, format!("{name}.{tok} must be a colour string like \"#1f1c18\""));
                }
            }
        }
    }
}

fn check_numbers(ctx: &mut Ctx, table: &str, t: &dyn TableLike, at: (usize, usize), extra: &[&str]) {
    let mut allowed: Vec<&str> =
        NUM_TOKENS.iter().filter(|n| n.table == table).map(|n| n.key).collect();
    allowed.extend_from_slice(extra);
    for (k, item) in t.iter() {
        let pos = key_pos(ctx, t, k, item, at);
        if let Some(tok) = NUM_TOKENS.iter().find(|n| n.table == table && n.key == k) {
            match item.as_value().and_then(as_number) {
                Some(n) if n >= tok.min && n <= tok.max => {
                    ctx.set(format!("{table}.{k}"), num_json(n), pos)
                }
                Some(n) => {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, format!("{table}.{k} = {n} is out of range; use {} to {}", num_json(tok.min), num_json(tok.max)));
                }
                None => {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, format!("{table}.{k} must be a number from {} to {}", num_json(tok.min), num_json(tok.max)));
                }
            }
        } else if table == "type" && k == "family" {
            check_scalar_enum(ctx, "type.family".into(), item, pos, FAMILIES, "family");
        } else {
            ctx.error(pos, unknown_key(k, &format!("[{table}]"), &allowed));
        }
    }
}

fn check_chrome(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (k, item) in t.iter() {
        let pos = key_pos(ctx, t, k, item, at);
        match k {
            "corners" => check_scalar_enum(ctx, "chrome.corners".into(), item, pos, CORNERS, "corners"),
            "stoplights" => {
                if item.as_value().and_then(Value::as_str) == Some("hidden") {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, "stoplights can't be 'hidden': macOS doesn't let K2 hide the window buttons; use round or square");
                } else {
                    check_scalar_enum(ctx, "chrome.stoplights".into(), item, pos, STOPLIGHTS, "stoplights");
                }
            }
            "stoplight-offset" => {
                let arr = item.as_value().and_then(Value::as_array);
                let nums: Option<Vec<f64>> = arr.map(|a| a.iter().filter_map(as_number).collect());
                match (arr, nums) {
                    (Some(a), Some(n)) if a.len() == 2 && n.len() == 2 => {
                        if n.iter().all(|v| *v >= 0.0 && *v <= STOPLIGHT_OFFSET_MAX) {
                            ctx.set("chrome.stoplight-offset".into(), json!([num_json(n[0]), num_json(n[1])]), pos);
                        } else {
                            let p = item_pos(ctx, item, pos);
                            ctx.error(p, "stoplight-offset values must be 0 to 24 px each");
                        }
                    }
                    _ => {
                        let p = item_pos(ctx, item, pos);
                        ctx.error(p, "stoplight-offset must be [x, y] in px, each 0 to 24");
                    }
                }
            }
            "corner-radius" | "radius" => {
                ctx.error(pos, "corners are 'system' or 'square' in Zen v1; other radii are untested on macOS");
            }
            other => ctx.error(pos, unknown_key(other, "[chrome]", &["corners", "stoplights", "stoplight-offset"])),
        }
    }
}

fn check_bezier(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (name, item) in t.iter() {
        let pos = key_pos(ctx, t, name, item, at);
        if !valid_curve_name(name) {
            ctx.error(pos, format!("curve name '{name}' must start with a letter and use letters, digits, - or _"));
            continue;
        }
        let arr = item.as_value().and_then(Value::as_array);
        let nums: Vec<f64> = arr.map(|a| a.iter().filter_map(as_number).collect()).unwrap_or_default();
        if arr.map(|a| a.len()) != Some(4) || nums.len() != 4 {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("bezier '{name}' must be [x1, y1, x2, y2]"));
            continue;
        }
        if !(0.0..=1.0).contains(&nums[0]) || !(0.0..=1.0).contains(&nums[2]) {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("bezier '{name}': x1 and x2 must be 0 to 1"));
            continue;
        }
        if !(BEZIER_Y_MIN..=BEZIER_Y_MAX).contains(&nums[1]) || !(BEZIER_Y_MIN..=BEZIER_Y_MAX).contains(&nums[3]) {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("bezier '{name}': y1 and y2 must be -2 to 3"));
            continue;
        }
        ctx.set(format!("bezier.{name}"), json!(nums.iter().map(|n| num_json(*n)).collect::<Vec<_>>()), pos);
    }
}

fn check_animation(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    let names: Vec<&str> = ANIMATION_TREE.iter().map(|(n, _)| *n).collect();
    for (name, item) in t.iter() {
        let pos = key_pos(ctx, t, name, item, at);
        if !names.contains(&name) {
            ctx.error(pos, unknown_key(name, "[animation]", &names));
            continue;
        }
        let vpos = item_pos(ctx, item, pos);
        let Some(arr) = item.as_value().and_then(Value::as_array) else {
            ctx.error(vpos, format!("{name} must be [on, speed, curve] or [on, speed, curve, style]"));
            continue;
        };
        let vals: Vec<&Value> = arr.iter().collect();
        let on = match vals.first() {
            Some(Value::Integer(i)) if *i.value() == 0 || *i.value() == 1 => *i.value() == 1,
            Some(Value::Boolean(b)) => *b.value(),
            _ => {
                ctx.error(vpos, format!("{name}: the first value is on/off, 1 or 0"));
                continue;
            }
        };
        if !on && vals.len() == 1 {
            ctx.set(format!("animation.{name}"), json!({ "on": false }), pos);
            continue;
        }
        if vals.len() < 3 || vals.len() > 4 {
            ctx.error(vpos, format!("{name} must be [on, speed, curve] or [on, speed, curve, style]"));
            continue;
        }
        let speed = match as_number(vals[1]) {
            Some(s) if (0.0..=SPEED_MAX_DS).contains(&s) => s,
            _ => {
                let p = value_pos(ctx, vals[1], vpos);
                ctx.error(p, format!("{name}: speed is tenths of a second, 0 to 100"));
                continue;
            }
        };
        let curve = match vals[2].as_str() {
            Some(c) if valid_curve_name(c) => c.to_string(),
            _ => {
                let p = value_pos(ctx, vals[2], vpos);
                ctx.error(p, format!("{name}: the third value is a curve name, like \"glide\""));
                continue;
            }
        };
        let mut spec = json!({ "on": on, "speed": num_json(speed), "curve": curve });
        if let Some(sv) = vals.get(3) {
            match sv.as_str().map(parse_style) {
                Some(Ok(style)) => {
                    spec["style"] = json!(style);
                }
                Some(Err(msg)) => {
                    let p = value_pos(ctx, sv, vpos);
                    ctx.error(p, format!("{name}: {msg}"));
                    continue;
                }
                None => {
                    let p = value_pos(ctx, sv, vpos);
                    ctx.error(p, format!("{name}: style must be a string"));
                    continue;
                }
            }
        }
        // Remember the curve's own position for the cross-check.
        let cpos = value_pos(ctx, vals[2], vpos);
        ctx.pos.insert(format!("animation.{name}#curve"), cpos);
        ctx.set(format!("animation.{name}"), spec, pos);
    }
}

/// Check one file's text. `base` is every layer below this file, builtin
/// included (it supplies curves and the canvas for the contrast check).
pub fn check(file: &str, src: &str, kind: FileKind, base: &Layer) -> Checked {
    let mut ctx = Ctx::new(file, src);
    if src.len() > MAX_FILE_BYTES {
        ctx.error((1, 1), format!("this file is {} bytes; Zen files must be under 64 KB", src.len()));
        return ctx.out;
    }
    let doc = match toml_edit::Document::parse(src) {
        Ok(d) => d,
        Err(e) => {
            let pos = ctx.at(e.span(), (1, 1));
            let msg = e.message().lines().next().unwrap_or("invalid TOML").trim().to_string();
            ctx.error(pos, format!("invalid TOML: {msg}"));
            return ctx.out;
        }
    };
    let root = doc.as_table();
    let mut saw_schema = false;
    for (k, item) in root.iter() {
        let pos = key_pos(&ctx, root, k, item, (1, 1));
        match k {
            "schema" => {
                saw_schema = true;
                match item.as_value() {
                    Some(Value::Integer(i)) if *i.value() == SCHEMA_VERSION => {}
                    Some(Value::Integer(i)) => {
                        let p = item_pos(&ctx, item, pos);
                        ctx.error(p, format!("schema = {} is not supported; this K2 reads Zen schema 1", i.value()));
                    }
                    _ => {
                        let p = item_pos(&ctx, item, pos);
                        ctx.error(p, "schema must be the number 1; this K2 reads Zen schema 1");
                    }
                }
            }
            "template" if kind == FileKind::Page => match item.as_value().and_then(Value::as_str) {
                Some(TEMPLATE_ID) => {}
                Some(other) => {
                    let p = item_pos(&ctx, item, pos);
                    ctx.error(p, format!("unknown template '{other}'; Zen v1 has one template: {TEMPLATE_ID}"));
                }
                None => {
                    let p = item_pos(&ctx, item, pos);
                    ctx.error(p, format!("template must be the string \"{TEMPLATE_ID}\""));
                }
            },
            "template" => ctx.error(pos, "unknown key 'template' in zen.toml; the template line belongs in pages/<home-id>.toml"),
            k if V2_TABLES.contains(&k) => ctx.warn(pos, V2_WARNING),
            "theme" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "theme") {
                    check_theme(&mut ctx, t, pos);
                }
            }
            "colors" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "colors") {
                    check_colors(&mut ctx, t, pos);
                }
            }
            "type" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "type") {
                    check_numbers(&mut ctx, "type", t, pos, &["family"]);
                }
            }
            "shape" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "shape") {
                    check_numbers(&mut ctx, "shape", t, pos, &[]);
                }
            }
            "chrome" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "chrome") {
                    check_chrome(&mut ctx, t, pos);
                }
            }
            "bezier" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "bezier") {
                    check_bezier(&mut ctx, t, pos);
                }
            }
            "animation" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "animation") {
                    check_animation(&mut ctx, t, pos);
                }
            }
            other => {
                let mut allowed: Vec<&str> = vec!["schema"];
                if kind == FileKind::Page {
                    allowed.push("template");
                }
                allowed.extend_from_slice(THEME_TABLES);
                ctx.error(pos, unknown_key(other, "the top level", &allowed));
            }
        }
    }
    if !saw_schema {
        ctx.error((1, 1), "missing `schema = 1` (the first line of every Zen file)");
    }
    cross_check(&mut ctx, base);
    ctx.out
}

/// Checks that need the layers below: curve names and contrast.
fn cross_check(ctx: &mut Ctx, base: &Layer) {
    let layer = ctx.out.layer.clone();
    for (key, spec) in &layer {
        let Some(name) = key.strip_prefix("animation.") else { continue };
        let Some(curve) = spec.get("curve").and_then(J::as_str) else { continue };
        let known = BUILTIN_BEZIERS.iter().any(|(n, _)| *n == curve)
            || layer.contains_key(&format!("bezier.{curve}"))
            || base.contains_key(&format!("bezier.{curve}"));
        if !known {
            let pos = ctx.pos.get(&format!("{key}#curve")).copied().unwrap_or((1, 1));
            ctx.error(pos, format!("{name}: unknown curve '{curve}'; define it in [bezier] or use one of: {}", BUILTIN_BEZIERS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")));
        }
    }
    for scheme in ["light", "dark"] {
        let get = |tok: &str| -> Option<String> {
            let k = format!("colors.{scheme}.{tok}");
            layer.get(&k).or_else(|| base.get(&k)).and_then(J::as_str).map(str::to_string)
        };
        let here = |tok: &str| layer.contains_key(&format!("colors.{scheme}.{tok}"));
        if !here("canvas") && !here("text") && !here("accent") {
            continue;
        }
        let Some(canvas) = get("canvas").as_deref().and_then(parse_color) else { continue };
        for fg in ["text", "accent"] {
            let Some(raw) = get(fg) else { continue };
            let Some(c) = parse_color(&raw) else { continue };
            let ratio = contrast(c, canvas, scheme == "dark");
            if ratio < MIN_CONTRAST {
                let at_key = if here(fg) { fg } else { "canvas" };
                let pos = ctx.pos.get(&format!("colors.{scheme}.{at_key}")).copied().unwrap_or((1, 1));
                ctx.error(
                    pos,
                    format!(
                        "colors.{scheme}: {fg} {raw} on canvas {} has contrast {ratio:.1}:1; it needs at least 3:1 so the Zen controls stay visible",
                        get("canvas").unwrap_or_default()
                    ),
                );
            }
        }
    }
}

/// Stack layers: later wins per key.
pub fn merge(layers: &[&Layer]) -> Layer {
    let mut out = Layer::new();
    for l in layers {
        for (k, v) in l.iter() {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn parent_of(name: &str) -> Option<&'static str> {
    ANIMATION_TREE.iter().find(|(n, _)| *n == name).and_then(|(_, p)| *p)
}

fn chain(name: &str) -> Vec<&str> {
    let mut out = vec![name];
    let mut cur = name;
    while let Some(p) = parent_of(cur) {
        out.push(p);
        cur = p;
    }
    out
}

fn ease_css(b: &[f64]) -> String {
    let f = |v: f64| {
        let s = format!("{v:.3}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s == "-0" { "0".to_string() } else { s }
    };
    format!("cubic-bezier({}, {}, {}, {})", f(b[0]), f(b[1]), f(b[2]), f(b[3]))
}

/// The resolved theme, chrome and motion for one Home.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTheme {
    pub theme: J,
    pub chrome: J,
    pub motion: J,
}

/// Resolve `user` (zen.toml + page override, page winning) over `builtin`.
///
/// Animations follow Z22: a node takes the nearest line set by the user on
/// its chain (itself, then its parents up to `global`); only when the user
/// set none does the builtin chain apply. So a user's `global` slows every
/// animation the user didn't set, builtin leaves included. A line without a
/// style takes the nearest style on the same chain.
pub fn resolve(builtin: &Layer, user: &Layer) -> ResolvedTheme {
    let merged = merge(&[builtin, user]);
    let s = |k: &str| merged.get(k).cloned().unwrap_or(J::Null);

    let mut colors = Map::new();
    for scheme in ["light", "dark"] {
        let mut m = Map::new();
        for tok in COLOR_TOKENS {
            m.insert((*tok).to_string(), s(&format!("colors.{scheme}.{tok}")));
        }
        colors.insert(scheme.to_string(), J::Object(m));
    }
    let mut type_ = Map::new();
    type_.insert("family".into(), s("type.family"));
    let mut shape = Map::new();
    for n in NUM_TOKENS {
        let target = if n.table == "type" { &mut type_ } else { &mut shape };
        target.insert(n.key.to_string(), s(&format!("{}.{}", n.table, n.key)));
    }
    let theme = json!({
        "scheme": s("theme.scheme"),
        "colors": colors,
        "type": type_,
        "shape": shape,
    });
    let chrome = json!({
        "corners": s("chrome.corners"),
        "stoplights": s("chrome.stoplights"),
        "stoplight-offset": s("chrome.stoplight-offset"),
    });

    let mut beziers = Map::new();
    for (n, b) in BUILTIN_BEZIERS {
        beziers.insert((*n).to_string(), json!(b.iter().map(|v| num_json(*v)).collect::<Vec<_>>()));
    }
    for (k, v) in &merged {
        if let Some(n) = k.strip_prefix("bezier.") {
            beziers.insert(n.to_string(), v.clone());
        }
    }

    let mut animations = Map::new();
    for (name, _) in ANIMATION_TREE {
        let ch = chain(name);
        let find = |layer: &Layer| -> Option<(String, J)> {
            ch.iter().find_map(|n| layer.get(&format!("animation.{n}")).map(|v| (n.to_string(), v.clone())))
        };
        let (from, spec) = find(user)
            .or_else(|| find(builtin))
            .unwrap_or_else(|| ("global".into(), json!({"on": true, "speed": 3, "curve": "glide"})));
        let on = spec.get("on").and_then(J::as_bool).unwrap_or(true);
        let speed = spec.get("speed").and_then(J::as_f64).unwrap_or(0.0);
        let curve = spec.get("curve").and_then(J::as_str).unwrap_or("linear").to_string();
        let style = spec.get("style").cloned().or_else(|| {
            ch.iter().find_map(|n| {
                user.get(&format!("animation.{n}"))
                    .and_then(|v| v.get("style").cloned())
            })
            .or_else(|| {
                ch.iter().find_map(|n| {
                    builtin.get(&format!("animation.{n}")).and_then(|v| v.get("style").cloned())
                })
            })
        });
        let bez: Vec<f64> = beziers
            .get(&curve)
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_f64).collect())
            .unwrap_or_else(|| vec![0.0, 0.0, 1.0, 1.0]);
        let duration_ms = if on { (speed * 100.0).round() as i64 } else { 0 };
        animations.insert(
            (*name).to_string(),
            json!({
                "on": on,
                "speed": num_json(if on { speed } else { 0.0 }),
                "durationMs": duration_ms,
                "curve": curve,
                "bezier": bez.iter().map(|v| num_json(*v)).collect::<Vec<_>>(),
                "ease": ease_css(&bez),
                "style": style.unwrap_or(J::Null),
                "from": from,
            }),
        );
    }
    let motion = json!({
        "beziers": beziers,
        "animations": animations,
        // Z22: prefers-reduced-motion turns every Zen animation instant.
        "reducedMotion": "instant",
    });
    ResolvedTheme { theme, chrome, motion }
}

/// 64-bit FNV-1a, hex. Stable across Rust versions.
pub fn fnv_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
