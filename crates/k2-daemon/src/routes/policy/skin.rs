//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/skin` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Admin, Member, NoLogin, Owner, Public};
use super::{both, get, post, split, Route};

pub(super) const ROWS: &[Route] = &[
    get("/cli/skin/access", Owner),
    post("/cli/skin/access/devices/revoke", Owner),
    post("/cli/skin/access/invites", Owner),
    post("/cli/skin/access/listed", Owner),
    // app-user-profile P6 (0.46.5): a person's profile, picture and
    // sessions as the owner sees and ends them.
    get("/cli/skin/access/profile", Owner),
    post("/cli/skin/access/profile/image/clear", Owner),
    post("/cli/skin/access/sessions/revoke", Owner),
    post("/cli/skin/access/settings", Owner),
    // 0.46.9: account deletion and client logs, a person's own session pass only.
    post("/cli/skin/account/delete", NoLogin),
    get("/cli/skin/agents", NoLogin),
    get("/cli/skin/app-info", Public),
    // prd-app-caps-0464-v1 AC5 (0.46.4): app.rooms, an App's own pass only.
    get("/cli/skin/app-rooms", NoLogin),
    // 0.46.11: auth.doorStatus, the App's own pass on its gateway link
    // (the link's App; the handler refuses a guest and off a link).
    get("/cli/skin/auth/door", NoLogin),
    // 0.46.9 sign-in door: the App's server swaps a door code on its
    // gateway link (the link's App; the handler refuses off a link).
    post("/cli/skin/auth/exchange", Public),
    post("/cli/skin/client-logs", NoLogin),
    get("/cli/skin/devices", NoLogin),
    post("/cli/skin/devices/revoke", NoLogin),
    // Email Notifications (0.46.5): the App's own email.* on its link
    // (the handler checks the pass and email:send).
    get("/cli/skin/email/log", NoLogin),
    post("/cli/skin/email/send", NoLogin),
    get("/cli/skin/email/status", NoLogin),
    both("/cli/skin/front-door", Owner),
    // Sew S27 grant hook: owner-tier like the grants routes; not an agent
    // verb (agents grant chat through K19's /cli/sew/admin/* later).
    get("/cli/skin/grant-hooks", Owner),
    get("/cli/skin/grant-hooks/log", Owner),
    post("/cli/skin/grant-hooks/reconcile", Owner),
    post("/cli/skin/grant-hooks/retry", Owner),
    post("/cli/skin/grant-hooks/skip", Owner),
    post("/cli/skin/grant-hooks/target", Owner),
    both("/cli/skin/grants", Owner),
    post("/cli/skin/grants/copy-host", Owner),
    post("/cli/skin/grants/delete", Owner),
    post("/cli/skin/grants/enabled", Owner),
    post("/cli/skin/grants/host", Owner),
    // An agent's own Apps' logins (the handler admits a passport only).
    get("/cli/skin/grants/own", Member),
    post("/cli/skin/grants/role", Owner),
    post("/cli/skin/grants/update", Owner),
    // Sew for Apps A2 (0.46.6): guests.chat / guests.setChat, an App's own
    // pass under the owner's guests-chat grant (the handler refuses the
    // rest). ("chat" sorts before "list".)
    both("/cli/skin/guests/chat", NoLogin),
    // prd-app-caps-0464-v1 AC5 (0.46.4): guests.list, an App's own pass
    // only (the handler refuses the rest).
    get("/cli/skin/guests/list", NoLogin),
    // RS-A15 (0.46.1): an App's own pass reads/sets its guests' rooms
    // under the owner's grant; no login (or owner token) uses it.
    both("/cli/skin/guests/rooms", NoLogin),
    both("/cli/skin/hydra", Owner),
    post("/cli/skin/login", Public),
    post("/cli/skin/logout", Public),
    // Room-scope R6 (0.46.1): a pass reads its own view; no login uses it.
    get("/cli/skin/me", NoLogin),
    get("/cli/skin/me/events", NoLogin),
    // Room-scope R9/R16 (0.46.1): room.members, app.self, app.usage for a
    // pass (the handler checks rooms, caps and the link).
    get("/cli/skin/members", NoLogin),
    // prd-app-phone-pairing-v1 (0.46.5): the phone redeems and checks a
    // one-time code (Public: the code is the credential, throttled); a
    // person's own web session starts, polls and cancels (the handler
    // checks the pass).
    post("/cli/skin/pair", Public),
    post("/cli/skin/pair/cancel", NoLogin),
    post("/cli/skin/pair/check", Public),
    post("/cli/skin/pair/start", NoLogin),
    get("/cli/skin/pair/status", NoLogin),
    post("/cli/skin/password/change", Public),
    post("/cli/skin/password/forgot", Public),
    post("/cli/skin/password/reset", Public),
    get("/cli/skin/password/set", Public),
    // app-user-profile (0.46.5): a person's own profile and picture (their
    // own session pass; the handler checks).
    both("/cli/skin/profile", NoLogin),
    post("/cli/skin/profile/image", NoLogin),
    post("/cli/skin/profile/image/clear", NoLogin),
    // 0.46.3 App project chat: a pass reads and posts in the projects
    // whose every member room it holds (the handler's all-members rule).
    get("/cli/skin/projects", NoLogin),
    get("/cli/skin/projects/events", NoLogin),
    get("/cli/skin/projects/messages", NoLogin),
    post("/cli/skin/projects/post", NoLogin),
    post("/cli/skin/projects/read", NoLogin),
    // prd-custom-app-push-v1: a guest's own device and the App's link
    // verbs (the handler checks the pass).
    get("/cli/skin/push/device", NoLogin),
    get("/cli/skin/push/devices", NoLogin),
    post("/cli/skin/push/prefs", NoLogin),
    post("/cli/skin/push/register", NoLogin),
    post("/cli/skin/push/send", NoLogin),
    get("/cli/skin/push/status", NoLogin),
    post("/cli/skin/push/unregister", NoLogin),
    post("/cli/skin/push/viewing", NoLogin),
    post("/cli/skin/refresh", Public),
    both("/cli/skin/roles", Owner),
    post("/cli/skin/roles/assign", Owner),
    post("/cli/skin/roles/from-template", Owner),
    post("/cli/skin/roles/paths", Owner),
    post("/cli/skin/roles/remove", Owner),
    post("/cli/skin/roles/room", Owner),
    get("/cli/skin/roles/suggest", Owner),
    post("/cli/skin/roles/unassign", Owner),
    post("/cli/skin/roles/update", Owner),
    get("/cli/skin/self", NoLogin),
    // app-user-profile (0.46.5): a person's own sessions in this App.
    get("/cli/skin/sessions", NoLogin),
    post("/cli/skin/sessions/revoke", NoLogin),
    post("/cli/skin/sessions/revoke-others", NoLogin),
    both("/cli/skin/templates", Owner),
    post("/cli/skin/templates/apply", Owner),
    post("/cli/skin/templates/delete", Owner),
    post("/cli/skin/templates/lines", Owner),
    post("/cli/skin/templates/lines/delete", Owner),
    post("/cli/skin/templates/lines/update", Owner),
    post("/cli/skin/templates/update", Owner),
    get("/cli/skin/usage", NoLogin),
    // 0.46.6 (Rosson 2026-10-29): an Admin lists App users and resets,
    // sets or unlocks App-only users' passwords; box logins stay the
    // owner's (`skin_routes::owner_or_admin_for`). Adding stays Owner.
    split("/cli/skin/users", Admin, Owner),
    post("/cli/skin/users/agent", Owner),
    post("/cli/skin/users/email", Owner),
    post("/cli/skin/users/full-name", Owner),
    post("/cli/skin/users/invite", Owner),
    post("/cli/skin/users/password", Admin),
    post("/cli/skin/users/remove", Owner),
    post("/cli/skin/users/reset-link", Admin),
    post("/cli/skin/users/rooms", Owner),
    // 0.46.8: `k2 app user sessions`; an App's own agent too (its own gate).
    get("/cli/skin/users/sessions", Owner),
    post("/cli/skin/users/sessions/revoke", Owner),
    post("/cli/skin/users/unlock", Admin),
    // App users' personal wallets (0.46.3): an App user's OWN session
    // pass only (`person_pass_required` for anything else); no Connect
    // login (Rosson #9: Connect members have no wallet).
    post("/cli/skin/wallet/add", NoLogin),
    get("/cli/skin/wallet/list", NoLogin),
    post("/cli/skin/wallet/login", NoLogin),
    post("/cli/skin/wallet/login/cancel", NoLogin),
    post("/cli/skin/wallet/login/input", NoLogin),
    get("/cli/skin/wallet/login/status", NoLogin),
    post("/cli/skin/wallet/remove", NoLogin),
    get("/cli/skin/wallet/status", NoLogin),
    post("/cli/skin/wallet/use", NoLogin),
];
