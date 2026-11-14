//! Tracks: `trak` (`tkhd`, `edts`/`elst`, `mdia`/`mdhd`/`hdlr`/`minf`/
//! `stbl`), the sample tables (ISO/IEC 14496-12 §8.6–8.7), and the sample
//! iterator that joins them (and any movie fragments) into one list.

use crate::bytes::{children, find_one, require, BoxRef, Ctx, Cur};
use crate::entry::{parse_stsd, SampleEntry};
use crate::error::{Error, FourCC, Limit, Result};

/// What the track's handler says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Timecode,
    /// Subtitles, metadata, hint tracks…: the handler type is on the track.
    Other,
}

/// One edit list entry (`elst`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    /// In the movie timescale (`mvhd`).
    pub segment_duration: u64,
    /// In the media timescale (`mdhd`); -1 is an empty edit.
    pub media_time: i64,
    /// 16.16 fixed point (0x0001_0000 is normal speed).
    pub media_rate: i32,
}

/// The edit list, as data.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EditList {
    pub entries: Vec<Edit>,
}

impl EditList {
    /// The total of the leading empty edits (`media_time == -1`), in the
    /// movie timescale: how long the track waits before it starts.
    pub fn initial_empty(&self) -> u64 {
        self.entries.iter().take_while(|e| e.media_time == -1).fold(0u64, |a, e| a.saturating_add(e.segment_duration))
    }
    /// The media time where presentation begins: the first non-empty edit's
    /// `media_time`, in the media timescale.
    pub fn first_media_time(&self) -> Option<i64> {
        self.entries.iter().find(|e| e.media_time != -1).map(|e| e.media_time)
    }
}

/// One sample, in the track's media timescale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// 0-based, across the moov tables and then the fragments.
    pub index: u64,
    /// Byte offset in the file.
    pub offset: u64,
    pub size: u32,
    /// Decode time.
    pub dts: u64,
    /// Composition time: `dts` plus the composition offset (`ctts`/`trun`).
    pub pts: i64,
    pub duration: u32,
    /// A sync sample (`stss`, or the fragment's sample flags).
    pub keyframe: bool,
    /// 1-based index into [`Track::entries`].
    pub description_index: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stsc {
    first_chunk: u32,
    samples_per_chunk: u32,
    description_index: u32,
}

#[derive(Clone, Debug)]
pub(crate) enum Sizes {
    Constant(u32),
    Table(Vec<u32>),
}

/// The decoded `stbl` tables of one track.
#[derive(Clone, Debug)]
pub(crate) struct SampleTable {
    stts: Vec<(u32, u32)>,
    ctts: Vec<(u32, i32)>,
    stss: Option<Vec<u32>>,
    stsc: Vec<Stsc>,
    sizes: Sizes,
    chunks: Vec<u64>,
    count: u64,
}

/// One `trun` of a movie fragment, with the `tfhd`/`trex` defaults applied.
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub base_dts: u64,
    pub data_offset: u64,
    pub count: u32,
    pub default_size: u32,
    pub default_duration: u32,
    pub default_flags: u32,
    pub first_flags: Option<u32>,
    /// Per-sample fields present: 1 duration, 2 size, 4 flags, 8 offset.
    pub present: u8,
    pub entries: Vec<RunEntry>,
    pub description_index: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RunEntry {
    pub duration: u32,
    pub size: u32,
    pub flags: u32,
    pub cto: i32,
}

impl Run {
    pub fn sample(&self, j: usize) -> (u32, u32, u32, i32) {
        let e = self.entries.get(j).copied().unwrap_or_default();
        let duration = if self.present & 1 != 0 { e.duration } else { self.default_duration };
        let size = if self.present & 2 != 0 { e.size } else { self.default_size };
        let flags = if self.present & 4 != 0 {
            e.flags
        } else if j == 0 {
            self.first_flags.unwrap_or(self.default_flags)
        } else {
            self.default_flags
        };
        let cto = if self.present & 8 != 0 { e.cto } else { 0 };
        (duration, size, flags, cto)
    }
}

