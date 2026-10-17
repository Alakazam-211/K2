//! `POST /cli/sew/pass` and `POST /cli/sew/owner-handoff` (S28 §9, S32
//! §11.1; K2 work item K7).
//!
//! ## `POST /cli/sew/pass`
//!
//! Body `{}` (Bearer credential) or exactly `{"handoff":"k2sho_…"}` (no
//! bearer). Checks, in order:
//! 1. POST only (route policy; the dispatcher 405s a GET).
//! 2. Ingress `tunnel` → 403 `loopback_only` (a pass is only useful to Sew
//!    on this box; BFFs run on the box).
//! 3. A browser-shaped head ([`has_browser_headers`]: `Origin`, any
//!    `Sec-Fetch-Site`/`-Dest`/`-User`, or a `Sec-Fetch-Mode` other than
//!    `cors`) → 403 `browser_not_allowed` (browsers always send them on a
//!    fetch POST; server BFFs don't, and Node's `fetch` sends only
//!    `Sec-Fetch-Mode: cors`).
//! 4. Body: unknown field → 400 `validation_failed`; `handoff` with a
//!    bearer → 400.
//! 5. Credential → [`super::identity`] (refusals there), or the handoff
//!    code (401 `handoff_invalid`; marked used before signing).
//! 6. Rate limit per `sid` and per daemon → 429 `rate_limited` +
//!    `Retry-After`.
//! 7. Install id ([`super::link`]) → else 503 `chat_unavailable`.
//! 8. Sign with the keyring's `current` key → else 503.
//! 9. Audit (`chat-pass` / `chat-handoff`, `ok:<jti>`). Respond with
//!    `Cache-Control: no-store` (the dispatcher adds it).
//!
//! The 401 for an unknown credential is not delayed and not audited per
//! request (a 128-bit-plus random token, not a password).
//!
//! **A pass is never person proof** (KM13): the response says
//! `"personProof": false` and names the provenance; the owner token's pass
//! is marked `owner-token`.
//!
//! ## `POST /cli/sew/owner-handoff`
//!
//! Body `{"confirmToken":"k2vc_…"}`. The owner token or an Owner-role
//! Connect login, plus a vault confirm for action `chat-owner-handoff`
//! (Touch ID / the Mac login sheet, the vault password, or the Connect
//! password) — never the owner token alone, so an agent can't open Sew as
//! its install owner (Rosson 2026-10-15). Loopback/LAN only; 10 mints per
//! 60 s per daemon; ≤ 5 outstanding codes. Response `{ok, code,
//! expiresInMs: 60000}`; the audit names the code by its first 8 hex of
//! SHA-256, never the code.

use serde::Deserialize;
use serde_json::{json, Value};

use k2_vault::{Actor, Role, VaultError};

use super::handoff::{self, RedeemError};
use super::identity::{self, PassIdentity, Provenance, Refusal, SewRole};
use super::link::SewStatus;
use super::ratelimit::{self, Limiter};
use super::routes::error;
use super::{ChatPassClaims, KeyError, PASS_TTL_SECS};
use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;

pub const PASS: &str = "/cli/sew/pass";
pub const OWNER_HANDOFF: &str = "/cli/sew/owner-handoff";
pub const MAX_BODY: usize = 4 * 1024;

pub fn is_route(path: &str) -> bool {
    path == PASS || path == OWNER_HANDOFF
}

/// A response plus an optional `Retry-After` (seconds).
pub struct Reply {
    pub response: CliResponse,
    pub retry_after_secs: Option<i64>,
}

impl From<CliResponse> for Reply {
    fn from(response: CliResponse) -> Reply {
        Reply { response, retry_after_secs: None }
    }
}

/// Everything the pass route reads from the request.
pub struct PassRequest<'a> {
    /// `loopback` | `lan` | `tunnel`.
    pub ingress: &'a str,
    pub ip: &'a str,
    /// A browser-shaped header was present ([`has_browser_headers`]).
    pub browser: bool,
    /// `Authorization: Bearer` only (never `?token=` or a cookie).
    pub bearer: Option<&'a str>,
    pub owner_token: &'a str,
    pub body: &'a [u8],
}

/// The pieces a test swaps.
pub struct Deps<'a> {
    pub now: i64,
    pub owner_connect_is_k2_owner: bool,
    pub status: &'a dyn SewStatus,
    pub sign: &'a dyn Fn(&ChatPassClaims) -> Result<String, KeyError>,
    pub limiter: &'a Limiter,
    pub codes: &'a handoff::Store,
}

