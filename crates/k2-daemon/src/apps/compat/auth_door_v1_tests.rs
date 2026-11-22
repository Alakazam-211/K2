//! `/auth/v1` over K2 App users (no Postgres needed: the auth door is the
//! user store's).

use serde_json::{json, Value};

use super::super::door::tests::{cfg_for, fx, Fx};
use super::super::door::{self, publishable_key, Ctx, Ingress, SESSION_COOKIE};
use super::super::http::{DoorAnswer, DoorRequest};
use super::*;

fn ctx(f: &Fx, ingress: Ingress, edit: impl FnOnce(&mut super::super::door_config::DoorConfig)) -> Ctx {
    let mut cfg = cfg_for(&f.app, "shop");
    edit(&mut cfg);
    Ctx { cfg, service_id: f.svc.clone(), ingress }
}

fn public() -> Ingress {
    Ingress::Public { visitor: None }
}

fn req(method: &str, target: &str, headers: &[(&str, &str)], body: Value) -> DoorRequest {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    DoorRequest {
        method: method.into(),
        path: path.into(),
        query: query.into(),
        headers: headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).chain([("host".to_string(), "shop.example".to_string())]).collect(),
        body: if body.is_null() { Vec::new() } else { body.to_string().into_bytes() },
        close: false,
    }
}

fn body(a: &DoorAnswer) -> Value {
    if a.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&a.body).unwrap()
    }
}

fn header<'a>(a: &'a DoorAnswer, n: &str) -> Option<&'a str> {
    a.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
}

/// The pass in a Set-Cookie.
fn cookie_pass(a: &DoorAnswer) -> String {
    let c = header(a, "Set-Cookie").unwrap_or_else(|| panic!("no cookie: {:?}", a.headers));
    assert!(c.contains("HttpOnly") && c.contains("Secure") && c.contains("SameSite=Lax"), "{c}");
    c.split(';').next().unwrap().split_once('=').unwrap().1.to_string()
}

fn ann(f: &Fx) -> String {
    skin::list_principals().unwrap().into_iter().find(|p| p.id == f.ann_id).unwrap().username
}

