//! PSD/PSB files and comments end to end through the vendored kit and K2's
//! own pieces: K2's rights mapping (`DocRights::kit`), K2's event arms
//! (`K2Host::on_event`), the indexer queue, the `/cli/docs/tool` answer
//! shapes and the error mapping. A temp store, no daemon runtime.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::IndexStatus;
use k2_mpc_base::session::{
    AuditEvent, BlobSession, Config, ConnId, Host, Need, Participant, ParticipantKind, Rights, SessionError, SessionManager,
    Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::ToolOptions;

use super::host::{DocRights, K2Host};
use super::routes::{self, VerbError};
use super::tier0;

#[path = "../../../../vendor/k2-mpc-psd/crates/psd/tests/support/mod.rs"]
mod psd;
use psd::{psd_file, Lay};

/// Rights by pid through K2's own mapping, and K2's event arms for the
/// structure index, comments and node claims.
struct TestHost {
    rights: Mutex<HashMap<String, DocRights>>,
    events: Mutex<Vec<&'static str>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights.lock().expect("rights").get(&who.pid).copied().unwrap_or(DocRights::NONE).kit()
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, ev: &AuditEvent) {
        let name = match ev {
            AuditEvent::IndexWanted { .. } => "index_wanted",
            AuditEvent::IndexReady { .. } => "index_ready",
            AuditEvent::IndexNotIndexed { .. } => "index_not_indexed",
            AuditEvent::Annotation { .. } => "annotation",
            AuditEvent::AnchorState { .. } => "anchor_state",
            AuditEvent::ClaimTaken { .. } => "claim_taken",
            AuditEvent::ClaimEnded { .. } => "claim_ended",
            _ => return,
        };
        self.events.lock().expect("events").push(name);
        // K2's real arms, called as the kit calls them: with the session locked.
        K2Host::default().on_event(ev);
    }
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
    key: DocKey,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("k2-blobflow-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let host = Arc::new(TestHost { rights: Mutex::new(HashMap::new()), events: Mutex::new(Vec::new()) });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
    let blobs =
        Arc::new(BlobStore::open(store.clone(), dir.join("blobs"), BlobConfig::default(), Arc::new(|| 1_000_000)).expect("blobs"));
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    let key = DocKey { space: format!("ws-{tag}-{}", N.fetch_add(1, Ordering::SeqCst)), path: "art/poster.psd".into() };
    Fx { dir, host, mgr, key }
}

impl Fx {
    fn open(&self, bytes: &[u8]) -> Arc<BlobSession> {
        self.mgr
            .open_blob(super::blob_packs::pack_for(&self.key.path), &self.key, Some(&mut &bytes[..]))
            .expect("open the PSD doc")
    }
    fn join(&self, s: &BlobSession, pid: &str, kind: ParticipantKind, r: DocRights) -> ConnId {
        self.host.rights.lock().expect("rights").insert(pid.into(), r);
        s.join(Participant::new(pid, pid, "#123456", kind, "cli"), Stay::Held).expect("join").conn
    }
}

fn poster() -> Vec<u8> {
    psd_file(false, &[Lay::new("Background").id(1), Lay::new("Logo").id(7), Lay::new("Title").id(9)], &[], &[])
}

fn index_now(s: &BlobSession) {
    let st = s.index_job(None).expect("index_job").expect("a job is due").run().expect("the job records");
    assert!(matches!(st, IndexStatus::Ready { .. }), "{st:?}");
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn error_of(r: &crate::cli_response::CliResponse) -> Value {
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body));
    assert_eq!(v["ok"], json!(false), "{}", r.body);
    v["error"].clone()
}

/// A refusal either way the kit gives it: a tool answer with the report
/// (`error`), or a session error mapped by `routes::session_error`. Returns
/// the error object (`id`, `exit`, and `hint` or `message`).
fn refusal_of(r: Result<Value, VerbError>) -> Value {
    match r {
        Ok(v) if v["error"].is_object() => {
            let mut e = v["error"].clone();
            e["hint"] = v["stderr"].clone();
            e
        }
        Ok(v) => panic!("answered: {v}"),
        Err(VerbError::Session(e)) => error_of(&routes::session_error(&e)),
        Err(VerbError::Response(resp)) => panic!("a K2 refusal, not the kit's: {} {}", resp.status, resp.body),
    }
}

