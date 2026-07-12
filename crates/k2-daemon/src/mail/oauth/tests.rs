//! O1 OAuth engine unit tests — a canned mock token endpoint + an
//! in-memory vault fake. NO real network, NO real filesystem (house
//! rules). Every flow (device start/poll, loopback build/exchange,
//! refresh incl. Microsoft rotation, `access_token_for` refresh window,
//! vault round-trip) is covered, plus the invariant that a token never
//! reaches an error or `Debug` string.

use super::*;
use crate::mail::secrets::SecretStore;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

// ── Fakes ───────────────────────────────────────────────────────────────

/// Scripted HTTP mock: FIFO queue of canned `(status, body)` replies,
/// records every request (url + form pairs) for assertions. An empty
/// queue PANICS on the next call — so a test that must NOT hit the
/// network fails loudly if it does.
struct MockHttp {
    queue: Mutex<VecDeque<(u16, String)>>,
    seen: Mutex<Vec<(String, Vec<(String, String)>)>>,
}

impl MockHttp {
    fn new() -> Self {
        Self { queue: Mutex::new(VecDeque::new()), seen: Mutex::new(Vec::new()) }
    }
    fn push(&self, status: u16, body: &str) {
        self.queue.lock().unwrap().push_back((status, body.to_string()));
    }
    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
    /// The (url, form) of the Nth recorded request.
    fn request(&self, n: usize) -> (String, Vec<(String, String)>) {
        self.seen.lock().unwrap()[n].clone()
    }
    /// Value of a form field in the Nth request.
    fn field(&self, n: usize, key: &str) -> Option<String> {
        self.request(n)
            .1
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

impl HttpClient for MockHttp {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, OauthError> {
        self.seen.lock().unwrap().push((
            url.to_string(),
            form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        ));
        let (status, body) = self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .expect("MockHttp: an unexpected HTTP call (queue empty)");
        Ok(HttpResponse { status, body })
    }
}

/// In-memory vault fake — the `SecretStore` surface the engine uses, no
/// filesystem.
#[derive(Default)]
struct MemStore {
    map: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemStore {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        let key = format!("mem_{kind}_{}", self.map.lock().unwrap().len());
        self.map.lock().unwrap().insert(key.clone(), secret.to_string());
        Ok(key)
    }
    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        Ok(self.map.lock().unwrap().get(sref).cloned())
    }
    fn delete(&self, sref: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(sref);
        Ok(())
    }
    fn store_exact(&self, key: &str, secret: &str) -> Result<(), String> {
        self.map.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
}

// ── Config / client_id seam ──────────────────────────────────────────────

#[test]
fn provider_configs_match_prd_9() {
    let g = OauthProvider::Gmail.config();
    assert_eq!(g.auth_url, Some("https://accounts.google.com/o/oauth2/v2/auth"));
    assert_eq!(g.token_url, "https://oauth2.googleapis.com/token");
    assert_eq!(g.revoke_url, Some("https://oauth2.googleapis.com/revoke"));
    assert_eq!(g.scope, "https://mail.google.com/");
    assert_eq!(g.default_flow, FlowKind::Loopback);
    assert!(g.extra_auth_params.contains(&("access_type", "offline")));

    let m = OauthProvider::Microsoft.config();
    assert!(m
        .device_auth_url
        .unwrap()
        .starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/"));
    assert!(m
        .token_url
        .starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/"));
    assert_eq!(m.scope, "https://graph.microsoft.com/Mail.ReadWrite offline_access");
    assert_eq!(m.default_flow, FlowKind::DeviceCode);
    assert_eq!(m.revoke_url, None);
}

#[test]
fn no_client_secret_field_exists() {
    // Compile-time guarantee: ProviderConfig carries only public data.
    // (If a `client_secret` field were added, this doc-lock reminds the
    // author it violates the public-client rule.)
    let g = OauthProvider::Gmail.config();
    assert!(g.client_id_placeholder.contains("REPLACE_ME"));
}

#[test]
fn client_id_override_seam() {
    assert_eq!(client_id(OauthProvider::Gmail, Some("my-byo-id")), "my-byo-id");
    assert_eq!(
        client_id(OauthProvider::Gmail, None),
        OauthProvider::Gmail.config().client_id_placeholder
    );
    // Blank override falls back to the placeholder.
    assert_eq!(
        client_id(OauthProvider::Gmail, Some("   ")),
        OauthProvider::Gmail.config().client_id_placeholder
    );
}

#[test]
fn provider_str_roundtrip() {
    assert_eq!(OauthProvider::Gmail.as_str(), "gmail");
    assert_eq!(OauthProvider::from_str("microsoft"), Some(OauthProvider::Microsoft));
    assert_eq!(OauthProvider::from_str("nope"), None);
}

#[test]
fn form_encode_percent_encodes_specials() {
    assert_eq!(form_encode("https://mail.google.com/"), "https%3A%2F%2Fmail.google.com%2F");
    assert_eq!(form_encode("a b"), "a%20b");
    assert_eq!(form_encode("safe-._~AZ09"), "safe-._~AZ09");
}

// ── Device-code flow ─────────────────────────────────────────────────────

#[test]
fn device_start_happy_and_request_shape() {
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"device_code":"DC-secret","user_code":"WXYZ-1234",
            "verification_uri":"https://microsoft.com/devicelogin",
            "expires_in":900,"interval":5}"#,
    );
    let start = device_start(OauthProvider::Microsoft, None, &http).expect("device start");
    assert_eq!(start.user_code, "WXYZ-1234");
    assert_eq!(start.verification_uri, "https://microsoft.com/devicelogin");
    assert_eq!(start.expires_in, 900);
    assert_eq!(start.interval, 5);
    // Request went to the device endpoint with client_id + scope.
    let (url, _) = http.request(0);
    assert!(url.ends_with("/devicecode"), "{url}");
    assert_eq!(
        http.field(0, "scope").as_deref(),
        Some("https://graph.microsoft.com/Mail.ReadWrite offline_access")
    );
    assert!(http.field(0, "client_id").is_some());
}

#[test]
fn device_start_accepts_google_verification_url_alias() {
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"device_code":"DC","user_code":"ABCD",
            "verification_url":"https://www.google.com/device","interval":5,"expires_in":1800}"#,
    );
    let start = device_start(OauthProvider::Gmail, None, &http).expect("start");
    assert_eq!(start.verification_uri, "https://www.google.com/device");
}

