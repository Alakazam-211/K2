//! Host events on the docs socket (rendering-data contract r1.4, gap 26 and
//! the relay half of gaps 29 and 30): what K2 itself knows about a doc and
//! the kit's change stream doesn't carry.
//!
//! | Event | Frame | Who gets it |
//! |---|---|---|
//! | a preview started drawing ([`super::preview`]) | `preview_rendering {container, render_hash, width, look?, rev}` | sockets on that doc; each forwards it only while its person may read the doc |
//! | a preview rendered or failed, or a scene view's objects renamed (its image again, same render hash, `ids` named anew) ([`super::preview`]) | `preview_ready {container, render_hash, width, state, failed, rev, rendered_at_ms}`, then on `ready`: `render_ms`, `without_modifiers`, and when there: `look`, `reused_from`, `camera`, `base_values`, `hidden_left_out`, `ids`; on `failed`: `look` when there | the same |
//! | an agent's turn ended ([`super::turn::ended`]) | `turn_end {turn, actor, at_ms}` | every socket; each forwards it only for a turn its doc's changes carried |
//! | a comment op (`AuditEvent::Annotation`) | `roster_event {kind: "comment_event", thread, comment, op, author, at_ms, anchor}` | sockets on that doc, readers only |
//! | an anchor's state moved (`AuditEvent::AnchorState`) | `roster_event {kind: "anchor_state", thread, rev, was, now}` | the same |
//!
//! | an outside save of a **text** doc's file (`AuditEvent::ExternalEdit`, `FileMissing`, `FileDeclined`) | `outside_save {at_ms, doc, outcome, rev? / code, message, before_version}`, the keyed watch's frame (`ingested`, `missing`, `declined`) plus K2's `before_version` ([`version_at`]) | that text doc's sockets, readers only; a socket opened later gets the doc's last one ([`subscribe_replaying`], [`REPLAY_MS`]) |
//! | an agent's verb on a **text** doc refused (`POST /cli/docs/tool`) | `verb_refused {at_ms, actor, client, turn, verb, unit: null, code, holder, claim}`, the keyed watch's frame | the same |
//! | someone said something in the doc's live room (`POST /cli/docs/say`, [`super::say`]) | `say {id, from, to, text, at, time, reply_to, at_ms}` (`from`/`to` actor records; `to`, `at`, `time`, `reply_to` null when absent) | sockets on that doc, readers only |
//!
//! **Never content.** A preview names a hash, a turn its id and actor, a
//! comment its thread, op and author: a body is read on demand through the
//! rights-checked routes. (A `say` is the exception by design: the message
//! is the room's own, bounded, sent only to the doc's readers, never
//! logged.) (A scene view's `camera` is the projection its
//! still was drawn with, and its `ids` the ids of the objects in it, sent to
//! readers only, as the image is.)
//!
//! **Older clients.** `preview_rendering` is a frame type they don't know
//! (and skip); `preview_ready` keeps every field it had, in order, with the
//! new ones after them.
//!
//! **Off the session lock.** The kit calls [`super::host::K2Host::on_event`]
//! with the session locked: [`publish`] only pushes onto bounded queues
//! (`try_send`; a full queue drops the event for that socket: a missed
//! `preview_ready` leaves the old image `stale` until the view asks, a
//! missed `turn_end` is covered by `turn_idle_ms`).

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};

use k2_mpc_base::errors::Value;
use k2_mpc_base::store::DocKey;

/// Events queued per socket before new ones are dropped.
pub const QUEUE: usize = 256;

