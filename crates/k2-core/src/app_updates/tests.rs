//! The feed's rules, records and the download-host client (TC-6 … TC-12),
//! with the TEST minisign key (`k2-root-proto` `test-signer`, TA-16) and the
//! stand-in host (`fake_host`). Never a real key, never the network.

use std::time::Duration;

use k2_root_proto::testsign;

use super::dl_client::{Client, DlError};
use super::fake_host::FakeHost;
use super::*;

const OTHER_SEED: [u8; 32] = [7u8; 32];
const OTHER_ID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

fn test_seed() -> [u8; 32] {
    let hex = include_str!("../../../k2-root-proto/tests/fixtures/test-signing-seed.hex").trim();
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

/// The TEST key as Tauri's `pubkey` (base64 of the two-line `.pub`).
fn tauri_pubkey(seed: &[u8; 32], id: [u8; 8]) -> String {
    let line = testsign::public_key_b64(seed, id);
    base64::engine::general_purpose::STANDARD.encode(format!("untrusted comment: minisign public key: TEST\n{line}\n"))
}

/// Tauri's `.sig` file content: base64 of the minisig text.
fn tauri_sig(seed: &[u8; 32], id: [u8; 8], data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(testsign::sign(seed, id, data, "timestamp:1\tfile:MyApp.app.tar.gz"))
}

// ── names ────────────────────────────────────────────────────────────

#[test]
fn channels_follow_the_rule_and_the_hosts_leading_letter() {
    for ok in ["stable", "beta", "staff-beta", "a", "nightly2"] {
        assert!(valid_channel(ok), "{ok}");
    }
    for bad in ["", "Stable", "2beta", "-x", "a_b", "abcdefghijklmnopqrstu", "be ta"] {
        assert!(!valid_channel(bad), "{bad}");
    }
}

#[test]
fn platform_keys_are_tauri_v2s() {
    assert_eq!(parse_platform("darwin-aarch64").expect("ok").key(), "darwin-aarch64");
    assert_eq!(parse_platform("linux-x86_64-appimage").expect("ok").installer.as_deref(), Some("appimage"));
    for bad in ["macos-aarch64", "darwin-arm64", "linux-x86_64-snap", "windows", "", "darwin-aarch64-app-extra"] {
        assert!(parse_platform(bad).is_err(), "{bad}");
    }
}

#[test]
fn artifact_types_and_host_names() {
    assert_eq!(artifact_type("MyApp.app.tar.gz"), Some((".app.tar.gz", "application/gzip")));
    assert_eq!(artifact_type("my_1.0_amd64.AppImage"), Some((".AppImage", "application/x-appimage")));
    assert_eq!(artifact_type("x.AppImage.tar.gz").map(|t| t.0), Some(".AppImage.tar.gz"));
    assert_eq!(artifact_type("setup.nsis.zip").map(|t| t.0), Some(".nsis.zip"));
    for bad in ["MyApp.dmg", "notes.txt", ".deb", "a.tar.gz", "a.zip"] {
        assert_eq!(artifact_type(bad), None, "{bad}");
    }
    assert_eq!(host_file_name("My App 1.5.0.app.tar.gz").as_deref(), Some("My-App-1.5.0.app.tar.gz"));
    assert_eq!(host_file_name("..evil..name.deb").as_deref(), Some("evil.name.deb"));
    assert_eq!(host_file_name("  .msi").as_deref(), Some("app.msi"));
    let long = format!("{}.deb", "a".repeat(300));
    assert_eq!(host_file_name(&long).map(|n| n.len()), Some(128));
}

#[test]
fn versions_are_strict_semver_and_build_metadata_never_counts() {
    for ok in ["1.4.1", "1.4.0-build.42", "0.0.1", "2.0.0-rc.1+sha.abc"] {
        assert!(parse_version(ok).is_ok(), "{ok}");
    }
    for bad in ["1.0", "v1.0.0", "01.2.3", "1.2.3.4", "", "latest"] {
        let e = parse_version(bad).expect_err(bad);
        assert!(e.contains("SemVer 2.0.0") || e.contains("longer"), "{bad}: {e}");
    }
    assert!(newer("1.5.0", "1.4.9"));
    assert!(newer("1.4.0", "1.4.0-build.42"), "a release beats its prerelease");
    assert!(!newer("1.4.0+b2", "1.4.0+b1"), "build metadata is ignored");
    assert!(!newer("1.4.0", "1.4.0"));
    assert!(!newer("garbage", "1.0.0"));
}

#[test]
fn download_ids_are_reverse_dns_with_the_box_name() {
    assert_eq!(dl_id("rosson", "jobs").expect("ok"), "dev.k2.rosson.jobs");
    assert_eq!(dl_id(" Rosson ", "Jobs").expect("ok"), "dev.k2.rosson.jobs");
    assert!(dl_id("", "jobs").expect_err("no box").contains("k2 connect login"));
    for bad in ["ci", "a--b", "-a", "a-", "a_b"] {
        assert!(dl_id("rosson", bad).is_err(), "{bad}");
    }
    assert!(dl_id("ros.son", "jobs").is_err());
}

#[test]
fn dl_tokens_have_the_hosts_shape() {
    assert!(valid_dl_token(&format!("k2dl_{}", "a1".repeat(16))));
    for bad in ["k2dl_", "k2dlr_00000000000000000000000000000000", &format!("k2dl_{}", "A1".repeat(16)), &format!("k2dl_{}", "a".repeat(31))] {
        assert!(!valid_dl_token(bad), "{bad}");
    }
    assert_eq!(token_fingerprint("k2dl_x").len(), 12);
}

// ── keys and signatures ──────────────────────────────────────────────

#[test]
fn a_public_key_parses_in_every_form_tauri_and_minisign_write() {
    let seed = test_seed();
    let wrapped = tauri_pubkey(&seed, testsign::TEST_KEY_ID_BYTES);
    let a = parse_pubkey(&wrapped).expect("wrapped");
    assert_eq!(a.wrapped, wrapped, "Tauri's form is kept byte for byte");
    assert_eq!(a.key_id, "5716343E2F010C58");
    let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(&wrapped).expect("b64")).expect("utf8");
    let b = parse_pubkey(&text).expect("two-line .pub");
    assert_eq!(b.key_id, a.key_id);
    let line = testsign::public_key_b64(&seed, testsign::TEST_KEY_ID_BYTES);
    let c = parse_pubkey(&line).expect("bare line");
    assert_eq!(c.key_id, a.key_id);
    // The normalised forms verify a real signature.
    let data = b"archive bytes";
    let (raw, _) = normalize_signature(&tauri_sig(&seed, testsign::TEST_KEY_ID_BYTES, data)).expect("sig");
    for k in [&a, &b, &c] {
        verify_and_hash(k, &raw, &data[..], MAX_ARTIFACT_BYTES).expect("verifies");
    }
}

