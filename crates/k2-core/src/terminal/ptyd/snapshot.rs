//! k2-ptyd S4: full screen restore (PRD prd-k2-ptyd-v1 §7.7, P41, P42,
//! P78; S0 amendments A–C in `ptyd-s0-results.md` §5).
//!
//! When a daemon lets go of a ptyd session (a restart that keeps agents),
//! it writes the session's Term to `<K2 home>/run/term/<sid>.snap`
//! together with the stream offset of the first byte the grid has NOT
//! seen (`IoEnd::safe_offset`, exact after `stop_sync`). The IO loop also
//! checkpoints the same way every 30 s while output changed, so a crash
//! restores too. The next daemon rebuilds the Term from the snapshot and
//! replays only the ring tail after it, instead of the whole ring: the
//! full scrollback (5,000 lines) comes back even when the 4 MiB ring has
//! wrapped past it, and a TUI's screen is exact.
//!
//! Content: the ACTIVE grid with its scrollback (serde), both cursors with
//! their pen and `input_needs_wrap` (the grid serializes without them),
//! the private-mode bits (with the kitty keyboard flags), the cursor
//! style, the palette, the size and the offset. Restore order (amendment
//! A): `Term::new(snapshot size)` → prelude (DECSET 1049 first, origin
//! before cursors, then the other modes, kitty flags, DECSCUSR, palette)
//! → `*grid_mut() = grid` → both cursors → resize to the live size → tail.
//! Not restored (N5): the inactive screen, DECSTBM, tab stops, the title
//! stack, the charsets, the parser's mid-sequence state.
//!
//! Codec (amendment C): bincode, then gzip. File: 0600 in a 0700 dir,
//! temp + fsync + rename, a magic + version header. Screen contents only:
//! never env values, tokens or keys (P72).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, Processor};
use serde::{Deserialize, Serialize};

/// F7: a saved-screen file larger than this is refused (and removed). A
/// full 5,000-line scrollback is well under 8 MiB.
pub const MAX_FILE: u64 = 64 << 20;
/// F7: the most a saved screen may inflate to.
pub const MAX_RAW: u64 = 256 << 20;

/// File magic + format version (a change in layout bumps the version; an
/// unknown version is ignored and the session replays from the ring).
const MAGIC: &[u8; 8] = b"K2TSNAP\0";
pub const FORMAT: u32 = 1;

/// Palette entries a terminal program can set with OSC 4/10/11/12: the 256
/// indexed colours plus foreground (256), background (257), cursor (258).
const PALETTE_SETTABLE: usize = 259;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapCursor {
    pub line: i32,
    pub column: usize,
    pub template: Cell,
    pub input_needs_wrap: bool,
}

/// One session's Term at a known stream offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// The first stream byte NOT on this grid: replay starts here.
    pub ring_offset: u64,
    pub cols: usize,
    pub rows: usize,
    pub grid: Grid<Cell>,
    pub cursor: SnapCursor,
    pub saved_cursor: SnapCursor,
    pub mode_bits: u32,
    /// DECSCUSR shape (0 block, 1 underline, 2 beam, 3 hollow, 4 hidden) and blink.
    pub cursor_shape: u8,
    pub cursor_blinking: bool,
    /// Palette entries that differ from unset: (index, r, g, b).
    pub palette: Vec<(u16, u8, u8, u8)>,
}

fn snap_cursor(c: &alacritty_terminal::grid::Cursor<Cell>) -> SnapCursor {
    SnapCursor { line: c.point.line.0, column: c.point.column.0, template: c.template.clone(), input_needs_wrap: c.input_needs_wrap }
}

fn shape_code(s: CursorShape) -> u8 {
    match s {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Beam => 2,
        CursorShape::HollowBlock => 3,
        CursorShape::Hidden => 4,
    }
}

impl Snapshot {
    /// Capture `term` with `ring_offset` = the first byte it has not seen.
    /// The caller holds the Term lock (or owns the Term) and has already
    /// flushed any synchronized update (`stop_sync`).
    pub fn capture<L: EventListener>(term: &Term<L>, ring_offset: u64) -> Snapshot {
        let grid = term.grid();
        let style = term.cursor_style();
        let colors = term.colors();
        let palette = (0..PALETTE_SETTABLE)
            .filter_map(|i| colors[i].map(|c| (i as u16, c.r, c.g, c.b)))
            .collect();
        Snapshot {
            ring_offset,
            cols: grid.columns(),
            rows: grid.screen_lines(),
            grid: grid.clone(),
            cursor: snap_cursor(&grid.cursor),
            saved_cursor: snap_cursor(&grid.saved_cursor),
            mode_bits: term.mode().bits(),
            cursor_shape: shape_code(style.shape),
            cursor_blinking: style.blinking,
            palette,
        }
    }

