//! `k2-app-engine` (`prd-k2-app-runtime-v1` §10.6 option C, S11/S12): the
//! small binary that runs as `k2app` and drives rootless Podman for the
//! guarded App path, on the daemon's orders only.
//!
//! This module is the contract the daemon and the engine share:
//! - fixed paths (the socket, the engine's state under `k2app`'s home);
//! - signed builds: the engine is installed by the root helper
//!   (`apps install-engine`) and is signed with the **root-helper key**,
//!   like `k2-node` (G8). The trusted comment's file name keeps the three
//!   binaries apart: `file:k2-app-engine-linux-<arch> version:<v>
//!   protocol:<n>`; the build marker is
//!   `K2_APP_ENGINE_BUILD v=<v> proto=<n> arch=<arch>;`;
//! - the wire: one request per connection, a JSON header line, an optional
//!   raw body of a declared length, one JSON answer line;
//! - the validators both sides run on every field (the engine trusts the
//!   daemon process, which is the trust boundary, but still refuses any
//!   path, id or value outside these shapes).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::apps::{AppId, APP_HOME, GATEWAY_PORTS, RESERVED_PORTS};
use crate::keys::{key_id_of, TrustedKey};
use crate::sig::parse_trusted_comment;
use crate::version::{arch_ok, elf_matches, Semver};

// ── paths ───────────────────────────────────────────────────────────────

/// Root-owned `k2app:k2app 0750` + an ACL `u:<daemon user>:x` (tmpfiles.d,
/// written by `apps install-engine`). The socket inside is the engine's.
pub const RUN_DIR: &str = "/run/k2app";
pub const SOCKET: &str = "/run/k2app/engine.sock";
pub const TMPFILES_FILE: &str = "/etc/tmpfiles.d/k2app.conf";
/// The engine's user unit (for `k2app` only: `ConditionUser=`).
pub const USER_UNIT_DIR: &str = "/etc/systemd/user";
pub const USER_UNIT_FILE: &str = "k2-app-engine.service";
pub const USER_UNIT: &str = "k2-app-engine.service";

/// Under `k2app`'s home (the engine writes here; the helper never does).
pub fn seeds_dir() -> String {
    format!("{APP_HOME}/seeds")
}
pub fn apps_dir() -> String {
    format!("{APP_HOME}/apps")
}
pub fn state_dir() -> String {
    format!("{APP_HOME}/engine")
}
/// Rootless Quadlet's per-user unit dir for `k2app`.
pub fn quadlet_dir() -> String {
    format!("{APP_HOME}/.config/containers/systemd")
}

/// Container, unit, Quadlet file and per-deployment state stem:
/// `k2app-<app>--<deployment>` (an App id never holds `--`, a deployment
/// id is `d-<12 hex>`, so the split is unambiguous).
pub fn unit_stem(app: &AppId, deployment: &str) -> String {
    format!("k2app-{}--{deployment}", app.as_str())
}

/// The App's own slice inside `user@<k2app>`: nested in `k2apps.slice`
/// by systemd's dash rule, with the App id's dashes as underscores so App
/// `nsi-skin` never nests inside App `nsi`.
pub fn app_slice(app: &AppId) -> String {
    format!("k2apps-{}.slice", app.unit_safe())
}

// ── per-App user namespace slots ────────────────────────────────────────

/// Ids per App user namespace (W: a Next BFF image needs < 65536).
pub const SLOT_SIZE: u64 = 65_536;
/// Box prep gives `k2app` 64 × 65536 subordinate ids.
pub const MAX_SLOTS: u64 = crate::apps::SUBID_COUNT / SLOT_SIZE;

/// The first id of slot `n` in `k2app`'s rootless namespace (0 = `k2app`
/// itself, 1.. = its subordinate ids in order).
pub fn slot_base(slot: u64) -> u64 {
    1 + slot * SLOT_SIZE
}

/// The App's gateway link port (S14 listens there; pasta `-T` forwards it
/// from the container's loopback; the per-App nft rule admits it).
pub fn gateway_port(slot: u64) -> Option<u16> {
    let p = GATEWAY_PORTS.lo as u64 + slot;
    (p <= GATEWAY_PORTS.hi as u64).then_some(p as u16)
}

