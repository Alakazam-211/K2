//! Federated tray packages — cover `.md` + small attachments on the same
//! sealed-envelope plane as live `k2 msg agent::host`.
//!
//! Bytes travel inside [`crate::awareness::SignalKind::Custom`] (`kind` =
//! [`TRAY_SIGNAL_KIND`]). The receive side writes the peer workspace's
//! `.k2/inbox/` and (optional) knocks with a short pointer — never dumps
//! file contents into the live PTY.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::inbox::sanitize_sidecar_filename;

/// Inner `SignalKind::Custom.kind` for a tray package.
pub const TRAY_SIGNAL_KIND: &str = "inbox-tray";

/// v1 decoded-bytes cap for one federated tray (cover + attachments).
/// Fail loud above this; larger files need Connect upload/chunk.
pub const TRAY_MAX_BYTES: u64 = 1024 * 1024;

/// Wire file inside a tray Custom payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayFile {
    pub name: String,
    #[serde(rename = "bytes_b64", alias = "base64")]
    pub bytes_b64: String,
}

/// Wire tray spec (Custom payload or `/cli/federation/send` `tray` field).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TraySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub wake: bool,
    #[serde(default)]
    pub files: Vec<TrayFile>,
}

/// Decoded tray ready to land via [`crate::inbox::deliver_named_bytes`].
#[derive(Debug, Clone)]
pub struct DecodedTray {
    pub title: Option<String>,
    pub body: Option<String>,
    pub wake: bool,
    pub files: Vec<(String, Vec<u8>)>,
}

/// Total decoded payload size.
pub fn decoded_size(files: &[(String, Vec<u8>)]) -> u64 {
    files.iter().map(|(_, b)| b.len() as u64).sum()
}

/// Decode a [`TraySpec`], rejecting empty packages, unsafe names, and
/// payloads over [`TRAY_MAX_BYTES`].
pub fn decode_spec(spec: &TraySpec) -> Result<DecodedTray, String> {
    if spec.files.is_empty() {
        return Err("tray package has no files".to_string());
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(spec.files.len());
    let mut used: Vec<String> = Vec::new();
    for f in &spec.files {
        let name = sanitize_sidecar_filename(&f.name);
        if name.is_empty() {
            return Err("tray file has an empty name".to_string());
        }
        let unique = unique_name(&name, &used);
        used.push(unique.clone());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(f.bytes_b64.trim())
            .map_err(|e| format!("tray file '{unique}' is not valid base64: {e}"))?;
        files.push((unique, bytes));
    }
    let size = decoded_size(&files);
    if size > TRAY_MAX_BYTES {
        return Err(format!(
            "federated tray package is {size} bytes (v1 cap {TRAY_MAX_BYTES}). \
             Cover .md + small attachments only. For larger files use K2 Connect \
             upload/chunk, or split the package."
        ));
    }
    let title = spec
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let body = spec
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(DecodedTray {
        title,
        body,
        wake: spec.wake,
        files,
    })
}

/// Parse a Custom-signal JSON payload into a decoded tray.
pub fn parse_payload(payload: &serde_json::Value) -> Result<DecodedTray, String> {
    let spec: TraySpec = serde_json::from_value(payload.clone())
        .map_err(|e| format!("inbox-tray payload: {e}"))?;
    decode_spec(&spec)
}

/// Encode decoded files back to a [`TraySpec`] (send path).
pub fn encode_spec(
    title: Option<&str>,
    body: Option<&str>,
    wake: bool,
    files: &[(String, Vec<u8>)],
) -> Result<TraySpec, String> {
    let size = decoded_size(files);
    if files.is_empty() {
        return Err("tray package has no files".to_string());
    }
    if size > TRAY_MAX_BYTES {
        return Err(format!(
            "federated tray package is {size} bytes (v1 cap {TRAY_MAX_BYTES}). \
             Cover .md + small attachments only. For larger files use K2 Connect \
             upload/chunk, or split the package."
        ));
    }
    Ok(TraySpec {
        title: title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        body: body
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        wake,
        files: files
            .iter()
            .map(|(name, bytes)| TrayFile {
                name: sanitize_sidecar_filename(name),
                bytes_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
            })
            .collect(),
    })
}

fn unique_name(base: &str, used: &[String]) -> String {
    if !used.iter().any(|u| u == base) {
        return base.to_string();
    }
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() && !e.contains('/') => {
            (s.to_string(), Some(e.to_string()))
        }
        _ => (base.to_string(), None),
    };
    let mut n = 2;
    loop {
        let candidate = match &ext {
            Some(e) => format!("{stem}-{n}.{e}"),
            None => format!("{stem}-{n}"),
        };
        if !used.iter().any(|u| u == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let files = vec![
            ("brief.md".to_string(), b"# hi\n".to_vec()),
            ("note.txt".to_string(), b"n".to_vec()),
        ];
        let spec = encode_spec(Some("Hello"), Some("please review"), true, &files).unwrap();
        assert!(spec.wake);
        let decoded = decode_spec(&spec).unwrap();
        assert_eq!(decoded.title.as_deref(), Some("Hello"));
        assert_eq!(decoded.body.as_deref(), Some("please review"));
        assert_eq!(decoded.files.len(), 2);
        assert_eq!(decoded.files[0].0, "brief.md");
        assert_eq!(decoded.files[0].1, b"# hi\n");
    }

    #[test]
    fn over_cap_fails_loud() {
        let big = vec![("big.bin".to_string(), vec![0u8; (TRAY_MAX_BYTES as usize) + 1])];
        let err = encode_spec(None, None, false, &big).unwrap_err();
        assert!(err.contains("v1 cap"), "{err}");
        assert!(err.contains("Connect") || err.contains("chunk"), "{err}");
    }

    #[test]
    fn empty_files_fail() {
        let err = encode_spec(Some("t"), None, false, &[]).unwrap_err();
        assert!(err.contains("no files"), "{err}");
    }

    #[test]
    fn path_components_sanitized() {
        let spec = TraySpec {
            files: vec![TrayFile {
                name: "../etc/passwd".into(),
                bytes_b64: base64::engine::general_purpose::STANDARD.encode(b"x"),
            }],
            ..TraySpec::default()
        };
        let decoded = decode_spec(&spec).unwrap();
        assert_eq!(decoded.files[0].0, "passwd");
        assert!(!decoded.files[0].0.contains('/'));
        assert!(!decoded.files[0].0.contains(".."));
    }
}