fn session_err(r: Result<Value, VerbError>) -> SessionError {
    match r {
        Err(VerbError::Session(e)) => e,
        Err(VerbError::Response(resp)) => panic!("a K2 refusal, not the kit's: {} {}", resp.status, resp.body),
        Ok(v) => panic!("answered: {v}"),
    }
}

/// `IndexWanted` comes with the session locked: K2's arm only queues the
/// version for the indexer thread (no kit call, so the write returns; a kit
/// call there would deadlock). The job is not run on the writer's thread:
/// the version is still pending afterwards.
#[test]
fn index_wanted_is_only_queued_under_the_lock() {
    let f = fx("queue");
    let s = f.open(&poster());
    let events = f.host.events.lock().expect("events").clone();
    assert!(events.contains(&"index_wanted"), "the import asks for an index: {events:?}");
    let seen = super::indexer::SEEN.lock().expect("seen").clone();
    let v = s.status().expect("status").version;
    assert!(seen.contains(&(f.key.clone(), v)), "K2's arm queued {}@{v}: {seen:?}", f.key.path);
    assert!(
        matches!(s.index_status(None).expect("index status"), IndexStatus::Pending { .. }),
        "the job was not run under the lock (no runtime here: the indexer thread drops it)"
    );
}

/// `DocRights.comment` reaches the kit as `Rights::with_comment`: a
/// read-only share comments only when granted; edit implies it. The kit's
/// refusal is K2's `forbidden_comment`, exit 3, with the way on.
#[test]
fn a_read_only_share_comments_only_when_granted() {
    let f = fx("rights");
    let s = f.open(&poster());
    let key = f.key.clone();
    let ro = DocRights { read: true, ..DocRights::NONE };
    let reader = f.join(&s, "agent:reader", ParticipantKind::Agent, ro);
    let opts = ToolOptions::new();
    let comment = argv(&["comment", "--doc", "--body", "looks good"]);
    let e = session_err(tier0::run_blob_tool(&s, reader, &key, &comment, &opts));
    assert!(matches!(e, SessionError::Forbidden { need: Need::Comment }), "{e:?}");
    let r = routes::session_error(&e);
    assert!(r.status.starts_with("403"), "{}", r.status);
    let err = error_of(&r);
    assert_eq!(err["code"], json!("forbidden_comment"), "{err}");
    assert_eq!(err["id"], json!("MPC-E116"), "{err}");
    assert_eq!(err["exit"], json!(3), "{err}");
    assert!(err["hint"].as_str().expect("hint").contains("--comment"), "teaches the share grant: {err}");

    let granted = f.join(&s, "agent:granted", ParticipantKind::Agent, DocRights { comment: true, ..ro });
    let ok = tier0::run_blob_tool(&s, granted, &key, &comment, &opts).unwrap_or_else(|_| panic!("a granted read-only share comments"));
    assert_eq!(ok["exit"], json!(0), "{ok}");

    let editor = f.join(&s, "person:sam", ParticipantKind::Person, DocRights::FULL);
    let ok = tier0::run_blob_tool(&s, editor, &key, &comment, &opts).unwrap_or_else(|_| panic!("edit implies comment"));
    assert_eq!(ok["exit"], json!(0), "{ok}");
    assert!(f.host.events.lock().expect("events").contains(&"annotation"), "each comment reports an Annotation");
}

