//! The agent's verbs on a live timeline, as the kit's structured verbs: each computes ops by
//! key on the live store under the session lock, and the kit applies them as one change with
//! its guards (an agent's edit needs `--base` or `--if-version`; a field written since the base
//! is refused, by name).
//!
//! | Verb | Ops | The agent's playhead |
//! |---|---|---|
//! | `timeline info` | read: name, rate, start timecode, length (the latest item end; and in ticks), where the end is from, how its tracks stack, counts | not moved |
//! | `track list [--after @trk:N] [--limit N]` | read: address, kind, label, name, items, `disabled` when the file disables it; at most 200 a page | not moved |
//! | `clip list @trk:N [--from T] [--to T] [--after @item:N] [--limit N]` | read: the track's items in time order; at most 200 a page | not moved |
//! | `clip at @trk:N T` | read: the item under a time | not moved |
//! | `clip show @item:N` | read: every field | not moved |
//! | `marker list [--from T] [--to T] [--after @mk:N] [--limit N]` | read: at most 200 a page | not moved |
//! | `media list [--after @med:N] [--limit N]` | read: address, kind, name, file, length, start in ticks (or `unknown`); at most 200 a page | not moved |
//! | `clip move @item:N --to T [--track @trk:N] [--snap]` | set `geom` (and `track`): one field set, no list move | the new start, in the track it is on now |
//! | `clip trim @item:N [--in T] [--out T] [--snap]` | set `geom`, `src` | `--in` (with or without `--out`): the new start; `--out` alone: the new end; in its track |
//! | `clip rename @item:N NAME` | set `name` | the item's start, in its track |
//! | `clip enable @item:N [--off]` | set `off` | the item's start, in its track |
//! | `clip delete @item:N` | delete in `items` (a lift: it leaves a gap), with its markers | where the item started, in its track |
//! | `clip add --media @med:N --track @trk:N --at T --in T --dur T [--name NAME] [--snap]` | insert in `items`, its fields | its start, in its track |
//! | `marker add --on @item:N\|@trk:N\|timeline --at T --name NAME [--dur T] [--note TEXT] [--color C] [--snap]` | insert in `markers` | the marker's time |
//! | `marker set @mk:N [--name NAME] [--note TEXT] [--color C] [--at T] [--dur T] [--snap]` | set its fields | the marker's time (its new one with `--at`) |
//! | `marker delete @mk:N` | delete in `markers` | where the marker was |
//! | `track rename @trk:N NAME` | set `name` | not moved (no time) |
//!
//! **Times** are read at the timeline's rate: a timecode (`01:00:10:00`, the timeline's own,
//! from its start timecode), frames (`240f`) or seconds (`10s`, `1001/24000s`) from the
//! timeline's zero, or either after `+` or `-` for a time relative to the item's own. A time
//! between frames is refused unless `--snap` names the snap (to the nearer frame).
//!
//! **The agent's playhead.** An edit verb that works leaves its agent's time position (the
//! kit's playhead presence) at its edit point, as the table says: a record time in ticks
//! (the view schema's `time_unit`, a decimal string), paused, in the track it worked on by
//! address (`@trk:N`). A marker's time is its record time (a marker on an item: where the item
//! places it), in its item's track or its own; a timeline marker's is in the doc's own time (no
//! track). The kit sets it once the call succeeds, for an agent only: a person's playhead is
//! theirs, and no verb moves it (nor a watcher's). A refusal, a missing element or a usage error
//! moves nothing. A verb that works but changes nothing (a rename to the same name, a move to
//! where the clip is) still moves the playhead to its edit point: the kit sets it on every call
//! that succeeds.
//!
//! **Refusals are typed and registered** ([`super::errors::ERRORS`], ids
//! `TIMELINE-OTIO-Ennn`): each prints `refused: <code>: <why>` and a `next:` line naming a
//! verb, and exits 2; nothing applies. A missing track, item, marker or media is
//! `tl_not_found`, exit 4. The host gets the entry's report, with the details `code`, `next` and
//! `verb` (and `address` for `tl_not_found`).
//!
//! **Results as data.** Each verb gives its payload as data (`StructOutput::data`), which the
//! kit puts in a call's result (`mpc.result/1`, asked for with `--json` or by the host) as
//! `data` (`data_schema` `typed`, `data_spec` `timeline.<verb>/1`, as `timeline.clip-list/1`); the
//! text form is unchanged. A time is `{shown, ticks}` (what the text shows, and
//! record ticks as a decimal string); a duration the same; an item `{address, kind, track,
//! start, duration, name, off, editable, retimed}`. A list's next page is the result's
//! `page.next` too. Each verb's shape is pinned in `tests/results_golden/`.

use std::collections::BTreeMap;

use k2_mpc_base::doc::Edit;
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::{PlayState, TimePosition};
use k2_mpc_base::tools::exit::{NOT_FOUND, OK, USAGE};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};

use super::index::TrackIndex;
use super::model::{
    f, key, prop, t, Store, ITEMS, ITEM_PROPS, MARKERS, MARKER_PROPS, MEDIA, MEDIA_PROPS, TIMELINE,
    TRACKS,
};
use super::view::{address_of, key_of, timecode};
use super::{next_mark, ticks};
use crate::mutant;
use crate::time::{parse_time, Rate, Snap};

/// The most rows one page holds: the default and the most.
pub const PAGE_MAX: usize = 200;

/// The longest text `clip show` prints of one field, in bytes.
pub const SHOW_MAX: u32 = 16_384;

const EXITS: &[u8] = &[OK, USAGE, NOT_FOUND];

/// The verbs, as the kit routes them.
pub const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "timeline info",
        "(name, rate, start timecode, length, end from, stacking, tracks, items, markers, media)",
        &[PersonAction::ReadAll],
        EXITS,
        parse_info,
    ),
    PackVerb::structured_read(
        "track list",
        "[--after @trk:N] [--limit N] (each track: address, kind, label, name, items; at most 200 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_track_list,
    ),
    PackVerb::structured_read(
        "clip list",
        "@trk:N [--from T] [--to T] [--after @item:N] [--limit N] (the track's items in time order; at most 200 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_clip_list,
    ),
    PackVerb::structured_read(
        "clip at",
        "@trk:N T (the item under a time)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_clip_at,
    ),
    PackVerb::structured_read(
        "clip show",
        "@item:N (every field)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_clip_show,
    ),
    PackVerb::structured_read(
        "marker list",
        "[--from T] [--to T] [--after @mk:N] [--limit N] (at most 200 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_marker_list,
    ),
    PackVerb::structured_read(
        "media list",
        "[--after @med:N] [--limit N] (each media: address, kind, name, file, length, start in ticks or unknown; at most 200 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_media_list,
    ),
    PackVerb::structured_edit(
        "clip move",
        "@item:N --to T [--track @trk:N] [--snap] (onto a free range of a track of its kind; not beside a transition)",
        &[PersonAction::RowMove],
        EXITS,
        parse_clip_move,
    ),
    PackVerb::structured_edit(
        "clip trim",
        "@item:N [--in T] [--out T] [--snap] (its first and last frame's record times; within its media)",
        &[PersonAction::RowMove],
        EXITS,
        parse_clip_trim,
    ),
    PackVerb::structured_edit(
        "clip rename",
        "@item:N NAME",
        &[PersonAction::TypeOver],
        EXITS,
        parse_clip_rename,
    ),
    PackVerb::structured_edit(
        "clip enable",
        "@item:N [--off]",
        &[PersonAction::Format],
        EXITS,
        parse_clip_enable,
    ),
    PackVerb::structured_edit(
        "clip delete",
        "@item:N (a lift: it leaves a gap; not beside a transition)",
        &[PersonAction::RowDelete],
        EXITS,
        parse_clip_delete,
    ),
    PackVerb::structured_edit(
        "clip add",
        "--media @med:N --track @trk:N --at T --in T --dur T [--name NAME] [--snap] (onto a free range)",
        &[PersonAction::RowInsert],
        EXITS,
        parse_clip_add,
    ),
    PackVerb::structured_edit(
        "marker add",
        "--on @item:N|@trk:N|timeline --at T --name NAME [--dur T] [--note TEXT] [--color C] [--snap]",
        &[PersonAction::Annotate],
        EXITS,
        parse_marker_add,
    ),
    PackVerb::structured_edit(
        "marker set",
        "@mk:N [--name NAME] [--note TEXT] [--color C] [--at T] [--dur T] [--snap]",
        &[PersonAction::Annotate],
        EXITS,
        parse_marker_set,
    ),
    PackVerb::structured_edit(
        "marker delete",
        "@mk:N",
        &[PersonAction::Annotate],
        EXITS,
        parse_marker_delete,
    ),
    PackVerb::structured_edit(
        "track rename",
        "@trk:N NAME",
        &[PersonAction::TypeOver],
        EXITS,
        parse_track_rename,
    ),
];

// ---------- arguments and outputs ----------

type R = Result<StructOutput, ToolError>;

fn usage(m: impl Into<String>) -> ToolError {
    ToolError::usage(m)
}

