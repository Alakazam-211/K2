//! Malformed input, each case with the typed error it must give; then
//! sweeps (every truncation, every single-byte corruption) that must never
//! panic. Fixtures are built in code.

mod common;

use common::*;
use k2_isobmff::*;
use std::io::Cursor;

fn open_with(data: Vec<u8>, limits: Limits) -> Result<Mp4Reader<Cursor<Vec<u8>>>> {
    Mp4Reader::with_limits(Cursor::new(data), limits)
}

fn err(data: Vec<u8>) -> Error {
    err_with(data, Limits::default())
}

fn err_with(data: Vec<u8>, limits: Limits) -> Error {
    match open_with(data, limits) {
        Ok(_) => panic!("opened a malformed file"),
        Err(e) => e,
    }
}

fn fc(b: &[u8; 4]) -> FourCC {
    FourCC(*b)
}

fn camera() -> Vec<u8> {
    build(&camera_mov()).to_vec()
}

fn simple_spec() -> MovieSpec {
    let t = TrackSpec::video(1, visual_entry(b"avc1", 64, 64, &[avcc()]), 12, 10, 4, 100, Some(vec![1, 5]));
    MovieSpec { brand: Some(*b"isom"), moov_at_end: false, timescale: 1000, tracks: vec![t], gap: 0 }
}

fn with_track(f: impl FnOnce(&mut TrackSpec)) -> Vec<u8> {
    let mut s = simple_spec();
    f(&mut s.tracks[0]);
    build(&s).to_vec()
}

fn mismatch(what_part: &str, e: Error) {
    match e {
        Error::TableMismatch { track_id: 1, what } => assert!(what.contains(what_part), "{what}"),
        e => panic!("expected a table mismatch about {what_part}, got {e:?}"),
    }
}

#[test]
fn top_level_structure() {
    assert_eq!(err(vec![]), Error::NoMovie);
    assert_eq!(err(bx(b"free", &[])), Error::NoMovie);
    assert_eq!(err(vec![0, 0, 0, 8, b'f']), Error::Truncated(fc(b"file")));
    assert_eq!(err(cat(&[&u32b(4), b"free"])), Error::BadBoxSize { fourcc: fc(b"free"), size: 4 });
    assert_eq!(err(cat(&[&u32b(100), b"moov"])), Error::PastEndOfFile { fourcc: fc(b"moov"), end: 100, file_len: 8 });
    assert_eq!(err(cat(&[&u32b(1), b"free", &u64b(8)])), Error::BadBoxSize { fourcc: fc(b"free"), size: 8 });
    assert_eq!(err(cat(&[&u32b(1), b"free", &[0, 0]])), Error::Truncated(fc(b"free")));
    assert_eq!(err(cat(&[&u32b(1), b"free", &u64b(u64::MAX)])), Error::PastEndOfFile { fourcc: fc(b"free"), end: u64::MAX, file_len: 16 });
    let c = camera();
    let moov = find_box(&c, b"moov", 0).unwrap();
    let twice = cat(&[&c, &c[moov..]]);
    assert_eq!(err(twice), Error::Duplicate(fc(b"moov")));
    let fl = u32::from_be_bytes(c[0..4].try_into().unwrap()) as usize;
    let ftyp2 = cat(&[&c[..fl], &c]);
    assert_eq!(err(ftyp2), Error::Duplicate(fc(b"ftyp")));
    // size 0 on the last box runs to the end of the file.
    let mut z = camera();
    let m = find_box(&z, b"moov", 0).unwrap();
    z[m..m + 4].copy_from_slice(&[0, 0, 0, 0]);
    assert!(open_with(z, Limits::default()).is_ok());
}

