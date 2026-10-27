//! T1, T2, T5 (prd-security-sweep-0464-v1 §5) with throwaway keys made
//! inside each test. Nothing here reads a real secret key.

use super::*;
use ring::rand::SecureRandom as _;
use ring::signature::Ed25519KeyPair;

struct TestKey {
    seed: [u8; 32],
    id: [u8; 8],
    line: String,
}

fn new_key() -> TestKey {
    let rng = ring::rand::SystemRandom::new();
    let mut seed = [0u8; 32];
    let mut id = [0u8; 8];
    rng.fill(&mut seed).expect("random seed");
    rng.fill(&mut id).expect("random key id");
    let line = k2_root_proto::testsign::public_key_b64(&seed, id);
    TestKey { seed, id, line }
}

impl TestKey {
    fn trusted(&self, role: &'static str) -> TrustedKey {
        TrustedKey {
            role,
            key_id: Box::leak(key_id_of(&self.line).expect("key id").into_boxed_str()),
            public_key_b64: Box::leak(self.line.clone().into_boxed_str()),
        }
    }
    fn sign(&self, data: &[u8], tc: &str) -> String {
        k2_root_proto::testsign::sign(&self.seed, self.id, data, tc)
    }
    /// A legacy (non-prehashed, `Ed`) minisign signature.
    fn sign_legacy(&self, data: &[u8], tc: &str) -> String {
        let kp = Ed25519KeyPair::from_seed_unchecked(&self.seed).expect("seed");
        let sig = kp.sign(data);
        let mut bin1 = b"Ed".to_vec();
        bin1.extend_from_slice(&self.id);
        bin1.extend_from_slice(sig.as_ref());
        let mut g = sig.as_ref().to_vec();
        g.extend_from_slice(tc.as_bytes());
        let global = kp.sign(&g);
        let b64 = base64::engine::general_purpose::STANDARD;
        format!(
            "untrusted comment: legacy\n{}\ntrusted comment: {tc}\n{}\n",
            b64.encode(bin1),
            b64.encode(global.as_ref())
        )
    }
}

