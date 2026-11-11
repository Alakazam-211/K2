//! The routes' [`Deskd`] and the stream's [`StreamDeskd`] over the real
//! link to k2-deskd ([`DeskdManager`], D3). [`boot`] installs it, and turns
//! deskd on when the switch was left on.
//!
//! **Frames.** deskd hands over one frames pipe per link
//! ([`DeskdManager::take_frames`]); one [`LinkFeed`] reads it for the link's
//! life, and each hub attaches its own [`LinkSource`] to that feed (never a
//! second reader on the pipe). A new link (deskd restarted) brings a new
//! pipe and a new feed.
//!
//! **Protocol 4** (D5): the engine's `layers`, `keyframe` and `rate` go
//! through [`DeskdManager::send_link`], a display switch through
//! [`DeskdManager::select_display`], the clock and codecs from the hello's
//! `link_fields`, and deskd's `layer_failed` / `capture_error` come back as
//! [`StreamEvent`]s. **Not modelled by deskd yet**, sent as raw JSON and
//! answered `bad_request` by today's deskd: `clipboard_get` (reply
//! `{text, reason?}`) and `clipboard_set {text}`; the events `cursor`,
//! `cursor_pos`, `displays_changed` and `locked` are relayed when deskd
//! sends them (today the pointer is drawn into the picture).
//!
//! **A failed layer** goes to the hub through the feed
//! ([`LinkFeed::fail_layer`]): a video layer's viewers move to JPEG; a JPEG
//! layer that fails (a Mac's deskd sends H.264 only) ends its viewers with
//! the reason, and the stream shows it as the "The picture stopped" card. The protocol-4 hello fields (`codecs`, `encoder`,
//! `clock_ns`) are read from deskd's hello when present.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value as J};

use k2_core::desktop::wire::{Display, Encoder, HostOs};
use k2_core::desktop::{store, MachineState, CONSOLE};
use k2_core::sim::stream::Codec;
use k2_core::sim::video::link::{Encoder as LinkEncoder, HelloFields, Request as LinkRequest};
use k2_core::stream::link_source::{DisplaySize, LinkClock, LinkFeed, LinkSource, SendRequest, SizesFn};
use k2_core::stream::EncodedSource;
use k2_deskd_proto::msg::{Display as DeskdDisplay, InputEvent, ReleaseReason};

use super::deskd::{os_backend, Deskd};
use super::deskd_link::{DeskdEvent, DeskdManager, DeskdState, CALL_TIMEOUT};
use super::stream::{StreamDeskd, StreamEvent};

/// How long `enable` waits for deskd to settle before answering.
const ENABLE_WAIT: Duration = Duration::from_secs(3);

/// [`Deskd`] over the manager.
pub struct ManagedDeskd {
    inner: Arc<Link>,
}

/// [`StreamDeskd`] over the manager.
pub struct Link {
    m: Arc<DeskdManager>,
    feed: Mutex<Option<LinkFeed>>,
}

impl ManagedDeskd {
    pub fn new(m: Arc<DeskdManager>) -> Self {
        ManagedDeskd { inner: Arc::new(Link { m, feed: Mutex::new(None) }) }
    }
}

/// The routes' state for deskd's.
pub fn machine_state(s: &DeskdState) -> MachineState {
    match s {
        DeskdState::Ready { .. } => MachineState::Ready,
        DeskdState::NeedsPermission { .. } => MachineState::NeedsPermission,
        DeskdState::NeedsUpdate { .. } => MachineState::NeedsUpdate,
        DeskdState::BlockedInLoginItems => MachineState::BlockedInLoginItems,
        DeskdState::NoSession => MachineState::NoSession,
        DeskdState::NoDisplay => MachineState::NoDisplay,
        DeskdState::Off | DeskdState::Starting | DeskdState::NeedsApp | DeskdState::Failed { .. } => MachineState::Unavailable(s.message()),
    }
}

impl Deskd for ManagedDeskd {
    fn backend(&self) -> &'static str {
        os_backend()
    }
    fn state(&self) -> MachineState {
        machine_state(&self.inner.m.state())
    }
    fn enable(&self) -> Result<(), String> {
        // Whatever it settles on (needs_permission, …) shows as the state.
        self.inner.m.enable(ENABLE_WAIT);
        Ok(())
    }
    fn disable(&self) -> Result<(), String> {
        *self.inner.feed.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.inner.m.disable().map_err(|e| e.to_string())
    }
    fn displays(&self) -> Vec<Display> {
        match self.inner.m.displays() {
            Ok(list) => list.iter().map(super::stream::wire_display).collect(),
            Err(e) => {
                k2_core::log_debug!("[desktop] displays: {e}");
                Vec::new()
            }
        }
    }
    fn stream(&self) -> Option<Arc<dyn StreamDeskd>> {
        Some(self.inner.clone() as Arc<dyn StreamDeskd>)
    }
}

impl Link {
    fn fields(&self) -> Option<HelloFields> {
        self.m.hello().map(|h| h.link_fields())
    }
}

