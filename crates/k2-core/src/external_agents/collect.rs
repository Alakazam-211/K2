//! Grok Bot messages collected into K2's own Thread store (0.46.0).
//!
//! Each local Grok Bot gets one Thread conversation in `k2-threads.redb`
//! (the store behind `k2 thread`), keyed `grokbot:<agent id>` and read at
//! the address `gb:<bot name>`. K2 COLLECTS into it, so the history grows
//! for as long as K2 runs, survives restarts and reads the same from every
//! client:
//!
//! - **Start.** The conversation starts when collection starts (the first
//!   time the daemon sees the bot with Show on): one system line, "K2
//!   started collecting <name>'s messages here on <date>". K2 does NOT
//!   import the older history in Grok Bot's `store.db` (for a bot whose
//!   live chat moved elsewhere it stops months back, Rosson 2026-10-15). It
//!   records `store.db`'s highest cursor at that moment as the start mark.
//! - **Current bots** (chat on this computer): every new `store.db` row
//!   after the mark (`seq > mark`), so nothing between two polls is lost.
//!   The newest rows are re-read on each change, so a line Grok Bot updates
//!   in place (a streamed reply, a card answered) is updated here too.
//! - **Stale bots** (live chat elsewhere, see `ExternalAgent::stale`): the
//!   gateway has no transcript, only each bot's LATEST entry (`listAgents`
//!   `lastEntry` / `lastMessagePreview` / ids / `snapshotSeq`). Each new one
//!   the poller sees is appended. When the snapshot counter jumped by more
//!   than one between two polls (beyond K2's own sends), a gap line goes
//!   first: "… earlier messages not shown — open Grok Bot". The counter
//!   also moves for non-chat entries, so a gap means "may have missed".
//! - **K2's own sends**, as they go out (`[from <name>] …`, stamped with the
//!   sender's credential), then linked to the line Grok Bot reports for them
//!   (by `clientNonce` in `store.db`, by text in the roster) so a line
//!   never shows twice.
//! - **Migration.** The 0.45.7 live tail
//!   (`~/.k2/external-agents/grokbots-live-tail.json`) is read once: its
//!   lines go after the start line (dated when that tail started), then
//!   the file is deleted.
//!
//! Dedupe: every line is indexed by the product's entry id (and its other
//! ids: a K2 send's nonce id, the gateway id) in the collected-conversation
//! index (`overlay::collected_write`), in the same redb transaction as the
//! append and the cursor, so a retry or a restart never adds a line twice.
//!
//! These conversations belong to no workspace: collecting never wakes an
//! agent, never injects into a PTY, never pushes and never federates.
//! Reads are gated by the daemon's Grok Bots routes (people who can see
//! Grok Bots only). Chat text only: never a token.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::grokbot::{fallback_name_for, GrokBot, StoreLine, StoreRead};
use super::{valid_agent_id, Author, ExternalAgent, ExternalAgentProvider, ThreadMessage};
use crate::overlay::doc::{ORIGIN_K2, ORIGIN_ROSTER, ORIGIN_STORE, ORIGIN_SYSTEM};
use crate::overlay::{self, CollectedMeta, ExtTxn, OverlayDoc, OverlayItem, OverlayPage, PostAuthor};

/// [`CollectedMeta::source`] for Grok Bot lines.
pub const SOURCE: &str = "grokbot";
/// The Thread address prefix: `gb:<bot name>` (or `gb:<agent id>`).
pub const ADDR_PREFIX: &str = "gb:";
/// Longest text kept per line (chars).
pub const MAX_TEXT_CHARS: usize = 8_000;
/// How long a K2 send waits to be matched by the entry the gateway later
/// reports for it (so it isn't shown twice).
pub const OWN_SEND_MATCH_MS: i64 = 15 * 60_000;
/// A summary text shorter than this (chars) only matches a K2 send exactly.
const MIN_PREFIX_MATCH_CHARS: usize = 12;
/// `store.db` rows read per query, and most queries per sync.
const STORE_BATCH: usize = 500;
const STORE_ROUNDS: usize = 20;
/// Newest collected `store.db` rows re-read for in-place updates.
const REFRESH_WINDOW: usize = 50;
/// Most unlinked K2 sends remembered for linking.
const OWN_KEEP: usize = 20;
/// The gap line's text.
pub const GAP_TEXT: &str = "… earlier messages not shown — open Grok Bot";
/// [`CollectedMeta::entry_id`] of the start line.
pub const START_ENTRY_ID: &str = "start";

