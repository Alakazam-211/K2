//! The live rows, each returning `Err("<row>: ...")` so a planted bug can say which row
//! caught it.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Applied, ListOp, MapOp, MapOpKind, Value};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::session::{
    test_support::{carrier_judge, CarrierVerdict},
    AuditEvent, ChangeEvent, ClaimEnd, ClaimSpan, ConnId, DiskCheck, Pack, PlayState, RosterEvent,
    Session, SpliceError, TimePosition, View, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::view_index::ViewSchema;
use k2_mpc_timeline::live::view::VIEW;
use k2_mpc_timeline::live::{
    model, pair_save, splice, ticks, watch_line, LiveLimits, TimelineLive, ITEMS, ITEM_PROPS,
    MARKERS, MARKER_PROPS, MEDIA, MEDIA_PROPS, TIMELINE, TRACKS, TRACK_PROPS,
};
use k2_mpc_timeline::otio::read_otio;
use k2_mpc_timeline::ReadLimits;

use super::{
    app_save, clip, cut, cut_clips, cut_tree, decimal_rate, decimal_tree, decimal_tree_5994, field,
    file, fx, gap, jraw, js, list, named, pretty, rt, run, run_based, set_field, stdout,
    time_of_day, timeline, track, tree_25, with_df_entry, ClipSpec, Fx, CLIP_LEVEL, J,
};

/// One frame at 24 fps, in ticks.
pub const F: i64 = 10_584_000_000;

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

/// The live doc's roots as `c` reads them (a read renders the file: its failure is the row's).
fn read_content(row: &str, s: &Session, c: ConnId) -> Result<Vec<(String, Neutral)>, String> {
    s.read(c)
        .map(|r| r.content)
        .map_err(|e| format!("{row}: the read failed: {e}"))
}

fn open(tag: &str, bytes: &[u8]) -> (Fx, Arc<Session>) {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    (f, s)
}

/// An agent's edit verb with a fresh base; exit 0 required.
fn verb(row: &str, s: &Session, c: ConnId, a: &[&str]) -> Result<String, String> {
    let o = run_based(s, c, a).map_err(|e| format!("{row}: {a:?}: {e}"))?;
    need(o.exit == 0, row, || {
        format!("{a:?} exited {}: {}", o.exit, stdout(&o))
    })?;
    Ok(stdout(&o))
}

/// An agent's edit verb with a fresh base, which must be refused with `code`; nothing applies.
fn refused(row: &str, s: &Session, c: ConnId, a: &[&str], code: &str) -> Result<(), String> {
    let v0 = run(s, c, &["timeline", "info"])
        .map_err(|e| format!("{row}: {e}"))?
        .version;
    let o = run_based(s, c, a).map_err(|e| format!("{row}: {a:?}: {e}"))?;
    let out = stdout(&o);
    need(
        o.exit == 2 && out.starts_with(&format!("refused: {code}:")) && out.contains("\nnext: "),
        row,
        || format!("{a:?}: not refused {code}: exit {}: {out}", o.exit),
    )?;
    let v1 = run(s, c, &["timeline", "info"])
        .map_err(|e| format!("{row}: {e}"))?
        .version;
    need(v0 == v1, row, || format!("{a:?}: something applied"))
}

/// Write back as `c`; the write must happen.
fn flush(row: &str, s: &Session, c: ConnId, what: &str) -> Result<(), String> {
    let out = s
        .flush(c)
        .map_err(|e| format!("{row}: {what}: the write-back failed: {e}"))?;
    need(out.written, row, || format!("{what}: nothing was written"))
}

fn drain(rx: &Receiver<ChangeEvent>) -> Vec<ChangeEvent> {
    rx.try_iter().collect()
}

fn geom(a: i64, d: i64) -> String {
    ticks::pair_text(a * F, d * F)
}

/// The items of a file as the tier-0 reader places them: `(name, start, duration, source in,
/// enabled)` per track, gaps left out, times in ticks.
type Placed = Vec<Vec<(String, i64, i64, i64, bool)>>;

fn placed(bytes: &[u8]) -> Result<Placed, String> {
    let d = read_otio(bytes, &ReadLimits::default()).map_err(|e| e.to_string())?;
    let t = d.timelines.first().ok_or("no timeline")?;
    Ok(t.tracks
        .iter()
        .map(|tr| {
            tr.items
                .iter()
                .filter(|i| i.kind.name() != "gap")
                .map(|i| {
                    let tk = |x| ticks::of(x).unwrap_or(i64::MIN);
                    (
                        i.name.clone(),
                        tk(i.start),
                        tk(i.duration),
                        i.source_in.map_or(0, tk),
                        i.enabled,
                    )
                })
                .collect()
        })
        .collect())
}

/// The longest common prefix and suffix of two byte strings.
fn common(a: &[u8], b: &[u8]) -> (usize, usize) {
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let max = a.len().min(b.len()) - p;
    let s = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max)
        .take_while(|(x, y)| x == y)
        .count();
    (p, s)
}

/// A field of the `timeline` map.
fn tl_field(c: &[(String, Neutral)], k: &str) -> Option<String> {
    c.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == TIMELINE => m.get(k).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn contains(hay: &[u8], needle: &str) -> bool {
    find(hay, needle.as_bytes()).is_some()
}

// ---------- the store ----------

/// The cut opens live; the keyed state lists every track, item, marker and media with their
/// fields, times as ticks.
pub fn row_opens_live_with_every_field() -> Result<(), String> {
    const ROW: &str = "row_opens_live_with_every_field";
    let (f, s) = open("fields", &cut());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect()
    };
    need(
        list(&c, TRACKS) == pairs(&[("t1", "video"), ("t2", "audio"), ("t3", "video")]),
        ROW,
        || format!("tracks {:?}", list(&c, TRACKS)),
    )?;
    need(
        list(&c, ITEMS)
            == pairs(&[
                ("i1", "clip"),
                ("i2", "clip"),
                ("i3", "transition"),
                ("i4", "clip"),
                ("i5", "clip"),
                ("i6", "clip"),
            ]),
        ROW,
        || format!("items {:?}", list(&c, ITEMS)),
    )?;
    need(list(&c, MARKERS).len() == 3, ROW, || {
        format!("markers {:?}", list(&c, MARKERS))
    })?;
    need(list(&c, MEDIA).len() == 4, ROW, || {
        format!("media {:?}", list(&c, MEDIA))
    })?;
    let want: &[(&str, &str, &str, String)] = &[
        (ITEM_PROPS, "i1", "geom", geom(24, 48)),
        (ITEM_PROPS, "i1", "src", (100 * F).to_string()),
        (ITEM_PROPS, "i1", "track", "t1".into()),
        (ITEM_PROPS, "i1", "media", "m1".into()),
        (ITEM_PROPS, "i1", "name", "Shot A".into()),
        (ITEM_PROPS, "i1", "off", "0".into()),
        (ITEM_PROPS, "i1", "edit", "1".into()),
        (ITEM_PROPS, "i3", "geom", geom(90, 12)),
        (ITEM_PROPS, "i3", "tr", geom(6, 6)),
        (ITEM_PROPS, "i3", "edit", "0".into()),
        (ITEM_PROPS, "i5", "geom", geom(144, 24)),
        (ITEM_PROPS, "i5", "media", "m1".into()),
        (
            ITEM_PROPS,
            "i6",
            "geom",
            ticks::pair_text(0, 2 * ticks::PER_SECOND),
        ),
        (ITEM_PROPS, "i6", "track", "t2".into()),
        (MARKER_PROPS, "k1", "owner", "i1".into()),
        (MARKER_PROPS, "k1", "at", (110 * F).to_string()),
        (MARKER_PROPS, "k1", "name", "beat".into()),
        (MARKER_PROPS, "k1", "note", "on the downbeat".into()),
        (MARKER_PROPS, "k1", "color", "RED".into()),
        (MARKER_PROPS, "k2", "owner", "t1".into()),
        (MARKER_PROPS, "k3", "owner", String::new()),
        (MARKER_PROPS, "k3", "at", (48 * F).to_string()),
        (MEDIA_PROPS, "m1", "file", "shot_a.mov".into()),
        (MEDIA_PROPS, "m4", "rate", "48000/1".into()),
        (TRACK_PROPS, "t1", "name", "V1".into()),
        (TRACK_PROPS, "t1", "nid", "V1".into()),
        (TRACK_PROPS, "t2", "nid", "A1".into()),
    ];
    for (root, elem, fld, v) in want {
        let got = field(&c, root, elem, fld);
        need(got.as_deref() == Some(v.as_str()), ROW, || {
            format!("{root} {elem} {fld}: {got:?}, not {v:?}")
        })?;
    }
    let tl = |k: &str| tl_field(&c, k);
    need(
        tl("tc") == Some((86_400 * F).to_string())
            && tl("rate").as_deref() == Some("24/1")
            && tl("name").as_deref() == Some("Cut")
            && tl("format").as_deref() == Some("otio"),
        ROW,
        || format!("the timeline: tc {:?} rate {:?}", tl("tc"), tl("rate")),
    )?;
    // no path on the wire or in the store
    let all = format!("{c:?}");
    need(!all.contains("/media/"), ROW, || {
        "a media path is in the store".into()
    })
}

/// The kit's view index: tracks are lanes, items bars at their record times in ticks.
pub fn row_view_index_is_the_lanes() -> Result<(), String> {
    const ROW: &str = "row_view_index_is_the_lanes";
    let (f, s) = open("view", &cut());
    let a = f.person(&s);
    let v = s
        .view_index(a)
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no view index"))?;
    let titles: Vec<Option<String>> = v.containers.iter().map(|c| c.title.clone()).collect();
    need(
        titles == vec![Some("V1".into()), Some("A1".into()), Some("V2".into())],
        ROW,
        || format!("the lanes {titles:?}"),
    )?;
    need(v.units.len() == 6, ROW, || {
        format!("{} units", v.units.len())
    })?;
    let a1 = v
        .units
        .iter()
        .find(|u| u.key == "@item:1")
        .ok_or_else(|| format!("{ROW}: no @item:1"))?;
    let b = a1.bbox.unwrap_or_default();
    need(
        a1.container == "@trk:1" && b.x == 24 * F && b.w == 48 * F && a1.name == "Shot A",
        ROW,
        || format!("@item:1 is {a1:?}"),
    )?;
    let derived = TimelineLive::default().page_derived(
        &View::of_content(&read_content(ROW, &s, a)?).into_keyed(),
        &[model::key('i', 1)],
    );
    need(
        derived.first().map(|d| d.values.clone())
            == Some(vec![("geom".into(), "01:00:01:00-01:00:03:00".into())]),
        ROW,
        || format!("the derived timecodes {derived:?}"),
    )
}

/// What can't open live is declined, typed, and stays at tier 0.
pub fn row_declines_are_typed() -> Result<(), String> {
    const ROW: &str = "row_declines_are_typed";
    let pack = TimelineLive::default();
    let decline = |bytes: &[u8]| pack.decode(bytes).err().map(|d| d.code);
    let one = |c: &ClipSpec| {
        file(&timeline(
            "x",
            None,
            vec![track("V1", "Video", vec![clip(c)], vec![])],
            vec![],
        ))
    };
    let collection = {
        let t = pretty(&cut_tree(&cut_clips()), 2);
        format!(
            "{{\"OTIO_SCHEMA\": \"SerializableCollection.1\", \"metadata\": {{}}, \"name\": \"c\", \"children\": [{t}, {t}]}}"
        )
        .into_bytes()
    };
    let layered = {
        let J::Obj(mut kv) = cut_tree(&cut_clips()) else {
            unreachable!()
        };
        if let Some((_, J::Obj(stack))) = kv.iter_mut().find(|(k, _)| *k == "tracks") {
            if let Some((_, J::Arr(ch))) = stack.iter_mut().find(|(k, _)| *k == "children") {
                ch.push(gap("4.0", "24.0"));
            }
        }
        file(&J::Obj(kv))
    };
    let off_tick = one(&ClipSpec::new("odd", "file:///o.mov", "1.0", "1.0", "11.0"));
    let long = one(&ClipSpec::new(
        "long",
        "file:///l.mov",
        "0.0",
        "900000.0",
        "24.0",
    ));
    let small = TimelineLive::new(LiveLimits {
        max_items: 2,
        ..LiveLimits::default()
    });
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("a collection", decline(&collection)),
        ("a layer in the stack", decline(&layered)),
        ("a time off the tick grid", decline(&off_tick)),
        ("a track past 2^53 ticks", decline(&long)),
        (
            "more items than the bound",
            small.decode(&cut()).err().map(|d| d.code),
        ),
        ("not JSON", decline(b"{ not json")),
    ];
    let want = [
        "live_many_timelines",
        "live_structure",
        "live_time_off_tick",
        "live_time_range",
        "live_too_large",
        "unreadable",
    ];
    for ((what, got), w) in cases.iter().zip(want) {
        need(*got == Some(w), ROW, || format!("{what}: {got:?}, not {w}"))?;
    }
    need(decline(&cut()).is_none(), ROW, || {
        "control: the cut opens".into()
    })
}

/// No read returns the whole timeline: lists page at 200, and `--limit` above it is refused.
pub fn row_reads_are_bounded_and_paged() -> Result<(), String> {
    const ROW: &str = "row_reads_are_bounded_and_paged";
    let (f, s) = open("paged", &super::big(1, 450, 3, 0));
    let a = f.agent(&s);
    let o = run(&s, a, &["clip", "list", "@trk:1"]).map_err(|e| format!("{ROW}: {e}"))?;
    let out = stdout(&o);
    let rows = out.lines().filter(|l| l.starts_with("@item:")).count();
    need(
        rows == 200 && out.contains("next: clip list @trk:1 --after @item:200"),
        ROW,
        || {
            format!(
                "the first page holds {rows} rows: {}",
                out.lines().last().unwrap_or("")
            )
        },
    )?;
    let o = run(&s, a, &["clip", "list", "@trk:1", "--after", "@item:400"])
        .map_err(|e| format!("{ROW}: {e}"))?;
    let rows = stdout(&o)
        .lines()
        .filter(|l| l.starts_with("@item:"))
        .count();
    need(rows == 50 && !stdout(&o).contains("next:"), ROW, || {
        format!("the last page holds {rows} rows")
    })?;
    let o = run(&s, a, &["clip", "list", "@trk:1", "--limit", "300"]);
    need(o.as_ref().map_or(true, |o| o.exit != 0), ROW, || {
        "--limit 300 was taken".into()
    })
}

// ---------- verbs ----------

/// `clip move` is one change setting one field, `geom`; a watcher prints it. (Its latency is
/// the release measurement's, never a timed assert here.)
pub fn row_move_one_event() -> Result<(), String> {
    const ROW: &str = "row_move_one_event";
    let (f, s) = open("move", &cut());
    let watcher = f.person(&s);
    let a = f.agent(&s);
    let rx = s.subscribe(watcher).map_err(|e| format!("{ROW}: {e}"))?;
    drain(&rx);
    verb(ROW, &s, a, &["clip", "move", "@item:5", "--to", "+2f"])?;
    let ev = drain(&rx);
    need(ev.len() == 1, ROW, || {
        format!("{} change events, not one", ev.len())
    })?;
    let ok = match &ev[0].applied {
        Applied::Map { root, ops } => {
            root == ITEM_PROPS
                && ops.len() == 1
                && ops[0].kind == MapOpKind::Set
                && ops[0].key == "i5\x1fgeom"
                && ops[0].after == Some(Value::str(geom(146, 24)))
        }
        _ => false,
    };
    need(ok, ROW, || format!("not one geom set: {:?}", ev[0].applied))?;
    let line = watch_line(&ev[0]);
    println!("the watcher prints: {line}");
    need(
        line.contains(&format!("@item:5 geom = {}", geom(146, 24))),
        ROW,
        || format!("the watch line: {line}"),
    )?;
    Ok(())
}

/// What a verb refuses, typed, with nothing applied: a move onto another clip, a time between
/// frames (taken with `--snap`), a clip a transition joins, a track of another kind, a
/// transition; an edit without a base.
pub fn row_refusals_are_typed() -> Result<(), String> {
    const ROW: &str = "row_refusals_are_typed";
    let (f, s) = open("refuse", &cut());
    let a = f.agent(&s);
    refused(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:5", "--to", "100f"],
        "tl_overlap",
    )?;
    refused(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:5", "--to", "+1/48s"],
        "tl_off_grid",
    )?;
    verb(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:5", "--to", "+1/48s", "--snap"],
    )?;
    refused(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:2", "--to", "+1f"],
        "tl_transition",
    )?;
    refused(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:1", "--to", "0f", "--track", "@trk:2"],
        "tl_track_kind",
    )?;
    refused(
        ROW,
        &s,
        a,
        &["clip", "rename", "@item:3", "x"],
        "tl_not_editable",
    )?;
    // the kit's guard: an agent's edit names its base
    let o = run(&s, a, &["clip", "rename", "@item:1", "x"]);
    need(o.as_ref().map_or(true, |o| o.exit != 0), ROW, || {
        "an edit without --base was taken".into()
    })
}

/// A read sorts by start: two clips added to an empty track, the later one first, list in
/// time order.
pub fn row_list_is_time_ordered() -> Result<(), String> {
    const ROW: &str = "row_list_is_time_ordered";
    let (f, s) = open("order", &cut());
    let a = f.agent(&s);
    for at in ["48f", "0f"] {
        verb(
            ROW,
            &s,
            a,
            &[
                "clip", "add", "--media", "@med:2", "--track", "@trk:3", "--at", at, "--in", "0f",
                "--dur", "12f",
            ],
        )?;
    }
    let o = run(&s, a, &["clip", "list", "@trk:3"]).map_err(|e| format!("{ROW}: {e}"))?;
    let keys: Vec<String> = stdout(&o)
        .lines()
        .filter_map(|l| l.split('\t').next())
        .map(str::to_owned)
        .collect();
    need(
        keys == vec!["@item:8".to_owned(), "@item:7".to_owned()],
        ROW,
        || format!("the track's items in this order: {keys:?}"),
    )
}

