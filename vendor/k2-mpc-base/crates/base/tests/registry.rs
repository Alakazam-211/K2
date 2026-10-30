//! The verb registry against the implementation: [`VerbSpec::runs_through_run_tool`] is data,
//! and here it must agree with what `Session::run_tool` does. Every flagged verb runs there
//! (a sample invocation per verb, the pack's verb included); every other registered verb is
//! refused there as not a tool, with nothing applied. Public API only.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::parity::{verb_form, PersonAction, Registry, Source, RUN_TOOL_VERBS};
use k2_mpc_base::session::{
    AuditEvent, ConnId, Declined, Host, Pack, PackContent, PackError, Participant, ParticipantKind,
    Rights, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{EditOutput, PackEdit, PackVerb, ToolError, ToolRunError};
use k2_mpc_base::Schema;

const TEXT: TextFilePack = TextFilePack::new("reg", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "a\nb\nc\n";

struct P;

impl Pack for P {
    fn schema(&self) -> Schema {
        TEXT.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        TEXT.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        TEXT.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        TEXT.normalize_input(root, text)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
}

fn shout(args: &[String]) -> Result<PackEdit, ToolError> {
    if !args.is_empty() {
        return Err(ToolError::unsupported(args.join(" "), "shout"));
    }
    Ok(Box::new(|_, text, _| {
        Ok(EditOutput::new(Some(text.to_uppercase()), Vec::new(), 0))
    }))
}

const VERBS: &[PackVerb] = &[PackVerb::edit(
    "shout",
    "shout",
    &[PersonAction::Format],
    &[0],
    shout,
)];

struct TestHost(AtomicU64);

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::editor()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    _mgr: SessionManager,
    s: Arc<Session>,
    c: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-registry-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(
        store,
        Arc::new(TestHost(AtomicU64::new(1_000_000))),
        Default::default(),
    );
    let key = DocKey {
        space: "ws".into(),
        path: "notes.md".into(),
    };
    let s = mgr.open(Arc::new(P), &key, Some(DOC.as_bytes())).unwrap();
    let p = Participant::new("agent", "Agent", "#123456", ParticipantKind::Agent, "cli");
    let c = s.join(p, Stay::Held).unwrap().conn;
    Fx {
        dir,
        _mgr: mgr,
        s,
        c,
    }
}

/// One valid invocation of each verb that runs through `run_tool`: (argv, stdin).
fn sample(verb: &str) -> Option<(Vec<&'static str>, &'static str)> {
    Some(match verb {
        "cat" => (vec!["cat"], ""),
        "wc" => (vec!["wc", "-l"], ""),
        "head" => (vec!["head", "-n", "1"], ""),
        "tail" => (vec!["tail", "-n", "1"], ""),
        "sed" => (vec!["sed", "-n", "2p"], ""),
        "view" => (vec!["view", "--lines", "1,2"], ""),
        "grep" => (vec!["grep", "b"], ""),
        "diff" => (vec!["diff", "-u"], "a\nB\nc\n"),
        "insert" => (vec!["insert", "--line", "1"], "x\n"),
        "replace" => (vec!["replace", "--lines", "2,2"], "B\n"),
        "delete" => (vec!["delete", "--lines", "3,3"], ""),
        "sed -i" => (vec!["sed", "-i", "s/a/A/"], ""),
        "patch" | "apply" => (
            vec![if verb == "patch" { "patch" } else { "apply" }],
            "--- notes.md\n+++ notes.md\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n",
        ),
        "append" => (vec!["append"], "z\n"),
        "shout" => (vec!["shout"], ""),
        _ => return None,
    })
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn every_flagged_verb_runs_through_run_tool() {
    let reg = Registry::for_pack(&P).unwrap();
    let flagged: Vec<_> = reg
        .verbs()
        .iter()
        .filter(|v| v.runs_through_run_tool())
        .collect();
    // the kit's list and the pack's verb, nothing else
    assert_eq!(flagged.len(), RUN_TOOL_VERBS.len() + VERBS.len());
    for v in flagged {
        let f = fx("flagged");
        let (a, stdin) =
            sample(v.name).unwrap_or_else(|| panic!("{}: flagged, but no sample here", v.name));
        let a = argv(&a);
        assert_eq!(verb_form(&a), v.name, "the sample names its verb");
        let out =
            f.s.run_tool(f.c, ROOT, &a, stdin.as_bytes())
                .unwrap_or_else(|e| panic!("{}: run_tool refused it: {e}", v.name));
        assert!(
            v.exits.contains(&out.exit),
            "{}: exit {} not in {:?}",
            v.name,
            out.exit,
            v.exits
        );
        let edit = v.kind == k2_mpc_base::parity::VerbKind::Edit;
        assert_eq!(out.changed.is_some(), edit, "{}: an edit changes", v.name);
        assert_eq!(out.fence.is_some(), !edit, "{}: a read is fenced", v.name);
        if v.source == Source::Pack {
            assert_eq!(f.s.read(f.c).unwrap().text_by_root[0].1, DOC.to_uppercase());
        }
    }
}

#[test]
fn every_other_verb_is_not_a_run_tool_verb() {
    let reg = Registry::for_pack(&P).unwrap();
    let f = fx("others");
    let v0 = f.s.status().unwrap().version;
    let mut n = 0;
    for v in reg.verbs().iter().filter(|v| !v.runs_through_run_tool()) {
        assert!(sample(v.name).is_none(), "{} has a sample", v.name);
        let a = argv(&v.name.split(' ').collect::<Vec<_>>());
        match f.s.run_tool(f.c, ROOT, &a, b"") {
            Err(ToolRunError::Tool(ToolError::Unsupported { .. })) => {}
            r => panic!(
                "{}: run_tool should refuse it as not a tool, got {r:?}",
                v.name
            ),
        }
        n += 1;
    }
    assert!(n > 20, "{n} non-tool verbs");
    assert_eq!(f.s.status().unwrap().version, v0, "nothing applied");
    assert_eq!(f.s.read(f.c).unwrap().text_by_root[0].1, DOC);
}
