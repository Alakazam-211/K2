//! The screen marker scan for titleless CLIs (DA28 / A13, Q12).
//!
//! Hermes and cursor-agent set no terminal title and ring no bell, so
//! with no hook and no transcript the daemon reads the bottom of their
//! grid once a second for the harness's own **interrupt marker**: the
//! hint a TUI shows only while a turn runs. Never prose: `waiting for`,
//! `thinking...` and friends also appear in ordinary answers, which is
//! why the renderer's phrase scan (deleted in S5) lit "working" on chat
//! text.
//!
//! A marker on screen is working evidence (`evidence: screen`). Idle
//! comes from the marker being gone for [`MARKER_GONE_MS`], or the bell.

/// How often a session's grid is read.
pub const SCAN_EVERY_MS: i64 = 1_000;

/// A marker absent this long ends the working state it started.
pub const MARKER_GONE_MS: i64 = 3_000;

/// Rows searched, counted up from the last non-blank row of the grid (a
/// TUI's footer sits at the bottom; a short screen's at its last line).
pub const SCAN_WINDOW_ROWS: usize = 15;

/// Per-harness interrupt markers (lower-case), from the 2026-07 TUI
/// signal study (the list `lib/agent-signals.ts` carried for these two).
pub fn markers_for(harness: &str) -> &'static [&'static str] {
    match harness {
        // Busy footer "msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel".
        "hermes" => &["msg=interrupt"],
        // Mid-turn input bar.
        "cursor" | "cursor-agent" => &["ctrl+c to stop"],
        _ => &[],
    }
}

/// Is any of `markers` in the last [`SCAN_WINDOW_ROWS`] non-blank-ended
/// rows of `rows`?
pub fn rows_show_marker(markers: &[&str], rows: &[String]) -> bool {
    if markers.is_empty() {
        return false;
    }
    let end = rows.iter().rposition(|r| !r.trim().is_empty()).map_or(0, |i| i + 1);
    let from = end.saturating_sub(SCAN_WINDOW_ROWS);
    rows[from..end].iter().any(|row| {
        let lower = row.to_lowercase();
        markers.iter().any(|m| lower.contains(m))
    })
}

/// One session's scan state.
#[derive(Debug, Clone, Default)]
pub struct ScreenScan {
    asserted: bool,
    last_seen: Option<i64>,
}

impl ScreenScan {
    /// One scan at `now`. `Some(true)`: the marker is on screen (working
    /// evidence, every scan it is seen). `Some(false)`: it has been gone
    /// [`MARKER_GONE_MS`] after asserting working. `None`: nothing to say.
    pub fn observe(&mut self, present: bool, now: i64) -> Option<bool> {
        if present {
            self.asserted = true;
            self.last_seen = Some(now);
            return Some(true);
        }
        if self.asserted && self.last_seen.is_some_and(|t| now - t >= MARKER_GONE_MS) {
            self.asserted = false;
            return Some(false);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(lines: &[&str]) -> Vec<String> {
        let mut rows = vec![String::new(); 30];
        let n = rows.len();
        for (i, l) in lines.iter().enumerate() {
            rows[n - lines.len() + i] = l.to_string();
        }
        rows
    }

    /// T-S3h (pure): a marker is working; prose is not; the marker gone
    /// 3 s is idle.
    #[test]
    fn markers_not_prose() {
        let hermes = markers_for("hermes");
        let cursor = markers_for("cursor");
        assert!(rows_show_marker(hermes, &screen(&["⠋ pondering (｡•́︿•̀｡)", "msg=interrupt · /queue · Ctrl+C cancel"])));
        assert!(rows_show_marker(cursor, &screen(&["  Ctrl+C to stop"])));
        // Prose the renderer's phrase list used to match.
        for prose in ["I'm waiting for the build to finish.", "Thinking... about it", "working..."] {
            assert!(!rows_show_marker(hermes, &screen(&[prose])), "{prose}");
            assert!(!rows_show_marker(cursor, &screen(&[prose])), "{prose}");
        }
        // Claude, Codex, Grok have titles; a shell has nothing to scan.
        for h in ["claude", "codex", "grok", "shell", ""] {
            assert!(markers_for(h).is_empty(), "{h}");
        }
        // Only the bottom window counts (older output above it is history).
        let mut rows = screen(&[]);
        rows[0] = "msg=interrupt".into();
        assert!(rows_show_marker(hermes, &rows), "a short screen's last line is its bottom");
        for (i, row) in rows.iter_mut().enumerate().skip(1) {
            *row = format!("later output {i}");
        }
        assert!(!rows_show_marker(hermes, &rows));
    }

    #[test]
    fn marker_gone_three_seconds_is_idle() {
        let mut s = ScreenScan::default();
        assert_eq!(s.observe(false, 0), None, "nothing asserted yet");
        assert_eq!(s.observe(true, 1_000), Some(true));
        assert_eq!(s.observe(true, 2_000), Some(true));
        assert_eq!(s.observe(false, 3_000), None);
        assert_eq!(s.observe(false, 4_999), None);
        assert_eq!(s.observe(false, 5_000), Some(false));
        assert_eq!(s.observe(false, 9_000), None, "idle is said once");
    }
}
