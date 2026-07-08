//! Minimal typed client for Stalwart's management API (JMAP).
//!
//! BOUNDARY (PRD §4 / pre-mortem #2): this is the ONLY way K2 talks to
//! Stalwart — plain HTTP against its public management API, over
//! localhost, authenticated with basic auth (bootstrap window) or the
//! least-privilege ApiKey the supervisor mints. No Stalwart crate is
//! ever linked.
//!
//! TLS: management traffic is PLAIN HTTP ON THE LOOPBACK ONLY (the
//! supervisor's `STALWART_MGMT_URL` decision — see the doc comment
//! there). This client therefore never needs
//! `danger_accept_invalid_certs`; the default-verifying TLS stack
//! stays intact for any https URL it is ever handed.
//!
//! ENDPOINT DISCOVERY (PRD §4.1): upstream docs are inconsistent about
//! `/api` vs `/jmap`, so the API path is NEVER hardcoded — it is read
//! from the JMAP **session document** at `/.well-known/jmap`
//! ([`StalwartClient::discover_api_url`], pure parser
//! [`parse_session_api_url`], unit-tested against a fixture).
//!
//! ── ⚠ v0.16 API-SHAPE UNCERTAINTY (live-box verification list) ─────
//! Stalwart v0.16 replaced its REST API with "every configuration and
//! management action is a JMAP object" (release notes), but publishes
//! no method-by-method reference yet. Every management call below is
//! therefore ISOLATED in one small function carrying a `⚠ LIVE-BOX`
//! doc comment; the S1 acceptance run on the rpm/scratch box verifies
//! (and, where wrong, fixes) exactly these functions and their
//! constants — nothing else in the daemon knows the shapes:
//!   1. [`STALWART_ADMIN_CAPABILITY`] — the `using` URN for admin
//!      objects.
//!   2. [`StalwartClient::settings_set`] — method name (`Settings/set`
//!      vs `Registry/set`) + args shape for key/value server config.
//!   3. [`StalwartClient::principal_create`] / [`principal_query_id`] /
//!      [`principal_update_secret`] — principal management shapes.
//!   4. [`StalwartClient::api_key_create`] — ApiKey/set shape +
//!      permission NAMES for the least-privilege list.
//!   5. [`StalwartBootstrap::configure_listeners`] — listener config
//!      KEY names (carried from the pre-0.16 `server.listener.*`
//!      scheme) + how a listener is removed/disabled.

use std::time::Duration;

/// Authentication for one client: the bootstrap window uses HTTP basic
/// auth (`admin` + one-time/rotated password); steady-state uses the
/// minted ApiKey as a bearer token.
#[derive(Clone)]
pub enum Auth {
    Bearer(String),
    Basic { username: String, password: String },
}

/// Client for one Stalwart instance's management API.
///
/// `base_url` is `mail_server.api_url` (`http://127.0.0.1:8180`, the
/// loopback-only plain-HTTP mgmt listener) or the bootstrap-window
/// setup listener (`http://127.0.0.1:8080`). Secrets are passed here
/// directly (resolved from the daemon's secret store by the caller —
/// never the ref). Never logged.
pub struct StalwartClient {
    base_url: String,
    auth: Auth,
}

/// Request timeout for mgmt calls — localhost, so generous is still
/// snappy; long-poll reads (S4 `wait`) will use their own client.
const MGMT_TIMEOUT: Duration = Duration::from_secs(15);

/// JMAP core capability URN (RFC 8620) — always in `using`.
const JMAP_CORE_CAPABILITY: &str = "urn:ietf:params:jmap:core";

/// ⚠ LIVE-BOX (#1): the capability URN Stalwart v0.16 expects in
/// `using` for management/admin objects. Best documented understanding
/// pending the official method reference.
const STALWART_ADMIN_CAPABILITY: &str = "https://stalw.art/jmap/admin";