/// A typed refusal by its code, one of the pack's registered entries
/// ([`super::errors::ERRORS`]): nothing applies; its text is `refused: <code>: <why>` and
/// `next: <next>`, its exit the entry's, and the host gets the entry's report.
fn refuse(code: &str, why: &str, next: &str) -> R {
    match super::errors::by_code(code) {
        Some(entry) => Ok(StructOutput::refused(entry, why, next)),
        // every code the verbs use is registered (`tests/results.rs` reads them out of this
        // file); this keeps the text form if one isn't
        None => Ok(StructOutput::new(
            Vec::new(),
            format!("refused: {code}: {why}\nnext: {next}\n").into_bytes(),
            USAGE,
        )),
    }
}

/// A track, item, marker or media that isn't there: the registered `tl_not_found`, exit 4,
/// with the address as a detail.
fn not_found(addr: &str, next: &str) -> R {
    Ok(StructOutput::refused(
        &super::errors::NOT_FOUND_ENTRY,
        format!("{addr} names nothing live"),
        next,
    )
    .detail("address", addr))
}

/// The version of every verb's data shape (the result's `data_spec`, `timeline.<verb>/1`): a new
/// one when a field is renamed, removed or given another type; a new field keeps it.
pub const DATA_VERSION: u32 = 1;

/// A verb's run, its result's data named `spec` at [`DATA_VERSION`] (the kit's
/// `StructOutput::data_spec`; a refusal has no data, and names none).
fn typed(spec: &'static str, f: impl FnOnce(&VerbCtx<'_>) -> R + 'static) -> StructEdit {
    Box::new(move |ctx: &VerbCtx<'_>| f(ctx).map(|o| o.data_spec(spec, DATA_VERSION)))
}

/// A verb that worked, with its payload as data (the call's result's `data`, when asked for).
fn done(edits: Vec<Edit>, out: String, data: Json) -> R {
    Ok(StructOutput::new(edits, out.into_bytes(), OK).data(data))
}

/// [`done`], leaving the acting agent's playhead at `at` record ticks, paused, in `within` (a
/// track's address; `None`: the doc's own time). The kit sets it once the call succeeds, for an
/// agent only.
fn done_at(edits: Vec<Edit>, out: String, data: Json, at: i64, within: Option<&str>) -> R {
    let o = StructOutput::new(edits, out.into_bytes(), OK).data(data);
    let Ok(t) = TimePosition::new(&ticks::text(at), PlayState::Paused) else {
        return Ok(o);
    };
    let t = match within {
        // an address the kit's grammar doesn't read: the time alone
        Some(w) => t.clone().within(w).unwrap_or(t),
        None => t,
    };
    Ok(o.time_position(t))
}

/// The address of the track whose key is `track`, as a playhead is in it; `None` when it names
/// none.
fn track_address(track: &str) -> Option<String> {
    Key::new(track).ok().map(|k| address_of(TRACKS, &k))
}

struct Args {
    pos: Vec<String>,
    vals: BTreeMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    fn val(&self, k: &str) -> Option<&str> {
        self.vals.get(k).map(String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.switches.iter().any(|s| s == k)
    }
    fn one(&self, what: &str) -> Result<&str, ToolError> {
        match self.pos.as_slice() {
            [x] => Ok(x),
            [] => Err(usage(format!("give the {what}"))),
            _ => Err(usage(format!("one {what}, and nothing else"))),
        }
    }
    fn limit(&self) -> Result<usize, ToolError> {
        match self.val("--limit") {
            None => Ok(PAGE_MAX),
            Some(v) => match v.parse::<usize>() {
                Ok(n) if (1..=PAGE_MAX).contains(&n) => Ok(n),
                _ => Err(usage(format!("--limit is 1 to {PAGE_MAX}"))),
            },
        }
    }
}

fn parse_args(args: &[String], with_value: &[&str], switches: &[&str]) -> Result<Args, ToolError> {
    let mut a = Args {
        pos: Vec::new(),
        vals: BTreeMap::new(),
        switches: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        if with_value.contains(&x.as_str()) {
            let v = args
                .get(i + 1)
                .ok_or_else(|| usage(format!("{x} needs a value")))?;
            if a.vals.insert(x.clone(), v.clone()).is_some() {
                return Err(usage(format!("{x} given twice")));
            }
            i += 2;
        } else if switches.contains(&x.as_str()) {
            a.switches.push(x.clone());
            i += 1;
        } else if x.starts_with("--") {
            return Err(usage(format!("unknown flag {x}")));
        } else {
            a.pos.push(x.clone());
            i += 1;
        }
    }
    Ok(a)
}

fn addr_of(root: &str, addr: &str, what: &str) -> Result<Key, ToolError> {
    key_of(root, addr).ok_or_else(|| usage(format!("{addr:?} isn't {what}")))
}

fn one_line(s: &str) -> String {
    let (kept, cut) = k2_mpc_base::index::cut_text(s, 200);
    let mut o = kept.replace(['\n', '\t', '\r'], " ");
    if cut {
        o.push('…');
    }
    o
}

// ---------- results as data ----------

/// A stored text as data: as stored, cut at [`SHOW_MAX`] bytes (`…` after a cut).
fn text_data(v: &str) -> Json {
    let (kept, cut) = k2_mpc_base::index::cut_text(v, SHOW_MAX);
    let mut o = kept.to_owned();
    if cut {
        o.push('…');
    }
    Json::from(o)
}

/// A record time as data: `{shown, ticks}`, what the text shows ([`show_t`]) and its ticks (a
/// decimal string, the view schema's `time_unit`).
fn time_data(s: &Doc<'_>, at: i64) -> Json {
    Json::obj([
        ("shown", Json::from(show_t(s, at, true))),
        ("ticks", Json::from(ticks::text(at))),
    ])
}

/// A duration as data: `{shown, ticks}` ([`show_d`]).
fn dur_data(s: &Doc<'_>, d: i64) -> Json {
    Json::obj([
        ("shown", Json::from(show_d(s, d))),
        ("ticks", Json::from(ticks::text(d))),
    ])
}

/// An item as the reads give it: `{address, kind, track, start, duration, name, off,
/// editable, retimed}`.
fn item_data(s: &Doc<'_>, k: &Key) -> Json {
    let (a, d) = s.geom(k).unwrap_or((0, 0));
    Json::obj([
        ("address", Json::from(address_of(ITEMS, k))),
        ("kind", Json::from(s.kind(k).unwrap_or(""))),
        (
            "track",
            Json::from(s.item(k, f::TRACK).and_then(track_address)),
        ),
        ("start", time_data(s, a)),
        ("duration", dur_data(s, d)),
        ("name", text_data(s.item(k, f::NAME).unwrap_or(""))),
        ("off", Json::from(s.item(k, f::OFF) == Some("1"))),
        ("editable", Json::from(s.item(k, f::EDIT) == Some("1"))),
        ("retimed", Json::from(s.item(k, f::RETIMED) == Some("1"))),
    ])
}

/// The arguments of a list verb's next page: `pos`, the flags given (`--from`, `--to`; `--limit`
/// when not the default), and `--after` the last row shown.
fn page_args(pos: &[&str], flags: &[(&str, Option<&str>)], n: usize, last: String) -> Vec<String> {
    let mut v: Vec<String> = pos.iter().map(|x| (*x).to_owned()).collect();
    for (f, x) in flags {
        if let Some(x) = x {
            v.push((*f).to_owned());
            v.push((*x).to_owned());
        }
    }
    if n != PAGE_MAX {
        v.push("--limit".to_owned());
        v.push(n.to_string());
    }
    v.push("--after".to_owned());
    v.push(last);
    v
}

// ---------- times ----------

/// A time as the verbs show it: a timecode when the rate has one and it's on a frame (`abs`:
/// from the start timecode; drop-frame when the timeline is), else exact seconds.
fn show_t(s: &Doc<'_>, ticks_at: i64, abs: bool) -> String {
    let tc = if abs {
        s.tl(t::TC).and_then(ticks::parse).unwrap_or(0)
    } else {
        0
    };
    if let Some(r) = s.rate() {
        if let Some(c) = timecode(ticks_at, tc, r, s.tl(t::DROP_FRAME) == Some("1")) {
            return c;
        }
    }
    ticks::time(ticks_at).map_or_else(|_| format!("{ticks_at}t"), |x| format!("{x}s"))
}

/// A duration as the verbs show it: frames when whole, else seconds.
fn show_d(s: &Doc<'_>, d: i64) -> String {
    if let Some(fr) = s.rate().and_then(ticks::frame) {
        if d % fr == 0 {
            return format!("{}f", d / fr);
        }
    }
    ticks::time(d).map_or_else(|_| format!("{d}t"), |x| format!("{x}s"))
}

/// Why a time an agent typed can't be read.
enum TimeWhy {
    /// Between two frames, and no `--snap`.
    OffGrid(String),
    /// Anything else.
    Bad(String),
}

/// A time an agent typed, as record ticks: a timecode is the timeline's own (from its start
/// timecode); frames and seconds count from zero; a signed one is relative to `base`.
fn read_t(s: &Doc<'_>, text: &str, snap: bool, base: Option<i64>) -> Result<i64, TimeWhy> {
    let rate: Rate = s
        .rate()
        .ok_or_else(|| TimeWhy::Bad("the timeline has no frame rate to read times at".into()))?;
    let snap = if snap || mutant::active("OffGridSnapped") {
        Snap::Nearest
    } else {
        Snap::Refuse
    };
    let input = parse_time(text, rate, snap).map_err(|e| match e {
        crate::time::TimeError::OffGrid { .. } => TimeWhy::OffGrid(e.to_string()),
        e => TimeWhy::Bad(e.to_string()),
    })?;
    let n = ticks::of(input.time).map_err(|_| TimeWhy::Bad("a time out of range".into()))?;
    let body = text.trim().trim_start_matches(['+', '-']);
    let n = if body.contains([':', ';']) {
        n - s.tl(t::TC).and_then(ticks::parse).unwrap_or(0)
    } else {
        n
    };
    match (input.sign, base) {
        (None, _) => Ok(n),
        (Some(sign), Some(b)) => Ok(match sign {
            crate::time::Sign::Plus => b.saturating_add(n),
            crate::time::Sign::Minus => b.saturating_sub(n),
        }),
        (Some(_), None) => Err(TimeWhy::Bad(
            "a relative time has nothing to count from".into(),
        )),
    }
}

/// A duration an agent typed, in ticks (no sign, no timecode offset).
fn read_d(s: &Doc<'_>, text: &str, snap: bool) -> Result<i64, TimeWhy> {
    if text.trim().contains([':', ';']) {
        // a timecode as a length: its frames from zero
        let rate = s
            .rate()
            .ok_or_else(|| TimeWhy::Bad("the timeline has no frame rate".into()))?;
        let i = parse_time(text, rate, Snap::Refuse).map_err(|e| TimeWhy::Bad(e.to_string()))?;
        return ticks::of(i.time).map_err(|_| TimeWhy::Bad("a time out of range".into()));
    }
    read_t(s, text, snap, None)
}

/// A time that can't be read, and what to run next. Between two frames: `call` (the whole call
/// as given) with `--snap`, when the verb takes it; else `timeline info`. Anything else:
/// `timeline info`, which gives the rate and start timecode a time is written at (`01:00:05:00`,
/// `120f`, `5s`, or `+10f` relative to the item).
fn time_refusal(w: TimeWhy, call: Option<&str>) -> R {
    match (w, call) {
        (TimeWhy::OffGrid(why), Some(c)) => refuse("tl_off_grid", &why, &format!("{c} --snap")),
        (TimeWhy::OffGrid(why), None) => refuse("tl_off_grid", &why, READ_TIMES),
        (TimeWhy::Bad(why), _) => refuse("tl_time", &why, READ_TIMES),
    }
}

/// The read that gives what a time is written at: the rate, the start timecode, the length.
const READ_TIMES: &str = "timeline info";

/// `s` as one argument of a call: as is, or in single quotes when it holds a space or a quote.
fn arg(s: &str) -> String {
    if s.is_empty() || s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_owned()
    }
}