/// The PSD pack refuses agent writes (`MPC-E715`): K2 answers
/// `agent_write_refused`, exit 2, telling the agent to propose a new file
/// and a comment. A person's write works.
#[test]
fn an_agents_psd_write_is_refused_with_the_proposal_route() {
    let f = fx("agent");
    let s = f.open(&poster());
    let v = s.status().expect("status").version;
    let bot = f.join(&s, "agent:bot", ParticipantKind::Agent, DocRights::FULL);
    let e = s.write_bytes(bot, b"agent bytes", v).expect_err("an agent can't write a PSD");
    assert_eq!(e, SessionError::AgentWriteRefused);
    let r = routes::session_error(&e);
    assert!(r.status.starts_with("400"), "{}", r.status);
    let err = error_of(&r);
    assert_eq!(err["code"], json!("agent_write_refused"), "{err}");
    assert_eq!(err["id"], json!("MPC-E715"), "{err}");
    assert_eq!(err["exit"], json!(2), "{err}");
    let hint = err["hint"].as_str().expect("hint");
    assert!(hint.contains("new file") && hint.contains("--proposal"), "{hint}");
    let sam = f.join(&s, "person:sam", ParticipantKind::Person, DocRights::FULL);
    s.write_bytes(sam, &psd_file(false, &[Lay::new("Logo").id(7)], &[], &[]), v).expect("a person writes it");
}

/// The index verbs through `run_tool`: an outline, a node read whose
/// missing address exits 4 with the kit's refusal (`MPC-E172`) and the rest
/// still listed, a bad cursor (`MPC-E174`), an ambiguous name (`MPC-E170`,
/// candidates in the hint), and claims by node with the roster's labels.
#[test]
fn index_reads_and_node_claims_answer_as_the_kit_says() {
    let f = fx("reads");
    let s = f.open(&psd_file(false, &[Lay::new("Logo").id(7), Lay::new("Dup"), Lay::new("Dup")], &[], &[]));
    index_now(&s);
    assert!(f.host.events.lock().expect("events").contains(&"index_ready"));
    let key = f.key.clone();
    let me = f.join(&s, "agent:me", ParticipantKind::Agent, DocRights::FULL);
    let opts = ToolOptions::new();

    let out = tier0::run_blob_tool(&s, me, &key, &argv(&["outline"]), &opts).unwrap_or_else(|_| panic!("outline"));
    assert_eq!(out["exit"], json!(0), "{out}");
    assert!(out["stdout"].as_str().expect("stdout").contains("Logo"), "{out}");
    assert!(out["fence"]["begin"].as_str().is_some(), "names come from the file: fenced");

    let js = tier0::run_blob_tool(&s, me, &key, &argv(&["layer", "list"]), &ToolOptions::new().json(true))
        .unwrap_or_else(|_| panic!("layer list --json"));
    let page: Value = serde_json::from_str(js["stdout"].as_str().expect("stdout")).expect("the page is JSON");
    assert!(page.is_object(), "{page}");

    let miss = tier0::run_blob_tool(&s, me, &key, &argv(&["node", "@lyid:7", "Nope"]), &opts).unwrap_or_else(|_| panic!("node"));
    assert_eq!(miss["exit"], json!(4), "{miss}");
    assert_eq!(miss["refusal"]["id"], json!("MPC-E172"), "{miss}");
    assert!(miss["stderr"].as_str().expect("stderr").contains("MPC-E172"), "{miss}");
    assert!(miss["stdout"].as_str().expect("stdout").contains("Logo"), "the found one is still listed: {miss}");

    let err = refusal_of(tier0::run_blob_tool(&s, me, &key, &argv(&["outline", "--cursor", "junk"]), &opts));
    assert_eq!(err["id"], json!("MPC-E174"), "{err}");
    assert_eq!(err["exit"], json!(2), "{err}");

    let err = refusal_of(tier0::run_blob_tool(&s, me, &key, &argv(&["node", "Dup"]), &opts));
    assert_eq!(err["id"], json!("MPC-E170"), "{err}");
    assert_eq!(err["details"]["candidates"].as_array().map(Vec::len), Some(2), "both holders are candidates: {err}");

    let claimed = tier0::run_blob_tool(&s, me, &key, &argv(&["claim", "--node", "@lyid:7", "--note", "recolour"]), &opts)
        .unwrap_or_else(|_| panic!("claim"));
    assert_eq!(claimed["exit"], json!(0), "{claimed}");
    assert!(f.host.events.lock().expect("events").contains(&"claim_taken"), "a node claim reports ClaimTaken");
    let other = f.join(&s, "agent:other", ParticipantKind::Agent, DocRights::FULL);
    let err = refusal_of(tier0::run_blob_tool(&s, other, &key, &argv(&["claim", "--node", "@lyid:7"]), &opts));
    assert_eq!(err["exit"], json!(5), "someone else's claim on that layer: {err}");
    let roster = s.roster(me).expect("roster");
    let mine = roster.iter().find(|r| r.participant.pid == "agent:me").expect("me in the roster");
    assert_eq!(mine.claims.len(), 1, "RosterEntry::claims lists the node claim");
    let wire = routes::roster_json(mine);
    assert_eq!(wire["claims"][0]["nodes"], json!(["@lyid:7"]), "the roster's claim names its node keys: {wire}");
    let labels = s.node_labels(me, &[k2_mpc_base::index::NodeKey::parse("@lyid:7").expect("key")]).expect("labels");
    assert_eq!(labels[0].as_ref().map(|l| l.name.as_str()), Some("Logo"));
    let released = tier0::run_blob_tool(&s, me, &key, &argv(&["claim", "--release"]), &opts).unwrap_or_else(|_| panic!("release"));
    assert_eq!(released["exit"], json!(0), "{released}");
    assert!(f.host.events.lock().expect("events").contains(&"claim_ended"));
}

