//! Minimal typed client for Stalwart's management API (JMAP).
//!
//! BOUNDARY (PRD §4 / pre-mortem #2): this is the ONLY way K2 talks to
//! Stalwart — plain HTTP against its public management API, over
//! localhost, authenticated with basic auth (bootstrap window /
//! provisioned admin) or the ApiKey the supervisor mints. No Stalwart
//! crate is ever linked.
//!
//! TLS: management traffic is PLAIN HTTP ON THE LOOPBACK ONLY (the
//! supervisor's `STALWART_MGMT_URL` decision — see the doc comment
//! there). This client therefore never needs
//! `danger_accept_invalid_certs`; the default-verifying TLS stack
//! stays intact for any https URL it is ever handed.
//!
//! ENDPOINT DISCOVERY: the API path is read from the JMAP session
//! document at `GET /jmap/session` ([`StalwartClient::discover_session`]).
//! ✔ LIVE-VERIFIED v0.16.10 (2026-07-10): `/.well-known/jmap` returns
//! an EMPTY body on the real binary; `/jmap/session` works in both
//! bootstrap and normal mode. In NORMAL mode the served `apiUrl` /
//! `downloadUrl` are ABSOLUTE URLS on the mail HOSTNAME
//! (`https://mail.<domain>/jmap/`) which is not how the daemon dials
//! the loopback — [`parse_session_api_url`] therefore keeps only the
//! PATH and rebases it onto our own `base_url`.
//!
//! ── v0.16.10 MANAGEMENT MODEL (all ✔ LIVE-VERIFIED 2026-07-10 on the
//!    pinned binary, k2-sandbox-01) ────────────────────────────────────
//! Stalwart v0.16 keeps ALL runtime configuration in a JMAP "registry"
//! inside the data store. Management methods are spelled
//! `x:<ObjectType>/get|set|query` under the `urn:stalwart:jmap`
//! capability; there are NO `Domain/*`/`Account/*`/`Settings/*`/
//! `Registry/*` top-level methods (all answer `unknownMethod`).
//! Verified object shapes this module encodes:
//!   1. ✔ `x:Bootstrap/set` — bootstrap mode's ONLY settable object
//!      (id literally `"singleton"`). Update carries `serverHostname`,
//!      `defaultDomain`, `dataStore` (`{"@type":"RocksDb","path":…}`),
//!      `requestTlsCertificate`, `generateDkimKeys`; the reply's
//!      `updated.singleton` returns the PROVISIONED ADMIN
//!      (`{username: "admin@<domain>", secret: …}`) — no journal
//!      scraping, no password rotation step needed.
//!   2. ✔ `x:NetworkListener/get|set` — listeners are registry objects
//!      (`name`, `bind: {"<addr:port>": true}`, `protocol`,
//!      `tlsImplicit`…). Defaults after bootstrap: smtp:25,
//!      submissions:465, imaps:993, pop3s:995, sieve:4190, https:443,
//!      http:8080. Listener changes need a RESTART (set succeeds but
//!      the sockets only move after `systemctl restart` — verified).
//!   3. ✔ `x:Account/set` — accounts are `{"@type":"User", name,
//!      domainId, credentials: {"0": {"@type":"Password", secret}},
//!      roles: {"@type":"Admin"|"User"}, quotas: {maxDiskQuota,
//!      maxEmails}}`. Lists serialize as INDEX-KEYED OBJECTS (a JSON
//!      array for `credentials` is rejected with `invalidPatch` —
//!      verified). v0.16 has NO account enable/disable flag; retire is
//!      a RENAME (delivery to the old address then answers
//!      `550 5.1.2 Mailbox does not exist` — verified).
//!   4. ✔ `x:ApiKey/set` — the ApiKey rides the TARGET account
//!      (request `accountId` = the service account); create returns
//!      `{id, secret: "API_…"}` once; the secret is a Bearer token.
//!      `permissions: {"@type":"Inherit"}` + `allowedIps` pin.
//!   5. ✔ `x:Domain/set|get|query` — create takes `name`,
//!      `dkimManagement {"@type":"Automatic"}` (defaults fill in),
//!      `subAddressing {"@type":"Enabled"}`, `catchAllAddress: null`;
//!      the create reply carries ONLY the id. `x:Domain/get` serves the
//!      computed `dnsZoneFile` (BIND text: MX/SPF/DKIM/DMARC/SRV/…);
//!      DKIM rows appear ~1 s after create (background task).
//!      DESTROY is refused with `objectIsLinked` while DkimSignature
//!      objects reference the domain — [`StalwartClient::domain_delete`]
//!      cascades `x:DkimSignature/query {domainId}` + destroy first.
//!   6. ✔ Delegated mail access (S4/S5): the service account is an
//!      admin-role principal; standard RFC 8620 delegation (method
//!      `accountId` = the member account, HTTP auth = service ApiKey)
//!      works for Mailbox/Email/Identity/EmailSubmission — verified
//!      end-to-end including a combined Email/set + EmailSubmission/set
//!      submit landing in another local mailbox.
//!   7. ✔ S6 relay: a smart host is an `x:MtaRoute` object
//!      (`{"@type":"Relay", name, address, port, protocol:"smtp",
//!      implicitTls, authUsername, authSecret:{"@type":"Value",
//!      secret}}`) BOUND per sender domain through the
//!      `x:MtaOutboundStrategy` SINGLETON's `route` expression
//!      (`{match: {"0": {if, then}, …}, else: "'mx'"}`) — K2 inserts
//!      `sender_domain == '<domain>'` → `'<route name>'`.
//!   8. `x:Certificate/set` create — PEM as PublicText/SecretText
//!      (`certificate: {"@type":"Text","value"}`, `privateKey:
//!      {"@type":"Text","secret"}`) then `x:SystemSettings/set`
//!      singleton `defaultCertificateId` (same id as
//!      `defaultHostname`). Stalwart 0.16.10 stores TLS in RocksDB
//!      (ProtectHome=yes cannot read `~/.k2/certs`). Field name from
//!      Stalwart 0.16 docs; method errors are not swallowed.
//!
//! ── ⚠ STILL LIVE-BOX (not yet verified) ────────────────────────────
//!   a. `x:Account/set update {name}` length limit: the retire rename
//!      truncates to 64 chars ([`retired_local_part`]) but the server's
//!      exact local-part validation was not probed.
//!   b. ApiKey permissions use `{"@type":"Inherit"}` (the key inherits
//!      the admin-role service account). A least-privilege `Replace`
//!      permission list is a follow-up — the permission NAME list
//!      (camelCase `Permission` variants, e.g. `sysDomainCreate`) is
//!      known from source but an exhaustive working set was not
//!      assembled/verified.

use std::sync::Mutex;
use std::time::Duration;

use super::supervisor::AdminCredentials;

/// Authentication for one client: basic auth (bootstrap window /
/// provisioned admin) or the minted ApiKey as a bearer token.
#[derive(Clone)]
pub enum Auth {
    Bearer(String),
    Basic { username: String, password: String },
}

/// Client for one Stalwart instance's management API.
///
/// `base_url` is `mail_server.api_url` (`http://127.0.0.1:8180`, the
/// loopback-only plain-HTTP mgmt listener) or the bootstrap-window
/// listener (`http://127.0.0.1:8080`). Secrets are passed here
/// directly (resolved from the daemon's secret store by the caller —
/// never the ref). Never logged.
pub struct StalwartClient {
    base_url: String,
    auth: Auth,
    /// One session-document fetch per client instance (pre-mortem #10:
    /// don't melt the mgmt API — clients are per-operation or
    /// per-request, so staleness is bounded).
    session_cache: Mutex<Option<Session>>,
}

/// The discovered slice of the JMAP session document the client needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The API endpoint REBASED onto our loopback `base_url`.
    pub api_url: String,
    /// The authenticated principal's account id (registry calls carry
    /// it as `accountId`). ✔ LIVE-VERIFIED: `d333333` for the
    /// bootstrap-mode recovery admin, a normal short id (`"b"`) for the
    /// provisioned admin.
    pub account_id: String,
}

/// Request timeout for mgmt calls — localhost, so generous is still
/// snappy; long-poll reads (S4 `wait`) will use their own client.
const MGMT_TIMEOUT: Duration = Duration::from_secs(15);

/// The JMAP session document path. ✔ LIVE-VERIFIED v0.16.10:
/// `/.well-known/jmap` is EMPTY on the real binary; this path answers
/// in both bootstrap and normal mode.
const SESSION_PATH: &str = "/jmap/session";

/// JMAP core capability URN (RFC 8620) — always in `using`.
const JMAP_CORE_CAPABILITY: &str = "urn:ietf:params:jmap:core";

/// ✔ LIVE-VERIFIED v0.16.10: the capability URN for the `x:*` registry
/// (management) methods.
const STALWART_CAPABILITY: &str = "urn:stalwart:jmap";

/// The client-chosen creation tag inside `*/set create` — any string
/// works (JMAP creation ids are caller-chosen); ours is stable so
/// fixtures and parsers agree.
const CREATE_TAG: &str = "k2";

/// Once-secret from `x:AppPassword/set` create (`created.k2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedAppPassword {
    pub id: String,
    pub secret: String,
}

/// List row from `x:AppPassword/get` — never carries `secret`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppPasswordInfo {
    pub id: String,
    pub description: String,
    pub created_at: serde_json::Value,
}

/// One `x:BlockedIp` / `x:AllowedIp` row (prd-hostmail-bans-v1).
#[derive(Debug, Clone, PartialEq)]
pub struct IpListEntry {
    pub id: String,
    pub address: String,
    pub reason: Option<String>,
    pub created_at: serde_json::Value,
    pub expires_at: serde_json::Value,
}

/// `x:Security` singleton `authBanRate` — `{count, period}` or null.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthBanRate {
    pub count: u64,
    pub period: u64,
}

/// Parsed `x:Security/get` singleton slice we care about.
#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySettings {
    pub auth_ban_rate: Option<AuthBanRate>,
}

/// One `x:DkimSignature` object. `privateKey` is never requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DkimSignature {
    pub id: String,
    pub selector: String,
    pub type_name: String,
    pub stage: String,
    pub public_key: Option<String>,
    pub next_transition_at: Option<String>,
}

/// Result of [`StalwartClient::dkim_force_rotate`]. Previous ids are
/// kept (retire / domain-remove destroy them).
#[derive(Debug, Clone)]
pub struct DkimRotatePlan {
    pub previous: Vec<DkimSignature>,
    pub minted: Vec<DkimSignature>,
    pub used_pem_fallback: bool,
}

#[derive(Clone, Copy)]
enum DkimAlg {
    Ed25519,
    Rsa,
}

fn utc_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn dkim_poll_attempts() -> u32 {
    std::env::var("K2_DKIM_POLL_ATTEMPTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20)
}

fn dkim_poll_ms() -> u64 {
    std::env::var("K2_DKIM_POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
}

fn unique_selector(taken: &std::collections::HashSet<String>, base: &str) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    for n in 2..50 {
        let s = format!("{base}-{n}");
        if !taken.contains(&s) {
            return s;
        }
    }
    format!("{base}-x")
}

fn generate_dkim_pem(alg: DkimAlg) -> Result<String, String> {
    let kp = match alg {
        DkimAlg::Ed25519 => rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)
            .map_err(|e| format!("ed25519 dkim pem: {e}"))?,
        DkimAlg::Rsa => rcgen::KeyPair::generate_for(&rcgen::PKCS_RSA_SHA256)
            .map_err(|e| format!("rsa dkim pem: {e}"))?,
    };
    Ok(kp.serialize_pem())
}

