//! `/cli/mail/*` route shim — K2 Mail (prd-email-server-v1 §11).
//!
//! THIN dispatcher only: every handler lives in a per-concern file
//! under `crate::mail` so later slices (S1 supervisor, S2 domains,
//! S3 addresses, S4 read/wait, S5 send/approvals, S6 doctor) never
//! collide in one giant routes file. The path → file partition map —
//! FROZEN for later slices:
//!
//! | path                              | handler file            |
//! |-----------------------------------|-------------------------|
//! | GET  /cli/mail/status  (REAL)     | mail/routes_server.rs   |
//! | GET  /cli/mail/preflight (REAL,S1)| mail/routes_server.rs   |
//! | POST /cli/mail/server/enable      | mail/routes_server.rs   |
//! | POST /cli/mail/server/disable     | mail/routes_server.rs   |
//! | POST /cli/mail/server/rotate-admin| mail/routes_server.rs   |
//! | POST /cli/mail/server/uninstall   | mail/routes_server.rs   |
//! | GET  /cli/mail/config             | mail/routes_server.rs   |
//! | POST /cli/mail/config/set         | mail/routes_server.rs   |
//! | GET  /cli/mail/doctor             | mail/routes_server.rs   |
//! | POST /cli/mail/doctor (run, S6)   | mail/routes_server.rs   |
//! | POST /cli/mail/domain/add         | mail/routes_domains.rs  |
//! | POST /cli/mail/domain/remove      | mail/routes_domains.rs  |
//! | POST /cli/mail/domain/check       | mail/routes_domains.rs  |
//! | GET  /cli/mail/domain/list        | mail/routes_domains.rs  |
//! | GET  /cli/mail/domain/show        | mail/routes_domains.rs  |
//! | POST /cli/mail/address/create     | mail/routes_addresses.rs|
//! | POST /cli/mail/address/delete     | mail/routes_addresses.rs|
//! | POST /cli/mail/address/password   | mail/routes_addresses.rs|
//! | GET  /cli/mail/address/list       | mail/routes_addresses.rs|
//! | GET  /cli/mail/messages           | mail/routes_messages.rs |
//! | GET  /cli/mail/read               | mail/routes_messages.rs |
//! | GET  /cli/mail/attachments        | mail/routes_messages.rs |
//! | GET  /cli/mail/wait               | mail/routes_messages.rs |
//! | POST /cli/mail/send               | mail/routes_send.rs     |
//! | POST /cli/mail/reply              | mail/routes_send.rs     |
//! | GET  /cli/mail/outbox             | mail/routes_send.rs     |
//! | GET  /cli/mail/approvals/list     | mail/routes_send.rs     |
//! | POST /cli/mail/approvals/approve  | mail/routes_send.rs     |
//! | POST /cli/mail/approvals/deny     | mail/routes_send.rs     |
//! | POST /cli/mail/external/add       | mail/routes_external.rs |
//! | POST /cli/mail/external/remove    | mail/routes_external.rs |
//! | POST /cli/mail/link/oauth/start   | mail/routes_link_oauth.rs |
//! | POST /cli/mail/link/oauth/complete| mail/routes_link_oauth.rs |
//! | GET  /cli/mail/link/oauth/status  | mail/routes_link_oauth.rs |
//! | POST /cli/mail/oauth-config/set   | mail/routes_oauth_config.rs |
//! | POST /cli/mail/oauth-config/clear | mail/routes_oauth_config.rs |
//! | GET  /cli/mail/oauth-config       | mail/routes_oauth_config.rs |
//! | POST /cli/mail/draft              | mail/routes_external.rs |
//! | GET  /cli/mail/inboxes            | mail/routes_access.rs   |
//! | POST /cli/mail/access/grant       | mail/routes_access.rs   |
//! | POST /cli/mail/access/revoke      | mail/routes_access.rs   |
//! | POST /cli/mail/access/set-primary | mail/routes_access.rs   |
//! | POST /cli/mail/access/set-level   | mail/routes_access.rs   |
//! | POST /cli/mail/access/set-manage  | mail/routes_access.rs   |
//! | POST /cli/mail/move               | mail/routes_messages.rs |
//! | POST /cli/mail/flag               | mail/routes_messages.rs |
//! | POST /cli/mail/archive            | mail/routes_messages.rs |
//! | POST /cli/mail/delete             | mail/routes_messages.rs |
//! | POST /cli/mail/folder/create      | mail/routes_messages.rs |
//! | POST /cli/mail/folder/rename      | mail/routes_messages.rs |
//! | GET  /cli/mail/folder/list        | mail/routes_messages.rs |
//! | GET  /cli/mail/quota              | mail/quota.rs          |
//! | POST /cli/mail/quota              | mail/quota.rs          |
//! | GET  /cli/mail/catchall           | mail/catchall.rs       |
//! | POST /cli/mail/catchall           | mail/catchall.rs       |
//! | GET  /cli/mail/alias              | mail/alias.rs          |
//! | POST /cli/mail/alias              | mail/alias.rs          |
//! | POST /cli/mail/alias/remove       | mail/alias.rs          |
//! | GET  /cli/mail/forward            | mail/forward.rs        |
//! | POST /cli/mail/forward            | mail/forward.rs        |
//! | POST /cli/mail/forward/unset      | mail/forward.rs        |
//! | GET  /cli/mail/ooo                | mail/ooo.rs            |
//! | POST /cli/mail/ooo                | mail/ooo.rs            |
//! | POST /cli/mail/ooo/unset          | mail/ooo.rs            |
//! | GET  /cli/mail/footer             | mail/footer.rs         |
//! | POST /cli/mail/footer             | mail/footer.rs         |
//! | POST /cli/mail/footer/unset       | mail/footer.rs         |
//! | GET  /cli/mail/app-password       | mail/app_password.rs   |
//! | POST /cli/mail/app-password       | mail/app_password.rs   |
//! | POST /cli/mail/app-password/revoke| mail/app_password.rs   |
//! | GET  /cli/mail/dkim               | mail/dkim.rs           |
//! | POST /cli/mail/dkim               | mail/dkim.rs           |
//! | POST /cli/mail/dkim/rotate        | mail/dkim.rs           |
//! | POST /cli/mail/dkim/retire        | mail/dkim.rs           |
//! | GET  /cli/mail/dmarc              | mail/dmarc.rs          |
//! | POST /cli/mail/dmarc              | mail/dmarc.rs          |
//! | POST /cli/mail/dmarc/report-to    | mail/dmarc.rs          |
//! | GET  /cli/mail/ptr                | mail/ptr.rs            |
//! | POST /cli/mail/ptr                | mail/ptr.rs (show)     |
//! | POST /cli/mail/ptr/set            | mail/ptr.rs            |
//! | GET  /cli/mail/bans               | mail/bans.rs           |
//! | POST /cli/mail/bans               | mail/bans.rs (list)    |
//! | POST /cli/mail/bans/clear         | mail/bans.rs           |
//! | GET  /cli/mail/allowlist          | mail/bans.rs           |
//! | POST /cli/mail/allowlist          | mail/bans.rs (list)    |
//! | POST /cli/mail/allowlist/add      | mail/bans.rs           |
//! | POST /cli/mail/allowlist/remove   | mail/bans.rs           |
//! | GET  /cli/mail/bans/migrate       | mail/bans.rs (status)  |
//! | POST /cli/mail/bans/migrate       | mail/bans.rs           |
//! | POST /cli/mail/bans/migrate/restore | mail/bans.rs         |
//!
//! (Family name is `mail`, deliberately NOT `inbox` — that collides
//! with K2's internal `/cli/inbox/*` queue, PRD §11.)
//!
//! Method gating follows the house rules (feedback_post_only_route_
//! guards): mutations are JSON-bodied POSTs listed in the dispatcher's
//! `post_allowed` allowlist + handled by [`dispatch_post`] behind
//! `require_post` + `token_ok` (+ `token_is_owner_or_admin` for the
//! owner-level paths — see the dispatcher's `/cli/mail/` arm); reads
//! are GETs through `crate::cli::dispatch` → [`dispatch`], which also
//! answers 405 for mutations reached via the GET chain.
//!
//! REAL as of S1+S2+S3+S4+S5: `/cli/mail/status` (the capability-
//! gating seam the Settings→Email page reads, pre-mortem #15),
//! `/cli/mail/preflight`, the server lifecycle mutations
//! (enable/disable/uninstall), the S2 domain family
//! (`domain/add|remove|check|list|show`), the S3 address family
//! (`address/create|delete|list`), the S4 read family
//! (`messages|read|attachments|wait` — note: the dispatcher gives
//! those four GETs their own `spawn_blocking` arm, since they dial
//! Stalwart over blocking reqwest and `wait` holds the request up to
//! 900 s), and the S5 send family (`send|reply|outbox|approvals/*` —
//! POSTs already run in the mail POST arm's `spawn_blocking`; the
//! `approvals/list` GET has its own dispatcher clause adding the
//! owner-or-admin gate, §11.1.3: owner verbs hard-fail for agent
//! tokens server-side). S6 completes the family: `config` +
//! `config/set` (the owner config surface) and `doctor` — GET = the
//! latest PERSISTED run (read-only, never probes), POST = run the
//! probes now (owner-level, in the POST arm's `spawn_blocking`:
//! blocking DNS/TCP/SMTP I/O). Every reserved path is now REAL.
//!
//! S9 extends the family with EXTERNAL assistant inboxes (PRD §17.5):
//! `external/add|remove` (owner POSTs) + `external/list` (owner GET,
//! own dispatcher clause like `approvals/list`) + `draft` (workspace-
//! token POST — the agent's save-a-reply-draft verb; it dials the
//! user's IMAP host, so it rides the POST arm's `spawn_blocking`).
//! Reads of external MESSAGES have no routes here — they flow through
//! the EXISTING `messages|read|attachments|wait` handlers via the
//! §17.5 `backend_for_address` seam.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::{
    routes_access, routes_addresses, routes_domains, routes_external, routes_link_oauth,
    routes_messages, routes_oauth_config, routes_send, routes_server,
};

