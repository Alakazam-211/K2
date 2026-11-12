//! Xvfb integration for L7 (prd-linux-virtual-desktop-v1 §3.6): K2 Screen's
//! clipboard on X11, text through the CLIPBOARD selection. Run with a
//! display, one test at a time (they share the one CLIPBOARD):
//!
//!   Xvfb :97 -screen 0 1280x720x24 -nolisten tcp &
//!   DISPLAY=:97 cargo nextest run -p k2-deskd --features xvfb-tests --test xvfb_clipboard -j 1
//!
//! (`scripts/test/deskd-checks.sh xvfb` runs it.) Another X client here is
//! a tiny x11rb peer that reads the selection, or owns it (directly, or in
//! INCR chunks as toolkits do for large copies).

#![cfg(all(target_os = "linux", feature = "xvfb-tests"))]

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property, SelectionNotifyEvent, Window, WindowClass,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use k2_deskd::clipboard::ClipRead;
use k2_deskd::x11::clipboard::X11Clipboard;
use k2_deskd_proto::msg::CLIPBOARD_MAX_BYTES;

/// Another X client on the same display.
struct Peer {
    conn: RustConnection,
    win: Window,
    clipboard: Atom,
    utf8: Atom,
    targets: Atom,
    incr: Atom,
    prop: Atom,
}

fn atom(c: &RustConnection, name: &str) -> Atom {
    c.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom
}

impl Peer {
    fn new() -> Peer {
        let (conn, screen) = x11rb::connect(None).expect("DISPLAY must name the Xvfb server");
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id().unwrap();
        conn.create_window(0, win, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE))
            .unwrap();
        let p = Peer {
            clipboard: atom(&conn, "CLIPBOARD"),
            utf8: atom(&conn, "UTF8_STRING"),
            targets: atom(&conn, "TARGETS"),
            incr: atom(&conn, "INCR"),
            prop: atom(&conn, "PEER_CLIP"),
            conn,
            win,
        };
        p.conn.sync().unwrap();
        p
    }

    fn wait_notify(&self) -> SelectionNotifyEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(Event::SelectionNotify(e)) = self.conn.poll_for_event().unwrap() {
                return e;
            }
            assert!(Instant::now() < deadline, "no SelectionNotify");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Ask the CLIPBOARD's owner for `target`; the property's (type, bytes),
    /// or None when the owner refused.
    fn convert(&self, target: Atom) -> Option<(Atom, Vec<u8>)> {
        self.conn.delete_property(self.win, self.prop).unwrap();
        self.conn.convert_selection(self.win, self.clipboard, target, self.prop, CURRENT_TIME).unwrap();
        self.conn.flush().unwrap();
        let e = self.wait_notify();
        if e.property == NONE {
            return None;
        }
        let r = self.conn.get_property(true, self.win, self.prop, AtomEnum::ANY, 0, u32::MAX / 4).unwrap().reply().unwrap();
        Some((r.type_, r.value))
    }

    fn owner(&self) -> Window {
        self.conn.get_selection_owner(self.clipboard).unwrap().reply().unwrap().owner
    }

    /// Own the CLIPBOARD with `text` and serve it from a thread until
    /// another client takes it. `incr`: hand it out in 64 KiB INCR chunks.
    fn own(self, text: Vec<u8>, incr: bool) -> std::thread::JoinHandle<()> {
        self.conn.set_selection_owner(self.win, self.clipboard, CURRENT_TIME).unwrap();
        self.conn.sync().unwrap();
        assert_eq!(self.owner(), self.win, "the peer owns the CLIPBOARD");
        std::thread::spawn(move || {
            let c = &self.conn;
            // INCR transfers in progress: (requestor, property, offset).
            let mut sending: Vec<(Window, Atom, usize)> = Vec::new();
            loop {
                let ev = match c.wait_for_event() {
                    Ok(e) => e,
                    Err(_) => return,
                };
                match ev {
                    Event::SelectionClear(_) => return,
                    Event::SelectionRequest(r) => {
                        let prop = if r.property == NONE { r.target } else { r.property };
                        let answered = if r.target == self.targets {
                            c.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::ATOM, &[self.targets, self.utf8]).unwrap();
                            true
                        } else if r.target == self.utf8 && incr {
                            c.change_window_attributes(r.requestor, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).unwrap();
                            c.change_property32(PropMode::REPLACE, r.requestor, prop, self.incr, &[text.len() as u32]).unwrap();
                            sending.push((r.requestor, prop, 0));
                            true
                        } else if r.target == self.utf8 {
                            c.change_property8(PropMode::REPLACE, r.requestor, prop, self.utf8, &text).unwrap();
                            true
                        } else {
                            false
                        };
                        let n = SelectionNotifyEvent {
                            response_type: SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: r.time,
                            requestor: r.requestor,
                            selection: r.selection,
                            target: r.target,
                            property: if answered { prop } else { NONE },
                        };
                        c.send_event(false, r.requestor, EventMask::NO_EVENT, n).unwrap();
                        c.flush().unwrap();
                    }
                    Event::PropertyNotify(p) if p.state == Property::DELETE => {
                        if let Some(i) = sending.iter().position(|(w, a, _)| *w == p.window && *a == p.atom) {
                            let (w, a, off) = sending[i];
                            let end = (off + 64 * 1024).min(text.len());
                            c.change_property8(PropMode::REPLACE, w, a, self.utf8, &text[off..end]).unwrap();
                            if off == end {
                                sending.remove(i); // the zero-length end
                            } else {
                                sending[i].2 = end;
                            }
                            c.flush().unwrap();
                        }
                    }
                    _ => {}
                }
            }
        })
    }
}

