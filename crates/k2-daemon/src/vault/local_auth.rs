//! macOS owner confirm (KV7a): the daemon asks LocalAuthentication
//! (`deviceOwnerAuthentication`). macOS draws the Touch ID / login
//! password sheet; the reason names the action and the secret. Blocks the
//! calling worker thread until the person answers (or 120 s pass).
//!
//! Verified on the signed build (§22.6 smoke #1). Tests use the fake in
//! `vault_routes` (no real sheet is ever shown by `cargo test`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LAContext, LAPolicy};

/// The person's answer at this Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Approved,
    /// The person said no (Cancel, a wrong password).
    Declined(String),
    /// No answer can come from this Mac right now: no one signed in at its
    /// screen, an SSH-only session, or no one answered in two minutes.
    Unavailable(String),
}

/// LAError codes that mean nobody can answer here (LAError.h).
const LA_ERROR_NOT_INTERACTIVE: isize = -1004;
const LA_ERROR_SYSTEM_CANCEL: isize = -4;
const LA_ERROR_PASSCODE_NOT_SET: isize = -5;

/// Ask the person at this Mac to approve `reason`.
pub fn ask(reason: &str) -> Answer {
    ask_until(reason, &AtomicBool::new(false))
}

/// [`ask`], closing the sheet as soon as `stop` is set (0.46.17: the
/// owner answered the same step on their phone first). A sheet closed that
/// way is `Unavailable`; the caller already has its answer.
pub fn ask_until(reason: &str, stop: &AtomicBool) -> Answer {
    let (tx, rx) = mpsc::channel::<(bool, Option<isize>)>();
    // SAFETY: LAContext is created and used from this thread; the reply
    // block only reads the error's code and sends on a channel.
    unsafe {
        let ctx = LAContext::new();
        if ctx.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication).is_err() {
            return Answer::Unavailable("this Mac can't show a Touch ID or password prompt right now (no one is signed in at its screen?)".into());
        }
        let reason = NSString::from_str(reason);
        let block = RcBlock::new(move |ok: Bool, e: *mut NSError| {
            let code = e.as_ref().map(|e| e.code());
            let _ = tx.send((ok.as_bool(), code));
        });
        ctx.evaluatePolicy_localizedReason_reply(LAPolicy::DeviceOwnerAuthentication, &reason, &block);
        let deadline = Instant::now() + Duration::from_secs(120);
        let got = loop {
            if stop.load(Ordering::SeqCst) {
                ctx.invalidate();
                return Answer::Unavailable("answered on your phone first".into());
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(x) => break Ok(x),
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(e) => break Err(e),
            }
        };
        match got {
            Ok((true, _)) => Answer::Approved,
            Ok((false, Some(LA_ERROR_NOT_INTERACTIVE | LA_ERROR_SYSTEM_CANCEL | LA_ERROR_PASSCODE_NOT_SET))) => {
                Answer::Unavailable("this Mac couldn't show its Touch ID or password prompt (no one at its screen?)".into())
            }
            Ok((false, _)) => Answer::Declined("not approved".into()),
            Err(_) => {
                ctx.invalidate();
                Answer::Unavailable("no one answered this Mac's prompt within two minutes".into())
            }
        }
    }
}

/// [`ask`] for the confirms that only need yes or no.
pub fn confirm(reason: &str) -> Result<(), String> {
    match ask(reason) {
        Answer::Approved => Ok(()),
        Answer::Declined(h) | Answer::Unavailable(h) => Err(h),
    }
}