/// The protocol-4 hello's video codecs (`Source.gives`).
pub fn hello_codecs(f: &HelloFields) -> Vec<Codec> {
    f.codecs.iter().map(|c| link_codec(*c)).filter(|c| c.is_video()).collect()
}

/// Does deskd make JPEG? (A Mac's deskd says `codecs: ["h264"]`.)
pub fn hello_jpeg(f: &HelloFields) -> bool {
    f.codecs.iter().any(|c| link_codec(*c) == Codec::Jpeg)
}

fn link_codec(c: k2_core::sim::video::link::Codec) -> Codec {
    use k2_core::sim::video::link::Codec as L;
    match c {
        L::Jpeg => Codec::Jpeg,
        L::H264 => Codec::H264,
        L::Vp8 => Codec::Vp8,
    }
}

/// A deskd event the stream relays.
pub fn stream_event(e: DeskdEvent) -> Option<StreamEvent> {
    match e {
        DeskdEvent::LocalInput { hold } => Some(StreamEvent::LocalInput { hold }),
        DeskdEvent::BannerDisconnect => Some(StreamEvent::BannerDisconnect),
        DeskdEvent::Other(v) => {
            let u = |k: &str| v.get(k).and_then(J::as_u64).map(|n| n.min(u32::MAX as u64) as u32);
            let f = |k: &str| v.get(k).and_then(J::as_f64).map(|n| n as f32);
            match v.get("event").and_then(J::as_str)? {
                "cursor" => Some(StreamEvent::Cursor {
                    id: u("id")?,
                    hot_x: u("hot_x").unwrap_or(0),
                    hot_y: u("hot_y").unwrap_or(0),
                    png: v.get("png").and_then(J::as_str)?.to_string(),
                }),
                "cursor_pos" => Some(StreamEvent::CursorPos { x: f("x")?, y: f("y")?, visible: v.get("visible").and_then(J::as_bool).unwrap_or(true) }),
                "displays_changed" => serde_json::from_value::<Vec<DeskdDisplay>>(v.get("displays")?.clone()).ok().map(StreamEvent::DisplaysChanged),
                "locked" => Some(StreamEvent::Locked(v.get("locked").and_then(J::as_bool).unwrap_or(true))),
                _ => None,
            }
        }
        DeskdEvent::LayerFailed { layer, error, .. } => Some(StreamEvent::LayerFailed { layer, why: error }),
        DeskdEvent::CaptureError { code, error, .. } => Some(StreamEvent::CaptureError { code, why: error }),
        DeskdEvent::State(_) | DeskdEvent::InputError { .. } => None,
    }
}

fn sizes_of(list: &[DeskdDisplay]) -> SizesFn {
    let map: HashMap<u32, DisplaySize> = list
        .iter()
        .map(|d| (d.id, DisplaySize { points: (d.width_pt.round().max(0.0) as u32, d.height_pt.round().max(0.0) as u32), native_px: Some((d.width_px, d.height_px)) }))
        .collect();
    Box::new(move |id| map.get(&id).copied())
}

impl StreamDeskd for Link {
    fn os(&self) -> HostOs {
        match self.m.hello().map(|h| h.os) {
            Some(o) if o == "macos" => HostOs::Macos,
            Some(o) if o == "windows" => HostOs::Windows,
            _ => HostOs::Linux,
        }
    }
    fn encoder(&self) -> Encoder {
        match self.fields().map(|f| f.encoder) {
            Some(LinkEncoder::Hardware) => Encoder::Hardware,
            _ => Encoder::Software,
        }
    }
    fn codecs(&self) -> Vec<Codec> {
        self.fields().map(|f| hello_codecs(&f)).unwrap_or_default()
    }
    fn jpeg(&self) -> bool {
        self.fields().is_some_and(|f| hello_jpeg(&f))
    }
    fn fail_layer(&self, layer: u8, why: &str) -> bool {
        match self.feed.lock().unwrap_or_else(|p| p.into_inner()).as_ref().filter(|f| f.ended().is_none()) {
            Some(f) => {
                f.fail_layer(layer, why);
                true
            }
            None => false,
        }
    }
    fn local_wins(&self) -> bool {
        self.m.hello().is_some_and(|h| h.backend != "wayland")
    }
    fn displays(&self) -> Result<Vec<DeskdDisplay>, String> {
        self.m.displays().map_err(|e| e.to_string())
    }
    fn source(&self, display: u32, displays: &[DeskdDisplay]) -> Result<Box<dyn EncodedSource>, String> {
        let feed = {
            let mut f = self.feed.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(pipe) = self.m.take_frames() {
                *f = Some(LinkFeed::spawn(pipe).map_err(|e| format!("the frames pipe: {e}"))?);
            }
            f.as_ref().filter(|x| x.ended().is_none()).cloned().ok_or_else(|| "K2 Screen isn't sending frames yet".to_string())?
        };
        let (hello, at) = self.m.hello_at().ok_or_else(|| "K2 Screen isn't connected".to_string())?;
        let clock = LinkClock::from_hello(&hello.link_fields(), at);
        let m = self.m.clone();
        let send: SendRequest = Box::new(move |r: &LinkRequest| m.send_link(r));
        Ok(Box::new(LinkSource::attach(&feed, clock, display, sizes_of(displays), send)))
    }
    fn select_display(&self, display: u32) -> Result<(), String> {
        self.m.select_display(display).map(|_| ()).map_err(|e| e.to_string())
    }
    fn inject(&self, ev: InputEvent) -> Result<(), String> {
        self.m.inject(ev).map_err(|e| e.to_string())
    }
    fn release_all(&self, why: ReleaseReason) {
        if let Err(e) = self.m.release_all(why) {
            k2_core::log_debug!("[desktop] release_all: {e}");
        }
    }
    fn clipboard_get(&self) -> Result<(Option<String>, Option<String>), String> {
        let v = self.m.call_json(json!({"op": "clipboard_get"}), CALL_TIMEOUT).map_err(|e| e.to_string())?;
        let s = |k: &str| v.get(k).and_then(J::as_str).map(str::to_string);
        Ok((s("text"), s("reason")))
    }
    fn clipboard_set(&self, text: &str) -> Result<(), String> {
        self.m.call_json(json!({"op": "clipboard_set", "text": text}), CALL_TIMEOUT).map(|_| ()).map_err(|e| e.to_string())
    }
    fn banner(&self, text: Option<String>) {
        if let Err(e) = self.m.banner(text) {
            k2_core::log_debug!("[desktop] banner: {e}");
        }
    }
    fn events(&self) -> std::sync::mpsc::Receiver<StreamEvent> {
        let rx = self.m.subscribe();
        let (tx, out) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("k2-desktop-deskd-events".into()).spawn(move || {
            while let Ok(e) = rx.recv() {
                if let Some(ev) = stream_event(e) {
                    if tx.send(ev).is_err() {
                        return; // the stream's pump stopped
                    }
                }
            }
        });
        out
    }
}

