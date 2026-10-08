//! The widget bundler (prd-zen-user-widgets-v2 UW8–UW10, UW13, UW46).
//!
//! A custom widget is one flat folder: an entry HTML file, local `*.js` and
//! `*.css`, and image and font assets. The daemon turns it into ONE
//! document the sealed frame loads from `srcdoc`:
//!
//! - `<script src="x.js">` → the file's text inline; `<link
//!   rel="stylesheet" href="x.css">` → `<style>`;
//! - `src` on `img`, `source`, `audio`, `video` (and `poster`, and an SVG
//!   `<image href>`) naming a folder file, and `url(x.png)` in CSS → a
//!   `data:` URL;
//! - every `<script>` gets `nonce="<N>"`; `N` is filled in per response
//!   ([`Bundle::render`]), so the widget CSP (`script-src 'nonce-N'`) runs
//!   only the scripts the daemon bundled, never markup injected later
//!   (an agent's text put into `innerHTML`);
//! - `window.K2_ASSETS` (name → `data:` URL) is set by a nonced script
//!   before the widget's first script, so `k2.asset("cat.png")` works.
//!
//! The bundle hash is the sha256 of the document with every nonce empty,
//! so it is stable across responses. HTML is read with html5ever's
//! tokenizer (the copy `feedback_brief` uses) and written back token by
//! token; comments are dropped.
//!
//! Errors stop a version (the last good one stays live); warnings never
//! block. Every finding is `widgets/<name>/<file>:line:col` (the Z13
//! shape).

use std::cell::RefCell;
use std::collections::BTreeMap;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, CharacterTokens, CommentToken, DoctypeToken, EndTag, NullCharacterToken, StartTag, Tag, TagToken,
    Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use sha2::{Digest, Sha256};

use super::schema::Diagnostic;

/// Code (HTML + JS + CSS that is inlined; assets excluded) per widget (UW8).
pub const MAX_CODE_BYTES: usize = 256 * 1024;
/// The bundled document, assets included (UW8).
pub const MAX_BUNDLE_BYTES: usize = 3 * 1024 * 1024;

/// Asset file types K2 inlines (UW8): extension → MIME. No SVG in this
/// cut, as for theme backgrounds.
pub const ASSET_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("woff2", "font/woff2"),
];

/// The MIME type of an asset file name, by extension (case-insensitive).
pub fn asset_mime(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    ASSET_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// What an asset's first bytes say it is.
pub fn sniff_asset(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"wOF2") {
        Some("font/woff2")
    } else {
        None
    }
}

/// A bundled document with its nonce slots. `parts.join(nonce)` is the
/// document a frame loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    parts: Vec<String>,
    /// sha256 (hex) of the document with every nonce empty.
    pub hash: String,
    /// HTML + JS + CSS bytes inlined (UW8's 256 KB limit).
    pub code_bytes: usize,
}

impl Bundle {
    fn new(parts: Vec<String>, code_bytes: usize) -> Self {
        let hash = sha256_hex(parts.concat().as_bytes());
        Self { parts, hash, code_bytes }
    }

    /// The document with `nonce` in every script's `nonce` attribute.
    pub fn render(&self, nonce: &str) -> String {
        self.parts.join(nonce)
    }

    /// The document with every nonce empty: what the hash covers and what
    /// `last-good.html` holds.
    pub fn template(&self) -> String {
        self.parts.concat()
    }

    /// Byte offsets in [`Bundle::template`] where a nonce goes.
    pub fn nonce_offsets(&self) -> Vec<usize> {
        let mut at = 0;
        let mut out = Vec::new();
        for p in &self.parts[..self.parts.len().saturating_sub(1)] {
            at += p.len();
            out.push(at);
        }
        out
    }

    /// The size of the template (the rendered document adds 24 bytes per
    /// script).
    pub fn bytes(&self) -> usize {
        self.parts.iter().map(String::len).sum()
    }

    /// Rebuild from a stored template and its nonce offsets. `None` when an
    /// offset is out of order, past the end, or not on a char boundary.
    pub fn from_template(html: &str, offsets: &[usize], code_bytes: usize) -> Option<Self> {
        let mut parts = Vec::with_capacity(offsets.len() + 1);
        let mut prev = 0;
        for &o in offsets {
            if o < prev || o > html.len() || !html.is_char_boundary(o) {
                return None;
            }
            parts.push(html[prev..o].to_string());
            prev = o;
        }
        parts.push(html[prev..].to_string());
        Some(Self::new(parts, code_bytes))
    }
}

/// 16 random bytes, base64: one per bundle response (UW9).
pub fn new_nonce() -> String {
    use base64::Engine as _;
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).unwrap_or_else(|e| panic!("OS random for a widget nonce: {e}"));
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The result of bundling one folder.
#[derive(Debug, Clone, Default)]
pub struct BundleOut {
    pub bundle: Option<Bundle>,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

/// What a `src` / `href` / `url()` value names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ref {
    Empty,
    Fragment,
    Data,
    /// A URL with a scheme or `//`.
    Remote(String),
    /// A path with `/`, `\` or `..`.
    Path,
    /// A file in the folder (leading `./`, `?…` and `#…` dropped).
    Local(String),
}

