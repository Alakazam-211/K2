//! The mail reader's HTML cleaner (prd-ocmail-admin-in-k2-v1 §5.4, §7.5).
//!
//! Mail is written by whoever sent it, so its HTML is hostile until proven
//! otherwise. The admin reader cleans it here, in the daemon, before it
//! leaves; the renderer cleans it again (DOMPurify) and draws it in an inert
//! frame (no script, no network, no same-origin). This is the first layer.
//!
//! What comes out:
//! - no script, style sheet, form, frame, object, SVG or MathML, and no event
//!   handler attributes (an allowlist: anything not listed goes);
//! - **no remote content**: an `img` keeps its `src` only when it is an
//!   inline `data:image/…;base64,` picture; `http(s):` and `cid:` pictures
//!   lose their `src` (the alt text stays), and `remote_blocked` says so;
//! - inline `style` is kept unless it could load something or run
//!   something (`url(`, `@import`, `expression(`, `javascript:`,
//!   `behavior`, `-moz-binding`): such a `style` goes whole;
//! - **links don't navigate**: an `a` keeps its target only as
//!   `data-k2-link` (http, https or mailto), never as `href`, so a click in
//!   the frame goes nowhere. When the sender gave the link no `title`, the
//!   target becomes its title too, so hovering shows where it really goes.
//!
//! `ammonia` (MIT OR Apache-2.0) does the parsing and the allowlist; it is
//! already this crate's dependency (the ticket brief cleaner).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Tags a cleaned message may keep.
const MAIL_TAGS: &[&str] = &[
    "a", "abbr", "address", "article", "b", "bdi", "bdo", "big", "blockquote", "br", "caption",
    "center", "cite", "code", "col", "colgroup", "dd", "del", "details", "dfn", "div", "dl", "dt",
    "em", "figcaption", "figure", "font", "footer", "h1", "h2", "h3", "h4", "h5", "h6", "header",
    "hr", "i", "img", "ins", "kbd", "li", "main", "mark", "ol", "p", "pre", "q", "s", "samp",
    "section", "small", "span", "strike", "strong", "sub", "summary", "sup", "table", "tbody",
    "td", "tfoot", "th", "thead", "time", "tr", "tt", "u", "ul", "var", "wbr",
];

/// Tags removed WITH their contents (everything else outside the allowlist
/// is unwrapped: the tag goes, its text stays).
const MAIL_DROP_WITH_CONTENT: &[&str] = &[
    "script", "style", "title", "head", "template", "iframe", "frame", "frameset", "object",
    "embed", "applet", "svg", "math", "noscript", "textarea", "select", "button", "form",
    "audio", "video", "canvas", "xmp", "noembed", "noframes", "plaintext",
];

/// Attributes every kept tag may carry (presentation only).
const MAIL_GENERIC_ATTRS: &[&str] = &[
    "align", "bgcolor", "border", "cellpadding", "cellspacing", "color", "dir", "face", "height",
    "lang", "size", "style", "title", "valign", "width",
];

/// What a `style` value must not contain (case-insensitive): anything that
/// loads a resource or runs code.
const STYLE_BANNED: &[&str] =
    &["url(", "@import", "expression(", "javascript:", "vbscript:", "behavior", "-moz-binding", "image-set("];

/// Link schemes a message link may name.
const LINK_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// A cleaned message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanMail {
    pub html: String,
    /// A remote (or `cid:`) picture or a style that would load something
    /// was taken out.
    pub remote_blocked: bool,
}

fn is_inline_picture(value: &str) -> bool {
    let v = value.trim_start().to_ascii_lowercase();
    ["png", "jpeg", "jpg", "gif", "webp"]
        .iter()
        .any(|kind| v.starts_with(&format!("data:image/{kind};base64,")))
}

fn is_link_target(value: &str) -> bool {
    match url::Url::parse(value.trim()) {
        Ok(u) => LINK_SCHEMES.contains(&u.scheme()),
        Err(_) => false,
    }
}

fn style_is_inert(value: &str) -> bool {
    let v: String = value.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    // `\` escapes can spell any banned word (`u\72l(`): refuse them outright.
    !v.contains('\\') && !STYLE_BANNED.iter().any(|b| v.contains(b))
}

