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

/// A change to what the host said about screen capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenChange {
    /// The display's size or density changed (it turned).
    Display,
    /// A MediaProjection was handed over.
    Given,
    /// The MediaProjection stopped.
    Stopped,
}

/// A screen listener: called on every [`ScreenChange`].
pub(crate) type ScreenListener = dyn Fn(ScreenChange) + Send + Sync;

/// The MediaProjection the host handed over, and the display it mirrors.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScreenState {
    /// The JNI global reference to the `MediaProjection`, as an address.
    pub(crate) projection: Option<usize>,
    /// The display's width and height in pixels now, and its density.
    pub(crate) display: (u32, u32, u32),
}

/// What the host app tells the Android backend while it runs: whether it is
/// in the background, which way the device is turned, and the screen it
/// lets the app capture.
#[derive(Default)]
pub(crate) struct HostState {
    background: AtomicBool,
    orientation: AtomicU32,
    listeners: Mutex<Vec<Weak<BackgroundListener>>>,
    /// Held while a virtual display is made from the projection, so the
    /// host's `media_projection_stopped` waits for that to finish.
    screen: Mutex<ScreenState>,
    screen_listeners: Mutex<Vec<Weak<ScreenListener>>>,
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

    /// The screen state, locked: the projection can't be dropped while
    /// the guard lives.
    pub(crate) fn screen(&self) -> std::sync::MutexGuard<'_, ScreenState> {
        lock(&self.screen)
    }

    /// Call `f` on every [`ScreenChange`], until `f` is dropped.
    #[cfg_attr(not(all(feature = "android", target_os = "android")), allow(dead_code))]
    pub(crate) fn listen_screen(&self, f: &Arc<ScreenListener>) {
        let mut l = lock(&self.screen_listeners);
        l.retain(|w| w.strong_count() > 0);
        l.push(Arc::downgrade(f));
    }

    fn screen_changed(&self, change: ScreenChange) {
        // Called outside the locks: a listener may resize or close screens.
        let live: Vec<Arc<ScreenListener>> = {
            let mut l = lock(&self.screen_listeners);
            l.retain(|w| w.strong_count() > 0);
            l.iter().filter_map(Weak::upgrade).collect()
        };
        for f in live {
            f(change);
        }
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

/// The host app's Android handles and lifecycle, and the MediaProjection
/// for screen capture, for the Android backend. Other platforms ignore it.
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
            .field("media_projection", &self.has_media_projection())
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

    /// Hand over a MediaProjection the person allowed, for screen capture:
    /// the result of `MediaProjectionManager.getMediaProjection` after the
    /// consent from `createScreenCaptureIntent`. The display it mirrors is
    /// `width` x `height` pixels now, at `density_dpi` (from
    /// `WindowMetrics` or `DisplayMetrics`). From here the backend lists one
    /// screen source, `pick_screen` answers it without a picker, and
    /// `open_screen` makes a `VirtualDisplay` into an `ImageReader`.
    ///
    /// The app's part, which needs Java or Kotlin:
    /// - Run a foreground service with
    ///   `foregroundServiceType="mediaProjection"` (and the
    ///   `FOREGROUND_SERVICE_MEDIA_PROJECTION` permission on Android 14+)
    ///   before asking for the projection, for as long as the screen is
    ///   shared.
    /// - On Android 14+, register a `MediaProjection.Callback` before handing
    ///   the projection over (Android throws otherwise), and from its
    ///   `onStop` call [`AndroidContext::media_projection_stopped`].
    /// - Call [`AndroidContext::set_screen_size`] when the display turns or
    ///   changes size.
    ///
    /// Android allows one virtual display per projection: open one screen
    /// source per projection.
    ///
    /// # Safety
    ///
    /// `projection` must be a JNI global reference to an
    /// `android.media.projection.MediaProjection`, valid until
    /// [`AndroidContext::media_projection_stopped`] returns (or until a
    /// later `set_media_projection` returns). The host deletes it after
    /// that; kessel-video keeps no reference to it.
    #[allow(unsafe_code)]
    pub unsafe fn set_media_projection(
        &self,
        projection: *mut std::ffi::c_void,
        width: u32,
        height: u32,
        density_dpi: u32,
    ) {
        {
            let mut st = self.host.screen();
            st.projection = (!projection.is_null()).then_some(projection as usize);
            st.display = (width, height, density_dpi);
        }
        self.host.screen_changed(ScreenChange::Given);
    }

    /// The display the projection mirrors turned or changed size: open
    /// screen sources resize their virtual display to keep the picture
    /// upright and unstretched (frames report `Started` with the new size).
    pub fn set_screen_size(&self, width: u32, height: u32, density_dpi: u32) {
        {
            let mut st = self.host.screen();
            if st.display == (width, height, density_dpi) {
                return;
            }
            st.display = (width, height, density_dpi);
        }
        self.host.screen_changed(ScreenChange::Display);
    }

    /// The MediaProjection stopped (its callback's `onStop`: the person
    /// stopped sharing, or the system did). Open screen sources end with
    /// [`crate::EndReason::SourceClosed`] and release their virtual
    /// displays, and screen capture is unsupported again until the next
    /// [`AndroidContext::set_media_projection`]. Once this returns,
    /// kessel-video no longer uses the projection's reference.
    pub fn media_projection_stopped(&self) {
        let had = self.host.screen().projection.take().is_some();
        if had {
            self.host.screen_changed(ScreenChange::Stopped);
        }
    }

    /// Is there a projection to capture the screen with?
    pub fn has_media_projection(&self) -> bool {
        self.host.screen().projection.is_some()
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

#[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
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
    all(feature = "apple", any(target_os = "macos", target_os = "ios")),
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