#[test]
fn sign_in_answers_a_cookie_on_the_public_host_and_the_pass_on_the_link() {
    let f = fx("auth_in");
    skin::set_principal_password(&ann(&f), Some("correct horse 1")).unwrap();
    let pk = publishable_key(&f.svc);
    let login = |c: &Ctx, pw: &str| route(&req("POST", "/auth/v1/token?grant_type=password", &[("apikey", &pk)], json!({"email": "ann-auth_in@example.com", "password": pw})), c);
    // Public host: the session is the cookie; the JSON holds no token.
    let a = login(&ctx(&f, public(), |_| {}), "correct horse 1");
    assert_eq!(a.status, 200, "{}", body(&a));
    let b = body(&a);
    assert_eq!((b["access_token"].as_str(), b["refresh_token"].as_str(), b["token_type"].as_str()), (Some(""), Some(""), Some("bearer")));
    assert_eq!(b["user"]["id"], f.ann_id.as_str());
    assert_eq!(b["user"]["email"], "ann-auth_in@example.com");
    assert!(b["expires_at"].as_i64().unwrap() > now_secs());
    let pass = cookie_pass(&a);
    assert!(pass.starts_with("k2skn_"));
    assert!(!String::from_utf8_lossy(&a.body).contains("k2skn_"), "no pass in a browser's answer");
    // The link: the pass for server code.
    let a = login(&ctx(&f, Ingress::Link, |_| {}), "correct horse 1");
    let token = body(&a)["access_token"].as_str().unwrap().to_string();
    assert_eq!(token.split('.').count(), 3, "a JWT's shape: supabase-js decodes it in setSession");
    assert!(door::unwrap_pass(&token).starts_with("k2skn_"));
    let claims = door::wrapped_claims(&token).unwrap();
    assert_eq!((claims["sub"].as_str(), claims["role"].as_str()), (Some(f.ann_id.as_str()), Some("authenticated")));
    assert!(claims["exp"].as_i64().unwrap() > now_secs());
    assert!(!body(&a)["refresh_token"].as_str().unwrap().is_empty(), "setSession needs a refresh token");
    let lc = ctx(&f, Ingress::Link, |_| {});
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &format!("Bearer {token}"))], Value::Null), &lc);
    assert_eq!(a.status, 200, "the wrapped pass works: {}", body(&a));
    // Claims someone edited (another sub, an app_metadata K2 never wrote)
    // are refused, so getClaims never reads claims K2 didn't write.
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    for forged in [json!({"sub": "someone-else", "role": "authenticated"}), {
        let mut c = claims.clone();
        c["app_metadata"] = json!({"role": "admin"});
        c
    }] {
        let parts: Vec<&str> = token.split('.').collect();
        let bad = format!("{}.{}.{}", parts[0], b64.encode(forged.to_string()), parts[2]);
        let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &format!("Bearer {bad}"))], Value::Null), &lc);
        assert_eq!((a.status, body(&a)["code"].as_str()), (403, Some("bad_jwt")), "{forged}");
        let data = DoorRequest { method: "GET".into(), path: "/rest/v1/x".into(), headers: vec![("apikey".into(), pk.clone()), ("authorization".into(), format!("Bearer {bad}"))], ..Default::default() };
        assert!(door::resolve(&data, &lc).is_err(), "the data door refuses it too");
    }
    // The claims are only a mirror: a real, unedited token whose pass has
    // been revoked is refused by every door (the pass decides, never the claims).
    let other = body(&route(&req("POST", "/auth/v1/token?grant_type=password", &[("apikey", &pk)], json!({"email": "ann-auth_in@example.com", "password": "correct horse 1"})), &lc))["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    let pass_id = skin::resolve_skin_token(door::unwrap_pass(&other)).unwrap().id;
    assert!(skin::revoke_token(&pass_id).unwrap());
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &format!("Bearer {other}"))], Value::Null), &lc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (403, Some("bad_jwt")), "a revoked pass in a well-formed token");
    let data = DoorRequest { method: "GET".into(), path: "/rest/v1/x".into(), headers: vec![("apikey".into(), pk.clone()), ("authorization".into(), format!("Bearer {other}"))], ..Default::default() };
    assert!(door::resolve(&data, &lc).is_err(), "the data door refuses the revoked pass too");
    // The refresh token gives the same session back while it lives.
    let rt = body(&route(&req("POST", "/auth/v1/token?grant_type=password", &[("apikey", &pk)], json!({"email": "ann-auth_in@example.com", "password": "correct horse 1"})), &lc))["refresh_token"].as_str().unwrap().to_string();
    let a = route(&req("POST", "/auth/v1/token?grant_type=refresh_token", &[("apikey", &pk)], json!({"refresh_token": rt})), &lc);
    assert_eq!((a.status, body(&a)["user"]["id"].as_str()), (200, Some(f.ann_id.as_str())));
    assert!(header(&a, "Set-Cookie").is_none());
    // A wrong password, and an unknown email: the same answer.
    for (who, pw) in [("ann-auth_in@example.com", "nope nope"), ("nobody@example.com", "x")] {
        let a = route(&req("POST", "/auth/v1/token?grant_type=password", &[("apikey", &pk)], json!({"email": who, "password": pw})), &ctx(&f, public(), |_| {}));
        assert_eq!((a.status, body(&a)["code"].as_str()), (400, Some("invalid_credentials")));
    }
    // No apikey: refused before anything.
    let a = route(&req("POST", "/auth/v1/token?grant_type=password", &[], json!({})), &ctx(&f, public(), |_| {}));
    assert_eq!(body(&a)["code"], "no_api_key");
}

#[test]
fn get_update_and_sign_out_follow_the_session() {
    let f = fx("auth_user");
    let pk = publishable_key(&f.svc);
    let pc = ctx(&f, public(), |_| {});
    let lc = ctx(&f, Ingress::Link, |_| {});
    let cookie = format!("{SESSION_COOKIE}={}", f.web);
    // getUser: the cookie on the public host; a web pass as bearer only on the link.
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Cookie", &cookie)], Value::Null), &pc);
    assert_eq!((a.status, body(&a)["id"].as_str()), (200, Some(f.ann_id.as_str())));
    let web_bearer = format!("Bearer {}", f.web);
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &web_bearer)], Value::Null), &pc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (403, Some("web_pass_as_bearer")));
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &web_bearer)], Value::Null), &lc);
    assert_eq!(a.status, 200);
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk)], Value::Null), &pc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (401, Some("no_authorization")));
    // updateUser: the person's own password; metadata waits for K2's store.
    let a = route(&req("PUT", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &web_bearer)], json!({"password": "brand new pw"})), &lc);
    assert_eq!(a.status, 200, "{}", body(&a));
    assert!(skin::verify_principal_password(&ann(&f), "brand new pw"));
    assert!(skin::resolve_skin_token(&f.web).is_some(), "the session that changed it stays");
    let a = route(&req("PUT", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &web_bearer)], json!({"data": {"x": 1}})), &lc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (422, Some("not_supported")));
    // signOut (local): the pass ends; the public host clears the cookie.
    let a = route(&req("POST", "/auth/v1/logout?scope=local", &[("apikey", &pk), ("Cookie", &cookie)], Value::Null), &pc);
    assert_eq!(a.status, 204);
    assert!(header(&a, "Set-Cookie").unwrap().contains("Max-Age=0"));
    assert!(skin::resolve_skin_token(&f.web).is_none(), "signed out");
}

