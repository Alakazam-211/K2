//! O1 — the provider-agnostic OAuth 2.0 engine for Email Link
//! (prd-email-oauth-providers-v1). This slice is the SHARED ENGINE
//! ONLY: it knows how to run the two token-acquisition flows, refresh +
//! revoke tokens, and vault them. It does NOT touch IMAP/Graph
//! transport, routes, the CLI, or the UI — those are O2–O5.
//!
//! Two flows (prd §3):
//!   * **Device-code (RFC 8628)** — `device_start` + `device_poll`.
//!     Works for local AND remote/headless daemons (no redirect): the
//!     daemon shows a short code + URL, the user approves in any
//!     browser, the daemon polls the token endpoint. The default for
//!     Microsoft here.
//!   * **Auth-code + loopback (RFC 8252)** — `loopback_build_url` +
//!     `loopback_exchange`, plus [`bind_ephemeral_loopback`] for O2's
//!     127.0.0.1 listener. The fast-path for a known-local daemon; the
//!     default for Gmail here.
//!
//! **No real network in this module's tests.** Every HTTP effect goes
//! through the injected [`HttpClient`] trait, so unit tests drive a
//! canned mock token endpoint — mirrors the `ImapOps`/JMAP fake pattern.
//! Production uses [`ReqwestHttp`] (blocking reqwest, form-urlencoded).
//!
//! **Client secret: Google only, and it is NOT confidential.** Microsoft
//! device-code is a TRUE public client — no secret, ever. But Google
//! "Desktop app" clients REQUIRE a `client_secret` at the token endpoint
//! (auth-code exchange AND refresh) or Google returns `invalid_client`.
//! That secret ships inside installed apps, so it is not confidential and
//! may be carried as config — but it is handled EXACTLY like a token:
//! vault/config only, NEVER a DB column, NEVER logged, redacted in
//! `Debug`. Only a `client_id` (+ optional Google `client_secret`) is
//! configured — placeholder consts now; Rosson registers the real
//! Google/Azure apps, prd §15 Q1 — with BYO override seams for
//! enterprises.
//!
//! **Tokens are vaulted, never a column, never logged.** They live in
//! the daemon vault under `ext-inbox-<row-id>-oauth` (JSON). [`Tokens`]
//! and [`OauthError`] REDACT every secret in their `Debug`/error
//! surface — a token in a log line is a full account compromise.

// O1 is the SHARED ENGINE with no in-tree callers yet (the routes/CLI
// that consume it land in O2). In a `bin` crate `pub` does not suppress
// `dead_code`, so this whole module is `allow(dead_code)` until O2 wires
// it in; the unit tests below exercise every path in the meantime.
#![allow(dead_code)]

use crate::mail::secrets::SecretStore;
use base64::Engine as _;
use parking_lot::Mutex as PlMutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

// ─────────────────────────────────────────────────────────────────────
// Providers + static config (prd §9)
// ─────────────────────────────────────────────────────────────────────

/// The OAuth vendors Email Link speaks. `kind` on the inbox row stays
/// `'imap'` for the XOAUTH2 transport (O3); this enum records which
/// vendor drives endpoints / scopes / refresh semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OauthProvider {
    Gmail,
    Microsoft,
}

impl OauthProvider {
    /// The stable DB spelling stored in `mail_external_inboxes.provider`.
    pub fn as_str(self) -> &'static str {
        match self {
            OauthProvider::Gmail => "gmail",
            OauthProvider::Microsoft => "microsoft",
        }
    }

    /// Parse the DB spelling back. Unknown → None (caller decides).
    pub fn from_str(s: &str) -> Option<OauthProvider> {
        match s {
            "gmail" => Some(OauthProvider::Gmail),
            "microsoft" => Some(OauthProvider::Microsoft),
            _ => None,
        }
    }

    pub fn config(self) -> &'static ProviderConfig {
        match self {
            OauthProvider::Gmail => &GMAIL,
            OauthProvider::Microsoft => &MICROSOFT,
        }
    }
}

/// Which flow a provider defaults to here (both flows are BUILT for both
/// providers; this only records the preferred entry point per prd §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    /// RFC 8628 device-code (show a code, poll).
    DeviceCode,
    /// RFC 8252 system-browser + 127.0.0.1 loopback redirect.
    Loopback,
}