#[test]
fn sizes_and_limits() {
    let lim = Limits { max_box_bytes: 64, ..Limits::default() };
    assert!(matches!(err_with(camera(), lim), Error::TooLarge { fourcc, limit: 64, .. } if fourcc == fc(b"moov")));
    assert_eq!(err_with(camera(), Limits { max_depth: 4, ..Limits::default() }), Error::LimitReached(Limit::Depth));
    assert_eq!(err_with(camera(), Limits { max_boxes: 3, ..Limits::default() }), Error::LimitReached(Limit::Boxes));
    assert_eq!(err_with(camera(), Limits { max_tracks: 2, ..Limits::default() }), Error::LimitReached(Limit::Tracks));
    assert_eq!(err_with(camera(), Limits { max_samples_per_track: 100, ..Limits::default() }), Error::LimitReached(Limit::TrackSamples));
    assert_eq!(err_with(camera(), Limits { max_total_samples: 4000, ..Limits::default() }), Error::LimitReached(Limit::TotalSamples));
    assert!(matches!(err_with(build(&simple_spec()).to_vec(), Limits { max_table_bytes: 16, ..Limits::default() }), Error::TooLarge { fourcc, .. } if fourcc == fc(b"stsz")));
    assert_eq!(err_with(build(&simple_spec()).to_vec(), Limits { max_entries: 1, ..Limits::default() }), Error::LimitReached(Limit::Entries));

    // A declared count past the bytes is refused before allocating.
    let mut d = build(&simple_spec()).to_vec();
    patch(&mut d, b"stsz", 0, 8, &u32b(1_000_000));
    assert_eq!(err(d), Error::Truncated(fc(b"stsz")));
    let mut d = build(&simple_spec()).to_vec();
    patch(&mut d, b"stsz", 0, 8, &u32b(u32::MAX));
    assert_eq!(err(d), Error::LimitReached(Limit::TrackSamples));
    let mut d = build(&simple_spec()).to_vec();
    patch(&mut d, b"stts", 0, 4, &u32b(0x1000_0000));
    assert_eq!(err(d), Error::LimitReached(Limit::Entries));
    let mut d = build(&simple_spec()).to_vec();
    patch(&mut d, b"stco", 0, 4, &u32b(100_000));
    assert_eq!(err(d), Error::Truncated(fc(b"stco")));
    let mut d = build(&simple_spec()).to_vec();
    patch(&mut d, b"stsd", 0, 4, &u32b(100_000));
    assert_eq!(err(d), Error::Truncated(fc(b"stsd")));
    let mut d = build(&simple_spec()).to_vec();
    patch(&mut d, b"stsd", 0, 4, &u32b(u32::MAX));
    assert_eq!(err(d), Error::LimitReached(Limit::Entries));

    let mut r = open_with(camera(), Limits { max_sample_bytes: 10, ..Limits::default() }).unwrap();
    let s = r.movie().tracks[0].samples().next().unwrap();
    assert!(matches!(r.read_sample(&s, &mut Vec::new()), Err(Error::TooLarge { .. })));
    let fake = Sample { offset: u64::MAX - 2, ..s };
    assert!(matches!(r.read_sample(&fake, &mut Vec::new()), Err(Error::TooLarge { .. }) | Err(Error::PastEndOfFile { .. })));
}