impl StalwartClient {
    /// Bearer-auth client (steady state: the minted ApiKey).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_auth(base_url, Auth::Bearer(api_key.into()))
    }

    /// Basic-auth client (bootstrap window: recovery admin; normal
    /// mode: the provisioned `admin@<domain>`).
    pub fn new_basic(
        base_url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self::with_auth(
            base_url,
            Auth::Basic { username: username.into(), password: password.into() },
        )
    }

    fn with_auth(base_url: impl Into<String>, auth: Auth) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self { base_url, auth, session_cache: Mutex::new(None) }
    }

    fn http() -> Result<reqwest::blocking::Client, String> {
        reqwest::blocking::Client::builder()
            .timeout(MGMT_TIMEOUT)
            .build()
            .map_err(|e| format!("mgmt http client: {e}"))
    }

    fn apply_auth(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        match &self.auth {
            Auth::Bearer(key) => req.bearer_auth(key),
            Auth::Basic { username, password } => req.basic_auth(username, Some(password)),
        }
    }

    /// GET `base_url + path` (path must start with `/`), parse JSON.
    /// Errors are one-line: status + a short body excerpt — never the
    /// credential.
    pub fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .apply_auth(Self::http()?.get(&url))
            .send()
            .map_err(|e| format!("GET {path}: {e}"))?;
        Self::json_or_err("GET", path, resp)
    }

    /// POST a JSON body to an ABSOLUTE url, parse the JSON reply.
    pub fn post_json_url(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        // Manual JSON body: the crate's reqwest is built without the
        // `json` feature (same minimal feature set the rest of the
        // daemon uses) — serialize + set the header ourselves.
        let payload =
            serde_json::to_string(body).map_err(|e| format!("POST {url}: body serialize: {e}"))?;
        let resp = self
            .apply_auth(Self::http()?.post(url))
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| format!("POST {url}: {e}"))?;
        Self::json_or_err("POST", url, resp)
    }

    fn json_or_err(
        method: &str,
        path: &str,
        resp: reqwest::blocking::Response,
    ) -> Result<serde_json::Value, String> {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            let excerpt: String = text.chars().take(200).collect();
            return Err(format!("{method} {path}: HTTP {status}: {excerpt}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("{method} {path}: invalid JSON: {e}"))
    }

    /// Discover the API endpoint + our account id from the session
    /// document (cached per client instance).
    pub fn discover_session(&self) -> Result<Session, String> {
        if let Some(hit) = self
            .session_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Ok(hit);
        }
        let doc = self.get_json(SESSION_PATH)?;
        let session = Session {
            api_url: parse_session_api_url(&self.base_url, &doc)?,
            account_id: parse_session_account_id(&doc)?,
        };
        *self
            .session_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(session.clone());
        Ok(session)
    }

    /// The discovered (rebased) API url.
    pub fn discover_api_url(&self) -> Result<String, String> {
        self.discover_session().map(|s| s.api_url)
    }

    /// Authenticated liveness ping: the session document itself (any
    /// authed principal can fetch it; a 401/refused/parse failure is a
    /// health `degraded`).
    pub fn ping(&self) -> Result<(), String> {
        self.discover_session().map(|_| ())
    }

    /// One REGISTRY (`x:*`) method call: injects our session
    /// `accountId`, wraps in the core+stalwart envelope, unwraps the
    /// method response. A JMAP-level `error` reply is an `Err`.
    fn registry_call(
        &self,
        method: &str,
        mut args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let session = self.discover_session()?;
        if args.get("accountId").is_none() {
            args["accountId"] = serde_json::Value::String(session.account_id);
        }
        let resp = self.post_json_url(&session.api_url, &registry_envelope(method, args))?;
        parse_method_response(method, &resp)
    }

    // ── S1 bootstrap + provisioning calls ───────────────────────────

    /// ✔ LIVE-VERIFIED: complete Stalwart's guided setup in bootstrap
    /// mode — one `x:Bootstrap/set` update of the `"singleton"` object.
    /// Persists the flat DataStore JSON to the config path (normal mode
    /// after restart) and PROVISIONS the permanent admin, whose
    /// credentials ride back in the reply (`updated.singleton`). The
    /// returned secret is vaulted by the caller and never logged.
    pub fn bootstrap_complete(
        &self,
        hostname: &str,
        default_domain: &str,
        request_tls_certificate: bool,
    ) -> Result<AdminCredentials, String> {
        let args = serde_json::json!({
            "update": {
                BOOTSTRAP_SINGLETON_ID: {
                    "serverHostname": hostname,
                    "defaultDomain": default_domain,
                    "requestTlsCertificate": request_tls_certificate,
                    "generateDkimKeys": true,
                    "dataStore": {
                        "@type": "RocksDb",
                        "path": format!("{}/data", super::supervisor::STALWART_DATA_DIR),
                    },
                }
            }
        });
        let resp = self.registry_call("x:Bootstrap/set", args)?;
        parse_bootstrap_updated(&resp)
    }

    /// Normal-mode SMTP banner hostname. Bootstrap writes
    /// `serverHostname` on `x:Bootstrap/set` once; after that the live
    /// object is the `x:SystemSettings` singleton's `defaultHostname`
    /// (SMTP greetings / MTA reports). Registry set — not re-bootstrap,
    /// not a unit wipe.
    pub fn set_server_hostname(&self, hostname: &str) -> Result<(), String> {
        let host = hostname.trim();
        if host.is_empty() {
            return Err("set_server_hostname: empty hostname".to_string());
        }
        let resp = self.registry_call(
            "x:SystemSettings/set",
            serde_json::json!({
                "update": {
                    SYSTEM_SETTINGS_SINGLETON_ID: { "defaultHostname": host }
                }
            }),
        )?;
        parse_set_updated(
            "x:SystemSettings/set",
            SYSTEM_SETTINGS_SINGLETON_ID,
            &resp,
        )
    }

    /// Plant a PEM chain + key as a Stalwart `Certificate` and point
    /// `SystemSettings.defaultCertificateId` at it so 443/465 SNI (and
    /// no-SNI) pick it up. Does not SIGTERM, wipe, or hostmail
    /// disable+enable. Errors include the JMAP method body.
    pub fn certificate_plant(&self, chain_pem: &str, key_pem: &str) -> Result<String, String> {
        let chain = chain_pem.trim();
        let key = key_pem.trim();
        if chain.is_empty() || key.is_empty() {
            return Err("certificate_plant: empty chain or private key".to_string());
        }
        let resp = self.registry_call("x:Certificate/set", certificate_create_args(chain, key))?;
        let id = parse_set_created_id("x:Certificate/set", &resp)?;
        let resp = self.registry_call(
            "x:SystemSettings/set",
            default_certificate_id_args(&id),
        )?;
        parse_set_updated(
            "x:SystemSettings/set",
            SYSTEM_SETTINGS_SINGLETON_ID,
            &resp,
        )?;
        Ok(id)
    }

    /// Retry ACME for the **mail hostname only** (C8/C24 — no extra
    /// SAN names in the request). Stalwart has no dedicated "renew now"
    /// method; the documented on-demand path is `x:Task/set` create of
    /// an `AcmeRenewal` task bound to the hostname's Domain. Does not
    /// SIGTERM, wipe, or re-bootstrap.
    pub fn renew_acme_for_mail_hostname(&self, hostname: &str) -> Result<String, String> {
        let host = hostname.trim();
        if host.is_empty() {
            return Err("renew_acme_for_mail_hostname: empty hostname".to_string());
        }
        let domain_id = match self.domain_query_id(host)? {
            Some(id) => id,
            None => {
                let parent = super::supervisor::default_domain_for(host);
                self.domain_query_id(&parent)?.ok_or_else(|| {
                    format!(
                        "no Stalwart domain for mail hostname '{host}' — cannot retry ACME"
                    )
                })?
            }
        };
        let due = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "@type": "AcmeRenewal",
                    "domainId": domain_id,
                    "status": { "@type": "Pending", "due": due },
                }
            }
        });
        let resp = self.registry_call("x:Task/set", args)?;
        parse_set_created_id("x:Task/set", &resp)
    }

    /// ✔ LIVE-VERIFIED: list the registry's network listeners
    /// (id + name are all the port plan needs).
    pub fn listeners_get(&self) -> Result<Vec<ListenerInfo>, String> {
        let resp = self.registry_call("x:NetworkListener/get", serde_json::json!({}))?;
        parse_listeners(&resp)
    }

    /// Bind bootstrap/recovery HTTP to loopback `:8080` (never `*:8080`
    /// / `[::]:8080`). Does not touch the public HTTPS listener
    /// (`/login` on :443 is a follow-up — do not close here).
    pub fn listeners_bind_setup_loopback(
        &self,
        listeners: &[ListenerInfo],
    ) -> Result<(), String> {
        let mut update = serde_json::Map::new();
        for l in listeners {
            if l.name == "http" || l.name == "http-recovery" {
                update.insert(
                    l.id.clone(),
                    serde_json::json!({ "bind": { STALWART_SETUP_BIND: true } }),
                );
            }
        }
        if update.is_empty() {
            return Err(
                "no http listener to bind on 127.0.0.1:8080 — refusing a world :8080 leftover"
                    .to_string(),
            );
        }
        let args = serde_json::json!({ "update": update });
        let resp = self.registry_call("x:NetworkListener/set", args)?;
        expect_set_clean("x:NetworkListener/set", &resp)
    }

    /// ✔ LIVE-VERIFIED: apply the §5.3 port plan in ONE
    /// `x:NetworkListener/set`:
    /// - destroy default `pop3s`/`sieve` (§10). Keep/create `imap` :143
    ///   STARTTLS and `imaps` :993 implicit (Mail.app). Do **not**
    ///   destroy `imaps` anymore (L6 leftover closed 2026-09-18).
    /// - move the default plain-HTTP `http` listener from `[::]:8080`
    ///   to the LOOPBACK mgmt bind (`127.0.0.1:8180`) — this is both
    ///   "setup listener off" (pre-mortem #13) and the permanent mgmt
    ///   endpoint in one move;
    /// - bind `https` per the port plan (`[::]:443` for `tls-alpn` only
    ///   when Caddy is not the box :443 Host router; loopback
    ///   `127.0.0.1:8443` for http-01 / dns-01 — never `[::]:443` then);
    /// - create the missing STARTTLS `submission` listener on :587.
    /// NOTE (✔ live-verified): the set succeeds but sockets only move
    /// on the supervisor's final RESTART. Public `/login` on :443 is
    /// a follow-up (do not close here).
    pub fn listeners_apply(
        &self,
        port_plan: &str,
        listeners: &[ListenerInfo],
    ) -> Result<(), String> {
        let https_bind = super::preflight::https_listener_bind(port_plan);
        let mut destroy: Vec<String> = Vec::new();
        let mut update = serde_json::Map::new();
        let mut create = serde_json::Map::new();
        let mut has_submission = false;
        let mut has_imap = false;
        let mut has_imaps = false;
        for l in listeners {
            match l.name.as_str() {
                "pop3s" | "sieve" => destroy.push(l.id.clone()),
                "http" => {
                    update.insert(
                        l.id.clone(),
                        serde_json::json!({ "bind": { STALWART_MGMT_BIND: true } }),
                    );
                }
                "https" => {
                    update.insert(
                        l.id.clone(),
                        serde_json::json!({ "bind": { https_bind: true } }),
                    );
                }
                "submission" => has_submission = true,
                "imap" => has_imap = true,
                "imaps" => has_imaps = true,
                _ => {}
            }
        }
        if !has_submission {
            create.insert(
                CREATE_TAG.to_string(),
                serde_json::json!({
                    "name": "submission",
                    "bind": { "[::]:587": true },
                    "protocol": "smtp",
                    "useTls": true,
                    "tlsImplicit": false,
                }),
            );
        }
        if !has_imap {
            create.insert(
                "imap".to_string(),
                serde_json::json!({
                    "name": "imap",
                    "bind": { "[::]:143": true },
                    "protocol": "imap",
                    "useTls": true,
                    "tlsImplicit": false,
                }),
            );
        }
        if !has_imaps {
            create.insert(
                "imaps".to_string(),
                serde_json::json!({
                    "name": "imaps",
                    "bind": { "[::]:993": true },
                    "protocol": "imap",
                    "useTls": true,
                    "tlsImplicit": true,
                }),
            );
        }
        let args = serde_json::json!({
            "create": create,
            "update": update,
            "destroy": destroy,
        });
        let resp = self.registry_call("x:NetworkListener/set", args)?;
        expect_set_clean("x:NetworkListener/set", &resp)
    }

    /// ✔ LIVE-VERIFIED: find a domain's registry id by name
    /// (`x:Domain/query` with the equality filter `{name}`).
    pub fn domain_query_id(&self, domain: &str) -> Result<Option<String>, String> {
        let resp = self.registry_call(
            "x:Domain/query",
            serde_json::json!({ "filter": { "name": domain } }),
        )?;
        Ok(parse_query_ids(&resp).into_iter().next())
    }

    /// ✔ LIVE-VERIFIED: create the `k2-daemon` service account — an
    /// ADMIN-role `User` account in `domain_id` (delegated mail access
    /// for the S4/S5 flows requires admin-class; see module header
    /// item 6). Returns the account's registry id. The password never
    /// appears in errors or logs.
    pub fn service_account_create(
        &self,
        name: &str,
        domain_id: &str,
        password: &str,
        description: &str,
    ) -> Result<String, String> {
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "@type": "User",
                    "name": name,
                    "domainId": domain_id,
                    "description": description,
                    "roles": { "@type": "Admin" },
                    "credentials": { "0": { "@type": "Password", "secret": password } },
                }
            }
        });
        let resp = self.registry_call("x:Account/set", args)?;
        parse_set_created_id("x:Account/set", &resp)
    }

    /// Find an account id by name (`x:Account/query` `{name}`).
    pub fn account_query_id(&self, name: &str) -> Result<Option<String>, String> {
        let resp = self.registry_call(
            "x:Account/query",
            serde_json::json!({ "filter": { "name": name } }),
        )?;
        Ok(parse_query_ids(&resp).into_iter().next())
    }

    /// Rotate an account's Password credential by username. Fails
    /// loud if the account is missing (leftover recovery principal
    /// `admin` must exist to invalidate).
    pub fn rotate_account_secret(&self, username: &str, new_secret: &str) -> Result<(), String> {
        let account_id = self.account_query_id(username)?.ok_or_else(|| {
            format!("admin account '{username}' not found")
        })?;
        self.account_set_password(&account_id, new_secret)
    }

    /// Rotate an account's Password credential.
    pub fn account_set_password(&self, account_id: &str, new_secret: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:Account/set",
            serde_json::json!({
                "update": {
                    account_id: {
                        "credentials": { "0": { "@type": "Password", "secret": new_secret } }
                    }
                }
            }),
        )?;
        expect_set_clean("x:Account/set", &resp)
    }

    /// Mint an AppPassword on a mailbox User. `account_id` is the
    /// mailbox `stalwart_account_id` (copy of [`Self::api_key_create`]).
    /// Empty `description` becomes `"k2"`. Secret is server-set.
    pub fn app_password_create(
        &self,
        account_id: &str,
        description: &str,
    ) -> Result<CreatedAppPassword, String> {
        let desc = description.trim();
        let desc = if desc.is_empty() { "k2" } else { desc };
        let args = serde_json::json!({
            "accountId": account_id,
            "create": {
                CREATE_TAG: {
                    "description": desc,
                    "permissions": { "@type": "Inherit" },
                    "allowedIps": {},
                }
            }
        });
        let resp = self.registry_call("x:AppPassword/set", args)?;
        if let Some(aid) = resp.get("accountId").and_then(|v| v.as_str()) {
            if aid != account_id {
                return Err(format!(
                    "x:AppPassword/set created on account '{aid}', not mailbox '{account_id}'"
                ));
            }
        }
        parse_app_password_created(&resp)
    }

    /// Query AppPassword ids on a mailbox. No filter (docs filter is
    /// only `expiresAt`; v1 does not require it).
    pub fn app_password_query_ids(&self, account_id: &str) -> Result<Vec<String>, String> {
        let resp = self.registry_call(
            "x:AppPassword/query",
            serde_json::json!({ "accountId": account_id }),
        )?;
        Ok(parse_query_ids(&resp))
    }

    /// List AppPasswords for a mailbox. Properties: id / description /
    /// createdAt — never `secret`.
    pub fn app_password_list(&self, account_id: &str) -> Result<Vec<AppPasswordInfo>, String> {
        let ids = self.app_password_query_ids(account_id)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:AppPassword/get",
            serde_json::json!({
                "accountId": account_id,
                "ids": ids,
                "properties": ["id", "description", "createdAt"],
            }),
        )?;
        Ok(parse_app_password_list(&resp))
    }

    /// Destroy one AppPassword on a mailbox. Callers must prove the id
    /// belongs to that mailbox (query first) before calling this.
    pub fn app_password_destroy(&self, account_id: &str, id: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:AppPassword/set",
            serde_json::json!({
                "accountId": account_id,
                "destroy": [id],
            }),
        )?;
        parse_set_destroyed("x:AppPassword/set", id, &resp)
    }

    /// ✔ LIVE-VERIFIED: mint the service account's ApiKey — request
    /// `accountId` addresses the TARGET account (the key lives in its
    /// credential list); `allowedIps` pins the loopback (pre-mortem
    /// #13). Returns the SECRET (a `API_…` bearer token, shown once).
    /// Permissions are `Inherit` for now (⚠ list item b in the module
    /// header — a Replace-mode least-privilege list is a follow-up).
    pub fn api_key_create(&self, account_id: &str) -> Result<String, String> {
        let args = serde_json::json!({
            "accountId": account_id,
            "create": {
                CREATE_TAG: {
                    "description": "K2 daemon mail supervisor",
                    "permissions": { "@type": "Inherit" },
                    "allowedIps": { "127.0.0.1": true },
                }
            }
        });
        let resp = self.registry_call("x:ApiKey/set", args)?;
        resp["created"][CREATE_TAG]["secret"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "x:ApiKey/set create: no secret in reply".to_string())
    }

    // ── S2 domain calls ─────────────────────────────────────────────

    /// S2 — `x:Domain/set create` with automatic DKIM key generation
    /// (Ed25519 + RSA, defaults fill in), sub-addressing enabled,
    /// catch-all OFF (spam magnet — per-domain opt-in later; PRD §6.1),
    /// manual DNS/certificate management (K2 never controls user DNS).
    /// ✔ LIVE-VERIFIED: the create reply carries ONLY the id — the
    /// zone file is read via [`Self::domain_dns_zonefile`] (DKIM rows
    /// appear ~1 s later; the engine impl in `mail::domains` polls).
    ///
    /// ADOPTION (✔ live-verified necessity): Stalwart's guided setup
    /// already created the DEFAULT domain (it hosts the provisioned
    /// admin account), so `k2 mail domain add <that domain>` must
    /// adopt the existing object instead of earning a
    /// `primaryKeyViolation` — the name is queried first.
    pub fn domain_create(&self, domain: &str) -> Result<CreatedDomain, String> {
        if let Some(id) = self.domain_query_id(domain)? {
            return Ok(CreatedDomain { id, dns_zone_file: None });
        }
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "name": domain,
                    "isEnabled": true,
                    "dkimManagement": { "@type": "Automatic" },
                    "subAddressing": { "@type": "Enabled" },
                    "catchAllAddress": null,
                    "dnsManagement": { "@type": "Manual" },
                    "certificateManagement": { "@type": "Manual" },
                }
            }
        });
        let resp = self.registry_call("x:Domain/set", args)?;
        let id = parse_set_created_id("x:Domain/set", &resp)?;
        Ok(CreatedDomain { id, dns_zone_file: None })
    }

    /// S2 — destroy a domain (the route layer has already required the
    /// explicit confirm + retired the domain's addresses, PRD §6.6).
    /// ✔ LIVE-VERIFIED: a bare destroy is refused with
    /// `objectIsLinked` while the domain's DkimSignature objects exist
    /// — cascade them first.
    pub fn domain_delete(&self, stalwart_domain_id: &str) -> Result<(), String> {
        let dkim = self.registry_call(
            "x:DkimSignature/query",
            serde_json::json!({ "filter": { "domainId": stalwart_domain_id } }),
        )?;
        let dkim_ids = parse_query_ids(&dkim);
        if !dkim_ids.is_empty() {
            let resp = self.registry_call(
                "x:DkimSignature/set",
                serde_json::json!({ "destroy": dkim_ids }),
            )?;
            expect_set_clean("x:DkimSignature/set", &resp)?;
        }
        let resp = self.registry_call(
            "x:Domain/set",
            serde_json::json!({ "destroy": [stalwart_domain_id] }),
        )?;
        parse_set_destroyed("x:Domain/set", stalwart_domain_id, &resp)
    }

    /// S2 — read the domain's computed `dnsZoneFile` (the SSOT for the
    /// record table, PRD §6.2 — K2 computes nothing itself except
    /// relay-mode SPF adjustments). ✔ LIVE-VERIFIED shape.
    pub fn domain_dns_zonefile(&self, stalwart_domain_id: &str) -> Result<String, String> {
        let resp = self.registry_call(
            "x:Domain/get",
            serde_json::json!({
                "ids": [stalwart_domain_id],
                "properties": ["dnsZoneFile"],
            }),
        )?;
        parse_domain_get_zonefile(stalwart_domain_id, &resp)
    }

    /// Read `Domain.catchAllAddress` (full addr or none). Properties
    /// list is catch-all only — do not send `aliases`.
    pub fn domain_get_catchall(
        &self,
        stalwart_domain_id: &str,
    ) -> Result<Option<String>, String> {
        let resp = self.registry_call(
            "x:Domain/get",
            serde_json::json!({
                "ids": [stalwart_domain_id],
                "properties": ["catchAllAddress"],
            }),
        )?;
        parse_domain_get_catchall(stalwart_domain_id, &resp)
    }

    /// Patch **only** `catchAllAddress` (full addr or JSON null).
    /// Never writes `Domain.aliases`.
    pub fn domain_set_catchall(
        &self,
        stalwart_domain_id: &str,
        address: Option<&str>,
    ) -> Result<(), String> {
        let value = match address {
            Some(a) => serde_json::Value::String(a.to_string()),
            None => serde_json::Value::Null,
        };
        let resp = self.registry_call(
            "x:Domain/set",
            serde_json::json!({
                "update": { stalwart_domain_id: { "catchAllAddress": value } }
            }),
        )?;
        parse_set_updated("x:Domain/set", stalwart_domain_id, &resp)
    }

    /// `x:Domain/get` `reportAddressUri` (DMARC/TLS-RPT dest). Default
    /// on a fresh Stalwart domain is `"mailto:postmaster"` — not a
    /// minted inbox. Missing/null → `None`.
    pub fn domain_get_report_address_uri(
        &self,
        stalwart_domain_id: &str,
    ) -> Result<Option<String>, String> {
        let resp = self.registry_call(
            "x:Domain/get",
            serde_json::json!({
                "ids": [stalwart_domain_id],
                "properties": ["reportAddressUri"],
            }),
        )?;
        parse_domain_get_report_address(stalwart_domain_id, &resp)
    }

    /// `x:Domain/set` update `reportAddressUri`. Full `mailto:` URI
    /// (or JSON null). Does not rewrite `_dmarc` TXT.
    pub fn domain_set_report_address_uri(
        &self,
        stalwart_domain_id: &str,
        uri: Option<&str>,
    ) -> Result<(), String> {
        let value = match uri {
            Some(u) => serde_json::Value::String(u.to_string()),
            None => serde_json::Value::Null,
        };
        let resp = self.registry_call(
            "x:Domain/set",
            serde_json::json!({
                "update": { stalwart_domain_id: { "reportAddressUri": value } }
            }),
        )?;
        parse_set_updated("x:Domain/set", stalwart_domain_id, &resp)
    }

    /// `x:DkimSignature/query` filter `domainId`. Empty is ok.
    pub fn dkim_query_ids(&self, stalwart_domain_id: &str) -> Result<Vec<String>, String> {
        let resp = self.registry_call(
            "x:DkimSignature/query",
            serde_json::json!({ "filter": { "domainId": stalwart_domain_id } }),
        )?;
        Ok(parse_query_ids(&resp))
    }

    /// `x:DkimSignature/get` for selector/stage/publicKey. Never
    /// requests `privateKey`.
    pub fn dkim_get(&self, ids: &[String]) -> Result<Vec<DkimSignature>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:DkimSignature/get",
            serde_json::json!({
                "ids": ids,
                "properties": [
                    "selector",
                    "@type",
                    "stage",
                    "publicKey",
                    "nextTransitionAt",
                    "domainId"
                ],
            }),
        )?;
        parse_dkim_get_list(&resp)
    }

    pub fn dkim_list(&self, stalwart_domain_id: &str) -> Result<Vec<DkimSignature>, String> {
        let ids = self.dkim_query_ids(stalwart_domain_id)?;
        self.dkim_get(&ids)
    }

    /// Patch `stage` / `nextTransitionAt` on existing signatures.
    /// Rotate uses this; it must not `destroy`.
    pub fn dkim_update(
        &self,
        updates: &[(String, serde_json::Value)],
    ) -> Result<(), String> {
        if updates.is_empty() {
            return Ok(());
        }
        let mut map = serde_json::Map::new();
        for (id, patch) in updates {
            map.insert(id.clone(), patch.clone());
        }
        let resp = self.registry_call(
            "x:DkimSignature/set",
            serde_json::json!({ "update": map }),
        )?;
        expect_set_clean("x:DkimSignature/set", &resp)?;
        for (id, _) in updates {
            parse_set_updated("x:DkimSignature/set", id, &resp)?;
        }
        Ok(())
    }

    /// PEM-create fallback: `stage` must be `pending` (Stalwart
    /// default is `active` and would sign before DNS).
    pub fn dkim_create_pending(
        &self,
        stalwart_domain_id: &str,
        type_name: &str,
        selector: &str,
        private_key_pem: &str,
    ) -> Result<String, String> {
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "@type": type_name,
                    "domainId": stalwart_domain_id,
                    "selector": selector,
                    "privateKey": { "@type": "Text", "secret": private_key_pem },
                    "stage": "pending",
                }
            }
        });
        let resp = self.registry_call("x:DkimSignature/set", args)?;
        parse_set_created_id("x:DkimSignature/set", &resp)
    }

    /// Destroy **one** selector (retire). Domain-remove cascade is
    /// [`Self::domain_delete`] — rotate must not call this on the
    /// active pair.
    pub fn dkim_destroy(&self, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let resp = self.registry_call(
            "x:DkimSignature/set",
            serde_json::json!({ "destroy": ids }),
        )?;
        expect_set_clean("x:DkimSignature/set", &resp)?;
        for id in ids {
            parse_set_destroyed("x:DkimSignature/set", id, &resp)?;
        }
        Ok(())
    }

    /// Emergency rotate with Manual DNS: poke `nextTransitionAt`, then
    /// `x:Task` `DkimManagement`. If the task path does not mint,
    /// PEM-create a pending Ed25519+RSA pair. Never destroys the
    /// previous ids.
    pub fn dkim_force_rotate(&self, stalwart_domain_id: &str) -> Result<DkimRotatePlan, String> {
        let previous = self.dkim_list(stalwart_domain_id)?;
        let now = utc_now_rfc3339();
        let poke: Vec<(String, serde_json::Value)> = previous
            .iter()
            .filter(|s| s.stage == "active")
            .map(|s| {
                (
                    s.id.clone(),
                    serde_json::json!({ "nextTransitionAt": now }),
                )
            })
            .collect();
        self.dkim_update(&poke)?;

        let task_err = self.task_create_dkim_management(stalwart_domain_id).err();
        let old_ids: std::collections::HashSet<String> =
            previous.iter().map(|s| s.id.clone()).collect();
        let mut minted = if task_err.is_none() {
            self.poll_new_dkim(stalwart_domain_id, &old_ids)?
        } else {
            Vec::new()
        };
        let mut used_pem_fallback = false;
        if minted.is_empty() {
            used_pem_fallback = true;
            minted = self.pem_create_pending_pair(stalwart_domain_id, &previous)?;
        }
        Ok(DkimRotatePlan {
            previous,
            minted,
            used_pem_fallback,
        })
    }

    fn task_create_dkim_management(&self, stalwart_domain_id: &str) -> Result<(), String> {
        let now = utc_now_rfc3339();
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "@type": "DkimManagement",
                    "domainId": stalwart_domain_id,
                    "status": { "@type": "Pending", "due": now },
                }
            }
        });
        let resp = self.registry_call("x:Task/set", args)?;
        parse_set_created_id("x:Task/set", &resp).map(|_| ())
    }

    fn poll_new_dkim(
        &self,
        stalwart_domain_id: &str,
        old_ids: &std::collections::HashSet<String>,
    ) -> Result<Vec<DkimSignature>, String> {
        let attempts = dkim_poll_attempts();
        let sleep_ms = dkim_poll_ms();
        let mut last = Vec::new();
        for i in 0..attempts {
            let all = self.dkim_list(stalwart_domain_id)?;
            last = all
                .into_iter()
                .filter(|s| !old_ids.contains(&s.id))
                .collect();
            if last.len() >= 2 {
                return Ok(last);
            }
            if i + 1 < attempts && sleep_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            }
        }
        Ok(last)
    }

    fn pem_create_pending_pair(
        &self,
        stalwart_domain_id: &str,
        existing: &[DkimSignature],
    ) -> Result<Vec<DkimSignature>, String> {
        let taken: std::collections::HashSet<String> =
            existing.iter().map(|s| s.selector.clone()).collect();
        let date = chrono::Utc::now().format("%Y%m%d").to_string();
        let ed_sel = unique_selector(&taken, &format!("v1-ed25519-{date}"));
        let rsa_sel = unique_selector(&taken, &format!("v1-rsa-{date}"));
        let ed_pem = generate_dkim_pem(DkimAlg::Ed25519)?;
        let rsa_pem = generate_dkim_pem(DkimAlg::Rsa)?;
        let ed_id = self.dkim_create_pending(
            stalwart_domain_id,
            "Dkim1Ed25519Sha256",
            &ed_sel,
            &ed_pem,
        )?;
        let rsa_id = self.dkim_create_pending(
            stalwart_domain_id,
            "Dkim1RsaSha256",
            &rsa_sel,
            &rsa_pem,
        )?;
        self.dkim_get(&[ed_id, rsa_id])
    }

    // ── S3 account calls ────────────────────────────────────────────

    /// S3 — create one mailbox account per minted address (PRD §7.1):
    /// `User` account, local-part `name` bound to `domainId`, the
    /// random vaulted password, §12 quotas (`maxDiskQuota` bytes +
    /// `maxEmails` count — ✔ live-verified key names). Returns the
    /// server-set account id. The password never appears in errors or
    /// logs (errors carry the server's SetError — never our args).
    pub fn account_create(
        &self,
        local_part: &str,
        stalwart_domain_id: &str,
        password: &str,
        quota_bytes: u64,
        max_messages: u64,
    ) -> Result<String, String> {
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "@type": "User",
                    "name": local_part,
                    "domainId": stalwart_domain_id,
                    "credentials": { "0": { "@type": "Password", "secret": password } },
                    "quotas": {
                        "maxDiskQuota": quota_bytes,
                        "maxEmails": max_messages,
                    },
                }
            }
        });
        let resp = self.registry_call("x:Account/set", args)?;
        parse_set_created_id("x:Account/set", &resp)
    }

    /// S3 — disable an account (address retire, PRD §7.2): the alias
    /// stops receiving, mailbox DATA IS KEPT for the retention window
    /// (§12) — never a destroy on the retire path.
    ///
    /// ✔ LIVE-VERIFIED MECHANISM: v0.16 accounts have NO enable flag —
    /// retire is a RENAME to a reserved local part
    /// ([`retired_local_part`]); RCPT to the old address then answers
    /// `550 5.1.2 Mailbox does not exist` while the mailbox (keyed by
    /// account id, not name) keeps its data.
    pub fn account_disable(&self, stalwart_account_id: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:Account/get",
            serde_json::json!({ "ids": [stalwart_account_id], "properties": ["name"] }),
        )?;
        let name = parse_account_get_name(stalwart_account_id, &resp)?;
        if name.contains(RETIRED_MARKER) {
            return Ok(()); // already retired — idempotent
        }
        let renamed = retired_local_part(&name, now_unix());
        let resp = self.registry_call(
            "x:Account/set",
            serde_json::json!({ "update": { stalwart_account_id: { "name": renamed } } }),
        )?;
        parse_set_updated("x:Account/set", stalwart_account_id, &resp)
    }

    /// Patch an existing account's mailbox quotas (`x:Account/set`
    /// update). `None` leaves that field unchanged. 0 = unlimited
    /// (passed through to Stalwart `maxDiskQuota` / `maxEmails`).
    pub fn account_set_quotas(
        &self,
        stalwart_account_id: &str,
        max_disk_quota: Option<u64>,
        max_emails: Option<u64>,
    ) -> Result<(), String> {
        if max_disk_quota.is_none() && max_emails.is_none() {
            return Err("account quota update needs maxDiskQuota and/or maxEmails".to_string());
        }
        let mut quotas = serde_json::Map::new();
        if let Some(bytes) = max_disk_quota {
            quotas.insert("maxDiskQuota".to_string(), serde_json::json!(bytes));
        }
        if let Some(msgs) = max_emails {
            quotas.insert("maxEmails".to_string(), serde_json::json!(msgs));
        }
        let resp = self.registry_call(
            "x:Account/set",
            serde_json::json!({
                "update": { stalwart_account_id: { "quotas": quotas } }
            }),
        )?;
        parse_set_updated("x:Account/set", stalwart_account_id, &resp)
    }

    /// Read an account's current `quotas` object from Stalwart
    /// (`x:Account/get` properties `quotas`). Engine truth — not a K2
    /// cache — so operators can see the live cap after a set.
    pub fn account_get_quotas(
        &self,
        stalwart_account_id: &str,
    ) -> Result<serde_json::Value, String> {
        let resp = self.registry_call(
            "x:Account/get",
            serde_json::json!({
                "ids": [stalwart_account_id],
                "properties": ["quotas"],
            }),
        )?;
        parse_account_get_quotas(stalwart_account_id, &resp)
    }

    /// `x:Account/query` with no name filter — every registry account id.
    pub fn account_query_ids(&self) -> Result<Vec<String>, String> {
        let resp = self.registry_call("x:Account/query", serde_json::json!({}))?;
        Ok(parse_query_ids(&resp))
    }

    /// `x:Account/get` name + domainId + aliases (List<EmailAlias>).
    pub fn account_get_user(&self, stalwart_account_id: &str) -> Result<AccountUser, String> {
        let resp = self.registry_call(
            "x:Account/get",
            serde_json::json!({
                "ids": [stalwart_account_id],
                "properties": ["name", "domainId", "aliases"],
            }),
        )?;
        parse_account_get_user(stalwart_account_id, &resp)
    }

    /// All User accounts with aliases (collision scan).
    pub fn account_list_users(&self) -> Result<Vec<AccountUser>, String> {
        let ids = self.account_query_ids()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:Account/get",
            serde_json::json!({
                "ids": ids,
                "properties": ["name", "domainId", "aliases"],
            }),
        )?;
        parse_account_get_users(&resp)
    }

    /// Replace `Account.aliases` with an **index-keyed object**
    /// (`"0"|"1"|…`). A JSON array is `invalidPatch` on 0.16.10.
    pub fn account_set_aliases(
        &self,
        stalwart_account_id: &str,
        aliases: &[EmailAlias],
    ) -> Result<(), String> {
        let patch = aliases_to_index_map(aliases);
        let resp = self.registry_call(
            "x:Account/set",
            serde_json::json!({
                "update": { stalwart_account_id: { "aliases": patch } }
            }),
        )?;
        parse_set_updated("x:Account/set", stalwart_account_id, &resp)
    }

    /// S3 — destroy an account. COMPENSATING ACTION ONLY (mint
    /// rollback: Stalwart create succeeded but the K2 row write failed
    /// — no orphans). The retire path uses [`Self::account_disable`];
    /// nothing else may call this in V1 (the 90-day purge job that
    /// eventually destroys retired accounts is a later slice — see the
    /// retention seam in `mail::addresses`).
    pub fn account_destroy(&self, stalwart_account_id: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:Account/set",
            serde_json::json!({ "destroy": [stalwart_account_id] }),
        )?;
        parse_set_destroyed("x:Account/set", stalwart_account_id, &resp)
    }

    // ── Next hostmail CLI (prd-hostmail-next-cli-v1) ─────────────────

    /// `x:Account/get` aliases list for collision checks.
    pub fn account_get_aliases(
        &self,
        stalwart_account_id: &str,
    ) -> Result<Vec<AccountAlias>, String> {
        let resp = self.registry_call(
            "x:Account/get",
            serde_json::json!({
                "ids": [stalwart_account_id],
                "properties": ["name", "domainId", "emailAddress", "aliases"],
            }),
        )?;
        parse_account_get_aliases(stalwart_account_id, &resp)
    }

    /// `x:Account/query` by `domainId` — fail loud if the filter is
    /// unknown (do not invent a second listing).
    pub fn account_query_ids_for_domain(
        &self,
        stalwart_domain_id: &str,
    ) -> Result<Vec<String>, String> {
        let resp = self.registry_call(
            "x:Account/query",
            serde_json::json!({ "filter": { "domainId": stalwart_domain_id } }),
        )?;
        Ok(parse_query_ids(&resp))
    }

    /// `x:MailingList/query`. Empty ids is not an error.
    ///
    /// 0.16 docs: filter is **free text or tenant**, not `emailAddress` /
    /// `domainId` (those 502 `unsupportedFilter` on lztek). Omit `filter`
    /// to enumerate.
    pub fn mailing_list_query(
        &self,
        filter: Option<serde_json::Value>,
    ) -> Result<Vec<String>, String> {
        let args = match filter {
            Some(f) => serde_json::json!({ "filter": f }),
            None => serde_json::json!({}),
        };
        let resp = self.registry_call("x:MailingList/query", args)?;
        Ok(parse_query_ids(&resp))
    }

    pub fn mailing_list_get(&self, ids: &[String]) -> Result<Vec<MailingListInfo>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:MailingList/get",
            serde_json::json!({
                "ids": ids,
                "properties": ["id", "name", "domainId", "emailAddress", "recipients", "description"],
            }),
        )?;
        Ok(parse_mailing_list_get(&resp))
    }

    /// Create a mailing list. Recipients is a set object `{addr: true}`.
    /// Do not mint an Account. Do not write Domain.aliases.
    pub fn mailing_list_create(
        &self,
        local_part: &str,
        stalwart_domain_id: &str,
        recipients: &serde_json::Value,
    ) -> Result<String, String> {
        let resp = self.registry_call(
            "x:MailingList/set",
            serde_json::json!({
                "create": {
                    CREATE_TAG: {
                        "name": local_part,
                        "domainId": stalwart_domain_id,
                        "recipients": recipients,
                        "aliases": {},
                    }
                }
            }),
        )?;
        parse_set_created_id("x:MailingList/set", &resp)
    }

    pub fn mailing_list_set_recipients(
        &self,
        list_id: &str,
        recipients: &serde_json::Value,
    ) -> Result<(), String> {
        let resp = self.registry_call(
            "x:MailingList/set",
            serde_json::json!({
                "update": { list_id: { "recipients": recipients } }
            }),
        )?;
        parse_set_updated("x:MailingList/set", list_id, &resp)
    }

    pub fn mailing_list_destroy(&self, list_id: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:MailingList/set",
            serde_json::json!({ "destroy": [list_id] }),
        )?;
        parse_set_destroyed("x:MailingList/set", list_id, &resp)
    }

    /// Probe `x:MemoryLookupKey/query` for a namespace. Success (even
    /// empty ids) means the namespace exists. Fail loud on
    /// `unknownMethod` / invalid filter — caller tries the next name.
    pub fn memory_lookup_query_namespace(&self, namespace: &str) -> Result<Vec<String>, String> {
        let resp = self.registry_call(
            "x:MemoryLookupKey/query",
            serde_json::json!({ "filter": { "namespace": namespace } }),
        )?;
        Ok(parse_query_ids(&resp))
    }

    pub fn memory_lookup_key_create(&self, namespace: &str, key: &str) -> Result<String, String> {
        let resp = self.registry_call(
            "x:MemoryLookupKey/set",
            serde_json::json!({
                "create": {
                    CREATE_TAG: {
                        "namespace": namespace,
                        "key": key,
                        "isGlobPattern": false,
                    }
                }
            }),
        )?;
        parse_set_created_id("x:MemoryLookupKey/set", &resp)
    }

    /// `x:Action/set` create `@type: InvalidateCaches` — lookup lists
    /// do not apply until reload. Not hostmail disable.
    pub fn action_invalidate_caches(&self) -> Result<(), String> {
        let resp = self.registry_call(
            "x:Action/set",
            serde_json::json!({
                "create": { CREATE_TAG: { "@type": "InvalidateCaches" } }
            }),
        )?;
        parse_set_created_id("x:Action/set", &resp).map(|_| ())
    }

    /// Reload blocked-IP set after destroy. Not `InvalidateCaches`.
    pub fn action_reload_blocked_ips(&self) -> Result<(), String> {
        let resp = self.registry_call(
            "x:Action/set",
            serde_json::json!({
                "create": { CREATE_TAG: { "@type": "ReloadBlockedIps" } }
            }),
        )?;
        parse_set_created_id("x:Action/set", &resp).map(|_| ())
    }

    /// Reload settings after allowlist / Security patch. Not
    /// `ReloadBlockedIps` / `InvalidateCaches`.
    pub fn action_reload_settings(&self) -> Result<(), String> {
        let resp = self.registry_call(
            "x:Action/set",
            serde_json::json!({
                "create": { CREATE_TAG: { "@type": "ReloadSettings" } }
            }),
        )?;
        parse_set_created_id("x:Action/set", &resp).map(|_| ())
    }

    pub fn blocked_ip_query(&self) -> Result<Vec<String>, String> {
        let resp = self.registry_call("x:BlockedIp/query", serde_json::json!({}))?;
        Ok(parse_query_ids(&resp))
    }

    pub fn blocked_ip_get(&self, ids: &[String]) -> Result<Vec<IpListEntry>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:BlockedIp/get",
            serde_json::json!({
                "ids": ids,
                "properties": ["id", "address", "reason", "createdAt", "expiresAt"],
            }),
        )?;
        Ok(parse_ip_list_get(&resp))
    }

    pub fn blocked_ip_destroy(&self, ids: &[String]) -> Result<Vec<String>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:BlockedIp/set",
            serde_json::json!({ "destroy": ids }),
        )?;
        parse_set_destroyed_many("x:BlockedIp/set", ids, &resp)
    }

    pub fn allowed_ip_query(&self) -> Result<Vec<String>, String> {
        let resp = self.registry_call("x:AllowedIp/query", serde_json::json!({}))?;
        Ok(parse_query_ids(&resp))
    }

    pub fn allowed_ip_get(&self, ids: &[String]) -> Result<Vec<IpListEntry>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:AllowedIp/get",
            serde_json::json!({
                "ids": ids,
                "properties": ["id", "address", "reason", "createdAt", "expiresAt"],
            }),
        )?;
        Ok(parse_ip_list_get(&resp))
    }

    pub fn allowed_ip_create(
        &self,
        address: &str,
        reason: Option<&str>,
        expires_at: Option<&str>,
    ) -> Result<String, String> {
        let addr = address.trim();
        if addr.is_empty() {
            return Err("allowed_ip_create: empty address".to_string());
        }
        let mut body = serde_json::json!({ "address": addr });
        if let Some(r) = reason.map(str::trim).filter(|s| !s.is_empty()) {
            body["reason"] = serde_json::json!(r);
        }
        body["expiresAt"] = match expires_at.map(str::trim).filter(|s| !s.is_empty()) {
            Some(e) => serde_json::json!(e),
            None => serde_json::Value::Null,
        };
        let resp = self.registry_call(
            "x:AllowedIp/set",
            serde_json::json!({
                "create": { CREATE_TAG: body }
            }),
        )?;
        parse_set_created_id("x:AllowedIp/set", &resp)
    }

    pub fn allowed_ip_destroy(&self, ids: &[String]) -> Result<Vec<String>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:AllowedIp/set",
            serde_json::json!({ "destroy": ids }),
        )?;
        parse_set_destroyed_many("x:AllowedIp/set", ids, &resp)
    }

    /// `x:Security/get` singleton — authBanRate only (never write
    /// authBanPeriod from hostmail).
    pub fn security_get(&self) -> Result<SecuritySettings, String> {
        let resp = self.registry_call(
            "x:Security/get",
            serde_json::json!({
                "ids": [SYSTEM_SETTINGS_SINGLETON_ID],
                "properties": ["authBanRate"],
            }),
        )?;
        parse_security_get(&resp)
    }

    /// Patch `authBanRate` only (object or null). Never `authBanPeriod`.
    pub fn security_set_auth_ban_rate(&self, rate: Option<&AuthBanRate>) -> Result<(), String> {
        let rate_v = match rate {
            None => serde_json::Value::Null,
            Some(r) => serde_json::json!({ "count": r.count, "period": r.period }),
        };
        let resp = self.registry_call(
            "x:Security/set",
            serde_json::json!({
                "update": {
                    SYSTEM_SETTINGS_SINGLETON_ID: { "authBanRate": rate_v }
                }
            }),
        )?;
        parse_set_updated(
            "x:Security/set",
            SYSTEM_SETTINGS_SINGLETON_ID,
            &resp,
        )
    }

    pub fn queued_message_query(&self) -> Result<Vec<String>, String> {
        let resp = self.registry_call("x:QueuedMessage/query", serde_json::json!({}))?;
        Ok(parse_query_ids(&resp))
    }

    pub fn queued_message_get(&self, ids: &[String]) -> Result<Vec<QueuedMessageInfo>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.registry_call(
            "x:QueuedMessage/get",
            serde_json::json!({
                "ids": ids,
                "properties": ["id", "nextRetry", "returnPath", "recipients", "blobId"],
            }),
        )?;
        Ok(parse_queued_message_get(&resp))
    }

    pub fn queued_message_retry(&self, id: &str, next_retry: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:QueuedMessage/set",
            serde_json::json!({
                "update": { id: { "nextRetry": next_retry } }
            }),
        )?;
        parse_set_updated("x:QueuedMessage/set", id, &resp)
    }

    pub fn queued_message_destroy(&self, id: &str) -> Result<(), String> {
        let resp = self.registry_call(
            "x:QueuedMessage/set",
            serde_json::json!({ "destroy": [id] }),
        )?;
        parse_set_destroyed("x:QueuedMessage/set", id, &resp)
    }

    /// Probe Inbox `shareWith`. `Ok(None)` = unknownProperty / omitted
    /// (IMAP SETACL fallback). `Ok(Some)` = JMAP sharing is live.
    pub fn mailbox_probe_share_with(
        &self,
        account_id: &str,
        mailbox_id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        match self.mail_call(
            account_id,
            "Mailbox/get",
            serde_json::json!({
                "ids": [mailbox_id],
                "properties": ["shareWith", "myRights"],
            }),
        ) {
            Ok(args) => Ok(parse_mailbox_share_with(mailbox_id, &args)),
            Err(e) if e.contains("unknownProperty") || e.contains("invalidArguments") => {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub fn mailbox_set_share_with(
        &self,
        account_id: &str,
        mailbox_id: &str,
        share_with: serde_json::Value,
    ) -> Result<(), String> {
        let args = self.mail_call(
            account_id,
            "Mailbox/set",
            serde_json::json!({
                "update": { mailbox_id: { "shareWith": share_with } }
            }),
        )?;
        parse_set_updated("Mailbox/set", mailbox_id, &args)
    }

    pub fn mailbox_id_for_role(&self, account_id: &str, role: &str) -> Result<String, String> {
        if role == "inbox" {
            return self.mailbox_inbox_id(account_id);
        }
        let boxes = self.mailbox_list(account_id)?;
        boxes
            .iter()
            .find(|m| m.role.as_deref() == Some(role))
            .map(|m| m.id.clone())
            .ok_or_else(|| format!("Mailbox/query: the account has no '{role}' mailbox"))
    }

    // ── S6 relay (smart host) ───────────────────────────────────────

    /// S6 — apply (`Some`) or clear (`None`) the SMART-HOST outbound
    /// route for one domain (PRD §8.3). ✔ LIVE-VERIFIED model: one
    /// `x:MtaRoute` Relay object named [`relay_route_name`] +
    /// a `sender_domain == '<domain>'` match inserted into the
    /// `x:MtaOutboundStrategy` singleton's `route` expression.
    /// Ordering: the route object exists BEFORE the expression
    /// references it; on clear the expression is unbound BEFORE the
    /// route is destroyed. The password rides the create/update payload
    /// once and is never logged (errors excerpt the RESPONSE only).
    pub fn relay_route_apply(
        &self,
        domain: &str,
        route: Option<&RelayRoute>,
    ) -> Result<(), String> {
        let route_name = relay_route_name(domain);
        let existing = self.registry_call("x:MtaRoute/get", serde_json::json!({}))?;
        let existing_id = parse_named_object_id(&existing, &route_name);

        if let Some(r) = route {
            let body = serde_json::json!({
                "@type": "Relay",
                "name": route_name,
                "description": format!("K2 smart-host route for {domain}"),
                "address": r.host,
                "port": r.port,
                "protocol": "smtp",
                "implicitTls": r.implicit_tls,
                "authUsername": r.username,
                "authSecret": { "@type": "Value", "secret": r.password },
            });
            let args = match &existing_id {
                Some(id) => serde_json::json!({ "update": { id: body } }),
                None => serde_json::json!({ "create": { CREATE_TAG: body } }),
            };
            let resp = self.registry_call("x:MtaRoute/set", args)?;
            expect_set_clean("x:MtaRoute/set", &resp)?;
            self.outbound_route_rebind(domain, Some(&route_name))
        } else {
            self.outbound_route_rebind(domain, None)?;
            if let Some(id) = existing_id {
                let resp = self.registry_call(
                    "x:MtaRoute/set",
                    serde_json::json!({ "destroy": [id] }),
                )?;
                parse_set_destroyed("x:MtaRoute/set", &id, &resp)?;
            }
            Ok(())
        }
    }

    /// Rewrite the outbound strategy singleton's `route` expression so
    /// `domain`'s outbound picks `route_name` (or falls back to the
    /// default when `None`).
    fn outbound_route_rebind(
        &self,
        domain: &str,
        route_name: Option<&str>,
    ) -> Result<(), String> {
        let got = self.registry_call(
            "x:MtaOutboundStrategy/get",
            serde_json::json!({ "ids": [OUTBOUND_STRATEGY_SINGLETON_ID] }),
        )?;
        let current = got["list"][0]["route"].clone();
        if current.is_null() {
            return Err(
                "x:MtaOutboundStrategy/get: singleton has no route expression".to_string()
            );
        }
        let rewritten = rewrite_route_expression(&current, domain, route_name)?;
        let resp = self.registry_call(
            "x:MtaOutboundStrategy/set",
            serde_json::json!({
                "update": { OUTBOUND_STRATEGY_SINGLETON_ID: { "route": rewritten } }
            }),
        )?;
        parse_set_updated(
            "x:MtaOutboundStrategy/set",
            OUTBOUND_STRATEGY_SINGLETON_ID,
            &resp,
        )
    }

    // ── S5 outbound submission — LOCAL Stalwart only (loopback JMAP
    //    EmailSubmission, RFC 8621 §7). The audit row in `mail_outbound`
    //    exists BEFORE these are called (pre-mortem #11: no row, no
    //    send); Stalwart's queue owns retries and the smart-host relay
    //    routing (pre-mortem #9 — no daemon-side retry logic, ever).
    //    Message content flows through here — never logged, never in
    //    errors (errors carry method names + server SetErrors only). ──

    /// ✔ LIVE-VERIFIED (module header item 6): submit one composed
    /// outbound message through the LOCAL Stalwart. Standard RFC 8621
    /// shape: `Email/set create` (the full message as a JSON Email
    /// object built by the ops layer — no MIME composing) +
    /// `EmailSubmission/set create` referencing it (`#k2out`), in ONE
    /// request; the SMTP envelope is explicit (`mailFrom` = the
    /// server-stamped From, `rcptTo` = to+cc). Returns Ok when the
    /// server ACCEPTED the message for delivery — never "delivered"
    /// (pre-mortem #9: greylisting/retries are Stalwart's business).
    pub fn submission_send(
        &self,
        account_id: &str,
        from_email: &str,
        rcpt_to: &[String],
        email_create: serde_json::Value,
    ) -> Result<(), String> {
        if rcpt_to.is_empty() {
            return Err("submission: empty rcptTo".to_string());
        }
        let identity_id = self.identity_id_for(account_id, from_email)?;
        // Created emails need a mailbox: the drafts role when the
        // account has one, else the inbox (✔ live-verified: fresh
        // accounts get Inbox/Drafts/Sent/Junk/Trash — the fallback
        // stays as belt+braces).
        let mailbox_id = match self.mailbox_role_id(account_id, "drafts")? {
            Some(id) => id,
            None => self.mailbox_inbox_id(account_id)?,
        };
        let mut email = email_create;
        email["mailboxIds"] = serde_json::json!({ mailbox_id: true });
        let envelope_rcpts: Vec<serde_json::Value> = rcpt_to
            .iter()
            .map(|e| serde_json::json!({ "email": e }))
            .collect();
        let body = serde_json::json!({
            "using": JMAP_SUBMISSION_USING,
            "methodCalls": [
                [
                    "Email/set",
                    { "accountId": account_id, "create": { SUBMIT_EMAIL_TAG: email } },
                    "0"
                ],
                [
                    "EmailSubmission/set",
                    {
                        "accountId": account_id,
                        "create": {
                            SUBMIT_SUB_TAG: {
                                "emailId": format!("#{SUBMIT_EMAIL_TAG}"),
                                "identityId": identity_id,
                                "envelope": {
                                    "mailFrom": { "email": from_email },
                                    "rcptTo": envelope_rcpts,
                                },
                            }
                        }
                    },
                    "1"
                ]
            ],
        });
        let api_url = self.discover_api_url()?;
        let reply = self.post_json_url(&api_url, &body)?;
        parse_submission_created(&reply)
    }

    /// ✔ LIVE-VERIFIED: the sending Identity for `from_email` in
    /// `account_id` — Stalwart pre-creates one per account address;
    /// `Identity/get` matched case-insensitively on the email, with an
    /// `Identity/set create` fallback kept as belt+braces.
    fn identity_id_for(&self, account_id: &str, from_email: &str) -> Result<String, String> {
        let args =
            self.submission_call(account_id, "Identity/get", serde_json::json!({ "ids": null }))?;
        if let Some(id) = parse_identity_for(&args, from_email) {
            return Ok(id);
        }
        let created = self.submission_call(
            account_id,
            "Identity/set",
            serde_json::json!({
                "create": { CREATE_TAG: { "email": from_email } }
            }),
        )?;
        created["created"][CREATE_TAG]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "Identity/set create: no created id in reply".to_string())
    }

    /// One JMAP method call with the SUBMISSION capabilities in `using`
    /// (Identity/EmailSubmission objects, RFC 8621 §6/§7) — otherwise
    /// identical to [`Self::mail_call`] (same delegated `accountId`
    /// scoping).
    fn submission_call(
        &self,
        account_id: &str,
        method: &str,
        mut args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        args["accountId"] = serde_json::Value::String(account_id.to_string());
        let api_url = self.discover_api_url()?;
        let body = serde_json::json!({
            "using": JMAP_SUBMISSION_USING,
            "methodCalls": [[method, args, "0"]],
        });
        let resp = self.post_json_url(&api_url, &body)?;
        parse_method_response(method, &resp)
    }

    /// S4/S5 — a mailbox id by RFC 8621 role (`Ok(None)` = the account
    /// has no mailbox with that role — not an error; callers decide).
    pub fn mailbox_role_id(
        &self,
        account_id: &str,
        role: &str,
    ) -> Result<Option<String>, String> {
        let args = self.mail_call(
            account_id,
            "Mailbox/query",
            serde_json::json!({ "filter": { "role": role } }),
        )?;
        Ok(parse_query_ids(&args).into_iter().next())
    }

    /// S5 — the reply-relevant slice of one message (`k2 mail reply`):
    /// sender, subject, thread id, `Message-ID` + raw `References`
    /// headers (the §8.4 loop caps + In-Reply-To/References stamping),
    /// and the Authentication-Results lines (the DMARC gate). Standard
    /// RFC 8621 header-fetch — same stability class as the S4 reads.
    /// `Ok(None)` = the server answered `notFound` (the route masks it).
    pub fn email_get_reply_context(
        &self,
        account_id: &str,
        email_id: &str,
    ) -> Result<Option<ReplyContext>, String> {
        let args = self.mail_call(
            account_id,
            "Email/get",
            serde_json::json!({
                "ids": [email_id],
                "properties": [
                    "id", "threadId", "from", "subject",
                    MSGID_PROP, REFS_PROP, AUTH_RESULTS_PROP,
                ],
            }),
        )?;
        parse_reply_context(email_id, &args)
    }

    // ── S4 mail reads — STANDARD JMAP mail (RFC 8621: Email/query,
    //    Email/get, Email/set, blob download), stable across Stalwart
    //    versions unlike the registry objects above. Message BODIES
    //    flow through here — they are never logged (pre-mortem #16)
    //    and never appear in errors. ─────────────────────────────────

    /// ✔ LIVE-VERIFIED (was S4 #1): the k2-daemon service account
    /// reads OTHER accounts' mail via standard RFC 8620 delegation —
    /// every method call carries `accountId: <target account>` while
    /// the HTTP authorization stays the service ApiKey. Works because
    /// the service account is an ADMIN-role principal (verified
    /// end-to-end on v0.16.10: Mailbox/query, Email/query|get|set,
    /// Identity/get, EmailSubmission/set).
    fn mail_call(
        &self,
        account_id: &str,
        method: &str,
        mut args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        args["accountId"] = serde_json::Value::String(account_id.to_string());
        let api_url = self.discover_api_url()?;
        let resp = self.post_json_url(&api_url, &mail_envelope(method, args))?;
        parse_method_response(method, &resp)
    }

    /// RFC 9661 Sieve on the mailbox `accountId` (delegated ApiKey).
    /// `using` includes `urn:ietf:params:jmap:sieve` — live `mail_call`
    /// is core+mail only and would `unknownMethod`.
    fn sieve_call(
        &self,
        account_id: &str,
        method: &str,
        mut args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        args["accountId"] = serde_json::Value::String(account_id.to_string());
        let api_url = self.discover_api_url()?;
        let resp = self.post_json_url(&api_url, &sieve_envelope(method, args))?;
        parse_method_response(method, &resp)
    }

    /// S4 — the target account's Inbox mailbox id (`Mailbox/query`
    /// filtered on the RFC 8621 `role: "inbox"`). Every read/wait
    /// query scopes to it.
    pub fn mailbox_inbox_id(&self, account_id: &str) -> Result<String, String> {
        let args = self.mail_call(
            account_id,
            "Mailbox/query",
            serde_json::json!({ "filter": { "role": "inbox" } }),
        )?;
        parse_query_ids(&args).into_iter().next().ok_or_else(|| {
            "Mailbox/query: the account has no inbox mailbox".to_string()
        })
    }

    /// S4 — `Email/query` newest-first (`receivedAt` desc) with a
    /// caller-built RFC 8621 filter (the ops layer owns filter
    /// semantics; this function owns only the wire shape). `offset` is
    /// the RFC 8621 `position` — the pagination cursor an agent walks a
    /// large mailbox by (0 for the first page).
    pub fn email_query_ids(
        &self,
        account_id: &str,
        filter: serde_json::Value,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, String> {
        let mut query = serde_json::json!({
            "filter": filter,
            "sort": [{ "property": "receivedAt", "isAscending": false }],
            "limit": limit,
        });
        if offset > 0 {
            query["position"] = serde_json::json!(offset);
        }
        let args = self.mail_call(account_id, "Email/query", query)?;
        Ok(parse_query_ids(&args))
    }

    /// S4 (triage) — every mailbox of an account as `(id, name, role)`
    /// so the read path can resolve `--folder`/`--junk` to a mailbox id
    /// (and teach with the real names on a miss). Standard RFC 8621
    /// `Mailbox/get` — same stability class as the Email reads.
    pub fn mailbox_list(&self, account_id: &str) -> Result<Vec<MailboxInfo>, String> {
        let args = self.mail_call(
            account_id,
            "Mailbox/get",
            serde_json::json!({ "ids": null, "properties": ["id", "name", "role"] }),
        )?;
        Ok(parse_mailbox_list(&args))
    }

    /// S4 — envelope-level `Email/get` for the summaries list (no
    /// bodies ride this call, by construction — the properties list
    /// has no body/bodyValues entries).
    pub fn email_get_summaries(
        &self,
        account_id: &str,
        ids: &[String],
    ) -> Result<Vec<EmailSummary>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let args = self.mail_call(
            account_id,
            "Email/get",
            serde_json::json!({ "ids": ids, "properties": SUMMARY_PROPERTIES }),
        )?;
        parse_email_summaries(&args)
    }

    /// S4 — one full message: envelope + text AND html bodyValues +
    /// attachment metadata + the raw-message `blobId` + the
    /// `Authentication-Results` headers (SPF/DKIM/DMARC verdicts on
    /// the inbound). `Ok(None)` = the server answered `notFound` for
    /// this id (the route masks it).
    pub fn email_get_full(
        &self,
        account_id: &str,
        email_id: &str,
    ) -> Result<Option<EmailFull>, String> {
        let args = self.mail_call(
            account_id,
            "Email/get",
            serde_json::json!({
                "ids": [email_id],
                "properties": FULL_PROPERTIES,
                "fetchTextBodyValues": true,
                "fetchHTMLBodyValues": true,
                "maxBodyValueBytes": MAX_BODY_VALUE_BYTES,
            }),
        )?;
        parse_email_full(email_id, &args)
    }

    /// S4 — mark one message read (`Email/set` keyword patch on
    /// `$seen`, RFC 8621 §4.1.1).
    pub fn email_mark_seen(&self, account_id: &str, email_id: &str) -> Result<(), String> {
        let args = self.mail_call(
            account_id,
            "Email/set",
            serde_json::json!({ "update": { email_id: { "keywords/$seen": true } } }),
        )?;
        parse_email_set_updated(email_id, &args)
    }

    /// S11 (manage) — MOVE one message to `mailbox_id`: an `Email/set`
    /// update REPLACING `mailboxIds` with the single destination
    /// (`{mailbox_id: true}`). Standard RFC 8621 §4.1.1 — the same
    /// stability class as [`Self::email_mark_seen`]. Archive and delete
    /// (move-to-Trash) both route through here after resolving the
    /// Archive/Trash mailbox by role; there is NO destroy code path.
    pub fn email_move(
        &self,
        account_id: &str,
        email_id: &str,
        mailbox_id: &str,
    ) -> Result<(), String> {
        let args = self.mail_call(
            account_id,
            "Email/set",
            serde_json::json!({
                "update": { email_id: { "mailboxIds": { mailbox_id: true } } }
            }),
        )?;
        parse_email_set_updated(email_id, &args)
    }

    /// S11 (manage) — set/clear ONE keyword on a message (`$seen` for
    /// read/unread, `$flagged` for flag/unflag). `on=true` patches the
    /// keyword to `true`; `on=false` patches it to `null` (RFC 8621
    /// keyword removal). `Email/set` keyword patch, same stability class
    /// as [`Self::email_mark_seen`].
    pub fn email_set_keyword(
        &self,
        account_id: &str,
        email_id: &str,
        keyword: &str,
        on: bool,
    ) -> Result<(), String> {
        let value = if on {
            serde_json::json!(true)
        } else {
            serde_json::Value::Null
        };
        let mut patch = serde_json::Map::new();
        patch.insert(format!("keywords/{keyword}"), value);
        let mut update = serde_json::Map::new();
        update.insert(email_id.to_string(), serde_json::Value::Object(patch));
        let args = self.mail_call(
            account_id,
            "Email/set",
            serde_json::json!({ "update": update }),
        )?;
        parse_email_set_updated(email_id, &args)
    }

    /// S11 (manage) — create a folder (`Mailbox/set create {name}`,
    /// RFC 8621 §2.5). Returns the server-set mailbox id. A top-level
    /// folder (no `parentId`).
    pub fn mailbox_create(&self, account_id: &str, name: &str) -> Result<String, String> {
        let args = self.mail_call(
            account_id,
            "Mailbox/set",
            serde_json::json!({ "create": { CREATE_TAG: { "name": name } } }),
        )?;
        parse_set_created_id("Mailbox/set", &args)
    }

    /// S11 (manage) — rename a folder (`Mailbox/set update {id:{name}}`).
    pub fn mailbox_rename(
        &self,
        account_id: &str,
        mailbox_id: &str,
        new_name: &str,
    ) -> Result<(), String> {
        let args = self.mail_call(
            account_id,
            "Mailbox/set",
            serde_json::json!({ "update": { mailbox_id: { "name": new_name } } }),
        )?;
        parse_set_updated("Mailbox/set", mailbox_id, &args)
    }

    /// S4 — download one blob (attachment bytes, or the raw RFC 822
    /// message via its `blobId`) through the session document's
    /// `downloadUrl` template (RFC 8620 §2 — discovered, never
    /// hardcoded, and REBASED onto our loopback base like `apiUrl`).
    /// RFC 8620 blob upload — POST `uploadUrl`, returns `blobId`.
    pub fn blob_upload(&self, account_id: &str, bytes: &[u8]) -> Result<String, String> {
        let session = self.get_json(SESSION_PATH)?;
        let template = parse_session_upload_url(&self.base_url, &session)?;
        let url = template.replace("{accountId}", &encode_uri_component(account_id));
        let client = reqwest::blocking::Client::builder()
            .timeout(BLOB_TIMEOUT)
            .build()
            .map_err(|e| format!("blob http client: {e}"))?;
        let resp = self
            .apply_auth(client.post(&url))
            .header("Content-Type", "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .map_err(|e| format!("POST blob: {e}"))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            let excerpt: String = text.chars().take(200).collect();
            return Err(format!("POST blob: HTTP {status}: {excerpt}"));
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("POST blob: invalid JSON: {e}"))?;
        v.get("blobId")
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .ok_or_else(|| "POST blob: no blobId in reply".to_string())
    }

    /// RFC 9661 `SieveScript/query` for the K2 forward script name.
    #[cfg(test)]
    pub fn sieve_script_query_id(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<Option<String>, String> {
        let args = self.sieve_call(
            account_id,
            "SieveScript/query",
            serde_json::json!({ "filter": { "name": name } }),
        )?;
        Ok(parse_query_ids(&args).into_iter().next())
    }

    /// Install/replace the `k2-forward` Sieve on this mailbox.
    /// `--keep` → `require ["copy"]; redirect :copy`. Else `redirect`.
    #[cfg(test)]
    pub fn sieve_k2_forward_set(
        &self,
        account_id: &str,
        dest: &str,
        keep: bool,
    ) -> Result<(), String> {
        let script = k2_forward_script(dest, keep);
        let blob_id = self.blob_upload(account_id, script.as_bytes())?;
        let existing = self.sieve_script_query_id(account_id, K2_FORWARD_SCRIPT)?;
        let args = if let Some(id) = existing {
            serde_json::json!({
                "update": { id.clone(): { "blobId": blob_id } },
                "onSuccessActivateScript": id,
            })
        } else {
            serde_json::json!({
                "create": {
                    CREATE_TAG: {
                        "name": K2_FORWARD_SCRIPT,
                        "blobId": blob_id,
                    }
                },
                "onSuccessActivateScript": format!("#{CREATE_TAG}"),
            })
        };
        let resp = self.sieve_call(account_id, "SieveScript/set", args)?;
        expect_set_clean("SieveScript/set", &resp)
    }

    /// Destroy **only** the `k2-forward` script. Idempotent if absent.
    #[cfg(test)]
    pub fn sieve_k2_forward_destroy(&self, account_id: &str) -> Result<(), String> {
        let Some(id) = self.sieve_script_query_id(account_id, K2_FORWARD_SCRIPT)? else {
            return Ok(());
        };
        let resp = self.sieve_call(
            account_id,
            "SieveScript/set",
            serde_json::json!({ "destroy": [id] }),
        )?;
        parse_set_destroyed("SieveScript/set", &id, &resp)
    }

    /// RFC 8621 `Email/import` of a previously uploaded blob into Inbox.
    pub fn email_import(
        &self,
        account_id: &str,
        blob_id: &str,
        mailbox_id: &str,
    ) -> Result<(), String> {
        let args = self.mail_call(
            account_id,
            "Email/import",
            serde_json::json!({
                "emails": {
                    "k2imp": {
                        "blobId": blob_id,
                        "mailboxIds": { mailbox_id: true },
                    }
                }
            }),
        )?;
        if args
            .get("notCreated")
            .and_then(|v| v.as_object())
            .is_some_and(|m| !m.is_empty())
        {
            let detail = serde_json::to_string(&args["notCreated"]).unwrap_or_default();
            let excerpt: String = detail.chars().take(200).collect();
            return Err(format!("Email/import: notCreated: {excerpt}"));
        }
        Ok(())
    }

    pub fn blob_download(
        &self,
        account_id: &str,
        blob_id: &str,
        name: &str,
        mime: &str,
    ) -> Result<Vec<u8>, String> {
        let session = self.get_json(SESSION_PATH)?;
        let template = parse_session_download_url(&self.base_url, &session)?;
        let url = expand_download_url(&template, account_id, blob_id, name, mime);
        self.get_bytes_url(&url)
    }

    /// Authenticated byte-download (its own client: blob transfers get
    /// a longer budget than the 15 s mgmt calls). Errors never carry
    /// the credential or the body.
    fn get_bytes_url(&self, url: &str) -> Result<Vec<u8>, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(BLOB_TIMEOUT)
            .build()
            .map_err(|e| format!("blob http client: {e}"))?;
        let resp = self
            .apply_auth(client.get(url))
            .send()
            .map_err(|e| format!("GET blob: {e}"))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .map_err(|e| format!("GET blob: read body: {e}"))?;
        if !status.is_success() {
            let excerpt: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            return Err(format!("GET blob: HTTP {status}: {excerpt}"));
        }
        Ok(bytes.to_vec())
    }

    // ── RFC 9661 SieveScript (user scripts) + trusted DATA footer ──

    /// List the mailbox's RFC 9661 Sieve scripts (`SieveScript/query` +
    /// `SieveScript/get`). Not `x:SieveScript`. Not VacationResponse.
    pub fn sieve_scripts_list(&self, account_id: &str) -> Result<Vec<SieveScriptInfo>, String> {
        let queried = self.sieve_call(
            account_id,
            "SieveScript/query",
            serde_json::json!({}),
        )?;
        let ids = parse_query_ids(&queried);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let got = self.sieve_call(
            account_id,
            "SieveScript/get",
            serde_json::json!({
                "ids": ids,
                "properties": ["id", "name", "blobId", "isActive"],
            }),
        )?;
        Ok(parse_sieve_script_list(&got))
    }

    pub fn sieve_script_blob(&self, account_id: &str, blob_id: &str) -> Result<Vec<u8>, String> {
        self.blob_download(account_id, blob_id, "k2.sieve", "application/sieve")
    }

    /// Create or update the named `k2` script and activate it. Optionally
    /// destroy leftover `k2-forward` ids in the same set.
    pub fn sieve_script_put_k2(
        &self,
        account_id: &str,
        existing_id: Option<&str>,
        blob_id: &str,
        destroy_ids: &[String],
    ) -> Result<(), String> {
        let mut args = serde_json::Map::new();
        let activate = match existing_id {
            Some(id) => {
                args.insert(
                    "update".to_string(),
                    serde_json::json!({ id: { "blobId": blob_id } }),
                );
                id.to_string()
            }
            None => {
                args.insert(
                    "create".to_string(),
                    serde_json::json!({
                        CREATE_TAG: { "name": "k2", "blobId": blob_id }
                    }),
                );
                format!("#{CREATE_TAG}")
            }
        };
        args.insert(
            "onSuccessActivateScript".to_string(),
            serde_json::Value::String(activate),
        );
        if !destroy_ids.is_empty() {
            args.insert(
                "destroy".to_string(),
                serde_json::Value::Array(
                    destroy_ids
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        let resp = self.sieve_call(
            account_id,
            "SieveScript/set",
            serde_json::Value::Object(args),
        )?;
        expect_set_clean("SieveScript/set", &resp)
    }

    pub fn sieve_scripts_destroy(&self, account_id: &str, ids: &[String]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let resp = self.sieve_call(
            account_id,
            "SieveScript/set",
            serde_json::json!({
                "destroy": ids,
                "onSuccessActivateScript": null,
            }),
        )?;
        expect_set_clean("SieveScript/set", &resp)
    }

    /// Trusted DATA-stage system scripts (`x:SieveSystemScript`).
    pub fn system_scripts_list(&self) -> Result<Vec<SystemScriptInfo>, String> {
        let resp = self.registry_call("x:SieveSystemScript/get", serde_json::json!({}))?;
        Ok(parse_system_script_list(&resp))
    }

    pub fn system_script_put(&self, name: &str, contents: &str) -> Result<(), String> {
        let existing = self.system_scripts_list()?;
        let existing_id = existing.iter().find(|s| s.name == name).map(|s| s.id.clone());
        let args = match existing_id {
            Some(id) => serde_json::json!({
                "update": { id: { "contents": contents, "isActive": true, "name": name } }
            }),
            None => serde_json::json!({
                "create": {
                    CREATE_TAG: {
                        "name": name,
                        "isActive": true,
                        "contents": contents,
                    }
                }
            }),
        };
        let resp = self.registry_call("x:SieveSystemScript/set", args)?;
        expect_set_clean("x:SieveSystemScript/set", &resp)
    }

    pub fn system_script_destroy(&self, name: &str) -> Result<bool, String> {
        let existing = self.system_scripts_list()?;
        let Some(found) = existing.into_iter().find(|s| s.name == name) else {
            return Ok(false);
        };
        let resp = self.registry_call(
            "x:SieveSystemScript/set",
            serde_json::json!({ "destroy": [found.id] }),
        )?;
        parse_set_destroyed("x:SieveSystemScript/set", &found.id, &resp)?;
        Ok(true)
    }

    pub fn mta_stage_data_get_script(&self) -> Result<serde_json::Value, String> {
        let resp = self.registry_call(
            "x:MtaStageData/get",
            serde_json::json!({
                "ids": [MTA_STAGE_DATA_SINGLETON_ID],
                "properties": ["script"],
            }),
        )?;
        parse_mta_stage_script(&resp)
    }

    pub fn mta_stage_data_set_script(&self, script: serde_json::Value) -> Result<(), String> {
        let resp = self.registry_call(
            "x:MtaStageData/set",
            serde_json::json!({
                "update": { MTA_STAGE_DATA_SINGLETON_ID: { "script": script } }
            }),
        )?;
        parse_set_updated("x:MtaStageData/set", MTA_STAGE_DATA_SINGLETON_ID, &resp)
    }
}

/// RFC 9661 SieveScript list entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SieveScriptInfo {
    pub id: String,
    pub name: String,
    pub blob_id: String,
    pub is_active: bool,
}

/// Trusted `x:SieveSystemScript` list entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemScriptInfo {
    pub id: String,
    pub name: String,
    pub contents: String,
    pub is_active: bool,
}

const MTA_STAGE_DATA_SINGLETON_ID: &str = "singleton";

const JMAP_SIEVE_USING: [&str; 2] = [
    "urn:ietf:params:jmap:core",
    "urn:ietf:params:jmap:sieve",
];

fn sieve_envelope(method: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "using": JMAP_SIEVE_USING,
        "methodCalls": [[method, args, "0"]],
    })
}

