//! macOS keychain generic passwords, written without a size limit and
//! without the secret on any process's argv.
//!
//! ## Why not just `/usr/bin/security`
//!
//! The `security` CLI has two ways to take a secret, and neither is safe
//! for large items:
//! - `security -i` (interactive mode, secret on stdin) reads each line
//!   into a ~4096-character buffer and splits longer lines: the rest is
//!   run as a second command. A Claude credential with MCP OAuth tokens
//!   is 2,000+ bytes, so its hex is longer than that, and the first
//!   chunk was actually written before the error (0.45.0 field bug).
//! - `-w <secret>` / `-X <hex>` on argv has no limit but shows the
//!   secret in `ps` to every user on the machine. (Claude Code itself
//!   falls back to argv above 4,032 characters.)
//! - `-w` as the last argument prompts instead, but only for the default
//!   keychain and through a tty.
//!
//! ## What this module does
//!
//! - **Existing item:** find it by service + account WITHOUT fetching
//!   its data, then `SecKeychainItemModifyAttributesAndData` in process.
//!   Verified on a throwaway keychain (2026-10-08): an item made by
//!   `security` (how Claude Code makes `Claude Code-credentials`) has
//!   the ACL `encrypt: any application; decrypt: /usr/bin/security`.
//!   Changing its data from another binary needs only `encrypt`, so it
//!   succeeds with no prompt, and the ACL is unchanged afterwards:
//!   Claude Code's own `security find-generic-password` reads and its
//!   `security -i add-generic-password -U` updates keep working. Sizes
//!   up to 64 KiB were checked.
//! - **Missing item:** created by `/usr/bin/security -i` with a short
//!   non-secret placeholder (so the item gets exactly the ACL and
//!   partition list Claude Code would give it, plus any `-T` trusted
//!   apps a caller asks for), then its data is set in process as above.
//!   An item created through Security.framework instead would trust only
//!   the creating binary, and every later `security` read (Claude's)
//!   would prompt.
//! - **Read:** always through `/usr/bin/security find-generic-password
//!   -g`, never Security.framework: `/usr/bin/security` is the trusted
//!   reader on these items, and a framework read from `k2-daemon` would
//!   prompt. `-g` prints `password: 0x<HEX>` for anything not plainly
//!   printable and `password: "<text>"` otherwise, so the parse is
//!   unambiguous (`-w` prints bare hex OR bare text).
//! - **Every write is read back and compared** before it returns `Ok`.
//! - Security.framework calls run with keychain user interaction turned
//!   off, so a locked keychain fails fast with a clear error instead of
//!   a dialog from a background process.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Mutex;

use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::CFString;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework_sys::base::{errSecSuccess, SecCopyErrorMessageString, SecKeychainItemRef, SecKeychainRef};
use security_framework_sys::keychain::{
    SecKeychainCopyDefault, SecKeychainFindGenericPassword, SecKeychainGetUserInteractionAllowed,
    SecKeychainOpen, SecKeychainSetUserInteractionAllowed,
};
use security_framework_sys::keychain_item::SecKeychainItemModifyAttributesAndData;

const SECURITY: &str = "/usr/bin/security";

/// `security -i` splits input lines longer than its ~4096-byte buffer.
/// Claude Code uses 4,032 as its own cut-off; stay under it.
pub const INTERACTIVE_LINE_MAX: usize = 4032;

/// The non-secret value a new item holds for the instant between its
/// creation and the real data being set.
const PLACEHOLDER: &[u8] = b"{}";

// Not in security-framework-sys.
#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
}
const K_SEC_UNLOCK_STATE_STATUS: u32 = 1;

// ── Errors ──────────────────────────────────────────────────────────

