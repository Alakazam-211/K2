//! The parity harness for comment threads (K-PAR-3, slice A4): every annotation action a
//! person takes in a tab (the [`BlobSession`] calls directly) has an agent verb
//! ([`super::ANNOTATION_VERBS`], run by [`BlobSession::run_annotation`]) with the same effect.
//! Each case runs both paths on two identical blob docs and compares the threads, their
//! comments, the history (who did what, never a body), the exit code and the output.
//!
//! **The one documented difference** (K-ANN-9): an agent's `reanchor` is a proposal. Its
//! case confirms it as the same person afterwards ([`AnnotCase::confirm`]), and the
//! comparison leaves out the proposal's own history row; everything else must match.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use super::harness::{Failure, ParityHost, Side, TmpDir, ACTOR, AGENT_VIA, OBSERVER, PERSON_VIA};
use super::registry::{verb_form, Registry, Tier};
use crate::anchor::AnchorTarget;
use crate::annot::{
    AnnotCall, AnnotOp, AnnotOutput, Attachment, NewAnchor, NewComment, Reanchored, Thread,
    ThreadId, ThreadQuery, ThreadStatus,
};
use crate::blob::{BlobConfig, BlobStore};
use crate::session::{
    BlobPack, BlobSession, Config, ConnId, Participant, ParticipantKind, Rights, SessionError,
    SessionManager, Stay,
};
use crate::store::{DocKey, DocStore};
use crate::tools;

/// The person path: the actor's typed call.
pub type AnnotPersonFn = fn(&AnnotWorld, ConnId) -> Result<AnnotOutput, SessionError>;
/// The agent path: the verb's argv.
pub type AnnotArgvFn = fn(&AnnotWorld) -> Vec<String>;
/// Others' activity first.
pub type AnnotSetupFn = fn(&mut AnnotWorld) -> Result<(), String>;

/// One annotation parity check.
#[derive(Clone, Copy)]
pub struct AnnotCase {
    pub name: &'static str,
    /// The registry form under test.
    pub verb: &'static str,
    /// The actor's rights (both paths).
    pub rights: Rights,
    /// Needs a node (the world's `node`): skipped on a type without an index.
    pub needs_node: bool,
    /// An agent's call is a proposal: the same person confirms it after the agent path.
    pub confirm: bool,
    pub setup: AnnotSetupFn,
    pub person: AnnotPersonFn,
    pub agent: AnnotArgvFn,
}

/// One world: the blob doc, an observer (full rights and owner), the threads setup made.
pub struct AnnotWorld {
    _mgr: SessionManager,
    host: Arc<ParityHost>,
    session: Arc<BlobSession>,
    observer: ConnId,
    /// The node a node case anchors to.
    pub node: Option<String>,
    /// Threads setup made, by name.
    pub marks: HashMap<&'static str, ThreadId>,
    // last: dropped after the stores
    _dir: TmpDir,
}

fn argv(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| (*x).to_owned()).collect()
}

