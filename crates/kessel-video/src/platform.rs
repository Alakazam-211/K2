//! Choosing the OS backend.

#[cfg(any(
    feature = "apple",
    feature = "windows",
    feature = "linux",
    feature = "android"
))]
use crate::api::VideoBackend;
#[cfg(any(
    feature = "apple",
    feature = "windows",
    feature = "linux",
    feature = "android"
))]
use crate::error::VideoError;
use crate::kit::lock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// A lifecycle listener: called with `true` when the app goes to the
/// background and `false` when it comes back.
pub(crate) type BackgroundListener = dyn Fn(bool) + Send + Sync;

/// What the host app tells the Android backend while it runs: whether it is
/// in the background and which way the device is turned.
#[derive(Default)]
pub(crate) struct HostState {
    background: AtomicBool,
    orientation: AtomicU32,
    listeners: Mutex<Vec<Weak<BackgroundListener>>>,
}

impl HostState {
    pub(crate) fn background(&self) -> bool {
        self.background.load(Ordering::Acquire)
    }

    pub(crate) fn orientation(&self) -> u32 {
        self.orientation.load(Ordering::Acquire)
    }

    /// Call `f` on every change of the background state, until `f` is
    /// dropped.
    #[cfg_attr(not(all(feature = "android", target_os = "android")), allow(dead_code))]
    pub(crate) fn listen(&self, f: &Arc<BackgroundListener>) {
        let mut l = lock(&self.listeners);
        l.retain(|w| w.strong_count() > 0);
        l.push(Arc::downgrade(f));
    }

    fn set_background(&self, background: bool) {
        if self.background.swap(background, Ordering::AcqRel) == background {
            return;
        }
        // Called outside the lock: a listener may close or reopen a camera.
        let live: Vec<Arc<BackgroundListener>> = {
            let mut l = lock(&self.listeners);
            l.retain(|w| w.strong_count() > 0);
            l.iter().filter_map(Weak::upgrade).collect()
        };
        for f in live {
            f(background);
        }
    }
}

/// The host app's Android handles and lifecycle, for the Android backend.
/// Other platforms ignore it.
///
/// Keep a clone: the backend learns from it when the app goes to the
/// background ([`AndroidContext::set_background`]) and which way the device
/// is turned ([`AndroidContext::set_device_orientation`]).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct AndroidContext {
    java_vm: usize,
    context: usize,
    host: Arc<HostState>,
}

impl std::fmt::Debug for AndroidContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidContext")
            .field("jni", &(self.java_vm != 0 && self.context != 0))
            .field("background", &self.host.background())
            .field("orientation", &self.host.orientation())
            .finish()
    }
}

impl AndroidContext {
    /// Handles from the host app: the process's `JavaVM*` and a JNI global
    /// reference to an `android.content.Context` (the Activity or the
    /// Application). In a Tauri or winit app, `ndk_context::android_context()`
    /// gives both (`vm()` and `context()`). They are used only to read the
    /// CAMERA permission; without them ([`AndroidContext::default`]) the
    /// permission reads as `NotDetermined` and opening a camera still says
    /// `NoPermission` when it is denied.
    ///
    /// # Safety
    ///
    /// `java_vm` must be the process's `JavaVM*` and `context` a global
    /// reference to a `Context`, both valid for as long as any backend made
    /// from this context lives. Null for either means "none".
    #[allow(unsafe_code)]
    pub unsafe fn new(
        java_vm: *mut std::ffi::c_void,
        context: *mut std::ffi::c_void,
    ) -> AndroidContext {
        AndroidContext {
            java_vm: java_vm as usize,
            context: context as usize,
            host: Arc::default(),
        }
    }

    /// Tell the backend the app went to the background (`true`, from
    /// `onPause` or a `Suspended` event) or came back (`false`, from
    /// `onResume` or `Resumed`). In the background, open cameras close and
    /// report [`crate::SourceEvent::Interrupted`] with
    /// [`crate::Interruption::Background`], new opens answer
    /// [`VideoError::ShutDown`], and subscribers get
    /// [`crate::BackendEvent::Background`]. Back in the foreground they
    /// reopen (on a thread of their own) and report
    /// [`crate::SourceEvent::Resumed`].
    pub fn set_background(&self, background: bool) {
        self.host.set_background(background)
    }

