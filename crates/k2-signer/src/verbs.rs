//! The command surface. Run only through the root-owned wrappers
//! (`scripts/k2sign/k2-signer`, `k2-signer-admin`): `env -i`, a fixed PATH
//! and HOME, umask 077. File bytes and documents come in on **stdin**;
//! signatures and JSON go out on stdout; no verb run as the signing user
//! opens a path the caller names.
//!
//! ```text
//! Signer verbs (sudo -n -u _k2sign /usr/local/libexec/k2-signer/k2-signer <verb> …):
//!   rehearse <project>                          (evidence bundle v2 on stdin) → JSON; registers and signs nothing
//!   evidence <project> <version>                (evidence bundle v2 on stdin) → JSON; registers it (never replaced)
//!   challenge <project> <version> <base> <asset>… [--reapprove]                → challenge JSON (client projects)
//!   approve <project> <nonce>                   (DER approval signature on stdin)
//!   sign <project> <slot> <asset> <version> raw|b64   (file bytes on stdin)  → minisign signature
//!   verdict <project> <asset> <version>         (file bytes on stdin) → JSON: every sign check, no signature (V5)
//!   status <project> <version>                                              → JSON
//!   pins                                                                     → JSON (what this signer trusts)
//!   identity                                                                 → JSON (the signer's pairing key)
//! Admin verbs (sudo /usr/local/libexec/k2-signer/k2-signer-admin <verb> …):
//!   policy-approve <project> [<sha256>]         (policy TOML on stdin: no sha = show the diff and its sha256)
//!   trust-owner <fp> <ed25519:key> [<label>]    untrust-owner <fp>
//!   roster-accept <owner fp> [<digest>]         (evidence bundle or signed roster on stdin: no digest = show the changes)
//!   pair-begin [<label>]   pair-reveal (the server's offer on stdin)   pair-accept <sas>   unpair <fp>
//!   pairing-pins <project> required|optional
//!   set-approver </abs/approver.pem>            adopt-k2sign
//!   generate-key <project> <slot>               import-key <project> <slot> </abs/key> (password on stdin)
//!   set-gh-token <project>                      (token on stdin)
//!   clear-evidence <project> <version>          repin
//! ```
//! Exit codes: 0 ok, 2 usage, 3 refused, 4 internal.

use std::cell::RefCell;
use std::collections::BTreeMap;

use k2_node_proto::admin::ApprovalKey;
use k2_node_proto::ci::Signed;
use k2_node_proto::crypto::SigningKey;
use k2_node_proto::evidence::{EvidenceBundle, Role, MAX_EVIDENCE_BYTES};
use k2_node_proto::node_roster::NodeRoster;
use k2_node_proto::signer_policy::{SignerPolicy, SourceKind};
use serde_json::{json, Value};

use crate::check::{self, Input, Summary, Target};
use crate::ctx::{self, audit, AuditLine, Ctx, Locked};
use crate::forge::{self, Forge, GitHub};
use crate::minisign::{self, SecretKey};
use crate::pairing;
use crate::policy;
use crate::registry;
use crate::session::{self, Session};
use crate::trust::{self, AcceptedKey, ControllerPin, OwnerPin, PinHow, Trust};
use crate::{refused, usage, Fail, R};

/// The biggest file `sign` takes (k2sign's `max_stdin_mb`).
pub const MAX_SIGN_BYTES: usize = 1 << 30;

/// What a verb runs with: the context, stdin (read on demand, with a
/// limit), an optional forge (tests), and stderr lines.
pub struct Env<'a> {
    pub ctx: &'a Ctx,
    pub stdin: &'a dyn Fn(usize) -> R<Vec<u8>>,
    pub forge: Option<&'a dyn Forge>,
    pub err: RefCell<Vec<String>>,
}

impl Env<'_> {
    fn say(&self, s: impl Into<String>) {
        self.err.borrow_mut().push(s.into());
    }
}

type Verb = fn(&Env, &[String]) -> R<Vec<u8>>;

pub fn signer_verbs() -> BTreeMap<&'static str, Verb> {
    let mut m: BTreeMap<&'static str, Verb> = BTreeMap::new();
    m.insert("rehearse", verb_rehearse);
    m.insert("evidence", verb_evidence);
    m.insert("challenge", verb_challenge);
    m.insert("approve", verb_approve);
    m.insert("sign", verb_sign);
    m.insert("verdict", verb_verdict);
    m.insert("status", verb_status);
    m.insert("pins", verb_pins);
    m.insert("identity", verb_identity);
    m
}

pub fn admin_verbs() -> BTreeMap<&'static str, Verb> {
    let mut m: BTreeMap<&'static str, Verb> = BTreeMap::new();
    m.insert("policy-approve", admin_policy_approve);
    m.insert("trust-owner", admin_trust_owner);
    m.insert("untrust-owner", admin_untrust_owner);
    m.insert("roster-accept", admin_roster_accept);
    m.insert("pair-begin", admin_pair_begin);
    m.insert("pair-reveal", admin_pair_reveal);
    m.insert("pair-accept", admin_pair_accept);
    m.insert("pairing-pins", admin_pairing_pins);
    m.insert("unpair", admin_unpair);
    m.insert("set-approver", admin_set_approver);
    m.insert("adopt-k2sign", admin_adopt_k2sign);
    m.insert("generate-key", admin_generate_key);
    m.insert("import-key", admin_import_key);
    m.insert("set-gh-token", admin_set_gh_token);
    m.insert("clear-evidence", admin_clear_evidence);
    m.insert("repin", admin_repin);
    m
}

/// Run one verb: `argv` = `[_, "signer"|"admin", verb, args…]`.
pub fn run(ctx: &Ctx, verb: &str, args: &[String], stdin: &dyn Fn(usize) -> R<Vec<u8>>, forge: Option<&dyn Forge>) -> (R<Vec<u8>>, Vec<String>) {
    let verbs = if ctx.admin { admin_verbs() } else { signer_verbs() };
    let env = Env { ctx, stdin, forge, err: RefCell::new(Vec::new()) };
    let res = match verbs.get(verb) {
        Some(f) => f(&env, args),
        None => usage(format!("verbs: {}", verbs.keys().copied().collect::<Vec<_>>().join(" "))),
    };
    (res, env.err.into_inner())
}

/// The binary's entry point; returns the exit code.
pub fn main(argv: Vec<String>) -> i32 {
    use std::io::Write;
    let say = |s: &str| {
        let _ = writeln!(std::io::stderr(), "k2-signer: {s}");
    };
    if argv.len() < 3 || !matches!(argv[1].as_str(), "signer" | "admin") {
        say("usage: run through /usr/local/libexec/k2-signer/k2-signer or k2-signer-admin");
        return 2;
    }
    let admin = argv[1] == "admin";
    let outcome = std::panic::catch_unwind(|| {
        let ctx = Ctx::from_env(admin)?;
        let stdin = |limit: usize| ctx::read_stdin(limit);
        let (res, err) = run(&ctx, &argv[2], &argv[3..], &stdin, None);
        for l in err {
            say(&l);
        }
        res
    });
    match outcome {
        Ok(Ok(out)) => {
            let mut o = std::io::stdout();
            if o.write_all(&out).and_then(|_| o.flush()).is_err() {
                return 4;
            }
            0
        }
        Ok(Err(Fail::Usage(m))) => {
            say(&format!("usage: {m}"));
            2
        }
        Ok(Err(Fail::Refused(m))) => {
            let _ = writeln!(std::io::stderr(), "\x1b[1;31mk2-signer: refused: {m}\x1b[0m");
            3
        }
        Ok(Err(Fail::Internal(m))) => {
            say(&format!("internal error: {m}"));
            4
        }
        Err(_) => {
            say("internal error: a panic");
            4
        }
    }
}