/// One host event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A preview of `container` at `render_hash` and `width` landed
    /// (`state: "ready"`) or failed (`"failed"`, with its code).
    PreviewReady {
        key: DocKey,
        container: String,
        render_hash: String,
        width: &'static str,
        state: &'static str,
        failed: Option<&'static str>,
        rev: u64,
        rendered_at_ms: u64,
        /// Fields after the old ones (`render_ms`, `without_modifiers`,
        /// `look`, `shape`, `reused_from`, `camera`, `base_values`,
        /// `hidden_left_out`, `ids`), in order.
        extras: Vec<(String, Value)>,
    },
    /// A preview of `container` at `render_hash`, `width`, `look` (left
    /// out: opaque) and `shape` (left out for a view that isn't shaped)
    /// started drawing.
    PreviewRendering { key: DocKey, container: String, render_hash: String, width: &'static str, look: Option<&'static str>, shape: Option<&'static str>, rev: u64 },
    /// An agent's turn ended (`actor`: its pid).
    TurnEnd { turn: String, actor: Option<String>, at_ms: u64 },
    /// A comment op on `key`'s doc: ids, op and author only.
    Comment { key: DocKey, thread: String, comment: Option<u32>, op: String, author: String, at_ms: u64, anchor: Option<String> },
    /// A thread's anchor changed state at `version`.
    AnchorState { key: DocKey, thread: String, version: u64, was: Option<String>, now: String },
    /// What came of an outside save of a **text** doc's file, as the keyed
    /// watch's `outside_save` frame: `ingested` (`rev`), `missing`, or
    /// `declined` (`code`). A keyed doc's watch gets the kit's own.
    /// `before_version`: the stored version holding the doc as it was just
    /// before the save ([`version_at`]), the id `GET /cli/docs/show`,
    /// `docs/diff` (`fromVersion`) and `POST /cli/docs/restore` take; `None`
    /// when there is none (a missing or declined file changes nothing, so
    /// the kit cuts no version for it).
    OutsideSave { key: DocKey, at_ms: u64, outcome: &'static str, rev: Option<u64>, code: Option<&'static str>, before_version: Option<u64> },
    /// An agent's verb on a **text** doc was refused (the tool route saw
    /// it), as the keyed watch's `verb_refused`: `actor` and `holder` pids,
    /// `claim` the refusing claim's id.
    VerbRefused {
        key: DocKey,
        at_ms: u64,
        actor: String,
        client: u64,
        turn: Option<String>,
        verb: String,
        code: String,
        holder: Option<String>,
        claim: Option<String>,
    },
    /// Someone watching the doc said something in its live room
    /// ([`super::say`]): to one participant (`to`, a pid) or the room.
    Say {
        key: DocKey,
        id: String,
        from: String,
        to: Option<String>,
        text: String,
        at: Option<String>,
        time: Option<String>,
        /// What the playhead is in (an address), when `time` named it.
        time_in: Option<String>,
        reply_to: Option<String>,
        at_ms: u64,
    },
}

impl HostEvent {
    /// The doc it is about (`None`: a turn end, for every socket).
    pub fn key(&self) -> Option<&DocKey> {
        match self {
            HostEvent::PreviewReady { key, .. }
            | HostEvent::PreviewRendering { key, .. }
            | HostEvent::Comment { key, .. }
            | HostEvent::AnchorState { key, .. }
            | HostEvent::OutsideSave { key, .. }
            | HostEvent::VerbRefused { key, .. }
            | HostEvent::Say { key, .. } => Some(key),
            HostEvent::TurnEnd { .. } => None,
        }
    }

