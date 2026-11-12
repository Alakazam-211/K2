//! The X11 clipboard (prd-linux-virtual-desktop-v1 L7, §3.6): text through
//! the CLIPBOARD selection, the OS end of [`crate::clipboard`] on Linux.
//!
//! X has no clipboard store: the program that copied owns the selection and
//! hands its contents to whoever asks. So K2 Screen keeps its own
//! connection with a small hidden window and a thread that:
//!
//! - **owns** the CLIPBOARD after a `clipboard_set` and answers requests for
//!   `TARGETS`, `UTF8_STRING`, `text/plain;charset=utf-8`, `TEXT` and
//!   `STRING` (Latin-1 text only) until another program copies;
//! - **reads** another owner's copy: `UTF8_STRING` first, else `STRING`,
//!   and the INCR protocol for copies sent in pieces. At most
//!   [`CLIPBOARD_MAX_BYTES`]: a bigger copy is [`ClipRead::TooLarge`], and
//!   only its size is ever looked at.
//!
//! Nothing here logs or returns the text in an error: refusals name sizes
//! at most (the policy and the access log are `crate::clipboard`'s and the
//! daemon's).

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use k2_deskd_proto::msg::{codes, CLIPBOARD_MAX_BYTES};

use crate::clipboard::ClipRead;

/// How long a copy's owner has to answer each step.
const ANSWER_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    utf8: Atom,
    mime_utf8: Atom,
    text: Atom,
    incr: Atom,
    /// Where an answer to our own read lands on our window.
    prop: Atom,
}

/// What the event thread hands a read in progress.
enum Note {
    /// `SelectionNotify`: the property the owner wrote (NONE: refused).
    Notify(Atom),
    /// A new value on our read property (an INCR piece).
    NewValue,
}

pub struct X11Clipboard {
    conn: Arc<RustConnection>,
    win: Window,
    atoms: Atoms,
    /// The text K2 owns now (`None` once another program copies).
    owned: Arc<Mutex<Option<String>>>,
    /// One read at a time; its notes from the event thread.
    reads: Mutex<Receiver<Note>>,
}

fn intern(c: &RustConnection, name: &str) -> Result<Atom, String> {
    Ok(c.intern_atom(false, name.as_bytes()).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom)
}

/// Text as a `STRING` (Latin-1), when every character fits.
fn latin1(s: &str) -> Option<Vec<u8>> {
    s.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect()
}

