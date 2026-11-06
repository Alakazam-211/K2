//! H.264 through Media Foundation, on a real Windows machine. These need
//! Windows with Media Foundation's H.264 (not an N or KN edition without
//! the Media Feature Pack, nor a Windows Server without its Media
//! Foundation feature); they fail there rather than skip.
//! `cargo test -p kessel-video --lib win::h264_tests -- --nocapture` prints
//! what was found and how long frames took.

use super::*;
use crate::api::no_wake;
use crate::convert::psnr_y;
use crate::frame::Planes;
use crate::h264;
use crate::kit::wait_for;
use std::time::Instant;

/// A moving NV12 test picture: smooth gradients and a bright square
/// (`detail`: a fine pseudo-random texture panning instead, which costs
/// bits).
fn picture(w: u32, h: u32, t: u32, detail: bool) -> VideoFrame {
    let (wu, hu) = (w as usize, h as usize);
    let mut buf = vec![0u8; PixelFormat::Nv12.frame_len(w, h)];
    let (luma, chroma) = buf.split_at_mut(wu * hu);
    let side = (h / 4) as usize;
    let (sx, sy) = (
        ((t * 7) % (w - h / 4)) as usize,
        ((t * 3) % (h - h / 4)) as usize,
    );
    for y in 0..hu {
        for x in 0..wu {
            luma[y * wu + x] = if detail {
                let (px, py) = ((x + t as usize * 3) as u32, y as u32);
                let n = px.wrapping_mul(2_654_435_761) ^ py.wrapping_mul(40_503);
                (n >> 24) as u8 / 2 + 64
            } else if (sx..sx + side).contains(&x) && (sy..sy + side).contains(&y) {
                235
            } else {
                (16 + (x * 200 / wu + y * 20 / hu) % 220) as u8
            };
        }
    }
    for y in 0..hu.div_ceil(2) {
        for x in 0..wu.div_ceil(2) {
            chroma[y * wu + 2 * x] = (64 + x * 128 / wu) as u8;
            chroma[y * wu + 2 * x + 1] = (192 - y * 128 / hu) as u8;
        }
    }
    VideoFrame::from_planes(
        Planes::tight(PixelFormat::Nv12, w, h, buf).expect("planes"),
        w,
        h,
        1_000_000 + t as u64 * 33_333,
    )
    .expect("frame")
}

/// Submit one frame and wait for its output.
fn encode_one(enc: &mut Box<dyn VideoEncoder>, f: &VideoFrame) -> EncodedFrame {
    enc.submit(f, EncodeOptions::default()).expect("submit");
    wait_for(Duration::from_secs(5), || enc.poll())
        .expect("an encoded frame in 5 s")
        .expect("encoded")
}

/// Submit one access unit and wait for its picture.
fn decode_one(dec: &mut Box<dyn VideoDecoder>, e: &EncodedFrame) -> VideoFrame {
    dec.submit(EncodedInput::new(
        VideoCodec::H264,
        e.data.clone(),
        e.keyframe,
        e.capture_us,
    ))
    .expect("submit");
    wait_for(Duration::from_secs(5), || dec.poll())
        .expect("a decoded frame in 5 s")
        .expect("decoded")
}

/// The NAL unit types of an access unit, checking its framing: every NAL
/// after a 4-byte start code, nothing before the first.
fn nal_types(data: &[u8]) -> Vec<u8> {
    assert!(
        data.starts_with(&h264::START_CODE),
        "{:02x?}",
        &data[..8.min(data.len())]
    );
    let nals = h264::annexb_nals(data);
    let mut rebuilt = Vec::new();
    for n in &nals {
        h264::push_annexb(&mut rebuilt, n);
    }
    assert_eq!(rebuilt, data, "only 4-byte start codes");
    nals.iter()
        .map(|n| h264::nal_type(n).expect("type"))
        .collect()
}

