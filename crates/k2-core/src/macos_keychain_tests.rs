//! Keychain tests. Every test makes its OWN throwaway keychain file under
//! the temp dir, unlocked with a known password, auto-lock off, never in
//! the user's search list, passed explicitly to every call, and deleted on
//! drop. Nothing here touches the login keychain or a real item, and
//! nothing can raise a dialog: framework calls run with user interaction
//! off, and no `security` call is made against a locked keychain.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const MARKER: &str = "K2TEST_KEYCHAIN_SECRET";
const ACCT: &str = "k2-test-user";

static N: AtomicUsize = AtomicUsize::new(0);

fn sec(args: &[&str]) -> std::process::Output {
    Command::new(SECURITY).args(args).stdin(Stdio::null()).output().expect("run security")
}

struct TempKeychain {
    dir: PathBuf,
    path: PathBuf,
    password: String,
}

impl TempKeychain {
    fn new(label: &str) -> TempKeychain {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k2-kc-{label}-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("k2-test.keychain-db");
        let password = format!("k2-test-{nanos}-{n}");
        // Built first so Drop deletes the keychain even if a step panics.
        let kc = TempKeychain { dir, path, password };
        let out = sec(&["create-keychain", "-p", &kc.password, kc.s()]);
        assert!(out.status.success(), "create-keychain: {out:?}");
        let out = sec(&["unlock-keychain", "-p", &kc.password, kc.s()]);
        assert!(out.status.success(), "unlock-keychain: {out:?}");
        // No timeout, no lock on sleep.
        let out = sec(&["set-keychain-settings", kc.s()]);
        assert!(out.status.success(), "set-keychain-settings: {out:?}");
        let list = sec(&["list-keychains", "-d", "user"]);
        assert!(
            !String::from_utf8_lossy(&list.stdout).contains(kc.dir.to_str().unwrap()),
            "the test keychain must never be in the user's search list"
        );
        kc
    }
    fn p(&self) -> &Path {
        &self.path
    }
    fn s(&self) -> &str {
        self.path.to_str().unwrap()
    }
    fn opts(&self) -> WriteOptions {
        WriteOptions { keychain: Some(self.path.clone()), ..Default::default() }
    }
    /// How many items with this service are in the keychain (attributes
    /// only; `dump-keychain` without `-d` prints no secrets).
    fn count(&self, service: &str) -> usize {
        let out = sec(&["dump-keychain", self.s()]);
        assert!(out.status.success(), "dump-keychain: {out:?}");
        let needle = format!("\"svce\"<blob>=\"{service}\"");
        String::from_utf8_lossy(&out.stdout).matches(&needle).count()
    }
    /// The item's access list as `security dump-keychain -a` prints it.
    fn acl(&self, service: &str) -> String {
        let out = sec(&["dump-keychain", "-a", self.s()]);
        assert!(out.status.success(), "dump-keychain -a: {out:?}");
        let txt = String::from_utf8_lossy(&out.stdout).to_string();
        let block = txt
            .split("keychain: ")
            .find(|b| b.contains(&format!("\"svce\"<blob>=\"{service}\"")))
            .unwrap_or_else(|| panic!("no item {service} in the dump"));
        // The `access:` section only (attributes like `mdat` change).
        let at = block.find("access:").unwrap_or_else(|| panic!("no access list for {service}: {block}"));
        block[at..].to_string()
    }
    /// Write an item the way Claude Code does: `security -i` with hex.
    fn claude_style_add(&self, service: &str, data: &[u8]) {
        let line = format!("add-generic-password -U -a \"{ACCT}\" -s \"{service}\" -X \"{}\" \"{}\"\n", hex(data), self.s());
        assert!(line.len() <= INTERACTIVE_LINE_MAX, "test payload must fit security -i");
        let mut child = Command::new(SECURITY)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(line.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "claude-style add: {out:?}");
    }
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        let _ = sec(&["delete-keychain", self.path.to_str().unwrap()]);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A printable Claude-shaped JSON credential of exactly `n` bytes.
fn json_payload(n: usize, tag: &str) -> Vec<u8> {
    let head = format!("{{\"claudeAiOauth\":{{\"accessToken\":\"{MARKER}-{tag}-");
    let tail = "\"}}";
    assert!(n > head.len() + tail.len());
    let mut s = head;
    let mut i = 0u32;
    while s.len() + tail.len() < n {
        s.push(char::from(b'a' + (i % 26) as u8));
        i += 1;
    }
    s.push_str(tail);
    assert_eq!(s.len(), n);
    serde_json::from_str::<serde_json::Value>(&s).expect("payload is JSON");
    s.into_bytes()
}

fn binary_payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn payloads_of_every_size_round_trip_through_write_and_read() {
    let kc = TempKeychain::new("sizes");
    for n in [100usize, 2 * 1024, 8 * 1024, 64 * 1024] {
        let svc = format!("k2-test-svc-{n}");
        let first = json_payload(n, "first");
        write(&svc, ACCT, &first, &kc.opts()).unwrap_or_else(|e| panic!("create {n}: {e}"));
        assert_eq!(read(&svc, ACCT, Some(kc.p())).unwrap().expect("item exists"), first, "create {n}");
        let second = json_payload(n, "second");
        write(&svc, ACCT, &second, &kc.opts()).unwrap_or_else(|e| panic!("update {n}: {e}"));
        assert_eq!(read(&svc, ACCT, Some(kc.p())).unwrap().expect("item exists"), second, "update {n}");
        assert_eq!(kc.count(&svc), 1, "update keeps one item ({n})");
    }
    // Bytes `security -w` would print as hex (NUL, newline, quote).
    let bin = binary_payload(8 * 1024);
    write("k2-test-bin", ACCT, &bin, &kc.opts()).unwrap();
    assert_eq!(read("k2-test-bin", ACCT, Some(kc.p())).unwrap().unwrap(), bin);
    // A printable all-hex secret stays text (the old `-w` reader decoded it).
    write("k2-test-hexy", ACCT, b"deadbeef", &kc.opts()).unwrap();
    assert_eq!(read("k2-test-hexy", ACCT, Some(kc.p())).unwrap().unwrap(), b"deadbeef");
    // A trailing newline survives.
    write("k2-test-nl", ACCT, b"{\"a\":1}\n", &kc.opts()).unwrap();
    assert_eq!(read("k2-test-nl", ACCT, Some(kc.p())).unwrap().unwrap(), b"{\"a\":1}\n");
    assert_eq!(read("k2-test-missing", ACCT, Some(kc.p())).unwrap(), None);
}

#[test]
fn updating_a_claude_made_item_keeps_its_acl_and_claudes_own_reads_and_writes_work() {
    let kc = TempKeychain::new("claude-acl");
    let svc = "Claude Code-credentials-k2test";
    kc.claude_style_add(svc, b"{\"claudeAiOauth\":{\"accessToken\":\"small\"}}");
    let before = kc.acl(svc);
    assert!(before.contains("/usr/bin/security"), "Claude's item trusts security: {before}");
    // The realistic case that broke in 0.45.0: > 4,032 hex characters.
    let big = json_payload(8 * 1024, "swap");
    write(svc, ACCT, &big, &kc.opts()).unwrap();
    let after = kc.acl(svc);
    assert_eq!(before, after, "the item's access list is unchanged");
    let exe = std::env::current_exe().unwrap();
    assert!(!after.contains(exe.to_str().unwrap()), "the test binary was not added to the ACL");
    assert_eq!(kc.count(svc), 1);
    // Claude Code's own read path (`security find-generic-password -w`).
    let out = sec(&["find-generic-password", "-a", ACCT, "-s", svc, "-w", kc.s()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout.strip_suffix(b"\n").unwrap(), big.as_slice());
    // Claude Code's own write path still updates it in place.
    let small = b"{\"claudeAiOauth\":{\"accessToken\":\"claude-refreshed\"}}";
    kc.claude_style_add(svc, small);
    assert_eq!(read(svc, ACCT, Some(kc.p())).unwrap().unwrap(), small);
    assert_eq!(kc.count(svc), 1);
}

#[test]
fn a_new_item_gets_securitys_acl_or_the_requested_trusted_apps() {
    let kc = TempKeychain::new("new-acl");
    let data = json_payload(6 * 1024, "new");
    write("k2-test-plain", ACCT, &data, &kc.opts()).unwrap();
    let acl = kc.acl("k2-test-plain");
    assert!(acl.contains("/usr/bin/security"), "{acl}");
    let exe = std::env::current_exe().unwrap();
    assert!(!acl.contains(exe.to_str().unwrap()), "created by security, not by this binary: {acl}");

    let opts = WriteOptions {
        keychain: Some(kc.path.clone()),
        label: Some("K2 test label".into()),
        trusted_apps: vec!["/usr/bin/security".into(), "/bin/ls".into()],
        replace_acl: true,
    };
    write("k2-test-acl", ACCT, &data, &opts).unwrap();
    let acl = kc.acl("k2-test-acl");
    assert!(acl.contains("/bin/ls") && acl.contains("/usr/bin/security"), "{acl}");
    // Rewrite with replace_acl: still one item, new data, same ACL.
    let data2 = json_payload(6 * 1024, "new2");
    write("k2-test-acl", ACCT, &data2, &opts).unwrap();
    assert_eq!(kc.count("k2-test-acl"), 1);
    assert_eq!(read("k2-test-acl", ACCT, Some(kc.p())).unwrap().unwrap(), data2);
    assert!(kc.acl("k2-test-acl").contains("/bin/ls"));
}

#[test]
fn no_secret_is_ever_on_a_spawned_commands_argv() {
    let opts = WriteOptions {
        keychain: Some(PathBuf::from("/tmp/k2 test/k.keychain-db")),
        label: Some("K2 label".into()),
        trusted_apps: vec!["/usr/bin/security".into(), "/Applications/K2.app/Contents/MacOS/k2".into()],
        replace_acl: false,
    };
    let (cmd, line) = placeholder_create("svc", ACCT, &opts).unwrap();
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    assert_eq!(cmd.get_program(), SECURITY);
    assert_eq!(args, vec!["-i".to_string()], "the create carries no data on argv");
    // The only data on stdin is the non-secret placeholder.
    assert!(line.contains(&format!("-X \"{}\"", hex(PLACEHOLDER))), "{line}");
    assert!(line.len() <= INTERACTIVE_LINE_MAX);
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
    for cmd in [read_command("svc", ACCT, Some(Path::new("/tmp/x"))), delete_command("svc", ACCT, None)] {
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(
            args.iter().all(|a| ["find-generic-password", "delete-generic-password", "-a", "-s", "-g", ACCT, "svc", "/tmp/x"].contains(&a.as_str())),
            "only names on argv: {args:?}"
        );
    }
    // A field that could break out of the quoted `security -i` line.
    assert_eq!(placeholder_create("svc\" -X \"41", ACCT, &opts).unwrap_err(), KeychainError::BadField("service"));
}

#[test]
fn a_missing_keychain_is_named_and_never_blamed_on_a_lock() {
    let kc = TempKeychain::new("missing");
    let missing = kc.dir.join("missing.keychain-db");
    let opts = WriteOptions { keychain: Some(missing.clone()), ..Default::default() };
    let err = write("k2-test-svc", ACCT, b"{\"a\":1}", &opts).unwrap_err();
    assert_eq!(err, KeychainError::NoSuchKeychain(missing.clone()));
    let text = err.to_string();
    assert!(text.contains("does not exist") && text.contains("missing.keychain-db"), "{text}");
    assert!(!text.contains("unlock"), "{text}");
    assert!(!err.is_locked());
    assert_eq!(read("k2-test-svc", ACCT, Some(&missing)).unwrap_err(), KeychainError::NoSuchKeychain(missing));
}

#[test]
fn a_locked_keychain_fails_fast_with_unlock_advice_and_no_dialog() {
    let kc = TempKeychain::new("locked");
    write("k2-test-exists", ACCT, b"{\"a\":1}", &kc.opts()).unwrap();
    let out = sec(&["lock-keychain", kc.s()]);
    assert!(out.status.success(), "{out:?}");
    // The lock is detected in process before any `security` call.
    {
        let _ui = NoUi::enter();
        let k = open_keychain(kc.p()).unwrap();
        let e = ensure_unlocked(Some(&k)).expect_err("locked keychain must be detected");
        assert!(e.is_locked(), "{e}");
    }
    // Existing item: the in-process data change is refused.
    let err = write("k2-test-exists", ACCT, b"{\"a\":2}", &kc.opts()).unwrap_err();
    assert!(err.is_locked(), "{err}");
    assert!(err.to_string().contains("unlock the login keychain"), "{err}");
    // Missing item: refused before `security` could ask to unlock.
    let err = write("k2-test-new", ACCT, b"{\"a\":3}", &kc.opts()).unwrap_err();
    assert!(err.is_locked(), "{err}");
    let out = sec(&["unlock-keychain", "-p", &kc.password, kc.s()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(read("k2-test-exists", ACCT, Some(kc.p())).unwrap().unwrap(), b"{\"a\":1}", "unchanged");
    assert_eq!(read("k2-test-new", ACCT, Some(kc.p())).unwrap(), None, "nothing created");
}

#[test]
fn error_text_maps_codes_and_scrubs_secrets() {
    assert_eq!(status_for_exit(36), Some(-25308));
    assert_eq!(status_for_exit(51), Some(-25293));
    assert_eq!(status_for_exit(44), Some(-25300));
    assert_eq!(status_for_exit(45), Some(-25299));
    assert_eq!(status_for_exit(128), Some(-128));
    assert_eq!(status_for_exit(1), None);

    // The 0.45.0 failure: `security -i` split the line and echoed the
    // remainder (secret hex) on stderr with exit 1.
    let secret = json_payload(3000, "leak");
    let tail = &hex(&secret)[4000..4100];
    let stderr = format!("security: unknown command \"{tail}\"\n{tail}\": returned 1\n");
    let e = KeychainError::cli("create keychain item", Some(1), stderr.as_bytes(), Some(&secret));
    let text = e.to_string();
    assert!(text.contains("security exited 1"), "{text}");
    assert!(!text.contains(tail), "hex chunk scrubbed: {text}");
    assert!(!text.contains("unlock"), "exit 1 is not a lock: {text}");
    assert!(text.contains("unknown command"), "stderr kept: {text}");

    let locked = KeychainError::cli("read keychain item", Some(36), b"security: SecKeychainSearchCopyNext: User interaction is not allowed.\n", None);
    assert!(locked.is_locked());
    assert!(locked.to_string().contains("unlock the login keychain"), "{locked}");
    let missing = KeychainError::cli("read keychain item", Some(44), b"", None);
    assert!(missing.is_not_found() && !missing.to_string().contains("unlock"));

    assert_eq!(scrub(&format!("x {MARKER} y"), Some(MARKER.as_bytes())), "x [redacted] y");
    assert_eq!(scrub("token sk-ant-oat01-ABCDEF0123456789xyz end", None), "token [redacted] end");
    assert_eq!(scrub("SecKeychainItemCreateFromContent failed", None), "SecKeychainItemCreateFromContent failed");
}

#[test]
fn g_password_lines_parse_unambiguously() {
    assert_eq!(parse_g_password(b"password: \"deadbeef\"\n").unwrap(), b"deadbeef");
    assert_eq!(parse_g_password(b"password: 0x000102FF \n").unwrap(), vec![0, 1, 2, 255]);
    assert_eq!(
        parse_g_password(b"password: 0x7B2261223A317D0A  \"{\"a\":1}\\012\"\n").unwrap(),
        b"{\"a\":1}\n"
    );
    assert_eq!(parse_g_password(b"keychain: x\npassword: \"{\"a\":\"b\"}\"\n").unwrap(), b"{\"a\":\"b\"}");
    assert_eq!(parse_g_password(b"password: \n").unwrap(), b"");
    assert!(parse_g_password(b"nothing here\n").is_none());
}
