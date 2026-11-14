//! Seed files for the fuzz targets (scripts/check-fuzz.sh), generated in
//! code from the test fixtures (tests/common), so the fuzzer starts from
//! inputs the reader takes: a QuickTime camera original with moov at the
//! end, moov first, a co64/stz2 MP4, ProRes, HEVC, the audio entries, and
//! fMP4 proxies (AV1 + Opus + H.264) from the writer. Plus Annex B seeds.
//! `cargo run -p k2-isobmff --example fuzz_seeds -- <dir>` writes
//! `<dir>/read/*.mp4` and `<dir>/annexb/*.bin`.

#[path = "../tests/common/mod.rs"]
mod common;

use common::*;
use k2_isobmff::*;
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "fuzz-seeds".into());
    let read = Path::new(&dir).join("read");
    let annexb = Path::new(&dir).join("annexb");
    std::fs::create_dir_all(&read).expect("seed folder");
    std::fs::create_dir_all(&annexb).expect("seed folder");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    let mut cam = camera_mov();
    cam.tracks[2].duration = 96;
    cam.tracks[2].stts = vec![(96, 1)];
    cam.tracks[2].stsc = vec![(1, 48, 1)];
    cam.tracks[2].sizes = SizeSpec::Constant(4, 96);
    cam.tracks[2].chunks = vec![vec![0x11; 192], vec![0x22; 192]];
    files.push(("camera-moov-last.mov".into(), build(&cam).to_vec()));
    cam.moov_at_end = false;
    cam.tracks[1].entries = vec![tmcd_entry(1, 30000, 1001, 30)];
    files.push(("camera-moov-first-df.mov".into(), build(&cam).to_vec()));

    let mut t = TrackSpec::video(1, visual_entry(b"avc1", 64, 64, &[avcc()]), 9, 6, 3, 512, Some(vec![1, 4]));
    t.co64 = true;
    t.sizes = SizeSpec::Stz2(8, vec![6, 7, 8, 6, 7, 8, 6, 7, 8]);
    t.ctts = Some((0, vec![(9, 512)]));
    t.elst = Some((1, vec![(100, -1), (900, 512)]));
    files.push(("co64-stz2.mp4".into(), build(&MovieSpec { brand: Some(*b"isom"), moov_at_end: false, timescale: 1000, tracks: vec![t], gap: 0 }).to_vec()));

    let one = |name: &str, handler: [u8; 4], entry: Vec<u8>| {
        let mut t = TrackSpec::video(1, entry, 3, 5, 3, 10, None);
        t.handler = handler;
        (name.to_string(), build(&MovieSpec { brand: Some(*b"qt  "), moov_at_end: true, timescale: 600, tracks: vec![t], gap: 0 }).to_vec())
    };
    files.push(one("prores.mov", *b"vide", visual_entry(b"apch", 1920, 1080, &[colr_nclc(1, 1, 1), pasp(1, 1)])));
    files.push(one("hevc.mp4", *b"vide", visual_entry(b"hvc1", 1920, 1080, &[hvcc(), colr_nclx(9, 16, 9, true)])));
    let av1 = Av1Config::from_sequence_header(&av1_sequence_header_obu(8, 1279, 719)).unwrap();
    files.push(one("av1.mp4", *b"vide", visual_entry(b"av01", 1280, 720, &[bx(b"av1C", &av1.to_bytes())])));
    files.push(one("aac.mov", *b"soun", audio_entry(b"mp4a", 2, 16, 44100, &[bx(b"wave", &esds_aac())])));
    files.push(one("lpcm.mov", *b"soun", audio_entry_qt_v2(b"lpcm", 48000.0, 2, 24, 0x0c)));
    files.push(one("ipcm.mp4", *b"soun", audio_entry(b"ipcm", 2, 24, 48000, &[full(b"pcmC", 0, 0, &[0, 24])])));
    files.push(one("opus.mp4", *b"soun", audio_entry(b"Opus", 2, 16, 48000, &[dops(312)])));

    // fMP4 proxies from the writer.
    let opus = OpusConfig { output_channel_count: 2, pre_skip: 312, input_sample_rate: 48000, output_gain: 0, channel_mapping_family: 0, channel_mapping: None };
    let tracks = vec![
        TrackSetup { track_id: 1, timescale: 60000, codec: TrackCodec::Av1(av1), width: 1280, height: 720, pixel_aspect: Some((1, 1)), colour: None, language: *b"und" },
        TrackSetup { track_id: 2, timescale: 48000, codec: TrackCodec::Opus(opus), width: 0, height: 0, pixel_aspect: None, colour: None, language: *b"und" },
        TrackSetup { track_id: 3, timescale: 30000, codec: TrackCodec::Avc(AvcConfig::parse(&avcc()[8..]).unwrap()), width: 640, height: 360, pixel_aspect: None, colour: None, language: *b"und" },
    ];
    let mut w = Fmp4Writer::new(InitConfig { movie_timescale: 1000, creation_time: 0, tracks }).unwrap();
    let mut f = w.init_segment().unwrap();
    for seg in 0..2u64 {
        let v: Vec<FragmentSample> = (0..4).map(|i| FragmentSample { duration: 1001, size: 8 + i, keyframe: i == 0, composition_offset: [0, 2002, -1001, 1001][i as usize] }).collect();
        let a: Vec<FragmentSample> = (0..3).map(|i| FragmentSample { duration: 960, size: 5 + i, keyframe: true, composition_offset: 0 }).collect();
        let vd = vec![0x5a; v.iter().map(|s| s.size as usize).sum()];
        let ad = vec![0xa5; a.iter().map(|s| s.size as usize).sum()];
        let frags = [
            TrackFragment { track_id: 1, base_media_decode_time: seg * 4004, samples: &v, data: &vd },
            TrackFragment { track_id: 2, base_media_decode_time: seg * 2880, samples: &a, data: &ad },
            TrackFragment { track_id: 3, base_media_decode_time: seg * 4004, samples: &v, data: &vd },
        ];
        f.extend(w.media_segment(&frags, SegmentOptions { styp: true, sidx: seg == 0 }).unwrap());
    }
    files.push(("proxy.mp4".into(), f));

    for (name, bytes) in &files {
        std::fs::write(read.join(name), bytes).expect("write a seed");
    }
    let nal = |n: &[u8], ls: usize| {
        let l = (n.len() as u32).to_be_bytes();
        [&l[4 - ls..], n].concat()
    };
    let idr = [0x65u8, 0x88, 0x84, 0x00, 0x10];
    let seeds: Vec<(&str, Vec<u8>)> = vec![
        ("idr4.bin", [&[2u8][..], &nal(&[0x06, 0x05, 0x01], 4), &nal(&idr, 4)].concat()),
        ("p1.bin", [&[0u8][..], &nal(&[0x41, 0x9a], 1), &nal(&[0x41, 0x9b], 1)].concat()),
        ("inband2.bin", [&[1u8][..], &nal(SPS, 2), &nal(PPS, 2), &nal(&idr, 2)].concat()),
        ("avcc.bin", avcc()[7..].to_vec()),
    ];
    for (name, bytes) in seeds {
        std::fs::write(annexb.join(name), bytes).expect("write a seed");
    }
    eprintln!("[fuzz seeds] {} read seeds and 4 annexb seeds in {dir}", files.len());
}
