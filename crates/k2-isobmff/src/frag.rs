//! Movie fragments (ISO/IEC 14496-12 §8.8): `mvex`/`trex`, `moof`/`mfhd`/
//! `traf`/`tfhd`/`tfdt`/`trun`, and the segment index `sidx` (§8.16.3).

use crate::bytes::{children, find_one, require, BoxRef, Ctx, Cur};
use crate::error::{Error, FourCC, Limit, Result};
use crate::track::{Run, RunEntry, Track, TrackExtends};

const MOOF: FourCC = FourCC(*b"moof");
const TRUN: FourCC = FourCC(*b"trun");

/// Reads `mvex` and attaches each `trex` to its track.
pub(crate) fn parse_mvex(b: BoxRef<'_>, depth: u32, ctx: &mut Ctx, tracks: &mut [Track]) -> Result<()> {
    let kids = children(b.payload, b.fourcc, depth, ctx)?;
    for k in kids.iter().filter(|k| k.fourcc.0 == *b"trex") {
        let mut c = Cur::new(k.payload, k.fourcc);
        c.full_box()?;
        let id = c.u32()?;
        let trex = TrackExtends {
            default_sample_description_index: c.u32()?,
            default_sample_duration: c.u32()?,
            default_sample_size: c.u32()?,
            default_sample_flags: c.u32()?,
        };
        let t = tracks.iter_mut().find(|t| t.id == id).ok_or(Error::UnknownTrack(id))?;
        if t.trex.is_some() {
            return Err(Error::Duplicate(k.fourcc));
        }
        t.trex = Some(trex);
    }
    Ok(())
}

/// Running totals checked against the limits across all fragments.
pub(crate) struct SampleBudget {
    pub total: u64,
}

/// Parses one `moof` (its payload, read into memory) that starts at
/// `moof_offset` in the file, and appends its runs to the tracks.
pub(crate) fn parse_moof(payload: &[u8], moof_offset: u64, depth: u32, ctx: &mut Ctx, tracks: &mut [Track], file_len: u64, budget: &mut SampleBudget) -> Result<()> {
    let kids = children(payload, MOOF, depth, ctx)?;
    require(&kids, MOOF, b"mfhd")?;
    let mut prev_end = moof_offset;
    for traf in kids.iter().filter(|k| k.fourcc.0 == *b"traf") {
        let tk = children(traf.payload, traf.fourcc, depth + 1, ctx)?;
        let tfhd = require(&tk, traf.fourcc, b"tfhd")?;
        let mut c = Cur::new(tfhd.payload, tfhd.fourcc);
        let (_, flags) = c.full_box()?;
        let id = c.u32()?;
        let base_data_offset = if flags & 0x01 != 0 { Some(c.u64()?) } else { None };
        let ti = tracks.iter().position(|t| t.id == id).ok_or(Error::UnknownTrack(id))?;
        let trex = tracks[ti].trex.ok_or(Error::Missing { parent: FourCC(*b"mvex"), child: FourCC(*b"trex") })?;
        let sdi = if flags & 0x02 != 0 { c.u32()? } else { trex.default_sample_description_index };
        let def_dur = if flags & 0x08 != 0 { c.u32()? } else { trex.default_sample_duration };
        let def_size = if flags & 0x10 != 0 { c.u32()? } else { trex.default_sample_size };
        let def_flags = if flags & 0x20 != 0 { c.u32()? } else { trex.default_sample_flags };
        if sdi == 0 || sdi as usize > tracks[ti].entries.len() {
            return Err(Error::Invalid { fourcc: tfhd.fourcc, what: "sample_description_index out of range" });
        }
        let base = match base_data_offset {
            Some(b) => b,
            // default-base-is-moof, or the first traf of a moof.
            None if flags & 0x02_0000 != 0 => moof_offset,
            None => prev_end,
        };
        let mut dts = match find_one(&tk, b"tfdt")? {
            Some(t) => {
                let mut c = Cur::new(t.payload, t.fourcc);
                let (v, _) = c.full_box()?;
                c.u32_or_u64(v)?
            }
            None => tracks[ti].next_dts,
        };
        let mut cursor = base;
        for tr in tk.iter().filter(|k| k.fourcc.0 == *b"trun") {
            let mut c = Cur::new(tr.payload, TRUN);
            let (v, tf) = c.full_box()?;
            if v > 1 {
                return Err(Error::UnsupportedVersion { fourcc: TRUN, version: v });
            }
            let count = c.u32()?;
            let start = if tf & 0x01 != 0 {
                let off = c.i32()? as i64;
                base.checked_add_signed(off).ok_or(Error::Invalid { fourcc: TRUN, what: "data_offset before the file start" })?
            } else {
                cursor
            };
            let first_flags = if tf & 0x04 != 0 { Some(c.u32()?) } else { None };
            let present = ((tf >> 8) & 0x0f) as u8;
            let per = present.count_ones() as u64 * 4;
            let t = &mut tracks[ti];
            let after = t.sample_count().checked_add(count as u64).ok_or(Error::LimitReached(Limit::TrackSamples))?;
            if after > ctx.limits.max_samples_per_track {
                return Err(Error::LimitReached(Limit::TrackSamples));
            }
            if budget.total + count as u64 > ctx.limits.max_total_samples {
                return Err(Error::LimitReached(Limit::TotalSamples));
            }
            let n = if per > 0 { c.entries(count as u64, per, &ctx.limits)? } else { 0 };
            let mut entries = Vec::with_capacity(n);
            for _ in 0..n {
                let mut e = RunEntry::default();
                if present & 1 != 0 {
                    e.duration = c.u32()?;
                }
                if present & 2 != 0 {
                    e.size = c.u32()?;
                }
                if present & 4 != 0 {
                    e.flags = c.u32()?;
                }
                if present & 8 != 0 {
                    e.cto = c.i32()?;
                }
                entries.push(e);
            }
            let run = Run {
                base_dts: dts,
                data_offset: start,
                count,
                default_size: def_size,
                default_duration: def_dur,
                default_flags: def_flags,
                first_flags,
                present,
                entries,
                description_index: sdi,
            };
            // Where the run's bytes and times end, checked.
            // Per-sample durations or sizes are in memory (one entry per
            // sample); defaults only, the totals are products.
            let (bytes, ticks) = if present & 3 == 0 {
                (def_size as u64 * count as u64, def_dur as u64 * count as u64)
            } else {
                (0..count as usize).fold((0u64, 0u64), |(b, t), j| {
                    let (d, s, _, _) = run.sample(j);
                    (b + s as u64, t + d as u64)
                })
            };
            let end = start.checked_add(bytes).ok_or(Error::Overflow(TRUN))?;
            if end > file_len {
                return Err(Error::PastEndOfFile { fourcc: TRUN, end, file_len });
            }
            dts = dts.checked_add(ticks).filter(|&d| d <= i64::MAX as u64 / 2).ok_or(Error::Overflow(TRUN))?;
            cursor = end;
            t.fragment_samples += count as u64;
            budget.total += count as u64;
            t.runs.push(run);
        }
        tracks[ti].next_dts = dts;
        prev_end = cursor;
    }
    Ok(())
}