/// Mail-domain GET dispatch. Returns `Some(resp)` for a handled path,
/// `None` if the path isn't a mail route. Claims the WHOLE
/// `/cli/mail/` prefix: unknown sub-paths 404 here (clearer than the
/// top-level catch-all for a reserved family).
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !path.starts_with("/cli/mail/") {
        return None;
    }
    let resp = match path {
        // ── Reads ───────────────────────────────────────────────────
        "/cli/mail/status" => routes_server::handle_status(params),
        // S1: read-only preflight checklist (PRD §5.1). GET-only by
        // design — it mutates nothing, so it is deliberately NOT in
        // the dispatcher's post_allowed list.
        "/cli/mail/preflight" => routes_server::handle_preflight(params),
        "/cli/mail/config" => routes_server::handle_config_get(params),
        // GET = the latest persisted run only; POST /cli/mail/doctor
        // (the probe run) arrives via dispatch_post — one path, two
        // methods, matching §11's `k2 mail doctor` verb.
        "/cli/mail/doctor" => routes_server::handle_doctor(params),
        "/cli/mail/domain/list" => routes_domains::handle_domain_list(params),
        "/cli/mail/domain/show" => routes_domains::handle_domain_show(params),
        "/cli/mail/address/list" => routes_addresses::handle_address_list(params),
        "/cli/mail/messages" => routes_messages::handle_messages(params),
        "/cli/mail/read" => routes_messages::handle_read(params),
        "/cli/mail/attachments" => routes_messages::handle_attachments(params),
        "/cli/mail/wait" => routes_messages::handle_wait(params),
        "/cli/mail/outbox" => routes_send::handle_outbox(params),
        "/cli/mail/approvals/list" => routes_send::handle_approvals_list(params),
        // S11/E1: unified inbox catalog (agent view via proven principal
        // or residual project=; owner view — no identity — is
        // owner-or-admin-gated in the dispatcher's dual-auth arm).
        "/cli/mail/inboxes" => routes_access::handle_inboxes(params),
        // 0081: list an inbox's folders (can_manage; workspace token).
        "/cli/mail/folder/list" => routes_messages::handle_folder_list(params),
        // O4: the OAuth-link long-poll target (owner-gated in the
        // dispatcher's own clause, like approvals/list). Returns only
        // {state, address?, hint?} — never a code/token.
        "/cli/mail/link/oauth/status" => routes_link_oauth::handle_link_oauth_status(params),
        // S1 BYO OAuth client: the owner reads their per-provider client
        // config (owner-gated in the dispatcher's own clause, like
        // approvals/list). Reports {source, clientId, secretSet} per
        // provider — NEVER the secret value.
        "/cli/mail/oauth-config" => routes_oauth_config::handle_oauth_config_get(params),
        "/cli/mail/quota" => crate::mail::quota::handle_quota_get(params),
        "/cli/mail/catchall" => crate::mail::catchall::handle_catchall_get(params),
        "/cli/mail/alias" => crate::mail::alias::handle_alias_list(params),
        "/cli/mail/forward" => crate::mail::forward::handle_forward_get(params),
        "/cli/mail/ooo" => crate::mail::ooo::handle_ooo_get(params),
        "/cli/mail/footer" => crate::mail::footer::handle_footer_get(params),
        "/cli/mail/list" => crate::mail::lists::handle_list_get(params),
        "/cli/mail/list/members" => crate::mail::lists::handle_members_get(params),
        "/cli/mail/spam/quarantine" => crate::mail::spam::handle_quarantine_get(params),
        "/cli/mail/queue" => crate::mail::queue::handle_queue_get(params),
        "/cli/mail/acl" => crate::mail::acl::handle_acl_get(params),
        "/cli/mail/autoconfig" => crate::mail::autoconfig::handle_autoconfig_get(params),
        "/cli/mail/app-password" => crate::mail::app_password::handle_app_password_get(params),
        "/cli/mail/dkim" => crate::mail::dkim::handle_dkim_get(params),
        "/cli/mail/dmarc" => crate::mail::dmarc::handle_dmarc_get(params),
        "/cli/mail/ptr" => crate::mail::ptr::handle_ptr_show(params),
        "/cli/mail/bans" => crate::mail::bans::handle_bans_list(params),
        "/cli/mail/allowlist" => crate::mail::bans::handle_allowlist_list(params),
        "/cli/mail/bans/migrate" => crate::mail::bans::handle_migrate_status(params),

        // ── POST-only mutations reached via the GET chain → 405 ─────
        // (feedback_post_only_route_guards house rule.)
        "/cli/mail/server/enable"
        | "/cli/mail/server/disable"
        | "/cli/mail/server/rotate-admin"
        | "/cli/mail/server/uninstall"
        | "/cli/mail/config/set"
        | "/cli/mail/domain/add"
        | "/cli/mail/domain/remove"
        | "/cli/mail/domain/check"
        | "/cli/mail/address/create"
        | "/cli/mail/address/delete"
        | "/cli/mail/address/password"
        | "/cli/mail/send"
        | "/cli/mail/reply"
        | "/cli/mail/outbox/cancel"
        | "/cli/mail/approvals/approve"
        | "/cli/mail/approvals/deny"
        | "/cli/mail/external/add"
        | "/cli/mail/external/remove"
        // `link/*` = the Settings UI's names for the same link-provisioning
        // handlers (the CLI verb is `k2 mail link`); aliased below.
        | "/cli/mail/link/add"
        | "/cli/mail/link/remove"
        // O4: begin / complete an OAuth link (POST-only; GET → 405).
        | "/cli/mail/link/oauth/start"
        | "/cli/mail/link/oauth/complete"
        // S1 BYO OAuth client set/clear (POST-only; GET on them → 405).
        | "/cli/mail/oauth-config/set"
        | "/cli/mail/oauth-config/clear"
        | "/cli/mail/access/grant"
        | "/cli/mail/access/revoke"
        | "/cli/mail/access/set-primary"
        | "/cli/mail/access/set-level"
        | "/cli/mail/access/set-manage"
        | "/cli/mail/move"
        | "/cli/mail/flag"
        | "/cli/mail/archive"
        | "/cli/mail/delete"
        | "/cli/mail/folder/create"
        | "/cli/mail/folder/rename"
        | "/cli/mail/draft"
        | "/cli/mail/import"
        | "/cli/mail/cert/renew"
        | "/cli/mail/alias/remove"
        | "/cli/mail/forward/unset"
        | "/cli/mail/ooo/unset"
        | "/cli/mail/footer/unset"
        | "/cli/mail/list/delete"
        | "/cli/mail/spam/train"
        | "/cli/mail/spam/allow"
        | "/cli/mail/spam/block"
        | "/cli/mail/spam/quarantine/release"
        | "/cli/mail/spam/quarantine/discard"
        | "/cli/mail/queue/retry"
        | "/cli/mail/queue/drop"
        | "/cli/mail/acl/revoke"
        | "/cli/mail/app-password/revoke"
        | "/cli/mail/dkim/rotate"
        | "/cli/mail/dkim/retire"
        | "/cli/mail/dmarc/report-to"
        | "/cli/mail/ptr/set"
        | "/cli/mail/bans/clear"
        | "/cli/mail/allowlist/add"
        | "/cli/mail/allowlist/remove"
        | "/cli/mail/bans/migrate/restore" => CliResponse::method_not_allowed(),

        _ => CliResponse::not_found(),
    };
    Some(resp)
}

