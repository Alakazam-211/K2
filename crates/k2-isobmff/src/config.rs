//! Decoder configuration records: `avcC` (ISO/IEC 14496-15 §5.3.3), `hvcC`
//! (§8.3.3), `av1C` and the AV1 sequence header (AV1-ISOBMFF §2.3, AV1
//! spec §5.5), `dOps` (Opus-in-ISOBMFF §4.3.2) and `esds` (14496-1 §7.2.6,
//! 14496-3 §1.6.2.1).

use crate::bytes::{Bits, Cur};
use crate::error::{Error, FourCC, Result};

const AVCC: FourCC = FourCC(*b"avcC");
const HVCC: FourCC = FourCC(*b"hvcC");
const AV1C: FourCC = FourCC(*b"av1C");
const DOPS: FourCC = FourCC(*b"dOps");
const ESDS: FourCC = FourCC(*b"esds");

/// `avcC`: the H.264 decoder configuration record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvcConfig {
    pub profile: u8,
    pub profile_compatibility: u8,
    pub level: u8,
    /// Bytes in each NAL length prefix: 1, 2 or 4.
    pub length_size: u8,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    /// The High-profile extension (profiles 100, 110, 122, 144), if present.
    pub chroma_format: Option<u8>,
    pub bit_depth_luma: Option<u8>,
    pub bit_depth_chroma: Option<u8>,
    pub sps_ext: Vec<Vec<u8>>,
}

fn nal_list(c: &mut Cur<'_>, count: usize) -> Result<Vec<Vec<u8>>> {
    let mut v = Vec::new();
    for _ in 0..count {
        let len = c.u16()? as usize;
        v.push(c.bytes(len)?.to_vec());
    }
    Ok(v)
}

impl AvcConfig {
    /// Parses an `avcC` payload.
    pub fn parse(data: &[u8]) -> Result<AvcConfig> {
        let mut c = Cur::new(data, AVCC);
        if c.u8()? != 1 {
            return Err(Error::Invalid { fourcc: AVCC, what: "configurationVersion is not 1" });
        }
        let profile = c.u8()?;
        let profile_compatibility = c.u8()?;
        let level = c.u8()?;
        let length_size = (c.u8()? & 3) + 1;
        if length_size == 3 {
            return Err(Error::Invalid { fourcc: AVCC, what: "lengthSizeMinusOne is 2" });
        }
        let nsps = (c.u8()? & 0x1f) as usize;
        let sps = nal_list(&mut c, nsps)?;
        let npps = c.u8()? as usize;
        let pps = nal_list(&mut c, npps)?;
        let mut cfg = AvcConfig {
            profile,
            profile_compatibility,
            level,
            length_size,
            sps,
            pps,
            chroma_format: None,
            bit_depth_luma: None,
            bit_depth_chroma: None,
            sps_ext: Vec::new(),
        };
        // The extension is often missing or short in real files: read it
        // only when it parses whole.
        if matches!(profile, 100 | 110 | 122 | 144) && c.remaining() >= 4 {
            let ext = (|| -> Result<(u8, u8, u8, Vec<Vec<u8>>)> {
                let cf = c.u8()? & 3;
                let bl = (c.u8()? & 7) + 8;
                let bc = (c.u8()? & 7) + 8;
                let n = c.u8()? as usize;
                Ok((cf, bl, bc, nal_list(&mut c, n)?))
            })();
            if let Ok((cf, bl, bc, e)) = ext {
                cfg.chroma_format = Some(cf);
                cfg.bit_depth_luma = Some(bl);
                cfg.bit_depth_chroma = Some(bc);
                cfg.sps_ext = e;
            }
        }
        Ok(cfg)
    }

