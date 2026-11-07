//! Test support: a world of test keys (an owner roster key, nodes, a
//! controller) and a builder for complete evidence bundles v2, signed the way
//! nodes and owners sign them, that each test bends one rule of. TEST KEYS
//! ONLY.

#![allow(dead_code)]

use std::collections::BTreeMap;

use k2_node_proto::admin::ApprovalSigner;
use k2_node_proto::ci::{Isolation, KeyStatus, KeyStore, RunFacts, SeatState, Signed, Taint, TaintBasis, Wall};
use k2_node_proto::crypto::{b64, SigningKey};
use k2_node_proto::evidence::{git_commit_sha1, EvidenceBundle, EvidenceReceipt, EvidenceSource, Role};
use k2_node_proto::frames::{argv_sha256, ExitInfo, JobState, Receipt, ReceiptLog, SignedReceipt, SrcRan};
use k2_node_proto::node_roster::{NodeRoster, OwnerAttestation, RosterEntry, SeatStatement};
use k2_node_proto::results::ArtifactInfo;
use k2_node_proto::signer_policy::SignerPolicy;
use k2_node_proto::statement::{KeyExposure, NodeStatement};
use k2_signer::forge::ReleaseSource;
use k2_signer::trust::{AcceptedKey, ControllerPin, OwnerPin, PinHow, RosterMark, Trust};
use serde_json::{json, Value};

pub const NOW: i64 = 1_800_100_000;
pub const VERSION: &str = "1.2.3";
pub const ARGV: [&str; 3] = ["bash", "scripts/ci/build.sh", "x86_64"];

pub fn sha256_hex(b: &[u8]) -> String {
    k2_node_proto::crypto::sha256_hex(b)
}

pub struct World {
    pub owner: ApprovalSigner,
    pub owner_fp: String,
    pub owner2: ApprovalSigner,
    pub owner2_fp: String,
    pub nodes: BTreeMap<String, SigningKey>,
    pub controller: SigningKey,
    pub other_controller: SigningKey,
    pub base: String,
    pub tree: String,
    pub commit_raw: Vec<u8>,
    pub commit: String,
    /// The built file `dist/acme`.
    pub artifact: Vec<u8>,
}

/// One receipt to build.
#[derive(Clone)]
pub struct Rc {
    /// Roster node name the job ran on (its key signs unless `signer` says).
    pub node: String,
    pub signer: String,
    pub build: String,
    pub role: Role,
    pub receipt: Receipt,
    pub plan: Value,
    /// Fill `plan_digest` from the plan (off: keep `receipt.plan_digest`).
    pub fix_plan_digest: bool,
    /// Fill `run.statement_digest` from the node's statement.
    pub fix_statement_digest: bool,
}

pub struct Spec {
    pub project: String,
    pub version: Option<String>,
    pub controller_fp: String,
    pub statements: BTreeMap<String, NodeStatement>,
    /// Statement signers (default: the node's own key).
    pub roster: NodeRoster,
    /// Fill each entry's `statement_digest` from its node's statement.
    pub fix_entry_digests: bool,
    pub prev_rosters: Vec<NodeRoster>,
    pub seats: Vec<SeatStatement>,
    pub attest: Vec<OwnerAttestation>,
    pub receipts: BTreeMap<String, Rc>,
    pub pairing_pins: bool,
    pub commit_object: bool,
    /// Edit this label's body after signing.
    pub tamper: Option<String>,
    /// Sign the roster and seats with this owner (default `owner`).
    pub roster_signer_is_owner2: bool,
}

impl World {
    pub fn new() -> Self {
        let owner = ApprovalSigner::from_seed(&[7u8; 32]).unwrap();
        let owner2 = ApprovalSigner::from_seed(&[8u8; 32]).unwrap();
        let mut nodes = BTreeMap::new();
        for n in ["node-a", "node-b", "node-c", "rogue"] {
            nodes.insert(n.to_string(), SigningKey::generate().unwrap());
        }
        let base = "6".repeat(40);
        let tree = "5".repeat(40);
        let commit_raw = format!(
            "tree {tree}\nparent {base}\nauthor ci <ci@example.com> 1800000650 +0000\ncommitter ci <ci@example.com> 1800000650 +0000\n\nrelease {VERSION}\n"
        )
        .into_bytes();
        let commit = git_commit_sha1(&commit_raw);
        let mut artifact = b"\x7fELF acme build ".to_vec();
        artifact.extend_from_slice(&k2_node_proto::crypto::random_bytes(256).unwrap());
        Self {
            owner_fp: owner.public().fingerprint(),
            owner2_fp: owner2.public().fingerprint(),
            owner,
            owner2,
            nodes,
            controller: SigningKey::generate().unwrap(),
            other_controller: SigningKey::generate().unwrap(),
            base,
            tree,
            commit_raw,
            commit,
            artifact,
        }
    }

