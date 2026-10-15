//! Chat-pass HTTP surface (S28 §8, §10).
//!
//! - `GET /v1/chat/jwks` — public, unauthenticated, served before the
//!   `/v1/` gate and during boot readiness, like `/v1/jwks`
//!   ([`handle_jwks`]). Creates the keyring on first use.
//! - `GET /cli/sew/keys` — the key lifecycle (kids, states, times). Never
//!   creates a keyring; never returns key material.
//! - `POST /cli/sew/keys/rotate` — body `{}`: promote `next` now if it has
//!   been published ≥ 10 min, else 409 `lead_too_short`. Body
//!   `{"emergency":true,"confirmToken":"k2vc_…"}`: destroy every key and
//!   start a new `current` + `next` (§7.4; also the corruption recovery).
//! - `POST /cli/sew/keys/drop-previous` — retire any `retiring` key now
//!   (added for K6; not in §10).
//!
//! Who: Owner or Admin people (owner token, or an Owner/Admin Connect
//! login); agent passports, app passes, API keys and Member logins are
//! refused. Loopback and LAN only (tunnel → 403 `loopback_only`).
//!
//! **Person-proof (KM13):** the emergency wipe is the one destructive
//! step here, and the owner token alone doesn't do it: it needs a vault
//! confirm token for action `chat-keys-emergency` (Touch ID / the Mac
//! login sheet, the vault password, or the Connect password; see
//! `vault/human.rs` and `vault_routes::confirm_*`). Agents can read the
//! owner token on shared-user boxes; they can't produce that confirm.

use serde::Deserialize;
use serde_json::{json, Value};

use k2_vault::Actor;

use super::keyring::{self, KeyError, KeyState, PUBLISH_LEAD_MIN_SECS, RETIRE_AFTER_LAST_USE_SECS, ROTATE_AFTER_SECS};
use super::{JWKS_PATH, PASS_TTL_SECS};
use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;

pub const KEYS: &str = "/cli/sew/keys";
pub const ROTATE: &str = "/cli/sew/keys/rotate";
pub const DROP_PREVIOUS: &str = "/cli/sew/keys/drop-previous";

/// The vault confirm action that stands in for a person on the emergency
/// wipe.
pub const EMERGENCY_CONFIRM_ACTION: &str = "chat-keys-emergency";

pub const POST_ROUTES: &[&str] = &[ROTATE, DROP_PREVIOUS];

/// Bodies here are tiny JSON.
pub const MAX_BODY: usize = 4 * 1024;

