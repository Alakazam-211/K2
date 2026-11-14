//! File-mode decoding through VideoToolbox: a clip with B-frames from
//! VideoToolbox's own encoder (frame reordering allowed, so no third-party
//! fixture), decoded from decode order into presentation order.

use super::ffi::*;
use super::pixel::fill_nv12;
use super::*;
use crate::api::no_wake;
use crate::error::DecodeError;
use crate::fake::{pattern_frame, read_frame_code};
use crate::frame::FrameBuffer;
use crate::kit::wait_for;
use std::ffi::c_void;
use std::ptr;

/// One encoded sample: Annex B, keyframe, pts and dts in µs.
type Encoded = (Vec<u8>, bool, Option<u64>, Option<u64>);

fn us(t: CMTime) -> Option<u64> {
    if t.flags & CM_TIME_VALID == 0 || t.timescale <= 0 {
        return None;
    }
    u64::try_from(t.value as i128 * 1_000_000 / t.timescale as i128).ok()
}

extern "C" fn on_clip(
    refcon: *mut c_void,
    _frame: *mut c_void,
    status: OSStatus,
    _flags: u32,
    sb: CMSampleBufferRef,
) {
    // SAFETY: refcon is the test's boxed Mutex, alive until the session is
    // invalidated.
    let out = unsafe { &*(refcon as *const Mutex<Vec<Encoded>>) };
    if status != 0 || sb.is_null() {
        return;
    }
    // SAFETY: a live sample buffer for the duration of the callback.
    let (frame, pts, dts) = unsafe {
        (
            super::vt_enc::sample_to_annexb(sb),
            CMSampleBufferGetPresentationTimeStamp(sb),
            CMSampleBufferGetDecodeTimeStamp(sb),
        )
    };
    if let Ok(f) = frame {
        lock(out).push((f.data.to_vec(), f.keyframe, us(pts), us(dts)));
    }
}

/// The pts of frame `i` of a clip (µs): 30 fps from an offset, so a time
/// that only comes from the input can't be mistaken for one made up.
fn clip_pts(i: u32) -> u64 {
    1_000 + i as u64 * 33_333
}

/// `n` frames at `w` x `h` through a VideoToolbox H.264 session with frame
/// reordering allowed (High profile, then Main), hardware or software as
/// `hardware` says. Frame `i` has frame code `i` and pts [`clip_pts`].
/// The samples in the order the encoder gave them: decode order.
fn encode_clip(n: u32, w: u32, h: u32, hardware: bool) -> Option<Vec<Encoded>> {
    let spec = dict();
    set_named(
        &spec,
        c"kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder",
        if hardware { yes() } else { no() },
    );
    let attrs = pixel_buffer_attributes(PIX_420V, Some((w, h)));
    let out: Box<Mutex<Vec<Encoded>>> = Box::default();
    let mut session: VTSessionRef = ptr::null_mut();
    // SAFETY: valid dictionaries, a refcon that outlives the session (it is
    // invalidated below, before `out` drops), an out pointer.
    let st = unsafe {
        VTCompressionSessionCreate(
            ptr::null(),
            w as i32,
            h as i32,
            K_CMVIDEO_CODEC_H264,
            spec.0,
            attrs.0,
            ptr::null(),
            Some(on_clip),
            &*out as *const Mutex<Vec<Encoded>> as *mut c_void,
            &mut session,
        )
    };
    if st != 0 || session.is_null() {
        eprintln!("[file] no encoder session (hardware {hardware}): OSStatus {st}");
        return None;
    }
    let prop = |key: &std::ffi::CStr, value: CFTypeRef| {
        // SAFETY: a live session, a framework key, a CF value.
        cf_const(key).is_some_and(|k| unsafe { VTSessionSetProperty(session, k, value) } == 0)
    };
    prop(c"kVTCompressionPropertyKey_RealTime", no());
    let reorder = prop(c"kVTCompressionPropertyKey_AllowFrameReordering", yes());
    let profile = [
        c"kVTProfileLevel_H264_High_AutoLevel",
        c"kVTProfileLevel_H264_Main_AutoLevel",
    ]
    .into_iter()
    .find(|p| cf_const(p).is_some_and(|v| prop(c"kVTCompressionPropertyKey_ProfileLevel", v)));
    prop(
        c"kVTCompressionPropertyKey_MaxKeyFrameInterval",
        num_i32(30).0,
    );
    prop(
        c"kVTCompressionPropertyKey_AverageBitRate",
        num_i32(2_000_000).0,
    );
    prop(
        c"kVTCompressionPropertyKey_ExpectedFrameRate",
        num_i32(30).0,
    );
    eprintln!("[file] encoder (hardware {hardware}): reordering {reorder}, {profile:?}");
    let mut ok = true;
    for i in 0..n {
        let f = pattern_frame(w, h, i, clip_pts(i)).expect("frame");
        let FrameBuffer::Planes(p) = &f.buffer else {
            panic!("pattern frames are CPU planes");
        };
        let mut pb: CVPixelBufferRef = ptr::null_mut();
        // SAFETY: an out pointer; we own the buffer after.
        let st = unsafe {
            CVPixelBufferCreate(
                ptr::null(),
                w as usize,
                h as usize,
                PIX_420V,
                pixel_buffer_attributes(PIX_420V, None).0,
                &mut pb,
            )
        };
        assert!(st == 0 && !pb.is_null(), "CVPixelBufferCreate: {st}");
        let owned = Cf(pb as CFTypeRef);
        fill_nv12(pb, p, w, h).expect("fill");
        let mut info = 0u32;
        // SAFETY: a live session and pixel buffer.
        let st = unsafe {
            VTCompressionSessionEncodeFrame(
                session,
                pb,
                cm_time(clip_pts(i) as i64, 1_000_000),
                cm_time(33_333, 1_000_000),
                ptr::null(),
                ptr::null_mut(),
                &mut info,
            )
        };
        drop(owned);
        if st != 0 {
            eprintln!("[file] encode {i}: OSStatus {st}");
            ok = false;
            break;
        }
    }
    // SAFETY: a live session; flush, then invalidate before `out` drops.
    unsafe {
        VTCompressionSessionCompleteFrames(session, CM_TIME_INVALID);
        VTCompressionSessionInvalidate(session);
        CFRelease(session as CFTypeRef);
    }
    let got = std::mem::take(&mut *lock(&out));
    (ok && got.len() == n as usize).then_some(got)
}

