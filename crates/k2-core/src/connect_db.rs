//! The K2 Connect account server (k2.dev accounts: sign-in, purchased
//! subdomains, the `claim_subdomain` lease): ONE place that says which
//! base URL every K2 client talks to.
//!
//! 0.46.6: the base URL can be set; the default is unchanged.
//!
//! * `K2_CONNECT_DB_URL` in the process environment wins;
//! * else the `connectDbUrl` setting (`~/.k2/settings.json`, Owner-only to
//!   write through `/cli/settings/update`);
//! * else [`DEFAULT_URL`], the Supabase project that has always backed
//!   k2.dev accounts.
//!
//! There is NO automatic fallback between hosts: whatever this resolves to
//! is the one host every account call goes to. A later release moves the
//! default to `https://db.k2.dev` once that server is live and its label is
//! reserved, behind a check that the server really is K2's account API
//! before any password is sent to it.
//!
//! Who reads it: the daemon's lease loop (`tunnel::lease`) calls [`base`];
//! the desktop client reads the daemon's answer from `GET
//! /cli/tunnel/config` (`accountDbUrl`); the bash `k2 connect` reads the
//! same env var and the same settings.json key itself, because it signs in
//! before a daemon has to be running.
//!
//! The anon key does not change with the base URL (the new server imports
//! the same JWT secret) and is never logged.

use std::sync::Mutex;

/// The Supabase project behind k2.dev accounts: the default base URL.
pub const SUPABASE_URL: &str = "https://ttgcalfrzzgkxnfepkiu.supabase.co";

/// The base URL used when neither the env var nor the setting names one.
pub const DEFAULT_URL: &str = SUPABASE_URL;

/// Env override (wins over the setting).
pub const ENV_VAR: &str = "K2_CONNECT_DB_URL";

/// The settings.json key (camelCase wire name of
/// `AppSettings::connect_db_url`).
pub const SETTING_KEY: &str = "connectDbUrl";

/// Public `anon` key for the account project. Safe to embed in a client
/// (it only authorizes the unauthenticated role; rows are RLS-scoped to the
/// signed-in caller via the bearer access token). Identical to the value in
/// the renderer and `cli/k2`. NEVER log it.
pub const ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InR0Z2NhbGZyenpna3huZmVwa2l1Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODA1MDIyMzksImV4cCI6MjA5NjA3ODIzOX0.L28xgtYkPEj5eCNDGO5Zf5xxhdKQLxKD8c1CJRHNqI8";

/// `User-Agent` for the daemon's account-server calls, so the server's
/// logs can tell K2 traffic apart (reqwest sends none by default).
pub const USER_AGENT: &str = concat!("k2-daemon/", env!("CARGO_PKG_VERSION"));

/// Where the resolved base came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    Setting,
    Env,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Setting => "setting",
            Source::Env => "env",
        }
    }
}

/// The resolved account-server base URL (no trailing slash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub url: String,
    pub source: Source,
    /// A configured value that was refused (bad scheme, userinfo, ...): why,
    /// so the one log line can say why the next source was used.
    pub ignored: Option<String>,
}