/// Whether a request head is browser-shaped: an `Origin` (every browser
/// sends it on a POST), any of `Sec-Fetch-Site` / `Sec-Fetch-Dest` /
/// `Sec-Fetch-User` (browsers send them together), or a `Sec-Fetch-Mode`
/// other than `cors`.
///
/// A lone `Sec-Fetch-Mode: cors` is NOT a browser: Node's built-in
/// `fetch` (undici) adds exactly that header to every request and nothing
/// else of the set, and K2's server SDK (`@k2/server`, which Apps use in
/// the wall) calls `fetch`. Browsers that send `Sec-Fetch-Mode` always
/// send `Sec-Fetch-Site` with it.
pub fn has_browser_headers(headers_blob: &str) -> bool {
    headers_blob.lines().skip(1).take_while(|l| !l.trim().is_empty()).any(|l| {
        let Some((n, v)) = l.split_once(':') else {
            return false;
        };
        let n = n.trim();
        let browser_only = ["origin", "sec-fetch-site", "sec-fetch-dest", "sec-fetch-user"];
        browser_only.iter().any(|b| n.eq_ignore_ascii_case(b))
            || (n.eq_ignore_ascii_case("sec-fetch-mode") && !v.trim().eq_ignore_ascii_case("cors"))
    })
}

fn audit(event: &str, user: &str, outcome: &str, ingress: &str, ip: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, user, outcome.to_string(), ingress, ip, "api"));
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PassBody {
    handoff: Option<String>,
}

fn parse_body<T: for<'de> Deserialize<'de> + Default>(body: &[u8], shape: &str) -> Result<T, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| {
        let msg = e.to_string();
        let hint = match msg.split('`').nth(1) {
            Some(f) if msg.starts_with("unknown field") => format!("unknown field {f}"),
            _ => format!("the body must be {shape}"),
        };
        error(400, "validation_failed", &hint, None)
    })
}

fn rate_limited(retry_ms: i64) -> Reply {
    Reply {
        response: error(
            429,
            "rate_limited",
            "too many chat passes; a BFF mints once per login and renew",
            Some(json!({ "retry_after_ms": retry_ms })),
        ),
        retry_after_secs: Some((retry_ms + 999) / 1000),
    }
}

/// `POST /cli/sew/pass` with the live keyring, link and limiter.
pub fn handle_pass(req: &PassRequest<'_>) -> Reply {
    let status = super::link::status_source();
    let sign = |c: &ChatPassClaims| super::sign_chat_pass(c);
    handle_pass_with(
        req,
        &Deps {
            now: super::keyring::now(),
            owner_connect_is_k2_owner: identity::OWNER_CONNECT_GETS_K2_OWNER,
            status: status.as_ref(),
            sign: &sign,
            limiter: &ratelimit::MINTS,
            codes: &handoff::CODES,
        },
    )
}

