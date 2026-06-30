//! Thin library surface for the k2so-daemon binary.
//!
//! The crate's primary artifact remains the `k2so-daemon` binary
//! (see `src/main.rs`). This lib exists so integration tests in
//! `crates/k2so-daemon/tests/*.rs` can reach internal modules like
//! `sessions_ws` without duplicating the code — the binary's own
//! `mod` declarations are unchanged and sit above `main.rs`.
//!
//! **Important:** `lib.rs` and `main.rs` are TWO INDEPENDENT
//! compilations of the same `src/*.rs` files. `main.rs` declares the
//! whole module tree privately (`mod foo;`); this lib re-declares a
//! curated set as `pub mod foo;` pointing at the same files. Adding a
//! module / type here therefore changes only the test-facing library —
//! it has ZERO effect on the production binary, whose `mod` graph in
//! `main.rs` is the source of truth. (#630 added the full module set +
//! [`DaemonState`]/[`BANNER`] here so the route DISPATCHER —
//! `routes::dispatcher::dispatch` — and every `crate::*` handler it
//! reaches can compile inside the lib, letting the auth-route
//! integration harness drive real HTTP requests through the real
//! dispatch + gate + handler stack. No runtime behavior is added or
//! changed: these are the same source files the binary already builds.)

// ── Full module tree (mirrors `main.rs`'s private `mod` graph) ──────
// Declared `pub` so the route dispatcher's `crate::*` references all
// resolve when `routes` is compiled into the lib for the #630 harness.
pub mod active_reaper;
pub mod session_reaper;
pub mod agents_routes;
pub mod awareness_ws;
pub mod boot_status;
pub mod canonical_session;
pub mod chat_routes;
pub mod claude_auth_host;
pub mod classify_routes;
pub mod cli;
pub mod cli_response;
pub mod clone_routes;
pub mod companion_host;
pub mod companion_routes;
pub mod connect_users_routes;
pub mod db_routes;
pub mod events;
pub mod federation_routes;
pub mod fs_routes;
pub mod grid_emitter;
pub mod git_routes;
pub mod heartbeat_launch;
pub mod heartbeat_routes;
pub mod inbox_routes;
pub mod llm_host;
pub mod llm_routes;
pub mod misc_routes;
pub mod ops_routes;
pub mod ops_stream_ws;
pub mod pending_live;
pub mod pinned_chat;
pub mod project_config_routes;
pub mod providers;
pub mod review_checklist_routes;
pub mod routes;
// COMPAT-58 (#58 Phase 0): scoped per-session hook tokens + per-cell UDS.
// Dormant superset, gated on K2_HOOK_SCOPED (default OFF).
pub mod cell_server;
pub mod cell_uds;
// P4-H5 (sandbox hardening): host nft fail-closed egress allowlist for microVM
// cells (default-DROP `skuid k2cell` + only 443/53). Reached ONLY from the
// microVM spawn path (linux + sandbox-microvm + K2_SANDBOX); no-op otherwise.
pub mod cell_egress;
// P4-H6 (sandbox hardening): per-session worker-uid allocator over a reserved
// high uid range. Each microVM cell's VMM drops to a DISTINCT uid → true
// multi-tenant fs isolation + per-session egress. Reached ONLY from the microVM
// spawn path (linux + sandbox-microvm + K2_SANDBOX); inert otherwise.
pub mod cell_uid_pool;
pub mod session_token;
// P3b (sandbox / K2-as-a-server): per-session stream tokens + the public
// `POST /v1/sandboxes` route + policy-resolver. Behind K2_SANDBOX_API (OFF).
pub mod stream_token;
// P4-H4 (sandbox hardening): per-principal + global concurrent-cell cap (429).
// Reached ONLY via /v1/sandboxes (K2_SANDBOX_API, default OFF).
pub mod sandbox_quota;
pub mod v1_sandboxes;
// F2 (sandbox): per-session agent response log + principal→session ownership map
// backing GET /v1/sandboxes/<id>/messages. Reached ONLY via the K2_SANDBOX_API
// surface (default OFF) + the scoped /cli/respond cell verb.
pub mod sandbox_responses;
pub mod session_events;
pub mod session_events_ws;
pub mod session_lookup;
pub mod sessions_bytes_ws;
pub mod sessions_grid_ws;
pub mod sessions_ws;
pub mod settings_routes;
pub mod signal_format;
pub mod skill_layers_routes;
pub mod skills_routes;
pub mod spawn;
pub mod terminal_event_sink;
pub mod terminal_lifecycle_routes;
pub mod terminal_routes;
pub mod themes_routes;
pub mod triage;
pub mod tunnel_tls_listener;
pub mod update_routes;
pub mod v2_session_map;
pub mod v2_spawn;
pub mod wake_headless;
pub mod watchdog;
pub mod workspace_layouts_dedup;
pub mod workspace_msg;
pub mod workspace_routes;

