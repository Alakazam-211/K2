//! A custom App's own push keys (prd-custom-app-push-v1 §6, CP12–CP17).
//!
//! - **Where.** The vault, `system` namespace, scope `app/<service id>`:
//!   [`APNS_SECRET`] (the `.p8` PEM) and [`FCM_SECRET`] (the Firebase
//!   service account, kept to the four fields K2 uses). Only the daemon
//!   resolves them (`Use::System`, `Requester::System("app-push")`); no
//!   route returns a system value, so the App, its agent, other Apps and
//!   the owner's own CLI can replace or remove a key but never read it back.
//! - **What else is kept.** `app_push_config` holds the metadata: key id,
//!   team id, bundle id, Firebase project and client email, an 8-byte
//!   SHA-256 fingerprint of each key, when it was set, the last refusal.
//! - **Checked before storing, no network** (CP14): the `.p8` parses as a
//!   PKCS#8 P-256 key and signs a JWT locally; ids have Apple's shapes; the
//!   service account is `type: service_account` with an RSA key that signs
//!   an assertion locally, and **`token_uri` is exactly Google's** (an
//!   owner-supplied file must never point the daemon's signed assertion at
//!   another host, V21). Errors name the field, never echo a value.
//! - `Debug` of every key type is redacted.

use k2_core::skin::app_push as store;
use k2_vault::{Actor, Delivery, Kind, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};
use serde::{Deserialize, Serialize};

/// The vault confirm action for setting or removing an App's push keys
/// (`secretId` = the App's service id).
pub const CONFIRM_ACTION: &str = "app-push-keys";
/// The vault names (system namespace, scope `app/<service id>`).
pub const APNS_SECRET: &str = "APNS_AUTH_KEY_P8";
pub const FCM_SECRET: &str = "FCM_SERVICE_ACCOUNT_JSON";
/// The only token endpoint a service account may name (V21).
pub const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
/// The OAuth scope FCM's HTTP v1 API needs.
pub const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// The vault requester / actor name.
pub const SUBSYSTEM: &str = "app-push";
/// A `.p8` is a few hundred bytes; refuse anything absurd.
pub const MAX_P8_BYTES: usize = 4 * 1024;
/// A service account JSON is a couple of KiB.
pub const MAX_SA_BYTES: usize = 16 * 1024;

/// An App's APNs signing key.
#[derive(Clone, PartialEq, Eq)]
pub struct ApnsKey {
    pub key_id: String,
    pub team_id: String,
    pub bundle_id: String,
    p8: String,
}

impl std::fmt::Debug for ApnsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApnsKey")
            .field("key_id", &self.key_id)
            .field("team_id", &self.team_id)
            .field("bundle_id", &self.bundle_id)
            .field("p8", &"<redacted>")
            .finish()
    }
}

/// An App's Firebase service account (the fields K2 uses).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FcmKey {
    pub project_id: String,
    pub client_email: String,
    #[serde(default)]
    pub private_key_id: Option<String>,
    private_key: String,
}

impl std::fmt::Debug for FcmKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FcmKey")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .field("private_key", &"<redacted>")
            .finish()
    }
}