#[test]
fn boxes_inside_the_movie() {
    // A child that runs past its parent.
    let mut d = camera();
    let mv = find_box(&d, b"mvhd", 0).unwrap();
    d[mv..mv + 4].copy_from_slice(&u32b(1_000_000));
    assert_eq!(err(d), Error::PastParent { fourcc: fc(b"mvhd") });
    // A child size below its header.
    let mut d = camera();
    let mv = find_box(&d, b"mvhd", 0).unwrap();
    d[mv..mv + 4].copy_from_slice(&u32b(3));
    assert_eq!(err(d), Error::BadBoxSize { fourcc: fc(b"mvhd"), size: 3 });
    // No mvhd.
    let mut d = camera();
    let mv = find_box(&d, b"mvhd", 0).unwrap();
    d[mv + 4..mv + 8].copy_from_slice(b"xxxx");
    assert_eq!(err(d), Error::Missing { parent: fc(b"moov"), child: fc(b"mvhd") });
    let mut d = camera();
    patch(&mut d, b"mvhd", 0, 12, &u32b(0));
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"mvhd"), what: "timescale is 0" });
    let mut d = camera();
    patch(&mut d, b"mdhd", 0, 12, &u32b(0));
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"mdhd"), what: "timescale is 0" });
    let mut d = camera();
    patch(&mut d, b"tkhd", 0, 0, &[3]);
    assert_eq!(err(d), Error::UnsupportedVersion { fourcc: fc(b"tkhd"), version: 3 });
    let mut d = camera();
    patch(&mut d, b"tkhd", 0, 12, &u32b(0));
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"tkhd"), what: "track_ID is 0" });
    let mut d = camera();
    patch(&mut d, b"tkhd", 1, 12, &u32b(1));
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"tkhd"), what: "two tracks share a track_ID" });
    let mut d = camera();
    patch(&mut d, b"elst", 0, 0, &[2]);
    assert_eq!(err(d), Error::UnsupportedVersion { fourcc: fc(b"elst"), version: 2 });
    let mut d = camera();
    patch(&mut d, b"elst", 0, 12, &u32b(-5i32 as u32));
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"elst"), what: "media_time below -1" });
    let mut d = camera();
    patch(&mut d, b"stsd", 0, 4, &u32b(2));
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"stsd"), what: "entry_count is not the number of entries" });
    let mut d = camera();
    let s = find_box(&d, b"stbl", 0).unwrap();
    let stsd = find_box(&d, b"stsd", 0).unwrap();
    assert!(stsd > s);
    d[stsd + 4..stsd + 8].copy_from_slice(b"xxxx");
    assert_eq!(err(d), Error::Missing { parent: fc(b"stbl"), child: fc(b"stsd") });
}

#[test]
fn codec_configurations() {
    let mut d = camera();
    patch(&mut d, b"avcC", 0, 0, &[2]);
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"avcC"), what: "configurationVersion is not 1" });
    let mut d = camera();
    patch(&mut d, b"avcC", 0, 4, &[0xfe]);
    assert_eq!(err(d), Error::Invalid { fourcc: fc(b"avcC"), what: "lengthSizeMinusOne is 2" });
    let mut d = camera();
    patch(&mut d, b"avcC", 0, 6, &u16b(500));
    assert_eq!(err(d), Error::Truncated(fc(b"avcC")));
    let mut d = camera();
    let a = find_box(&d, b"avcC", 0).unwrap();
    d[a + 4..a + 8].copy_from_slice(b"xxxx");
    assert_eq!(err(d), Error::Missing { parent: fc(b"avc1"), child: fc(b"avcC") });

    let mut s = camera_mov();
    s.tracks[1].entries = vec![tmcd_entry(0, 24000, 1000, 0)];
    assert_eq!(err(build(&s).to_vec()), Error::Invalid { fourcc: fc(b"tmcd"), what: "tmcd number of frames is 0" });

    let mut s = camera_mov();
    s.tracks[2].entries = vec![audio_entry(b"Opus", 2, 16, 48000, &[])];
    assert_eq!(err(build(&s).to_vec()), Error::Missing { parent: fc(b"Opus"), child: fc(b"dOps") });
    let mut s = camera_mov();
    s.tracks[2].entries = vec![audio_entry(b"Opus", 2, 16, 48000, &[bx(b"dOps", &[1, 2])])];
    assert_eq!(err(build(&s).to_vec()), Error::Invalid { fourcc: fc(b"dOps"), what: "Version is not 0" });
    let mut s = camera_mov();
    s.tracks[2].entries = vec![audio_entry(b"mp4a", 2, 16, 48000, &[full(b"esds", 0, 0, &[0x03, 0x7f, 0, 1])])];
    assert_eq!(err(build(&s).to_vec()), Error::Truncated(fc(b"esds")));

    let mut s = camera_mov();
    s.tracks[0].entries = vec![visual_entry(b"av01", 64, 64, &[bx(b"av1C", &[0x01, 0, 0, 0])])];
    assert_eq!(err(build(&s).to_vec()), Error::Invalid { fourcc: fc(b"av1C"), what: "marker or version is not 1" });
    assert!(Av1Config::from_sequence_header(&[0x0a, 0x05, 0xff]).is_err());
    assert!(Av1Config::from_sequence_header(&[0x8a, 0x00]).is_err());
    assert!(Av1Config::from_sequence_header(&[]).is_err());
    assert!(Av1Config::from_sequence_header(&[0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]).is_err());
    assert!(Av1SequenceHeader::parse_payload(&[0xe0]).is_err(), "seq_profile 7");
}

