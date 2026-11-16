//! A live session over an OTIO file (the kit's session manager, a store, a host that grants
//! everyone everything), the timelines the live rows use (written here, in the reference
//! writer's layout, never taken from another program's files), and the rows themselves
//! ([`rows`]), shared by `tests/live.rs` and the planted bugs in `tests/mutants.rs`.
#![allow(dead_code)]

pub mod rows;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, PlayState, Rights, Session,
    SessionError, SessionManager, Stay, TimePosition,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{ToolOutput, ToolRunError};
use k2_mpc_timeline::live::{TimelineLive, ITEMS};
use k2_mpc_timeline::mutant;
use k2_mpc_timeline::otio::CONVERTER_DF_KEY;

pub struct TestHost {
    pub now: AtomicU64,
    pub events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

pub struct Fx {
    pub dir: PathBuf,
    pub ws: PathBuf,
    pub host: Arc<TestHost>,
    pub store: Arc<DocStore>,
    pub mgr: SessionManager,
    pub pack: Arc<TimelineLive>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub const NAME: &str = "cut.otio";

/// A fixture with the kit's default config, as `set` changes it.
pub fn fx(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2tl-live-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c);
    Fx {
        dir: p,
        ws,
        host,
        store,
        mgr,
        pack: Arc::new(TimelineLive::default()),
    }
}

pub fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: NAME.into(),
    }
}

impl Fx {
    pub fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    pub fn write(&self, bytes: &[u8]) {
        fs::write(self.path(), bytes).unwrap();
    }
    pub fn bytes(&self) -> Vec<u8> {
        fs::read(self.path()).unwrap()
    }
    pub fn try_open(&self) -> Result<Arc<Session>, SessionError> {
        self.mgr
            .open_file(self.pack.clone(), &key(), &self.path(), &self.ws)
    }
    pub fn open(&self) -> Arc<Session> {
        self.try_open().expect("the timeline opens live")
    }
    pub fn join(&self, s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
        let conn = s
            .join(
                Participant::new(pid, pid, "#123456", kind, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        if kind == ParticipantKind::Person {
            PEOPLE.with(|p| p.borrow_mut().push((session_id(s), conn)));
        }
        conn
    }
    pub fn person(&self, s: &Session) -> ConnId {
        self.join(s, "ada", ParticipantKind::Person)
    }
    pub fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "atlas", ParticipantKind::Agent)
    }
    /// A restart: a new manager on the same store and a new pack, the old manager dropped
    /// without a write-back. Drop every session of the old manager first.
    pub fn restart(&mut self) {
        self.mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        );
        self.pack = Arc::new(TimelineLive::default());
    }
    pub fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
}

/// An app's save: written beside the file, then renamed over it.
pub fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

pub fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// Run a verb as `c`, with no base.
pub fn run(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    s.run_tool(c, ITEMS, &args(a), b"")
}

/// Run an edit verb as `c`, with the version of a read it just made as its base; then the
/// host's side of the call ([`host_after_call`]).
pub fn run_based(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    let v = run(s, c, &["timeline", "info"])?.version;
    let mut all = args(a);
    all.push("--base".into());
    all.push(v.to_string());
    let out = s.run_tool(c, ITEMS, &all, b"")?;
    host_after_call(s, c, &out);
    Ok(out)
}

thread_local! {
    /// The people each session has, by [`session_id`], as [`Fx::join`] joined them: for the
    /// host's planted presence bugs ([`host_after_call`]).
    static PEOPLE: RefCell<Vec<(usize, ConnId)>> = const { RefCell::new(Vec::new()) };
}

/// A session's identity while it is open: its address.
fn session_id(s: &Session) -> usize {
    std::ptr::from_ref(s) as usize
}

