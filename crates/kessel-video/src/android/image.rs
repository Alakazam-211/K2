//! Camera images on Android: the turn that makes them upright (the camera's
//! sensor orientation and the device's turn) and the orientation-free size
//! box. The stride-aware copy and turn itself is [`crate::yuv`].

use crate::types::Facing;

pub(crate) use crate::yuv::{to_i420, Plane, Yuv420};

/// The clockwise turn that makes a camera's image upright: the sensor's
/// orientation, plus the device's turn for a back camera or minus it for a
/// front one (`CaptureRequest.JPEG_ORIENTATION`'s formula). An external
/// camera doesn't turn with the device. `device` is clockwise degrees from
/// the natural orientation.
pub(crate) fn upright_rotation(sensor: u32, facing: Facing, device: u32) -> u32 {
    let snap = |d: u32| ((d % 360 + 45) / 90 % 4) * 90;
    let (sensor, device) = (snap(sensor), snap(device));
    match facing {
        Facing::Front => (sensor + 360 - device) % 360,
        Facing::Back | Facing::Unknown => (sensor + device) % 360,
        _ => sensor,
    }
}

/// The box for `rotated` frames: a size box is orientation-free on a
/// phone, so a landscape 1280x720 box also takes a portrait 720x1280 frame.
pub(crate) fn oriented_box(
    max_width: Option<u32>,
    max_height: Option<u32>,
    portrait: bool,
) -> (Option<u32>, Option<u32>) {
    match (max_width, max_height) {
        (Some(w), Some(h)) if portrait == (w > h) => (Some(h), Some(w)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yuv::rotated_size;

    /// A 4x2 image: Y 1..=8, chroma 2x1. `layout` picks how the chroma
    /// planes sit in memory, as cameras hand them out.
    fn planes_of(layout: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize) {
        // Y with 6-byte rows (2 of padding), the last row cut short.
        let y = vec![1, 2, 3, 4, 0xEE, 0xEE, 5, 6, 7, 8];
        match layout {
            // Planar: U = [10, 11], V = [20, 21], 4-byte rows.
            "planar" => (y, vec![10, 11, 0xEE, 0xEE], vec![20, 21], 4, 1),
            // Interleaved like NV12: U and V share one buffer, U first;
            // each plane's view starts at its own byte and the last pixel
            // stops short.
            _ => (y, vec![10, 20, 11], vec![20, 11, 21], 4, 2),
        }
    }

    fn image<'a>(y: &'a [u8], u: &'a [u8], v: &'a [u8], crs: usize, cps: usize) -> Yuv420<'a> {
        Yuv420 {
            width: 4,
            height: 2,
            planes: [
                Plane {
                    data: y,
                    row_stride: 6,
                    pixel_stride: 1,
                },
                Plane {
                    data: u,
                    row_stride: crs,
                    pixel_stride: cps,
                },
                Plane {
                    data: v,
                    row_stride: crs,
                    pixel_stride: cps,
                },
            ],
        }
    }

    #[test]
    fn planar_and_interleaved_chroma_read_the_same() {
        for layout in ["planar", "interleaved"] {
            let (y, u, v, crs, cps) = planes_of(layout);
            let (out, w, h) = to_i420(&image(&y, &u, &v, crs, cps), 0).expect(layout);
            assert_eq!((w, h), (4, 2));
            assert_eq!(
                out,
                vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 20, 21],
                "{layout}"
            );
        }
    }

    #[test]
    fn rotations_turn_clockwise() {
        let (y, u, v, crs, cps) = planes_of("planar");
        let img = image(&y, &u, &v, crs, cps);
        // 4x2:      90 cw (2x4):   180 (4x2):   270 cw (2x4):
        // 1 2 3 4   5 1            8 7 6 5      4 8
        // 5 6 7 8   6 2            4 3 2 1      3 7
        //           7 3                         2 6
        //           8 4                         1 5
        let (out, w, h) = to_i420(&img, 90).expect("90");
        assert_eq!((w, h), (2, 4));
        assert_eq!(&out[..8], &[5, 1, 6, 2, 7, 3, 8, 4]);
        // Chroma 2x1 turns to 1x2.
        assert_eq!(&out[8..], &[10, 11, 20, 21]);
        let (out, w, h) = to_i420(&img, 180).expect("180");
        assert_eq!((w, h), (4, 2));
        assert_eq!(&out[..8], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&out[8..], &[11, 10, 21, 20]);
        let (out, w, h) = to_i420(&img, 270).expect("270");
        assert_eq!((w, h), (2, 4));
        assert_eq!(&out[..8], &[4, 8, 3, 7, 2, 6, 1, 5]);
        assert_eq!(&out[8..], &[11, 10, 21, 20]);
    }

    #[test]
    fn odd_sizes_round_chroma_up_and_turn() {
        // 3x3, tight planar.
        let y: Vec<u8> = (1..=9).collect();
        let u = [10, 11, 12, 13];
        let v = [20, 21, 22, 23];
        let img = Yuv420 {
            width: 3,
            height: 3,
            planes: [
                Plane {
                    data: &y,
                    row_stride: 3,
                    pixel_stride: 1,
                },
                Plane {
                    data: &u,
                    row_stride: 2,
                    pixel_stride: 1,
                },
                Plane {
                    data: &v,
                    row_stride: 2,
                    pixel_stride: 1,
                },
            ],
        };
        let (out, w, h) = to_i420(&img, 90).expect("3x3");
        assert_eq!((w, h), (3, 3));
        assert_eq!(&out[..9], &[7, 4, 1, 8, 5, 2, 9, 6, 3]);
        assert_eq!(&out[9..13], &[12, 10, 13, 11]);
        let frame = crate::frame::VideoFrame::from_planes(
            crate::frame::Planes::tight(crate::frame::PixelFormat::I420, w, h, out)
                .expect("tight I420"),
            w,
            h,
            0,
        );
        assert!(frame.is_ok(), "the output is a valid I420 frame");
    }

    #[test]
    fn short_or_bad_planes_fail_loudly() {
        let (y, u, v, crs, cps) = planes_of("planar");
        let short = &y[..9];
        assert!(to_i420(&image(short, &u, &v, crs, cps), 0).is_err());
        let mut img = image(&y, &u, &v, crs, cps);
        img.planes[1].pixel_stride = 0;
        assert!(to_i420(&img, 0).is_err());
        let mut img = image(&y, &u, &v, crs, cps);
        img.planes[0].row_stride = 3;
        assert!(to_i420(&img, 0).is_err(), "a row stride under the width");
        assert!(to_i420(&image(&y, &u, &v, crs, cps), 45).is_err());
        let mut img = image(&y, &u, &v, crs, cps);
        img.width = 0;
        assert!(to_i420(&img, 0).is_err());
    }

    #[test]
    fn the_upright_turn_follows_the_sensor_and_the_device() {
        // A typical phone: back sensor at 90, front at 270.
        assert_eq!(upright_rotation(90, Facing::Back, 0), 90);
        assert_eq!(upright_rotation(270, Facing::Front, 0), 270);
        // Turned clockwise to landscape (90): the back camera needs 180,
        // the front 180 too.
        assert_eq!(upright_rotation(90, Facing::Back, 90), 180);
        assert_eq!(upright_rotation(270, Facing::Front, 90), 180);
        // Turned the other way (270).
        assert_eq!(upright_rotation(90, Facing::Back, 270), 0);
        assert_eq!(upright_rotation(270, Facing::Front, 270), 0);
        // A USB camera doesn't turn with the phone.
        assert_eq!(upright_rotation(0, Facing::External, 90), 0);
        // Odd values snap to the nearest quarter turn.
        assert_eq!(upright_rotation(91, Facing::Back, 359), 90);
    }

    #[test]
    fn boxes_are_orientation_free() {
        assert_eq!(rotated_size(1280, 720, 90), (720, 1280));
        assert_eq!(rotated_size(1280, 720, 180), (1280, 720));
        assert_eq!(
            oriented_box(Some(1280), Some(720), true),
            (Some(720), Some(1280))
        );
        assert_eq!(
            oriented_box(Some(1280), Some(720), false),
            (Some(1280), Some(720))
        );
        assert_eq!(
            oriented_box(Some(720), Some(1280), false),
            (Some(1280), Some(720))
        );
        assert_eq!(oriented_box(Some(640), None, true), (Some(640), None));
    }
}
