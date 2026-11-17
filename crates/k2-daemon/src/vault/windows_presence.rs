//! Windows: the computer's own user check for the vault's first-setup
//! steps (see `vault_routes::presence`).
//!
//! Windows Hello (`UserConsentVerifier`, its prompt over the window in
//! front). When Hello isn't set up on this PC, Windows' own credential
//! prompt for the signed-in user (`CredUIPromptForWindowsCredentialsW`):
//! the answer is checked with `LogonUserW` and must be this process's own
//! user. Blocks the calling worker thread until the person answers (or 120 s
//! pass). Tests use the fake in `vault_routes::presence` (no real prompt is
//! ever shown by `cargo test`).

use std::sync::mpsc;
use std::time::Duration;

use windows::core::{factory, HSTRING, PCWSTR, PWSTR};
use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::Credentials::{
    CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW, CREDUIWIN_ENUMERATE_CURRENT_USER, CREDUI_INFOW,
    CRED_PACK_PROTECTED_CREDENTIALS,
};
use windows::Win32::Security::{
    EqualSid, GetTokenInformation, LogonUserW, TokenUser, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows_future::IAsyncOperation;

const WAIT: Duration = Duration::from_secs(120);
/// `ERROR_CANCELLED`: the person closed the credential prompt.
const ERROR_CANCELLED: u32 = 1223;

/// What the person (or Windows) answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Approved,
    /// The prompt showed and the person didn't approve (or it timed out).
    Declined(String),
    /// No prompt could be shown on this PC right now.
    Unavailable(String),
}

/// Ask the person at this PC to approve `reason`: Windows Hello, else the
/// signed-in user's own Windows credentials.
pub fn confirm(reason: &str) -> Answer {
    match hello_available() {
        Ok(true) => hello(reason),
        _ => credentials(reason),
    }
}

fn hello_available() -> windows::core::Result<bool> {
    let op = UserConsentVerifier::CheckAvailabilityAsync()?;
    Ok(op.join()? == UserConsentVerifierAvailability::Available)
}

fn hello(reason: &str) -> Answer {
    let started = (|| -> windows::core::Result<IAsyncOperation<UserConsentVerificationResult>> {
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
        // SAFETY: a plain window handle (possibly null) and an owned HSTRING.
        unsafe { interop.RequestVerificationForWindowAsync(GetForegroundWindow(), &HSTRING::from(reason)) }
    })();
    let op = match started {
        Ok(op) => op,
        Err(e) => return Answer::Unavailable(format!("Windows Hello couldn't start: {e}")),
    };
    let (tx, rx) = mpsc::channel();
    let waiting = op.clone();
    std::thread::spawn(move || {
        let _ = tx.send(waiting.join());
    });
    match rx.recv_timeout(WAIT) {
        Ok(Ok(r)) if r == UserConsentVerificationResult::Verified => Answer::Approved,
        Ok(Ok(r)) if r == UserConsentVerificationResult::DeviceNotPresent || r == UserConsentVerificationResult::NotConfiguredForUser => {
            credentials(reason)
        }
        Ok(Ok(_)) => Answer::Declined("not approved".into()),
        Ok(Err(e)) => Answer::Declined(format!("Windows Hello: {e}")),
        Err(_) => {
            let _ = op.Cancel();
            Answer::Declined("no answer within two minutes".into())
        }
    }
}

