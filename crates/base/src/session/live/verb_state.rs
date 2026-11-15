//! A pack's **verb state** ([`Pack::verb_state_build`]): what its structured verbs read beside
//! a live keyed doc that isn't a table, kept by the kit beside the doc's live view.
//!
//! - **Built once**, from the live view, the first time a structured verb runs on the doc
//!   (after a load, or after the state was dropped), and handed to every verb as a read-only
//!   borrow ([`crate::tools::VerbCtx::verb_state`]).
//! - **Kept by every change**, with the live view: the change's applied stream and the view
//!   after it go to [`Pack::verb_state_change`] in the change's commit (a local change, an
//!   undo, a redo, a restore or a revert, and an outside save alike), so a verb never reads a
//!   state behind the doc. A hook that answers `false` drops it; it is built again on the next
//!   verb. A state at another version than the change's start (a change the view didn't keep)
//!   is dropped the same way.
//! - **Dropped with the live view** (a reload, a purge or erasure, a text merge that keeps no
//!   stream), and never stored or sent: it is the pack's, in memory only.
//! - **Checked** in test builds (and with the `differential` feature), as the live view is:
//!   after every change and at every hand-out the kept state is compared with a fresh build
//!   from the view, and a difference fails an assertion, naming the doc, the version and the
//!   change.
//!
//! It holds no interior mutability and shares nothing with the live view: the view handed to
//! the hooks is dropped before the change returns, so the next change doesn't copy the view.

use super::*;
use crate::session::VerbState;

/// The pack's kept verb state and the doc's version it is at.
pub(super) struct KeptVerbState {
    seq: Seq,
    state: Box<dyn VerbState>,
}

/// Where a change came from, for the plant and the checks' messages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ChangeFrom {
    /// A change made through the session (an edit, an undo or redo, a restore, a revert).
    Local,
    /// An outside save taken in by key.
    Outside,
}

impl ChangeFrom {
    fn words(self) -> &'static str {
        match self {
            ChangeFrom::Local => "a change made through the session",
            ChangeFrom::Outside => "an outside save",
        }
    }
}

/// The planted fault [`crate::differential::VerbStateMutant::OutsideSaveSkipped`].
fn planted_outside_save_skipped() -> bool {
    plant_on!(
        crate::differential::verb_state_mutant()
            == Some(crate::differential::VerbStateMutant::OutsideSaveSkipped)
    )
}

/// At most this much of a state's `Debug` in a check's assertion message.
const SHOWN: usize = 600;

fn shown(s: &(dyn VerbState + 'static)) -> String {
    let mut d = format!("{s:?}");
    if d.len() > SHOWN {
        let mut end = SHOWN;
        while !d.is_char_boundary(end) {
            end -= 1;
        }
        d.truncate(end);
        d.push('…');
    }
    d
}

impl Session {
    /// The pack's verb state, built from `view` (the live view at the doc's version) when the
    /// doc keeps none yet: called before a structured verb runs. A kept state at another
    /// version (a change didn't keep it: a kit bug, refused loudly in test builds) is built
    /// again.
    pub(super) fn ensure_verb_state(&self, st: &mut State, view: &View) {
        if !self.keeps_view() {
            return;
        }
        if let Some(k) = &st.verb_state {
            if k.seq == st.version {
                if keyed_view::checks_on() {
                    self.check_verb_state(st, &*k.state, view, "handed to a verb");
                }
                return;
            }
            assert!(
                !keyed_view::checks_on(),
                "the verb state of {} is at v{}, the doc at v{}: a change didn't keep it",
                st.key.path,
                k.seq,
                st.version
            );
            st.verb_state = None;
        }
        if let Some(state) = self.pack.verb_state_build(view) {
            #[cfg(any(test, feature = "differential"))]
            crate::differential::count_verb_state_build();
            st.verb_state = Some(KeptVerbState {
                seq: st.version,
                state,
            });
        }
    }

    /// The kept verb state at the doc's version, for a verb's history (`VerbCtx::verb_state`).
    pub(super) fn verb_state_now(st: &State) -> Option<&(dyn VerbState + 'static)> {
        st.verb_state
            .as_ref()
            .filter(|k| k.seq == st.version)
            .map(|k| &*k.state)
    }

    /// Keep the verb state in step with a change from `before` to `after` (the live view
    /// already after it): the pack's hook with the change's applied `stream` and the view.
    /// A state at another version than `before`, or one the hook can't keep, is dropped.
    pub(super) fn verb_state_after(
        &self,
        st: &mut State,
        before: Seq,
        after: Seq,
        stream: &[crate::elem::Applied],
        from: ChangeFrom,
    ) {
        let Some(mut kept) = st.verb_state.take() else {
            return;
        };
        if kept.seq != before {
            return;
        }
        if from == ChangeFrom::Outside && planted_outside_save_skipped() {
            kept.seq = after;
            st.verb_state = Some(kept);
            return;
        }
        let Ok(view) = self.verb_view(st) else {
            return;
        };
        if !self.pack.verb_state_change(&mut *kept.state, stream, &view) {
            return;
        }
        if keyed_view::checks_on() {
            let what = format!("after {} (v{before} to v{after})", from.words());
            self.check_verb_state(st, &*kept.state, &view, &what);
        }
        // the view shares the live view's state: let it go, so the next change doesn't copy it
        drop(view);
        kept.seq = after;
        st.verb_state = Some(kept);
    }

    /// Test builds: `kept` is the state a fresh build from `view` gives, else an assertion fails,
    /// naming the doc, its version and `what` (when it was compared).
    fn check_verb_state(
        &self,
        st: &State,
        kept: &(dyn VerbState + 'static),
        view: &View,
        what: &str,
    ) {
        let fresh = self.pack.verb_state_build(view);
        let same = fresh.as_deref().is_some_and(|f| kept.same_as(f));
        assert!(
            same,
            "the verb state of {} at v{} is not a fresh build ({what}): kept {}, fresh {}",
            st.key.path,
            st.version,
            shown(kept),
            fresh.as_deref().map_or_else(|| "none".to_owned(), shown)
        );
    }
}