impl X11Clipboard {
    /// Its own connection to `DISPLAY`, a hidden window and the thread.
    pub fn connect() -> Result<X11Clipboard, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 connect: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms {
            clipboard: intern(&conn, "CLIPBOARD")?,
            targets: intern(&conn, "TARGETS")?,
            utf8: intern(&conn, "UTF8_STRING")?,
            mime_utf8: intern(&conn, "text/plain;charset=utf-8")?,
            text: intern(&conn, "TEXT")?,
            incr: intern(&conn, "INCR")?,
            prop: intern(&conn, "K2_SCREEN_CLIPBOARD")?,
        };
        let win = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_window(0, win, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY))
            .map_err(|e| e.to_string())?;
        conn.sync().map_err(|e| e.to_string())?;
        let conn = Arc::new(conn);
        let owned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = channel();
        let (c2, o2) = (Arc::clone(&conn), Arc::clone(&owned));
        std::thread::Builder::new()
            .name("k2-deskd-clipboard".into())
            .spawn(move || serve(&c2, win, atoms, &o2, &tx))
            .map_err(|e| e.to_string())?;
        Ok(X11Clipboard { conn, win, atoms, owned, reads: Mutex::new(rx) })
    }

    /// Put `text` on the CLIPBOARD (already checked by
    /// [`crate::clipboard::check_set`]).
    pub fn write(&self, text: &str) -> Result<(), (&'static str, String)> {
        let fail = || (codes::UNSUPPORTED, "K2 Screen lost its X11 connection".to_string());
        *self.owned.lock().unwrap_or_else(|p| p.into_inner()) = Some(text.to_string());
        self.conn.set_selection_owner(self.win, self.atoms.clipboard, CURRENT_TIME).map_err(|_| fail())?;
        let owner = self.conn.get_selection_owner(self.atoms.clipboard).map_err(|_| fail())?.reply().map_err(|_| fail())?.owner;
        if owner != self.win {
            return Err((codes::UNSUPPORTED, "Another program kept the clipboard".to_string()));
        }
        Ok(())
    }

    /// What the CLIPBOARD holds, as text.
    pub fn read(&self) -> ClipRead {
        let notes = self.reads.lock().unwrap_or_else(|p| p.into_inner());
        while notes.try_recv().is_ok() {}
        let Ok(owner) = self.conn.get_selection_owner(self.atoms.clipboard).map(|c| c.reply()) else { return ClipRead::Unsupported };
        let Ok(owner) = owner.map(|r| r.owner) else { return ClipRead::Unsupported };
        if owner == NONE {
            return ClipRead::Empty;
        }
        if owner == self.win {
            return match self.owned.lock().unwrap_or_else(|p| p.into_inner()).clone() {
                Some(t) if !t.is_empty() => ClipRead::Text(t),
                _ => ClipRead::Empty,
            };
        }
        for target in [self.atoms.utf8, AtomEnum::STRING.into()] {
            match self.ask(target, &notes) {
                Asked::Refused => continue,
                Asked::Failed => return ClipRead::NotText,
                Asked::Got(r) => return r,
            }
        }
        ClipRead::NotText
    }

    /// Ask the owner for `target` and read the answer.
    fn ask(&self, target: Atom, notes: &Receiver<Note>) -> Asked {
        let c = &self.conn;
        let sent = c
            .delete_property(self.win, self.atoms.prop)
            .and_then(|_| c.convert_selection(self.win, self.atoms.clipboard, target, self.atoms.prop, CURRENT_TIME))
            .and_then(|_| c.flush());
        if sent.is_err() {
            return Asked::Failed;
        }
        let property = match wait(notes, |n| match n {
            Note::Notify(p) => Some(p),
            Note::NewValue => None,
        }) {
            Some(p) => p,
            None => return Asked::Failed,
        };
        if property == NONE {
            return Asked::Refused;
        }
        // Ask one long word more than the cap: anything left over is too big.
        let long_len = (CLIPBOARD_MAX_BYTES / 4 + 1) as u32;
        let Ok(Ok(r)) = c.get_property(false, self.win, self.atoms.prop, AtomEnum::ANY, 0, long_len).map(|x| x.reply()) else { return Asked::Failed };
        if r.type_ == self.atoms.incr {
            let size = r.value32().and_then(|mut v| v.next()).unwrap_or(0) as usize;
            return Asked::Got(self.read_incr(size, target, notes));
        }
        let _ = c.delete_property(self.win, self.atoms.prop);
        let _ = c.flush();
        if r.bytes_after > 0 || r.value.len() > CLIPBOARD_MAX_BYTES {
            return Asked::Got(ClipRead::TooLarge { bytes: r.value.len() + r.bytes_after as usize });
        }
        if r.format != 8 {
            return Asked::Refused;
        }
        Asked::Got(decode(r.value, target == self.atoms.utf8))
    }

    /// The INCR protocol: delete the property to say "go on", then take
    /// each new value until an empty one. A copy announced or found to be
    /// over the cap stops there: only its size was looked at.
    fn read_incr(&self, announced: usize, target: Atom, notes: &Receiver<Note>) -> ClipRead {
        if announced > CLIPBOARD_MAX_BYTES {
            return ClipRead::TooLarge { bytes: announced };
        }
        let c = &self.conn;
        if c.delete_property(self.win, self.atoms.prop).and_then(|_| c.flush()).is_err() {
            return ClipRead::NotText;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            if wait(notes, |n| matches!(n, Note::NewValue).then_some(())).is_none() {
                // The owner stopped answering.
                return ClipRead::NotText;
            }
            // Take the piece and delete it (that asks for the next one).
            let Ok(Ok(r)) = c.get_property(true, self.win, self.atoms.prop, AtomEnum::ANY, 0, u32::MAX / 4).map(|x| x.reply()) else { return ClipRead::NotText };
            let _ = c.flush();
            if r.type_ == NONE {
                continue;
            }
            if r.value.is_empty() {
                break;
            }
            if out.len() + r.value.len() > CLIPBOARD_MAX_BYTES {
                return ClipRead::TooLarge { bytes: out.len() + r.value.len() };
            }
            out.extend_from_slice(&r.value);
        }
        decode(out, target == self.atoms.utf8)
    }
}