fn parse_sieve_script_list(args: &serde_json::Value) -> Vec<SieveScriptInfo> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
                    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let blob_id = m
                        .get("blobId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_active = m.get("isActive").and_then(|v| v.as_bool()).unwrap_or(false);
                    Some(SieveScriptInfo {
                        id: id.to_string(),
                        name,
                        blob_id,
                        is_active,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_system_script_list(args: &serde_json::Value) -> Vec<SystemScriptInfo> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
                    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let contents = m
                        .get("contents")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_active = m.get("isActive").and_then(|v| v.as_bool()).unwrap_or(false);
                    Some(SystemScriptInfo {
                        id: id.to_string(),
                        name,
                        contents,
                        is_active,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_mta_stage_script(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(MTA_STAGE_DATA_SINGLETON_ID))
        })
        .ok_or_else(|| "x:MtaStageData/get: singleton not in the reply list".to_string())?;
    Ok(entry
        .get("script")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"else": "false"})))
}

/// DATA-stage `script` expression: run `k2-footer` only when
/// `authenticated_as` is set (outbound submission). Preserve other
/// match rows.
pub fn rewrite_data_script_footer(
    expr: &serde_json::Value,
    footer_name: Option<&str>,
) -> serde_json::Value {
    let else_ = expr
        .get("else")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("false".to_string()));
    let our_if = "!is_empty(authenticated_as)";
    let mut matches: Vec<(usize, serde_json::Value)> = Vec::new();
    if let Some(m) = expr.get("match").and_then(|v| v.as_object()) {
        for (k, v) in m {
            if let Ok(idx) = k.parse::<usize>() {
                matches.push((idx, v.clone()));
            }
        }
    }
    matches.sort_by_key(|(i, _)| *i);
    let mut kept: Vec<serde_json::Value> = matches
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| v.get("if").and_then(|s| s.as_str()) != Some(our_if))
        .collect();
    if let Some(name) = footer_name {
        kept.push(serde_json::json!({ "if": our_if, "then": format!("'{name}'") }));
    }
    if kept.is_empty() {
        return serde_json::json!({ "else": "false" });
    }
    let map: serde_json::Map<String, serde_json::Value> = kept
        .into_iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), v))
        .collect();
    serde_json::json!({ "match": map, "else": else_ })
}

