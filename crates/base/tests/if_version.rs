//! `--if-version V` on every edit verb run through `run_tool` (amendment 14): applied only if
//! the doc is still at version V, checked under the session lock, before every other
//! refusal; a stale V exits 1 with one line, `doc changed since V (now W)`, and no doc text.
//! Read verbs refuse the flag (exit 2).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::parity::{PersonAction, Registry};
use k2_mpc_base::session::{
    AuditEvent, ClaimEnd, ClaimPolicy, Config, ConnId, Declined, EditOptions, EditWithError, Host,
    LineRange, Pack, PackContent, PackError, Participant, ParticipantKind, ReservationEnd, Rights,
    Session, SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::tools::{
    self, EditOutput, Kind, PackEdit, PackVerb, ToolError, ToolOptions, ToolRunError,
};
use k2_mpc_base::{Edit, Schema};

const TEXT: TextFilePack = TextFilePack::new("ifv", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;
const DOC: &str = "alpha one\nbeta two\ngamma three\ndelta four\n";
/// Words of `DOC` that a refusal must never quote.
const DOC_WORDS: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "one", "two", "three", "four",
];

// ---------- a pack with one verb of its own ----------

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

/// `shout`: uppercase the doc. Takes no arguments, so it refuses any `--if-version` the kit
/// failed to take out first.
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

// ---------- the host: rights, a clock, and a hook on the clock ----------

type Inject = Box<dyn FnOnce() + Send>;

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    /// Runs once, on the next clock read: `edit_with` reads the clock before it takes the
    /// session lock, so this lands inside a `run_tool` call, before its critical section.
    inject: Mutex<Option<Inject>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        let hook = self.inject.lock().unwrap().take();
        if let Some(f) = hook {
            f();
        }
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: std::path::PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx_with(tag: &str, tweak: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-ifv-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        inject: Mutex::new(None),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    tweak(&mut c);
    let mgr = SessionManager::new(store, host.clone(), c);
    Fx { dir, host, mgr }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, |_| {})
}

impl Fx {
    fn open(&self, path: &str) -> Arc<Session> {
        let key = DocKey {
            space: "ws".into(),
            path: path.into(),
        };
        self.mgr
            .open(Arc::new(P), &key, Some(DOC.as_bytes()))
            .unwrap()
    }
    fn join_as(&self, s: &Session, p: Participant, r: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(p.pid.clone(), r);
        s.join(p, Stay::UntilIdle).unwrap().conn
    }
    fn join(&self, s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
        let p = Participant::new(pid, pid.to_uppercase(), "#123456", kind, "cli");
        self.join_as(s, p, Rights::editor())
    }
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn run(s: &Session, c: ConnId, a: &[&str], stdin: &str) -> Result<tools::ToolOutput, ToolRunError> {
    s.run_tool(c, ROOT, &argv(a), stdin.as_bytes())
}

fn text(s: &Session, c: ConnId) -> String {
    s.read(c).unwrap().text_by_root.remove(0).1
}

/// The version a read prints (what an agent passes back as `--if-version`).
fn read_version(s: &Session, c: ConnId) -> u64 {
    let out = run(s, c, &["cat"], "").unwrap();
    // the number the agent sees: `ToolOutput::version`, printed in the summary and the fence
    assert!(out
        .stderr_summary
        .ends_with(&format!(".md v{}", out.version)));
    assert_eq!(out.fence.as_ref().unwrap().version, out.version);
    out.version
}

/// Someone else's edit: a line added at the end.
fn other_edit(s: &Session, c: ConnId) {
    let end = text(s, c).len();
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: end,
                end,
                insert: "sam was here\n".into(),
            }],
        }],
        None,
    )
    .unwrap();
}