/// A keychain failure. Its text never contains secret bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    /// A Security.framework call or the `security` CLI failed. `status`
    /// is the OSStatus when known (the CLI's exit code is the low byte
    /// of it and is mapped back for known codes).
    Status { op: &'static str, status: Option<i32>, exit: Option<i32>, detail: String },
    /// `security` could not be started.
    Spawn(String),
    /// The keychain file passed in does not exist.
    NoSuchKeychain(PathBuf),
    /// A service/account/label/path K2 can't pass safely to `security -i`.
    BadField(&'static str),
    /// The data read back after a write is not what was written.
    Verify(String),
}

/// Known OSStatus codes: (OSStatus, `security` exit code, meaning, is
/// it a locked/refused keychain).
const KNOWN: &[(i32, i32, &str, bool)] = &[
    (-25308, 36, "the keychain needs user interaction this background process can't show (errSecInteractionNotAllowed): the keychain is locked", true),
    (-25293, 51, "the keychain refused access (errSecAuthFailed): it is locked, or this program isn't on the item's access list", true),
    (-25300, 44, "the item was not found (errSecItemNotFound)", false),
    (-25299, 45, "the item already exists (errSecDuplicateItem)", false),
    (-128, 128, "the keychain prompt was canceled (errSecUserCanceled)", false),
    (-25294, 50, "the keychain does not exist (errSecNoSuchKeychain)", false),
    (-25295, 49, "the keychain is not valid (errSecInvalidKeychain)", false),
    (-25292, 52, "the keychain is read-only (errSecReadOnly)", false),
    (-25291, 53, "no keychain is available (errSecNotAvailable)", false),
    (-25307, 37, "there is no default keychain (errSecNoDefaultKeychain)", false),
    (-61, 195, "write permission denied (errSecWrPerm)", false),
    (-34, 222, "the disk is full (errSecDiskFull)", false),
    (-50, 206, "invalid parameter (errSecParam)", false),
];

/// OSStatus for a `security` exit code, when the code is a known one.
pub fn status_for_exit(exit: i32) -> Option<i32> {
    KNOWN.iter().find(|k| k.1 == exit).map(|k| k.0)
}

fn known(status: i32) -> Option<(&'static str, bool)> {
    KNOWN.iter().find(|k| k.0 == status).map(|k| (k.2, k.3))
}

impl KeychainError {
    /// The keychain is locked (or refused access) — the only case where
    /// "unlock the login keychain" is the right advice.
    pub fn is_locked(&self) -> bool {
        matches!(self, KeychainError::Status { status: Some(s), .. } if known(*s).map(|k| k.1).unwrap_or(false))
    }
    pub fn is_not_found(&self) -> bool {
        matches!(self, KeychainError::Status { status: Some(-25300), .. })
    }
    fn status(op: &'static str, status: i32) -> KeychainError {
        KeychainError::Status { op, status: Some(status), exit: None, detail: String::new() }
    }
    fn cli(op: &'static str, exit: Option<i32>, stderr: &[u8], secret: Option<&[u8]>) -> KeychainError {
        KeychainError::Status {
            op,
            status: exit.and_then(status_for_exit),
            exit,
            detail: scrub(&String::from_utf8_lossy(stderr), secret),
        }
    }
}

impl std::fmt::Display for KeychainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeychainError::Status { op, status, exit, detail } => {
                write!(f, "{op} failed: ")?;
                match status {
                    Some(s) => match known(*s) {
                        Some((meaning, _)) => write!(f, "{meaning} (OSStatus {s})")?,
                        None => write!(f, "{} (OSStatus {s})", os_message(*s))?,
                    },
                    None => match exit {
                        Some(c) => write!(f, "security exited {c}")?,
                        None => write!(f, "security was killed by a signal")?,
                    },
                }
                if !detail.trim().is_empty() {
                    write!(f, "; security said: {}", detail.trim())?;
                }
                if self.is_locked() {
                    write!(f, "; unlock the login keychain on this Mac and try again")?;
                }
                Ok(())
            }
            KeychainError::Spawn(e) => write!(f, "could not run {SECURITY}: {e}"),
            KeychainError::NoSuchKeychain(p) => write!(f, "the keychain {} does not exist", p.display()),
            KeychainError::BadField(what) => write!(f, "the keychain {what} contains a character K2 can't pass to security"),
            KeychainError::Verify(why) => write!(f, "the keychain item did not read back as written ({why})"),
        }
    }
}

impl std::error::Error for KeychainError {}

fn os_message(status: i32) -> String {
    unsafe {
        let s = SecCopyErrorMessageString(status, ptr::null_mut());
        if s.is_null() {
            return "keychain error".to_string();
        }
        CFString::wrap_under_create_rule(s).to_string()
    }
}

