//! The reader against hand-built MOV / MP4 fixtures: moov at the end, co64
//! past 4 GiB, stz2, ctts v1 with negative offsets, edit lists, a tmcd
//! track at 01:00:00:00, ProRes identification, the codec configurations,
//! and the H.264 Annex B helper.

mod common;

use common::*;
use k2_isobmff::*;
use std::io::Cursor;

fn open(b: &Built) -> Mp4Reader<Cursor<Vec<u8>>> {
    Mp4Reader::new(Cursor::new(b.to_vec())).expect("opens")
}

#[test]
fn camera_mov_with_moov_at_the_end() {
    let built = build(&camera_mov());
    let mut r = open(&built);
    let m = r.movie();
    assert!(m.quicktime);
    assert_eq!(m.file_type.as_ref().unwrap().major_brand, FourCC(*b"qt  "));
    assert!(m.moov_offset > 0, "moov is after mdat");
    assert_eq!(m.timescale, 600);
    assert_eq!(m.tracks.len(), 3);
    assert!(!m.fragmented);

    let v = m.track(1).unwrap();
    assert_eq!(v.kind, TrackKind::Video);
    assert_eq!(v.timescale, 24000);
    assert!(v.enabled);
    assert_eq!(v.tkhd_width >> 16, 1920);
    let e = &v.entries[0];
    assert_eq!(e.fourcc, FourCC(*b"avc1"));
    let vi = e.video.as_ref().unwrap();
    assert_eq!((vi.width, vi.height), (1920, 1080));
    assert_eq!(vi.pixel_aspect, Some((1, 1)));
    assert_eq!(vi.colour, Some(Colour::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: None }));
    assert_eq!(e.bit_depth, Some(8));
    let Codec::Avc(cfg) = &e.codec else { panic!("avc") };
    assert_eq!(cfg.sps, vec![SPS.to_vec()]);
    assert_eq!(cfg.pps, vec![PPS.to_vec()]);
    assert_eq!(cfg.length_size, 4);
    assert_eq!(cfg.codec_string(), "avc1.64001F");

    // ctts v1: +1001, -1001, -1001, +1001 ×3, 0 ×6.
    let cto = [1001, -1001, -1001, 1001, 1001, 1001, 0, 0, 0, 0, 0, 0];
    let samples: Vec<Sample> = v.samples().collect();
    assert_eq!(samples.len(), 12);
    assert_eq!(v.sample_count(), 12);
    let first = built.pieces[1].0;
    let mut off = first;
    for (i, s) in samples.iter().enumerate() {
        assert_eq!(s.index, i as u64);
        assert_eq!(s.size, 40 + i as u32 % 3);
        assert_eq!(s.offset, off, "sample {i}");
        off += s.size as u64;
        assert_eq!(s.dts, i as u64 * 1001);
        assert_eq!(s.pts, i as i64 * 1001 + cto[i]);
        assert_eq!(s.duration, 1001);
        assert_eq!(s.keyframe, i == 0 || i == 6, "sample {i}");
        assert_eq!(s.description_index, 1);
    }
    // Edit list: 500 movie ticks at 600/s, media time 1001.
    let el = v.edits.as_ref().unwrap();
    assert_eq!(el.entries, vec![Edit { segment_duration: 500, media_time: 1001, media_rate: 0x0001_0000 }]);
    assert_eq!(el.first_media_time(), Some(1001));
    assert_eq!(el.initial_empty(), 0);
    assert_eq!(v.presentation_offset(600), -1001);

    let mut buf = Vec::new();
    r.read_sample(&samples[5], &mut buf).unwrap();
    assert_eq!(buf, vec![5u8; 42]);

    // tmcd: 24 fps, frame 86400 = 01:00:00:00.
    let tc = r.start_timecode(2).unwrap().unwrap();
    assert_eq!(tc.to_string(), "01:00:00:00");
    assert_eq!((tc.hours, tc.minutes, tc.seconds, tc.frames), (1, 0, 0, 0));
    assert_eq!(r.start_timecode(1).unwrap(), None);

    let a = r.movie().track(3).unwrap();
    assert_eq!(a.kind, TrackKind::Audio);
    assert_eq!(a.entries[0].codec, Codec::Pcm(Pcm { float: false, big_endian: false, signed: true, bits_per_sample: 16 }));
    let ai = a.entries[0].audio.as_ref().unwrap();
    assert_eq!((ai.channels, ai.sample_rate, ai.sample_size), (2, 48000, 16));
    assert_eq!(a.sample_count(), 4800);
    let last = a.samples().last().unwrap();
    assert_eq!(last.size, 4);
    assert_eq!(last.dts, 4799);
    let all: Vec<Sample> = a.samples().collect();
    assert_eq!(all[2400].offset, all[2399].offset + 4, "chunks are back to back here");
}

