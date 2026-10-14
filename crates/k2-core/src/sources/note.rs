//! Trigger note builder (contract §9 "Note template", <= 4 KB). Owner: B2.
//! Only the wake pipeline calls it.
//!
//! The note is written from fixed parts: names, item keys, states, reason
//! codes and (for `values` sources only) up to two short field values per
//! item. Never a secret, a raw body, a header or a URL query. The vault's
//! redactor runs over the finished text as a belt.

use super::extract::Projection;
use super::spec::{AgentAccess, Json, Keep};
use super::state::{Health, SourceState, Transition, TransitionKind};
use super::store::SourceRow;

/// One event in a (possibly coalesced) note.
pub struct NoteEvent<'a> {
    pub source: &'a SourceRow,
    pub transition: &'a Transition,
    pub state: &'a SourceState,
    pub projection: Option<&'a Projection>,
    pub at: i64,
    /// The trigger's owner note.
    pub owner_note: Option<&'a str>,
}

pub struct NoteInput<'a> {
    /// Oldest first.
    pub events: Vec<NoteEvent<'a>>,
    /// The server this check runs on (`Runs on <host>`).
    pub host: &'a str,
}

/// Longest field value shown in an items line.
const MAX_VALUE_CHARS: usize = 60;

/// `HH:MM` (UTC) of a wall-clock second.
pub fn hhmm(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// `40 s`, `12 min`, `3 h 5 min`, `2 d`.
pub fn dur(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else if s < 86_400 {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if m == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {m} min")
        }
    } else {
        format!("{} d", s / 86_400)
    }
}

/// A URL without its query or fragment.
pub fn strip_query(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

/// The phrase for one transition: `went red`, `ohana went red`, ….
pub fn phrase(tr: &Transition, row: &SourceRow, st: &SourceState, at: i64) -> String {
    match (&tr.kind, &tr.scope_item) {
        (TransitionKind::WentCrit, Some(item)) => format!("{item} went red"),
        (TransitionKind::WentWarn, Some(item)) => format!("{item} went amber"),
        (TransitionKind::Recovered, Some(item)) => format!("{item} recovered"),
        (TransitionKind::WentCrit, None) => "went red".into(),
        (TransitionKind::WentWarn, None) => "went amber".into(),
        (TransitionKind::Recovered, None) => "recovered".into(),
        (TransitionKind::WentStale, _) => match st.updated_at {
            Some(u) => format!("has had no good check for {}", dur(at - u)),
            None => "has had no good check yet".into(),
        },
        (other, _) => {
            let _ = row;
            other.code().replace('_', " ")
        }
    }
}

/// Values may be shown in this source's notes.
fn values_allowed(row: &SourceRow) -> bool {
    row.agent_access == AgentAccess::Values && row.keep == Keep::Values
}

fn short_value(v: &Json) -> String {
    let s = match v {
        Json::String(s) => strip_query(s).to_string(),
        Json::Null => "none".into(),
        Json::Bool(_) | Json::Number(_) => v.to_string(),
        // Arrays and objects are never pasted into a note.
        Json::Array(a) => format!("{} values", a.len()),
        Json::Object(_) => "…".into(),
    };
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if s.chars().count() > MAX_VALUE_CHARS {
        format!("{}…", s.chars().take(MAX_VALUE_CHARS).collect::<String>())
    } else {
        s
    }
}

/// `ohana (timeout since 03:04)` / `luzz (version 0.44.4, phase null)`.
fn item_summary(ev: &NoteEvent, key: &str) -> String {
    let st = ev.state.items.get(key);
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = st.and_then(|i| i.reason) {
        parts.push(r.code().to_string());
    } else if values_allowed(ev.source) {
        if let Some(pi) = ev.projection.and_then(|p| p.item(key)) {
            for (name, v) in pi.fields.iter().take(2) {
                parts.push(format!("{name} {}", short_value(v)));
            }
        }
    }
    let mut s = parts.join(", ");
    if let Some(since) = st.and_then(|i| i.hard_since) {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&format!("since {}", hhmm(since)));
    }
    if s.is_empty() {
        key.to_string()
    } else {
        format!("{key} ({s})")
    }
}