/// Every edit verb, as (argv, stdin), each changing `DOC`. The pack's own verb is last.
fn edit_cases() -> Vec<(Vec<&'static str>, String)> {
    let patched = DOC.replace("beta two", "BETA TWO");
    let diff = tools::unified(DOC, &patched, 3, "a/d.md", "b/d.md");
    vec![
        (vec!["sed", "-i", "s/alpha/ALPHA/"], String::new()),
        (vec!["patch"], diff.clone()),
        (vec!["apply", "-p1"], diff),
        (vec!["append"], "omega\n".into()),
        (vec!["append", "--ensure-newline"], "omega".into()),
        (vec!["insert", "--line", "1"], "zeta\n".into()),
        (vec!["insert", "--after", "$"], "zeta\n".into()),
        (vec!["insert", "--at", "1:1"], ">> ".into()),
        (vec!["replace", "--lines", "2,2"], "B\n".into()),
        (vec!["replace", "--range", "1:1-1:6"], "ALPHA".into()),
        (
            vec!["replace", "--old", "gamma", "--new", "GAMMA"],
            String::new(),
        ),
        (
            vec!["replace", "--old", "a", "--new", "A", "--all"],
            String::new(),
        ),
        (vec!["replace", "--old", "-", "--new", "D"], "delta".into()),
        (vec!["delete", "--lines", "3,4"], String::new()),
        (vec!["delete", "--range", "1:1-2:1"], String::new()),
        (vec!["shout"], String::new()),
    ]
}