#[cfg(test)]
mod test_support;

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::broadcast;

use crate::events::{WireEvent, EVENT_CHANNEL_CAP};

/// Build/identity banner. Mirrors `main.rs`'s `BANNER` (referenced by
/// the `/ping` route in the dispatcher). Defined here so the dispatcher
/// compiles inside the lib for the #630 harness.
pub const BANNER: &str = concat!(
    "k2so-daemon ",
    env!("CARGO_PKG_VERSION"),
    " — scaffolding build (tokio)",
);

/// Shared per-process state pulled into every connection task. Cheap to
/// clone: all fields are either `Copy`, `&'static`, or `Arc`-wrapped.
///
/// Mirrors the `DaemonState` defined in `main.rs`. The two are distinct
/// types in distinct crate compilations, but field-for-field identical;
/// the dispatcher references `crate::DaemonState`, which resolves to
/// THIS definition when compiled into the lib.
#[derive(Clone)]
pub struct DaemonState {
    pub token: Arc<String>,
    pub started_at: Instant,
    pub port: u16,
    /// Broadcast channel the daemon's `AgentHookEventSink` publishes into.
    /// Every `/events` WS subscriber takes a `Receiver` off this sender.
    pub event_tx: Arc<broadcast::Sender<WireEvent>>,
    /// Supervisor-agnostic graceful-shutdown trigger (#651). Mirrors the
    /// `main.rs` field. `Some` in the running daemon; a `send(())` here
    /// drives the SAME teardown path SIGINT uses so the supervisor (launchd
    /// `KeepAlive` / systemd `Restart=always`) respawns the process. `None`
    /// in the test harness so `POST /cli/daemon/restart` can be asserted
    /// (200 + would-restart) WITHOUT firing a real restart.
    pub shutdown_tx: Option<broadcast::Sender<()>>,
}

// ── #630 auth-route integration harness ─────────────────────────────
//
// Compiled ONLY into the test-facing lib (never the binary). Lets
// `crates/k2so-daemon/tests/auth_routes_integration.rs` spin a real
// in-process daemon listener and drive REAL HTTP requests through
// `routes::dispatcher::dispatch` so the full dispatch + auth-gate +
// handler stack is exercised end-to-end.
pub mod test_harness {
    use super::*;
    use std::net::TcpListener as StdTcpListener;
    use tokio::net::TcpListener;

    /// A running in-process daemon for tests. Holds the bound port +
    /// owner token; the accept loop runs on the ambient tokio runtime.
    pub struct TestDaemon {
        /// Owner (host) daemon token — passes `token_is_owner`.
        pub owner_token: String,
        /// The ephemeral 127.0.0.1 port the listener bound.
        pub port: u16,
    }

    /// Bind an ephemeral `127.0.0.1:0` listener, mark boot-status
    /// `ready` (so the dispatcher's readiness gate lets real routes
    /// through), and spawn the accept loop calling the REAL
    /// [`crate::routes::dispatcher::dispatch`]. Returns the chosen port
    /// and the owner token. Must be called from within a tokio runtime
    /// (e.g. `#[tokio::test]`).
    ///
    /// The seeded connect-user store is the caller's responsibility —
    /// seed via `k2_core::connect_users::{add_user,set_role,
    /// create_session}` after pointing `$HOME` at a tempdir.
    pub async fn start(owner_token: impl Into<String>) -> TestDaemon {
        let owner_token = owner_token.into();

        // Bind synchronously on an ephemeral port so we can read the
        // actual port before handing the socket to tokio.
        let std_listener =
            StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral 127.0.0.1 port");
        let port = std_listener.local_addr().expect("local_addr").port();
        std_listener
            .set_nonblocking(true)
            .expect("set_nonblocking");
        let listener = TcpListener::from_std(std_listener).expect("tokio adopt listener");

        // The dispatcher 503s every real route until boot-status is
        // `ready`; flip it so auth tests see real status codes.
        crate::boot_status::set_ready();

        let (event_tx, _rx) = broadcast::channel::<WireEvent>(EVENT_CHANNEL_CAP);
        let event_tx = Arc::new(event_tx);

        let state = DaemonState {
            token: Arc::new(owner_token.clone()),
            started_at: Instant::now(),
            port,
            event_tx,
            // #651: the test harness must NEVER fire a real restart. `None`
            // makes the `POST /cli/daemon/restart` handler return its 200
            // "would-restart" response and SKIP the live shutdown trigger,
            // so gate + happy-path can be asserted without killing the test
            // process.
            shutdown_tx: None,
        };

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let st = state.clone();
                        tokio::spawn(async move {
                            crate::routes::dispatcher::dispatch(stream, st).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        TestDaemon { owner_token, port }
    }
}
