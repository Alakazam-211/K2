//! Host-session capability envelope (PRD `prd-v1-host-session-capabilities-v1`
//! S1–S3 core): ES256 capability JWTs, JWKS publish, request validation,
//! `K2_CAPABILITY_TOKEN` packaging, and mode-0600 stage files.
//!
//! ## Surface
//!
//! - [`handle_v1_jwks`] — `GET /v1/jwks` public JWKS (no private material).
//! - [`parse_capabilities`] — validate spawn/resume `capabilities[]` entries.
//! - [`mint_and_stage`] / [`mint_and_stage_for_session`] — mint one JWT per
//!   capability, package the env payload, stage under
//!   `{workspace}/.k2/caps/<session_id>.json`, return non-secret metadata.
//!
//! Private key lives only on the daemon host at
//! `~/.k2/capability-signing.pem` (mode 0600), generated on first use.
//! **Pilot (0.40.76):** that key is STATIC (no rotation yet). Apps fetch
//! public material from `GET /v1/jwks` (API surface must be enabled + auth).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use p256::SecretKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cli_response::CliResponse;
use k2_core::log_debug;

/// Stable issuer claim for host-session capability JWTs.
pub const ISSUER: &str = "k2-host-sessions";

/// Env var name staged for the agent process (and documented in metadata).
pub const ENV_NAME: &str = "K2_CAPABILITY_TOKEN";

/// On-disk private key under the daemon state dir (`~/.k2/`).
const KEY_FILE_NAME: &str = "capability-signing.pem";

/// v1 kind allowlist (PRD §5.1).
const KIND_HTTP_CALLBACK: &str = "http_callback";

/// Resource max length (charset-constrained `interview:` + id).
const RESOURCE_MAX_LEN: usize = 128;

/// Allowed HTTP methods for `actions` (PRD §5.1).
const ALLOWED_ACTIONS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

/// Template placeholder permitted in `audience` (resolved at mint).
const RESOURCE_ID_PLACEHOLDER: &str = "{resource_id}";

// ── Public types ──────────────────────────────────────────────────────

/// One validated capability request entry from spawn/resume JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapRequest {
    pub kind: String,
    pub audience: String,
    pub resource: String,
    pub actions: Vec<String>,
}

/// Result of mint + package + stage (includes the secret env payload).
#[derive(Debug, Clone)]
pub struct MintOutcome {
    /// JSON array for `K2_CAPABILITY_TOKEN` (contains JWTs — secret).
    pub env_value: String,
    /// Non-secret response metadata (`staged`, `env`, `expires_at`, `jtis`).
    pub metadata: serde_json::Value,
    /// jtis minted (same as metadata.jtis).
    pub jtis: Vec<String>,
}

// ── JWT claims ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct CapClaims {
    iss: String,
    aud: String,
    sub: String,
    resource: String,
    actions: Vec<String>,
    exp: i64,
    iat: i64,
    jti: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ws: Option<String>,
}

// ── Signing material ──────────────────────────────────────────────────

/// PEM + derived public material. Cached by absolute path so `$HOME`
/// redirects in tests reload correctly (`OnceLock` would pin the first
/// sandbox forever). `EncodingKey` is not `Clone`, so we re-parse PEM.
struct CapMaterialOwned {
    path: PathBuf,
    pem: String,
    kid: String,
    jwk: serde_json::Value,
}

struct CapMaterial {
    encoding: EncodingKey,
    kid: String,
    jwk: serde_json::Value,
}

fn material_owned_cache() -> &'static Mutex<Option<CapMaterialOwned>> {
    static CACHE: Mutex<Option<CapMaterialOwned>> = Mutex::new(None);
    &CACHE
}

/// `~/.k2/capability-signing.pem` (via [`k2_core::paths::k2_home`]).
pub fn signing_key_path() -> PathBuf {
    k2_core::paths::k2_home().join(KEY_FILE_NAME)
}