    /// The escape sequences that put a fresh Term (same config) into this
    /// snapshot's modes, cursor style and palette (amendment A order:
    /// alt screen first, origin mode before the cursors are set).
    pub fn prelude(&self, defaults: TermMode) -> Vec<u8> {
        let mode = TermMode::from_bits_truncate(self.mode_bits);
        let mut out = Vec::new();
        if mode.contains(TermMode::ALT_SCREEN) {
            out.extend_from_slice(b"\x1b[?1049h");
        }
        // (mode, DEC private number) — private modes K2 can set by DECSET.
        const PRIVATE: &[(TermMode, u16)] = &[
            (TermMode::ORIGIN, 6),
            (TermMode::APP_CURSOR, 1),
            (TermMode::SHOW_CURSOR, 25),
            (TermMode::LINE_WRAP, 7),
            (TermMode::MOUSE_REPORT_CLICK, 1000),
            (TermMode::MOUSE_DRAG, 1002),
            (TermMode::MOUSE_MOTION, 1003),
            (TermMode::FOCUS_IN_OUT, 1004),
            (TermMode::UTF8_MOUSE, 1005),
            (TermMode::SGR_MOUSE, 1006),
            (TermMode::ALTERNATE_SCROLL, 1007),
            (TermMode::URGENCY_HINTS, 1042),
            (TermMode::BRACKETED_PASTE, 2004),
        ];
        for &(m, n) in PRIVATE {
            if mode.contains(m) != defaults.contains(m) {
                let c = if mode.contains(m) { 'h' } else { 'l' };
                out.extend_from_slice(format!("\x1b[?{n}{c}").as_bytes());
            }
        }
        // ANSI (non-private) modes: IRM (4), LNM (20).
        for (m, n) in [(TermMode::INSERT, 4u16), (TermMode::LINE_FEED_NEW_LINE, 20)] {
            if mode.contains(m) != defaults.contains(m) {
                let c = if mode.contains(m) { 'h' } else { 'l' };
                out.extend_from_slice(format!("\x1b[{n}{c}").as_bytes());
            }
        }
        if mode.contains(TermMode::APP_KEYPAD) != defaults.contains(TermMode::APP_KEYPAD) {
            out.extend_from_slice(if mode.contains(TermMode::APP_KEYPAD) { b"\x1b=" } else { b"\x1b>" });
        }
        // Kitty keyboard flags (TermMode bits 18..=22, in protocol order):
        // push them as the current set.
        let kitty = (self.mode_bits >> 18) & 0x1f;
        if kitty != 0 {
            out.extend_from_slice(format!("\x1b[>{kitty}u").as_bytes());
        }
        // DECSCUSR: 1/2 block, 3/4 underline, 5/6 beam (odd = blinking).
        let base = match self.cursor_shape {
            0 => Some(1),
            1 => Some(3),
            2 => Some(5),
            _ => None, // hollow/hidden are not settable by DECSCUSR
        };
        if let Some(b) = base {
            let n = if self.cursor_blinking { b } else { b + 1 };
            out.extend_from_slice(format!("\x1b[{n} q").as_bytes());
        }
        for &(i, r, g, b) in &self.palette {
            let rgb = format!("rgb:{r:02x}/{g:02x}/{b:02x}");
            let osc = match i {
                0..=255 => format!("\x1b]4;{i};{rgb}\x07"),
                256 => format!("\x1b]10;{rgb}\x07"),
                257 => format!("\x1b]11;{rgb}\x07"),
                258 => format!("\x1b]12;{rgb}\x07"),
                _ => continue,
            };
            out.extend_from_slice(osc.as_bytes());
        }
        out
    }