#[test]
fn a_secret_key_is_refused_by_name_in_any_form() {
    // A minisign secret key file (rsign/Tauri): its comment says so and
    // its key line is far longer than a public key's 42 bytes.
    let sk_line = base64::engine::general_purpose::STANDARD.encode([b"Ed".as_slice(), b"Sc", &[0u8; 154]].concat());
    let text = format!("untrusted comment: rsign encrypted secret key\n{sk_line}\n");
    let wrapped = base64::engine::general_purpose::STANDARD.encode(&text);
    for form in [text.as_str(), wrapped.as_str(), sk_line.as_str()] {
        let (code, hint) = parse_pubkey(form).expect_err("secret");
        assert_eq!(code, codes::SECRET_KEY_REFUSED, "{form}");
        assert!(hint.contains("signing Mac"), "{hint}");
    }
    let (code, _) = parse_pubkey("not a key").expect_err("junk");
    assert_eq!(code, codes::PUBKEY_INVALID);
    let (code, _) = parse_pubkey("").expect_err("empty");
    assert_eq!(code, codes::PUBKEY_INVALID);
}

#[test]
fn signatures_check_the_key_the_bytes_and_the_cap() {
    let seed = test_seed();
    let key = parse_pubkey(&tauri_pubkey(&seed, testsign::TEST_KEY_ID_BYTES)).expect("key");
    let data = vec![42u8; 300_000];
    let (raw, b64) = normalize_signature(&tauri_sig(&seed, testsign::TEST_KEY_ID_BYTES, &data)).expect("sig");
    assert!(raw.starts_with("untrusted comment"));
    let (raw2, b642) = normalize_signature(&raw).expect("raw form too");
    assert_eq!((raw2.as_str(), b642.as_str()), (raw.as_str(), b64.as_str()), "both forms normalise the same");
    let ok = verify_and_hash(&key, &raw, &data[..], MAX_ARTIFACT_BYTES).expect("good");
    assert_eq!(ok.size, data.len() as u64);
    use sha2::Digest as _;
    let want: String = sha2::Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(ok.sha256, want);
    // Changed bytes.
    let mut bad = data.clone();
    bad[10] ^= 1;
    let (code, hint) = verify_and_hash(&key, &raw, &bad[..], MAX_ARTIFACT_BYTES).err().expect("tampered");
    assert_eq!(code, codes::SIGNATURE_INVALID);
    assert!(hint.contains("doesn't match"), "{hint}");
    // Another key's signature.
    let (other, _) = normalize_signature(&tauri_sig(&OTHER_SEED, OTHER_ID, &data)).expect("sig");
    let (code, hint) = verify_and_hash(&key, &other, &data[..], MAX_ARTIFACT_BYTES).err().expect("wrong key");
    assert_eq!(code, codes::SIGNATURE_INVALID);
    assert!(hint.contains("another key") && hint.contains("5716343E2F010C58"), "{hint}");
    // Over the cap.
    let (code, _) = verify_and_hash(&key, &raw, &data[..], 1000).err().expect("cap");
    assert_eq!(code, codes::ARTIFACT_TOO_LARGE);
    assert!(normalize_signature("garbage").is_err());
}

