//! Receipts and build statements on the server (prd-compute-build-
//! attestations-v1 §5, slice S3). The node signs an in-toto statement
//! beside a job's receipt (S2, `k2_node_proto::provenance`); this file is
//! the server's half:
//!
//! - **Asking.** A plan carries `provenance: 1` only to a node offering
//!   `provenance-v1`, with the CI preview on ([`ask`]); `fit_plan` strips
//!   it for any other node, so an older node's plan and digest don't move.
//! - **Receiving.** [`received`] checks the envelope with the node key this
//!   server pinned and against the receipt it came with
//!   (`Dsse::open_for_receipt`), and refuses one whose plain env names a
//!   vault secret (BA-39). The job's stored receipt keeps it under
//!   `provenance`, or the reason under `provenance_refused`: no migration.
//! - **Reading.** `GET /cli/compute/jobs/provenance` (`k2 compute
//!   provenance`): the `.intoto.jsonl` line. `POST /cli/compute/verify`
//!   (`k2 compute verify`): checks a statement against this server's
//!   records and says what it checked. Both are a convenience for agents:
//!   a release signer checks everything again with its own pins.
//! - `GET /cli/compute/jobs/receipt` (`k2 compute receipt`) lives here too.
//!
//! Spec: `docs/provenance-v1.md`.

use std::collections::{BTreeMap, HashMap};

use k2_core::compute::proto::ci::{KeyStatus, Signed};
use k2_core::compute::proto::crypto;
use k2_core::compute::proto::frames::{Plan, Receipt, ReceiptFrame, SignedReceipt, RECEIPT_V2};
use k2_core::compute::proto::provenance::{self as prov, Dsse, Owner, Statement};
use k2_core::compute::{self as compute, codes, store, trust};
use serde_json::{json, Value};

use crate::compute_routes::{ci_gate, job_for, json_body, no, s, Who, R};
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::cli_name;

/// `GET /cli/compute/jobs/receipt?job=` (`k2 compute receipt`).
pub const RECEIPT: &str = "/cli/compute/jobs/receipt";
/// `GET /cli/compute/jobs/provenance?job=` (`k2 compute provenance`).
pub const PROVENANCE: &str = "/cli/compute/jobs/provenance";
/// `POST /cli/compute/verify` (`k2 compute verify`).
pub const VERIFY: &str = "/cli/compute/verify";

pub const ROUTES: &[&str] = &[RECEIPT, PROVENANCE, VERIFY];
pub const POST_ROUTES: &[&str] = &[VERIFY];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

/// Refusal codes.
pub mod refusals {
    /// The job has no build statement this server can hand out; the hint
    /// says why (no files, an older node, refused on arrival, …).
    pub const NO_STATEMENT: &str = "no_statement";
    /// BA-39: a plain env name of the statement is also a secret's name in
    /// the vault, so its value may be in the statement. Not written out.
    pub const SECRET_IN_ENV: &str = "provenance_secret_in_env";
}

/// Where a job's stored receipt keeps the statement, or why it doesn't.
const KEY: &str = "provenance";
const KEY_REFUSED: &str = "provenance_refused";

pub(crate) fn handle(p: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    match p {
        RECEIPT => job_receipt(who, params),
        PROVENANCE => job_provenance(who, params),
        VERIFY => verify(who, body),
        _ => Err(no("not_found", 404, "no such compute route")),
    }
}

// ── asking ───────────────────────────────────────────────────────────

/// `Plan.provenance` for a node with these features: `1` when it offers
/// `provenance-v1` and `ci_on`; else absent.
pub fn ask_for(features: &[String], ci_on: bool) -> Option<u32> {
    (ci_on && features.iter().any(|f| f == prov::FEATURE_PROVENANCE_V1)).then_some(prov::PROVENANCE_V1)
}

/// [`ask_for`] with this server's CI preview.
pub fn ask(features: &[String]) -> Option<u32> {
    ask_for(features, compute::ci_enabled())
}

