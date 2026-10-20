//! K2SO core library.
//!
//! Home of the device-local runtime that was previously embedded inside
//! `src-tauri/src/`: SQLite database, llama-cpp LLM integration, Alacritty
//! terminal backend, companion WebSocket server, heartbeat scheduler,
//! agent-lifecycle HTTP hooks, and the pluggable `PushTarget` interface for
//! notification delivery.
//!
//! Both the `k2so-daemon` binary and the `src-tauri` Tauri app link this
//! crate so the core logic executes in exactly one place — the daemon —
//! while the Tauri app stays a thin client that proxies state-mutating
//! commands over HTTP.
//!
//! Module migration from src-tauri lands incrementally.

/// Safe `eprintln!` replacement that silently ignores stderr write
/// failures. When K2SO is launched from Finder there's no tty attached and
/// the default `eprintln!` panics on broken-pipe, which then cascades into
/// abort(). This macro swallows the write result instead.
///
/// `#[macro_export]` so both k2so-core-internal modules and downstream
/// crates (src-tauri, k2so-daemon) can share one definition.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use std::io::Write;
        // PRD connect-login-edge-only L4: every daemon log line carries an
        // RFC3339 UTC timestamp prefix so audit/forensics can correlate
        // (nothing parses these lines byte-exactly — see that PRD).
        let _ = writeln!(
            std::io::stderr(),
            "{} {}",
            $crate::log_timestamp(),
            format_args!($($arg)*)
        );
    }};
}