/// A call: `verb`, then each given flag with its value.
fn call_of(verb: &str, flags: &[(&str, Option<&str>)]) -> String {
    let mut c = verb.to_owned();
    for (f, v) in flags {
        if let Some(v) = v {
            c.push_str(&format!(" {f} {}", arg(v)));
        }
    }
    c
}

// ---------- the doc, as a verb reads it ----------

/// The next item key: one past both the mark and every key the items list ever held.
fn next_item(s: &Doc<'_>) -> u64 {
    let mark: u64 = s.tl(t::MARK_I).and_then(|v| v.parse().ok()).unwrap_or(0);
    if mutant::active("MintBelowMark") {
        // planted: the mark ignored, one past the live keys only
        return s.ix().max_live_number() + 1;
    }
    mark.max(s.ix().max_number()) + 1
}

fn has(list: &[(Key, Value)], k: &Key) -> bool {
    list.iter().any(|(x, _)| x == k)
}

/// The doc as a verb reads it: the store, and the track index the kit keeps beside it (built
/// here from the view when the kit keeps none, outside a session).
struct Doc<'a> {
    s: Store<'a>,
    ix: Ix<'a>,
    view: &'a k2_mpc_base::session::View,
}

enum Ix<'a> {
    Kept(&'a TrackIndex),
    Own(Box<TrackIndex>),
}

impl<'a> Doc<'a> {
    fn of(ctx: &VerbCtx<'a>) -> Doc<'a> {
        let view = ctx.view();
        Doc {
            s: Store::of_view(view),
            ix: match ctx.verb_state::<TrackIndex>() {
                Some(i) => Ix::Kept(i),
                None => Ix::Own(Box::new(TrackIndex::build(view))),
            },
            view,
        }
    }

    fn ix(&self) -> &TrackIndex {
        match &self.ix {
            Ix::Kept(i) => i,
            Ix::Own(i) => i,
        }
    }

    /// Whether item `k` is live.
    fn live(&self, k: &Key) -> bool {
        self.ix().get(k).is_some()
    }

    /// Item `k`'s kind.
    fn kind(&self, k: &Key) -> Option<&str> {
        self.ix().get(k).map(|e| e.kind.as_str())
    }

    /// The items of `track` in time order.
    fn track_items(&self, track: &Key) -> Vec<&Key> {
        if mutant::active("ListOrderTrusted") {
            // planted: the list's order taken as time order
            return self.s.track_items(track);
        }
        self.ix().ordered(track.as_str(), self.view)
    }
}

impl<'a> std::ops::Deref for Doc<'a> {
    type Target = Store<'a>;
    fn deref(&self) -> &Store<'a> {
        &self.s
    }
}

/// Whether a transition on `track` sits on the cut at `start` or `end`.
fn beside_transition(s: &Doc<'_>, track: &str, start: i64, end: i64) -> bool {
    s.ix().transition_at(track, start).is_some() || s.ix().transition_at(track, end).is_some()
}

/// The first item on `track` (but `skip`) whose range meets `start..end`; transitions aside.
fn overlap<'d>(
    s: &'d Doc<'_>,
    track: &str,
    skip: Option<&Key>,
    start: i64,
    end: i64,
) -> Option<&'d Key> {
    if mutant::active("OverlapAllowed") {
        // planted: nothing checked
        return None;
    }
    s.ix().overlap(track, skip, start, end)
}

fn set(root_key: String, v: impl Into<String>) -> MapOp {
    MapOp::Set {
        key: root_key,
        value: Value::str(v.into()),
    }
}

fn map_edit(root: &str, ops: Vec<MapOp>) -> Option<Edit> {
    (!ops.is_empty()).then(|| Edit::Map {
        root: root.to_owned(),
        ops,
    })
}

/// Every field of element `k` of a composite map, deleted.
fn drop_fields(m: &BTreeMap<String, Value>, k: &Key) -> Vec<MapOp> {
    let start = prop(k, "");
    m.range(start.clone()..)
        .take_while(|(x, _)| x.starts_with(&start))
        .map(|(x, _)| MapOp::Delete { key: x.clone() })
        .collect()
}

/// The rate item `k`'s times are written at, when it has one.
fn item_rate(s: &Doc<'_>, k: &Key) -> Option<Rate> {
    s.ix().get(k).and_then(|e| e.rate)
}

/// The rate a new clip of media `m` is written at: that of an item using it, else the media's
/// own.
fn media_rate(s: &Doc<'_>, m: &Key) -> Option<Rate> {
    s.ix()
        .media_users(m.as_str())
        .find_map(|k| item_rate(s, k))
        .or_else(|| s.media_field(m, f::RATE).and_then(ticks::parse_rate))
}

/// Whether `n` ticks is a value an OTIO time holds exactly at one of `rates` (any rate when none
/// is known): what the write-back can write.
fn rate_ok(n: i64, rates: &[Option<Rate>]) -> bool {
    if n == 0 || mutant::active("RateGridIgnored") {
        return true;
    }
    let known: Vec<Rate> = rates.iter().flatten().copied().collect();
    known.is_empty()
        || known
            .iter()
            .any(|r| ticks::value_text(n, *r, "0").is_some())
}

/// An item beside a range, with its end (before) or its start (after).
type Side<'a> = Option<(&'a Key, i64)>;

/// The items on `track` (but `skip`, transitions aside) around `start..end`: the last ending at or
/// before `start`, and the first starting at or after `end`.
fn neighbours<'d>(
    s: &'d Doc<'_>,
    track: &str,
    skip: Option<&Key>,
    start: i64,
    end: i64,
) -> (Side<'d>, Side<'d>) {
    s.ix().neighbours(track, skip, start, end)
}

/// Whether the gaps before and after `start..end` on `track` (an item at rate `own`) can be
/// written at a neighbour's rate, as the write-back writes them.
fn gaps_on_rate(
    s: &Doc<'_>,
    track: &str,
    skip: Option<&Key>,
    start: i64,
    end: i64,
    own: Option<Rate>,
) -> bool {
    let (p, n) = neighbours(s, track, skip, start, end);
    let before = start - p.map_or(0, |x| x.1);
    let p_rate = p.and_then(|x| item_rate(s, x.0));
    let after_ok = match n {
        Some((nk, ns)) => rate_ok(ns - end, &[item_rate(s, nk), own]),
        None => true,
    };
    rate_ok(before, &[own, p_rate]) && after_ok
}

/// An item that exists and every verb may write, or the refusal.
fn writable(s: &Doc<'_>, k: &Key, addr: &str) -> Option<R> {
    if !s.live(k) {
        return Some(not_found(addr, "track list"));
    }
    if s.item(k, f::EDIT) != Some("1") {
        return Some(refuse(
            "tl_not_editable",
            &format!(
                "nothing writes this {} live (a transition, a nest or a retimed clip)",
                s.kind(k).unwrap_or("item")
            ),
            &format!("clip show {addr}"),
        ));
    }
    None
}

