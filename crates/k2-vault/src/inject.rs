//! Sealed injection (VS3) and the key-format registry (VS12).
//!
//! The secret's creator fixes **where** the value goes in an outgoing
//! request (`dst`) and **how** it's written (`fmt`). An agent calling
//! `k2 vault fetch` names the secret; the daemon applies `inject`. The
//! agent may pick only from the sealed `allowed_dst` / `allowed_fmt`.
//!
//! Presets for common key formats fill this in automatically, so a person
//! answering a Thread card just confirms.

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Dst {
    Header(String),
    Query(String),
    /// `Authorization: Basic base64(fmt(value))`.
    Basic,
    /// Replace `{{secret}}` in the request body. Only when a person set it.
    Body,
}

impl Dst {
    pub fn parse(s: &str) -> Result<Dst> {
        let s = s.trim();
        if let Some(h) = s.strip_prefix("header:") {
            validate_header_name(h)?;
            if is_forbidden_header(h) {
                return Err(VaultError::bad_request("bad_inject", format!("can't inject into {h}")));
            }
            return Ok(Dst::Header(canonical_header(h)));
        }
        if let Some(q) = s.strip_prefix("query:") {
            if q.is_empty() || q.len() > 64 || !q.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
                return Err(VaultError::bad_request("bad_inject", "query parameter names use letters, digits, _ - ."));
            }
            return Ok(Dst::Query(q.to_string()));
        }
        match s {
            "basic" => Ok(Dst::Basic),
            "body" => Ok(Dst::Body),
            _ => Err(VaultError::bad_request("bad_inject", "dst is header:<Name>, query:<param>, basic or body")),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Dst::Header(h) => format!("header:{h}"),
            Dst::Query(q) => format!("query:{q}"),
            Dst::Basic => "basic".into(),
            Dst::Body => "body".into(),
        }
    }
}

impl Serialize for Dst {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.render())
    }
}

impl<'de> Deserialize<'de> for Dst {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Dst, D::Error> {
        let s = String::deserialize(d)?;
        Dst::parse(&s).map_err(|e| serde::de::Error::custom(e.hint))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inject {
    pub dst: Dst,
    /// Exactly one `{}` where the value goes.
    pub fmt: String,
    #[serde(default, rename = "allowedDst", skip_serializing_if = "Vec::is_empty")]
    pub allowed_dst: Vec<Dst>,
    #[serde(default, rename = "allowedFmt", skip_serializing_if = "Vec::is_empty")]
    pub allowed_fmt: Vec<String>,
}

impl Default for Inject {
    fn default() -> Self {
        Inject::bearer()
    }
}

impl Inject {
    pub fn bearer() -> Inject {
        Inject { dst: Dst::Header("Authorization".into()), fmt: "Bearer {}".into(), allowed_dst: vec![], allowed_fmt: vec![] }
    }

    pub fn header(name: &str, fmt: &str) -> Inject {
        Inject { dst: Dst::Header(canonical_header(name)), fmt: fmt.into(), allowed_dst: vec![], allowed_fmt: vec![] }
    }

    pub fn validate(&self) -> Result<()> {
        for f in std::iter::once(&self.fmt).chain(self.allowed_fmt.iter()) {
            if f.matches("{}").count() != 1 {
                return Err(VaultError::bad_request("bad_inject", "fmt must contain exactly one {}"));
            }
            if f.len() > 128 || f.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
                return Err(VaultError::bad_request("bad_inject", "fmt is one short line"));
            }
        }
        Ok(())
    }

    /// Does this policy put the value in a request body?
    pub fn uses_body(&self) -> bool {
        self.dst == Dst::Body || self.allowed_dst.contains(&Dst::Body)
    }

    /// Pick the dst/fmt for one request: the defaults, or an agent's choice
    /// from the sealed lists.
    pub fn choose(&self, dst: Option<&str>, fmt: Option<&str>) -> Result<(Dst, String)> {
        let d = match dst {
            None => self.dst.clone(),
            Some(s) => {
                let d = Dst::parse(s)?;
                if d != self.dst && !self.allowed_dst.contains(&d) {
                    return Err(VaultError::forbidden(
                        "inject_not_allowed",
                        "this secret's creator didn't allow that destination",
                    ));
                }
                d
            }
        };
        let f = match fmt {
            None => self.fmt.clone(),
            Some(s) => {
                if s != self.fmt && !self.allowed_fmt.iter().any(|x| x == s) {
                    return Err(VaultError::forbidden(
                        "inject_not_allowed",
                        "this secret's creator didn't allow that format",
                    ));
                }
                s.to_string()
            }
        };
        Ok((d, f))
    }
}

/// RFC 9110 token characters.
pub fn validate_header_name(h: &str) -> Result<()> {
    let ok = !h.is_empty()
        && h.len() <= 64
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(VaultError::bad_request("header_not_allowed", format!("bad header name: {h}")))
    }
}

/// Headers an agent may never set and the vault never injects into
/// (VS5): routing, framing, hop-by-hop.
pub fn is_forbidden_header(h: &str) -> bool {
    let l = h.to_ascii_lowercase();
    matches!(
        l.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "proxy-connection"
            | "expect"
            | "accept-encoding"
    ) || l.starts_with(':')
        || l.starts_with("proxy-")
}

fn canonical_header(h: &str) -> String {
    h.split('-')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// A known key format (VS12) and its provider defaults.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    #[serde(skip)]
    pub prefixes: &'static [&'static str],
    /// Exact hosts. Empty = the person enters the project's own host.
    pub hosts: &'static [&'static str],
    pub dst: &'static str,
    pub fmt: &'static str,
}