/// `argv` with `--if-version v` added, in one of the accepted spellings (`k` picks it).
fn with_version(a: &[&'static str], v: u64, k: usize) -> Vec<String> {
    let mut out = argv(a);
    let flag = match k % 4 {
        0 => vec!["--if-version".to_owned(), v.to_string()],
        1 => vec![format!("--if-version={v}")],
        2 => vec!["--if-version".to_owned(), format!("v{v}")],
        // first, before the verb's own flags (getopt permutation)
        _ => {
            out.insert(1, format!("--if-version={v}"));
            return out;
        }
    };
    out.extend(flag);
    out
}

fn assert_stale(
    r: Result<tools::ToolOutput, ToolRunError>,
    expected: u64,
    actual: u64,
    what: &str,
) {
    let e = match r {
        Err(e) => e,
        Ok(o) => panic!("{what}: applied at a stale version: {o:?}"),
    };
    assert_eq!(
        e,
        ToolRunError::Session(SessionError::Stale { expected, actual }),
        "{what}"
    );
    assert_eq!(e.exit(), 1, "{what}");
    let msg = e.to_string();
    assert_eq!(
        msg,
        format!("doc changed since {expected} (now {actual})"),
        "{what}"
    );
    assert!(!msg.contains('\n'), "{what}: one line");
    for w in DOC_WORDS {
        assert!(!msg.contains(w), "{what}: the refusal quotes the doc ({w})");
    }
}

// ---------- every edit verb ----------

#[test]
fn every_edit_verb_applies_at_the_current_version() {
    let f = fx("current");
    for (k, (a, stdin)) in edit_cases().into_iter().enumerate() {
        let s = f.open(&format!("ok{k}.md"));
        let agent = f.join(&s, "agent", ParticipantKind::Agent);
        let v = read_version(&s, agent);
        let before = text(&s, agent);
        let out = s
            .run_tool(agent, ROOT, &with_version(&a, v, k), stdin.as_bytes())
            .unwrap_or_else(|e| panic!("{a:?}: {e}"));
        assert_eq!(out.exit, 0, "{a:?}");
        assert!(out.changed.is_some(), "{a:?} changed the doc");
        assert_eq!(out.version, v + 1, "{a:?}: one change, on top of V");
        assert_ne!(text(&s, agent), before, "{a:?}");
        // the same verb without the flag behaves the same (on a fresh doc)
        let s2 = f.open(&format!("plain{k}.md"));
        let agent2 = f.join(&s2, "agent", ParticipantKind::Agent);
        let plain = run(&s2, agent2, &a, &stdin).unwrap();
        assert_eq!(text(&s2, agent2), text(&s, agent), "{a:?}: same result");
        assert_eq!(plain.exit, out.exit);
    }
}

#[test]
fn every_edit_verb_refuses_a_stale_version_and_applies_nothing() {
    let f = fx("stale");
    for (k, (a, stdin)) in edit_cases().into_iter().enumerate() {
        let s = f.open(&format!("st{k}.md"));
        let agent = f.join(&s, "agent", ParticipantKind::Agent);
        let sam = f.join(&s, "sam", ParticipantKind::Person);
        let v = read_version(&s, agent);
        other_edit(&s, sam);
        let now = s.status().unwrap().version;
        assert!(now > v);
        let before = text(&s, agent);
        let r = s.run_tool(agent, ROOT, &with_version(&a, v, k), stdin.as_bytes());
        assert_stale(r, v, now, &format!("{a:?}"));
        assert_eq!(text(&s, agent), before, "{a:?}: nothing applied");
        assert_eq!(s.status().unwrap().version, now, "{a:?}: no new version");
        // re-read, and the same call at the new version applies
        let v2 = read_version(&s, agent);
        let out = s
            .run_tool(agent, ROOT, &with_version(&a, v2, k), stdin.as_bytes())
            .unwrap_or_else(|e| panic!("{a:?} at v{v2}: {e}"));
        assert_eq!(out.version, v2 + 1, "{a:?}");
    }
}

#[test]
fn an_abbreviated_flag_works_on_kit_verbs() {
    let f = fx("abbrev");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let v = read_version(&s, agent);
    let stale = (v + 7).to_string();
    let r = run(&s, agent, &["append", "--if-v", &stale], "x\n");
    assert_stale(r, v + 7, v, "append --if-v");
    let out = run(
        &s,
        agent,
        &["delete", "--if", &v.to_string(), "--lines", "1,1"],
        "",
    )
    .unwrap();
    assert_eq!(out.version, v + 1);
}

// ---------- races ----------

/// The agent reads, Sam edits the same line, the agent's find-and-replace from its read is
/// refused. Without the flag it would have applied where the text now is.
#[test]
fn race_an_edit_between_the_read_and_the_edit_is_refused() {
    let f = fx("race");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let seen = run(&s, agent, &["view", "--lines", "2,2"], "").unwrap();
    assert_eq!(seen.stdout, b"beta two\n");
    // Sam rewrites line 2 and adds a second "two"
    s.replace_text(sam, ROOT, &DOC.replace("beta two", "beta two (and two)"))
        .unwrap();
    let now = s.status().unwrap().version;
    let before = text(&s, agent);
    let v = seen.version.to_string();
    for a in [
        &[
            "replace",
            "--old",
            "two",
            "--new",
            "2",
            "--all",
            "--if-version",
            &v,
        ][..],
        &["sed", "-i", "--if-version", &v, "2s/two/2/"],
        &["replace", "--lines", "2,2", "--if-version", &v],
    ] {
        assert_stale(
            run(&s, agent, a, "beta 2\n"),
            seen.version,
            now,
            &format!("{a:?}"),
        );
    }
    assert_eq!(text(&s, agent), before);
    // the same edit without the guard lands on Sam's text
    run(&s, agent, &["sed", "-i", "2s/two/2/"], "").unwrap();
    assert_eq!(text(&s, agent).lines().nth(1), Some("beta 2 (and two)"));
}

/// Deterministic race inside one call: Sam's edit lands after `run_tool` has started (parsed
/// the verb, looked up the doc) but before its critical section. The check must see it: a
/// check against a version taken outside the lock would let the edit through.
#[test]
fn race_an_edit_landing_inside_the_call_before_the_lock_is_refused() {
    let f = fx("race-inside");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let v = read_version(&s, agent);
    let (tx, rx) = std::sync::mpsc::channel();
    let s2 = s.clone();
    *f.host.inject.lock().unwrap() = Some(Box::new(move || {
        // on its own thread, so a clock read under the session lock fails loudly instead of
        // deadlocking
        let s3 = s2.clone();
        std::thread::spawn(move || {
            other_edit(&s3, sam);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the injected edit blocked: the clock was read under the session lock");
    }));
    let r = run(
        &s,
        agent,
        &[
            "replace",
            "--old",
            "alpha",
            "--new",
            "ALPHA",
            "--if-version",
            &v.to_string(),
        ],
        "",
    );
    assert!(f.host.inject.lock().unwrap().is_none(), "the hook ran");
    assert_stale(r, v, v + 1, "edit after an injected edit");
    assert!(text(&s, agent).contains("alpha one"), "nothing applied");
}

/// Threads race guarded appends from their own reads: every applied one was computed at
/// exactly the version it names, and every refused one names a version that had moved.
#[test]
fn race_threads_each_guarded_edit_applies_on_its_version_or_not_at_all() {
    let f = fx("race-threads");
    let s = f.open("d.md");
    let start = s.status().unwrap().version;
    let conns: Vec<ConnId> = (0..4)
        .map(|t| f.join(&s, &format!("agent{t}"), ParticipantKind::Agent))
        .collect();
    let applied = AtomicU64::new(0);
    std::thread::scope(|sc| {
        for (t, &c) in conns.iter().enumerate() {
            let (s, applied) = (&s, &applied);
            sc.spawn(move || {
                for i in 0..25 {
                    let v = read_version(s, c);
                    let a = ["append", "--if-version", &v.to_string()];
                    match run(s, c, &a, &format!("t{t} i{i}\n")) {
                        Ok(out) => {
                            assert_eq!(out.version, v + 1, "applied on top of v{v} only");
                            applied.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(ToolRunError::Session(SessionError::Stale { expected, actual })) => {
                            assert_eq!(expected, v);
                            assert!(actual > v);
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
            });
        }
    });
    let n = applied.load(Ordering::SeqCst);
    assert!(n > 0);
    assert_eq!(s.status().unwrap().version, start + n);
    assert_eq!(text(&s, conns[0]).lines().count() as u64, 4 + n);
}

// ---------- read verbs refuse it ----------

#[test]
fn read_verbs_refuse_the_flag() {
    let f = fx("reads");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let v = read_version(&s, agent);
    for (a, stdin) in [
        (&["cat"][..], ""),
        (&["cat", "-n"], ""),
        (&["head", "-n", "1"], ""),
        (&["tail", "-n", "1"], ""),
        (&["wc", "-l"], ""),
        (&["grep", "alpha"], ""),
        (&["sed", "-n", "1p"], ""),
        (&["diff"], DOC),
        (&["view", "--lines", "1,2"], ""),
        (&["view", "--lines", "1,2", "--for-edit"], ""),
    ] {
        // the current version and a stale one: a usage error either way, never Stale
        for k in 0..4 {
            for ver in [v, v + 3] {
                let r = s.run_tool(agent, ROOT, &with_version(a, ver, k), stdin.as_bytes());
                let Err(ToolRunError::Tool(ToolError::Usage(m))) = &r else {
                    panic!("{a:?}: {r:?}");
                };
                assert!(m.contains("--if-version is for edit verbs"), "{a:?}: {m}");
                assert_eq!(r.unwrap_err().exit(), 2);
            }
        }
    }
    // a pattern or operand that is literally the flag is not the flag
    let out = run(&s, agent, &["grep", "-c", "-e", "--if-version"], "").unwrap();
    assert_eq!((out.exit, out.stdout.as_slice()), (1, &b"0\n"[..]));
    assert_eq!(s.status().unwrap().version, v, "nothing changed");
}

#[test]
fn bad_values_are_usage_errors() {
    let f = fx("bad");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let v = read_version(&s, agent).to_string();
    for a in [
        &["append", "--if-version"][..],
        &["append", "--if-version", "x"],
        &["append", "--if-version", "-1"],
        &["append", "--if-version="],
        &["append", "--if-version", "v"],
        &["append", "--if-version", "99999999999999999999999"],
        &["append", "--if-version", &v, "--if-version", &v],
        &["sed", "-i", "--if-version", "1.5", "s/a/b/"],
        &["shout", "--if-version", "x"],
        &["shout", "--if-version"],
        &["shout", "--if-version", &v, "--if-version", &v],
    ] {
        let r = run(&s, agent, a, "x\n");
        assert!(
            matches!(r, Err(ToolRunError::Tool(ToolError::Usage(_)))),
            "{a:?}: {r:?}"
        );
    }
    // a pack verb's arguments after `--` are the pack's
    let r = run(&s, agent, &["shout", "--", "--if-version", &v], "");
    assert!(matches!(
        r,
        Err(ToolRunError::Tool(ToolError::Unsupported { .. }))
    ));
    assert_eq!(text(&s, agent), DOC);
}

// ---------- interplay: the check comes first ----------

#[test]
fn stale_wins_over_the_verbs_own_no() {
    let f = fx("order-no");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let v = read_version(&s, agent);
    other_edit(&s, sam);
    let now = v + 1;
    let vs = v.to_string();
    let bad_patch = tools::unified("no\nsuch\n", "no\nSUCH\n", 3, "a/d.md", "b/d.md");
    for (a, stdin, current) in [
        // no match: exit 1 with a report on stdout, at the current version
        (&["replace", "--old", "nowhere", "--new", "x"][..], "", 1u8),
        // more than one match
        (&["replace", "--old", "a", "--new", "x"], "", 1),
        // old == new: a no-op that still checks the version
        (&["replace", "--old", "alpha", "--new", "alpha"], "", 0),
        // a patch that doesn't apply, and a dry run
        (&["patch"], bad_patch.as_str(), 1),
        (&["patch", "--dry-run"], bad_patch.as_str(), 1),
        // a sed that changes nothing
        (&["sed", "-i", "s/nowhere/x/"], "", 0),
    ] {
        let mut stale = argv(a);
        stale.extend(["--if-version".to_owned(), vs.clone()]);
        assert_stale(
            s.run_tool(agent, ROOT, &stale, stdin.as_bytes()),
            v,
            now,
            &format!("{a:?}"),
        );
        // at the current version: the verb's own answer, nothing applied
        let mut cur = argv(a);
        cur.extend(["--if-version".to_owned(), now.to_string()]);
        let out = s.run_tool(agent, ROOT, &cur, stdin.as_bytes()).unwrap();
        assert_eq!(out.exit, current, "{a:?}");
        assert!(out.changed.is_none(), "{a:?}");
    }
    let out = run(
        &s,
        agent,
        &[
            "replace",
            "--old",
            "alpha",
            "--new",
            "alpha",
            "--if-version",
            &now.to_string(),
        ],
        "",
    )
    .unwrap();
    assert!(
        out.stderr_summary.starts_with("no change to d.md"),
        "{}",
        out.stderr_summary
    );
    assert_eq!(s.status().unwrap().version, now);
}

#[test]
fn stale_wins_over_a_tool_error_found_on_the_live_text() {
    let f = fx("order-tool");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let v = read_version(&s, agent);
    other_edit(&s, sam);
    // a range past the end is the tool's usage error, found only on the live text
    let r = run(
        &s,
        agent,
        &[
            "delete",
            "--range",
            "90:1-91:1",
            "--if-version",
            &(v + 1).to_string(),
        ],
        "",
    );
    assert!(matches!(r, Err(ToolRunError::Tool(_))), "{r:?}");
    let r = run(
        &s,
        agent,
        &[
            "delete",
            "--range",
            "90:1-91:1",
            "--if-version",
            &v.to_string(),
        ],
        "",
    );
    assert_stale(r, v, v + 1, "delete past the end");
}

#[test]
fn stale_wins_over_the_claim_policy() {
    let f = fx("order-claims");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let v = read_version(&s, agent);
    s.claim(
        bot,
        LineRange {
            root: ROOT.into(),
            first: 1,
            last: 2,
        },
        None,
        None,
        false,
    )
    .unwrap();
    let respect = ToolOptions::new().policy(ClaimPolicy::Respect);
    let tool = |ver: u64| {
        s.run_tool_with(
            agent,
            ROOT,
            &argv(&["sed", "-i", "--if-version", &ver.to_string(), "1s/alpha/A/"]),
            b"",
            &respect,
        )
    };
    // claims change no version: at V it is the claim that refuses (5)
    let e = tool(v).unwrap_err();
    assert!(
        matches!(e, ToolRunError::Session(SessionError::Claimed { .. })),
        "{e:?}"
    );
    assert_eq!(e.exit(), 5);
    // at a stale V, the version refuses first
    assert_stale(tool(v - 1), v - 1, v, "claimed and stale");
    assert_eq!(text(&s, agent), DOC);
}

#[test]
fn stale_wins_over_the_approvals_backlog() {
    let f = fx_with("order-backlog", |c| c.pending_max_records = 0);
    let s = f.open("d.md");
    let aide = Participant::new("aide", "Aide", "#123456", ParticipantKind::Agent, "cli")
        .on_behalf_of("alice");
    let aide = f.join_as(&s, aide, Rights::editor());
    let v = read_version(&s, aide);
    let r = run(&s, aide, &["append", "--if-version", &v.to_string()], "x\n").unwrap_err();
    assert!(
        matches!(r, ToolRunError::Session(SessionError::Held { .. })),
        "{r:?}"
    );
    let stale = v + 5;
    assert_stale(
        run(
            &s,
            aide,
            &["append", "--if-version", &stale.to_string()],
            "x\n",
        ),
        stale,
        v,
        "held and stale",
    );
}

/// Admission comes before the version, as for `Session::edit`: a caller without edit
/// rights is refused for that (exit 3), whatever version it names.
#[test]
fn rights_come_before_the_version() {
    let f = fx("order-rights");
    let s = f.open("d.md");
    let reader = Participant::new("r", "R", "#123456", ParticipantKind::Agent, "cli");
    let reader = f.join_as(&s, reader, Rights::read_only());
    let v = read_version(&s, reader);
    let e = run(
        &s,
        reader,
        &["append", "--if-version", &(v + 9).to_string()],
        "x\n",
    )
    .unwrap_err();
    assert!(
        matches!(e, ToolRunError::Session(SessionError::Forbidden { .. })),
        "{e:?}"
    );
    assert_eq!(e.exit(), 3);
}

// ---------- the pack's verbs, the registry ----------

#[test]
fn pack_verbs_take_the_flag_too() {
    let f = fx("pack");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let v = read_version(&s, agent);
    other_edit(&s, sam);
    assert_stale(
        run(&s, agent, &["shout", "--if-version", &v.to_string()], ""),
        v,
        v + 1,
        "shout",
    );
    let out = run(
        &s,
        agent,
        &["shout", &format!("--if-version=v{}", v + 1)],
        "",
    )
    .unwrap();
    assert_eq!(out.version, v + 2);
    assert!(text(&s, agent).starts_with("ALPHA ONE\n"));
}

#[test]
fn every_kit_edit_verb_names_the_flag_in_its_usage() {
    let r = Registry::kit();
    for (verb, kind) in tools::VERBS {
        let usage = r.get(verb).unwrap_or_else(|| panic!("{verb}")).usage;
        let named = usage.contains("--if-version") || usage == "apply: as patch";
        assert_eq!(named, *kind == Kind::Edit, "{verb}: {usage}");
    }
}

// ---------- claim ends carry the range and the version (amendment 14) ----------

fn lines(first: usize, last: usize) -> LineRange {
    LineRange {
        root: ROOT.into(),
        first,
        last,
    }
}

/// Sam inserts `ins` before byte `at`.
fn insert_at(s: &Session, c: ConnId, at: usize, ins: &str) {
    s.edit(
        c,
        vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: at,
                end: at,
                insert: ins.into(),
            }],
        }],
        None,
    )
    .unwrap();
}

/// The host's flow: a claim ends, the host tells the bounced agent "lines X–Y are free; the
/// doc is at `version`; re-read", and sends its next edit with `--if-version` = what it last
/// read. `ClaimEnded`/`ReservationEnded` carry the range as it ends (moved by the edits made
/// while it was live) and the very version `--if-version` compares.
#[test]
fn claim_and_reservation_ends_carry_current_lines_and_the_guard_version() {
    let f = fx("claim-end");
    let s = f.open("d.md");
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    // the agent read before it was bounced off the bot's claim on lines 3-4
    let read_before = read_version(&s, agent);
    let claim = s.claim(bot, lines(3, 4), None, None, false).unwrap();
    s.register_waiter(agent, claim.id).unwrap();
    // while the claim is live: two lines above it, one inside it
    insert_at(&s, sam, 0, "x\ny\n");
    let at = text(&s, sam).find("delta").unwrap();
    insert_at(&s, sam, at, "mid\n");
    s.release_claim(bot, Some(claim.id)).unwrap();
    let w = s.status().unwrap().version;
    assert!(w > read_before);
    let ended: Vec<(LineRange, u64)> = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AuditEvent::ClaimEnded {
                claim: id,
                lines,
                version,
                ..
            } if *id == claim.id => Some((lines.clone(), *version)),
            _ => None,
        })
        .collect();
    assert_eq!(
        ended,
        vec![(lines(5, 7), w)],
        "moved and grown, at the version now"
    );
    let (_, ended_version) = ended[0].clone();
    // the version it read before is refused, naming the ClaimEnded version as now
    let stale = run(
        &s,
        agent,
        &[
            "replace",
            "--lines",
            "5,5",
            "--if-version",
            &read_before.to_string(),
        ],
        "GAMMA\n",
    );
    assert_stale(stale, read_before, ended_version, "the pre-claim read");
    // the ClaimEnded version applies, on an immediate edit (inside its reservation)
    let out = run(
        &s,
        agent,
        &[
            "replace",
            "--lines",
            "5,5",
            "--if-version",
            &ended_version.to_string(),
        ],
        "GAMMA\n",
    )
    .unwrap();
    assert_eq!(out.version, ended_version + 1);
    assert_eq!(text(&s, agent).lines().nth(4), Some("GAMMA"));
    // that edit used the reservation: its end carries the range and the version after it
    let res_ended: Vec<(LineRange, u64)> = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AuditEvent::ReservationEnded {
                claim: id,
                how: ReservationEnd::Edited,
                lines,
                version,
                ..
            } if *id == claim.id => Some((lines.clone(), *version)),
            _ => None,
        })
        .collect();
    assert_eq!(res_ended, vec![(lines(5, 7), out.version)]);
}

