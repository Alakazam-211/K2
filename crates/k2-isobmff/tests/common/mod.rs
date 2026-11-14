//! Hand-built ISO BMFF / MOV fixtures, made in code (no media files are
//! committed). Shared by the tests and `examples/fuzz_seeds.rs`.
#![allow(dead_code)]

use std::io::{Read, Seek, SeekFrom};

pub fn bx(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(fourcc);
    v.extend_from_slice(body);
    v
}

pub fn full(fourcc: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = ((version as u32) << 24 | flags).to_be_bytes().to_vec();
    b.extend_from_slice(body);
    bx(fourcc, &b)
}

pub fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

pub fn u16b(v: u16) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}
pub fn u32b(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}
pub fn u64b(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

/// A visual sample entry with the given child boxes.
pub fn visual_entry(fourcc: &[u8; 4], w: u16, h: u16, kids: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend(u16b(1));
    b.extend([0u8; 16]);
    b.extend(u16b(w));
    b.extend(u16b(h));
    b.extend(u32b(0x0048_0000));
    b.extend(u32b(0x0048_0000));
    b.extend(u32b(0));
    b.extend(u16b(1));
    b.extend([0u8; 32]);
    b.extend(u16b(24));
    b.extend(u16b(0xffff));
    for k in kids {
        b.extend_from_slice(k);
    }
    bx(fourcc, &b)
}

/// An ISO / QuickTime v0 sound sample entry.
pub fn audio_entry(fourcc: &[u8; 4], channels: u16, sample_size: u16, rate: u32, kids: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend(u16b(1));
    b.extend([0u8; 8]);
    b.extend(u16b(channels));
    b.extend(u16b(sample_size));
    b.extend(u16b(0));
    b.extend(u16b(0));
    b.extend(u32b(rate << 16));
    for k in kids {
        b.extend_from_slice(k);
    }
    bx(fourcc, &b)
}

/// A QuickTime v2 sound description (`lpcm`).
pub fn audio_entry_qt_v2(fourcc: &[u8; 4], rate: f64, channels: u32, bits: u32, flags: u32) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend(u16b(1));
    b.extend(u16b(2)); // version
    b.extend(u16b(0));
    b.extend(u32b(0));
    b.extend(u16b(3));
    b.extend(u16b(16));
    b.extend(u16b(0xfffe));
    b.extend(u16b(0));
    b.extend(u32b(65536));
    b.extend(u32b(72));
    b.extend(u64b(rate.to_bits()));
    b.extend(u32b(channels));
    b.extend(u32b(0x7f00_0000));
    b.extend(u32b(bits));
    b.extend(u32b(flags));
    b.extend(u32b(bits / 8 * channels));
    b.extend(u32b(1));
    bx(fourcc, &b)
}

pub fn tmcd_entry(flags: u32, timescale: u32, frame_duration: u32, frames: u8) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend(u16b(1));
    b.extend(u32b(0));
    b.extend(u32b(flags));
    b.extend(u32b(timescale));
    b.extend(u32b(frame_duration));
    b.push(frames);
    b.push(0);
    bx(b"tmcd", &b)
}

pub const SPS: &[u8] = &[0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x05, 0xbb, 0x01, 0x10];
pub const PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];

/// `avcC` (High profile, 4-byte lengths, with the 4:2:0 8-bit extension).
pub fn avcc() -> Vec<u8> {
    let mut b = vec![1, 0x64, 0x00, 0x1f, 0xff, 0xe1];
    b.extend(u16b(SPS.len() as u16));
    b.extend_from_slice(SPS);
    b.push(1);
    b.extend(u16b(PPS.len() as u16));
    b.extend_from_slice(PPS);
    b.extend([0xfd, 0xf8, 0xf8, 0x00]);
    bx(b"avcC", &b)
}