#[cfg(unix)]
fn restrict_mode(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn restrict_mode(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn load_or_generate_material() -> Result<CapMaterial, String> {
    let path = signing_key_path();
    {
        let guard = material_owned_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ref m) = *guard {
            if m.path == path {
                let encoding = EncodingKey::from_ec_pem(m.pem.as_bytes()).map_err(|e| {
                    format!("parse capability signing key {}: {e}", path.display())
                })?;
                return Ok(CapMaterial {
                    encoding,
                    kid: m.kid.clone(),
                    jwk: m.jwk.clone(),
                });
            }
        }
    }

    let pem = if path.exists() {
        fs::read_to_string(&path)
            .map_err(|e| format!("read capability signing key {}: {e}", path.display()))?
    } else {
        generate_and_persist_key(&path)?
    };

    let (kid, jwk) = jwk_from_private_pem(&pem)?;
    let encoding = EncodingKey::from_ec_pem(pem.as_bytes())
        .map_err(|e| format!("parse capability signing key {}: {e}", path.display()))?;

    {
        let mut guard = material_owned_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(CapMaterialOwned {
            path,
            pem,
            kid: kid.clone(),
            jwk: jwk.clone(),
        });
    }

    Ok(CapMaterial {
        encoding,
        kid,
        jwk,
    })
}

fn generate_and_persist_key(path: &Path) -> Result<String, String> {
    let secret = random_p256_secret()?;
    let pem = secret
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| format!("encode capability signing key PKCS8 PEM: {e}"))?
        .to_string();

    let dir = path
        .parent()
        .ok_or_else(|| format!("capability key path has no parent: {}", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    // tmp + rename so a crash mid-write never leaves a half key.
    let tmp = dir.join(format!("{}.tmp.{}", KEY_FILE_NAME, std::process::id()));
    fs::write(&tmp, pem.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp)?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", path.display())
    })?;
    restrict_mode(path)?;
    log_debug!(
        "[v1_capabilities] generated ES256 capability signing key at {}",
        path.display()
    );
    Ok(pem)
}

/// CSPRNG P-256 secret via `getrandom` (already a daemon dep).
fn random_p256_secret() -> Result<SecretKey, String> {
    // Rejection sampling until the 32-byte seed is a valid scalar.
    for _ in 0..64 {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| format!("os rng unavailable for capability key: {e}"))?;
        if let Ok(sk) = SecretKey::from_slice(&bytes) {
            return Ok(sk);
        }
    }
    Err("failed to sample a valid P-256 secret key".into())
}

fn jwk_from_private_pem(pem: &str) -> Result<(String, serde_json::Value), String> {
    let secret = SecretKey::from_pkcs8_pem(pem)
        .map_err(|e| format!("parse capability private key for JWKS: {e}"))?;
    let public = secret.public_key();
    let point = public.to_encoded_point(false);
    let x = point
        .x()
        .ok_or_else(|| "P-256 public point missing x".to_string())?;
    let y = point
        .y()
        .ok_or_else(|| "P-256 public point missing y".to_string())?;
    let x_b64 = B64URL.encode(x.as_slice());
    let y_b64 = B64URL.encode(y.as_slice());

    // Stable kid = first 8 bytes of SHA-256(x||y) as lowercase hex.
    let mut hasher = Sha256::new();
    hasher.update(x.as_slice());
    hasher.update(y.as_slice());
    let digest = hasher.finalize();
    let kid = bytes_to_hex(&digest[..8]);

    let jwk = serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": x_b64,
        "y": y_b64,
        "use": "sig",
        "alg": "ES256",
        "kid": kid,
    });
    Ok((kid, jwk))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

// ── Public handlers / API ─────────────────────────────────────────────

/// `GET /v1/jwks` — public JWKS document. No secrets. Fail-closed 500 if the
/// signing key cannot be loaded/generated.
pub fn handle_v1_jwks() -> CliResponse {
    match public_jwks() {
        Ok(body) => CliResponse::ok_json(body.to_string()),
        Err(e) => {
            log_debug!("[v1_capabilities] jwks unavailable (fail-closed): {e}");
            CliResponse::internal_error("capability signing key unavailable")
        }
    }
}

