//! Sample entries in `stsd` (ISO/IEC 14496-12 §8.5.2, §12.1.3, §12.2.3;
//! QuickTime File Format "Sample description atoms" for video, sound and
//! timecode).

use crate::bytes::{children, find_one, require, BoxRef, Ctx, Cur};
use crate::config::{AacConfig, Av1Config, AvcConfig, HevcConfig, OpusConfig};
use crate::error::{Error, FourCC, Result};
use crate::timecode::TimecodeEntry;

/// The ProRes flavour named by the sample entry's four-cc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProRes {
    /// `apco`
    Proxy422,
    /// `apcs`
    Lt422,
    /// `apcn`
    Standard422,
    /// `apch`
    Hq422,
    /// `ap4h`
    P4444,
    /// `ap4x`
    Xq4444,
}

impl ProRes {
    fn from_fourcc(f: &[u8; 4]) -> Option<ProRes> {
        Some(match f {
            b"apco" => ProRes::Proxy422,
            b"apcs" => ProRes::Lt422,
            b"apcn" => ProRes::Standard422,
            b"apch" => ProRes::Hq422,
            b"ap4h" => ProRes::P4444,
            b"ap4x" => ProRes::Xq4444,
            _ => return None,
        })
    }
}

/// Raw PCM layout, from the sample entry (and `pcmC` for `ipcm`/`fpcm`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pcm {
    pub float: bool,
    pub big_endian: bool,
    pub signed: bool,
    pub bits_per_sample: u16,
}

/// What the sample entry carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Codec {
    /// `avc1`/`avc3`.
    Avc(AvcConfig),
    /// `hvc1`/`hev1`.
    Hevc(HevcConfig),
    /// `av01`.
    Av1(Av1Config),
    /// A ProRes four-cc: identified only.
    ProRes(ProRes),
    /// `Opus`.
    Opus(OpusConfig),
    /// `mp4a` (AAC and the other MPEG-4 audio object types).
    Mp4Audio(AacConfig),
    /// `lpcm`, `sowt`, `twos`, `ipcm`, `fpcm` (and QuickTime `raw `, `in24`,
    /// `in32`, `fl32`, `fl64`).
    Pcm(Pcm),
    /// `tmcd`.
    Timecode(TimecodeEntry),
    /// Anything else: the entry is kept with its four-cc.
    Unknown,
}

/// The colour box (`colr`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Colour {
    /// `nclx` (ISO, with the range flag) or `nclc` (QuickTime, without).
    Nclx { primaries: u16, transfer: u16, matrix: u16, full_range: Option<bool> },
    /// `rICC` or `prof`: an ICC profile of this many bytes.
    Icc { restricted: bool, len: usize },
    /// Another colour type.
    Other(FourCC),
}

/// The visual fields of a video sample entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoInfo {
    pub width: u16,
    pub height: u16,
    /// The entry's depth field (24, or 32 with alpha).
    pub depth: u16,
    /// `pasp`: (hSpacing, vSpacing).
    pub pixel_aspect: Option<(u32, u32)>,
    pub colour: Option<Colour>,
}

/// The audio fields of a sound sample entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioInfo {
    pub channels: u32,
    /// Hz. From the 16.16 field, or the QuickTime v2 double.
    pub sample_rate: u32,
    /// The entry's sample size field (bits).
    pub sample_size: u16,
    /// The QuickTime sound description version (0, 1, 2), or 0 for ISO.
    pub qt_version: u16,
}

/// One entry of `stsd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleEntry {
    pub fourcc: FourCC,
    pub data_reference_index: u16,
    pub codec: Codec,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    /// Bits per sample where the configuration says it (`avcC` High
    /// extension, `hvcC`, `av1C`, PCM).
    pub bit_depth: Option<u8>,
    /// `encv`/`enca`: the original format from `sinf`/`frma`.
    pub protected_original: Option<FourCC>,
}

const STSD: FourCC = FourCC(*b"stsd");

fn is_video(f: &[u8; 4]) -> bool {
    matches!(f, b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"av01" | b"encv" | b"mp4v" | b"vp08" | b"vp09" | b"jpeg" | b"mjpa" | b"mjpb")
        || ProRes::from_fourcc(f).is_some()
}

fn is_audio(f: &[u8; 4]) -> bool {
    matches!(
        f,
        b"Opus" | b"mp4a" | b"lpcm" | b"sowt" | b"twos" | b"ipcm" | b"fpcm" | b"enca" | b"raw " | b"in24" | b"in32" | b"fl32" | b"fl64" | b"ac-3" | b"ec-3" | b"alac" | b"fLaC"
    )
}

/// Parses `stsd` (payload after the box header).
pub(crate) fn parse_stsd(data: &[u8], handler: FourCC, depth: u32, ctx: &mut Ctx) -> Result<Vec<SampleEntry>> {
    let mut c = Cur::new(data, STSD);
    let (version, _) = c.full_box()?;
    if version > 1 {
        return Err(Error::UnsupportedVersion { fourcc: STSD, version });
    }
    let count = c.u32()? as u64;
    // Every entry is at least a box header plus the 8-byte SampleEntry.
    c.entries(count, 16, &ctx.limits)?;
    let kids = children(c.rest(), STSD, depth, ctx)?;
    if kids.len() as u64 != count {
        return Err(Error::Invalid { fourcc: STSD, what: "entry_count is not the number of entries" });
    }
    kids.iter().map(|k| parse_entry(k, version, handler, depth + 1, ctx)).collect()
}

