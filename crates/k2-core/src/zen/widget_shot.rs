//! `k2 zen widget preview --dir <folder> --shot <png>` (0.46.3, Garden
//! gaps): a staged widget folder rendered headless, with no person at a Zen
//! window and nothing written in `~/.k2`.
//!
//! The daemon assembles everything a Zen window would (daemon-first): the
//! folder is checked and bundled exactly like a widget folder
//! ([`super::widgets::check_files`]), then wrapped the way the renderer
//! wraps a widget frame:
//!
//! - [`frame_src_doc`] mirrors `frameSrcDoc(html, 'widget', {nonce,
//!   prelude})` (`src/renderer/lib/frame-csp.ts`): the dns-prefetch and CSP
//!   metas after a leading doctype;
//! - [`frame_prelude`] mirrors `zenFramePrelude` (`zen-custom-prelude.ts`):
//!   the sealed-frame guard, the Garden's `--zen-*` theme style (its scheme
//!   `!important`), the scheme attribute, `k2-frame.js` (the same generated
//!   file), then one nonced script per `requires.libs` library.
//!   `src/shared/zen-frame-shape.golden.html` pins the shape both sides make
//!   (a Rust test here and a vitest test on the TS side read it).
//! - [`shot_host`]: the parent page, a Garden canvas of the chosen theme with
//!   the frame in it and a **stand-in bridge** answering the frame's port
//!   like the host bridge does, from sample data (or the caller's `--data`
//!   fixtures). It records `ready`, runtime errors and calls for the runner.
//!
//! Libraries: a bundled library's text lives in the desktop renderer, not
//! the daemon, so its script is a slot (`__K2_SHOT_LIB_<n>__`) the runner
//! (`cli/k2-widget-shot.mjs`) fills from K2's `zen-lib` folder after
//! checking each file's sha256. Download and CDN libraries are refused.
//!
//! The runner renders the page in Playwright's WebKit (the desktop
//! webview's engine) and writes the PNG. Nothing here writes a file.

use std::collections::BTreeMap;

use serde_json::{json, Value as J};

use super::bundle::new_nonce;
use super::schema::font_family;
use super::stdlib::{self, LibRef, LibSource};
use super::store::ZenError;
use super::widgets;

/// The generated frame runtime (`contract-gen`), the same text the renderer
/// inlines (`zen-custom-prelude.ts`).
pub const K2_FRAME_JS: &str = include_str!("../../../../sdk/generated/k2-frame.js");
/// The sealed-frame guard (`src/shared/sealed-frame-guard.js`).
pub const SEALED_FRAME_GUARD: &str = include_str!("../../../../src/shared/sealed-frame-guard.js");
/// The shape both prelude builders must make (see the module doc).
pub const FRAME_SHAPE_GOLDEN: &str = include_str!("../../../../src/shared/zen-frame-shape.golden.html");
/// The parent page with the stand-in bridge.
const HOST_TEMPLATE: &str = include_str!("widget_shot_host.html");

/// `ZEN_FRAME_FEATURES` (`ZenCustomFrame.tsx`): the features a Zen window's
/// hello names. The golden file pins this list too.
pub const FRAME_FEATURES: &[&str] = &["zen-v1", "zen-gardens-v1", "zen-widgets-v1", "sources-v1", "garden-data-v1", "guests-rooms-v1"];

/// The shot's size limits (CSS pixels).
pub const MIN_SIDE: u32 = 120;
pub const MAX_SIDE: u32 = 2400;
/// Default size: about one Garden card.
pub const DEFAULT_WIDTH: u32 = 420;
pub const DEFAULT_HEIGHT: u32 = 360;
/// At most this many bytes of `--data` fixtures.
pub const MAX_DATA_BYTES: usize = 512 * 1024;
/// At most this many config keys.
const MAX_CONFIG: usize = 16;

/// The Garden's light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
}

impl Scheme {
    pub fn parse(s: &str) -> Option<Scheme> {
        match s {
            "light" => Some(Scheme::Light),
            "dark" => Some(Scheme::Dark),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Light => "light",
            Scheme::Dark => "dark",
        }
    }
}

/// What the caller asks for besides the files.
#[derive(Debug, Clone)]
pub struct ShotOptions {
    /// `None`: the theme's own fixed scheme, else light.
    pub scheme: Option<Scheme>,
    /// A built-in Zen theme (default `basic`).
    pub theme: String,
    pub config: BTreeMap<String, String>,
    /// Fixtures for the stand-in bridge (see `widget_shot_host.html`).
    pub data: Option<J>,
    pub width: u32,
    pub height: u32,
}

