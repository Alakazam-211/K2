//! The AST rules behind [`super::check`]: widget rules, the catalog lint
//! and the leak lints. Two walks: the first finds the **handlers** (code
//! that runs again and again: event listeners, request handlers, `k2`
//! subscriptions, `setInterval` callbacks, exported route handlers), the
//! second runs every rule knowing which functions those are.
//!
//! "Module level" is the top of the file plus the body of a top-level
//! IIFE (`(function () { … })()`), the way widget scripts are written.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use oxc_ast::ast::*;
use oxc_ast_visit::{walk, Visit};
use oxc_semantic::Scoping;
use oxc_syntax::{scope::ScopeId, symbol::SymbolId};
use oxc_span::GetSpan;
use oxc_syntax::scope::ScopeFlags;

use super::{Finding, Severity, Surface};
use crate::contract::{self, Exposure, VerbKind};

/// Members of the frame's `k2` object that aren't verb surfaces
/// (`sdk/k2-runtime.js`, "The k2 object").
const K2_RUNTIME_MEMBERS: &[&str] =
    &["call", "subscribe", "on", "can", "connected", "ready", "asset", "config", "widget", "motion", "shape", "K2Error", "retryable"];

/// Hand-written frame-runtime helpers on a generated surface (no catalog
/// row of their own, so no arity check; `contract::gen::SDK_SURFACE_EXTRAS`
/// documents their shapes): `k2.store.subscribe` streams `store.changes`;
/// the tx / chunked helpers wrap `store.*` calls.
const FRAME_HELPERS: &[&str] = &["store.subscribe", "store.tx", "store.putChunked", "store.getChunked"];

/// Surfaces the frame makes callable: `k2.viewer()` is `viewer.get`.
const CALLABLE_SURFACES: &[(&str, &str)] = &[("viewer", "viewer.get")];

/// A row a custom widget can call: exposed to `widget` (the bridge) or
/// `garden-local` (this computer's daemon, contract-garden-data-v1 §7.2) —
/// the same set as `Catalog::widget_callable_verbs`.
pub(crate) fn widget_callable(r: &contract::VerbRow) -> bool {
    r.exposed_to(Exposure::Widget) || r.exposed_to(Exposure::GardenLocal)
}

/// A call on `x.<name>(…)` whose function arguments run again and again.
const REGISTRARS: &[&str] = &[
    "addEventListener",
    "on",
    "once",
    "addListener",
    "prependListener",
    "subscribe",
    "subscribeCurrent",
    "changed",
    "use",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "all",
    "head",
    "options",
    "handle",
    "createServer",
    "setInterval",
    "serve",
];

/// Exported function names a framework calls per request (Next, Remix,
/// SvelteKit, plain `handler`).
const EXPORTED_HANDLERS: &[&str] =
    &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "handler", "middleware", "loader", "action", "fetch"];

const LISTEN_METHODS: &[&str] = &["addEventListener", "on", "once", "addListener", "prependListener"];
const UNLISTEN_METHODS: &[&str] = &["removeEventListener", "off", "removeListener", "removeAllListeners"];
const LONG_LIVED_GLOBALS: &[&str] = &["window", "document", "process", "globalThis", "self", "navigator"];

pub(super) fn run(program: &Program<'_>, scoping: &Scoping, surface: Surface) -> Vec<Finding> {
    let mut pre = Prepass { scoping, handler_spans: HashSet::new(), handler_symbols: HashSet::new() };
    pre.visit_program(program);
    let module_scopes = module_scopes(program, scoping);
    let mut l = Lint {
        scoping,
        surface,
        module_scopes,
        handler_spans: pre.handler_spans,
        handler_symbols: pre.handler_symbols,
        depth: 0,
        discarded: HashSet::new(),
        out: Vec::new(),
        widget_seen: BTreeSet::new(),
        collections: BTreeMap::new(),
        growth: BTreeMap::new(),
        shrunk: HashSet::new(),
        intervals: Vec::new(),
        clears_interval: false,
        listens: Vec::new(),
        unlistened: HashSet::new(),
        session_fns: HashSet::new(),
    };
    l.visit_program(program);
    l.finish();
    l.out.sort_by_key(|f| f.offset);
    l.out
}

