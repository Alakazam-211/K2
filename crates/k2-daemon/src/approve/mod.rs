//! Signed approvals, the daemon's side (prd-signed-approvals-v1 rev 2, S1).
//!
//! An agent with the `approve.request` permission asks a named person to
//! approve a relying party's payload bytes (`k2 approve request`). The
//! daemon is the carrier: it checks the bytes' shape only, keeps them
//! exactly as sent, puts an `approval` card on the Thread between the agent
//! and the person, pushes to the person's phone, and hands the device's
//! approval record back to the agent (`k2 approve status`, `--wait`). It
//! never decides whether an approval is valid: the relying party verifies
//! the signature against keys it pinned itself (PRD §7). The daemon can
//! delay or lose an approval; it can't make one.
//!
//! | File | What |
//! |---|---|
//! | [`routes`] | `/cli/approve/*`: the agent verbs, the person's verbs, the Owner/Admin permission switch |
//! | [`people`] | who `--to` names, and whether that is the person the agent works for |
//! | [`shape`] | the payload and record shape checks (the `k2-approve` crate's) |
//! | [`card`] | the Thread card, its frames and the push |
//! | [`wait`] | the long-poll behind `k2 approve request --wait` |
//! | [`expiry`] | the sweep that expires requests past `answer_by` |
//! | [`tcp`] | the TCP dispatcher's arm (body caps, the blocking hop) |
//!
//! State, the card, the push and expiry live here, so a headless daemon
//! with no client attached does all of it. Clients only draw the card;
//! in S1 the desktop app shows it read-only (open it on your phone).

pub(crate) mod card;
pub(crate) mod expiry;
pub(crate) mod people;
pub(crate) mod routes;
pub(crate) mod shape;
pub(crate) mod tcp;
pub(crate) mod wait;

#[cfg(test)]
mod routes_tests;