impl StalwartClient {
    /// Bearer-auth client (steady state: the minted ApiKey).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_auth(base_url, Auth::Bearer(api_key.into()))
    }

    /// Basic-auth client (bootstrap window: admin + one-time password).
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
        Self { base_url, auth }
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

    /// Discover the mgmt API endpoint from the JMAP session document at
    /// `/.well-known/jmap` (PRD §4.1 — never hardcode `/api` vs
    /// `/jmap`). Returns an ABSOLUTE URL.
    pub fn discover_api_url(&self) -> Result<String, String> {
        let session = self.get_json("/.well-known/jmap")?;
        parse_session_api_url(&self.base_url, &session)
    }

    /// Authenticated liveness ping: the session document itself (any
    /// authed principal can fetch it; a 401/refused/parse failure is a
    /// health `degraded`).
    pub fn ping(&self) -> Result<(), String> {
        self.discover_api_url().map(|_| ())
    }

    /// One JMAP method call against the DISCOVERED api endpoint:
    /// `[[method, args, "0"]]` with core + admin capabilities. Returns
    /// the method-response payload; a JMAP-level `error` response (or
    /// a `<Method>/set` reply carrying `notCreated`/`notUpdated`/
    /// `notDestroyed`) is an `Err`.
    pub fn jmap_call(
        &self,
        api_url: &str,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let body = serde_json::json!({
            "using": [JMAP_CORE_CAPABILITY, STALWART_ADMIN_CAPABILITY],
            "methodCalls": [[method, args, "0"]],
        });
        let reply = self.post_json_url(api_url, &body)?;
        parse_single_method_response(method, &reply)
    }

    // ── S1 bootstrap calls (each isolated; see the module-header
    //    live-box list) ─────────────────────────────────────────────

    /// ⚠ LIVE-BOX (#2): set server configuration key/value pairs.
    /// Modeled as `Settings/set` with an `update` map keyed by setting
    /// name; v0.16's release notes describe config-as-JMAP-objects but
    /// the method may be `Registry/set` — this function is the single
    /// place to fix.
    pub fn settings_set(
        &self,
        api_url: &str,
        settings: &[(String, serde_json::Value)],
    ) -> Result<(), String> {
        let map: serde_json::Map<String, serde_json::Value> =
            settings.iter().cloned().collect();
        self.jmap_call(api_url, "Settings/set", serde_json::json!({ "update": map }))
            .map(|_| ())
    }

    /// ⚠ LIVE-BOX (#2): remove/unset configuration keys (used to tear
    /// out default listeners: IMAP/POP3/ManageSieve/CalDAV + the :8080
    /// setup listener).
    pub fn settings_destroy(&self, api_url: &str, keys: &[String]) -> Result<(), String> {
        self.jmap_call(api_url, "Settings/set", serde_json::json!({ "destroy": keys }))
            .map(|_| ())
    }

    /// ⚠ LIVE-BOX (#3): create a principal (the least-privilege
    /// `k2-daemon` service account). Returns the created principal id.
    pub fn principal_create(
        &self,
        api_url: &str,
        name: &str,
        description: &str,
    ) -> Result<String, String> {
        let created = self.jmap_call(
            api_url,
            "Principal/set",
            serde_json::json!({
                "create": {
                    "k2": {
                        "type": "individual",
                        "name": name,
                        "description": description,
                        // No mail delivery, no quota use — an
                        // automation identity only.
                        "quota": 0
                    }
                }
            }),
        )?;
        created["created"]["k2"]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| {
                format!("Principal/set create '{name}': no created id in reply")
            })
    }

    /// ⚠ LIVE-BOX (#3): find a principal's id by name (used to address
    /// the `admin` principal for the password rotation).
    pub fn principal_query_id(&self, api_url: &str, name: &str) -> Result<String, String> {
        let reply = self.jmap_call(
            api_url,
            "Principal/query",
            serde_json::json!({ "filter": { "name": name } }),
        )?;
        reply["ids"][0]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("Principal/query '{name}': no id returned"))
    }

    /// ⚠ LIVE-BOX (#3): set a principal's secret (rotate the bootstrap
    /// admin password to the generated, vaulted value).
    pub fn principal_update_secret(
        &self,
        api_url: &str,
        principal_id: &str,
        new_secret: &str,
    ) -> Result<(), String> {
        self.jmap_call(
            api_url,
            "Principal/set",
            serde_json::json!({ "update": { principal_id: { "secret": new_secret } } }),
        )
        .map(|_| ())
    }

    /// ⚠ LIVE-BOX (#4): mint the scoped ApiKey for the service
    /// account: Replace-mode permission list (domain/account/DKIM/queue
    /// management only), allowedIps pinned to the loopback (pre-mortem
    /// #13). Returns the SECRET (Stalwart shows it exactly once).
    /// The permission NAMES are the flagged uncertainty.
    pub fn api_key_create(&self, api_url: &str, account_id: &str) -> Result<String, String> {
        let created = self.jmap_call(
            api_url,
            "ApiKey/set",
            serde_json::json!({
                "create": {
                    "k": {
                        "accountId": account_id,
                        "description": "K2 daemon mail supervisor (least-privilege)",
                        "permissions": {
                            "mode": "Replace",
                            "list": [
                                "domain-get", "domain-set",
                                "principal-get", "principal-set",
                                "dkim-get", "dkim-set",
                                "queue-get", "queue-set",
                                "settings-get", "settings-set"
                            ]
                        },
                        "allowedIps": ["127.0.0.1"]
                    }
                }
            }),
        )?;
        created["created"]["k"]["secret"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "ApiKey/set create: no secret in reply".to_string())
    }

    // ── Typed calls for later slices (S3/S5 stubs follow the REAL
    //    S1 admin + S2 domain calls) ──────────────────────────────

    /// Compose + POST one JMAP method call against the DISCOVERED api
    /// url and return that method's response arguments. The whole
    /// Stalwart conversation for domains flows through here so the
    /// envelope/parse rules live in exactly one place
    /// ([`jmap_envelope`] + [`parse_method_response`], both pure and
    /// fixture-tested).
    fn mgmt_call(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let api_url = self.discover_api_url()?;
        let resp = self.post_json_url(&api_url, &jmap_envelope(method, args))?;
        parse_method_response(method, &resp)
    }

    /// S2 — `Domain/set create` with automatic DKIM key generation
    /// (Ed25519 + RSA immediately), sub-addressing enabled, catch-all
    /// OFF (spam magnet — per-domain opt-in later; PRD §6.1). Returns
    /// the server-set domain id + `dnsZoneFile` when the create reply
    /// carries it (server-set properties ride `created`; a missing
    /// zone file falls back to [`Self::domain_dns_zonefile`]).
    pub fn domain_create(&self, domain: &str) -> Result<CreatedDomain, String> {
        let args = serde_json::json!({
            "create": {
                CREATE_TAG: {
                    "name": domain,
                    "dkimManagement": "automatic",
                    "subAddressing": "enabled",
                    "catchAllAddress": null,
                }
            }
        });
        let resp = self.mgmt_call("Domain/set", args)?;
        parse_domain_set_created(&resp)
    }

    /// S2 — destroy a domain (the route layer has already required the
    /// explicit confirm + retired the domain's addresses, PRD §6.6).
    pub fn domain_delete(&self, stalwart_domain_id: &str) -> Result<(), String> {
        let args = serde_json::json!({ "destroy": [stalwart_domain_id] });
        let resp = self.mgmt_call("Domain/set", args)?;
        parse_domain_set_destroyed(stalwart_domain_id, &resp)
    }

    /// S2 — read the domain's server-set `dnsZoneFile` (the SSOT for
    /// the record table, PRD §6.2 — K2 computes nothing itself except
    /// relay-mode SPF adjustments).
    pub fn domain_dns_zonefile(&self, stalwart_domain_id: &str) -> Result<String, String> {
        let args = serde_json::json!({
            "ids": [stalwart_domain_id],
            "properties": ["dnsZoneFile"],
        });
        let resp = self.mgmt_call("Domain/get", args)?;
        parse_domain_get_zonefile(stalwart_domain_id, &resp)
    }

    /// S3 — `Account/set create` (PRD §7.1): one Stalwart account per
    /// minted address — local-part `name` bound to `domainId`, the
    /// random vaulted password, and the §12 quotas. Returns the
    /// server-set account id. The password never appears in errors or
    /// logs (only the parse errors below surface, and they carry the
    /// server's SetError — never our args).
    ///
    /// ⚠ LIVE-BOX (S3 #1): the account-creation shape. Uncertainties
    /// the first live run must confirm (this function is the single
    /// place to fix): the METHOD name (`Account/set` per the PRD §7.1
    /// wording — Stalwart may spell it `Principal/set` like S1's
    /// service account); the `type` value (`individual` carried from
    /// S1's principal shape; the PRD says "User"); the domain-binding
    /// key (`domainId`); the password key (`secret`, matching S1's
    /// `principal_update_secret`); and BOTH §12 quota keys (`quota`
    /// bytes + `maxMessages` count — Stalwart may express the message
    /// cap elsewhere, e.g. only in server config).
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
                    "type": "individual",
                    "name": local_part,
                    "domainId": stalwart_domain_id,
                    "secret": password,
                    "quota": quota_bytes,
                    "maxMessages": max_messages,
                }
            }
        });
        let resp = self.mgmt_call("Account/set", args)?;
        parse_account_set_created(&resp)
    }

    /// S3 — disable an account (address retire, PRD §7.2): the alias
    /// stops receiving, mailbox DATA IS KEPT for the retention window
    /// (§12) — never a destroy on the retire path.
    ///
    /// ⚠ LIVE-BOX (S3 #2): the disable property. Modeled as an
    /// `Account/set update` flipping `active: false`; Stalwart may
    /// spell it differently (e.g. an `enabled` flag or a type change)
    /// — this function is the single place to fix.
    pub fn account_disable(&self, stalwart_account_id: &str) -> Result<(), String> {
        let args = serde_json::json!({
            "update": { stalwart_account_id: { "active": false } }
        });
        let resp = self.mgmt_call("Account/set", args)?;
        parse_account_set_updated(stalwart_account_id, &resp)
    }

    /// S3 — destroy an account. COMPENSATING ACTION ONLY (mint
    /// rollback: Stalwart create succeeded but the K2 row write failed
    /// — no orphans). The retire path uses [`Self::account_disable`];
    /// nothing else may call this in V1 (the 90-day purge job that
    /// eventually destroys retired accounts is a later slice — see the
    /// retention seam in `mail::addresses`).
    ///
    /// ⚠ LIVE-BOX (S3 #1): same `Account/set` method-name uncertainty
    /// as [`Self::account_create`].
    pub fn account_destroy(&self, stalwart_account_id: &str) -> Result<(), String> {
        let args = serde_json::json!({ "destroy": [stalwart_account_id] });
        let resp = self.mgmt_call("Account/set", args)?;
        parse_account_set_destroyed(stalwart_account_id, &resp)
    }

    /// S5 — hand an APPROVED outbound message to Stalwart's queue.
    /// The audit row in `mail_outbound` exists BEFORE this is called
    /// (pre-mortem #11: no row, no send); Stalwart's queue owns
    /// retries (pre-mortem #9 — no daemon-side retry logic, ever).
    #[allow(dead_code)] // S5 wires this into the approved-send path.
    pub fn queue_submit(&self, outbound_id: &str) -> Result<(), String> {
        let _ = outbound_id;
        Err(super::not_built_err("S5", "jmap queue submit"))
    }
}