/// The host's side of an edit call. A host moves nobody's playhead after a verb (the kit moves
/// the acting agent's, on success only), so this does nothing; the planted bugs
/// (`tests/mutants.rs`) are a host's mistakes at this seam, where a pack's verb can't reach:
/// `HostMirrorsToPeople` copies an agent's playhead onto every person in the session after a
/// call that worked; `HostMovesOnRefusal` puts the caller's playhead at zero after a call that
/// didn't.
fn host_after_call(s: &Session, c: ConnId, out: &ToolOutput) {
    if mutant::active("HostMirrorsToPeople") && out.exit == 0 {
        let agents = s
            .roster(c)
            .ok()
            .and_then(|r| {
                r.into_iter()
                    .find(|e| e.participant.kind == ParticipantKind::Agent && e.time.is_some())
            })
            .and_then(|e| e.time);
        if let Some(t) = agents {
            let id = session_id(s);
            let people: Vec<ConnId> = PEOPLE.with(|p| {
                p.borrow()
                    .iter()
                    .filter(|(x, _)| *x == id)
                    .map(|(_, c)| *c)
                    .collect()
            });
            for p in people {
                let _ = s.set_time_position(p, Some(t.clone()));
            }
        }
    }
    if mutant::active("HostMovesOnRefusal") && out.exit != 0 {
        let zero = TimePosition::new("0", PlayState::Paused).ok();
        let _ = s.set_time_position(c, zero);
    }
}

pub fn stdout(o: &ToolOutput) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A field of the live store: `root`'s map, key `<elem>\x1f<field>`.
pub fn field(content: &[(String, Neutral)], root: &str, elem: &str, fld: &str) -> Option<String> {
    let k = format!("{elem}\x1f{fld}");
    content.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == root => m.get(&k).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    })
}

/// Set a field of `content` (a composite map's `<elem>\x1f<field>`), as an edit would.
pub fn set_field(content: &mut [(String, Neutral)], root: &str, elem: &str, fld: &str, v: &str) {
    for (r, n) in content.iter_mut() {
        if let (true, Neutral::Map(m)) = (r == root, n) {
            m.insert(format!("{elem}\x1f{fld}"), Value::str(v));
        }
    }
}

/// A list root's `(key, value)`s.
pub fn list(content: &[(String, Neutral)], root: &str) -> Vec<(String, String)> {
    content
        .iter()
        .find_map(|(r, n)| match n {
            Neutral::List(l) if r == root => Some(
                l.iter()
                    .map(|(k, v)| (k.as_str().to_owned(), v.as_str().unwrap_or("").to_owned()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The key of the element of `root` whose `name` field (in `props`) is `name`.
pub fn named(content: &[(String, Neutral)], root: &str, props: &str, name: &str) -> Option<String> {
    list(content, root)
        .into_iter()
        .map(|(k, _)| k)
        .find(|k| field(content, props, k, "name").as_deref() == Some(name))
}

// ---------- an OTIO writer of our own, in the reference writer's layout ----------

/// A JSON value for the generator; numbers are tokens as written.
#[derive(Debug, Clone)]
pub enum J {
    Obj(Vec<(&'static str, J)>),
    Arr(Vec<J>),
    Str(String),
    Raw(String),
}

fn q(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `v` printed at nesting `level`: four spaces a level, `": "` after a key, empty objects and
/// arrays as `{}` and `[]`.
pub fn pretty(v: &J, level: usize) -> String {
    let pad = |n: usize| "    ".repeat(n);
    match v {
        J::Str(s) => q(s),
        J::Raw(r) => r.clone(),
        J::Obj(kv) if kv.is_empty() => "{}".into(),
        J::Arr(a) if a.is_empty() => "[]".into(),
        J::Obj(kv) => {
            let inner: Vec<String> = kv
                .iter()
                .map(|(k, v)| format!("{}{}: {}", pad(level + 1), q(k), pretty(v, level + 1)))
                .collect();
            format!("{{\n{}\n{}}}", inner.join(",\n"), pad(level))
        }
        J::Arr(a) => {
            let inner: Vec<String> = a
                .iter()
                .map(|v| format!("{}{}", pad(level + 1), pretty(v, level + 1)))
                .collect();
            format!("[\n{}\n{}]", inner.join(",\n"), pad(level))
        }
    }
}

fn s(x: &str) -> J {
    J::Str(x.into())
}

fn raw(x: impl Into<String>) -> J {
    J::Raw(x.into())
}

/// A `RationalTime.1`: `value` and `rate` as tokens.
pub fn rt(value: &str, rate: &str) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("RationalTime.1")),
        ("rate", raw(rate)),
        ("value", raw(value)),
    ])
}

/// A `TimeRange.1`.
pub fn tr(start: &str, dur: &str, rate: &str) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("TimeRange.1")),
        ("duration", rt(dur, rate)),
        ("start_time", rt(start, rate)),
    ])
}

/// A marker: name, start and duration tokens at `rate`, colour, comment.
pub fn marker(name: &str, start: &str, dur: &str, rate: &str, color: &str, comment: &str) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("Marker.2")),
        ("metadata", J::Obj(vec![])),
        ("name", s(name)),
        ("color", s(color)),
        ("marked_range", tr(start, dur, rate)),
        ("comment", s(comment)),
    ])
}

