//! A scripted multiplayer demo: three agents and one person editing one Markdown doc through
//! the kit's sessions. Headless and deterministic (a fixed host clock, fixed names, no
//! fences printed), so its transcript is byte-stable: `tests/demo_transcript.rs` compares it
//! with `tests/golden/multiplayer_demo.txt`.
//!
//! Run it with `cargo run -p k2-mpc-markdown --example multiplayer_demo`. Each scene prints
//! who did what (as a host's doc CLI would be called), what went to stdout and stderr, the
//! exit code, host events, and the doc after. `docs/DEMO.md` maps every scene to the host
//! CLI steps that replay it by hand.
//!
//! The demo checks itself as it goes and fails loudly (exit 1) when anything differs from
//! the script: an exit code, a refusal, a line that must have survived, the final text.
//!
//! It knows nothing of any particular host: the "host" here is a few lines below (rights
//! from a table, a clock that only moves when the script moves it, events kept in a list).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::session::{
    ActivityKind, At, AuditEvent, Blame, ClaimPolicy, Config, ConnId, LineCol, LineRange, Pack,
    Participant, ParticipantKind, Rights, Role, Session, SessionError, SessionManager, Stay,
    UndoOutcome, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore, Seq};
use k2_mpc_base::tools::{ToolOptions, ToolRunError};
use k2_mpc_markdown::{PACK, ROOT};

/// 2026-01-01 00:00:00 UTC: the host clock's start.
const T0: u64 = 1_767_225_600_000;
const DOC: &str = "plan.md";

const INITIAL: &str = "\
# Launch plan

## Goals

- ship the Markdown core
- keep files plain

## Timeline

- week 1: spike
- week 2: engine

## Risks

- memory on large docs (TODO: measure)
- licence not chosen (TODO: ask the owner)

## Notes

Drafted by the team.
TODO: link the spike report.
";

/// The doc at the end of the script.
const FINAL: &str = "\
# Launch plan

## Goals

- ship the Markdown core
- keep files plain
- show agents' presence to people

## Timeline

- week 1: spike (done)
- week 1.5: review the spike
- week 2: engine

## Risks

- memory on large docs (measured: 3x the file size)
- licence not chosen (TODO: ask the owner)
- two agents rewriting one section

## Notes

Drafted by the team; reviewed by B.
TODO: link the spike report.
";

// ---------------------------------------------------------------------------------------
// the host: rights from a table, a scripted clock, events kept for the transcript
// ---------------------------------------------------------------------------------------

struct DemoHost {
    now: AtomicU64,
    events: Mutex<Vec<String>>,
}

impl k2_mpc_base::Host for DemoHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        match who.kind {
            // the person owns the doc (revert of anyone's change)
            ParticipantKind::Person => Rights::full().with_owner(true),
            _ => Rights::full(),
        }
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        // Called with the session locked: only record, never call back into the kit.
        let line = match ev {
            AuditEvent::ClaimTaken {
                claim,
                pid,
                lines,
                expires_ms,
                hold,
                for_edit,
                ..
            } => format!(
                "ClaimTaken {claim} by {pid}: {}, {} s{}{}",
                lines_text(lines.first, lines.last),
                (expires_ms - self.now_ms()) / 1000,
                if *hold { ", hold" } else { "" },
                if *for_edit { ", for-edit" } else { "" },
            ),
            AuditEvent::ClaimEnded {
                claim,
                pid,
                how,
                lines,
                version,
                ..
            } => format!(
                "ClaimEnded {claim} by {pid}: {}; {} free; doc at v{version}",
                how.as_str(),
                lines_text(lines.first, lines.last),
            ),
            _ => return,
        };
        if let Ok(mut v) = self.events.lock() {
            v.push(line);
        }
    }
}

fn lines_text(first: usize, last: usize) -> String {
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    }
}

/// A temp folder for the store, removed at the end (its path is never printed).
struct TmpDir(PathBuf);

