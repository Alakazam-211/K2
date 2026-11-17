//! The keyed watch on the docs socket (kit v0.16, `client/WIRE.md` "The
//! keyed watch"): what a person watching a live keyed doc that isn't a table
//! (a deck, a board) gets instead of the welcome's previews and the bare
//! `change` frames.
//!
//! - **The gate.** A socket's welcome lists `"keyed_watch"`
//!   ([`KEYED_WATCH_FEATURE`]) only for a doc whose session says
//!   [`Session::keyed_watchable`] (never a text doc, never a table): see
//!   [`features`].
//! - **`watch`** (the client's frame): the watch is opened once per
//!   connection ([`Session::watch_keyed`], needs read), then a fresh
//!   snapshot goes out: `keyed_snapshot`, then its `keyed_page`s
//!   ([`Session::watch_snapshot`]). A second `watch` (the client saw a gap)
//!   is a fresh snapshot on the same watch.
//! - **Then every change** as `ops_change`, numbered by `seq`
//!   ([`KeyedWatch::try_frames`]), and a fresh snapshot whenever
//!   [`KeyedWatch::needs_snapshot`] says so (the watch fell behind, or a
//!   change's frame was over the cap).
//! - **A refused snapshot** (`MPC-E401`: one entry over the frame cap) goes
//!   out as `watch_refused {error}`, unnumbered; no fresh snapshot is tried
//!   again until the client asks (`watch`).
//! - **`report`** (the last `seq` the client applied) is taken and ignored,
//!   as the kit allows on this engine.
//!
//! Every watch frame is the kit's [`WatchFrame::to_json`], byte for byte.
//! A client that never sends `watch` (an older desktop build) gets the old
//! deck path untouched: the welcome's `previews`, the `change` frames and
//! `preview_ready`. The wire stays version 3.

use k2_mpc_base::errors::{ErrorReport, Value};
use k2_mpc_base::session::{ConnId, KeyedWatch, Session, SessionError, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE};

use super::live_wire as wire;

/// The feature names a socket on this doc lists in `welcome.features`:
/// `keyed_watch` for a keyed doc that isn't a table, none otherwise.
pub fn features(session: &Session) -> Vec<&'static str> {
    if session.keyed_watchable() {
        vec![KEYED_WATCH_FEATURE]
    } else {
        Vec::new()
    }
}

/// The view this connection looks at, by address (kit v0.18.0, WIRE.md
/// "Feature names"): the client frame `view_address`.
pub const VIEW_ADDRESS_FEATURE: &str = "view_address";

/// This connection's playhead (kit v0.18.0): the client frame
/// `time_position`.
pub const TIME_POSITION_FEATURE: &str = "time_position";

/// The presence families a keyed doc's socket takes besides the keyed
/// watch, for readers and editors alike (the kit refuses only a watcher, and
/// K2 joins none): [`VIEW_ADDRESS_FEATURE`] and [`TIME_POSITION_FEATURE`].
/// Follow uses the view address (a scene's `@cam:<id>`, a deck's slide) and
/// the timeline's playheads the time position.
pub const PRESENCE_FEATURES: [&str; 2] = [VIEW_ADDRESS_FEATURE, TIME_POSITION_FEATURE];

/// What a text doc's socket lists in `welcome.features`: the view address
/// (the editor's Follow by heading), beside its carets and lines; no keyed
/// watch, no playhead.
pub const TEXT_SOCKET_FEATURES: [&str; 1] = [VIEW_ADDRESS_FEATURE];

/// Claims on elements (WIRE.md "Claim frames"): the client frames `claim`
/// and `release`, answered `claimed` / `released` (or `presence_refused`).
pub const CLAIMS_FEATURE: &str = "claims";

/// Selection by element, pointer and viewport (WIRE.md "Presence frames"):
/// the client frames `select`, `pointer` and `viewport`. Read right only.
pub const PRESENCE_FEATURE: &str = "presence";

/// What a scene's socket lists besides the keyed watch: the view address
/// (Follow by `@cam:N` or `@obj:N`), presence (a person's selected objects,
/// pointer and viewport) and, when people may change the scene, claims on
/// objects and collections ("I'm moving the lamp"). A scene has no time,
/// so no `time_position`.
pub const SCENE_FEATURES: [&str; 3] = [VIEW_ADDRESS_FEATURE, PRESENCE_FEATURE, CLAIMS_FEATURE];

/// Everything a keyed doc's socket lists in `welcome.features`: the keyed
/// watch when the doc has it ([`features`]), then [`PRESENCE_FEATURES`].
pub fn keyed_socket_features(session: &Session) -> Vec<&'static str> {
    let mut f = features(session);
    f.extend(PRESENCE_FEATURES);
    f
}