impl Preset {
    pub fn inject(&self) -> Inject {
        Inject {
            dst: Dst::parse(self.dst).expect("preset dst is valid"),
            fmt: self.fmt.to_string(),
            allowed_dst: vec![],
            allowed_fmt: vec![],
        }
    }

    pub fn host_list(&self) -> Vec<String> {
        self.hosts.iter().map(|h| h.to_string()).collect()
    }
}

/// Order matters: longer, more specific prefixes first.
pub const PRESETS: &[Preset] = &[
    Preset { id: "anthropic", label: "Anthropic", prefixes: &["sk-ant-"], hosts: &["https://api.anthropic.com:443"], dst: "header:x-api-key", fmt: "{}" },
    Preset { id: "stripe", label: "Stripe", prefixes: &["sk_live_", "rk_live_", "sk_test_", "rk_test_"], hosts: &["https://api.stripe.com:443"], dst: "header:Authorization", fmt: "Bearer {}" },
    Preset { id: "openai", label: "OpenAI", prefixes: &["sk-proj-", "sk-svcacct-", "sk-admin-", "sk-"], hosts: &["https://api.openai.com:443"], dst: "header:Authorization", fmt: "Bearer {}" },
    Preset { id: "xai", label: "xAI", prefixes: &["xai-"], hosts: &["https://api.x.ai:443"], dst: "header:Authorization", fmt: "Bearer {}" },
    Preset { id: "gemini", label: "Google Gemini", prefixes: &["AIza"], hosts: &["https://generativelanguage.googleapis.com:443"], dst: "header:x-goog-api-key", fmt: "{}" },
    Preset { id: "github", label: "GitHub", prefixes: &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_"], hosts: &["https://api.github.com:443"], dst: "header:Authorization", fmt: "Bearer {}" },
    Preset { id: "slack", label: "Slack", prefixes: &["xoxb-", "xoxp-"], hosts: &["https://slack.com:443"], dst: "header:Authorization", fmt: "Bearer {}" },
    Preset { id: "supabase", label: "Supabase", prefixes: &["sb_secret_"], hosts: &[], dst: "header:apikey", fmt: "{}" },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Which provider a pasted value looks like (by prefix).
pub fn detect(value: &[u8]) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.prefixes.iter().any(|pre| value.starts_with(pre.as_bytes())))
}

/// The card check (VS12): the value looks like `<provider>` but the
/// proposed hosts don't include that provider's host. Returns the preset
/// when there's a mismatch.
pub fn host_mismatch(value: &[u8], proposed_hosts: &[String]) -> Option<&'static Preset> {
    let p = detect(value)?;
    if p.hosts.is_empty() || proposed_hosts.is_empty() {
        return None;
    }
    if proposed_hosts.iter().all(|h| p.hosts.contains(&h.as_str())) {
        None
    } else {
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dst_parse_and_forbidden_headers() {
        assert_eq!(Dst::parse("header:authorization").expect("ok"), Dst::Header("Authorization".into()));
        assert_eq!(Dst::parse("header:x-api-key").expect("ok").render(), "header:X-Api-Key");
        for bad in ["header:Host", "header:Transfer-Encoding", "header:Content-Length", "header:", "header:a b", "nope", "query:a&b"] {
            assert!(Dst::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn choose_only_from_sealed_lists() {
        let i = Inject::bearer();
        assert_eq!(i.choose(None, None).expect("defaults"), (Dst::Header("Authorization".into()), "Bearer {}".into()));
        assert_eq!(i.choose(Some("query:key"), None).expect_err("not allowed").code, "inject_not_allowed");
        assert_eq!(i.choose(None, Some("{}")).expect_err("not allowed").code, "inject_not_allowed");
        let mut j = Inject::bearer();
        j.allowed_dst = vec![Dst::Query("key".into())];
        assert_eq!(j.choose(Some("query:key"), None).expect("allowed").0, Dst::Query("key".into()));
    }

    #[test]
    fn fmt_needs_one_slot() {
        let mut i = Inject::bearer();
        i.validate().expect("ok");
        i.fmt = "Bearer".into();
        assert!(i.validate().is_err());
        i.fmt = "{}{}".into();
        assert!(i.validate().is_err());
        i.fmt = "a {}\n".into();
        assert!(i.validate().is_err());
    }

    #[test]
    fn registry() {
        // Every prefix detects its own provider. Values are built from the
        // table at run time, so no key-shaped literal sits in the source.
        for p in PRESETS {
            for pre in p.prefixes {
                let v = [pre.as_bytes(), b"x1"].concat();
                assert_eq!(detect(&v).expect("detected").id, p.id, "{pre}");
            }
        }
        assert!(detect(b"hunter2").is_none());
        assert!(detect(b"k2test_fake_key").is_none());
        for p in PRESETS {
            p.inject().validate().expect("preset inject valid");
            for h in p.hosts {
                let n = crate::hosts::OriginPattern::parse(h, false).expect("preset host valid").render();
                assert_eq!(&n, h, "preset hosts are stored normalised");
            }
        }
    }

    #[test]
    fn mismatch_check() {
        let stripe = vec!["https://api.stripe.com:443".to_string()];
        let key = [preset("stripe").expect("stripe").prefixes[0].as_bytes(), b"x"].concat();
        assert!(host_mismatch(&key, &stripe).is_none());
        let off = vec!["https://api-stripe.example:443".to_string()];
        assert_eq!(host_mismatch(&key, &off).expect("mismatch").id, "stripe");
        assert!(host_mismatch(b"plain", &off).is_none());
    }
}