#[test]
fn dates_are_whole_second_utc() {
    assert_eq!(day_of(0), "1970-01-01");
    assert_eq!(rfc3339(1_761_739_200), "2025-10-29T12:00:00Z");
}

// ── records and the feed ─────────────────────────────────────────────

fn release(conn: &rusqlite::Connection, app: &str, version: &str, ch: &str, platforms: &[&str], live: bool) -> store::ReleaseRow {
    let r = store::insert_release(conn, &store::NewRelease { app_id: app, version, channel: ch, notes: Some("n"), pub_date: "2026-10-29T12:00:00Z", created_by: "t" }, 100)
        .expect("release");
    for p in platforms {
        store::insert_artifact(
            conn,
            &store::ArtifactRow {
                release_id: r.id.clone(),
                platform: p.to_string(),
                file_name: format!("{p}.app.tar.gz"),
                content_type: "application/gzip".into(),
                sha256: "ab".repeat(32),
                size: 10,
                signature: format!("sig-{version}-{p}"),
                url: None,
                upload_state: store::UP_PENDING.into(),
                error: None,
            },
        )
        .expect("artifact");
        if live {
            store::set_artifact_upload(conn, &r.id, p, store::UP_UPLOADED, Some(&format!("https://dl.k2.dev/dev.k2.b.{app}/{version}/x/{p}")), None).expect("up");
        }
    }
    if live {
        store::set_release_state(conn, &r.id, store::STATE_LIVE, None, 101).expect("live");
    }
    r
}

