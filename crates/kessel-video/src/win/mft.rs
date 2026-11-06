//! Media Foundation transforms (MFTs): finding the OS's H.264 encoders and
//! decoders, opening one, and driving it one frame at a time.
//!
//! Two kinds are used, both shipped with Windows or its GPU drivers:
//! - **Hardware MFTs** (`MFT_ENUM_FLAG_HARDWARE`), from the GPU's driver.
//!   They are asynchronous: they say when they want input and when output
//!   is ready through their event queue (`METransformNeedInput`,
//!   `METransformHaveOutput`).
//! - **Microsoft's software MFTs**, picked by CLSID so that no other
//!   software MFT an app registered on the machine is ever used. They are
//!   synchronous: `ProcessInput`, then `ProcessOutput` until it wants more.
//!
//! [`Mft::process`] hides the difference: one input sample in, the output
//! samples for it out.

use super::camera::string_attr;
use super::map_error;
use super::mfdll::mfplat;
use crate::error::{unsupported, VideoError};
use crate::types::ImplPreference;
use std::time::{Duration, Instant};
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Media::MediaFoundation::*;

/// What an MFT does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    /// NV12 in, H.264 out.
    Encoder,
    /// H.264 in, NV12 out.
    Decoder,
}

/// One MFT Media Foundation offers, not yet opened.
#[derive(Clone)]
pub(super) struct Found {
    pub(super) activate: IMFActivate,
    /// Enumerated with `MFT_ENUM_FLAG_HARDWARE` and carrying a hardware URL.
    pub(super) hardware: bool,
    /// The friendly name, for logs.
    pub(super) name: String,
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found")
            .field("hardware", &self.hardware)
            .field("name", &self.name)
            .finish()
    }
}

fn h264_type() -> MFT_REGISTER_TYPE_INFO {
    MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    }
}

/// Does the activation object say it is a hardware MFT?
fn has_hardware_url(a: &IMFAttributes) -> bool {
    // SAFETY: a read of a live attribute store.
    unsafe { a.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute) }.is_ok()
}

/// Run `f` on a short-lived thread of its own and wait for it. Opening an
/// MFT joins the calling thread to COM's multithreaded apartment; the
/// caller's thread may be an app's UI thread that must stay free to join a
/// single-threaded one later (a webview does). The MFTs are free-threaded,
/// so what `f` opens works from the codec's own thread afterwards.
pub(super) fn on_com_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("kessel-video-mf".into())
            .spawn_scoped(s, f)
            .map(|t| t.join())
    })
    .unwrap_or_else(|e| panic!("kessel-video: no thread to open Media Foundation on: {e}"))
    .unwrap_or_else(|p| std::panic::resume_unwind(p))
}

