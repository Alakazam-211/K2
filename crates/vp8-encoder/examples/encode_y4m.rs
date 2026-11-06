//! Encodes a Y4M (4:2:0) file to IVF and reports speed, bitrate and PSNR.
//! Also writes what vp8-decoder decodes from the output as raw I420, so
//! other decoders' output can be compared byte for byte.
//!
//!   cargo run --release -p vp8-encoder --example encode_y4m -- \
//!       in.y4m out.ivf BITRATE [WIDTH HEIGHT] [--layers 1|2|3] [--frames N] [--loop]
//!
//! `--loop` plays a short clip forward, then backward, and so on until
//! `--frames` frames, for steady-state rate measurements.

use std::io::Write;
use std::time::Instant;
use vp8_decoder::Decoder;
use vp8_encoder::{scale_i420, Encoder, EncoderConfig, Picture, TemporalLayers};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: encode_y4m IN.y4m OUT.ivf BITRATE [WIDTH HEIGHT] [--layers N] [--frames N] [--loop]";
    let input = args.first().expect(usage);
    let output = args.get(1).expect(usage);
    let bitrate: u32 = args.get(2).expect(usage).parse().expect("bitrate in bit/s");
    let mut size: Option<(u32, u32)> = None;
    let mut layers = TemporalLayers::L1T1;
    let mut max_frames = usize::MAX;
    let mut looped = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--layers" => {
                layers = match args[i + 1].as_str() {
                    "2" => TemporalLayers::L1T2,
                    "3" => TemporalLayers::L1T3,
                    _ => TemporalLayers::L1T1,
                };
                i += 2;
            }
            "--loop" => {
                looped = true;
                i += 1;
            }
            "--frames" => {
                max_frames = args[i + 1].parse().expect("frame count");
                i += 2;
            }
            w => {
                size = Some((w.parse().expect("width"), args[i + 1].parse().expect("height")));
                i += 2;
            }
        }
    }

    let data = std::fs::read(input).unwrap_or_else(|e| panic!("{input}: {e}"));
    let header_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let header = std::str::from_utf8(&data[..header_end]).expect("y4m header text");
    assert!(header.starts_with("YUV4MPEG2"), "not a Y4M file");
    let mut sw = 0usize;
    let mut sh = 0usize;
    let mut fps = 30u32;
    for tok in header.split(' ') {
        match tok.as_bytes().first() {
            Some(b'W') => sw = tok[1..].parse().expect("width"),
            Some(b'H') => sh = tok[1..].parse().expect("height"),
            Some(b'F') => {
                let (n, d) = tok[1..].split_once(':').expect("frame rate");
                let (n, d): (u32, u32) = (n.parse().expect("rate"), d.parse().expect("rate"));
                fps = (n + d / 2) / d.max(1);
            }
            Some(b'C') => assert!(tok.starts_with("C420"), "only 4:2:0 input: {tok}"),
            _ => {}
        }
    }
    let (w, h) = size.unwrap_or((sw as u32, sh as u32));
    let frame_len = sw * sh + 2 * sw.div_ceil(2) * sh.div_ceil(2);
    let cfg = EncoderConfig::new(w, h, fps, bitrate).with_temporal_layers(layers);
    let mut enc = Encoder::new(cfg).expect("encoder");
    let mut dec = Decoder::new();

    let mut ivf = Vec::new();
    ivf.extend_from_slice(b"DKIF");
    ivf.extend_from_slice(&0u16.to_le_bytes());
    ivf.extend_from_slice(&32u16.to_le_bytes());
    ivf.extend_from_slice(b"VP80");
    ivf.extend_from_slice(&(w as u16).to_le_bytes());
    ivf.extend_from_slice(&(h as u16).to_le_bytes());
    ivf.extend_from_slice(&fps.to_le_bytes());
    ivf.extend_from_slice(&1u32.to_le_bytes());
    let count_pos = ivf.len();
    ivf.extend_from_slice(&0u32.to_le_bytes());
    ivf.extend_from_slice(&0u32.to_le_bytes());
    let mut decoded_yuv = std::io::BufWriter::new(std::fs::File::create(format!("{output}.dec.yuv")).expect("create .dec.yuv"));

    // Where each frame's pixels start.
    let mut starts = Vec::new();
    let mut pos = header_end + 1;
    while pos < data.len() {
        let line_end = pos + data[pos..].iter().position(|&b| b == b'\n').expect("FRAME line");
        pos = line_end + 1;
        assert!(pos + frame_len <= data.len(), "truncated frame");
        starts.push(pos);
        pos += frame_len;
    }
    assert!(!starts.is_empty(), "no frames");
    if looped {
        assert!(max_frames != usize::MAX, "--loop needs --frames");
    } else {
        max_frames = max_frames.min(starts.len());
    }
    // After the first second: the rate without the first keyframe and the
    // rate control's warm-up.
    let mut steady_bytes = 0usize;
    let mut n = 0usize;
    let mut out_frames = 0u32;
    let mut bytes = 0usize;
    let mut keyframes = 0;
    let mut psnr_sum = 0.0;
    let mut encode_secs = 0.0;
    let mut scaled = Vec::new();
    let mut qs = Vec::new();
    while n < max_frames {
        // Forward, then backward, without repeating the end frames.
        let period = (2 * starts.len()).saturating_sub(2).max(1);
        let k = n % period;
        let idx = if k < starts.len() { k } else { period - k };
        let src = &data[starts[idx]..starts[idx] + frame_len];
        let full = Picture::from_i420(src, sw as u32, sh as u32).expect("source picture");
        let pic = if (w, h) == (sw as u32, sh as u32) {
            full
        } else {
            scaled.clear();
            scale_i420(&full, w, h, &mut scaled);
            Picture::from_i420(&scaled, w, h).expect("scaled picture")
        };
        let reference = pic.to_i420();
        let t = Instant::now();
        let out = enc.encode(&pic, false).expect("encode");
        encode_secs += t.elapsed().as_secs_f64();
        n += 1;
        let Some(out) = out else { continue };
        keyframes += out.keyframe as usize;
        qs.push(out.q_index);
        bytes += out.data.len();
        if n > fps as usize {
            steady_bytes += out.data.len();
        }
        ivf.extend_from_slice(&(out.data.len() as u32).to_le_bytes());
        ivf.extend_from_slice(&(n as u64 - 1).to_le_bytes());
        ivf.extend_from_slice(&out.data);
        out_frames += 1;
        let p = dec.decode(&out.data).expect("own output decodes").expect("shown");
        let got = p.to_i420();
        decoded_yuv.write_all(&got).expect("write .dec.yuv");
        let mse: f64 = got.iter().zip(&reference).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum::<f64>() / got.len() as f64;
        psnr_sum += if mse == 0.0 { 99.0 } else { 10.0 * (65025.0 / mse).log10() };
    }
    qs.sort_unstable();
    let q_range = match (qs.first(), qs.last()) {
        (Some(lo), Some(hi)) => format!("q {lo}..{hi} (median {})", qs[qs.len() / 2]),
        _ => "no frames".to_string(),
    };
    let steady = if n > 2 * fps as usize {
        format!("{:.0} kbit/s after the first second", steady_bytes as f64 * 8.0 / ((n - fps as usize) as f64 / fps as f64) / 1000.0)
    } else {
        "too short for a steady rate".to_string()
    };
    ivf[count_pos..count_pos + 4].copy_from_slice(&out_frames.to_le_bytes());
    std::fs::write(output, &ivf).unwrap_or_else(|e| panic!("{output}: {e}"));
    println!(
        "[encode] {w}x{h} {:?} target {} kbit/s: {n} frames in, {out_frames} out, {keyframes} keyframes, {:.0} kbit/s ({steady}), {q_range}, PSNR {:.2} dB, {:.1} fps encode ({:.2} ms/frame)",
        layers,
        bitrate / 1000,
        bytes as f64 * 8.0 / (n as f64 / fps as f64) / 1000.0,
        psnr_sum / out_frames.max(1) as f64,
        n as f64 / encode_secs,
        encode_secs * 1000.0 / n as f64
    );
}
