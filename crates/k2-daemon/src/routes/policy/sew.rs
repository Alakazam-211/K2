//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/sew` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Admin, Member, NoLogin, Owner};
use super::{get, post, Route};

pub(super) const ROWS: &[Route] = &[
    // Sew on K2. Floors are for Connect logins; the owner token and agent
    // passports pass to the handlers, which apply the real gates:
    // - S29 server (K12) and S28 keys/pass/handoff: reads for any login,
    //   the lifecycle Admin (or a passport with "Manage Sew"); purge, the
    //   recovery key and drop-prev are person-only (owner + vault confirm).
    // - S30 `k2 chat` agent verbs `/cli/sew/chat/*` incl. community
    //   management `/cli/sew/chat/manage/*` (K17, K20): passports only
    //   (NoLogin); manage also needs Manage Sew + level manage.
    // - S30 approvals and the admin plane (K18, K19): Admin for logins, or
    //   a Manage-Sew passport (no self-loosening; approve is person-only
    //   with a vault confirm unless approvals.by allows agents).
    // - S30 on-behalf opt-ins: the person themselves (the handler refuses
    //   everyone else and passports), so any login reaches the handler.
    get("/cli/sew/admin/agents", Admin),
    post("/cli/sew/admin/agents/grant", Admin),
    post("/cli/sew/admin/agents/revoke", Admin),
    post("/cli/sew/admin/agents/set", Admin),
    // S31 K24: the delivery hub per workspace (Manage Sew for passports).
    get("/cli/sew/admin/deliveries", Admin),
    post("/cli/sew/admin/deliveries/reset", Admin),
    post("/cli/sew/admin/deliveries/set", Admin),
    get("/cli/sew/admin/log", Admin),
    get("/cli/sew/admin/people", Admin),
    post("/cli/sew/admin/people/grant", Admin),
    post("/cli/sew/admin/people/revoke", Admin),
    post("/cli/sew/admin/people/update", Admin),
    // - S32 K31: App ↔ Space bindings (read: any login; bind/unbind: the
    //   Manage Sew gate in the handler, Admin floor for logins).
    // ("app-spaces" sorts before "approvals": '-' < 'r'.)
    // Sew 0.6.0: `k2 app chat <app>` (a read), and the governance switch
    // (the owner or an Admin login in person, with a confirm: the handler).
    get("/cli/sew/app-chat", Member),
    get("/cli/sew/app-spaces", Member),
    post("/cli/sew/app-spaces/bind", Admin),
    post("/cli/sew/app-spaces/governance", Admin),
    // Sew 0.9.0: a Space's recording override (narrowing for the App's own
    // agent too, widening a person's), and the CUI/FCI lock (the owner only,
    // in person: handler).
    post("/cli/sew/app-spaces/recording", Admin),
    post("/cli/sew/app-spaces/recording-lock", Owner),
    post("/cli/sew/app-spaces/unbind", Admin),
    // Sew for Apps A6 (0.46.6): chat.member / chat.whois, an App's own
    // pass with chat:read (the handler refuses the rest).
    // Sew for Apps A9 (0.46.6): chat.createChannel, an App's own pass
    // with chat:channels in its own Space (the handler refuses the rest).
    post("/cli/sew/app/channels", NoLogin),
    // Sew 0.6.0: chat.channels / chat.setChannels, an App's own pass in its
    // own App-governed Space, with a chat key (the handler refuses the rest).
    post("/cli/sew/app/channels/bulk", NoLogin),
    get("/cli/sew/app/member", NoLogin),
    post("/cli/sew/app/member-channels", NoLogin),
    // Sew 0.9.0: recordings.get / transcript / list / events, an App's own
    // pass with server_read, in the Spaces it governs (the handler refuses
    // the rest).
    get("/cli/sew/app/recording", NoLogin),
    get("/cli/sew/app/recording/transcript", NoLogin),
    get("/cli/sew/app/recordings", NoLogin),
    get("/cli/sew/app/recordings/events", NoLogin),
    get("/cli/sew/app/space", NoLogin),
    post("/cli/sew/app/whois", NoLogin),
    get("/cli/sew/approvals", Admin),
    post("/cli/sew/approvals/approve", Admin),
    post("/cli/sew/approvals/approve-batch", Admin),
    post("/cli/sew/approvals/deny", Admin),
    get("/cli/sew/approvals/policy", Admin),
    post("/cli/sew/approvals/policy/set", Admin),
    get("/cli/sew/backups", Member),
    get("/cli/sew/chat/access", NoLogin),
    get("/cli/sew/chat/channels", NoLogin),
    post("/cli/sew/chat/delete", NoLogin),
    post("/cli/sew/chat/dm", NoLogin),
    post("/cli/sew/chat/edit", NoLogin),
    post("/cli/sew/chat/follow", NoLogin),
    post("/cli/sew/chat/manage/ban", NoLogin),
    post("/cli/sew/chat/manage/channel/create", NoLogin),
    post("/cli/sew/chat/manage/channel/perms", NoLogin),
    post("/cli/sew/chat/manage/channel/set", NoLogin),
    post("/cli/sew/chat/manage/role/add", NoLogin),
    post("/cli/sew/chat/manage/role/create", NoLogin),
    post("/cli/sew/chat/manage/role/remove", NoLogin),
    post("/cli/sew/chat/manage/space/create", NoLogin),
    get("/cli/sew/chat/manage/space/members", NoLogin),
    post("/cli/sew/chat/manage/space/members/add", NoLogin),
    post("/cli/sew/chat/manage/space/members/remove", NoLogin),
    post("/cli/sew/chat/manage/timeout", NoLogin),
    post("/cli/sew/chat/manage/unban", NoLogin),
    post("/cli/sew/chat/mark-read", NoLogin),
    get("/cli/sew/chat/messages", NoLogin),
    get("/cli/sew/chat/outbox", NoLogin),
    post("/cli/sew/chat/outbox/cancel", NoLogin),
    // S31 K24: an agent sets its OWN delivery prefs (self-quieting).
    get("/cli/sew/chat/prefs", NoLogin),
    post("/cli/sew/chat/prefs/set", NoLogin),
    get("/cli/sew/chat/read", NoLogin),
    post("/cli/sew/chat/reply", NoLogin),
    post("/cli/sew/chat/send", NoLogin),
    get("/cli/sew/chat/spaces", NoLogin),
    post("/cli/sew/chat/unfollow", NoLogin),
    get("/cli/sew/chat/unread", NoLogin),
    get("/cli/sew/chat/wait", NoLogin),
    get("/cli/sew/chat/whoami", NoLogin),
    // Decision 13 (Sew for Apps, 0.46.6): the home client and an App's
    // "App-only" mark (read: any login; writes: Manage Sew, Admin floor).
    get("/cli/sew/home", Member),
    post("/cli/sew/home/app", Admin),
    post("/cli/sew/home/client", Admin),
    get("/cli/sew/job", Member),
    get("/cli/sew/keys", Admin),
    post("/cli/sew/keys/drop-previous", Admin),
    post("/cli/sew/keys/rotate", Admin),
    post("/cli/sew/media/install", Admin),
    get("/cli/sew/media/preflight", Member),
    post("/cli/sew/media/probe", Admin),
    post("/cli/sew/media/rebind", Admin),
    post("/cli/sew/media/renew", Admin),
    post("/cli/sew/media/tier", Admin),
    post("/cli/sew/media/uninstall", Admin),
    post("/cli/sew/media/url", Admin),
    post("/cli/sew/meet/bind", Admin),
    post("/cli/sew/meet/unbind", Admin),
    get("/cli/sew/on-behalf", Member),
    post("/cli/sew/on-behalf/review", Member),
    post("/cli/sew/owner-handoff", Owner),
    post("/cli/sew/pass", Member),
    get("/cli/sew/preflight", Member),
    // 0.46.11 K-a: Sew's recorder. Reads: any login. Enable: an Owner or
    // Admin in person (vault confirm); disable and uninstall: Manage Sew.
    get("/cli/sew/recording", Member),
    post("/cli/sew/recording/disable", Admin),
    post("/cli/sew/recording/enable", Admin),
    // Sew 0.9.0: the org recording policies (read; set by the owner or an
    // Admin in person, recording.required by the owner: handler). Sorted in
    // with the rest: `lookup` is a binary search.
    get("/cli/sew/recording/policies", Admin),
    post("/cli/sew/recording/policy", Admin),
    get("/cli/sew/recording/preflight", Member),
    post("/cli/sew/recording/uninstall", Admin),
    // S32 (K27): the app URL behind Open Sew as owner (Manage Sew).
    post("/cli/sew/server/app-url", Admin),
    post("/cli/sew/server/backup", Admin),
    post("/cli/sew/server/config", Admin),
    post("/cli/sew/server/disable", Admin),
    post("/cli/sew/server/doctor", Admin),
    post("/cli/sew/server/enable", Admin),
    // S32 (K27): Sew's keyring mirror (Manage Sew; never the vault).
    post("/cli/sew/server/mirror", Admin),
    // 0.46.6 (Sew 2026-10-27): `sew install --reconfigure` by hand, and
    // `k2 hostchat restart` (Manage Sew in the handler).
    post("/cli/sew/server/reconfigure", Admin),
    // 0.46.8 (voice rooms PRD v1.9 §2.7): the launch key is the owner's only.
    post("/cli/sew/server/recovery-key", Owner),
    post("/cli/sew/server/restart", Admin),
    post("/cli/sew/server/restore", Admin),
    post("/cli/sew/server/rollback", Admin),
    post("/cli/sew/server/rotate-master", Admin),
    // S32 (K27): off-box backup copies (Manage Sew; `none` is audited).
    post("/cli/sew/server/ship", Admin),
    post("/cli/sew/server/uninstall", Admin),
    post("/cli/sew/server/upgrade", Admin),
    get("/cli/sew/status", Member),
    get("/cli/sew/users", Admin),
    post("/cli/sew/users/invite", Admin),
    post("/cli/sew/users/owner", Admin),
    post("/cli/sew/users/reactivate", Admin),
    post("/cli/sew/users/reset", Admin),
    post("/cli/sew/users/revoke", Admin),
    post("/cli/sew/users/role", Admin),
];