fn classify(v: &str) -> Ref {
    let v = v.trim();
    if v.is_empty() {
        return Ref::Empty;
    }
    if v.starts_with('#') {
        return Ref::Fragment;
    }
    if v.len() >= 5 && v[..5].eq_ignore_ascii_case("data:") {
        return Ref::Data;
    }
    if v.starts_with("//") {
        return Ref::Remote(v.to_string());
    }
    if let Some((scheme, _)) = v.split_once(':') {
        let ok_scheme = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
        if ok_scheme {
            return Ref::Remote(scheme.to_ascii_lowercase());
        }
    }
    let v = v.strip_prefix("./").unwrap_or(v);
    let v = v.split(['?', '#']).next().unwrap_or(v);
    if v.contains('/') || v.contains('\\') || v.contains("..") || v.starts_with('.') || v.is_empty() {
        return Ref::Path;
    }
    Ref::Local(v.to_string())
}

const NETWORK: &str = "widgets can't load from the network in this version; put the file in the widget's folder";
const FLAT: &str = "widgets are one flat folder: name the file itself (like \"cat.png\"), with no folders or ..";

fn remote_message(scheme: &str) -> String {
    if scheme == "javascript" {
        "javascript: URLs don't run in a widget; use addEventListener in a script file".to_string()
    } else {
        NETWORK.to_string()
    }
}

/// Elements a widget can't use (UW10).
const BANNED_ELEMENTS: &[(&str, &str)] = &[
    ("iframe", "<iframe> isn't allowed in a widget: a widget can't hold other pages"),
    ("frame", "<frame> isn't allowed in a widget: a widget can't hold other pages"),
    ("frameset", "<frameset> isn't allowed in a widget: a widget can't hold other pages"),
    ("object", "<object> isn't allowed in a widget"),
    ("embed", "<embed> isn't allowed in a widget"),
    ("applet", "<applet> isn't allowed in a widget"),
    ("portal", "<portal> isn't allowed in a widget"),
    ("base", "<base> isn't allowed in a widget: name files by their own names"),
];

/// Attributes that hold a URL: any network value is an error.
const URL_ATTRS: &[&str] = &[
    "src", "href", "poster", "action", "formaction", "xlink:href", "background", "data", "cite", "longdesc", "ping",
    "manifest", "codebase", "archive", "icon",
];

/// `(pattern, needs a word boundary before, message)`: UW10's warnings.
const JS_WARNINGS: &[(&str, &str)] = &[
    ("innerHTML", "if this shows agent or ticket text, use textContent"),
    ("outerHTML", "if this shows agent or ticket text, use textContent"),
    ("insertAdjacentHTML", "if this shows agent or ticket text, use textContent"),
    ("document.write", "if this shows agent or ticket text, use textContent"),
    ("fetch(", "blocked: widgets have no network in this version"),
    ("XMLHttpRequest", "blocked: widgets have no network in this version"),
    ("WebSocket", "blocked: widgets have no network in this version"),
    ("EventSource", "blocked: widgets have no network in this version"),
    ("localStorage", "not available in a sealed widget"),
    ("sessionStorage", "not available in a sealed widget"),
    ("indexedDB", "not available in a sealed widget"),
    ("eval(", "blocked by the widget's security policy"),
    ("new Function", "blocked by the widget's security policy"),
];

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// 1-based `(line, col)` of byte `off` in `text`.
fn line_col(text: &str, off: usize) -> (usize, usize) {
    let before = &text[..off.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[start..].chars().count() + 1)
}

/// UW10's JS warnings for `text`, at most one per pattern. `base` is where
/// the text starts in its file (an inline `<script>` in the HTML).
pub fn scan_js(file: &str, text: &str, base: (usize, usize)) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let at = |off: usize| {
        let (l, c) = line_col(text, off);
        if l == 1 { (base.0, base.1 + c - 1) } else { (base.0 + l - 1, c) }
    };
    for (pat, msg) in JS_WARNINGS {
        let mut from = 0;
        while let Some(i) = text[from..].find(pat).map(|i| i + from) {
            let before_ok = text[..i].chars().next_back().is_none_or(|c| !is_ident(c));
            let after_ok = pat.ends_with('(') || text[i + pat.len()..].chars().next().is_none_or(|c| !is_ident(c));
            if before_ok && after_ok {
                let (line, col) = at(i);
                let name = pat.trim_end_matches('(');
                out.push(Diagnostic { file: file.to_string(), line, col, message: format!("{name}: {msg}") });
                break;
            }
            from = i + pat.len();
        }
    }
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let is_import = (t.starts_with("import ") || t.starts_with("import{"))
            && (t.contains(" from ") || t.contains("}from") || t.trim_start_matches("import").trim_start().starts_with(['"', '\'']));
        if is_import {
            let (l, c) = at(off + (line.len() - t.len()));
            out.push(Diagnostic {
                file: file.to_string(),
                line: l,
                col: c,
                message: "import: one script per file; imports between files don't load (list each file in a <script src>)"
                    .to_string(),
            });
            break;
        }
        off += line.len();
    }
    out
}

