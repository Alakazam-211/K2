//! Renderer memory watcher — `proc_pidinfo`-backed RSS sampler.
//!
//! Background: 0.38.x shipped 7 releases of fast iteration, and the
//! Tauri WebView's resident-memory baseline drifted from ~140 MB up
//! to >1 GB during sustained sessions (see C3PO ticket `c9b0d9a9`).
//! macOS RunningBoard reaps the app under Jetsam pressure when this
//! happens. We don't have visibility from inside the WebKit renderer
//! (no Chromium `performance.memory` on WebKit), so the daemon-side
//! mechanism for measuring memory is the Tauri process's own kernel
//! task info via `proc_pidinfo`.
//!
//! The renderer polls this command every 5 minutes via a top-level
//! `<MemoryWatcher />` component:
//!
//! - **Log**: console.info every sample so future memory-leak triage
//!   doesn't require a fresh Jetsam IPS to see the growth curve.
//! - **Warn**: toast notification if RSS crosses 800 MB (the value
//!   at which the reporter observed the WebKit `Strict` policy
//!   escalation kicking in).
//!
//! The 800 MB threshold is empirical; tune as we collect data.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RendererMemoryStatus {
    /// Resident set size in bytes. On macOS this is the
    /// `pti_resident_size` field of `proc_taskinfo` — the live
    /// physical pages backed by this process.
    pub resident_bytes: u64,
    /// Virtual size in bytes (`pti_virtual_size`). Less actionable
    /// than RSS for leak detection but useful for cross-reference
    /// with `lifetimeMax` in JetsamEvent IPS files (which are also
    /// in pages of RSS-like accounting).
    pub virtual_bytes: u64,
    /// The Tauri process PID. Surfaced so the renderer can log a
    /// stable identifier across the polling lifetime.
    pub pid: u32,
}

#[cfg(target_os = "macos")]
fn read_process_memory() -> Result<RendererMemoryStatus, String> {
    use libc::{c_int, c_void};
    // `proc_pidinfo` constants — these aren't always re-exported by
    // the `libc` crate on every target, so define what we need
    // inline. Values are stable kernel ABI on Darwin.
    const PROC_PIDTASKINFO: c_int = 4;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct ProcTaskinfo {
        pti_virtual_size: u64,
        pti_resident_size: u64,
        pti_total_user: u64,
        pti_total_system: u64,
        pti_threads_user: u64,
        pti_threads_system: u64,
        pti_policy: i32,
        pti_faults: i32,
        pti_pageins: i32,
        pti_cow_faults: i32,
        pti_messages_sent: i32,
        pti_messages_received: i32,
        pti_syscalls_mach: i32,
        pti_syscalls_unix: i32,
        pti_csw: i32,
        pti_threadnum: i32,
        pti_numrunning: i32,
        pti_priority: i32,
    }

    extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    let pid = std::process::id() as c_int;
    let mut info = ProcTaskinfo::default();
    let size = std::mem::size_of::<ProcTaskinfo>() as c_int;
    let ret = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if ret != size {
        return Err(format!(
            "proc_pidinfo PROC_PIDTASKINFO returned {ret} (expected {size})"
        ));
    }
    Ok(RendererMemoryStatus {
        resident_bytes: info.pti_resident_size,
        virtual_bytes: info.pti_virtual_size,
        pid: pid as u32,
    })
}

#[cfg(not(target_os = "macos"))]
fn read_process_memory() -> Result<RendererMemoryStatus, String> {
    // Other platforms: return zeros + the PID so the renderer-side
    // logging still has a stable shape; threshold checks won't fire
    // because the value is 0. We can plumb a platform-appropriate
    // call here when K2SO ports to Linux/Windows.
    Ok(RendererMemoryStatus {
        resident_bytes: 0,
        virtual_bytes: 0,
        pid: std::process::id(),
    })
}

/// Returns the Tauri app process's current memory footprint.
/// Polled by the renderer's `MemoryWatcher` every 5 min.
#[tauri::command]
pub fn renderer_memory_status() -> Result<RendererMemoryStatus, String> {
    read_process_memory()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_process_memory_returns_nonzero_rss_on_macos() {
        // On macOS we expect proc_pidinfo to succeed and return a
        // real RSS (cargo test process itself has *some* memory).
        // On other platforms the stub returns 0; both are acceptable
        // — the test just guards against the function panicking.
        let result = read_process_memory().expect("memory read should not error");
        assert!(result.pid > 0, "pid should be positive");
        #[cfg(target_os = "macos")]
        assert!(
            result.resident_bytes > 0,
            "RSS should be nonzero on macOS"
        );
    }
}