    pub fn fp(&self, node: &str) -> String {
        self.nodes[node].fingerprint()
    }

    pub fn spki(&self, node: &str) -> String {
        b64(self.nodes[node].spki_der())
    }

    pub fn policy_text(&self, minisign_public: &str) -> String {
        POLICY
            .replace("@CTL@", &self.controller.fingerprint())
            .replace("@OWNER@", &self.owner_fp)
            .replace("@PUB@", minisign_public)
    }

    pub fn policy(&self) -> SignerPolicy {
        k2_signer::policy::parse(&self.policy_text(TEST_PUB), "acme-desktop").unwrap()
    }

    /// The controller paired, the owner pinned, and these node keys accepted.
    pub fn trust(&self, accepted: &[&str]) -> Trust {
        let mut t = Trust::default();
        t.controllers.insert(
            self.controller.fingerprint(),
            ControllerPin { label: "client.example".into(), spki: Some(b64(self.controller.spki_der())), how: PinHow::Sas, at: NOW - 86_400 },
        );
        let mut acc = BTreeMap::new();
        for n in accepted {
            acc.insert(self.fp(n), AcceptedKey { name: n.to_string(), seq: 1, at: NOW - 86_400 });
        }
        t.owners.insert(self.owner_fp.clone(), OwnerPin { key: self.owner.public().encode(), label: "owner".into(), at: NOW - 86_400, accepted: acc });
        t
    }

    pub fn statement(&self, node: &str) -> NodeStatement {
        NodeStatement {
            v: 1,
            node_fp: self.fp(node),
            node_spki: self.spki(node),
            key_epoch: 0,
            key_born_at: NOW - 200_000,
            prev_fp: None,
            issued_at: NOW - 100_000,
            wall: Wall::JobUser,
            dev: false,
            taint: Taint::Clean,
            key_exposure: KeyExposure { store: KeyStore::File, owner: "k2node".into(), mode: Some(0o600), job_readable: false },
            k2node_version: "0.46.10".into(),
            k2node_commit: Some("4".repeat(40)),
            protocol: 9,
            os: "linux".into(),
            arch: "x86_64".into(),
            distro: Some("ubuntu".into()),
            distro_version: Some("24.04".into()),
            kernel: Some("6.8.0".into()),
            ptrace_scope: Some(1),
            host_id: None,
        }
    }