    /// Serialises the record (the `avcC` payload).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if !matches!(self.length_size, 1 | 2 | 4) {
            return Err(Error::Writer("avcC length_size must be 1, 2 or 4"));
        }
        if self.sps.len() > 31 || self.pps.len() > 255 || self.sps_ext.len() > 255 {
            return Err(Error::Writer("avcC: too many parameter sets"));
        }
        let mut o = vec![1, self.profile, self.profile_compatibility, self.level, 0xfc | (self.length_size - 1), 0xe0 | self.sps.len() as u8];
        let put = |o: &mut Vec<u8>, list: &[Vec<u8>]| -> Result<()> {
            for p in list {
                let len = u16::try_from(p.len()).map_err(|_| Error::Writer("avcC: parameter set over 65535 bytes"))?;
                o.extend_from_slice(&len.to_be_bytes());
                o.extend_from_slice(p);
            }
            Ok(())
        };
        put(&mut o, &self.sps)?;
        o.push(self.pps.len() as u8);
        put(&mut o, &self.pps)?;
        if let (Some(cf), Some(bl), Some(bc)) = (self.chroma_format, self.bit_depth_luma, self.bit_depth_chroma) {
            if !(8..=15).contains(&bl) || !(8..=15).contains(&bc) {
                return Err(Error::Writer("avcC: bit depth out of range"));
            }
            o.push(0xfc | (cf & 3));
            o.push(0xf8 | (bl - 8));
            o.push(0xf8 | (bc - 8));
            o.push(self.sps_ext.len() as u8);
            put(&mut o, &self.sps_ext)?;
        }
        Ok(o)
    }

    /// The RFC 6381 codec string, e.g. `avc1.640028`.
    pub fn codec_string(&self) -> String {
        format!("avc1.{:02X}{:02X}{:02X}", self.profile, self.profile_compatibility, self.level)
    }
}

/// `hvcC`: the HEVC decoder configuration record (read only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HevcConfig {
    pub profile_space: u8,
    pub tier: bool,
    pub profile_idc: u8,
    pub profile_compatibility: u32,
    pub constraint_flags: [u8; 6],
    pub level_idc: u8,
    pub chroma_format: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub length_size: u8,
    /// (NAL unit type, the NAL units) per array: VPS, SPS, PPS, SEI.
    pub arrays: Vec<(u8, Vec<Vec<u8>>)>,
}

impl HevcConfig {
    /// Parses an `hvcC` payload.
    pub fn parse(data: &[u8]) -> Result<HevcConfig> {
        let mut c = Cur::new(data, HVCC);
        if c.u8()? != 1 {
            return Err(Error::Invalid { fourcc: HVCC, what: "configurationVersion is not 1" });
        }
        let b = c.u8()?;
        let profile_compatibility = c.u32()?;
        let mut constraint_flags = [0u8; 6];
        constraint_flags.copy_from_slice(c.bytes(6)?);
        let level_idc = c.u8()?;
        c.skip(2)?; // min_spatial_segmentation_idc
        c.skip(1)?; // parallelismType
        let chroma_format = c.u8()? & 3;
        let bit_depth_luma = (c.u8()? & 7) + 8;
        let bit_depth_chroma = (c.u8()? & 7) + 8;
        c.skip(2)?; // avgFrameRate
        let length_size = (c.u8()? & 3) + 1;
        let narrays = c.u8()? as usize;
        let mut arrays = Vec::new();
        for _ in 0..narrays {
            let t = c.u8()? & 0x3f;
            let n = c.u16()? as usize;
            arrays.push((t, nal_list(&mut c, n)?));
        }
        Ok(HevcConfig {
            profile_space: b >> 6,
            tier: b & 0x20 != 0,
            profile_idc: b & 0x1f,
            profile_compatibility,
            constraint_flags,
            level_idc,
            chroma_format,
            bit_depth_luma,
            bit_depth_chroma,
            length_size,
            arrays,
        })
    }
}