/// Daemon boot: the routes and the stream talk to the real deskd, and a
/// switch left on turns deskd back on.
pub fn boot() {
    super::deskd::install(Arc::new(ManagedDeskd::new(super::deskd_link::manager().clone())));
    match store::is_enabled(CONSOLE) {
        Ok(true) => {
            let _ = std::thread::Builder::new().name("k2-deskd-boot".into()).spawn(|| {
                let st = super::deskd_link::manager().enable(ENABLE_WAIT);
                k2_core::log_debug!("[desktop] the switch is on: K2 Screen is {}", st.name());
                super::routes::emit_state();
            });
        }
        Ok(false) => {}
        Err(e) => k2_core::log_debug!("[desktop] reading the switch at boot: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_codecs_reads_the_video_codecs_only() {
        use k2_core::sim::video::link::Codec as L;
        let f = |codecs: Vec<L>| HelloFields { version: 4, accepts: vec![4], codecs, encoder: LinkEncoder::Hardware, clock_ns: 0 };
        assert_eq!(hello_codecs(&f(vec![L::H264, L::Jpeg, L::Vp8])), vec![Codec::H264, Codec::Vp8]);
        assert!(!hello_jpeg(&f(vec![L::H264])), "a Mac's deskd: H.264 only");
        assert!(hello_jpeg(&f(vec![L::Vp8, L::Jpeg])));
    }

    #[test]
    fn deskd_layer_and_capture_failures_become_stream_events() {
        let ev = stream_event(DeskdEvent::LayerFailed { layer: 2, code: "unsupported".into(), error: "no encoder".into() });
        assert_eq!(ev, Some(StreamEvent::LayerFailed { layer: 2, why: "no encoder".into() }));
        let ev = stream_event(DeskdEvent::CaptureError { display: 1, code: "no_screen_permission".into(), error: "no".into() });
        assert_eq!(ev, Some(StreamEvent::CaptureError { code: "no_screen_permission".into(), why: "no".into() }));
    }

    #[test]
    fn deskd_events_become_stream_events() {
        let ev = |v: J| stream_event(DeskdEvent::Other(v));
        assert_eq!(ev(json!({"event": "cursor", "id": 3, "hot_x": 1, "hot_y": 2, "png": "iVBO"})), Some(StreamEvent::Cursor { id: 3, hot_x: 1, hot_y: 2, png: "iVBO".into() }));
        assert_eq!(ev(json!({"event": "cursor_pos", "x": 4.5, "y": 6, "visible": false})), Some(StreamEvent::CursorPos { x: 4.5, y: 6.0, visible: false }));
        assert_eq!(ev(json!({"event": "locked", "locked": true})), Some(StreamEvent::Locked(true)));
        assert_eq!(ev(json!({"event": "cursor", "png": "x"})), None, "a cursor needs its id");
        assert_eq!(stream_event(DeskdEvent::LocalInput { hold: true }), Some(StreamEvent::LocalInput { hold: true }));
        assert_eq!(stream_event(DeskdEvent::BannerDisconnect), Some(StreamEvent::BannerDisconnect));
        assert_eq!(machine_state(&DeskdState::Ready { backend: "macos".into() }), MachineState::Ready);
        assert!(matches!(machine_state(&DeskdState::NeedsApp), MachineState::Unavailable(_)));
    }
}