/// The root scope plus the function scope of every top-level IIFE.
fn module_scopes(program: &Program<'_>, scoping: &Scoping) -> HashSet<ScopeId> {
    let mut out = HashSet::new();
    out.insert(scoping.root_scope_id());
    for stmt in &program.body {
        let Statement::ExpressionStatement(es) = stmt else { continue };
        let mut e = es.expression.without_parentheses();
        if let Expression::UnaryExpression(u) = e {
            e = u.argument.without_parentheses();
        }
        let Expression::CallExpression(call) = e else { continue };
        let mut callee = call.callee.without_parentheses();
        // `(function () {}).call(this)`
        if let Expression::StaticMemberExpression(m) = callee {
            if matches!(m.property.name.as_str(), "call" | "apply") {
                callee = m.object.without_parentheses();
            }
        }
        match callee {
            Expression::FunctionExpression(f) => {
                out.insert(f.scope_id());
            }
            Expression::ArrowFunctionExpression(f) => {
                out.insert(f.scope_id());
            }
            _ => {}
        }
    }
    out
}

/// The property name of `x.name` (static member), if the callee is one.
fn member_name<'a>(e: &'a Expression<'a>) -> Option<&'a str> {
    match e.without_parentheses() {
        Expression::StaticMemberExpression(m) => Some(m.property.name.as_str()),
        _ => None,
    }
}

fn function_start(e: &Expression<'_>) -> Option<u32> {
    match e.without_parentheses() {
        Expression::FunctionExpression(f) => Some(f.span.start),
        Expression::ArrowFunctionExpression(f) => Some(f.span.start),
        _ => None,
    }
}

/// Is the callee a registrar (`x.on(…)`, `setInterval(…)`, `createServer(…)`)?
fn is_registrar(callee: &Expression<'_>) -> bool {
    match callee.without_parentheses() {
        Expression::Identifier(id) => matches!(id.name.as_str(), "setInterval" | "createServer" | "serve"),
        Expression::StaticMemberExpression(m) => {
            let n = m.property.name.as_str();
            REGISTRARS.contains(&n) || n.starts_with("subscribe")
        }
        _ => false,
    }
}

struct Prepass<'s> {
    scoping: &'s Scoping,
    handler_spans: HashSet<u32>,
    handler_symbols: HashSet<SymbolId>,
}

impl Prepass<'_> {
    fn mark(&mut self, e: &Expression<'_>) {
        if let Some(s) = function_start(e) {
            self.handler_spans.insert(s);
        } else if let Expression::Identifier(id) = e.without_parentheses() {
            if let Some(sym) = self.scoping.get_reference(id.reference_id()).symbol_id() {
                self.handler_symbols.insert(sym);
            }
        }
    }

    fn mark_object_methods(&mut self, o: &ObjectExpression<'_>) {
        for p in &o.properties {
            if let ObjectPropertyKind::ObjectProperty(p) = p {
                if p.key.static_name().is_some_and(|n| EXPORTED_HANDLERS.contains(&n.as_ref())) {
                    self.mark(&p.value);
                }
            }
        }
    }
}

impl<'a> Visit<'a> for Prepass<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if is_registrar(&it.callee) {
            for a in &it.arguments {
                if let Some(e) = a.as_expression() {
                    self.mark(e);
                    // `Bun.serve({fetch(req) {}})`, `Deno.serve({handler})`
                    if let Expression::ObjectExpression(o) = e.without_parentheses() {
                        self.mark_object_methods(o);
                    }
                }
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_export_named_declaration(&mut self, it: &ExportNamedDeclaration<'a>) {
        match &it.declaration {
            Some(Declaration::FunctionDeclaration(f)) => {
                if f.id.as_ref().is_some_and(|id| EXPORTED_HANDLERS.contains(&id.name.as_str())) {
                    self.handler_spans.insert(f.span.start);
                }
            }
            Some(Declaration::VariableDeclaration(v)) => {
                for d in &v.declarations {
                    let named = d.id.get_binding_identifier().is_some_and(|b| EXPORTED_HANDLERS.contains(&b.name.as_str()));
                    if let (true, Some(init)) = (named, &d.init) {
                        self.mark(init);
                    }
                }
            }
            _ => {}
        }
        walk::walk_export_named_declaration(self, it);
    }

    fn visit_export_default_declaration(&mut self, it: &ExportDefaultDeclaration<'a>) {
        match &it.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                self.handler_spans.insert(f.span.start);
            }
            kind => {
                if let Some(e) = kind.as_expression() {
                    self.mark(e);
                    if let Expression::ObjectExpression(o) = e.without_parentheses() {
                        self.mark_object_methods(o);
                    }
                }
            }
        }
        walk::walk_export_default_declaration(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        // `x.onmessage = fn`, `module.exports = fn`
        if let Some(m) = it.left.as_member_expression() {
            let on = m.static_property_name().is_some_and(|n| n.len() > 2 && n.starts_with("on"));
            let exports = m.object().is_specific_id("module") && m.static_property_name() == Some("exports");
            if on || exports {
                self.mark(&it.right);
            }
        }
        walk::walk_assignment_expression(self, it);
    }
}

