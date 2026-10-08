//! macOS owner confirm (KV7a): the daemon asks LocalAuthentication
//! (`deviceOwnerAuthentication`). macOS draws the Touch ID / login
//! password sheet; the reason names the action and the secret. Blocks the
//! calling worker thread until the person answers (or 120 s pass).
//!
//! Verified on the signed build (§22.6 smoke #1). Tests use the fake in
//! `vault_routes` (no real sheet is ever shown by `cargo test`).

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LAContext, LAPolicy};

/// Ask the person at this Mac to approve `reason`. `Ok(())` only when
/// they authenticated.
pub fn confirm(reason: &str) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<bool>();
    // SAFETY: LAContext is created and used from this thread; the reply
    // block only sends on a channel.
    unsafe {
        let ctx = LAContext::new();
        if ctx.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication).is_err() {
            return Err("this Mac can't show a Touch ID or password prompt right now".into());
        }
        let reason = NSString::from_str(reason);
        let block = RcBlock::new(move |ok: Bool, _err: *mut NSError| {
            let _ = tx.send(ok.as_bool());
        });
        ctx.evaluatePolicy_localizedReason_reply(LAPolicy::DeviceOwnerAuthentication, &reason, &block);
        match rx.recv_timeout(Duration::from_secs(120)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("not approved".into()),
            Err(_) => {
                ctx.invalidate();
                Err("no answer within two minutes".into())
            }
        }
    }
}
