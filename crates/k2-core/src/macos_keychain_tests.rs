//! Keychain tests. Every test makes its OWN throwaway keychain file under
//! the temp dir, unlocked with a known password, auto-lock off, never in
//! the user's search list, passed explicitly to every call, and deleted on
//! drop. Nothing here touches the login keychain or a real item.
//!
//! The file sits at `<temp>/…/Library/Keychains/k2-test.keychain-db`: a
//! keychain created under a `Library/Keychains` directory is the
//! partition-aware version (0x200) the login keychain is, and one created
//! anywhere else is 0x100, which has no partition lists at all (that is
//! why the 0.45.1 tests missed the partition bug). `TempKeychain::new`
//! asserts 0x200.
//!
//! No dialog can come from here: framework calls run with user
//! interaction off, every `security` call against a locked keychain is
//! refused before it runs, and nothing runs `security` on an item whose
//! partition list lacks `apple-tool:` (the tests check the list first).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const MARKER: &str = "K2TEST_KEYCHAIN_SECRET";
const ACCT: &str = "k2-test-user";

static N: AtomicUsize = AtomicUsize::new(0);

// SPI (exported by Security.framework; used by `security` itself).
#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecKeychainGetKeychainVersion(keychain: SecKeychainRef, version: *mut u32) -> i32;
}

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
        let kc_dir = dir.join("Library").join("Keychains");
        std::fs::create_dir_all(&kc_dir).unwrap();
        let path = kc_dir.join("k2-test.keychain-db");
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
        assert_eq!(kc.version(), 0x200, "the test keychain must be partition-aware, like the login keychain");
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
    fn version(&self) -> u32 {
        let _ui = NoUi::enter();
        let k = open_keychain(self.p()).unwrap();
        let mut v = 0u32;
        let st = unsafe { SecKeychainGetKeychainVersion(k.0 as SecKeychainRef, &mut v) };
        assert_eq!(st, 0, "SecKeychainGetKeychainVersion");
        v
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
    /// The access list minus the `integrity` entry (a digest that changes
    /// with every data change): who may do what, and the partition list.
    fn acl_shape(&self, service: &str) -> String {
        let acl = self.acl(service);
        let mut entries: Vec<Vec<&str>> = Vec::new();
        for line in acl.lines().skip(1) {
            if line.trim_start().starts_with("entry ") {
                entries.push(Vec::new());
            } else if let Some(e) = entries.last_mut() {
                e.push(line);
            }
        }
        let mut shape: Vec<String> = entries
            .into_iter()
            .filter(|e| !e.iter().any(|l| l.contains("): integrity")))
            .map(|e| e.join("\n"))
            .collect();
        shape.sort();
        shape.join("\n--\n")
    }
    /// The partition list as `security dump-keychain -a` prints it (every
    /// `partition_id` entry's description). Read without the item's data.
    fn partitions_dumped(&self, service: &str) -> Vec<String> {
        let acl = self.acl(service);
        let lines: Vec<&str> = acl.lines().collect();
        let mut out = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            if l.contains("authorizations (1): partition_id") {
                let desc = lines[i + 1..]
                    .iter()
                    .find_map(|d| d.trim().strip_prefix("description: "))
                    .unwrap_or_else(|| panic!("partition entry without description: {acl}"));
                out.extend(desc.split(", ").map(str::to_string));
            }
        }
        out
    }
    /// Assert `security` can read the item silently BEFORE anything runs
    /// `security` on it: its partition list is exactly `apple-tool:`.
    fn assert_security_readable(&self, service: &str) {
        assert_eq!(self.partitions_dumped(service), vec![APPLE_TOOL_PARTITION.to_string()], "{}", self.acl(service));
        assert_eq!(check_item(service, ACCT, Some(self.p())).unwrap(), ItemAccess::Ok);
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
    /// What 0.45.1/0.45.2 did: change the item's data from THIS process.
    /// Used only to reproduce the bug; K2 itself never does this now.
    fn in_process_data_write(&self, service: &str, data: &[u8]) {
        let _ui = NoUi::enter();
        let k = open_keychain(self.p()).unwrap();
        let item = find_item(Some(&k), service, ACCT).unwrap().expect("item exists");
        let st = unsafe {
            security_framework_sys::keychain_item::SecKeychainItemModifyAttributesAndData(
                item.0 as SecKeychainItemRef,
                ptr::null(),
                data.len() as u32,
                data.as_ptr().cast(),
            )
        };
        assert_eq!(st, 0, "in-process modify");
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

fn stdin_line(service: &str, secret: &[u8], opts: &WriteOptions) -> String {
    let (cmd, line) = add_command(service, ACCT, secret, opts).unwrap();
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    assert_eq!(cmd.get_program(), SECURITY);
    assert_eq!(args, vec!["-i".to_string()], "the secret is never on argv");
    line
}

#[test]
fn every_write_is_one_security_stdin_line_hex_then_text_then_refused() {
    let opts = WriteOptions {
        keychain: Some(PathBuf::from("/tmp/k2 test/Library/Keychains/k.keychain-db")),
        label: Some("K2 label".into()),
        trusted_apps: vec!["/usr/bin/security".into(), "/Applications/K2.app/Contents/MacOS/k2".into()],
        replace_acl: false,
    };
    // Small: hex, exactly what Claude Code sends.
    let small = json_payload(500, "small");
    let line = stdin_line("svc", &small, &opts);
    assert!(line.starts_with("add-generic-password -U -a \"k2-test-user\" -s \"svc\" -l \"K2 label\" -T \"/usr/bin/security\" -T \"/Applications/K2.app/Contents/MacOS/k2\" -X \""), "{line}");
    assert!(line.contains(&hex(&small)), "{line}");
    assert!(line.ends_with(" \"/tmp/k2 test/Library/Keychains/k.keychain-db\"\n") && line.matches('\n').count() == 1);
    // Too long as hex, fits as text: `-w` with `"` and `\` escaped.
    let mid = br#"{"claudeAiOauth":{"accessToken":"a\"b\\c"}}"#.iter().copied().chain(std::iter::repeat(b'x').take(2500)).collect::<Vec<u8>>();
    let line = stdin_line("svc", &mid, &opts);
    assert!(!line.contains(" -X "), "{}", &line[..200]);
    assert!(line.contains(r#" -w "{\"claudeAiOauth\":{\"accessToken\":\"a\\\"b\\\\c\"}}xxx"#), "{}", &line[..200]);
    assert!(line.len() <= INTERACTIVE_LINE_MAX);
    // Binary that is too long as hex can't go as text: refused.
    assert_eq!(add_command("svc", ACCT, &binary_payload(2500), &opts).unwrap_err(), KeychainError::TooLarge(2500));
    // Text too long even unescaped: refused, never argv, never in process.
    let big = json_payload(8 * 1024, "big");
    let err = add_command("svc", ACCT, &big, &opts).unwrap_err();
    assert_eq!(err, KeychainError::TooLarge(8 * 1024));
    assert!(err.to_string().contains("won't put it on a command line"), "{err}");
    // A field that could break out of the quoted line.
    assert_eq!(add_command("svc\" -X \"41", ACCT, b"x", &opts).unwrap_err(), KeychainError::BadField("service"));
    // Reads carry only names on argv.
    let cmd = read_command("svc", ACCT, Some(Path::new("/tmp/x")));
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    assert_eq!(args, ["find-generic-password", "-a", ACCT, "-s", "svc", "-g", "/tmp/x"]);
}

#[test]
fn no_source_file_writes_keychain_item_data_in_process() {
    // The ratchet behind the 0.45.2 fix: changing an item's data from a
    // K2 process replaces its partition list with K2's, and every
    // `security` read (Claude Code's) then prompts. Only this module's
    // tests may call these.
    // Calls, not mentions in comments.
    let banned = [
        "SecKeychainItemModifyAttributesAndData(",
        "SecKeychainItemModifyContent(",
        "SecKeychainItemCreateFromContent(",
        "SecKeychainAddGenericPassword(",
        "SecItemAdd(",
        "SecItemUpdate(",
        "set_generic_password(",
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let mut hits = Vec::new();
    for dir in ["crates/k2-core/src", "crates/k2-daemon/src"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    if p.file_name().unwrap() == "macos_keychain_tests.rs" {
                        continue;
                    }
                    let text = std::fs::read_to_string(&p).unwrap();
                    for b in banned {
                        if text.contains(b) {
                            hits.push(format!("{}: {b}", p.display()));
                        }
                    }
                }
            }
        }
    }
    assert!(hits.is_empty(), "in-process keychain data writes: {hits:#?}");
}

#[test]
fn payloads_round_trip_through_write_and_read() {
    let kc = TempKeychain::new("sizes");
    // 100 / 1,900 go as hex; 2,048 / 3,800 as text.
    for n in [100usize, 1900, 2 * 1024, 3800] {
        let svc = format!("k2-test-svc-{n}");
        let first = json_payload(n, "first");
        write(&svc, ACCT, &first, &kc.opts()).unwrap_or_else(|e| panic!("create {n}: {e}"));
        kc.assert_security_readable(&svc);
        assert_eq!(read(&svc, ACCT, Some(kc.p())).unwrap().expect("item exists"), first, "create {n}");
        let second = json_payload(n, "second");
        write(&svc, ACCT, &second, &kc.opts()).unwrap_or_else(|e| panic!("update {n}: {e}"));
        kc.assert_security_readable(&svc);
        assert_eq!(read(&svc, ACCT, Some(kc.p())).unwrap().expect("item exists"), second, "update {n}");
        assert_eq!(kc.count(&svc), 1, "update keeps one item ({n})");
    }
    // Every printable ASCII byte through the text path (`"` and `\` too).
    let mut all: Vec<u8> = (0x20u8..=0x7e).collect::<Vec<u8>>().repeat(25);
    all.extend_from_slice(b"\\\"\\\\\"");
    write("k2-test-ascii", ACCT, &all, &kc.opts()).unwrap();
    assert_eq!(read("k2-test-ascii", ACCT, Some(kc.p())).unwrap().unwrap(), all);
    // Bytes `security -w` would print as hex (NUL, newline, quote).
    let bin = binary_payload(1500);
    write("k2-test-bin", ACCT, &bin, &kc.opts()).unwrap();
    assert_eq!(read("k2-test-bin", ACCT, Some(kc.p())).unwrap().unwrap(), bin);
    // A printable all-hex secret stays text.
    write("k2-test-hexy", ACCT, b"deadbeef", &kc.opts()).unwrap();
    assert_eq!(read("k2-test-hexy", ACCT, Some(kc.p())).unwrap().unwrap(), b"deadbeef");
    // A trailing newline survives.
    write("k2-test-nl", ACCT, b"{\"a\":1}\n", &kc.opts()).unwrap();
    assert_eq!(read("k2-test-nl", ACCT, Some(kc.p())).unwrap().unwrap(), b"{\"a\":1}\n");
    assert_eq!(read("k2-test-missing", ACCT, Some(kc.p())).unwrap(), None);
    // Too big: refused before anything changes.
    let big = json_payload(8 * 1024, "big");
    assert_eq!(write("k2-test-svc-100", ACCT, &big, &kc.opts()).unwrap_err(), KeychainError::TooLarge(8 * 1024));
    assert_eq!(read("k2-test-svc-100", ACCT, Some(kc.p())).unwrap().unwrap(), json_payload(100, "second"));
}

#[test]
fn a_wallet_add_and_restore_cycle_leaves_claudes_item_readable_by_security() {
    // The Appa field report (0.45.2): add a second Claude subscription
    // (Claude signs in through its live item, then K2 puts the previous
    // login back), switch, restore. Afterwards the item's partition list
    // must still be `apple-tool:` and its ACL Claude's, or every Claude
    // read prompts.
    let kc = TempKeychain::new("wallet-cycle");
    let svc = "Claude Code-credentials-k2test";
    let first = json_payload(450, "first-login");
    kc.claude_style_add(svc, &first);
    kc.assert_security_readable(svc);
    let before = kc.acl_shape(svc);
    assert!(before.contains("/usr/bin/security"), "Claude's item trusts security: {before}");

    // Add: K2 reads the live login (to keep it), Claude signs the new one
    // in through the same item, K2 reads it and restores the previous.
    assert_eq!(read(svc, ACCT, Some(kc.p())).unwrap().unwrap(), first);
    let second = json_payload(2600, "second-login-with-mcp-oauth");
    kc.claude_style_add(svc, &second[..1900]); // Claude's own write (≤ 4,032 hex)
    kc.assert_security_readable(svc);
    write(svc, ACCT, &second, &kc.opts()).unwrap(); // K2 fixes up / re-saves (text path)
    kc.assert_security_readable(svc);
    write(svc, ACCT, &first, &kc.opts()).unwrap(); // restore the previous login
    kc.assert_security_readable(svc);
    // Switch to the new login, then back.
    write(svc, ACCT, &second, &kc.opts()).unwrap();
    kc.assert_security_readable(svc);
    write(svc, ACCT, &first, &kc.opts()).unwrap();
    kc.assert_security_readable(svc);

    assert_eq!(kc.acl_shape(svc), before, "the item's ACL and partition list are unchanged");
    let exe = std::env::current_exe().unwrap();
    assert!(!kc.acl(svc).contains(exe.to_str().unwrap()), "the test binary was not added to the ACL");
    assert_eq!(kc.count(svc), 1);
    // Claude Code's own read path (`security find-generic-password -w`).
    let out = sec(&["find-generic-password", "-a", ACCT, "-s", svc, "-w", kc.s()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout.strip_suffix(b"\n").unwrap(), first.as_slice());
    // Claude Code's own write path still updates it in place.
    let refreshed = json_payload(500, "claude-refreshed");
    kc.claude_style_add(svc, &refreshed);
    kc.assert_security_readable(svc);
    assert_eq!(read(svc, ACCT, Some(kc.p())).unwrap().unwrap(), refreshed);
    assert_eq!(kc.count(svc), 1);
}

#[test]
fn an_in_process_write_breaks_the_partition_and_k2_detects_it_without_reading() {
    // Reproduces the 0.45.1/0.45.2 write path, then checks K2's answer.
    let kc = TempKeychain::new("broken");
    let svc = "Claude Code-credentials-k2broken";
    kc.claude_style_add(svc, &json_payload(450, "orig"));
    kc.assert_security_readable(svc);
    kc.in_process_data_write(svc, &json_payload(450, "k2-0452"));
    // The bug: the partition list is now ONLY this binary's.
    let parts = kc.partitions_dumped(svc);
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert!(parts[0].starts_with("cdhash:") || parts[0].starts_with("teamid:"), "{parts:?}");
    assert!(!parts.contains(&APPLE_TOOL_PARTITION.to_string()));

    // Detected in process (no data, no prompt), with the exact fix.
    let ItemAccess::NeedsRepair(r) = check_item(svc, ACCT, Some(kc.p())).unwrap() else {
        panic!("a partition without apple-tool: must need repair");
    };
    assert_eq!(r.partitions, parts, "the in-process ACL read matches dump-keychain");
    assert_eq!(
        r.command,
        format!(
            "security set-generic-password-partition-list -s '{svc}' -a '{ACCT}' -S 'apple-tool:,apple:,{}' '{}'",
            parts[0],
            kc.s()
        )
    );
    // Not read (a `security` read would prompt): an error that carries
    // the fix.
    let err = read(svc, ACCT, Some(kc.p())).unwrap_err();
    assert_eq!(err, KeychainError::NeedsRepair(r.clone()));
    assert!(err.to_string().contains("run this once in Terminal") && err.to_string().contains(&r.command), "{err}");

    // The printed command works (here with -k for the throwaway keychain;
    // in Terminal `security` asks for the login password instead).
    let mut argv: Vec<String> = vec!["set-generic-password-partition-list".into(), "-s".into(), svc.into(), "-a".into(), ACCT.into()];
    argv.extend(["-S".into(), format!("apple-tool:,apple:,{}", parts[0]), "-k".into(), kc.password.clone(), kc.s().into()]);
    let out = sec(&argv.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(out.status.success(), "{out:?}");
    assert_eq!(check_item(svc, ACCT, Some(kc.p())).unwrap(), ItemAccess::Ok);
    assert_eq!(read(svc, ACCT, Some(kc.p())).unwrap().unwrap(), json_payload(450, "k2-0452"));
}

#[test]
fn a_k2_write_to_a_broken_item_repairs_it_without_reading_it() {
    let kc = TempKeychain::new("repair-by-write");
    let svc = "Claude Code-credentials-k2repair";
    kc.claude_style_add(svc, &json_payload(450, "orig"));
    kc.in_process_data_write(svc, &json_payload(450, "k2-0452"));
    assert!(matches!(check_item(svc, ACCT, Some(kc.p())).unwrap(), ItemAccess::NeedsRepair(_)));
    // Deleted in process (no prompt) and created fresh by `security`.
    let fresh = json_payload(3000, "authoritative");
    write(svc, ACCT, &fresh, &kc.opts()).unwrap();
    kc.assert_security_readable(svc);
    assert_eq!(read(svc, ACCT, Some(kc.p())).unwrap().unwrap(), fresh);
    assert_eq!(kc.count(svc), 1);
    // A delete of a broken item is silent too.
    kc.in_process_data_write(svc, &json_payload(450, "k2-0452-again"));
    delete(svc, ACCT, Some(kc.p())).unwrap();
    assert_eq!(kc.count(svc), 0);
    assert_eq!(check_item(svc, ACCT, Some(kc.p())).unwrap(), ItemAccess::Missing);
}

#[test]
fn a_new_item_gets_securitys_acl_or_the_requested_trusted_apps() {
    let kc = TempKeychain::new("new-acl");
    let data = json_payload(1800, "new");
    write("k2-test-plain", ACCT, &data, &kc.opts()).unwrap();
    kc.assert_security_readable("k2-test-plain");
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
    kc.assert_security_readable("k2-test-acl");
    let acl = kc.acl("k2-test-acl");
    assert!(acl.contains("/bin/ls") && acl.contains("/usr/bin/security"), "{acl}");
    // Rewrite with replace_acl: still one item, new data, same ACL.
    let data2 = json_payload(3000, "new2");
    write("k2-test-acl", ACCT, &data2, &opts).unwrap();
    kc.assert_security_readable("k2-test-acl");
    assert_eq!(kc.count("k2-test-acl"), 1);
    assert_eq!(read("k2-test-acl", ACCT, Some(kc.p())).unwrap().unwrap(), data2);
    assert!(kc.acl("k2-test-acl").contains("/bin/ls"));
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
    // Existing item: refused before `security` could ask to unlock.
    let err = write("k2-test-exists", ACCT, b"{\"a\":2}", &kc.opts()).unwrap_err();
    assert!(err.is_locked(), "{err}");
    assert!(err.to_string().contains("unlock the login keychain"), "{err}");
    // Missing item: refused the same way.
    let err = write("k2-test-new", ACCT, b"{\"a\":3}", &kc.opts()).unwrap_err();
    assert!(err.is_locked(), "{err}");
    // Reads too: no `security` unlock dialog.
    let err = read("k2-test-exists", ACCT, Some(kc.p())).unwrap_err();
    assert!(err.is_locked(), "{err}");
    let out = sec(&["unlock-keychain", "-p", &kc.password, kc.s()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(read("k2-test-exists", ACCT, Some(kc.p())).unwrap().unwrap(), b"{\"a\":1}", "unchanged");
    assert_eq!(read("k2-test-new", ACCT, Some(kc.p())).unwrap(), None, "nothing created");
}

#[test]
fn partition_descriptions_parse_from_hex_plists_and_plain_lists() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Partitions</key><array><string>teamid:36B8R93HXV</string></array></dict></plist>"#;
    assert_eq!(parse_partition_description(&hex(xml.as_bytes())).unwrap(), vec!["teamid:36B8R93HXV"]);
    assert_eq!(parse_partition_description("apple-tool:, apple:").unwrap(), vec!["apple-tool:", "apple:"]);
    // Hex that isn't a {Partitions} plist is refused, not guessed at.
    assert_eq!(parse_partition_description(&hex(b"<plist><string>x</string></plist>")), None);
    assert_eq!(
        repair_command("Claude Code-credentials", "rosson", &["teamid:36B8R93HXV".into()], None),
        "security set-generic-password-partition-list -s 'Claude Code-credentials' -a 'rosson' -S 'apple-tool:,apple:,teamid:36B8R93HXV'"
    );
    assert_eq!(
        repair_command("it's", "a", &["apple:".into()], Some(Path::new("/k c"))),
        "security set-generic-password-partition-list -s 'it'\\''s' -a 'a' -S 'apple-tool:,apple:' '/k c'"
    );
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