/// Dispatch a `/cli/mail/*` POST body to its per-concern handler.
/// Exact-match paths; unknown paths 404 (mirrors
/// `feedback_routes::dispatch_post`). The caller (the dispatcher's
/// `/cli/mail/` arm) has already enforced require_post + token_ok +
/// the owner-or-admin gate for owner-level paths.
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    dispatch_post_at(path, body, None)
}

/// Same as [`dispatch_post`], with the live daemon HTTP port for mail
/// Enable → `skin_door::apply` (dispatcher `state.port`).
pub fn dispatch_post_at(path: &str, body: &[u8], daemon_port: Option<u16>) -> CliResponse {
    match path {
        "/cli/mail/server/enable" => routes_server::handle_server_enable_at(body, daemon_port),
        "/cli/mail/server/disable" => routes_server::handle_server_disable(body),
        "/cli/mail/server/rotate-admin" => routes_server::handle_server_rotate_admin(body),
        "/cli/mail/server/uninstall" => routes_server::handle_server_uninstall(body),
        "/cli/mail/config/set" => routes_server::handle_config_set(body),
        "/cli/mail/doctor" => routes_server::handle_doctor_run(body),
        "/cli/mail/domain/add" => routes_domains::handle_domain_add(body),
        "/cli/mail/domain/remove" => routes_domains::handle_domain_remove(body),
        "/cli/mail/domain/check" => routes_domains::handle_domain_check(body),
        "/cli/mail/address/create" => routes_addresses::handle_address_create(body),
        "/cli/mail/address/delete" => routes_addresses::handle_address_delete(body),
        "/cli/mail/address/password" => routes_addresses::handle_address_password(body),
        "/cli/mail/send" => routes_send::handle_send(body),
        "/cli/mail/reply" => routes_send::handle_reply(body),
        "/cli/mail/outbox/cancel" => routes_send::handle_outbox_cancel(body),
        "/cli/mail/approvals/approve" => routes_send::handle_approvals_approve(body),
        "/cli/mail/approvals/deny" => routes_send::handle_approvals_deny(body),
        "/cli/mail/external/add" => routes_external::handle_external_add(body),
        "/cli/mail/external/remove" => routes_external::handle_external_remove(body),
        // `link/*` aliases — the Settings → Email Link UI POSTs these names
        // for the same link-provisioning handlers the `k2 mail link` CLI uses
        // via `external/*`. Same handlers, no behavior difference.
        "/cli/mail/link/add" => routes_external::handle_external_add(body),
        "/cli/mail/link/remove" => routes_external::handle_external_remove(body),
        // O4: begin / complete an OAuth link (Gmail loopback / Microsoft
        // device flow / remote client-capture complete).
        // Owner-gated (is_owner_level_mutation's /cli/mail/link/oauth/
        // prefix); the server-side flow runs in this arm's spawn_blocking.
        "/cli/mail/link/oauth/start" => routes_link_oauth::handle_link_oauth_start(body),
        "/cli/mail/link/oauth/complete" => routes_link_oauth::handle_link_oauth_complete(body),
        // S1 BYO OAuth client: owner sets/clears their per-provider client.
        "/cli/mail/oauth-config/set" => routes_oauth_config::handle_oauth_config_set(body),
        "/cli/mail/oauth-config/clear" => routes_oauth_config::handle_oauth_config_clear(body),
        "/cli/mail/access/grant" => routes_access::handle_grant(body),
        "/cli/mail/access/revoke" => routes_access::handle_revoke(body),
        "/cli/mail/access/set-primary" => routes_access::handle_set_primary(body),
        "/cli/mail/access/set-level" => routes_access::handle_set_level(body),
        "/cli/mail/access/set-manage" => routes_access::handle_set_manage(body),
        // 0081 management/delete action verbs (workspace token; can_manage
        // / can_delete gated in the handlers, NOT owner-level).
        "/cli/mail/move" => routes_messages::handle_move(body),
        "/cli/mail/flag" => routes_messages::handle_flag(body),
        "/cli/mail/archive" => routes_messages::handle_archive(body),
        "/cli/mail/delete" => routes_messages::handle_delete(body),
        "/cli/mail/folder/create" => routes_messages::handle_folder_create(body),
        "/cli/mail/folder/rename" => routes_messages::handle_folder_rename(body),
        "/cli/mail/draft" => routes_external::handle_draft(body),
        "/cli/mail/import" => crate::mail::import::handle_import(body),
        "/cli/mail/quota" => crate::mail::quota::handle_quota_set(body),
        "/cli/mail/catchall" => crate::mail::catchall::handle_catchall_post(body),
        "/cli/mail/alias" => crate::mail::alias::handle_alias_add(body),
        "/cli/mail/alias/remove" => crate::mail::alias::handle_alias_remove(body),
        "/cli/mail/forward" => crate::mail::forward::handle_forward_set(body),
        "/cli/mail/forward/unset" => crate::mail::forward::handle_forward_unset(body),
        "/cli/mail/ooo" => crate::mail::ooo::handle_ooo_set(body),
        "/cli/mail/ooo/unset" => crate::mail::ooo::handle_ooo_unset(body),
        "/cli/mail/footer" => crate::mail::footer::handle_footer_set(body),
        "/cli/mail/footer/unset" => crate::mail::footer::handle_footer_unset(body),
        "/cli/mail/app-password" => crate::mail::app_password::handle_app_password_add(body),
        "/cli/mail/app-password/revoke" => {
            crate::mail::app_password::handle_app_password_revoke(body)
        }
        "/cli/mail/cert/renew" => routes_server::handle_cert_renew(body),
        "/cli/mail/list" => crate::mail::lists::handle_list_create(body),
        "/cli/mail/list/members" => crate::mail::lists::handle_members_post(body),
        "/cli/mail/list/delete" => crate::mail::lists::handle_list_delete(body),
        "/cli/mail/spam/train" => crate::mail::spam::handle_train(body),
        "/cli/mail/spam/allow" => crate::mail::spam::handle_allow(body),
        "/cli/mail/spam/block" => crate::mail::spam::handle_block(body),
        "/cli/mail/spam/quarantine/release" => crate::mail::spam::handle_quarantine_release(body),
        "/cli/mail/spam/quarantine/discard" => crate::mail::spam::handle_quarantine_discard(body),
        "/cli/mail/queue/retry" => crate::mail::queue::handle_retry(body),
        "/cli/mail/queue/drop" => crate::mail::queue::handle_drop(body),
        "/cli/mail/acl" => crate::mail::acl::handle_acl_grant(body),
        "/cli/mail/acl/revoke" => crate::mail::acl::handle_acl_revoke(body),
        "/cli/mail/autoconfig" => crate::mail::autoconfig::handle_autoconfig_apply(body),
        "/cli/mail/dkim" | "/cli/mail/dkim/rotate" => crate::mail::dkim::handle_dkim_rotate(body),
        "/cli/mail/dkim/retire" => crate::mail::dkim::handle_dkim_retire(body),
        "/cli/mail/dmarc" | "/cli/mail/dmarc/report-to" => {
            crate::mail::dmarc::handle_dmarc_report_to(body)
        }
        // Dual GET+POST show; set is a separate path.
        "/cli/mail/ptr" => crate::mail::ptr::handle_ptr_show_post(body),
        "/cli/mail/ptr/set" => crate::mail::ptr::handle_ptr_set(body),
        "/cli/mail/bans" => crate::mail::bans::handle_bans_list_post(body),
        "/cli/mail/bans/clear" => crate::mail::bans::handle_bans_clear(body),
        "/cli/mail/allowlist" => crate::mail::bans::handle_allowlist_list_post(body),
        "/cli/mail/allowlist/add" => crate::mail::bans::handle_allowlist_add(body),
        "/cli/mail/allowlist/remove" => crate::mail::bans::handle_allowlist_remove(body),
        "/cli/mail/bans/migrate" => crate::mail::bans::handle_migrate_post(body),
        "/cli/mail/bans/migrate/restore" => crate::mail::bans::handle_migrate_restore(body),
        _ => CliResponse::not_found(),
    }
}

