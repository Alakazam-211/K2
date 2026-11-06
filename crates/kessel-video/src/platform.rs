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
/// feature). `Unsupported` on a target whose backend isn't built.
#[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
pub fn system(config: SystemConfig) -> Result<Arc<dyn VideoBackend>, VideoError> {
    let _ = config;
    os::backend()
}

/// The OS and version, for logs.
pub(crate) fn os_name() -> String {
    os::name()
}

#[cfg(all(feature = "apple", target_os = "macos"))]
mod os {
    pub(super) use crate::apple::{backend, os_name as name};
}

#[cfg(all(feature = "windows", windows))]
mod os {
    pub(super) use crate::win::{backend, os_name as name};
}

#[cfg(all(feature = "linux", target_os = "linux"))]
mod os {
    pub(super) use crate::linux::{backend, os_name as name};
}

#[cfg(not(any(
    all(feature = "apple", target_os = "macos"),
    all(feature = "windows", windows),
    all(feature = "linux", target_os = "linux")
)))]
mod os {
    #[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
    pub(super) fn backend() -> Result<std::sync::Arc<dyn crate::api::VideoBackend>, crate::error::VideoError> {
        Err(crate::error::unsupported(format!("no kessel-video backend for {}", std::env::consts::OS)))
    }

    pub(super) fn name() -> String {
        std::env::consts::OS.to_string()
    }
}
