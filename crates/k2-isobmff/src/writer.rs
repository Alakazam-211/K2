//! A CMAF-style fragmented MP4 writer for the proxy ladder: one init segment
//! (`ftyp` + `moov` with `mvex`), then media segments (`styp`? `sidx`?
//! `moof` + `mdat`). The output is a function of the input only: no clock,
//! no random ids, so the same input gives the same bytes.

use crate::config::{Av1Config, AvcConfig, OpusConfig};
use crate::entry::Colour;
use crate::error::{Error, Result};

/// The codec of a track, with the configuration the caller built (for AV1,
/// [`Av1Config::from_sequence_header`] on the stream's own header).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackCodec {
    Av1(Av1Config),
    Avc(AvcConfig),
    Opus(OpusConfig),
}

/// One track of the init segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackSetup {
    /// Non-zero and unique.
    pub track_id: u32,
    /// Ticks per second of this track's media (the source's timescale for
    /// video, 48,000 for Opus).
    pub timescale: u32,
    pub codec: TrackCodec,
    /// Coded size (video only).
    pub width: u16,
    pub height: u16,
    /// `pasp` (hSpacing, vSpacing), written when given.
    pub pixel_aspect: Option<(u32, u32)>,
    /// `colr`: only `Colour::Nclx` is written.
    pub colour: Option<Colour>,
    /// ISO-639-2/T code, e.g. `*b"und"`.
    pub language: [u8; 3],
}

impl TrackSetup {
    fn is_video(&self) -> bool {
        !matches!(self.codec, TrackCodec::Opus(_))
    }
}

/// The init segment's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitConfig {
    /// `mvhd` timescale (1,000 is usual).
    pub movie_timescale: u32,
    /// Seconds since 1904-01-01 for `mvhd`/`tkhd`/`mdhd`; 0 for none.
    pub creation_time: u64,
    pub tracks: Vec<TrackSetup>,
}

/// One sample of a fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentSample {
    pub duration: u32,
    pub size: u32,
    pub keyframe: bool,
    /// pts − dts, in the track's timescale.
    pub composition_offset: i32,
}

/// One track's part of a media segment.
#[derive(Clone, Copy, Debug)]
pub struct TrackFragment<'a> {
    pub track_id: u32,
    /// The first sample's decode time (`tfdt`).
    pub base_media_decode_time: u64,
    pub samples: &'a [FragmentSample],
    /// The samples' bytes, back to back; its length is the sizes' sum.
    pub data: &'a [u8],
}

/// Optional boxes of a media segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SegmentOptions {
    /// Start the segment with `styp` (major brand `cmfs`; `cmfs`, `msdh`).
    pub styp: bool,
    /// Write a `sidx` for the first track of the segment, referencing this
    /// segment's `moof` + `mdat`.
    pub sidx: bool,
}

/// Writes one init segment and any number of media segments, numbering
/// the fragments from 1.
#[derive(Clone, Debug)]
pub struct Fmp4Writer {
    config: InitConfig,
    next_sequence: u32,
}

/// A small big-endian box builder.
struct W {
    b: Vec<u8>,
}

impl W {
    fn new() -> W {
        W { b: Vec::new() }
    }
    fn u8(&mut self, v: u8) {
        self.b.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.b.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.b.extend_from_slice(v);
    }
    fn zeros(&mut self, n: usize) {
        self.b.resize(self.b.len() + n, 0);
    }
    fn begin(&mut self, fourcc: &[u8; 4]) -> usize {
        let at = self.b.len();
        self.u32(0);
        self.bytes(fourcc);
        at
    }
    fn full(&mut self, fourcc: &[u8; 4], version: u8, flags: u32) -> usize {
        let at = self.begin(fourcc);
        self.u32((version as u32) << 24 | (flags & 0x00ff_ffff));
        at
    }
    fn end(&mut self, at: usize) -> Result<()> {
        let size = u32::try_from(self.b.len() - at).map_err(|_| Error::Writer("box over 4 GiB"))?;
        self.b[at..at + 4].copy_from_slice(&size.to_be_bytes());
        Ok(())
    }
    fn matrix(&mut self) {
        for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            self.u32(v);
        }
    }
    fn time_fields(&mut self, wide: bool, t: u64) {
        if wide {
            self.u64(t);
            self.u64(t);
        } else {
            self.u32(t as u32);
            self.u32(t as u32);
        }
    }
}

fn language(l: &[u8; 3]) -> Result<u16> {
    let mut v = 0u16;
    for &c in l {
        if !c.is_ascii_lowercase() {
            return Err(Error::Writer("language must be three lowercase letters"));
        }
        v = (v << 5) | (c - 0x60) as u16;
    }
    Ok(v)
}