fn json_out(v: &Value) -> R<Vec<u8>> {
    let mut s = serde_json::to_string(v).map_err(|e| Fail::Internal(e.to_string()))?;
    s.push('\n');
    Ok(s.into_bytes())
}

fn need(args: &[String], n: usize, u: &str) -> R<()> {
    if args.len() != n {
        return usage(u.to_string());
    }
    Ok(())
}

fn check_slot(slot: &str) -> R<()> {
    if !k2_node_proto::ci::is_name(slot) {
        return usage(format!("bad key slot {slot:?}"));
    }
    Ok(())
}

fn check_asset_name(a: &str) -> R<()> {
    let ok = !a.is_empty()
        && a.len() <= 128
        && a.as_bytes()[0].is_ascii_alphanumeric()
        && a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if !ok {
        return usage(format!("bad asset name {a:?}"));
    }
    Ok(())
}

// ── evidence ───────────────────────────────────────────────────────────

/// project `k2`: k2sign's `release_commit_files` (its root-owned, pinned
/// policy): what release.sh's version bump may change on top of the base.
fn release_files(ctx: &Ctx, project: &str) -> R<Option<Vec<String>>> {
    if project != "k2" {
        return Ok(None);
    }
    match k2sign_policy_value(ctx, "release_commit_files")? {
        Some(v) => Ok(Some(v.split_whitespace().map(String::from).collect())),
        None => refused("k2sign's policy has no release_commit_files"),
    }
}

/// One `key = value` of k2sign's installed (root-owned, pinned) policy.
fn k2sign_policy_value(ctx: &Ctx, key: &str) -> R<Option<String>> {
    let p = ctx.k2sign_etc().join("policy");
    let raw = ctx::read_trusted(ctx, &p, 64 * 1024)?;
    for line in String::from_utf8_lossy(&raw).lines() {
        let l = line.split('#').next().unwrap_or_default().trim();
        if let Some((k, v)) = l.split_once('=') {
            if k.trim() == key {
                return Ok(Some(v.trim().to_string()));
            }
        }
    }
    Ok(None)
}

/// M3: for project `k2`, k2-signer signs only what k2sign itself would sign
/// from compute (its `compute_assets`), and a Windows setup only when the
/// session approved Windows.
fn k2_asset_rules(ctx: &Ctx, project: &str, asset: &str, version: &str, sess: &Session) -> R<()> {
    if project != "k2" {
        return Ok(());
    }
    let compute = k2sign_policy_value(ctx, "compute_assets")?.unwrap_or_default();
    if !compute.split_whitespace().any(|a| a == asset) {
        return refused(format!("{asset}: project k2's k2-signer signs only k2sign's compute_assets ({compute}); the rest is k2sign's"));
    }
    let windows = k2sign_policy_value(ctx, "windows_assets")?.unwrap_or_default();
    if windows.split_whitespace().any(|a| a.replace("{version}", version) == asset) && !sess.windows {
        return refused(format!("Windows setup not approved for {version}"));
    }
    Ok(())
}

/// Check a bundle for a release under its session (the forge's binding).
fn check_release(env: &Env, pol: &SignerPolicy, bundle: &EvidenceBundle, sess: &Session, version: &str) -> R<Summary> {
    let ctx = env.ctx;
    let trust = Trust::load(ctx)?;
    let marks = trust::load_marks(ctx)?;
    let source = match pol.source.kind {
        SourceKind::GithubTag => {
            let gh;
            let f: &dyn Forge = match env.forge {
                Some(f) => f,
                None => {
                    gh = GitHub::for_project(ctx, &pol.project)?;
                    &gh
                }
            };
            let needs_pin = pol.build.values().any(|b| b.needs.iter().any(|n| n.contains("{rust_pin}")));
            let rf = release_files(ctx, &pol.project)?;
            Some(forge::resolve_github(f, pol, version, &sess.base_commit, rf.as_deref(), needs_pin)?)
        }
        SourceKind::ApprovalCommit => None,
    };
    check::check(&Input {
        policy: pol,
        trust: &trust,
        marks: &marks,
        bundle,
        now: ctx.now(),
        target: Target::Release { version, base: &sess.base_commit, opened: Some(sess.opened()), source: source.as_ref() },
    })
}

fn read_bundle(env: &Env) -> R<(Vec<u8>, String)> {
    let raw = (env.stdin)(MAX_EVIDENCE_BYTES)?;
    if raw.is_empty() {
        return refused("no evidence bundle on stdin");
    }
    let sha = ctx::sha256_hex(&raw);
    Ok((raw, sha))
}

fn verb_rehearse(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 1, "rehearse <project>   (an evidence bundle v2 on stdin: k2 ci evidence of a shadow run)")?;
    let ctx = env.ctx;
    let project = &args[0];
    policy::check_project_name(project)?;
    ctx::check_pins(ctx)?;
    let pol = policy::load(ctx, project)?;
    let _l = Locked::take(ctx)?;
    let (raw, sha) = read_bundle(env)?;
    let dir = registry::rehearsal_audit_dir(ctx, project);
    let res = (|| -> R<Summary> {
        let bundle = EvidenceBundle::parse(&raw).map_err(Fail::Refused)?;
        let trust = Trust::load(ctx)?;
        let marks = trust::load_marks(ctx)?;
        check::check(&Input { policy: &pol, trust: &trust, marks: &marks, bundle: &bundle, now: ctx.now(), target: Target::Rehearse })
    })();
    let summary = match res {
        Ok(s) => s,
        Err(e) => {
            audit(ctx, &dir, &AuditLine::event("REFUSED-REHEARSAL", &format!("bundle {}; {e}", &sha[..16])))?;
            return Err(e);
        }
    };
    let jobs = summary.jobs.iter().map(|(k, j)| format!("{k} job {} on {}", j.job, j.node)).collect::<Vec<_>>().join("; ");
    audit(ctx, &dir, &AuditLine::event("REHEARSAL", &format!("bundle {}; tree {}; {jobs}", &sha[..16], summary.tree)))?;
    env.say(format!(
        "rehearsal passed for {project}: {} verify with the pinned owners and controller, and every dual-built file matches (nothing was registered or signed)",
        summary.jobs.iter().map(|(k, j)| format!("{k} on {}", j.node)).collect::<Vec<_>>().join(", ")
    ));
    json_out(&json!({"ok": true, "rehearsal": true, "bundle_sha256": sha, "summary": summary}))
}