    pub fn entry(&self, node: &str, host: &str) -> RosterEntry {
        RosterEntry {
            node_fp: self.fp(node),
            node_spki: self.spki(node),
            name: node.into(),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "0".repeat(64)),
            status: KeyStatus::Active,
            revoked_reason: None,
            valid_from: NOW - 200_000,
            valid_to: None,
            taint_basis: Some(TaintBasis::SelfClean),
            attestation_digest: None,
            host: host.into(),
            provider: Some("provider-x".into()),
            site: Some("site-1".into()),
            admin_reach: vec!["fleet-ssh".into()],
            pod: None,
            point: None,
        }
    }

    pub fn receipt(&self, node: &str, job: &str, isolation: Isolation) -> Rc {
        let argv: Vec<String> = ARGV.iter().map(|s| s.to_string()).collect();
        let receipt = Receipt {
            v: 2,
            job_id: job.into(),
            attempt: 1,
            generation: 1,
            plan_digest: String::new(),
            node_fp: self.fp(node),
            controller_fp: self.controller.fingerprint(),
            workspace_id: "ws-client".into(),
            argv_sha256: argv_sha256(&argv),
            env_names: ["BUILD_COMMIT", "BUILD_TREE", "CI", "HOME", "PATH"].iter().map(|s| s.to_string()).collect(),
            src: Some(SrcRan { commit: self.commit.clone(), tree: self.tree.clone(), dirty_sha256: None, slot: Some(0), warmth: Some("private".into()) }),
            tools: [("rustc".to_string(), "1.99.1".to_string())].into_iter().collect(),
            boot_id: format!("boot-{node}"),
            started_at: Some(NOW - 1800),
            ended_at: NOW - 600,
            state: JobState::Done,
            exit: Some(ExitInfo { code: Some(0), signal: None }),
            log: ReceiptLog { sha256: "8".repeat(64), bytes: 1234, truncated: false },
            artifacts: vec![ArtifactInfo { name: "dist/acme".into(), sha256: sha256_hex(&self.artifact), bytes: self.artifact.len() as u64 }],
            secret_env: vec!["BUILD_API_KEY".into()],
            tools_used: [("rustc".to_string(), "1.99.1".to_string())].into_iter().collect(),
            run: Some(RunFacts {
                isolation,
                uid_index: if isolation == Isolation::JobUid { Some(2) } else { None },
                wall: Wall::JobUser,
                tenant_slot: 1,
                k2node_version: "0.46.10".into(),
                k2node_commit: Some("4".repeat(40)),
                key_epoch: 0,
                statement_digest: format!("sha256:{}", "0".repeat(64)),
                roster_seq_seen: Some(1),
                lane: Some("ci-standard".into()),
            }),
        };
        let plan = json!({
            "argv": ARGV, "artifacts": ["dist/*"], "created_at": NOW - 1900, "cwd": null,
            "env": {"BUILD_COMMIT": self.commit, "BUILD_TREE": self.tree},
            "exclusive": isolation == Isolation::Exclusive, "job_id": job,
            "limits": {"cpu_millis": 4000, "disk_bytes": 107374182400u64, "log_cap_bytes": 67108864, "max_secs": 7200, "mem_bytes": 8589934592u64},
            "needs": ["rust@1.99.1"], "node_id": node, "receipt_v": 2, "requested_by": "agent",
            "secret_env": ["BUILD_API_KEY"],
            "src": {"commit": self.commit, "dirty": null, "project_key": "f".repeat(64), "remote_url": null, "slots": 1},
            "tier": "ci", "workspace_id": "ws-client", "workspace_label": "client",
        });
        Rc {
            node: node.into(),
            signer: node.into(),
            build: "linux-x86_64".into(),
            role: Role::Build,
            receipt,
            plan,
            fix_plan_digest: true,
            fix_statement_digest: true,
        }
    }

    /// The default spec: L1 on node-a (job_uid), V1 on node-b (exclusive),
    /// node-c on host-a beside node-a; one active seat; roster seq 1.
    pub fn spec(&self) -> Spec {
        let mut statements = BTreeMap::new();
        for n in ["node-a", "node-b", "node-c"] {
            statements.insert(n.to_string(), self.statement(n));
        }
        let mut entries = vec![self.entry("node-a", "host-a"), self.entry("node-b", "host-b"), self.entry("node-c", "host-a")];
        entries.sort_by(|a, b| a.node_fp.cmp(&b.node_fp));
        let roster = NodeRoster {
            v: 1,
            owner_id: "owner.example".into(),
            owner_server_fp: "e".repeat(64),
            roster_key: self.owner.public().encode(),
            seq: 1,
            issued_at: NOW - 3600,
            prev_digest: None,
            entries,
        };
        let seat = SeatStatement {
            v: 1,
            owner_id: "owner.example".into(),
            roster_key: self.owner.public().encode(),
            seat_id: "seat-1".into(),
            client_fp: self.controller.fingerprint(),
            client_label: "client.example".into(),
            lane: "ci-standard".into(),
            nodes: vec!["node-a".into(), "node-b".into(), "node-c".into()],
            roster_seq: 1,
            state: SeatState::Active,
            issued_at: NOW - 3600,
            pod: None,
            line: None,
        };
        let mut receipts = BTreeMap::new();
        receipts.insert("L1".to_string(), self.receipt("node-a", &"a".repeat(32), Isolation::JobUid));
        let mut v1 = self.receipt("node-b", &"b".repeat(32), Isolation::Exclusive);
        v1.role = Role::Verify;
        receipts.insert("V1".to_string(), v1);
        Spec {
            project: "acme-desktop".into(),
            version: Some(VERSION.into()),
            controller_fp: self.controller.fingerprint(),
            statements,
            roster,
            fix_entry_digests: true,
            prev_rosters: Vec::new(),
            seats: vec![seat],
            attest: Vec::new(),
            receipts,
            pairing_pins: true,
            commit_object: true,
            tamper: None,
            roster_signer_is_owner2: false,
        }
    }

    pub fn build(&self, mut s: Spec) -> EvidenceBundle {
        let mut st_signed = Vec::new();
        let mut st_digest = BTreeMap::new();
        for (name, st) in &s.statements {
            let key = self.nodes.get(name).unwrap_or(&self.nodes["node-a"]);
            let signed = if key.fingerprint() == st.node_fp {
                st.sign(key).unwrap()
            } else {
                Signed::sign_node(key, k2_node_proto::ci::DOMAIN_NODE_STATEMENT, st).unwrap()
            };
            st_digest.insert(name.clone(), signed.digest());
            st_signed.push(signed);
        }
        if s.fix_entry_digests {
            for e in s.roster.entries.iter_mut() {
                if let Some(d) = st_digest.get(&e.name) {
                    e.statement_digest = d.clone();
                }
            }
        }
        let mut attestations = Vec::new();
        for a in &s.attest {
            let mut a = a.clone();
            if let Some(d) = st_digest.get(s.roster.entries.iter().find(|e| e.node_fp == a.node_fp).map(|e| e.name.as_str()).unwrap_or("")) {
                a.statement_digest = d.clone();
            }
            let signed = a.sign(&self.owner).unwrap();
            for e in s.roster.entries.iter_mut() {
                if e.node_fp == a.node_fp {
                    e.taint_basis = Some(TaintBasis::OwnerAttested);
                    e.attestation_digest = Some(signed.digest());
                }
            }
            attestations.push(signed);
        }
        let owner = if s.roster_signer_is_owner2 { &self.owner2 } else { &self.owner };
        let mut rosters: Vec<Signed> = s.prev_rosters.iter().map(|r| r.sign(owner).unwrap()).collect();
        if s.roster_signer_is_owner2 {
            s.roster.roster_key = self.owner2.public().encode();
        }
        rosters.push(s.roster.sign(owner).unwrap());
        let seats: Vec<Signed> = s.seats.iter().map(|x| x.sign(&self.owner).unwrap()).collect();
        let mut receipts = BTreeMap::new();
        for (label, rc) in &s.receipts {
            let mut r = rc.receipt.clone();
            if rc.fix_statement_digest {
                if let (Some(run), Some(d)) = (r.run.as_mut(), st_digest.get(&rc.node)) {
                    run.statement_digest = d.clone();
                }
            }
            if rc.fix_plan_digest {
                r.plan_digest = format!("sha256:{}", sha256_hex(&k2_node_proto::canonical::to_vec(&rc.plan).unwrap()));
            }
            let signed = SignedReceipt::sign(&self.nodes[&rc.signer], &r).unwrap();
            let mut body = signed.body;
            if s.tamper.as_deref() == Some(label.as_str()) {
                body = body.replace(&format!("\"boot_id\":\"boot-{}\"", rc.node), "\"boot_id\":\"boot-x\"");
            }
            receipts.insert(label.clone(), EvidenceReceipt { build: rc.build.clone(), role: rc.role, body, sig: signed.sig, plan: rc.plan.clone() });
        }
        let mut pins = BTreeMap::new();
        if s.pairing_pins {
            for n in ["node-a", "node-b", "node-c"] {
                pins.insert(self.fp(n), self.spki(n));
            }
        }
        EvidenceBundle {
            v: 2,
            kind: "k2-ci-evidence".into(),
            project: s.project,
            version: s.version,
            run_id: "run-1".into(),
            controller_fp: s.controller_fp,
            created_at: NOW - 300,
            source: EvidenceSource {
                commit: self.commit.clone(),
                tree: self.tree.clone(),
                commit_object: s.commit_object.then(|| b64(&self.commit_raw)),
            },
            receipts,
            statements: st_signed,
            rotations: Vec::new(),
            attestations,
            rosters,
            seats,
            pairing_pins: pins,
        }
    }

    /// The forge's word: the tag is the approved, built commit.
    pub fn source(&self) -> ReleaseSource {
        ReleaseSource { tag_commit: self.commit.clone(), tree: self.tree.clone(), base: self.commit.clone(), rust_pin: Some("1.99.1".into()) }
    }

    pub fn attestation(&self, node: &str) -> OwnerAttestation {
        OwnerAttestation {
            v: 1,
            owner_id: "owner.example".into(),
            roster_key: self.owner.public().encode(),
            node_fp: self.fp(node),
            key_epoch: 0,
            statement_digest: format!("sha256:{}", "0".repeat(64)),
            installed_with_wall: true,
            unwalled_jobs_seen: false,
            checked: "install log and job ledger".into(),
            approved_by: "owner".into(),
            at: NOW - 7200,
        }
    }
}