#[test]
fn admin_needs_the_server_key_and_links_need_their_scope() {
    let f = fx("auth_admin");
    let pk = publishable_key(&f.svc);
    let lc = ctx(&f, Ingress::Link, |_| {});
    let sk = f.service_key.clone();
    let a = route(&req("GET", "/auth/v1/admin/users", &[("apikey", &pk)], Value::Null), &lc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (403, Some("not_admin")));
    let a = route(&req("GET", "/auth/v1/admin/users", &[("apikey", &sk), ("Authorization", &format!("Bearer {sk}"))], Value::Null), &lc);
    assert_eq!(a.status, 200);
    let users = body(&a)["users"].as_array().unwrap().clone();
    assert!(users.iter().any(|u| u["id"] == f.ann_id.as_str()), "{users:?}");
    assert!(!users.iter().any(|u| u["email"].as_str().is_some_and(|e| e.starts_with("bo-"))), "another App's user never shows");
    assert_eq!(header(&a, "x-total-count"), Some(users.len().to_string().as_str()));
    // Create, read, update (ban), delete.
    let a = route(&req("POST", "/auth/v1/admin/users", &[("apikey", &sk)], json!({"email": "New.Person+x@example.com", "password": "temp pass 123", "email_confirm": true})), &lc);
    assert_eq!(a.status, 200, "{}", body(&a));
    let id = body(&a)["id"].as_str().unwrap().to_string();
    assert_eq!(body(&a)["email"], "new.person+x@example.com");
    let a = route(&req("POST", "/auth/v1/admin/users", &[("apikey", &sk)], json!({"email": "new.person+x@example.com"})), &lc);
    assert_eq!(body(&a)["code"], "email_exists");
    let a = route(&req("GET", &format!("/auth/v1/admin/users/{id}"), &[("apikey", &sk)], Value::Null), &lc);
    assert_eq!(body(&a)["id"], id.as_str());
    let a = route(&req("PUT", &format!("/auth/v1/admin/users/{id}"), &[("apikey", &sk)], json!({"ban_duration": "876000h"})), &lc);
    assert!(body(&a)["banned_until"].is_string(), "{}", body(&a));
    // The key set that password: the person must change it first.
    let a = route(&req("POST", "/auth/v1/token?grant_type=password", &[("apikey", &pk)], json!({"email": "new.person+x@example.com", "password": "temp pass 123"})), &lc);
    assert_ne!(a.status, 200, "{}", body(&a));
    // generate_link without the links scope.
    let a = route(&req("POST", "/auth/v1/admin/generate_link", &[("apikey", &sk)], json!({"type": "recovery", "email": "ann-auth_admin@example.com"})), &lc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (403, Some("links_scope_required")));
    let a = route(&req("DELETE", &format!("/auth/v1/admin/users/{id}"), &[("apikey", &sk)], Value::Null), &lc);
    assert_eq!(a.status, 200);
    let a = route(&req("GET", &format!("/auth/v1/admin/users/{id}"), &[("apikey", &sk)], Value::Null), &lc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (404, Some("user_not_found")));
}

