//! O1 OAuth engine unit tests — a canned mock token endpoint + an
//! in-memory vault fake. NO real network, NO real filesystem (house
//! rules). Every flow (device start/poll, loopback build/exchange,
//! refresh incl. Microsoft rotation, `access_token_for` refresh window,
//! vault round-trip) is covered, plus the invariant that a token never
//! reaches an error or `Debug` string.

use super::*;
use crate::mail::secrets::SecretStore;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

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
fn client_secret_config_is_google_only() {
    // Google Desktop clients REQUIRE a token-endpoint secret; Microsoft
    // device-code is a true public client → no secret, ever.
    let g = OauthProvider::Gmail.config();
    assert_eq!(g.client_secret_placeholder, Some("REPLACE_ME-google-client-secret"));
    assert!(g.client_id_placeholder.contains("REPLACE_ME"));
    let m = OauthProvider::Microsoft.config();
    assert_eq!(m.client_secret_placeholder, None, "Microsoft is a public client");
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
fn client_secret_override_seam() {
    // Override wins over the placeholder (BYO / enterprise seam).
    assert_eq!(
        client_secret(OauthProvider::Gmail, Some("my-byo-secret")),
        Some("my-byo-secret")
    );
    // No override → Google's placeholder secret.
    assert_eq!(
        client_secret(OauthProvider::Gmail, None),
        OauthProvider::Gmail.config().client_secret_placeholder
    );
    // Blank override falls back to the placeholder (mirrors client_id).
    assert_eq!(
        client_secret(OauthProvider::Gmail, Some("   ")),
        OauthProvider::Gmail.config().client_secret_placeholder
    );
    // Microsoft has no secret even with a blank/None override.
    assert_eq!(client_secret(OauthProvider::Microsoft, None), None);
    // But an explicit override still applies if a caller forces one.
    assert_eq!(client_secret(OauthProvider::Microsoft, Some("x")), Some("x"));
}

#[test]
fn provider_config_debug_redacts_client_secret() {
    // The Debug impl must never surface the secret placeholder value.
    let dbg = format!("{:?}", OauthProvider::Gmail.config());
    assert!(
        !dbg.contains("REPLACE_ME-google-client-secret"),
        "ProviderConfig Debug leaked the client_secret: {dbg}"
    );
    assert!(dbg.contains("<redacted>"), "secret should show as <redacted>: {dbg}");
    // Microsoft's None secret prints as None (no leakage, no redaction tag
    // needed since there is no secret).
    let m = format!("{:?}", OauthProvider::Microsoft.config());
    assert!(m.contains("client_secret_placeholder: None"), "{m}");
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
fn device_forms_never_carry_client_secret() {
    // Microsoft device-code is a true public client: neither device_start
    // nor device_poll may put a client_secret on the wire.
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"device_code":"DC","user_code":"WXYZ","verification_uri":"https://x","expires_in":900,"interval":5}"#,
    );
    http.push(
        200,
        r#"{"access_token":"AT","refresh_token":"RT","expires_in":3600,"token_type":"Bearer"}"#,
    );
    device_start(OauthProvider::Microsoft, None, &http).expect("start");
    device_poll(OauthProvider::Microsoft, "DC", None, &http).expect("poll");
    assert_eq!(http.field(0, "client_secret"), None, "device_start carried a secret");
    assert_eq!(http.field(1, "client_secret"), None, "device_poll carried a secret");
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
        None,
        &http,
    )
    .expect("exchange");
    assert_eq!(t.access_token, "AT-lb");
    assert_eq!(t.refresh_token.as_deref(), Some("RT-lb"));
    assert_eq!(http.field(0, "grant_type").as_deref(), Some("authorization_code"));
    assert_eq!(http.field(0, "code").as_deref(), Some("auth-code-xyz"));
    assert_eq!(http.field(0, "redirect_uri").as_deref(), Some("http://127.0.0.1:53017/cb"));
    // Google Desktop client → the exchange form MUST include client_secret
    // (resolved to the provider placeholder here).
    assert_eq!(
        http.field(0, "client_secret").as_deref(),
        OauthProvider::Gmail.config().client_secret_placeholder,
        "Gmail loopback exchange must carry the resolved client_secret"
    );
}

#[test]
fn loopback_exchange_gmail_includes_secret_override_wins() {
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"access_token":"AT","refresh_token":"RT","expires_in":3600,"token_type":"Bearer"}"#,
    );
    loopback_exchange(
        OauthProvider::Gmail,
        "code",
        "http://127.0.0.1:1/cb",
        None,
        Some("byo-secret-123"),
        &http,
    )
    .expect("exchange");
    // An override secret wins over the placeholder on the wire.
    assert_eq!(http.field(0, "client_secret").as_deref(), Some("byo-secret-123"));
}

