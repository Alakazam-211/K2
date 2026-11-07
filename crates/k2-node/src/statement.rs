//! The node's self-statement (protocol 9, slice C0;
//! prd-ci-on-compute-for-clients-v1 §4.1, vs-live V8–V10; the format is
//! `docs/ci-formats-v1.md` §2, [`k2_node_proto::statement::NodeStatement`]).
//!
//! What the node says about itself, signed with its own key, and sent to
//! every server that speaks protocol 9 (`node_statement`, feature
//! `node-statement`). Receipt v2 names the statement in force when its job
//! started (`run.statement_digest`).
//!
//! Node-owned files, all in `state/` (0700, the node user):
//! - [`KEY_META_FILE`]: the key's epoch, birth time and whether the taint
//!   flag existed when it was made. A key this build makes is
//!   `flag_since_birth`; a key from before is not, so its taint reads
//!   `unknown` until it is re-keyed (C1) or the owner attests it (V12).
//! - [`TAINT_FILE`]: V10, every key that ever started a job under the wall
//!   `none` (`--dev`, an executor that isn't ready, `--no-job-wall`). Never
//!   cleared for a key. Mirrored into root's executor state (the
//!   `key_taint` job op, [`crate::admin::jobs::JobOp::KeyTaint`]) so a job
//!   running as the node user can't erase it by deleting this file; the
//!   mark is kept here until root acknowledged it. Self-reported either way:
//!   it catches mistakes, not a hostile root (vs-live S5).
//! - [`STATEMENTS_DIR`]: every statement this node signed lately, by
//!   digest, so a receipt shipped later (a reconnect) still finds the one
//!   its job started under.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use k2_node_proto::ci::{self, KeyStore, Signed, Taint};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::host::HostFacts;
use k2_node_proto::statement::{self, KeyExposure, NodeStatement, NODE_STATEMENT_V};
use serde::{Deserialize, Serialize};

/// `state/node-key.meta.json`.
pub const KEY_META_FILE: &str = "node-key.meta.json";
/// `state/key-taint.json` (node side) and `<executor state>/key-taint.json`
/// (root side): the same shape.
pub const TAINT_FILE: &str = "key-taint.json";
/// `state/statements/<sha256 hex>.json`.
pub const STATEMENTS_DIR: &str = "statements";
/// How many signed statements are kept (the newest; the current one always).
pub const STATEMENTS_KEEP: usize = 64;

/// The node key's own facts (node-owned; written once per key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyMeta {
    pub node_fp: String,
    /// 0 for the enrolment key, +1 per rotation (C1).
    pub key_epoch: u32,
    pub key_born_at: i64,
    /// The key this one replaced (C1); `None` at epoch 0.
    #[serde(default)]
    pub prev_fp: Option<String>,
    /// The taint flag existed when this key was made, so "never tainted"
    /// can be said as `clean`; otherwise the key is `unknown` (V10).
    pub flag_since_birth: bool,
}

/// Read the key's meta, or make it: `created` means this process just
/// made the key (so the flag covers its whole life). A meta that names
/// another key (a key swapped by hand) is replaced as a key of unknown
/// history. Written 0600.
pub fn key_meta(state: &Path, key: &SigningKey, created: bool, now: i64) -> Result<KeyMeta, String> {
    let path = state.join(KEY_META_FILE);
    let fp = key.fingerprint();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(m) = serde_json::from_str::<KeyMeta>(&text) {
            if m.node_fp == fp {
                return Ok(m);
            }
        }
    }
    let born = if created {
        now
    } else {
        std::fs::metadata(state.join("node-key.pem"))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(now)
            .min(now)
    };
    let m = KeyMeta { node_fp: fp, key_epoch: 0, key_born_at: born, prev_fp: None, flag_since_birth: created };
    let body = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&path, &body, 0o600)?;
    Ok(m)
}

/// One key's taint mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaintMark {
    /// When the first job started under the wall `none` with this key.
    pub at: i64,
    /// Why there was no wall (`--dev`, the executor's answer, …).
    pub why: String,
    /// Node side only: root's copy has it too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root_ack: bool,
}