/// `av1C`: the AV1 codec configuration record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Av1Config {
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: bool,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub chroma_subsampling_x: bool,
    pub chroma_subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub initial_presentation_delay_minus_one: Option<u8>,
    /// The configuration OBUs (the sequence header, with size fields).
    pub config_obus: Vec<u8>,
}

impl Av1Config {
    /// Parses an `av1C` payload.
    pub fn parse(data: &[u8]) -> Result<Av1Config> {
        let mut c = Cur::new(data, AV1C);
        let b0 = c.u8()?;
        if b0 & 0x80 == 0 || b0 & 0x7f != 1 {
            return Err(Error::Invalid { fourcc: AV1C, what: "marker or version is not 1" });
        }
        let b1 = c.u8()?;
        let b2 = c.u8()?;
        let b3 = c.u8()?;
        Ok(Av1Config {
            seq_profile: b1 >> 5,
            seq_level_idx_0: b1 & 0x1f,
            seq_tier_0: b2 & 0x80 != 0,
            high_bitdepth: b2 & 0x40 != 0,
            twelve_bit: b2 & 0x20 != 0,
            monochrome: b2 & 0x10 != 0,
            chroma_subsampling_x: b2 & 0x08 != 0,
            chroma_subsampling_y: b2 & 0x04 != 0,
            chroma_sample_position: b2 & 3,
            initial_presentation_delay_minus_one: if b3 & 0x10 != 0 { Some(b3 & 0x0f) } else { None },
            config_obus: c.rest().to_vec(),
        })
    }

    /// Serialises the record (the `av1C` payload).
    pub fn to_bytes(&self) -> Vec<u8> {
        let b1 = (self.seq_profile & 7) << 5 | (self.seq_level_idx_0 & 0x1f);
        let b2 = (self.seq_tier_0 as u8) << 7
            | (self.high_bitdepth as u8) << 6
            | (self.twelve_bit as u8) << 5
            | (self.monochrome as u8) << 4
            | (self.chroma_subsampling_x as u8) << 3
            | (self.chroma_subsampling_y as u8) << 2
            | (self.chroma_sample_position & 3);
        let b3 = match self.initial_presentation_delay_minus_one {
            Some(d) => 0x10 | (d & 0x0f),
            None => 0,
        };
        let mut o = vec![0x81, b1, b2, b3];
        o.extend_from_slice(&self.config_obus);
        o
    }

    /// Builds the record from a sequence header OBU, as the encoder wrote it
    /// in the stream (with or without its size field). The level, tier,
    /// profile and colour fields come from the header, never from a table;
    /// the OBU is stored with its size field (`obu_has_size_field = 1`).
    pub fn from_sequence_header(obu: &[u8]) -> Result<Av1Config> {
        let (hdr, normalised) = Av1SequenceHeader::parse_obu(obu)?;
        Ok(Av1Config {
            seq_profile: hdr.seq_profile,
            seq_level_idx_0: hdr.seq_level_idx_0,
            seq_tier_0: hdr.seq_tier_0,
            high_bitdepth: hdr.high_bitdepth,
            twelve_bit: hdr.twelve_bit,
            monochrome: hdr.monochrome,
            chroma_subsampling_x: hdr.subsampling_x,
            chroma_subsampling_y: hdr.subsampling_y,
            chroma_sample_position: hdr.chroma_sample_position,
            initial_presentation_delay_minus_one: None,
            config_obus: normalised,
        })
    }

    /// The sequence header in `config_obus`, parsed (`None` when there is
    /// no sequence header OBU there).
    pub fn sequence_header(&self) -> Option<Result<Av1SequenceHeader>> {
        let mut pos = 0;
        while pos < self.config_obus.len() {
            match obu_at(&self.config_obus[pos..]) {
                Ok((ty, payload, len)) => {
                    if ty == OBU_SEQUENCE_HEADER {
                        return Some(Av1SequenceHeader::parse_payload(payload));
                    }
                    pos += len;
                }
                Err(e) => return Some(Err(e)),
            }
        }
        None
    }