/// A track every item verb may place on, or the refusal.
fn track_ok(s: &Doc<'_>, track: &str) -> Option<R> {
    let tk = Key::new(track).ok();
    if s.track(tk.as_ref().unwrap_or(&key('t', 0)), f::EDIT) != Some("1") {
        return Some(refuse(
            "tl_track_not_editable",
            "a gap on this track holds more than a duration (a name, markers, effects or metadata): live v1 doesn't rebuild its gaps",
            "track list",
        ));
    }
    None
}

// ---------- reads ----------

fn parse_info(args: &[String]) -> Result<StructEdit, ToolError> {
    parse_args(args, &[], &[])?;
    Ok(typed("timeline.timeline-info", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        // the latest end of any item, from the index: no item is read
        let end = s.ix().max_end().unwrap_or(0);
        let out = format!(
            "name\t{}\nrate\t{}\nstart\t{}\nlength\t{}\nlength ticks\t{end}\nend from\t{}\nstacking\t{}\ntracks\t{}\nitems\t{}\nmarkers\t{}\nmedia\t{}\n",
            one_line(s.tl(t::NAME).unwrap_or("")),
            s.rate().map_or_else(|| "none".to_owned(), |r| r.to_string()),
            show_t(&s, 0, true),
            show_d(&s, end),
            one_line(s.tl(t::END_FROM).unwrap_or("")),
            one_line(s.tl(t::STACKING).unwrap_or("")),
            s.tracks.len(),
            s.items.len(),
            s.markers.len(),
            s.media.len(),
        );
        let data = Json::obj([
            ("name", text_data(s.tl(t::NAME).unwrap_or(""))),
            ("rate", Json::from(s.rate().map(|r| r.to_string()))),
            ("start", time_data(&s, 0)),
            ("length", dur_data(&s, end)),
            ("end_from", Json::from(s.tl(t::END_FROM).unwrap_or(""))),
            ("stacking", Json::from(s.tl(t::STACKING).unwrap_or(""))),
            ("drop_frame", Json::from(s.tl(t::DROP_FRAME) == Some("1"))),
            ("tracks", Json::from(s.tracks.len())),
            ("items", Json::from(s.items.len())),
            ("markers", Json::from(s.markers.len())),
            ("media", Json::from(s.media.len())),
        ]);
        done(Vec::new(), out, data)
    }))
}

fn parse_track_list(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after", "--limit"], &[])?;
    let n = a.limit()?;
    let after = a
        .val("--after")
        .map(|x| addr_of(TRACKS, x, "a track (@trk:N)"))
        .transpose()?;
    Ok(typed("timeline.track-list", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        let start = match &after {
            Some(k) => match s.tracks.iter().position(|(x, _)| x == k) {
                Some(i) => i + 1,
                None => return not_found(&address_of(TRACKS, k), "track list"),
            },
            None => 0,
        };
        let mut out = String::new();
        let mut rows = Vec::new();
        let page: Vec<&(Key, Value)> = s.tracks.iter().skip(start).take(n).collect();
        for (k, v) in &page {
            let items = s
                .items
                .iter()
                .filter(|(i, _)| s.item(i, f::TRACK) == Some(k.as_str()))
                .count();
            let off = if s.track(k, f::ENABLED) == Some("0") {
                "\tdisabled"
            } else {
                ""
            };
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{} items{off}\n",
                address_of(TRACKS, k),
                v.as_str().unwrap_or(""),
                s.track(k, f::NID).unwrap_or(""),
                one_line(s.track(k, f::NAME).unwrap_or("")),
                items
            ));
            rows.push(Json::obj([
                ("address", Json::from(address_of(TRACKS, k))),
                ("kind", Json::from(v.as_str().unwrap_or(""))),
                ("label", Json::from(s.track(k, f::NID).unwrap_or(""))),
                ("name", text_data(s.track(k, f::NAME).unwrap_or(""))),
                ("items", Json::from(items)),
                ("enabled", Json::from(off.is_empty())),
            ]));
        }
        let mut more = None;
        if start + page.len() < s.tracks.len() {
            if let Some((last, _)) = page.last() {
                out.push_str(&format!(
                    "next: track list --after {}\n",
                    address_of(TRACKS, last)
                ));
                more = Some(page_args(&[], &[], n, address_of(TRACKS, last)));
            }
        }
        let o = done(Vec::new(), out, Json::obj([("tracks", Json::List(rows))]))?;
        Ok(match more {
            Some(a) => o.next_page("track list", a),
            None => o,
        })
    }))
}

fn item_line(s: &Doc<'_>, k: &Key) -> String {
    let (a, d) = s.geom(k).unwrap_or((0, 0));
    let mut flags = Vec::new();
    if s.item(k, f::OFF) == Some("1") {
        flags.push("off");
    }
    if s.item(k, f::EDIT) != Some("1") {
        flags.push("read-only");
    }
    if s.item(k, f::RETIMED) == Some("1") {
        flags.push("retimed");
    }
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        address_of(ITEMS, k),
        s.kind(k).unwrap_or(""),
        show_t(s, a, true),
        show_d(s, d),
        one_line(s.item(k, f::NAME).unwrap_or("")),
        flags.join(",")
    )
}

fn parse_clip_list(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--from", "--to", "--after", "--limit"], &[])?;
    let addr = a.one("track (@trk:N)")?.to_owned();
    let tk = addr_of(TRACKS, &addr, "a track (@trk:N)")?;
    let n = if mutant::active("ReadUnbounded") {
        // planted: every item of the track in one read
        usize::MAX
    } else {
        a.limit()?
    };
    let after = a
        .val("--after")
        .map(|x| addr_of(ITEMS, x, "an item (@item:N)"))
        .transpose()?;
    let (from, to) = (
        a.val("--from").map(str::to_owned),
        a.val("--to").map(str::to_owned),
    );
    Ok(typed("timeline.clip-list", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !has(s.tracks, &tk) {
            return not_found(&addr, "track list");
        }
        let range = |x: &Option<String>| -> Result<Option<i64>, TimeWhy> {
            x.as_deref().map(|v| read_t(&s, v, false, None)).transpose()
        };
        let next = format!("clip list {addr}");
        let lo = match range(&from) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, None),
        };
        let hi = match range(&to) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, None),
        };
        let all: Vec<&Key> = s
            .track_items(&tk)
            .into_iter()
            .filter(|k| {
                let (st, d) = s.geom(k).unwrap_or((0, 0));
                lo.is_none_or(|l| st.saturating_add(d) > l) && hi.is_none_or(|h| st < h)
            })
            .collect();
        let start = match &after {
            Some(k) => match all.iter().position(|x| *x == k) {
                Some(i) => i + 1,
                None => return not_found(&address_of(ITEMS, k), &next),
            },
            None => 0,
        };
        let mut out = String::new();
        let page: Vec<&&Key> = all.iter().skip(start).take(n).collect();
        for k in &page {
            out.push_str(&item_line(&s, k));
        }
        let rows: Vec<Json> = page.iter().map(|k| item_data(&s, k)).collect();
        let mut more = None;
        if start + page.len() < all.len() {
            if let Some(last) = page.last() {
                out.push_str(&format!(
                    "next: clip list {addr} --after {}\n",
                    address_of(ITEMS, last)
                ));
                more = Some(page_args(
                    &[addr.as_str()],
                    &[("--from", from.as_deref()), ("--to", to.as_deref())],
                    n,
                    address_of(ITEMS, last),
                ));
            }
        }
        let data = Json::obj([
            ("track", Json::from(addr.as_str())),
            ("items", Json::List(rows)),
        ]);
        let o = done(Vec::new(), out, data)?;
        Ok(match more {
            Some(a) => o.next_page("clip list", a),
            None => o,
        })
    }))
}

fn parse_clip_at(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let (addr, at) = match a.pos.as_slice() {
        [x, y] => (x.clone(), y.clone()),
        _ => return Err(usage("give a track (@trk:N) and a time")),
    };
    let tk = addr_of(TRACKS, &addr, "a track (@trk:N)")?;
    Ok(typed("timeline.clip-at", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !has(s.tracks, &tk) {
            return not_found(&addr, "track list");
        }
        let when = match read_t(&s, &at, false, None) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, None),
        };
        let hit = s.ix().at(tk.as_str(), when);
        match hit {
            Some(k) => done(
                Vec::new(),
                item_line(&s, k),
                Json::obj([
                    ("track", Json::from(addr.as_str())),
                    ("at", Json::from(at.as_str())),
                    ("item", item_data(&s, k)),
                ]),
            ),
            None => not_found(&format!("{addr} at {at}"), &format!("clip list {addr}")),
        }
    }))
}