/// True when the DATA script still selects `k2-footer` on authenticated
/// submission (so unset may reset it).
pub fn data_script_points_at_footer(expr: &serde_json::Value, name: &str) -> bool {
    let want = format!("'{name}'");
    if let Some(m) = expr.get("match").and_then(|v| v.as_object()) {
        for v in m.values() {
            if v.get("then").and_then(|s| s.as_str()) == Some(want.as_str())
                || v.get("then").and_then(|s| s.as_str()) == Some(name)
            {
                return true;
            }
        }
    }
    false
}

// ── Session-document parsers ────────────────────────────────────────────

/// Keep only the PATH (+query) of an absolute http(s) URL.
fn url_path_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    Some(rest.find('/').map(|i| &rest[i..]).unwrap_or("/"))
}

/// Pure session-document parser: extract `apiUrl` and REBASE it onto
/// `base_url`. ✔ LIVE-VERIFIED: normal mode serves an ABSOLUTE url on
/// the mail hostname (`https://mail.<domain>/jmap/`) — unreachable /
/// wrong scheme for the loopback mgmt dial, so only its path is kept.
pub fn parse_session_api_url(
    base_url: &str,
    session: &serde_json::Value,
) -> Result<String, String> {
    let api_url = session
        .get("apiUrl")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "JMAP session document has no usable 'apiUrl' — is this really a Stalwart \
             /jmap/session endpoint?"
                .to_string()
        })?;
    let path = if let Some(p) = url_path_of(api_url) {
        p
    } else if api_url.starts_with('/') {
        api_url
    } else {
        return Err(format!(
            "JMAP session 'apiUrl' is neither absolute nor root-relative: '{api_url}'"
        ));
    };
    Ok(format!("{}{}", base_url.trim_end_matches('/'), path))
}

/// Pure session-document parser: the authenticated principal's account
/// id — `primaryAccounts["urn:stalwart:jmap"]` when present (normal
/// admin), else the single `accounts` key (bootstrap recovery admin's
/// session has no stalwart primaryAccount entry in bootstrap mode).
pub fn parse_session_account_id(session: &serde_json::Value) -> Result<String, String> {
    if let Some(id) = session["primaryAccounts"][STALWART_CAPABILITY].as_str() {
        return Ok(id.to_string());
    }
    session
        .get("accounts")
        .and_then(|v| v.as_object())
        .and_then(|m| m.keys().next())
        .map(String::from)
        .ok_or_else(|| {
            "JMAP session document has no accounts — cannot address registry calls".to_string()
        })
}

/// Pure session-document parser for the RFC 8620 `downloadUrl`
/// template — discovered, never hardcoded, REBASED onto `base_url`
/// (same rule as `apiUrl`).
pub fn parse_session_download_url(
    base_url: &str,
    session: &serde_json::Value,
) -> Result<String, String> {
    let url = session
        .get("downloadUrl")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "JMAP session document has no usable 'downloadUrl'".to_string()
        })?;
    let path = if let Some(p) = url_path_of(url) {
        p
    } else if url.starts_with('/') {
        url
    } else {
        return Err(format!(
            "JMAP session 'downloadUrl' is neither absolute nor root-relative: '{url}'"
        ));
    };
    Ok(format!("{}{}", base_url.trim_end_matches('/'), path))
}

/// Same rebase rule as [`parse_session_download_url`] for `uploadUrl`.
pub fn parse_session_upload_url(
    base_url: &str,
    session: &serde_json::Value,
) -> Result<String, String> {
    let url = session
        .get("uploadUrl")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "JMAP session document has no usable 'uploadUrl'".to_string())?;
    let path = if let Some(p) = url_path_of(url) {
        p
    } else if url.starts_with('/') {
        url
    } else {
        return Err(format!(
            "JMAP session 'uploadUrl' is neither absolute nor root-relative: '{url}'"
        ));
    };
    Ok(format!("{}{}", base_url.trim_end_matches('/'), path))
}

// ── Registry wire layer (envelope + pure parsers) ───────────────────────

/// ✔ LIVE-VERIFIED: the Bootstrap object is a singleton whose JMAP id
/// is literally the string `"singleton"`.
const BOOTSTRAP_SINGLETON_ID: &str = "singleton";

/// `x:SystemSettings` singleton — live `defaultHostname` (SMTP banner)
/// after bootstrap. Same literal id as Bootstrap / MtaOutboundStrategy.
const SYSTEM_SETTINGS_SINGLETON_ID: &str = "singleton";

/// Stalwart 0.16 `x:Certificate/set` create: PEM as Text value +
/// SecretText secret (not File paths — ProtectHome cannot read
/// `~/.k2/certs`).
fn certificate_create_args(chain_pem: &str, key_pem: &str) -> serde_json::Value {
    serde_json::json!({
        "create": {
            CREATE_TAG: {
                "certificate": { "@type": "Text", "value": chain_pem },
                "privateKey": { "@type": "Text", "secret": key_pem },
            }
        }
    })
}

/// Point the SystemSettings singleton at a planted Certificate id.
fn default_certificate_id_args(cert_id: &str) -> serde_json::Value {
    serde_json::json!({
        "update": {
            SYSTEM_SETTINGS_SINGLETON_ID: { "defaultCertificateId": cert_id }
        }
    })
}

/// ✔ LIVE-VERIFIED: the MtaOutboundStrategy singleton uses the same
/// literal id.
const OUTBOUND_STRATEGY_SINGLETON_ID: &str = "singleton";

/// The loopback bind for the permanent plain-HTTP mgmt listener (the
/// supervisor's `STALWART_MGMT_URL` counterpart).
const STALWART_MGMT_BIND: &str = "127.0.0.1:8180";

/// Bootstrap/recovery HTTP bind while we still need :8080.
/// Never `*:8080` / `[::]:8080`.
const STALWART_SETUP_BIND: &str = super::supervisor::STALWART_SETUP_BIND;

/// The JMAP `using` capabilities for registry (`x:*`) calls.
/// ✔ LIVE-VERIFIED v0.16.10: `urn:stalwart:jmap`.
const JMAP_REGISTRY_USING: [&str; 2] = [JMAP_CORE_CAPABILITY, STALWART_CAPABILITY];

/// Pure envelope builder for a single registry method call.
fn registry_envelope(method: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "using": JMAP_REGISTRY_USING,
        "methodCalls": [[method, args, "0"]],
    })
}

/// Pure response unwrapper: extract the first `methodResponses` entry,
/// require it to answer `method` (a JMAP-level failure answers
/// `error`), and return its arguments. Errors are one-line and name
/// the method + the server's error type/description.
fn parse_method_response(
    method: &str,
    resp: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let first = resp
        .get("methodResponses")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("{method}: reply has no methodResponses — not a JMAP response?"))?;
    let name = first.first().and_then(|v| v.as_str()).unwrap_or("");
    let args = first.get(1).cloned().unwrap_or(serde_json::Value::Null);
    if name == method {
        return Ok(args);
    }
    if name == "error" {
        let etype = args.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
        let desc = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("no description");
        return Err(format!("{method}: JMAP error '{etype}': {desc}"));
    }
    Err(format!("{method}: unexpected method response '{name}'"))
}

/// Format a JMAP SetError (`{type, description?}`) into one line.
fn set_error_line(err: &serde_json::Value) -> String {
    let etype = err.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
    match err.get("description").and_then(|v| v.as_str()) {
        Some(d) => format!("{etype}: {d}"),
        None => etype.to_string(),
    }
}

/// Pure `/set` reply guard: any non-empty `notCreated`/`notUpdated`/
/// `notDestroyed` map is a loud Err carrying the server's SetError
/// excerpt (a partially-applied listener plan must never read as ok).
fn expect_set_clean(method: &str, args: &serde_json::Value) -> Result<(), String> {
    for reject in ["notCreated", "notUpdated", "notDestroyed"] {
        if let Some(map) = args.get(reject).and_then(|v| v.as_object()) {
            if !map.is_empty() {
                let detail = serde_json::to_string(map).unwrap_or_default();
                let excerpt: String = detail.chars().take(200).collect();
                return Err(format!("{method}: {reject}: {excerpt}"));
            }
        }
    }
    Ok(())
}

/// Pure `x:Bootstrap/set` reply parser: `updated.singleton` carries
/// the provisioned admin `{username, secret}` (✔ live-verified);
/// `notUpdated` surfaces the server's SetError verbatim.
fn parse_bootstrap_updated(args: &serde_json::Value) -> Result<AdminCredentials, String> {
    if let Some(updated) = args
        .get("updated")
        .and_then(|v| v.get(BOOTSTRAP_SINGLETON_ID))
    {
        let username = updated
            .get("username")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let secret = updated
            .get("secret")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        return match (username, secret) {
            (Some(u), Some(s)) => Ok(AdminCredentials {
                username: u.to_string(),
                secret: s.to_string(),
            }),
            _ => Err(
                "x:Bootstrap/set: updated but no admin credentials in the reply — \
                 was an external directory configured?"
                    .to_string(),
            ),
        };
    }
    if let Some(err) = args
        .get("notUpdated")
        .and_then(|v| v.get(BOOTSTRAP_SINGLETON_ID))
    {
        return Err(format!("x:Bootstrap/set rejected — {}", set_error_line(err)));
    }
    Err("x:Bootstrap/set: reply has neither updated nor notUpdated for the singleton".to_string())
}

/// One registry network listener (id + name is all the port plan
/// needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerInfo {
    pub id: String,
    pub name: String,
}

/// Pure `x:NetworkListener/get` reply parser.
fn parse_listeners(args: &serde_json::Value) -> Result<Vec<ListenerInfo>, String> {
    let list = args
        .get("list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "x:NetworkListener/get: reply has no list".to_string())?;
    list.iter()
        .map(|e| {
            let id = e
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "x:NetworkListener/get: entry without an id".to_string())?;
            let name = e
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            Ok(ListenerInfo { id: id.to_string(), name: name.to_string() })
        })
        .collect()
}

/// One mailbox (folder) of an account — the read path's `--folder`/
/// `--junk` resolution surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxInfo {
    pub id: String,
    pub name: String,
    /// The RFC 8621 role (`inbox`/`junk`/`sent`/…) when the mailbox has
    /// one; `None` for user-created folders.
    pub role: Option<String>,
}

/// Pure `Mailbox/get` reply parser: `list` of `{id, name, role}`
/// (roleless folders keep `None`). Entries without an id are skipped
/// (never a hard error — a partial folder list still lets `--folder`
/// resolve).
fn parse_mailbox_list(args: &serde_json::Value) -> Vec<MailboxInfo> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
                    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                    let role = m
                        .get("role")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    Some(MailboxInfo { id: id.to_string(), name: name.to_string(), role })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pure `*/set create` reply parser: our creation tag must appear
/// under `created` with a server-set id — `notCreated` surfaces the
/// server's SetError verbatim.
fn parse_set_created_id(method: &str, args: &serde_json::Value) -> Result<String, String> {
    if let Some(created) = args.get("created").and_then(|v| v.get(CREATE_TAG)) {
        return created
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .ok_or_else(|| format!("{method} create: reply has no server-set 'id'"));
    }
    if let Some(err) = args.get("notCreated").and_then(|v| v.get(CREATE_TAG)) {
        return Err(format!("{method} create rejected — {}", set_error_line(err)));
    }
    Err(format!(
        "{method} create: reply has neither created nor notCreated for our tag"
    ))
}

/// Pure `*/set update` reply parser: the id must appear as a key of
/// the `updated` map (RFC 8620: id → server-set props or null);
/// `notUpdated` surfaces the server's SetError.
fn parse_set_updated(method: &str, id: &str, args: &serde_json::Value) -> Result<(), String> {
    let updated = args
        .get("updated")
        .and_then(|v| v.as_object())
        .map(|m| m.contains_key(id))
        .unwrap_or(false);
    if updated {
        return Ok(());
    }
    if let Some(err) = args.get("notUpdated").and_then(|v| v.get(id)) {
        return Err(format!("{method} update rejected — {}", set_error_line(err)));
    }
    Err(format!("{method} update: '{id}' not in the updated map"))
}

/// Pure `*/set destroy` reply parser: the id must appear under
/// `destroyed`; `notDestroyed` surfaces the server's SetError.
fn parse_set_destroyed(method: &str, id: &str, args: &serde_json::Value) -> Result<(), String> {
    let destroyed = args
        .get("destroyed")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|v| v.as_str() == Some(id)))
        .unwrap_or(false);
    if destroyed {
        return Ok(());
    }
    if let Some(err) = args.get("notDestroyed").and_then(|v| v.get(id)) {
        return Err(format!("{method} destroy rejected — {}", set_error_line(err)));
    }
    Err(format!("{method} destroy: '{id}' not in the destroyed list"))
}

/// Batch destroy: every requested id must land in `destroyed`.
fn parse_set_destroyed_many(
    method: &str,
    ids: &[String],
    args: &serde_json::Value,
) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        parse_set_destroyed(method, id, args)?;
        out.push(id.clone());
    }
    Ok(out)
}

/// Pure `x:BlockedIp/get` / `x:AllowedIp/get` list parser.
fn parse_ip_list_get(args: &serde_json::Value) -> Vec<IpListEntry> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    let id = e
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())?;
                    let address = e
                        .get("address")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())?;
                    Some(IpListEntry {
                        id: id.to_string(),
                        address: address.to_string(),
                        reason: e
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                        created_at: e
                            .get("createdAt")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        expires_at: e
                            .get("expiresAt")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pure `x:Security/get` singleton parser for `authBanRate`.
fn parse_security_get(args: &serde_json::Value) -> Result<SecuritySettings, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| {
            a.iter().find(|e| {
                e.get("id").and_then(|v| v.as_str()) == Some(SYSTEM_SETTINGS_SINGLETON_ID)
            })
        });
    let Some(entry) = entry else {
        return Err("x:Security/get: singleton not in the reply list".to_string());
    };
    let auth_ban_rate = match entry.get("authBanRate") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => {
            let count = v
                .get("count")
                .and_then(|c| c.as_u64())
                .ok_or_else(|| "x:Security/get: authBanRate missing count".to_string())?;
            let period = v
                .get("period")
                .and_then(|p| p.as_u64())
                .ok_or_else(|| "x:Security/get: authBanRate missing period".to_string())?;
            Some(AuthBanRate { count, period })
        }
    };
    Ok(SecuritySettings { auth_ban_rate })
}

fn parse_domain_get_report_address(
    id: &str,
    args: &serde_json::Value,
) -> Result<Option<String>, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)));
    let Some(entry) = entry else {
        return Err(format!("x:Domain/get: domain '{id}' not in the reply list"));
    };
    match entry.get("reportAddressUri") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => Ok(v
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)),
    }
}

fn parse_dkim_get_list(args: &serde_json::Value) -> Result<Vec<DkimSignature>, String> {
    let list = args
        .get("list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "x:DkimSignature/get: reply has no list".to_string())?;
    let mut out = Vec::with_capacity(list.len());
    for e in list {
        let id = e
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "x:DkimSignature/get: entry has no id".to_string())?;
        let selector = e
            .get("selector")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        let type_name = e
            .get("@type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let stage = e
            .get("stage")
            .and_then(|v| v.as_str())
            .unwrap_or("active")
            .to_string();
        let public_key = e
            .get("publicKey")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let next_transition_at = e
            .get("nextTransitionAt")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        out.push(DkimSignature {
            id: id.to_string(),
            selector,
            type_name,
            stage,
            public_key,
            next_transition_at,
        });
    }
    Ok(out)
}



/// Pure `x:AppPassword/set` create parser: `created.k2` must carry
/// server-set `id` + `secret`. `notCreated` surfaces the SetError.
fn parse_app_password_created(args: &serde_json::Value) -> Result<CreatedAppPassword, String> {
    if let Some(created) = args.get("created").and_then(|v| v.get(CREATE_TAG)) {
        let id = created
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "x:AppPassword/set create: no server-set 'id'".to_string())?;
        let secret = created
            .get("secret")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "x:AppPassword/set create: no secret in reply".to_string())?;
        return Ok(CreatedAppPassword {
            id: id.to_string(),
            secret: secret.to_string(),
        });
    }
    if let Some(err) = args.get("notCreated").and_then(|v| v.get(CREATE_TAG)) {
        return Err(format!(
            "x:AppPassword/set create rejected — {}",
            set_error_line(err)
        ));
    }
    Err("x:AppPassword/set create: reply has neither created nor notCreated for our tag".to_string())
}

/// Pure `x:AppPassword/get` list parser. Drops `secret` even if the
/// engine sent it.
fn parse_app_password_list(args: &serde_json::Value) -> Vec<AppPasswordInfo> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    let id = e
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())?;
                    Some(AppPasswordInfo {
                        id: id.to_string(),
                        description: e
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        created_at: e
                            .get("createdAt")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}



/// Pure `x:Domain/get` reply parser: find our id in `list` and return
/// its non-empty `dnsZoneFile`.
fn parse_domain_get_zonefile(
    id: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)));
    let Some(entry) = entry else {
        return Err(format!("x:Domain/get: domain '{id}' not in the reply list"));
    };
    entry
        .get("dnsZoneFile")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| format!("x:Domain/get: domain '{id}' has no dnsZoneFile"))
}

/// Pure `x:Account/get` reply parser: our id's `quotas` object.
fn parse_account_get_quotas(id: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)));
    let Some(entry) = entry else {
        return Err(format!("x:Account/get: account '{id}' not in the reply list"));
    };
    match entry.get("quotas") {
        Some(q) if q.is_object() => Ok(q.clone()),
        Some(q) if q.is_null() => Ok(serde_json::json!({})),
        _ => Err(format!("x:Account/get: account '{id}' has no quotas object")),
    }
}

/// Stalwart 0.16 `EmailAlias` (`name` local-part + `domainId`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAlias {
    pub name: String,
    pub domain_id: String,
    pub enabled: bool,
}

/// Alias of [`EmailAlias`] — next-cli lists used this name.
pub type AccountAlias = EmailAlias;

/// `x:Account` `@type: User` slice used by alias add/list/remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUser {
    pub id: String,
    pub name: String,
    pub domain_id: String,
    pub aliases: Vec<EmailAlias>,
}

/// Index-keyed object (`"0"|"1"|…`) — a JSON array is `invalidPatch`.
pub fn aliases_to_index_map(aliases: &[EmailAlias]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (i, a) in aliases.iter().enumerate() {
        map.insert(
            i.to_string(),
            serde_json::json!({
                "name": a.name,
                "domainId": a.domain_id,
                "enabled": a.enabled,
            }),
        );
    }
    serde_json::Value::Object(map)
}

pub fn parse_email_aliases(v: Option<&serde_json::Value>) -> Vec<EmailAlias> {
    let Some(v) = v else {
        return Vec::new();
    };
    if v.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(arr) = v.as_array() {
        for e in arr {
            if let Some(a) = parse_one_email_alias(e) {
                out.push(a);
            }
        }
        return out;
    }
    if let Some(obj) = v.as_object() {
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort_by_key(|k| k.parse::<usize>().unwrap_or(usize::MAX));
        for k in keys {
            if let Some(a) = parse_one_email_alias(&obj[k]) {
                out.push(a);
            }
        }
    }
    out
}

fn parse_one_email_alias(v: &serde_json::Value) -> Option<EmailAlias> {
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_ascii_lowercase();
    let domain_id = v
        .get("domainId")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
    Some(EmailAlias {
        name,
        domain_id,
        enabled,
    })
}

/// Collision vs another User's primary `name@domainId` or EmailAlias.
/// Own account id is excluded (idempotent add on self).
pub fn alias_collides(
    mailbox_account_id: &str,
    alias_local: &str,
    domain_id: &str,
    users: &[AccountUser],
) -> bool {
    let local = alias_local.trim().to_ascii_lowercase();
    for u in users {
        if u.id == mailbox_account_id {
            continue;
        }
        if u.name.eq_ignore_ascii_case(&local) && u.domain_id == domain_id {
            return true;
        }
        if u
            .aliases
            .iter()
            .any(|a| a.name == local && a.domain_id == domain_id)
        {
            return true;
        }
    }
    false
}

fn parse_account_user_entry(e: &serde_json::Value) -> Option<AccountUser> {
    let id = e.get("id").and_then(|v| v.as_str())?.to_string();
    let name = e
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let domain_id = e
        .get("domainId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(AccountUser {
        id,
        name,
        domain_id,
        aliases: parse_email_aliases(e.get("aliases")),
    })
}

fn parse_account_get_user(id: &str, args: &serde_json::Value) -> Result<AccountUser, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)));
    let Some(entry) = entry else {
        return Err(format!("x:Account/get: account '{id}' not in the reply list"));
    };
    parse_account_user_entry(entry)
        .ok_or_else(|| format!("x:Account/get: account '{id}' has no id"))
}

fn parse_account_get_users(args: &serde_json::Value) -> Result<Vec<AccountUser>, String> {
    let list = args
        .get("list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "x:Account/get: reply has no list".to_string())?;
    Ok(list.iter().filter_map(parse_account_user_entry).collect())
}

/// `x:MailingList/get` row. Recipients is a set object on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct MailingListInfo {
    pub id: String,
    pub name: String,
    pub domain_id: String,
    pub email_address: Option<String>,
    pub recipients: serde_json::Value,
    pub description: Option<String>,
}

/// `x:QueuedMessage/get` row.
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedMessageInfo {
    pub id: String,
    pub next_retry: Option<String>,
    pub return_path: Option<String>,
    pub recipients: serde_json::Value,
}

fn parse_domain_get_catchall(
    id: &str,
    args: &serde_json::Value,
) -> Result<Option<String>, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)));
    let Some(entry) = entry else {
        return Err(format!("x:Domain/get: domain '{id}' not in the reply list"));
    };
    match entry.get("catchAllAddress") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                Ok(Some(t.to_string()))
            }
        }
        Some(_) => Err(format!(
            "x:Domain/get: domain '{id}' catchAllAddress is not a string or null"
        )),
    }
}

/// Sieve body for `k2-forward`. Dest is quoted; keep uses `:copy`.
#[cfg(test)]
pub fn k2_forward_script(dest: &str, keep: bool) -> String {
    let quoted = sieve_quote(dest);
    if keep {
        format!("require [\"copy\"];\nredirect :copy {quoted};\n")
    } else {
        format!("redirect {quoted};\n")
    }
}

#[cfg(test)]
fn sieve_quote(addr: &str) -> String {
    let escaped = addr.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
fn sieve_unquote(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        return Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
    }
    if !s.is_empty() {
        return Some(s.to_string());
    }
    None
}

#[cfg(test)]
pub fn parse_k2_forward_script(src: &str) -> Option<(String, bool)> {
    let mut dest = None;
    let mut keep = false;
    for line in src.lines() {
        let t = line.trim().trim_end_matches(';').trim();
        if let Some(rest) = t.strip_prefix("redirect") {
            let rest = rest.trim();
            if let Some(addr) = rest.strip_prefix(":copy") {
                keep = true;
                dest = sieve_unquote(addr.trim());
            } else {
                dest = sieve_unquote(rest);
            }
        }
    }
    dest.map(|d| (d, keep))
}

fn parse_account_get_aliases(
    id: &str,
    args: &serde_json::Value,
) -> Result<Vec<AccountAlias>, String> {
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)));
    let Some(entry) = entry else {
        return Err(format!("x:Account/get: account '{id}' not in the reply list"));
    };
    Ok(parse_alias_list(entry.get("aliases")))
}

fn parse_alias_list(v: Option<&serde_json::Value>) -> Vec<AccountAlias> {
    let Some(v) = v else {
        return Vec::new();
    };
    if let Some(arr) = v.as_array() {
        return arr.iter().filter_map(parse_one_alias).collect();
    }
    if let Some(obj) = v.as_object() {
        return obj.values().filter_map(parse_one_alias).collect();
    }
    Vec::new()
}

fn parse_one_alias(v: &serde_json::Value) -> Option<AccountAlias> {
    let name = v.get("name").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())?;
    let domain_id = v
        .get("domainId")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
    Some(AccountAlias {
        name: name.to_string(),
        domain_id,
        enabled,
    })
}

