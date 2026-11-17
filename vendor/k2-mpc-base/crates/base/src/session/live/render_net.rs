//! The net under a file that can't be rendered (docs/SESSIONS.md, "When the file can't be
//! rendered"): a whole read gives the doc's content with the file's bytes as last synced and the
//! failure ([`ReadResult::render_failed`]); a pack's structured read verb gives its result with
//! a warning; the tick reports a write-back that can't render once per version; and a
//! whole-file write from a read given the synced bytes is refused (`MPC-E824`). A child of
//! `live`, so it works on the session's own state.

use super::*;

/// The most ranges of versions whose whole read was given the synced bytes the session keeps
/// (`State::unrendered_reads`). Past it the two oldest merge into one range: the versions
/// between them are refused too, which is safe (the writer reads again).
pub(super) const UNRENDERED_RANGES_MAX: usize = 32;

/// The planted fault [`crate::differential::KeyedMutant::ReadReplaysRenderFailure`].
pub(super) fn planted_read_replays_render_failure() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::ReadReplaysRenderFailure)
    )
}

impl Session {
    /// The doc as a whole read gives it ([`Session::read`]), and who changed it since the
    /// caller last read: its content, and the file's bytes as the pack renders them now. When
    /// the pack can't render them, the bytes are the file's as last synced (the carrier; none
    /// for a pack that keeps none) and `render_failed` is the pack's error. Nothing moves:
    /// no pairing is made, no rendering recorded, no carrier taken, the doc stays dirty.
    pub(super) fn read_or_synced(&self, st: &State, i: usize) -> Result<ReadResult> {
        let ReadHead {
            version,
            contributors_since_last_read: contributors,
            ..
        } = Self::read_head(st, i);
        let content = self.content(st)?;
        let (bytes, render_failed) = match self.encode_live(st, &content, &st.codec) {
            Ok(bytes) => (bytes, None),
            Err(e) => {
                let synced = match (&st.carrier, self.pack.wants_carrier()) {
                    (Some(c), true) => c.to_vec(),
                    _ => Vec::new(),
                };
                (synced, Some(e))
            }
        };
        Ok(ReadResult {
            bytes,
            text_by_root: content
                .iter()
                .filter_map(|(r, n)| Some((r.clone(), n.as_text()?.to_owned())))
                .collect(),
            content,
            version,
            contributors_since_last_read: contributors,
            render_failed,
        })
    }

    /// A whole read at `version` was given the synced bytes: a whole-file write from it is
    /// refused from now on ([`Session::check_rendered_base`]). Ranges in version order; a
    /// version next to the newest range joins it.
    pub(super) fn note_unrendered_read(st: &mut State, version: Seq) {
        let r = &mut st.unrendered_reads;
        match r.last_mut() {
            Some((from, to)) if version >= *from && version <= to.saturating_add(1) => {
                *to = (*to).max(version);
            }
            _ => r.push((version, version)),
        }
        if r.len() > UNRENDERED_RANGES_MAX {
            let (_, to) = r.remove(1);
            r[0].1 = to;
        }
    }

    /// A whole-file write (`write --base`, a table's `write --base` or `--seen`) from `base`:
    /// refused ([`SessionError::BaseUnrendered`], `MPC-E824`) when a whole read at `base` was
    /// given the synced bytes, which aren't that version's file.
    pub(super) fn check_rendered_base(st: &State, base: Seq) -> Result<()> {
        match st
            .unrendered_reads
            .iter()
            .any(|(from, to)| (*from..=*to).contains(&base))
        {
            true => Err(SessionError::BaseUnrendered { base }),
            false => Ok(()),
        }
    }

    /// The tick already reported that the write-back can't render the doc at its version, and
    /// the failure stands: the tick doesn't try again until the version moves (or a new carrier
    /// clears the failure).
    pub(super) fn render_reported(st: &State) -> bool {
        st.render_reported == Some(st.version) && Self::render_failure(st).is_some()
    }

    /// The tick's write-back failed: if it failed rendering the doc at its version, that is
    /// reported now, once ([`Session::render_reported`]).
    pub(super) fn note_render_reported(st: &mut State) {
        if Self::render_failure(st).is_some() {
            st.render_reported = Some(st.version);
        }
    }
}
