//! Design mode page notes (prd-browser-design-mode-v1 §8, D-A9–D-A13).
//!
//! A person picks elements on a page in a Design tab, writes a note for
//! each, and sends the batch to the workspace's agent. The page is
//! untrusted: everything it reported (selector, HTML, text, styles) is
//! clamped here, at the trust boundary, before it is formatted into one
//! Thread post. The desktop picker clamps the same way (it calls
//! [`clamp_element`]); the daemon clamps again on `POST /cli/browser/notes`
//! because a client is never trusted to have done it.
//!
//! Shared by `k2-daemon` (route + `k2 browser note`) and `src-tauri`
//! (the in-page picker's return channel), so the budgets live in one place.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Most notes one Send carries (PRD C2: cap 20 per tab).
pub const MAX_NOTES: usize = 20;
/// The person's own words per note (vs-live D-A13).
pub const MAX_COMMENT_CHARS: usize = 2000;
pub const MAX_URL_CHARS: usize = 2048;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_SELECTOR_CHARS: usize = 512;
pub const MAX_TEXT_CHARS: usize = 200;
pub const MAX_HTML_CHARS: usize = 4096;
pub const MAX_NEARBY: usize = 5;
pub const MAX_NEARBY_CHARS: usize = 120;
pub const MAX_CLASSES: usize = 8;
pub const MAX_CLASS_CHARS: usize = 64;
pub const MAX_TAG_CHARS: usize = 32;
pub const MAX_ROLE_CHARS: usize = 64;
pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_SOURCE_CHARS: usize = 256;
pub const MAX_STYLE_VALUE_CHARS: usize = 120;
pub const MAX_ITEM_ID_CHARS: usize = 128;
/// Past this the formatted post drops HTML snippets (last note first) so a
/// full tray still fits one Thread post and one PTY inject.
pub const MAX_FORMATTED_CHARS: usize = 32_000;

/// Computed styles the picker may report (PRD §8: allowlist only).
pub const STYLE_ALLOWLIST: &[&str] = &[
    "display",
    "position",
    "width",
    "height",
    "margin",
    "padding",
    "color",
    "background-color",
    "font-family",
    "font-size",
    "font-weight",
    "line-height",
    "text-align",
    "z-index",
    "opacity",
    "border",
    "border-radius",
    "box-shadow",
    "flex-direction",
    "justify-content",
    "align-items",
    "gap",
    "grid-template-columns",
    "grid-template-rows",
];

/// Attribute names whose values never leave the page.
const SECRET_ATTR_WORDS: &[&str] = &[
    "password", "passwd", "token", "api_key", "apikey", "api-key", "authorization", "auth", "cookie",
    "secret", "session", "csrf", "xsrf", "credential", "private", "signature", "nonce",
];

/// Elements whose whole subtree is dropped from an HTML snippet.
const DROPPED_ELEMENTS: &[&str] = &["script", "iframe", "object", "embed", "noscript", "template", "style"];

pub const REDACTED: &str = "[redacted]";

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteViewport {
    pub width: f64,
    pub height: f64,
}

/// What the picker captured about one element. Every field is untrusted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NoteElement {
    pub selector: String,
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Accessible name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub text: String,
    pub html: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub styles: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<NoteRect>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nearby_text: Vec<String>,
    /// `file:line` from a dev build's React source info, when cheap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteIntent {
    #[default]
    Change,
    Question,
}

