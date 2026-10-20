//! Runtime errors from sealed widget frames reach agents
//! (decision-k2-runtime-v1 stage 0 (d)). Before this, a frame's uncaught
//! errors went to the window's `console.warn` only (`zen-custom-host.ts`).
//!
//! The renderer posts each frame's `ready` and each uncaught error or
//! rejection (`POST /cli/zen/widget/run`) with the bundle hash it was
//! running. The daemon keeps a small log per user widget in its history
//! folder (`.history/widgets/<name>/runtime.jsonl`, the newest
//! [`KEEP`] events; no migration). Agents read it with `k2 zen widget errors
//! <name> [--since] [--follow]`, and `k2 zen validate` says how many runtime
//! errors the current version (hash) has hit since it was saved. Built-in
//! widgets are K2's own and aren't logged.
//!
//! **Garden v3 (contract-sources-v1 §11.4, GV49).** A report may also carry
//! `kind` (`error`, or `refusal` for a bridge refusal such as `not_bound`),
//! `class` (`TypeError`, or the refused verb), `code` (the catalog error
//! code) and `line`. For a widget whose manifest asks for `sources:read`
//! the daemon keeps **only** those fixed fields: never the message or the
//! stack, because a card that reads sources could otherwise throw a value
//! into text agents read.

use std::fs;
use std::io::Write;

use serde_json::{json, Value as J};

use super::{io, now_rfc3339, widget_label};
use crate::zen::builtin_widgets::{parse_widget_ref, WidgetRef};
use crate::zen::store::{ZenError, ZenFiles};

/// Events kept per widget (older ones are dropped when the log doubles).
pub const KEEP: usize = 300;
/// Text kept from an error's message and stack.
pub const MAX_MESSAGE: usize = 500;
pub const MAX_STACK: usize = 2000;
const LOG: &str = "runtime.jsonl";

/// 0.46.1: one writer at a time. A report reads the log for the last
/// `seq`, then appends; two frames erroring at once used to read the same
/// last seq and both log `#n+1`. Process-wide (reports are rare and tiny).
static RUN_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The fixed fields a run report may carry besides its text (GV49).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunDetail<'a> {
    pub message: Option<&'a str>,
    pub stack: Option<&'a str>,
    /// `error` (default) or `refusal`.
    pub kind: Option<&'a str>,
    /// `TypeError`, or the refused verb (`sources.read`).
    pub class: Option<&'a str>,
    /// A catalog error code (`not_bound`).
    pub code: Option<&'a str>,
    pub line: Option<u64>,
    /// 0.46.1 S3: the source address a refused `sources.*` call named. Kept
    /// only when it is one this widget's placements (or its manifest) name,
    /// so it is config, never a value read from a source; otherwise the row
    /// says `addressWithheld`.
    pub address: Option<&'a str>,
}

/// Longest source address kept on a refusal row.
pub const MAX_ADDRESS: usize = 200;

/// A source address's shape (`name` or `name::host`): letters, digits and
/// `- _ . :`, at most [`MAX_ADDRESS`].
fn address_shaped(a: &str) -> bool {
    !a.is_empty() && a.len() <= MAX_ADDRESS && a.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Longest `class` / `code` kept.
pub const MAX_CLASS: usize = 60;

/// `class`: a JS identifier or a dotted verb name; `code`: snake case.
fn fixed_word(s: &str, dots: bool) -> bool {
    !s.is_empty()
        && s.len() <= MAX_CLASS
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || (dots && b == b'.'))
}

