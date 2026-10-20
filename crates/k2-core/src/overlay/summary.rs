//! One-line message summaries (0.46.3, `plan-projects-threads-chatter-v1`
//! "Summaries"). K2 ships no AI: the sender writes the line
//! (`k2 msg --summary "…"`) and the daemon only falls back mechanically.
//!
//! - **Sender**: newlines collapsed, trimmed, capped at [`SUMMARY_MAX`]
//!   characters, redacted like the body. `summarySource = "sender"`.
//! - **Derived** (no summary sent, older agents, federated peers): the
//!   first prose line, cut to its first sentence and at most
//!   [`DERIVED_MAX`] characters. A heading joins the line after it;
//!   code fences and table rows are skipped. `summarySource = "derived"`.
//!   Computed at write time, and at read time for docs written before the
//!   field, so nothing is backfilled.
//!
//! The summary is for people (Chatter lists, the Projects view, phone
//! clients). The recipient's terminal still gets the whole body.

use super::doc::OverlayDoc;

/// Longest sender summary kept (characters).
pub const SUMMARY_MAX: usize = 160;
/// Longest derived summary (characters).
pub const DERIVED_MAX: usize = 120;
/// Bodies longer than this (characters) should carry a summary; the CLI
/// hints when one is missing.
pub const LONG_BODY_CHARS: usize = 600;

pub const SOURCE_SENDER: &str = "sender";
pub const SOURCE_DERIVED: &str = "derived";

/// Cut `s` to at most `max` characters, ending in `…` when cut.
fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// A sender's summary, cleaned: one line, trimmed, capped, redacted.
/// `None` when blank.
pub fn clean_sender_summary(raw: &str) -> Option<String> {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return None;
    }
    Some(cap(&crate::redaction::redact(&one_line), SUMMARY_MAX))
}

/// The mechanical fallback: the first prose line of `body`.
pub fn derive_summary(body: &str) -> Option<String> {
    let mut in_fence = false;
    let mut heading: Option<String> = None;
    for raw in body.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.is_empty() || line.starts_with('|') || line.starts_with("<!--") {
            continue;
        }
        // `[from x · m_…] ` prefixes never reach a doc body, but a quoted
        // one can: it is not prose.
        let text = line
            .trim_start_matches('#')
            .trim_start_matches(['-', '*', '>', ' '])
            .trim();
        if text.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            // A heading joins the line after it.
            if heading.is_none() {
                heading = Some(text.trim_end_matches(':').to_string());
                continue;
            }
        }
        let sentence = first_sentence(text);
        let joined = match heading.take() {
            Some(h) => format!("{h}: {sentence}"),
            None => sentence,
        };
        let one_line = joined.split_whitespace().collect::<Vec<_>>().join(" ");
        return Some(cap(&crate::redaction::redact(&one_line), DERIVED_MAX));
    }
    heading.map(|h| cap(&crate::redaction::redact(&h), DERIVED_MAX))
}

/// Up to and including the first `. `, `! ` or `? ` (a whole short line
/// stays whole).
fn first_sentence(text: &str) -> String {
    let bytes = text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if matches!(b, b'.' | b'!' | b'?') && bytes.get(i + 1).is_some_and(|n| *n == b' ') {
            return text[..=i].to_string();
        }
    }
    text.to_string()
}

/// Set a chatter doc's summary: the sender's (cleaned) when given, else
/// the derived one.
pub fn apply(doc: &mut OverlayDoc, sender: Option<&str>) {
    if let Some(s) = sender.and_then(clean_sender_summary) {
        doc.summary = Some(s);
        doc.summary_source = Some(SOURCE_SENDER.to_string());
        return;
    }
    fill_derived(doc);
}

/// Read time: a chatter doc from before the field gets its derived
/// summary (never stored, so nothing is backfilled).
pub fn fill_derived(doc: &mut OverlayDoc) {
    if doc.kind != "chatter" || doc.summary.is_some() {
        return;
    }
    if let Some(s) = doc.body.as_deref().and_then(derive_summary) {
        doc.summary = Some(s);
        doc.summary_source = Some(SOURCE_DERIVED.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_summary_is_one_line_capped_and_redacted() {
        assert_eq!(clean_sender_summary("  ship\nthe   fix \n").as_deref(), Some("ship the fix"));
        assert_eq!(clean_sender_summary(" \n\t "), None);
        let long = "a".repeat(400);
        let s = clean_sender_summary(&long).unwrap();
        assert_eq!(s.chars().count(), SUMMARY_MAX);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn derived_summary_takes_the_first_prose_sentence() {
        assert_eq!(derive_summary("Done. Pushed hold/x and ran the tests.").as_deref(), Some("Done."));
        assert_eq!(derive_summary("short single line").as_deref(), Some("short single line"));
        assert_eq!(
            derive_summary("## Status\nAll 8 items built. Details below.\n- a\n- b").as_deref(),
            Some("Status: All 8 items built.")
        );
        assert_eq!(
            derive_summary("```\ncargo test\n```\n| a | b |\n|---|---|\n\nThe flake is fixed. Root cause below.").as_deref(),
            Some("The flake is fixed.")
        );
        assert_eq!(derive_summary("\n\n  \n"), None);
        let long = format!("{} end", "word ".repeat(60));
        let d = derive_summary(&long).unwrap();
        assert_eq!(d.chars().count(), DERIVED_MAX);
        assert!(d.ends_with('…'));
        // A version number's dot is not a sentence end.
        assert_eq!(derive_summary("Built 0.46.3 today. More later.").as_deref(), Some("Built 0.46.3 today."));
    }

    #[test]
    fn apply_prefers_the_sender_and_marks_the_source() {
        let mut d = OverlayDoc::chatter("i".into(), "a".into(), "b".into(), "Long body. More.".into(), "msg", "accepted");
        apply(&mut d, Some("what I need: a review"));
        assert_eq!(d.summary.as_deref(), Some("what I need: a review"));
        assert_eq!(d.summary_source.as_deref(), Some(SOURCE_SENDER));
        let mut d = OverlayDoc::chatter("i".into(), "a".into(), "b".into(), "Long body. More.".into(), "msg", "accepted");
        apply(&mut d, Some("   "));
        assert_eq!((d.summary.as_deref(), d.summary_source.as_deref()), (Some("Long body."), Some(SOURCE_DERIVED)));
        // Serialized as camelCase fields on the doc.
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["summary"], "Long body.");
        assert_eq!(v["summarySource"], "derived");
    }

    #[test]
    fn read_time_fill_only_touches_chatter_without_a_summary() {
        let mut old = OverlayDoc::chatter("i".into(), "a".into(), "b".into(), "Old line. x".into(), "msg", "accepted");
        old.summary = None;
        old.summary_source = None;
        fill_derived(&mut old);
        assert_eq!(old.summary.as_deref(), Some("Old line."));
        let mut text = OverlayDoc::text("i".into(), "a".into(), "b".into(), "A thread line.".into(), "thread");
        fill_derived(&mut text);
        assert_eq!(text.summary, None, "Thread lines carry no summary");
    }
}