/// Red/amber item keys, the transition's own item first.
fn bad_items(ev: &NoteEvent, health: Health) -> Vec<String> {
    let mut keys: Vec<String> =
        ev.state.items.values().filter(|i| i.health == health).map(|i| i.item_key.clone()).collect();
    if let Some(own) = &ev.transition.scope_item {
        if let Some(pos) = keys.iter().position(|k| k == own) {
            let k = keys.remove(pos);
            keys.insert(0, k);
        }
    }
    keys
}

/// One event block, showing at most `limit` items (red first, then amber).
fn block(ev: &NoteEvent, host: &str, limit: usize) -> (String, usize) {
    let row = ev.source;
    let tr = ev.transition;
    let mut lines: Vec<String> = Vec::new();
    let hard = match tr.kind {
        TransitionKind::WentCrit | TransitionKind::WentWarn if row.fail_after > 1 => {
            format!(" (hard after {} checks)", row.fail_after)
        }
        _ => String::new(),
    };
    lines.push(format!(
        "[source:{}] {} at {} UTC{hard}. Runs on {host}.",
        row.name,
        phrase(tr, row, ev.state, ev.at),
        hhmm(ev.at)
    ));
    let red = bad_items(ev, Health::Crit);
    let amber = bad_items(ev, Health::Warn);
    let total = red.len() + amber.len();
    let mut shown = 0usize;
    let mut items_line: Vec<String> = Vec::new();
    for (label, keys) in [("Red", &red), ("Amber", &amber)] {
        let room = limit.saturating_sub(shown);
        let take: Vec<String> = keys.iter().take(room).map(|k| item_summary(ev, k)).collect();
        shown += take.len();
        if !take.is_empty() {
            items_line.push(format!("{label}: {}", take.join(", ")));
        }
    }
    if total > 0 {
        let mut line = items_line.join(". ");
        if shown < total {
            if !line.is_empty() {
                line.push_str("; ");
            }
            line.push_str(&format!("and {} more; run `k2 sources show {}`", total - shown, row.name));
        } else {
            line.push('.');
        }
        lines.push(line);
    }
    let reason = tr.reason.or(ev.state.reason);
    match (ev.state.updated_at, reason) {
        (Some(u), Some(r)) => lines.push(format!("Last good: {} UTC. Reason: {}.", hhmm(u), r.code())),
        (Some(u), None) => lines.push(format!("Last good: {} UTC.", hhmm(u))),
        (None, Some(r)) => lines.push(format!("Reason: {}.", r.code())),
        (None, None) => {}
    }
    if let Some(link) = row.link.as_deref().map(strip_query).filter(|l| !l.is_empty()) {
        lines.push(format!("Link: {link}"));
    }
    if let Some(n) = ev.owner_note.map(str::trim).filter(|n| !n.is_empty()) {
        let n: String = n.chars().filter(|c| !c.is_control()).take(super::triggers::MAX_TRIGGER_NOTE).collect();
        lines.push(format!("Owner's note: {n}"));
    }
    lines.push(format!("Read it: k2 sources show {}", row.name));
    (lines.join("\n"), total)
}

fn assemble(input: &NoteInput, limits: &[usize]) -> String {
    let blocks: Vec<String> = input
        .events
        .iter()
        .zip(limits)
        .map(|(ev, &lim)| block(ev, input.host, lim).0)
        .collect();
    blocks.join("\n\n")
}

/// Cut to `max` bytes on a char boundary (a last resort).
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The note text, <= `NOTE_MAX_BYTES`. Coalesced events are one block each,
/// oldest first, separated by a blank line. Over the limit, whole items are
/// cut (from the longest list first) and the line ends with "and N more".
pub fn build(input: &NoteInput) -> String {
    let max = super::NOTE_MAX_BYTES;
    let mut limits: Vec<usize> =
        input.events.iter().map(|ev| block(ev, input.host, usize::MAX).1).collect();
    loop {
        let text = crate::redaction::redact(&assemble(input, &limits));
        if text.len() <= max {
            return text;
        }
        match limits.iter().enumerate().filter(|(_, &l)| l > 0).max_by_key(|(_, &l)| l) {
            Some((i, _)) => limits[i] -= 1,
            None => return cut(&text, max),
        }
    }
}

