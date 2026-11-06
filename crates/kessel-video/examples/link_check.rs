//! Links kessel-video the way an app does and calls the system backend's
//! entry points: capabilities, cameras, the camera permission, then an
//! encoder and a decoder for each codec the backend offers, with one frame
//! encoded and decoded through each.
//!
//! A library build never links, so a framework or system library that the
//! crate forgets to ask for only shows up when an app links. The scripts
//! link this example the ways apps do:
//!
//! - `scripts/check-apple-link.sh` builds it as a static library and links
//!   that with the C compiler and no `-framework` flags, as Xcode links a
//!   Tauri iOS app's `libapp.a`. Then it runs the program (macOS) or checks
//!   the archive's undefined symbols against its linker options (iOS).
//! - `scripts/check-android-link.sh` links it as a shared library with the
//!   NDK, as a Tauri Android app's `libapp.so`.
//!
//! As a plain program (`cargo run --example link_check`) it prints the same
//! report and exits 1 on any error.

use std::time::{Duration, Instant};

/// For the C program: 0 when every step answered, 1 otherwise.
#[no_mangle]
pub extern "C" fn kessel_video_link_check() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("link_check: {e}");
            1
        }
    }
}

pub fn main() {
    std::process::exit(kessel_video_link_check());
}

#[cfg(any(
    feature = "apple",
    feature = "windows",
    feature = "linux",
    feature = "android"
))]
fn run() -> Result<(), String> {
    use kessel_video::{
        no_wake, system, CodecConfig, CodecDirection, DecoderConfig, EncodeOptions, EncodedInput,
        EncoderConfig, PixelFormat, SystemConfig, VideoCodec, VideoFrame,
    };

    let backend = system(SystemConfig::default()).map_err(|e| format!("system(): {e}"))?;
    let caps = backend.capabilities();
    println!("backend {} on {}", caps.backend, caps.os);
    println!("camera API {}, screen {:?}", caps.camera, caps.screen);
    println!("cameras: {}", backend.cameras().len());
    println!("camera permission: {:?}", backend.camera_permission());

    let mut tried = 0;
    for codec in [VideoCodec::H264, VideoCodec::Vp8] {
        let Some(enc_cap) = caps.best(codec, CodecDirection::Encode) else {
            println!("{codec:?}: no encoder here");
            continue;
        };
        tried += 1;
        let config = match codec {
            VideoCodec::H264 => EncoderConfig::h264(320, 240, 30, 300_000),
            _ => EncoderConfig::new(CodecConfig::Vp8, 320, 240, 30, 300_000),
        };
        let mut encoder = backend
            .encoder(config, no_wake())
            .map_err(|e| format!("{codec:?} encoder ({}): {e}", enc_cap.name))?;
        let frame = VideoFrame::filled(PixelFormat::I420, 320, 240, [81, 90, 240], 0)
            .map_err(|e| format!("a test frame: {e}"))?;
        // Some encoders hold the first frames back: feed frames until one
        // comes out.
        let deadline = Instant::now() + Duration::from_secs(10);
        let encoded = loop {
            encoder
                .submit(&frame, EncodeOptions::default())
                .map_err(|e| format!("{codec:?} submit: {e}"))?;
            if let Some(r) = poll_until(Instant::now() + Duration::from_millis(100), || {
                encoder.poll()
            }) {
                break r.map_err(|e| format!("{codec:?} encode: {e}"))?;
            }
            if Instant::now() >= deadline {
                return Err(format!("{codec:?}: no encoded frame within 10 s"));
            }
        };
        println!(
            "{codec:?}: encoded {} bytes with {}{}",
            encoded.data.len(),
            enc_cap.name,
            if encoded.keyframe { " (keyframe)" } else { "" }
        );

        if caps.best(codec, CodecDirection::Decode).is_none() {
            println!("{codec:?}: no decoder here");
            continue;
        }
        let mut decoder = backend
            .decoder(DecoderConfig::new(codec), no_wake())
            .map_err(|e| format!("{codec:?} decoder: {e}"))?;
        decoder
            .submit(EncodedInput::new(
                codec,
                encoded.data.clone(),
                encoded.keyframe,
                0,
            ))
            .map_err(|e| format!("{codec:?} decoder submit: {e}"))?;
        let decoded = poll_until(Instant::now() + Duration::from_secs(10), || decoder.poll())
            .ok_or_else(|| format!("{codec:?}: no decoded frame within 10 s"))?
            .map_err(|e| format!("{codec:?} decode: {e}"))?;
        println!("{codec:?}: decoded {}x{}", decoded.width, decoded.height);
    }
    backend.shutdown();
    if tried == 0 {
        return Err("the backend offers no encoder at all".into());
    }
    println!("link_check: ok");
    Ok(())
}

#[cfg(not(any(
    feature = "apple",
    feature = "windows",
    feature = "linux",
    feature = "android"
)))]
fn run() -> Result<(), String> {
    Err("built without a backend feature: there is nothing to link".into())
}

#[cfg_attr(
    not(any(
        feature = "apple",
        feature = "windows",
        feature = "linux",
        feature = "android"
    )),
    allow(dead_code)
)]
fn poll_until<T>(deadline: Instant, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    loop {
        if let Some(t) = f() {
            return Some(t);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
