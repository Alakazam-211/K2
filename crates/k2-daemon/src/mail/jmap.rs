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

    /// ✔ LIVE-VERIFIED: list the registry's network listeners
    /// (id + name are all the port plan needs).
    pub fn listeners_get(&self) -> Result<Vec<ListenerInfo>, String> {
        let resp = self.registry_call("x:NetworkListener/get", serde_json::json!({}))?;
        parse_listeners(&resp)
    }

    /// ✔ LIVE-VERIFIED: apply the §5.3 port plan in ONE
    /// `x:NetworkListener/set`:
    /// - destroy the default `imaps`/`pop3s`/`sieve` listeners (§10);
    /// - move the default plain-HTTP `http` listener from `[::]:8080`
    ///   to the LOOPBACK mgmt bind (`127.0.0.1:8180`) — this is both
    ///   "setup listener off" (pre-mortem #13) and the permanent mgmt
    ///   endpoint in one move;
    /// - bind `https` per the port plan (`[::]:443` for `tls-alpn`,
    ///   loopback `127.0.0.1:8443` otherwise);
    /// - create the missing STARTTLS `submission` listener on :587.
    /// NOTE (✔ live-verified): the set succeeds but sockets only move
    /// on the supervisor's final RESTART.
    pub fn listeners_apply(
        &self,
        port_plan: &str,
        listeners: &[ListenerInfo],
    ) -> Result<(), String> {
        let https_bind = if port_plan == "tls-alpn" { "[::]:443" } else { "127.0.0.1:8443" };
        let mut destroy: Vec<String> = Vec::new();
        let mut update = serde_json::Map::new();
        let mut create = serde_json::Map::new();
        let mut has_submission = false;
        for l in listeners {
            match l.name.as_str() {
                "imaps" | "pop3s" | "sieve" => destroy.push(l.id.clone()),
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
    /// semantics; this function owns only the wire shape).
    pub fn email_query_ids(
        &self,
        account_id: &str,
        filter: serde_json::Value,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        let args = self.mail_call(
            account_id,
            "Email/query",
            serde_json::json!({
                "filter": filter,
                "sort": [{ "property": "receivedAt", "isAscending": false }],
                "limit": limit,
            }),
        )?;
        Ok(parse_query_ids(&args))
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

    /// S4 — download one blob (attachment bytes, or the raw RFC 822
    /// message via its `blobId`) through the session document's
    /// `downloadUrl` template (RFC 8620 §2 — discovered, never
    /// hardcoded, and REBASED onto our loopback base like `apiUrl`).
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

// ── Registry wire layer (envelope + pure parsers) ───────────────────────

/// ✔ LIVE-VERIFIED: the Bootstrap object is a singleton whose JMAP id
/// is literally the string `"singleton"`.
const BOOTSTRAP_SINGLETON_ID: &str = "singleton";

/// ✔ LIVE-VERIFIED: the MtaOutboundStrategy singleton uses the same
/// literal id.
const OUTBOUND_STRATEGY_SINGLETON_ID: &str = "singleton";

/// The loopback bind for the permanent plain-HTTP mgmt listener (the
/// supervisor's `STALWART_MGMT_URL` counterpart).
const STALWART_MGMT_BIND: &str = "127.0.0.1:8180";

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
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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

    pub(super) fn spawn_mock_server(
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

    pub(super) fn body_json(req: &str) -> serde_json::Value {
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
                "destroyed": ["L-imaps", "L-pop3s", "L-sieve"],
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
        assert_eq!(destroy, ["L-imaps", "L-pop3s", "L-sieve"], "§10 listeners torn out");
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
            .email_query_ids("acc-7", serde_json::json!({ "inMailbox": "IB" }), 20)
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
