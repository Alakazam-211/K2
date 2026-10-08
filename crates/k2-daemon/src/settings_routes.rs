//! Daemon-side handlers for `/cli/settings/{get,update,reset}` —
//! Phase 2 Unit 7a.
//!
//! Owns the read/write surface for `~/.k2so/settings.json` so the
//! Tauri thin client (and any K2SO Connect / Mobile Companion future
//! client) talks to a single writer instead of racing with an
//! in-process Tauri copy. The actual JSON shape + tmp+rename writer
//! lives in `k2_core::app_settings`; these handlers translate
//! between HTTP and the typed `AppSettings`.
//!
//! # F3 close
//!
//! Pre-Unit-7a, `commands/settings.rs::settings_update` invalidated
//! companion sessions in Tauri's in-process `STATE` — which is empty
//! after Unit 1 moved the companion runtime to the daemon. The call
//! was a no-op and rotated credentials left live tokens valid until
//! their TTL expired.
//!
//! `app_settings::update()` now runs the comparison + invalidation
//! itself, in the same process as the live companion `STATE`. The
//! handler below just hands `partial` off to that single critical
//! section.

use crate::cli_response::CliResponse;

/// Settings keys that grant REMOTE ACCESS to this daemon — the federation
/// master switch, the remote-instruct delivery consent, and (0.40.43 1c)
/// the public `/v1` API switch. Writes touching any of these require the
/// owner-or-admin tier (federation-toggle audit finding #4): with only
/// `token_ok` on `/cli/settings/update`, a Member connect-user could POST
/// `{"federationEnabled":true}` and open the cross-server surface the
/// renderer only HID from them — and `{"apiEnabled":true}` would open the
/// whole external `/v1` surface the same way. Keys are the camelCase wire
/// names (`AppSettings` is `rename_all = "camelCase"` with no aliases, so
/// no other spelling reaches the struct fields).
const REMOTE_ACCESS_KEYS: &[&str] = &[
    "federationEnabled",
    "allowRemoteInstruct",
    "apiEnabled",
    "dnsManageEnabled",
    // C1 (0.40.45) — agents-may-create-connections app master.
    "agentsCanCreateConnections",
    // Remote Session Layer 0 — master switch for remote shell sessions.
    "remoteSessionsEnabled",
    // Hosted web client Layer 0 — master switch for browser data-plane
    // access (PRD §6.7). Owner/admin only: a Member must not flip the
    // browser door on/off for the whole daemon.
    "webClientEnabled",
    // PRD connect-login-edge-only S1 — who may password-login over the
    // public tunnel (edge|any|off). Owner/admin only: a Member must not
    // reopen the public login surface.
    "connectLoginIngress",
    // 0.44.4 (noun-tiers A3): the global agent mail-send default is an
    // agent permission switch like DNS/connections above — Admin floor.
    "mailAgentSend",
    // prd-daemon-activity-and-thread-working-v1 Q2 — keep K2's hooks in
    // the daemon user's agent CLI configs. Same floor as
    // `POST /cli/hooks/install` (Admin): it rewrites those files.
    "agentHooks",
];

/// LM5 (prd-lan-mode-toggle-tls-docs-v1 §11) — keys only the Owner may
/// write: what the daemon listens on and advertises (`listenLan`, and the
/// LAN-mode `lanScope` / `lanAdvertise` names, gated before they exist),
/// air-gap, the push gateway and the companion block. An Admin or Member
/// login touching any of them gets an atomic 403 with nothing written.
/// Agent passports never reach `/cli/settings/*` (`token_ok` refuses them).
const OWNER_ONLY_KEYS: &[&str] = &[
    "listenLan",
    "lanScope",
    "lanAdvertise",
    "airgap",
    "pushGatewayUrl",
    "pushGatewayToken",
    "companion",
];

/// Who is writing settings, resolved by the dispatcher from the request
/// credential: the owner token or an Owner login, an Admin login, or any
/// other accepted login (Member).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsActor {
    Member,
    Admin,
    Owner,
}

impl SettingsActor {
    /// Resolve from the effective auth query — the same resolvers the
    /// route gates use (`owner_role_identity`, `token_is_owner_or_admin`).
    pub fn from_query(query: &str, owner_token: &str) -> Self {
        if crate::routes::http::owner_role_identity(query, owner_token).is_some() {
            SettingsActor::Owner
        } else if crate::routes::http::token_is_owner_or_admin(query, owner_token) {
            SettingsActor::Admin
        } else {
            SettingsActor::Member
        }
    }