/// Does decode order differ from presentation order (B-frames)?
fn reordered(clip: &[Encoded]) -> bool {
    clip.windows(2).any(|w| w[1].2 < w[0].2)
}

/// A clip with B-frames: the hardware encoder's if it reorders, else the
/// software one's.
fn b_frame_clip(n: u32, w: u32, h: u32) -> Vec<Encoded> {
    for hardware in [true, false] {
        if let Some(clip) = encode_clip(n, w, h, hardware) {
            if reordered(&clip) {
                let which = if hardware { "hardware" } else { "software" };
                eprintln!("[file] B-frames from the {which} encoder");
                return clip;
            }
            eprintln!("[file] the hardware {hardware} encoder made no B-frames");
        }
    }
    panic!("no VideoToolbox H.264 encoder here made B-frames (frame reordering allowed)");
}

fn inputs(clip: &[Encoded]) -> Vec<EncodedInput> {
    clip.iter()
        .map(|(data, key, pts, dts)| {
            EncodedInput::file(
                VideoCodec::H264,
                data.clone(),
                *key,
                pts.expect("every sample has a pts"),
                *dts,
            )
        })
        .collect()
}

/// Submit every input in order, polling as `submit` pushes back, then
/// flush: the frames out, in the order they came.
fn decode_all(
    dec: &mut Box<dyn VideoDecoder>,
    inputs: Vec<EncodedInput>,
) -> (Vec<VideoFrame>, usize) {
    let mut out = Vec::new();
    let mut busy = 0;
    let mut ended = false;
    let take = |dec: &mut Box<dyn VideoDecoder>, out: &mut Vec<VideoFrame>| {
        while let Some(r) = dec.poll() {
            match r {
                Ok(f) => out.push(f),
                Err(DecodeError::EndOfStream) => return true,
                Err(e) => panic!("decode: {e:?}"),
            }
        }
        false
    };
    for input in inputs {
        loop {
            match dec.submit(input.clone()) {
                Ok(()) => break,
                Err(DecodeError::Busy) => {
                    busy += 1;
                    std::thread::sleep(Duration::from_millis(1));
                    assert!(!take(dec, &mut out));
                }
                Err(e) => panic!("submit: {e:?}"),
            }
        }
    }
    let before_flush = {
        std::thread::sleep(Duration::from_millis(200));
        assert!(!take(dec, &mut out));
        out.len()
    };
    dec.flush().expect("flush");
    wait_for(Duration::from_secs(10), || {
        ended |= take(dec, &mut out);
        ended.then_some(())
    })
    .expect("EndOfStream within 10 s of the flush");
    eprintln!(
        "[file] {before_flush} frames before the flush, {} after; Busy {busy} times",
        out.len()
    );
    (out, before_flush)
}

