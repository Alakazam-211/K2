//! `/auth/v1/admin/*`: supabase-js's `auth.admin` calls with the App's
//! server key (prd-app-supabase-compat-v1 §8.1, §8.3). They act on this
//! App's users only, with the App key's rules (`app_key_routes`): a person
//! who also holds a box login or another App's access is never removed or
//! reset from here, and a password the key sets is one the person must
//! change at their next sign-in (K2's must-change rule: someone else chose
//! it).
//!
//! `generate_link` (invite, recovery) needs the key's `links` scope: the
//! link is handed back for the App to mail itself.

use std::collections::HashMap;

use k2_core::app_grace::keys::{self, AppKeyGrant};
use k2_core::skin::{self, SkinPrincipal};
use serde_json::{json, Value};

use super::auth_door_v1::{gerr, user_json, API_VERSION};
use super::door::Ctx;
use super::http::{DoorAnswer, DoorRequest};
use k2_core::brand::PRODUCT_NAME;

fn ok(status: u16, v: Value) -> DoorAnswer {
    DoorAnswer::json(status, &v).header("x-supabase-api-version", API_VERSION)
}

fn store_err(e: impl std::fmt::Display) -> DoorAnswer {
    gerr(500, "unexpected_failure", e.to_string())
}

/// The App's public origins (`https://<label>.<box>.k2.dev`, custom names).
pub fn app_origins(svc: &str) -> Vec<String> {
    let name = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, svc).ok().flatten().map(|s| s.name).unwrap_or_default()
    };
    crate::apps::origins::for_service(svc, &name).into_iter().map(|o| o.trim_end_matches('/').to_string()).collect()
}

/// The runtime App id of a service (the claims' and users' `k2_app`).
fn app_id_of(svc: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::get_app_by_service(&conn, svc).ok().flatten().map(|a| a.app_id).unwrap_or_else(|| svc.to_string())
}

/// This App's users: (person, their grant on this App).
fn app_users(svc: &str) -> Result<Vec<(SkinPrincipal, skin::SkinGrant)>, DoorAnswer> {
    let principals = skin::list_principals().map_err(store_err)?;
    let grants = skin::list_grants().map_err(store_err)?;
    Ok(grants
        .into_iter()
        .filter(|g| g.subject_kind == "principal" && g.kind == "app" && g.target_id == svc)
        .filter_map(|g| principals.iter().find(|p| p.id == g.subject_id).cloned().map(|p| (p, g)))
        .collect())
}

fn one_user(svc: &str, id: &str) -> Result<(SkinPrincipal, skin::SkinGrant), DoorAnswer> {
    app_users(svc)?.into_iter().find(|(p, _)| p.id == id).ok_or_else(|| gerr(404, "user_not_found", "User not found"))
}

/// The person belongs to this App alone (no box login, no other App).
fn only_this_app(svc: &str, principal_id: &str) -> Result<bool, DoorAnswer> {
    let gs = skin::list_grants().map_err(store_err)?;
    Ok(gs.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == principal_id).all(|g| g.kind == "app" && g.target_id == svc))
}

fn admin_user_json(svc: &str, p: &SkinPrincipal, g: &skin::SkinGrant) -> Value {
    let mut u = user_json(&app_id_of(svc), p);
    if !g.enabled {
        u["banned_until"] = json!("2999-12-31T00:00:00Z");
    }
    u
}

fn body(req: &DoorRequest) -> Result<Value, DoorAnswer> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(&req.body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(gerr(400, "bad_json", "the body is a JSON object")),
    }
}

fn has_metadata(v: &Value) -> bool {
    ["user_metadata", "app_metadata", "data"].iter().any(|k| v.get(*k).and_then(Value::as_object).is_some_and(|o| !o.is_empty()))
}

/// A K2 username from an email: its local part in K2's alphabet, with a
/// number when taken.
fn username_for(email: &str) -> Result<String, DoorAnswer> {
    let local: String = email.split('@').next().unwrap_or("").to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    let mut base: String = local.trim_matches('-').chars().take(56).collect();
    if base.len() < 2 {
        base = format!("user-{base}");
    }
    for n in 0..1000 {
        let u = if n == 0 { base.clone() } else { format!("{base}-{n}") };
        if !skin::principal_exists(&u).map_err(store_err)? {
            return Ok(u);
        }
    }
    Err(gerr(422, "username_exhausted", concat!("no free ", k2_core::product_name!(), " username for that email")))
}

