//! The MediaCodec H.264 encoder's plain parts: filling an input buffer at
//! the codec's stride and slice height, and turning its output into the
//! API's access units (4-byte start codes, SPS and PPS before every IDR,
//! Constrained Baseline flags). No NDK here, so the tests run on any host.

use crate::error::VideoError;
use crate::frame::{PixelFormat, Planes};
use crate::h264::{annexb_nals, nal_type, push_annexb, NAL_IDR, NAL_PPS, NAL_SPS};

/// `COLOR_FormatYUV420Planar` (I420).
pub(crate) const COLOR_I420: i32 = 19;
/// `COLOR_FormatYUV420SemiPlanar` (NV12).
pub(crate) const COLOR_NV12: i32 = 21;

/// The pixel format the encoder takes for a MediaCodec colour format.
pub(crate) fn input_format(color: i32) -> PixelFormat {
    if color == COLOR_I420 {
        PixelFormat::I420
    } else {
        PixelFormat::Nv12
    }
}

/// How an encoder lays out one input picture: luma rows `stride` apart,
/// chroma after `slice_height` luma rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InputLayout {
    pub(crate) color: i32,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
    pub(crate) slice_height: usize,
}

impl InputLayout {
    /// `stride` and `slice_height` as the codec reports them (0 or less:
    /// not reported, so tight), never under the picture.
    pub(crate) fn new(color: i32, width: u32, height: u32, stride: i32, slice: i32) -> InputLayout {
        let (w, h) = (width as usize, height as usize);
        InputLayout {
            color,
            width: w,
            height: h,
            stride: (stride.max(0) as usize).max(w),
            slice_height: (slice.max(0) as usize).max(h),
        }
    }

    /// The bytes one picture takes.
    pub(crate) fn len(&self) -> usize {
        let luma = self.stride * self.slice_height;
        let ch = self.height.div_ceil(2);
        if self.color == COLOR_I420 {
            // U and V each at half the stride, half the slice height; the
            // last plane needs only its rows.
            let cs = self.stride.div_ceil(2);
            luma + cs * self.slice_height.div_ceil(2) + cs * (ch - 1) + self.width.div_ceil(2)
        } else {
            luma + self.stride * (ch - 1) + self.width.div_ceil(2) * 2
        }
    }

    /// Copy tight `planes` (in [`input_format`] of this layout's colour, at
    /// its size) into `buf`. Returns the bytes written.
    pub(crate) fn fill(&self, planes: &Planes, buf: &mut [u8]) -> Result<usize, VideoError> {
        let want = input_format(self.color);
        if planes.format != want {
            return Err(VideoError::Invalid(format!(
                "the encoder takes {want:?}, not {:?}",
                planes.format
            )));
        }
        let need = self.len();
        if buf.len() < need {
            return Err(VideoError::os(
                0,
                format!("a {}-byte encoder input buffer, {need} needed", buf.len()),
            ));
        }
        let (w, h) = (self.width as u32, self.height as u32);
        planes.validate(w, h)?;
        let luma = self.stride * self.slice_height;
        let mut copy = |plane: usize, base: usize, stride: usize| {
            let (_, rows) = want.plane_size(plane, w, h);
            for y in 0..rows {
                let row = planes.row(plane, y, w, h);
                buf[base + y * stride..][..row.len()].copy_from_slice(row);
            }
        };
        copy(0, 0, self.stride);
        if self.color == COLOR_I420 {
            let cs = self.stride.div_ceil(2);
            copy(1, luma, cs);
            copy(2, luma + cs * self.slice_height.div_ceil(2), cs);
        } else {
            copy(1, luma, self.stride);
        }
        Ok(need)
    }
}

/// An SPS with Constrained Baseline's flags set when its profile is
/// Baseline (66): Android encoders write Baseline without FMO, ASO or
/// redundant slices, which is Constrained Baseline, but many leave the
/// flags clear (`4200`), and WebRTC peers look for `42e0`.
fn constrain_sps(nal: &[u8]) -> Vec<u8> {
    let mut sps = nal.to_vec();
    if sps.len() >= 4 && sps[1] == 66 {
        sps[2] |= 0xE0;
    }
    sps
}