const SYNC_FLAGS: u32 = 0x0200_0000; // depends on no other sample
const NON_SYNC_FLAGS: u32 = 0x0101_0000; // depends on others, non-sync

impl Fmp4Writer {
    /// Checks the setup: at least one track, non-zero unique ids and
    /// timescales, a size for every video track.
    pub fn new(config: InitConfig) -> Result<Fmp4Writer> {
        if config.tracks.is_empty() {
            return Err(Error::Writer("no tracks"));
        }
        if config.movie_timescale == 0 {
            return Err(Error::Writer("movie timescale is 0"));
        }
        for (i, t) in config.tracks.iter().enumerate() {
            if t.track_id == 0 || config.tracks[..i].iter().any(|o| o.track_id == t.track_id) {
                return Err(Error::Writer("track ids must be non-zero and unique"));
            }
            if t.timescale == 0 {
                return Err(Error::Writer("track timescale is 0"));
            }
            if t.is_video() && (t.width == 0 || t.height == 0) {
                return Err(Error::Writer("video track without a size"));
            }
            if let Some((h, v)) = t.pixel_aspect {
                if h == 0 || v == 0 {
                    return Err(Error::Writer("pixel aspect has a 0"));
                }
            }
            language(&t.language)?;
            if let TrackCodec::Opus(o) = &t.codec {
                if o.output_channel_count == 0 {
                    return Err(Error::Writer("Opus with 0 channels"));
                }
                o.to_bytes()?;
            }
            if let TrackCodec::Avc(a) = &t.codec {
                a.to_bytes()?;
            }
        }
        Ok(Fmp4Writer { config, next_sequence: 1 })
    }

    /// The `mfhd` sequence number the next media segment gets.
    pub fn next_sequence_number(&self) -> u32 {
        self.next_sequence
    }

    /// `ftyp` + `moov` (`mvhd`, one `trak` per track with empty sample
    /// tables, `mvex` with a `trex` per track).
    pub fn init_segment(&self) -> Result<Vec<u8>> {
        let cfg = &self.config;
        let mut w = W::new();
        let f = w.begin(b"ftyp");
        w.bytes(b"iso6");
        w.u32(0);
        w.bytes(b"iso6");
        w.bytes(b"cmfc");
        if cfg.tracks.iter().any(|t| matches!(t.codec, TrackCodec::Av1(_))) {
            w.bytes(b"av01");
        }
        w.end(f)?;

        let wide = cfg.creation_time > u32::MAX as u64;
        let v = wide as u8;
        let moov = w.begin(b"moov");
        let mvhd = w.full(b"mvhd", v, 0);
        w.time_fields(wide, cfg.creation_time);
        w.u32(cfg.movie_timescale);
        if wide {
            w.u64(0);
        } else {
            w.u32(0);
        }
        w.u32(0x0001_0000);
        w.u16(0x0100);
        w.zeros(10);
        w.matrix();
        w.zeros(24);
        let max_id = cfg.tracks.iter().map(|t| t.track_id).max().unwrap_or(0);
        w.u32(max_id.saturating_add(1));
        w.end(mvhd)?;

        for t in &cfg.tracks {
            write_trak(&mut w, t, cfg.creation_time)?;
        }

        let mvex = w.begin(b"mvex");
        for t in &cfg.tracks {
            let trex = w.full(b"trex", 0, 0);
            w.u32(t.track_id);
            w.u32(1);
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.end(trex)?;
        }
        w.end(mvex)?;
        w.end(moov)?;
        Ok(w.b)
    }