/// Strip anything secret-shaped from `security`'s stderr before it goes
/// into an error: the secret itself, its hex (`security -i` echoes the
/// rest of a split line), and any long run of token characters that
/// contains a digit. Capped at 300 characters.
pub fn scrub(text: &str, secret: Option<&[u8]>) -> String {
    let mut s = text.to_string();
    if let Some(sec) = secret {
        if sec.len() >= 4 {
            let lower: String = sec.iter().map(|b| format!("{b:02x}")).collect();
            s = s.replace(&lower, "[redacted]").replace(&lower.to_uppercase(), "[redacted]");
            if let Ok(t) = std::str::from_utf8(sec) {
                s = s.replace(t, "[redacted]");
            }
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 16 && run.chars().any(|c| c.is_ascii_digit()) {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-' | '.') {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    let out = out.replace('\n', " ");
    let out = out.trim();
    if out.chars().count() > 300 {
        format!("{}…", out.chars().take(300).collect::<String>())
    } else {
        out.to_string()
    }
}

// ── Options ─────────────────────────────────────────────────────────

/// How to write an item.
#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    /// A specific keychain file; `None` = the user's search list (find)
    /// and default keychain (create), which is what Claude Code uses.
    pub keychain: Option<PathBuf>,
    /// Label for a newly created item (`-l`).
    pub label: Option<String>,
    /// Trusted applications for a newly created item (`-T`, one each).
    /// Empty = `security`'s default (trusts `/usr/bin/security` only).
    pub trusted_apps: Vec<String>,
    /// Delete any existing item first so a new one is created with this
    /// ACL (`-U` would keep the old ACL).
    pub replace_acl: bool,
}

fn check_field(what: &'static str, v: &str) -> Result<(), KeychainError> {
    if v.chars().any(|c| matches!(c, '"' | '\\' | '\n' | '\r' | '\0')) {
        return Err(KeychainError::BadField(what));
    }
    Ok(())
}

fn check_keychain(kc: Option<&Path>) -> Result<(), KeychainError> {
    if let Some(p) = kc {
        if !p.exists() {
            return Err(KeychainError::NoSuchKeychain(p.to_path_buf()));
        }
        check_field("path", &p.to_string_lossy())?;
    }
    Ok(())
}

// ── Security.framework plumbing ─────────────────────────────────────

/// Serializes K2's Security.framework calls and turns keychain user
/// interaction off for their duration (restoring the previous setting).
struct NoUi {
    prev: u8,
    _g: std::sync::MutexGuard<'static, ()>,
}

static FW_LOCK: Mutex<()> = Mutex::new(());

impl NoUi {
    fn enter() -> NoUi {
        let g = FW_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut prev: u8 = 1;
        unsafe {
            if SecKeychainGetUserInteractionAllowed(&mut prev) != errSecSuccess {
                prev = 1;
            }
            SecKeychainSetUserInteractionAllowed(0);
        }
        NoUi { prev, _g: g }
    }
}

impl Drop for NoUi {
    fn drop(&mut self) {
        unsafe {
            SecKeychainSetUserInteractionAllowed(self.prev);
        }
    }
}

/// An owned CF reference released on drop.
struct Owned(CFTypeRef);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn open_keychain(path: &Path) -> Result<Owned, KeychainError> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| KeychainError::BadField("path"))?;
    let mut kc: SecKeychainRef = ptr::null_mut();
    let st = unsafe { SecKeychainOpen(c.as_ptr(), &mut kc) };
    if st != errSecSuccess {
        return Err(KeychainError::status("open keychain", st));
    }
    Ok(Owned(kc as *const _))
}