#[test]
fn file_mode_decodes_b_frames_into_presentation_order_with_their_pts() {
    const N: u32 = 60;
    let clip = b_frame_clip(N, 640, 360);
    let decode_pts: Vec<u64> = clip.iter().map(|c| c.2.expect("pts")).collect();
    eprintln!(
        "[file] decode-order pts (first 12): {:?}",
        &decode_pts[..12]
    );
    let sps = crate::h264::annexb_nals(&clip[0].0)
        .into_iter()
        .find(|n| crate::h264::nal_type(n) == Some(crate::h264::NAL_SPS))
        .map(|n| crate::h264::parse_sps(n).expect("the encoder's SPS parses"))
        .expect("an SPS in the first sample");
    eprintln!("[file] SPS: {sps:?}, reorder depth {}", sps.reorder_depth());
    assert_ne!(sps.profile_idc, 66, "a profile with B-frames");
    assert_eq!((sps.width, sps.height), (640, 360));

    let b = backend().expect("backend");
    assert!(b
        .capabilities()
        .best(VideoCodec::H264, CodecDirection::Decode)
        .is_some_and(|c| c.file_mode));
    let mut cfg = DecoderConfig::file(VideoCodec::H264);
    cfg.output_queue = 4;
    let mut dec = b.decoder(cfg, no_wake()).expect("decoder");
    let (out, before_flush) = decode_all(&mut dec, inputs(&clip));
    assert!(
        before_flush < N as usize,
        "the flush gave the held-back frames"
    );
    assert_eq!(out.len(), N as usize, "every frame came out");
    for w in out.windows(2) {
        assert!(w[0].capture_us < w[1].capture_us, "strictly increasing pts");
    }
    for (i, f) in out.iter().enumerate() {
        assert_eq!(f.capture_us, clip_pts(i as u32), "frame {i}'s pts");
        assert_eq!((f.width, f.height), (640, 360));
        assert_eq!(read_frame_code(f), Some(i as u32), "frame {i}'s content");
    }
    let st = dec.stats();
    eprintln!("[file] stats: {st:?}");
    assert_eq!((st.frames_in, st.frames_out), (N as u64, N as u64));
    assert_eq!((st.dropped_out, st.errors), (0, 0));

    // Real time on the same clip: decode order out, as before (no
    // reordering there), each frame with its own input's time.
    let mut rt = b
        .decoder(DecoderConfig::new(VideoCodec::H264), no_wake())
        .expect("decoder");
    let mut rt_pts = Vec::new();
    for input in inputs(&clip).into_iter().take(12) {
        rt.submit(input).expect("submit");
        let f = wait_for(Duration::from_secs(5), || rt.poll())
            .expect("a frame")
            .expect("decode");
        rt_pts.push(f.capture_us);
    }
    assert_eq!(rt_pts, decode_pts[..12], "real time stays in submit order");
}

#[test]
fn file_mode_refuses_a_dts_going_back_and_restarts_from_a_keyframe() {
    let clip = b_frame_clip(20, 320, 180);
    let all = inputs(&clip);
    let b = backend().expect("backend");
    let mut d = b
        .decoder(DecoderConfig::file(VideoCodec::H264), no_wake())
        .expect("decoder");
    let mut first = all[0].clone();
    first.dts_us = Some(1_000_000);
    d.submit(first).expect("key");
    let mut back = all[1].clone();
    back.dts_us = Some(1_000_000);
    assert!(matches!(
        d.submit(back),
        Err(DecodeError::Video(VideoError::Invalid(_)))
    ));
    assert_eq!(d.submit(all[1].clone()), Err(DecodeError::NeedKeyframe));
    d.flush().expect("flush");
    let mut ended = false;
    wait_for(Duration::from_secs(5), || {
        while let Some(r) = d.poll() {
            ended |= matches!(r, Err(DecodeError::EndOfStream));
        }
        ended.then_some(())
    })
    .expect("EndOfStream");
}