/// First 8 bytes of SHA-256, hex (`app_push_config.*_key_fpr`).
pub fn fingerprint(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn apple_id(v: &str) -> bool {
    v.len() == 10 && v.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// A bundle id: reverse DNS, `[A-Za-z0-9-]` labels, at least two.
pub fn valid_bundle_id(v: &str) -> bool {
    v.len() <= 155
        && v.split('.').count() >= 2
        && v.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn normalize_pem(raw: &str) -> String {
    let mut s = raw.trim().replace("\r\n", "\n");
    s.push('\n');
    s
}

#[derive(Serialize)]
struct ApnsClaims<'a> {
    iss: &'a str,
    iat: i64,
}

/// The APNs provider token: ES256, `kid` = key id, `{iss: team id, iat}`.
pub fn apns_jwt(key: &ApnsKey, iat: i64) -> Result<String, String> {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = Some(key.key_id.clone());
    let enc = jsonwebtoken::EncodingKey::from_ec_pem(key.p8.as_bytes()).map_err(|_| "the APNs key isn't a usable EC key".to_string())?;
    jsonwebtoken::encode(&header, &ApnsClaims { iss: &key.team_id, iat }, &enc).map_err(|_| "the APNs key can't sign (is it a P-256 .p8?)".to_string())
}

#[derive(Serialize)]
struct FcmClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

/// The service account's signed assertion for Google's token endpoint
/// (RS256; `aud` is always [`GOOGLE_TOKEN_URI`]).
pub fn fcm_assertion(key: &FcmKey, iat: i64) -> Result<String, String> {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = key.private_key_id.clone();
    let enc = jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())
        .map_err(|_| "the service account's private_key isn't a usable RSA key".to_string())?;
    let claims = FcmClaims { iss: &key.client_email, scope: FCM_SCOPE, aud: GOOGLE_TOKEN_URI, iat, exp: iat + 3600 };
    jsonwebtoken::encode(&header, &claims, &enc).map_err(|_| "the service account's private_key can't sign".to_string())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Check an APNs key (CP14). Errors name the field.
pub fn validate_apns(p8: &str, key_id: &str, team_id: &str, bundle_id: &str) -> Result<ApnsKey, String> {
    let key_id = key_id.trim();
    let team_id = team_id.trim();
    let bundle_id = bundle_id.trim();
    if !apple_id(key_id) {
        return Err("apns.keyId must be Apple's 10-character key id (A-Z, 0-9)".into());
    }
    if !apple_id(team_id) {
        return Err("apns.teamId must be Apple's 10-character team id (A-Z, 0-9)".into());
    }
    if !valid_bundle_id(bundle_id) {
        return Err("apns.bundleId must be your app's bundle id, reverse DNS (com.example.app)".into());
    }
    if p8.len() > MAX_P8_BYTES {
        return Err("apns.p8 is too large for an APNs key".into());
    }
    let pem = normalize_pem(p8);
    if !pem.starts_with("-----BEGIN PRIVATE KEY-----\n") || !pem.ends_with("-----END PRIVATE KEY-----\n") {
        return Err("apns.p8 must be the AuthKey_XXXXXXXXXX.p8 file's contents (a PKCS#8 PEM)".into());
    }
    {
        use p256::pkcs8::DecodePrivateKey;
        p256::SecretKey::from_pkcs8_pem(&pem).map_err(|_| "apns.p8 isn't a P-256 private key (download the .p8 again)".to_string())?;
    }
    let key = ApnsKey { key_id: key_id.into(), team_id: team_id.into(), bundle_id: bundle_id.into(), p8: pem };
    // The self-test: a provider token mints locally.
    apns_jwt(&key, now()).map_err(|_| "apns.p8 can't sign a provider token".to_string())?;
    Ok(key)
}

#[derive(Deserialize)]
struct RawServiceAccount {
    #[serde(rename = "type")]
    kind: Option<String>,
    project_id: Option<String>,
    client_email: Option<String>,
    private_key: Option<String>,
    private_key_id: Option<String>,
    token_uri: Option<String>,
}

/// Check a Firebase service account JSON (CP14). Errors name the field.
pub fn validate_fcm(json_text: &str) -> Result<FcmKey, String> {
    if json_text.len() > MAX_SA_BYTES {
        return Err("fcm.serviceAccountJson is too large for a service account".into());
    }
    let raw: RawServiceAccount =
        serde_json::from_str(json_text).map_err(|_| "fcm.serviceAccountJson must be the service account's JSON file".to_string())?;
    if raw.kind.as_deref() != Some("service_account") {
        return Err("fcm.serviceAccountJson: type must be service_account (Project settings > Service accounts > Generate new private key)".into());
    }
    let project_id = raw.project_id.map(|s| s.trim().to_string()).unwrap_or_default();
    if project_id.is_empty() || project_id.len() > 64 || !project_id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err("fcm.serviceAccountJson: project_id is missing or not a Firebase project id".into());
    }
    let client_email = raw.client_email.map(|s| s.trim().to_string()).unwrap_or_default();
    if client_email.len() > 254
        || !client_email.contains('@')
        || !client_email.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
    {
        return Err("fcm.serviceAccountJson: client_email is missing or malformed".into());
    }
    match raw.token_uri.as_deref().map(str::trim) {
        None => {}
        Some(GOOGLE_TOKEN_URI) => {}
        Some(_) => return Err(format!("fcm.serviceAccountJson: token_uri must be exactly {GOOGLE_TOKEN_URI}")),
    }
    let private_key_id = raw.private_key_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric()));
    let pem = normalize_pem(raw.private_key.as_deref().unwrap_or(""));
    if !pem.starts_with("-----BEGIN PRIVATE KEY-----\n") {
        return Err("fcm.serviceAccountJson: private_key is missing or not a PKCS#8 PEM".into());
    }
    let key = FcmKey { project_id, client_email, private_key_id, private_key: pem };
    fcm_assertion(&key, now()).map_err(|_| "fcm.serviceAccountJson: private_key isn't an RSA key that can sign".to_string())?;
    Ok(key)
}

