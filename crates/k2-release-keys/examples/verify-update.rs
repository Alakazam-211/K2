//! Check published update files with the SAME Rust verifier the app and the
//! daemon run, against the keys compiled into this checkout
//! (`UPDATER_KEYS`). For the person checking a release on z3mbpZ; nothing
//! here signs or uploads.
//!
//!   cargo run -q -p k2-release-keys --example verify-update -- \
//!       app <latest.json> <K2.app.tar.gz> <platform url> <version>
//!   cargo run -q -p k2-release-keys --example verify-update -- \
//!       daemon <file> <file.sig2> <asset name> <version>
//!
//! Exit 0 and one `ok` line, or exit 1 with the verifier's refusal.

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let read = |p: &str| std::fs::read(p).unwrap_or_else(|e| fail(&format!("read {p}: {e}")));
    let r = match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["app", manifest, bundle, url, version] => {
            let raw: serde_json::Value = serde_json::from_slice(&read(manifest)).unwrap_or_else(|e| fail(&format!("{manifest}: {e}")));
            if raw.get("version").and_then(|v| v.as_str()) != Some(*version) {
                fail(&format!("{manifest} is for {:?}, not {version}", raw.get("version")));
            }
            k2_release_keys::verify_app_update(&raw, url, version, &read(bundle))
        }
        ["daemon", file, sig2, asset, version] => {
            let sig = String::from_utf8(read(sig2)).unwrap_or_else(|e| fail(&format!("{sig2}: {e}")));
            k2_release_keys::verify_updater_sig(&read(file), &sig, asset, version)
        }
        _ => fail("usage: verify-update app <latest.json> <bundle> <url> <version> | daemon <file> <file.sig2> <asset> <version>"),
    };
    match r {
        Ok(v) => println!("ok: signed by {} ({}), trusted comment {:?}", v.key_id, v.role, v.trusted_comment),
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("verify-update: REFUSED: {msg}");
    std::process::exit(1)
}