fn verb_evidence(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 2, "evidence <project> <version>   (the evidence bundle v2 on stdin)")?;
    let ctx = env.ctx;
    let (project, version) = (&args[0], &args[1]);
    policy::check_project_name(project)?;
    session::check_version(version)?;
    ctx::check_pins(ctx)?;
    ctx::check_var(ctx)?;
    let pol = policy::load(ctx, project)?;
    let _l = Locked::take(ctx)?;
    let dir = registry::audit_dir(ctx, project, version);
    let sess = session::load_valid(ctx, project, version)?;
    let (raw, sha) = read_bundle(env)?;
    let res = (|| -> R<(bool, Summary)> {
        let again = registry::evidence_again(ctx, project, version, &sha)?;
        let bundle = EvidenceBundle::parse(&raw).map_err(Fail::Refused)?;
        let summary = check_release(env, &pol, &bundle, &sess, version)?;
        {
            let rids: Vec<(&str, &str)> = summary.jobs.iter().map(|(k, j)| (k.as_str(), j.receipt.as_str())).collect();
            registry::check_replays(ctx, project, version, &rids)?;
        }
        Ok((again, summary))
    })();
    let (again, summary) = match res {
        Ok(x) => x,
        Err(e) => {
            audit(ctx, &dir, &AuditLine::event("REFUSED-EVIDENCE", &e.to_string()))?;
            return Err(e);
        }
    };
    let rids: Vec<&str> = summary.jobs.values().map(|j| j.receipt.as_str()).collect();
    registry::record_receipts(ctx, project, version, &rids)?;
    for (fp, m) in &summary.new_marks {
        trust::save_mark(ctx, fp, m)?;
    }
    let sum_v = serde_json::to_value(&summary).map_err(|e| Fail::Internal(e.to_string()))?;
    if !again {
        let text = String::from_utf8(raw).map_err(|_| Fail::Refused("the bundle is not UTF-8".into()))?;
        registry::save_evidence(
            ctx,
            &registry::Registered {
                project: project.clone(),
                version: version.clone(),
                bundle_sha256: sha.clone(),
                registered: ctx.now(),
                bundle: text,
                summary: sum_v.clone(),
            },
        )?;
    }
    let jobs = summary.jobs.iter().map(|(k, j)| format!("{k} job {} on {} receipt {}", j.job, j.node, &j.receipt[..16])).collect::<Vec<_>>().join("; ");
    audit(
        ctx,
        &dir,
        &AuditLine::event(if again { "EVIDENCE-AGAIN" } else { "EVIDENCE" }, &format!("{project}; bundle {}; tree {}; {jobs}", &sha[..16], summary.tree)),
    )?;
    for n in &summary.notes {
        env.say(format!("note: {n}"));
    }
    env.say(format!(
        "evidence for {project} {version} {}: tree {}",
        if again { "already registered (the same bundle, checked again)" } else { "registered" },
        &summary.tree[..12]
    ));
    json_out(&json!({"ok": true, "project": project, "version": version, "again": again, "summary": sum_v}))
}

// ── approvals ──────────────────────────────────────────────────────────

fn verb_challenge(env: &Env, args: &[String]) -> R<Vec<u8>> {
    let ctx = env.ctx;
    let reapprove = args.iter().any(|a| a == "--reapprove");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--reapprove").collect();
    if let Some(f) = rest.iter().find(|a| a.starts_with("--")) {
        return usage(format!("challenge: unknown flag {f}"));
    }
    if rest.len() < 4 {
        return usage("challenge <project> <version> <base-commit> <asset>... [--reapprove]");
    }
    let (project, version, base) = (rest[0], rest[1], rest[2]);
    policy::check_project_name(project)?;
    session::check_version(version)?;
    session::check_commit(base)?;
    ctx::check_pins(ctx)?;
    let pol = policy::load(ctx, project)?;
    let mut assets: Vec<String> = Vec::new();
    for a in &rest[3..] {
        check_asset_name(a)?;
        if !pol.assets.contains_key(a.as_str()) {
            return refused(format!("asset {a} is not signable in project {project}'s policy"));
        }
        if assets.contains(*a) {
            return usage(format!("asset {a} listed twice"));
        }
        assets.push((*a).clone());
    }
    assets.sort();
    let _l = Locked::take(ctx)?;
    let text = session::challenge(ctx, project, version, base, &assets, reapprove)?;
    Ok(text.into_bytes())
}

fn verb_approve(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 2, "approve <project> <nonce>   (the DER approval signature on stdin)")?;
    let ctx = env.ctx;
    policy::check_project_name(&args[0])?;
    ctx::check_pins(ctx)?;
    let _l = Locked::take(ctx)?;
    let sig = (env.stdin)(1024)?;
    if sig.is_empty() {
        return refused("no approval signature on stdin");
    }
    let s = match session::approve(ctx, &args[0], &args[1], &sig) {
        Ok(s) => s,
        Err(e) => {
            let d = registry::audit_dir(ctx, &args[0], "approvals");
            audit(ctx, &d, &AuditLine::event("REFUSED-APPROVAL", &e.to_string()))?;
            return Err(e);
        }
    };
    let d = registry::audit_dir(ctx, &args[0], &s.version);
    audit(ctx, &d, &AuditLine::event("APPROVED", &format!("base {}; {}; expires {}", s.base_commit, s.assets.join(" "), ctx::iso(s.expires))))?;
    env.say(format!("signing session for {} {} approved until {}", args[0], s.version, ctx::hhmm(s.expires)));
    Ok(format!("{}\n", s.nonce).into_bytes())
}

// ── signing ────────────────────────────────────────────────────────────

fn keys_dir(ctx: &Ctx, project: &str) -> std::path::PathBuf {
    if project == "k2" {
        ctx.var().join("keys")
    } else {
        registry::project_dir(ctx, project).join("keys")
    }
}

fn open_key(ctx: &Ctx, project: &str, slot: &str) -> R<SecretKey> {
    let d = keys_dir(ctx, project);
    if !d.is_dir() || ctx::mode_of(&d)? != 0o700 {
        return refused(format!("{} must exist with mode 700", d.display()));
    }
    let (k, pw) = (d.join(format!("{slot}.key")), d.join(format!("{slot}.pw")));
    for p in [&k, &pw] {
        if !p.is_file() {
            return refused(format!(
                "{project}'s {slot} key is not installed: {} missing (k2-signer-admin generate-key / import-key)",
                p.display()
            ));
        }
        if ctx::mode_of(p)? != 0o600 {
            return refused(format!("{} must be mode 600", p.display()));
        }
    }
    let raw = std::fs::read(&k)?;
    // L9: the password and the key file never outlive this call in memory.
    let password = zeroize::Zeroizing::new(std::fs::read(&pw)?);
    let raw = zeroize::Zeroizing::new(raw);
    let pw = password.strip_suffix(b"\n").unwrap_or(&password);
    SecretKey::open(&raw, pw).map_err(|e| Fail::Refused(format!("{project}'s {slot} key: {e}")))
}

/// The comments a signature carries: k2sign's for project `k2` (U1 as
/// Tauri's, U2 with the version), Tauri's plus the version for a client.
fn comments(project: &str, slot: &str, asset: &str, version: &str, now: i64) -> (String, String) {
    let ts = format!("timestamp:{now}");
    if project == "k2" && slot == "u1" {
        ("signature from tauri secret key".into(), format!("{ts}\tfile:{asset}"))
    } else if project == "k2" {
        ("signature from K2 update key".into(), format!("{ts}\tfile:{asset}\tversion:{version}"))
    } else {
        ("signature from tauri secret key".into(), format!("{ts}\tfile:{asset}\tversion:{version}"))
    }
}