// ── receiving ────────────────────────────────────────────────────────

/// What a receipt frame's statement comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// None came.
    Nothing,
    Statement(Box<Dsse>),
    /// One came and this server refused it: the job has no statement.
    Refused(String),
}

/// Check the statement a receipt frame carries: the receipt passed this
/// server's check (`receipt_ok`), the plan asked for one, it verifies with
/// the pinned node key `spki` and agrees with the receipt, and no plain
/// env name is a vault secret's (BA-39; with the vault closed that check
/// waits until the statement is written out).
pub fn received(job: &store::JobRow, spki: Option<&[u8]>, rf: &ReceiptFrame, receipt_ok: bool) -> Kept {
    let Some(env) = &rf.provenance else { return Kept::Nothing };
    let refused = |why: &str| Kept::Refused(why.to_string());
    if !receipt_ok {
        return refused("the receipt it came with failed this server's check");
    }
    if job.plan().and_then(|p| p.provenance) != Some(prov::PROVENANCE_V1) {
        return refused("the job's plan didn't ask for one");
    }
    let Some(spki) = spki else { return refused("this server holds no key for the node") };
    let st = match env.open_for_receipt(spki, &rf.receipt) {
        Ok(st) => st,
        Err(e) => return Kept::Refused(e),
    };
    if let Ok(names) = secret_names(&job.workspace_id, &st) {
        if !names.is_empty() {
            return Kept::Refused(secret_in_env(&names));
        }
    }
    Kept::Statement(Box::new(env.clone()))
}

/// Put what [`received`] decided into the stored receipt object.
pub fn keep(stored: &mut Value, kept: &Kept) {
    match kept {
        Kept::Nothing => {}
        Kept::Statement(env) => stored[KEY] = serde_json::to_value(env.as_ref()).unwrap_or_default(),
        Kept::Refused(why) => stored[KEY_REFUSED] = Value::from(why.as_str()),
    }
}

// ── BA-39 ────────────────────────────────────────────────────────────

/// The statement's plain env names that are also a secret's name in the
/// vault (the job's workspace, or the server). `Err` when the vault is
/// closed and can't be asked.
pub(crate) fn secret_names(ws_id: &str, st: &Statement) -> Result<Vec<String>, String> {
    let env = &st.predicate.build_definition.external_parameters.env;
    if env.is_empty() {
        return Ok(Vec::new());
    }
    let vault = crate::vault::get().map_err(|e| format!("the vault isn't open ({}), so {PRODUCT_NAME} can't check that no plain env name is a secret's; open it and try again", e.code))?;
    let ws = k2_vault::Scope::workspace(ws_id);
    Ok(env.keys().filter(|k| vault.user_secret_exists(&ws, k) || vault.user_secret_exists(&k2_vault::Scope::server(), k)).cloned().collect())
}

fn secret_in_env(names: &[String]) -> String {
    format!(
        "plain env {} {} also a secret's name in the vault, and plain env values are written into the statement: send secrets with --secret NAME (names only), never as plain env",
        names.join(", "),
        if names.len() == 1 { "is" } else { "are" }
    )
}

// ── reading ──────────────────────────────────────────────────────────