#[test]
fn moov_first_and_moov_last_give_the_same_tables() {
    let mut s = camera_mov();
    let last = build(&s);
    s.moov_at_end = false;
    let first = build(&s);
    let a = open(&last);
    let b = open(&first);
    for (ta, tb) in a.movie().tracks.iter().zip(&b.movie().tracks) {
        let sa: Vec<_> = ta.samples().map(|s| (s.size, s.dts, s.pts, s.keyframe)).collect();
        let sb: Vec<_> = tb.samples().map(|s| (s.size, s.dts, s.pts, s.keyframe)).collect();
        assert_eq!(sa, sb);
    }
}

#[test]
fn co64_and_a_64_bit_mdat_past_4_gib() {
    let mut v = TrackSpec::video(1, visual_entry(b"avc1", 640, 360, &[avcc()]), 6, 10, 2, 1000, None);
    v.co64 = true;
    let spec = MovieSpec { brand: Some(*b"isom"), moov_at_end: true, timescale: 1000, tracks: vec![v], gap: 5 << 30 };
    let built = build(&spec);
    assert!(built.len > 5 << 30);
    let mut r = Mp4Reader::new(built.reader()).unwrap();
    let samples: Vec<Sample> = r.movie().tracks[0].samples().collect();
    assert_eq!(samples.len(), 6);
    assert!(samples.iter().all(|s| s.offset > u32::MAX as u64));
    assert!(samples.iter().all(|s| s.keyframe), "no stss: every sample is a sync sample");
    let mut buf = Vec::new();
    r.read_sample(&samples[3], &mut buf).unwrap();
    assert_eq!(buf, vec![3u8; 10]);
}

#[test]
fn stz2_with_4_8_and_16_bit_fields() {
    for (field, sizes) in [(4u8, vec![1u32, 15, 7, 3, 9]), (8, vec![200, 1, 255, 17]), (16, vec![1000, 65535, 3, 4])] {
        let n = sizes.len() as u32;
        let chunk: Vec<u8> = sizes.iter().enumerate().flat_map(|(i, &s)| std::iter::repeat_n(i as u8, s as usize)).collect();
        let t = TrackSpec {
            sizes: SizeSpec::Stz2(field, sizes.clone()),
            chunks: vec![chunk],
            stsc: vec![(1, n, 1)],
            stts: vec![(n, 512)],
            duration: n as u64 * 512,
            ..TrackSpec::video(1, visual_entry(b"avc1", 64, 64, &[avcc()]), n, 1, n, 512, None)
        };
        let built = build(&MovieSpec { brand: Some(*b"mp42"), moov_at_end: false, timescale: 1000, tracks: vec![t], gap: 0 });
        let r = open(&built);
        let got: Vec<u32> = r.movie().tracks[0].samples().map(|s| s.size).collect();
        assert_eq!(got, sizes, "stz2 field size {field}");
    }
}

