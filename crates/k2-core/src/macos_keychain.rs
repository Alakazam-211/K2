//! macOS keychain generic passwords: every write and read goes through
//! `/usr/bin/security`, with the secret on its stdin (never argv).
//!
//! ## Why every write must be made by `/usr/bin/security`
//!
//! On a partition-aware keychain (the login keychain; version 0x200), an
//! item carries a `partition_id` list next to its ACL, and EVERY change of
//! the item's data REPLACES that list with the writing process's own
//! partition. Measured 2026-10-09 on a throwaway 0x200 keychain:
//! `security -i add-generic-password` gives `apple-tool:`; a
//! `SecKeychainItemModifyAttributesAndData` from any other binary leaves
//! ONLY that binary's partition (`cdhash:…` for an unsigned build,
//! `teamid:36B8R93HXV` for the signed K2 daemon) — no prompt, no error.
//! Claude Code reads its item through `/usr/bin/security`, which needs
//! `apple-tool:`, so after one in-process write every read Claude makes
//! raises a keychain password dialog ("Always Allow" doesn't stick). That
//! was the 0.45.1/0.45.2 field bug (Appa, 12 prompts in 5 minutes after
//! adding a second Claude subscription). Throwaway keychains made by
//! `security create-keychain` outside `~/Library/Keychains` are version
//! 0x100 and have no partitions at all, which is why the 0.45.1 tests
//! didn't see it.
//!
//! So K2 never writes item data in process. Writes are
//! `security -i add-generic-password -U …` with the secret on stdin, the
//! same command Claude Code itself runs, so the item ends up exactly as if
//! Claude had written it.
//!
//! ## Size
//!
//! `security -i` reads each stdin line into a ~4096-byte buffer and runs
//! the rest of a longer line as a second command (0.45.0 field bug). The
//! secret goes as `-X <hex>` when that fits ([`INTERACTIVE_LINE_MAX`]),
//! else as `-w "<text>"` (printable ASCII, `"` and `\` escaped; JSON
//! logins are), which fits about 3.9 KB. Anything bigger is refused with
//! [`KeychainError::TooLarge`]: the only other ways in are the secret on
//! argv (other users on the Mac can see it in `ps`) or an in-process write
//! (breaks the partition, above).
//!
//! ## Reads never prompt
//!
//! - Reads go through `/usr/bin/security find-generic-password -g`, the
//!   trusted reader on these items. `-g` prints `password: 0x<HEX>` for
//!   anything not plainly printable and `password: "<text>"` otherwise, so
//!   the parse is unambiguous.
//! - Before any `security` call K2 looks the item up IN PROCESS without
//!   its data (no prompt possible; keychain user interaction is off): a
//!   locked keychain fails fast instead of raising an unlock dialog, and an
//!   item whose partition list lacks `apple-tool:` is NOT read (that read
//!   would prompt) but reported as [`KeychainError::NeedsRepair`] with the
//!   one-line Terminal fix ([`repair_command`]).
//! - Deletes run in process with user interaction off (no dialog is
//!   possible). Deleting is not partition-gated (measured: a binary that
//!   is not on the item's list deletes it, no prompt), so a write to an
//!   item that needs repair deletes it and lets `security` create it
//!   fresh: repaired, silently.
//! - Every write is read back and compared before it returns `Ok`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Mutex;

use core_foundation::array::{CFArray, CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType};
use core_foundation::data::CFData;
use core_foundation::dictionary::{CFDictionaryGetTypeID, CFDictionaryGetValue};
use core_foundation::string::{CFString, CFStringGetTypeID, CFStringRef};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework_sys::base::{errSecSuccess, SecAccessRef, SecCopyErrorMessageString, SecKeychainItemRef, SecKeychainRef};
use security_framework_sys::keychain::{
    SecKeychainFindGenericPassword, SecKeychainGetUserInteractionAllowed, SecKeychainOpen,
    SecKeychainSetUserInteractionAllowed,
};

const SECURITY: &str = "/usr/bin/security";