/// Validate and normalize a configured base URL. `https://` only, except
/// plain `http://` to a loopback host (local fakes and dev servers). No
/// `user:pass@`, no query or fragment, no whitespace; trailing `/` trimmed.
pub fn normalize(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{SETTING_KEY} must not contain spaces"));
    }
    let rest = if let Some(r) = t.strip_prefix("https://") {
        r
    } else if let Some(r) = t.strip_prefix("http://") {
        let authority = r.split(['/', '?', '#']).next().unwrap_or("");
        let host = match authority.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or(""),
            None => authority.split(':').next().unwrap_or(""),
        };
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
            return Err(format!(
                "{SETTING_KEY} must be an https:// URL (plain http:// only to localhost)"
            ));
        }
        r
    } else {
        return Err(format!("{SETTING_KEY} must be an https:// URL"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(format!("{SETTING_KEY} has no host"));
    }
    if authority.contains('@') {
        return Err(format!("{SETTING_KEY} must not carry a user name or password"));
    }
    if rest.contains('?') || rest.contains('#') {
        return Err(format!("{SETTING_KEY} must not carry a query or fragment"));
    }
    Ok(t.trim_end_matches('/').to_string())
}

/// Pure resolution: env value, then setting value, then [`DEFAULT_URL`].
/// An empty value counts as unset; an invalid one is skipped (and noted in
/// `ignored`). The order is fixed; nothing here ever picks a host because
/// another one failed.
pub fn resolve(env: Option<&str>, setting: &str) -> Base {
    let mut ignored = None;
    if let Some(v) = env.map(str::trim).filter(|v| !v.is_empty()) {
        match normalize(v) {
            Ok(url) => return Base { url, source: Source::Env, ignored: None },
            Err(e) => ignored = Some(format!("{ENV_VAR} ignored: {e}")),
        }
    }
    let s = setting.trim();
    if !s.is_empty() {
        match normalize(s) {
            Ok(url) => return Base { url, source: Source::Setting, ignored },
            Err(e) => ignored = Some(format!("setting {SETTING_KEY} ignored: {e}")),
        }
    }
    Base { url: DEFAULT_URL.to_string(), source: Source::Default, ignored }
}

/// The one log line for a resolved base: the URL and its source, never
/// the key.
pub fn log_line(b: &Base) -> String {
    let mut line = format!("[connect-db] account server: {} (from {})", b.url, b.source.as_str());
    if let Some(why) = &b.ignored {
        line.push_str("; ");
        line.push_str(why);
    }
    line
}

/// Process-facing resolution: the env var, then the settings file. Logs
/// [`log_line`] once per distinct answer (the first call, then only when
/// the env or the setting changes what it resolves to).
pub fn base() -> Base {
    let env = std::env::var(ENV_VAR).ok();
    let setting = crate::app_settings::load().connect_db_url;
    let b = resolve(env.as_deref(), &setting);
    static LOGGED: Mutex<Option<(String, &'static str)>> = Mutex::new(None);
    let mut last = LOGGED.lock().unwrap_or_else(|p| p.into_inner());
    let key = (b.url.clone(), b.source.as_str());
    if last.as_ref() != Some(&key) {
        crate::log_debug!("{}", log_line(&b));
        *last = Some(key);
    }
    b
}

/// The base URL only (see [`base`]).
pub fn base_url() -> String {
    base().url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_supabase_project() {
        let b = resolve(None, "");
        assert_eq!(b.url, "https://ttgcalfrzzgkxnfepkiu.supabase.co");
        assert_eq!(b.source, Source::Default);
        assert_eq!(b.ignored, None);
        assert_eq!(DEFAULT_URL, SUPABASE_URL);
        // Blank env and blank setting are "unset", not an override.
        assert_eq!(resolve(Some("  "), "  ").source, Source::Default);
    }

    #[test]
    fn setting_overrides_the_default() {
        let b = resolve(None, "https://db.example.test/");
        assert_eq!(b.url, "https://db.example.test");
        assert_eq!(b.source, Source::Setting);
    }

    #[test]
    fn env_overrides_the_setting() {
        let b = resolve(Some("https://env.example.test"), "https://db.example.test");
        assert_eq!(b.url, "https://env.example.test");
        assert_eq!(b.source, Source::Env);
    }

    #[test]
    fn invalid_values_skip_to_the_next_source_in_order() {
        let b = resolve(Some("ftp://nope"), "https://db.example.test");
        assert_eq!((b.url.as_str(), b.source), ("https://db.example.test", Source::Setting));
        assert!(b.ignored.as_deref().is_some_and(|w| w.contains(ENV_VAR)), "{b:?}");
        let b = resolve(None, "http://db.example.test");
        assert_eq!((b.url.as_str(), b.source), (DEFAULT_URL, Source::Default));
        assert!(b.ignored.as_deref().is_some_and(|w| w.contains("https://")), "{b:?}");
    }

    #[test]
    fn normalize_rules() {
        assert_eq!(normalize(" https://db.k2.dev/ ").as_deref(), Ok("https://db.k2.dev"));
        assert_eq!(normalize("https://h.test:8443/base").as_deref(), Ok("https://h.test:8443/base"));
        assert_eq!(normalize("http://127.0.0.1:5999").as_deref(), Ok("http://127.0.0.1:5999"));
        assert_eq!(normalize("http://localhost:5999/").as_deref(), Ok("http://localhost:5999"));
        assert_eq!(normalize("http://[::1]:5999").as_deref(), Ok("http://[::1]:5999"));
        for bad in [
            "",
            "db.k2.dev",
            "http://db.k2.dev",
            "http://127.0.0.1.evil.test",
            "http://localhost@evil.test",
            "https://",
            "https://u:p@db.k2.dev",
            "https://db.k2.dev/?x=1",
            "https://db.k2.dev/#f",
            "https://db.k2 .dev",
            "javascript:alert(1)",
        ] {
            assert!(normalize(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn log_line_names_url_and_source_and_never_the_key() {
        for b in [
            resolve(None, ""),
            resolve(None, "https://db.example.test"),
            resolve(Some("https://env.example.test"), ""),
            resolve(Some("nope"), "also-nope"),
        ] {
            let line = log_line(&b);
            assert!(line.contains(&b.url), "{line}");
            assert!(line.contains(b.source.as_str()), "{line}");
            assert!(!line.contains(ANON_KEY), "the anon key leaked into the log line");
            assert!(!line.contains("eyJ"), "a JWT-looking value leaked into the log line");
        }
    }

    #[test]
    fn process_resolution_reads_env_then_setting() {
        let _home = crate::test_env::TempHome::new();
        let _env = crate::test_env::EnvVar::remove(ENV_VAR);
        assert_eq!(base().source, Source::Default);
        assert_eq!(base_url(), DEFAULT_URL);
        crate::app_settings::update(serde_json::json!({ SETTING_KEY: "https://db.example.test/" }))
            .expect("setting write");
        let b = base();
        assert_eq!((b.url.as_str(), b.source), ("https://db.example.test", Source::Setting));
        let _env2 = crate::test_env::EnvVar::set(ENV_VAR, "http://127.0.0.1:5999");
        let b = base();
        assert_eq!((b.url.as_str(), b.source), ("http://127.0.0.1:5999", Source::Env));
    }
}
