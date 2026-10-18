//! What a script *does*, for `k2 zen inspect` (Rosson 2026-10-11: importing
//! a Garden is an agent workflow; the agent reviews before it adds). Static
//! only: the code is parsed, never run. Lists the K2 verbs it calls, the
//! network and escape attempts the sealed frame would block (or that need a
//! look), the URLs it names, and obfuscation signals.

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::GetSpan;

use super::Lang;

/// One thing found at a byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub offset: u32,
    /// A stable kind: `fetch`, `websocket`, `dynamic-import`, `eval`, …
    pub kind: &'static str,
    /// Extra detail (a URL, a verb, a length). Untrusted text: callers
    /// escape and cap it before showing it.
    pub detail: String,
}

/// Everything one script reveals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// `k2` verbs called: (verb, offset). `k2.can('x')` counts as a mention.
    pub verbs: Vec<(String, u32)>,
    /// Members of the frame's `k2` object used (`connected`, `ready`, …).
    pub k2_members: Vec<String>,
    pub network: Vec<Hit>,
    pub urls: Vec<Hit>,
    pub obfuscation: Vec<Hit>,
    /// Lines longer than 2,000 characters (minified or packed code).
    pub long_lines: usize,
}

/// A string this long that is all base64/hex characters looks encoded.
const ENCODED_MIN: usize = 200;
const LONG_LINE: usize = 2000;

/// The inventory of `text`, or `None` when it doesn't parse (the parse
/// errors come from [`super::check`]).
pub fn inventory(text: &str, lang: Lang) -> Option<Inventory> {
    let alloc = Allocator::default();
    let parsed = Parser::new(&alloc, text, lang.source_type()).parse();
    if parsed.panicked || parsed.diagnostics.has_errors() {
        return None;
    }
    let sem = SemanticBuilder::new().build(&parsed.program);
    let mut v = Inv { scoping: sem.semantic.scoping(), out: Inventory::default() };
    v.visit_program(&parsed.program);
    v.out.long_lines = text.lines().filter(|l| l.chars().count() > LONG_LINE).count();
    // `\x41`-style escapes packed into the raw text.
    let escapes = text.matches("\\x").count() + text.matches("\\u00").count();
    if escapes >= 40 {
        v.out.obfuscation.push(Hit { offset: 0, kind: "escaped-text", detail: format!("{escapes} \\x / \\u00 escapes") });
    }
    v.out.network.sort_by_key(|h| h.offset);
    v.out.obfuscation.sort_by_key(|h| h.offset);
    Some(v.out)
}

struct Inv<'s> {
    scoping: &'s Scoping,
    out: Inventory,
}

const K2_RUNTIME: &[&str] = &["call", "subscribe", "on", "can", "connected", "ready", "asset", "config", "widget", "motion", "shape", "K2Error", "retryable"];
const GLOBAL_OBJECTS: &[&str] = &["window", "globalThis", "self", "top", "parent"];

impl Inv<'_> {
    fn global(&self, id: &IdentifierReference<'_>, name: &str) -> bool {
        id.name.as_str() == name && self.scoping.get_reference(id.reference_id()).symbol_id().is_none()
    }

    /// `name` or `window.name` (and `globalThis.` / `self.`) as a global.
    fn global_ref(&self, e: &Expression<'_>, name: &str) -> bool {
        match e.without_parentheses() {
            Expression::Identifier(id) => self.global(id, name),
            Expression::StaticMemberExpression(m) => {
                m.property.name.as_str() == name
                    && matches!(m.object.without_parentheses(), Expression::Identifier(o)
                        if GLOBAL_OBJECTS.iter().any(|g| self.global(o, g)))
            }
            _ => false,
        }
    }

    fn net(&mut self, offset: u32, kind: &'static str, detail: impl Into<String>) {
        self.out.network.push(Hit { offset, kind, detail: detail.into() });
    }

    fn obf(&mut self, offset: u32, kind: &'static str, detail: impl Into<String>) {
        self.out.obfuscation.push(Hit { offset, kind, detail: detail.into() });
    }

    fn first_string(&self, args: &[Argument<'_>]) -> Option<String> {
        match args.first().and_then(|a| a.as_expression())?.without_parentheses() {
            Expression::StringLiteral(s) => Some(s.value.to_string()),
            Expression::TemplateLiteral(t) if t.expressions.is_empty() => {
                t.quasis.first().map(|q| q.value.raw.to_string())
            }
            _ => None,
        }
    }

    fn string_value(&mut self, value: &str, offset: u32) {
        let v = value.trim();
        let lower = v.to_ascii_lowercase();
        if ["http://", "https://", "ws://", "wss://", "ftp://"].iter().any(|p| lower.starts_with(p)) || (lower.starts_with("//") && lower.len() > 3 && lower.as_bytes()[2].is_ascii_alphanumeric()) {
            self.out.urls.push(Hit { offset, kind: "url", detail: v.to_string() });
        }
        if lower.starts_with("data:text/html") || lower.starts_with("data:application/javascript") || lower.starts_with("javascript:") {
            self.obf(offset, "code-url", v.chars().take(40).collect::<String>());
        }
        let n = v.chars().count();
        if n >= ENCODED_MIN && !v.contains(char::is_whitespace) {
            let b64 = v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'));
            if b64 {
                self.obf(offset, "encoded-string", format!("{n} characters of base64/hex-like text"));
            }
        }
    }
}