    /// Apply this snapshot to a FRESH Term built at the snapshot's size
    /// (amendment A order), then resize it to `cols`×`rows`. The caller
    /// has muted the listener's query replies (P75): the prelude must not
    /// answer anything into the agent.
    pub fn restore_into<L: EventListener>(&self, term: &mut Term<L>, cols: usize, rows: usize) {
        let defaults = *term.mode();
        let mut parser: Processor = Processor::new();
        parser.advance(term, &self.prelude(defaults));
        *term.grid_mut() = self.grid.clone();
        let g = term.grid_mut();
        for (dst, src) in [(&mut g.cursor, &self.cursor), (&mut g.saved_cursor, &self.saved_cursor)] {
            dst.point = Point::new(Line(src.line), Column(src.column));
            dst.template = src.template.clone();
            dst.input_needs_wrap = src.input_needs_wrap;
        }
        if cols != self.cols || rows != self.rows {
            term.resize(crate::terminal::daemon_pty::TermSize { cols: cols.max(1), rows: rows.max(1) });
        }
    }

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let raw = bincode::serialize(self).map_err(io::Error::other)?;
        let mut out = Vec::with_capacity(raw.len() / 8 + 16);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT.to_be_bytes());
        let mut gz = flate2::write::GzEncoder::new(out, flate2::Compression::fast());
        gz.write_all(&raw)?;
        gz.finish()
    }

    /// `None` for a foreign file, another format version, or corrupt data.
    /// Bounded (security review F7): the gzip stream inflates to at most
    /// [`MAX_RAW`] bytes and bincode reads within that limit, so a hostile
    /// file can't exhaust memory.
    pub fn decode(bytes: &[u8]) -> Option<Snapshot> {
        use bincode::Options;
        let rest = bytes.strip_prefix(MAGIC.as_slice())?;
        let (ver, body) = rest.split_at_checked(4)?;
        if u32::from_be_bytes(ver.try_into().ok()?) != FORMAT {
            return None;
        }
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(body).take(MAX_RAW + 1).read_to_end(&mut raw).ok()?;
        if raw.len() as u64 > MAX_RAW {
            return None;
        }
        // The same encoding `bincode::serialize` writes, with a limit.
        let s: Snapshot = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .with_limit(MAX_RAW)
            .deserialize(&raw)
            .ok()?;
        // Sanity: the cursors must sit on the grid it carries.
        let ok = |c: &SnapCursor| c.column < s.cols.max(1) && c.line >= 0 && (c.line as usize) < s.rows.max(1);
        (s.cols == s.grid.columns() && s.rows == s.grid.screen_lines() && ok(&s.cursor) && ok(&s.saved_cursor)).then_some(s)
    }
}

/// `<K2 home>/run/term` (P88: from the K2 home).
pub fn dir() -> PathBuf {
    crate::paths::k2_home().join("run").join("term")
}

/// The snapshot path of `sid` (a session UUID; anything else is refused).
pub fn path_for(sid: &str) -> Option<PathBuf> {
    let ok = !sid.is_empty() && sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ok.then(|| dir().join(format!("{sid}.snap")))
}

/// Write atomically: 0700 dir, 0600 temp file, fsync, rename.
pub fn write(sid: &str, snap: &Snapshot) -> io::Result<()> {
    let path = path_for(sid).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a session id"))?;
    write_to(&path, snap)
}

pub fn write_to(path: &Path, snap: &Snapshot) -> io::Result<()> {
    write_private_atomic(path, &snap.encode()?)
}

/// Security review F8: make `dir` a private directory WITHOUT following a
/// planted symlink: create it 0700 if missing, then `lstat` it — it must be
/// a real directory owned by our euid — and only then tighten its mode.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let md = std::fs::symlink_metadata(dir)?;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    if md.file_type().is_symlink() || !md.is_dir() || md.uid() != me {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not a private directory of ours", dir.display())));
    }
    if md.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Security review F8: write `bytes` to `path` atomically and privately —
/// a private parent dir ([`ensure_private_dir`]), a temp file with an
/// unpredictable name opened `create_new` + `O_NOFOLLOW` at 0600 (a
/// planted file or symlink makes it fail rather than be followed or
/// truncated), fsync, rename. The temp file is removed on any failure.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        ensure_private_dir(d)?;
    }
    let rand = k2_ptyd::auth::hex(&k2_ptyd::auth::random_bytes::<8>()?);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{rand}.tmp"));
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Read `sid`'s snapshot (`None` when absent or unusable). Security review
/// F7: only a regular file (not a symlink) of at most [`MAX_FILE`] bytes
/// that decodes within bounds AND restores cleanly at `cols`×`rows` (a
/// trial restore under `catch_unwind`: a malformed grid must never panic
/// adoption into a crash loop) is used; anything else is removed and logged.
pub fn read(sid: &str, cols: usize, rows: usize) -> Option<Snapshot> {
    read_path(&path_for(sid)?, cols, rows)
}