    /// Bits per sample: 8, 10 or 12.
    pub fn bit_depth(&self) -> u8 {
        match (self.high_bitdepth, self.twelve_bit) {
            (false, _) => 8,
            (true, false) => 10,
            (true, true) => 12,
        }
    }

    /// The RFC 6381 codec string in its short form, e.g. `av01.0.08M.08`.
    pub fn codec_string(&self) -> String {
        format!("av01.{}.{:02}{}.{:02}", self.seq_profile, self.seq_level_idx_0, if self.seq_tier_0 { 'H' } else { 'M' }, self.bit_depth())
    }
}

const OBU_SEQUENCE_HEADER: u8 = 1;

fn leb128(data: &[u8]) -> Result<(u64, usize)> {
    let mut v: u64 = 0;
    for i in 0..8 {
        let b = *data.get(i).ok_or(Error::Truncated(AV1C))?;
        v |= ((b & 0x7f) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Ok((v, i + 1));
        }
    }
    Err(Error::Invalid { fourcc: AV1C, what: "leb128 longer than 8 bytes" })
}

fn put_leb128(o: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            o.push(b);
            return;
        }
        o.push(b | 0x80);
    }
}

/// One OBU at the start of `data`: (type, payload, total length). An OBU
/// without a size field runs to the end of `data`.
fn obu_at(data: &[u8]) -> Result<(u8, &[u8], usize)> {
    let h = *data.first().ok_or(Error::Truncated(AV1C))?;
    if h & 0x80 != 0 {
        return Err(Error::Invalid { fourcc: AV1C, what: "OBU forbidden bit set" });
    }
    let ty = (h >> 3) & 0x0f;
    let ext = h & 0x04 != 0;
    let has_size = h & 0x02 != 0;
    let mut pos = 1 + ext as usize;
    if pos > data.len() {
        return Err(Error::Truncated(AV1C));
    }
    let size = if has_size {
        let (s, n) = leb128(&data[pos..])?;
        pos += n;
        s
    } else {
        (data.len() - pos) as u64
    };
    let end = (pos as u64).checked_add(size).filter(|&e| e <= data.len() as u64).ok_or(Error::Truncated(AV1C))? as usize;
    Ok((ty, &data[pos..end], end))
}

/// The fields of an AV1 sequence header OBU that the container and the
/// player need (AV1 spec §5.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Av1SequenceHeader {
    pub seq_profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,
    /// (num_units_in_display_tick, time_scale) when timing info is present.
    pub timing_info: Option<(u32, u32)>,
    pub operating_points: u8,
    /// Operating point 0's level and tier.
    pub seq_level_idx_0: u8,
    pub seq_tier_0: bool,
    pub max_frame_width: u32,
    pub max_frame_height: u32,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub bit_depth: u8,
    pub monochrome: bool,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub full_range: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub film_grain_params_present: bool,
}

impl Av1SequenceHeader {
    /// Parses a whole sequence header OBU (header included) and returns the
    /// header and the OBU re-emitted with its size field.
    pub fn parse_obu(obu: &[u8]) -> Result<(Av1SequenceHeader, Vec<u8>)> {
        let (ty, payload, _) = obu_at(obu)?;
        if ty != OBU_SEQUENCE_HEADER {
            return Err(Error::Invalid { fourcc: AV1C, what: "not a sequence header OBU" });
        }
        let hdr = Av1SequenceHeader::parse_payload(payload)?;
        let ext = obu[0] & 0x04 != 0;
        let mut o = vec![obu[0] | 0x02];
        if ext {
            o.push(obu[1]);
        }
        put_leb128(&mut o, payload.len() as u64);
        o.extend_from_slice(payload);
        Ok((hdr, o))
    }

