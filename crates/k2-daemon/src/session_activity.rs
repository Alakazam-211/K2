//! Daemon-side per-session activity detection (the 0.40.23 deferred item).
//!
//! The renderer's activity detection is pane-fed: viewport scans, title
//! and bell handlers all ride the pane's grid-WS, which PARKS when a tab
//! is hidden — and the pane's local idle watcher then writes a FALSE
//! idle ~1s after switch-away (the "spinner dies when you leave the tab"
//! bug, and the false early completion-chime with it). The daemon sees
//! every session's Title/Bell on the PTY event broadcast regardless of
//! viewers, so activity truth lives HERE: one observer task per live
//! session, emitting `SessionActivityChanged` on the app-level bus only
//! on state TRANSITIONS.
//!
//! Heuristics (SSOT: `.k2/notes/activity-detection-study.md` +
//! `tui-signal-study-*.md`, validated against claude/codex/grok/hermes/
//! cursor/gemini):
//!   - Title starting with a braille spinner glyph (U+2800–U+28FF) →
//!     WORKING evidence (claude/codex/grok re-emit ~100ms while busy).
//!   - Title starting with an asterisk-family idle marker → IDLE now.
//!   - Title starting with `⚠ Action Required` → PERMISSION (grok's
//!     HITL signal — "cleanest of any agent"); any later non-⚠ title
//!     clears it.
//!   - Bell → IDLE now (claude bells on done; no studied agent bells
//!     spuriously — codex's supposed bell was disproved).
//!   - WORKING decays to IDLE 2s after the last working evidence (the
//!     no-glyph agents' idle has no positive signal; spinners refresh
//!     ~100ms so 2s cannot flicker mid-turn).
//!   - ChildExit / unregister → hard IDLE.
//!
//! Known v1 limitation (studies): hermes/cursor emit no titles and no
//! bell — their sessions never assert daemon-side WORKING and keep the
//! renderer's client-side detection as the fallback (the store's merge
//! rule only prefers daemon truth for keys the daemon has spoken for).
//!
//! prd-daemon-activity-and-thread-working-v1 S2 (DA28): every transition
//! feeds the daemon activity store as title evidence, which counts only
//! for a session with no hook and no transcript evidence. A child exit
//! always counts (`pty_exited`). Since S4 (RL5, A29) this observer emits
//! nothing on the bus: the compat `SessionActivityChanged` and the
//! token-ledger idle scan come from the store (`activity_events`).

use std::sync::Arc;
use std::time::Duration;

use k2_core::log_debug;
use k2_core::terminal::{AlacEvent, DaemonPtySession};

/// The observer's three states (fed to the activity store as title
/// evidence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Working,
    Idle,
    Permission,
}

/// WORKING evidence expires this long after the last spinner-glyph
/// title. Spinners refresh ~100ms (studies), so mid-turn flicker is
/// impossible; 2s (vs the renderer's 1s) absorbs bus/encode jitter.
const WORKING_GRACE: Duration = Duration::from_secs(2);

/// First-char braille spinner range — claude/codex/grok working marker.
fn title_is_working(title: &str) -> bool {
    matches!(title.chars().next(), Some(c) if ('\u{2800}'..='\u{28FF}').contains(&c))
}

/// Asterisk-family idle markers (claude's done/idle title lead glyphs).
/// Ported verbatim from the renderer's title heuristic.
fn title_is_idle_marker(title: &str) -> bool {
    matches!(
        title.chars().next(),
        Some(
            '*' | '✱' | '✲' | '✳' | '✴' | '✵' | '✶' | '✷' | '✸' | '✹' | '⚹' | '⁎' | '∗' | '※'
        )
    )
}

/// Grok's human-in-the-loop marker (title-owned; a later non-⚠ title
/// clears it — same semantics as the renderer's recordTitlePermission).
fn title_is_permission(title: &str) -> bool {
    title.starts_with("⚠ Action Required")
}

/// Pure transition function — unit-tested without a PTY. Returns the
/// next state; the caller emits only when it differs from the current.
pub fn next_state(current: Activity, title: Option<&str>, bell: bool, grace_expired: bool) -> Activity {
    if bell {
        return Activity::Idle;
    }
    if let Some(t) = title {
        if title_is_permission(t) {
            return Activity::Permission;
        }
        if title_is_working(t) {
            return Activity::Working;
        }
        if title_is_idle_marker(t) {
            return Activity::Idle;
        }
        // Any other title: clears permission (title-owned), otherwise
        // no opinion — keep current (subject to grace decay).
        if current == Activity::Permission {
            return Activity::Idle;
        }
        return current;
    }
    if grace_expired && current == Activity::Working {
        return Activity::Idle;
    }
    current
}