impl NoteIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            NoteIntent::Change => "change",
            NoteIntent::Question => "question",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "change" => Ok(NoteIntent::Change),
            "question" => Ok(NoteIntent::Question),
            other => Err(format!("intent must be 'change' or 'question', got {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesignNote {
    /// The picked element. Absent for a page-level note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<NoteElement>,
    pub comment: String,
    pub intent: NoteIntent,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotesPage {
    pub url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewport: Option<NoteViewport>,
}

/// Body of `POST /cli/browser/notes`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotesRequest {
    /// Workspace handle, name, path or id.
    pub workspace: String,
    pub page: NotesPage,
    pub notes: Vec<DesignNote>,
    /// The Design tab the notes came from (stored on the Thread item's
    /// `design` origin; never trusted for anything but display).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

/// Characters, not bytes; control characters (but newline and tab) dropped.
pub fn clamp_text(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// One line: whitespace runs collapse to one space, then clamp.
pub fn clamp_line(s: &str, max: usize) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    clamp_text(&collapsed, max)
}

fn clamp_opt_line(s: &Option<String>, max: usize) -> Option<String> {
    s.as_deref().map(|v| clamp_line(v, max)).filter(|v| !v.is_empty())
}

/// http(s) only; userinfo, query and hash stripped (PRD §8).
pub fn clean_page_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("page url is required".into());
    }
    let mut u = url::Url::parse(trimmed).map_err(|e| format!("page url is not a URL: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(format!("page url must be http or https, got {}", u.scheme()));
    }
    u.set_query(None);
    u.set_fragment(None);
    let _ = u.set_username("");
    let _ = u.set_password(None);
    let s = u.to_string();
    if s.chars().count() > MAX_URL_CHARS {
        return Err(format!("page url is longer than {MAX_URL_CHARS} characters"));
    }
    Ok(s)
}

fn is_secret_attr(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_ATTR_WORDS.iter().any(|w| n.contains(w))
}

/// Strip `script`/`iframe`/`object`/… subtrees, comments and `on*`
/// attributes; redact secret-looking attribute values (and the `value` of
/// password and hidden inputs); then clamp. Not a full HTML parser: it
/// only has to make a snippet the page produced safe to show and quote.
pub fn sanitize_html(raw: &str, max: usize) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Comment.
        if starts_with_at(&chars, i, "<!--") {
            match find_at(&chars, i + 4, "-->") {
                Some(end) => i = end + 3,
                None => break,
            }
            continue;
        }
        let Some(tag_end) = tag_close(&chars, i + 1) else {
            // A lone `<` with no closing `>`: drop the rest.
            break;
        };
        let inner: String = chars[i + 1..tag_end].iter().collect();
        let closing = inner.trim_start().starts_with('/');
        let name: String = inner
            .trim_start()
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            // `<!doctype>`, `<?xml>`, stray `< `: drop it.
            i = tag_end + 1;
            continue;
        }
        if DROPPED_ELEMENTS.contains(&name.as_str()) {
            if closing {
                i = tag_end + 1;
                continue;
            }
            let self_closing = inner.trim_end().ends_with('/');
            if self_closing {
                i = tag_end + 1;
                continue;
            }
            let close = format!("</{name}");
            match find_ci(&chars, tag_end + 1, &close) {
                Some(at) => match tag_close(&chars, at + 1) {
                    Some(e) => i = e + 1,
                    None => break,
                },
                None => break,
            }
            continue;
        }
        if closing {
            out.push_str(&format!("</{name}>"));
        } else {
            out.push_str(&rebuild_open_tag(&name, &inner));
        }
        i = tag_end + 1;
    }
    clamp_text(&out, max)
}

fn starts_with_at(chars: &[char], at: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    at + p.len() <= chars.len() && chars[at..at + p.len()] == p[..]
}

fn find_at(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    (from..chars.len()).find(|&k| starts_with_at(chars, k, pat))
}

fn find_ci(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().map(|c| c.to_ascii_lowercase()).collect();
    (from..chars.len()).find(|&k| {
        k + p.len() <= chars.len() && chars[k..k + p.len()].iter().map(|c| c.to_ascii_lowercase()).eq(p.iter().copied())
    })
}

/// Index of the `>` that ends the tag opened just before `from`, skipping
/// quoted attribute values.
fn tag_close(chars: &[char], from: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (k, &c) in chars.iter().enumerate().skip(from) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '>' => return Some(k),
            None => {}
        }
    }
    None
}

