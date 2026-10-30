//! The k2-daemon library: the whole daemon.
//!
//! `src/main.rs` is a two-line shim that calls [`daemon_main::run`]. Since
//! 0.45.1 there is ONE module graph (it used to be compiled twice: privately
//! by `main.rs` and publicly here), so every static exists once and every
//! unit test runs once. Integration tests in `tests/*.rs` reach modules
//! through this library, and [`test_harness`] runs the real dispatcher
//! in-process.

// ── Full module tree (mirrors `main.rs`'s private `mod` graph) ──────
// Declared `pub` so the route dispatcher's `crate::*` references all
// resolve when `routes` is compiled into the lib for the #630 harness.
pub mod action_cards;
pub mod active_reaper;
pub mod session_reaper;
// `k2 agent harness`: move a workspace's pinned chat to another harness (0.46.6).
pub mod agent_harness;
pub mod agents_routes;
pub mod agent_name_repair_migrate;
pub mod agent_retire;
pub mod awareness_ws;
pub mod boot_status;
pub mod ptyd_status;
pub mod ptyd_adopt;
pub mod daemon_stop;
pub mod keep_alive;
pub mod session_liveness;
pub mod browser_routes;
pub mod canonical_session;
pub mod chat_routes;
pub mod sandbox_chat_routes;
pub mod sandbox_reaper;
pub mod v1_ws_message;
pub mod claude_auth_host;
pub mod subscription_usage;
pub mod classify_routes;
pub mod cli;
pub mod cli_response;
pub mod clone_routes;
pub mod clone_vault;
pub mod clone_gate;
pub mod clone_preview;
pub mod clone_proposal_routes;
pub mod thread_transfer_routes;
pub mod companion_host;
pub mod companion_routes;
pub mod connect_users_routes;
// phone-web-off (2026-10-24): Companion pairing codes + device sessions.
pub mod connect_device_routes;
// phone-web-off: the /pair landing page + the iOS app-site-association file.
pub mod pair_page;
pub mod login_throttle;
pub mod db_routes;
pub mod events;
pub mod federation_address;
pub mod federation_drain;
// Sew S27 grant hook: thin mgmt UDS client + the outbox delivery worker.
pub mod sew_mgmt;
pub mod tool_hook_worker;
pub mod feedback_routes;
pub mod federation_rosters;
pub mod federation_routes;
pub mod federation_vault;
pub mod fs_routes;
/// prd-app-caps-0464-v1 AC16/AC17 (0.46.4): `files.downloads` / `files.download`.
pub mod fs_download;
pub mod fs_live;
pub mod charter_compose_watch;
/// The home database switch, daemon side (prd-k2-db-rename-v1): the
/// refusal card, `GET /cli/db-home`, `POST /cli/db-home/keep`.
pub mod db_home_routes;
/// Folder-trust gate, daemon side (security sweep SS9–SS11): the card,
/// the spawn gate, the 409, `/cli/folder-trust/*`, boot.
pub mod folder_trust;
/// Re-scan folders with live K2 sessions when their risky files change
/// (U0: Claude reloads hooks mid-session).
pub mod folder_trust_watch;
pub mod notify_bound;
// Zen Mode v1 (prd-zen-mode-v1 S1): `/cli/zen/*` + the `~/.k2/zen` watcher.
pub mod zen_lib_bundled;
pub mod zen_lib_routes;
pub mod zen_downloads_routes;
pub mod zen_links_routes;
pub mod zen_remote_routes;
pub mod zen_routes;
pub mod zen_sync_routes;
pub mod zen_watch;
pub mod grid_emitter;
pub mod git_routes;
/// First run without git (0.46.4): `/cli/git-tools*` + the headless watch loop.
pub mod git_tools_routes;
/// Changes tab v1.1 (C5): review notes routes (store in k2_core::git_notes).
pub mod git_notes_routes;
/// Changes tab v1: debounced `git_changed` events (watcher + git routes).
pub mod git_live;
pub mod heartbeat_app_routes;
// 0.45.5 app identity surface: `/cli/agent/identity` + `/cli/agent/handle`.
pub mod agent_identity_routes;
pub mod heartbeat_launch;
pub mod heartbeat_monitor;
// 0.46.6: pause / resume another workspace's heartbeat (people; agents ask).
pub mod heartbeat_other;
pub mod heartbeat_routes;
// `k2 job`: daemon-run scheduled jobs (prd-vault-gaps-v1 item 5).
pub mod jobs;
pub mod hire_routes;
pub mod heartbeat_wait;
pub mod home_avatar_routes;
// Desktop mirrors (0.46.4): this computer's app forwards its former
// in-process k2-core commands here; owner token only.
pub mod desktop_mirror_routes;
// Local Grok Bots (0.45.7): `/cli/grokbots/*` + the change poller.
pub mod grokbots_routes;
pub mod grokbots_watch;
pub mod power;
pub mod inbox_routes;
pub mod wiki_routes;
pub mod llm_host;
pub mod llm_routes;
// K2 Mail foundation (prd-email-server-v1) — Stalwart-sidecar mail
// family: supervisor/jmap/doctor skeletons + per-concern `/cli/mail/*`
// handlers, dispatched by the thin `mail_routes` shim. Linux-only at
// RUNTIME (supervisor::mail_supported); compiles + tests everywhere.
pub mod mail;
pub mod mail_routes;
pub mod root_helper;
pub mod root_helper_routes;
// Workspace data sidecar (prd-workspace-data-sidecar-v1) — supervised
// Postgres + JSONB store. Linux-only at RUNTIME (`sql_supported`).
pub mod sew;
pub mod sql;
pub mod sql_routes;
pub mod v1_db;
// DNS K1 — control-plane proxy + principal-bound `/cli/dns/*` routes
// (local capability envelope + tunnel-token proxy to k2-dev-web).
pub mod dns;
pub mod dns_routes;
// Custom-domain inventory + certs (prd-custom-domains-and-certs-v1).
pub mod domains;
pub mod domain_routes;
pub mod connect_proxy;
pub mod misc_routes;
pub mod token_usage_scan;
pub mod usage_routes;
/// Usage v2 limits: live feeds (statusLine, Codex session logs) and the every-login document.
pub mod usage_live;
pub mod ops_routes;
pub mod ops_stream_ws;
pub mod pending_live;
// Permission cards (prd-agent-settings-cards-v1, 0.46.3 Beta): P0 stubs per
// lane; nothing is wired, so with the switches off K2 is unchanged.
mod permission;
pub mod pinned_chat;
// S1 (presence/multiplayer arc): process-wide connected-users registry +
// roster broadcast. Registered from the `/cli/sessions/events` WS handler.
pub mod presence;
pub mod project_config_routes;
pub mod project_group_routes;
pub mod providers;
// AR6: one owner per nested URL label (claim/point/run/transfer rule).
pub mod label_owner;
pub mod publish_routes;
pub mod publish_runtime;
// Companion C4 (prd-companion-v2 §4) — `/cli/push/*` device
// registration + the mobile-push dispatch triggers (dormant until a
// gateway URL + token is configured).
pub mod push_routes;
// Thread push prefs routes (prd-thread-push-notifications-v1 S2).
pub mod notify_routes;
/// homes-sync-v1 (0.46.1): `/cli/me/homes*`, a person's Homes between devices.
pub mod homes_routes;
// Thread push trigger + per-(Thread, person) coalescer (S3).
pub mod thread_push;
// push-content-available-v1 (0.46.7): stale hints for Thread changes that never alert.
pub mod stale_hint;
pub mod thread_attachments;
pub mod ticket_attachments;
pub mod guest_paths;
// `viewing` frames from the overlay WS: who looks at which Thread (S4).
pub mod thread_viewers;
// thread-read-v1 (0.46.1): unread/mute routes, live read frames, and the
// silent badge-correction push.
pub mod read_routes;
pub mod read_live;
pub mod badge_push;
pub mod review_checklist_routes;
pub mod routes;
// K2 Cloud S2 — boot-time consume-once ~/.k2/seed-users.json. Exposed
// in the lib so the provision_users integration suite can drive the
// SAME module function main.rs's boot sequence calls.
pub mod seed_users;
// COMPAT-58 (#58 Phase 1 / PR-A): scoped per-session hook tokens + per-cell UDS
// (default ON; opt out with K2_HOOK_SCOPED=0). Owner never enters agent env.
pub mod cell_identity;
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
// Wave 0: caller-asserted workspace must never be identity (mail/DNS grants).
pub mod caller_workspace;
// C2 (0.40.45): connection-required local agent msg/read/inbox.
pub mod comms;
// P3b (sandbox / K2-as-a-server): per-session stream tokens + the public
// `POST /v1/sandboxes` route + policy-resolver. Behind K2_SANDBOX_API (OFF).
pub mod stream_token;
// P4-H4 (sandbox hardening): per-principal + global concurrent-cell cap (429).
// Reached ONLY via /v1/sandboxes (K2_SANDBOX_API, default OFF).
pub mod sandbox_quota;
// Durable host-session spawn queue (prd-host-session-spawn-queue-v1).
pub mod spawn_queue;
pub mod v1_sandboxes;
// F1 (prd-v1-api-completion §3): NON-SANDBOXED host sessions on /v1 —
// spawn/list/message/read a real session in a granted workspace, no microVM.
// Behind the /v1 surface gate (K2_API, default OFF); honestly labeled
// `"sandbox":"none"`.
pub mod v1_host_sessions;
// Julie 2–4: POST /v1/w hire + wiki notes + context layers (host-sessions cap).
pub mod v1_hire;
pub mod v1_capabilities;
// Sew S28 / K6: chat-pass keyring (vault), /v1/chat/jwks, rotation tick.
pub mod chat_pass;
// Shared ES256 helpers (capability key + chat-pass keys).
pub mod es256;
// F2 (sandbox): per-session agent response log + principal→session ownership map
// backing GET /v1/sandboxes/<id>/messages. Reached ONLY via the K2_SANDBOX_API
// surface (default OFF) + the scoped /cli/respond cell verb.
pub mod sandbox_responses;
pub mod session_activity;
// prd-daemon-activity-and-thread-working-v1 S1: `POST /hook/event` ingest +
// owner check, and the daemon-owned hook installer.
pub mod hook_ingest;
pub mod activity_store;
pub mod activity_transcript;
// prd-daemon-activity-and-thread-working-v1 S4: activity_changed + snapshot.
pub mod activity_events;
pub mod activity_routes;
// S6: the Thread working strip (turns, ephemeral `activity` frames).
pub mod thread_activity;
/// N1 (prd-app-caps-0464-v1 AC29, 0.46.4): the catch-up GET's per-caller limit.
pub mod thread_activity_limit;
pub mod hook_install;
pub mod session_events;
pub mod session_events_ws;
// inbox-stream-v1 (0.46.7): the phone inbox stream on /cli/sessions/events.
pub mod inbox_stream;
pub mod session_author;
pub mod session_lookup;
pub mod sessions_bytes_ws;
pub mod sessions_grid_ws;
pub mod sessions_ws;
pub mod settings_routes;
pub mod sidecar_routes;
pub mod ticket_settings_routes;
// K2 compute nodes (prd-k2-compute-nodes-v1): routes + the live engine.
pub mod compute_admin;
pub mod compute_advertise;
pub mod compute_local;
// "Allow managing compute nodes" (`compute_manage`, 0194).
pub mod compute_manage;
pub mod compute_place;
// `.k2/compute.toml` defaults and presets (prd-compute-easy-mode-0467 CE-3).
pub mod compute_preset;
pub mod compute_secrets;
pub mod compute_routes;
// The job and node views (`nodes`, `status`, `queue`, `jobs`, `get`, `cancel`).
pub mod compute_views;
/// App runtime stage 1 (prd-k2-app-runtime-v1): `/cli/app/*` routes.
pub mod apps;
pub mod compute_shards;
pub mod compute_tenant_guard;
pub mod compute_ws;
// K2 vault (prd-k2-vault-v1): daemon glue + `/cli/vault/*`.
pub mod vault;
pub mod vault_handoff_routes;
pub mod vault_routes;
pub mod vault_use_routes;
pub mod vault_systemd_routes;
// Box moves carry the vault (prd-box-move-carry-v1): `/cli/migrate/secrets/*`.
pub mod migrate_secrets_routes;
// Garden v3 data sources (prd-garden-v3-external-data-cards-v1): the poller,
// trigger wakes, `/cli/sources/*`, and the shared sleep/wake detector.
pub mod sources;
pub mod sources_routes;
// Sew chat for agents (`k2 chat`, S30 core): `/cli/sew/chat/*`, the chat
// approval queue `/cli/sew/approvals*`, and the `SewChat` seam to Sew.
pub mod sew_chat;
pub mod wall_jump;
// Garden data (prd-gardens-local-services-v1 1A, contract-garden-data-v1):
// widget keyspaces, grants, the widget door `/cli/zen/call` and the agent
// door `/cli/zen/data/*`.
pub mod garden_data;
// Garden access (prd-garden-access-v1 A1): caller → History actor, the G8
// delete hook and the daily History sweep. The store is k2-core's.
pub mod garden_access;
pub mod gardens_manage;
pub mod zen_agent_routes;
pub mod zen_agents_order_routes;
pub mod zen_data_routes;
// Garden access (prd-garden-access-v1): the list routes and gate (A2) and
// K2's own garden.access / garden.delete cards (A4).
pub mod zen_garden_cards;
pub mod zen_access_routes;
// 0.46.5 S0: K2's zen.sign card (a widget version whose manifest changed).
pub mod zen_sign_cards;
// Garden History (A6, G29): edits outside K2 and the History merge.
pub mod zen_history;
pub mod llm_account_for;
pub mod llm_accounts_routes;
/// Mobile dev (prd-mobile-dev-v1, 0.46.6): `/cli/sim/*`.
pub mod sim_routes;
/// Mobile dev S3: `/cli/sim/stream` (JPEG frames + people's input).
pub mod sim_stream_ws;
pub mod llm_accounts_runtime;
pub mod person_wallet_routes;
pub mod person_wallet_runtime;
pub mod remote_session_routes;
pub mod remote_session_sessions;
pub mod signal_format;
pub mod skill_layers_routes;
pub mod skills_routes;
// Context management stack — optional AGENTS.md layer stack (prd-context-hamburger-v1).
pub mod context_routes;
pub mod spawn;
pub mod terminal_event_sink;
pub mod terminal_lifecycle_routes;
pub mod terminal_routes;
pub mod themes_routes;
pub mod triage;
pub mod tunnel_tls_listener;
// 0.46.3: the real client IP for tunnelled Apps (PROXY v2 from frpc).
pub mod client_ip;
pub mod tunnel_autostart;
pub mod tunnel_ingress_listener;
// Runtime stage H: front-door hand-off (no-downtime upgrades).
pub mod front443;
// 0.46.6: the public-ports view and the owner's firewall options.
pub mod firewall;
pub mod handoff;
pub mod handoff_setup;
pub mod tunnel_identity_routes;
pub mod upstream_dial;
// 0.46.3: tunnel splice provenance + proxy-target policy.
pub mod splice_guard;
pub mod update_routes;
pub mod v2_session_map;
pub mod v2_spawn;
// K2 vault V4 (prd-k2-vault-v1 KV18, VS5/VS6): the single outbound gate and
// the fetch engine built on it.
pub mod egress;
pub mod vault_fetch;
pub mod wake_headless;
pub mod watchdog;
pub mod workspace_layouts_dedup;
pub mod workspace_msg;
pub mod workspace_resources_migrate;
pub mod workspace_resources_routes;
pub mod workspace_routes;
pub mod overlay_routes;
pub mod overlay_ws;
pub mod room_stream_ws;
pub mod fs_events_ws;
pub mod activity_events_ws;
pub mod skin_hydra;
pub mod skin_routes;
// Room-scope R6: `GET /cli/skin/me` (lane 4, 0.46.1).
pub mod skin_me_routes;
pub mod app_mobile_routes;
pub mod app_profile_routes;
// Push notifications for custom Apps (prd-custom-app-push-v1, 0.46.4).
pub mod app_push;
// Phone pairing for custom Apps (prd-app-phone-pairing-v1, 0.46.5).
pub mod app_pairing;
// Email Notifications (prd-email-notifications-core-v1, 0.46.5): K2's
// one-way mail from senders no agent can read, the App `email.send` door.
pub mod email_notify;
// agent-app-users (0.46.4): invites, owner reset links, App invite settings.
pub mod app_user_invites;
pub mod chat_channel_routes;
pub mod chat_map_routes;
pub mod guest_chat_routes;
pub mod guest_rooms_routes;
pub mod skin_gateway;
// The daemon's Tokio runtime for plain std threads (wakes off the runtime).
pub mod rt;
// The boot sequence (was the body of `main.rs`).
pub mod daemon_main;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod codex_no_daemon_tests;

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::broadcast;

