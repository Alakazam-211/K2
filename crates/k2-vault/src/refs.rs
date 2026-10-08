//! `k2v://` references (§6.4). Safe anywhere a value would otherwise go:
//! preset env, publish bindings, `.env.k2`, tickets, wiki.
//!
//! Grammar: `k2v://<scope>/<name>[@<version>]` where scope is `server`,
//! `ws/<workspace name|id>`, `app/<install id|name>` (reserved) or
//! `session/<id>`. `system` secrets have no user-facing reference.

use crate::error::{Result, VaultError};
use crate::model::{validate_name, ScopeKind};

pub const PREFIX: &str = "k2v://";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRef {
    pub scope_kind: ScopeKind,
    /// Workspace name or id as written (normalised to an id when saved by
    /// the daemon, which knows the workspace table). Empty for server.
    pub scope_ref: String,
    pub name: String,
    pub version: Option<u32>,
}

impl VaultRef {
    pub fn is_ref(s: &str) -> bool {
        s.trim_start().starts_with(PREFIX)
    }

    pub fn parse(s: &str) -> Result<VaultRef> {
        let bad = |h: &str| VaultError::bad_request("bad_ref", format!("{h} (expected k2v://ws/<workspace>/<NAME>)"));
        let rest = s.trim().strip_prefix(PREFIX).ok_or_else(|| bad("not a k2v:// reference"))?;
        let (scope_kind, scope_ref, tail) = if let Some(t) = rest.strip_prefix("server/") {
            (ScopeKind::Server, String::new(), t)
        } else {
            let (k, t) = rest.split_once('/').ok_or_else(|| bad("missing scope"))?;
            let kind = match k {
                "ws" => ScopeKind::Workspace,
                "app" => ScopeKind::App,
                "session" => ScopeKind::Session,
                _ => return Err(bad("unknown scope")),
            };
            let (id, t) = t.split_once('/').ok_or_else(|| bad("missing name"))?;
            if id.is_empty()
                || id.len() > 128
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            {
                return Err(bad("bad scope id"));
            }
            (kind, id.to_string(), t)
        };
        let (name, version) = match tail.split_once('@') {
            Some((n, v)) => {
                let ver: u32 = v.parse().map_err(|_| bad("bad version"))?;
                if ver == 0 {
                    return Err(bad("versions start at 1"));
                }
                (n, Some(ver))
            }
            None => (tail, None),
        };
        validate_name(name, true)?;
        Ok(VaultRef { scope_kind, scope_ref, name: name.to_string(), version })
    }

    pub fn render(&self) -> String {
        let scope = match self.scope_kind {
            ScopeKind::Server => "server".to_string(),
            ScopeKind::Workspace => format!("ws/{}", self.scope_ref),
            ScopeKind::App => format!("app/{}", self.scope_ref),
            ScopeKind::Session => format!("session/{}", self.scope_ref),
            ScopeKind::Engine | ScopeKind::Person => format!("{}/{}", self.scope_kind, self.scope_ref),
        };
        match self.version {
            Some(v) => format!("{PREFIX}{scope}/{}@{v}", self.name),
            None => format!("{PREFIX}{scope}/{}", self.name),
        }
    }
}

/// Placeholder token prefix for the loopback proxy (§6.4).
pub const PLACEHOLDER_PREFIX: &str = "k2v_ph_";

pub fn new_placeholder() -> String {
    format!("{PLACEHOLDER_PREFIX}{}", crate::crypto::base62(32))
}

pub fn looks_like_placeholder(s: &str) -> bool {
    s.len() == PLACEHOLDER_PREFIX.len() + 32
        && s.starts_with(PLACEHOLDER_PREFIX)
        && s[PLACEHOLDER_PREFIX.len()..].bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_refs() {
        let cases = [
            ("k2v://server/OPENAI_KEY", ScopeKind::Server, "", "OPENAI_KEY", None),
            ("k2v://ws/sales/STRIPE_KEY", ScopeKind::Workspace, "sales", "STRIPE_KEY", None),
            ("k2v://ws/sales/STRIPE_KEY@3", ScopeKind::Workspace, "sales", "STRIPE_KEY", Some(3)),
            ("k2v://session/abc-123/TOKEN", ScopeKind::Session, "abc-123", "TOKEN", None),
            ("  k2v://ws/0b5e8c1e-1f2a-4c1d-9b5e-123456789abc/DATABASE_URL ", ScopeKind::Workspace,
                "0b5e8c1e-1f2a-4c1d-9b5e-123456789abc", "DATABASE_URL", None),
        ];
        for (s, k, id, n, v) in cases {
            let r = VaultRef::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
            assert_eq!((r.scope_kind, r.scope_ref.as_str(), r.name.as_str(), r.version), (k, id, n, v), "{s}");
            assert_eq!(VaultRef::parse(&r.render()).expect("re-parses"), r);
        }
    }

    #[test]
    fn bad_refs() {
        for s in [
            "k2v:/ws/a/B",
            "k2v://ws/a",
            "k2v://ws//B",
            "k2v://nope/a/B",
            "k2v://ws/a/B@0",
            "k2v://ws/a/B@x",
            "k2v://ws/a b/B",
            "k2v://ws/a/../B",
            "k2v://server/",
            "k2test_live_123",
        ] {
            assert!(VaultRef::parse(s).is_err(), "{s} should be refused");
        }
    }

    #[test]
    fn placeholders() {
        let p = new_placeholder();
        assert!(looks_like_placeholder(&p));
        assert!(!looks_like_placeholder("k2v_ph_short"));
        assert_ne!(new_placeholder(), p);
    }
}
