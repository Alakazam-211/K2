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
    /// A signer fixed by the creator (PRD vault-gaps item 2): K2 computes
    /// headers from the value at send time instead of pasting it; `dst`
    /// and `fmt` then aren't used. Optional, so a secret without one
    /// serialises byte-identically to before, and an older daemon's serde
    /// ignores the field (R1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign: Option<Sign>,
}

/// Signature encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Enc {
    #[default]
    Hex,
    Base64,
}

impl Enc {
    pub fn parse(s: &str) -> Result<Enc> {
        match s.trim() {
            "hex" => Ok(Enc::Hex),
            "base64" => Ok(Enc::Base64),
            _ => Err(VaultError::bad_request("bad_inject", "the signature encoding is hex or base64")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Enc::Hex => "hex",
            Enc::Base64 => "base64",
        }
    }
}

pub const DEFAULT_TS_TEMPLATE: &str = "{ts}.{body}";
pub const DEFAULT_SIG_VALUE: &str = "{sig}";

/// How K2 signs a request with the secret. Future schemes (not specified):
/// AWS SigV4, Google service-account JWT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scheme")]
pub enum Sign {
    /// OVH API v1 (`$1$` + SHA-1). The value is JSON:
    /// `{"applicationKey","applicationSecret","consumerKey"}`.
    #[serde(rename = "ovh")]
    Ovh,
    /// `header: prefix + enc(HMAC-SHA256(key, body))`.
    #[serde(rename = "hmac-sha256")]
    HmacSha256 {
        header: String,
        #[serde(default)]
        encoding: Enc,
        #[serde(default)]
        prefix: String,
    },
    /// `header: value_fmt{sig = enc(HMAC-SHA256(key, template{ts, body})), ts}`,
    /// plus `ts_header: ts` when set. `ts` is Unix seconds.
    #[serde(rename = "hmac-sha256-ts")]
    HmacSha256Ts {
        header: String,
        #[serde(default, rename = "tsHeader", skip_serializing_if = "Option::is_none")]
        ts_header: Option<String>,
        template: String,
        #[serde(rename = "valueFmt")]
        value_fmt: String,
        #[serde(default)]
        encoding: Enc,
    },
}

/// The headers OVH signing sets.
pub const OVH_HEADERS: &[&str] = &["X-Ovh-Application", "X-Ovh-Consumer", "X-Ovh-Timestamp", "X-Ovh-Signature"];

/// The parts of an OVH secret's value.
pub struct OvhKeys {
    pub application_key: String,
    pub application_secret: String,
    pub consumer_key: String,
}

impl Sign {
    pub fn scheme(&self) -> &'static str {
        match self {
            Sign::Ovh => "ovh",
            Sign::HmacSha256 { .. } => "hmac-sha256",
            Sign::HmacSha256Ts { .. } => "hmac-sha256-ts",
        }
    }

    /// Every header this signer sets (canonical case).
    pub fn header_names(&self) -> Vec<String> {
        match self {
            Sign::Ovh => OVH_HEADERS.iter().map(|h| h.to_string()).collect(),
            Sign::HmacSha256 { header, .. } => vec![header.clone()],
            Sign::HmacSha256Ts { header, ts_header, .. } => {
                std::iter::once(header.clone()).chain(ts_header.clone()).collect()
            }
        }
    }

    /// Normalise header names (canonical case) and check every field.
    pub fn normalised(mut self) -> Result<Sign> {
        let check_header = |h: &str| -> Result<String> {
            validate_header_name(h).map_err(|e| VaultError::bad_request("bad_inject", e.hint))?;
            if is_forbidden_header(h) {
                return Err(VaultError::bad_request("bad_inject", format!("can't sign into {h}")));
            }
            Ok(canonical_header(h))
        };
        let one_line = |s: &str, max: usize, what: &str| -> Result<()> {
            if s.chars().count() > max || s.bytes().any(|b| !(0x20..0x7f).contains(&b)) {
                return Err(VaultError::bad_request("bad_inject", format!("{what} is at most {max} printable ASCII characters")));
            }
            Ok(())
        };
        match &mut self {
            Sign::Ovh => {}
            Sign::HmacSha256 { header, prefix, .. } => {
                *header = check_header(header)?;
                one_line(prefix, 32, "the signature prefix")?;
            }
            Sign::HmacSha256Ts { header, ts_header, template, value_fmt, .. } => {
                *header = check_header(header)?;
                if let Some(t) = ts_header {
                    *t = check_header(t)?;
                    if t.eq_ignore_ascii_case(header) {
                        return Err(VaultError::bad_request("bad_inject", "the timestamp and signature headers must differ"));
                    }
                }
                one_line(template, 64, "the signing template")?;
                one_line(value_fmt, 64, "the signature value format")?;
                if template.matches("{body}").count() != 1 || template.matches("{ts}").count() != 1 {
                    return Err(VaultError::bad_request("bad_inject", "the signing template needs exactly one {body} and one {ts}"));
                }
                if value_fmt.matches("{sig}").count() != 1 || value_fmt.matches("{ts}").count() > 1 {
                    return Err(VaultError::bad_request("bad_inject", "the signature value format needs exactly one {sig} and at most one {ts}"));
                }
            }
        }
        Ok(self)
    }