/// Pure session-document parser: extract `apiUrl` and absolutize it
/// against `base_url` when relative. Kept free of I/O so it unit-tests
/// against fixture JSON.
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
             /.well-known/jmap endpoint?"
                .to_string()
        })?;
    if api_url.starts_with("http://") || api_url.starts_with("https://") {
        return Ok(api_url.to_string());
    }
    if !api_url.starts_with('/') {
        return Err(format!(
            "JMAP session 'apiUrl' is neither absolute nor root-relative: '{api_url}'"
        ));
    }
    Ok(format!("{}{}", base_url.trim_end_matches('/'), api_url))
}

/// Pure parser for a single-call JMAP reply: `methodResponses[0]` must
/// echo `method` (an `"error"` name — or a `/set` reply with
/// `notCreated`/`notUpdated`/`notDestroyed` entries — is an `Err` with
/// the server's type/description).
pub fn parse_single_method_response(
    method: &str,
    reply: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let first = reply["methodResponses"][0]
        .as_array()
        .ok_or_else(|| format!("{method}: reply has no methodResponses"))?;
    let name = first.first().and_then(|v| v.as_str()).unwrap_or_default();
    let payload = first.get(1).cloned().unwrap_or(serde_json::Value::Null);
    if name == "error" {
        let t = payload["type"].as_str().unwrap_or("unknown");
        let desc = payload["description"].as_str().unwrap_or_default();
        return Err(format!("{method}: JMAP error '{t}' {desc}").trim().to_string());
    }
    if name != method {
        return Err(format!("{method}: unexpected response method '{name}'"));
    }
    for reject in ["notCreated", "notUpdated", "notDestroyed"] {
        if let Some(map) = payload.get(reject).and_then(|v| v.as_object()) {
            if !map.is_empty() {
                let detail = serde_json::to_string(map).unwrap_or_default();
                let excerpt: String = detail.chars().take(200).collect();
                return Err(format!("{method}: {reject}: {excerpt}"));
            }
        }
    }
    Ok(payload)
}

// ── The supervisor's BootstrapApi implementation ────────────────────────

/// Real [`crate::mail::supervisor::BootstrapApi`]: a basic-auth
/// [`StalwartClient`] against the bootstrap-window listener, with the
/// JMAP endpoint discovered once at `authenticate`.
#[derive(Default)]
pub struct StalwartBootstrap {
    session: Option<(StalwartClient, String)>, // (client, discovered api url)
}

impl StalwartBootstrap {
    pub fn new() -> Self {
        Self::default()
    }

    fn session(&self) -> Result<&(StalwartClient, String), String> {
        self.session
            .as_ref()
            .ok_or_else(|| "bootstrap API used before authenticate()".to_string())
    }

    fn settings(&self, settings: Vec<(String, serde_json::Value)>) -> Result<(), String> {
        let (client, api_url) = self.session()?;
        client.settings_set(api_url, &settings)
    }
}

impl crate::mail::supervisor::BootstrapApi for StalwartBootstrap {
    fn authenticate(
        &mut self,
        base_url: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        let client = StalwartClient::new_basic(base_url, username, password);
        let api_url = client.discover_api_url()?;
        self.session = Some((client, api_url));
        Ok(())
    }

    fn set_hostname(&mut self, hostname: &str) -> Result<(), String> {
        self.settings(vec![(
            "server.hostname".to_string(),
            serde_json::json!(hostname),
        )])
    }

