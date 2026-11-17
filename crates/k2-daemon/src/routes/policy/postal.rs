//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/postal` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.
//!
//! Postal (Beta, prd-postal-in-k2-v1 S3): the floor here lets a login in;
//! the handler (`postal_routes`) decides by role and by passport: claiming
//! is an Owner's or Admin's, answering a request the Owner's (Admins when
//! allowed), an agent's invite and the p5 link are cards, and an agent
//! removes or blocks its own friends itself. None is on the Voice & Chat
//! only allowlist.

use super::Floor::{Admin, Member, Owner};
use super::{get, post, split, Route};

pub(super) const ROWS: &[Route] = &[
    post("/cli/postal/block", Member),
    post("/cli/postal/claim", Admin),
    post("/cli/postal/decide", Admin),
    get("/cli/postal/friends", Member),
    post("/cli/postal/invite", Member),
    post("/cli/postal/link-p5", Member),
    post("/cli/postal/remove", Member),
    split("/cli/postal/settings", Member, Owner),
    get("/cli/postal/status", Member),
];