/// [`TAINT_FILE`]: tainted keys by fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaintFile {
    #[serde(default)]
    pub keys: BTreeMap<String, TaintMark>,
}

/// Read a taint file; a missing file is empty. A file that doesn't parse
/// is an error: the caller treats the key as tainted (fail closed).
pub fn read_taint(path: &Path) -> Result<TaintFile, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TaintFile::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

pub fn write_taint(path: &Path, f: &TaintFile) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?;
    crate::util::atomic_write(path, &body, 0o600)
}

/// Mark `fp` tainted (never un-marks; an earlier mark keeps its time and
/// reason). True when the mark is new.
pub fn mark_taint(path: &Path, fp: &str, why: &str, now: i64) -> Result<bool, String> {
    // An unreadable file is replaced by one with this mark: the mark is what
    // matters, and the statement already reads an unreadable file as
    // tainted.
    let mut f = read_taint(path).unwrap_or_default();
    if f.keys.contains_key(fp) {
        return Ok(false);
    }
    let why: String = why.chars().filter(|c| !c.is_control()).take(200).collect();
    f.keys.insert(fp.to_string(), TaintMark { at: now, why, root_ack: false });
    write_taint(path, &f)?;
    Ok(true)
}

/// The taint of `meta`'s key, from the node's file and root's answer
/// (`root`: `Some(true)` tainted there, `Some(false)` not, `None` no
/// answer). An unreadable node file counts as tainted.
pub fn taint_of(node_file: &Result<TaintFile, String>, root: Option<bool>, meta: &KeyMeta) -> Taint {
    let here = match node_file {
        Ok(f) => f.keys.contains_key(&meta.node_fp),
        Err(_) => true,
    };
    if here || root == Some(true) {
        Taint::Tainted
    } else if meta.flag_since_birth {
        Taint::Clean
    } else {
        Taint::Unknown
    }
}

/// V9: Yama's `ptrace_scope` (0–3), Linux only; `None` without Yama.
pub fn ptrace_scope() -> Option<u8> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    parse_ptrace_scope(&std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok()?)
}

pub fn parse_ptrace_scope(text: &str) -> Option<u8> {
    text.trim().parse::<u8>().ok().filter(|p| *p <= 3)
}

/// The machine id behind `host_id` (salted by
/// [`statement::host_id`] before it leaves the node): `/etc/machine-id` on
/// Linux, the IOPlatformUUID on macOS. Read once.
pub fn machine_id() -> Option<String> {
    static ID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let id = if cfg!(target_os = "macos") {
            std::process::Command::new("/usr/sbin/ioreg")
                .args(["-rd1", "-c", "IOPlatformExpertDevice"])
                .output()
                .ok()
                .and_then(|o| parse_platform_uuid(&String::from_utf8_lossy(&o.stdout)))
        } else {
            ["/etc/machine-id", "/var/lib/dbus/machine-id"].iter().find_map(|p| std::fs::read_to_string(p).ok())
        };
        id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() <= 128)
    })
    .clone()
}

/// `"IOPlatformUUID" = "…"` from `ioreg -rd1 -c IOPlatformExpertDevice`.
pub fn parse_platform_uuid(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("\"IOPlatformUUID\"")?.trim().strip_prefix('=')?.trim();
        Some(rest.trim_matches('"').to_string())
    })
}

/// The wire spelling of the node's wall.
pub fn wire_wall(w: &crate::wall::Wall) -> ci::Wall {
    match w {
        crate::wall::Wall::None { .. } => ci::Wall::None,
        crate::wall::Wall::Seatbelt => ci::Wall::Seatbelt,
        crate::wall::Wall::JobUser { .. } => ci::Wall::JobUser,
    }
}