#[test]
fn the_probe_lists_what_opens_and_sessions_report_their_kind() {
    let b = backend().expect("backend");
    let caps = b.capabilities();
    let p = h264_probe();
    eprintln!("H.264 on this machine: {p:#?}");
    let h264: Vec<_> = caps
        .codecs
        .iter()
        .filter(|c| c.codec == VideoCodec::H264)
        .collect();
    for c in &h264 {
        eprintln!(
            "listed: {:?} {:?} {} up to {}x{}",
            c.direction, c.implementation, c.name, c.max_width, c.max_height
        );
    }
    assert!(
        p.software_encoder.is_some() && p.software_decoder.is_some(),
        "Microsoft's software H.264 MFTs are part of Windows here"
    );
    for (imp, pref) in [
        (Implementation::Hardware, ImplPreference::HardwareOnly),
        (Implementation::Software, ImplPreference::SoftwareOnly),
    ] {
        let listed_enc = h264
            .iter()
            .any(|c| c.direction == CodecDirection::Encode && c.implementation == imp);
        let mut cfg = EncoderConfig::h264(640, 360, 30, 600_000);
        cfg.implementation = pref;
        match b.encoder(cfg, no_wake()) {
            Ok(e) => {
                assert!(listed_enc, "{imp:?} opened but isn't listed");
                assert_eq!(e.info().implementation, imp, "{}", e.info().name);
                eprintln!("{pref:?} encoder: {}", e.info().name);
            }
            Err(VideoError::Unsupported(why)) => {
                assert!(!listed_enc, "{imp:?} listed but refused: {why}");
                eprintln!("{pref:?} encoder: none ({why})");
            }
            Err(e) => panic!("{pref:?}: {e}"),
        }
        let listed_dec = h264
            .iter()
            .any(|c| c.direction == CodecDirection::Decode && c.implementation == imp);
        let mut dcfg = DecoderConfig::new(VideoCodec::H264);
        dcfg.implementation = pref;
        match b.decoder(dcfg, no_wake()) {
            Ok(d) => {
                assert!(listed_dec, "{imp:?} decoder opened but isn't listed");
                assert_eq!(d.stats().implementation, Some(imp));
            }
            Err(VideoError::Unsupported(why)) => {
                assert!(!listed_dec, "{imp:?} decoder listed but refused: {why}");
                eprintln!("{pref:?} decoder: none ({why})");
            }
            Err(e) => panic!("{pref:?} decoder: {e}"),
        }
    }
    // The default open takes hardware when it is there.
    let e = b
        .encoder(EncoderConfig::h264(640, 360, 30, 600_000), no_wake())
        .expect("an H.264 encoder");
    let want = if p.hardware_encoder.is_some() {
        Implementation::Hardware
    } else {
        Implementation::Software
    };
    assert_eq!(e.info().implementation, want);
}

#[test]
fn frames_round_trip_at_360p_and_720p() {
    let b = backend().expect("backend");
    for (w, h, bps) in [(640, 360, 800_000), (1280, 720, 2_000_000)] {
        let mut enc = b
            .encoder(EncoderConfig::h264(w, h, 30, bps), no_wake())
            .expect("encoder");
        let mut dec = b
            .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
            .expect("decoder");
        let (mut enc_us, mut dec_us, mut bytes) = (0u64, 0u64, 0usize);
        let mut worst = f64::MAX;
        let n = 30;
        for t in 0..n {
            let src = picture(w, h, t, false);
            let e = encode_one(&mut enc, &src);
            assert_eq!((e.width, e.height, e.capture_us), (w, h, src.capture_us));
            let types = nal_types(&e.data);
            if t == 0 {
                assert!(e.keyframe, "the first frame is a keyframe");
                assert_eq!(&types[..2], &[h264::NAL_SPS, h264::NAL_PPS], "{types:?}");
                let nals = h264::annexb_nals(&e.data);
                let codec = h264::codec_string(&nals).expect("codec string");
                eprintln!("{w}x{h}: {codec}, {}", enc.info().name);
                // Baseline (66), as WebRTC needs, unless High was asked for.
                assert_eq!(nals[0][1], 66, "profile_idc of {codec}");
            } else {
                assert!(!e.keyframe, "frame {t}: keyframes only on request");
                assert!(!types.contains(&h264::NAL_IDR), "{types:?}");
            }
            assert!(types.contains(&h264::NAL_IDR) || types.contains(&h264::NAL_SLICE));
            enc_us += e.encode_us as u64;
            bytes += e.data.len();
            let t0 = Instant::now();
            let out = decode_one(&mut dec, &e);
            dec_us += t0.elapsed().as_micros() as u64;
            assert_eq!((out.width, out.height), (w, h));
            assert_eq!(out.capture_us, src.capture_us);
            assert_eq!(out.format(), PixelFormat::I420);
            let q = psnr_y(&src, &out).expect("psnr");
            worst = worst.min(q);
        }
        eprintln!(
            "{w}x{h}: encode {} µs, decode {} µs a frame on average, {} bytes a frame, worst luma PSNR {worst:.1} dB",
            enc_us / n as u64,
            dec_us / n as u64,
            bytes / n as usize
        );
        assert!(worst > 30.0, "{w}x{h}: {worst:.1} dB");
        assert_eq!(enc.stats().frames_out, n as u64);
        assert_eq!(dec.stats().frames_out, n as u64);
    }
}