fn parse_clip_show(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let addr = a.one("item (@item:N)")?.to_owned();
    let k = addr_of(ITEMS, &addr, "an item (@item:N)")?;
    Ok(typed("timeline.clip-show", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !s.live(&k) {
            return not_found(&addr, "track list");
        }
        let mut out = format!("{addr}\t{}\n", s.kind(&k).unwrap_or(""));
        let mut fields = Vec::new();
        let mut marks = Vec::new();
        let start = prop(&k, "");
        for (key, v) in s.ip.range(start.clone()..) {
            let Some(fld) = key.strip_prefix(&start) else {
                break;
            };
            let v = v.as_str().unwrap_or("");
            let shown = match fld {
                f::TRACK => Key::new(v).map_or_else(|_| v.to_owned(), |x| address_of(TRACKS, &x)),
                f::MEDIA => Key::new(v).map_or_else(|_| v.to_owned(), |x| address_of(MEDIA, &x)),
                f::GEOM => match ticks::parse_pair(v) {
                    Some((st, d)) => {
                        format!("{v} ({} for {})", show_t(&s, st, true), show_d(&s, d))
                    }
                    None => v.to_owned(),
                },
                _ => {
                    let (kept, cut) = k2_mpc_base::index::cut_text(v, SHOW_MAX);
                    let mut o = kept.replace('\n', "\\n");
                    if cut {
                        o.push('…');
                    }
                    o
                }
            };
            out.push_str(&format!("{fld}\t{shown}\n"));
            let as_data = match fld {
                f::TRACK | f::MEDIA => Json::from(shown),
                _ => text_data(v),
            };
            fields.push((fld.to_owned(), as_data));
        }
        for (mk, _) in s.markers {
            if s.marker(mk, f::OWNER) == Some(k.as_str()) {
                out.push_str(&format!(
                    "marker\t{}\t{}\n",
                    address_of(MARKERS, mk),
                    one_line(s.marker(mk, f::NAME).unwrap_or(""))
                ));
                marks.push(Json::obj([
                    ("address", Json::from(address_of(MARKERS, mk))),
                    ("name", text_data(s.marker(mk, f::NAME).unwrap_or(""))),
                ]));
            }
        }
        let data = Json::obj([
            ("address", Json::from(addr.as_str())),
            ("kind", Json::from(s.kind(&k).unwrap_or(""))),
            ("item", item_data(&s, &k)),
            ("fields", Json::Obj(fields)),
            ("markers", Json::List(marks)),
        ]);
        done(Vec::new(), out, data)
    }))
}

fn parse_marker_list(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--from", "--to", "--after", "--limit"], &[])?;
    let n = a.limit()?;
    let after = a
        .val("--after")
        .map(|x| addr_of(MARKERS, x, "a marker (@mk:N)"))
        .transpose()?;
    let (from, to) = (
        a.val("--from").map(str::to_owned),
        a.val("--to").map(str::to_owned),
    );
    Ok(typed("timeline.marker-list", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        let lo = match from
            .as_deref()
            .map(|v| read_t(&s, v, false, None))
            .transpose()
        {
            Ok(v) => v,
            Err(w) => return time_refusal(w, None),
        };
        let hi = match to
            .as_deref()
            .map(|v| read_t(&s, v, false, None))
            .transpose()
        {
            Ok(v) => v,
            Err(w) => return time_refusal(w, None),
        };
        // each marker's record time: an item's media time placed by the item
        let mut all: Vec<(i64, usize, &Key)> = s
            .markers
            .iter()
            .enumerate()
            .filter_map(|(i, (k, _))| Some((marker_rec(&s, k)?, i, k)))
            .filter(|(r, _, _)| lo.is_none_or(|l| *r >= l) && hi.is_none_or(|h| *r < h))
            .collect();
        all.sort_by_key(|(r, i, _)| (*r, *i));
        let start = match &after {
            Some(k) => match all.iter().position(|(_, _, x)| *x == k) {
                Some(i) => i + 1,
                None => return not_found(&address_of(MARKERS, k), "marker list"),
            },
            None => 0,
        };
        let mut out = String::new();
        let mut rows = Vec::new();
        let page: Vec<&(i64, usize, &Key)> = all.iter().skip(start).take(n).collect();
        for (r, _, k) in &page {
            let owner = s.marker(k, f::OWNER).unwrap_or("");
            let on = Key::new(owner).map_or_else(
                |_| "timeline".to_owned(),
                |o| address_of(if s.live(&o) { ITEMS } else { TRACKS }, &o),
            );
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                address_of(MARKERS, k),
                show_t(&s, *r, true),
                on,
                one_line(s.marker(k, f::NAME).unwrap_or("")),
                one_line(s.marker(k, f::NOTE).unwrap_or(""))
            ));
            rows.push(Json::obj([
                ("address", Json::from(address_of(MARKERS, k))),
                ("at", time_data(&s, *r)),
                ("owner", Json::from(on)),
                ("name", text_data(s.marker(k, f::NAME).unwrap_or(""))),
                ("note", text_data(s.marker(k, f::NOTE).unwrap_or(""))),
            ]));
        }
        let mut more = None;
        if start + page.len() < all.len() {
            if let Some((_, _, last)) = page.last() {
                out.push_str(&format!(
                    "next: marker list --after {}\n",
                    address_of(MARKERS, last)
                ));
                more = Some(page_args(
                    &[],
                    &[("--from", from.as_deref()), ("--to", to.as_deref())],
                    n,
                    address_of(MARKERS, last),
                ));
            }
        }
        let o = done(Vec::new(), out, Json::obj([("markers", Json::List(rows))]))?;
        Ok(match more {
            Some(a) => o.next_page("marker list", a),
            None => o,
        })
    }))
}

fn parse_media_list(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after", "--limit"], &[])?;
    let n = a.limit()?;
    let after = a
        .val("--after")
        .map(|x| addr_of(MEDIA, x, "a media (@med:N)"))
        .transpose()?;
    Ok(typed("timeline.media-list", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        let start = match &after {
            Some(k) => match s.media.iter().position(|(x, _)| x == k) {
                Some(i) => i + 1,
                None => return not_found(&address_of(MEDIA, k), "media list"),
            },
            None => 0,
        };
        let mut out = String::new();
        let mut rows = Vec::new();
        let page: Vec<&(Key, Value)> = s.media.iter().skip(start).take(n).collect();
        for (k, v) in &page {
            let len_ticks = s.media_field(k, f::DUR).and_then(ticks::parse);
            let len = len_ticks.map_or_else(String::new, |d| show_d(&s, d));
            // its start in ticks, so a clip's place in it is its `src` less this
            let first = s.media_field(k, f::START).unwrap_or(f::ORIGIN_UNKNOWN);
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                address_of(MEDIA, k),
                v.as_str().unwrap_or(""),
                one_line(s.media_field(k, f::NAME).unwrap_or("")),
                one_line(s.media_field(k, f::FILE).unwrap_or("")),
                len,
                one_line(first)
            ));
            rows.push(Json::obj([
                ("address", Json::from(address_of(MEDIA, k))),
                ("kind", Json::from(v.as_str().unwrap_or(""))),
                ("name", text_data(s.media_field(k, f::NAME).unwrap_or(""))),
                ("file", text_data(s.media_field(k, f::FILE).unwrap_or(""))),
                ("length", len_ticks.map_or(Json::Null, |d| dur_data(&s, d))),
                (
                    "start_ticks",
                    Json::from(s.media_field(k, f::START).map(str::to_owned)),
                ),
            ]));
        }
        let mut more = None;
        if start + page.len() < s.media.len() {
            if let Some((last, _)) = page.last() {
                out.push_str(&format!(
                    "next: media list --after {}\n",
                    address_of(MEDIA, last)
                ));
                more = Some(page_args(&[], &[], n, address_of(MEDIA, last)));
            }
        }
        let o = done(Vec::new(), out, Json::obj([("media", Json::List(rows))]))?;
        Ok(match more {
            Some(a) => o.next_page("media list", a),
            None => o,
        })
    }))
}

// ---------- edits ----------