/// `hvcC`, Main 10, with one VPS, SPS and PPS.
pub fn hvcc() -> Vec<u8> {
    let mut b = vec![1, 0x02];
    b.extend(u32b(0x2000_0000));
    b.extend([0x90, 0, 0, 0, 0, 0]);
    b.push(120);
    b.extend([0xf0, 0x00, 0xfc, 0xfd, 0xfa, 0xfa, 0x00, 0x00, 0x0f]);
    b.push(3);
    for (t, nal) in [(32u8, &[0x40u8, 0x01][..]), (33, &[0x42, 0x01]), (34, &[0x44, 0x01])] {
        b.push(0x80 | t);
        b.extend(u16b(1));
        b.extend(u16b(nal.len() as u16));
        b.extend_from_slice(nal);
    }
    bx(b"hvcC", &b)
}

pub fn pasp(h: u32, v: u32) -> Vec<u8> {
    bx(b"pasp", &cat(&[&u32b(h), &u32b(v)]))
}

pub fn colr_nclc(p: u16, t: u16, m: u16) -> Vec<u8> {
    bx(b"colr", &cat(&[b"nclc", &u16b(p), &u16b(t), &u16b(m)]))
}

pub fn colr_nclx(p: u16, t: u16, m: u16, full_range: bool) -> Vec<u8> {
    bx(b"colr", &cat(&[b"nclx", &u16b(p), &u16b(t), &u16b(m), &[if full_range { 0x80 } else { 0 }]]))
}

/// `dOps`, stereo, family 0.
pub fn dops(pre_skip: u16) -> Vec<u8> {
    bx(b"dOps", &cat(&[&[0, 2], &u16b(pre_skip), &u32b(48000), &u16b(0), &[0]]))
}

/// `esds` with an AAC LC AudioSpecificConfig (44.1 kHz stereo: 0x12 0x10).
pub fn esds_aac() -> Vec<u8> {
    let asc = [0x12u8, 0x10];
    let dsi = cat(&[&[0x05, asc.len() as u8], &asc]);
    let dcd_body = cat(&[&[0x40, 0x15, 0, 0x18, 0], &u32b(128000), &u32b(128000), &dsi]);
    let dcd = cat(&[&[0x04, 0x80, 0x80, 0x80, dcd_body.len() as u8], &dcd_body]);
    let sl = [0x06u8, 1, 2];
    let es_body = cat(&[&u16b(1), &[0], &dcd, &sl]);
    let es = cat(&[&[0x03, es_body.len() as u8], &es_body]);
    full(b"esds", 0, 0, &es)
}

pub enum SizeSpec {
    Stsz(Vec<u32>),
    Constant(u32, u32),
    Stz2(u8, Vec<u32>),
}

pub struct TrackSpec {
    pub id: u32,
    pub handler: [u8; 4],
    pub timescale: u32,
    pub duration: u64,
    pub tkhd_size: (u32, u32),
    pub entries: Vec<Vec<u8>>,
    pub stts: Vec<(u32, u32)>,
    pub ctts: Option<(u8, Vec<(u32, i32)>)>,
    pub stss: Option<Vec<u32>>,
    pub stsc: Vec<(u32, u32, u32)>,
    pub sizes: SizeSpec,
    pub chunks: Vec<Vec<u8>>,
    pub co64: bool,
    pub elst: Option<(u8, Vec<(u64, i64)>)>,
}

impl TrackSpec {
    /// A video track: `n` samples of `size` bytes (byte = sample index),
    /// `per_chunk` per chunk, duration `delta`, keyframes as given.
    pub fn video(id: u32, entry: Vec<u8>, n: u32, size: u32, per_chunk: u32, delta: u32, keys: Option<Vec<u32>>) -> TrackSpec {
        let sizes: Vec<u32> = (0..n).map(|i| size + i % 3).collect();
        let mut chunks = Vec::new();
        let mut i = 0u32;
        while i < n {
            let mut c = Vec::new();
            for j in i..(i + per_chunk).min(n) {
                c.extend(std::iter::repeat_n(j as u8, sizes[j as usize] as usize));
            }
            chunks.push(c);
            i += per_chunk;
        }
        let mut stsc = vec![(1, per_chunk, 1)];
        let rem = n % per_chunk;
        if rem != 0 && chunks.len() > 1 {
            stsc.push((chunks.len() as u32, rem, 1));
        } else if rem != 0 {
            stsc = vec![(1, rem, 1)];
        }
        TrackSpec {
            id,
            handler: *b"vide",
            timescale: 24000,
            duration: n as u64 * delta as u64,
            tkhd_size: (1920 << 16, 1080 << 16),
            entries: vec![entry],
            stts: vec![(n, delta)],
            ctts: None,
            stss: keys,
            stsc,
            sizes: SizeSpec::Stsz(sizes),
            chunks,
            co64: false,
            elst: None,
        }
    }
}