impl AnnotWorld {
    fn new(pack: Arc<dyn BlobPack>, initial: &[u8], node: Option<&str>) -> Result<Self, String> {
        let dir = TmpDir::new()?;
        let host = Arc::new(ParityHost {
            now: AtomicU64::new(1_700_000_000_000),
            rights: Mutex::new(HashMap::new()),
        });
        let store =
            Arc::new(DocStore::open(dir.0.join("docs.redb")).map_err(|e| format!("store: {e}"))?);
        let h = host.clone();
        let blobs = BlobStore::open(
            store.clone(),
            dir.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now.load(std::sync::atomic::Ordering::SeqCst)),
        )
        .map_err(|e| format!("blob store: {e}"))?;
        let mgr = SessionManager::new(store, host.clone(), Config::default())
            .with_blob_store(Arc::new(blobs));
        let key = DocKey {
            space: "ws".into(),
            path: "design.bin".into(),
        };
        let session = mgr
            .open_blob(pack, &key, Some(&mut &initial[..]))
            .map_err(|e| format!("open_blob: {e}"))?;
        if let Some(job) = session.index_job(None).map_err(|e| e.to_string())? {
            job.run().map_err(|e| format!("index: {e}"))?;
        }
        let mut w = AnnotWorld {
            _mgr: mgr,
            host,
            session,
            observer: ConnId::fresh(),
            node: node.map(str::to_owned),
            marks: HashMap::new(),
            _dir: dir,
        };
        w.grant(OBSERVER, Rights::full().with_owner(true));
        w.observer = w
            .session
            .join(
                Participant::new(
                    OBSERVER,
                    "Observer",
                    "#888888",
                    ParticipantKind::Person,
                    PERSON_VIA,
                ),
                Stay::Held,
            )
            .map_err(|e| format!("observer join: {e}"))?
            .conn;
        Ok(w)
    }

    pub fn session(&self) -> &Arc<BlobSession> {
        &self.session
    }

    /// The observer's connection (full rights, owner): for setup.
    pub fn observer(&self) -> ConnId {
        self.observer
    }

    pub fn grant(&self, pid: &str, r: Rights) {
        if let Ok(mut m) = self.host.rights.lock() {
            m.insert(pid.to_owned(), r);
        }
    }

    /// A setup thread mark.
    pub fn mark(&self, name: &str) -> ThreadId {
        self.marks.get(name).copied().unwrap_or(ThreadId(0))
    }

    /// `ann-N` of a mark.
    pub fn mark_arg(&self, name: &str) -> String {
        self.mark(name).to_string()
    }

    /// A thread on the doc by `pid` (a person, full rights), as setup.
    pub fn thread_by(&mut self, mark: &'static str, pid: &str, body: &str) -> Result<(), String> {
        self.grant(pid, Rights::full());
        let p = Participant::new(pid, pid, "#555555", ParticipantKind::Person, PERSON_VIA);
        let c = self
            .session
            .join(p, Stay::UntilIdle)
            .map_err(|e| e.to_string())?
            .conn;
        let t = self
            .session
            .comment(c, NewComment::new(NewAnchor::Doc, body))
            .map_err(|e| e.to_string())?;
        self.session.leave(c).map_err(|e| e.to_string())?;
        self.marks.insert(mark, t.id);
        Ok(())
    }

    fn join_actor(&self, kind: ParticipantKind, via: &str) -> Result<ConnId, SessionError> {
        Ok(self
            .session
            .join(
                Participant::new(ACTOR, "Actor", "#2a9d8f", kind, via),
                Stay::Held,
            )?
            .conn)
    }

    /// What both worlds must agree on, as lines: every thread (where it points or, for a
    /// proposal, will point; status; its state; every comment), then the history. Never
    /// `via` or a participant's kind (the surfaces differ by design).
    fn observe(&self, drop_proposals: bool) -> Result<Vec<String>, String> {
        let s = &self.session;
        let page = s
            .comments(
                self.observer,
                &ThreadQuery::new().limit(crate::annot::ANNOT_PAGE_MAX),
            )
            .map_err(|e| format!("observe: {e}"))?;
        let mut out = Vec::new();
        for t in &page.threads {
            out.extend(thread_lines(t));
        }
        let log = s
            .annotation_log(self.observer, None, 0, usize::MAX)
            .map_err(|e| format!("observe log: {e}"))?;
        for e in log {
            if drop_proposals && e.op == AnnotOp::ReanchorProposed {
                continue;
            }
            out.push(format!(
                "log {} {} c{:?} by {} for {:?} to {:?} v{:?}",
                e.thread,
                e.op.name(),
                e.comment,
                e.by.pid,
                e.by.on_behalf_of,
                e.to.as_ref().map(target_word),
                e.version
            ));
        }
        Ok(out)
    }
}

fn target_word(t: &AnchorTarget) -> String {
    match t {
        AnchorTarget::Doc => "doc".to_owned(),
        AnchorTarget::Node { key } => key.to_string(),
    }
}