/// Route one admin call (the server key, already checked).
pub fn route(req: &DoorRequest, ctx: &Ctx, key_id: &str, grant: &AppKeyGrant, q: &HashMap<String, String>) -> DoorAnswer {
    let rest = req.path.strip_prefix(super::door::AUTH).unwrap_or("").strip_prefix("/admin").unwrap_or("");
    let svc = ctx.service_id.as_str();
    let actor = format!("app-key:{key_id}");
    let r = match (req.method.as_str(), rest) {
        ("GET", "/users") => list(svc, q),
        ("POST", "/users") => create(svc, &actor, req),
        ("POST", "/generate_link") => {
            if !grant.has(keys::SCOPE_LINKS) {
                return gerr(
                    403,
                    "links_scope_required",
                    concat!("generate_link hands the link back to the App: the owner mints the server key with the links scope (", k2_core::cli_name!(), " app key mint <app> --scopes service,links)"),
                );
            }
            generate_link(req, ctx, &actor)
        }
        (m, r) if r.starts_with("/users/") => {
            let id = &r["/users/".len()..];
            if id.is_empty() || id.contains('/') {
                return gerr(404, "not_found", "no such admin route");
            }
            match m {
                "GET" => one_user(svc, id).map(|(p, g)| ok(200, admin_user_json(svc, &p, &g))),
                "PUT" => update(svc, &actor, id, req),
                "DELETE" => delete(svc, id),
                _ => Err(gerr(405, "method_not_allowed", "GET, PUT or DELETE")),
            }
        }
        _ => Err(gerr(404, "not_found", format!("{} /auth/v1/admin{rest}: not supported by {PRODUCT_NAME}", req.method))),
    };
    let a = r.unwrap_or_else(|a| a);
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "app-key-auth-admin",
        &actor,
        format!("{} /auth/v1/admin{rest} -> {} app={svc}", req.method, a.status),
        "app",
        "-",
        "apps",
    ));
    a
}

fn list(svc: &str, q: &HashMap<String, String>) -> Result<DoorAnswer, DoorAnswer> {
    let page = q.get("page").and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).max(1);
    let per = q.get("per_page").and_then(|p| p.parse::<usize>().ok()).unwrap_or(50).clamp(1, 1000);
    let mut all = app_users(svc)?;
    all.sort_by_key(|(p, _)| (p.created_at, p.id.clone()));
    let total = all.len();
    let users: Vec<Value> = all.iter().skip((page - 1) * per).take(per).map(|(p, g)| admin_user_json(svc, p, g)).collect();
    let last = total.div_ceil(per).max(1);
    let mut links = Vec::new();
    if page < last {
        links.push(format!("</admin/users?page={}&per_page={per}>; rel=\"next\"", page + 1));
    }
    links.push(format!("</admin/users?page={last}&per_page={per}>; rel=\"last\""));
    Ok(ok(200, json!({ "users": users, "aud": "authenticated" })).header("x-total-count", total.to_string()).header("Link", links.join(", ")))
}

