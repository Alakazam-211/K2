//! What the NDK reports, read without the NDK: camera characteristics into
//! [`CamDesc`], the frame-rate range to ask for, and status codes into
//! [`VideoError`]. Plain data in, so the tests run on any host.

use crate::error::VideoError;
use crate::frame::PixelFormat;
use crate::types::{CameraInfo, CaptureMode, Facing};

/// `AIMAGE_FORMAT_YUV_420_888`.
pub(crate) const FORMAT_YUV_420_888: i32 = 0x23;

/// `ACAMERA_LENS_FACING_*`.
const LENS_FACING_FRONT: u8 = 0;
const LENS_FACING_BACK: u8 = 1;
const LENS_FACING_EXTERNAL: u8 = 2;

/// A camera as the backend needs it: what callers see, plus the sensor's
/// orientation and the auto-exposure frame-rate ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CamDesc {
    pub(crate) info: CameraInfo,
    /// Clockwise degrees (`ACAMERA_SENSOR_ORIENTATION`).
    pub(crate) sensor_orientation: u32,
    /// `ACAMERA_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES`: (min, max) pairs.
    pub(crate) fps_ranges: Vec<(u32, u32)>,
}

/// The raw characteristics of one camera.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawCharacteristics<'a> {
    /// `ACAMERA_LENS_FACING`, if listed.
    pub(crate) facing: Option<u8>,
    /// `ACAMERA_SENSOR_ORIENTATION`, if listed.
    pub(crate) orientation: Option<i32>,
    /// `ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS`: (format, width,
    /// height, is_input) quadruples.
    pub(crate) stream_configs: &'a [i32],
    /// `ACAMERA_SCALER_AVAILABLE_MIN_FRAME_DURATIONS`: (format, width,
    /// height, nanoseconds) quadruples.
    pub(crate) min_durations: &'a [i64],
    /// `ACAMERA_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES`: (min, max) pairs.
    pub(crate) fps_ranges: &'a [i32],
}