/// A live scene's `welcome.features`: the keyed watch when the doc has it,
/// then [`SCENE_FEATURES`], without `claims` while the scene is read-only
/// (`claims` false: a claim needs the edit right, which no one has on a
/// `.blend` for now, `blend_read_only`).
pub fn scene_socket_features(session: &Session, claims: bool) -> Vec<&'static str> {
    let mut f = features(session);
    f.extend(SCENE_FEATURES.into_iter().filter(|x| claims || *x != CLAIMS_FEATURE));
    f
}

/// `welcome.features` as the welcome carries it.
pub fn features_value(features: &[&str]) -> Value {
    Value::List(features.iter().map(|f| Value::from(*f)).collect())
}

/// `watch_refused {error}`: the host can't serve the watch (unnumbered).
pub fn refused_frame(e: &ErrorReport) -> Value {
    Value::obj([("type", Value::from("watch_refused")), ("error", e.to_value())])
}

/// Why a client's watch frame wasn't taken on this socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotTaken(pub String);

/// One socket's keyed watch: opened on the client's first `watch`.
pub struct SocketWatch {
    opts: WatchOptions,
    watch: Option<KeyedWatch>,
    /// The last snapshot was refused: nothing more goes out until the
    /// client asks again.
    refused: bool,
}

impl SocketWatch {
    /// For a socket whose welcome listed `features`.
    pub fn new(features: &[&str]) -> Self {
        Self::with_options(WatchOptions::new(features).frame_max(wire::FRAME_MAX))
    }

    /// With the kit's options as they are (a test's small caps).
    pub fn with_options(opts: WatchOptions) -> Self {
        SocketWatch { opts, watch: None, refused: false }
    }

    /// The welcome listed the feature.
    pub fn listed(&self) -> bool {
        self.opts.listed()
    }

    /// The client asked: the watch is open, so its frames replace the bare
    /// `change` frames on this socket.
    pub fn open(&self) -> bool {
        self.watch.is_some()
    }

    /// The client's `watch`: open the watch if there is none, then a fresh
    /// snapshot. Every refusal is answered (`watch_refused`): the client is
    /// waiting for a snapshot head.
    pub fn on_watch(&mut self, session: &Session, conn: ConnId) -> Result<Vec<String>, NotTaken> {
        if !self.listed() {
            return Err(NotTaken("`watch` isn't taken here: this doc has no keyed watch (not listed in welcome.features)".into()));
        }
        if self.watch.is_none() {
            match session.watch_keyed(conn, &self.opts) {
                Ok(Some(w)) => self.watch = Some(w),
                Ok(None) => return Err(NotTaken("`watch` isn't taken here: this doc can't be watched by key".into())),
                Err(e) => {
                    self.refused = true;
                    return Ok(vec![refused_frame(&e.report()).to_json()]);
                }
            }
        }
        Ok(self.snapshot(session, true))
    }

    /// The client's `report`: taken (the kit's engine needs none).
    pub fn on_report(&self) -> Result<(), NotTaken> {
        if self.listed() {
            Ok(())
        } else {
            Err(NotTaken("`report` isn't taken here: this doc has no keyed watch".into()))
        }
    }

    /// The change frames queued since the last call, and a fresh snapshot
    /// when the watch needs one. Nothing before the client's first `watch`.
    pub fn pump(&mut self, session: &Session) -> Vec<String> {
        let Some(w) = self.watch.as_mut() else { return Vec::new() };
        let mut out: Vec<String> = w.try_frames().iter().map(WatchFrame::to_json).collect();
        if w.needs_snapshot() && !self.refused {
            out.extend(self.snapshot(session, false));
        }
        out
    }

    /// A fresh snapshot's frames. Refused `MPC-E401`: `watch_refused`.
    /// Anything else (the connection left, rights lost) goes out only when
    /// the client asked; the socket's own close says the rest.
    fn snapshot(&mut self, session: &Session, asked: bool) -> Vec<String> {
        let w = self.watch.as_mut().expect("an open watch");
        match session.watch_snapshot(w) {
            Ok(frames) => {
                self.refused = false;
                frames.iter().map(WatchFrame::to_json).collect()
            }
            Err(e) => {
                self.refused = true;
                if asked || matches!(e, SessionError::Declined(_)) {
                    vec![refused_frame(&e.report()).to_json()]
                } else {
                    k2_core::log_debug!("[docs/live] no fresh watch snapshot: {}", e.report().plain());
                    Vec::new()
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "live_watch_tests.rs"]
mod tests;
