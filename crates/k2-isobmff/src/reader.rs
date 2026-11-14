//! The file reader: walks the top-level boxes of a `Read + Seek` input by
//! seeking (so `moov` may come first or last), loads `ftyp`, `moov`, each
//! `moof` and `sidx` into memory within the limits, and skips `mdat`.

use crate::bytes::{children, find_one, require, Ctx, Cur};
use crate::entry::Codec;
use crate::error::{Error, FourCC, Limit, Limits, Result};
use crate::frag::{parse_mvex, parse_moof, parse_sidx, SampleBudget, SegmentIndex};
use crate::timecode::Timecode;
use crate::track::{parse_trak, table_end_dts, validate_table, Sample, Track, TrackKind};
use std::io::{Read, Seek, SeekFrom};

/// `ftyp` (or the first `styp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileType {
    pub major_brand: FourCC,
    pub minor_version: u32,
    pub compatible_brands: Vec<FourCC>,
}

/// What the movie box says, with its tracks.
#[derive(Clone, Debug)]
pub struct Movie {
    pub file_type: Option<FileType>,
    /// A QuickTime file: major brand `qt  `, or no `ftyp` at all.
    pub quicktime: bool,
    /// Where the `moov` box starts in the file.
    pub moov_offset: u64,
    pub creation_time: u64,
    pub modification_time: u64,
    /// `mvhd` timescale and duration.
    pub timescale: u32,
    pub duration: u64,
    pub next_track_id: u32,
    /// The movie has `mvex` (it may be followed by fragments).
    pub fragmented: bool,
    /// `moof` boxes read.
    pub fragment_count: u64,
    pub tracks: Vec<Track>,
    /// Top-level `sidx` boxes, in file order.
    pub segment_indexes: Vec<SegmentIndex>,
}

impl Movie {
    /// The track with this `track_ID`.
    pub fn track(&self, id: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
}

/// A parsed file over its input. Opening reads and checks every structure;
/// afterwards only [`Mp4Reader::read_sample`] and
/// [`Mp4Reader::start_timecode`] touch the input.
pub struct Mp4Reader<R> {
    input: R,
    file_len: u64,
    limits: Limits,
    movie: Movie,
}

const FILE: FourCC = FourCC(*b"file");

fn read_at<R: Read + Seek>(r: &mut R, offset: u64, buf: &mut [u8]) -> Result<()> {
    r.seek(SeekFrom::Start(offset))?;
    r.read_exact(buf).map_err(|e| if e.kind() == std::io::ErrorKind::UnexpectedEof { Error::Truncated(FILE) } else { e.into() })
}

/// Reads a whole box payload into memory, refusing it before allocating
/// when it is over `limit`.
fn load<R: Read + Seek>(r: &mut R, fourcc: FourCC, offset: u64, len: u64, limit: u64) -> Result<Vec<u8>> {
    if len > limit {
        return Err(Error::TooLarge { fourcc, size: len, limit });
    }
    let mut buf = vec![0u8; len as usize];
    read_at(r, offset, &mut buf)?;
    Ok(buf)
}

fn parse_ftyp(p: &[u8], fourcc: FourCC, ctx: &Ctx) -> Result<FileType> {
    let mut c = Cur::new(p, fourcc);
    let major_brand = c.fourcc_field()?;
    let minor_version = c.u32()?;
    let n = c.remaining() / 4;
    c.entries(n as u64, 4, &ctx.limits)?;
    let mut compatible_brands = Vec::with_capacity(n);
    for _ in 0..n {
        compatible_brands.push(c.fourcc_field()?);
    }
    Ok(FileType { major_brand, minor_version, compatible_brands })
}

impl<R: Read + Seek> Mp4Reader<R> {
    /// Opens with [`Limits::default`].
    pub fn new(input: R) -> Result<Mp4Reader<R>> {
        Mp4Reader::with_limits(input, Limits::default())
    }

