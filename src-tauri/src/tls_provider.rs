//! The app's process-wide TLS crypto provider (Kessel Steward R9 and R10).
//!
//! rustls under reqwest (the app's own clients and Sew's) and under Tauri's
//! updater is built with no crypto provider of its own: no ring, no
//! aws-lc-rs. Before anything builds a reqwest client or opens a TLS
//! connection, `main` (and `run`, as a guard) installs graviola
//! (`rustls-graviola`) as the process default `CryptoProvider`.
//!
//! Graviola needs a 64-bit CPU with some extensions: on x86_64 aes,
//! pclmulqdq, bmi1, adx, avx and avx2 (Intel and AMD from about 2014 on);
//! on aarch64 neon, aes, pmull and sha2. There is no fallback provider
//! (R10, option b). So the CPU check runs first, and on a CPU without those
//! the app tells the person and exits, before any TLS. Without the check,
//! graviola would panic in the middle of the first handshake.
//!
//! A reqwest client can't even be built without a provider (reqwest panics
//! "No provider set"), and the app's daemon client is a reqwest client, so
//! the app can't run at all on such a CPU. That is why this refuses instead
//! of going on without TLS.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("K2's desktop app is 64-bit only: its TLS provider (graviola) supports x86_64 and aarch64");

use std::sync::OnceLock;

/// What startup decided for TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsChoice {
    /// graviola is the process default provider.
    Graviola,
    /// This CPU lacks what graviola needs. No provider was installed; the
    /// app must not make any TLS connection and exits.
    UnsupportedCpu,
}

/// The title of the message shown on an unsupported CPU.
pub const UNSUPPORTED_CPU_TITLE: &str = "K2 can't run on this computer";

/// The message shown on an unsupported CPU.
pub const UNSUPPORTED_CPU_MESSAGE: &str = "This computer's processor isn't supported. \
K2 needs a 64-bit processor with modern encryption instructions: \
an Intel or AMD processor from about 2014 or later, or an ARM processor \
with the cryptography extensions (every Apple silicon Mac has them). \
K2 will close now.";

static CHOICE: OnceLock<TlsChoice> = OnceLock::new();

/// The pure decision: graviola when the CPU check passes, else refuse.
pub fn choose(cpu_supported: bool) -> TlsChoice {
    if cpu_supported {
        TlsChoice::Graviola
    } else {
        TlsChoice::UnsupportedCpu
    }
}

/// Runs the CPU check once per process and installs graviola when it
/// passes. Later calls return the first answer and install nothing.
/// `cpu_supported` is `graviola::util::is_cpu_supported` in the app; tests
/// pass a stub.
pub fn install_with(cpu_supported: impl FnOnce() -> bool) -> TlsChoice {
    *CHOICE.get_or_init(|| {
        let choice = choose(cpu_supported());
        match choice {
            TlsChoice::Graviola => {
                if rustls_graviola::default_provider().install_default().is_err() {
                    // Nothing in this process installs a provider before
                    // this runs, so this means a code path changed. Say so:
                    // that provider, not graviola, would carry the TLS.
                    eprintln!("[boot] tls: a rustls provider was already installed before graviola; graviola was not installed");
                } else {
                    eprintln!("[boot] tls: provider graviola (rustls-graviola), CPU check passed");
                }
            }
            TlsChoice::UnsupportedCpu => {
                eprintln!("[boot] tls: this CPU lacks what graviola needs; no TLS provider installed, refusing to start");
            }
        }
        choice
    })
}

/// The real CPU check: graviola's own, plus the x86_64 features its code
/// uses that 0.4.4's check leaves out. Its `low/x86_64/cpu.rs` tests bmi1,
/// but the bignum code uses BMI2 (`mulx`) and ML-KEM uses BMI2 `pext` and
/// POPCNT. No real CPU has ADX and AVX2 without them, but a VM can hide
/// them, so check here too (review of the 0.4.3 to 0.4.4 diff, 11-05).
pub fn cpu_supported() -> bool {
    graviola::util::is_cpu_supported() && extra_cpu_features()
}

