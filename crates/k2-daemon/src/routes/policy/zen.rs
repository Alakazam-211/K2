//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/zen` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Member, NoLogin, VoiceChat};
use super::{both, get, post, Route};

pub(super) const ROWS: &[Route] = &[
    // Zen Mode (prd-zen-mode-v1 Z15, vs-live Z59/Z62; prd-zen-gardens-v1
    // G13/G45): Member floor so a Member's own desktop works; every handler
    // then requires the LOCAL owner token and answers 403 `zen_local_only`
    // to anything else. `page/ensure` and `homes/sync` are gone (G13).
    // Zen v2 (prd-zen-user-widgets-v2 UW35, UWB6, UWB15, UWB23): the widget
    // and grant rows are Member too; the grant-giving ones take the owner
    // token only (403 `owner_only`, UWB3). `lib/*` is B3's zen_lib_routes.
    // The person's order and pins in a Garden's Agents widget (0.46.4):
    // the read takes the owner token or a local passport (agents read it),
    // the set the owner token only (zen_agents_order_routes).
    get("/cli/zen/agents/order", Member),
    post("/cli/zen/agents/order/set", Member),
    // Garden data (prd-gardens-local-services-v1 1A, contract-garden-data-v1
    // §4.4): this computer only, so no Connect login ever (NoLogin). The
    // owner token and agent passports reach the handlers, which apply
    // grants on every call; `/cli/zen/call` takes the owner token only.
    get("/cli/zen/blob/get", NoLogin),
    post("/cli/zen/blob/ingest", NoLogin),
    post("/cli/zen/blob/put", NoLogin),
    post("/cli/zen/call", NoLogin),
    post("/cli/zen/data/admin", NoLogin),
    post("/cli/zen/data/admin/revoke", NoLogin),
    post("/cli/zen/data/batch", NoLogin),
    get("/cli/zen/data/blob/get", NoLogin),
    get("/cli/zen/data/blob/list", NoLogin),
    post("/cli/zen/data/blob/put", NoLogin),
    post("/cli/zen/data/board", NoLogin),
    get("/cli/zen/data/changes", NoLogin),
    post("/cli/zen/data/delete", NoLogin),
    get("/cli/zen/data/export", NoLogin),
    get("/cli/zen/data/get", NoLogin),
    post("/cli/zen/data/grant", NoLogin),
    get("/cli/zen/data/grants", NoLogin),
    // prd-garden-board-needs-v1 §6 (garden-history-v1).
    get("/cli/zen/data/history", NoLogin),
    post("/cli/zen/data/import", NoLogin),
    get("/cli/zen/data/list", NoLogin),
    post("/cli/zen/data/patch", NoLogin),
    post("/cli/zen/data/put", NoLogin),
    post("/cli/zen/data/restore", NoLogin),
    post("/cli/zen/data/revoke", NoLogin),
    get("/cli/zen/data/show", NoLogin),
    post("/cli/zen/data/snapshot", NoLogin),
    post("/cli/zen/data/space/delete", NoLogin),
    post("/cli/zen/data/space/move", NoLogin),
    get("/cli/zen/data/spaces", NoLogin),
    post("/cli/zen/data/tx/abort", NoLogin),
    post("/cli/zen/data/tx/add", NoLogin),
    post("/cli/zen/data/tx/begin", NoLogin),
    post("/cli/zen/data/tx/commit", NoLogin),
    get("/cli/zen/doctor", Member),
    // `k2.download` consent (0.46.5): the widgets a Garden lets save files
    // without asking. The owner token only (zen_downloads_routes).
    get("/cli/zen/downloads", Member),
    post("/cli/zen/downloads/allow", Member),
    post("/cli/zen/downloads/forget", Member),
    post("/cli/zen/downloads/spend", Member),
    // Garden access (prd-garden-access-v1 G14), live only with the
    // `gardenAccess` switch on (`live_row`): GET the list (the person; a
    // passport learns whether it is listed), POST a change (the person);
    // the ask (a live local passport); History (the person).
    both("/cli/zen/garden/access", Member),
    post("/cli/zen/garden/access/ask", Member),
    // A staged Garden page checked, never written (0.46.1 S3).
    post("/cli/zen/garden/check", Member),
    post("/cli/zen/garden/delete", Member),
    get("/cli/zen/garden/history", Member),
    // Garden Lock (prd-garden-lock-and-sync-v1 §5.2): the person only; the
    // handler refuses every passport, login and app pass (403 owner_only).
    post("/cli/zen/garden/lock", Member),
    post("/cli/zen/garden/new", Member),
    // Agents run Gardens end to end (0181): a person or an agent with
    // "Manage Gardens and data sources" writes; any other local agent
    // proposes (zen_agent_routes).
    post("/cli/zen/garden/put", Member),
    post("/cli/zen/garden/rename", Member),
    post("/cli/zen/garden/reorder", Member),
    // Garden access (live only with the switch on): Restore a deleted
    // Garden (the person).
    post("/cli/zen/garden/restore", Member),
    post("/cli/zen/garden/sync", Member),
    post("/cli/zen/garden/template", Member),
    get("/cli/zen/gardens", Member),
    // Garden access (live only with the switch on): deleted Gardens.
    get("/cli/zen/gardens/deleted", Member),
    get("/cli/zen/get", Member),
    get("/cli/zen/history", Member),
    // Static review of a received Garden bundle or folder (Rosson
    // 2026-10-11: importing a Garden is an agent workflow). Reads only.
    get("/cli/zen/inspect", Member),
    post("/cli/zen/lib/fetch", Member),
    get("/cli/zen/lib/file", Member),
    // `k2.open` (0.46.4): the sites a Garden opens without asking. Reads
    // for the owner token and local passports; writes the owner token only
    // (zen_links_routes).
    get("/cli/zen/links", Member),
    post("/cli/zen/links/allow", Member),
    post("/cli/zen/links/forget", Member),
    get("/cli/zen/news", Member),
    post("/cli/zen/news/seen", Member),
    get("/cli/zen/proposals", Member),
    post("/cli/zen/proposals/approve", Member),
    post("/cli/zen/proposals/reject", Member),
    get("/cli/zen/proposals/show", Member),
    post("/cli/zen/reload", Member),
    // Gardens on the phone (prd-gardens-on-phone-v1 §3.2): the remote
    // Garden door. Member so role_gate lets every credential through; the
    // handler's gate takes only the owner token or a paired phone of the
    // owner person (zen_remote_routes::gate).
    get("/cli/zen/remote/agents-order", Member),
    get("/cli/zen/remote/blob/get", VoiceChat),
    post("/cli/zen/remote/blob/put", Member),
    post("/cli/zen/remote/call", VoiceChat),
    get("/cli/zen/remote/frame", VoiceChat),
    get("/cli/zen/remote/gardens", VoiceChat),
    get("/cli/zen/remote/lib", VoiceChat),
    post("/cli/zen/remote/lib/fetch", VoiceChat),
    get("/cli/zen/remote/page", VoiceChat),
    post("/cli/zen/remote/pause", Member),
    post("/cli/zen/remote/report", Member),
    post("/cli/zen/remote/resume", Member),
    get("/cli/zen/remote/sources-changes", VoiceChat),
    post("/cli/zen/reset", Member),
    post("/cli/zen/setup", Member),
    // Garden access (live only with the switch on): View this version.
    get("/cli/zen/snapshot", Member),
    get("/cli/zen/status", Member),
    get("/cli/zen/sync", Member),
    get("/cli/zen/templates", Member),
    get("/cli/zen/theme/list", Member),
    post("/cli/zen/theme/new", Member),
    post("/cli/zen/theme/next", Member),
    post("/cli/zen/theme/prev", Member),
    post("/cli/zen/theme/set", Member),
    get("/cli/zen/validate", Member),
    get("/cli/zen/widget/bundle", Member),
    // 0.46.1: the app checks a widget's approved cap before a post to
    // another server (owner token only, checked in zen_routes).
    post("/cli/zen/widget/check", Member),
    // Authorship (contract-garden-data-v1 §4.3, BG3): claim takes the
    // owner token only; sign takes it or a local agent/shell passport.
    post("/cli/zen/widget/claim", Member),
    // 0.46.5: an unplaced widget folder into history (k2 zen reset undoes).
    post("/cli/zen/widget/delete", Member),
    get("/cli/zen/widget/errors", Member),
    post("/cli/zen/widget/errors/clear", Member),
    // The widget dev loop (0.46.1 S3): a staged folder checked, written
    // (or proposed) whole, or previewed on K2's own preview Garden.
    post("/cli/zen/widget/folder/check", Member),
    post("/cli/zen/widget/folder/put", Member),
    get("/cli/zen/widget/history", Member),
    post("/cli/zen/widget/new", Member),
    post("/cli/zen/widget/pause", Member),
    post("/cli/zen/widget/preview", Member),
    post("/cli/zen/widget/put", Member),
    post("/cli/zen/widget/resume", Member),
    post("/cli/zen/widget/run", Member),
    // 0.46.3: a staged folder's page for a headless shot (writes nothing).
    post("/cli/zen/widget/shot", Member),
    get("/cli/zen/widget/show", Member),
    post("/cli/zen/widget/sign", Member),
    get("/cli/zen/widgets", Member),
];