fn parse_mailing_list_get(args: &serde_json::Value) -> Vec<MailingListInfo> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    let id = e.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
                    Some(MailingListInfo {
                        id: id.to_string(),
                        name: e
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        domain_id: e
                            .get("domainId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        email_address: e
                            .get("emailAddress")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        recipients: e
                            .get("recipients")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                        description: e
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_queued_message_get(args: &serde_json::Value) -> Vec<QueuedMessageInfo> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    let id = e.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
                    Some(QueuedMessageInfo {
                        id: id.to_string(),
                        next_retry: e
                            .get("nextRetry")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        return_path: e
                            .get("returnPath")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        recipients: e
                            .get("recipients")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_mailbox_share_with(id: &str, args: &serde_json::Value) -> Option<serde_json::Value> {
    let entry = args.get("list").and_then(|v| v.as_array()).and_then(|a| {
        a.iter()
            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id))
    })?;
    entry.get("shareWith").cloned()
}

/// Recipients set-object `{addr: true, …}` — never a JSON array.
pub fn recipients_set_object(addrs: &[String]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for a in addrs {
        map.insert(a.clone(), serde_json::json!(true));
    }
    serde_json::Value::Object(map)
}

/// Parse recipients whether the engine served a set object or (wrongly)
/// an array. Always return unique lowercase addresses.
pub fn recipients_addrs(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if val.as_bool() == Some(true) || val.is_object() {
                let t = k.trim().to_ascii_lowercase();
                if !t.is_empty() && !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    } else if let Some(arr) = v.as_array() {
        for e in arr {
            if let Some(s) = e.as_str() {
                let t = s.trim().to_ascii_lowercase();
                if !t.is_empty() && !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Pure `x:Account/get` reply parser: our id's `name`.
fn parse_account_get_name(id: &str, args: &serde_json::Value) -> Result<String, String> {
    args.get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)))
        .and_then(|e| e.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| format!("x:Account/get: account '{id}' not found or has no name"))
}

/// The marker the retire rename embeds (idempotence + recognizability
/// in Stalwart's own admin views).
const RETIRED_MARKER: &str = "-k2r-";

/// Pure retire-rename builder: `<local>-k2r-<unix>` truncated so the
/// local part stays within the RFC's 64-char budget (⚠ module-header
/// item a: the server-side limit itself was not probed).
pub fn retired_local_part(local: &str, unix: i64) -> String {
    let suffix = format!("{RETIRED_MARKER}{unix}");
    let keep = 64usize.saturating_sub(suffix.len());
    let base: String = local.chars().take(keep).collect();
    format!("{base}{suffix}")
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A Stalwart domain the S2 create call just made: the server-set id;
/// `dns_zone_file` is filled by the engine impl's post-create poll
/// (the create reply itself carries only the id — ✔ live-verified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedDomain {
    pub id: String,
    pub dns_zone_file: Option<String>,
}

/// One smart-host outbound route (PRD §8.3): what Stalwart needs to
/// relay a domain's outbound mail through the owner's SMTP provider.
/// The password is the RESOLVED secret (the caller resolves the
/// `mail_relay_configs.secret_ref` through the secret store) — it
/// lives only for the duration of the apply call and is never logged.
pub struct RelayRoute {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// true = implicit TLS (:465-style); false = STARTTLS.
    pub implicit_tls: bool,
}

/// The MtaRoute object name for one domain's relay (dots → dashes so
/// the name reads cleanly in Stalwart's own views; also the expression
/// literal).
pub fn relay_route_name(domain: &str) -> String {
    format!("k2-relay-{}", domain.replace('.', "-"))
}

/// Pure parser: find a named object's id in a `*/get` reply list.
fn parse_named_object_id(args: &serde_json::Value, name: &str) -> Option<String> {
    args.get("list")?
        .as_array()?
        .iter()
        .find(|e| e.get("name").and_then(|v| v.as_str()) == Some(name))
        .and_then(|e| e.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Pure rewrite of the outbound strategy `route` expression
/// (`{match: {"0": {if, then}, …}, else}` — ✔ live-verified shape):
/// drop any existing match for `domain`, then (when `route_name` is
/// `Some`) append `sender_domain == '<domain>'` → `'<route_name>'`.
/// Existing rules (the `is_local_domain(rcpt_domain)` local rule
/// first) keep their relative order.
pub fn rewrite_route_expression(
    expr: &serde_json::Value,
    domain: &str,
    route_name: Option<&str>,
) -> Result<serde_json::Value, String> {
    let else_ = expr
        .get("else")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "route expression has no 'else'".to_string())?;
    let our_if = format!("sender_domain == '{domain}'");
    let mut matches: Vec<(usize, serde_json::Value)> = Vec::new();
    if let Some(m) = expr.get("match").and_then(|v| v.as_object()) {
        for (k, v) in m {
            let idx: usize = k.parse().map_err(|_| {
                format!("route expression match has a non-numeric key '{k}'")
            })?;
            matches.push((idx, v.clone()));
        }
    }
    matches.sort_by_key(|(i, _)| *i);
    let mut kept: Vec<serde_json::Value> = matches
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| v.get("if").and_then(|s| s.as_str()) != Some(our_if.as_str()))
        .collect();
    if let Some(name) = route_name {
        kept.push(serde_json::json!({ "if": our_if, "then": format!("'{name}'") }));
    }
    let map: serde_json::Map<String, serde_json::Value> = kept
        .into_iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), v))
        .collect();
    Ok(serde_json::json!({ "match": map, "else": else_ }))
}

// ── S4 mail-read wire layer (RFC 8621 constants, structs, parsers) ─────

/// JMAP `using` for MAIL data calls (RFC 8621) — distinct from the
/// registry envelope's Stalwart capability.
const JMAP_MAIL_USING: [&str; 2] = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];

/// Hostmail forward script name. Unset destroys this name only.
#[cfg(test)]
pub const K2_FORWARD_SCRIPT: &str = "k2-forward";

/// Per-part body cap on `Email/get` (`maxBodyValueBytes`): 256 KiB of
/// text per part is far beyond any verification mail and bounds the
/// daemon's per-message memory. `isTruncated` rides the bodyValue when
/// the server clipped.
const MAX_BODY_VALUE_BYTES: u64 = 262_144;

/// Blob transfers (attachments / raw messages) get a longer budget
/// than the 15 s mgmt calls — still loopback.
const BLOB_TIMEOUT: Duration = Duration::from_secs(60);

/// RFC 8621 §4.1.3 header-fetch property: every Authentication-Results
/// header as raw text (SPF/DKIM/DMARC verdicts parsed at the ops
/// layer, not here).
const AUTH_RESULTS_PROP: &str = "header:Authentication-Results:asText:all";

// ── S5 submission wire layer (RFC 8621 §6/§7) ───────────────────────────

/// JMAP `using` for SUBMISSION calls (Identity + EmailSubmission live
/// under the submission capability; mail rides along for the combined
/// Email/set + EmailSubmission/set request).
const JMAP_SUBMISSION_USING: [&str; 3] = [
    "urn:ietf:params:jmap:core",
    "urn:ietf:params:jmap:mail",
    "urn:ietf:params:jmap:submission",
];

/// Creation tags inside the combined submit request (caller-chosen,
/// stable so fixtures and parsers agree — the CREATE_TAG idiom).
const SUBMIT_EMAIL_TAG: &str = "k2out";
const SUBMIT_SUB_TAG: &str = "k2sub";

/// Reply-context header-fetch properties (S5 §8.4 guardrails).
const MSGID_PROP: &str = "header:Message-ID:asText";
const REFS_PROP: &str = "header:References:asText";

/// Envelope-only properties for the summaries list — deliberately no
/// body/bodyValues entries (summaries never carry untrusted body
/// content, and never pay body transfer costs).
const SUMMARY_PROPERTIES: [&str; 8] = [
    "id",
    "threadId",
    "from",
    "to",
    "subject",
    "receivedAt",
    "keywords",
    "hasAttachment",
];

/// Full-read properties (§8.1): envelope + both body part lists +
/// their values + attachment metadata + the raw-message blob id + the
/// auth-results headers.
const FULL_PROPERTIES: [&str; 15] = [
    "id",
    "blobId",
    "threadId",
    "from",
    "to",
    "cc",
    "subject",
    "receivedAt",
    "keywords",
    "hasAttachment",
    "textBody",
    "htmlBody",
    "bodyValues",
    "attachments",
    AUTH_RESULTS_PROP,
];

/// Pure envelope builder for a single JMAP MAIL method call.
fn mail_envelope(method: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "using": JMAP_MAIL_USING,
        "methodCalls": [[method, args, "0"]],
    })
}

/// One RFC 8621 EmailAddress (`{name?, email}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAddr {
    pub name: Option<String>,
    pub email: String,
}

/// Envelope-level view of one message (the summaries list + the wait
/// loop's match candidates).
#[derive(Debug, Clone)]
pub struct EmailSummary {
    pub id: String,
    pub thread_id: Option<String>,
    pub from: Vec<MailAddr>,
    pub to: Vec<MailAddr>,
    pub subject: String,
    /// RFC 8621 `receivedAt` (UTCDate, always `Z`).
    pub received_at: String,
    pub unread: bool,
    pub has_attachment: bool,
}

/// Attachment metadata (§8.1 — bytes only move on explicit
/// `attachments --get`, via [`StalwartClient::blob_download`]).
#[derive(Debug, Clone)]
pub struct AttachmentMeta {
    pub blob_id: String,
    pub filename: Option<String>,
    pub mime: String,
    pub size: u64,
}

/// One full message as fetched — RAW body text; the §8.1 shaping
/// (untrusted-content markers, HTML-strip fallback, auth-verdict
/// parsing) is deliberately NOT here (§17.5: it lives at the route/ops
/// layer so any future backend inherits it).
#[derive(Debug, Clone)]
pub struct EmailFull {
    pub summary: EmailSummary,
    pub cc: Vec<MailAddr>,
    /// Blob id of the raw RFC 822 message (`read --raw`).
    pub blob_id: Option<String>,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<AttachmentMeta>,
    /// Raw Authentication-Results header lines, newest first as served.
    pub auth_results: Vec<String>,
}

