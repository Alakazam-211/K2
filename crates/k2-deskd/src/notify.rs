//! The in-use banner on Linux (S6): one persistent desktop notification
//! through `org.freedesktop.Notifications`, with a **Disconnect** action.
//! Ordinary UX (Rosson 2026-11-10). A notification server without action
//! support shows the text with no button; dismissing it is allowed.
//!
//! zbus on its blocking API (Steward R27).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;
use zbus::MatchRule;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
pub const ACTION: &str = "disconnect";
pub const SUMMARY: &str = "Remote desktop";

struct State {
    conn: Option<Connection>,
    /// The notification on screen (0 = none).
    id: u32,
    listening: bool,
}

pub struct NotifyBanner {
    state: Mutex<State>,
    on_disconnect: Arc<dyn Fn() + Send + Sync>,
}

/// Is this signal our notification's Disconnect? (Pure, for tests.)
pub fn is_our_disconnect(member: &str, id: u32, action: &str, current: u32) -> bool {
    member == "ActionInvoked" && current != 0 && id == current && action == ACTION
}

impl NotifyBanner {
    pub fn new(on_disconnect: Arc<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new(NotifyBanner { state: Mutex::new(State { conn: None, id: 0, listening: false }), on_disconnect })
    }

    fn conn(st: &mut State) -> Result<Connection, String> {
        if let Some(c) = &st.conn {
            return Ok(c.clone());
        }
        let c = Connection::session().map_err(|e| format!("session bus: {e}"))?;
        st.conn = Some(c.clone());
        Ok(c)
    }

    pub fn set(self: &Arc<Self>, text: Option<String>) {
        if let Err(e) = self.try_set(text) {
            eprintln!("k2-deskd: banner: {e}");
        }
    }

    fn try_set(self: &Arc<Self>, text: Option<String>) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        match text {
            Some(body) => {
                let conn = Self::conn(&mut st)?;
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
                    let conn = Self::conn(&mut st)?;
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

    #[test]
    fn only_our_notifications_disconnect_counts() {
        assert!(is_our_disconnect("ActionInvoked", 7, "disconnect", 7));
        assert!(!is_our_disconnect("ActionInvoked", 8, "disconnect", 7));
        assert!(!is_our_disconnect("ActionInvoked", 7, "default", 7));
        assert!(!is_our_disconnect("NotificationClosed", 7, "disconnect", 7));
        assert!(!is_our_disconnect("ActionInvoked", 0, "disconnect", 0));
    }
}