/// The job's stored statement, or why it has none, in plain words.
pub(crate) fn stored(j: &store::JobRow) -> Result<Dsse, String> {
    let Some(raw) = j.receipt_json.as_deref() else {
        return Err("no receipt yet: the node sends the receipt and its statement when the job ends".into());
    };
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("the stored receipt doesn't read: {e}"))?;
    if let Some(why) = v.get(KEY_REFUSED).and_then(Value::as_str) {
        return Err(format!("this server refused the statement the node sent: {why}"));
    }
    if let Some(env) = v.get(KEY) {
        return serde_json::from_value(env.clone()).map_err(|e| format!("the stored statement doesn't read: {e}"));
    }
    let short = compute::message::short(&j.id);
    if j.plan().and_then(|p| p.provenance).is_none() {
        return Err("the job's plan didn't ask for one: its node didn't offer build statements when the job was sent (k2-node 0.46.13 or newer does, with the CI preview on)".into());
    }
    let signed: Option<SignedReceipt> = serde_json::from_value(v).ok();
    let files = signed.and_then(|s| serde_json::from_str::<Receipt>(&s.body).ok()).map(|r| r.artifacts.len()).unwrap_or(0);
    if files == 0 {
        return Err("the job kept no files, and a statement names the files a job made (no files, no statement)".into());
    }
    Err(format!("the node sent none with the receipt (its k2-node couldn't make one, e.g. for a job it adopted after an update; {CLI_NAME} compute get {short} shows its notes)"))
}

/// The statement to write out: [`stored`], then BA-39 again (a secret may
/// have been added to the vault since it arrived).
pub(crate) fn writable(j: &store::JobRow) -> R<(Dsse, Statement)> {
    let short = compute::message::short(&j.id);
    let env = stored(j).map_err(|why| no(refusals::NO_STATEMENT, 409, format!("job {short} has no build statement: {why}")))?;
    let st = env.peek().map_err(|e| no(refusals::NO_STATEMENT, 409, format!("job {short}'s stored statement doesn't read: {e}")))?;
    let names = secret_names(&j.workspace_id, &st).map_err(|e| no("vault_locked", 503, e))?;
    if !names.is_empty() {
        return Err(no(refusals::SECRET_IN_ENV, 409, format!("job {short}'s statement is not written out: {}", secret_in_env(&names))));
    }
    Ok((env, st))
}

/// The node's name and PEM key by fingerprint, for `trust/`.
fn node_key(conn: &rusqlite::Connection, node_id: &str) -> Option<(String, Value)> {
    store::node_by_id(conn, node_id).map(|n| (n.fingerprint.clone(), json!({"node": n.name, "pem": n.public_key_pem})))
}

/// What a client needs beside the statements to check them without K2
/// (written as `<dir>/trust/`, prd §5): each node's public key (PEM, for a
/// stock DSSE tool), the node self-statements the receipts name, and the
/// newest roster this server holds from each owner the statements name
/// (with its key fingerprint, the one value to pin).
pub(crate) fn trust_files(jobs: &[&store::JobRow], statements: &[&Statement]) -> Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut keys = serde_json::Map::new();
    let mut selfs: BTreeMap<String, Signed> = BTreeMap::new();
    for j in jobs {
        if let Some((fp, k)) = node_key(&conn, &j.node_id) {
            keys.insert(fp, k);
        }
        let st = j.receipt_json.as_deref().and_then(|r| serde_json::from_str::<Value>(r).ok()).and_then(|v| v.get("statement").cloned());
        if let Some(s) = st.and_then(|s| serde_json::from_value::<Signed>(s).ok()) {
            selfs.insert(s.digest(), s);
        }
    }
    let mut owners = Vec::new();
    let mut rosters = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for st in statements {
        if let Some(Owner::RosterKey(fp)) = st.builder_owner() {
            if seen.insert(fp.clone()) {
                if let Some(r) = trust::latest_roster(&conn, &fp) {
                    owners.push(json!({"id": r.roster.owner_id, "rosterKeyFp": fp, "rosterSeq": r.roster.seq}));
                    rosters.push(r.signed);
                }
            }
        }
    }
    json!({"owners": owners, "rosters": rosters, "statements": selfs.into_values().collect::<Vec<_>>(), "nodeKeys": keys})
}

