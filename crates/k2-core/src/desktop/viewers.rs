//! The live viewer registry: who is watching or driving a desktop, and the
//! handle that closes each one.
//!
//! The stream (D6) registers a viewer after its connect line is written
//! and unregisters it when the socket ends. `disable` closes every viewer
//! (4410 `desktop_off`), `revoke` closes that login's viewers (4401
//! `grant_gone`), `kick` closes one viewer or one login's (4403 `kicked`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use super::wire::Mode;

/// Closes one viewer's socket with a WebSocket close code and reason.
pub trait ViewerHandle: Send + Sync {
    fn close(&self, code: u16, reason: &str);
}

/// One viewer (the owner's `viewers` view).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerInfo {
    pub id: String,
    #[serde(skip)]
    pub desktop: String,
    /// The login's name.
    pub user: String,
    /// The login's stable id (revoke closes by it).
    #[serde(skip)]
    pub user_id: String,
    pub role: String,
    pub mode: Mode,
    pub ingress: String,
    pub ip: String,
    pub device: String,
    pub display: String,
    /// Unix ms.
    pub since: i64,
}

struct Entry {
    info: ViewerInfo,
    handle: Arc<dyn ViewerHandle>,
}

fn registry() -> &'static Mutex<BTreeMap<String, Entry>> {
    static R: OnceLock<Mutex<BTreeMap<String, Entry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn new_id() -> String {
    format!("dv_{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Register a viewer (its `id` is filled in). Returns the id.
pub fn register(mut info: ViewerInfo, handle: Arc<dyn ViewerHandle>) -> String {
    if info.id.is_empty() {
        info.id = new_id();
    }
    if info.since == 0 {
        info.since = super::now_ms();
    }
    let id = info.id.clone();
    registry().lock().unwrap_or_else(|p| p.into_inner()).insert(id.clone(), Entry { info, handle });
    id
}

/// The socket ended.
pub fn unregister(id: &str) -> Option<ViewerInfo> {
    registry().lock().unwrap_or_else(|p| p.into_inner()).remove(id).map(|e| e.info)
}

/// Update a viewer's mode (take control, view-only).
pub fn set_mode(id: &str, mode: Mode) {
    if let Some(e) = registry().lock().unwrap_or_else(|p| p.into_inner()).get_mut(id) {
        e.info.mode = mode;
    }
}

/// Every viewer of `desktop`, oldest first.
pub fn list(desktop: &str) -> Vec<ViewerInfo> {
    let mut v: Vec<ViewerInfo> = registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .values()
        .filter(|e| e.info.desktop == desktop)
        .map(|e| e.info.clone())
        .collect();
    v.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.id.cmp(&b.id)));
    v
}

pub fn count(desktop: &str) -> usize {
    registry().lock().unwrap_or_else(|p| p.into_inner()).values().filter(|e| e.info.desktop == desktop).count()
}

/// Remove the matching viewers and close them (outside the lock).
fn close_where(code: u16, reason: &str, pred: impl Fn(&ViewerInfo) -> bool) -> usize {
    let gone: Vec<Entry> = {
        let mut r = registry().lock().unwrap_or_else(|p| p.into_inner());
        let ids: Vec<String> = r.values().filter(|e| pred(&e.info)).map(|e| e.info.id.clone()).collect();
        ids.iter().filter_map(|id| r.remove(id)).collect()
    };
    for e in &gone {
        e.handle.close(code, reason);
    }
    gone.len()
}

/// Close every viewer of `desktop`.
pub fn close_all(desktop: &str, code: u16, reason: &str) -> usize {
    close_where(code, reason, |v| v.desktop == desktop)
}

/// Close a login's viewers (by its stable id, or by name when `user_id`
/// is empty).
pub fn close_user(desktop: &str, user_id: &str, username: &str, code: u16, reason: &str) -> usize {
    close_where(code, reason, |v| {
        v.desktop == desktop && ((!user_id.is_empty() && v.user_id == user_id) || (!username.is_empty() && v.user == username))
    })
}

/// Close one viewer.
pub fn close_viewer(desktop: &str, id: &str, code: u16, reason: &str) -> usize {
    close_where(code, reason, |v| v.desktop == desktop && v.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::wire::{CLOSE_DESKTOP_OFF, CLOSE_FORBIDDEN};

    #[derive(Default)]
    struct Fake(Mutex<Vec<(u16, String)>>);
    impl ViewerHandle for Fake {
        fn close(&self, code: u16, reason: &str) {
            self.0.lock().unwrap().push((code, reason.to_string()));
        }
    }

    fn info(desktop: &str, user: &str) -> ViewerInfo {
        ViewerInfo {
            id: String::new(),
            desktop: desktop.into(),
            user: user.into(),
            user_id: format!("u-{user}"),
            role: "member".into(),
            mode: Mode::View,
            ingress: "tunnel".into(),
            ip: "203.0.113.1".into(),
            device: "web".into(),
            display: "1".into(),
            since: 0,
        }
    }

    #[test]
    fn close_all_closes_only_that_desktops_viewers_once() {
        let d = format!("test:{}", uuid::Uuid::new_v4());
        let other = format!("test:{}", uuid::Uuid::new_v4());
        let a = Arc::new(Fake::default());
        let b = Arc::new(Fake::default());
        let c = Arc::new(Fake::default());
        register(info(&d, "mel"), a.clone());
        register(info(&d, "ann"), b.clone());
        register(info(&other, "bob"), c.clone());
        assert_eq!(count(&d), 2);
        assert_eq!(close_all(&d, CLOSE_DESKTOP_OFF, "desktop_off"), 2);
        assert_eq!(close_all(&d, CLOSE_DESKTOP_OFF, "desktop_off"), 0, "gone");
        assert_eq!(*a.0.lock().unwrap(), vec![(4410, "desktop_off".to_string())]);
        assert_eq!(*b.0.lock().unwrap(), vec![(4410, "desktop_off".to_string())]);
        assert!(c.0.lock().unwrap().is_empty(), "another desktop untouched");
        assert_eq!(count(&other), 1);
        close_all(&other, CLOSE_DESKTOP_OFF, "desktop_off");
    }

    #[test]
    fn close_user_and_close_viewer() {
        let d = format!("test:{}", uuid::Uuid::new_v4());
        let a = Arc::new(Fake::default());
        let b = Arc::new(Fake::default());
        let id_a = register(info(&d, "mel"), a.clone());
        let id_b = register(info(&d, "ann"), b.clone());
        assert!(id_a.starts_with("dv_") && id_a != id_b);
        set_mode(&id_b, Mode::Control);
        assert_eq!(list(&d).iter().find(|v| v.id == id_b).unwrap().mode, Mode::Control);
        assert_eq!(close_user(&d, "u-mel", "", 4401, "grant_gone"), 1);
        assert_eq!(*a.0.lock().unwrap(), vec![(4401, "grant_gone".to_string())]);
        assert_eq!(close_viewer(&d, &id_b, CLOSE_FORBIDDEN, "kicked"), 1);
        assert_eq!(*b.0.lock().unwrap(), vec![(4403, "kicked".to_string())]);
        assert!(list(&d).is_empty());
        assert_eq!(unregister(&id_a), None, "closing removed it");
    }

    #[test]
    fn the_viewers_view_hides_the_id_and_desktop() {
        let mut v = info("console", "mel");
        v.id = "dv_x".into();
        v.since = 7;
        let j = serde_json::to_value(&v).unwrap();
        assert!(j.get("userId").is_none() && j.get("desktop").is_none(), "{j}");
        assert_eq!(j["id"], "dv_x");
        assert_eq!(j["since"], 7);
    }
}