/// Owner-level `/cli/mail/*` mutations — the paths the dispatcher's
/// POST arm additionally gates with `token_is_owner_or_admin` (PRD
/// §10: server enable/disable/uninstall + domain add/remove/check +
/// mode/relay config + approvals + S9 external-inbox CRUD =
/// owner-or-admin; address create/delete + send/reply + S9 `draft`
/// stay workspace-token so agents can act).
pub fn is_owner_level_mutation(path: &str) -> bool {
    path.starts_with("/cli/mail/server/")
        || path.starts_with("/cli/mail/domain/")
        || path.starts_with("/cli/mail/config/")
        || path.starts_with("/cli/mail/approvals/")
        // S9: connecting/removing the user's OWN external account is
        // owner surface (it binds credentials + a workspace); the
        // agent verb on that inbox is `draft`, which is deliberately
        // NOT in this set.
        || path.starts_with("/cli/mail/external/")
        // O4: beginning an OAuth link (Gmail/Microsoft) is owner surface
        // (it binds tokens + a workspace). The `status` GET is owner-gated
        // separately in the dispatcher; it never reaches this classifier.
        || path.starts_with("/cli/mail/link/oauth/")
        // S1: bring-your-own OAuth client set/clear is owner surface (it
        // stores the owner's OAuth client id + a token-grade secret). The
        // GET is owner-gated separately in the dispatcher; it never reaches
        // this classifier (the prefix has a trailing slash so it matches
        // only /cli/mail/oauth-config/{set,clear}).
        || path.starts_with("/cli/mail/oauth-config/")
        // S11: the PRIMARY/owner access-management surface (grant /
        // revoke / set-primary / set-level) is owner-or-admin.
        || path.starts_with("/cli/mail/access/")
        // The doctor RUN (POST /cli/mail/doctor) is an owner verb
        // (§11/§11.1.3); the GET on the same path is a plain read and
        // never reaches this classifier (it only sees POSTs).
        || path == "/cli/mail/doctor"
}

/// Owner/admin hostmail surfaces (POST + GET) that a valid scoped agent
/// passport must not drive. Broader than [`is_owner_level_mutation`]:
/// includes read-only owner GETs (`/cli/mail/config`, oauth-config, …)
/// that match the session_token DENY_PREFIXES for hostmail.
///
/// #34: when a valid scoped token hits these paths, answer `owner_only`
/// exit-3 teaching — not opaque "invalid or missing token".
pub fn is_mail_owner_surface(path: &str) -> bool {
    is_owner_level_mutation(path)
        // GET config / oauth-config (no trailing slash — not covered by
        // the mutation prefixes that end in `/`).
        || path == "/cli/mail/config"
        || path == "/cli/mail/oauth-config"
        // link/* aliases for external (DENY_PREFIX `/cli/mail/link/`).
        || path.starts_with("/cli/mail/link/")
        // Toggle writer: agents cannot self-grant.
        || path == "/cli/mail-manage"
        || path == "/cli/mail/import"
        || path == "/cli/mail/cert/renew"
        || path == "/cli/mail/server/rotate-admin"
}

/// `GET /cli/mail/domain/list` uses client `project=` to pick the owner
/// table (no param) vs the agent verified-only view (`k2 mail domains`).
/// `stamp_principal` always writes `project` for a scoped cell — that
/// must not flip `k2 hostmail domain list` onto the agent view (H8).
/// Capture the client keys BEFORE stamp; restore (or strip) after.
pub fn restore_domain_list_audience(
    path: &str,
    params: &mut HashMap<String, String>,
    client_project: Option<String>,
) {
    if path != "/cli/mail/domain/list" {
        return;
    }
    match client_project.filter(|s| !s.trim().is_empty()) {
        Some(t) => {
            params.insert("project".to_string(), t.clone());
            params.insert("project_path".to_string(), t);
        }
        None => {
            params.remove("project");
            params.remove("project_path");
        }
    }
}

/// Snapshot of client `project` / `project_path` before identity stamp.
pub fn client_project_param(params: &HashMap<String, String>) -> Option<String> {
    ["project", "project_path"]
        .iter()
        .find_map(|k| params.get(*k))
        .cloned()
        .filter(|s| !s.trim().is_empty())
}

/// M5 hostmail manage paths the workspace toggle can open for a scoped
/// passport. Exact paths only — never prefix `/cli/mail/server/` or
/// `/cli/mail/domain/` (uninstall / remove stay M6). C5b (C21/C31)
/// adds access/doctor/approvals/config-set/import; leftover M6 stays
/// owner.
pub fn is_mail_manage_surface(path: &str) -> bool {
    matches!(
        path,
        "/cli/mail/server/enable"
            | "/cli/mail/server/disable"
            | "/cli/mail/domain/list"
            | "/cli/mail/domain/show"
            | "/cli/mail/domain/add"
            | "/cli/mail/domain/check"
            | "/cli/mail/access/grant"
            | "/cli/mail/access/revoke"
            | "/cli/mail/access/set-primary"
            | "/cli/mail/access/set-level"
            | "/cli/mail/access/set-manage"
            | "/cli/mail/doctor"
            | "/cli/mail/approvals/list"
            | "/cli/mail/approvals/approve"
            | "/cli/mail/approvals/deny"
            | "/cli/mail/config/set"
            | "/cli/mail/import"
            | "/cli/mail/quota"
            | "/cli/mail/catchall"
            | "/cli/mail/alias"
            | "/cli/mail/alias/remove"
            | "/cli/mail/forward"
            | "/cli/mail/forward/unset"
            | "/cli/mail/ooo"
            | "/cli/mail/ooo/unset"
            | "/cli/mail/footer"
            | "/cli/mail/footer/unset"
            | "/cli/mail/cert/renew"
            | "/cli/mail/server/rotate-admin"
            | "/cli/mail/address/password"
            | "/cli/mail/list"
            | "/cli/mail/list/members"
            | "/cli/mail/list/delete"
            | "/cli/mail/spam/train"
            | "/cli/mail/spam/allow"
            | "/cli/mail/spam/block"
            | "/cli/mail/spam/quarantine"
            | "/cli/mail/spam/quarantine/release"
            | "/cli/mail/spam/quarantine/discard"
            | "/cli/mail/queue"
            | "/cli/mail/queue/retry"
            | "/cli/mail/queue/drop"
            | "/cli/mail/acl"
            | "/cli/mail/acl/revoke"
            | "/cli/mail/autoconfig"
            | "/cli/mail/app-password"
            | "/cli/mail/app-password/revoke"
            | "/cli/mail/dkim"
            | "/cli/mail/dkim/rotate"
            | "/cli/mail/dkim/retire"
            | "/cli/mail/dmarc"
            | "/cli/mail/dmarc/report-to"
            | "/cli/mail/ptr"
            | "/cli/mail/ptr/set"
            | "/cli/mail/bans"
            | "/cli/mail/bans/clear"
            | "/cli/mail/allowlist"
            | "/cli/mail/allowlist/add"
            | "/cli/mail/allowlist/remove"
            | "/cli/mail/bans/migrate"
            | "/cli/mail/bans/migrate/restore"
    )
}

const OWNER_ONLY_HINT: &str = "requires owner/admin — ask your human (k2 hostmail and mail access/link/domain/server/config/approvals/doctor are owner surfaces)";

const OWNER_ONLY_MANAGE_HINT: &str = "requires owner/admin — ask your human (k2 hostmail and mail access/link/domain/server/config/approvals/doctor are owner surfaces). To let this workspace's agent manage hosted mail on this host, Settings → Workspaces → (this workspace) → Allow agents to manage hosted mail on this host.";

/// C33: flag-on leftover M6 names the missing verb, not the Settings toggle.
fn leftover_m6_hint(path: &str) -> String {
    let verb = if path == "/cli/mail/server/uninstall" {
        "k2 hostmail uninstall"
    } else if path == "/cli/mail/domain/remove" {
        "k2 hostmail domain remove"
    } else if path.starts_with("/cli/mail/oauth-config") {
        "k2 mail oauth-config"
    } else if path.starts_with("/cli/mail/link/") {
        "k2 mail link"
    } else if path.starts_with("/cli/mail/external/") {
        "k2 mail link"
    } else if path == "/cli/mail-manage" {
        "POST /cli/mail-manage"
    } else {
        path
    };
    format!("requires owner/admin — '{verb}' stays owner-only. Ask your human.")
}

fn owner_only_with_hint(hint: &str) -> crate::cli_response::CliResponse {
    crate::cli_response::CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "owner_only",
                "hint": hint,
            },
        })
        .to_string(),
    }
}

/// Stable teaching response for agent tokens on hostmail / mail-owner
/// surfaces (GH #34). CLI maps `owner_only` + 403 → exit 3.
pub fn owner_only_response() -> crate::cli_response::CliResponse {
    owner_only_with_hint(OWNER_ONLY_HINT)
}

/// Teaching response for M5 hostmail manage when a valid scoped hook
/// hits a workspace whose hosted-mail toggle is OFF.
pub fn owner_only_manage_response() -> crate::cli_response::CliResponse {
    owner_only_with_hint(OWNER_ONLY_MANAGE_HINT)
}