fn thread_lines(t: &Thread) -> Vec<String> {
    let status = match &t.status {
        ThreadStatus::Open => "open".to_owned(),
        ThreadStatus::Resolved { by } => format!("resolved by {}", by.pid),
    };
    let dest = match &t.pending_anchor {
        Some(p) => target_word(&p.to),
        None => target_word(&t.anchor.target),
    };
    let state = t
        .resolution
        .as_ref()
        .map(|r| format!("{}{:?}", r.state.name(), r.state.doubt()));
    let mut out = vec![format!(
        "{} -> {dest} {status} {state:?} by {}",
        t.id, t.created.pid
    )];
    for c in &t.comments {
        out.push(format!(
            "  c{} by {} ({}) for {:?}: {:?} mentions {:?} attachments {:?} deleted by {:?}",
            c.n,
            c.by.pid,
            c.by.name,
            c.by.on_behalf_of,
            c.body,
            c.mentions,
            c.attachments.iter().map(|a| &a.target).collect::<Vec<_>>(),
            c.deleted.as_ref().map(|d| &d.pid)
        ));
    }
    out
}

/// An output, as compared: the thread (or page, history, re-anchor) without its surface.
fn output_lines(o: &AnnotOutput) -> Vec<String> {
    match o {
        AnnotOutput::Thread(t) => thread_lines(t),
        AnnotOutput::Page(p) => {
            let mut v = vec![format!("page next {:?}", p.next)];
            for t in &p.threads {
                v.extend(thread_lines(t));
            }
            v
        }
        AnnotOutput::Log(l) => l
            .iter()
            .map(|e| {
                format!(
                    "log {} {} {:?} by {}",
                    e.thread,
                    e.op.name(),
                    e.comment,
                    e.by.pid
                )
            })
            .collect(),
        AnnotOutput::Reanchored(Reanchored::Applied(t) | Reanchored::Proposed(t)) => {
            thread_lines(t)
        }
    }
}

fn no_setup(_: &mut AnnotWorld) -> Result<(), String> {
    Ok(())
}

fn one_thread(w: &mut AnnotWorld) -> Result<(), String> {
    w.thread_by("t", OBSERVER, "Check the margins")
}

