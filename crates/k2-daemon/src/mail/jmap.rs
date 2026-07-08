//! Minimal typed client for Stalwart's management API (JMAP).
//!
//! BOUNDARY (PRD §4 / pre-mortem #2): this is the ONLY way K2 talks to
//! Stalwart — plain HTTP against its public management API, over
//! localhost, authenticated with the least-privilege ApiKey the
//! supervisor mints at bootstrap. No Stalwart crate is ever linked.
//!
//! ENDPOINT DISCOVERY (PRD §4.1): upstream docs are inconsistent about
//! `/api` vs `/jmap`, so the API path is NEVER hardcoded — it is read
//! from the JMAP **session document** at `/.well-known/jmap`
//! ([`StalwartClient::discover_api_url`], pure parser
//! [`parse_session_api_url`], unit-tested against a fixture).
//!
//! The S2 domain calls (`domain_create` / `domain_delete` /
//! `domain_dns_zonefile`) are REAL: JMAP method calls composed here,
//! posted to the DISCOVERED api url, parsed by pure fixture-tested
//! parsers. The remaining typed calls are S1/S3/S5 stubs returning the
//! structured not-built error; the transport helpers are real.
//!
//! LIVE-BOX VERIFICATION FLAG (S2): the exact `using` capability URN
//! for Stalwart's Domain management methods and the property spellings
//! (`dkimManagement: "automatic"`, `subAddressing: "enabled"`,
//! server-set `dnsZoneFile`) follow the PRD script (§6.1) — Stalwart
//! has no mgmt-API stability policy, so S2's first live-box run must
//! confirm the envelope against the pinned v0.16.x before ship. The
//! parsers are strict JMAP `/set`/`/get` shapes and will fail LOUDLY
//! (named method + body excerpt) on any drift.

use std::time::Duration;

/// Client for one Stalwart instance's management API.
///
/// `base_url` is `mail_server.api_url` (e.g. `https://127.0.0.1:8443`,
/// localhost-only in port plans B/C); `api_key` is resolved from the
/// daemon's secret store via `mail_server.api_key_ref` — the caller
/// passes the SECRET here, never the ref. Never logged.
// dead_code allows in this file: the client's first production caller
// is the S1 supervisor bootstrap — until then the binary compilation
// (main.rs's private mod graph) sees no call sites. The transport +
// parser are REAL and unit-tested below; remove the allows as S1 wires
// them in.
#[allow(dead_code)]
pub struct StalwartClient {
    base_url: String,
    api_key: String,
}

/// Request timeout for mgmt calls — localhost, so generous is still
/// snappy; long-poll reads (S4 `wait`) will use their own client.
#[allow(dead_code)] // first caller: S1 supervisor bootstrap.
const MGMT_TIMEOUT: Duration = Duration::from_secs(15);

#[allow(dead_code)] // first caller: S1 supervisor bootstrap.
impl StalwartClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self { base_url, api_key: api_key.into() }
    }

    fn http() -> Result<reqwest::blocking::Client, String> {
        reqwest::blocking::Client::builder()
            .timeout(MGMT_TIMEOUT)
            // The sidecar's cert may be self-signed pre-ACME during
            // bootstrap; S1 decides the exact trust story (pinned cert
            // vs plain-HTTP-on-loopback). Default-verify until then.
            .build()
            .map_err(|e| format!("mgmt http client: {e}"))
    }

    /// GET `base_url + path` (path must start with `/`), parse JSON.
    /// The ApiKey rides `Authorization: Bearer` per Stalwart's API-key
    /// auth. Errors are one-line: status + a short body excerpt —
    /// never the key.
    pub fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let resp = Self::http()?
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .map_err(|e| format!("GET {path}: {e}"))?;
        Self::json_or_err("GET", path, resp)
    }

    /// POST a JSON body to `base_url + path`, parse the JSON reply.
    pub fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.post_json_url(&format!("{}{}", self.base_url, path), path, body)
    }

    /// POST a JSON body to an ABSOLUTE url (the discovered JMAP apiUrl
    /// is absolute, PRD §4.1). `label` is the short name used in error
    /// lines so they never leak the full url/key.
    fn post_json_url(
        &self,
        url: &str,
        label: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let path = label;
        // Manual JSON body: the crate's reqwest is built without the
        // `json` feature (same minimal feature set the rest of the
        // daemon uses) — serialize + set the header ourselves.
        let payload =
            serde_json::to_string(body).map_err(|e| format!("POST {path}: body serialize: {e}"))?;
        let resp = Self::http()?
            .post(url)
            .bearer_auth(&self.api_key)
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| format!("POST {path}: {e}"))?;
        Self::json_or_err("POST", path, resp)
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

    // ── Typed calls (S2 domain calls REAL; S1/S3/S5 stubs) ──────────

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
        let resp = self.post_json_url(&api_url, "jmap api", &jmap_envelope(method, args))?;
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

    /// S3 — `Account/set create` (type User, local-part + domain,
    /// random password K2 stores but never surfaces, quota per §12).
    #[allow(dead_code)] // S3 wires this into address minting.
    pub fn account_create(
        &self,
        local_part: &str,
        stalwart_domain_id: &str,
    ) -> Result<String, String> {
        let _ = (local_part, stalwart_domain_id);
        Err(super::not_built_err("S3", "jmap Account/set create"))
    }

    /// S3 — disable an account (address retire: stops receiving,
    /// mailbox data kept for the retention window, PRD §7.2).
    #[allow(dead_code)] // S3 wires this into address retire.
    pub fn account_disable(&self, stalwart_account_id: &str) -> Result<(), String> {
        let _ = stalwart_account_id;
        Err(super::not_built_err("S3", "jmap Account/set disable"))
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

/// The JMAP `using` capabilities for domain management calls.
/// LIVE-BOX FLAG (see module docs): the Stalwart-specific URN must be
/// confirmed against the pinned v0.16.x on the first live run.
const JMAP_USING: [&str; 2] = [
    "urn:ietf:params:jmap:core",
    "https://stalw.art/jmap/domain",
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

#[cfg(test)]
mod tests {
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
    /// double the slash; the remaining typed stubs (S3/S5) fail with
    /// the structured error.
    #[test]
    fn client_construction_and_stub_errors() {
        let c = StalwartClient::new("https://127.0.0.1:8443///", "k2-test-key");
        assert_eq!(c.base_url, "https://127.0.0.1:8443");
        assert!(c
            .account_create("scout", "d1")
            .unwrap_err()
            .contains("not built yet — mail slice S3"));
        assert!(c
            .account_disable("a1")
            .unwrap_err()
            .contains("not built yet — mail slice S3"));
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