#[test]
fn ctts_v0_and_an_initial_empty_edit() {
    let mut v = TrackSpec::video(1, visual_entry(b"avc1", 64, 64, &[avcc()]), 4, 8, 4, 100, Some(vec![1]));
    v.timescale = 1000;
    v.ctts = Some((0, vec![(1, 200), (1, 0), (2, 100)]));
    v.elst = Some((1, vec![(300, -1), (400, 200)]));
    let built = build(&MovieSpec { brand: Some(*b"isom"), moov_at_end: false, timescale: 600, tracks: vec![v], gap: 0 });
    let r = open(&built);
    let t = &r.movie().tracks[0];
    let pts: Vec<i64> = t.samples().map(|s| s.pts).collect();
    assert_eq!(pts, vec![200, 100, 300, 400]);
    let e = t.edits.as_ref().unwrap();
    assert_eq!(e.initial_empty(), 300);
    assert_eq!(e.first_media_time(), Some(200));
    // 300 movie ticks at 600/s = 500 media ticks at 1000/s; minus 200.
    assert_eq!(t.presentation_offset(600), 300);
    assert!(!r.movie().quicktime);
}

fn single_entry_movie(handler: [u8; 4], entry: Vec<u8>) -> MovieSpec {
    let mut t = TrackSpec::video(1, entry, 2, 4, 2, 10, None);
    t.handler = handler;
    MovieSpec { brand: Some(*b"qt  "), moov_at_end: false, timescale: 600, tracks: vec![t], gap: 0 }
}

fn entry_of(spec: MovieSpec) -> SampleEntry {
    let built = build(&spec);
    let r = open(&built);
    r.movie().tracks[0].entries[0].clone()
}

#[test]
fn prores_four_ccs_are_identified() {
    for (cc, want) in [
        (b"apch", ProRes::Hq422),
        (b"apcn", ProRes::Standard422),
        (b"apcs", ProRes::Lt422),
        (b"apco", ProRes::Proxy422),
        (b"ap4h", ProRes::P4444),
        (b"ap4x", ProRes::Xq4444),
    ] {
        let e = entry_of(single_entry_movie(*b"vide", visual_entry(cc, 3840, 2160, &[colr_nclc(9, 16, 9)])));
        assert_eq!(e.codec, Codec::ProRes(want), "{}", String::from_utf8_lossy(cc));
        let v = e.video.unwrap();
        assert_eq!((v.width, v.height), (3840, 2160));
        assert_eq!(v.colour, Some(Colour::Nclx { primaries: 9, transfer: 16, matrix: 9, full_range: None }));
    }
}

