//! The xdg-desktop-portal ScreenCast interface over D-Bus (zbus, pure
//! Rust): a session, the system's own picker, and the PipeWire remote whose
//! stream carries the chosen display or window.

use crate::error::VideoError;
use crate::types::PickOptions;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.ScreenCast";

/// Source types (bit flags).
pub(crate) const SOURCE_MONITOR: u32 = 1;
pub(crate) const SOURCE_WINDOW: u32 = 2;
/// Cursor modes (bit flags).
pub(crate) const CURSOR_HIDDEN: u32 = 1;
pub(crate) const CURSOR_EMBEDDED: u32 = 2;

/// What the portal offers on this session bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Offer {
    pub(crate) source_types: u32,
    pub(crate) cursor_modes: u32,
}

/// One stream the person shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stream {
    pub(crate) node_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) window: bool,
}

/// A started ScreenCast session: the connection (the session ends when it
/// closes), the session handle, the stream and the PipeWire remote.
pub(crate) struct Session {
    conn: Connection,
    handle: OwnedObjectPath,
    pub(crate) stream: Stream,
    /// Taken by the capture that opens the stream.
    pub(crate) remote: Option<OwnedFd>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("stream", &self.stream).finish_non_exhaustive()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(p) = Proxy::new(&self.conn, DEST, self.handle.as_ref(), "org.freedesktop.portal.Session") {
            let _: Result<(), _> = p.call("Close", &());
        }
    }
}

fn dbus_err(e: zbus::Error, what: &str) -> VideoError {
    VideoError::os(0, format!("{what}: {e}"))
}

/// The portal's object path for a request with `token`, from the bus's
/// unique name (":1.42" becomes "1_42").
pub(crate) fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

