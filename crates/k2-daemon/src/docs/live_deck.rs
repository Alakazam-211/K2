//! The docs socket on a live deck (`WS /cli/docs/live?doc=talk.pptx`, the
//! Slides core on): a person **watches** a deck that agents change with
//! `k2 doc` (contract r1.4 §3: people don't write decks live in v1).
//!
//! **Timelines and scenes** take the same socket: an `.otio` with the
//! timeline core on (`root: "tracks"`, `family: "timeline"`, `mode:
//! "timeline"`) and a `.blend` with the blend core on (`root: "colls"`,
//! `family: "scene"`, `mode: "scene"`). A timeline's welcome `features`
//! lists `keyed_watch`, `view_address` and `time_position` (a playhead to
//! follow); a scene's `keyed_watch`, `view_address` and `presence` (the
//! `select`, `pointer` and `viewport` frames; a scene has no time). A scene
//! is read-only for now (`blend_read_only`): its `rights` is `read` and the
//! welcome's `read_only_reason` says why; once people may change one it
//! also lists `claims` (the `claim` and `release` frames,
//! WIRE.md "Claim frames": claim objects or collections by key or address
//! while moving them; the socket takes them already, a claim needs the edit
//! right). `previews` is a scene's cameras and objects (`null` for a
//! timeline: no lane tiles on any host yet). Everything below holds for all
//! three.
//! The URL's `root` may be the doc's own container root or `source` (what
//! the text views send), or left out.
//!
//! **What it sends** (wire 3, `welcome.wire`):
//! - `welcome {root: "slides", version, client, self, rights, wire, core,
//!   family: "deck", path, turn_idle_ms}`, then K2's deck extras: `mode:
//!   "deck"`, `previews` (the containers at the head with their render
//!   hashes: `{kind, rev, page, containers: [{key, position, title, hidden,
//!   render_hash, ...the type's extras}]}`, or `null` when previews aren't
//!   available here), and
//!   `comments` (the snapshot of open threads: `{threads, next}`; live docs
//!   carry no comments in kit v0.15.x, so it is empty until the kit adds
//!   them, gap 29);
//! - every change (`change`, with `turn` and `verb`; a deck's changes have
//!   no hunks: what they did to the store comes with the kit's keyed watch
//!   stream, see below), every roster event (claims with their actor and
//!   times, element presence, `watching`), and the host events:
//!   `preview_ready` (only while this person may read the doc), `turn_end`
//!   (only for a turn this doc's changes carried), comment and anchor
//!   events.
//!
//! **What it takes:** `since`, `visible`, `hello`, the keyed watch's
//! `watch` and `report`, and the presence frames `view_address` and
//! `time_position` (kit v0.18.0: the welcome's `features` lists both, for
//! every person on the socket, so Follow can follow a view and a playhead). Every frame that writes or reads text (`edit`,
//! `undo`, `redo`, `snapshot`, `cursor`, `view`, `take_notes`) is refused,
//! `MPC-E113`, with its `op` when it has one.
//!
//! **The keyed watch** (kit v0.16, [`super::live_watch`]): the welcome
//! lists `"keyed_watch"` in `features` when the deck's session is keyed
//! watchable. A client that sends `watch` gets `keyed_snapshot` and its
//! `keyed_page`s, then one `ops_change` per change (its `render_hashes` and
//! `containers` say which slides to re-request); its socket stops sending
//! the bare `change` frames. A client that never sends `watch` (an older
//! build) keeps this path as it was: the welcome's `previews`, `change`
//! frames and `preview_ready`. The host events and the preview service stay
//! as they are for both.

use std::collections::HashSet;
use std::sync::Arc;

use k2_mpc_base::errors::Value;
use k2_mpc_base::session::{ConnId, Session};
use k2_mpc_base::store::DocKey;

use super::caller::DocCaller;
use super::host::{self, Principal};
use super::host_events::HostEvent;
use super::live_wire::{self as wire, ClientFrame};
use super::preview::{self, service};
use super::target::Target;

