//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/app` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Admin, Member, Owner};
use super::{both, get, post, split, Route};

pub(super) const ROWS: &[Route] = &[
    // 0.46.9: `k2 app account-deletion` (the handler holds writes to the owner,
    // an Admin or the App's own agent with the App-user switch).
    get("/cli/app/account-deletion", Member),
    post("/cli/app/account-deletion/cancel", Admin),
    post("/cli/app/account-deletion/complete", Admin),
    // App runtime stage 1 (prd-k2-app-runtime-v1 §8.5). Passport rules
    // (own workspace + the two App switches, prd-agent-app-permissions-v1)
    // in the handler. Q7 (Rosson 2026-10-20): Members are read-only, so
    // every mutating `/cli/app/*` floor is Admin (suspend, deny, validate
    // and the shell included; a test below keeps it so). The handlers also
    // refuse a Member login with `member_read_only`.
    // Box prep (S10): the doctor is Admin (agents read it through their
    // passport); prepare runs root-helper verbs and is the owner's.
    // Held App requests (0.46.0 grace + S15 egress, one `app_widenings`
    // table): a passport lists its own workspace's; approve is the owner's
    // in person (handler: owner token or Owner login + a vault confirm
    // bound to the request, never a passport).
    get("/cli/app/approvals", Member),
    // 0.46.6 (client papercut 13): the owner withdraws an approved run's
    // standing for identical re-runs.
    post("/cli/app/approvals/revoke", Owner),
    post("/cli/app/approve", Owner),
    // S19 (0.46.1 door): the per-call audit of an App (a passport: its own
    // workspace's Apps).
    get("/cli/app/audit", Member),
    // 0.46.9 sign-in door: the box switch is the owner's (the handler
    // checks); one App's door and origins the owner's, Admins' and the
    // App's agent's.
    get("/cli/app/auth", Admin),
    post("/cli/app/auth/set", Admin),
    get("/cli/app/box", Admin),
    // Box services (0.46.0): read, propose and revoke are Member (a
    // passport sees its own workspace's Apps); a new one is always a held
    // request (approve is the owner's above); revoking is immediate.
    get("/cli/app/box-service", Member),
    post("/cli/app/box-service/request", Admin),
    post("/cli/app/box-service/revoke", Admin),
    // 0.46.3 F6: the owner's Retry of a blocked (or undone) automatic prep.
    post("/cli/app/box/auto-prepare/retry", Owner),
    // Lane P: remove K2's Podman bundle (prepare --install-podman put it in).
    post("/cli/app/box/podman/uninstall", Owner),
    post("/cli/app/box/prepare", Owner),
    // 0.46.3 F8: undo box prep.
    post("/cli/app/box/unprep", Owner),
    post("/cli/app/build", Admin),
    // App-governed Spaces S1 (0.46.7): an App's chat key. Read: Admin and
    // the App's agent; on / widen: the owner in person (an Admin within
    // the defaults) with a vault confirm; narrow and revoke: Admin or the
    // App's own agent (handler).
    split("/cli/app/chat-key", Admin, Admin),
    post("/cli/app/chat-key/revoke", Admin),
    // 0.46.0 grace: clean mode on narrows (anyone, own Apps for passports);
    // off is held unless the owner turns it off.
    post("/cli/app/clean-mode", Admin),
    // 0.46.9: `k2 app logs <app> --client`, read like the App's own logs.
    get("/cli/app/client-logs", Member),
    // S13: narrowing applies now, widening fields are held.
    post("/cli/app/config", Admin),
    // 0.46.4: read back, list, remove and rename inside an App's data
    // folder (handler: as data put; no Member reads App data).
    get("/cli/app/data/get", Admin),
    get("/cli/app/data/ls", Admin),
    post("/cli/app/data/mv", Admin),
    // 0.46.3: copy workspace files into an App's data folder (handler:
    // only the App's own workspace folder; switch 1 for passports).
    post("/cli/app/data/put", Admin),
    post("/cli/app/data/rm", Admin),
    // 0.46.11: an App's database (db_connect.rs). Connect and disconnect
    // change the App (no Member); the check changes nothing but runs inside
    // the App, as exec does (no Member); status is a read.
    post("/cli/app/db/check", Admin),
    post("/cli/app/db/connect", Admin),
    post("/cli/app/db/disconnect", Admin),
    get("/cli/app/db/status", Member),
    // deny is an Owner/Admin human's, or the requester's withdraw (handler).
    post("/cli/app/deny", Admin),
    post("/cli/app/deploy", Admin),
    // Litmus L3: who, commit, live/failed/stalled of one deployment.
    get("/cli/app/deployment", Member),
    // 0.46.9: k2 app doctor --store reads a repo folder, as init does.
    post("/cli/app/doctor/store", Admin),
    // 0.46.4: any subdomain of the App's workspace onto the App (handler:
    // as label take; passports their own workspace's Apps and labels).
    post("/cli/app/domain/add", Admin),
    // 0.46.6: custom domains (re-check now; the App's primary origin).
    post("/cli/app/domain/check", Admin),
    // 0.46.6: the owner's own web server serves a custom name (on|off).
    post("/cli/app/domain/external", Admin),
    post("/cli/app/domain/primary", Admin),
    post("/cli/app/domain/rm", Admin),
    get("/cli/app/domains", Member),
    // Egress (S15): read and propose are Member (a passport sees its own
    // workspace's Apps); every new host is a held request; narrowing is
    // immediate.
    get("/cli/app/egress", Member),
    post("/cli/app/egress/add", Admin),
    get("/cli/app/egress/log", Member),
    post("/cli/app/egress/mode", Admin),
    post("/cli/app/egress/rm", Admin),
    // Email Notifications (0.46.5, `k2 app email`): reads, off, test and
    // send are Admin-floor (the App's agent by its passport; send rides
    // its agentSend gate); on is the owner's with a fresh confirm.
    get("/cli/app/email", Admin),
    get("/cli/app/email/log", Admin),
    post("/cli/app/email/off", Admin),
    post("/cli/app/email/on", Owner),
    post("/cli/app/email/send", Admin),
    get("/cli/app/email/status", Admin),
    post("/cli/app/email/test", Admin),
    // The guarded engine's install/update through the root helper (S11).
    post("/cli/app/engine/install", Owner),
    // 0.46.4 (update continuity R9): the engine's status, read-only.
    get("/cli/app/engine/status", Member),
    get("/cli/app/errors", Member),
    // The process exception (§4.3): on = the owner in person with an owner
    // confirm (handler); off (back to the guarded container) = manage.
    post("/cli/app/exception", Admin),
    // S28: own workspace + "Allow this agent to manage Apps" for passports
    // (handler); never App credentials or delegation; audited.
    post("/cli/app/exec", Admin),
    // 0.46.7: a public URL on (or off) for an installed App, no re-create
    // (handler: as install --tunnel; passports their own workspace's App).
    post("/cli/app/expose", Admin),
    // RS-A15 (0.46.1): the owner grants an App guests:rooms; revoking
    // narrows (a passport: its own App). Guest rooms over the CLI: the
    // owner, or a passport within its own App's grant (handler refuses
    // Admin/Member logins).
    post("/cli/app/grant", Owner),
    // 0.46.4: copy box logins onto an App (the owner at once; an agent's
    // passport files one held request: the handler).
    post("/cli/app/grant/copy-host", Owner),
    post("/cli/app/grant/revoke", Admin),
    get("/cli/app/grants", Member),
    // prd-app-caps-0464-v1 AC4 (0.46.4): reads (the owner, or a passport
    // on its own App; the handler refuses logins). GET only.
    get("/cli/app/guest/ceiling", Member),
    // Sew for Apps A2 (0.46.6): `k2 app user chat` (the owner: any person;
    // a passport: its own App's guests within the App's guests-chat
    // grant; the handler refuses Admin/Member logins).
    split("/cli/app/guest/chat", Member, Admin),
    get("/cli/app/guest/list", Member),
    split("/cli/app/guest/rooms", Member, Admin),
    // Q8 (prd-agent-app-permissions-v1 §6): the owner hands an App to
    // another workspace (handler: owner token or Owner login only).
    post("/cli/app/handover", Owner),
    // 0.46.5 (prd-app-handoff-v1, `k2 app handover`): B accepts or
    // declines an offer; A, the owner or an Admin cancels; retry a re-key
    // (B or the owner); end the undo window (the owner, or B after a
    // deploy); the plan, status and brief (handlers: passports their own
    // handovers; the plan never to a Member, it names guests). Undo files a
    // card for the owner. Every write is above Member (Q7).
    post("/cli/app/handover/accept", Admin),
    get("/cli/app/handover/brief", Member),
    post("/cli/app/handover/cancel", Admin),
    post("/cli/app/handover/decline", Admin),
    post("/cli/app/handover/finish", Admin),
    get("/cli/app/handover/plan", Admin),
    post("/cli/app/handover/rekey", Admin),
    get("/cli/app/handover/status", Member),
    post("/cli/app/handover/undo", Owner),
    get("/cli/app/history", Member),
    // 0.46.7 (TC-13): k2 app init --template writes into a folder, as
    // sdk add (handler: switch 1 for passports, their own workspace).
    post("/cli/app/init", Admin),
    get("/cli/app/inspect", Member),
    post("/cli/app/install", Admin),
    // 0.46.0 grace: App-scoped keys. Minting needs the owner's confirm
    // (anyone else's mint is held); revoking narrows (Admin; a passport for
    // its own Apps).
    post("/cli/app/key/mint", Admin),
    post("/cli/app/key/revoke", Admin),
    get("/cli/app/keys", Member),
    // 0.46.3: an App takes its own workspace's legacy label (and gives it
    // back); the handler scopes passports to their workspace.
    post("/cli/app/label/return", Admin),
    post("/cli/app/label/take", Admin),
    // S14: the App's gateway link (status; rotate the App pass now).
    get("/cli/app/link", Member),
    post("/cli/app/link/rotate", Admin),
    // 0.46.7 (prd-tauri-core-0467-v1 TC-20): link files as an App setting;
    // the handler lets the App's own agent ("Build and run Apps") in too.
    // The check fetches over the internet: the owner's.
    get("/cli/app/links", Admin),
    post("/cli/app/links/check", Owner),
    post("/cli/app/links/off", Admin),
    post("/cli/app/links/set", Admin),
    get("/cli/app/list", Member),
    get("/cli/app/logs", Member),
    // Room-scope R9 (0.46.1): who holds a grant on a room (owner/admin any
    // room; a passport its own room; a member login is refused).
    get("/cli/app/members", Member),
    // 0.46.4: the steps of a --cmd service's move; one held request for
    // the owner's steps (handler: passports their own workspace's).
    get("/cli/app/move-plan", Member),
    post("/cli/app/move-plan/request", Admin),
    // prd-app-phone-pairing-v1 (0.46.5): `k2 app pairing …`. Status, off
    // and the log are Admin-floor (the App's agent by its passport; the
    // log also needs its App-user switch); the phone app ids and the
    // network check are the Owner's (set: in person + a fresh confirm).
    get("/cli/app/pairing", Admin),
    post("/cli/app/pairing/check", Owner),
    get("/cli/app/pairing/log", Admin),
    post("/cli/app/pairing/off", Admin),
    post("/cli/app/pairing/set", Owner),
    // Q8: park stops an App and holds it for the owner (lowering: Admin).
    post("/cli/app/park", Admin),
    // app-user-profile (0.46.5): an App's profile settings. The owner or an
    // Admin; the App's own agent (passport) lowers only, in the handler.
    both("/cli/app/profile-settings", Admin),
    // prd-custom-app-push-v1 CP36: status, devices, send are Admin-floor
    // (the App's agent by its passport); keys and check are the Owner's.
    get("/cli/app/push", Admin),
    post("/cli/app/push/check", Owner),
    post("/cli/app/push/credentials", Owner),
    post("/cli/app/push/credentials/remove", Owner),
    get("/cli/app/push/devices", Admin),
    post("/cli/app/push/devices/remove", Admin),
    get("/cli/app/push/log", Admin),
    post("/cli/app/push/send", Admin),
    post("/cli/app/push/set", Admin),
    post("/cli/app/push/test", Admin),
    // S19: per-App quotas. Lowering applies now (manage); a raise is the
    // owner's, anyone else's is a held `quota` request (handler).
    get("/cli/app/quota", Member),
    post("/cli/app/quota/set", Admin),
    // Sew 0.9.0 recording in Apps: the read is Admin's and the App's agent's;
    // a set narrows for them too, widening is a person's with a confirm,
    // `required` / `server_read` on the owner's only (handler).
    get("/cli/app/recording", Admin),
    post("/cli/app/recording/set", Admin),
    // 0.46.4 (update continuity R8/R16): the re-host wave's status, a Member
    // read. `/boot-status` and the update job point here from S1; the
    // handler is S2's (`apps::rehost`).
    get("/cli/app/rehost", Member),
    // Run one App's re-host now (restart-class: switch 1 for its own App,
    // A20) and its update settings (policy, window, "can't run two copies":
    // switch 2; Rosson Q4 + Q16).
    post("/cli/app/rehost/policy", Admin),
    post("/cli/app/rehost/run", Admin),
    // Q8: one way out for every App of a workspace before its removal
    // (uninstall / park: Admin; handover: the handler holds it to the owner).
    post("/cli/app/release", Admin),
    // AR10: remote App passes. Listing and revoking (narrows) are Member;
    // minting is a credential mint, the owner's (handler refuses passports).
    get("/cli/app/remote-pass", Member),
    post("/cli/app/remote-pass/revoke", Admin),
    post("/cli/app/remote-pass/rotate", Owner),
    // 0.46.3 box usage + disk guard (prd-app-resources-disk-guard-v1): the view
    // is owner/Admin/agents (agents never see folder paths, handler); rescan is
    // rate-limited; settings are the owner's (handler: owner token or Owner login).
    get("/cli/app/resources", Admin),
    // 0.46.4 Slice B: clean (an agent only its own Apps', handler) and the
    // CPU/memory trends, the view's readers.
    post("/cli/app/resources/clean", Admin),
    post("/cli/app/resources/rescan", Admin),
    post("/cli/app/resources/settings", Owner),
    get("/cli/app/resources/trends", Admin),
    post("/cli/app/restart", Admin),
    // S19: only the owner resumes a suspended App.
    post("/cli/app/resume", Owner),
    post("/cli/app/rollback", Admin),
    // Room-scope R9: the owner hides guests' names in one room.
    post("/cli/app/room-names", Owner),
    // 0.46.6: @k2/server (and Sew's App kit) into an App folder, its
    // status, and keeping Sew's kit: as build (handler: switch 1 for
    // passports, inside their own workspace).
    post("/cli/app/sdk/add", Admin),
    post("/cli/app/sdk/kit", Admin),
    get("/cli/app/sdk/status", Admin),
    // K-M3 (0.46.3): whether an App has its seal key (never the value);
    // rotating signs everyone out: the owner, or a passport with switch 2
    // for its own App (handler).
    get("/cli/app/seal", Member),
    post("/cli/app/seal/rotate", Owner),
    // Room-scope R16: an App's self (id, live Seed, health, deploy time).
    get("/cli/app/self", Member),
    // 0.46.9: `k2 app set` (account deletion, client log retention): any login
    // reads; the owner, an Admin or the App's agent (users switch) writes.
    split("/cli/app/settings", Member, Admin),
    // S28 `k2 app shell` (WebSocket; same rules as exec).
    get("/cli/app/shell", Admin),
    post("/cli/app/start", Admin),
    // 0.46.7 (TC-15): k2 app status (a passport: its own workspace's Apps;
    // each part keeps its own route's rule).
    get("/cli/app/status", Member),
    post("/cli/app/stop", Admin),
    // S19: suspend narrows, for the owner and Admins (Rosson 0.46.8: never a
    // Member). A passport: its own workspace's Apps, with no switch (handler).
    post("/cli/app/suspend", Admin),
    // 0.46.4: test guests scoped to one App. Guests are the owner's, as
    // `/cli/skin/users` (an agent passport is refused `owner_only` in the
    // handler while agent test guests are parked).
    post("/cli/app/test-guest/add", Owner),
    post("/cli/app/test-guest/password", Owner),
    post("/cli/app/test-guest/rm", Owner),
    get("/cli/app/test-guests", Member),
    // purge (deletes the App's data) is owner-only in the handler.
    post("/cli/app/uninstall", Admin),
    // Q8: the owner gives a parked App to a workspace.
    post("/cli/app/unpark", Owner),
    // 0.46.7: k2 app updates (the update feed). Reads Member; changes
    // Admin (the handler: switch 1 for agents, a key change after releases
    // held for the owner); the fleet view Admin.
    get("/cli/app/updates", Member),
    get("/cli/app/updates/fleet", Admin),
    get("/cli/app/updates/key", Member),
    post("/cli/app/updates/key/set", Admin),
    post("/cli/app/updates/promote", Admin),
    post("/cli/app/updates/pull", Admin),
    post("/cli/app/updates/push", Admin),
    post("/cli/app/updates/rollback", Admin),
    get("/cli/app/updates/stats", Member),
    get("/cli/app/updates/token", Admin),
    post("/cli/app/updates/token/set", Admin),
    // Room-scope R16: an App's quotas and per-call counts from its audit.
    get("/cli/app/usage", Member),
    post("/cli/app/validate", Admin),
    // App users' personal wallets (0.46.3), the owner's side: existence,
    // label and status only; force-remove; personal agents; per-App
    // settings. Admin floor for logins (the handler also takes the owner
    // token, and a switch-2 agent for its own App's fallback).
    get("/cli/app/wallet/people", Admin),
    post("/cli/app/wallet/personal-agent", Admin),
    get("/cli/app/wallet/personal-agents", Admin),
    post("/cli/app/wallet/remove", Admin),
    split("/cli/app/wallet/settings", Admin, Admin),
    // 0.46.0 grace: what breaks when a service moves to an App.
    get("/cli/app/warnings", Member),
];
