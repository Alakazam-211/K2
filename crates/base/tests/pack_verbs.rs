//! A pack's own verbs (amendment 6): `Pack::verbs` routed by `run_tool` like kit edit
//! verbs, and registered for parity.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::parity::{PersonAction, Registry, Source};
use k2_mpc_base::session::{
    AuditEvent, Config, Declined, Host, Pack, PackContent, PackError, Participant, ParticipantKind,
    Rights, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{EditOutput, PackEdit, PackVerb, ToolError, ToolRunError};
use k2_mpc_base::Schema;

struct Shouty(&'static [PackVerb]);

const TEXT: TextFilePack = TextFilePack::new("shouty", 1 << 20);

impl Pack for Shouty {
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
        self.0
    }
}

/// `shout [--line N]`: uppercase the doc (or one line); `--line` past the end is exit 1
/// with nothing applied.
fn shout(args: &[String]) -> Result<PackEdit, ToolError> {
    let line = match args {
        [] => None,
        [f, n] if f == "--line" => Some(
            n.parse::<usize>()
                .map_err(|_| ToolError::usage("--line N"))?,
        ),
        _ => return Err(ToolError::unsupported(args.join(" "), "shout [--line N]")),
    };
    Ok(Box::new(move |name, text, _stdin| {
        let Some(n) = line else {
            return Ok(EditOutput::new(Some(text.to_uppercase()), Vec::new(), 0));
        };
        if n == 0 || n > text.lines().count() {
            let msg = format!("{name}: no line {n}\n").into_bytes();
            return Ok(EditOutput::new(None, msg, 1));
        }
        let out: String = text
            .split_inclusive('\n')
            .enumerate()
            .map(|(i, l)| {
                if i + 1 == n {
                    l.to_uppercase()
                } else {
                    l.to_owned()
                }
            })
            .collect();
        Ok(EditOutput::new(Some(out), Vec::new(), 0))
    }))
}

fn never(_: &[String]) -> Result<PackEdit, ToolError> {
    panic!("a kit verb's name must never reach the pack")
}

const VERBS: &[PackVerb] = &[
    PackVerb::edit(
        "shout",
        "shout [--line N]",
        &[PersonAction::Format],
        &[0, 1],
        shout,
    ),
    PackVerb::edit(
        "cat",
        "a pack verb named like a kit verb",
        &[PersonAction::Format],
        &[0],
        never,
    ),
];

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        if who.pid == "reader" {
            Rights::read_only()
        } else {
            Rights::full()
        }
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn argv(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| (*x).to_owned()).collect()
}

#[test]
fn run_tool_routes_pack_verbs() {
    let dir = std::env::temp_dir().join(format!("k2mpc-packverbs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "a.txt".into(),
    };
    let s = mgr
        .open(Arc::new(Shouty(VERBS)), &key, Some(b"one\r\ntwo\r\n"))
        .unwrap();
    let who = |pid: &str| Participant::new(pid, pid, "#000000", ParticipantKind::Agent, "cli");
    let c = s.join_shared(who("ada"), Stay::UntilIdle).unwrap().conn;
    let root = TextFilePack::ROOT;

    // one change, attributed, encoded back by the pack (CRLF kept)
    let out = s
        .run_tool(c, root, &argv(&["shout", "--line", "2"]), b"")
        .unwrap();
    assert_eq!(out.exit, 0);
    let changed = out.changed.expect("applied");
    assert_eq!((changed.lines_added, changed.lines_removed), (1, 1));
    assert!(
        out.stderr_summary.starts_with("edited a.txt line 2"),
        "{}",
        out.stderr_summary
    );
    assert_eq!(s.read(c).unwrap().bytes, b"one\r\nTWO\r\n");

    // the tool's "no": exit 1, its report on stdout, nothing applied
    let v = s.status().unwrap().version;
    let out = s
        .run_tool(c, root, &argv(&["shout", "--line", "9"]), b"")
        .unwrap();
    assert_eq!((out.exit, out.changed.is_none()), (1, true));
    assert_eq!(out.stdout, b"a.txt: no line 9\n");
    assert!(
        out.stderr_summary.starts_with("nothing applied"),
        "{}",
        out.stderr_summary
    );
    assert_eq!(s.status().unwrap().version, v);

    // bad arguments: exit 2 before the doc is touched
    let e = s
        .run_tool(c, root, &argv(&["shout", "--loud"]), b"")
        .unwrap_err();
    assert_eq!(e.exit(), 2);
    assert_eq!(s.status().unwrap().version, v);

    // an edit verb: read-only participants are refused
    let r = s.join_shared(who("reader"), Stay::UntilIdle).unwrap().conn;
    let e = s.run_tool(r, root, &argv(&["shout"]), b"").unwrap_err();
    assert!(matches!(e, ToolRunError::Session(_)), "{e:?}");
    assert_eq!(e.exit(), 3);

    // a kit verb's name always means the kit's verb
    let out = s.run_tool(c, root, &argv(&["cat"]), b"").unwrap();
    assert_eq!(out.stdout, b"one\r\nTWO\r\n");

    // one undo reverts the pack verb's change
    s.undo(c, 1).unwrap();
    assert_eq!(s.read(c).unwrap().bytes, b"one\r\ntwo\r\n");

    // a pack without the verb doesn't know it
    let plain = mgr
        .open(
            Arc::new(TEXT),
            &DocKey {
                space: "ws".into(),
                path: "b.txt".into(),
            },
            Some(b"x\n"),
        )
        .unwrap();
    let p = plain.join_shared(who("ada"), Stay::UntilIdle).unwrap().conn;
    let e = plain.run_tool(p, root, &argv(&["shout"]), b"").unwrap_err();
    assert_eq!(e.exit(), 2);
    drop((s, plain, mgr));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registry_takes_pack_verbs_and_refuses_clashes() {
    let r = Registry::for_pack(&Shouty(&VERBS[..1])).unwrap();
    let v = r.get("shout").expect("registered");
    assert_eq!(v.source, Source::Pack);
    assert_eq!(v.serves, &[PersonAction::Format]);
    assert_eq!(v.exits, &[0, 1]);
    assert!(
        Registry::for_pack(&Shouty(VERBS)).is_err(),
        "cat clashes with the kit"
    );
    const NONE: &[PackVerb] = &[PackVerb::edit("idle", "", &[], &[0], never)];
    assert!(Registry::for_pack(&Shouty(NONE)).is_err(), "serves nothing");
    const SPACED: &[PackVerb] = &[PackVerb::edit(
        "two words",
        "",
        &[PersonAction::Format],
        &[0],
        never,
    )];
    assert!(Registry::for_pack(&Shouty(SPACED)).is_err());
    assert_eq!(Registry::for_pack(&TEXT).unwrap(), Registry::kit());
}