/// Find an item's reference WITHOUT reading its data (reading the data
/// would need the item's decrypt ACL, i.e. a prompt for `k2-daemon`).
fn find_item(kc: Option<&Owned>, service: &str, account: &str) -> Result<Option<Owned>, KeychainError> {
    let arr;
    let search: CFTypeRef = match kc {
        Some(k) => {
            let kc = unsafe { SecKeychain::wrap_under_get_rule(k.0 as SecKeychainRef) };
            arr = CFArray::from_CFTypes(&[kc]);
            arr.as_CFTypeRef()
        }
        None => ptr::null(),
    };
    let mut item: SecKeychainItemRef = ptr::null_mut();
    let st = unsafe {
        SecKeychainFindGenericPassword(
            search,
            service.len() as u32,
            service.as_ptr().cast(),
            account.len() as u32,
            account.as_ptr().cast(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut item,
        )
    };
    match st {
        0 => Ok(Some(Owned(item as *const _))),
        -25300 => Ok(None),
        _ => Err(KeychainError::status("find keychain item", st)),
    }
}

fn set_item_data(item: &Owned, secret: &[u8]) -> Result<(), KeychainError> {
    let len = u32::try_from(secret.len()).map_err(|_| KeychainError::BadField("data size"))?;
    let st = unsafe {
        SecKeychainItemModifyAttributesAndData(item.0 as SecKeychainItemRef, ptr::null(), len, secret.as_ptr().cast())
    };
    if st != errSecSuccess {
        return Err(KeychainError::status("set keychain item data", st));
    }
    Ok(())
}

/// Refuse before `security` would raise an unlock dialog: a locked
/// keychain fails here with errSecInteractionNotAllowed.
fn ensure_unlocked(kc: Option<&Owned>) -> Result<(), KeychainError> {
    let default;
    let kref = match kc {
        Some(k) => k.0 as SecKeychainRef,
        None => {
            let mut d: SecKeychainRef = ptr::null_mut();
            let st = unsafe { SecKeychainCopyDefault(&mut d) };
            if st != errSecSuccess {
                return Err(KeychainError::status("find the default keychain", st));
            }
            default = Owned(d as *const _);
            default.0 as SecKeychainRef
        }
    };
    let mut status: u32 = 0;
    let st = unsafe { SecKeychainGetStatus(kref, &mut status) };
    if st != errSecSuccess {
        return Err(KeychainError::status("check keychain status", st));
    }
    if status & K_SEC_UNLOCK_STATE_STATUS == 0 {
        return Err(KeychainError::status("check keychain status", -25308));
    }
    Ok(())
}

// ── `security` CLI ──────────────────────────────────────────────────

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The `security -i` command and its stdin line that create a new item
/// holding [`PLACEHOLDER`]. The real secret never goes through here: it
/// is set in process afterwards. `pub(crate)` so tests can assert on the
/// exact argv and stdin.
pub(crate) fn placeholder_create(service: &str, account: &str, opts: &WriteOptions) -> Result<(Command, String), KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    let mut line = format!("add-generic-password -U -a \"{account}\" -s \"{service}\"");
    if let Some(l) = &opts.label {
        check_field("label", l)?;
        line.push_str(&format!(" -l \"{l}\""));
    }
    for app in &opts.trusted_apps {
        check_field("trusted app path", app)?;
        line.push_str(&format!(" -T \"{app}\""));
    }
    line.push_str(&format!(" -X \"{}\"", hex(PLACEHOLDER)));
    if let Some(kc) = &opts.keychain {
        check_field("path", &kc.to_string_lossy())?;
        line.push_str(&format!(" \"{}\"", kc.display()));
    }
    line.push('\n');
    if line.len() > INTERACTIVE_LINE_MAX {
        return Err(KeychainError::BadField("command (too long for security -i)"));
    }
    let mut cmd = Command::new(SECURITY);
    cmd.arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    Ok((cmd, line))
}

