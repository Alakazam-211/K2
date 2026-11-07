//! End to end through the verbs, in a test root (no root, no `_k2sign`, no
//! keychain, no network: a fake forge): policy approval, owner pinning,
//! pairing with the SAS, roster acceptance, rehearsal, a client approval
//! session, evidence registration (never replaced, no replays), signing (one
//! sha per asset), and project `k2` on k2sign's shared state (V28).

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use common::*;
use k2_node_proto::ci::Signed;
use k2_node_proto::crypto::SigningKey;
use k2_signer::ctx::{self, Ctx};
use k2_signer::forge::{Compare, FakeForge, Forge};
use k2_signer::minisign::{self, SecretKey};
use k2_signer::pairing;
use k2_signer::trust::{PinHow, Trust};
use k2_signer::verbs;
use k2_signer::Fail;
use serde_json::Value;

struct Root {
    dir: PathBuf,
    admin: Ctx,
    signer: Ctx,
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn root() -> Root {
    let rnd = k2_node_proto::crypto::hex(&k2_node_proto::crypto::random_bytes(8).unwrap());
    let dir = std::env::temp_dir().join(format!("k2-signer-test-{rnd}"));
    let libexec = dir.join("usr/local/libexec/k2-signer");
    std::fs::create_dir_all(&libexec).unwrap();
    for n in ["k2-signer", "k2-signer-admin", "k2-signer-bin"] {
        std::fs::write(libexec.join(n), format!("test {n}\n")).unwrap();
    }
    std::fs::create_dir_all(dir.join("usr/local/etc/k2-signer")).unwrap();
    let var = dir.join("var/k2sign");
    std::fs::create_dir_all(&var).unwrap();
    std::fs::set_permissions(&var, std::fs::Permissions::from_mode(0o700)).unwrap();
    let admin = Ctx::for_test(&dir, true, NOW);
    let signer = Ctx::for_test(&dir, false, NOW);
    ctx::repin(&admin).unwrap();
    Root { dir, admin, signer }
}

fn run(c: &Ctx, verb: &str, args: &[&str], stdin: &[u8], forge: Option<&dyn Forge>) -> Result<(String, Vec<String>), Fail> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let input = stdin.to_vec();
    let read = move |lim: usize| -> k2_signer::R<Vec<u8>> {
        if input.len() > lim {
            return Err(Fail::Refused("too large".into()));
        }
        Ok(input.clone())
    };
    let (res, err) = verbs::run(c, verb, &args, &read, forge);
    res.map(|o| (String::from_utf8(o).unwrap(), err))
}

#[track_caller]
fn ok(r: Result<(String, Vec<String>), Fail>) -> String {
    match r {
        Ok((o, _)) => o,
        Err(e) => panic!("refused: {e}"),
    }
}

#[track_caller]
fn refused(r: Result<(String, Vec<String>), Fail>, text: &str) {
    match r {
        Ok((o, _)) => panic!("accepted ({o}), want a refusal containing {text:?}"),
        Err(Fail::Refused(m)) => assert!(m.contains(text), "{m:?} does not contain {text:?}"),
        Err(e) => panic!("{e:?}, want a refusal containing {text:?}"),
    }
}

fn bundle_json(b: &k2_node_proto::evidence::EvidenceBundle) -> Vec<u8> {
    serde_json::to_vec(b).unwrap()
}