#[test]
fn the_static_and_dynamic_answers_follow_the_channel() {
    let _db = crate::db::scoped_for_test();
    let h = crate::db::shared();
    let conn = h.lock();
    let r1 = release(&conn, "jobs", "1.4.0", "stable", &["darwin-aarch64", "windows-x86_64"], true);
    store::set_channel(&conn, "jobs", "stable", Some(&r1.id), store::ACT_PUSH, "t", 1).expect("ch");
    // 1.5.0 ships macOS only (its push named --drop-platform windows-x86_64).
    let r2 = release(&conn, "jobs", "1.5.0", "stable", &["darwin-aarch64"], true);
    store::set_channel(&conn, "jobs", "stable", Some(&r2.id), store::ACT_PUSH, "t", 2).expect("ch");
    // 1.6.0 is uploading: never served.
    let r3 = release(&conn, "jobs", "1.6.0", "beta", &["darwin-aarch64"], false);
    store::set_channel(&conn, "jobs", "beta", Some(&r3.id), store::ACT_PUSH, "t", 3).expect("ch");
    let f = feed::load(&conn, "jobs").expect("feed");
    assert!(!f.channels.contains_key("beta"), "a channel whose release isn't live serves nothing");
    let st = &f.channels["stable"];
    let m = feed::static_manifest(st.current());
    assert_eq!(m["version"], "1.5.0");
    assert_eq!(m["pub_date"], "2026-10-29T12:00:00Z");
    assert_eq!(m["platforms"]["darwin-aarch64"]["signature"], "sig-1.5.0-darwin-aarch64");
    assert!(m["platforms"]["darwin-aarch64"]["url"].as_str().expect("url").starts_with("https://dl.k2.dev/"));
    // Dynamic: macOS on 1.4.0 gets 1.5.0; Windows on 1.3.0 gets 1.4.0 (the
    // newest release this channel carried that has its platform).
    match feed::dynamic(st, "darwin", "aarch64", "1.4.0") {
        feed::Dynamic::Update { version, body, .. } => {
            assert_eq!(version, "1.5.0");
            assert_eq!(body["notes"], "n");
        }
        other => panic!("{other:?}"),
    }
    match feed::dynamic(st, "windows", "x86_64", "1.3.0") {
        feed::Dynamic::Update { version, platform, .. } => assert_eq!((version.as_str(), platform.as_str()), ("1.4.0", "windows-x86_64")),
        other => panic!("{other:?}"),
    }
    assert_eq!(feed::dynamic(st, "windows", "x86_64", "1.4.0"), feed::Dynamic::NoUpdate);
    assert_eq!(feed::dynamic(st, "darwin", "aarch64", "1.5.0"), feed::Dynamic::NoUpdate);
    assert_eq!(feed::dynamic(st, "linux", "x86_64", "1.0.0"), feed::Dynamic::NoUpdate, "no release has linux");
    assert!(matches!(feed::dynamic(st, "macos", "aarch64", "1.0.0"), feed::Dynamic::Bad(_)));
    assert!(matches!(feed::dynamic(st, "darwin", "aarch64", "one"), feed::Dynamic::Bad(_)));
    // A rollback to 1.4.0: nothing above it is served.
    store::set_channel(&conn, "jobs", "stable", Some(&r1.id), store::ACT_ROLLBACK, "t", 4).expect("rb");
    let f = feed::load(&conn, "jobs").expect("feed");
    assert_eq!(f.channels["stable"].current().version, "1.4.0");
    assert_eq!(f.channels["stable"].releases.len(), 1, "1.5.0 is above the channel now");
    // A pulled release is never served.
    store::set_release_state(&conn, &r1.id, store::STATE_YANKED, None, 5).expect("yank");
    assert!(feed::load(&conn, "jobs").expect("feed").channels.is_empty());
}