    /// ⚠ LIVE-BOX (#5): listener configuration. Keys carried from the
    /// documented pre-0.16 `server.listener.<id>.*` scheme; the §5.3
    /// port plan decides the HTTPS bind; the `k2-mgmt` listener is the
    /// PLAIN-HTTP LOOPBACK management endpoint (supervisor TLS
    /// decision); the destroy list tears out every default listener we
    /// must not serve (§10: IMAP/POP3/ManageSieve — CalDAV/CardDAV ride
    /// the http listener and are disabled as features below).
    fn configure_listeners(&mut self, port_plan: &str) -> Result<(), String> {
        let https_bind = if port_plan == "tls-alpn" {
            "[::]:443"
        } else {
            "127.0.0.1:8443"
        };
        let (client, api_url) = self.session()?;
        client.settings_destroy(
            api_url,
            &[
                "server.listener.imap".to_string(),
                "server.listener.imaptls".to_string(),
                "server.listener.pop3".to_string(),
                "server.listener.pop3s".to_string(),
                "server.listener.sieve".to_string(),
            ],
        )?;
        let set: Vec<(String, serde_json::Value)> = vec![
            ("server.listener.smtp.bind".into(), serde_json::json!("[::]:25")),
            ("server.listener.smtp.protocol".into(), serde_json::json!("smtp")),
            ("server.listener.submissions.bind".into(), serde_json::json!("[::]:465")),
            ("server.listener.submissions.protocol".into(), serde_json::json!("smtp")),
            ("server.listener.submissions.tls.implicit".into(), serde_json::json!(true)),
            ("server.listener.submission.bind".into(), serde_json::json!("[::]:587")),
            ("server.listener.submission.protocol".into(), serde_json::json!("smtp")),
            ("server.listener.https.bind".into(), serde_json::json!(https_bind)),
            ("server.listener.https.protocol".into(), serde_json::json!("http")),
            ("server.listener.https.tls.implicit".into(), serde_json::json!(true)),
            // The daemon's management path: loopback-only, plain HTTP.
            ("server.listener.k2-mgmt.bind".into(), serde_json::json!("127.0.0.1:8180")),
            ("server.listener.k2-mgmt.protocol".into(), serde_json::json!("http")),
            // CalDAV/CardDAV/webmail surfaces stay off in V1 (§10).
            ("calendar.enable".into(), serde_json::json!(false)),
            ("contacts.enable".into(), serde_json::json!(false)),
        ];
        client.settings_set(api_url, &set)
    }

    /// ⚠ LIVE-BOX (#2): spam-filter enable key.
    fn enable_spam_filter(&mut self) -> Result<(), String> {
        self.settings(vec![(
            "spam-filter.enable".to_string(),
            serde_json::json!(true),
        )])
    }

    fn create_service_account(&mut self) -> Result<String, String> {
        let (client, api_url) = self.session()?;
        client.principal_create(api_url, "k2-daemon", "K2 daemon mail supervisor")
    }

    fn mint_api_key(&mut self, account_id: &str) -> Result<String, String> {
        let (client, api_url) = self.session()?;
        client.api_key_create(api_url, account_id)
    }

    fn rotate_admin_password(&mut self, new_password: &str) -> Result<(), String> {
        let (client, api_url) = self.session()?;
        let admin_id = client.principal_query_id(api_url, "admin")?;
        client.principal_update_secret(api_url, &admin_id, new_password)
    }

    /// ⚠ LIVE-BOX (#5): tear out the :8080 setup listener (pre-mortem
    /// #13) — the LAST bootstrap API act.
    fn disable_setup_listener(&mut self) -> Result<(), String> {
        let (client, api_url) = self.session()?;
        client.settings_destroy(api_url, &["server.listener.setup".to_string()])
    }
}

/// A Stalwart domain the S2 create call just made: the server-set id
/// plus the server-set `dnsZoneFile` when the create reply carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedDomain {
    pub id: String,
    pub dns_zone_file: Option<String>,
}

/// The client-chosen creation tag inside `Domain/set create` — any
/// string works (JMAP creation ids are caller-chosen); ours is stable
/// so fixtures and parsers agree.
const CREATE_TAG: &str = "k2";

/// The JMAP `using` capabilities for management calls (S2 domains +
/// S3 accounts share one envelope; extra capabilities in `using` are
/// harmless per RFC 8620).
/// LIVE-BOX FLAG (see module docs): BOTH Stalwart-specific URNs must
/// be confirmed against the pinned v0.16.x on the first live run.
const JMAP_USING: [&str; 3] = [
    "urn:ietf:params:jmap:core",
    "https://stalw.art/jmap/domain",
    "https://stalw.art/jmap/principal",
];

/// Pure envelope builder for a single JMAP method call.
fn jmap_envelope(method: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "using": JMAP_USING,
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

/// Pure `Domain/set create` reply parser: our creation tag must appear
/// under `created` (server-set id required; `dnsZoneFile` optional) —
/// `notCreated` surfaces the server's SetError verbatim.
fn parse_domain_set_created(args: &serde_json::Value) -> Result<CreatedDomain, String> {
    if let Some(created) = args.get("created").and_then(|v| v.get(CREATE_TAG)) {
        let id = created
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Domain/set create: reply has no server-set 'id'".to_string())?;
        let dns_zone_file = created
            .get("dnsZoneFile")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(String::from);
        return Ok(CreatedDomain { id: id.to_string(), dns_zone_file });
    }
    if let Some(err) = args.get("notCreated").and_then(|v| v.get(CREATE_TAG)) {
        return Err(format!("Domain/set create rejected — {}", set_error_line(err)));
    }
    Err("Domain/set create: reply has neither created nor notCreated for our tag".to_string())
}

/// Pure `Domain/set destroy` reply parser: the id must appear under
/// `destroyed`; `notDestroyed` surfaces the server's SetError.
fn parse_domain_set_destroyed(
    id: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    let destroyed = args
        .get("destroyed")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|v| v.as_str() == Some(id)))
        .unwrap_or(false);
    if destroyed {
        return Ok(());
    }
    if let Some(err) = args.get("notDestroyed").and_then(|v| v.get(id)) {
        return Err(format!("Domain/set destroy rejected — {}", set_error_line(err)));
    }
    Err(format!("Domain/set destroy: '{id}' not in the destroyed list"))
}

/// Pure `Account/set create` reply parser (S3): our creation tag must
/// appear under `created` with a server-set id — `notCreated`
/// surfaces the server's SetError verbatim.
fn parse_account_set_created(args: &serde_json::Value) -> Result<String, String> {
    if let Some(created) = args.get("created").and_then(|v| v.get(CREATE_TAG)) {
        return created
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .ok_or_else(|| "Account/set create: reply has no server-set 'id'".to_string());
    }
    if let Some(err) = args.get("notCreated").and_then(|v| v.get(CREATE_TAG)) {
        return Err(format!("Account/set create rejected — {}", set_error_line(err)));
    }
    Err("Account/set create: reply has neither created nor notCreated for our tag".to_string())
}

/// Pure `Account/set update` reply parser (S3 disable): the id must
/// appear as a key of the `updated` map (RFC 8620: id → server-changed
/// props or null); `notUpdated` surfaces the server's SetError.
fn parse_account_set_updated(id: &str, args: &serde_json::Value) -> Result<(), String> {
    let updated = args
        .get("updated")
        .and_then(|v| v.as_object())
        .map(|m| m.contains_key(id))
        .unwrap_or(false);
    if updated {
        return Ok(());
    }
    if let Some(err) = args.get("notUpdated").and_then(|v| v.get(id)) {
        return Err(format!("Account/set update rejected — {}", set_error_line(err)));
    }
    Err(format!("Account/set update: '{id}' not in the updated map"))
}