impl<'a> Visit<'a> for Inv<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let callee = it.callee.without_parentheses();
        let at = it.span.start;
        // k2 verbs.
        if let Expression::StaticMemberExpression(outer) = callee {
            match outer.object.without_parentheses() {
                Expression::Identifier(root) if self.global(root, "k2") => {
                    let m = outer.property.name.as_str();
                    if matches!(m, "call" | "subscribe" | "on" | "can") {
                        if let Some(v) = self.first_string(&it.arguments) {
                            self.out.verbs.push((v, at));
                        } else {
                            self.obf(at, "dynamic-verb", format!("k2.{m}(…) with a computed verb name"));
                        }
                    }
                }
                Expression::StaticMemberExpression(inner) => {
                    if let Expression::Identifier(root) = inner.object.without_parentheses() {
                        let s = inner.property.name.as_str();
                        if self.global(root, "k2") && !K2_RUNTIME.contains(&s) {
                            self.out.verbs.push((format!("{s}.{}", outer.property.name), at));
                        }
                    }
                }
                _ => {}
            }
        }
        // Network and escapes.
        if self.global_ref(callee, "fetch") {
            let url = self.first_string(&it.arguments).unwrap_or_default();
            self.net(at, "fetch", url);
        }
        if self.global_ref(callee, "importScripts") {
            self.net(at, "importScripts", self.first_string(&it.arguments).unwrap_or_default());
        }
        if let Expression::StaticMemberExpression(m) = callee {
            let p = m.property.name.as_str();
            let obj = m.object.without_parentheses();
            if p == "sendBeacon" {
                self.net(at, "sendBeacon", self.first_string(&it.arguments).unwrap_or_default());
            }
            if p == "open" && GLOBAL_OBJECTS.iter().any(|g| matches!(obj, Expression::Identifier(o) if self.global(o, g))) {
                self.net(at, "window-open", self.first_string(&it.arguments).unwrap_or_default());
            }
            if matches!(p, "assign" | "replace") && matches!(obj, Expression::Identifier(o) if self.global(o, "location"))
                || matches!(p, "assign" | "replace") && matches!(obj, Expression::StaticMemberExpression(l) if l.property.name.as_str() == "location")
            {
                self.net(at, "navigate", self.first_string(&it.arguments).unwrap_or_default());
            }
            if p == "postMessage" && matches!(obj, Expression::Identifier(o) if ["parent", "top", "opener"].iter().any(|g| self.global(o, g)))
            {
                self.net(at, "post-to-parent", String::new());
            }
            if p == "register" && matches!(obj, Expression::StaticMemberExpression(s) if s.property.name.as_str() == "serviceWorker") {
                self.net(at, "service-worker", self.first_string(&it.arguments).unwrap_or_default());
            }
            if p == "fromCharCode" && obj.is_specific_id("String") && it.arguments.len() >= 8 {
                self.obf(at, "char-codes", format!("String.fromCharCode with {} codes", it.arguments.len()));
            }
        }
        // Obfuscation.
        if self.global_ref(callee, "eval") {
            self.obf(at, "eval", String::new());
        }
        if self.global_ref(callee, "Function") {
            self.obf(at, "function-constructor", String::new());
        }
        if self.global_ref(callee, "atob") || self.global_ref(callee, "unescape") {
            let name = if self.global_ref(callee, "atob") { "atob" } else { "unescape" };
            self.obf(at, "decode", name.to_string());
        }
        for timer in ["setTimeout", "setInterval"] {
            if self.global_ref(callee, timer) && matches!(it.arguments.first().and_then(|a| a.as_expression()).map(|e| e.without_parentheses()), Some(Expression::StringLiteral(_) | Expression::TemplateLiteral(_) | Expression::BinaryExpression(_)))
            {
                self.obf(at, "string-timer", format!("{timer} with code in a string"));
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let callee = it.callee.without_parentheses();
        let at = it.span.start;
        for (name, kind) in [
            ("XMLHttpRequest", "xhr"),
            ("WebSocket", "websocket"),
            ("EventSource", "event-source"),
            ("RTCPeerConnection", "webrtc"),
            ("Worker", "worker"),
            ("SharedWorker", "worker"),
        ] {
            if self.global_ref(callee, name) {
                let detail = self.first_string(&it.arguments).unwrap_or_default();
                self.net(at, kind, detail);
            }
        }
        if self.global_ref(callee, "Function") {
            self.obf(at, "function-constructor", String::new());
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        let detail = match it.source.without_parentheses() {
            Expression::StringLiteral(s) => s.value.to_string(),
            _ => "computed".into(),
        };
        self.net(it.span.start, "dynamic-import", detail.clone());
        self.obf(it.span.start, "dynamic-import", detail);
        walk::walk_import_expression(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let Some(m) = it.left.as_member_expression() {
            let p = m.static_property_name();
            let obj = m.object().without_parentheses();
            let value = match it.right.without_parentheses() {
                Expression::StringLiteral(s) => s.value.to_string(),
                _ => String::new(),
            };
            if p == Some("href") && matches!(obj, Expression::Identifier(o) if self.global(o, "location"))
                || p == Some("location") && matches!(obj, Expression::Identifier(o) if GLOBAL_OBJECTS.iter().any(|g| self.global(o, g)) || self.global(o, "document"))
            {
                self.net(m.span().start, "navigate", value.clone());
            }
            if p == Some("cookie") && matches!(obj, Expression::Identifier(o) if self.global(o, "document")) {
                self.net(m.span().start, "cookie", String::new());
            }
            if p == Some("src") && (value.starts_with("http") || value.starts_with("//")) {
                self.net(m.span().start, "remote-src", value.clone());
            }
        } else if let AssignmentTarget::AssignmentTargetIdentifier(id) = &it.left {
            if self.global(id, "location") {
                self.net(it.span.start, "navigate", String::new());
            }
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.scoping.get_reference(it.reference_id()).symbol_id().is_none() {
            match it.name.as_str() {
                "localStorage" | "sessionStorage" | "indexedDB" => self.net(it.span.start, "storage", it.name.to_string()),
                _ => {}
            }
        }
        walk::walk_identifier_reference(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(root) = it.object.without_parentheses() {
            if self.global(root, "k2") {
                let p = it.property.name.to_string();
                if K2_RUNTIME.contains(&p.as_str()) && !self.out.k2_members.contains(&p) {
                    self.out.k2_members.push(p);
                }
            }
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        self.string_value(&it.value.to_string(), it.span.start);
    }

    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        for q in &it.quasis {
            self.string_value(&q.value.raw.to_string(), q.span.start);
        }
        walk::walk_template_literal(self, it);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(src: &str) -> Inventory {
        inventory(src, Lang::Script).unwrap_or_else(|| panic!("parses: {src}"))
    }

    #[test]
    fn verbs_members_and_network_are_listed() {
        let i = inv(
            "k2.connected.then(() => k2.agents.subscribe(draw))\nk2.call('thread.read', 'k2/1')\nfetch('https://evil.example/x')\nnew WebSocket('wss://w.example')\nnavigator.sendBeacon('/b', d)\nparent.postMessage(1, '*')\nimport('./x.js')\ndocument.cookie = 'a=1'\nlocation.href = 'https://away.example'\nfunction draw() {}\n",
        );
        let verbs: Vec<&str> = i.verbs.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(verbs, vec!["agents.subscribe", "thread.read"]);
        assert_eq!(i.k2_members, vec!["connected".to_string(), "call".to_string()]);
        let kinds: Vec<&str> = i.network.iter().map(|h| h.kind).collect();
        assert_eq!(kinds, vec!["fetch", "websocket", "sendBeacon", "post-to-parent", "dynamic-import", "cookie", "navigate"]);
        assert_eq!(i.network[0].detail, "https://evil.example/x");
        let urls: Vec<&str> = i.urls.iter().map(|h| h.detail.as_str()).collect();
        assert_eq!(urls, vec!["https://evil.example/x", "wss://w.example", "https://away.example"]);
    }

    #[test]
    fn obfuscation_signals() {
        let blob = "QUJD".repeat(80);
        let src = format!(
            "eval(atob('{blob}'))\nnew Function('return 1')\nsetTimeout('go()', 10)\nString.fromCharCode(1,2,3,4,5,6,7,8)\nk2.call(name)\n"
        );
        let i = inv(&src);
        let mut kinds: Vec<&str> = i.obfuscation.iter().map(|h| h.kind).collect();
        kinds.sort();
        assert_eq!(kinds, vec!["char-codes", "decode", "dynamic-verb", "encoded-string", "eval", "function-constructor", "string-timer"]);
    }

    #[test]
    fn shadowed_names_are_not_globals_and_broken_code_has_no_inventory() {
        let i = inv("function f(fetch, k2) { fetch('https://x.example'); k2.thread.post('a', 'b') }\n");
        assert!(i.network.is_empty() && i.verbs.is_empty(), "{i:?}");
        assert_eq!(i.urls.len(), 1, "a URL string is still a URL: {i:?}");
        assert!(inventory("const = 1", Lang::Script).is_none());
    }
}