/// M15 extra gate after dual-auth. M5 is allowed iff owner-or-admin
/// **or** (scoped `HookPrincipal` AND `mail_manage_allowed_for_path` on
/// that principal's workspace). Flag does not replace Admin. M6 stay
/// owner-or-admin only. Flag key is principal uuid → path, never client
/// `project=`. Used by the TCP extra gate **and** the cell UDS mail arm.
pub fn mail_manage_authorized(
    path: &str,
    is_owner_or_admin: bool,
    principal: Option<&crate::session_token::HookPrincipal>,
) -> Result<(), crate::cli_response::CliResponse> {
    let needs_gate = is_owner_level_mutation(path) || is_mail_manage_surface(path);
    if !needs_gate {
        return Ok(());
    }
    if is_owner_or_admin {
        return Ok(());
    }
    if is_mail_manage_surface(path) {
        if let Some(p) = principal {
            let ws_path = crate::workspace_msg::resolve_workspace(&p.workspace_uuid);
            if ws_path
                .as_deref()
                .is_some_and(k2_core::workspace::settings::mail_manage_allowed_for_path)
            {
                return Ok(());
            }
            return Err(owner_only_manage_response());
        }
        return Err(owner_only_response());
    }
    Err(owner_only_with_hint(&leftover_m6_hint(path)))
}

