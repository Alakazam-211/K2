//! Live tail for a stale external agent (0.45.7).
//!
//! A "stale" agent's live conversation runs outside this computer (another
//! box, the product's cloud), so its local chat store stops at some old
//! date and the product's gateway has no transcript endpoint. Sends still
//! reach the live conversation. What K2 CAN see of the new lines is the
//! roster summary (`listAgents`): the id, kind and text of the agent's
//! LATEST entry, its activity time and a snapshot counter.
//!
//! So the daemon's roster poller (every few seconds) records, per stale
//! agent, each new latest entry it observes, and K2 records its own sends
//! as they go out (it knows the text). The thread route shows this tail
//! after the old local history.
//!
//! **What the tail can and can't show.** Only what the gateway's summary
//! exposed: the latest entry at each poll. When several lines land between
//! two polls (a burst, a long turn), only the last one is seen. When the
//! snapshot counter jumped by more than one between two polls (beyond the
//! sends K2 itself made), the entry is marked [`TailEntry::gap_before`] and
//! the client shows "… earlier messages not shown — open Grok Bot". The
//! counter may also move for entries that are not chat lines (tool calls,
//! in-place updates), so a gap marker means "may have missed lines", not
//! "did".
//!
//! Stored as one small JSON file under `~/.k2/external-agents/` (0600) so
//! the tail survives a daemon restart: at most [`MAX_ENTRIES`] per agent,
//! deduped by entry id. It holds chat text only — never a token.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::Author;

/// Most entries kept per agent (oldest dropped first).
pub const MAX_ENTRIES: usize = 200;
/// Longest text kept per entry (chars).
pub const MAX_TEXT_CHARS: usize = 8_000;
/// How long a K2 send waits to be matched by the entry the gateway later
/// reports for it (so it isn't shown twice).
pub const OWN_SEND_MATCH_MS: i64 = 15 * 60_000;
/// A summary text shorter than this (chars) only matches a K2 send exactly,
/// never as a prefix of it.
const MIN_PREFIX_MATCH_CHARS: usize = 12;

/// What the roster summary says about an agent's latest entry, already
/// mapped by the product adapter (author, text). Product-agnostic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveSummary {
    /// The latest entry's id (Grok Bot: `lastMessageId`, else
    /// `newestEntryId`).
    pub entry_id: Option<String>,
    /// When it happened (ms since epoch).
    pub at_ms: Option<i64>,
    pub author: Option<Author>,
    /// The entry's text, as much as the summary gives.
    pub text: Option<String>,
    /// `text` came from the one-line preview (may be cut short).
    pub preview_only: bool,
    /// Snapshot counter: compared within one epoch only.
    pub snapshot_epoch: Option<String>,
    pub snapshot_seq: Option<i64>,
}

/// One live line, on disk and on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailEntry {
    /// The product's entry id, or `k2-<nonce>` for a K2 send.
    pub id: String,
    /// For a K2 send: the product's id for it, once the roster reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
    pub at_ms: i64,
    pub author: Author,
    pub text: String,
    /// Sent from K2.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub via_k2: bool,
    /// Lines may be missing just before this one (see the module doc).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gap_before: bool,
    /// The text is the product's one-line preview (may be cut short).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview_only: bool,
}

/// An agent's tail as the thread route returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTail {
    /// When K2 started following this agent's live conversation.
    pub since_ms: i64,
    pub entries: Vec<TailEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AgentTail {
    since_ms: i64,
    /// Latest entry id seen in the roster.
    last_id: Option<String>,
    epoch: Option<String>,
    seq: Option<i64>,
    /// K2 sends since the last observation (each moves the counter once).
    pending_own: i64,
    entries: VecDeque<TailEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct TailFile {
    version: u32,
    agents: BTreeMap<String, AgentTail>,
}

const FILE_VERSION: u32 = 1;

fn path() -> PathBuf {
    crate::paths::k2_home()
        .join("external-agents")
        .join("grokbots-live-tail.json")
}

struct Loaded {
    path: PathBuf,
    file: TailFile,
}

static STATE: Mutex<Option<Loaded>> = Mutex::new(None);

fn load_file(p: &std::path::Path) -> TailFile {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice::<TailFile>(&b).ok())
        .filter(|f| f.version == FILE_VERSION)
        .unwrap_or(TailFile {
            version: FILE_VERSION,
            agents: BTreeMap::new(),
        })
}