/// One reference in a segment index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidxReference {
    /// true: the reference points at another `sidx`.
    pub reference_type: bool,
    pub referenced_size: u32,
    pub subsegment_duration: u32,
    pub starts_with_sap: bool,
    pub sap_type: u8,
    pub sap_delta_time: u32,
}

/// A `sidx` box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentIndex {
    /// Where the `sidx` box starts in the file, and its whole size.
    pub offset: u64,
    pub size: u64,
    pub reference_id: u32,
    pub timescale: u32,
    pub earliest_presentation_time: u64,
    /// Bytes from the end of this box to the first referenced byte.
    pub first_offset: u64,
    pub references: Vec<SidxReference>,
}

pub(crate) fn parse_sidx(payload: &[u8], offset: u64, size: u64, ctx: &Ctx) -> Result<SegmentIndex> {
    let mut c = Cur::new(payload, FourCC(*b"sidx"));
    let (v, _) = c.full_box()?;
    let reference_id = c.u32()?;
    let timescale = c.u32()?;
    let earliest_presentation_time = c.u32_or_u64(v)?;
    let first_offset = c.u32_or_u64(v)?;
    c.skip(2)?;
    let n = c.u16()? as u64;
    let n = c.entries(n, 12, &ctx.limits)?;
    let mut references = Vec::with_capacity(n);
    for _ in 0..n {
        let a = c.u32()?;
        let d = c.u32()?;
        let s = c.u32()?;
        references.push(SidxReference {
            reference_type: a >> 31 != 0,
            referenced_size: a & 0x7fff_ffff,
            subsegment_duration: d,
            starts_with_sap: s >> 31 != 0,
            sap_type: ((s >> 28) & 7) as u8,
            sap_delta_time: s & 0x0fff_ffff,
        });
    }
    Ok(SegmentIndex { offset, size, reference_id, timescale, earliest_presentation_time, first_offset, references })
}
