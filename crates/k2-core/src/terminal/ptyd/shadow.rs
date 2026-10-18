//! Shadow scanner (S0 decision A1): runs beside the vte parser over every
//! byte a ptyd session reads and tracks where the current *unfinished*
//! escape / UTF-8 sequence began, and where an open DEC 2026 synchronized
//! update began.
//!
//! After `stop_sync`, every byte before [`Shadow::partial_start`] is on the
//! grid and every byte from it on is not (an unfinished CSI/OSC/DCS/UTF-8
//! sequence has no effect until it completes). So a snapshot taken at
//! detach replays exactly from `partial_start()` — verified at 2,573 split
//! points in the S0 spike (0 mismatches), where "offset = bytes read" was
//! wrong at 633.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum St {
    #[default]
    Ground,
    Esc,
    EscIntermediate,
    Csi,
    Osc,
    OscEsc,
    Str,
    StrEsc,
    Utf8(u8),
}

#[derive(Debug, Default, Clone)]
pub struct Shadow {
    pos: u64,
    st: St,
    seq_start: Option<u64>,
    csi: Vec<u8>,
    sync_start: Option<u64>,
}

impl Shadow {
    /// A scanner whose first byte is at stream offset `base`.
    pub fn at(base: u64) -> Shadow {
        Shadow { pos: base, ..Shadow::default() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
            self.pos += 1;
        }
    }

    /// Offset of the next byte to be scanned (= bytes read so far + base).
    pub fn pos(&self) -> u64 {
        self.pos
    }

    /// First offset the grid has NOT seen (after `stop_sync`).
    pub fn partial_start(&self) -> u64 {
        self.seq_start.unwrap_or(self.pos)
    }

    /// Start of an open synchronized update, if any.
    pub fn sync_start(&self) -> Option<u64> {
        self.sync_start
    }

    fn begin(&mut self, st: St) {
        if self.seq_start.is_none() {
            self.seq_start = Some(self.pos);
        }
        self.st = st;
    }

    fn end(&mut self) {
        self.st = St::Ground;
        self.seq_start = None;
    }

    fn restart_esc_at(&mut self, at: u64) {
        self.st = St::Esc;
        self.seq_start = Some(at);
    }

    fn step(&mut self, b: u8) {
        match self.st {
            St::Ground => match b {
                0x1b => {
                    self.seq_start = None;
                    self.begin(St::Esc)
                }
                0xc2..=0xdf => self.begin(St::Utf8(1)),
                0xe0..=0xef => self.begin(St::Utf8(2)),
                0xf0..=0xf4 => self.begin(St::Utf8(3)),
                _ => {}
            },
            St::Esc => match b {
                b'[' => {
                    self.st = St::Csi;
                    self.csi.clear();
                }
                b']' => self.st = St::Osc,
                b'P' | b'X' | b'^' | b'_' => self.st = St::Str,
                0x20..=0x2f => self.st = St::EscIntermediate,
                0x18 | 0x1a => self.end(),
                0x1b => self.seq_start = Some(self.pos),
                _ => self.end(),
            },
            St::EscIntermediate => {
                if !(0x20..=0x2f).contains(&b) {
                    self.end()
                }
            }
            St::Csi => match b {
                0x40..=0x7e => {
                    self.csi.push(b);
                    if self.csi == b"?2026h" && self.sync_start.is_none() {
                        self.sync_start = self.seq_start;
                    }
                    if self.csi == b"?2026l" {
                        self.sync_start = None;
                    }
                    self.end()
                }
                0x1b => self.restart_esc_at(self.pos),
                0x18 | 0x1a => self.end(),
                _ => {
                    if self.csi.len() < 64 {
                        self.csi.push(b)
                    }
                }
            },
            St::Osc => match b {
                0x07 | 0x18 | 0x1a => self.end(),
                0x1b => self.st = St::OscEsc,
                _ => {}
            },
            St::OscEsc | St::StrEsc => {
                if b == b'\\' {
                    self.end()
                } else {
                    // ESC ended the string; it also starts a new escape.
                    let at = self.pos - 1;
                    self.end();
                    self.restart_esc_at(at);
                    self.step(b);
                }
            }
            St::Str => match b {
                0x18 | 0x1a => self.end(),
                0x1b => self.st = St::StrEsc,
                _ => {}
            },
            St::Utf8(left) => {
                if (0x80..=0xbf).contains(&b) {
                    if left == 1 {
                        self.end()
                    } else {
                        self.st = St::Utf8(left - 1)
                    }
                } else {
                    self.end();
                    self.step(b);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(b: &[u8]) -> Shadow {
        let mut s = Shadow::default();
        s.feed(b);
        s
    }

    #[test]
    fn ground_text_has_no_partial() {
        let s = scan(b"hello \x1b[31mred\x1b[0m");
        assert_eq!(s.partial_start(), s.pos());
    }

    #[test]
    fn unfinished_csi_osc_dcs_and_utf8_point_at_their_start() {
        assert_eq!(scan(b"ab\x1b[3").partial_start(), 2);
        assert_eq!(scan(b"ab\x1b]2;tit").partial_start(), 2);
        assert_eq!(scan(b"ab\x1bP1$r").partial_start(), 2);
        assert_eq!(scan(b"ab\x1b").partial_start(), 2);
        assert_eq!(scan("abï".as_bytes()[..3].as_ref()).partial_start(), 2);
        assert_eq!(scan("ab漢".as_bytes()[..4].as_ref()).partial_start(), 2);
        // Finished ones don't.
        assert_eq!(scan(b"\x1b]2;t\x07x").partial_start(), 7);
        assert_eq!(scan(b"\x1b]2;t\x1b\\x").partial_start(), 8);
    }

    #[test]
    fn st_esc_that_is_not_backslash_starts_a_new_escape() {
        // OSC ended by ESC, which then begins an unfinished CSI.
        let s = scan(b"\x1b]2;t\x1b[1");
        assert_eq!(s.partial_start(), 5);
    }

    #[test]
    fn sync_updates_are_tracked() {
        let mut s = scan(b"x\x1b[?2026hframe");
        assert_eq!(s.sync_start(), Some(1));
        s.feed(b"\x1b[?2026l");
        assert_eq!(s.sync_start(), None);
    }

    #[test]
    fn base_offset_is_honoured() {
        let mut s = Shadow::at(1000);
        s.feed(b"ab\x1b[");
        assert_eq!(s.partial_start(), 1002);
        assert_eq!(s.pos(), 1004);
    }
}