/// Build the JWKS JSON document (`{ "keys": [ … ] }`).
pub fn public_jwks() -> Result<serde_json::Value, String> {
    let m = load_or_generate_material()?;
    Ok(serde_json::json!({ "keys": [m.jwk] }))
}

/// Validate a JSON `capabilities` array (spawn/resume body field).
///
/// Rules (PRD §5.1):
/// - must be a JSON array
/// - each entry: `kind` = `http_callback`
/// - `audience`: absolute `https://…` URL **or** URL template containing only
///   `{resource_id}` as a placeholder
/// - `resource`: non-empty, max length, charset `interview:` + alnum/`_`/`-`
/// - `actions`: non-empty subset of GET, POST, PUT, PATCH, DELETE
pub fn parse_capabilities(value: &serde_json::Value) -> Result<Vec<CapRequest>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "capabilities must be a JSON array".to_string())?;

    let mut out = Vec::with_capacity(arr.len());
    for (i, entry) in arr.iter().enumerate() {
        let obj = entry
            .as_object()
            .ok_or_else(|| format!("capabilities[{i}] must be an object"))?;

        let kind = obj
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("capabilities[{i}].kind is required"))?
            .trim();
        if kind != KIND_HTTP_CALLBACK {
            return Err(format!(
                "capabilities[{i}].kind must be \"{KIND_HTTP_CALLBACK}\" (got {kind:?})"
            ));
        }

        let audience = obj
            .get("audience")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("capabilities[{i}].audience is required"))?
            .trim()
            .to_string();
        validate_audience(&audience)
            .map_err(|e| format!("capabilities[{i}].audience: {e}"))?;

        let resource = obj
            .get("resource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("capabilities[{i}].resource is required"))?
            .trim()
            .to_string();
        validate_resource(&resource)
            .map_err(|e| format!("capabilities[{i}].resource: {e}"))?;

        let actions_val = obj
            .get("actions")
            .ok_or_else(|| format!("capabilities[{i}].actions is required"))?;
        let actions = parse_actions(actions_val)
            .map_err(|e| format!("capabilities[{i}].actions: {e}"))?;

        out.push(CapRequest {
            kind: KIND_HTTP_CALLBACK.to_string(),
            audience,
            resource,
            actions,
        });
    }
    Ok(out)
}

/// Mint one JWT per cap, package `K2_CAPABILITY_TOKEN`, stage mode-0600 file.
///
/// `exp_secs` is the lifetime from now (clamped to at least 1s). Optional
/// `principal` / `ws` claims are included when present and non-empty.
pub fn mint_and_stage(
    session_id: &str,
    workspace: &Path,
    caps: &[CapRequest],
    exp_secs: u64,
    principal: Option<&str>,
    ws: Option<&str>,
) -> Result<MintOutcome, String> {
    if session_id.trim().is_empty() {
        return Err("session_id is required".into());
    }
    if session_id.contains('/') || session_id.contains('\\') || session_id.contains("..") {
        return Err("session_id contains path-unsafe characters".into());
    }
    if caps.is_empty() {
        let metadata = serde_json::json!({
            "staged": false,
            "env": ENV_NAME,
            "expires_at": serde_json::Value::Null,
            "jtis": [],
        });
        return Ok(MintOutcome {
            env_value: "[]".to_string(),
            metadata,
            jtis: vec![],
        });
    }

    let material = load_or_generate_material()?;
    let now = Utc::now();
    let lifetime = ChronoDuration::seconds(exp_secs.max(1) as i64);
    let exp_dt = now + lifetime;
    let exp = exp_dt.timestamp();
    let iat = now.timestamp();
    let expires_at = exp_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let principal = principal
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let ws = ws
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut package: Vec<serde_json::Value> = Vec::with_capacity(caps.len());
    let mut jtis: Vec<String> = Vec::with_capacity(caps.len());

    for cap in caps {
        let aud = resolve_audience(&cap.audience, &cap.resource)?;
        let jti = uuid::Uuid::new_v4().to_string();
        let claims = CapClaims {
            iss: ISSUER.to_string(),
            aud,
            sub: session_id.to_string(),
            resource: cap.resource.clone(),
            actions: cap.actions.clone(),
            exp,
            iat,
            jti: jti.clone(),
            principal: principal.clone(),
            ws: ws.clone(),
        };

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(material.kid.clone());

        let token = encode(&header, &claims, &material.encoding)
            .map_err(|e| format!("sign capability JWT: {e}"))?;

        package.push(serde_json::json!({
            "actions": cap.actions,
            "token": token,
        }));
        jtis.push(jti);
    }

    let env_value = serde_json::to_string(&package)
        .map_err(|e| format!("serialize K2_CAPABILITY_TOKEN: {e}"))?;

    stage_file(workspace, session_id, &env_value)?;

    let metadata = serde_json::json!({
        "staged": true,
        "env": ENV_NAME,
        "expires_at": expires_at,
        "jtis": jtis,
    });

    let jtis_out: Vec<String> = metadata
        .get("jtis")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    Ok(MintOutcome {
        env_value,
        metadata,
        jtis: jtis_out,
    })
}

