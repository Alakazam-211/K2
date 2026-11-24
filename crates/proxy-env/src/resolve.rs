//! Which proxy a request uses, from the environment, as the curl command
//! picks it.

use crate::error::ConfigError;
use crate::no_proxy::NoProxy;
use crate::url::ProxyUrl;
use std::ffi::OsString;

/// The process environment, for [`proxy_for`].
pub fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// One variable's value as bytes; an empty value counts as unset.
fn value(env: &impl Fn(&str) -> Option<OsString>, name: &str) -> Option<Vec<u8>> {
    env(name)
        .map(OsString::into_encoded_bytes)
        .filter(|v| !v.is_empty())
}

/// The variable that names the proxy for a URL of `scheme`, and its
/// value: `<scheme>_proxy`, then `<SCHEME>_PROXY` (never `HTTP_PROXY`,
/// for `http`), then `all_proxy`, then `ALL_PROXY`. The lower-case name
/// wins; an empty value counts as unset. `None`: no proxy.
pub fn proxy_setting(
    scheme: &str,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Option<(String, Vec<u8>)> {
    let scheme = scheme.to_ascii_lowercase();
    let mut names = vec![format!("{scheme}_proxy")];
    if scheme != "http" {
        names.push(format!("{}_PROXY", scheme.to_ascii_uppercase()));
    }
    names.push("all_proxy".into());
    names.push("ALL_PROXY".into());
    names
        .into_iter()
        .find_map(|n| value(env, &n).map(|v| (n, v)))
}

/// `no_proxy`, else `NO_PROXY` (an empty value counts as unset).
pub fn no_proxy_setting(env: &impl Fn(&str) -> Option<OsString>) -> Option<NoProxy> {
    value(env, "no_proxy")
        .or_else(|| value(env, "NO_PROXY"))
        .map(|v| NoProxy::parse(&v))
}

/// The proxy a request to `scheme://host` uses, read from `env` as curl
/// reads the environment ([`process_env`] for the real one):
///
/// 1. the proxy variable for the scheme ([`proxy_setting`]); none set:
///    no proxy;
/// 2. `no_proxy` ([`no_proxy_setting`]): a listed host uses no proxy
///    ([`NoProxy::matches`]), even when the proxy URL is one curl can't
///    read;
/// 3. the proxy URL ([`ProxyUrl::parse`]): an `Err` is a setting curl
///    refuses, and the request fails; it never goes direct.
///
/// `host` is the URL's host as written: a name, four numbers, or an
/// IPv6 address (with or without brackets).
pub fn proxy_for(
    scheme: &str,
    host: &str,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<ProxyUrl>, ConfigError> {
    let Some((_name, raw)) = proxy_setting(scheme, &env) else {
        return Ok(None);
    };
    if no_proxy_setting(&env).is_some_and(|n| n.matches(host)) {
        return Ok(None);
    }
    ProxyUrl::parse(&raw).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |n: &str| m.get(n).cloned()
    }

    /// The order of the variables, and an empty value counts as unset.
    #[test]
    fn which_variable_names_the_proxy() {
        type Row = (
            &'static [(&'static str, &'static str)],
            &'static str,
            Option<&'static str>,
        );
        let rows: &[Row] = &[
            (
                &[
                    ("https_proxy", "a"),
                    ("HTTPS_PROXY", "b"),
                    ("all_proxy", "c"),
                ],
                "https",
                Some("a"),
            ),
            (
                &[("HTTPS_PROXY", "b"), ("all_proxy", "c")],
                "https",
                Some("b"),
            ),
            (
                &[("https_proxy", ""), ("HTTPS_PROXY", "b")],
                "https",
                Some("b"),
            ),
            (
                &[("all_proxy", "c"), ("ALL_PROXY", "d")],
                "https",
                Some("c"),
            ),
            (&[("all_proxy", ""), ("ALL_PROXY", "d")], "https", Some("d")),
            (&[("http_proxy", "e"), ("HTTP_PROXY", "f")], "https", None),
            (
                &[("http_proxy", "e"), ("https_proxy", "a")],
                "http",
                Some("e"),
            ),
            (&[("HTTP_PROXY", "f")], "http", None),
            (
                &[("HTTP_PROXY", "f"), ("ALL_PROXY", "d")],
                "http",
                Some("d"),
            ),
            (&[("https_proxy", "a")], "http", None),
            (&[("https_proxy", "a")], "HTTPS", Some("a")),
            (&[], "https", None),
        ];
        for (vars, scheme, want) in rows {
            let got = proxy_setting(scheme, &env(vars)).map(|(_, v)| v);
            assert_eq!(got.as_deref(), want.map(str::as_bytes), "{vars:?} {scheme}");
        }
        let e = env(&[("no_proxy", ""), ("NO_PROXY", "x")]);
        assert!(no_proxy_setting(&e).unwrap().matches("x"));
        let e = env(&[("no_proxy", "y"), ("NO_PROXY", "x")]);
        assert!(!no_proxy_setting(&e).unwrap().matches("x"));
    }

    #[test]
    fn the_proxy_for_a_host() {
        let p = |e: &[(&str, &str)], host: &str| proxy_for("https", host, env(e));
        let set = [
            ("https_proxy", "p.example:3128"),
            ("no_proxy", "box.example"),
        ];
        assert_eq!(p(&set, "box.example"), Ok(None));
        assert_eq!(p(&set, "a.box.example"), Ok(None));
        assert_eq!(
            p(&set, "other.example").unwrap().unwrap().to_string(),
            "http://p.example:3128"
        );
        assert_eq!(p(&[], "box.example"), Ok(None));
        // A listed host goes direct even when curl couldn't read the URL.
        let bad = [("https_proxy", "ftp://p.example"), ("NO_PROXY", "*")];
        assert_eq!(p(&bad, "box.example"), Ok(None));
        // An unlisted one fails: never direct.
        let bad = [("https_proxy", "ftp://p.example")];
        assert_eq!(
            p(&bad, "box.example"),
            Err(ConfigError::UnsupportedScheme("ftp".into()))
        );
        // A no_proxy with no proxy set changes nothing.
        assert_eq!(p(&[("no_proxy", "a b")], "box.example"), Ok(None));
    }

    #[cfg(unix)]
    #[test]
    fn values_need_not_be_text() {
        use std::os::unix::ffi::OsStringExt;
        let e = |n: &str| match n {
            "https_proxy" => Some(OsString::from_vec(b"http://p.example:1/\xff".to_vec())),
            "no_proxy" => Some(OsString::from_vec(b"\xff".to_vec())),
            _ => None,
        };
        let got = proxy_for("https", "box.example", e).unwrap().unwrap();
        assert_eq!(got.to_string(), "http://p.example:1");
    }
}