fn tc(file: &str, version: &str) -> String {
    format!("timestamp:1792800000\tfile:{file}\tversion:{version}")
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

// ── T2: verify_updater_sig ──────────────────────────────────────────

#[test]
fn accepts_primary_and_backup_with_matching_file_and_version() {
    let (u2, u3) = (new_key(), new_key());
    let keys = [u2.trusted("primary"), u3.trusted("backup")];
    let data = b"daemon bytes";
    let a = "k2-daemon-linux-x86_64";
    let v = verify_updater_sig_with(&keys, data, &u2.sign(data, &tc(a, "0.46.4")), a, "0.46.4").expect("U2 verifies");
    assert_eq!(v.role, "primary");
    assert_eq!(v.key_id, key_id_of(&u2.line).expect("id"));
    let v = verify_updater_sig_with(&keys, data, &u3.sign(data, &tc(a, "0.46.4")), a, "0.46.4").expect("U3 verifies");
    assert_eq!(v.role, "backup");
    // Tauri's base64-wrapped form is accepted too.
    let wrapped = b64(&u2.sign(data, &tc(a, "0.46.4")));
    verify_updater_sig_with(&keys, data, &wrapped, a, "0.46.4").expect("wrapped verifies");
}

#[test]
fn refuses_untrusted_key_wrong_file_wrong_version_legacy_garbage_and_tampering() {
    let (u1, u2) = (new_key(), new_key());
    let keys = [u2.trusted("primary")];
    let data = b"daemon bytes";
    let a = "k2-daemon-linux-x86_64";
    let good = u2.sign(data, &tc(a, "0.46.4"));

    let e = verify_updater_sig_with(&keys, data, &u1.sign(data, &tc(a, "0.46.4")), a, "0.46.4").unwrap_err();
    assert!(e.contains("doesn't trust"), "U1-only signature: {e}");
    let e = verify_updater_sig_with(&keys, data, &good, "k2-daemon-linux-aarch64", "0.46.4").unwrap_err();
    assert!(e.contains("names file"), "{e}");
    let e = verify_updater_sig_with(&keys, data, &good, a, "0.46.5").unwrap_err();
    assert!(e.contains("for version"), "{e}");
    let no_version = u2.sign(data, &format!("timestamp:1\tfile:{a}"));
    let e = verify_updater_sig_with(&keys, data, &no_version, a, "0.46.4").unwrap_err();
    assert!(e.contains("for version []"), "{e}");
    let no_file = u2.sign(data, "timestamp:1\tversion:0.46.4");
    assert!(verify_updater_sig_with(&keys, data, &no_file, a, "0.46.4").unwrap_err().contains("names file []"));
    let two_files = u2.sign(data, &format!("timestamp:1\tfile:{a}\tfile:other\tversion:0.46.4"));
    assert!(verify_updater_sig_with(&keys, data, &two_files, a, "0.46.4").is_err());
    // A space is not a field separator here (strict, unlike RH24).
    let spaced = u2.sign(data, &format!("timestamp:1 file:{a} version:0.46.4"));
    assert!(verify_updater_sig_with(&keys, data, &spaced, a, "0.46.4").is_err());
    let legacy = u2.sign_legacy(data, &tc(a, "0.46.4"));
    let e = verify_updater_sig_with(&keys, data, &legacy, a, "0.46.4").unwrap_err();
    assert!(e.contains("check failed"), "legacy: {e}");
    assert!(verify_updater_sig_with(&keys, data, "garbage", a, "0.46.4").unwrap_err().contains("decode failed"));
    // Changed bytes, and a changed (unsigned) trusted comment.
    assert!(verify_updater_sig_with(&keys, b"other bytes", &good, a, "0.46.4").is_err());
    let forged = good.replace("version:0.46.4", "version:0.46.9");
    assert!(verify_updater_sig_with(&keys, data, &forged, a, "0.46.9").is_err());
    // No keys: nothing verifies.
    assert!(verify_updater_sig_with(&[], data, &good, a, "0.46.4").unwrap_err().contains("ceremony"));
}

// ── T5: the desktop app update (latest.json) ────────────────────────

fn latest_json(version: &str, url: &str, u1_sig: &str, k2: Option<(&str, &str)>) -> serde_json::Value {
    let mut entry = serde_json::json!({ "signature": b64(u1_sig), "url": url });
    if let Some((id, sig)) = k2 {
        entry["k2_signatures"] = serde_json::json!({ id: b64(sig) });
    }
    serde_json::json!({ "version": version, "platforms": { "darwin-aarch64": entry } })
}

/// What a 0.46.3 app does (tauri-plugin-updater 2.13 `verify_signature`):
/// the plugin's one config key, the `signature` field, legacy allowed, and
/// a `version:` only checked when present.
fn old_app_accepts(u1_tauri_pubkey: &str, raw: &serde_json::Value, data: &[u8]) -> bool {
    let b = base64::engine::general_purpose::STANDARD;
    let pk_text = String::from_utf8(b.decode(u1_tauri_pubkey).expect("pk b64")).expect("pk utf8");
    let pk = minisign_verify::PublicKey::decode(&pk_text).expect("pk");
    let sig_b64 = raw["platforms"]["darwin-aarch64"]["signature"].as_str().expect("signature");
    let sig = minisign_verify::Signature::decode(&String::from_utf8(b.decode(sig_b64).expect("sig b64")).expect("utf8")).expect("sig");
    if pk.verify(data, &sig, true).is_err() {
        return false;
    }
    match sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")) {
        Some(v) => v == raw["version"].as_str().expect("version"),
        None => true,
    }
}