pub struct MovieSpec {
    /// The ftyp major brand; `None` writes no ftyp (old QuickTime).
    pub brand: Option<[u8; 4]>,
    pub moov_at_end: bool,
    pub timescale: u32,
    pub tracks: Vec<TrackSpec>,
    /// Zero bytes at the start of mdat's payload; above 0 the mdat uses a
    /// 64-bit size.
    pub gap: u64,
}

/// A built file as pieces at offsets (zeros between): files over 4 GiB
/// are read through [`Sparse`] without holding the gap in memory.
pub struct Built {
    pub pieces: Vec<(u64, Vec<u8>)>,
    pub len: u64,
}

impl Built {
    pub fn to_vec(&self) -> Vec<u8> {
        assert!(self.len < 1 << 28, "too large to materialise");
        let mut v = vec![0u8; self.len as usize];
        for (o, p) in &self.pieces {
            v[*o as usize..*o as usize + p.len()].copy_from_slice(p);
        }
        v
    }
    pub fn reader(&self) -> Sparse<'_> {
        Sparse { b: self, pos: 0 }
    }
}

pub struct Sparse<'a> {
    b: &'a Built,
    pos: u64,
}

impl Read for Sparse<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.b.len || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.b.len - self.pos) as usize;
        for (o, p) in &self.b.pieces {
            let end = o + p.len() as u64;
            if self.pos >= *o && self.pos < end {
                let n = want.min((end - self.pos) as usize);
                let s = (self.pos - o) as usize;
                buf[..n].copy_from_slice(&p[s..s + n]);
                self.pos += n as u64;
                return Ok(n);
            }
        }
        let next = self.b.pieces.iter().map(|(o, _)| *o).filter(|&o| o > self.pos).min().unwrap_or(self.b.len);
        let n = want.min((next - self.pos) as usize);
        buf[..n].fill(0);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Sparse<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.b.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

