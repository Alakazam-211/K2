//! The VALIDATE stage's JS/TS check (decision-k2-runtime-v1 §2.2, §5 and
//! stage 0 (c); prd-k2-app-runtime-v1 S4 and §6.8).
//!
//! Before this, `k2 zen validate` never parsed a widget's JavaScript: it ran
//! a substring scan (`bundle::scan_js`, now removed). This module
//! parses with **oxc** (MIT, linked in-process, pinned exactly) and runs:
//!
//! - **parse + early errors** — the parser's syntax errors plus oxc's
//!   semantic early errors (`let x; let x;`, `break` outside a loop, …): the
//!   things a browser throws as `SyntaxError`, which stop the whole script.
//!   These are **errors**.
//! - **widget rules** (Garden widgets only) — the AST versions of the 13
//!   substring warnings (`innerHTML` writes, network, storage, `eval`, …),
//!   so a word in a string or comment no longer warns. Warnings.
//! - **catalog lint** (Garden widgets only) — `k2.<surface>.<action>(…)`
//!   calls checked against the verb catalog (`contract/catalog.json`):
//!   unknown verbs, verbs not open to widgets, and argument counts.
//!   Warnings, never errors: an installed widget is never stopped by them.
//! - **leak lints** (widgets and Apps, §6.8) — module-level `Map`/`Set`/
//!   array/object growth inside handlers, `setInterval` started in a handler
//!   with no clear, listeners added to long-lived targets per request or
//!   event, and in-memory session stores with no expiry. Warnings.
//!
//! Every finding has a byte offset in the text it was given, a message and
//! a `suggestion` line (lakebed-style agent errors): what to do next.
//! Callers turn offsets into `file:line:col` (`line_col`).
//!
//! The type stage (tsc-rs as a subprocess, never linked) comes with `--types`.

mod lint;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

/// How bad a finding is. Errors stop a Garden widget version (the last
/// good one stays live) and fail `validate`; warnings never block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One finding in one source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Byte offset in the checked text.
    pub offset: u32,
    pub severity: Severity,
    /// Stable rule id (`parse`, `widget-network`, `catalog-unknown`,
    /// `leak-growth`, …), for tests and JSON.
    pub rule: &'static str,
    pub message: String,
    /// What to do about it, in one line.
    pub suggestion: String,
}

/// Who runs the code: the rules differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A custom Garden widget in its sealed frame.
    Widget,
    /// An App's code (pages or server).
    App,
}

/// How to read a source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// A classic `<script>` (sloppy unless it says `"use strict"`).
    Script,
    /// `<script type="module">` or `.mjs`.
    Module,
    /// `.js` / `.jsx` in an App: a module when it imports or exports,
    /// else a script; JSX allowed.
    Unambiguous,
    /// `.cjs`.
    CommonJs,
    /// `.ts` / `.mts` / `.cts`.
    TypeScript,
    /// `.tsx`.
    Tsx,
}

impl Lang {
    /// The language of an App file by name; `None` for files this check
    /// doesn't read (`.d.ts`, `.min.js`, non-code).
    pub fn for_file(name: &str) -> Option<Lang> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".d.ts") || lower.ends_with(".d.mts") || lower.ends_with(".d.cts") || lower.ends_with(".min.js") {
            return None;
        }
        let ext = lower.rsplit_once('.')?.1;
        Some(match ext {
            "js" | "jsx" => Lang::Unambiguous,
            "mjs" => Lang::Module,
            "cjs" => Lang::CommonJs,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            _ => return None,
        })
    }

    fn source_type(self) -> SourceType {
        match self {
            Lang::Script => SourceType::script(),
            Lang::Module => SourceType::mjs(),
            Lang::Unambiguous => SourceType::unambiguous().with_jsx(true),
            Lang::CommonJs => SourceType::cjs(),
            Lang::TypeScript => SourceType::ts(),
            Lang::Tsx => SourceType::tsx(),
        }
    }
}