#[test]
fn a_dual_signed_app_update_installs_on_old_and_new_apps() {
    let (u1, u2, u3) = (new_key(), new_key(), new_key());
    let keys = [u2.trusted("primary"), u3.trusted("backup")];
    let url = "https://github.com/Alakazam-211/K2/releases/download/v0.46.4/K2.app.tar.gz";
    let data = b"app bundle";
    // U1 exactly as release.sh makes it (no version), U2 with version.
    let u1_sig = u1.sign(data, "timestamp:1792800000\tfile:K2.app.tar.gz");
    let u2_sig = u2.sign(data, &tc("K2.app.tar.gz", "0.46.4"));
    let id2 = key_id_of(&u2.line).expect("id");
    let raw = latest_json("0.46.4", url, &u1_sig, Some((&id2, &u2_sig)));

    assert!(old_app_accepts(&tauri_pubkey(&u1.line).expect("tauri pk"), &raw, data), "an install that trusts only U1 accepts it");
    let v = verify_app_update_with(&keys, &raw, url, "0.46.4", data).expect("an install that trusts only U2/U3 accepts it");
    assert_eq!(v.role, "primary");

    // Signed with U1 only (a pre-0.46.4 release): the new app refuses.
    let old = latest_json("0.46.3", url, &u1_sig, None);
    let e = verify_app_update_with(&keys, &old, url, "0.46.3", data).unwrap_err();
    assert!(e.contains("no K2 signature"), "{e}");
    // A U1 signature filed under the U2 key id: refused.
    let swapped = latest_json("0.46.4", url, &u1_sig, Some((&id2, &u1_sig)));
    assert!(verify_app_update_with(&keys, &swapped, url, "0.46.4", data).is_err());
    // Tampered bytes, announced version, URL (another asset) and the key id.
    assert!(verify_app_update_with(&keys, &raw, url, "0.46.4", b"evil bundle").is_err());
    let e = verify_app_update_with(&keys, &raw, url, "0.46.5", data).unwrap_err();
    assert!(e.contains("for version"), "{e}");
    let other_url = url.replace("K2.app.tar.gz", "K2-old.app.tar.gz");
    let mut moved = raw.clone();
    moved["platforms"]["darwin-aarch64"]["url"] = serde_json::json!(other_url);
    let e = verify_app_update_with(&keys, &moved, &other_url, "0.46.4", data).unwrap_err();
    assert!(e.contains("is not K2.app.tar.gz"), "the asset is pinned per platform, not taken from the URL: {e}");
    // A platform K2 doesn't publish: refused even with a good signature.
    let mut odd = raw.clone();
    let entry = odd["platforms"]["darwin-aarch64"].clone();
    odd["platforms"] = serde_json::json!({ "linux-x86_64": entry });
    assert!(verify_app_update_with(&keys, &odd, url, "0.46.4", data).unwrap_err().contains("no app update for platform"));
    // Windows: the setup name carries the version.
    let win_url = "https://dl.invalid/v0.46.4/K2_0.46.4_x64-setup.exe";
    let win_sig = u2.sign(data, &tc("K2_0.46.4_x64-setup.exe", "0.46.4"));
    let win = serde_json::json!({ "version": "0.46.4", "platforms": { "windows-x86_64": {
        "signature": b64(&u1_sig), "url": win_url, "k2_signatures": { id2.clone(): b64(&win_sig) } } } });
    verify_app_update_with(&keys, &win, win_url, "0.46.4", data).expect("windows setup verifies");
    let e = verify_app_update_with(&keys, &win, win_url, "0.46.5", data).unwrap_err();
    assert!(e.contains("is not K2_0.46.5_x64-setup.exe"), "{e}");
    assert!(verify_app_update_with(&keys, &raw, &other_url, "0.46.4", data).unwrap_err().contains("no platform"));
    let unknown = latest_json("0.46.4", url, &u1_sig, Some(("0123456789ABCDEF", &u2_sig)));
    assert!(verify_app_update_with(&keys, &unknown, url, "0.46.4", data).unwrap_err().contains("trusts"));
    // The backup key alone (after losing U2) still installs.
    let id3 = key_id_of(&u3.line).expect("id");
    let backup = latest_json("0.46.4", url, &u1_sig, Some((&id3, &u3.sign(data, &tc("K2.app.tar.gz", "0.46.4")))));
    assert_eq!(verify_app_update_with(&keys, &backup, url, "0.46.4", data).expect("U3").role, "backup");
}

#[test]
fn helpers() {
    let k = new_key();
    let tp = tauri_pubkey(&k.line).expect("tauri pk");
    let file = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(tp).expect("b64")).expect("utf8");
    assert_eq!(file.lines().nth(1), Some(k.line.as_str()));
    assert!(file.starts_with("untrusted comment: minisign public key: "));
    assert_eq!(key_id_of(LEGACY_UPDATER_KEY_B64).as_deref(), Some(LEGACY_UPDATER_KEY_ID));
    assert_eq!(url_asset_name("https://x/y/K2.app.tar.gz?a=1"), Some("K2.app.tar.gz"));
    assert!(version_older_than("0.46.3", FIRST_U2_SIGNED_VERSION));
    // v0.46.4 shipped signed with U1 only (`4ee5c5e61`): a 0.46.5+ verifier
    // must call it a retired-key release, never point people at it.
    assert!(version_older_than("0.46.4", FIRST_U2_SIGNED_VERSION));
    assert!(!version_older_than("0.46.5", FIRST_U2_SIGNED_VERSION));
    assert!(!version_older_than("0.47.0", FIRST_U2_SIGNED_VERSION));
    assert!(version_older_than("v0.9.99", FIRST_U2_SIGNED_VERSION));
    assert!(version_older_than("junk", FIRST_U2_SIGNED_VERSION));
}