fn trak(t: &TrackSpec, offsets: &[u64]) -> Vec<u8> {
    let mut tk = vec![0u8; 8];
    tk.extend(u32b(t.id));
    tk.extend(u32b(0));
    tk.extend(u32b(t.duration as u32));
    tk.extend([0u8; 8]);
    tk.extend([0u8; 8]);
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        tk.extend(u32b(v));
    }
    tk.extend(u32b(t.tkhd_size.0));
    tk.extend(u32b(t.tkhd_size.1));
    let tkhd = full(b"tkhd", 0, 3, &tk);

    let edts = match &t.elst {
        Some((v, es)) => {
            let mut b = u32b(es.len() as u32);
            for &(d, m) in es {
                if *v == 1 {
                    b.extend(u64b(d));
                    b.extend(u64b(m as u64));
                } else {
                    b.extend(u32b(d as u32));
                    b.extend(u32b(m as i32 as u32));
                }
                b.extend(u32b(0x0001_0000));
            }
            bx(b"edts", &full(b"elst", *v, 0, &b))
        }
        None => Vec::new(),
    };

    let mdhd = full(b"mdhd", 0, 0, &cat(&[&[0u8; 8], &u32b(t.timescale), &u32b(t.duration as u32), &u16b(0x55c4), &u16b(0)]));
    let hdlr = full(b"hdlr", 0, 0, &cat(&[b"mhlr", &t.handler, &[0u8; 12], b"\0"]));

    let mut stsd = u32b(t.entries.len() as u32);
    for e in &t.entries {
        stsd.extend_from_slice(e);
    }
    let stsd = full(b"stsd", 0, 0, &stsd);
    let mut stts = u32b(t.stts.len() as u32);
    for &(c, d) in &t.stts {
        stts.extend(u32b(c));
        stts.extend(u32b(d));
    }
    let stts = full(b"stts", 0, 0, &stts);
    let ctts = match &t.ctts {
        Some((v, es)) => {
            let mut b = u32b(es.len() as u32);
            for &(c, o) in es {
                b.extend(u32b(c));
                b.extend(u32b(o as u32));
            }
            full(b"ctts", *v, 0, &b)
        }
        None => Vec::new(),
    };
    let stss = match &t.stss {
        Some(k) => {
            let mut b = u32b(k.len() as u32);
            for &s in k {
                b.extend(u32b(s));
            }
            full(b"stss", 0, 0, &b)
        }
        None => Vec::new(),
    };
    let mut stsc = u32b(t.stsc.len() as u32);
    for &(a, b, c) in &t.stsc {
        stsc.extend(u32b(a));
        stsc.extend(u32b(b));
        stsc.extend(u32b(c));
    }
    let stsc = full(b"stsc", 0, 0, &stsc);
    let sizes = match &t.sizes {
        SizeSpec::Stsz(v) => {
            let mut b = cat(&[&u32b(0), &u32b(v.len() as u32)]);
            for &s in v {
                b.extend(u32b(s));
            }
            full(b"stsz", 0, 0, &b)
        }
        SizeSpec::Constant(s, n) => full(b"stsz", 0, 0, &cat(&[&u32b(*s), &u32b(*n)])),
        SizeSpec::Stz2(field, v) => {
            let mut b = cat(&[&[0, 0, 0, *field], &u32b(v.len() as u32)]);
            match field {
                4 => {
                    for pair in v.chunks(2) {
                        b.push(((pair[0] as u8) << 4) | pair.get(1).map_or(0, |&x| x as u8));
                    }
                }
                8 => b.extend(v.iter().map(|&x| x as u8)),
                _ => {
                    for &x in v {
                        b.extend(u16b(x as u16));
                    }
                }
            }
            full(b"stz2", 0, 0, &b)
        }
    };
    let mut co = u32b(offsets.len() as u32);
    for &o in offsets {
        if t.co64 {
            co.extend(u64b(o));
        } else {
            co.extend(u32b(o as u32));
        }
    }
    let co = full(if t.co64 { b"co64" } else { b"stco" }, 0, 0, &co);
    let stbl = bx(b"stbl", &cat(&[&stsd, &stts, &ctts, &stss, &stsc, &sizes, &co]));
    let minf = bx(b"minf", &stbl);
    let mdia = bx(b"mdia", &cat(&[&mdhd, &hdlr, &minf]));
    bx(b"trak", &cat(&[&tkhd, &edts, &mdia]))
}

fn moov(m: &MovieSpec, offsets: &[Vec<u64>]) -> Vec<u8> {
    let dur = m.tracks.iter().map(|t| t.duration).max().unwrap_or(0);
    let mut mv = vec![0u8; 8];
    mv.extend(u32b(m.timescale));
    mv.extend(u32b(dur as u32));
    mv.extend(u32b(0x0001_0000));
    mv.extend(u16b(0x0100));
    mv.extend([0u8; 10]);
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        mv.extend(u32b(v));
    }
    mv.extend([0u8; 24]);
    mv.extend(u32b(m.tracks.len() as u32 + 1));
    let mut body = full(b"mvhd", 0, 0, &mv);
    for (t, o) in m.tracks.iter().zip(offsets) {
        body.extend(trak(t, o));
    }
    bx(b"moov", &body)
}