/// Pure `*/query` reply parser: the `ids` array (empty when absent —
/// an empty result is not an error).
fn parse_query_ids(args: &serde_json::Value) -> Vec<String> {
    args.get("ids")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Pure EmailAddress-list parser (`from`/`to`/`cc`).
fn parse_addr_list(v: Option<&serde_json::Value>) -> Vec<MailAddr> {
    v.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let email = e.get("email").and_then(|v| v.as_str())?;
                    Some(MailAddr {
                        name: e
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                        email: email.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pure per-entry summary parser. `id` is required (fail loud);
/// everything else degrades to empty/false.
fn parse_email_summary_entry(entry: &serde_json::Value) -> Result<EmailSummary, String> {
    let id = entry
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Email/get: entry without an id".to_string())?;
    let unread = !entry
        .get("keywords")
        .and_then(|k| k.get("$seen"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(EmailSummary {
        id: id.to_string(),
        thread_id: entry
            .get("threadId")
            .and_then(|v| v.as_str())
            .map(String::from),
        from: parse_addr_list(entry.get("from")),
        to: parse_addr_list(entry.get("to")),
        subject: entry
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        received_at: entry
            .get("receivedAt")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        unread,
        has_attachment: entry
            .get("hasAttachment")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Pure `Email/get` (summaries) reply parser.
fn parse_email_summaries(args: &serde_json::Value) -> Result<Vec<EmailSummary>, String> {
    args.get("list")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(parse_email_summary_entry).collect())
        .unwrap_or_else(|| Err("Email/get: reply has no list".to_string()))
}

/// Assemble one body kind from RFC 8621 `textBody`/`htmlBody` part
/// lists + `bodyValues` (multiple parts of the kind concatenate in
/// order, per spec display semantics).
fn body_text_from(entry: &serde_json::Value, key: &str, want_type: &str) -> Option<String> {
    let values = entry.get("bodyValues")?.as_object()?;
    let parts = entry.get(key)?.as_array()?;
    let mut out = String::new();
    for p in parts {
        if p.get("type").and_then(|v| v.as_str()) != Some(want_type) {
            continue;
        }
        let Some(pid) = p.get("partId").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(v) = values.get(pid).and_then(|v| v.get("value")).and_then(|v| v.as_str()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(v);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Pure `Email/get` (full) reply parser: `Ok(None)` when the server
/// put the id in `notFound`; loud error when the reply names neither.
fn parse_email_full(
    email_id: &str,
    args: &serde_json::Value,
) -> Result<Option<EmailFull>, String> {
    let not_found = args
        .get("notFound")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|v| v.as_str() == Some(email_id)))
        .unwrap_or(false);
    if not_found {
        return Ok(None);
    }
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(email_id))
        })
        .ok_or_else(|| {
            format!("Email/get: '{email_id}' in neither list nor notFound")
        })?;
    let summary = parse_email_summary_entry(entry)?;
    let attachments = entry
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    // A part without a blobId cannot be fetched — skip
                    // it rather than serve a dead index.
                    let blob_id = p.get("blobId").and_then(|v| v.as_str())?;
                    Some(AttachmentMeta {
                        blob_id: blob_id.to_string(),
                        filename: p
                            .get("name")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                        mime: p
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("application/octet-stream")
                            .to_string(),
                        size: p.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let auth_results = entry
        .get(AUTH_RESULTS_PROP)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(EmailFull {
        cc: parse_addr_list(entry.get("cc")),
        blob_id: entry
            .get("blobId")
            .and_then(|v| v.as_str())
            .map(String::from),
        text: body_text_from(entry, "textBody", "text/plain"),
        html: body_text_from(entry, "htmlBody", "text/html"),
        attachments,
        auth_results,
        summary,
    }))
}

/// Pure `Email/set` (mark-seen) reply parser — same contract as the
/// registry updates: the id must appear in `updated`.
fn parse_email_set_updated(id: &str, args: &serde_json::Value) -> Result<(), String> {
    let updated = args
        .get("updated")
        .and_then(|v| v.as_object())
        .map(|m| m.contains_key(id))
        .unwrap_or(false);
    if updated {
        return Ok(());
    }
    if let Some(err) = args.get("notUpdated").and_then(|v| v.get(id)) {
        return Err(format!("Email/set update rejected — {}", set_error_line(err)));
    }
    Err(format!("Email/set update: '{id}' not in the updated map"))
}

/// The reply-relevant slice of one message (S5 `k2 mail reply`,
/// §8.4). Raw wire values only — verdict parsing (DMARC) and the
/// guardrail decisions live at the ops layer ([`crate::mail::send`]),
/// per the §17.5 route-layer rule.
#[derive(Debug, Clone)]
pub struct ReplyContext {
    pub from: Vec<MailAddr>,
    pub subject: String,
    pub thread_id: Option<String>,
    /// The original's `Message-ID` header (trimmed), for In-Reply-To.
    pub message_id: Option<String>,
    /// The original's raw `References` header text (loop caps + the
    /// outgoing References chain).
    pub references: Option<String>,
    pub auth_results: Vec<String>,
}

/// Pure `Email/get` (reply-context) parser: `Ok(None)` when the server
/// put the id in `notFound`; loud when it names neither.
fn parse_reply_context(
    email_id: &str,
    args: &serde_json::Value,
) -> Result<Option<ReplyContext>, String> {
    let not_found = args
        .get("notFound")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|v| v.as_str() == Some(email_id)))
        .unwrap_or(false);
    if not_found {
        return Ok(None);
    }
    let entry = args
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(email_id))
        })
        .ok_or_else(|| {
            format!("Email/get: '{email_id}' in neither list nor notFound")
        })?;
    let header_text = |prop: &str| -> Option<String> {
        entry
            .get(prop)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let auth_results = entry
        .get(AUTH_RESULTS_PROP)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect())
        .unwrap_or_default();
    Ok(Some(ReplyContext {
        from: parse_addr_list(entry.get("from")),
        subject: entry
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        thread_id: entry
            .get("threadId")
            .and_then(|v| v.as_str())
            .map(String::from),
        message_id: header_text(MSGID_PROP),
        references: header_text(REFS_PROP),
        auth_results,
    }))
}

/// Pure `Identity/get` matcher: the id of the identity whose email
/// equals `from_email` (ASCII-case-insensitive). `None` = no match
/// (the caller falls back to `Identity/set create`).
fn parse_identity_for(args: &serde_json::Value, from_email: &str) -> Option<String> {
    args.get("list")?
        .as_array()?
        .iter()
        .find(|e| {
            e.get("email")
                .and_then(|v| v.as_str())
                .map(|em| em.eq_ignore_ascii_case(from_email))
                .unwrap_or(false)
        })
        .and_then(|e| e.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Pure parser for the combined submit reply: BOTH the `Email/set`
/// create (`k2out`) and the `EmailSubmission/set` create (`k2sub`)
/// must have succeeded — a JMAP-level `error`, a `notCreated` on
/// either, or a missing response is a loud Err carrying the server's
/// SetError (never the message content).
fn parse_submission_created(reply: &serde_json::Value) -> Result<(), String> {
    let responses = reply
        .get("methodResponses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "submission: reply has no methodResponses".to_string())?;
    let mut email_ok = false;
    let mut sub_ok = false;
    for entry in responses {
        let Some(arr) = entry.as_array() else { continue };
        let name = arr.first().and_then(|v| v.as_str()).unwrap_or("");
        let payload = arr.get(1).cloned().unwrap_or(serde_json::Value::Null);
        match name {
            "error" => {
                let etype = payload["type"].as_str().unwrap_or("unknown");
                let desc = payload["description"].as_str().unwrap_or("no description");
                return Err(format!("submission: JMAP error '{etype}': {desc}"));
            }
            "Email/set" => {
                if payload["created"].get(SUBMIT_EMAIL_TAG).is_some() {
                    email_ok = true;
                } else if let Some(err) = payload["notCreated"].get(SUBMIT_EMAIL_TAG) {
                    return Err(format!(
                        "submission: Email/set create rejected — {}",
                        set_error_line(err)
                    ));
                }
            }
            "EmailSubmission/set" => {
                if payload["created"].get(SUBMIT_SUB_TAG).is_some() {
                    sub_ok = true;
                } else if let Some(err) = payload["notCreated"].get(SUBMIT_SUB_TAG) {
                    return Err(format!(
                        "submission: EmailSubmission/set create rejected — {}",
                        set_error_line(err)
                    ));
                }
            }
            _ => {}
        }
    }
    if !email_ok {
        return Err("submission: no successful Email/set create in reply".to_string());
    }
    if !sub_ok {
        return Err("submission: no successful EmailSubmission/set create in reply".to_string());
    }
    Ok(())
}

/// Percent-encode one URI component (RFC 3986 unreserved passthrough).
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Expand the RFC 8620 download template's `{accountId}/{blobId}/
/// {name}?accept={type}` placeholders (each component encoded).
fn expand_download_url(
    template: &str,
    account_id: &str,
    blob_id: &str,
    name: &str,
    mime: &str,
) -> String {
    template
        .replace("{accountId}", &encode_uri_component(account_id))
        .replace("{blobId}", &encode_uri_component(blob_id))
        .replace("{name}", &encode_uri_component(name))
        .replace("{type}", &encode_uri_component(mime))
}

// ── The supervisor's BootstrapApi implementation ────────────────────────

/// Real [`crate::mail::supervisor::BootstrapApi`]: a basic-auth
/// [`StalwartClient`] against whichever listener answers during the
/// enable machine's phase (bootstrap :8080 / post-plan :8180).
#[derive(Default)]
pub struct StalwartBootstrap {
    client: Option<StalwartClient>,
}

impl StalwartBootstrap {
    pub fn new() -> Self {
        Self::default()
    }

    fn client(&self) -> Result<&StalwartClient, String> {
        self.client
            .as_ref()
            .ok_or_else(|| "bootstrap API used before authenticate()".to_string())
    }
}

impl crate::mail::supervisor::BootstrapApi for StalwartBootstrap {
    /// Basic-auth session against `base_url` + session discovery (the
    /// probe doubles as the credential check).
    fn authenticate(
        &mut self,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        let client = StalwartClient::new_basic(base_url, username, password);
        client.discover_session()?;
        self.client = Some(client);
        Ok(())
    }

    fn complete_bootstrap(
        &mut self,
        hostname: &str,
        default_domain: &str,
        request_tls_certificate: bool,
    ) -> Result<AdminCredentials, String> {
        self.client()?
            .bootstrap_complete(hostname, default_domain, request_tls_certificate)
    }

    fn set_server_hostname(&mut self, hostname: &str) -> Result<(), String> {
        self.client()?.set_server_hostname(hostname)
    }

    fn bind_setup_http_loopback(&mut self) -> Result<(), String> {
        let client = self.client()?;
        let listeners = client.listeners_get()?;
        client.listeners_bind_setup_loopback(&listeners)
    }

    fn configure_listeners(&mut self, port_plan: &str) -> Result<(), String> {
        let client = self.client()?;
        let listeners = client.listeners_get()?;
        client.listeners_apply(port_plan, &listeners)
    }

    fn create_service_account(&mut self, default_domain: &str) -> Result<String, String> {
        let client = self.client()?;
        let domain_id = client.domain_query_id(default_domain)?.ok_or_else(|| {
            format!("default domain '{default_domain}' not found in the mail server")
        })?;
        let password = super::secrets::generate_secret()?;
        client.service_account_create(
            "k2-daemon",
            &domain_id,
            &password,
            "K2 daemon mail supervisor",
        )
    }

    fn mint_api_key(&mut self, account_id: &str) -> Result<String, String> {
        self.client()?.api_key_create(account_id)
    }

    fn rotate_admin_secret(
        &mut self,
        username: &str,
        current: &str,
        new_secret: &str,
    ) -> Result<(), String> {
        let _ = current;
        self.client()?.rotate_account_secret(username, new_secret)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// The REAL bootstrap-mode session document served by Stalwart
    /// v0.16.10 (captured live 2026-07-10, trimmed to the fields the
    /// client reads — accounts key + apiUrl are verbatim).
    const BOOTSTRAP_SESSION_FIXTURE: &str = r#"{
        "capabilities": { "urn:ietf:params:jmap:core": { "maxSizeUpload": 50000000 } },
        "accounts": { "d333333": { "name": "admin", "isPersonal": true, "isReadOnly": false } },
        "primaryAccounts": {
            "urn:ietf:params:jmap:mail": "d333333",
            "urn:stalwart:jmap": "d333333"
        },
        "username": "admin",
        "apiUrl": "/jmap/",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "20f78199"
    }"#;

    /// The REAL normal-mode session document shape: absolute URLs on
    /// the MAIL HOSTNAME (captured live 2026-07-10) — the client must
    /// rebase them onto its loopback base.
    const NORMAL_SESSION_FIXTURE: &str = r#"{
        "capabilities": { "urn:ietf:params:jmap:core": {} },
        "accounts": { "b": { "name": "admin@k2livebox.test" } },
        "primaryAccounts": { "urn:ietf:params:jmap:mail": "b", "urn:stalwart:jmap": "b" },
        "username": "admin@k2livebox.test",
        "apiUrl": "https://mail.k2livebox.test/jmap/",
        "downloadUrl": "https://mail.k2livebox.test/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "state": "73a05484"
    }"#;

    #[test]
    fn session_api_url_rebases_absolute_and_relative_onto_base() {
        // Bootstrap mode: root-relative.
        let session: serde_json::Value =
            serde_json::from_str(BOOTSTRAP_SESSION_FIXTURE).expect("fixture JSON");
        assert_eq!(
            parse_session_api_url("http://127.0.0.1:8080", &session).expect("parsed"),
            "http://127.0.0.1:8080/jmap/"
        );
        // Normal mode: ABSOLUTE https url on the mail hostname —
        // only the path survives (live-verified behavior).
        let session: serde_json::Value =
            serde_json::from_str(NORMAL_SESSION_FIXTURE).expect("fixture JSON");
        assert_eq!(
            parse_session_api_url("http://127.0.0.1:8180", &session).expect("parsed"),
            "http://127.0.0.1:8180/jmap/"
        );
        assert_eq!(
            parse_session_download_url("http://127.0.0.1:8180", &session).expect("parsed"),
            "http://127.0.0.1:8180/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
        );
    }

    #[test]
    fn session_account_id_prefers_stalwart_primary_then_first_account() {
        let session: serde_json::Value =
            serde_json::from_str(NORMAL_SESSION_FIXTURE).expect("fixture JSON");
        assert_eq!(parse_session_account_id(&session).expect("parsed"), "b");
        // No primaryAccounts entry → the single accounts key.
        let bare = serde_json::json!({ "accounts": { "d333333": {} } });
        assert_eq!(parse_session_account_id(&bare).expect("parsed"), "d333333");
        // Neither → loud.
        assert!(parse_session_account_id(&serde_json::json!({})).is_err());
    }

    #[test]
    fn missing_or_garbage_api_url_fails_loudly() {
        for session in [
            serde_json::json!({}),
            serde_json::json!({ "apiUrl": "" }),
            serde_json::json!({ "apiUrl": 42 }),
            serde_json::json!({ "capabilities": {} }),
        ] {
            let err = parse_session_api_url("http://127.0.0.1:8180", &session)
                .expect_err("must reject");
            assert!(err.contains("apiUrl"), "{err}");
        }
        // Neither absolute nor root-relative → loud, named value.
        let err = parse_session_api_url(
            "http://127.0.0.1:8180",
            &serde_json::json!({ "apiUrl": "jmap/" }),
        )
        .expect_err("must reject");
        assert!(err.contains("jmap/"), "{err}");
    }

    /// Constructor normalizes a trailing-slash base so path joins never
    /// double the slash.
    #[test]
    fn client_construction_normalizes_base() {
        let c = StalwartClient::new("http://127.0.0.1:8180///", "k2-test-key");
        assert_eq!(c.base_url, "http://127.0.0.1:8180");
    }

    // ── S1 pure parsers (fixtures = REAL live-box replies) ──────────

    /// The REAL `x:Bootstrap/set` reply (captured live 2026-07-10;
    /// secret replaced).
    #[test]
    fn bootstrap_set_reply_yields_provisioned_admin_credentials() {
        let args = serde_json::json!({
            "accountId": "d333333",
            "updated": {
                "singleton": {
                    "username": "admin@k2livebox.test",
                    "secret": "REDACTED-16CHARS"
                }
            }
        });
        let creds = parse_bootstrap_updated(&args).expect("parsed");
        assert_eq!(creds.username, "admin@k2livebox.test");
        assert_eq!(creds.secret, "REDACTED-16CHARS");

        // The REAL first-attempt failure (perm denied writing the
        // config path) — notUpdated surfaces the SetError verbatim.
        let rejected = serde_json::json!({
            "notUpdated": { "singleton": {
                "type": "invalidProperties",
                "description": "Failed to save data store settings: Permission denied",
                "properties": ["dataStore"]
            } }
        });
        let err = parse_bootstrap_updated(&rejected).expect_err("must reject");
        assert!(err.contains("Permission denied"), "{err}");

        // Updated but WITHOUT credentials (external directory case) →
        // loud, never a silent success without an admin.
        let no_creds = serde_json::json!({ "updated": { "singleton": null } });
        assert!(parse_bootstrap_updated(&no_creds).is_err());
        assert!(parse_bootstrap_updated(&serde_json::json!({})).is_err());
    }

    /// The REAL default listener set (ids + names captured live).
    #[test]
    fn listeners_parse_and_port_plan_builds_the_right_set() {
        let reply = serde_json::json!({
            "list": [
                { "id": "iz1vbh9qaeqb", "name": "smtp", "bind": { "[::]:25": true }, "protocol": "smtp" },
                { "id": "iz1vbh9qafab", "name": "submissions", "bind": { "[::]:465": true } },
                { "id": "iz1vbh9qafqb", "name": "imaps", "bind": { "[::]:993": true } },
                { "id": "iz1vbh9sagab", "name": "pop3s", "bind": { "[::]:995": true } },
                { "id": "iz1vbh9sagqb", "name": "sieve", "bind": { "[::]:4190": true } },
                { "id": "iz1vbh9sahab", "name": "https", "bind": { "[::]:443": true } },
                { "id": "iz1vbh9sahqb", "name": "http", "bind": { "[::]:8080": true } },
            ],
            "notFound": []
        });
        let listeners = parse_listeners(&reply).expect("parsed");
        assert_eq!(listeners.len(), 7);
        assert_eq!(listeners[0], ListenerInfo { id: "iz1vbh9qaeqb".into(), name: "smtp".into() });
        // No list → loud; entry without id → loud.
        assert!(parse_listeners(&serde_json::json!({})).is_err());
        assert!(parse_listeners(&serde_json::json!({ "list": [{ "name": "x" }] })).is_err());
    }

    #[test]
    fn set_clean_guard_rejects_any_partial_application() {
        expect_set_clean("x:NetworkListener/set", &serde_json::json!({
            "created": { "k2": { "id": "n1" } },
            "updated": { "l1": null },
            "destroyed": ["l2"],
        }))
        .expect("clean set");
        let partial = serde_json::json!({
            "destroyed": ["l2"],
            "notDestroyed": { "l3": { "type": "notFound" } },
        });
        let err = expect_set_clean("x:NetworkListener/set", &partial).expect_err("must reject");
        assert!(err.contains("notDestroyed"), "{err}");
    }

    #[test]
    fn retired_local_part_marks_and_respects_the_64_char_budget() {
        let r = retired_local_part("research-bot", 1_760_000_000);
        assert_eq!(r, "research-bot-k2r-1760000000");
        assert!(r.contains(RETIRED_MARKER));
        let long = "a".repeat(64);
        let r = retired_local_part(&long, 1_760_000_000);
        assert_eq!(r.len(), 64, "budget respected");
        assert!(r.ends_with("-k2r-1760000000"));
    }

    // ── Loopback mock-server helpers (ephemeral port; the one allowed
    //    form of network in tests) ───────────────────────────────────

    pub(crate) fn spawn_mock_server(
        replies: Vec<String>,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for reply in replies {
                let (mut sock, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                        let want: usize = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if raw.len() >= head_end + 4 + want {
                            break;
                        }
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&raw).to_string());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.len(),
                    reply
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (port, rx)
    }

    pub(crate) fn body_json(req: &str) -> serde_json::Value {
        let start = req.find("\r\n\r\n").expect("body") + 4;
        serde_json::from_str(&req[start..]).expect("JSON body")
    }

    /// Bootstrap round-trip: authenticate discovers /jmap/session with
    /// BASIC auth, complete_bootstrap posts the x:Bootstrap/set update
    /// (singleton id, DataStore RocksDb, hostname/domain) and yields
    /// the provisioned admin credentials.
    #[test]
    fn bootstrap_round_trip_against_loopback_mock() {
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Bootstrap/set", {
                "accountId": "d333333",
                "updated": { "singleton": {
                    "username": "admin@acme.dev", "secret": "s3cr3t16"
                } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) =
            spawn_mock_server(vec![BOOTSTRAP_SESSION_FIXTURE.to_string(), set_reply]);

        use crate::mail::supervisor::BootstrapApi;
        let mut api = StalwartBootstrap::new();
        api.authenticate(&format!("http://127.0.0.1:{port}"), "admin", "recovery-pw")
            .expect("authenticate");
        let creds = api
            .complete_bootstrap("mail.acme.dev", "acme.dev", false)
            .expect("bootstrap");
        assert_eq!(creds.username, "admin@acme.dev");
        assert_eq!(creds.secret, "s3cr3t16");

        let req1 = rx.recv().expect("req1");
        assert!(req1.starts_with("GET /jmap/session"), "{req1}");
        // Basic base64("admin:recovery-pw").
        assert!(
            req1.contains("authorization: Basic YWRtaW46cmVjb3ZlcnktcHc=")
                || req1.contains("Authorization: Basic YWRtaW46cmVjb3ZlcnktcHc="),
            "{req1}"
        );

        let req2 = rx.recv().expect("req2");
        assert!(req2.starts_with("POST /jmap/"), "{req2}");
        let v = body_json(&req2);
        assert_eq!(v["using"][1], "urn:stalwart:jmap");
        assert_eq!(v["methodCalls"][0][0], "x:Bootstrap/set");
        let args = &v["methodCalls"][0][1];
        assert_eq!(args["accountId"], "d333333", "session account id injected");
        let up = &args["update"]["singleton"];
        assert_eq!(up["serverHostname"], "mail.acme.dev");
        assert_eq!(up["defaultDomain"], "acme.dev");
        assert_eq!(up["requestTlsCertificate"], false);
        assert_eq!(up["generateDkimKeys"], true);
        assert_eq!(up["dataStore"]["@type"], "RocksDb");
        assert_eq!(up["dataStore"]["path"], "/var/lib/stalwart/data");
        // Using before authenticate fails loudly.
        let mut cold = StalwartBootstrap::new();
        let err = cold
            .complete_bootstrap("x", "y", false)
            .expect_err("must fail");
        assert!(err.contains("before authenticate"), "{err}");
    }

    /// Port-plan round-trip: one x:NetworkListener/set destroys the
    /// §10 listeners, retargets http to the loopback mgmt bind, binds
    /// https per plan, creates submission :587.
    #[test]
    fn listeners_apply_builds_the_port_plan_set() {
        let get_reply = serde_json::json!({
            "methodResponses": [["x:NetworkListener/get", {
                "accountId": "b",
                "list": [
                    { "id": "L-smtp", "name": "smtp" },
                    { "id": "L-subs", "name": "submissions" },
                    { "id": "L-imaps", "name": "imaps" },
                    { "id": "L-pop3s", "name": "pop3s" },
                    { "id": "L-sieve", "name": "sieve" },
                    { "id": "L-https", "name": "https" },
                    { "id": "L-http", "name": "http" },
                ],
                "notFound": [],
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:NetworkListener/set", {
                "created": { "k2": { "id": "L-new" } },
                "updated": { "L-http": null, "L-https": null },
                "destroyed": ["L-pop3s", "L-sieve"],
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            get_reply,
            set_reply,
        ]);
        use crate::mail::supervisor::BootstrapApi;
        let mut api = StalwartBootstrap::new();
        api.authenticate(&format!("http://127.0.0.1:{port}"), "admin@k2livebox.test", "pw")
            .expect("authenticate");
        api.configure_listeners("http-01").expect("apply");

        let _sess = rx.recv().expect("req1");
        let _get = rx.recv().expect("req2");
        let set = rx.recv().expect("req3");
        let v = body_json(&set);
        assert_eq!(v["methodCalls"][0][0], "x:NetworkListener/set");
        let args = &v["methodCalls"][0][1];
        let destroy: Vec<&str> = args["destroy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(destroy, ["L-pop3s", "L-sieve"], "pop3s/sieve torn out; imaps kept");
        assert!(
            !destroy.contains(&"L-imaps"),
            "Mail.app IMAPS 993 must survive the port plan: {destroy:?}"
        );
        assert_eq!(args["create"]["imap"]["name"], "imap");
        assert_eq!(args["create"]["imap"]["bind"]["[::]:143"], true);
        assert_eq!(args["create"]["imap"]["tlsImplicit"], false);
        assert_eq!(
            args["update"]["L-http"]["bind"]["127.0.0.1:8180"], true,
            "the 8080 listener becomes the loopback mgmt endpoint"
        );
        assert_eq!(
            args["update"]["L-https"]["bind"]["127.0.0.1:8443"], true,
            "http-01 plan keeps https loopback-only"
        );
        let sub = &args["create"]["k2"];
        assert_eq!(sub["name"], "submission");
        assert_eq!(sub["bind"]["[::]:587"], true);
        assert_eq!(sub["protocol"], "smtp");
        assert_eq!(sub["tlsImplicit"], false, "STARTTLS on 587");
        assert!(
            !destroy.contains(&"L-https"),
            "public /login on 443 is a follow-up — do not close https here: {destroy:?}"
        );
    }

    /// L12: bootstrap HTTP bind is 127.0.0.1:8080, never *:8080.
    #[test]
    fn bind_setup_http_loopback_is_127_not_world() {
        let get_reply = serde_json::json!({
            "methodResponses": [["x:NetworkListener/get", {
                "accountId": "b",
                "list": [
                    { "id": "L-http", "name": "http", "bind": { "[::]:8080": true } },
                    { "id": "L-https", "name": "https", "bind": { "[::]:443": true } },
                ],
                "notFound": [],
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:NetworkListener/set", {
                "updated": { "L-http": null },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            get_reply,
            set_reply,
        ]);
        use crate::mail::supervisor::BootstrapApi;
        let mut api = StalwartBootstrap::new();
        api.authenticate(&format!("http://127.0.0.1:{port}"), "admin@k2livebox.test", "pw")
            .expect("authenticate");
        api.bind_setup_http_loopback().expect("bind loopback 8080");

        let _sess = rx.recv().expect("req1");
        let _get = rx.recv().expect("req2");
        let set = rx.recv().expect("req3");
        let v = body_json(&set);
        assert_eq!(v["methodCalls"][0][0], "x:NetworkListener/set");
        let args = &v["methodCalls"][0][1];
        assert_eq!(
            args["update"]["L-http"]["bind"]["127.0.0.1:8080"], true,
            "bootstrap HTTP must bind loopback 8080: {args}"
        );
        assert!(
            args["update"]["L-http"]["bind"].get("[::]:8080").is_none(),
            "must not keep world [::]:8080: {args}"
        );
        assert!(
            args["update"]["L-http"]["bind"].get("0.0.0.0:8080").is_none(),
            "must not keep world 0.0.0.0:8080: {args}"
        );
        assert!(
            args["update"].get("L-https").is_none(),
            "do not close public /login on 443: {args}"
        );
    }

    /// Service-account + ApiKey round-trip: domain id resolved by
    /// query; account is an Admin-role User with object-keyed
    /// credentials (the live-verified list shape); the ApiKey rides
    /// the TARGET accountId and pins the loopback.
    #[test]
    fn service_account_and_api_key_round_trip() {
        let domain_query = serde_json::json!({
            "methodResponses": [["x:Domain/query", { "ids": ["b"] }, "0"]],
        })
        .to_string();
        let account_set = serde_json::json!({
            "methodResponses": [["x:Account/set", {
                "created": { "k2": { "id": "d" } },
            }, "0"]],
        })
        .to_string();
        let key_set = serde_json::json!({
            "methodResponses": [["x:ApiKey/set", {
                "accountId": "d",
                "created": { "k2": { "id": "b", "secret": "API_once-shown" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            domain_query,
            account_set,
            key_set,
        ]);
        use crate::mail::supervisor::BootstrapApi;
        let mut api = StalwartBootstrap::new();
        api.authenticate(&format!("http://127.0.0.1:{port}"), "admin@k2livebox.test", "pw")
            .expect("authenticate");
        let account_id = api.create_service_account("k2livebox.test").expect("account");
        assert_eq!(account_id, "d");
        let secret = api.mint_api_key(&account_id).expect("key");
        assert_eq!(secret, "API_once-shown");

        let _sess = rx.recv().expect("req1");
        let q = body_json(&rx.recv().expect("req2"));
        assert_eq!(q["methodCalls"][0][0], "x:Domain/query");
        assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "k2livebox.test");

        let a = body_json(&rx.recv().expect("req3"));
        assert_eq!(a["methodCalls"][0][0], "x:Account/set");
        let create = &a["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["@type"], "User");
        assert_eq!(create["name"], "k2-daemon");
        assert_eq!(create["domainId"], "b");
        assert_eq!(create["roles"]["@type"], "Admin");
        assert_eq!(
            create["credentials"]["0"]["@type"], "Password",
            "credentials are an INDEX-KEYED OBJECT (live-verified; an array is rejected)"
        );
        assert_eq!(create["credentials"]["0"]["secret"].as_str().unwrap().len(), 64);

        let k = body_json(&rx.recv().expect("req4"));
        assert_eq!(k["methodCalls"][0][0], "x:ApiKey/set");
        let args = &k["methodCalls"][0][1];
        assert_eq!(args["accountId"], "d", "the key rides the TARGET account");
        let create = &args["create"]["k2"];
        assert_eq!(create["allowedIps"]["127.0.0.1"], true, "pre-mortem #13: loopback-pinned");
        assert_eq!(create["permissions"]["@type"], "Inherit");
    }

    #[test]
    fn http_error_replies_surface_status_without_credentials() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let body = r#"{"error":"unauthorized"}"#;
            let resp = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).expect("write");
        });
        let client = StalwartClient::new(format!("http://{addr}"), "bad-key");
        let err = client.get_json(SESSION_PATH).expect_err("401 is an error");
        server.join().expect("server thread");
        assert!(err.contains("401"), "{err}");
        assert!(!err.contains("bad-key"), "credential must never appear in errors: {err}");
    }

    /// Hostname retarget: `x:SystemSettings/set` singleton
    /// `defaultHostname` — not Bootstrap/set, not a wipe.
    #[test]
    fn set_server_hostname_patches_system_settings_not_bootstrap() {
        let set_reply = serde_json::json!({
            "methodResponses": [["x:SystemSettings/set", {
                "accountId": "b",
                "updated": { "singleton": null },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), set_reply]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        client
            .set_server_hostname("mail.lztek.io")
            .expect("set hostname");
        let _sess = rx.recv().expect("session");
        let req = rx.recv().expect("set");
        assert!(req.starts_with("POST /jmap/"), "{req}");
        let v = body_json(&req);
        assert_eq!(v["methodCalls"][0][0], "x:SystemSettings/set");
        assert_ne!(v["methodCalls"][0][0], "x:Bootstrap/set");
        let args = &v["methodCalls"][0][1];
        assert_eq!(args["update"]["singleton"]["defaultHostname"], "mail.lztek.io");
        assert!(
            args["update"]["singleton"].get("serverHostname").is_none(),
            "normal-mode field is defaultHostname, not Bootstrap serverHostname: {args}"
        );
        let err = client
            .set_server_hostname("  ")
            .expect_err("empty hostname must fail loud");
        assert!(err.contains("empty hostname"), "{err}");
    }

    /// Plant envelope: `x:Certificate/set` create uses Text/value +
    /// Text/secret (not File paths), then SystemSettings
    /// `defaultCertificateId`.
    #[test]
    fn certificate_create_envelope_is_text_pem_and_secret_key() {
        let chain = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        let key = "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n";
        let args = certificate_create_args(chain, key);
        let create = &args["create"]["k2"];
        assert_eq!(create["certificate"]["@type"], "Text");
        assert_eq!(create["certificate"]["value"], chain);
        assert!(
            create["certificate"].get("secret").is_none(),
            "certificate PEM is PublicText value, not secret: {create}"
        );
        assert_eq!(create["privateKey"]["@type"], "Text");
        assert_eq!(create["privateKey"]["secret"], key);
        assert!(
            create["privateKey"].get("value").is_none(),
            "privateKey is SecretText secret, not value: {create}"
        );
        assert!(
            create.get("filePath").is_none()
                && create["certificate"].get("filePath").is_none()
                && create["privateKey"].get("filePath").is_none(),
            "ProtectHome=yes: never File paths into ~/.k2/certs: {create}"
        );
        let env = registry_envelope("x:Certificate/set", args);
        assert_eq!(env["methodCalls"][0][0], "x:Certificate/set");
        assert_eq!(env["using"][1], "urn:stalwart:jmap");

        let patch = default_certificate_id_args("cert-plant-1");
        assert_eq!(
            patch["update"]["singleton"]["defaultCertificateId"],
            "cert-plant-1"
        );
    }

    #[test]
    fn certificate_plant_sets_default_certificate_id() {
        let cert_reply = serde_json::json!({
            "methodResponses": [["x:Certificate/set", {
                "accountId": "b",
                "created": { "k2": { "id": "cert-plant-1" } },
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:SystemSettings/set", {
                "accountId": "b",
                "updated": { "singleton": null },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            cert_reply,
            set_reply,
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let id = client
            .certificate_plant(
                "-----BEGIN CERTIFICATE-----\nLEAF\n-----END CERTIFICATE-----\n",
                "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n",
            )
            .expect("plant");
        assert_eq!(id, "cert-plant-1");
        let _sess = rx.recv().expect("session");
        let c = body_json(&rx.recv().expect("certificate/set"));
        assert_eq!(c["methodCalls"][0][0], "x:Certificate/set");
        let create = &c["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["certificate"]["@type"], "Text");
        assert_eq!(create["privateKey"]["@type"], "Text");
        assert!(
            create["privateKey"]["secret"].as_str().unwrap().contains("PRIVATE KEY"),
            "{create}"
        );
        let s = body_json(&rx.recv().expect("systemsettings/set"));
        assert_eq!(s["methodCalls"][0][0], "x:SystemSettings/set");
        assert_eq!(
            s["methodCalls"][0][1]["update"]["singleton"]["defaultCertificateId"],
            "cert-plant-1"
        );
    }

    #[test]
    fn certificate_plant_surfaces_system_settings_method_error() {
        let cert_reply = serde_json::json!({
            "methodResponses": [["x:Certificate/set", {
                "created": { "k2": { "id": "cert-x" } },
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:SystemSettings/set", {
                "notUpdated": { "singleton": {
                    "type": "invalidPatch",
                    "description": "Failed to parse Id from string | Properties: defaultCertificateId",
                } },
            }, "0"]],
        })
        .to_string();
        let (port, _rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            cert_reply,
            set_reply,
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let err = client
            .certificate_plant("-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----\n", "-----BEGIN PRIVATE KEY-----\nY\n-----END PRIVATE KEY-----\n")
            .expect_err("must fail loud");
        assert!(err.contains("invalidPatch"), "{err}");
        assert!(err.contains("defaultCertificateId"), "{err}");
    }

    /// Cert renew: `x:Task/set` create AcmeRenewal for the mail
    /// hostname's domain — no extra SAN names, no enable/bootstrap.
    #[test]
    fn renew_acme_records_task_create_without_enable() {
        let query_reply = serde_json::json!({
            "methodResponses": [["x:Domain/query", {
                "accountId": "b",
                "ids": ["dom-mail"],
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Task/set", {
                "accountId": "b",
                "created": { "k2": { "id": "task-acme-1" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            query_reply,
            set_reply,
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let id = client
            .renew_acme_for_mail_hostname("mail.acme.dev")
            .expect("renew");
        assert_eq!(id, "task-acme-1");
        let _sess = rx.recv().expect("session");
        let q = body_json(&rx.recv().expect("query"));
        assert_eq!(q["methodCalls"][0][0], "x:Domain/query");
        assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "mail.acme.dev");
        let t = body_json(&rx.recv().expect("task"));
        assert_eq!(t["methodCalls"][0][0], "x:Task/set");
        let create = &t["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["@type"], "AcmeRenewal");
        assert_eq!(create["domainId"], "dom-mail");
        assert!(
            create.get("subjectAlternativeNames").is_none(),
            "C8/C24: optional names must not ride the renew request: {create}"
        );
        assert!(
            create.get("names").is_none(),
            "C8/C24: no extra names field: {create}"
        );
        assert_ne!(t["methodCalls"][0][0], "x:Bootstrap/set");
    }

    #[test]
    fn renew_acme_falls_back_to_default_domain_then_fails_loud() {
        let miss = serde_json::json!({
            "methodResponses": [["x:Domain/query", {
                "accountId": "b",
                "ids": [],
            }, "0"]],
        })
        .to_string();
        let hit = serde_json::json!({
            "methodResponses": [["x:Domain/query", {
                "accountId": "b",
                "ids": ["dom-parent"],
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Task/set", {
                "created": { "k2": { "id": "task-2" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            miss.clone(),
            hit,
            set_reply,
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        client
            .renew_acme_for_mail_hostname("mail.acme.dev")
            .expect("parent domain");
        let _sess = rx.recv().expect("session");
        let q1 = body_json(&rx.recv().expect("query host"));
        assert_eq!(q1["methodCalls"][0][1]["filter"]["name"], "mail.acme.dev");
        let q2 = body_json(&rx.recv().expect("query parent"));
        assert_eq!(q2["methodCalls"][0][1]["filter"]["name"], "acme.dev");
        let t = body_json(&rx.recv().expect("task"));
        assert_eq!(t["methodCalls"][0][1]["create"]["k2"]["domainId"], "dom-parent");

        let (port2, _rx2) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            miss.clone(),
            miss,
        ]);
        let client2 = StalwartClient::new(format!("http://127.0.0.1:{port2}"), "k2-test-key");
        let err = client2
            .renew_acme_for_mail_hostname("mail.missing.test")
            .expect_err("no domain must fail loud");
        assert!(err.contains("mail.missing.test"), "{err}");
        assert!(err.contains("cannot retry ACME"), "{err}");
    }
}

#[cfg(test)]
mod s2_domain_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    #[test]
    fn envelope_carries_registry_using_and_single_method_call() {
        let env = registry_envelope("x:Domain/set", serde_json::json!({"destroy": ["d1"]}));
        assert_eq!(env["using"][0], "urn:ietf:params:jmap:core");
        assert_eq!(env["using"][1], "urn:stalwart:jmap");
        assert_eq!(env["methodCalls"][0][0], "x:Domain/set");
        assert_eq!(env["methodCalls"][0][1]["destroy"][0], "d1");
        assert_eq!(env["methodCalls"][0][2], "0");
    }

    #[test]
    fn method_response_unwraps_matching_method_and_surfaces_errors() {
        let ok = serde_json::json!({
            "methodResponses": [["x:Domain/set", { "created": {} }, "0"]],
            "sessionState": "s1",
        });
        let args = parse_method_response("x:Domain/set", &ok).expect("unwrapped");
        assert!(args.get("created").is_some());

        // JMAP-level error → named type + description (the REAL shape
        // a wrong method name earns on the live box).
        let err = serde_json::json!({
            "methodResponses": [["error", {
                "type": "unknownMethod",
                "description": "Domain/set is not known",
            }, "0"]],
        });
        let msg = parse_method_response("x:Domain/set", &err).expect_err("must reject");
        assert!(msg.contains("unknownMethod"), "{msg}");

        // Not a JMAP response at all → loud.
        let msg = parse_method_response("x:Domain/set", &serde_json::json!({"ok": true}))
            .expect_err("must reject");
        assert!(msg.contains("no methodResponses"), "{msg}");

        // A different method answering → loud.
        let odd = serde_json::json!({
            "methodResponses": [["x:Domain/get", {}, "0"]],
        });
        let msg = parse_method_response("x:Domain/set", &odd).expect_err("must reject");
        assert!(msg.contains("x:Domain/get"), "{msg}");
    }

    /// The REAL create reply (live 2026-07-10): id only — no zone file
    /// rides the create.
    #[test]
    fn domain_set_created_parses_id_and_rejections() {
        let args = serde_json::json!({ "accountId": "d", "created": { "k2": { "id": "c" } } });
        assert_eq!(
            parse_set_created_id("x:Domain/set", &args).expect("created"),
            "c"
        );

        let rejected = serde_json::json!({
            "notCreated": { "k2": { "type": "alreadyExists",
                                     "description": "domain exists" } },
        });
        let msg = parse_set_created_id("x:Domain/set", &rejected).expect_err("must reject");
        assert!(msg.contains("alreadyExists"), "{msg}");
        assert!(msg.contains("domain exists"), "{msg}");

        assert!(parse_set_created_id("x:Domain/set", &serde_json::json!({})).is_err());
    }

    #[test]
    fn domain_set_destroyed_requires_id_in_destroyed_list() {
        let ok = serde_json::json!({ "destroyed": ["dom-1", "dom-2"] });
        parse_set_destroyed("x:Domain/set", "dom-1", &ok).expect("destroyed");

        // The REAL refusal shape when DkimSignatures still link the
        // domain (live-verified: `objectIsLinked`).
        let rejected = serde_json::json!({
            "notDestroyed": { "dom-1": {
                "type": "objectIsLinked",
                "linkedObjects": [{ "object": "DkimSignature", "id": "iz1v70naabqa" }],
            } },
        });
        let msg =
            parse_set_destroyed("x:Domain/set", "dom-1", &rejected).expect_err("must reject");
        assert!(msg.contains("objectIsLinked"), "{msg}");

        let silent = serde_json::json!({ "destroyed": [] });
        assert!(parse_set_destroyed("x:Domain/set", "dom-1", &silent).is_err());
    }

    /// A trimmed REAL dnsZoneFile (live 2026-07-10) — MX + SPF + DKIM +
    /// DMARC lines exactly as served.
    const ZONE_FIXTURE: &str = "v1-ed25519-20260710._domainkey.k2livebox.test. IN TXT \"v=DKIM1; k=ed25519; h=sha256; p=1bt0i...\"\nk2livebox.test. IN TXT \"v=spf1 mx -all\"\nk2livebox.test. IN MX 10 mail.k2livebox.test.\n_dmarc.k2livebox.test. IN TXT \"v=DMARC1; p=reject; rua=mailto:postmaster@k2livebox.test\"\n";

    #[test]
    fn domain_get_zonefile_finds_our_id() {
        let args = serde_json::json!({
            "list": [
                { "id": "dom-other", "dnsZoneFile": "wrong.zone" },
                { "id": "b", "dnsZoneFile": ZONE_FIXTURE },
            ],
            "notFound": [],
        });
        let zone = parse_domain_get_zonefile("b", &args).expect("found");
        assert!(zone.contains("IN MX 10 mail.k2livebox.test."));
        assert!(zone.contains("_domainkey"));
        // Missing id / empty zone file → loud.
        assert!(parse_domain_get_zonefile("dom-9", &args).is_err());
        let empty = serde_json::json!({ "list": [{ "id": "b", "dnsZoneFile": "  " }] });
        assert!(parse_domain_get_zonefile("b", &empty).is_err());
    }

    /// Full `domain_create` round-trip: session discovery (once,
    /// cached), the adopt-check query (empty → create), then the
    /// x:Domain/set create envelope with the §6.1 args in the
    /// live-verified shapes.
    #[test]
    fn domain_create_round_trip_against_loopback_mock() {
        let query_reply = serde_json::json!({
            "methodResponses": [["x:Domain/query", { "ids": [] }, "0"]],
        })
        .to_string();
        let created_reply = serde_json::json!({
            "methodResponses": [["x:Domain/set", {
                "accountId": "b",
                "created": { "k2": { "id": "c" } },
            }, "0"]],
        })
        .to_string();
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
                .to_string();
        let (port, rx) = spawn_mock_server(vec![session, query_reply, created_reply]);

        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let created = c.domain_create("acme.dev").expect("create round-trip");
        assert_eq!(created.id, "c");
        assert_eq!(created.dns_zone_file, None, "zone file is a separate get");

        let req1 = rx.recv().expect("first request recorded");
        assert!(req1.starts_with("GET /jmap/session"), "{req1}");
        assert!(req1.contains("authorization: Bearer k2-test-key")
            || req1.contains("Authorization: Bearer k2-test-key"), "{req1}");

        let q = body_json(&rx.recv().expect("adopt-check query"));
        assert_eq!(q["methodCalls"][0][0], "x:Domain/query");
        assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "acme.dev");

        let req2 = rx.recv().expect("create request recorded");
        assert!(req2.starts_with("POST /jmap/"), "{req2}");
        let body = body_json(&req2);
        assert_eq!(body["methodCalls"][0][0], "x:Domain/set");
        assert_eq!(body["methodCalls"][0][1]["accountId"], "b");
        let create = &body["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["name"], "acme.dev");
        assert_eq!(create["isEnabled"], true);
        assert_eq!(create["dkimManagement"]["@type"], "Automatic");
        assert_eq!(create["subAddressing"]["@type"], "Enabled");
        assert!(create["catchAllAddress"].is_null(), "catch-all OFF by default");
        assert_eq!(create["dnsManagement"]["@type"], "Manual", "K2 never controls user DNS");
        assert_eq!(create["certificateManagement"]["@type"], "Manual");
    }

    /// The guided-setup default domain already exists in Stalwart —
    /// `domain_create` ADOPTS it (query hit → no create call).
    #[test]
    fn domain_create_adopts_an_existing_domain() {
        let query_reply = serde_json::json!({
            "methodResponses": [["x:Domain/query", { "ids": ["b"] }, "0"]],
        })
        .to_string();
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
                .to_string();
        let (port, rx) = spawn_mock_server(vec![session, query_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let created = c.domain_create("k2livebox.test").expect("adopt");
        assert_eq!(created.id, "b");
        let _sess = rx.recv().expect("req1");
        let _query = rx.recv().expect("req2");
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(),
            "no create call when adopting"
        );
    }

    /// domain_delete cascades the DkimSignature children first (the
    /// live-verified objectIsLinked rule).
    #[test]
    fn domain_delete_cascades_dkim_signatures() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
                .to_string();
        let dkim_query = serde_json::json!({
            "methodResponses": [["x:DkimSignature/query", {
                "ids": ["dk1", "dk2"],
            }, "0"]],
        })
        .to_string();
        let dkim_destroy = serde_json::json!({
            "methodResponses": [["x:DkimSignature/set", {
                "destroyed": ["dk1", "dk2"],
            }, "0"]],
        })
        .to_string();
        let domain_destroy = serde_json::json!({
            "methodResponses": [["x:Domain/set", { "destroyed": ["c"] }, "0"]],
        })
        .to_string();
        let (port, rx) =
            spawn_mock_server(vec![session, dkim_query, dkim_destroy, domain_destroy]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.domain_delete("c").expect("cascade delete");

        let _sess = rx.recv().expect("req1");
        let q = body_json(&rx.recv().expect("req2"));
        assert_eq!(q["methodCalls"][0][0], "x:DkimSignature/query");
        assert_eq!(q["methodCalls"][0][1]["filter"]["domainId"], "c");
        let d = body_json(&rx.recv().expect("req3"));
        assert_eq!(d["methodCalls"][0][0], "x:DkimSignature/set");
        assert_eq!(d["methodCalls"][0][1]["destroy"][0], "dk1");
        let dd = body_json(&rx.recv().expect("req4"));
        assert_eq!(dd["methodCalls"][0][0], "x:Domain/set");
        assert_eq!(dd["methodCalls"][0][1]["destroy"][0], "c");
    }
}

#[cfg(test)]
mod s3_account_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    // ── Pure reply parsers (fixtures, no network) ───────────────────

    #[test]
    fn account_set_created_parses_id_and_surfaces_rejections() {
        // The REAL create reply shape (live 2026-07-10).
        let ok = serde_json::json!({ "accountId": "d", "created": { "k2": { "id": "e" } } });
        assert_eq!(parse_set_created_id("x:Account/set", &ok).expect("created"), "e");

        // Server-set id missing/blank → loud.
        let no_id = serde_json::json!({ "created": { "k2": { "quota": 1 } } });
        assert!(parse_set_created_id("x:Account/set", &no_id).is_err());
        let blank = serde_json::json!({ "created": { "k2": { "id": "  " } } });
        assert!(parse_set_created_id("x:Account/set", &blank).is_err());

        // The REAL wrong-shape rejection (credentials as a JSON array)
        // — notCreated surfaces the server's SetError verbatim.
        let rejected = serde_json::json!({
            "notCreated": { "k2": { "type": "invalidPatch",
                                    "description": "Invalid value for object property",
                                    "properties": ["credentials"] } },
        });
        let msg = parse_set_created_id("x:Account/set", &rejected).expect_err("must reject");
        assert!(msg.contains("invalidPatch"), "{msg}");

        // Neither → loud.
        assert!(parse_set_created_id("x:Account/set", &serde_json::json!({})).is_err());
    }

    #[test]
    fn account_set_updated_requires_id_in_updated_map() {
        // RFC 8620 /set: updated maps id → null (the live reply shape).
        let ok = serde_json::json!({ "updated": { "f": null } });
        parse_set_updated("x:Account/set", "f", &ok).expect("updated");

        let rejected = serde_json::json!({
            "notUpdated": { "f": { "type": "forbidden" } },
        });
        let msg = parse_set_updated("x:Account/set", "f", &rejected).expect_err("must reject");
        assert!(msg.contains("forbidden"), "{msg}");

        // Someone ELSE updated / empty reply → loud, never a silent ok.
        let other = serde_json::json!({ "updated": { "zz": null } });
        assert!(parse_set_updated("x:Account/set", "f", &other).is_err());
        assert!(parse_set_updated("x:Account/set", "f", &serde_json::json!({})).is_err());
    }

    #[test]
    fn account_get_name_parses_the_list_entry() {
        let args = serde_json::json!({
            "list": [{ "id": "e", "name": "research-bot" }],
            "notFound": [],
        });
        assert_eq!(parse_account_get_name("e", &args).expect("name"), "research-bot");
        assert!(parse_account_get_name("zz", &args).is_err());
    }

    #[test]
    fn account_create_round_trip_against_loopback_mock() {
        let created_reply = serde_json::json!({
            "methodResponses": [["x:Account/set", {
                "created": { "k2": { "id": "e" } },
            }, "0"]],
        })
        .to_string();
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let (port, rx) = spawn_mock_server(vec![session, created_reply]);

        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let id = c
            .account_create("research-bot", "b", "s3cret-pw", 1_073_741_824, 10_000)
            .expect("create round-trip");
        assert_eq!(id, "e");

        let req1 = rx.recv().expect("first request recorded");
        assert!(req1.starts_with("GET /jmap/session"), "{req1}");

        // The JMAP envelope with the LIVE-VERIFIED create shape.
        let req2 = rx.recv().expect("second request recorded");
        assert!(req2.starts_with("POST /jmap/"), "{req2}");
        let body = body_json(&req2);
        assert_eq!(body["using"][1], "urn:stalwart:jmap");
        assert_eq!(body["methodCalls"][0][0], "x:Account/set");
        let create = &body["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["@type"], "User");
        assert_eq!(create["name"], "research-bot");
        assert_eq!(create["domainId"], "b");
        assert_eq!(create["credentials"]["0"]["@type"], "Password");
        assert_eq!(create["credentials"]["0"]["secret"], "s3cret-pw");
        assert_eq!(create["quotas"]["maxDiskQuota"], 1_073_741_824u64, "§12: 1 GB quota");
        assert_eq!(create["quotas"]["maxEmails"], 10_000, "§12: 10k message cap");
    }

    #[test]
    fn account_set_quotas_records_account_set_update() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.account_set_quotas("e", Some(3_221_225_472), Some(40_000))
            .expect("quota update");

        let _sess = rx.recv().expect("session");
        let req = rx.recv().expect("Account/set");
        assert!(req.starts_with("POST /jmap/"), "{req}");
        let body = body_json(&req);
        assert_eq!(body["methodCalls"][0][0], "x:Account/set");
        let patch = &body["methodCalls"][0][1]["update"]["e"]["quotas"];
        assert_eq!(patch["maxDiskQuota"], 3_221_225_472u64);
        assert_eq!(patch["maxEmails"], 40_000);
    }

    #[test]
    fn account_set_quotas_zero_is_unlimited() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.account_set_quotas("e", Some(0), None).expect("0 = unlimited");
        let _sess = rx.recv().expect("session");
        let req = rx.recv().expect("Account/set");
        let body = body_json(&req);
        let patch = &body["methodCalls"][0][1]["update"]["e"]["quotas"];
        assert_eq!(patch["maxDiskQuota"], 0);
        assert!(patch.get("maxEmails").is_none(), "unset field omitted: {patch}");
    }

    #[test]
    fn account_get_quotas_parses_the_list_entry() {
        let args = serde_json::json!({
            "list": [{ "id": "e", "quotas": { "maxDiskQuota": 1073741824u64, "maxEmails": 10000 } }],
            "notFound": [],
        });
        let q = parse_account_get_quotas("e", &args).expect("quotas");
        assert_eq!(q["maxDiskQuota"], 1_073_741_824u64);
        assert_eq!(q["maxEmails"], 10_000);
        assert!(parse_account_get_quotas("zz", &args).is_err());
    }

    /// Retire = rename (the live-verified v0.16 disable mechanism):
    /// get the current name, rename to the retired local part;
    /// idempotent when already retired.
    #[test]
    fn account_disable_renames_and_is_idempotent() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let get_reply = serde_json::json!({
            "methodResponses": [["x:Account/get", {
                "list": [{ "id": "e", "name": "research-bot" }], "notFound": [],
            }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session.clone(), get_reply, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.account_disable("e").expect("disable");
        let _sess = rx.recv().expect("req1");
        let _get = rx.recv().expect("req2");
        let set = body_json(&rx.recv().expect("req3"));
        let new_name = set["methodCalls"][0][1]["update"]["e"]["name"]
            .as_str()
            .expect("rename update");
        assert!(new_name.starts_with("research-bot-k2r-"), "{new_name}");

        // Already-retired name → no update call happens (idempotent).
        let retired_get = serde_json::json!({
            "methodResponses": [["x:Account/get", {
                "list": [{ "id": "e", "name": "research-bot-k2r-1760000000" }], "notFound": [],
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, retired_get]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.account_disable("e").expect("idempotent disable");
        let _sess = rx.recv().expect("req1");
        let _get = rx.recv().expect("req2");
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(),
            "no rename when already retired"
        );
    }

    #[test]
    fn account_engine_errors_never_leak_the_password() {
        // A refused/failed create must surface the transport error
        // without the secret riding along (mirrors the S1 credential
        // rule). Closed port → immediate refusal, no live dial.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener); // port now closed
        let c = StalwartClient::new(format!("http://{addr}"), "k2-test-key");
        let err = c
            .account_create("bot", "dom-1", "super-secret-pw", 1, 1)
            .expect_err("closed port must fail");
        assert!(!err.contains("super-secret-pw"), "password leaked: {err}");
    }

    #[test]
    fn domain_set_catchall_sends_null_not_empty_and_omits_aliases() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Domain/set", { "updated": { "dom-1": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session.clone(), set_reply.clone()]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.domain_set_catchall("dom-1", None).expect("unset");
        let _sess = rx.recv().expect("session");
        let req = rx.recv().expect("Domain/set");
        let body = body_json(&req);
        assert_eq!(body["methodCalls"][0][0], "x:Domain/set");
        let patch = &body["methodCalls"][0][1]["update"]["dom-1"];
        assert!(patch["catchAllAddress"].is_null(), "{patch}");
        assert!(
            patch.get("aliases").is_none(),
            "Domain.aliases must never be written: {patch}"
        );

        let set_reply = serde_json::json!({
            "methodResponses": [["x:Domain/set", { "updated": { "dom-1": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.domain_set_catchall("dom-1", Some("post@acme.dev"))
            .expect("set");
        let _sess = rx.recv().expect("session");
        let req = rx.recv().expect("Domain/set");
        let body = body_json(&req);
        let patch = &body["methodCalls"][0][1]["update"]["dom-1"];
        assert_eq!(patch["catchAllAddress"], "post@acme.dev");
        assert!(patch.get("aliases").is_none(), "{patch}");
    }

    #[test]
    fn domain_get_catchall_parses_null_and_addr() {
        let null = serde_json::json!({
            "list": [{ "id": "dom-1", "catchAllAddress": null }],
            "notFound": [],
        });
        assert_eq!(parse_domain_get_catchall("dom-1", &null).expect("null"), None);
        let addr = serde_json::json!({
            "list": [{ "id": "dom-1", "catchAllAddress": "post@acme.dev" }],
        });
        assert_eq!(
            parse_domain_get_catchall("dom-1", &addr).expect("addr"),
            Some("post@acme.dev".into())
        );
        assert!(parse_domain_get_catchall("zz", &addr).is_err());
    }

    #[test]
    fn account_set_aliases_is_index_keyed_not_array() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.account_set_aliases(
            "e",
            &[EmailAlias {
                name: "sales".into(),
                domain_id: "dom-1".into(),
                enabled: true,
            }],
        )
        .expect("set aliases");
        let _sess = rx.recv().expect("session");
        let req = rx.recv().expect("Account/set");
        let body = body_json(&req);
        assert_eq!(body["methodCalls"][0][0], "x:Account/set");
        assert_ne!(body["methodCalls"][0][0], "x:User/set");
        let aliases = &body["methodCalls"][0][1]["update"]["e"]["aliases"];
        assert!(aliases.is_object(), "index-keyed object, not array: {aliases}");
        assert!(aliases.as_array().is_none(), "{aliases}");
        assert_eq!(aliases["0"]["name"], "sales");
        assert_eq!(aliases["0"]["domainId"], "dom-1");
        assert_eq!(aliases["0"]["enabled"], true);
    }

    #[test]
    fn parse_email_aliases_index_keyed_and_array() {
        let keyed = serde_json::json!({
            "1": { "name": "info", "domainId": "d", "enabled": true },
            "0": { "name": "Sales", "domainId": "d", "enabled": true },
        });
        let got = parse_email_aliases(Some(&keyed));
        assert_eq!(got[0].name, "sales");
        assert_eq!(got[1].name, "info");
        let arr = serde_json::json!([{ "name": "x", "domainId": "d" }]);
        assert_eq!(parse_email_aliases(Some(&arr))[0].name, "x");
        assert!(parse_email_aliases(Some(&serde_json::Value::Null)).is_empty());
    }

    #[test]
    fn k2_forward_script_keep_and_default() {
        let def = k2_forward_script("dest@acme.dev", false);
        assert!(def.contains("redirect \"dest@acme.dev\";"), "{def}");
        assert!(!def.contains(":copy"), "{def}");
        assert!(!def.contains("require"), "{def}");
        let keep = k2_forward_script("dest@acme.dev", true);
        assert!(keep.contains("require [\"copy\"];"), "{keep}");
        assert!(keep.contains("redirect :copy \"dest@acme.dev\";"), "{keep}");
        assert_eq!(
            parse_k2_forward_script(&def),
            Some(("dest@acme.dev".into(), false))
        );
        assert_eq!(
            parse_k2_forward_script(&keep),
            Some(("dest@acme.dev".into(), true))
        );
    }

    #[test]
    fn sieve_k2_forward_set_declares_sieve_capability_and_name() {
        let session = serde_json::json!({
            "apiUrl": "/jmap/",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "accounts": { "b": {} },
            "primaryAccounts": { "urn:stalwart:jmap": "b" },
        })
        .to_string();
        let blob = serde_json::json!({ "blobId": "Bfwd" }).to_string();
        let query = serde_json::json!({
            "methodResponses": [["SieveScript/query", { "ids": [] }, "0"]],
        })
        .to_string();
        let set = serde_json::json!({
            "methodResponses": [["SieveScript/set", {
                "created": { "k2": { "id": "s1" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            session.clone(),
            blob,
            session,
            query,
            set,
        ]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.sieve_k2_forward_set("acc-1", "dest@acme.dev", true)
            .expect("set forward");

        let blob_sess = rx.recv().expect("blob session");
        assert!(blob_sess.starts_with("GET /jmap/session"), "{blob_sess}");
        let blob_req = rx.recv().expect("blob POST");
        assert!(blob_req.contains("POST /jmap/upload/acc-1/"), "{blob_req}");
        assert!(
            blob_req.contains("require [\"copy\"];") && blob_req.contains("redirect :copy"),
            "{blob_req}"
        );
        let _disc = rx.recv().expect("sieve session");
        let q = body_json(&rx.recv().expect("query"));
        assert_eq!(q["methodCalls"][0][0], "SieveScript/query");
        assert_eq!(q["using"][1], "urn:ietf:params:jmap:sieve");
        assert_eq!(q["methodCalls"][0][1]["filter"]["name"], K2_FORWARD_SCRIPT);
        let set_req = body_json(&rx.recv().expect("set"));
        assert_eq!(set_req["methodCalls"][0][0], "SieveScript/set");
        assert_ne!(set_req["methodCalls"][0][0], "x:SieveScript/set");
        assert_ne!(set_req["methodCalls"][0][0], "x:SieveSystemScript/set");
        assert_eq!(set_req["using"][1], "urn:ietf:params:jmap:sieve");
        let create = &set_req["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["name"], "k2-forward");
        assert_eq!(create["blobId"], "Bfwd");
        assert_eq!(
            set_req["methodCalls"][0][1]["onSuccessActivateScript"],
            "#k2"
        );
    }

    #[test]
    fn sieve_k2_forward_destroy_only_that_name() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
                .to_string();
        let query = serde_json::json!({
            "methodResponses": [["SieveScript/query", { "ids": ["s1"] }, "0"]],
        })
        .to_string();
        let destroy = serde_json::json!({
            "methodResponses": [["SieveScript/set", { "destroyed": ["s1"] }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, query, destroy]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.sieve_k2_forward_destroy("acc-1").expect("destroy");
        let _sess = rx.recv().expect("session");
        let q = body_json(&rx.recv().expect("query"));
        assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "k2-forward");
        let d = body_json(&rx.recv().expect("destroy"));
        assert_eq!(d["methodCalls"][0][0], "SieveScript/set");
        assert_eq!(d["methodCalls"][0][1]["destroy"][0], "s1");
        assert!(
            d["methodCalls"][0][1].get("create").is_none(),
            "unset must not recreate: {d}"
        );
    }
}

#[cfg(test)]
mod s4_mail_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    // ── Pure fixture parsers (no network) ───────────────────────────

    /// A realistic `Email/get` summaries reply entry set: one unread
    /// with attachment, one read.
    fn summaries_fixture() -> serde_json::Value {
        serde_json::json!({
            "accountId": "acc-1",
            "state": "s1",
            "list": [
                {
                    "id": "M1",
                    "threadId": "T1",
                    "from": [{ "name": "GitHub", "email": "noreply@github.com" }],
                    "to": [{ "name": null, "email": "bot@acme.dev" }],
                    "subject": "Verify your device",
                    "receivedAt": "2026-07-08T10:15:00Z",
                    "keywords": {},
                    "hasAttachment": true
                },
                {
                    "id": "M2",
                    "threadId": "T2",
                    "from": [{ "email": "news@example.com" }],
                    "to": [{ "email": "bot@acme.dev" }],
                    "subject": "Weekly digest",
                    "receivedAt": "2026-07-07T09:00:00Z",
                    "keywords": { "$seen": true },
                    "hasAttachment": false
                }
            ],
            "notFound": []
        })
    }

    #[test]
    fn summaries_parse_envelope_unread_and_attachments() {
        let list = parse_email_summaries(&summaries_fixture()).expect("parsed");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "M1");
        assert_eq!(list[0].thread_id.as_deref(), Some("T1"));
        assert_eq!(list[0].from[0].name.as_deref(), Some("GitHub"));
        assert_eq!(list[0].from[0].email, "noreply@github.com");
        assert_eq!(list[0].subject, "Verify your device");
        assert!(list[0].unread, "no $seen keyword = unread");
        assert!(list[0].has_attachment);
        assert!(!list[1].unread, "$seen = read");
        // No list at all → loud; an entry without id → loud.
        assert!(parse_email_summaries(&serde_json::json!({})).is_err());
        let bad = serde_json::json!({ "list": [{ "subject": "x" }] });
        assert!(parse_email_summaries(&bad).is_err());
    }

    /// A realistic full `Email/get` reply: multipart text+html with
    /// bodyValues, one attachment, auth-results headers, blobId.
    fn full_fixture() -> serde_json::Value {
        serde_json::json!({
            "accountId": "acc-1",
            "list": [{
                "id": "M1",
                "blobId": "B-raw",
                "threadId": "T1",
                "from": [{ "name": "GitHub", "email": "noreply@github.com" }],
                "to": [{ "email": "bot@acme.dev" }],
                "cc": [{ "name": "Ops", "email": "ops@acme.dev" }],
                "subject": "Verify your device",
                "receivedAt": "2026-07-08T10:15:00Z",
                "keywords": {},
                "hasAttachment": true,
                "textBody": [
                    { "partId": "1", "type": "text/plain" },
                    { "partId": "3", "type": "text/plain" }
                ],
                "htmlBody": [{ "partId": "2", "type": "text/html" }],
                "bodyValues": {
                    "1": { "value": "Your code is 424242.", "isTruncated": false },
                    "2": { "value": "<p>Your code is <b>424242</b>.</p>", "isTruncated": false },
                    "3": { "value": "-- footer", "isTruncated": false }
                },
                "attachments": [
                    { "blobId": "B1", "name": "invite.ics", "type": "text/calendar", "size": 512 },
                    { "name": "no-blob.bin", "type": "application/octet-stream", "size": 9 }
                ],
                "header:Authentication-Results:asText:all": [
                    " mail.acme.dev; spf=pass smtp.mailfrom=github.com; dkim=pass header.d=github.com; dmarc=pass"
                ]
            }],
            "notFound": []
        })
    }

    #[test]
    fn full_parse_assembles_bodies_attachments_and_auth_headers() {
        let full = parse_email_full("M1", &full_fixture())
            .expect("parsed")
            .expect("present");
        assert_eq!(full.summary.id, "M1");
        assert_eq!(full.blob_id.as_deref(), Some("B-raw"));
        assert_eq!(
            full.text.as_deref(),
            Some("Your code is 424242.\n-- footer"),
            "multiple text parts concatenate in order"
        );
        assert!(full.html.as_deref().unwrap().contains("<b>424242</b>"));
        assert_eq!(full.cc[0].email, "ops@acme.dev");
        assert_eq!(full.attachments.len(), 1, "blob-less part is skipped");
        assert_eq!(full.attachments[0].blob_id, "B1");
        assert_eq!(full.attachments[0].filename.as_deref(), Some("invite.ics"));
        assert_eq!(full.attachments[0].mime, "text/calendar");
        assert_eq!(full.attachments[0].size, 512);
        assert!(full.auth_results[0].contains("spf=pass"));

        // notFound → Ok(None) (the route masks it).
        let nf = serde_json::json!({ "list": [], "notFound": ["M9"] });
        assert!(parse_email_full("M9", &nf).expect("parsed").is_none());
        // Neither list nor notFound → loud.
        assert!(parse_email_full("M9", &serde_json::json!({ "list": [] })).is_err());
    }

    #[test]
    fn email_set_updated_parses_ok_and_rejections() {
        let ok = serde_json::json!({ "updated": { "M1": null } });
        parse_email_set_updated("M1", &ok).expect("updated");
        let rejected = serde_json::json!({
            "notUpdated": { "M1": { "type": "notFound" } },
        });
        let msg = parse_email_set_updated("M1", &rejected).expect_err("must reject");
        assert!(msg.contains("notFound"), "{msg}");
        assert!(parse_email_set_updated("M1", &serde_json::json!({})).is_err());
    }

    #[test]
    fn download_url_discovery_and_expansion() {
        let session = serde_json::json!({
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
        });
        let template = parse_session_download_url("http://127.0.0.1:8180", &session)
            .expect("template");
        let url = expand_download_url(&template, "acc 1", "B/1", "réport.pdf", "application/pdf");
        assert_eq!(
            url,
            "http://127.0.0.1:8180/jmap/download/acc%201/B%2F1/r%C3%A9port.pdf?accept=application%2Fpdf"
        );
        // Missing/garbage downloadUrl → loud.
        assert!(parse_session_download_url("http://x", &serde_json::json!({})).is_err());
        assert!(
            parse_session_download_url("http://x", &serde_json::json!({ "downloadUrl": "jmap/" }))
                .is_err()
        );
    }

    #[test]
    fn reply_context_parses_headers_and_masks_not_found() {
        let args = serde_json::json!({
            "list": [{
                "id": "M1",
                "threadId": "T1",
                "from": [{ "name": "GitHub", "email": "noreply@github.com" }],
                "subject": "Verify your device",
                "header:Message-ID:asText": " <abc@github.com> ",
                "header:References:asText": "<r1@x> <r2@x>",
                "header:Authentication-Results:asText:all": [
                    "mx; spf=pass; dkim=pass; dmarc=fail"
                ],
            }]
        });
        let ctx = parse_reply_context("M1", &args)
            .expect("parses")
            .expect("found");
        assert_eq!(ctx.from[0].email, "noreply@github.com");
        assert_eq!(ctx.subject, "Verify your device");
        assert_eq!(ctx.thread_id.as_deref(), Some("T1"));
        assert_eq!(ctx.message_id.as_deref(), Some("<abc@github.com>"), "trimmed");
        assert_eq!(ctx.references.as_deref(), Some("<r1@x> <r2@x>"));
        assert_eq!(ctx.auth_results.len(), 1);

        // notFound → Ok(None); neither → loud.
        let nf = serde_json::json!({ "notFound": ["M1"], "list": [] });
        assert!(parse_reply_context("M1", &nf).expect("parses").is_none());
        assert!(parse_reply_context("M1", &serde_json::json!({ "list": [] })).is_err());
    }

    #[test]
    fn identity_matcher_is_case_insensitive_and_none_when_absent() {
        let args = serde_json::json!({
            "list": [
                { "id": "I1", "email": "other@acme.dev" },
                { "id": "I2", "email": "Bot@ACME.dev", "name": "Bot" },
            ]
        });
        assert_eq!(parse_identity_for(&args, "bot@acme.dev").as_deref(), Some("I2"));
        assert_eq!(parse_identity_for(&args, "nobody@acme.dev"), None);
        assert_eq!(parse_identity_for(&serde_json::json!({}), "x@y.z"), None);
    }

    #[test]
    fn submission_reply_requires_both_creates() {
        // Happy path: both created (the REAL live reply carried ids +
        // sendAt on the submission).
        let ok = serde_json::json!({
            "methodResponses": [
                ["Email/set", { "created": { "k2out": { "id": "eaaaaab", "threadId": "b" } } }, "0"],
                ["EmailSubmission/set", { "created": { "k2sub": {
                    "id": "b", "sendAt": "2026-07-10T03:51:02Z", "undoStatus": "final"
                } } }, "1"],
            ]
        });
        parse_submission_created(&ok).expect("both created");

        // Email/set notCreated surfaces the server's SetError.
        let rejected = serde_json::json!({
            "methodResponses": [
                ["Email/set", { "notCreated": { "k2out": {
                    "type": "invalidProperties", "description": "bad body"
                } } }, "0"],
            ]
        });
        let err = parse_submission_created(&rejected).expect_err("must reject");
        assert!(err.contains("invalidProperties"), "{err}");

        // EmailSubmission/set notCreated (e.g. forbiddenFrom) is loud.
        let sub_rejected = serde_json::json!({
            "methodResponses": [
                ["Email/set", { "created": { "k2out": { "id": "M9" } } }, "0"],
                ["EmailSubmission/set", { "notCreated": { "k2sub": {
                    "type": "forbiddenFrom"
                } } }, "1"],
            ]
        });
        let err = parse_submission_created(&sub_rejected).expect_err("must reject");
        assert!(err.contains("forbiddenFrom"), "{err}");

        // A method-level JMAP error is loud; a half-answered reply
        // (Email/set only) is loud too — a message created but never
        // submitted must not read as success.
        let jmap_err = serde_json::json!({
            "methodResponses": [["error", { "type": "unknownMethod" }, "0"]]
        });
        assert!(parse_submission_created(&jmap_err).is_err());
        let half = serde_json::json!({
            "methodResponses": [
                ["Email/set", { "created": { "k2out": { "id": "M9" } } }, "0"],
            ]
        });
        let err = parse_submission_created(&half).expect_err("must reject");
        assert!(err.contains("EmailSubmission/set"), "{err}");
        assert!(parse_submission_created(&serde_json::json!({})).is_err());
    }

    // ── The ONE loopback mock round-trip for the S4 read path ────────
    // (127.0.0.1 only, house rule — locks the wire shape end to end:
    // discovery (cached once per client) → mail envelope + accountId
    // injection → query → get → mark-seen → blob download.)

    #[test]
    fn mail_read_round_trip_against_loopback_mock() {
        let session = r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}}"#.to_string();
        let query_reply = serde_json::json!({
            "methodResponses": [["Email/query", { "ids": ["M1"] }, "0"]],
        })
        .to_string();
        let get_reply = serde_json::json!({
            "methodResponses": [["Email/get", summaries_fixture(), "0"]],
        })
        .to_string();
        let seen_reply = serde_json::json!({
            "methodResponses": [["Email/set", { "updated": { "M1": null } }, "0"]],
        })
        .to_string();
        // ONE session fetch (cached on the client), then three calls.
        let (port, rx) = spawn_mock_server(vec![
            session,
            query_reply,
            get_reply,
            seen_reply,
        ]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");

        let ids = c
            .email_query_ids("acc-7", serde_json::json!({ "inMailbox": "IB" }), 20, 0)
            .expect("query");
        assert_eq!(ids, vec!["M1".to_string()]);
        let summaries = c.email_get_summaries("acc-7", &ids).expect("get");
        assert_eq!(summaries.len(), 2);
        c.email_mark_seen("acc-7", "M1").expect("seen");

        // Request 1: discovery (the NEW /jmap/session path). Request 2:
        // the Email/query envelope — mail capability + the INJECTED
        // accountId (the delegated-read seam, live-verified).
        let disc = rx.recv().expect("req1");
        assert!(disc.starts_with("GET /jmap/session"), "{disc}");
        let q = rx.recv().expect("req2");
        assert!(q.starts_with("POST /jmap/"), "{q}");
        let v = body_json(&q);
        assert_eq!(v["using"][1], "urn:ietf:params:jmap:mail");
        assert_eq!(v["methodCalls"][0][0], "Email/query");
        assert_eq!(v["methodCalls"][0][1]["accountId"], "acc-7");
        assert_eq!(v["methodCalls"][0][1]["filter"]["inMailbox"], "IB");
        assert_eq!(
            v["methodCalls"][0][1]["sort"][0]["property"], "receivedAt");
        assert_eq!(v["methodCalls"][0][1]["sort"][0]["isAscending"], false);
        assert_eq!(v["methodCalls"][0][1]["limit"], 20);

        let g = rx.recv().expect("req3");
        let v = body_json(&g);
        assert_eq!(v["methodCalls"][0][0], "Email/get");
        assert_eq!(v["methodCalls"][0][1]["accountId"], "acc-7");
        let props = v["methodCalls"][0][1]["properties"].as_array().unwrap();
        assert!(
            !props.iter().any(|p| p == "bodyValues"),
            "summaries must never fetch bodies"
        );

        let s = rx.recv().expect("req4");
        let v = body_json(&s);
        assert_eq!(v["methodCalls"][0][0], "Email/set");
        assert_eq!(v["methodCalls"][0][1]["update"]["M1"]["keywords/$seen"], true);
    }

    #[test]
    fn blob_download_uses_discovered_template_with_auth() {
        let session = serde_json::json!({
            "apiUrl": "/jmap/",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        })
        .to_string();
        // Reply 2 is served as JSON content-type but blob_download only
        // reads bytes — fine for the wire assertion.
        let (port, rx) = spawn_mock_server(vec![session, "PDFBYTES".to_string()]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let bytes = c
            .blob_download("acc-7", "B1", "report.pdf", "application/pdf")
            .expect("blob");
        assert_eq!(bytes, b"PDFBYTES");
        let disc = rx.recv().expect("req1");
        assert!(disc.starts_with("GET /jmap/session"), "{disc}");
        let dl = rx.recv().expect("req2");
        assert!(
            dl.starts_with("GET /jmap/download/acc-7/B1/report.pdf?accept=application%2Fpdf"),
            "{dl}"
        );
        assert!(
            dl.contains("authorization: Bearer k2-test-key")
                || dl.contains("Authorization: Bearer k2-test-key"),
            "{dl}"
        );
    }
}

#[cfg(test)]
mod s6_relay_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    #[test]
    fn relay_route_names_are_domain_scoped_and_dot_free() {
        assert_eq!(relay_route_name("acme.dev"), "k2-relay-acme-dev");
        // Different domains never collide on a route name.
        assert_ne!(relay_route_name("a.dev"), relay_route_name("b.dev"));
    }

    /// The REAL default route expression (live 2026-07-10) — rewrite
    /// keeps the local rule first, appends our sender-domain match,
    /// and strips it again on clear.
    #[test]
    fn route_expression_rewrite_adds_and_removes_our_match() {
        let default_expr = serde_json::json!({
            "match": { "0": { "if": "is_local_domain(rcpt_domain)", "then": "'local'" } },
            "else": "'mx'",
        });
        let bound =
            rewrite_route_expression(&default_expr, "acme.dev", Some("k2-relay-acme-dev"))
                .expect("rewrite");
        assert_eq!(bound["match"]["0"]["if"], "is_local_domain(rcpt_domain)", "local rule first");
        assert_eq!(bound["match"]["1"]["if"], "sender_domain == 'acme.dev'");
        assert_eq!(bound["match"]["1"]["then"], "'k2-relay-acme-dev'");
        assert_eq!(bound["else"], "'mx'");

        // Re-apply is idempotent (the old match is dropped first).
        let rebound =
            rewrite_route_expression(&bound, "acme.dev", Some("k2-relay-acme-dev"))
                .expect("rewrite");
        assert_eq!(rebound, bound);

        // Clear removes only OUR match.
        let cleared = rewrite_route_expression(&bound, "acme.dev", None).expect("rewrite");
        assert_eq!(cleared, default_expr);

        // Missing else → loud.
        assert!(rewrite_route_expression(&serde_json::json!({}), "a.dev", None).is_err());
    }

    /// Apply round-trip: MtaRoute create + outbound-strategy rebind
    /// (creds ride the create payload; the wire assertion is the ONLY
    /// place the test looks for them).
    #[test]
    fn relay_route_apply_creates_route_and_binds_expression() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let routes_get = serde_json::json!({
            "methodResponses": [["x:MtaRoute/get", { "list": [], "notFound": [] }, "0"]],
        })
        .to_string();
        let route_set = serde_json::json!({
            "methodResponses": [["x:MtaRoute/set", {
                "created": { "k2": { "id": "R1" } },
            }, "0"]],
        })
        .to_string();
        let strategy_get = serde_json::json!({
            "methodResponses": [["x:MtaOutboundStrategy/get", {
                "list": [{
                    "id": "singleton",
                    "route": {
                        "match": { "0": { "if": "is_local_domain(rcpt_domain)", "then": "'local'" } },
                        "else": "'mx'",
                    },
                }],
                "notFound": [],
            }, "0"]],
        })
        .to_string();
        let strategy_set = serde_json::json!({
            "methodResponses": [["x:MtaOutboundStrategy/set", {
                "updated": { "singleton": null },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            session,
            routes_get,
            route_set,
            strategy_get,
            strategy_set,
        ]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let route = RelayRoute {
            host: "smtp.mailgun.org".into(),
            port: 587,
            username: "postmaster@acme.dev".into(),
            password: "relay-pw".into(),
            implicit_tls: false,
        };
        c.relay_route_apply("acme.dev", Some(&route)).expect("apply");

        let _sess = rx.recv().expect("req1");
        let _routes = rx.recv().expect("req2");
        let set = body_json(&rx.recv().expect("req3"));
        assert_eq!(set["methodCalls"][0][0], "x:MtaRoute/set");
        let create = &set["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["@type"], "Relay");
        assert_eq!(create["name"], "k2-relay-acme-dev");
        assert_eq!(create["address"], "smtp.mailgun.org");
        assert_eq!(create["port"], 587);
        assert_eq!(create["protocol"], "smtp");
        assert_eq!(create["implicitTls"], false);
        assert_eq!(create["authUsername"], "postmaster@acme.dev");
        assert_eq!(create["authSecret"]["@type"], "Value");
        assert_eq!(create["authSecret"]["secret"], "relay-pw");

        let _sget = rx.recv().expect("req4");
        let sset = body_json(&rx.recv().expect("req5"));
        assert_eq!(sset["methodCalls"][0][0], "x:MtaOutboundStrategy/set");
        let expr = &sset["methodCalls"][0][1]["update"]["singleton"]["route"];
        assert_eq!(expr["match"]["1"]["if"], "sender_domain == 'acme.dev'");
        assert_eq!(expr["match"]["1"]["then"], "'k2-relay-acme-dev'");
    }

    /// Clear round-trip: expression unbound FIRST, then the route
    /// destroyed (never a dangling expression reference).
    #[test]
    fn relay_route_clear_unbinds_then_destroys() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let routes_get = serde_json::json!({
            "methodResponses": [["x:MtaRoute/get", {
                "list": [{ "id": "R1", "name": "k2-relay-acme-dev" }],
                "notFound": [],
            }, "0"]],
        })
        .to_string();
        let strategy_get = serde_json::json!({
            "methodResponses": [["x:MtaOutboundStrategy/get", {
                "list": [{
                    "id": "singleton",
                    "route": {
                        "match": {
                            "0": { "if": "is_local_domain(rcpt_domain)", "then": "'local'" },
                            "1": { "if": "sender_domain == 'acme.dev'", "then": "'k2-relay-acme-dev'" },
                        },
                        "else": "'mx'",
                    },
                }],
                "notFound": [],
            }, "0"]],
        })
        .to_string();
        let strategy_set = serde_json::json!({
            "methodResponses": [["x:MtaOutboundStrategy/set", {
                "updated": { "singleton": null },
            }, "0"]],
        })
        .to_string();
        let route_destroy = serde_json::json!({
            "methodResponses": [["x:MtaRoute/set", { "destroyed": ["R1"] }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            session,
            routes_get,
            strategy_get,
            strategy_set,
            route_destroy,
        ]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.relay_route_apply("acme.dev", None).expect("clear");

        let _sess = rx.recv().expect("req1");
        let _routes = rx.recv().expect("req2");
        let _sget = rx.recv().expect("req3");
        let sset = body_json(&rx.recv().expect("req4"));
        let expr = &sset["methodCalls"][0][1]["update"]["singleton"]["route"];
        assert!(
            expr["match"]["1"].is_null(),
            "our sender-domain match removed: {expr}"
        );
        let destroy = body_json(&rx.recv().expect("req5"));
        assert_eq!(destroy["methodCalls"][0][0], "x:MtaRoute/set");
        assert_eq!(destroy["methodCalls"][0][1]["destroy"][0], "R1");
    }
}

#[cfg(test)]
mod sieve_jmap_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    #[test]
    fn sieve_envelope_is_rfc_9661_not_registry() {
        let env = sieve_envelope("SieveScript/set", serde_json::json!({"accountId": "e"}));
        assert_eq!(env["using"][0], "urn:ietf:params:jmap:core");
        assert_eq!(env["using"][1], "urn:ietf:params:jmap:sieve");
        assert_eq!(env["using"].as_array().map(|a| a.len()), Some(2));
        assert_ne!(env["using"][1], "urn:stalwart:jmap");
        assert_eq!(env["methodCalls"][0][0], "SieveScript/set");
        assert_ne!(env["methodCalls"][0][0], "x:SieveScript/set");
        assert_ne!(env["methodCalls"][0][0], "VacationResponse/set");
    }

    #[test]
    fn sieve_script_put_k2_records_blob_then_set_with_sieve_using() {
        let session =
            r#"{"apiUrl": "/jmap/", "uploadUrl": "http://127.0.0.1/upload/{accountId}", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let upload = r#"{"blobId":"blob-k2"}"#.to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["SieveScript/set", {
                "created": { "k2": { "id": "s1" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session.clone(), upload, session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let blob = c
            .blob_upload("acct-e", b"require [\"vacation\"];")
            .expect("upload");
        assert_eq!(blob, "blob-k2");
        c.sieve_script_put_k2("acct-e", None, &blob, &[])
            .expect("SieveScript/set");

        let _sess = rx.recv().expect("session");
        let up = rx.recv().expect("blob POST");
        assert!(up.contains("POST "), "{up}");
        let _sess2 = rx.recv().expect("session 2");
        let set = body_json(&rx.recv().expect("SieveScript/set"));
        assert_eq!(set["using"][1], "urn:ietf:params:jmap:sieve");
        assert_eq!(set["methodCalls"][0][0], "SieveScript/set");
        assert_ne!(set["methodCalls"][0][0], "x:SieveScript/set");
        assert_ne!(set["methodCalls"][0][0], "VacationResponse/set");
        let args = &set["methodCalls"][0][1];
        assert_eq!(args["accountId"], "acct-e");
        assert_eq!(args["create"]["k2"]["name"], "k2");
        assert_eq!(args["create"]["k2"]["blobId"], "blob-k2");
        assert_eq!(args["onSuccessActivateScript"], "#k2");
    }

    #[test]
    fn sieve_script_put_k2_update_destroys_legacy_forward() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["SieveScript/set", {
                "updated": { "s1": null },
                "destroyed": ["fwd1"],
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.sieve_script_put_k2("acct-e", Some("s1"), "blob-2", &["fwd1".to_string()])
            .expect("update");
        let _sess = rx.recv().expect("session");
        let set = body_json(&rx.recv().expect("set"));
        let args = &set["methodCalls"][0][1];
        assert_eq!(args["update"]["s1"]["blobId"], "blob-2");
        assert_eq!(args["destroy"][0], "fwd1");
        assert_eq!(args["onSuccessActivateScript"], "s1");
        assert_eq!(set["methodCalls"][0][0], "SieveScript/set");
    }

    #[test]
    fn system_script_put_is_registry_k2_footer() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let get_reply = serde_json::json!({
            "methodResponses": [["x:SieveSystemScript/get", { "list": [], "notFound": [] }, "0"]],
        })
        .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:SieveSystemScript/set", {
                "created": { "k2": { "id": "sys1" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, get_reply, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.system_script_put("k2-footer", "require [\"variables\"];")
            .expect("put");
        let _sess = rx.recv().expect("session");
        let get = body_json(&rx.recv().expect("get"));
        assert_eq!(get["methodCalls"][0][0], "x:SieveSystemScript/get");
        assert_eq!(get["using"][1], "urn:stalwart:jmap");
        let set = body_json(&rx.recv().expect("set"));
        assert_eq!(set["methodCalls"][0][0], "x:SieveSystemScript/set");
        let create = &set["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["name"], "k2-footer");
        assert_eq!(create["isActive"], true);
        assert!(
            create["contents"].as_str().unwrap().contains("require"),
            "{create}"
        );
    }
}

#[cfg(test)]
mod next_cli_jmap_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    fn session() -> String {
        r#"{"apiUrl":"/jmap/","accounts":{"b":{}},"primaryAccounts":{"urn:stalwart:jmap":"b","urn:ietf:params:jmap:mail":"b"}}"#.to_string()
    }

    fn method_ok(method: &str, args: serde_json::Value) -> String {
        serde_json::json!({ "methodResponses": [[method, args, "0"]] }).to_string()
    }

    #[test]
    fn mailing_list_create_sends_recipients_set_object_not_array() {
        let created = method_ok(
            "x:MailingList/set",
            serde_json::json!({ "created": { "k2": { "id": "ml1" } } }),
        );
        let (port, rx) = spawn_mock_server(vec![session(), created]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let recips = recipients_set_object(&["a@acme.dev".into(), "b@acme.dev".into()]);
        let id = c
            .mailing_list_create("team", "dom-1", &recips)
            .expect("create");
        assert_eq!(id, "ml1");
        let _sess = rx.recv().expect("session");
        let body = body_json(&rx.recv().expect("set"));
        assert_eq!(body["methodCalls"][0][0], "x:MailingList/set");
        let create = &body["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["name"], "team");
        assert_eq!(create["domainId"], "dom-1");
        assert!(create["recipients"].is_object(), "{}", create);
        assert_eq!(create["recipients"]["a@acme.dev"], true);
        assert!(create["recipients"].as_array().is_none());
        assert_eq!(create["aliases"], serde_json::json!({}));
        assert!(
            create.get("@type").is_none(),
            "list is not an Account: {create}"
        );
    }

    #[test]
    fn mta_stage_data_set_authenticated_as_match() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["x:MtaStageData/set", {
                "updated": { "singleton": null },
            }, "0"]],
        })
        .to_string();
        let script = rewrite_data_script_footer(
            &serde_json::json!({"else": "false"}),
            Some("k2-footer"),
        );
        assert_eq!(script["match"]["0"]["if"], "!is_empty(authenticated_as)");
        assert_eq!(script["match"]["0"]["then"], "'k2-footer'");
        assert_eq!(script["else"], "false");
        assert!(data_script_points_at_footer(&script, "k2-footer"));
        let cleared = rewrite_data_script_footer(&script, None);
        assert_eq!(cleared, serde_json::json!({"else": "false"}));
        assert!(!data_script_points_at_footer(&cleared, "k2-footer"));

        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.mta_stage_data_set_script(script).expect("set");
        let _sess = rx.recv().expect("session");
        let set = body_json(&rx.recv().expect("MtaStageData/set"));
        assert_eq!(set["methodCalls"][0][0], "x:MtaStageData/set");
        let patch = &set["methodCalls"][0][1]["update"]["singleton"]["script"];
        assert_eq!(patch["match"]["0"]["if"], "!is_empty(authenticated_as)");
        assert_eq!(patch["match"]["0"]["then"], "'k2-footer'");
    }

    #[test]
    fn sieve_scripts_destroy_deactivates() {
        let session =
            r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
                .to_string();
        let set_reply = serde_json::json!({
            "methodResponses": [["SieveScript/set", {
                "destroyed": ["s1"],
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![session, set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.sieve_scripts_destroy("acct-e", &["s1".to_string()])
            .expect("destroy");
        let _sess = rx.recv().expect("session");
        let set = body_json(&rx.recv().expect("set"));
        assert_eq!(set["using"][1], "urn:ietf:params:jmap:sieve");
        assert_eq!(set["methodCalls"][0][0], "SieveScript/set");
        let args = &set["methodCalls"][0][1];
        assert_eq!(args["destroy"][0], "s1");
        assert!(args["onSuccessActivateScript"].is_null(), "{args}");
    }

    #[test]
    fn mailing_list_destroy_is_set_destroy() {
        let destroyed = method_ok(
            "x:MailingList/set",
            serde_json::json!({ "destroyed": ["ml1"] }),
        );
        let (port, rx) = spawn_mock_server(vec![session(), destroyed]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.mailing_list_destroy("ml1").expect("destroy");
        let _sess = rx.recv().expect("session");
        let body = body_json(&rx.recv().expect("set"));
        assert_eq!(body["methodCalls"][0][1]["destroy"][0], "ml1");
    }

    #[test]
    fn queued_message_retry_patches_next_retry_drop_destroys() {
        let updated = method_ok(
            "x:QueuedMessage/set",
            serde_json::json!({ "updated": { "q1": null } }),
        );
        let destroyed = method_ok(
            "x:QueuedMessage/set",
            serde_json::json!({ "destroyed": ["q1"] }),
        );
        let (port, rx) = spawn_mock_server(vec![session(), updated, destroyed]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.queued_message_retry("q1", "2026-09-18T00:00:00Z")
            .expect("retry");
        c.queued_message_destroy("q1").expect("drop");
        let _s1 = rx.recv().expect("s1");
        let retry = body_json(&rx.recv().expect("retry"));
        assert_eq!(retry["methodCalls"][0][0], "x:QueuedMessage/set");
        assert_eq!(
            retry["methodCalls"][0][1]["update"]["q1"]["nextRetry"],
            "2026-09-18T00:00:00Z"
        );
        assert!(
            retry["methodCalls"][0][1].get("destroy").is_none(),
            "retry is not destroy"
        );
        let drop = body_json(&rx.recv().expect("drop"));
        assert_eq!(drop["methodCalls"][0][1]["destroy"][0], "q1");
    }

    #[test]
    fn memory_lookup_then_invalidate_caches() {
        let q = method_ok("x:MemoryLookupKey/query", serde_json::json!({ "ids": [] }));
        let created = method_ok(
            "x:MemoryLookupKey/set",
            serde_json::json!({ "created": { "k2": { "id": "lk1" } } }),
        );
        let action = method_ok(
            "x:Action/set",
            serde_json::json!({ "created": { "k2": { "id": "act1" } } }),
        );
        let (port, rx) = spawn_mock_server(vec![session(), q, created, action]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.memory_lookup_query_namespace("trusted-domains")
            .expect("probe");
        c.memory_lookup_key_create("trusted-domains", "spam.example")
            .expect("create");
        c.action_invalidate_caches().expect("invalidate");
        let _s = rx.recv().expect("session");
        let qbody = body_json(&rx.recv().expect("query"));
        assert_eq!(qbody["methodCalls"][0][0], "x:MemoryLookupKey/query");
        let cbody = body_json(&rx.recv().expect("create"));
        let create = &cbody["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["namespace"], "trusted-domains");
        assert_eq!(create["key"], "spam.example");
        assert_eq!(create["isGlobPattern"], false);
        let abody = body_json(&rx.recv().expect("action"));
        assert_eq!(
            abody["methodCalls"][0][1]["create"]["k2"]["@type"],
            "InvalidateCaches"
        );
    }

    #[test]
    fn unknown_spam_method_fails_loud() {
        let err = serde_json::json!({
            "methodResponses": [["error", {
                "type": "unknownMethod",
                "description": "x:Spam/train is not known",
            }, "0"]],
        })
        .to_string();
        let (port, _rx) = spawn_mock_server(vec![session(), err]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let e = c
            .memory_lookup_query_namespace("nope")
            .expect_err("unknownMethod");
        assert!(e.contains("unknownMethod"), "{e}");
    }

    #[test]
    fn recipients_set_roundtrip() {
        let v = recipients_set_object(&["A@B.test".into()]);
        assert_eq!(v["A@B.test"], true);
        assert_eq!(recipients_addrs(&serde_json::json!({"x@y.z": true})), vec!["x@y.z"]);
    }

    #[test]
    fn parse_blocked_ip_get_fixture() {
        let args = serde_json::json!({
            "list": [{
                "id": "bip1",
                "address": "65.130.10.89",
                "reason": "authFailure",
                "createdAt": "2026-09-18T12:00:00Z",
                "expiresAt": null,
            }]
        });
        let rows = parse_ip_list_get(&args);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "bip1");
        assert_eq!(rows[0].address, "65.130.10.89");
        assert_eq!(rows[0].reason.as_deref(), Some("authFailure"));
        assert!(rows[0].expires_at.is_null());
    }

    #[test]
    fn parse_security_get_object_and_null() {
        let with_rate = serde_json::json!({
            "list": [{
                "id": "singleton",
                "authBanRate": { "count": 100, "period": 86400000 },
            }]
        });
        let s = parse_security_get(&with_rate).expect("object");
        assert_eq!(
            s.auth_ban_rate,
            Some(AuthBanRate {
                count: 100,
                period: 86400000
            })
        );
        let nulled = serde_json::json!({
            "list": [{ "id": "singleton", "authBanRate": null }]
        });
        let s = parse_security_get(&nulled).expect("null");
        assert_eq!(s.auth_ban_rate, None);
    }

    #[test]
    fn reload_actions_are_typed_not_invalidate() {
        let action = method_ok(
            "x:Action/set",
            serde_json::json!({ "created": { "k2": { "id": "act1" } } }),
        );
        let (port, rx) = spawn_mock_server(vec![session(), action.clone(), action]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.action_reload_blocked_ips().expect("reload blocked");
        c.action_reload_settings().expect("reload settings");
        let _s = rx.recv().expect("session");
        let blocked = body_json(&rx.recv().expect("blocked"));
        assert_eq!(
            blocked["methodCalls"][0][1]["create"]["k2"]["@type"],
            "ReloadBlockedIps"
        );
        assert_ne!(
            blocked["methodCalls"][0][1]["create"]["k2"]["@type"],
            "InvalidateCaches"
        );
        let settings = body_json(&rx.recv().expect("settings"));
        assert_eq!(
            settings["methodCalls"][0][1]["create"]["k2"]["@type"],
            "ReloadSettings"
        );
    }

    #[test]
    fn security_set_auth_ban_rate_never_touches_period() {
        let set_reply = method_ok(
            "x:Security/set",
            serde_json::json!({ "updated": { "singleton": null } }),
        );
        let (port, rx) = spawn_mock_server(vec![session(), set_reply.clone(), set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.security_set_auth_ban_rate(None).expect("null rate");
        let rate = AuthBanRate {
            count: 100,
            period: 86400000,
        };
        c.security_set_auth_ban_rate(Some(&rate)).expect("restore");
        let _s = rx.recv().expect("session");
        let null_body = body_json(&rx.recv().expect("null set"));
        let update = &null_body["methodCalls"][0][1]["update"]["singleton"];
        assert!(update.get("authBanRate").unwrap().is_null());
        assert!(
            update.get("authBanPeriod").is_none(),
            "never write authBanPeriod: {update}"
        );
        let restore = body_json(&rx.recv().expect("restore set"));
        let update = &restore["methodCalls"][0][1]["update"]["singleton"];
        assert_eq!(update["authBanRate"]["count"], 100);
        assert_eq!(update["authBanRate"]["period"], 86400000);
        assert!(update.get("authBanPeriod").is_none());
    }
}

#[cfg(test)]
mod app_password_tests {
    use super::tests::{body_json, spawn_mock_server};
    use super::*;

    const SESSION: &str = r#"{"apiUrl": "/jmap/", "accounts": {"svc": {}}, "primaryAccounts": {"urn:stalwart:jmap": "svc"}}"#;

    #[test]
    fn parse_create_yields_server_set_secret_once() {
        let ok = serde_json::json!({
            "accountId": "mbox",
            "created": { "k2": { "id": "ap1", "secret": "app_once-shown" } },
        });
        let created = parse_app_password_created(&ok).expect("created");
        assert_eq!(created.id, "ap1");
        assert_eq!(created.secret, "app_once-shown");

        let no_secret = serde_json::json!({ "created": { "k2": { "id": "ap1" } } });
        let err = parse_app_password_created(&no_secret).expect_err("secret required");
        assert!(err.contains("secret"), "{err}");

        let cap = serde_json::json!({
            "notCreated": { "k2": { "type": "overQuota", "description": "maxAppPasswords" } },
        });
        let err = parse_app_password_created(&cap).expect_err("cap");
        assert!(err.contains("overQuota"), "{err}");
        assert!(err.contains("maxAppPasswords"), "{err}");
    }

    #[test]
    fn parse_list_never_keeps_secret() {
        let args = serde_json::json!({
            "list": [{
                "id": "ap1",
                "description": "Mail.app",
                "createdAt": "2026-09-18T00:00:00Z",
                "secret": "app_MUST_NOT_LEAK",
            }],
        });
        let rows = parse_app_password_list(&args);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "ap1");
        assert_eq!(rows[0].description, "Mail.app");
        assert_eq!(rows[0].created_at, "2026-09-18T00:00:00Z");
        let dumped = serde_json::to_string(&serde_json::json!({
            "id": rows[0].id,
            "label": rows[0].description,
            "createdAt": rows[0].created_at,
        }))
        .unwrap();
        assert!(
            !dumped.contains("app_MUST_NOT_LEAK"),
            "list JSON must not contain the secret: {dumped}"
        );
        assert!(!dumped.contains("secret"), "{dumped}");
    }

    #[test]
    fn create_recorded_jmap_copies_api_key_envelope() {
        let created = serde_json::json!({
            "methodResponses": [["x:AppPassword/set", {
                "accountId": "mbox-1",
                "created": { "k2": { "id": "ap1", "secret": "app_once-shown" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), created]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let got = c.app_password_create("mbox-1", "  ").expect("create");
        assert_eq!(got.id, "ap1");
        assert_eq!(got.secret, "app_once-shown");
        assert!(got.secret.starts_with("app_"), "0.16.3+ prefix: {}", got.secret);

        let _sess = rx.recv().expect("session");
        let req = body_json(&rx.recv().expect("set"));
        assert_eq!(req["using"][1], "urn:stalwart:jmap");
        assert_eq!(req["methodCalls"][0][0], "x:AppPassword/set");
        let args = &req["methodCalls"][0][1];
        assert_eq!(
            args["accountId"], "mbox-1",
            "must address the mailbox, not the supervisor session"
        );
        let create = &args["create"]["k2"];
        assert_eq!(create["description"], "k2", "empty label defaults to k2");
        assert_eq!(create["permissions"]["@type"], "Inherit");
        assert_eq!(create["allowedIps"], serde_json::json!({}));
        assert!(
            create.get("secret").is_none(),
            "do not invent the secret: {create}"
        );
        assert!(
            args.get("update").is_none(),
            "must not patch Account.credentials: {args}"
        );
    }

    #[test]
    fn create_on_wrong_account_is_engine_error() {
        let created = serde_json::json!({
            "methodResponses": [["x:AppPassword/set", {
                "accountId": "svc",
                "created": { "k2": { "id": "ap1", "secret": "app_wrong-account" } },
            }, "0"]],
        })
        .to_string();
        let (port, _rx) = spawn_mock_server(vec![SESSION.to_string(), created]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let err = c
            .app_password_create("mbox-1", "k2")
            .expect_err("must not hand supervisor secret to Mail.app");
        assert!(err.contains("svc"), "{err}");
        assert!(err.contains("mbox-1"), "{err}");
        assert!(
            !err.contains("app_wrong-account"),
            "must not leak the foreign secret: {err}"
        );
    }

    #[test]
    fn list_get_omits_secret_property() {
        let query = serde_json::json!({
            "methodResponses": [["x:AppPassword/query", { "ids": ["ap1"] }, "0"]],
        })
        .to_string();
        let get = serde_json::json!({
            "methodResponses": [["x:AppPassword/get", {
                "list": [{ "id": "ap1", "description": "Mail.app", "createdAt": "2026-09-18T00:00:00Z" }],
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), query, get]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let rows = c.app_password_list("mbox-1").expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "ap1");
        let _sess = rx.recv().expect("session");
        let q = body_json(&rx.recv().expect("query"));
        assert_eq!(q["methodCalls"][0][0], "x:AppPassword/query");
        assert_eq!(q["methodCalls"][0][1]["accountId"], "mbox-1");
        assert!(
            q["methodCalls"][0][1].get("filter").is_none(),
            "v1 query has no expiresAt filter: {}",
            q["methodCalls"][0][1]
        );
        let g = body_json(&rx.recv().expect("get"));
        assert_eq!(g["methodCalls"][0][0], "x:AppPassword/get");
        let props = g["methodCalls"][0][1]["properties"]
            .as_array()
            .expect("properties");
        let props: Vec<&str> = props.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(props, ["id", "description", "createdAt"]);
        assert!(!props.contains(&"secret"));
    }

    #[test]
    fn destroy_recorded_jmap() {
        let destroy = serde_json::json!({
            "methodResponses": [["x:AppPassword/set", { "destroyed": ["ap1"] }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), destroy]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.app_password_destroy("mbox-1", "ap1").expect("destroy");
        let _sess = rx.recv().expect("session");
        let req = body_json(&rx.recv().expect("set"));
        assert_eq!(req["methodCalls"][0][0], "x:AppPassword/set");
        assert_eq!(req["methodCalls"][0][1]["accountId"], "mbox-1");
        assert_eq!(req["methodCalls"][0][1]["destroy"][0], "ap1");
    }

    #[test]
    fn account_set_password_patches_credentials_0_only() {
        let set_reply = serde_json::json!({
            "methodResponses": [["x:Account/set", { "updated": { "mbox-1": null } }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), set_reply]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.account_set_password("mbox-1", "new-primary-secret")
            .expect("rotate");
        let _sess = rx.recv().expect("session");
        let req = body_json(&rx.recv().expect("set"));
        assert_eq!(req["methodCalls"][0][0], "x:Account/set");
        let update = &req["methodCalls"][0][1]["update"]["mbox-1"];
        let creds = &update["credentials"];
        assert!(creds.is_object(), "never JSON-array credentials: {creds}");
        assert_eq!(creds["0"]["@type"], "Password");
        assert_eq!(creds["0"]["secret"], "new-primary-secret");
        assert!(creds.get("1").is_none(), "only index-0: {creds}");
        assert_ne!(creds["0"]["@type"], "AppPassword");
    }

    #[test]
    fn app_password_survives_primary_rotate_recorded_jmap() {
        let created = serde_json::json!({
            "methodResponses": [["x:AppPassword/set", {
                "accountId": "mbox-1",
                "created": { "k2": { "id": "ap1", "secret": "app_lives" } },
            }, "0"]],
        })
        .to_string();
        let rotate = serde_json::json!({
            "methodResponses": [["x:Account/set", { "updated": { "mbox-1": null } }, "0"]],
        })
        .to_string();
        let query = serde_json::json!({
            "methodResponses": [["x:AppPassword/query", { "ids": ["ap1"] }, "0"]],
        })
        .to_string();
        let get = serde_json::json!({
            "methodResponses": [["x:AppPassword/get", {
                "list": [{ "id": "ap1", "description": "k2", "createdAt": "2026-09-18T00:00:00Z" }],
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            SESSION.to_string(),
            created,
            rotate,
            query,
            get,
        ]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let created = c.app_password_create("mbox-1", "k2").expect("create");
        assert_eq!(created.id, "ap1");
        c.account_set_password("mbox-1", "rotated-primary")
            .expect("rotate primary");
        let rows = c.app_password_list("mbox-1").expect("list after rotate");
        assert!(
            rows.iter().any(|r| r.id == "ap1"),
            "rotate must not wipe app passwords: {rows:?}"
        );

        let _sess = rx.recv().expect("session");
        let _create = rx.recv().expect("create");
        let rotate_req = body_json(&rx.recv().expect("rotate"));
        let creds = &rotate_req["methodCalls"][0][1]["update"]["mbox-1"]["credentials"];
        assert!(creds.is_object(), "{creds}");
        assert_eq!(creds["0"]["@type"], "Password");
        assert!(creds.get("1").is_none(), "{creds}");
        let _query = rx.recv().expect("query");
        let _get = rx.recv().expect("get");
    }
}