fn parse_clip_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--to", "--track"], &["--snap"])?;
    let addr = a.one("item (@item:N)")?.to_owned();
    let k = addr_of(ITEMS, &addr, "an item (@item:N)")?;
    let to = a
        .val("--to")
        .ok_or_else(|| usage("give --to T"))?
        .to_owned();
    let target = a
        .val("--track")
        .map(|x| Ok::<_, ToolError>((x.to_owned(), addr_of(TRACKS, x, "a track (@trk:N)")?)))
        .transpose()?;
    let snap = a.has("--snap");
    Ok(typed("timeline.clip-move", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if let Some(r) = writable(&s, &k, &addr) {
            return r;
        }
        let (start, dur) = s.geom(&k).unwrap_or((0, 0));
        let from_track = s.item(&k, f::TRACK).unwrap_or("").to_owned();
        let next = call_of(
            &format!("clip move {addr}"),
            &[
                ("--to", Some(to.as_str())),
                ("--track", target.as_ref().map(|(t, _)| t.as_str())),
            ],
        );
        let new_start = match read_t(&s, &to, snap, Some(start)) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, Some(&next)),
        };
        let (track, track_addr) = match &target {
            Some((ta, tk)) => {
                if !has(s.tracks, tk) {
                    return not_found(ta, "track list");
                }
                let kind = |x: &str| {
                    s.tracks
                        .iter()
                        .find(|(y, _)| y.as_str() == x)
                        .and_then(|(_, v)| v.as_str())
                };
                if kind(tk.as_str()) != kind(&from_track) {
                    return refuse(
                        "tl_track_kind",
                        "the track carries another kind (video, audio)",
                        "track list",
                    );
                }
                (tk.as_str().to_owned(), ta.clone())
            }
            None => (
                from_track.clone(),
                Key::new(&from_track)
                    .map_or_else(|_| from_track.clone(), |x| address_of(TRACKS, &x)),
            ),
        };
        for tr in [&from_track, &track] {
            if let Some(r) = track_ok(&s, tr) {
                return r;
            }
        }
        if beside_transition(&s, &from_track, start, start.saturating_add(dur)) {
            return refuse(
                "tl_transition",
                "a transition joins it to its neighbour: moving it would break the transition",
                &format!("clip show {addr}"),
            );
        }
        let new_end =
            match new_start.checked_add(dur) {
                Some(e) if new_start >= 0 && e <= ticks::RECORD_MAX => e,
                _ => return refuse(
                    "tl_range",
                    "the clip would start before zero or end past the longest timeline held live",
                    READ_TIMES,
                ),
            };
        if let Some(o) = overlap(&s, &track, Some(&k), new_start, new_end) {
            return refuse(
                "tl_overlap",
                &format!("{} is there", address_of(ITEMS, o)),
                &format!("clip list {track_addr} --from {to}"),
            );
        }
        let own = item_rate(&s, &k);
        let lands = gaps_on_rate(&s, &track, Some(&k), new_start, new_end, own);
        let leaves = {
            let (p, n) = neighbours(&s, &from_track, Some(&k), start, start.saturating_add(dur));
            match n {
                Some((nk, ns)) => {
                    let pe = p.map_or(0, |x| x.1);
                    rate_ok(
                        ns - pe,
                        &[item_rate(&s, nk), p.and_then(|x| item_rate(&s, x.0))],
                    )
                }
                None => true,
            }
        };
        if !(lands && leaves) {
            return refuse(
                "tl_rate",
                "the gaps it leaves or makes aren't whole frames of the clips around them (another rate)",
                &format!("clip show {addr}"),
            );
        }
        let mut ops = Vec::new();
        if (new_start, new_end) != (start, start.saturating_add(dur)) {
            ops.push(set(prop(&k, f::GEOM), ticks::pair_text(new_start, dur)));
        }
        if track != from_track {
            ops.push(set(prop(&k, f::TRACK), track.clone()));
        }
        let out = format!("{addr}\t{}\t{}\n", show_t(&s, new_start, true), track_addr);
        let data = Json::obj([
            ("item", Json::from(addr.as_str())),
            ("start", time_data(&s, new_start)),
            ("track", Json::from(track_addr.as_str())),
        ]);
        done_at(
            map_edit(ITEM_PROPS, ops).into_iter().collect(),
            out,
            data,
            new_start,
            Some(track_addr.as_str()),
        )
    }))
}

fn parse_clip_trim(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--in", "--out"], &["--snap"])?;
    let addr = a.one("item (@item:N)")?.to_owned();
    let k = addr_of(ITEMS, &addr, "an item (@item:N)")?;
    let (inp, outp) = (
        a.val("--in").map(str::to_owned),
        a.val("--out").map(str::to_owned),
    );
    if inp.is_none() && outp.is_none() {
        return Err(usage("give --in T or --out T"));
    }
    let snap = a.has("--snap");
    Ok(typed("timeline.clip-trim", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if let Some(r) = writable(&s, &k, &addr) {
            return r;
        }
        let track = s.item(&k, f::TRACK).unwrap_or("").to_owned();
        if let Some(r) = track_ok(&s, &track) {
            return r;
        }
        let (start, dur) = s.geom(&k).unwrap_or((0, 0));
        let end = start.saturating_add(dur);
        let Some(src) = s.item(&k, f::SRC).and_then(ticks::parse) else {
            return refuse(
                "tl_not_editable",
                "it has no source time",
                &format!("clip show {addr}"),
            );
        };
        let next = call_of(
            &format!("clip trim {addr}"),
            &[("--in", inp.as_deref()), ("--out", outp.as_deref())],
        );
        let new_start = match &inp {
            Some(x) => match read_t(&s, x, snap, Some(start)) {
                Ok(v) => v,
                Err(w) => return time_refusal(w, Some(&next)),
            },
            None => start,
        };
        let new_end = match &outp {
            Some(x) => match read_t(&s, x, snap, Some(end)) {
                Ok(v) => v,
                Err(w) => return time_refusal(w, Some(&next)),
            },
            None => end,
        };
        if new_start < 0 || new_end <= new_start || new_end > ticks::RECORD_MAX {
            return refuse(
                "tl_range",
                "the clip would be empty or out of range",
                &format!("clip show {addr}"),
            );
        }
        if (new_start != start && beside_transition(&s, &track, start, start))
            || (new_end != end && beside_transition(&s, &track, end, end))
        {
            return refuse(
                "tl_transition",
                "a transition sits on that edge",
                &format!("clip show {addr}"),
            );
        }
        let new_src = src.saturating_add(new_start - start);
        let new_dur = new_end - new_start;
        let media = s.item(&k, f::MEDIA).and_then(|m| Key::new(m).ok());
        if let Some(m) = &media {
            let ms = s.media_field(m, f::START).and_then(ticks::parse);
            let md = s.media_field(m, f::DUR).and_then(ticks::parse);
            if let (Some(ms), Some(md)) = (ms, md) {
                if new_src < ms || new_src.saturating_add(new_dur) > ms.saturating_add(md) {
                    return refuse(
                        "tl_media_range",
                        "the media doesn't hold those frames",
                        &format!("clip show {addr}"),
                    );
                }
            }
        }
        if let Some(o) = overlap(&s, &track, Some(&k), new_start, new_end) {
            return refuse(
                "tl_overlap",
                &format!("{} is there", address_of(ITEMS, o)),
                &format!("clip show {addr}"),
            );
        }
        let own = item_rate(&s, &k);
        if !rate_ok(new_src, &[own])
            || !rate_ok(new_dur, &[own])
            || !gaps_on_rate(&s, &track, Some(&k), new_start, new_end, own)
        {
            return refuse(
                "tl_rate",
                "that isn't a whole frame of the clip's own rate",
                &format!("clip show {addr}"),
            );
        }
        let mut ops = Vec::new();
        if (new_start, new_dur) != (start, dur) {
            ops.push(set(prop(&k, f::GEOM), ticks::pair_text(new_start, new_dur)));
        }
        if new_src != src {
            ops.push(set(prop(&k, f::SRC), ticks::text(new_src)));
        }
        let out = format!(
            "{addr}\t{}\t{}\n",
            show_t(&s, new_start, true),
            show_d(&s, new_dur)
        );
        // the edge it trimmed: the in point when it moved one, else the out point
        let at = if inp.is_some() || mutant::active("TrimOutAtStart") {
            new_start
        } else {
            new_end
        };
        let data = Json::obj([
            ("item", Json::from(addr.as_str())),
            ("start", time_data(&s, new_start)),
            ("duration", dur_data(&s, new_dur)),
            ("track", Json::from(track_address(&track))),
        ]);
        done_at(
            map_edit(ITEM_PROPS, ops).into_iter().collect(),
            out,
            data,
            at,
            track_address(&track).as_deref(),
        )
    }))
}

fn parse_clip_rename(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let (addr, name) = match a.pos.as_slice() {
        [x, y] => (x.clone(), y.clone()),
        _ => return Err(usage("give an item (@item:N) and its new name")),
    };
    let k = addr_of(ITEMS, &addr, "an item (@item:N)")?;
    Ok(typed("timeline.clip-rename", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if let Some(r) = writable(&s, &k, &addr) {
            return r;
        }
        let mut ops = Vec::new();
        if s.item(&k, f::NAME) != Some(name.as_str()) {
            ops.push(set(prop(&k, f::NAME), name.clone()));
        }
        let (start, _) = s.geom(&k).unwrap_or((0, 0));
        done_at(
            map_edit(ITEM_PROPS, ops).into_iter().collect(),
            format!("{addr}\t{}\n", one_line(&name)),
            Json::obj([
                ("item", Json::from(addr.as_str())),
                ("name", text_data(&name)),
            ]),
            start,
            s.item(&k, f::TRACK).and_then(track_address).as_deref(),
        )
    }))
}

fn parse_clip_enable(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &["--off"])?;
    let addr = a.one("item (@item:N)")?.to_owned();
    let k = addr_of(ITEMS, &addr, "an item (@item:N)")?;
    let off = a.has("--off");
    Ok(typed("timeline.clip-enable", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if let Some(r) = writable(&s, &k, &addr) {
            return r;
        }
        let want = if off { "1" } else { "0" };
        let ops = if s.item(&k, f::OFF) == Some(want) {
            Vec::new()
        } else {
            vec![set(prop(&k, f::OFF), want)]
        };
        let (start, _) = s.geom(&k).unwrap_or((0, 0));
        done_at(
            map_edit(ITEM_PROPS, ops).into_iter().collect(),
            format!("{addr}\t{}\n", if off { "off" } else { "on" }),
            Json::obj([
                ("item", Json::from(addr.as_str())),
                ("enabled", Json::from(!off)),
            ]),
            start,
            s.item(&k, f::TRACK).and_then(track_address).as_deref(),
        )
    }))
}