fn builder(blocked: Arc<AtomicBool>) -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::empty();
    b.tags(MAIL_TAGS.iter().copied().collect())
        .clean_content_tags(MAIL_DROP_WITH_CONTENT.iter().copied().collect())
        .generic_attributes(MAIL_GENERIC_ATTRS.iter().copied().collect())
        .tag_attributes(HashMap::from([
            ("a", HashSet::from(["href"])),
            ("img", HashSet::from(["src", "alt"])),
            ("td", HashSet::from(["colspan", "rowspan", "nowrap"])),
            ("th", HashSet::from(["colspan", "rowspan", "nowrap", "scope"])),
            ("col", HashSet::from(["span"])),
            ("colgroup", HashSet::from(["span"])),
            ("ol", HashSet::from(["start", "type"])),
            ("ul", HashSet::from(["type"])),
            ("li", HashSet::from(["value"])),
            ("details", HashSet::from(["open"])),
        ]))
        // `url_schemes` is global; the attribute filter narrows it per tag.
        .url_schemes(HashSet::from(["http", "https", "mailto", "data", "cid"]))
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(move |element, attribute, value| match (element, attribute) {
            ("a", "href") if !is_link_target(value) => None,
            ("img", "src") if !is_inline_picture(value) => {
                blocked.store(true, Ordering::Relaxed);
                None
            }
            (_, "style") if !style_is_inert(value) => {
                blocked.store(true, Ordering::Relaxed);
                None
            }
            _ => Some(Cow::Borrowed(value)),
        });
    b
}

/// Rename every `href` attribute on an `a` start tag in ammonia's output to
/// `data-k2-link`, and give a link with no `title` its target as the title.
/// ammonia serializes every attribute as `name="value"` with `"` escaped
/// inside values and `<` escaped in text, so a scan that tracks double
/// quotes inside tags is exact here (and a value copied from one attribute
/// to another stays escaped).
fn hrefs_to_data_links(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len() + 32);
    let mut i = 0;
    while i < bytes.len() {
        let is_a_tag = bytes[i] == b'<'
            && bytes.get(i + 1).is_some_and(|c| c.eq_ignore_ascii_case(&b'a'))
            && bytes.get(i + 2).is_some_and(|c| *c == b' ' || *c == b'>' || *c == b'/');
        if !is_a_tag {
            // Copy up to the next '<' as one slice (UTF-8 safe: '<' is ASCII).
            let next = html[i + 1..].find('<').map(|p| p + i + 1).unwrap_or(bytes.len());
            out.push_str(&html[i..next]);
            i = next;
            continue;
        }
        // Inside `<a …>`: walk to the closing '>' outside quotes.
        let mut j = i;
        let mut in_quote = false;
        while j < bytes.len() {
            match bytes[j] {
                b'"' => in_quote = !in_quote,
                b'>' if !in_quote => break,
                _ => {}
            }
            j += 1;
        }
        let end = (j + 1).min(bytes.len());
        let tag = &html[i..end];
        let mut rewritten = String::with_capacity(tag.len() + 16);
        let tb = tag.as_bytes();
        let mut k = 0;
        let mut q = false;
        let mut has_title = false;
        let mut target: Option<String> = None;
        while k < tb.len() {
            if tb[k] == b'"' {
                q = !q;
            }
            if !q && tb[k] == b' ' && tag[k + 1..].starts_with("title=\"") {
                has_title = true;
            }
            if !q && tb[k] == b' ' && tag[k + 1..].starts_with("href=\"") {
                rewritten.push_str(" data-k2-link=\"");
                k += " href=\"".len();
                let close = tag[k..].find('"').map(|p| p + k).unwrap_or(tag.len());
                target = Some(tag[k..close].to_string());
                q = true;
                continue;
            }
            let ch_end = tag[k..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            rewritten.push_str(&tag[k..k + ch_end]);
            k += ch_end;
        }
        if let (Some(t), false) = (target, has_title) {
            // `<a …>` → `<a … title="target">` (before the closing `>`).
            if rewritten.ends_with('>') {
                rewritten.pop();
                rewritten.push_str(&format!(" title=\"{t}\">"));
            }
        }
        out.push_str(&rewritten);
        i = end;
    }
    out
}