#[test]
fn a_recovery_link_gives_a_pending_pass_that_only_sets_the_password() {
    let f = fx("auth_link");
    let pk = publishable_key(&f.svc);
    let (_, links_key) = k2_core::api_keys::create_app_key("links", &f.svc, r#"{"scopes":["service","links"]}"#).unwrap();
    // The server key on the public host (allowed here) so the link's host is known.
    let pc = ctx(&f, public(), |c| c.service_key_public = true);
    let lc = ctx(&f, Ingress::Link, |_| {});
    let a = route(
        &req("POST", "/auth/v1/admin/generate_link", &[("apikey", &links_key)], json!({"type": "recovery", "email": "ann-auth_link@example.com", "redirect_to": "https://shop.example/reset"})),
        &pc,
    );
    assert_eq!(a.status, 200, "{}", body(&a));
    let b = body(&a);
    let token = b["hashed_token"].as_str().unwrap().to_string();
    assert!(token.starts_with("k2inv_"));
    assert_eq!(b["verification_type"], "recovery");
    let action = b["action_link"].as_str().unwrap();
    assert!(action.starts_with("https://shop.example/auth/v1/verify?token=k2inv_") && action.contains("type=recovery") && action.contains("redirect_to=https%3A%2F%2Fshop.example%2Freset"), "{action}");
    // verifyOtp on the link: a pending pass, never a session.
    let a = route(&req("POST", "/auth/v1/verify", &[("apikey", &pk)], json!({"type": "recovery", "token_hash": token})), &lc);
    assert_eq!(a.status, 200, "{}", body(&a));
    let pending_pass = body(&a)["access_token"].as_str().unwrap().to_string();
    assert!(door::unwrap_pass(&pending_pass).starts_with(PENDING_PREFIX));
    let bearer = format!("Bearer {pending_pass}");
    let a = route(&req("GET", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &bearer)], Value::Null), &lc);
    assert_eq!(body(&a)["id"], f.ann_id.as_str());
    // The data door refuses it until the password is set.
    let data_req = DoorRequest { method: "GET".into(), path: "/rest/v1/notes".into(), headers: vec![("apikey".into(), pk.clone()), ("authorization".into(), bearer.clone())], ..Default::default() };
    assert!(door::resolve(&data_req, &lc).is_err());
    // Setting the password uses the link and makes the pass a session.
    let a = route(&req("PUT", "/auth/v1/user", &[("apikey", &pk), ("Authorization", &bearer)], json!({"password": "my own pass 9"})), &lc);
    assert_eq!(a.status, 200, "{}", body(&a));
    assert!(skin::verify_principal_password(&ann(&f), "my own pass 9"));
    let w = door::resolve(&data_req, &lc).expect("the upgraded pass works for server code");
    assert_eq!(w.caller.claims["sub"], f.ann_id.as_str());
    assert!(door::resolve(&data_req, &ctx(&f, public(), |_| {})).is_err(), "never as a bearer on the public host");
    // The link is used: verifying it again fails.
    let a = route(&req("POST", "/auth/v1/verify", &[("apikey", &pk)], json!({"type": "recovery", "token_hash": token})), &lc);
    assert_eq!((a.status, body(&a)["code"].as_str()), (403, Some("otp_expired")));
}

#[test]
fn a_clicked_link_sets_a_cookie_and_redirects_only_inside_the_app() {
    let f = fx("auth_click");
    let pk = publishable_key(&f.svc);
    let (_, links_key) = k2_core::api_keys::create_app_key("links", &f.svc, r#"{"scopes":["service","links"]}"#).unwrap();
    let pc = ctx(&f, public(), |c| c.service_key_public = true);
    let a = route(&req("POST", "/auth/v1/admin/generate_link", &[("apikey", &links_key)], json!({"type": "invite", "email": "ann-auth_click@example.com"})), &pc);
    let token = body(&a)["hashed_token"].as_str().unwrap().to_string();
    let click = |redirect: &str| route(&req("GET", &format!("/auth/v1/verify?token={token}&type=invite&redirect_to={redirect}"), &[("apikey", &pk)], Value::Null), &pc);
    let a = click("https://evil.example/x");
    assert_eq!(a.status, 400);
    let a = click("https://shop.example/welcome");
    assert_eq!((a.status, header(&a, "Location")), (303, Some("https://shop.example/welcome")));
    assert!(cookie_pass(&a).starts_with(PENDING_PREFIX));
    // An unknown token goes back to the page with GoTrue's error fragment.
    let a = route(&req("GET", "/auth/v1/verify?token=k2inv_nope&type=invite&redirect_to=https://shop.example/welcome", &[("apikey", &pk)], Value::Null), &pc);
    assert!(header(&a, "Location").unwrap().contains("#error=access_denied&error_code=otp_expired"));
}