/// A deleted clip's key is never minted again.
pub fn row_mint_past_the_mark() -> Result<(), String> {
    const ROW: &str = "row_mint_past_the_mark";
    let (f, s) = open("mint", &cut());
    let a = f.agent(&s);
    let add = [
        "clip", "add", "--media", "@med:2", "--track", "@trk:3", "--at", "0f", "--in", "0f",
        "--dur", "12f",
    ];
    let o = verb(ROW, &s, a, &add)?;
    need(o.starts_with("@item:7\t"), ROW, || {
        format!("the first add: {o}")
    })?;
    verb(ROW, &s, a, &["clip", "delete", "@item:7"])?;
    let o = verb(ROW, &s, a, &add)?;
    need(o.starts_with("@item:8\t"), ROW, || {
        format!("the second add: {o}")
    })
}

// ---------- the splice ----------

/// The generated corpus: the cut, a decimal-rate timeline, a time-of-day timeline, a larger
/// one.
pub fn generated_corpus() -> Vec<(String, Vec<u8>)> {
    vec![
        ("cut.otio".into(), cut()),
        ("decimal.otio".into(), decimal_rate()),
        ("day.otio".into(), time_of_day()),
        ("big.otio".into(), super::big(3, 40, 7, 12)),
    ]
}

/// The kit's byte-exact row (`carrier_judge`): the paired hook, the file opened live, and the
/// same after a restart.
pub fn row_untouched_writes_back_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_untouched_writes_back_byte_for_byte";
    let corpus = generated_corpus();
    let refs: Vec<(&str, &[u8])> = corpus
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    match carrier_judge(Arc::new(TimelineLive::default()), &refs) {
        Ok(CarrierVerdict::ByteExact { files }) => {
            println!("carrier_judge ByteExact over {files} generated files");
            need(files == corpus.len(), ROW, || {
                format!("{files} files judged")
            })
        }
        other => Err(format!("{ROW}: {other:?}")),
    }
}

fn decoded(row: &str, bytes: &[u8]) -> Result<Vec<(String, Neutral)>, String> {
    TimelineLive::default()
        .decode(bytes)
        .map(|c| c.roots)
        .map_err(|e| format!("{row}: control: {e}"))
}

fn splice_into(
    row: &str,
    carrier: &[u8],
    content: &[(String, Neutral)],
) -> Result<Vec<u8>, String> {
    let pack = TimelineLive::default();
    let d = decoded(row, carrier)?;
    let pairing = pack
        .pairing(&d, k2_mpc_timeline::live::CODEC)
        .map_err(|e| format!("{row}: {e}"))?;
    splice::encode_from_paired(carrier, &pairing, content, &LiveLimits::default())
        .map_err(|e| format!("{row}: the splice failed: {e}"))
}

/// A clip moved later writes back with only its track's gaps and its own stamp changed: the
/// gap before it grows, the trailing gap shrinks so the track keeps its length, every other
/// object keeps its bytes.
pub fn row_move_rewrites_only_its_gaps() -> Result<(), String> {
    const ROW: &str = "row_move_rewrites_only_its_gaps";
    let carrier = cut();
    let mut c = decoded(ROW, &carrier)?;
    set_field(&mut c, ITEM_PROPS, "i5", "geom", &geom(150, 24));
    let out = splice_into(ROW, &carrier, &c)?;
    let got = placed(&out).map_err(|e| format!("{ROW}: {e}"))?;
    let d = got[0].iter().find(|i| i.0 == "Shot A again");
    need(d.map(|i| i.1) == Some(150 * F), ROW, || {
        format!("the moved clip reads back at {d:?}")
    })?;
    let clips = cut_clips();
    for spec in [&clips[0], &clips[1], &clips[2], &clips[4]] {
        let text = pretty(&clip(spec), CLIP_LEVEL);
        need(contains(&out, &text), ROW, || {
            format!("{}'s bytes changed", spec.name)
        })?;
    }
    let (p, sfx) = common(&carrier, &out);
    let c_text = pretty(&clip(&clips[2]), CLIP_LEVEL);
    let c_end = find(&carrier, c_text.as_bytes()).unwrap_or(0) + c_text.len();
    let a1_at = find(&carrier, b"\"name\": \"A1\"").unwrap_or(carrier.len());
    need(p >= c_end && carrier.len() - sfx <= a1_at, ROW, || {
        format!(
            "bytes changed outside the moved clip's gaps: first at {p} (Shot C ends at {c_end}), last at {} (A1 at {a1_at})",
            carrier.len() - sfx
        )
    })?;
    let s = String::from_utf8_lossy(&out);
    need(s.matches("\"value\": 18.0").count() >= 2, ROW, || {
        "the gaps around it aren't 18 frames".into()
    })
}

/// A clip the splice touched carries our stamp, and an app's re-save that also trims it (so its
/// derived identity changes) pairs it by that stamp.
pub fn row_stamp_pairs_a_resave() -> Result<(), String> {
    const ROW: &str = "row_stamp_pairs_a_resave";
    let carrier = cut();
    let mut c = decoded(ROW, &carrier)?;
    let doc = tl_field(&c, "doc").unwrap_or_default();
    set_field(&mut c, ITEM_PROPS, "i2", "name", "B2");
    let out = splice_into(ROW, &carrier, &c)?;
    let stamp = format!("\"mpc\": {{\"key\": \"i2\", \"doc\": \"{doc}\"}}");
    need(contains(&out, &stamp), ROW, || {
        format!("the touched clip carries no stamp {stamp}")
    })?;
    // the app's save: Shot B renamed and trimmed at its head, our stamp kept
    let mut clips = cut_clips();
    clips[1].name = "B2".into();
    clips[1].src = "2.0".into();
    clips[1].dur = "22.0".into();
    clips[1].metadata = vec![(
        "mpc",
        J::Obj(vec![
            ("key", J::Str("i2".into())),
            ("doc", J::Str(doc.clone())),
        ]),
    )];
    let save = file(&cut_tree(&clips));
    let new = model::read(&save, &LiveLimits::default()).map_err(|e| format!("{ROW}: {e}"))?;
    let (keys, _) = pair_save(&View::of_content(&c), &new);
    let b = new.items.iter().position(|i| i.name == "B2");
    let got = b.map(|i| keys.items[i].as_str().to_owned());
    need(got.as_deref() == Some("i2"), ROW, || {
        format!("the re-saved clip pairs as {got:?}, not i2")
    })
}

/// A save in which two clips carry one stamp (a clip copied in an app) pairs neither by it; a
/// stamp from another doc pairs nothing; each falls to its derived identity.
pub fn row_copied_stamp_pairs_neither() -> Result<(), String> {
    const ROW: &str = "row_copied_stamp_pairs_neither";
    let c = decoded(ROW, &cut())?;
    let doc = tl_field(&c, "doc").unwrap_or_default();
    let st = |k: &str, d: &str| {
        vec![(
            "mpc",
            J::Obj(vec![("key", J::Str(k.into())), ("doc", J::Str(d.into()))]),
        )]
    };
    let mut clips = cut_clips();
    // Shot C, trimmed (its identity changes), and Shot A again both claim i4
    clips[2].metadata = st("i4", &doc);
    clips[2].src = "12.0".into();
    clips[2].dur = "34.0".into();
    clips[3].metadata = st("i4", &doc);
    // Shot B claims i1 from another doc
    clips[1].metadata = st("i1", "another-doc");
    let save = file(&cut_tree(&clips));
    let new = model::read(&save, &LiveLimits::default()).map_err(|e| format!("{ROW}: {e}"))?;
    let (keys, _) = pair_save(&View::of_content(&c), &new);
    let key_of = |name: &str| {
        new.items
            .iter()
            .position(|i| i.name == name)
            .map(|i| keys.items[i].as_str().to_owned())
    };
    let got = (key_of("Shot C"), key_of("Shot A again"), key_of("Shot B"));
    need(
        got == (Some("i7".into()), Some("i5".into()), Some("i2".into())),
        ROW,
        || format!("Shot C, Shot A again, Shot B pair as {got:?}, not i7 (new), i5, i2"),
    )
}

/// A file whose rate is written `29.97`: a trimmed clip keeps the token and the file's `30.0`
/// style.
pub fn row_rate_token_kept() -> Result<(), String> {
    const ROW: &str = "row_rate_token_kept";
    let carrier = decimal_rate();
    let mut c = decoded(ROW, &carrier)?;
    let g = field(&c, ITEM_PROPS, "i1", "geom").ok_or_else(|| format!("{ROW}: no geom"))?;
    let (a, d) = ticks::parse_pair(&g).ok_or_else(|| format!("{ROW}: geom {g}"))?;
    set_field(
        &mut c,
        ITEM_PROPS,
        "i1",
        "geom",
        &ticks::pair_text(a, d / 30 * 24),
    );
    let out = splice_into(ROW, &carrier, &c)?;
    let want = pretty(&rt("24.0", "29.97"), 7);
    need(contains(&out, &want), ROW, || {
        format!("the trimmed duration isn't written as {want:?}")
    })?;
    need(
        !contains(&out, "29.970") && !contains(&out, "29.97002997"),
        ROW,
        || "a rate token was rewritten".into(),
    )
}

/// A source time of day past 2^53 ticks round trips exactly: in the store, on the wire (a
/// keyed watch's snapshot), and through a head trim written back.
pub fn row_time_of_day_round_trips() -> Result<(), String> {
    const ROW: &str = "row_time_of_day_round_trips";
    let ntsc_frame: i64 = 8_475_667_200;
    let frame_4096: i64 = 62_015_625;
    let dawn = (2_592_000 * ntsc_frame).to_string();
    let tick = 353_894_401 * frame_4096;
    let (f, s) = open("day", &time_of_day());
    let p = f.person(&s);
    let c = read_content(ROW, &s, p)?;
    need(
        field(&c, ITEM_PROPS, "i1", "src").as_deref() == Some(dawn.as_str())
            && field(&c, ITEM_PROPS, "i2", "src") == Some(tick.to_string()),
        ROW,
        || {
            format!(
                "the store holds {:?} and {:?}",
                field(&c, ITEM_PROPS, "i1", "src"),
                field(&c, ITEM_PROPS, "i2", "src")
            )
        },
    )?;
    if let Some(mut w) = s
        .watch_keyed(p, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("{ROW}: {e}"))?
    {
        let wire: String = s
            .watch_snapshot(&mut w)
            .map_err(|e| format!("{ROW}: the snapshot: {e}"))?
            .iter()
            .map(|fr| fr.to_json())
            .collect();
        need(
            wire.contains(&format!("\"{dawn}\"")) && wire.contains(&format!("\"{tick}\"")),
            ROW,
            || "the snapshot doesn't carry the source times as exact strings".into(),
        )?;
    } else {
        return Err(format!("{ROW}: no keyed watch"));
    }
    // a one-frame head trim of the 4096 fps clip, written back
    let g = field(&c, ITEM_PROPS, "i2", "geom").ok_or_else(|| format!("{ROW}: no geom"))?;
    let (a, d) = ticks::parse_pair(&g).ok_or_else(|| format!("{ROW}: geom {g}"))?;
    let new_src = tick + frame_4096;
    s.edit(
        p,
        vec![Edit::Map {
            root: ITEM_PROPS.into(),
            ops: vec![
                MapOp::Set {
                    key: "i2\x1fgeom".into(),
                    value: Value::str(ticks::pair_text(a + frame_4096, d - frame_4096)),
                },
                MapOp::Set {
                    key: "i2\x1fsrc".into(),
                    value: Value::str(new_src.to_string()),
                },
            ],
        }],
        None,
    )
    .map_err(|e| format!("{ROW}: the trim: {e}"))?;
    flush(ROW, &s, p, "the trim")?;
    let back = TimelineLive::default()
        .decode(&f.bytes())
        .map_err(|e| format!("{ROW}: the written file: {e}"))?;
    need(
        field(&back.roots, ITEM_PROPS, "i2", "src") == Some(new_src.to_string())
            && field(&back.roots, ITEM_PROPS, "i1", "src").as_deref() == Some(dawn.as_str()),
        ROW,
        || {
            format!(
                "the file holds {:?} and {:?}",
                field(&back.roots, ITEM_PROPS, "i1", "src"),
                field(&back.roots, ITEM_PROPS, "i2", "src")
            )
        },
    )?;
    need(contains(&f.bytes(), "\"value\": 353894402"), ROW, || {
        "the trimmed source time isn't written as a whole frame".into()
    })
}

/// A pairing that doesn't fit the carrier is refused, typed, and nothing is spliced.
pub fn row_pairing_mismatch_refused_typed() -> Result<(), String> {
    const ROW: &str = "row_pairing_mismatch_refused_typed";
    let carrier = cut();
    let c = decoded(ROW, &carrier)?;
    let mut pairing = TimelineLive::default()
        .pairing(&c, k2_mpc_timeline::live::CODEC)
        .map_err(|e| format!("{ROW}: {e}"))?;
    for (r, keys) in &mut pairing.roots {
        if r == ITEMS {
            keys.pop();
        }
    }
    match splice::encode_from_paired(&carrier, &pairing, &c, &LiveLimits::default()) {
        Err(SpliceError::PairingMismatch(_)) => Ok(()),
        other => Err(format!(
            "{ROW}: not refused as a mismatch: {:?}",
            other.map(|b| b.len())
        )),
    }
}

// ---------- outside saves ----------

/// Through the kit: an app's save adds a clip before a live-edited one (in the leading gap);
/// every other item keeps its key; a rename then lands on the right clip.
pub fn row_save_adds_before_then_edit_lands_right() -> Result<(), String> {
    const ROW: &str = "row_save_adds_before_then_edit_lands_right";
    let (f, s) = open("adds", &cut());
    let a = f.agent(&s);
    f.advance(1_000);
    let mut clips = cut_clips();
    let insert = ClipSpec::new("Insert", "file:///media/insert.mov", "0.0", "24.0", "24.0");
    clips.push(insert);
    let J::Obj(mut kv) = cut_tree(&clips) else {
        unreachable!()
    };
    // the leading gap of V1 becomes the new clip
    if let Some((_, J::Obj(stack))) = kv.iter_mut().find(|(k, _)| *k == "tracks") {
        if let Some((_, J::Arr(tracks))) = stack.iter_mut().find(|(k, _)| *k == "children") {
            if let J::Obj(t) = &mut tracks[0] {
                if let Some((_, J::Arr(ch))) = t.iter_mut().find(|(k, _)| *k == "children") {
                    ch[0] = clip(&clips[5]);
                }
            }
        }
    }
    let save = file(&J::Obj(kv));
    app_save(&f.path(), &save);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("control: the save is taken in: {out:?}")
    })?;
    let c = read_content(ROW, &s, a)?;
    let keyed: Vec<(String, Option<String>)> = [
        "Shot A",
        "Shot B",
        "Shot C",
        "Shot A again",
        "Music",
        "Insert",
    ]
    .iter()
    .map(|n| ((*n).to_owned(), named(&c, ITEMS, ITEM_PROPS, n)))
    .collect();
    let want = ["i1", "i2", "i4", "i5", "i6", "i7"];
    for ((n, k), w) in keyed.iter().zip(want) {
        need(k.as_deref() == Some(w), ROW, || {
            format!("{n} is {k:?}, not {w}")
        })?;
    }
    verb(ROW, &s, a, &["clip", "rename", "@item:5", "D2"])?;
    flush(ROW, &s, a, "the write after the save")?;
    let got = placed(&f.bytes()).map_err(|e| format!("{ROW}: {e}"))?;
    let names: Vec<&str> = got[0].iter().map(|i| i.0.as_str()).collect();
    need(
        names == vec!["Insert", "Shot A", "Shot B", "dissolve", "Shot C", "D2"],
        ROW,
        || format!("the rename landed on the wrong clip: {names:?}"),
    )
}

/// The same at the pack's hooks: the save's pairing puts the keys on its clips, and a rename of
/// `i5` is spliced into that clip only.
pub fn row_pack_save_adds_before_then_edit_lands_right() -> Result<(), String> {
    const ROW: &str = "row_pack_save_adds_before_then_edit_lands_right";
    let synced = decoded(ROW, &cut())?;
    let mut clips = cut_clips();
    clips.insert(
        0,
        ClipSpec::new("Insert", "file:///media/insert.mov", "0.0", "24.0", "24.0"),
    );
    let tree = timeline(
        "Cut",
        Some(("86400.0", "24.0")),
        vec![
            track(
                "V1",
                "Video",
                vec![
                    clip(&clips[0]),
                    clip(&clips[1]),
                    clip(&clips[2]),
                    super::transition("6.0", "6.0", "24.0"),
                    clip(&clips[3]),
                    gap("12.0", "24.0"),
                    clip(&clips[4]),
                    gap("24.0", "24.0"),
                ],
                vec![super::marker("act two", "120.0", "0.0", "24.0", "BLUE", "")],
            ),
            track("A1", "Audio", vec![clip(&clips[5])], vec![]),
            track("V2", "Video", vec![], vec![]),
        ],
        vec![super::marker("chapter", "48.0", "0.0", "24.0", "GREEN", "")],
    );
    let save = file(&tree);
    let new =
        model::read(&save, &LiveLimits::default()).map_err(|e| format!("{ROW}: control: {e}"))?;
    let (keys, marks) = pair_save(&View::of_content(&synced), &new);
    let doc = new.doc_id.clone();
    let mut content = new.to_content(&keys, marks, &doc);
    set_field(&mut content, ITEM_PROPS, "i5", "name", "D2");
    let pack = TimelineLive::default();
    let pairing = pack
        .pairing(
            &new.to_content(&keys, marks, &doc),
            k2_mpc_timeline::live::CODEC,
        )
        .map_err(|e| format!("{ROW}: {e}"))?;
    let out = splice::encode_from_paired(&save, &pairing, &content, &LiveLimits::default())
        .map_err(|e| format!("{ROW}: the rename isn't spliced: {e}"))?;
    let got = placed(&out).map_err(|e| format!("{ROW}: {e}"))?;
    let names: Vec<&str> = got[0].iter().map(|i| i.0.as_str()).collect();
    need(
        names == vec!["Insert", "Shot A", "Shot B", "dissolve", "Shot C", "D2"],
        ROW,
        || format!("the rename landed on the wrong clip: {names:?}"),
    )
}