/// Everything a statement says, gathered by the node.
pub struct Facts<'a> {
    pub key: &'a SigningKey,
    pub meta: &'a KeyMeta,
    pub wall: ci::Wall,
    pub dev: bool,
    pub taint: Taint,
    /// The OS user that owns the key file (the node user).
    pub key_owner: &'a str,
    /// The key file's mode bits.
    pub key_mode: u32,
    pub version: &'a str,
    pub commit: Option<&'a str>,
    pub protocol: u32,
    pub os: &'a str,
    pub arch: &'a str,
    pub host: &'a HostFacts,
    pub ptrace_scope: Option<u8>,
    pub machine_id: Option<&'a str>,
}

fn opt(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.chars().filter(|c| !c.is_control()).take(128).collect())
}

/// The statement for `f`, issued at `now`. Never signed here.
pub fn build(f: &Facts<'_>, now: i64) -> NodeStatement {
    let distro = opt(&f.host.distro).map(|d| d.to_ascii_lowercase());
    NodeStatement {
        v: NODE_STATEMENT_V,
        node_fp: f.key.fingerprint(),
        node_spki: crypto::b64(f.key.spki_der()),
        key_epoch: f.meta.key_epoch,
        key_born_at: f.meta.key_born_at,
        prev_fp: f.meta.prev_fp.clone(),
        issued_at: now.max(f.meta.key_born_at),
        wall: f.wall,
        dev: f.dev,
        taint: f.taint,
        key_exposure: KeyExposure {
            store: KeyStore::File,
            owner: f.key_owner.chars().filter(|c| !c.is_control()).take(64).collect(),
            mode: Some(f.key_mode & 0o7777),
            // A job runs as the key's own uid only without a wall (Seatbelt
            // denies the node's state; the Linux wall is another uid).
            job_readable: f.wall == ci::Wall::None,
        },
        k2node_version: f.version.to_string(),
        k2node_commit: f.commit.map(str::to_string),
        protocol: f.protocol,
        os: f.os.to_string(),
        arch: f.arch.to_string(),
        distro,
        distro_version: opt(&f.host.distro_version),
        kernel: opt(&f.host.kernel),
        ptrace_scope: f.ptrace_scope,
        host_id: f.machine_id.map(statement::host_id),
    }
}

/// The statement in force: kept until a fact changes, so its digest (and
/// what receipts name) stays the same across offers and sessions.
#[derive(Debug, Clone, Default)]
pub struct Current {
    pub doc: Option<NodeStatement>,
    pub signed: Option<Signed>,
}

impl Current {
    /// Sign `next` unless it says what the current one says (ignoring
    /// `issued_at`). True when a new statement was signed.
    pub fn update(&mut self, next: NodeStatement, key: &SigningKey) -> Result<bool, String> {
        if let Some(cur) = &self.doc {
            if (NodeStatement { issued_at: cur.issued_at, ..next.clone() }) == *cur {
                return Ok(false);
            }
        }
        let signed = next.sign(key)?;
        self.doc = Some(next);
        self.signed = Some(signed);
        Ok(true)
    }

    pub fn digest(&self) -> Option<String> {
        self.signed.as_ref().map(Signed::digest)
    }
}

fn statement_path(state: &Path, digest: &str) -> Option<PathBuf> {
    let hex = digest.strip_prefix("sha256:").filter(|h| ci::is_fp(h))?;
    Some(state.join(STATEMENTS_DIR).join(format!("{hex}.json")))
}