/// RFC3339 UTC timestamp (millisecond precision, `Z`) for [`log_debug!`].
pub fn log_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub mod active;
// S17: agent sessions in their own memory budget inside the daemon's unit.
pub mod agents_leaf;
// prd-daemon-activity-and-thread-working-v1 S2: the per-session activity
// row and state machine (pure; the daemon's `activity_store` owns rows).
pub mod activity;
pub mod agent_hooks;
// Phase 2.5e: `agents` module retired. All historical homes for
// agent-scoped code redistributed across `workspace/`, `skills/`,
// `heartbeats/`, `deprecated/`, `migrations/`, top-level
// `connections.rs`, etc. The directory + module are deleted as of
// Phase 2.5e; downstream callers (daemon CLI + Tauri commands) use
// the canonical post-relocation paths directly.
/// The 0.46.0 App grace release: App keys, held requests, config
/// classifier, per-service warnings (scan), owner-token uses.
pub mod app_grace;
/// The per-call App audit (`~/.k2/apps/app-audit.db`, runtime S19).
pub mod app_audit;
pub mod app_settings;
/// Private memory-backed runtime dir (rendered frpc config; K2 vault §10.4).
pub mod runtime_dir;
/// Redaction of known secret values in K2-stored text (K2 vault KV33).
pub mod redaction;
/// K2 vault references for LLM keys kept in the main DB (0148).
pub mod secret_refs;
pub mod chat_history;
pub mod chat_continue;
/// Host token ledger (`~/.k2/usage/tokens.sqlite`). Not the app database.
pub mod token_usage;
/// Estimated API-equivalent cost for the ledger (LiteLLM table, MIT).
pub mod usage_prices;
pub mod chat_user_archive;
// K2 Connect "Clone to" — the pure bundle engine (inventory the three
// state locations, scrub secrets, exclude bulk, tar/gz a manifest-driven
// bundle). Powers both the high-bar push (P2) and the README fallback
// (P3). HTTP/Tauri-free; see module docs + PRD k2-connect-clone-to.md.
pub mod cli_stage;
/// Pre-trust Claude/Codex/Grok/Gemini folders on workspace add + harness spawn.
pub mod cli_folder_trust;
pub mod clone;
// Agent-proposed clones held for a person (prd-clone-carries-threads-and-vault-v1 §4.4).
pub mod clone_proposals;
pub mod companion;
// K2 compute nodes, controller side (prd-k2-compute-nodes-v1).
pub mod compute;
// Phase 2.5e: workspace-to-workspace connections registry (relocated
// from `agents/connections.rs`). Powers `k2so connections list/add/
// remove`. Not workspace-scoped — operates on `workspace_relations`
// linking two projects together.
pub mod connections;
/// K2's verb catalog (prd-zen-user-widgets-v2 UWA1, UWB29).
pub mod contract;
// K2SO #617: connect-user accounts (username+password, owner-provisioned)
// + in-memory login sessions. The auth boundary for the PUBLIC K2 Connect
// tunnel surface. See module docs.
pub mod connect_users;
// phone-web-off (2026-10-24): one-time codes that pair a Companion phone
// to a Connect login (a device session that passes the web-access wall).
pub mod connect_device_pairing;
// phone-web-off: the pairing-link QR matrix (byte mode, level M).
pub mod qr;
// PRD connect-login-edge-only: append-only auth audit log
// (`~/.k2/auth-audit.jsonl`) + Ed25519 edge attestation verify.
pub mod auth_audit;
pub mod edge_attest;
// P3a (sandbox / K2-as-a-server): the first-class, owner-minted, revocable
// API-key auth tier for the external `/v1/*` surface. Stores SHA-256 key
// digests + an optional BYO Anthropic cred; resolves a presented key to an
// `ApiPrincipal` for the P3b policy-resolver. See module docs.
pub mod api_keys;
// Skin capability passes (prd-skin-auth-v1): guest list + hashed `k2skn_`
// tokens in `~/.k2/skin.db`. Overlay Thread rooms; never grid. See module docs.
pub mod skin;
// The App-facing route table shared by the browser helper and the App
// gateway link (runtime S14, AR8). See module docs.
pub mod app_surface;
/// Which body encodings the App gateway link takes on each write.
pub mod app_body;
// Sew S28 §12 / K8: `sew_session_revoke` ops into S27's outbox (sink seam).
pub mod sew_revoke;
// Skin front door (slice 3): Caddy path-filter renderer + apply (nested
// `skin.<sub>.k2.dev` on the existing tunnel; never a second frpc).
pub mod skin_door;
// front443 (0.46.1): the owner's choice to put the box's :443 behind the
// daemon's SNI router (the router lives in k2-daemon).
pub mod front443;
// Sew S27 generic grant hook: `chat` grant kind, durable `tool_outbox` in
// skin.db, reconcile diff. Decisions here; Sew IO in the daemon worker.
pub mod tool_hook;
// Garden v3 data sources (prd-garden-v3-external-data-cards-v1): HTTP JSON
// checks the daemon polls, their state, card view and triggers. Pure half;
// the poller and routes live in k2-daemon. See module docs.
pub mod sources;
// Sew chat for agents (`k2 chat`, slice S30 core): levels, approval modes,
// addressed rules, limits, and the access / action / approval-queue store
// (migration 0167). Pure half; routes and the Sew seam live in k2-daemon.
pub mod sew_chat;
// Garden data (prd-gardens-local-services-v1 1A, contract-garden-data-v1):
// widget keyspaces in a daemon-owned SQLite file, grants, authorship,
// transactions and snapshots. Pure half; routes live in k2-daemon.
pub mod garden_data;
// Local Grok Bots (0.45.7): agent products on this computer that are not K2
// workspaces. Provider trait + Grok Bot adapter; routes and the poller live
// in k2-daemon. See module docs.
pub mod external_agents;
// Phase 1 (prd-wiki-public-chat-api-loopback-v1): daemon-held host_sessions
// API key for public wiki chat (`~/.k2/wiki-public-chat/`). See module docs.
pub mod wiki_public_chat;
// Headless-friendly daemon lifecycle abstractions (label, plist
// generation, launchctl arg vectors). Shared between Tauri's
// `daemon_install` / `daemon_restart` commands and K2 Connect's
// remote-daemon bootstrap path so both agree on the canonical
// label + plist shape.
pub use k2_client::daemon_lifecycle;
pub mod db;
/// Email Notifications' k2.db rows (migration 0227,
/// prd-email-notifications-core-v1, 0.46.5); the rules live in k2-daemon.
pub mod email_notify;
// Runtime stage H: the "handed off, drain only" flag the spawn choke
// points check.
pub mod handoff_state;
/// App runtime stage 1: Seeds, deployments, deploy audit (prd-k2-app-runtime-v1).
pub mod deploy;
/// The disk guard's shared contract (prd-app-resources-disk-guard-v1, 0.46.3).
pub mod disk_guard;
/// Mobile dev: sim sessions, the `SimLane` trait, tree format, control
/// hold, stream wire format (prd-mobile-dev-v1, 0.46.6).
pub mod sim;
// 0.46.4 box usage trends (disk guard Slice B, migration 0217).
pub mod resource_samples;
pub use k2_client::migration_home;
pub use k2_client::paths;
/// Permission cards (prd-agent-settings-cards-v1, 0.46.3 Beta): catalog,
/// request and card-view types plus the two switches. Off by default.
pub mod permission;
pub mod private_fs;
#[cfg(windows)]
pub mod win_fs;
#[cfg(windows)]
pub mod win_proc;
pub mod private_home;
pub mod session_archive;
pub use k2_client::migration_launchd;
pub mod workspace_dir;
pub use workspace_dir::workspace_dot_dir;
pub mod db_ops;
pub mod inject_flow;
// The VALIDATE stage: oxc parse + lints for Garden widgets and Apps
// (decision-k2-runtime-v1 stage 0).
pub mod jscheck;
// Phase 2.5c: retired-but-preserved surface; modules here are
// `#[deprecated]` and exist only for the deprecation window.
pub mod deprecated;
pub use k2_client::editors;
// Cross-Server Agent Comms (federation) — Phase 1: internal cryptographic +
// trust-store primitives ONLY (TOFU peer store + signed-envelope seal/open).
// Opens NO network trust boundary; inert until later phases wire ingress.
// Gated behind the default-OFF `K2_FEDERATION` flag (see `federation::enabled`).
pub mod federation;
// Feedback F1 (prd-agent-feedback-notifications) — durable agent→human
// asks (`k2 feedback ask`) + per-item comment threads.
pub mod feedback;
// Ticket HTML brief (prd-ticket-html-brief-v1): clean + store the
// agent's HTML brief next to its ticket (`feedback_briefs`, 0126).
pub mod feedback_brief;
// Thread survives a tab rename (TR20): one-time repair for fork tabs that
// claimed the source workspace's conversation id before 0.45.1.
pub mod fork_tab_repair;
// Remote Session Layer 0 — master switch + denial audit (default OFF).
pub mod remote_sessions;
pub mod fs_abstract;
pub mod fs_atomic;
pub mod fs_commands;
pub mod file_links;
pub mod secret_paths;
pub mod fs_compress;
pub mod fs_extract;
/// Resumable chunked upload for app guests (part bound to the guest).
pub mod fs_guest_upload;
pub mod git;
/// Changes tab v1: snapshot, both-sides diff, discard-to-Trash, rev.
pub mod git_changes;
/// Changes tab v1.1 (C5): review notes left on changed lines for the agent.
pub mod git_notes;
/// The one builder for K2-run `git`: no hooks, fsmonitor, ext diff or textconv (SS15).
pub mod git_safe;
/// Ensure global git user.name / user.email when missing (init-git path).
pub mod git_identity;
/// First run without git: deferred git steps, the CLT install, agents' note (0.46.4).
pub mod git_tools;
// Phase 2.5c: workspace heartbeat schedules + launchd + cron live here
// post-refactor. Was [`agents::heartbeat`]; the `agents/` back-compat
// alias remains for one release cycle.
pub mod heartbeats;
// `k2 job`: daemon-run scheduled jobs, store + due evaluator (0220).
pub mod jobs;
// Agent hire through the v1 API: one core for every door (0.45.6).
pub mod hire;
/// Home avatar cache: `~/.k2/cache/agent-avatars/` (prd-home-picker-and-remote-avatars-v1 S3).
pub mod home_avatars;
/// homes-sync-v1 (0.46.1): a person's Homes synced between their devices.
pub mod homes_sync;
// Phase 2.5c: workspace lifecycle + state + identity + launch helpers.
// Post-Phase-2.1 "workspace == agent" 1:1 means these are
// workspace-scoped concerns; was scattered under `agents/`.
pub mod workspace;
pub mod workspace_compose_history;
/// Durable sidecar handles (`sales/1`, `sales/reviewer`).
pub mod workspace_session_handles;
/// k2 sidecar v1 — names, brief, per-harness resume, layout, discovery.
pub mod sidecar;
/// Overlay threads + chatter (prd-overlay-threads-v1 S1).
pub mod overlay;
pub mod hook_config;
pub mod inbox;
/// Per-workspace knowledge base (brain map) under `.k2/wiki/`.
pub mod wiki;
// Phase 2.5c: historical migration helpers (one-shot, sentinel-gated).
pub mod migrations;
pub mod llm;
/// LLM login wallet: one active login per tool, idle logins in
/// `~/.k2/llm-accounts` (`k2 llm accounts`, Settings → LLMs).
pub mod llm_accounts;
/// App users' personal wallets (prd-app-user-llm-logins-v1 rev 2, 0.46.3).
pub mod person_wallet;
pub mod sealed_slots;
/// macOS keychain generic passwords: every write by `/usr/bin/security`
/// with the secret on stdin (keeps the item's `apple-tool:` partition),
/// no read that could prompt, every write read back (Claude's live login,
/// Connect session, companion).
#[cfg(target_os = "macos")]
pub use k2_client::macos_keychain;
// The keychain tests stay here: they walk k2-core's sources and use the
// companion / llm_accounts / Connect item options.
#[cfg(all(test, target_os = "macos"))]
#[path = "macos_keychain_tests.rs"]
mod macos_keychain_tests;
// K2 Mail (prd-email-server-v1 pre-mortem #14) — the ONE mail-domain
// normalization helper (lowercase punycode A-label) used at every
// boundary. Store only what it returns; display-decode at the edge.
pub mod mail_domain;
// 0.40.34 — browser-open shim staging (~/.k2/bin/k2-open): xdg-open /
// $BROWSER inside a K2 session surfaces the URL in the connected app
// instead of launching a browser on the daemon's (possibly headless) box.
pub mod open_shim;
pub use k2_client::perf;
pub use k2_client::{perf_hist, perf_timer};
// MasterControl air-gap / LAN listen (prd-air-gap-and-lan-listen-v1).
pub mod airgap;
pub mod listen;
// Phase 2.5 follow-up — daemon port stability (sub-fix A of finding #547).
pub mod port_claim;
// prd-daemon-activity-and-thread-working-v1 DA14: live process lookup for
// the hook owner check (macOS proc_pidinfo, Linux /proc).
pub mod proc_ancestry;
pub mod project_config;
pub mod project_dashboard_layout;
// Linux package SHA256SUMS parsing + verification (desktop updater pkexec
// install refuses a package without a matching line).
pub use k2_client::release_sums;
// Projects V1 P1 (prd-projects-v1) — named GROUPS of workspaces
// (`project_groups`, NOT the legacy `projects` workspace registry):
// membership + PoC rules, the per-group chat stream, and canonical
// shared dashboards.
pub mod project_groups;
pub mod projects_ops;
/// Daemon-owned published services (`published_services` / migration 0104).
pub mod published_services;
/// Who may frame a K2 web page: the one `frame-ancestors` policy for the
/// daemon, the App gateway helper and the Caddy front door.
pub mod frame_policy;
/// Host-wide custom-domain inventory (`domain_bindings` / `domain_names`, 0120).
pub mod domains;
/// Daemon-owned workspace resources (`workspace_resources` / migration 0105).
pub mod workspace_resources;
pub mod push;
// Thread push prefs + recipients (prd-thread-push-notifications-v1,
// migration 0157): person keys, effective switches, recipients.
pub mod notify_prefs;
// Unread counts for Threads and project chats (thread-read-v1, migration
// 0190): per-person read cursors, mutes, derived counts.
pub mod read_state;
pub mod person_color;
pub mod owner_account_prompt;
pub mod person_name;
// Companion C4 (prd-companion-v2 §4) — the daemon-held mobile
// push-device registry (`push_devices`): upsert-by-device-id
// registration, dead-token pruning. The `push::K2Cloud` adapter
// reads it at dispatch time.
pub mod push_devices;
// End-to-end encrypted pushes (prd-e2e-push-v1 rev 2, migration 0211): the
// per-device P-256 public key + PSK registry.
pub mod push_device_keys;
pub mod review_checklist;
pub mod safe_delete;
// 0.39.0: test-aware trash wrapper for production call sites that are
// exercised by cargo tests + bash CLI sandbox tests. Bypasses the
// `trash` crate (and the macOS AppleEvents Touch ID prompt) for
// paths under `std::env::temp_dir()`. Production user-content paths
// keep going through `safe_delete::trash`. See module docs.
pub mod safe_delete_scratch;
pub mod scheduler;
pub mod skill_layers;
// Phase 2.5b — unified home for documentation profiles. Consolidates
// the three legacy folders (`.k2so/agents/`, `.k2so/agent-templates/`,
// bare-md `.k2so/skills/*.md`) into `.k2so/skills/<name>/SKILL.md`
// via a first-boot daemon migration. See [`skills`] module docs.
pub mod skills;
pub mod terminal;
pub mod themes;
// prd-daemon-activity-and-thread-working-v1 S3: the one transcript
// follower the Chat view and the activity store share.
pub mod transcript_follow;
// K2 Connect tunnel connector (source-available client side): renders an frpc
// config + supervises the `frpc` child that dials our hosted frps so the
// local daemon is reachable at https://<user>.k2.dev. See module docs +
// the proprietary `k2-connect` repo for the server contract.
pub mod tunnel;
pub use k2_client::wake;
pub mod whats_new;
// Zen Mode v1 (prd-zen-mode-v1): `~/.k2/zen/` schema, validation,
// last-good resolve, `.history/` and the `k2-zen` skill body.
pub mod zen;