/// An app's save after a kit write keeps both: the save is the live doc, the kit's version a
/// conflict copy.
pub fn row_save_after_kit_write_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_save_after_kit_write_keeps_both";
    let (f, s) = open("after-write", &cut());
    let a = f.agent(&s);
    verb(ROW, &s, a, &["clip", "rename", "@item:1", "Kit"])?;
    flush(ROW, &s, a, "the kit's write")?;
    f.advance(1_000);
    let mut clips = cut_clips();
    clips[4].name = "App".into();
    app_save(&f.path(), &file(&cut_tree(&clips)));
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let DiskCheck::Conflict { copy, .. } = &out else {
        return Err(format!("{ROW}: not kept both: {out:?}"));
    };
    let c = read_content(ROW, &s, a)?;
    need(
        field(&c, ITEM_PROPS, "i1", "name").as_deref() == Some("Shot A")
            && field(&c, ITEM_PROPS, "i6", "name").as_deref() == Some("App"),
        ROW,
        || "the save is the live doc".into(),
    )?;
    let copy = copy
        .clone()
        .ok_or_else(|| format!("{ROW}: no conflict copy"))?;
    let kept = std::fs::read(&copy).map_err(|e| format!("{ROW}: {e}"))?;
    need(contains(&kept, "\"name\": \"Kit\""), ROW, || {
        "the copy holds the kit's version".into()
    })
}

/// An app's save while the doc holds changes the file doesn't keeps both too; the save becomes
/// the live doc exactly.
pub fn row_save_on_unwritten_changes_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_save_on_unwritten_changes_keeps_both";
    let (f, s) = open("unwritten", &cut());
    let a = f.agent(&s);
    verb(ROW, &s, a, &["clip", "move", "@item:5", "--to", "+4f"])?;
    f.advance(10);
    let mut clips = cut_clips();
    clips[4].name = "App".into();
    let save = file(&cut_tree(&clips));
    app_save(&f.path(), &save);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Conflict { .. }), ROW, || {
        format!("not kept both: {out:?}")
    })?;
    let c = read_content(ROW, &s, a)?;
    need(
        field(&c, ITEM_PROPS, "i5", "geom") == Some(geom(144, 24))
            && field(&c, ITEM_PROPS, "i6", "name").as_deref() == Some("App"),
        ROW,
        || "the save is the live doc exactly".into(),
    )?;
    need(f.bytes() == save, ROW, || {
        "the file is the save, unwritten over".into()
    })
}

// ---------- the write-back through the kit ----------

/// A restart, then a verb: the write-back splices into the carrier the store kept.
pub fn row_restart_then_verb_writes_exactly() -> Result<(), String> {
    const ROW: &str = "row_restart_then_verb_writes_exactly";
    let mut f = fx("restart", |_| {});
    f.write(&cut());
    {
        let s = f.open();
        let a = f.agent(&s);
        verb(ROW, &s, a, &["clip", "move", "@item:5", "--to", "+6f"])?;
        flush(ROW, &s, a, "before the restart")?;
    }
    f.mgr
        .close(&super::key(), "restart")
        .map_err(|e| format!("{ROW}: {e}"))?;
    f.restart();
    let s = f.open();
    let a = f.agent(&s);
    verb(ROW, &s, a, &["clip", "rename", "@item:4", "C2"])?;
    flush(ROW, &s, a, "after the restart")?;
    let got = placed(&f.bytes()).map_err(|e| format!("{ROW}: {e}"))?;
    let v1: Vec<(&str, i64)> = got[0].iter().map(|i| (i.0.as_str(), i.1 / F)).collect();
    need(
        v1 == vec![
            ("Shot A", 24),
            ("Shot B", 72),
            ("dissolve", 90),
            ("C2", 96),
            ("Shot A again", 150),
        ],
        ROW,
        || format!("V1 reads back as {v1:?}"),
    )
}

/// The differential oracle: a run of live verbs, written back, equals the same edits worked
/// out on the tier-0 reader's model of the original file.
pub fn row_live_verbs_equal_model_edits() -> Result<(), String> {
    const ROW: &str = "row_live_verbs_equal_model_edits";
    let (f, s) = open("oracle", &cut());
    let a = f.agent(&s);
    let steps: &[&[&str]] = &[
        &["clip", "move", "@item:5", "--to", "+12f"],
        &["clip", "trim", "@item:4", "--out", "-6f"],
        &["clip", "rename", "@item:2", "B prime"],
        &["clip", "enable", "@item:1", "--off"],
        &[
            "clip",
            "add",
            "--media",
            "@med:3",
            "--track",
            "@trk:3",
            "--at",
            "01:00:02:00",
            "--in",
            "20f",
            "--dur",
            "10f",
            "--name",
            "C on V2",
        ],
        &["clip", "trim", "@item:6", "--in", "+12f"],
        &[
            "clip", "move", "@item:5", "--to", "96f", "--track", "@trk:3",
        ],
    ];
    for st in steps {
        verb(ROW, &s, a, st)?;
    }
    flush(ROW, &s, a, "the run")?;
    let got = placed(&f.bytes()).map_err(|e| format!("{ROW}: {e}"))?;
    // the model's edits, worked out from the original
    let mut want = placed(&cut()).map_err(|e| format!("{ROW}: {e}"))?;
    {
        let v1 = &mut want[0];
        let d = v1.remove(4); // Shot A again leaves V1
        v1[3].2 -= 6 * F; // Shot C's out
        v1[1].0 = "B prime".into();
        v1[0].4 = false;
        let v2 = &mut want[2];
        v2.push(("C on V2".into(), 48 * F, 10 * F, 20 * F, true));
        v2.push((d.0, 96 * F, d.2, d.3, d.4));
        v2.sort_by_key(|i| i.1);
        let m = &mut want[1][0];
        m.1 += 12 * F;
        m.2 -= 12 * F;
        m.3 += 12 * F;
    }
    need(got == want, ROW, || {
        format!("the live run wrote {got:?}\nthe model says {want:?}")
    })
}

/// `clip delete` lifts the clip with its markers and its media when nothing else uses it; the
/// write-back removes them from the file.
pub fn row_delete_lifts_with_markers() -> Result<(), String> {
    const ROW: &str = "row_delete_lifts_with_markers";
    let (f, s) = open("delete", &cut());
    let a = f.agent(&s);
    verb(ROW, &s, a, &["clip", "delete", "@item:6"])?;
    verb(
        ROW,
        &s,
        a,
        &[
            "marker",
            "add",
            "--on",
            "@item:4",
            "--at",
            "01:00:04:12",
            "--name",
            "cue",
        ],
    )?;
    verb(ROW, &s, a, &["marker", "set", "@mk:1", "--note", "moved"])?;
    flush(ROW, &s, a, "the delete")?;
    let bytes = f.bytes();
    let c = TimelineLive::default()
        .decode(&bytes)
        .map_err(|e| format!("{ROW}: {e}"))?
        .roots;
    need(
        list(&c, MEDIA).len() == 3 && !contains(&bytes, "music.wav"),
        ROW,
        || "the music's media is still in the file".into(),
    )?;
    need(
        contains(&bytes, "\"comment\": \"moved\"") && contains(&bytes, "\"name\": \"cue\""),
        ROW,
        || "the markers weren't written".into(),
    )
}

/// A time that isn't on the clip's own rate grid is refused at the verb (`tl_rate`), nothing
/// applied: a 29.97 frame on a 4096 fps clip, moved or trimmed. A 29.97 clip trimmed by a 29.97
/// frame is taken.
pub fn row_rate_grid_refused() -> Result<(), String> {
    const ROW: &str = "row_rate_grid_refused";
    let (f, s) = open("rate-grid", &time_of_day());
    let a = f.agent(&s);
    refused(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:2", "--to", "+1f"],
        "tl_rate",
    )?;
    refused(
        ROW,
        &s,
        a,
        &["clip", "trim", "@item:2", "--in", "+1f"],
        "tl_rate",
    )?;
    verb(ROW, &s, a, &["clip", "trim", "@item:1", "--out", "-1f"])?;
    flush(ROW, &s, a, "the 29.97 trim")
}

/// A hand edit of the store that changes a clip's source in and start (a head trim) but not
/// its derived identity writes back: the identity is worked out when a save is paired, never
/// read from a field that can go stale.
pub fn row_hand_edit_writes_back() -> Result<(), String> {
    const ROW: &str = "row_hand_edit_writes_back";
    let (f, s) = open("hand-edit", &cut());
    let p = f.person(&s);
    s.edit(
        p,
        vec![Edit::Map {
            root: ITEM_PROPS.into(),
            ops: vec![
                MapOp::Set {
                    key: "i5\x1fgeom".into(),
                    value: Value::str(geom(146, 22)),
                },
                MapOp::Set {
                    key: "i5\x1fsrc".into(),
                    value: Value::str((202 * F).to_string()),
                },
            ],
        }],
        None,
    )
    .map_err(|e| format!("{ROW}: the hand edit: {e}"))?;
    flush(ROW, &s, p, "the hand edit")?;
    let got = placed(&f.bytes()).map_err(|e| format!("{ROW}: {e}"))?;
    let d = got[0].iter().find(|i| i.0 == "Shot A again").cloned();
    need(
        d.as_ref().map(|i| (i.1, i.2, i.3)) == Some((146 * F, 22 * F, 202 * F)),
        ROW,
        || format!("the file holds {d:?}"),
    )
}

/// At the pack's hooks: after a hand edit of a clip's source in (no stamp in the save, no
/// identity field touched), an app's save holding that clip at that source in pairs it by the
/// identity worked out from the live fields.
pub fn row_save_pairs_by_live_identity() -> Result<(), String> {
    const ROW: &str = "row_save_pairs_by_live_identity";
    let mut synced = decoded(ROW, &cut())?;
    set_field(&mut synced, ITEM_PROPS, "i5", "src", &(202 * F).to_string());
    let mut clips = cut_clips();
    clips[3].src = "202.0".into();
    let save = file(&cut_tree(&clips));
    let new = model::read(&save, &LiveLimits::default()).map_err(|e| format!("{ROW}: {e}"))?;
    let (keys, _) = pair_save(&View::of_content(&synced), &new);
    let got = new
        .items
        .iter()
        .position(|i| i.name == "Shot A again")
        .map(|i| keys.items[i].as_str().to_owned());
    need(got.as_deref() == Some("i5"), ROW, || {
        format!("the clip pairs as {got:?}, not i5")
    })
}

/// The cut with Shot A again cut from the same frames as Shot A: two clips of one kind, one
/// media and one source in, which pairing by identity can't tell apart. With `swap`, an app
/// has put the two in each other's places.
fn duplicates(swap: bool) -> Vec<u8> {
    let mut clips = cut_clips();
    clips[3].src = "100.0".into();
    let J::Obj(mut kv) = cut_tree(&clips) else {
        unreachable!()
    };
    if swap {
        if let Some((_, J::Obj(stack))) = kv.iter_mut().find(|(k, _)| *k == "tracks") {
            if let Some((_, J::Arr(tracks))) = stack.iter_mut().find(|(k, _)| *k == "children") {
                if let J::Obj(t) = &mut tracks[0] {
                    if let Some((_, J::Arr(ch))) = t.iter_mut().find(|(k, _)| *k == "children") {
                        ch.swap(1, 6);
                    }
                }
            }
        }
    }
    file(&J::Obj(kv))
}

/// At the pack's hooks: a save holding two clips of one identity in each other's places, with
/// no stamp: neither pairs (a fresh key each), so no key moves to the other clip.
pub fn row_duplicate_identity_never_swaps() -> Result<(), String> {
    const ROW: &str = "row_duplicate_identity_never_swaps";
    let synced = decoded(ROW, &duplicates(false))?;
    let has_name = |k: &str, n: &str| field(&synced, ITEM_PROPS, k, "name").as_deref() == Some(n);
    need(
        has_name("i1", "Shot A") && has_name("i5", "Shot A again"),
        ROW,
        || "control: the duplicates are i1 and i5".into(),
    )?;
    let new = model::read(&duplicates(true), &LiveLimits::default())
        .map_err(|e| format!("{ROW}: {e}"))?;
    let (keys, _) = pair_save(&View::of_content(&synced), &new);
    let key_of = |name: &str| {
        new.items
            .iter()
            .position(|i| i.name == name)
            .map(|i| keys.items[i].as_str().to_owned())
    };
    let (a, d) = (key_of("Shot A"), key_of("Shot A again"));
    need(
        a.as_deref() != Some("i5") && d.as_deref() != Some("i1"),
        ROW,
        || format!("the two clips' keys swapped: Shot A {a:?}, Shot A again {d:?}"),
    )?;
    let fresh = |k: &Option<String>| k.as_deref().is_some_and(|k| k != "i1" && k != "i5");
    need(fresh(&a) && fresh(&d), ROW, || {
        format!("an ambiguous identity paired: Shot A {a:?}, Shot A again {d:?}")
    })
}

/// Through the kit: one of two clips of one identity is moved past the other and written back
/// (its stamp goes with it). An app's save that keeps our stamps pairs the moved one by its
/// stamp and the other by its identity, unique among the rest; a later save with the stamps
/// stripped pairs neither (fresh keys). Never a swap.
pub fn row_moved_duplicate_never_swaps() -> Result<(), String> {
    const ROW: &str = "row_moved_duplicate_never_swaps";
    let (f, s) = open("dup-move", &duplicates(false));
    let a = f.agent(&s);
    verb(ROW, &s, a, &["clip", "move", "@item:5", "--to", "0f"])?;
    flush(ROW, &s, a, "the move")?;
    let doc = tl_field(&read_content(ROW, &s, a)?, "doc").unwrap_or_default();
    let stamp = format!("\"mpc\": {{\"key\": \"i5\", \"doc\": \"{doc}\"}}");
    let written = String::from_utf8_lossy(&f.bytes()).into_owned();
    need(written.contains(&stamp), ROW, || {
        "control: the moved clip carries its stamp".into()
    })?;
    // an app's save, our stamp kept
    f.advance(1_000);
    let save = written.replace("\"name\": \"Music\"", "\"name\": \"App\"");
    app_save(&f.path(), save.as_bytes());
    s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let c = read_content(ROW, &s, a)?;
    let keyed = (
        named(&c, ITEMS, ITEM_PROPS, "Shot A"),
        named(&c, ITEMS, ITEM_PROPS, "Shot A again"),
    );
    need(keyed == (Some("i1".into()), Some("i5".into())), ROW, || {
        format!("with stamps: Shot A, Shot A again are {keyed:?}, not i1, i5")
    })?;
    // the next save, our stamps stripped
    f.advance(1_000);
    let save = save
        .replace(&stamp, "")
        .replace("\"name\": \"App\"", "\"name\": \"App2\"");
    app_save(&f.path(), save.as_bytes());
    s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let c = read_content(ROW, &s, a)?;
    let (x, y) = (
        named(&c, ITEMS, ITEM_PROPS, "Shot A"),
        named(&c, ITEMS, ITEM_PROPS, "Shot A again"),
    );
    need(
        x.as_deref() != Some("i5") && y.as_deref() != Some("i1"),
        ROW,
        || format!("without stamps the two clips' keys swapped: Shot A {x:?}, Shot A again {y:?}"),
    )?;
    let fresh = |k: &Option<String>| k.as_deref().is_some_and(|k| k != "i1" && k != "i5");
    need(fresh(&x) && fresh(&y), ROW, || {
        format!("without stamps an ambiguous identity paired: Shot A {x:?}, Shot A again {y:?}")
    })
}

/// The read-back refuses a write that doesn't hold the live content: a clip's `rate`, which
/// nothing writes, set in the store; the splice writes nothing for it, the file reads back with
/// the old rate, and the write-back is refused before anything reaches the file.
pub fn row_read_back_refuses_an_unwritten_field() -> Result<(), String> {
    const ROW: &str = "row_read_back_refuses_an_unwritten_field";
    let carrier = cut();
    let mut c = decoded(ROW, &carrier)?;
    set_field(&mut c, ITEM_PROPS, "i1", "rate", "25/1");
    match splice_into(ROW, &carrier, &c) {
        Err(e) if e.contains("doesn't read back") => Ok(()),
        Err(e) => Err(format!("{ROW}: refused, but not by the read-back: {e}")),
        Ok(_) => Err(format!(
            "{ROW}: a write that doesn't hold the live content was made"
        )),
    }
}