/// What a clip is.
#[derive(Debug, Clone)]
pub struct ClipSpec {
    pub name: String,
    pub url: String,
    pub src: String,
    pub dur: String,
    pub rate: String,
    pub avail: Option<(String, String)>,
    pub clip2: bool,
    pub enabled: bool,
    pub markers: Vec<J>,
    pub metadata: Vec<(&'static str, J)>,
}

impl ClipSpec {
    pub fn new(name: &str, url: &str, src: &str, dur: &str, rate: &str) -> Self {
        ClipSpec {
            name: name.into(),
            url: url.into(),
            src: src.into(),
            dur: dur.into(),
            rate: rate.into(),
            avail: Some(("0.0".into(), "100000.0".into())),
            clip2: true,
            enabled: true,
            markers: Vec::new(),
            metadata: Vec::new(),
        }
    }
}

pub fn clip(c: &ClipSpec) -> J {
    let reference = J::Obj(vec![
        ("OTIO_SCHEMA", s("ExternalReference.1")),
        ("metadata", J::Obj(vec![])),
        ("name", s("")),
        (
            "available_range",
            match &c.avail {
                Some((a, b)) => tr(a, b, &c.rate),
                None => raw("null"),
            },
        ),
        ("available_image_bounds", raw("null")),
        ("target_url", s(&c.url)),
    ]);
    let mut kv = vec![
        ("OTIO_SCHEMA", s(if c.clip2 { "Clip.2" } else { "Clip.1" })),
        ("metadata", J::Obj(c.metadata.clone())),
        ("name", s(&c.name)),
        ("source_range", tr(&c.src, &c.dur, &c.rate)),
        ("effects", J::Arr(vec![])),
        ("markers", J::Arr(c.markers.clone())),
        ("enabled", raw(if c.enabled { "true" } else { "false" })),
    ];
    if c.clip2 {
        kv.push((
            "media_references",
            J::Obj(vec![("DEFAULT_MEDIA", reference)]),
        ));
        kv.push(("active_media_reference_key", s("DEFAULT_MEDIA")));
    } else {
        kv.push(("media_reference", reference));
    }
    J::Obj(kv)
}

pub fn gap(dur: &str, rate: &str) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("Gap.1")),
        ("metadata", J::Obj(vec![])),
        ("name", s("")),
        ("source_range", tr("0.0", dur, rate)),
        ("effects", J::Arr(vec![])),
        ("markers", J::Arr(vec![])),
        ("enabled", raw("true")),
    ])
}

pub fn transition(inn: &str, out: &str, rate: &str) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("Transition.1")),
        ("metadata", J::Obj(vec![])),
        ("name", s("dissolve")),
        ("in_offset", rt(inn, rate)),
        ("out_offset", rt(out, rate)),
        ("transition_type", s("SMPTE_Dissolve")),
    ])
}

pub fn track(name: &str, kind: &str, children: Vec<J>, markers: Vec<J>) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("Track.1")),
        ("metadata", J::Obj(vec![])),
        ("name", s(name)),
        ("source_range", raw("null")),
        ("effects", J::Arr(vec![])),
        ("markers", J::Arr(markers)),
        ("enabled", raw("true")),
        ("children", J::Arr(children)),
        ("kind", s(kind)),
    ])
}

