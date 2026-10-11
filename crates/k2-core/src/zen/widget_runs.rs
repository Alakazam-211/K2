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
            "not_bound" => "use an address this widget reaches (k2.agents.list() lists them)".into(),
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
        let mut runs = self.read_runs(&name);
        if event == "ready" && runs.iter().any(|r| r["event"] == "ready" && r["hash"] == hash) {
            return Ok(json!({ "ok": true, "logged": false }));
        }
        let seq = runs.last().and_then(|r| r["seq"].as_u64()).unwrap_or(0) + 1;
        let mut row = json!({ "seq": seq, "at": now_rfc3339(), "event": event, "hash": hash });
        if event == "error" {
            let message = cut(message.unwrap_or("error"), MAX_MESSAGE);
            row["suggestion"] = J::String(suggestion_for(&name, &message));
            row["message"] = J::String(message);
            if let Some(s) = stack.filter(|s| !s.trim().is_empty()) {
                row["stack"] = J::String(cut(s, MAX_STACK));
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
        let runs = self.read_runs(&name);
        let last = runs.last().and_then(|r| r["seq"].as_u64()).unwrap_or(0);
        let errors: Vec<J> = runs
            .into_iter()
            .filter(|r| r["event"] == "error" && r["seq"].as_u64().unwrap_or(0) > since.unwrap_or(0))
            .map(|mut r| {
                r["current"] = J::Bool(current.as_deref().is_some_and(|h| r["hash"] == h));
                r
            })
            .collect();
        Ok(json!({ "ok": true, "widget": name, "hash": current, "last": last, "errors": errors }))
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
    fn cut_drops_control_characters_and_caps() {
        assert_eq!(cut("a\u{0}b\u{1b}[31mc\nd", 100), "ab[31mc\nd");
        assert_eq!(cut(&"x".repeat(10), 4), "xxxx…");
    }
}