#[test]
fn hevc_av1_opus_aac_and_pcm_entries() {
    let e = entry_of(single_entry_movie(*b"vide", visual_entry(b"hvc1", 1920, 1080, &[hvcc(), colr_nclx(9, 16, 9, false)])));
    let Codec::Hevc(h) = &e.codec else { panic!("hevc") };
    assert_eq!((h.profile_idc, h.level_idc, h.bit_depth_luma, h.chroma_format, h.length_size), (2, 120, 10, 1, 4));
    assert_eq!(h.arrays.len(), 3);
    assert_eq!(e.bit_depth, Some(10));
    assert_eq!(e.video.unwrap().colour, Some(Colour::Nclx { primaries: 9, transfer: 16, matrix: 9, full_range: Some(false) }));

    let seq = av1_sequence_header_obu(9, 1919, 1079);
    let cfg = Av1Config::from_sequence_header(&seq).unwrap();
    let e = entry_of(single_entry_movie(*b"vide", visual_entry(b"av01", 1920, 1080, &[bx(b"av1C", &cfg.to_bytes())])));
    let Codec::Av1(a) = &e.codec else { panic!("av1") };
    assert_eq!(a, &cfg);
    assert_eq!(a.codec_string(), "av01.0.09M.08");
    let sh = a.sequence_header().unwrap().unwrap();
    assert_eq!((sh.max_frame_width, sh.max_frame_height, sh.bit_depth), (1920, 1080, 8));

    let mut opus = single_entry_movie(*b"soun", audio_entry(b"Opus", 2, 16, 48000, &[dops(312)]));
    opus.brand = Some(*b"iso6");
    let e = entry_of(opus);
    let Codec::Opus(o) = &e.codec else { panic!("opus") };
    assert_eq!((o.output_channel_count, o.pre_skip, o.input_sample_rate, o.channel_mapping_family), (2, 312, 48000, 0));

    let e = entry_of(single_entry_movie(*b"soun", audio_entry(b"mp4a", 2, 16, 44100, &[esds_aac()])));
    let Codec::Mp4Audio(a) = &e.codec else { panic!("aac") };
    assert_eq!((a.object_type_indication, a.audio_object_type, a.sample_rate, a.channel_configuration), (0x40, Some(2), Some(44100), Some(2)));
    assert_eq!(a.decoder_specific, vec![0x12, 0x10]);
    assert_eq!(e.audio.unwrap().sample_rate, 44100);

    // QuickTime: esds inside wave.
    let wave = bx(b"wave", &cat(&[&bx(b"frma", b"mp4a"), &esds_aac()]));
    let e = entry_of(single_entry_movie(*b"soun", audio_entry(b"mp4a", 2, 16, 44100, &[wave])));
    assert!(matches!(e.codec, Codec::Mp4Audio(_)));

    let e = entry_of(single_entry_movie(*b"soun", audio_entry_qt_v2(b"lpcm", 96000.0, 6, 24, 0x0c)));
    assert_eq!(e.codec, Codec::Pcm(Pcm { float: false, big_endian: false, signed: true, bits_per_sample: 24 }));
    let ai = e.audio.unwrap();
    assert_eq!((ai.channels, ai.sample_rate, ai.sample_size, ai.qt_version), (6, 96000, 24, 2));
    assert_eq!(e.bit_depth, Some(24));

    let e = entry_of(single_entry_movie(*b"soun", audio_entry(b"twos", 1, 16, 44100, &[])));
    assert_eq!(e.codec, Codec::Pcm(Pcm { float: false, big_endian: true, signed: true, bits_per_sample: 16 }));

    let pcmc = full(b"pcmC", 0, 0, &[1, 32]);
    let e = entry_of(single_entry_movie(*b"soun", audio_entry(b"fpcm", 2, 32, 48000, &[pcmc])));
    assert_eq!(e.codec, Codec::Pcm(Pcm { float: true, big_endian: false, signed: false, bits_per_sample: 32 }));
    let pcmc = full(b"pcmC", 0, 0, &[0, 24]);
    let e = entry_of(single_entry_movie(*b"soun", audio_entry(b"ipcm", 2, 24, 48000, &[pcmc])));
    assert_eq!(e.codec, Codec::Pcm(Pcm { float: false, big_endian: true, signed: true, bits_per_sample: 24 }));

    let e = entry_of(single_entry_movie(*b"vide", visual_entry(b"xyzw", 8, 8, &[])));
    assert_eq!(e.codec, Codec::Unknown);
}

#[test]
fn drop_frame_timecode_at_one_hour() {
    let mut s = camera_mov();
    // 29.97 drop frame: 01:00:00;00 is frame 107,892.
    s.tracks[1].entries = vec![tmcd_entry(1, 30000, 1001, 30)];
    s.tracks[1].chunks = vec![107_892u32.to_be_bytes().to_vec()];
    let built = build(&s);
    let mut r = open(&built);
    let tc = r.start_timecode(2).unwrap().unwrap();
    assert_eq!(tc.to_string(), "01:00:00;00");
    let e = TimecodeEntry { flags: 1, timescale: 30000, frame_duration: 1001, number_of_frames: 30 };
    assert_eq!(Timecode::from_frame_number(1800, &e).to_string(), "00:01:00;02");
    assert_eq!(Timecode::from_frame_number(17982, &e).to_string(), "00:10:00;00");
    let nd = TimecodeEntry { flags: 0, timescale: 25, frame_duration: 1, number_of_frames: 25 };
    assert_eq!(Timecode::from_frame_number(90_000 + 24, &nd).to_string(), "01:00:00:24");
}