/// The evidence one asset's bytes need (sign and verdict): registered
/// evidence for the version, checked again in full now; the bytes are the
/// build receipt's artifact (which the dual build reproduced); K2's daemon
/// byte checks for project `k2`.
#[allow(clippy::too_many_arguments)]
fn asset_evidence(
    env: &Env,
    pol: &SignerPolicy,
    rule: &k2_node_proto::signer_policy::AssetRule,
    project: &str,
    asset: &str,
    version: &str,
    sess: &Session,
    data: &[u8],
    sha: &str,
) -> R<Summary> {
    let ctx = env.ctx;
    let Some(reg) = registry::load_evidence(ctx, project, version)? else {
        return refused(format!("no registered evidence for {project} {version} (k2-signer evidence {project} {version} first)"));
    };
    let bundle = EvidenceBundle::parse(reg.bundle.as_bytes()).map_err(Fail::Refused)?;
    let summary = check_release(env, pol, &bundle, sess, version)?;
    let Some((label, _)) = summary.build_job(&rule.build) else {
        return refused(format!("the registered evidence has no build receipt for build {}", rule.build));
    };
    let er = &bundle.receipts[label];
    if er.role != Role::Build {
        return Err(Fail::Internal("build_job returned a verify receipt".into()));
    }
    let Some((asha, abytes)) = check::receipt_artifact(label, er, &rule.artifact)? else {
        return refused(format!("{asset}: {} is not an artifact in {label}'s signed receipt", rule.artifact));
    };
    if asha != sha || abytes != data.len() as u64 {
        return refused(format!("{asset}: these bytes ({}…) are not {label}'s signed artifact ({}…)", &sha[..12], &asha[..12]));
    }
    if project == "k2" {
        crate::k2daemon::check(asset, data, version)?;
    }
    Ok(summary)
}

/// `verdict <project> <asset> <version>` (file on stdin): every check `sign`
/// runs, without signing (V5: a `sign-*.sh` that signs with a key the signer
/// doesn't hold refuses without this verdict). Audited; changes nothing.
fn verb_verdict(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 3, "verdict <project> <asset> <version>   (file bytes on stdin)")?;
    let ctx = env.ctx;
    let (project, asset, version) = (&args[0], &args[1], &args[2]);
    policy::check_project_name(project)?;
    check_asset_name(asset)?;
    session::check_version(version)?;
    ctx::check_pins(ctx)?;
    ctx::check_var(ctx)?;
    let pol = policy::load(ctx, project)?;
    let Some(rule) = pol.assets.get(asset.as_str()) else {
        return refused(format!("asset {asset} is not in project {project}'s policy"));
    };
    let _l = Locked::take(ctx)?;
    let dir = registry::audit_dir(ctx, project, version);
    let sess = session::load_valid(ctx, project, version)?;
    if !sess.assets.contains(asset) {
        return refused(format!("{asset} is not in {project} {version}'s approved asset list"));
    }
    k2_asset_rules(ctx, project, asset, version, &sess)?;
    let data = (env.stdin)(MAX_SIGN_BYTES)?;
    let sha = ctx::sha256_hex(&data);
    if let Some(f) = registry::signed_assets(ctx, project, version)?.get(asset.as_str()) {
        if *f != sha {
            return refused(format!("REFUSED-SWAP: {asset} was already signed as {f}"));
        }
    }
    match asset_evidence(env, &pol, rule, project, asset, version, &sess, &data, &sha) {
        Ok(summary) => {
            audit(ctx, &dir, &AuditLine { event: "VERDICT-OK", slot: "", asset, sha: &sha, key_id: "", tc: "", detail: "k2-signer verdict" })?;
            json_out(&json!({"ok": true, "project": project, "asset": asset, "version": version, "sha256": sha,
                "tree": summary.tree, "jobs": summary.jobs}))
        }
        Err(e) => {
            audit(ctx, &dir, &AuditLine { event: "REFUSED-VERDICT", slot: "", asset, sha: &sha, key_id: "", tc: "", detail: &e.to_string() })?;
            Err(e)
        }
    }
}

fn verb_sign(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 5, "sign <project> <slot> <asset> <version> raw|b64   (file bytes on stdin)")?;
    let ctx = env.ctx;
    let (project, slot, asset, version, form) = (&args[0], &args[1], &args[2], &args[3], &args[4]);
    policy::check_project_name(project)?;
    check_slot(slot)?;
    check_asset_name(asset)?;
    session::check_version(version)?;
    if form != "raw" && form != "b64" {
        return usage("form must be raw or b64");
    }
    ctx::check_pins(ctx)?;
    ctx::check_var(ctx)?;
    let pol = policy::load(ctx, project)?;
    let Some(rule) = pol.assets.get(asset.as_str()) else {
        return refused(format!("asset {asset} is not signable in project {project}'s policy"));
    };
    // K2's U1/U2 bridge (until SS6): K2's assets sign under both keys.
    let slot_ok = *slot == rule.key || (project == "k2" && slot == "u1" && rule.key == "u2");
    if !slot_ok {
        return refused(format!("asset {asset} is signed with key {}, not {slot}", rule.key));
    }
    let Some(key_rule) = pol.keys.get(slot.as_str()) else {
        return refused(format!("project {project}'s policy has no [keys.{slot}]"));
    };
    let _l = Locked::take(ctx)?;
    let dir = registry::audit_dir(ctx, project, version);
    let mut sess = session::load_valid(ctx, project, version)?;
    if !sess.assets.contains(asset) {
        return refused(format!("{asset} is not in {project} {version}'s approved asset list"));
    }
    k2_asset_rules(ctx, project, asset, version, &sess)?;
    let data = (env.stdin)(MAX_SIGN_BYTES)?;
    let sha = ctx::sha256_hex(&data);
    let first = registry::signed_assets(ctx, project, version)?;
    if let Some(f) = first.get(asset.as_str()) {
        if *f != sha {
            audit(ctx, &dir, &AuditLine { event: "REFUSED-SWAP", slot, asset, sha: &sha, key_id: "", tc: "", detail: &format!("already signed as {f}") })?;
            return refused(format!("REFUSED-SWAP: {asset} already signed as {f}"));
        }
    }
    let skey = format!("{slot} {asset} {sha}");
    let again = sess.signed.contains(&skey);
    if !again && sess.count >= sess.max_signatures {
        return refused(format!("the session for {project} {version} reached its {} signatures", sess.max_signatures));
    }
    let ev = asset_evidence(env, &pol, rule, project, asset, version, &sess, &data, &sha);
    if let Err(e) = ev {
        audit(ctx, &dir, &AuditLine { event: "REFUSED-EVIDENCE", slot, asset, sha: &sha, key_id: "", tc: "", detail: &e.to_string() })?;
        return Err(e);
    }
    let sk = open_key(ctx, project, slot)?;
    let pinned = minisign::PublicKey::parse(&key_rule.public).map_err(Fail::Refused)?;
    if sk.public() != pinned {
        return refused(format!("{project}'s {slot} key is {}, but the policy pins {}", sk.public().key_id(), pinned.key_id()));
    }
    let (uc, tc) = comments(project, slot, asset, version, ctx.now());
    let text = sk.sign(&data, &uc, &tc).map_err(Fail::Refused)?;
    let (algo, key_id) = minisign::sig_algo(&text).map_err(Fail::Refused)?;
    if algo != "ED" || key_id != pinned.key_id() {
        return refused(format!("{asset}: the new signature is {algo} by {key_id}, want ED by {}", pinned.key_id()));
    }
    let got = minisign::verify(&pinned.line(), &data, &text).map_err(|e| Fail::Refused(format!("{asset}: the new signature does not verify with the pinned key: {e}")))?;
    if got != tc {
        return refused(format!("{asset}: trusted comment round-trip mismatch"));
    }
    if !first.contains_key(asset.as_str()) {
        registry::record_asset(ctx, project, version, asset, &sha)?;
    }
    if !again {
        sess.count += 1;
        sess.signed.push(skey);
        session::save(ctx, project, &sess)?;
    }
    audit(
        ctx,
        &dir,
        &AuditLine { event: "SIGNED", slot, asset, sha: &sha, key_id: &key_id, tc: &tc, detail: if again { "k2-signer; re-sign, same bytes" } else { "k2-signer" } },
    )?;
    Ok(if form == "b64" { k2_node_proto::crypto::b64(text.as_bytes()).into_bytes() } else { text.into_bytes() })
}