// Session Stream primitives — landed in 0.34.0 behind a feature flag;
// the flag was retired in 0.39.0e (these are always-on now). See
// .k2so/prds/session-stream-and-awareness-bus.md for the original rollout.
pub mod awareness;
pub use k2_client::browser_notes;
pub mod session;
pub mod term;

// Process bootstrap (login-shell PATH, RLIMIT_NOFILE) lives in
// `k2_client::process` so the thin client runs the same code; re-exported
// at the old `k2_core::*` paths.
#[cfg(unix)]
pub use k2_client::process::{enrich_path_from_login_shell, login_shell_path, nofile_raise_target};
pub use k2_client::process::{nofile_soft_limit, raise_nofile_limit};

/// K-M13: a listener's concurrent-connection cap sized from the fd budget
/// instead of a fixed number. Each spliced connection holds two fds (the
/// client and its backend), and two listeners (the E2E TLS listener and
/// the :443 front) share the process with PTYs, watchers and the DB, so
/// each gets an eighth of the soft limit in connections — a quarter of the
/// fds — clamped to `[floor, ceiling]`. With systemd's raised soft limit
/// (524288) that is the ceiling; with an un-raised 1024 it is the floor
/// (256, what the E2E listener always had).
pub fn connection_cap_for(soft_limit: Option<u64>, floor: usize, ceiling: usize) -> usize {
    match soft_limit {
        Some(soft) => usize::try_from(soft / 8).unwrap_or(usize::MAX).clamp(floor, ceiling),
        None => floor,
    }
}