    /// Parses a sequence header OBU's payload.
    pub fn parse_payload(p: &[u8]) -> Result<Av1SequenceHeader> {
        let mut b = Bits::new(p, AV1C);
        let seq_profile = b.bits(3)? as u8;
        if seq_profile > 2 {
            return Err(Error::Invalid { fourcc: AV1C, what: "seq_profile above 2" });
        }
        let still_picture = b.bit()?;
        let reduced = b.bit()?;
        let mut timing_info = None;
        let mut operating_points = 1u8;
        let seq_level_idx_0;
        let mut seq_tier_0 = false;
        if reduced {
            seq_level_idx_0 = b.bits(5)? as u8;
        } else {
            let timing_present = b.bit()?;
            let mut decoder_model_present = false;
            let mut buffer_delay_len = 0u32;
            if timing_present {
                let units = b.bits(32)?;
                let scale = b.bits(32)?;
                if b.bit()? {
                    b.uvlc()?;
                }
                timing_info = Some((units, scale));
                decoder_model_present = b.bit()?;
                if decoder_model_present {
                    buffer_delay_len = b.bits(5)? + 1;
                    b.bits(32)?;
                    b.bits(5)?;
                    b.bits(5)?;
                }
            }
            let initial_display_delay_present = b.bit()?;
            operating_points = b.bits(5)? as u8 + 1;
            let mut level0 = 0;
            for i in 0..operating_points {
                b.bits(12)?;
                let level = b.bits(5)? as u8;
                let tier = if level > 7 { b.bit()? } else { false };
                if i == 0 {
                    level0 = level;
                    seq_tier_0 = tier;
                }
                if decoder_model_present && b.bit()? {
                    b.bits(buffer_delay_len)?;
                    b.bits(buffer_delay_len)?;
                    b.bit()?;
                }
                if initial_display_delay_present && b.bit()? {
                    b.bits(4)?;
                }
            }
            seq_level_idx_0 = level0;
        }
        let wbits = b.bits(4)? + 1;
        let hbits = b.bits(4)? + 1;
        let max_frame_width = b.bits(wbits)? as u64 + 1;
        let max_frame_height = b.bits(hbits)? as u64 + 1;
        if !reduced && b.bit()? {
            b.bits(4)?;
            b.bits(3)?;
        }
        b.bit()?; // use_128x128_superblock
        b.bit()?; // enable_filter_intra
        b.bit()?; // enable_intra_edge_filter
        if !reduced {
            b.bit()?; // interintra
            b.bit()?; // masked compound
            b.bit()?; // warped motion
            b.bit()?; // dual filter
            let order_hint = b.bit()?;
            if order_hint {
                b.bit()?; // jnt comp
                b.bit()?; // ref frame mvs
            }
            let choose_sct = b.bit()?;
            let force_sct = if choose_sct { 2 } else { b.bit()? as u8 };
            if force_sct > 0 && !b.bit()? {
                b.bit()?; // seq_force_integer_mv
            }
            if order_hint {
                b.bits(3)?;
            }
        }
        b.bit()?; // superres
        b.bit()?; // cdef
        b.bit()?; // restoration
        // color_config()
        let high_bitdepth = b.bit()?;
        let mut twelve_bit = false;
        let bit_depth = if seq_profile == 2 && high_bitdepth {
            twelve_bit = b.bit()?;
            if twelve_bit {
                12
            } else {
                10
            }
        } else if high_bitdepth {
            10
        } else {
            8
        };
        let monochrome = if seq_profile == 1 { false } else { b.bit()? };
        let (cp, tc, mc) = if b.bit()? { (b.bits(8)? as u8, b.bits(8)? as u8, b.bits(8)? as u8) } else { (2, 2, 2) };
        let full_range;
        let (sx, sy);
        let mut csp = 0u8;
        if monochrome {
            full_range = b.bit()?;
            sx = true;
            sy = true;
        } else if cp == 1 && tc == 13 && mc == 0 {
            full_range = true;
            sx = false;
            sy = false;
        } else {
            full_range = b.bit()?;
            match seq_profile {
                0 => {
                    sx = true;
                    sy = true;
                }
                1 => {
                    sx = false;
                    sy = false;
                }
                _ => {
                    if bit_depth == 12 {
                        sx = b.bit()?;
                        sy = if sx { b.bit()? } else { false };
                    } else {
                        sx = true;
                        sy = false;
                    }
                }
            }
            if sx && sy {
                csp = b.bits(2)? as u8;
            }
        }
        if !monochrome {
            b.bit()?; // separate_uv_delta_q
        }
        let film_grain_params_present = b.bit()?;
        Ok(Av1SequenceHeader {
            seq_profile,
            still_picture,
            reduced_still_picture_header: reduced,
            timing_info,
            operating_points,
            seq_level_idx_0,
            seq_tier_0,
            max_frame_width: max_frame_width as u32,
            max_frame_height: max_frame_height as u32,
            high_bitdepth,
            twelve_bit,
            bit_depth,
            monochrome,
            colour_primaries: cp,
            transfer_characteristics: tc,
            matrix_coefficients: mc,
            full_range,
            subsampling_x: sx,
            subsampling_y: sy,
            chroma_sample_position: csp,
            film_grain_params_present,
        })
    }
}