// ── T1: the copies agree; U1 is in no verifier ──────────────────────

fn repo_file(rel: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn shell_list(text: &str, var: &str) -> Vec<String> {
    let prefix = format!("{var}=\"");
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with(&prefix)).collect();
    assert_eq!(lines.len(), 1, "{var} must be assigned exactly once");
    let v = lines[0].strip_prefix(&prefix).and_then(|r| r.strip_suffix('"')).unwrap_or_else(|| panic!("{var} line: {}", lines[0]));
    v.split_whitespace().map(str::to_string).collect()
}

#[test]
fn key_copies_are_equal_and_u1_is_in_no_verifier() {
    let ours: Vec<String> = UPDATER_KEYS.iter().map(|k| k.public_key_b64.to_string()).collect();
    for k in UPDATER_KEYS {
        assert_eq!(key_id_of(k.public_key_b64).as_deref(), Some(k.key_id), "key id of {}", k.public_key_b64);
        assert_ne!(k.public_key_b64, LEGACY_UPDATER_KEY_B64, "U1 must never be a trusted update key");
    }
    if !UPDATER_KEYS.is_empty() {
        assert_eq!(UPDATER_KEYS[0].role, "primary");
    }
    let cli = repo_file("cli/k2");
    let inst = repo_file("scripts/install-daemon.sh");
    assert_eq!(shell_list(&cli, "_DAEMON_UPDATE_PUBKEYS"), ours, "cli/k2 _DAEMON_UPDATE_PUBKEYS");
    assert_eq!(shell_list(&inst, "K2_DAEMON_PUBKEYS"), ours, "scripts/install-daemon.sh K2_DAEMON_PUBKEYS");
    // The first U2-signed version: one value in all three verifiers (the
    // shell installers refuse older --version requests with it).
    assert_eq!(shell_list(&cli, "_DAEMON_FIRST_U2_VERSION"), [FIRST_U2_SIGNED_VERSION], "cli/k2 _DAEMON_FIRST_U2_VERSION");
    assert_eq!(shell_list(&inst, "K2_FIRST_U2_VERSION"), [FIRST_U2_SIGNED_VERSION], "scripts/install-daemon.sh K2_FIRST_U2_VERSION");
    let conf: serde_json::Value = serde_json::from_str(&repo_file("src-tauri/tauri.conf.json")).expect("tauri.conf.json");
    let pk = conf["plugins"]["updater"]["pubkey"].as_str().expect("plugins.updater.pubkey");
    let want = match UPDATER_KEYS.first() {
        Some(primary) => tauri_pubkey(primary.public_key_b64).expect("primary"),
        // Before the ceremony lands the plugin keeps U1 (nothing verifies with it).
        None => tauri_pubkey(LEGACY_UPDATER_KEY_B64).expect("U1"),
    };
    assert_eq!(pk, want, "tauri.conf.json plugins.updater.pubkey");
    // U1, raw or wrapped, appears in no verifier.
    let u1_wrapped = tauri_pubkey(LEGACY_UPDATER_KEY_B64).expect("U1");
    for (name, text) in [("cli/k2", &cli), ("scripts/install-daemon.sh", &inst)] {
        assert!(!text.contains(LEGACY_UPDATER_KEY_B64) && !text.contains(&u1_wrapped), "{name} still carries U1");
    }
    for f in ["crates/k2-daemon/src/update_routes.rs", "src-tauri/src/commands/updater.rs"] {
        let t = repo_file(f);
        assert!(!t.contains(LEGACY_UPDATER_KEY_B64) && !t.contains(&u1_wrapped), "{f} still carries U1");
    }
}

// ── Interop with the REAL signers (fixtures) ────────────────────────
//
// tests/fixtures/real-minisign was written by
// tests/packaging/updater-signing-real-minisign.sh: U1 made by the real Tauri
// signer (tauri-cli 2.12), every signature made by real minisign 0.12 through
// scripts/lib/sign-updater.sh (k2u_sign), exactly as release.sh signs.
// Throwaway keys; only public keys, data and signatures are in the folder.

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real-minisign").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn fixture_text(name: &str) -> String {
    String::from_utf8(fixture(name)).expect("utf8 fixture").trim().to_string()
}