fn job_provenance(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    ci_gate(concat!(cli_name!(), " compute provenance"))?;
    let j = job_for(who, params.get("job").map(String::as_str))?;
    let (env, st) = writable(&j)?;
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id).map(|n| n.name)
    };
    let line = env.line().map_err(|e| no("internal", 500, e))?;
    let short = compute::message::short(&j.id);
    Ok(json!({
        "job": j.id,
        "short": short,
        "node": node,
        "keyid": env.keyid(),
        "builderId": st.predicate.run_details.builder.id,
        "subjects": st.subject.iter().map(|s| json!({"name": s.name, "sha256": s.sha256()})).collect::<Vec<_>>(),
        "file": format!("{short}.intoto.jsonl"),
        "line": line,
        "envelope": env,
        "trust": trust_files(&[&j], &[&st]),
    }))
}

// ── verify ───────────────────────────────────────────────────────────

/// One check and what it found.
struct Checks {
    done: Vec<Value>,
    failed: Option<String>,
}

impl Checks {
    fn pass(&mut self, check: &str, detail: String) {
        self.done.push(json!({"check": check, "ok": true, "detail": detail}));
    }
    fn fail(&mut self, check: &str, detail: String) {
        self.done.push(json!({"check": check, "ok": false, "detail": detail.clone()}));
        self.failed.get_or_insert(detail);
    }
}

fn short_fp(fp: &str) -> String {
    format!("{}…{}", &fp[..fp.len().min(8)], &fp[fp.len().saturating_sub(4)..])
}