    /// Check a value for this signer (put, rotate, policy): OVH needs its
    /// JSON with three non-empty one-line strings. The error never quotes
    /// the value.
    pub fn check_value(&self, value: &[u8]) -> Result<()> {
        match self {
            Sign::Ovh => Self::ovh_keys(value).map(|_| ()),
            _ => Ok(()),
        }
    }

    pub fn ovh_keys(value: &[u8]) -> Result<OvhKeys> {
        let bad = |what: &str| {
            VaultError::bad_request(
                "bad_value",
                format!("an OVH secret is JSON {{\"applicationKey\",\"applicationSecret\",\"consumerKey\"}}: {what}"),
            )
        };
        let v: serde_json::Value = serde_json::from_slice(value).map_err(|_| bad("this isn't JSON"))?;
        let field = |name: &str| -> Result<String> {
            let s = v.get(name).and_then(|x| x.as_str()).ok_or_else(|| bad(&format!("{name} is missing")))?;
            if s.is_empty() || s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
                return Err(bad(&format!("{name} is empty or has a line break")));
            }
            Ok(s.to_string())
        };
        Ok(OvhKeys {
            application_key: field("applicationKey")?,
            application_secret: field("applicationSecret")?,
            consumer_key: field("consumerKey")?,
        })
    }
}

impl Default for Inject {
    fn default() -> Self {
        Inject::bearer()
    }
}

impl Inject {
    pub fn bearer() -> Inject {
        Inject { dst: Dst::Header("Authorization".into()), fmt: "Bearer {}".into(), allowed_dst: vec![], allowed_fmt: vec![], sign: None }
    }