/// The `trex` defaults for a track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackExtends {
    pub default_sample_description_index: u32,
    pub default_sample_duration: u32,
    pub default_sample_size: u32,
    pub default_sample_flags: u32,
}

/// One track of the movie.
#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub kind: TrackKind,
    pub handler: FourCC,
    /// `tkhd` flags bit 0.
    pub enabled: bool,
    /// `tkhd` width and height, 16.16 fixed point.
    pub tkhd_width: u32,
    pub tkhd_height: u32,
    /// `tkhd` duration, in the movie timescale.
    pub tkhd_duration: u64,
    /// `mdhd` timescale (ticks per second) and duration.
    pub timescale: u32,
    pub duration: u64,
    /// `mdhd` packed ISO-639-2 language (or a QuickTime language code).
    pub language: u16,
    pub edits: Option<EditList>,
    pub entries: Vec<SampleEntry>,
    pub trex: Option<TrackExtends>,
    pub(crate) table: SampleTable,
    pub(crate) runs: Vec<Run>,
    pub(crate) fragment_samples: u64,
    /// Decode time where the next fragment without `tfdt` starts.
    pub(crate) next_dts: u64,
}

impl Track {
    /// Samples in the moov tables and the fragments together.
    pub fn sample_count(&self) -> u64 {
        self.table.count + self.fragment_samples
    }

    /// Every sample in decode order: the moov tables first, then each
    /// fragment run in file order. Validated when the file was opened, so
    /// iteration cannot fail.
    pub fn samples(&self) -> Samples<'_> {
        Samples { t: self, tab: TableIter::new(&self.table), run: 0, j: 0, run_dts: 0, run_off: 0, index: 0 }
    }

    /// The shift that maps a sample's `pts` to presentation time on the
    /// movie's timeline, in this track's timescale, from the edit list:
    /// `initial_empty` (converted from the movie timescale) minus
    /// `first_media_time`. `pts + shift` is when the sample is shown; 0 when
    /// there is no edit list. Only the first non-empty edit is used: later
    /// edits (cuts, repeats, rate changes) are left to the caller.
    pub fn presentation_offset(&self, movie_timescale: u32) -> i64 {
        let Some(e) = &self.edits else { return 0 };
        let empty = if movie_timescale == 0 { 0 } else { e.initial_empty() as i128 * self.timescale as i128 / movie_timescale as i128 };
        let start = e.first_media_time().unwrap_or(0) as i128;
        (empty - start).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }
}

struct TableIter {
    i: u64,
    stts_i: usize,
    stts_left: u32,
    ctts_i: usize,
    ctts_left: u32,
    stss_i: usize,
    chunk: usize,
    stsc_i: usize,
    left_in_chunk: u32,
    offset: u64,
    dts: u64,
}

impl TableIter {
    fn new(t: &SampleTable) -> TableIter {
        TableIter {
            i: 0,
            stts_i: 0,
            stts_left: t.stts.first().map_or(0, |e| e.0),
            ctts_i: 0,
            ctts_left: t.ctts.first().map_or(0, |e| e.0),
            stss_i: 0,
            chunk: 0,
            stsc_i: 0,
            left_in_chunk: t.stsc.first().map_or(0, |e| e.samples_per_chunk),
            offset: t.chunks.first().copied().unwrap_or(0),
            dts: 0,
        }
    }