/// The Thread conversation a Grok Bot's messages are collected into.
pub fn conversation_id(agent_id: &str) -> String {
    format!("grokbot:{}", agent_id.to_ascii_lowercase())
}

/// A bot's Thread address.
pub fn address(name: &str) -> String {
    format!("{ADDR_PREFIX}{name}")
}

/// The bot named by a `gb:` address (name or id), or None when `addr` is
/// not one.
pub fn parse_addr(addr: &str) -> Option<&str> {
    let a = addr.trim();
    if a.len() < ADDR_PREFIX.len() || !a[..ADDR_PREFIX.len()].eq_ignore_ascii_case(ADDR_PREFIX) {
        return None;
    }
    let rest = a[ADDR_PREFIX.len()..].trim();
    (!rest.is_empty()).then_some(rest)
}

/// The start line's text.
pub fn start_text(name: &str, started_ms: i64) -> String {
    let when = chrono::DateTime::from_timestamp_millis(started_ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string())
        .unwrap_or_default();
    format!("K2 started collecting {name}'s messages here on {when}.")
}

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

/// A K2 send not yet linked to the line Grok Bot reports for it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct OwnSend {
    ext_id: String,
    doc_id: String,
    text: String,
    at_ms: i64,
}

/// The collector's cursor for one bot (redb `ext_state`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    started_ms: i64,
    /// `store.db`'s highest cursor when collection started (None while the
    /// store can't be read: set the first time it can).
    store_mark: Option<i64>,
    /// Last `store.db` cursor collected.
    store_seq: i64,
    /// Roster: latest entry id seen, snapshot counter, K2 sends since.
    last_id: Option<String>,
    epoch: Option<String>,
    seq: Option<i64>,
    pending_own: i64,
    own: Vec<OwnSend>,
}

fn load_state(tx: &ExtTxn<'_>) -> Result<Option<State>, String> {
    match tx.state()? {
        Some(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| format!("grokbot collect state: {e}")),
        None => Ok(None),
    }
}

fn read_state(conv: &str) -> Result<Option<State>, String> {
    match overlay::collected_state(conv)? {
        Some(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| format!("grokbot collect state: {e}")),
        None => Ok(None),
    }
}

