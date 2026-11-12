//! The rate table (fix 2; PRD remote desktop §3.5, E6): a viewer's fps cap
//! by source kind, codec and [`ViewerPath`].
//!
//! **Sim:** JPEG up to 30 fps on this machine, 2–15 on the LAN or over
//! Connect (default 8); H.264 up to 60 on this machine, 2–60 on the LAN
//! (default 60), 2–30 over Connect (default 24). `K2_SIM_VIDEO_MAX_FPS=30`
//! (read once) puts the local and LAN H.264 caps back to 30.
//!
//! **Desktop** (k2-deskd, PRD §3.5): H.264 up to 60 on this machine, 2–60
//! on the LAN (default 60), 2–30 over Connect (default 30); VP8 up to 15,
//! or up to 30 when the layer is 720p or less (2 at least off this machine;
//! default the cap); JPEG up to 15 on this machine, 2–8 elsewhere (default
//! 8). The sim's rollback switch doesn't apply. A viewer's fps is clamped
//! when it joins, before the source's first unit, so the picture height is
//! usually unknown then: VP8 with no height takes the 1080p row (15).
//! [`clamp_fps_sized`] takes the height when the route knows it (deskd's
//! display, after the layer's 1920-wide cap).

use std::sync::OnceLock;

use k2_core::sim::stream::{ViewerPath, LOCAL_MAX_FPS, LOCAL_VIDEO_MAX_FPS, REMOTE_DEFAULT_FPS, REMOTE_MAX_FPS, REMOTE_MIN_FPS, REMOTE_VIDEO_DEFAULT_FPS, REMOTE_VIDEO_MAX_FPS};
use k2_core::stream::{Codec, SourceKind};

/// Desktop H.264: the most on this machine and the LAN; over Connect.
pub const DESKTOP_VIDEO_MAX_FPS: u32 = 60;
pub const DESKTOP_REMOTE_VIDEO_MAX_FPS: u32 = 30;
/// Desktop VP8: the cap above 720p, and at 720p or less.
pub const DESKTOP_VP8_MAX_FPS: u32 = 15;
pub const DESKTOP_VP8_SMALL_MAX_FPS: u32 = 30;
/// The tallest picture that gets [`DESKTOP_VP8_SMALL_MAX_FPS`].
pub const DESKTOP_VP8_SMALL_HEIGHT: u32 = 720;
/// Desktop JPEG: the most on this machine, and elsewhere (also the default
/// elsewhere).
pub const DESKTOP_JPEG_MAX_FPS: u32 = 15;
pub const DESKTOP_REMOTE_JPEG_MAX_FPS: u32 = 8;

/// The highest H.264 rate on this machine and the LAN: 60, or what
/// `K2_SIM_VIDEO_MAX_FPS` says (read once; the fix 2 rollback is 30).
pub fn video_max_fps() -> u32 {
    static MAX: OnceLock<u32> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("K2_SIM_VIDEO_MAX_FPS").ok().and_then(|v| v.trim().parse::<u32>().ok()).map_or(LOCAL_VIDEO_MAX_FPS, |n| n.clamp(REMOTE_MIN_FPS, LOCAL_VIDEO_MAX_FPS))
    })
}

/// A viewer's fps: what it asked (`None` = the default), clamped by the
/// table. `video_max` is the local and LAN H.264 cap ([`video_max_fps`]).
pub fn clamp_fps_with(kind: SourceKind, codec: Codec, path: ViewerPath, asked: Option<u32>, video_max: u32) -> u32 {
    match kind {
        SourceKind::Sim => sim_row(codec, path, asked, video_max),
        SourceKind::Desktop => desktop_row(codec, path, asked, None),
    }
}

/// [`clamp_fps`] with the picture's height in pixels, where the route knows
/// it (only the desktop's VP8 row reads it; `None` = the 1080p row).
pub fn clamp_fps_sized(kind: SourceKind, codec: Codec, path: ViewerPath, asked: Option<u32>, height_px: Option<u32>) -> u32 {
    match kind {
        SourceKind::Sim => sim_row(codec, path, asked, video_max_fps()),
        SourceKind::Desktop => desktop_row(codec, path, asked, height_px),
    }
}

fn desktop_row(codec: Codec, path: ViewerPath, asked: Option<u32>, height_px: Option<u32>) -> u32 {
    let min = if path == ViewerPath::Local { 1 } else { REMOTE_MIN_FPS };
    let (default, max) = match (codec, path) {
        (Codec::H264, ViewerPath::Local | ViewerPath::Lan) => (DESKTOP_VIDEO_MAX_FPS, DESKTOP_VIDEO_MAX_FPS),
        (Codec::H264, ViewerPath::Tunnel) => (DESKTOP_REMOTE_VIDEO_MAX_FPS, DESKTOP_REMOTE_VIDEO_MAX_FPS),
        (Codec::Vp8, _) => {
            let small = height_px.is_some_and(|h| h <= DESKTOP_VP8_SMALL_HEIGHT);
            let cap = if small { DESKTOP_VP8_SMALL_MAX_FPS } else { DESKTOP_VP8_MAX_FPS };
            (cap, cap)
        }
        (Codec::Jpeg, ViewerPath::Local) => (DESKTOP_JPEG_MAX_FPS, DESKTOP_JPEG_MAX_FPS),
        (Codec::Jpeg, _) => (DESKTOP_REMOTE_JPEG_MAX_FPS, DESKTOP_REMOTE_JPEG_MAX_FPS),
    };
    asked.unwrap_or(default).clamp(min, max)
}