    /// One media segment: `styp` and `sidx` if asked, `moof` (`mfhd`, a `traf` per
    /// fragment with `tfhd` (default-base-is-moof), `tfdt` v1 and a `trun`
    /// with each sample's duration, size, flags and composition offset),
    /// then one `mdat` with the fragments' data in order.
    pub fn media_segment(&mut self, fragments: &[TrackFragment<'_>], opts: SegmentOptions) -> Result<Vec<u8>> {
        if fragments.is_empty() {
            return Err(Error::Writer("a segment needs at least one track fragment"));
        }
        let mut total_data: u64 = 0;
        for (i, fr) in fragments.iter().enumerate() {
            if !self.config.tracks.iter().any(|t| t.track_id == fr.track_id) {
                return Err(Error::Writer("fragment for a track the init segment does not have"));
            }
            if fragments[..i].iter().any(|o| o.track_id == fr.track_id) {
                return Err(Error::Writer("two fragments for one track in a segment"));
            }
            if fr.samples.is_empty() {
                return Err(Error::Writer("a track fragment needs at least one sample"));
            }
            let sum: u64 = fr.samples.iter().map(|s| s.size as u64).sum();
            if sum != fr.data.len() as u64 {
                return Err(Error::Writer("sample sizes do not add up to the data length"));
            }
            total_data += sum;
        }
        let seq = self.next_sequence;
        let mut w = W::new();
        if opts.styp {
            let s = w.begin(b"styp");
            w.bytes(b"cmfs");
            w.u32(0);
            w.bytes(b"cmfs");
            w.bytes(b"msdh");
            w.end(s)?;
        }
        let sidx_at = if opts.sidx {
            // Filled in once the moof and mdat sizes are known.
            let at = w.full(b"sidx", 1, 0);
            w.zeros(4 + 4 + 8 + 8 + 2 + 2 + 12);
            w.end(at)?;
            Some(at)
        } else {
            None
        };

        let moof_at = w.b.len();
        let moof = w.begin(b"moof");
        let mfhd = w.full(b"mfhd", 0, 0);
        w.u32(seq);
        w.end(mfhd)?;
        let mut patches = Vec::with_capacity(fragments.len());
        for fr in fragments {
            let traf = w.begin(b"traf");
            let tfhd = w.full(b"tfhd", 0, 0x02_0000);
            w.u32(fr.track_id);
            w.end(tfhd)?;
            let tfdt = w.full(b"tfdt", 1, 0);
            w.u64(fr.base_media_decode_time);
            w.end(tfdt)?;
            let any_cto = fr.samples.iter().any(|s| s.composition_offset != 0);
            let neg = fr.samples.iter().any(|s| s.composition_offset < 0);
            let flags = 0x01 | 0x100 | 0x200 | 0x400 | if any_cto { 0x800 } else { 0 };
            let trun = w.full(b"trun", neg as u8, flags);
            w.u32(u32::try_from(fr.samples.len()).map_err(|_| Error::Writer("too many samples"))?);
            patches.push(w.b.len());
            w.u32(0);
            for s in fr.samples {
                w.u32(s.duration);
                w.u32(s.size);
                w.u32(if s.keyframe { SYNC_FLAGS } else { NON_SYNC_FLAGS });
                if any_cto {
                    w.u32(s.composition_offset as u32);
                }
            }
            w.end(trun)?;
            w.end(traf)?;
        }
        w.end(moof)?;
        let moof_size = (w.b.len() - moof_at) as u64;

        let large = total_data + 8 > u32::MAX as u64;
        let mdat_header: u64 = if large { 16 } else { 8 };
        let mut data_start = moof_size + mdat_header;
        for (fr, &p) in fragments.iter().zip(&patches) {
            let off = i32::try_from(data_start).map_err(|_| Error::Writer("data offset over 2 GiB"))?;
            w.b[p..p + 4].copy_from_slice(&off.to_be_bytes());
            data_start += fr.data.len() as u64;
        }
        if large {
            w.u32(1);
            w.bytes(b"mdat");
            w.u64(total_data + 16);
        } else {
            w.u32((total_data + 8) as u32);
            w.bytes(b"mdat");
        }
        for fr in fragments {
            w.bytes(fr.data);
        }

        if let Some(at) = sidx_at {
            let fr = &fragments[0];
            let ts = self.config.tracks.iter().find(|t| t.track_id == fr.track_id).map_or(1, |t| t.timescale);
            let mut dts = fr.base_media_decode_time as i128;
            let mut earliest = i128::MAX;
            let mut dur: u64 = 0;
            for s in fr.samples {
                earliest = earliest.min(dts + s.composition_offset as i128);
                dts += s.duration as i128;
                dur += s.duration as u64;
            }
            let referenced = moof_size + mdat_header + total_data;
            let referenced = u32::try_from(referenced).ok().filter(|&r| r < 0x8000_0000).ok_or(Error::Writer("segment over 2 GiB for sidx"))?;
            let dur = u32::try_from(dur).map_err(|_| Error::Writer("segment duration over 32 bits for sidx"))?;
            let mut s = W::new();
            s.u32(fr.track_id);
            s.u32(ts);
            s.u64(earliest.clamp(0, u64::MAX as i128) as u64);
            s.u64(0);
            s.u16(0);
            s.u16(1);
            s.u32(referenced);
            s.u32(dur);
            let key = fr.samples[0].keyframe;
            s.u32(if key { 0x9000_0000 } else { 0 });
            let body = at + 12;
            w.b[body..body + s.b.len()].copy_from_slice(&s.b);
        }
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        Ok(w.b)
    }
}

fn write_trak(w: &mut W, t: &TrackSetup, creation: u64) -> Result<()> {
    let wide = creation > u32::MAX as u64;
    let v = wide as u8;
    let video = t.is_video();
    let trak = w.begin(b"trak");
    let tkhd = w.full(b"tkhd", v, 0x03);
    w.time_fields(wide, creation);
    w.u32(t.track_id);
    w.u32(0);
    if wide {
        w.u64(0);
    } else {
        w.u32(0);
    }
    w.zeros(8);
    w.u16(0); // layer
    w.u16(0); // alternate group
    w.u16(if video { 0 } else { 0x0100 });
    w.u16(0);
    w.matrix();
    if video {
        let (h, vs) = t.pixel_aspect.unwrap_or((1, 1));
        let dw = ((t.width as u64) << 16) * h as u64 / vs as u64;
        w.u32(dw.min(u32::MAX as u64) as u32);
        w.u32((t.height as u32) << 16);
    } else {
        w.u32(0);
        w.u32(0);
    }
    w.end(tkhd)?;

    let mdia = w.begin(b"mdia");
    let mdhd = w.full(b"mdhd", v, 0);
    w.time_fields(wide, creation);
    w.u32(t.timescale);
    if wide {
        w.u64(0);
    } else {
        w.u32(0);
    }
    w.u16(language(&t.language)?);
    w.u16(0);
    w.end(mdhd)?;
    let hdlr = w.full(b"hdlr", 0, 0);
    w.u32(0);
    w.bytes(if video { b"vide" } else { b"soun" });
    w.zeros(12);
    w.bytes(if video { b"VideoHandler\0" } else { b"SoundHandler\0" });
    w.end(hdlr)?;

    let minf = w.begin(b"minf");
    if video {
        let vmhd = w.full(b"vmhd", 0, 1);
        w.zeros(8);
        w.end(vmhd)?;
    } else {
        let smhd = w.full(b"smhd", 0, 0);
        w.zeros(4);
        w.end(smhd)?;
    }
    let dinf = w.begin(b"dinf");
    let dref = w.full(b"dref", 0, 0);
    w.u32(1);
    let url = w.full(b"url ", 0, 1);
    w.end(url)?;
    w.end(dref)?;
    w.end(dinf)?;

    let stbl = w.begin(b"stbl");
    let stsd = w.full(b"stsd", 0, 0);
    w.u32(1);
    write_entry(w, t)?;
    w.end(stsd)?;
    for name in [b"stts", b"stsc", b"stco"] {
        let b = w.full(name, 0, 0);
        w.u32(0);
        w.end(b)?;
    }
    let stsz = w.full(b"stsz", 0, 0);
    w.u32(0);
    w.u32(0);
    w.end(stsz)?;
    w.end(stbl)?;
    w.end(minf)?;
    w.end(mdia)?;
    w.end(trak)?;
    Ok(())
}

fn write_entry(w: &mut W, t: &TrackSetup) -> Result<()> {
    match &t.codec {
        TrackCodec::Av1(c) => visual(w, t, b"av01", b"av1C", &c.to_bytes()),
        TrackCodec::Avc(c) => visual(w, t, b"avc1", b"avcC", &c.to_bytes()?),
        TrackCodec::Opus(c) => {
            let e = w.begin(b"Opus");
            w.zeros(6);
            w.u16(1);
            w.zeros(8);
            w.u16(c.output_channel_count as u16);
            w.u16(16);
            w.u16(0);
            w.u16(0);
            w.u32(48000 << 16);
            let d = w.begin(b"dOps");
            w.bytes(&c.to_bytes()?);
            w.end(d)?;
            w.end(e)
        }
    }
}

fn visual(w: &mut W, t: &TrackSetup, fourcc: &[u8; 4], cfg_name: &[u8; 4], cfg: &[u8]) -> Result<()> {
    let e = w.begin(fourcc);
    w.zeros(6);
    w.u16(1);
    w.zeros(16);
    w.u16(t.width);
    w.u16(t.height);
    w.u32(0x0048_0000);
    w.u32(0x0048_0000);
    w.u32(0);
    w.u16(1);
    w.zeros(32);
    w.u16(0x0018);
    w.u16(0xffff);
    let c = w.begin(cfg_name);
    w.bytes(cfg);
    w.end(c)?;
    if let Some(Colour::Nclx { primaries, transfer, matrix, full_range }) = &t.colour {
        let b = w.begin(b"colr");
        w.bytes(b"nclx");
        w.u16(*primaries);
        w.u16(*transfer);
        w.u16(*matrix);
        w.u8(if full_range.unwrap_or(false) { 0x80 } else { 0 });
        w.end(b)?;
    }
    if let Some((h, v)) = t.pixel_aspect {
        let b = w.begin(b"pasp");
        w.u32(h);
        w.u32(v);
        w.end(b)?;
    }
    w.end(e)
}