/// Every code the build list names maps to the kit's id, name and exit, a
/// hint with what to do next (`docs/ERRORS.md`), and never the old 500
/// `docs_failed` catch-all.
#[test]
fn every_new_kit_refusal_maps_to_its_id_name_and_exit() {
    let t = k2_mpc_base::annot::ThreadId::parse("ann-3").expect("thread id");
    let cases: Vec<(SessionError, &str, &str, u8, &str)> = vec![
        (SessionError::BadParticipant("pid too long".into()), "MPC-E115", "bad_participant", 2, "400"),
        (SessionError::Forbidden { need: Need::Comment }, "MPC-E116", "forbidden_comment", 3, "403"),
        (SessionError::NotCommentAuthor { thread: t, comment: 2 }, "MPC-E117", "forbidden_comment_author", 3, "403"),
        (SessionError::NoAnnotation { thread: t, comment: None }, "MPC-E320", "no_annotation", 4, "404"),
        (SessionError::BadAnnotation("an empty body".into()), "MPC-E321", "bad_annotation", 2, "400"),
        (SessionError::AnnotationCap { what: "comments", limit: 1000 }, "MPC-E907", "annotation_cap", 2, "400"),
        (SessionError::IndexUnsupported, "MPC-E711", "index_unsupported", 2, "400"),
        (
            SessionError::NotIndexed { version: 4, why: k2_mpc_base::index::NotIndexedWhy::Unsupported },
            "MPC-E712",
            "not_indexed",
            4,
            "404",
        ),
        (SessionError::IndexPending { version: 4, retry_after_ms: 500 }, "MPC-E713", "index_pending", 1, "409"),
        (SessionError::AgentWriteRefused, "MPC-E715", "agent_write_refused", 2, "400"),
    ];
    for (e, id, code, exit, status) in cases {
        let r = routes::session_error(&e);
        let err = error_of(&r);
        assert_eq!(err["exit"], json!(exit), "{e:?}: {err}");
        assert!(r.status.starts_with(status), "{e:?}: {}", r.status);
        assert_eq!(err["code"], json!(code), "{e:?}: {err}");
        assert_ne!(err["code"], json!("docs_failed"), "{e:?}");
        assert_eq!(err["id"], json!(id), "{e:?}: {err}");
        assert!(err["hint"].as_str().is_some_and(|h| h.len() > 20), "{e:?}: {err}");
    }
    let pending = error_of(&routes::session_error(&SessionError::IndexPending { version: 4, retry_after_ms: 500 }));
    assert_eq!(pending["retryAfterMs"], json!(500), "{pending}");
    assert_eq!(pending["nextStep"]["verb"], json!("status"), "the kit's next step, as data: {pending}");
    // The kit's export ids: E716 is a 422, exit 4.
    let r = super::export::export_error(&k2_mpc_base::export::ExportError::NeedsRenderer { kind: "text".into() });
    assert!(r.status.starts_with("422"), "{}", r.status);
    let err = error_of(&r);
    assert_eq!(err["id"], json!("MPC-E716"), "{err}");
    assert_eq!(err["exit"], json!(4), "{err}");
    // E714 / E717 are the indexer's (not a verb's): their ids, for the log.
    use k2_mpc_base::errors::Refusal;
    assert_eq!(k2_mpc_base::index::IndexJobError::BadResult("x".into()).report().id(), "MPC-E714");
    assert_eq!(k2_mpc_base::index::IndexJobError::Stale { version: 1, reason: "purged" }.report().id(), "MPC-E717");
}