/// Lays the movie out: [ftyp] [moov] mdat [moov].
pub fn build(m: &MovieSpec) -> Built {
    let ftyp = match m.brand {
        Some(b) => bx(b"ftyp", &cat(&[&b, &u32b(0x200), &b, b"isom"])),
        None => Vec::new(),
    };
    let zero: Vec<Vec<u64>> = m.tracks.iter().map(|t| vec![0; t.chunks.len()]).collect();
    let moov_len = moov(m, &zero).len() as u64;
    let data_len: u64 = m.tracks.iter().flat_map(|t| t.chunks.iter()).map(|c| c.len() as u64).sum();
    let large = m.gap > 0;
    let mdat_header = if large { 16 } else { 8 };
    let mdat_at = ftyp.len() as u64 + if m.moov_at_end { 0 } else { moov_len };
    let mut pos = mdat_at + mdat_header + m.gap;
    let mut offsets = Vec::new();
    let mut data = Vec::new();
    for t in &m.tracks {
        let mut o = Vec::new();
        for c in &t.chunks {
            o.push(pos);
            pos += c.len() as u64;
            data.extend_from_slice(c);
        }
        offsets.push(o);
    }
    let moov = moov(m, &offsets);
    let mdat_size = mdat_header + m.gap + data_len;
    let mdat_head = if large { cat(&[&u32b(1), b"mdat", &u64b(mdat_size)]) } else { cat(&[&u32b(mdat_size as u32), b"mdat"]) };
    let mut pieces = Vec::new();
    let mut head = ftyp.clone();
    if !m.moov_at_end {
        head.extend_from_slice(&moov);
    }
    head.extend_from_slice(&mdat_head);
    pieces.push((0u64, head));
    pieces.push((mdat_at + mdat_header + m.gap, data));
    let mut len = mdat_at + mdat_size;
    if m.moov_at_end {
        pieces.push((len, moov.clone()));
        len += moov.len() as u64;
    }
    Built { pieces, len }
}

/// An AV1 sequence header OBU (profile 0, 8-bit 4:2:0, no timing info),
/// level `level`, max size (w+1)×(h+1), with its size field.
pub fn av1_sequence_header_obu(level: u8, w_minus_1: u32, h_minus_1: u32) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    let mut put = |v: u32, n: u32| {
        for i in (0..n).rev() {
            bits.push((v >> i) & 1 == 1);
        }
    };
    put(0, 3); // seq_profile
    put(0, 1); // still_picture
    put(0, 1); // reduced_still_picture_header
    put(0, 1); // timing_info_present_flag
    put(0, 1); // initial_display_delay_present_flag
    put(0, 5); // operating_points_cnt_minus_1
    put(0, 12); // operating_point_idc[0]
    put(level as u32, 5);
    if level > 7 {
        put(0, 1); // seq_tier
    }
    put(11, 4); // frame_width_bits_minus_1
    put(11, 4);
    put(w_minus_1, 12);
    put(h_minus_1, 12);
    put(0, 1); // frame_id_numbers_present_flag
    put(0, 1); // use_128x128_superblock
    put(1, 1); // enable_filter_intra
    put(1, 1); // enable_intra_edge_filter
    put(0, 4); // interintra, masked, warped, dual filter
    put(1, 1); // enable_order_hint
    put(0, 2); // jnt_comp, ref_frame_mvs
    put(1, 1); // seq_choose_screen_content_tools
    put(1, 1); // seq_choose_integer_mv
    put(6, 3); // order_hint_bits_minus_1
    put(0, 1); // superres
    put(1, 1); // cdef
    put(1, 1); // restoration
    put(0, 1); // high_bitdepth
    put(0, 1); // mono_chrome
    put(0, 1); // color_description_present_flag
    put(0, 1); // color_range
    put(0, 2); // chroma_sample_position
    put(0, 1); // separate_uv_delta_q
    put(0, 1); // film_grain_params_present
    put(1, 1); // trailing one bit
    while !bits.len().is_multiple_of(8) {
        bits.push(false);
    }
    let payload: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b as u8)).collect();
    let mut obu = vec![0x0a, payload.len() as u8];
    obu.extend(payload);
    obu
}