pub fn is_route(path: &str) -> bool {
    path == KEYS || POST_ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

/// K2's structured error: `{"ok":false,"error":{"code","hint","details"?}}`.
pub fn error(code_status: u16, code: &str, hint: &str, details: Option<Value>) -> CliResponse {
    let mut e = json!({ "code": code, "hint": hint });
    if let Some(d) = details {
        e["details"] = d;
    }
    CliResponse {
        status: status_line(code_status),
        content_type: "application/json",
        body: json!({ "ok": false, "error": e }).to_string(),
    }
}

fn key_error(e: &KeyError) -> CliResponse {
    match e {
        KeyError::LeadTooShort { retry_after_secs } => error(
            409,
            "lead_too_short",
            "the next key was published under 10 minutes ago; Sew may not have it yet. Try again later, or use an emergency rotation",
            Some(json!({ "retry_after_ms": (*retry_after_secs).max(0) * 1000 })),
        ),
        KeyError::VaultClosed(_) => error(503, "chat_keys_unavailable", "the K2 vault isn't open, so the chat keys can't be read", None),
        KeyError::Corrupt(_) => error(
            500,
            "chat_keys_unavailable",
            "the chat-pass keyring is damaged; recover with an emergency rotation (POST /cli/sew/keys/rotate {\"emergency\":true} with a confirm)",
            None,
        ),
        KeyError::Store(_) | KeyError::BadClaims(_) => error(500, "chat_keys_unavailable", "the chat-pass keyring failed; see the daemon log", None),
    }
}

/// The lifecycle record as the routes show it.
pub fn state_json(s: Option<&KeyState>) -> Value {
    let keys: Vec<Value> = s
        .map(|s| {
            s.published()
                .into_iter()
                .map(|k| {
                    let mut o = json!({
                        "kid": k.kid,
                        "state": k.state.as_str(),
                        "createdAt": k.created_at,
                        "publishedAt": k.published_at,
                    });
                    if let Some(a) = k.activated_at {
                        o["activatedAt"] = json!(a);
                    }
                    if let Some(u) = k.last_used_at {
                        o["lastUsedAt"] = json!(u);
                        o["retireAfter"] = json!(u + RETIRE_AFTER_LAST_USE_SECS);
                    }
                    if k.state == keyring::KeyRole::Current {
                        o["rotateDueAt"] = json!(k.activated_at.unwrap_or(k.created_at) + ROTATE_AFTER_SECS);
                    }
                    o
                })
                .collect()
        })
        .unwrap_or_default();
    let history: Vec<Value> = s
        .map(|s| s.history.iter().map(|h| json!({ "kid": h.kid, "retiredAt": h.retired_at })).collect())
        .unwrap_or_default();
    json!({
        "ok": true,
        "exists": s.is_some(),
        "keys": keys,
        "history": history,
        "rotateAfterSecs": ROTATE_AFTER_SECS,
        "publishLeadMinSecs": PUBLISH_LEAD_MIN_SECS,
        "retireAfterLastUseSecs": RETIRE_AFTER_LAST_USE_SECS,
        "passTtlSecs": PASS_TTL_SECS,
        "jwksPath": JWKS_PATH,
    })
}

// ── GET /v1/chat/jwks ───────────────────────────────────────────────

/// Public JWKS. Fail-closed 500 `chat_keys_unavailable` (no keys served
/// rather than stale or regenerated ones).
pub fn handle_jwks() -> CliResponse {
    match super::public_jwks() {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => {
            keyring::klog(format!("jwks unavailable (fail-closed): {e}"));
            let mut r = key_error(&e);
            r.status = "500 Internal Server Error";
            r
        }
    }
}

// ── owner key routes ────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotateBody {
    #[serde(default)]
    emergency: bool,
    confirm_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| {
        let msg = e.to_string();
        // serde names the field: "unknown field `x`, expected …". Never
        // echo values.
        let hint = match msg.split('`').nth(1) {
            Some(f) if msg.starts_with("unknown field") => format!("unknown field {f}"),
            _ => "the body must be {} or {\"emergency\":true,\"confirmToken\":\"…\"}".to_string(),
        };
        error(400, "validation_failed", &hint, None)
    })
}

fn caller_gate(caller: &Result<VaultCaller, k2_vault::VaultError>) -> Result<Actor, CliResponse> {
    let c = match caller {
        Ok(c) => c,
        Err(e) if e.status == 401 => return Err(error(401, "unauthenticated", "send the owner token or a Connect login as Authorization: Bearer", None)),
        Err(_) => return Err(error(403, "credential_not_allowed", "app passes and API keys can't manage the chat keys", None)),
    };
    if c.is_passport() {
        return Err(error(403, "owner_only", "agents can't manage the chat keys; tell your owner", None));
    }
    if !c.actor.is_admin_human() {
        return Err(error(403, "owner_only", "managing the chat keys needs the owner or an admin", None));
    }
    Ok(c.actor.clone())
}

/// One of the `/cli/sew/keys*` routes. `ingress` is `loopback` | `lan` |
/// `tunnel`.
pub fn handle(path: &str, is_post: bool, ingress: &str, caller: Result<VaultCaller, k2_vault::VaultError>, body: &[u8]) -> CliResponse {
    let ring = keyring::global();
    handle_with(path, is_post, ingress, caller, body, ring.as_deref().map_err(Clone::clone), keyring::now())
}