/// Clean one message's HTML for the reader (see the module docs).
pub fn clean_mail_html(raw: &str) -> CleanMail {
    let blocked = Arc::new(AtomicBool::new(false));
    let cleaned = builder(blocked.clone()).clean(raw).to_string();
    CleanMail { html: hrefs_to_data_links(&cleaned), remote_blocked: blocked.load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T9: script, event handlers, remote pictures, style sheets, link tags
    /// and CSS url(…) all come out; text survives.
    #[test]
    fn t9_hostile_mail_html_comes_back_with_none_of_it() {
        let raw = r#"<html><head><link rel="stylesheet" href="https://evil.example/a.css">
            <style>body{background:url(https://evil.example/track.gif)}</style>
            <script>alert(1)</script></head>
            <body onload="alert(2)">
            <p onclick="alert(3)" style="color:red">Hello <b>there</b></p>
            <img src="https://evil.example/pixel.gif" onerror="alert(4)" alt="pixel">
            <img src="cid:part1@x" alt="logo">
            <div style="background-image:url(http://evil.example/bg.png)">bg</div>
            <div style="background:u\72l(http://evil.example/x)">esc</div>
            <a href="javascript:alert(5)">js</a>
            <a href="https://bank.example/login" title="Your bank">bank</a>
            <iframe src="https://evil.example/frame"></iframe>
            <form action="https://evil.example/steal"><input name="pw"></form>
            <svg><image href="https://evil.example/s.png"/></svg>
            <meta http-equiv="refresh" content="0;url=https://evil.example">
            <base href="https://evil.example/">
            </body></html>"#;
        let c = clean_mail_html(raw);
        let lower = c.html.to_ascii_lowercase();
        for banned in [
            "<script", "alert(", "onload", "onclick", "onerror", "<style", "<link", "url(", "u\\72l",
            "<iframe", "<form", "<input", "<svg", "<meta", "<base", "evil.example", "javascript:",
            " href=", "cid:",
        ] {
            assert!(!lower.contains(banned), "{banned:?} survived: {}", c.html);
        }
        assert!(c.html.contains("Hello <b>there</b>"), "{}", c.html);
        assert!(c.html.contains(r#"style="color:red""#), "harmless style stays: {}", c.html);
        assert!(c.html.contains(r#"alt="pixel""#), "alt text stays: {}", c.html);
        assert!(
            c.html.contains(r#"data-k2-link="https://bank.example/login""#),
            "the link target stays visible, inert: {}",
            c.html
        );
        assert!(c.html.contains(r#"title="Your bank""#), "the sender's title stays: {}", c.html);
        assert!(!c.html.contains(r#"title="https://bank.example/login""#), "{}", c.html);
        let bare = clean_mail_html(r#"<a href="https://x.example/a?b=1&amp;c=2">x</a>"#);
        assert_eq!(
            bare.html,
            r#"<a data-k2-link="https://x.example/a?b=1&amp;c=2" title="https://x.example/a?b=1&amp;c=2">x</a>"#,
            "no title: the target becomes the hover text"
        );
        assert!(c.remote_blocked, "the remote pictures and the url() style are reported");
    }

    #[test]
    fn inline_pictures_stay_and_plain_mail_reports_nothing_blocked() {
        let c = clean_mail_html(r#"<p>Hi</p><img src="data:image/png;base64,iVBORw0KGgo=" alt="x">"#);
        assert!(c.html.contains(r#"src="data:image/png;base64,iVBORw0KGgo=""#), "{}", c.html);
        assert!(!c.remote_blocked);
        let svg = clean_mail_html(r#"<img src="data:image/svg+xml;base64,PHN2Zz4=">"#);
        assert!(!svg.html.contains("svg"), "an SVG picture can carry script: {}", svg.html);
    }

    #[test]
    fn a_quote_or_angle_bracket_in_a_link_does_not_break_out() {
        let c = clean_mail_html(r#"<a href="https://x.example/?q=&quot;&gt;&lt;b&gt;" title='t"x'>go</a><p>then</p>"#);
        assert!(!c.html.contains(" href="), "{}", c.html);
        assert!(c.html.starts_with(r#"<a data-k2-link="https://x.example/?q=&quot;"#), "{}", c.html);
        assert!(c.html.contains(r#"title="t&quot;x""#), "{}", c.html);
        assert!(c.html.ends_with(">go</a><p>then</p>"), "the tag closed where it should: {}", c.html);
        assert_eq!(c.html.matches("title=").count(), 1, "the sender's title is kept, none added: {}", c.html);
        // Text that looks like an attribute is left alone.
        let t = clean_mail_html(r#"<p>write href="x" here</p>"#);
        assert_eq!(t.html, r#"<p>write href="x" here</p>"#);
    }
}