/// What a module-level collection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coll {
    Map,
    Set,
    Array,
    Object,
}

impl Coll {
    fn word(self) -> &'static str {
        match self {
            Coll::Map => "Map",
            Coll::Set => "Set",
            Coll::Array => "array",
            Coll::Object => "object",
        }
    }
}

struct Lint<'s> {
    scoping: &'s Scoping,
    surface: Surface,
    module_scopes: HashSet<ScopeId>,
    handler_spans: HashSet<u32>,
    handler_symbols: HashSet<SymbolId>,
    /// > 0 inside a handler (or anything nested in one).
    depth: u32,
    /// Starts of calls whose value is thrown away (`f();` as a statement).
    discarded: HashSet<u32>,
    out: Vec<Finding>,
    /// Widget rules fire once per file each (UW10's "one per pattern").
    widget_seen: BTreeSet<&'static str>,
    collections: BTreeMap<SymbolId, (String, Coll)>,
    /// First growth inside a handler: symbol → (offset, how).
    growth: BTreeMap<SymbolId, (u32, String)>,
    shrunk: HashSet<SymbolId>,
    /// `setInterval` in a handler: (offset, value thrown away).
    intervals: Vec<(u32, bool)>,
    clears_interval: bool,
    /// Listener added in a handler: (offset, target, method, target symbol).
    listens: Vec<(u32, String, String, Option<SymbolId>)>,
    unlistened: HashSet<String>,
    /// Local names of `express-session`'s default export.
    session_fns: HashSet<String>,
}