#[test]
fn installer_keys_are_picked_for_a_dynamic_caller() {
    let mut r = feed::FeedRelease { id: "r".into(), version: "2.0.0".into(), notes: None, pub_date: "d".into(), platforms: Default::default() };
    r.platforms.insert("linux-x86_64-deb".into(), ("u1".into(), "s".into()));
    r.platforms.insert("linux-x86_64-appimage".into(), ("u2".into(), "s".into()));
    assert_eq!(feed::platform_for(&r, "linux", "x86_64"), Some("linux-x86_64-appimage"));
    r.platforms.insert("linux-x86_64".into(), ("u3".into(), "s".into()));
    assert_eq!(feed::platform_for(&r, "linux", "x86_64"), Some("linux-x86_64"), "the plain key wins");
    assert_eq!(feed::platform_for(&r, "darwin", "x86_64"), None);
    let m = feed::static_manifest(&r);
    assert!(m.get("notes").is_none(), "no notes key when there are none");
}

#[test]
fn an_interrupted_push_fails_and_moves_no_channel() {
    let _db = crate::db::scoped_for_test();
    let h = crate::db::shared();
    let conn = h.lock();
    let r = release(&conn, "jobs", "1.0.0", "stable", &["darwin-aarch64"], false);
    let failed = store::fail_interrupted(&conn, 9).expect("boot");
    assert_eq!(failed, vec![("jobs".to_string(), "1.0.0".to_string(), r.id.clone())]);
    let row = store::release(&conn, &r.id).expect("q").expect("row");
    assert_eq!(row.state, store::STATE_FAILED);
    assert!(store::channels(&conn, "jobs").expect("q").is_empty());
    assert_eq!(store::artifacts(&conn, &r.id).expect("q")[0].upload_state, store::UP_FAILED);
    // A retry of the same version replaces the failed release.
    let again = release(&conn, "jobs", "1.0.0", "stable", &["darwin-aarch64"], true);
    assert_ne!(again.id, r.id);
    assert!(store::release(&conn, &r.id).expect("q").is_none());
}

#[test]
fn check_ins_count_distinct_installs_per_day_and_forget_goes_everywhere() {
    let _db = crate::db::scoped_for_test();
    let h = crate::db::shared();
    let conn = h.lock();
    assert!(store::install_seen_first(&conn, "jobs", "2026-10-29", "h1").expect("q"));
    assert!(!store::install_seen_first(&conn, "jobs", "2026-10-29", "h1").expect("q"));
    assert!(store::install_seen_first(&conn, "jobs", "2026-10-30", "h1").expect("q"), "a new day counts again");
    store::add_checkins(&conn, "jobs", "2026-10-29", "stable", "darwin-aarch64", "1.0.0", 3, 1).expect("add");
    store::add_checkins(&conn, "jobs", "2026-10-29", "stable", "darwin-aarch64", "1.0.0", 2, 1).expect("add");
    let rows = store::checkins(&conn, "jobs", "2026-10-01").expect("q");
    assert_eq!((rows.len(), rows[0].checks, rows[0].installs), (1, 5, 2));
    assert_eq!(store::prune_install_days(&conn, "2026-10-30").expect("prune"), 1);
    store::set_key(&conn, "jobs", "pk", "ID", "t", 1).expect("key");
    let r = release(&conn, "jobs", "1.0.0", "stable", &["darwin-aarch64"], true);
    store::set_channel(&conn, "jobs", "stable", Some(&r.id), store::ACT_PUSH, "t", 1).expect("ch");
    assert_eq!(store::apps_with_feed(&conn).expect("q"), vec!["jobs".to_string()]);
    store::forget_app(&conn, "jobs").expect("forget");
    assert!(store::key(&conn, "jobs").expect("q").is_none());
    assert!(store::releases(&conn, "jobs").expect("q").is_empty());
    assert!(store::checkins(&conn, "jobs", "2000-01-01").expect("q").is_empty());
    assert!(store::apps_with_feed(&conn).expect("q").is_empty());
}

// ── the download-host client against the stand-in ────────────────────

const TOKEN: &str = "k2dl_0123456789abcdef0123456789abcdef";
const ID: &str = "dev.k2.box.jobs";

fn quick(c: &mut Client) {
    c.backoff = Duration::from_millis(5);
    c.max_wait = Duration::from_millis(20);
}