/// What a 0.46.4 daemon does (frozen): the one embedded U1 key in Tauri's
/// wrapped form, prehashed only, then RH24 (`file:k2-daemon-…`).
fn old_daemon_accepts(u1_tauri_pubkey: &str, sig: &str, data: &[u8]) -> bool {
    let b = base64::engine::general_purpose::STANDARD;
    let pk_text = String::from_utf8(b.decode(u1_tauri_pubkey).expect("pk b64")).expect("pk utf8");
    let pk = minisign_verify::PublicKey::decode(pk_text.trim()).expect("pk");
    let Ok(sig) = minisign_verify::Signature::decode(sig.trim()) else { return false };
    if pk.verify(data, &sig, false).is_err() {
        return false;
    }
    sig.trusted_comment().split(['\t', ' ']).find_map(|f| f.strip_prefix("file:")).is_some_and(|f| f.starts_with("k2-daemon-"))
}

fn fixture_keys() -> [TrustedKey; 2] {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let (u2, u3) = (fixture_text("u2.pub.line"), fixture_text("u3.pub.line"));
    [
        TrustedKey { role: "primary", key_id: leak(key_id_of(&u2).expect("u2 id")), public_key_b64: leak(u2) },
        TrustedKey { role: "backup", key_id: leak(key_id_of(&u3).expect("u3 id")), public_key_b64: leak(u3) },
    ]
}

#[test]
fn real_minisign_daemon_signatures_satisfy_old_and_new_daemons() {
    let keys = fixture_keys();
    let a = "k2-daemon-linux-x86_64";
    let bin = fixture(a);
    let (sig1, sig2) = (fixture_text(&format!("{a}.sig")), fixture_text(&format!("{a}.sig2")));
    let v = verify_updater_sig_with(&keys, &bin, &sig2, a, "9.9.9").expect("a 0.46.5 daemon accepts the real U2 .sig2");
    assert_eq!(v.role, "primary");
    assert_eq!(v.key_id, fixture_text("u2.id"));
    assert!(old_daemon_accepts(&fixture_text("u1.tauri.pub"), &sig1, &bin), "a 0.46.4 daemon accepts the real U1 .sig (Tauri-signer key, minisign 0.12)");
    // Each half only works where it should.
    assert!(verify_updater_sig_with(&keys, &bin, &sig1, a, "9.9.9").is_err(), "U1 is not trusted by 0.46.5");
    assert!(!old_daemon_accepts(&fixture_text("u1.tauri.pub"), &sig2, &bin), "U2 means nothing to 0.46.4");
    assert!(verify_updater_sig_with(&keys, &bin, &sig2, a, "9.9.8").is_err(), "version is bound");
    let mut changed = bin.clone();
    changed[0] ^= 1;
    assert!(verify_updater_sig_with(&keys, &changed, &sig2, a, "9.9.9").is_err());
    assert!(!old_daemon_accepts(&fixture_text("u1.tauri.pub"), &sig1, &changed));
}

#[test]
fn real_minisign_app_signatures_satisfy_old_and_new_apps() {
    let keys = fixture_keys();
    let bundle = fixture("K2.app.tar.gz");
    let url = "https://github.com/Alakazam-211/K2/releases/download/v9.9.9/K2.app.tar.gz";
    // latest.json exactly as release.sh Step 8 writes it: `signature` = the
    // b64 .sig file's text; k2_signatures[U2 id] = base64 of the raw .sig2.
    let raw = serde_json::json!({ "version": "9.9.9", "platforms": { "darwin-aarch64": {
        "signature": fixture_text("K2.app.tar.gz.sig"),
        "url": url,
        "k2_signatures": { fixture_text("u2.id"): b64(&String::from_utf8(fixture("K2.app.tar.gz.sig2")).expect("utf8")) },
    } } });
    let v = verify_app_update_with(&keys, &raw, url, "9.9.9", &bundle).expect("a 0.46.5 app accepts the real U2 signature");
    assert_eq!(v.role, "primary");
    assert!(old_app_accepts(&fixture_text("u1.tauri.pub"), &raw, &bundle), "a 0.46.4 app (plugin, U1) accepts the real bridge signature");
    assert!(verify_app_update_with(&keys, &raw, url, "9.9.10", &bundle).is_err(), "version is bound");
    assert!(verify_app_update_with(&keys, &raw, url, "9.9.9", b"other bytes").is_err());
}
