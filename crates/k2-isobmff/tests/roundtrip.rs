//! C2 through C1: proxies of synthetic AV1, Opus and H.264 samples (dummy
//! payloads, valid configurations) are written as CMAF fMP4, read back,
//! and every field compared: sizes, dts, pts, keyframes, durations, bytes,
//! configurations, `sidx`. The writer is deterministic.

mod common;

use common::*;
use k2_isobmff::*;
use std::io::Cursor;

fn av1_setup(level: u8) -> TrackSetup {
    TrackSetup {
        track_id: 1,
        timescale: 60000,
        codec: TrackCodec::Av1(Av1Config::from_sequence_header(&av1_sequence_header_obu(level, 1279, 719)).unwrap()),
        width: 1280,
        height: 720,
        pixel_aspect: Some((1, 1)),
        colour: Some(Colour::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: Some(false) }),
        language: *b"und",
    }
}

fn opus_setup() -> TrackSetup {
    TrackSetup {
        track_id: 2,
        timescale: 48000,
        codec: TrackCodec::Opus(OpusConfig { output_channel_count: 2, pre_skip: 312, input_sample_rate: 48000, output_gain: 0, channel_mapping_family: 0, channel_mapping: None }),
        width: 0,
        height: 0,
        pixel_aspect: None,
        colour: None,
        language: *b"eng",
    }
}

fn avc_setup() -> TrackSetup {
    TrackSetup {
        track_id: 3,
        timescale: 30000,
        codec: TrackCodec::Avc(AvcConfig::parse(&avcc()[8..]).unwrap()),
        width: 640,
        height: 360,
        pixel_aspect: None,
        colour: None,
        language: *b"und",
    }
}

/// One expected sample.
#[derive(Clone, Debug, PartialEq)]
struct Want {
    size: u32,
    dts: u64,
    pts: i64,
    duration: u32,
    keyframe: bool,
    bytes: Vec<u8>,
}

/// Per track: the fragments of each segment (samples and data) and the
/// expected samples overall.
struct Stream {
    track_id: u32,
    segments: Vec<(u64, Vec<FragmentSample>, Vec<u8>)>,
    want: Vec<Want>,
}

fn stream(track_id: u32, segs: usize, per_seg: usize, duration: u32, size: impl Fn(usize) -> u32, key: impl Fn(usize) -> bool, cto: impl Fn(usize) -> i32) -> Stream {
    let mut segments = Vec::new();
    let mut want = Vec::new();
    let mut dts = 0u64;
    let mut i = 0usize;
    for _ in 0..segs {
        let base = dts;
        let mut samples = Vec::new();
        let mut data = Vec::new();
        for _ in 0..per_seg {
            let s = size(i);
            let bytes: Vec<u8> = (0..s).map(|k| (i as u32 * 31 + k * 7 + track_id) as u8).collect();
            let fs = FragmentSample { duration, size: s, keyframe: key(i), composition_offset: cto(i) };
            want.push(Want { size: s, dts, pts: dts as i64 + fs.composition_offset as i64, duration, keyframe: fs.keyframe, bytes: bytes.clone() });
            samples.push(fs);
            data.extend(bytes);
            dts += duration as u64;
            i += 1;
        }
        segments.push((base, samples, data));
    }
    Stream { track_id, segments, want }
}

fn av1_stream() -> Stream {
    // 2 s segments at 59.94 fps (1001 ticks at 60,000): 120 frames each,
    // a keyframe at each segment start.
    stream(1, 3, 120, 1001, |i| 200 + (i as u32 * 37) % 900, |i| i % 120 == 0, |_| 0)
}

fn opus_stream() -> Stream {
    stream(2, 3, 100, 960, |i| 120 + (i as u32 * 13) % 80, |_| true, |_| 0)
}

fn avc_stream() -> Stream {
    // B-frames: composition offsets, some negative (trun v1).
    let pattern = [1001, 3003, 0, 0, 2002, -1001, 1001, 0];
    stream(3, 3, 60, 1001, |i| 500 + (i as u32 * 101) % 3000, |i| i % 60 == 0, move |i| pattern[i % pattern.len()])
}

fn write(setups: Vec<TrackSetup>, streams: &[&Stream], opts: impl Fn(usize) -> SegmentOptions) -> (Vec<u8>, Vec<(usize, usize)>) {
    let mut w = Fmp4Writer::new(InitConfig { movie_timescale: 1000, creation_time: 0, tracks: setups }).unwrap();
    let mut out = w.init_segment().unwrap();
    let mut ranges = Vec::new();
    for seg in 0..streams[0].segments.len() {
        let frags: Vec<TrackFragment<'_>> = streams
            .iter()
            .map(|s| {
                let (base, samples, data) = &s.segments[seg];
                TrackFragment { track_id: s.track_id, base_media_decode_time: *base, samples, data }
            })
            .collect();
        let start = out.len();
        out.extend(w.media_segment(&frags, opts(seg)).unwrap());
        ranges.push((start, out.len()));
    }
    (out, ranges)
}