fn resolved_thread(w: &mut AnnotWorld) -> Result<(), String> {
    w.thread_by("t", OBSERVER, "Check the margins")?;
    w.session
        .resolve(w.observer, w.mark("t"))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn own_thread(w: &mut AnnotWorld) -> Result<(), String> {
    w.thread_by("t", ACTOR, "My own note")
}

fn two_threads(w: &mut AnnotWorld) -> Result<(), String> {
    w.thread_by("a", OBSERVER, "First")?;
    w.thread_by("b", OBSERVER, "Second")?;
    w.session
        .resolve(w.observer, w.mark("b"))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The kit's annotation cases. Node cases need the world's node (a pack with an index).
pub fn kit_annotation_cases() -> Vec<AnnotCase> {
    let base = AnnotCase {
        name: "",
        verb: "",
        rights: Rights::editor(),
        needs_node: false,
        confirm: false,
        setup: no_setup,
        person: |_, _| Err(SessionError::Unloaded),
        agent: |_| Vec::new(),
    };
    vec![
        AnnotCase {
            name: "comment on the whole doc, mentioning someone, with a proposal",
            verb: "comment",
            person: |w, c| {
                w.session
                    .comment(
                        c,
                        NewComment::new(NewAnchor::Doc, "Swap the logo")
                            .with_mentions(["bob"])
                            .with_attachment(Attachment::proposal("out/logo-v2.bin")),
                    )
                    .map(AnnotOutput::Thread)
            },
            agent: |_| {
                argv(&[
                    "comment",
                    "--doc",
                    "--body",
                    "Swap the logo",
                    "--mention",
                    "bob",
                    "--proposal",
                    "out/logo-v2.bin",
                ])
            },
            ..base
        },
        AnnotCase {
            name: "comment on a node",
            verb: "comment",
            needs_node: true,
            person: |w, c| {
                let k = w.node.clone().unwrap_or_default();
                w.session
                    .comment(c, NewComment::new(NewAnchor::node(k, None), "Too dark"))
                    .map(AnnotOutput::Thread)
            },
            agent: |w| {
                let k = w.node.clone().unwrap_or_default();
                argv(&["comment", "--node", &k, "--body", "Too dark"])
            },
            ..base
        },
        AnnotCase {
            name: "comment on a read-only share granted the comment right",
            verb: "comment",
            rights: Rights::read_only().with_comment(true),
            person: |w, c| {
                w.session
                    .comment(c, NewComment::new(NewAnchor::Doc, "Looks good"))
                    .map(AnnotOutput::Thread)
            },
            agent: |_| argv(&["comment", "--doc", "--body", "Looks good"]),
            ..base
        },
        AnnotCase {
            name: "comment without the comment right: refused",
            verb: "comment",
            rights: Rights::read_only(),
            person: |w, c| {
                w.session
                    .comment(c, NewComment::new(NewAnchor::Doc, "Hi"))
                    .map(AnnotOutput::Thread)
            },
            agent: |_| argv(&["comment", "--doc", "--body", "Hi"]),
            ..base
        },
        AnnotCase {
            name: "reply on a thread",
            verb: "reply",
            setup: one_thread,
            person: |w, c| {
                w.session
                    .reply(
                        c,
                        w.mark("t"),
                        "Done".into(),
                        vec![OBSERVER.into()],
                        Vec::new(),
                    )
                    .map(AnnotOutput::Thread)
            },
            agent: |w| {
                argv(&[
                    "reply",
                    &w.mark_arg("t"),
                    "--body",
                    "Done",
                    "--mention",
                    OBSERVER,
                ])
            },
            ..base
        },
        AnnotCase {
            name: "resolve a thread",
            verb: "resolve",
            setup: one_thread,
            person: |w, c| w.session.resolve(c, w.mark("t")).map(AnnotOutput::Thread),
            agent: |w| argv(&["resolve", &w.mark_arg("t")]),
            ..base
        },
        AnnotCase {
            name: "reopen a resolved thread",
            verb: "resolve --reopen",
            setup: resolved_thread,
            person: |w, c| w.session.reopen(c, w.mark("t")).map(AnnotOutput::Thread),
            agent: |w| argv(&["resolve", &w.mark_arg("t"), "--reopen"]),
            ..base
        },
        AnnotCase {
            name: "delete my own comment",
            verb: "comment --delete",
            setup: own_thread,
            person: |w, c| {
                w.session
                    .delete_comment(c, w.mark("t"), 0)
                    .map(AnnotOutput::Thread)
            },
            agent: |w| argv(&["comment", "--delete", &w.mark_arg("t")]),
            ..base
        },
        AnnotCase {
            name: "delete someone else's comment: refused",
            verb: "comment --delete",
            setup: one_thread,
            person: |w, c| {
                w.session
                    .delete_comment(c, w.mark("t"), 0)
                    .map(AnnotOutput::Thread)
            },
            agent: |w| argv(&["comment", "--delete", &w.mark_arg("t"), "--n", "0"]),
            ..base
        },
        AnnotCase {
            name: "list the open threads",
            verb: "comments",
            rights: Rights::read_only(),
            setup: two_threads,
            person: |w, c| {
                w.session
                    .comments(c, &ThreadQuery::new().open(true))
                    .map(AnnotOutput::Page)
            },
            agent: |_| argv(&["comments", "--open"]),
            ..base
        },
        AnnotCase {
            name: "read the threads' history",
            verb: "comments --log",
            rights: Rights::read_only().with_history(true),
            setup: two_threads,
            person: |w, c| {
                w.session
                    .annotation_log(c, None, 0, crate::annot::ANNOT_LOG_MAX)
                    .map(AnnotOutput::Log)
            },
            agent: |_| argv(&["comments", "--log"]),
            ..base
        },
        AnnotCase {
            name: "re-anchor a thread to a node (an agent's is confirmed by the person)",
            verb: "reanchor",
            needs_node: true,
            confirm: true,
            setup: one_thread,
            person: |w, c| {
                let k = w.node.clone().unwrap_or_default();
                w.session
                    .reanchor(c, w.mark("t"), NewAnchor::node(k, None))
                    .map(AnnotOutput::Reanchored)
            },
            agent: |w| {
                let k = w.node.clone().unwrap_or_default();
                argv(&["reanchor", &w.mark_arg("t"), "--node", &k])
            },
            ..base
        },
        AnnotCase {
            name: "re-anchor a thread to the whole doc (an agent's is confirmed by the person)",
            verb: "reanchor",
            confirm: true,
            setup: one_thread,
            person: |w, c| {
                w.session
                    .reanchor(c, w.mark("t"), NewAnchor::Doc)
                    .map(AnnotOutput::Reanchored)
            },
            agent: |w| argv(&["reanchor", &w.mark_arg("t"), "--doc"]),
            ..base
        },
    ]
}

fn exit_of(r: &Result<AnnotOutput, SessionError>) -> u8 {
    match r {
        Ok(_) => 0,
        Err(e) => tools::session_exit(e),
    }
}

/// Run one case on two fresh worlds. Returns the failures (none: parity holds).
pub fn run_annotation_case(
    pack: Arc<dyn BlobPack>,
    initial: &[u8],
    node: Option<&str>,
    registry: &Registry,
    case: &AnnotCase,
) -> Vec<Failure> {
    let mut f = Vec::new();
    let path = |side: Side, error: String| Failure::Path {
        case: case.name.to_owned(),
        side,
        error,
    };
    let world = || -> Result<AnnotWorld, String> {
        let mut w = AnnotWorld::new(pack.clone(), initial, node)?;
        (case.setup)(&mut w)?;
        w.grant(ACTOR, case.rights);
        Ok(w)
    };
    let (pw, aw) = match (world(), world()) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return vec![path(Side::Setup, e)],
    };
    // the person path
    let person = match pw.join_actor(ParticipantKind::Person, PERSON_VIA) {
        Ok(c) => (case.person)(&pw, c),
        Err(e) => return vec![path(Side::Person, format!("join: {e}"))],
    };
    // the agent path: registered verbs only
    let a = (case.agent)(&aw);
    let form = verb_form(&a);
    if form != case.verb {
        f.push(path(
            Side::Agent,
            format!("the argv names {form:?}, not {:?}", case.verb),
        ));
    }
    let Some(spec) = registry.get(&form).copied() else {
        f.push(Failure::MissingVerb {
            action: super::PersonAction::Annotate,
            verb: form,
            at: "agent path",
        });
        return f;
    };
    if let Ok(call) = AnnotCall::parse(&a) {
        if !spec.api.contains(&call.api()) {
            f.push(Failure::WrongApi {
                verb: form.clone(),
                declared: spec.api.to_vec(),
                called: call.api(),
            });
        }
    }
    let agent = match aw.join_actor(ParticipantKind::Agent, AGENT_VIA) {
        Ok(c) => aw.session.run_annotation(c, &a),
        Err(e) => return vec![path(Side::Agent, format!("join: {e}"))],
    };
    let agent_exit = match &agent {
        Ok(_) => 0,
        Err(e) => e.exit(),
    };
    if agent.is_ok() && !spec.exits.contains(&agent_exit) {
        f.push(Failure::UndeclaredExit {
            verb: form.clone(),
            exit: agent_exit,
        });
    }
    // an agent's re-anchor is a proposal: the same person confirms it
    if case.confirm {
        if let Ok(AnnotOutput::Reanchored(Reanchored::Proposed(t))) = &agent {
            let confirm = t.pending_anchor.as_ref().map(|p| match &p.to {
                AnchorTarget::Node { key } => NewAnchor::node(key.as_str(), Some(p.at)),
                _ => NewAnchor::Doc,
            });
            let done = match (confirm, aw.join_actor(ParticipantKind::Person, PERSON_VIA)) {
                (Some(to), Ok(c)) => aw.session.reanchor(c, t.id, to).map(|_| ()),
                (None, _) => Err(SessionError::BadAnnotation("no proposal".into())),
                (_, Err(e)) => Err(e),
            };
            if let Err(e) = done {
                f.push(path(Side::Agent, format!("the person's confirmation: {e}")));
            }
        } else {
            f.push(path(
                Side::Agent,
                format!("an agent's re-anchor must be a proposal: {agent:?}"),
            ));
        }
    }
    let mut diffs = Vec::new();
    let pe = exit_of(&person);
    if pe != agent_exit {
        diffs.push(format!("exit: person {pe}, agent {agent_exit}"));
    }
    if !case.confirm {
        let po = person.as_ref().map(output_lines).unwrap_or_default();
        let ao = agent.as_ref().map(output_lines).unwrap_or_default();
        if po != ao {
            diffs.push(format!("output: person {po:?}, agent {ao:?}"));
        }
    }
    match (pw.observe(false), aw.observe(case.confirm)) {
        (Ok(p), Ok(a)) => {
            if p != a {
                diffs.push(format!("threads: person {p:#?}, agent {a:#?}"));
            }
        }
        (Err(e), _) | (_, Err(e)) => f.push(path(Side::Setup, e)),
    }
    if !diffs.is_empty() {
        f.push(Failure::Mismatch {
            case: case.name.to_owned(),
            diffs,
        });
    }
    f
}

/// **Annotation parity** for a blob pack: every [`kit_annotation_cases`] case on `initial`
/// (node cases on `node`, skipped when `None`), against `registry` (normally
/// `Registry::for_tier(Tier::Blob).with_annotations()`), and every annotation verb with a
/// case. Returns how many cases ran.
pub fn check_annotations_with(
    pack: Arc<dyn BlobPack>,
    initial: &[u8],
    node: Option<&str>,
    registry: &Registry,
) -> Result<usize, Vec<Failure>> {
    let cases: Vec<AnnotCase> = kit_annotation_cases()
        .into_iter()
        .filter(|c| node.is_some() || !c.needs_node)
        .collect();
    let mut f = Vec::new();
    for v in super::ANNOTATION_VERBS {
        if !cases.iter().any(|c| c.verb == v.name) {
            f.push(Failure::NoCase {
                action: super::PersonAction::Annotate,
                verb: v.name.to_owned(),
            });
        }
    }
    for c in &cases {
        f.extend(run_annotation_case(
            pack.clone(),
            initial,
            node,
            registry,
            c,
        ));
    }
    if f.is_empty() {
        Ok(cases.len())
    } else {
        Err(f)
    }
}

/// [`check_annotations_with`] the blob registry and the annotation verbs.
pub fn check_annotations(
    pack: Arc<dyn BlobPack>,
    initial: &[u8],
    node: Option<&str>,
) -> Result<usize, Vec<Failure>> {
    check_annotations_with(
        pack,
        initial,
        node,
        &Registry::for_tier(Tier::Blob).with_annotations(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::AnyFile;

    /// Doc anchors on any file (no index: the node cases are skipped).
    #[test]
    fn annotation_parity_holds_on_any_file() {
        crate::differential::under_env_annot_mutant(|| {
            let n = check_annotations(Arc::new(AnyFile), b"%PDF-1.7\n", None)
                .unwrap_or_else(|f| panic!("{}", super::super::render(&f)));
            assert_eq!(n, 11);
        });
    }

    /// A negative control: without a verb, the harness says so.
    #[test]
    fn a_missing_annotation_verb_is_reported() {
        let r = Registry::for_tier(Tier::Blob)
            .with_annotations()
            .without("reply");
        let f = check_annotations_with(Arc::new(AnyFile), b"x", None, &r).unwrap_err();
        assert!(
            f.iter()
                .any(|x| matches!(x, Failure::MissingVerb { verb, .. } if verb == "reply")),
            "{f:?}"
        );
        // and a registry without the annotation verbs has none of them
        let plain = Registry::for_tier(Tier::Blob);
        assert!(super::super::ANNOTATION_VERBS
            .iter()
            .all(|v| plain.get(v.name).is_none()));
    }
}