/// Purge and erasure are the doc owner's: K2 refuses an agent and a
/// non-owner before the kit, and the kit refuses a non-owner connection too.
#[test]
fn purge_and_erasure_are_owner_only() {
    let f = fx("purge");
    let s = f.open(&poster());
    let key = f.key.clone();
    let sam = f.join(&s, "person:sam", ParticipantKind::Person, DocRights::FULL);
    tier0::run_blob_tool(&s, sam, &key, &argv(&["comment", "--doc", "--body", "remove me later"]), &ToolOptions::new())
        .unwrap_or_else(|_| panic!("comment"));
    let e = s.purge_annotations(sam, k2_mpc_base::annot::AnnotPurge::All).expect_err("an editor is not the owner");
    let err = error_of(&routes::session_error(&e));
    assert_eq!(err["code"], json!("not_the_owner"), "{err}");
    let e = s.erase_participant(sam, "person:x", "Former member").expect_err("an editor can't erase");
    assert!(matches!(e, SessionError::Forbidden { need: Need::Owner }), "{e:?}");

    let target = super::target::Target {
        project_id: key.space.clone(),
        root: f.dir.clone(),
        rel: key.path.clone(),
        abs: f.dir.join(&key.path),
        tier: super::target::Tier::Blob,
    };
    let params = HashMap::new();
    let input = routes::Input { params: &params, body: json!({ "all": true }) };
    let agent = super::caller::DocCaller::Agent { project_id: key.space.clone() };
    let person = super::caller::DocCaller::Person { key: "sam".into(), via: "cli" };
    let owner = super::caller::owner();
    for (caller, rights, ok) in [(&agent, DocRights::OWNER, false), (&person, DocRights::FULL, false), (&owner, DocRights::OWNER, true)] {
        let c = routes::Call { caller, on_behalf: None, target: &target, input: &input, rights };
        match super::annotations::owner_only(&c) {
            Ok(()) => assert!(ok, "{caller:?} passed"),
            Err(r) => {
                assert!(!ok, "{caller:?} refused");
                assert!(r.status.starts_with("403"), "{}", r.status);
                assert_eq!(error_of(&r)["exit"], json!(3));
            }
        }
    }

    let boss = f.join(&s, "person:owner", ParticipantKind::Person, DocRights::OWNER);
    let o = s.purge_annotations(boss, k2_mpc_base::annot::AnnotPurge::All).expect("the owner purges");
    let v = super::annotations::outcome_json(&o);
    assert_eq!(v["counts"]["comments"], json!(1), "{v}");
    assert_eq!(v["storeRewritten"], json!(true), "{v}");
    assert!(
        v["notReached"].as_array().expect("not reached").iter().any(|n| n["what"] == json!("SentNotifications") && n["k2"].is_string()),
        "the mention notes K2 sent are named: {v}"
    );
}

/// The two new routes are POST-only behind the guard, and a GET is 405.
#[test]
fn the_purge_and_erase_routes_are_post_only() {
    let caller = super::caller::owner();
    let params = HashMap::new();
    for r in [super::annotations::PURGE, super::annotations::ERASE] {
        assert!(routes::ROUTES.contains(&r), "{r} is a docs route");
        let resp = routes::handle("GET", r, &params, b"", &caller);
        assert!(resp.status.starts_with("405"), "GET {r}: {}", resp.status);
    }
    // Comments and claims ride /cli/docs/tool, POST-only too.
    assert!(routes::handle("GET", routes::TOOL, &params, b"", &caller).status.starts_with("405"));
}
