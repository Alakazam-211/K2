//! The host turn: which agent step a doc write belongs to (rendering-data
//! contract r1.4, Item 2, gap 8; K2's host markup §A.2).
//!
//! **The id.** `"t:" + 16 hex of sha256(session_id ‖ 0x00 ‖ registered_at as
//! 8 bytes big-endian) + "." + turn_seq`, for example `t:1a2b3c4d5e6f7a8b.12`
//! (about 22 bytes; the kit takes up to 64 printable ASCII). The source is
//! the session's activity row (`k2_core::activity::Row`): `turn_seq` counts
//! the turns started in this registration at its one turn-start point
//! (`Row::start_turn`), and `registered_at` keeps a re-registered session's
//! ids apart. The session id is hashed: it is the passport's selector and
//! must never enter doc history that other workspaces may read.
//!
//! **Server-side only.** [`current`] reads the caller K2 resolved (the
//! passport's session), never the request: an agent can't merge or split
//! its own activity groups. `None` for a person, for a shell, for a session
//! with no row, and for one with no turn seen yet.
//!
//! **Where it goes.** Every write entry, both tiers: `run_tool_with`,
//! `edit_with_options`, `replace_text_with_options`, `write_base_with_options`,
//! `undo_with` / `redo_with`, `revert_with`, `restore_with`, and at tier 0
//! `write_bytes_with`, `restore_with` and `run_tool` (`routes`, `tier0`).
//!
//! **Its end.** When the row ends a turn, [`ended`] tells every open docs
//! socket `turn_end {turn, actor, at_ms}` ([`super::host_events`]). A view
//! that misses it treats a turn as ended after [`TURN_IDLE_MS`] without a
//! change from that agent (the socket's welcome says so).
//!
//! **Known limits** (KH §A.2): a subagent's writes get the lead's turn (they
//! share its passport); a harness started by hand in a shell tab is a
//! person ("via terminal"), so it gets none.

use sha2::{Digest as _, Sha256};

use k2_mpc_base::Turn;

use super::caller::DocCaller;

/// The view's fallback: a turn reads as ended once its agent has made no
/// change for this long (sent in the docs socket's welcome as `turn_idle_ms`).
pub const TURN_IDLE_MS: u64 = 120_000;

/// The id of turn `seq` of the session `session_id` registered at
/// `registered_at` (unix ms).
pub fn id(session_id: &str, registered_at: i64, seq: u64) -> String {
    let mut h = Sha256::new();
    h.update(session_id.as_bytes());
    h.update([0u8]);
    h.update(registered_at.to_be_bytes());
    let d = h.finalize();
    let hex: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("t:{hex}.{seq}")
}

/// The turn of a row, when it has one: `None` for a shell and before the
/// first turn start.
pub fn of_row(row: &k2_core::activity::Row) -> Option<String> {
    of_parts(&row.facts.session_id, &row.facts.harness, row.registered_at, row.turn_seq)
}

fn of_parts(session_id: &str, harness: &str, registered_at: i64, seq: u64) -> Option<String> {
    if seq == 0 || harness == "shell" || session_id.is_empty() {
        return None;
    }
    Some(id(session_id, registered_at, seq))
}

/// The turn a write by `caller` belongs to now, as the kit takes it. A
/// write after the lead stopped (a background job) still belongs to the
/// last turn that started.
pub fn current(caller: &DocCaller) -> Option<Turn> {
    let DocCaller::Agent { session: Some(session), .. } = caller else { return None };
    let id = crate::activity_store::with_row(session, of_row)??;
    match Turn::new(&id) {
        Ok(t) => Some(t),
        Err(e) => {
            // Never: the id is ASCII hex, a dot and digits, under 64 bytes.
            k2_core::log_debug!("[docs] WARN: turn id {id:?} refused by the kit: {e}");
            None
        }
    }
}