/// Pure `Account/set destroy` reply parser (S3 compensation): the id
/// must appear under `destroyed`; `notDestroyed` surfaces the server's
/// SetError.
fn parse_account_set_destroyed(id: &str, args: &serde_json::Value) -> Result<(), String> {
    let destroyed = args
        .get("destroyed")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|v| v.as_str() == Some(id)))
        .unwrap_or(false);
    if destroyed {
        return Ok(());
    }
    if let Some(err) = args.get("notDestroyed").and_then(|v| v.get(id)) {
        return Err(format!("Account/set destroy rejected — {}", set_error_line(err)));
    }
    Err(format!("Account/set destroy: '{id}' not in the destroyed list"))
}

/// Pure `Domain/get` reply parser: find our id in `list` and return
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
        return Err(format!("Domain/get: domain '{id}' not in the reply list"));
    };
    entry
        .get("dnsZoneFile")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| format!("Domain/get: domain '{id}' has no dnsZoneFile"))
}

/// Pure session-document parser: extract `apiUrl` and absolutize it
/// against `base_url` when relative. Kept free of I/O so it unit-tests
/// against fixture JSON (never a live fetch — house rule: no network
/// in tests).
#[allow(dead_code)] // first caller: S1 supervisor bootstrap (unit-tested today).
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A trimmed real-shaped Stalwart JMAP session document (fixture —
    /// no network). Stalwart serves absolute URLs here.
    const SESSION_FIXTURE: &str = r#"{
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 50000000,
                "maxConcurrentRequests": 4
            },
            "urn:ietf:params:jmap:mail": {}
        },
        "accounts": {
            "a": { "name": "k2-daemon", "isPersonal": false, "isReadOnly": false }
        },
        "primaryAccounts": { "urn:ietf:params:jmap:mail": "a" },
        "username": "k2-daemon",
        "apiUrl": "https://127.0.0.1:8443/jmap/",
        "downloadUrl": "https://127.0.0.1:8443/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://127.0.0.1:8443/jmap/upload/{accountId}/",
        "eventSourceUrl": "https://127.0.0.1:8443/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}",
        "state": "cyrus-0;p-5;vfs-0"
    }"#;

    #[test]
    fn session_fixture_parses_to_absolute_api_url() {
        let session: serde_json::Value =
            serde_json::from_str(SESSION_FIXTURE).expect("fixture is valid JSON");
        let url = parse_session_api_url("https://127.0.0.1:8443", &session)
            .expect("apiUrl extracted");
        assert_eq!(url, "https://127.0.0.1:8443/jmap/");
    }

    #[test]
    fn relative_api_url_is_absolutized_against_base() {
        let session = serde_json::json!({ "apiUrl": "/api/jmap/" });
        assert_eq!(
            parse_session_api_url("https://127.0.0.1:8443/", &session).expect("joined"),
            "https://127.0.0.1:8443/api/jmap/"
        );
    }

    #[test]
    fn missing_or_garbage_api_url_fails_loudly() {
        for session in [
            serde_json::json!({}),
            serde_json::json!({ "apiUrl": "" }),
            serde_json::json!({ "apiUrl": 42 }),
            serde_json::json!({ "capabilities": {} }),
        ] {
            let err = parse_session_api_url("https://127.0.0.1:8443", &session)
                .expect_err("must reject");
            assert!(err.contains("apiUrl"), "{err}");
        }
        // Neither absolute nor root-relative → loud, named value.
        let err = parse_session_api_url(
            "https://127.0.0.1:8443",
            &serde_json::json!({ "apiUrl": "jmap/" }),
        )
        .expect_err("must reject");
        assert!(err.contains("jmap/"), "{err}");
    }

    /// Constructor normalizes a trailing-slash base so path joins never
    /// double the slash; the remaining typed stub (S5) fails with the
    /// structured error. (The S2 domain + S3 account calls are REAL —
    /// their behavior is owned by the fixture/loopback tests, never a
    /// live dial from here.)
    #[test]
    fn client_construction_and_stub_errors() {
        let c = StalwartClient::new("https://127.0.0.1:8443///", "k2-test-key");
        assert_eq!(c.base_url, "https://127.0.0.1:8443");
        assert!(c
            .queue_submit("o1")
            .unwrap_err()
            .contains("not built yet — mail slice S5"));
    }

    #[test]
    fn method_response_parser_handles_ok_error_and_rejections() {
        // Success payload comes back verbatim.
        let ok = serde_json::json!({
            "methodResponses": [["Settings/set", { "updated": { "server.hostname": null } }, "0"]]
        });
        let payload = parse_single_method_response("Settings/set", &ok).expect("ok");
        assert!(payload["updated"].is_object());

        // JMAP-level error → Err with type + description.
        let err = serde_json::json!({
            "methodResponses": [["error", { "type": "forbidden", "description": "nope" }, "0"]]
        });
        let e = parse_single_method_response("Settings/set", &err).expect_err("error reply");
        assert!(e.contains("forbidden") && e.contains("nope"), "{e}");

        // A /set reply that quietly rejected the create → Err.
        let not_created = serde_json::json!({
            "methodResponses": [["ApiKey/set", {
                "created": null,
                "notCreated": { "k": { "type": "forbidden" } }
            }, "0"]]
        });
        let e = parse_single_method_response("ApiKey/set", &not_created)
            .expect_err("notCreated must be an error");
        assert!(e.contains("notCreated"), "{e}");

        // Mismatched method name → Err.
        let wrong = serde_json::json!({ "methodResponses": [["Core/echo", {}, "0"]] });
        let e = parse_single_method_response("Settings/set", &wrong).expect_err("wrong method");
        assert!(e.contains("Core/echo"), "{e}");

        // No methodResponses at all → Err.
        let e = parse_single_method_response("Settings/set", &serde_json::json!({}))
            .expect_err("empty reply");
        assert!(e.contains("methodResponses"), "{e}");
    }

    // ── Loopback mock-server tests (ephemeral port; the one allowed
    //    form of network in tests) ───────────────────────────────────

    /// Serve exactly `responses.len()` HTTP exchanges on an ephemeral
    /// loopback port; captured requests come back through the handle.
    fn serve(
        responses: Vec<&'static str>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let addr = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let mut captured = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buf = vec![0u8; 65536];
                let mut req = String::new();
                loop {
                    let n = stream.read(&mut buf).expect("read");
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    // Complete when headers ended and any content-length
                    // body has arrived.
                    if let Some(head_end) = req.find("\r\n\r\n") {
                        let clen = req
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.split(':').nth(1))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if req.len() >= head_end + 4 + clen {
                            break;
                        }
                    }
                }
                captured.push(req);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).expect("write");
            }
            captured
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn settings_set_posts_a_jmap_envelope_with_basic_auth() {
        let (base, server) = serve(vec![
            r#"{"methodResponses":[["Settings/set",{"updated":{}},"0"]]}"#,
        ]);
        let client = StalwartClient::new_basic(&base, "admin", "one-time-pw");
        let api_url = format!("{base}/jmap");
        client
            .settings_set(&api_url, &[("server.hostname".into(), serde_json::json!("mail.acme.dev"))])
            .expect("settings_set ok");
        let reqs = server.join().expect("server thread");
        let req = &reqs[0];
        assert!(req.starts_with("POST /jmap HTTP/1.1"), "{req}");
        // Basic base64("admin:one-time-pw").
        assert!(req.contains("authorization: Basic YWRtaW46b25lLXRpbWUtcHc=")
            || req.contains("Authorization: Basic YWRtaW46b25lLXRpbWUtcHc="), "{req}");
        let body = &req[req.find("\r\n\r\n").expect("body") + 4..];
        let v: serde_json::Value = serde_json::from_str(body).expect("json body");
        assert_eq!(v["using"][0], "urn:ietf:params:jmap:core");
        assert_eq!(v["methodCalls"][0][0], "Settings/set");
        assert_eq!(
            v["methodCalls"][0][1]["update"]["server.hostname"],
            "mail.acme.dev"
        );
    }

    #[test]
    fn api_key_create_extracts_the_once_shown_secret_and_pins_loopback() {
        let (base, server) = serve(vec![
            r#"{"methodResponses":[["ApiKey/set",{"created":{"k":{"id":"key1","secret":"s3cr3t-once"}}},"0"]]}"#,
        ]);
        let client = StalwartClient::new_basic(&base, "admin", "pw");
        let api_url = format!("{base}/jmap");
        let secret = client.api_key_create(&api_url, "principal-k2").expect("created");
        assert_eq!(secret, "s3cr3t-once");
        let reqs = server.join().expect("server thread");
        let body = &reqs[0][reqs[0].find("\r\n\r\n").expect("body") + 4..];
        let v: serde_json::Value = serde_json::from_str(body).expect("json");
        let create = &v["methodCalls"][0][1]["create"]["k"];
        assert_eq!(create["accountId"], "principal-k2");
        assert_eq!(create["allowedIps"][0], "127.0.0.1", "pre-mortem #13: loopback-pinned");
        assert_eq!(create["permissions"]["mode"], "Replace", "least-privilege, never Inherit");
    }

    #[test]
    fn bootstrap_discovers_api_url_then_routes_calls_through_it() {
        // Exchange 1: /.well-known/jmap discovery (relative apiUrl).
        // Exchange 2: the Settings/set ride the discovered path.
        let (base, server) = serve(vec![
            r#"{"apiUrl":"/api/jmap/"}"#,
            r#"{"methodResponses":[["Settings/set",{"updated":{}},"0"]]}"#,
        ]);
        use crate::mail::supervisor::BootstrapApi;
        let mut api = StalwartBootstrap::new();
        api.authenticate(&base, "admin", "pw").expect("authenticate");
        api.set_hostname("mail.acme.dev").expect("set_hostname");
        let reqs = server.join().expect("server thread");
        assert!(reqs[0].starts_with("GET /.well-known/jmap HTTP/1.1"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("POST /api/jmap/ HTTP/1.1"), "discovered path used: {}", reqs[1]);
        // Using before authenticate fails loudly.
        let mut cold = StalwartBootstrap::new();
        let err = cold.set_hostname("x").expect_err("must fail");
        assert!(err.contains("before authenticate"), "{err}");
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
        let err = client.get_json("/.well-known/jmap").expect_err("401 is an error");
        server.join().expect("server thread");
        assert!(err.contains("401"), "{err}");
        assert!(!err.contains("bad-key"), "credential must never appear in errors: {err}");
    }
}