pub fn mark(seq: u64, digest: &str) -> RosterMark {
    RosterMark { seq, digest: digest.into(), issued_at: NOW - 7200, seen_at: NOW - 7000, auto_accepted: BTreeMap::new() }
}

/// A minisign public line (jedisct1's) for policies that sign nothing.
pub const TEST_PUB: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";

pub const POLICY: &str = r#"version = 1
project = "acme-desktop"
controllers = ["@CTL@"]
owners = ["@OWNER@"]
workspaces = ["ws-client"]
receipt_max_hours = 24
roster_max_age_hours = 168
roster_changes = "ask"
seat_moves = "ask"
taint_basis = ["owner_attested", "rekeyed", "self_clean"]

[source]
kind = "github_tag"
repo = "acme/desktop"
tag_prefix = "v"

[build.linux-x86_64]
argv = ["bash", "scripts/ci/build.sh", "x86_64"]
needs = ["rust@{rust_pin}"]
artifacts = ["dist/*"]
env = ["BUILD_COMMIT", "BUILD_TREE"]
secrets = ["BUILD_API_KEY"]
isolation = ["job_uid", "exclusive"]
walls = ["job_user"]
dual = ["dist/acme"]
independent = ["host"]

[assets.acme]
key = "updater"
build = "linux-x86_64"
artifact = "dist/acme"
check = "artifact"

[keys.updater]
kind = "minisign"
public = "@PUB@"
"#;