/// Static per-provider OAuth endpoints + scope (prd §9). Endpoints are
/// public knowledge. The only sensitive-ish field is Google's
/// `client_secret_placeholder` (a Desktop-client secret — not truly
/// confidential, but handled like a token): the manual `Debug` impl
/// below REDACTS it so it can never reach a log line. `auth_url` powers
/// the loopback flow, `device_auth_url` the device flow — a provider
/// that lacks one for the requested flow yields an [`OauthError::Config`].
pub struct ProviderConfig {
    /// The vendor's stable spelling (must match `OauthProvider::as_str`).
    pub name: &'static str,
    /// Authorization endpoint for the auth-code / loopback flow (§3b).
    pub auth_url: Option<&'static str>,
    /// Device-authorization endpoint for the device-code flow (§3a).
    pub device_auth_url: Option<&'static str>,
    /// Token endpoint (device poll, loopback exchange, refresh).
    pub token_url: &'static str,
    /// Best-effort revocation endpoint (None = the vendor has no simple
    /// public revoke; unlink still wipes the vault entry).
    pub revoke_url: Option<&'static str>,
    /// The OAuth scope string requested (space-separated).
    pub scope: &'static str,
    /// Extra query params appended to the loopback auth URL (e.g.
    /// Google's `access_type=offline` + `prompt=consent`, which are what
    /// make Google mint a refresh_token). Never secrets.
    pub extra_auth_params: &'static [(&'static str, &'static str)],
    /// The default flow for this provider (prd §9).
    pub default_flow: FlowKind,
    /// The PUBLIC-client id placeholder. Rosson swaps these for the real
    /// registered Google/Azure client ids (prd §15 Q1). A per-call
    /// override (BYO client_id, enterprise) wins over this.
    pub client_id_placeholder: &'static str,
    /// The token-endpoint client secret placeholder, when the provider
    /// requires one. `Some` for Google (Desktop clients MUST send a
    /// `client_secret` on the auth-code exchange AND refresh or Google
    /// returns `invalid_client`); `None` for Microsoft (a true public
    /// client — its forms carry NO secret, ever). Not truly confidential
    /// (it ships in installed apps) but handled like a token: vault/config
    /// only, never a DB column, never logged, REDACTED in `Debug`. A
    /// per-call override wins over this.
    pub client_secret_placeholder: Option<&'static str>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("name", &self.name)
            .field("auth_url", &self.auth_url)
            .field("device_auth_url", &self.device_auth_url)
            .field("token_url", &self.token_url)
            .field("revoke_url", &self.revoke_url)
            .field("scope", &self.scope)
            .field("extra_auth_params", &self.extra_auth_params)
            .field("default_flow", &self.default_flow)
            .field("client_id_placeholder", &self.client_id_placeholder)
            // A client secret NEVER reaches a log line — redact but keep
            // the Some/None shape so config debugging still works.
            .field(
                "client_secret_placeholder",
                &self.client_secret_placeholder.map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Google / Gmail / Google Workspace (prd §9). Loopback is the default;
/// the IMAP scope `https://mail.google.com/` is coarse by necessity
/// (there is no read-only IMAP scope). `access_type=offline` +
/// `prompt=consent` force a refresh_token on the loopback grant.
// Build-time credential injection (prd §15 Q1 / release hygiene). The
// real Alakazam client id/secret are injected at BUILD time via env vars
// (`K2_GMAIL_CLIENT_ID` / `K2_GMAIL_CLIENT_SECRET` / `K2_MICROSOFT_CLIENT_ID`)
// so the PUBLIC source only ever carries the `REPLACE_ME` fallback — no
// secret is committed. A dev/CI build without the env vars falls back to
// the placeholder (which Google rejects as invalid_client — whole Linux
// fleet outage if release builds forget these). Shipping builds MUST set
// them: release.sh / build-app.sh source .env + scripts/require-mail-
// oauth-build-env.sh; daemon-binaries.yml (Linux GHA) injects the same
// names from repo Actions secrets and strings-checks the binary for
// REPLACE_ME. The secret is still handled like a token everywhere
// downstream (vault/config only, never a DB column, never logged,
// redacted in `Debug`).
const GMAIL_CLIENT_ID: &str = match option_env!("K2_GMAIL_CLIENT_ID") {
    Some(v) => v,
    None => "REPLACE_ME.apps.googleusercontent.com",
};
const GMAIL_CLIENT_SECRET: &str = match option_env!("K2_GMAIL_CLIENT_SECRET") {
    Some(v) => v,
    None => "REPLACE_ME-google-client-secret",
};
const MICROSOFT_CLIENT_ID: &str = match option_env!("K2_MICROSOFT_CLIENT_ID") {
    Some(v) => v,
    None => "REPLACE_ME-microsoft-client-id",
};

pub static GMAIL: ProviderConfig = ProviderConfig {
    name: "gmail",
    auth_url: Some("https://accounts.google.com/o/oauth2/v2/auth"),
    device_auth_url: Some("https://oauth2.googleapis.com/device/code"),
    token_url: "https://oauth2.googleapis.com/token",
    revoke_url: Some("https://oauth2.googleapis.com/revoke"),
    // MCS1: the `mail` feature set ([`scope_for`] with mail only) —
    // identity (`openid email`, for the account check) + the IMAP/SMTP
    // XOAUTH2 scope. `scope_for` is the source; a test pins them equal.
    scope: "openid email https://mail.google.com/",
    extra_auth_params: &[("access_type", "offline"), ("prompt", "consent")],
    default_flow: FlowKind::Loopback,
    // Injected at build time (env `K2_GMAIL_CLIENT_ID`); `REPLACE_ME` fallback.
    client_id_placeholder: GMAIL_CLIENT_ID,
    // Google Desktop clients REQUIRE a client_secret at the token endpoint
    // (exchange + refresh). Injected at build time (env `K2_GMAIL_CLIENT_SECRET`);
    // not confidential (ships in the installed app) but handled like a token.
    client_secret_placeholder: Some(GMAIL_CLIENT_SECRET),
};

/// Microsoft 365 / Exchange Online (prd §9). Device-code is the default
/// (basic-auth IMAP is dead for M365). `offline_access` is REQUIRED to
/// receive a refresh_token; MS ROTATES the refresh_token on every
/// refresh, so callers must always persist the newest one.
pub static MICROSOFT: ProviderConfig = ProviderConfig {
    name: "microsoft",
    auth_url: Some("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"),
    device_auth_url: Some("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"),
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    // Microsoft has no simple standalone revoke endpoint for a single
    // token (revocation is tenant/consent management) — unlink wipes the
    // vault entry, which is the effective local revoke.
    revoke_url: None,
    // MCS1: the `mail` feature set ([`scope_for`] with mail only) —
    // identity (`openid profile email offline_access User.Read`, for the
    // account check and the refresh token) + Graph mail read/write and
    // send. `scope_for` is the source; a test pins them equal.
    scope: "openid profile email offline_access https://graph.microsoft.com/User.Read \
            https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send",
    extra_auth_params: &[],
    default_flow: FlowKind::DeviceCode,
    // Injected at build time (env `K2_MICROSOFT_CLIENT_ID`); `REPLACE_ME`
    // fallback. Azure AD app registration (public client, "allow public
    // client flows" = yes).
    client_id_placeholder: MICROSOFT_CLIENT_ID,
    // Microsoft device-code is a TRUE public client — NO client_secret on
    // any of its forms, ever.
    client_secret_placeholder: None,
};

/// Resolve the effective client_id. Order (BYO OAuth client, S1):
/// **explicit call override > stored BYO override > baked default**. An
/// explicit non-blank `override_id` always wins (the enterprise/per-call
/// seam); otherwise the owner's STORED client id ([`stored_client_id`]) is
/// used when set; otherwise the provider's placeholder const. Returns an
/// owned `String` because the stored value is read at runtime.
pub fn client_id(provider: OauthProvider, override_id: Option<&str>) -> String {
    if let Some(id) = override_id.map(str::trim).filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    if let Some(id) = stored_client_id(provider) {
        return id;
    }
    provider.config().client_id_placeholder.to_string()
}

/// Resolve the effective client_secret, mirroring [`client_id`]. Order:
/// **explicit call override > stored BYO override > baked default**.
/// `None` means "this provider sends NO client_secret" (Microsoft, a true
/// public client) — callers MUST omit the field from the token form when
/// this is `None` so Microsoft's request stays byte-for-byte secret-free.
/// Google returns `Some(secret)` (Desktop clients require it at the token
/// endpoint). The result is a token-grade value — NEVER log it.
pub fn client_secret(provider: OauthProvider, override_secret: Option<&str>) -> Option<String> {
    if let Some(secret) = override_secret.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(secret.to_string());
    }
    if let Some(secret) = stored_client_secret(provider) {
        return Some(secret);
    }
    provider.config().client_secret_placeholder.map(str::to_string)
}

// ── Stored BYO override seam (S1) ───────────────────────────────────────
//
// In production these read the owner's stored client from
// `mail::oauth_config` (app_settings id + vault secret). In the O1 unit
// tests they read a THREAD-LOCAL fake so the engine stays hermetic (no real
// fs) — the thread-local defaults to empty, so every existing engine test
// resolves to the baked default exactly as before, while the S1 resolution
// tests install values on their own test thread with no cross-test leakage.

#[cfg(not(test))]
fn stored_client_id(provider: OauthProvider) -> Option<String> {
    crate::mail::oauth_config::stored_client_id(provider)
}
#[cfg(not(test))]
fn stored_client_secret(provider: OauthProvider) -> Option<String> {
    crate::mail::oauth_config::stored_client_secret_default(provider)
}

#[cfg(test)]
fn stored_client_id(provider: OauthProvider) -> Option<String> {
    test_byo::client_id(provider)
}
#[cfg(test)]
fn stored_client_secret(provider: OauthProvider) -> Option<String> {
    test_byo::client_secret(provider)
}

/// Thread-local BYO override fake for the O1 engine tests. Per-test-thread
/// (Rust runs each `#[test]` on its own thread) so installing a value in one
/// resolution test never bleeds into a concurrent engine test.
#[cfg(test)]
pub(crate) mod test_byo {
    use super::OauthProvider;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static IDS: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());
        static SECRETS: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());
        static TENANT: RefCell<String> = const { RefCell::new(String::new()) };
    }

    /// MCS1: the thread-local Microsoft tenant fake (empty = `common`).
    pub fn set_tenant(t: &str) {
        TENANT.with(|v| *v.borrow_mut() = t.to_string());
    }
    pub fn tenant() -> String {
        TENANT.with(|v| v.borrow().clone())
    }

    pub fn set_client_id(p: OauthProvider, v: &str) {
        IDS.with(|m| m.borrow_mut().insert(p.as_str(), v.to_string()));
    }
    pub fn set_client_secret(p: OauthProvider, v: &str) {
        SECRETS.with(|m| m.borrow_mut().insert(p.as_str(), v.to_string()));
    }
    pub fn clear() {
        IDS.with(|m| m.borrow_mut().clear());
        SECRETS.with(|m| m.borrow_mut().clear());
        TENANT.with(|v| v.borrow_mut().clear());
    }
    pub fn client_id(p: OauthProvider) -> Option<String> {
        IDS.with(|m| m.borrow().get(p.as_str()).cloned())
    }
    pub fn client_secret(p: OauthProvider) -> Option<String> {
        SECRETS.with(|m| m.borrow().get(p.as_str()).cloned())
    }
}

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

// ─────────────────────────────────────────────────────────────────────
// Token model — Debug/error REDACTED (a token in a log = compromise)
// ─────────────────────────────────────────────────────────────────────

/// A token set as returned by a token endpoint. `refresh_token` is
/// optional: Google OMITS it on a refresh response (keep the old one);
/// Microsoft ROTATES it (a new one is present). `Debug` redacts both
/// secrets — never let a token reach a log line.
#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    pub token_type: String,
    /// Lifetime in seconds from issuance (used to compute an absolute
    /// `token_expires_at`).
    pub expires_in: i64,
    /// The OpenID Connect id_token from the token endpoint (MCS1 account
    /// check reads its claims). Never vaulted, never logged; `None` on a
    /// refresh and on bundles loaded from the vault.
    pub id_token: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("scope", &self.scope)
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// The freshest access token for an inbox + the (possibly refreshed)
/// absolute expiry the caller must persist to `token_expires_at`.
/// `access_token` is NOT `Debug`-derived on purpose (it's a bare
/// String field — callers must never log it).
pub struct FreshAccess {
    pub access_token: String,
    pub token_expires_at: i64,
    /// True if this call performed a refresh (so the caller knows to
    /// write `token_expires_at` back to the row).
    pub refreshed: bool,
    /// The granted scope string of the (possibly refreshed) bundle, when
    /// the provider reported one. Non-secret: the caller mirrors it into
    /// `granted_scopes` (MCS1 backfill for rows linked before 0188).
    pub scope: Option<String>,
}

impl std::fmt::Debug for FreshAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FreshAccess")
            .field("access_token", &"<redacted>")
            .field("token_expires_at", &self.token_expires_at)
            .field("refreshed", &self.refreshed)
            .field("scope", &self.scope)
            .finish()
    }
}

// ─────────────────────────────────────────────────────────────────────
// Errors — never carry a token or a raw response body
// ─────────────────────────────────────────────────────────────────────

/// Every failure the engine can surface. NONE of these variants carry a
/// token, a SASL string, or a raw response body — provider errors keep
/// only the standardized `error` code + human `error_description`
/// (which never contain a token). Parse failures deliberately DROP the
/// body (a token-endpoint body contains the token).
#[derive(Debug)]
pub enum OauthError {
    /// Transport failure reaching the endpoint (no token in the text).
    Http(String),
    /// A structured OAuth error response (`{"error":...}`).
    Provider { error: String, description: Option<String> },
    /// The response was not the JSON shape we expected. The raw body is
    /// intentionally NOT included (it may hold a token).
    Parse(String),
    /// Missing/omitted config (e.g. no device endpoint for a provider).
    Config(String),
    /// Vault read/write failure.
    Vault(String),
    /// `state` mismatch on a loopback redirect (CSRF guard).
    State,
}