#[test]
fn loopback_exchange_microsoft_omits_secret() {
    // Microsoft is a public client: even via the loopback exchange path
    // its form must carry NO client_secret (byte-for-byte unchanged).
    let http = MockHttp::new();
    http.push(
        200,
        r#"{"access_token":"AT","refresh_token":"RT","expires_in":3600,"token_type":"Bearer"}"#,
    );
    loopback_exchange(
        OauthProvider::Microsoft,
        "code",
        "http://127.0.0.1:1/cb",
        None,
        None,
        &http,
    )
    .expect("exchange");
    assert_eq!(http.field(0, "client_secret"), None, "MS exchange must carry no secret");
    // The form is exactly the four public-client fields.
    let keys: Vec<String> = http.request(0).1.into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec!["client_id", "code", "redirect_uri", "grant_type"]);
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
    let t = refresh(OauthProvider::Microsoft, "RT-old", None, None, &http).expect("refresh");
    assert_eq!(t.access_token, "AT-new");
    assert_eq!(t.refresh_token.as_deref(), Some("RT-new"), "MS rotates → new RT present");
    assert_eq!(http.field(0, "grant_type").as_deref(), Some("refresh_token"));
    assert_eq!(http.field(0, "refresh_token").as_deref(), Some("RT-old"));
    // Microsoft is a public client → NO client_secret on its refresh form.
    assert_eq!(http.field(0, "client_secret"), None, "MS refresh must carry no secret");
}

#[test]
fn refresh_google_omits_refresh_token() {
    let http = MockHttp::new();
    http.push(200, r#"{"access_token":"AT-g","expires_in":3600,"token_type":"Bearer"}"#);
    let t = refresh(OauthProvider::Gmail, "RT-keep", None, None, &http).expect("refresh");
    assert_eq!(t.access_token, "AT-g");
    assert_eq!(t.refresh_token, None, "Google omits RT on refresh (keep the old one)");
    // Google Desktop clients MUST send client_secret on refresh too.
    assert_eq!(
        http.field(0, "client_secret").as_deref(),
        OauthProvider::Gmail.config().client_secret_placeholder,
        "Gmail refresh must carry the resolved client_secret"
    );
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
        None,
        &http,
    )
    .unwrap_err();
    assert!(matches!(err, OauthError::Vault(_)));
    assert_eq!(http.calls(), 0);
}

// ── M1: per-inbox single-flight around the refresh cycle ─────────────────

/// A counting, rotating HTTP mock for the M1 concurrency tests. Every
/// `post_form` here is a refresh POST: it records the presented
/// `refresh_token`, bumps the POST counter, then either rendezvous on a
/// shared [`Barrier`] (to PROVE two refreshes run in parallel) or sleeps
/// `delay` (to widen the single-flight race window), and finally returns a
/// UNIQUELY rotated bundle — modeling Microsoft rotation (a brand-new
/// `refresh_token` on every refresh), which is what makes a racing second
/// refresh able to persist an already-revoked token.
struct RaceHttp {
    posts: Mutex<usize>,
    seen_refresh: Mutex<Vec<String>>,
    seq: Mutex<usize>,
    barrier: Option<Arc<Barrier>>,
    delay: Duration,
}

impl RaceHttp {
    fn new(barrier: Option<Arc<Barrier>>, delay: Duration) -> Self {
        Self {
            posts: Mutex::new(0),
            seen_refresh: Mutex::new(Vec::new()),
            seq: Mutex::new(0),
            barrier,
            delay,
        }
    }
    fn posts(&self) -> usize {
        *self.posts.lock().unwrap()
    }
    fn seen_refresh(&self) -> Vec<String> {
        self.seen_refresh.lock().unwrap().clone()
    }
}