#[cfg(test)]
mod s2_domain_tests {
    use super::*;

    /// A trimmed real-shaped Stalwart JMAP session document (fixture —
    /// no network). Stalwart serves absolute URLs here.
    const SESSION_FIXTURE: &str = r#"{
        "capabilities": {
            "urn:ietf:params:jmap:core": {
                "maxSizeUpload": 50000000,
                "maxConcurrentRequests": 4
            },
            "urn:ietf:params:jmap:mail": {}
        },
        "accounts": {
            "a": { "name": "k2-daemon", "isPersonal": false, "isReadOnly": false }
        },
        "primaryAccounts": { "urn:ietf:params:jmap:mail": "a" },
        "username": "k2-daemon",
        "apiUrl": "https://127.0.0.1:8443/jmap/",
        "downloadUrl": "https://127.0.0.1:8443/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "https://127.0.0.1:8443/jmap/upload/{accountId}/",
        "eventSourceUrl": "https://127.0.0.1:8443/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}",
        "state": "cyrus-0;p-5;vfs-0"
    }"#;

    #[test]
    fn session_fixture_parses_to_absolute_api_url() {
        let session: serde_json::Value =
            serde_json::from_str(SESSION_FIXTURE).expect("fixture is valid JSON");
        let url = parse_session_api_url("https://127.0.0.1:8443", &session)
            .expect("apiUrl extracted");
        assert_eq!(url, "https://127.0.0.1:8443/jmap/");
    }

    #[test]
    fn relative_api_url_is_absolutized_against_base() {
        let session = serde_json::json!({ "apiUrl": "/api/jmap/" });
        assert_eq!(
            parse_session_api_url("https://127.0.0.1:8443/", &session).expect("joined"),
            "https://127.0.0.1:8443/api/jmap/"
        );
    }

    #[test]
    fn missing_or_garbage_api_url_fails_loudly() {
        for session in [
            serde_json::json!({}),
            serde_json::json!({ "apiUrl": "" }),
            serde_json::json!({ "apiUrl": 42 }),
            serde_json::json!({ "capabilities": {} }),
        ] {
            let err = parse_session_api_url("https://127.0.0.1:8443", &session)
                .expect_err("must reject");
            assert!(err.contains("apiUrl"), "{err}");
        }
        // Neither absolute nor root-relative → loud, named value.
        let err = parse_session_api_url(
            "https://127.0.0.1:8443",
            &serde_json::json!({ "apiUrl": "jmap/" }),
        )
        .expect_err("must reject");
        assert!(err.contains("jmap/"), "{err}");
    }

    /// Constructor normalizes a trailing-slash base so path joins never
    /// double the slash; the remaining typed stub (S5) fails with the
    /// structured error. (S3's account calls are REAL as of the
    /// addresses slice — see `s3_account_tests`.)
    #[test]
    fn client_construction_and_stub_errors() {
        let c = StalwartClient::new("https://127.0.0.1:8443///", "k2-test-key");
        assert_eq!(c.base_url, "https://127.0.0.1:8443");
        assert!(c
            .queue_submit("o1")
            .unwrap_err()
            .contains("not built yet — mail slice S5"));
    }

    // ── S2 — pure envelope + reply parsers (fixtures, no network) ───

    #[test]
    fn envelope_carries_using_and_single_method_call() {
        let env = jmap_envelope("Domain/set", serde_json::json!({"destroy": ["d1"]}));
        assert_eq!(env["using"][0], "urn:ietf:params:jmap:core");
        assert_eq!(env["methodCalls"][0][0], "Domain/set");
        assert_eq!(env["methodCalls"][0][1]["destroy"][0], "d1");
        assert_eq!(env["methodCalls"][0][2], "0");
    }

    #[test]
    fn method_response_unwraps_matching_method_and_surfaces_errors() {
        let ok = serde_json::json!({
            "methodResponses": [["Domain/set", { "created": {} }, "0"]],
            "sessionState": "s1",
        });
        let args = parse_method_response("Domain/set", &ok).expect("unwrapped");
        assert!(args.get("created").is_some());

        // JMAP-level error → named type + description.
        let err = serde_json::json!({
            "methodResponses": [["error", {
                "type": "unknownMethod",
                "description": "Domain/set is not known",
            }, "0"]],
        });
        let msg = parse_method_response("Domain/set", &err).expect_err("must reject");
        assert!(msg.contains("unknownMethod"), "{msg}");
        assert!(msg.contains("Domain/set is not known"), "{msg}");

        // Not a JMAP response at all → loud.
        let msg = parse_method_response("Domain/set", &serde_json::json!({"ok": true}))
            .expect_err("must reject");
        assert!(msg.contains("no methodResponses"), "{msg}");

        // A different method answering → loud.
        let odd = serde_json::json!({
            "methodResponses": [["Domain/get", {}, "0"]],
        });
        let msg = parse_method_response("Domain/set", &odd).expect_err("must reject");
        assert!(msg.contains("Domain/get"), "{msg}");
    }

    /// A realistic `Domain/set create` reply — server-set id +
    /// dnsZoneFile ride `created` under our creation tag.
    #[test]
    fn domain_set_created_parses_id_and_zonefile() {
        let args = serde_json::json!({
            "created": {
                "k2": {
                    "id": "dom-7f3a",
                    "dnsZoneFile": "acme.dev. IN MX 10 mail.acme.dev.\n",
                }
            },
            "notCreated": null,
        });
        let created = parse_domain_set_created(&args).expect("created parsed");
        assert_eq!(created.id, "dom-7f3a");
        assert!(created.dns_zone_file.as_deref().unwrap().contains("MX"));

        // Zone file absent from the create reply → None (the caller
        // falls back to Domain/get).
        let args = serde_json::json!({ "created": { "k2": { "id": "dom-1" } } });
        let created = parse_domain_set_created(&args).expect("created parsed");
        assert_eq!(created.dns_zone_file, None);

        // notCreated → the server's SetError verbatim.
        let args = serde_json::json!({
            "notCreated": { "k2": { "type": "alreadyExists",
                                     "description": "domain exists" } },
        });
        let msg = parse_domain_set_created(&args).expect_err("must reject");
        assert!(msg.contains("alreadyExists"), "{msg}");
        assert!(msg.contains("domain exists"), "{msg}");

        // Neither → loud.
        assert!(parse_domain_set_created(&serde_json::json!({})).is_err());
    }

    #[test]
    fn domain_set_destroyed_requires_id_in_destroyed_list() {
        let ok = serde_json::json!({ "destroyed": ["dom-1", "dom-2"] });
        parse_domain_set_destroyed("dom-1", &ok).expect("destroyed");

        let rejected = serde_json::json!({
            "notDestroyed": { "dom-1": { "type": "forbidden" } },
        });
        let msg = parse_domain_set_destroyed("dom-1", &rejected).expect_err("must reject");
        assert!(msg.contains("forbidden"), "{msg}");

        let silent = serde_json::json!({ "destroyed": [] });
        assert!(parse_domain_set_destroyed("dom-1", &silent).is_err());
    }

    #[test]
    fn domain_get_zonefile_finds_our_id() {
        let args = serde_json::json!({
            "list": [
                { "id": "dom-other", "dnsZoneFile": "wrong.zone" },
                { "id": "dom-1", "dnsZoneFile": "acme.dev. IN MX 10 mail.acme.dev." },
            ],
            "notFound": [],
        });
        assert_eq!(
            parse_domain_get_zonefile("dom-1", &args).expect("found"),
            "acme.dev. IN MX 10 mail.acme.dev."
        );
        // Missing id / empty zone file → loud.
        assert!(parse_domain_get_zonefile("dom-9", &args).is_err());
        let empty = serde_json::json!({ "list": [{ "id": "dom-1", "dnsZoneFile": "  " }] });
        assert!(parse_domain_get_zonefile("dom-1", &empty).is_err());
    }

    // ── S2 — loopback mock round-trip (127.0.0.1 only, house rule) ──

    /// Minimal one-shot HTTP responder: accepts `hits` connections on
    /// a loopback port, reads each request (headers + Content-Length
    /// body), records it, and answers the canned JSON.
    fn spawn_mock_server(
        replies: Vec<String>,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
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
                // Read until the full head + declared body is in.
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

    /// Full `domain_create` round-trip against a loopback mock: the
    /// client discovers the api url from /.well-known/jmap, POSTs the
    /// PRD §6.1 create envelope with the Bearer key, and parses the
    /// created reply. Locks the whole S2 wire shape end to end.
    #[test]
    fn domain_create_round_trip_against_loopback_mock() {
        let created_reply = serde_json::json!({
            "methodResponses": [["Domain/set", {
                "created": { "k2": {
                    "id": "dom-42",
                    "dnsZoneFile": "acme.dev. 3600 IN MX 10 mail.acme.dev.\n",
                } },
            }, "0"]],
        })
        .to_string();
        // Reply 1: the session document (apiUrl is root-relative to
        // prove absolutization rides the real path too).
        let (port, rx) = spawn_mock_server(vec![
            r#"{"apiUrl": "/jmap/", "capabilities": {}}"#.to_string(),
            created_reply,
        ]);

        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let created = c.domain_create("acme.dev").expect("create round-trip");
        assert_eq!(created.id, "dom-42");
        assert!(created.dns_zone_file.as_deref().unwrap().contains("MX 10"));

        // Request 1 hit the well-known session document with the key.
        let req1 = rx.recv().expect("first request recorded");
        assert!(req1.starts_with("GET /.well-known/jmap"), "{req1}");
        assert!(req1.contains("authorization: Bearer k2-test-key")
            || req1.contains("Authorization: Bearer k2-test-key"), "{req1}");

        // Request 2 POSTed the JMAP envelope to the DISCOVERED path.
        let req2 = rx.recv().expect("second request recorded");
        assert!(req2.starts_with("POST /jmap/"), "{req2}");
        let body_start = req2.find("\r\n\r\n").expect("body") + 4;
        let body: serde_json::Value =
            serde_json::from_str(&req2[body_start..]).expect("JSON body");
        assert_eq!(body["methodCalls"][0][0], "Domain/set");
        let create = &body["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["name"], "acme.dev");
        assert_eq!(create["dkimManagement"], "automatic");
        assert_eq!(create["subAddressing"], "enabled");
        assert!(create["catchAllAddress"].is_null(), "catch-all OFF by default");
    }
}

#[cfg(test)]
mod s3_account_tests {
    use super::*;

    // ── Pure reply parsers (fixtures, no network) ───────────────────

    #[test]
    fn account_set_created_parses_id_and_surfaces_rejections() {
        let ok = serde_json::json!({
            "created": { "k2": { "id": "acc-7f3a", "quota": 1073741824u64 } },
            "notCreated": null,
        });
        assert_eq!(parse_account_set_created(&ok).expect("created"), "acc-7f3a");

        // Server-set id missing/blank → loud.
        let no_id = serde_json::json!({ "created": { "k2": { "quota": 1 } } });
        assert!(parse_account_set_created(&no_id).is_err());
        let blank = serde_json::json!({ "created": { "k2": { "id": "  " } } });
        assert!(parse_account_set_created(&blank).is_err());

        // notCreated → the server's SetError verbatim.
        let rejected = serde_json::json!({
            "notCreated": { "k2": { "type": "alreadyExists",
                                    "description": "name is taken" } },
        });
        let msg = parse_account_set_created(&rejected).expect_err("must reject");
        assert!(msg.contains("alreadyExists"), "{msg}");
        assert!(msg.contains("name is taken"), "{msg}");

        // Neither → loud.
        assert!(parse_account_set_created(&serde_json::json!({})).is_err());
    }

    #[test]
    fn account_set_updated_requires_id_in_updated_map() {
        // RFC 8620 /set: updated maps id → null (or server-set props).
        let ok = serde_json::json!({ "updated": { "acc-1": null } });
        parse_account_set_updated("acc-1", &ok).expect("updated");

        let rejected = serde_json::json!({
            "notUpdated": { "acc-1": { "type": "forbidden" } },
        });
        let msg = parse_account_set_updated("acc-1", &rejected).expect_err("must reject");
        assert!(msg.contains("forbidden"), "{msg}");

        // Someone ELSE updated / empty reply → loud, never a silent ok.
        let other = serde_json::json!({ "updated": { "acc-9": null } });
        assert!(parse_account_set_updated("acc-1", &other).is_err());
        assert!(parse_account_set_updated("acc-1", &serde_json::json!({})).is_err());
    }

    #[test]
    fn account_set_destroyed_requires_id_in_destroyed_list() {
        let ok = serde_json::json!({ "destroyed": ["acc-1", "acc-2"] });
        parse_account_set_destroyed("acc-1", &ok).expect("destroyed");

        let rejected = serde_json::json!({
            "notDestroyed": { "acc-1": { "type": "notFound" } },
        });
        let msg = parse_account_set_destroyed("acc-1", &rejected).expect_err("must reject");
        assert!(msg.contains("notFound"), "{msg}");

        let silent = serde_json::json!({ "destroyed": [] });
        assert!(parse_account_set_destroyed("acc-1", &silent).is_err());
    }

    // ── The ONE loopback mock round-trip for the S3 account call ────
    // (127.0.0.1 only, house rule — locks the whole wire shape:
    // discovery → envelope → create args → id extraction.)

    /// Minimal sequential responder (the s2 module's `spawn_mock_server`
    /// shape): accepts one connection per canned reply, records the raw
    /// request, answers 200 JSON.
    fn spawn_mock_server(
        replies: Vec<String>,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
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

    #[test]
    fn account_create_round_trip_against_loopback_mock() {
        let created_reply = serde_json::json!({
            "methodResponses": [["Account/set", {
                "created": { "k2": { "id": "acc-42" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            r#"{"apiUrl": "/jmap/", "capabilities": {}}"#.to_string(),
            created_reply,
        ]);

        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let id = c
            .account_create("research-bot", "dom-7", "s3cret-pw", 1_073_741_824, 10_000)
            .expect("create round-trip");
        assert_eq!(id, "acc-42");

        // Request 1: discovery with the Bearer key.
        let req1 = rx.recv().expect("first request recorded");
        assert!(req1.starts_with("GET /.well-known/jmap"), "{req1}");
        assert!(req1.contains("authorization: Bearer k2-test-key")
            || req1.contains("Authorization: Bearer k2-test-key"), "{req1}");

        // Request 2: the JMAP envelope on the DISCOVERED path with the
        // §7.1/§12 create args.
        let req2 = rx.recv().expect("second request recorded");
        assert!(req2.starts_with("POST /jmap/"), "{req2}");
        let body_start = req2.find("\r\n\r\n").expect("body") + 4;
        let body: serde_json::Value =
            serde_json::from_str(&req2[body_start..]).expect("JSON body");
        assert_eq!(body["using"][0], "urn:ietf:params:jmap:core");
        assert_eq!(body["methodCalls"][0][0], "Account/set");
        let create = &body["methodCalls"][0][1]["create"]["k2"];
        assert_eq!(create["type"], "individual");
        assert_eq!(create["name"], "research-bot");
        assert_eq!(create["domainId"], "dom-7");
        assert_eq!(create["secret"], "s3cret-pw");
        assert_eq!(create["quota"], 1_073_741_824u64, "§12: 1 GB quota at create");
        assert_eq!(create["maxMessages"], 10_000, "§12: 10k message cap at create");
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
}