/// The signed-in user's own Windows credentials, checked by Windows.
fn credentials(reason: &str) -> Answer {
    let caption = HSTRING::from(k2_core::brand::PRODUCT_NAME);
    let message = HSTRING::from(reason);
    let info = CREDUI_INFOW {
        cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
        // SAFETY: no arguments; returns a handle or null.
        hwndParent: unsafe { GetForegroundWindow() },
        pszMessageText: PCWSTR(message.as_ptr()),
        pszCaptionText: PCWSTR(caption.as_ptr()),
        hbmBanner: Default::default(),
    };
    let mut package = 0u32;
    let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut out_len = 0u32;
    // SAFETY: every pointer is to a live local; Windows allocates `out`.
    let rc = unsafe {
        CredUIPromptForWindowsCredentialsW(Some(&info as *const CREDUI_INFOW), 0, &mut package, None, 0, &mut out, &mut out_len, None, CREDUIWIN_ENUMERATE_CURRENT_USER)
    };
    if rc == ERROR_CANCELLED {
        return Answer::Declined("not approved".into());
    }
    if rc != 0 || out.is_null() {
        return Answer::Unavailable(format!("Windows couldn't show its sign-in prompt (error {rc})"));
    }
    let checked = check_buffer(out, out_len);
    // SAFETY: `out` was allocated by CredUI with CoTaskMemAlloc; it is wiped
    // (it holds the password) and freed once.
    unsafe {
        std::ptr::write_bytes(out as *mut u8, 0, out_len as usize);
        CoTaskMemFree(Some(out as *const core::ffi::c_void));
    }
    match checked {
        Ok(true) => Answer::Approved,
        Ok(false) => Answer::Declined("those aren't this PC's signed-in user's credentials".into()),
        Err(e) => Answer::Declined(format!("Windows didn't accept those credentials: {e}")),
    }
}

/// Unpack the prompt's answer, log it on, and compare its user with ours.
fn check_buffer(buf: *mut core::ffi::c_void, len: u32) -> windows::core::Result<bool> {
    let mut user = vec![0u16; 514];
    let mut domain = vec![0u16; 338];
    let mut pass = vec![0u16; 1024];
    let (mut ul, mut dl, mut pl) = (user.len() as u32, domain.len() as u32, pass.len() as u32);
    // SAFETY: the buffers are sized as passed; CredUI writes NUL-terminated
    // strings into them.
    let unpacked = unsafe {
        CredUnPackAuthenticationBufferW(
            CRED_PACK_PROTECTED_CREDENTIALS,
            buf,
            len,
            Some(PWSTR(user.as_mut_ptr())),
            &mut ul,
            Some(PWSTR(domain.as_mut_ptr())),
            Some(&mut dl as *mut u32),
            Some(PWSTR(pass.as_mut_ptr())),
            &mut pl,
        )
    };
    let result = unpacked.and_then(|()| {
        let dom = if domain[0] == 0 { PCWSTR::null() } else { PCWSTR(domain.as_ptr()) };
        let mut token = HANDLE::default();
        // SAFETY: NUL-terminated strings from CredUnPack; `token` is ours to close.
        unsafe { LogonUserW(PCWSTR(user.as_ptr()), dom, PCWSTR(pass.as_ptr()), LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &mut token)? };
        let same = same_user(token);
        // SAFETY: the handle LogonUserW returned, closed once.
        unsafe {
            let _ = CloseHandle(token);
        }
        same
    });
    pass.iter_mut().for_each(|c| *c = 0);
    result
}

/// Does `token` belong to the same user as this process?
fn same_user(token: HANDLE) -> windows::core::Result<bool> {
    let mut own = HANDLE::default();
    // SAFETY: the pseudo handle of this process; `own` is ours to close.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut own)? };
    let a = token_user(token);
    let b = token_user(own);
    // SAFETY: the handle OpenProcessToken returned, closed once.
    unsafe {
        let _ = CloseHandle(own);
    }
    let (a, b) = (a?, b?);
    // SAFETY: both buffers start with a TOKEN_USER whose SID points into
    // the same buffer, which outlives the call.
    let eq = unsafe {
        let ta = &*(a.as_ptr() as *const TOKEN_USER);
        let tb = &*(b.as_ptr() as *const TOKEN_USER);
        EqualSid(ta.User.Sid, tb.User.Sid)
    };
    Ok(eq.is_ok())
}

/// A token's TOKEN_USER (and the SID after it), in a u64-aligned buffer.
fn token_user(token: HANDLE) -> windows::core::Result<Vec<u64>> {
    let mut need = 0u32;
    // SAFETY: a size query (no buffer); it fails with the size it needs.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut need) };
    let mut buf = vec![0u64; (need as usize).div_ceil(8).max(1)];
    // SAFETY: `buf` holds at least `need` bytes.
    unsafe { GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), (buf.len() * 8) as u32, &mut need)? };
    Ok(buf)
}