/// What a run row says in one line: its message, or for a redacted row
/// (a widget that reads sources) its class, code and line only.
pub fn run_text(r: &J) -> String {
    if let Some(m) = r["message"].as_str() {
        return m.to_string();
    }
    let mut out = String::new();
    if r["kind"] == "refusal" {
        out.push_str("refused");
    } else {
        out.push_str(r["class"].as_str().unwrap_or("error"));
    }
    if r["kind"] == "refusal" {
        if let Some(c) = r["class"].as_str() {
            out.push_str(&format!(" {c}"));
        }
    }
    if let Some(c) = r["code"].as_str() {
        out.push_str(&format!(" ({c})"));
    }
    if let Some(a) = r["address"].as_str() {
        out.push_str(&format!(" for {a}"));
    } else if r["addressWithheld"] == true {
        out.push_str(" for an address no placement of this widget names (withheld)");
    }
    if let Some(l) = r["line"].as_u64() {
        out.push_str(&format!(" at line {l}"));
    }
    if r["redacted"] == true {
        out.push_str(" [text withheld: this widget reads data sources]");
    }
    out
}

/// A placed widget's runtime errors on its saved version fail `k2 zen
/// validate` only while they are this fresh (0.46.3): older ones, and any
/// on a widget no Garden places, are warnings (validate reflects what runs
/// now, not a log from days ago). `k2 zen widget errors <name> --clear`
/// drops the ones that no longer count.
pub const RUNTIME_FRESH_SECS: i64 = 24 * 60 * 60;

/// Why a runtime error on the saved version doesn't fail validate.
pub const STALE_UNPLACED: &str = "unplaced";
pub const STALE_OLD: &str = "old";

/// What validate makes of one widget version's runtime log (0.46.3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeVerdict {
    /// A frame said `ready` for this version.
    pub ran: bool,
    /// Where it is placed: `{garden, gardenName, placement}`.
    pub placements: Vec<J>,
    /// Errors that fail validate: placed, this version, fresh.
    pub failing: Vec<J>,
    /// Errors on this version that don't count (each with `staleReason`).
    pub stale: Vec<J>,
}

impl RuntimeVerdict {
    pub fn placed(&self) -> bool {
        !self.placements.is_empty()
    }
}

/// Seconds since a run row's `at`; `None` when it doesn't parse (such a
/// row counts as fresh: never hide an error on a guess).
fn row_age_secs(r: &J, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(r["at"].as_str()?).ok()?;
    Some((now - at.with_timezone(&chrono::Utc)).num_seconds())
}

/// The reason an error row of the saved version doesn't count, if any.
fn stale_reason(r: &J, placed: bool, now: chrono::DateTime<chrono::Utc>) -> Option<&'static str> {
    if !placed {
        Some(STALE_UNPLACED)
    } else if row_age_secs(r, now).is_some_and(|a| a > RUNTIME_FRESH_SECS) {
        Some(STALE_OLD)
    } else {
        None
    }
}

/// What one widget version (hash) has done at runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    /// Uncaught errors and rejections seen for that hash.
    pub errors: usize,
    /// A frame said `ready` for that hash.
    pub ran: bool,
    /// The newest few errors for that hash (as logged).
    pub last: Vec<J>,
}