impl Default for ShotOptions {
    fn default() -> Self {
        ShotOptions { scheme: None, theme: super::DEFAULT_THEME.into(), config: BTreeMap::new(), data: None, width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT }
    }
}

/// A theme value that can sit inside a style rule (`safeCssValue`).
fn safe_css_value(v: &str) -> bool {
    !(v.contains(['<', '>', '{', '}', ';', '\\']) || v.contains("/*"))
}

fn toml_table<'a>(v: &'a toml::Value, path: &[&str]) -> Option<&'a toml::value::Table> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_table()
}

/// A built-in theme's `--zen-*` variables for a scheme (its file over
/// `basic`, as every theme sits on basic), and the scheme it is fixed to
/// (`[theme] scheme = "light" | "dark"`), if any.
pub fn theme_vars(name: &str, scheme: Option<Scheme>) -> Result<(BTreeMap<String, String>, Scheme), ZenError> {
    let parse = |n: &str| -> Result<toml::Value, ZenError> {
        let t = super::builtin_theme(n).ok_or_else(|| {
            let known: Vec<&str> = super::BUILTIN_THEMES.iter().map(|t| t.name).collect();
            ZenError::BadRequest(format!("'{n}' is not a built-in Zen theme ({})", known.join(", ")))
        })?;
        t.toml.parse::<toml::Value>().map_err(|e| ZenError::Io(format!("built-in theme {n}: {e}")))
    };
    let base = parse(super::DEFAULT_THEME)?;
    let top = parse(name)?;
    let fixed = top.get("theme").and_then(|t| t.get("scheme")).and_then(toml::Value::as_str).and_then(Scheme::parse);
    let scheme = scheme.or(fixed).unwrap_or(Scheme::Light);
    let mut vars = BTreeMap::new();
    for layer in [&base, &top] {
        if let Some(colors) = toml_table(layer, &["colors", scheme.as_str()]) {
            for (k, v) in colors {
                if let Some(s) = v.as_str() {
                    vars.insert(format!("--zen-{k}"), s.to_string());
                }
            }
        }
        if let Some(font) = toml_table(layer, &["font"]) {
            if let Some((stack, _)) = font.get("family").and_then(toml::Value::as_str).and_then(font_family) {
                vars.insert("--zen-font-family".into(), stack.to_string());
            }
            if let Some(n) = font.get("size").and_then(|v| v.as_integer().map(|i| i as f64).or(v.as_float())) {
                vars.insert("--zen-font-size".into(), format!("{n}px"));
            }
            if let Some(n) = font.get("line-height").and_then(|v| v.as_float().or(v.as_integer().map(|i| i as f64))) {
                vars.insert("--zen-line-height".into(), format!("{n}"));
            }
        }
        if let Some(shape) = toml_table(layer, &["shape"]) {
            for key in ["radius", "bubble-radius", "gap", "list-width"] {
                if let Some(n) = shape.get(key).and_then(|v| v.as_integer().map(|i| i as f64).or(v.as_float())) {
                    vars.insert(format!("--zen-{key}"), format!("{n}px"));
                }
            }
        }
    }
    vars.retain(|k, v| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && safe_css_value(v));
    Ok((vars, scheme))
}

/// `zenScriptText`: `</script` → `<\/script`, the same in JS.
pub fn script_text(text: &str) -> String {
    // ASCII lower-casing keeps byte offsets, so `lower`'s matches index `text`.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (i, _) in lower.match_indices("</script") {
        out.push_str(&text[at..i]);
        out.push_str("<\\/");
        at = i + 2;
    }
    out.push_str(&text[at..]);
    out
}

fn nonced(nonce: &str, text: &str) -> String {
    format!("<script nonce=\"{nonce}\">{}</script>", script_text(text))
}

