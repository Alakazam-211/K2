//! Test fixtures (k2-core's own tests and, with `test-util`, the daemon's):
//! a publisher-shaped Seed tarball in Sew's layout (`manifest.json`,
//! `build.json`, `server.js`, the inner `SHA256SUMS`), and malformed ones.

use sha2::{Digest, Sha256};

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

pub const COMMIT: &str = "1111111111111111111111111111111111111111";
pub const K2_CAPS: &[&str] = &["chat:read", "chat:post", "voice:join", "voice:video"];

/// A Seed's manifest in Sew's shape (`apps/hearth/manifest.json`).
pub fn manifest(app: &str, version: &str, variant: &str, caps: &[&str], box_services: &[&str]) -> serde_json::Value {
    let prefix = if app == "meet" { "MEET" } else { "HEARTH" };
    let mut reasons = serde_json::Map::new();
    for c in caps {
        reasons.insert(c.to_string(), serde_json::json!("sign-in and chat for the person signed in"));
    }
    let mut m = serde_json::json!({
        "schema": 1,
        "kind": "app",
        "id": app,
        "name": if app == "meet" { "Meet" } else { "Hearth" },
        "version": version,
        "license": "MIT OR Apache-2.0",
        "requires": { "k2": ">=0.46.3", "sew": ">=0.2.0", "voice": true },
        "host": {
            "mode": "cmd",
            "start": ["node", "--disable-warning=ExperimentalWarning", "server.js"],
            "health": { "path": "/healthz", "timeoutSecs": 120 }
        },
        "boxServices": box_services.iter().map(|b| serde_json::json!({ "name": b, "reason": "Sew" })).collect::<Vec<_>>(),
    });
    let mut env = serde_json::Map::new();
    env.insert(format!("{prefix}_PROFILE"), serde_json::json!("k2-tunnel"));
    env.insert(format!("{prefix}_IDENTITY"), serde_json::json!(if variant == "k2" { "k2-bff" } else { "standalone" }));
    m["env"] = serde_json::Value::Object(env);
    if !caps.is_empty() {
        m["caps"] = serde_json::json!(caps);
        m["reasons"] = serde_json::Value::Object(reasons);
    }
    m
}

/// The Seed's files: manifest, build.json, server.js, and SHA256SUMS over
/// the rest.
pub fn seed_files(manifest: &serde_json::Value, build: &serde_json::Value) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("build.json".into(), (build.to_string() + "\n").into_bytes()),
        ("manifest.json".into(), (serde_json::to_string_pretty(manifest).unwrap() + "\n").into_bytes()),
        ("server.js".into(), b"import('./dist/server/main.js')\n".to_vec()),
        ("dist/server/main.js".into(), b"console.log('hello from the fixture')\n".to_vec()),
    ];
    files.sort();
    let sums: String = files.iter().map(|(p, b)| format!("{}  {p}\n", sha256_hex(b))).collect();
    files.push(("SHA256SUMS".into(), sums.into_bytes()));
    files
}

pub fn build_json(app: &str, version: &str, variant: &str, commit: &str) -> serde_json::Value {
    serde_json::json!({ "app": app, "version": version, "variant": variant, "commit": commit })
}

/// A gzip'd tar of `files` (regular files, folders made implicitly).
pub fn tar_gz(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut b = tar::Builder::new(enc);
    for (p, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append_data(&mut h, p, data.as_slice()).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

/// One raw tar entry: the name and link bytes go in as given (no path
/// checks), so a test can build what a hostile publisher would.
pub struct Raw<'a> {
    pub name: &'a [u8],
    pub kind: tar::EntryType,
    pub data: &'a [u8],
    pub link: Option<&'a [u8]>,
}

pub fn tar_gz_raw(entries: &[Raw<'_>]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut b = tar::Builder::new(enc);
    for e in entries {
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..e.name.len()].copy_from_slice(e.name);
        if let Some(l) = e.link {
            h.as_old_mut().linkname[..l.len()].copy_from_slice(l);
        }
        h.set_size(e.data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(e.kind);
        h.set_cksum();
        b.append(&h, e.data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

/// The catalog version that pins `tarball` (and a made-up source file) for
/// `variant`, with K2's tree hash `seed_hash`.
#[allow(clippy::too_many_arguments)]
pub fn version(
    app: &str,
    version: &str,
    variant: &str,
    tarball: &[u8],
    seed_hash: &str,
    caps: &[&str],
    box_services: &[&str],
    setup: &[&str],
) -> super::Version {
    let file = if variant == "k2" { format!("{app}-k2-{version}.tar.gz") } else { format!("{app}-{version}.tar.gz") };
    let mut variants = std::collections::BTreeMap::new();
    variants.insert(
        variant.to_string(),
        super::VariantPin {
            file,
            sha256: sha256_hex(tarball),
            size: tarball.len() as u64,
            seed_hash: seed_hash.to_string(),
            content: None,
            caps: caps.iter().map(|c| c.to_string()).collect(),
            box_services: box_services.iter().map(|c| c.to_string()).collect(),
        },
    );
    super::Version {
        version: version.into(),
        tag: format!("seeds-{version}"),
        commit: COMMIT.into(),
        requires: serde_json::from_value(serde_json::json!({ "k2": ">=0.46.3", "sew": ">=0.2.0", "voice": true })).unwrap(),
        node: Some(">=22.13.0".into()),
        variants,
        source: super::FilePin { file: format!("{app}-{version}-src.tar.gz"), sha256: sha256_hex(b"source"), size: 6 },
        health: super::Health { live: "/healthz".into(), ready: "/readyz".into() },
        data: vec!["hearth-sessions.sqlite".into()],
        setup: setup.iter().map(|s| s.to_string()).collect(),
    }
}