use crate::events::{WireEvent, EVENT_CHANNEL_CAP};

/// Build/identity banner (logged at boot, served by `/ping`).
pub const BANNER: &str = concat!(
    "k2-daemon ",
    env!("CARGO_PKG_VERSION"),
    " (tokio)",
);

/// Shared per-process state pulled into every connection task. Cheap to
/// clone: all fields are either `Copy`, `&'static`, or `Arc`-wrapped.
///
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
    /// The login limiter (per-IP buckets + LM4 anonymous-failure
    /// ceiling) of THIS daemon. One per process in production, shared by
    /// the main and tunnel-ingress listeners; one per in-process test
    /// daemon, so tests never share limiter state.
    pub login_limiter: Arc<crate::login_throttle::LoginLimiter>,
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
        /// The ephemeral port the listener bound.
        pub port: u16,
        /// Bound socket address (loopback or `0.0.0.0`).
        pub local_addr: std::net::SocketAddr,
        /// PRD connect-login-edge-only: a per-harness TUNNEL-INGRESS
        /// listener (`tunnel_ingress_listener::spawn`) running the same
        /// dispatcher tagged `Ingress::Tunnel`. Connect here to exercise
        /// the tunnel gate exactly as the E2E splice / frpc would.
        pub tunnel_port: u16,
        /// This daemon's own login limiter (both listeners share it).
        pub login_limiter: Arc<crate::login_throttle::LoginLimiter>,
        /// This daemon's main port in the splice guard's registry (0.46.3);
        /// unregistered when the harness is dropped.
        _daemon_port_reg: crate::splice_guard::DaemonPortReg,
    }

    /// A guest who chose `password` themselves (the person's own path,
    /// must-change off), for tests that need a guest who simply signs in.
    /// 0.46.6: a password the Owner, an Admin, an agent or K2 sets on the
    /// users routes asks for a new one at the next sign-in (a 403 with a
    /// change token, never a pass), so setup that only wants a signed-in
    /// guest uses this. Tests about that set path call the route and
    /// assert the 403. Panics on a store error.
    pub fn guest_with_own_password(username: &str, password: &str) {
        let p = k2_core::skin::set_principal_password(username, Some(password))
            .unwrap_or_else(|e| panic!("guest_with_own_password {username}: {e}"));
        assert!(!k2_core::skin::must_change::is_flagged(&p.id).expect("flag read"), "{username}: must-change stays off");
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
        start_on(owner_token, "127.0.0.1:0").await
    }

    /// Like [`start`] but binds `addr` (e.g. `0.0.0.0:0` for LAN tests).
    /// `TestDaemon.local_addr` is the actual bound address — LAN tests
    /// must assert it is unspecified, not merely that 127.0.0.1 connects.
    pub async fn start_on(owner_token: impl Into<String>, addr: &str) -> TestDaemon {
        let owner_token = owner_token.into();
        // As at daemon boot: std-thread wakes run in this test's runtime.
        crate::rt::install(tokio::runtime::Handle::current());

        let std_listener = StdTcpListener::bind(addr).unwrap_or_else(|e| {
            panic!("bind {addr}: {e}");
        });
        let local_addr = std_listener.local_addr().expect("local_addr");
        let port = local_addr.port();
        crate::publish_runtime::set_loopback_port(port);
        // Windows has no per-cell socket: a K2 terminal reaches the daemon
        // only over TCP, through the K2_PORT its spawn env carries (the
        // daemon sets this at boot). Unix terminals use the cell socket.
        #[cfg(windows)]
        k2_core::hook_config::set_port(port);
        let daemon_port_reg = crate::splice_guard::register_daemon_port(port);
        k2_core::listen::set_lan_bound(local_addr.ip().is_unspecified());
        std_listener
            .set_nonblocking(true)
            .expect("set_nonblocking");
        let listener = TcpListener::from_std(std_listener).expect("tokio adopt listener");

        // The activity store consumes the hook plane in main.rs; run it
        // here too so `/hook/*` reaches rows and the compat events.
        crate::activity_store::spawn();
        crate::activity_events::spawn();
        crate::thread_activity::spawn();
        crate::overlay_routes::spawn_card_expiry();
        crate::overlay_ws::install_move_listener();
        // A spawn that finishes waiting git steps emits ProjectsChanged.
        crate::git_tools_routes::install_listener();
        crate::read_routes::boot();

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
            // Its own limiter: no other test daemon in this process can
            // trip it, and the test can drive it through `TestDaemon`.
            login_limiter: Arc::new(crate::login_throttle::LoginLimiter::new()),
        };
        let login_limiter = state.login_limiter.clone();

        // Per-harness tunnel-ingress listener with THIS state (never the
        // process-wide record — each test has its own owner token).
        let tunnel_port = crate::tunnel_ingress_listener::spawn(state.clone())
            .await
            .expect("spawn tunnel-ingress listener");
        // App gateway links (runtime S14) dispatch with this harness's state.
        crate::apps::gateway::install(state.clone());

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let st = state.clone();
                        // I4: same peer classification as `main.rs`.
                        let ingress = crate::routes::dispatcher::Ingress::from_peer(peer);
                        tokio::spawn(async move {
                            crate::routes::dispatcher::dispatch(stream, st, ingress).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        TestDaemon {
            owner_token,
            port,
            local_addr,
            tunnel_port,
            login_limiter,
            _daemon_port_reg: daemon_port_reg,
        }
    }
}