/// The H.264 MFTs for `role`, in the order `pref` asks for: hardware ones
/// (Media Foundation's merit order), then Microsoft's software MFT. Empty
/// where Windows has no H.264 (an N or KN edition without the Media Feature
/// Pack); `Unsupported` where Media Foundation itself is missing.
pub(super) fn find(role: Role, pref: ImplPreference) -> Result<Vec<Found>, VideoError> {
    super::camera::init()?;
    let mf = mfplat()?;
    let t = h264_type();
    let (category, input, output, ms_clsid) = match role {
        Role::Encoder => (
            MFT_CATEGORY_VIDEO_ENCODER,
            None,
            Some(&t),
            CLSID_MSH264EncoderMFT,
        ),
        Role::Decoder => (
            MFT_CATEGORY_VIDEO_DECODER,
            Some(&t),
            None,
            CLSID_MSH264DecoderMFT,
        ),
    };
    let named = |a: &IMFActivate| {
        string_attr(a, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_else(|| "H.264 MFT".into())
    };
    let mut out = Vec::new();
    if pref != ImplPreference::SoftwareOnly {
        let flags = MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0;
        // An enumeration error is "none": a machine without the GPU's MFTs.
        for a in mf
            .tenum_ex(category, flags as u32, input, output)
            .unwrap_or_default()
        {
            if has_hardware_url(&a) {
                out.push(Found {
                    name: named(&a),
                    activate: a,
                    hardware: true,
                });
            }
        }
    }
    if pref != ImplPreference::HardwareOnly {
        let flags = MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0;
        for a in mf
            .tenum_ex(category, flags as u32, input, output)
            .unwrap_or_default()
        {
            // SAFETY: a read of a live attribute store.
            let clsid = unsafe { a.GetGUID(&MFT_TRANSFORM_CLSID_Attribute) }.ok();
            if clsid == Some(ms_clsid) && !has_hardware_url(&a) {
                out.push(Found {
                    name: named(&a),
                    activate: a,
                    hardware: false,
                });
            }
        }
    }
    Ok(out)
}

/// `HRESULT_FROM_WIN32` codes and DXGI codes that mean the GPU went away.
const DXGI_ERROR_DEVICE_REMOVED: HRESULT = HRESULT(0x887A_0005_u32 as i32);
const DXGI_ERROR_DEVICE_RESET: HRESULT = HRESULT(0x887A_0007_u32 as i32);

/// Is `e` the hardware session going away (a GPU reset, a driver update)?
pub(super) fn is_lost(e: &VideoError) -> bool {
    matches!(e, VideoError::DeviceLost)
        || matches!(e, VideoError::Os { code, .. }
            if [
                DXGI_ERROR_DEVICE_REMOVED,
                DXGI_ERROR_DEVICE_RESET,
                MF_E_SHUTDOWN,
                MF_E_HW_MFT_FAILED_START_STREAMING,
            ]
            .iter()
            .any(|h| h.0 as i64 == *code))
}

/// How long one frame may take before the MFT counts as stuck.
const STUCK: Duration = Duration::from_secs(3);
/// After an asynchronous MFT asks for more input without giving output,
/// how long to wait for that output before draining it.
const ASYNC_OUTPUT_WAIT: Duration = Duration::from_millis(40);

/// What [`Mft::process`] does when an input gives no output at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoOutput {
    /// Drain the MFT so this input's output comes out now (an encoder: one
    /// access unit per frame, never held back).
    Drain,
    /// Accept that there is none yet (a decoder fed only parameter sets).
    Wait,
}

/// An open MFT.
pub(super) struct Mft {
    pub(super) transform: IMFTransform,
    activate: IMFActivate,
    /// Asynchronous MFTs only.
    events: Option<IMFMediaEventGenerator>,
    /// `METransformNeedInput` events not yet answered with an input.
    credits: u32,
    pub(super) hardware: bool,
    pub(super) name: String,
    in_id: u32,
    out_id: u32,
    out_info: MFT_OUTPUT_STREAM_INFO,
    /// Output buffer size when the MFT neither says nor allocates.
    pub(super) out_size_hint: u32,
    streaming: bool,
}

// SAFETY: MFTs are free-threaded COM objects (Microsoft's are registered
// "Both"; hardware MFTs are asynchronous, which requires it). One thread at
// a time drives each (&mut self).
unsafe impl Send for Mft {}