impl TmpDir {
    fn new() -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let p = std::env::temp_dir().join(format!("mpc-md-demo-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).map_err(|e| format!("temp dir: {e}"))?;
        Ok(TmpDir(p))
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------------------
// the stage
// ---------------------------------------------------------------------------------------

struct Demo {
    out: String,
    host: Arc<DemoHost>,
    session: Arc<Session>,
    conns: BTreeMap<&'static str, ConnId>,
    // last: the store goes before its folder
    _mgr: SessionManager,
    _dir: TmpDir,
}

type R<T = ()> = Result<T, String>;

/// One verb's result as the transcript shows it.
struct Ran {
    exit: u8,
    version: Seq,
    stdout: String,
}

impl Demo {
    fn new() -> R<Self> {
        let dir = TmpDir::new()?;
        let host = Arc::new(DemoHost {
            now: AtomicU64::new(T0),
            events: Mutex::new(Vec::new()),
        });
        let store = DocStore::open(dir.0.join("docs.redb")).map_err(|e| format!("store: {e}"))?;
        let mgr = SessionManager::new(Arc::new(store), host.clone(), Config::default());
        let pack: Arc<dyn Pack> = Arc::new(PACK);
        let key = DocKey {
            space: "demo".into(),
            path: DOC.into(),
        };
        let session = mgr
            .open(pack, &key, Some(INITIAL.as_bytes()))
            .map_err(|e| format!("open: {e}"))?;
        Ok(Demo {
            out: String::new(),
            host,
            session,
            conns: BTreeMap::new(),
            _mgr: mgr,
            _dir: dir,
        })
    }

    fn say(&mut self, s: impl AsRef<str>) {
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    fn scene(&mut self, n: u32, title: &str) {
        self.say("");
        self.say(format!("=== Scene {n}: {title} ==="));
    }

    /// Narration: what is about to happen and why.
    fn tell(&mut self, s: &str) {
        self.say(format!("-- {s}"));
    }

    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn clock(&self) -> String {
        let s = (self.host.now.load(Ordering::SeqCst) - T0) / 1000;
        format!("[{:02}:{:02}]", s / 60, s % 60)
    }

    fn conn(&self, who: &str) -> R<ConnId> {
        self.conns
            .get(who)
            .copied()
            .ok_or_else(|| format!("{who} has not joined"))
    }

    fn text(&self) -> R<String> {
        let c = self.conn("sam")?;
        let r = self.session.read(c).map_err(|e| format!("read: {e}"))?;
        Ok(r.text_by_root[0].1.clone())
    }

    fn version(&self) -> R<Seq> {
        Ok(self
            .session
            .status()
            .map_err(|e| format!("status: {e}"))?
            .version)
    }

    /// Host events since the last call, one `event:` line each.
    fn events(&mut self) {
        let evs: Vec<String> = self
            .host
            .events
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();
        for e in evs {
            self.say(format!("  event: {e}"));
        }
    }

    fn join(&mut self, pid: &'static str, name: &str, kind: ParticipantKind) -> R {
        let (via, stay, colour) = match kind {
            ParticipantKind::Person => ("tab", Stay::Held, "#e76f51"),
            _ => ("cli", Stay::UntilIdle, "#2a9d8f"),
        };
        let p = Participant::new(pid, name, colour, kind, via);
        let j = if kind == ParticipantKind::Person {
            self.session.join(p, stay)
        } else {
            self.session.join_shared(p, stay)
        }
        .map_err(|e| format!("join {pid}: {e}"))?;
        self.conns.insert(pid, j.conn);
        self.say(format!(
            "{} {pid} joins ({}, via {via}) at v{}",
            self.clock(),
            kind_name(kind),
            j.version
        ));
        Ok(())
    }

    /// Run one doc verb as `who`, the way a host's doc CLI does: the host passes
    /// `ClaimPolicy::Respect` on every edit (its default for agents; a person is never
    /// refused by it). Prints the call, stdout, stderr (summary, notes, for-edit) and the
    /// exit code, and checks the exit code against `want`.
    fn run(&mut self, who: &str, argv: &[&str], stdin: &str, want: u8) -> R<Ran> {
        let conn = self.conn(who)?;
        let shown: Vec<String> = argv.iter().map(|a| quote(a)).collect();
        let mut line = format!("{} {who} $ doc {} {DOC}", self.clock(), shown.join(" "));
        if !stdin.is_empty() {
            let _ = write!(line, " <<< {}", quote(stdin.trim_end_matches('\n')));
        }
        self.say(line);
        let argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
        let opts = ToolOptions::new().policy(ClaimPolicy::Respect);
        let res = self
            .session
            .run_tool_with(conn, ROOT, &argv, stdin.as_bytes(), &opts);
        let ran = match res {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
                for l in stdout.lines() {
                    self.say(format!("  | {l}"));
                }
                self.say(format!("  stderr: {}", o.stderr_summary));
                if let Some(f) = &o.for_edit {
                    self.say(format!("  stderr: {f}"));
                }
                for l in o.notes.lines() {
                    self.say(format!("  stderr: {l}"));
                }
                if o.notes.held > 0 {
                    self.say(format!(
                        "  (host: {} note(s) held back by the rate limit)",
                        o.notes.held
                    ));
                }
                Ran {
                    exit: o.exit,
                    version: o.version,
                    stdout,
                }
            }
            Err(e) => {
                self.say(format!("  stderr: error: {e}"));
                if let ToolRunError::Session(SessionError::Claimed {
                    id,
                    by,
                    lines,
                    remaining_ms,
                    ..
                }) = &e
                {
                    self.say(format!(
                        "  (bounced: {id} by {by}, {}, {remaining_ms} ms left)",
                        lines_text(lines.0, lines.1)
                    ));
                }
                Ran {
                    exit: e.exit(),
                    version: self.version()?,
                    stdout: String::new(),
                }
            }
        };
        self.say(format!("  exit {}", ran.exit));
        self.events();
        if ran.exit != want {
            return Err(format!(
                "{who} doc {}: exit {}, the script expects {want}",
                argv.join(" "),
                ran.exit
            ));
        }
        Ok(ran)
    }

    fn show_doc(&mut self) -> R {
        let text = self.text()?;
        self.say(format!("  {DOC} at v{}:", self.version()?));
        for (i, l) in text.lines().enumerate() {
            self.say(format!("  {:>3}  {l}", i + 1));
        }
        Ok(())
    }

    fn blame(&mut self, who: &str, lines: Option<(usize, usize)>) -> R<Blame> {
        let conn = self.conn(who)?;
        let mut cmd = format!("{} {who} $ doc blame {DOC}", self.clock());
        let range = lines.map(|(a, b)| {
            let _ = write!(cmd, " -L {a},{b}");
            LineRange {
                root: ROOT.into(),
                first: a,
                last: b,
            }
        });
        self.say(cmd);
        let b = self
            .session
            .blame(conn, ROOT, range)
            .map_err(|e| format!("blame: {e}"))?;
        for l in b.render(label).lines() {
            self.say(format!("  | {l}"));
        }
        self.say("  exit 0");
        Ok(b)
    }

    fn undo(&mut self, who: &str, want: u8) -> R<UndoOutcome> {
        let conn = self.conn(who)?;
        self.say(format!("{} {who} $ doc undo {DOC}", self.clock()));
        let o = self
            .session
            .undo(conn, 1)
            .map_err(|e| format!("undo: {e}"))?;
        self.say(format!("  {}", undo_text(&o)));
        self.say(format!("  exit {}", o.exit()));
        self.events();
        if o.exit() != want {
            return Err(format!(
                "{who} undo: exit {}, the script expects {want}",
                o.exit()
            ));
        }
        Ok(o)
    }

    fn must_have(&self, needles: &[&str]) -> R {
        let t = self.text()?;
        for n in needles {
            if !t.contains(n) {
                return Err(format!("lost text: {n:?} is not in the doc:\n{t}"));
            }
        }
        Ok(())
    }

    fn must_lack(&self, needles: &[&str]) -> R {
        let t = self.text()?;
        for n in needles {
            if t.contains(n) {
                return Err(format!("{n:?} should be gone:\n{t}"));
            }
        }
        Ok(())
    }

    fn line_of(&self, needle: &str) -> R<usize> {
        let t = self.text()?;
        t.lines()
            .position(|l| l.contains(needle))
            .map(|i| i + 1)
            .ok_or_else(|| format!("no line with {needle:?}"))
    }
}

fn label(pid: &str) -> Option<String> {
    Some(
        match pid {
            "agent-a" => "Agent A",
            "agent-b" => "Agent B",
            "agent-c" => "Agent C",
            "sam" => "Sam",
            "kit:import" => "(file)",
            _ => return None,
        }
        .to_owned(),
    )
}

fn kind_name(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Person => "person",
        ParticipantKind::Agent => "agent",
        _ => "other",
    }
}

/// Shell-style quoting for the printed command line.
fn quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.,:/=#".contains(c));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''").replace('\n', "\\n"))
    }
}