    /// The socket frame. `author` turns a pid into the actor record
    /// `{id, name, kind, colour}` (the socket knows K2's names).
    pub fn frame(&self, author: impl Fn(&str) -> Value) -> Value {
        match self {
            HostEvent::PreviewReady { container, render_hash, width, state, failed, rev, rendered_at_ms, extras, .. } => {
                let mut f: Vec<(String, Value)> = vec![
                    ("type".into(), Value::from("preview_ready")),
                    ("container".into(), Value::from(container.as_str())),
                    ("render_hash".into(), Value::from(render_hash.as_str())),
                    ("width".into(), Value::from(*width)),
                    ("state".into(), Value::from(*state)),
                    ("failed".into(), failed.map_or(Value::Null, |c| Value::obj([("code", Value::from(c))]))),
                    ("rev".into(), Value::from(*rev)),
                    ("rendered_at_ms".into(), Value::from(*rendered_at_ms)),
                ];
                f.extend(extras.iter().cloned());
                Value::Obj(f)
            }
            HostEvent::PreviewRendering { container, render_hash, width, look, shape, rev, .. } => {
                let mut f: Vec<(String, Value)> = vec![
                    ("type".into(), Value::from("preview_rendering")),
                    ("container".into(), Value::from(container.as_str())),
                    ("render_hash".into(), Value::from(render_hash.as_str())),
                    ("width".into(), Value::from(*width)),
                ];
                if let Some(l) = look {
                    f.push(("look".into(), Value::from(*l)));
                }
                if let Some(s) = shape {
                    f.push(("shape".into(), Value::from(*s)));
                }
                f.push(("rev".into(), Value::from(*rev)));
                Value::Obj(f)
            }
            HostEvent::TurnEnd { turn, actor, at_ms } => Value::obj([
                ("type", Value::from("turn_end")),
                ("turn", Value::from(turn.as_str())),
                ("actor", actor.as_deref().map_or(Value::Null, &author)),
                ("at_ms", Value::from(*at_ms)),
            ]),
            HostEvent::Comment { thread, comment, op, author: by, at_ms, anchor, .. } => roster(Value::obj([
                ("kind", Value::from("comment_event")),
                ("thread", Value::from(thread.as_str())),
                ("comment", comment.map_or(Value::Null, Value::from)),
                ("op", Value::from(op.as_str())),
                ("author", author(by)),
                ("at_ms", Value::from(*at_ms)),
                ("anchor", anchor.as_deref().map_or(Value::Null, Value::from)),
            ])),
            HostEvent::AnchorState { thread, version, was, now, .. } => roster(Value::obj([
                ("kind", Value::from("anchor_state")),
                ("thread", Value::from(thread.as_str())),
                ("rev", Value::from(*version)),
                ("was", was.as_deref().map_or(Value::Null, Value::from)),
                ("now", Value::from(now.as_str())),
            ])),
            // The kit's `outside_save` (client/WIRE.md "Outside saves"),
            // field for field, then K2's `before_version` (always there,
            // `null` when unknown: an older client ignores it).
            HostEvent::OutsideSave { key, at_ms, outcome, rev, code, before_version } => {
                let mut f: Vec<(String, Value)> = vec![
                    ("type".into(), Value::from("outside_save")),
                    ("at_ms".into(), Value::from(*at_ms)),
                    ("doc".into(), Value::from(key.path.as_str())),
                    ("outcome".into(), Value::from(*outcome)),
                ];
                if let Some(r) = rev {
                    f.push(("rev".into(), Value::from(*r)));
                }
                if let Some(c) = code {
                    f.push(("code".into(), Value::from(*c)));
                    f.push(("message".into(), Value::from(declined_message(c))));
                }
                f.push(("before_version".into(), before_version.map_or(Value::Null, Value::from)));
                Value::Obj(f)
            }
            // The kit's `verb_refused` (no unit: a text verb names lines).
            HostEvent::VerbRefused { at_ms, actor, client, turn, verb, code, holder, claim, .. } => Value::obj([
                ("type", Value::from("verb_refused")),
                ("at_ms", Value::from(*at_ms)),
                ("actor", author(actor)),
                ("client", Value::from(*client)),
                ("turn", turn.as_deref().map_or(Value::Null, Value::from)),
                ("verb", Value::from(verb.as_str())),
                ("unit", Value::Null),
                ("code", Value::from(code.as_str())),
                ("holder", holder.as_deref().map_or(Value::Null, &author)),
                ("claim", claim.as_deref().map_or(Value::Null, Value::from)),
            ]),
            HostEvent::Say { id, from, to, text, at, time, time_in, reply_to, at_ms, .. } => {
                let mut f: Vec<(String, Value)> = vec![
                    ("type".into(), Value::from("say")),
                    ("id".into(), Value::from(id.as_str())),
                    ("from".into(), author(from)),
                    ("to".into(), to.as_deref().map_or(Value::Null, &author)),
                    ("text".into(), Value::from(text.as_str())),
                    ("at".into(), at.as_deref().map_or(Value::Null, Value::from)),
                    ("time".into(), time.as_deref().map_or(Value::Null, Value::from)),
                ];
                // Only when the time named what it is in (`{at, in}`).
                if let Some(w) = time_in {
                    f.push(("time_in".into(), Value::from(w.as_str())));
                }
                f.push(("reply_to".into(), reply_to.as_deref().map_or(Value::Null, Value::from)));
                f.push(("at_ms".into(), Value::from(*at_ms)));
                Value::Obj(f)
            }
        }
    }
}

/// A declined file in plain words (the kit's audit event has the code only).
pub fn declined_message(code: &str) -> String {
    format!("the file can't be a live doc any more ({code}); the last live version is in its saved versions")
}