#[test]
fn size_changes_mid_stream_decode_at_each_new_size() {
    // One decoder, then IDRs with new SPSes: a strip 48 rows high (the
    // least Microsoft's encoder makes), a picture 45 times taller, smaller,
    // larger, the smallest. Each change must be read with its own layout,
    // never the cached one: a small frame's buffer read with a tall
    // frame's layout would run far past it.
    let b = backend().expect("backend");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    for (w, h) in [(1280, 48), (1280, 2160), (320, 180), (1920, 1080), (48, 48)] {
        let bps = (w * h * 2).max(100_000);
        let mut enc = b
            .encoder(EncoderConfig::h264(w, h, 30, bps), no_wake())
            .unwrap_or_else(|e| panic!("{w}x{h} encoder: {e}"));
        for t in 0..3 {
            let src = picture(w, h, t, false);
            let e = encode_one(&mut enc, &src);
            assert_eq!(e.keyframe, t == 0);
            let out = decode_one(&mut dec, &e);
            assert_eq!((out.width, out.height), (w, h), "frame {t}");
            let q = psnr_y(&src, &out).expect("psnr");
            assert!(q > 30.0, "{w}x{h} frame {t}: {q:.1} dB");
        }
    }
}

#[test]
fn constrained_high_round_trips_with_no_b_frames() {
    let b = backend().expect("backend");
    let mut cfg = EncoderConfig::h264(640, 360, 30, 800_000);
    cfg.codec = CodecConfig::H264 {
        profile: H264Profile::ConstrainedHigh,
        level: H264Level::L3_1,
    };
    let mut enc = b.encoder(cfg, no_wake()).expect("encoder");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    for t in 0..10 {
        let src = picture(640, 360, t, false);
        let e = encode_one(&mut enc, &src);
        if t == 0 {
            let nals = h264::annexb_nals(&e.data);
            let codec = h264::codec_string(&nals).expect("codec string");
            eprintln!("Constrained High: {codec}, {}", enc.info().name);
            assert_eq!(nals[0][1], 100, "profile_idc of {codec}");
        }
        // No reordering: each output is the frame just given.
        let out = decode_one(&mut dec, &e);
        assert_eq!(out.capture_us, src.capture_us);
        assert!(psnr_y(&src, &out).expect("psnr") > 30.0);
    }
}

#[test]
fn a_bitrate_change_takes_effect_without_a_keyframe() {
    let b = backend().expect("backend");
    let (w, h) = (640, 360);
    let mut enc = b
        .encoder(EncoderConfig::h264(w, h, 30, 1_500_000), no_wake())
        .expect("encoder");
    let sizes = |enc: &mut Box<dyn VideoEncoder>, from: u32, n: u32| -> Vec<(usize, bool)> {
        (from..from + n)
            .map(|t| {
                let e = encode_one(enc, &picture(w, h, t, true));
                (e.data.len(), e.keyframe)
            })
            .collect()
    };
    let high = sizes(&mut enc, 0, 60);
    enc.set_rates(Rates::new(250_000, 30)).expect("set_rates");
    let low = sizes(&mut enc, 60, 60);
    assert!(
        low.iter().all(|(_, key)| !key),
        "a rate change never brings a keyframe"
    );
    assert_eq!(enc.info().config.bitrate_bps, 250_000);
    // Leave the first frames of each half: the IDR, and rate control
    // settling after the change.
    let avg = |v: &[(usize, bool)]| v.iter().map(|(n, _)| n).sum::<usize>() / v.len();
    let (before, after) = (avg(&high[10..]), avg(&low[30..]));
    eprintln!(
        "1.5 Mbit/s: {before} bytes a frame ({} kbit/s); 250 kbit/s: {after} bytes a frame ({} kbit/s)",
        before * 8 * 30 / 1000,
        after * 8 * 30 / 1000
    );
    assert!(after * 2 < before, "{after} vs {before} bytes a frame");
    // Roughly on target: within 2x of 250 kbit/s.
    assert!(after * 8 * 30 < 500_000, "{} bit/s", after * 8 * 30);
}