fn parse_clip_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let addr = a.one("item (@item:N)")?.to_owned();
    let k = addr_of(ITEMS, &addr, "an item (@item:N)")?;
    Ok(typed("timeline.clip-delete", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if let Some(r) = writable(&s, &k, &addr) {
            return r;
        }
        let track = s.item(&k, f::TRACK).unwrap_or("").to_owned();
        if let Some(r) = track_ok(&s, &track) {
            return r;
        }
        let (start, dur) = s.geom(&k).unwrap_or((0, 0));
        if beside_transition(&s, &track, start, start.saturating_add(dur)) {
            return refuse(
                "tl_transition",
                "a transition joins it to its neighbour",
                &format!("clip show {addr}"),
            );
        }
        let mut edits = vec![Edit::List {
            root: ITEMS.to_owned(),
            ops: vec![ListOp::Delete { key: k.clone() }],
        }];
        edits.extend(map_edit(ITEM_PROPS, drop_fields(s.ip, &k)));
        // its markers go with it
        let mine: Vec<Key> = s
            .markers
            .iter()
            .filter(|(m, _)| s.marker(m, f::OWNER) == Some(k.as_str()))
            .map(|(m, _)| m.clone())
            .collect();
        if !mine.is_empty() {
            edits.push(Edit::List {
                root: MARKERS.to_owned(),
                ops: mine
                    .iter()
                    .map(|m| ListOp::Delete { key: m.clone() })
                    .collect(),
            });
            edits.extend(map_edit(
                MARKER_PROPS,
                mine.iter().flat_map(|m| drop_fields(s.kp, m)).collect(),
            ));
        }
        // and its media, when nothing else uses it (a file holds only the media its clips use)
        let mut media_gone = None;
        if let Some(m) = s.item(&k, f::MEDIA) {
            let used = s.ix().media_users(m).any(|o| o != &k);
            if let (false, Ok(mk)) = (used, Key::new(m)) {
                media_gone = Some(address_of(MEDIA, &mk));
                edits.push(Edit::List {
                    root: MEDIA.to_owned(),
                    ops: vec![ListOp::Delete { key: mk.clone() }],
                });
                edits.extend(map_edit(MEDIA_PROPS, drop_fields(s.mp, &mk)));
            }
        }
        let data = Json::obj([
            ("item", Json::from(addr.as_str())),
            ("deleted", Json::from(true)),
            (
                "markers",
                Json::list(mine.iter().map(|m| address_of(MARKERS, m))),
            ),
            ("media", Json::from(media_gone)),
        ]);
        done_at(
            edits,
            format!("{addr}\tdeleted\n"),
            data,
            start,
            track_address(&track).as_deref(),
        )
    }))
}

fn parse_clip_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(
        args,
        &["--media", "--track", "--at", "--in", "--dur", "--name"],
        &["--snap"],
    )?;
    if !a.pos.is_empty() {
        return Err(usage("clip add takes flags only"));
    }
    let need = |x: &str| {
        a.val(x)
            .map(str::to_owned)
            .ok_or_else(|| usage(format!("give {x}")))
    };
    let (ma, ta) = (need("--media")?, need("--track")?);
    let (at, inp, dur) = (need("--at")?, need("--in")?, need("--dur")?);
    let mk = addr_of(MEDIA, &ma, "a media (@med:N)")?;
    let tk = addr_of(TRACKS, &ta, "a track (@trk:N)")?;
    let name = a.val("--name").map(str::to_owned);
    let snap = a.has("--snap");
    Ok(typed("timeline.clip-add", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !has(s.media, &mk) {
            return not_found(&ma, "media list");
        }
        if !has(s.tracks, &tk) {
            return not_found(&ta, "track list");
        }
        if let Some(r) = track_ok(&s, tk.as_str()) {
            return r;
        }
        let next = format!("clip add --media {ma} --track {ta} --at {at} --in {inp} --dur {dur}");
        let start = match read_t(&s, &at, snap, None) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, Some(&next)),
        };
        let src = match read_d(&s, &inp, snap) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, Some(&next)),
        };
        let d = match read_d(&s, &dur, snap) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, Some(&next)),
        };
        let end = match start.checked_add(d) {
            Some(e) if start >= 0 && d > 0 && e <= ticks::RECORD_MAX => e,
            _ => {
                return refuse(
                    "tl_range",
                    "the clip would be empty or out of range",
                    READ_TIMES,
                )
            }
        };
        let ms = s.media_field(&mk, f::START).and_then(ticks::parse);
        let md = s.media_field(&mk, f::DUR).and_then(ticks::parse);
        if let (Some(ms), Some(md)) = (ms, md) {
            if src < ms || src.saturating_add(d) > ms.saturating_add(md) {
                return refuse(
                    "tl_media_range",
                    "the media doesn't hold those frames",
                    "media list",
                );
            }
        }
        if let Some(o) = overlap(&s, tk.as_str(), None, start, end) {
            return refuse(
                "tl_overlap",
                &format!("{} is there", address_of(ITEMS, o)),
                &format!("clip list {ta} --from {at}"),
            );
        }
        let own = media_rate(&s, &mk);
        if !rate_ok(src, &[own])
            || !rate_ok(d, &[own])
            || !gaps_on_rate(&s, tk.as_str(), None, start, end, own)
        {
            return refuse(
                "tl_rate",
                "that isn't a whole frame of the media's rate",
                "media list",
            );
        }
        let n = next_item(&s);
        let k = key('i', n);
        let kind = match s
            .media
            .iter()
            .find(|(x, _)| x == &mk)
            .and_then(|(_, v)| v.as_str())
        {
            Some("generator") => "generator",
            _ => "clip",
        };
        let media_name = s.media_field(&mk, f::NAME).unwrap_or("").to_owned();
        let name = name.clone().unwrap_or_else(|| media_name.clone());
        let mut ops = vec![
            set(prop(&k, f::TRACK), tk.as_str()),
            set(prop(&k, f::GEOM), ticks::pair_text(start, d)),
            set(prop(&k, f::SRC), ticks::text(src)),
            set(prop(&k, f::NAME), name.clone()),
            set(prop(&k, f::MEDIA), mk.as_str()),
            set(prop(&k, f::OFF), "0"),
            set(prop(&k, f::EDIT), "1"),
            set(prop(&k, f::RETIMED), "0"),
        ];
        if let Some(r) = own {
            // the rate the write-back writes its times at: its media's first clip's
            ops.push(set(prop(&k, f::RATE), ticks::rate_text(r)));
        }
        let edits = vec![
            Edit::List {
                root: ITEMS.to_owned(),
                ops: vec![ListOp::Insert {
                    key: k.clone(),
                    at: Anchor::End,
                    value: Value::str(kind),
                }],
            },
            Edit::Map {
                root: ITEM_PROPS.to_owned(),
                ops,
            },
            Edit::Map {
                root: TIMELINE.to_owned(),
                ops: vec![set(t::MARK_I.to_owned(), n.to_string())],
            },
        ];
        let data = Json::obj([
            ("item", Json::from(address_of(ITEMS, &k))),
            ("start", time_data(&s, start)),
            ("track", Json::from(ta.as_str())),
            ("media", Json::from(ma.as_str())),
        ]);
        done_at(
            edits,
            format!(
                "{}\t{}\t{ta}\n",
                address_of(ITEMS, &k),
                show_t(&s, start, true)
            ),
            data,
            start,
            Some(address_of(TRACKS, &tk).as_str()),
        )
    }))
}

/// Marker `mk`'s record time: an item's media time placed by the item, else its own.
fn marker_rec(s: &Doc<'_>, mk: &Key) -> Option<i64> {
    let at = s.marker(mk, f::AT).and_then(ticks::parse)?;
    let owner = s.marker(mk, f::OWNER).unwrap_or("");
    match Key::new(owner).ok().filter(|o| s.live(o)) {
        Some(o) => {
            let (st, _) = s.geom(&o)?;
            let src = s.item(&o, f::SRC).and_then(ticks::parse)?;
            Some(st.saturating_add(at.saturating_sub(src)))
        }
        None => Some(at),
    }
}

/// The track a marker owned by `owner` is in, by address: its item's track, or the track that
/// owns it; `None` for a timeline marker (the doc's own time).
fn marker_track(s: &Doc<'_>, owner: &str) -> Option<String> {
    let k = Key::new(owner).ok()?;
    if s.live(&k) {
        return s.item(&k, f::TRACK).and_then(track_address);
    }
    has(s.tracks, &k).then(|| address_of(TRACKS, &k))
}

/// A marker's owner as `--on` names it: an item's or a track's key, or empty for the timeline.
fn owner_of(on: &str) -> Result<String, ToolError> {
    if on == "timeline" {
        return Ok(String::new());
    }
    if let Some(k) = key_of(ITEMS, on) {
        return Ok(k.as_str().to_owned());
    }
    key_of(TRACKS, on)
        .map(|k| k.as_str().to_owned())
        .ok_or_else(|| usage("--on takes @item:N, @trk:N or timeline"))
}

/// A marker's stored time for record time `rec` on `owner`: an item's media time, else `rec`.
/// `None` when it falls outside the item.
fn marker_at(s: &Doc<'_>, owner: &str, rec: i64) -> Option<i64> {
    match Key::new(owner).ok().filter(|o| s.live(o)) {
        Some(o) => {
            let (st, d) = s.geom(&o)?;
            if rec < st || rec > st.saturating_add(d) {
                return None;
            }
            let src = s.item(&o, f::SRC).and_then(ticks::parse)?;
            Some(src.saturating_add(rec - st))
        }
        None => Some(rec),
    }
}