fn enable_field(v: &serde_json::Value) -> Option<bool> {
    let raw = v.get("enable")?;
    if let Some(n) = raw.as_i64() {
        return match n {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    if let Some(s) = raw.as_str() {
        return match s.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
    }
    None
}

/// `POST /cli/mail-manage` `{project, enable: 0|1}` — owner-or-admin
/// writer for `projects.mail_manage_enabled`. Not `workspace/set`.
pub fn handle_mail_manage(body: &[u8]) -> crate::cli_response::CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) if body.iter().all(|b| b.is_ascii_whitespace()) => serde_json::json!({}),
        Err(e) => {
            return crate::cli_response::CliResponse::bad_request(format!("invalid JSON body: {e}"))
        }
    };
    let project = v
        .get("project")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(project) = project else {
        return crate::cli_response::CliResponse::bad_request("missing project");
    };
    let Some(enable) = enable_field(&v) else {
        return crate::cli_response::CliResponse::bad_request("enable must be 0 or 1");
    };
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return crate::workspace_routes::workspace_not_found_response(project);
    };
    match k2_core::workspace::settings::set_mail_manage_enabled(&path, enable) {
        Ok(()) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncProjects,
                serde_json::Value::Null,
            );
            crate::cli_response::CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "mailManageEnabled": enable,
                })
                .to_string(),
            )
        }
        Err(e) => crate::cli_response::CliResponse::bad_request(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — shim wiring
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_domain_list_audience_strips_stamped_project() {
        let mut params = HashMap::new();
        params.insert("project".to_string(), "/stamped/ws".to_string());
        params.insert("project_path".to_string(), "/stamped/ws".to_string());
        restore_domain_list_audience("/cli/mail/domain/list", &mut params, None);
        assert!(
            params.get("project").is_none(),
            "H8: hostmail domain list must not inherit stamp_principal project="
        );
        assert!(params.get("project_path").is_none());

        let mut params = HashMap::new();
        params.insert("project".to_string(), "/stamped/ws".to_string());
        restore_domain_list_audience(
            "/cli/mail/domain/list",
            &mut params,
            Some("/client/ws".to_string()),
        );
        assert_eq!(
            params.get("project").map(String::as_str),
            Some("/client/ws")
        );
        assert_eq!(
            params.get("project_path").map(String::as_str),
            Some("/client/ws")
        );

        let mut params = HashMap::new();
        params.insert("project".to_string(), "/stamped/ws".to_string());
        restore_domain_list_audience("/cli/mail/status", &mut params, None);
        assert_eq!(
            params.get("project").map(String::as_str),
            Some("/stamped/ws"),
            "other mail GETs keep the stamp"
        );
    }

    /// GH #34: hostmail / access / link / config GETs are owner surfaces
    /// even when not in the POST-only mutation classifier (no trailing
    /// slash on config/oauth-config; link/* aliases).
    #[test]
    fn is_mail_owner_surface_covers_hostmail_and_access() {
        for p in [
            "/cli/mail/domain/list",
            "/cli/mail/domain/add",
            "/cli/mail/server/enable",
            "/cli/mail/config",
            "/cli/mail/config/set",
            "/cli/mail/oauth-config",
            "/cli/mail/approvals/list",
            "/cli/mail/access/grant",
            "/cli/mail/link/oauth/start",
            "/cli/mail/doctor",
            "/cli/mail-manage",
        ] {
            assert!(is_mail_owner_surface(p), "expected owner surface: {p}");
        }
        // Agent mail verbs stay open (not owner surface).
        for p in [
            "/cli/mail/messages",
            "/cli/mail/inboxes",
            "/cli/mail/send",
            "/cli/mail/status",
            "/cli/mail/read",
        ] {
            assert!(!is_mail_owner_surface(p), "must NOT be owner surface: {p}");
        }
    }

    #[test]
    fn owner_only_response_has_stable_code_and_403() {
        let r = owner_only_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "owner_only");
        let hint = v["error"]["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("owner") || hint.contains("human"),
            "hint should teach owner/human: {hint}"
        );
        assert!(
            !hint.contains("Allow agents to manage hosted mail"),
            "generic hint must not name the Settings row: {hint}"
        );
    }

    #[test]
    fn is_mail_manage_surface_is_exact_m5() {
        for p in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/domain/list",
            "/cli/mail/domain/show",
            "/cli/mail/domain/add",
            "/cli/mail/domain/check",
            "/cli/mail/access/grant",
            "/cli/mail/access/revoke",
            "/cli/mail/access/set-primary",
            "/cli/mail/access/set-level",
            "/cli/mail/access/set-manage",
            "/cli/mail/doctor",
            "/cli/mail/approvals/list",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
            "/cli/mail/config/set",
            "/cli/mail/import",
            "/cli/mail/quota",
            "/cli/mail/catchall",
            "/cli/mail/alias",
            "/cli/mail/alias/remove",
            "/cli/mail/forward",
            "/cli/mail/forward/unset",
            "/cli/mail/ooo",
            "/cli/mail/ooo/unset",
            "/cli/mail/footer",
            "/cli/mail/footer/unset",
            "/cli/mail/cert/renew",
            "/cli/mail/server/rotate-admin",
            "/cli/mail/address/password",
            "/cli/mail/list",
            "/cli/mail/list/members",
            "/cli/mail/list/delete",
            "/cli/mail/spam/train",
            "/cli/mail/spam/allow",
            "/cli/mail/spam/block",
            "/cli/mail/spam/quarantine",
            "/cli/mail/spam/quarantine/release",
            "/cli/mail/spam/quarantine/discard",
            "/cli/mail/queue",
            "/cli/mail/queue/retry",
            "/cli/mail/queue/drop",
            "/cli/mail/acl",
            "/cli/mail/acl/revoke",
            "/cli/mail/autoconfig",
            "/cli/mail/app-password",
            "/cli/mail/app-password/revoke",
            "/cli/mail/dkim",
            "/cli/mail/dkim/rotate",
            "/cli/mail/dkim/retire",
            "/cli/mail/dmarc",
            "/cli/mail/dmarc/report-to",
            "/cli/mail/ptr",
            "/cli/mail/ptr/set",
            "/cli/mail/bans",
            "/cli/mail/bans/clear",
            "/cli/mail/allowlist",
            "/cli/mail/allowlist/add",
            "/cli/mail/allowlist/remove",
            "/cli/mail/bans/migrate",
            "/cli/mail/bans/migrate/restore",
        ] {
            assert!(is_mail_manage_surface(p), "M5: {p}");
        }
        for p in [
            "/cli/mail/server/uninstall",
            "/cli/mail/domain/remove",
            "/cli/mail/oauth-config",
            "/cli/mail/address/create",
            "/cli/mail/status",
            "/cli/mail/config",
            "/cli/mail-manage",
            "/cli/mail/link/oauth/start",
            "/cli/mail/external/add",
            "/cli/mail/domain/dkim",
            "/cli/mail/domain/dmarc",
        ] {
            assert!(!is_mail_manage_surface(p), "not M5: {p}");
        }
    }

    #[test]
    fn owner_only_manage_response_names_settings_row() {
        let r = owner_only_manage_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only");
        let hint = v["error"]["hint"].as_str().unwrap_or("");
        assert!(hint.contains("ask your human"), "got {hint}");
        assert!(
            hint.contains("Allow agents to manage hosted mail on this host"),
            "flag-off hint must name the Settings row: {hint}"
        );
    }

    #[test]
    fn mail_manage_authorized_admin_and_flag_split() {
        let p = crate::session_token::HookPrincipal {
            workspace_uuid: "no-such-ws".to_string(),
            agent_address: "agent".to_string(),
        };
        assert!(mail_manage_authorized("/cli/mail/send", false, Some(&p)).is_ok());
        assert!(mail_manage_authorized("/cli/mail/server/enable", true, None).is_ok());
        assert!(mail_manage_authorized("/cli/mail/server/uninstall", true, None).is_ok());
        let m5 = mail_manage_authorized("/cli/mail/server/disable", false, Some(&p));
        assert!(m5.is_err(), "unknown principal path must fail closed");
        let body = m5.err().unwrap().body;
        assert!(body.contains("owner_only"), "{body}");
        assert!(
            body.contains("Allow agents to manage hosted mail"),
            "M5 flag-off names Settings: {body}"
        );
        let m6 = mail_manage_authorized("/cli/mail/server/uninstall", false, Some(&p));
        assert!(m6.is_err());
        let body = m6.err().unwrap().body;
        assert!(body.contains("owner_only"), "{body}");
        assert!(
            !body.contains("Allow agents to manage hosted mail"),
            "M6 must not promise the toggle: {body}"
        );
        assert!(
            body.contains("uninstall") || body.contains("hostmail uninstall"),
            "C33 leftover M6 names the verb: {body}"
        );
        let member_m5 = mail_manage_authorized("/cli/mail/domain/list", false, None);
        assert!(member_m5.is_err());
        let pw = mail_manage_authorized("/cli/mail/address/password", false, Some(&p));
        assert!(pw.is_err(), "M5 off password rotate is owner_only");
        assert!(
            mail_manage_authorized("/cli/mail/address/password", true, None).is_ok(),
            "owner/admin rotates IMAP/SMTP password"
        );
        assert!(
            !is_owner_level_mutation("/cli/mail/address/password"),
            "password rotate is mail_manage, not leftover M6"
        );
        assert!(!is_mail_manage_surface("/cli/mail/address/create"));
        let caf = mail_manage_authorized("/cli/mail/catchall", false, Some(&p));
        assert!(caf.is_err(), "M5 off catchall is owner_only");
        assert!(
            mail_manage_authorized("/cli/mail/alias", true, None).is_ok(),
            "owner/admin alias"
        );
        assert!(!is_owner_level_mutation("/cli/mail/forward"));
        assert!(
            !is_owner_level_mutation("/cli/mail/ooo")
                && !is_owner_level_mutation("/cli/mail/footer"),
            "ooo/footer are mail_manage, not leftover M6"
        );
        let ooo_off = mail_manage_authorized("/cli/mail/ooo", false, Some(&p));
        assert!(ooo_off.is_err(), "M5 off ooo is owner_only");
        assert!(
            mail_manage_authorized("/cli/mail/ooo", true, None).is_ok(),
            "owner/admin sets OOO"
        );
        let footer_off = mail_manage_authorized("/cli/mail/footer", false, Some(&p));
        assert!(footer_off.is_err(), "M5 off footer is owner_only");
        let ap = mail_manage_authorized("/cli/mail/app-password", false, Some(&p));
        assert!(ap.is_err(), "M5 off app-password is owner_only");
        assert!(
            mail_manage_authorized("/cli/mail/app-password", true, None).is_ok(),
            "owner/admin lists/adds app passwords"
        );
        assert!(
            mail_manage_authorized("/cli/mail/app-password/revoke", true, None).is_ok(),
            "owner/admin revokes app passwords"
        );
        assert!(
            !is_owner_level_mutation("/cli/mail/app-password"),
            "app-password is mail_manage, not leftover M6"
        );
        assert!(!is_owner_level_mutation("/cli/mail/app-password/revoke"));
        let dkim_off = mail_manage_authorized("/cli/mail/dkim", false, Some(&p));
        assert!(dkim_off.is_err(), "M5 off dkim is owner_only");
        assert!(
            mail_manage_authorized("/cli/mail/dkim", true, None).is_ok(),
            "owner/admin dkim"
        );
        assert!(
            mail_manage_authorized("/cli/mail/dmarc/report-to", true, None).is_ok()
        );
        assert!(!is_owner_level_mutation("/cli/mail/dkim"));
        assert!(!is_owner_level_mutation("/cli/mail/dmarc"));
        let ptr_off = mail_manage_authorized("/cli/mail/ptr", false, Some(&p));
        assert!(ptr_off.is_err(), "M5 off ptr is owner_only");
        assert!(
            mail_manage_authorized("/cli/mail/ptr", true, None).is_ok(),
            "owner/admin ptr show"
        );
        assert!(
            mail_manage_authorized("/cli/mail/ptr/set", true, None).is_ok(),
            "owner/admin ptr set"
        );
        assert!(!is_owner_level_mutation("/cli/mail/ptr"));
        assert!(!is_owner_level_mutation("/cli/mail/ptr/set"));
    }

    /// GET on every POST-only mutation answers an explicit 405 through
    /// the read dispatch chain (feedback_post_only_route_guards), the
    /// POST dispatcher 404s unknown paths, and non-mail paths are not
    /// claimed.
    #[test]
    fn mail_mutations_405_on_get_and_post_404_unknown() {
        let params = HashMap::new();
        for route in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/server/uninstall",
            "/cli/mail/config/set",
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
            "/cli/mail/address/create",
            "/cli/mail/address/delete",
            "/cli/mail/address/password",
            "/cli/mail/send",
            "/cli/mail/reply",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
            "/cli/mail/external/add",
            "/cli/mail/external/remove",
            "/cli/mail/link/oauth/start",
            "/cli/mail/oauth-config/set",
            "/cli/mail/oauth-config/clear",
            "/cli/mail/access/grant",
            "/cli/mail/access/revoke",
            "/cli/mail/access/set-primary",
            "/cli/mail/access/set-level",
            "/cli/mail/access/set-manage",
            "/cli/mail/move",
            "/cli/mail/flag",
            "/cli/mail/archive",
            "/cli/mail/delete",
            "/cli/mail/folder/create",
            "/cli/mail/folder/rename",
            "/cli/mail/draft",
            "/cli/mail/import",
            "/cli/mail/cert/renew",
            "/cli/mail/server/rotate-admin",
            "/cli/mail/alias/remove",
            "/cli/mail/forward/unset",
            "/cli/mail/ooo/unset",
            "/cli/mail/footer/unset",
            "/cli/mail/list/delete",
            "/cli/mail/spam/train",
            "/cli/mail/spam/allow",
            "/cli/mail/spam/block",
            "/cli/mail/spam/quarantine/release",
            "/cli/mail/spam/quarantine/discard",
            "/cli/mail/queue/retry",
            "/cli/mail/queue/drop",
            "/cli/mail/acl/revoke",
            "/cli/mail/ptr/set",
            "/cli/mail/bans/clear",
            "/cli/mail/allowlist/add",
            "/cli/mail/allowlist/remove",
            "/cli/mail/bans/migrate/restore",
        ] {
            let resp = dispatch(route, &params).expect("route claimed by GET chain");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
            assert!(resp.body.contains("POST required"), "body={}", resp.body);
        }
        // GET /cli/mail/ptr is show (not 405). Do not call the live
        // handler here — it dials what-is-my-ip; coverage lives in
        // mail::ptr unit tests with FakeEnv.
        assert!(
            is_mail_manage_surface("/cli/mail/ptr")
                && is_mail_manage_surface("/cli/mail/ptr/set"),
            "ptr paths are M5 exact"
        );
        let ptr_set = dispatch_post("/cli/mail/ptr/set", b"{}");
        assert_ne!(
            ptr_set.status, "404 Not Found",
            "POST /cli/mail/ptr/set must be wired"
        );
        assert_eq!(
            ptr_set.status, "400 Bad Request",
            "missing hostname is usage (before network): {}",
            ptr_set.body
        );
        // Bans/allowlist/migrate exact M5 + POST wired (no live Stalwart).
        for p in [
            "/cli/mail/bans",
            "/cli/mail/bans/clear",
            "/cli/mail/allowlist",
            "/cli/mail/allowlist/add",
            "/cli/mail/allowlist/remove",
            "/cli/mail/bans/migrate",
            "/cli/mail/bans/migrate/restore",
        ] {
            assert!(is_mail_manage_surface(p), "bans M5: {p}");
        }
        let bans_clear = dispatch_post("/cli/mail/bans/clear", b"{}");
        assert_ne!(
            bans_clear.status, "404 Not Found",
            "POST /cli/mail/bans/clear must be wired"
        );
        assert_eq!(
            bans_clear.status, "400 Bad Request",
            "missing ip is usage (before network): {}",
            bans_clear.body
        );
        let migrate_restore_get =
            dispatch("/cli/mail/bans/migrate/restore", &params).expect("claimed");
        assert_eq!(
            migrate_restore_get.status, "405 Method Not Allowed",
            "GET restore must 405"
        );
        let quota_get = dispatch("/cli/mail/quota", &params).expect("quota GET claimed");
        assert_ne!(
            quota_get.status, "405 Method Not Allowed",
            "GET /cli/mail/quota is the engine read, not POST-only: {}",
            quota_get.body
        );
        assert_eq!(quota_get.status, "400 Bad Request", "{}", quota_get.body);
        let quota_post = dispatch_post("/cli/mail/quota", b"{}");
        assert_ne!(
            quota_post.status, "404 Not Found",
            "quota POST must be wired"
        );
        assert_eq!(quota_post.status, "400 Bad Request", "{}", quota_post.body);
        let catchall_get = dispatch("/cli/mail/catchall", &params).expect("catchall GET claimed");
        assert_ne!(
            catchall_get.status, "405 Method Not Allowed",
            "GET /cli/mail/catchall is show, not POST-only: {}",
            catchall_get.body
        );
        assert_eq!(
            catchall_get.status, "400 Bad Request",
            "{}",
            catchall_get.body
        );
        let alias_get = dispatch("/cli/mail/alias", &params).expect("alias GET claimed");
        assert_ne!(
            alias_get.status, "405 Method Not Allowed",
            "GET /cli/mail/alias is list: {}",
            alias_get.body
        );
        let forward_get = dispatch("/cli/mail/forward", &params).expect("forward GET claimed");
        assert_ne!(
            forward_get.status, "405 Method Not Allowed",
            "GET /cli/mail/forward is show: {}",
            forward_get.body
        );
        for post_path in [
            "/cli/mail/catchall",
            "/cli/mail/alias",
            "/cli/mail/alias/remove",
            "/cli/mail/forward",
            "/cli/mail/forward/unset",
        ] {
            let r = dispatch_post(post_path, b"{}");
            assert_ne!(r.status, "404 Not Found", "{post_path} POST must be wired");
            assert_ne!(
                r.status, "405 Method Not Allowed",
                "{post_path} POST is allowed"
            );
        }
        assert!(!is_owner_level_mutation("/cli/mail/catchall"));
        assert!(!is_owner_level_mutation("/cli/mail/alias"));
        assert!(!is_owner_level_mutation("/cli/mail/forward"));
        assert!(!is_owner_level_mutation("/cli/mail/alias/remove"));
        assert!(!is_owner_level_mutation("/cli/mail/forward/unset"));
        let ooo_get = dispatch("/cli/mail/ooo", &params).expect("ooo GET claimed");
        assert_ne!(
            ooo_get.status, "405 Method Not Allowed",
            "GET /cli/mail/ooo is show, not POST-only: {}",
            ooo_get.body
        );
        assert_eq!(ooo_get.status, "400 Bad Request", "{}", ooo_get.body);
        let ooo_post = dispatch_post("/cli/mail/ooo", b"{}");
        assert_ne!(ooo_post.status, "404 Not Found", "ooo POST must be wired");
        assert_eq!(ooo_post.status, "400 Bad Request", "{}", ooo_post.body);
        let footer_get = dispatch("/cli/mail/footer", &params).expect("footer GET claimed");
        assert_ne!(
            footer_get.status, "405 Method Not Allowed",
            "GET /cli/mail/footer is show: {}",
            footer_get.body
        );
        let footer_post = dispatch_post("/cli/mail/footer", b"{}");
        assert_ne!(
            footer_post.status, "404 Not Found",
            "footer POST must be wired"
        );
        assert_eq!(
            footer_post.status, "400 Bad Request",
            "{}",
            footer_post.body
        );
        for route in [
            "/cli/mail/list",
            "/cli/mail/list/members",
            "/cli/mail/spam/quarantine",
            "/cli/mail/queue",
            "/cli/mail/acl",
            "/cli/mail/autoconfig",
        ] {
            let get = dispatch(route, &params).expect("N16 show/list GET claimed");
            assert_ne!(
                get.status, "405 Method Not Allowed",
                "GET {route} is dual show/list, not POST-only: {}",
                get.body
            );
            assert_ne!(get.status, "404 Not Found", "GET {route} must be wired");
        }
        for (route, body) in [
            ("/cli/mail/list", br#"{}"#.as_slice()),
            ("/cli/mail/list/members", br#"{}"#),
            ("/cli/mail/list/delete", br#"{}"#),
            ("/cli/mail/spam/train", br#"{}"#),
            ("/cli/mail/spam/allow", br#"{}"#),
            ("/cli/mail/spam/block", br#"{}"#),
            ("/cli/mail/spam/quarantine/release", br#"{}"#),
            ("/cli/mail/spam/quarantine/discard", br#"{}"#),
            ("/cli/mail/queue/retry", br#"{}"#),
            ("/cli/mail/queue/drop", br#"{}"#),
            ("/cli/mail/acl", br#"{}"#),
            ("/cli/mail/acl/revoke", br#"{}"#),
            ("/cli/mail/autoconfig", br#"{}"#),
        ] {
            let post = dispatch_post(route, body);
            assert_ne!(
                post.status, "404 Not Found",
                "N16 POST {route} must be wired"
            );
        }
        assert!(
            is_mail_manage_surface("/cli/mail/acl") && !is_owner_level_mutation("/cli/mail/acl"),
            "ACL is mail_manage, not /cli/mail/access/"
        );
        assert!(
            !is_mail_manage_surface("/cli/mail/access/acl")
                && is_owner_level_mutation("/cli/mail/access/grant"),
            "do not hang ACL under /cli/mail/access/"
        );
        let ap_get = dispatch("/cli/mail/app-password", &params).expect("app-password GET claimed");
        assert_ne!(
            ap_get.status, "405 Method Not Allowed",
            "GET /cli/mail/app-password is list, not POST-only: {}",
            ap_get.body
        );
        assert_eq!(ap_get.status, "400 Bad Request", "{}", ap_get.body);
        let ap_post = dispatch_post("/cli/mail/app-password", b"{}");
        assert_ne!(
            ap_post.status, "404 Not Found",
            "app-password POST must be wired"
        );
        assert_eq!(ap_post.status, "400 Bad Request", "{}", ap_post.body);
        let ap_revoke_get =
            dispatch("/cli/mail/app-password/revoke", &params).expect("revoke GET claimed");
        assert_eq!(
            ap_revoke_get.status, "405 Method Not Allowed",
            "GET /cli/mail/app-password/revoke is POST-only: {}",
            ap_revoke_get.body
        );
        let ap_revoke_post = dispatch_post("/cli/mail/app-password/revoke", b"{}");
        assert_ne!(
            ap_revoke_post.status, "404 Not Found",
            "app-password/revoke POST must be wired"
        );
        assert_eq!(
            ap_revoke_post.status, "400 Bad Request",
            "{}",
            ap_revoke_post.body
        );
        let dkim_get = dispatch("/cli/mail/dkim", &params).expect("dkim GET claimed");
        assert_ne!(
            dkim_get.status, "405 Method Not Allowed",
            "GET /cli/mail/dkim is show, not POST-only: {}",
            dkim_get.body
        );
        assert_eq!(dkim_get.status, "400 Bad Request", "{}", dkim_get.body);
        let dmarc_get = dispatch("/cli/mail/dmarc", &params).expect("dmarc GET claimed");
        assert_ne!(dmarc_get.status, "405 Method Not Allowed", "{}", dmarc_get.body);
        assert_eq!(dmarc_get.status, "400 Bad Request", "{}", dmarc_get.body);
        for mutator in [
            "/cli/mail/dkim/rotate",
            "/cli/mail/dkim/retire",
            "/cli/mail/dmarc/report-to",
        ] {
            let resp = dispatch(mutator, &params).expect("claimed");
            assert_eq!(resp.status, "405 Method Not Allowed", "GET {mutator}");
        }
        let dkim_post = dispatch_post("/cli/mail/dkim", b"{}");
        assert_ne!(dkim_post.status, "404 Not Found", "dkim POST must be wired");
        assert_eq!(dkim_post.status, "400 Bad Request", "{}", dkim_post.body);
        let dkim_rotate = dispatch_post("/cli/mail/dkim/rotate", b"{}");
        assert_ne!(dkim_rotate.status, "404 Not Found");
        let dkim_retire = dispatch_post("/cli/mail/dkim/retire", b"{}");
        assert_ne!(dkim_retire.status, "404 Not Found");
        let dmarc_post = dispatch_post("/cli/mail/dmarc", b"{}");
        assert_ne!(dmarc_post.status, "404 Not Found");
        assert_eq!(
            dispatch_post("/cli/mail/unknown", b"{}").status,
            "404 Not Found"
        );
        let renew = dispatch_post("/cli/mail/cert/renew", b"{}");
        assert_ne!(renew.status, "404 Not Found", "cert/renew must be wired");
        assert_ne!(renew.status, "501 Not Implemented", "{}", renew.body);
        assert!(
            !renew.body.contains("alreadyEnabled"),
            "renew is not enable: {}",
            renew.body
        );
        let rotate = dispatch_post("/cli/mail/server/rotate-admin", b"{}");
        assert_ne!(rotate.status, "404 Not Found", "rotate-admin must be wired");
        assert_ne!(rotate.status, "501 Not Implemented", "{}", rotate.body);
        assert!(
            !rotate.body.contains("alreadyEnabled"),
            "rotate-admin is not enable no-op: {}",
            rotate.body
        );
        assert_eq!(
            dispatch("/cli/mail/unknown", &params)
                .expect("prefix claimed")
                .status,
            "404 Not Found"
        );
        assert!(
            dispatch("/cli/feedback/list", &params).is_none(),
            "not our prefix"
        );
        assert!(
            dispatch("/cli/mailbox", &params).is_none(),
            "no bare-prefix match"
        );
    }

    /// Every route in the partition map answers its REAL contract —
    /// as of S6 nothing 501s any more.
    #[test]
    fn reserved_routes_501_and_real_routes_answer() {
        let params = HashMap::new();
        // S6 — config + doctor are REAL: the GETs answer 200 (config =
        // effective configuration; doctor = latest persisted run or
        // null), config/set validates its body (`{}` = usage 400), and
        // the doctor POST validates + stops at the platform gate on a
        // Mac (never a probe from the example page). Deep behavior is
        // owned by routes_server/mail::config/mail::doctor tests.
        for route in ["/cli/mail/config", "/cli/mail/doctor"] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "200 OK", "route={route}: {}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["ok"], true, "route={route}");
        }
        let resp = dispatch_post("/cli/mail/config/set", b"{}");
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
        let resp = dispatch_post("/cli/mail/doctor", b"not json");
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        if !cfg!(target_os = "linux") {
            let resp = dispatch_post("/cli/mail/doctor", b"{}");
            assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
            assert!(resp.body.contains("unsupported"), "{}", resp.body);
        }
        let resp = dispatch("/cli/mail/status", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK", "status is REAL from day one");

        // S2 — the domain family is REAL: reads answer through the
        // shim (list 200; show without a domain = usage 400), and the
        // mutations validate their bodies (`{}` = usage 400) instead
        // of 501-ing. Deep behavior is owned by routes_domains tests.
        let resp = dispatch("/cli/mail/domain/list", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        let resp = dispatch("/cli/mail/domain/show", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        for route in [
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "400 Bad Request", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S3 — the address family is REAL: the list read validates its
        // params through the shim (missing project = usage 400; the
        // owner table answers 200), and the mutations validate their
        // bodies (`{}` = usage 400) instead of 501-ing. Deep behavior
        // is owned by routes_addresses/mail::addresses tests.
        let resp = dispatch("/cli/mail/address/list", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
        let all_params: HashMap<String, String> =
            HashMap::from([("all".to_string(), "true".to_string())]);
        let resp = dispatch("/cli/mail/address/list", &all_params).expect("claimed");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        for route in [
            "/cli/mail/address/create",
            "/cli/mail/address/delete",
            "/cli/mail/address/password",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "400 Bad Request", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S1 — preflight is REAL.
        let resp = dispatch("/cli/mail/preflight", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK", "preflight is REAL as of S1");

        // S5 — the send family is REAL: reads validate their params
        // through the shim (outbox without a project = usage 400; the
        // owner approvals queue answers 200 — its owner gate lives in
        // the dispatcher clause), and the mutations validate their
        // bodies (`{}` = usage 400) instead of 501-ing. Deep behavior
        // is owned by routes_send/mail::send tests.
        let resp = dispatch("/cli/mail/outbox", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
        let resp = dispatch("/cli/mail/approvals/list", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        for route in [
            "/cli/mail/send",
            "/cli/mail/reply",
            "/cli/mail/outbox/cancel",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(
                resp.status, "400 Bad Request",
                "route={route}: {}",
                resp.body
            );
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S4 — the read family is REAL: every route validates its
        // params through the shim (missing project/id = usage 400)
        // instead of 501-ing. Deep behavior is owned by
        // routes_messages/mail::messages tests.
        for route in [
            "/cli/mail/messages",
            "/cli/mail/read",
            "/cli/mail/attachments",
            "/cli/mail/wait",
        ] {
            let resp = dispatch(route, &params).expect("claimed");
            assert_eq!(resp.status, "400 Bad Request", "route={route}");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }

        // S9/S11 — the external + access families are REAL: the unified
        // inbox catalog answers 200 through the shim (its owner gate for
        // the no-project view lives in the dispatcher clause), and the
        // mutations + draft validate their bodies (`{}` = usage 400)
        // instead of 501-ing. Deep behavior is owned by
        // routes_external/routes_access/mail::access tests.
        let resp = dispatch("/cli/mail/inboxes", &params).expect("claimed");
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        for route in [
            "/cli/mail/external/add",
            "/cli/mail/external/remove",
            "/cli/mail/link/oauth/start",
            "/cli/mail/oauth-config/set",
            "/cli/mail/oauth-config/clear",
            "/cli/mail/access/grant",
            "/cli/mail/access/revoke",
            "/cli/mail/access/set-primary",
            "/cli/mail/access/set-level",
            "/cli/mail/access/set-manage",
            "/cli/mail/move",
            "/cli/mail/flag",
            "/cli/mail/archive",
            "/cli/mail/delete",
            "/cli/mail/folder/create",
            "/cli/mail/folder/rename",
            "/cli/mail/draft",
        ] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(
                resp.status, "400 Bad Request",
                "route={route}: {}",
                resp.body
            );
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "route={route}");
        }
        // 0081: folder/list is a GET; missing project = usage 400.
        let resp = dispatch("/cli/mail/folder/list", &params).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
    }

    /// The S1-real server mutations answer through their handlers (no
    /// 404/501): enable validates its body, disable/uninstall answer
    /// the structured not-installed conflict on an empty table.
    /// (Deeper behavior is tested in mail::routes_server.)
    #[test]
    fn s1_server_mutations_are_wired_to_real_handlers() {
        let _g = crate::mail::mail_server_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
        }
        let resp = dispatch_post("/cli/mail/server/enable", b"{}");
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        for route in ["/cli/mail/server/disable", "/cli/mail/server/uninstall"] {
            let resp = dispatch_post(route, b"{}");
            assert_eq!(resp.status, "409 Conflict", "route={route}: {}", resp.body);
            assert!(resp.body.contains("not_installed"), "{}", resp.body);
        }
        let rotate = dispatch_post("/cli/mail/server/rotate-admin", b"{}");
        assert_eq!(rotate.status, "409 Conflict", "{}", rotate.body);
        assert!(
            rotate.body.contains("not_ready")
                || rotate.body.contains("unsupported")
                || rotate.body.contains("not installed"),
            "{}",
            rotate.body
        );
        assert!(
            !rotate.body.contains("alreadyEnabled"),
            "rotate-admin is not enable: {}",
            rotate.body
        );
    }

    /// The owner-or-admin classification the dispatcher arm relies on.
    #[test]
    fn owner_level_mutation_classification() {
        for owner_path in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/server/rotate-admin",
            "/cli/mail/server/uninstall",
            "/cli/mail/config/set",
            "/cli/mail/domain/add",
            "/cli/mail/domain/remove",
            "/cli/mail/domain/check",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
            // S9: connecting an external account = owner surface.
            // S11: the access-management surface = owner surface.
            "/cli/mail/external/add",
            "/cli/mail/external/remove",
            // O4: beginning an OAuth link is owner surface.
            "/cli/mail/link/oauth/start",
            "/cli/mail/oauth-config/set",
            "/cli/mail/oauth-config/clear",
            "/cli/mail/access/grant",
            "/cli/mail/access/revoke",
            "/cli/mail/access/set-primary",
            "/cli/mail/access/set-level",
            // 0081: setting the manage/delete caps is owner surface.
            "/cli/mail/access/set-manage",
            // The doctor RUN (POST; the GET read never reaches the
            // classifier).
            "/cli/mail/doctor",
        ] {
            assert!(is_owner_level_mutation(owner_path), "{owner_path}");
        }
        for agent_path in [
            "/cli/mail/address/create",
            "/cli/mail/address/delete",
            "/cli/mail/address/password",
            "/cli/mail/send",
            "/cli/mail/reply",
            // S9: drafting into the bound external inbox is the AGENT
            // verb — workspace token, ownership-masked handler-side.
            "/cli/mail/draft",
            // 0081: the management/delete action verbs are AGENT verbs
            // (workspace token; can_manage/can_delete gated in-handler).
            "/cli/mail/move",
            "/cli/mail/flag",
            "/cli/mail/archive",
            "/cli/mail/delete",
            "/cli/mail/folder/create",
            "/cli/mail/folder/rename",
        ] {
            assert!(!is_owner_level_mutation(agent_path), "{agent_path}");
        }
    }
}