/// [`connection_cap_for`] on this process's current soft limit.
pub fn connection_cap(floor: usize, ceiling: usize) -> usize {
    connection_cap_for(nofile_soft_limit(), floor, ceiling)
}

#[cfg(test)]
mod connection_cap_tests {
    use super::connection_cap_for;

    #[test]
    fn caps_follow_the_fd_budget_within_bounds() {
        assert_eq!(connection_cap_for(Some(1024), 256, 16_384), 256, "un-raised: the old fixed 256");
        assert_eq!(connection_cap_for(Some(256), 256, 16_384), 256, "never below the floor");
        assert_eq!(connection_cap_for(Some(65_536), 256, 16_384), 8_192);
        assert_eq!(connection_cap_for(Some(524_288), 256, 16_384), 16_384, "systemd's raised limit: the ceiling");
        assert_eq!(connection_cap_for(Some(u64::MAX), 256, 16_384), 16_384, "unlimited");
        assert_eq!(connection_cap_for(None, 256, 16_384), 256);
    }

    #[cfg(unix)]
    #[test]
    fn raising_reaches_the_platform_target() {
        super::raise_nofile_limit();
        let mut rlim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: valid out-pointer.
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) }, 0);
        let want = super::nofile_raise_target(rlim.rlim_max);
        // launchd's 256 must not survive the raise on macOS (the old code
        // tried soft = RLIM_INFINITY there and silently kept 256).
        // At the target, or already above it (a shell with `ulimit -n unlimited`).
        assert!(rlim.rlim_cur >= want, "soft RLIMIT_NOFILE {} not raised to the platform target {want}", rlim.rlim_cur);
        #[cfg(target_os = "macos")]
        assert!(rlim.rlim_cur >= 10_240, "macOS soft limit {} after the raise", rlim.rlim_cur);
    }
}