fn parse_entry(b: &BoxRef<'_>, stsd_version: u8, handler: FourCC, depth: u32, ctx: &mut Ctx) -> Result<SampleEntry> {
    let f = b.fourcc;
    let mut c = Cur::new(b.payload, f);
    c.skip(6)?;
    let data_reference_index = c.u16()?;
    let mut entry = SampleEntry { fourcc: f, data_reference_index, codec: Codec::Unknown, video: None, audio: None, bit_depth: None, protected_original: None };
    let video = is_video(&f.0) || (handler.0 == *b"vide" && !is_audio(&f.0) && f.0 != *b"tmcd");
    let audio = !video && (is_audio(&f.0) || handler.0 == *b"soun");
    if f.0 == *b"tmcd" {
        entry.codec = Codec::Timecode(TimecodeEntry::parse(&mut c)?);
        return Ok(entry);
    }
    if video {
        c.skip(16)?;
        let width = c.u16()?;
        let height = c.u16()?;
        c.skip(4 + 4 + 4 + 2 + 32)?;
        let vdepth = c.u16()?;
        c.skip(2)?;
        let kids = children(c.rest(), f, depth, ctx)?;
        let pixel_aspect = match find_one(&kids, b"pasp")? {
            Some(p) => {
                let mut pc = Cur::new(p.payload, p.fourcc);
                Some((pc.u32()?, pc.u32()?))
            }
            None => None,
        };
        let colour = match kids.iter().find(|k| k.fourcc.0 == *b"colr") {
            Some(k) => Some(parse_colr(k.payload)?),
            None => None,
        };
        entry.video = Some(VideoInfo { width, height, depth: vdepth, pixel_aspect, colour });
        let mut fmt = f;
        if f.0 == *b"encv" {
            entry.protected_original = original_format(&kids, depth + 1, ctx)?;
            if let Some(o) = entry.protected_original {
                fmt = o;
            }
        }
        video_codec(&mut entry, fmt, &kids)?;
    } else if audio {
        let qt_version = c.u16()?;
        c.skip(6)?;
        let mut channels = c.u16()? as u32;
        let mut sample_size = c.u16()?;
        c.skip(4)?;
        let rate_fixed = c.u32()?;
        let mut sample_rate = rate_fixed >> 16;
        let mut v2_flags = None;
        // QuickTime sound descriptions v1 and v2 extend the ISO layout; an
        // ISO AudioSampleEntryV1 (stsd version 1) does not.
        let qt = if stsd_version == 0 { qt_version } else { 0 };
        match qt {
            0 => {}
            1 => c.skip(16)?,
            2 => {
                c.skip(4)?; // sizeOfStructOnly
                let rate = f64::from_bits(c.u64()?);
                sample_rate = if rate.is_finite() && rate > 0.0 { rate.round().min(u32::MAX as f64) as u32 } else { 0 };
                channels = c.u32()?;
                c.skip(4)?;
                let bits = c.u32()?;
                sample_size = bits.min(u16::MAX as u32) as u16;
                v2_flags = Some(c.u32()?);
                c.skip(8)?;
            }
            _ => return Err(Error::UnsupportedVersion { fourcc: f, version: qt.min(255) as u8 }),
        }
        let kids = children(c.rest(), f, depth, ctx)?;
        entry.audio = Some(AudioInfo { channels, sample_rate, sample_size, qt_version: qt });
        let mut fmt = f;
        if f.0 == *b"enca" {
            entry.protected_original = original_format(&kids, depth + 1, ctx)?;
            if let Some(o) = entry.protected_original {
                fmt = o;
            }
        }
        audio_codec(&mut entry, fmt, &kids, sample_size, v2_flags, depth + 1, ctx)?;
    }
    Ok(entry)
}

fn parse_colr(p: &[u8]) -> Result<Colour> {
    let mut c = Cur::new(p, FourCC(*b"colr"));
    let ty = c.fourcc_field()?;
    Ok(match &ty.0 {
        b"nclx" => Colour::Nclx { primaries: c.u16()?, transfer: c.u16()?, matrix: c.u16()?, full_range: Some(c.u8()? & 0x80 != 0) },
        b"nclc" => Colour::Nclx { primaries: c.u16()?, transfer: c.u16()?, matrix: c.u16()?, full_range: None },
        b"rICC" | b"prof" => Colour::Icc { restricted: ty.0 == *b"rICC", len: c.remaining() },
        _ => Colour::Other(ty),
    })
}

