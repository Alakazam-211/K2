//! The in-use banner on Linux (S6): one persistent desktop notification
//! through `org.freedesktop.Notifications`, with a **Disconnect** action.
//! Ordinary UX (Rosson 2026-11-10). A notification server without action
//! support shows the text with no button; dismissing it is allowed.
//!
//! zbus on its blocking API (Steward R27).
//!
//! **It never fails or holds up the link** (Linux virtual desktop LD-7): a
//! virtual desktop has no notification server, and often no session bus.
//! The D-Bus calls run on the banner's own thread; [`NotifyBanner::set`]
//! waits for them at most [`SET_WAIT`] (so a close on shutdown still goes
//! out), and a missing bus or server is one line on stderr. The viewer
//! list in the tab and the access log carry who is watching instead.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;
use zbus::MatchRule;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
pub const ACTION: &str = "disconnect";
pub const SUMMARY: &str = "Remote desktop";
/// The longest [`NotifyBanner::set`] waits for the D-Bus calls.
pub const SET_WAIT: Duration = Duration::from_secs(1);

struct State {
    conn: Option<Connection>,
    /// The notification on screen (0 = none).
    id: u32,
    listening: bool,
}

/// The banner thread's queue: the text, and where to say it's done.
type BannerQueue = mpsc::Sender<(Option<String>, mpsc::Sender<()>)>;

pub struct NotifyBanner {
    state: Mutex<State>,
    on_disconnect: Arc<dyn Fn() + Send + Sync>,
    /// The session bus to use; `None`: the session's own
    /// (`DBUS_SESSION_BUS_ADDRESS`).
    address: Option<String>,
    worker: Mutex<Option<BannerQueue>>,
}

/// Is this signal our notification's Disconnect? (Pure, for tests.)
pub fn is_our_disconnect(member: &str, id: u32, action: &str, current: u32) -> bool {
    member == "ActionInvoked" && current != 0 && id == current && action == ACTION
}

impl NotifyBanner {
    pub fn new(on_disconnect: Arc<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Self::with_address(on_disconnect, None)
    }

    /// A banner on the bus at `address` (tests); `None`: the session's.
    pub fn with_address(on_disconnect: Arc<dyn Fn() + Send + Sync>, address: Option<String>) -> Arc<Self> {
        Arc::new(NotifyBanner { state: Mutex::new(State { conn: None, id: 0, listening: false }), on_disconnect, address, worker: Mutex::new(None) })
    }

    fn conn(&self, st: &mut State) -> Result<Connection, String> {
        if let Some(c) = &st.conn {
            return Ok(c.clone());
        }
        let c = match &self.address {
            Some(a) => zbus::blocking::connection::Builder::address(a.as_str()).and_then(|b| b.build()),
            None => Connection::session(),
        }
        .map_err(|e| format!("session bus: {e}"))?;
        st.conn = Some(c.clone());
        Ok(c)
    }

    /// Shows (`Some`) or hides (`None`) the banner. Never fails, and
    /// returns within [`SET_WAIT`] whatever the bus does.
    pub fn set(self: &Arc<Self>, text: Option<String>) {
        let (done_tx, done_rx) = mpsc::channel();
        let queued = {
            let mut w = self.worker.lock().unwrap();
            if w.is_none() {
                *w = self.spawn_worker();
            }
            w.as_ref().is_some_and(|tx| tx.send((text, done_tx)).is_ok())
        };
        if queued {
            let _ = done_rx.recv_timeout(SET_WAIT);
        }
    }

    fn spawn_worker(self: &Arc<Self>) -> Option<BannerQueue> {
        let (tx, rx) = mpsc::channel::<(Option<String>, mpsc::Sender<()>)>();
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("k2-deskd-banner".into()).spawn(move || {
            while let Ok((text, done)) = rx.recv() {
                if let Err(e) = me.try_set(text) {
                    eprintln!("k2-deskd: banner: {e}");
                }
                let _ = done.send(());
            }
        });
        match spawned {
            Ok(_) => Some(tx),
            Err(e) => {
                eprintln!("k2-deskd: banner thread: {e}");
                None
            }
        }
    }

    fn try_set(self: &Arc<Self>, text: Option<String>) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        match text {
            Some(body) => {
                let conn = self.conn(&mut st)?;
                if !st.listening {
                    self.listen(conn.clone())?;
                    st.listening = true;
                }
                let mut hints: HashMap<&str, Value> = HashMap::new();
                hints.insert("resident", Value::Bool(true));
                hints.insert("urgency", Value::U8(2));
                let actions = vec![ACTION, "Disconnect"];
                let reply = conn
                    .call_method(Some(DEST), PATH, Some(DEST), "Notify", &("K2", st.id, "", SUMMARY, body.as_str(), actions, hints, 0i32))
                    .map_err(|e| format!("Notify: {e}"))?;
                st.id = reply.body().deserialize::<u32>().map_err(|e| format!("Notify reply: {e}"))?;
            }
            None => {
                if st.id != 0 {
                    let id = std::mem::take(&mut st.id);
                    let conn = self.conn(&mut st)?;
                    let _ = conn.call_method(Some(DEST), PATH, Some(DEST), "CloseNotification", &(id,));
                }
            }
        }
        Ok(())
    }

    fn listen(self: &Arc<Self>, conn: Connection) -> Result<(), String> {
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(DEST)
            .map_err(|e| e.to_string())?
            .build();
        let iter = MessageIterator::for_match_rule(rule, &conn, Some(16)).map_err(|e| format!("match rule: {e}"))?;
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("k2-deskd-notify".into())
            .spawn(move || {
                for m in iter {
                    let Ok(m) = m else { continue };
                    let member = m.header().member().map(|n| n.as_str().to_string()).unwrap_or_default();
                    match member.as_str() {
                        "ActionInvoked" => {
                            let Ok((id, action)) = m.body().deserialize::<(u32, String)>() else { continue };
                            let current = me.state.lock().unwrap().id;
                            if is_our_disconnect(&member, id, &action, current) {
                                (me.on_disconnect)();
                            }
                        }
                        "NotificationClosed" => {
                            if let Ok((id, _reason)) = m.body().deserialize::<(u32, u32)>() {
                                let mut st = me.state.lock().unwrap();
                                if st.id == id {
                                    st.id = 0;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LD-7: no session bus, or a bus with no notification server: `set`
    /// comes back at once (well inside the 5 s the daemon allows a call),
    /// every time, and nothing panics.
    #[test]
    fn a_missing_bus_or_server_never_holds_up_the_caller() {
        let b = NotifyBanner::with_address(Arc::new(|| {}), Some("unix:path=/nonexistent/k2-deskd-test-bus".into()));
        for text in [Some("Ann is watching this screen".to_string()), Some("Ann is controlling this screen".to_string()), None] {
            let t0 = std::time::Instant::now();
            b.set(text);
            assert!(t0.elapsed() <= SET_WAIT + Duration::from_millis(500), "{:?}", t0.elapsed());
        }
        assert_eq!(b.state.lock().unwrap().id, 0, "nothing shown");
    }

    #[test]
    fn only_our_notifications_disconnect_counts() {
        assert!(is_our_disconnect("ActionInvoked", 7, "disconnect", 7));
        assert!(!is_our_disconnect("ActionInvoked", 8, "disconnect", 7));
        assert!(!is_our_disconnect("ActionInvoked", 7, "default", 7));
        assert!(!is_our_disconnect("NotificationClosed", 7, "disconnect", 7));
        assert!(!is_our_disconnect("ActionInvoked", 0, "disconnect", 0));
    }
}