/// A row ended its turn (`activity_store`'s effects, off its lock): every
/// open docs socket hears `turn_end` for it, and forwards it when its doc
/// saw that turn.
pub fn ended(row_turn: &str, project_id: Option<&str>, at_ms: i64) {
    let actor = project_id.map(|p| format!("{}{p}", super::host::AGENT_PREFIX));
    super::host_events::publish_turn_end(row_turn, actor.as_deref(), at_ms.max(0) as u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_the_contract_format_and_fits_the_kit() {
        let t = id("sess-1", 1_700_000_000_000, 12);
        let (head, seq) = t.rsplit_once('.').expect("a dot");
        assert_eq!(seq, "12");
        let hex = head.strip_prefix("t:").expect("t: prefix");
        assert_eq!(hex.len(), 16);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{t}");
        assert!(t.len() <= 64);
        assert_eq!(Turn::new(&t).expect("the kit takes it").as_str(), t);
        // Deterministic, and apart per session, registration and turn.
        assert_eq!(id("sess-1", 1_700_000_000_000, 12), t);
        assert_ne!(id("sess-2", 1_700_000_000_000, 12), t);
        assert_ne!(id("sess-1", 1_700_000_000_001, 12), t, "a re-registered session gets new ids");
        assert_ne!(id("sess-1", 1_700_000_000_000, 13), t);
        // The session id never appears in it.
        assert!(!t.contains("sess"));
    }

    #[test]
    fn no_turn_for_a_shell_or_before_the_first_start() {
        assert_eq!(of_parts("s", "claude", 5, 0), None, "no turn seen yet");
        assert_eq!(of_parts("s", "shell", 5, 3), None, "a shell has no turns");
        assert_eq!(of_parts("", "claude", 5, 3), None);
        assert_eq!(of_parts("s", "codex", 5, 3), Some(id("s", 5, 3)));
    }

    #[test]
    fn a_person_never_has_a_turn() {
        assert!(current(&super::super::caller::owner()).is_none());
        let no_session = DocCaller::Agent { project_id: "p1".into(), session: None };
        assert!(current(&no_session).is_none());
        let unknown = DocCaller::Agent { project_id: "p1".into(), session: Some("no-such-session-turn-test".into()) };
        assert!(current(&unknown).is_none(), "a session with no activity row");
    }

    /// The passport's session is the activity row's key: a row registered
    /// under it gives the caller its turn, the same id `of_row` gives, and
    /// a turn start moves it on.
    #[test]
    fn an_agent_gets_its_rows_turn() {
        crate::test_support::with_temp_home(|| {
            let sid = format!("turn-test-{}", uuid::Uuid::new_v4());
            crate::activity_store::register(crate::activity_store::SessionFacts {
                session_id: sid.clone(),
                agent_name: "turn-agent".into(),
                cwd: None,
                program: Some("claude".into()),
            });
            let caller = DocCaller::Agent { project_id: "p1".into(), session: Some(sid.clone()) };
            assert!(current(&caller).is_none(), "registered, no turn yet");
            let hook = |bucket: &str, at: i64| {
                crate::activity_store::apply_at(&sid, k2_core::activity::Evidence::LegacyHook(bucket), at)
            };
            let t0 = chrono::Utc::now().timestamp_millis() + 1_000;
            hook("start", t0);
            let first = current(&caller).expect("a turn");
            let want = crate::activity_store::with_row(&sid, of_row).expect("row").expect("turn");
            assert_eq!(first.as_str(), want);
            assert!(first.as_str().ends_with(".1"), "{first:?}");
            let watching = super::super::host_events::subscribe(&k2_mpc_base::store::DocKey { space: "p1".into(), path: "deck.pptx".into() });
            hook("stop", t0 + 1_000);
            let ended = watching.rx.try_iter().find_map(|e| match e {
                super::super::host_events::HostEvent::TurnEnd { turn, actor, at_ms } if turn == first.as_str() => Some((actor, at_ms)),
                _ => None,
            });
            assert_eq!(ended, Some((None, (t0 + 1_000) as u64)), "the stop ended the first turn on the docs sockets");
            let after_stop = current(&caller).expect("a write after the stop");
            assert_eq!(after_stop, first, "a write after the lead stopped is still the last turn's");
            hook("start", t0 + 2_000);
            let second = current(&caller).expect("a turn");
            assert!(second.as_str().ends_with(".2"), "{second:?}");
            assert_eq!(first.as_str().rsplit_once('.').map(|p| p.0), second.as_str().rsplit_once('.').map(|p| p.0));
            crate::activity_store::unregister(&sid);
            assert!(current(&caller).is_none(), "the row went with the session");
        });
    }
}
