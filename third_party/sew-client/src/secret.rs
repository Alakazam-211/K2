//! `SecretString`: a string whose `Debug` never shows it (S25 §16.1).

use std::fmt;
use std::ops::Deref;

/// A token, pass or password. `Debug` prints `SecretString(***)`; there is no `Display`, so a
/// secret cannot reach a log line through `{}` by accident. [`SecretString::expose`] (or
/// `Deref<Target = str>`) reads it where it must be sent.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret.
    pub fn new(s: impl Into<String>) -> SecretString {
        SecretString(s.into())
    }

    /// The secret text.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The secret text, owned.
    pub fn into_inner(self) -> String {
        self.0
    }

    /// Whether the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

impl Deref for SecretString {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        SecretString(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        SecretString(s.to_owned())
    }
}

impl From<&String> for SecretString {
    fn from(s: &String) -> Self {
        SecretString(s.clone())
    }
}

impl From<&SecretString> for SecretString {
    fn from(s: &SecretString) -> Self {
        s.clone()
    }
}

/// `…?exp=…&sig=***`: a signed URL with its signature cut (S25 §7).
pub fn redact_url(url: &str) -> String {
    let Some((head, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let parts: Vec<String> = query
        .split('&')
        .map(|kv| match kv.split_once('=') {
            Some((k, _)) if is_secret_param(k) => format!("{k}=***"),
            _ => kv.to_owned(),
        })
        .collect();
    format!("{head}?{}", parts.join("&"))
}

fn is_secret_param(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "sig" | "signature" | "token" | "ticket" | "pass"
    )
}

/// Cuts secrets out of free text: `sews_…`, `sewtk_…`, `sewsvc_…`, `sewpw_…` tokens, JWT-shaped
/// passes and `sig=` values.
pub(crate) fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&scrub_word(word));
    }
    out
}

fn scrub_word(w: &str) -> String {
    for prefix in ["sews_", "sewtk_", "sewsvc_", "sewpw_"] {
        if let Some(at) = w.find(prefix) {
            return format!("{}{prefix}***", &w[..at]);
        }
    }
    if w.contains("sig=") {
        return redact_url(w);
    }
    // A JWT: three base64url segments, the first starting with `eyJ`.
    if let Some(at) = w.find("eyJ")
        && w[at..].matches('.').count() >= 2
    {
        return format!("{}<jwt>", &w[..at]);
    }
    w.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_redaction() {
        let s = SecretString::from("sews_abc.def");
        assert_eq!(format!("{s:?}"), "SecretString(***)");
        assert_eq!(s.expose(), "sews_abc.def");
        assert_eq!(
            redact_url("https://h/api/v1/files/1?exp=10&sig=abcdef"),
            "https://h/api/v1/files/1?exp=10&sig=***"
        );
        assert_eq!(redact_url("https://h/x"), "https://h/x");
        assert_eq!(
            scrub("bearer sews_abc.def failed"),
            "bearer sews_*** failed"
        );
        assert_eq!(scrub("pass eyJa.eyJb.c bad"), "pass <jwt> bad");
    }
}