impl HttpClient for RaceHttp {
    fn post_form(&self, _url: &str, form: &[(&str, &str)]) -> Result<HttpResponse, OauthError> {
        if let Some((_, v)) = form.iter().find(|(k, _)| *k == "refresh_token") {
            self.seen_refresh.lock().unwrap().push(v.to_string());
        }
        *self.posts.lock().unwrap() += 1;
        if let Some(b) = &self.barrier {
            b.wait();
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let n = {
            let mut s = self.seq.lock().unwrap();
            *s += 1;
            *s
        };
        // Microsoft-style rotation: a fresh access token AND a NEW refresh
        // token on every refresh.
        let body = format!(
            r#"{{"access_token":"AT{n}","refresh_token":"RT-rot-{n}","expires_in":3600,"token_type":"Bearer"}}"#
        );
        Ok(HttpResponse { status: 200, body })
    }
}

#[test]
fn access_token_for_single_flight_one_refresh_per_inbox() {
    // Two concurrent callers on the SAME expired inbox. Without the
    // single-flight lock both would load RT1 and both would POST
    // grant_type=refresh_token; Microsoft rotates RT1→RT2 (revoking RT1),
    // so the racing second writer could persist an already-revoked token
    // and brick the inbox. Prove exactly ONE refresh happens.
    let store = MemStore::default();
    seed(&store, "m1-single", "AT-old", Some("RT1"));
    // 50ms in the (single) refresh guarantees the second caller is blocked
    // on the per-inbox lock while the first refreshes.
    let http = RaceHttp::new(None, Duration::from_millis(50));
    let start = Barrier::new(2);
    let now = 1_000_000; // far past the seeded expiry → both take the slow path

    let (a, b) = std::thread::scope(|s| {
        let h1 = s.spawn(|| {
            start.wait();
            access_token_for(
                &store,
                "m1-single",
                OauthProvider::Microsoft,
                None, // unknown expiry → force the refresh path
                now,
                None,
                None,
                &http,
            )
            .expect("caller 1")
        });
        let h2 = s.spawn(|| {
            start.wait();
            access_token_for(
                &store,
                "m1-single",
                OauthProvider::Microsoft,
                None,
                now,
                None,
                None,
                &http,
            )
            .expect("caller 2")
        });
        (h1.join().unwrap(), h2.join().unwrap())
    });

    // Exactly ONE network refresh despite two concurrent callers.
    assert_eq!(http.posts(), 1, "single-flight: exactly one refresh POST per inbox");
    // That one refresh presented RT1 — the second caller never POSTed at
    // all, so a now-revoked RT1 can never be re-presented (or re-persisted).
    assert_eq!(http.seen_refresh(), vec!["RT1".to_string()]);
    // Both callers got the SAME freshly-minted, valid access token.
    assert_eq!(a.access_token, "AT1");
    assert_eq!(b.access_token, "AT1");
    assert!(a.refreshed && b.refreshed);
    // The PERSISTED refresh token is the ROTATED one (RT-rot-1), never the
    // stale/revoked RT1 — the last-writer-wins corruption is impossible.
    let reloaded = load_tokens(&store, "m1-single").expect("reload");
    assert_eq!(reloaded.refresh_token.as_deref(), Some("RT-rot-1"));
    assert_eq!(reloaded.access_token, "AT1");
}

#[test]
fn access_token_for_fast_path_no_network_under_concurrency() {
    // A still-valid token returns via the fast path with NO refresh POST,
    // even under concurrent callers (the fast path takes no lock at all).
    let store = MemStore::default();
    seed(&store, "m1-fast", "AT-valid", Some("RT1"));
    let http = RaceHttp::new(None, Duration::ZERO);
    let now = 1_000; // exp is far ahead → always fresh
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                let f = access_token_for(
                    &store,
                    "m1-fast",
                    OauthProvider::Microsoft,
                    Some(1_000_000),
                    now,
                    None,
                    None,
                    &http,
                )
                .expect("fast path");
                assert_eq!(f.access_token, "AT-valid");
                assert!(!f.refreshed);
            });
        }
    });
    assert_eq!(http.posts(), 0, "fast path must never hit the network");
}

#[test]
fn access_token_for_different_inboxes_refresh_concurrently() {
    // Distinct row_ids must NOT serialize against each other — they get
    // distinct locks.
    assert!(
        !Arc::ptr_eq(&refresh_lock_for("m1-a"), &refresh_lock_for("m1-b")),
        "distinct inboxes must not share a refresh lock"
    );

    let store = MemStore::default();
    seed(&store, "m1-a", "AT-a-old", Some("RTa"));
    seed(&store, "m1-b", "AT-b-old", Some("RTb"));
    // A 2-party barrier INSIDE the refresh proves both refreshes are in
    // flight simultaneously: if the two inboxes serialized on one lock the
    // second would never reach the barrier and the test would hang.
    let barrier = Arc::new(Barrier::new(2));
    let http = RaceHttp::new(Some(barrier), Duration::ZERO);
    let now = 1_000_000;

    let (a, b) = std::thread::scope(|s| {
        let h1 = s.spawn(|| {
            access_token_for(&store, "m1-a", OauthProvider::Microsoft, None, now, None, None, &http)
                .expect("inbox a")
        });
        let h2 = s.spawn(|| {
            access_token_for(&store, "m1-b", OauthProvider::Microsoft, None, now, None, None, &http)
                .expect("inbox b")
        });
        (h1.join().unwrap(), h2.join().unwrap())
    });

    // Both inboxes refreshed independently → two POSTs.
    assert_eq!(http.posts(), 2, "distinct inboxes each perform their own refresh");
    let mut seen = http.seen_refresh();
    seen.sort();
    assert_eq!(seen, vec!["RTa".to_string(), "RTb".to_string()]);
    assert!(a.refreshed && b.refreshed);
    // Each inbox persisted its OWN distinct rotated token.
    let ra = load_tokens(&store, "m1-a").expect("reload a");
    let rb = load_tokens(&store, "m1-b").expect("reload b");
    assert!(ra.refresh_token.as_deref().unwrap().starts_with("RT-rot-"));
    assert!(rb.refresh_token.as_deref().unwrap().starts_with("RT-rot-"));
    assert_ne!(ra.refresh_token, rb.refresh_token, "each inbox got a distinct rotated token");
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
    let err = refresh(OauthProvider::Gmail, "RT", None, None, &http).unwrap_err();
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
