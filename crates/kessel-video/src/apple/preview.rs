//! A camera source's own `AVCaptureSession`, handed to the host for a
//! self-view with no hop ([`ApplePreview`]).

use super::ffi::{cm_time, CMTime};
use super::objc::{is_main_thread, ObjSend};
use crate::error::VideoError;
use crate::types::Facing;
use objc2::msg_send;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use std::ffi::c_void;
use std::ptr::NonNull;

/// macOS and iOS: a handle on a camera source's own `AVCaptureSession`, for
/// a self-view with no hop. From [`FrameSource::apple_preview`].
///
/// The usual self-view path is capture callback, then the app's sink, then
/// an `AVSampleBufferDisplayLayer`, then the next vsync: one to three frames
/// late. An `AVCaptureVideoPreviewLayer` on the session itself is drawn by
/// the system straight from the camera, as the Camera app and FaceTime
/// draw theirs. The source's frames keep coming through
/// [`FrameSource::try_recv`] to the encoder exactly as before: the layer is
/// a second output of the same session.
///
/// No AVFoundation or objc2 type is in this API. The session and the layer
/// cross as raw, opaque pointers (`NonNull<c_void>`) for the host's own
/// Objective-C or Swift code.
///
/// # Ownership and lifetime
///
/// - The handle holds a strong reference (a retain) on the session; each
///   clone takes another, and dropping one releases it. It is `Send`,
///   `Sync`, `Clone` and `Debug`: holding, cloning, sending and dropping it
///   need no `unsafe`. Only [`capture_session`](ApplePreview::capture_session)
///   and [`new_preview_layer`](ApplePreview::new_preview_layer) are `unsafe`
///   to call.
/// - Take it from the camera source itself, right after opening it: a
///   wrapper `FrameSource` that doesn't forward `apple_preview` answers
///   `None`.
/// - The session runs while the source is open. Pausing the source
///   ([`FrameSource::set_paused`]) stops only the frames to the caller: the
///   session, and so the preview, keeps running. An interruption (iOS: a
///   call, the background, another app taking the camera) stops both, and
///   both come back with `Resumed`.
/// - Dropping the source stops the session, and once a handle has been
///   taken, also takes the camera input and the source's video output out
///   of it. A layer or handle that outlives the source then holds a
///   stopped, empty session: no camera, no new frames. kessel-video doesn't
///   clear the layer's picture, and whether AVFoundation shows the last
///   frame or nothing once the input is gone is not promised (a stopped
///   preview keeps its last frame), so never rely on it going black: take
///   the layer down (or hide it) when the source goes. Each camera flip or
///   reopen is a new source with a new session, so it needs a new layer.
/// - [`capture_session`](ApplePreview::capture_session) lends the pointer
///   (+0); [`new_preview_layer`](ApplePreview::new_preview_layer) gives the
///   caller an owned layer (+1). The layer keeps its own strong reference
///   on the session (`AVCaptureVideoPreviewLayer.session` is strong).
///
/// # Mirroring
///
/// - The encoded frames are never mirrored by this. On iOS kessel-video sets
///   the source's video output unmirrored (the far end sees what the camera
///   sees); on macOS it leaves AVFoundation's default for a video data
///   output, which is unmirrored.
/// - A preview layer from [`new_preview_layer`](ApplePreview::new_preview_layer)
///   is mirrored as asked, or with `None` as a self-view is: mirrored when
///   the camera faces the person ([`Facing::Front`]: an iPhone's or iPad's
///   front camera, a Mac's built-in camera), not otherwise. Automatic
///   mirroring is turned off on the layer's connection, so AVFoundation
///   doesn't change it later. Mirroring the layer never changes the encoded
///   frames.
///
/// # Rotation
///
/// - The encoded frames: on iOS kessel-video turns the video output's
///   connection by the way the phone is held (iOS 17+:
///   `AVCaptureDevice.RotationCoordinator`'s horizon-level capture angle;
///   before, `UIDevice.orientation`), so frames come out upright. Nothing
///   turns on macOS.
/// - The preview layer is never turned by kessel-video: a layer follows the
///   app's interface, not the phone. AVFoundation's default on iOS is
///   portrait, which is right for a portrait-only interface. An interface
///   that rotates sets the layer connection's `videoRotationAngle` from a
///   `RotationCoordinator(device:previewLayer:)`'s
///   `videoRotationAngleForHorizonLevelPreview` (iOS 17+), or its
///   `videoOrientation` from the interface orientation (before), on the
///   main thread.
///
/// # Example
///
/// ```no_run
/// use kessel_video::{no_wake, system, FrameSource, SystemConfig, VideoBackend};
///
/// let backend = system(SystemConfig::default())?;
/// let camera: Box<dyn FrameSource> =
///     backend.open_camera(None, Default::default(), no_wake())?;
/// let preview = camera.apple_preview().expect("an Apple camera source has one");
/// // On the main thread. SAFETY: the main thread; the layer (+1) goes to
/// // Swift, which takes it over with `takeRetainedValue()` and so
/// // releases it once.
/// let layer = unsafe { preview.new_preview_layer(None)? };
/// hand_to_swift(layer.as_ptr());
/// // The camera's frames still come to the encoder:
/// let _frame = camera_frames(camera);
/// # fn hand_to_swift(_: *mut std::ffi::c_void) {}
/// # fn camera_frames(mut c: Box<dyn FrameSource>) -> Option<kessel_video::VideoFrame> { c.try_recv() }
/// # Ok::<(), kessel_video::VideoError>(())
/// ```
///
/// In Swift, on the main thread:
///
/// ```swift
/// let layer = Unmanaged<AVCaptureVideoPreviewLayer>
///     .fromOpaque(ptr).takeRetainedValue()
/// layer.videoGravity = .resizeAspectFill
/// layer.frame = selfView.bounds
/// selfView.layer.addSublayer(layer)
/// ```
///
/// [`FrameSource::apple_preview`]: crate::FrameSource::apple_preview
/// [`FrameSource::try_recv`]: crate::FrameSource::try_recv
/// [`FrameSource::set_paused`]: crate::FrameSource::set_paused
#[derive(Clone)]
pub struct ApplePreview {
    session: ObjSend,
    device: ObjSend,
    facing: Facing,
    /// The frame rate the source fixed at open, if it fixed one.
    fixed_fps: Option<u32>,
}