/// `security -i` splits input lines longer than its ~4096-byte buffer.
/// Claude Code uses 4,032 as its own cut-off; stay under it.
pub const INTERACTIVE_LINE_MAX: usize = 4032;

/// The partition `/usr/bin/security` (and so Claude Code) needs on an item.
pub const APPLE_TOOL_PARTITION: &str = "apple-tool:";

// Not in security-framework-sys.
#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
    fn SecKeychainItemCopyKeychain(item: SecKeychainItemRef, keychain: *mut SecKeychainRef) -> i32;
    fn SecKeychainItemCopyAccess(item: SecKeychainItemRef, access: *mut SecAccessRef) -> i32;
    fn SecAccessCopyMatchingACLList(access: SecAccessRef, authorization_tag: CFTypeRef) -> CFArrayRef;
    fn SecACLCopyContents(
        acl: CFTypeRef,
        application_list: *mut CFArrayRef,
        description: *mut CFStringRef,
        prompt_selector: *mut u16,
    ) -> i32;
    static kSecACLAuthorizationPartitionID: CFStringRef;
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
    /// The secret is too big to hand to `security -i` on stdin.
    TooLarge(usize),
    /// The item's partition list lacks `apple-tool:`: any read through
    /// `/usr/bin/security` (K2's or Claude's) would raise a password
    /// dialog, so K2 doesn't read it. [`Repair::command`] fixes it.
    NeedsRepair(Repair),
}

/// An item that `/usr/bin/security` can't read without a dialog, and the
/// one-line Terminal command that fixes it (asks for the login password
/// once).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub service: String,
    pub account: String,
    /// The item's partition list now (e.g. `["teamid:36B8R93HXV"]`).
    pub partitions: Vec<String>,
    pub command: String,
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
            KeychainError::TooLarge(n) => write!(
                f,
                "the secret is {n} bytes, more than macOS's security tool takes on its input (about 3,900 bytes of printable text); K2 won't put it on a command line or write it from its own process"
            ),
            KeychainError::NeedsRepair(r) => write!(
                f,
                "the keychain item \"{}\" can't be read without a macOS password prompt (its partition list is {} and lacks {APPLE_TOOL_PARTITION}); to fix it, run this once in Terminal and enter your login password: {}",
                r.service,
                if r.partitions.is_empty() { "empty".to_string() } else { r.partitions.join(",") },
                r.command
            ),
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