fn emit_status(session_id: &str, status: Activity) {
    let signal = match status {
        Activity::Working => k2_core::activity::TitleSignal::Working,
        Activity::Idle => k2_core::activity::TitleSignal::Idle,
        Activity::Permission => k2_core::activity::TitleSignal::Permission,
    };
    crate::activity_store::apply_title(session_id, signal);
}

/// Spawn the observer for one registered session. Lives and dies with
/// the PTY: exits on ChildExit or channel close (last Arc dropped). A
/// force-removed session can't strand a WORKING state: its activity row
/// goes away in `v2_session_map::unregister`.
pub fn spawn_observer(agent_name: String, session: Arc<DaemonPtySession>) {
    let session_id = session.session_id.to_string();
    let mut rx = session.subscribe_events();
    tokio::spawn(async move {
        let mut state = Activity::Idle;
        // Armed only while WORKING; None = no pending decay.
        let mut grace_deadline: Option<tokio::time::Instant> = None;
        loop {
            let next = if let Some(deadline) = grace_deadline {
                tokio::select! {
                    ev = rx.recv() => Some(ev),
                    _ = tokio::time::sleep_until(deadline) => None, // grace expired
                }
            } else {
                Some(rx.recv().await)
            };

            let (title, bell, grace_expired) = match next {
                None => (None, false, true),
                Some(Ok(AlacEvent::Title(t))) => (Some(t), false, false),
                Some(Ok(AlacEvent::ResetTitle)) => (Some(String::new()), false, false),
                Some(Ok(AlacEvent::Bell)) => (None, true, false),
                Some(Ok(AlacEvent::ChildExit(_))) => {
                    if state != Activity::Idle {
                        emit_status(&session_id, Activity::Idle);
                    }
                    crate::activity_store::apply(&session_id, k2_core::activity::Evidence::PtyExited);
                    log_debug!("[session-activity] observer exit (child) agent={agent_name}");
                    return;
                }
                Some(Ok(_)) => continue, // frames/clipboard/etc — not evidence
                Some(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Some(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    if state != Activity::Idle {
                        emit_status(&session_id, Activity::Idle);
                    }
                    crate::activity_store::apply(&session_id, k2_core::activity::Evidence::PtyExited);
                    log_debug!("[session-activity] observer exit (closed) agent={agent_name}");
                    return;
                }
            };

            let new_state = next_state(state, title.as_deref(), bell, grace_expired);

            // Refresh / arm / clear the decay timer.
            grace_deadline = if new_state == Activity::Working {
                Some(tokio::time::Instant::now() + WORKING_GRACE)
            } else {
                None
            };

            if new_state != state {
                state = new_state;
                emit_status(&session_id, state);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_titles_assert_working_and_decay_to_idle() {
        let s = next_state(Activity::Idle, Some("⠋ Thinking…"), false, false);
        assert_eq!(s, Activity::Working);
        // Repeated spinner frames: still working (no transition).
        let s = next_state(s, Some("⠙ Thinking…"), false, false);
        assert_eq!(s, Activity::Working);
        // Grace expiry with no new evidence → idle.
        let s = next_state(s, None, false, true);
        assert_eq!(s, Activity::Idle);
    }

    #[test]
    fn bell_and_idle_markers_end_the_turn() {
        assert_eq!(next_state(Activity::Working, None, true, false), Activity::Idle);
        assert_eq!(
            next_state(Activity::Working, Some("✳ Done"), false, false),
            Activity::Idle
        );
    }

    #[test]
    fn grok_permission_is_title_owned() {
        let s = next_state(Activity::Working, Some("⚠ Action Required"), false, false);
        assert_eq!(s, Activity::Permission);
        // Grace expiry never demotes permission — it is title-owned.
        assert_eq!(next_state(s, None, false, true), Activity::Permission);
        // Any other title clears it.
        assert_eq!(next_state(s, Some("⠋ resumed"), false, false), Activity::Working);
        assert_eq!(
            next_state(Activity::Permission, Some("plain title"), false, false),
            Activity::Idle
        );
    }

    #[test]
    fn plain_titles_keep_current_state() {
        assert_eq!(
            next_state(Activity::Working, Some("my-project — zsh"), false, false),
            Activity::Working,
            "non-signal titles are not idle evidence; decay handles idle"
        );
        assert_eq!(
            next_state(Activity::Idle, Some("my-project — zsh"), false, false),
            Activity::Idle
        );
    }
}