impl ApnsKey {
    pub fn fingerprint(&self) -> String {
        fingerprint(self.p8.as_bytes())
    }

    pub fn meta(&self, set_at: i64) -> store::ApnsMeta {
        store::ApnsMeta {
            key_id: self.key_id.clone(),
            team_id: self.team_id.clone(),
            bundle_id: self.bundle_id.clone(),
            key_fingerprint: self.fingerprint(),
            set_at,
            proven_at: None,
        }
    }
}

impl FcmKey {
    pub fn fingerprint(&self) -> String {
        fingerprint(self.private_key.as_bytes())
    }

    pub fn meta(&self, set_at: i64) -> store::FcmMeta {
        store::FcmMeta {
            project_id: self.project_id.clone(),
            client_email: self.client_email.clone(),
            key_fingerprint: self.fingerprint(),
            set_at,
            proven_at: None,
        }
    }
}

// ── the vault ────────────────────────────────────────────────────────

/// Why keys couldn't be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The vault isn't open (`503 vault_closed`, A11).
    VaultClosed,
    /// No key for that platform.
    NotConfigured,
    /// The vault answered with an error (its code only).
    Vault(String),
}

impl KeyError {
    pub fn code(&self) -> String {
        match self {
            KeyError::VaultClosed => "vault_closed".into(),
            KeyError::NotConfigured => "push_not_configured".into(),
            KeyError::Vault(c) => format!("vault:{c}"),
        }
    }
}

fn vault() -> Result<std::sync::Arc<k2_vault::LocalVault>, KeyError> {
    crate::vault::get().map_err(|_| KeyError::VaultClosed)
}

fn sel(app: &str, name: &str) -> Selector {
    Selector::system(Scope::app(app), name)
}

fn put(app: &str, name: &str, bytes: &[u8], what: &str) -> Result<(), KeyError> {
    let v = vault()?;
    let mut r = PutRequest::new(Namespace::System, Scope::app(app), name, Kind::Generic);
    r.default_delivery = Some(Delivery::None);
    r.delivery_ceiling = Some(Delivery::None);
    r.origin = format!("system:{SUBSYSTEM}");
    r.description = format!("A custom App's {what} (push notifications); daemon-only.");
    r.mode = PutMode::Upsert;
    r.idempotent = true;
    v.put(r, SecretBytes::from_slice(bytes), &Actor::System(SUBSYSTEM.into())).map(|_| ()).map_err(|e| KeyError::Vault(e.code.to_string()))
}

fn get(app: &str, name: &str) -> Result<Option<Vec<u8>>, KeyError> {
    let v = vault()?;
    match v.resolve(&sel(app, name), Use::System, &Requester::System(SUBSYSTEM.into())) {
        Ok(r) => Ok(Some(r.value.expose().to_vec())),
        Err(e) if e.status == 404 || e.code == "secret_not_active" => Ok(None),
        Err(e) => Err(KeyError::Vault(e.code.to_string())),
    }
}