    fn next(&mut self, t: &SampleTable) -> Option<Sample> {
        if self.i >= t.count {
            return None;
        }
        while self.left_in_chunk == 0 {
            self.chunk += 1;
            let c1 = u32::try_from(self.chunk + 1).ok()?;
            if t.stsc.get(self.stsc_i + 1).is_some_and(|e| e.first_chunk == c1) {
                self.stsc_i += 1;
            }
            self.left_in_chunk = t.stsc.get(self.stsc_i)?.samples_per_chunk;
            self.offset = *t.chunks.get(self.chunk)?;
        }
        let size = match &t.sizes {
            Sizes::Constant(s) => *s,
            Sizes::Table(v) => *v.get(self.i as usize)?,
        };
        while self.stts_left == 0 {
            self.stts_i += 1;
            self.stts_left = t.stts.get(self.stts_i)?.0;
        }
        let duration = t.stts.get(self.stts_i)?.1;
        self.stts_left -= 1;
        let mut cto = 0i32;
        while self.ctts_left == 0 && self.ctts_i + 1 < t.ctts.len() {
            self.ctts_i += 1;
            self.ctts_left = t.ctts[self.ctts_i].0;
        }
        if self.ctts_left > 0 {
            cto = t.ctts[self.ctts_i].1;
            self.ctts_left -= 1;
        }
        let keyframe = match &t.stss {
            None => true,
            Some(v) => {
                if v.get(self.stss_i).is_some_and(|&n| n as u64 == self.i + 1) {
                    self.stss_i += 1;
                    true
                } else {
                    false
                }
            }
        };
        let s = Sample {
            index: self.i,
            offset: self.offset,
            size,
            dts: self.dts,
            pts: (self.dts as i64).saturating_add(cto as i64),
            duration,
            keyframe,
            description_index: t.stsc.get(self.stsc_i)?.description_index,
        };
        self.offset = self.offset.saturating_add(size as u64);
        self.left_in_chunk -= 1;
        self.dts = self.dts.saturating_add(duration as u64);
        self.i += 1;
        Some(s)
    }
}

/// The iterator [`Track::samples`] returns.
pub struct Samples<'a> {
    t: &'a Track,
    tab: TableIter,
    run: usize,
    j: usize,
    run_dts: u64,
    run_off: u64,
    index: u64,
}

impl Iterator for Samples<'_> {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        if let Some(s) = self.tab.next(&self.t.table) {
            self.index = s.index + 1;
            return Some(s);
        }
        loop {
            let r = self.t.runs.get(self.run)?;
            if self.j == 0 {
                self.run_dts = r.base_dts;
                self.run_off = r.data_offset;
            }
            if self.j >= r.count as usize {
                self.run += 1;
                self.j = 0;
                continue;
            }
            let (duration, size, flags, cto) = r.sample(self.j);
            let s = Sample {
                index: self.index,
                offset: self.run_off,
                size,
                dts: self.run_dts,
                pts: (self.run_dts as i64).saturating_add(cto as i64),
                duration,
                keyframe: flags & SAMPLE_IS_NON_SYNC == 0,
                description_index: r.description_index,
            };
            self.j += 1;
            self.index += 1;
            self.run_dts = self.run_dts.saturating_add(duration as u64);
            self.run_off = self.run_off.saturating_add(size as u64);
            return Some(s);
        }
    }
}

/// `sample_is_non_sync_sample` in the sample flags.
pub(crate) const SAMPLE_IS_NON_SYNC: u32 = 0x0001_0000;

const STBL: FourCC = FourCC(*b"stbl");

fn table<'a>(b: BoxRef<'a>, ctx: &Ctx) -> Result<Cur<'a>> {
    if b.payload.len() as u64 > ctx.limits.max_table_bytes {
        return Err(Error::TooLarge { fourcc: b.fourcc, size: b.payload.len() as u64, limit: ctx.limits.max_table_bytes });
    }
    Ok(Cur::new(b.payload, b.fourcc))
}

fn full_box_v0(c: &mut Cur<'_>) -> Result<u8> {
    let (v, _) = c.full_box()?;
    Ok(v)
}