// ── read-only ──────────────────────────────────────────────────────────

fn verb_status(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 2, "status <project> <version>")?;
    let ctx = env.ctx;
    let (project, version) = (&args[0], &args[1]);
    policy::check_project_name(project)?;
    session::check_version(version)?;
    ctx::check_pins(ctx)?;
    let sess = session::load(ctx, project, version)?;
    let reg = registry::load_evidence(ctx, project, version)?;
    let signed: Vec<String> = registry::signed_assets(ctx, project, version)?.into_iter().map(|(a, s)| format!("{a}\t{s}")).collect();
    json_out(&json!({
        "project": project, "version": version,
        "session": sess.map(|s| json!({"base_commit": s.base_commit, "assets": s.assets, "approved": ctx::iso(s.approved),
            "expires": ctx::iso(s.expires), "valid": ctx.now() <= s.expires, "by": s.by, "count": s.count,
            "max_signatures": s.max_signatures})),
        "signed": signed,
        "evidence": reg.map(|r| json!({"bundle_sha256": r.bundle_sha256, "registered": ctx::iso(r.registered), "summary": r.summary})),
    }))
}

fn verb_pins(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 0, "pins")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let t = Trust::load(ctx)?;
    let marks = trust::load_marks(ctx)?;
    let mut projects = Vec::new();
    if ctx.projects_dir().is_dir() {
        for e in std::fs::read_dir(ctx.projects_dir())? {
            let n = e?.file_name().to_string_lossy().into_owned();
            if let Some(p) = n.strip_suffix(".toml") {
                projects.push(p.to_string());
            }
        }
    }
    projects.sort();
    json_out(&json!({
        "controllers": t.controllers.iter().map(|(fp, c)| json!({"fp": fp, "label": c.label, "how": c.how})).collect::<Vec<_>>(),
        "owners": t.owners.iter().map(|(fp, o)| json!({"fp": fp, "label": o.label, "accepted_keys": o.accepted.len(),
            "roster_seq": marks.get(fp).map(|m| m.seq), "roster_issued": marks.get(fp).map(|m| ctx::iso(m.issued_at))})).collect::<Vec<_>>(),
        "projects": projects,
    }))
}

fn identity_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.own_dir().join("identity.pem")
}

fn load_identity(ctx: &Ctx) -> R<Option<SigningKey>> {
    let p = identity_path(ctx);
    if !p.exists() {
        return Ok(None);
    }
    if ctx::mode_of(&p)? != 0o600 {
        return refused(format!("{} must be mode 600", p.display()));
    }
    Ok(Some(SigningKey::from_pkcs8_pem(&std::fs::read_to_string(&p)?).map_err(Fail::Refused)?))
}

fn verb_identity(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 0, "identity")?;
    let k = load_identity(env.ctx)?;
    json_out(&json!({"fp": k.as_ref().map(|k| k.fingerprint()), "spki": k.as_ref().map(|k| k2_node_proto::crypto::b64(k.spki_der()))}))
}

// ── admin ──────────────────────────────────────────────────────────────

fn admin_audit(ctx: &Ctx, event: &str, detail: &str) -> R<()> {
    audit(ctx, &ctx.own_dir().join("audit").join("admin"), &AuditLine::event(event, detail))
}

fn admin_policy_approve(env: &Env, args: &[String]) -> R<Vec<u8>> {
    if args.is_empty() || args.len() > 2 {
        return usage("policy-approve <project> [<sha256 of the policy, from the first run>]   (the policy TOML on stdin)");
    }
    let ctx = env.ctx;
    let project = &args[0];
    let path = policy::policy_path(ctx, project)?;
    let raw = (env.stdin)(policy::MAX_POLICY_BYTES as usize)?;
    let text = String::from_utf8(raw).map_err(|_| Fail::Refused("the policy is not UTF-8".into()))?;
    let p = policy::parse(&text, project)?;
    let sha = ctx::sha256_hex(text.as_bytes());
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let Some(confirm) = args.get(1) else {
        env.say(format!("project {project}: the proposed policy parses ({} builds, {} assets, {} keys)", p.build.len(), p.assets.len(), p.keys.len()));
        let diff = policy::diff(&old, &text);
        return Ok(format!(
            "{diff}\nTo approve exactly this policy, run again with its sha256:\n  sudo k2-signer-admin policy-approve {project} {sha} < the-same-file\n"
        )
        .into_bytes());
    };
    if *confirm != sha {
        return refused(format!("that policy's sha256 is {sha}, not the {confirm} you approved: not installed (run without the sha to see it)"));
    }
    ctx::check_pins(ctx)?;
    std::fs::create_dir_all(ctx.projects_dir())?;
    let tmp = ctx.projects_dir().join(format!(".{project}.toml.tmp"));
    std::fs::write(&tmp, &text)?;
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
    std::fs::rename(&tmp, &path)?;
    ctx::repin(ctx)?;
    admin_audit(ctx, "POLICY-APPROVED", &format!("{project}; sha256 {sha}"))?;
    env.say(format!("policy for {project} installed (sha256 {sha}) and pinned"));
    Ok(Vec::new())
}

fn admin_trust_owner(env: &Env, args: &[String]) -> R<Vec<u8>> {
    if args.len() < 2 || args.len() > 3 {
        return usage("trust-owner <fingerprint you read on the owner's own screen> <ed25519:key> [<label>]");
    }
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let (fp, key) = (&args[0], &args[1]);
    if !k2_node_proto::ci::is_fp(fp) {
        return usage("the fingerprint is 64 lowercase hex (k2 compute owners on the owner's server)");
    }
    let k = ApprovalKey::parse(key).ok_or_else(|| Fail::Usage("the key is ed25519:<base64 of 32 bytes>".into()))?;
    if k.fingerprint() != *fp {
        return refused(format!("that key's fingerprint is {}, not the {fp} you read: not pinned", k.fingerprint()));
    }
    let mut t = Trust::load(ctx)?;
    if let Some(o) = t.owners.get(fp) {
        if o.key != k.encode() {
            return Err(Fail::Internal("two keys with one fingerprint".into()));
        }
        return refused(format!("owner {fp} is already pinned ({})", o.label));
    }
    let label = args.get(2).cloned().unwrap_or_default();
    k2_node_proto::ci::is_name(&label).then_some(()).or(label.is_empty().then_some(())).ok_or_else(|| Fail::Usage("the label is a name ([a-z0-9._-])".into()))?;
    t.owners.insert(fp.clone(), OwnerPin { key: k.encode(), label: label.clone(), at: ctx.now(), accepted: BTreeMap::new() });
    t.save(ctx)?;
    admin_audit(ctx, "OWNER-TRUSTED", &format!("{fp} {label}"))?;
    env.say(format!("owner {label} pinned: roster key {fp}. Its node keys are accepted one roster at a time (roster-accept)."));
    Ok(Vec::new())
}

fn admin_untrust_owner(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 1, "untrust-owner <fp>")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let mut t = Trust::load(ctx)?;
    if t.owners.remove(&args[0]).is_none() {
        return refused(format!("no owner {} is pinned", args[0]));
    }
    t.save(ctx)?;
    admin_audit(ctx, "OWNER-UNTRUSTED", &args[0])?;
    Ok(Vec::new())
}