/// [`read`] for an explicit path.
pub fn read_path(path: &Path, cols: usize, rows: usize) -> Option<Snapshot> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = path.to_path_buf();
    let bad = |why: &str| {
        crate::log_debug!("[ptyd] saved screen {} refused ({why}); removed", path.display());
        let _ = std::fs::remove_file(&path);
        None
    };
    let md = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(_) => return None, // none saved
    };
    if !md.is_file() {
        return bad("not a regular file");
    }
    if md.len() > MAX_FILE {
        return bad("too large");
    }
    let mut bytes = Vec::new();
    let opened = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path);
    match opened.and_then(|f| f.take(MAX_FILE + 1).read_to_end(&mut bytes)) {
        Ok(_) if bytes.len() as u64 <= MAX_FILE => {}
        _ => return bad("unreadable"),
    }
    let Some(snap) = Snapshot::decode(&bytes) else { return bad("undecodable") };
    if !snap.restores_cleanly(cols, rows) {
        return bad("does not restore");
    }
    Some(snap)
}

impl Snapshot {
    /// Restore into a scratch Term and catch any panic (a grid whose
    /// internals don't hold together).
    pub fn restores_cleanly(&self, cols: usize, rows: usize) -> bool {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::Config;
        let snap = self.clone();
        std::panic::catch_unwind(move || {
            let size = crate::terminal::daemon_pty::TermSize { cols: snap.cols.max(1), rows: snap.rows.max(1) };
            let mut t = Term::new(Config::default(), &size, VoidListener);
            snap.restore_into(&mut t, cols.max(1), rows.max(1));
        })
        .is_ok()
    }
}

/// The session ended: its screen contents go (S-5).
pub fn remove(sid: &str) {
    if let Some(p) = path_for(sid) {
        let _ = std::fs::remove_file(p);
    }
}

/// The standard cache-directory tag (bford.info/cachedir): borg, restic
/// (`--exclude-caches`), GNU tar (`--exclude-caches`) and others skip a
/// directory holding it.
pub const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
# This file is a cache directory tag created by K2: runtime state (saved agent\n\
# screens, sockets, keys) that must not be backed up.\n";

/// Security review F2: keep `<K2 home>/run` — saved screens, carried
/// activity rows, sockets, ptyd's key — out of backups.
/// - macOS: Time Machine's sticky exclusion (`tmutil addexclusion`, an
///   xattr on the directory; no root), set when not already excluded.
/// - Everywhere: a `CACHEDIR.TAG`, which the common Linux backup tools
///   honour (there is no OS-wide exclusion on Linux).
///
/// Best effort (logged); once per process.
pub fn keep_out_of_backups(run: &Path) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        if let Err(e) = std::fs::create_dir_all(run) {
            crate::log_debug!("[ptyd] backups: {}: {e}", run.display());
            return;
        }
        let tag = run.join("CACHEDIR.TAG");
        if !tag.exists() {
            if let Err(e) = std::fs::write(&tag, CACHEDIR_TAG) {
                crate::log_debug!("[ptyd] backups: CACHEDIR.TAG: {e}");
            }
        }
        #[cfg(target_os = "macos")]
        {
            let excluded = crate::bg_proc::command("/usr/bin/tmutil")
                .arg("isexcluded")
                .arg(run)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("[Excluded]"))
                .unwrap_or(false);
            if !excluded {
                match crate::bg_proc::command("/usr/bin/tmutil").arg("addexclusion").arg(run).status() {
                    Ok(s) if s.success() => crate::log_debug!("[ptyd] backups: Time Machine excludes {}", run.display()),
                    Ok(s) => crate::log_debug!("[ptyd] backups: tmutil addexclusion {}: {s}", run.display()),
                    Err(e) => crate::log_debug!("[ptyd] backups: tmutil: {e}"),
                }
            }
        }
    });
}