fn check_track(r: &mut Mp4Reader<Cursor<Vec<u8>>>, s: &Stream) {
    let t = r.movie().track(s.track_id).unwrap();
    assert_eq!(t.sample_count(), s.want.len() as u64);
    let got: Vec<Sample> = t.samples().collect();
    assert_eq!(got.len(), s.want.len());
    let mut buf = Vec::new();
    for (i, (g, w)) in got.iter().zip(&s.want).enumerate() {
        assert_eq!((g.size, g.dts, g.pts, g.duration, g.keyframe), (w.size, w.dts, w.pts, w.duration, w.keyframe), "track {} sample {i}", s.track_id);
        assert_eq!(g.index, i as u64);
        assert_eq!(g.description_index, 1);
        r.read_sample(g, &mut buf).unwrap();
        assert_eq!(buf, w.bytes, "track {} sample {i} bytes", s.track_id);
    }
}

#[test]
fn av1_opus_h264_in_one_file_read_back_field_for_field() {
    let (a, o, h) = (av1_stream(), opus_stream(), avc_stream());
    let setups = vec![av1_setup(8), opus_setup(), avc_setup()];
    let (file, ranges) = write(setups.clone(), &[&a, &o, &h], |seg| SegmentOptions { styp: seg == 0, sidx: true });
    let mut r = Mp4Reader::new(Cursor::new(file.clone())).unwrap();
    let m = r.movie();
    assert!(m.fragmented);
    assert!(!m.quicktime);
    let ft = m.file_type.as_ref().unwrap();
    assert_eq!(ft.major_brand, FourCC(*b"iso6"));
    assert_eq!(ft.compatible_brands, vec![FourCC(*b"iso6"), FourCC(*b"cmfc"), FourCC(*b"av01")]);
    assert_eq!(m.fragment_count, 3);
    assert_eq!(m.timescale, 1000);
    assert_eq!((m.creation_time, m.next_track_id), (0, 4));
    assert_eq!(m.tracks.len(), 3);

    let v = m.track(1).unwrap();
    assert_eq!((v.kind, v.timescale, v.tkhd_width >> 16, v.tkhd_height >> 16), (TrackKind::Video, 60000, 1280, 720));
    let e = &v.entries[0];
    let TrackCodec::Av1(want_av1) = &setups[0].codec else { unreachable!() };
    assert_eq!(e.codec, Codec::Av1(want_av1.clone()));
    let Codec::Av1(c) = &e.codec else { unreachable!() };
    assert_eq!(c.codec_string(), "av01.0.08M.08");
    assert_eq!(e.video.as_ref().unwrap().pixel_aspect, Some((1, 1)));
    assert_eq!(e.video.as_ref().unwrap().colour, setups[0].colour);
    assert_eq!(v.trex, Some(TrackExtends { default_sample_description_index: 1, default_sample_duration: 0, default_sample_size: 0, default_sample_flags: 0 }));

    let at = m.track(2).unwrap();
    assert_eq!(at.kind, TrackKind::Audio);
    let TrackCodec::Opus(want_opus) = &setups[1].codec else { unreachable!() };
    assert_eq!(at.entries[0].codec, Codec::Opus(want_opus.clone()));
    let ai = at.entries[0].audio.as_ref().unwrap();
    assert_eq!((ai.channels, ai.sample_rate), (2, 48000));
    assert_eq!(at.language, (5 << 10) | (14 << 5) | 7, "eng");

    let ht = m.track(3).unwrap();
    let TrackCodec::Avc(want_avc) = &setups[2].codec else { unreachable!() };
    assert_eq!(ht.entries[0].codec, Codec::Avc(want_avc.clone()));

    // sidx: one per segment, for the first track, referencing moof + mdat.
    assert_eq!(m.segment_indexes.len(), 3);
    for (i, (sx, &(start, end))) in m.segment_indexes.iter().zip(&ranges).enumerate() {
        assert_eq!(sx.reference_id, 1);
        assert_eq!(sx.timescale, 60000);
        assert_eq!(sx.earliest_presentation_time, a.segments[i].0);
        assert_eq!(sx.references.len(), 1);
        let rf = sx.references[0];
        assert_eq!(rf.subsegment_duration, 120 * 1001);
        assert!(rf.starts_with_sap && rf.sap_type == 1 && !rf.reference_type);
        assert_eq!(sx.offset + sx.size + rf.referenced_size as u64, end as u64, "segment {i} ends where sidx says");
        assert!(sx.offset as usize >= start);
    }

    check_track(&mut r, &a);
    check_track(&mut r, &o);
    check_track(&mut r, &h);

    // Deterministic: the same input, the same bytes.
    let (again, _) = write(setups, &[&a, &o, &h], |seg| SegmentOptions { styp: seg == 0, sidx: true });
    assert_eq!(again, file);
}