fn admin_roster_accept(env: &Env, args: &[String]) -> R<Vec<u8>> {
    if args.is_empty() || args.len() > 2 {
        return usage("roster-accept <owner fp> [<roster digest, from the first run>]   (an evidence bundle or a signed roster on stdin)");
    }
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let owner = &args[0];
    let mut t = Trust::load(ctx)?;
    let Some(key) = t.owner_key(owner) else {
        return refused(format!("owner {owner} is not pinned (trust-owner first)"));
    };
    let raw = (env.stdin)(MAX_EVIDENCE_BYTES)?;
    let v: Value = serde_json::from_slice(&raw).map_err(|e| Fail::Refused(format!("stdin: {e}")))?;
    let (signed, rotations): (Vec<Signed>, Vec<Signed>) = if v.get("kind").is_some() {
        let b = EvidenceBundle::parse(&raw).map_err(Fail::Refused)?;
        (b.rosters, b.rotations)
    } else {
        (vec![serde_json::from_value(v).map_err(|e| Fail::Refused(format!("not a signed roster: {e}")))?], Vec::new())
    };
    let mut best: Option<(NodeRoster, String)> = None;
    for s in &signed {
        let Ok(peek) = s.peek::<NodeRoster>(k2_node_proto::ci::DOMAIN_NODE_ROSTER) else { continue };
        if peek.key_fp().ok().as_deref() != Some(owner.as_str()) {
            continue;
        }
        let r = NodeRoster::open(s, &key).map_err(Fail::Refused)?;
        if best.as_ref().is_none_or(|(b, _)| r.seq > b.seq) {
            best = Some((r, s.digest()));
        }
    }
    let Some((r, digest)) = best else {
        return refused(format!("no node roster from owner {owner} on stdin"));
    };
    if let Some(m) = trust::load_marks(ctx)?.get(owner.as_str()) {
        if r.seq < m.seq {
            return refused(format!("{}: roster seq {} is older than the registered seq {}", k2_node_proto::ci::codes::ROSTER_REPLAYED, r.seq, m.seq));
        }
    }
    let accepted = t.owners[owner].accepted.clone();
    let mut rotated: BTreeMap<String, String> = BTreeMap::new();
    for s in &rotations {
        if let Ok(p) = s.peek::<k2_node_proto::statement::RotationStatement>(k2_node_proto::ci::DOMAIN_NODE_ROTATION) {
            if let Some(old) = r.entry(&p.old_fp) {
                if let Ok(der) = old.spki_der() {
                    if let Ok(o) = k2_node_proto::statement::RotationStatement::open(s, &der) {
                        rotated.insert(o.new_fp, o.old_fp);
                    }
                }
            }
        }
    }
    let mut lines = vec![format!("{}'s node roster seq {} (issued {}), digest {digest}", r.owner_id, r.seq, ctx::hhmm(r.issued_at))];
    let mut new_keys = Vec::new();
    for e in &r.entries {
        let attrs = trust::entry_attrs(e);
        let prior = accepted.get(&e.node_fp);
        let known = prior.is_some_and(|a| a.attrs == attrs);
        let tag = if known {
            "  accepted"
        } else if e.status == k2_node_proto::ci::KeyStatus::Revoked {
            "  revoked "
        } else if prior.is_some() {
            "~ CHANGED "
        } else {
            "+ NEW     "
        };
        let link = match rotated.get(&e.node_fp) {
            Some(old) if accepted.contains_key(old) => format!(", re-key of accepted {}…", &old[..16]),
            Some(old) => format!(", re-key of {}… (not accepted)", &old[..16]),
            None => String::new(),
        };
        lines.push(format!(
            "{tag} {} {}… epoch {} {:?} basis {:?} host {} provider {} site {} admin {}{link}",
            e.name,
            &e.node_fp[..16],
            e.key_epoch,
            e.status,
            e.taint_basis,
            e.host,
            e.provider.as_deref().unwrap_or("-"),
            e.site.as_deref().unwrap_or("-"),
            e.admin_reach.join(",")
        ));
        if !known && e.status != k2_node_proto::ci::KeyStatus::Revoked {
            new_keys.push(e.clone());
        }
    }
    for fp in accepted.keys() {
        if r.entry(fp).is_none() {
            lines.push(format!("- GONE      {}… (accepted earlier, not in this roster)", &fp[..16]));
        }
    }
    let Some(confirm) = args.get(1) else {
        lines.push(String::new());
        if new_keys.is_empty() {
            lines.push("Nothing new to accept.".into());
        } else {
            lines.push(format!(
                "{} new or changed key(s). Check them with the owner, then accept exactly this roster:\n  sudo k2-signer-admin roster-accept {owner} {digest} < the-same-input",
                new_keys.len()
            ));
        }
        let mut out = lines.join("\n");
        out.push('\n');
        return Ok(out.into_bytes());
    };
    if *confirm != digest {
        return refused(format!("the roster on stdin is {digest}, not the {confirm} you accepted: nothing accepted"));
    }
    let now = ctx.now();
    let names: Vec<String> = new_keys.iter().map(|e| e.name.clone()).collect();
    if let Some(o) = t.owners.get_mut(owner.as_str()) {
        for e in &new_keys {
            o.accepted.insert(e.node_fp.clone(), AcceptedKey { name: e.name.clone(), seq: r.seq, at: now, attrs: trust::entry_attrs(e) });
        }
    }
    t.save(ctx)?;
    admin_audit(ctx, "ROSTER-ACCEPTED", &format!("owner {owner}; seq {}; {digest}; {}", r.seq, names.join(" ")))?;
    env.say(format!("accepted {} key(s) from {}'s roster seq {}: {}", new_keys.len(), r.owner_id, r.seq, names.join(", ")));
    Ok(Vec::new())
}

fn pending_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.own_dir().join("pairing").join("pending.json")
}

fn admin_pair_begin(env: &Env, args: &[String]) -> R<Vec<u8>> {
    if args.len() > 1 {
        return usage("pair-begin [<label for this signer>]");
    }
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    ctx::mkdirs(ctx, &ctx.own_dir())?;
    let identity = match load_identity(ctx)? {
        Some(k) => k,
        None => {
            let k = SigningKey::generate().map_err(Fail::Internal)?;
            let pem = k.to_pkcs8_pem().map_err(Fail::Internal)?;
            ctx::write_file(ctx, &identity_path(ctx), pem.as_bytes(), 0o600)?;
            k
        }
    };
    let label = args.first().cloned().unwrap_or_else(|| "k2-signer".into());
    pairing::check_label("the label", &label).map_err(Fail::Usage)?;
    let (code, hello, pending) = pairing::signer_begin(&identity, &label, ctx.now()).map_err(Fail::Internal)?;
    ctx::mkdirs(ctx, &ctx.own_dir().join("pairing"))?;
    ctx::write_file(ctx, &pending_path(ctx), &serde_json::to_vec(&pending).map_err(|e| Fail::Internal(e.to_string()))?, 0o600)?;
    admin_audit(ctx, "PAIR-BEGIN", &format!("signer {}", identity.fingerprint()))?;
    env.say(format!(
        "On your K2 server, as its owner or an Admin (never an agent): k2 ci signer add {code}\nThen, within {} minutes: sudo k2-signer-admin pair-reveal < offer.json, and type the 6-digit code the server then shows: sudo k2-signer-admin pair-accept <code>",
        pairing::CODE_TTL_SECS / 60
    ));
    json_out(&json!({"code": code, "hello": hello}))
}

