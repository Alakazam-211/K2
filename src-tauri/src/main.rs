// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn is_version_arg(args: &[std::ffi::OsString]) -> bool {
    args.len() == 2 && args[1] == std::ffi::OsStr::new("--version")
}

fn main() {
    // `--version` is also the signed-build AMFI smoke check. Exit before
    // Tauri startup so this probe can never install or reload launchd agents.
    let args: Vec<_> = std::env::args_os().collect();
    if is_version_arg(&args) {
        println!("K2 {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // 0.37.9 — raise RLIMIT_NOFILE so the Tauri process can hold
    // enough fds for many concurrent PTYs / WS sockets / watchers.
    // launchd-launched apps inherit a 256/1024 soft limit by default,
    // which saturates quickly when users open 10+ terminal panes.
    // No-op if already at the hard limit. Must run before anything
    // else opens fds.
    #[cfg(unix)]
    k2_core::raise_nofile_limit();

    // Phase 2 Unit 2 — `--llm-worker` arm moved to k2so-daemon. The
    // daemon now spawns itself as `k2so-daemon --llm-worker <payload>`
    // to run inference in an isolated child process. Tauri is no
    // longer involved in the LLM lifecycle; the renderer calls
    // `/cli/llm/*` on the daemon directly.

    // Fire the reqwest pool warmup IMMEDIATELY — before Tauri even
    // starts parsing the window config. reqwest::blocking's tokio
    // runtime takes ~500-800ms to materialize on first send(). By
    // spawning this thread at the very top of main(), it has a
    // head start on daemon startup + window hydration. Restored
    // terminals that spawn during React rehydration then hit an
    // already-warm pool instead of paying 600ms of first-call cost.
    k2_lib::warm_http_pool_async();

    k2_lib::run()
}

#[cfg(test)]
mod tests {
    use super::is_version_arg;
    use std::ffi::OsString;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn version_flag_is_exact() {
        assert!(is_version_arg(&args(&["k2", "--version"])));
        assert!(!is_version_arg(&args(&["k2", "version"])));
        assert!(!is_version_arg(&args(&[
            "k2", "--version", "extra",
        ])));
        assert!(!is_version_arg(&args(&["k2"])));
    }
}
