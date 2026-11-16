//! A measurement on the public corpus (`TIMELINE_PUBLIC_CORPUS`; fails when unset;
//! scripts/public-corpus.sh runs it): how many files hold a time that isn't a whole number of ticks
//! (1/254,016,000,000 s), the unit a live timeline stores its times in. Such a file can't open live
//! and stays at tier 0 with its index. Also counted: files with more than one timeline (one
//! timeline per live doc), a time past an `i64` of ticks, and a record extent past 2^53 ticks.

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_mpc_timeline::model::{Item, Marker, Timeline, Track};
use k2_mpc_timeline::{Document, ReadError, ReadLimits, Time};

type Reader = fn(&[u8], &ReadLimits) -> Result<Document, ReadError>;

const MANIFEST: &str = include_str!("corpus/manifest.tsv");
const TICKS_PER_SECOND: i128 = 254_016_000_000;

/// What one time is, in ticks.
enum Tick {
    Whole(i128),
    Off,
}

fn tick(t: Time) -> Tick {
    let n = i128::from(t.num()) * TICKS_PER_SECOND;
    let d = i128::from(t.den());
    if n % d == 0 {
        Tick::Whole(n / d)
    } else {
        Tick::Off
    }
}

#[derive(Default)]
struct Seen {
    times: u64,
    off: u64,
    past_i64: u64,
    first_off: Option<String>,
}

impl Seen {
    fn time(&mut self, what: &str, t: Time) {
        self.times += 1;
        match tick(t) {
            Tick::Whole(v) => {
                if i64::try_from(v).is_err() {
                    self.past_i64 += 1;
                }
            }
            Tick::Off => {
                self.off += 1;
                if self.first_off.is_none() {
                    self.first_off = Some(format!("{what} {t}"));
                }
            }
        }
    }

    fn markers(&mut self, ms: &[Marker]) {
        for m in ms {
            self.time("marker at", m.at);
            self.time("marker duration", m.duration);
            if let Some(a) = m.media_at {
                self.time("marker media time", a);
            }
        }
    }

    fn item(&mut self, i: &Item) {
        self.time("item start", i.start);
        self.time("item duration", i.duration);
        if let Some(s) = i.source_in {
            self.time("item source in", s);
        }
        if let Some(off) = i.transition.as_ref().and_then(|t| t.in_offset) {
            self.time("transition offset", off);
        }
        self.markers(&i.markers);
        self.tracks(&i.tracks);
    }

    fn tracks(&mut self, ts: &[Track]) {
        for t in ts {
            for i in &t.items {
                self.item(i);
            }
            self.markers(&t.markers);
        }
    }

    fn timeline(&mut self, t: &Timeline) {
        self.time("timecode start", t.tc_start);
        if let Some(d) = t.duration {
            self.time("timeline duration", d);
        }
        self.tracks(&t.tracks);
        self.markers(&t.markers);
    }
}

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn share_of_public_files_off_the_tick_grid() {
    let dir = std::env::var_os("TIMELINE_PUBLIC_CORPUS")
        .map(PathBuf::from)
        .expect("TIMELINE_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    // format -> (files, off grid, many timelines, past i64, record extent past 2^53, times)
    let mut by: BTreeMap<&str, [u64; 6]> = BTreeMap::new();
    for line in MANIFEST
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let name = line.split('\t').next().unwrap();
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (format, read): (&str, Reader) = if name.ends_with(".otio") {
            ("otio", k2_mpc_timeline::otio::read_otio)
        } else if name.ends_with(".fcpxml") {
            ("fcpxml", k2_mpc_timeline::fcpxml::read_fcpxml)
        } else {
            ("prproj", k2_mpc_timeline::prproj::read_prproj)
        };
        let doc = read(&bytes, &ReadLimits::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut seen = Seen::default();
        for m in &doc.media {
            if let Some(s) = m.start {
                seen.time("media start", s);
            }
            if let Some(d) = m.duration {
                seen.time("media duration", d);
            }
        }
        let mut long = false;
        for t in &doc.timelines {
            seen.timeline(t);
            let extent = t.extent().unwrap_or_else(|e| panic!("{name}: {e}"));
            if let Tick::Whole(v) = tick(extent) {
                long |= v > (1i128 << 53);
            }
        }
        let row = by.entry(format).or_default();
        row[0] += 1;
        row[1] += u64::from(seen.off > 0);
        row[2] += u64::from(doc.timelines.len() > 1);
        row[3] += u64::from(seen.past_i64 > 0);
        row[4] += u64::from(long);
        row[5] += seen.times;
        if let Some(what) = &seen.first_off {
            println!(
                "tick grid: off: {name}: {} of {} times, first {what}",
                seen.off, seen.times
            );
        }
    }
    let mut total = [0u64; 6];
    for (format, row) in &by {
        println!(
            "tick grid: {format}: {} files, {} off the grid, {} with more than one timeline, \
             {} past i64, {} with a record extent past 2^53 ticks ({} times read)",
            row[0], row[1], row[2], row[3], row[4], row[5]
        );
        for (t, r) in total.iter_mut().zip(row) {
            *t += r;
        }
    }
    println!(
        "tick grid: all: {} files, {} off the grid ({:.1}%), {} with more than one timeline, \
         {} past i64, {} with a record extent past 2^53 ticks",
        total[0],
        total[1],
        100.0 * total[1] as f64 / total[0].max(1) as f64,
        total[2],
        total[3],
        total[4]
    );
}