/// Escape inlined JS so it can't end its `<script>` early or enter the
/// HTML parser's double-escaped state.
fn escape_script(js: &str) -> String {
    escape_close(js, "</script").replace("<!--", "<\\!--")
}

/// Escape inlined CSS so it can't end its `<style>` early.
fn escape_style(css: &str) -> String {
    escape_close(css, "</style")
}

/// Write every case-insensitive `needle` (ASCII, starting `</`) in `s` as
/// `<\/` plus the original letters. ASCII lowering keeps byte offsets.
fn escape_close(s: &str, needle: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len() + 8);
    let mut last = 0;
    for (i, _) in lower.match_indices(needle) {
        out.push_str(&s[last..i]);
        out.push_str("<\\/");
        out.push_str(&s[i + 2..i + needle.len()]);
        last = i + needle.len();
    }
    out.push_str(&s[last..]);
    out
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

/// Run html5ever's tokenizer over `html`, collecting `(token, line)`.
/// Switches to the raw-text states the tree builder would (a `<script>`
/// always, self-closing or not, as browsers do), so script and style
/// bodies never read as tags.
fn tokenize(html: &str) -> Vec<(Token, u64)> {
    struct Sink {
        out: RefCell<Vec<(Token, u64)>>,
    }
    impl TokenSink for Sink {
        type Handle = ();
        fn process_token(&self, token: Token, line: u64) -> TokenSinkResult<()> {
            let raw = match &token {
                TagToken(Tag { kind: StartTag, name, self_closing, .. }) => match &**name {
                    "script" => Some(TokenSinkResult::RawData(RawKind::ScriptData)),
                    "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" if !*self_closing => {
                        Some(TokenSinkResult::RawData(RawKind::Rawtext))
                    }
                    "title" | "textarea" if !*self_closing => Some(TokenSinkResult::RawData(RawKind::Rcdata)),
                    _ => None,
                },
                _ => None,
            };
            self.out.borrow_mut().push((token, line));
            raw.unwrap_or(TokenSinkResult::Continue)
        }
    }
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    let tok = Tokenizer::new(Sink { out: RefCell::new(Vec::new()) }, TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();
    tok.sink.out.take()
}

/// Where a needle sits: the first match on `line` (1-based) or the lines
/// before it, searching back (the tokenizer reports a tag at its `>`).
fn find_col(src: &str, line: u64, needle: &str) -> (usize, usize) {
    let lines: Vec<&str> = src.split('\n').collect();
    let needle = needle.to_ascii_lowercase();
    let mut l = (line as usize).clamp(1, lines.len().max(1));
    loop {
        if let Some(text) = lines.get(l - 1) {
            if let Some(i) = text.to_ascii_lowercase().find(&needle) {
                return (l, text[..i].chars().count() + 1);
            }
        }
        if l == 1 {
            return ((line as usize).max(1), 1);
        }
        l -= 1;
    }
}

/// Where an element's text starts: just after the `>` of its start tag
/// (`open` is `<script` or `<style`), found from the tag's line.
fn after_tag(src: &str, line: u64, open: &str) -> (usize, usize) {
    let (l, c) = find_col(src, line, open);
    let Some(text) = src.split('\n').nth(l - 1) else { return (l, 1) };
    let start: usize = text.char_indices().nth(c - 1).map_or(text.len(), |(i, _)| i);
    match text[start..].find('>') {
        Some(gt) => (l, text[..start + gt + 1].chars().count() + 1),
        None => (l + 1, 1),
    }
}

/// Builds the output document with nonce slots.
struct Out {
    parts: Vec<String>,
    cur: String,
}

impl Out {
    fn push(&mut self, s: &str) {
        self.cur.push_str(s);
    }
    fn nonce_attr(&mut self) {
        self.cur.push_str(" nonce=\"");
        self.parts.push(std::mem::take(&mut self.cur));
        self.cur.push('"');
    }
    fn finish(mut self) -> Vec<String> {
        self.parts.push(self.cur);
        self.parts
    }
}

/// Inside a raw-text element.
enum Raw {
    /// A `<script>`: `emit` is false when its `src` was inlined (or
    /// refused): the body is dropped, as a browser ignores it.
    Script { emit: bool, body: String, line: u64 },
    Style { body: String, line: u64 },
    /// `noscript`, `xmp`, …: passed through as written.
    Other { name: String },
}

struct Ctx<'a> {
    prefix: &'a str,
    entry: &'a str,
    src: &'a str,
    files: &'a BTreeMap<String, Vec<u8>>,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    data_urls: BTreeMap<String, String>,
    code_bytes: usize,
}