fn cut(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

/// What an agent should try for a runtime error message.
pub fn suggestion_for(widget: &str, message: &str) -> String {
    let m = message;
    let validate = format!("k2 zen validate --widget {widget}");
    if let Some(rest) = m.strip_prefix("k2: ") {
        // `k2: <verb> refused (<code>): <message>` from the frame runtime.
        let code = rest.split_once('(').and_then(|(_, r)| r.split_once(')')).map(|(c, _)| c).unwrap_or("");
        return match code {
            "cap_not_granted" => "add the verb's cap to manifest.json \"caps\" (k2 zen guide api names each verb's cap), then save".into(),
            "not_bound" => "use an address this widget reaches (k2.agents.list() lists them; for sources, the placement's config.source / config.sources)".into(),
            "server_not_saved" => "the source's server isn't one of the person's saved servers: ask them to add it in Settings → Network → Connected Servers".into(),
            "unreachable" => "the source's server didn't answer: draw the `last` view greyed and try again later".into(),
            "removed" => "the source was removed on its server: show that, and ask the person which source to use instead".into(),
            "no_history" => "this source keeps states only (keep: states): it has no history to chart".into(),
            "" => "see the code in the message: k2 zen guide api lists every verb's errors".into(),
            c => format!("handle '{c}' with an onError callback; k2 zen guide api explains it"),
        };
    }
    if m.contains("not connected") {
        return "call verbs after k2.connected resolves: k2.connected.then(() => { … })".into();
    }
    if m.contains("SyntaxError") {
        return format!("the script doesn't parse: run {validate} for the line");
    }
    if m.contains("is not defined") {
        return format!("a name is undefined in the frame: check its spelling, and that the file defining it loads first (<script src> order); {validate}");
    }
    if m.contains("is not a function") || m.contains("undefined") || m.contains("null") {
        return "a value was missing when this ran: find the line in the stack, and guard data that arrives later (subscriptions call back with it)".into();
    }
    if m.contains("Content Security Policy") || m.contains("Refused to") {
        return "the sealed frame blocks this (network, eval, inline handlers): use k2 verbs and files in the widget's folder".into();
    }
    format!("find the line in the stack, fix it and save; k2 zen widget errors {widget} --since <seq> shows only new ones")
}

/// The text [`suggestion_for`] words a redacted row from: a refusal as the
/// runtime words it (`k2: <verb> refused (<code>)`), else the class.
fn redacted_hint(kind: &str, code: Option<&str>, text: &str) -> String {
    match (kind, code) {
        ("refusal", Some(c)) => format!("k2: refused ({c})"),
        _ => text.to_string(),
    }
}

impl ZenFiles {
    fn runs_path(&self, name: &str) -> std::path::PathBuf {
        self.widget_history_dir(name).join(LOG)
    }

    fn read_runs(&self, name: &str) -> Vec<J> {
        fs::read_to_string(self.runs_path(name))
            .map(|t| t.lines().filter_map(|l| serde_json::from_str::<J>(l).ok()).collect())
            .unwrap_or_default()
    }

    /// The user widget a run report names, or 400 (built-ins aren't logged).
    fn run_widget(&self, widget: &str) -> Result<String, ZenError> {
        match parse_widget_ref(widget.trim()) {
            Ok(WidgetRef::User(n)) => Ok(n),
            Ok(WidgetRef::Builtin { .. }) => {
                Err(ZenError::BadRequest(format!("'{widget}' is built into K2; its runtime errors aren't logged")))
            }
            Err(m) => Err(ZenError::BadRequest(m)),
        }
    }

    /// POST `/cli/zen/widget/run {widget, hash, event, message?, stack?}`:
    /// log a frame's `ready` (once per hash) or an uncaught error.
    pub fn record_widget_run(
        &self,
        widget: &str,
        hash: &str,
        event: &str,
        message: Option<&str>,
        stack: Option<&str>,
    ) -> Result<J, ZenError> {
        self.record_widget_run_with(widget, hash, event, &RunDetail { message, stack, ..RunDetail::default() })
    }

    /// Whether this widget's manifest (live, or its last good) asks for
    /// `sources:read`: its run reports then keep fixed fields only (GV49).
    pub fn widget_reads_sources(&self, name: &str) -> bool {
        let cap = crate::sources::SOURCES_READ_CAP;
        if self.widget_status(name).requested().iter().any(|c| c == cap) {
            return true;
        }
        self.last_good(name).is_some_and(|(m, _, _)| m.caps.iter().any(|c| c == cap))
    }

    /// [`Self::record_widget_run`] with the GV49 fields.
    pub fn record_widget_run_with(&self, widget: &str, hash: &str, event: &str, d: &RunDetail) -> Result<J, ZenError> {
        let (message, stack) = (d.message, d.stack);
        self.require_set_up()?;
        let name = self.run_widget(widget)?;
        if !self.widget_dir(&name).is_dir() {
            return Err(ZenError::NotFound(format!("{} has no folder", widget_label(&name))));
        }
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ZenError::BadRequest("hash must be the bundle's sha256 (64 hex characters)".into()));
        }
        if event != "ready" && event != "error" {
            return Err(ZenError::BadRequest("event is ready or error".into()));
        }
        // Held from the read of the last seq through the append (or rewrite).
        let _log = RUN_LOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut runs = self.read_runs(&name);
        if event == "ready" && runs.iter().any(|r| r["event"] == "ready" && r["hash"] == hash) {
            return Ok(json!({ "ok": true, "logged": false }));
        }
        let kind = d.kind.unwrap_or("error");
        if kind != "error" && kind != "refusal" {
            return Err(ZenError::BadRequest("kind is error or refusal".into()));
        }
        if d.class.is_some_and(|c| !fixed_word(c, true)) {
            return Err(ZenError::BadRequest(format!("class is a name like TypeError or a verb (at most {MAX_CLASS} characters)")));
        }
        if d.code.is_some_and(|c| !fixed_word(c, false)) {
            return Err(ZenError::BadRequest(format!("code is an error code like not_bound (at most {MAX_CLASS} characters)")));
        }
        let seq = runs.last().and_then(|r| r["seq"].as_u64()).unwrap_or(0) + 1;
        let mut row = json!({ "seq": seq, "at": now_rfc3339(), "event": event, "hash": hash });
        if event == "error" {
            row["kind"] = J::String(kind.into());
            if let Some(c) = d.class {
                row["class"] = J::String(c.into());
            }
            if let Some(c) = d.code {
                row["code"] = J::String(c.into());
            }
            if let Some(l) = d.line {
                row["line"] = J::from(l);
            }
            // 0.46.1 S3: name the refused source address when it is config
            // (one a placement or the manifest names), never otherwise.
            let mut address_hint: Option<String> = None;
            if let (true, Some(a)) = (kind == "refusal", d.address.map(str::trim).filter(|a| !a.is_empty())) {
                let a = a.to_ascii_lowercase();
                if address_shaped(&a) && self.configured_source_addresses(&name).contains(&a) {
                    row["address"] = J::String(a.clone());
                    if d.code == Some("not_bound") {
                        address_hint = Some(format!(
                            "'{a}' is named in this widget's placement config but was refused: the window may have had an older page (K2 re-subscribes when the placement changes; Reload the widget otherwise). Check the source with k2 sources list"
                        ));
                    }
                } else {
                    row["addressWithheld"] = J::Bool(true);
                    if d.code == Some("not_bound") {
                        address_hint = Some(format!(
                            "the widget asked for a source no placement names: add it to the placement's config.sources (k2 zen widget show {name} lists what each placement reads)"
                        ));
                    }
                }
            }
            if self.widget_reads_sources(&name) {
                // GV49: fixed fields only; the message and stack never land.
                row["redacted"] = J::Bool(true);
                let text = run_text(&row);
                row["suggestion"] = J::String(suggestion_for(&name, &redacted_hint(kind, d.code, &text)));
            } else {
                let message = cut(message.unwrap_or("error"), MAX_MESSAGE);
                row["suggestion"] = J::String(suggestion_for(&name, &message));
                row["message"] = J::String(message);
                if let Some(s) = stack.filter(|s| !s.trim().is_empty()) {
                    row["stack"] = J::String(cut(s, MAX_STACK));
                }
            }
            if let Some(h) = address_hint {
                row["suggestion"] = J::String(h);
            }
        }
        let dir = self.widget_history_dir(&name);
        fs::create_dir_all(&dir).map_err(|e| io(e, &dir))?;
        let path = self.runs_path(&name);
        if runs.len() + 1 > KEEP * 2 {
            runs.push(row.clone());
            let keep = &runs[runs.len() - KEEP..];
            let text: String = keep.iter().map(|r| r.to_string() + "\n").collect();
            crate::fs_atomic::atomic_write_str(&path, &text).map_err(|e| io(e, &path))?;
        } else {
            let mut f = fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| io(e, &path))?;
            f.write_all((row.to_string() + "\n").as_bytes()).map_err(|e| io(e, &path))?;
        }
        Ok(json!({ "ok": true, "logged": true, "seq": seq }))
    }

    /// GET `/cli/zen/widget/errors?widget=&since=`: the logged errors after
    /// `since`, oldest first, each marked `current` when it hit the version
    /// on disk now.
    pub fn widget_errors(&self, widget: &str, since: Option<u64>) -> Result<J, ZenError> {
        self.require_set_up()?;
        let name = self.run_widget(widget)?;
        if !self.widget_dir(&name).is_dir() {
            return Err(ZenError::NotFound(format!("{} has no folder", widget_label(&name))));
        }
        let current = self.current_hash(&name);
        let placed = self.widget_is_placed(&name);
        let now = chrono::Utc::now();
        let runs = self.read_runs(&name);
        let last = runs.last().and_then(|r| r["seq"].as_u64()).unwrap_or(0);
        let errors: Vec<J> = runs
            .into_iter()
            .filter(|r| r["event"] == "error" && r["seq"].as_u64().unwrap_or(0) > since.unwrap_or(0))
            .map(|mut r| {
                let cur = current.as_deref().is_some_and(|h| r["hash"] == h);
                r["current"] = J::Bool(cur);
                // 0.46.3: does validate count it (placed, this version, fresh)?
                let why = if cur { stale_reason(&r, placed, now) } else { Some("older-version") };
                r["counted"] = J::Bool(why.is_none());
                if let Some(w) = why {
                    r["staleReason"] = J::String(w.into());
                }
                r
            })
            .collect();
        Ok(json!({ "ok": true, "widget": name, "hash": current, "placed": placed, "last": last, "errors": errors }))
    }

    /// The bundle hash of the folder as it is on disk, when it bundles clean.
    pub(crate) fn current_hash(&self, name: &str) -> Option<String> {
        let st = self.widget_status(name);
        (st.state == super::WidgetState::Ok).then(|| st.hash().to_string()).filter(|h| !h.is_empty())
    }

    /// What version `hash` of a user widget has done at runtime.
    pub fn widget_run_summary(&self, name: &str, hash: &str) -> RunSummary {
        let mut s = RunSummary::default();
        for r in self.read_runs(name).into_iter().filter(|r| r["hash"] == hash) {
            if r["event"] == "ready" {
                s.ran = true;
            } else if r["event"] == "error" {
                s.errors += 1;
                s.last.push(r);
            }
        }
        let n = s.last.len();
        s.last.drain(..n.saturating_sub(3));
        s
    }

    /// Where a user widget is placed: `{garden, gardenName, placement}`.
    pub fn widget_placements(&self, name: &str) -> Vec<J> {
        self.custom_placements()
            .into_iter()
            .filter(|p| p.widget == name)
            .map(|p| json!({ "garden": p.garden, "gardenName": p.garden_name, "placement": p.placement }))
            .collect()
    }

    /// What validate makes of version `hash`'s runtime log now (0.46.3):
    /// errors fail only when the widget is placed and they are fresh
    /// ([`RUNTIME_FRESH_SECS`]); the rest are stale (warnings).
    pub fn widget_runtime(&self, name: &str, hash: &str) -> RuntimeVerdict {
        self.widget_runtime_at(name, hash, chrono::Utc::now())
    }

    /// [`Self::widget_runtime`] as of `now` (tests).
    pub fn widget_runtime_at(&self, name: &str, hash: &str, now: chrono::DateTime<chrono::Utc>) -> RuntimeVerdict {
        let mut v = RuntimeVerdict { placements: self.widget_placements(name), ..RuntimeVerdict::default() };
        let placed = v.placed();
        for mut r in self.read_runs(name).into_iter().filter(|r| r["hash"] == hash) {
            if r["event"] == "ready" {
                v.ran = true;
            } else if r["event"] == "error" {
                match stale_reason(&r, placed, now) {
                    Some(why) => {
                        r["staleReason"] = J::String(why.into());
                        v.stale.push(r);
                    }
                    None => v.failing.push(r),
                }
            }
        }
        v
    }

    /// POST `/cli/zen/widget/errors/clear {widget, all?}` (0.46.3): drop
    /// the runtime errors validate no longer counts (an older version's,
    /// a widget no Garden places, older than [`RUNTIME_FRESH_SECS`]), or
    /// every error with `all`. `ready` rows stay (validate's pending rule
    /// reads them). Writes only the widget's `runtime.jsonl`.
    pub fn clear_widget_errors(&self, widget: &str, all: bool) -> Result<J, ZenError> {
        self.clear_widget_errors_at(widget, all, chrono::Utc::now())
    }

    /// [`Self::clear_widget_errors`] as of `now` (tests).
    pub fn clear_widget_errors_at(&self, widget: &str, all: bool, now: chrono::DateTime<chrono::Utc>) -> Result<J, ZenError> {
        self.require_set_up()?;
        let name = self.run_widget(widget)?;
        if !self.widget_dir(&name).is_dir() {
            return Err(ZenError::NotFound(format!("{} has no folder", widget_label(&name))));
        }
        let current = self.current_hash(&name);
        let placed = self.widget_is_placed(&name);
        let _log = RUN_LOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let runs = self.read_runs(&name);
        let before = runs.len();
        let last_seq = runs.last().and_then(|r| r["seq"].as_u64()).unwrap_or(0);
        let mut keep: Vec<J> = runs
            .into_iter()
            .filter(|r| {
                if r["event"] != "error" {
                    return true;
                }
                if all {
                    return false;
                }
                // Kept: what validate still counts.
                current.as_deref().is_some_and(|h| r["hash"] == h) && stale_reason(r, placed, now).is_none()
            })
            .collect();
        let cleared = before - keep.len();
        if cleared > 0 {
            // A marker keeps the sequence going (`--since` never sees a
            // number twice); validate and `widget errors` skip it.
            keep.push(json!({ "seq": last_seq, "at": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), "event": "cleared", "count": cleared }));
            let path = self.runs_path(&name);
            let text: String = keep.iter().map(|r| r.to_string() + "\n").collect();
            crate::fs_atomic::atomic_write_str(&path, &text).map_err(|e| io(e, &path))?;
        }
        let left = keep.iter().filter(|r| r["event"] == "error").count();
        Ok(json!({ "ok": true, "widget": name, "cleared": cleared, "left": left, "all": all }))
    }

    /// `k2 zen validate --widget <name> --types[=all]`: the optional type
    /// stage (`jscheck::types`, tsc-rs as a subprocess). Warnings only.
    pub fn widget_types(&self, widget: &str, all: bool) -> Result<J, ZenError> {
        self.require_set_up()?;
        let name = self.run_widget(widget)?;
        let dir = self.widget_dir(&name);
        if !dir.is_dir() {
            return Err(ZenError::NotFound(format!("{} has no folder", widget_label(&name))));
        }
        let read = crate::zen::widgets::read_folder(&dir, &widget_label(&name));
        let r = crate::jscheck::types::check_widget(&widget_label(&name), &read.files, crate::jscheck::types::find_checker(), all);
        Ok(json!({
            "checker": r.checker,
            "warnings": r.warnings,
            "hidden": r.hidden,
            "note": r.note,
        }))
    }

    /// Every source address this widget's placements bind (each placement's
    /// `sourceAddresses` on its Garden's resolved page) plus its manifest's
    /// fixed names, lower-case. Config the person or an agent wrote: never
    /// a value read from a source.
    pub fn configured_source_addresses(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |a: &str| {
            let a = a.trim().to_ascii_lowercase();
            if !a.is_empty() && !out.contains(&a) {
                out.push(a);
            }
        };
        if let Some(J::Array(list)) = self.widget_status(name).manifest.as_ref().and_then(|m| m.sources.clone()) {
            for a in list.iter().filter_map(J::as_str) {
                push(a);
            }
        }
        let placed: Vec<_> = self.custom_placements().into_iter().filter(|p| p.widget == name).collect();
        let mut gardens: Vec<&str> = placed.iter().map(|p| p.garden.as_str()).collect();
        gardens.sort_unstable();
        gardens.dedup();
        for g in gardens {
            let Ok(page) = self.resolve(Some(g)) else { continue };
            for w in page["page"]["widgets"].as_array().into_iter().flatten() {
                let ours = placed.iter().any(|p| p.garden == g && w["id"] == p.placement.as_str()) && w["widget"] == name;
                if ours {
                    for a in w["sourceAddresses"].as_array().into_iter().flatten().filter_map(J::as_str) {
                        push(a);
                    }
                }
            }
        }
        out
    }

    /// Is this user widget placed in any Garden?
    pub fn widget_is_placed(&self, name: &str) -> bool {
        self.custom_placements().iter().any(|p| p.widget == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_follow_the_message() {
        assert!(suggestion_for("w", "k2: thread.post refused (cap_not_granted): no").contains("manifest.json"));
        assert!(suggestion_for("w", "ReferenceError: draw is not defined").contains("<script src> order"));
        assert!(suggestion_for("w", "TypeError: Cannot read properties of undefined (reading 'x')").contains("guard data"));
        assert!(suggestion_for("w", "k2 failed: not connected").contains("k2.connected"));
        assert!(suggestion_for("w", "boom").contains("k2 zen widget errors w --since"));
    }

    #[test]
    fn run_text_of_a_redacted_row_names_fixed_fields_only() {
        let r = json!({"kind": "error", "class": "TypeError", "line": 12, "redacted": true});
        assert_eq!(run_text(&r), "TypeError at line 12 [text withheld: this widget reads data sources]");
        let r = json!({"kind": "refusal", "class": "sources.read", "code": "not_bound", "redacted": true});
        assert_eq!(run_text(&r), "refused sources.read (not_bound) [text withheld: this widget reads data sources]");
        assert_eq!(run_text(&json!({"message": "boom"})), "boom");
        assert!(fixed_word("sources.read", true) && !fixed_word("sources.read", false));
        assert!(!fixed_word("a b", true) && !fixed_word("", true) && !fixed_word(&"x".repeat(61), true));
    }

    /// 0.46.1: frames erroring at the same moment each get their own
    /// `#seq` (the read-then-append used to race to duplicates).
    #[test]
    fn concurrent_reports_get_unique_seqs() {
        let d = std::env::temp_dir().join(format!("k2-zen-runs-race-{}", uuid::Uuid::new_v4().simple()));
        let root = d.join(".k2").join("zen");
        let f = ZenFiles::new(root.clone());
        assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
        f.setup().expect("setup");
        f.new_widget("racer", None).expect("new widget");
        let hash = "a".repeat(64);
        const N: usize = 24;
        let start = std::sync::Arc::new(std::sync::Barrier::new(N));
        let threads: Vec<_> = (0..N)
            .map(|i| {
                let (root, hash, start) = (root.clone(), hash.clone(), start.clone());
                std::thread::spawn(move || {
                    let f = ZenFiles::new(root);
                    start.wait();
                    let msg = format!("Error: boom {i}");
                    f.record_widget_run("racer", &hash, "error", Some(&msg), None).expect("record")["seq"]
                        .as_u64()
                        .expect("seq")
                })
            })
            .collect();
        let mut seqs: Vec<u64> = threads.into_iter().map(|t| t.join().expect("thread")).collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=N as u64).collect::<Vec<_>>(), "every report its own seq");
        let logged = f.widget_errors("racer", None).expect("errors");
        let mut on_disk: Vec<u64> = logged["errors"]
            .as_array()
            .expect("errors")
            .iter()
            .map(|e| e["seq"].as_u64().expect("seq"))
            .collect();
        on_disk.sort_unstable();
        assert_eq!(on_disk, (1..=N as u64).collect::<Vec<_>>(), "{logged}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn cut_drops_control_characters_and_caps() {
        assert_eq!(cut("a\u{0}b\u{1b}[31mc\nd", 100), "ab[31mc\nd");
        assert_eq!(cut(&"x".repeat(10), 4), "xxxx…");
    }
}