/// Parses one `trak`.
pub(crate) fn parse_trak(b: BoxRef<'_>, depth: u32, ctx: &mut Ctx) -> Result<Track> {
    let kids = children(b.payload, b.fourcc, depth, ctx)?;
    let trak = b.fourcc;

    let tkhd = require(&kids, trak, b"tkhd")?;
    let mut c = Cur::new(tkhd.payload, tkhd.fourcc);
    let (v, flags) = c.full_box()?;
    let (id, tkhd_duration) = match v {
        0 => {
            c.skip(8)?;
            let id = c.u32()?;
            c.skip(4)?;
            (id, c.u32()? as u64)
        }
        1 => {
            c.skip(16)?;
            let id = c.u32()?;
            c.skip(4)?;
            (id, c.u64()?)
        }
        v => return Err(Error::UnsupportedVersion { fourcc: tkhd.fourcc, version: v }),
    };
    c.skip(8 + 2 + 2 + 2 + 2 + 36)?;
    let tkhd_width = c.u32()?;
    let tkhd_height = c.u32()?;
    if id == 0 {
        return Err(Error::Invalid { fourcc: tkhd.fourcc, what: "track_ID is 0" });
    }

    let edits = match find_one(&kids, b"edts")? {
        Some(e) => {
            let ek = children(e.payload, e.fourcc, depth + 1, ctx)?;
            match find_one(&ek, b"elst")? {
                Some(l) => Some(parse_elst(l, ctx)?),
                None => None,
            }
        }
        None => None,
    };

    let mdia = require(&kids, trak, b"mdia")?;
    let mk = children(mdia.payload, mdia.fourcc, depth + 1, ctx)?;
    let mdhd = require(&mk, mdia.fourcc, b"mdhd")?;
    let mut c = Cur::new(mdhd.payload, mdhd.fourcc);
    let (v, _) = c.full_box()?;
    c.skip(if v == 1 { 16 } else { 8 })?;
    let timescale = c.u32()?;
    let duration = c.u32_or_u64(v)?;
    let language = c.u16()?;
    if timescale == 0 {
        return Err(Error::Invalid { fourcc: mdhd.fourcc, what: "timescale is 0" });
    }
    let hdlr = require(&mk, mdia.fourcc, b"hdlr")?;
    let mut c = Cur::new(hdlr.payload, hdlr.fourcc);
    c.full_box()?;
    c.skip(4)?;
    let handler = c.fourcc_field()?;
    let kind = match &handler.0 {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        b"tmcd" => TrackKind::Timecode,
        _ => TrackKind::Other,
    };
    let minf = require(&mk, mdia.fourcc, b"minf")?;
    let nk = children(minf.payload, minf.fourcc, depth + 2, ctx)?;
    let stbl = require(&nk, minf.fourcc, b"stbl")?;
    let sk = children(stbl.payload, STBL, depth + 3, ctx)?;
    let stsd = require(&sk, STBL, b"stsd")?;
    let entries = parse_stsd(stsd.payload, handler, depth + 4, ctx)?;
    let table = parse_tables(&sk, id, entries.len(), ctx)?;

    Ok(Track {
        id,
        kind,
        handler,
        enabled: flags & 1 != 0,
        tkhd_width,
        tkhd_height,
        tkhd_duration,
        timescale,
        duration,
        language,
        edits,
        entries,
        trex: None,
        table,
        runs: Vec::new(),
        fragment_samples: 0,
        next_dts: 0,
    })
}

fn parse_elst(b: BoxRef<'_>, ctx: &Ctx) -> Result<EditList> {
    let mut c = table(b, ctx)?;
    let v = full_box_v0(&mut c)?;
    let count = c.u32()? as u64;
    let n = match v {
        0 => c.entries(count, 12, &ctx.limits)?,
        1 => c.entries(count, 20, &ctx.limits)?,
        v => return Err(Error::UnsupportedVersion { fourcc: b.fourcc, version: v }),
    };
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let (segment_duration, media_time) = if v == 1 { (c.u64()?, c.i64()?) } else { (c.u32()? as u64, c.i32()? as i64) };
        if media_time < -1 {
            return Err(Error::Invalid { fourcc: b.fourcc, what: "media_time below -1" });
        }
        entries.push(Edit { segment_duration, media_time, media_rate: c.i32()? });
    }
    Ok(EditList { entries })
}