impl Lint<'_> {
    fn push(&mut self, offset: u32, rule: &'static str, message: impl Into<String>, suggestion: impl Into<String>) {
        self.out.push(Finding { offset, severity: Severity::Warning, rule, message: message.into(), suggestion: suggestion.into() });
    }

    fn widget_once(&mut self, offset: u32, key: &'static str, rule: &'static str, message: String, suggestion: &str) {
        if self.surface != Surface::Widget || !self.widget_seen.insert(key) {
            return;
        }
        self.push(offset, rule, message, suggestion);
    }

    /// The symbol an identifier reference resolves to (`None` = a global).
    fn symbol_of(&self, id: &IdentifierReference<'_>) -> Option<SymbolId> {
        self.scoping.get_reference(id.reference_id()).symbol_id()
    }

    fn is_global(&self, id: &IdentifierReference<'_>, name: &str) -> bool {
        id.name.as_str() == name && self.symbol_of(id).is_none()
    }

    /// `name`, `window.name`, `globalThis.name` or `self.name` as an
    /// unresolved global.
    fn is_global_ref(&self, e: &Expression<'_>, name: &str) -> bool {
        match e.without_parentheses() {
            Expression::Identifier(id) => self.is_global(id, name),
            Expression::StaticMemberExpression(m) => {
                m.property.name.as_str() == name
                    && matches!(m.object.without_parentheses(), Expression::Identifier(o)
                        if ["window", "globalThis", "self"].iter().any(|g| self.is_global(o, g)))
            }
            _ => false,
        }
    }

    fn in_handler(&self) -> bool {
        self.depth > 0
    }

    fn finish(&mut self) {
        // Collections that grow in a handler and never shrink.
        let growth = std::mem::take(&mut self.growth);
        for (sym, (offset, how)) in growth {
            let Some((name, coll)) = self.collections.get(&sym).cloned() else { continue };
            let reassigned = self.scoping.get_resolved_references(sym).any(|r| r.is_write());
            if self.shrunk.contains(&sym) || reassigned {
                continue;
            }
            let session = name.to_ascii_lowercase().contains("sess");
            if session {
                self.push(
                    offset,
                    "leak-session-store",
                    format!("'{name}' is an in-memory session store with no expiry: each login adds an entry ({how}) and nothing ever removes one"),
                    "expire sessions (keep an expiry time and sweep, delete on logout), or keep sessions in a persistent store instead of memory",
                );
            } else {
                self.push(
                    offset,
                    "leak-growth",
                    format!(
                        "'{name}' is a module-level {} that grows inside a handler ({how}) and nothing ever removes entries, so memory grows until the process restarts",
                        coll.word()
                    ),
                    "remove entries when they're done (on close, logout or expiry), cap the size (drop the oldest), or keep the data in the store instead of memory",
                );
            }
        }
        let has_clear = self.clears_interval;
        for (offset, discarded) in std::mem::take(&mut self.intervals) {
            if discarded || !has_clear {
                self.push(
                    offset,
                    "leak-interval",
                    "setInterval inside a handler starts another timer every time the handler runs, and nothing clears it",
                    "keep the id and clearInterval it when the request, connection or subscription ends, or start one timer at load instead",
                );
            }
        }
        for (offset, target, method, sym) in std::mem::take(&mut self.listens) {
            let long_lived = match sym {
                Some(s) => self.module_scopes.contains(&self.scoping.symbol_scope_id(s)),
                None => LONG_LIVED_GLOBALS.contains(&target.as_str()),
            };
            if !long_lived || self.unlistened.contains(&target) {
                continue;
            }
            self.push(
                offset,
                "leak-listener",
                format!("{target}.{method}(…) inside a handler adds another listener to '{target}' every time the handler runs, and none is ever removed"),
                format!("add the listener once at load, or remove it when the handler is done ({target}.removeEventListener / .off)"),
            );
        }
    }

    /// A module-level `const x = new Map()` (or `[]`, `{}`, `new Set()`).
    fn note_collection(&mut self, d: &VariableDeclarator<'_>) {
        let (Some(b), Some(init)) = (d.id.get_binding_identifier(), &d.init) else { return };
        let sym = b.symbol_id();
        if !self.module_scopes.contains(&self.scoping.symbol_scope_id(sym)) {
            return;
        }
        let coll = match init.without_parentheses() {
            Expression::NewExpression(n) => match n.callee.without_parentheses() {
                Expression::Identifier(id) if self.is_global(id, "Map") => Some(Coll::Map),
                Expression::Identifier(id) if self.is_global(id, "Set") => Some(Coll::Set),
                Expression::Identifier(id) if self.is_global(id, "Array") => Some(Coll::Array),
                _ => None,
            },
            Expression::ArrayExpression(_) => Some(Coll::Array),
            Expression::ObjectExpression(o) if o.properties.is_empty() => Some(Coll::Object),
            Expression::CallExpression(c)
                if c.callee.is_specific_member_access("Object", "create")
                    && c.arguments.first().and_then(|a| a.as_expression()).is_some_and(|e| matches!(e, Expression::NullLiteral(_))) =>
            {
                Some(Coll::Object)
            }
            _ => None,
        };
        if let Some(c) = coll {
            self.collections.insert(sym, (b.name.to_string(), c));
        }
    }

    fn collection_symbol(&self, e: &Expression<'_>) -> Option<SymbolId> {
        match e.without_parentheses() {
            Expression::Identifier(id) => self.symbol_of(id),
            _ => None,
        }
    }

    fn grow(&mut self, sym: SymbolId, offset: u32, how: String) {
        if self.in_handler() {
            self.growth.entry(sym).or_insert((offset, how));
        }
    }

    // ── the catalog lint (widgets) ───────────────────────────────────────

    fn catalog_call(&mut self, call: &CallExpression<'_>) {
        if self.surface != Surface::Widget {
            return;
        }
        let Expression::StaticMemberExpression(outer) = call.callee.without_parentheses() else { return };
        let n_args = call.arguments.len();
        let spread = call.arguments.iter().any(|a| matches!(a, Argument::SpreadElement(_)));
        // `k2.<runtime>(…)`: `k2.call('x.y', …)`, `k2.subscribe('x.y', cb)`, `k2.can('x.y')`.
        if let Expression::Identifier(root) = outer.object.without_parentheses() {
            if !self.is_global(root, "k2") {
                return;
            }
            let member = outer.property.name.as_str();
            if let Some((_, verb)) = CALLABLE_SURFACES.iter().find(|(m, _)| *m == member) {
                self.check_verb(outer.property.span.start, verb, &format!("k2.{member}()"), None, n_args, spread);
                return;
            }
            if !matches!(member, "call" | "subscribe" | "on" | "can") {
                if !K2_RUNTIME_MEMBERS.contains(&member) {
                    let names = K2_RUNTIME_MEMBERS.join(", ");
                    self.push(
                        outer.property.span.start,
                        "catalog-unknown",
                        format!("k2.{member}(…) isn't part of the widget API"),
                        format!("verbs are k2.<surface>.<action>(…); the k2 object itself has {names} (k2 zen guide api)"),
                    );
                }
                return;
            }
            let Some(Expression::StringLiteral(s)) = call.arguments.first().and_then(|a| a.as_expression()) else { return };
            let verb = s.value.as_str().to_string();
            let rest = n_args.saturating_sub(1);
            let as_call = if member == "can" { None } else { Some(member == "call") };
            self.check_verb(s.span.start, &verb, &format!("k2.{member}('{verb}')"), as_call, rest, spread);
            return;
        }
        // `k2.<surface>.<action>(…)`
        let Expression::StaticMemberExpression(inner) = outer.object.without_parentheses() else { return };
        let Expression::Identifier(root) = inner.object.without_parentheses() else { return };
        if !self.is_global(root, "k2") {
            return;
        }
        let surface = inner.property.name.as_str();
        if K2_RUNTIME_MEMBERS.contains(&surface) {
            return;
        }
        let verb = format!("{surface}.{}", outer.property.name.as_str());
        if FRAME_HELPERS.contains(&verb.as_str()) {
            return;
        }
        self.check_verb(inner.span.start, &verb, &format!("k2.{verb}"), None, n_args, spread);
    }

    /// `as_call`: `Some(true)` for `k2.call`, `Some(false)` for
    /// `k2.subscribe`/`k2.on`, `None` for a named helper (its kind is the
    /// row's) or `k2.can` (no arity).
    fn check_verb(&mut self, offset: u32, verb: &str, shown: &str, as_call: Option<bool>, n: usize, spread: bool) {
        let cat = contract::catalog();
        let exposed: Vec<&str> = cat.widget_callable_verbs().iter().map(|v| v.verb.as_str()).collect();
        let Some(row) = cat.verb(verb).filter(|r| widget_callable(r)) else {
            let (msg, sug) = if cat.verb(verb).is_some() {
                (
                    format!("{shown}: '{verb}' isn't open to widgets (only K2's own pages can use it), so it's undefined in a widget"),
                    "k2 zen guide api lists every verb a widget can call".to_string(),
                )
            } else {
                let near = nearest(verb, &exposed);
                (
                    format!("{shown}: there's no verb '{verb}' in K2's catalog, so it's undefined in a widget"),
                    match near {
                        Some(v) => format!("did you mean {v}? (k2 zen guide api lists every verb)"),
                        None => "k2 zen guide api lists every verb a widget can call".to_string(),
                    },
                )
            };
            self.push(offset, "catalog-unknown", msg, sug);
            return;
        };
        if spread {
            return;
        }
        let items = contract::lint::request_items(row);
        let total = items.len();
        let required = items.iter().filter(|a| a.get("default").is_none()).count();
        let names: Vec<String> = items.iter().map(|a| a.get("title").and_then(|t| t.as_str()).unwrap_or("arg").to_string()).collect();
        let sig = contract::gen::signature(row);
        let is_call = row.kind == VerbKind::Call;
        match as_call {
            None if shown.starts_with("k2.can") => {}
            Some(c) if c != is_call => {
                let (want, got) = if is_call { ("k2.call", "k2.subscribe") } else { ("k2.subscribe", "k2.call") };
                self.push(
                    offset,
                    "catalog-kind",
                    format!("{shown}: '{verb}' is a {} verb, not something for {got}", if is_call { "call" } else { "subscribe" }),
                    format!("use {want}('{verb}', …) or k2.{verb}(…); it reads {sig}"),
                );
            }
            _ if is_call => {
                if n > total || n < required {
                    let need = if n < required { format!("it needs {}", names[..required].join(", ")) } else { format!("it takes at most {total}") };
                    self.push(
                        offset,
                        "catalog-arity",
                        format!("{shown} is called with {n} argument{}; {need}", if n == 1 { "" } else { "s" }),
                        format!("call it as {sig} (k2 zen guide api)"),
                    );
                }
            }
            _ => {
                // Subscriptions: the request arguments, then cb and an optional onError.
                if n < required + 1 || n > total + 2 {
                    let need = if n < required + 1 { "it needs its arguments and then a callback".to_string() } else { format!("it takes at most {} plus a callback and onError", total) };
                    self.push(
                        offset,
                        "catalog-arity",
                        format!("{shown} is called with {n} argument{}; {need}", if n == 1 { "" } else { "s" }),
                        format!("call it as {sig} (k2 zen guide api)"),
                    );
                }
            }
        }
    }
}

