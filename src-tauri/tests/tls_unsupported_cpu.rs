//! The unsupported-CPU path with the CPU check stubbed (Kessel Steward R10,
//! option b: no fallback provider). Its own test binary, so its own process:
//! the TLS choice is made once per process.

use k2_lib::tls_provider::{self, TlsChoice, UNSUPPORTED_CPU_MESSAGE, UNSUPPORTED_CPU_TITLE};
use rustls::crypto::CryptoProvider;

#[test]
fn an_unsupported_cpu_shows_the_message_and_installs_no_provider() {
    let mut shown: Option<(String, String)> = None;
    let started = tls_provider::startup_with(
        || false,
        |title, message| shown = Some((title.to_string(), message.to_string())),
    );
    assert!(!started, "startup must refuse on an unsupported CPU");
    let (title, message) = shown.expect("the refusal must show a message");
    assert_eq!(title, UNSUPPORTED_CPU_TITLE);
    assert_eq!(message, UNSUPPORTED_CPU_MESSAGE);
    assert!(message.starts_with("This computer's processor isn't supported."), "{message}");
    assert_eq!(tls_provider::active(), Some(TlsChoice::UnsupportedCpu));
    assert!(
        CryptoProvider::get_default().is_none(),
        "no TLS provider may be installed on an unsupported CPU (no fallback)"
    );

    // `run`'s guard asks again: same answer, the check isn't rerun, and still
    // nothing is installed.
    let mut asked_again = false;
    let again = tls_provider::install_with(|| {
        asked_again = true;
        true
    });
    assert_eq!(again, TlsChoice::UnsupportedCpu);
    assert!(!asked_again, "the CPU check runs once per process");
    assert!(CryptoProvider::get_default().is_none());
}