/// A claim the owner's own edit ends: the event's version is the one after that edit (the
/// version its next read reports), and its lines are the range after it.
#[test]
fn a_claim_ended_by_its_owners_edit_reports_the_version_after_it() {
    let f = fx("claim-edited");
    let s = f.open("d.md");
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let claim = s.claim(bot, lines(2, 3), None, None, false).unwrap();
    insert_at(&s, sam, 0, "top\n");
    let v = read_version(&s, bot);
    let out = run(
        &s,
        bot,
        &["insert", "--after", "3", "--if-version", &v.to_string()],
        "more\n",
    )
    .unwrap();
    let ev = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match e {
            AuditEvent::ClaimEnded {
                claim: id,
                how,
                lines,
                version,
                ..
            } if *id == claim.id => Some((*how, lines.clone(), *version)),
            _ => None,
        })
        .expect("ended");
    assert_eq!(ev.0, ClaimEnd::Edited);
    assert_eq!(ev.2, out.version);
    assert_eq!(ev.2, read_version(&s, bot));
    assert_eq!(ev.1, lines(3, 5), "moved by Sam's line, grown by the bot's");
}

// ---------- `Session::edit_with_options`: the guard and the policy in one public call ----------

/// A host's direct edit route: the text it computed from a read at version V, applied only if
/// the doc is still at V, under the claim policy, both checked under the lock.
fn direct(
    s: &Session,
    c: ConnId,
    opts: &EditOptions,
    new_text: &str,
) -> Result<Option<k2_mpc_base::session::EditResult>, EditWithError<std::convert::Infallible>> {
    let t = new_text.to_owned();
    s.edit_with_options(c, ROOT, opts, move |_| Ok(t))
}