#[test]
fn one_file_per_rung_each_level_from_the_stream() {
    for (level, codec) in [(1u8, "av01.0.01M.08"), (8, "av01.0.08M.08"), (9, "av01.0.09M.08")] {
        let a = av1_stream();
        let (file, ranges) = write(vec![av1_setup(level)], &[&a], |_| SegmentOptions::default());
        let mut r = Mp4Reader::new(Cursor::new(file.clone())).unwrap();
        let Codec::Av1(c) = &r.movie().tracks[0].entries[0].codec else { panic!("av1") };
        assert_eq!(c.codec_string(), codec);
        assert!(r.movie().segment_indexes.is_empty());
        check_track(&mut r, &a);
        // Each 2 s segment starts with a keyframe at its tfdt.
        let samples: Vec<Sample> = r.movie().tracks[0].samples().collect();
        for (k, &(start, end)) in ranges.iter().enumerate() {
            let first = samples.iter().find(|s| s.offset >= start as u64 && s.offset < end as u64).unwrap();
            assert!(first.keyframe);
            assert_eq!(first.dts, a.segments[k].0);
        }
        // The init segment alone opens, with no samples.
        let init_len = ranges[0].0;
        let r = Mp4Reader::new(Cursor::new(file[..init_len].to_vec())).unwrap();
        assert_eq!(r.movie().tracks[0].sample_count(), 0);
        assert_eq!(r.movie().fragment_count, 0);
    }
}

#[test]
fn single_track_segments_interleaved_by_track() {
    // Audio and video in separate moofs (one traf each), alternating.
    let (a, o) = (av1_stream(), opus_stream());
    let mut w = Fmp4Writer::new(InitConfig { movie_timescale: 90000, creation_time: 5_000_000_000, tracks: vec![av1_setup(8), opus_setup()] }).unwrap();
    let mut file = w.init_segment().unwrap();
    for seg in 0..3 {
        for s in [&a, &o] {
            let (base, samples, data) = &s.segments[seg];
            file.extend(w.media_segment(&[TrackFragment { track_id: s.track_id, base_media_decode_time: *base, samples, data }], SegmentOptions::default()).unwrap());
        }
    }
    assert_eq!(w.next_sequence_number(), 7);
    let mut r = Mp4Reader::new(Cursor::new(file)).unwrap();
    assert_eq!(r.movie().fragment_count, 6);
    assert_eq!(r.movie().creation_time, 5_000_000_000);
    check_track(&mut r, &a);
    check_track(&mut r, &o);
}

#[test]
fn writer_refuses_inconsistent_input() {
    let cfg = |tracks| InitConfig { movie_timescale: 1000, creation_time: 0, tracks };
    assert!(Fmp4Writer::new(cfg(vec![])).is_err());
    assert!(Fmp4Writer::new(InitConfig { movie_timescale: 0, ..cfg(vec![opus_setup()]) }).is_err());
    let mut dup = opus_setup();
    dup.track_id = 1;
    assert!(Fmp4Writer::new(cfg(vec![av1_setup(8), dup])).is_err());
    let mut zero = opus_setup();
    zero.track_id = 0;
    assert!(Fmp4Writer::new(cfg(vec![zero])).is_err());
    let mut ts = opus_setup();
    ts.timescale = 0;
    assert!(Fmp4Writer::new(cfg(vec![ts])).is_err());
    let mut nosize = av1_setup(8);
    nosize.width = 0;
    assert!(Fmp4Writer::new(cfg(vec![nosize])).is_err());
    let mut lang = opus_setup();
    lang.language = *b"EN!";
    assert!(Fmp4Writer::new(cfg(vec![lang])).is_err());

    let mut w = Fmp4Writer::new(cfg(vec![av1_setup(8)])).unwrap();
    let s = [FragmentSample { duration: 1, size: 3, keyframe: true, composition_offset: 0 }];
    let f = |id, data: &'static [u8]| TrackFragment { track_id: id, base_media_decode_time: 0, samples: &s, data };
    assert_eq!(w.media_segment(&[], SegmentOptions::default()), Err(Error::Writer("a segment needs at least one track fragment")));
    assert!(w.media_segment(&[f(9, b"abc")], SegmentOptions::default()).is_err());
    assert_eq!(w.media_segment(&[f(1, b"ab")], SegmentOptions::default()), Err(Error::Writer("sample sizes do not add up to the data length")));
    assert!(w.media_segment(&[f(1, b"abc"), f(1, b"abc")], SegmentOptions::default()).is_err());
    assert!(w.media_segment(&[TrackFragment { track_id: 1, base_media_decode_time: 0, samples: &[], data: b"" }], SegmentOptions::default()).is_err());
    assert_eq!(w.next_sequence_number(), 1, "refused segments take no number");
    assert!(w.media_segment(&[f(1, b"abc")], SegmentOptions { styp: true, sidx: true }).is_ok());
    assert_eq!(w.next_sequence_number(), 2);
}