/// The live deck's container root (the pptx pack's `slides`).
pub const ROOT: &str = "slides";

/// A live timeline's container root (the timeline pack's view: `tracks`).
pub const TIMELINE_ROOT: &str = k2_mpc_timeline::live::TRACKS;

/// A live scene's container root (the BLEND pack's view: `colls`).
pub const SCENE_ROOT: &str = k2_mpc_blend::live::COLLS;

/// The welcome's `root` for a keyed tier: its view's container root.
pub fn root_of(tier: super::target::Tier) -> &'static str {
    match tier {
        super::target::Tier::Timeline => TIMELINE_ROOT,
        super::target::Tier::Scene => SCENE_ROOT,
        _ => ROOT,
    }
}

/// The welcome's `mode` for a keyed tier: `deck`, `timeline` or `scene`.
pub fn mode_of(tier: super::target::Tier) -> &'static str {
    match tier {
        super::target::Tier::Timeline => "timeline",
        super::target::Tier::Scene => "scene",
        _ => "deck",
    }
}

/// The `root` a keyed doc's socket takes: none, its own container root
/// ([`root_of`]), or `source`, the root the text views send.
pub fn root_ok_for(tier: super::target::Tier, root: Option<&str>) -> bool {
    root.is_none_or(|r| r == root_of(tier) || r == super::live::ROOT)
}

/// The colour K2 reserves for the file itself (an outside save,
/// `kit:external`), contract Item 2.
pub const EXTERNAL_COLOUR: &str = "#64748b";

/// Most turn ids a socket remembers (for `turn_end`).
pub const TURNS_KEPT: usize = 256;


/// The deck's extras for the welcome, after the wire's fields.
pub fn welcome_extras(t: &Target, session: &Session, conn: ConnId) -> Vec<(String, Value)> {
    let previews = match (super::runtime().ok().and_then(|rt| rt.blobs()), preview::renderer_for(t)) {
        (Some(blobs), Some(r)) => match service::head(&blobs, r, &t.key(), session, conn) {
            Ok(s) => previews_value(&s),
            Err(e) => {
                k2_core::log_debug!("[docs/live] no previews for {}: {e:?}", host::key_label(&t.key()));
                Value::Null
            }
        },
        _ => Value::Null,
    };
    vec![
        ("mode".into(), Value::from(mode_of(t.tier))),
        ("previews".into(), previews),
        ("comments".into(), Value::obj([("threads", Value::List(Vec::new())), ("next", Value::Null)])),
    ]
}

/// A snapshot's containers as the welcome carries them.
pub fn previews_value(s: &service::Snapshot) -> Value {
    let page = s.page.map_or(Value::Null, |p| {
        Value::obj([("w", Value::from(p.w)), ("h", Value::from(p.h)), ("unit", Value::from(p.unit))])
    });
    let containers = s
        .containers
        .iter()
        .map(|k| {
            let mut fields: Vec<(String, Value)> = vec![
                ("key".into(), Value::from(k.c.key.as_str())),
                ("position".into(), Value::from(k.c.position)),
                ("title".into(), Value::from(k.c.title.as_str())),
                ("hidden".into(), Value::from(k.c.hidden)),
                ("render_hash".into(), Value::from(k.hash.as_str())),
            ];
            // The type's extras after them (a scene's bounds and camera display values; none for a slide).
            fields.extend(preview::extras::wire_fields(&k.c.extras));
            Value::Obj(fields)
        })
        .collect();
    Value::obj([
        ("kind", Value::from(s.renderer.kind())),
        ("rev", Value::from(s.rev)),
        ("page", page),
        ("containers", Value::List(containers)),
    ])
}