/// `zenFrameThemeCss(theme, ownScheme)`: the Garden's scheme `!important`
/// unless the widget's manifest says `"colorScheme": "own"`.
pub fn frame_theme_css(vars: &BTreeMap<String, String>, scheme: Scheme, own_scheme: bool) -> String {
    let mut lines = String::new();
    for (k, v) in vars {
        if k.starts_with("--zen-") && k.len() > 6 && k[6..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && safe_css_value(v) {
            lines.push_str(&format!("{k}:{v};"));
        }
    }
    format!(
        "<style data-k2-theme=\"\">:root{{{lines}color-scheme:{}{};background:transparent;}}html,body{{background:transparent;}}</style>",
        scheme.as_str(),
        if own_scheme { "" } else { "!important" }
    )
}

/// `zenFramePrelude({nonce, theme, libs, ownScheme})` with `libs` already
/// as script texts (or slots).
pub fn frame_prelude(nonce: &str, vars: &BTreeMap<String, String>, scheme: Scheme, libs: &[String], own_scheme: bool) -> String {
    let own = if own_scheme { "document.documentElement.setAttribute('data-k2-scheme', 'own');" } else { "" };
    let attr = format!("document.documentElement.setAttribute('data-zen-scheme', {});{own}", json!(scheme.as_str()));
    let mut out = nonced(nonce, SEALED_FRAME_GUARD);
    out.push_str(&frame_theme_css(vars, scheme, own_scheme));
    out.push_str(&nonced(nonce, &attr));
    out.push_str(&nonced(nonce, K2_FRAME_JS));
    for l in libs {
        out.push_str(&nonced(nonce, l));
    }
    out
}

/// `frameCsp('widget', {nonce})`.
pub fn widget_csp(nonce: &str) -> String {
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; img-src data: blob:; media-src data: blob:; \
         font-src data:; connect-src data: blob:; frame-src 'none'; child-src 'none'; worker-src blob:; object-src 'none'; \
         manifest-src 'none'; form-action 'none'; base-uri 'none'"
    )
}

/// `DNS_PREFETCH_OFF_META`.
pub const DNS_PREFETCH_OFF_META: &str = "<meta http-equiv=\"x-dns-prefetch-control\" content=\"off\">";

/// The length of a leading `<!doctype …>` (`LEADING_DOCTYPE` in
/// frame-csp.ts: an optional BOM, whitespace, `<!doctype` then up to `>`).
fn leading_doctype(html: &str) -> Option<usize> {
    let s = html.strip_prefix('\u{feff}').unwrap_or(html);
    let bom = html.len() - s.len();
    let trimmed = s.trim_start_matches(['\t', '\n', '\x0c', '\r', ' ']);
    let ws = s.len() - trimmed.len();
    if trimmed.len() < 9 || !trimmed[..9].eq_ignore_ascii_case("<!doctype") {
        return None;
    }
    let after = &trimmed[9..];
    if !after.chars().next().is_some_and(|c| !(c.is_ascii_alphanumeric() || c == '_')) {
        return None;
    }
    let end = after.find('>')?;
    Some(bom + ws + 9 + end + 1)
}

/// `frameSrcDoc(html, 'widget', {nonce, prelude})`.
pub fn frame_src_doc(html: &str, nonce: &str, prelude: &str) -> String {
    let head = format!(
        "{DNS_PREFETCH_OFF_META}<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">{prelude}",
        widget_csp(nonce)
    );
    match leading_doctype(html) {
        Some(n) => format!("{}{head}{}", &html[..n], &html[n..]),
        None => format!("{head}{html}"),
    }
}