fn undo_text(o: &UndoOutcome) -> String {
    match o {
        UndoOutcome::Applied {
            steps,
            result,
            skipped,
            ..
        } => format!(
            "undone: {steps} step (+{} −{}) · v{}{}",
            result.lines_added,
            result.lines_removed,
            result.version,
            if skipped.is_empty() {
                String::new()
            } else {
                format!(", {} region(s) skipped", skipped.len())
            }
        ),
        UndoOutcome::Conflicts { conflicts, .. } | UndoOutcome::Blocked { conflicts } => {
            let mut s = String::from("nothing applied: conflicts at");
            for c in conflicts {
                let _ = write!(s, " {} ({:?})", lines_text(c.lines.0, c.lines.1), c.kind);
            }
            s
        }
        UndoOutcome::Nothing => "nothing to undo".into(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------------------
// the script
// ---------------------------------------------------------------------------------------

/// Run the whole script; the transcript, or why it went off script.
pub fn transcript() -> Result<String, String> {
    let mut d = Demo::new()?;
    match script(&mut d) {
        Ok(()) => Ok(std::mem::take(&mut d.out)),
        Err(e) => Err(format!("{e}\n--- the transcript up to there:\n{}", d.out)),
    }
}

fn script(d: &mut Demo) -> R {
    d.say(format!(
        "Multiplayer Markdown demo: 3 agents + 1 person on {DOC} (host clock starts 2026-01-01 00:00:00 UTC)"
    ));
    d.say("Legend: `who $ doc <verb> ...` is one call of the host's doc CLI; `| ` is stdout;");
    d.say("`stderr:` is stderr; `event:` is a host event; agents' edits run under ClaimPolicy::Respect.");

    scene_1(d)?;
    scene_2(d)?;
    let b_change = scene_3(d)?;
    scene_4(d)?;
    scene_5(d)?;
    scene_6(d)?;
    scene_7(d)?;
    scene_8(d, b_change)?;

    let text = d.text()?;
    if text != FINAL {
        return Err(format!(
            "the final text is not the expected one:\n--- got\n{text}--- want\n{FINAL}"
        ));
    }
    d.say("");
    d.say(
        "Checks: every exit code as scripted; no participant's text lost; final text as expected.",
    );
    Ok(())
}

fn scene_1(d: &mut Demo) -> R {
    d.scene(1, "join");
    d.join("agent-a", "Agent A", ParticipantKind::Agent)?;
    d.join("agent-b", "Agent B", ParticipantKind::Agent)?;
    d.join("agent-c", "Agent C", ParticipantKind::Agent)?;
    d.join("sam", "Sam", ParticipantKind::Person)?;
    roster(d, "sam")?;
    d.show_doc()
}

fn roster(d: &mut Demo, who: &str) -> R {
    let conn = d.conn(who)?;
    d.say(format!("{} {who} $ doc who {DOC}", d.clock()));
    let r = d.session.roster(conn).map_err(|e| format!("roster: {e}"))?;
    for e in r {
        let mut s = format!(
            "  | {} ({}) {} {}",
            e.participant.name,
            e.participant.pid,
            kind_name(e.participant.kind),
            match e.role {
                Role::Editor => "editing",
                _ => "viewing",
            }
        );
        if let Some(v) = &e.view {
            let _ = write!(s, ", looking at {}", lines_text(v.first, v.last));
        }
        if let Some(c) = &e.cursor {
            let _ = write!(s, ", caret at line {}", c.head.line);
        }
        for c in &e.claims {
            let _ = write!(
                s,
                ", claim {} on {}",
                c.id,
                lines_text(c.lines.first, c.lines.last)
            );
        }
        d.say(s);
    }
    d.say("  exit 0");
    Ok(())
}

fn scene_2(d: &mut Demo) -> R {
    d.scene(2, "presence: reads, carets and heads-up notes");
    d.tell("agent-a reads Goals, agent-b reads Timeline: different sections, no notes.");
    d.advance(1_000);
    d.run("agent-a", &["view", "--lines", "3,6"], "", 0)?;
    d.advance(1_000);
    d.run("agent-b", &["view", "--lines", "8,11"], "", 0)?;
    d.tell("Sam puts a caret in Risks (line 15) from the editor.");
    d.advance(1_000);
    let sam = d.conn("sam")?;
    let at = At::Line(LineCol { line: 15, col: 3 });
    d.session
        .set_cursor(sam, ROOT, at, at)
        .map_err(|e| format!("cursor: {e}"))?;
    d.say(format!(
        "{} sam moves the caret to line 15 (editor)",
        d.clock()
    ));
    d.tell("agent-c reads lines 1–11, overlapping both agents' views: two notes on stderr.");
    d.advance(1_000);
    d.run("agent-c", &["view", "--lines", "1,11"], "", 0)?;
    d.tell("agent-c greps the TODOs 1 s later: Sam's caret is there, but the rate limit (one batch per 5 s) holds the note back.");
    d.advance(1_000);
    d.run("agent-c", &["grep", "-n", "TODO"], "", 0)?;
    d.tell("5 s later the same grep gets the held note.");
    d.advance(5_000);
    d.run("agent-c", &["grep", "-n", "TODO"], "", 0)?;
    d.tell("Sam asks what the agents have been doing.");
    d.advance(1_000);
    activity(d, "sam", 10)?;
    roster(d, "sam")
}

fn activity(d: &mut Demo, who: &str, n: usize) -> R {
    let conn = d.conn(who)?;
    d.say(format!("{} {who} $ doc activity -n {n} {DOC}", d.clock()));
    let rows = d
        .session
        .activity(conn, n)
        .map_err(|e| format!("activity: {e}"))?;
    for a in rows {
        let at = (a.at_ms - T0) / 1000;
        let mut s = format!(
            "  | [{:02}:{:02}] {} {}",
            at / 60,
            at % 60,
            a.pid,
            match a.kind {
                ActivityKind::ClaimEnded(how) => format!("claim-ended ({})", how.as_str()),
                k => k.as_str().to_owned(),
            }
        );
        let _ = write!(s, " {}", a.verb);
        if let Some(l) = &a.lines {
            let _ = write!(s, " {}", lines_text(l.first, l.last));
        }
        if let Some(c) = a.claim {
            let _ = write!(s, " ({c})");
        }
        d.say(s);
    }
    d.say("  exit 0");
    Ok(())
}

/// Returns agent-b's Timeline change (scene 8 reverts it).
fn scene_3(d: &mut Demo) -> R<Seq> {
    d.scene(3, "claims, a bounce, and a retry with --if-version");
    d.tell("agent-a reads Timeline for editing: an implicit 30 s claim on the lines shown.");
    d.advance(2_000);
    d.run("agent-a", &["view", "--lines", "8,11", "--for-edit"], "", 0)?;
    d.tell("agent-b reads the doc (v1), then edits inside agent-a's claim: bounced, exit 5.");
    d.advance(1_000);
    let read = d.run("agent-b", &["view", "--lines", "10,11"], "", 0)?;
    let b_read = read.version;
    d.advance(1_000);
    d.run(
        "agent-b",
        &[
            "replace",
            "--old",
            "- week 2: engine",
            "--new",
            "- week 2: sync protocol",
        ],
        "",
        5,
    )?;
    d.tell(
        "Sam types a line inside the same claim: a person is never refused, whatever the policy.",
    );
    d.advance(1_000);
    d.run(
        "sam",
        &["insert", "--after", "10"],
        "- week 1.5: review the spike\n",
        0,
    )?;
    d.tell("agent-a's own edit lands; its claim ends (edited) with the lines and the version.");
    d.advance(2_000);
    d.run(
        "agent-a",
        &[
            "replace",
            "--old",
            "- week 1: spike",
            "--new",
            "- week 1: spike (done)",
        ],
        "",
        0,
    )?;
    d.tell(&format!(
        "agent-b retries guarded by the version it read (v{b_read}): the doc moved on, so it is refused (exit 1)."
    ));
    d.advance(1_000);
    let v = b_read.to_string();
    let args = [
        "replace",
        "--old",
        "- week 2: engine",
        "--new",
        "- week 2: sync protocol",
        "--if-version",
        v.as_str(),
    ];
    d.run("agent-b", &args, "", 1)?;
    d.tell("agent-b re-reads (as the 'free now' message says) and retries with --if-version: it lands.");
    d.advance(1_000);
    let read = d.run("agent-b", &["view", "--lines", "10,12"], "", 0)?;
    let v = read.version.to_string();
    d.advance(1_000);
    let args = [
        "replace",
        "--old",
        "- week 2: engine",
        "--new",
        "- week 2: sync protocol",
        "--if-version",
        v.as_str(),
    ];
    let ran = d.run("agent-b", &args, "", 0)?;
    d.must_have(&[
        "- week 1: spike (done)",
        "- week 1.5: review the spike",
        "- week 2: sync protocol",
    ])?;
    d.show_doc()?;
    Ok(ran.version)
}

fn scene_4(d: &mut Demo) -> R {
    d.scene(4, "write --base: a clean three-way merge, then a conflict");
    d.tell("agent-c reads the whole doc, then works from that read.");
    d.advance(2_000);
    let base = d.run("agent-c", &["cat"], "", 0)?;
    let stale = base.stdout.clone();
    d.tell("Meanwhile Sam adds a goal from the editor.");
    d.advance(1_000);
    let sam = d.conn("sam")?;
    d.session
        .edit_with_policy(sam, ROOT, ClaimPolicy::Respect, |t: &str| {
            Ok::<_, String>(t.replace(
                "- keep files plain\n",
                "- keep files plain\n- show agents' presence to people\n",
            ))
        })
        .map_err(|e| format!("sam's edit: {e}"))?;
    d.say(format!(
        "{} sam types '- show agents' presence to people' after line 6 (editor) · v{}",
        d.clock(),
        d.version()?
    ));
    d.tell(&format!(
        "agent-c writes its whole rewrite of Risks with --base v{}: only its own change applies; Sam's goal stays.",
        base.version
    ));
    d.advance(2_000);
    let ours = stale.replace(
        "- licence not chosen (TODO: ask the owner)\n",
        "- licence not chosen (TODO: ask the owner)\n- two agents rewriting one section\n",
    );
    write_base(d, "agent-c", base.version, &ours, 0)?;
    d.must_have(&[
        "- show agents' presence to people",
        "- two agents rewriting one section",
    ])?;

    d.tell("Conflict: agent-b claims the Notes line (hold) and edits it; agent-c then rewrites the same line from an older read.");
    d.advance(2_000);
    let base = d.run("agent-c", &["cat"], "", 0)?;
    let stale = base.stdout.clone();
    d.advance(1_000);
    let notes_line = d.line_of("Drafted by the team.")?;
    claim(d, "agent-b", notes_line, notes_line, "tightening the notes")?;
    d.advance(1_000);
    d.run(
        "agent-b",
        &[
            "replace",
            "--old",
            "Drafted by the team.",
            "--new",
            "Drafted by the team; reviewed by B.",
        ],
        "",
        0,
    )?;
    d.advance(2_000);
    let ours = stale.replace("Drafted by the team.", "Drafted by the team in 2026.");
    let before = d.version()?;
    write_base(d, "agent-c", base.version, &ours, 1)?;
    if d.version()? != before {
        return Err("a conflicting write --base applied something".into());
    }
    d.must_have(&["Drafted by the team; reviewed by B."])?;
    d.tell("agent-b releases its claim; agent-c would re-read and redo its change by hand.");
    d.advance(1_000);
    let b = d.conn("agent-b")?;
    let ids = d
        .session
        .release_claim(b, None)
        .map_err(|e| format!("release: {e}"))?;
    d.say(format!("{} agent-b $ doc claim --release {DOC}", d.clock()));
    d.say(format!("  released {}", ids.len()));
    d.say("  exit 0");
    d.events();
    d.show_doc()
}

fn claim(d: &mut Demo, who: &str, first: usize, last: usize, note: &str) -> R {
    let conn = d.conn(who)?;
    d.say(format!(
        "{} {who} $ doc claim --lines {first},{last} --ttl 120000 --hold --note {} {DOC}",
        d.clock(),
        quote(note)
    ));
    let c = d
        .session
        .claim(
            conn,
            LineRange {
                root: ROOT.into(),
                first,
                last,
            },
            Some(120_000),
            Some(note.to_owned()),
            true,
        )
        .map_err(|e| format!("claim: {e}"))?;
    d.say(format!(
        "  claimed {} for {} s ({})",
        lines_text(c.lines.first, c.lines.last),
        c.remaining_ms / 1000,
        c.id
    ));
    d.say("  exit 0");
    d.events();
    Ok(())
}

fn write_base(d: &mut Demo, who: &str, base: Seq, text: &str, want: u8) -> R {
    let conn = d.conn(who)?;
    d.say(format!(
        "{} {who} $ doc write --base {base} {DOC} < (its edited copy)",
        d.clock()
    ));
    let rep = d
        .session
        .write_base_with(conn, ROOT, base, text, ClaimPolicy::Respect)
        .map_err(|e| format!("write --base: {e}"))?;
    match &rep.outcome {
        WriteOutcome::Applied(r) => {
            let ch: Vec<String> = r
                .changed
                .iter()
                .map(|l| lines_text(l.first, l.last))
                .collect();
            d.say(format!(
                "  stderr: merged; edited {DOC} {} (+{} −{}) · v{}",
                ch.join(", "),
                r.lines_added,
                r.lines_removed,
                r.version
            ));
            for l in rep.notes.lines() {
                d.say(format!("  stderr: {l}"));
            }
        }
        WriteOutcome::Conflicts(cs) => {
            for (i, c) in cs.iter().enumerate() {
                d.say(format!(
                    "  stderr: conflict at {} of v{base}: yours {:?}, now {:?}",
                    lines_text(c.lines.0, c.lines.1),
                    c.ours.trim_end_matches('\n'),
                    c.theirs.trim_end_matches('\n'),
                ));
                for (k, n) in &rep.conflict_notes {
                    if *k == i {
                        d.say(format!("  stderr: {n}"));
                    }
                }
            }
            d.say("  stderr: nothing applied; re-read and retry");
        }
        WriteOutcome::BaseExpired => d.say("  stderr: base expired; re-read and retry"),
    }
    let exit = rep.outcome.exit();
    d.say(format!("  exit {exit}"));
    d.events();
    if exit != want {
        return Err(format!(
            "{who} write --base: exit {exit}, the script expects {want}"
        ));
    }
    Ok(())
}

fn scene_5(d: &mut Demo) -> R {
    d.scene(5, "exact replace: unique, then ambiguous");
    d.tell("agent-a replaces one exact piece of text: one match, it applies.");
    d.advance(2_000);
    d.run(
        "agent-a",
        &[
            "replace",
            "--old",
            "TODO: measure",
            "--new",
            "measured: 3x the file size",
        ],
        "",
        0,
    )?;
    d.tell("agent-b tries 'TODO' alone: two matches, refused (exit 1) with their line numbers; nothing applied.");
    d.advance(2_000);
    let before = d.version()?;
    d.run(
        "agent-b",
        &["replace", "--old", "TODO", "--new", "DONE"],
        "",
        1,
    )?;
    if d.version()? != before {
        return Err("an ambiguous replace applied something".into());
    }
    d.show_doc()
}

fn scene_6(d: &mut Demo) -> R {
    d.scene(6, "undo with provenance");
    d.tell("Each agent adds a line to Notes, one after the other.");
    for (who, line) in [
        ("agent-a", "- A: links checked\n"),
        ("agent-b", "- B: spelling checked\n"),
        ("agent-c", "- C: dates checked\n"),
    ] {
        d.advance(2_000);
        d.run(who, &["append"], line, 0)?;
    }
    d.tell("Each undoes its own line, in the order a, c, b: the others' lines survive every undo.");
    d.advance(2_000);
    d.undo("agent-a", 0)?;
    d.must_lack(&["- A: links checked"])?;
    d.must_have(&["- B: spelling checked", "- C: dates checked"])?;
    d.advance(2_000);
    d.undo("agent-c", 0)?;
    d.must_lack(&["- C: dates checked"])?;
    d.must_have(&["- B: spelling checked"])?;
    d.advance(2_000);
    d.undo("agent-b", 0)?;
    d.must_lack(&["- B: spelling checked"])?;
    d.must_have(&[
        "Drafted by the team; reviewed by B.",
        "- two agents rewriting one section",
    ])?;

    d.tell("agent-a writes a sentence; agent-b edits inside it, then undoes that.");
    d.advance(2_000);
    d.run(
        "agent-a",
        &["append"],
        "Sync runs over one socket per doc.\n",
        0,
    )?;
    d.advance(2_000);
    d.run(
        "agent-b",
        &["replace", "--old", "one socket", "--new", "a shared socket"],
        "",
        0,
    )?;
    let l = d.line_of("Sync runs over")?;
    d.advance(1_000);
    d.blame("sam", Some((l, l)))?;
    d.advance(1_000);
    d.undo("agent-b", 0)?;
    d.must_have(&["Sync runs over one socket per doc."])?;
    d.tell("Blame names agent-a, who first wrote the text; agent-b put 'one' back.");
    d.advance(1_000);
    let b = d.blame("sam", Some((l, l)))?;
    restored(d, &b, "agent-a", "agent-b")?;
    d.tell("agent-a's undo still applies: the text agent-b put back counts as agent-a's.");
    d.advance(2_000);
    d.undo("agent-a", 0)?;
    d.must_lack(&["Sync runs over"])?;

    d.tell("Sam deletes a whole line by mistake, then presses Ctrl+Z: the line comes back as the file's.");
    d.advance(2_000);
    let sam = d.conn("sam")?;
    d.session
        .edit_with_policy(sam, ROOT, ClaimPolicy::Respect, |t: &str| {
            Ok::<_, String>(t.replace("TODO: link the spike report.\n", ""))
        })
        .map_err(|e| format!("sam's delete: {e}"))?;
    d.say(format!(
        "{} sam deletes the line 'TODO: link the spike report.' (editor) · v{}",
        d.clock(),
        d.version()?
    ));
    d.advance(1_000);
    d.undo("sam", 0)?;
    d.must_have(&["TODO: link the spike report.\n"])?;
    let l = d.line_of("TODO: link the spike report.")?;
    d.advance(1_000);
    let b = d.blame("sam", Some((l, l)))?;
    restored(d, &b, "kit:import", "sam")?;
    d.show_doc()
}

/// Blame by root: the line's author is the text's first author, and its `restored_by`
/// names who put it back, also when only part of the line was put back (base v0.9.1 fixed
/// finding F1).
fn restored(_d: &mut Demo, b: &Blame, author: &str, by: &str) -> R {
    let line = b.lines.first().ok_or("blame: no line")?;
    if line.author != author {
        return Err(format!(
            "blame by root: author {}, the script expects {author}",
            line.author
        ));
    }
    match line.restored_by.as_deref() {
        Some(r) if r == by => Ok(()),
        None => Err(format!(
            "blame: no 'restored by {by}' (finding F1, fixed in base v0.9.1)"
        )),
        Some(r) => Err(format!("blame: restored by {r}, the script expects {by}")),
    }
}

fn scene_7(d: &mut Demo) -> R {
    d.scene(7, "blame");
    d.advance(2_000);
    d.blame("sam", None)?;
    Ok(())
}

fn scene_8(d: &mut Demo, b_change: Seq) -> R {
    d.scene(8, "revert");
    d.tell(&format!(
        "Sam (an owner) reverts agent-b's Timeline change, v{b_change}, from scene 3. The text it replaced comes back."
    ));
    d.advance(2_000);
    let sam = d.conn("sam")?;
    d.say(format!("{} sam $ doc revert {b_change} {DOC}", d.clock()));
    let o = d
        .session
        .revert(sam, b_change, false)
        .map_err(|e| format!("revert: {e}"))?;
    d.say(format!(
        "  {}",
        undo_text(&o).replacen("undone", "reverted", 1)
    ));
    d.say(format!("  exit {}", o.exit()));
    d.events();
    if o.exit() != 0 {
        return Err(format!("revert: exit {}, the script expects 0", o.exit()));
    }
    d.must_have(&["- week 2: engine\n", "- week 1.5: review the spike"])?;
    let l = d.line_of("- week 2: engine")?;
    d.advance(1_000);
    let b = d.blame("sam", Some((l, l)))?;
    restored(d, &b, "kit:import", "sam")?;
    d.show_doc()
}

#[allow(dead_code)] // the test includes this file for `transcript` only
fn main() {
    match transcript() {
        Ok(t) => print!("{t}"),
        Err(e) => {
            eprintln!("demo went off script: {e}");
            std::process::exit(1);
        }
    }
}