/// `POST /cli/compute/verify` `{job}` or `{envelope}` (a `.intoto.jsonl`
/// line), plus `{sha256, file}` to check a file is one of its subjects and
/// `{ownerKey}` to pin an owner's roster key. Checks, in order, and stops
/// at the first that fails: the key that signed is the one this server
/// pinned for the job's node; the receipt verifies with it; the statement
/// is well formed and agrees with the receipt; no plain env name is a
/// vault secret's; the file is a subject; and the owner (`builder.id`)
/// against the rosters held here. `verified` is the answer; a refusal is
/// only for a bad question (no such job, a body that isn't a statement).
fn verify(who: &Who, body: &[u8]) -> R<Value> {
    ci_gate(concat!(cli_name!(), " compute verify"))?;
    let v = json_body(body)?;
    let posted: Option<Dsse> = match v.get("envelope") {
        Some(e) => Some(serde_json::from_value(e.clone()).map_err(|e| {
            no(codes::BAD_REQUEST, 400, format!("that isn't a build statement (one DSSE envelope: payloadType, payload, signatures): {e}"))
        })?),
        None => None,
    };
    let token = match (&posted, s(&v, "job")) {
        (Some(_), Some(_)) => return Err(no(codes::BAD_REQUEST, 400, "a job or a statement file, not both")),
        (Some(env), None) => {
            env.peek().map_err(|e| no(codes::BAD_REQUEST, 400, format!("that statement doesn't read: {e}")))?.predicate.build_definition.internal_parameters.job_id
        }
        (None, Some(t)) => t.to_string(),
        (None, None) => return Err(no(codes::BAD_REQUEST, 400, concat!("name a job or a statement file: ", cli_name!(), " compute verify <job> | <file.intoto.jsonl>"))),
    };
    // The job the statement names, looked up here only to find its receipt
    // and node; nothing is taken on the statement's word.
    let j = job_for(who, Some(&token)).map_err(|mut e| {
        if posted.is_some() {
            e.hint = format!("{}: the statement names job {token}, and {CLI_NAME} compute verify checks a statement against this server's own records (its receipt and node key)", e.hint);
        }
        e
    })?;
    let short = compute::message::short(&j.id);
    let env = match posted {
        Some(e) => e,
        None => stored(&j).map_err(|why| no(refusals::NO_STATEMENT, 409, format!("job {short} has no build statement: {why}")))?,
    };
    let pin = s(&v, "ownerKey").map(str::to_ascii_lowercase);
    if pin.as_deref().is_some_and(|p| !k2_core::compute::proto::ci::is_fp(p)) {
        return Err(no(codes::BAD_REQUEST, 400, concat!("--owner-key is the owner's roster key fingerprint: 64 hex (", cli_name!(), " compute owners shows it)")));
    }
    let want_sha = s(&v, "sha256").map(str::to_ascii_lowercase);
    let file = s(&v, "file").unwrap_or("the file").to_string();
    let (node, receipt) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::node_by_id(&conn, &j.node_id), j.receipt_json.clone())
    };
    let mut c = Checks { done: Vec::new(), failed: None };
    let st = run_checks(&mut c, &j, node.as_ref(), receipt.as_deref(), &env, want_sha.as_deref(), &file, pin.as_deref());
    let mut out = json!({
        "job": j.id,
        "short": short,
        "node": node.as_ref().map(|n| n.name.clone()),
        "verified": c.failed.is_none(),
        "failed": c.failed,
        "checks": c.done,
        "notChecked": "whether the node is honest, and whether this server's records are: a release signer checks again with keys it pinned itself",
    });
    if let Some(st) = st {
        out["builderId"] = st.predicate.run_details.builder.id.clone().into();
        out["subjects"] = st.subject.iter().map(|s| json!({"name": s.name, "sha256": s.sha256()})).collect::<Vec<_>>().into();
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn run_checks(
    c: &mut Checks,
    j: &store::JobRow,
    node: Option<&store::NodeRow>,
    receipt: Option<&str>,
    env: &Dsse,
    want_sha: Option<&str>,
    file: &str,
    pin: Option<&str>,
) -> Option<Statement> {
    // 1. The key.
    let Some(node) = node else {
        c.fail("node key", "the job's node is no longer on this server, so there is no pinned key to check with".into());
        return None;
    };
    let Ok(spki) = crypto::pem_decode("PUBLIC KEY", &node.public_key_pem) else {
        c.fail("node key", format!("this server's key for {} doesn't read", node.name));
        return None;
    };
    let keyid = env.keyid().unwrap_or("");
    if keyid != node.fingerprint {
        c.fail(
            "node key",
            format!("signed by key {}, but the key this server pinned for {} is {} (re-keyed since, or another node's statement)", short_fp(keyid), node.name, short_fp(&node.fingerprint)),
        );
        return None;
    }
    c.pass("node key", format!("signed by {} (key {}), the key this server pinned when the node enrolled", node.name, short_fp(keyid)));
    // 2. The receipt.
    let Some(signed) = receipt.and_then(|r| serde_json::from_str::<SignedReceipt>(r).ok()) else {
        c.fail("receipt", "the job has no receipt here yet".into());
        return None;
    };
    let Some(rc) = signed.verify(&spki) else {
        c.fail("receipt", "the job's receipt doesn't verify with that key".into());
        return None;
    };
    c.pass("receipt", format!("the job's receipt (v{}) verifies with the same key", rc.v));
    // 3. The statement and its receipt.
    let st = match env.open_for_receipt(&spki, &signed) {
        Ok(st) => st,
        Err(e) => {
            c.fail("statement", e);
            return None;
        }
    };
    c.pass(
        "statement",
        format!(
            "the signature verifies; the format is in-toto v1 with SLSA provenance v1; it names this receipt and agrees with it ({} file(s) and their sha256, commit, command, times)",
            st.subject.len()
        ),
    );
    // 4. BA-39.
    match secret_names(&j.workspace_id, &st) {
        Ok(n) if n.is_empty() => c.pass("plain env", "no plain env name is a secret's name in the vault".into()),
        Ok(n) => c.fail("plain env", secret_in_env(&n)),
        Err(e) => c.fail("plain env", e),
    }
    if c.failed.is_some() {
        return Some(st);
    }
    // 5. The file.
    if let Some(sha) = want_sha {
        match st.subject.iter().position(|s| s.sha256() == Some(sha)) {
            Some(i) => c.pass("file", format!("{file} (sha256 {}…) is subject {} of {}: {}", &sha[..sha.len().min(12)], i + 1, st.subject.len(), st.subject[i].name.as_deref().unwrap_or("?"))),
            None => {
                c.fail("file", format!("{file} (sha256 {}…) is none of the statement's {} subject(s)", &sha[..sha.len().min(12)], st.subject.len()));
                return Some(st);
            }
        }
    }
    // 6. The owner.
    owner_check(c, &st, &rc, pin);
    Some(st)
}

/// `builder.id` against the rosters this server holds. With `pin` (an
/// owner's roster key): the statement must name that key, never the
/// owner-server form, and the node key must be active in that owner's
/// roster held here. Without: a held roster that voids the key fails; a
/// roster not held is said and not checked.
fn owner_check(c: &mut Checks, st: &Statement, rc: &Receipt, pin: Option<&str>) {
    if let Some(p) = pin {
        if let Err(e) = st.check_builder_owner(p) {
            c.fail("owner", e.trim_start_matches("provenance: ").to_string());
            return;
        }
    }
    let fp = match st.builder_owner() {
        Some(Owner::RosterKey(fp)) => fp,
        Some(Owner::Server(fp)) => {
            c.pass("owner", format!("not checked: the node held no roster from its owner when it signed (builder.id names owner server {}), so nothing ties it to an owner's roster key", short_fp(&fp)));
            return;
        }
        None => {
            c.fail("owner", "builder.id is not an owner's".into());
            return;
        }
    };
    let held = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let void = trust::receipt_void(&conn, &rc.node_fp, rc.started_at);
        (trust::latest_roster(&conn, &fp), void)
    };
    let (roster, void) = held;
    if let Some(why) = void {
        c.fail("owner", why);
        return;
    }
    let Some(r) = roster else {
        let msg = format!("builder.id names roster key {}, and this server holds no roster under that key", short_fp(&fp));
        if pin.is_some() {
            c.fail("owner", msg);
        } else {
            c.pass("owner", format!("not checked: {msg}"));
        }
        return;
    };
    let started = rc.started_at.unwrap_or(rc.ended_at);
    let what = format!("roster #{} from {} (key {})", r.roster.seq, r.roster.owner_id, short_fp(&fp));
    let Some(e) = r.roster.entry(&rc.node_fp) else {
        c.fail("owner", format!("{what} doesn't list the node's key"));
        return;
    };
    let ok = match e.status {
        KeyStatus::Active => e.valid_from <= started,
        KeyStatus::Retired => e.valid_from <= started && e.valid_to.is_some_and(|t| started <= t),
        KeyStatus::Revoked => false,
    };
    let status = match e.status {
        KeyStatus::Active => "active",
        KeyStatus::Retired => "retired",
        KeyStatus::Revoked => "revoked",
    };
    if ok {
        c.pass("owner", format!("{what} lists {} (key epoch {}) as {status} when the job started", e.name, e.key_epoch));
    } else {
        c.fail("owner", format!("{what} lists {}'s key as {status}, which doesn't cover a job that started then", e.name));
    }
}

