//! `GET|POST /cli/mail/footer` and `POST /cli/mail/footer/unset`.
//!
//! Host-wide outbound DATA-stage trusted Sieve (`x:SieveSystemScript`
//! name `k2-footer` + `x:MtaStageData` `script` gated on
//! `authenticated_as`). `--domain` is 400 this cut.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::domains;
use crate::mail::jmap::{self, StalwartClient};
use crate::mail::sieve_user::{cap_text, reject_html, reject_lone_dot_line};

pub const FOOTER_SCRIPT_NAME: &str = "k2-footer";
const FOOTER_VAR: &str = "k2footer";

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct FooterSetBody {
    text: Option<String>,
    domain: Option<String>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
struct FooterUnsetBody {
    domain: Option<String>,
}

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
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

fn usage(hint: String) -> CliResponse {
    err_json("400 Bad Request", "usage", hint)
}

fn domain_reserved(domain: Option<&str>) -> Result<(), CliResponse> {
    if domain.map(str::trim).filter(|s| !s.is_empty()).is_some() {
        return Err(usage(
            "--domain is reserved — footer is host-wide this cut".to_string(),
        ));
    }
    Ok(())
}

fn engine() -> Result<StalwartClient, CliResponse> {
    domains::engine_from_db()
        .map(|(e, _)| e)
        .map_err(|hint| err_json("503 Service Unavailable", "not_ready", hint))
}

/// Trusted DATA-stage append: `foreverypart` `text/plain` only; skip
/// if the part already contains the exact footer (idempotent).
pub fn compose_footer_script(text: &str) -> String {
    format!(
        "require [\"foreverypart\", \"mime\", \"replace\", \"extracttext\", \"variables\"];\n\
         set \"{FOOTER_VAR}\" text:\n\
         {text}\n\
         .\n\
         foreverypart\n\
         {{\n\
           if header :mime :contenttype :is \"Content-Type\" \"text/plain\"\n\
           {{\n\
             extracttext \"text_content\";\n\
             if not string :contains \"${{text_content}}\" \"${{{FOOTER_VAR}}}\" {{\n\
               replace text:\n\
         ${{text_content}}\n\
         \n\
         ${{{FOOTER_VAR}}}\n\
         .\n\
             }}\n\
           }}\n\
         }}\n"
    )
}

fn parse_footer_text(contents: &str) -> Option<String> {
    let key = format!("set \"{FOOTER_VAR}\" text:");
    let i = contents.find(&key)?;
    let after = &contents[i + key.len()..];
    let after = after.strip_prefix('\r').unwrap_or(after);
    let after = after.strip_prefix('\n').unwrap_or(after);
    let mut body = String::new();
    for line in after.lines() {
        let line = line.trim_end_matches('\r');
        if line == "." {
            return Some(body.trim_end_matches('\n').to_string());
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(line);
    }
    None
}

/// GET `/cli/mail/footer`
pub fn handle_footer_get(_params: &HashMap<String, String>) -> CliResponse {
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    match engine.system_scripts_list() {
        Ok(list) => {
            let found = list.into_iter().find(|s| s.name == FOOTER_SCRIPT_NAME);
            match found {
                Some(s) if s.is_active || !s.contents.is_empty() => {
                    let text = parse_footer_text(&s.contents).unwrap_or(s.contents);
                    CliResponse::ok_json(
                        serde_json::json!({
                            "ok": true,
                            "enabled": true,
                            "text": text,
                            "reloaded": false,
                        })
                        .to_string(),
                    )
                }
                _ => CliResponse::ok_json(
                    serde_json::json!({
                        "ok": true,
                        "enabled": false,
                        "text": serde_json::Value::Null,
                        "reloaded": false,
                    })
                    .to_string(),
                ),
            }
        }
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
}

/// POST `/cli/mail/footer` `{text}` — `--domain` 400.
pub fn handle_footer_set(body: &[u8]) -> CliResponse {
    let b: FooterSetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if let Err(r) = domain_reserved(b.domain.as_deref()) {
        return r;
    }
    let text = b.text.as_deref().map(str::trim_end).unwrap_or("");
    if text.trim().is_empty() {
        return usage("missing 'text' — the outbound footer to append".to_string());
    }
    if let Err(h) = cap_text(text, "footer") {
        return err_json("400 Bad Request", "text_too_long", h);
    }
    if let Err(h) = reject_lone_dot_line(text, "footer") {
        return usage(h);
    }
    if let Err(h) = reject_html(text) {
        return usage(h);
    }
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let script = compose_footer_script(text);
    if let Err(e) = engine.system_script_put(FOOTER_SCRIPT_NAME, &script) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    let current = match engine.mta_stage_data_get_script() {
        Ok(v) => v,
        Err(_) => serde_json::json!({"else": "false"}),
    };
    let rewritten = jmap::rewrite_data_script_footer(&current, Some(FOOTER_SCRIPT_NAME));
    if let Err(e) = engine.mta_stage_data_set_script(rewritten) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "enabled": true,
            "text": text,
            "reloaded": false,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/footer/unset` `{}` — destroy `k2-footer` and reset
/// DATA `script` only if it still points at `k2-footer`.
pub fn handle_footer_unset(body: &[u8]) -> CliResponse {
    let b: FooterUnsetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if let Err(r) = domain_reserved(b.domain.as_deref()) {
        return r;
    }
    let engine = match engine() {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    if let Err(e) = engine.system_script_destroy(FOOTER_SCRIPT_NAME) {
        return err_json("502 Bad Gateway", "engine", e);
    }
    match engine.mta_stage_data_get_script() {
        Ok(current) => {
            if jmap::data_script_points_at_footer(&current, FOOTER_SCRIPT_NAME) {
                let rewritten = jmap::rewrite_data_script_footer(&current, None);
                if let Err(e) = engine.mta_stage_data_set_script(rewritten) {
                    return err_json("502 Bad Gateway", "engine", e);
                }
            }
        }
        Err(_) => {}
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "enabled": false,
            "text": serde_json::Value::Null,
            "reloaded": false,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_set_without_text_is_usage() {
        let r = handle_footer_set(br#"{}"#);
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("text"), "{}", r.body);
    }

    #[test]
    fn footer_set_domain_is_400() {
        let r = handle_footer_set(br#"{"text":"Acme","domain":"acme.test"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("domain") || r.body.contains("host-wide"),
            "{}",
            r.body
        );
    }

    #[test]
    fn footer_unset_domain_is_400() {
        let r = handle_footer_unset(br#"{"domain":"acme.test"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn footer_set_html_is_400() {
        let r = handle_footer_set(br#"{"text":"<html><body>nope</body></html>"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(
            r.body.contains("plain") || r.body.contains("HTML"),
            "{}",
            r.body
        );
    }

    #[test]
    fn footer_set_over_cap_is_text_too_long() {
        let text = "a".repeat(crate::mail::sieve_user::TEXT_CAP + 1);
        let body = serde_json::json!({ "text": text }).to_string();
        let r = handle_footer_set(body.as_bytes());
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        assert!(r.body.contains("text_too_long"), "{}", r.body);
        assert!(r.body.contains("4096"), "{}", r.body);
    }

    #[test]
    fn footer_set_lone_dot_is_400() {
        let r = handle_footer_set(br#"{"text":"hello\n.\nworld"}"#);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    }

    #[test]
    fn compose_footer_is_plain_foreverypart_contains_guard() {
        let src = compose_footer_script("Acme Legal — confidential.");
        assert!(src.contains("foreverypart"), "{src}");
        assert!(src.contains("text/plain"), "{src}");
        assert!(!src.contains("text/html"), "{src}");
        assert!(src.contains(":contains"), "{src}");
        assert!(src.contains("Acme Legal — confidential."), "{src}");
        assert_eq!(
            parse_footer_text(&src).as_deref(),
            Some("Acme Legal — confidential.")
        );
    }

    #[test]
    fn footer_get_without_server_is_not_ready() {
        let r = handle_footer_get(&HashMap::new());
        assert_ne!(r.status, "405 Method Not Allowed");
        assert!(
            r.status == "503 Service Unavailable" || r.status == "502 Bad Gateway",
            "{}",
            r.body
        );
    }
}