fn create(svc: &str, actor: &str, req: &DoorRequest) -> Result<DoorAnswer, DoorAnswer> {
    let v = body(req)?;
    if has_metadata(&v) {
        return Err(gerr(422, "not_supported", concat!("user_metadata and app_metadata are not stored by ", k2_core::product_name!(), " yet")));
    }
    if v.get("phone").is_some() {
        return Err(gerr(422, "not_supported", concat!("phone users are not supported by ", k2_core::product_name!())));
    }
    let email = v.get("email").and_then(Value::as_str).map(str::trim).filter(|e| !e.is_empty()).ok_or_else(|| gerr(400, "validation_failed", "email is required"))?;
    let email = skin::normalize_email(email).map_err(|e| gerr(400, "validation_failed", e))?.ok_or_else(|| gerr(400, "validation_failed", "email is required"))?;
    if skin::login_principal_username(&email).map_err(store_err)?.is_some() {
        return Err(gerr(422, "email_exists", "A user with this email address has already been registered"));
    }
    let username = username_for(&email)?;
    let ws = skin::ties::app_workspace(svc).ok_or_else(|| gerr(409, "app_gone", "this App's service is gone"))?;
    let by = skin::AddedBy { app_key_workspace: Some(&ws), ..Default::default() };
    let p = skin::add_principal_for_app_by(&username, Some(&email), svc, None, actor, by).map_err(|e| gerr(422, "validation_failed", e))?;
    if let Some(pw) = v.get("password").and_then(Value::as_str).filter(|p| !p.is_empty()) {
        let tie = skin::ties::TieWrite { workspace_id: &ws, set_by: actor, reason: skin::ties::REASON_APP_KEY };
        // Must-change: the key chose it, not the person.
        skin::set_principal_password_tied(&p.username, Some(pw), Some(tie), true).map_err(|e| gerr(422, "weak_password", e))?;
    }
    let (p, g) = one_user(svc, &p.id)?;
    Ok(ok(200, admin_user_json(svc, &p, &g)))
}

fn update(svc: &str, actor: &str, id: &str, req: &DoorRequest) -> Result<DoorAnswer, DoorAnswer> {
    let v = body(req)?;
    if has_metadata(&v) {
        return Err(gerr(422, "not_supported", concat!("user_metadata and app_metadata are not stored by ", k2_core::product_name!(), " yet")));
    }
    let (p, g) = one_user(svc, id)?;
    let sole = only_this_app(svc, &p.id)?;
    if let Some(pw) = v.get("password").and_then(Value::as_str).filter(|p| !p.is_empty()) {
        if !sole {
            return Err(gerr(403, "not_only_this_app", "this person also signs in elsewhere on this server; only the owner sets their password"));
        }
        let ws = skin::ties::app_workspace(svc).ok_or_else(|| gerr(409, "app_gone", "this App's service is gone"))?;
        let tie = skin::ties::TieWrite { workspace_id: &ws, set_by: actor, reason: skin::ties::REASON_APP_KEY };
        skin::set_principal_password_tied(&p.username, Some(pw), Some(tie), true).map_err(|e| gerr(422, "weak_password", e))?;
    }
    if let Some(email) = v.get("email").and_then(Value::as_str) {
        if !sole {
            return Err(gerr(403, "not_only_this_app", "this person also signs in elsewhere on this server; only the owner changes their email"));
        }
        skin::set_principal_email_as(&p.username, Some(email), actor).map_err(|e| gerr(422, "validation_failed", e))?;
    }
    if let Some(ban) = v.get("ban_duration").and_then(Value::as_str) {
        let on = ban == "none" || ban == "0" || ban.is_empty();
        skin::set_grant_enabled(&g.id, on).map_err(store_err)?;
    }
    let (p, g) = one_user(svc, id)?;
    Ok(ok(200, admin_user_json(svc, &p, &g)))
}

fn delete(svc: &str, id: &str) -> Result<DoorAnswer, DoorAnswer> {
    let (p, g) = one_user(svc, id)?;
    if only_this_app(svc, &p.id)? {
        if skin::remove_principal(&p.username).map_err(store_err)? {
            crate::read_routes::forget_guest(&p.id);
        }
    } else {
        // Someone with other access keeps it; this App's access goes.
        skin::delete_grant_by_id(&g.id).map_err(store_err)?;
    }
    Ok(ok(200, json!({})))
}