/// Route-facing helper: parse `capabilities` JSON, mint/stage, return
/// **non-secret** metadata `Value` for the spawn/resume response body.
///
/// `timeout_secs` becomes JWT lifetime (`min(timeout_secs, 3600)`).
pub fn mint_and_stage_for_session(
    caps_val: &serde_json::Value,
    session_id: &str,
    workspace: &str,
    ws_slug: &str,
    principal: &str,
    timeout_secs: u64,
) -> Result<serde_json::Value, String> {
    let caps = parse_capabilities(caps_val)?;
    // Align exp with idle budget but keep a short fixed ceiling (PRD §8).
    let exp_secs = timeout_secs.min(3600).max(1);
    let out = mint_and_stage(
        session_id,
        Path::new(workspace),
        &caps,
        exp_secs,
        Some(principal),
        Some(ws_slug),
    )?;
    Ok(out.metadata)
}

// ── Validation helpers ────────────────────────────────────────────────

fn validate_resource(resource: &str) -> Result<(), String> {
    if resource.is_empty() {
        return Err("must be non-empty".into());
    }
    if resource.len() > RESOURCE_MAX_LEN {
        return Err(format!("exceeds max length {RESOURCE_MAX_LEN}"));
    }
    // charset: interview: + alnum / _ / -
    let Some(id) = resource.strip_prefix("interview:") else {
        return Err("must start with \"interview:\"".into());
    };
    if id.is_empty() {
        return Err("interview id after prefix must be non-empty".into());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("interview id may only contain alnum, '_', '-'".into());
    }
    Ok(())
}

fn validate_audience(audience: &str) -> Result<(), String> {
    if audience.is_empty() {
        return Err("must be non-empty".into());
    }
    if audience.chars().any(|c| c.is_whitespace()) {
        return Err("must not contain whitespace".into());
    }

    // Only `{resource_id}` placeholder is allowed (any other `{…}` is reject).
    let mut rest = audience;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| "unclosed '{' in audience template".to_string())?;
        let name = &after[..end];
        if name != "resource_id" {
            return Err(format!(
                "only {{resource_id}} placeholder is allowed (found {{{name}}})"
            ));
        }
        rest = &after[end + 1..];
    }

    // Validate shape after resolving a dummy resource_id.
    let resolved = audience.replace(RESOURCE_ID_PLACEHOLDER, "x");
    validate_absolute_https(&resolved)
}

fn validate_absolute_https(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("must be an absolute https URL".into());
    }
    let rest = &url["https://".len()..];
    if rest.is_empty() {
        return Err("https URL missing host".into());
    }
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains(' ') {
        return Err("https URL missing host".into());
    }
    // Reject obvious non-absolute forms.
    if host.starts_with('.') || host.starts_with('/') {
        return Err("https URL host is invalid".into());
    }
    Ok(())
}

