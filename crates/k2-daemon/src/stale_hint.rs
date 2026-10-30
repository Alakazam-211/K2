//! Stale hints for Thread changes that never alert anyone
//! (push-content-available-v1, 0.46.7; `contract-phone-inbox-stream-0462.md`
//! §4.3).
//!
//! An agent's post is the Thread push coalescer's (`thread_push`): it alerts
//! a person, or notes the chat stale for them when it can't (muted, viewing
//! on the desktop, over budget). Every OTHER Thread write changes a phone's
//! cached chat with no alert at all:
//! - a person's own post (from the desktop, another phone, the web);
//! - a card closing (answered, voided, expired, a handoff closed).
//!
//! For those, every Thread push recipient of the chat's project (the people
//! `notify_prefs::recipients` would alert, muted or not) gets the chat noted
//! stale ([`crate::badge_push::note_stale`]), so their next silent push says
//! which chats to refresh. Ids and seqs only, never content.
//!
//! [`on_thread_frame`] is called from `overlay_ws::publish` (every Thread
//! write publishes there, sometimes under the DB lock): it only classifies
//! the frame and hands it to one worker thread, which does the people and
//! DB reads.

use std::sync::mpsc;
use std::sync::OnceLock;

use crate::overlay_ws::OverlayFrame;

/// What the worker needs from one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    conversation_id: String,
    from: String,
    closed_card: bool,
}

/// A card that is no longer waiting for an answer (its frame is the close).
fn is_closed_card(doc: &k2_core::overlay::OverlayDoc) -> bool {
    match doc.kind.as_str() {
        "choice" => !doc.is_pending_choice(),
        "secret" => !doc.is_pending_secret(),
        "handoff" => !doc.is_pending_handoff(),
        _ => false,
    }
}

/// The frame's job, or `None` for frames that never hint (not a Thread
/// item, no doc, no conversation).
fn job_of(frame: &OverlayFrame) -> Option<Job> {
    if frame.collection != "thread" {
        return None;
    }
    let doc = frame.doc.as_ref()?;
    let conversation_id = frame.conversation_id.clone().filter(|c| !c.trim().is_empty())?;
    Some(Job { conversation_id, from: doc.from.clone(), closed_card: is_closed_card(doc) })
}

/// Does this write need a hint? A closed card always; otherwise only a
/// person's post (an agent's new post or open card is the coalescer's).
fn needs_hint(job: &Job, from_is_human: bool) -> bool {
    job.closed_card || from_is_human
}

#[cfg_attr(test, allow(dead_code))]
fn worker() -> &'static mpsc::Sender<Job> {
    static TX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new().name("stale-hint".into()).spawn(move || {
            for job in rx {
                run(&job);
            }
        });
        if let Err(e) = spawned {
            k2_core::log_debug!("[stale_hint] worker spawn failed: {e}");
        }
        tx
    })
}

/// A Thread frame was published. Cheap: classification and a channel send.
pub fn on_thread_frame(frame: &OverlayFrame) {
    let Some(job) = job_of(frame) else { return };
    #[cfg(test)]
    {
        // Unit tests drive `needs_hint` / `job_of` directly; route tests
        // never start the worker or touch the global badge state.
        let _ = job;
    }
    #[cfg(not(test))]
    {
        if worker().send(job).is_err() {
            k2_core::log_debug!("[stale_hint] worker gone; hint dropped");
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn run(job: &Job) {
    // Nothing leaves without the per-daemon path or under air-gap; skip
    // the reads too.
    if crate::push_routes::airgap_blocks() || !crate::push_routes::per_daemon_path() {
        return;
    }
    if !needs_hint(job, !job.closed_card && crate::thread_push::is_human_from(&job.from)) {
        return;
    }
    let project_id: Option<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT project_id FROM overlay_conversations WHERE conversation_id = ?1",
            rusqlite::params![job.conversation_id],
            |r| r.get(0),
        )
        .ok()
    };
    let Some(project_id) = project_id else { return };
    use crate::thread_push::PushEnv as _;
    for r in crate::thread_push::DaemonEnv.recipients(&project_id) {
        crate::badge_push::note_stale(&r.person, &job.conversation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::overlay::OverlayDoc;

    fn frame(collection: &str, conv: Option<&str>, doc: Option<OverlayDoc>) -> OverlayFrame {
        OverlayFrame {
            collection: collection.into(),
            seq: 3,
            id: "it-1".into(),
            doc,
            activity: None,
            conversation_id: conv.map(str::to_string),
            rev: 0,
        }
    }

    fn text(from: &str) -> OverlayDoc {
        OverlayDoc::text("it-1".into(), from.into(), "k2".into(), "canary body text".into(), "thread")
    }

    fn card(status: &str) -> OverlayDoc {
        let mut d = OverlayDoc::choice("it-2".into(), "cortana".into(), "owner".into(), "Ship?".into(), vec!["yes".into(), "no".into()], false);
        d.choice.as_mut().expect("a choice card has a body").status = status.into();
        d
    }

    #[test]
    fn only_thread_items_with_a_conversation_are_jobs() {
        assert!(job_of(&frame("chatter", Some("c-1"), Some(text("owner")))).is_none());
        assert!(job_of(&frame("thread", None, Some(text("owner")))).is_none());
        assert!(job_of(&frame("thread", Some("c-1"), None)).is_none(), "moved / address frames carry no doc");
        let j = job_of(&frame("thread", Some("c-1"), Some(text("owner")))).expect("job");
        assert_eq!(j, Job { conversation_id: "c-1".into(), from: "owner".into(), closed_card: false });
        assert!(!format!("{j:?}").contains("canary"), "a job never holds the text");
    }

    #[test]
    fn a_persons_post_or_a_closed_card_hints_an_agents_post_does_not() {
        let post = job_of(&frame("thread", Some("c-1"), Some(text("cortana")))).expect("job");
        assert!(needs_hint(&post, true), "a person's post");
        assert!(!needs_hint(&post, false), "an agent's post is the coalescer's");
        for status in ["answered", "voided", "expired"] {
            let closed = job_of(&frame("thread", Some("c-1"), Some(card(status)))).expect("job");
            assert!(closed.closed_card, "{status}");
            assert!(needs_hint(&closed, false), "a closed card always hints ({status})");
        }
        let pending = job_of(&frame("thread", Some("c-1"), Some(card("pending")))).expect("job");
        assert!(!pending.closed_card, "an open card is the agent's ask");
        assert!(!needs_hint(&pending, false));
    }
}
