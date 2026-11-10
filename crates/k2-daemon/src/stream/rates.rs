//! The rate table (fix 2; PRD remote desktop §3.5, E6): a viewer's fps cap
//! by source kind, codec and [`ViewerPath`].
//!
//! **Sim:** JPEG up to 30 fps on this machine, 2–15 on the LAN or over
//! Connect (default 8); H.264 up to 60 on this machine, 2–60 on the LAN
//! (default 60), 2–30 over Connect (default 24). `K2_SIM_VIDEO_MAX_FPS=30`
//! (read once) puts the local and LAN H.264 caps back to 30.

use std::sync::OnceLock;

use k2_core::sim::stream::{ViewerPath, LOCAL_MAX_FPS, LOCAL_VIDEO_MAX_FPS, REMOTE_DEFAULT_FPS, REMOTE_MAX_FPS, REMOTE_MIN_FPS, REMOTE_VIDEO_DEFAULT_FPS, REMOTE_VIDEO_MAX_FPS};
use k2_core::stream::{Codec, SourceKind};

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
        // TODO(S5): the desktop row (PRD §3.5: H.264 ≤ 60 local and LAN,
        // 2–30 over Connect with default 30; VP8 ≤ 15 at 1080p, ≤ 30 at 720p
        // and below; JPEG ≤ 15 local, 2–8 elsewhere). Until S5 lands the
        // desktop route, a desktop source reads the sim row.
        SourceKind::Desktop => sim_row(codec, path, asked, video_max),
    }
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