fn load_pending(ctx: &Ctx) -> R<(SigningKey, pairing::Pending)> {
    let Some(identity) = load_identity(ctx)? else {
        return refused("no pairing started (pair-begin first)");
    };
    let pp = pending_path(ctx);
    if !pp.exists() {
        return refused("no pairing started (pair-begin first)");
    }
    let pending: pairing::Pending = serde_json::from_slice(&std::fs::read(&pp)?).map_err(|e| Fail::Internal(e.to_string()))?;
    Ok((identity, pending))
}

/// H2: check the server's offer (signature, the code's MAC, this pairing's
/// commitment) and reveal the nonce for that one offer.
fn admin_pair_reveal(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 0, "pair-reveal   (the server's offer on stdin)")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let (identity, mut pending) = load_pending(ctx)?;
    let raw = (env.stdin)(64 * 1024)?;
    let msg: pairing::OfferMsg = serde_json::from_slice(&raw).map_err(|e| Fail::Refused(format!("the offer: {e}")))?;
    let (offer, reveal) = match pairing::signer_reveal(&identity, &pending, &msg, ctx.now()) {
        Ok(x) => x,
        Err(e) => {
            admin_audit(ctx, "REFUSED-PAIR", &e)?;
            return refused(e);
        }
    };
    pending.offer = Some(msg.offer.clone());
    ctx::write_file(ctx, &pending_path(ctx), &serde_json::to_vec(&pending).map_err(|e| Fail::Internal(e.to_string()))?, 0o600)?;
    admin_audit(ctx, "PAIR-REVEAL", &format!("server {} {}", offer.server_fp, offer.server_label))?;
    env.say(format!(
        "offer from {} (key {}…) checked. Give this answer to the server; it then shows a 6-digit code: sudo k2-signer-admin pair-accept <code>",
        offer.server_label,
        &offer.server_fp[..16]
    ));
    json_out(&serde_json::to_value(&reveal).map_err(|e| Fail::Internal(e.to_string()))?)
}

fn admin_pair_accept(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 1, "pair-accept <the 6-digit code your K2 server shows>")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let (identity, pending) = load_pending(ctx)?;
    let (fp, spki, label, done) = match pairing::signer_accept(&identity, &pending, &args[0], ctx.now()) {
        Ok(x) => x,
        Err(e) => {
            admin_audit(ctx, "REFUSED-PAIR", &e)?;
            return refused(e);
        }
    };
    let mut t = Trust::load(ctx)?;
    t.controllers.insert(fp.clone(), ControllerPin { label: label.clone(), spki: Some(spki), how: PinHow::Sas, at: ctx.now() });
    t.save(ctx)?;
    std::fs::remove_file(pending_path(ctx))?;
    admin_audit(ctx, "PAIRED", &format!("controller {fp} {label}"))?;
    env.say(format!("paired with {label}: controller {fp} pinned. Name it in a project's policy (controllers) to let it build that project."));
    json_out(&serde_json::to_value(&done).map_err(|e| Fail::Internal(e.to_string()))?)
}

/// M7: `pairing-pins <project> required|optional`.
fn admin_pairing_pins(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 2, "pairing-pins <project> required|optional")?;
    let ctx = env.ctx;
    policy::check_project_name(&args[0])?;
    let on = match args[1].as_str() {
        "required" => true,
        "optional" => false,
        _ => return usage("pairing-pins <project> required|optional"),
    };
    ctx::check_pins(ctx)?;
    let mut t = Trust::load(ctx)?;
    t.pairing_pins.insert(args[0].clone(), on);
    t.save(ctx)?;
    admin_audit(ctx, "PAIRING-PINS", &format!("{} {}", args[0], args[1]))?;
    Ok(Vec::new())
}

fn admin_unpair(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 1, "unpair <controller fp>")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let mut t = Trust::load(ctx)?;
    if t.controllers.remove(&args[0]).is_none() {
        return refused(format!("no controller {} is paired", args[0]));
    }
    t.save(ctx)?;
    admin_audit(ctx, "UNPAIRED", &args[0])?;
    Ok(Vec::new())
}

fn read_abs_file(path: &str, max: u64) -> R<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    if !path.starts_with('/') {
        return usage("an absolute path (the wrapper runs from /)");
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| Fail::Refused(format!("can't read {path}: {e}")))?;
    if !f.metadata()?.is_file() {
        return refused(format!("{path} is not a regular file"));
    }
    let mut buf = Vec::new();
    f.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return refused(format!("{path} is too large"));
    }
    Ok(buf)
}

fn install_root_file(ctx: &Ctx, name: &str, data: &[u8]) -> R<()> {
    std::fs::create_dir_all(ctx.etc())?;
    let tmp = ctx.etc().join(format!(".{name}.tmp"));
    std::fs::write(&tmp, data)?;
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
    std::fs::rename(&tmp, ctx.etc().join(name))?;
    ctx::repin(ctx)?;
    Ok(())
}

fn approver_pem_ok(data: &[u8]) -> R<()> {
    let pem = std::str::from_utf8(data).map_err(|_| Fail::Refused("not a PEM public key".into()))?;
    k2_node_proto::crypto::fingerprint_of_spki_pem(pem).map_err(|e| Fail::Refused(format!("not a P-256 public key: {e}")))?;
    Ok(())
}

fn admin_set_approver(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 1, "set-approver </abs/approver.pem>   (K2 Release Approve's public key)")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    let data = read_abs_file(&args[0], 4096)?;
    approver_pem_ok(&data)?;
    install_root_file(ctx, "approver.pem", &data)?;
    admin_audit(ctx, "APPROVER-SET", &ctx::sha256_hex(&data))?;
    Ok(Vec::new())
}

fn admin_adopt_k2sign(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 0, "adopt-k2sign")?;
    let ctx = env.ctx;
    ctx::check_pins(ctx)?;
    ctx::check_var(ctx)?;
    let mut t = Trust::load(ctx)?;
    let mut report = Vec::new();
    let pins = ctx.k2sign_etc().join("compute-pins");
    if pins.exists() {
        let raw = ctx::read_trusted(ctx, &pins, 64 * 1024)?;
        for line in String::from_utf8_lossy(&raw).lines() {
            let l = line.split('#').next().unwrap_or_default().trim();
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() == 2 && parts[0] == "controller" && k2_node_proto::ci::is_fp(parts[1]) {
                let fp = parts[1].to_string();
                if !t.controllers.contains_key(&fp) {
                    t.controllers.insert(fp.clone(), ControllerPin { label: "k2sign-adopted".into(), spki: None, how: PinHow::K2signAdopted, at: ctx.now() });
                    report.push(format!("controller {fp}: adopted from k2sign's S3b pin"));
                }
            } else if parts.first() == Some(&"node") {
                report.push(format!("node pin {}: not adopted (node keys come from owner rosters now; trust-owner + roster-accept)", parts.get(1).unwrap_or(&"?")));
            }
        }
    } else {
        report.push("k2sign has no compute-pins: no controller to adopt".into());
    }
    let ap = ctx.etc().join("approver.pem");
    let kap = ctx.k2sign_etc().join("approver.pem");
    if !ap.exists() && kap.exists() {
        let data = ctx::read_trusted(ctx, &kap, 4096)?;
        approver_pem_ok(&data)?;
        install_root_file(ctx, "approver.pem", &data)?;
        report.push("approver.pem: copied from k2sign (the same K2 Release Approve key)".into());
    }
    t.save(ctx)?;
    for (what, p) in [
        ("one sha per asset (project k2)", ctx.state().join("assets")),
        ("receipt replay registry", registry::receipts_path(ctx)),
        ("approval sessions (project k2)", ctx.state().join("sessions")),
        ("U1/U2 keys (project k2)", ctx.var().join("keys")),
    ] {
        report.push(format!("shared with k2sign: {what}: {}{}", p.display(), if p.exists() { "" } else { " (not there yet)" }));
    }
    admin_audit(ctx, "ADOPTED-K2SIGN", &report.join("; "))?;
    let mut out = report.join("\n");
    out.push('\n');
    Ok(out.into_bytes())
}