#[test]
fn a_requested_keyframe_comes_next_with_its_parameter_sets() {
    let b = backend().expect("backend");
    let (w, h) = (640, 360);
    let mut enc = b
        .encoder(EncoderConfig::h264(w, h, 30, 600_000), no_wake())
        .expect("encoder");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    let mut keys = Vec::new();
    for t in 0..20 {
        if t == 10 || t == 11 {
            // Twice in a row coalesces into one.
            enc.request_keyframe();
            enc.request_keyframe();
        }
        let e = encode_one(&mut enc, &picture(w, h, t, false));
        if e.keyframe {
            keys.push(t);
            let types = nal_types(&e.data);
            assert_eq!(&types[..2], &[h264::NAL_SPS, h264::NAL_PPS], "{types:?}");
            assert!(types.contains(&h264::NAL_IDR));
            // A decoder that starts at this keyframe shows it.
            dec.reset();
            assert_eq!(decode_one(&mut dec, &e).width, w);
        }
    }
    assert_eq!(keys, vec![0, 10, 11]);
    assert_eq!(enc.stats().keyframes, 3);
}

#[test]
fn a_new_size_starts_with_a_keyframe() {
    let b = backend().expect("backend");
    let mut enc = b
        .encoder(EncoderConfig::h264(640, 360, 30, 600_000), no_wake())
        .expect("encoder");
    let first = encode_one(&mut enc, &picture(640, 360, 0, false));
    assert!(first.keyframe);
    assert!(!encode_one(&mut enc, &picture(640, 360, 1, false)).keyframe);
    enc.set_size(320, 180).expect("set_size");
    let e = encode_one(&mut enc, &picture(640, 360, 2, false));
    assert!(e.keyframe);
    assert_eq!((e.width, e.height), (320, 180));
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    let out = decode_one(&mut dec, &e);
    assert_eq!((out.width, out.height), (320, 180));
}

#[test]
fn a_delta_before_any_keyframe_is_refused_and_garbage_fails_loudly() {
    let b = backend().expect("backend");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    let mut delta = Vec::new();
    h264::push_annexb(&mut delta, &[0x41, 0x9a, 0x00, 0x10]);
    assert_eq!(
        dec.submit(EncodedInput::new(VideoCodec::H264, delta, false, 0)),
        Err(crate::error::DecodeError::NeedKeyframe)
    );
    // An "IDR" of junk: an error or nothing, never a picture.
    let mut junk = Vec::new();
    h264::push_annexb(&mut junk, &[0x67, 0x42, 0xe0, 0x1f, 0xff, 0xff]);
    h264::push_annexb(&mut junk, &[0x68, 0xff]);
    h264::push_annexb(&mut junk, &[0x65, 0xff, 0xff, 0xff, 0xff, 0x00, 0x13]);
    dec.submit(EncodedInput::new(VideoCodec::H264, junk, true, 0))
        .expect("submit");
    let got = wait_for(Duration::from_millis(500), || dec.poll());
    assert!(!matches!(got, Some(Ok(_))), "junk gave a picture");
}

#[test]
fn access_units_get_four_byte_start_codes_and_parameter_sets_first() {
    // An encoder's output with an AUD, a 3-byte start code and in-band
    // parameter sets after an SEI.
    let raw: &[u8] = &[
        0, 0, 0, 1, 0x09, 0xf0, // AUD
        0, 0, 1, 0x06, 0x05, 0x01, // SEI, 3-byte start code
        0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1e, // SPS
        0, 0, 0, 1, 0x68, 0xce, // PPS
        0, 0, 0, 1, 0x65, 0x88, 0x84, // IDR
    ];
    let nals = h264::annexb_nals(raw);
    let sps = [0x67, 0x42, 0xc0, 0x1e];
    let pps = [0x68, 0xce];
    let au = h264_enc::access_unit(&nals, Some((&sps, &pps)));
    assert_eq!(
        au,
        [
            &[0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1e][..],
            &[0, 0, 0, 1, 0x68, 0xce],
            &[0, 0, 0, 1, 0x06, 0x05, 0x01],
            &[0, 0, 0, 1, 0x65, 0x88, 0x84],
        ]
        .concat()
    );
    let delta = h264::annexb_nals(&[0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x41, 0x9a]);
    assert_eq!(
        h264_enc::access_unit(&delta, None),
        [0, 0, 0, 1, 0x41, 0x9a]
    );
}

