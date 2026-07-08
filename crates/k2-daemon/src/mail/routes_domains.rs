//! `/cli/mail/domain/*` — DOMAIN-concern handlers (mail slice S2):
//! add / remove / check + the dnsZoneFile record table reads.
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! (PRD §10), enforced in the dispatcher's `/cli/mail/` POST arm:
//! domain add/remove/check = OWNER-OR-ADMIN (`token_is_owner_or_admin`),
//! POST-only (`require_post` + `post_allowed`, house rule
//! feedback_post_only_route_guards); the list/show reads = any authed
//! token (GET) — `domain/list` serves BOTH audiences (§11.1.2): the
//! bare call is the owner table, `?project=<ws>` selects the agent
//! `k2 mail domains` view (Verified domains + the workspace default).
//!
//! NORMALIZATION contract (pre-mortem #14): every domain input runs
//! through `k2_core::mail_domain::normalize_mail_domain` inside the
//! ops layer (`super::domains`) BEFORE any lookup or store — only the
//! lowercase punycode A-label form ever reaches the DB or Stalwart.
//!
//! Error contract (feedback_routes conventions, camelCase bodies):
//! `{"ok":false,"error":{"code","hint"}}` with stable codes —
//! `usage` (400) · `not_found` (404) · `exists` / `confirm_required`
//! (409) · `not_ready` (503, mail server not installed/running) ·
//! `engine` (502, Stalwart said no) · `dns_unavailable` (500).

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::dns_verify;
use crate::mail::domains::{self, OpError};

// ── Response helpers ────────────────────────────────────────────────────

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

/// Map an ops-layer error onto the stable wire shapes.
fn op_error_response(err: OpError) -> CliResponse {
    match err {
        OpError::Usage(hint) => error_response("400 Bad Request", "usage", &hint),
        OpError::NotFound(hint) => error_response("404 Not Found", "not_found", &hint),
        OpError::Exists(hint) => error_response("409 Conflict", "exists", &hint),
        OpError::ConfirmRequired(hint) => {
            error_response("409 Conflict", "confirm_required", &hint)
        }
        OpError::Engine(hint) => error_response("502 Bad Gateway", "engine", &hint),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

// ── POST handlers ───────────────────────────────────────────────────────

/// `POST /cli/mail/domain/add` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AddBody {
    domain: String,
}

/// POST `/cli/mail/domain/add` — normalize → Stalwart `Domain/set
/// create` (auto-DKIM, sub-addressing on, catch-all OFF) → parse the
/// server-set `dnsZoneFile` → persist + return the DNS record table
/// (PRD §6.1-6.2).
pub fn handle_domain_add(body: &[u8]) -> CliResponse {
    let b: AddBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.domain.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'domain'");
    }
    // The live Stalwart client comes from the mail_server row; without
    // an installed+running server there is nothing to create domains
    // in (S1 owns getting it there).
    let (engine, _hostname) = match domains::engine_from_db() {
        Ok(e) => e,
        Err(hint) => return error_response("503 Service Unavailable", "not_ready", &hint),
    };
    match domains::add_domain(&engine, &b.domain) {
        Ok(v) => ok_json(v),
        Err(e) => op_error_response(e),
    }
}

/// `POST /cli/mail/domain/remove` body. `confirm` is REQUIRED true
/// (the refusal names the active-address count); `purge` (mail data)
/// is the separate checkbox (§6.6).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RemoveBody {
    domain: String,
    confirm: bool,
    purge: bool,
}

/// POST `/cli/mail/domain/remove` — retires the domain's addresses
/// (count reported), destroys the Stalwart domain after the explicit
/// confirm, deletes the K2 row (PRD §6.6).
pub fn handle_domain_remove(body: &[u8]) -> CliResponse {
    let b: RemoveBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.domain.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'domain'");
    }
    // The engine is only needed when a Stalwart-side domain exists.
    // No mail_server row at all (uninstalled) still allows removing
    // K2-side rows; an installed-but-unreachable server fails closed
    // inside remove_domain.
    let engine = domains::engine_from_db().ok();
    match domains::remove_domain(
        engine.as_ref().map(|(e, _)| e as &dyn domains::DomainEngine),
        &b.domain,
        b.confirm,
        b.purge,
    ) {
        Ok(v) => ok_json(v),
        Err(e) => op_error_response(e),
    }
}

/// `POST /cli/mail/domain/check` body.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CheckBody {
    domain: String,
}