fn original_format(kids: &[BoxRef<'_>], depth: u32, ctx: &mut Ctx) -> Result<Option<FourCC>> {
    let Some(sinf) = find_one(kids, b"sinf")? else { return Ok(None) };
    let sk = children(sinf.payload, sinf.fourcc, depth, ctx)?;
    match find_one(&sk, b"frma")? {
        Some(f) => Ok(Some(Cur::new(f.payload, f.fourcc).fourcc_field()?)),
        None => Ok(None),
    }
}

fn video_codec(entry: &mut SampleEntry, fmt: FourCC, kids: &[BoxRef<'_>]) -> Result<()> {
    match &fmt.0 {
        b"avc1" | b"avc3" => {
            let cfg = AvcConfig::parse(require(kids, fmt, b"avcC")?.payload)?;
            entry.bit_depth = cfg.bit_depth_luma;
            entry.codec = Codec::Avc(cfg);
        }
        b"hvc1" | b"hev1" => {
            let cfg = HevcConfig::parse(require(kids, fmt, b"hvcC")?.payload)?;
            entry.bit_depth = Some(cfg.bit_depth_luma);
            entry.codec = Codec::Hevc(cfg);
        }
        b"av01" => {
            let cfg = Av1Config::parse(require(kids, fmt, b"av1C")?.payload)?;
            entry.bit_depth = Some(cfg.bit_depth());
            entry.codec = Codec::Av1(cfg);
        }
        f => {
            if let Some(p) = ProRes::from_fourcc(f) {
                entry.codec = Codec::ProRes(p);
            }
        }
    }
    Ok(())
}

fn audio_codec(entry: &mut SampleEntry, fmt: FourCC, kids: &[BoxRef<'_>], sample_size: u16, v2_flags: Option<u32>, depth: u32, ctx: &mut Ctx) -> Result<()> {
    let pcm = |float, big_endian, signed, bits| Codec::Pcm(Pcm { float, big_endian, signed, bits_per_sample: bits });
    let codec = match &fmt.0 {
        b"Opus" => Codec::Opus(OpusConfig::parse(find_one(kids, b"dOps")?.ok_or(Error::Missing { parent: fmt, child: FourCC(*b"dOps") })?.payload)?),
        b"mp4a" => {
            // QuickTime puts esds inside a `wave` atom.
            let esds = match find_one(kids, b"esds")? {
                Some(e) => e.payload.to_vec(),
                None => match find_one(kids, b"wave")? {
                    Some(w) => {
                        let wk = children(w.payload, w.fourcc, depth, ctx)?;
                        find_one(&wk, b"esds")?.ok_or(Error::Missing { parent: w.fourcc, child: FourCC(*b"esds") })?.payload.to_vec()
                    }
                    None => return Err(Error::Missing { parent: fmt, child: FourCC(*b"esds") }),
                },
            };
            Codec::Mp4Audio(AacConfig::parse(&esds)?)
        }
        b"lpcm" => {
            // QuickTime v2 LPCM: formatSpecificFlags (kAudioFormatFlagIsFloat
            // 1, IsBigEndian 2, IsSignedInteger 4).
            let fl = v2_flags.unwrap_or(0);
            pcm(fl & 1 != 0, fl & 2 != 0, fl & 4 != 0, sample_size)
        }
        b"sowt" => pcm(false, false, true, sample_size),
        b"twos" => pcm(false, true, true, sample_size),
        b"raw " => pcm(false, true, false, 8),
        // Big-endian unless a QuickTime `enda` atom (in `wave`, or beside
        // it) says little-endian.
        b"in24" => pcm(false, !little_endian(kids, depth, ctx)?, true, 24),
        b"in32" => pcm(false, !little_endian(kids, depth, ctx)?, true, 32),
        b"fl32" => pcm(true, !little_endian(kids, depth, ctx)?, true, 32),
        b"fl64" => pcm(true, !little_endian(kids, depth, ctx)?, true, 64),
        b"ipcm" | b"fpcm" => {
            let float = fmt.0 == *b"fpcm";
            match find_one(kids, b"pcmC")? {
                Some(p) => {
                    let mut c = Cur::new(p.payload, p.fourcc);
                    c.full_box()?;
                    let flags = c.u8()?;
                    let bits = c.u8()? as u16;
                    pcm(float, flags & 1 == 0, !float, bits)
                }
                None => return Err(Error::Missing { parent: fmt, child: FourCC(*b"pcmC") }),
            }
        }
        _ => Codec::Unknown,
    };
    if let Codec::Pcm(p) = &codec {
        entry.bit_depth = u8::try_from(p.bits_per_sample).ok();
    }
    entry.codec = codec;
    Ok(())
}

/// QuickTime's `enda` atom (16 bits, 1 = little-endian), in `wave` or
/// directly in the sound sample entry.
fn little_endian(kids: &[BoxRef<'_>], depth: u32, ctx: &mut Ctx) -> Result<bool> {
    let read = |b: BoxRef<'_>| -> Result<bool> { Ok(Cur::new(b.payload, b.fourcc).u16()? & 0xff != 0) };
    if let Some(e) = find_one(kids, b"enda")? {
        return read(e);
    }
    if let Some(w) = find_one(kids, b"wave")? {
        let wk = children(w.payload, w.fourcc, depth, ctx)?;
        if let Some(e) = find_one(&wk, b"enda")? {
            return read(e);
        }
    }
    Ok(false)
}