pub fn timeline(name: &str, start: Option<(&str, &str)>, tracks: Vec<J>, markers: Vec<J>) -> J {
    J::Obj(vec![
        ("OTIO_SCHEMA", s("Timeline.1")),
        ("metadata", J::Obj(vec![])),
        ("name", s(name)),
        (
            "global_start_time",
            match start {
                Some((v, r)) => rt(v, r),
                None => raw("null"),
            },
        ),
        (
            "tracks",
            J::Obj(vec![
                ("OTIO_SCHEMA", s("Stack.1")),
                ("metadata", J::Obj(vec![])),
                ("name", s("tracks")),
                ("source_range", raw("null")),
                ("effects", J::Arr(vec![])),
                ("markers", J::Arr(markers)),
                ("enabled", raw("true")),
                ("children", J::Arr(tracks)),
            ]),
        ),
    ])
}

/// The nesting level a clip object prints at: root, stack, its children, a track, its
/// children.
pub const CLIP_LEVEL: usize = 5;

/// The file: `v` printed, with a final newline.
pub fn file(v: &J) -> Vec<u8> {
    let mut o = pretty(v, 0);
    o.push('\n');
    o.into_bytes()
}

/// The clips of [`cut`], so a row can find their bytes.
pub fn cut_clips() -> Vec<ClipSpec> {
    let mut a = ClipSpec::new(
        "Shot A",
        "file:///media/shot_a.mov",
        "100.0",
        "48.0",
        "24.0",
    );
    a.markers = vec![marker(
        "beat",
        "110.0",
        "0.0",
        "24.0",
        "RED",
        "on the downbeat",
    )];
    let mut b = ClipSpec::new("Shot B", "file:///media/shot_b.mov", "0.0", "24.0", "24.0");
    b.clip2 = false;
    let c = ClipSpec::new("Shot C", "file:///media/shot_c.mov", "10.0", "36.0", "24.0");
    let d = ClipSpec::new(
        "Shot A again",
        "file:///media/shot_a.mov",
        "200.0",
        "24.0",
        "24.0",
    );
    let mut m = ClipSpec::new(
        "Music",
        "file:///media/music.wav",
        "0.0",
        "96000.0",
        "48000.0",
    );
    m.avail = Some(("0.0".into(), "4800000.0".into()));
    vec![a, b, c, d, m]
}

/// The timeline most rows use, at 24 fps from 01:00:00:00:
/// - `V1`: a gap of 24 frames; Shot A (48 frames, a marker); Shot B (24, a `Clip.1`);
///   a dissolve across the cut to Shot C (36); a gap of 12; Shot A again (24); a trailing gap
///   of 24; a track marker;
/// - `A1`: Music (2 s at 48 kHz) from zero;
/// - `V2`: empty;
/// - a timeline marker at frame 48.
pub fn cut() -> Vec<u8> {
    let c = cut_clips();
    file(&cut_tree(&c))
}

pub fn cut_tree(c: &[ClipSpec]) -> J {
    timeline(
        "Cut",
        Some(("86400.0", "24.0")),
        vec![
            track(
                "V1",
                "Video",
                vec![
                    gap("24.0", "24.0"),
                    clip(&c[0]),
                    clip(&c[1]),
                    transition("6.0", "6.0", "24.0"),
                    clip(&c[2]),
                    gap("12.0", "24.0"),
                    clip(&c[3]),
                    gap("24.0", "24.0"),
                ],
                vec![marker("act two", "120.0", "0.0", "24.0", "BLUE", "")],
            ),
            track("A1", "Audio", vec![clip(&c[4])], vec![]),
            track("V2", "Video", vec![], vec![]),
        ],
        vec![marker("chapter", "48.0", "0.0", "24.0", "GREEN", "")],
    )
}

/// A one-track timeline: One for `dur` frames from source frame 0, a gap of `dur` frames, then
/// Two (from source frame `dur / 2`), at the rate written `rate`, starting at frame `start`.
fn two_clips(start: &str, rate: &str, dur: &str, two_in: &str) -> J {
    let a = ClipSpec::new("One", "file:///media/one.mov", "0.0", dur, rate);
    let b = ClipSpec::new("Two", "file:///media/two.mov", two_in, dur, rate);
    timeline(
        "Decimal",
        Some((start, rate)),
        vec![track(
            "V1",
            "Video",
            vec![clip(&a), gap(dur, rate), clip(&b)],
            vec![],
        )],
        vec![],
    )
}

