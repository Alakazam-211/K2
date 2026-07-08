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
//!   6. S5 [`StalwartClient::submission_send`] — the outbound hand-off
//!      (RFC 8621 §7 EmailSubmission over the loopback mgmt endpoint).
//!      Uncertainties the first live send must confirm, all isolated in
//!      that one function (+ its [`Self::identity_id_for`] helper):
//!      whether Stalwart pre-creates an Identity per account address or
//!      the `Identity/set create` fallback is needed; whether the
//!      service account may create/submit in a MEMBER account via the
//!      delegated `accountId` (the same S4 #1 scoping question);
//!      whether a `drafts`-role mailbox exists on fresh accounts (the
//!      inbox fallback covers its absence); and whether Stalwart stamps
//!      `Message-ID`/`Date` on submission when the created Email lacks
//!      them.
//!   7. S6 [`StalwartClient::relay_route_apply`] (+ [`relay_route_keys`])
//!      — the smart-host outbound-route config keys
//!      (`queue.route.<id>.*`) and how a route is BOUND to one sender
//!      domain (a `sender-domain` key here; possibly a routing
//!      expression upstream).

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

    /// S6 — apply (`Some`) or clear (`None`) the SMART-HOST outbound
    /// route for one domain (PRD §8.3: "the daemon configures
    /// Stalwart's outbound route for that domain to the smart host").
    /// Called by the S6 config surface when a domain's send mode
    /// enters/leaves `relay` or its attached relay config changes.
    /// The password rides the settings payload once and is never
    /// logged (transport errors excerpt the RESPONSE body only).
    ///
    /// ⚠ LIVE-BOX (#7): the relay-route config-key scheme. Modeled on
    /// the documented pre-0.16 `queue.route.<id>.*` block (address /
    /// port / protocol / implicit-TLS / auth credentials) plus a
    /// `sender-domain` binding key so ONLY this domain's outbound uses
    /// the smart host — Stalwart may spell the binding as a routing
    /// EXPRESSION instead of a per-route key. This function (and
    /// [`relay_route_keys`]) is the single place to fix on the live
    /// box; nothing else in the daemon knows the shape.
    pub fn relay_route_apply(
        &self,
        domain: &str,
        route: Option<&RelayRoute>,
    ) -> Result<(), String> {
        let api_url = self.discover_api_url()?;
        let keys = relay_route_keys(domain);
        match route {
            Some(r) => {
                let set: Vec<(String, serde_json::Value)> = keys
                    .iter()
                    .cloned()
                    .zip([
                        serde_json::json!(r.host),
                        serde_json::json!(r.port),
                        serde_json::json!("smtp"),
                        serde_json::json!(r.implicit_tls),
                        serde_json::json!(r.username),
                        serde_json::json!(r.password),
                        serde_json::json!(domain),
                    ])
                    .collect();
                self.settings_set(&api_url, &set)
            }
            None => self.settings_destroy(&api_url, &keys),
        }
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

    // ── S5 outbound submission — LOCAL Stalwart only (loopback JMAP
    //    EmailSubmission, RFC 8621 §7). The audit row in `mail_outbound`
    //    exists BEFORE these are called (pre-mortem #11: no row, no
    //    send); Stalwart's queue owns retries and the smart-host relay
    //    routing (pre-mortem #9 — no daemon-side retry logic, ever).
    //    Message content flows through here — never logged, never in
    //    errors (errors carry method names + server SetErrors only). ──

    /// ⚠ LIVE-BOX (S5 #1, module-header item 6): submit one composed
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
        // account has one, else the inbox (fresh Stalwart accounts may
        // lack a drafts mailbox — flagged in the module-header item 6).
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

    /// ⚠ LIVE-BOX (S5 #1 helper): the sending Identity for `from_email`
    /// in `account_id` — `Identity/get`, matched case-insensitively on
    /// the email; when the account has none (Stalwart may or may not
    /// pre-create one per address), fall back to `Identity/set create`.
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
    /// scoping, S4 #1).
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
    //    Email/get, Email/set, blob download), which is stable across
    //    Stalwart versions, unlike the ⚠ mgmt objects above. The ONE
    //    flagged uncertainty is the account-scoping shape, isolated in
    //    [`Self::mail_call`]. Message BODIES flow through here — they
    //    are never logged (pre-mortem #16) and never appear in errors
    //    (errors carry method names + server SetErrors only). ────────

    /// ⚠ LIVE-BOX (S4 #1): HOW the k2-daemon service account reads
    /// OTHER accounts' mail. Modeled as the most standard JMAP shape
    /// (RFC 8620 delegated access): every method call carries
    /// `accountId: <target Stalwart account>` while the HTTP
    /// authorization stays the service account's ApiKey — Stalwart
    /// admin-class principals can address member accounts this way.
    /// If the live box refuses (a `forbidden`/`accountNotFound` JMAP
    /// error), the fix lands HERE and nowhere else; candidate shapes
    /// to try on the box: grant the service account per-account ACLs,
    /// an admin masquerade header (`X-*`?), or widening the ApiKey's
    /// permission list with the individual-access permission. Nothing
    /// outside this function knows how scoping is spelled.
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
    /// hardcoded, same rule as `apiUrl`).
    pub fn blob_download(
        &self,
        account_id: &str,
        blob_id: &str,
        name: &str,
        mime: &str,
    ) -> Result<Vec<u8>, String> {
        let session = self.get_json("/.well-known/jmap")?;
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

/// ⚠ LIVE-BOX (#7, helper): the config keys for one domain's relay
/// route, in the order [`StalwartClient::relay_route_apply`] zips its
/// values. Dots in the domain would read as key separators — the route
/// id replaces them with dashes.
pub fn relay_route_keys(domain: &str) -> Vec<String> {
    let id = format!("k2-relay-{}", domain.replace('.', "-"));
    [
        "address",
        "port",
        "protocol",
        "tls.implicit",
        "auth.username",
        "auth.secret",
        "sender-domain",
    ]
    .iter()
    .map(|k| format!("queue.route.{id}.{k}"))
    .collect()
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

// ── S4 mail-read wire layer (RFC 8621 constants, structs, parsers) ─────

/// JMAP `using` for MAIL data calls (RFC 8621) — distinct from the
/// mgmt envelope's Stalwart-specific URNs.
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
/// S3 account update: the id must appear in `updated`.
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

/// Pure session-document parser for the RFC 8620 `downloadUrl`
/// template — discovered, never hardcoded (the `apiUrl` rule).
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
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(url.to_string());
    }
    if !url.starts_with('/') {
        return Err(format!(
            "JMAP session 'downloadUrl' is neither absolute nor root-relative: '{url}'"
        ));
    }
    Ok(format!("{}{}", base_url.trim_end_matches('/'), url))
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
    /// double the slash. (The S2 domain + S3 account + S5 submission
    /// calls are REAL — their behavior is owned by the fixture tests,
    /// never a live dial from here.)
    #[test]
    fn client_construction_normalizes_base() {
        let c = StalwartClient::new("https://127.0.0.1:8443///", "k2-test-key");
        assert_eq!(c.base_url, "https://127.0.0.1:8443");
    }

    // ── S5 submission parsers (fixtures — no network) ──

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
        // Happy path: both created.
        let ok = serde_json::json!({
            "methodResponses": [
                ["Email/set", { "created": { "k2out": { "id": "M9" } } }, "0"],
                ["EmailSubmission/set", { "created": { "k2sub": { "id": "S1" } } }, "1"],
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
    /// double the slash. (Every typed call is REAL as of S5 — the last
    /// stub, queue submit, became [`StalwartClient::submission_send`];
    /// S3's account calls live in `s3_account_tests`.)
    #[test]
    fn client_construction_and_stub_errors() {
        let c = StalwartClient::new("https://127.0.0.1:8443///", "k2-test-key");
        assert_eq!(c.base_url, "https://127.0.0.1:8443");
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

#[cfg(test)]
mod s4_mail_tests {
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

    // ── The ONE loopback mock round-trip for the S4 read path ────────
    // (127.0.0.1 only, house rule — locks the wire shape end to end:
    // discovery → mail envelope + accountId injection → query → get →
    // mark-seen → blob download.)

    use std::io::{Read, Write};

    fn spawn_mock_server(
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

    fn body_json(req: &str) -> serde_json::Value {
        let start = req.find("\r\n\r\n").expect("body") + 4;
        serde_json::from_str(&req[start..]).expect("JSON body")
    }

    #[test]
    fn mail_read_round_trip_against_loopback_mock() {
        let session = r#"{"apiUrl": "/jmap/", "capabilities": {}}"#.to_string();
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
        // Each mail_call discovers the api url first (session, call).
        let (port, rx) = spawn_mock_server(vec![
            session.clone(),
            query_reply,
            session.clone(),
            get_reply,
            session,
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

        // Request 1: discovery. Request 2: the Email/query envelope —
        // mail capability + the INJECTED accountId (the LIVE-BOX seam).
        let _disc = rx.recv().expect("req1");
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

        let _disc = rx.recv().expect("req3");
        let g = rx.recv().expect("req4");
        let v = body_json(&g);
        assert_eq!(v["methodCalls"][0][0], "Email/get");
        assert_eq!(v["methodCalls"][0][1]["accountId"], "acc-7");
        let props = v["methodCalls"][0][1]["properties"].as_array().unwrap();
        assert!(
            !props.iter().any(|p| p == "bodyValues"),
            "summaries must never fetch bodies"
        );

        let _disc = rx.recv().expect("req5");
        let s = rx.recv().expect("req6");
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
        let _disc = rx.recv().expect("req1");
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
    use super::*;

    /// Same loopback mock the sibling test modules use (each module
    /// keeps its own copy — house pattern).
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
    fn relay_route_keys_are_domain_scoped_and_dot_free() {
        let keys = relay_route_keys("acme.dev");
        assert_eq!(keys.len(), 7);
        assert!(keys.iter().all(|k| k.starts_with("queue.route.k2-relay-acme-dev.")), "{keys:?}");
        assert!(keys.iter().any(|k| k.ends_with(".auth.secret")));
        assert!(keys.iter().any(|k| k.ends_with(".sender-domain")));
        // Different domains never collide on a route id.
        assert_ne!(relay_route_keys("a.dev")[0], relay_route_keys("b.dev")[0]);
    }

    /// Apply round-trip: discovery + one Settings/set carrying every
    /// route key (creds ride the payload; the wire assertion is the
    /// ONLY place the test looks for them).
    #[test]
    fn relay_route_apply_sets_and_clears_the_route_block() {
        let ok = r#"{"methodResponses":[["Settings/set",{"updated":{}},"0"]]}"#.to_string();
        let session = r#"{"apiUrl": "/jmap/", "capabilities": {}}"#.to_string();
        let (port, rx) = spawn_mock_server(vec![session.clone(), ok.clone()]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let route = RelayRoute {
            host: "smtp.mailgun.org".into(),
            port: 587,
            username: "postmaster@acme.dev".into(),
            password: "relay-pw".into(),
            implicit_tls: false,
        };
        c.relay_route_apply("acme.dev", Some(&route)).expect("apply");
        let _disc = rx.recv().expect("req1");
        let set = rx.recv().expect("req2");
        let body: serde_json::Value =
            serde_json::from_str(&set[set.find("\r\n\r\n").expect("body") + 4..]).expect("json");
        assert_eq!(body["methodCalls"][0][0], "Settings/set");
        let update = &body["methodCalls"][0][1]["update"];
        assert_eq!(update["queue.route.k2-relay-acme-dev.address"], "smtp.mailgun.org");
        assert_eq!(update["queue.route.k2-relay-acme-dev.port"], 587);
        assert_eq!(update["queue.route.k2-relay-acme-dev.tls.implicit"], false);
        assert_eq!(update["queue.route.k2-relay-acme-dev.auth.username"], "postmaster@acme.dev");
        assert_eq!(update["queue.route.k2-relay-acme-dev.auth.secret"], "relay-pw");
        assert_eq!(update["queue.route.k2-relay-acme-dev.sender-domain"], "acme.dev");

        // Clear (None) destroys the same key set.
        let (port, rx) = spawn_mock_server(vec![session, ok]);
        let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        c.relay_route_apply("acme.dev", None).expect("clear");
        let _disc = rx.recv().expect("req1");
        let destroy = rx.recv().expect("req2");
        let body: serde_json::Value = serde_json::from_str(
            &destroy[destroy.find("\r\n\r\n").expect("body") + 4..],
        )
        .expect("json");
        let destroyed = body["methodCalls"][0][1]["destroy"].as_array().expect("destroy list");
        assert_eq!(destroyed.len(), 7);
        assert!(destroyed
            .iter()
            .all(|k| k.as_str().unwrap().starts_with("queue.route.k2-relay-acme-dev.")));
    }
}