impl std::fmt::Display for OauthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OauthError::Http(e) => write!(f, "oauth transport error: {e}"),
            OauthError::Provider { error, description } => match description {
                Some(d) => write!(f, "oauth provider error '{error}': {d}"),
                None => write!(f, "oauth provider error '{error}'"),
            },
            OauthError::Parse(e) => write!(f, "oauth parse error: {e}"),
            OauthError::Config(e) => write!(f, "oauth config error: {e}"),
            OauthError::Vault(e) => write!(f, "oauth vault error: {e}"),
            OauthError::State => write!(f, "oauth state mismatch (possible CSRF)"),
        }
    }
}

impl std::error::Error for OauthError {}

// ─────────────────────────────────────────────────────────────────────
// The injected HTTP seam (production: ReqwestHttp; tests: a mock)
// ─────────────────────────────────────────────────────────────────────

/// A raw HTTP reply. `body` may contain a TOKEN (a token-endpoint
/// success body), so this type is deliberately NOT `Debug` — never dump
/// it to a log.
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Every network effect the OAuth engine performs, behind ONE trait so
/// the whole module unit-tests with a canned mock — no real network,
/// ever (house rule). All OAuth token/device/revoke calls are
/// `application/x-www-form-urlencoded` POSTs; the token/device endpoints
/// return `{ "error": ... }` JSON with a 400 on the expected error
/// cases, so the seam returns status + body and the caller parses the
/// body regardless of status.
pub trait HttpClient: Send + Sync {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, OauthError>;

    /// One Bearer-authed JSON GET (MCS1 account check: Graph `/me`, the
    /// Gmail profile). The token rides only the Authorization header. The
    /// default refuses, so a test double that never expects a GET fails
    /// loudly when one happens.
    fn get_bearer(&self, url: &str, _bearer: &str) -> Result<HttpResponse, OauthError> {
        Err(OauthError::Config(format!("this HTTP client cannot GET {url}")))
    }
}

/// `application/x-www-form-urlencoded` value encoding: unreserved
/// (RFC 3986 `A-Za-z0-9-._~`) pass through, everything else percent-
/// encoded (space as `%20`, unambiguous for servers). Used for both
/// form bodies and loopback query strings.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `k1=v1&k2=v2` with every key and value `form_encode`d.
fn encode_form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Production HTTP client: blocking reqwest, form-urlencoded POSTs,
/// short timeout. Errors are REDACTED (transport failures carry no
/// token; we keep the message minimal regardless).
pub struct ReqwestHttp {
    timeout: std::time::Duration,
}

impl Default for ReqwestHttp {
    fn default() -> Self {
        Self { timeout: std::time::Duration::from_secs(30) }
    }
}

impl HttpClient for ReqwestHttp {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, OauthError> {
        let body = encode_form(form);
        let client = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| OauthError::Http(format!("http client build: {e}")))?;
        let resp = client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(body)
            .send()
            // reqwest's error carries the URL/host, never the body/token.
            .map_err(|e| OauthError::Http(format!("POST {url}: {e}")))?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(HttpResponse { status, body })
    }

    fn get_bearer(&self, url: &str, bearer: &str) -> Result<HttpResponse, OauthError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| OauthError::Http(format!("http client build: {e}")))?;
        let resp = client
            .get(url)
            .bearer_auth(bearer)
            .header("Accept", "application/json")
            .send()
            // reqwest's error carries the URL/host, never the header.
            .map_err(|e| OauthError::Http(format!("GET {url}: {e}")))?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(HttpResponse { status, body })
    }
}

// ─────────────────────────────────────────────────────────────────────
// Response parsing (bodies may hold tokens → NEVER echo them in errors)
// ─────────────────────────────────────────────────────────────────────

fn parse_json(status: u16, body: &str) -> Result<serde_json::Value, OauthError> {
    serde_json::from_str(body)
        // Deliberately drop `body` — it may contain a token.
        .map_err(|_| OauthError::Parse(format!("non-JSON response (HTTP {status})")))
}

/// A standardized `{"error": "...", "error_description": "..."}` reply,
/// if present. The `error`/`error_description` fields are safe (no
/// token); the rest of the body is ignored.
fn provider_error(v: &serde_json::Value) -> Option<OauthError> {
    let error = v.get("error").and_then(|e| e.as_str())?;
    Some(OauthError::Provider {
        error: error.to_string(),
        description: v
            .get("error_description")
            .and_then(|d| d.as_str())
            .map(str::to_string),
    })
}

/// Number that a provider may encode as JSON number OR string
/// (`expires_in` is number for Google/MS, but be liberal).
fn as_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    let f = v.get(key)?;
    f.as_i64().or_else(|| f.as_str().and_then(|s| s.parse().ok()))
}

/// Extract a [`Tokens`] from a SUCCESS token response value. Missing
/// `access_token` is a parse error (never echo the body).
fn tokens_from_value(v: &serde_json::Value) -> Result<Tokens, OauthError> {
    let access_token = v
        .get("access_token")
        .and_then(|s| s.as_str())
        .ok_or_else(|| OauthError::Parse("token response missing access_token".to_string()))?
        .to_string();
    Ok(Tokens {
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        scope: v.get("scope").and_then(|s| s.as_str()).map(str::to_string),
        token_type: v
            .get("token_type")
            .and_then(|s| s.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        // A token with no lifetime is treated as already-expiring so the
        // next use refreshes rather than trusting it forever.
        expires_in: as_i64(v, "expires_in").unwrap_or(0),
        id_token: v.get("id_token").and_then(|s| s.as_str()).map(str::to_string),
    })
}

/// Parse a token endpoint reply that is expected to be a success:
/// surface a `{"error":...}` as [`OauthError::Provider`], else Tokens.
fn tokens_or_error(resp: &HttpResponse) -> Result<Tokens, OauthError> {
    let v = parse_json(resp.status, &resp.body)?;
    if let Some(e) = provider_error(&v) {
        return Err(e);
    }
    tokens_from_value(&v)
}

// ─────────────────────────────────────────────────────────────────────
// Flow A — device-code (RFC 8628)
// ─────────────────────────────────────────────────────────────────────

/// Step 1 of the device flow: what the daemon shows the user.
#[derive(Clone)]
pub struct DeviceStart {
    /// The daemon's polling handle. Secret-ish (short-lived) — O2 vaults
    /// it with the pending link; redacted here in `Debug`.
    pub device_code: String,
    /// The short code the USER types into the browser.
    pub user_code: String,
    /// The URL the user opens to enter `user_code`.
    pub verification_uri: String,
    /// Seconds until `device_code` expires (~15 min typical).
    pub expires_in: i64,
    /// Minimum seconds between polls (honor + back off on slow_down).
    pub interval: i64,
}

impl std::fmt::Debug for DeviceStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceStart")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// Begin the device-code flow: POST the device-authorization endpoint
/// with `client_id` + `scope`; parse the code/URL/interval.
pub fn device_start(
    provider: OauthProvider,
    client_id_override: Option<&str>,
    http: &dyn HttpClient,
) -> Result<DeviceStart, OauthError> {
    device_start_scoped(provider, client_id_override, provider.config().scope, http)
}

/// [`device_start`] with an explicit scope string (MCS1: the scope set is
/// a function of the features asked for, [`scope_for`]).
pub fn device_start_scoped(
    provider: OauthProvider,
    client_id_override: Option<&str>,
    scope: &str,
    http: &dyn HttpClient,
) -> Result<DeviceStart, OauthError> {
    let cfg = provider.config();
    let url = device_auth_url(provider).ok_or_else(|| {
        OauthError::Config(format!("{} has no device-authorization endpoint", cfg.name))
    })?;
    let cid = client_id(provider, client_id_override);
    let resp = http.post_form(&url, &[("client_id", cid.as_str()), ("scope", scope)])?;
    let v = parse_json(resp.status, &resp.body)?;
    if let Some(e) = provider_error(&v) {
        return Err(e);
    }
    let device_code = v
        .get("device_code")
        .and_then(|s| s.as_str())
        .ok_or_else(|| OauthError::Parse("device response missing device_code".to_string()))?
        .to_string();
    let user_code = v
        .get("user_code")
        .and_then(|s| s.as_str())
        .ok_or_else(|| OauthError::Parse("device response missing user_code".to_string()))?
        .to_string();
    // Google uses `verification_url`; Microsoft/RFC 8628 use
    // `verification_uri` — accept either.
    let verification_uri = v
        .get("verification_uri")
        .or_else(|| v.get("verification_url"))
        .and_then(|s| s.as_str())
        .ok_or_else(|| OauthError::Parse("device response missing verification_uri".to_string()))?
        .to_string();
    Ok(DeviceStart {
        device_code,
        user_code,
        verification_uri,
        expires_in: as_i64(&v, "expires_in").unwrap_or(900),
        interval: as_i64(&v, "interval").unwrap_or(5),
    })
}

/// The result of one device-code poll (RFC 8628 §3.5). `Debug`-safe:
/// the `Ok` variant's `Tokens` redacts its own secrets.
#[derive(Debug)]
pub enum Poll {
    /// `authorization_pending` — keep polling at `interval`.
    Pending,
    /// `slow_down` — increase the interval by 5s, then keep polling.
    SlowDown,
    /// `access_denied` — the user refused.
    Denied,
    /// `expired_token` — the `device_code` timed out; start over.
    Expired,
    /// Success — tokens are ready.
    Ok(Tokens),
}

/// Poll the token endpoint once with the device grant. Maps the four
/// standard error codes to [`Poll`] variants; any other `error` is a
/// hard [`OauthError::Provider`].
pub fn device_poll(
    provider: OauthProvider,
    device_code: &str,
    client_id_override: Option<&str>,
    http: &dyn HttpClient,
) -> Result<Poll, OauthError> {
    let cid = client_id(provider, client_id_override);
    let resp = http.post_form(
        &token_url(provider),
        &[
            ("client_id", cid.as_str()),
            ("device_code", device_code),
            ("grant_type", DEVICE_GRANT),
        ],
    )?;
    let v = parse_json(resp.status, &resp.body)?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Ok(match err {
            "authorization_pending" => Poll::Pending,
            "slow_down" => Poll::SlowDown,
            "access_denied" => Poll::Denied,
            "expired_token" => Poll::Expired,
            other => {
                return Err(OauthError::Provider {
                    error: other.to_string(),
                    description: v
                        .get("error_description")
                        .and_then(|d| d.as_str())
                        .map(str::to_string),
                })
            }
        });
    }
    Ok(Poll::Ok(tokens_from_value(&v)?))
}