/// Whether `key` is a text doc (Markdown or the text pack): the docs whose
/// sockets get K2's own `outside_save` and `verb_refused` (a keyed doc's
/// watch gets the kit's).
pub fn is_text_doc(key: &DocKey) -> bool {
    matches!(super::target::Tier::of(&key.path), super::target::Tier::Markdown | super::target::Tier::Text(_))
}

/// An outside save of a text doc's file, to its sockets (called by the
/// host's audit hook, under the session lock: a queue push only), and kept
/// as the doc's last one for a socket opened later ([`subscribe_replaying`]). Nothing
/// for a keyed or any-file doc.
pub fn outside_save_on_text(
    key: &DocKey,
    outcome: &'static str,
    rev: Option<u64>,
    code: Option<&'static str>,
    before_version: Option<u64>,
    at_ms: u64,
) {
    if !is_text_doc(key) {
        return;
    }
    let ev = HostEvent::OutsideSave { key: key.clone(), at_ms, outcome, rev, code, before_version };
    // Kept, then published, under the kept list's lock: a socket subscribing
    // meanwhile gets this save once, live or replayed (never both).
    let mut k = last_saves().lock().unwrap_or_else(|p| p.into_inner());
    k.retain(|e| e.key() != Some(key));
    if k.len() >= KEPT_SAVES {
        k.pop_front();
    }
    k.push_back(ev.clone());
    publish(ev);
}

/// The stored version holding `key`'s doc exactly as it was at change `at`
/// (the newest one there), when the store has one. An outside save of a
/// text doc is change `seq` = the doc's version + 1, and the kit cuts a
/// version at the doc's version first (`VersionReason::BeforeExternal`,
/// unless one is already there) in the same commit, before its audit event
/// (kit `Session::file_changed`): `version_at(seq - 1)` is that version.
/// Reads the store only (a redb read never waits behind the session lock).
pub fn version_at(store: &k2_mpc_base::store::DocStore, key: &DocKey, at: u64) -> Option<u64> {
    let id = store.find(key).ok()??;
    store.version_infos(id).ok()?.into_iter().filter(|v| v.seq == at).map(|v| v.id).max()
}

/// Text docs whose last outside save is kept for sockets opened later.
pub const KEPT_SAVES: usize = 32;

/// How long a doc's last outside save is replayed to a socket opened after
/// it (the merged notice's Compare and Undo their save, in a tab that joined
/// late). Older ones aren't: the tab opens on the doc as it is.
pub const REPLAY_MS: u64 = 30 * 60_000;

/// Each text doc's last `OutsideSave` (oldest doc first), in memory: a
/// restart forgets them.
fn last_saves() -> &'static Mutex<VecDeque<HostEvent>> {
    static K: OnceLock<Mutex<VecDeque<HostEvent>>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// The file is back (`AuditEvent::FileRestored`): a kept `missing` or
/// `declined` no longer says how the doc is, so a later socket isn't told.
pub fn file_restored(key: &DocKey) {
    last_saves()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|e| !(e.key() == Some(key) && matches!(e, HostEvent::OutsideSave { outcome: "missing" | "declined", .. })));
}

/// The doc was renamed or purged: its kept outside save goes.
pub fn forget_saves(key: &DocKey) {
    last_saves().lock().unwrap_or_else(|p| p.into_inner()).retain(|e| e.key() != Some(key));
}

/// `key`'s last outside save, for a socket opening now: kept, at most
/// [`REPLAY_MS`] old, and not one a resuming client already has (an
/// `ingested` save at or before its `resume` version).
fn replay_for(kept: &VecDeque<HostEvent>, key: &DocKey, resume: Option<u64>, now_ms: u64) -> Option<HostEvent> {
    let ev = kept.iter().rev().find(|e| e.key() == Some(key))?;
    let HostEvent::OutsideSave { at_ms, rev, .. } = ev else { return None };
    if now_ms.saturating_sub(*at_ms) > REPLAY_MS {
        return None;
    }
    if let (Some(v), Some(r)) = (resume, *rev) {
        if r <= v {
            return None;
        }
    }
    Some(ev.clone())
}

fn roster(event: Value) -> Value {
    Value::obj([("type", Value::from("roster_event")), ("event", event)])
}

struct Sub {
    /// The doc this socket is on.
    key: DocKey,
    tx: SyncSender<HostEvent>,
}