fn keychain_unlocked(kref: SecKeychainRef) -> Result<(), KeychainError> {
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

/// Refuse before `security` would raise an unlock dialog: a locked
/// keychain fails here with errSecInteractionNotAllowed. `kc = None` =
/// the default keychain (where `security add-generic-password` creates).
fn ensure_unlocked(kc: Option<&Owned>) -> Result<(), KeychainError> {
    match kc {
        Some(k) => keychain_unlocked(k.0 as SecKeychainRef),
        None => {
            let mut d: SecKeychainRef = ptr::null_mut();
            let st = unsafe { security_framework_sys::keychain::SecKeychainCopyDefault(&mut d) };
            if st != errSecSuccess {
                return Err(KeychainError::status("find the default keychain", st));
            }
            let default = Owned(d as *const _);
            keychain_unlocked(default.0 as SecKeychainRef)
        }
    }
}

/// The keychain an item lives in must be unlocked (the item may be in
/// any keychain on the search list, not only the default one).
fn ensure_item_keychain_unlocked(item: &Owned) -> Result<(), KeychainError> {
    let mut kc: SecKeychainRef = ptr::null_mut();
    let st = unsafe { SecKeychainItemCopyKeychain(item.0 as SecKeychainItemRef, &mut kc) };
    if st != errSecSuccess {
        return Err(KeychainError::status("find the item's keychain", st));
    }
    let kc = Owned(kc as *const _);
    keychain_unlocked(kc.0 as SecKeychainRef)
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Parse a `partition_id` ACL entry's description. securityd stores a
/// hex-encoded property list `{Partitions = ("apple-tool:", …)}`; a plain
/// `a, b` list (what `security dump-keychain` prints) is accepted too.
pub(crate) fn parse_partition_description(desc: &str) -> Option<Vec<String>> {
    let d = desc.trim();
    if !d.is_empty() && d.len() % 2 == 0 && d.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes = decode_hex(d)?;
        let data = CFData::from_buffer(&bytes);
        let (plist, _fmt) = core_foundation::propertylist::create_with_data(data, 0).ok()?;
        let plist = Owned(plist);
        unsafe {
            if CFGetTypeID(plist.0) != CFDictionaryGetTypeID() {
                return None;
            }
            let key = CFString::from_static_string("Partitions");
            let arr = CFDictionaryGetValue(plist.0 as _, key.as_concrete_TypeRef() as *const _);
            if arr.is_null() || CFGetTypeID(arr) != CFArrayGetTypeID() {
                return None;
            }
            let arr = arr as CFArrayRef;
            let mut out = Vec::new();
            for i in 0..CFArrayGetCount(arr) {
                let v = CFArrayGetValueAtIndex(arr, i);
                if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
                    return None;
                }
                out.push(CFString::wrap_under_get_rule(v as CFStringRef).to_string());
            }
            return Some(out);
        }
    }
    Some(d.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
}

/// The item's partition list, read IN PROCESS from its ACL (no data, no
/// prompt). `None` = the item has no `partition_id` entry (a 0x100
/// keychain, or an item from before partitions): nothing to check.
fn item_partitions(item: &Owned) -> Result<Option<Vec<String>>, KeychainError> {
    let mut access: SecAccessRef = ptr::null_mut();
    let st = unsafe { SecKeychainItemCopyAccess(item.0 as SecKeychainItemRef, &mut access) };
    if st != errSecSuccess {
        return Err(KeychainError::status("read the item's access list", st));
    }
    let access = Owned(access as *const _);
    let list = unsafe { SecAccessCopyMatchingACLList(access.0 as SecAccessRef, kSecACLAuthorizationPartitionID as CFTypeRef) };
    if list.is_null() {
        return Ok(None);
    }
    let list = Owned(list as *const _);
    let n = unsafe { CFArrayGetCount(list.0 as CFArrayRef) };
    if n == 0 {
        return Ok(None);
    }
    let mut all = Vec::new();
    for i in 0..n {
        let acl = unsafe { CFArrayGetValueAtIndex(list.0 as CFArrayRef, i) };
        let mut apps: CFArrayRef = ptr::null();
        let mut desc: CFStringRef = ptr::null();
        let mut prompt: u16 = 0;
        let st = unsafe { SecACLCopyContents(acl, &mut apps, &mut desc, &mut prompt) };
        let _apps = Owned(apps as *const _);
        if st != errSecSuccess {
            return Err(KeychainError::status("read the item's partition list", st));
        }
        if desc.is_null() {
            continue;
        }
        let desc = unsafe { CFString::wrap_under_create_rule(desc) }.to_string();
        let ids = parse_partition_description(&desc)
            .ok_or_else(|| KeychainError::Verify("could not parse the item's partition list".into()))?;
        for id in ids {
            if !all.contains(&id) {
                all.push(id);
            }
        }
    }
    Ok(Some(all))
}

/// Single-quote for a POSIX shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The Terminal command that puts `apple-tool:` and `apple:` back on an
/// item's partition list, keeping what's there. `security` asks for the
/// keychain (login) password once.
pub fn repair_command(service: &str, account: &str, partitions: &[String], keychain: Option<&Path>) -> String {
    let mut ids: Vec<String> = vec![APPLE_TOOL_PARTITION.to_string(), "apple:".to_string()];
    for p in partitions {
        if !ids.contains(p) {
            ids.push(p.clone());
        }
    }
    let mut cmd = format!(
        "security set-generic-password-partition-list -s {} -a {} -S {}",
        sh_quote(service),
        sh_quote(account),
        sh_quote(&ids.join(","))
    );
    if let Some(kc) = keychain {
        cmd.push(' ');
        cmd.push_str(&sh_quote(&kc.to_string_lossy()));
    }
    cmd
}

/// What `/usr/bin/security` would meet on this item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemAccess {
    Missing,
    /// Readable through `/usr/bin/security` with no dialog.
    Ok,
    NeedsRepair(Repair),
}