// ─────────────────────────────────────────────────────────────────────
// Flow B — auth-code + loopback (RFC 8252) + PKCE (RFC 7636, M2)
// ─────────────────────────────────────────────────────────────────────

/// A PKCE (RFC 7636) `code_verifier` + `code_challenge` pair for ONE
/// loopback flow (M2 — closes the local token-injection vector).
///
/// The `verifier` is a per-flow SECRET the daemon keeps to itself and only
/// reveals on the token exchange; the `challenge` = `BASE64URL-NOPAD(
/// SHA256(verifier))` is public and rides on the consent URL. Because the
/// authorization server binds the issued code to *this* challenge, an
/// attacker who races the loopback with a code minted in THEIR own session
/// (a different, unknown verifier) fails the exchange (`invalid_grant`) —
/// even if they stole the genuine `state` off the browser-launch argv.
///
/// `Debug` REDACTS the verifier (it is token-grade: whoever holds it
/// defeats PKCE). The challenge is public knowledge, so it prints.
#[derive(Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

impl std::fmt::Debug for PkcePair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PkcePair")
            // The verifier is a per-flow secret — never let it reach a log.
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// URL-safe, no-pad Base64 (RFC 4648 §5) — the encoding both PKCE
/// (`code_challenge`) and the verifier itself use so every byte lands in
/// the RFC 7636 unreserved set.
fn base64_url_nopad(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `code_challenge = BASE64URL-NOPAD(SHA256(code_verifier))` — the RFC 7636
/// **S256** transform. Pure; no secret is retained.
pub fn pkce_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    base64_url_nopad(&hasher.finalize())
}

/// Mint a fresh per-flow [`PkcePair`]. The verifier is 32 bytes of OS
/// randomness rendered as 43-char URL-safe Base64 — inside RFC 7636's
/// 43–128 unreserved-char range, and the same getrandom-for-secrets idiom
/// the crate uses for other high-entropy secrets (`secrets::generate_
/// secret`). Fails LOUD if the OS RNG is unavailable rather than mint a
/// weak, guessable verifier (which would silently defeat the M2 fix).
pub fn generate_pkce() -> Result<PkcePair, OauthError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| OauthError::Config(format!("os rng unavailable for PKCE: {e}")))?;
    let verifier = base64_url_nopad(&bytes);
    let challenge = pkce_challenge(&verifier);
    Ok(PkcePair { verifier, challenge })
}

/// Build the system-browser authorization URL for the loopback flow
/// (PURE — no network). `redirect_uri` is O2's `http://127.0.0.1:<port>`
/// callback; `state` is O2's CSRF nonce (validated on the redirect with
/// [`validate_state`]); `code_challenge` is the PKCE S256 challenge (M2),
/// the public half of a per-flow [`PkcePair`].
///
/// NOTE (M2): the finished URL is handed to the SYSTEM browser via
/// `open`/`xdg-open`, so it (with `state` + `client_id`) is inherently
/// visible on the process argv to any local user — that leak is inherent
/// to launching a browser and is NOT something we can remove. PKCE is what
/// neutralizes it: a stolen `state` alone can no longer inject a code,
/// because the attacker cannot produce the matching `code_verifier`.
pub fn loopback_build_url(
    provider: OauthProvider,
    client_id_override: Option<&str>,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String, OauthError> {
    loopback_build_url_scoped(
        provider,
        client_id_override,
        redirect_uri,
        state,
        code_challenge,
        provider.config().scope,
        &[],
    )
}

/// [`loopback_build_url`] with an explicit scope string and extra
/// query params (MCS1: `login_hint`, and `include_granted_scopes=true`
/// for an incremental re-consent). `extra` never carries a secret.
pub fn loopback_build_url_scoped(
    provider: OauthProvider,
    client_id_override: Option<&str>,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    scope: &str,
    extra: &[(&str, &str)],
) -> Result<String, OauthError> {
    let cfg = provider.config();
    let auth = auth_url(provider)
        .ok_or_else(|| OauthError::Config(format!("{} has no authorization endpoint", cfg.name)))?;
    let cid = client_id(provider, client_id_override);
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id", cid.as_str()),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", scope),
        ("state", state),
        // PKCE S256 (M2): binds the issued code to our per-flow verifier.
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
    ];
    params.extend(cfg.extra_auth_params.iter().copied());
    params.extend(extra.iter().copied());
    Ok(format!("{auth}?{}", encode_form(&params)))
}

/// Constant-time `state` check (CSRF guard). Empty expected/actual never
/// matches. O2's loopback listener calls this on every redirect and only a
/// state-VALID callback proceeds to the exchange (M2). The comparison is
/// constant-time in the byte content (L3) so a local attacker cannot use
/// response timing to recover the genuine `state` byte-by-byte.
pub fn validate_state(expected: &str, got: &str) -> Result<(), OauthError> {
    if !expected.is_empty() && constant_time_eq(expected.as_bytes(), got.as_bytes()) {
        Ok(())
    } else {
        Err(OauthError::State)
    }
}

/// Constant-time byte-slice equality. Length inequality returns early
/// (lengths are not secret here — the `state` length is fixed), but for
/// equal lengths every byte is always compared, so a matching prefix does
/// not leak via a shorter runtime.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Exchange a loopback authorization `code` for tokens (`grant_type=
/// authorization_code`). `redirect_uri` MUST match the one in
/// `loopback_build_url`. `code_verifier` is the SAME per-flow PKCE secret
/// whose challenge went out on the auth URL (M2) — Google (and any RFC 7636
/// server) rejects the exchange with `invalid_grant` if it is absent or
/// does not hash to the code's bound challenge. It is never logged.
pub fn loopback_exchange(
    provider: OauthProvider,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    client_id_override: Option<&str>,
    client_secret_override: Option<&str>,
    http: &dyn HttpClient,
) -> Result<Tokens, OauthError> {
    let cid = client_id(provider, client_id_override);
    // Resolved once, hoisted so the borrow outlives `form` (it is a
    // token-grade owned String; never logged). `None` for Microsoft.
    let secret = client_secret(provider, client_secret_override);
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", cid.as_str()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
        // PKCE proof (M2): the verifier for this flow's challenge. Google
        // Desktop clients send BOTH this AND the client_secret below.
        ("code_verifier", code_verifier),
    ];
    // Google Desktop clients require a `client_secret` here or the token
    // endpoint answers `invalid_client`. Microsoft resolves to `None` →
    // the field is OMITTED, keeping its public-client form unchanged.
    if let Some(secret) = secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let resp = http.post_form(&token_url(provider), &form)?;
    tokens_or_error(&resp)
}

/// Bind an ephemeral 127.0.0.1 listener for the loopback redirect and
/// return it with its bound address. O2 owns the accept/parse loop; O1
/// only provides the bind so the port is chosen the same way in tests +
/// prod. Loopback-only (never 0.0.0.0) — the redirect must never be
/// reachable off-box.
pub fn bind_ephemeral_loopback() -> Result<(std::net::TcpListener, std::net::SocketAddr), String> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("bind loopback: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("loopback local_addr: {e}"))?;
    Ok((listener, addr))
}

// ─────────────────────────────────────────────────────────────────────
// Refresh + revoke (prd §6)
// ─────────────────────────────────────────────────────────────────────

/// Refresh an access token (`grant_type=refresh_token`). The returned
/// [`Tokens`] carries a NEW `refresh_token` ONLY when the provider
/// rotated it (Microsoft ALWAYS; Google NEVER — it omits the field).
/// The keep-old-vs-take-new decision is made by [`access_token_for`].
pub fn refresh(
    provider: OauthProvider,
    refresh_token: &str,
    client_id_override: Option<&str>,
    client_secret_override: Option<&str>,
    http: &dyn HttpClient,
) -> Result<Tokens, OauthError> {
    let cid = client_id(provider, client_id_override);
    // Hoisted so the owned secret String outlives `form` (never logged).
    let secret = client_secret(provider, client_secret_override);
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", cid.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    // Google requires the `client_secret` on refresh too (same
    // `invalid_client` failure without it). Microsoft → `None` → OMITTED,
    // so its refresh form is unchanged.
    if let Some(secret) = secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let resp = http.post_form(&token_url(provider), &form)?;
    tokens_or_error(&resp)
}