fn save_state(tx: &mut ExtTxn<'_>, st: &State) -> Result<(), String> {
    let b = serde_json::to_vec(st).map_err(|e| format!("grokbot collect state json: {e}"))?;
    tx.set_state(&b)
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

fn author_str(a: Author) -> &'static str {
    match a {
        Author::Human => "human",
        Author::Agent => "agent",
        Author::Peer => "peer",
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// One collected line before it becomes a doc.
struct Line {
    origin: &'static str,
    entry_id: String,
    author: Author,
    text: String,
    at_ms: Option<i64>,
    via_k2: bool,
    preview_only: bool,
    detail: Option<Value>,
    /// Shown to readers when the line has no text of its own.
    label: Option<String>,
}

fn line_doc(name: &str, l: &Line, sender: Option<&str>) -> OverlayDoc {
    let addr = address(name);
    let (from, to) = match l.author {
        Author::Human => (sender.unwrap_or("you").to_string(), addr),
        Author::Agent => (addr, "you".to_string()),
        Author::Peer => (
            l.detail
                .as_ref()
                .and_then(|d| d["fromAgent"]["name"].as_str())
                .unwrap_or("another agent")
                .to_string(),
            addr,
        ),
    };
    let body_is_label = l.text.is_empty();
    let body = if body_is_label {
        l.label.clone().unwrap_or_default()
    } else {
        l.text.clone()
    };
    let mut doc = OverlayDoc::text(uuid::Uuid::new_v4().to_string(), from, to, body, "grokbot");
    if let Some(at) = l.at_ms {
        doc.created_at = at / 1000;
    }
    doc.external = Some(CollectedMeta {
        source: SOURCE.into(),
        origin: l.origin.into(),
        entry_id: l.entry_id.clone(),
        author: author_str(l.author).into(),
        at_ms: l.at_ms,
        via_k2: l.via_k2,
        preview_only: l.preview_only,
        body_is_label,
        detail: l.detail.clone(),
    });
    doc
}

fn system_doc(name: &str, kind: &str, entry_id: String, text: String, at_ms: i64) -> OverlayDoc {
    let mut doc = OverlayDoc::text(
        uuid::Uuid::new_v4().to_string(),
        "k2".into(),
        address(name),
        text,
        "system",
    );
    doc.created_at = at_ms / 1000;
    doc.external = Some(CollectedMeta {
        source: SOURCE.into(),
        origin: ORIGIN_SYSTEM.into(),
        entry_id,
        author: "system".into(),
        at_ms: Some(at_ms),
        detail: Some(json!({ "system": kind })),
        ..Default::default()
    });
    doc
}

fn append_gap(tx: &mut ExtTxn<'_>, name: &str, before_id: &str, at_ms: i64) -> Result<(), String> {
    let gap_id = format!("gap:{before_id}");
    if tx.lookup(&gap_id)?.is_none() {
        let doc = system_doc(name, "gap", gap_id.clone(), GAP_TEXT.into(), at_ms);
        tx.append(&doc, &[gap_id.as_str()])?;
    }
    Ok(())
}

/// Append the start line and the new cursor.
fn start(
    tx: &mut ExtTxn<'_>,
    name: &str,
    started_ms: i64,
    mark: Option<i64>,
    live: Option<&LiveSummary>,
) -> Result<State, String> {
    let doc = system_doc(
        name,
        "start",
        START_ENTRY_ID.into(),
        start_text(name, started_ms),
        started_ms,
    );
    tx.append(&doc, &[START_ENTRY_ID])?;
    let st = State {
        started_ms,
        store_mark: mark,
        store_seq: mark.unwrap_or(0),
        // The bot's latest entry when collection starts is history: not
        // collected.
        last_id: live.and_then(|l| l.entry_id.clone()),
        epoch: live.and_then(|l| l.snapshot_epoch.clone()),
        seq: live.and_then(|l| l.snapshot_seq),
        ..Default::default()
    };
    save_state(tx, &st)?;
    Ok(st)
}

/// The start mark: `store.db`'s highest cursor now (0 without a store),
/// None while it can't be read.
fn store_mark(p: &GrokBot, agent_id: &str) -> Option<i64> {
    probe_mark(p, agent_id).0
}

/// [`store_mark`], plus why the store can't be read.
fn probe_mark(p: &GrokBot, agent_id: &str) -> (Option<i64>, Option<String>) {
    match p.store_max_seq(agent_id) {
        Ok(StoreRead::Ok(n)) => (Some(n), None),
        Ok(StoreRead::NoStore) => (Some(0), None),
        Ok(StoreRead::Unsupported(r)) => (None, Some(r)),
        Err(_) => (None, None),
    }
}

/// Replace `doc`'s text/meta with `new`'s (same id, seq, time) when they
/// differ. Returns whether it changed.
fn update_in_place(tx: &mut ExtTxn<'_>, old: &OverlayDoc, mut new: OverlayDoc) -> Result<bool, String> {
    new.id = old.id.clone();
    new.created_at = old.created_at;
    new.author = old.author.clone();
    if new.body == old.body && new.external == old.external && new.from == old.from {
        return Ok(false);
    }
    tx.put_doc(&new)?;
    Ok(true)
}

fn detail_of(m: &ThreadMessage) -> Option<Value> {
    let v = serde_json::to_value(m).ok()?;
    let mut out = Map::new();
    for k in ["toAgent", "fromAgent", "openInApp", "card"] {
        if let Some(x) = v.get(k).filter(|x| !x.is_null()) {
            out.insert(k.to_string(), x.clone());
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

fn label_of(m: &ThreadMessage) -> Option<String> {
    let what = m.open_in_app.as_deref().unwrap_or("message");
    Some(format!("[{what}: open Grok Bot to see it]"))
}

/// Collect one `store.db` row. Returns whether a visible line changed.
fn upsert_store_line(
    tx: &mut ExtTxn<'_>,
    st: &mut State,
    name: &str,
    row: &StoreLine,
) -> Result<bool, String> {
    let Some(m) = &row.message else {
        return Ok(false);
    };
    // A K2 send Grok Bot stored: link it to the line K2 recorded.
    let own_id = row
        .client_nonce
        .as_deref()
        .filter(|_| m.via_k2)
        .map(|n| format!("k2-{n}"));
    if let Some(own_id) = &own_id {
        if let Some(doc_id) = tx.lookup(own_id)? {
            if tx.lookup(&row.entry_id)?.is_none() {
                tx.alias(&row.entry_id, &doc_id)?;
            }
            st.own.retain(|o| &o.ext_id != own_id);
            return Ok(false);
        }
    }
    let line = Line {
        origin: ORIGIN_STORE,
        entry_id: row.entry_id.clone(),
        author: m.author,
        text: cap_text(&m.text),
        at_ms: m.at_ms,
        via_k2: m.via_k2,
        preview_only: false,
        detail: detail_of(m),
        label: label_of(m),
    };
    let doc = line_doc(name, &line, None);
    if let Some(doc_id) = tx.lookup(&row.entry_id)? {
        let Some(old) = tx.doc(&doc_id)? else {
            return Err(format!("grokbot collect: index points at missing doc {doc_id}"));
        };
        let origin = old.external.as_ref().map(|x| x.origin.as_str());
        // A K2 send keeps the text K2 sent; store rows win over the roster.
        if origin == Some(ORIGIN_K2) || origin == Some(ORIGIN_SYSTEM) {
            return Ok(false);
        }
        return update_in_place(tx, &old, doc);
    }
    let mut ids = vec![row.entry_id.as_str()];
    if let Some(o) = &own_id {
        ids.push(o.as_str());
    }
    tx.append(&doc, &ids)?;
    Ok(true)
}

/// Record what one roster poll said. Returns (visible change, state dirty).
fn observe(
    tx: &mut ExtTxn<'_>,
    st: &mut State,
    name: &str,
    stale: bool,
    s: &LiveSummary,
    in_store: impl FnOnce(&str) -> bool,
    now: i64,
) -> Result<(bool, bool), String> {
    let mut dirty = false;
    let same_epoch = st.epoch.is_some() && st.epoch == s.snapshot_epoch;
    let jumped = match (same_epoch, st.seq, s.snapshot_seq) {
        (true, Some(old), Some(new)) => new - old > 1 + st.pending_own,
        _ => false,
    };
    // K2's sends since the last move stay counted until the counter moves.
    if st.epoch != s.snapshot_epoch || st.seq != s.snapshot_seq {
        st.epoch = s.snapshot_epoch.clone();
        st.seq = s.snapshot_seq;
        st.pending_own = 0;
        dirty = true;
    }
    let Some(id) = s.entry_id.as_deref().filter(|i| !i.is_empty()) else {
        return Ok((false, dirty));
    };
    let text = s.text.as_deref().map(cap_text).filter(|t| !t.is_empty());
    let new_id = st.last_id.as_deref() != Some(id);
    if new_id {
        st.last_id = Some(id.to_string());
        dirty = true;
    }
    if let Some(doc_id) = tx.lookup(id)? {
        // Grok Bot may update a line in place; only roster lines follow the
        // roster (a store row or a K2 send has the full text).
        let Some(old) = tx.doc(&doc_id)? else {
            return Err(format!("grokbot collect: index points at missing doc {doc_id}"));
        };
        let roster_line = old.external.as_ref().is_some_and(|x| x.origin == ORIGIN_ROSTER);
        let Some(t) = text.filter(|_| roster_line) else {
            return Ok((false, dirty));
        };
        let mut doc = old.clone();
        doc.body = Some(crate::redaction::redact(&t));
        if let Some(x) = doc.external.as_mut() {
            x.preview_only = s.preview_only;
            x.body_is_label = false;
        }
        let changed = update_in_place(tx, &old, doc)?;
        return Ok((changed, dirty));
    }
    if !new_id || in_store(id) {
        // Already seen, or on this computer: the store path collects it.
        return Ok((false, dirty));
    }
    let Some(text) = text else {
        return Ok((false, dirty));
    };
    // A K2 send the roster now reports: link it, don't show it twice.
    if let Some(i) = st
        .own
        .iter()
        .rposition(|o| now - o.at_ms <= OWN_SEND_MATCH_MS && matches_sent(&text, &o.text))
    {
        let own = st.own.remove(i);
        tx.alias(id, &own.doc_id)?;
        return Ok((false, true));
    }
    let at = s.at_ms.unwrap_or(now);
    if jumped && stale {
        append_gap(tx, name, id, at)?;
    }
    let line = Line {
        origin: ORIGIN_ROSTER,
        entry_id: id.to_string(),
        author: s.author.unwrap_or(Author::Agent),
        text,
        at_ms: Some(at),
        via_k2: false,
        preview_only: s.preview_only,
        detail: None,
        label: None,
    };
    tx.append(&line_doc(name, &line, None), &[id])?;
    Ok((true, true))
}

/// What one sync of a bot did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    /// A line was added or changed (tell clients).
    pub changed: bool,
    /// `store.db` could not be read (the reason, for people).
    pub unsupported: Option<String>,
}

/// Collect what's new for bot `a`: `store.db` rows after the start mark
/// (when `read_store`: the poller passes whether the bot's chat marker
/// moved), the roster's latest entry, and the start line on first sight.
pub fn sync_agent(p: &GrokBot, a: &ExternalAgent, read_store: bool, now: i64) -> Result<Synced, String> {
    if !valid_agent_id(&a.id) {
        return Ok(Synced::default());
    }
    let conv = conversation_id(&a.id);
    let st0 = read_state(&conv)?;
    let (mark_now, mut unsupported) = match &st0 {
        Some(s) if s.store_mark.is_some() => (None, None),
        _ => probe_mark(p, &a.id),
    };
    // Read Grok Bot's store before taking the Thread store's write lock.
    let mut rows: Vec<StoreLine> = Vec::new();
    if let (true, Some(s)) = (read_store, &st0) {
        if let Some(mark) = s.store_mark {
            let mut after = s.store_seq.max(mark);
            for _ in 0..STORE_ROUNDS {
                match p.store_lines(&a.id, after, None, STORE_BATCH) {
                    Ok(StoreRead::Ok(batch)) => {
                        let n = batch.len();
                        if let Some(last) = batch.last() {
                            after = last.seq;
                        }
                        rows.extend(batch);
                        if n < STORE_BATCH {
                            break;
                        }
                    }
                    Ok(StoreRead::Unsupported(r)) => {
                        unsupported = Some(r);
                        break;
                    }
                    Ok(StoreRead::NoStore) | Err(_) => break,
                }
            }
            if let Ok(StoreRead::Ok(window)) = p.store_lines(&a.id, mark, Some(REFRESH_WINDOW), 0) {
                for w in window {
                    if !rows.iter().any(|r| r.seq == w.seq) {
                        rows.push(w);
                    }
                }
            }
        }
    }
    let live_id = a.live.as_ref().and_then(|l| l.entry_id.clone());
    let needs_store_check = live_id.as_deref().is_some_and(|id| {
        st0.as_ref().map_or(true, |s| s.last_id.as_deref() != Some(id))
    });
    let in_store = match (&live_id, needs_store_check) {
        (Some(id), true) => p.store_has_entry(&a.id, id) == Some(true),
        _ => false,
    };
    let changed = overlay::collected_write(&conv, |tx| {
        let mut changed = false;
        let mut dirty = false;
        let mut st = match load_state(tx)? {
            Some(s) => s,
            None => {
                changed = true;
                start(tx, &a.name, now, mark_now, a.live.as_ref())?
            }
        };
        if st.store_mark.is_none() {
            if let Some(m) = mark_now {
                st.store_mark = Some(m);
                st.store_seq = m;
                dirty = true;
            }
        }
        if let Some(mark) = st.store_mark {
            for row in rows.iter().filter(|r| r.seq > mark) {
                changed |= upsert_store_line(tx, &mut st, &a.name, row)?;
                if row.seq > st.store_seq {
                    st.store_seq = row.seq;
                    dirty = true;
                }
            }
        }
        if let Some(live) = &a.live {
            let (c, d) = observe(tx, &mut st, &a.name, a.stale, live, |_| in_store, now)?;
            changed |= c;
            dirty |= d;
        }
        let before = st.own.len();
        st.own.retain(|o| now - o.at_ms <= OWN_SEND_MATCH_MS);
        dirty |= st.own.len() != before;
        if dirty || changed {
            save_state(tx, &st)?;
        }
        Ok(changed)
    })?;
    Ok(Synced { changed, unsupported })
}

/// Record a send K2 just made to bot `agent_id` (`prompt` is the text as
/// delivered, `[from <name>] …`; `author` the sender's credential).
pub fn record_own_send(
    p: &GrokBot,
    agent_id: &str,
    client_nonce: &str,
    prompt: &str,
    sender: &str,
    author: Option<PostAuthor>,
    now: i64,
) -> Result<(), String> {
    if !valid_agent_id(agent_id) {
        return Err("invalid agent id".into());
    }
    let conv = conversation_id(agent_id);
    // First sight: start collecting first (with the roster's latest entry
    // as the start point), so the send lands after the start line.
    let roster = if read_state(&conv)?.is_none() {
        p.list_agents()
            .ok()
            .and_then(|l| l.into_iter().find(|a| a.id.eq_ignore_ascii_case(agent_id)))
    } else {
        None
    };
    let name = match &roster {
        Some(a) => {
            sync_agent(p, a, false, now)?;
            a.name.clone()
        }
        None => name_from_thread(&conv).unwrap_or_else(|| fallback_name_for(agent_id)),
    };
    let mark_now = store_mark(p, agent_id);
    let ext_id = format!("k2-{client_nonce}");
    overlay::collected_write(&conv, |tx| {
        let mut st = match load_state(tx)? {
            Some(s) => s,
            None => start(tx, &name, now, mark_now, None)?,
        };
        if tx.lookup(&ext_id)?.is_some() {
            return Ok(());
        }
        let line = Line {
            origin: ORIGIN_K2,
            entry_id: ext_id.clone(),
            author: Author::Human,
            text: cap_text(prompt),
            at_ms: Some(now),
            via_k2: true,
            preview_only: false,
            detail: None,
            label: None,
        };
        let mut doc = line_doc(&name, &line, Some(sender));
        doc.author = author;
        tx.append(&doc, &[ext_id.as_str()])?;
        st.pending_own += 1;
        st.own.push(OwnSend {
            ext_id: ext_id.clone(),
            doc_id: doc.id.clone(),
            text: line.text.clone(),
            at_ms: now,
        });
        while st.own.len() > OWN_KEEP {
            st.own.remove(0);
        }
        save_state(tx, &st)
    })
}

/// The bot name the start line recorded (`to: gb:<name>`).
fn name_from_thread(conv: &str) -> Option<String> {
    let page = overlay::read_thread_page(conv, None, Some(2), 1).ok()?;
    let to = page.items.first()?.doc.to.clone()?;
    parse_addr(&to).map(str::to_string)
}

// ── reading ─────────────────────────────────────────────────────────────

/// A page of a bot's collected Thread (`limit == 0`: everything).
pub fn page(agent_id: &str, since: Option<i64>, before: Option<i64>, limit: usize) -> Result<OverlayPage, String> {
    overlay::read_thread_page(&conversation_id(agent_id), since, before, limit)
}

/// thread-delta-v1 on a collected Thread: the lines after `after_seq`,
/// oldest first, plus lines at or below it updated in place since.
pub fn delta(agent_id: &str, after_seq: i64, limit: usize) -> Result<overlay::ThreadDelta, String> {
    overlay::read_thread_delta(&conversation_id(agent_id), after_seq, limit)
}

/// Whether K2 has started collecting this bot.
pub fn started(agent_id: &str) -> Result<bool, String> {
    Ok(read_state(&conversation_id(agent_id))?.is_some())
}

/// The newest collected chat line (not a system line): `(at ms, text)`.
pub fn newest_line(agent_id: &str) -> Option<(i64, String)> {
    let page = page(agent_id, None, None, 5).ok()?;
    page.items.iter().rev().find_map(|i| {
        let x = i.doc.external.as_ref()?;
        if x.origin == ORIGIN_SYSTEM || x.body_is_label {
            return None;
        }
        Some((x.at_ms.unwrap_or(i.doc.created_at * 1000), i.doc.body.clone()?))
    })
}

/// A collected Thread item as the Grok Bot chat view's message.
pub fn message_json(item: &OverlayItem) -> Value {
    let x = item.doc.external.clone().unwrap_or_default();
    let body = item.doc.body.clone().unwrap_or_default();
    let mut o = json!({
        "seq": item.seq,
        "id": item.id,
        "entryId": x.entry_id,
        "origin": x.origin,
        "author": if x.author.is_empty() { "system".to_string() } else { x.author.clone() },
        "text": if x.body_is_label { String::new() } else { body },
        "atMs": x.at_ms.unwrap_or(item.doc.created_at * 1000),
        "viaK2": x.via_k2,
        "previewOnly": x.preview_only,
    });
    let obj = o.as_object_mut().expect("object");
    if let Some(Value::Object(d)) = &x.detail {
        for (k, v) in d {
            obj.insert(k.clone(), v.clone());
        }
    }
    if let Some(a) = &item.doc.author {
        obj.insert("sentBy".into(), serde_json::to_value(a).unwrap_or(Value::Null));
    }
    o
}

// ── migration of the 0.45.7 live tail ──────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LegacyEntry {
    id: String,
    gateway_id: Option<String>,
    at_ms: i64,
    author: Option<Author>,
    text: String,
    via_k2: bool,
    gap_before: bool,
    preview_only: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LegacyAgent {
    since_ms: i64,
    last_id: Option<String>,
    epoch: Option<String>,
    seq: Option<i64>,
    pending_own: i64,
    entries: VecDeque<LegacyEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyFile {
    agents: BTreeMap<String, LegacyAgent>,
}

/// The 0.45.7 live tail file (read once, then deleted).
pub fn legacy_tail_path() -> PathBuf {
    crate::paths::k2_home()
        .join("external-agents")
        .join("grokbots-live-tail.json")
}

/// Move the 0.45.7 live tail's lines into each bot's Thread (after
/// the start line, dated when that tail started), then delete the file.
/// Idempotent (entry ids dedupe). `names`: agent id → name. Returns the
/// bots whose Thread changed.
pub fn migrate_legacy_tail(p: &GrokBot, names: &HashMap<String, String>) -> Result<Vec<String>, String> {
    let path = legacy_tail_path();
    let raw = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let file: LegacyFile = match serde_json::from_slice(&raw) {
        Ok(f) => f,
        Err(e) => {
            crate::log_debug!("[grokbots] old live tail unreadable, retired: {e}");
            LegacyFile::default()
        }
    };
    let mut moved = Vec::new();
    for (agent_id, tail) in &file.agents {
        if !valid_agent_id(agent_id) {
            continue;
        }
        let name = names
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(agent_id))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| fallback_name_for(agent_id));
        let conv = conversation_id(agent_id);
        let mark_now = if read_state(&conv)?.is_none() {
            store_mark(p, agent_id)
        } else {
            None
        };
        let changed = overlay::collected_write(&conv, |tx| {
            let mut changed = false;
            let mut st = match load_state(tx)? {
                Some(s) => s,
                None => {
                    changed = true;
                    let started = if tail.since_ms > 0 { tail.since_ms } else { now_ms() };
                    let mut s = start(tx, &name, started, mark_now, None)?;
                    s.last_id = tail.last_id.clone();
                    s.epoch = tail.epoch.clone();
                    s.seq = tail.seq;
                    s.pending_own = tail.pending_own;
                    s
                }
            };
            for e in &tail.entries {
                if e.id.is_empty() || e.text.trim().is_empty() {
                    continue;
                }
                let known = tx.lookup(&e.id)?.is_some()
                    || match &e.gateway_id {
                        Some(g) => tx.lookup(g)?.is_some(),
                        None => false,
                    };
                if known {
                    continue;
                }
                if e.gap_before {
                    append_gap(tx, &name, &e.id, e.at_ms)?;
                }
                let author = e.author.unwrap_or(if e.via_k2 { Author::Human } else { Author::Agent });
                let line = Line {
                    origin: if e.via_k2 { ORIGIN_K2 } else { ORIGIN_ROSTER },
                    entry_id: e.id.clone(),
                    author,
                    text: cap_text(&e.text),
                    at_ms: Some(e.at_ms),
                    via_k2: e.via_k2,
                    preview_only: e.preview_only,
                    detail: None,
                    label: None,
                };
                let sender = e.via_k2.then(|| {
                    e.text
                        .strip_prefix("[from ")
                        .and_then(|r| r.split_once("] "))
                        .map(|(n, _)| n.to_string())
                        .unwrap_or_else(|| "you".into())
                });
                let doc = line_doc(&name, &line, sender.as_deref());
                let mut ids = vec![e.id.as_str()];
                if let Some(g) = &e.gateway_id {
                    ids.push(g.as_str());
                }
                tx.append(&doc, &ids)?;
                changed = true;
            }
            st.own.clear();
            save_state(tx, &st)?;
            Ok(changed)
        })?;
        if changed {
            moved.push(agent_id.clone());
        }
    }
    match std::fs::remove_file(&path) {
        Ok(()) => crate::log_debug!("[grokbots] old live tail moved into Threads and retired"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("retire {}: {e}", path.display())),
    }
    Ok(moved)
}

#[cfg(test)]
mod tests;