fn len_prefixed(nals: &[&[u8]], ls: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for n in nals {
        let l = (n.len() as u32).to_be_bytes();
        v.extend_from_slice(&l[4 - ls..]);
        v.extend_from_slice(n);
    }
    v
}

#[test]
fn h264_to_annex_b() {
    let cfg = AvcConfig::parse(&avcc()[8..]).unwrap();
    let sei: &[u8] = &[0x06, 0x05, 0x01, 0x80];
    let idr: &[u8] = &[0x65, 0x88, 0x84, 0x00];
    let idr2: &[u8] = &[0x65, 0x11];
    let p: &[u8] = &[0x41, 0x9a, 0x02];
    let sc: &[u8] = &[0, 0, 0, 1];
    let mut out = Vec::new();

    avcc_to_annexb(&len_prefixed(&[sei, idr, idr2], 4), &cfg, &mut out).unwrap();
    assert_eq!(out, cat(&[sc, sei, sc, SPS, sc, PPS, sc, idr, sc, idr2]));

    avcc_to_annexb(&len_prefixed(&[p], 4), &cfg, &mut out).unwrap();
    assert_eq!(out, cat(&[sc, p]));

    // In-band parameter sets: nothing added.
    avcc_to_annexb(&len_prefixed(&[SPS, PPS, idr], 4), &cfg, &mut out).unwrap();
    assert_eq!(out, cat(&[sc, SPS, sc, PPS, sc, idr]));

    let mut two = cfg.clone();
    two.length_size = 2;
    avcc_to_annexb(&len_prefixed(&[idr, &[]], 2), &two, &mut out).unwrap();
    assert_eq!(out, cat(&[sc, SPS, sc, PPS, sc, idr]), "an empty NAL is dropped");

    let bad = cat(&[&[0, 0, 0, 9], idr]);
    assert_eq!(avcc_to_annexb(&bad, &cfg, &mut out), Err(Error::Truncated(FourCC(*b"mdat"))));
    assert_eq!(avcc_to_annexb(&[0, 0], &cfg, &mut out), Err(Error::Truncated(FourCC(*b"mdat"))));
    let mut three = cfg.clone();
    three.length_size = 3;
    assert!(matches!(avcc_to_annexb(&[], &three, &mut out), Err(Error::Invalid { .. })));
}

#[test]
fn avcc_round_trips_through_its_bytes() {
    let cfg = AvcConfig::parse(&avcc()[8..]).unwrap();
    assert_eq!(cfg.to_bytes().unwrap(), avcc()[8..].to_vec());
    assert_eq!((cfg.chroma_format, cfg.bit_depth_luma, cfg.bit_depth_chroma), (Some(1), Some(8), Some(8)));
}

#[test]
fn av1_config_comes_from_the_stream_header() {
    for (level, want) in [(1u8, "av01.0.01M.08"), (8, "av01.0.08M.08"), (9, "av01.0.09M.08")] {
        let obu = av1_sequence_header_obu(level, 1279, 719);
        let cfg = Av1Config::from_sequence_header(&obu).unwrap();
        assert_eq!(cfg.codec_string(), want);
        assert_eq!(cfg.config_obus, obu, "already has a size field: stored as is");
        assert!(cfg.chroma_subsampling_x && cfg.chroma_subsampling_y && !cfg.monochrome);
        assert_eq!(Av1Config::parse(&cfg.to_bytes()).unwrap(), cfg);
    }
    // Without the size field: stored with one.
    let obu = av1_sequence_header_obu(8, 639, 359);
    let bare: Vec<u8> = [&[0x08u8][..], &obu[2..]].concat();
    let cfg = Av1Config::from_sequence_header(&bare).unwrap();
    assert_eq!(cfg.config_obus, obu);
    assert!(Av1Config::from_sequence_header(&[0x12, 0x00]).is_err(), "a temporal delimiter is not a sequence header");
}