// ── signed builds ───────────────────────────────────────────────────────

/// What this build of the engine speaks on its socket.
pub const ENGINE_PROTOCOL: u32 = 1;

pub fn asset_name(arch: &str) -> String {
    format!("k2-app-engine-linux-{arch}")
}

pub fn trusted_comment(arch: &str, version: &str, protocol: u32) -> String {
    format!("file:{} version:{version} protocol:{protocol}", asset_name(arch))
}

/// The marker needle, stored shifted by one byte so the plain prefix
/// appears in a binary only inside its one embedded marker.
pub fn marker_prefix() -> String {
    let shifted: &[u8] = std::hint::black_box(b"L3`BQQ`FOHJOF`CVJME!w>");
    shifted.iter().map(|b| (b - 1) as char).collect()
}

pub fn format_marker(version: &str, protocol: u32, arch: &str) -> String {
    format!("{}{version} proto={protocol} arch={arch};", marker_prefix())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineMarker {
    pub version: Semver,
    pub protocol: u32,
    pub arch: String,
}

/// Every engine marker in `bytes`; `Err` on any doctored occurrence.
pub fn find_markers(bytes: &[u8]) -> Result<Vec<EngineMarker>, &'static str> {
    let needle = marker_prefix();
    let needle = needle.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let rest = &bytes[i + needle.len()..];
        let end = rest.iter().take(80).position(|&b| b == b';').ok_or("marker not terminated")?;
        let body = std::str::from_utf8(&rest[..end]).map_err(|_| "marker not utf-8")?;
        let mut it = body.split(' ');
        let version = it.next().and_then(Semver::parse).ok_or("marker malformed")?;
        let protocol = it.next().and_then(|p| p.strip_prefix("proto=")).ok_or("marker malformed")?;
        let arch = it.next().and_then(|a| a.strip_prefix("arch=")).ok_or("marker malformed")?;
        if it.next().is_some() || !arch_ok(arch) || protocol.is_empty() || protocol.len() > 4 || !protocol.bytes().all(|b| b.is_ascii_digit()) {
            return Err("marker malformed");
        }
        out.push(EngineMarker { version, protocol: protocol.parse().map_err(|_| "marker malformed")?, arch: arch.into() });
        i += needle.len() + end + 1;
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEngineBuild {
    pub version: Semver,
    pub protocol: u32,
    pub arch: String,
    pub key_id: String,
    pub key_role: &'static str,
}

/// The one check (the helper before installing, the daemon before
/// spending a root call): a minisign signature by one of `keys` (incl.
/// the global signature over the trusted comment); the comment names this
/// arch's engine asset; a 64-bit ELF for that arch; exactly one marker,
/// equal to the comment. Static refusal strings only.
pub fn verify_engine_build(bytes: &[u8], sig_text: &str, keys: &[TrustedKey], arch: &str) -> Result<VerifiedEngineBuild, &'static str> {
    if !arch_ok(arch) {
        return Err("unsupported arch");
    }
    let sig = minisign_verify::Signature::decode(sig_text).map_err(|_| "signature unreadable")?;
    let mut verified: Option<&TrustedKey> = None;
    for k in keys {
        let Ok(pk) = minisign_verify::PublicKey::from_base64(k.public_key_b64) else { continue };
        if pk.verify(bytes, &sig, false).is_ok() {
            verified = Some(k);
            break;
        }
    }
    let key = verified.ok_or("signature not valid for any trusted key")?;
    let comment = parse_trusted_comment(sig.trusted_comment()).ok_or("trusted comment malformed")?;
    if comment.asset != asset_name(arch) {
        return Err("trusted comment names another file");
    }
    if !elf_matches(bytes, arch) {
        return Err("not an ELF for this arch");
    }
    let markers = find_markers(bytes)?;
    if markers.len() != 1 {
        return Err("build marker missing or repeated");
    }
    let m = &markers[0];
    if m.version != comment.version || m.protocol != comment.protocol || m.arch != arch {
        return Err("build marker does not match the trusted comment");
    }
    Ok(VerifiedEngineBuild {
        version: comment.version,
        protocol: comment.protocol,
        arch: arch.into(),
        key_id: key_id_of(key.public_key_b64).unwrap_or_else(|| key.key_id.to_string()),
        key_role: key.role,
    })
}