#[doc(hidden)]
pub fn __scaffolding_marker() {}

#[cfg(all(test, unix))]
mod path_enrichment_tests {
    //! Regression tests for the launchd-PATH gap that broke v0.35.0
    //! production: launchd hands `/Applications/K2SO.app` and the daemon
    //! a sparse PATH (`/usr/bin:/bin:/usr/sbin:/sbin`). Spawns of user-
    //! installed tools like `claude` failed with ENOENT until the
    //! `enrich_path_from_login_shell` helper landed in 0.35.1.
    //!
    //! Hermetic: the login shell is a FAKE (a temp script) that prints a
    //! known, marked PATH. The real login shell is never run, so the result
    //! never depends on whoever runs the suite (their rc files, or whether
    //! `/bin/zsh` exists: the Linux gate box has no zsh, and `env -i` leaves
    //! `SHELL` unset). The capture is tested through the pure
    //! [`login_shell_path`]; only one test drives the process-level wrapper,
    //! and its fake PATH only ADDS a dir to the current PATH, so concurrent
    //! tests that spawn `sh` / `git` by name never lose a directory.
    use super::*;
    use std::path::{Path, PathBuf};

    /// A temp dir holding an executable `login-shell` script with `body`.
    /// The script ignores its `-ilc <probe>` arguments.
    struct FakeShell {
        dir: PathBuf,
    }