/// [`handle_pass`] against given deps (tests).
pub fn handle_pass_with(req: &PassRequest<'_>, d: &Deps<'_>) -> Reply {
    let bearer = req.bearer.map(str::trim).filter(|b| !b.is_empty());
    if req.ingress == "tunnel" {
        audit("chat-pass", "-", "loopback_only", req.ingress, req.ip);
        return error(403, "loopback_only", "chat passes are minted on the box (loopback or LAN), never through the tunnel", None).into();
    }
    if req.browser {
        audit("chat-pass", "-", "browser_not_allowed", req.ingress, req.ip);
        return error(
            403,
            "browser_not_allowed",
            "a chat pass goes to a server-side BFF, never a browser: call this from your App's server without Origin/Sec-Fetch-Mode",
            None,
        )
        .into();
    }
    let body: PassBody = match parse_body(req.body, "{} or {\"handoff\":\"k2sho_…\"}") {
        Ok(b) => b,
        Err(r) => return r.into(),
    };

    let mut handoff_minter: Option<String> = None;
    let (ident, event) = match body.handoff {
        Some(code) => {
            if bearer.is_some() {
                return error(400, "validation_failed", "handoff and bearer are exclusive", None).into();
            }
            // Rate limit before the code burns, keyed on the code's sid.
            let sid = handoff::sid_for(&code);
            if let Err(ms) = d.limiter.check_and_record(&sid, d.now) {
                audit("chat-handoff", "handoff", "rate_limited", req.ingress, req.ip);
                return rate_limited(ms);
            }
            match d.codes.redeem(&code, d.now) {
                Ok(minted_by) => {
                    handoff_minter = Some(minted_by);
                    (
                        PassIdentity {
                            sub: identity::OWNER_SUB.into(),
                            sid,
                            actor: format!("handoff:{}", handoff::audit_handle(&code)),
                            provenance: Provenance::OwnerHandoff,
                            sew_role: SewRole::InstallOwner,
                            caps: Vec::new(),
                            app: None,
                        },
                        "chat-handoff",
                    )
                }
                Err(e) => {
                    let e: RedeemError = e;
                    audit("chat-handoff", "handoff", &format!("handoff_invalid:{}", e.reason()), req.ingress, req.ip);
                    return error(401, "handoff_invalid", "this handoff code is unknown, used or expired; open Sew again from K2", None).into();
                }
            }
        }
        None => {
            let Some(bearer) = bearer else {
                return error(401, "unauthenticated", "send Authorization: Bearer (an App login session, a Connect login, or the owner token)", None)
                    .into();
            };
            match identity::classify_with(bearer, req.owner_token, d.owner_connect_is_k2_owner) {
                Ok(i) => {
                    if let Err(ms) = d.limiter.check_and_record(&i.sid, d.now) {
                        audit("chat-pass", &i.actor, "rate_limited", req.ingress, req.ip);
                        return rate_limited(ms);
                    }
                    (i, "chat-pass")
                }
                Err(Refusal::Unauthenticated) => {
                    return error(401, "unauthenticated", "that credential is unknown, expired or revoked", None).into();
                }
                Err(Refusal::NotAllowed { hint, actor }) => {
                    audit("chat-pass", &actor, "credential_not_allowed", req.ingress, req.ip);
                    return error(403, "credential_not_allowed", hint, None).into();
                }
                Err(Refusal::NoChatGrant { actor }) => {
                    audit("chat-pass", &actor, "no_chat_grant", req.ingress, req.ip);
                    return error(403, "no_chat_grant", "this person has no chat access (grant kind chat)", None).into();
                }
                Err(Refusal::Unavailable(e)) => {
                    super::keyring::klog(format!("pass mint: store unavailable: {e}"));
                    audit("chat-pass", "-", "chat_unavailable", req.ingress, req.ip);
                    return error(503, "chat_unavailable", "K2's user store is unavailable", None).into();
                }
            }
        }
    };

    let install_id = match super::link::install_id_for_mint(d.now, d.status) {
        Ok(id) => id,
        Err(e) => {
            super::keyring::klog(format!("pass mint: no install id: {e}"));
            audit(event, &ident.actor, "chat_unavailable", req.ingress, req.ip);
            return error(503, "chat_unavailable", "Sew is not enabled or not reachable", None).into();
        }
    };
    let claims = match ChatPassClaims::new(&install_id, &ident.sub, &ident.sid, d.now).and_then(|c| c.with_caps(ident.caps.clone())) {
        Ok(c) => c,
        Err(e) => {
            super::keyring::klog(format!("pass mint: bad claims: {e}"));
            audit(event, &ident.actor, "chat_unavailable", req.ingress, req.ip);
            return error(503, "chat_unavailable", "the pass could not be built", None).into();
        }
    };
    let pass = match (d.sign)(&claims) {
        Ok(p) => p,
        Err(e) => {
            super::keyring::klog(format!("pass mint: signing failed: {e}"));
            audit(event, &ident.actor, "chat_unavailable", req.ingress, req.ip);
            return error(503, "chat_unavailable", "the chat-pass keys are unavailable", None).into();
        }
    };
    let outcome = match &handoff_minter {
        Some(by) => format!("redeemed:{} minted-by:{by}", claims.jti),
        None => format!("ok:{}", claims.jti),
    };
    audit(event, &ident.actor, &outcome, req.ingress, req.ip);

    let expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(claims.exp, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();
    let mut v = json!({
        "ok": true,
        "pass": pass,
        "sub": claims.sub,
        "sid": claims.sid,
        "jti": claims.jti,
        "expiresAt": expires_at,
        "expiresInMs": PASS_TTL_SECS * 1000,
        "provenance": ident.provenance.as_str(),
        "personProof": false,
        "sewRole": ident.sew_role.as_str(),
    });
    if let Some(caps) = &claims.caps {
        v["caps"] = json!(caps);
    }
    if let Some(app) = &ident.app {
        v["app"] = json!(app);
    }
    CliResponse::ok_json(v.to_string()).into()
}

// ── POST /cli/sew/owner-handoff ─────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HandoffBody {
    confirm_token: Option<String>,
}