/// `dOps`: the Opus specific box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpusConfig {
    pub output_channel_count: u8,
    pub pre_skip: u16,
    pub input_sample_rate: u32,
    pub output_gain: i16,
    pub channel_mapping_family: u8,
    /// (stream count, coupled count, channel mapping) for family ≠ 0.
    pub channel_mapping: Option<(u8, u8, Vec<u8>)>,
}

impl OpusConfig {
    /// Parses a `dOps` payload.
    pub fn parse(data: &[u8]) -> Result<OpusConfig> {
        let mut c = Cur::new(data, DOPS);
        if c.u8()? != 0 {
            return Err(Error::Invalid { fourcc: DOPS, what: "Version is not 0" });
        }
        let output_channel_count = c.u8()?;
        let pre_skip = c.u16()?;
        let input_sample_rate = c.u32()?;
        let output_gain = c.i16()?;
        let channel_mapping_family = c.u8()?;
        let channel_mapping = if channel_mapping_family != 0 {
            let s = c.u8()?;
            let cc = c.u8()?;
            Some((s, cc, c.bytes(output_channel_count as usize)?.to_vec()))
        } else {
            None
        };
        Ok(OpusConfig { output_channel_count, pre_skip, input_sample_rate, output_gain, channel_mapping_family, channel_mapping })
    }

    /// Serialises the box payload.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut o = vec![0, self.output_channel_count];
        o.extend_from_slice(&self.pre_skip.to_be_bytes());
        o.extend_from_slice(&self.input_sample_rate.to_be_bytes());
        o.extend_from_slice(&self.output_gain.to_be_bytes());
        o.push(self.channel_mapping_family);
        match (&self.channel_mapping, self.channel_mapping_family) {
            (None, 0) => {}
            (Some((s, cc, map)), f) if f != 0 => {
                if map.len() != self.output_channel_count as usize {
                    return Err(Error::Writer("dOps: channel mapping length is not the channel count"));
                }
                o.push(*s);
                o.push(*cc);
                o.extend_from_slice(map);
            }
            _ => return Err(Error::Writer("dOps: channel mapping does not match the family")),
        }
        Ok(o)
    }
}