impl Mft {
    /// Create the MFT behind `found`. Asynchronous ones are unlocked; low
    /// latency is asked for (decoders then output each frame at once).
    pub(super) fn open(found: &Found) -> Result<Mft, VideoError> {
        // SAFETY: activation of an object Media Foundation enumerated.
        let transform: IMFTransform = unsafe { found.activate.ActivateObject() }
            .map_err(|e| map_error(&e, "ActivateObject"))?;
        let mut mft = Mft {
            transform,
            activate: found.activate.clone(),
            events: None,
            credits: 0,
            hardware: found.hardware,
            name: found.name.clone(),
            in_id: 0,
            out_id: 0,
            out_info: MFT_OUTPUT_STREAM_INFO::default(),
            out_size_hint: 1 << 20,
            streaming: false,
        };
        // SAFETY: attribute reads and writes on the MFT's own store; the
        // event generator is the same object (asynchronous MFTs must have
        // it).
        unsafe {
            if let Ok(a) = mft.transform.GetAttributes() {
                if a.GetUINT32(&MF_TRANSFORM_ASYNC) == Ok(1) {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .map_err(|e| map_error(&e, "MF_TRANSFORM_ASYNC_UNLOCK"))?;
                    mft.events = Some(
                        mft.transform
                            .cast()
                            .map_err(|e| map_error(&e, "IMFMediaEventGenerator"))?,
                    );
                }
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let (mut i, mut o) = ([0u32], [0u32]);
            // E_NOTIMPL means fixed streams numbered from 0.
            if mft.transform.GetStreamIDs(&mut i, &mut o).is_ok() {
                (mft.in_id, mft.out_id) = (i[0], o[0]);
            }
        }
        Ok(mft)
    }

    pub(super) fn in_id(&self) -> u32 {
        self.in_id
    }

    pub(super) fn out_id(&self) -> u32 {
        self.out_id
    }

    /// The MFT's ICodecAPI, if it has one.
    pub(super) fn codec_api(&self) -> Option<ICodecAPI> {
        self.transform.cast().ok()
    }

    /// Re-read the output stream's buffer needs (after its type changes).
    pub(super) fn refresh_output_info(&mut self) -> Result<(), VideoError> {
        // SAFETY: a query on a live MFT with its types set.
        self.out_info = unsafe { self.transform.GetOutputStreamInfo(self.out_id) }
            .map_err(|e| map_error(&e, "GetOutputStreamInfo"))?;
        Ok(())
    }

    /// Start streaming, once both media types are set.
    pub(super) fn begin(&mut self) -> Result<(), VideoError> {
        self.refresh_output_info()?;
        // SAFETY: notifications on a live MFT whose types are set.
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| map_error(&e, "MFT_MESSAGE_NOTIFY_BEGIN_STREAMING"))?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| map_error(&e, "MFT_MESSAGE_NOTIFY_START_OF_STREAM"))?;
        }
        self.streaming = true;
        Ok(())
    }

    /// Throw away everything inside (a decoder's reset).
    pub(super) fn flush(&mut self) -> Result<(), VideoError> {
        // SAFETY: a command on a live MFT.
        unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0) }
            .map_err(|e| map_error(&e, "MFT_MESSAGE_COMMAND_FLUSH"))?;
        if let Some(ev) = &self.events {
            // A flushed asynchronous MFT asks for input afresh.
            self.credits = 0;
            while let Some(t) = next_event(ev)? {
                if t == METransformNeedInput {
                    self.credits += 1;
                }
            }
        }
        Ok(())
    }

    /// One input sample in; the output samples it gave out. `on_change` is
    /// called when the output type changes (a decoder learning the stream's
    /// size); it must set a new output type.
    pub(super) fn process(
        &mut self,
        input: &IMFSample,
        no_output: NoOutput,
        on_change: &mut dyn FnMut(&IMFTransform) -> Result<(), VideoError>,
    ) -> Result<Vec<IMFSample>, VideoError> {
        if self.events.is_some() {
            self.process_async(input, no_output, on_change)
        } else {
            self.process_sync(input, no_output, on_change)
        }
    }

    fn process_sync(
        &mut self,
        input: &IMFSample,
        no_output: NoOutput,
        on_change: &mut dyn FnMut(&IMFTransform) -> Result<(), VideoError>,
    ) -> Result<Vec<IMFSample>, VideoError> {
        let mut out = Vec::new();
        loop {
            // SAFETY: a live MFT and sample.
            match unsafe { self.transform.ProcessInput(self.in_id, input, 0) } {
                Ok(()) => break,
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    let before = out.len();
                    self.pull_all(&mut out, on_change)?;
                    if out.len() == before {
                        return Err(VideoError::os(
                            e.code().0 as i64,
                            format!("{} takes no input and gives no output", self.name),
                        ));
                    }
                }
                Err(e) => return Err(map_error(&e, "ProcessInput")),
            }
        }
        self.pull_all(&mut out, on_change)?;
        if out.is_empty() && no_output == NoOutput::Drain {
            // SAFETY: a command on a live MFT; it then gives what it holds
            // and takes input again.
            unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) }
                .map_err(|e| map_error(&e, "MFT_MESSAGE_COMMAND_DRAIN"))?;
            self.pull_all(&mut out, on_change)?;
        }
        Ok(out)
    }

    /// `ProcessOutput` until the MFT wants more input.
    fn pull_all(
        &mut self,
        out: &mut Vec<IMFSample>,
        on_change: &mut dyn FnMut(&IMFTransform) -> Result<(), VideoError>,
    ) -> Result<(), VideoError> {
        // Bounded: an MFT that never stops giving output is broken.
        for _ in 0..64 {
            match self.pull_one(on_change)? {
                Pulled::Sample(s) => out.push(s),
                Pulled::Changed => {}
                Pulled::NeedInput => return Ok(()),
            }
        }
        Err(VideoError::os(
            0,
            format!("{} gave more than 64 outputs for one input", self.name),
        ))
    }

    fn pull_one(
        &mut self,
        on_change: &mut dyn FnMut(&IMFTransform) -> Result<(), VideoError>,
    ) -> Result<Pulled, VideoError> {
        let provides = self.out_info.dwFlags
            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                as u32
            != 0;
        let sample = if provides {
            None
        } else {
            let mf = mfplat()?;
            let size = if self.out_info.cbSize > 0 {
                self.out_info.cbSize
            } else {
                self.out_size_hint
            };
            let s = mf
                .create_sample()
                .map_err(|e| map_error(&e, "MFCreateSample"))?;
            let b = mf
                .create_memory_buffer(size)
                .map_err(|e| map_error(&e, "MFCreateMemoryBuffer"))?;
            // SAFETY: a new sample and buffer.
            unsafe { s.AddBuffer(&b) }.map_err(|e| map_error(&e, "AddBuffer"))?;
            Some(s)
        };
        let mut buf = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: self.out_id,
            pSample: std::mem::ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: std::mem::ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        // SAFETY: a live MFT and one output buffer for its one stream; we
        // own whatever sample and event collection are in it afterwards and
        // release them below.
        let r = unsafe { self.transform.ProcessOutput(0, &mut buf, &mut status) };
        let [b] = buf;
        let got = std::mem::ManuallyDrop::into_inner(b.pSample);
        drop(std::mem::ManuallyDrop::into_inner(b.pEvents));
        match r {
            Ok(()) => got
                .map(Pulled::Sample)
                .ok_or_else(|| VideoError::os(0, format!("{} gave an empty output", self.name))),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Pulled::NeedInput),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                on_change(&self.transform)?;
                self.refresh_output_info()?;
                Ok(Pulled::Changed)
            }
            Err(e) => Err(map_error(&e, "ProcessOutput")),
        }
    }

    fn process_async(
        &mut self,
        input: &IMFSample,
        no_output: NoOutput,
        on_change: &mut dyn FnMut(&IMFTransform) -> Result<(), VideoError>,
    ) -> Result<Vec<IMFSample>, VideoError> {
        let ev = self.events.clone().expect("asynchronous");
        let mut out = Vec::new();
        let start = Instant::now();
        // Wait to be asked for input (outputs of earlier inputs that show
        // up meanwhile belong to them; with one input at a time there are
        // none, but keep them in order).
        while self.credits == 0 {
            match next_event(&ev)? {
                Some(t) => self.on_event(t, &mut out, on_change)?,
                None if start.elapsed() > STUCK => {
                    return Err(VideoError::os(
                        0,
                        format!("{} asked for no input in {STUCK:?}", self.name),
                    ))
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        // SAFETY: a live MFT that asked for input, and a live sample.
        unsafe { self.transform.ProcessInput(self.in_id, input, 0) }
            .map_err(|e| map_error(&e, "ProcessInput"))?;
        self.credits -= 1;
        let had = out.len();
        let mut asked_at: Option<Instant> = None;
        loop {
            match next_event(&ev)? {
                Some(t) => {
                    if t == METransformNeedInput && asked_at.is_none() {
                        asked_at = Some(Instant::now());
                    }
                    self.on_event(t, &mut out, on_change)?;
                    if out.len() > had {
                        return Ok(out);
                    }
                }
                None => {
                    // Wants more input and gave nothing for this one yet.
                    if asked_at.is_some_and(|t| t.elapsed() > ASYNC_OUTPUT_WAIT) {
                        break;
                    }
                    if start.elapsed() > STUCK {
                        return Err(VideoError::os(
                            0,
                            format!("{} gave no output in {STUCK:?}", self.name),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        if no_output == NoOutput::Wait {
            return Ok(out);
        }
        // SAFETY: a command on a live MFT; it answers with its held
        // outputs, then METransformDrainComplete.
        unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) }
            .map_err(|e| map_error(&e, "MFT_MESSAGE_COMMAND_DRAIN"))?;
        let drain_start = Instant::now();
        loop {
            match next_event(&ev)? {
                Some(t) if t == METransformDrainComplete => break,
                Some(t) => self.on_event(t, &mut out, on_change)?,
                None if drain_start.elapsed() > STUCK => {
                    return Err(VideoError::os(
                        0,
                        format!("{} didn't finish draining in {STUCK:?}", self.name),
                    ))
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        // A drained asynchronous MFT starts a new stream.
        // SAFETY: a notification on a live MFT.
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
        }
        .map_err(|e| map_error(&e, "MFT_MESSAGE_NOTIFY_START_OF_STREAM"))?;
        Ok(out)
    }

    fn on_event(
        &mut self,
        t: MF_EVENT_TYPE,
        out: &mut Vec<IMFSample>,
        on_change: &mut dyn FnMut(&IMFTransform) -> Result<(), VideoError>,
    ) -> Result<(), VideoError> {
        if t == METransformNeedInput {
            self.credits += 1;
        } else if t == METransformHaveOutput {
            if let Pulled::Sample(s) = self.pull_one(on_change)? {
                out.push(s);
            }
        }
        Ok(())
    }
}

enum Pulled {
    Sample(IMFSample),
    Changed,
    NeedInput,
}

/// The next queued event's type, without waiting. An event carrying a
/// failure (a hardware error) is an error.
fn next_event(ev: &IMFMediaEventGenerator) -> Result<Option<MF_EVENT_TYPE>, VideoError> {
    // SAFETY: a non-blocking read of a live event queue.
    match unsafe { ev.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
        Ok(e) => {
            // SAFETY: reads of a live event.
            let (t, status) = unsafe { (e.GetType(), e.GetStatus()) };
            let t = MF_EVENT_TYPE(t.map_err(|e| map_error(&e, "GetType"))? as i32);
            if let Ok(st) = status {
                if st.is_err() {
                    return Err(map_error(&windows::core::Error::from(st), "MFT event"));
                }
            }
            Ok(Some(t))
        }
        Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => Ok(None),
        Err(e) => Err(map_error(&e, "GetEvent")),
    }
}

impl Drop for Mft {
    fn drop(&mut self) {
        // SAFETY: end-of-stream notifications, then the activation object
        // shuts the MFT down (asynchronous MFTs need it to stop their
        // threads).
        unsafe {
            if self.streaming {
                let _ = self
                    .transform
                    .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
                let _ = self
                    .transform
                    .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            }
            let _ = self.activate.ShutdownObject();
        }
    }
}

// ───────────────────────────── media types and values ─────────────────────────────

/// `MFSetAttributeSize` / `MFSetAttributeRatio`: two 32-bit halves.
pub(super) fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// The two halves of a packed size or ratio.
pub(super) fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// A video media type of `subtype`.
pub(super) fn video_type(subtype: &GUID) -> Result<IMFMediaType, VideoError> {
    let t = mfplat()?
        .create_media_type()
        .map_err(|e| map_error(&e, "MFCreateMediaType"))?;
    // SAFETY: writes to a new media type.
    unsafe {
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| map_error(&e, "MF_MT_MAJOR_TYPE"))?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)
            .map_err(|e| map_error(&e, "MF_MT_SUBTYPE"))?;
    }
    Ok(t)
}

/// The first type the MFT offers on its input (`input`) or output with
/// this subtype.
pub(super) fn available(
    mft: &IMFTransform,
    stream: u32,
    input: bool,
    subtype: &GUID,
) -> Option<IMFMediaType> {
    for i in 0..64 {
        // SAFETY: queries on a live MFT; an error ends the list.
        let t = unsafe {
            if input {
                mft.GetInputAvailableType(stream, i)
            } else {
                mft.GetOutputAvailableType(stream, i)
            }
        }
        .ok()?;
        // SAFETY: a read of a live media type.
        if unsafe { t.GetGUID(&MF_MT_SUBTYPE) }.ok().as_ref() == Some(subtype) {
            return Some(t);
        }
    }
    None
}

/// A `VARIANT` holding a `u32` (`VT_UI4`).
pub(super) fn var_u32(v: u32) -> windows::Win32::System::Variant::VARIANT {
    use windows::Win32::System::Variant::{VARIANT, VT_UI4};
    let mut var = VARIANT::default();
    // SAFETY: writing the tag and the matching member of a zeroed VARIANT.
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    var
}

/// A `VARIANT` holding a `BOOL` (`VT_BOOL`).
pub(super) fn var_bool(v: bool) -> windows::Win32::System::Variant::VARIANT {
    use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
    use windows::Win32::System::Variant::{VARIANT, VT_BOOL};
    let mut var = VARIANT::default();
    // SAFETY: writing the tag and the matching member of a zeroed VARIANT.
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if v { VARIANT_TRUE } else { VARIANT_FALSE };
    }
    var
}

/// Set one ICodecAPI value; `false` when the MFT doesn't take it.
pub(super) fn set_codec_value(
    api: &ICodecAPI,
    key: &GUID,
    value: &windows::Win32::System::Variant::VARIANT,
) -> bool {
    // SAFETY: a live ICodecAPI, a codec property GUID and a plain VARIANT
    // (no owned members, so nothing to clear).
    unsafe { api.SetValue(key, value) }.is_ok()
}

/// Copy `bytes` into a new sample, stamped `time_100ns` long `dur_100ns`.
pub(super) fn sample_from(
    bytes: &[u8],
    time_100ns: i64,
    dur_100ns: i64,
) -> Result<IMFSample, VideoError> {
    let mf = mfplat()?;
    let len =
        u32::try_from(bytes.len().max(1)).map_err(|_| VideoError::os(0, "a sample over 4 GB"))?;
    let buf = mf
        .create_memory_buffer(len)
        .map_err(|e| map_error(&e, "MFCreateMemoryBuffer"))?;
    // SAFETY: the buffer is locked while we write at most its length into
    // it, then unlocked; the sample takes a reference to it.
    unsafe {
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut max = 0u32;
        buf.Lock(&mut p, Some(&mut max), None)
            .map_err(|e| map_error(&e, "Lock"))?;
        if p.is_null() || (max as usize) < bytes.len() {
            let _ = buf.Unlock();
            return Err(VideoError::os(0, "a media buffer smaller than asked for"));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        buf.Unlock().map_err(|e| map_error(&e, "Unlock"))?;
        buf.SetCurrentLength(bytes.len() as u32)
            .map_err(|e| map_error(&e, "SetCurrentLength"))?;
        let s = mf
            .create_sample()
            .map_err(|e| map_error(&e, "MFCreateSample"))?;
        s.AddBuffer(&buf).map_err(|e| map_error(&e, "AddBuffer"))?;
        s.SetSampleTime(time_100ns)
            .map_err(|e| map_error(&e, "SetSampleTime"))?;
        s.SetSampleDuration(dur_100ns)
            .map_err(|e| map_error(&e, "SetSampleDuration"))?;
        Ok(s)
    }
}

/// The bytes of a sample, all its buffers in order.
pub(super) fn sample_bytes(s: &IMFSample) -> Result<Vec<u8>, VideoError> {
    // SAFETY: a contiguous view of a live sample, locked while we copy its
    // current length, then unlocked.
    unsafe {
        let buf = s
            .ConvertToContiguousBuffer()
            .map_err(|e| map_error(&e, "ConvertToContiguousBuffer"))?;
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut p, None, Some(&mut len))
            .map_err(|e| map_error(&e, "Lock"))?;
        let out = if p.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(p, len as usize).to_vec()
        };
        buf.Unlock().map_err(|e| map_error(&e, "Unlock"))?;
        Ok(out)
    }
}

/// Unsupported, naming why there is no H.264 here.
pub(super) fn none_found(role: Role, pref: ImplPreference) -> VideoError {
    let what = match role {
        Role::Encoder => "encoder",
        Role::Decoder => "decoder",
    };
    match pref {
        ImplPreference::HardwareOnly => {
            unsupported(format!("no hardware H.264 {what} MFT on this machine"))
        }
        ImplPreference::SoftwareOnly => unsupported(format!(
            "no Microsoft software H.264 {what} MFT (an N or KN edition of Windows without \
             the Media Feature Pack?)"
        )),
        _ => unsupported(format!(
            "no H.264 {what} MFT (an N or KN edition of Windows without the Media Feature \
             Pack?)"
        )),
    }
}