/// The camera `id`, described. `None` when it has no YUV output (a depth
/// or raw-only camera).
pub(crate) fn describe(id: &str, raw: &RawCharacteristics<'_>) -> Option<CamDesc> {
    let facing = match raw.facing {
        Some(LENS_FACING_FRONT) => Facing::Front,
        Some(LENS_FACING_BACK) => Facing::Back,
        Some(LENS_FACING_EXTERNAL) => Facing::External,
        _ => Facing::Unknown,
    };
    let fps_ranges: Vec<(u32, u32)> = raw
        .fps_ranges
        .chunks_exact(2)
        .filter(|r| r[0] > 0 && r[1] >= r[0])
        .map(|r| (r[0] as u32, r[1] as u32))
        .collect();
    let ae_max = fps_ranges.iter().map(|r| r.1).max().unwrap_or(30);
    let mut modes: Vec<CaptureMode> = Vec::new();
    for c in raw.stream_configs.chunks_exact(4) {
        let (format, w, h, is_input) = (c[0], c[1], c[2], c[3]);
        if format != FORMAT_YUV_420_888 || is_input != 0 || w < 16 || h < 16 {
            continue;
        }
        let duration = raw
            .min_durations
            .chunks_exact(4)
            .find(|d| d[0] == FORMAT_YUV_420_888 as i64 && d[1] == w as i64 && d[2] == h as i64)
            .map_or(0, |d| d[3]);
        // No listed duration: the AE ranges are the only bound.
        let size_max = if duration > 0 {
            (1_000_000_000f64 / duration as f64).round() as u32
        } else {
            ae_max
        };
        let mode = CaptureMode::new(
            w as u32,
            h as u32,
            size_max.min(ae_max).max(1),
            PixelFormat::I420,
        );
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    if modes.is_empty() {
        return None;
    }
    modes.sort_by_key(|m| std::cmp::Reverse((m.width as u64 * m.height as u64, m.max_fps)));
    let name = match facing {
        Facing::Front => format!("Front camera ({id})"),
        Facing::Back => format!("Back camera ({id})"),
        Facing::External => format!("External camera ({id})"),
        _ => format!("Camera {id}"),
    };
    let mut info = CameraInfo::new(id, name, facing, false);
    info.modes = modes;
    Some(CamDesc {
        info,
        sensor_orientation: raw.orientation.unwrap_or(0).rem_euclid(360) as u32,
        fps_ranges,
    })
}

/// Mark the default: the first front camera (the one a call wants), else
/// the first camera.
pub(crate) fn mark_default(cams: &mut [CamDesc]) {
    let i = cams
        .iter()
        .position(|c| c.info.facing == Facing::Front)
        .unwrap_or(0);
    for (k, c) in cams.iter_mut().enumerate() {
        c.info.is_default = k == i;
    }
}

/// The auto-exposure range for `fps`: one whose top is `fps` if there is
/// one (the highest floor wins: steadier motion), else the lowest top above
/// it, preferring one whose floor is at or under `fps`. `None` when the
/// camera lists no range that reaches `fps`.
pub(crate) fn fps_range(ranges: &[(u32, u32)], fps: u32) -> Option<(u32, u32)> {
    ranges
        .iter()
        .filter(|r| r.1 == fps)
        .max_by_key(|r| r.0)
        .or_else(|| {
            ranges
                .iter()
                .filter(|r| r.1 > fps)
                .min_by_key(|r| (r.0 > fps, r.1, std::cmp::Reverse(r.0)))
        })
        .copied()
}

// ───────────────────────────── status codes ─────────────────────────────

/// `camera_status_t` values (`NdkCameraError.h`).
pub(crate) mod camera_status {
    pub(crate) const OK: i32 = 0;
    pub(crate) const INVALID_PARAMETER: i32 = -10001;
    pub(crate) const CAMERA_DISCONNECTED: i32 = -10002;
    pub(crate) const NOT_ENOUGH_MEMORY: i32 = -10003;
    pub(crate) const CAMERA_IN_USE: i32 = -10010;
    pub(crate) const MAX_CAMERA_IN_USE: i32 = -10011;
    pub(crate) const CAMERA_DISABLED: i32 = -10012;
    pub(crate) const PERMISSION_DENIED: i32 = -10013;
    pub(crate) const UNSUPPORTED_OPERATION: i32 = -10014;
}

/// A `camera_status_t` from `what` as an error (`Ok` for `ACAMERA_OK`).
pub(crate) fn camera_result(status: i32, what: &str) -> Result<(), VideoError> {
    use camera_status::*;
    match status {
        OK => Ok(()),
        PERMISSION_DENIED | CAMERA_DISABLED => Err(VideoError::NoPermission),
        CAMERA_IN_USE | MAX_CAMERA_IN_USE => Err(VideoError::DeviceBusy),
        CAMERA_DISCONNECTED => Err(VideoError::DeviceLost),
        NOT_ENOUGH_MEMORY => Err(VideoError::NoResources),
        UNSUPPORTED_OPERATION => Err(VideoError::Unsupported(format!(
            "{what}: the camera can't do that"
        ))),
        INVALID_PARAMETER => Err(VideoError::os(
            status as i64,
            format!("{what}: an invalid parameter"),
        )),
        other => Err(VideoError::os(other as i64, format!("{what} failed"))),
    }
}

/// `media_status_t` values (`NdkMediaError.h`).
pub(crate) mod media_status {
    pub(crate) const OK: i32 = 0;
    pub(crate) const CODEC_INSUFFICIENT_RESOURCE: i32 = 1100;
    pub(crate) const CODEC_RECLAIMED: i32 = 1101;
    pub(crate) const UNSUPPORTED: i32 = -10002;
    pub(crate) const IMGREADER_NO_BUFFER_AVAILABLE: i32 = -30001;
    pub(crate) const IMGREADER_MAX_IMAGES_ACQUIRED: i32 = -30002;
}

/// A `media_status_t` from `what` as an error (`Ok` for `AMEDIA_OK`).
pub(crate) fn media_result(status: i32, what: &str) -> Result<(), VideoError> {
    use media_status::*;
    match status {
        OK => Ok(()),
        CODEC_INSUFFICIENT_RESOURCE | CODEC_RECLAIMED => Err(VideoError::NoResources),
        UNSUPPORTED => Err(VideoError::Unsupported(format!(
            "{what}: not supported by this device"
        ))),
        IMGREADER_NO_BUFFER_AVAILABLE | IMGREADER_MAX_IMAGES_ACQUIRED => Err(VideoError::Busy),
        other => Err(VideoError::os(other as i64, format!("{what} failed"))),
    }
}

/// How a running camera stopped, from the device's state callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lost {
    /// `onDisconnected`: another client took it, or it was unplugged.
    Disconnected,
    /// `onError` with `ERROR_CAMERA_IN_USE` or `ERROR_MAX_CAMERAS_IN_USE`.
    InUse,
    /// `onError` with `ERROR_CAMERA_DISABLED` (device policy).
    Disabled,
    /// `onError` with `ERROR_CAMERA_DEVICE` or `ERROR_CAMERA_SERVICE`, or
    /// another code.
    Failed(i32),
}

