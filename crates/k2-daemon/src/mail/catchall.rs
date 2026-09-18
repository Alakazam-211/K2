//! `GET|POST /cli/mail/catchall` — Domain.catchAllAddress only.
//!
//! Set `{domain, address}` (full minted addr on that domain).
//! Unset `{domain, address:""}` → JMAP `catchAllAddress: null`.
//! Never writes `Domain.aliases`.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::hostmail_auth::{
    authorize_mailbox, engine, err_json, load_hosted_domain, stalwart_domain_id,
};

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CatchallBody {
    domain: Option<String>,
    address: Option<serde_json::Value>,
}

fn domain_of_address(address: &str) -> Option<&str> {
    address.split_once('@').map(|(_, d)| d)
}

/// GET `/cli/mail/catchall?domain=` — live `catchAllAddress` or null.
pub fn handle_catchall_get(params: &HashMap<String, String>) -> CliResponse {
    let domain = crate::cli::str_param(params, "domain");
    if domain.trim().is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'domain' — which hosted domain?".to_string(),
        );
    }
    let row = match load_hosted_domain(&domain) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let domain_id = match stalwart_domain_id(&row) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match client.domain_get_catchall(&domain_id) {
        Ok(addr) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "domain": row.domain,
                "catchAllAddress": addr,
            })
            .to_string(),
        ),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/catchall` `{domain, address}` set or `{domain, address:""}` unset.
pub fn handle_catchall_post(body: &[u8]) -> CliResponse {
    let b: CatchallBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let domain_raw = b
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    if domain_raw.is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "missing 'domain' — which hosted domain?".to_string(),
        );
    }
    let address_field = b.address.as_ref();
    let unset = match address_field {
        None => {
            return err_json(
                "400 Bad Request",
                "usage",
                "missing 'address' — minted mailbox on this domain, or empty to unset".to_string(),
            )
        }
        Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(s)) => s.trim().is_empty(),
        Some(_) => {
            return err_json(
                "400 Bad Request",
                "usage",
                "'address' must be a full address or empty/null to unset".to_string(),
            )
        }
    };
    let row = match load_hosted_domain(domain_raw) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let domain_id = match stalwart_domain_id(&row) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let set_to = if unset {
        None
    } else {
        let raw = address_field.and_then(|v| v.as_str()).unwrap_or("").trim();
        let dest = match authorize_mailbox(raw) {
            Ok(r) => r,
            Err(resp) => {
                if resp.status == "404 Not Found" {
                    return err_json(
                        "400 Bad Request",
                        "usage",
                        format!("catch-all dest '{raw}' is not a minted mailbox on this host"),
                    );
                }
                return resp;
            }
        };
        let dest_domain = domain_of_address(&dest.address).unwrap_or("");
        if dest_domain != row.domain {
            return err_json(
                "400 Bad Request",
                "usage",
                format!(
                    "catch-all dest '{}' must be minted on domain '{}'",
                    dest.address, row.domain
                ),
            );
        }
        Some(dest.address)
    };

    let client = match engine() {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Err(e) = client.domain_set_catchall(&domain_id, set_to.as_deref()) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "domain": row.domain,
            "catchAllAddress": set_to,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_without_domain_is_usage_not_405() {
        let r = handle_catchall_get(&HashMap::new());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("domain"), "{}", r.body);
        assert_ne!(r.status, "405 Method Not Allowed");
    }

    #[test]
    fn post_without_domain_is_usage() {
        let r = handle_catchall_post(br#"{"address":"a@b.test"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("domain"), "{}", r.body);
    }

    #[test]
    fn post_without_address_is_usage() {
        let r = handle_catchall_post(br#"{"domain":"b.test"}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("address"), "{}", r.body);
    }

    #[test]
    fn unknown_domain_is_not_found() {
        let mut params = HashMap::new();
        params.insert(
            "domain".to_string(),
            "no-such-catchall-domain.example.test".to_string(),
        );
        let r = handle_catchall_get(&params);
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
    }
}
