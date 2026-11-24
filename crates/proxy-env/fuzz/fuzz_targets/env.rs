//! The environment as any bytes: the input splits at NUL bytes into a
//! proxy variable, a `no_proxy` list and a target host. Reading them
//! never panics; a proxy URL that reads prints back (without its user
//! part) as one that reads the same.
//!
//! Run from this folder: `cargo +nightly fuzz run env`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use proxy_env::{proxy_for, NoProxy, ProxyUrl};
use std::ffi::OsString;

#[cfg(unix)]
fn os(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(b.to_vec())
}

#[cfg(not(unix))]
fn os(b: &[u8]) -> OsString {
    String::from_utf8_lossy(b).into_owned().into()
}

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(3, |b| *b == 0);
    let proxy = parts.next().unwrap_or(b"");
    let no_proxy = parts.next().unwrap_or(b"");
    let host = String::from_utf8_lossy(parts.next().unwrap_or(b"")).into_owned();

    if let Ok(p) = ProxyUrl::parse(proxy) {
        let again = ProxyUrl::parse(p.to_string().as_bytes())
            .expect("a proxy URL prints back as one that reads");
        assert_eq!(
            (again.scheme(), again.host(), again.port()),
            (p.scheme(), p.host(), p.port())
        );
    }
    let _ = NoProxy::parse(no_proxy).matches(&host);
    let env = |n: &str| match n {
        "https_proxy" => Some(os(proxy)),
        "no_proxy" => Some(os(no_proxy)),
        _ => None,
    };
    let _ = proxy_for("https", &host, env);
});