/// The reply to a client frame a deck socket doesn't take; `None` for the
/// frames it takes (`since`, `visible`, `hello`).
pub fn refuse(f: &ClientFrame) -> Option<Value> {
    let (op, what) = match f {
        ClientFrame::Edit { op, .. } => (Some(*op), "edit"),
        ClientFrame::Undo { op } => (Some(*op), "undo"),
        ClientFrame::Redo { op } => (Some(*op), "redo"),
        ClientFrame::Snapshot { op } => (Some(*op), "snapshot"),
        ClientFrame::Cursor { .. } => (None, "cursor"),
        ClientFrame::View { .. } => (None, "view"),
        ClientFrame::TakeNotes => (None, "take_notes"),
        ClientFrame::Since { .. }
        | ClientFrame::Visible(_)
        | ClientFrame::Hello
        | ClientFrame::Watch
        | ClientFrame::Report { .. }
        | ClientFrame::ViewAddress { .. }
        | ClientFrame::TimePosition { .. }
        | ClientFrame::Claim { .. }
        | ClientFrame::Release { .. }
        | ClientFrame::Select { .. }
        | ClientFrame::Pointer { .. }
        | ClientFrame::Viewport { .. } => return None,
    };
    let e = wire::unreadable(&format!(
        "`{what}` isn't taken on a deck, timeline or scene: people watch a live keyed doc, agents change it with k2 doc"
    ));
    Some(match op {
        Some(op) => wire::refused_frame(op, &e),
        None => wire::error_frame(&e),
    })
}

/// A pid as the frames' actor record `{id, name, kind, colour}`, from K2's
/// own records (never the request).
pub fn actor_record(pid: &str) -> Value {
    if pid == k2_mpc_base::session::EXTERNAL_PID {
        return Value::obj([
            ("id", Value::from(pid)),
            ("name", Value::from("the file")),
            ("kind", Value::from("file")),
            ("colour", Value::from(EXTERNAL_COLOUR)),
        ]);
    }
    let caller = match Principal::parse(pid) {
        Some(Principal::Person(key)) => DocCaller::Person { key, via: super::caller::VIA_CLI },
        Some(Principal::Agent(project_id)) => DocCaller::Agent { project_id, session: None },
        None => {
            return Value::obj([("id", Value::from(pid)), ("name", Value::from(pid)), ("kind", Value::from("other")), ("colour", Value::Null)])
        }
    };
    let p = caller.participant(None);
    Value::obj([
        ("id", Value::from(pid)),
        ("name", Value::from(p.name.as_str())),
        ("kind", Value::from(wire::kind(p.kind))),
        ("colour", Value::from(p.colour.as_str())),
    ])
}

/// Turns this socket's doc carried (from its change frames), for `turn_end`.
#[derive(Default)]
pub struct Turns(HashSet<String>, std::collections::VecDeque<String>);

impl Turns {
    pub fn saw(&mut self, turn: &str) {
        if self.0.insert(turn.to_string()) {
            self.1.push_back(turn.to_string());
            if self.1.len() > TURNS_KEPT {
                if let Some(old) = self.1.pop_front() {
                    self.0.remove(&old);
                }
            }
        }
    }

    pub fn has(&self, turn: &str) -> bool {
        self.0.contains(turn)
    }
}

/// The frame for a host event on this socket, or `None` when it isn't for
/// this person: a doc event needs read on the doc now (asked fresh), a turn
/// end a turn this doc saw.
pub fn host_event_frame(ev: &HostEvent, pid: &str, key: &DocKey, turns: &Turns) -> Option<Value> {
    match ev {
        HostEvent::TurnEnd { turn, .. } if !turns.has(turn) => return None,
        HostEvent::TurnEnd { .. } => {}
        _ => {
            if ev.key() != Some(key) {
                return None;
            }
            let store = super::runtime().ok().and_then(|rt| rt.store());
            if !host::rights_for(pid, key, store.as_deref()).read {
                return None;
            }
        }
    }
    Some(ev.frame(actor_record))
}