impl std::fmt::Debug for ApplePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplePreview")
            .field("session", &Retained::as_ptr(&self.session.0))
            .field("facing", &self.facing)
            .field("fixed_fps", &self.fixed_fps)
            .finish()
    }
}

impl ApplePreview {
    /// A handle on `session`, the capture session of `device`.
    pub(crate) fn new(
        session: Retained<AnyObject>,
        device: Retained<AnyObject>,
        facing: Facing,
        fixed_fps: Option<u32>,
    ) -> ApplePreview {
        ApplePreview {
            session: ObjSend(session),
            device: ObjSend(device),
            facing,
            fixed_fps,
        }
    }

    /// Which way the source's camera faces. A preview layer from
    /// [`new_preview_layer(None)`](ApplePreview::new_preview_layer) is
    /// mirrored when this is [`Facing::Front`].
    pub fn facing(&self) -> Facing {
        self.facing
    }

    /// The source's `AVCaptureSession *`, lent (+0): valid while this
    /// handle (or a clone of it) lives. In Swift,
    /// `Unmanaged<AVCaptureSession>.fromOpaque(ptr).takeUnretainedValue()`
    /// takes a strong reference of its own.
    ///
    /// For a host that builds its own preview: an
    /// `AVCaptureVideoPreviewLayer(session:)`, or SwiftUI or UIKit views
    /// that take a session. [`new_preview_layer`](ApplePreview::new_preview_layer)
    /// does that, with the mirroring rules above, for hosts that would
    /// rather not.
    ///
    /// # Safety
    ///
    /// The session belongs to the source. With it the caller may:
    /// - read its properties;
    /// - attach and detach `AVCaptureVideoPreviewLayer`s (`init(session:)`,
    ///   `layer.session = …`, `setSessionWithNoConnection:` with a
    ///   connection of its own), and set the mirroring, rotation and
    ///   enabled state of those layers' connections.
    ///
    /// It must not, from any thread:
    /// - start or stop it, or call `beginConfiguration` or
    ///   `commitConfiguration`, other than as attaching a preview layer
    ///   does;
    /// - add or remove inputs or outputs;
    /// - change its preset, or the device's format, frame rate, focus or
    ///   exposure;
    /// - touch the source's own video data output or its connection
    ///   (its delegate, its pixel format, its mirroring, its rotation).
    ///
    /// Any of those can stop, reformat or misroute the frames the source
    /// hands to the encoder, or race the source's own teardown. The pointer
    /// must not be released by the caller (it is lent, not given), nor used
    /// after the last handle is dropped unless the caller took its own
    /// reference first.
    pub unsafe fn capture_session(&self) -> NonNull<c_void> {
        // A Retained is never null.
        NonNull::from(&*self.session.0).cast()
    }

