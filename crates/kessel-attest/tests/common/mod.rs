//! A test owner with a roster, two node keys, self-statements and signed
//! statements, written as the `trust/` and `provenance/` files a server
//! hands out. The first node key is the golden vectors' (its PKCS#8 PEM is
//! in the fixture, marked as a test key); everything is signed here.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ed25519_dalek::Signer as _;
use kessel_attest::canonical;
use kessel_attest::dsse::{pae, PAYLOAD_TYPE};
use kessel_attest::util::{b64, parse_rfc3339, sha256_hex, signed_bytes};
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey};
use serde_json::{json, Value};

/// A named change to a JSON document and the words the refusal must contain.
pub type EditCase = (&'static str, Box<dyn Fn(&mut Value)>, &'static str);

pub fn fixture() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/release/fixtures/provenance-v1.json");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).expect("provenance-v1.json is JSON")
}

pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

pub fn t(s: &str) -> i64 {
    parse_rfc3339(s).unwrap_or_else(|| panic!("bad time {s}"))
}

/// A folder under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!("kessel-attest-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Node {
    pub key: p256::ecdsa::SigningKey,
    pub spki_der: Vec<u8>,
    pub fp: String,
    pub name: String,
    pub host: String,
    pub epoch: u32,
}

impl Node {
    pub fn golden() -> Self {
        let fx = fixture();
        let key = p256::ecdsa::SigningKey::from_pkcs8_pem(s(&fx["keys"]["node"]["private_key_pem"])).unwrap();
        Self::from_key(key, "test-node-01", "host-a", 3)
    }

    pub fn other() -> Self {
        let key = p256::ecdsa::SigningKey::from_bytes(&[0x42u8; 32].into()).unwrap();
        Self::from_key(key, "test-node-02", "host-b", 0)
    }

    fn from_key(key: p256::ecdsa::SigningKey, name: &str, host: &str, epoch: u32) -> Self {
        let spki_der = key.verifying_key().to_public_key_der().unwrap().as_bytes().to_vec();
        let fp = sha256_hex(&spki_der);
        Self { key, spki_der, fp, name: name.into(), host: host.into(), epoch }
    }

    pub fn pem(&self) -> String {
        let body = b64(&self.spki_der);
        let mut out = "-----BEGIN PUBLIC KEY-----\n".to_string();
        for c in body.as_bytes().chunks(64) {
            out.push_str(std::str::from_utf8(c).unwrap());
            out.push('\n');
        }
        out.push_str("-----END PUBLIC KEY-----\n");
        out
    }

    /// `{body, sig}` of a self-statement; `edit` changes the JSON first.
    pub fn self_statement(&self, edit: impl FnOnce(&mut Value)) -> Value {
        let mut v = json!({
            "v": 1, "node_fp": self.fp, "node_spki": b64(&self.spki_der), "key_epoch": self.epoch,
            "key_born_at": t("2026-12-01T00:00:00Z"),
            "prev_fp": if self.epoch == 0 { Value::Null } else { json!("c".repeat(64)) },
            "issued_at": t("2026-12-02T00:00:00Z"), "wall": "job_user", "dev": false, "taint": "clean",
            "key_exposure": {"store": "file", "owner": "k2node", "mode": 384, "job_readable": false},
            "k2node_version": "0.46.13", "k2node_commit": Value::Null, "protocol": 10,
            "os": "linux", "arch": "x86_64", "distro": "debian", "distro_version": "13", "kernel": "6.12.0",
            "ptrace_scope": 1, "host_id": Value::Null,
        });
        edit(&mut v);
        let body = String::from_utf8(canonical::to_vec(&v).unwrap()).unwrap();
        let sig: p256::ecdsa::Signature = self.key.sign(&signed_bytes("k2-node-statement-v1", &[body.as_bytes()]));
        json!({"body": body, "sig": b64(sig.to_der().as_bytes())})
    }

    /// A DSSE envelope over `statement` (any JSON: the caller may break it).
    pub fn envelope(&self, statement: &Value) -> Value {
        let payload = canonical::to_vec(statement).unwrap();
        self.envelope_bytes(&payload)
    }

    pub fn envelope_bytes(&self, payload: &[u8]) -> Value {
        let sig: p256::ecdsa::Signature = self.key.sign(&pae(PAYLOAD_TYPE, payload));
        json!({"payloadType": PAYLOAD_TYPE, "payload": b64(payload), "signatures": [{"keyid": self.fp, "sig": b64(sig.to_der().as_bytes())}]})
    }
}

pub struct Owner {
    pub key: ed25519_dalek::SigningKey,
    pub fp: String,
}

impl Owner {
    pub fn new(seed: u8) -> Self {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let fp = sha256_hex(key.verifying_key().as_bytes());
        Self { key, fp }
    }

    pub fn roster_key(&self) -> String {
        format!("ed25519:{}", b64(self.key.verifying_key().as_bytes()))
    }

    /// A roster entry for `n`, active, vouched for by re-key.
    pub fn entry(&self, n: &Node, statement_digest: &str) -> Value {
        json!({
            "node_fp": n.fp, "node_spki": b64(&n.spki_der), "name": n.name, "key_epoch": n.epoch,
            "statement_digest": statement_digest, "status": "active", "revoked_reason": Value::Null,
            "valid_from": t("2026-12-01T00:00:00Z"), "valid_to": Value::Null,
            "taint_basis": if n.epoch == 0 { "self_clean" } else { "rekeyed" }, "attestation_digest": Value::Null,
            "host": n.host, "provider": "acme-cloud", "site": "site-1", "admin_reach": ["admin-1"], "pod": Value::Null, "point": Value::Null,
        })
    }