/// A move, then a read: `clip list` gives the track in its new time order, from the index the
/// kit kept through the move (a clip moved from the end into the leading gap lists first).
pub fn row_move_then_list_sees_new_order() -> Result<(), String> {
    const ROW: &str = "row_move_then_list_sees_new_order";
    let (f, s) = open("move-list", &cut());
    let a = f.agent(&s);
    let before = run(&s, a, &["clip", "list", "@trk:1"]).map_err(|e| format!("{ROW}: {e}"))?;
    need(stdout(&before).starts_with("@item:1\t"), ROW, || {
        format!(
            "control: the track starts with @item:1: {}",
            stdout(&before)
        )
    })?;
    verb(ROW, &s, a, &["clip", "move", "@item:5", "--to", "0f"])?;
    let o = run(&s, a, &["clip", "list", "@trk:1"]).map_err(|e| format!("{ROW}: {e}"))?;
    let keys: Vec<String> = stdout(&o)
        .lines()
        .filter_map(|l| l.split('\t').next())
        .map(str::to_owned)
        .collect();
    need(
        keys == ["@item:5", "@item:1", "@item:2", "@item:3", "@item:4"],
        ROW,
        || format!("the track after the move: {keys:?}"),
    )
}

/// `timeline info`'s length in ticks, and the latest end of any item scanned from the live
/// store: they must be equal.
fn info_against_scan(row: &str, s: &Session, c: ConnId, when: &str) -> Result<(), String> {
    let o = run(s, c, &["timeline", "info"]).map_err(|e| format!("{row}: {e}"))?;
    let out = stdout(&o);
    let info: Option<i64> = out
        .lines()
        .find_map(|l| l.strip_prefix("length ticks\t"))
        .and_then(|v| v.parse().ok());
    let content = read_content(row, s, c)?;
    let scan = list(&content, ITEMS)
        .iter()
        .filter_map(|(k, _)| field(&content, ITEM_PROPS, k, "geom"))
        .filter_map(|g| ticks::parse_pair(&g))
        .map(|(a, d)| a + d)
        .max()
        .unwrap_or(0);
    need(info == Some(scan), row, || {
        format!("{when}: timeline info says {info:?}, the scan {scan}")
    })
}

/// `timeline info`'s length, read from the kept index, equals a scan of every item: on a file
/// as it opens (`tag` names it in failures).
pub fn info_equals_scan_on(tag: &str, bytes: &[u8]) -> Result<(), String> {
    const ROW: &str = "row_info_length_equals_the_scan";
    let (f, s) = open("info", bytes);
    let a = f.agent(&s);
    info_against_scan(ROW, &s, a, tag)
}

/// `timeline info`'s length, read from the kept index, equals a scan of every item: on each
/// generated file as it opens, and on the cut after each edit that moves its end (the last clip
/// moved later, trimmed, deleted; a clip added past everything; an outside save).
pub fn row_info_length_equals_the_scan() -> Result<(), String> {
    const ROW: &str = "row_info_length_equals_the_scan";
    for (name, bytes) in generated_corpus() {
        info_equals_scan_on(&name, &bytes)?;
    }
    let (f, s) = open("info-edits", &cut());
    let a = f.agent(&s);
    let steps: &[&[&str]] = &[
        &["clip", "move", "@item:5", "--to", "+30f"],
        &["clip", "trim", "@item:5", "--out", "-4f"],
        &[
            "clip", "add", "--media", "@med:2", "--track", "@trk:3", "--at", "400f", "--in", "0f",
            "--dur", "12f",
        ],
        &["clip", "delete", "@item:7"],
        &["clip", "delete", "@item:5"],
    ];
    for st in steps {
        verb(ROW, &s, a, st)?;
        info_against_scan(ROW, &s, a, &format!("after {}", st.join(" ")))?;
    }
    f.advance(1_000);
    let mut clips = cut_clips();
    clips[4].dur = "192000.0".into();
    app_save(&f.path(), &file(&cut_tree(&clips)));
    s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    info_against_scan(ROW, &s, a, "after an outside save")
}

// ---------- what a player reads ----------

/// The keyed watch's snapshot as `c` gets it, each frame as the wire carries it.
fn snapshot_frames(row: &str, s: &Session, c: ConnId) -> Result<Vec<Json>, String> {
    let mut w = s
        .watch_keyed(c, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("{row}: {e}"))?
        .ok_or_else(|| format!("{row}: no keyed watch"))?;
    Ok(s.watch_snapshot(&mut w)
        .map_err(|e| format!("{row}: the snapshot: {e}"))?
        .iter()
        .map(WatchFrame::to_value)
        .collect())
}

/// The keyed watch's snapshot as `c` gets it, its frames as the wire carries them.
fn snapshot_wire(row: &str, s: &Session, c: ConnId) -> Result<String, String> {
    Ok(snapshot_frames(row, s, c)?
        .iter()
        .map(Json::to_json)
        .collect())
}

/// Whether the key `k` of the root `r` is one of the fields `new` (`(root, field)`: a field of
/// a composite map's elements, or a key of the `timeline` map).
fn is_new_field(new: &[(&str, &str)], r: &str, k: &str) -> bool {
    new.iter().any(|(root, fld)| {
        r == *root && (k == *fld || k.split_once('\x1f').is_some_and(|(_, x)| x == *fld))
    })
}