/// An owner every marker verb may write under, or the refusal.
fn owner_ok(s: &Doc<'_>, owner: &str, on: &str) -> Option<R> {
    if owner.is_empty() {
        return None;
    }
    let Ok(k) = Key::new(owner) else {
        return Some(not_found(on, "track list"));
    };
    if has(s.tracks, &k) {
        return None;
    }
    writable(s, &k, on)
}

fn parse_marker_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(
        args,
        &["--on", "--at", "--name", "--dur", "--note", "--color"],
        &["--snap"],
    )?;
    let on = a.val("--on").ok_or_else(|| usage("give --on"))?.to_owned();
    let owner = owner_of(&on)?;
    let at = a
        .val("--at")
        .ok_or_else(|| usage("give --at T"))?
        .to_owned();
    let name = a
        .val("--name")
        .ok_or_else(|| usage("give --name"))?
        .to_owned();
    let dur = a.val("--dur").map(str::to_owned);
    let note = a.val("--note").unwrap_or("").to_owned();
    let color = a.val("--color").unwrap_or("").to_owned();
    let snap = a.has("--snap");
    Ok(typed("timeline.marker-add", move |ctx: &VerbCtx<'_>| {
        let view = ctx.view();
        let s = Doc::of(ctx);
        if let Some(r) = owner_ok(&s, &owner, &on) {
            return r;
        }
        let next = call_of(
            "marker add",
            &[
                ("--on", Some(on.as_str())),
                ("--at", Some(at.as_str())),
                ("--name", Some(name.as_str())),
                ("--dur", dur.as_deref()),
            ],
        );
        // the read that shows the owner's range
        let owner_read = if on.starts_with("@item:") {
            format!("clip show {on}")
        } else {
            READ_TIMES.to_owned()
        };
        let rec = match read_t(&s, &at, snap, None) {
            Ok(v) => v,
            Err(w) => return time_refusal(w, Some(&next)),
        };
        let d = match dur.as_deref().map(|x| read_d(&s, x, snap)).transpose() {
            Ok(v) => v.unwrap_or(0),
            Err(w) => return time_refusal(w, Some(&next)),
        };
        let Some(stored) = marker_at(&s, &owner, rec).filter(|_| d >= 0 && rec >= 0) else {
            return refuse(
                "tl_range",
                "the marker falls outside its owner",
                &owner_read,
            );
        };
        let n = next_mark(view, MARKERS, 'k', t::MARK_K);
        let k = key('k', n);
        let ops = vec![
            set(prop(&k, f::OWNER), owner.clone()),
            set(prop(&k, f::AT), ticks::text(stored)),
            set(prop(&k, f::DUR), ticks::text(d)),
            set(prop(&k, f::NAME), name.clone()),
            set(prop(&k, f::NOTE), note.clone()),
            set(prop(&k, f::COLOR), color.clone()),
        ];
        done_at(
            vec![
                Edit::List {
                    root: MARKERS.to_owned(),
                    ops: vec![ListOp::Insert {
                        key: k.clone(),
                        at: Anchor::End,
                        value: Value::str("marker"),
                    }],
                },
                Edit::Map {
                    root: MARKER_PROPS.to_owned(),
                    ops,
                },
                Edit::Map {
                    root: TIMELINE.to_owned(),
                    ops: vec![set(t::MARK_K.to_owned(), n.to_string())],
                },
            ],
            format!(
                "{}\t{}\t{on}\n",
                address_of(MARKERS, &k),
                show_t(&s, rec, true)
            ),
            Json::obj([
                ("marker", Json::from(address_of(MARKERS, &k))),
                ("at", time_data(&s, rec)),
                ("owner", Json::from(on.as_str())),
            ]),
            rec,
            marker_track(&s, &owner).as_deref(),
        )
    }))
}

fn parse_marker_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(
        args,
        &["--name", "--note", "--color", "--at", "--dur"],
        &["--snap"],
    )?;
    let addr = a.one("marker (@mk:N)")?.to_owned();
    let k = addr_of(MARKERS, &addr, "a marker (@mk:N)")?;
    let name = a.val("--name").map(str::to_owned);
    let note = a.val("--note").map(str::to_owned);
    let color = a.val("--color").map(str::to_owned);
    let at = a.val("--at").map(str::to_owned);
    let dur = a.val("--dur").map(str::to_owned);
    let snap = a.has("--snap");
    if [&name, &note, &color, &at, &dur]
        .iter()
        .all(|x| x.is_none())
    {
        return Err(usage("give --name, --note, --color, --at or --dur"));
    }
    Ok(typed("timeline.marker-set", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !has(s.markers, &k) {
            return not_found(&addr, "marker list");
        }
        let owner = s.marker(&k, f::OWNER).unwrap_or("").to_owned();
        if let Some(r) = owner_ok(&s, &owner, &addr) {
            return r;
        }
        let next = call_of(
            &format!("marker set {addr}"),
            &[("--at", at.as_deref()), ("--dur", dur.as_deref())],
        );
        let mut ops = Vec::new();
        let old_at = s.marker(&k, f::AT).and_then(ticks::parse).unwrap_or(0);
        let mut new_at = old_at;
        // its record time after the set: the one typed, else where it is
        let mut new_rec = marker_rec(&s, &k);
        if let Some(x) = &at {
            let rec = match read_t(&s, x, snap, None) {
                Ok(v) => v,
                Err(w) => return time_refusal(w, Some(&next)),
            };
            match marker_at(&s, &owner, rec).filter(|_| rec >= 0) {
                Some(v) => new_at = v,
                None => {
                    return refuse(
                        "tl_range",
                        "the marker falls outside its owner",
                        "marker list",
                    )
                }
            }
            new_rec = Some(rec);
        }
        if let Some(x) = &dur {
            match read_d(&s, x, snap) {
                Ok(d) if d >= 0 => {
                    if s.marker(&k, f::DUR) != Some(ticks::text(d).as_str()) {
                        ops.push(set(prop(&k, f::DUR), ticks::text(d)));
                    }
                }
                Ok(_) => return refuse("tl_range", "a negative duration", "marker list"),
                Err(w) => return time_refusal(w, Some(&next)),
            }
        }
        if new_at != old_at {
            ops.push(set(prop(&k, f::AT), ticks::text(new_at)));
        }
        for (fld, v) in [(f::NAME, &name), (f::NOTE, &note), (f::COLOR, &color)] {
            if let Some(v) = v {
                if s.marker(&k, fld) != Some(v.as_str()) {
                    ops.push(set(prop(&k, fld), v.clone()));
                }
            }
        }
        let new_name = name
            .clone()
            .unwrap_or_else(|| s.marker(&k, f::NAME).unwrap_or("").to_owned());
        let edits = map_edit(MARKER_PROPS, ops).into_iter().collect();
        let out = format!("{addr}\t{}\n", one_line(&new_name));
        let data = Json::obj([
            ("marker", Json::from(addr.as_str())),
            ("name", text_data(&new_name)),
        ]);
        match new_rec {
            Some(rec) => done_at(edits, out, data, rec, marker_track(&s, &owner).as_deref()),
            None => done(edits, out, data),
        }
    }))
}

fn parse_marker_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let addr = a.one("marker (@mk:N)")?.to_owned();
    let k = addr_of(MARKERS, &addr, "a marker (@mk:N)")?;
    Ok(typed("timeline.marker-delete", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !has(s.markers, &k) {
            return not_found(&addr, "marker list");
        }
        let owner = s.marker(&k, f::OWNER).unwrap_or("").to_owned();
        if let Some(r) = owner_ok(&s, &owner, &addr) {
            return r;
        }
        let mut edits = vec![Edit::List {
            root: MARKERS.to_owned(),
            ops: vec![ListOp::Delete { key: k.clone() }],
        }];
        edits.extend(map_edit(MARKER_PROPS, drop_fields(s.kp, &k)));
        let out = format!("{addr}\tdeleted\n");
        let data = Json::obj([
            ("marker", Json::from(addr.as_str())),
            ("deleted", Json::from(true)),
        ]);
        // where it was
        match marker_rec(&s, &k) {
            Some(rec) => done_at(edits, out, data, rec, marker_track(&s, &owner).as_deref()),
            None => done(edits, out, data),
        }
    }))
}

fn parse_track_rename(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let (addr, name) = match a.pos.as_slice() {
        [x, y] => (x.clone(), y.clone()),
        _ => return Err(usage("give a track (@trk:N) and its new name")),
    };
    let k = addr_of(TRACKS, &addr, "a track (@trk:N)")?;
    Ok(typed("timeline.track-rename", move |ctx: &VerbCtx<'_>| {
        let s = Doc::of(ctx);
        if !has(s.tracks, &k) {
            return not_found(&addr, "track list");
        }
        let ops = if s.track(&k, f::NAME) == Some(name.as_str()) {
            Vec::new()
        } else {
            vec![set(prop(&k, f::NAME), name.clone())]
        };
        done(
            map_edit(super::TRACK_PROPS, ops).into_iter().collect(),
            format!("{addr}\t{}\n", one_line(&name)),
            Json::obj([
                ("track", Json::from(addr.as_str())),
                ("name", text_data(&name)),
            ]),
        )
    }))
}
