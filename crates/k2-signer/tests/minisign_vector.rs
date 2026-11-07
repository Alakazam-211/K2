//! k2-signer's minisign against the real minisign 0.12: tests/fixtures holds
//! a TEST key `minisign -G` made (password `k2-signer-test-password`,
//! minisign's default scrypt limits: N = 2^20, 1 GiB), its public key, a file
//! and the signature `minisign -S` made of it. Ed25519 is deterministic, so
//! the Rust signature must be byte-identical; it must also verify with the
//! independent minisign-verify crate. TEST KEY ONLY.

use std::path::PathBuf;

use k2_signer::minisign::{self, PublicKey, SecretKey};

fn fx(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn signs_byte_identical_to_minisign_0_12() {
    // Not `*.key`: the repo ignores those (real keys never land in git).
    let key = fx("test-secret.minisign");
    let public = PublicKey::parse(&String::from_utf8(fx("test.pub")).unwrap()).unwrap();
    assert_eq!(public.key_id(), "7835E51A2106C44F");
    let data = fx("data.txt");
    let want = String::from_utf8(fx("data.txt.minisig")).unwrap();
    assert!(SecretKey::open(&key, b"wrong password").unwrap_err().contains("wrong password"));
    let sk = SecretKey::open(&key, b"k2-signer-test-password").unwrap();
    assert_eq!(sk.public(), public);
    let tc = "timestamp:1800000000\tfile:data.txt\tversion:1.2.3";
    let got = sk.sign(&data, "signature from tauri secret key", tc).unwrap();
    assert_eq!(got, want, "k2-signer's signature differs from minisign's");
    assert_eq!(minisign::verify(&public.line(), &data, &got).unwrap(), tc);
    // The streamed prehash signs the same.
    let h = minisign::prehash_reader(&data[..]).unwrap();
    assert_eq!(sk.sign_prehashed(&h, "signature from tauri secret key", tc).unwrap(), want);
    // Tauri's form of the key file (base64 of it) opens the same key.
    let tauri = k2_node_proto::crypto::b64(&key);
    assert_eq!(SecretKey::open(tauri.as_bytes(), b"k2-signer-test-password").unwrap().public(), public);
}