/// What a client from before the fields `new` reads: the content with them taken out builds
/// the same view index, so a client that doesn't know them reads the doc as it did. The content
/// must hold them (a control).
fn older_client_reads_the_same(
    row: &str,
    content: &[(String, Neutral)],
    new: &[(&str, &str)],
) -> Result<(), String> {
    let mut taken = 0;
    let old: Vec<(String, Neutral)> = content
        .iter()
        .map(|(r, n)| match n {
            Neutral::Map(m) => {
                let kept: std::collections::BTreeMap<String, Value> = m
                    .iter()
                    .filter(|(k, _)| !is_new_field(new, r, k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                taken += m.len() - kept.len();
                (r.clone(), Neutral::Map(kept))
            }
            other => (r.clone(), other.clone()),
        })
        .collect();
    need(taken > 0, row, || {
        format!("control: the store holds none of {new:?}")
    })?;
    let pack = TimelineLive::default();
    let index = |c: &[(String, Neutral)]| {
        k2_mpc_base::view_index::ViewIndex::live(&pack, &View::of_content(c).into_keyed(), 1)
    };
    let (now, before) = (index(content), index(&old));
    need(now.is_some() && now == before, row, || {
        format!("without {new:?} the view index differs: {before:?}")
    })
}

/// Tracks are the file's stacking order, as OTIO means it: a later track composites over an
/// earlier one. The `timeline` map says so (`stacking` = `last_on_top`), a watcher's snapshot
/// carries it, and the view's containers are in that order. A client from before it reads the
/// doc as it did.
pub fn row_tracks_stack_last_on_top() -> Result<(), String> {
    const ROW: &str = "row_tracks_stack_last_on_top";
    let (f, s) = open("stacking", &cut());
    let p = f.person(&s);
    let c = read_content(ROW, &s, p)?;
    let labels: Vec<String> = list(&c, TRACKS)
        .iter()
        .map(|(k, _)| field(&c, TRACK_PROPS, k, "nid").unwrap_or_default())
        .collect();
    need(labels == ["V1", "A1", "V2"], ROW, || {
        format!("the tracks in order: {labels:?}")
    })?;
    let v = s
        .view_index(p)
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no view index"))?;
    let lanes: Vec<&str> = v.containers.iter().map(|c| c.key.as_str()).collect();
    need(lanes == ["@trk:1", "@trk:2", "@trk:3"], ROW, || {
        format!("the view's lanes in order: {lanes:?}")
    })?;
    let stacking = tl_field(&c, "stacking");
    need(stacking.as_deref() == Some("last_on_top"), ROW, || {
        format!("the timeline's stacking is {stacking:?}")
    })?;
    let wire = snapshot_wire(ROW, &s, p)?;
    need(
        wire.contains("\"stacking\"") && wire.contains("\"last_on_top\""),
        ROW,
        || "the snapshot doesn't carry the stacking".into(),
    )?;
    older_client_reads_the_same(ROW, &c, &[(TIMELINE, "stacking")])
}

/// `j` (an object) with its entry `k` set to `v` (added when it has none).
fn with_entry(j: J, k: &'static str, v: J) -> J {
    match j {
        J::Obj(mut kv) => {
            match kv.iter_mut().find(|(x, _)| *x == k) {
                Some((_, old)) => *old = v,
                None => kv.push((k, v)),
            }
            J::Obj(kv)
        }
        other => other,
    }
}

/// A timeline of three tracks, one clip each: `V1` enabled, `A1` disabled (a muted audio track)
/// and `V2` disabled (a hidden video track).
pub fn disabled_tracks() -> Vec<u8> {
    let c = cut_clips();
    let off = |t: J| with_entry(t, "enabled", J::Raw("false".into()));
    file(&timeline(
        "Off",
        Some(("86400.0", "24.0")),
        vec![
            track("V1", "Video", vec![clip(&c[0])], vec![]),
            off(track("A1", "Audio", vec![clip(&c[4])], vec![])),
            off(track("V2", "Video", vec![clip(&c[1])], vec![])),
        ],
        vec![],
    ))
}

/// A track's `enabled` (`0` on a hidden video track) and `muted` (`1` on a muted audio track:
/// any disabled track, as before) are read from the file into the store, read-only: the pack
/// reports them and hides no lane (how a lane looks is the host's call), a watcher's snapshot
/// carries both, no verb writes them,
/// and a write-back refuses a doc whose `enabled` was changed. A client from before `enabled`
/// reads the doc as it did.
pub fn row_track_enabled_and_muted_read_only() -> Result<(), String> {
    const ROW: &str = "row_track_enabled_and_muted_read_only";
    let bytes = disabled_tracks();
    let (f, s) = open("enabled", &bytes);
    let p = f.person(&s);
    let c = read_content(ROW, &s, p)?;
    for (t, enabled, muted) in [("t1", "1", "0"), ("t2", "0", "1"), ("t3", "0", "1")] {
        let got = (
            field(&c, TRACK_PROPS, t, "enabled"),
            field(&c, TRACK_PROPS, t, "muted"),
        );
        need(
            got.0.as_deref() == Some(enabled) && got.1.as_deref() == Some(muted),
            ROW,
            || format!("{t}: enabled and muted {got:?}, not {enabled} and {muted}"),
        )?;
    }
    let v = s
        .view_index(p)
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no view index"))?;
    let hidden: Vec<(&str, bool)> = v
        .containers
        .iter()
        .map(|c| (c.key.as_str(), c.hidden))
        .collect();
    need(
        hidden == [("@trk:1", false), ("@trk:2", false), ("@trk:3", false)],
        ROW,
        || format!("the view's lanes, hidden: {hidden:?}"),
    )?;
    let wire = snapshot_wire(ROW, &s, p)?;
    need(
        wire.contains(r#"["@trk:3","enabled","0"]"#) && wire.contains(r#"["@trk:2","muted","1"]"#),
        ROW,
        || "the snapshot doesn't carry the tracks' enabled and muted".into(),
    )?;
    // no verb writes them
    let a = f.agent(&s);
    let verbs: &[&[&str]] = &[
        &["track", "enable", "@trk:3"],
        &["track", "mute", "@trk:2"],
        &["track", "set", "@trk:3", "--enabled", "1"],
    ];
    for verb in verbs {
        let o = run_based(&s, a, verb);
        need(o.as_ref().map_or(true, |o| o.exit != 0), ROW, || {
            format!("{verb:?} ran")
        })?;
    }
    // a hand edit of `enabled` doesn't write back
    let mut edited = decoded(ROW, &bytes)?;
    set_field(&mut edited, TRACK_PROPS, "t3", "enabled", "1");
    match splice_into(ROW, &bytes, &edited) {
        Err(e) if e.contains("writes only names") => {}
        Err(e) => return Err(format!("{ROW}: refused, but not as read-only: {e}")),
        Ok(_) => return Err(format!("{ROW}: a track's enabled was written to the file")),
    }
    // on the cut, whose tracks are all enabled
    let (g, t) = open("enabled-cut", &cut());
    let q = g.person(&t);
    let on_cut = read_content(ROW, &t, q)?;
    older_client_reads_the_same(ROW, &on_cut, &[(TRACK_PROPS, "enabled")])
}

/// The cut with a duration stored on `V1` (its own `source_range`, OTIO's trim of a track), seven
/// frames: shorter than its items, so it can't be the timeline's end.
pub fn stated_duration() -> Vec<u8> {
    let c = cut_clips();
    let J::Obj(mut kv) = cut_tree(&c) else {
        unreachable!("a timeline is an object")
    };
    if let Some((_, J::Obj(stack))) = kv.iter_mut().find(|(k, _)| *k == "tracks") {
        if let Some((_, J::Arr(ch))) = stack.iter_mut().find(|(k, _)| *k == "children") {
            ch[0] = with_entry(
                ch[0].clone(),
                "source_range",
                super::tr("0.0", "7.0", "24.0"),
            );
        }
    }
    file(&J::Obj(kv))
}

/// The timeline's end is the latest end (record start + duration) of any item on any track, and
/// `timeline info` prints it; a duration the file stores (a track's own range) is carried in the
/// file, never used for it, and the `timeline` map says which the end is (`end_from` =
/// `items`), which a watcher's snapshot carries. A client from before it reads the doc as it
/// did.
pub fn row_end_is_the_latest_item_end() -> Result<(), String> {
    const ROW: &str = "row_end_is_the_latest_item_end";
    let bytes = stated_duration();
    need(contains(&bytes, "\"value\": 7.0"), ROW, || {
        "control: V1 states a duration".into()
    })?;
    let (f, s) = open("end", &bytes);
    let p = f.person(&s);
    info_against_scan(ROW, &s, p, "with a stated duration")?;
    let o = run(&s, p, &["timeline", "info"]).map_err(|e| format!("{ROW}: {e}"))?;
    let out = stdout(&o);
    need(
        out.contains(&format!("length ticks\t{}\n", 168 * F)),
        ROW,
        || format!("timeline info: {out}"),
    )?;
    let c = read_content(ROW, &s, p)?;
    let from = tl_field(&c, "end_from");
    need(from.as_deref() == Some("items"), ROW, || {
        format!("the timeline's end_from is {from:?}")
    })?;
    need(out.contains("end from\titems\n"), ROW, || {
        format!("timeline info doesn't say where its end is from: {out}")
    })?;
    let wire = snapshot_wire(ROW, &s, p)?;
    need(wire.contains(r#"["end_from",null,"items"]"#), ROW, || {
        "the snapshot doesn't carry end_from".into()
    })?;
    // the stated duration stays in the file through an edit
    let a = f.agent(&s);
    verb(ROW, &s, a, &["clip", "move", "@item:6", "--to", "+2f"])?;
    flush(ROW, &s, a, "the move")?;
    need(contains(&f.bytes(), "\"value\": 7.0"), ROW, || {
        "the stated duration is gone from the file".into()
    })?;
    older_client_reads_the_same(ROW, &c, &[(TIMELINE, "end_from")])
}

/// Three clips on their media: a 24 fps source whose timecode starts at 01:00:00:00 (its
/// available range from frame 86,400), a 29.97 source on a 24-hour clock (its range from
/// 23:59:58:00, frame 2,591,940, past 2^53 ticks), and a source with no available range.
pub fn media_origins() -> Vec<u8> {
    let mut hour = ClipSpec::new("Hour", "file:///media/hour.mov", "86520", "48", "24");
    hour.avail = Some(("86400".into(), "1000".into()));
    let mut clock = ClipSpec::new("Clock", "file:///media/clock.mov", "2591950", "60", "29.97");
    clock.avail = Some(("2591940".into(), "600".into()));
    let mut bare = ClipSpec::new("Bare", "file:///media/bare.mov", "12", "24", "24");
    bare.avail = None;
    file(&timeline(
        "Origins",
        None,
        vec![
            track("V1", "Video", vec![clip(&hour), clip(&bare)], vec![]),
            track("V2", "Video", vec![clip(&clock)], vec![]),
        ],
        vec![],
    ))
}

/// Each media carries `start`, its available range's start in ticks (a decimal string, like
/// every tick field), so a clip's position in its media is `src - start`, exactly: an hour's
/// timecode and a 24-hour clock map to the clip's frames in whole ticks. A media with no
/// available range carries no `start` and is marked `origin` = `unknown` (the others `range`).
/// A watcher's snapshot carries both. A client from before `origin` reads the doc as it did.
pub fn row_media_start_maps_source_exactly() -> Result<(), String> {
    const ROW: &str = "row_media_start_maps_source_exactly";
    let (f, s) = open("origins", &media_origins());
    let p = f.person(&s);
    let c = read_content(ROW, &s, p)?;
    let frame_24 = F;
    let frame_ntsc: i64 = 8_475_667_200;
    // (item, its media, the clip's frames into its media, a frame)
    for (item, media, frames, frame) in [("i1", "m1", 120, frame_24), ("i3", "m3", 10, frame_ntsc)]
    {
        let src = field(&c, ITEM_PROPS, item, "src").and_then(|v| ticks::parse(&v));
        let start = field(&c, MEDIA_PROPS, media, "start").and_then(|v| ticks::parse(&v));
        let origin = field(&c, MEDIA_PROPS, media, "origin");
        need(
            field(&c, ITEM_PROPS, item, "media").as_deref() == Some(media),
            ROW,
            || format!("control: {item}'s media isn't {media}"),
        )?;
        let (Some(src), Some(start)) = (src, start) else {
            return Err(format!(
                "{ROW}: {item} src {src:?}, {media} start {start:?}"
            ));
        };
        need(
            src.checked_sub(start) == Some(frames * frame) && start % frame == 0,
            ROW,
            || format!("{item}: src {src} - start {start} isn't {frames} frames of {frame} ticks"),
        )?;
        need(origin.as_deref() == Some("range"), ROW, || {
            format!("{media}: origin {origin:?}")
        })?;
    }
    // the clock's start is past 2^53 ticks, exact
    let clock = field(&c, MEDIA_PROPS, "m3", "start");
    need(
        clock == Some((2_591_940 * frame_ntsc).to_string()),
        ROW,
        || format!("the clock's start {clock:?}"),
    )?;
    // no available range: no start, origin unknown
    let (start, origin) = (
        field(&c, MEDIA_PROPS, "m2", "start"),
        field(&c, MEDIA_PROPS, "m2", "origin"),
    );
    need(
        start.is_none() && origin.as_deref() == Some("unknown"),
        ROW,
        || format!("the bare media: start {start:?}, origin {origin:?}"),
    )?;
    let wire = snapshot_wire(ROW, &s, p)?;
    need(
        wire.contains(r#"["@med:2","origin","unknown"]"#)
            && wire.contains(&format!(
                r#"["@med:3","start","{}"]"#,
                2_591_940 * frame_ntsc
            )),
        ROW,
        || "the snapshot doesn't carry the media's start and origin".into(),
    )?;
    let (g, t) = open("origins-cut", &cut());
    let q = g.person(&t);
    let on_cut = read_content(ROW, &t, q)?;
    older_client_reads_the_same(ROW, &on_cut, &[(MEDIA_PROPS, "origin")])
}

/// An OTIO effect object of `schema`, named `name`.
fn effect(schema: &str, name: &str, more: Vec<(&'static str, J)>) -> J {
    let mut kv = vec![
        ("OTIO_SCHEMA", J::Str(schema.into())),
        ("metadata", J::Obj(vec![])),
        ("name", J::Str(String::new())),
        ("effect_name", J::Str(name.into())),
    ];
    kv.extend(more);
    J::Obj(kv)
}

/// A track of five items: a clip at twice its speed (a linear time warp), a held frame (a
/// freeze frame), a clip with an effect the store doesn't keep (a blur), a plain clip, and a
/// nest whose inside clip is retimed.
pub fn retimed_clips() -> Vec<u8> {
    let c = |name: &str| {
        ClipSpec::new(
            name,
            &format!("file:///media/{name}.mov"),
            "0.0",
            "24.0",
            "24.0",
        )
    };
    let with_fx = |j: J, e: J| with_entry(j, "effects", J::Arr(vec![e]));
    let fast = with_fx(
        clip(&c("fast")),
        effect(
            "LinearTimeWarp.1",
            "speed",
            vec![("time_scalar", J::Raw("2.0".into()))],
        ),
    );
    let held = with_fx(clip(&c("held")), effect("FreezeFrame.1", "hold", vec![]));
    let blur = with_fx(clip(&c("blur")), effect("Effect.1", "blur", vec![]));
    let inner = with_fx(
        clip(&c("inner")),
        effect(
            "LinearTimeWarp.1",
            "speed",
            vec![("time_scalar", J::Raw("0.5".into()))],
        ),
    );
    let nest = J::Obj(vec![
        ("OTIO_SCHEMA", J::Str("Stack.1".into())),
        ("metadata", J::Obj(vec![])),
        ("name", J::Str("nested".into())),
        ("source_range", J::Raw("null".into())),
        ("effects", J::Arr(vec![])),
        ("markers", J::Arr(vec![])),
        ("enabled", J::Raw("true".into())),
        (
            "children",
            J::Arr(vec![track("inside", "Video", vec![inner], vec![])]),
        ),
    ]);
    file(&timeline(
        "Speeds",
        Some(("86400.0", "24.0")),
        vec![track(
            "V1",
            "Video",
            vec![fast, held, blur, clip(&c("plain")), nest],
            vec![],
        )],
        vec![],
    ))
}

/// A clip's `retimed` (`1`/`0`), apart from `edit`: a clip with a speed change or a held frame
/// (a time warp or freeze frame effect) is retimed; a clip with an effect the store doesn't keep,
/// a plain clip, a nest whose inside clip is retimed, and a transition aren't. A watcher's
/// snapshot carries it. A client from before it reads the doc as it did.
pub fn row_retimed_is_its_own_field() -> Result<(), String> {
    const ROW: &str = "row_retimed_is_its_own_field";
    let (f, s) = open("retimed", &retimed_clips());
    let p = f.person(&s);
    let c = read_content(ROW, &s, p)?;
    let want = [
        ("i1", "fast", "1", "0"),
        ("i2", "held", "1", "0"),
        ("i3", "blur", "0", "1"),
        ("i4", "plain", "0", "1"),
        ("i5", "nested", "0", "0"),
    ];
    for (item, name, retimed, edit) in want {
        let got = (
            field(&c, ITEM_PROPS, item, "name"),
            field(&c, ITEM_PROPS, item, "retimed"),
            field(&c, ITEM_PROPS, item, "edit"),
        );
        need(
            got.0.as_deref() == Some(name)
                && got.1.as_deref() == Some(retimed)
                && got.2.as_deref() == Some(edit),
            ROW,
            || format!("{item}: name, retimed, edit {got:?}, not {name} {retimed} {edit}"),
        )?;
    }
    let wire = snapshot_wire(ROW, &s, p)?;
    need(
        wire.contains(r#"["@item:1","retimed","1"]"#)
            && wire.contains(r#"["@item:5","retimed","0"]"#),
        ROW,
        || "the snapshot doesn't carry retimed".into(),
    )?;
    // on the cut: a transition isn't retimed
    let (g, t) = open("retimed-cut", &cut());
    let q = g.person(&t);
    let on_cut = read_content(ROW, &t, q)?;
    let tr = field(&on_cut, ITEM_PROPS, "i3", "retimed");
    need(tr.as_deref() == Some("0"), ROW, || {
        format!("the cut's dissolve: retimed {tr:?}")
    })?;
    older_client_reads_the_same(ROW, &on_cut, &[(ITEM_PROPS, "retimed")])
}

/// A muted audio lane is reported, never hidden: the view keeps every lane the file has, in
/// its order, none of them hidden, and the store flags the muted audio lane `muted` `1` and the
/// disabled video lane `enabled` `0`. The player passes over both; how they look is the host's
/// call.
pub fn row_muted_lane_stays_in_the_view() -> Result<(), String> {
    const ROW: &str = "row_muted_lane_stays_in_the_view";
    let (f, s) = open("muted-lane", &disabled_tracks());
    let p = f.person(&s);
    let v = s
        .view_index(p)
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no view index"))?;
    let lanes: Vec<(&str, Option<&str>, bool)> = v
        .containers
        .iter()
        .map(|c| (c.key.as_str(), c.title.as_deref(), c.hidden))
        .collect();
    need(
        lanes
            == [
                ("@trk:1", Some("V1"), false),
                ("@trk:2", Some("A1"), false),
                ("@trk:3", Some("V2"), false),
            ],
        ROW,
        || format!("the view's lanes (key, title, hidden): {lanes:?}"),
    )?;
    let c = read_content(ROW, &s, p)?;
    let (muted, enabled) = (
        field(&c, TRACK_PROPS, "t2", "muted"),
        field(&c, TRACK_PROPS, "t3", "enabled"),
    );
    need(
        muted.as_deref() == Some("1") && enabled.as_deref() == Some("0"),
        ROW,
        || format!("A1's muted {muted:?} (want 1), V2's enabled {enabled:?} (want 0)"),
    )
}

// ---------- the agent's playhead ----------

/// `pid`'s time position on the roster as `viewer` reads it.
fn time_of(
    row: &str,
    s: &Session,
    viewer: ConnId,
    pid: &str,
) -> Result<Option<TimePosition>, String> {
    let roster = s
        .roster(viewer)
        .map_err(|e| format!("{row}: the roster: {e}"))?;
    let e = roster
        .into_iter()
        .find(|e| e.participant.pid == pid)
        .ok_or_else(|| format!("{row}: no roster row for {pid}"))?;
    Ok(e.time)
}

/// The host clock now.
fn now(f: &Fx) -> u64 {
    f.host.now.load(std::sync::atomic::Ordering::SeqCst)
}

/// The agent `atlas`'s playhead, after `what`, is at `frames` (24 fps frames, in ticks), paused,
/// at no rate, in `within`, stamped now.
fn playhead_at(
    row: &str,
    f: &Fx,
    s: &Session,
    viewer: ConnId,
    what: &str,
    frames: i64,
    within: Option<&str>,
) -> Result<TimePosition, String> {
    let t = time_of(row, s, viewer, "atlas")?
        .ok_or_else(|| format!("{row}: {what}: the agent has no playhead"))?;
    let want = ticks::text(frames * F);
    need(
        (t.at(), t.state(), t.in_address(), t.rate_text(), t.at_ms())
            == (want.as_str(), PlayState::Paused, within, None, now(f)),
        row,
        || {
            format!(
                "{what}: the agent's playhead is {t:?}, not at {want} ({frames} frames), paused, \
                 in {within:?}, at {}",
                now(f)
            )
        },
    )?;
    Ok(t)
}

/// The agent's playhead follows its trims: `--in` leaves it at the clip's new record start,
/// `--out` at its new end, both at the new start; each in the clip's track, paused, stamped when
/// the verb ran.
pub fn row_agent_playhead_follows_a_trim() -> Result<(), String> {
    const ROW: &str = "row_agent_playhead_follows_a_trim";
    let (f, s) = open("playhead-trim", &cut());
    let p = f.person(&s);
    let a = f.agent(&s);
    let before = time_of(ROW, &s, p, "atlas")?;
    need(before.is_none(), ROW, || {
        format!("control: the agent has a playhead before any verb: {before:?}")
    })?;
    // Shot A is 24f..72f on V1
    f.advance(40);
    verb(ROW, &s, a, &["clip", "trim", "@item:1", "--in", "+6f"])?;
    playhead_at(ROW, &f, &s, p, "trim --in", 30, Some("@trk:1"))?;
    f.advance(40);
    verb(ROW, &s, a, &["clip", "trim", "@item:1", "--out", "-6f"])?;
    playhead_at(ROW, &f, &s, p, "trim --out", 66, Some("@trk:1"))?;
    f.advance(40);
    verb(
        ROW,
        &s,
        a,
        &["clip", "trim", "@item:1", "--in", "+2f", "--out", "+2f"],
    )?;
    playhead_at(ROW, &f, &s, p, "trim --in --out", 32, Some("@trk:1"))?;
    Ok(())
}

/// Every edit verb leaves the agent's playhead at its edit point, in the track it worked on: a
/// move (onto another track) at the new start, an add at its start, a lift where the clip
/// started, a marker add and set at the marker's record time, a rename and an enable at the
/// clip's start, a marker delete where the marker was; a timeline marker in the doc's own time
/// (no track). A track rename (no time), a refused verb and a missing item move nothing.
pub fn row_agent_playhead_at_each_edit_point() -> Result<(), String> {
    const ROW: &str = "row_agent_playhead_at_each_edit_point";
    let (f, s) = open("playhead-edits", &cut());
    let p = f.person(&s);
    let a = f.agent(&s);
    f.advance(40);
    verb(
        ROW,
        &s,
        a,
        &[
            "clip",
            "move",
            "@item:5",
            "--to",
            "01:00:10:00",
            "--track",
            "@trk:3",
        ],
    )?;
    playhead_at(ROW, &f, &s, p, "clip move", 240, Some("@trk:3"))?;
    f.advance(40);
    verb(
        ROW,
        &s,
        a,
        &[
            "clip",
            "add",
            "--media",
            "@med:3",
            "--track",
            "@trk:3",
            "--at",
            "01:00:02:00",
            "--in",
            "20f",
            "--dur",
            "10f",
            "--name",
            "C on V2",
        ],
    )?;
    playhead_at(ROW, &f, &s, p, "clip add", 48, Some("@trk:3"))?;
    f.advance(40);
    // the music, from zero on A1
    verb(ROW, &s, a, &["clip", "delete", "@item:6"])?;
    playhead_at(ROW, &f, &s, p, "clip delete", 0, Some("@trk:2"))?;
    f.advance(40);
    // Shot C is 96f..132f on V1
    let out = verb(
        ROW,
        &s,
        a,
        &[
            "marker",
            "add",
            "--on",
            "@item:4",
            "--at",
            "01:00:04:12",
            "--name",
            "cue",
        ],
    )?;
    playhead_at(ROW, &f, &s, p, "marker add", 108, Some("@trk:1"))?;
    let mk = out.split('\t').next().unwrap_or("").to_owned();
    need(mk.starts_with("@mk:"), ROW, || {
        format!("marker add printed no marker: {out:?}")
    })?;
    f.advance(40);
    verb(ROW, &s, a, &["marker", "set", &mk, "--at", "01:00:04:18"])?;
    playhead_at(ROW, &f, &s, p, "marker set", 114, Some("@trk:1"))?;
    f.advance(40);
    verb(ROW, &s, a, &["clip", "rename", "@item:1", "Shot A1"])?;
    playhead_at(ROW, &f, &s, p, "clip rename", 24, Some("@trk:1"))?;
    f.advance(40);
    verb(ROW, &s, a, &["marker", "delete", &mk])?;
    playhead_at(ROW, &f, &s, p, "marker delete", 114, Some("@trk:1"))?;
    f.advance(40);
    verb(ROW, &s, a, &["clip", "enable", "@item:4", "--off"])?;
    playhead_at(ROW, &f, &s, p, "clip enable", 96, Some("@trk:1"))?;
    f.advance(40);
    verb(
        ROW,
        &s,
        a,
        &[
            "marker",
            "add",
            "--on",
            "timeline",
            "--at",
            "01:00:05:00",
            "--name",
            "mid",
        ],
    )?;
    let kept = playhead_at(ROW, &f, &s, p, "marker add on the timeline", 120, None)?;
    // nothing below moves it, its stamp included
    f.advance(40);
    verb(ROW, &s, a, &["track", "rename", "@trk:2", "Music"])?;
    let t = time_of(ROW, &s, p, "atlas")?;
    need(t.as_ref() == Some(&kept), ROW, || {
        format!("track rename moved the agent's playhead: {t:?}")
    })?;
    f.advance(40);
    refused(
        ROW,
        &s,
        a,
        &["clip", "move", "@item:2", "--to", "+1f"],
        "tl_transition",
    )?;
    let t = time_of(ROW, &s, p, "atlas")?;
    need(t.as_ref() == Some(&kept), ROW, || {
        format!("a refused move moved the agent's playhead: {t:?}")
    })?;
    f.advance(40);
    let o = run_based(&s, a, &["clip", "rename", "@item:99", "x"])
        .map_err(|e| format!("{ROW}: {e}"))?;
    need(o.exit == 4, ROW, || {
        format!("a missing item: exit {}: {}", o.exit, stdout(&o))
    })?;
    let t = time_of(ROW, &s, p, "atlas")?;
    need(t.as_ref() == Some(&kept), ROW, || {
        format!("a verb on a missing item moved the agent's playhead: {t:?}")
    })
}

/// A person's playhead is the person's: an agent's verb doesn't move it, nor does the person's
/// own verb (and that verb leaves the agent's as it was).
pub fn row_person_playhead_never_moved_by_a_verb() -> Result<(), String> {
    const ROW: &str = "row_person_playhead_never_moved_by_a_verb";
    let (f, s) = open("playhead-person", &cut());
    let p = f.person(&s);
    let a = f.agent(&s);
    let mine = TimePosition::new(&ticks::text(500 * F), PlayState::Playing)
        .and_then(|t| t.within("@trk:2"))
        .map_err(|e| format!("{ROW}: {e}"))?;
    let set = s
        .set_time_position(p, Some(mine))
        .map_err(|e| format!("{ROW}: set_time_position: {e}"))?
        .ok_or_else(|| format!("{ROW}: set_time_position stored none"))?;
    f.advance(40);
    verb(ROW, &s, a, &["clip", "move", "@item:5", "--to", "+2f"])?;
    let t = time_of(ROW, &s, p, "ada")?;
    need(t.as_ref() == Some(&set), ROW, || {
        format!("an agent's verb moved the person's playhead: {t:?}, set {set:?}")
    })?;
    // control: the verb moved its own agent's
    let agents = playhead_at(ROW, &f, &s, p, "the agent's move", 146, Some("@trk:1"))?;
    f.advance(40);
    verb(ROW, &s, p, &["clip", "trim", "@item:1", "--in", "+6f"])?;
    let t = time_of(ROW, &s, p, "ada")?;
    need(t.as_ref() == Some(&set), ROW, || {
        format!("the person's own verb moved their playhead: {t:?}, set {set:?}")
    })?;
    let t = time_of(ROW, &s, p, "atlas")?;
    need(t.as_ref() == Some(&agents), ROW, || {
        format!("the person's verb moved the agent's playhead: {t:?}")
    })
}

/// The view schema says what a playhead counts in: `time_unit` `tick`, and a watcher's snapshot
/// carries it.
pub fn row_view_time_unit_is_tick() -> Result<(), String> {
    const ROW: &str = "row_view_time_unit_is_tick";
    let unit = TimelineLive::default()
        .view_schema()
        .and_then(|v| v.time_unit);
    need(unit == Some("tick"), ROW, || {
        format!("the view schema's time unit is {unit:?}, not tick")
    })?;
    let (f, s) = open("time-unit", &cut());
    let p = f.person(&s);
    let wire = snapshot_wire(ROW, &s, p)?;
    need(wire.contains("\"time_unit\":\"tick\""), ROW, || {
        "the watcher's snapshot carries no time_unit tick".into()
    })
}

/// An older reader ignores the new `time_unit`: the schema the pack serves, on the wire without
/// it, is the schema as it was before (the declared one with no time unit), key for key, in
/// order: the field is added, nothing else moves.
pub fn row_older_reader_ignores_time_unit() -> Result<(), String> {
    const ROW: &str = "row_older_reader_ignores_time_unit";
    let served = TimelineLive::default()
        .view_schema()
        .ok_or_else(|| format!("{ROW}: the pack serves no view schema"))?;
    let wire = served.to_value();
    need(wire.get("time_unit").is_some(), ROW, || {
        "control: the schema's wire value has no time_unit".into()
    })?;
    let without = match &wire {
        Json::Obj(kv) => Json::Obj(
            kv.iter()
                .filter(|(k, _)| k != "time_unit")
                .cloned()
                .collect(),
        ),
        other => return Err(format!("{ROW}: the schema isn't an object: {other:?}")),
    };
    let before = ViewSchema {
        time_unit: None,
        ..VIEW
    }
    .to_value();
    need(without == before, ROW, || {
        format!(
            "without time_unit the schema differs:\n{}\nbefore:\n{}",
            without.to_json(),
            before.to_json()
        )
    })
}

// ---------- each clip's timecode ----------

/// Each unit's start timecode on the cut (24 fps from 01:00:00:00), by address in the index's
/// order: worked out by hand from the record starts (frames 24, 72, 90, 96, 144 on `V1`, 0 on
/// `A1`).
const CUT_TC: &[(&str, &str)] = &[
    ("@item:1", "01:00:01:00"),
    ("@item:2", "01:00:03:00"),
    ("@item:3", "01:00:03:18"),
    ("@item:4", "01:00:04:00"),
    ("@item:5", "01:00:06:00"),
    ("@item:6", "01:00:00:00"),
];

/// Each unit's derived `tc` in `c`'s view index, by address in its order (`None`: none).
fn index_tc(row: &str, s: &Session, c: ConnId) -> Result<Vec<(String, Option<String>)>, String> {
    let v = s
        .view_index(c)
        .map_err(|e| format!("{row}: {e}"))?
        .ok_or_else(|| format!("{row}: no view index"))?;
    Ok(v.units
        .iter()
        .map(|u| (u.key.clone(), u.derived_value("tc").map(str::to_owned)))
        .collect())
}

/// `want` as [`index_tc`] gives it.
fn want_tc(want: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    want.iter()
        .map(|(u, t)| ((*u).to_owned(), Some((*t).to_owned())))
        .collect()
}

/// The units the snapshot's units pages carry, by address in their order, each with its
/// derived `tc` as the wire has it (`None`: no `derived.tc`).
fn snapshot_unit_tc(frames: &[Json]) -> Vec<(String, Option<String>)> {
    frames
        .iter()
        .filter(|fr| fr.get("type").and_then(Json::as_str) == Some("keyed_page"))
        .filter_map(|fr| match fr.get("units").and_then(|p| p.get("units")) {
            Some(Json::List(us)) => Some(us),
            _ => None,
        })
        .flatten()
        .map(|u| {
            (
                u.get("key").and_then(Json::as_str).unwrap_or("").to_owned(),
                u.get("derived")
                    .and_then(|d| d.get("tc"))
                    .and_then(Json::as_str)
                    .map(str::to_owned),
            )
        })
        .collect()
}

/// `derived.units.<unit>.tc` of a change frame as the wire carries it: `None` when the frame
/// has none for `unit`; `Some(Json::Null)` when it says the unit has none now.
fn frame_tc(frame: &Json, unit: &str) -> Option<Json> {
    frame
        .get("derived")
        .and_then(|d| d.get("units"))
        .and_then(|u| u.get(unit))
        .and_then(|u| u.get("tc"))
        .cloned()
}

/// The one change frame among `frames`, as the wire carries it.
fn one_change(row: &str, what: &str, frames: &[WatchFrame]) -> Result<Json, String> {
    let changes: Vec<Json> = frames
        .iter()
        .filter(|fr| matches!(fr, WatchFrame::Change(_)))
        .map(WatchFrame::to_value)
        .collect();
    match changes.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!(
            "{row}: {what}: {} change frames, want one",
            changes.len()
        )),
    }
}

/// Each clip (each unit of the view) carries its start's timecode as the derived value `tc`:
/// its record start at the timeline's rate, counted from the timeline's start timecode
/// (`01:00:01:00` for a clip one second in, at 24 fps from 01:00:00:00). The view index has it,
/// and so do the keyed watch's snapshot units pages (`derived.tc` on each unit). On a 29.97
/// timeline (`30000/1001`, from frame 107892) it counts that rate's frames, labelled at 30 a
/// second, non-drop as the file says nothing of drop-frame: `00:59:56:12` at the start,
/// `00:59:58:12` two seconds of frames later.
pub fn row_each_clip_carries_its_start_tc() -> Result<(), String> {
    const ROW: &str = "row_each_clip_carries_its_start_tc";
    let (f, s) = open("clip-tc", &cut());
    let p = f.person(&s);
    let got = index_tc(ROW, &s, p)?;
    need(got == want_tc(CUT_TC), ROW, || {
        format!("the view index's tc per unit: {got:?}, want {CUT_TC:?}")
    })?;
    let frames = snapshot_frames(ROW, &s, p)?;
    let paged = snapshot_unit_tc(&frames);
    need(paged == want_tc(CUT_TC), ROW, || {
        format!("the snapshot's units pages carry tc {paged:?}, want {CUT_TC:?}")
    })?;
    let (g, d) = open("clip-tc-ntsc", &decimal_rate());
    let q = g.person(&d);
    let ntsc: &[(&str, &str)] = &[("@item:1", "00:59:56:12"), ("@item:2", "00:59:58:12")];
    let got = index_tc(ROW, &d, q)?;
    need(got == want_tc(ntsc), ROW, || {
        format!("at 30000/1001 the view index's tc per unit: {got:?}, want {ntsc:?}")
    })
}

/// A change that moves a clip's start carries its new `tc` in the change frame
/// (`derived.units.<clip>.tc`), and the index the kit kept through it says the same: a trim of
/// Shot A's start by six frames (`01:00:01:06`), then a move of Shot A again twelve frames later
/// (`01:00:06:12`). The file's bytes aren't written between them, so a `tc` read from the file
/// rather than the live store would still be the old one.
pub fn row_edit_frame_carries_the_new_tc() -> Result<(), String> {
    const ROW: &str = "row_edit_frame_carries_the_new_tc";
    let (f, s) = open("tc-frame", &cut());
    let (p, a) = (f.person(&s), f.agent(&s));
    let mut w = s
        .watch_keyed(p, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no keyed watch"))?;
    s.watch_snapshot(&mut w)
        .map_err(|e| format!("{ROW}: the snapshot: {e}"))?;
    w.try_frames();
    let edits: &[(&[&str], &str, &str)] = &[
        (
            &["clip", "trim", "@item:1", "--in", "+6f"],
            "@item:1",
            "01:00:01:06",
        ),
        (
            &["clip", "move", "@item:5", "--to", "+12f"],
            "@item:5",
            "01:00:06:12",
        ),
    ];
    for (edit, unit, want) in edits {
        let what = edit.join(" ");
        verb(ROW, &s, a, edit)?;
        let frame = one_change(ROW, &what, &w.try_frames())?;
        let got = frame_tc(&frame, unit);
        need(
            got.as_ref().and_then(Json::as_str) == Some(*want),
            ROW,
            || {
                format!(
                    "{what}: the change frame's tc for {unit} is {}, want {want} (its derived: {})",
                    got.map_or("absent".into(), |g| g.to_json()),
                    frame.get("derived").map_or("absent".into(), Json::to_json)
                )
            },
        )?;
        let ix = index_tc(ROW, &s, p)?;
        let tc = ix
            .iter()
            .find(|(u, _)| u == unit)
            .and_then(|(_, t)| t.clone());
        need(tc.as_deref() == Some(*want), ROW, || {
            format!("{what}: the view index's tc for {unit} is {tc:?}, want {want}")
        })?;
    }
    Ok(())
}

/// A change of the timeline's start timecode (a person's hand edit of the `timeline` map's
/// `tc`, one second later) moves every clip's `tc`, though no clip changed: the change frame
/// carries each one's new `tc`, and the index the kit kept through it says the same.
pub fn row_start_tc_change_moves_every_tc() -> Result<(), String> {
    const ROW: &str = "row_start_tc_change_moves_every_tc";
    let (f, s) = open("start-tc", &cut());
    let p = f.person(&s);
    // the index read once before, so the kit keeps it through the edit
    index_tc(ROW, &s, p)?;
    let mut w = s
        .watch_keyed(p, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no keyed watch"))?;
    s.watch_snapshot(&mut w)
        .map_err(|e| format!("{ROW}: the snapshot: {e}"))?;
    w.try_frames();
    s.edit(
        p,
        vec![Edit::Map {
            root: TIMELINE.into(),
            ops: vec![MapOp::Set {
                key: "tc".into(),
                value: Value::str(ticks::text(86_424 * F)),
            }],
        }],
        None,
    )
    .map_err(|e| format!("{ROW}: the hand edit: {e}"))?;
    let later: &[(&str, &str)] = &[
        ("@item:1", "01:00:02:00"),
        ("@item:2", "01:00:04:00"),
        ("@item:3", "01:00:04:18"),
        ("@item:4", "01:00:05:00"),
        ("@item:5", "01:00:07:00"),
        ("@item:6", "01:00:01:00"),
    ];
    let frame = one_change(ROW, "the start's edit", &w.try_frames())?;
    for (unit, want) in later {
        let got = frame_tc(&frame, unit);
        need(
            got.as_ref().and_then(Json::as_str) == Some(*want),
            ROW,
            || {
                format!(
                    "the change frame's tc for {unit} is {}, want {want} (its derived: {})",
                    got.map_or("absent".into(), |g| g.to_json()),
                    frame.get("derived").map_or("absent".into(), Json::to_json)
                )
            },
        )?;
    }
    let got = index_tc(ROW, &s, p)?;
    need(got == want_tc(later), ROW, || {
        format!("the view index's tc per unit after: {got:?}, want {later:?}")
    })
}

/// A watcher's snapshot carries the timeline's rate and start timecode: the `timeline` root's
/// keyed page holds the entries `rate` (`24/1`) and `tc` (01:00:00:00 at 24 fps, in ticks),
/// before the view's units pages, so a player has the clock its units' `tc` count in.
pub fn row_snapshot_carries_the_rate_and_start_tc() -> Result<(), String> {
    const ROW: &str = "row_snapshot_carries_the_rate_and_start_tc";
    let (f, s) = open("snapshot-clock", &cut());
    let p = f.person(&s);
    let frames = snapshot_frames(ROW, &s, p)?;
    let pages: Vec<&Json> = frames
        .iter()
        .filter(|fr| fr.get("type").and_then(Json::as_str) == Some("keyed_page"))
        .collect();
    let at = pages
        .iter()
        .position(|fr| fr.get("root").and_then(Json::as_str) == Some(TIMELINE))
        .ok_or_else(|| format!("{ROW}: the snapshot has no page of the timeline root"))?;
    let entry = |k: &str| -> Option<String> {
        match pages[at].get("entries") {
            Some(Json::List(es)) => es.iter().find_map(|e| match e {
                Json::List(t) if t.len() == 3 && t[0].as_str() == Some(k) && t[1] == Json::Null => {
                    t[2].as_str().map(str::to_owned)
                }
                _ => None,
            }),
            _ => None,
        }
    };
    let (rate, tc) = (entry("rate"), entry("tc"));
    let start = ticks::text(86_400 * F);
    need(
        rate.as_deref() == Some("24/1") && tc.as_deref() == Some(start.as_str()),
        ROW,
        || {
            format!(
                "the timeline page (page {} of {}) carries rate {rate:?} and tc {tc:?}, want \
                 24/1 and {start}",
                pages[at].get("page").map_or("?".into(), Json::to_json),
                pages[at].get("pages").map_or("?".into(), Json::to_json)
            )
        },
    )?;
    let units = pages.iter().position(|fr| fr.get("units").is_some());
    need(units.is_some_and(|u| at < u), ROW, || {
        format!("the timeline page ({at}) isn't before the units pages ({units:?})")
    })?;
    println!(
        "{ROW}: the timeline page is keyed page {} of {}: rate {}, tc {}",
        pages[at].get("page").map_or("?".into(), Json::to_json),
        pages[at].get("pages").map_or("?".into(), Json::to_json),
        rate.unwrap_or_default(),
        tc.unwrap_or_default()
    );
    Ok(())
}

// ---------- drop-frame ----------

/// `bytes` opened live, or why not (a decline is the row's failure, never a panic).
fn try_open(row: &str, tag: &str, bytes: &[u8]) -> Result<(Fx, Arc<Session>), String> {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f
        .try_open()
        .map_err(|e| format!("{row}: {tag}: it doesn't open live: {e}"))?;
    Ok((f, s))
}

/// Each unit's span on its `geom` page (`derived.tc.<unit>.geom`) as the snapshot's keyed pages
/// carry it, by address in their order.
fn snapshot_page_tc(frames: &[Json]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for fr in frames
        .iter()
        .filter(|fr| fr.get("type").and_then(Json::as_str) == Some("keyed_page"))
    {
        if let Some(Json::Obj(units)) = fr.get("derived").and_then(|d| d.get("tc")) {
            for (u, v) in units {
                if let Some(g) = v.get("geom").and_then(Json::as_str) {
                    out.push((u.clone(), g.to_owned()));
                }
            }
        }
    }
    out
}

/// The tier-0 reader's `drop_frame` for the file's one timeline.
fn tier0_drop_frame(row: &str, bytes: &[u8]) -> Result<bool, String> {
    let doc = read_otio(bytes, &ReadLimits::default())
        .map_err(|e| format!("{row}: control: the tier-0 read: {e}"))?;
    doc.timelines
        .first()
        .map(|t| t.drop_frame)
        .ok_or_else(|| format!("{row}: control: no timeline"))
}

/// A drop-frame OTIO timeline labels its timecodes drop-frame, `HH:MM:SS;FF`: a 29.97 file
/// (`30000/1001`, from frame 107892) whose timeline metadata carries a third-party converter's
/// drop-frame (DF/NDF) entry, `DF`, opens with the `timeline` map's `drop_frame` `1`; its first
/// clip starts at `01:00:00;00` and the second, sixty frames later, at `01:00:02;00` (two labels
/// a minute skipped, so not the non-drop `00:59:56:12`), in the view index, the snapshot's units
/// pages, each unit's `geom` page (its span) and `clip list`; the tier-0 reader says drop-frame
/// too. At 59.94 (`60000/1001`, from frame 215784) the same: `01:00:00;00` and, 120 frames
/// later, `01:00:02;00`.
pub fn row_drop_frame_labels_drop_frame() -> Result<(), String> {
    const ROW: &str = "row_drop_frame_labels_drop_frame";
    let bytes = file(&with_df_entry(decimal_tree(), js("DF")));
    let (f, s) = try_open(ROW, "drop-frame", &bytes)?;
    let (p, a) = (f.person(&s), f.agent(&s));
    let df: &[(&str, &str)] = &[("@item:1", "01:00:00;00"), ("@item:2", "01:00:02;00")];
    let got = index_tc(ROW, &s, p)?;
    need(got == want_tc(df), ROW, || {
        format!("at 30000/1001 drop-frame the view index's tc per unit: {got:?}, want {df:?}")
    })?;
    let c = read_content(ROW, &s, p)?;
    let flag = tl_field(&c, "drop_frame");
    need(flag.as_deref() == Some("1"), ROW, || {
        format!("the timeline's drop_frame from a DF file is {flag:?}, want \"1\"")
    })?;
    let frames = snapshot_frames(ROW, &s, p)?;
    let paged = snapshot_unit_tc(&frames);
    need(paged == want_tc(df), ROW, || {
        format!("the snapshot's units pages carry tc {paged:?}, want {df:?}")
    })?;
    let spans = snapshot_page_tc(&frames);
    let want_spans = vec![
        ("@item:1".to_owned(), "01:00:00;00-01:00:01;00".to_owned()),
        ("@item:2".to_owned(), "01:00:02;00-01:00:03;00".to_owned()),
    ];
    need(spans == want_spans, ROW, || {
        format!("the geom pages' spans {spans:?}, want {want_spans:?}")
    })?;
    let o = run(&s, a, &["clip", "list", "@trk:1"]).map_err(|e| format!("{ROW}: {e}"))?;
    let out = stdout(&o);
    need(
        o.exit == 0 && out.contains("\t01:00:00;00\t") && out.contains("\t01:00:02;00\t"),
        ROW,
        || format!("clip list shows the starts non-drop: {out}"),
    )?;
    let tier0 = tier0_drop_frame(ROW, &bytes)?;
    need(tier0, ROW, || {
        "the tier-0 reader's drop_frame is false".into()
    })?;
    let (g, d) = try_open(
        ROW,
        "drop-frame-5994",
        &file(&with_df_entry(decimal_tree_5994(), js("DF"))),
    )?;
    let q = g.person(&d);
    let got = index_tc(ROW, &d, q)?;
    need(got == want_tc(df), ROW, || {
        format!("at 60000/1001 drop-frame the view index's tc per unit: {got:?}, want {df:?}")
    })
}

/// A non-drop timeline is labelled as before: a 29.97 OTIO file with no drop-frame entry, or
/// with the entry `NDF`, opens with the `timeline` map's `drop_frame` `0`, and reads
/// `00:59:56:12` and `00:59:58:12` from frame 107892 (spans `00:59:56:12-00:59:57:12` and
/// `00:59:58:12-00:59:59:12`). The flag is read-only: a changed one doesn't write back. A client
/// from before the field reads the doc as it did.
pub fn row_non_drop_is_unchanged() -> Result<(), String> {
    const ROW: &str = "row_non_drop_is_unchanged";
    let ndf: &[(&str, &str)] = &[("@item:1", "00:59:56:12"), ("@item:2", "00:59:58:12")];
    let bytes = decimal_rate();
    let (f, s) = try_open(ROW, "non-drop", &bytes)?;
    let p = f.person(&s);
    let got = index_tc(ROW, &s, p)?;
    need(got == want_tc(ndf), ROW, || {
        format!("at 30000/1001 non-drop the view index's tc per unit: {got:?}, want {ndf:?}")
    })?;
    let spans = snapshot_page_tc(&snapshot_frames(ROW, &s, p)?);
    let want_spans = vec![
        ("@item:1".to_owned(), "00:59:56:12-00:59:57:12".to_owned()),
        ("@item:2".to_owned(), "00:59:58:12-00:59:59:12".to_owned()),
    ];
    need(spans == want_spans, ROW, || {
        format!("the geom pages' spans {spans:?}, want {want_spans:?}")
    })?;
    let c = read_content(ROW, &s, p)?;
    let flag = tl_field(&c, "drop_frame");
    need(flag.as_deref() == Some("0"), ROW, || {
        format!("the timeline's drop_frame from an OTIO file is {flag:?}, want \"0\"")
    })?;
    // the entry NDF: non-drop
    let (g, t) = try_open(
        ROW,
        "non-drop-ndf",
        &file(&with_df_entry(decimal_tree(), js("NDF"))),
    )?;
    let q = g.person(&t);
    let got = index_tc(ROW, &t, q)?;
    let flag = tl_field(&read_content(ROW, &t, q)?, "drop_frame");
    need(
        got == want_tc(ndf) && flag.as_deref() == Some("0"),
        ROW,
        || format!("with NDF: drop_frame {flag:?}, the index's tc {got:?}, want \"0\" and {ndf:?}"),
    )?;
    // read-only: a changed one doesn't write back
    let mut edited = decoded(ROW, &bytes)?;
    for (r, n) in edited.iter_mut() {
        if let (true, Neutral::Map(m)) = (r == TIMELINE, n) {
            m.insert("drop_frame".into(), Value::str("1"));
        }
    }
    match splice_into(ROW, &bytes, &edited) {
        Err(e) if e.contains("drop_frame") => {}
        Err(e) => return Err(format!("{ROW}: refused, but not for drop_frame: {e}")),
        Ok(_) => {
            return Err(format!(
                "{ROW}: a changed drop_frame was written to the file"
            ))
        }
    }
    older_client_reads_the_same(ROW, &c, &[(TIMELINE, "drop_frame")])
}

/// The drop-frame entry is read defensively: only exactly `DF` or `NDF`, and `DF` only at
/// `30000/1001` or `60000/1001`. Lower case (`df`), an unknown value, a value that isn't a
/// string, and `DF` at 25 fps are each ignored: the file opens live (no decline, no error), the
/// `timeline` map's `drop_frame` is `0`, the tier-0 reader says non-drop, and the labels are
/// non-drop (`00:59:56:12` and `00:59:58:12` at 29.97; `01:00:00:00` and `01:00:02:00` at
/// 25 fps from frame 90000).
pub fn row_drop_frame_entry_read_defensively() -> Result<(), String> {
    const ROW: &str = "row_drop_frame_entry_read_defensively";
    let ndf: &[(&str, &str)] = &[("@item:1", "00:59:56:12"), ("@item:2", "00:59:58:12")];
    let at25: &[(&str, &str)] = &[("@item:1", "01:00:00:00"), ("@item:2", "01:00:02:00")];
    type Want = &'static [(&'static str, &'static str)];
    let cases: Vec<(&str, Vec<u8>, Want)> = vec![
        (
            "lower case df",
            file(&with_df_entry(decimal_tree(), js("df"))),
            ndf,
        ),
        (
            "an unknown value",
            file(&with_df_entry(decimal_tree(), js("DROP"))),
            ndf,
        ),
        (
            "a number",
            file(&with_df_entry(decimal_tree(), jraw("1"))),
            ndf,
        ),
        (
            "DF at 25 fps",
            file(&with_df_entry(tree_25(), js("DF"))),
            at25,
        ),
    ];
    for (n, (what, bytes, want)) in cases.iter().enumerate() {
        let (f, s) = try_open(ROW, &format!("df-entry-{n}"), bytes)?;
        let p = f.person(&s);
        let flag = tl_field(&read_content(ROW, &s, p)?, "drop_frame");
        need(flag.as_deref() == Some("0"), ROW, || {
            format!("{what}: the timeline's drop_frame is {flag:?}, want \"0\"")
        })?;
        let got = index_tc(ROW, &s, p)?;
        need(got == want_tc(want), ROW, || {
            format!("{what}: the view index's tc per unit: {got:?}, want {want:?}")
        })?;
        let tier0 = tier0_drop_frame(ROW, bytes)?;
        need(!tier0, ROW, || {
            format!("{what}: the tier-0 reader's drop_frame is true")
        })?;
    }
    Ok(())
}

// ---------- each track's label and kind ----------

/// A timeline whose tracks' names aren't their labels, at 24 fps from 01:00:00:00, bottom
/// first: `Picture` (video, Shot A), `Dialogue` (audio, Music), `Titles` (video, empty) and
/// `Music` (audio, empty). With `insert`, a video track `Insert` (one clip) before them all, as
/// an app's save that adds a track at the bottom writes it.
pub fn named_tracks(insert: bool) -> Vec<u8> {
    let c = cut_clips();
    let mut tracks = Vec::new();
    if insert {
        let i = ClipSpec::new("Insert", "file:///media/insert.mov", "0.0", "24.0", "24.0");
        tracks.push(track("Insert", "Video", vec![clip(&i)], vec![]));
    }
    tracks.extend([
        track("Picture", "Video", vec![clip(&c[0])], vec![]),
        track("Dialogue", "Audio", vec![clip(&c[4])], vec![]),
        track("Titles", "Video", vec![], vec![]),
        track("Music", "Audio", vec![], vec![]),
    ]);
    file(&timeline(
        "Named",
        Some(("86400.0", "24.0")),
        tracks,
        vec![],
    ))
}

/// One lane of the view index: its address, title, and derived `kind_label` and `kind`.
type Lane = (String, Option<String>, Option<String>, Option<String>);

/// Each container of `c`'s view index, in its order, as a [`Lane`].
fn index_lanes(row: &str, s: &Session, c: ConnId) -> Result<Vec<Lane>, String> {
    let v = s
        .view_index(c)
        .map_err(|e| format!("{row}: {e}"))?
        .ok_or_else(|| format!("{row}: no view index"))?;
    Ok(v.containers
        .iter()
        .map(|c| {
            (
                c.key.clone(),
                c.title.clone(),
                c.derived_value("kind_label").map(str::to_owned),
                c.derived_value("kind").map(str::to_owned),
            )
        })
        .collect())
}

/// `want` (address, title, label, kind) as [`index_lanes`] gives it.
fn want_lanes(want: &[(&str, &str, &str, &str)]) -> Vec<Lane> {
    want.iter()
        .map(|(k, t, l, d)| {
            (
                (*k).to_owned(),
                Some((*t).to_owned()),
                Some((*l).to_owned()),
                Some((*d).to_owned()),
            )
        })
        .collect()
}

/// The lanes of [`named_tracks`] as it opens: video tracks `V1`, `V2` and audio tracks `A1`,
/// `A2`, each counted among its kind in stacking order.
const NAMED_LANES: &[(&str, &str, &str, &str)] = &[
    ("@trk:1", "Picture", "V1", "video"),
    ("@trk:2", "Dialogue", "A1", "audio"),
    ("@trk:3", "Titles", "V2", "video"),
    ("@trk:4", "Music", "A2", "audio"),
];

/// The containers' part of the change frames' `derived` among `frames`
/// (`{container: {name: text or null}}`), joined over every change frame in order, as compact
/// JSON; `None` when no change frame has one.
fn frames_container_derived(frames: &[WatchFrame]) -> Option<String> {
    let mut all: Vec<(String, Json)> = Vec::new();
    let mut any = false;
    for fr in frames
        .iter()
        .filter(|fr| matches!(fr, WatchFrame::Change(_)))
    {
        let v = fr.to_value();
        if let Some(Json::Obj(cs)) = v.get("derived").and_then(|d| d.get("containers")) {
            any = true;
            all.extend(cs.iter().cloned());
        }
    }
    any.then(|| Json::Obj(all).to_json())
}

/// A keyed watch on `c` with its snapshot read, so the next frames are the changes'.
fn watch_after_snapshot(
    row: &str,
    s: &Session,
    c: ConnId,
) -> Result<k2_mpc_base::session::KeyedWatch, String> {
    let mut w = s
        .watch_keyed(c, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("{row}: {e}"))?
        .ok_or_else(|| format!("{row}: no keyed watch"))?;
    s.watch_snapshot(&mut w)
        .map_err(|e| format!("{row}: the snapshot: {e}"))?;
    w.try_frames();
    Ok(w)
}

/// Each track (each container of the view) carries two derived values: `kind_label`, its label
/// by the format's convention (its kind's letter, `V` or `A`, and its place among the tracks of
/// its kind in stacking order: `V1`, `A2`), never its name (the container's title); and `kind`,
/// `video` or `audio`. The view index has them, the label is the one `track list` prints, and
/// the host's read of one container gives them in the schema's order.
pub fn row_each_track_carries_its_label_and_kind() -> Result<(), String> {
    const ROW: &str = "row_each_track_carries_its_label_and_kind";
    let (f, s) = open("track-label", &named_tracks(false));
    let p = f.person(&s);
    let got = index_lanes(ROW, &s, p)?;
    let want = want_lanes(NAMED_LANES);
    need(got == want, ROW, || {
        format!("the view index's lanes (address, title, kind_label, kind): {got:?}, want {want:?}")
    })?;
    let list = run(&s, p, &["track", "list"]).map_err(|e| format!("{ROW}: {e}"))?;
    let printed: Vec<(String, String)> = stdout(&list)
        .lines()
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .filter(|c| c.len() >= 3)
        .map(|c| (c[0].to_owned(), c[2].to_owned()))
        .collect();
    let labels: Vec<(String, String)> = NAMED_LANES
        .iter()
        .map(|(k, _, l, _)| ((*k).to_owned(), (*l).to_owned()))
        .collect();
    need(printed == labels, ROW, || {
        format!("track list's labels {printed:?}, want {labels:?}")
    })?;
    let read = s
        .view_container(p, "@trk:2")
        .map_err(|e| format!("{ROW}: {e}"))?
        .map(|c| c.derived);
    let want = vec![
        ("kind_label".to_owned(), "A1".to_owned()),
        ("kind".to_owned(), "audio".to_owned()),
    ];
    need(read.as_ref() == Some(&want), ROW, || {
        format!("the host's read of @trk:2 gives {read:?}, want {want:?}")
    })
}

/// A watcher's snapshot carries each track's `kind_label` and `kind` on its container
/// (`view.containers[].derived`, `{"kind_label": "V1", "kind": "video"}`), and the schema names
/// them (`container_derived`), so a player draws each lane's header from the snapshot alone.
pub fn row_snapshot_carries_track_label_and_kind() -> Result<(), String> {
    const ROW: &str = "row_snapshot_carries_track_label_and_kind";
    let (f, s) = open("track-label-snapshot", &named_tracks(false));
    let p = f.person(&s);
    let frames = snapshot_frames(ROW, &s, p)?;
    let view = frames
        .first()
        .and_then(|h| h.get("view"))
        .ok_or_else(|| format!("{ROW}: control: no view in the snapshot's head"))?;
    let Some(Json::List(cs)) = view.get("containers") else {
        return Err(format!("{ROW}: control: the snapshot has no containers"));
    };
    for (key, _, label, kind) in NAMED_LANES {
        let got = cs
            .iter()
            .find(|c| c.get("key").and_then(Json::as_str) == Some(*key))
            .and_then(|c| c.get("derived"))
            .map(Json::to_json);
        let want = format!(r#"{{"kind_label":"{label}","kind":"{kind}"}}"#);
        need(got.as_deref() == Some(want.as_str()), ROW, || {
            format!("the snapshot's {key} has derived {got:?}, want {want}")
        })?;
    }
    let names = view
        .get("schema")
        .and_then(|s| s.get("container_derived"))
        .map(Json::to_json);
    need(
        names.as_deref() == Some(r#"["kind_label","kind"]"#),
        ROW,
        || format!("the schema's container_derived is {names:?}"),
    )
}

/// A track's rename changes its title, not its label or kind (the label is the format's
/// convention, never the name): after `track rename @trk:1 "Picture B"` the view index the kit
/// kept through it still gives `@trk:1` `V1` and `video`, and the rename's change frame carries
/// no derived value for it (a value the change didn't move isn't sent again).
pub fn row_rename_keeps_track_label_and_kind() -> Result<(), String> {
    const ROW: &str = "row_rename_keeps_track_label_and_kind";
    let (f, s) = open("track-label-rename", &named_tracks(false));
    let (p, a) = (f.person(&s), f.agent(&s));
    // the index read once before, so the kit keeps it through the rename
    index_lanes(ROW, &s, p)?;
    let mut w = watch_after_snapshot(ROW, &s, p)?;
    verb(ROW, &s, a, &["track", "rename", "@trk:1", "Picture B"])?;
    let frames = w.try_frames();
    one_change(ROW, "the rename", &frames)?;
    let got = index_lanes(ROW, &s, p)?;
    let mut lanes = NAMED_LANES.to_vec();
    lanes[0].1 = "Picture B";
    let want = want_lanes(&lanes);
    need(got == want, ROW, || {
        format!("after the rename the view index's lanes are {got:?}, want {want:?}")
    })?;
    let carried = frames_container_derived(&frames);
    need(
        !carried.as_deref().is_some_and(|c| c.contains("\"@trk:1\"")),
        ROW,
        || format!("the rename's frame carries @trk:1's derived values: {carried:?}"),
    )
}

/// A change that moves tracks' labels carries the new ones in its change frame
/// (`derived.containers.<track>.kind_label`), only for the tracks whose label moved, and the
/// index the kit kept through it says the same:
/// - a person's hand edit that moves `@trk:3` (`Titles`, `V2`) to the bottom of the stack makes
///   it `V1` and `@trk:1` (`Picture`) `V2`; the audio tracks keep theirs;
/// - an app's save that adds a video track at the bottom: tracks pair by their name and kind, so
///   every kept track keeps its key, the track that comes is a new container `@trk:5` with `V1`
///   and `video`, and the video tracks above it move up a place: `@trk:1` (`Picture`) is `V2`
///   and `@trk:3` (`Titles`) `V3`; the audio tracks keep theirs.
pub fn row_track_label_shift_rides_the_frame() -> Result<(), String> {
    const ROW: &str = "row_track_label_shift_rides_the_frame";
    let (f, s) = open("track-label-move", &named_tracks(false));
    let p = f.person(&s);
    index_lanes(ROW, &s, p)?;
    let mut w = watch_after_snapshot(ROW, &s, p)?;
    s.edit(
        p,
        vec![Edit::List {
            root: TRACKS.into(),
            ops: vec![ListOp::Move {
                key: model::key('t', 3),
                to: Anchor::Start,
            }],
        }],
        None,
    )
    .map_err(|e| format!("{ROW}: the hand edit: {e}"))?;
    let frames = w.try_frames();
    one_change(ROW, "the move", &frames)?;
    let carried = frames_container_derived(&frames);
    let want = r#"{"@trk:3":{"kind_label":"V1"},"@trk:1":{"kind_label":"V2"}}"#;
    need(carried.as_deref() == Some(want), ROW, || {
        format!("the move's frame carries derived containers {carried:?}, want {want}")
    })?;
    let got = index_lanes(ROW, &s, p)?;
    let want = want_lanes(&[
        ("@trk:3", "Titles", "V1", "video"),
        ("@trk:1", "Picture", "V2", "video"),
        ("@trk:2", "Dialogue", "A1", "audio"),
        ("@trk:4", "Music", "A2", "audio"),
    ]);
    need(got == want, ROW, || {
        format!("after the move the view index's lanes are {got:?}, want {want:?}")
    })?;
    // an app's save that adds a track at the bottom
    let (g, d) = open("track-label-save", &named_tracks(false));
    let q = g.person(&d);
    index_lanes(ROW, &d, q)?;
    let mut w = watch_after_snapshot(ROW, &d, q)?;
    g.advance(1_000);
    app_save(&g.path(), &named_tracks(true));
    let out = d.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("control: the save is taken in: {out:?}")
    })?;
    // Restated for pairing by name and kind. Under pairing by label it asserted:
    //   want = r#"{"@trk:5":{"kind_label":"V3","kind":"video"}}"#;
    //   lanes ("@trk:1", "Insert", "V1"), ("@trk:3", "Picture", "V2"), ("@trk:2", "Dialogue",
    //   "A1"), ("@trk:5", "Titles", "V3"), ("@trk:4", "Music", "A2")
    let carried = frames_container_derived(&w.try_frames());
    let want = SAVE_ADDS_BOTTOM_FRAME;
    need(carried.as_deref() == Some(want), ROW, || {
        format!("the save's frames carry derived containers {carried:?}, want {want}")
    })?;
    let got = index_lanes(ROW, &d, q)?;
    let want = want_lanes(SAVE_ADDS_BOTTOM_LANES);
    need(got == want, ROW, || {
        format!("after the save the view index's lanes are {got:?}, want {want:?}")
    })
}

// ---------- outside saves: which track is which ----------

/// The change frames' derived containers after an app's save adds a video track (`Insert`) at
/// the bottom of [`named_tracks`]: the new track's values, and the video tracks above it each a
/// place up.
const SAVE_ADDS_BOTTOM_FRAME: &str = r#"{"@trk:5":{"kind_label":"V1","kind":"video"},"@trk:1":{"kind_label":"V2"},"@trk:3":{"kind_label":"V3"}}"#;

/// The view's lanes after that save: every kept track on its key, the new one `@trk:5`.
const SAVE_ADDS_BOTTOM_LANES: &[(&str, &str, &str, &str)] = &[
    ("@trk:5", "Insert", "V1", "video"),
    ("@trk:1", "Picture", "V2", "video"),
    ("@trk:2", "Dialogue", "A1", "audio"),
    ("@trk:3", "Titles", "V3", "video"),
    ("@trk:4", "Music", "A2", "audio"),
];

/// A clip of the tracks rows: `Shot A` (`a`), `Shot B` (`b`), `Music` (`m`) or `Insert`
/// (`i`).
fn tclip(which: char) -> J {
    let c = cut_clips();
    match which {
        'a' => clip(&c[0]),
        'b' => clip(&c[1]),
        'm' => clip(&c[4]),
        _ => clip(&ClipSpec::new(
            "Insert",
            "file:///media/insert.mov",
            "0.0",
            "24.0",
            "24.0",
        )),
    }
}

/// A timeline of `tracks`, bottom first: each its name, its OTIO kind (`Video`, `Audio`) and its
/// clips, at 24 fps from 01:00:00:00.
fn tracks_file(tracks: Vec<(&str, &str, Vec<J>)>) -> Vec<u8> {
    file(&timeline(
        "Named",
        Some(("86400.0", "24.0")),
        tracks
            .into_iter()
            .map(|(n, k, c)| track(n, k, c, vec![]))
            .collect(),
        vec![],
    ))
}

/// The live tracks as `c` reads them, in order: each key, its kind and its name.
fn tracks_now(row: &str, s: &Session, c: ConnId) -> Result<Vec<(String, String, String)>, String> {
    let content = read_content(row, s, c)?;
    Ok(list(&content, TRACKS)
        .into_iter()
        .map(|(k, v)| {
            let n = field(&content, TRACK_PROPS, &k, "name").unwrap_or_default();
            (k, v, n)
        })
        .collect())
}

/// `want` (key, kind, name) as [`tracks_now`] gives it.
fn want_tracks(want: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    want.iter()
        .map(|(k, v, n)| ((*k).to_owned(), (*v).to_owned(), (*n).to_owned()))
        .collect()
}

/// The track key of the item named `name`, as `c` reads it.
fn item_track(row: &str, s: &Session, c: ConnId, name: &str) -> Result<Option<String>, String> {
    let content = read_content(row, s, c)?;
    Ok(named(&content, ITEMS, ITEM_PROPS, name)
        .and_then(|k| field(&content, ITEM_PROPS, &k, "track")))
}

/// An app's save of `bytes` over `f`'s file, which `s` must take in.
fn outside_save(row: &str, f: &Fx, s: &Session, bytes: &[u8]) -> Result<(), String> {
    f.advance(1_000);
    app_save(&f.path(), bytes);
    let out = s.file_changed().map_err(|e| format!("{row}: {e}"))?;
    need(matches!(out, DiskCheck::Ingested { .. }), row, || {
        format!("control: the save is taken in: {out:?}")
    })
}

/// [`named_tracks`] as it opens: `Picture` `t1`, `Dialogue` `t2`, `Titles` `t3`, `Music` `t4`.
const NAMED_KEYS: &[(&str, &str, &str)] = &[
    ("t1", "video", "Picture"),
    ("t2", "audio", "Dialogue"),
    ("t3", "video", "Titles"),
    ("t4", "audio", "Music"),
];

/// An app's save that adds a video track (`Insert`, one clip) at the bottom: tracks pair by
/// their name and kind, so every kept track keeps its key and its contents (its name, its clip),
/// the new track takes a new key (`t5`) at the bottom, and the change frame moves the video
/// tracks' labels up a place (`Picture` `V1` to `V2`, `Titles` `V2` to `V3`).
pub fn row_save_adding_a_bottom_track_keeps_every_key() -> Result<(), String> {
    const ROW: &str = "row_save_adding_a_bottom_track_keeps_every_key";
    let (f, s) = open("pair-bottom", &named_tracks(false));
    let p = f.person(&s);
    let got = tracks_now(ROW, &s, p)?;
    need(got == want_tracks(NAMED_KEYS), ROW, || {
        format!("control: the tracks as it opens: {got:?}")
    })?;
    index_lanes(ROW, &s, p)?;
    let mut w = watch_after_snapshot(ROW, &s, p)?;
    outside_save(ROW, &f, &s, &named_tracks(true))?;
    let got = tracks_now(ROW, &s, p)?;
    let want = want_tracks(&[
        ("t5", "video", "Insert"),
        ("t1", "video", "Picture"),
        ("t2", "audio", "Dialogue"),
        ("t3", "video", "Titles"),
        ("t4", "audio", "Music"),
    ]);
    need(got == want, ROW, || {
        format!("after the save the tracks (key, kind, name) are {got:?}, want {want:?}")
    })?;
    for (clip, track) in [("Shot A", "t1"), ("Music", "t2"), ("Insert", "t5")] {
        let on = item_track(ROW, &s, p, clip)?;
        need(on.as_deref() == Some(track), ROW, || {
            format!("after the save {clip} is on {on:?}, want {track}")
        })?;
    }
    let carried = frames_container_derived(&w.try_frames());
    need(
        carried.as_deref() == Some(SAVE_ADDS_BOTTOM_FRAME),
        ROW,
        || {
            format!("the save's frames carry derived containers {carried:?}, want {SAVE_ADDS_BOTTOM_FRAME}")
        },
    )?;
    let got = index_lanes(ROW, &s, p)?;
    let want = want_lanes(SAVE_ADDS_BOTTOM_LANES);
    need(got == want, ROW, || {
        format!("after the save the view index's lanes are {got:?}, want {want:?}")
    })
}

/// Renames and moves in an outside save, in the pairing's order: a track's name and kind first,
/// so a track moved up or down the stack keeps its key; a uniquely named track whose name
/// matches no track of its kind on the other side is never paired by its place (a name that
/// changed can't be told from a track deleted and another added), so a track renamed outside is
/// a new track, under a new key, and its old key goes, whether it also moved or not:
/// - one save that moves `Titles` to the bottom and renames `Music` to `Score` in place:
///   `Titles` keeps `t3` at the bottom, `Score` is `t5`, `t4` goes;
/// - one save that renames `Titles` to `Credits` and moves it to the bottom: `Credits` is `t5`,
///   `t3` goes; every other track keeps its key.
pub fn row_save_rename_and_move_pair_by_name_first() -> Result<(), String> {
    const ROW: &str = "row_save_rename_and_move_pair_by_name_first";
    let (f, s) = open("pair-move-rename", &named_tracks(false));
    let p = f.person(&s);
    outside_save(
        ROW,
        &f,
        &s,
        &tracks_file(vec![
            ("Titles", "Video", vec![]),
            ("Picture", "Video", vec![tclip('a')]),
            ("Dialogue", "Audio", vec![tclip('m')]),
            ("Score", "Audio", vec![]),
        ]),
    )?;
    let got = tracks_now(ROW, &s, p)?;
    let want = want_tracks(&[
        ("t3", "video", "Titles"),
        ("t1", "video", "Picture"),
        ("t2", "audio", "Dialogue"),
        ("t5", "audio", "Score"),
    ]);
    need(got == want, ROW, || {
        format!("a move and a rename: the tracks (key, kind, name) are {got:?}, want {want:?}")
    })?;
    let (g, d) = open("pair-rename-moved", &named_tracks(false));
    let q = g.person(&d);
    outside_save(
        ROW,
        &g,
        &d,
        &tracks_file(vec![
            ("Credits", "Video", vec![]),
            ("Picture", "Video", vec![tclip('a')]),
            ("Dialogue", "Audio", vec![tclip('m')]),
            ("Music", "Audio", vec![]),
        ]),
    )?;
    let got = tracks_now(ROW, &d, q)?;
    let want = want_tracks(&[
        ("t5", "video", "Credits"),
        ("t1", "video", "Picture"),
        ("t2", "audio", "Dialogue"),
        ("t4", "audio", "Music"),
    ]);
    need(got == want, ROW, || {
        format!(
            "a track renamed and moved: the tracks (key, kind, name) are {got:?}, want {want:?}"
        )
    })
}

/// Where a name can't decide (two video tracks both `Cam`, an audio track with no name), the
/// tracks of that name and kind pair in their order, bottom first (their label counted among
/// them): a save that adds a video track `Insert` at the bottom leaves the two `Cam` tracks on
/// `t1` and `t2` (they are the file's `V2` and `V3` now, the doc's `V1` and `V2` before) and the
/// unnamed audio track on `t4`; `Insert` is `t5`, and each clip stays on its track.
pub fn row_save_duplicate_names_fall_back_to_their_order() -> Result<(), String> {
    const ROW: &str = "row_save_duplicate_names_fall_back_to_their_order";
    let before = tracks_file(vec![
        ("Cam", "Video", vec![tclip('a')]),
        ("Cam", "Video", vec![tclip('b')]),
        ("Dialogue", "Audio", vec![tclip('m')]),
        ("", "Audio", vec![]),
    ]);
    let (f, s) = open("pair-dup", &before);
    let p = f.person(&s);
    let got = tracks_now(ROW, &s, p)?;
    let opened = want_tracks(&[
        ("t1", "video", "Cam"),
        ("t2", "video", "Cam"),
        ("t3", "audio", "Dialogue"),
        ("t4", "audio", ""),
    ]);
    need(got == opened, ROW, || {
        format!("control: the tracks as it opens: {got:?}")
    })?;
    outside_save(
        ROW,
        &f,
        &s,
        &tracks_file(vec![
            ("Insert", "Video", vec![tclip('i')]),
            ("Cam", "Video", vec![tclip('a')]),
            ("Cam", "Video", vec![tclip('b')]),
            ("Dialogue", "Audio", vec![tclip('m')]),
            ("", "Audio", vec![]),
        ]),
    )?;
    let got = tracks_now(ROW, &s, p)?;
    let want = want_tracks(&[
        ("t5", "video", "Insert"),
        ("t1", "video", "Cam"),
        ("t2", "video", "Cam"),
        ("t3", "audio", "Dialogue"),
        ("t4", "audio", ""),
    ]);
    need(got == want, ROW, || {
        format!("after the save the tracks (key, kind, name) are {got:?}, want {want:?}")
    })?;
    for (clip, track) in [("Shot A", "t1"), ("Shot B", "t2"), ("Music", "t3")] {
        let on = item_track(ROW, &s, p, clip)?;
        need(on.as_deref() == Some(track), ROW, || {
            format!("after the save {clip} is on {on:?}, want {track}")
        })?;
    }
    Ok(())
}

/// A name pairs only within its kind: a save where the video track `Main` is renamed `Picture`
/// and the audio track `Dialogue` renamed `Main` gives both new keys (`t3`, `t4`), and never
/// the video track's key to the audio track. A video and an audio track that share a name in
/// both docs each keep their own key.
pub fn row_save_pairs_a_name_within_its_kind() -> Result<(), String> {
    const ROW: &str = "row_save_pairs_a_name_within_its_kind";
    let (f, s) = open(
        "pair-kind",
        &tracks_file(vec![
            ("Main", "Video", vec![tclip('a')]),
            ("Dialogue", "Audio", vec![tclip('m')]),
        ]),
    );
    let p = f.person(&s);
    outside_save(
        ROW,
        &f,
        &s,
        &tracks_file(vec![
            ("Picture", "Video", vec![tclip('a')]),
            ("Main", "Audio", vec![tclip('m')]),
        ]),
    )?;
    let got = tracks_now(ROW, &s, p)?;
    let want = want_tracks(&[("t3", "video", "Picture"), ("t4", "audio", "Main")]);
    need(got == want, ROW, || {
        format!("after the save the tracks (key, kind, name) are {got:?}, want {want:?}")
    })?;
    let both = |titles: bool| {
        let mut t = vec![
            ("Main", "Video", vec![tclip('a')]),
            ("Main", "Audio", vec![tclip('m')]),
        ];
        if titles {
            t.push(("Titles", "Video", vec![]));
        }
        tracks_file(t)
    };
    let (g, d) = open("pair-kind-both", &both(false));
    let q = g.person(&d);
    outside_save(ROW, &g, &d, &both(true))?;
    let got = tracks_now(ROW, &d, q)?;
    let want = want_tracks(&[
        ("t1", "video", "Main"),
        ("t2", "audio", "Main"),
        ("t3", "video", "Titles"),
    ]);
    need(got == want, ROW, || {
        format!("a name on both kinds: the tracks (key, kind, name) are {got:?}, want {want:?}")
    })
}

/// A claim on a track (the kit's claim on elements, held) taken before an outside save that adds
/// a track at the bottom is on the same track after it: its key still names `Picture`, with
/// Shot A on it.
pub fn row_claim_on_a_track_stays_through_a_save() -> Result<(), String> {
    const ROW: &str = "row_claim_on_a_track_stays_through_a_save";
    let (f, s) = open("pair-claim", &named_tracks(false));
    let (p, a) = (f.person(&s), f.agent(&s));
    s.claim_elems(a, TRACKS, &[model::key('t', 1)], None, None, true)
        .map_err(|e| format!("{ROW}: control: the claim: {e}"))?;
    outside_save(ROW, &f, &s, &named_tracks(true))?;
    let claims = s.claims(a).map_err(|e| format!("{ROW}: {e}"))?;
    let keys: Vec<String> = claims
        .iter()
        .filter_map(|c| match &c.span {
            ClaimSpan::Elems { root, keys } if root == TRACKS => Some(
                keys.iter()
                    .map(|k| k.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    need(keys == ["t1"], ROW, || {
        format!("control: after the save the agent's track claims cover {keys:?}, want [t1]")
    })?;
    let content = read_content(ROW, &s, p)?;
    let name = field(&content, TRACK_PROPS, "t1", "name");
    need(name.as_deref() == Some("Picture"), ROW, || {
        format!("after the save the claimed track t1 is {name:?}, want Picture")
    })?;
    let on = item_track(ROW, &s, p, "Shot A")?;
    need(on.as_deref() == Some("t1"), ROW, || {
        format!("after the save Shot A is on {on:?}, want the claimed t1")
    })
}

/// A hold on a track renamed outside ends with its own reason: an agent holds `Music` (`t4`); an
/// app's save renames it `Score` in place, so the pairing gives `Score` a new key (`t5`) and `t4`
/// goes, and the claim ends `gone_outside_save` (the kit's `ClaimEnd::GoneOutsideSave`, not
/// `deleted`), naming the track as it was (`@trk:4`) on the host's end event and the roster's,
/// so the view can say which track the hold was on. Control: the claim is live before the save.
pub fn row_hold_on_a_track_renamed_outside_ends_gone_outside_save() -> Result<(), String> {
    const ROW: &str = "row_hold_on_a_track_renamed_outside_ends_gone_outside_save";
    let (f, s) = open("pair-claim-renamed", &named_tracks(false));
    let (p, a) = (f.person(&s), f.agent(&s));
    let roster = s.subscribe_roster(p).map_err(|e| format!("{ROW}: {e}"))?;
    let id = s
        .claim_elems(a, TRACKS, &[model::key('t', 4)], None, None, true)
        .map_err(|e| format!("{ROW}: control: the claim: {e}"))?
        .id;
    let live = s.claims(a).map_err(|e| format!("{ROW}: {e}"))?.len();
    need(live == 1, ROW, || {
        format!("control: before the save the agent holds {live} claims, want 1")
    })?;
    outside_save(
        ROW,
        &f,
        &s,
        &tracks_file(vec![
            ("Picture", "Video", vec![tclip('a')]),
            ("Dialogue", "Audio", vec![tclip('m')]),
            ("Titles", "Video", vec![]),
            ("Score", "Audio", vec![]),
        ]),
    )?;
    let got = tracks_now(ROW, &s, p)?;
    let want = want_tracks(&[
        ("t1", "video", "Picture"),
        ("t2", "audio", "Dialogue"),
        ("t3", "video", "Titles"),
        ("t5", "audio", "Score"),
    ]);
    need(got == want, ROW, || {
        format!("control: after the save the tracks (key, kind, name) are {got:?}, want {want:?}")
    })?;
    let audit = f.host.events.lock().unwrap().iter().find_map(|e| match e {
        AuditEvent::ClaimEnded {
            claim,
            how,
            addresses,
            ..
        } if *claim == id => Some((*how, addresses.clone())),
        _ => None,
    });
    let want = Some((ClaimEnd::GoneOutsideSave, vec!["@trk:4".to_owned()]));
    need(audit == want, ROW, || {
        format!("the hold on t4 ended {audit:?}, want {want:?}")
    })?;
    f.mgr.flush_rosters();
    let ended = roster.try_iter().find_map(|e| match e {
        RosterEvent::ClaimEnded {
            claim,
            how,
            addresses,
            ..
        } if claim == id => Some((how, addresses)),
        _ => None,
    });
    need(ended == want, ROW, || {
        format!("the roster's end of the hold {ended:?}, want {want:?}")
    })
}

/// A track deleted outside (`Picture`, the bottom video track, with its clip): its key goes,
/// and every other track keeps its own, though `Titles` is the bottom video track now.
pub fn row_save_deleting_a_track_keeps_the_others() -> Result<(), String> {
    const ROW: &str = "row_save_deleting_a_track_keeps_the_others";
    let (f, s) = open("pair-delete", &named_tracks(false));
    let p = f.person(&s);
    outside_save(
        ROW,
        &f,
        &s,
        &tracks_file(vec![
            ("Dialogue", "Audio", vec![tclip('m')]),
            ("Titles", "Video", vec![]),
            ("Music", "Audio", vec![]),
        ]),
    )?;
    let got = tracks_now(ROW, &s, p)?;
    let want = want_tracks(&[
        ("t2", "audio", "Dialogue"),
        ("t3", "video", "Titles"),
        ("t4", "audio", "Music"),
    ]);
    need(got == want, ROW, || {
        format!("after the save the tracks (key, kind, name) are {got:?}, want {want:?}")
    })
}