    fn can_manage(self) -> bool {
        !matches!(self, SettingsActor::Member)
    }
}

fn forbidden(msg: String) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": msg }).to_string(),
    }
}

/// Handler for `GET /cli/settings/get`.
///
/// Token check happens in `main.rs` before this is called. Returns
/// the full `AppSettings` JSON serialized via serde so the renderer
/// (or `k2so` CLI) sees the same shape Tauri's `settings_get` used to
/// produce.
pub fn handle_settings_get() -> CliResponse {
    let settings = k2_core::app_settings::load();
    match serde_json::to_string(&settings) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => CliResponse::bad_request(format!("serialize settings: {e}")),
    }
}

/// Handler for `POST /cli/settings/update`.
///
/// Body: a JSON object of partial settings to deep-merge into the
/// current state. Accepts the camelCase shape the renderer already
/// sends to Tauri's `settings_update` so a single payload format
/// covers the proxy path AND any future direct-daemon callers.
///
/// On success returns the full post-merge `AppSettings`. F3-close
/// runs inside `app_settings::update()` — when companion-affecting
/// fields differ from disk, every live companion session is
/// invalidated before this handler returns.
///
/// `actor` is the dispatcher-resolved [`SettingsActor`]. When the partial
/// touches ANY [`OWNER_ONLY_KEYS`] entry and the actor is not the Owner,
/// or ANY [`REMOTE_ACCESS_KEYS`] entry and the actor is a Member, the
/// WHOLE request is rejected 403 with nothing written — atomic, so a
/// mixed gated+ungated payload can never half-apply and leave the caller
/// guessing which keys landed. Other settings keys keep the pre-existing
/// any-authenticated-user behavior.
pub fn handle_settings_update(body: &[u8], actor: SettingsActor) -> CliResponse {
    let partial: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if !partial.is_object() {
        return CliResponse::bad_request(
            "expected JSON object at top level (got non-object)".to_string(),
        );
    }
    // Owner decree: "Only Admin and Owner can enable federation." Gate on
    // key PRESENCE (not value) — a Member force-disabling the owner's
    // federation is the same class of unauthorized remote-access write.
    if actor != SettingsActor::Owner {
        if let Some(gated) = OWNER_ONLY_KEYS.iter().find(|k| partial.get(**k).is_some()) {
            return forbidden(format!("updating \"{gated}\" requires the Owner role"));
        }
    }
    if !actor.can_manage() {
        if let Some(gated) = REMOTE_ACCESS_KEYS.iter().find(|k| partial.get(**k).is_some()) {
            return forbidden(format!(
                "updating \"{gated}\" requires the Owner or Admin role"
            ));
        }
    }
    // TODO grant announce — if partial flips dnsManageEnabled false→true,
    // best-effort append a one-line DNS-manage capability note (daemon
    // agent follow-up). Same hook as per-workspace `/cli/dns-manage`.
    let agent_hooks_touched = partial.get("agentHooks").is_some();
    match k2_core::app_settings::update(partial) {
        Ok(merged) => {
            // Sync the federation master switch into the running process so the
            // /cli/federation/* gate flips immediately (no restart) when the K2
            // Connect toggle changes it. Env-var force-on still wins in enabled().
            k2_core::federation::set_enabled(merged.federation_enabled);
            // 0.40.43 (1c): same deal for the public /v1 API switch — sync the
            // runtime mirror so misc_routes::api_enabled() (checked PER REQUEST
            // by the dispatcher's /v1 arm) flips the surface live, with no
            // restart and no confirm+reboot dialog. K2_API force-on still wins.
            k2_core::app_settings::set_api_enabled(merged.api_enabled);
            k2_core::airgap::set_setting_enabled(merged.airgap);
            k2_core::listen::set_setting_lan(merged.listen_lan);
            // Q2: an `agentHooks` flip installs or removes K2's hook entries
            // now, not at the next 10-minute self-heal pass.
            if agent_hooks_touched {
                crate::hook_install::request_run(crate::hook_install::Trigger::Setting);
            }
            // Local-window live-sync (v1): renderer listens on `sync:settings`.
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncSettings,
                serde_json::Value::Null,
            );
            match serde_json::to_string(&merged) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(format!("serialize merged: {e}")),
            }
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/settings/reset`.
///
/// Restores `AppSettings::default()`, clears the macOS-Keychain
/// companion password hash, and invalidates every live companion
/// session — matching the pre-Unit-7a Tauri `settings_reset` body
/// exactly. POST (not GET) because the call is destructive and
/// shouldn't be reachable via a browser-cached idempotent fetch.
///
/// LM5: Owner only (route floor Owner in `route_policy.rs`; this check is
/// the defence in depth). A reset rewrites every gated key at once —
/// re-opening the browser door, the tunnel login ingress, LAN and
/// air-gap — so a non-Owner gets 403 with nothing changed.
pub fn handle_settings_reset(actor: SettingsActor) -> CliResponse {
    if actor != SettingsActor::Owner {
        return forbidden("resetting settings requires the Owner role".to_string());
    }
    match k2_core::app_settings::reset() {
        Ok(defaults) => {
            // Reset returns federation to its default (OFF) — sync it.
            k2_core::federation::set_enabled(defaults.federation_enabled);
            // …and the /v1 API switch back to its default (OFF) too (1c).
            k2_core::app_settings::set_api_enabled(defaults.api_enabled);
            k2_core::airgap::set_setting_enabled(defaults.airgap);
            k2_core::listen::set_setting_lan(defaults.listen_lan);
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncSettings,
                serde_json::Value::Null,
            );
            match serde_json::to_string(&defaults) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(format!("serialize defaults: {e}")),
            }
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;
    use k2_core::connect_users;

    /// Resolve the owner-or-admin bit exactly as the dispatcher does for
    /// `/cli/settings/update`, so these tests exercise the REAL gate chain
    /// (token → role → can-manage) rather than a hand-picked bool.
    fn can_manage(query: &str, owner_token: &str) -> bool {
        crate::routes::http::token_is_owner_or_admin(query, owner_token)
    }

    /// Admin or Member from the manage bit (callers that never mean Owner).
    fn tier(can_manage: bool) -> SettingsActor {
        if can_manage {
            SettingsActor::Admin
        } else {
            SettingsActor::Member
        }
    }

    #[test]
    fn owner_token_may_write_federation_enabled() {
        with_temp_home(|| {
            let cm = can_manage("token=ownertok", "ownertok");
            assert!(cm, "owner token must resolve to the manage tier");
            let r = handle_settings_update(br#"{"federationEnabled":true}"#, tier(cm));
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert!(
                k2_core::app_settings::load().federation_enabled,
                "owner write must persist"
            );
        });
    }

    #[test]
    fn admin_connect_user_may_write_federation_enabled() {
        with_temp_home(|| {
            connect_users::add_user("adminu", "password1").expect("add");
            connect_users::set_role("adminu", connect_users::Role::Admin).expect("promote");
            let tok = connect_users::create_session("adminu");
            let cm = can_manage(&format!("token={tok}"), "ownertok");
            assert!(cm, "Admin session must resolve to the manage tier");
            let r = handle_settings_update(br#"{"federationEnabled":true}"#, tier(cm));
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert!(k2_core::app_settings::load().federation_enabled);
        });
    }

    /// 0.40.43 (1c): an owner write of `apiEnabled` must persist AND sync
    /// the runtime mirror in the same request — the mirror is what
    /// `misc_routes::api_enabled()` reads per request, so this IS the
    /// no-restart property (surface flips live, no confirm+reboot dialog).
    /// Runs entirely with the env flags untouched; ends with the switch
    /// OFF so the process-wide mirror never leaks into other tests.
    #[test]
    fn owner_token_may_write_api_enabled_and_mirror_syncs_live() {
        with_temp_home(|| {
            let cm = can_manage("token=ownertok", "ownertok");
            assert!(cm, "owner token must resolve to the manage tier");

            let r = handle_settings_update(br#"{"apiEnabled":true}"#, tier(cm));
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert!(
                k2_core::app_settings::load().api_enabled,
                "owner write must persist"
            );
            assert!(
                k2_core::app_settings::api_enabled_setting(),
                "update must sync the runtime mirror in-request (the no-restart path)"
            );

            // …and back OFF (also restores process state for other tests).
            let r = handle_settings_update(br#"{"apiEnabled":false}"#, tier(cm));
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert!(!k2_core::app_settings::load().api_enabled);
            assert!(
                !k2_core::app_settings::api_enabled_setting(),
                "disable must sync the mirror OFF just as immediately"
            );
        });
    }

    /// 1c: reset() returns `apiEnabled` to its OFF default and syncs the
    /// mirror down with it — a reset must never leave the /v1 surface open.
    #[test]
    fn reset_returns_api_enabled_to_off_and_syncs_mirror() {
        with_temp_home(|| {
            let r = handle_settings_update(br#"{"apiEnabled":true}"#, SettingsActor::Owner);
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert!(k2_core::app_settings::api_enabled_setting());

            let r = handle_settings_reset(SettingsActor::Owner);
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert!(!k2_core::app_settings::load().api_enabled);
            assert!(
                !k2_core::app_settings::api_enabled_setting(),
                "reset must sync the mirror OFF"
            );
        });
    }

    #[test]
    fn member_connect_user_gets_403_and_value_unchanged() {
        with_temp_home(|| {
            connect_users::add_user("memberu", "password1").expect("add");
            let tok = connect_users::create_session("memberu");
            let query = format!("token={tok}");
            // The hole this closes: token_ok ADMITS the Member session…
            assert!(crate::routes::http::token_ok(&query, "ownertok"));
            // …but the manage tier must not.
            let cm = can_manage(&query, "ownertok");
            assert!(!cm, "Member session must NOT resolve to the manage tier");
            for body in [
                // Gate is on key PRESENCE — enabling, disabling, and the
                // consent / DNS-manage keys are all barred for a Member.
                br#"{"federationEnabled":true}"#.as_slice(),
                br#"{"federationEnabled":false}"#.as_slice(),
                br#"{"allowRemoteInstruct":true}"#.as_slice(),
                br#"{"apiEnabled":true}"#.as_slice(),
                br#"{"apiEnabled":false}"#.as_slice(),
                br#"{"dnsManageEnabled":true}"#.as_slice(),
                br#"{"agentsCanCreateConnections":true}"#.as_slice(),
                br#"{"remoteSessionsEnabled":true}"#.as_slice(),
                br#"{"webClientEnabled":false}"#.as_slice(),
            ] {
                let r = handle_settings_update(body, tier(cm));
                assert_eq!(r.status, "403 Forbidden", "got: {}", r.body);
                assert!(
                    r.body.contains("Owner or Admin"),
                    "error must name the required role, got: {}",
                    r.body
                );
            }
            let s = k2_core::app_settings::load();
            assert!(!s.federation_enabled, "403 must leave the value unchanged");
            assert!(!s.allow_remote_instruct, "403 must leave the value unchanged");
            assert!(!s.api_enabled, "403 must leave the value unchanged");
            assert!(
                !k2_core::app_settings::api_enabled_setting(),
                "403 must never sync the /v1 mirror"
            );
            assert!(!s.dns_manage_enabled, "403 must leave the value unchanged");
            assert!(
                !s.agents_can_create_connections,
                "403 must leave the value unchanged"
            );
            assert!(!s.remote_sessions_enabled, "403 must leave the value unchanged");
            // Product default ON — Member force-off must not land.
            assert!(
                s.web_client_enabled,
                "403 must leave webClientEnabled at its ON default"
            );
        });
    }

    #[test]
    fn member_may_still_write_ungated_settings() {
        with_temp_home(|| {
            // Member tier (can_manage=false) writing a key OUTSIDE
            // REMOTE_ACCESS_KEYS keeps today's token_ok-only behavior.
            let r = handle_settings_update(br#"{"defaultAgent":"codex"}"#, SettingsActor::Member);
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert_eq!(k2_core::app_settings::load().default_agent, "codex");
        });
    }

    #[test]
    fn mixed_payload_from_member_is_atomic_403() {
        with_temp_home(|| {
            // Gated + ungated keys in ONE payload from a Member: the WHOLE
            // request is rejected — the ungated key must not half-apply.
            let r = handle_settings_update(
                br#"{"federationEnabled":true,"defaultAgent":"codex"}"#,
                SettingsActor::Member,
            );
            assert_eq!(r.status, "403 Forbidden", "got: {}", r.body);
            let s = k2_core::app_settings::load();
            assert!(!s.federation_enabled, "gated key must be unchanged");
            assert_eq!(s.default_agent, "claude", "ungated key must be unchanged too");
        });
    }

    /// Sessions for each login tier, resolved the way the dispatcher does.
    fn session_actor(username: &str, role: connect_users::Role) -> SettingsActor {
        connect_users::add_user(username, "password1").expect("add");
        connect_users::set_role(username, role).expect("role");
        let tok = connect_users::create_session(username);
        SettingsActor::from_query(&format!("token={tok}"), "ownertok")
    }

    #[test]
    fn actor_resolves_owner_admin_member() {
        with_temp_home(|| {
            assert_eq!(
                SettingsActor::from_query("token=ownertok", "ownertok"),
                SettingsActor::Owner
            );
            assert_eq!(
                session_actor("ownerlogin", connect_users::Role::Owner),
                SettingsActor::Owner
            );
            assert_eq!(
                session_actor("adminlogin", connect_users::Role::Admin),
                SettingsActor::Admin
            );
            assert_eq!(
                session_actor("memberlogin", connect_users::Role::Member),
                SettingsActor::Member
            );
        });
    }

    /// LM5: listen / air-gap / push gateway / companion keys are Owner
    /// only. Admin and Member get an atomic 403 and nothing changes.
    #[test]
    fn owner_only_keys_refused_for_admin_and_member() {
        with_temp_home(|| {
            let admin = session_actor("adminlm5", connect_users::Role::Admin);
            let member = session_actor("memberlm5", connect_users::Role::Member);
            assert_eq!(admin, SettingsActor::Admin);
            assert_eq!(member, SettingsActor::Member);
            for actor in [admin, member] {
                for body in [
                    br#"{"listenLan":true}"#.as_slice(),
                    br#"{"listenLan":false}"#.as_slice(),
                    br#"{"lanScope":"any"}"#.as_slice(),
                    br#"{"lanAdvertise":"http://10.0.0.5:1"}"#.as_slice(),
                    br#"{"airgap":true}"#.as_slice(),
                    br#"{"airgap":false}"#.as_slice(),
                    br#"{"pushGatewayUrl":"https://evil.example"}"#.as_slice(),
                    br#"{"pushGatewayToken":"t"}"#.as_slice(),
                    br#"{"companion":{}}"#.as_slice(),
                    br#"{"listenLan":true,"defaultAgent":"codex"}"#.as_slice(),
                ] {
                    let r = handle_settings_update(body, actor);
                    assert_eq!(r.status, "403 Forbidden", "{actor:?}: got {}", r.body);
                    assert!(
                        r.body.contains("requires the Owner role"),
                        "{actor:?}: error must name the Owner role, got {}",
                        r.body
                    );
                }
            }
            let s = k2_core::app_settings::load();
            assert!(!s.listen_lan, "403 must leave listenLan off");
            assert!(!s.airgap, "403 must leave airgap off");
            assert_eq!(s.push_gateway_url, None);
            assert_eq!(s.push_gateway_token, None);
            assert_eq!(s.default_agent, "claude", "mixed payload must not half-apply");
            // An Admin still manages the remote-access keys.
            let r = handle_settings_update(br#"{"federationEnabled":false}"#, admin);
            assert_eq!(r.status, "200 OK", "Admin keeps its tier: {}", r.body);
        });
    }

    #[test]
    fn owner_may_write_owner_only_keys() {
        with_temp_home(|| {
            let r = handle_settings_update(
                br#"{"pushGatewayUrl":"https://push.example"}"#,
                SettingsActor::Owner,
            );
            assert_eq!(r.status, "200 OK", "got: {}", r.body);
            assert_eq!(
                k2_core::app_settings::load().push_gateway_url.as_deref(),
                Some("https://push.example")
            );
        });
    }

    /// LM5: a reset re-opens the browser door and flips the tunnel login
    /// ingress back to default, so only the Owner may do it. Admin and
    /// Member get 403 and every value stays as the Owner left it.
    #[test]
    fn reset_is_owner_only_and_non_owner_changes_nothing() {
        with_temp_home(|| {
            let r = handle_settings_update(
                br#"{"webClientEnabled":false,"connectLoginIngress":"off","defaultAgent":"codex"}"#,
                SettingsActor::Owner,
            );
            assert_eq!(r.status, "200 OK", "owner setup: {}", r.body);
            let admin = session_actor("adminreset", connect_users::Role::Admin);
            let member = session_actor("memberreset", connect_users::Role::Member);
            for actor in [admin, member] {
                let r = handle_settings_reset(actor);
                assert_eq!(r.status, "403 Forbidden", "{actor:?}: got {}", r.body);
                assert!(r.body.contains("Owner role"), "{}", r.body);
                let s = k2_core::app_settings::load();
                assert!(!s.web_client_enabled, "{actor:?} reset must not re-enable the web client");
                assert_eq!(s.connect_login_ingress, "off", "{actor:?} reset must not reopen ingress");
                assert_eq!(s.default_agent, "codex", "{actor:?} reset must change nothing");
            }
            let r = handle_settings_reset(SettingsActor::Owner);
            assert_eq!(r.status, "200 OK", "owner reset: {}", r.body);
            assert!(k2_core::app_settings::load().web_client_enabled, "owner reset restores defaults");
        });
    }
}