/// The closest name by edit distance (at most 3 edits).
fn nearest<'v>(want: &str, names: &[&'v str]) -> Option<&'v str> {
    names.iter().map(|n| (edit_distance(want, n), *n)).filter(|(d, _)| *d <= 3).min_by_key(|(d, _)| *d).map(|(_, n)| n)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

const TEXT_CONTENT: &str = "if this shows agent or ticket text, use textContent";
const NO_NETWORK: &str = "blocked: widgets have no network in this version";
const NO_STORAGE: &str = "not available in a sealed widget";
const CSP: &str = "blocked by the widget's security policy";

impl<'a> Visit<'a> for Lint<'_> {
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        let handler = self.handler_spans.contains(&it.span.start)
            || it.id.as_ref().is_some_and(|id| self.handler_symbols.contains(&id.symbol_id()));
        self.depth += u32::from(handler);
        walk::walk_function(self, it, flags);
        self.depth -= u32::from(handler);
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let handler = self.handler_spans.contains(&it.span.start);
        self.depth += u32::from(handler);
        walk::walk_arrow_function_expression(self, it);
        self.depth -= u32::from(handler);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        self.note_collection(it);
        if let (Some(b), Some(init)) = (it.id.get_binding_identifier(), &it.init) {
            if self.handler_symbols.contains(&b.symbol_id()) {
                if let Some(s) = function_start(init) {
                    self.handler_spans.insert(s);
                }
            }
            // `const session = require('express-session')`
            if let Expression::CallExpression(c) = init.without_parentheses() {
                let req = matches!(c.callee.without_parentheses(), Expression::Identifier(id) if self.is_global(id, "require"));
                let from = c.arguments.first().and_then(|a| a.as_expression());
                if req && matches!(from, Some(Expression::StringLiteral(s)) if s.value.as_str() == "express-session") {
                    self.session_fns.insert(b.name.to_string());
                }
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if it.source.value.as_str() == "express-session" {
            for s in it.specifiers.iter().flatten() {
                match s {
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(d) => {
                        self.session_fns.insert(d.local.name.to_string());
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(d) => {
                        self.session_fns.insert(d.local.name.to_string());
                    }
                    ImportDeclarationSpecifier::ImportSpecifier(_) => {}
                }
            }
        }
        if self.surface == Surface::Widget {
            self.widget_once(
                it.span.start,
                "import",
                "widget-import",
                "import: one script per file; imports between files don't load (list each file in a <script src>)".into(),
                "list each file in its own <script src=\"file.js\"> in the order they need each other",
            );
        }
        walk::walk_import_declaration(self, it);
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        if let Expression::CallExpression(c) = it.expression.without_parentheses() {
            self.discarded.insert(c.span.start);
        }
        walk::walk_expression_statement(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.catalog_call(it);
        let callee = it.callee.without_parentheses();
        let prop = member_name(callee);

        // Widget rules.
        if prop == Some("insertAdjacentHTML") {
            self.widget_once(it.span.start, "insertAdjacentHTML", "widget-html", format!("insertAdjacentHTML: {TEXT_CONTENT}"), "set textContent, or build elements with createElement");
        }
        if let Expression::StaticMemberExpression(m) = callee {
            if matches!(m.property.name.as_str(), "write" | "writeln") && matches!(m.object.without_parentheses(), Expression::Identifier(d) if self.is_global(d, "document")) {
                self.widget_once(it.span.start, "document.write", "widget-html", format!("document.write: {TEXT_CONTENT}"), "set textContent, or build elements with createElement");
            }
        }
        if self.is_global_ref(callee, "fetch") {
            self.widget_once(it.span.start, "fetch", "widget-network", format!("fetch: {NO_NETWORK}"), "get data through a k2 verb (k2 zen guide api), or put the file in the widget's folder");
        }
        if matches!(callee, Expression::Identifier(id) if self.is_global(id, "eval")) {
            self.widget_once(it.span.start, "eval", "widget-csp", format!("eval: {CSP}"), "write the code directly; widgets can't build code from strings");
        }
        if matches!(callee, Expression::Identifier(id) if self.is_global(id, "Function")) {
            self.widget_once(it.span.start, "new Function", "widget-csp", format!("new Function: {CSP}"), "write the code directly; widgets can't build code from strings");
        }

        // Leak lints.
        if self.is_global_ref(callee, "clearInterval") {
            self.clears_interval = true;
        }
        if self.in_handler() && self.is_global_ref(callee, "setInterval") {
            let discarded = self.discarded.contains(&it.span.start);
            self.intervals.push((it.span.start, discarded));
        }
        if let Expression::StaticMemberExpression(m) = callee {
            let method = m.property.name.as_str();
            if let Some(sym) = self.collection_symbol(&m.object) {
                match method {
                    "set" | "add" | "push" | "unshift" => self.grow(sym, it.span.start, format!(".{method}")),
                    "delete" | "clear" | "pop" | "shift" | "splice" => {
                        self.shrunk.insert(sym);
                    }
                    _ => {}
                }
            }
            if UNLISTEN_METHODS.contains(&method) {
                if let Some(t) = target_name(&m.object) {
                    self.unlistened.insert(t);
                }
            }
            if self.in_handler() && LISTEN_METHODS.contains(&method) && !once_option(it) {
                if let Some(t) = target_name(&m.object) {
                    let sym = self.collection_symbol(&m.object);
                    self.listens.push((it.span.start, t, method.to_string(), sym));
                }
            }
            // `k2.x.subscribe(cb)` again on every event.
            let k2_sub = method.starts_with("subscribe") || method == "changed" || method == "on";
            if self.in_handler() && k2_sub && rooted_at_global_k2(self, &m.object) && self.discarded.contains(&it.span.start) {
                self.push(
                    it.span.start,
                    "leak-listener",
                    "a k2 subscription made inside a handler subscribes again every time the handler runs".to_string(),
                    "subscribe once at load (inside k2.connected.then), or keep the returned unsubscribe and call it",
                );
            }
        }
        // `session({ secret })` from express-session, with no store.
        if let Expression::Identifier(id) = callee {
            if self.session_fns.contains(id.name.as_str()) {
                if let Some(Expression::ObjectExpression(o)) = it.arguments.first().and_then(|a| a.as_expression()) {
                    let has_store = o.properties.iter().any(|p| matches!(p, ObjectPropertyKind::ObjectProperty(p) if p.key.static_name().is_some_and(|n| n == "store")));
                    if !has_store {
                        self.push(
                            it.span.start,
                            "leak-session-store",
                            "express-session with no store keeps every session in its MemoryStore, which never frees expired sessions",
                            "pass a store with expiry (store: …), e.g. a database- or file-backed store",
                        );
                    }
                }
            }
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let callee = it.callee.without_parentheses();
        if matches!(callee, Expression::Identifier(id) if self.is_global(id, "Function")) {
            self.widget_once(it.span.start, "new Function", "widget-csp", format!("new Function: {CSP}"), "write the code directly; widgets can't build code from strings");
        }
        let memory_store = match callee {
            Expression::Identifier(id) => id.name.as_str() == "MemoryStore",
            Expression::StaticMemberExpression(m) => m.property.name.as_str() == "MemoryStore",
            _ => false,
        };
        if memory_store {
            self.push(
                it.span.start,
                "leak-session-store",
                "a MemoryStore keeps every session in this process's memory and never frees expired ones",
                "use a store with expiry (database- or file-backed), not MemoryStore",
            );
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let Some(m) = it.left.as_member_expression() {
            if let Some(name) = m.static_property_name() {
                if name == "innerHTML" || name == "outerHTML" {
                    let key = if name == "innerHTML" { "innerHTML" } else { "outerHTML" };
                    self.widget_once(m.span().start, key, "widget-html", format!("{name}: {TEXT_CONTENT}"), "set textContent, or build elements with createElement");
                }
            }
            if let Some(sym) = self.collection_symbol(m.object()) {
                if m.static_property_name() == Some("length") {
                    self.shrunk.insert(sym);
                } else if matches!(m, MemberExpression::ComputedMemberExpression(_)) {
                    self.grow(sym, m.span().start, "[key] = …".into());
                } else if let Some(p) = m.static_property_name() {
                    if self.collections.get(&sym).is_some_and(|(_, c)| *c == Coll::Object) {
                        self.grow(sym, m.span().start, format!(".{p} = …"));
                    }
                }
            }
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == UnaryOperator::Delete {
            if let Some(m) = it.argument.as_member_expression() {
                if let Some(sym) = self.collection_symbol(m.object()) {
                    self.shrunk.insert(sym);
                }
            }
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.symbol_of(it).is_none() {
            let (key, msg, sug): (&'static str, &str, &str) = match it.name.as_str() {
                "XMLHttpRequest" => ("XMLHttpRequest", NO_NETWORK, "get data through a k2 verb (k2 zen guide api)"),
                "WebSocket" => ("WebSocket", NO_NETWORK, "get live data through a k2 subscription (k2 zen guide api)"),
                "EventSource" => ("EventSource", NO_NETWORK, "get live data through a k2 subscription (k2 zen guide api)"),
                "localStorage" => ("localStorage", NO_STORAGE, "keep state in memory, or in the widget's config"),
                "sessionStorage" => ("sessionStorage", NO_STORAGE, "keep state in memory, or in the widget's config"),
                "indexedDB" => ("indexedDB", NO_STORAGE, "keep state in memory, or in the widget's config"),
                _ => ("", "", ""),
            };
            if !key.is_empty() {
                self.widget_once(it.span.start, key, if msg == NO_NETWORK { "widget-network" } else { "widget-storage" }, format!("{key}: {msg}"), sug);
            }
        }
        walk::walk_identifier_reference(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        // `window.localStorage` (the bare name is caught above).
        let name = it.property.name.as_str();
        let on_global = matches!(it.object.without_parentheses(), Expression::Identifier(o)
            if ["window", "globalThis", "self"].iter().any(|g| self.is_global(o, g)));
        if on_global {
            let (key, rule, msg, sug): (&'static str, &'static str, &str, &str) = match name {
                "localStorage" => ("localStorage", "widget-storage", NO_STORAGE, "keep state in memory, or in the widget's config"),
                "sessionStorage" => ("sessionStorage", "widget-storage", NO_STORAGE, "keep state in memory, or in the widget's config"),
                "indexedDB" => ("indexedDB", "widget-storage", NO_STORAGE, "keep state in memory, or in the widget's config"),
                "XMLHttpRequest" => ("XMLHttpRequest", "widget-network", NO_NETWORK, "get data through a k2 verb (k2 zen guide api)"),
                "WebSocket" => ("WebSocket", "widget-network", NO_NETWORK, "get live data through a k2 subscription (k2 zen guide api)"),
                "EventSource" => ("EventSource", "widget-network", NO_NETWORK, "get live data through a k2 subscription (k2 zen guide api)"),
                _ => ("", "", "", ""),
            };
            if !key.is_empty() {
                self.widget_once(it.span.start, key, rule, format!("{key}: {msg}"), sug);
            }
        }
        walk::walk_static_member_expression(self, it);
    }
}

/// `{ once: true }` as the third argument of addEventListener.
fn once_option(call: &CallExpression<'_>) -> bool {
    let Some(Expression::ObjectExpression(o)) = call.arguments.get(2).and_then(|a| a.as_expression()) else { return false };
    o.properties.iter().any(|p| {
        matches!(p, ObjectPropertyKind::ObjectProperty(p)
            if p.key.static_name().is_some_and(|n| n == "once") && matches!(p.value, Expression::BooleanLiteral(ref b) if b.value))
    })
}

/// A listener target as text: `window`, `bus`, `process.stdin`.
fn target_name(e: &Expression<'_>) -> Option<String> {
    match e.without_parentheses() {
        Expression::Identifier(id) => Some(id.name.to_string()),
        Expression::StaticMemberExpression(m) => Some(format!("{}.{}", target_name(&m.object)?, m.property.name)),
        Expression::ThisExpression(_) => Some("this".into()),
        _ => None,
    }
}

/// Is `e` a member chain rooted at the global `k2`?
fn rooted_at_global_k2(l: &Lint<'_>, e: &Expression<'_>) -> bool {
    match e.without_parentheses() {
        Expression::Identifier(id) => l.is_global(id, "k2"),
        Expression::StaticMemberExpression(m) => rooted_at_global_k2(l, &m.object),
        _ => false,
    }
}