/// Boot (B1): drop snapshots of sessions no longer running in ptyd.
pub fn sweep(keep: &std::collections::HashSet<String>) -> usize {
    let Ok(rd) = std::fs::read_dir(dir()) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let sid = name.split('.').next().unwrap_or("");
        if !keep.contains(sid) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;

    fn term(cols: usize, rows: usize) -> Term<VoidListener> {
        let size = crate::terminal::daemon_pty::TermSize { cols, rows };
        Term::new(Config { scrolling_history: 5000, ..Config::default() }, &size, VoidListener)
    }

    fn feed(t: &mut Term<VoidListener>, bytes: &[u8]) {
        let mut p: Processor = Processor::new();
        p.advance(t, bytes);
    }

    fn screen(t: &Term<VoidListener>) -> Vec<String> {
        let g = t.grid();
        let top = -(g.history_size() as i32);
        (top..g.screen_lines() as i32)
            .map(|l| (0..g.columns()).map(|c| g[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string())
            .collect()
    }

    /// A rich stream: coloured scrollback, UTF-8/CJK, modes, kitty flags,
    /// DECSCUSR, palette, DECSC, a pen left on.
    fn stream() -> Vec<u8> {
        let mut s = Vec::new();
        for i in 0..300 {
            s.extend_from_slice(format!("\x1b[3{}mline {i} ünï 漢字\x1b[0m\r\n", i % 8).as_bytes());
        }
        s.extend_from_slice(b"\x1b[?2004h\x1b[?1h\x1b=\x1b[?1004h\x1b[4 q\x1b]4;1;rgb:12/34/56\x07\x1b]11;rgb:01/02/03\x07");
        s.extend_from_slice(b"\x1b[5;10H\x1b7\x1b[2;3H\x1b[1;31mpen-on");
        s
    }

    /// Snapshot at an offset, restore, feed the same tail to both: the
    /// restored Term equals the uninterrupted one (cells, history, cursor,
    /// pen, modes, cursor style, palette).
    #[test]
    fn restore_then_tail_equals_the_uninterrupted_term() {
        let all = stream();
        let tail = b"\x1b[0m and more\r\nnext \x1b8saved".to_vec();
        let mut whole = term(80, 24);
        feed(&mut whole, &all);
        feed(&mut whole, &tail);

        let mut before = term(80, 24);
        feed(&mut before, &all);
        let snap = Snapshot::capture(&before, all.len() as u64);
        let snap = Snapshot::decode(&snap.encode().expect("encode")).expect("decode");
        assert_eq!(snap.ring_offset, all.len() as u64);

        let mut restored = term(snap.cols, snap.rows);
        snap.restore_into(&mut restored, 80, 24);
        feed(&mut restored, &tail);

        assert_eq!(screen(&restored), screen(&whole), "cells and scrollback");
        assert_eq!(restored.grid().history_size(), whole.grid().history_size());
        assert_eq!(restored.grid().cursor.point, whole.grid().cursor.point, "cursor");
        assert_eq!(restored.grid().cursor.template, whole.grid().cursor.template, "pen");
        assert_eq!(restored.grid().saved_cursor.point, whole.grid().saved_cursor.point, "saved cursor");
        assert_eq!(restored.mode(), whole.mode(), "modes");
        assert_eq!(restored.cursor_style(), whole.cursor_style(), "cursor style");
        for i in 0..PALETTE_SETTABLE {
            assert_eq!(restored.colors()[i], whole.colors()[i], "palette {i}");
        }
    }

    /// Amendment A: a snapshot taken on the alt screen restores ON the alt
    /// screen (the prelude's 1049 runs before the grid is put back).
    #[test]
    fn alt_screen_snapshot_restores_on_the_alt_screen() {
        let mut t = term(40, 10);
        feed(&mut t, b"primary\r\n\x1b[?1049h\x1b[Halt-screen-ui");
        let snap = Snapshot::capture(&t, 0);
        let mut r = term(40, 10);
        snap.restore_into(&mut r, 40, 10);
        assert!(r.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(screen(&r)[r.grid().history_size()], "alt-screen-ui");
        assert_eq!(screen(&r), screen(&t));
    }

    /// The snapshot size is restored first, then resized to the live size.
    #[test]
    fn restores_at_snapshot_size_then_resizes() {
        let mut t = term(100, 30);
        feed(&mut t, &stream());
        let snap = Snapshot::capture(&t, 7);
        let mut r = term(snap.cols, snap.rows);
        snap.restore_into(&mut r, 120, 40);
        assert_eq!((r.grid().columns(), r.grid().screen_lines()), (120, 40));
        let mut want = term(100, 30);
        feed(&mut want, &stream());
        want.resize(crate::terminal::daemon_pty::TermSize { cols: 120, rows: 40 });
        assert_eq!(screen(&r), screen(&want));
    }

    #[test]
    fn decode_refuses_foreign_and_other_versions() {
        let snap = Snapshot::capture(&term(10, 4), 0);
        let mut bytes = snap.encode().unwrap();
        assert!(Snapshot::decode(&bytes).is_some());
        assert!(Snapshot::decode(b"not a snapshot").is_none());
        bytes[MAGIC.len() + 3] ^= 0xff; // the version
        assert!(Snapshot::decode(&bytes).is_none());
        let mut bytes = snap.encode().unwrap();
        let n = bytes.len();
        bytes.truncate(n - 8);
        assert!(Snapshot::decode(&bytes).is_none(), "truncated gzip");
    }

    #[test]
    fn a_full_scrollback_snapshot_is_small_and_fast() {
        let mut t = term(200, 50);
        let mut s = Vec::new();
        for i in 0..6000 {
            s.extend_from_slice(format!("\x1b[3{}m{i:05} the quick brown fox jumps over the lazy dog {}\x1b[0m\r\n", i % 8, "x".repeat(i % 120)).as_bytes());
        }
        feed(&mut t, &s);
        let t0 = std::time::Instant::now();
        let bytes = Snapshot::capture(&t, 0).encode().unwrap();
        let took = t0.elapsed();
        assert!(bytes.len() < 8 << 20, "snapshot is {} bytes", bytes.len());
        // Generous (debug builds, busy CI); S0 measured ~110 ms release.
        assert!(took < std::time::Duration::from_secs(5), "capture + encode took {took:?}");
    }

    /// F7: a gzip bomb (a valid header over a stream inflating past
    /// MAX_RAW) is refused without inflating it all.
    #[test]
    fn a_gzip_bomb_is_refused() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let zeros = vec![0u8; 1 << 20];
        for _ in 0..(MAX_RAW / (1 << 20) + 2) {
            gz.write_all(&zeros).unwrap();
        }
        let body = gz.finish().unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&FORMAT.to_be_bytes());
        bytes.extend_from_slice(&body);
        assert!(bytes.len() < 2 << 20, "the bomb itself is small ({} bytes)", bytes.len());
        assert!(Snapshot::decode(&bytes).is_none());
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2snap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// F7: a saved screen that isn't usable is removed (never retried at
    /// every boot) and reads as "none": too large, garbage, a symlink. A
    /// good one reads back.
    #[test]
    fn unusable_saved_screens_are_removed() {
        let d = scratch("read");
        let good = d.join("good.snap");
        let mut t = term(20, 5);
        feed(&mut t, b"hello\r\nworld");
        write_to(&good, &Snapshot::capture(&t, 3)).unwrap();
        assert_eq!(read_path(&good, 20, 5).expect("good").ring_offset, 3);
        let garbage = d.join("garbage.snap");
        std::fs::write(&garbage, b"K2TSNAP\0\0\0\0\x01not gzip").unwrap();
        assert!(read_path(&garbage, 20, 5).is_none());
        assert!(!garbage.exists(), "a garbage file is removed");
        let big = d.join("big.snap");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FILE + 1).unwrap();
        assert!(read_path(&big, 20, 5).is_none());
        assert!(!big.exists(), "an oversized file is removed");
        let link = d.join("link.snap");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(read_path(&link, 20, 5).is_none());
        assert!(good.exists(), "the symlink's target is untouched");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// F8: writes never follow a planted symlink: a symlinked directory is
    /// refused (its target's mode untouched), and a symlink at the final
    /// path is replaced by the new file while its target stays intact.
    #[test]
    fn private_writes_do_not_follow_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("f8");
        // A symlinked directory.
        let elsewhere = d.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
        let term_dir = d.join("term");
        std::os::unix::fs::symlink(&elsewhere, &term_dir).unwrap();
        let err = write_private_atomic(&term_dir.join("x.snap"), b"data").expect_err("a symlinked dir must be refused");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        let mode = std::fs::metadata(&elsewhere).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "the symlink's target was chmodded");
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing was written through the symlink");
        // A symlink planted at the final path.
        let real = d.join("real");
        let victim = d.join("victim.txt");
        std::fs::write(&victim, b"victim").unwrap();
        let target = real.join("s.snap");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&victim, &target).unwrap();
        write_private_atomic(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"victim", "the victim file was overwritten");
        assert!(std::fs::symlink_metadata(&target).unwrap().is_file(), "the symlink was replaced by a real file");
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o700);
        // No temp files are left behind.
        let names: Vec<String> = std::fs::read_dir(&real).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["s.snap".to_string()], "{names:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn paths_stay_in_their_dir() {
        assert!(path_for("1ade4322-dfe4-47d9-af31-de9b03a6baf7").is_some());
        for bad in ["", "../x", "a/b", "x.snap"] {
            assert!(path_for(bad).is_none(), "{bad:?}");
        }
    }
}