/// A token for one request or session, unique in this process.
fn token(kind: &str) -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!("kessel_video_{kind}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

/// A Response code to a result: 0 success, 1 the person cancelled, other
/// ended for some other reason.
pub(crate) fn response_result(code: u32, what: &str) -> Result<(), VideoError> {
    match code {
        0 => Ok(()),
        1 => Err(VideoError::NotFound),
        n => Err(VideoError::os(n as i64, format!("the screen-cast portal ended {what}"))),
    }
}

/// Is the portal there, and what does it offer? (Cheap: one connection, two
/// property reads.)
pub(crate) fn offer() -> Result<Offer, VideoError> {
    let conn = Connection::session().map_err(|e| dbus_err(e, "the session bus"))?;
    let p = Proxy::new(&conn, DEST, PATH, IFACE).map_err(|e| dbus_err(e, "the portal"))?;
    let source_types: u32 = p.get_property("AvailableSourceTypes").map_err(|e| dbus_err(e, "AvailableSourceTypes"))?;
    let cursor_modes: u32 = p.get_property("AvailableCursorModes").unwrap_or(CURSOR_HIDDEN);
    Ok(Offer { source_types, cursor_modes })
}

/// Run the portal's picker and start the stream: CreateSession,
/// SelectSources, Start, OpenPipeWireRemote.
pub(crate) fn pick(opts: PickOptions) -> Result<Session, VideoError> {
    let offer = offer()?;
    let conn = Connection::session().map_err(|e| dbus_err(e, "the session bus"))?;
    let portal = Proxy::new(&conn, DEST, PATH, IFACE).map_err(|e| dbus_err(e, "the portal"))?;

    // CreateSession.
    let t = token("req");
    let session_token = token("session");
    let mut o: HashMap<&str, Value<'_>> = HashMap::new();
    o.insert("handle_token", Value::from(t.as_str()));
    o.insert("session_handle_token", Value::from(session_token.as_str()));
    let results = call_request(&conn, &portal, "CreateSession", &(o,), &t)?;
    let handle: String = results
        .get("session_handle")
        .and_then(|v| String::try_from(v.clone()).ok())
        .ok_or_else(|| VideoError::os(0, "CreateSession gave no session handle"))?;
    let handle = OwnedObjectPath::try_from(handle).map_err(|e| VideoError::os(0, format!("session handle: {e}")))?;

    // SelectSources: what the picker offers.
    let want = (if opts.displays || !opts.windows { SOURCE_MONITOR } else { 0 }) | (if opts.windows || !opts.displays { SOURCE_WINDOW } else { 0 });
    let types = want & offer.source_types;
    let cursor = if offer.cursor_modes & CURSOR_EMBEDDED != 0 { CURSOR_EMBEDDED } else { CURSOR_HIDDEN };
    let t = token("req");
    let mut o: HashMap<&str, Value<'_>> = HashMap::new();
    o.insert("handle_token", Value::from(t.as_str()));
    o.insert("types", Value::from(if types == 0 { offer.source_types } else { types }));
    o.insert("multiple", Value::from(false));
    o.insert("cursor_mode", Value::from(cursor));
    call_request(&conn, &portal, "SelectSources", &(handle.as_ref(), o), &t)?;

    // Start: the portal shows its picker now.
    let t = token("req");
    let mut o: HashMap<&str, Value<'_>> = HashMap::new();
    o.insert("handle_token", Value::from(t.as_str()));
    let results = call_request(&conn, &portal, "Start", &(handle.as_ref(), "", o), &t)?;
    let streams = results.get("streams").ok_or_else(|| VideoError::os(0, "Start gave no streams"))?;
    let stream = parse_streams(streams).ok_or_else(|| VideoError::os(0, "Start's streams were unreadable"))?;

    // The PipeWire remote that carries the stream.
    let empty: HashMap<&str, Value<'_>> = HashMap::new();
    let fd: zbus::zvariant::OwnedFd = portal
        .call("OpenPipeWireRemote", &(handle.as_ref(), empty))
        .map_err(|e| dbus_err(e, "OpenPipeWireRemote"))?;
    // Our own copy: the zvariant fd closes its own on drop.
    // SAFETY: dup of a valid descriptor; we own the result.
    let dup = unsafe { libc::dup(fd.as_raw_fd()) };
    if dup < 0 {
        return Err(VideoError::os(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as i64, "dup the PipeWire fd"));
    }
    // SAFETY: a fresh descriptor from dup, owned from here on.
    let remote = unsafe { OwnedFd::from_raw_fd(dup) };
    Ok(Session { conn, handle, stream, remote: Some(remote) })
}

/// Call a request-style method and wait for its Response signal (as long as
/// the person keeps the dialog open).
fn call_request(conn: &Connection, portal: &Proxy<'_>, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType), handle_token: &str) -> Result<HashMap<String, OwnedValue>, VideoError> {
    let unique = conn.unique_name().ok_or_else(|| VideoError::os(0, "no unique bus name"))?.to_string();
    let path = request_path(&unique, handle_token);
    // Listen before calling, so a fast reply isn't missed.
    let req = Proxy::new(conn, DEST, path.as_str(), "org.freedesktop.portal.Request").map_err(|e| dbus_err(e, "the request"))?;
    let mut responses = req.receive_signal("Response").map_err(|e| dbus_err(e, "Response"))?;
    let reply_path: OwnedObjectPath = portal.call(method, body).map_err(|e| dbus_err(e, method))?;
    if reply_path.as_str() != path {
        // An old portal that ignores handle_token: listen on its path.
        let req = Proxy::new(conn, DEST, reply_path.as_ref(), "org.freedesktop.portal.Request").map_err(|e| dbus_err(e, "the request"))?;
        responses = req.receive_signal("Response").map_err(|e| dbus_err(e, "Response"))?;
    }
    let msg = responses.next().ok_or_else(|| VideoError::os(0, format!("{method}: the portal closed the request")))?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().map_err(|e| dbus_err(e, "Response body"))?;
    response_result(code, method)?;
    Ok(results)
}

/// The first stream from Start's `streams`: `a(ua{sv})`.
fn parse_streams(v: &OwnedValue) -> Option<Stream> {
    let list: Vec<(u32, HashMap<String, OwnedValue>)> = v.try_clone().ok()?.try_into().ok()?;
    let (node_id, props) = list.into_iter().next()?;
    let (width, height) = props
        .get("size")
        .and_then(|s| <(i32, i32)>::try_from(s.try_clone().ok()?).ok())
        .map_or((0, 0), |(w, h)| (w.max(0) as u32, h.max(0) as u32));
    let window = props
        .get("source_type")
        .and_then(|s| u32::try_from(s.try_clone().ok()?).ok())
        .is_some_and(|t| t == SOURCE_WINDOW);
    Some(Stream { node_id, width, height, window })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_follow_the_sender_name() {
        assert_eq!(request_path(":1.42", "t1"), "/org/freedesktop/portal/desktop/request/1_42/t1");
        assert_eq!(request_path(":1.1234", "kessel_video_req_7_0"), "/org/freedesktop/portal/desktop/request/1_1234/kessel_video_req_7_0");
        assert_ne!(token("req"), token("req"), "tokens are unique");
    }

    #[test]
    fn response_codes() {
        assert_eq!(response_result(0, "Start"), Ok(()));
        assert_eq!(response_result(1, "Start"), Err(VideoError::NotFound), "cancelled is not-found, as on macOS");
        assert!(matches!(response_result(2, "Start"), Err(VideoError::Os { code: 2, .. })));
    }

    #[test]
    fn start_streams_parse() {
        let mut props: HashMap<String, OwnedValue> = HashMap::new();
        props.insert("size".into(), OwnedValue::try_from(Value::from((1920i32, 1080i32))).expect("size"));
        props.insert("source_type".into(), OwnedValue::from(SOURCE_WINDOW));
        let v = OwnedValue::try_from(Value::from(vec![(57u32, props)])).expect("streams");
        let s = parse_streams(&v).expect("a stream");
        assert_eq!(s, Stream { node_id: 57, width: 1920, height: 1080, window: true });
    }

    /// Without a desktop session there is no portal: a clean error, not a
    /// hang or a panic.
    #[test]
    fn the_offer_answers_without_a_desktop() {
        match offer() {
            Ok(o) => eprintln!("[portal] offers types {:#b}, cursors {:#b}", o.source_types, o.cursor_modes),
            Err(e) => {
                eprintln!("[portal] none here: {e}");
                assert!(matches!(e, VideoError::Os { .. }), "{e:?}");
            }
        }
    }
}