fn run_placeholder_create(service: &str, account: &str, opts: &WriteOptions) -> Result<(), KeychainError> {
    let (mut cmd, line) = placeholder_create(service, account, opts)?;
    let mut child = cmd.spawn().map_err(|e| KeychainError::Spawn(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(line.as_bytes()).map_err(|e| KeychainError::Spawn(format!("stdin: {e}")))?;
    }
    let out = child.wait_with_output().map_err(|e| KeychainError::Spawn(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(KeychainError::cli("create keychain item", out.status.code(), &out.stderr, None))
    }
}

/// Parse `security find-generic-password -g`'s `password:` line.
pub(crate) fn parse_g_password(stderr: &[u8]) -> Option<Vec<u8>> {
    // The first line that starts with "password: " (a quoted, printable
    // password never contains a newline). Work on bytes: the quoted form
    // carries the raw printable bytes.
    let rest = stderr.split(|&b| b == b'\n').find_map(|l| l.strip_prefix(b"password: "))?;
    if rest.is_empty() {
        return Some(Vec::new());
    }
    if let Some(h) = rest.strip_prefix(b"0x") {
        let end = h.iter().position(|b| !b.is_ascii_hexdigit()).unwrap_or(h.len());
        return decode_hex(std::str::from_utf8(&h[..end]).ok()?);
    }
    if rest.len() >= 2 && rest[0] == b'"' && rest[rest.len() - 1] == b'"' {
        return Some(rest[1..rest.len() - 1].to_vec());
    }
    None
}

/// The read command (argv carries only names, never data).
pub(crate) fn read_command(service: &str, account: &str, keychain: Option<&Path>) -> Command {
    let mut cmd = Command::new(SECURITY);
    cmd.args(["find-generic-password", "-a", account, "-s", service, "-g"]);
    if let Some(kc) = keychain {
        cmd.arg(kc);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

/// Read a generic password through `/usr/bin/security` (the trusted
/// reader on Claude's item). `Ok(None)` = no such item. No size limit.
pub fn read(service: &str, account: &str, keychain: Option<&Path>) -> Result<Option<Vec<u8>>, KeychainError> {
    check_keychain(keychain)?;
    let out = read_command(service, account, keychain)
        .output()
        .map_err(|e| KeychainError::Spawn(e.to_string()))?;
    if out.status.success() {
        return parse_g_password(&out.stderr)
            .map(Some)
            .ok_or_else(|| KeychainError::Verify("could not parse security's password line".into()));
    }
    if out.status.code() == Some(44) {
        return Ok(None);
    }
    // stderr here is `security: …` plus, on success only, the password
    // line, so it holds no secret; scrub anyway.
    Err(KeychainError::cli("read keychain item", out.status.code(), &out.stderr, None))
}

/// The delete command (argv carries only names).
fn delete_command(service: &str, account: &str, keychain: Option<&Path>) -> Command {
    let mut cmd = Command::new(SECURITY);
    cmd.args(["delete-generic-password", "-a", account, "-s", service]);
    if let Some(kc) = keychain {
        cmd.arg(kc);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

/// Delete a generic password. A missing item is not an error.
pub fn delete(service: &str, account: &str, keychain: Option<&Path>) -> Result<(), KeychainError> {
    check_keychain(keychain)?;
    let out = delete_command(service, account, keychain)
        .output()
        .map_err(|e| KeychainError::Spawn(e.to_string()))?;
    if out.status.success() || out.status.code() == Some(44) {
        Ok(())
    } else {
        Err(KeychainError::cli("delete keychain item", out.status.code(), &out.stderr, None))
    }
}

/// Create or update a generic password, then read it back and compare.
/// The secret goes only to Security.framework in this process: never
/// on argv, never on a child's stdin. Any size up to 4 GiB.
pub fn write(service: &str, account: &str, secret: &[u8], opts: &WriteOptions) -> Result<(), KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    let kc_path = opts.keychain.as_deref();
    check_keychain(kc_path)?;
    if opts.replace_acl {
        // Best effort: if the old item can't be deleted, updating its
        // data in place still stores the new value (with the old ACL).
        let _ = delete(service, account, kc_path);
    }
    let created = {
        let _ui = NoUi::enter();
        let kc = kc_path.map(open_keychain).transpose()?;
        match find_item(kc.as_ref(), service, account)? {
            Some(item) => {
                set_item_data(&item, secret)?;
                false
            }
            None => {
                ensure_unlocked(kc.as_ref())?;
                run_placeholder_create(service, account, opts)?;
                let item = find_item(kc.as_ref(), service, account)?.ok_or_else(|| {
                    KeychainError::Verify("the new item was not found after security created it".into())
                })?;
                if let Err(e) = set_item_data(&item, secret) {
                    drop(item);
                    let _ = delete(service, account, kc_path);
                    return Err(e);
                }
                true
            }
        }
    };
    let back = read(service, account, kc_path);
    let ok = matches!(&back, Ok(Some(b)) if b.as_slice() == secret);
    if ok {
        return Ok(());
    }
    if created {
        let _ = delete(service, account, kc_path);
    }
    Err(match back {
        Ok(Some(b)) => KeychainError::Verify(format!("wrote {} bytes, read back {}", secret.len(), b.len())),
        Ok(None) => KeychainError::Verify("the item is gone after the write".into()),
        Err(e) => KeychainError::Verify(format!("read back failed: {e}")),
    })
}

#[cfg(test)]
#[path = "macos_keychain_tests.rs"]
mod tests;