/// Look the item up in process (attributes only, user interaction off):
/// missing, fine, or needing the partition repair. A locked keychain is
/// an error (`is_locked`), never a dialog.
pub fn check_item(service: &str, account: &str, keychain: Option<&Path>) -> Result<ItemAccess, KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    check_keychain(keychain)?;
    let _ui = NoUi::enter();
    let kc = keychain.map(open_keychain).transpose()?;
    let Some(item) = find_item(kc.as_ref(), service, account)? else {
        return Ok(ItemAccess::Missing);
    };
    ensure_item_keychain_unlocked(&item)?;
    match item_partitions(&item)? {
        Some(ids) if !ids.iter().any(|p| p == APPLE_TOOL_PARTITION) => {
            let command = repair_command(service, account, &ids, keychain);
            Ok(ItemAccess::NeedsRepair(Repair {
                service: service.to_string(),
                account: account.to_string(),
                partitions: ids,
                command,
            }))
        }
        _ => Ok(ItemAccess::Ok),
    }
}

/// Does the item exist? (In process, attributes only: no prompt.)
pub fn exists(service: &str, account: &str, keychain: Option<&Path>) -> Result<bool, KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    check_keychain(keychain)?;
    let _ui = NoUi::enter();
    let kc = keychain.map(open_keychain).transpose()?;
    Ok(find_item(kc.as_ref(), service, account)?.is_some())
}

