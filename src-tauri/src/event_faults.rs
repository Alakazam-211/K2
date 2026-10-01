//! Field log for macOS events that vendored tao dropped instead of aborting.
//!
//! tao's `TaoApp` / `TaoWindow` `sendEvent:` IMPs are `extern "C"`. An ObjC
//! exception or a Rust panic from AppKit or wry used to abort the whole app
//! with no reason in the crash report (0.41.0, 0.41.4). The K2 patch in
//! `third_party/tao/src/platform_impl/macos/event_fault.rs` now catches it,
//! drops that one event, and calls the hook installed here. This writes one
//! line to `~/.k2/client-event-faults.log` so the next fault names what threw.
//!
//! Everything here runs on the main thread inside `sendEvent:`. It must not
//! panic: no unwraps, and IO errors are ignored. tao also wraps the hook in a
//! catch, so a mistake here costs the record, not the window.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use tao::platform::macos::EventFault;

pub const LOG_FILE_NAME: &str = "client-event-faults.log";
/// Past this size the log moves to `client-event-faults.log.1` (one old copy).
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Install the tao fault hook. Call once at startup.
pub fn install() {
    let _ = tao::platform::macos::set_event_fault_hook(on_fault);
}

fn on_fault(fault: &EventFault<'_>) {
    let line = format_line(
        &chrono::Utc::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
        fault,
    );
    log::error!("{line}");
    if let Some(path) = log_path() {
        append_capped(&path, &line, MAX_LOG_BYTES);
    }
}

/// One K2-side record in the same log (for example a traffic-light observer
/// that caught a fault): `<rfc3339> k2=<version> <what>`. Best-effort.
pub fn record_line(what: &str) {
    let what = what.replace(['\r', '\n'], " ");
    let line = format!(
        "{} k2={} {what}",
        chrono::Utc::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
    );
    log::error!("{line}");
    if let Some(path) = log_path() {
        append_capped(&path, &line, MAX_LOG_BYTES);
    }
}

fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)?;
    Some(home.join(".k2").join(LOG_FILE_NAME))
}

/// One record: `<rfc3339> k2=<version> <tao fault>`. Newlines in the detail
/// are flattened so a record is always one line.
pub fn format_line(timestamp: &str, version: &str, fault: &EventFault<'_>) -> String {
    let fault = fault.to_string().replace(['\r', '\n'], " ");
    format!("{timestamp} k2={version} {fault}")
}

/// Append `line` to `path`. If that would pass `cap` bytes, the current file
/// is first renamed to `<path>.1` (replacing an older `.1`). Best-effort.
pub fn append_capped(path: &Path, line: &str, cap: u64) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let incoming = line.len() as u64 + 1;
    if let Ok(meta) = fs::metadata(path) {
        if meta.len().saturating_add(incoming) > cap {
            let mut rotated = path.as_os_str().to_owned();
            rotated.push(".1");
            let _ = fs::rename(path, PathBuf::from(rotated));
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.write_all(b"\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tao::platform::macos::EventFaultKind;

    fn fault(detail: &str) -> EventFault<'_> {
        EventFault {
            source: "TaoApp",
            event_type: 10,
            window_number: 42,
            kind: EventFaultKind::ObjcException,
            detail,
        }
    }

    #[test]
    fn line_has_time_version_event_window_and_reason() {
        let line = format_line(
            "2026-09-30T13:00:00+00:00",
            "0.41.6",
            &fault("exception <NSException: 0x1> 'NSInternalInconsistencyException' reason: boom\nsecond"),
        );
        assert_eq!(
            line,
            "2026-09-30T13:00:00+00:00 k2=0.41.6 TaoApp dropped NSEvent type=10 window=42 \
             objc-exception: exception <NSException: 0x1> 'NSInternalInconsistencyException' \
             reason: boom second"
        );
    }

    #[test]
    fn one_fault_writes_exactly_one_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".k2").join(LOG_FILE_NAME);
        let line = format_line("2026-09-30T13:00:00+00:00", "0.41.6", &fault("reason: one"));
        append_capped(&path, &line, MAX_LOG_BYTES);
        let body = fs::read_to_string(&path).expect("log written");
        assert_eq!(body, format!("{line}\n"));
        assert_eq!(body.lines().count(), 1);
    }

    #[test]
    fn past_the_cap_the_log_rotates_to_dot_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LOG_FILE_NAME);
        let rotated = dir.path().join(format!("{LOG_FILE_NAME}.1"));
        let cap = 64;
        let old = "x".repeat(60);
        fs::write(&path, format!("{old}\n")).expect("seed log");
        fs::write(&rotated, "older\n").expect("seed old rotation");

        append_capped(&path, "new line", cap);

        assert_eq!(fs::read_to_string(&path).expect("new log"), "new line\n");
        assert_eq!(
            fs::read_to_string(&rotated).expect("rotated log"),
            format!("{old}\n")
        );
    }

    #[test]
    fn under_the_cap_the_log_appends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LOG_FILE_NAME);
        append_capped(&path, "a", 64);
        append_capped(&path, "b", 64);
        assert_eq!(fs::read_to_string(&path).expect("log"), "a\nb\n");
        assert!(!dir.path().join(format!("{LOG_FILE_NAME}.1")).exists());
    }

    #[test]
    fn unwritable_path_is_ignored_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A regular file where the parent directory should be.
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, "").expect("seed blocker");
        append_capped(&blocker.join(LOG_FILE_NAME), "lost", 64);
        assert!(!blocker.join(LOG_FILE_NAME).exists());
    }
}