    /// Opens with the host's limits.
    pub fn with_limits(mut input: R, limits: Limits) -> Result<Mp4Reader<R>> {
        let file_len = input.seek(SeekFrom::End(0))?;
        let mut ctx = Ctx::new(limits);
        let mut file_type = None;
        let mut seen_ftyp = false;
        let mut moov: Option<(u64, Vec<u8>)> = None;
        let mut moofs: Vec<(u64, u64, u64)> = Vec::new(); // (box offset, payload offset, payload len)
        let mut sidx = Vec::new();
        let mut pos = 0u64;
        while pos < file_len {
            ctx.count_box()?;
            if file_len - pos < 8 {
                return Err(Error::Truncated(FILE));
            }
            let mut h = [0u8; 8];
            read_at(&mut input, pos, &mut h)?;
            let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
            let fourcc = FourCC([h[4], h[5], h[6], h[7]]);
            let mut header = 8u64;
            let size = match size32 {
                0 => file_len - pos,
                1 => {
                    if file_len - pos < 16 {
                        return Err(Error::Truncated(fourcc));
                    }
                    let mut l = [0u8; 8];
                    read_at(&mut input, pos + 8, &mut l)?;
                    header = 16;
                    u64::from_be_bytes(l)
                }
                s => s,
            };
            if fourcc.0 == *b"uuid" {
                header += 16;
            }
            if size < header {
                return Err(Error::BadBoxSize { fourcc, size });
            }
            let end = pos.checked_add(size).ok_or(Error::BadBoxSize { fourcc, size })?;
            if end > file_len {
                return Err(Error::PastEndOfFile { fourcc, end, file_len });
            }
            let (po, pl) = (pos + header, size - header);
            match &fourcc.0 {
                b"ftyp" | b"styp" => {
                    let p = load(&mut input, fourcc, po, pl, 4096)?;
                    let ft = parse_ftyp(&p, fourcc, &ctx)?;
                    if fourcc.0 == *b"ftyp" {
                        if seen_ftyp {
                            return Err(Error::Duplicate(fourcc));
                        }
                        seen_ftyp = true;
                        file_type = Some(ft);
                    } else if file_type.is_none() {
                        file_type = Some(ft);
                    }
                }
                b"moov" => {
                    if moov.is_some() {
                        return Err(Error::Duplicate(fourcc));
                    }
                    moov = Some((pos, load(&mut input, fourcc, po, pl, limits.max_box_bytes)?));
                }
                b"moof" => {
                    if moofs.len() as u64 >= limits.max_fragments {
                        return Err(Error::LimitReached(Limit::Fragments));
                    }
                    moofs.push((pos, po, pl));
                }
                b"sidx" => {
                    let p = load(&mut input, fourcc, po, pl, limits.max_box_bytes)?;
                    sidx.push(parse_sidx(&p, pos, size, &ctx)?);
                }
                _ => {}
            }
            pos = end;
        }
        let (moov_offset, moov) = moov.ok_or(Error::NoMovie)?;
        let quicktime = file_type.as_ref().is_none_or(|f| f.major_brand.0 == *b"qt  ");

        let mkids = children(&moov, FourCC(*b"moov"), 2, &mut ctx)?;
        let mvhd = require(&mkids, FourCC(*b"moov"), b"mvhd")?;
        let mut c = Cur::new(mvhd.payload, mvhd.fourcc);
        let (v, _) = c.full_box()?;
        let creation_time = c.u32_or_u64(v)?;
        let modification_time = c.u32_or_u64(v)?;
        let timescale = c.u32()?;
        let duration = c.u32_or_u64(v)?;
        c.skip(4 + 2 + 10 + 36 + 24)?;
        let next_track_id = c.u32()?;
        if timescale == 0 {
            return Err(Error::Invalid { fourcc: mvhd.fourcc, what: "timescale is 0" });
        }
        let mut tracks: Vec<Track> = Vec::new();
        let mut budget = SampleBudget { total: 0 };
        for k in mkids.iter().filter(|k| k.fourcc.0 == *b"trak") {
            if tracks.len() as u32 >= limits.max_tracks {
                return Err(Error::LimitReached(Limit::Tracks));
            }
            let t = parse_trak(*k, 3, &mut ctx)?;
            if tracks.iter().any(|o| o.id == t.id) {
                return Err(Error::Invalid { fourcc: FourCC(*b"tkhd"), what: "two tracks share a track_ID" });
            }
            budget.total += t.sample_count();
            if budget.total > limits.max_total_samples {
                return Err(Error::LimitReached(Limit::TotalSamples));
            }
            validate_table(&t, file_len)?;
            tracks.push(t);
        }
        for t in &mut tracks {
            t.next_dts = table_end_dts(t);
        }
        let fragmented = match find_one(&mkids, b"mvex")? {
            Some(m) => {
                parse_mvex(m, 3, &mut ctx, &mut tracks)?;
                true
            }
            None => false,
        };
        if !moofs.is_empty() && !fragmented {
            return Err(Error::Invalid { fourcc: FourCC(*b"moof"), what: "movie fragment without mvex" });
        }
        for &(off, po, pl) in &moofs {
            let p = load(&mut input, FourCC(*b"moof"), po, pl, limits.max_box_bytes)?;
            parse_moof(&p, off, 2, &mut ctx, &mut tracks, file_len, &mut budget)?;
        }
        let movie = Movie {
            file_type,
            quicktime,
            moov_offset,
            creation_time,
            modification_time,
            timescale,
            duration,
            next_track_id,
            fragmented,
            fragment_count: moofs.len() as u64,
            tracks,
            segment_indexes: sidx,
        };
        Ok(Mp4Reader { input, file_len, limits, movie })
    }