#[test]
fn opening_h264_leaves_the_callers_thread_free_for_a_ui_apartment() {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    let b = backend().expect("backend");
    assert!(b
        .capabilities()
        .codecs
        .iter()
        .any(|c| c.codec == VideoCodec::H264));
    let mut enc = b
        .encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake())
        .expect("encoder");
    let mut dec = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    let e = encode_one(&mut enc, &picture(320, 180, 0, false));
    assert_eq!(decode_one(&mut dec, &e).width, 320);
    // A webview's thread joins a single-threaded apartment; that fails on
    // a thread already in the multithreaded one.
    // SAFETY: COM on this test's own thread, undone below.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    assert!(hr.is_ok(), "{hr:?}");
    // SAFETY: balances the successful call above.
    unsafe { CoUninitialize() };
}

#[test]
fn the_software_encoder_gives_each_frame_out_without_a_drain() {
    // Low latency: one input, its access unit straight back. A drain per
    // frame would work but costs time.
    let found = mft::find(mft::Role::Encoder, ImplPreference::SoftwareOnly).expect("find");
    let f = found.first().expect("Microsoft's software encoder");
    let mut enc = h264_enc::MfEncoder::open_one(f, &EncoderConfig::h264(640, 360, 30, 600_000))
        .expect("open");
    for t in 0..10u32 {
        let p = picture(640, 360, t, false);
        let s = mft::sample_from(
            &p.planes().expect("planes").data,
            t as i64 * 333_333,
            333_333,
        )
        .expect("sample");
        let outs = enc
            .mft
            .process(&s, mft::NoOutput::Wait, &mut |_| Ok(()))
            .expect("process");
        assert_eq!(outs.len(), 1, "frame {t}");
    }
}

/// The DLLs a PE file (this test binary) imports at load time.
fn pe_imports(pe: &[u8]) -> Vec<String> {
    let u16_at = |o: usize| u16::from_le_bytes([pe[o], pe[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(pe[o..o + 4].try_into().expect("4 bytes")) as usize;
    let nt = u32_at(0x3c);
    assert_eq!(&pe[nt..nt + 4], b"PE\0\0");
    let coff = nt + 4;
    let sections = u16_at(coff + 2);
    let opt = coff + 20;
    assert_eq!(u16_at(opt), 0x20b, "PE32+");
    // Data directory 1 (imports) of a PE32+ optional header.
    let import_rva = u32_at(opt + 112 + 8);
    let table = opt + u16_at(coff + 16);
    let to_file = |rva: usize| {
        (0..sections)
            .map(|i| table + i * 40)
            .find_map(|s| {
                let (va, size, raw) = (
                    u32_at(s + 12),
                    u32_at(s + 8).max(u32_at(s + 16)),
                    u32_at(s + 20),
                );
                (va..va + size).contains(&rva).then(|| rva - va + raw)
            })
            .expect("an RVA inside a section")
    };
    let mut out = Vec::new();
    let mut d = to_file(import_rva);
    while u32_at(d + 12) != 0 {
        let name = to_file(u32_at(d + 12));
        let end = pe[name..].iter().position(|&b| b == 0).expect("NUL") + name;
        out.push(String::from_utf8_lossy(&pe[name..end]).to_lowercase());
        d += 20;
    }
    out
}

#[test]
fn media_foundation_is_not_imported_so_n_editions_start() {
    let exe = std::fs::read(std::env::current_exe().expect("exe")).expect("read");
    let imports = pe_imports(&exe);
    assert!(imports.iter().any(|d| d == "kernel32.dll"), "{imports:?}");
    for dll in ["mfplat.dll", "mf.dll", "mfreadwrite.dll"] {
        assert!(!imports.iter().any(|d| d == dll), "{dll} in {imports:?}");
    }
}

#[test]
fn a_missing_system_dll_is_none_not_a_crash() {
    assert!(mfdll::load("kessel-video-no-such-library.dll").is_none());
    assert!(
        mfdll::load("mfplat.dll").is_some(),
        "Media Foundation is here"
    );
    assert!(mfdll::mfplat().is_ok());
}