fn subs() -> &'static Mutex<HashMap<u64, Sub>> {
    static S: OnceLock<Mutex<HashMap<u64, Sub>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// A socket's subscription: its doc's events and every turn end. Dropping
/// it unsubscribes.
pub struct Subscription {
    id: u64,
    pub rx: Receiver<HostEvent>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        subs().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.id);
    }
}

/// Subscribe a socket on `key`'s doc.
pub fn subscribe(key: &DocKey) -> Subscription {
    let (tx, rx) = sync_channel(QUEUE);
    let id = next_id();
    subs().lock().unwrap_or_else(|p| p.into_inner()).insert(id, Sub { key: key.clone(), tx });
    Subscription { id, rx }
}

/// [`subscribe`] for a live socket opening now: a text doc's last outside
/// save, when it is recent ([`REPLAY_MS`]), is queued first, so a tab
/// opened after the save still gets its `outside_save` (with
/// `before_version`), after the welcome like any host event. `resume`: the
/// client's version on a reconnect (a save it already has isn't sent
/// again); `now_ms` the host clock.
pub fn subscribe_replaying(key: &DocKey, resume: Option<u64>, now_ms: u64) -> Subscription {
    let (tx, rx) = sync_channel(QUEUE);
    let id = next_id();
    // The kept list's lock first, as `outside_save_on_text` takes them: a
    // save made meanwhile reaches this socket once, replayed or live.
    let kept = last_saves().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ev) = replay_for(&kept, key, resume, now_ms) {
        let _ = tx.try_send(ev);
    }
    subs().lock().unwrap_or_else(|p| p.into_inner()).insert(id, Sub { key: key.clone(), tx });
    drop(kept);
    Subscription { id, rx }
}

/// The doc's events to its sockets; a turn end to every socket. Never
/// blocks. Returns how many queues took it.
pub fn publish(ev: HostEvent) -> usize {
    let mut s = subs().lock().unwrap_or_else(|p| p.into_inner());
    let mut gone = Vec::new();
    let mut n = 0;
    for (id, sub) in s.iter() {
        if ev.key().is_some_and(|k| *k != sub.key) {
            continue;
        }
        match sub.tx.try_send(ev.clone()) {
            Ok(()) => n += 1,
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => gone.push(*id),
        }
    }
    for id in gone {
        s.remove(&id);
    }
    n
}

/// [`super::turn::ended`]'s event.
pub fn publish_turn_end(turn: &str, actor: Option<&str>, at_ms: u64) -> usize {
    publish(HostEvent::TurnEnd { turn: turn.to_string(), actor: actor.map(str::to_string), at_ms })
}