// ── k2 compute receipt ───────────────────────────────────────────────

fn job_receipt(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    let j = job_for(who, params.get("job").map(String::as_str))?;
    let Some(raw) = j.receipt_json.as_deref() else {
        return Err(no("no_receipt", 404, "no receipt yet (the node sends it when the job ends)"));
    };
    let signed: SignedReceipt = serde_json::from_str(raw).map_err(|e| no("internal", 500, format!("stored receipt: {e}")))?;
    let body: Value = serde_json::from_str(&signed.body).unwrap_or_default();
    // The node's public key as this server pinned it at enrollment. Display
    // only: whoever checks the receipt elsewhere (the release signer, S3b)
    // pins node keys itself and never trusts this field.
    let node_key = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id).map(|n| n.public_key_pem)
    };
    let mut out = json!({
        "job": compute::message::short(&j.id),
        "verified": j.receipt_ok == Some(true),
        "receipt": body,
        // The exact bytes the node signed (canonical JSON), so a checker
        // never has to re-serialize `receipt` to verify `signature`.
        "body": signed.body,
        "signature": signed.sig,
        "node_public_key_pem": node_key,
        // The plan the node ran (its canonical JSON hashes to the receipt's
        // `plan_digest`), so a checker sees every input the receipt only
        // digests: --with, cwd, --cache, needs, env. Env VALUES are in it
        // (as in every plan); vault secrets never are (names only).
        "plan": j.plan().and_then(|p| serde_json::to_value::<Plan>(p).ok()),
        "proves": "which node said it ran which inputs and got which outputs — not that the node is honest",
    });
    // SE8/SE29: the env names the node says the job got from the vault.
    if let Some(s) = body.get("secret_env").filter(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
        out["secrets"] = s.clone();
    }
    // Protocol 9, receipt v2: the receipt's version, and the node's signed
    // self-statement it names (`run.statement_digest`), exactly as the node
    // signed it, for the evidence bundle (`docs/ci-formats-v1.md` §2, §8).
    // A v1 receipt's answer is unchanged (S3b reads it).
    if body.get("v").and_then(|v| v.as_u64()) == Some(u64::from(RECEIPT_V2)) {
        out["v"] = json!(RECEIPT_V2);
        let stored: Value = serde_json::from_str(raw).unwrap_or_default();
        out["statement"] = stored.get("statement").cloned().unwrap_or(Value::Null);
    }
    // C1: the node's owner revoked this key in its roster: the receipt is
    // void (compromised: every receipt, past and future). Absent otherwise,
    // so a v1 answer is unchanged.
    if let Some(fp) = body.get("node_fp").and_then(|v| v.as_str()) {
        let started = body.get("started_at").and_then(|v| v.as_i64());
        if let Some(why) = crate::compute_trust::receipt_void(fp, started) {
            out["voided"] = why.into();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_asks_only_a_provenance_node_with_the_ci_preview_on() {
        let with = vec!["artifacts".to_string(), prov::FEATURE_PROVENANCE_V1.to_string()];
        let without = vec!["artifacts".to_string(), k2_core::compute::proto::gens::FEATURE_RECEIPT_V3.to_string()];
        assert_eq!(ask_for(&with, true), Some(prov::PROVENANCE_V1));
        assert_eq!(ask_for(&with, false), None, "dark while the CI preview is off");
        assert_eq!(ask_for(&without, true), None);
        assert_eq!(ask_for(&[], true), None);
    }

    #[test]
    fn kept_goes_into_the_stored_receipt_under_its_own_key() {
        let mut v = json!({"body": "{}", "sig": "x"});
        keep(&mut v, &Kept::Nothing);
        assert_eq!(v, json!({"body": "{}", "sig": "x"}), "nothing added when none came");
        keep(&mut v, &Kept::Refused("bad".into()));
        assert_eq!(v[KEY_REFUSED], "bad");
        let env = Dsse { payload_type: prov::PAYLOAD_TYPE.into(), payload: "e30=".into(), signatures: vec![] };
        let mut v = json!({"body": "{}", "sig": "x"});
        keep(&mut v, &Kept::Statement(Box::new(env.clone())));
        assert_eq!(serde_json::from_value::<Dsse>(v[KEY].clone()).unwrap(), env);
        // Readers of the plain signed receipt ignore the extra keys.
        assert!(serde_json::from_value::<SignedReceipt>(v).is_ok());
    }

    #[test]
    fn ba39_names_the_clash_plainly() {
        let one = secret_in_env(&["API_TOKEN".into()]);
        assert!(one.starts_with("plain env API_TOKEN is also a secret's name") && one.contains("--secret NAME"), "{one}");
        assert!(secret_in_env(&["A".into(), "B".into()]).starts_with("plain env A, B are also"));
    }
}