impl Ctx<'_> {
    fn label(&self, file: &str) -> String {
        format!("{}/{file}", self.prefix)
    }

    fn error_at(&mut self, file: &str, pos: (usize, usize), msg: impl Into<String>) {
        self.errors.push(Diagnostic { file: self.label(file), line: pos.0, col: pos.1, message: msg.into() });
    }

    fn html_error(&mut self, line: u64, needle: &str, msg: impl Into<String>) {
        let pos = find_col(self.src, line, needle);
        let entry = self.entry.to_string();
        self.error_at(&entry, pos, msg);
    }

    /// A folder file as a `data:` URL, or the message why not.
    fn data_url(&mut self, name: &str) -> Result<String, String> {
        if let Some(u) = self.data_urls.get(name) {
            return Ok(u.clone());
        }
        let Some(bytes) = self.files.get(name) else {
            return Err(format!("'{name}' isn't in this widget's folder"));
        };
        let Some(mime) = asset_mime(name) else {
            return Err(format!(
                "'{name}' isn't an image or font; K2 inlines .png, .jpg, .jpeg, .webp, .gif and .woff2"
            ));
        };
        use base64::Engine as _;
        let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
        self.data_urls.insert(name.to_string(), url.clone());
        Ok(url)
    }

    /// A text file in the folder (`.js` / `.css`), counted as code.
    fn text_file(&mut self, name: &str, ext: &str) -> Result<String, String> {
        if !name.to_ascii_lowercase().ends_with(ext) {
            return Err(format!("'{name}' must be a {ext} file"));
        }
        let Some(bytes) = self.files.get(name) else {
            return Err(format!("'{name}' isn't in this widget's folder"));
        };
        let text = std::str::from_utf8(bytes).map_err(|_| format!("'{name}' is not UTF-8 text"))?;
        self.code_bytes += bytes.len();
        Ok(text.trim_start_matches('\u{feff}').to_string())
    }

    /// Rewrite `url(…)` in CSS to `data:` URLs; `@import` is an error.
    /// Findings go to `file` at `base` (+ the offset inside `css`).
    fn rewrite_css(&mut self, file: &str, css: &str, base: (usize, usize)) -> String {
        let at = |off: usize| {
            let (l, c) = line_col(css, off);
            if l == 1 { (base.0, base.1 + c - 1) } else { (base.0 + l - 1, c) }
        };
        let lower = css.to_ascii_lowercase();
        if let Some(i) = lower.find("@import") {
            let pos = at(i);
            self.error_at(file, pos, "@import doesn't load in a widget; link each CSS file from the HTML");
        }
        let mut out = String::with_capacity(css.len());
        let mut last = 0;
        let mut from = 0;
        while let Some(i) = lower[from..].find("url(").map(|i| i + from) {
            let start = i + 4;
            let Some(close_rel) = css[start..].find(')') else { break };
            let close = start + close_rel;
            let raw = css[start..close].trim();
            let value = raw.trim_matches(|c| c == '"' || c == '\'');
            let pos = at(i);
            let replacement = match classify(value) {
                Ref::Empty | Ref::Fragment | Ref::Data => None,
                Ref::Remote(s) => {
                    self.error_at(file, pos, remote_message(&s));
                    None
                }
                Ref::Path => {
                    self.error_at(file, pos, FLAT);
                    None
                }
                Ref::Local(name) => match self.data_url(&name) {
                    Ok(u) => Some(u),
                    Err(m) => {
                        self.error_at(file, pos, m);
                        None
                    }
                },
            };
            if let Some(u) = replacement {
                out.push_str(&css[last..i]);
                out.push_str("url(\"");
                out.push_str(&u);
                out.push_str("\")");
                last = close + 1;
            }
            from = close + 1;
        }
        out.push_str(&css[last..]);
        out
    }
}