#[test]
fn sample_tables_that_disagree() {
    mismatch("stts", err(with_track(|t| t.stts = vec![(11, 100)])));
    mismatch("stsc first_chunk", err(with_track(|t| t.stsc = vec![(2, 4, 1)])));
    mismatch("stsc first_chunk", err(with_track(|t| t.stsc = vec![(1, 4, 1), (1, 4, 1)])));
    mismatch("stsc first_chunk", err(with_track(|t| t.stsc = vec![(1, 4, 1), (9, 4, 1)])));
    mismatch("stsc maps", err(with_track(|t| t.stsc = vec![(1, 3, 1)])));
    // Zero samples per chunk everywhere: a map that never reaches the
    // sample count (no looping).
    mismatch("stsc maps", err(with_track(|t| t.stsc = vec![(1, 0, 1)])));
    mismatch("sample_description_index", err(with_track(|t| t.stsc = vec![(1, 4, 2)])));
    mismatch("sample_description_index", err(with_track(|t| t.stsc = vec![(1, 4, 0)])));
    mismatch("stss", err(with_track(|t| t.stss = Some(vec![5, 1]))));
    mismatch("stss", err(with_track(|t| t.stss = Some(vec![0]))));
    mismatch("stss", err(with_track(|t| t.stss = Some(vec![13]))));
    // A sample past the end of the file.
    assert_eq!(
        err(with_track(|t| if let SizeSpec::Stsz(v) = &mut t.sizes {
            *v.last_mut().unwrap() = 1_000_000
        })),
        Error::SampleOutOfFile { track_id: 1, sample: 11 }
    );
    // stco and co64 both; stsz and stz2 both (stss renamed in place).
    let mut d = build(&simple_spec()).to_vec();
    let at = find_box(&d, b"stss", 0).unwrap();
    d[at + 4..at + 8].copy_from_slice(b"co64");
    assert_eq!(err(d), Error::Duplicate(fc(b"co64")));
    let mut d = build(&simple_spec()).to_vec();
    let at = find_box(&d, b"stss", 0).unwrap();
    d[at + 4..at + 8].copy_from_slice(b"stz2");
    assert_eq!(err(d), Error::Duplicate(fc(b"stz2")));
    let mut d = build(&simple_spec()).to_vec();
    let at = find_box(&d, b"stco", 0).unwrap();
    d[at + 4..at + 8].copy_from_slice(b"xxxx");
    assert_eq!(err(d), Error::Missing { parent: fc(b"stbl"), child: fc(b"stco") });
}

fn sweep(data: &[u8], limits: Limits) {
    let try_open = |d: Vec<u8>| {
        if let Ok(mut r) = open_with(d, limits) {
            let samples: Vec<Sample> = r.movie().tracks.iter().flat_map(|t| t.samples().take(64)).collect();
            let ids: Vec<u32> = r.movie().tracks.iter().map(|t| t.id).collect();
            let mut buf = Vec::new();
            for s in &samples {
                let _ = r.read_sample(s, &mut buf);
            }
            for id in ids {
                let _ = r.start_timecode(id);
            }
        }
    };
    for n in 0..data.len() {
        try_open(data[..n].to_vec());
    }
    for i in 0..data.len() {
        for v in [0x00u8, 0xff, data[i] ^ 0x01, data[i].wrapping_add(8)] {
            let mut d = data.to_vec();
            d[i] = v;
            try_open(d);
        }
    }
}