/// A deck's or a scene's change was forwarded: the preview service lists it
/// again after the burst, through this socket's connection, and re-draws
/// its dirty slides, cameras and objects. A timeline has no previews here.
pub fn changed(t_key: &DocKey, rel: &str, session: &Arc<Session>, conn: ConnId) {
    let Some(blobs) = super::runtime().ok().and_then(|rt| rt.blobs()) else { return };
    let renderer: &'static dyn preview::PreviewRenderer = match super::target::tier_now(rel) {
        super::target::Tier::Deck => &preview::deck::DECK,
        super::target::Tier::Scene => &preview::scene::SCENE,
        _ => return,
    };
    service::doc_changed(blobs, renderer, t_key, session.clone(), conn);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deck_socket_refuses_text_frames_with_their_op() {
        let edit = ClientFrame::Edit { op: 4, root: "source".into(), base: 1, hunks: vec![], group: None };
        let r = refuse(&edit).expect("refused").to_json();
        assert!(r.starts_with(r#"{"type":"refused","op":4,"error":{"id":"MPC-E113""#), "{r}");
        assert!(r.contains("k2 doc"), "{r}");
        let r = refuse(&ClientFrame::Cursor { root: "source".into(), version: 1, anchor: 0, head: 0 }).expect("refused").to_json();
        assert!(r.starts_with(r#"{"type":"error","error":{"id":"MPC-E113""#), "{r}");
        for ok in [
            ClientFrame::Since { op: 1, version: 2 },
            ClientFrame::Visible(true),
            ClientFrame::Hello,
            ClientFrame::Watch,
            ClientFrame::Report { seq: 3 },
            ClientFrame::ViewAddress { req: 1, address: Some("@sld:256".into()) },
            ClientFrame::TimePosition { req: 2, time: None },
        ] {
            assert!(refuse(&ok).is_none(), "{ok:?}");
        }
        let deck = |r| root_ok_for(super::super::target::Tier::Deck, r);
        assert!(deck(None) && deck(Some("slides")) && deck(Some("source")) && !deck(Some("tracks")));
    }

    /// Each keyed tier's root, mode and the roots its socket takes: its own
    /// container root, `source`, or none; never another tier's.
    #[test]
    fn each_keyed_tier_has_its_root_and_mode() {
        use super::super::target::Tier;
        assert_eq!((root_of(Tier::Deck), mode_of(Tier::Deck)), ("slides", "deck"));
        assert_eq!((root_of(Tier::Timeline), mode_of(Tier::Timeline)), ("tracks", "timeline"));
        assert_eq!((root_of(Tier::Scene), mode_of(Tier::Scene)), ("colls", "scene"));
        for t in [Tier::Deck, Tier::Timeline, Tier::Scene] {
            assert!(root_ok_for(t, None) && root_ok_for(t, Some("source")) && root_ok_for(t, Some(root_of(t))), "{t:?}");
            assert!(!root_ok_for(t, Some("nope")), "{t:?}");
        }
        assert!(!root_ok_for(Tier::Timeline, Some("slides")) && !root_ok_for(Tier::Scene, Some("tracks")));
        let r = refuse(&ClientFrame::Snapshot { op: 2 }).expect("refused").to_json();
        assert!(r.contains("timeline or scene") && r.contains("k2 doc"), "{r}");
    }

    #[test]
    fn turn_ends_go_only_where_the_turn_was_seen() {
        let key = DocKey { space: "s".into(), path: "d.pptx".into() };
        let mut t = Turns::default();
        let end = HostEvent::TurnEnd { turn: "t:aa.1".into(), actor: Some("kit:external".into()), at_ms: 3 };
        assert!(host_event_frame(&end, "person:owner", &key, &t).is_none(), "a turn this doc never carried");
        t.saw("t:aa.1");
        let f = host_event_frame(&end, "person:owner", &key, &t).expect("forwarded").to_json();
        assert!(f.starts_with(r#"{"type":"turn_end","turn":"t:aa.1","actor":{"id":"kit:external","name":"the file","kind":"file""#), "{f}");
        for i in 0..TURNS_KEPT + 5 {
            t.saw(&format!("t:bb.{i}"));
        }
        assert!(!t.has("t:aa.1"), "the oldest turns are forgotten");
        assert!(t.has(&format!("t:bb.{}", TURNS_KEPT + 4)));
    }
}