fn generate_link(req: &DoorRequest, ctx: &Ctx, actor: &str) -> Result<DoorAnswer, DoorAnswer> {
    let svc = ctx.service_id.as_str();
    let v = body(req)?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let purpose = match kind {
        "invite" => skin::invites::PURPOSE_INVITE,
        "recovery" => skin::invites::PURPOSE_RESET,
        k => return Err(gerr(422, "not_supported", format!("generate_link type {k} is not supported by {PRODUCT_NAME} (invite and recovery are)"))),
    };
    let email = v.get("email").and_then(Value::as_str).map(str::trim).filter(|e| !e.is_empty()).ok_or_else(|| gerr(400, "validation_failed", "email is required"))?;
    let email = skin::normalize_email(email).map_err(|e| gerr(400, "validation_failed", e))?.unwrap_or_default();
    // An invite to a new address makes the person first, as GoTrue does.
    let pid = match skin::login_principal_username(&email).map_err(store_err)? {
        Some(u) => skin::list_principals().map_err(store_err)?.into_iter().find(|p| p.username == u).map(|p| p.id),
        None if kind == "invite" => {
            let fake = DoorRequest { body: json!({ "email": email }).to_string().into_bytes(), ..Default::default() };
            let a = create(svc, actor, &fake)?;
            serde_json::from_slice::<Value>(&a.body).ok().and_then(|u| u["id"].as_str().map(str::to_string))
        }
        None => None,
    };
    let Some(pid) = pid else { return Err(gerr(404, "user_not_found", "User with this email not found")) };
    let (p, g) = one_user(svc, &pid)?;
    if !only_this_app(svc, &p.id)? {
        return Err(gerr(403, "not_only_this_app", "this person also signs in elsewhere on this server; only the owner sends them a link"));
    }
    let ws = skin::ties::app_workspace(svc).ok_or_else(|| gerr(409, "app_gone", "this App's service is gone"))?;
    if skin::ties::get(&p.id).map_err(store_err)?.is_some_and(|t| t.workspace_id != ws) {
        return Err(gerr(409, skin::ties::TIED_ELSEWHERE, "this person is tied to another workspace; only the owner sends them a link"));
    }
    if !skin::guard::take_reset_mail(&p.id, svc) {
        return Err(gerr(429, "over_email_send_rate_limit", "too many links for this person or this App lately; try again later"));
    }
    let settings = skin::invites::settings(svc).map_err(store_err)?;
    let ttl = if purpose == skin::invites::PURPOSE_INVITE { settings.invite_ttl_secs } else { settings.reset_ttl_secs };
    let minted = match skin::invites::mint(&p.id, svc, purpose, ttl, p.email.as_deref(), false, Some((&ws, skin::ties::REASON_APP_KEY)), actor) {
        Ok(m) => m,
        Err(e) => {
            skin::guard::return_reset_mail(&p.id, svc);
            return Err(match e {
                skin::invites::MintError::Throttled => gerr(429, "over_email_send_rate_limit", "one link per person a minute"),
                skin::invites::MintError::TiedElsewhere => gerr(409, skin::ties::TIED_ELSEWHERE, "this person is tied to another workspace"),
                skin::invites::MintError::OwnerResetPending => gerr(409, "owner_reset_pending", "the owner sent this person a reset; it stays the only one until it is used or runs out"),
                skin::invites::MintError::Store(e) => store_err(e),
            });
        }
    };
    skin::invites::activate(&minted.id).map_err(store_err)?;
    let redirect = v.get("redirect_to").or_else(|| v.pointer("/options/redirectTo")).and_then(Value::as_str).unwrap_or("").to_string();
    let base = match &ctx.ingress {
        super::door::Ingress::Public { .. } => req.header("host").map(|h| format!("https://{h}")),
        super::door::Ingress::Link => app_origins(svc).into_iter().find(|o| o.starts_with("https://")),
    }
    .ok_or_else(|| gerr(409, "no_public_host", "this App has no public address for the link"))?;
    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    let mut action = format!("{base}/auth/v1/verify?token={}&type={kind}", minted.token);
    if !redirect.is_empty() {
        action.push_str(&format!("&redirect_to={}", enc(&redirect)));
    }
    skin::guest_rooms::audit_user_event(svc, "app_user_link_generated", actor, json!({ "username": p.username, "type": kind }));
    let mut out = admin_user_json(svc, &p, &g);
    out["action_link"] = json!(action);
    out["email_otp"] = json!("");
    out["hashed_token"] = json!(minted.token);
    out["verification_type"] = json!(kind);
    out["redirect_to"] = json!(redirect);
    Ok(ok(200, out))
}