fn parse_actions(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "must be a non-empty array".to_string())?;
    if arr.is_empty() {
        return Err("must be non-empty".into());
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, a) in arr.iter().enumerate() {
        let s = a
            .as_str()
            .ok_or_else(|| format!("[{i}] must be a string"))?
            .trim();
        if s.is_empty() {
            return Err(format!("[{i}] is empty"));
        }
        if !ALLOWED_ACTIONS.contains(&s) {
            return Err(format!(
                "[{i}] {s:?} is not an allowed action (GET|POST|PUT|PATCH|DELETE)"
            ));
        }
        if out.iter().any(|x: &String| x == s) {
            return Err(format!("duplicate action {s}"));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

/// Resolve `{resource_id}` from `resource` (`interview:<id>` → `<id>`).
fn resolve_audience(audience: &str, resource: &str) -> Result<String, String> {
    let resource_id = resource.strip_prefix("interview:").unwrap_or(resource);
    let resolved = audience.replace(RESOURCE_ID_PLACEHOLDER, resource_id);
    validate_absolute_https(&resolved)?;
    Ok(resolved)
}

/// Atomic stage: write temp + `rename()` on the same filesystem so a concurrent
/// agent re-read never sees a torn/partial JSON (k2-dev-web 0.40.76 flag).
fn stage_file(workspace: &Path, session_id: &str, env_value: &str) -> Result<(), String> {
    let caps_dir = workspace.join(".k2").join("caps");
    fs::create_dir_all(&caps_dir).map_err(|e| format!("mkdir {}: {e}", caps_dir.display()))?;
    // Restrict the caps directory too (best-effort).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&caps_dir, fs::Permissions::from_mode(0o700));
        let _ = fs::set_permissions(workspace.join(".k2"), fs::Permissions::from_mode(0o700));
    }

    let dest = caps_dir.join(format!("{session_id}.json"));
    // Unique temp name avoids clobber races between concurrent remints.
    let tmp = caps_dir.join(format!(
        "{session_id}.json.tmp.{}.{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    fs::write(&tmp, env_value.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp)?;
    fs::rename(&tmp, &dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", dest.display())
    })?;
    restrict_mode(&dest)?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};

    fn sample_cap(actions: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "kind": "http_callback",
            "audience": "https://scout.example/api/v1/interviews/{resource_id}/phase1",
            "resource": "interview:ivw_abc",
            "actions": actions,
        })
    }

    #[test]
    fn parse_accepts_valid_http_callback() {
        let v = serde_json::json!([sample_cap(&["GET", "POST"])]);
        let caps = parse_capabilities(&v).expect("valid");
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].kind, "http_callback");
        assert_eq!(caps[0].resource, "interview:ivw_abc");
        assert_eq!(caps[0].actions, vec!["GET", "POST"]);
    }

    #[test]
    fn parse_rejects_bad_kind() {
        let mut c = sample_cap(&["GET"]);
        c["kind"] = serde_json::json!("ssh");
        let err = parse_capabilities(&serde_json::json!([c])).unwrap_err();
        assert!(err.contains("http_callback"), "{err}");
    }

    #[test]
    fn parse_rejects_http_audience() {
        let mut c = sample_cap(&["GET"]);
        c["audience"] = serde_json::json!("http://insecure.example/x");
        let err = parse_capabilities(&serde_json::json!([c])).unwrap_err();
        assert!(err.contains("https"), "{err}");
    }

    #[test]
    fn parse_rejects_unknown_placeholder() {
        let mut c = sample_cap(&["GET"]);
        c["audience"] = serde_json::json!("https://scout.example/{other}/x");
        let err = parse_capabilities(&serde_json::json!([c])).unwrap_err();
        assert!(err.contains("resource_id"), "{err}");
    }

    #[test]
    fn parse_rejects_bad_resource_charset() {
        let mut c = sample_cap(&["GET"]);
        c["resource"] = serde_json::json!("interview:ivw/../x");
        let err = parse_capabilities(&serde_json::json!([c])).unwrap_err();
        assert!(err.contains("alnum") || err.contains("interview"), "{err}");
    }

    #[test]
    fn parse_rejects_empty_actions() {
        let mut c = sample_cap(&["GET"]);
        c["actions"] = serde_json::json!([]);
        let err = parse_capabilities(&serde_json::json!([c])).unwrap_err();
        assert!(err.contains("non-empty"), "{err}");
    }

    #[test]
    fn parse_rejects_unknown_action() {
        let mut c = sample_cap(&["GET"]);
        c["actions"] = serde_json::json!(["TRACE"]);
        let err = parse_capabilities(&serde_json::json!([c])).unwrap_err();
        assert!(err.contains("TRACE") || err.contains("allowed"), "{err}");
    }

    #[test]
    fn parse_rejects_non_array() {
        let err = parse_capabilities(&serde_json::json!({"kind": "http_callback"})).unwrap_err();
        assert!(err.contains("array"), "{err}");
    }

    #[test]
    fn mint_verify_roundtrip_with_jwks() {
        // Isolate ~/.k2 so we never touch the developer's real key.
        let _home = crate::test_support::TempHome::new();

        let mut post_cap = sample_cap(&["POST"]);
        post_cap["audience"] = serde_json::json!(
            "https://scout.example/api/v1/interviews/{resource_id}/results"
        );
        let caps = parse_capabilities(&serde_json::json!([
            sample_cap(&["GET"]),
            post_cap
        ]))
        .expect("parse");

        let workspace = _home.path().join("ws");
        fs::create_dir_all(&workspace).expect("ws");

        let out = mint_and_stage(
            "sess-abc-123",
            &workspace,
            &caps,
            600,
            Some("principal-1"),
            Some("my-ws"),
        )
        .expect("mint");

        assert!(out.metadata["staged"].as_bool().unwrap());
        assert_eq!(out.metadata["env"].as_str().unwrap(), ENV_NAME);
        assert_eq!(out.jtis.len(), 2);
        assert!(out.metadata["expires_at"].as_str().is_some());

        // Stage file exists, mode 0600, payload matches env_value.
        let staged_path = workspace.join(".k2/caps/sess-abc-123.json");
        assert!(staged_path.is_file(), "stage file missing: {staged_path:?}");
        let staged = fs::read_to_string(&staged_path).expect("read stage");
        assert_eq!(staged, out.env_value);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&staged_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "stage file must be 0600, got {mode:o}");
        }

        // Package shape.
        let package: Vec<serde_json::Value> =
            serde_json::from_str(&out.env_value).expect("package json");
        assert_eq!(package.len(), 2);
        assert_eq!(package[0]["actions"], serde_json::json!(["GET"]));
        assert_eq!(package[1]["actions"], serde_json::json!(["POST"]));

        // JWKS + verify each JWT.
        let jwks = public_jwks().expect("jwks");
        let key = &jwks["keys"][0];
        assert_eq!(key["kty"], "EC");
        assert_eq!(key["crv"], "P-256");
        assert_eq!(key["alg"], "ES256");
        assert_eq!(key["use"], "sig");
        let kid = key["kid"].as_str().expect("kid");
        let x = key["x"].as_str().expect("x");
        let y = key["y"].as_str().expect("y");

        let decoding = DecodingKey::from_ec_components(x, y).expect("decoding key from jwks");
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_issuer(&[ISSUER]);
        validation.set_required_spec_claims(&["exp", "sub", "iss"]);
        // aud is a free-form URL; disable default aud check, assert manually.
        validation.validate_aud = false;

        for (i, item) in package.iter().enumerate() {
            let token = item["token"].as_str().expect("token");
            let header = decode_header(token).expect("header");
            assert_eq!(header.alg, Algorithm::ES256);
            assert_eq!(header.kid.as_deref(), Some(kid));

            let data = decode::<CapClaims>(token, &decoding, &validation).expect("verify");
            assert_eq!(data.claims.iss, ISSUER);
            assert_eq!(data.claims.sub, "sess-abc-123");
            assert_eq!(data.claims.resource, "interview:ivw_abc");
            assert_eq!(data.claims.jti, out.jtis[i]);
            assert_eq!(data.claims.principal.as_deref(), Some("principal-1"));
            assert_eq!(data.claims.ws.as_deref(), Some("my-ws"));
            // Template resolved: {resource_id} → ivw_abc
            assert!(
                data.claims.aud.contains("ivw_abc"),
                "aud should resolve resource_id: {}",
                data.claims.aud
            );
            assert!(!data.claims.aud.contains("{resource_id}"));
        }

        // Key file mode 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let key_path = signing_key_path();
            assert!(key_path.is_file());
            let mode = fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "signing key must be 0600, got {mode:o}");
        }
    }

    #[test]
    fn mint_and_stage_for_session_returns_metadata_only() {
        let _home = crate::test_support::TempHome::new();
        let workspace = _home.path().join("ws2");
        fs::create_dir_all(&workspace).expect("ws");

        let caps = serde_json::json!([sample_cap(&["GET"])]);
        let meta = mint_and_stage_for_session(
            &caps,
            "sid-xyz",
            workspace.to_str().unwrap(),
            "slug-a",
            "user-1",
            180,
        )
        .expect("mint");

        assert_eq!(meta["staged"], true);
        assert_eq!(meta["env"], ENV_NAME);
        assert!(meta.get("token").is_none(), "metadata must not leak token");
        assert!(meta["jtis"].as_array().unwrap().len() == 1);

        // handle_v1_jwks is pure-ish (uses key material) and returns 200.
        let resp = handle_v1_jwks();
        assert_eq!(resp.status, "200 OK");
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("jwks body");
        assert!(body["keys"].as_array().unwrap().len() >= 1);
    }

    #[test]
    fn resolve_audience_strips_interview_prefix() {
        let resolved =
            resolve_audience("https://x.example/r/{resource_id}/z", "interview:ivw_99").unwrap();
        assert_eq!(resolved, "https://x.example/r/ivw_99/z");
    }

    /// Atomic remint: second mint for the same session_id replaces stage-file
    /// content entirely (no torn merge). File is a valid JSON array of
    /// `{actions, token}` entries (k2-dev-web 0.40.76 integrator contract).
    #[test]
    fn stage_file_remint_replaces_content() {
        let _home = crate::test_support::TempHome::new();
        let workspace = _home.path().join("ws-remint");
        fs::create_dir_all(&workspace).expect("ws");

        let sid = "sess-remint-1";
        let caps_get = parse_capabilities(&serde_json::json!([sample_cap(&["GET"])]))
            .expect("parse get");
        let caps_post = parse_capabilities(&serde_json::json!([sample_cap(&["POST"])]))
            .expect("parse post");

        let first = mint_and_stage(sid, &workspace, &caps_get, 120, None, None).expect("mint1");
        let staged_path = workspace.join(".k2/caps").join(format!("{sid}.json"));
        assert!(staged_path.is_file(), "stage file after first mint");
        let after_first = fs::read_to_string(&staged_path).expect("read1");
        assert_eq!(after_first, first.env_value, "disk must match first env_value");

        let second = mint_and_stage(sid, &workspace, &caps_post, 120, None, None).expect("mint2");
        let after_second = fs::read_to_string(&staged_path).expect("read2");
        assert_eq!(
            after_second, second.env_value,
            "second remint must replace stage file content"
        );
        assert_ne!(
            after_second, first.env_value,
            "remint must not leave first mint payload"
        );
        assert_ne!(
            first.jtis, second.jtis,
            "remint mints fresh jtis (not reuse)"
        );

        // Valid JSON array; each entry has actions + token (non-empty JWT).
        let package: Vec<serde_json::Value> =
            serde_json::from_str(&after_second).expect("stage file must be valid JSON array");
        assert_eq!(package.len(), 1, "one capability staged");
        let entry = &package[0];
        assert!(
            entry.get("actions").and_then(|a| a.as_array()).is_some(),
            "entry must have actions array: {entry}"
        );
        assert_eq!(entry["actions"], serde_json::json!(["POST"]));
        let token = entry
            .get("token")
            .and_then(|t| t.as_str())
            .expect("entry must have token string");
        assert!(
            token.split('.').count() == 3 && !token.is_empty(),
            "token must look like a JWT (3 segments): {token}"
        );
        // First mint's GET action must not leak into the replaced file.
        assert_ne!(entry["actions"], serde_json::json!(["GET"]));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&staged_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "reminted stage file must stay 0600, got {mode:o}");
        }
    }
}