/// Bundle `entry` from `files` (every top-level file of the folder).
/// `prefix` labels findings (`widgets/<name>`). Asset size and type rules
/// are the folder check's (`widgets`); this checks what the HTML and CSS
/// reference.
pub fn bundle(prefix: &str, entry: &str, files: &BTreeMap<String, Vec<u8>>) -> BundleOut {
    let mut out = BundleOut::default();
    let Some(raw) = files.get(entry) else {
        out.errors.push(Diagnostic {
            file: format!("{prefix}/manifest.json"),
            line: 1,
            col: 1,
            message: format!("the entry file '{entry}' isn't in this widget's folder"),
        });
        return out;
    };
    let Ok(text) = std::str::from_utf8(raw) else {
        out.errors.push(Diagnostic {
            file: format!("{prefix}/{entry}"),
            line: 1,
            col: 1,
            message: format!("'{entry}' is not UTF-8 text"),
        });
        return out;
    };
    let src = text.trim_start_matches('\u{feff}');
    let mut cx = Ctx {
        prefix,
        entry,
        src,
        files,
        errors: Vec::new(),
        warnings: Vec::new(),
        data_urls: BTreeMap::new(),
        code_bytes: raw.len(),
    };
    let mut doc = Out { parts: Vec::new(), cur: String::new() };
    let mut raw_state: Option<Raw> = None;
    let mut assets_injected = false;
    let entry_owned = entry.to_string();

    for (token, line) in tokenize(src) {
        if let Some(state) = raw_state.as_mut() {
            let ends = matches!(&token, TagToken(Tag { kind: EndTag, name, .. })
                if match state { Raw::Script { .. } => &**name == "script", Raw::Style { .. } => &**name == "style", Raw::Other { name: n } => &**name == n.as_str() })
                || matches!(token, Token::EOFToken);
            if !ends {
                let text: String = match &token {
                    CharacterTokens(t) => t.to_string(),
                    NullCharacterToken => "\u{FFFD}".to_string(),
                    _ => String::new(),
                };
                match state {
                    Raw::Script { body, .. } | Raw::Style { body, .. } => body.push_str(&text),
                    Raw::Other { .. } => doc.push(&text),
                }
                continue;
            }
            match raw_state.take() {
                Some(Raw::Script { emit, body, line: l }) => {
                    if emit {
                        let base = after_tag(src, l, "<script");
                        cx.warnings.extend(scan_js(&cx.label(&entry_owned), &body, base));
                        doc.push(&body);
                    }
                    doc.push("</script>");
                }
                Some(Raw::Style { body, line: l }) => {
                    let base = after_tag(src, l, "<style");
                    let css = cx.rewrite_css(&entry_owned, &body, base);
                    doc.push(&escape_style(&css));
                    doc.push("</style>");
                }
                Some(Raw::Other { name }) => doc.push(&format!("</{name}>")),
                None => {}
            }
            continue;
        }
        match token {
            DoctypeToken(d) => {
                let name = d.name.as_ref().map(|n| n.to_string()).unwrap_or_else(|| "html".into());
                doc.push(&format!("<!DOCTYPE {name}>"));
            }
            CommentToken(_) => {}
            CharacterTokens(t) => doc.push(&escape_text(&t)),
            NullCharacterToken => doc.push("\u{FFFD}"),
            TagToken(tag) if tag.kind == EndTag => {
                let name = &*tag.name;
                if BANNED_ELEMENTS.iter().any(|(n, _)| *n == name) || name == "link" {
                    continue;
                }
                doc.push(&format!("</{name}>"));
            }
            TagToken(tag) => {
                let name = tag.name.to_string();
                if let Some((_, msg)) = BANNED_ELEMENTS.iter().find(|(n, _)| *n == name) {
                    cx.html_error(line, &format!("<{name}"), *msg);
                    if !tag.self_closing && matches!(name.as_str(), "iframe") {
                        raw_state = Some(Raw::Other { name });
                    }
                    continue;
                }
                let attr = |k: &str| tag.attrs.iter().find(|a| &*a.name.local == k).map(|a| a.value.to_string());
                if name == "meta" && attr("http-equiv").is_some() {
                    cx.html_error(line, "http-equiv", "<meta http-equiv> isn't allowed in a widget");
                    continue;
                }
                // Inline event handlers never run under the nonce CSP.
                for a in &tag.attrs {
                    let an = a.name.local.to_string();
                    if an.len() > 2 && an.starts_with("on") {
                        cx.html_error(
                            line,
                            &format!("{an}="),
                            format!("{an}= won't run; use addEventListener in a script file"),
                        );
                    }
                }
                if name == "script" {
                    if !assets_injected {
                        assets_injected = true;
                        doc.push("<script");
                        doc.nonce_attr();
                        doc.push(">window.K2_ASSETS=Object.freeze(");
                        doc.push(&assets_json(&mut cx));
                        doc.push(");</script>");
                    }
                    let mut emit_body = true;
                    let mut opened = false;
                    if let Some(s) = attr("src") {
                        emit_body = false;
                        match classify(&s) {
                            Ref::Local(file) => match cx.text_file(&file, ".js") {
                                Ok(js) => {
                                    cx.warnings.extend(scan_js(&cx.label(&file), &js, (1, 1)));
                                    copied_library_warning(&mut cx, &file);
                                    write_start(&mut doc, "script", &tag, &["src", "nonce", "integrity", "crossorigin", "async", "defer"]);
                                    doc.nonce_attr();
                                    doc.push(">");
                                    doc.push(&escape_script(&js));
                                    opened = true;
                                }
                                Err(m) => cx.html_error(line, "src", m),
                            },
                            Ref::Remote(sch) => cx.html_error(line, "src", remote_message(&sch)),
                            Ref::Path => cx.html_error(line, "src", FLAT),
                            Ref::Empty | Ref::Fragment | Ref::Data => {
                                cx.html_error(line, "src", "a script's src names a .js file in the widget's folder")
                            }
                        }
                    } else {
                        write_start(&mut doc, "script", &tag, &["nonce", "integrity", "crossorigin"]);
                        doc.nonce_attr();
                        doc.push(">");
                        opened = true;
                    }
                    if opened {
                        raw_state = Some(Raw::Script { emit: emit_body, body: String::new(), line });
                    } else {
                        // Refused: swallow its body and end tag.
                        doc.push("<script");
                        doc.nonce_attr();
                        doc.push(">");
                        raw_state = Some(Raw::Script { emit: false, body: String::new(), line });
                    }
                    continue;
                }
                if name == "link" {
                    let rel = attr("rel").unwrap_or_default().to_ascii_lowercase();
                    let href = attr("href").unwrap_or_default();
                    let is_sheet = rel.split_ascii_whitespace().any(|r| r == "stylesheet");
                    match classify(&href) {
                        Ref::Remote(sch) => cx.html_error(line, "href", remote_message(&sch)),
                        Ref::Path => cx.html_error(line, "href", FLAT),
                        Ref::Local(file) if is_sheet => match cx.text_file(&file, ".css") {
                            Ok(css) => {
                                let css = cx.rewrite_css(&file, &css, (1, 1));
                                doc.push("<style>");
                                doc.push(&escape_style(&css));
                                doc.push("</style>");
                            }
                            Err(m) => cx.html_error(line, "href", m),
                        },
                        _ => {
                            let pos = find_col(src, line, "<link");
                            cx.warnings.push(Diagnostic {
                                file: cx.label(&entry_owned),
                                line: pos.0,
                                col: pos.1,
                                message: "K2 ignores this <link>; only <link rel=\"stylesheet\" href=\"file.css\"> loads in a widget"
                                    .to_string(),
                            });
                        }
                    }
                    continue;
                }
                // Every other element: check URL attributes, inline assets.
                let mut rewritten: Vec<(String, String)> = Vec::with_capacity(tag.attrs.len());
                for a in &tag.attrs {
                    let an = a.name.local.to_string();
                    let av = a.value.to_string();
                    if an.len() > 2 && an.starts_with("on") {
                        continue;
                    }
                    if an == "srcset" || an == "imagesrcset" {
                        cx.html_error(line, &an, format!("{an} isn't supported in a widget; use src"));
                        continue;
                    }
                    if an == "style" {
                        let css = cx.rewrite_css(&entry_owned, &av, find_col(src, line, "style="));
                        rewritten.push((an, css));
                        continue;
                    }
                    if !URL_ATTRS.contains(&an.as_str()) {
                        rewritten.push((an, av));
                        continue;
                    }
                    let inlines = matches!(
                        (name.as_str(), an.as_str()),
                        ("img" | "source" | "audio" | "video" | "track" | "input" | "image", "src")
                            | ("video", "poster")
                            | ("image", "href" | "xlink:href")
                    );
                    match classify(&av) {
                        Ref::Empty | Ref::Fragment | Ref::Data => rewritten.push((an, av)),
                        Ref::Remote(sch) => cx.html_error(line, &format!("{an}="), remote_message(&sch)),
                        Ref::Path => cx.html_error(line, &format!("{an}="), FLAT),
                        Ref::Local(file) if inlines => match cx.data_url(&file) {
                            Ok(u) => rewritten.push((an, u)),
                            Err(m) => cx.html_error(line, &format!("{an}="), m),
                        },
                        Ref::Local(_) if matches!(name.as_str(), "a" | "area") => cx.html_error(
                            line,
                            &format!("{an}="),
                            "a widget is one page: links can only jump within it (href=\"#id\")",
                        ),
                        Ref::Local(file) => cx.html_error(
                            line,
                            &format!("{an}="),
                            format!("K2 inlines '{file}' only from src on img, source, audio and video"),
                        ),
                    }
                }
                doc.push(&format!("<{name}"));
                for (k, v) in &rewritten {
                    doc.push(&format!(" {k}=\"{}\"", escape_attr(v)));
                }
                doc.push(if tag.self_closing { " />" } else { ">" });
                if !tag.self_closing && name == "style" {
                    raw_state = Some(Raw::Style { body: String::new(), line });
                } else if !tag.self_closing && matches!(name.as_str(), "noscript" | "xmp" | "noembed" | "noframes") {
                    raw_state = Some(Raw::Other { name });
                }
            }
            Token::EOFToken | Token::ParseError(_) => {}
        }
    }
    // An unterminated script or style: close it like a browser at EOF.
    match raw_state.take() {
        Some(Raw::Script { emit, body, .. }) => {
            if emit {
                doc.push(&body);
            }
            doc.push("</script>");
        }
        Some(Raw::Style { body, line }) => {
            let css = cx.rewrite_css(&entry_owned, &body, (line as usize, 1));
            doc.push(&escape_style(&css));
            doc.push("</style>");
        }
        Some(Raw::Other { name }) => doc.push(&format!("</{name}>")),
        None => {}
    }
    if cx.code_bytes > MAX_CODE_BYTES {
        cx.errors.push(Diagnostic {
            file: cx.label(&entry_owned),
            line: 1,
            col: 1,
            message: format!(
                "this widget's code (HTML, JS and CSS) is {} bytes; the limit is {MAX_CODE_BYTES} bytes (256 KB)",
                cx.code_bytes
            ),
        });
    }
    let bundle = Bundle::new(doc.finish(), cx.code_bytes);
    if bundle.bytes() > MAX_BUNDLE_BYTES {
        cx.errors.push(Diagnostic {
            file: cx.label(&entry_owned),
            line: 1,
            col: 1,
            message: format!(
                "the bundled widget is {} bytes; the limit is {MAX_BUNDLE_BYTES} bytes (3 MB). Use smaller images",
                bundle.bytes()
            ),
        });
    }
    out.errors = cx.errors;
    out.warnings = cx.warnings;
    if out.errors.is_empty() {
        out.bundle = Some(bundle);
    }
    out
}