/// JSON safe inside a `<script>` element (no `<`, no line separators), and
/// with no `_`, so no value can spell a `__K2_SHOT_*__` slot. JSON has `_`
/// only inside strings, where `_` means the same.
fn script_json(v: &J) -> String {
    v.to_string()
        .replace('<', "\\u003c")
        .replace('_', "\\u005f")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// The parent page: the canvas, the frame slot and the stand-in bridge.
/// The frame's srcdoc goes in `__K2_SHOT_FRAME__` (the runner fills it
/// after the library slots, as a JSON string).
pub fn shot_host(hello: &J, theme_info: &J, data: &J, canvas: &str, scheme: Scheme, width: u32, height: u32) -> String {
    HOST_TEMPLATE
        .replace("__K2_SHOT_SCHEME__", scheme.as_str())
        .replace("__K2_SHOT_CANVAS__", if safe_css_value(canvas) { canvas } else { "transparent" })
        .replace("__K2_SHOT_WIDTH__", &width.to_string())
        .replace("__K2_SHOT_HEIGHT__", &height.to_string())
        .replace("__K2_SHOT_HELLO__", &script_json(hello))
        .replace("__K2_SHOT_THEME__", &script_json(theme_info))
        .replace("__K2_SHOT_DATA__", &script_json(data))
}

/// Check, bundle and wrap a staged folder for a shot. Writes nothing and
/// needs no Zen folder. A folder with errors answers `{ok: false, status:
/// "failed", errors, …}` (the `validate --dir` shape); a clean one `{ok:
/// true, widget, hash, nonce, frame, host, libs, width, height, scheme,
/// theme, warnings}`.
pub fn shot_page(name: &str, files: &BTreeMap<String, Vec<u8>>, o: &ShotOptions) -> Result<J, ZenError> {
    if !(MIN_SIDE..=MAX_SIDE).contains(&o.width) || !(MIN_SIDE..=MAX_SIDE).contains(&o.height) {
        return Err(ZenError::BadRequest(format!("--width and --height are {MIN_SIDE} to {MAX_SIDE} pixels")));
    }
    if o.config.len() > MAX_CONFIG {
        return Err(ZenError::BadRequest(format!("at most {MAX_CONFIG} config keys")));
    }
    if let Some(d) = &o.data {
        if !d.is_object() {
            return Err(ZenError::BadRequest("--data is one JSON object (keys: \"<verb>\" or \"<verb> <address>\", and \"store\")".into()));
        }
        if d.to_string().len() > MAX_DATA_BYTES {
            return Err(ZenError::BadRequest(format!("--data is at most {} KB", MAX_DATA_BYTES / 1024)));
        }
    }
    let (vars, scheme) = theme_vars(&o.theme, o.scheme)?;
    let prefix = format!("staged/{name}");
    let c = widgets::check_files(&prefix, name, files);
    let (Some(manifest), Some(bundle), true) = (c.manifest.as_ref(), c.bundle.as_ref(), c.errors.is_empty()) else {
        return Ok(json!({
            "ok": false,
            "status": "failed",
            "widget": name,
            "errors": c.errors,
            "warnings": c.warnings,
            "files": [prefix],
        }));
    };
    // Libraries: bundled ones become slots the runner fills.
    let m = stdlib::zen_lib_manifest();
    let mut libs = Vec::new();
    let mut slots = Vec::new();
    for (i, raw) in manifest.libs.iter().enumerate() {
        let r = LibRef::from_json(raw).map_err(ZenError::BadRequest)?;
        let entry = match &r {
            LibRef::Cdn { url, .. } => {
                return Err(ZenError::BadRequest(format!("--shot can't load CDN libraries yet ({url}); preview it in Zen instead")));
            }
            LibRef::Named { id, version } => stdlib::find(m, &r).ok_or_else(|| ZenError::BadRequest(format!("no library {id}@{version} in K2")))?,
        };
        if entry.source != LibSource::Bundled {
            return Err(ZenError::BadRequest(format!(
                "--shot can't load {} yet (a library K2 downloads on first use); preview it in Zen instead",
                entry.key()
            )));
        }
        slots.push(format!("__K2_SHOT_LIB_{i}__"));
        libs.push(json!({ "slot": format!("__K2_SHOT_LIB_{i}__"), "key": entry.key(), "files": entry.files }));
    }
    // A manifest's `"colorScheme": "own"` (0.46.3) opts out of the Garden's
    // scheme winning; read from the file so the shot follows Zen exactly.
    let own_scheme = files
        .get(widgets::MANIFEST_FILE)
        .and_then(|b| serde_json::from_slice::<J>(b).ok())
        .is_some_and(|m| m["colorScheme"] == "own");
    let nonce = new_nonce();
    let prelude = frame_prelude(&nonce, &vars, scheme, &slots, own_scheme);
    let frame = frame_src_doc(&bundle.render(&nonce), &nonce, &prelude);
    let hello = json!({
        "k2": "hello",
        "v": 1,
        "caps": manifest.caps,
        "features": FRAME_FEATURES,
        "widget": { "id": "shot", "name": manifest.name, "garden": "shot" },
        "config": o.config,
        // A still picture: no animation mid-shot.
        "motion": { "reduced": true },
    });
    let theme_info = json!({
        "theme": { "scheme": scheme.as_str(), "vars": vars },
        "chrome": { "corners": J::Null, "stoplights": J::Null },
        "motion": { "reduced": true },
    });
    let canvas = vars.get("--zen-canvas").cloned().unwrap_or_else(|| "transparent".into());
    let data = o.data.clone().unwrap_or_else(|| json!({}));
    let host = shot_host(&hello, &theme_info, &data, &canvas, scheme, o.width, o.height);
    Ok(json!({
        "ok": true,
        "status": "ok",
        "widget": name,
        "hash": bundle.hash,
        "nonce": nonce,
        "frame": frame,
        "host": host,
        "libs": libs,
        "width": o.width,
        "height": o.height,
        "scheme": scheme.as_str(),
        "theme": o.theme,
        "caps": manifest.caps,
        "colorScheme": if own_scheme { "own" } else { "garden" },
        "warnings": c.warnings,
    }))
}

#[cfg(test)]
#[path = "widget_shot_tests.rs"]
mod tests;