fn rebuild_open_tag(name: &str, inner: &str) -> String {
    let attrs = parse_attrs(inner.trim_start().get(name.len()..).unwrap_or(""));
    let input_type = attrs
        .iter()
        .find(|(k, _)| k == "type")
        .and_then(|(_, v)| v.clone())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let hide_value = name == "input" && matches!(input_type.as_str(), "password" | "hidden");
    let mut out = format!("<{name}");
    for (k, v) in attrs {
        if k.starts_with("on") || k.is_empty() {
            continue;
        }
        let secret = is_secret_attr(&k) || (hide_value && k == "value");
        let lower_v = v.as_deref().map(str::to_ascii_lowercase).unwrap_or_default();
        let scripty = matches!(k.as_str(), "href" | "src" | "action" | "formaction" | "xlink:href")
            && lower_v.trim_start().starts_with("javascript:");
        out.push(' ');
        out.push_str(&k);
        match v {
            Some(_) if secret || scripty => out.push_str(&format!("=\"{REDACTED}\"")),
            Some(val) => out.push_str(&format!("=\"{}\"", val.replace('"', "&quot;"))),
            None => {}
        }
    }
    if inner.trim_end().ends_with('/') {
        out.push_str(" /");
    }
    out.push('>');
    out
}

fn parse_attrs(s: &str) -> Vec<(String, Option<String>)> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == '/') {
            i += 1;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' && chars[i] != '/' {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
        if name.is_empty() {
            if i < chars.len() {
                i += 1;
            }
            continue;
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i < chars.len() && chars[i] == '=' {
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            let value = if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
                let q = chars[i];
                i += 1;
                let vs = i;
                while i < chars.len() && chars[i] != q {
                    i += 1;
                }
                let v: String = chars[vs..i].iter().collect();
                i += 1;
                v
            } else {
                let vs = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                chars[vs..i].iter().collect()
            };
            out.push((name, Some(value)));
        } else {
            out.push((name, None));
        }
    }
    out
}

/// Clamp and sanitize everything a picker reported about one element.
pub fn clamp_element(e: &NoteElement) -> NoteElement {
    let styles = e
        .styles
        .iter()
        .filter(|(k, _)| STYLE_ALLOWLIST.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), clamp_line(v, MAX_STYLE_VALUE_CHARS)))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let rect = e.rect.filter(|r| [r.x, r.y, r.width, r.height].iter().all(|n| n.is_finite()));
    NoteElement {
        selector: clamp_line(&e.selector, MAX_SELECTOR_CHARS),
        tag: clamp_line(&e.tag, MAX_TAG_CHARS).to_ascii_lowercase(),
        id: clamp_opt_line(&e.id, MAX_CLASS_CHARS),
        classes: e
            .classes
            .iter()
            .map(|c| clamp_line(c, MAX_CLASS_CHARS))
            .filter(|c| !c.is_empty())
            .take(MAX_CLASSES)
            .collect(),
        role: clamp_opt_line(&e.role, MAX_ROLE_CHARS),
        name: clamp_opt_line(&e.name, MAX_NAME_CHARS),
        text: clamp_line(&e.text, MAX_TEXT_CHARS),
        html: sanitize_html(&e.html, MAX_HTML_CHARS),
        styles,
        rect,
        nearby_text: e
            .nearby_text
            .iter()
            .map(|t| clamp_line(t, MAX_NEARBY_CHARS))
            .filter(|t| !t.is_empty())
            .take(MAX_NEARBY)
            .collect(),
        source: clamp_opt_line(&e.source, MAX_SOURCE_CHARS),
    }
}

/// Validate and clamp a whole request. Errors are the person's mistakes
/// (no notes, too many, an empty comment, a bad URL), never page content:
/// page content is clamped, not refused.
pub fn clamp_request(req: &NotesRequest) -> Result<NotesRequest, String> {
    if req.notes.is_empty() {
        return Err("no notes to send".into());
    }
    if req.notes.len() > MAX_NOTES {
        return Err(format!("at most {MAX_NOTES} notes per send, got {}", req.notes.len()));
    }
    let url = clean_page_url(&req.page.url)?;
    let mut notes = Vec::with_capacity(req.notes.len());
    for (i, n) in req.notes.iter().enumerate() {
        let comment = clamp_text(&n.comment, MAX_COMMENT_CHARS);
        if comment.is_empty() {
            return Err(format!("note {} has no text", i + 1));
        }
        notes.push(DesignNote { payload: n.payload.as_ref().map(clamp_element), comment, intent: n.intent });
    }
    let viewport = req.page.viewport.filter(|v| v.width.is_finite() && v.height.is_finite() && v.width >= 0.0 && v.height >= 0.0);
    Ok(NotesRequest {
        workspace: req.workspace.trim().to_string(),
        page: NotesPage { url, title: clamp_line(&req.page.title, MAX_TITLE_CHARS), viewport },
        notes,
        item_id: clamp_opt_line(&req.item_id, MAX_ITEM_ID_CHARS),
    })
}