fn one_of<'a>(kids: &[BoxRef<'a>], a: &[u8; 4], b: &[u8; 4]) -> Result<BoxRef<'a>> {
    match (find_one(kids, a)?, find_one(kids, b)?) {
        (Some(x), None) | (None, Some(x)) => Ok(x),
        (Some(_), Some(y)) => Err(Error::Duplicate(y.fourcc)),
        (None, None) => Err(Error::Missing { parent: STBL, child: FourCC(*a) }),
    }
}

fn parse_tables(sk: &[BoxRef<'_>], track_id: u32, n_entries: usize, ctx: &mut Ctx) -> Result<SampleTable> {
    let lim = ctx.limits;
    let mismatch = |what| Error::TableMismatch { track_id, what };

    // stsz / stz2
    let sz = one_of(sk, b"stsz", b"stz2")?;
    let mut c = table(sz, ctx)?;
    full_box_v0(&mut c)?;
    let (sizes, count) = if sz.fourcc.0 == *b"stsz" {
        let size = c.u32()?;
        let count = c.u32()? as u64;
        if count > lim.max_samples_per_track {
            return Err(Error::LimitReached(Limit::TrackSamples));
        }
        if size != 0 {
            (Sizes::Constant(size), count)
        } else {
            let n = c.entries(count, 4, &lim)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(c.u32()?);
            }
            (Sizes::Table(v), count)
        }
    } else {
        c.skip(3)?;
        let field = c.u8()?;
        let count = c.u32()? as u64;
        if count > lim.max_samples_per_track {
            return Err(Error::LimitReached(Limit::TrackSamples));
        }
        let bytes = match field {
            4 => count.div_ceil(2),
            8 => count,
            16 => count * 2,
            _ => return Err(Error::Invalid { fourcc: sz.fourcc, what: "field_size is not 4, 8 or 16" }),
        };
        if bytes > c.remaining() as u64 {
            return Err(Error::Truncated(sz.fourcc));
        }
        let raw = c.bytes(bytes as usize)?;
        let mut v = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            v.push(match field {
                4 => {
                    let b = raw[i / 2];
                    (if i % 2 == 0 { b >> 4 } else { b & 0x0f }) as u32
                }
                8 => raw[i] as u32,
                _ => u16::from_be_bytes([raw[2 * i], raw[2 * i + 1]]) as u32,
            });
        }
        (Sizes::Table(v), count)
    };

    // stts
    let b = require(sk, STBL, b"stts")?;
    let mut c = table(b, ctx)?;
    full_box_v0(&mut c)?;
    let ec = c.u32()? as u64;
    let n = c.entries(ec, 8, &lim)?;
    let mut stts = Vec::with_capacity(n);
    let mut total = 0u64;
    for _ in 0..n {
        let (k, d) = (c.u32()?, c.u32()?);
        total += k as u64;
        stts.push((k, d));
    }
    if total != count {
        return Err(mismatch("stts sample count is not stsz's"));
    }

    // ctts (optional; v0 is read as signed too, as writers use it so)
    let ctts = match find_one(sk, b"ctts")? {
        Some(b) => {
            let mut c = table(b, ctx)?;
            let v = full_box_v0(&mut c)?;
            if v > 1 {
                return Err(Error::UnsupportedVersion { fourcc: b.fourcc, version: v });
            }
            let ec = c.u32()? as u64;
            let n = c.entries(ec, 8, &lim)?;
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                out.push((c.u32()?, c.i32()?));
            }
            out
        }
        None => Vec::new(),
    };

    // stss (optional: absent means every sample is a sync sample)
    let stss = match find_one(sk, b"stss")? {
        Some(b) => {
            let mut c = table(b, ctx)?;
            full_box_v0(&mut c)?;
            let ec = c.u32()? as u64;
            let n = c.entries(ec, 4, &lim)?;
            let mut out: Vec<u32> = Vec::with_capacity(n);
            for _ in 0..n {
                let s = c.u32()?;
                if s == 0 || s as u64 > count || out.last().is_some_and(|&p| s <= p) {
                    return Err(mismatch("stss entries not increasing within the sample count"));
                }
                out.push(s);
            }
            Some(out)
        }
        None => None,
    };

    // stco / co64
    let co = one_of(sk, b"stco", b"co64")?;
    let mut c = table(co, ctx)?;
    full_box_v0(&mut c)?;
    let ec = c.u32()? as u64;
    let wide = co.fourcc.0 == *b"co64";
    let n = c.entries(ec, if wide { 8 } else { 4 }, &lim)?;
    let mut chunks = Vec::with_capacity(n);
    for _ in 0..n {
        chunks.push(if wide { c.u64()? } else { c.u32()? as u64 });
    }

    // stsc
    let b = require(sk, STBL, b"stsc")?;
    let mut c = table(b, ctx)?;
    full_box_v0(&mut c)?;
    let ec = c.u32()? as u64;
    let n = c.entries(ec, 12, &lim)?;
    let mut stsc: Vec<Stsc> = Vec::with_capacity(n);
    for _ in 0..n {
        let e = Stsc { first_chunk: c.u32()?, samples_per_chunk: c.u32()?, description_index: c.u32()? };
        let ok_first = match stsc.last() {
            None => e.first_chunk == 1,
            Some(p) => e.first_chunk > p.first_chunk,
        };
        if !ok_first || e.first_chunk as u64 > chunks.len() as u64 {
            return Err(mismatch("stsc first_chunk not increasing from 1 within the chunk count"));
        }
        if e.description_index == 0 || e.description_index as usize > n_entries {
            return Err(mismatch("stsc sample_description_index out of range"));
        }
        stsc.push(e);
    }
    // The samples the chunk map holds must be the sample count.
    let mut mapped: u128 = 0;
    for (i, e) in stsc.iter().enumerate() {
        let next = stsc.get(i + 1).map_or(chunks.len() as u64 + 1, |n| n.first_chunk as u64);
        mapped += (next - e.first_chunk as u64) as u128 * e.samples_per_chunk as u128;
    }
    if mapped != count as u128 {
        return Err(mismatch("stsc maps a different number of samples than stsz holds"));
    }
    Ok(SampleTable { stts, ctts, stss, stsc, sizes, chunks, count })
}

/// Checks every moov-table sample lies inside the file and every time fits.
pub(crate) fn validate_table(t: &Track, file_len: u64) -> Result<()> {
    let mut it = TableIter::new(&t.table);
    let mut n = 0u64;
    while let Some(s) = it.next(&t.table) {
        let end = s.offset.checked_add(s.size as u64).ok_or(Error::SampleOutOfFile { track_id: t.id, sample: s.index })?;
        if end > file_len {
            return Err(Error::SampleOutOfFile { track_id: t.id, sample: s.index });
        }
        s.dts.checked_add(s.duration as u64).filter(|&d| d <= i64::MAX as u64 / 2).ok_or(Error::Overflow(FourCC(*b"stts")))?;
        n += 1;
    }
    if n != t.table.count {
        return Err(Error::TableMismatch { track_id: t.id, what: "tables end before the sample count" });
    }
    Ok(())
}

/// The decode time after the last moov-table sample.
pub(crate) fn table_end_dts(t: &Track) -> u64 {
    t.table.stts.iter().fold(0u64, |a, &(k, d)| a.saturating_add(k as u64 * d as u64))
}