#[test]
fn edit_with_options_applies_at_the_version_and_refuses_a_stale_one() {
    let f = fx("ewo-version");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let v = read_version(&s, agent);
    let mine = DOC.replace("alpha", "ALPHA");
    // at V: applies, one change
    let r = direct(&s, agent, &EditOptions::new().if_version(v), &mine)
        .unwrap()
        .expect("a change");
    assert_eq!(r.version, v + 1);
    assert_eq!(text(&s, agent), mine);
    // at the old V: Stale, nothing applied, no new version, no doc text in the refusal
    let e = direct(&s, agent, &EditOptions::new().if_version(v), DOC).unwrap_err();
    match &e {
        EditWithError::Session(SessionError::Stale { expected, actual }) => {
            assert_eq!((*expected, *actual), (v, v + 1));
        }
        e => panic!("want Stale, got {e:?}"),
    }
    let shown = format!("{e} {e:?}");
    for w in DOC_WORDS {
        assert!(!shown.contains(w), "a refusal quotes the doc: {shown}");
    }
    assert_eq!(text(&s, agent), mine);
    assert_eq!(read_version(&s, agent), v + 1);
    // no guard: as edit_with
    assert!(direct(&s, agent, &EditOptions::new(), DOC)
        .unwrap()
        .is_some());
    assert_eq!(text(&s, agent), DOC);
}