/// A cheap minisign key (low scrypt limits) written as the project's key slot.
fn install_key(r: &Root, rel_dir: &str, slot: &str) -> String {
    let (_, file, public) = SecretKey::generate(b"pw-123", 32_768, 1 << 20).unwrap();
    let d = r.dir.join("var/k2sign").join(rel_dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (n, data) in [(format!("{slot}.key"), file.into_bytes()), (format!("{slot}.pw"), b"pw-123\n".to_vec())] {
        let p = d.join(n);
        std::fs::write(&p, data).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    public.line()
}

fn approve_policy(r: &Root, project: &str, text: &str) {
    let out = ok(run(&r.admin, "policy-approve", &[project], text.as_bytes(), None));
    let sha = k2_node_proto::crypto::sha256_hex(text.as_bytes());
    assert!(out.contains(&sha) && out.contains("+ version = 1"), "{out}");
    refused(run(&r.admin, "policy-approve", &[project, &"0".repeat(64)], text.as_bytes(), None), "not the");
    ok(run(&r.admin, "policy-approve", &[project, &sha], text.as_bytes(), None));
}

struct Approver(EcdsaKeyPair);

impl Approver {
    fn new(r: &Root) -> Self {
        let k = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap();
        let der = k.public_key().as_der().unwrap();
        let pem = k2_node_proto::crypto::pem_encode("PUBLIC KEY", der.as_ref());
        let p = r.dir.join("approver.pem");
        std::fs::write(&p, pem).unwrap();
        ok(run(&r.admin, "set-approver", &[p.to_str().unwrap()], b"", None));
        Self(k)
    }
    fn open(&self, r: &Root, project: &str, version: &str, base: &str, assets: &[&str]) {
        let mut args = vec![project, version, base];
        args.extend_from_slice(assets);
        // Approved an hour before the builds' clock (receipts start after it).
        let early = r.signer.clone().with_now(NOW - 3600);
        let text = ok(run(&early, "challenge", &args, b"", None));
        let c: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(c["kind"], "k2-signer-approval");
        assert_eq!(c["project"], project);
        let nonce = c["nonce"].as_str().unwrap().to_string();
        let sig = self.0.sign(&SystemRandom::new(), text.as_bytes()).unwrap();
        refused(run(&early, "approve", &[project, &nonce], b"\x30\x06\x02\x01\x01\x02\x01\x01", None), "does not verify");
        ok(run(&early, "approve", &[project, &nonce], sig.as_ref(), None));
        refused(run(&early, "approve", &[project, &nonce], sig.as_ref(), None), "already used");
    }
}

fn pair(r: &Root, controller: &SigningKey) {
    let out = ok(run(&r.admin, "pair-begin", &["release-mac"], b"", None));
    let v: Value = serde_json::from_str(&out).unwrap();
    let code = v["code"].as_str().unwrap();
    let hello: pairing::Hello = serde_json::from_value(v["hello"].clone()).unwrap();
    let (offer, sas) = pairing::server_offer(controller, "client.example", code, &hello, NOW).unwrap();
    let offer_json = serde_json::to_vec(&offer).unwrap();
    let wrong = if sas == "123456" { "654321" } else { "123456" };
    refused(run(&r.admin, "pair-accept", &[wrong], &offer_json, None), "not the one this signer computed");
    let done = ok(run(&r.admin, "pair-accept", &[&sas], &offer_json, None));
    let done: Signed = serde_json::from_str(&done).unwrap();
    assert_eq!(pairing::server_finish(&offer, &hello, &done).unwrap(), v["hello"]["signer_fp"].as_str().unwrap());
    let t = Trust::load(&r.admin).unwrap();
    assert_eq!(t.controllers[&controller.fingerprint()].how, PinHow::Sas);
    refused(run(&r.admin, "pair-accept", &[&sas], &offer_json, None), "no pairing started");
}

fn forge_for(w: &World, version: &str) -> FakeForge {
    let mut f = FakeForge::default();
    f.tags.insert(format!("v{version}"), w.commit.clone());
    f.commits.insert(w.commit.clone(), (w.tree.clone(), vec![w.base.clone()]));
    f.files.insert(("rust-toolchain.toml".into(), w.commit.clone()), b"[toolchain]\nchannel = \"1.99.1\"\n".to_vec());
    f
}

fn audit_lines(p: &Path) -> String {
    std::fs::read_to_string(p.join("signatures.tsv")).unwrap_or_default()
}

#[test]
fn a_client_release_end_to_end() {
    let r = root();
    let w = World::new();
    let pubk = install_key(&r, "k2-signer/projects/acme-desktop/keys", "updater");
    let b = w.build(w.spec());
    let bj = bundle_json(&b);
    refused(run(&r.signer, "rehearse", &["acme-desktop"], &bj, None), "no policy for project acme-desktop");
    approve_policy(&r, "acme-desktop", &w.policy_text(&pubk));
    // A file the admin didn't pin breaks the pins (as k2sign's repin rule).
    std::fs::write(r.dir.join("usr/local/etc/k2-signer/projects/acme-desktop.toml"), w.policy_text(TEST_PUB)).unwrap();
    refused(run(&r.signer, "rehearse", &["acme-desktop"], &bj, None), "pins mismatch: etc/projects/acme-desktop.toml");
    std::fs::write(r.dir.join("usr/local/etc/k2-signer/projects/acme-desktop.toml"), w.policy_text(&pubk)).unwrap();
    ok(run(&r.signer, "pins", &[], b"", None));

    refused(run(&r.signer, "rehearse", &["acme-desktop"], &bj, None), "not paired with this signer");
    pair(&r, &w.controller);
    refused(run(&r.signer, "rehearse", &["acme-desktop"], &bj, None), "is not pinned at this signer");
    let key = w.owner.public().encode();
    refused(run(&r.admin, "trust-owner", &[&"0".repeat(64), &key, "owner"], b"", None), "not the");
    ok(run(&r.admin, "trust-owner", &[&w.owner_fp, &key, "owner"], b"", None));
    // S1: every new node key is a person's decision.
    refused(run(&r.signer, "rehearse", &["acme-desktop"], &bj, None), "roster_change_needs_person");
    let show = ok(run(&r.admin, "roster-accept", &[&w.owner_fp], &bj, None));
    assert!(show.contains("+ NEW") && show.contains("node-a") && show.contains("3 new key(s)"), "{show}");
    let digest = b.rosters[0].digest();
    refused(run(&r.admin, "roster-accept", &[&w.owner_fp, &format!("sha256:{}", "0".repeat(64))], &bj, None), "nothing accepted");
    ok(run(&r.admin, "roster-accept", &[&w.owner_fp, &digest], &bj, None));
    assert!(ok(run(&r.admin, "roster-accept", &[&w.owner_fp], &bj, None)).contains("Nothing new to accept"));

    // Rehearse: passes, registers and signs nothing.
    let out = ok(run(&r.signer, "rehearse", &["acme-desktop"], &bj, None));
    assert!(out.contains("\"rehearsal\":true"), "{out}");
    assert!(!r.dir.join("var/k2sign/k2-signer/evidence").exists());
    assert!(!r.dir.join("var/k2sign/k2-signer/rosters").exists());
    assert!(audit_lines(&r.dir.join("var/k2sign/k2-signer/audit/acme-desktop/rehearsal")).contains("\tREHEARSAL\t"));

    // No session, no evidence.
    let f = forge_for(&w, VERSION);
    refused(run(&r.signer, "evidence", &["acme-desktop", VERSION], &bj, Some(&f)), "no approved signing session");
    let approver = Approver::new(&r);
    approver.open(&r, "acme-desktop", VERSION, &w.commit, &["acme"]);
    refused(run(&r.signer, "sign", &["acme-desktop", "updater", "acme", VERSION, "raw"], &w.artifact, Some(&f)), "no registered evidence");
    let out = ok(run(&r.signer, "evidence", &["acme-desktop", VERSION], &bj, Some(&f)));
    assert!(out.contains("\"again\":false"), "{out}");
    assert!(r.dir.join(format!("var/k2sign/k2-signer/rosters/{}.json", w.owner_fp)).exists());
    let out = ok(run(&r.signer, "evidence", &["acme-desktop", VERSION], &bj, Some(&f)));
    assert!(out.contains("\"again\":true"), "{out}");
    // L3: other evidence for a registered version is refused.
    let other = bundle_json(&w.build(w.spec()));
    refused(run(&r.signer, "evidence", &["acme-desktop", VERSION], &other, Some(&f)), "never replaced");

    // Sign: the file must be L1's signed artifact.
    let mut other_bytes = w.artifact.clone();
    other_bytes.push(0);
    // V5's verdict: every sign check, no signature, nothing registered.
    let v: Value = serde_json::from_str(&ok(run(&r.signer, "verdict", &["acme-desktop", "acme", VERSION], &w.artifact, Some(&f)))).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["jobs"]["V1"]["node"], "node-b");
    refused(run(&r.signer, "verdict", &["acme-desktop", "acme", VERSION], &other_bytes, Some(&f)), "are not L1's signed artifact");
    assert!(!r.dir.join("var/k2sign/k2-signer/projects/acme-desktop/assets").exists());
    refused(run(&r.signer, "sign", &["acme-desktop", "updater", "acme", VERSION, "raw"], &other_bytes, Some(&f)), "are not L1's signed artifact");
    let sig = ok(run(&r.signer, "sign", &["acme-desktop", "updater", "acme", VERSION, "raw"], &w.artifact, Some(&f)));
    let tc = minisign::verify(&pubk, &w.artifact, &sig).unwrap();
    assert!(tc.contains("file:acme") && tc.contains(&format!("version:{VERSION}")), "{tc}");
    let b64 = ok(run(&r.signer, "sign", &["acme-desktop", "updater", "acme", VERSION, "b64"], &w.artifact, Some(&f)));
    let text = String::from_utf8(k2_node_proto::crypto::unb64(&b64).unwrap()).unwrap();
    minisign::verify(&pubk, &w.artifact, &text).unwrap();
    // One sha per asset per version.
    refused(run(&r.signer, "sign", &["acme-desktop", "updater", "acme", VERSION, "raw"], &other_bytes, Some(&f)), "REFUSED-SWAP");
    refused(run(&r.signer, "sign", &["acme-desktop", "u1", "acme", VERSION, "raw"], &w.artifact, Some(&f)), "is signed with key updater");
    refused(run(&r.signer, "sign", &["acme-desktop", "updater", "other", VERSION, "raw"], &w.artifact, Some(&f)), "not signable");
    let st: Value = serde_json::from_str(&ok(run(&r.signer, "status", &["acme-desktop", VERSION], b"", None))).unwrap();
    assert_eq!(st["session"]["count"], 1);
    assert_eq!(st["signed"][0], format!("acme\t{}", sha256_hex(&w.artifact)));
    let a = audit_lines(&r.dir.join(format!("var/k2sign/k2-signer/audit/acme-desktop/{VERSION}")));
    assert!(a.contains("\tEVIDENCE\t") && a.contains("\tEVIDENCE-AGAIN\t") && a.contains("\tSIGNED\t") && a.contains("\tREFUSED-EVIDENCE\t"), "{a}");

    // A replay: the same receipts offered for the next version.
    approver.open(&r, "acme-desktop", "1.2.4", &w.commit, &["acme"]);
    let mut s = w.spec();
    s.version = Some("1.2.4".into());
    let f2 = forge_for(&w, "1.2.4");
    refused(
        run(&r.signer, "evidence", &["acme-desktop", "1.2.4"], &bundle_json(&w.build(s)), Some(&f2)),
        "a replayed receipt: it was registered for acme-desktop/1.2.3",
    );
    // A version not newer than the last approved one.
    refused(run(&r.signer, "challenge", &["acme-desktop", "1.2.0", &w.commit, "acme"], b"", None), "is not newer than 1.2.4");

    // Root clears the evidence: other evidence registers, but the bytes signed stay the only ones signed.
    ok(run(&r.admin, "clear-evidence", &["acme-desktop", VERSION], b"", None));
    ok(run(&r.signer, "evidence", &["acme-desktop", VERSION], &other, Some(&f)));
    refused(run(&r.signer, "sign", &["acme-desktop", "updater", "acme", VERSION, "raw"], &other_bytes, Some(&f)), "REFUSED-SWAP");
}

#[test]
fn project_k2_shares_k2sign_state() {
    let r = root();
    let mut w = World::new();
    let version = "0.47.1";
    // A K2-shaped daemon (the k2sign-s3b.sh mkset shape) as the build's file.
    let ptyd = {
        let mut h = vec![0u8; 64];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[5] = 1;
        h[18..20].copy_from_slice(&62u16.to_le_bytes());
        h[0x28..0x30].copy_from_slice(&(8192u64 - 128).to_le_bytes());
        h[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
        h[0x3C..0x3E].copy_from_slice(&2u16.to_le_bytes());
        h.resize(8192, 0x5A);
        h
    };
    let mut daemon = ptyd[..64].to_vec();
    daemon.resize(4096, 0x11);
    daemon.extend_from_slice(&ptyd);
    daemon.extend_from_slice(sha256_hex(&ptyd).as_bytes());
    daemon.extend_from_slice(format!("\0K2_DAEMON_BUILD v={version} os=linux arch=x86_64;").as_bytes());
    w.artifact = daemon.clone();
    let u1 = install_key(&r, "keys", "u1");
    let u2 = install_key(&r, "keys", "u2");
    let text = w
        .policy_text(&u2)
        .replace("project = \"acme-desktop\"", "project = \"k2\"")
        .replace("repo = \"acme/desktop\"", "repo = \"Alakazam-211/K2\"")
        .replace("dual = [\"dist/acme\"]", "dual = [\"dist/k2-daemon-linux-x86_64\"]")
        .replace("[assets.acme]\nkey = \"updater\"", "[assets.k2-daemon-linux-x86_64]\nkey = \"u2\"")
        .replace("artifact = \"dist/acme\"", "artifact = \"dist/k2-daemon-linux-x86_64\"")
        .replace("[keys.updater]", &format!("[keys.u1]\nkind = \"minisign\"\npublic = \"{u1}\"\n\n[keys.u2]"));
    approve_policy(&r, "k2", &text);
    pair(&r, &w.controller);
    ok(run(&r.admin, "trust-owner", &[&w.owner_fp, &w.owner.public().encode()], b"", None));
    let mut s = w.spec();
    s.project = "k2".into();
    s.version = Some(version.into());
    for rc in s.receipts.values_mut() {
        rc.receipt.artifacts[0].name = "dist/k2-daemon-linux-x86_64".into();
    }
    let b = w.build(s);
    let bj = bundle_json(&b);
    ok(run(&r.admin, "roster-accept", &[&w.owner_fp, &b.rosters[0].digest()], &bj, None));
    // k2sign's installed policy names the release commit's files; its session opened the version.
    let k2etc = r.dir.join("usr/local/etc/k2sign");
    std::fs::create_dir_all(&k2etc).unwrap();
    std::fs::write(k2etc.join("policy"), "approval = on\nrelease_commit_files = package.json Cargo.lock WHATS_NEW.md\n").unwrap();
    let state = r.dir.join("var/k2sign/state");
    std::fs::create_dir_all(state.join("sessions")).unwrap();
    let sess = serde_json::json!({"version": version, "base_commit": w.base, "assets": ["k2-daemon-linux-x86_64"], "windows": false,
        "max_signatures": 6, "approved": NOW - 3600, "opened": NOW - 3600, "expires": NOW + 3600, "nonce": "0".repeat(32),
        "reapproval": false, "by": "touchid", "count": 0, "signed": []});
    std::fs::write(state.join("sessions").join(format!("{version}.json")), sess.to_string()).unwrap();
    // GitHub: v0.47.1 = T, the release bump on base B; the jobs built the preview commit (same tree, parent B).
    let t = "7".repeat(40);
    let mut f = FakeForge::default();
    f.tags.insert(format!("v{version}"), t.clone());
    f.commits.insert(t.clone(), (w.tree.clone(), vec![w.base.clone()]));
    f.compares.insert((w.base.clone(), t.clone()), Compare { status: "ahead".into(), ahead_by: 1, files: vec!["package.json".into()] });
    f.compares.insert((w.base.clone(), "main".into()), Compare { status: "ahead".into(), ahead_by: 3, files: vec![] });
    f.files.insert(("rust-toolchain.toml".into(), t.clone()), b"[toolchain]\nchannel = \"1.99.1\"\n".to_vec());
    // k2-signer refuses challenges for k2 (k2sign's), and k2sign's v1 evidence for the version.
    refused(run(&r.signer, "challenge", &["k2", version, &w.base, "k2-daemon-linux-x86_64"], b"", None), "k2sign's");
    std::fs::create_dir_all(state.join("compute")).unwrap();
    std::fs::write(state.join("compute").join(format!("{version}.json")), "{}").unwrap();
    refused(run(&r.signer, "evidence", &["k2", version], &bj, Some(&f)), "already has k2sign's S3b compute evidence");
    std::fs::remove_file(state.join("compute").join(format!("{version}.json"))).unwrap();
    // A release commit that changes another file is not base+bump.
    let mut bad = f.clone();
    bad.compares.insert((w.base.clone(), t.clone()), Compare { status: "ahead".into(), ahead_by: 1, files: vec!["src/main.rs".into()] });
    refused(run(&r.signer, "evidence", &["k2", version], &bj, Some(&bad)), "release commit changes non-release files: src/main.rs");
    ok(run(&r.signer, "evidence", &["k2", version], &bj, Some(&f)));
    let receipts = std::fs::read_to_string(state.join("compute/receipts.tsv")).unwrap();
    assert!(receipts.lines().all(|l| l.ends_with(&format!("\t{version}"))) && receipts.lines().count() == 2, "{receipts}");
    // U2, and U1 (the bridge until SS6), both on the shared one-sha registry.
    let sig2 = ok(run(&r.signer, "sign", &["k2", "u2", "k2-daemon-linux-x86_64", version, "raw"], &daemon, Some(&f)));
    assert!(minisign::verify(&u2, &daemon, &sig2).unwrap().ends_with(&format!("version:{version}")));
    let sig1 = ok(run(&r.signer, "sign", &["k2", "u1", "k2-daemon-linux-x86_64", version, "raw"], &daemon, Some(&f)));
    assert!(sig1.starts_with("untrusted comment: signature from tauri secret key"));
    minisign::verify(&u1, &daemon, &sig1).unwrap();
    assert_eq!(std::fs::read_to_string(state.join(format!("assets/{version}.tsv"))).unwrap(), format!("k2-daemon-linux-x86_64\t{}\n", sha256_hex(&daemon)));
    let s: Value = serde_json::from_slice(&std::fs::read(state.join(format!("sessions/{version}.json"))).unwrap()).unwrap();
    assert_eq!(s["count"], 2);
    assert!(audit_lines(&r.dir.join(format!("var/k2sign/audit/{version}"))).contains("\tSIGNED\tu2\tk2-daemon-linux-x86_64\t"));
    // k2sign signed other bytes first for some asset: the shared registry refuses ours.
    std::fs::write(state.join("assets/0.47.2.tsv"), format!("k2-daemon-linux-x86_64\t{}\n", "0".repeat(64))).unwrap();
    let mut s2 = sess.clone();
    s2["version"] = "0.47.2".into();
    std::fs::write(state.join("sessions/0.47.2.json"), s2.to_string()).unwrap();
    refused(run(&r.signer, "sign", &["k2", "u2", "k2-daemon-linux-x86_64", "0.47.2", "raw"], &daemon, Some(&f)), "REFUSED-SWAP");
}

#[test]
fn adopt_k2sign_and_admin_refusals() {
    let r = root();
    let w = World::new();
    let k2etc = r.dir.join("usr/local/etc/k2sign");
    std::fs::create_dir_all(&k2etc).unwrap();
    std::fs::write(
        k2etc.join("compute-pins"),
        format!("# pins\ncontroller {}\nnode k2n7 {} {}\n", w.controller.fingerprint(), w.fp("node-a"), w.spki("node-a")),
    )
    .unwrap();
    let k = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap();
    std::fs::write(k2etc.join("approver.pem"), k2_node_proto::crypto::pem_encode("PUBLIC KEY", k.public_key().as_der().unwrap().as_ref())).unwrap();
    let out = ok(run(&r.admin, "adopt-k2sign", &[], b"", None));
    assert!(out.contains("adopted from k2sign") && out.contains("not adopted") && out.contains("approver.pem: copied"), "{out}");
    let t = Trust::load(&r.admin).unwrap();
    assert_eq!(t.controllers[&w.controller.fingerprint()].how, PinHow::K2signAdopted);
    ctx::check_pins(&r.signer).unwrap();
    // Client keys: k2's are k2sign's; a second key in one slot is refused.
    refused(run(&r.admin, "generate-key", &["k2", "u2"], b"", None), "k2sign's");
    refused(run(&r.admin, "set-gh-token", &["k2"], b"ghp_aaaaaaaaaaaaaaaaaaaaaaaa", None), "k2sign's token");
    let (_, file, public) = SecretKey::generate(b"pw", 32_768, 1 << 20).unwrap();
    let kp = r.dir.join("acme.key");
    std::fs::write(&kp, file).unwrap();
    refused(run(&r.admin, "import-key", &["acme", "updater", kp.to_str().unwrap()], b"wrong\n", None), "wrong password");
    let line = ok(run(&r.admin, "import-key", &["acme", "updater", kp.to_str().unwrap()], b"pw\n", None));
    assert_eq!(line.trim(), public.line());
    refused(run(&r.admin, "import-key", &["acme", "updater", kp.to_str().unwrap()], b"pw\n", None), "already exists");
    // Paths are absolute; usage errors exit 2.
    assert!(matches!(run(&r.admin, "set-approver", &["relative.pem"], b"", None), Err(Fail::Usage(_))));
    assert!(matches!(run(&r.signer, "nope", &[], b"", None), Err(Fail::Usage(_))));
    assert!(matches!(run(&r.signer, "sign", &["acme"], b"", None), Err(Fail::Usage(_))));
    // An admin verb is not a signer verb.
    assert!(matches!(run(&r.signer, "trust-owner", &[], b"", None), Err(Fail::Usage(_))));
}