    /// A new `AVCaptureVideoPreviewLayer *` on the source's session, owned
    /// by the caller (+1). `mirrored`: `Some(true)` or `Some(false)` sets
    /// it; `None` mirrors as a self-view does, when the camera faces the
    /// person ([`Facing::Front`]). Automatic mirroring is turned off on the
    /// layer's connection. The layer's video gravity, frame and rotation
    /// are AVFoundation's defaults (`resizeAspect`, a zero frame, portrait
    /// on iOS): the host sets them. One layer per self-view: attaching
    /// one reconfigures the session briefly, so make it once, soon after
    /// opening the source, rather than per view update.
    ///
    /// If the session's frame rate was fixed at open and attaching the
    /// layer moved it, the rate is fixed again. `Unsupported` where
    /// AVFoundation has no `AVCaptureVideoPreviewLayer`; an OS error if it
    /// gives no layer.
    ///
    /// # Safety
    ///
    /// - **Main thread.** The layer is a `CALayer`: call this on the main
    ///   thread, and on the main thread only put it in a layer tree,
    ///   change its properties or its connection, or take it out. Called
    ///   on another thread, the layer is made inside an explicit
    ///   `CATransaction` (so no implicit one is left open), but it must
    ///   still be handed to the main thread before any of those.
    /// - **Ownership.** The caller owns the returned reference and releases
    ///   it exactly once: Swift's
    ///   `Unmanaged<AVCaptureVideoPreviewLayer>.fromOpaque(ptr).takeRetainedValue()`,
    ///   `CFRelease`, or `-release` / `CFBridgingRelease` in Objective-C.
    ///   Never more than once, and not with `takeUnretainedValue()`, which
    ///   leaks it.
    /// - **Lifetime.** The layer keeps the session alive on its own. After
    ///   the source is dropped the session is stopped and empty (see
    ///   [`ApplePreview`]): the layer shows nothing new; remove it.
    pub unsafe fn new_preview_layer(
        &self,
        mirrored: Option<bool>,
    ) -> Result<NonNull<c_void>, VideoError> {
        let cls = AnyClass::get(c"AVCaptureVideoPreviewLayer").ok_or_else(|| {
            VideoError::Unsupported("AVFoundation has no AVCaptureVideoPreviewLayer".into())
        })?;
        let mirror = mirrored.unwrap_or(self.facing == Facing::Front);
        let transaction = if is_main_thread() {
            None
        } else {
            AnyClass::get(c"CATransaction")
        };
        let layer = autoreleasepool(|_| {
            if let Some(t) = transaction {
                let _: () = msg_send![t, begin];
            }
            let layer: Option<Retained<AnyObject>> =
                msg_send![cls, layerWithSession: &*self.session];
            if let Some(layer) = &layer {
                mirror_connection(layer, mirror);
            }
            if let Some(t) = transaction {
                let _: () = msg_send![t, commit];
            }
            layer
        });
        let layer = layer.ok_or_else(|| VideoError::os(0, "no AVCaptureVideoPreviewLayer"))?;
        self.fix_rate_again();
        // The caller's +1: into_raw keeps the retain the layer was made with.
        Ok(NonNull::new_unchecked(Retained::into_raw(layer)).cast())
    }

    /// Fix the source's frame rate again if attaching a layer moved it.
    fn fix_rate_again(&self) {
        let Some(fps) = self.fixed_fps else {
            return;
        };
        let want = cm_time(1, fps as i32);
        autoreleasepool(|_| {
            // SAFETY: the live device of the session; AVFoundation's
            // documented selectors; lock and unlock balance.
            unsafe {
                let now: CMTime = msg_send![&*self.device, activeVideoMinFrameDuration];
                if super::ffi::CMTimeCompare(now, want) == 0 {
                    return;
                }
                let mut err: *mut AnyObject = std::ptr::null_mut();
                let locked: Bool = msg_send![&*self.device, lockForConfiguration: &mut err];
                if locked.as_bool() {
                    let _: () = msg_send![&*self.device, setActiveVideoMinFrameDuration: want];
                    let _: () = msg_send![&*self.device, setActiveVideoMaxFrameDuration: want];
                    let _: () = msg_send![&*self.device, unlockForConfiguration];
                }
            }
        });
    }
}

/// Set a preview layer's connection mirrored or not, with automatic
/// mirroring off. A connection that can't mirror is left alone.
///
/// # Safety
/// `layer` must be a live AVCaptureVideoPreviewLayer.
unsafe fn mirror_connection(layer: &AnyObject, mirrored: bool) {
    let conn: *mut AnyObject = msg_send![layer, connection];
    if conn.is_null() {
        return;
    }
    let ok: Bool = msg_send![conn, isVideoMirroringSupported];
    if ok.as_bool() {
        let _: () = msg_send![conn, setAutomaticallyAdjustsVideoMirroring: Bool::NO];
        let _: () = msg_send![conn, setVideoMirrored: Bool::new(mirrored)];
    }
}

/// Is a preview layer's connection mirrored? (Tests.)
///
/// # Safety
/// `layer` must be a live AVCaptureVideoPreviewLayer.
#[cfg(test)]
pub(crate) unsafe fn layer_mirrored(layer: *mut AnyObject) -> Option<bool> {
    let conn: *mut AnyObject = msg_send![layer, connection];
    if conn.is_null() {
        return None;
    }
    let b: Bool = msg_send![conn, isVideoMirrored];
    Some(b.as_bool())
}

/// The session a preview layer shows. (Tests.)
///
/// # Safety
/// `layer` must be a live AVCaptureVideoPreviewLayer.
#[cfg(test)]
pub(crate) unsafe fn layer_session(layer: *mut AnyObject) -> *mut AnyObject {
    msg_send![layer, session]
}