/// `esds`: the MPEG-4 elementary stream descriptor of an `mp4a` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AacConfig {
    /// objectTypeIndication: 0x40 is MPEG-4 audio (AAC), 0x66–0x68 MPEG-2
    /// AAC, 0x69/0x6B MP3.
    pub object_type_indication: u8,
    pub stream_type: u8,
    pub max_bitrate: u32,
    pub avg_bitrate: u32,
    /// The DecoderSpecificInfo bytes (the AudioSpecificConfig for AAC).
    pub decoder_specific: Vec<u8>,
    /// From the AudioSpecificConfig: audio object type (2 = AAC LC, 5 = SBR,
    /// 29 = PS, …), the sampling rate and the channel configuration.
    pub audio_object_type: Option<u8>,
    pub sample_rate: Option<u32>,
    pub channel_configuration: Option<u8>,
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// One descriptor: (tag, body).
fn descriptor<'a>(c: &mut Cur<'a>) -> Result<(u8, &'a [u8])> {
    let tag = c.u8()?;
    let mut size: u32 = 0;
    for i in 0..4 {
        let b = c.u8()?;
        size = (size << 7) | (b & 0x7f) as u32;
        if b & 0x80 == 0 {
            break;
        }
        if i == 3 {
            return Err(Error::Invalid { fourcc: ESDS, what: "descriptor size longer than 4 bytes" });
        }
    }
    Ok((tag, c.bytes(size as usize)?))
}

impl AacConfig {
    /// Parses an `esds` payload (FullBox header included).
    pub fn parse(data: &[u8]) -> Result<AacConfig> {
        let mut c = Cur::new(data, ESDS);
        let (v, _) = c.full_box()?;
        if v != 0 {
            return Err(Error::UnsupportedVersion { fourcc: ESDS, version: v });
        }
        let (tag, es) = descriptor(&mut c)?;
        if tag != 0x03 {
            return Err(Error::Invalid { fourcc: ESDS, what: "no ES_Descriptor" });
        }
        let mut e = Cur::new(es, ESDS);
        e.skip(2)?; // ES_ID
        let flags = e.u8()?;
        if flags & 0x80 != 0 {
            e.skip(2)?;
        }
        if flags & 0x40 != 0 {
            let n = e.u8()? as usize;
            e.skip(n)?;
        }
        if flags & 0x20 != 0 {
            e.skip(2)?;
        }
        while e.remaining() > 0 {
            let (tag, body) = descriptor(&mut e)?;
            if tag != 0x04 {
                continue;
            }
            let mut d = Cur::new(body, ESDS);
            let oti = d.u8()?;
            let st = d.u8()? >> 2;
            d.skip(3)?;
            let max_bitrate = d.u32()?;
            let avg_bitrate = d.u32()?;
            let mut dsi = Vec::new();
            while d.remaining() > 0 {
                let (t, b) = descriptor(&mut d)?;
                if t == 0x05 {
                    dsi = b.to_vec();
                    break;
                }
            }
            let mut cfg = AacConfig {
                object_type_indication: oti,
                stream_type: st,
                max_bitrate,
                avg_bitrate,
                decoder_specific: dsi,
                audio_object_type: None,
                sample_rate: None,
                channel_configuration: None,
            };
            if oti == 0x40 && !cfg.decoder_specific.is_empty() {
                // AudioSpecificConfig is informational: a short one leaves
                // the fields empty rather than failing the track.
                let mut b = Bits::new(&cfg.decoder_specific, ESDS);
                let asc = (|| -> Result<(u8, Option<u32>, u8)> {
                    let mut aot = b.bits(5)? as u8;
                    if aot == 31 {
                        aot = 32 + b.bits(6)? as u8;
                    }
                    let sfi = b.bits(4)? as usize;
                    let rate = if sfi == 15 { Some(b.bits(24)?) } else { AAC_RATES.get(sfi).copied() };
                    Ok((aot, rate, b.bits(4)? as u8))
                })();
                if let Ok((aot, rate, ch)) = asc {
                    cfg.audio_object_type = Some(aot);
                    cfg.sample_rate = rate;
                    cfg.channel_configuration = Some(ch);
                }
            }
            return Ok(cfg);
        }
        Err(Error::Invalid { fourcc: ESDS, what: "no DecoderConfigDescriptor" })
    }
}