/// Best-effort revoke (unlink). A provider with no revoke endpoint
/// (Microsoft) is a no-op success — the caller still wipes the vault
/// entry, which is the effective local revoke. A transport/HTTP failure
/// is swallowed to `Ok(())` (best-effort: never block an unlink), but a
/// misconfiguration bug still surfaces via the caller's logs.
pub fn revoke(
    provider: OauthProvider,
    token: &str,
    http: &dyn HttpClient,
) -> Result<(), OauthError> {
    let Some(url) = revoke_url(provider) else {
        return Ok(());
    };
    // Google's revoke takes the token as `token=<value>`. Best-effort:
    // any reply (2xx or not) is acceptable; a transport error is
    // swallowed so unlink always proceeds.
    match http.post_form(&url, &[("token", token)]) {
        Ok(_) => Ok(()),
        Err(_) => Ok(()),
    }
}

/// What a revoke at the provider did (MCS1 / MCS-A11: unlink reports it,
/// and a failure never blocks the unlink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// The provider accepted the revoke (or the token was already invalid).
    Revoked,
    /// The provider has no single-token revoke (Microsoft).
    Unsupported,
    /// The revoke call failed; the reason carries no token.
    Failed(String),
}

/// Revoke `token` at the provider and REPORT the outcome (unlike
/// [`revoke`], which swallows it). Google answers 200 on success and 400
/// `invalid_token` when the token is already dead — both mean nothing is
/// left to revoke. The token never rides the returned reason.
pub fn revoke_report(provider: OauthProvider, token: &str, http: &dyn HttpClient) -> RevokeOutcome {
    let Some(url) = revoke_url(provider) else {
        return RevokeOutcome::Unsupported;
    };
    match http.post_form(&url, &[("token", token)]) {
        Ok(resp) if (200..300).contains(&resp.status) => RevokeOutcome::Revoked,
        Ok(resp) => {
            let code = serde_json::from_str::<serde_json::Value>(&resp.body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
            match code.as_deref() {
                Some("invalid_token") => RevokeOutcome::Revoked,
                Some(c) => RevokeOutcome::Failed(format!("HTTP {}: {c}", resp.status)),
                None => RevokeOutcome::Failed(format!("HTTP {}", resp.status)),
            }
        }
        Err(e) => RevokeOutcome::Failed(e.to_string()),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Vault glue (prd §4/§6) — key `ext-inbox-<row-id>-oauth`, JSON
// ─────────────────────────────────────────────────────────────────────

/// The deterministic vault key for a row's OAuth token bundle. Distinct
/// suffix from the app-password key (`ext-inbox-<id>`) so the two never
/// collide.
pub fn oauth_vault_key(row_id: &str) -> String {
    format!("ext-inbox-{row_id}-oauth")
}

/// Serialize a token set to the vaulted JSON shape (prd §4:
/// `{access_token, refresh_token, scope, token_type}`) plus a redundant
/// absolute `token_expires_at` copy (M1). The row remains the source of
/// truth for freshness; this vaulted copy exists ONLY so a single-flight
/// follower (see [`access_token_for`]) can hand back the exact expiry a
/// sibling just computed without re-reading the DB. A timestamp is not a
/// secret, so it is safe to co-locate with the token bundle.
fn tokens_to_json(t: &Tokens, expires_at: i64) -> String {
    // Manual serde_json::Value build (no derive on the secret struct).
    serde_json::json!({
        "access_token": t.access_token,
        "refresh_token": t.refresh_token,
        "scope": t.scope,
        "token_type": t.token_type,
        "token_expires_at": expires_at,
    })
    .to_string()
}

/// Store a token bundle in the vault under [`oauth_vault_key`] and
/// return the absolute `token_expires_at` (= `now + expires_in`) the
/// caller writes to the row. `now` is injected (no clock in the engine).
pub fn store_tokens(
    secrets: &dyn SecretStore,
    row_id: &str,
    tokens: &Tokens,
    now: i64,
) -> Result<i64, OauthError> {
    let expires_at = now.saturating_add(tokens.expires_in);
    secrets
        .store_exact(&oauth_vault_key(row_id), &tokens_to_json(tokens, expires_at))
        .map_err(OauthError::Vault)?;
    Ok(expires_at)
}

/// Load a token bundle from the vault. A missing entry is an
/// [`OauthError::Vault`] (an oauth inbox with no vaulted tokens is a
/// broken row — fail loud, never silently treat as unauthenticated).
pub fn load_tokens(secrets: &dyn SecretStore, row_id: &str) -> Result<Tokens, OauthError> {
    Ok(load_tokens_with_expiry(secrets, row_id)?.0)
}

/// Like [`load_tokens`] but also returns the redundant vaulted absolute
/// `token_expires_at` (M1), if present. `None` = a bundle written before
/// M1 (or by a hand-crafted vault entry) with no expiry copy → the
/// single-flight follower path treats that as "unknown" and falls back.
/// Internal: only [`access_token_for`]'s double-check needs the expiry.
fn load_tokens_with_expiry(
    secrets: &dyn SecretStore,
    row_id: &str,
) -> Result<(Tokens, Option<i64>), OauthError> {
    let raw = secrets
        .resolve(&oauth_vault_key(row_id))
        .map_err(OauthError::Vault)?
        .ok_or_else(|| OauthError::Vault(format!("no vaulted oauth tokens for inbox {row_id}")))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        // Never echo `raw` — it holds the tokens.
        .map_err(|_| OauthError::Parse("vaulted oauth tokens are not valid JSON".to_string()))?;
    let access_token = v
        .get("access_token")
        .and_then(|s| s.as_str())
        .ok_or_else(|| OauthError::Parse("vaulted oauth tokens missing access_token".to_string()))?
        .to_string();
    let tokens = Tokens {
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        scope: v.get("scope").and_then(|s| s.as_str()).map(str::to_string),
        token_type: v
            .get("token_type")
            .and_then(|s| s.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        // The vault stores no lifetime; the row's `token_expires_at` is
        // the source of truth. Treat a loaded token as expiry-unknown.
        expires_in: 0,
        id_token: None,
    };
    let vaulted_expiry = as_i64(&v, "token_expires_at");
    Ok((tokens, vaulted_expiry))
}

// ─────────────────────────────────────────────────────────────────────
// M1 — per-inbox single-flight around the refresh cycle
// ─────────────────────────────────────────────────────────────────────

/// Per-inbox single-flight locks (M1), keyed by `row_id`.
///
/// The refresh cycle — load token → (if stale) POST `refresh_token` →
/// re-vault — is a read-modify-write with no lock spanning it in the
/// original code. Two concurrent operations on the SAME inbox both loaded
/// refresh token `RT1` and both POSTed `grant_type=refresh_token`. Since
/// Microsoft ROTATES the refresh token (`RT1`→`RT2`, invalidating `RT1`),
/// the racing second writer could persist a token the provider has
/// ALREADY revoked, bricking the inbox (manual re-link). This lock
/// serializes the refresh cycle per inbox so exactly ONE refresh happens
/// and every other caller reuses the freshly-vaulted result.
///
/// The outer map mutex is held only long enough to clone out the per-row
/// `Arc<Mutex<()>>` — NEVER across the network refresh. Every refresher
/// of the same inbox converges on the SAME inner mutex; different
/// `row_id`s get different mutexes and never serialize against each
/// other. Entries are tiny; we never reap them. `parking_lot` mutexes do
/// NOT poison, and the guard is RAII (released on every return path), so
/// a panic or early-return can never leave an inbox wedged. Mirrors the
/// keyed-lock pattern in `workspace_msg.rs` (INJECT_LOCKS / WAKE_LOCKS).
static REFRESH_LOCKS: LazyLock<PlMutex<HashMap<String, Arc<PlMutex<()>>>>> =
    LazyLock::new(|| PlMutex::new(HashMap::new()));

/// Clone out the single-flight refresh lock for `row_id`, creating it on
/// first use. The map lock is short-lived — held only for the insert/clone.
fn refresh_lock_for(row_id: &str) -> Arc<PlMutex<()>> {
    let mut map = REFRESH_LOCKS.lock();
    map.entry(row_id.to_string())
        .or_insert_with(|| Arc::new(PlMutex::new(())))
        .clone()
}

/// The refresh-if-stale entry point (prd §6). Given the row's
/// `token_expires_at` (the only non-secret expiry bit), return a fresh
/// access token — refreshing ONLY if we're within 60s of expiry (or
/// expiry is unknown/`None`). On refresh: keep the OLD refresh_token if
/// the provider omitted a new one (Google), else take the NEW one
/// (Microsoft rotation), re-vault the bundle, and hand back the new
/// absolute expiry for the caller to persist. `now` is injected.
///
/// **M1 — single-flight.** The refresh cycle is serialized PER INBOX (see
/// [`REFRESH_LOCKS`]). The still-valid fast path takes NO lock (the common
/// case has zero contention). Only when a refresh is needed do we acquire
/// the per-inbox lock and DOUBLE-CHECK: re-load from the vault under the
/// lock; if the vaulted access token changed since our pre-lock read, a
/// sibling just refreshed — reuse its result and skip the network. This
/// makes it impossible for two callers to both POST `RT1` and race a
/// rotated-token write.
pub fn access_token_for(
    secrets: &dyn SecretStore,
    row_id: &str,
    provider: OauthProvider,
    token_expires_at: Option<i64>,
    now: i64,
    client_id_override: Option<&str>,
    client_secret_override: Option<&str>,
    http: &dyn HttpClient,
) -> Result<FreshAccess, OauthError> {
    const SKEW: i64 = 60; // refresh 60s early (prd §10 clock margin)
    let stored = load_tokens(secrets, row_id)?;

    // Fast path — NO lock. Fresh enough per the row's persisted expiry?
    // Reuse the vaulted access token untouched. The common case never
    // contends on the single-flight lock.
    if let Some(exp) = token_expires_at {
        if now < exp - SKEW {
            return Ok(FreshAccess {
                access_token: stored.access_token,
                token_expires_at: exp,
                refreshed: false,
                scope: stored.scope,
            });
        }
    }

    // A refresh is needed → serialize per inbox (M1 single-flight). The
    // RAII guard releases on EVERY return path below; parking_lot never
    // poisons, so an early return / panic can't wedge the inbox.
    let lock = refresh_lock_for(row_id);
    let _guard = lock.lock();

    // Double-check under the lock: another thread may have refreshed while
    // we blocked. Re-load from the vault; if the access token changed
    // since our pre-lock read, a sibling just rotated + re-vaulted — reuse
    // its freshly-vaulted token and its exact expiry, skipping the network
    // entirely (so we never POST a now-revoked refresh token).
    let (current, vaulted_expiry) = load_tokens_with_expiry(secrets, row_id)?;
    if current.access_token != stored.access_token {
        return Ok(FreshAccess {
            access_token: current.access_token,
            // A sibling that refreshed always wrote the vaulted expiry;
            // fall back to the caller's expiry only defensively.
            token_expires_at: vaulted_expiry.or(token_expires_at).unwrap_or(now),
            refreshed: true,
            scope: current.scope,
        });
    }

    // Still ours to refresh. A missing refresh_token is fatal (the inbox
    // must be re-linked) — never keep serving a stale/absent token silently.
    let refresh_token = current.refresh_token.clone().ok_or_else(|| {
        OauthError::Vault(format!(
            "inbox {row_id} has no refresh_token — it must be re-linked"
        ))
    })?;
    let fresh = refresh(
        provider,
        &refresh_token,
        client_id_override,
        client_secret_override,
        http,
    )?;

    // Google KEEPS the refresh_token (omits it on refresh); Microsoft
    // ROTATES it (returns a new one). Persist the newest either way.
    let merged = Tokens {
        access_token: fresh.access_token,
        refresh_token: fresh.refresh_token.or(Some(refresh_token)),
        scope: fresh.scope.or(current.scope),
        token_type: fresh.token_type,
        expires_in: fresh.expires_in,
        id_token: None,
    };
    let new_expiry = store_tokens(secrets, row_id, &merged, now)?;
    Ok(FreshAccess {
        access_token: merged.access_token,
        token_expires_at: new_expiry,
        refreshed: true,
        scope: merged.scope,
    })
    // `_guard` drops here → the per-inbox lock is released.
}

// ─────────────────────────────────────────────────────────────────────
// MCS1 — endpoints (tenant + test fakes), scope sets per feature, the
// minting client, the account check, link-error teaching
// ─────────────────────────────────────────────────────────────────────

/// The base URL of the provider fakes (`tests/fakes/oauth-providers`),
/// compiled ONLY into a `provider-fakes` build (PRD §8.2 / §9.10): a
/// release build has no override, so nothing that can set env can
/// redirect tokens. `K2_PROVIDER_FAKES_URL=http://127.0.0.1:<port>`.
fn fake_base() -> Option<String> {
    #[cfg(feature = "provider-fakes")]
    {
        std::env::var("K2_PROVIDER_FAKES_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(feature = "provider-fakes"))]
    {
        None
    }
}

/// Microsoft's consumer (personal account) tenant id: a `tid` equal to it
/// marks an outlook.com / hotmail / live account.
pub const MS_CONSUMER_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";

/// Validate + normalize a Microsoft authority (`mailOauthMicrosoftTenant`).
/// Empty → `common`. Accepts `common`, `organizations`, `consumers`, a
/// tenant GUID, or a tenant domain (`contoso.onmicrosoft.com`). Anything
/// else is a usage error naming the accepted shapes.
pub fn validate_tenant(raw: &str) -> Result<String, String> {
    let t = raw.trim().to_ascii_lowercase();
    if t.is_empty() {
        return Ok("common".to_string());
    }
    if matches!(t.as_str(), "common" | "organizations" | "consumers") {
        return Ok(t);
    }
    let is_guid = {
        let parts: Vec<&str> = t.split('-').collect();
        parts.len() == 5
            && [8usize, 4, 4, 4, 12]
                .iter()
                .zip(parts.iter())
                .all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
    };
    let is_domain = t.len() <= 253
        && t.contains('.')
        && !t.starts_with('.')
        && !t.ends_with('.')
        && !t.contains("..")
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if is_guid || is_domain {
        Ok(t)
    } else {
        Err(format!(
            "invalid Microsoft tenant '{}' — use common, organizations, consumers, a tenant id \
             (GUID) or a tenant domain such as contoso.onmicrosoft.com",
            raw.trim()
        ))
    }
}

/// The box's Microsoft authority (the `mailOauthMicrosoftTenant` setting;
/// `common` when unset or unreadable).
pub fn microsoft_tenant() -> String {
    #[cfg(not(test))]
    let raw = k2_core::app_settings::load().mail_oauth_microsoft_tenant;
    #[cfg(test)]
    let raw = test_byo::tenant();
    validate_tenant(&raw).unwrap_or_else(|_| "common".to_string())
}

fn ms_login_base() -> String {
    match fake_base() {
        Some(b) => format!("{b}/ms/login"),
        None => "https://login.microsoftonline.com".to_string(),
    }
}

/// Authorization endpoint (loopback flow).
pub fn auth_url(provider: OauthProvider) -> Option<String> {
    match provider {
        OauthProvider::Gmail => Some(match fake_base() {
            Some(b) => format!("{b}/google/auth"),
            None => GMAIL.auth_url?.to_string(),
        }),
        OauthProvider::Microsoft => Some(format!(
            "{}/{}/oauth2/v2.0/authorize",
            ms_login_base(),
            microsoft_tenant()
        )),
    }
}

/// Device-authorization endpoint (device flow).
pub fn device_auth_url(provider: OauthProvider) -> Option<String> {
    match provider {
        OauthProvider::Gmail => Some(match fake_base() {
            Some(b) => format!("{b}/google/device/code"),
            None => GMAIL.device_auth_url?.to_string(),
        }),
        OauthProvider::Microsoft => Some(format!(
            "{}/{}/oauth2/v2.0/devicecode",
            ms_login_base(),
            microsoft_tenant()
        )),
    }
}

/// Token endpoint (device poll, loopback exchange, refresh).
pub fn token_url(provider: OauthProvider) -> String {
    match provider {
        OauthProvider::Gmail => match fake_base() {
            Some(b) => format!("{b}/google/token"),
            None => GMAIL.token_url.to_string(),
        },
        OauthProvider::Microsoft => {
            format!("{}/{}/oauth2/v2.0/token", ms_login_base(), microsoft_tenant())
        }
    }
}

/// Revoke endpoint (`None` = the provider has no single-token revoke).
pub fn revoke_url(provider: OauthProvider) -> Option<String> {
    match provider {
        OauthProvider::Gmail => Some(match fake_base() {
            Some(b) => format!("{b}/google/revoke"),
            None => GMAIL.revoke_url?.to_string(),
        }),
        OauthProvider::Microsoft => None,
    }
}

/// The Microsoft Graph v1.0 base (the Graph mail backend and the `/me`
/// account check).
pub fn graph_base() -> String {
    match fake_base() {
        Some(b) => format!("{b}/ms/graph/v1.0"),
        None => "https://graph.microsoft.com/v1.0".to_string(),
    }
}

/// The Google Calendar API v3 base (MCS4 calendars on linked Google
/// inboxes). A `provider-fakes` build may point it at the fakes.
pub fn google_calendar_base() -> String {
    match fake_base() {
        Some(b) => format!("{b}/google/calendar/v3"),
        None => "https://www.googleapis.com/calendar/v3".to_string(),
    }
}

/// The Gmail profile endpoint (account check fallback; the
/// `https://mail.google.com/` scope covers it, MCS-A10).
pub fn gmail_profile_url() -> String {
    match fake_base() {
        Some(b) => format!("{b}/google/gmail/v1/users/me/profile"),
        None => "https://gmail.googleapis.com/gmail/v1/users/me/profile".to_string(),
    }
}

/// The admin-consent URL a tenant admin opens to approve the app for the
/// whole tenant (`admin_consent_required`). A specific configured tenant
/// is used as-is; `common`/`consumers` fall back to `organizations`.
pub fn admin_consent_url(client_id: &str) -> String {
    let t = microsoft_tenant();
    let authority = match t.as_str() {
        "common" | "consumers" => "organizations".to_string(),
        other => other.to_string(),
    };
    format!(
        "https://login.microsoftonline.com/{authority}/adminconsent?client_id={}",
        form_encode(client_id)
    )
}

// ── Scope sets per feature (PRD §3.2) ────────────────────────────────────

/// What a link is consented for. A link always carries the identity
/// scopes; `mail` and `calendar` add their sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features {
    pub mail: bool,
    pub calendar: bool,
}

impl Features {
    pub const MAIL: Features = Features { mail: true, calendar: false };

    /// Parse `mail`, `calendar`, `mail,calendar` (commas or spaces, any
    /// order, case-insensitive). Empty → `None` (the caller picks the
    /// default). Unknown words are a usage error.
    pub fn parse(raw: &str) -> Result<Option<Features>, String> {
        let mut f = Features::default();
        let mut any = false;
        for w in raw.split(|c: char| c == ',' || c.is_whitespace()).filter(|w| !w.is_empty()) {
            match w.to_ascii_lowercase().as_str() {
                "mail" => f.mail = true,
                "calendar" | "calendars" => f.calendar = true,
                other => {
                    return Err(format!(
                        "unknown feature '{other}' — use mail, calendar or mail,calendar"
                    ))
                }
            }
            any = true;
        }
        Ok(if any { Some(f) } else { None })
    }

    /// The stored spelling (`features` column): `mail`, `calendar` or
    /// `mail,calendar`.
    pub fn as_db(self) -> String {
        match (self.mail, self.calendar) {
            (true, true) => "mail,calendar".to_string(),
            (false, true) => "calendar".to_string(),
            _ => "mail".to_string(),
        }
    }

    pub fn union(self, other: Features) -> Features {
        Features { mail: self.mail || other.mail, calendar: self.calendar || other.calendar }
    }
}

const GOOGLE_IDENTITY: &[&str] = &["openid", "email"];
const GOOGLE_MAIL: &[&str] = &["https://mail.google.com/"];
/// Sensitive (not restricted) Calendar API scopes, checked against
/// Google's scope list for MCS4 (MCS-A9, 2026-10-17): `calendar.events`
/// (events list/get/insert/patch/delete + `syncToken`),
/// `calendar.calendarlist.readonly` (`calendarList.list`) and
/// `calendar.freebusy` (`freeBusy.query`). Only `calendar.events` is
/// required: a person who unticks the other two still gets a working
/// calendar (list falls back to the primary calendar, free/busy to the
/// event list).
const GOOGLE_CALENDAR: &[&str] = &[
    "https://www.googleapis.com/auth/calendar.events",
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "https://www.googleapis.com/auth/calendar.freebusy",
];
const MS_IDENTITY: &[&str] =
    &["openid", "profile", "email", "offline_access", "https://graph.microsoft.com/User.Read"];
const MS_MAIL: &[&str] =
    &["https://graph.microsoft.com/Mail.ReadWrite", "https://graph.microsoft.com/Mail.Send"];
const MS_CALENDAR: &[&str] = &["https://graph.microsoft.com/Calendars.ReadWrite"];

/// The space-separated scope string for `features` (PRD §3.2 table).
/// Both providers return ONE refresh token for the whole set, so a row
/// keeps one vaulted bundle whatever it was consented for.
pub fn scope_for(provider: OauthProvider, features: Features) -> String {
    let (identity, mail, calendar) = match provider {
        OauthProvider::Gmail => (GOOGLE_IDENTITY, GOOGLE_MAIL, GOOGLE_CALENDAR),
        OauthProvider::Microsoft => (MS_IDENTITY, MS_MAIL, MS_CALENDAR),
    };
    let mut out: Vec<&str> = identity.to_vec();
    if features.mail {
        out.extend_from_slice(mail);
    }
    if features.calendar {
        out.extend_from_slice(calendar);
    }
    out.join(" ")
}

/// Does a granted scope string carry `want`? Microsoft answers either the
/// bare (`Mail.Send`) or the resource-qualified
/// (`https://graph.microsoft.com/Mail.Send`) spelling; both match,
/// case-insensitively.
pub fn scope_granted(granted: &str, want: &str) -> bool {
    const GRAPH: &str = "https://graph.microsoft.com/";
    let norm = |s: &str| {
        let lower = s.to_ascii_lowercase();
        lower.strip_prefix(GRAPH).map(str::to_string).unwrap_or(lower)
    };
    let want = norm(want);
    granted.split_whitespace().any(|g| norm(g) == want)
}

/// The scopes K2 itself would need for each feature, checked against a
/// granted string: the features the grant actually covers.
pub fn features_granted(provider: OauthProvider, granted: &str) -> Features {
    let all = |set: &[&str]| set.iter().all(|s| scope_granted(granted, s));
    match provider {
        OauthProvider::Gmail => Features {
            mail: all(GOOGLE_MAIL),
            calendar: all(&GOOGLE_CALENDAR[..1]),
        },
        OauthProvider::Microsoft => Features {
            mail: scope_granted(granted, "Mail.ReadWrite"),
            calendar: all(MS_CALENDAR),
        },
    }
}

// ── The minting client (PRD §3.2: refresh by the issuing client) ─────────

/// Which client a link was minted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSource {
    /// K2's built-in (build-time baked) app.
    Baked,
    /// The owner's own app (`k2 mail oauth-config set`).
    Byo,
}

impl ClientSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientSource::Baked => "baked",
            ClientSource::Byo => "byo",
        }
    }
}

/// A resolved OAuth client: the id, the (Google-only) secret and where it
/// came from. `Debug` redacts the secret.
#[derive(Clone)]
pub struct ClientChoice {
    pub id: String,
    pub secret: Option<String>,
    pub source: ClientSource,
}

impl std::fmt::Debug for ClientChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientChoice")
            .field("id", &self.id)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("source", &self.source)
            .finish()
    }
}