// ── canned bytes the helper writes ──────────────────────────────────────

/// `/etc/systemd/user/k2-app-engine.service`. A user unit that only
/// `k2app`'s manager runs (`ConditionUser=`); enabled by the helper with
/// `systemctl --user` as `k2app`. No root anywhere on this path.
pub fn user_unit() -> &'static str {
    "# K2 App engine (prd-k2-app-runtime-v1 §10.6). Written by\n\
     # k2-root-helper `apps install-engine`. Runs only in k2app's manager.\n\
     [Unit]\n\
     Description=K2 App engine\n\
     ConditionUser=k2app\n\
     After=default.target\n\
     \n\
     [Service]\n\
     Type=simple\n\
     ExecStart=/usr/local/libexec/k2-app-engine serve\n\
     Restart=always\n\
     RestartSec=2\n\
     NoNewPrivileges=yes\n\
     UMask=0077\n\
     LimitNOFILE=8192\n\
     \n\
     [Install]\n\
     WantedBy=default.target\n"
}

/// `/etc/tmpfiles.d/k2app.conf`: the socket dir, traversable only by
/// `k2app` and each configured daemon user (ACL), recreated every boot.
pub fn tmpfiles_conf(daemon_users: &[&str]) -> String {
    let mut s = String::from(
        "# K2 App engine socket dir. Written by k2-root-helper `apps install-engine`.\n\
         d /run/k2app 0750 k2app k2app -\n",
    );
    for u in daemon_users {
        s.push_str(&format!("a+ /run/k2app - - - - u:{u}:x\n"));
    }
    s
}

// ── the wire ────────────────────────────────────────────────────────────

/// Largest request header line (a start carries the App's secrets).
pub const MAX_HEADER: usize = 1024 * 1024;
/// Largest Seed tarball and image archive streamed to the engine.
pub const MAX_SEED_BYTES: u64 = 4 << 30;
pub const MAX_IMAGE_BYTES: u64 = 4 << 30;
/// Largest logs / exec output the engine returns.
pub const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// `{engine version, protocol, runtime, podman}`.
    Version,
    SeedHas { hash: String },
    /// Followed by `len` bytes of a tar of the Seed folder.
    SeedPut { hash: String, len: u64 },
    ImageHas { image: String },
    /// Followed by `len` bytes of an OCI archive (made by the daemon with
    /// skopeo from the pinned digest); loaded and tagged `image`.
    ImageLoad { image: String, len: u64 },
    /// Allocate the App's slot, make its folders, and pay the first-start
    /// cost of its id mapping (W: 13–20 s) before any switch.
    Prepare { app: String, image: String, seed: String },
    Start(Box<StartSpec>),
    Stop { app: String, deployment: String },
    Liveness { app: String, deployment: String },
    Stats { app: String, deployment: String },
    Logs { app: String, deployment: String, lines: u32 },
    Exec { app: String, deployment: String, argv: Vec<String> },
    Adopt,
    /// Stop every instance of an App; `data` also deletes its folders and
    /// frees its slot (uninstall --purge).
    RemoveApp { app: String, data: bool },
}

/// One secret for a container: a Podman secret, never in the unit file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretItem {
    /// `Some(VAR)`: an environment variable. `None`: a file
    /// `/run/secrets/<file>` (mode 0400).
    pub env: Option<String>,
    pub file: Option<String>,
    /// The value, hex (file secrets may be binary).
    pub value_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartSpec {
    pub app: String,
    pub deployment: String,
    pub seed: String,
    pub image: String,
    /// Host loopback port the App's `$PORT` is published on.
    pub port: u16,
    pub argv: Vec<String>,
    /// Declared, non-secret variables plus K2's own (an env file; never
    /// systemd-expanded).
    pub env: BTreeMap<String, String>,
    pub memory_mib: u64,
    /// The App's drain window; the container's stop timeout is longer
    /// (AR45: `StopTimeout` ≥ drain).
    pub drain_secs: u64,
    pub secrets: Vec<SecretItem>,
    /// The App pass (S14), hex; a Podman secret mounted at
    /// [`PASS_TARGET`].
    pub pass_hex: Option<String>,
}