    /// `{body, sig}` of a roster (entries sorted here).
    pub fn roster(&self, seq: u64, issued_at: i64, prev_digest: Option<&str>, mut entries: Vec<Value>) -> Value {
        entries.sort_by(|a, b| s(&a["node_fp"]).cmp(s(&b["node_fp"])));
        let v = json!({
            "v": 1, "owner_id": "acme-owner.example.test", "owner_server_fp": "d".repeat(64), "roster_key": self.roster_key(),
            "seq": seq, "issued_at": issued_at, "prev_digest": prev_digest, "entries": entries,
        });
        self.sign_roster_json(&v)
    }

    pub fn sign_roster_json(&self, v: &Value) -> Value {
        let body = String::from_utf8(canonical::to_vec(v).unwrap()).unwrap();
        let sig = self.key.sign(&signed_bytes("k2-node-roster-v1", &[body.as_bytes()]));
        json!({"body": body, "sig": b64(&sig.to_bytes())})
    }

    pub fn builder_id(&self) -> String {
        format!("https://k2.dev/compute/builder/v1/owner/{}", self.fp)
    }
}

pub fn body_digest(signed: &Value) -> String {
    format!("sha256:{}", sha256_hex(s(&signed["body"]).as_bytes()))
}

/// Golden vector a's statement, made this owner's and this node's.
pub fn statement_for(owner: &Owner, node: &Node, self_statement: &Value) -> Value {
    let fx = fixture();
    let mut st = fx["vectors"][0]["statement"].clone();
    let rd = &mut st["predicate"]["runDetails"];
    rd["builder"]["id"] = json!(owner.builder_id());
    let digest_hex = &body_digest(self_statement)["sha256:".len()..];
    for d in rd["builder"]["builderDependencies"].as_array_mut().unwrap() {
        match s(&d["name"]) {
            "k2-node-statement" => d["digest"]["sha256"] = json!(digest_hex),
            "k2-node-key" => {
                d["digest"]["sha256"] = json!(node.fp);
                d["annotations"]["keyEpoch"] = json!(node.epoch);
                d["annotations"]["node"] = json!(node.name);
            }
            _ => {}
        }
    }
    st
}

/// The bytes of golden vector a's first subject (`dist/acme-linux-x86_64`):
/// the generator hashed exactly this text.
pub const BIN_A_BYTES: &[u8] = b"acme-linux-x86_64";
pub const SUM_A_BYTES: &[u8] = b"acme-linux-x86_64.sha256";

pub fn write(path: &Path, text: &str) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

pub fn write_json(path: &Path, v: &Value) {
    write(path, &(serde_json::to_string(v).unwrap() + "\n"));
}

/// A complete, valid set: one owner, the golden node (and the other node,
/// for twins), their self-statements, a roster issued after the build, and
/// a statement per node. Tests change one thing and expect one failure.
pub struct World {
    pub dir: TempDir,
    pub owner: Owner,
    pub node: Node,
    pub other: Node,
    pub self_st: Value,
    pub other_self_st: Value,
    pub entries: Vec<Value>,
    pub statement: Value,
    pub other_statement: Value,
}

impl World {
    pub fn new(tag: &str) -> Self {
        let owner = Owner::new(7);
        let node = Node::golden();
        let other = Node::other();
        let self_st = node.self_statement(|_| {});
        let other_self_st = other.self_statement(|_| {});
        let entries = vec![owner.entry(&node, &body_digest(&self_st)), owner.entry(&other, &body_digest(&other_self_st))];
        let statement = statement_for(&owner, &node, &self_st);
        let other_statement = statement_for(&owner, &other, &other_self_st);
        Self { dir: TempDir::new(tag), owner, node, other, self_st, other_self_st, entries, statement, other_statement }
    }

    pub fn trust(&self) -> PathBuf {
        self.dir.path().join("trust")
    }

    pub fn provenance(&self, label: &str) -> PathBuf {
        self.dir.path().join("provenance").join(format!("{label}.intoto.jsonl"))
    }

    /// Write trust/ (roster issued at `roster_at`) and provenance/L1 (and V1).
    pub fn write(&self, roster_at: i64) {
        let roster = self.owner.roster(1, roster_at, None, self.entries.clone());
        self.write_with_roster(&roster);
    }

    pub fn write_with_roster(&self, roster: &Value) {
        let tr = self.trust();
        write_json(&tr.join("rosters").join(format!("{}.json", self.owner.fp)), roster);
        write(&tr.join("owners.json"), &serde_json::to_string(&json!([{"id": "acme-owner.example.test", "rosterKeyFp": self.owner.fp, "rosterSeq": 1}])).unwrap());
        for (n, st) in [(&self.node, &self.self_st), (&self.other, &self.other_self_st)] {
            write(&tr.join("nodes").join(format!("{}.pem", n.fp)), &n.pem());
            write_json(&tr.join("statements").join(format!("{}.json", &body_digest(st)["sha256:".len()..])), st);
        }
        write_json(&self.provenance("L1"), &self.node.envelope(&self.statement));
        write_json(&self.provenance("V1"), &self.other.envelope(&self.other_statement));
    }

    pub fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

/// After the golden statement's build (finishedOn 2027-01-15T08:37:40Z).
pub const AFTER_BUILD: &str = "2027-01-16T00:00:00Z";
/// "Now" for the library tests: a day after the roster.
pub const NOW: &str = "2027-01-17T00:00:00Z";
