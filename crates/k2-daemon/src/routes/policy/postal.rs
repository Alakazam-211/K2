//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/postal` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.
//!
//! Postal (Beta, prd-postal-in-k2-v1 S3): the floor here lets a login in;
//! the handler (`postal_routes`) decides by role and by passport: claiming
//! is an Owner's or Admin's, answering a request the Owner's (Admins when
//! allowed), turning Postal on the Owner's; an agent asks for each of them,
//! for an invite and for the p5 link on a card, and removes, blocks or
//! rejects its own friends and requests itself. None is on the Voice & Chat
//! only allowlist.

use super::Floor::{Admin, Member, Owner};
use super::{get, post, split, Route};

pub(super) const ROWS: &[Route] = &[
    post("/cli/postal/block", Member),
    post("/cli/postal/claim", Admin),
    // People only (the handler refuses a passport): this server's pairing code.
    get("/cli/postal/code", Member),
    post("/cli/postal/decide", Admin),
    // The handler: the Owner turns it on; an agent's ask is a card.
    post("/cli/postal/enable", Member),
    get("/cli/postal/friends", Member),
    post("/cli/postal/invite", Member),
    post("/cli/postal/link-p5", Member),
    // The Owner links Postal approvals (the plane's postal-decide grant).
    post("/cli/postal/owner-link", Owner),
    post("/cli/postal/remove", Member),
    // The handler: an agent only makes its own routing quieter.
    post("/cli/postal/route", Member),
    // An agent sends as its own address; a person needs Admin.
    post("/cli/postal/send", Member),
    split("/cli/postal/settings", Member, Owner),
    get("/cli/postal/status", Member),
];