fn sim_row(codec: Codec, path: ViewerPath, asked: Option<u32>, video_max: u32) -> u32 {
    match (codec, path) {
        (Codec::Jpeg, ViewerPath::Local) => asked.unwrap_or(LOCAL_MAX_FPS).clamp(1, LOCAL_MAX_FPS),
        (Codec::Jpeg, _) => asked.unwrap_or(REMOTE_DEFAULT_FPS).clamp(REMOTE_MIN_FPS, REMOTE_MAX_FPS),
        // A sim has no VP8; it would read the H.264 row.
        (Codec::H264 | Codec::Vp8, ViewerPath::Local) => asked.unwrap_or(video_max).clamp(1, video_max),
        (Codec::H264 | Codec::Vp8, ViewerPath::Lan) => asked.unwrap_or(video_max).clamp(REMOTE_MIN_FPS, video_max),
        (Codec::H264 | Codec::Vp8, ViewerPath::Tunnel) => asked.unwrap_or(REMOTE_VIDEO_DEFAULT_FPS).clamp(REMOTE_MIN_FPS, REMOTE_VIDEO_MAX_FPS),
    }
}

/// [`clamp_fps_with`] with this daemon's H.264 cap.
pub fn clamp_fps(kind: SourceKind, codec: Codec, path: ViewerPath, asked: Option<u32>) -> u32 {
    clamp_fps_with(kind, codec, path, asked, video_max_fps())
}

/// The highest fps the table gives this viewer (a layer's rate never goes
/// past it).
pub fn cap(kind: SourceKind, codec: Codec, path: ViewerPath) -> u32 {
    clamp_fps(kind, codec, path, Some(u32::MAX))
}

/// [`cap`] with the picture's height, where the source knows it.
pub fn cap_sized(kind: SourceKind, codec: Codec, path: ViewerPath, height_px: Option<u32>) -> u32 {
    clamp_fps_sized(kind, codec, path, Some(u32::MAX), height_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ViewerPath::{Lan, Local, Tunnel};

    fn d(codec: Codec, path: ViewerPath, asked: Option<u32>) -> u32 {
        clamp_fps(SourceKind::Desktop, codec, path, asked)
    }

    /// PRD remote desktop §3.5, the desktop's row.
    #[test]
    fn the_desktop_row_follows_the_prd_table() {
        // H.264: ≤ 60 here (default 60), 2–60 on the LAN (default 60),
        // 2–30 over Connect (default 30).
        assert_eq!(d(Codec::H264, Local, None), 60);
        assert_eq!(d(Codec::H264, Local, Some(99)), 60);
        assert_eq!(d(Codec::H264, Local, Some(1)), 1);
        assert_eq!(d(Codec::H264, Lan, None), 60);
        assert_eq!(d(Codec::H264, Lan, Some(1)), 2);
        assert_eq!(d(Codec::H264, Lan, Some(45)), 45);
        assert_eq!(d(Codec::H264, Tunnel, None), 30, "Connect defaults to 30, not the sim's 24");
        assert_eq!(d(Codec::H264, Tunnel, Some(60)), 30);
        assert_eq!(d(Codec::H264, Tunnel, Some(1)), 2);
        // VP8: ≤ 15 above 720p (and when the height isn't known yet), ≤ 30
        // at 720p or less, on every path.
        for p in [Local, Lan, Tunnel] {
            assert_eq!(d(Codec::Vp8, p, None), 15, "{p:?}: unknown height takes the 1080p row");
            assert_eq!(d(Codec::Vp8, p, Some(60)), 15);
            assert_eq!(clamp_fps_sized(SourceKind::Desktop, Codec::Vp8, p, Some(60), Some(1080)), 15);
            assert_eq!(clamp_fps_sized(SourceKind::Desktop, Codec::Vp8, p, Some(60), Some(720)), 30);
            assert_eq!(clamp_fps_sized(SourceKind::Desktop, Codec::Vp8, p, None, Some(480)), 30);
            assert_eq!(clamp_fps_sized(SourceKind::Desktop, Codec::Vp8, p, Some(10), Some(1080)), 10);
        }
        assert_eq!(d(Codec::Vp8, Lan, Some(1)), 2);
        assert_eq!(cap_sized(SourceKind::Desktop, Codec::Vp8, Tunnel, Some(720)), 30, "a 720p desktop's layer may run at 30");
        assert_eq!(cap_sized(SourceKind::Desktop, Codec::Vp8, Tunnel, Some(1080)), 15);
        assert_eq!(cap_sized(SourceKind::Desktop, Codec::Vp8, Tunnel, None), cap(SourceKind::Desktop, Codec::Vp8, Tunnel));
        assert_eq!(d(Codec::Vp8, Local, Some(1)), 1);
        // JPEG: ≤ 15 here, 2–8 elsewhere (default 8).
        assert_eq!(d(Codec::Jpeg, Local, None), 15);
        assert_eq!(d(Codec::Jpeg, Local, Some(30)), 15);
        for p in [Lan, Tunnel] {
            assert_eq!(d(Codec::Jpeg, p, None), 8);
            assert_eq!(d(Codec::Jpeg, p, Some(15)), 8);
            assert_eq!(d(Codec::Jpeg, p, Some(1)), 2);
        }
        // The caps the hub's tiers stop at.
        assert_eq!(cap(SourceKind::Desktop, Codec::H264, Tunnel), 30);
        assert_eq!(cap(SourceKind::Desktop, Codec::Vp8, Local), 15);
        assert_eq!(cap(SourceKind::Desktop, Codec::Jpeg, Lan), 8);
        // The sim's rollback cap never reaches the desktop row.
        assert_eq!(clamp_fps_with(SourceKind::Desktop, Codec::H264, Local, None, 30), 60);
        assert_eq!(clamp_fps_with(SourceKind::Sim, Codec::H264, Local, None, 30), 30);
    }
}
