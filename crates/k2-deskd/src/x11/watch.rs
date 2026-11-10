//! Local-input detection on X11 (D4, vs-live R11): XInput2 raw events from
//! every device except the XTEST ones. XScreenSaver idle can't tell (XTEST
//! input resets it, so the controller's own input would count as local);
//! raw events carry the source device, and XTEST input comes from the
//! "Virtual core XTEST pointer/keyboard" slaves.
//!
//! Its own X connection, so its event queue never mixes with the poster's.

use std::collections::HashSet;
use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceId, XIEventMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::hold::LocalHold;

/// XIAllDevices / XIAllMasterDevices.
const ALL_DEVICES: DeviceId = 0;
const ALL_MASTER_DEVICES: DeviceId = 1;

/// Pure: is an event from `source` the person (not XTEST)?
pub fn is_local(source: DeviceId, xtest: &HashSet<DeviceId>) -> bool {
    !xtest.contains(&source)
}

/// Device ids whose name says XTEST.
pub fn xtest_ids(conn: &RustConnection) -> Result<HashSet<DeviceId>, String> {
    let r = conn.xinput_xi_query_device(ALL_DEVICES).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
    Ok(r.infos.iter().filter(|d| String::from_utf8_lossy(&d.name).contains("XTEST")).map(|d| d.deviceid).collect())
}

fn source_of(e: &Event) -> Option<DeviceId> {
    match e {
        Event::XinputRawKeyPress(r) | Event::XinputRawKeyRelease(r) => Some(r.sourceid),
        Event::XinputRawButtonPress(r) | Event::XinputRawButtonRelease(r) | Event::XinputRawMotion(r) => Some(r.sourceid),
        _ => None,
    }
}

/// Connects, selects raw events on the root and starts the thread.
pub fn spawn(hold: Arc<LocalHold>) -> Result<(), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 connect: {e}"))?;
    let v = conn.xinput_xi_query_version(2, 2).map_err(|e| e.to_string())?.reply().map_err(|e| format!("XInput2: {e}"))?;
    if v.major_version < 2 {
        return Err(format!("the X server has XInput {}.{}, deskd needs 2.0", v.major_version, v.minor_version));
    }
    let root = conn.setup().roots[screen].root;
    let mask = XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE | XIEventMask::RAW_BUTTON_PRESS | XIEventMask::RAW_BUTTON_RELEASE | XIEventMask::RAW_MOTION;
    conn.xinput_xi_select_events(root, &[xinput::EventMask { deviceid: ALL_MASTER_DEVICES, mask: vec![mask] }]).map_err(|e| e.to_string())?.check().map_err(|e| format!("XISelectEvents: {e}"))?;
    let mut xtest = xtest_ids(&conn)?;
    std::thread::Builder::new()
        .name("k2-deskd-xi2".into())
        .spawn(move || {
            let mut known: HashSet<DeviceId> = HashSet::new();
            loop {
                let ev = match conn.wait_for_event() {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("k2-deskd: X11 event stream ended: {e}");
                        return;
                    }
                };
                let Some(src) = source_of(&ev) else { continue };
                if !known.contains(&src) && !xtest.contains(&src) {
                    // A device we haven't seen: re-read the list once (hotplug).
                    if let Ok(ids) = xtest_ids(&conn) {
                        xtest = ids;
                    }
                    known.insert(src);
                }
                if is_local(src, &xtest) {
                    hold.note_local();
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xtest_devices_are_not_local() {
        let xt: HashSet<DeviceId> = [4, 5].into_iter().collect();
        assert!(!is_local(4, &xt));
        assert!(!is_local(5, &xt));
        assert!(is_local(9, &xt));
    }
}