/// Keep `s` by its digest; the oldest beyond [`STATEMENTS_KEEP`] go.
pub fn store(state: &Path, s: &Signed) -> Result<(), String> {
    let dir = state.join(STATEMENTS_DIR);
    crate::util::mkdir_mode(&dir, 0o700)?;
    let path = statement_path(state, &s.digest()).ok_or("statement digest")?;
    let body = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&path, &body, 0o600)?;
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if all.len() > STATEMENTS_KEEP {
        all.sort();
        let extra = all.len() - STATEMENTS_KEEP;
        for (_, p) in all.into_iter().take(extra) {
            if p != path {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    Ok(())
}

/// A statement this node signed, by digest (checked: the stored text
/// hashes to it and still opens with its own key).
pub fn lookup(state: &Path, digest: &str) -> Option<Signed> {
    let p = statement_path(state, digest)?;
    let s: Signed = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    (s.digest() == digest && NodeStatement::open(&s).is_ok()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostFacts {
        HostFacts {
            os: "linux".into(),
            arch: "x86_64".into(),
            distro: "Ubuntu".into(),
            distro_version: "24.04".into(),
            kernel: "6.8.0-45-generic".into(),
            ..Default::default()
        }
    }

    fn facts<'a>(key: &'a SigningKey, meta: &'a KeyMeta, h: &'a HostFacts, taint: Taint, wall: ci::Wall) -> Facts<'a> {
        Facts {
            key,
            meta,
            wall,
            dev: false,
            taint,
            key_owner: "k2node",
            key_mode: 0o100600,
            version: "0.46.10",
            commit: Some("0123456789abcdef0123456789abcdef01234567"),
            protocol: 9,
            os: "linux",
            arch: "x86_64",
            host: h,
            ptrace_scope: Some(1),
            machine_id: Some("0123456789abcdef0123456789abcdef"),
        }
    }

    #[test]
    fn a_new_key_is_tracked_from_birth_an_old_one_is_unknown() {
        let d = crate::util::temp_dir("stmt-meta");
        let state = d.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let k = SigningKey::generate().unwrap();
        let m = key_meta(&state, &k, true, 1_700_000_000).unwrap();
        assert!(m.flag_since_birth);
        assert_eq!((m.key_epoch, m.key_born_at, m.prev_fp.clone()), (0, 1_700_000_000, None));
        // Read back, not remade (a later process doesn't reset the birth).
        assert_eq!(key_meta(&state, &k, false, 1_800_000_000).unwrap(), m);
        // Another key (swapped by hand): unknown history.
        let k2 = SigningKey::generate().unwrap();
        let m2 = key_meta(&state, &k2, false, 1_800_000_000).unwrap();
        assert!(!m2.flag_since_birth);
        assert_eq!(m2.node_fp, k2.fingerprint());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(state.join(KEY_META_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn taint_is_never_cleared_and_unreadable_reads_as_tainted() {
        let d = crate::util::temp_dir("stmt-taint");
        let p = d.join(TAINT_FILE);
        let k = SigningKey::generate().unwrap();
        let meta = KeyMeta { node_fp: k.fingerprint(), key_epoch: 0, key_born_at: 1, prev_fp: None, flag_since_birth: true };
        assert_eq!(taint_of(&read_taint(&p), None, &meta), Taint::Clean);
        assert_eq!(taint_of(&read_taint(&p), Some(true), &meta), Taint::Tainted, "root's copy counts alone");
        assert!(mark_taint(&p, &meta.node_fp, "--dev\nnode", 5).unwrap());
        assert!(!mark_taint(&p, &meta.node_fp, "later", 9).unwrap(), "the first mark stays");
        let f = read_taint(&p).unwrap();
        assert_eq!(f.keys[&meta.node_fp].at, 5);
        assert_eq!(f.keys[&meta.node_fp].why, "--devnode", "no control characters");
        assert_eq!(taint_of(&read_taint(&p), Some(false), &meta), Taint::Tainted, "root not told yet: still tainted");
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(taint_of(&read_taint(&p), None, &meta), Taint::Tainted, "fail closed");
        // An old key without the flag is unknown, never clean.
        let old = KeyMeta { flag_since_birth: false, ..meta };
        assert_eq!(taint_of(&Ok(TaintFile::default()), Some(false), &old), Taint::Unknown);
    }

    #[test]
    fn the_statement_is_signed_once_per_set_of_facts() {
        let k = SigningKey::generate().unwrap();
        let meta = KeyMeta { node_fp: k.fingerprint(), key_epoch: 0, key_born_at: 1_700_000_000, prev_fp: None, flag_since_birth: true };
        let h = host();
        let st = build(&facts(&k, &meta, &h, Taint::Clean, ci::Wall::JobUser), 1_700_000_100);
        st.check().unwrap();
        assert_eq!(st.distro.as_deref(), Some("ubuntu"));
        assert_eq!(st.key_exposure.mode, Some(0o600));
        assert!(!st.key_exposure.job_readable);
        assert_eq!(st.host_id, Some(statement::host_id("0123456789abcdef0123456789abcdef")));
        let mut cur = Current::default();
        assert!(cur.update(st.clone(), &k).unwrap());
        let d1 = cur.digest().unwrap();
        // Same facts later: same statement, same digest.
        let again = build(&facts(&k, &meta, &h, Taint::Clean, ci::Wall::JobUser), 1_700_009_999);
        assert!(!cur.update(again, &k).unwrap());
        assert_eq!(cur.digest().unwrap(), d1);
        // A fact changed (tainted): a new one.
        let tainted = build(&facts(&k, &meta, &h, Taint::Tainted, ci::Wall::None), 1_700_010_000);
        assert!(tainted.key_exposure.job_readable);
        assert!(cur.update(tainted, &k).unwrap());
        assert_ne!(cur.digest().unwrap(), d1);
        NodeStatement::open(cur.signed.as_ref().unwrap()).unwrap();
        // Empty host facts (before the first probe): nulls, still valid.
        let empty = HostFacts::default();
        let st = build(&facts(&k, &meta, &empty, Taint::Clean, ci::Wall::JobUser), 1_700_000_100);
        assert_eq!((st.distro, st.distro_version, st.kernel), (None, None, None));
    }

    #[test]
    fn statements_are_kept_by_digest_and_pruned() {
        let d = crate::util::temp_dir("stmt-store");
        let state = d.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let k = SigningKey::generate().unwrap();
        let meta = KeyMeta { node_fp: k.fingerprint(), key_epoch: 0, key_born_at: 1_700_000_000, prev_fp: None, flag_since_birth: true };
        let h = host();
        let st = build(&facts(&k, &meta, &h, Taint::Clean, ci::Wall::JobUser), 1_700_000_100);
        let s = st.sign(&k).unwrap();
        store(&state, &s).unwrap();
        assert_eq!(lookup(&state, &s.digest()).unwrap(), s);
        assert!(lookup(&state, &format!("sha256:{}", "0".repeat(64))).is_none());
        assert!(lookup(&state, "sha256:../../x").is_none());
        // A doctored file doesn't come back.
        let p = state.join(STATEMENTS_DIR).join(format!("{}.json", s.digest().trim_start_matches("sha256:")));
        std::fs::write(&p, serde_json::to_vec(&Signed { body: s.body.replace("\"dev\":false", "\"dev\":true"), sig: s.sig.clone() }).unwrap()).unwrap();
        assert!(lookup(&state, &s.digest()).is_none());
        for i in 0..(STATEMENTS_KEEP + 5) {
            let st = build(&facts(&k, &meta, &h, Taint::Clean, ci::Wall::JobUser), 1_700_000_200 + i as i64);
            let st = NodeStatement { kernel: Some(format!("k{i}")), ..st };
            store(&state, &st.sign(&k).unwrap()).unwrap();
        }
        let n = std::fs::read_dir(state.join(STATEMENTS_DIR)).unwrap().count();
        assert!(n <= STATEMENTS_KEEP, "{n}");
    }

    #[test]
    fn host_probes_parse() {
        assert_eq!(parse_ptrace_scope("1\n"), Some(1));
        assert_eq!(parse_ptrace_scope("3"), Some(3));
        assert_eq!(parse_ptrace_scope("4"), None);
        assert_eq!(parse_ptrace_scope("x"), None);
        let ioreg = "+-o J314sAP  <class IOPlatformExpertDevice>\n    {\n      \"IOPlatformUUID\" = \"4C4C4544-0042-3510-8051-B4C04F4E4D32\"\n    }\n";
        assert_eq!(parse_platform_uuid(ioreg).as_deref(), Some("4C4C4544-0042-3510-8051-B4C04F4E4D32"));
        assert_eq!(parse_platform_uuid("nothing"), None);
    }
}