impl ClientChoice {
    /// The non-secret fingerprint stamped on the row (`oauth_client_hash`).
    pub fn hash(&self) -> String {
        client_hash(&self.id)
    }
}

/// sha256(client id), first 16 hex chars. A client id is public; the hash
/// only detects "the BYO app was swapped since this row was minted".
pub fn client_hash(id: &str) -> String {
    let mut h = Sha256::new();
    h.update(id.trim().as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Is this client id still the public-source placeholder (no real app
/// was baked in and none is configured)?
pub fn is_placeholder_client_id(id: &str) -> bool {
    id.trim().is_empty() || id.trim().starts_with("REPLACE_ME")
}

/// K2's built-in client for `provider`, ignoring any BYO override.
pub fn baked_client(provider: OauthProvider) -> ClientChoice {
    let cfg = provider.config();
    ClientChoice {
        id: cfg.client_id_placeholder.to_string(),
        secret: cfg.client_secret_placeholder.map(str::to_string),
        source: ClientSource::Baked,
    }
}

/// The client a NEW link uses right now: the owner's stored BYO app when
/// set, else the baked one. Resolution matches [`client_id`] /
/// [`client_secret`] exactly (a BYO Google id without a BYO secret keeps
/// the baked secret, as before).
pub fn resolve_client(provider: OauthProvider) -> ClientChoice {
    match stored_client_id(provider) {
        Some(id) => ClientChoice {
            id,
            secret: client_secret(provider, None),
            source: ClientSource::Byo,
        },
        None => baked_client(provider),
    }
}

/// The client to refresh a row with: the one stamped on the row at link
/// time. `None` (a row from before 0188) keeps today's resolution. A
/// `byo` row whose app was removed or swapped since fails loud with the
/// fix — never a silent refresh against a different app.
pub fn issuing_client(
    provider: OauthProvider,
    stamped: Option<&str>,
    stamped_hash: Option<&str>,
) -> Result<ClientChoice, String> {
    match stamped {
        None => Ok(resolve_client(provider)),
        Some("baked") => Ok(baked_client(provider)),
        Some("byo") => {
            let Some(id) = stored_client_id(provider) else {
                return Err(format!(
                    "this inbox was linked with your own {} OAuth app, which is no longer \
                     configured — set it again with 'k2 mail oauth-config set --provider {} …', \
                     or relink with 'k2 mail link upgrade <address>'",
                    provider.as_str(),
                    provider.as_str()
                ));
            };
            if let Some(h) = stamped_hash.filter(|h| !h.is_empty()) {
                if client_hash(&id) != h {
                    return Err(format!(
                        "this inbox was linked with a different {} OAuth app than the one \
                         configured now — relink it with 'k2 mail link upgrade <address>' \
                         (refresh never moves an inbox to another app)",
                        provider.as_str()
                    ));
                }
            }
            Ok(ClientChoice {
                id,
                secret: client_secret(provider, None),
                source: ClientSource::Byo,
            })
        }
        Some(other) => Err(format!("unrecognized oauth_client '{other}' on the inbox row")),
    }
}

// ── Account check (PRD §3.2) ─────────────────────────────────────────────

/// Who signed in, as far as the id_token (and `/me` / the Gmail profile)
/// tell. `identities` are lower-cased addresses; never a token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountInfo {
    pub identities: Vec<String>,
    /// Microsoft `tid` / Google Workspace `hd`.
    pub tenant: Option<String>,
    /// `personal` | `work` (Microsoft), `consumer` | `workspace` (Google).
    pub kind: Option<String>,
}

impl AccountInfo {
    fn push(&mut self, s: Option<&str>) {
        if let Some(v) = s.map(str::trim).filter(|v| v.contains('@')) {
            let v = v.to_ascii_lowercase();
            if !self.identities.contains(&v) {
                self.identities.push(v);
            }
        }
    }

    /// Does the signed-in account equal `address` (case-folded)?
    pub fn matches(&self, address: &str) -> bool {
        let a = address.trim().to_ascii_lowercase();
        self.identities.iter().any(|i| *i == a)
    }
}

/// The claims of an id_token (the JWT payload), unverified. It came
/// straight from the provider's token endpoint over TLS in this same
/// exchange, which OIDC Core §3.1.3.7 allows in place of a signature
/// check. `None` for a malformed token.
pub fn id_token_claims(id_token: &str) -> Option<serde_json::Value> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read who signed in from the id_token claims: Google `email`
/// (+`email_verified`), `hd`; Microsoft `email`, `preferred_username`,
/// `upn`, `tid`.
pub fn account_from_claims(provider: OauthProvider, claims: &serde_json::Value) -> AccountInfo {
    let mut info = AccountInfo::default();
    let s = |k: &str| claims.get(k).and_then(|v| v.as_str());
    match provider {
        OauthProvider::Gmail => {
            let verified = claims.get("email_verified").and_then(|v| v.as_bool()) != Some(false);
            if verified {
                info.push(s("email"));
            }
            match s("hd").map(str::trim).filter(|h| !h.is_empty()) {
                Some(hd) => {
                    info.tenant = Some(hd.to_ascii_lowercase());
                    info.kind = Some("workspace".to_string());
                }
                None => info.kind = Some("consumer".to_string()),
            }
        }
        OauthProvider::Microsoft => {
            info.push(s("email"));
            info.push(s("preferred_username"));
            info.push(s("upn"));
            if let Some(tid) = s("tid").map(str::trim).filter(|t| !t.is_empty()) {
                let tid = tid.to_ascii_lowercase();
                info.kind = Some(
                    if tid == MS_CONSUMER_TENANT { "personal" } else { "work" }.to_string(),
                );
                info.tenant = Some(tid);
            }
        }
    }
    info
}

/// The account check failed: the address typed is not the account that
/// consented. `signed_in` is what the provider reported (for the hint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountMismatch {
    pub signed_in: Option<String>,
}

/// Check that the account that just consented is `address` (PRD §3.2).
/// The id_token claims answer first; when they don't match (a Microsoft
/// UPN that differs from the mailbox address, or no id_token), Graph
/// `/me` (mail, UPN, proxy addresses) or the Gmail profile is asked with
/// the new access token. Nothing is stored by this function.
pub fn check_account(
    provider: OauthProvider,
    tokens: &Tokens,
    address: &str,
    http: &dyn HttpClient,
) -> Result<AccountInfo, AccountMismatch> {
    let mut info = tokens
        .id_token
        .as_deref()
        .and_then(id_token_claims)
        .map(|c| account_from_claims(provider, &c))
        .unwrap_or_default();
    if info.matches(address) {
        return Ok(info);
    }
    match provider {
        OauthProvider::Microsoft => {
            let url = format!(
                "{}/me?$select=mail,userPrincipalName,proxyAddresses,otherMails",
                graph_base()
            );
            if let Ok(resp) = http.get_bearer(&url, &tokens.access_token) {
                if (200..300).contains(&resp.status) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp.body) {
                        info.push(v.get("mail").and_then(|x| x.as_str()));
                        info.push(v.get("userPrincipalName").and_then(|x| x.as_str()));
                        for key in ["proxyAddresses", "otherMails"] {
                            for a in v.get(key).and_then(|x| x.as_array()).into_iter().flatten() {
                                // proxyAddresses read `SMTP:primary@x` / `smtp:alias@x`.
                                let raw = a.as_str().unwrap_or("");
                                let addr = raw
                                    .strip_prefix("SMTP:")
                                    .or_else(|| raw.strip_prefix("smtp:"))
                                    .unwrap_or(raw);
                                info.push(Some(addr));
                            }
                        }
                    }
                }
            }
        }
        OauthProvider::Gmail => {
            if let Ok(resp) = http.get_bearer(&gmail_profile_url(), &tokens.access_token) {
                if (200..300).contains(&resp.status) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp.body) {
                        info.push(v.get("emailAddress").and_then(|x| x.as_str()));
                    }
                }
            }
        }
    }
    if info.matches(address) {
        Ok(info)
    } else {
        Err(AccountMismatch { signed_in: info.identities.first().cloned() })
    }
}