impl Drop for X11Clipboard {
    fn drop(&mut self) {
        // Ends the thread (its wait fails) and gives up the selection.
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}

enum Asked {
    /// The owner has no such target: try the next.
    Refused,
    /// No answer, or the connection failed.
    Failed,
    Got(ClipRead),
}

fn decode(bytes: Vec<u8>, utf8: bool) -> ClipRead {
    let text = if utf8 {
        match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => return ClipRead::NotText,
        }
    } else {
        bytes.iter().map(|&b| char::from(b)).collect()
    };
    if text.is_empty() {
        ClipRead::Empty
    } else {
        ClipRead::Text(text)
    }
}

/// The next note `pick` accepts, within [`ANSWER_WAIT`].
fn wait<T>(notes: &Receiver<Note>, pick: impl Fn(Note) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + ANSWER_WAIT;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        match notes.recv_timeout(left) {
            Ok(n) => {
                if let Some(t) = pick(n) {
                    return Some(t);
                }
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// The event thread: answer requests for what K2 owns, and pass a read's
/// notices on.
fn serve(c: &RustConnection, win: Window, a: Atoms, owned: &Mutex<Option<String>>, notes: &Sender<Note>) {
    loop {
        let ev = match c.wait_for_event() {
            Ok(e) => e,
            Err(_) => return,
        };
        match ev {
            Event::SelectionRequest(r) => {
                let text = owned.lock().unwrap_or_else(|p| p.into_inner()).clone();
                let property = answer(c, &r, a, text.as_deref());
                let n = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: r.time,
                    requestor: r.requestor,
                    selection: r.selection,
                    target: r.target,
                    property,
                };
                let _ = c.send_event(false, r.requestor, EventMask::NO_EVENT, n);
                let _ = c.flush();
            }
            Event::SelectionClear(e) if e.selection == a.clipboard => {
                *owned.lock().unwrap_or_else(|p| p.into_inner()) = None;
            }
            Event::SelectionNotify(e) if e.requestor == win => {
                let _ = notes.send(Note::Notify(e.property));
            }
            Event::PropertyNotify(p) if p.window == win && p.atom == a.prop && p.state == Property::NEW_VALUE => {
                let _ = notes.send(Note::NewValue);
            }
            Event::DestroyNotify(d) if d.window == win => return,
            _ => {}
        }
    }
}

/// Write the answer to a request on the requestor's property; the property
/// written, or NONE when refused.
fn answer(c: &RustConnection, r: &SelectionRequestEvent, a: Atoms, text: Option<&str>) -> Atom {
    if r.selection != a.clipboard {
        return NONE;
    }
    let Some(text) = text else { return NONE };
    // An old client names no property: use the target (ICCCM).
    let prop = if r.property == NONE { r.target } else { r.property };
    // Leave room for the request's own header within the server's limit;
    // anything bigger than one request is refused (the cap keeps K2's own
    // copies to 1 MiB, and BIG-REQUESTS servers take that in one).
    let room = c.maximum_request_bytes().saturating_sub(64);
    let ok = if r.target == a.targets {
        let list = [a.targets, a.utf8, a.mime_utf8, a.text, AtomEnum::STRING.into()];
        c.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::ATOM, &list).is_ok()
    } else if (r.target == a.utf8 || r.target == a.mime_utf8 || r.target == a.text) && text.len() <= room {
        let ty = if r.target == a.mime_utf8 { a.mime_utf8 } else { a.utf8 };
        c.change_property8(PropMode::REPLACE, r.requestor, prop, ty, text.as_bytes()).is_ok()
    } else if r.target == Atom::from(AtomEnum::STRING) {
        match latin1(text) {
            Some(b) if b.len() <= room => c.change_property8(PropMode::REPLACE, r.requestor, prop, AtomEnum::STRING, &b).is_ok(),
            _ => false,
        }
    } else {
        false
    };
    if ok {
        prop
    } else {
        NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_only_when_every_character_fits() {
        assert_eq!(latin1("héllo"), Some(vec![b'h', 0xe9, b'l', b'l', b'o']));
        assert_eq!(latin1("✓"), None);
    }

    #[test]
    fn decoding_keeps_text_and_refuses_bad_utf8() {
        assert_eq!(decode("pässwörd\n".as_bytes().to_vec(), true), ClipRead::Text("pässwörd\n".into()));
        assert_eq!(decode(vec![0xff, 0xfe], true), ClipRead::NotText);
        assert_eq!(decode(vec![b'h', 0xe9], false), ClipRead::Text("hé".into()));
        assert_eq!(decode(Vec::new(), true), ClipRead::Empty);
    }
}
