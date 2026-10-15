//! Env and file delivery names (VS10).

use crate::error::{Result, VaultError};

/// Env names that change how programs load code, find binaries, trust
/// certificates, route traffic, or talk to K2 itself. A grant can never
/// deliver a secret under one of these.
const DENY_EXACT: &[&str] = &[
    "PATH", "HOME", "SHELL", "USER", "LOGNAME", "TMPDIR", "TMP", "TEMP", "PWD", "IFS",
    "NODE_OPTIONS", "NODE_PATH", "NODE_EXTRA_CA_CERTS", "RUBYOPT", "RUBYLIB", "BASH_ENV", "ENV",
    "PROMPT_COMMAND", "SSL_CERT_FILE", "SSL_CERT_DIR", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE",
    "NO_PROXY", "EDITOR", "VISUAL", "PAGER", "ZDOTDIR", "JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS",
    "CLASSPATH", "GOFLAGS", "CARGO_HOME", "RUSTFLAGS", "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME",
    "CLAUDE_CONFIG_DIR", "CODEX_HOME", "GROK_HOME",
];

const DENY_PREFIX: &[&str] = &[
    "LD_", "DYLD_", "PYTHON", "PERL5", "GIT_", "SSH_", "SSL_CERT_", "K2_", "K2SO_", "NPM_CONFIG_",
];

/// Provider base URLs the loopback proxy owns: an env grant under one of
/// these could point a real key somewhere else.
const DENY_SUFFIX: &[&str] = &["_PROXY", "_BASE_URL", "_API_BASE", "_API_BASE_URL"];

pub fn validate_env_name(name: &str) -> Result<()> {
    let ok_shape = !name.is_empty()
        && name.len() <= 64
        && name.bytes().next().is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if !ok_shape {
        return Err(VaultError::bad_request(
            "bad_env_name",
            "environment names use A-Z, 0-9 and _ and start with a letter or _",
        ));
    }
    let upper = name.to_ascii_uppercase();
    if DENY_EXACT.contains(&upper.as_str())
        || DENY_PREFIX.iter().any(|p| upper.starts_with(p))
        || DENY_SUFFIX.iter().any(|s| upper.ends_with(s))
    {
        return Err(VaultError::bad_request(
            "env_name_not_allowed",
            format!("{name} changes how programs run; pick another name"),
        ));
    }
    Ok(())
}

/// The env name a secret is delivered under when nothing names one:
/// uppercase, `-` and `.` become `_`, and a leading digit gets a `_`
/// (`mini-k2n1-admin-pw` → `MINI_K2N1_ADMIN_PW`). The result still goes
/// through [`validate_env_name`], so a denied name (`k2-token` →
/// `K2_TOKEN`) is still refused.
pub fn default_env_name(secret_name: &str) -> String {
    let mut s: String = secret_name
        .chars()
        .map(|c| match c {
            '-' | '.' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

pub fn validate_file_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(VaultError::bad_request("bad_file_name", "file names use letters, digits, _ . - (no paths)"))
    }
}

/// Sensitive-name lint (§10.4): keys that must hold a `k2v://` reference,
/// not an inline value, in preset env, publish bindings and app installs.
pub fn is_sensitive_name(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    k.ends_with("_API_KEY")
        || k == "API_KEY"
        || k.ends_with("_TOKEN")
        || k.ends_with("_SECRET")
        || k.contains("PASSWORD")
        || k == "DATABASE_URL"
        || k.ends_with("_DSN")
}

/// Docker's `--env-file` has no quoting: refuse values with a newline.
pub fn validate_env_file_value(v: &[u8]) -> Result<()> {
    if v.contains(&b'\n') || v.contains(&b'\r') || v.contains(&0) {
        return Err(VaultError::bad_request(
            "value_not_env_file_safe",
            "this value has a line break; use file delivery instead",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names() {
        for ok in ["STRIPE_KEY", "_X", "OPENAI_API_KEY", "DATABASE_URL", "A1"] {
            validate_env_name(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in ["", "1A", "a_lower", "X-Y", &"A".repeat(65)] {
            assert_eq!(validate_env_name(bad).expect_err(bad).code, "bad_env_name", "{bad}");
        }
        for denied in [
            "NODE_OPTIONS", "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "PATH", "HOME", "BASH_ENV",
            "PYTHONPATH", "GIT_SSH_COMMAND", "SSH_AUTH_SOCK", "HTTPS_PROXY", "K2_HOOK_TOKEN",
            "ANTHROPIC_BASE_URL", "OPENAI_API_BASE", "REQUESTS_CA_BUNDLE", "SSL_CERT_FILE",
        ] {
            assert_eq!(validate_env_name(denied).expect_err(denied).code, "env_name_not_allowed", "{denied}");
        }
    }

    #[test]
    fn file_names() {
        validate_file_name("stripe.key").expect("ok");
        for bad in ["", ".", "..", "../x", "a/b", "a b", &"a".repeat(65)] {
            assert!(validate_file_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn default_env_names() {
        for (secret, env) in [
            ("mini-k2n1-admin-pw", "MINI_K2N1_ADMIN_PW"),
            ("stripe.key", "STRIPE_KEY"),
            ("DATABASE_URL", "DATABASE_URL"),
            ("1password", "_1PASSWORD"),
        ] {
            assert_eq!(default_env_name(secret), env);
            validate_env_name(env).unwrap_or_else(|e| panic!("{env}: {e}"));
        }
        assert_eq!(default_env_name("k2-token"), "K2_TOKEN");
        assert_eq!(validate_env_name(&default_env_name("k2-token")).expect_err("denied").code, "env_name_not_allowed");
    }

    #[test]
    fn sensitive_names() {
        for s in ["OPENAI_API_KEY", "GITHUB_TOKEN", "WEBHOOK_SECRET", "DB_PASSWORD", "DATABASE_URL", "PG_DSN"] {
            assert!(is_sensitive_name(s), "{s}");
        }
        for s in ["PORT", "NODE_ENV", "LOG_LEVEL"] {
            assert!(!is_sensitive_name(s), "{s}");
        }
    }

    #[test]
    fn env_file_values() {
        validate_env_file_value(b"abc").expect("ok");
        assert!(validate_env_file_value(b"a\nb").is_err());
    }
}