fn clip() -> X11Clipboard {
    X11Clipboard::connect().expect("K2 Screen's clipboard connects to the display")
}

#[test]
fn set_then_another_x_client_reads_it_back() {
    let k2 = clip();
    let text = "héllo from K2 ✓\nsecond line";
    k2.write(text).expect("set");
    let peer = Peer::new();
    assert_ne!(peer.owner(), NONE, "K2 Screen owns the CLIPBOARD");
    let (ty, bytes) = peer.convert(peer.utf8).expect("K2 answers UTF8_STRING");
    assert_eq!(ty, peer.utf8);
    assert_eq!(String::from_utf8(bytes).unwrap(), text);
    let (_, targets) = peer.convert(peer.targets).expect("K2 answers TARGETS");
    let atoms: Vec<u32> = targets.chunks(4).map(|b| u32::from_ne_bytes(b.try_into().unwrap())).collect();
    assert!(atoms.contains(&peer.utf8), "TARGETS lists UTF8_STRING: {atoms:?}");
    // K2 reads back what it owns.
    assert_eq!(k2.read(), ClipRead::Text(text.to_string()));
}

#[test]
fn a_copy_made_by_another_client_reads_as_text_direct_and_incr() {
    let k2 = clip();
    let peer = Peer::new();
    let small = "copied in another app".as_bytes().to_vec();
    let h = peer.own(small, false);
    assert_eq!(k2.read(), ClipRead::Text("copied in another app".into()));
    // Taking it back ends the peer.
    k2.write("x").unwrap();
    h.join().unwrap();

    let peer = Peer::new();
    let big: String = "abcdefghij".repeat(30_000); // 300 KB, in 64 KiB chunks
    let h = peer.own(big.clone().into_bytes(), true);
    assert_eq!(k2.read(), ClipRead::Text(big));
    k2.write("x").unwrap();
    h.join().unwrap();
}

#[test]
fn over_one_mib_is_refused_both_ways_and_nothing_echoes_the_text() {
    let k2 = clip();
    let secret = format!("hunter2-{}", "z".repeat(CLIPBOARD_MAX_BYTES));
    // Setting: the policy check refuses it before X sees it.
    let (code, msg) = k2_deskd::clipboard::check_set(&secret).unwrap_err();
    assert_eq!(code, "too_large");
    assert!(!msg.contains("hunter2"), "{msg}");
    // Reading a foreign copy over the cap, sent at once and in INCR chunks.
    for incr in [false, true] {
        let peer = Peer::new();
        let h = peer.own(secret.clone().into_bytes(), incr);
        let r = k2.read();
        assert!(matches!(r, ClipRead::TooLarge { .. }), "incr {incr}: {:?}", std::mem::discriminant(&r));
        let reply = k2_deskd::clipboard::get_reply(r);
        assert!(!reply.to_string().contains("hunter2"), "the reply never carries the text: {reply}");
        assert_eq!(reply, serde_json::json!({"text": null, "reason": "too_large"}));
        k2.write("x").unwrap();
        h.join().unwrap();
    }
}

#[test]
fn an_empty_clipboard_and_a_non_text_copy_say_so() {
    let k2 = clip();
    // Nobody owns it: release by owning it with a window that goes away.
    {
        let peer = Peer::new();
        peer.conn.set_selection_owner(peer.win, peer.clipboard, CURRENT_TIME).unwrap();
        peer.conn.sync().unwrap();
        // The peer's connection closes here; the selection has no owner.
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Peer::new().owner() != NONE {
        assert!(Instant::now() < deadline, "the selection kept an owner");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(k2.read(), ClipRead::Empty);

    // An owner that offers no text target (an image): not text.
    let peer = Peer::new();
    peer.conn.set_selection_owner(peer.win, peer.clipboard, CURRENT_TIME).unwrap();
    peer.conn.sync().unwrap();
    let h = std::thread::spawn(move || loop {
        match peer.conn.wait_for_event() {
            Ok(Event::SelectionRequest(r)) => {
                let n = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: r.time,
                    requestor: r.requestor,
                    selection: r.selection,
                    target: r.target,
                    property: NONE,
                };
                peer.conn.send_event(false, r.requestor, EventMask::NO_EVENT, n).unwrap();
                peer.conn.flush().unwrap();
            }
            Ok(Event::SelectionClear(_)) | Err(_) => return,
            Ok(_) => {}
        }
    });
    assert_eq!(k2.read(), ClipRead::NotText);
    k2.write("x").unwrap();
    h.join().unwrap();
}