// ── `security` CLI ──────────────────────────────────────────────────

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `security -i` reads a double-quoted argument with `\"` and `\\`
/// escapes. Only printable ASCII goes this way.
fn quote_text(secret: &[u8]) -> Option<String> {
    if !secret.iter().all(|b| (0x20..=0x7e).contains(b)) {
        return None;
    }
    let s = std::str::from_utf8(secret).ok()?;
    Some(s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The ONE way K2 writes an item: `/usr/bin/security -i` and its stdin
/// line `add-generic-password -U … -X "<hex>"` (or `-w "<text>"` when the
/// hex is too long). argv is just `-i`; the secret is only on stdin.
/// `-U` updates an existing item in place (its ACL is kept; the data
/// change gives it `security`'s `apple-tool:` partition, as when Claude
/// Code writes it). `pub(crate)` so tests can assert on argv and stdin.
pub(crate) fn add_command(service: &str, account: &str, secret: &[u8], opts: &WriteOptions) -> Result<(Command, String), KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    let mut head = format!("add-generic-password -U -a \"{account}\" -s \"{service}\"");
    if let Some(l) = &opts.label {
        check_field("label", l)?;
        head.push_str(&format!(" -l \"{l}\""));
    }
    for app in &opts.trusted_apps {
        check_field("trusted app path", app)?;
        head.push_str(&format!(" -T \"{app}\""));
    }
    let tail = match &opts.keychain {
        Some(kc) => {
            check_field("path", &kc.to_string_lossy())?;
            format!(" \"{}\"\n", kc.display())
        }
        None => "\n".to_string(),
    };
    let fits = |data: &str| head.len() + data.len() + tail.len() <= INTERACTIVE_LINE_MAX;
    let as_hex = format!(" -X \"{}\"", hex(secret));
    let line = if fits(&as_hex) {
        format!("{head}{as_hex}{tail}")
    } else {
        match quote_text(secret).map(|t| format!(" -w \"{t}\"")) {
            Some(as_text) if fits(&as_text) => format!("{head}{as_text}{tail}"),
            _ => return Err(KeychainError::TooLarge(secret.len())),
        }
    };
    let mut cmd = Command::new(SECURITY);
    cmd.arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    Ok((cmd, line))
}

/// Would [`write`] accept this secret (size, encoding, fields)? Touches
/// nothing.
pub fn check_fits(service: &str, account: &str, secret: &[u8]) -> Result<(), KeychainError> {
    add_command(service, account, secret, &WriteOptions::default()).map(|_| ())
}

fn run_add(service: &str, account: &str, secret: &[u8], opts: &WriteOptions) -> Result<(), KeychainError> {
    let (mut cmd, line) = add_command(service, account, secret, opts)?;
    let mut child = cmd.spawn().map_err(|e| KeychainError::Spawn(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(line.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(KeychainError::Spawn(format!("stdin: {e}")));
        }
    }
    let out = child.wait_with_output().map_err(|e| KeychainError::Spawn(e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    // `security -i` echoes a failed command's text on stderr: scrub the
    // escaped text form too. (The caller reads back and compares anyway.)
    let quoted = quote_text(secret).unwrap_or_default();
    let mut stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if quoted.len() >= 4 {
        stderr = stderr.replace(&quoted, "[redacted]");
    }
    Err(KeychainError::cli("write keychain item", out.status.code(), stderr.as_bytes(), Some(secret)))
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
/// Never raises a dialog: a locked keychain is an error, and an item
/// `security` can't read silently is [`KeychainError::NeedsRepair`].
pub fn read(service: &str, account: &str, keychain: Option<&Path>) -> Result<Option<Vec<u8>>, KeychainError> {
    match check_item(service, account, keychain)? {
        ItemAccess::Missing => return Ok(None),
        ItemAccess::NeedsRepair(r) => return Err(KeychainError::NeedsRepair(r)),
        ItemAccess::Ok => {}
    }
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

/// Delete a generic password IN PROCESS with keychain user interaction
/// off: no dialog is possible (a refusal would come back as an error).
/// Deleting is not partition-gated (measured: a binary that is not on the
/// item's partition list deletes it with no prompt). A missing item is
/// not an error.
pub fn delete(service: &str, account: &str, keychain: Option<&Path>) -> Result<(), KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    check_keychain(keychain)?;
    let _ui = NoUi::enter();
    let kc = keychain.map(open_keychain).transpose()?;
    let Some(item) = find_item(kc.as_ref(), service, account)? else {
        return Ok(());
    };
    let st = unsafe { security_framework_sys::keychain_item::SecKeychainItemDelete(item.0 as SecKeychainItemRef) };
    match st {
        0 | -25300 => Ok(()),
        _ => Err(KeychainError::status("delete keychain item", st)),
    }
}

/// Create or update a generic password through `/usr/bin/security -i`
/// (secret on stdin only), then read it back and compare. Never writes
/// item data from this process: that would replace the item's partition
/// list with K2's and make every `security` read (Claude's) prompt.
///
/// - Missing item: `security` creates it (its default ACL and
///   `apple-tool:` partition, as when Claude Code creates it).
/// - Readable item: `security -U` updates it in place (ACL kept).
/// - Item needing repair (an earlier in-process write): deleted in
///   process (no prompt possible) and created fresh by `security`, which
///   repairs it. Its old data was unreadable without a prompt anyway;
///   callers pass the authoritative new value.
pub fn write(service: &str, account: &str, secret: &[u8], opts: &WriteOptions) -> Result<(), KeychainError> {
    check_field("service", service)?;
    check_field("account", account)?;
    let kc_path = opts.keychain.as_deref();
    check_keychain(kc_path)?;
    // Build the command first: a secret that can't go on stdin fails
    // before anything changes.
    add_command(service, account, secret, opts)?;
    {
        let _ui = NoUi::enter();
        let kc = kc_path.map(open_keychain).transpose()?;
        ensure_unlocked(kc.as_ref())?;
    }
    if opts.replace_acl {
        // A fresh item gets the requested ACL (`-U` would keep the old).
        delete(service, account, kc_path)?;
    }
    let existed = match check_item(service, account, kc_path)? {
        ItemAccess::Missing => false,
        ItemAccess::Ok => true,
        ItemAccess::NeedsRepair(_) => {
            delete(service, account, kc_path)?;
            false
        }
    };
    run_add(service, account, secret, opts)?;
    let back = read(service, account, kc_path);
    if matches!(&back, Ok(Some(b)) if b.as_slice() == secret) {
        return Ok(());
    }
    if !existed {
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