/// The timeline of [`decimal_rate`].
pub fn decimal_tree() -> J {
    two_clips("107892.0", "29.97", "30.0", "15.0")
}

/// A timeline whose clips are at 29.97 written `29.97` (a decimal rate token), values in the
/// `86.0` style.
pub fn decimal_rate() -> Vec<u8> {
    file(&decimal_tree())
}

/// [`decimal_rate`] at 59.94 written `59.94`, from frame 215784 (01:00:00;00 drop-frame at
/// `60000/1001`): One for 60 frames, a 60-frame gap, then Two.
pub fn decimal_tree_5994() -> J {
    two_clips("215784.0", "59.94", "60.0", "30.0")
}

/// The same at 25 fps from frame 90000 (01:00:00:00): One for 25 frames, a 25-frame gap, then
/// Two.
pub fn tree_25() -> J {
    two_clips("90000.0", "25.0", "25.0", "12.0")
}

/// `tl` (a timeline object) with its `metadata` holding a third-party converter's drop-frame
/// (DF/NDF) entry, `value` as written (any JSON value), under the key the pack reads
/// (`otio::CONVERTER_DF_KEY`, its parts split at `/`).
pub fn with_df_entry(tl: J, value: J) -> J {
    let mut v = value;
    for k in CONVERTER_DF_KEY.rsplit('/') {
        v = J::Obj(vec![(k, v)]);
    }
    match tl {
        J::Obj(mut kv) => {
            match kv.iter_mut().find(|(x, _)| *x == "metadata") {
                Some((_, old)) => *old = v,
                None => kv.push(("metadata", v)),
            }
            J::Obj(kv)
        }
        other => other,
    }
}

/// A string as JSON.
pub fn js(x: &str) -> J {
    s(x)
}

/// Raw JSON text.
pub fn jraw(x: &str) -> J {
    raw(x)
}

/// A timeline whose clip's source time is a time of day past 2^53 ticks: 24:00:00:00 at
/// 30000/1001 (`2592000` frames), and a second at 4096 fps one frame past 24 hours, an odd
/// number of ticks a double can't hold.
pub fn time_of_day() -> Vec<u8> {
    let mut a = ClipSpec::new("Dawn", "file:///media/dawn.mov", "2592000", "60", "29.97");
    a.avail = Some(("2591000".into(), "10000".into()));
    let mut b = ClipSpec::new(
        "Tick",
        "file:///media/tick.wav",
        "353894401",
        "8192",
        "4096",
    );
    b.avail = Some(("353894000".into(), "100000".into()));
    file(&timeline(
        "Day",
        None,
        vec![
            track("V1", "Video", vec![clip(&a)], vec![]),
            track("A1", "Audio", vec![clip(&b)], vec![]),
        ],
        vec![],
    ))
}

/// A timeline of `tracks` tracks of `per` clips each, a one-frame gap between clips, `media`
/// media in turn, `markers` markers on clips in turn: the measurements' shape.
pub fn big(tracks: usize, per: usize, media: usize, markers: usize) -> Vec<u8> {
    let mut ts = Vec::new();
    let mut m = 0usize;
    for t in 0..tracks {
        let mut kids = Vec::new();
        for i in 0..per {
            let n = t * per + i;
            let mut c = ClipSpec::new(
                &format!("c{n}"),
                &format!("file:///media/m{}.mov", n % media.max(1)),
                &format!("{}.0", (i % 50) * 10),
                "24.0",
                "24.0",
            );
            if m < markers && n % (tracks * per / markers.max(1)).max(1) == 0 {
                c.markers = vec![marker(&format!("k{m}"), &c.src, "0.0", "24.0", "RED", "")];
                m += 1;
            }
            kids.push(clip(&c));
            kids.push(gap("1.0", "24.0"));
        }
        ts.push(track(&format!("V{}", t + 1), "Video", kids, vec![]));
    }
    file(&timeline("Big", Some(("86400.0", "24.0")), ts, vec![]))
}

/// The live doc's roots, as `c` reads them.
pub fn content(s: &Session, c: ConnId) -> Vec<(String, Neutral)> {
    s.read(c).unwrap().content
}