/// The parameter sets from a `BUFFER_FLAG_CODEC_CONFIG` buffer, as NAL
/// units (SPS first).
pub(crate) fn parameter_sets(config: &[u8]) -> Vec<Vec<u8>> {
    let mut sets: Vec<Vec<u8>> = annexb_nals(config)
        .into_iter()
        .filter(|n| matches!(nal_type(n), Some(NAL_SPS) | Some(NAL_PPS)))
        .map(|n| {
            if nal_type(n) == Some(NAL_SPS) {
                constrain_sps(n)
            } else {
                n.to_vec()
            }
        })
        .collect();
    sets.sort_by_key(|n| nal_type(n) != Some(NAL_SPS));
    sets
}

/// One encoder output as an access unit: every NAL after a 4-byte start
/// code, the SPS flagged Constrained Baseline, and `sets` (from
/// [`parameter_sets`]) before an IDR that doesn't carry its own. Returns the
/// bytes and whether it is a keyframe (the codec's key flag or an IDR slice).
pub(crate) fn access_unit(data: &[u8], key_flag: bool, sets: &[Vec<u8>]) -> (Vec<u8>, bool) {
    let nals = annexb_nals(data);
    let keyframe = key_flag || nals.iter().any(|n| nal_type(n) == Some(NAL_IDR));
    let has_sps = nals.iter().any(|n| nal_type(n) == Some(NAL_SPS));
    let mut out = Vec::with_capacity(data.len() + 64);
    if keyframe && !has_sps {
        for s in sets {
            push_annexb(&mut out, s);
        }
    }
    for n in nals {
        if nal_type(n) == Some(NAL_SPS) {
            push_annexb(&mut out, &constrain_sps(n));
        } else {
            push_annexb(&mut out, n);
        }
    }
    (out, keyframe)
}

