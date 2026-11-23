//! M2 review D1: an agent's (a session passport's) job from a fenced
//! workspace ships no source until the git steps that read the repo run
//! under its fence (K2 reading the repo for it could be pointed outside the
//! fence mid-read). `--no-source` runs as before, and the owner's or a
//! login's own run from that workspace ships as before.

use crate::compute_routes::{no, Who, Ws, R};

/// A fenced agent's job asked to ship source.
pub(crate) const FENCED_NO_SOURCE: &str = "fenced_no_source";

/// Refuses a session passport's source run from a fenced workspace.
pub(crate) fn refusal(who: &Who, ws: &Ws) -> R<()> {
    if who.passport().is_some() && crate::agent_isolation::workspace_and_fence(&ws.id).1 {
        return Err(no(
            FENCED_NO_SOURCE,
            403,
            format!(
                "workspace {}'s folder fence is on, and K2 doesn't ship a fenced agent's code yet; run with --no-source (copy what the job needs into its command or an artifact), or ask the owner to run it",
                ws.handle
            ),
        ));
    }
    Ok(())
}

/// A job's `requested_by` (`Who::label`) names a session passport.
pub(crate) fn requested_by_passport(requested_by: &str) -> bool {
    requested_by.starts_with("agent:") || requested_by.starts_with("shell:")
}