fn inline_code(s: &str) -> String {
    format!("`{}`", s.replace('`', "'"))
}

fn element_label(e: &NoteElement) -> String {
    let mut s = format!("<{}", if e.tag.is_empty() { "element" } else { &e.tag });
    if let Some(id) = &e.id {
        s.push('#');
        s.push_str(id);
    }
    for c in e.classes.iter().take(3) {
        s.push('.');
        s.push_str(c);
    }
    s.push('>');
    s
}

fn px(n: f64) -> String {
    format!("{}", n.round() as i64)
}

fn format_note(out: &mut String, n: usize, note: &DesignNote, with_html: bool) {
    let intent = match note.intent {
        NoteIntent::Change => "Change",
        NoteIntent::Question => "Question",
    };
    match &note.payload {
        Some(e) => out.push_str(&format!("### {n}. {intent}: {}\n", inline_code(&element_label(e)))),
        None => out.push_str(&format!("### {n}. {intent} (whole page)\n")),
    }
    out.push_str(&note.comment);
    out.push('\n');
    let Some(e) = &note.payload else {
        return;
    };
    out.push('\n');
    if !e.selector.is_empty() {
        out.push_str(&format!("- selector: {}\n", inline_code(&e.selector)));
    }
    let mut a11y = Vec::new();
    if let Some(r) = &e.role {
        a11y.push(format!("role={r}"));
    }
    if let Some(name) = &e.name {
        a11y.push(format!("name=\"{name}\""));
    }
    if !a11y.is_empty() {
        out.push_str(&format!("- a11y: {}\n", a11y.join(" ")));
    }
    if let Some(r) = &e.rect {
        out.push_str(&format!("- box: {}×{} at ({}, {})\n", px(r.width), px(r.height), px(r.x), px(r.y)));
    }
    if !e.text.is_empty() {
        out.push_str(&format!("- text: {}\n", e.text));
    }
    if let Some(src) = &e.source {
        out.push_str(&format!("- source: {}\n", inline_code(src)));
    }
    if !e.styles.is_empty() {
        let s = e.styles.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("; ");
        out.push_str(&format!("- styles: {s}\n"));
    }
    if !e.nearby_text.is_empty() {
        out.push_str(&format!("- nearby: {}\n", e.nearby_text.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ")));
    }
    if !e.html.is_empty() {
        if with_html {
            out.push_str("\n```html\n");
            out.push_str(&e.html.replace("```", "'''"));
            out.push_str("\n```\n");
        } else {
            out.push_str("- html: (left out: the notes were too long)\n");
        }
    }
}

/// The one Thread post a Send makes. Stable shape (agents read it):
///
/// ```text
/// Design notes (2) on http://localhost:5173/pricing
/// Page: Pricing · 1280×800
///
/// ### 1. Change: `<button.cta>`
/// make this the primary colour
///
/// - selector: `main > button.cta`
/// - a11y: role=button name="Buy"
/// …
/// ```
///
/// Input must already be clamped ([`clamp_request`]).
pub fn format_notes(req: &NotesRequest) -> String {
    let render = |html_upto: usize| {
        let mut out = String::new();
        let count = req.notes.len();
        out.push_str(&format!(
            "Design notes ({count}) on {}\n",
            req.page.url
        ));
        let mut page_line = Vec::new();
        if !req.page.title.is_empty() {
            page_line.push(req.page.title.clone());
        }
        if let Some(v) = &req.page.viewport {
            page_line.push(format!("{}×{}", px(v.width), px(v.height)));
        }
        if !page_line.is_empty() {
            out.push_str(&format!("Page: {}\n", page_line.join(" · ")));
        }
        for (i, note) in req.notes.iter().enumerate() {
            out.push('\n');
            format_note(&mut out, i + 1, note, i < html_upto);
        }
        out.trim_end().to_string()
    };
    // Drop HTML from the last notes first until the post fits.
    let mut keep = req.notes.len();
    loop {
        let s = render(keep);
        if s.chars().count() <= MAX_FORMATTED_CHARS || keep == 0 {
            return s;
        }
        keep -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element() -> NoteElement {
        NoteElement {
            selector: "main > button.cta".into(),
            tag: "BUTTON".into(),
            classes: vec!["cta".into()],
            role: Some("button".into()),
            name: Some("Buy".into()),
            text: "Buy".into(),
            html: "<button class=\"cta\" onclick=\"steal()\">Buy</button>".into(),
            rect: Some(NoteRect { x: 24.4, y: 310.0, width: 160.0, height: 40.0 }),
            ..Default::default()
        }
    }

    fn request(n: usize) -> NotesRequest {
        NotesRequest {
            workspace: "docs".into(),
            page: NotesPage {
                url: "http://localhost:5173/pricing?token=abc#top".into(),
                title: "Pricing".into(),
                viewport: Some(NoteViewport { width: 1280.0, height: 800.0 }),
            },
            notes: (0..n)
                .map(|i| DesignNote { payload: Some(element()), comment: format!("note {i}"), intent: NoteIntent::Change })
                .collect(),
            item_id: Some("design-1".into()),
        }
    }

    #[test]
    fn url_strips_query_hash_and_userinfo_and_refuses_other_schemes() {
        assert_eq!(clean_page_url("http://localhost:5173/a?x=1#h").unwrap(), "http://localhost:5173/a");
        assert_eq!(clean_page_url("https://u:p@example.com/p?access_token=z").unwrap(), "https://example.com/p");
        assert!(clean_page_url("javascript:alert(1)").unwrap_err().contains("http or https"));
        assert!(clean_page_url("file:///etc/passwd").unwrap_err().contains("http or https"));
        assert!(clean_page_url("").unwrap_err().contains("required"));
        let long = format!("https://example.com/{}", "a".repeat(3000));
        assert!(clean_page_url(&long).unwrap_err().contains("longer"));
    }

    #[test]
    fn html_drops_scripts_handlers_comments_and_redacts_secrets() {
        let raw = r#"<div onclick="x()" data-token="sk-123" class="a"><!-- c --><script>alert(1)</script><iframe src="x"></iframe><input type="password" value="hunter2"><input type="text" value="ok"><a href="javascript:evil()">l</a><p>hi</p></div>"#;
        let out = sanitize_html(raw, MAX_HTML_CHARS);
        assert!(!out.contains("onclick"), "{out}");
        assert!(!out.contains("sk-123"), "{out}");
        assert!(out.contains("data-token=\"[redacted]\""), "{out}");
        assert!(!out.contains("alert"), "{out}");
        assert!(!out.contains("iframe"), "{out}");
        assert!(!out.contains("<!--"), "{out}");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("value=\"ok\""), "{out}");
        assert!(!out.contains("evil"), "{out}");
        assert!(out.contains("<p>hi</p>"), "{out}");
        assert!(out.contains("class=\"a\""), "{out}");
    }

    #[test]
    fn html_unclosed_script_drops_the_rest() {
        let out = sanitize_html("<b>x</b><script>never closed", 100);
        assert_eq!(out, "<b>x</b>");
    }

    #[test]
    fn element_fields_are_clamped_and_styles_allowlisted() {
        let mut e = element();
        e.selector = "s".repeat(1000);
        e.text = "t".repeat(1000);
        e.html = format!("<p>{}</p>", "h".repeat(10_000));
        e.classes = (0..20).map(|i| format!("c{i}")).collect();
        e.nearby_text = (0..9).map(|_| "n".repeat(500)).collect();
        e.styles.insert("color".into(), "red".into());
        e.styles.insert("background-image".into(), "url(https://x/?token=1)".into());
        e.rect = Some(NoteRect { x: f64::NAN, y: 0.0, width: 1.0, height: 1.0 });
        let c = clamp_element(&e);
        assert_eq!(c.selector.chars().count(), MAX_SELECTOR_CHARS);
        assert_eq!(c.text.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(c.html.chars().count(), MAX_HTML_CHARS);
        assert_eq!(c.classes.len(), MAX_CLASSES);
        assert_eq!(c.nearby_text.len(), MAX_NEARBY);
        assert!(c.nearby_text.iter().all(|t| t.chars().count() == MAX_NEARBY_CHARS));
        assert_eq!(c.styles.get("color").map(String::as_str), Some("red"));
        assert!(!c.styles.contains_key("background-image"));
        assert_eq!(c.rect, None, "a non-finite rect is dropped");
        assert_eq!(c.tag, "button");
    }

    #[test]
    fn request_limits_are_the_persons_errors() {
        assert!(clamp_request(&request(0)).unwrap_err().contains("no notes"));
        assert!(clamp_request(&request(MAX_NOTES + 1)).unwrap_err().contains("at most"));
        let mut r = request(1);
        r.notes[0].comment = "   ".into();
        assert!(clamp_request(&r).unwrap_err().contains("no text"));
        let mut r = request(1);
        r.notes[0].comment = "x".repeat(5000);
        let c = clamp_request(&r).unwrap();
        assert_eq!(c.notes[0].comment.chars().count(), MAX_COMMENT_CHARS);
        assert_eq!(c.page.url, "http://localhost:5173/pricing");
    }

    #[test]
    fn format_is_stable() {
        let c = clamp_request(&request(1)).unwrap();
        let s = format_notes(&c);
        let want = "Design notes (1) on http://localhost:5173/pricing\n\
Page: Pricing · 1280×800\n\
\n\
### 1. Change: `<button.cta>`\n\
note 0\n\
\n\
- selector: `main > button.cta`\n\
- a11y: role=button name=\"Buy\"\n\
- box: 160×40 at (24, 310)\n\
- text: Buy\n\
\n\
```html\n\
<button class=\"cta\">Buy</button>\n\
```";
        assert_eq!(s, want);
    }

    #[test]
    fn page_level_question_and_backticks() {
        let mut r = request(1);
        r.notes[0].payload = None;
        r.notes[0].intent = NoteIntent::Question;
        r.notes[0].comment = "why is the header sticky?".into();
        let s = format_notes(&clamp_request(&r).unwrap());
        assert!(s.contains("### 1. Question (whole page)\nwhy is the header sticky?"), "{s}");
        let mut r = request(1);
        r.notes[0].payload.as_mut().unwrap().selector = "a`b".into();
        r.notes[0].payload.as_mut().unwrap().html = "<pre>```</pre>".into();
        let s = format_notes(&clamp_request(&r).unwrap());
        assert!(s.contains("- selector: `a'b`"), "{s}");
        assert!(s.contains("<pre>'''</pre>"), "{s}");
    }

    #[test]
    fn oversize_batches_drop_html_from_the_end_first() {
        let mut r = request(MAX_NOTES);
        for n in &mut r.notes {
            n.comment = "c".repeat(1000);
            n.payload.as_mut().unwrap().html = format!("<p>{}</p>", "h".repeat(MAX_HTML_CHARS));
        }
        let s = format_notes(&clamp_request(&r).unwrap());
        assert!(s.chars().count() <= MAX_FORMATTED_CHARS, "{}", s.chars().count());
        assert!(s.contains("(left out: the notes were too long)"));
        assert!(s.starts_with("Design notes (20) on "));
    }

    #[test]
    fn intent_parse() {
        assert_eq!(NoteIntent::parse("").unwrap(), NoteIntent::Change);
        assert_eq!(NoteIntent::parse("Question").unwrap(), NoteIntent::Question);
        assert!(NoteIntent::parse("delete").is_err());
        let v: DesignNote = serde_json::from_value(serde_json::json!({"comment": "x", "intent": "question"})).unwrap();
        assert_eq!(v.intent, NoteIntent::Question);
    }
}