fn new_key_files(ctx: &Ctx, project: &str, slot: &str) -> R<(std::path::PathBuf, std::path::PathBuf)> {
    if project == "k2" {
        return refused("project k2's U1/U2 keys are k2sign's (k2sign-admin import u1 / generate-u2)");
    }
    let d = keys_dir(ctx, project);
    ctx::mkdirs(ctx, &ctx.own_dir())?;
    ctx::mkdirs(ctx, &ctx.own_dir().join("projects"))?;
    ctx::mkdirs(ctx, &registry::project_dir(ctx, project))?;
    ctx::mkdirs(ctx, &d)?;
    let (k, pw) = (d.join(format!("{slot}.key")), d.join(format!("{slot}.pw")));
    if k.exists() || pw.exists() {
        return refused(format!("{project}'s {slot} key already exists in {}", d.display()));
    }
    Ok((k, pw))
}

fn admin_generate_key(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 2, "generate-key <project> <slot>")?;
    let ctx = env.ctx;
    policy::check_project_name(&args[0])?;
    check_slot(&args[1])?;
    ctx::check_pins(ctx)?;
    let (k, pw) = new_key_files(ctx, &args[0], &args[1])?;
    let password = zeroize::Zeroizing::new(k2_node_proto::crypto::hex(&k2_node_proto::crypto::random_bytes(32).map_err(Fail::Internal)?));
    let (sk, file, public) =
        SecretKey::generate(password.as_bytes(), minisign::OPSLIMIT_SENSITIVE, minisign::MEMLIMIT_SENSITIVE).map_err(Fail::Internal)?;
    let test = sk.sign(b"k2-signer generate self-test\n", "self-test", "timestamp:0\tfile:self-test").map_err(Fail::Internal)?;
    minisign::verify(&public.line(), b"k2-signer generate self-test\n", &test).map_err(|e| Fail::Internal(format!("self-test: {e}")))?;
    ctx::write_file(ctx, &k, file.as_bytes(), 0o600)?;
    ctx::write_file(ctx, &pw, zeroize::Zeroizing::new(format!("{}\n", password.as_str())).as_bytes(), 0o600)?;
    admin_audit(ctx, "KEY-GENERATED", &format!("{} {} {}", args[0], args[1], public.key_id()))?;
    env.say(format!(
        "{}'s {} key generated (its password is random and was never shown). Key id {}. Put this public line in the policy's [keys.{}]:",
        args[0],
        args[1],
        public.key_id(),
        args[1]
    ));
    Ok(format!("{}\n", public.line()).into_bytes())
}

fn admin_import_key(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 3, "import-key <project> <slot> </abs/key file>   (its password on stdin, one line)")?;
    let ctx = env.ctx;
    policy::check_project_name(&args[0])?;
    check_slot(&args[1])?;
    ctx::check_pins(ctx)?;
    let raw = read_abs_file(&args[2], 64 * 1024)?;
    let pw_raw = (env.stdin)(4096)?;
    let pw_text = String::from_utf8(pw_raw).map_err(|e| {
        let mut b = e.into_bytes();
        zeroize::Zeroize::zeroize(&mut b);
        Fail::Refused("the password is not UTF-8".into())
    })?;
    let mut pw_text = pw_text;
    let password = zeroize::Zeroizing::new(pw_text.lines().next().unwrap_or_default().to_string());
    zeroize::Zeroize::zeroize(&mut pw_text);
    if password.is_empty() {
        return refused("an empty password: the signer imports only encrypted keys");
    }
    if !minisign::is_encrypted(&raw).map_err(Fail::Refused)? {
        return refused("an unencrypted minisign key: the signer imports only password-encrypted keys");
    }
    let sk = SecretKey::open(&raw, password.as_bytes()).map_err(Fail::Refused)?;
    let text = minisign::unwrap_key_file(&raw).map_err(Fail::Refused)?;
    let (k, pw) = new_key_files(ctx, &args[0], &args[1])?;
    ctx::write_file(ctx, &k, text.as_bytes(), 0o600)?;
    ctx::write_file(ctx, &pw, zeroize::Zeroizing::new(format!("{}\n", password.as_str())).as_bytes(), 0o600)?;
    admin_audit(ctx, "KEY-IMPORTED", &format!("{} {} {}", args[0], args[1], sk.public().key_id()))?;
    env.say(format!("{}'s {} key imported: key id {}", args[0], args[1], sk.public().key_id()));
    Ok(format!("{}\n", sk.public().line()).into_bytes())
}

fn admin_set_gh_token(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 1, "set-gh-token <project>   (a read-only token for the project's repo, on stdin)")?;
    let ctx = env.ctx;
    policy::check_project_name(&args[0])?;
    if args[0] == "k2" {
        return refused("project k2 reads GitHub with k2sign's token (k2sign-admin set-gh-token)");
    }
    ctx::check_pins(ctx)?;
    let raw = (env.stdin)(4096)?;
    let tok = String::from_utf8_lossy(&raw).trim().to_string();
    if tok.len() < 20 || !tok.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return refused("that doesn't look like a GitHub token");
    }
    let d = registry::project_dir(ctx, &args[0]);
    ctx::mkdirs(ctx, &ctx.own_dir().join("projects"))?;
    ctx::mkdirs(ctx, &d)?;
    ctx::write_file(ctx, &d.join("gh-token"), format!("{tok}\n").as_bytes(), 0o600)?;
    admin_audit(ctx, "GH-TOKEN-SET", &args[0])?;
    Ok(Vec::new())
}

fn admin_clear_evidence(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 2, "clear-evidence <project> <version>")?;
    let ctx = env.ctx;
    let (project, version) = (&args[0], &args[1]);
    policy::check_project_name(project)?;
    session::check_version(version)?;
    ctx::check_pins(ctx)?;
    let _l = Locked::take(ctx)?;
    let Some(r) = registry::load_evidence(ctx, project, version)? else {
        return refused(format!("{project} {version} has no evidence registered"));
    };
    std::fs::remove_file(registry::evidence_path(ctx, project, version))?;
    audit(
        ctx,
        &registry::audit_dir(ctx, project, version),
        &AuditLine::event("EVIDENCE-CLEARED-BY-ADMIN", &format!("{project}; bundle {}", r.bundle_sha256)),
    )?;
    env.say(format!(
        "evidence for {project} {version} cleared (audited). The one-sha-per-asset rule still holds: bytes already signed stay the only ones it signs."
    ));
    Ok(Vec::new())
}

fn admin_repin(env: &Env, args: &[String]) -> R<Vec<u8>> {
    need(args, 0, "repin")?;
    Ok(ctx::repin(env.ctx)?.into_bytes())
}