/// Sam's edit lands after the call started, before its critical section: refused, because the
/// version is checked under the lock (a host comparing texts before the call would miss it).
#[test]
fn edit_with_options_race_inside_the_call_is_refused() {
    let f = fx("ewo-race");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    let v = read_version(&s, agent);
    let (tx, rx) = std::sync::mpsc::channel();
    let s2 = s.clone();
    *f.host.inject.lock().unwrap() = Some(Box::new(move || {
        let s3 = s2.clone();
        std::thread::spawn(move || {
            other_edit(&s3, sam);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the injected edit blocked: the clock was read under the session lock");
    }));
    let e = direct(
        &s,
        agent,
        &EditOptions::new().if_version(v),
        &DOC.replace("beta", "BETA"),
    )
    .unwrap_err();
    assert!(f.host.inject.lock().unwrap().is_none(), "the hook ran");
    assert!(
        matches!(e, EditWithError::Session(SessionError::Stale { expected, actual }) if expected == v && actual == v + 1),
        "{e:?}"
    );
    assert!(text(&s, agent).contains("beta two") && text(&s, agent).contains("sam was here"));
}

#[test]
fn edit_with_options_respects_claims_for_agents_only_and_stale_comes_first() {
    let f = fx("ewo-claims");
    let s = f.open("d.md");
    let agent = f.join(&s, "agent", ParticipantKind::Agent);
    let bot = f.join(&s, "bot", ParticipantKind::Agent);
    let sam = f.join(&s, "sam", ParticipantKind::Person);
    s.claim(bot, lines(1, 2), None, None, false).unwrap();
    let v = read_version(&s, agent);
    let mine = DOC.replace("alpha", "A");
    let respect = EditOptions::new().policy(ClaimPolicy::Respect);
    // an agent under Respect at V: the claim refuses (5), nothing applied
    let e = direct(&s, agent, &respect.if_version(v), &mine).unwrap_err();
    assert!(
        matches!(e, EditWithError::Session(SessionError::Claimed { ref by, .. }) if by == "bot"),
        "{e:?}"
    );
    // stale and claimed: the version refuses first
    let e = direct(&s, agent, &respect.if_version(v - 1), &mine).unwrap_err();
    assert!(
        matches!(e, EditWithError::Session(SessionError::Stale { .. })),
        "{e:?}"
    );
    assert_eq!(text(&s, agent), DOC);
    // an edit outside the claim applies under Respect
    let outside = DOC.replace("delta", "D");
    assert!(direct(&s, agent, &respect.if_version(v), &outside)
        .unwrap()
        .is_some());
    // Ignore: the same claimed edit applies; and a person is never refused
    let v = read_version(&s, agent);
    let mine = outside.replace("alpha", "A");
    assert!(direct(&s, agent, &EditOptions::new().if_version(v), &mine)
        .unwrap()
        .is_some());
    let v = read_version(&s, sam);
    assert!(
        direct(&s, sam, &respect.if_version(v), &mine.replace("beta", "B"))
            .unwrap()
            .is_some()
    );
    // the defaults are Ignore and no guard
    assert_eq!(EditOptions::default().policy, ClaimPolicy::Ignore);
    assert_eq!(EditOptions::default().if_version, None);
}
