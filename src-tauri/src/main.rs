// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
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
