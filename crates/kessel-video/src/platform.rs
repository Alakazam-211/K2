//! Choosing the OS backend.

#[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
use crate::api::VideoBackend;
#[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
use crate::error::VideoError;
#[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
use std::sync::Arc;

/// The host's Android JNI handles (reserved: the Android backend isn't
/// built yet).
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct AndroidContext {}

/// Backend settings.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SystemConfig {
    /// Android: the JNI env and Context come from the host. Other platforms
    /// ignore it.
    pub android: Option<AndroidContext>,
    /// Use the software VP8 even when a hardware VP8 exists.
    pub force_software_vp8: bool,
}

/// The OS backend for this target (feature `system`, or the per-OS
/// feature). `Unsupported` on a target whose backend isn't built yet
/// (macOS is; Windows and Linux are next).
#[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
pub fn system(config: SystemConfig) -> Result<Arc<dyn VideoBackend>, VideoError> {
    let _ = config;
    #[cfg(all(feature = "apple", target_os = "macos"))]
    {
        crate::apple::backend()
    }
    #[cfg(not(all(feature = "apple", target_os = "macos")))]
    {
        Err(crate::error::unsupported(format!(
            "no kessel-video backend for {} yet",
            std::env::consts::OS
        )))
    }
}

/// The OS and version, for logs.
pub(crate) fn os_name() -> String {
    #[cfg(all(feature = "apple", target_os = "macos"))]
    {
        crate::apple::os_name()
    }
    #[cfg(not(all(feature = "apple", target_os = "macos")))]
    {
        std::env::consts::OS.to_string()
    }
}