// ── Link-error teaching (PRD §3.2 Microsoft tenant and consent) ─────────

/// A link failure with a stable code + a hint a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFailure {
    pub code: &'static str,
    pub hint: String,
    /// `admin_consent_required` only: the URL a tenant admin opens.
    pub admin_consent_url: Option<String>,
}

/// A personal (Microsoft account / live.com) sign-in refused by an app
/// registration that accepts only work or school accounts:
///  - `AADSTS50059` no tenant-identifying information (a personal account
///    against an org-only app on `common`);
///  - `AADSTS500200` "User account is a personal Microsoft account…";
///  - `AADSTS90072` / `AADSTS50020` when the identity provider is
///    `live.com` (the personal-account IdP) — a guest from another org gets
///    the same numbers without `live.com` and stays a tenant problem;
///  - the consumer endpoint's "not enabled for consumers" (no AADSTS code).
fn is_personal_account_refusal(text: &str) -> bool {
    let live = text.contains("live.com") || text.contains("Microsoft Account");
    text.contains("AADSTS50059")
        || text.contains("AADSTS500200")
        || text.contains("not enabled for consumers")
        || (live && (text.contains("AADSTS90072") || text.contains("AADSTS50020")))
}

/// Map a token-endpoint / device-start error to a teaching failure. The
/// AADSTS codes are Microsoft's stable error numbers; anything unknown
/// keeps the provider's own text under `oauth_error`.
pub fn classify_link_error(provider: OauthProvider, err: &OauthError, client_id: &str) -> LinkFailure {
    let text = err.to_string();
    let has = |code: &str| text.contains(code);
    if provider == OauthProvider::Microsoft {
        if has("AADSTS65001") || has("AADSTS90094") || has("AADSTS90008") {
            let url = admin_consent_url(client_id);
            return LinkFailure {
                code: "admin_consent_required",
                hint: format!(
                    "this Microsoft tenant needs an administrator to approve K2 before people \
                     can link their mailboxes. A tenant admin opens {url}, approves, then you run \
                     the link again. ({text})"
                ),
                admin_consent_url: Some(url),
            };
        }
        if has("AADSTS53") || has("AADSTS50097") {
            return LinkFailure {
                code: "device_code_blocked",
                hint: format!(
                    "your tenant's Conditional Access policy blocked the device-code sign-in. \
                     Ask your Microsoft 365 admin to allow it for K2, or note this error and \
                     tell Alakazam (a browser sign-in fallback is planned). ({text})"
                ),
                admin_consent_url: None,
            };
        }
        if has("AADSTS7000218") {
            return LinkFailure {
                code: "client_misconfigured",
                hint: format!(
                    "the Microsoft app registration does not allow public client flows — in \
                     Entra → App registrations → <app> → Authentication, set 'Allow public \
                     client flows' to Yes, then link again. ({text})"
                ),
                admin_consent_url: None,
            };
        }
        if is_personal_account_refusal(&text) {
            return LinkFailure {
                code: "personal_account_not_allowed",
                hint: format!(
                    "This server's Microsoft app only accepts work or school accounts. The \
                     owner can allow personal accounts on the app registration, or use their \
                     own app: k2 mail oauth-config set --provider microsoft --client-id <id> \
                     --tenant consumers ({text})"
                ),
                admin_consent_url: None,
            };
        }
        if has("AADSTS700016") || has("AADSTS50020") || has("AADSTS90002") || has("AADSTS50194")
        {
            return LinkFailure {
                code: "client_misconfigured",
                hint: format!(
                    "Microsoft could not use this app for this account — check the client id \
                     and the tenant ('k2 mail oauth-config show'). A single-tenant app needs \
                     '--tenant <its tenant id>'; one for any organisation or personal accounts \
                     needs 'common'. ({text})"
                ),
                admin_consent_url: None,
            };
        }
        if has("AADSTS70011") || has("AADSTS65005") {
            return LinkFailure {
                code: "client_misconfigured",
                hint: format!(
                    "the Microsoft app registration does not allow the scopes K2 asks for — add \
                     the delegated Graph permissions openid, profile, email, offline_access, \
                     User.Read, Mail.ReadWrite, Mail.Send and Calendars.ReadWrite. ({text})"
                ),
                admin_consent_url: None,
            };
        }
    }
    if text.contains("'invalid_client'") || text.contains("unauthorized_client") {
        return LinkFailure {
            code: "client_misconfigured",
            hint: format!(
                "the {} OAuth app was refused — check 'k2 mail oauth-config show' (a build \
                 without a real client id needs your own app). ({text})",
                provider.as_str()
            ),
            admin_consent_url: None,
        };
    }
    LinkFailure { code: "oauth_error", hint: format!("oauth error: {text}"), admin_consent_url: None }
}

#[cfg(test)]
mod tests;