/// An `ACameraDevice` `onError` code as a [`Lost`].
pub(crate) fn device_error(code: i32) -> Lost {
    match code {
        1 | 2 => Lost::InUse,
        3 => Lost::Disabled,
        other => Lost::Failed(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YUV: i32 = FORMAT_YUV_420_888;
    const JPEG: i32 = 0x100;

    #[test]
    fn characteristics_become_modes_facing_and_orientation() {
        let configs = [
            YUV, 1920, 1080, 0, //
            YUV, 1280, 720, 0, //
            YUV, 640, 480, 0, //
            YUV, 640, 480, 1, // an input stream: skipped
            JPEG, 4000, 3000, 0, // not YUV: skipped
            YUV, 1280, 720, 0, // a duplicate: skipped
        ];
        let durations = [
            YUV as i64, 1920, 1080, 50_000_000, // 20 fps
            YUV as i64, 1280, 720, 33_333_333, // 30 fps
            YUV as i64, 640, 480, 16_666_666, // 60 fps, capped by AE at 30
        ];
        let ranges = [15, 30, 30, 30, 7, 30];
        let raw = RawCharacteristics {
            facing: Some(LENS_FACING_BACK),
            orientation: Some(90),
            stream_configs: &configs,
            min_durations: &durations,
            fps_ranges: &ranges,
        };
        let d = describe("0", &raw).expect("a YUV camera");
        assert_eq!(d.info.facing, Facing::Back);
        assert_eq!(d.info.id, "0");
        assert_eq!(d.info.name, "Back camera (0)");
        assert_eq!(d.sensor_orientation, 90);
        assert_eq!(d.fps_ranges, vec![(15, 30), (30, 30), (7, 30)]);
        let modes: Vec<(u32, u32, u32)> = d
            .info
            .modes
            .iter()
            .map(|m| (m.width, m.height, m.max_fps))
            .collect();
        assert_eq!(
            modes,
            vec![(1920, 1080, 20), (1280, 720, 30), (640, 480, 30)]
        );
        assert!(d.info.modes.iter().all(|m| m.format == PixelFormat::I420));

        let front = describe(
            "1",
            &RawCharacteristics {
                facing: Some(LENS_FACING_FRONT),
                orientation: Some(270),
                ..raw
            },
        )
        .expect("front");
        assert_eq!(front.info.facing, Facing::Front);
        let mut cams = vec![d, front];
        mark_default(&mut cams);
        assert!(
            !cams[0].info.is_default && cams[1].info.is_default,
            "front first"
        );

        let depth_only = RawCharacteristics {
            stream_configs: &[JPEG, 640, 480, 0],
            ..raw
        };
        assert!(describe("2", &depth_only).is_none(), "no YUV, no camera");
        let bare = RawCharacteristics {
            stream_configs: &[YUV, 640, 480, 0],
            ..RawCharacteristics::default()
        };
        let d = describe("3", &bare).expect("no durations, no ranges");
        assert_eq!(d.info.facing, Facing::Unknown);
        assert_eq!(
            d.info.modes[0].max_fps, 30,
            "assumed 30 with nothing listed"
        );
    }

    #[test]
    fn the_fps_range_tops_out_at_the_asked_rate() {
        let ranges = [(15, 30), (30, 30), (7, 30), (15, 15), (24, 60)];
        assert_eq!(fps_range(&ranges, 30), Some((30, 30)));
        assert_eq!(fps_range(&ranges, 15), Some((15, 15)));
        assert_eq!(fps_range(&ranges, 24), Some((15, 30)), "not 30-30 over 24");
        assert_eq!(fps_range(&ranges, 50), Some((24, 60)));
        assert_eq!(fps_range(&ranges, 120), None);
        assert_eq!(fps_range(&[], 30), None);
    }

    #[test]
    fn status_codes_map_to_clear_errors() {
        use camera_status as c;
        assert_eq!(camera_result(c::OK, "x"), Ok(()));
        assert_eq!(
            camera_result(c::PERMISSION_DENIED, "open"),
            Err(VideoError::NoPermission)
        );
        assert_eq!(
            camera_result(c::CAMERA_DISABLED, "open"),
            Err(VideoError::NoPermission)
        );
        assert_eq!(
            camera_result(c::CAMERA_IN_USE, "open"),
            Err(VideoError::DeviceBusy)
        );
        assert_eq!(
            camera_result(c::MAX_CAMERA_IN_USE, "open"),
            Err(VideoError::DeviceBusy)
        );
        assert_eq!(
            camera_result(c::CAMERA_DISCONNECTED, "open"),
            Err(VideoError::DeviceLost)
        );
        assert_eq!(
            camera_result(-10005, "ACameraDevice_createCaptureSession"),
            Err(VideoError::os(
                -10005,
                "ACameraDevice_createCaptureSession failed"
            ))
        );
        assert_eq!(
            camera_result(c::UNSUPPORTED_OPERATION, "x").map_err(|e| e.code()),
            Err("unsupported")
        );

        use media_status as m;
        assert_eq!(media_result(m::OK, "x"), Ok(()));
        assert_eq!(
            media_result(m::CODEC_INSUFFICIENT_RESOURCE, "configure"),
            Err(VideoError::NoResources)
        );
        assert_eq!(
            media_result(m::UNSUPPORTED, "configure").map_err(|e| e.code()),
            Err("unsupported")
        );
        assert_eq!(
            media_result(m::IMGREADER_MAX_IMAGES_ACQUIRED, "acquire"),
            Err(VideoError::Busy)
        );
        assert_eq!(
            media_result(-10000, "AMediaCodec_start"),
            Err(VideoError::os(-10000, "AMediaCodec_start failed"))
        );

        assert_eq!(device_error(1), Lost::InUse);
        assert_eq!(device_error(2), Lost::InUse);
        assert_eq!(device_error(3), Lost::Disabled);
        assert_eq!(device_error(4), Lost::Failed(4));
    }
}