/// POST `/cli/mail/domain/check` — force re-verification NOW (§6.3
/// [Check now]): resolve every required record, classify
/// Valid/Missing/Wrong with the expected-vs-live diff (never a bare
/// "missing", pre-mortem #4), persist, return the full table.
pub fn handle_domain_check(body: &[u8]) -> CliResponse {
    let b: CheckBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}")),
    };
    if b.domain.trim().is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'domain'");
    }
    let resolver = match dns_verify::SystemResolver::new() {
        Ok(r) => r,
        Err(hint) => {
            return error_response("500 Internal Server Error", "dns_unavailable", &hint)
        }
    };
    match dns_verify::check_domain_now(&resolver, &b.domain) {
        Ok(v) => ok_json(v),
        Err(e) => op_error_response(e),
    }
}

// ── GET handlers ────────────────────────────────────────────────────────

/// GET `/cli/mail/domain/list` — two audiences, one route (§11.1.2):
///
/// - **Owner table** (no `project` param): every domain + status chip
///   + send mode + per-record state summary + the DMARC nag.
/// - **Agent view** (`?project=<name|path|uuid>` — how `k2 mail
///   domains` calls it): ONLY Verified domains (what `k2 mail create`
///   can mint on) with the workspace default marked. Default-domain
///   resolution: the global `mail_default_domain` setting when it is
///   currently Verified, else the first Verified domain (empty setting
///   = first-verified — documented contract).
pub fn handle_domain_list(params: &HashMap<String, String>) -> CliResponse {
    let project = ["project", "project_path"]
        .iter()
        .find_map(|k| params.get(*k))
        .filter(|v| !v.is_empty());
    match project {
        None => ok_json(domains::list_owner_json()),
        Some(ws) => {
            // Validate the workspace token so a typo'd `k2 mail
            // domains` fails like every other workspace-scoped verb.
            if crate::workspace_msg::resolve_workspace(ws).is_none() {
                return crate::workspace_routes::workspace_not_found_response(ws);
            }
            ok_json(domains::list_agent_json())
        }
    }
}