    pub fn header(name: &str, fmt: &str) -> Inject {
        Inject { dst: Dst::Header(canonical_header(name)), fmt: fmt.into(), allowed_dst: vec![], allowed_fmt: vec![], sign: None }
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
        if let Some(s) = &self.sign {
            if s.clone().normalised()? != *s {
                return Err(VaultError::bad_request("bad_inject", "signer header names must be in canonical form"));
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
        if self.sign.is_some() && (dst.is_some() || fmt.is_some()) {
            return Err(VaultError::forbidden(
                "inject_not_allowed",
                "this secret signs the request; its creator fixed how, so --dst and --fmt can't be used",
            ));
        }
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
    /// A signer scheme (`ovh`): K2 signs requests instead of pasting the
    /// value. Signed presets have no prefixes, so [`detect`] never picks one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign: Option<&'static str>,
}

impl Preset {
    pub fn inject(&self) -> Inject {
        Inject {
            dst: Dst::parse(self.dst).expect("preset dst is valid"),
            fmt: self.fmt.to_string(),
            allowed_dst: vec![],
            allowed_fmt: vec![],
            sign: match self.sign {
                Some("ovh") => Some(Sign::Ovh),
                Some(other) => unreachable!("preset signer {other} isn't known"),
                None => None,
            },
        }
    }

    pub fn host_list(&self) -> Vec<String> {
        self.hosts.iter().map(|h| h.to_string()).collect()
    }
}

/// Order matters: longer, more specific prefixes first.
pub const PRESETS: &[Preset] = &[
    Preset { id: "anthropic", label: "Anthropic", prefixes: &["sk-ant-"], hosts: &["https://api.anthropic.com:443"], dst: "header:x-api-key", fmt: "{}", sign: None },
    Preset { id: "stripe", label: "Stripe", prefixes: &["sk_live_", "rk_live_", "sk_test_", "rk_test_"], hosts: &["https://api.stripe.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: None },
    Preset { id: "openai", label: "OpenAI", prefixes: &["sk-proj-", "sk-svcacct-", "sk-admin-", "sk-"], hosts: &["https://api.openai.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: None },
    Preset { id: "xai", label: "xAI", prefixes: &["xai-"], hosts: &["https://api.x.ai:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: None },
    Preset { id: "gemini", label: "Google Gemini", prefixes: &["AIza"], hosts: &["https://generativelanguage.googleapis.com:443"], dst: "header:x-goog-api-key", fmt: "{}", sign: None },
    Preset { id: "github", label: "GitHub", prefixes: &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_"], hosts: &["https://api.github.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: None },
    Preset { id: "slack", label: "Slack", prefixes: &["xoxb-", "xoxp-"], hosts: &["https://slack.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: None },
    Preset { id: "supabase", label: "Supabase", prefixes: &["sb_secret_"], hosts: &[], dst: "header:apikey", fmt: "{}", sign: None },
    // OVH API v1 (signed; the value is the three-key JSON).
    Preset { id: "ovh-eu", label: "OVHcloud Europe", prefixes: &[], hosts: &["https://eu.api.ovh.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: Some("ovh") },
    Preset { id: "ovh-ca", label: "OVHcloud Canada", prefixes: &[], hosts: &["https://ca.api.ovh.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: Some("ovh") },
    Preset { id: "ovh-us", label: "OVHcloud US", prefixes: &[], hosts: &["https://api.us.ovhcloud.com:443"], dst: "header:Authorization", fmt: "Bearer {}", sign: Some("ovh") },
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

    fn ts_sign(template: &str, value_fmt: &str) -> Sign {
        Sign::HmacSha256Ts { header: "X-Sig".into(), ts_header: Some("X-Ts".into()), template: template.into(), value_fmt: value_fmt.into(), encoding: Enc::Hex }
    }

    #[test]
    fn no_sign_json_is_byte_identical_and_sign_round_trips() {
        assert_eq!(serde_json::to_string(&Inject::bearer()).expect("json"), r#"{"dst":"header:Authorization","fmt":"Bearer {}"}"#);
        for s in [
            Sign::Ovh,
            Sign::HmacSha256 { header: "X-Hub-Signature-256".into(), encoding: Enc::Hex, prefix: "sha256=".into() },
            ts_sign("{ts}.{body}", "sha256={sig}"),
        ] {
            let i = Inject { sign: Some(s.clone()), ..Inject::bearer() };
            let j = serde_json::to_string(&i).expect("json");
            assert!(j.contains(&format!(r#""scheme":"{}""#, s.scheme())), "{j}");
            assert_eq!(serde_json::from_str::<Inject>(&j).expect("parse"), i);
            i.validate().expect("valid");
        }
        assert_eq!(
            serde_json::to_string(&Inject { sign: Some(Sign::Ovh), ..Inject::bearer() }).expect("json"),
            r#"{"dst":"header:Authorization","fmt":"Bearer {}","sign":{"scheme":"ovh"}}"#
        );
    }

    #[test]
    fn a_signer_refuses_agent_dst_and_fmt() {
        let i = Inject { sign: Some(Sign::Ovh), ..Inject::bearer() };
        assert_eq!(i.choose(Some("header:Authorization"), None).expect_err("dst").code, "inject_not_allowed");
        assert_eq!(i.choose(None, Some("Bearer {}")).expect_err("fmt").code, "inject_not_allowed");
        i.choose(None, None).expect("defaults are fine");
    }

    #[test]
    fn signer_templates_and_headers_are_checked() {
        for (t, v) in [("{body}", "{sig}"), ("{ts}.{ts}.{body}", "{sig}"), ("{ts}.{body}.{body}", "{sig}"), ("{ts}.{body}", "x"), ("{ts}.{body}", "{sig}{sig}"), ("{ts}.{body}", "{ts}{ts}{sig}")] {
            assert_eq!(ts_sign(t, v).normalised().expect_err(t).code, "bad_inject", "{t} / {v}");
        }
        assert_eq!(ts_sign(&format!("{{ts}}.{{body}}{}", "x".repeat(60)), "{sig}").normalised().expect_err("long").code, "bad_inject");
        ts_sign("v0:{ts}:{body}", "t={ts},v1={sig}").normalised().expect("ok");
        let h = |header: &str, prefix: &str| Sign::HmacSha256 { header: header.into(), encoding: Enc::Hex, prefix: prefix.into() }.normalised();
        assert_eq!(h("Host", "").expect_err("host").code, "bad_inject");
        assert_eq!(h("X A", "").expect_err("space").code, "bad_inject");
        assert_eq!(h("X-Sig", &"p".repeat(33)).expect_err("prefix").code, "bad_inject");
        assert_eq!(h("X-Sig", "a\tb").expect_err("tab").code, "bad_inject");
        match h("x-hub-signature-256", "sha256=").expect("ok") {
            Sign::HmacSha256 { header, .. } => assert_eq!(header, "X-Hub-Signature-256"),
            _ => panic!("hmac"),
        }
        let same = Sign::HmacSha256Ts { header: "X-Sig".into(), ts_header: Some("x-sig".into()), template: DEFAULT_TS_TEMPLATE.into(), value_fmt: DEFAULT_SIG_VALUE.into(), encoding: Enc::Hex };
        assert_eq!(same.normalised().expect_err("same").code, "bad_inject");
        // A stored inject with a non-canonical header is refused by validate.
        let raw = Inject { sign: Some(Sign::HmacSha256 { header: "x-sig".into(), encoding: Enc::Hex, prefix: String::new() }), ..Inject::bearer() };
        assert_eq!(raw.validate().expect_err("canonical").code, "bad_inject");
    }

    #[test]
    fn ovh_presets_sign_and_are_never_detected() {
        for (id, host) in [("ovh-eu", "https://eu.api.ovh.com:443"), ("ovh-ca", "https://ca.api.ovh.com:443"), ("ovh-us", "https://api.us.ovhcloud.com:443")] {
            let p = preset(id).expect(id);
            assert!(p.prefixes.is_empty());
            assert_eq!(p.hosts, &[host]);
            assert_eq!(p.inject().sign, Some(Sign::Ovh));
        }
        assert!(PRESETS.iter().filter(|p| p.sign.is_some()).all(|p| p.prefixes.is_empty()));
    }
}
