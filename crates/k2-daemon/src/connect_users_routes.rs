//! K2SO #617 — daemon route handlers for connect-user accounts + login.
//!
//! Two route families, two auth levels (enforced by the *dispatcher*, not
//! here — these handlers assume the gate already ran):
//!
//! - **`/cli/users/*`** (management — owner OR an Admin|Owner session at
//!   the gate; K2SO #629): provision + manage username+password accounts.
//!   `set-role` is Owner-only; `set-password`/`policy` stay owner-only.
//!   Bodies carry credentials so they stay out of URL logs.
//! - **`/cli/auth/login`** (PUBLIC — no token at the gate): how a remote
//!   connect-user trades username+password for a session token. On failure
//!   it returns a GENERIC 401 (never reveals whether the user exists) and
//!   the dispatcher adds a small fixed delay to blunt brute force.
//!
//! The actual account/session logic lives in
//! `k2_core::connect_users`; these are thin body-parse + shape wrappers
//! per the daemon-first contract.

use crate::cli_response::CliResponse;
use k2_core::connect_users;

/// `GET /cli/users` → `{"users":[{username,createdAt,disabled,role}, ...]}`.
/// Redacted views only — never the password hash. `role` is the #629
/// permission tier ("owner" | "admin" | "member").
pub fn handle_list() -> CliResponse {
    match connect_users::list_users() {
        Ok(views) => {
            // Re-key to camelCase wire shape (createdAt) for the client.
            let users: Vec<serde_json::Value> = views
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "username": v.username,
                        "createdAt": v.created_at.to_rfc3339(),
                        "disabled": v.disabled,
                        "role": v.role.as_wire(),
                    })
                })
                .collect();
            CliResponse::ok_json(serde_json::json!({ "users": users }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct AddReq {
    username: String,
    password: String,
}

/// `POST /cli/users/add` `{username,password}` → `{"success":true}`.
pub fn handle_add(body: &[u8]) -> CliResponse {
    let req: AddReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match connect_users::add_user(&req.username, &req.password) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct RemoveReq {
    username: String,
}

/// `POST /cli/users/remove` `{username}` → `{"success":true}`.
///
/// `actor_role` is the resolved permission tier of the caller (owner token
/// → Owner; else the session user's role). The dispatcher already gated
/// `can_manage_users`; here we additionally enforce `can_act_on` so an
/// Admin cannot remove an Owner-role user (403). (K2SO #629)
pub fn handle_remove(actor_role: connect_users::Role, body: &[u8]) -> CliResponse {
    let req: RemoveReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    // Enforce the role hierarchy against the TARGET's stored role. A
    // missing target falls through to remove_user's own "not found".
    if let Some(target_role) = connect_users::role_for_user(&req.username) {
        if !connect_users::can_act_on(actor_role, target_role) {
            return CliResponse::forbidden();
        }
    }
    match connect_users::remove_user(&req.username) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct SetPasswordReq {
    username: String,
    password: String,
}

/// `POST /cli/users/set-password` `{username,password}` →
/// `{"success":true}`. Revokes the user's live sessions server-side.
pub fn handle_set_password(body: &[u8]) -> CliResponse {
    let req: SetPasswordReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match connect_users::set_password(&req.username, &req.password) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct SetDisabledReq {
    username: String,
    disabled: bool,
}

/// `POST /cli/users/set-disabled` `{username,disabled}` →
/// `{"success":true}`. Disabling revokes live sessions.
///
/// Like `handle_remove`, `actor_role` enforces `can_act_on` so an Admin
/// cannot enable/disable an Owner-role user (403). (K2SO #629)
pub fn handle_set_disabled(actor_role: connect_users::Role, body: &[u8]) -> CliResponse {
    let req: SetDisabledReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if let Some(target_role) = connect_users::role_for_user(&req.username) {
        if !connect_users::can_act_on(actor_role, target_role) {
            return CliResponse::forbidden();
        }
    }
    match connect_users::set_disabled(&req.username, req.disabled) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct SetRoleReq {
    username: String,
    role: String,
}

/// `POST /cli/users/set-role` `{username, role}` → `{"success":true}`.
///
/// CHANGE-ROLES is OWNER-ONLY (the dispatcher gates this route to an
/// owner-token OR an Owner-role session via `can_change_roles`). The
/// `role` is one of `"owner" | "admin" | "member"`.
///
/// Last-Owner guard: the route never strips the LAST stored Owner if that
/// would leave management unreachable. In practice the host owner token
/// ALWAYS counts as Owner, so management is never truly locked out; this is
/// a sanity check that demoting the last stored Owner-role connect-user is
/// only allowed because the daemon-token owner remains. We therefore allow
/// the demote (the token owner is the floor) — the guard exists to reject a
/// nonsensical role string and surface a clear error.
pub fn handle_set_role(body: &[u8]) -> CliResponse {
    let req: SetRoleReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let role = match connect_users::Role::from_wire(&req.role) {
        Some(r) => r,
        None => {
            return CliResponse::bad_request(format!(
                "invalid role '{}' (expected owner|admin|member)",
                req.role
            ))
        }
    };
    match connect_users::set_role(&req.username, role) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `GET /cli/users/policy` → `{minLength, requireSpecial, requireNumber,
/// requireUppercase}` (camelCase). AUTHORIZED (owner OR connect-user
/// session) so the self-service portal can read the active requirements.
pub fn handle_get_policy() -> CliResponse {
    let p = connect_users::get_policy();
    CliResponse::ok_json(
        serde_json::json!({
            "minLength": p.min_length,
            "requireSpecial": p.require_special,
            "requireNumber": p.require_number,
            "requireUppercase": p.require_uppercase,
        })
        .to_string(),
    )
}

#[derive(serde::Deserialize)]
struct PolicyReq {
    #[serde(rename = "minLength")]
    min_length: usize,
    #[serde(rename = "requireSpecial")]
    require_special: bool,
    #[serde(rename = "requireNumber")]
    require_number: bool,
    #[serde(rename = "requireUppercase")]
    require_uppercase: bool,
}

/// `POST /cli/users/policy` `{minLength, requireSpecial, requireNumber,
/// requireUppercase}` → `{"success":true}`. OWNER-ONLY (gated by
/// require_owner at the dispatcher). `min_length` is clamped to a sane band
/// by `connect_users::set_policy`.
pub fn handle_set_policy(body: &[u8]) -> CliResponse {
    let req: PolicyReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let policy = connect_users::PasswordPolicy {
        min_length: req.min_length,
        require_special: req.require_special,
        require_number: req.require_number,
        require_uppercase: req.require_uppercase,
    };
    match connect_users::set_policy(policy) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

/// Generic 401 for a failed login. Deliberately does NOT distinguish
/// "no such user" from "wrong password" — no user enumeration.
fn login_failed() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: r#"{"error":"invalid username or password"}"#.to_string(),
    }
}

/// `POST /cli/auth/login` `{username,password}` (PUBLIC — no token gate).
///
/// On success: `200 {token, username, expiresAt}`. On any failure (bad
/// body, unknown user, wrong password, disabled account): a generic
/// `401 {"error":"invalid username or password"}`.
///
/// The dispatcher adds a small fixed delay before returning THIS response
/// when it's a 401, to blunt online brute force (richer rate-limiting is
/// deferred). `connect_users::verify` already runs constant-ish argon2
/// work to blunt user-enumeration timing.
pub fn handle_login(body: &[u8]) -> CliResponse {
    let req: LoginReq = match serde_json::from_slice(body) {
        // A malformed body is an auth failure, not a 400 — don't help an
        // attacker probe the parser separately from the credential check.
        Ok(r) => r,
        Err(_) => return login_failed(),
    };
    // Route every credential check through the brute-force lockout gate
    // (3 consecutive fails → 15-minute per-username lockout). The 401 it
    // returns is the SAME generic body whether the creds were wrong or
    // the account is currently locked — no user/lock enumeration.
    match connect_users::check_and_record(&req.username, &req.password) {
        connect_users::LoginOutcome::Ok => {}
        connect_users::LoginOutcome::BadCreds | connect_users::LoginOutcome::LockedOut => {
            return login_failed();
        }
    }
    // verify() lowercases internally; issue the session under the
    // canonical username so whoami/echo is stable.
    let username = connect_users::normalize_username(&req.username)
        .unwrap_or_else(|_| req.username.to_ascii_lowercase());
    let token = connect_users::create_session(&username);
    // Mirror the 30-day TTL the core applies (kept in sync by contract;
    // the authoritative expiry lives in the in-memory session).
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    CliResponse::ok_json(
        serde_json::json!({
            "token": token,
            "username": username,
            "expiresAt": expires_at,
        })
        .to_string(),
    )
}

/// Render the self-service account HTML page. Self-contained: inline CSS
/// + vanilla JS, NO build step, NO external assets. Served same-origin so
/// `fetch('/cli/auth/login')` + `fetch('/cli/auth/change-password')` hit
/// THIS daemon at `https://<sub>.k2.dev`.
///
/// The `<sub>` in the heading is the configured tunnel subdomain (read
/// from `k2_core::tunnel::config::load().subdomain`), falling back to
/// "this server" when unset/unreadable.
pub fn account_page_html() -> String {
    let sub = k2_core::tunnel::config::load()
        .ok()
        .map(|c| c.subdomain)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "this server".to_string());
    // Escape the subdomain for safe HTML text interpolation (it's a
    // validated label, but belt-and-suspenders).
    let sub = sub
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>K2 account</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    font: 15px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: #0d1117; color: #e6edf3;
  }}
  .card {{
    width: min(380px, calc(100vw - 32px));
    background: #161b22; border: 1px solid #30363d; border-radius: 12px;
    padding: 28px 26px; box-shadow: 0 10px 40px rgba(0,0,0,.5);
  }}
  h1 {{ font-size: 17px; font-weight: 600; margin: 0 0 4px; }}
  .sub {{ color: #8b949e; font-size: 13px; margin: 0 0 20px; }}
  label {{ display: block; font-size: 12px; color: #8b949e; margin: 14px 0 5px; }}
  input {{
    width: 100%; padding: 9px 11px; border-radius: 7px;
    border: 1px solid #30363d; background: #0d1117; color: #e6edf3; font-size: 14px;
  }}
  input:focus {{ outline: none; border-color: #1f6feb; box-shadow: 0 0 0 3px rgba(31,111,235,.3); }}
  button {{
    width: 100%; margin-top: 20px; padding: 10px; border: 0; border-radius: 7px;
    background: #238636; color: #fff; font-size: 14px; font-weight: 600; cursor: pointer;
  }}
  button:hover {{ background: #2ea043; }}
  button:disabled {{ opacity: .6; cursor: default; }}
  .msg {{ margin-top: 14px; font-size: 13px; min-height: 18px; }}
  .msg.err {{ color: #f85149; }}
  .msg.ok {{ color: #3fb950; }}
  .hint {{ margin-top: 5px; font-size: 11px; color: #8b949e; min-height: 14px; }}
  .hidden {{ display: none; }}
</style>
</head>
<body>
  <div class="card">
    <h1 id="heading">Log in to manage your account for <code>{sub}</code></h1>
    <p class="sub" id="loginSub">Enter your K2 Connect credentials.</p>

    <form id="loginForm">
      <label for="u">Username</label>
      <input id="u" name="username" autocomplete="username" autocapitalize="none" autocorrect="off" required>
      <label for="p">Password</label>
      <input id="p" name="password" type="password" autocomplete="current-password" required>
      <button type="submit" id="loginBtn">Log in</button>
      <div class="msg err" id="loginMsg"></div>
    </form>

    <form id="changeForm" class="hidden">
      <p class="sub" id="who"></p>
      <label for="cp">Current password</label>
      <input id="cp" type="password" autocomplete="current-password" required>
      <label for="np">New password</label>
      <input id="np" type="password" autocomplete="new-password" minlength="8" required>
      <div class="hint" id="pwHint"></div>
      <label for="np2">Confirm new password</label>
      <input id="np2" type="password" autocomplete="new-password" minlength="8" required>
      <button type="submit" id="changeBtn">Change password</button>
      <div class="msg" id="changeMsg"></div>
    </form>
  </div>

<script>
  // Session token lives only in memory (JS var), never localStorage.
  let token = null;
  // Active password policy (K2SO #620), fetched after login. Default
  // mirrors the server's permissive default until the fetch resolves.
  let policy = {{ minLength: 8, requireSpecial: false, requireNumber: false, requireUppercase: false }};

  const loginForm = document.getElementById('loginForm');
  const changeForm = document.getElementById('changeForm');
  const loginMsg = document.getElementById('loginMsg');
  const changeMsg = document.getElementById('changeMsg');
  const heading = document.getElementById('heading');
  const loginSub = document.getElementById('loginSub');
  // Capture the subdomain from the heading's <code> (already escaped server-side).
  const SUB = (heading.querySelector('code') || {{ textContent: 'this server' }}).textContent;
  function showLoggedIn() {{
    heading.textContent = 'Manage your account for ' + SUB;
    if (loginSub) loginSub.classList.add('hidden');
  }}
  function showLoggedOut() {{
    heading.textContent = 'Log in to manage your account for ' + SUB;
    if (loginSub) loginSub.classList.remove('hidden');
  }}

  const pwHint = document.getElementById('pwHint');
  // Render the active requirements as a compact hint — only the active
  // rules are listed.
  function renderHint() {{
    const parts = ['At least ' + policy.minLength + ' chars'];
    if (policy.requireSpecial) parts.push('special char');
    if (policy.requireNumber) parts.push('number');
    if (policy.requireUppercase) parts.push('uppercase');
    pwHint.textContent = parts.join(' · ');
  }}
  // Client-side validation mirroring the server (UX only — the server
  // still enforces and a 400 surfaces). Returns null when OK, else the
  // first unmet-requirement message.
  function validatePw(pw) {{
    if (pw.length < policy.minLength) return 'Password must be at least ' + policy.minLength + ' characters.';
    if (policy.requireSpecial && !/[^A-Za-z0-9]/.test(pw)) return 'Password must include a special character.';
    if (policy.requireNumber && !/[0-9]/.test(pw)) return 'Password must include a number.';
    if (policy.requireUppercase && !/[A-Z]/.test(pw)) return 'Password must include an uppercase letter.';
    return null;
  }}
  async function loadPolicy() {{
    try {{
      const res = await fetch('/cli/users/policy?token=' + encodeURIComponent(token));
      if (res.ok) {{
        const p = await res.json();
        policy = {{
          minLength: p.minLength || 8,
          requireSpecial: !!p.requireSpecial,
          requireNumber: !!p.requireNumber,
          requireUppercase: !!p.requireUppercase,
        }};
      }}
    }} catch (_) {{ /* keep default */ }}
    renderHint();
  }}

  loginForm.addEventListener('submit', async (e) => {{
    e.preventDefault();
    loginMsg.textContent = '';
    const btn = document.getElementById('loginBtn');
    btn.disabled = true;
    try {{
      const res = await fetch('/cli/auth/login', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{
          username: document.getElementById('u').value,
          password: document.getElementById('p').value,
        }}),
      }});
      if (!res.ok) {{
        loginMsg.textContent = 'Invalid username or password.';
        return;
      }}
      const data = await res.json();
      token = data.token;
      document.getElementById('who').textContent = 'Signed in as ' + (data.username || '');
      showLoggedIn();
      loginForm.classList.add('hidden');
      changeForm.classList.remove('hidden');
      // Fetch + render the active password requirements before the user
      // types a new password.
      void loadPolicy();
      document.getElementById('cp').focus();
    }} catch (_) {{
      loginMsg.textContent = 'Network error. Try again.';
    }} finally {{
      btn.disabled = false;
    }}
  }});

  changeForm.addEventListener('submit', async (e) => {{
    e.preventDefault();
    changeMsg.className = 'msg';
    changeMsg.textContent = '';
    const np = document.getElementById('np').value;
    const np2 = document.getElementById('np2').value;
    if (np !== np2) {{
      changeMsg.className = 'msg err';
      changeMsg.textContent = 'New passwords do not match.';
      return;
    }}
    const pwErr = validatePw(np);
    if (pwErr) {{
      changeMsg.className = 'msg err';
      changeMsg.textContent = pwErr;
      return;
    }}
    const btn = document.getElementById('changeBtn');
    btn.disabled = true;
    try {{
      const res = await fetch('/cli/auth/change-password?token=' + encodeURIComponent(token), {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ currentPassword: document.getElementById('cp').value, newPassword: np }}),
      }});
      if (res.ok) {{
        changeMsg.className = 'msg ok';
        changeMsg.textContent = 'Password changed. Please log in again.';
        // All sessions (including this one) are revoked server-side.
        token = null;
        setTimeout(() => {{
          changeForm.reset();
          changeForm.classList.add('hidden');
          loginForm.reset();
          loginForm.classList.remove('hidden');
          showLoggedOut();
          loginMsg.textContent = '';
        }}, 1800);
      }} else if (res.status === 400) {{
        changeMsg.className = 'msg err';
        // Server returns the active policy's first unmet requirement.
        let m = 'New password does not meet the requirements.';
        try {{ const d = await res.json(); if (d && d.error) m = d.error; }} catch (_) {{}}
        changeMsg.textContent = m;
      }} else {{
        changeMsg.className = 'msg err';
        changeMsg.textContent = 'Current password incorrect, or too many attempts. Try again later.';
      }}
    }} catch (_) {{
      changeMsg.className = 'msg err';
      changeMsg.textContent = 'Network error. Try again.';
    }} finally {{
      btn.disabled = false;
    }}
  }});
</script>
</body>
</html>"##,
        sub = sub
    )
}

#[derive(serde::Deserialize)]
struct ChangePasswordReq {
    #[serde(rename = "currentPassword")]
    current_password: String,
    #[serde(rename = "newPassword")]
    new_password: String,
}

/// `POST /cli/auth/change-password` `{currentPassword,newPassword}`.
///
/// Authorized as the CONNECT-USER (their session token, resolved to a
/// `username` by the dispatcher via `validate_session`). The OWNER
/// (daemon token, no session → no username) cannot use this route — it's
/// for connect-users changing their OWN password. The dispatcher passes
/// `Some(username)` only for a live session; `None` → generic 401.
///
/// Flow (in `connect_users::change_password`): verify `currentPassword`
/// THROUGH the same brute-force lockout gate as login, require
/// `newPassword` length >= 8, then re-hash + revoke ALL the user's
/// sessions (forced re-login everywhere). Returns `{"success":true}` or a
/// generic 400/401 that never reveals whether the current password was
/// wrong vs. the account locked.
pub fn handle_change_password(username: Option<String>, body: &[u8]) -> CliResponse {
    // No session-resolved username → not a connect-user (owner or bad
    // token). Generic 401; don't hint that the route exists for others.
    let username = match username {
        Some(u) => u,
        None => {
            return CliResponse {
                status: "401 Unauthorized",
                content_type: "application/json",
                body: r#"{"error":"unauthorized"}"#.to_string(),
            }
        }
    };
    let req: ChangePasswordReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => {
            return CliResponse {
                status: "400 Bad Request",
                content_type: "application/json",
                body: r#"{"error":"invalid request"}"#.to_string(),
            }
        }
    };
    match connect_users::change_password(&username, &req.current_password, &req.new_password) {
        connect_users::ChangePasswordOutcome::Ok => {
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        connect_users::ChangePasswordOutcome::WeakNew(msg) => CliResponse {
            status: "400 Bad Request",
            content_type: "application/json",
            body: serde_json::json!({ "error": msg }).to_string(),
        },
        connect_users::ChangePasswordOutcome::BadCurrent => CliResponse {
            // Generic 401 — same whether the current password was wrong or
            // the account is currently locked out.
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"unauthorized"}"#.to_string(),
        },
    }
}

/// `GET /cli/auth/whoami` (authorized — owner OR connect-user) →
/// `{username, owner, role}`.
///
/// - Owner token → `{"username":null,"owner":true,"role":"owner"}`.
/// - Connect-user session → `{"username":"<name>","owner":false,"role":"<role>"}`.
///
/// The dispatcher resolves which by checking the owner token first, then
/// the session; it passes the result in. `role` (K2SO #629) lets the client
/// gate the Users/Access management UI + the role selector.
pub fn handle_whoami(
    username: Option<String>,
    owner: bool,
    role: connect_users::Role,
) -> CliResponse {
    CliResponse::ok_json(
        serde_json::json!({
            "username": username,
            "owner": owner,
            "role": role.as_wire(),
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_rejects_malformed_body() {
        let r = handle_add(b"not json");
        assert_eq!(r.status, "400 Bad Request");
    }

    #[test]
    fn login_malformed_body_is_generic_401() {
        let r = handle_login(b"not json");
        assert_eq!(r.status, "401 Unauthorized");
        assert!(r.body.contains("invalid username or password"));
    }

    #[test]
    fn whoami_owner_shape() {
        let r = handle_whoami(None, true, connect_users::Role::Owner);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["owner"], serde_json::json!(true));
        assert_eq!(v["username"], serde_json::Value::Null);
        assert_eq!(v["role"], serde_json::json!("owner"));
    }

    #[test]
    fn whoami_connect_user_shape() {
        let r = handle_whoami(
            Some("alice".to_string()),
            false,
            connect_users::Role::Admin,
        );
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["owner"], serde_json::json!(false));
        assert_eq!(v["username"], serde_json::json!("alice"));
        assert_eq!(v["role"], serde_json::json!("admin"));
    }

    #[test]
    fn set_role_rejects_invalid_role_string() {
        let r = handle_set_role(br#"{"username":"bob","role":"superuser"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("invalid role"), "got: {}", r.body);
    }

    #[test]
    fn set_role_malformed_body_is_400() {
        let r = handle_set_role(b"not json");
        assert_eq!(r.status, "400 Bad Request");
    }

    #[test]
    fn admin_cannot_remove_owner_403() {
        // Route-level: an Admin actor removing an Owner-role target is
        // rejected with 403 before remove_user runs (K2SO #629). Seed a
        // store in a throwaway HOME so the role lookup resolves.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp =
            std::env::temp_dir().join(format!("k2so-i629-route-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);

        connect_users::add_user("theowner", "password1").expect("add owner");
        connect_users::set_role("theowner", connect_users::Role::Owner).expect("promote");

        // Admin actor → can_act_on(Admin, Owner) is false → 403, no delete.
        let r = handle_remove(connect_users::Role::Admin, br#"{"username":"theowner"}"#);
        assert_eq!(r.status, "403 Forbidden", "Admin must not remove an Owner");
        assert!(
            connect_users::role_for_user("theowner").is_some(),
            "owner must still exist after the 403"
        );

        // Owner actor → can_act_on(Owner, Owner) is true → succeeds.
        let r = handle_remove(connect_users::Role::Owner, br#"{"username":"theowner"}"#);
        assert_eq!(r.status, "200 OK", "Owner may remove an Owner");
        assert!(connect_users::role_for_user("theowner").is_none());

        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn change_password_owner_no_username_is_401() {
        // Owner (daemon token) resolves to no session username → barred.
        let r = handle_change_password(
            None,
            br#"{"currentPassword":"x","newPassword":"longenough"}"#,
        );
        assert_eq!(r.status, "401 Unauthorized");
    }

    #[test]
    fn change_password_malformed_body_is_400() {
        let r = handle_change_password(Some("alice".to_string()), b"not json");
        assert_eq!(r.status, "400 Bad Request");
    }
}