    /// The parsed movie.
    pub fn movie(&self) -> &Movie {
        &self.movie
    }

    /// The input's length in bytes.
    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    /// Reads one sample's bytes into `buf` (replacing its contents).
    pub fn read_sample(&mut self, sample: &Sample, buf: &mut Vec<u8>) -> Result<()> {
        if sample.size as u64 > self.limits.max_sample_bytes {
            return Err(Error::TooLarge { fourcc: FourCC(*b"mdat"), size: sample.size as u64, limit: self.limits.max_sample_bytes });
        }
        if sample.offset.checked_add(sample.size as u64).is_none_or(|e| e > self.file_len) {
            return Err(Error::PastEndOfFile { fourcc: FourCC(*b"mdat"), end: sample.offset.saturating_add(sample.size as u64), file_len: self.file_len });
        }
        buf.clear();
        buf.resize(sample.size as usize, 0);
        read_at(&mut self.input, sample.offset, buf)
    }

    /// The start timecode of a `tmcd` track: its first sample, a 32-bit
    /// frame number, converted with the entry's rate and flags. `None` when
    /// the track is not a timecode track or has no samples.
    pub fn start_timecode(&mut self, track_id: u32) -> Result<Option<Timecode>> {
        let Some(t) = self.movie.track(track_id) else { return Ok(None) };
        if t.kind != TrackKind::Timecode && t.handler.0 != *b"tmcd" {
            return Ok(None);
        }
        let Some(s) = t.samples().next() else { return Ok(None) };
        let entry = match t.entries.get((s.description_index as usize).wrapping_sub(1)).map(|e| &e.codec) {
            Some(Codec::Timecode(e)) => *e,
            _ => return Ok(None),
        };
        if s.size < 4 {
            return Err(Error::Invalid { fourcc: FourCC(*b"tmcd"), what: "timecode sample shorter than 4 bytes" });
        }
        let mut b = [0u8; 4];
        read_at(&mut self.input, s.offset, &mut b)?;
        let raw = u32::from_be_bytes(b);
        let frame = if entry.flags & 4 != 0 { raw as i32 as i64 } else { raw as i64 };
        Ok(Some(Timecode::from_frame_number(frame, &entry)))
    }

    /// Gives the input back.
    pub fn into_inner(self) -> R {
        self.input
    }
}
