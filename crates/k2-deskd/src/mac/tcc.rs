//! Screen Recording and Accessibility for "K2 Screen" (§3.2 TCC flow).
//!
//! - **Preflight:** `CGPreflightScreenCaptureAccess()` and
//!   `AXIsProcessTrusted()`.
//! - **Request:** `CGRequestScreenCaptureAccess()` and
//!   `AXIsProcessTrustedWithOptions({prompt: true})`, then open the System
//!   Settings pane for what is still missing. Someone at the Mac clicks.
//! - **Re-check every 5 s** (server::TCC_RECHECK).
//!
//! **Why a probe child.** The screen-capture preflight can keep returning a
//! process's first answer until it restarts (seen on 10.15–14; to confirm
//! on the minis, D0). So while Screen Recording reads "no", deskd asks a
//! fresh copy of itself (`k2-deskd --tcc-probe`, which only prints the two
//! flags). A child of deskd has deskd as its responsible process, so TCC
//! answers for K2 Screen.

use std::process::Command;
use std::time::Duration;

use k2_deskd_proto::msg::Tcc;

use super::ffi::*;

pub const PANE_SCREEN: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
pub const PANE_ACCESSIBILITY: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// This process's own answer (what `--tcc-probe` prints).
pub fn preflight_self() -> Tcc {
    // SAFETY: plain C calls with no arguments.
    unsafe { Tcc { required: true, screen: CGPreflightScreenCaptureAccess(), accessibility: AXIsProcessTrusted() != 0 } }
}

/// `--tcc-probe`'s output line.
pub fn probe_line() -> String {
    let t = preflight_self();
    serde_json::json!({"screen": t.screen, "accessibility": t.accessibility}).to_string()
}

pub fn parse_probe(out: &str) -> Option<Tcc> {
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    Some(Tcc { required: true, screen: v.get("screen")?.as_bool()?, accessibility: v.get("accessibility")?.as_bool()? })
}

/// The live answer: this process's, or a fresh probe's while Screen
/// Recording reads "no" here.
pub fn current() -> Tcc {
    let mine = preflight_self();
    if mine.screen {
        return mine;
    }
    match probe_child(Duration::from_secs(3)) {
        Some(t) => Tcc { accessibility: mine.accessibility || t.accessibility, ..t },
        None => mine,
    }
}

fn probe_child(timeout: Duration) -> Option<Tcc> {
    let exe = std::env::current_exe().ok()?;
    let mut child = Command::new(exe)
        .arg("--tcc-probe")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_probe(&out)
}

/// Fires the prompts and opens the panes for what's missing. Returns the
/// state right after (usually still missing: the person hasn't clicked).
pub fn request() -> Tcc {
    let before = current();
    // SAFETY: CF objects created here are released by `Cf`; the keys are
    // framework statics.
    unsafe {
        if !before.screen {
            CGRequestScreenCaptureAccess();
        }
        if !before.accessibility {
            let keys = [kAXTrustedCheckOptionPrompt as *const std::ffi::c_void];
            let values = [kCFBooleanTrue as *const std::ffi::c_void];
            let opts = Cf(CFDictionaryCreate(kCFAllocatorDefault, keys.as_ptr(), values.as_ptr(), 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));
            AXIsProcessTrustedWithOptions(opts.0);
        }
    }
    if !before.screen {
        open_pane(PANE_SCREEN);
    } else if !before.accessibility {
        open_pane(PANE_ACCESSIBILITY);
    }
    current()
}

fn open_pane(url: &str) {
    let _ = Command::new("/usr/bin/open").arg(url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_output_round_trips() {
        assert_eq!(parse_probe("{\"screen\":true,\"accessibility\":false}\n"), Some(Tcc { required: true, screen: true, accessibility: false }));
        assert_eq!(parse_probe("garbage"), None);
        assert_eq!(parse_probe("{\"screen\":true}"), None);
    }
}