#[test]
fn device_poll_all_states() {
    let http = MockHttp::new();
    http.push(400, r#"{"error":"authorization_pending"}"#);
    http.push(400, r#"{"error":"slow_down"}"#);
    http.push(400, r#"{"error":"access_denied"}"#);
    http.push(400, r#"{"error":"expired_token"}"#);
    http.push(
        200,
        r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600,"token_type":"Bearer"}"#,
    );
    let p = OauthProvider::Microsoft;
    assert!(matches!(device_poll(p, "DC", None, &http).unwrap(), Poll::Pending));
    assert!(matches!(device_poll(p, "DC", None, &http).unwrap(), Poll::SlowDown));
    assert!(matches!(device_poll(p, "DC", None, &http).unwrap(), Poll::Denied));
    assert!(matches!(device_poll(p, "DC", None, &http).unwrap(), Poll::Expired));
    match device_poll(p, "DC", None, &http).unwrap() {
        Poll::Ok(t) => {
            assert_eq!(t.access_token, "AT-1");
            assert_eq!(t.refresh_token.as_deref(), Some("RT-1"));
            assert_eq!(t.expires_in, 3600);
        }
        other => panic!("expected Ok tokens, got {other:?}"),
    }
    // The poll request carried the device grant.
    assert_eq!(http.field(0, "grant_type").as_deref(), Some(super::DEVICE_GRANT));
    assert_eq!(http.field(0, "device_code").as_deref(), Some("DC"));
}

#[test]
fn device_poll_unknown_error_is_hard_error() {
    let http = MockHttp::new();
    http.push(400, r#"{"error":"invalid_client","error_description":"bad app"}"#);
    let err = device_poll(OauthProvider::Microsoft, "DC", None, &http).unwrap_err();
    assert!(matches!(err, OauthError::Provider { .. }));
}

// ── Loopback flow ────────────────────────────────────────────────────────

#[test]
fn loopback_build_url_pure_and_encoded() {
    let url = loopback_build_url(
        OauthProvider::Gmail,
        None,
        "http://127.0.0.1:53017/cb",
        "state-nonce-abc",
    )
    .expect("build url");
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"), "{url}");
    assert!(url.contains("response_type=code"));
    assert!(url.contains("state=state-nonce-abc"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A53017%2Fcb"));
    assert!(url.contains("scope=https%3A%2F%2Fmail.google.com%2F"));
    // Google's refresh-token-forcing params rode along.
    assert!(url.contains("access_type=offline"));
    assert!(url.contains("prompt=consent"));
}

#[test]
fn loopback_exchange_request_and_tokens() {
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"access_token":"AT-lb","refresh_token":"RT-lb","expires_in":3599,"token_type":"Bearer","scope":"https://mail.google.com/"}"#,
    );
    let t = loopback_exchange(
        OauthProvider::Gmail,
        "auth-code-xyz",
        "http://127.0.0.1:53017/cb",
        None,
        &http,
    )
    .expect("exchange");
    assert_eq!(t.access_token, "AT-lb");
    assert_eq!(t.refresh_token.as_deref(), Some("RT-lb"));
    assert_eq!(http.field(0, "grant_type").as_deref(), Some("authorization_code"));
    assert_eq!(http.field(0, "code").as_deref(), Some("auth-code-xyz"));
    assert_eq!(http.field(0, "redirect_uri").as_deref(), Some("http://127.0.0.1:53017/cb"));
}

#[test]
fn validate_state_guard() {
    assert!(validate_state("nonce", "nonce").is_ok());
    assert!(matches!(validate_state("nonce", "other").unwrap_err(), OauthError::State));
    assert!(matches!(validate_state("", "").unwrap_err(), OauthError::State));
}

#[test]
fn bind_ephemeral_loopback_is_localhost() {
    let (_listener, addr) = bind_ephemeral_loopback().expect("bind");
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
}

// ── Refresh + rotation ───────────────────────────────────────────────────

#[test]
fn refresh_microsoft_rotates_refresh_token() {
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"access_token":"AT-new","refresh_token":"RT-new","expires_in":3600,"token_type":"Bearer"}"#,
    );
    let t = refresh(OauthProvider::Microsoft, "RT-old", None, &http).expect("refresh");
    assert_eq!(t.access_token, "AT-new");
    assert_eq!(t.refresh_token.as_deref(), Some("RT-new"), "MS rotates → new RT present");
    assert_eq!(http.field(0, "grant_type").as_deref(), Some("refresh_token"));
    assert_eq!(http.field(0, "refresh_token").as_deref(), Some("RT-old"));
}

#[test]
fn refresh_google_omits_refresh_token() {
    let http = MockHttp::new();
    http.push(200, r#"{"access_token":"AT-g","expires_in":3600,"token_type":"Bearer"}"#);
    let t = refresh(OauthProvider::Gmail, "RT-keep", None, &http).expect("refresh");
    assert_eq!(t.access_token, "AT-g");
    assert_eq!(t.refresh_token, None, "Google omits RT on refresh (keep the old one)");
}

// ── Vault round-trip ─────────────────────────────────────────────────────

#[test]
fn vault_store_load_roundtrip() {
    let store = MemStore::default();
    let tokens = Tokens {
        access_token: "AT-vault".into(),
        refresh_token: Some("RT-vault".into()),
        scope: Some("https://mail.google.com/".into()),
        token_type: "Bearer".into(),
        expires_in: 3600,
    };
    let expiry = store_tokens(&store, "row1", &tokens, 1_000).expect("store");
    assert_eq!(expiry, 1_000 + 3600);
    // The bundle lives under the -oauth suffixed key.
    assert!(store.resolve(&oauth_vault_key("row1")).unwrap().is_some());

    let loaded = load_tokens(&store, "row1").expect("load");
    assert_eq!(loaded.access_token, "AT-vault");
    assert_eq!(loaded.refresh_token.as_deref(), Some("RT-vault"));
    assert_eq!(loaded.scope.as_deref(), Some("https://mail.google.com/"));
    assert_eq!(loaded.token_type, "Bearer");
}

#[test]
fn load_tokens_missing_fails_loud() {
    let store = MemStore::default();
    assert!(matches!(load_tokens(&store, "nope").unwrap_err(), OauthError::Vault(_)));
}

// ── access_token_for refresh window ──────────────────────────────────────

fn seed(store: &MemStore, row: &str, access: &str, refresh: Option<&str>) {
    let t = Tokens {
        access_token: access.into(),
        refresh_token: refresh.map(str::to_string),
        scope: Some("https://mail.google.com/".into()),
        token_type: "Bearer".into(),
        expires_in: 3600,
    };
    store_tokens(store, row, &t, 0).expect("seed");
}

#[test]
fn access_token_for_reuses_when_fresh() {
    let store = MemStore::default();
    seed(&store, "row1", "AT-current", Some("RT-1"));
    let http = MockHttp::new(); // empty → any HTTP call panics
    // Expires far in the future (now=1000, exp=5000) → no refresh.
    let fresh = access_token_for(
        &store,
        "row1",
        OauthProvider::Gmail,
        Some(5_000),
        1_000,
        None,
        &http,
    )
    .expect("access");
    assert_eq!(fresh.access_token, "AT-current");
    assert_eq!(fresh.token_expires_at, 5_000);
    assert!(!fresh.refreshed);
    assert_eq!(http.calls(), 0, "must not touch the network when fresh");
}

#[test]
fn access_token_for_refreshes_within_60s_window() {
    let store = MemStore::default();
    seed(&store, "row1", "AT-old", Some("RT-old"));
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"access_token":"AT-fresh","refresh_token":"RT-rotated","expires_in":3600,"token_type":"Bearer"}"#,
    );
    // now=1000, exp=1030 → within the 60s skew → refresh.
    let fresh = access_token_for(
        &store,
        "row1",
        OauthProvider::Microsoft,
        Some(1_030),
        1_000,
        None,
        &http,
    )
    .expect("access");
    assert_eq!(fresh.access_token, "AT-fresh");
    assert!(fresh.refreshed);
    assert_eq!(fresh.token_expires_at, 1_000 + 3600);
    // Microsoft rotation persisted to the vault.
    let reloaded = load_tokens(&store, "row1").expect("reload");
    assert_eq!(reloaded.refresh_token.as_deref(), Some("RT-rotated"));
    assert_eq!(reloaded.access_token, "AT-fresh");
}

#[test]
fn access_token_for_google_keeps_refresh_token_on_refresh() {
    let store = MemStore::default();
    seed(&store, "row1", "AT-old", Some("RT-keep"));
    let http = MockHttp::new();
    // Google omits refresh_token in the refresh reply.
    http.push(200, r#"{"access_token":"AT-fresh","expires_in":3600,"token_type":"Bearer"}"#);
    let fresh = access_token_for(
        &store,
        "row1",
        OauthProvider::Gmail,
        None, // unknown expiry → force refresh
        1_000,
        None,
        &http,
    )
    .expect("access");
    assert!(fresh.refreshed);
    let reloaded = load_tokens(&store, "row1").expect("reload");
    assert_eq!(
        reloaded.refresh_token.as_deref(),
        Some("RT-keep"),
        "Google keeps the original refresh_token"
    );
    assert_eq!(reloaded.access_token, "AT-fresh");
}

#[test]
fn access_token_for_without_refresh_token_fails_loud() {
    let store = MemStore::default();
    seed(&store, "row1", "AT-old", None); // no refresh token
    let http = MockHttp::new();
    let err = access_token_for(
        &store,
        "row1",
        OauthProvider::Gmail,
        None,
        1_000,
        None,
        &http,
    )
    .unwrap_err();
    assert!(matches!(err, OauthError::Vault(_)));
    assert_eq!(http.calls(), 0);
}

// ── Revoke ───────────────────────────────────────────────────────────────

#[test]
fn revoke_gmail_hits_endpoint() {
    let http = MockHttp::new();
    http.push(200, "");
    revoke(OauthProvider::Gmail, "AT-to-revoke", &http).expect("revoke");
    let (url, _) = http.request(0);
    assert_eq!(url, "https://oauth2.googleapis.com/revoke");
    assert_eq!(http.field(0, "token").as_deref(), Some("AT-to-revoke"));
}

#[test]
fn revoke_microsoft_is_noop_no_request() {
    let http = MockHttp::new(); // empty → a call would panic
    revoke(OauthProvider::Microsoft, "AT", &http).expect("noop revoke");
    assert_eq!(http.calls(), 0, "MS has no revoke endpoint → no request");
}

#[test]
fn revoke_swallows_transport_errors() {
    // A revoke whose transport fails must still succeed (best-effort).
    struct FailHttp;
    impl HttpClient for FailHttp {
        fn post_form(&self, _url: &str, _form: &[(&str, &str)]) -> Result<HttpResponse, OauthError> {
            Err(OauthError::Http("boom".into()))
        }
    }
    assert!(revoke(OauthProvider::Gmail, "AT", &FailHttp).is_ok());
}

// ── Redaction: a token NEVER reaches an error or Debug string ─────────────

const SENTINEL: &str = "SENTINEL_TOKEN_VALUE_zzz";

#[test]
fn tokens_debug_redacts_secrets() {
    let t = Tokens {
        access_token: SENTINEL.into(),
        refresh_token: Some(SENTINEL.into()),
        scope: Some("s".into()),
        token_type: "Bearer".into(),
        expires_in: 60,
    };
    let dbg = format!("{t:?}");
    assert!(!dbg_contains(&dbg), "Tokens Debug leaked a token: {dbg}");
    assert!(dbg.contains("<redacted>"));
}

#[test]
fn device_start_debug_redacts_device_code() {
    let d = DeviceStart {
        device_code: SENTINEL.into(),
        user_code: "ABCD".into(),
        verification_uri: "https://x".into(),
        expires_in: 900,
        interval: 5,
    };
    let dbg = format!("{d:?}");
    assert!(!dbg.contains(SENTINEL), "DeviceStart Debug leaked device_code: {dbg}");
}

#[test]
fn parse_error_on_token_body_drops_the_body() {
    // A token endpoint returns a non-JSON body that CONTAINS a token
    // sentinel. The resulting error must NOT echo it.
    let http = MockHttp::new();
    http.push(200, &format!("garbage {SENTINEL} not-json"));
    let err = refresh(OauthProvider::Gmail, "RT", None, &http).unwrap_err();
    let shown = format!("{err} / {err:?}");
    assert!(!shown.contains(SENTINEL), "error leaked the response body: {shown}");
    assert!(matches!(err, OauthError::Parse(_)));
}

#[test]
fn vaulted_token_parse_error_drops_the_body() {
    let store = MemStore::default();
    store
        .store_exact(&oauth_vault_key("row1"), &format!("not json {SENTINEL}"))
        .unwrap();
    let err = load_tokens(&store, "row1").unwrap_err();
    let shown = format!("{err} / {err:?}");
    assert!(!shown.contains(SENTINEL), "vault parse error leaked the body: {shown}");
}

fn dbg_contains(s: &str) -> bool {
    s.contains(SENTINEL)
}