    impl FakeShell {
        fn new(body: &str) -> Self {
            let dir = crate::test_env::unique_temp_path("fake-shell");
            std::fs::create_dir_all(&dir).expect("create fake shell dir");
            // Not fs::write + exec: ETXTBSY under parallel forks (see
            // test_env::write_executable).
            crate::test_env::write_executable(&dir.join("login-shell"), &format!("#!/bin/sh\n{body}\n"));
            Self { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.join("login-shell")
        }
    }

    impl Drop for FakeShell {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A shell command that prints `path` between the probe markers.
    fn marked(path: &str) -> String {
        format!(
            "printf '%s' '{}{path}{}'",
            terminal::login_path::LOGIN_PATH_BEGIN,
            terminal::login_path::LOGIN_PATH_END
        )
    }

    #[test]
    fn enrich_path_widens_sparse_launchd_default() {
        let shell = FakeShell::new(&format!(
            "echo 'Last login: noise from an rc file'\n{}",
            marked("/opt/k2-test/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        ));
        assert_eq!(
            login_shell_path(&shell.path()).as_deref(),
            Some("/opt/k2-test/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        );
    }

    #[test]
    fn enrich_path_ignores_a_junk_preamble() {
        // The 0.45.0 smoke bug: zsh printed ~11 KB of completion-function
        // source before the PATH. Only the marked PATH may be adopted.
        let shell = FakeShell::new(&format!(
            "i=0\nwhile [ $i -lt 200 ]; do echo 'compdump () {{ local _d_file _d_f _d_bks _d_line _d_als _d_files _d_name _d_tmp; }}'; i=$((i+1)); done\n{}",
            marked("/opt/k2-test/bin:/usr/bin:/bin")
        ));
        assert_eq!(login_shell_path(&shell.path()).as_deref(), Some("/opt/k2-test/bin:/usr/bin:/bin"));
    }

    #[test]
    fn enrich_path_keeps_path_when_the_shell_fails() {
        let shell = FakeShell::new(&format!("{}\nexit 3", marked("/opt/k2-test/bin:/usr/bin")));
        assert_eq!(login_shell_path(&shell.path()), None, "non-zero exit adopts nothing");
    }

    #[test]
    fn enrich_path_keeps_path_when_the_shell_is_missing() {
        let missing = crate::test_env::unique_temp_path("no-such-shell");
        assert_eq!(login_shell_path(Path::new(&missing)), None, "spawn failure adopts nothing");
    }

    #[test]
    fn enrich_path_keeps_path_without_markers() {
        let shell = FakeShell::new("echo /opt/k2-test/bin:/usr/bin");
        assert_eq!(login_shell_path(&shell.path()), None, "unmarked output adopts nothing");
    }

    #[test]
    fn enrich_path_safe_to_call_multiple_times() {
        // The process-level wrapper: adopts the fake login PATH, and a
        // second call changes nothing. The fake PATH is the current PATH
        // plus one dir, so concurrent spawns by name keep working.
        let _env = crate::test_env::lock();
        let current = std::env::var("PATH").expect("PATH is set in the test env");
        let widened = format!("/opt/k2-test/bin:{current}");
        let want = terminal::login_path::extract_login_path(&format!(
            "{}{widened}{}",
            terminal::login_path::LOGIN_PATH_BEGIN,
            terminal::login_path::LOGIN_PATH_END
        ))
        .expect("current PATH has safe entries");
        let shell = FakeShell::new(&marked(&widened));
        let _shell = crate::test_env::EnvVar::set("SHELL", shell.path());
        let _path = crate::test_env::EnvVar::set("PATH", &current);
        enrich_path_from_login_shell();
        let after_one = std::env::var("PATH").expect("PATH after first enrich");
        enrich_path_from_login_shell();
        let after_two = std::env::var("PATH").expect("PATH after second enrich");
        assert_eq!(after_one, want);
        assert!(after_one.starts_with("/opt/k2-test/bin"), "{after_one}");
        assert_eq!(after_two, after_one, "a second call must not grow or change PATH");
    }
}

// K2 test rules (2026-10-06): tests panic when they can reach the
// production daemon (inherited session env, or the real home's port/token).
#[cfg(any(test, feature = "test-util"))]
pub mod test_isolation;

// Quiet-gate PRD §5.3 (0.45.1): the ONE lock + RAII guards for env
// mutation in tests (HOME, PATH, SHELL, K2_*). k2-daemon re-uses it.
#[cfg(any(test, feature = "test-util"))]
pub mod test_env;