/// GET `/cli/mail/domain/show?domain=<d>` — one domain's full record
/// table: per-record status + live values + copy strings (joined TXT)
/// + zone-file display form (split TXT preserved) + the downloadable
/// zone-file payload.
pub fn handle_domain_show(params: &HashMap<String, String>) -> CliResponse {
    let domain = crate::cli::str_param(params, "domain");
    if domain.is_empty() {
        return error_response("400 Bad Request", "usage", "missing 'domain' parameter");
    }
    match domains::show_json(&domain) {
        Ok(v) => ok_json(v),
        Err(e) => op_error_response(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — no network, no real DNS (house rules)
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::domains::tests::{FakeEngine, ZONE_FIXTURE};

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    fn cleanup(domain: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM mail_domains WHERE domain = ?1",
            rusqlite::params![domain],
        );
    }

    /// Seed a domain through the real ops path (fake engine — no
    /// network) so the GET routes read realistic persisted state.
    fn seed_domain(domain: &str) {
        cleanup(domain);
        let engine = FakeEngine {
            zone: ZONE_FIXTURE.replace("acme.dev", domain),
            ..FakeEngine::ok()
        };
        domains::add_domain(&engine, domain).expect("seed add");
    }

    // ── POST validation + gating ──

    #[test]
    fn add_rejects_bad_bodies_and_reports_not_ready_without_server() {
        // Body validation happens before any engine construction.
        let resp = handle_domain_add(b"not json");
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        let resp = handle_domain_add(b"{}");
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");

        // No mail_server row in the shared test DB → not_ready with a
        // hint that names the fix (Settings → Email), never a panic or
        // an accidental network dial.
        let resp = handle_domain_add(br#"{"domain":"acme.dev"}"#);
        assert_eq!(resp.status, "503 Service Unavailable");
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "not_ready");
        assert!(
            v["error"]["hint"].as_str().unwrap().contains("Settings → Email"),
            "{v}"
        );
    }

    #[test]
    fn remove_validates_body_and_maps_op_errors() {
        let resp = handle_domain_remove(b"{}");
        assert_eq!(resp.status, "400 Bad Request");

        // Unknown domain → 404 not_found (no engine needed to say so).
        let resp = handle_domain_remove(br#"{"domain":"ghost.example","confirm":true}"#);
        assert_eq!(resp.status, "404 Not Found");
        assert_eq!(body_json(&resp)["error"]["code"], "not_found");

        // Known domain without confirm → 409 confirm_required naming
        // the address count (§6.6).
        seed_domain("routes-rm.example");
        let resp = handle_domain_remove(br#"{"domain":"routes-rm.example"}"#);
        assert_eq!(resp.status, "409 Conflict");
        let v = body_json(&resp);
        assert_eq!(v["error"]["code"], "confirm_required");
        assert!(v["error"]["hint"].as_str().unwrap().contains("0 active address"), "{v}");

        // Confirmed, but the row has a Stalwart id and NO reachable
        // engine (no mail_server row) → fails CLOSED as engine error.
        let resp = handle_domain_remove(br#"{"domain":"routes-rm.example","confirm":true}"#);
        assert_eq!(resp.status, "502 Bad Gateway");
        assert_eq!(body_json(&resp)["error"]["code"], "engine");
        cleanup("routes-rm.example");
    }

    #[test]
    fn check_validates_before_touching_dns() {
        // Validation failures must not construct a resolver (no DNS in
        // tests): bad JSON + missing domain return usage immediately.
        let resp = handle_domain_check(b"nope");
        assert_eq!(resp.status, "400 Bad Request");
        let resp = handle_domain_check(b"{}");
        assert_eq!(resp.status, "400 Bad Request");
        assert_eq!(body_json(&resp)["error"]["code"], "usage");
    }

    // ── GET reads ──

    #[test]
    fn show_returns_the_full_record_table() {
        seed_domain("routes-show.example");
        let mut params = HashMap::new();
        params.insert("domain".to_string(), "ROUTES-SHOW.example.".to_string());
        // Normalization at the boundary: case + FQDN dot still hit.
        let resp = handle_domain_show(&params);
        assert_eq!(resp.status, "200 OK");
        let v = body_json(&resp);
        assert_eq!(v["ok"], true);
        assert_eq!(v["domain"], "routes-show.example");
        assert_eq!(v["status"], "pending");
        assert_eq!(v["dmarcNag"], true);
        let records = v["records"].as_array().unwrap();
        assert_eq!(records[0]["id"], "mx");
        assert_eq!(records[0]["status"], "pending");
        // The split DKIM row rides the wire with copy string, display
        // form, AND chunks (pre-mortem #4/#16).
        let rsa = records.iter().find(|r| r["id"] == "dkim:202601r").unwrap();
        assert!(rsa["expected"].as_str().unwrap().ends_with("IDAQAB"));
        assert!(rsa["expectedDisplay"].as_str().unwrap().contains("\" \""));
        assert_eq!(rsa["chunks"].as_array().unwrap().len(), 2);
        // Zone-file download payload present.
        assert!(v["zoneFile"].as_str().unwrap().contains("IN\tMX"));

        // Missing/unknown domain → usage / not_found.
        let resp = handle_domain_show(&HashMap::new());
        assert_eq!(resp.status, "400 Bad Request");
        let mut params = HashMap::new();
        params.insert("domain".to_string(), "ghost.example".to_string());
        let resp = handle_domain_show(&params);
        assert_eq!(resp.status, "404 Not Found");
        cleanup("routes-show.example");
    }

    #[test]
    fn list_owner_table_and_agent_view() {
        seed_domain("routes-list.example");
        // Flip it Verified directly (the dns_verify e2e test owns the
        // full transition; here we test the read shapes).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE mail_domains SET status = 'verified', verified_at = 123 \
                 WHERE domain = 'routes-list.example'",
                [],
            )
            .expect("flip verified");
        }

        // Owner table: full chips + record summary.
        let resp = handle_domain_list(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v = body_json(&resp);
        let ours = v["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["domain"] == "routes-list.example")
            .expect("listed");
        assert_eq!(ours["status"], "verified");
        assert_eq!(ours["sendMode"], "receive-only");
        assert!(ours["records"]["pending"].as_u64().unwrap() >= 4);
        assert_eq!(ours["dmarcNag"], true);

        // Agent view (`?project=`): unknown workspace → the shared 404
        // shape; a valid one isn't constructible in this unit test
        // (workspace registration is another module's fixture), so the
        // agent SHAPE is asserted through list_agent_json directly.
        let mut params = HashMap::new();
        params.insert("project".to_string(), "no-such-workspace-xyz".to_string());
        let resp = handle_domain_list(&params);
        assert_eq!(resp.status, "404 Not Found");

        let v = domains::list_agent_json();
        let ours = v["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["domain"] == "routes-list.example")
            .expect("verified domain visible to agents");
        // Agent rows carry ONLY domain + default marker — no status
        // chips, no record table (§11.1.2: agents see what they can
        // mint on).
        assert_eq!(ours.as_object().unwrap().len(), 2, "{ours}");
        assert!(ours["isDefault"].is_boolean());
        assert!(v["defaultDomain"].is_string(), "some verified domain is the default");

        cleanup("routes-list.example");
    }
}