#[test]
fn no_truncation_or_byte_corruption_panics() {
    let limits = Limits { max_samples_per_track: 1 << 16, max_total_samples: 1 << 17, ..Limits::default() };
    let mut small = camera_mov();
    let a = &mut small.tracks[2];
    a.duration = 48;
    a.stts = vec![(48, 1)];
    a.stsc = vec![(1, 24, 1)];
    a.sizes = SizeSpec::Constant(4, 48);
    a.chunks = vec![vec![0x11; 96], vec![0x22; 96]];
    sweep(&build(&small).to_vec(), limits);
    sweep(&build(&simple_spec()).to_vec(), limits);
}

// Movie fragments: an fMP4 init segment from the writer, then moofs built
// by hand.

fn frag_init() -> Vec<u8> {
    let opus = OpusConfig { output_channel_count: 2, pre_skip: 312, input_sample_rate: 48000, output_gain: 0, channel_mapping_family: 0, channel_mapping: None };
    let t = TrackSetup { track_id: 1, timescale: 48000, codec: TrackCodec::Opus(opus), width: 0, height: 0, pixel_aspect: None, colour: None, language: *b"und" };
    Fmp4Writer::new(InitConfig { movie_timescale: 1000, creation_time: 0, tracks: vec![t] }).unwrap().init_segment().unwrap()
}

fn tfhd(track: u32, flags: u32, fields: &[u32]) -> Vec<u8> {
    let mut b = u32b(track);
    for &f in fields {
        b.extend(u32b(f));
    }
    full(b"tfhd", 0, flags, &b)
}

fn trun(version: u8, flags: u32, count: u32, rest: &[u8]) -> Vec<u8> {
    full(b"trun", version, flags, &cat(&[&u32b(count), rest]))
}

fn moof(kids: &[Vec<u8>]) -> Vec<u8> {
    let mut b = full(b"mfhd", 0, 0, &u32b(1));
    for k in kids {
        b.extend_from_slice(k);
    }
    bx(b"moof", &b)
}

fn traf(kids: &[Vec<u8>]) -> Vec<u8> {
    bx(b"traf", &kids.concat())
}

/// init + a moof with one traf (tfhd default-base-is-moof, default size 4,
/// duration 960) + an mdat of `data` bytes.
fn frag_file(tf: Vec<u8>, tr: Vec<u8>, data: usize) -> Vec<u8> {
    cat(&[&frag_init(), &moof(&[traf(&[tf, tr])]), &bx(b"mdat", &vec![7u8; data])])
}

fn base_tfhd() -> Vec<u8> {
    tfhd(1, 0x02_0000 | 0x08 | 0x10, &[960, 4])
}