/// Where the App pass appears inside the container (`K2_PASS_FILE`).
pub const PASS_TARGET: &str = "/run/k2/pass";
pub const SECRETS_TARGET_DIR: &str = "/run/secrets";

/// One answer line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Answer {
    pub fn ok(data: serde_json::Value) -> Self {
        Self { ok: true, code: None, message: None, data }
    }
    pub fn err(code: &str, message: impl Into<String>) -> Self {
        Self { ok: false, code: Some(code.into()), message: Some(message.into()), data: serde_json::Value::Null }
    }
}

/// Error codes the engine answers with.
pub mod codes {
    pub const CALLER_NOT_DAEMON: &str = "caller_not_daemon";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const NOT_FOUND: &str = "not_found";
    pub const NO_SLOT: &str = "no_slot";
    pub const PODMAN: &str = "podman_failed";
    pub const SYSTEMD: &str = "systemd_failed";
    pub const IO: &str = "io_failed";
    pub const TOO_LARGE: &str = "too_large";
}

// ── validators ──────────────────────────────────────────────────────────

pub fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect()
}

/// `d-<12 lowercase hex>` (k2-core `store::insert_deployment`).
pub fn deployment_ok(d: &str) -> bool {
    d.len() == 14 && d.starts_with("d-") && d[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A Seed hash: 64 lowercase hex.
pub fn seed_hash_ok(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// K2's local base-image names: `localhost/k2-base/<name>:<tag>`, lower
/// alnum and `-` in the name, `[a-z0-9.-]` in the tag.
pub fn image_ok(i: &str) -> bool {
    let Some(rest) = i.strip_prefix("localhost/k2-base/") else { return false };
    let Some((name, tag)) = rest.split_once(':') else { return false };
    !name.is_empty()
        && name.len() <= 40
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !tag.is_empty()
        && tag.len() <= 64
        && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

/// The App's own published port: ≥ 1024, outside K2's reserved range.
pub fn app_port_ok(p: u16) -> bool {
    p >= 1024 && !(RESERVED_PORTS.0..=RESERVED_PORTS.1).contains(&p)
}

/// POSIX-ish env name, not one K2 sets for the container itself.
pub fn env_name_ok(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_uppercase() || b[0] == b'_' || b[0].is_ascii_lowercase())
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// No control characters (an env file line, a unit line), bounded.
pub fn value_ok(v: &str, max: usize) -> bool {
    v.len() <= max && !v.chars().any(|c| c.is_control())
}

/// A file secret's name under `/run/secrets`.
pub fn secret_file_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Every field of a start, checked (the engine runs this before anything).
pub fn check_start(s: &StartSpec) -> Result<AppId, &'static str> {
    let app = AppId::parse(&s.app).ok_or("app id")?;
    if !deployment_ok(&s.deployment) {
        return Err("deployment id");
    }
    if !seed_hash_ok(&s.seed) {
        return Err("seed hash");
    }
    if !image_ok(&s.image) {
        return Err("image");
    }
    if !app_port_ok(s.port) {
        return Err("port");
    }
    if s.argv.is_empty() || s.argv.len() > 64 || !s.argv.iter().all(|a| !a.is_empty() && value_ok(a, 4096)) {
        return Err("argv");
    }
    if s.env.len() > 256 || !s.env.iter().all(|(k, v)| env_name_ok(k) && value_ok(v, 32 * 1024)) {
        return Err("env");
    }
    if !(64..=65_536).contains(&s.memory_mib) {
        return Err("memory");
    }
    if s.drain_secs > 600 {
        return Err("drain");
    }
    if s.secrets.len() > 64 {
        return Err("secrets");
    }
    for it in &s.secrets {
        match (&it.env, &it.file) {
            (Some(e), None) if env_name_ok(e) => {}
            (None, Some(f)) if secret_file_ok(f) => {}
            _ => return Err("secret target"),
        }
        if it.value_hex.len() > 2 * 1024 * 1024 || hex_decode(&it.value_hex).is_none() {
            return Err("secret value");
        }
    }
    if let Some(p) = &s.pass_hex {
        if p.len() > 64 * 1024 || hex_decode(p).is_none() {
            return Err("pass");
        }
    }
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> StartSpec {
        StartSpec {
            app: "nsi-skin".into(),
            deployment: "d-0123456789ab".into(),
            seed: "a".repeat(64),
            image: "localhost/k2-base/node24:ec857ea6bb60".into(),
            port: 41234,
            argv: vec!["node".into(), "server.mjs".into()],
            env: [("NODE_ENV".to_string(), "production".to_string())].into(),
            memory_mib: 512,
            drain_secs: 30,
            secrets: vec![SecretItem { env: None, file: Some("db.json".into()), value_hex: hex_encode(b"{}") }],
            pass_hex: Some(hex_encode(b"k2skn_x")),
        }
    }

    #[test]
    fn names_and_slots() {
        let a = AppId::parse("nsi-skin").unwrap();
        assert_eq!(unit_stem(&a, "d-0123456789ab"), "k2app-nsi-skin--d-0123456789ab");
        assert_eq!(app_slice(&a), "k2apps-nsi_skin.slice");
        assert_eq!(slot_base(0), 1);
        assert_eq!(slot_base(1), 65_537);
        assert_eq!(MAX_SLOTS, 64);
        assert_eq!(slot_base(MAX_SLOTS - 1) + SLOT_SIZE - 1, crate::apps::SUBID_COUNT);
        assert_eq!(gateway_port(0), Some(38_600));
        assert_eq!(gateway_port(MAX_SLOTS - 1), Some(38_663));
        assert_eq!(gateway_port(400), None);
    }

    #[test]
    fn start_checks() {
        assert!(check_start(&spec()).is_ok());
        type Tweak = Box<dyn Fn(&mut StartSpec)>;
        let bad: Vec<Tweak> = vec![
            Box::new(|s| s.app = "../x".into()),
            Box::new(|s| s.app = "a--b".into()),
            Box::new(|s| s.deployment = "d-0123456789AB".into()),
            Box::new(|s| s.deployment = "d-../../etc".into()),
            Box::new(|s| s.seed = "../".repeat(10)),
            Box::new(|s| s.image = "docker.io/library/node:24".into()),
            Box::new(|s| s.image = "localhost/k2-base/node24:latest;rm".into()),
            Box::new(|s| s.port = 80),
            Box::new(|s| s.port = 38_700),
            Box::new(|s| s.argv = vec![]),
            Box::new(|s| s.argv = vec!["node\n[Service]".into()]),
            Box::new(|s| {
                s.env.insert("BAD-NAME".into(), "x".into());
            }),
            Box::new(|s| {
                s.env.insert("X".into(), "a\nb".into());
            }),
            Box::new(|s| s.memory_mib = 1),
            Box::new(|s| s.drain_secs = 3600),
            Box::new(|s| s.secrets[0].file = Some("../shadow".into())),
            Box::new(|s| s.secrets[0].env = Some("X".into())),
            Box::new(|s| s.secrets[0].value_hex = "zz".into()),
            Box::new(|s| s.pass_hex = Some("abc".into())),
        ];
        for (i, f) in bad.iter().enumerate() {
            let mut s = spec();
            f(&mut s);
            assert!(check_start(&s).is_err(), "case {i}");
        }
    }

    #[test]
    fn wire_round_trips_and_refuses_unknown_fields() {
        let r = Request::Start(Box::new(spec()));
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.starts_with("{\"verb\":\"start\""), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
        let r = Request::SeedPut { hash: "a".repeat(64), len: 9 };
        assert_eq!(serde_json::to_string(&r).unwrap(), format!("{{\"verb\":\"seed-put\",\"hash\":\"{}\",\"len\":9}}", "a".repeat(64)));
        assert!(serde_json::from_str::<Request>(&format!("{{\"verb\":\"seed-has\",\"hash\":\"{}\",\"x\":1}}", "a".repeat(64))).is_err());
        assert!(serde_json::from_str::<Request>("{\"verb\":\"stop\",\"app\":\"a\",\"deployment\":\"d-0123456789ab\",\"graceSecs\":1}").is_err());
        assert!(serde_json::from_str::<Request>("{\"verb\":\"shell\",\"cmd\":\"id\"}").is_err());
        assert_eq!(hex_decode(&hex_encode(b"\x00\xffk2")).unwrap(), b"\x00\xffk2");
    }

    #[test]
    fn engine_markers_are_their_own() {
        let m = format_marker("0.46.1", 1, "x86_64");
        assert!(m.starts_with("K2_APP_ENGINE_BUILD v=0.46.1 proto=1 arch=x86_64;"));
        assert_eq!(find_markers(m.as_bytes()).unwrap().len(), 1);
        assert!(crate::version::find_markers(m.as_bytes()).unwrap().is_empty());
        assert!(crate::node_build::find_markers(m.as_bytes()).unwrap().is_empty());
        let helper = crate::version::format_marker("0.46.1", 6, "x86_64");
        assert!(find_markers(helper.as_bytes()).unwrap().is_empty());
        assert!(find_markers(format!("{}0.46.1 proto=1 arch=mips;", marker_prefix()).as_bytes()).is_err());
    }

    #[cfg(feature = "test-signer")]
    #[test]
    fn signed_engine_builds_verify_and_each_tamper_fails() {
        use crate::keys::TEST_KEY;
        use crate::testsign::{fake_elf, sign_with_test_key};
        let keys = [TEST_KEY];
        let elf = fake_elf("x86_64", &format_marker("0.46.1", 1, "x86_64"));
        let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.1", 1));
        let v = verify_engine_build(&elf, &sig, &keys, "x86_64").unwrap();
        assert_eq!((v.version, v.protocol), (Semver(0, 46, 1), 1));
        assert!(verify_engine_build(&elf, &sig, crate::keys::RELEASE_KEYS, "x86_64").is_err());
        assert!(verify_engine_build(&elf, &sig, &keys, "aarch64").is_err());
        let mut bad = elf.clone();
        bad[70] ^= 1;
        assert!(verify_engine_build(&bad, &sig, &keys, "x86_64").is_err());
        // Domain separation: a helper or node comment over the same bytes.
        let s = sign_with_test_key(&elf, &crate::sig::trusted_comment("x86_64", "0.46.1", 1));
        assert_eq!(verify_engine_build(&elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
        let s = sign_with_test_key(&elf, &crate::node_build::trusted_comment("linux", "x86_64", "0.46.1", 1));
        assert_eq!(verify_engine_build(&elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
        let s = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.2", 1));
        assert_eq!(verify_engine_build(&elf, &s, &keys, "x86_64"), Err("build marker does not match the trusted comment"));
        // And the helper's verifier refuses an engine comment.
        let s = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.1", 1));
        assert_eq!(crate::sig::verify_signed_build(&elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
    }

    #[test]
    fn canned_unit_and_tmpfiles() {
        let u = user_unit();
        assert!(u.contains("\nConditionUser=k2app\n"));
        assert!(u.contains("\nExecStart=/usr/local/libexec/k2-app-engine serve\n"));
        assert!(u.contains("\nNoNewPrivileges=yes\n"));
        let t = tmpfiles_conf(&["k2", "k2std"]);
        assert!(t.contains("d /run/k2app 0750 k2app k2app -\n"));
        assert!(t.contains("a+ /run/k2app - - - - u:k2:x\n"));
        assert!(t.contains("a+ /run/k2app - - - - u:k2std:x\n"));
    }
}