/// How many sockets are subscribed (tests and status).
pub fn subscribers() -> usize {
    subs().lock().unwrap_or_else(|p| p.into_inner()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(space: &str, path: &str) -> DocKey {
        DocKey { space: space.into(), path: path.into() }
    }

    fn ready(k: &DocKey) -> HostEvent {
        HostEvent::PreviewReady {
            key: k.clone(),
            container: "@sld:256".into(),
            render_hash: "ab12".into(),
            width: "thumb",
            state: "ready",
            failed: None,
            rev: 7,
            rendered_at_ms: 1_000,
            extras: Vec::new(),
        }
    }

    #[test]
    fn a_docs_events_reach_only_its_sockets_and_turn_ends_reach_all() {
        let a = key("host-events-ws", "a.pptx");
        let b = key("host-events-ws", "b.pptx");
        let on_a = subscribe(&a);
        let on_b = subscribe(&b);
        assert!(publish(ready(&a)) >= 1);
        assert_eq!(on_a.rx.try_recv().expect("a's socket"), ready(&a));
        assert!(on_b.rx.try_recv().is_err(), "b's socket never hears a's preview");
        publish_turn_end("t:00000000000000aa.1", Some("agent:p1"), 5);
        for s in [&on_a, &on_b] {
            assert!(matches!(s.rx.try_recv().expect("a turn end"), HostEvent::TurnEnd { ref turn, .. } if turn == "t:00000000000000aa.1"));
        }
        let before = subscribers();
        drop(on_a);
        assert_eq!(subscribers(), before - 1, "dropping unsubscribes");
    }

    #[test]
    fn a_full_queue_drops_without_blocking() {
        let k = key("host-events-full", "deck.pptx");
        let s = subscribe(&k);
        for _ in 0..QUEUE + 10 {
            publish(ready(&k));
        }
        assert_eq!(s.rx.try_iter().count(), QUEUE);
    }

    #[test]
    fn frames_carry_ids_and_never_content() {
        let k = key("host-events-frames", "deck.pptx");
        let who = |pid: &str| Value::obj([("id", Value::from(pid))]);
        assert_eq!(
            ready(&k).frame(who).to_json(),
            r#"{"type":"preview_ready","container":"@sld:256","render_hash":"ab12","width":"thumb","state":"ready","failed":null,"rev":7,"rendered_at_ms":1000}"#
        );
        let failed = HostEvent::PreviewReady {
            key: k.clone(),
            container: "@sld:256".into(),
            render_hash: "ab12".into(),
            width: "full",
            state: "failed",
            failed: Some("timeout"),
            rev: 7,
            rendered_at_ms: 1_000,
            extras: Vec::new(),
        };
        assert!(failed.frame(who).to_json().contains(r#""state":"failed","failed":{"code":"timeout"}"#));
        assert_eq!(
            HostEvent::TurnEnd { turn: "t:1.2".into(), actor: Some("agent:p1".into()), at_ms: 9 }.frame(who).to_json(),
            r#"{"type":"turn_end","turn":"t:1.2","actor":{"id":"agent:p1"},"at_ms":9}"#
        );
        let c = HostEvent::Comment {
            key: k.clone(),
            thread: "ann-3".into(),
            comment: Some(1),
            op: "replied".into(),
            author: "person:ada".into(),
            at_ms: 4,
            anchor: Some("@sld:256".into()),
        };
        assert_eq!(
            c.frame(who).to_json(),
            r#"{"type":"roster_event","event":{"kind":"comment_event","thread":"ann-3","comment":1,"op":"replied","author":{"id":"person:ada"},"at_ms":4,"anchor":"@sld:256"}}"#
        );
        let st = HostEvent::AnchorState { key: k, thread: "ann-3".into(), version: 12, was: Some("attached".into()), now: "orphaned".into() };
        assert_eq!(
            st.frame(who).to_json(),
            r#"{"type":"roster_event","event":{"kind":"anchor_state","thread":"ann-3","rev":12,"was":"attached","now":"orphaned"}}"#
        );
    }

    /// A text doc's outside save and refused verb go out as the keyed
    /// watch's frames, field for field (client/WIRE.md "Outside saves",
    /// `verb_refused`); a deck's aren't published (its watch has the kit's).
    #[test]
    fn a_text_docs_outside_save_and_refused_verb_take_the_keyed_watchs_shape() {
        let md = key("host-events-text", "notes/plan.md");
        let deck = key("host-events-text", "talk.pptx");
        let (on_md, on_deck) = (subscribe(&md), subscribe(&deck));
        assert!(is_text_doc(&md) && is_text_doc(&key("w", "src/main.rs")) && !is_text_doc(&deck) && !is_text_doc(&key("w", "a.png")));
        outside_save_on_text(&md, "ingested", Some(12), None, Some(4), 1_700_000_000_000);
        outside_save_on_text(&deck, "ingested", Some(3), None, None, 1_700_000_000_000);
        let ev = on_md.rx.try_recv().expect("the text doc's socket");
        let name = |pid: &str| Value::obj([("id", Value::from(pid))]);
        assert_eq!(
            ev.frame(name).to_json(),
            r#"{"type":"outside_save","at_ms":1700000000000,"doc":"notes/plan.md","outcome":"ingested","rev":12,"before_version":4}"#
        );
        assert!(on_deck.rx.try_recv().is_err(), "a keyed doc's watch has the kit's own frame");
        outside_save_on_text(&md, "missing", None, None, None, 5);
        assert_eq!(
            on_md.rx.try_recv().expect("missing").frame(name).to_json(),
            r#"{"type":"outside_save","at_ms":5,"doc":"notes/plan.md","outcome":"missing","before_version":null}"#
        );
        outside_save_on_text(&md, "declined", None, Some("too_large"), None, 6);
        let f = on_md.rx.try_recv().expect("declined").frame(name).to_json();
        assert!(
            f.starts_with(r#"{"type":"outside_save","at_ms":6,"doc":"notes/plan.md","outcome":"declined","code":"too_large","message":"the file can't be a live doc any more (too_large)"#),
            "{f}"
        );
        assert!(f.ends_with(r#","before_version":null}"#), "{f}");

        let refused = HostEvent::VerbRefused {
            key: md.clone(),
            at_ms: 9,
            actor: "agent:atlas".into(),
            client: 4,
            turn: Some("t:ab.1".into()),
            verb: "replace-section".into(),
            code: "MPC-E301".into(),
            holder: Some("agent:brook".into()),
            claim: Some("claim-7".into()),
        };
        assert_eq!(refused.key(), Some(&md));
        assert_eq!(
            refused.frame(name).to_json(),
            r#"{"type":"verb_refused","at_ms":9,"actor":{"id":"agent:atlas"},"client":4,"turn":"t:ab.1","verb":"replace-section","unit":null,"code":"MPC-E301","holder":{"id":"agent:brook"},"claim":"claim-7"}"#
        );
    }

    /// A socket opened after an outside save gets the doc's last one first
    /// (with `before_version`) while it is recent; never a deck's, never one
    /// a resuming client already has, never after the file came back
    /// (missing) or the doc moved.
    #[test]
    fn a_socket_opened_after_an_outside_save_gets_the_last_one() {
        let md = key("host-events-late", "notes/late.md");
        let deck = key("host-events-late", "talk.pptx");
        let t = 1_700_000_000_000;
        assert!(subscribe_replaying(&md, None, t).rx.try_recv().is_err(), "no save yet");
        outside_save_on_text(&md, "ingested", Some(8), None, Some(3), t);
        outside_save_on_text(&md, "ingested", Some(12), None, Some(5), t + 10);
        outside_save_on_text(&deck, "ingested", Some(2), None, None, t);
        let late = subscribe_replaying(&md, None, t + 1_000);
        let ev = late.rx.try_recv().expect("the last save, replayed");
        let name = |pid: &str| Value::obj([("id", Value::from(pid))]);
        assert_eq!(
            ev.frame(name).to_json(),
            r#"{"type":"outside_save","at_ms":1700000000010,"doc":"notes/late.md","outcome":"ingested","rev":12,"before_version":5}"#,
            "the newest save only, as it was sent"
        );
        assert!(late.rx.try_recv().is_err(), "once");
        assert!(subscribe(&md).rx.try_recv().is_err(), "a plain subscription replays nothing");
        assert!(subscribe_replaying(&deck, None, t).rx.try_recv().is_err(), "a deck's watch has the kit's own");
        assert!(subscribe_replaying(&md, Some(12), t + 1_000).rx.try_recv().is_err(), "a resume at the save has it");
        assert!(subscribe_replaying(&md, Some(11), t + 1_000).rx.try_recv().is_ok(), "a resume before the save gets it");
        assert!(subscribe_replaying(&md, None, t + 10 + REPLAY_MS + 1).rx.try_recv().is_err(), "too old");

        // A newer save after the subscription arrives live, after the replay.
        let both = subscribe_replaying(&md, None, t + 1_000);
        outside_save_on_text(&md, "missing", None, None, None, t + 2_000);
        let got: Vec<&'static str> = both
            .rx
            .try_iter()
            .map(|e| match e {
                HostEvent::OutsideSave { outcome, .. } => outcome,
                other => panic!("not an outside save: {other:?}"),
            })
            .collect();
        assert_eq!(got, ["ingested", "missing"]);
        assert!(matches!(
            subscribe_replaying(&md, None, t + 3_000).rx.try_recv(),
            Ok(HostEvent::OutsideSave { outcome: "missing", before_version: None, .. })
        ));
        file_restored(&md);
        assert!(subscribe_replaying(&md, None, t + 3_000).rx.try_recv().is_err(), "the file is back");

        outside_save_on_text(&md, "ingested", Some(20), None, Some(9), t + 4_000);
        file_restored(&md);
        assert!(subscribe_replaying(&md, None, t + 5_000).rx.try_recv().is_ok(), "a merged save stays after a restore");
        forget_saves(&md);
        assert!(subscribe_replaying(&md, None, t + 5_000).rx.try_recv().is_err(), "moved or purged");
    }
}

/// The version id through the kit itself: K2's audit hook on a real outside
/// save, a socket subscribing after it, and that version read back.
#[cfg(test)]
#[path = "outside_save_tests.rs"]
mod outside_save_tests;