#[cfg(target_arch = "x86_64")]
fn extra_cpu_features() -> bool {
    std::arch::is_x86_feature_detected!("bmi2") && std::arch::is_x86_feature_detected!("popcnt")
}

#[cfg(not(target_arch = "x86_64"))]
fn extra_cpu_features() -> bool {
    true
}

/// The real check ([`cpu_supported`]) and install.
pub fn install() -> TlsChoice {
    install_with(cpu_supported)
}

/// What startup decided, if it ran yet.
pub fn active() -> Option<TlsChoice> {
    CHOICE.get().copied()
}

/// Startup's TLS step with the check and the refusal passed in, so tests
/// can stub both. On an unsupported CPU, `refuse` gets the title and the
/// message (the app shows them in a native dialog) and this returns false:
/// the caller exits without touching the network.
pub fn startup_with(cpu_supported: impl FnOnce() -> bool, refuse: impl FnOnce(&str, &str)) -> bool {
    match install_with(cpu_supported) {
        TlsChoice::Graviola => true,
        TlsChoice::UnsupportedCpu => {
            refuse(UNSUPPORTED_CPU_TITLE, UNSUPPORTED_CPU_MESSAGE);
            false
        }
    }
}

/// `main`'s and `run`'s first step: install graviola, or tell the person
/// this CPU isn't supported and exit (status 1) before any TLS. Never
/// returns on an unsupported CPU.
pub fn install_or_exit() {
    if !startup_with(cpu_supported, show_unsupported_cpu_message) {
        std::process::exit(1);
    }
}

/// A native message with no TLS and no Tauri: the window stack isn't up
/// yet, and nothing here may build an HTTP client. stderr always gets it
/// too, for a terminal launch and the logs.
fn show_unsupported_cpu_message(title: &str, message: &str) {
    eprintln!("[boot] {title}: {message}");
    native_alert(title, message);
}

#[cfg(target_os = "macos")]
fn native_alert(title: &str, message: &str) {
    // `display alert` from Standard Additions; osascript ships with macOS.
    // The title and message are constants with no quotes or backslashes.
    let script = format!(
        "display alert \"{title}\" message \"{message}\" as critical buttons {{\"Quit\"}} default button \"Quit\""
    );
    if let Err(e) = std::process::Command::new("/usr/bin/osascript").arg("-e").arg(script).status() {
        eprintln!("[boot] tls: could not show the alert (osascript: {e})");
    }
}

#[cfg(windows)]
fn native_alert(title: &str, message: &str) {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let title = HSTRING::from(title);
    let message = HSTRING::from(message);
    // SAFETY: both strings are NUL-terminated wide strings that outlive the call.
    unsafe {
        MessageBoxW(None, PCWSTR(message.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn native_alert(title: &str, message: &str) {
    // The usual desktop dialog tools, first one present wins. With neither,
    // stderr (above) is all there is.
    let tries: [(&str, Vec<&str>); 2] = [
        ("zenity", vec!["--error", "--title", title, "--text", message]),
        ("kdialog", vec!["--title", title, "--error", message]),
    ];
    for (tool, args) in tries {
        if std::process::Command::new(tool).args(&args).status().is_ok() {
            return;
        }
    }
    eprintln!("[boot] tls: no zenity or kdialog to show the message in a window");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_supported_cpu_chooses_graviola() {
        assert_eq!(choose(true), TlsChoice::Graviola);
    }

    #[test]
    fn an_unsupported_cpu_refuses_with_no_fallback() {
        assert_eq!(choose(false), TlsChoice::UnsupportedCpu);
    }

    #[test]
    fn the_message_says_the_processor_is_not_supported() {
        assert!(UNSUPPORTED_CPU_MESSAGE.starts_with("This computer's processor isn't supported."));
        // osascript gets these inside double quotes.
        for text in [UNSUPPORTED_CPU_TITLE, UNSUPPORTED_CPU_MESSAGE] {
            assert!(!text.contains('"') && !text.contains('\\'), "{text}");
        }
    }
}