/// The `level` key for an H.264 `level_idc` (`AVCLevel*` in
/// `MediaCodecInfo.CodecProfileLevel`).
pub(crate) fn avc_level(level_idc: u8) -> i32 {
    match level_idc {
        0..=10 => 0x1,
        11 => 0x4,
        12 => 0x8,
        13 => 0x10,
        20 => 0x20,
        21 => 0x40,
        22 => 0x80,
        30 => 0x100,
        31 => 0x200,
        32 => 0x400,
        40 => 0x800,
        41 => 0x1000,
        42 => 0x2000,
        50 => 0x4000,
        51 => 0x8000,
        _ => 0x10000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::VideoFrame;

    #[test]
    fn input_buffers_follow_the_stride_and_slice_height() {
        // 4x2 at stride 8, slice height 4.
        let f = VideoFrame::filled(PixelFormat::Nv12, 4, 2, [50, 60, 70], 0).expect("nv12");
        let p = f.planes().expect("planes");
        let l = InputLayout::new(COLOR_NV12, 4, 2, 8, 4);
        assert_eq!(l.len(), 8 * 4 + 4);
        let mut buf = vec![0xEE; l.len()];
        assert_eq!(l.fill(p, &mut buf).expect("fill"), 36);
        assert_eq!(&buf[..4], &[50; 4]);
        assert_eq!(&buf[4..8], &[0xEE; 4], "row padding untouched");
        assert_eq!(&buf[8..12], &[50; 4]);
        assert_eq!(&buf[32..36], &[60, 70, 60, 70], "UV after 4 luma rows");

        // I420 tight: what the codec says when it reports nothing.
        let f = VideoFrame::filled(PixelFormat::I420, 4, 2, [50, 60, 70], 0).expect("i420");
        let l = InputLayout::new(COLOR_I420, 4, 2, 0, -1);
        assert_eq!((l.stride, l.slice_height), (4, 2));
        let mut buf = vec![0; l.len()];
        assert_eq!(buf.len(), 12);
        l.fill(f.planes().expect("planes"), &mut buf).expect("fill");
        assert_eq!(buf, vec![50, 50, 50, 50, 50, 50, 50, 50, 60, 60, 70, 70]);

        // I420 padded: U at stride/2 after slice-height rows, V after
        // slice-height/2 chroma rows.
        let l = InputLayout::new(COLOR_I420, 4, 2, 8, 4);
        let mut buf = vec![0xEE; l.len()];
        l.fill(f.planes().expect("planes"), &mut buf).expect("fill");
        assert_eq!(&buf[32..34], &[60, 60]);
        assert_eq!(&buf[40..42], &[70, 70]);

        // Wrong format or a short buffer fails loudly.
        let l = InputLayout::new(COLOR_NV12, 4, 2, 0, 0);
        let mut buf = vec![0; l.len()];
        assert!(l.fill(f.planes().expect("planes"), &mut buf).is_err());
        let nv = VideoFrame::filled(PixelFormat::Nv12, 4, 2, [50, 60, 70], 0).expect("nv12");
        assert!(l.fill(nv.planes().expect("planes"), &mut buf[..5]).is_err());
    }

    #[test]
    fn outputs_become_access_units_with_parameter_sets() {
        let sps = [0x67, 66, 0x00, 31, 0xAA];
        let pps = [0x68, 0xCE, 0x3C, 0x80];
        // The config buffer: 3-byte start codes, PPS first.
        let mut config = vec![0, 0, 1];
        config.extend_from_slice(&pps);
        config.extend_from_slice(&[0, 0, 1]);
        config.extend_from_slice(&sps);
        let sets = parameter_sets(&config);
        assert_eq!(sets.len(), 2);
        assert_eq!(
            sets[0],
            vec![0x67, 66, 0xE0, 31, 0xAA],
            "SPS first, constrained"
        );
        assert_eq!(sets[1], pps.to_vec());

        let idr = [0x65, 0x88, 0x84, 0x10, 0x00];
        let mut data = vec![0, 0, 1];
        data.extend_from_slice(&idr);
        let (au, key) = access_unit(&data, false, &sets);
        assert!(key, "an IDR slice is a keyframe");
        let nals = annexb_nals(&au);
        assert_eq!(nals.len(), 3);
        assert_eq!(nal_type(nals[0]), Some(NAL_SPS));
        assert_eq!(nal_type(nals[1]), Some(NAL_PPS));
        assert_eq!(nals[2], &idr[..4], "trailing zeros are padding");
        assert!(au.starts_with(&[0, 0, 0, 1, 0x67]));
        assert_eq!(
            crate::h264::codec_string(&nals).as_deref(),
            Some("avc1.42E01F")
        );

        let delta = [0, 0, 0, 1, 0x41, 0x9A, 0x02];
        let (au, key) = access_unit(&delta, false, &sets);
        assert!(!key);
        assert_eq!(au, delta.to_vec(), "a delta frame passes as it is");

        // An IDR that carries its own SPS gets no second copy.
        let mut own = vec![0, 0, 0, 1];
        own.extend_from_slice(&sps);
        own.extend_from_slice(&[0, 0, 0, 1]);
        own.extend_from_slice(&idr);
        let (au, key) = access_unit(&own, true, &sets);
        assert!(key);
        let nals = annexb_nals(&au);
        assert_eq!(nals.len(), 2);
        assert_eq!(nals[0][2], 0xE0, "its SPS is constrained too");

        // High profile SPS keeps its flags.
        let high = parameter_sets(&[0, 0, 0, 1, 0x67, 100, 0x0C, 40]);
        assert_eq!(high[0][2], 0x0C);
    }

    #[test]
    fn levels_map_to_mediacodec_constants() {
        assert_eq!(avc_level(31), 0x200);
        assert_eq!(avc_level(40), 0x800);
        assert_eq!(avc_level(52), 0x10000);
        assert_eq!(avc_level(10), 0x1);
    }
}