#[test]
fn hostile_fragments() {
    // A good one first: 3 samples of 4 bytes after the moof header.
    let tr = |count| trun(0, 0x01, count, &u32b(0));
    let ok = frag_file(base_tfhd(), tr(3), 12);
    let moof_len = u32::from_be_bytes(ok[frag_init().len()..][..4].try_into().unwrap());
    let good = frag_file(base_tfhd(), trun(0, 0x01, 3, &u32b(moof_len + 8)), 12);
    let r = open_with(good.clone(), Limits::default()).unwrap();
    let s: Vec<Sample> = r.movie().tracks[0].samples().collect();
    assert_eq!(s.len(), 3);
    assert_eq!((s[2].dts, s[2].size), (1920, 4));

    assert_eq!(err(cat(&[&camera(), &moof(&[])])), Error::Invalid { fourcc: fc(b"moof"), what: "movie fragment without mvex" });
    assert_eq!(err(frag_file(tfhd(99, 0x02_0000, &[]), tr(1), 4)), Error::UnknownTrack(99));
    assert_eq!(err(frag_file(base_tfhd(), tr(u32::MAX), 0)), Error::LimitReached(Limit::TrackSamples));
    assert_eq!(err(frag_file(base_tfhd(), trun(0, 0x01 | 0x200, 1000, &u32b(0)), 0)), Error::Truncated(fc(b"trun")));
    assert!(matches!(err(frag_file(base_tfhd(), tr(1000), 12)), Error::PastEndOfFile { fourcc, .. } if fourcc == fc(b"trun")));
    assert_eq!(err(frag_file(tfhd(1, 0x02_0000 | 0x02, &[5]), tr(1), 4)), Error::Invalid { fourcc: fc(b"tfhd"), what: "sample_description_index out of range" });
    assert_eq!(err(frag_file(base_tfhd(), trun(2, 0x01, 1, &u32b(0)), 4)), Error::UnsupportedVersion { fourcc: fc(b"trun"), version: 2 });
    let before = frag_file(base_tfhd(), trun(0, 0x01, 1, &u32b(-100_000i32 as u32)), 4);
    assert_eq!(err(before), Error::Invalid { fourcc: fc(b"trun"), what: "data_offset before the file start" });
    let no_mfhd = cat(&[&frag_init(), &bx(b"moof", &traf(&[base_tfhd(), tr(1)])), &bx(b"mdat", &[0; 4])]);
    assert_eq!(err(no_mfhd), Error::Missing { parent: fc(b"moof"), child: fc(b"mfhd") });
    let no_tfhd = cat(&[&frag_init(), &moof(&[traf(&[tr(1)])])]);
    assert_eq!(err(no_tfhd), Error::Missing { parent: fc(b"traf"), child: fc(b"tfhd") });
    let two = cat(&[&good, &moof(&[traf(&[base_tfhd(), trun(0, 0, 0, &[])])])]);
    assert_eq!(err_with(two.clone(), Limits { max_fragments: 1, ..Limits::default() }), Error::LimitReached(Limit::Fragments));
    assert!(open_with(two, Limits::default()).is_ok());
    assert!(matches!(err_with(good.clone(), Limits { max_box_bytes: 100, ..Limits::default() }), Error::TooLarge { .. }));
    assert_eq!(err_with(good.clone(), Limits { max_total_samples: 2, ..Limits::default() }), Error::LimitReached(Limit::TotalSamples));
    let mut no_trex = good.clone();
    let t = find_box(&no_trex, b"trex", 0).unwrap();
    no_trex[t + 4..t + 8].copy_from_slice(b"xxxx");
    assert_eq!(err(no_trex), Error::Missing { parent: fc(b"mvex"), child: fc(b"trex") });
    let mut trex_bad = good.clone();
    patch(&mut trex_bad, b"trex", 0, 4, &u32b(42));
    assert_eq!(err(trex_bad), Error::UnknownTrack(42));
    // The time sum cannot overflow.
    let huge = frag_file(tfhd(1, 0x02_0000 | 0x08 | 0x10, &[u32::MAX, 0]), trun(0, 0, 1 << 20, &[]), 0);
    let late = cat(&[&frag_init(), &moof(&[traf(&[base_tfhd(), full(b"tfdt", 1, 0, &u64b(u64::MAX - 10)), tr(1)])]), &bx(b"mdat", &[0; 4])]);
    assert!(open_with(huge, Limits::default()).is_ok(), "2^20 × 2^32 ticks still fits");
    assert_eq!(err(late), Error::Overflow(fc(b"trun")));
}

#[test]
fn no_fragment_truncation_or_corruption_panics() {
    let tr = trun(1, 0x01 | 0x100 | 0x200 | 0x400 | 0x800, 3, &cat(&[&u32b(0), &[0u8; 48]]));
    let file = frag_file(base_tfhd(), tr, 64);
    sweep(&file, Limits { max_samples_per_track: 1 << 16, max_total_samples: 1 << 17, ..Limits::default() });
}
