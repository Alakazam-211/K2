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
//! The typed calls below are S1/S2/S3/S5 stubs returning the
//! structured not-built error; the transport helpers are real.

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
        let url = format!("{}{}", self.base_url, path);
        // Manual JSON body: the crate's reqwest is built without the
        // `json` feature (same minimal feature set the rest of the
        // daemon uses) — serialize + set the header ourselves.
        let payload =
            serde_json::to_string(body).map_err(|e| format!("POST {path}: body serialize: {e}"))?;
        let resp = Self::http()?
            .post(&url)
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

    // ── Typed calls (stubs — bodies land in their slices) ───────────

    /// S2 — `Domain/set create` with automatic DKIM + sub-addressing,
    /// catch-all OFF (PRD §6.1). Returns the server-set domain id.
    #[allow(dead_code)] // S2 wires this into domain add.
    pub fn domain_create(&self, domain: &str) -> Result<String, String> {
        let _ = domain;
        Err(super::not_built_err("S2", "jmap Domain/set create"))
    }

    /// S2 — destroy a domain (after the route layer's explicit
    /// confirm + address retirement, PRD §6.6).
    #[allow(dead_code)] // S2 wires this into domain remove.
    pub fn domain_delete(&self, stalwart_domain_id: &str) -> Result<(), String> {
        let _ = stalwart_domain_id;
        Err(super::not_built_err("S2", "jmap Domain/set destroy"))
    }

    /// S2 — read the domain's server-set `dnsZoneFile` (the SSOT for
    /// the record table, PRD §6.2 — K2 computes nothing itself except
    /// relay-mode SPF adjustments).
    #[allow(dead_code)] // S2 wires this into the record table.
    pub fn domain_dns_zonefile(&self, stalwart_domain_id: &str) -> Result<String, String> {
        let _ = stalwart_domain_id;
        Err(super::not_built_err("S2", "jmap Domain dnsZoneFile read"))
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
    /// double the slash; typed stubs fail with the structured error.
    #[test]
    fn client_construction_and_stub_errors() {
        let c = StalwartClient::new("https://127.0.0.1:8443///", "k2-test-key");
        assert_eq!(c.base_url, "https://127.0.0.1:8443");
        assert!(c
            .domain_create("acme.dev")
            .unwrap_err()
            .contains("not built yet — mail slice S2"));
        assert!(c
            .domain_delete("d1")
            .unwrap_err()
            .contains("not built yet — mail slice S2"));
        assert!(c
            .domain_dns_zonefile("d1")
            .unwrap_err()
            .contains("not built yet — mail slice S2"));
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
}