/// The QuickTime camera-original shape: moov at the end, an H.264 track
/// (12 samples, 2 B-frames per GOP via ctts v1, keyframes 1 and 7), an
/// edit list, a 24p tmcd track at 01:00:00:00 and a sowt audio track.
pub fn camera_mov() -> MovieSpec {
    let mut v = TrackSpec::video(1, visual_entry(b"avc1", 1920, 1080, &[avcc(), pasp(1, 1), colr_nclc(1, 1, 1)]), 12, 40, 4, 1001, Some(vec![1, 7]));
    v.ctts = Some((1, vec![(1, 1001), (2, -1001), (3, 1001), (6, 0)]));
    v.elst = Some((0, vec![(500, 1001)]));
    let tc = TrackSpec {
        id: 2,
        handler: *b"tmcd",
        timescale: 24000,
        duration: 12 * 1001,
        tkhd_size: (0, 0),
        entries: vec![tmcd_entry(0, 24000, 1000, 24)],
        stts: vec![(1, 12 * 1001)],
        ctts: None,
        stss: None,
        stsc: vec![(1, 1, 1)],
        sizes: SizeSpec::Stsz(vec![4]),
        chunks: vec![86400u32.to_be_bytes().to_vec()],
        co64: false,
        elst: None,
    };
    let a = TrackSpec {
        id: 3,
        handler: *b"soun",
        timescale: 48000,
        duration: 4800,
        tkhd_size: (0, 0),
        entries: vec![audio_entry(b"sowt", 2, 16, 48000, &[])],
        stts: vec![(4800, 1)],
        ctts: None,
        stss: None,
        stsc: vec![(1, 2400, 1)],
        sizes: SizeSpec::Constant(4, 4800),
        chunks: vec![vec![0x11; 9600], vec![0x22; 9600]],
        co64: false,
        elst: None,
    };
    MovieSpec { brand: Some(*b"qt  "), moov_at_end: true, timescale: 600, tracks: vec![v, tc, a], gap: 0 }
}

/// Where the `nth` box of this type starts, walking the box tree (the
/// containers, `stsd`, and the video/audio sample entries' children).
pub fn find_box(data: &[u8], fourcc: &[u8; 4], nth: usize) -> Option<usize> {
    let mut found = Vec::new();
    walk(data, 0, data.len(), &mut |at, ty| {
        if ty == fourcc {
            found.push(at);
        }
    });
    found.get(nth).copied()
}

fn walk(data: &[u8], mut pos: usize, end: usize, f: &mut dyn FnMut(usize, &[u8; 4])) {
    while pos + 8 <= end {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let ty: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();
        let (size, hdr) = match size {
            0 => (end - pos, 8),
            1 => (u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap()) as usize, 16),
            s => (s, 8),
        };
        if size < hdr || pos + size > end {
            return;
        }
        f(pos, &ty);
        let body = pos + hdr;
        let skip = match &ty {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" | b"mvex" | b"moof" | b"traf" | b"wave" | b"sinf" => Some(0),
            b"stsd" => Some(8),
            b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"av01" | b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => Some(78),
            b"Opus" | b"mp4a" | b"sowt" | b"twos" | b"ipcm" | b"fpcm" => Some(28),
            _ => None,
        };
        if let Some(s) = skip {
            if body + s <= pos + size {
                walk(data, body + s, pos + size, f);
            }
        }
        pos += size;
    }
}

/// Overwrites big-endian bytes at `box start + 8 + at` (inside the payload).
pub fn patch(data: &mut [u8], fourcc: &[u8; 4], nth: usize, at: usize, bytes: &[u8]) {
    let b = find_box(data, fourcc, nth).unwrap_or_else(|| panic!("no {}", String::from_utf8_lossy(fourcc)));
    data[b + 8 + at..b + 8 + at + bytes.len()].copy_from_slice(bytes);
}