/// The `type` of a `<script>` element → how to read its body, or `None`
/// when it isn't JavaScript (`application/json`, `text/template`,
/// `importmap`, …) and is never run as code.
pub fn script_lang(type_attr: Option<&str>) -> Option<Lang> {
    let Some(t) = type_attr else { return Some(Lang::Script) };
    let t = t.trim().to_ascii_lowercase();
    let t = t.split(';').next().unwrap_or("").trim().to_string();
    match t.as_str() {
        "" | "text/javascript" | "application/javascript" | "application/ecmascript" | "text/ecmascript"
        | "text/jscript" | "application/x-javascript" => Some(Lang::Script),
        "module" => Some(Lang::Module),
        _ => None,
    }
}

const PARSE_SUGGESTION: &str =
    "this file doesn't parse, so none of it runs: fix the code at this line, then validate again";

/// Parse `text` and run every rule for `surface`. Parse errors come back
/// alone (lints on a broken tree would only add noise).
pub fn check(text: &str, lang: Lang, surface: Surface) -> Vec<Finding> {
    let alloc = Allocator::default();
    let parsed = Parser::new(&alloc, text, lang.source_type()).parse();
    let mut errors: Vec<Finding> = parsed.diagnostics.errors().map(|d| from_oxc(d, text)).collect();
    if parsed.panicked || !errors.is_empty() {
        return import_hint(text, lang, errors);
    }
    let sem = SemanticBuilder::new().with_check_syntax_error(true).build(&parsed.program);
    errors.extend(sem.diagnostics.errors().map(|d| from_oxc(d, text)));
    if !errors.is_empty() {
        return import_hint(text, lang, errors);
    }
    lint::run(&parsed.program, sem.semantic.scoping(), surface)
}

/// A classic script that fails only because it uses `import`/`export`:
/// say that (UW10's message), not the parser's generic one.
fn import_hint(text: &str, lang: Lang, errors: Vec<Finding>) -> Vec<Finding> {
    if lang != Lang::Script {
        return errors;
    }
    let alloc = Allocator::default();
    let module = Parser::new(&alloc, text, SourceType::mjs()).parse();
    if module.panicked || module.diagnostics.has_errors() {
        return errors;
    }
    let first = module.program.body.iter().find(|s| s.is_module_declaration());
    match first {
        Some(stmt) => {
            use oxc_span::GetSpan;
            vec![Finding {
                offset: stmt.span().start,
                severity: Severity::Error,
                rule: "parse-import",
                message: "import/export only work in <script type=\"module\">, and a widget's files can't import each other"
                    .into(),
                suggestion: "list each file in its own <script src=\"file.js\"> in order, and share values through globals"
                    .into(),
            }]
        }
        None => errors,
    }
}

fn from_oxc(d: &oxc_diagnostics::OxcDiagnostic, text: &str) -> Finding {
    // The primary label, else the latest one: with two (a redeclaration),
    // the first points at the earlier, valid declaration.
    let labels = d.labels.as_slice();
    let at = labels.iter().find(|l| l.primary()).or_else(|| labels.iter().max_by_key(|l| l.offset()));
    let offset = at.map(|l| l.offset()).unwrap_or(0).min(text.len() as u32);
    let mut message = d.message.to_string();
    if let Some(note) = at.and_then(|l| l.label()) {
        if !note.is_empty() && !message.contains(note) {
            message = format!("{message} ({note})");
        }
    }
    let suggestion = match &d.help {
        Some(h) => format!("{h}; {PARSE_SUGGESTION}"),
        None => PARSE_SUGGESTION.to_string(),
    };
    Finding { offset, severity: Severity::Error, rule: "parse", message, suggestion }
}

/// 1-based `(line, col)` of byte `off` in `text` (col counts chars).
pub fn line_col(text: &str, off: usize) -> (usize, usize) {
    let mut off = off.min(text.len());
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    let before = &text[..off];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[start..].chars().count() + 1)
}

/// `(line, col)` of `off` in a text that starts at `base` in its file (an
/// inline `<script>` body).
pub fn line_col_from(text: &str, off: usize, base: (usize, usize)) -> (usize, usize) {
    let (l, c) = line_col(text, off);
    if l == 1 { (base.0, base.1 + c - 1) } else { (base.0 + l - 1, c) }
}

#[cfg(test)]
mod tests;