/// A test wake from Settings (`[TEST] …`): proves the path, says nothing is
/// wrong.
pub fn build_test(source: &SourceRow, host: &str, owner_note: Option<&str>) -> String {
    let mut lines = vec![format!(
        "[TEST] [source:{}] This is a test from Settings; nothing is wrong. Runs on {host}.",
        source.name
    )];
    if let Some(n) = owner_note.map(str::trim).filter(|n| !n.is_empty()) {
        lines.push(format!("Owner's note: {n}"));
    }
    lines.push(format!("Read it: k2 sources show {}", source.name));
    cut(&crate::redaction::redact(&lines.join("\n")), super::NOTE_MAX_BYTES)
}

/// The inbox fallback title: `source:<name> <phrase>`.
pub fn title(source_name: &str, phrase: &str) -> String {
    format!("source:{source_name} {phrase}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::sources::extract::ProjectedItem;
    use crate::sources::state::{ItemState, Phase, Reason};
    use crate::sources::triggers::tests::row;

    fn item(key: &str, health: Health, reason: Option<Reason>) -> ItemState {
        ItemState {
            item_key: key.into(),
            is_target: true,
            origin: None,
            health,
            soft_health: None,
            soft_count: 0,
            reason,
            http_status: None,
            hard_since: Some(11_040), // 03:04
            recovered_at: None,
            updated_at: None,
            checked_at: None,
            etag: None,
            last_modified: None,
            incident_id: Some(format!("inc-{key}")),
        }
    }

    fn crit(item: Option<&str>) -> Transition {
        Transition {
            scope_item: item.map(str::to_string),
            from: Health::Ok,
            to: Health::Crit,
            kind: TransitionKind::WentCrit,
            incident_id: Some("inc".into()),
            reason: Some(Reason::Timeout),
            hard: true,
        }
    }

    fn fixture() -> (SourceRow, SourceState, Projection) {
        let mut r = row();
        r.link = Some("https://k2.dev/admin/fleet?team=abc#x".into());
        let mut st = SourceState::new("s1");
        st.phase = Phase::Live;
        st.updated_at = Some(10_920); // 03:02
        st.items.insert("ohana".into(), item("ohana", Health::Crit, Some(Reason::Timeout)));
        st.items.insert("luzz".into(), item("luzz", Health::Crit, None));
        st.items.insert("akzm".into(), item("akzm", Health::Ok, None));
        let proj = Projection {
            fields: BTreeMap::new(),
            items: vec![ProjectedItem {
                key: "luzz".into(),
                fields: BTreeMap::from([
                    ("version".to_string(), Json::from("0.44.4")),
                    ("url".to_string(), Json::from("https://luzz.k2.dev/x?token=abc")),
                ]),
            }],
        };
        (r, st, proj)
    }

    #[test]
    fn note_follows_the_template() {
        let (r, st, proj) = fixture();
        let t = crit(Some("ohana"));
        let note = build(&NoteInput {
            events: vec![NoteEvent { source: &r, transition: &t, state: &st, projection: Some(&proj), at: 11_520, owner_note: Some("check the relay first") }],
            host: "akzm.k2.dev",
        });
        let lines: Vec<&str> = note.lines().collect();
        assert_eq!(lines[0], "[source:fleet] ohana went red at 03:12 UTC (hard after 3 checks). Runs on akzm.k2.dev.");
        assert_eq!(lines[1], "Red: ohana (timeout since 03:04), luzz (url https://luzz.k2.dev/x, version 0.44.4 since 03:04).");
        assert_eq!(lines[2], "Last good: 03:02 UTC. Reason: timeout.");
        assert_eq!(lines[3], "Link: https://k2.dev/admin/fleet");
        assert_eq!(lines[4], "Owner's note: check the relay first");
        assert_eq!(lines[5], "Read it: k2 sources show fleet");
        assert!(!note.contains('?'), "no query strings: {note}");
        assert!(!note.contains("token"), "{note}");
    }

    #[test]
    fn shape_and_states_sources_carry_no_values() {
        for (aa, keep) in [(AgentAccess::Shape, Keep::Values), (AgentAccess::Values, Keep::States)] {
            let (mut r, st, proj) = fixture();
            r.agent_access = aa;
            r.keep = keep;
            let t = crit(Some("luzz"));
            let note = build(&NoteInput {
                events: vec![NoteEvent { source: &r, transition: &t, state: &st, projection: Some(&proj), at: 11_520, owner_note: None }],
                host: "akzm.k2.dev",
            });
            assert!(!note.contains("0.44.4"), "{aa:?}/{keep:?}: {note}");
            assert!(!note.contains("version"), "{note}");
            assert!(note.contains("luzz (since 03:04)"), "{note}");
            assert!(!note.contains("Owner's note"), "{note}");
        }
    }

    #[test]
    fn long_lists_end_with_and_n_more_under_4_kb() {
        let (r, mut st, _) = fixture();
        for i in 0..200 {
            let k = format!("box-with-a-fairly-long-name-{i:03}");
            st.items.insert(k.clone(), item(&k, Health::Crit, Some(Reason::Dns)));
        }
        let t = crit(None);
        let note = build(&NoteInput {
            events: vec![NoteEvent { source: &r, transition: &t, state: &st, projection: None, at: 11_520, owner_note: Some(&"n".repeat(500)) }],
            host: "akzm.k2.dev",
        });
        assert!(note.len() <= crate::sources::NOTE_MAX_BYTES, "{} bytes", note.len());
        assert!(note.contains("more; run `k2 sources show fleet`"), "{note}");
        assert!(note.ends_with("Read it: k2 sources show fleet"), "whole lines kept");
    }

    #[test]
    fn coalesced_notes_are_blocks_oldest_first() {
        let (r, st, _) = fixture();
        let a = crit(Some("ohana"));
        let mut b = crit(None);
        b.kind = TransitionKind::Recovered;
        let note = build(&NoteInput {
            events: vec![
                NoteEvent { source: &r, transition: &a, state: &st, projection: None, at: 11_520, owner_note: None },
                NoteEvent { source: &r, transition: &b, state: &st, projection: None, at: 12_000, owner_note: None },
            ],
            host: "h",
        });
        let blocks: Vec<&str> = note.split("\n\n").collect();
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].starts_with("[source:fleet] ohana went red at 03:12"));
        assert!(blocks[1].starts_with("[source:fleet] recovered at 03:20 UTC. Runs on h."));
    }

    #[test]
    fn stale_phrase_and_omitted_lines() {
        let mut r = row();
        r.link = None;
        let st = SourceState::new("s1");
        let mut t = crit(None);
        t.kind = TransitionKind::WentStale;
        t.reason = None;
        let note = build(&NoteInput {
            events: vec![NoteEvent { source: &r, transition: &t, state: &st, projection: None, at: 100, owner_note: None }],
            host: "h",
        });
        assert_eq!(note, "[source:fleet] has had no good check yet at 00:01 UTC. Runs on h.\nRead it: k2 sources show fleet");
        let mut st2 = st.clone();
        st2.updated_at = Some(100);
        assert_eq!(phrase(&t, &r, &st2, 100 + 1900), "has had no good check for 31 min");
    }

    #[test]
    fn redactor_runs_over_the_note() {
        let secret = "k2-note-test-secret-value-9f8e7d6c5b4a";
        // The only redaction hook k2-core's tests install (the daemon
        // installs the vault's at boot).
        assert!(crate::redaction::install(Box::new(|s: &str| s.replace(
            "k2-note-test-secret-value-9f8e7d6c5b4a",
            "[k2v:TEST]"
        ))));
        let (r, st, _) = fixture();
        let t = crit(None);
        let note = build(&NoteInput {
            events: vec![NoteEvent { source: &r, transition: &t, state: &st, projection: None, at: 11_520, owner_note: Some(secret) }],
            host: "h",
        });
        assert!(!note.contains(secret), "{note}");
    }

    #[test]
    fn test_wakes_are_marked() {
        let r = row();
        let n = build_test(&r, "akzm.k2.dev", Some("hi"));
        assert!(n.starts_with("[TEST] [source:fleet] This is a test from Settings; nothing is wrong."));
        assert!(n.contains("Owner's note: hi"));
        assert_eq!(title("fleet", "went red"), "source:fleet went red");
    }
}