fn destroy(app: &str, name: &str) -> Result<bool, KeyError> {
    let v = vault()?;
    match v.destroy(&sel(app, name), &Actor::System(SUBSYSTEM.into())) {
        Ok(()) => Ok(true),
        // Never set, or already destroyed: nothing to do.
        Err(e) if e.status == 404 || e.code == "secret_destroyed" => Ok(false),
        Err(e) => Err(KeyError::Vault(e.code.to_string())),
    }
}

/// Store an APNs key (the `.p8` in the vault, the rest as metadata).
pub fn store_apns(app: &str, key: &ApnsKey, actor: &str) -> Result<store::PushConfig, KeyError> {
    put(app, APNS_SECRET, key.p8.as_bytes(), "APNs key")?;
    store::set_apns_meta(app, Some(&key.meta(now())), actor).map_err(KeyError::Vault)
}

/// Store a Firebase service account (the four fields K2 uses).
pub fn store_fcm(app: &str, key: &FcmKey, actor: &str) -> Result<store::PushConfig, KeyError> {
    let json = serde_json::to_vec(key).map_err(|e| KeyError::Vault(format!("encode:{e}")))?;
    put(app, FCM_SECRET, &json, "Firebase service account")?;
    store::set_fcm_meta(app, Some(&key.meta(now())), actor).map_err(KeyError::Vault)
}

/// The App's APNs key, for the sender. The `.p8` from the vault, the ids
/// from the metadata.
pub fn load_apns(app: &str) -> Result<ApnsKey, KeyError> {
    let cfg = store::config(app).map_err(KeyError::Vault)?;
    let Some(meta) = cfg.apns else { return Err(KeyError::NotConfigured) };
    let bytes = get(app, APNS_SECRET)?.ok_or(KeyError::NotConfigured)?;
    let p8 = String::from_utf8(bytes).map_err(|_| KeyError::Vault("not_utf8".into()))?;
    Ok(ApnsKey { key_id: meta.key_id, team_id: meta.team_id, bundle_id: meta.bundle_id, p8 })
}

/// The App's Firebase service account, for the sender.
pub fn load_fcm(app: &str) -> Result<FcmKey, KeyError> {
    let cfg = store::config(app).map_err(KeyError::Vault)?;
    if cfg.fcm.is_none() {
        return Err(KeyError::NotConfigured);
    }
    let bytes = get(app, FCM_SECRET)?.ok_or(KeyError::NotConfigured)?;
    serde_json::from_slice(&bytes).map_err(|_| KeyError::Vault("bad_json".into()))
}

/// Remove one platform's key (`apns` | `fcm`). `Ok(false)` = there was none.
pub fn remove(app: &str, platform: &str, actor: &str) -> Result<bool, KeyError> {
    // "Was there one" is the metadata's answer (the vault keeps a destroyed
    // secret's tombstone, so its own answer can't tell).
    let cfg = store::config(app).map_err(KeyError::Vault)?;
    match platform {
        store::PLATFORM_APNS => {
            destroy(app, APNS_SECRET)?;
            store::set_apns_meta(app, None, actor).map_err(KeyError::Vault)?;
            Ok(cfg.apns.is_some())
        }
        store::PLATFORM_FCM => {
            destroy(app, FCM_SECRET)?;
            store::set_fcm_meta(app, None, actor).map_err(KeyError::Vault)?;
            Ok(cfg.fcm.is_some())
        }
        _ => Err(KeyError::Vault("bad_platform".into())),
    }
}

/// The App went away: destroy both keys (best effort; a closed vault is
/// logged, and the secrets stay unreadable to everything but the daemon).
pub fn forget_app(app: &str) {
    for name in [APNS_SECRET, FCM_SECRET] {
        match destroy(app, name) {
            Ok(_) => {}
            Err(e) => k2_core::log_debug!("[app-push] {app} removed: WARN destroying a push key: {}", e.code()),
        }
    }
}

#[cfg(test)]
#[path = "creds_tests.rs"]
mod tests;