/// Run `f` on the (lazily loaded) tail file; saves when `f` says it
/// changed something.
fn with_state<R>(f: impl FnOnce(&mut TailFile) -> (R, bool)) -> R {
    let p = path();
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().map(|l| &l.path) != Some(&p) {
        *guard = Some(Loaded {
            file: load_file(&p),
            path: p.clone(),
        });
    }
    let loaded = guard.as_mut().expect("loaded above");
    let (out, dirty) = f(&mut loaded.file);
    if dirty {
        loaded.file.version = FILE_VERSION;
        match serde_json::to_vec(&loaded.file) {
            Ok(body) => {
                if let Err(e) = crate::fs_atomic::atomic_write_private(&p, &body) {
                    crate::log_debug!("[grokbots] live tail not saved: {e}");
                }
            }
            Err(e) => crate::log_debug!("[grokbots] live tail not encoded: {e}"),
        }
    }
    out
}

/// Forget the in-memory copy (the next call re-reads the file), like a
/// daemon restart.
#[cfg(any(test, feature = "test-util"))]
pub fn forget_cache_for_tests() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn cap_text(t: &str) -> String {
    t.trim().chars().take(MAX_TEXT_CHARS).collect()
}

fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` without a leading `[from <name>] ` (K2's envelope).
fn strip_from(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("[from ") {
        if let Some(i) = rest.find("] ") {
            if !rest[..i].contains(['[', '\n']) {
                return &rest[i + 2..];
            }
        }
    }
    text
}

/// Whether a roster summary text is the line K2 sent as `sent` (exactly,
/// or as a cut-short preview of it; with or without K2's envelope).
fn matches_sent(observed: &str, sent: &str) -> bool {
    let obs = norm(observed);
    let obs = obs
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim_end()
        .to_string();
    if obs.is_empty() {
        return false;
    }
    [norm(sent), norm(strip_from(sent))].iter().any(|c| {
        *c == obs || (obs.chars().count() >= MIN_PREFIX_MATCH_CHARS && c.starts_with(&obs))
    })
}

/// Add `e` in time order (after every line at or before its time) and drop
/// the oldest past [`MAX_ENTRIES`]. A line the roster reports late can be
/// older than a K2 send recorded at once (a send right after a restart,
/// before the first poll), so it goes before that send, not after.
/// A K2 send is always the newest line (`by_time: false` appends it).
fn push_capped(entries: &mut VecDeque<TailEntry>, e: TailEntry, by_time: bool) {
    let at = if by_time {
        entries
            .iter()
            .rposition(|x| x.at_ms <= e.at_ms)
            .map_or(0, |i| i + 1)
    } else {
        entries.len()
    };
    entries.insert(at, e);
    while entries.len() > MAX_ENTRIES {
        entries.pop_front();
    }
}

/// Record what one roster poll said about stale agent `agent_id`.
/// `in_local_store(id)` says whether an entry is already in the local chat
/// store (then it shows there, not in the tail). Returns whether the tail's
/// lines changed (a new line, a line updated in place, a K2 send linked).
pub fn observe(
    agent_id: &str,
    s: &LiveSummary,
    now_ms: i64,
    in_local_store: impl FnOnce(&str) -> bool,
) -> bool {
    with_state(|file| {
        let mut created = false;
        let st = file.agents.entry(agent_id.to_string()).or_insert_with(|| {
            created = true;
            AgentTail {
                since_ms: now_ms,
                ..Default::default()
            }
        });
        // `dirty`: the file needs saving. `changed`: the lines changed.
        let mut dirty = created;
        // Counter jump since the last poll, net of K2's own sends.
        let same_epoch = st.epoch.is_some() && st.epoch == s.snapshot_epoch;
        let jumped = match (same_epoch, st.seq, s.snapshot_seq) {
            (true, Some(old), Some(new)) => new - old > 1 + st.pending_own,
            _ => false,
        };
        if st.epoch != s.snapshot_epoch || st.seq != s.snapshot_seq || st.pending_own != 0 {
            st.epoch = s.snapshot_epoch.clone();
            st.seq = s.snapshot_seq;
            st.pending_own = 0;
            dirty = true;
        }
        let Some(id) = s.entry_id.as_deref().filter(|i| !i.is_empty()) else {
            return (false, dirty);
        };
        let text = s.text.as_deref().map(cap_text).filter(|t| !t.is_empty());
        let new_id = st.last_id.as_deref() != Some(id);
        if new_id {
            st.last_id = Some(id.to_string());
            dirty = true;
        }

        // Already in the tail: Grok Bot may update a line in place.
        if let Some(e) = st
            .entries
            .iter_mut()
            .find(|e| e.id == id || e.gateway_id.as_deref() == Some(id))
        {
            // A K2 send keeps the text K2 sent (it is the full line).
            let changed = match text {
                Some(t) if !e.via_k2 && (e.text != t || e.preview_only != s.preview_only) => {
                    e.text = t;
                    e.preview_only = s.preview_only;
                    true
                }
                _ => false,
            };
            return (changed, dirty || changed);
        }
        if !new_id || in_local_store(id) {
            return (false, dirty);
        }
        let Some(text) = text else {
            return (false, dirty);
        };
        // A K2 send the roster now reports: link it, don't show it twice.
        if let Some(own) = st.entries.iter_mut().rev().find(|e| {
            e.via_k2
                && e.gateway_id.is_none()
                && now_ms - e.at_ms <= OWN_SEND_MATCH_MS
                && matches_sent(&text, &e.text)
        }) {
            own.gateway_id = Some(id.to_string());
            own.gap_before |= jumped;
            return (true, true);
        }
        push_capped(
            &mut st.entries,
            TailEntry {
                id: id.to_string(),
                gateway_id: None,
                at_ms: s.at_ms.unwrap_or(now_ms),
                author: s.author.unwrap_or(Author::Agent),
                text,
                via_k2: false,
                gap_before: jumped,
                preview_only: s.preview_only,
            },
            true,
        );
        (true, true)
    })
}

/// Record a send K2 just made to stale agent `agent_id` (`prompt` is the
/// text as delivered, `[from <name>] …`).
pub fn record_own_send(agent_id: &str, client_nonce: &str, prompt: &str, now_ms: i64) {
    let id = format!("k2-{client_nonce}");
    with_state(|file| {
        let st = file
            .agents
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentTail {
                since_ms: now_ms,
                ..Default::default()
            });
        if st.entries.iter().any(|e| e.id == id) {
            return ((), false);
        }
        st.pending_own += 1;
        push_capped(
            &mut st.entries,
            TailEntry {
                id,
                gateway_id: None,
                at_ms: now_ms,
                author: Author::Human,
                text: cap_text(prompt),
                via_k2: true,
                gap_before: false,
                preview_only: false,
            },
            false,
        );
        ((), true)
    })
}

/// The agent's tail (`None` when K2 never followed it).
pub fn tail(agent_id: &str) -> Option<LiveTail> {
    with_state(|file| {
        let out = file.agents.get(agent_id).map(|st| LiveTail {
            since_ms: st.since_ms,
            entries: st.entries.iter().cloned().collect(),
        });
        (out, false)
    })
}

/// The newest line of the agent's tail.
pub fn newest(agent_id: &str) -> Option<TailEntry> {
    with_state(|file| {
        let out = file
            .agents
            .get(agent_id)
            .and_then(|st| st.entries.back().cloned());
        (out, false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::TempHome;

    const A: &str = "0e5f5de8-7619-4ba4-9753-32c5470b2346";

    fn summary(id: &str, seq: i64, author: Author, text: &str) -> LiveSummary {
        LiveSummary {
            entry_id: Some(id.into()),
            at_ms: Some(1_000 + seq),
            author: Some(author),
            text: Some(text.into()),
            preview_only: false,
            snapshot_epoch: Some("ep1".into()),
            snapshot_seq: Some(seq),
        }
    }

    fn ids(t: &LiveTail) -> Vec<String> {
        t.entries.iter().map(|e| e.id.clone()).collect()
    }

    #[test]
    fn new_latest_entries_append_once_and_lines_in_the_local_store_are_skipped() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        assert!(tail(A).is_none());
        assert!(observe(
            A,
            &summary("e1", 10, Author::Agent, "first"),
            5_000,
            |_| false
        ));
        // The same latest entry again: nothing new.
        assert!(!observe(
            A,
            &summary("e1", 10, Author::Agent, "first"),
            5_100,
            |_| false
        ));
        assert!(observe(
            A,
            &summary("e2", 11, Author::Agent, "second"),
            5_200,
            |_| false
        ));
        // An entry this computer's store already has is not a tail line.
        assert!(!observe(
            A,
            &summary("e3", 12, Author::Agent, "local"),
            5_300,
            |id| id == "e3"
        ));
        // Back to an id already in the tail (no duplicate).
        observe(
            A,
            &summary("e2", 12, Author::Agent, "second"),
            5_400,
            |_| false,
        );
        let t = tail(A).expect("tail");
        assert_eq!(t.since_ms, 5_000);
        assert_eq!(ids(&t), vec!["e1", "e2"]);
        assert_eq!(t.entries[1].text, "second");
        assert_eq!(t.entries[1].at_ms, 1_011);
        assert!(t.entries.iter().all(|e| !e.gap_before && !e.via_k2));
        assert_eq!(newest(A).expect("newest").id, "e2");
    }

    #[test]
    fn a_line_updated_in_place_keeps_its_slot() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        observe(
            A,
            &summary("e1", 1, Author::Agent, "Working on it"),
            1,
            |_| false,
        );
        assert!(observe(
            A,
            &summary("e1", 2, Author::Agent, "Working on it. Done."),
            2,
            |_| false
        ));
        let t = tail(A).expect("tail");
        assert_eq!(ids(&t), vec!["e1"]);
        assert_eq!(t.entries[0].text, "Working on it. Done.");
    }

    #[test]
    fn the_ring_is_capped_and_survives_a_restart() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        for i in 0..(MAX_ENTRIES as i64 + 25) {
            observe(
                A,
                &summary(&format!("e{i}"), i, Author::Agent, &format!("line {i}")),
                i,
                |_| false,
            );
        }
        let t = tail(A).expect("tail");
        assert_eq!(t.entries.len(), MAX_ENTRIES);
        assert_eq!(t.entries[0].id, "e25", "oldest dropped first");
        assert_eq!(
            t.entries.last().expect("last").id,
            format!("e{}", MAX_ENTRIES + 24)
        );

        // A daemon restart: the file is read again.
        forget_cache_for_tests();
        let back = tail(A).expect("tail after restart");
        assert_eq!(back, t);
        // ...and keeps deduping against what it had.
        assert!(!observe(
            A,
            &summary(
                &format!("e{}", MAX_ENTRIES + 24),
                MAX_ENTRIES as i64 + 24,
                Author::Agent,
                &format!("line {}", MAX_ENTRIES + 24)
            ),
            0,
            |_| false
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path())
                .expect("tail file")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "tail file is owner-only");
        }
        let raw = std::fs::read_to_string(path()).expect("read tail file");
        assert!(raw.contains("line 30"), "{}", &raw[..200.min(raw.len())]);
    }

    #[test]
    fn a_snapshot_jump_marks_a_gap_but_not_for_k2s_own_sends_or_a_new_epoch() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        observe(A, &summary("e1", 10, Author::Agent, "one"), 1, |_| false);
        observe(A, &summary("e2", 11, Author::Agent, "two"), 2, |_| false);
        observe(A, &summary("e5", 14, Author::Agent, "five"), 3, |_| false);
        // K2 sent once: a jump of 2 is that send + the reply.
        record_own_send(
            A,
            "6b32aaaa-0000-4000-8000-000000000001",
            "[from Pat] ping",
            4,
        );
        observe(A, &summary("e7", 16, Author::Agent, "pong"), 5, |_| false);
        // A new epoch: the counter restarts, no gap judged.
        let mut s = summary("e8", 1, Author::Agent, "after reset");
        s.snapshot_epoch = Some("ep2".into());
        s.at_ms = Some(2_000);
        observe(A, &s, 6, |_| false);
        let t = tail(A).expect("tail");
        let gaps: Vec<(String, bool)> = t
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.gap_before))
            .collect();
        assert_eq!(
            gaps,
            vec![
                ("e1".to_string(), false),
                ("e2".to_string(), false),
                ("e5".to_string(), true),
                ("k2-6b32aaaa-0000-4000-8000-000000000001".to_string(), false),
                ("e7".to_string(), false),
                ("e8".to_string(), false),
            ]
        );
    }

    #[test]
    fn k2s_own_send_is_recorded_at_once_and_linked_when_the_roster_reports_it() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        let nonce = "6b32bbbb-0000-4000-8000-000000000002";
        record_own_send(
            A,
            nonce,
            "[from Rosson] Please ship the October report today",
            100,
        );
        let t = tail(A).expect("tail");
        assert_eq!(t.since_ms, 100);
        assert_eq!(t.entries.len(), 1);
        let e = &t.entries[0];
        assert_eq!(
            (e.author, e.via_k2, e.text.as_str()),
            (
                Author::Human,
                true,
                "[from Rosson] Please ship the October report today"
            )
        );
        // The roster's preview of it (cut short, envelope kept): linked.
        let mut s = summary(
            "u1",
            1,
            Author::Human,
            "[from Rosson] Please ship the October…",
        );
        s.preview_only = true;
        assert!(observe(A, &s, 200, |_| false));
        let t = tail(A).expect("tail");
        assert_eq!(t.entries.len(), 1, "not shown twice: {:?}", t.entries);
        assert_eq!(t.entries[0].gateway_id.as_deref(), Some("u1"));
        assert_eq!(
            t.entries[0].text,
            "[from Rosson] Please ship the October report today"
        );
        // The reply lands next.
        observe(A, &summary("r1", 2, Author::Agent, "On it."), 300, |_| {
            false
        });
        let t = tail(A).expect("tail");
        assert_eq!(ids(&t), vec![format!("k2-{nonce}"), "r1".to_string()]);
        assert!(!t.entries[1].via_k2);
        // Recording the same send twice is a no-op.
        record_own_send(A, nonce, "[from Rosson] again", 400);
        assert_eq!(tail(A).expect("tail").entries.len(), 2);
    }

    #[test]
    fn a_reply_reported_after_a_k2_send_but_older_than_it_goes_before_it() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        // A send right after a restart, before the first poll saw the bot.
        record_own_send(
            A,
            "6b32cccc-0000-4000-8000-000000000003",
            "[from Pat] next?",
            5_000,
        );
        // The poll then reports the bot's latest line, written before it.
        let mut s = summary("r0", 1, Author::Agent, "earlier reply");
        s.at_ms = Some(4_000);
        assert!(observe(A, &s, 5_100, |_| false));
        // And the answer, after it.
        let mut s = summary("r1", 3, Author::Agent, "the answer");
        s.at_ms = Some(6_000);
        observe(A, &s, 6_100, |_| false);
        let t = tail(A).expect("tail");
        assert_eq!(
            ids(&t),
            vec!["r0", "k2-6b32cccc-0000-4000-8000-000000000003", "r1"]
        );
    }

    #[test]
    fn sent_text_matching_rules() {
        assert!(matches_sent("[from Pat] hi", "[from Pat] hi"));
        assert!(matches_sent("hi", "[from Pat] hi"), "envelope stripped");
        assert!(matches_sent(
            "[from Pat]  a long\nmessage…",
            "[from Pat] a long message here"
        ));
        assert!(
            !matches_sent("h", "[from Pat] hi there"),
            "short prefixes never match"
        );
        assert!(!matches_sent("something else entirely", "[from Pat] hi"));
        assert!(!matches_sent("", "[from Pat] hi"));
        assert_eq!(strip_from("[from a] b"), "b");
        assert_eq!(strip_from("no envelope"), "no envelope");
    }

    #[test]
    fn a_summary_without_text_moves_the_cursor_but_adds_nothing() {
        let _home = TempHome::new();
        forget_cache_for_tests();
        let mut s = summary("e1", 1, Author::Agent, "");
        s.text = None;
        assert!(!observe(A, &s, 1, |_| false), "no line added");
        assert_eq!(tail(A).expect("followed").entries.len(), 0);
    }
}
