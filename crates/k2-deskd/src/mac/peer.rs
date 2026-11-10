//! The peer check on deskd's socket: read the connecting process's audit
//! token (`LOCAL_PEERTOKEN`) and check its code signature against a fixed
//! requirement (K2's team and `dev.k2.app.daemon`).
//!
//! **Ordinary hygiene, not a boundary.** It keeps non-K2 programs from
//! talking to deskd. Same-user code can still reach the screen (run a
//! second signed daemon, mint a login, …); Rosson accepted that for v1
//! ("agent inception", 2026-11-10), and isolation is the 0.49 arc.
//!
//! The audit token names the exact process (pid + pid version), so a pid
//! reused after the peer exits can't pass as it.

use std::ffi::c_void;
use std::os::fd::AsRawFd;

use super::ffi::*;

const SOL_LOCAL: libc::c_int = 0;
const LOCAL_PEERTOKEN: libc::c_int = 0x006;
const ERR_SEC_CS_REQ_FAILED: OSStatus = -67050;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AuditToken {
    val: [u32; 8],
}

fn peer_audit_token(sock: &impl AsRawFd) -> Result<AuditToken, String> {
    let mut tok = AuditToken::default();
    let mut len = std::mem::size_of::<AuditToken>() as libc::socklen_t;
    // SAFETY: tok is a properly sized out buffer.
    let rc = unsafe { libc::getsockopt(sock.as_raw_fd(), SOL_LOCAL, LOCAL_PEERTOKEN, (&mut tok as *mut AuditToken).cast(), &mut len) };
    if rc != 0 {
        return Err(format!("LOCAL_PEERTOKEN: {}", std::io::Error::last_os_error()));
    }
    if len as usize != std::mem::size_of::<AuditToken>() {
        return Err(format!("LOCAL_PEERTOKEN returned {len} bytes"));
    }
    Ok(tok)
}

/// `Ok(())` if the process on the other end of `sock` satisfies the code
/// requirement `requirement` (the code signing requirement language).
pub fn check_peer(sock: &impl AsRawFd, requirement: &str) -> Result<(), String> {
    let tok = peer_audit_token(sock)?;
    // SAFETY: every CF object is created here and released by `Cf`; the
    // pointers handed to Security live for the call.
    unsafe {
        let data = Cf(CFDataCreate(kCFAllocatorDefault, (&tok as *const AuditToken).cast(), std::mem::size_of::<AuditToken>() as CFIndex));
        if data.0.is_null() {
            return Err("CFDataCreate failed".into());
        }
        let keys = [kSecGuestAttributeAudit as *const c_void];
        let values = [data.0];
        let attrs = Cf(CFDictionaryCreate(kCFAllocatorDefault, keys.as_ptr(), values.as_ptr(), 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));
        if attrs.0.is_null() {
            return Err("CFDictionaryCreate failed".into());
        }
        let mut code: SecCodeRef = std::ptr::null_mut();
        let st = SecCodeCopyGuestWithAttributes(std::ptr::null_mut(), attrs.0, 0, &mut code);
        if st != 0 || code.is_null() {
            return Err(format!("SecCodeCopyGuestWithAttributes: OSStatus {st}"));
        }
        let code = Cf(code as CFTypeRef);
        let text = cf_string(requirement);
        let mut req: SecRequirementRef = std::ptr::null_mut();
        let st = SecRequirementCreateWithString(text.0, 0, &mut req);
        if st != 0 || req.is_null() {
            return Err(format!("bad code requirement ({st}): {requirement}"));
        }
        let req = Cf(req as CFTypeRef);
        let st = SecCodeCheckValidity(code.0 as SecCodeRef, 0, req.0 as SecRequirementRef);
        match st {
            0 => Ok(()),
            ERR_SEC_CS_REQ_FAILED => Err("the peer does not satisfy deskd's code requirement".into()),
            other => Err(format!("SecCodeCheckValidity: OSStatus {other}")),
        }
    }
}

/// The requirement this build of deskd holds its peer to.
pub fn requirement() -> String {
    #[cfg(debug_assertions)]
    {
        // A debug deskd also serves a daemon signed for development
        // (vs-live R7). The marker lets the release check refuse a debug
        // deskd that slipped into a bundle.
        let _marker = std::hint::black_box(k2_deskd_proto::DEV_REQUIREMENT_MARKER);
        format!("({}) or ({})", k2_deskd_proto::release_peer_requirement(), k2_deskd_proto::dev_peer_requirement())
    }
    #[cfg(not(debug_assertions))]
    {
        k2_deskd_proto::release_peer_requirement()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_test_process_passes_always_and_fails_the_release_requirement() {
        let (a, _b) = UnixStream::pair().unwrap();
        // The peer of `a` is this very test process (ad hoc signed by the
        // linker): it is valid code, but not K2's signed daemon.
        check_peer(&a, "always").expect("any valid code satisfies `always`");
        let e = check_peer(&a, &k2_deskd_proto::release_peer_requirement()).unwrap_err();
        assert!(e.contains("code requirement"), "{e}");
        let e = check_peer(&a, &k2_deskd_proto::dev_peer_requirement()).unwrap_err();
        assert!(e.contains("code requirement"), "{e}");
        assert!(check_peer(&a, "never").is_err());
    }

    #[test]
    fn a_malformed_requirement_is_an_error_not_a_pass() {
        let (a, _b) = UnixStream::pair().unwrap();
        let e = check_peer(&a, "identifier ((( nonsense").unwrap_err();
        assert!(e.starts_with("bad code requirement"), "{e}");
    }

    #[test]
    fn the_release_requirement_names_the_team_and_daemon() {
        let r = k2_deskd_proto::release_peer_requirement();
        assert!(r.contains("36B8R93HXV") && r.contains("dev.k2.app.daemon") && r.contains("anchor apple generic"));
    }
}