    /// Is the app in the background, as last told?
    pub fn is_background(&self) -> bool {
        self.host.background()
    }

    /// How far the device is turned clockwise from its natural orientation,
    /// in degrees, as `OrientationEventListener` reports it (rounded here to
    /// 0, 90, 180 or 270). From `Display.getRotation()` that is
    /// `(360 - 90 * rotation) % 360`. Camera frames are rotated upright for
    /// it from the next frame. The default is 0 (held upright).
    pub fn set_device_orientation(&self, degrees: u32) {
        let snapped = ((degrees % 360 + 45) / 90 % 4) * 90;
        self.host.orientation.store(snapped, Ordering::Release);
    }

    /// The device orientation, as last told (0, 90, 180 or 270).
    pub fn device_orientation(&self) -> u32 {
        self.host.orientation()
    }

    /// The `JavaVM*` and `Context` global reference, if both were given.
    #[cfg_attr(not(all(feature = "android", target_os = "android")), allow(dead_code))]
    pub(crate) fn jni(&self) -> Option<(usize, usize)> {
        (self.java_vm != 0 && self.context != 0).then_some((self.java_vm, self.context))
    }

    #[cfg_attr(not(all(feature = "android", target_os = "android")), allow(dead_code))]
    pub(crate) fn host(&self) -> &Arc<HostState> {
        &self.host
    }
}

/// Backend settings.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SystemConfig {
    /// Android: the JNI handles and lifecycle from the host app. Other
    /// platforms ignore it.
    pub android: Option<AndroidContext>,
    /// Use the software VP8 even when a hardware VP8 exists.
    pub force_software_vp8: bool,
}

/// The OS backend for this target (feature `system`, or the per-OS
/// feature). `Unsupported` on a target whose backend isn't built.
#[cfg(any(
    feature = "apple",
    feature = "windows",
    feature = "linux",
    feature = "android"
))]
pub fn system(config: SystemConfig) -> Result<Arc<dyn VideoBackend>, VideoError> {
    #[cfg(all(feature = "android", target_os = "android"))]
    {
        crate::android::backend(config.android.unwrap_or_default())
    }
    #[cfg(not(all(feature = "android", target_os = "android")))]
    {
        let _ = config;
        os::backend()
    }
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

#[cfg(all(feature = "android", target_os = "android"))]
mod os {
    pub(super) use crate::android::os_name as name;
}

#[cfg(not(any(
    all(feature = "apple", target_os = "macos"),
    all(feature = "windows", windows),
    all(feature = "linux", target_os = "linux"),
    all(feature = "android", target_os = "android")
)))]
mod os {
    #[cfg(any(
        feature = "apple",
        feature = "windows",
        feature = "linux",
        feature = "android"
    ))]
    pub(super) fn backend(
    ) -> Result<std::sync::Arc<dyn crate::api::VideoBackend>, crate::error::VideoError> {
        Err(crate::error::unsupported(format!(
            "no kessel-video backend for {}",
            std::env::consts::OS
        )))
    }

    pub(super) fn name() -> String {
        std::env::consts::OS.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_state_reaches_listeners_once_per_change() {
        let ctx = AndroidContext::default();
        assert!(ctx.jni().is_none(), "no JNI handles by default");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let f: Arc<BackgroundListener> = Arc::new(move |bg| lock(&s).push(bg));
        ctx.host().listen(&f);
        ctx.set_background(true);
        ctx.set_background(true);
        ctx.clone().set_background(false);
        assert_eq!(*lock(&seen), vec![true, false], "clones share the state");
        drop(f);
        ctx.set_background(true);
        assert_eq!(lock(&seen).len(), 2, "a dropped listener hears nothing");
        assert!(ctx.is_background());

        for (given, snapped) in [(0, 0), (44, 0), (46, 90), (269, 270), (316, 0), (450, 90)] {
            ctx.set_device_orientation(given);
            assert_eq!(ctx.device_orientation(), snapped, "{given} degrees");
        }
    }
}