/// `window.K2_ASSETS`: every asset in the folder, name → `data:` URL, as a
/// JSON object safe inside `<script>`.
fn assets_json(cx: &mut Ctx<'_>) -> String {
    let names: Vec<String> = cx.files.keys().filter(|n| asset_mime(n).is_some()).cloned().collect();
    let mut map = serde_json::Map::new();
    for n in names {
        if let Ok(u) = cx.data_url(&n) {
            map.insert(n, serde_json::Value::String(u));
        }
    }
    serde_json::Value::Object(map)
        .to_string()
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Write `<name attrs…` (no closing `>`), skipping `skip` and handlers.
fn write_start(doc: &mut Out, name: &str, tag: &Tag, skip: &[&str]) {
    doc.push(&format!("<{name}"));
    for a in &tag.attrs {
        let an = &*a.name.local;
        if skip.contains(&an) || (an.len() > 2 && an.starts_with("on")) {
            continue;
        }
        doc.push(&format!(" {an}=\"{}\"", escape_attr(&a.value)));
    }
}

/// Library ids a widget folder shouldn't carry a copy of (UWB17).
const COMMON_LIBRARIES: &[&str] = &[
    "three", "d3", "pixi", "phaser", "kaplay", "matter", "chart", "echarts", "anime", "lottie", "perfect-freehand",
    "rough", "howler", "tone", "leaflet", "marked", "purify", "dompurify", "highlight", "katex", "preact", "lit",
    "babylon", "plotly", "globe", "cannon", "mermaid",
];

/// A big JS file named like a standard library: suggest `requires.libs`.
fn copied_library_warning(cx: &mut Ctx<'_>, file: &str) {
    let size = cx.files.get(file).map_or(0, Vec::len);
    if size < 30 * 1024 {
        return;
    }
    let stem = file.to_ascii_lowercase();
    let stem = stem.trim_end_matches(".js");
    let lib_ids: Vec<String> = super::stdlib::zen_lib_manifest().libs.iter().map(|l| l.id.to_ascii_lowercase()).collect();
    let hit = COMMON_LIBRARIES
        .iter()
        .map(|s| s.to_string())
        .chain(lib_ids)
        .find(|id| stem == id || stem.strip_prefix(id.as_str()).is_some_and(|r| r.starts_with(['.', '-', '_'])));
    if let Some(id) = hit {
        let label = cx.label(file);
        cx.warnings.push(Diagnostic {
            file: label,
            line: 1,
            col: 1,
            message: format!(
                "this looks like a copy of {id}; K2 ships a widget library: list it in manifest.json requires.libs (see k2 zen guide libs) instead of copying {file}"
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        list.iter().map(|(n, b)| (n.to_string(), b.to_vec())).collect()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";

    #[test]
    fn inlines_scripts_styles_and_assets_with_nonce_slots() {
        let f = files(&[
            (
                "index.html",
                b"<!doctype html>\n<html><head><link rel=\"stylesheet\" href=\"a.css\"></head>\n<body><img src=\"cat.png\" alt=\"a & b\">\n<p>1 &lt; 2</p><!-- gone -->\n<script src=\"./a.js\"></script>\n<script>k2.ready()</script></body></html>\n",
            ),
            ("a.js", b"console.log('</script>')"),
            ("a.css", b"body { background: url(cat.png) }"),
            ("cat.png", PNG),
        ]);
        let out = bundle("widgets/w", "index.html", &f);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let b = out.bundle.expect("bundle");
        let html = b.render("NONCE");
        assert_eq!(html.matches("nonce=\"NONCE\"").count(), 3, "assets script + 2 scripts: {html}");
        assert!(!html.contains("src=\"./a.js\"") && !html.contains("src=\"cat.png\""), "{html}");
        assert!(html.contains("window.K2_ASSETS=Object.freeze({\"cat.png\":\"data:image/png;base64,"));
        assert!(html.contains("<img src=\"data:image/png;base64,"));
        assert!(html.contains("url(\"data:image/png;base64,"));
        assert!(html.contains("console.log('<\\/script>')"), "{html}");
        assert!(html.contains("alt=\"a &amp; b\""));
        assert!(html.contains("<p>1 &lt; 2</p>"));
        assert!(!html.contains("gone"), "comments are dropped");
        assert_eq!(b.render("x").len(), b.template().len() + 3, "three one-byte nonces");
        assert_eq!(b.hash, sha256_hex(b.template().as_bytes()));
        assert_eq!(b.render("A") != b.render("B"), true);
        let again = Bundle::from_template(&b.template(), &b.nonce_offsets(), b.code_bytes).expect("rebuild");
        assert_eq!(again, b);
    }

    #[test]
    fn escape_close_keeps_case_and_text() {
        assert_eq!(escape_script("a</SCRIPT>b<!--c"), "a<\\/SCRIPT>b<\\!--c");
        assert_eq!(escape_style("x</style y"), "x<\\/style y");
        assert_eq!(escape_script("plain"), "plain");
    }

    fn one_error(html: &str) -> Diagnostic {
        let f = files(&[("index.html", html.as_bytes()), ("ok.js", b"1"), ("cat.png", PNG)]);
        let out = bundle("widgets/w", "index.html", &f);
        assert_eq!(out.errors.len(), 1, "{html}: {:?}", out.errors);
        assert!(out.bundle.is_none());
        out.errors[0].clone()
    }

    #[test]
    fn each_bad_line_is_one_error_at_its_place() {
        for (html, line, col, has) in [
            ("<p>\n<script src=\"https://cdn.example.test/x.js\"></script>", 2, 9, "network"),
            ("<p>\n<link rel=\"stylesheet\" href=\"//cdn.example.test/x.css\">", 2, 24, "network"),
            ("<p>\n  <script src=\"../x.js\"></script>", 2, 11, "flat folder"),
            ("<script src=\"missing.js\"></script>", 1, 9, "isn't in this widget's folder"),
            ("<p>\n<button onclick=\"go()\">x</button>", 2, 9, "addEventListener"),
            ("<iframe src=\"x.html\"></iframe>", 1, 1, "<iframe>"),
            ("<p></p>\n<base href=\"x\">", 2, 1, "<base>"),
            ("<meta http-equiv=\"refresh\" content=\"0\">", 1, 7, "http-equiv"),
            ("<img srcset=\"cat.png 2x\">", 1, 6, "srcset"),
            ("<a href=\"javascript:alert(1)\">x</a>", 1, 4, "javascript:"),
            ("<style>@import \"x.css\";</style>", 1, 8, "@import"),
        ] {
            let d = one_error(html);
            assert_eq!((d.line, d.col), (line, col), "{html}: {d:?}");
            assert!(d.message.contains(has), "{html}: {}", d.message);
            assert_eq!(d.file, "widgets/w/index.html");
        }
    }

    #[test]
    fn js_warnings_name_their_place_and_never_block() {
        let f = files(&[
            ("index.html", b"<script src=\"a.js\"></script>"),
            (
                "a.js",
                b"import x from './b.js'\nel.innerHTML = t\nfetch('/x')\nlocalStorage.x = 1\nconst y = eval('1')\nmyfetch(1)\n",
            ),
        ]);
        let out = bundle("widgets/w", "index.html", &f);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(out.bundle.is_some());
        let got: Vec<(usize, usize, String)> =
            out.warnings.iter().map(|d| (d.line, d.col, d.message.split(':').next().unwrap_or("").to_string())).collect();
        for want in [(2, 4, "innerHTML"), (3, 1, "fetch"), (4, 1, "localStorage"), (5, 11, "eval"), (1, 1, "import")] {
            assert!(got.contains(&(want.0, want.1, want.2.to_string())), "missing {want:?} in {got:?}");
        }
        assert_eq!(got.len(), 5, "myfetch( is not fetch(: {got:?}");
        assert!(out.warnings.iter().all(|d| d.file == "widgets/w/a.js"));
    }

    #[test]
    fn code_over_256_kb_is_an_error() {
        let big = vec![b'x'; MAX_CODE_BYTES + 1];
        let f = files(&[("index.html", b"<script src=\"a.js\"></script>"), ("a.js", &big)]);
        let out = bundle("widgets/w", "index.html", &f);
        assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
        assert!(out.errors[0].message.contains("256 KB"));
    }

    #[test]
    fn classify_values() {
        assert_eq!(classify("cat.png"), Ref::Local("cat.png".into()));
        assert_eq!(classify("./cat.png?v=2"), Ref::Local("cat.png".into()));
        assert_eq!(classify("#top"), Ref::Fragment);
        assert_eq!(classify("DATA:image/png;base64,AA"), Ref::Data);
        assert_eq!(classify("https://example.test/x"), Ref::Remote("https".into()));
        assert_eq!(classify("//example.test/x"), Ref::Remote("//example.test/x".into()));
        assert_eq!(classify("a/b.png"), Ref::Path);
        assert_eq!(classify("..x.png"), Ref::Path);
    }
}