/// [`handle`] against a given keyring and clock (tests).
pub fn handle_with(
    path: &str,
    is_post: bool,
    ingress: &str,
    caller: Result<VaultCaller, k2_vault::VaultError>,
    body: &[u8],
    ring: Result<&keyring::Keyring, KeyError>,
    now: i64,
) -> CliResponse {
    if is_post_route(path) && !is_post {
        return error(405, "post_required", "this route is POST only", None);
    }
    if path == KEYS && is_post {
        return error(405, "get_required", "GET /cli/sew/keys reads; rotate with POST /cli/sew/keys/rotate", None);
    }
    if ingress == "tunnel" {
        return error(403, "loopback_only", "the chat keys are managed on the box or its LAN, not through the tunnel", None);
    }
    let actor = match caller_gate(&caller) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let stamp = actor.stamp();
    let ring = match ring {
        Ok(r) => r,
        Err(e) => return key_error(&e),
    };
    match path {
        KEYS => match ring.state() {
            Ok(s) => CliResponse::ok_json(state_json(s.as_ref()).to_string()),
            Err(e) => key_error(&e),
        },
        ROTATE => {
            let b: RotateBody = match parse(body) {
                Ok(b) => b,
                Err(r) => return r,
            };
            if !b.emergency {
                if b.confirm_token.is_some() {
                    return error(400, "validation_failed", "confirmToken goes with emergency:true only", None);
                }
                return match ring.rotate_now(now) {
                    Ok(s) => {
                        crate::vault::auth_audit("chat-keys-rotate", &stamp, "ok", ingress);
                        CliResponse::ok_json(state_json(Some(&s)).to_string())
                    }
                    Err(e) => {
                        crate::vault::auth_audit("chat-keys-rotate", &stamp, &format!("fail {}", err_code(&e)), ingress);
                        key_error(&e)
                    }
                };
            }
            // Emergency: a person's confirm, never the token alone.
            let Some(token) = b.confirm_token.as_deref().filter(|t| !t.is_empty()) else {
                crate::vault::auth_audit("chat-keys-emergency", &stamp, "deny vault_confirm_required", ingress);
                return error(
                    403,
                    "vault_confirm_required",
                    "an emergency key wipe needs a person: confirm first (POST /cli/vault/confirm/finish with action chat-keys-emergency: Touch ID, the vault password or your Connect password), then send confirmToken",
                    None,
                );
            };
            if let Err(e) = ring.vault().confirm.take(token, EMERGENCY_CONFIRM_ACTION, None, &stamp) {
                crate::vault::auth_audit("chat-keys-emergency", &stamp, &format!("deny {}", e.code), ingress);
                return error(403, e.code, &e.hint, None);
            }
            match ring.emergency(now) {
                Ok(s) => {
                    crate::vault::auth_audit("chat-keys-emergency", &stamp, "ok", ingress);
                    let mut v = state_json(Some(&s));
                    v["hint"] = json!(
                        "every old chat key is gone; passes signed with them fail once Sew refetches the JWKS (on the first unknown kid). Sew sessions already created are not revoked: end them in Sew."
                    );
                    CliResponse::ok_json(v.to_string())
                }
                Err(e) => {
                    crate::vault::auth_audit("chat-keys-emergency", &stamp, &format!("fail {}", err_code(&e)), ingress);
                    key_error(&e)
                }
            }
        }
        DROP_PREVIOUS => {
            if let Err(r) = parse::<EmptyBody>(body) {
                return r;
            }
            match ring.drop_previous(now) {
                Ok((s, dropped)) => {
                    crate::vault::auth_audit("chat-keys-drop-previous", &stamp, &format!("ok {}", dropped.len()), ingress);
                    let mut v = state_json(s.as_ref());
                    v["dropped"] = json!(dropped);
                    CliResponse::ok_json(v.to_string())
                }
                Err(e) => {
                    crate::vault::auth_audit("chat-keys-drop-previous", &stamp, &format!("fail {}", err_code(&e)), ingress);
                    key_error(&e)
                }
            }
        }
        _ => error(404, "not_found", "no such chat-key route", None),
    }
}

fn err_code(e: &KeyError) -> &'static str {
    match e {
        KeyError::VaultClosed(_) => "vault_closed",
        KeyError::Corrupt(_) => "corrupt",
        KeyError::Store(_) => "store",
        KeyError::BadClaims(_) => "bad_claims",
        KeyError::LeadTooShort { .. } => "lead_too_short",
    }
}