/// Consumes a vault confirm token for `(action, actor stamp)`; returns the
/// confirm method.
pub type ConfirmTake<'a> = &'a dyn Fn(&str, &str) -> Result<String, VaultError>;

/// Owner token or an Owner-role Connect login. Install-owner is the top of
/// Sew; Admins manage the install but don't become its owner.
fn owner_gate(caller: &Result<VaultCaller, VaultError>) -> Result<Actor, CliResponse> {
    let c = match caller {
        Ok(c) => c,
        Err(e) if e.status == 401 => return Err(error(401, "unauthenticated", "send the owner token or an Owner Connect login as Authorization: Bearer", None)),
        Err(_) => return Err(error(403, "owner_only", "app passes and API keys can't open Sew as its owner", None)),
    };
    if c.is_passport() {
        return Err(error(403, "owner_only", "agents never get Sew's install owner; ask a person to open Sew as owner", None));
    }
    match &c.actor {
        Actor::Owner | Actor::Login { role: Role::Owner, .. } => Ok(c.actor.clone()),
        _ => Err(error(403, "owner_only", "opening Sew as its install owner needs the box owner", None)),
    }
}

/// `POST /cli/sew/owner-handoff` with the live vault confirm broker.
pub fn handle_owner_handoff(ingress: &str, caller: Result<VaultCaller, VaultError>, body: &[u8]) -> Reply {
    let take = |token: &str, stamp: &str| -> Result<String, VaultError> {
        let v = crate::vault::get()?;
        match v.confirm.take(token, handoff::CONFIRM_ACTION, None, stamp)? {
            k2_vault::HumanProof::Confirmed { method } => Ok(method),
            k2_vault::HumanProof::None => Err(VaultError::forbidden("vault_confirm_required", "confirm again")),
        }
    };
    handle_owner_handoff_with(ingress, caller, body, super::keyring::now(), &take, &ratelimit::HANDOFFS, &handoff::CODES)
}

pub fn handle_owner_handoff_with(
    ingress: &str,
    caller: Result<VaultCaller, VaultError>,
    body: &[u8],
    now: i64,
    take: ConfirmTake<'_>,
    limiter: &Limiter,
    codes: &handoff::Store,
) -> Reply {
    if ingress == "tunnel" {
        return error(403, "loopback_only", "open Sew as owner from the box or its LAN, not through the tunnel", None).into();
    }
    let actor = match owner_gate(&caller) {
        Ok(a) => a,
        Err(r) => return r.into(),
    };
    let stamp = actor.stamp();
    let audit_user = match &actor {
        Actor::Owner => "owner-token".to_string(),
        Actor::Login { username, .. } => format!("user:{username}"),
        other => other.stamp(),
    };
    let b: HandoffBody = match parse_body(body, "{\"confirmToken\":\"k2vc_…\"}") {
        Ok(b) => b,
        Err(r) => return r.into(),
    };
    let Some(token) = b.confirm_token.as_deref().filter(|t| !t.is_empty()) else {
        audit("chat-handoff", &audit_user, "deny vault_confirm_required", ingress, "-");
        return error(
            403,
            "vault_confirm_required",
            "opening Sew as its owner needs a person: confirm first (POST /cli/vault/confirm/finish with action chat-owner-handoff: Touch ID, the vault password or your Connect password), then send confirmToken",
            None,
        )
        .into();
    };
    let method = match take(token, &stamp) {
        Ok(m) => m,
        Err(e) => {
            audit("chat-handoff", &audit_user, &format!("deny {}", e.code), ingress, "-");
            return error(403, e.code, &e.hint, None).into();
        }
    };
    if let Err(ms) = limiter.check_and_record("owner-handoff", now) {
        audit("chat-handoff", &audit_user, "rate_limited", ingress, "-");
        return rate_limited(ms);
    }
    let code = match codes.mint(now, &audit_user) {
        Ok(c) => c,
        Err(e) => {
            super::keyring::klog(format!("handoff mint: {e}"));
            return error(503, "chat_unavailable", "could not make a handoff code", None).into();
        }
    };
    audit("chat-handoff", &audit_user, &format!("minted:{} via {method}", handoff::audit_handle(&code)), ingress, "-");
    let v: Value = json!({
        "ok": true,
        "code": code,
        "expiresInMs": handoff::TTL_SECS * 1000,
        "confirmedBy": method,
    });
    CliResponse::ok_json(v.to_string()).into()
}
