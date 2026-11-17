//! A pack's check of every live change ([`Pack::check_edit`]; docs/SESSIONS.md, "A pack's
//! check of every live change"): asked by [`Session::commit`] for each step of every change,
//! whoever makes it, before the step applies. A child of `live`, so it works on the session's
//! own state.

use super::*;

/// The planted fault [`crate::differential::KeyedMutant::EditCheckDropped`].
fn planted_edit_check_dropped() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::EditCheckDropped)
    )
}

impl Session {
    /// The pack's verdict on `edits`, one step of a change about to apply, on the doc as it is
    /// now (for a later step of a change, after the steps before it): a refusal is
    /// [`SessionError::PackRefused`] (`MPC-E823`). Nothing is asked of a pack that doesn't opt
    /// in ([`Pack::checks_edits`]), and no view is built for it.
    pub(super) fn check_edit(&self, st: &mut State, edits: &[Edit]) -> Result<()> {
        if !self.pack.checks_edits() || planted_edit_check_dropped() {
            return Ok(());
        }
        prof!("commit.check_edit");
        // the live view (no copy) of a keyed doc that isn't a table; dropped before the step
        // applies, which keeps the live view in step (holding it would copy it)
        let view = self.verb_view(st)?;
        self.pack
            .check_edit(&view, edits)
            .map_err(SessionError::PackRefused)
    }
}
