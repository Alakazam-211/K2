//! Host events on the docs socket (rendering-data contract r1.4, gap 26 and
//! the relay half of gaps 29 and 30): what K2 itself knows about a doc and
//! the kit's change stream doesn't carry.
//!
//! | Event | Frame | Who gets it |
//! |---|---|---|
//! | a preview started drawing ([`super::preview`]) | `preview_rendering {container, render_hash, width, look?, rev}` | sockets on that doc; each forwards it only while its person may read the doc |
//! | a preview rendered or failed, or a scene view's objects renamed (its image again, same render hash, `ids` named anew) ([`super::preview`]) | `preview_ready {container, render_hash, width, state, failed, rev, rendered_at_ms}`, then on `ready`: `render_ms`, `without_modifiers`, and when there: `look`, `reused_from`, `camera`, `ids`; on `failed`: `look` when there | the same |
//! | an agent's turn ended ([`super::turn::ended`]) | `turn_end {turn, actor, at_ms}` | every socket; each forwards it only for a turn its doc's changes carried |
//! | a comment op (`AuditEvent::Annotation`) | `roster_event {kind: "comment_event", thread, comment, op, author, at_ms, anchor}` | sockets on that doc, readers only |
//! | an anchor's state moved (`AuditEvent::AnchorState`) | `roster_event {kind: "anchor_state", thread, rev, was, now}` | the same |
//!
//! | an outside save of a **text** doc's file (`AuditEvent::ExternalEdit`, `FileMissing`, `FileDeclined`) | `outside_save {at_ms, doc, outcome, rev? / code, message}`, the keyed watch's frame (`ingested`, `missing`, `declined`) | that text doc's sockets, readers only |
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

use std::collections::HashMap;
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
        /// `look`, `reused_from`, `camera`, `ids`), in order.
        extras: Vec<(String, Value)>,
    },
    /// A preview of `container` at `render_hash`, `width` and `look` (left
    /// out: opaque) started drawing.
    PreviewRendering { key: DocKey, container: String, render_hash: String, width: &'static str, look: Option<&'static str>, rev: u64 },
    /// An agent's turn ended (`actor`: its pid).
    TurnEnd { turn: String, actor: Option<String>, at_ms: u64 },
    /// A comment op on `key`'s doc: ids, op and author only.
    Comment { key: DocKey, thread: String, comment: Option<u32>, op: String, author: String, at_ms: u64, anchor: Option<String> },
    /// A thread's anchor changed state at `version`.
    AnchorState { key: DocKey, thread: String, version: u64, was: Option<String>, now: String },
    /// What came of an outside save of a **text** doc's file, as the keyed
    /// watch's `outside_save` frame: `ingested` (`rev`), `missing`, or
    /// `declined` (`code`). A keyed doc's watch gets the kit's own.
    OutsideSave { key: DocKey, at_ms: u64, outcome: &'static str, rev: Option<u64>, code: Option<&'static str> },
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
            HostEvent::PreviewRendering { container, render_hash, width, look, rev, .. } => {
                let mut f: Vec<(String, Value)> = vec![
                    ("type".into(), Value::from("preview_rendering")),
                    ("container".into(), Value::from(container.as_str())),
                    ("render_hash".into(), Value::from(render_hash.as_str())),
                    ("width".into(), Value::from(*width)),
                ];
                if let Some(l) = look {
                    f.push(("look".into(), Value::from(*l)));
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
            // field for field.
            HostEvent::OutsideSave { key, at_ms, outcome, rev, code } => {
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
            HostEvent::Say { id, from, to, text, at, time, reply_to, at_ms, .. } => Value::obj([
                ("type", Value::from("say")),
                ("id", Value::from(id.as_str())),
                ("from", author(from)),
                ("to", to.as_deref().map_or(Value::Null, &author)),
                ("text", Value::from(text.as_str())),
                ("at", at.as_deref().map_or(Value::Null, Value::from)),
                ("time", time.as_deref().map_or(Value::Null, Value::from)),
                ("reply_to", reply_to.as_deref().map_or(Value::Null, Value::from)),
                ("at_ms", Value::from(*at_ms)),
            ]),
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
/// host's audit hook, under the session lock: a queue push only). Nothing
/// for a keyed or any-file doc.
pub fn outside_save_on_text(key: &DocKey, outcome: &'static str, rev: Option<u64>, code: Option<&'static str>, at_ms: u64) {
    if is_text_doc(key) {
        publish(HostEvent::OutsideSave { key: key.clone(), at_ms, outcome, rev, code });
    }
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
        outside_save_on_text(&md, "ingested", Some(12), None, 1_700_000_000_000);
        outside_save_on_text(&deck, "ingested", Some(3), None, 1_700_000_000_000);
        let ev = on_md.rx.try_recv().expect("the text doc's socket");
        let name = |pid: &str| Value::obj([("id", Value::from(pid))]);
        assert_eq!(ev.frame(name).to_json(), r#"{"type":"outside_save","at_ms":1700000000000,"doc":"notes/plan.md","outcome":"ingested","rev":12}"#);
        assert!(on_deck.rx.try_recv().is_err(), "a keyed doc's watch has the kit's own frame");
        outside_save_on_text(&md, "missing", None, None, 5);
        assert_eq!(on_md.rx.try_recv().expect("missing").frame(name).to_json(), r#"{"type":"outside_save","at_ms":5,"doc":"notes/plan.md","outcome":"missing"}"#);
        outside_save_on_text(&md, "declined", None, Some("too_large"), 6);
        let f = on_md.rx.try_recv().expect("declined").frame(name).to_json();
        assert!(
            f.starts_with(r#"{"type":"outside_save","at_ms":6,"doc":"notes/plan.md","outcome":"declined","code":"too_large","message":"the file can't be a live doc any more (too_large)"#),
            "{f}"
        );

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
}