/// A temp folder removed on drop.
struct Tmp(std::path::PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file_with(bytes: &[u8]) -> (Tmp, std::path::PathBuf, String) {
    use sha2::Digest as _;
    let d = Tmp(std::env::temp_dir().join(format!("k2-dl-test-{}", uuid::Uuid::new_v4().simple())));
    std::fs::create_dir_all(&d.0).expect("tmp");
    let p = d.0.join("a.app.tar.gz");
    std::fs::write(&p, bytes).expect("write");
    let sha = sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    (d, p, sha)
}

#[test]
fn uploads_are_idempotent_and_content_addressed() {
    let host = FakeHost::start(&[(ID, TOKEN)]);
    let mut c = Client::new(&host.base, TOKEN).expect("client");
    quick(&mut c);
    let (_d, p, sha) = file_with(b"the archive");
    let up = c.upload(ID, "1.0.0", "a.app.tar.gz", &p, &sha, 11, "application/gzip").expect("upload");
    assert!(up.created);
    assert_eq!(up.url, format!("{}/{ID}/1.0.0/{sha}/a.app.tar.gz", host.base));
    let again = c.upload(ID, "1.0.0", "a.app.tar.gz", &p, &sha, 11, "application/gzip").expect("retry");
    assert!(!again.created, "the same bytes again: 200, not a second file");
    assert_eq!(again.url, up.url);
    // Different bytes under the same name: the host's 409.
    let (_d2, p2, sha2) = file_with(b"other bytes");
    match c.upload(ID, "1.0.0", "a.app.tar.gz", &p2, &sha2, 11, "application/gzip") {
        Err(DlError::Refused { status: 409, code, .. }) => assert_eq!(code, "immutable"),
        other => panic!("{other:?}"),
    }
    // The manifest names only uploaded files.
    let m = serde_json::json!({ "version": "1.0.0", "pub_date": "2026-10-29T12:00:00Z", "platforms": { "darwin-aarch64": { "signature": "s", "url": up.url } } });
    let url = c.put_manifest(ID, "stable", &m).expect("manifest");
    assert_eq!(url, format!("{}/{ID}/stable/latest.json", host.base));
    let bad = serde_json::json!({ "version": "1.0.0", "platforms": { "darwin-aarch64": { "signature": "s", "url": format!("{}/{ID}/1.0.0/{}/nope.deb", host.base, "0".repeat(64)) } } });
    match c.put_manifest(ID, "stable", &bad) {
        Err(DlError::Refused { status: 422, code, .. }) => assert_eq!(code, "bad_manifest"),
        other => panic!("{other:?}"),
    }
    assert_eq!(c.health().expect("health")["ok"], true);
}

#[test]
fn busy_answers_are_retried_and_refusals_mapped() {
    let host = FakeHost::start(&[(ID, TOKEN), ("dev.k2.box.other", "k2dl_ffffffffffffffffffffffffffffffff")]);
    let mut c = Client::new(&host.base, TOKEN).expect("client");
    quick(&mut c);
    let (_d, p, sha) = file_with(b"bytes");
    host.lock().fail_next.extend([503, 500]);
    c.upload(ID, "1.0.0", "a.app.tar.gz", &p, &sha, 5, "application/gzip").expect("third try lands");
    host.lock().fail_next.extend([503, 503, 503, 503]);
    match c.upload(ID, "1.0.1", "a.app.tar.gz", &p, &sha, 5, "application/gzip") {
        Err(DlError::Unavailable(w)) => assert!(w.contains("4 attempts"), "{w}"),
        other => panic!("{other:?}"),
    }
    // Another App's id with this token.
    match c.upload("dev.k2.box.other", "1.0.0", "a.app.tar.gz", &p, &sha, 5, "application/gzip") {
        Err(DlError::Refused { status: 403, code, .. }) => assert_eq!(code, "wrong_app"),
        other => panic!("{other:?}"),
    }
    // A wrong sha256 header: the host's 422.
    match c.upload(ID, "1.0.2", "a.app.tar.gz", &p, &"0".repeat(64), 5, "application/gzip") {
        Err(DlError::Refused { status: 422, code, .. }) => assert_eq!(code, "sha_mismatch"),
        other => panic!("{other:?}"),
    }
    // Over the host's cap.
    host.lock().max_file = 3;
    match c.upload(ID, "1.0.3", "a.app.tar.gz", &p, &sha, 5, "application/gzip") {
        Err(DlError::Refused { status: 413, code, .. }) => assert_eq!(code, "too_large"),
        other => panic!("{other:?}"),
    }
    host.lock().max_file = 1 << 20;
    // A host that lies about what it stored is caught.
    host.lock().lie_sha = true;
    match c.upload(ID, "1.0.4", "a.app.tar.gz", &p, &sha, 5, "application/gzip") {
        Err(DlError::Bad(w)) => assert!(w.contains("sha256"), "{w}"),
        other => panic!("{other:?}"),
    }
    // A token the host doesn't know.
    let mut bad = Client::new(&host.base, "k2dl_00000000000000000000000000000000").expect("client");
    quick(&mut bad);
    match bad.stats(ID, "2026-10-01", "2026-10-29") {
        Err(DlError::Refused { status: 401, code, .. }) => assert_eq!(code, "unauthorized"),
        other => panic!("{other:?}"),
    }
    // The token never shows in Debug or in an error line.
    assert!(!format!("{c:?}").contains(TOKEN));
    assert!(!DlError::Unavailable("x".into()).line().contains(TOKEN));
}

#[test]
fn a_pull_reaches_a_new_host_and_an_old_one_says_not_supported() {
    let host = FakeHost::start(&[(ID, TOKEN)]);
    let mut c = Client::new(&host.base, TOKEN).expect("client");
    quick(&mut c);
    assert_eq!(c.pull_version(ID, "1.0.0").expect("old host"), dl_client::Pulled::NotSupported, "k2dl < 1.4.0: 404");
    host.lock().supports_pull = true;
    assert_eq!(c.pull_version(ID, "1.0.0").expect("new host"), dl_client::Pulled::Done);
    assert_eq!(host.lock().pulled, vec![(ID.to_string(), "1.0.0".to_string())]);
    // Another id with this App's token: the host's 403 wrong_app.
    match c.pull_version("dev.k2.box.other", "1.0.0") {
        Err(DlError::Refused { status: 403, ref code, .. }) if code == "wrong_app" => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unreachable_host_is_unavailable_not_zero() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", l.local_addr().expect("addr"));
    drop(l);
    let mut c = Client::new(&base, TOKEN).expect("client");
    quick(&mut c);
    c.attempts = 2;
    assert!(matches!(c.stats(ID, "2026-10-01", "2026-10-29"), Err(DlError::Unavailable(_))));
}

#[test]
fn downloads_add_up_per_version() {
    let s = serde_json::json!({ "versions": [
        { "version": "1.0.0", "files": [ { "days": [ { "day": "d1", "downloads": 3, "bytes": 30 }, { "day": "d2", "downloads": 1, "bytes": 10 } ] },
                                          { "days": [ { "day": "d1", "downloads": 2, "bytes": 20 } ] } ] },
        { "version": "1.1.0", "files": [] } ] });
    let (by, total) = dl_client::downloads_by_version(&s);
    assert_eq!(by["1.0.0"], (6, 60));
    assert_eq!(by["1.1.0"], (0, 0));
    assert_eq!(total, 6);
    assert!(dl_client::url_is_for("https://dl.k2.dev/dev.k2.b.a/1.0.0/abc/f.deb", "dev.k2.b.a", "1.0.0", "abc"));
    assert!(!dl_client::url_is_for("https://dl.k2.dev/dev.k2.b.a/1.0.0/abd/f.deb", "dev.k2.b.a", "1.0.0", "abc"));
    assert!(!dl_client::url_is_for("ftp://x/dev.k2.b.a/1.0.0/abc/f.deb", "dev.k2.b.a", "1.0.0", "abc"));
}
